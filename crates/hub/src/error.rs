//! Type d'erreur unifié du crate `kubewatch-hub`.
//!
//! Toutes les fonctions publiques du crate renvoient [`Result`]. Chaque variante sait se
//! traduire en code HTTP ([`Error::status_code`]) et en identifiant stable
//! ([`Error::kind_str`]) afin que la couche HTTP puisse produire l'enveloppe
//! `{"error": {"kind": "...", "message": "..."}}` sans connaître le détail des variantes.

use thiserror::Error as ThisError;

/// Complète le message de quota dépassé avec le délai conseillé quand le registre le fournit.
fn retry_hint(seconds: &Option<u64>) -> String {
    match seconds {
        Some(s) => format!(", réessayez dans {s} s"),
        None => String::new(),
    }
}

/// Erreurs produites par la recherche d'images, l'inspection de registres,
/// l'interrogation d'Artifact Hub et la génération de manifestes.
#[derive(Debug, ThisError)]
pub enum Error {
    /// Échec réseau ou statut d'erreur remonté par un registre distant.
    #[error("échec de la requête HTTP : {0}")]
    Http(#[from] reqwest::Error),

    /// L'image, le tag, le chart ou la version demandés n'existent pas.
    #[error("introuvable : {0}")]
    NotFound(String),

    /// Entrée utilisateur incorrecte (référence d'image malformée, manifeste invalide…).
    #[error("entrée invalide : {0}")]
    Invalid(String),

    /// Quota d'appels dépassé. Contient le délai d'attente conseillé, en secondes.
    #[error("quota d'appels du registre dépassé{}", retry_hint(.0))]
    RateLimited(Option<u64>),

    /// Le registre exige des identifiants, ou ceux fournis ont été refusés.
    #[error("authentification refusée : {0}")]
    Auth(String),

    /// Fonctionnalité indisponible dans cet environnement (binaire absent, API inexistante…).
    #[error("opération non prise en charge : {0}")]
    Unsupported(String),

    /// Réponse JSON illisible ou de forme inattendue.
    #[error("réponse JSON illisible : {0}")]
    Json(#[from] serde_json::Error),

    /// Document YAML illisible ou impossible à sérialiser.
    #[error("document YAML illisible : {0}")]
    Yaml(String),

    /// Erreur remontée par `kubewatch-core` (convertie en texte pour éviter un couplage fort).
    #[error("erreur du noyau Kubernetes : {0}")]
    Core(String),

    /// Toute autre erreur, avec un message explicite destiné à l'utilisateur.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Code HTTP à renvoyer au client pour cette erreur.
    pub fn status_code(&self) -> u16 {
        match self {
            // Un échec côté registre distant est une panne de passerelle, sauf si le
            // registre nous a lui-même renvoyé un statut exploitable.
            Error::Http(e) => e.status().map(|s| s.as_u16()).unwrap_or(502),
            Error::NotFound(_) => 404,
            Error::Invalid(_) | Error::Yaml(_) => 400,
            Error::RateLimited(_) => 429,
            Error::Auth(_) => 401,
            Error::Unsupported(_) => 501,
            Error::Json(_) => 502,
            Error::Core(_) | Error::Other(_) => 500,
        }
    }

    /// Identifiant stable de la famille d'erreur, exposé tel quel dans l'API HTTP.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Error::Http(_) => "http",
            Error::NotFound(_) => "notFound",
            Error::Invalid(_) => "invalid",
            Error::RateLimited(_) => "rateLimited",
            Error::Auth(_) => "auth",
            Error::Unsupported(_) => "unsupported",
            Error::Json(_) => "json",
            Error::Yaml(_) => "yaml",
            Error::Core(_) => "core",
            Error::Other(_) => "other",
        }
    }

    /// Vrai si une nouvelle tentative ultérieure a des chances d'aboutir.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::RateLimited(_) => true,
            Error::Http(e) => e.is_timeout() || e.is_connect(),
            _ => false,
        }
    }

    /// Raccourci pour construire une erreur générique à partir de n'importe quel affichage.
    pub fn other(msg: impl std::fmt::Display) -> Self {
        Error::Other(msg.to_string())
    }

    /// Raccourci pour construire une erreur d'entrée invalide.
    pub fn invalid(msg: impl std::fmt::Display) -> Self {
        Error::Invalid(msg.to_string())
    }
}

impl From<serde_yaml_ng::Error> for Error {
    fn from(e: serde_yaml_ng::Error) -> Self {
        Error::Yaml(e.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Other(format!("erreur d'entrée/sortie : {e}"))
    }
}

/// Alias de résultat utilisé partout dans le crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_http_attendus() {
        assert_eq!(Error::NotFound("x".into()).status_code(), 404);
        assert_eq!(Error::Invalid("x".into()).status_code(), 400);
        assert_eq!(Error::RateLimited(Some(30)).status_code(), 429);
        assert_eq!(Error::Auth("x".into()).status_code(), 401);
        assert_eq!(Error::Unsupported("x".into()).status_code(), 501);
        assert_eq!(Error::Core("x".into()).status_code(), 500);
        assert_eq!(Error::Yaml("x".into()).status_code(), 400);
    }

    #[test]
    fn identifiants_de_famille_stables() {
        assert_eq!(Error::NotFound("x".into()).kind_str(), "notFound");
        assert_eq!(Error::RateLimited(None).kind_str(), "rateLimited");
        assert_eq!(Error::Other("x".into()).kind_str(), "other");
    }

    #[test]
    fn message_de_quota_mentionne_le_delai() {
        let msg = Error::RateLimited(Some(42)).to_string();
        assert!(msg.contains("42"), "message inattendu : {msg}");
    }
}
