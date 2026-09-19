//! Coquille de l'application : fenêtre, navigation, boucle d'évènements,
//! raccourcis clavier et persistance des préférences.
//!
//! Règle d'or de ce fichier : le thread d'interface ne bloque jamais. Toute
//! opération qui parle au réseau part vers le backend sous forme de
//! [`Command`] et revient plus tard sous forme d'[`Event`]. Les évènements sont
//! traités dans [`eframe::App::logic`], jamais pendant le dessin : une image
//! rendue voit toujours un état stable.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Align, Align2, Key, Layout, Modifiers, RichText, ViewportCommand};

use crate::backend::{Backend, Command, Event};
use crate::state::{AppState, View};
use crate::views::settings::{appliquer_densite, appliquer_theme, Density, ThemeChoice};
use crate::{format, icons, theme, views, widgets};

/// Libellé posé par la vue « Catalogue » sur un déploiement en cours.
///
/// Les vues marquent leurs requêtes d'un libellé (`AppState::pending`) : c'est
/// ce libellé qui permet d'aiguiller un `Event::ApplyOutcome` vers le
/// formulaire de déploiement plutôt que vers la console YAML.
const LBL_HUB_DEPLOY: &str = "Déploiement";

/// Clé sous laquelle les préférences sont rangées dans le stockage d'eframe.
const STORAGE_KEY: &str = "kubewatch.ui";

/// Nom de l'application, utilisé pour le titre et le dossier de stockage.
const APP_NAME: &str = "KubeWatch";

/// Bornes du facteur d'échelle de l'interface.
const ZOOM_RANGE: (f32, f32) = (0.7, 2.0);

// ---------------------------------------------------------------------------
// Démarrage
// ---------------------------------------------------------------------------

/// Point d'entrée : ouvre la fenêtre et rend la main à la fermeture.
pub fn run() -> eframe::Result<()> {
    init_tracing();
    install_crypto_provider();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            // Doit correspondre au nom du fichier .desktop et au <id> AppStream,
            // sinon le compositeur ne rattache pas la fenêtre à son entrée de bureau.
            .with_app_id("io.kubewatch.KubeWatch")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([960.0, 640.0])
            .with_icon(app_icon()),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| {
            let app = KubeWatchApp::new(cc)?;
            Ok(Box::new(app) as Box<dyn eframe::App>)
        }),
    )
}

/// Sélectionne explicitement le fournisseur cryptographique de rustls.
///
/// Le binaire embarque deux fournisseurs : `ring` (activé par `kube`) et
/// `aws-lc-rs` (activé par `reqwest`). Quand les deux sont présents, rustls
/// refuse de choisir seul et **panique à la première connexion TLS** — donc
/// dès l'import d'un kubeconfig. On tranche ici, avant que le moindre client
/// Kubernetes ou HTTP ne soit construit.
fn install_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // La seule erreur possible est une installation concurrente : dans ce
        // cas un fournisseur est déjà en place, ce qui est le résultat voulu.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}


/// Initialise `tracing`. `RUST_LOG` pilote le niveau, `info` par défaut.
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    // Les traces vont sur stderr : la sortie standard reste libre.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

// ---------------------------------------------------------------------------
// Icône de la fenêtre, dessinée par le code
// ---------------------------------------------------------------------------

/// Construit l'icône de l'application : un carré arrondi bleu portant un « K ».
///
/// Aucun fichier externe n'est lu : l'image est calculée en RGBA 64×64, ce qui
/// garantit que le binaire reste autonome.
fn app_icon() -> egui::IconData {
    const SIZE: u32 = 64;
    const MARGIN: f32 = 2.0;
    const RADIUS: f32 = 13.0;

    let mut image = image::RgbaImage::new(SIZE, SIZE);
    let side = SIZE as f32;

    // Les trois segments qui composent le glyphe « K ».
    let strokes = [
        ((20.0, 16.0), (20.0, 48.0)),
        ((21.0, 33.0), (42.0, 16.0)),
        ((21.0, 33.0), (42.0, 48.0)),
    ];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            let alpha = rounded_square_alpha(px, py, MARGIN, side - MARGIN, RADIUS);
            if alpha <= 0.0 {
                image.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
                continue;
            }

            // Dégradé vertical, du bleu profond vers le bleu ciel.
            let t = (py / side).clamp(0.0, 1.0);
            let background = mix((17, 94, 150), (56, 189, 248), t);

            // Couverture du glyphe : distance au segment le plus proche.
            let mut glyph = 0.0f32;
            for (a, b) in strokes {
                let d = segment_distance(px, py, a, b);
                glyph = glyph.max((3.0 - d).clamp(0.0, 1.0));
            }

            let color = mix(background, (255, 255, 255), glyph);
            let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
            image.put_pixel(x, y, image::Rgba([color.0, color.1, color.2, a]));
        }
    }

    egui::IconData {
        rgba: image.into_raw(),
        width: SIZE,
        height: SIZE,
    }
}

/// Couverture d'un pixel par un carré aux coins arrondis (anticrénelée).
fn rounded_square_alpha(px: f32, py: f32, min: f32, max: f32, radius: f32) -> f32 {
    let center = (min + max) / 2.0;
    let half = (max - min) / 2.0 - radius;
    let dx = (px - center).abs() - half;
    let dy = (py - center).abs() - half;
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    let inside = dx.max(dy).min(0.0);
    let distance = outside + inside - radius;
    (0.5 - distance).clamp(0.0, 1.0)
}

/// Distance d'un point à un segment.
fn segment_distance(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (ax, ay) = a;
    let (bx, by) = b;
    let (vx, vy) = (bx - ax, by - ay);
    let length2 = vx * vx + vy * vy;
    let t = if length2 <= f32::EPSILON {
        0.0
    } else {
        (((px - ax) * vx + (py - ay) * vy) / length2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (ax + vx * t, ay + vy * t);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Interpolation linéaire entre deux couleurs RVB.
fn mix(from: (u8, u8, u8), to: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    (
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

// ---------------------------------------------------------------------------
// Préférences persistées
// ---------------------------------------------------------------------------

/// Préférences restaurées d'une session à l'autre.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Persisted {
    /// Préférence de thème : 0 = système, 1 = clair, 2 = sombre.
    theme: u8,
    /// Densité d'affichage : 0 = compacte, 1 = normale, 2 = aérée.
    density: u8,
    /// Facteur d'échelle de l'interface.
    zoom: f32,
    /// Largeur du panneau de navigation.
    nav_width: f32,
    /// Rafraîchissement automatique.
    auto_refresh: bool,
    /// Période du rafraîchissement automatique, en secondes.
    refresh_secs: u64,
    /// Dernier cluster sélectionné.
    cluster: Option<String>,
    /// Dernier kind affiché.
    kind: String,
    /// Dernier écran affiché.
    view: usize,
    /// Taille de page demandée au serveur d'API.
    page_size: u32,
    /// Nombre de lignes de journal conservées.
    log_lines: usize,
    /// Confirmation avant les opérations destructrices.
    confirm_destructive: bool,
}

impl Default for Persisted {
    fn default() -> Self {
        let defaults = AppState::new();
        Self {
            theme: theme_to_u8(defaults.settings.theme),
            density: density_to_u8(defaults.settings.density),
            zoom: defaults.settings.zoom,
            nav_width: defaults.settings.nav_width,
            auto_refresh: defaults.auto_refresh,
            refresh_secs: defaults.refresh_every.as_secs(),
            cluster: None,
            kind: defaults.selected_kind.clone(),
            view: View::Overview.index(),
            page_size: defaults.settings.page_size,
            log_lines: defaults.settings.log_lines,
            confirm_destructive: defaults.settings.confirm_destructive,
        }
    }
}

/// Code de persistance d'une préférence de thème.
fn theme_to_u8(choice: ThemeChoice) -> u8 {
    match choice {
        ThemeChoice::System => 0,
        ThemeChoice::Light => 1,
        ThemeChoice::Dark => 2,
    }
}

/// Préférence de thème relue depuis le stockage ; toute valeur inconnue
/// retombe sur le réglage du système.
fn theme_from_u8(code: u8) -> ThemeChoice {
    match code {
        1 => ThemeChoice::Light,
        2 => ThemeChoice::Dark,
        _ => ThemeChoice::System,
    }
}

/// Code de persistance d'une densité d'affichage.
fn density_to_u8(density: Density) -> u8 {
    match density {
        Density::Compact => 0,
        Density::Normal => 1,
        Density::Comfortable => 2,
    }
}

/// Densité relue depuis le stockage ; toute valeur inconnue retombe sur normale.
fn density_from_u8(code: u8) -> Density {
    match code {
        0 => Density::Compact,
        2 => Density::Comfortable,
        _ => Density::Normal,
    }
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// L'application elle-même : un état, un backend, quelques bascules d'affichage.
pub struct KubeWatchApp {
    /// État affiché par les vues.
    state: AppState,
    /// Pont vers le cœur Kubernetes.
    backend: Backend,
    /// Fenêtre d'aide (F1).
    help_open: bool,
    /// Palette de recherche de kind (Ctrl+K).
    palette_open: bool,
    /// Texte saisi dans la palette.
    palette_query: String,
    /// Cluster à resélectionner dès que la liste arrive.
    restore_cluster: Option<String>,
    /// Kind à restaurer dès que la découverte arrive.
    restore_kind: Option<String>,
}

impl KubeWatchApp {
    /// Construit l'application : thème, polices, backend, préférences.
    ///
    /// Aucune opération réseau n'est lancée ici de façon bloquante : la
    /// première commande part vers le backend et la fenêtre s'ouvre
    /// immédiatement, même si aucun cluster n'est joignable.
    fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let persisted = cc
            .storage
            .and_then(|storage| eframe::get_value::<Persisted>(storage, STORAGE_KEY))
            .unwrap_or_default();

        // Polices : celles d'egui, plus la police d'icônes Phosphor embarquée
        // dans le binaire ; les tailles sont redéfinies dans `theme`.
        let mut fonts = egui::FontDefinitions::default();
        icons::install(&mut fonts);
        cc.egui_ctx.set_fonts(fonts);
        let choix = theme_from_u8(persisted.theme);
        let densite = density_from_u8(persisted.density);
        theme::install(&cc.egui_ctx);
        appliquer_theme(&cc.egui_ctx, choix);
        appliquer_densite(&cc.egui_ctx, densite);
        cc.egui_ctx
            .set_zoom_factor(persisted.zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1));

        // Le backend crée son propre runtime tokio et réveille l'interface
        // après chaque évènement.
        let backend = Backend::new(cc.egui_ctx.clone(), state_dir())?;

        let mut state = AppState::new();
        state.view = View::from_index(persisted.view);
        state.auto_refresh = persisted.auto_refresh;
        state.refresh_every = Duration::from_secs(persisted.refresh_secs.clamp(2, 3600));
        if !persisted.kind.trim().is_empty() {
            state.selected_kind = persisted.kind.clone();
        }
        state.settings.theme = choix;
        state.settings.density = densite;
        state.settings.dark = cc.egui_ctx.theme() == egui::Theme::Dark;
        state.settings.zoom = persisted.zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        state.settings.nav_width = persisted.nav_width.clamp(140.0, 420.0);
        state.settings.page_size = persisted.page_size.clamp(10, 5000);
        state.settings.log_lines = persisted.log_lines.clamp(200, 200_000);
        state.settings.confirm_destructive = persisted.confirm_destructive;
        state.detail.log_max_lines = state.settings.log_lines;
        state.last_message = "Prêt".to_string();

        let mut app = Self {
            state,
            backend,
            help_open: false,
            palette_open: false,
            palette_query: String::new(),
            restore_cluster: persisted.cluster.clone(),
            restore_kind: Some(persisted.kind),
        };

        app.refresh_clusters();
        Ok(app)
    }

    // -- Envoi de commandes -------------------------------------------------

    /// Demande la liste des clusters enregistrés.
    fn refresh_clusters(&mut self) {
        let id = self.state.begin("clusters");
        self.backend.send(Command::RefreshClusters { id });
    }

    /// Bascule sur un cluster et recharge tout ce qui en dépend.
    fn select_cluster(&mut self, name: String) {
        self.state.current_cluster = Some(name.clone());
        self.state.clear_cluster_data();
        self.backend
            .send(Command::SelectCluster { name: name.clone() });

        let id = self.state.begin("types de ressources");
        self.backend.send(Command::LoadKinds {
            id,
            cluster: name.clone(),
        });

        let id = self.state.begin("namespaces");
        self.backend.send(Command::LoadNamespaces {
            id,
            cluster: name.clone(),
        });

        self.reload_view();
    }

    /// Recharge les données de l'écran courant.
    fn reload_view(&mut self) {
        self.state.last_refresh = Instant::now();

        // La chaîne est copiée d'abord : l'emprunt sur l'état s'arrête ici.
        let current = self.state.cluster().map(str::to_string);
        let Some(cluster) = current else {
            // Sans cluster, seul l'écran des réglages a quelque chose à dire.
            if matches!(self.state.view, View::Settings) {
                self.refresh_clusters();
            }
            return;
        };

        match self.state.view {
            View::Overview => {
                let id = self.state.begin("synthèse du cluster");
                self.backend.send(Command::LoadOverview {
                    id,
                    cluster: cluster.clone(),
                });
                let id = self.state.begin("mesures");
                self.backend.send(Command::LoadMetrics {
                    id,
                    cluster,
                    namespace: self.state.namespace.clone(),
                });
            }
            View::Resources => self.list_resources(),
            View::Graph => views::graph::request(&mut self.state, &self.backend),
            View::Updates => {
                // Même libellé que la vue : son indicateur d'attente reste juste.
                let id = self.state.begin("Chargement des surveillances");
                self.backend.send(Command::UpdatesList { id });
            }
            View::Deploy => {
                // L'assistant confronte l'empreinte du déploiement à la
                // capacité du cluster : sans synthèse, il ne peut rien prévoir.
                let id = self.state.begin("synthèse du cluster");
                self.backend.send(Command::LoadOverview {
                    id,
                    cluster: cluster.clone(),
                });
                let id = self.state.begin("namespaces");
                self.backend.send(Command::LoadNamespaces { id, cluster });
            }
            View::Settings => self.refresh_clusters(),
            View::Yaml | View::Hub => {}
        }
    }

    /// Relance le listing du kind courant.
    fn list_resources(&mut self) {
        let current = self.state.cluster().map(str::to_string);
        let Some(cluster) = current else {
            return;
        };
        let kind = self.state.selected_kind.clone();
        if kind.trim().is_empty() {
            return;
        }
        let opts = self.state.list_options();
        let id = self.state.begin(format!("liste : {kind}"));
        self.backend.send(Command::ListResources {
            id,
            cluster,
            kind,
            opts,
        });
    }

    /// Change d'écran et charge ce qui lui manque.
    fn goto(&mut self, view: View) {
        if self.state.view == view {
            return;
        }
        self.state.view = view;
        self.reload_view();
    }

    /// Arrête les flux attachés au panneau de détail (journaux, terminal).
    fn stop_detail_streams(&mut self) {
        if let Some(id) = self.state.detail.log_stream.take() {
            self.backend.send(Command::StopLogs { id });
            self.state.finish(id);
        }
        if let Some(id) = self.state.detail.exec_session.take() {
            self.backend.send(Command::StopExec { id });
            self.state.finish(id);
        }
    }

    // -- Notifications ------------------------------------------------------

    /// Signale une réussite. Point de passage unique vers les notifications.
    fn notify_ok(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.state.last_message = message.clone();
        self.state.toasts.success(message);
    }

    /// Signale un échec, toujours avec un message lisible.
    fn notify_err(&mut self, message: impl Into<String>) {
        let message = message.into();
        tracing::warn!(%message, "échec d'une commande");
        self.state.last_message = message.clone();
        self.state.toasts.error(message);
    }

    // -- Réception des évènements ------------------------------------------

    /// Applique un évènement du backend à l'état.
    fn on_event(&mut self, event: Event) {
        match event {
            Event::Clusters { id, clusters } => {
                self.state.finish(id);
                self.state.clusters = clusters;
                if self.state.current_cluster.is_none() {
                    if let Some(name) = self.pick_cluster() {
                        self.select_cluster(name);
                    }
                }
            }

            Event::Contexts { id, contexts } => {
                self.state.finish(id);
                self.state.settings.contexts = contexts;
            }

            Event::Overview {
                id,
                cluster,
                overview,
            } => {
                self.state.finish(id);
                if self.is_current(&cluster) {
                    self.state.overview = Some(*overview);
                }
            }

            Event::Kinds { id, cluster, kinds } => {
                self.state.finish(id);
                if !self.is_current(&cluster) {
                    return;
                }
                let mut kinds = kinds;
                kinds.sort_by(|a, b| a.plural.cmp(&b.plural));
                self.state.kinds = kinds;

                // Le kind mémorisé n'existe pas forcément sur ce cluster.
                if let Some(wanted) = self.restore_kind.take() {
                    if self.state.kind_by_plural(&wanted).is_some() {
                        self.state.selected_kind = wanted;
                    }
                }
                if self
                    .state
                    .kind_by_plural(&self.state.selected_kind)
                    .is_none()
                {
                    if let Some(first) = self.state.kinds.first() {
                        self.state.selected_kind = first.plural.clone();
                    }
                }
                if matches!(self.state.view, View::Resources) {
                    self.list_resources();
                }
            }

            Event::Namespaces {
                id,
                cluster,
                namespaces,
            } => {
                self.state.finish(id);
                if self.is_current(&cluster) {
                    self.state.namespaces = namespaces;
                }
            }

            Event::Resources {
                id,
                cluster,
                kind,
                page,
            } => {
                self.state.finish(id);
                // Réponse obsolète : l'utilisateur a changé de kind entre-temps.
                if !self.is_current(&cluster) || kind != self.state.selected_kind {
                    return;
                }
                let page = *page;
                self.state.rows = page.items;
                self.state.continue_token = page.continue_token;
            }

            Event::Graph {
                id,
                cluster,
                objects,
                warnings,
            } => {
                self.state.finish(id);
                if self.state.graph.pending == Some(id) {
                    self.state.graph.pending = None;
                }
                if self.is_current(&cluster) {
                    self.state.graph.receive(objects, warnings);
                }
            }

            Event::Yaml {
                id,
                reference,
                yaml,
            } => {
                self.state.finish(id);
                if self.state.yaml_console.pending == Some(id) {
                    self.state.yaml_console.pending = None;
                    self.state.yaml_console.source = yaml;
                    self.state.yaml_console.path = None;
                } else {
                    self.state.detail.yaml_pending = None;
                    self.state.detail.reference = Some(reference);
                    self.state.detail.yaml = yaml;
                    self.state.detail.yaml_edited = false;
                }
            }

            Event::ApplyOutcome { id, outcome } => {
                // Le libellé de la requête dit d'où venait le manifeste.
                let origine = self.state.pending.get(&id).cloned().unwrap_or_default();
                let pour_le_hub = origine == LBL_HUB_DEPLOY;
                let pour_l_assistant = origine == views::deploy::LBL_DEPLOY;
                self.state.finish(id);
                self.state.yaml_console.pending = None;
                let outcome = *outcome;
                let total = outcome.items.len();
                let failed = outcome.failed;
                if pour_l_assistant {
                    self.state.wizard.outcome = Some(outcome);
                } else if pour_le_hub {
                    self.state.hub.deploy.outcome = Some(outcome);
                } else {
                    self.state.yaml_console.output = describe_outcome(&outcome);
                    self.state.yaml_console.outcome = Some(outcome);
                }
                if failed == 0 {
                    self.notify_ok(format!("{} document(s) appliqué(s)", total));
                } else {
                    self.notify_err(format!(
                        "{failed} document(s) en échec sur {total} ; voir le compte rendu"
                    ));
                }
            }

            Event::Diff { id, items } => {
                self.state.finish(id);
                self.state.yaml_console.pending = None;
                if items.is_empty() {
                    self.state.yaml_console.output =
                        "Aucune différence avec l'état du cluster.".to_string();
                } else {
                    self.state.yaml_console.output = format!("{} objet(s) diffèrent.", items.len());
                }
                self.state.yaml_console.diff = items;
            }

            Event::Events { id, events } => {
                self.state.finish(id);
                if self.state.detail.events_pending == Some(id) {
                    self.state.detail.events_pending = None;
                    self.state.detail.events = events;
                } else {
                    self.state.events = events;
                }
            }

            Event::Containers { id, containers } => {
                self.state.finish(id);
                self.state.detail.containers = containers;
                if self.state.detail.container.is_none() {
                    // Premier conteneur applicatif, à défaut le premier de la liste.
                    let pick = self
                        .state
                        .detail
                        .containers
                        .iter()
                        .find(|c| !c.init)
                        .or_else(|| self.state.detail.containers.first())
                        .map(|c| c.name.clone());
                    self.state.detail.container = pick;
                }
            }

            Event::LogLine { id, line } => {
                if self.state.detail.log_stream == Some(id) {
                    self.state.detail.push_log(line);
                }
            }

            Event::LogEnded { id } => {
                self.state.finish(id);
                if self.state.detail.log_stream == Some(id) {
                    self.state.detail.log_stream = None;
                    self.state.last_message = "Flux de journal terminé".to_string();
                }
            }

            Event::ExecOutput { id, data } => {
                if self.state.detail.exec_session == Some(id) {
                    self.state.detail.push_exec(&data);
                }
            }

            Event::ExecEnded { id, message } => {
                self.state.finish(id);
                if self.state.detail.exec_session == Some(id) {
                    self.state.detail.exec_session = None;
                }
                match message {
                    Some(m) => self.notify_err(format!("Terminal fermé : {m}")),
                    None => self.state.last_message = "Terminal fermé".to_string(),
                }
            }

            Event::Metrics { id, nodes, pods } => {
                self.state.finish(id);
                self.state.metrics_nodes = nodes;
                self.state.metrics_pods = pods;
            }

            Event::HubImages { id, images } => {
                self.state.finish(id);
                if images.is_empty() {
                    self.state.hub.message = Some("Aucune image ne correspond.".to_string());
                }
                self.state.hub.images = images;
            }

            Event::HubTags { id, tags } => {
                self.state.finish(id);
                self.state.hub.tags = tags;
            }

            Event::HubDetails { id, details } => {
                self.state.finish(id);
                self.state.hub.details = Some(*details);
            }

            Event::HubCharts { id, charts } => {
                self.state.finish(id);
                if charts.is_empty() {
                    self.state.hub.message = Some("Aucun chart ne correspond.".to_string());
                }
                self.state.hub.charts = charts;
            }

            Event::HubCatalog { id, apps } => {
                self.state.finish(id);
                self.state.hub.catalog = apps;
            }

            Event::HubRendered { id, yaml } => {
                let pour_l_assistant = self.state.pending.get(&id).map(String::as_str)
                    == Some(views::deploy::LBL_RENDER);
                self.state.finish(id);
                if pour_l_assistant {
                    self.state.wizard.preview = Some(yaml);
                } else {
                    self.state.hub.deploy.preview = Some(yaml);
                }
            }

            Event::Watchers {
                id,
                watchers,
                findings,
            } => {
                self.state.finish(id);
                self.state.updates.watchers = watchers;
                self.state.updates.findings = findings;
            }

            Event::Findings { id, findings } => {
                self.state.finish(id);
                self.state.updates.findings = findings;
            }

            Event::RolloutDone { id, result } => {
                self.state.finish(id);
                self.state.updates.history.insert(0, *result);
                self.notify_ok("Déploiement de mise à jour terminé");
            }

            Event::History { id, history } => {
                self.state.finish(id);
                self.state.updates.history = history;
            }

            Event::WorkloadImages { id, images } => {
                self.state.finish(id);
                self.state.updates.workload_images = images;
            }

            Event::Suggestions { id, watchers } => {
                self.state.finish(id);
                // Toutes les suggestions sont retenues par défaut ; la vue
                // laisse ensuite décocher ligne à ligne.
                self.state.updates.suggestion_keep = vec![true; watchers.len()];
                self.state.updates.suggestions = watchers;
            }

            Event::Settings { id, settings } => {
                self.state.finish(id);
                self.state.updates.settings = Some(*settings);
            }

            Event::Ok { id, message } => {
                self.state.finish(id);
                self.clear_pending(id);
                self.notify_ok(message);
                // Une écriture aboutie change ce que le tableau montre.
                if matches!(self.state.view, View::Resources) {
                    self.list_resources();
                }
            }

            Event::Failed { id, message } => {
                self.state.finish(id);
                self.clear_pending(id);
                self.notify_err(message);
            }

            Event::Busy { id: _, what } => {
                self.state.last_message = format!("{what}…");
            }
        }
    }

    /// Choisit le cluster à afficher au démarrage.
    fn pick_cluster(&mut self) -> Option<String> {
        let wanted = self.restore_cluster.take();
        wanted
            .filter(|name| self.state.clusters.iter().any(|c| &c.name == name))
            .or_else(|| {
                self.state
                    .clusters
                    .iter()
                    .find(|c| c.connected)
                    .map(|c| c.name.clone())
            })
            .or_else(|| self.state.clusters.first().map(|c| c.name.clone()))
    }

    /// Vrai si le nom désigne le cluster affiché.
    fn is_current(&self, cluster: &str) -> bool {
        self.state.cluster() == Some(cluster)
    }

    /// Libère les marqueurs « en cours » des sous-états portant cet identifiant.
    fn clear_pending(&mut self, id: crate::state::RequestId) {
        let slots = [
            &mut self.state.yaml_console.pending,
            &mut self.state.graph.pending,
            &mut self.state.detail.yaml_pending,
            &mut self.state.detail.events_pending,
            &mut self.state.detail.log_stream,
            &mut self.state.detail.exec_session,
        ];
        for slot in slots {
            if *slot == Some(id) {
                *slot = None;
            }
        }
    }

    /// Déclenche le rafraîchissement automatique quand son heure est venue.
    fn tick_auto_refresh(&mut self) {
        if !self.state.auto_refresh || self.state.current_cluster.is_none() {
            return;
        }
        // L'assistant affiche un formulaire, pas des données vivantes :
        // relancer la synthèse du cluster sous les doigts de l'utilisateur
        // coûterait un listing complet toutes les dix secondes sans rien lui
        // apprendre. Ctrl+R reste disponible pour la remettre à jour.
        if matches!(self.state.view, View::Deploy) {
            return;
        }
        if self.state.last_refresh.elapsed() < self.state.refresh_every {
            return;
        }
        // Si le cluster est lent, on n'empile pas les requêtes.
        if self.state.pending.len() > 4 {
            self.state.last_refresh = Instant::now();
            return;
        }
        self.reload_view();
    }

    /// Programme le prochain réveil de l'interface.
    ///
    /// Une application de bureau au repos ne doit pas consommer de CPU : on
    /// demande un réveil daté plutôt qu'un redessin permanent.
    fn schedule_repaint(&self, ctx: &egui::Context) {
        // Une seconde suffit à garder les âges affichés à jour.
        let mut delay = Duration::from_secs(1);

        if self.state.auto_refresh && self.state.current_cluster.is_some() {
            let remaining = self
                .state
                .refresh_every
                .saturating_sub(self.state.last_refresh.elapsed())
                .max(Duration::from_millis(100));
            delay = delay.min(remaining);
        }
        if self.state.is_busy() {
            delay = delay.min(Duration::from_millis(250));
        }

        ctx.request_repaint_after(delay);
    }

    // -- Raccourcis clavier -------------------------------------------------

    /// Applique les raccourcis globaux. Ils sont documentés dans l'aide (F1).
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let cmd = Modifiers::COMMAND;

        if ctx.input_mut(|i| i.consume_key(cmd, Key::R)) {
            self.reload_view();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, Key::K)) {
            self.palette_open = true;
            self.palette_query.clear();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, Key::Q)) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F1)) {
            self.help_open = !self.help_open;
        }

        const KEYS: [Key; 8] = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
        ];
        for (index, key) in KEYS.into_iter().enumerate() {
            if ctx.input_mut(|i| i.consume_key(cmd, key)) {
                self.goto(View::from_index(index));
            }
        }

        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.escape();
        }
    }

    /// Ferme, dans l'ordre, ce qui est ouvert par-dessus l'interface.
    fn escape(&mut self) {
        if self.palette_open {
            self.palette_open = false;
        } else if self.help_open {
            self.help_open = false;
        } else if self.state.confirm.is_some() {
            self.state.confirm = None;
        } else if self.state.detail.open {
            self.stop_detail_streams();
            self.state.detail.close();
        }
    }

    // -- Dessin -------------------------------------------------------------

    /// Barre supérieure : cluster, namespace, recherche, rafraîchissement, thème.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let mut pick_cluster: Option<String> = None;
        let mut relist = false;
        let mut refresh = false;
        let mut toggle_theme = false;

        {
            let st = &mut self.state;
            egui::Panel::top("barre").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("KubeWatch").strong());
                    ui.separator();

                    // Sélecteur de cluster.
                    let current = st.current_cluster.clone();
                    let label = current
                        .clone()
                        .unwrap_or_else(|| "Aucun cluster".to_string());
                    egui::ComboBox::from_id_salt("choix-cluster")
                        .selected_text(format::truncate(&label, 28))
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            if st.clusters.is_empty() {
                                ui.label(
                                    RichText::new("Aucun cluster enregistré").italics().weak(),
                                );
                            }
                            for cluster in &st.clusters {
                                let selected = current.as_deref() == Some(cluster.name.as_str());
                                let mark = if cluster.connected { "●" } else { "○" };
                                let text = format!("{mark} {}", cluster.name);
                                if ui.selectable_label(selected, text).clicked() && !selected {
                                    pick_cluster = Some(cluster.name.clone());
                                }
                            }
                        });

                    // Sélecteur de namespace.
                    let before = st.namespace.clone();
                    let label = st
                        .namespace
                        .clone()
                        .unwrap_or_else(|| "Tous les namespaces".to_string());
                    egui::ComboBox::from_id_salt("choix-namespace")
                        .selected_text(format::truncate(&label, 24))
                        .width(190.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut st.namespace, None, "Tous les namespaces");
                            for ns in &st.namespaces {
                                ui.selectable_value(
                                    &mut st.namespace,
                                    Some(ns.clone()),
                                    ns.as_str(),
                                );
                            }
                        });
                    if before != st.namespace {
                        relist = true;
                    }

                    // Recherche globale (filtre local du tableau).
                    ui.add(
                        egui::TextEdit::singleline(&mut st.filter)
                            .hint_text("Filtrer (nom, namespace, statut)")
                            .desired_width(240.0),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button(if st.settings.dark {
                                icons::SUN
                            } else {
                                icons::MOON
                            })
                            .on_hover_text("Basculer le thème clair / sombre")
                            .clicked()
                        {
                            toggle_theme = true;
                        }
                        if ui
                            .toggle_value(&mut st.auto_refresh, format!("{} auto", icons::REFRESH))
                            .on_hover_text(format!(
                                "Rafraîchissement automatique toutes les {} s",
                                st.refresh_every.as_secs()
                            ))
                            .changed()
                        {
                            st.last_refresh = Instant::now();
                        }
                        if ui.button("Rafraîchir").on_hover_text("Ctrl+R").clicked() {
                            refresh = true;
                        }
                        if st.is_busy() {
                            ui.spinner();
                        }
                    });
                });
                ui.add_space(4.0);
            });
        }

        if let Some(name) = pick_cluster {
            self.select_cluster(name);
        }
        if toggle_theme {
            // La bascule fige la préférence : on sort de « Système » dès que
            // l'utilisateur choisit lui-même.
            let vers_sombre = !self.state.settings.dark;
            self.state.settings.theme = if vers_sombre {
                ThemeChoice::Dark
            } else {
                ThemeChoice::Light
            };
            self.state.settings.dark = vers_sombre;
            appliquer_theme(ui.ctx(), self.state.settings.theme);
        }
        if refresh {
            self.reload_view();
        } else if relist {
            self.list_resources();
            // La topologie est lue pour un namespace donné : elle suit le sélecteur.
            if matches!(self.state.view, View::Graph) {
                views::graph::request(&mut self.state, &self.backend);
            }
        }
    }

    /// Panneau de navigation : les écrans et l'état du cluster courant.
    fn nav_panel(&mut self, ui: &mut egui::Ui) {
        let mut goto: Option<View> = None;
        let dark = self.state.settings.dark;
        let palette = theme::palette(dark);
        let width = self.state.settings.nav_width;

        let response = {
            let st = &mut self.state;
            egui::Panel::left("nav")
                .resizable(true)
                .default_size(width)
                .min_size(150.0)
                .max_size(420.0)
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    for view in View::ALL {
                        let selected = st.view == view;
                        let text = format!("{}  {}", view.icon(), view.label());
                        let response = ui.selectable_label(selected, text);
                        if response.on_hover_text(view.shortcut()).clicked() && !selected {
                            goto = Some(view);
                        }
                    }

                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // État du cluster courant.
                    match st.cluster_info() {
                        None => {
                            ui.label(RichText::new("Aucun cluster").color(palette.muted));
                            ui.label(
                                RichText::new("Réglages → Ajouter un cluster")
                                    .small()
                                    .color(palette.muted),
                            );
                        }
                        Some(info) => {
                            let (mark, color, texte) = if info.connected {
                                ("●", palette.ok, "connecté")
                            } else {
                                ("●", palette.error, "injoignable")
                            };
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(mark).color(color));
                                ui.label(RichText::new(info.name.as_str()).strong());
                            });
                            ui.label(RichText::new(texte).small().color(color));
                            if let Some(version) = &info.version {
                                ui.label(
                                    RichText::new(version.as_str()).small().color(palette.muted),
                                );
                            }
                            if !info.server.is_empty() {
                                ui.label(
                                    RichText::new(format::truncate(&info.server, 34))
                                        .small()
                                        .color(palette.muted),
                                )
                                .on_hover_text(info.server.as_str());
                            }
                            if let Some(err) = &info.last_error {
                                ui.label(
                                    RichText::new(format::truncate(err, 60))
                                        .small()
                                        .color(palette.error),
                                )
                                .on_hover_text(err.as_str());
                            }
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(if info.metrics_available {
                                    "mesures disponibles"
                                } else {
                                    "mesures indisponibles"
                                })
                                .small()
                                .color(palette.muted),
                            );
                        }
                    }
                })
        };

        // La largeur retenue par l'utilisateur est persistée.
        self.state.settings.nav_width = response.response.rect.width().clamp(140.0, 420.0);

        if let Some(view) = goto {
            self.goto(view);
        }
    }

    /// Barre d'état : cluster, version, requêtes en vol, dernier message.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(self.state.settings.dark);
        let st = &self.state;

        egui::Panel::bottom("statut").show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let cluster = st.cluster().unwrap_or("—");
                ui.label(RichText::new(cluster).small().strong());
                ui.separator();

                let version = st
                    .cluster_info()
                    .and_then(|c| c.version.clone())
                    .unwrap_or_else(|| format::UNKNOWN.to_string());
                ui.label(RichText::new(version).small().color(palette.muted));
                ui.separator();

                let namespace = st.namespace.as_deref().unwrap_or("tous namespaces");
                ui.label(RichText::new(namespace).small().color(palette.muted));
                ui.separator();

                if st.pending.is_empty() {
                    ui.label(
                        RichText::new("aucune requête en vol")
                            .small()
                            .color(palette.muted),
                    );
                } else {
                    let labels: Vec<&str> =
                        st.pending.values().map(String::as_str).take(3).collect();
                    ui.label(
                        RichText::new(format!(
                            "{} requête(s) : {}",
                            st.pending.len(),
                            labels.join(", ")
                        ))
                        .small()
                        .color(palette.info),
                    );
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("F1 : aide").small().color(palette.muted));
                    ui.separator();
                    ui.label(
                        RichText::new(format::truncate(&st.last_message, 90))
                            .small()
                            .color(palette.muted),
                    )
                    .on_hover_text(st.last_message.as_str());
                });
            });
            ui.add_space(2.0);
        });
    }

    /// Zone centrale : panneau de détail à droite, puis la vue courante.
    ///
    /// L'ordre compte : un panneau latéral doit être posé avant la zone
    /// centrale, sinon celle-ci a déjà pris toute la place.
    fn central(&mut self, ui: &mut egui::Ui) {
        let Self { state, backend, .. } = self;
        views::detail::show(ui, state, backend);
        egui::CentralPanel::default().show(ui, |ui| match state.view {
            View::Overview => views::overview::show(ui, state, backend),
            View::Resources => views::resources::show(ui, state, backend),
            View::Graph => views::graph::show(ui, state, backend),
            View::Yaml => views::yaml::show(ui, state, backend),
            View::Deploy => views::deploy::show(ui, state, backend),
            View::Hub => views::hub::show(ui, state, backend),
            View::Updates => views::updates::show(ui, state, backend),
            View::Settings => views::settings::show(ui, state, backend),
        });
    }

    /// Surcouches dessinées par-dessus tout le reste.
    ///
    /// La modale de confirmation n'exécute rien : elle renvoie l'action validée,
    /// que [`Self::run_confirmed`] traduit en commande pour le backend.
    fn overlays(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.help_window(ctx);
        self.kind_palette(ctx);

        if let Some(action) = widgets::confirm::show(ctx, &mut self.state.confirm) {
            self.run_confirmed(action);
        }
        self.state.toasts.show(ui);
    }

    /// Traduit l'action validée dans la modale en commande pour le backend.
    fn run_confirmed(&mut self, action: widgets::confirm::ConfirmAction) {
        use widgets::confirm::ConfirmAction as Action;

        /// Message unique quand l'action exige un cluster et qu'il n'y en a pas.
        const SANS_CLUSTER: &str = "Aucun cluster sélectionné : l'action est annulée.";

        let cluster = self.state.cluster().map(str::to_string);

        match action {
            Action::DeleteResource(reference) => {
                if let Some(cluster) = cluster {
                    let id = self
                        .state
                        .begin(format!("suppression de {}", reference.display()));
                    self.backend.send(Command::DeleteResource {
                        id,
                        cluster,
                        reference,
                        // Suppression en avant-plan : les objets dépendants
                        // partent avant que l'objet lui-même disparaisse.
                        propagation: Some("Foreground".to_string()),
                    });
                } else {
                    self.notify_err(SANS_CLUSTER);
                }
            }

            Action::DrainNode(node) => {
                if let Some(cluster) = cluster {
                    let id = self.state.begin(format!("vidange du nœud {node}"));
                    self.backend.send(Command::Drain { id, cluster, node });
                } else {
                    self.notify_err(SANS_CLUSTER);
                }
            }

            Action::RestartWorkload(reference) => {
                if let Some(cluster) = cluster {
                    let id = self
                        .state
                        .begin(format!("redémarrage de {}", reference.display()));
                    self.backend.send(Command::Restart {
                        id,
                        cluster,
                        reference,
                    });
                } else {
                    self.notify_err(SANS_CLUSTER);
                }
            }

            Action::RollbackWorkload(reference) => {
                if let Some(cluster) = cluster {
                    let id = self
                        .state
                        .begin(format!("rollback de {}", reference.display()));
                    self.backend.send(Command::Rollback {
                        id,
                        cluster,
                        reference,
                    });
                } else {
                    self.notify_err(SANS_CLUSTER);
                }
            }

            Action::ScaleWorkload(reference, replicas) => {
                if let Some(cluster) = cluster {
                    let id = self
                        .state
                        .begin(format!("{} → {replicas} réplique(s)", reference.display()));
                    self.backend.send(Command::Scale {
                        id,
                        cluster,
                        reference,
                        replicas,
                    });
                } else {
                    self.notify_err(SANS_CLUSTER);
                }
            }
        }
    }

    /// Fenêtre d'aide (F1) : la liste des raccourcis.
    fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.help_open;
        egui::Window::new("Aide — raccourcis clavier")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let raccourcis = [
                    ("Ctrl+R", "Rafraîchir l'écran courant"),
                    ("Ctrl+K", "Chercher un type de ressource"),
                    ("Ctrl+1 … Ctrl+8", "Changer d'écran"),
                    ("Échap", "Fermer la modale, la palette ou le détail"),
                    ("Ctrl+Q", "Quitter"),
                    ("F1", "Afficher ou masquer cette aide"),
                ];
                egui::Grid::new("grille-aide")
                    .num_columns(2)
                    .spacing([18.0, 6.0])
                    .show(ui, |ui| {
                        for (touche, effet) in raccourcis {
                            ui.label(RichText::new(touche).monospace().strong());
                            ui.label(effet);
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "KubeWatch reste utilisable sans cluster joignable : \
                         aucune opération réseau ne bloque l'interface.",
                    )
                    .small()
                    .weak(),
                );
            });
        self.help_open = open;
    }

    /// Palette de recherche de kind (Ctrl+K).
    fn kind_palette(&mut self, ctx: &egui::Context) {
        if !self.palette_open {
            return;
        }

        let mut open = true;
        let mut chosen: Option<String> = None;
        let query = self.palette_query.to_ascii_lowercase();

        let matches: Vec<(String, String)> = self
            .state
            .kinds
            .iter()
            .filter(|k| {
                query.is_empty()
                    || k.plural.to_ascii_lowercase().contains(&query)
                    || k.kind.to_ascii_lowercase().contains(&query)
                    || k.short_names
                        .iter()
                        .any(|s| s.to_ascii_lowercase().contains(&query))
            })
            .take(60)
            .map(|k| {
                let detail = if k.group.is_empty() {
                    k.kind.clone()
                } else {
                    format!("{} · {}", k.kind, k.group)
                };
                (k.plural.clone(), detail)
            })
            .collect();

        egui::Window::new("Chercher un type de ressource")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(Align2::CENTER_TOP, [0.0, 90.0])
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.palette_query)
                        .hint_text("pods, deploy, svc…")
                        .desired_width(f32::INFINITY),
                );
                response.request_focus();

                let valider = ui.ctx().input(|i| i.key_pressed(Key::Enter));
                if valider {
                    if let Some((plural, _)) = matches.first() {
                        chosen = Some(plural.clone());
                    }
                }

                ui.add_space(6.0);
                if matches.is_empty() {
                    ui.label(RichText::new("Aucun type ne correspond").weak());
                }
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for (plural, detail) in &matches {
                            let selected = *plural == self.state.selected_kind;
                            if ui
                                .selectable_label(selected, format!("{plural}  —  {detail}"))
                                .clicked()
                            {
                                chosen = Some(plural.clone());
                            }
                        }
                    });
            });

        if let Some(plural) = chosen {
            self.state.selected_kind = plural;
            self.state.rows.clear();
            self.state.selected = None;
            self.palette_open = false;
            self.state.view = View::Resources;
            self.list_resources();
        } else {
            self.palette_open = open;
        }
    }

    /// Préférences à écrire sur le disque.
    fn persisted(&self) -> Persisted {
        Persisted {
            theme: theme_to_u8(self.state.settings.theme),
            density: density_to_u8(self.state.settings.density),
            zoom: self.state.settings.zoom,
            nav_width: self.state.settings.nav_width,
            auto_refresh: self.state.auto_refresh,
            refresh_secs: self.state.refresh_every.as_secs().max(1),
            cluster: self.state.current_cluster.clone(),
            kind: self.state.selected_kind.clone(),
            view: self.state.view.index(),
            page_size: self.state.settings.page_size,
            log_lines: self.state.settings.log_lines,
            confirm_destructive: self.state.settings.confirm_destructive,
        }
    }
}

impl eframe::App for KubeWatchApp {
    /// Traitement des évènements : appelé avant le dessin, sans rien dessiner.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for event in self.backend.drain() {
            self.on_event(event);
        }

        // Le thème effectif peut changer sans nous (préférence « Système »
        // suivant le bureau) : on le relit plutôt que de le supposer.
        self.state.settings.dark = ctx.theme() == egui::Theme::Dark;
        self.state.detail.log_max_lines = self.state.settings.log_lines.max(200);

        self.tick_auto_refresh();
        self.schedule_repaint(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.shortcuts(&ctx);
        self.top_bar(ui);
        self.status_bar(ui);
        self.nav_panel(ui);
        self.central(ui);
        self.overlays(ui, &ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.persisted());
    }
}

impl Drop for KubeWatchApp {
    fn drop(&mut self) {
        // Le worker arrête ses flux et rend la main ; l'envoi ne bloque jamais.
        self.backend.send(Command::Quit);
    }
}

// ---------------------------------------------------------------------------
// Utilitaires
// ---------------------------------------------------------------------------

/// Dossier d'état de KubeWatch : `<data_dir>/kubewatch`, avec repli local.
///
/// `KUBEWATCH_STATE_DIR` le remplace, pour isoler un profil de test. Le dossier
/// est celui des versions précédentes : les clusters enregistrés et l'état du
/// moteur de mise à jour d'une installation existante sont repris tels quels.
fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("KUBEWATCH_STATE_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Some(dir) = dirs::data_dir() {
        return dir.join("kubewatch");
    }
    if let Some(dir) = dirs::home_dir() {
        return dir.join(".kubewatch");
    }
    PathBuf::from(".kubewatch")
}

/// Résume le bilan d'une application de manifeste en texte lisible.
fn describe_outcome(outcome: &kubewatch_core::apply::ApplyOutcome) -> String {
    let total = outcome.items.len();
    let ok = total.saturating_sub(outcome.failed);
    let mut lines = vec![format!(
        "{ok} document(s) appliqué(s), {} en échec, {total} au total.",
        outcome.failed
    )];
    match serde_json::to_string_pretty(outcome) {
        Ok(detail) => lines.push(detail),
        Err(err) => lines.push(format!("(détail illisible : {err})")),
    }
    lines.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icone_carree_en_rgba() {
        let icon = app_icon();
        assert_eq!(icon.width, 64);
        assert_eq!(icon.height, 64);
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
        // Le centre est opaque, les coins sont transparents.
        let at = |x: usize, y: usize| icon.rgba[(y * 64 + x) * 4 + 3];
        assert_eq!(at(32, 32), 255);
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(63, 63), 0);
    }

    #[test]
    fn preferences_par_defaut_coherentes() {
        let p = Persisted::default();
        assert!(p.refresh_secs >= 2);
        assert!(p.zoom >= ZOOM_RANGE.0 && p.zoom <= ZOOM_RANGE.1);
        assert_eq!(p.view, View::Overview.index());
    }

    #[test]
    fn distance_au_segment() {
        // Point sur le segment.
        assert!(segment_distance(5.0, 0.0, (0.0, 0.0), (10.0, 0.0)) < 1e-6);
        // Point à l'aplomb du milieu.
        assert!((segment_distance(5.0, 3.0, (0.0, 0.0), (10.0, 0.0)) - 3.0).abs() < 1e-6);
        // Au-delà de l'extrémité : la distance est mesurée depuis celle-ci.
        assert!((segment_distance(13.0, 4.0, (0.0, 0.0), (10.0, 0.0)) - 5.0).abs() < 1e-6);
    }
}
