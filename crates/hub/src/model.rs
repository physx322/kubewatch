//! Types de données exposés par le hub : références d'images, résumés de recherche,
//! tags, détails d'image, charts Helm et applications du catalogue intégré.
//!
//! Tous les DTO renvoyés par l'API HTTP sont sérialisés en `camelCase`.

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Registre utilisé par défaut quand la référence n'en mentionne aucun.
pub const DEFAULT_REGISTRY: &str = "docker.io";

/// Espace de noms implicite des images officielles Docker Hub.
pub const DOCKER_OFFICIAL_NAMESPACE: &str = "library";

/// Point d'entrée réel de l'API Registry v2 de Docker Hub (`docker.io` n'en est qu'un alias).
pub const DOCKER_REGISTRY_HOST: &str = "registry-1.docker.io";

/// Tag appliqué quand la référence n'en précise aucun et ne porte pas de digest.
pub const DEFAULT_TAG: &str = "latest";

/// Un segment de dépôt : `bitnami`, `nginx`, `mon-app.v2`…
static REPOSITORY_SEGMENT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^[a-zA-Z0-9]([a-zA-Z0-9._-]*[a-zA-Z0-9])?$").expect("motif de segment valide")
});

/// Un tag d'image selon la spécification OCI de distribution.
static TAG_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[A-Za-z0-9_][A-Za-z0-9._-]{0,127}$").expect("motif de tag valide"));

/// Un digest `algorithme:hexadécimal`.
static DIGEST_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z0-9]+(?:[.+_-][a-z0-9]+)*:[a-fA-F0-9]{32,}$").expect("motif de digest valide"));

/// Famille de registre, déterminée à partir de l'hôte de la référence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RegistryKind {
    /// Docker Hub (`docker.io`), famille par défaut.
    #[serde(alias = "dockerhub", alias = "docker", alias = "docker.io", alias = "hub")]
    #[default]
    DockerHub,
    /// GitHub Container Registry (`ghcr.io`).
    #[serde(alias = "github", alias = "ghcr.io")]
    Ghcr,
    /// Quay (`quay.io`).
    #[serde(alias = "quay.io", alias = "redhat")]
    Quay,
    /// Tout autre registre conforme à l'API Registry v2.
    #[serde(alias = "other", alias = "custom")]
    Generic,
}

impl RegistryKind {
    /// Déduit la famille de registre depuis un nom d'hôte.
    pub fn from_host(host: &str) -> Self {
        let host = host.trim().trim_end_matches('/').to_ascii_lowercase();
        match host.as_str() {
            "docker.io" | "index.docker.io" | "registry-1.docker.io" | "registry.hub.docker.com" => {
                RegistryKind::DockerHub
            }
            "ghcr.io" => RegistryKind::Ghcr,
            "quay.io" => RegistryKind::Quay,
            _ => RegistryKind::Generic,
        }
    }

    /// Analyse tolérante d'une valeur venue d'un paramètre de requête HTTP.
    pub fn from_hint(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "docker" | "dockerhub" | "docker.io" | "hub" => RegistryKind::DockerHub,
            "ghcr" | "github" | "ghcr.io" => RegistryKind::Ghcr,
            "quay" | "quay.io" | "redhat" => RegistryKind::Quay,
            _ => RegistryKind::Generic,
        }
    }

    /// Nom d'hôte canonique de la famille, ou `None` pour un registre générique.
    pub fn default_host(&self) -> Option<&'static str> {
        match self {
            RegistryKind::DockerHub => Some(DEFAULT_REGISTRY),
            RegistryKind::Ghcr => Some("ghcr.io"),
            RegistryKind::Quay => Some("quay.io"),
            RegistryKind::Generic => None,
        }
    }

    /// Libellé lisible, affiché dans l'interface.
    pub fn label(&self) -> &'static str {
        match self {
            RegistryKind::DockerHub => "Docker Hub",
            RegistryKind::Ghcr => "GitHub Container Registry",
            RegistryKind::Quay => "Quay",
            RegistryKind::Generic => "Registre OCI",
        }
    }
}

/// Référence complète d'une image de conteneur, décomposée en ses quatre parties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRef {
    /// Hôte du registre, normalisé (`docker.io`, `ghcr.io`, `registry.local:5000`…).
    pub registry: String,
    /// Chemin du dépôt, préfixé de `library/` pour les images officielles Docker Hub.
    pub repository: String,
    /// Tag demandé, `latest` par défaut en l'absence de digest.
    pub tag: Option<String>,
    /// Digest `sha256:…` quand la référence en porte un.
    pub digest: Option<String>,
}

impl ImageRef {
    /// Analyse une référence d'image sous toutes ses formes usuelles.
    ///
    /// Exemples acceptés : `nginx`, `nginx:1.27`, `library/nginx`, `ghcr.io/o/r:tag`,
    /// `registry:5000/o/r@sha256:…`, `quay.io/o/r`.
    pub fn parse(s: &str) -> Result<Self> {
        let raw = s.trim();
        if raw.is_empty() {
            return Err(Error::Invalid(
                "référence d'image vide : indiquez par exemple « nginx:1.27 »".to_string(),
            ));
        }
        if raw.chars().any(|c| c.is_whitespace()) {
            return Err(Error::Invalid(format!(
                "référence d'image « {raw} » invalide : elle ne doit pas contenir d'espace"
            )));
        }

        // 1. Détacher le digest éventuel.
        let (rest, digest) = match raw.split_once('@') {
            Some((head, tail)) => {
                if !DIGEST_PATTERN.is_match(tail) {
                    return Err(Error::Invalid(format!(
                        "digest « {tail} » invalide : attendu « sha256:<hexadécimal> »"
                    )));
                }
                (head, Some(tail.to_ascii_lowercase()))
            }
            None => (raw, None),
        };
        if rest.is_empty() {
            return Err(Error::Invalid(format!(
                "référence « {raw} » invalide : le nom du dépôt est manquant avant le digest"
            )));
        }

        // 2. Détacher l'hôte du registre : le premier segment n'en est un que s'il
        //    ressemble à un nom d'hôte (point, port, ou « localhost »).
        let (registry, remainder) = match rest.split_once('/') {
            Some((head, tail))
                if head == "localhost" || head.contains('.') || head.contains(':') =>
            {
                if tail.is_empty() {
                    return Err(Error::Invalid(format!(
                        "référence « {raw} » invalide : nom de dépôt manquant après « {head} »"
                    )));
                }
                (head.to_ascii_lowercase(), tail.to_string())
            }
            _ => (DEFAULT_REGISTRY.to_string(), rest.to_string()),
        };
        let registry = normalize_registry(&registry);

        // 3. Détacher le tag : dans ce qui reste, un « : » ne peut plus être un port.
        let (repository, tag) = match remainder.rfind(':') {
            Some(i) if !remainder[i + 1..].contains('/') => {
                let tag = &remainder[i + 1..];
                if tag.is_empty() {
                    return Err(Error::Invalid(format!(
                        "référence « {raw} » invalide : tag vide après « : »"
                    )));
                }
                if !TAG_PATTERN.is_match(tag) {
                    return Err(Error::Invalid(format!("tag « {tag} » invalide")));
                }
                (remainder[..i].to_string(), Some(tag.to_string()))
            }
            _ => (remainder.clone(), None),
        };

        if repository.is_empty() {
            return Err(Error::Invalid(format!(
                "référence « {raw} » invalide : nom de dépôt manquant"
            )));
        }
        for segment in repository.split('/') {
            if !REPOSITORY_SEGMENT.is_match(segment) {
                return Err(Error::Invalid(format!(
                    "dépôt « {repository} » invalide : le segment « {segment} » n'est pas un nom valide"
                )));
            }
        }

        // 4. Les images officielles Docker Hub vivent sous « library/ ».
        let repository = if registry == DEFAULT_REGISTRY && !repository.contains('/') {
            format!("{DOCKER_OFFICIAL_NAMESPACE}/{repository}")
        } else {
            repository
        };

        // 5. Sans tag ni digest, on vise « latest ».
        let tag = match (tag, &digest) {
            (Some(t), _) => Some(t),
            (None, Some(_)) => None,
            (None, None) => Some(DEFAULT_TAG.to_string()),
        };

        Ok(ImageRef { registry, repository, tag, digest })
    }

    /// Reconstruit la référence canonique complète, registre inclus.
    pub fn to_string_full(&self) -> String {
        let mut out = format!("{}/{}", self.registry, self.repository);
        if let Some(tag) = &self.tag {
            out.push(':');
            out.push_str(tag);
        }
        if let Some(digest) = &self.digest {
            out.push('@');
            out.push_str(digest);
        }
        out
    }

    /// Famille du registre de cette référence.
    pub fn registry_kind(&self) -> RegistryKind {
        RegistryKind::from_host(&self.registry)
    }

    /// Hôte à contacter pour l'API Registry v2 (`docker.io` redirige vers `registry-1.docker.io`).
    pub fn registry_host(&self) -> String {
        if self.registry == DEFAULT_REGISTRY {
            DOCKER_REGISTRY_HOST.to_string()
        } else {
            self.registry.clone()
        }
    }

    /// Référence à utiliser pour interroger un manifeste : le digest s'il existe, sinon le tag.
    pub fn manifest_reference(&self) -> String {
        self.digest
            .clone()
            .or_else(|| self.tag.clone())
            .unwrap_or_else(|| DEFAULT_TAG.to_string())
    }

    /// Nom court affiché dans l'interface (`nginx`, `bitnami/postgresql`…).
    pub fn short_name(&self) -> String {
        if self.registry == DEFAULT_REGISTRY {
            match self.repository.strip_prefix("library/") {
                Some(name) => name.to_string(),
                None => self.repository.clone(),
            }
        } else {
            format!("{}/{}", self.registry, self.repository)
        }
    }

    /// Même image, mais épinglée sur un autre tag.
    pub fn with_tag(&self, tag: &str) -> ImageRef {
        ImageRef {
            registry: self.registry.clone(),
            repository: self.repository.clone(),
            tag: Some(tag.to_string()),
            digest: None,
        }
    }

    /// Portée OAuth demandée au service de jetons du registre pour une lecture.
    pub fn pull_scope(&self) -> String {
        format!("repository:{}:pull", self.repository)
    }
}

impl std::fmt::Display for ImageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_string_full())
    }
}

impl std::str::FromStr for ImageRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        ImageRef::parse(s)
    }
}

/// Ramène les alias connus de Docker Hub sur l'hôte canonique.
fn normalize_registry(host: &str) -> String {
    match host {
        "index.docker.io" | "registry-1.docker.io" | "registry.hub.docker.com" => {
            DEFAULT_REGISTRY.to_string()
        }
        other => other.to_string(),
    }
}

/// Résultat de recherche d'image, tel que renvoyé par un registre.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSummary {
    /// Nom du dépôt tel qu'il s'affiche (`nginx`, `bitnami/postgresql`…).
    pub name: String,
    /// Espace de noms propriétaire, quand le registre le distingue du nom.
    pub namespace: Option<String>,
    /// Description courte fournie par le registre.
    pub description: Option<String>,
    /// Nombre d'étoiles, si le registre l'expose.
    pub stars: Option<i64>,
    /// Nombre de téléchargements, si le registre l'expose.
    pub pulls: Option<i64>,
    /// Vrai pour les images officielles (Docker Hub `library/`).
    pub official: bool,
    /// Registre d'origine du résultat.
    pub registry: RegistryKind,
    /// Date de dernière mise à jour connue.
    pub updated_at: Option<DateTime<Utc>>,
    /// Dépôt de code source déduit des métadonnées, au format `propriétaire/dépôt`.
    pub source_repo: Option<String>,
}

impl ImageSummary {
    /// Résumé minimal pour un dépôt dont on ne connaît que la référence.
    pub fn from_reference(image: &ImageRef) -> Self {
        ImageSummary {
            name: image.short_name(),
            namespace: image
                .repository
                .rsplit_once('/')
                .map(|(ns, _)| ns.to_string()),
            description: None,
            stars: None,
            pulls: None,
            official: image.registry == DEFAULT_REGISTRY
                && image.repository.starts_with("library/"),
            registry: image.registry_kind(),
            updated_at: None,
            source_repo: None,
        }
    }
}

/// Un tag d'image et ses métadonnées.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagInfo {
    /// Nom du tag (`1.27.1`, `latest`, `16-alpine`…).
    pub name: String,
    /// Digest du manifeste, quand le registre le fournit dans la liste.
    pub digest: Option<String>,
    /// Taille compressée totale en octets, quand elle est connue.
    pub size_bytes: Option<i64>,
    /// Date de publication du tag.
    pub pushed_at: Option<DateTime<Utc>>,
    /// Plates-formes disponibles, au format `os/architecture`.
    pub platforms: Vec<String>,
    /// Version sémantique normalisée du tag, quand il en porte une.
    pub semver: Option<String>,
}

impl TagInfo {
    /// Construit une entrée ne portant que le nom du tag (cas de l'API Registry v2 nue).
    pub fn bare(name: impl Into<String>) -> Self {
        let name = name.into();
        let semver = crate::registry::parse_tag(&name).map(|t| t.version.to_string());
        TagInfo { name, digest: None, size_bytes: None, pushed_at: None, platforms: Vec::new(), semver }
    }
}

/// Métadonnées extraites de la configuration OCI d'une image.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDetails {
    /// Référence complète inspectée.
    pub reference: String,
    /// Digest du manifeste (ou de l'index multi-architecture).
    pub digest: Option<String>,
    /// Ports déclarés par l'image, extraits de `config.ExposedPorts`.
    pub exposed_ports: Vec<u16>,
    /// Variables d'environnement par défaut, au format `CLÉ=valeur`.
    pub env: Vec<String>,
    /// Étiquettes OCI de l'image.
    pub labels: BTreeMap<String, String>,
    /// Point d'entrée du conteneur.
    pub entrypoint: Vec<String>,
    /// Arguments par défaut.
    pub cmd: Vec<String>,
    /// Architecture du manifeste retenu (`amd64`, `arm64`…).
    pub architecture: Option<String>,
    /// Système d'exploitation du manifeste retenu.
    pub os: Option<String>,
    /// Dépôt de code source, déduit de `org.opencontainers.image.source`.
    pub source_repo: Option<String>,
}

/// Résumé d'un chart Helm publié sur Artifact Hub.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartSummary {
    /// Nom du chart.
    pub name: String,
    /// Nom du dépôt Artifact Hub qui l'héberge.
    pub repository: String,
    /// Version du chart.
    pub version: String,
    /// Version de l'application empaquetée.
    pub app_version: Option<String>,
    /// Description courte.
    pub description: Option<String>,
    /// URL de l'icône.
    pub icon: Option<String>,
    /// Nombre d'étoiles sur Artifact Hub.
    pub stars: Option<i64>,
    /// Site du projet.
    pub home: Option<String>,
    /// URL du dépôt Helm, utilisable avec `helm template --repo`.
    pub repo_url: Option<String>,
}

/// Application prête à déployer, issue du catalogue intégré.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogApp {
    /// Identifiant stable, utilisé par l'interface.
    pub id: String,
    /// Nom affiché.
    pub name: String,
    /// Description en français.
    pub description: String,
    /// URL d'une icône, quand elle est connue.
    pub icon: Option<String>,
    /// Catégorie de classement (`Base de données`, `Observabilité`…).
    pub category: String,
    /// Image épinglée sur un tag stable.
    pub image: String,
    /// Port principal exposé par l'application.
    pub default_port: Option<u16>,
    /// Variables d'environnement requises ou fortement recommandées.
    pub env: Vec<(String, String)>,
    /// Vrai si l'application a besoin d'un volume persistant.
    pub needs_pvc: bool,
    /// Chart Helm officiel équivalent, quand il existe.
    pub chart: Option<ChartSummary>,
    /// Documentation en ligne.
    pub docs_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> ImageRef {
        ImageRef::parse(s).unwrap_or_else(|e| panic!("« {s} » aurait dû être analysable : {e}"))
    }

    #[test]
    fn image_officielle_sans_tag() {
        let r = p("nginx");
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "library/nginx");
        assert_eq!(r.tag.as_deref(), Some("latest"));
        assert_eq!(r.digest, None);
        assert_eq!(r.registry_kind(), RegistryKind::DockerHub);
        assert_eq!(r.short_name(), "nginx");
    }

    #[test]
    fn image_officielle_avec_tag() {
        let r = p("nginx:1.27");
        assert_eq!(r.repository, "library/nginx");
        assert_eq!(r.tag.as_deref(), Some("1.27"));
    }

    #[test]
    fn forme_library_explicite() {
        let r = p("library/nginx");
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "library/nginx");
    }

    #[test]
    fn depot_utilisateur_docker_hub() {
        let r = p("bitnami/postgresql:16.4.0");
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "bitnami/postgresql");
        assert_eq!(r.tag.as_deref(), Some("16.4.0"));
        // Forme courte canonique Docker : le registre par défaut n'est pas préfixé
        // (`docker pull bitnami/postgresql` est la façon dont on écrit cette image).
        assert_eq!(r.short_name(), "bitnami/postgresql");
    }

    #[test]
    fn ghcr_avec_tag() {
        let r = p("ghcr.io/proprietaire/depot:v2.1.0");
        assert_eq!(r.registry, "ghcr.io");
        assert_eq!(r.repository, "proprietaire/depot");
        assert_eq!(r.tag.as_deref(), Some("v2.1.0"));
        assert_eq!(r.registry_kind(), RegistryKind::Ghcr);
        assert_eq!(r.registry_host(), "ghcr.io");
    }

    #[test]
    fn quay_sans_tag() {
        let r = p("quay.io/prometheus/node-exporter");
        assert_eq!(r.registry_kind(), RegistryKind::Quay);
        assert_eq!(r.repository, "prometheus/node-exporter");
        assert_eq!(r.tag.as_deref(), Some("latest"));
    }

    #[test]
    fn registre_prive_avec_port_et_digest() {
        let r = p("registry:5000/equipe/service@sha256:abababababababababababababababababababababababababababababababab");
        assert_eq!(r.registry, "registry:5000");
        assert_eq!(r.repository, "equipe/service");
        assert_eq!(r.tag, None);
        assert!(r.digest.is_some());
        assert_eq!(r.registry_kind(), RegistryKind::Generic);
    }

    #[test]
    fn localhost_est_un_registre() {
        let r = p("localhost:5000/app:dev");
        assert_eq!(r.registry, "localhost:5000");
        assert_eq!(r.repository, "app");
        assert_eq!(r.tag.as_deref(), Some("dev"));
    }

    #[test]
    fn alias_docker_hub_normalise() {
        let r = p("index.docker.io/library/redis:7.4");
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.registry_host(), "registry-1.docker.io");
    }

    #[test]
    fn tag_et_digest_simultanes() {
        let r = p("ghcr.io/o/r:1.0@sha256:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd");
        assert_eq!(r.tag.as_deref(), Some("1.0"));
        assert!(r.digest.is_some());
        assert!(r.to_string_full().contains(":1.0@sha256:"));
    }

    #[test]
    fn depot_imbrique_style_harbor() {
        let r = p("harbor.interne.lan/projet/equipe/api:2.3.4");
        assert_eq!(r.registry, "harbor.interne.lan");
        assert_eq!(r.repository, "projet/equipe/api");
    }

    #[test]
    fn aller_retour_textuel() {
        let r = p("ghcr.io/o/r:1.2.3");
        assert_eq!(r.to_string_full(), "ghcr.io/o/r:1.2.3");
        assert_eq!(p("nginx").to_string_full(), "docker.io/library/nginx:latest");
    }

    #[test]
    fn references_invalides_rejetees() {
        for mauvais in [
            "",
            "   ",
            "nginx:",
            "nginx :1.0",
            "ghcr.io/",
            "nginx@sha256:pastuncondensat",
            "nginx@abc",
            "-mauvais/depot",
        ] {
            assert!(
                ImageRef::parse(mauvais).is_err(),
                "« {mauvais} » aurait dû être rejeté"
            );
        }
    }

    #[test]
    fn portee_oauth_de_lecture() {
        assert_eq!(p("nginx").pull_scope(), "repository:library/nginx:pull");
    }

    #[test]
    fn deduction_de_famille_depuis_lhote() {
        assert_eq!(RegistryKind::from_host("GHCR.IO"), RegistryKind::Ghcr);
        assert_eq!(RegistryKind::from_host("registry.local:5000"), RegistryKind::Generic);
        assert_eq!(RegistryKind::from_hint("docker"), RegistryKind::DockerHub);
        assert_eq!(RegistryKind::from_hint("inconnu"), RegistryKind::Generic);
    }
}
