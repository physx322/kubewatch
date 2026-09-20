//! Informations générales et actions système.

use serde::Serialize;
use tauri::State;

use crate::state::AppState;

/// Ce que l'interface affiche dans « À propos » et au démarrage.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// Version du binaire.
    pub version: String,
    /// Dossier d'état.
    pub state_dir: String,
    /// Avertissements du démarrage (consommés : un seul affichage).
    pub warnings: Vec<String>,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    let warnings = std::mem::take(&mut *state.startup_warnings.lock());
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        state_dir: state.state_dir.display().to_string(),
        warnings,
    }
}

/// Ouvre une adresse dans le navigateur de l'utilisateur.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    let u = url.trim();
    if !(u.starts_with("https://") || u.starts_with("http://")) {
        return Err(format!("adresse refusée : « {u} »"));
    }
    open::that_detached(u).map_err(|e| format!("ouverture de « {u} » impossible : {e}"))
}
