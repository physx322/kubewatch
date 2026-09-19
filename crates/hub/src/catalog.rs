//! Catalogue intégré d'applications prêtes à déployer.
//!
//! Chaque entrée est épinglée sur un tag stable connu plutôt que sur `latest` : un déploiement
//! doit être reproductible, et le détecteur de mises à jour se charge ensuite de proposer les
//! versions plus récentes. Les variables d'environnement listées sont celles qu'il faut
//! renseigner pour que l'application démarre réellement.

use crate::model::{CatalogApp, ChartSummary};
use once_cell::sync::Lazy;

/// Base des icônes, servies par le CDN public de Simple Icons.
const ICON_CDN: &str = "https://cdn.simpleicons.org";

/// Construit une entrée de catalogue de façon compacte et lisible.
#[allow(clippy::too_many_arguments)]
fn app(
    id: &str,
    name: &str,
    description: &str,
    category: &str,
    image: &str,
    default_port: Option<u16>,
    env: &[(&str, &str)],
    needs_pvc: bool,
    icon_slug: Option<&str>,
    chart: Option<(&str, &str, &str, &str)>,
    docs_url: &str,
) -> CatalogApp {
    CatalogApp {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        icon: icon_slug.map(|slug| format!("{ICON_CDN}/{slug}")),
        category: category.to_string(),
        image: image.to_string(),
        default_port,
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        needs_pvc,
        chart: chart.map(|(repository, chart_name, version, repo_url)| ChartSummary {
            name: chart_name.to_string(),
            repository: repository.to_string(),
            version: version.to_string(),
            app_version: None,
            description: None,
            icon: None,
            stars: None,
            home: None,
            repo_url: Some(repo_url.to_string()),
        }),
        docs_url: Some(docs_url.to_string()),
    }
}

/// Dépôt Helm de Bitnami, utilisé par plusieurs entrées.
const BITNAMI: &str = "https://charts.bitnami.com/bitnami";

/// Catalogue statique, construit une seule fois au premier accès.
static APPS: Lazy<Vec<CatalogApp>> = Lazy::new(|| {
    vec![
        app(
            "nginx",
            "NGINX",
            "Serveur web et proxy inverse, idéal pour servir des fichiers statiques ou tester \
             rapidement un Ingress.",
            "Serveur web",
            "nginx:1.27.3-alpine",
            Some(80),
            &[],
            false,
            Some("nginx"),
            Some(("bitnami", "nginx", "18.3.5", BITNAMI)),
            "https://hub.docker.com/_/nginx",
        ),
        app(
            "traefik",
            "Traefik",
            "Contrôleur d'entrée et proxy inverse dynamique, avec certificats Let's Encrypt \
             automatiques.",
            "Réseau",
            "traefik:v3.2.3",
            Some(80),
            &[],
            false,
            Some("traefikproxy"),
            Some((
                "traefik",
                "traefik",
                "33.2.1",
                "https://traefik.github.io/charts",
            )),
            "https://doc.traefik.io/traefik/",
        ),
        app(
            "postgresql",
            "PostgreSQL",
            "Base de données relationnelle de référence. Le mot de passe administrateur est \
             obligatoire au premier démarrage.",
            "Base de données",
            "postgres:16.6-alpine",
            Some(5432),
            &[
                ("POSTGRES_PASSWORD", "changez-moi"),
                ("POSTGRES_USER", "postgres"),
                ("POSTGRES_DB", "app"),
                ("PGDATA", "/var/lib/postgresql/data/pgdata"),
            ],
            true,
            Some("postgresql"),
            Some(("bitnami", "postgresql", "16.3.0", BITNAMI)),
            "https://hub.docker.com/_/postgres",
        ),
        app(
            "redis",
            "Redis",
            "Cache clé-valeur en mémoire, également utilisable comme file d'attente légère.",
            "Cache",
            "redis:7.4.1-alpine",
            Some(6379),
            &[],
            true,
            Some("redis"),
            Some(("bitnami", "redis", "20.6.1", BITNAMI)),
            "https://hub.docker.com/_/redis",
        ),
        app(
            "mysql",
            "MySQL",
            "Base de données relationnelle très répandue, compatible avec la plupart des CMS.",
            "Base de données",
            "mysql:8.4.3",
            Some(3306),
            &[
                ("MYSQL_ROOT_PASSWORD", "changez-moi"),
                ("MYSQL_DATABASE", "app"),
                ("MYSQL_USER", "app"),
                ("MYSQL_PASSWORD", "changez-moi"),
            ],
            true,
            Some("mysql"),
            Some(("bitnami", "mysql", "12.2.2", BITNAMI)),
            "https://hub.docker.com/_/mysql",
        ),
        app(
            "mariadb",
            "MariaDB",
            "Fork communautaire de MySQL, souvent préféré pour les charges auto-hébergées.",
            "Base de données",
            "mariadb:11.6.2",
            Some(3306),
            &[
                ("MARIADB_ROOT_PASSWORD", "changez-moi"),
                ("MARIADB_DATABASE", "app"),
                ("MARIADB_USER", "app"),
                ("MARIADB_PASSWORD", "changez-moi"),
            ],
            true,
            Some("mariadb"),
            Some(("bitnami", "mariadb", "20.2.1", BITNAMI)),
            "https://hub.docker.com/_/mariadb",
        ),
        app(
            "mongodb",
            "MongoDB",
            "Base de données orientée documents, adaptée aux schémas évolutifs.",
            "Base de données",
            "mongo:7.0.15",
            Some(27017),
            &[
                ("MONGO_INITDB_ROOT_USERNAME", "root"),
                ("MONGO_INITDB_ROOT_PASSWORD", "changez-moi"),
            ],
            true,
            Some("mongodb"),
            Some(("bitnami", "mongodb", "16.4.0", BITNAMI)),
            "https://hub.docker.com/_/mongo",
        ),
        app(
            "grafana",
            "Grafana",
            "Tableaux de bord et alertes pour vos métriques et journaux.",
            "Observabilité",
            "grafana/grafana:11.4.0",
            Some(3000),
            &[
                ("GF_SECURITY_ADMIN_USER", "admin"),
                ("GF_SECURITY_ADMIN_PASSWORD", "changez-moi"),
            ],
            true,
            Some("grafana"),
            Some((
                "grafana",
                "grafana",
                "8.8.2",
                "https://grafana.github.io/helm-charts",
            )),
            "https://grafana.com/docs/grafana/latest/",
        ),
        app(
            "prometheus",
            "Prometheus",
            "Collecte et stockage de métriques par interrogation périodique des cibles.",
            "Observabilité",
            "prom/prometheus:v3.0.1",
            Some(9090),
            &[],
            true,
            Some("prometheus"),
            Some((
                "prometheus-community",
                "prometheus",
                "26.0.0",
                "https://prometheus-community.github.io/helm-charts",
            )),
            "https://prometheus.io/docs/introduction/overview/",
        ),
        app(
            "loki",
            "Grafana Loki",
            "Agrégation de journaux indexée par étiquettes, pensée pour Kubernetes.",
            "Observabilité",
            "grafana/loki:3.3.1",
            Some(3100),
            &[],
            true,
            Some("grafana"),
            Some((
                "grafana",
                "loki",
                "6.24.0",
                "https://grafana.github.io/helm-charts",
            )),
            "https://grafana.com/docs/loki/latest/",
        ),
        app(
            "minio",
            "MinIO",
            "Stockage objet compatible S3, utile comme cible de sauvegarde ou de données.",
            "Stockage",
            "minio/minio:RELEASE.2024-11-07T00-52-20Z",
            Some(9000),
            &[
                ("MINIO_ROOT_USER", "minio"),
                ("MINIO_ROOT_PASSWORD", "changez-moi-12-caracteres"),
            ],
            true,
            Some("minio"),
            Some(("bitnami", "minio", "14.8.5", BITNAMI)),
            "https://min.io/docs/minio/kubernetes/upstream/",
        ),
        app(
            "n8n",
            "n8n",
            "Automatisation de flux de travail par blocs, alternative auto-hébergée à Zapier.",
            "Automatisation",
            "n8nio/n8n:1.72.1",
            Some(5678),
            &[
                ("N8N_HOST", "n8n.exemple.local"),
                ("N8N_PORT", "5678"),
                ("N8N_PROTOCOL", "https"),
                ("GENERIC_TIMEZONE", "Europe/Paris"),
            ],
            true,
            Some("n8n"),
            None,
            "https://docs.n8n.io/hosting/",
        ),
        app(
            "gitea",
            "Gitea",
            "Forge Git légère avec interface web, tickets et registre de paquets.",
            "Développement",
            "gitea/gitea:1.22.6",
            Some(3000),
            &[
                ("GITEA__database__DB_TYPE", "sqlite3"),
                ("GITEA__server__ROOT_URL", "https://git.exemple.local/"),
            ],
            true,
            Some("gitea"),
            Some((
                "gitea-charts",
                "gitea",
                "10.6.0",
                "https://dl.gitea.com/charts/",
            )),
            "https://docs.gitea.com/installation/install-with-docker",
        ),
        app(
            "vaultwarden",
            "Vaultwarden",
            "Gestionnaire de mots de passe compatible avec les clients Bitwarden.",
            "Sécurité",
            "vaultwarden/server:1.32.7",
            Some(80),
            &[
                ("DOMAIN", "https://coffre.exemple.local"),
                ("SIGNUPS_ALLOWED", "false"),
                ("ADMIN_TOKEN", "changez-moi"),
            ],
            true,
            Some("vaultwarden"),
            None,
            "https://github.com/dani-garcia/vaultwarden/wiki",
        ),
        app(
            "uptime-kuma",
            "Uptime Kuma",
            "Supervision de disponibilité avec notifications, interface simple et légère.",
            "Observabilité",
            "louislam/uptime-kuma:1.23.16",
            Some(3001),
            &[],
            true,
            Some("uptimekuma"),
            None,
            "https://github.com/louislam/uptime-kuma/wiki",
        ),
        app(
            "jellyfin",
            "Jellyfin",
            "Serveur multimédia libre pour films, séries et musique.",
            "Médias",
            "jellyfin/jellyfin:10.10.3",
            Some(8096),
            &[("TZ", "Europe/Paris")],
            true,
            Some("jellyfin"),
            None,
            "https://jellyfin.org/docs/general/installation/container/",
        ),
        app(
            "nextcloud",
            "Nextcloud",
            "Suite collaborative : fichiers, agenda, contacts et documents partagés.",
            "Productivité",
            "nextcloud:30.0.2-apache",
            Some(80),
            &[
                ("NEXTCLOUD_ADMIN_USER", "admin"),
                ("NEXTCLOUD_ADMIN_PASSWORD", "changez-moi"),
                ("NEXTCLOUD_TRUSTED_DOMAINS", "nuage.exemple.local"),
            ],
            true,
            Some("nextcloud"),
            Some((
                "nextcloud",
                "nextcloud",
                "6.6.2",
                "https://nextcloud.github.io/helm/",
            )),
            "https://hub.docker.com/_/nextcloud",
        ),
        app(
            "rabbitmq",
            "RabbitMQ",
            "Courtier de messages AMQP avec console de gestion intégrée.",
            "Messagerie",
            "rabbitmq:4.0.5-management",
            Some(5672),
            &[
                ("RABBITMQ_DEFAULT_USER", "rabbit"),
                ("RABBITMQ_DEFAULT_PASS", "changez-moi"),
            ],
            true,
            Some("rabbitmq"),
            Some(("bitnami", "rabbitmq", "15.2.0", BITNAMI)),
            "https://www.rabbitmq.com/kubernetes/operator/operator-overview",
        ),
        app(
            "memcached",
            "Memcached",
            "Cache mémoire distribué, très simple à exploiter et sans persistance.",
            "Cache",
            "memcached:1.6.32-alpine",
            Some(11211),
            &[],
            false,
            None,
            Some(("bitnami", "memcached", "7.6.1", BITNAMI)),
            "https://hub.docker.com/_/memcached",
        ),
        app(
            "cert-manager",
            "cert-manager",
            "Émission et renouvellement automatiques de certificats TLS dans le cluster. \
             À installer de préférence par son chart, qui pose aussi les CRD nécessaires.",
            "Sécurité",
            "quay.io/jetstack/cert-manager-controller:v1.16.2",
            None,
            &[],
            false,
            None,
            Some((
                "jetstack",
                "cert-manager",
                "v1.16.2",
                "https://charts.jetstack.io",
            )),
            "https://cert-manager.io/docs/installation/helm/",
        ),
        app(
            "metrics-server",
            "Metrics Server",
            "Fournit les métriques CPU et mémoire consommées par « kubectl top » et par les \
             tableaux de bord de KubeWatch.",
            "Kubernetes",
            "registry.k8s.io/metrics-server/metrics-server:v0.7.2",
            Some(4443),
            &[],
            false,
            Some("kubernetes"),
            Some((
                "metrics-server",
                "metrics-server",
                "3.12.2",
                "https://kubernetes-sigs.github.io/metrics-server/",
            )),
            "https://github.com/kubernetes-sigs/metrics-server",
        ),
        app(
            "portainer",
            "Portainer",
            "Console de gestion de conteneurs et de clusters, complémentaire à KubeWatch.",
            "Kubernetes",
            "portainer/portainer-ce:2.21.4",
            Some(9000),
            &[],
            true,
            Some("portainer"),
            Some((
                "portainer",
                "portainer",
                "1.0.60",
                "https://portainer.github.io/k8s/",
            )),
            "https://docs.portainer.io/start/install-ce/server/kubernetes",
        ),
        app(
            "homepage",
            "Homepage",
            "Page d'accueil configurable regroupant les liens et l'état de vos services.",
            "Productivité",
            "ghcr.io/gethomepage/homepage:v0.10.9",
            Some(3000),
            &[("HOMEPAGE_ALLOWED_HOSTS", "accueil.exemple.local")],
            true,
            None,
            None,
            "https://gethomepage.dev/installation/k8s/",
        ),
        app(
            "adminer",
            "Adminer",
            "Client web léger pour administrer PostgreSQL, MySQL ou MariaDB.",
            "Développement",
            "adminer:4.8.1",
            Some(8080),
            &[("ADMINER_DEFAULT_SERVER", "postgresql")],
            false,
            None,
            None,
            "https://hub.docker.com/_/adminer",
        ),
    ]
});

/// Applications prêtes à déployer, livrées avec KubeWatch.
pub fn builtin_apps() -> &'static [CatalogApp] {
    APPS.as_slice()
}

/// Retrouve une application du catalogue par son identifiant.
pub fn find_app(id: &str) -> Option<&'static CatalogApp> {
    let id = id.trim();
    builtin_apps().iter().find(|a| a.id == id)
}

/// Catégories présentes dans le catalogue, triées par ordre alphabétique.
pub fn categories() -> Vec<String> {
    let mut out: Vec<String> = builtin_apps().iter().map(|a| a.category.clone()).collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ImageRef;

    #[test]
    fn catalogue_suffisamment_fourni() {
        assert!(
            builtin_apps().len() >= 18,
            "le catalogue doit proposer au moins 18 applications, il en compte {}",
            builtin_apps().len()
        );
    }

    #[test]
    fn identifiants_uniques() {
        let mut ids: Vec<&str> = builtin_apps().iter().map(|a| a.id.as_str()).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), total, "deux applications partagent le même identifiant");
    }

    #[test]
    fn images_epinglees_et_analysables() {
        for a in builtin_apps() {
            let image = ImageRef::parse(&a.image)
                .unwrap_or_else(|e| panic!("image invalide pour « {} » : {e}", a.id));
            assert_ne!(
                image.tag.as_deref(),
                Some("latest"),
                "« {} » doit être épinglée sur une version précise",
                a.id
            );
        }
    }

    #[test]
    fn metadonnees_completes() {
        for a in builtin_apps() {
            assert!(!a.name.is_empty(), "nom manquant pour « {} »", a.id);
            assert!(!a.description.is_empty(), "description manquante pour « {} »", a.id);
            assert!(!a.category.is_empty(), "catégorie manquante pour « {} »", a.id);
            assert!(a.docs_url.is_some(), "documentation manquante pour « {} »", a.id);
            if let Some(port) = a.default_port {
                assert!(port >= 1, "port invalide pour « {} »", a.id);
            }
        }
    }

    #[test]
    fn recherche_par_identifiant() {
        assert!(find_app("postgresql").is_some());
        assert!(find_app("  redis  ").is_some());
        assert!(find_app("inexistant").is_none());
    }

    #[test]
    fn categories_triees_et_uniques() {
        let cats = categories();
        let mut trie = cats.clone();
        trie.sort();
        trie.dedup();
        assert_eq!(cats, trie);
        assert!(cats.contains(&"Base de données".to_string()));
    }
}
