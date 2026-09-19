//! Modèles de données partagés entre le cœur et l'application de bureau.
//!
//! Tous les types de ce module sérialisent en `camelCase`, la convention des objets
//! Kubernetes eux-mêmes. Les champs optionnels vides sont omis de la charge utile.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Référence complète et non ambiguë vers un objet du cluster.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRef {
    /// Groupe d'API (vide pour le groupe « core »).
    #[serde(default)]
    pub group: String,
    /// Version du groupe d'API (par exemple `v1`).
    #[serde(default)]
    pub version: String,
    /// Kind au singulier et en PascalCase (par exemple `Deployment`).
    pub kind: String,
    /// Nom pluriel de la ressource dans l'URL (par exemple `deployments`).
    #[serde(default)]
    pub plural: String,
    /// Namespace, absent pour les ressources de portée cluster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Nom de l'objet.
    pub name: String,
}

impl ResourceRef {
    /// Construit une référence à partir d'un kind découvert.
    pub fn from_kind(kind: &ResourceKind, namespace: Option<&str>, name: &str) -> Self {
        Self {
            group: kind.group.clone(),
            version: kind.version.clone(),
            kind: kind.kind.clone(),
            plural: kind.plural.clone(),
            namespace: if kind.namespaced {
                namespace.map(str::to_string)
            } else {
                None
            },
            name: name.to_string(),
        }
    }

    /// Triplet groupe/version/kind attendu par `kube`.
    pub fn gvk(&self) -> kube::core::GroupVersionKind {
        kube::core::GroupVersionKind::gvk(&self.group, &self.version, &self.kind)
    }

    /// `apiVersion` tel qu'il apparaît dans un manifeste YAML.
    pub fn api_version(&self) -> String {
        if self.group.is_empty() {
            self.version.clone()
        } else {
            format!("{}/{}", self.group, self.version)
        }
    }

    /// Libellé lisible : `namespace/nom` ou `nom` pour les ressources cluster.
    pub fn display(&self) -> String {
        match &self.namespace {
            Some(ns) if !ns.is_empty() => format!("{}/{}", ns, self.name),
            _ => self.name.clone(),
        }
    }
}

/// Description d'un type de ressource exposé par le serveur d'API.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceKind {
    /// Groupe d'API (vide pour le groupe « core »).
    #[serde(default)]
    pub group: String,
    /// Version préférée du groupe.
    pub version: String,
    /// Kind au singulier et en PascalCase.
    pub kind: String,
    /// Nom pluriel utilisé dans les URL.
    pub plural: String,
    /// Vrai si la ressource vit dans un namespace.
    pub namespaced: bool,
    /// Verbes autorisés (`get`, `list`, `watch`, `create`, `delete`, `patch`, `update`...).
    #[serde(default)]
    pub verbs: Vec<String>,
    /// Abréviations reconnues par le serveur (`po`, `deploy`, `svc`...).
    #[serde(default)]
    pub short_names: Vec<String>,
    /// Catégories (`all`, `api-extensions`...).
    #[serde(default)]
    pub categories: Vec<String>,
}

impl ResourceKind {
    /// `apiVersion` tel qu'il apparaît dans un manifeste YAML.
    pub fn api_version(&self) -> String {
        if self.group.is_empty() {
            self.version.clone()
        } else {
            format!("{}/{}", self.group, self.version)
        }
    }

    /// Triplet groupe/version/kind attendu par `kube`.
    pub fn gvk(&self) -> kube::core::GroupVersionKind {
        kube::core::GroupVersionKind::gvk(&self.group, &self.version, &self.kind)
    }

    /// Identifiant canonique `pluriel.groupe` (ou `pluriel` pour le groupe core).
    pub fn full_name(&self) -> String {
        if self.group.is_empty() {
            self.plural.clone()
        } else {
            format!("{}.{}", self.plural, self.group)
        }
    }

    /// Vrai si le verbe est supporté par cette ressource.
    pub fn supports(&self, verb: &str) -> bool {
        self.verbs.iter().any(|v| v.eq_ignore_ascii_case(verb))
    }
}

/// Propriétaire déclaré dans `metadata.ownerReferences`.
///
/// C'est par cette chaîne qu'un pod remonte à son ReplicaSet puis à son
/// Deployment, ou un Job à son CronJob. Le namespace n'est pas répété : un
/// propriétaire vit toujours dans celui de l'objet possédé.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerRef {
    /// Kind du propriétaire (`ReplicaSet`, `Deployment`, `Job`...).
    pub kind: String,
    /// Nom du propriétaire.
    pub name: String,
    /// UID du propriétaire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// Vrai si ce propriétaire est le contrôleur de l'objet.
    #[serde(default)]
    pub controller: bool,
}

/// Vue condensée d'un objet, suffisante pour alimenter un tableau de l'interface.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectSummary {
    /// Nom de l'objet.
    pub name: String,
    /// Namespace, absent pour les ressources de portée cluster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Kind de l'objet.
    pub kind: String,
    /// `apiVersion` de l'objet.
    pub api_version: String,
    /// UID attribué par le serveur d'API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// Horodatage de création.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// Âge en secondes, précalculé pour éviter une dérive d'horloge côté navigateur.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<i64>,
    /// Statut synthétique (`Running`, `Healthy`, `Bound`, `Active`...).
    #[serde(default)]
    pub status: String,
    /// Compteur de disponibilité au format `prêts/total` (par exemple `2/3`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<String>,
    /// Nombre total de redémarrages des conteneurs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restarts: Option<i64>,
    /// Nœud hébergeant l'objet, le cas échéant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Images utilisées par l'objet.
    #[serde(default)]
    pub images: Vec<String>,
    /// Labels de l'objet.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Annotations de l'objet.
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    /// Propriétaires déclarés dans `metadata.ownerReferences`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<OwnerRef>,
    /// Colonnes supplémentaires spécifiques au kind.
    #[serde(default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Page de résultats d'un listing, avec jeton de continuation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectListPage {
    /// Objets de la page courante.
    #[serde(default)]
    pub items: Vec<ObjectSummary>,
    /// Jeton à repasser pour obtenir la page suivante.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_token: Option<String>,
    /// Nombre d'éléments restants annoncé par le serveur, si disponible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<i64>,
}

/// Options de listing communes à tous les kinds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListOptions {
    /// Namespace ciblé ; `None` signifie « tous les namespaces ».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Sélecteur de labels (`app=web,tier!=db`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_selector: Option<String>,
    /// Sélecteur de champs (`status.phase=Running`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_selector: Option<String>,
    /// Taille de page demandée au serveur.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Jeton de continuation renvoyé par la page précédente.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_token: Option<String>,
}

impl ListOptions {
    /// Options limitées à un namespace.
    pub fn in_namespace(namespace: impl Into<String>) -> Self {
        Self {
            namespace: Some(namespace.into()),
            ..Default::default()
        }
    }
}

/// Contexte déclaré dans un kubeconfig.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextInfo {
    /// Nom du contexte.
    pub name: String,
    /// Nom du cluster référencé.
    pub cluster: String,
    /// Nom de l'utilisateur référencé.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Namespace par défaut du contexte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// URL du serveur d'API du cluster référencé.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// Vrai s'il s'agit du `current-context` du fichier.
    pub current: bool,
}

/// État d'un cluster enregistré dans KubeWatch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterInfo {
    /// Nom local du cluster dans KubeWatch.
    pub name: String,
    /// Contexte kubeconfig d'origine, le cas échéant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// URL du serveur d'API.
    #[serde(default)]
    pub server: String,
    /// Version du serveur (`gitVersion`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Plateforme du serveur (`linux/amd64`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// Vrai si la dernière sonde de santé a réussi.
    pub connected: bool,
    /// Nombre de nœuds, si la lecture a réussi.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_count: Option<usize>,
    /// Nombre de namespaces, si la lecture a réussi.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace_count: Option<usize>,
    /// Namespace utilisé quand l'appelant n'en précise pas.
    #[serde(default)]
    pub default_namespace: String,
    /// Vrai si l'API `metrics.k8s.io` répond.
    pub metrics_available: bool,
    /// Dernière erreur rencontrée lors de la collecte, le cas échéant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Version du serveur d'API Kubernetes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    /// Version majeure (`1`).
    #[serde(default)]
    pub major: String,
    /// Version mineure (`31`, parfois suffixée `+`).
    #[serde(default)]
    pub minor: String,
    /// Version complète (`v1.31.2`).
    #[serde(default)]
    pub git_version: String,
    /// Plateforme du serveur (`linux/amd64`).
    #[serde(default)]
    pub platform: String,
}

/// Mesure de consommation d'un nœud, d'un pod ou d'un conteneur.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSample {
    /// Nom du nœud ou du pod mesuré.
    pub name: String,
    /// Namespace du pod mesuré, absent pour un nœud.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Conteneur mesuré lorsque la mesure est détaillée.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// CPU consommé en millicores.
    #[serde(default)]
    pub cpu_millis: f64,
    /// Mémoire consommée en octets.
    #[serde(default)]
    pub memory_bytes: i64,
    /// Horodatage de la mesure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
}

/// Synthèse de l'état d'un cluster, affichée sur la page d'accueil.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterOverview {
    /// Nombre total de nœuds.
    #[serde(default)]
    pub nodes_total: usize,
    /// Nœuds dont la condition `Ready` vaut `True`.
    #[serde(default)]
    pub nodes_ready: usize,
    /// Nombre total de pods.
    #[serde(default)]
    pub pods_total: usize,
    /// Pods en phase `Running`.
    #[serde(default)]
    pub pods_running: usize,
    /// Pods en phase `Pending`.
    #[serde(default)]
    pub pods_pending: usize,
    /// Pods en phase `Failed`.
    #[serde(default)]
    pub pods_failed: usize,
    /// Nombre de namespaces.
    #[serde(default)]
    pub namespaces: usize,
    /// Capacité CPU cumulée en millicores.
    #[serde(default)]
    pub cpu_capacity_millis: f64,
    /// CPU consommé en millicores (nécessite `metrics.k8s.io`).
    #[serde(default)]
    pub cpu_used_millis: f64,
    /// Capacité mémoire cumulée en octets.
    #[serde(default)]
    pub memory_capacity_bytes: i64,
    /// Mémoire consommée en octets (nécessite `metrics.k8s.io`).
    #[serde(default)]
    pub memory_used_bytes: i64,
    /// Décompte par kind de charge de travail (`Deployment`, `StatefulSet`...).
    #[serde(default)]
    pub workloads: BTreeMap<String, usize>,
    /// Avertissements collectés pendant la synthèse (sous-requêtes en échec, quotas...).
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Vrai si les mesures de consommation ont pu être lues.
    pub metrics_available: bool,
}

/// Évènement Kubernetes condensé.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventSummary {
    /// Namespace de l'évènement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Nom de l'objet `Event`.
    pub name: String,
    /// Raison courte (`Scheduled`, `BackOff`, `FailedMount`...).
    #[serde(default)]
    pub reason: String,
    /// Message détaillé.
    #[serde(default)]
    pub message: String,
    /// Type de l'évènement (`Normal` ou `Warning`).
    #[serde(rename = "type", default)]
    pub type_: String,
    /// Nombre d'occurrences agrégées.
    #[serde(default)]
    pub count: i32,
    /// Première occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<DateTime<Utc>>,
    /// Dernière occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<DateTime<Utc>>,
    /// Kind de l'objet concerné.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub involved_kind: Option<String>,
    /// Nom de l'objet concerné.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub involved_name: Option<String>,
    /// Composant émetteur (`kubelet`, `default-scheduler`...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Conteneur d'un pod, y compris les conteneurs d'initialisation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerInfo {
    /// Nom du conteneur.
    pub name: String,
    /// Image utilisée.
    #[serde(default)]
    pub image: String,
    /// Vrai si la sonde de disponibilité passe.
    pub ready: bool,
    /// Nombre de redémarrages.
    #[serde(default)]
    pub restart_count: i64,
    /// État courant (`running`, `waiting: CrashLoopBackOff`, `terminated: Error`...).
    #[serde(default)]
    pub state: String,
    /// Vrai s'il s'agit d'un conteneur d'initialisation.
    pub init: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_gvk_et_api_version() {
        let core = ResourceRef {
            group: String::new(),
            version: "v1".into(),
            kind: "Pod".into(),
            plural: "pods".into(),
            namespace: Some("default".into()),
            name: "web".into(),
        };
        assert_eq!(core.api_version(), "v1");
        assert_eq!(core.display(), "default/web");
        let gvk = core.gvk();
        assert_eq!(gvk.group, "");
        assert_eq!(gvk.version, "v1");
        assert_eq!(gvk.kind, "Pod");

        let apps = ResourceRef {
            group: "apps".into(),
            version: "v1".into(),
            kind: "Deployment".into(),
            plural: "deployments".into(),
            namespace: None,
            name: "api".into(),
        };
        assert_eq!(apps.api_version(), "apps/v1");
        assert_eq!(apps.display(), "api");
    }

    #[test]
    fn reference_depuis_kind() {
        let cluster_kind = ResourceKind {
            group: String::new(),
            version: "v1".into(),
            kind: "Node".into(),
            plural: "nodes".into(),
            namespaced: false,
            ..Default::default()
        };
        let r = ResourceRef::from_kind(&cluster_kind, Some("default"), "node-1");
        assert!(r.namespace.is_none(), "un kind cluster ignore le namespace");
        assert_eq!(r.plural, "nodes");
    }

    #[test]
    fn kind_nom_complet_et_verbes() {
        let k = ResourceKind {
            group: "apps".into(),
            version: "v1".into(),
            kind: "Deployment".into(),
            plural: "deployments".into(),
            namespaced: true,
            verbs: vec!["get".into(), "list".into(), "delete".into()],
            short_names: vec!["deploy".into()],
            categories: vec!["all".into()],
        };
        assert_eq!(k.full_name(), "deployments.apps");
        assert_eq!(k.api_version(), "apps/v1");
        assert!(k.supports("DELETE"));
        assert!(!k.supports("patch"));
    }

    #[test]
    fn serialisation_camel_case() {
        let s = ObjectSummary {
            name: "web".into(),
            namespace: Some("default".into()),
            kind: "Pod".into(),
            api_version: "v1".into(),
            age_seconds: Some(42),
            restarts: Some(3),
            ..Default::default()
        };
        let v = serde_json::to_value(&s).expect("sérialisation");
        assert!(v.get("apiVersion").is_some());
        assert!(v.get("ageSeconds").is_some());
        assert!(v.get("restarts").is_some());
        // Les options vides sont omises.
        assert!(v.get("uid").is_none());
        assert!(v.get("createdAt").is_none());
        assert!(v.get("owners").is_none());
        // Le tour complet doit être stable.
        let back: ObjectSummary = serde_json::from_value(v).expect("désérialisation");
        assert_eq!(back, s);
    }

    #[test]
    fn evenement_serialise_le_champ_type() {
        let e = EventSummary {
            name: "web.17a".into(),
            reason: "BackOff".into(),
            type_: "Warning".into(),
            count: 7,
            ..Default::default()
        };
        let v = serde_json::to_value(&e).expect("sérialisation");
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("Warning"));
        assert!(v.get("type_").is_none());
        assert!(v.get("involvedKind").is_none());
        let back: EventSummary = serde_json::from_value(v).expect("désérialisation");
        assert_eq!(back.type_, "Warning");
        assert_eq!(back.count, 7);
    }

    #[test]
    fn options_de_listing_camel_case() {
        let o = ListOptions {
            namespace: Some("kube-system".into()),
            label_selector: Some("app=web".into()),
            limit: Some(250),
            ..Default::default()
        };
        let v = serde_json::to_value(&o).expect("sérialisation");
        assert!(v.get("labelSelector").is_some());
        assert!(v.get("fieldSelector").is_none());
        assert!(v.get("continueToken").is_none());
    }

    #[test]
    fn synthese_cluster_par_defaut() {
        let o = ClusterOverview::default();
        let v = serde_json::to_value(&o).expect("sérialisation");
        assert_eq!(v.get("nodesTotal").and_then(|n| n.as_u64()), Some(0));
        assert_eq!(
            v.get("metricsAvailable").and_then(|n| n.as_bool()),
            Some(false)
        );
        assert!(v.get("cpuCapacityMillis").is_some());
        assert!(v.get("memoryUsedBytes").is_some());
    }
}
