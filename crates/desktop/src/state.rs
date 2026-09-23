//! État partagé de l'application, géré par Tauri et injecté dans les commandes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use tokio::task::AbortHandle;

use kubewatch_ai::AiStore;
use kubewatch_core::exec::ExecControl;
use kubewatch_core::{ClusterHandle, ClusterManager};
use kubewatch_hub::HubClient;
use kubewatch_updater::store::Store;
use kubewatch_updater::UpdateEngine;

/// Nom du fichier d'état de l'updater dans le dossier d'état (inchangé depuis
/// les versions précédentes : une installation existante est reprise telle
/// quelle).
const UPDATER_STATE_FILE: &str = "updater.json";

/// Flux ouvert (journaux ou session interactive), interruptible.
pub struct StreamEntry {
    /// Tâche qui pompe le flux vers l'interface.
    pub task: AbortHandle,
    /// Entrée standard de la session interactive, le cas échéant.
    pub stdin: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    /// Pilotage de la session interactive (taille, interruption).
    pub control: Option<ExecControl>,
}

/// Services et flux de l'application.
pub struct AppState {
    /// Dossier d'état.
    pub state_dir: PathBuf,
    /// Parc de clusters.
    pub clusters: ClusterManager,
    /// État persistant du moteur de mise à jour.
    pub store: Store,
    /// Client des registres d'images ; `None` si le client HTTP n'a pas pu être construit.
    pub hub: Option<HubClient>,
    /// Moteur de mise à jour ; `None` pour la même raison.
    pub engine: Option<UpdateEngine>,
    /// Réglages et profils de l'assistant.
    pub ai: AiStore,
    /// Flux ouverts, par identifiant.
    pub streams: Mutex<HashMap<u64, StreamEntry>>,
    /// Conversations en cours avec l'assistant, par identifiant.
    pub chats: Mutex<HashMap<u64, AbortHandle>>,
    /// Avertissements du démarrage, à montrer une fois à l'utilisateur.
    pub startup_warnings: Mutex<Vec<String>>,
    next_id: AtomicU64,
}

impl AppState {
    /// Construit les services. Aucune entrée/sortie réseau ici : la reconnexion
    /// aux clusters enregistrés se fait en tâche de fond.
    pub fn new(state_dir: PathBuf) -> Self {
        let mut warnings = Vec::new();
        let clusters = ClusterManager::new(state_dir.clone());

        let store = Store::new(state_dir.join(UPDATER_STATE_FILE));
        if let Err(e) = store.load() {
            warnings.push(format!(
                "état des mises à jour illisible, on repart d'un état vide : {e}"
            ));
        }

        let hub = match HubClient::new() {
            Ok(h) => Some(h.with_github_token(store.settings().github_token)),
            Err(e) => {
                warnings.push(format!("client HTTP du hub indisponible : {e}"));
                None
            }
        };
        let engine = hub
            .clone()
            .map(|h| UpdateEngine::new(clusters.clone(), h, store.clone()));

        let ai = AiStore::open(&state_dir);
        if let Some(e) = ai.load_error() {
            warnings.push(format!(
                "réglages de l'assistant illisibles, on repart d'un état vide : {e}"
            ));
        }
        // Premier démarrage : si Claude Code est connecté sur cette machine, on
        // reprend son compte pour que l'assistant marche sans ressaisir de clé.
        if let Some(p) = ai.seed_from_claude_code() {
            tracing::info!(profil = %p.name, "assistant configuré depuis ~/.claude");
        }

        Self {
            state_dir,
            clusters,
            store,
            hub,
            engine,
            ai,
            streams: Mutex::new(HashMap::new()),
            chats: Mutex::new(HashMap::new()),
            startup_warnings: Mutex::new(warnings),
            next_id: AtomicU64::new(1),
        }
    }

    /// Identifiant unique pour un flux.
    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Cluster nommé, ou cluster courant si le nom est vide.
    pub fn handle(&self, name: &str) -> Result<ClusterHandle, String> {
        let wanted = name.trim();
        let found = if wanted.is_empty() {
            self.clusters.get_or_current(None)
        } else {
            self.clusters.get(wanted)
        };
        found.map_err(|e| format!("cluster indisponible : {e}"))
    }

    /// Client du hub, ou explication de son absence.
    pub fn hub(&self) -> Result<HubClient, String> {
        self.hub.clone().ok_or_else(|| {
            "le client HTTP n'a pas pu être initialisé : les registres d'images sont \
             inaccessibles pour cette session"
                .to_string()
        })
    }

    /// Moteur de mise à jour, ou explication de son absence.
    pub fn engine(&self) -> Result<UpdateEngine, String> {
        self.engine.clone().ok_or_else(|| {
            "le moteur de mise à jour est indisponible : le client HTTP n'a pas pu être \
             initialisé"
                .to_string()
        })
    }

    /// Interrompt un flux ; vrai s'il existait.
    pub fn abort_stream(&self, id: u64) -> bool {
        match self.streams.lock().remove(&id) {
            Some(entry) => {
                entry.task.abort();
                if let Some(c) = entry.control {
                    c.abort();
                }
                true
            }
            None => false,
        }
    }

    /// Interrompt tous les flux (fermeture de l'application).
    pub fn abort_all(&self) {
        let ids: Vec<u64> = self.streams.lock().keys().copied().collect();
        for id in ids {
            self.abort_stream(id);
        }
        for (_, h) in self.chats.lock().drain() {
            h.abort();
        }
    }
}
