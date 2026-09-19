//! Le pont asynchrone entre l'interface egui et le cœur Kubernetes.
//!
//! # Principe
//!
//! egui est synchrone et *immediate-mode* : le thread d'interface doit rendre une
//! image toutes les quelques millisecondes et ne peut donc **jamais** attendre le
//! réseau. `kube`, `reqwest` et le moteur de mise à jour sont asynchrones.
//!
//! [`Backend`] réunit les deux :
//!
//! * une file **non bornée** de [`Command`] va de l'interface vers le worker
//!   ([`Backend::send`] n'attend jamais) ;
//! * une file `std::sync::mpsc` de [`Event`] revient du worker vers l'interface
//!   ([`Backend::drain`] la vide sans jamais bloquer) ;
//! * le worker appelle `egui::Context::request_repaint()` après chaque envoi pour
//!   réveiller l'interface, qui peut donc rester endormie le reste du temps.
//!
//! Le [`tokio::runtime::Runtime`] est conservé dans [`Backend`] : s'il était
//! abandonné, toutes les tâches en cours mourraient avec lui.
//!
//! # Fournisseur cryptographique rustls — conclusion vérifiée
//!
//! Le binaire contient deux fournisseurs rustls : `ring` (activé par `kube`, via
//! `kube/ring` dans le `Cargo.toml` racine) et `aws-lc-rs` (activé par `reqwest`).
//! Lorsque les deux sont présents, `rustls::ClientConfig::builder()` ne peut pas
//! choisir seul et **panique** ; il faut donc installer explicitement un
//! fournisseur par défaut au démarrage du processus.
//!
//! Vérification faite dans les sources de `kube-client` 4.2.0 installées ici :
//!
//! * `src/client/builder.rs` ligne 120 : l'installation automatique
//!   (`rustls::crypto::aws_lc_rs::default_provider().install_default()`) est
//!   gardée par `#[cfg(all(feature = "aws-lc-rs", feature = "rustls-tls"))]` ;
//! * or ce dépôt active `ring`, **pas** `aws-lc-rs` (`kube = { …, features = [
//!   "rustls-tls", "ring", … ] }`) ;
//! * les seules autres occurrences (`src/client/tls.rs` lignes 257 et 290) sont
//!   dans des modules `#[cfg(test)]`.
//!
//! **Conclusion : `kube` n'installe PAS le fournisseur dans cette configuration.**
//! C'est `app.rs::install_crypto_provider()` qui appelle
//! `rustls::crypto::ring::default_provider().install_default()` au démarrage,
//! avant toute connexion. Par sécurité, chaque commande est en outre exécutée
//! sous [`run_guarded`], qui convertit une panique en [`Event::Failed`] lisible
//! plutôt que de laisser l'application muette : si la panique mentionne le
//! fournisseur, le message indique la correction à apporter.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use futures::StreamExt;
use parking_lot::Mutex;

use kubewatch_core::apply::{self, ApplyOptions};
use kubewatch_core::exec::ExecControl;
use kubewatch_core::model::{ListOptions, ObjectSummary, ResourceKind, ResourceRef};
use kubewatch_core::{client, events, exec, logs, metrics, resource};
use kubewatch_core::{ClusterHandle, ClusterManager};
use kubewatch_hub::model::ImageRef;
use kubewatch_hub::{catalog, deploy, HubClient};
use kubewatch_updater::store::Store;
use kubewatch_updater::{scan, UpdateEngine};

use super::command::{Command, RequestId, NO_REQUEST};
use super::event::Event;

/// Nom du fichier d'état de l'updater dans le dossier d'état. Inchangé depuis
/// les versions précédentes : une installation existante est reprise telle quelle.
const UPDATER_STATE_FILE: &str = "updater.json";

/// Cadence maximale de réveil de l'interface : environ 30 images par seconde.
///
/// Un pod bavard peut produire des dizaines de milliers de lignes par seconde ;
/// demander un rendu à chaque ligne effondrerait l'interface.
pub(crate) const UI_FRAME: Duration = Duration::from_millis(33);

/// Fenêtre d'observation du débit des journaux.
pub(crate) const LOG_WINDOW: Duration = Duration::from_secs(1);

/// Au-delà de ce nombre de lignes par [`LOG_WINDOW`], les lignes sont regroupées.
pub(crate) const LOG_BURST_LINES: usize = 2_000;

/// Plafond de lignes accumulées avant vidage forcé (borne la mémoire du worker).
pub(crate) const MAX_PENDING_LINES: usize = 10_000;

/// Nombre maximal d'évènements dépilés par image, pour ne jamais retarder le rendu.
const MAX_EVENTS_PER_FRAME: usize = 500;

/// Nombre d'évènements Kubernetes récupérés par `LoadEvents`.
const EVENTS_LIMIT: u32 = 300;

/// Types lus par `LoadGraph`, par nom pluriel.
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

/// Taille de page demandée pour chaque type de la topologie.
const GRAPH_PAGE: u32 = 1_000;

/// Plafond d'objets conservés par type : au-delà, le graphe serait illisible.
const GRAPH_MAX_PER_KIND: usize = 5_000;

/// Nombre de résultats demandés aux API du hub.
const HUB_LIMIT: usize = 60;

/// Nombre de tags demandés pour une image.
const HUB_TAGS_LIMIT: usize = 200;

/// Profondeur d'historique renvoyée par `UpdatesHistory`.
const HISTORY_LIMIT: usize = 200;

// ---------------------------------------------------------------------------
// Le pont, vu de l'interface
// ---------------------------------------------------------------------------

/// Pont asynchrone détenu par l'application.
///
/// Toutes ses méthodes sont non bloquantes : elles sont appelées depuis le thread
/// d'interface, une fois par image.
pub struct Backend {
    /// File des commandes vers le worker.
    tx: tokio::sync::mpsc::UnboundedSender<Command>,
    /// File des évènements en retour.
    rx: std::sync::mpsc::Receiver<Event>,
    /// Contexte egui, utilisé pour redemander un rendu quand la file déborde.
    ctx: egui::Context,
    /// Runtime tokio maintenu en vie pour la durée de l'application.
    ///
    /// Enveloppé dans une `Option` uniquement pour pouvoir l'arrêter avec un délai
    /// borné dans [`Drop`] : abandonner un runtime peut sinon attendre indéfiniment
    /// qu'une tâche bloquante se termine, et l'application semblerait figée à la
    /// fermeture.
    rt: Option<tokio::runtime::Runtime>,
}

impl Backend {
    /// Démarre le runtime et le worker.
    ///
    /// Aucune opération réseau n'est effectuée ici : la reconnexion aux clusters
    /// persistés part en tâche de fond et l'interface s'affiche immédiatement, même
    /// si tous les clusters enregistrés sont injoignables.
    pub fn new(ctx: egui::Context, state_dir: PathBuf) -> anyhow::Result<Self> {
        if let Err(e) = std::fs::create_dir_all(&state_dir) {
            tracing::warn!(
                dossier = %state_dir.display(),
                erreur = %e,
                "dossier d'état non créé, l'état ne sera pas conservé"
            );
        }

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("kubewatch-worker")
            .build()
            .context("démarrage du runtime asynchrone impossible")?;

        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Command>();
        let (ev_tx, ev_rx) = std::sync::mpsc::channel::<Event>();
        let sink = EventSink::new(ev_tx, ctx.clone());

        rt.spawn(worker_loop(cmd_rx, sink, state_dir));

        Ok(Self {
            tx: cmd_tx,
            rx: ev_rx,
            ctx,
            rt: Some(rt),
        })
    }

    /// Envoie une commande au worker. Ne bloque jamais.
    pub fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_err() {
            tracing::error!("worker asynchrone arrêté : commande ignorée");
        }
    }

    /// Vide la file des évènements, une fois par image.
    ///
    /// Le nombre d'évènements traités est plafonné : sur un flux de journaux très
    /// bavard, mieux vaut rendre une image et reprendre ensuite que retarder
    /// l'affichage. Si la file n'est pas vide au retour, un nouveau rendu est
    /// demandé pour que le reste soit traité à l'image suivante.
    pub fn drain(&self) -> Vec<Event> {
        let mut out = Vec::new();
        let mut saturated = true;
        for _ in 0..MAX_EVENTS_PER_FRAME {
            match self.rx.try_recv() {
                Ok(ev) => out.push(ev),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    saturated = false;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    saturated = false;
                    break;
                }
            }
        }
        if saturated {
            self.ctx.request_repaint();
        }
        out
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        // Demander l'arrêt propre : le worker abandonne toutes ses tâches.
        let _ = self.tx.send(Command::Quit);
        if let Some(rt) = self.rt.take() {
            // Délai borné : la fermeture de la fenêtre ne doit jamais paraître figée.
            rt.shutdown_timeout(Duration::from_millis(300));
        }
    }
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backend")
            .field("runtime_actif", &self.rt.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Émission des évènements
// ---------------------------------------------------------------------------

/// Canal d'émission vers l'interface, clonable et partageable entre tâches.
///
/// Le `Sender` est protégé par un `Mutex` non contendu : cela rend le puits
/// `Sync`, donc utilisable tel quel dans n'importe quelle tâche tokio, sans avoir
/// à raisonner sur la `Send`-ité des futures à chaque point d'attente.
#[derive(Clone)]
pub(crate) struct EventSink {
    tx: Arc<Mutex<std::sync::mpsc::Sender<Event>>>,
    ctx: egui::Context,
}

impl EventSink {
    fn new(tx: std::sync::mpsc::Sender<Event>, ctx: egui::Context) -> Self {
        Self {
            tx: Arc::new(Mutex::new(tx)),
            ctx,
        }
    }

    /// Envoie un évènement et réveille l'interface.
    pub(crate) fn send(&self, ev: Event) {
        if self.send_quiet(ev) {
            self.ctx.request_repaint();
        }
    }

    /// Envoie un évènement **sans** réveiller l'interface.
    ///
    /// Réservé aux flux (journaux, sortie de session) dont le réveil est cadencé
    /// par [`RepaintPacer`]. Renvoie `false` si l'interface a disparu.
    pub(crate) fn send_quiet(&self, ev: Event) -> bool {
        self.tx.lock().send(ev).is_ok()
    }

    /// Réveille l'interface sans rien envoyer.
    pub(crate) fn repaint(&self) {
        self.ctx.request_repaint();
    }

    /// Signale un échec avec un message français.
    pub(crate) fn fail(&self, id: RequestId, message: String) {
        self.send(Event::Failed { id, message });
    }
}

// ---------------------------------------------------------------------------
// Cadencement de l'interface et regroupement des journaux
// ---------------------------------------------------------------------------

/// Limite le nombre de réveils de l'interface à un par [`UI_FRAME`].
#[derive(Debug, Default)]
pub(crate) struct RepaintPacer {
    last: Option<Instant>,
}

impl RepaintPacer {
    /// Nouveau cadenceur, prêt à autoriser un premier réveil immédiat.
    pub(crate) fn new() -> Self {
        Self { last: None }
    }

    /// Vrai s'il faut réveiller l'interface maintenant.
    pub(crate) fn due(&mut self, now: Instant) -> bool {
        match self.last {
            Some(previous) if now.duration_since(previous) < UI_FRAME => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

/// Regroupe les lignes de journal lorsqu'elles arrivent plus vite que l'œil.
///
/// # Mécanisme
///
/// Le débit est mesuré sur une fenêtre glissante de [`LOG_WINDOW`]. Tant qu'il
/// reste sous [`LOG_BURST_LINES`] lignes par fenêtre, chaque ligne part
/// immédiatement dans son propre [`Event::LogLine`] : c'est le cas nominal, et
/// l'affichage est instantané.
///
/// Dès que le compteur de la fenêtre courante dépasse le seuil, le regroupement
/// s'enclenche **sans attendre la fin de la fenêtre** : les lignes s'accumulent et
/// ne sont émises qu'une fois par [`UI_FRAME`], réunies dans un seul évènement,
/// séparées par `\n`. Un pod qui crache 50 000 lignes par seconde produit donc au
/// plus 30 évènements par seconde au lieu de 50 000, et l'interface tient la
/// cadence. L'accumulation est bornée par [`MAX_PENDING_LINES`] : au-delà, le lot
/// est vidé immédiatement, ce qui empêche le worker de gonfler en mémoire face à
/// un flux plus rapide que la boucle de rendu.
///
/// L'ordre des lignes est toujours préservé : tant qu'un lot est en attente, les
/// nouvelles lignes le rejoignent plutôt que de le doubler.
#[derive(Debug)]
pub(crate) struct LogBatcher {
    /// Début de la fenêtre de mesure courante.
    window_start: Instant,
    /// Lignes vues depuis le début de la fenêtre.
    window_count: usize,
    /// Vrai si le regroupement est actif.
    batching: bool,
    /// Lignes en attente d'émission.
    pending: Vec<String>,
    /// Dernière émission.
    last_flush: Instant,
}

impl LogBatcher {
    /// Nouveau regroupeur, fenêtre ouverte à `now`.
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            window_count: 0,
            batching: false,
            pending: Vec::new(),
            last_flush: now,
        }
    }

    /// Vrai si le regroupement est actif.
    pub(crate) fn is_batching(&self) -> bool {
        self.batching
    }

    /// Vrai si des lignes attendent d'être émises.
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Enregistre une ligne et renvoie le contenu à émettre, s'il y a lieu.
    pub(crate) fn push(&mut self, line: String, now: Instant) -> Option<String> {
        if now.duration_since(self.window_start) >= LOG_WINDOW {
            self.window_start = now;
            self.window_count = 0;
            self.batching = false;
        }
        self.window_count += 1;
        if self.window_count > LOG_BURST_LINES {
            self.batching = true;
        }

        // Cas nominal : débit raisonnable et rien en attente, la ligne part seule.
        if !self.batching && self.pending.is_empty() {
            self.last_flush = now;
            return Some(line);
        }

        self.pending.push(line);
        if now.duration_since(self.last_flush) >= UI_FRAME
            || self.pending.len() >= MAX_PENDING_LINES
        {
            self.last_flush = now;
            return Some(self.take_pending());
        }
        None
    }

    /// Émet le lot en attente lorsque le flux marque une pause.
    pub(crate) fn tick(&mut self, now: Instant) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        self.last_flush = now;
        Some(self.take_pending())
    }

    /// Émet tout ce qui reste, à la fermeture du flux.
    pub(crate) fn flush(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            None
        } else {
            Some(self.take_pending())
        }
    }

    fn take_pending(&mut self) -> String {
        self.pending.drain(..).collect::<Vec<_>>().join("\n")
    }
}

// ---------------------------------------------------------------------------
// Tâches longues (journaux, sessions interactives)
// ---------------------------------------------------------------------------

/// Poignées d'une session interactive, pilotables depuis la boucle du worker.
#[derive(Clone)]
struct ExecHandles {
    /// Entrée clavier ; file non bornée pour que l'interface n'attende jamais.
    stdin: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    /// Pilotage, disponible une fois la session réellement ouverte.
    control: Arc<Mutex<Option<ExecControl>>>,
    /// Taille de terminal reçue avant l'ouverture, appliquée dès que possible.
    pending_size: Arc<Mutex<Option<(u16, u16)>>>,
}

/// Tâche longue enregistrée, avec de quoi l'interrompre.
struct TaskEntry {
    handle: tokio::task::JoinHandle<()>,
    exec: Option<ExecHandles>,
}

/// Registre des tâches longues, indexé par l'identifiant de requête.
type TaskMap = Arc<Mutex<HashMap<RequestId, TaskEntry>>>;

/// Enregistre une tâche longue et purge les entrées déjà terminées.
///
/// Les tâches ne se retirent jamais elles-mêmes du registre : une tâche qui
/// échouerait instantanément pourrait sinon supprimer son entrée avant même que la
/// boucle ne l'ait insérée. La purge à l'insertion règle le problème sans verrou
/// supplémentaire, et le coût d'une poignée terminée est négligeable.
fn register_task(tasks: &TaskMap, id: RequestId, entry: TaskEntry) {
    let mut guard = tasks.lock();
    guard.retain(|_, e| !e.handle.is_finished());
    if let Some(previous) = guard.insert(id, entry) {
        abort_entry(&previous);
    }
}

/// Interrompt une tâche longue et libère ses ressources distantes.
fn abort_entry(entry: &TaskEntry) {
    if let Some(ex) = &entry.exec {
        if let Some(control) = ex.control.lock().take() {
            control.abort();
        }
    }
    entry.handle.abort();
}

/// Abandonne toutes les tâches enregistrées : aucune fuite à l'arrêt du worker.
fn abort_all(tasks: &TaskMap) {
    for (_, entry) in tasks.lock().drain() {
        abort_entry(&entry);
    }
}

// ---------------------------------------------------------------------------
// Services partagés par les commandes
// ---------------------------------------------------------------------------

/// Ressources partagées par toutes les commandes.
///
/// Tous les champs sont bon marché à cloner (état interne en `Arc`).
#[derive(Clone)]
struct Services {
    clusters: ClusterManager,
    /// `None` si le client HTTP n'a pas pu être construit ; les commandes du hub
    /// échouent alors proprement au lieu de faire tomber l'application.
    hub: Option<HubClient>,
    /// `None` pour la même raison : le moteur de mise à jour a besoin du hub.
    engine: Option<UpdateEngine>,
    store: Store,
}

/// Résout un cluster, ou signale l'échec à l'interface.
fn handle_of(svc: &Services, sink: &EventSink, id: RequestId, name: &str) -> Option<ClusterHandle> {
    let wanted = name.trim();
    let found = if wanted.is_empty() {
        svc.clusters.get_or_current(None)
    } else {
        svc.clusters.get(wanted)
    };
    match found {
        Ok(h) => Some(h),
        Err(e) => {
            sink.fail(id, format!("cluster indisponible : {e}"));
            None
        }
    }
}

/// Récupère le client du hub, ou signale son absence.
fn hub_of(svc: &Services, sink: &EventSink, id: RequestId) -> Option<HubClient> {
    match &svc.hub {
        Some(h) => Some(h.clone()),
        None => {
            sink.fail(
                id,
                "le client HTTP n'a pas pu être initialisé : les registres d'images sont \
                 inaccessibles pour cette session"
                    .to_string(),
            );
            None
        }
    }
}

/// Récupère le moteur de mise à jour, ou signale son absence.
fn engine_of(svc: &Services, sink: &EventSink, id: RequestId) -> Option<UpdateEngine> {
    match &svc.engine {
        Some(e) => Some(e.clone()),
        None => {
            sink.fail(
                id,
                "le moteur de mise à jour est indisponible : le client HTTP n'a pas pu être \
                 initialisé"
                    .to_string(),
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Boucle du worker
// ---------------------------------------------------------------------------

/// Boucle principale du worker, exécutée dans le runtime tokio.
async fn worker_loop(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Command>,
    sink: EventSink,
    state_dir: PathBuf,
) {
    let clusters = ClusterManager::new(state_dir.clone());

    let store = Store::new(state_dir.join(UPDATER_STATE_FILE));
    if let Err(e) = store.load() {
        sink.fail(
            NO_REQUEST,
            format!("état des mises à jour illisible, on repart d'un état vide : {e}"),
        );
    }

    let hub = match HubClient::new() {
        Ok(h) => Some(h.with_github_token(store.settings().github_token)),
        Err(e) => {
            sink.fail(NO_REQUEST, format!("client HTTP du hub indisponible : {e}"));
            None
        }
    };
    let engine = hub
        .clone()
        .map(|h| UpdateEngine::new(clusters.clone(), h, store.clone()));

    let svc = Services {
        clusters,
        hub,
        engine,
        store,
    };

    // Reconnexion aux clusters persistés : EN TÂCHE DE FOND. Un cluster injoignable
    // met jusqu'à 30 s à répondre ; l'interface, elle, s'affiche tout de suite.
    {
        let svc = svc.clone();
        let sink = sink.clone();
        tokio::spawn(async move {
            if let Err(e) = svc.clusters.load_persisted().await {
                sink.fail(
                    NO_REQUEST,
                    format!("clusters enregistrés partiellement illisibles : {e}"),
                );
            }
            let clusters = svc.clusters.list_info().await;
            sink.send(Event::Clusters {
                id: NO_REQUEST,
                clusters,
            });
        });
    }

    let tasks: TaskMap = Arc::new(Mutex::new(HashMap::new()));

    while let Some(cmd) = rx.recv().await {
        if matches!(cmd, Command::Quit) {
            break;
        }

        // Signaler l'attente avant toute chose : l'interface affiche l'indicateur
        // dès l'image suivante, même si la commande met plusieurs secondes.
        if let (Some(id), Some(what)) = (cmd.id(), cmd.busy_label()) {
            sink.send(Event::Busy { id, what });
        }

        if cmd.is_immediate() {
            handle_immediate(cmd, &svc, &sink, &tasks);
            continue;
        }

        match cmd {
            Command::StartLogs {
                id,
                cluster,
                pod,
                opts,
            } => {
                let Some(handle) = handle_of(&svc, &sink, id, &cluster) else {
                    sink.send(Event::LogEnded { id });
                    continue;
                };
                let sink_task = sink.clone();
                let task = tokio::spawn(run_guarded(
                    sink.clone(),
                    id,
                    format!("journaux de {}", pod.display()),
                    pump_logs(id, handle, pod, opts, sink_task),
                ));
                register_task(
                    &tasks,
                    id,
                    TaskEntry {
                        handle: task,
                        exec: None,
                    },
                );
            }
            Command::StartExec {
                id,
                cluster,
                pod,
                container,
                command,
            } => {
                let Some(handle) = handle_of(&svc, &sink, id, &cluster) else {
                    sink.send(Event::ExecEnded {
                        id,
                        message: Some("cluster indisponible".to_string()),
                    });
                    continue;
                };
                let (stdin_tx, stdin_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
                let ex = ExecHandles {
                    stdin: stdin_tx,
                    control: Arc::new(Mutex::new(None)),
                    pending_size: Arc::new(Mutex::new(None)),
                };
                let sink_task = sink.clone();
                let label = format!("session sur {}", pod.display());
                let task = tokio::spawn(run_guarded(
                    sink.clone(),
                    id,
                    label,
                    pump_exec(
                        id,
                        handle,
                        pod,
                        container,
                        command,
                        sink_task,
                        ex.clone(),
                        stdin_rx,
                    ),
                ));
                register_task(
                    &tasks,
                    id,
                    TaskEntry {
                        handle: task,
                        exec: Some(ex),
                    },
                );
            }
            other => {
                let id = other.id().unwrap_or(NO_REQUEST);
                let what = other
                    .busy_label()
                    .unwrap_or_else(|| "opération".to_string());
                let svc = svc.clone();
                let sink_guard = sink.clone();
                let sink_task = sink.clone();
                tokio::spawn(run_guarded(
                    sink_guard,
                    id,
                    what,
                    execute(other, svc, sink_task),
                ));
            }
        }
    }

    // Fin de vie : plus aucune tâche ne doit survivre au worker.
    abort_all(&tasks);
}

/// Traite en ligne les commandes d'ordre : elles doivent prendre effet tout de
/// suite, sans se retrouver derrière les commandes lentes déjà en vol.
fn handle_immediate(cmd: Command, svc: &Services, sink: &EventSink, tasks: &TaskMap) {
    match cmd {
        Command::SelectCluster { name } => match svc.clusters.set_current(&name) {
            Ok(()) => {
                sink.send(Event::Ok {
                    id: NO_REQUEST,
                    message: format!("cluster courant : « {name} »"),
                });
                spawn_cluster_refresh(svc.clone(), sink.clone());
            }
            Err(e) => sink.fail(
                NO_REQUEST,
                format!("sélection du cluster « {name} » impossible : {e}"),
            ),
        },
        Command::RemoveCluster { name } => match svc.clusters.remove(&name) {
            Ok(()) => {
                sink.send(Event::Ok {
                    id: NO_REQUEST,
                    message: format!("cluster « {name} » retiré"),
                });
                spawn_cluster_refresh(svc.clone(), sink.clone());
            }
            Err(e) => sink.fail(
                NO_REQUEST,
                format!("suppression du cluster « {name} » impossible : {e}"),
            ),
        },
        Command::StopLogs { id } => {
            if let Some(entry) = tasks.lock().remove(&id) {
                abort_entry(&entry);
            }
            // La tâche est abandonnée : c'est ici que la fin du flux est annoncée.
            sink.send(Event::LogEnded { id });
        }
        Command::StopExec { id } => {
            if let Some(entry) = tasks.lock().remove(&id) {
                abort_entry(&entry);
            }
            sink.send(Event::ExecEnded {
                id,
                message: Some("session fermée".to_string()),
            });
        }
        Command::ExecInput { id, data } => {
            let sender = tasks.lock().get(&id).and_then(|e| e.exec.clone());
            match sender {
                // File non bornée : l'envoi est immédiat et ne perd aucune frappe.
                Some(ex) => {
                    if ex.stdin.send(data).is_err() {
                        sink.fail(id, "la session interactive est close".to_string());
                    }
                }
                None => sink.fail(id, "aucune session interactive sous cet écran".to_string()),
            }
        }
        Command::ExecResize { id, cols, rows } => {
            if let Some(ex) = tasks.lock().get(&id).and_then(|e| e.exec.clone()) {
                let control = ex.control.lock().clone();
                match control {
                    Some(c) => c.resize(cols, rows),
                    // La session n'est pas encore ouverte : la taille sera appliquée
                    // dès qu'elle le sera.
                    None => *ex.pending_size.lock() = Some((cols, rows)),
                }
            }
        }
        Command::Quit => abort_all(tasks),
        // Les autres commandes ne sont jamais routées ici (cf. `Command::is_immediate`).
        other => tracing::error!(commande = ?other, "commande non immédiate routée en ligne"),
    }
}

/// Renvoie l'état à jour du parc de clusters, sans bloquer l'appelant.
fn spawn_cluster_refresh(svc: Services, sink: EventSink) {
    tokio::spawn(async move {
        let clusters = svc.clusters.list_info().await;
        sink.send(Event::Clusters {
            id: NO_REQUEST,
            clusters,
        });
    });
}

// ---------------------------------------------------------------------------
// Garde-fou : une panique ne doit jamais rendre le worker muet
// ---------------------------------------------------------------------------

/// Exécute une commande en convertissant toute panique en [`Event::Failed`].
///
/// Sans cette enveloppe, une panique dans une dépendance (par exemple le choix du
/// fournisseur cryptographique de rustls, cf. l'en-tête de module) tuerait
/// silencieusement la tâche : l'interface resterait bloquée sur son indicateur
/// d'attente, sans la moindre explication.
async fn run_guarded<F>(sink: EventSink, id: RequestId, what: String, fut: F)
where
    F: std::future::Future<Output = ()> + Send,
{
    use futures::FutureExt;
    if let Err(payload) = std::panic::AssertUnwindSafe(fut).catch_unwind().await {
        let detail = describe_panic(payload);
        tracing::error!(operation = %what, detail = %detail, "panique interceptée dans le worker");
        sink.fail(
            id,
            format!("« {what} » a échoué : {detail}{}", repair_hint(&detail)),
        );
    }
}

/// Extrait un message lisible d'une charge utile de panique.
fn describe_panic(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "erreur interne sans message".to_string()
    }
}

/// Ajoute une piste de correction aux paniques dont la cause est connue.
fn repair_hint(detail: &str) -> &'static str {
    let lower = detail.to_lowercase();
    if lower.contains("cryptoprovider") || lower.contains("crypto provider") {
        " — le fournisseur cryptographique rustls n'a pas été choisi au démarrage : \
         installez-le explicitement dans main.rs (voir l'en-tête de backend/worker.rs)"
    } else {
        ""
    }
}

// ---------------------------------------------------------------------------
// Exécution d'une commande
// ---------------------------------------------------------------------------

/// Exécute une commande et émet le ou les évènements correspondants.
///
/// Aucune erreur n'est avalée : chaque chemin d'échec produit un [`Event::Failed`]
/// porteur d'un message français.
async fn execute(cmd: Command, svc: Services, sink: EventSink) {
    match cmd {
        // ---------------------------------------------------------- clusters
        Command::Connect {
            id,
            spec,
            name,
            persist,
        } => match svc.clusters.add(&name, spec, persist).await {
            Ok(h) => {
                sink.send(Event::Ok {
                    id,
                    message: format!("cluster « {} » connecté", h.name),
                });
                let clusters = svc.clusters.list_info().await;
                sink.send(Event::Clusters { id, clusters });
            }
            Err(e) => sink.fail(id, format!("connexion à « {name} » impossible : {e}")),
        },

        Command::ImportKubeconfig {
            id,
            path,
            all_contexts,
        } => match svc
            .clusters
            .import_kubeconfig(path.as_deref(), all_contexts)
            .await
        {
            Ok(added) => {
                sink.send(Event::Ok {
                    id,
                    message: match added.len() {
                        0 => "aucun contexte importé".to_string(),
                        1 => format!("cluster « {} » importé", added[0]),
                        n => format!("{n} clusters importés"),
                    },
                });
                let clusters = svc.clusters.list_info().await;
                sink.send(Event::Clusters { id, clusters });
            }
            Err(e) => sink.fail(id, format!("import du kubeconfig impossible : {e}")),
        },

        Command::ListContexts { id, path } => match client::list_contexts(path.as_deref()) {
            Ok(contexts) => sink.send(Event::Contexts { id, contexts }),
            Err(e) => sink.fail(id, format!("lecture du kubeconfig impossible : {e}")),
        },

        Command::RefreshClusters { id } => {
            let clusters = svc.clusters.list_info().await;
            sink.send(Event::Clusters { id, clusters });
        }

        // ----------------------------------------------------------- lecture
        Command::LoadOverview { id, cluster } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match metrics::overview(&h).await {
                Ok(overview) => sink.send(Event::Overview {
                    id,
                    cluster,
                    overview: Box::new(overview),
                }),
                Err(e) => sink.fail(id, format!("synthèse de « {cluster} » impossible : {e}")),
            };
        }

        Command::LoadKinds { id, cluster } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            // Le catalogue est en mémoire : lecture immédiate, verrou jamais tenu
            // au travers d'un point d'attente.
            let mut kinds: Vec<ResourceKind> = {
                let guard = h.catalog.read();
                guard.listable().into_iter().cloned().collect()
            };
            kinds.sort_by(|a, b| a.full_name().cmp(&b.full_name()));
            sink.send(Event::Kinds { id, cluster, kinds });
        }

        Command::LoadNamespaces { id, cluster } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            let kind = match h.resolve_kind("namespaces") {
                Ok(k) => k,
                Err(e) => {
                    sink.fail(id, format!("namespaces introuvables : {e}"));
                    return;
                }
            };
            let opts = ListOptions {
                limit: Some(1_000),
                ..Default::default()
            };
            match resource::list(&h, &kind, &opts).await {
                Ok(page) => {
                    let mut namespaces: Vec<String> =
                        page.items.into_iter().map(|i| i.name).collect();
                    namespaces.sort();
                    sink.send(Event::Namespaces {
                        id,
                        cluster,
                        namespaces,
                    });
                }
                Err(e) => sink.fail(id, format!("lecture des namespaces impossible : {e}")),
            };
        }

        Command::ListResources {
            id,
            cluster,
            kind,
            opts,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            // Copie possédée : `kind` est déplacé dans l'évènement de succès, alors
            // que le libellé du type reste nécessaire au message d'échec.
            let wanted: String = match kind.trim() {
                "" => "pods".to_string(),
                other => other.to_string(),
            };
            let resolved = match h.resolve_kind(&wanted) {
                Ok(k) => k,
                Err(e) => {
                    sink.fail(id, format!("type « {wanted} » inconnu : {e}"));
                    return;
                }
            };
            match resource::list(&h, &resolved, &opts).await {
                Ok(page) => sink.send(Event::Resources {
                    id,
                    cluster,
                    // On renvoie le type demandé, pas le type résolu : la vue compare
                    // cette valeur à sa sélection courante pour écarter les réponses
                    // obsolètes.
                    kind,
                    page: Box::new(page),
                }),
                Err(e) => sink.fail(id, format!("listing des « {wanted} » impossible : {e}")),
            };
        }

        Command::LoadGraph {
            id,
            cluster,
            namespace,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            let (objects, warnings) = load_graph(&h, namespace.as_deref()).await;
            if objects.is_empty() && warnings.len() == GRAPH_KINDS.len() {
                // Tout a échoué : le cluster ne répond pas, ou aucun droit.
                sink.fail(
                    id,
                    format!(
                        "topologie de « {cluster} » illisible : {}",
                        warnings.join(" ; ")
                    ),
                );
                return;
            }
            sink.send(Event::Graph {
                id,
                cluster,
                objects,
                warnings,
            });
        }

        Command::GetYaml {
            id,
            cluster,
            reference,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match resource::get_yaml(&h, &reference).await {
                Ok(yaml) => sink.send(Event::Yaml {
                    id,
                    reference,
                    yaml,
                }),
                Err(e) => sink.fail(
                    id,
                    format!("lecture de {} impossible : {e}", reference.display()),
                ),
            };
        }

        // ---------------------------------------------------------- écriture
        Command::ApplyYaml {
            id,
            cluster,
            yaml,
            dry_run,
            force,
            namespace,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            let opts = ApplyOptions {
                force,
                dry_run,
                default_namespace: namespace,
                ..Default::default()
            };
            match apply::apply_yaml(&h, &yaml, &opts).await {
                Ok(outcome) => sink.send(Event::ApplyOutcome {
                    id,
                    outcome: Box::new(outcome),
                }),
                Err(e) => sink.fail(id, format!("application impossible : {e}")),
            };
        }

        Command::DiffYaml {
            id,
            cluster,
            yaml,
            namespace,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            let opts = ApplyOptions {
                dry_run: true,
                default_namespace: namespace,
                ..Default::default()
            };
            match apply::diff_yaml(&h, &yaml, &opts).await {
                Ok(items) => sink.send(Event::Diff { id, items }),
                Err(e) => sink.fail(id, format!("calcul des différences impossible : {e}")),
            };
        }

        Command::DeleteYaml {
            id,
            cluster,
            yaml,
            namespace,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            let opts = ApplyOptions {
                default_namespace: namespace,
                ..Default::default()
            };
            match apply::delete_yaml(&h, &yaml, &opts).await {
                Ok(outcome) => sink.send(Event::ApplyOutcome {
                    id,
                    outcome: Box::new(outcome),
                }),
                Err(e) => sink.fail(id, format!("suppression impossible : {e}")),
            };
        }

        Command::ReplaceYaml {
            id,
            cluster,
            reference,
            yaml,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match resource::replace_yaml(&h, &reference, &yaml).await {
                Ok(()) => sink.send(Event::Ok {
                    id,
                    message: format!("{} remplacé", reference.display()),
                }),
                Err(e) => sink.fail(
                    id,
                    format!("remplacement de {} impossible : {e}", reference.display()),
                ),
            };
        }

        Command::DeleteResource {
            id,
            cluster,
            reference,
            propagation,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match resource::delete(&h, &reference, propagation.as_deref()).await {
                Ok(()) => sink.send(Event::Ok {
                    id,
                    message: format!("{} supprimé", reference.display()),
                }),
                Err(e) => sink.fail(
                    id,
                    format!("suppression de {} impossible : {e}", reference.display()),
                ),
            };
        }

        Command::Scale {
            id,
            cluster,
            reference,
            replicas,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match resource::scale(&h, &reference, replicas).await {
                Ok(()) => sink.send(Event::Ok {
                    id,
                    message: format!("{} : {replicas} réplique(s)", reference.display()),
                }),
                Err(e) => sink.fail(id, format!("changement d'échelle impossible : {e}")),
            };
        }

        Command::Restart {
            id,
            cluster,
            reference,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match resource::restart(&h, &reference).await {
                Ok(()) => sink.send(Event::Ok {
                    id,
                    message: format!("{} redémarré", reference.display()),
                }),
                Err(e) => sink.fail(id, format!("redémarrage impossible : {e}")),
            };
        }

        Command::SetImage {
            id,
            cluster,
            reference,
            container,
            image,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match resource::set_image(&h, &reference, container.as_deref(), &image).await {
                Ok(()) => sink.send(Event::Ok {
                    id,
                    message: format!("{} → {image}", reference.display()),
                }),
                Err(e) => sink.fail(id, format!("changement d'image impossible : {e}")),
            };
        }

        Command::Rollback {
            id,
            cluster,
            reference,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match resource::rollback(&h, &reference).await {
                Ok(()) => sink.send(Event::Ok {
                    id,
                    message: format!("{} revenu à la révision précédente", reference.display()),
                }),
                Err(e) => sink.fail(id, format!("retour arrière impossible : {e}")),
            };
        }

        Command::Cordon {
            id,
            cluster,
            node,
            on,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match resource::cordon(&h, &node, on).await {
                Ok(()) => sink.send(Event::Ok {
                    id,
                    message: if on {
                        format!("nœud « {node} » mis hors service")
                    } else {
                        format!("nœud « {node} » remis en service")
                    },
                }),
                Err(e) => sink.fail(id, format!("opération sur le nœud impossible : {e}")),
            };
        }

        Command::Drain { id, cluster, node } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match resource::drain(&h, &node, None).await {
                Ok(evicted) => sink.send(Event::Ok {
                    id,
                    message: format!("nœud « {node} » vidé : {} pod(s) évincé(s)", evicted.len()),
                }),
                Err(e) => sink.fail(id, format!("vidage du nœud « {node} » impossible : {e}")),
            };
        }

        // -------------------------------------------------- détail d'un objet
        Command::LoadEvents {
            id,
            cluster,
            namespace,
            involved,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match events::recent(&h, namespace.as_deref(), involved.as_ref(), EVENTS_LIMIT).await {
                Ok(events) => sink.send(Event::Events { id, events }),
                Err(e) => sink.fail(id, format!("lecture des évènements impossible : {e}")),
            };
        }

        Command::LoadContainers { id, cluster, pod } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match resource::containers(&h, &pod).await {
                Ok(containers) => sink.send(Event::Containers { id, containers }),
                Err(e) => sink.fail(
                    id,
                    format!("conteneurs de {} illisibles : {e}", pod.display()),
                ),
            };
        }

        // --------------------------------------------------------- métriques
        Command::LoadMetrics {
            id,
            cluster,
            namespace,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            let nodes = metrics::node_metrics(&h.client).await;
            let pods = metrics::pod_metrics(&h.client, namespace.as_deref()).await;
            match (nodes, pods) {
                (Err(en), Err(ep)) => sink.fail(
                    id,
                    format!(
                        "métriques indisponibles (l'API metrics.k8s.io répond-elle ?) : \
                         nœuds — {en} ; pods — {ep}"
                    ),
                ),
                (nodes, pods) => {
                    // Une moitié manquante n'efface pas l'autre : l'utilisateur voit
                    // ce qui est lisible, le détail part dans les journaux.
                    let nodes = nodes.unwrap_or_else(|e| {
                        tracing::warn!(erreur = %e, "métriques des nœuds indisponibles");
                        Vec::new()
                    });
                    let pods = pods.unwrap_or_else(|e| {
                        tracing::warn!(erreur = %e, "métriques des pods indisponibles");
                        Vec::new()
                    });
                    sink.send(Event::Metrics { id, nodes, pods });
                }
            }
        }

        // --------------------------------------------------------------- hub
        Command::HubSearchImages {
            id,
            query,
            registry,
        } => {
            let Some(hub) = hub_of(&svc, &sink, id) else {
                return;
            };
            match hub.search_images(&query, registry, HUB_LIMIT).await {
                Ok(images) => sink.send(Event::HubImages { id, images }),
                Err(e) => sink.fail(id, format!("recherche d'images impossible : {e}")),
            };
        }

        Command::HubListTags { id, image } => {
            let Some(hub) = hub_of(&svc, &sink, id) else {
                return;
            };
            let reference = match ImageRef::parse(&image) {
                Ok(r) => r,
                Err(e) => {
                    sink.fail(id, format!("référence d'image « {image} » invalide : {e}"));
                    return;
                }
            };
            match hub.list_tags(&reference, HUB_TAGS_LIMIT).await {
                Ok(tags) => sink.send(Event::HubTags { id, tags }),
                Err(e) => sink.fail(id, format!("tags de « {image} » illisibles : {e}")),
            };
        }

        Command::HubInspect { id, image } => {
            let Some(hub) = hub_of(&svc, &sink, id) else {
                return;
            };
            let reference = match ImageRef::parse(&image) {
                Ok(r) => r,
                Err(e) => {
                    sink.fail(id, format!("référence d'image « {image} » invalide : {e}"));
                    return;
                }
            };
            match hub.inspect(&reference).await {
                Ok(details) => sink.send(Event::HubDetails {
                    id,
                    details: Box::new(details),
                }),
                Err(e) => sink.fail(id, format!("inspection de « {image} » impossible : {e}")),
            };
        }

        Command::HubSearchCharts { id, query } => {
            let Some(hub) = hub_of(&svc, &sink, id) else {
                return;
            };
            match hub.search_charts(&query, HUB_LIMIT).await {
                Ok(charts) => sink.send(Event::HubCharts { id, charts }),
                Err(e) => sink.fail(id, format!("recherche de charts impossible : {e}")),
            };
        }

        Command::HubCatalog { id } => {
            // Catalogue embarqué dans le binaire : aucune entrée/sortie.
            sink.send(Event::HubCatalog {
                id,
                apps: catalog::builtin_apps().to_vec(),
            });
        }

        Command::HubRender { id, request } => match deploy::render_manifests(&request) {
            Ok(yaml) => sink.send(Event::HubRendered { id, yaml }),
            Err(e) => sink.fail(id, format!("génération des manifestes impossible : {e}")),
        },

        Command::HubDeploy {
            id,
            cluster,
            request,
            dry_run,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            let yaml = match deploy::render_manifests(&request) {
                Ok(y) => y,
                Err(e) => {
                    sink.fail(id, format!("génération des manifestes impossible : {e}"));
                    return;
                }
            };
            let opts = ApplyOptions {
                dry_run,
                default_namespace: Some(request.namespace.clone()),
                ..Default::default()
            };
            match apply::apply_yaml(&h, &yaml, &opts).await {
                Ok(outcome) => sink.send(Event::ApplyOutcome {
                    id,
                    outcome: Box::new(outcome),
                }),
                Err(e) => sink.fail(
                    id,
                    format!("déploiement de « {} » impossible : {e}", request.name),
                ),
            };
        }

        // ----------------------------------------------------------- updater
        Command::UpdatesList { id } => {
            sink.send(Event::Watchers {
                id,
                watchers: svc.store.watchers(),
                findings: svc.store.findings(),
            });
        }

        Command::UpdatesUpsertWatcher { id, watcher } => {
            let name = watcher.name.clone();
            match svc.store.upsert_watcher(*watcher) {
                Ok(()) => {
                    sink.send(Event::Watchers {
                        id,
                        watchers: svc.store.watchers(),
                        findings: svc.store.findings(),
                    });
                    sink.send(Event::Ok {
                        id,
                        message: format!("surveillant « {name} » enregistré"),
                    });
                }
                Err(e) => sink.fail(id, format!("enregistrement impossible : {e}")),
            };
        }

        Command::UpdatesRemoveWatcher { id, watcher_id } => {
            match svc.store.remove_watcher(&watcher_id) {
                Ok(()) => {
                    sink.send(Event::Watchers {
                        id,
                        watchers: svc.store.watchers(),
                        findings: svc.store.findings(),
                    });
                    sink.send(Event::Ok {
                        id,
                        message: "surveillant supprimé".to_string(),
                    });
                }
                Err(e) => sink.fail(id, format!("suppression impossible : {e}")),
            };
        }

        Command::UpdatesCheck { id } => {
            let Some(engine) = engine_of(&svc, &sink, id) else {
                return;
            };
            match engine.check_all().await {
                Ok(findings) => sink.send(Event::Findings { id, findings }),
                Err(e) => sink.fail(
                    id,
                    format!("vérification des mises à jour impossible : {e}"),
                ),
            };
        }

        Command::UpdatesApply { id, finding_id } => {
            let Some(engine) = engine_of(&svc, &sink, id) else {
                return;
            };
            match engine.apply(&finding_id).await {
                Ok(result) => sink.send(Event::RolloutDone {
                    id,
                    result: Box::new(result),
                }),
                Err(e) => sink.fail(
                    id,
                    format!("application de la mise à jour impossible : {e}"),
                ),
            };
        }

        Command::UpdatesHistory { id } => {
            sink.send(Event::History {
                id,
                history: svc.store.history(HISTORY_LIMIT),
            });
        }

        Command::UpdatesScan {
            id,
            cluster,
            namespace,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match scan::scan_workloads(&h, namespace.as_deref()).await {
                Ok(images) => sink.send(Event::WorkloadImages { id, images }),
                Err(e) => sink.fail(id, format!("inventaire des images impossible : {e}")),
            };
        }

        Command::UpdatesSuggest {
            id,
            cluster,
            namespace,
        } => {
            let Some(h) = handle_of(&svc, &sink, id, &cluster) else {
                return;
            };
            match scan::scan_workloads(&h, namespace.as_deref()).await {
                Ok(images) => {
                    let policy = svc.store.settings().default_policy;
                    sink.send(Event::Suggestions {
                        id,
                        watchers: scan::suggest_watchers(&images, &policy),
                    });
                }
                Err(e) => sink.fail(id, format!("suggestion de surveillants impossible : {e}")),
            };
        }

        Command::UpdatesSettings { id } => {
            sink.send(Event::Settings {
                id,
                // Jamais les secrets en clair vers l'interface.
                settings: Box::new(svc.store.settings_redacted()),
            });
        }

        Command::UpdatesSaveSettings { id, settings } => match svc.store.set_settings(*settings) {
            Ok(()) => {
                sink.send(Event::Settings {
                    id,
                    settings: Box::new(svc.store.settings_redacted()),
                });
                sink.send(Event::Ok {
                    id,
                    message: "réglages enregistrés".to_string(),
                });
            }
            Err(e) => sink.fail(id, format!("enregistrement des réglages impossible : {e}")),
        },

        // Traitées ailleurs : en ligne dans la boucle, ou par une tâche dédiée.
        Command::RemoveCluster { .. }
        | Command::SelectCluster { .. }
        | Command::StopLogs { .. }
        | Command::StartLogs { .. }
        | Command::StartExec { .. }
        | Command::ExecInput { .. }
        | Command::ExecResize { .. }
        | Command::StopExec { .. }
        | Command::Quit => {
            tracing::error!("commande routée vers le mauvais chemin d'exécution");
        }
    }
}

/// Lit tous les types de la topologie en parallèle.
///
/// Chaque type est paginé jusqu'au bout (ou jusqu'au plafond) ; un type absent
/// du cluster ou interdit par les droits devient un avertissement lisible,
/// les autres types sont renvoyés quand même.
async fn load_graph(
    h: &ClusterHandle,
    namespace: Option<&str>,
) -> (Vec<ObjectSummary>, Vec<String>) {
    let reads = GRAPH_KINDS.iter().map(|plural| async move {
        let kind = h
            .resolve_kind(plural)
            .map_err(|e| format!("{plural} : {e}"))?;
        let mut items: Vec<ObjectSummary> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let opts = ListOptions {
                namespace: namespace.map(str::to_string),
                limit: Some(GRAPH_PAGE),
                continue_token: token.take(),
                ..Default::default()
            };
            let page = resource::list(h, &kind, &opts)
                .await
                .map_err(|e| format!("{plural} : {e}"))?;
            items.extend(page.items);
            token = page.continue_token;
            if token.is_none() || items.len() >= GRAPH_MAX_PER_KIND {
                break;
            }
        }
        Ok::<_, String>(items)
    });

    let mut objects = Vec::new();
    let mut warnings = Vec::new();
    for result in futures::future::join_all(reads).await {
        match result {
            Ok(items) => objects.extend(items),
            Err(warning) => warnings.push(warning),
        }
    }
    (objects, warnings)
}

// ---------------------------------------------------------------------------
// Pompes de flux
// ---------------------------------------------------------------------------

/// Pompe un flux de journaux vers l'interface, en régulant le débit.
async fn pump_logs(
    id: RequestId,
    handle: ClusterHandle,
    pod: ResourceRef,
    opts: logs::LogOptions,
    sink: EventSink,
) {
    let mut stream = match logs::stream(&handle, &pod, opts).await {
        Ok(s) => s,
        Err(e) => {
            sink.fail(
                id,
                format!("journaux de {} indisponibles : {e}", pod.display()),
            );
            sink.send(Event::LogEnded { id });
            return;
        }
    };

    let mut batcher = LogBatcher::new(Instant::now());
    let mut pacer = RepaintPacer::new();
    let mut alive = true;
    // Sert uniquement à tracer les bascules du régulateur de débit.
    let mut grouped = false;

    while alive {
        // Tant qu'un lot attend, on borne l'attente pour le vider à temps ; sinon on
        // dort jusqu'à la prochaine ligne, sans réveil inutile.
        let next = if batcher.has_pending() {
            match tokio::time::timeout(UI_FRAME, stream.next()).await {
                Ok(item) => item,
                Err(_) => {
                    if let Some(chunk) = batcher.tick(Instant::now()) {
                        alive = sink.send_quiet(Event::LogLine { id, line: chunk });
                        sink.repaint();
                    }
                    continue;
                }
            }
        } else {
            stream.next().await
        };

        match next {
            Some(Ok(line)) => {
                if let Some(chunk) = batcher.push(line, Instant::now()) {
                    alive = sink.send_quiet(Event::LogLine { id, line: chunk });
                    if pacer.due(Instant::now()) {
                        sink.repaint();
                    }
                }
                if batcher.is_batching() != grouped {
                    grouped = batcher.is_batching();
                    tracing::debug!(
                        flux = id,
                        regroupement = grouped,
                        "régulation du débit des journaux"
                    );
                }
            }
            Some(Err(e)) => {
                sink.fail(id, format!("flux de journaux interrompu : {e}"));
                break;
            }
            None => break,
        }
    }

    if let Some(chunk) = batcher.flush() {
        sink.send_quiet(Event::LogLine { id, line: chunk });
    }
    sink.send(Event::LogEnded { id });
}

/// Ouvre une session interactive et fait circuler les octets dans les deux sens.
#[allow(clippy::too_many_arguments)]
async fn pump_exec(
    id: RequestId,
    handle: ClusterHandle,
    pod: ResourceRef,
    container: Option<String>,
    command: Vec<String>,
    sink: EventSink,
    handles: ExecHandles,
    mut stdin_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let session = match exec::start(&handle, &pod, container.as_deref(), command, true).await {
        Ok(s) => s,
        Err(e) => {
            sink.fail(
                id,
                format!("session sur {} impossible : {e}", pod.display()),
            );
            sink.send(Event::ExecEnded {
                id,
                message: Some("la session n'a pas pu s'ouvrir".to_string()),
            });
            return;
        }
    };

    let kubewatch_core::exec::ExecSession {
        stdin,
        mut output,
        control,
    } = session;

    *handles.control.lock() = Some(control.clone());
    // Une taille reçue avant l'ouverture de la session est appliquée maintenant.
    if let Some((cols, rows)) = handles.pending_size.lock().take() {
        control.resize(cols, rows);
    }

    // Pompe clavier : la file non bornée côté interface est déversée dans la file
    // bornée de la session, sans jamais faire attendre l'interface.
    let feeder = tokio::spawn(async move {
        while let Some(data) = stdin_rx.recv().await {
            if stdin.send(data).await.is_err() {
                break;
            }
        }
    });

    let mut pacer = RepaintPacer::new();
    while let Some(data) = output.recv().await {
        if !sink.send_quiet(Event::ExecOutput { id, data }) {
            break;
        }
        // Un `cat` sur un gros fichier produit des milliers de blocs : on réveille
        // l'interface au plus 30 fois par seconde, comme pour les journaux.
        if pacer.due(Instant::now()) {
            sink.repaint();
        }
    }

    feeder.abort();
    control.abort();
    sink.send(Event::ExecEnded { id, message: None });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// En régime normal, chaque ligne part seule et immédiatement.
    #[test]
    fn lignes_emises_une_a_une_en_regime_normal() {
        let t0 = Instant::now();
        let mut b = LogBatcher::new(t0);
        for i in 0..100 {
            let out = b.push(format!("ligne {i}"), t0 + Duration::from_millis(i as u64));
            assert_eq!(out.as_deref(), Some(format!("ligne {i}").as_str()));
        }
        assert!(!b.is_batching());
        assert!(!b.has_pending());
        assert!(b.flush().is_none());
    }

    /// Au-delà du seuil, les lignes sont regroupées et n'arrivent qu'une fois par image.
    #[test]
    fn regroupement_au_dela_du_seuil() {
        let t0 = Instant::now();
        let mut b = LogBatcher::new(t0);

        // Les `LOG_BURST_LINES` premières lignes passent encore une à une.
        for i in 0..LOG_BURST_LINES {
            assert!(
                b.push(format!("l{i}"), t0).is_some(),
                "la ligne {i} devait passer immédiatement"
            );
        }
        assert!(!b.is_batching());

        // La suivante fait basculer en mode regroupé, sans attendre la fin de la fenêtre.
        assert!(b.push("depassement".into(), t0).is_none());
        assert!(b.is_batching());

        // Tant que l'intervalle d'image n'est pas écoulé, tout s'accumule.
        for i in 0..100 {
            assert!(b.push(format!("x{i}"), t0).is_none());
        }
        assert!(b.has_pending());

        // Au bout de 33 ms, un seul évènement porte les 102 lignes, dans l'ordre.
        let chunk = b
            .push("derniere".into(), t0 + UI_FRAME)
            .expect("le lot devait être vidé");
        let lignes: Vec<&str> = chunk.lines().collect();
        assert_eq!(lignes.len(), 102);
        assert_eq!(lignes[0], "depassement");
        assert_eq!(lignes[1], "x0");
        assert_eq!(lignes[101], "derniere");
    }

    /// Quand le débit retombe, le regroupement se désactive à la fenêtre suivante.
    #[test]
    fn retour_au_regime_normal_apres_la_rafale() {
        let t0 = Instant::now();
        let mut b = LogBatcher::new(t0);
        for i in 0..=LOG_BURST_LINES {
            let _ = b.push(format!("l{i}"), t0);
        }
        assert!(b.is_batching());
        let _ = b.flush();

        let plus_tard = t0 + LOG_WINDOW + Duration::from_millis(100);
        assert_eq!(
            b.push("calme".into(), plus_tard).as_deref(),
            Some("calme"),
            "après une fenêtre calme, la ligne repart seule"
        );
        assert!(!b.is_batching());
    }

    /// L'accumulation reste bornée même si l'interface ne consomme pas assez vite.
    #[test]
    fn accumulation_bornee_en_memoire() {
        let t0 = Instant::now();
        let mut b = LogBatcher::new(t0);
        for i in 0..=LOG_BURST_LINES {
            let _ = b.push(format!("l{i}"), t0);
        }
        assert!(b.is_batching());

        // Instant figé : seul le plafond peut déclencher le vidage.
        let mut vide = None;
        for i in 0..MAX_PENDING_LINES {
            if let Some(chunk) = b.push(format!("x{i}"), t0) {
                vide = Some(chunk);
                break;
            }
        }
        let chunk = vide.expect("le plafond doit forcer un vidage");
        assert_eq!(chunk.lines().count(), MAX_PENDING_LINES);
    }

    /// Une pause du flux vide le lot en attente.
    #[test]
    fn pause_du_flux_vide_le_lot() {
        let t0 = Instant::now();
        let mut b = LogBatcher::new(t0);
        for i in 0..=LOG_BURST_LINES {
            let _ = b.push(format!("l{i}"), t0);
        }
        assert!(b.has_pending());
        let chunk = b.tick(t0 + UI_FRAME).expect("le lot doit être vidé");
        assert_eq!(chunk, "l2000");
        assert!(b.tick(t0 + UI_FRAME).is_none());
    }

    /// Le cadenceur ne réveille jamais l'interface plus de ~30 fois par seconde.
    #[test]
    fn cadenceur_limite_a_trente_hertz() {
        let t0 = Instant::now();
        let mut pacer = RepaintPacer::new();
        let mut reveils = 0usize;
        // Une sollicitation par milliseconde pendant une seconde.
        for ms in 0..1_000u64 {
            if pacer.due(t0 + Duration::from_millis(ms)) {
                reveils += 1;
            }
        }
        assert!(
            (29..=31).contains(&reveils),
            "le cadenceur a autorisé {reveils} réveils en une seconde"
        );
    }

    #[test]
    fn message_de_panique_lisible() {
        let statique: Box<dyn std::any::Any + Send> = Box::new("boum");
        assert_eq!(describe_panic(statique), "boum");

        let possede: Box<dyn std::any::Any + Send> = Box::new(String::from("échec interne"));
        assert_eq!(describe_panic(possede), "échec interne");

        let inconnu: Box<dyn std::any::Any + Send> = Box::new(42u32);
        assert_eq!(describe_panic(inconnu), "erreur interne sans message");
    }

    #[test]
    fn piste_de_correction_pour_le_fournisseur_rustls() {
        let hint = repair_hint("no process-level CryptoProvider available");
        assert!(hint.contains("rustls"), "la piste doit nommer rustls");
        assert_eq!(repair_hint("index out of bounds"), "");
    }
}
