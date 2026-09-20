//! Surveillance des mises à jour d'images et de releases.

use serde::Serialize;
use tauri::State;

use kubewatch_updater::model::{RolloutResult, UpdateFinding, WatcherSpec};
use kubewatch_updater::scan::{self, WorkloadImage};
use kubewatch_updater::store::UpdaterSettings;

use super::fail;
use crate::state::AppState;

const HISTORY_LIMIT: usize = 200;

/// Surveillants et détections en attente.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchersBundle {
    /// Surveillants configurés.
    pub watchers: Vec<WatcherSpec>,
    /// Détections en attente.
    pub findings: Vec<UpdateFinding>,
}

fn bundle(state: &AppState) -> WatchersBundle {
    WatchersBundle {
        watchers: state.store.watchers(),
        findings: state.store.findings(),
    }
}

#[tauri::command]
pub fn updates_list(state: State<'_, AppState>) -> WatchersBundle {
    bundle(&state)
}

#[tauri::command]
pub fn updates_upsert_watcher(
    state: State<'_, AppState>,
    watcher: WatcherSpec,
) -> Result<WatchersBundle, String> {
    state
        .store
        .upsert_watcher(watcher)
        .map_err(|e| fail("enregistrement impossible", e))?;
    Ok(bundle(&state))
}

#[tauri::command]
pub fn updates_remove_watcher(
    state: State<'_, AppState>,
    watcher_id: String,
) -> Result<WatchersBundle, String> {
    state
        .store
        .remove_watcher(&watcher_id)
        .map_err(|e| fail("suppression impossible", e))?;
    Ok(bundle(&state))
}

#[tauri::command]
pub async fn updates_check(state: State<'_, AppState>) -> Result<Vec<UpdateFinding>, String> {
    let engine = state.engine()?;
    engine
        .check_all()
        .await
        .map_err(|e| fail("vérification des mises à jour impossible", e))
}

#[tauri::command]
pub async fn updates_apply(
    state: State<'_, AppState>,
    finding_id: String,
) -> Result<RolloutResult, String> {
    let engine = state.engine()?;
    engine
        .apply(&finding_id)
        .await
        .map_err(|e| fail("application de la mise à jour impossible", e))
}

#[tauri::command]
pub fn updates_history(state: State<'_, AppState>) -> Vec<RolloutResult> {
    state.store.history(HISTORY_LIMIT)
}

#[tauri::command]
pub async fn updates_scan(
    state: State<'_, AppState>,
    cluster: String,
    namespace: Option<String>,
) -> Result<Vec<WorkloadImage>, String> {
    let h = state.handle(&cluster)?;
    let ns = namespace
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    scan::scan_workloads(&h, ns)
        .await
        .map_err(|e| fail("inventaire des images impossible", e))
}

#[tauri::command]
pub async fn updates_suggest(
    state: State<'_, AppState>,
    cluster: String,
    namespace: Option<String>,
) -> Result<Vec<WatcherSpec>, String> {
    let h = state.handle(&cluster)?;
    let ns = namespace
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let images = scan::scan_workloads(&h, ns)
        .await
        .map_err(|e| fail("suggestion de surveillants impossible", e))?;
    let policy = state.store.settings().default_policy;
    Ok(scan::suggest_watchers(&images, &policy))
}

#[tauri::command]
pub fn updates_settings(state: State<'_, AppState>) -> UpdaterSettings {
    // Jamais les secrets en clair vers l'interface.
    state.store.settings_redacted()
}

#[tauri::command]
pub fn updates_save_settings(
    state: State<'_, AppState>,
    settings: UpdaterSettings,
) -> Result<UpdaterSettings, String> {
    state
        .store
        .set_settings(settings)
        .map_err(|e| fail("enregistrement des réglages impossible", e))?;
    Ok(state.store.settings_redacted())
}
