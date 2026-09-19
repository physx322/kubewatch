//! Type d'erreur unifié du crate `kubewatch-updater`.
//!
//! La forme est volontairement identique à celle de `kubewatch-core` et
//! `kubewatch-hub` : un `enum Error` (thiserror), un alias `Result<T>`, et deux
//! accesseurs (`status_code`, `kind_str`) que la couche HTTP utilise pour
//! produire `{"error": {"kind": "...", "message": "..."}}` avec le bon statut.

use thiserror::Error as ThisError;

/// Formate le délai d'attente d'un quota d'API pour l'affichage.
fn fmt_retry(secs: &Option<u64>) -> String {
    match secs {
        Some(s) => format!(" (réessayer dans {s}s)"),
        None => String::new(),
    }
}

/// Alias de résultat utilisé partout dans le crate updater.
pub type Result<T> = std::result::Result<T, Error>;

/// Erreurs remontées par le détecteur de mises à jour.
#[derive(Debug, ThisError)]
pub enum Error {
    /// Échec d'un appel HTTP sortant (GitHub, registre OCI, dépôt Helm).
    #[error("erreur HTTP: {0}")]
    Http(#[from] reqwest::Error),

    /// Erreur propagée depuis `kubewatch-core` (accès au cluster).
    #[error("erreur cluster: {0}")]
    Core(String),

    /// Erreur propagée depuis `kubewatch-hub` (registre d'images / charts).
    #[error("erreur hub: {0}")]
    Hub(String),

    /// Ressource, watcher ou finding introuvable.
    #[error("introuvable: {0}")]
    NotFound(String),

    /// Entrée invalide (politique, contrainte semver, identifiant de dépôt...).
    #[error("entrée invalide: {0}")]
    Invalid(String),

    /// Quota d'API atteint. Contient le nombre de secondes avant réinitialisation.
    #[error("quota d'API atteint{}", fmt_retry(.0))]
    RateLimited(Option<u64>),

    /// Authentification refusée (jeton GitHub absent, expiré ou insuffisant).
    #[error("authentification refusée: {0}")]
    Auth(String),

    /// Erreur de lecture ou d'écriture de l'état persistant.
    #[error("erreur de stockage: {0}")]
    Store(String),

    /// Erreur de (dé)sérialisation JSON.
    #[error("erreur JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// Signature de webhook invalide ou absente.
    #[error("signature invalide: {0}")]
    Signature(String),

    /// Erreur d'entrée/sortie (fichier d'état, répertoire de travail).
    #[error("erreur d'E/S: {0}")]
    Io(#[from] std::io::Error),

    /// Toute autre erreur non classée.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Constructeur pratique pour [`Error::NotFound`].
    pub fn not_found(msg: impl Into<String>) -> Self {
        Error::NotFound(msg.into())
    }

    /// Constructeur pratique pour [`Error::Invalid`].
    pub fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }

    /// Constructeur pratique pour [`Error::Auth`].
    pub fn auth(msg: impl Into<String>) -> Self {
        Error::Auth(msg.into())
    }

    /// Constructeur pratique pour [`Error::Store`].
    pub fn store(msg: impl Into<String>) -> Self {
        Error::Store(msg.into())
    }

    /// Constructeur pratique pour [`Error::Core`].
    pub fn core(msg: impl Into<String>) -> Self {
        Error::Core(msg.into())
    }

    /// Constructeur pratique pour [`Error::Hub`].
    pub fn hub(msg: impl Into<String>) -> Self {
        Error::Hub(msg.into())
    }

    /// Constructeur pratique pour [`Error::Signature`].
    pub fn signature(msg: impl Into<String>) -> Self {
        Error::Signature(msg.into())
    }

    /// Constructeur pratique pour [`Error::Other`].
    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }

    /// Code HTTP à renvoyer au client pour cette erreur.
    pub fn status_code(&self) -> u16 {
        match self {
            Error::Http(e) => e.status().map(|s| s.as_u16()).unwrap_or(502),
            Error::Core(_) | Error::Hub(_) | Error::Store(_) | Error::Io(_) | Error::Other(_) => {
                500
            }
            Error::NotFound(_) => 404,
            Error::Invalid(_) | Error::Json(_) => 400,
            Error::RateLimited(_) => 429,
            Error::Auth(_) => 401,
            Error::Signature(_) => 403,
        }
    }

    /// Identifiant stable de la famille d'erreur, exposé dans le JSON d'erreur.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Error::Http(_) => "http",
            Error::Core(_) => "core",
            Error::Hub(_) => "hub",
            Error::NotFound(_) => "notFound",
            Error::Invalid(_) => "invalid",
            Error::RateLimited(_) => "rateLimited",
            Error::Auth(_) => "auth",
            Error::Store(_) => "store",
            Error::Json(_) => "json",
            Error::Signature(_) => "signature",
            Error::Io(_) => "io",
            Error::Other(_) => "other",
        }
    }

    /// Nombre de secondes à attendre avant de réessayer, si connu.
    pub fn retry_after_seconds(&self) -> Option<u64> {
        match self {
            Error::RateLimited(s) => *s,
            _ => None,
        }
    }
}

impl From<kubewatch_core::Error> for Error {
    fn from(e: kubewatch_core::Error) -> Self {
        Error::Core(e.to_string())
    }
}

impl From<kubewatch_hub::Error> for Error {
    fn from(e: kubewatch_hub::Error) -> Self {
        Error::Hub(e.to_string())
    }
}

impl From<serde_yaml_ng::Error> for Error {
    fn from(e: serde_yaml_ng::Error) -> Self {
        Error::Invalid(format!("YAML: {e}"))
    }
}

impl From<semver::Error> for Error {
    fn from(e: semver::Error) -> Self {
        Error::Invalid(format!("semver: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_http_par_variante() {
        assert_eq!(Error::not_found("w").status_code(), 404);
        assert_eq!(Error::invalid("x").status_code(), 400);
        assert_eq!(Error::RateLimited(Some(30)).status_code(), 429);
        assert_eq!(Error::auth("nope").status_code(), 401);
        assert_eq!(Error::signature("bad").status_code(), 403);
        assert_eq!(Error::store("disque").status_code(), 500);
        assert_eq!(Error::other("boum").status_code(), 500);
    }

    #[test]
    fn kinds_stables() {
        assert_eq!(Error::not_found("w").kind_str(), "notFound");
        assert_eq!(Error::RateLimited(None).kind_str(), "rateLimited");
        assert_eq!(Error::core("c").kind_str(), "core");
        assert_eq!(Error::hub("h").kind_str(), "hub");
    }

    #[test]
    fn retry_after_expose_le_delai() {
        assert_eq!(Error::RateLimited(Some(42)).retry_after_seconds(), Some(42));
        assert_eq!(Error::RateLimited(None).retry_after_seconds(), None);
        assert_eq!(Error::other("x").retry_after_seconds(), None);
    }

    #[test]
    fn message_quota_lisible() {
        let msg = Error::RateLimited(Some(12)).to_string();
        assert!(msg.contains("12s"), "message inattendu: {msg}");
        assert!(!Error::RateLimited(None).to_string().contains("réessayer"));
    }
}
