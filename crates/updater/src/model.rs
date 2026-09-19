//! Types de données du détecteur de mises à jour.
//!
//! Tous les DTO exposés à l'API HTTP sont `Serialize + Deserialize` en
//! `camelCase`, afin que l'interface web puisse les consommer tels quels.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Origine d'une nouvelle version : release GitHub, tag de registre OCI ou chart Helm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UpdateSource {
    /// Suit les releases (ou, à défaut, les tags) d'un dépôt GitHub.
    #[serde(rename_all = "camelCase")]
    GithubRelease {
        /// Propriétaire du dépôt (organisation ou utilisateur).
        owner: String,
        /// Nom du dépôt.
        repo: String,
        /// Préfixe à retirer des tags avant l'analyse semver (ex. `"release-"`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag_prefix: Option<String>,
    },
    /// Suit les tags d'une image de conteneur.
    #[serde(rename_all = "camelCase")]
    ContainerRegistry {
        /// Référence d'image, tag inclus ou non (ex. `"docker.io/library/nginx:1.27"`).
        image: String,
    },
    /// Suit les versions publiées d'un chart Helm.
    #[serde(rename_all = "camelCase")]
    HelmChart {
        /// URL ou nom du dépôt de charts.
        repo: String,
        /// Nom du chart.
        chart: String,
    },
}

impl UpdateSource {
    /// Libellé court et lisible, utilisé dans les journaux et l'interface.
    pub fn label(&self) -> String {
        match self {
            UpdateSource::GithubRelease { owner, repo, .. } => format!("github:{owner}/{repo}"),
            UpdateSource::ContainerRegistry { image } => format!("image:{image}"),
            UpdateSource::HelmChart { repo, chart } => format!("helm:{repo}/{chart}"),
        }
    }

    /// Préfixe de tag configuré, le cas échéant.
    pub fn tag_prefix(&self) -> Option<&str> {
        match self {
            UpdateSource::GithubRelease { tag_prefix, .. } => tag_prefix.as_deref(),
            _ => None,
        }
    }
}

/// Canal de mise à jour : jusqu'où on autorise le saut de version.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateChannel {
    /// Toute version strictement supérieure, y compris un changement de majeure.
    Major,
    /// Même majeure, `(minor, patch)` strictement supérieur.
    Minor,
    /// Même majeure et même mineure, correctif strictement supérieur.
    #[default]
    Patch,
    /// Comme `Major`, mais les pré-versions sont acceptées.
    Prerelease,
    /// Version figée : aucune mise à jour proposée.
    Pinned,
}

/// Politique appliquée à un watcher pour choisir (et éventuellement appliquer) une version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePolicy {
    /// Canal autorisé.
    #[serde(default)]
    pub channel: UpdateChannel,
    /// Autorise les pré-versions (`1.2.3-rc.1`) hors canal `Prerelease`.
    #[serde(default)]
    pub allow_prerelease: bool,
    /// Contrainte semver supplémentaire (ex. `">=1.2, <2"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint: Option<String>,
    /// Versions ou tags à ignorer explicitement.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Applique automatiquement la mise à jour détectée.
    #[serde(default)]
    pub auto_apply: bool,
    /// Période entre deux vérifications, en secondes.
    #[serde(default = "default_check_interval")]
    pub check_interval_seconds: u64,
    /// Fenêtre de maintenance UTC au format `"HH:MM-HH:MM"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance_window: Option<String>,
}

fn default_check_interval() -> u64 {
    3600
}

impl Default for UpdatePolicy {
    fn default() -> Self {
        Self {
            channel: UpdateChannel::Patch,
            allow_prerelease: false,
            constraint: None,
            ignore: Vec::new(),
            auto_apply: false,
            check_interval_seconds: default_check_interval(),
            maintenance_window: None,
        }
    }
}

impl UpdatePolicy {
    /// Intervalle de vérification sous forme de `Duration`, borné à 60 secondes minimum.
    pub fn check_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.check_interval_seconds.max(60))
    }

    /// Indique si les pré-versions sont acceptées pour ce canal.
    pub fn prerelease_allowed(&self) -> bool {
        self.allow_prerelease || self.channel == UpdateChannel::Prerelease
    }
}

/// Objet Kubernetes surveillé par un watcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchTarget {
    /// Namespace de l'objet (absent pour une ressource de portée cluster).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Kind Kubernetes (`Deployment`, `StatefulSet`, `DaemonSet`, ...).
    pub kind: String,
    /// Nom de l'objet.
    pub name: String,
}

impl WatchTarget {
    /// Construit une cible.
    pub fn new(
        kind: impl Into<String>,
        namespace: Option<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            namespace,
            kind: kind.into(),
            name: name.into(),
        }
    }

    /// Représentation `namespace/kind/name` utilisée dans les journaux.
    pub fn label(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{ns}/{}/{}", self.kind, self.name),
            None => format!("{}/{}", self.kind, self.name),
        }
    }
}

/// Alias historiques conservés pour la compatibilité des appelants.
pub type WatcherTarget = WatchTarget;
/// Alias historiques conservés pour la compatibilité des appelants.
pub type TargetRef = WatchTarget;

/// Déclaration persistante d'une surveillance de version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherSpec {
    /// Identifiant stable (UUID v4).
    pub id: String,
    /// Nom lisible affiché dans l'interface.
    pub name: String,
    /// Watcher actif ou mis en pause.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Nom du cluster enregistré dans `ClusterManager`.
    pub cluster: String,
    /// Objet Kubernetes surveillé.
    pub target: WatchTarget,
    /// Conteneur ciblé dans le pod ; `None` = premier conteneur.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Source de vérité des versions disponibles.
    pub source: UpdateSource,
    /// Politique de sélection et d'application.
    #[serde(default)]
    pub policy: UpdatePolicy,
    /// Date de création.
    pub created_at: DateTime<Utc>,
    /// Date de la dernière vérification aboutie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<DateTime<Utc>>,
    /// Dernière version connue sur le cluster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_known_version: Option<String>,
}

fn default_true() -> bool {
    true
}

impl WatcherSpec {
    /// Crée un watcher avec un identifiant neuf et l'horodatage courant.
    pub fn new(
        name: impl Into<String>,
        cluster: impl Into<String>,
        target: WatchTarget,
        source: UpdateSource,
        policy: UpdatePolicy,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            enabled: true,
            cluster: cluster.into(),
            target,
            container: None,
            source,
            policy,
            created_at: Utc::now(),
            last_checked_at: None,
            last_known_version: None,
        }
    }
}

/// Fichier attaché à une release GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAsset {
    /// Nom du fichier.
    pub name: String,
    /// Taille en octets.
    pub size: i64,
    /// URL de téléchargement direct.
    pub download_url: String,
    /// Type MIME déclaré.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

/// Version publiée en amont (release GitHub, tag OCI ou version de chart).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    /// Tag brut tel que publié (ex. `"v1.2.3"`).
    pub tag: String,
    /// Titre de la release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Notes de version (Markdown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Date de publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<DateTime<Utc>>,
    /// Marquée comme pré-version en amont.
    #[serde(default)]
    pub prerelease: bool,
    /// Brouillon non publié.
    #[serde(default)]
    pub draft: bool,
    /// Page web de la release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_url: Option<String>,
    /// Version semver normalisée à partir du tag, si elle a pu être déduite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semver: Option<String>,
    /// Fichiers attachés.
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

impl ReleaseInfo {
    /// Construit une release minimale à partir d'un simple tag.
    pub fn from_tag(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            name: None,
            body: None,
            published_at: None,
            prerelease: false,
            draft: false,
            html_url: None,
            semver: None,
            assets: Vec::new(),
        }
    }

    /// Version semver analysée à partir du champ `semver`, si présent.
    pub fn parsed_semver(&self) -> Option<semver::Version> {
        self.semver
            .as_deref()
            .and_then(|s| semver::Version::parse(s).ok())
    }
}

/// Ampleur d'un saut de version.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateSeverity {
    /// Changement de version majeure : ruptures possibles.
    Major,
    /// Nouvelles fonctionnalités compatibles.
    Minor,
    /// Correctif compatible.
    Patch,
    /// Impossible de comparer (version courante inconnue ou non semver).
    #[default]
    Unknown,
}

/// Mise à jour détectée pour une cible surveillée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFinding {
    /// Identifiant stable du constat.
    pub id: String,
    /// Identifiant du watcher à l'origine du constat.
    pub watcher_id: String,
    /// Nom lisible du watcher.
    pub watcher_name: String,
    /// Cluster concerné.
    pub cluster: String,
    /// Kind de l'objet ciblé.
    pub target_kind: String,
    /// Namespace de l'objet ciblé.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_namespace: Option<String>,
    /// Nom de l'objet ciblé.
    pub target_name: String,
    /// Conteneur concerné.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Version actuellement déployée.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_version: Option<String>,
    /// Image actuellement déployée.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_image: Option<String>,
    /// Version disponible en amont.
    pub available_version: String,
    /// Image complète à déployer pour obtenir cette version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_image: Option<String>,
    /// Source ayant permis la détection.
    pub source: UpdateSource,
    /// Ampleur du saut de version.
    #[serde(default)]
    pub severity: UpdateSeverity,
    /// Détails de la release amont.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<ReleaseInfo>,
    /// Date de détection.
    pub detected_at: DateTime<Utc>,
    /// La mise à jour a déjà été appliquée.
    #[serde(default)]
    pub applied: bool,
}

impl UpdateFinding {
    /// Cible du constat sous forme structurée.
    pub fn target(&self) -> WatchTarget {
        WatchTarget {
            namespace: self.target_namespace.clone(),
            kind: self.target_kind.clone(),
            name: self.target_name.clone(),
        }
    }

    /// Identifiant déterministe d'un constat : deux détections identiques se
    /// réconcilient au lieu de s'empiler dans la liste.
    pub fn deterministic_id(watcher_id: &str, available_version: &str) -> String {
        format!("{watcher_id}:{available_version}")
    }
}

/// Résultat d'une application (ou d'un retour arrière) de mise à jour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RolloutResult {
    /// Constat appliqué ; chaîne vide pour un retour arrière manuel.
    #[serde(default)]
    pub finding_id: String,
    /// Objet Kubernetes modifié.
    pub target: WatchTarget,
    /// Image avant modification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_image: Option<String>,
    /// Image après modification.
    pub new_image: String,
    /// Date d'application.
    pub applied_at: DateTime<Utc>,
    /// Statut lisible (`"applied"`, `"rolledBack"`, `"failed: ..."`).
    pub status: String,
}

impl RolloutResult {
    /// Statut conventionnel d'une application réussie.
    pub const STATUS_APPLIED: &'static str = "applied";
    /// Statut conventionnel d'un retour arrière réussi.
    pub const STATUS_ROLLED_BACK: &'static str = "rolledBack";
    /// Statut conventionnel d'un échec.
    pub const STATUS_FAILED: &'static str = "failed";

    /// Indique si l'opération a réussi.
    pub fn succeeded(&self) -> bool {
        !self.status.starts_with(Self::STATUS_FAILED)
    }
}

/// État de convergence d'un déploiement après modification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RolloutStatus {
    /// Répliques prêtes.
    pub ready_replicas: i32,
    /// Répliques déjà à la nouvelle version.
    pub updated_replicas: i32,
    /// Répliques souhaitées.
    pub desired_replicas: i32,
    /// Le déploiement est considéré comme disponible.
    pub available: bool,
    /// Message lisible résumant l'état.
    pub message: String,
}

/// Compteurs affichés dans l'interface pour le module de mise à jour.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdaterStats {
    /// Nombre total de watchers enregistrés.
    pub watchers: usize,
    /// Nombre de watchers actifs.
    pub enabled: usize,
    /// Nombre de constats en attente.
    pub findings: usize,
    /// Date de la dernière campagne de vérification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<DateTime<Utc>>,
    /// Quota GitHub restant, si connu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_rate_remaining: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_serialisee_avec_tag_et_camel_case() {
        let s = UpdateSource::GithubRelease {
            owner: "kubewatch-io".into(),
            repo: "kubewatch".into(),
            tag_prefix: Some("release-".into()),
        };
        let j = serde_json::to_value(&s).expect("sérialisation");
        assert_eq!(j["type"], "githubRelease");
        assert_eq!(j["owner"], "kubewatch-io");
        assert_eq!(j["tagPrefix"], "release-");
        let back: UpdateSource = serde_json::from_value(j).expect("désérialisation");
        assert_eq!(back, s);
    }

    #[test]
    fn source_image_et_chart() {
        let img = UpdateSource::ContainerRegistry {
            image: "nginx:1.27".into(),
        };
        assert_eq!(
            serde_json::to_value(&img).expect("json")["type"],
            "containerRegistry"
        );
        assert_eq!(img.label(), "image:nginx:1.27");
        let chart = UpdateSource::HelmChart {
            repo: "https://charts.bitnami.com".into(),
            chart: "redis".into(),
        };
        assert_eq!(
            serde_json::to_value(&chart).expect("json")["type"],
            "helmChart"
        );
    }

    #[test]
    fn politique_par_defaut() {
        let p = UpdatePolicy::default();
        assert_eq!(p.channel, UpdateChannel::Patch);
        assert!(!p.allow_prerelease);
        assert!(!p.auto_apply);
        assert_eq!(p.check_interval_seconds, 3600);
        assert!(p.constraint.is_none());
        assert!(p.maintenance_window.is_none());
        assert!(p.ignore.is_empty());
    }

    #[test]
    fn politique_tolere_un_json_partiel() {
        let p: UpdatePolicy = serde_json::from_str(r#"{"channel":"minor"}"#).expect("json partiel");
        assert_eq!(p.channel, UpdateChannel::Minor);
        assert_eq!(p.check_interval_seconds, 3600);
    }

    #[test]
    fn intervalle_borne_a_soixante_secondes() {
        let p = UpdatePolicy {
            check_interval_seconds: 5,
            ..Default::default()
        };
        assert_eq!(p.check_interval().as_secs(), 60);
    }

    #[test]
    fn prerelease_autorisee_par_le_canal() {
        let p = UpdatePolicy {
            channel: UpdateChannel::Prerelease,
            ..Default::default()
        };
        assert!(p.prerelease_allowed());
    }

    #[test]
    fn canaux_en_camel_case() {
        assert_eq!(
            serde_json::to_string(&UpdateChannel::Prerelease).expect("json"),
            "\"prerelease\""
        );
        assert_eq!(
            serde_json::to_string(&UpdateSeverity::Major).expect("json"),
            "\"major\""
        );
    }

    #[test]
    fn watcher_spec_aller_retour_json() {
        let w = WatcherSpec::new(
            "nginx prod",
            "prod",
            WatchTarget::new("Deployment", Some("web".into()), "nginx"),
            UpdateSource::ContainerRegistry {
                image: "nginx".into(),
            },
            UpdatePolicy::default(),
        );
        let j = serde_json::to_string(&w).expect("sérialisation");
        assert!(j.contains("\"createdAt\""));
        let back: WatcherSpec = serde_json::from_str(&j).expect("désérialisation");
        assert_eq!(back.id, w.id);
        assert_eq!(back.target.kind, "Deployment");
        assert!(back.enabled);
    }

    #[test]
    fn libelle_de_cible() {
        assert_eq!(
            WatchTarget::new("Deployment", Some("web".into()), "nginx").label(),
            "web/Deployment/nginx"
        );
        assert_eq!(WatchTarget::new("Node", None, "n1").label(), "Node/n1");
    }

    #[test]
    fn finding_expose_sa_cible() {
        let f = UpdateFinding {
            id: UpdateFinding::deterministic_id("w1", "1.2.3"),
            watcher_id: "w1".into(),
            watcher_name: "nginx".into(),
            cluster: "prod".into(),
            target_kind: "Deployment".into(),
            target_namespace: Some("web".into()),
            target_name: "nginx".into(),
            container: None,
            current_version: Some("1.2.2".into()),
            current_image: Some("nginx:1.2.2".into()),
            available_version: "1.2.3".into(),
            available_image: Some("nginx:1.2.3".into()),
            source: UpdateSource::ContainerRegistry {
                image: "nginx".into(),
            },
            severity: UpdateSeverity::Patch,
            release: None,
            detected_at: Utc::now(),
            applied: false,
        };
        assert_eq!(f.id, "w1:1.2.3");
        assert_eq!(f.target().label(), "web/Deployment/nginx");
        let j = serde_json::to_value(&f).expect("json");
        assert_eq!(j["availableVersion"], "1.2.3");
        assert_eq!(j["targetNamespace"], "web");
        assert_eq!(j["severity"], "patch");
    }

    #[test]
    fn statut_de_rollout() {
        let ok = RolloutResult {
            finding_id: "w1:1.2.3".into(),
            target: WatchTarget::new("Deployment", Some("web".into()), "nginx"),
            previous_image: Some("nginx:1.2.2".into()),
            new_image: "nginx:1.2.3".into(),
            applied_at: Utc::now(),
            status: RolloutResult::STATUS_APPLIED.into(),
        };
        assert!(ok.succeeded());
        let ko = RolloutResult {
            status: "failed: quota".into(),
            ..ok
        };
        assert!(!ko.succeeded());
    }

    #[test]
    fn release_depuis_un_tag() {
        let mut r = ReleaseInfo::from_tag("v1.2.3");
        assert!(r.parsed_semver().is_none());
        r.semver = Some("1.2.3".into());
        assert_eq!(r.parsed_semver().expect("semver").major, 1);
    }
}
