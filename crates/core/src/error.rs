//! Type d'erreur unifié du cœur de KubeWatch.
//!
//! Toutes les fonctions publiques du crate renvoient [`Result<T>`]. Chaque variante sait
//! se traduire en code HTTP (`status_code`) et en identifiant machine (`kind_str`) afin que
//! la couche HTTP puisse produire un corps `{"error": {"kind": ..., "message": ...}}`
//! homogène sans connaître le détail des erreurs internes.

use thiserror::Error;

/// Alias de résultat utilisé partout dans `kubewatch-core`.
pub type Result<T> = std::result::Result<T, Error>;

/// Erreur du cœur de KubeWatch.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Erreur remontée par le client Kubernetes.
    #[error("erreur Kubernetes: {0}")]
    Kube(#[from] kube::Error),

    /// Kubeconfig illisible, incomplet ou contexte inconnu.
    #[error("kubeconfig invalide: {0}")]
    KubeConfig(String),

    /// Échec de la découverte des ressources de l'API.
    #[error("découverte d'API impossible: {0}")]
    Discovery(String),

    /// Ressource, contexte ou cluster introuvable.
    #[error("introuvable: {0}")]
    NotFound(String),

    /// Requête ou paramètre invalide fourni par l'appelant.
    #[error("requête invalide: {0}")]
    Invalid(String),

    /// Document YAML illisible ou non conforme.
    #[error("YAML invalide: {0}")]
    Yaml(String),

    /// Erreur de (dé)sérialisation JSON.
    #[error("JSON invalide: {0}")]
    Json(#[from] serde_json::Error),

    /// Erreur d'entrée/sortie (fichier d'état, kubeconfig, socket...).
    #[error("erreur d'entrée/sortie: {0}")]
    Io(#[from] std::io::Error),

    /// Opération non supportée par le cluster ou par cette version de KubeWatch.
    #[error("opération non supportée: {0}")]
    Unsupported(String),

    /// Conflit de version ou ressource déjà existante.
    #[error("conflit: {0}")]
    Conflict(String),

    /// Droits insuffisants (RBAC).
    #[error("accès refusé: {0}")]
    Forbidden(String),

    /// Délai d'attente dépassé.
    #[error("délai dépassé: {0}")]
    Timeout(String),

    /// Toute autre erreur.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Code HTTP conseillé pour cette erreur.
    pub fn status_code(&self) -> u16 {
        match self {
            Error::Kube(e) => kube_status_code(e),
            Error::KubeConfig(_) => 400,
            Error::Discovery(_) => 502,
            Error::NotFound(_) => 404,
            Error::Invalid(_) => 400,
            Error::Yaml(_) => 400,
            Error::Json(_) => 400,
            Error::Io(_) => 500,
            Error::Unsupported(_) => 501,
            Error::Conflict(_) => 409,
            Error::Forbidden(_) => 403,
            Error::Timeout(_) => 408,
            Error::Other(_) => 500,
        }
    }

    /// Identifiant machine stable, exposé dans le corps JSON des erreurs de l'API.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Error::Kube(_) => "kube",
            Error::KubeConfig(_) => "kubeConfig",
            Error::Discovery(_) => "discovery",
            Error::NotFound(_) => "notFound",
            Error::Invalid(_) => "invalid",
            Error::Yaml(_) => "yaml",
            Error::Json(_) => "json",
            Error::Io(_) => "io",
            Error::Unsupported(_) => "unsupported",
            Error::Conflict(_) => "conflict",
            Error::Forbidden(_) => "forbidden",
            Error::Timeout(_) => "timeout",
            Error::Other(_) => "other",
        }
    }

    /// Vrai si l'erreur correspond à une ressource absente, quelle que soit sa variante.
    pub fn is_not_found(&self) -> bool {
        self.status_code() == 404
    }

    /// Convertit une erreur `kube` en variante sémantique lorsque le serveur a renvoyé un
    /// statut explicite (404, 403, 409, 408). Les autres cas restent en [`Error::Kube`].
    pub fn from_kube(e: kube::Error) -> Self {
        match kube_status_code(&e) {
            404 => Error::NotFound(kube_message(&e)),
            403 | 401 => Error::Forbidden(kube_message(&e)),
            409 => Error::Conflict(kube_message(&e)),
            408 | 504 => Error::Timeout(kube_message(&e)),
            400 | 422 => Error::Invalid(kube_message(&e)),
            _ => Error::Kube(e),
        }
    }

    /// Construit une erreur générique à partir de n'importe quel message.
    pub fn other(msg: impl std::fmt::Display) -> Self {
        Error::Other(msg.to_string())
    }
}

/// Extrait le code HTTP d'une erreur `kube` (500 si le serveur n'en a pas fourni).
fn kube_status_code(e: &kube::Error) -> u16 {
    match e {
        kube::Error::Api(status) if status.code != 0 => status.code,
        kube::Error::Auth(_) => 401,
        kube::Error::Discovery(_) => 502,
        kube::Error::SerdeError(_) => 500,
        _ => 500,
    }
}

/// Message le plus parlant disponible pour une erreur `kube`.
fn kube_message(e: &kube::Error) -> String {
    match e {
        kube::Error::Api(status) if !status.message.is_empty() => status.message.clone(),
        other => other.to_string(),
    }
}

impl From<serde_yaml_ng::Error> for Error {
    fn from(e: serde_yaml_ng::Error) -> Self {
        Error::Yaml(e.to_string())
    }
}

impl From<kube::config::KubeconfigError> for Error {
    fn from(e: kube::config::KubeconfigError) -> Self {
        Error::KubeConfig(e.to_string())
    }
}

impl From<kube::config::InferConfigError> for Error {
    fn from(e: kube::config::InferConfigError) -> Self {
        Error::KubeConfig(e.to_string())
    }
}

impl From<kube::config::InClusterError> for Error {
    fn from(e: kube::config::InClusterError) -> Self {
        Error::KubeConfig(e.to_string())
    }
}

impl From<http::Error> for Error {
    fn from(e: http::Error) -> Self {
        Error::Other(format!("requête HTTP mal formée: {e}"))
    }
}

impl From<http::uri::InvalidUri> for Error {
    fn from(e: http::uri::InvalidUri) -> Self {
        Error::Invalid(format!("URL invalide: {e}"))
    }
}

impl From<base64::DecodeError> for Error {
    fn from(e: base64::DecodeError) -> Self {
        Error::Invalid(format!("base64 invalide: {e}"))
    }
}

impl From<std::string::FromUtf8Error> for Error {
    fn from(e: std::string::FromUtf8Error) -> Self {
        Error::Invalid(format!("séquence UTF-8 invalide: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_http_par_variante() {
        assert_eq!(Error::NotFound("pod".into()).status_code(), 404);
        assert_eq!(Error::Invalid("champ".into()).status_code(), 400);
        assert_eq!(Error::Conflict("rv".into()).status_code(), 409);
        assert_eq!(Error::Forbidden("rbac".into()).status_code(), 403);
        assert_eq!(Error::Timeout("10s".into()).status_code(), 408);
        assert_eq!(Error::Unsupported("helm".into()).status_code(), 501);
        assert_eq!(Error::Other("bam".into()).status_code(), 500);
        assert_eq!(Error::Yaml("bad".into()).status_code(), 400);
    }

    #[test]
    fn identifiants_machine_stables() {
        assert_eq!(Error::NotFound("x".into()).kind_str(), "notFound");
        assert_eq!(Error::KubeConfig("x".into()).kind_str(), "kubeConfig");
        assert!(Error::NotFound("x".into()).is_not_found());
        assert!(!Error::Invalid("x".into()).is_not_found());
    }

    #[test]
    fn conversion_statut_kube() {
        let status = kube::core::Status {
            code: 404,
            message: "pods \"x\" not found".to_string(),
            ..Default::default()
        };
        let err = Error::from_kube(kube::Error::Api(Box::new(status)));
        assert!(matches!(err, Error::NotFound(_)));
        assert_eq!(err.status_code(), 404);
    }

    #[test]
    fn conversion_yaml() {
        let err: Error = serde_yaml_ng::from_str::<serde_yaml_ng::Value>("a: [\n")
            .unwrap_err()
            .into();
        assert_eq!(err.kind_str(), "yaml");
        assert_eq!(err.status_code(), 400);
    }
}
