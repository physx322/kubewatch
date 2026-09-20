//! Parc de clusters : connexion, import, sélection.

use std::path::{Path, PathBuf};

use tauri::State;

use kubewatch_core::client;
use kubewatch_core::model::{ClusterInfo, ContextInfo};
use kubewatch_core::ConnectionSpec;

use super::fail;
use crate::state::AppState;

#[tauri::command]
pub async fn list_clusters(state: State<'_, AppState>) -> Result<Vec<ClusterInfo>, String> {
    Ok(state.clusters.list_info().await)
}

#[tauri::command]
pub async fn connect_cluster(
    state: State<'_, AppState>,
    name: String,
    spec: ConnectionSpec,
    persist: bool,
) -> Result<ClusterInfo, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("le nom du cluster est vide".to_string());
    }
    let h = state
        .clusters
        .add(&name, spec, persist)
        .await
        .map_err(|e| fail(&format!("connexion à « {name} » impossible"), e))?;
    Ok(h.info().await)
}

#[tauri::command]
pub async fn import_kubeconfig(
    state: State<'_, AppState>,
    path: Option<String>,
    all_contexts: bool,
) -> Result<Vec<String>, String> {
    let path = path
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    state
        .clusters
        .import_kubeconfig(path.as_deref(), all_contexts)
        .await
        .map_err(|e| fail("import du kubeconfig impossible", e))
}

#[tauri::command]
pub fn list_contexts(path: Option<String>) -> Result<Vec<ContextInfo>, String> {
    let path = path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    client::list_contexts(path.as_deref().map(Path::new))
        .map_err(|e| fail("lecture du kubeconfig impossible", e))
}

#[tauri::command]
pub fn default_kubeconfig_path() -> Option<String> {
    client::default_kubeconfig_path().map(|p| p.display().to_string())
}

#[tauri::command]
pub fn remove_cluster(state: State<'_, AppState>, name: String) -> Result<(), String> {
    state.abort_all_for_cluster();
    state
        .clusters
        .remove(&name)
        .map_err(|e| fail(&format!("suppression du cluster « {name} » impossible"), e))
}

#[tauri::command]
pub fn select_cluster(state: State<'_, AppState>, name: String) -> Result<(), String> {
    state
        .clusters
        .set_current(&name)
        .map_err(|e| fail(&format!("sélection du cluster « {name} » impossible"), e))
}

#[tauri::command]
pub fn current_cluster(state: State<'_, AppState>) -> Option<String> {
    state.clusters.current_name()
}

#[tauri::command]
pub async fn refresh_cluster_catalog(
    state: State<'_, AppState>,
    name: String,
) -> Result<(), String> {
    state
        .clusters
        .refresh_catalog(&name)
        .await
        .map_err(|e| fail("rafraîchissement du catalogue impossible", e))
}

/// Sélecteur de fichier natif (portail XDG sous Linux).
#[tauri::command]
pub async fn pick_file(title: Option<String>) -> Result<Option<String>, String> {
    let mut dialog = rfd::AsyncFileDialog::new();
    if let Some(t) = title {
        dialog = dialog.set_title(t);
    }
    if let Some(home) = dirs::home_dir() {
        dialog = dialog.set_directory(home.join(".kube"));
    }
    Ok(dialog
        .pick_file()
        .await
        .map(|f| f.path().display().to_string()))
}

impl AppState {
    /// Un cluster retiré ne doit laisser aucun flux ouvert derrière lui.
    fn abort_all_for_cluster(&self) {
        // Les flux ne mémorisent pas leur cluster : par prudence, tout est fermé.
        let ids: Vec<u64> = self.streams.lock().keys().copied().collect();
        for id in ids {
            self.abort_stream(id);
        }
    }
}
