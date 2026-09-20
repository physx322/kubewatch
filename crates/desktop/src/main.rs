//! KubeWatch — application de bureau pour piloter des clusters Kubernetes :
//! cœur en Rust, interface web (React) embarquée depuis `ui/`, assistant IA
//! relié au cluster.
//!
//! Ce binaire est la moitié Rust de l'application : il expose le cœur
//! (`kubewatch-core`, `-hub`, `-updater`, `-ai`) à l'interface sous forme de
//! commandes Tauri, et fait circuler les flux (journaux, sessions
//! interactives, réponse de l'assistant) par des canaux.
#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assistant;
mod commands;
mod state;
mod util;

use tauri::{Emitter, Manager};
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

fn main() {
    util::install_crypto_provider();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,kube=warn,hyper=warn,tower=warn,h2=warn")),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let state = AppState::new(util::state_dir());
            tracing::info!(dossier = %state.state_dir.display(), "dossier d'état");
            let clusters = state.clusters.clone();
            app.manage(state);

            // Reconnexion aux clusters enregistrés : en tâche de fond, un cluster
            // injoignable met jusqu'à 30 s à répondre et la fenêtre doit s'afficher
            // tout de suite.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = clusters.load_persisted().await {
                    tracing::warn!(erreur = %e, "clusters enregistrés partiellement illisibles");
                    let _ = handle.emit(
                        "startup-warning",
                        format!("clusters enregistrés partiellement illisibles : {e}"),
                    );
                }
                let _ = handle.emit("clusters-changed", ());
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if let Some(state) = window.try_state::<AppState>() {
                    state.abort_all();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::app_info,
            commands::app::open_url,
            commands::clusters::list_clusters,
            commands::clusters::connect_cluster,
            commands::clusters::import_kubeconfig,
            commands::clusters::list_contexts,
            commands::clusters::default_kubeconfig_path,
            commands::clusters::remove_cluster,
            commands::clusters::select_cluster,
            commands::clusters::current_cluster,
            commands::clusters::refresh_cluster_catalog,
            commands::clusters::pick_file,
            commands::resources::cluster_overview,
            commands::resources::list_kinds,
            commands::resources::list_namespaces,
            commands::resources::list_resources,
            commands::resources::load_graph,
            commands::resources::get_yaml,
            commands::resources::apply_yaml,
            commands::resources::diff_yaml,
            commands::resources::delete_yaml,
            commands::resources::replace_yaml,
            commands::resources::delete_resource,
            commands::resources::scale_resource,
            commands::resources::restart_resource,
            commands::resources::set_image,
            commands::resources::rollback_resource,
            commands::resources::cordon_node,
            commands::resources::drain_node,
            commands::resources::load_events,
            commands::resources::load_containers,
            commands::resources::load_metrics,
            commands::streams::start_logs,
            commands::streams::stop_logs,
            commands::streams::start_exec,
            commands::streams::exec_input,
            commands::streams::exec_resize,
            commands::streams::stop_exec,
            commands::hub::hub_search_images,
            commands::hub::hub_list_tags,
            commands::hub::hub_inspect,
            commands::hub::hub_search_charts,
            commands::hub::hub_catalog,
            commands::hub::hub_render,
            commands::hub::hub_deploy,
            commands::updater::updates_list,
            commands::updater::updates_upsert_watcher,
            commands::updater::updates_remove_watcher,
            commands::updater::updates_check,
            commands::updater::updates_apply,
            commands::updater::updates_history,
            commands::updater::updates_scan,
            commands::updater::updates_suggest,
            commands::updater::updates_settings,
            commands::updater::updates_save_settings,
            commands::ai::ai_settings,
            commands::ai::ai_upsert_profile,
            commands::ai::ai_remove_profile,
            commands::ai::ai_set_active,
            commands::ai::ai_set_general,
            commands::ai::ai_list_models,
            commands::ai::ai_chat,
            commands::ai::ai_cancel,
        ])
        .run(tauri::generate_context!())
        .expect("KubeWatch n'a pas pu démarrer");
}
