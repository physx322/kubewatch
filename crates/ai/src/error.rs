//! Erreurs du crate, toutes porteuses d'un message français lisible.

/// Erreur de l'assistant.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Profil incomplet ou incohérent (clé absente, modèle vide…).
    #[error("configuration : {0}")]
    Config(String),
    /// Le fournisseur est injoignable ou la connexion a échoué.
    #[error("réseau : {0}")]
    Http(#[from] reqwest::Error),
    /// Le fournisseur a répondu par une erreur HTTP.
    #[error("le fournisseur a répondu {status} : {message}")]
    Api {
        /// Code HTTP (0 si l'erreur est arrivée dans le flux, après le 200).
        status: u16,
        /// Message renvoyé par le fournisseur, ou début du corps de réponse.
        message: String,
    },
    /// Le flux ne respecte pas le format attendu.
    #[error("réponse inattendue du fournisseur : {0}")]
    Protocol(String),
    /// Lecture ou écriture du fichier de réglages.
    #[error("entrée/sortie : {0}")]
    Io(#[from] std::io::Error),
    /// Fichier de réglages illisible.
    #[error("réglages illisibles : {0}")]
    Json(#[from] serde_json::Error),
}

/// Résultat du crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Vrai si l'erreur vient du fournisseur qui a explicitement refusé
    /// l'authentification : la clé est absente ou invalide.
    pub fn is_auth(&self) -> bool {
        matches!(
            self,
            Error::Api {
                status: 401 | 403,
                ..
            }
        )
    }
}
