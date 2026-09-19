//! État de l'application.
//!
//! Tout ce que l'interface affiche vit ici. Les vues lisent cet état et
//! envoient des [`crate::backend::Command`] ; elles ne font jamais d'appel
//! réseau et n'écrivent jamais sur le disque. Le seul endroit où cet état
//! change est `app.rs`, dans `logic()`, à la réception des évènements du
//! backend : pendant le dessin, l'état est stable.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use indexmap::IndexMap;

use kubewatch_core::apply::ApplyOutcome;
use kubewatch_core::logs::LogOptions;
use kubewatch_core::model::{
    ClusterInfo, ClusterOverview, ContainerInfo, ContextInfo, EventSummary, ListOptions,
    MetricsSample, ObjectSummary, ResourceKind, ResourceRef,
};
use kubewatch_hub::model::{
    CatalogApp, ChartSummary, ImageDetails, ImageSummary, RegistryKind, TagInfo,
};
use kubewatch_updater::model::{RolloutResult, UpdateFinding, WatcherSpec};
use kubewatch_updater::scan::WorkloadImage;
use kubewatch_updater::store::UpdaterSettings;

use crate::icons;
use crate::views::deploy::WizardState;
use crate::views::graph::GraphState;
use crate::views::hub::{DeployForm, HubTab};
use crate::views::settings::{AddClusterMode, Density, SettingsConfirm, SettingsTab, ThemeChoice};
use crate::views::updates::{UpdatesConfirm, UpdatesTab, WatcherForm};
use crate::widgets;

/// Identifiant d'une requête en vol.
///
/// Compteur monotone : l'interface compare l'identifiant porté par un
/// évènement à celui qu'elle attend et ignore les réponses obsolètes
/// (l'utilisateur a changé d'écran pendant que le cluster répondait).
/// Le backend utilise le même alias ; tous deux sont des `u64`.
pub type RequestId = u64;

/// Kind affiché par défaut au premier démarrage.
pub const DEFAULT_KIND: &str = "pods";

/// Nombre de lignes de journal conservées par défaut en mémoire.
pub const DEFAULT_LOG_LINES: usize = 5_000;

/// Taille de page demandée au serveur d'API par défaut.
pub const DEFAULT_PAGE_SIZE: u32 = 500;

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

/// Les huit écrans de l'application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum View {
    /// Synthèse du cluster courant.
    #[default]
    Overview,
    /// Tableau des objets d'un kind.
    Resources,
    /// Graphe des objets et de leurs liens : charges, pods, services, nœuds.
    Graph,
    /// Console YAML : appliquer, comparer, supprimer un manifeste.
    Yaml,
    /// Assistant de déploiement en quatre étapes.
    Deploy,
    /// Recherche d'images et de charts, formulaire de déploiement complet.
    Hub,
    /// Surveillance des nouvelles versions.
    Updates,
    /// Réglages de l'application et gestion des clusters.
    Settings,
}

impl View {
    /// Les vues dans l'ordre de la barre de navigation.
    pub const ALL: [View; 8] = [
        View::Overview,
        View::Resources,
        View::Graph,
        View::Yaml,
        View::Deploy,
        View::Hub,
        View::Updates,
        View::Settings,
    ];

    /// Libellé affiché dans la navigation.
    pub fn label(self) -> &'static str {
        match self {
            View::Overview => "Vue d'ensemble",
            View::Resources => "Ressources",
            View::Graph => "Topologie",
            View::Yaml => "Console YAML",
            View::Deploy => "Déployer",
            View::Hub => "Catalogue",
            View::Updates => "Mises à jour",
            View::Settings => "Réglages",
        }
    }

    /// Icône de la vue : un glyphe de la police Phosphor (voir [`crate::icons`]).
    pub fn icon(self) -> &'static str {
        match self {
            View::Overview => icons::OVERVIEW,
            View::Resources => icons::RESOURCES,
            View::Graph => icons::GRAPH,
            View::Yaml => icons::YAML,
            View::Deploy => icons::DEPLOY,
            View::Hub => icons::HUB,
            View::Updates => icons::UPDATES,
            View::Settings => icons::SETTINGS,
        }
    }

    /// Raccourci clavier associé, pour l'aide et les infobulles.
    pub fn shortcut(self) -> &'static str {
        match self {
            View::Overview => "Ctrl+1",
            View::Resources => "Ctrl+2",
            View::Graph => "Ctrl+3",
            View::Yaml => "Ctrl+4",
            View::Deploy => "Ctrl+5",
            View::Hub => "Ctrl+6",
            View::Updates => "Ctrl+7",
            View::Settings => "Ctrl+8",
        }
    }

    /// Position de la vue dans [`View::ALL`], utilisée pour la persistance.
    pub fn index(self) -> usize {
        View::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }

    /// Vue correspondant à une position ; hors bornes, la vue d'ensemble.
    pub fn from_index(index: usize) -> View {
        View::ALL.get(index).copied().unwrap_or(View::Overview)
    }
}

// ---------------------------------------------------------------------------
// Panneau de détail
// ---------------------------------------------------------------------------

/// Onglet actif du panneau de détail d'un objet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailTab {
    /// Manifeste complet de l'objet.
    #[default]
    Yaml,
    /// Évènements concernant l'objet.
    Events,
    /// Conteneurs du pod.
    Containers,
    /// Journal d'un conteneur.
    Logs,
    /// Terminal interactif dans un conteneur.
    Exec,
}

/// État du panneau de détail : YAML, évènements, conteneurs, journaux, terminal.
///
/// Les journaux et la sortie du terminal sont stockés ici sous forme de données
/// brutes ; les composants de `widgets/` ne font que les rendre.
#[derive(Debug, Default)]
pub struct DetailState {
    /// Panneau ouvert ou replié.
    pub open: bool,
    /// Onglet affiché.
    pub tab: DetailTab,
    /// Objet observé.
    pub reference: Option<ResourceRef>,
    /// Manifeste reçu du cluster.
    pub yaml: String,
    /// Requête de lecture du manifeste en cours.
    pub yaml_pending: Option<RequestId>,
    /// Vrai si l'utilisateur a modifié le manifeste dans l'éditeur.
    pub yaml_edited: bool,
    /// Évènements de l'objet.
    pub events: Vec<EventSummary>,
    /// Requête de lecture des évènements en cours.
    pub events_pending: Option<RequestId>,
    /// Conteneurs du pod, conteneurs d'initialisation compris.
    pub containers: Vec<ContainerInfo>,
    /// Conteneur sélectionné pour les journaux et le terminal.
    pub container: Option<String>,
    /// Options de lecture des journaux.
    pub log_options: LogOptions,
    /// Lignes de journal reçues, les plus anciennes en tête.
    pub log_lines: VecDeque<String>,
    /// Filtre appliqué à l'affichage des journaux (sous-chaîne).
    pub log_filter: String,
    /// Retour à la ligne automatique dans le journal.
    pub log_wrap: bool,
    /// Défilement automatique vers la dernière ligne.
    pub log_stick_to_bottom: bool,
    /// Flux de journal en cours, le cas échéant.
    pub log_stream: Option<RequestId>,
    /// Nombre maximal de lignes conservées en mémoire.
    pub log_max_lines: usize,
    /// Session de terminal en cours, le cas échéant.
    pub exec_session: Option<RequestId>,
    /// Commande à lancer dans le conteneur.
    pub exec_command: String,
    /// Émulateur de terminal : grille, décodage ANSI en flux, taille courante
    /// et traduction des touches. La vue ne fait que l'afficher et transmettre
    /// au cluster ce qu'il produit.
    pub terminal: widgets::term::Terminal,
    /// Valeur saisie dans le formulaire de mise à l'échelle.
    pub scale_replicas: i32,
    /// Valeur saisie dans le formulaire de changement d'image.
    pub image: String,
}

impl DetailState {
    /// Ouvre le panneau sur un nouvel objet et vide les données de l'ancien.
    pub fn focus(&mut self, reference: ResourceRef) {
        let same = self.reference.as_ref() == Some(&reference);
        if !same {
            self.reset_data();
        }
        self.reference = Some(reference);
        self.open = true;
    }

    /// Ferme le panneau sans perdre les flux en cours (ils seront arrêtés par
    /// `app.rs`, qui seul sait dialoguer avec le backend).
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Vide tout ce qui dépend de l'objet observé.
    pub fn reset_data(&mut self) {
        self.yaml.clear();
        self.yaml_pending = None;
        self.yaml_edited = false;
        self.events.clear();
        self.events_pending = None;
        self.containers.clear();
        self.container = None;
        self.log_lines.clear();
        self.log_stream = None;
        self.exec_session = None;
        self.terminal.clear();
        self.scale_replicas = 1;
        self.image.clear();
    }

    /// Ajoute une ligne de journal en respectant la limite de mémoire.
    pub fn push_log(&mut self, line: String) {
        let max = self.log_max_lines.max(1);
        while self.log_lines.len() >= max {
            self.log_lines.pop_front();
        }
        self.log_lines.push_back(line);
    }

    /// Absorbe une trame de sortie du terminal distant.
    ///
    /// Le décodage et la borne mémoire sont l'affaire de l'émulateur : il
    /// reconstitue un caractère UTF-8 coupé entre deux trames et ne conserve
    /// qu'un nombre fixe de lignes d'historique.
    pub fn push_exec(&mut self, data: &[u8]) {
        self.terminal.feed(data);
    }
}

// ---------------------------------------------------------------------------
// Console YAML
// ---------------------------------------------------------------------------

/// État de la console YAML : le manifeste saisi et le résultat de la dernière
/// opération.
#[derive(Debug, Default)]
pub struct YamlConsoleState {
    /// Texte du manifeste en cours d'édition.
    pub source: String,
    /// Fichier d'origine, quand le manifeste vient du disque.
    pub path: Option<PathBuf>,
    /// Namespace appliqué aux objets qui n'en déclarent pas.
    pub namespace: Option<String>,
    /// Mode simulation (`--dry-run=server`).
    pub dry_run: bool,
    /// Force l'appropriation des champs en cas de conflit.
    pub force: bool,
    /// Compte rendu lisible de la dernière opération.
    pub output: String,
    /// Différences calculées par la dernière comparaison.
    pub diff: Vec<(ResourceRef, String)>,
    /// Bilan détaillé de la dernière application.
    pub outcome: Option<ApplyOutcome>,
    /// Opération en cours, le cas échéant.
    pub pending: Option<RequestId>,
}

// ---------------------------------------------------------------------------
// Catalogue (images, charts, déploiement guidé)
// ---------------------------------------------------------------------------

/// État du catalogue.
///
/// Les types d'écran (`HubTab`, `DeployForm`) appartiennent à la vue : c'est
/// elle qui les fait vivre, l'état ne fait que les porter d'une image à l'autre.
#[derive(Debug, Default)]
pub struct HubState {
    /// Onglet affiché.
    pub tab: HubTab,
    /// Registre interrogé.
    pub registry: RegistryKind,
    /// Terme de recherche d'images.
    pub image_query: String,
    /// Terme de recherche de charts.
    pub chart_query: String,
    /// Message affiché sous la barre de recherche (saisie vide, aucun résultat).
    pub message: Option<String>,
    /// Résultats de la recherche d'images.
    pub images: Vec<ImageSummary>,
    /// Image sélectionnée, référence complète.
    pub selected_image: Option<String>,
    /// Tags de l'image sélectionnée.
    pub tags: Vec<TagInfo>,
    /// Tag sélectionné.
    pub selected_tag: Option<String>,
    /// Fiche détaillée de l'image sélectionnée.
    pub details: Option<ImageDetails>,
    /// Résultats de la recherche de charts.
    pub charts: Vec<ChartSummary>,
    /// Applications du catalogue intégré.
    pub catalog: Vec<CatalogApp>,
    /// Vrai une fois le catalogue demandé, pour ne pas le redemander à chaque image.
    pub catalog_requested: bool,
    /// Formulaire du déploiement guidé.
    pub deploy: DeployForm,
}

// ---------------------------------------------------------------------------
// Mises à jour
// ---------------------------------------------------------------------------

/// État de l'écran des mises à jour.
#[derive(Debug, Default)]
pub struct UpdatesState {
    /// Onglet affiché.
    pub tab: UpdatesTab,
    /// Vrai une fois l'état persistant demandé au démarrage de la vue.
    pub loaded: bool,
    /// Surveillants configurés.
    pub watchers: Vec<WatcherSpec>,
    /// Constats en attente de décision.
    pub findings: Vec<UpdateFinding>,
    /// Historique des déploiements, le plus récent en tête.
    pub history: Vec<RolloutResult>,
    /// Images observées sur le cluster.
    pub workload_images: Vec<WorkloadImage>,
    /// Surveillants suggérés à partir des images observées.
    pub suggestions: Vec<WatcherSpec>,
    /// Cases cochées en face des suggestions, dans le même ordre.
    pub suggestion_keep: Vec<bool>,
    /// Namespace ciblé par l'inventaire (vide = tous).
    pub scan_namespace: String,
    /// Réglages du moteur, absents tant qu'ils n'ont pas été lus.
    pub settings: Option<UpdaterSettings>,
    /// Jeton GitHub saisi ; `None` = champ non modifié.
    pub github_token_input: Option<String>,
    /// Secret de webhook saisi ; `None` = champ non modifié.
    pub webhook_secret_input: Option<String>,
    /// Filtre textuel appliqué aux listes.
    pub filter: String,
    /// Formulaire de surveillance.
    pub form: WatcherForm,
    /// Demande de confirmation propre à cet écran.
    pub confirm: Option<UpdatesConfirm>,
}

// ---------------------------------------------------------------------------
// Réglages
// ---------------------------------------------------------------------------

/// Réglages de l'application et brouillons des formulaires de connexion.
#[derive(Debug)]
pub struct SettingsState {
    /// Onglet affiché.
    pub tab: SettingsTab,
    /// Manière d'ajouter un cluster.
    pub add_mode: AddClusterMode,
    /// Préférence de thème.
    pub theme: ThemeChoice,
    /// Densité d'affichage.
    pub density: Density,
    /// Vrai si le thème effectif est sombre ; tenu à jour par `app.rs`.
    pub dark: bool,
    /// Facteur d'échelle de l'interface.
    pub zoom: f32,
    /// Largeur du panneau de navigation.
    pub nav_width: f32,
    /// Taille de page demandée au serveur d'API.
    pub page_size: u32,
    /// Nombre de lignes de journal conservées.
    pub log_lines: usize,
    /// Demander confirmation avant toute opération destructrice.
    pub confirm_destructive: bool,
    /// Dernier message d'erreur d'un formulaire de connexion.
    pub error: Option<String>,
    /// Demande de confirmation propre à cet écran.
    pub confirm: Option<SettingsConfirm>,
    /// Contextes lus dans le kubeconfig sélectionné.
    pub contexts: Vec<ContextInfo>,
    /// Chemin du kubeconfig saisi ; vide = emplacement par défaut.
    pub kubeconfig_path: String,
    /// Contexte retenu dans la liste.
    pub selected_context: Option<String>,
    /// Importer tous les contextes plutôt que le seul contexte choisi.
    pub all_contexts: bool,
    /// Enregistrer la connexion sur le disque.
    pub persist: bool,
    /// Nom local du cluster à enregistrer.
    pub cluster_name: String,
    /// Kubeconfig collé dans la zone de texte.
    pub inline_yaml: String,
    /// Contexte à charger dans le kubeconfig collé.
    pub inline_context: String,
    /// URL du serveur d'API pour une connexion directe.
    pub remote_server: String,
    /// Jeton porteur pour une connexion directe.
    pub remote_token: String,
    /// Autorité de certification au format PEM.
    pub remote_ca_pem: String,
    /// Certificat client au format PEM.
    pub remote_client_cert_pem: String,
    /// Clé privée client au format PEM.
    pub remote_client_key_pem: String,
    /// Namespace par défaut de la connexion directe.
    pub remote_namespace: String,
    /// Désactive la vérification du certificat serveur.
    pub remote_insecure: bool,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            tab: SettingsTab::default(),
            add_mode: AddClusterMode::default(),
            theme: ThemeChoice::default(),
            density: Density::default(),
            dark: true,
            zoom: 1.0,
            nav_width: 208.0,
            page_size: DEFAULT_PAGE_SIZE,
            log_lines: DEFAULT_LOG_LINES,
            confirm_destructive: true,
            error: None,
            confirm: None,
            contexts: Vec::new(),
            kubeconfig_path: String::new(),
            selected_context: None,
            // L'import complet est le geste attendu : on récupère tous les
            // contextes du fichier en une fois.
            all_contexts: true,
            persist: true,
            cluster_name: String::new(),
            inline_yaml: String::new(),
            inline_context: String::new(),
            remote_server: String::new(),
            remote_token: String::new(),
            remote_ca_pem: String::new(),
            remote_client_cert_pem: String::new(),
            remote_client_key_pem: String::new(),
            remote_namespace: String::new(),
            remote_insecure: false,
        }
    }
}

// ---------------------------------------------------------------------------
// État global
// ---------------------------------------------------------------------------

/// État complet de l'application.
pub struct AppState {
    /// Écran affiché.
    pub view: View,
    /// Clusters enregistrés.
    pub clusters: Vec<ClusterInfo>,
    /// Cluster courant.
    pub current_cluster: Option<String>,
    /// Kinds découverts sur le cluster courant.
    pub kinds: Vec<ResourceKind>,
    /// Namespaces du cluster courant.
    pub namespaces: Vec<String>,
    /// Namespace filtré ; `None` = tous les namespaces.
    pub namespace: Option<String>,
    /// Kind affiché dans le tableau (nom pluriel, par exemple `pods`).
    pub selected_kind: String,
    /// Lignes du tableau.
    pub rows: Vec<ObjectSummary>,
    /// Filtre textuel appliqué au tableau.
    pub filter: String,
    /// Sélecteur de labels envoyé au serveur d'API.
    pub label_selector: String,
    /// Tri du tableau : (colonne, ascendant).
    pub sort: (usize, bool),
    /// Objet sélectionné dans le tableau.
    pub selected: Option<ResourceRef>,
    /// Panneau de détail.
    pub detail: DetailState,
    /// Topologie : instantané, graphe, disposition.
    pub graph: GraphState,
    /// Console YAML.
    pub yaml_console: YamlConsoleState,
    /// Catalogue.
    pub hub: HubState,
    /// Assistant de déploiement.
    pub wizard: WizardState,
    /// Mises à jour.
    pub updates: UpdatesState,
    /// Réglages.
    pub settings: SettingsState,
    /// Notifications éphémères.
    pub toasts: widgets::toast::Toasts,
    /// Requêtes en vol, de l'identifiant vers un libellé affichable.
    pub pending: IndexMap<RequestId, String>,
    /// Demande de confirmation affichée par-dessus l'interface.
    pub confirm: Option<widgets::confirm::Confirm>,
    /// Prochain identifiant de requête.
    pub next_id: RequestId,
    /// Rafraîchissement automatique de l'écran courant.
    pub auto_refresh: bool,
    /// Période du rafraîchissement automatique.
    pub refresh_every: Duration,
    /// Date du dernier rafraîchissement automatique.
    pub last_refresh: Instant,

    // --- Données d'écran, renseignées par les évènements du backend ---
    /// Synthèse du cluster courant.
    pub overview: Option<ClusterOverview>,
    /// Mesures des nœuds.
    pub metrics_nodes: Vec<MetricsSample>,
    /// Mesures des pods.
    pub metrics_pods: Vec<MetricsSample>,
    /// Évènements du namespace courant.
    pub events: Vec<EventSummary>,
    /// Jeton de pagination de la dernière page reçue.
    pub continue_token: Option<String>,
    /// Dernier message affiché dans la barre d'état.
    pub last_message: String,
}

impl AppState {
    /// Alloue un identifiant de requête.
    pub fn next_id(&mut self) -> RequestId {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }

    /// Nom du cluster courant.
    pub fn cluster(&self) -> Option<&str> {
        self.current_cluster.as_deref()
    }

    /// Alloue un identifiant et enregistre la requête comme étant en vol.
    ///
    /// Le libellé est affiché dans la barre d'état tant que la réponse n'est
    /// pas arrivée.
    pub fn begin(&mut self, label: impl Into<String>) -> RequestId {
        let id = self.next_id();
        self.pending.insert(id, label.into());
        id
    }

    /// Retire une requête de la liste des requêtes en vol.
    pub fn finish(&mut self, id: RequestId) -> Option<String> {
        self.pending.shift_remove(&id)
    }

    /// Vrai si au moins une requête est en vol.
    pub fn is_busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Fiche du cluster courant.
    pub fn cluster_info(&self) -> Option<&ClusterInfo> {
        let name = self.current_cluster.as_deref()?;
        self.clusters.iter().find(|c| c.name == name)
    }

    /// Cherche un kind par son nom pluriel (`pods`) ou complet (`pods.`).
    pub fn kind_by_plural(&self, plural: &str) -> Option<&ResourceKind> {
        let needle = plural.trim();
        self.kinds
            .iter()
            .find(|k| k.plural == needle || k.full_name() == needle)
    }

    /// Options de listing correspondant aux filtres courants.
    pub fn list_options(&self) -> ListOptions {
        let selector = self.label_selector.trim();
        ListOptions {
            namespace: self.namespace.clone(),
            label_selector: if selector.is_empty() {
                None
            } else {
                Some(selector.to_string())
            },
            field_selector: None,
            limit: Some(self.settings.page_size.max(1)),
            continue_token: None,
        }
    }

    /// Remet à zéro tout ce qui dépend du cluster courant.
    pub fn clear_cluster_data(&mut self) {
        self.kinds.clear();
        self.namespaces.clear();
        self.rows.clear();
        self.continue_token = None;
        self.selected = None;
        self.overview = None;
        self.metrics_nodes.clear();
        self.metrics_pods.clear();
        self.events.clear();
        self.detail.reset_data();
        self.detail.close();
        self.detail.reference = None;
        self.graph.clear();
    }
}

impl AppState {
    /// État de démarrage, avant restauration des préférences : aucun cluster,
    /// aucune requête en vol, tableau des pods.
    ///
    /// `Instant` n'a pas de valeur par défaut, d'où l'écriture explicite de
    /// tous les champs plutôt qu'un `#[derive(Default)]`.
    pub fn new() -> Self {
        Self {
            view: View::default(),
            clusters: Vec::new(),
            current_cluster: None,
            kinds: Vec::new(),
            namespaces: Vec::new(),
            namespace: None,
            selected_kind: DEFAULT_KIND.to_string(),
            rows: Vec::new(),
            filter: String::new(),
            label_selector: String::new(),
            sort: (0, true),
            selected: None,
            detail: DetailState {
                log_max_lines: DEFAULT_LOG_LINES,
                log_wrap: false,
                log_stick_to_bottom: true,
                scale_replicas: 1,
                // Taille de départ : elle est recalculée d'après la police et
                // la place disponible dès la première image du terminal.
                terminal: {
                    let mut t = widgets::term::Terminal::new();
                    t.set_size(120, 30);
                    t
                },
                exec_command: "/bin/sh".to_string(),
                ..DetailState::default()
            },
            graph: GraphState::default(),
            yaml_console: YamlConsoleState::default(),
            hub: HubState::default(),
            wizard: WizardState::default(),
            updates: UpdatesState::default(),
            settings: SettingsState::default(),
            toasts: widgets::toast::Toasts::default(),
            pending: IndexMap::new(),
            confirm: None,
            next_id: 0,
            auto_refresh: true,
            refresh_every: Duration::from_secs(10),
            last_refresh: Instant::now(),
            overview: None,
            metrics_nodes: Vec::new(),
            metrics_pods: Vec::new(),
            events: Vec::new(),
            continue_token: None,
            last_message: String::new(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiants_monotones() {
        let mut st = AppState::new();
        let a = st.next_id();
        let b = st.next_id();
        assert!(b > a, "les identifiants doivent croître");
    }

    #[test]
    fn requetes_en_vol() {
        let mut st = AppState::new();
        assert!(!st.is_busy());
        let id = st.begin("chargement des pods");
        assert!(st.is_busy());
        assert_eq!(st.finish(id).as_deref(), Some("chargement des pods"));
        assert!(!st.is_busy());
        // Une réponse obsolète ne retire rien.
        assert!(st.finish(id).is_none());
    }

    #[test]
    fn options_de_listing_sans_selecteur_vide() {
        let mut st = AppState::new();
        st.label_selector = "   ".to_string();
        assert!(st.list_options().label_selector.is_none());
        st.label_selector = "app=web".to_string();
        assert_eq!(st.list_options().label_selector.as_deref(), Some("app=web"));
    }

    #[test]
    fn journal_borne_en_memoire() {
        let mut d = DetailState {
            log_max_lines: 3,
            ..DetailState::default()
        };
        for i in 0..10 {
            d.push_log(format!("ligne {i}"));
        }
        assert_eq!(d.log_lines.len(), 3);
        assert_eq!(d.log_lines.front().map(String::as_str), Some("ligne 7"));
    }

    #[test]
    fn sortie_terminal_toujours_utf8() {
        let mut d = DetailState::default();
        // Séquence d'octets invalide en UTF-8 : elle ne doit pas paniquer.
        d.push_exec(&[0xff, 0xfe, b'o', b'k']);
        assert!(d.terminal.to_text().ends_with("ok"));
    }

    #[test]
    fn vues_et_index() {
        for v in View::ALL {
            assert_eq!(View::from_index(v.index()), v);
            assert!(!v.label().is_empty());
        }
        assert_eq!(View::from_index(99), View::Overview);
    }
}
