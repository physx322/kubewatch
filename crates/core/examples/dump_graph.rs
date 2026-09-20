//! Écrit sur la sortie standard l'instantané d'objets tel que l'écran
//! « Topologie » le reçoit (`{ "objects": [...], "warnings": [...] }`), à partir
//! des clusters enregistrés dans le dossier d'état.
//!
//!     cargo run -p kubewatch-core --example dump_graph -- [cluster] [namespace]
//!
//! `KUBEWATCH_STATE_DIR` désigne le dossier d'état ; à défaut, celui de
//! l'installation courante. Sert à rejouer le graphe hors de l'application
//! (tests, diagnostic d'un rendu).
use std::path::PathBuf;

use kubewatch_core::model::{ListOptions, ObjectSummary};
use kubewatch_core::{resource, ClusterManager};

const GRAPH_KINDS: [&str; 11] = [
    "pods",
    "replicasets",
    "deployments",
    "statefulsets",
    "daemonsets",
    "jobs",
    "cronjobs",
    "services",
    "ingresses",
    "persistentvolumeclaims",
    "nodes",
];

fn state_dir() -> PathBuf {
    if let Ok(d) = std::env::var("KUBEWATCH_STATE_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    dirs::data_dir()
        .map(|d| d.join("kubewatch"))
        .unwrap_or_else(|| PathBuf::from(".kubewatch"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let mut args = std::env::args().skip(1);
    let cluster = args.next().filter(|s| !s.is_empty());
    let namespace = args.next().filter(|s| !s.is_empty());

    let clusters = ClusterManager::new(state_dir());
    clusters.load_persisted().await?;
    let h = clusters.get_or_current(cluster.as_deref())?;

    let mut objects: Vec<ObjectSummary> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for plural in GRAPH_KINDS {
        let kind = match h.resolve_kind(plural) {
            Ok(k) => k,
            Err(e) => {
                warnings.push(format!("{plural} : {e}"));
                continue;
            }
        };
        let mut token: Option<String> = None;
        loop {
            let opts = ListOptions {
                namespace: namespace.clone(),
                limit: Some(1_000),
                continue_token: token.take(),
                ..Default::default()
            };
            match resource::list(&h, &kind, &opts).await {
                Ok(page) => {
                    objects.extend(page.items);
                    token = page.continue_token;
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    warnings.push(format!("{plural} : {e}"));
                    break;
                }
            }
        }
    }
    eprintln!(
        "cluster « {} » : {} objets, {} avertissement(s)",
        h.name,
        objects.len(),
        warnings.len()
    );
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({ "objects": objects, "warnings": warnings })
        )?
    );
    Ok(())
}
