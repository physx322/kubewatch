//! Commandes exposées à l'interface web (`invoke("nom", { … })`).
//!
//! Conventions :
//! * chaque commande renvoie `Result<T, String>` ; l'erreur est un message
//!   français prêt à afficher ;
//! * les flux (journaux, sessions interactives, réponse de l'assistant) passent
//!   par un `Channel` Tauri fourni par l'interface ;
//! * `cluster` vide désigne le cluster courant.

pub mod ai;
pub mod app;
pub mod clusters;
pub mod hub;
pub mod resources;
pub mod streams;
pub mod updater;

/// Convertit une erreur quelconque en message d'interface.
pub(crate) fn fail<E: std::fmt::Display>(prefix: &str, e: E) -> String {
    if prefix.is_empty() {
        e.to_string()
    } else {
        format!("{prefix} : {e}")
    }
}
