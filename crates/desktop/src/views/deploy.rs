//! Vue « Déployer » : l'assistant de déploiement en quatre étapes.
//!
//! Le formulaire du catalogue ([`crate::views::hub`]) expose les vingt champs
//! d'une [`DeployRequest`] : c'est ce qu'il faut pour un cas particulier, et
//! c'est trop pour déployer une application ordinaire. Cet écran pose les
//! quatre questions qui comptent — quoi, où, comment, combien — traduit les
//! réponses en manifestes, et montre ce que le déploiement coûtera au cluster
//! avant qu'il ne parte.
//!
//! Comme toutes les vues, celle-ci ne fait aucun appel réseau : elle lit
//! [`AppState`] et poste des [`Command`] au backend.

use kubewatch_core::apply::ApplyOutcome;
use kubewatch_core::model::ClusterOverview;
use kubewatch_hub::deploy::{self, DeployRequest, PortSpec, PvcSpec};
use kubewatch_hub::model::{CatalogApp, ImageDetails, ImageRef};
use kubewatch_hub::sizing::{self, Assessment, Axis, Fit, NoteLevel, SizeProfile};

use crate::backend::{Backend, Command};
use crate::icons;
use crate::state::AppState;
use crate::views::hub::{
    couleur_avertissement, couleur_erreur, couleur_info, couleur_ok, dispatch, is_busy,
    outcome_view, truncate,
};

// ---------------------------------------------------------------------------
// Libellés des requêtes en vol
// ---------------------------------------------------------------------------

/// Libellé de l'inspection d'image lancée par l'assistant.
pub const LBL_INSPECT: &str = "Lecture de l'image";

/// Libellé de la génération de manifeste lancée par l'assistant.
pub const LBL_RENDER: &str = "Préparation du déploiement";

/// Libellé de l'application des manifestes lancée par l'assistant.
///
/// `app.rs` s'en sert pour aiguiller un `Event::ApplyOutcome` vers cet écran
/// plutôt que vers la console YAML ou le formulaire du catalogue.
pub const LBL_DEPLOY: &str = "Déploiement guidé";

/// Libellé du chargement du catalogue embarqué.
const LBL_CATALOG: &str = "Chargement du catalogue";

/// Largeur d'une carte d'application dans la grille de l'étape 1.
const CARTE_APP: f32 = 236.0;

/// Largeur d'une carte de profil de taille.
const CARTE_TAILLE: f32 = 186.0;
/// Marges intérieures d'une carte, gauche et droite réunies (voir [`carte`]).
const CARTE_MARGES: f32 = 20.0;

/// Hauteur des barres de capacité.
const HAUTEUR_BARRE: f32 = 14.0;

// ---------------------------------------------------------------------------
// État de l'assistant
// ---------------------------------------------------------------------------

/// Les quatre étapes, dans l'ordre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
pub enum Step {
    /// Choisir l'application : catalogue ou image libre.
    #[default]
    What,
    /// Régler le nom, la taille, l'exposition et le stockage.
    How,
    /// Lire la prévision de consommation et le manifeste.
    Preview,
    /// Compte rendu de ce qui a été appliqué.
    Result,
}

impl Step {
    /// Les étapes dans l'ordre d'affichage.
    pub const ALL: [Step; 4] = [Step::What, Step::How, Step::Preview, Step::Result];

    /// Titre affiché dans le fil d'étapes.
    fn label(self) -> &'static str {
        match self {
            Step::What => "Quoi",
            Step::How => "Comment",
            Step::Preview => "Vérification",
            Step::Result => "Résultat",
        }
    }
}

/// D'où vient l'application à déployer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// Une entrée du catalogue embarqué.
    #[default]
    Catalog,
    /// Une référence d'image saisie à la main.
    Image,
}

/// Comment l'application sera jointe, une fois déployée.
///
/// Ces quatre choix couvrent les cas réels ; ils se traduisent en type de
/// Service et en Ingress, que l'assistant n'a pas besoin de nommer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Exposure {
    /// `ClusterIP` : joignable depuis le cluster seulement.
    #[default]
    Internal,
    /// `NodePort` : un port ouvert sur chaque nœud.
    NodePort,
    /// `LoadBalancer` : une adresse IP demandée à l'hébergeur.
    LoadBalancer,
    /// `ClusterIP` plus un Ingress sur un nom de domaine.
    Domain,
}

impl Exposure {
    /// Les quatre expositions, de la plus fermée à la plus ouverte.
    const ALL: [Exposure; 4] = [
        Exposure::Internal,
        Exposure::NodePort,
        Exposure::LoadBalancer,
        Exposure::Domain,
    ];

    /// Intitulé affiché.
    fn label(self) -> &'static str {
        match self {
            Exposure::Internal => "Interne au cluster",
            Exposure::NodePort => "Port sur les nœuds",
            Exposure::LoadBalancer => "Adresse IP publique",
            Exposure::Domain => "Nom de domaine",
        }
    }

    /// Ce que le choix implique, dit sans jargon.
    fn description(self) -> &'static str {
        match self {
            Exposure::Internal => {
                "Les autres applications du cluster y accèdent par son nom. Rien n'est ouvert \
                 vers l'extérieur."
            }
            Exposure::NodePort => {
                "Kubernetes ouvre le même port sur tous les nœuds. Pratique en local, rarement \
                 en production."
            }
            Exposure::LoadBalancer => {
                "L'hébergeur attribue une adresse IP. Sans contrôleur adapté, le Service reste \
                 « pending »."
            }
            Exposure::Domain => {
                "Un Ingress route un nom de domaine vers l'application. Il faut un contrôleur \
                 d'entrée sur le cluster."
            }
        }
    }

    /// Type de Service correspondant.
    fn service_type(self) -> &'static str {
        match self {
            Exposure::NodePort => "NodePort",
            Exposure::LoadBalancer => "LoadBalancer",
            Exposure::Internal | Exposure::Domain => "ClusterIP",
        }
    }

    /// Retrouve le choix inscrit dans une requête existante.
    fn detect(req: &DeployRequest) -> Exposure {
        let domaine = req
            .ingress_host
            .as_deref()
            .map(str::trim)
            .is_some_and(|h| !h.is_empty());
        if domaine {
            return Exposure::Domain;
        }
        match req.service_type.as_deref().map(str::trim) {
            Some("NodePort") => Exposure::NodePort,
            Some("LoadBalancer") => Exposure::LoadBalancer,
            _ => Exposure::Internal,
        }
    }
}

/// État complet de l'assistant.
///
/// `request` est la source de vérité : les champs qui l'entourent ne sont que
/// la forme sous laquelle l'utilisateur les manipule, et [`sync`] les y
/// reverse à chaque image.
#[derive(Debug)]
pub struct WizardState {
    /// Étape affichée.
    pub step: Step,
    /// Étape la plus avancée atteinte, qui borne le fil cliquable.
    pub reached: Step,
    /// Origine de l'application.
    pub source: Source,
    /// Filtre textuel de la grille du catalogue.
    pub filter: String,
    /// Catégorie retenue ; `None` = toutes.
    pub category: Option<String>,
    /// Identifiant de l'application choisie dans le catalogue.
    pub picked: Option<String>,
    /// Référence d'image saisie à la main.
    pub image_input: String,
    /// La requête en cours de construction.
    pub request: DeployRequest,
    /// Profil de taille retenu.
    pub profile: SizeProfile,
    /// Mode d'exposition retenu.
    pub exposure: Exposure,
    /// Nom de domaine, quand l'exposition est un Ingress.
    pub host: String,
    /// Un volume persistant est demandé.
    pub storage: bool,
    /// Taille du volume, en gibioctets.
    pub storage_gib: u32,
    /// Point de montage du volume.
    pub storage_path: String,
    /// Manifeste renvoyé par le backend.
    pub preview: Option<String>,
    /// Manifeste déplié sous la prévision.
    pub show_yaml: bool,
    /// Bilan de la dernière application.
    pub outcome: Option<ApplyOutcome>,
    /// Vrai si ce bilan vient d'une simulation.
    pub outcome_was_dry_run: bool,
    /// Message d'aide ou d'erreur affiché sous les actions.
    pub message: Option<String>,
    /// Dernière étape dessinée, qui sert à repérer l'entrée dans une étape.
    shown: Step,
}

impl Default for WizardState {
    fn default() -> Self {
        WizardState {
            step: Step::default(),
            reached: Step::default(),
            source: Source::default(),
            filter: String::new(),
            category: None,
            picked: None,
            image_input: String::new(),
            request: DeployRequest::default(),
            profile: SizeProfile::default(),
            exposure: Exposure::default(),
            host: String::new(),
            storage: false,
            storage_gib: 10,
            storage_path: "/data".to_string(),
            preview: None,
            show_yaml: false,
            outcome: None,
            outcome_was_dry_run: false,
            message: None,
            shown: Step::default(),
        }
    }
}

impl WizardState {
    /// Revient à la première étape et oublie l'application choisie.
    pub fn restart(&mut self) {
        let namespace = self.request.namespace.clone();
        *self = WizardState {
            request: DeployRequest {
                namespace,
                ..DeployRequest::default()
            },
            ..WizardState::default()
        };
    }

    /// Avance jusqu'à une étape, en retenant le plus loin atteint.
    fn go(&mut self, step: Step) {
        self.step = step;
        self.reached = self.reached.max(step);
    }

    /// Recompose la requête à partir des réglages de l'assistant.
    ///
    /// Appelée après chaque étape : plutôt que de tenir vingt champs à jour un
    /// par un, on les réécrit tous depuis la forme simplifiée. Un champ que
    /// l'assistant n'expose pas (`command`, `args`, `nodeSelector`…) garde la
    /// valeur qu'il avait, celle du formulaire avancé par exemple.
    fn sync(&mut self) {
        self.profile.apply_to(&mut self.request);

        self.request.service_type = Some(self.exposure.service_type().to_string());
        self.request.ingress_host = match self.exposure {
            Exposure::Domain => {
                let host = self.host.trim();
                (!host.is_empty()).then(|| host.to_string())
            }
            _ => None,
        };

        self.request.pvc = if self.storage {
            let existant = self.request.pvc.clone().unwrap_or_default();
            Some(PvcSpec {
                size: format!("{}Gi", self.storage_gib.max(1)),
                mount_path: {
                    let p = self.storage_path.trim();
                    if p.is_empty() {
                        "/data".to_string()
                    } else {
                        p.to_string()
                    }
                },
                ..existant
            })
        } else {
            None
        };
    }

    /// La requête nettoyée, telle qu'elle sera envoyée au cluster.
    fn built(&self) -> DeployRequest {
        let mut req = self.request.clone();
        req.name = req.name.trim().to_string();
        req.namespace = {
            let ns = req.namespace.trim();
            if ns.is_empty() {
                "default".to_string()
            } else {
                ns.to_string()
            }
        };
        req.image = req.image.trim().to_string();
        req.ports.retain(|p| p.container_port != 0);
        let env = std::mem::take(&mut req.env);
        req.env = env
            .into_iter()
            .filter(|(k, _)| !k.trim().is_empty())
            .map(|(k, v)| (k.trim().to_string(), v))
            .collect();
        req
    }
}

// ---------------------------------------------------------------------------
// Point d'entrée
// ---------------------------------------------------------------------------

/// Dessine l'assistant de déploiement.
pub fn show(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    // Le catalogue est embarqué dans le binaire et partagé avec la vue
    // « Catalogue » : une seule demande sert les deux écrans.
    if !st.hub.catalog_requested {
        st.hub.catalog_requested = true;
        dispatch(st, backend, LBL_CATALOG, |id| Command::HubCatalog { id });
    }

    egui::Panel::top("deploy-fil").show(ui, |ui| {
        ui.add_space(8.0);
        fil_etapes(ui, st);
        ui.add_space(8.0);
    });

    egui::Panel::bottom("deploy-actions").show(ui, |ui| {
        ui.add_space(6.0);
        barre_actions(ui, st, backend);
        ui.add_space(6.0);
    });

    // Le manifeste est demandé à chaque entrée dans l'étape de vérification,
    // quel que soit le chemin emprunté (bouton ou fil d'étapes), et une seule
    // fois : un rendu en échec ne relance donc pas de requête en boucle.
    if st.wizard.step == Step::Preview && st.wizard.shown != Step::Preview {
        demander_manifeste(st, backend);
    }
    st.wizard.shown = st.wizard.step;

    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match st.wizard.step {
                Step::What => etape_quoi(ui, st, backend),
                Step::How => etape_comment(ui, st),
                Step::Preview => etape_apercu(ui, st),
                Step::Result => etape_resultat(ui, st),
            });
    });

    st.wizard.sync();
}

/// Fil d'étapes cliquable : « ① Quoi → ② Comment → ③ Vérification → ④ Résultat ».
fn fil_etapes(ui: &mut egui::Ui, st: &mut AppState) {
    let courante = st.wizard.step;
    let atteinte = st.wizard.reached;
    let accent = couleur_info(ui);
    let mut aller: Option<Step> = None;

    ui.horizontal_wrapped(|ui| {
        for (rang, etape) in Step::ALL.into_iter().enumerate() {
            if rang > 0 {
                ui.label(egui::RichText::new("›").weak());
            }
            let numero = format!("{}", rang + 1);
            let texte = format!("{numero}. {}", etape.label());
            let accessible = etape <= atteinte;
            let mut riche = egui::RichText::new(texte);
            if etape == courante {
                riche = riche.strong().color(accent);
            } else if !accessible {
                riche = riche.weak();
            }
            let reponse = ui.add_enabled(
                accessible,
                egui::Button::selectable(etape == courante, riche),
            );
            if reponse.clicked() && etape != courante {
                aller = Some(etape);
            }
        }
    });

    if let Some(etape) = aller {
        st.wizard.step = etape;
    }
}

// ---------------------------------------------------------------------------
// Étape 1 — quoi déployer
// ---------------------------------------------------------------------------

fn etape_quoi(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    ui.add_space(4.0);
    ui.heading("Que voulez-vous déployer ?");
    ui.label(
        egui::RichText::new(
            "Choisissez une application prête à l'emploi, ou indiquez l'image de la vôtre.",
        )
        .weak(),
    );
    ui.add_space(10.0);

    ui.horizontal(|ui| {
        ui.selectable_value(&mut st.wizard.source, Source::Catalog, "  Catalogue  ");
        ui.selectable_value(&mut st.wizard.source, Source::Image, "  Mon image  ");
    });
    ui.add_space(10.0);

    match st.wizard.source {
        Source::Catalog => grille_catalogue(ui, st),
        Source::Image => champ_image(ui, st, backend),
    }
}

/// Grille des applications du catalogue, filtrable par texte et par catégorie.
fn grille_catalogue(ui: &mut egui::Ui, st: &mut AppState) {
    if st.hub.catalog.is_empty() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Chargement du catalogue…");
        });
        return;
    }

    let mut categories: Vec<String> = st.hub.catalog.iter().map(|a| a.category.clone()).collect();
    categories.sort();
    categories.dedup();

    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut st.wizard.filter)
                .hint_text("Filtrer : base de données, grafana…")
                .desired_width(240.0),
        );
        ui.separator();
        if ui
            .selectable_label(st.wizard.category.is_none(), "Toutes")
            .clicked()
        {
            st.wizard.category = None;
        }
        for categorie in &categories {
            let choisie = st.wizard.category.as_deref() == Some(categorie.as_str());
            if ui.selectable_label(choisie, categorie).clicked() {
                st.wizard.category = if choisie {
                    None
                } else {
                    Some(categorie.clone())
                };
            }
        }
    });
    ui.add_space(8.0);

    let filtre = st.wizard.filter.trim().to_lowercase();
    let categorie = st.wizard.category.clone();
    let retenues: Vec<CatalogApp> = st
        .hub
        .catalog
        .iter()
        .filter(|a| categorie.as_deref().is_none_or(|c| a.category == c))
        .filter(|a| {
            filtre.is_empty()
                || a.name.to_lowercase().contains(&filtre)
                || a.description.to_lowercase().contains(&filtre)
                || a.category.to_lowercase().contains(&filtre)
                || a.image.to_lowercase().contains(&filtre)
        })
        .cloned()
        .collect();

    if retenues.is_empty() {
        ui.add_space(10.0);
        ui.weak("Aucune application ne correspond à ce filtre.");
        return;
    }

    let choisie = st.wizard.picked.clone();
    let mut prise: Option<CatalogApp> = None;
    grille_de_cartes(ui, &retenues, CARTE_APP, |ui, app, largeur| {
        let selectionnee = choisie.as_deref() == Some(app.id.as_str());
        let reponse = carte(ui, selectionnee, largeur, |ui| {
            ui.horizontal(|ui| {
                pastille(ui, &app.name);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&app.name).strong());
                    ui.label(egui::RichText::new(&app.category).small().weak());
                });
            });
            ui.add_space(4.0);
            ui.label(egui::RichText::new(truncate(&app.description, 110)).small());
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(port) = app.default_port {
                    etiquette(ui, &format!("port {port}"));
                }
                if app.needs_pvc {
                    etiquette(ui, "volume");
                }
                if !app.env.is_empty() {
                    etiquette(ui, &format!("{} réglage(s)", app.env.len()));
                }
            });
        });
        if reponse.clicked() {
            prise = Some(app.clone());
        }
    });

    if let Some(app) = prise {
        prefill_from_catalog(&mut st.wizard, &app);
        st.wizard.go(Step::How);
    }
}

/// Saisie d'une référence d'image, avec lecture facultative de ses métadonnées.
fn champ_image(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let lecture = is_busy(st, LBL_INSPECT);
    let mut valider = false;
    let mut inspecter = false;

    ui.horizontal_wrapped(|ui| {
        let champ = ui.add(
            egui::TextEdit::singleline(&mut st.wizard.image_input)
                .hint_text("ghcr.io/mon-org/mon-app:1.4.0")
                .desired_width(340.0),
        );
        if champ.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            valider = true;
        }
        if ui
            .add_enabled(!lecture, egui::Button::new("Lire l'image"))
            .on_hover_text(
                "Interroge le registre pour récupérer les ports exposés et les variables \
                 d'environnement par défaut de l'image.",
            )
            .clicked()
        {
            inspecter = true;
        }
        if lecture {
            ui.spinner();
        }
    });

    let saisie = st.wizard.image_input.trim().to_string();
    ui.add_space(6.0);
    if saisie.is_empty() {
        ui.weak("Une référence complète : registre, dépôt et version.");
    } else {
        match ImageRef::parse(&saisie) {
            Ok(reference) => {
                ui.colored_label(
                    couleur_ok(ui),
                    format!("{} {}", icons::SUCCESS, reference.to_string_full()),
                );
            }
            Err(e) => {
                ui.colored_label(couleur_erreur(ui), format!("{} {e}", icons::WARNING));
            }
        }
    }

    if let Some(details) = details_de(st, &saisie) {
        ui.add_space(8.0);
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Ce que dit l'image").strong());
                ui.add_space(4.0);
                let ports = if details.exposed_ports.is_empty() {
                    "aucun port déclaré".to_string()
                } else {
                    details
                        .exposed_ports
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                ui.label(format!("Ports : {ports}"));
                ui.label(format!("Variables par défaut : {}", details.env.len()));
                if let Some(arch) = &details.architecture {
                    ui.label(format!(
                        "Plate-forme : {}/{arch}",
                        details.os.as_deref().unwrap_or("linux")
                    ));
                }
            });
    }

    ui.add_space(10.0);
    let utilisable = ImageRef::parse(&saisie).is_ok();
    if ui
        .add_enabled(utilisable, egui::Button::new("Continuer avec cette image"))
        .clicked()
    {
        valider = true;
    }

    if inspecter && !saisie.is_empty() {
        let image = saisie.clone();
        dispatch(st, backend, LBL_INSPECT, move |id| Command::HubInspect {
            id,
            image,
        });
    }
    if valider && utilisable {
        let details = details_de(st, &saisie);
        let ports: Vec<u16> = details
            .as_ref()
            .map(|d| d.exposed_ports.clone())
            .unwrap_or_default();
        prefill_from_image(&mut st.wizard, &saisie, &ports);
        st.wizard.go(Step::How);
    }
}

/// Les métadonnées de l'image saisie, si la dernière inspection les concerne.
///
/// `HubState::details` porte la dernière image inspectée, quel que soit l'écran
/// qui l'a demandée : on ne s'en sert que si sa référence canonique est bien
/// celle qui est dans le champ.
fn details_de(st: &AppState, saisie: &str) -> Option<ImageDetails> {
    let attendue = ImageRef::parse(saisie).ok()?.to_string_full();
    let details = st.hub.details.as_ref()?;
    (details.reference == attendue).then(|| details.clone())
}

// ---------------------------------------------------------------------------
// Étape 2 — comment le déployer
// ---------------------------------------------------------------------------

fn etape_comment(ui: &mut egui::Ui, st: &mut AppState) {
    ui.add_space(4.0);
    ui.heading("Comment le déployer ?");
    ui.label(
        egui::RichText::new("Les valeurs proposées conviennent dans la plupart des cas.").weak(),
    );
    ui.add_space(10.0);

    bloc_identite(ui, st);
    ui.add_space(12.0);
    bloc_taille(ui, st);
    ui.add_space(12.0);
    bloc_exposition(ui, st);
    ui.add_space(12.0);
    bloc_stockage(ui, st);
    ui.add_space(12.0);
    bloc_reglages(ui, st);
    ui.add_space(12.0);

    // Tout ce que l'assistant n'expose pas — commande, arguments, placement,
    // secrets de registre — vit dans le formulaire du catalogue. Le passage s'y
    // fait avec les réglages déjà saisis.
    ui.horizontal(|ui| {
        if ui
            .button("Réglages avancés…")
            .on_hover_text(
                "Reprend ce déploiement dans le formulaire complet du catalogue : commande, \
                 arguments, placement sur les nœuds, secrets de registre.",
            )
            .clicked()
        {
            st.hub.deploy.request = st.wizard.built();
            st.hub.deploy.node_selector = st
                .wizard
                .request
                .node_selector
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            st.hub.deploy.preview = None;
            st.hub.deploy.outcome = None;
            st.hub.deploy.open = true;
            st.hub.tab = crate::views::hub::HubTab::Catalog;
            st.view = crate::state::View::Hub;
        }
    });
    ui.add_space(12.0);
}

/// Nom, namespace et nombre de répliques.
fn bloc_identite(ui: &mut egui::Ui, st: &mut AppState) {
    let namespaces = st.namespaces.clone();
    section(ui, "Identité", |ui| {
        egui::Grid::new("deploy-identite-assistant")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let w = &mut st.wizard;
                ui.label("Nom");
                ui.vertical(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut w.request.name)
                            .hint_text("mon-api")
                            .desired_width(240.0),
                    );
                    if !deploy::is_dns1123_label(w.request.name.trim()) {
                        ui.label(
                            egui::RichText::new(
                                "Minuscules, chiffres et tirets : « mon-api », « site-vitrine ».",
                            )
                            .small()
                            .color(couleur_avertissement(ui)),
                        );
                    }
                });
                ui.end_row();

                ui.label("Namespace");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut w.request.namespace)
                            .hint_text("default")
                            .desired_width(160.0),
                    );
                    if !namespaces.is_empty() {
                        egui::ComboBox::from_id_salt("deploy-ns")
                            .selected_text("Existants")
                            .width(140.0)
                            .show_ui(ui, |ui| {
                                for ns in &namespaces {
                                    if ui.selectable_label(false, ns).clicked() {
                                        w.request.namespace = ns.clone();
                                    }
                                }
                            });
                    }
                });
                ui.end_row();

                ui.label("Image");
                ui.label(egui::RichText::new(w.request.image.clone()).monospace());
                ui.end_row();

                ui.label("Répliques");
                ui.horizontal(|ui| {
                    // Le curseur borne la valeur qu'il affiche : au-delà de dix
                    // répliques (venues du formulaire avancé), un compteur les
                    // préserve au lieu de les rogner en silence.
                    if w.request.replicas <= 10 {
                        ui.add(egui::Slider::new(&mut w.request.replicas, 0..=10));
                    } else {
                        ui.add(
                            egui::DragValue::new(&mut w.request.replicas)
                                .range(0..=1000)
                                .speed(0.2),
                        );
                    }
                    let texte = match w.request.replicas {
                        0 => "aucun pod ne démarrera",
                        1 => "une copie de l'application",
                        _ => "copies réparties sur les nœuds",
                    };
                    ui.label(egui::RichText::new(texte).small().weak());
                });
                ui.end_row();
            });
    });
}

/// Choix du profil de taille, sous forme de cartes chiffrées.
fn bloc_taille(ui: &mut egui::Ui, st: &mut AppState) {
    section(ui, "Taille", |ui| {
        ui.label(
            egui::RichText::new(
                "La demande est réservée par l'ordonnanceur ; la limite est le plafond que \
                 l'application ne dépassera pas.",
            )
            .small()
            .weak(),
        );
        ui.add_space(8.0);

        let actuel = st.wizard.profile;
        let mut choisi: Option<SizeProfile> = None;
        grille_de_cartes(
            ui,
            &SizeProfile::PRESETS,
            CARTE_TAILLE,
            |ui, profil, largeur| {
                let profil = *profil;
                let r = profil
                    .resources()
                    .expect("un préréglage porte ses quantités");
                let reponse = carte(ui, profil == actuel, largeur, |ui| {
                    ui.label(egui::RichText::new(profil.label()).strong());
                    ui.label(
                        egui::RichText::new(format!(
                            "{} CPU · {}",
                            r.cpu_request, r.memory_request
                        ))
                        .monospace()
                        .small(),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "jusqu'à {} CPU · {}",
                            r.cpu_limit, r.memory_limit
                        ))
                        .small()
                        .weak(),
                    );
                    ui.add_space(3.0);
                    ui.label(egui::RichText::new(profil.description()).small().weak());
                });
                if reponse.clicked() {
                    choisi = Some(profil);
                }
            },
        );
        if let Some(profil) = choisi {
            st.wizard.profile = profil;
        }
        if actuel == SizeProfile::Custom {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Quantités personnalisées, conservées telles quelles. Choisir un profil \
                     ci-dessus les remplacera.",
                )
                .small()
                .color(couleur_info(ui)),
            );
        }
    });
}

/// Choix de l'exposition réseau et des ports.
fn bloc_exposition(ui: &mut egui::Ui, st: &mut AppState) {
    section(ui, "Accès", |ui| {
        let actuelle = st.wizard.exposure;
        for mode in Exposure::ALL {
            ui.horizontal(|ui| {
                if ui.radio(mode == actuelle, mode.label()).clicked() {
                    st.wizard.exposure = mode;
                }
            });
            ui.label(egui::RichText::new(mode.description()).small().weak());
            ui.add_space(2.0);
        }

        if st.wizard.exposure == Exposure::Domain {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Domaine");
                ui.add(
                    egui::TextEdit::singleline(&mut st.wizard.host)
                        .hint_text("mon-app.exemple.fr")
                        .desired_width(240.0),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Classe d'Ingress");
                let mut classe = st.wizard.request.ingress_class.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut classe)
                            .hint_text("nginx, traefik… (facultatif)")
                            .desired_width(240.0),
                    )
                    .changed()
                {
                    st.wizard.request.ingress_class = (!classe.trim().is_empty()).then_some(classe);
                }
            });
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Ports").strong());
        let mut retirer: Option<usize> = None;
        for (index, port) in st.wizard.request.ports.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label("Le conteneur écoute sur");
                ui.add(
                    egui::DragValue::new(&mut port.container_port)
                        .range(1..=65535)
                        .speed(1.0),
                );
                if ui
                    .small_button(icons::CLOSE)
                    .on_hover_text("Retirer ce port")
                    .clicked()
                {
                    retirer = Some(index);
                }
            });
        }
        if let Some(index) = retirer {
            st.wizard.request.ports.remove(index);
        }
        if ui.small_button("+ Ajouter un port").clicked() {
            st.wizard.request.ports.push(PortSpec {
                container_port: 8080,
                ..PortSpec::default()
            });
        }
        if st.wizard.request.ports.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Sans port, aucun Service n'est créé : l'application ne sera pas joignable.",
                )
                .small()
                .color(couleur_avertissement(ui)),
            );
        }
    });
}

/// Volume persistant.
fn bloc_stockage(ui: &mut egui::Ui, st: &mut AppState) {
    section(ui, "Stockage", |ui| {
        ui.checkbox(
            &mut st.wizard.storage,
            "Conserver les données entre les redémarrages",
        );
        ui.label(
            egui::RichText::new(
                "Sans volume, tout ce que l'application écrit disparaît à chaque redémarrage du \
                 pod.",
            )
            .small()
            .weak(),
        );
        if !st.wizard.storage {
            return;
        }
        ui.add_space(6.0);
        egui::Grid::new("deploy-stockage")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Taille");
                ui.add(
                    egui::Slider::new(&mut st.wizard.storage_gib, 1..=500)
                        .suffix(" Gio")
                        .logarithmic(true),
                );
                ui.end_row();

                ui.label("Monté dans");
                ui.add(
                    egui::TextEdit::singleline(&mut st.wizard.storage_path)
                        .hint_text("/data")
                        .desired_width(240.0),
                );
                ui.end_row();

                ui.label("Classe de stockage");
                let mut classe = st
                    .wizard
                    .request
                    .pvc
                    .as_ref()
                    .and_then(|p| p.storage_class.clone())
                    .unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut classe)
                            .hint_text("celle du cluster (facultatif)")
                            .desired_width(240.0),
                    )
                    .changed()
                {
                    if let Some(pvc) = st.wizard.request.pvc.as_mut() {
                        pvc.storage_class = (!classe.trim().is_empty()).then_some(classe);
                    }
                }
                ui.end_row();
            });
    });
}

/// Variables d'environnement, présentées comme les réglages de l'application.
fn bloc_reglages(ui: &mut egui::Ui, st: &mut AppState) {
    section(ui, "Réglages de l'application", |ui| {
        if st.wizard.request.env.is_empty() {
            ui.label(
                egui::RichText::new("Aucune variable d'environnement.")
                    .small()
                    .weak(),
            );
        }
        let mut retirer: Option<usize> = None;
        egui::Grid::new("deploy-env")
            .num_columns(3)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                for (index, (cle, valeur)) in st.wizard.request.env.iter_mut().enumerate() {
                    ui.add(
                        egui::TextEdit::singleline(cle)
                            .hint_text("CLÉ")
                            .desired_width(200.0),
                    );
                    ui.add(
                        egui::TextEdit::singleline(valeur)
                            .hint_text("valeur")
                            .desired_width(260.0),
                    );
                    if ui.small_button(icons::CLOSE).clicked() {
                        retirer = Some(index);
                    }
                    ui.end_row();
                }
            });
        if let Some(index) = retirer {
            st.wizard.request.env.remove(index);
        }
        if ui.small_button("+ Ajouter une variable").clicked() {
            st.wizard.request.env.push((String::new(), String::new()));
        }
    });
}

// ---------------------------------------------------------------------------
// Étape 3 — vérification
// ---------------------------------------------------------------------------

fn etape_apercu(ui: &mut egui::Ui, st: &mut AppState) {
    let requete = st.wizard.built();
    let erreur = deploy::validate(&requete).err();
    let bilan = sizing::assess(&requete, st.overview.as_ref());

    ui.add_space(4.0);
    ui.heading("Avant de déployer");
    ui.add_space(10.0);

    if let Some(e) = &erreur {
        egui::Frame::group(ui.style())
            .fill(couleur_erreur(ui).linear_multiply(0.12))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.colored_label(couleur_erreur(ui), format!("{} {e}", icons::WARNING));
                ui.label(
                    egui::RichText::new("Revenez à l'étape précédente pour corriger.")
                        .small()
                        .weak(),
                );
            });
        ui.add_space(10.0);
    }

    resume(ui, &requete);
    ui.add_space(12.0);
    consommation(ui, st.overview.as_ref(), &bilan);
    ui.add_space(12.0);
    remarques(ui, &bilan);
    ui.add_space(12.0);
    manifeste(ui, st);
    ui.add_space(12.0);
}

/// « Ce qui sera créé » : la liste des objets Kubernetes, en clair.
fn resume(ui: &mut egui::Ui, req: &DeployRequest) {
    section(ui, "Ce qui sera créé", |ui| {
        let nom = if req.name.is_empty() {
            "—"
        } else {
            &req.name
        };
        ligne_objet(
            ui,
            "Deployment",
            nom,
            &format!("{} pod(s) de {}", req.replicas.max(0), req.image),
        );
        if !req.ports.is_empty() {
            let ports: Vec<String> = req
                .ports
                .iter()
                .map(|p| p.container_port.to_string())
                .collect();
            ligne_objet(
                ui,
                "Service",
                nom,
                &format!(
                    "{}, port(s) {}",
                    req.service_type.as_deref().unwrap_or("ClusterIP"),
                    ports.join(", ")
                ),
            );
        }
        if let Some(host) = req.ingress_host.as_deref().filter(|h| !h.trim().is_empty()) {
            ligne_objet(ui, "Ingress", nom, &format!("https://{host}"));
        }
        if let Some(pvc) = &req.pvc {
            ligne_objet(
                ui,
                "PersistentVolumeClaim",
                &format!("{nom}-data"),
                &format!("{} monté dans {}", pvc.size, pvc.mount_path),
            );
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!("Dans le namespace « {} ».", req.namespace))
                .small()
                .weak(),
        );
    });
}

/// Une ligne de la liste des objets créés.
fn ligne_objet(ui: &mut egui::Ui, kind: &str, nom: &str, detail: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("＋").color(couleur_ok(ui)));
        ui.label(egui::RichText::new(kind).strong());
        ui.label(egui::RichText::new(nom).monospace());
        ui.label(egui::RichText::new(detail).small().weak());
    });
}

/// Aperçu de la consommation : barres avant/après sur chaque axe.
fn consommation(ui: &mut egui::Ui, overview: Option<&ClusterOverview>, bilan: &Assessment) {
    section(ui, "Consommation prévue", |ui| {
        let couleur = couleur_du_verdict(ui, bilan.fit);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(glyphe_du_verdict(bilan.fit))
                    .color(couleur)
                    .size(16.0),
            );
            ui.label(
                egui::RichText::new(bilan.fit.headline())
                    .strong()
                    .color(couleur),
            );
        });
        ui.add_space(8.0);

        match (bilan.cpu, bilan.memory) {
            (None, None) => {
                let raison = match overview {
                    None => {
                        "Le cluster n'a pas encore répondu : ouvrez la vue d'ensemble ou \
                         rafraîchissez (Ctrl+R)."
                    }
                    Some(o) if !o.metrics_available => {
                        // L'API agrégée peut être enregistrée et pourtant muette :
                        // metrics-server arrêté, mis à zéro réplique ou pas encore
                        // prêt répondent tous « 503 ». Le message ne présume donc
                        // pas de la cause.
                        "Les mesures de consommation ne sont pas disponibles sur ce cluster \
                         (metrics-server absent, arrêté ou non prêt). Le déploiement reste \
                         possible."
                    }
                    Some(_) => "Capacité du cluster inconnue.",
                };
                ui.label(egui::RichText::new(raison).small().weak());
            }
            _ => {
                if let Some(axe) = bilan.cpu {
                    axe_capacite(ui, "Processeur", &axe, bilan.fit, crate::format::cpu);
                    ui.add_space(10.0);
                }
                if let Some(axe) = bilan.memory {
                    axe_capacite(ui, "Mémoire", &axe, bilan.fit, |v| {
                        crate::format::bytes(v as i64)
                    });
                    ui.add_space(10.0);
                }
                ui.label(
                    egui::RichText::new(
                        "La part occupée est la consommation mesurée du cluster ; la part ajoutée \
                         est ce que ce déploiement réserve. Les réservations déjà faites par les \
                         autres applications ne sont pas visibles dans cette mesure.",
                    )
                    .small()
                    .weak(),
                );
            }
        }

        // Le stockage ne se compare à rien : le cluster n'annonce pas la place
        // restante sur ses volumes. On l'affiche donc comme une simple demande.
        if let Some(octets) = bilan.footprint.storage_bytes {
            ui.add_space(6.0);
            ui.label(format!(
                "Disque : {} demandés à la classe de stockage du cluster.",
                crate::format::bytes(octets)
            ));
        }
    });
}

/// Une barre « avant / ajouté / libre » avec ses chiffres.
fn axe_capacite(
    ui: &mut egui::Ui,
    titre: &str,
    axe: &Axis,
    verdict: Fit,
    format: impl Fn(f64) -> String,
) {
    let largeur = (ui.available_width() - 8.0).max(160.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(titre).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let reste = axe.remaining();
            let texte = if reste >= 0.0 {
                format!("{} libres après", format(reste))
            } else {
                format!("{} de trop", format(-reste))
            };
            ui.label(egui::RichText::new(texte).small().weak());
        });
    });
    ui.add_space(3.0);

    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(largeur, HAUTEUR_BARRE), egui::Sense::hover());
    let peintre = ui.painter();
    let arrondi = egui::CornerRadius::same(3);
    peintre.rect_filled(rect, arrondi, ui.visuals().extreme_bg_color);

    let avant = axe.before_fraction();
    let apres = axe.after_fraction();
    let couleur_avant = ui.visuals().weak_text_color().linear_multiply(0.8);
    let couleur_ajout = couleur_du_verdict(ui, verdict);

    // La part ajoutée est peinte d'abord, la part déjà occupée par-dessus : les
    // deux segments se lisent alors de gauche à droite sans calcul d'offset.
    if apres > 0.0 {
        let mut ajout = rect;
        ajout.set_right(rect.left() + rect.width() * apres);
        peintre.rect_filled(ajout, arrondi, couleur_ajout);
    }
    if avant > 0.0 {
        let mut occupe = rect;
        occupe.set_right(rect.left() + rect.width() * avant);
        peintre.rect_filled(occupe, arrondi, couleur_avant);
    }

    ui.add_space(3.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(format!("{} occupés", format(axe.used)))
                .small()
                .color(couleur_avant),
        );
        ui.label(
            egui::RichText::new(format!("+ {} réservés", format(axe.added)))
                .small()
                .color(couleur_ajout),
        );
        ui.label(
            egui::RichText::new(format!(
                "sur {} — {:.0} % après déploiement",
                format(axe.capacity),
                axe.after_ratio() * 100.0
            ))
            .small()
            .weak(),
        );
    });
}

/// Les remarques de la prévision, la plus grave en tête.
fn remarques(ui: &mut egui::Ui, bilan: &Assessment) {
    if bilan.notes.is_empty() {
        return;
    }
    section(ui, "À savoir", |ui| {
        for note in &bilan.notes {
            let (glyphe, couleur) = match note.level {
                NoteLevel::Danger => (icons::BLOCKED, couleur_erreur(ui)),
                NoteLevel::Warning => (icons::WARNING, couleur_avertissement(ui)),
                NoteLevel::Info => (icons::INFO, couleur_info(ui)),
            };
            ui.horizontal_top(|ui| {
                ui.label(egui::RichText::new(glyphe).color(couleur));
                ui.label(&note.text);
            });
            ui.add_space(3.0);
        }
    });
}

/// Manifeste généré, replié par défaut.
fn manifeste(ui: &mut egui::Ui, st: &mut AppState) {
    let en_cours = is_busy(st, LBL_RENDER);
    let mut ouvert = st.wizard.show_yaml;
    let reponse = egui::CollapsingHeader::new("Manifeste Kubernetes")
        .open(Some(ouvert))
        .show(ui, |ui| match st.wizard.preview.clone() {
            Some(yaml) => {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Lecture seule").small().weak());
                    if ui.small_button("Copier").clicked() {
                        ui.ctx().copy_text(yaml.clone());
                    }
                });
                let mut tampon = yaml;
                egui::ScrollArea::vertical()
                    .id_salt("deploy-yaml")
                    .max_height(320.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut tampon)
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    });
            }
            None if en_cours => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Génération en cours…");
                });
            }
            None => {
                // Ni manifeste ni requête en vol : soit la requête est encore
                // incomplète — l'erreur est affichée plus haut —, soit la
                // génération a échoué et le message est parti en notification.
                ui.label(
                    egui::RichText::new(
                        "Aucun manifeste : corrigez les réglages signalés, puis revenez ici.",
                    )
                    .small()
                    .weak(),
                );
            }
        });
    if reponse.header_response.clicked() {
        ouvert = !ouvert;
    }
    st.wizard.show_yaml = ouvert;
}

// ---------------------------------------------------------------------------
// Étape 4 — résultat
// ---------------------------------------------------------------------------

fn etape_resultat(ui: &mut egui::Ui, st: &mut AppState) {
    ui.add_space(4.0);
    let Some(outcome) = st.wizard.outcome.clone() else {
        ui.heading("Déploiement en cours…");
        ui.add_space(8.0);
        ui.spinner();
        return;
    };

    let simulation = st.wizard.outcome_was_dry_run;
    let reussi = outcome.failed == 0;
    let couleur = if reussi {
        couleur_ok(ui)
    } else {
        couleur_erreur(ui)
    };
    let titre = match (simulation, reussi) {
        (true, true) => "La simulation est passée",
        (true, false) => "La simulation a échoué",
        (false, true) => "Application déployée",
        (false, false) => "Le déploiement a échoué",
    };
    ui.heading(egui::RichText::new(titre).color(couleur));
    if simulation && reussi {
        ui.label(
            egui::RichText::new(
                "Le serveur d'API a accepté les manifestes sans rien écrire. Vous pouvez déployer \
                 pour de bon.",
            )
            .weak(),
        );
    }
    ui.add_space(10.0);
    outcome_view(ui, &outcome);
}

// ---------------------------------------------------------------------------
// Barre d'actions
// ---------------------------------------------------------------------------

fn barre_actions(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let etape = st.wizard.step;
    let cluster = st.cluster().map(str::to_string);
    let requete = st.wizard.built();
    let valide = deploy::validate(&requete).is_ok();
    let travail = is_busy(st, LBL_RENDER) || is_busy(st, LBL_DEPLOY);

    let mut aller: Option<Step> = None;
    let mut simuler = false;
    let mut deployer = false;
    let mut recommencer = false;

    ui.horizontal_wrapped(|ui| {
        match etape {
            Step::What => {}
            Step::How => {
                if ui
                    .button(format!("{} Changer d'application", icons::BACK))
                    .clicked()
                {
                    aller = Some(Step::What);
                }
            }
            Step::Preview | Step::Result => {
                if ui
                    .button(format!("{} Modifier les réglages", icons::BACK))
                    .clicked()
                {
                    aller = Some(Step::How);
                }
            }
        }

        match etape {
            Step::What => {
                ui.label(
                    egui::RichText::new("Choisissez une application pour continuer.")
                        .small()
                        .weak(),
                );
            }
            Step::How => {
                let pret = !requete.name.trim().is_empty() && !requete.image.trim().is_empty();
                if ui
                    .add_enabled(pret, egui::Button::new("Vérifier →"))
                    .clicked()
                {
                    aller = Some(Step::Preview);
                }
            }
            Step::Preview => {
                if ui
                    .add_enabled(
                        valide && cluster.is_some() && !travail,
                        egui::Button::new("Déployer"),
                    )
                    .clicked()
                {
                    deployer = true;
                }
                if ui
                    .add_enabled(
                        valide && cluster.is_some() && !travail,
                        egui::Button::new("Simuler d'abord"),
                    )
                    .on_hover_text(
                        "Envoie les manifestes au serveur d'API en mode « dry-run » : il les \
                         valide sans rien écrire.",
                    )
                    .clicked()
                {
                    simuler = true;
                }
                if travail {
                    ui.spinner();
                }
            }
            Step::Result => {
                if st.wizard.outcome_was_dry_run
                    && ui
                        .add_enabled(
                            cluster.is_some() && !travail,
                            egui::Button::new("Déployer pour de bon"),
                        )
                        .clicked()
                {
                    deployer = true;
                }
                if ui.button("Déployer autre chose").clicked() {
                    recommencer = true;
                }
                if travail {
                    ui.spinner();
                }
            }
        }

        if cluster.is_none() {
            ui.label(
                egui::RichText::new("Aucun cluster sélectionné.")
                    .small()
                    .color(couleur_avertissement(ui)),
            );
        }
    });

    if let Some(message) = st.wizard.message.clone() {
        ui.label(egui::RichText::new(message).small().weak());
    }

    // --- suites à donner, hors des fermetures de dessin
    if let Some(etape) = aller {
        st.wizard.go(etape);
    }
    if recommencer {
        st.wizard.restart();
    }
    if simuler || deployer {
        if let Some(nom_cluster) = cluster {
            let dry_run = simuler;
            st.wizard.outcome = None;
            st.wizard.outcome_was_dry_run = dry_run;
            st.wizard.go(Step::Result);
            let requete = requete.clone();
            dispatch(st, backend, LBL_DEPLOY, move |id| Command::HubDeploy {
                id,
                cluster: nom_cluster,
                request: Box::new(requete),
                dry_run,
            });
        }
    }
}

/// Demande au backend le manifeste correspondant à la requête courante.
pub fn demander_manifeste(st: &mut AppState, backend: &Backend) {
    let requete = st.wizard.built();
    if deploy::validate(&requete).is_err() {
        st.wizard.preview = None;
        return;
    }
    st.wizard.preview = None;
    dispatch(st, backend, LBL_RENDER, move |id| Command::HubRender {
        id,
        request: Box::new(requete),
    });
}

// ---------------------------------------------------------------------------
// Pré-remplissage
// ---------------------------------------------------------------------------

/// Prépare l'assistant pour une application du catalogue.
pub fn prefill_from_catalog(w: &mut WizardState, app: &CatalogApp) {
    let namespace = w.request.namespace.clone();
    w.picked = Some(app.id.clone());
    w.image_input = app.image.clone();
    w.profile = sizing::recommended_profile(app);
    w.exposure = Exposure::Internal;
    w.host.clear();
    w.storage = app.needs_pvc;
    w.storage_gib = 10;
    w.storage_path = "/data".to_string();
    w.preview = None;
    w.outcome = None;
    w.message = None;
    w.request = DeployRequest {
        name: sanitize_name(&app.id),
        namespace,
        image: app.image.clone(),
        replicas: 1,
        ports: app
            .default_port
            .map(|p| {
                vec![PortSpec {
                    container_port: p,
                    ..PortSpec::default()
                }]
            })
            .unwrap_or_default(),
        env: app.env.clone(),
        ..DeployRequest::default()
    };
    w.sync();
}

/// Prépare l'assistant pour une image saisie à la main.
pub fn prefill_from_image(w: &mut WizardState, reference: &str, ports: &[u16]) {
    let namespace = w.request.namespace.clone();
    w.picked = None;
    w.profile = SizeProfile::Small;
    w.exposure = Exposure::Internal;
    w.host.clear();
    w.storage = false;
    w.preview = None;
    w.outcome = None;
    w.message = None;
    w.request = DeployRequest {
        name: sanitize_name(nom_court(reference)),
        namespace,
        image: reference.to_string(),
        replicas: 1,
        ports: ports
            .iter()
            .map(|p| PortSpec {
                container_port: *p,
                ..PortSpec::default()
            })
            .collect(),
        ..DeployRequest::default()
    };
    w.sync();
}

/// Recharge l'assistant depuis une requête existante, réglages compris.
///
/// Sert quand l'utilisateur arrive du formulaire avancé : les champs que
/// l'assistant expose sont déduits de la requête, les autres sont conservés.
pub fn adopt(w: &mut WizardState, req: DeployRequest) {
    w.profile = SizeProfile::detect(&req);
    w.exposure = Exposure::detect(&req);
    w.host = req.ingress_host.clone().unwrap_or_default();
    w.storage = req.pvc.is_some();
    if let Some(pvc) = &req.pvc {
        w.storage_gib = sizing::gib_of(&pvc.size).unwrap_or(10);
        w.storage_path = pvc.mount_path.clone();
    }
    w.image_input = req.image.clone();
    w.request = req;
    w.preview = None;
    w.outcome = None;
}

/// `ghcr.io/o/depot:1.2` → `depot`.
fn nom_court(reference: &str) -> &str {
    let sans_digest = reference.split('@').next().unwrap_or(reference);
    let dernier = sans_digest.rsplit('/').next().unwrap_or(sans_digest);
    dernier.split(':').next().unwrap_or(dernier)
}

/// Ramène un texte quelconque sur une étiquette DNS-1123 utilisable comme nom.
fn sanitize_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(63).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "application".to_string()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// Briques d'affichage
// ---------------------------------------------------------------------------

/// Bloc encadré avec un titre.
fn section(ui: &mut egui::Ui, titre: &str, contenu: impl FnOnce(&mut egui::Ui)) {
    ui.label(egui::RichText::new(titre).strong().size(15.0));
    ui.add_space(4.0);
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            let largeur = ui.available_width() - 4.0;
            ui.set_width(largeur);
            ui.vertical(contenu);
        });
}

/// Carte cliquable, mise en valeur quand elle est retenue.
/// Grille de cartes : autant de colonnes que de cartes de `largeur_min` tiennent
/// côte à côte dans la largeur disponible, au moins une, et la largeur restante
/// répartie entre elles. `cellule` reçoit la largeur intérieure à donner à
/// [`carte`].
///
/// Un `horizontal_wrapped` ne convient pas aux cartes : egui ne connaît pas la
/// largeur d'un cadre avant de l'avoir dessiné et laisse la dernière carte de
/// chaque ligne déborder de la fenêtre. Ici les lignes sont formées d'avance.
fn grille_de_cartes<T>(
    ui: &mut egui::Ui,
    elements: &[T],
    largeur_min: f32,
    mut cellule: impl FnMut(&mut egui::Ui, &T, f32),
) {
    let espacement = ui.spacing().item_spacing.x;
    let (colonnes, largeur) = colonnes_de_cartes(ui.available_width(), largeur_min, espacement);
    for ligne in elements.chunks(colonnes) {
        ui.horizontal_top(|ui| {
            for element in ligne {
                cellule(ui, element, largeur);
            }
        });
    }
}

/// Nombre de colonnes et largeur intérieure des cartes pour une largeur donnée.
///
/// Plus étroit qu'une carte, l'espace ne donne qu'une colonne et la carte se
/// resserre plutôt que de déborder.
fn colonnes_de_cartes(disponible: f32, largeur_min: f32, espacement: f32) -> (usize, f32) {
    let exterieure = largeur_min + CARTE_MARGES;
    let colonnes =
        (((disponible + espacement) / (exterieure + espacement)).floor() as usize).max(1);
    let largeur =
        (disponible - espacement * (colonnes as f32 - 1.0)) / colonnes as f32 - CARTE_MARGES;
    (colonnes, largeur.max(80.0))
}

fn carte(
    ui: &mut egui::Ui,
    selectionnee: bool,
    largeur: f32,
    contenu: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let (fond, trait_) = if selectionnee {
        (
            ui.visuals().selection.bg_fill.linear_multiply(0.35),
            egui::Stroke::new(1.5, ui.visuals().selection.stroke.color),
        )
    } else {
        (
            ui.visuals().faint_bg_color,
            ui.visuals().widgets.noninteractive.bg_stroke,
        )
    };
    let reponse = egui::Frame::group(ui.style())
        .fill(fond)
        .stroke(trait_)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(largeur);
            ui.vertical(contenu);
        })
        .response
        .interact(egui::Sense::click());
    if reponse.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    reponse
}

/// Pastille colorée portant l'initiale de l'application.
///
/// Les icônes du catalogue sont des URL : les charger demanderait un accès
/// réseau depuis le fil d'affichage. Une pastille dérivée du nom donne le même
/// repère visuel, hors ligne et sans dépendance.
fn pastille(ui: &mut egui::Ui, nom: &str) {
    const TAILLE: f32 = 28.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(TAILLE, TAILLE), egui::Sense::hover());
    let teinte = couleur_stable(nom, ui.visuals().dark_mode);
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(6), teinte);
    let initiale = nom
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initiale,
        egui::FontId::proportional(15.0),
        egui::Color32::WHITE,
    );
}

/// Couleur reproductible tirée d'un nom : la même application garde sa teinte.
fn couleur_stable(nom: &str, sombre: bool) -> egui::Color32 {
    let empreinte = nom.bytes().fold(2166136261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16777619)
    });
    let teinte = f32::from(u16::try_from(empreinte % 360).unwrap_or(0));
    let (saturation, valeur) = if sombre { (0.55, 0.62) } else { (0.62, 0.72) };
    let (r, g, b) = hsv_vers_rgb(teinte, saturation, valeur);
    egui::Color32::from_rgb(r, g, b)
}

/// Conversion TSV → RVB, pour la palette des pastilles.
fn hsv_vers_rgb(teinte: f32, saturation: f32, valeur: f32) -> (u8, u8, u8) {
    let c = valeur * saturation;
    let secteur = (teinte / 60.0) % 6.0;
    let x = c * (1.0 - (secteur % 2.0 - 1.0).abs());
    let m = valeur - c;
    let (r, g, b) = match secteur as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let octet = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (octet(r), octet(g), octet(b))
}

/// Petite étiquette grise, pour les attributs d'une carte.
fn etiquette(ui: &mut egui::Ui, texte: &str) {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(5, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(texte).small().weak());
        });
}

/// Couleur associée au verdict de la prévision.
fn couleur_du_verdict(ui: &egui::Ui, fit: Fit) -> egui::Color32 {
    match fit {
        Fit::Fits => couleur_ok(ui),
        Fit::Tight => couleur_avertissement(ui),
        Fit::Exceeds => couleur_erreur(ui),
        Fit::Unknown => couleur_info(ui),
    }
}

/// Glyphe associé au verdict.
fn glyphe_du_verdict(fit: Fit) -> &'static str {
    match fit {
        Fit::Fits => icons::SUCCESS,
        Fit::Tight => icons::WARNING,
        Fit::Exceeds => icons::BLOCKED,
        Fit::Unknown => icons::INFO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wizard_avec_app(id: &str) -> WizardState {
        let app = kubewatch_hub::catalog::find_app(id)
            .unwrap_or_else(|| panic!("« {id} » doit être au catalogue"));
        let mut w = WizardState::default();
        prefill_from_catalog(&mut w, app);
        w
    }

    #[test]
    fn prefill_catalogue_produit_une_requete_valide() {
        for app in kubewatch_hub::catalog::builtin_apps() {
            let mut w = WizardState::default();
            prefill_from_catalog(&mut w, app);
            let req = w.built();
            deploy::validate(&req).unwrap_or_else(|e| {
                panic!("« {} » doit produire une requête déployable : {e}", app.id)
            });
        }
    }

    #[test]
    fn une_base_de_donnees_recoit_un_volume_et_un_profil_moyen() {
        let w = wizard_avec_app("postgresql");
        assert!(w.storage, "PostgreSQL demande un volume persistant");
        assert_eq!(w.profile, SizeProfile::Medium);
        let req = w.built();
        let pvc = req.pvc.expect("le volume doit être dans la requête");
        assert_eq!(pvc.size, "10Gi");
        assert_eq!(pvc.mount_path, "/data");
    }

    #[test]
    fn le_profil_ecrit_bien_les_quantites() {
        let mut w = wizard_avec_app("nginx");
        w.profile = SizeProfile::Large;
        w.sync();
        let req = w.built();
        assert_eq!(req.cpu_request.as_deref(), Some("1"));
        assert_eq!(req.memory_limit.as_deref(), Some("4Gi"));
        // Et l'empreinte suit les répliques.
        w.request.replicas = 3;
        let f = sizing::footprint(&w.built());
        assert_eq!(f.cpu_request_millis, Some(3000.0));
    }

    #[test]
    fn exposition_traduite_en_service_et_ingress() {
        let mut w = wizard_avec_app("nginx");

        w.exposure = Exposure::Internal;
        w.sync();
        assert_eq!(w.request.service_type.as_deref(), Some("ClusterIP"));
        assert!(w.request.ingress_host.is_none());

        w.exposure = Exposure::LoadBalancer;
        w.sync();
        assert_eq!(w.request.service_type.as_deref(), Some("LoadBalancer"));

        w.exposure = Exposure::Domain;
        w.host = "app.exemple.fr".to_string();
        w.sync();
        assert_eq!(w.request.service_type.as_deref(), Some("ClusterIP"));
        assert_eq!(w.request.ingress_host.as_deref(), Some("app.exemple.fr"));
        deploy::validate(&w.built()).expect("un Ingress nommé reste valide");

        // Un domaine vide ne doit pas produire d'Ingress sans hôte.
        w.host = "   ".to_string();
        w.sync();
        assert!(w.request.ingress_host.is_none());
    }

    #[test]
    fn stockage_active_et_desactive_le_volume() {
        let mut w = wizard_avec_app("nginx");
        assert!(!w.storage);
        w.storage = true;
        w.storage_gib = 25;
        w.storage_path = "/var/www".to_string();
        w.sync();
        let pvc = w.request.pvc.clone().expect("volume demandé");
        assert_eq!(pvc.size, "25Gi");
        assert_eq!(pvc.mount_path, "/var/www");
        w.storage = false;
        w.sync();
        assert!(w.request.pvc.is_none());
    }

    #[test]
    fn aller_retour_avec_le_formulaire_avance() {
        let mut w = wizard_avec_app("postgresql");
        w.exposure = Exposure::Domain;
        w.host = "db.exemple.fr".to_string();
        w.profile = SizeProfile::Large;
        w.sync();
        let requete = w.built();

        let mut repris = WizardState::default();
        adopt(&mut repris, requete);
        assert_eq!(repris.profile, SizeProfile::Large);
        assert_eq!(repris.exposure, Exposure::Domain);
        assert_eq!(repris.host, "db.exemple.fr");
        assert!(repris.storage);
        assert_eq!(repris.storage_gib, 10);
    }

    #[test]
    fn built_nettoie_la_saisie() {
        let mut w = WizardState::default();
        w.request.name = "  mon-api  ".to_string();
        w.request.namespace = "   ".to_string();
        w.request.image = " nginx:1.27 ".to_string();
        w.request.env = vec![
            ("  CLE  ".to_string(), "valeur".to_string()),
            ("   ".to_string(), "orpheline".to_string()),
        ];
        w.request.ports = vec![
            PortSpec {
                container_port: 0,
                ..PortSpec::default()
            },
            PortSpec {
                container_port: 80,
                ..PortSpec::default()
            },
        ];
        let req = w.built();
        assert_eq!(req.name, "mon-api");
        assert_eq!(
            req.namespace, "default",
            "un namespace vide retombe sur default"
        );
        assert_eq!(req.image, "nginx:1.27");
        assert_eq!(req.env, vec![("CLE".to_string(), "valeur".to_string())]);
        assert_eq!(req.ports.len(), 1);
    }

    #[test]
    fn noms_derives_des_references_dimage() {
        let mut w = WizardState::default();
        prefill_from_image(&mut w, "ghcr.io/mon-org/Mon_App:1.4.0", &[8080]);
        assert_eq!(w.request.name, "mon-app");
        assert_eq!(w.request.ports.len(), 1);
        deploy::validate(&w.built()).expect("requête valide");

        prefill_from_image(&mut w, "registry.local:5000/equipe/api@sha256:aa", &[]);
        assert_eq!(w.request.name, "api");
    }

    #[test]
    fn redemarrage_garde_le_namespace() {
        let mut w = wizard_avec_app("redis");
        w.request.namespace = "production".to_string();
        w.go(Step::Preview);
        w.restart();
        assert_eq!(w.step, Step::What);
        assert_eq!(w.reached, Step::What);
        assert_eq!(
            w.request.namespace, "production",
            "le namespace choisi survit à un nouveau déploiement"
        );
        assert!(w.picked.is_none());
        assert!(w.request.image.is_empty());
    }

    #[test]
    fn le_fil_retient_letape_la_plus_avancee() {
        let mut w = WizardState::default();
        w.go(Step::Preview);
        w.go(Step::How);
        assert_eq!(w.step, Step::How);
        assert_eq!(w.reached, Step::Preview, "on peut revenir en avant");
    }

    /// Dessine l'assistant hors écran, à chaque étape, pour vérifier qu'aucune
    /// disposition ne panique et qu'aucun emprunt ne se chevauche.
    ///
    /// Le backend démarre pour de bon, mais les seules commandes que l'écran
    /// poste ici — catalogue embarqué, génération de manifeste — se traitent
    /// sans réseau ni cluster.
    #[test]
    fn chaque_etape_se_dessine_sans_paniquer() {
        let ctx = egui::Context::default();
        let dossier = std::env::temp_dir().join(format!(
            "kubewatch-test-deploy-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let backend = Backend::new(ctx, dossier.clone()).expect("backend de test");

        let mut st = AppState::new();
        st.namespaces = vec!["default".to_string(), "production".to_string()];
        st.overview = Some(kubewatch_core::model::ClusterOverview {
            cpu_capacity_millis: 8000.0,
            cpu_used_millis: 2500.0,
            memory_capacity_bytes: 16 * 1024 * 1024 * 1024,
            memory_used_bytes: 6 * 1024 * 1024 * 1024,
            metrics_available: true,
            ..Default::default()
        });
        st.hub.catalog = kubewatch_hub::catalog::builtin_apps().to_vec();

        let app = kubewatch_hub::catalog::find_app("postgresql").expect("postgresql présent");
        prefill_from_catalog(&mut st.wizard, app);
        st.wizard.reached = Step::Result;
        st.wizard.preview = Some("apiVersion: apps/v1\nkind: Deployment\n".to_string());
        st.wizard.show_yaml = true;

        for etape in Step::ALL {
            st.wizard.step = etape;
            egui::__run_test_ui(|ui| show(ui, &mut st, &backend));
        }

        // Et les cas particuliers : saisie d'image libre, cluster non mesuré.
        st.wizard.step = Step::What;
        st.wizard.source = Source::Image;
        st.wizard.image_input = "ghcr.io/org/app:1.0".to_string();
        egui::__run_test_ui(|ui| show(ui, &mut st, &backend));

        st.overview = None;
        st.wizard.step = Step::Preview;
        egui::__run_test_ui(|ui| show(ui, &mut st, &backend));

        let _ = std::fs::remove_dir_all(&dossier);
    }

    #[test]
    fn couleurs_de_pastille_stables_et_distinctes() {
        assert_eq!(couleur_stable("NGINX", true), couleur_stable("NGINX", true));
        assert_ne!(couleur_stable("NGINX", true), couleur_stable("Redis", true));
    }
}

#[cfg(test)]
mod grille_tests {
    use super::*;

    #[test]
    fn colonnes_selon_la_largeur() {
        // 1000 px : trois cartes de 256 px extérieurs et deux espacements tiennent.
        let (colonnes, largeur) = colonnes_de_cartes(1000.0, CARTE_APP, 8.0);
        assert_eq!(colonnes, 3);
        assert!(largeur >= CARTE_APP);
        // La largeur restante est répartie : la ligne est remplie exactement.
        let occupee = colonnes as f32 * (largeur + CARTE_MARGES) + 2.0 * 8.0;
        assert!((occupee - 1000.0).abs() < 0.01);
        // Plus large : une colonne de plus, pas des cartes démesurées.
        let (colonnes, largeur) = colonnes_de_cartes(1400.0, CARTE_APP, 8.0);
        assert_eq!(colonnes, 5);
        assert!(largeur < 2.0 * CARTE_APP);
    }

    #[test]
    fn jamais_moins_d_une_colonne_ni_de_debordement() {
        let (colonnes, largeur) = colonnes_de_cartes(200.0, CARTE_APP, 8.0);
        assert_eq!(colonnes, 1);
        // Plus étroit que la carte : elle se resserre plutôt que de déborder.
        assert!(largeur + CARTE_MARGES <= 200.0);
        let (colonnes, largeur) = colonnes_de_cartes(0.0, CARTE_APP, 8.0);
        assert_eq!(colonnes, 1);
        assert!(largeur >= 80.0);
    }
}
