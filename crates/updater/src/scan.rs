//! Inventaire des images déployées dans un cluster et proposition automatique
//! de surveillants (« watchers ») de mise à jour.

use crate::error::Result;
use crate::model::{UpdatePolicy, UpdateSource, WatchTarget, WatcherSpec};
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::CronJob;
use k8s_openapi::api::core::v1::PodSpec;
use kube::api::{Api, ListParams, ResourceExt};
use kubewatch_core::ClusterHandle;
use serde::{Deserialize, Serialize};

/// Une image utilisée par une charge de travail du cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkloadImage {
    /// Nom du cluster KubeWatch d'où provient l'observation.
    pub cluster: String,
    /// Namespace de la charge de travail (`None` pour une ressource non namespacée).
    pub namespace: Option<String>,
    /// Type de la charge de travail: Deployment, StatefulSet, DaemonSet, CronJob.
    pub kind: String,
    /// Nom de la charge de travail.
    pub name: String,
    /// Nom du conteneur (les conteneurs d'initialisation sont inclus).
    pub container: String,
    /// Référence d'image complète telle que déclarée dans le manifeste.
    pub image: String,
    /// Dépôt GitHub « owner/repo » déduit de l'image, quand il est identifiable.
    pub source_repo_guess: Option<String>,
}

/// Construit la cible d'un surveillant.
///
/// Point de construction unique de [`WatchTarget`] pour tout le crate: engine et
/// rollout passent par ici afin de garder une seule définition de la cible.
pub(crate) fn watch_target(namespace: Option<String>, kind: &str, name: &str) -> WatchTarget {
    WatchTarget {
        namespace,
        kind: kind.to_string(),
        name: name.to_string(),
    }
}

/// Liste toutes les images des charges de travail d'un cluster.
///
/// Parcourt les Deployments, StatefulSets, DaemonSets et CronJobs. Un type qui ne
/// peut pas être listé (droits RBAC insuffisants, CRD absent) est journalisé puis
/// ignoré: l'inventaire reste partiel plutôt que de tomber en erreur.
pub async fn scan_workloads(
    h: &ClusterHandle,
    namespace: Option<&str>,
) -> Result<Vec<WorkloadImage>> {
    let mut out: Vec<WorkloadImage> = Vec::new();

    // `$obj` est fourni par l'appelant pour que `$extract` puisse s'y référer
    // (hygiène des macros): cela évite un pointeur de fonction à durée de vie
    // supérieure, que l'inférence de fermeture ne sait pas toujours produire.
    macro_rules! collect {
        ($kind:expr, $ty:ty, $obj:ident => $extract:expr) => {{
            let api: Api<$ty> = match namespace {
                Some(ns) => Api::namespaced(h.client.clone(), ns),
                None => Api::all(h.client.clone()),
            };
            match api.list(&ListParams::default()).await {
                Ok(list) => {
                    for $obj in list {
                        let name = $obj.name_any();
                        let obj_ns = $obj.namespace();
                        if let Some(pod_spec) = $extract {
                            push_containers(
                                &h.name,
                                obj_ns.as_deref(),
                                $kind,
                                &name,
                                pod_spec,
                                &mut out,
                            );
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        cluster = %h.name,
                        kind = %$kind,
                        erreur = %err,
                        "inventaire partiel: ce type de charge de travail n'a pas pu être listé"
                    );
                }
            }
        }};
    }

    collect!("Deployment", Deployment, d => d
        .spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref()));
    collect!("StatefulSet", StatefulSet, s => s
        .spec
        .as_ref()
        .and_then(|inner| inner.template.spec.as_ref()));
    collect!("DaemonSet", DaemonSet, d => d
        .spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref()));
    // NB: dans k8s-openapi 0.28 (v1_36), `CronJob.spec` n'est pas un `Option`,
    // contrairement à Deployment/StatefulSet/DaemonSet. Pas de `.as_ref()` ici.
    collect!("CronJob", CronJob, c => c
        .spec
        .job_template
        .spec
        .as_ref()
        .and_then(|j| j.template.spec.as_ref()));

    out.sort_by(|a, b| {
        a.namespace
            .cmp(&b.namespace)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.container.cmp(&b.container))
    });
    Ok(out)
}

/// Ajoute à `out` une entrée par conteneur (initialisation comprise) du `PodSpec`.
fn push_containers(
    cluster: &str,
    namespace: Option<&str>,
    kind: &str,
    name: &str,
    spec: &PodSpec,
    out: &mut Vec<WorkloadImage>,
) {
    let init = spec.init_containers.iter().flatten();
    for c in init.chain(spec.containers.iter()) {
        let image = match c.image.as_deref().map(str::trim) {
            Some(i) if !i.is_empty() => i.to_string(),
            _ => continue,
        };
        out.push(WorkloadImage {
            cluster: cluster.to_string(),
            namespace: namespace.map(str::to_string),
            kind: kind.to_string(),
            name: name.to_string(),
            container: c.name.clone(),
            source_repo_guess: source_repo_guess(&image),
            image,
        });
    }
}

/// Découpe une référence d'image en `(registre, dépôt, tag)`.
///
/// Applique les conventions Docker: registre implicite `docker.io`, espace de noms
/// implicite `library/` pour les images officielles, digest ignoré.
pub fn split_image(image: &str) -> (String, String, Option<String>) {
    // Le digest prime sur le tag; on le retire pour isoler le chemin.
    let without_digest = image.split('@').next().unwrap_or(image);

    // Un `:` n'introduit un tag que s'il apparaît après le dernier `/`
    // (sinon c'est le port du registre, ex. `registry:5000/app`).
    let (path, tag) = match without_digest.rfind(':') {
        Some(idx) if idx > without_digest.rfind('/').unwrap_or(0) => {
            let (p, t) = without_digest.split_at(idx);
            (p, Some(t[1..].to_string()))
        }
        _ => (without_digest, None),
    };

    let (registry, repository) = match path.split_once('/') {
        Some((head, rest)) if head.contains('.') || head.contains(':') || head == "localhost" => {
            (head.to_ascii_lowercase(), rest.to_string())
        }
        _ => ("docker.io".to_string(), path.to_string()),
    };

    let repository = if registry == "docker.io" && !repository.contains('/') {
        format!("library/{repository}")
    } else {
        repository
    };

    (registry, repository, tag.filter(|t| !t.is_empty()))
}

/// Remplace le tag d'une référence d'image (et retire un éventuel digest).
///
/// Suit les mêmes conventions que [`split_image`]: un `:` n'introduit un tag que
/// s'il apparaît après le dernier `/`, sans quoi c'est le port du registre.
pub fn replace_tag(image: &str, new_tag: &str) -> String {
    let without_digest = image.split('@').next().unwrap_or(image);
    let base = match without_digest.rfind(':') {
        Some(idx) if idx > without_digest.rfind('/').unwrap_or(0) => &without_digest[..idx],
        _ => without_digest,
    };
    format!("{base}:{new_tag}")
}

/// Correspondances `registre/dépôt` -> dépôt GitHub, pour les registres non Docker Hub.
const REGISTRY_REPOS: &[(&str, &str)] = &[
    (
        "registry.k8s.io/metrics-server/metrics-server",
        "kubernetes-sigs/metrics-server",
    ),
    (
        "registry.k8s.io/ingress-nginx/controller",
        "kubernetes/ingress-nginx",
    ),
    (
        "registry.k8s.io/kube-state-metrics/kube-state-metrics",
        "kubernetes/kube-state-metrics",
    ),
    (
        "registry.k8s.io/external-dns/external-dns",
        "kubernetes-sigs/external-dns",
    ),
    (
        "quay.io/jetstack/cert-manager-controller",
        "cert-manager/cert-manager",
    ),
    (
        "quay.io/jetstack/cert-manager-webhook",
        "cert-manager/cert-manager",
    ),
    (
        "quay.io/jetstack/cert-manager-cainjector",
        "cert-manager/cert-manager",
    ),
    ("quay.io/prometheus/prometheus", "prometheus/prometheus"),
    ("quay.io/prometheus/alertmanager", "prometheus/alertmanager"),
    (
        "quay.io/prometheus/node-exporter",
        "prometheus/node_exporter",
    ),
    ("quay.io/argoproj/argocd", "argoproj/argo-cd"),
    ("quay.io/minio/minio", "minio/minio"),
    (
        "docker.elastic.co/elasticsearch/elasticsearch",
        "elastic/elasticsearch",
    ),
    ("docker.elastic.co/kibana/kibana", "elastic/kibana"),
];

/// Correspondances `dépôt Docker Hub` -> dépôt GitHub amont.
const DOCKERHUB_REPOS: &[(&str, &str)] = &[
    ("library/nginx", "nginx/nginx"),
    ("library/traefik", "traefik/traefik"),
    ("library/redis", "redis/redis"),
    ("library/postgres", "postgres/postgres"),
    ("library/mysql", "mysql/mysql"),
    ("library/mariadb", "MariaDB/server"),
    ("library/mongo", "mongodb/mongo"),
    ("library/rabbitmq", "rabbitmq/rabbitmq-server"),
    ("library/memcached", "memcached/memcached"),
    ("library/haproxy", "haproxy/haproxy"),
    ("library/httpd", "apache/httpd"),
    ("library/caddy", "caddyserver/caddy"),
    ("library/registry", "distribution/distribution"),
    ("library/influxdb", "influxdata/influxdb"),
    ("library/consul", "hashicorp/consul"),
    ("library/vault", "hashicorp/vault"),
    ("library/nextcloud", "nextcloud/docker"),
    ("library/node", "nodejs/node"),
    ("library/golang", "golang/go"),
    ("library/python", "python/cpython"),
    ("library/rust", "rust-lang/rust"),
    ("grafana/grafana", "grafana/grafana"),
    ("grafana/loki", "grafana/loki"),
    ("grafana/promtail", "grafana/loki"),
    ("grafana/tempo", "grafana/tempo"),
    ("prom/prometheus", "prometheus/prometheus"),
    ("prom/alertmanager", "prometheus/alertmanager"),
    ("prom/node-exporter", "prometheus/node_exporter"),
    ("prom/blackbox-exporter", "prometheus/blackbox_exporter"),
    ("gitea/gitea", "go-gitea/gitea"),
    ("minio/minio", "minio/minio"),
    ("n8nio/n8n", "n8n-io/n8n"),
    ("vaultwarden/server", "dani-garcia/vaultwarden"),
    ("louislam/uptime-kuma", "louislam/uptime-kuma"),
    ("jellyfin/jellyfin", "jellyfin/jellyfin"),
    ("portainer/portainer-ce", "portainer/portainer"),
    ("homeassistant/home-assistant", "home-assistant/core"),
    ("adguard/adguardhome", "AdguardTeam/AdGuardHome"),
    ("pihole/pihole", "pi-hole/docker-pi-hole"),
    ("authelia/authelia", "authelia/authelia"),
    ("cloudflare/cloudflared", "cloudflare/cloudflared"),
    ("bitwardenrs/server", "dani-garcia/vaultwarden"),
    ("nextcloud/all-in-one", "nextcloud/all-in-one"),
    ("hashicorp/vault", "hashicorp/vault"),
    ("hashicorp/consul", "hashicorp/consul"),
    ("jenkins/jenkins", "jenkinsci/jenkins"),
    ("sonarqube", "SonarSource/sonarqube"),
];

/// Déduit le dépôt GitHub « owner/repo » d'une référence d'image, si possible.
///
/// Trois stratégies, dans l'ordre: les images `ghcr.io` portent directement le
/// chemin GitHub; sinon une table des registres connus; sinon une table Docker Hub.
/// Aucune supposition n'est faite en dehors de ces cas: mieux vaut pas de dépôt
/// qu'un dépôt faux qui proposerait des versions sans rapport.
pub fn source_repo_guess(image: &str) -> Option<String> {
    let (registry, repository, _tag) = split_image(image);

    if registry == "ghcr.io" {
        let mut parts = repository.split('/');
        let owner = parts.next().filter(|s| !s.is_empty())?;
        let repo = parts.next().filter(|s| !s.is_empty())?;
        return Some(format!("{owner}/{repo}"));
    }

    let full = format!("{registry}/{repository}");
    if let Some((_, gh)) = REGISTRY_REPOS.iter().find(|(k, _)| *k == full) {
        return Some((*gh).to_string());
    }

    if registry == "docker.io" {
        if let Some((_, gh)) = DOCKERHUB_REPOS.iter().find(|(k, _)| *k == repository) {
            return Some((*gh).to_string());
        }
        // Les tables listent aussi quelques noms officiels sans `library/`.
        if let Some(short) = repository.strip_prefix("library/") {
            if let Some((_, gh)) = DOCKERHUB_REPOS.iter().find(|(k, _)| *k == short) {
                return Some((*gh).to_string());
            }
        }
    }

    None
}

/// Propose un surveillant par couple (charge de travail, conteneur).
///
/// La source est GitHub Releases dès qu'un dépôt amont est identifiable, sinon le
/// registre de conteneurs. Les images sans tag exploitable (`:latest` notamment)
/// restent surveillées via le registre: leur digest peut changer.
pub fn suggest_watchers(
    images: &[WorkloadImage],
    default_policy: &UpdatePolicy,
) -> Vec<WatcherSpec> {
    let now = chrono::Utc::now();
    let mut out = Vec::with_capacity(images.len());

    for w in images {
        let source = match w.source_repo_guess.as_deref().and_then(split_repo) {
            Some((owner, repo)) => UpdateSource::GithubRelease {
                owner,
                repo,
                tag_prefix: None,
            },
            None => UpdateSource::ContainerRegistry {
                image: w.image.clone(),
            },
        };

        let (_, repository, tag) = split_image(&w.image);
        let short = repository
            .strip_prefix("library/")
            .unwrap_or(&repository)
            .to_string();

        out.push(WatcherSpec {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("{} {}/{}", w.kind, w.name, w.container),
            enabled: true,
            cluster: w.cluster.clone(),
            target: watch_target(w.namespace.clone(), &w.kind, &w.name),
            container: Some(w.container.clone()),
            source,
            policy: default_policy.clone(),
            created_at: now,
            last_checked_at: None,
            last_known_version: tag.or(Some(short)),
        });
    }

    out
}

/// Découpe « owner/repo » en ses deux composantes non vides.
fn split_repo(s: &str) -> Option<(String, String)> {
    let (owner, repo) = s.split_once('/')?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoupe_image_officielle() {
        assert_eq!(
            split_image("nginx:1.27.3"),
            (
                "docker.io".to_string(),
                "library/nginx".to_string(),
                Some("1.27.3".to_string())
            )
        );
    }

    #[test]
    fn decoupe_sans_tag() {
        assert_eq!(
            split_image("redis"),
            ("docker.io".to_string(), "library/redis".to_string(), None)
        );
    }

    #[test]
    fn decoupe_registre_avec_port() {
        // Le `:5000` est un port, pas un tag.
        assert_eq!(
            split_image("registry.local:5000/team/app:2.1.0"),
            (
                "registry.local:5000".to_string(),
                "team/app".to_string(),
                Some("2.1.0".to_string())
            )
        );
        assert_eq!(
            split_image("registry.local:5000/team/app"),
            (
                "registry.local:5000".to_string(),
                "team/app".to_string(),
                None
            )
        );
    }

    #[test]
    fn decoupe_ignore_le_digest() {
        let (reg, repo, tag) = split_image(
            "ghcr.io/kubewatch-io/kubewatch:v1.2.3@sha256:aaaabbbbccccddddeeeeffff0000111122223333444455556666777788889999",
        );
        assert_eq!(reg, "ghcr.io");
        assert_eq!(repo, "kubewatch-io/kubewatch");
        assert_eq!(tag.as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn devine_depot_ghcr() {
        assert_eq!(
            source_repo_guess("ghcr.io/home-assistant/home-assistant:2024.6.1"),
            Some("home-assistant/home-assistant".to_string())
        );
        // Chemin plus profond: on ne garde que les deux premiers segments.
        assert_eq!(
            source_repo_guess("ghcr.io/owner/repo/sub:1.0.0"),
            Some("owner/repo".to_string())
        );
    }

    #[test]
    fn devine_depot_dockerhub() {
        assert_eq!(
            source_repo_guess("grafana/grafana:11.2.0"),
            Some("grafana/grafana".to_string())
        );
        assert_eq!(
            source_repo_guess("prom/prometheus:v2.54.1"),
            Some("prometheus/prometheus".to_string())
        );
        assert_eq!(
            source_repo_guess("nginx:1.27-alpine"),
            Some("nginx/nginx".to_string())
        );
        assert_eq!(
            source_repo_guess("docker.io/library/traefik:v3.1.2"),
            Some("traefik/traefik".to_string())
        );
        assert_eq!(
            source_repo_guess("gitea/gitea:1.22.3"),
            Some("go-gitea/gitea".to_string())
        );
    }

    #[test]
    fn devine_depot_registres_connus() {
        assert_eq!(
            source_repo_guess("registry.k8s.io/metrics-server/metrics-server:v0.7.2"),
            Some("kubernetes-sigs/metrics-server".to_string())
        );
        assert_eq!(
            source_repo_guess("quay.io/jetstack/cert-manager-controller:v1.15.3"),
            Some("cert-manager/cert-manager".to_string())
        );
    }

    #[test]
    fn image_inconnue_sans_depot() {
        assert_eq!(
            source_repo_guess("mon-registre.interne/equipe/api:3.2.1"),
            None
        );
        assert_eq!(source_repo_guess("acme/service-interne:latest"), None);
    }

    #[test]
    fn suggestion_choisit_la_bonne_source() {
        let images = vec![
            WorkloadImage {
                cluster: "prod".to_string(),
                namespace: Some("monitoring".to_string()),
                kind: "Deployment".to_string(),
                name: "grafana".to_string(),
                container: "grafana".to_string(),
                image: "grafana/grafana:11.2.0".to_string(),
                source_repo_guess: source_repo_guess("grafana/grafana:11.2.0"),
            },
            WorkloadImage {
                cluster: "prod".to_string(),
                namespace: Some("apps".to_string()),
                kind: "StatefulSet".to_string(),
                name: "api".to_string(),
                container: "api".to_string(),
                image: "mon-registre.interne/equipe/api:latest".to_string(),
                source_repo_guess: None,
            },
        ];
        let policy = UpdatePolicy::default();
        let watchers = suggest_watchers(&images, &policy);
        assert_eq!(watchers.len(), 2);

        match &watchers[0].source {
            UpdateSource::GithubRelease { owner, repo, .. } => {
                assert_eq!(owner, "grafana");
                assert_eq!(repo, "grafana");
            }
            other => panic!("attendu une source GitHub, obtenu {other:?}"),
        }
        assert_eq!(watchers[0].last_known_version.as_deref(), Some("11.2.0"));
        assert!(watchers[0].enabled);
        assert_eq!(watchers[0].target.kind, "Deployment");
        assert_eq!(watchers[0].target.name, "grafana");
        assert_eq!(watchers[0].target.namespace.as_deref(), Some("monitoring"));

        // `:latest` reste surveillé par le registre: le digest peut bouger.
        match &watchers[1].source {
            UpdateSource::ContainerRegistry { image } => {
                assert_eq!(image, "mon-registre.interne/equipe/api:latest");
            }
            other => panic!("attendu une source registre, obtenu {other:?}"),
        }

        // Les identifiants sont uniques.
        assert_ne!(watchers[0].id, watchers[1].id);
    }
}
