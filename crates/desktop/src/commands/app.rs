//! Informations générales et actions système.

use serde::Serialize;
use tauri::State;

use crate::render::{self, Acceleration, RenderView, TextRendering};
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

/// Réglage du rendu de la fenêtre, tel qu'il est et tel qu'il sera.
#[tauri::command]
pub fn render_settings(state: State<'_, AppState>) -> RenderView {
    render::view(&state.state_dir)
}

/// Choisit l'accélération matérielle ; elle s'applique au démarrage suivant.
#[tauri::command]
pub fn render_set_acceleration(
    state: State<'_, AppState>,
    acceleration: Acceleration,
) -> Result<RenderView, String> {
    render::set_acceleration(&state.state_dir, acceleration)
}

/// Choisit la rastérisation du texte ; elle s'applique au démarrage suivant.
///
/// Le processus web garde les polices qu'il a construites : changer
/// `GtkSettings` en cours de route ne referait pas ce qui est déjà à l'écran,
/// pas même après un rechargement de la page.
#[tauri::command]
pub fn render_set_text(
    state: State<'_, AppState>,
    text: TextRendering,
) -> Result<RenderView, String> {
    render::set_text_rendering(&state.state_dir, text)
}

/// Relance l'application, pour appliquer un changement de rendu.
#[tauri::command]
pub fn app_restart(app: tauri::AppHandle) {
    tracing::info!("redémarrage demandé depuis les réglages");
    app.restart()
}
