//! Découverte des types de ressources exposés par un cluster.
//!
//! Le catalogue est construit une fois à la connexion puis conservé en cache : il permet de
//! résoudre une saisie utilisateur (`po`, `pods`, `Pod`, `deployments.apps`) vers un
//! [`ResourceKind`] complet, y compris pour les définitions de ressources personnalisées.

use std::collections::{BTreeMap, BTreeSet};

use kube::discovery::{ApiResource, Discovery, Scope};
use kube::Client;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::ResourceKind;

/// Abréviations usuelles de `kubectl`, en complément de celles annoncées par le serveur.
///
/// La valeur est l'identifiant canonique `pluriel` ou `pluriel.groupe`.
const ABBREVIATIONS: &[(&str, &str)] = &[
    ("po", "pods"),
    ("pod", "pods"),
    ("no", "nodes"),
    ("node", "nodes"),
    ("ns", "namespaces"),
    ("cm", "configmaps"),
    ("secret", "secrets"),
    ("svc", "services"),
    ("ep", "endpoints"),
    ("sa", "serviceaccounts"),
    ("pv", "persistentvolumes"),
    ("pvc", "persistentvolumeclaims"),
    ("rc", "replicationcontrollers"),
    ("quota", "resourcequotas"),
    ("limits", "limitranges"),
    ("cs", "componentstatuses"),
    ("ev", "events"),
    ("deploy", "deployments.apps"),
    ("deployment", "deployments.apps"),
    ("rs", "replicasets.apps"),
    ("sts", "statefulsets.apps"),
    ("ds", "daemonsets.apps"),
    ("job", "jobs.batch"),
    ("cj", "cronjobs.batch"),
    ("cronjob", "cronjobs.batch"),
    ("ing", "ingresses.networking.k8s.io"),
    ("ingress", "ingresses.networking.k8s.io"),
    ("netpol", "networkpolicies.networking.k8s.io"),
    ("ingressclass", "ingressclasses.networking.k8s.io"),
    ("hpa", "horizontalpodautoscalers.autoscaling"),
    ("pdb", "poddisruptionbudgets.policy"),
    ("sc", "storageclasses.storage.k8s.io"),
    ("crd", "customresourcedefinitions.apiextensions.k8s.io"),
    ("crds", "customresourcedefinitions.apiextensions.k8s.io"),
    ("cr", "clusterroles.rbac.authorization.k8s.io"),
    ("crb", "clusterrolebindings.rbac.authorization.k8s.io"),
    ("rb", "rolebindings.rbac.authorization.k8s.io"),
    ("role", "roles.rbac.authorization.k8s.io"),
    ("csr", "certificatesigningrequests.certificates.k8s.io"),
    ("pc", "priorityclasses.scheduling.k8s.io"),
    ("rc-class", "runtimeclasses.node.k8s.io"),
];

/// Catalogue des types de ressources connus d'un cluster.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceCatalog {
    /// Types découverts, triés par groupe puis par kind.
    pub kinds: Vec<ResourceKind>,
}

impl ResourceCatalog {
    /// Interroge le serveur d'API et construit le catalogue complet.
    ///
    /// Les abréviations et catégories, absentes de l'API de découverte de `kube`, sont
    /// complétées par une lecture directe des `APIResourceList`. Cet enrichissement est
    /// facultatif : son échec n'invalide pas le catalogue.
    pub async fn build(client: &Client) -> Result<Self> {
        let discovery = Discovery::new(client.clone())
            .run()
            .await
            .map_err(|e| Error::Discovery(e.to_string()))?;

        let mut kinds = Vec::new();
        for group in discovery.groups() {
            for (ar, caps) in group.recommended_resources() {
                kinds.push(ResourceKind {
                    group: ar.group.clone(),
                    version: ar.version.clone(),
                    kind: ar.kind.clone(),
                    plural: ar.plural.clone(),
                    namespaced: caps.scope == Scope::Namespaced,
                    verbs: caps.operations.clone(),
                    short_names: Vec::new(),
                    categories: Vec::new(),
                });
            }
        }

        if kinds.is_empty() {
            return Err(Error::Discovery(
                "le serveur d'API n'a annoncé aucune ressource".to_string(),
            ));
        }

        enrich(client, &mut kinds).await;
        kinds.sort_by(|a, b| {
            a.group
                .cmp(&b.group)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| a.version.cmp(&b.version))
        });
        kinds.dedup_by(|a, b| a.group == b.group && a.kind == b.kind && a.version == b.version);

        Ok(Self { kinds })
    }

    /// Catalogue minimal des ressources natives, utilisé lorsque la découverte échoue.
    ///
    /// Il permet à l'interface de rester utilisable sur un cluster dont l'agrégation d'API
    /// est partiellement indisponible.
    pub fn fallback() -> Self {
        const BUILTIN: &[(&str, &str, &str, &str, bool)] = &[
            ("", "v1", "Pod", "pods", true),
            ("", "v1", "Service", "services", true),
            ("", "v1", "ConfigMap", "configmaps", true),
            ("", "v1", "Secret", "secrets", true),
            ("", "v1", "Namespace", "namespaces", false),
            ("", "v1", "Node", "nodes", false),
            ("", "v1", "Event", "events", true),
            ("", "v1", "ServiceAccount", "serviceaccounts", true),
            ("", "v1", "PersistentVolume", "persistentvolumes", false),
            (
                "",
                "v1",
                "PersistentVolumeClaim",
                "persistentvolumeclaims",
                true,
            ),
            ("apps", "v1", "Deployment", "deployments", true),
            ("apps", "v1", "StatefulSet", "statefulsets", true),
            ("apps", "v1", "DaemonSet", "daemonsets", true),
            ("apps", "v1", "ReplicaSet", "replicasets", true),
            ("batch", "v1", "Job", "jobs", true),
            ("batch", "v1", "CronJob", "cronjobs", true),
            ("networking.k8s.io", "v1", "Ingress", "ingresses", true),
            (
                "networking.k8s.io",
                "v1",
                "NetworkPolicy",
                "networkpolicies",
                true,
            ),
        ];
        let verbs: Vec<String> = [
            "get", "list", "watch", "create", "update", "patch", "delete",
        ]
        .iter()
        .map(|v| v.to_string())
        .collect();
        Self {
            kinds: BUILTIN
                .iter()
                .map(|(group, version, kind, plural, namespaced)| ResourceKind {
                    group: (*group).to_string(),
                    version: (*version).to_string(),
                    kind: (*kind).to_string(),
                    plural: (*plural).to_string(),
                    namespaced: *namespaced,
                    verbs: verbs.clone(),
                    short_names: Vec::new(),
                    categories: vec!["all".to_string()],
                })
                .collect(),
        }
    }

    /// Nombre de types connus.
    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    /// Vrai si le catalogue est vide.
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// Résout une saisie utilisateur vers un type de ressource.
    ///
    /// Sont acceptés, sans distinction de casse : le kind (`Pod`), le pluriel (`pods`),
    /// la forme qualifiée (`deployments.apps`), les abréviations annoncées par le serveur
    /// et les abréviations usuelles de `kubectl` (`po`, `deploy`, `svc`...).
    pub fn resolve(&self, needle: &str) -> Option<&ResourceKind> {
        let needle = needle.trim().trim_end_matches('.');
        if needle.is_empty() {
            return None;
        }
        if let Some(found) = self.resolve_direct(needle) {
            return Some(found);
        }
        let lower = needle.to_ascii_lowercase();
        let canonical = ABBREVIATIONS
            .iter()
            .find(|(abbr, _)| *abbr == lower)
            .map(|(_, canonical)| *canonical)?;
        self.resolve_direct(canonical)
            // L'abréviation peut désigner un groupe absent de ce cluster : on retente sur
            // le seul pluriel, sans le groupe.
            .or_else(|| {
                canonical
                    .split_once('.')
                    .and_then(|(plural, _)| self.resolve_direct(plural))
            })
    }

    /// Résolution sans passer par la table d'abréviations statique.
    fn resolve_direct(&self, needle: &str) -> Option<&ResourceKind> {
        if let Some((head, group)) = needle.split_once('.') {
            let matches: Vec<&ResourceKind> = self
                .kinds
                .iter()
                .filter(|k| {
                    k.group.eq_ignore_ascii_case(group)
                        && (k.plural.eq_ignore_ascii_case(head)
                            || k.kind.eq_ignore_ascii_case(head)
                            || k.short_names.iter().any(|s| s.eq_ignore_ascii_case(head)))
                })
                .collect();
            if let Some(found) = best(matches) {
                return Some(found);
            }
        }

        let by_kind: Vec<&ResourceKind> = self
            .kinds
            .iter()
            .filter(|k| k.kind.eq_ignore_ascii_case(needle))
            .collect();
        if let Some(found) = best(by_kind) {
            return Some(found);
        }

        let by_plural: Vec<&ResourceKind> = self
            .kinds
            .iter()
            .filter(|k| k.plural.eq_ignore_ascii_case(needle))
            .collect();
        if let Some(found) = best(by_plural) {
            return Some(found);
        }

        let by_short: Vec<&ResourceKind> = self
            .kinds
            .iter()
            .filter(|k| k.short_names.iter().any(|s| s.eq_ignore_ascii_case(needle)))
            .collect();
        best(by_short)
    }

    /// Recherche exacte par groupe, version et kind.
    pub fn find_gvk(&self, group: &str, version: &str, kind: &str) -> Option<&ResourceKind> {
        self.kinds
            .iter()
            .find(|k| k.group == group && k.version == version && k.kind == kind)
    }

    /// Convertit un type découvert en descripteur utilisable par `kube::Api`.
    pub fn api_resource(k: &ResourceKind) -> ApiResource {
        ApiResource {
            group: k.group.clone(),
            version: k.version.clone(),
            api_version: k.api_version(),
            kind: k.kind.clone(),
            plural: k.plural.clone(),
        }
    }

    /// Types appartenant à une catégorie (`all`, `api-extensions`...).
    pub fn by_category(&self, cat: &str) -> Vec<&ResourceKind> {
        self.kinds
            .iter()
            .filter(|k| k.categories.iter().any(|c| c.eq_ignore_ascii_case(cat)))
            .collect()
    }

    /// Types listables, utiles pour peupler la navigation de l'interface.
    pub fn listable(&self) -> Vec<&ResourceKind> {
        self.kinds.iter().filter(|k| k.supports("list")).collect()
    }
}

/// Choisit le meilleur candidat : le groupe core d'abord, puis le groupe le plus court.
fn best(mut matches: Vec<&ResourceKind>) -> Option<&ResourceKind> {
    matches.sort_by(|a, b| {
        (
            !a.group.is_empty(),
            a.group.len(),
            a.group.as_str(),
            a.kind.as_str(),
        )
            .cmp(&(
                !b.group.is_empty(),
                b.group.len(),
                b.group.as_str(),
                b.kind.as_str(),
            ))
    });
    matches.into_iter().next()
}

/// Complète abréviations et catégories en lisant les `APIResourceList` du serveur.
async fn enrich(client: &Client, kinds: &mut [ResourceKind]) {
    let group_versions: BTreeSet<(String, String)> = kinds
        .iter()
        .map(|k| (k.group.clone(), k.version.clone()))
        .collect();

    let fetches = group_versions.into_iter().map(|(group, version)| {
        let client = client.clone();
        async move {
            let result = api_resource_list(&client, &group, &version).await;
            (group, version, result)
        }
    });

    // Clé: (groupe, version, pluriel) -> (abréviations, catégories)
    let mut extras: BTreeMap<(String, String, String), (Vec<String>, Vec<String>)> =
        BTreeMap::new();
    for (group, version, result) in futures::future::join_all(fetches).await {
        let list = match result {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(
                    %group, %version, error = %e,
                    "abréviations indisponibles pour ce groupe d'API"
                );
                continue;
            }
        };
        let Some(resources) = list.get("resources").and_then(|r| r.as_array()) else {
            continue;
        };
        for res in resources {
            let Some(name) = res.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            // Les sous-ressources (`pods/log`, `deployments/scale`) ne sont pas des kinds.
            if name.contains('/') {
                continue;
            }
            let strings = |key: &str| -> Vec<String> {
                res.get(key)
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            extras.insert(
                (group.clone(), version.clone(), name.to_string()),
                (strings("shortNames"), strings("categories")),
            );
        }
    }

    for k in kinds.iter_mut() {
        if let Some((short_names, categories)) =
            extras.get(&(k.group.clone(), k.version.clone(), k.plural.clone()))
        {
            k.short_names = short_names.clone();
            k.categories = categories.clone();
        }
    }
}

/// Lit la liste brute des ressources d'un groupe/version.
async fn api_resource_list(
    client: &Client,
    group: &str,
    version: &str,
) -> Result<serde_json::Value> {
    let path = if group.is_empty() {
        format!("/api/{version}")
    } else {
        format!("/apis/{group}/{version}")
    };
    let req = http::Request::get(path).body(Vec::new())?;
    client.request(req).await.map_err(Error::from_kube)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(
        group: &str,
        version: &str,
        kind: &str,
        plural: &str,
        namespaced: bool,
        short: &[&str],
    ) -> ResourceKind {
        ResourceKind {
            group: group.to_string(),
            version: version.to_string(),
            kind: kind.to_string(),
            plural: plural.to_string(),
            namespaced,
            verbs: vec!["get".into(), "list".into(), "delete".into()],
            short_names: short.iter().map(|s| s.to_string()).collect(),
            categories: if group.is_empty() || group == "apps" {
                vec!["all".to_string()]
            } else {
                Vec::new()
            },
        }
    }

    fn catalogue() -> ResourceCatalog {
        ResourceCatalog {
            kinds: vec![
                kind("", "v1", "Pod", "pods", true, &["po"]),
                kind("", "v1", "Service", "services", true, &["svc"]),
                kind("", "v1", "Namespace", "namespaces", false, &["ns"]),
                kind("", "v1", "Node", "nodes", false, &["no"]),
                kind("", "v1", "ConfigMap", "configmaps", true, &["cm"]),
                kind("", "v1", "Event", "events", true, &["ev"]),
                kind("apps", "v1", "Deployment", "deployments", true, &[]),
                kind("apps", "v1", "StatefulSet", "statefulsets", true, &[]),
                kind("apps", "v1", "DaemonSet", "daemonsets", true, &[]),
                kind("apps", "v1", "ReplicaSet", "replicasets", true, &[]),
                kind("batch", "v1", "Job", "jobs", true, &[]),
                kind("batch", "v1", "CronJob", "cronjobs", true, &[]),
                kind("events.k8s.io", "v1", "Event", "events", true, &[]),
                kind(
                    "networking.k8s.io",
                    "v1",
                    "Ingress",
                    "ingresses",
                    true,
                    &["ing"],
                ),
                kind(
                    "apiextensions.k8s.io",
                    "v1",
                    "CustomResourceDefinition",
                    "customresourcedefinitions",
                    false,
                    &[],
                ),
                kind(
                    "autoscaling",
                    "v2",
                    "HorizontalPodAutoscaler",
                    "horizontalpodautoscalers",
                    true,
                    &[],
                ),
                kind(
                    "cert-manager.io",
                    "v1",
                    "Certificate",
                    "certificates",
                    true,
                    &["cert"],
                ),
            ],
        }
    }

    #[test]
    fn resolution_par_kind_et_pluriel() {
        let c = catalogue();
        assert_eq!(c.resolve("Pod").expect("pod").kind, "Pod");
        assert_eq!(c.resolve("pod").expect("pod").kind, "Pod");
        assert_eq!(c.resolve("pods").expect("pods").kind, "Pod");
        assert_eq!(c.resolve("PODS").expect("pods").kind, "Pod");
        assert_eq!(c.resolve("  deployments ").expect("deploy").group, "apps");
    }

    #[test]
    fn resolution_par_abreviation_serveur() {
        let c = catalogue();
        assert_eq!(c.resolve("po").expect("po").kind, "Pod");
        assert_eq!(c.resolve("svc").expect("svc").kind, "Service");
        assert_eq!(c.resolve("cert").expect("cert").group, "cert-manager.io");
    }

    #[test]
    fn resolution_par_table_statique() {
        let c = catalogue();
        for (saisie, attendu) in [
            ("deploy", "Deployment"),
            ("sts", "StatefulSet"),
            ("ds", "DaemonSet"),
            ("rs", "ReplicaSet"),
            ("job", "Job"),
            ("cj", "CronJob"),
            ("ing", "Ingress"),
            ("cm", "ConfigMap"),
            ("ns", "Namespace"),
            ("no", "Node"),
            ("hpa", "HorizontalPodAutoscaler"),
            ("crd", "CustomResourceDefinition"),
        ] {
            let k = c
                .resolve(saisie)
                .unwrap_or_else(|| panic!("« {saisie} » doit se résoudre"));
            assert_eq!(k.kind, attendu, "saisie « {saisie} »");
        }
    }

    #[test]
    fn resolution_qualifiee_par_groupe() {
        let c = catalogue();
        let d = c.resolve("deployments.apps").expect("deployments.apps");
        assert_eq!(d.group, "apps");
        assert_eq!(d.kind, "Deployment");
        let e = c.resolve("events.events.k8s.io").expect("events qualifiés");
        assert_eq!(e.group, "events.k8s.io");
        // Sans qualification, le groupe core est préféré.
        assert_eq!(c.resolve("events").expect("events").group, "");
        assert_eq!(c.resolve("ev").expect("ev").group, "");
    }

    #[test]
    fn resolution_inconnue() {
        let c = catalogue();
        assert!(c.resolve("").is_none());
        assert!(c.resolve("   ").is_none());
        assert!(c.resolve("gizmos").is_none());
        assert!(c.resolve("deployments.inexistant").is_none());
    }

    #[test]
    fn abreviation_dun_groupe_absent_retombe_sur_le_pluriel() {
        // Catalogue sans le groupe `networking.k8s.io` mais avec un `Ingress` en extensions.
        let c = ResourceCatalog {
            kinds: vec![kind(
                "extensions",
                "v1beta1",
                "Ingress",
                "ingresses",
                true,
                &[],
            )],
        };
        let k = c.resolve("ing").expect("ing doit retomber sur le pluriel");
        assert_eq!(k.group, "extensions");
    }

    #[test]
    fn conversion_en_api_resource() {
        let c = catalogue();
        let d = c.resolve("deploy").expect("deploy");
        let ar = ResourceCatalog::api_resource(d);
        assert_eq!(ar.group, "apps");
        assert_eq!(ar.version, "v1");
        assert_eq!(ar.api_version, "apps/v1");
        assert_eq!(ar.kind, "Deployment");
        assert_eq!(ar.plural, "deployments");

        let p = c.resolve("po").expect("po");
        let ar = ResourceCatalog::api_resource(p);
        assert_eq!(ar.api_version, "v1");
        assert!(ar.group.is_empty());
    }

    #[test]
    fn categories_et_listables() {
        let c = catalogue();
        let all = c.by_category("all");
        assert!(all.iter().any(|k| k.kind == "Pod"));
        assert!(all.iter().any(|k| k.kind == "Deployment"));
        assert!(!all.iter().any(|k| k.kind == "Ingress"));
        assert_eq!(c.listable().len(), c.len());
        assert!(!c.is_empty());
    }

    #[test]
    fn catalogue_de_repli_utilisable() {
        let c = ResourceCatalog::fallback();
        assert!(!c.is_empty());
        assert_eq!(c.resolve("po").expect("po").kind, "Pod");
        assert_eq!(c.resolve("deploy").expect("deploy").group, "apps");
        assert!(c.find_gvk("apps", "v1", "Deployment").is_some());
    }
}
