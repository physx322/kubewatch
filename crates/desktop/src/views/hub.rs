//! Vue « Hub » : recherche d'images de conteneurs, recherche de charts Helm,
//! catalogue intégré et formulaire de déploiement.
//!
//! Cette vue ne fait **aucun** appel réseau : elle lit [`AppState`] et envoie des
//! [`Command`] au backend, qui répond plus tard par des évènements. Tout ce qui
//! s'affiche ici provient donc de l'état, jamais d'une attente bloquante.

use crate::backend::command::RequestId;
use crate::backend::{Backend, Command};
use crate::icons;
use crate::state::AppState;
use kubewatch_core::apply::{ApplyAction, ApplyOutcome};
use kubewatch_hub::deploy::{self, DeployRequest, PortSpec, PvcSpec};
use kubewatch_hub::model::{CatalogApp, RegistryKind};

// ---------------------------------------------------------------------------
// Types d'état propres à la vue (référencés par `state::HubState`)
// ---------------------------------------------------------------------------

/// Onglet actif du hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HubTab {
    /// Recherche d'images de conteneurs.
    #[default]
    Images,
    /// Recherche de charts Helm sur Artifact Hub.
    Charts,
    /// Applications prêtes à déployer, embarquées dans le binaire.
    Catalog,
}

/// Formulaire de déploiement : la requête en cours d'édition et ses annexes.
#[derive(Debug, Clone)]
pub struct DeployForm {
    /// Le panneau est visible.
    pub open: bool,
    /// Requête en cours d'édition.
    pub request: DeployRequest,
    /// Contraintes de placement, éditées sous forme de liste (clé, valeur).
    ///
    /// `DeployRequest::node_selector` est une `BTreeMap` : éditer directement
    /// une clé de map ferait disparaître la ligne à chaque frappe.
    pub node_selector: Vec<(String, String)>,
    /// Simulation côté serveur au lieu d'une écriture réelle.
    pub dry_run: bool,
    /// Manifeste renvoyé par `Event::HubRendered`.
    pub preview: Option<String>,
    /// Bilan renvoyé par `Event::ApplyOutcome`.
    pub outcome: Option<ApplyOutcome>,
}

impl Default for DeployForm {
    fn default() -> Self {
        Self {
            open: false,
            request: DeployRequest::default(),
            node_selector: Vec::new(),
            // Le premier déploiement d'un utilisateur est une simulation : on ne
            // touche au cluster que sur un geste explicite.
            dry_run: true,
            preview: None,
            outcome: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Libellés des requêtes en vol
// ---------------------------------------------------------------------------

const LBL_SEARCH_IMAGES: &str = "Recherche d'images";
const LBL_TAGS: &str = "Chargement des tags";
const LBL_INSPECT: &str = "Inspection de l'image";
const LBL_SEARCH_CHARTS: &str = "Recherche de charts";
const LBL_CATALOG: &str = "Chargement du catalogue";
const LBL_RENDER: &str = "Génération du manifeste";
const LBL_DEPLOY: &str = "Déploiement";

// ---------------------------------------------------------------------------
// Point d'entrée
// ---------------------------------------------------------------------------

/// Dessine la vue Hub.
pub fn show(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    // Le catalogue est embarqué dans le binaire : on le demande une seule fois,
    // et le drapeau empêche de reposter la commande à chaque image.
    if !st.hub.catalog_requested {
        st.hub.catalog_requested = true;
        dispatch(st, backend, LBL_CATALOG, |id| Command::HubCatalog { id });
    }

    egui::Panel::top("hub-onglets").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut st.hub.tab, HubTab::Images, "Images");
            ui.selectable_value(&mut st.hub.tab, HubTab::Charts, "Charts Helm");
            ui.selectable_value(&mut st.hub.tab, HubTab::Catalog, "Catalogue");
            ui.separator();
            let libelle = if st.hub.deploy.open {
                "Masquer le formulaire"
            } else {
                "Formulaire de déploiement"
            };
            if ui.button(libelle).clicked() {
                st.hub.deploy.open = !st.hub.deploy.open;
            }
        });
        ui.add_space(4.0);
    });

    if st.hub.deploy.open {
        egui::Panel::right("hub-deploiement")
            .resizable(true)
            .default_size(420.0)
            .min_size(320.0)
            .show(ui, |ui| deploy_panel(ui, st, backend));
    }

    egui::CentralPanel::default().show(ui, |ui| match st.hub.tab {
        HubTab::Images => images_tab(ui, st, backend),
        HubTab::Charts => charts_tab(ui, st, backend),
        HubTab::Catalog => catalog_tab(ui, st, backend),
    });
}

// ---------------------------------------------------------------------------
// Onglet « Images »
// ---------------------------------------------------------------------------

fn images_tab(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    // Les indicateurs d'attente sont calculés avant les fermetures : une
    // fermeture ne peut pas emprunter `st` en lecture et un de ses champs en
    // écriture au même instant.
    let recherche_en_cours = is_busy(st, LBL_SEARCH_IMAGES);
    let mut lancer = false;
    ui.horizontal_wrapped(|ui| {
        let champ = ui.add(
            egui::TextEdit::singleline(&mut st.hub.image_query)
                .hint_text("nginx, postgres, redis…")
                .desired_width(240.0),
        );
        if champ.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            lancer = true;
        }
        egui::ComboBox::from_id_salt("hub-registre")
            .selected_text(st.hub.registry.label())
            .width(200.0)
            .show_ui(ui, |ui| {
                for kind in [
                    RegistryKind::DockerHub,
                    RegistryKind::Ghcr,
                    RegistryKind::Quay,
                    RegistryKind::Generic,
                ] {
                    ui.selectable_value(&mut st.hub.registry, kind, kind.label());
                }
            });
        if ui.button("Rechercher").clicked() {
            lancer = true;
        }
        if recherche_en_cours {
            ui.spinner();
        }
    });

    if lancer {
        let query = st.hub.image_query.trim().to_string();
        if query.is_empty() {
            st.hub.message = Some("Indiquez un terme de recherche.".to_string());
        } else {
            st.hub.message = None;
            st.hub.images.clear();
            st.hub.selected_image = None;
            st.hub.tags.clear();
            st.hub.selected_tag = None;
            st.hub.details = None;
            let registry = st.hub.registry;
            dispatch(st, backend, LBL_SEARCH_IMAGES, move |id| {
                Command::HubSearchImages {
                    id,
                    query,
                    registry,
                }
            });
        }
    }

    if let Some(msg) = st.hub.message.clone() {
        let couleur = couleur_avertissement(ui);
        ui.colored_label(couleur, msg);
    }
    ui.separator();

    if st.hub.selected_image.is_some() {
        egui::Panel::right("hub-tags")
            .resizable(true)
            .default_size(380.0)
            .min_size(260.0)
            .show(ui, |ui| tags_panel(ui, st, backend));
    }

    // Panneau imbriqué : pas de second cadre, la marge du panneau parent suffit.
    egui::CentralPanel::no_frame().show(ui, |ui| {
        if st.hub.images.is_empty() {
            ui.add_space(12.0);
            ui.weak("Aucun résultat. Lancez une recherche pour explorer un registre.");
            return;
        }
        let couleur_officielle = couleur_ok(ui);
        let mut choix: Option<String> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for img in &st.hub.images {
                    let actif = st.hub.selected_image.as_deref() == Some(img.name.as_str());
                    let reponse = ui
                        .push_id(&img.name, |ui| {
                            egui::Frame::group(ui.style())
                                .inner_margin(egui::Margin::symmetric(8, 6))
                                .show(ui, |ui| {
                                    let largeur = ui.available_width();
                                    ui.set_width(largeur);
                                    ui.horizontal_wrapped(|ui| {
                                        let titre = egui::RichText::new(&img.name).strong();
                                        ui.label(if actif { titre.underline() } else { titre });
                                        if img.official {
                                            ui.label(
                                                egui::RichText::new("officielle")
                                                    .small()
                                                    .color(couleur_officielle),
                                            );
                                        }
                                        if let Some(n) = img.stars {
                                            ui.weak(format!("{} {n}", icons::STAR));
                                        }
                                        if let Some(n) = img.pulls {
                                            ui.weak(format!(
                                                "{} {}",
                                                icons::DOWNLOADS,
                                                nombre_court(n)
                                            ));
                                        }
                                        ui.weak(img.registry.label());
                                    });
                                    if let Some(d) = &img.description {
                                        ui.weak(truncate(d, 160));
                                    }
                                });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if reponse.clicked() {
                        choix = Some(img.name.clone());
                    }
                }
            });

        if let Some(nom) = choix {
            st.hub.selected_image = Some(nom.clone());
            st.hub.tags.clear();
            st.hub.selected_tag = None;
            st.hub.details = None;
            dispatch(st, backend, LBL_TAGS, move |id| Command::HubListTags {
                id,
                image: nom,
            });
        }
    });
}

/// Panneau latéral : les tags de l'image sélectionnée, puis ses détails.
fn tags_panel(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let image = match st.hub.selected_image.clone() {
        Some(i) => i,
        None => return,
    };

    ui.horizontal(|ui| {
        ui.heading(truncate(&image, 34));
        if ui
            .small_button(icons::CLOSE)
            .on_hover_text("Fermer")
            .clicked()
        {
            st.hub.selected_image = None;
            st.hub.tags.clear();
            st.hub.selected_tag = None;
            st.hub.details = None;
        }
    });
    if st.hub.selected_image.is_none() {
        return;
    }
    let tags_en_cours = is_busy(st, LBL_TAGS);
    if tags_en_cours {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Chargement des tags…");
        });
    }
    ui.separator();

    let mut tag_choisi: Option<String> = None;
    egui::ScrollArea::vertical()
        .id_salt("hub-tags-liste")
        .max_height(260.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if st.hub.tags.is_empty() && !tags_en_cours {
                ui.weak("Aucun tag publié n'a été retourné par le registre.");
            }
            for tag in &st.hub.tags {
                let actif = st.hub.selected_tag.as_deref() == Some(tag.name.as_str());
                let mut ligne = tag.name.clone();
                if let Some(sv) = &tag.semver {
                    if sv != &tag.name {
                        ligne.push_str(&format!("  ({sv})"));
                    }
                }
                let reponse = ui.selectable_label(actif, ligne);
                let mut detail = String::new();
                if let Some(o) = tag.size_bytes {
                    detail.push_str(&human_bytes(o));
                }
                if tag.pushed_at.is_some() {
                    if !detail.is_empty() {
                        detail.push_str(" · ");
                    }
                    detail.push_str(&human_date(tag.pushed_at));
                }
                if !tag.platforms.is_empty() {
                    if !detail.is_empty() {
                        detail.push_str(" · ");
                    }
                    detail.push_str(&tag.platforms.join(", "));
                }
                if !detail.is_empty() {
                    ui.indent(("detail-tag", &tag.name), |ui| {
                        ui.weak(egui::RichText::new(detail).small());
                    });
                }
                if reponse.clicked() {
                    tag_choisi = Some(tag.name.clone());
                }
            }
        });

    if let Some(tag) = tag_choisi {
        st.hub.selected_tag = Some(tag.clone());
        st.hub.details = None;
        let reference = format!("{image}:{tag}");
        dispatch(st, backend, LBL_INSPECT, move |id| Command::HubInspect {
            id,
            image: reference,
        });
    }

    ui.separator();
    if is_busy(st, LBL_INSPECT) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Inspection du manifeste OCI…");
        });
        return;
    }

    let mut prefill: Option<(String, Vec<u16>, Vec<String>)> = None;
    egui::ScrollArea::vertical()
        .id_salt("hub-details")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let details = match &st.hub.details {
                Some(d) => d,
                None => {
                    ui.weak("Sélectionnez un tag pour inspecter l'image.");
                    return;
                }
            };

            ui.label(egui::RichText::new(&details.reference).monospace().small());
            egui::Grid::new("hub-details-grid")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    if let Some(d) = &details.digest {
                        ui.weak("Digest");
                        ui.label(egui::RichText::new(truncate(d, 30)).monospace().small())
                            .on_hover_text(d.clone());
                        ui.end_row();
                    }
                    let plateforme = match (&details.os, &details.architecture) {
                        (Some(os), Some(arch)) => format!("{os}/{arch}"),
                        (Some(os), None) => os.clone(),
                        (None, Some(arch)) => arch.clone(),
                        (None, None) => "—".to_string(),
                    };
                    ui.weak("Plateforme");
                    ui.label(plateforme);
                    ui.end_row();

                    ui.weak("Ports exposés");
                    if details.exposed_ports.is_empty() {
                        ui.label("aucun");
                    } else {
                        ui.label(
                            details
                                .exposed_ports
                                .iter()
                                .map(|p| p.to_string())
                                .collect::<Vec<_>>()
                                .join(", "),
                        );
                    }
                    ui.end_row();

                    if !details.entrypoint.is_empty() {
                        ui.weak("Entrypoint");
                        ui.label(egui::RichText::new(details.entrypoint.join(" ")).monospace());
                        ui.end_row();
                    }
                    if !details.cmd.is_empty() {
                        ui.weak("Commande");
                        ui.label(egui::RichText::new(details.cmd.join(" ")).monospace());
                        ui.end_row();
                    }
                    if let Some(repo) = &details.source_repo {
                        ui.weak("Dépôt source");
                        ui.hyperlink_to(truncate(repo, 34), repo_url(repo));
                        ui.end_row();
                    }
                });

            if !details.env.is_empty() {
                ui.collapsing(
                    format!("Variables d'environnement ({})", details.env.len()),
                    |ui| {
                        for v in &details.env {
                            ui.label(egui::RichText::new(v).monospace().small());
                        }
                    },
                );
            }
            if !details.labels.is_empty() {
                ui.collapsing(format!("Étiquettes ({})", details.labels.len()), |ui| {
                    egui::Grid::new("hub-labels")
                        .num_columns(2)
                        .spacing([8.0, 2.0])
                        .show(ui, |ui| {
                            for (k, v) in &details.labels {
                                ui.label(egui::RichText::new(k).monospace().small());
                                ui.label(egui::RichText::new(truncate(v, 60)).small())
                                    .on_hover_text(v.clone());
                                ui.end_row();
                            }
                        });
                });
            }

            ui.add_space(6.0);
            if ui.button("Déployer cette image").clicked() {
                prefill = Some((
                    details.reference.clone(),
                    details.exposed_ports.clone(),
                    details.env.clone(),
                ));
            }
        });

    if let Some((reference, ports, env)) = prefill {
        prefill_from_image(&mut st.hub.deploy, &reference, &ports, &env);
        st.hub.deploy.open = true;
    }
}

// ---------------------------------------------------------------------------
// Onglet « Charts »
// ---------------------------------------------------------------------------

fn charts_tab(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let recherche_en_cours = is_busy(st, LBL_SEARCH_CHARTS);
    let mut lancer = false;
    ui.horizontal_wrapped(|ui| {
        let champ = ui.add(
            egui::TextEdit::singleline(&mut st.hub.chart_query)
                .hint_text("postgresql, ingress-nginx…")
                .desired_width(260.0),
        );
        if champ.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            lancer = true;
        }
        if ui.button("Rechercher sur Artifact Hub").clicked() {
            lancer = true;
        }
        if recherche_en_cours {
            ui.spinner();
        }
    });

    if lancer {
        let query = st.hub.chart_query.trim().to_string();
        if !query.is_empty() {
            st.hub.charts.clear();
            dispatch(st, backend, LBL_SEARCH_CHARTS, move |id| {
                Command::HubSearchCharts { id, query }
            });
        }
    }

    ui.separator();
    if st.hub.charts.is_empty() {
        ui.add_space(12.0);
        ui.weak("Aucun chart. Lancez une recherche pour interroger Artifact Hub.");
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for chart in &st.hub.charts {
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        let largeur = ui.available_width();
                        ui.set_width(largeur);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(&chart.name).strong());
                            ui.weak(format!("dépôt {}", chart.repository));
                            ui.weak(format!("chart {}", chart.version));
                            if let Some(av) = &chart.app_version {
                                ui.weak(format!("application {av}"));
                            }
                            if let Some(n) = chart.stars {
                                ui.weak(format!("{} {n}", icons::STAR));
                            }
                        });
                        if let Some(d) = &chart.description {
                            ui.weak(truncate(d, 200));
                        }
                        ui.horizontal_wrapped(|ui| {
                            if let Some(home) = &chart.home {
                                ui.hyperlink_to("Site du projet", home);
                            }
                            if let Some(url) = &chart.repo_url {
                                ui.weak(egui::RichText::new(url).monospace().small());
                            }
                        });
                    });
            }
        });
}

// ---------------------------------------------------------------------------
// Onglet « Catalogue »
// ---------------------------------------------------------------------------

fn catalog_tab(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let chargement = is_busy(st, LBL_CATALOG);
    let mut recharger = false;
    ui.horizontal(|ui| {
        ui.weak("Applications prêtes à déployer, embarquées dans KubeWatch.");
        if ui.small_button("Recharger").clicked() {
            recharger = true;
        }
        if chargement {
            ui.spinner();
        }
    });
    if recharger {
        dispatch(st, backend, LBL_CATALOG, |id| Command::HubCatalog { id });
    }
    ui.separator();

    if st.hub.catalog.is_empty() {
        ui.add_space(12.0);
        ui.weak("Catalogue vide.");
        return;
    }

    // Regroupement par catégorie, ordre alphabétique stable.
    let mut par_categorie: std::collections::BTreeMap<String, Vec<&CatalogApp>> =
        std::collections::BTreeMap::new();
    for app in &st.hub.catalog {
        par_categorie
            .entry(app.category.clone())
            .or_default()
            .push(app);
    }

    let mut choix: Option<CatalogApp> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (categorie, apps) in &par_categorie {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(categorie).heading().size(15.0));
                ui.horizontal_wrapped(|ui| {
                    for app in apps {
                        egui::Frame::group(ui.style())
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.set_width(250.0);
                                ui.vertical(|ui| {
                                    ui.label(egui::RichText::new(&app.name).strong());
                                    ui.weak(truncate(&app.description, 130));
                                    ui.label(egui::RichText::new(&app.image).monospace().small());
                                    ui.horizontal_wrapped(|ui| {
                                        if let Some(p) = app.default_port {
                                            ui.weak(format!("port {p}"));
                                        }
                                        if app.needs_pvc {
                                            ui.weak("volume persistant");
                                        }
                                        if let Some(doc) = &app.docs_url {
                                            ui.hyperlink_to("Doc", doc);
                                        }
                                    });
                                    if ui.button("Déployer").clicked() {
                                        choix = Some((**app).clone());
                                    }
                                });
                            });
                    }
                });
            }
        });

    if let Some(app) = choix {
        prefill_from_catalog(&mut st.hub.deploy, &app);
        st.hub.deploy.open = true;
    }
}

// ---------------------------------------------------------------------------
// Formulaire de déploiement
// ---------------------------------------------------------------------------

fn deploy_panel(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let mut vers_assistant = false;
    ui.horizontal(|ui| {
        ui.heading("Déploiement");
        if ui
            .small_button("Assistant")
            .on_hover_text(
                "Reprend ce déploiement dans l'assistant guidé, avec l'aperçu de la \
                 consommation de ressources.",
            )
            .clicked()
        {
            vers_assistant = true;
        }
        if ui
            .small_button(icons::CLOSE)
            .on_hover_text("Fermer")
            .clicked()
        {
            st.hub.deploy.open = false;
        }
    });
    ui.separator();

    if vers_assistant {
        let requete = build_request(&st.hub.deploy);
        crate::views::deploy::adopt(&mut st.wizard, requete);
        st.wizard.step = crate::views::deploy::Step::How;
        st.wizard.reached = crate::views::deploy::Step::How;
        st.view = crate::state::View::Deploy;
        return;
    }

    let cluster = st.cluster().map(|c| c.to_string());
    if cluster.is_none() {
        let couleur = couleur_avertissement(ui);
        ui.colored_label(
            couleur,
            "Aucun cluster sélectionné : le manifeste peut être généré, mais pas appliqué.",
        );
    }
    // Calculé hors des fermetures : voir la note de `images_tab`.
    let travail_en_cours = is_busy(st, LBL_RENDER) || is_busy(st, LBL_DEPLOY);

    egui::ScrollArea::vertical()
        .id_salt("hub-formulaire")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            formulaire_identite(ui, st);
            ui.add_space(6.0);
            formulaire_ports(ui, st);
            ui.add_space(6.0);
            formulaire_env(ui, st);
            ui.add_space(6.0);
            formulaire_ressources(ui, st);
            ui.add_space(6.0);
            formulaire_reseau(ui, st);
            ui.add_space(6.0);
            formulaire_stockage(ui, st);
            ui.add_space(6.0);
            formulaire_execution(ui, st);
            ui.add_space(6.0);
            formulaire_placement(ui, st);

            ui.add_space(10.0);
            ui.separator();

            // Validation en direct : les messages viennent du hub lui-même, on
            // n'en réinvente aucun.
            let requete = build_request(&st.hub.deploy);
            let erreur = deploy::validate(&requete).err();
            let rouge = couleur_erreur(ui);
            let vert = couleur_ok(ui);
            match &erreur {
                Some(e) => {
                    ui.colored_label(rouge, format!("{} {e}", icons::WARNING));
                }
                None => {
                    ui.colored_label(vert, format!("{} La requête est valide.", icons::SUCCESS));
                }
            }

            let valide = erreur.is_none();
            let mut apercu = false;
            let mut deployer = false;
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(valide, egui::Button::new("Aperçu du YAML"))
                    .clicked()
                {
                    apercu = true;
                }
                ui.checkbox(&mut st.hub.deploy.dry_run, "Dry-run")
                    .on_hover_text("Simulation côté serveur : rien n'est écrit dans le cluster.");
                if ui
                    .add_enabled(valide && cluster.is_some(), egui::Button::new("Déployer"))
                    .clicked()
                {
                    deployer = true;
                }
                if travail_en_cours {
                    ui.spinner();
                }
            });

            if apercu {
                st.hub.deploy.preview = None;
                let requete = requete.clone();
                dispatch(st, backend, LBL_RENDER, move |id| Command::HubRender {
                    id,
                    request: Box::new(requete),
                });
            }
            if deployer {
                if let Some(nom_cluster) = cluster.clone() {
                    st.hub.deploy.outcome = None;
                    let requete = requete.clone();
                    let dry_run = st.hub.deploy.dry_run;
                    dispatch(st, backend, LBL_DEPLOY, move |id| Command::HubDeploy {
                        id,
                        cluster: nom_cluster,
                        request: Box::new(requete),
                        dry_run,
                    });
                }
            }

            if let Some(yaml) = st.hub.deploy.preview.clone() {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Manifeste généré").strong());
                    ui.weak("(lecture seule)");
                    if ui.small_button("Copier").clicked() {
                        ui.ctx().copy_text(yaml.clone());
                    }
                });
                // Le tampon est une copie jetée à chaque image : le texte affiché
                // reste sélectionnable sans jamais devenir modifiable.
                let mut lecture_seule = yaml;
                egui::ScrollArea::vertical()
                    .id_salt("hub-apercu")
                    .max_height(260.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut lecture_seule)
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    });
            }

            if let Some(outcome) = st.hub.deploy.outcome.clone() {
                ui.add_space(8.0);
                outcome_view(ui, &outcome);
            }
        });
}

fn formulaire_identite(ui: &mut egui::Ui, st: &mut AppState) {
    ui.label(egui::RichText::new("Identité").strong());
    egui::Grid::new("deploy-identite")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            let req = &mut st.hub.deploy.request;
            ui.label("Nom");
            ui.add(
                egui::TextEdit::singleline(&mut req.name)
                    .hint_text("mon-api")
                    .desired_width(220.0),
            );
            ui.end_row();

            ui.label("Namespace");
            ui.add(
                egui::TextEdit::singleline(&mut req.namespace)
                    .hint_text("default")
                    .desired_width(220.0),
            );
            ui.end_row();

            ui.label("Image");
            ui.add(
                egui::TextEdit::singleline(&mut req.image)
                    .hint_text("nginx:1.27-alpine")
                    .desired_width(220.0),
            );
            ui.end_row();

            ui.label("Répliques");
            ui.add(
                egui::DragValue::new(&mut req.replicas)
                    .range(0..=1000)
                    .speed(0.2),
            );
            ui.end_row();

            ui.label("Secret de registre");
            edit_opt_string(ui, &mut req.image_pull_secret, "regcred", 220.0);
            ui.end_row();

            ui.label("Compte de service");
            edit_opt_string(ui, &mut req.service_account, "default", 220.0);
            ui.end_row();
        });
}

fn formulaire_ports(ui: &mut egui::Ui, st: &mut AppState) {
    ui.label(egui::RichText::new("Ports").strong());
    let mut retirer: Option<usize> = None;
    for (i, port) in st.hub.deploy.request.ports.iter_mut().enumerate() {
        ui.push_id(("port", i), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("conteneur");
                ui.add(
                    egui::DragValue::new(&mut port.container_port)
                        .range(1..=65535)
                        .speed(1.0),
                );
                let mut expose = port.service_port.is_some();
                if ui
                    .checkbox(&mut expose, "port de service distinct")
                    .changed()
                {
                    port.service_port = if expose {
                        Some(port.container_port)
                    } else {
                        None
                    };
                }
                if let Some(sp) = port.service_port.as_mut() {
                    ui.add(egui::DragValue::new(sp).range(1..=65535).speed(1.0));
                }
                edit_opt_string(ui, &mut port.name, "nom", 90.0);
                let mut proto = port.protocol.clone().unwrap_or_else(|| "TCP".to_string());
                egui::ComboBox::from_id_salt(("proto", i))
                    .selected_text(proto.clone())
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        for p in ["TCP", "UDP", "SCTP"] {
                            ui.selectable_value(&mut proto, p.to_string(), p);
                        }
                    });
                port.protocol = Some(proto);
                if ui
                    .small_button(icons::CLOSE)
                    .on_hover_text("Retirer ce port")
                    .clicked()
                {
                    retirer = Some(i);
                }
            });
        });
    }
    if let Some(i) = retirer {
        st.hub.deploy.request.ports.remove(i);
    }
    if ui.button("+ Ajouter un port").clicked() {
        st.hub.deploy.request.ports.push(PortSpec {
            container_port: 8080,
            ..PortSpec::default()
        });
    }
}

fn formulaire_env(ui: &mut egui::Ui, st: &mut AppState) {
    ui.label(egui::RichText::new("Variables d'environnement").strong());
    let mut retirer: Option<usize> = None;
    for (i, (cle, valeur)) in st.hub.deploy.request.env.iter_mut().enumerate() {
        ui.push_id(("env", i), |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(cle)
                        .hint_text("CLÉ")
                        .desired_width(120.0),
                );
                ui.add(
                    egui::TextEdit::singleline(valeur)
                        .hint_text("valeur")
                        .desired_width(150.0),
                );
                if ui
                    .small_button(icons::CLOSE)
                    .on_hover_text("Retirer")
                    .clicked()
                {
                    retirer = Some(i);
                }
            });
        });
    }
    if let Some(i) = retirer {
        st.hub.deploy.request.env.remove(i);
    }
    if ui.button("+ Ajouter une variable").clicked() {
        st.hub
            .deploy
            .request
            .env
            .push((String::new(), String::new()));
    }
}

fn formulaire_ressources(ui: &mut egui::Ui, st: &mut AppState) {
    ui.label(egui::RichText::new("Ressources").strong());
    egui::Grid::new("deploy-ressources")
        .num_columns(3)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            let req = &mut st.hub.deploy.request;
            ui.label("");
            ui.weak("demande");
            ui.weak("limite");
            ui.end_row();

            ui.label("CPU");
            edit_opt_string(ui, &mut req.cpu_request, "100m", 110.0);
            edit_opt_string(ui, &mut req.cpu_limit, "500m", 110.0);
            ui.end_row();

            ui.label("Mémoire");
            edit_opt_string(ui, &mut req.memory_request, "128Mi", 110.0);
            edit_opt_string(ui, &mut req.memory_limit, "512Mi", 110.0);
            ui.end_row();
        });
}

fn formulaire_reseau(ui: &mut egui::Ui, st: &mut AppState) {
    ui.label(egui::RichText::new("Exposition").strong());
    egui::Grid::new("deploy-reseau")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            let req = &mut st.hub.deploy.request;
            ui.label("Type de Service");
            let mut type_service = req
                .service_type
                .clone()
                .unwrap_or_else(|| "ClusterIP".to_string());
            egui::ComboBox::from_id_salt("deploy-service-type")
                .selected_text(type_service.clone())
                .width(160.0)
                .show_ui(ui, |ui| {
                    for t in ["ClusterIP", "NodePort", "LoadBalancer"] {
                        ui.selectable_value(&mut type_service, t.to_string(), t);
                    }
                });
            req.service_type = Some(type_service);
            ui.end_row();

            ui.label("Hôte d'Ingress");
            edit_opt_string(ui, &mut req.ingress_host, "app.exemple.fr", 220.0);
            ui.end_row();

            ui.label("Classe d'Ingress");
            edit_opt_string(ui, &mut req.ingress_class, "nginx", 220.0);
            ui.end_row();

            ui.label("Secret TLS");
            edit_opt_string(ui, &mut req.ingress_tls_secret, "app-tls", 220.0);
            ui.end_row();
        });
    ui.weak("Sans hôte d'Ingress, aucun objet Ingress n'est généré.");
}

fn formulaire_stockage(ui: &mut egui::Ui, st: &mut AppState) {
    ui.label(egui::RichText::new("Stockage").strong());
    let req = &mut st.hub.deploy.request;
    let mut actif = req.pvc.is_some();
    if ui.checkbox(&mut actif, "Volume persistant").changed() {
        req.pvc = if actif {
            Some(PvcSpec {
                size: "10Gi".to_string(),
                mount_path: "/data".to_string(),
                ..PvcSpec::default()
            })
        } else {
            None
        };
    }
    if let Some(pvc) = req.pvc.as_mut() {
        egui::Grid::new("deploy-pvc")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Taille");
                ui.add(
                    egui::TextEdit::singleline(&mut pvc.size)
                        .hint_text("10Gi")
                        .desired_width(120.0),
                );
                ui.end_row();

                ui.label("Point de montage");
                ui.add(
                    egui::TextEdit::singleline(&mut pvc.mount_path)
                        .hint_text("/data")
                        .desired_width(220.0),
                );
                ui.end_row();

                ui.label("Classe de stockage");
                edit_opt_string(ui, &mut pvc.storage_class, "standard", 220.0);
                ui.end_row();

                ui.label("Mode d'accès");
                let mut mode = pvc
                    .access_mode
                    .clone()
                    .unwrap_or_else(|| "ReadWriteOnce".to_string());
                egui::ComboBox::from_id_salt("deploy-pvc-mode")
                    .selected_text(mode.clone())
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for m in [
                            "ReadWriteOnce",
                            "ReadOnlyMany",
                            "ReadWriteMany",
                            "ReadWriteOncePod",
                        ] {
                            ui.selectable_value(&mut mode, m.to_string(), m);
                        }
                    });
                pvc.access_mode = Some(mode);
                ui.end_row();
            });
    }
}

fn formulaire_execution(ui: &mut egui::Ui, st: &mut AppState) {
    ui.label(egui::RichText::new("Exécution").strong());
    ui.weak("Laissez vide pour conserver l'entrypoint de l'image.");
    liste_de_chaines(ui, "cmd", "Commande", &mut st.hub.deploy.request.command);
    liste_de_chaines(ui, "args", "Arguments", &mut st.hub.deploy.request.args);
}

fn formulaire_placement(ui: &mut egui::Ui, st: &mut AppState) {
    ui.label(egui::RichText::new("Placement (nodeSelector)").strong());
    let mut retirer: Option<usize> = None;
    for (i, (cle, valeur)) in st.hub.deploy.node_selector.iter_mut().enumerate() {
        ui.push_id(("ns", i), |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(cle)
                        .hint_text("kubernetes.io/os")
                        .desired_width(150.0),
                );
                ui.add(
                    egui::TextEdit::singleline(valeur)
                        .hint_text("linux")
                        .desired_width(120.0),
                );
                if ui
                    .small_button(icons::CLOSE)
                    .on_hover_text("Retirer")
                    .clicked()
                {
                    retirer = Some(i);
                }
            });
        });
    }
    if let Some(i) = retirer {
        st.hub.deploy.node_selector.remove(i);
    }
    if ui.button("+ Ajouter une contrainte").clicked() {
        st.hub
            .deploy
            .node_selector
            .push((String::new(), String::new()));
    }
}

/// Éditeur de liste de chaînes (commande, arguments).
fn liste_de_chaines(ui: &mut egui::Ui, sel: &str, titre: &str, valeurs: &mut Vec<String>) {
    ui.push_id(sel, |ui| {
        ui.horizontal(|ui| {
            ui.weak(titre);
            if ui
                .small_button("+")
                .on_hover_text("Ajouter un élément")
                .clicked()
            {
                valeurs.push(String::new());
            }
        });
        let mut retirer: Option<usize> = None;
        for (i, v) in valeurs.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(v).desired_width(220.0));
                    if ui.small_button(icons::CLOSE).clicked() {
                        retirer = Some(i);
                    }
                });
            });
        }
        if let Some(i) = retirer {
            valeurs.remove(i);
        }
    });
}

// ---------------------------------------------------------------------------
// Construction et préremplissage de la requête
// ---------------------------------------------------------------------------

/// Assemble la requête finale : champs nettoyés, lignes vides écartées.
///
/// Les lignes vides restent dans le formulaire (l'utilisateur est peut-être en
/// train de les remplir) mais ne partent jamais vers le générateur.
pub fn build_request(form: &DeployForm) -> DeployRequest {
    let mut req = form.request.clone();
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
    req.command.retain(|c| !c.trim().is_empty());
    req.args.retain(|a| !a.trim().is_empty());
    req.node_selector = form
        .node_selector
        .iter()
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    req
}

/// Prérremplit le formulaire à partir d'une image inspectée.
pub fn prefill_from_image(form: &mut DeployForm, reference: &str, ports: &[u16], env: &[String]) {
    form.request = DeployRequest {
        name: sanitize_name(nom_court_depuis_reference(reference)),
        namespace: form.request.namespace.clone(),
        image: reference.to_string(),
        replicas: 1,
        ports: ports
            .iter()
            .map(|p| PortSpec {
                container_port: *p,
                ..PortSpec::default()
            })
            .collect(),
        // Les variables par défaut de l'image sont proposées, à l'utilisateur de
        // ne garder que celles qu'il veut surcharger.
        env: env
            .iter()
            .filter_map(|e| e.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..DeployRequest::default()
    };
    form.node_selector.clear();
    form.preview = None;
    form.outcome = None;
}

/// Prérremplit le formulaire à partir d'une application du catalogue.
pub fn prefill_from_catalog(form: &mut DeployForm, app: &CatalogApp) {
    form.request = DeployRequest {
        name: sanitize_name(&app.id),
        namespace: form.request.namespace.clone(),
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
        pvc: if app.needs_pvc {
            Some(PvcSpec {
                size: "10Gi".to_string(),
                mount_path: "/data".to_string(),
                ..PvcSpec::default()
            })
        } else {
            None
        },
        ..DeployRequest::default()
    };
    form.node_selector.clear();
    form.preview = None;
    form.outcome = None;
}

/// `ghcr.io/o/depot:1.2` -> `depot`.
fn nom_court_depuis_reference(reference: &str) -> &str {
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
    let out = out.trim_matches('-').to_string();
    let out: String = out.chars().take(63).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "application".to_string()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// Rendu d'un ApplyOutcome
// ---------------------------------------------------------------------------

/// Affiche le bilan d'une application de manifeste.
pub fn outcome_view(ui: &mut egui::Ui, outcome: &ApplyOutcome) {
    let total = outcome.items.len();
    let entete = if outcome.failed == 0 {
        egui::RichText::new(format!("{total} document(s) traité(s), aucun échec"))
            .color(couleur_ok(ui))
    } else {
        egui::RichText::new(format!(
            "{total} document(s) traité(s), {} en échec",
            outcome.failed
        ))
        .color(couleur_erreur(ui))
    };
    ui.label(entete.strong());

    egui::Grid::new("hub-outcome")
        .num_columns(3)
        .spacing([10.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for item in &outcome.items {
                let (libelle, couleur) = action_style(ui, item.action);
                ui.colored_label(couleur, libelle);
                ui.label(format!(
                    "{} {}",
                    item.resource.kind,
                    item.resource.display()
                ));
                match &item.message {
                    Some(m) => {
                        ui.label(egui::RichText::new(truncate(m, 90)).small())
                            .on_hover_text(m.clone());
                    }
                    None => {
                        ui.label("");
                    }
                }
                ui.end_row();
            }
        });
}

fn action_style(ui: &egui::Ui, action: ApplyAction) -> (&'static str, egui::Color32) {
    match action {
        ApplyAction::Created => ("créé", couleur_ok(ui)),
        ApplyAction::Configured => ("configuré", couleur_info(ui)),
        ApplyAction::Unchanged => ("inchangé", ui.visuals().weak_text_color()),
        ApplyAction::DryRun => ("simulé", couleur_avertissement(ui)),
        ApplyAction::Deleted => ("supprimé", couleur_avertissement(ui)),
        ApplyAction::Failed => ("échec", couleur_erreur(ui)),
    }
}

// ---------------------------------------------------------------------------
// Utilitaires partagés avec les autres vues du hub
// ---------------------------------------------------------------------------

/// Envoie une commande porteuse d'un identifiant et note la requête en vol.
pub(crate) fn dispatch(
    st: &mut AppState,
    backend: &Backend,
    label: &str,
    make: impl FnOnce(RequestId) -> Command,
) {
    let id = st.next_id();
    st.pending.insert(id, label.to_string());
    backend.send(make(id));
}

/// Vrai si une requête portant ce libellé est encore en vol.
pub(crate) fn is_busy(st: &AppState, label: &str) -> bool {
    st.pending.values().any(|l| l == label)
}

/// Éditeur d'un champ optionnel : vide signifie « absent ».
pub(crate) fn edit_opt_string(
    ui: &mut egui::Ui,
    value: &mut Option<String>,
    hint: &str,
    largeur: f32,
) {
    let mut tampon = value.clone().unwrap_or_default();
    let reponse = ui.add(
        egui::TextEdit::singleline(&mut tampon)
            .hint_text(hint)
            .desired_width(largeur),
    );
    if reponse.changed() {
        *value = if tampon.is_empty() {
            None
        } else {
            Some(tampon)
        };
    }
}

/// Tronque proprement sur une frontière de caractère.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Taille lisible en octets binaires.
pub(crate) fn human_bytes(octets: i64) -> String {
    if octets < 0 {
        return "—".to_string();
    }
    const UNITES: [&str; 5] = ["o", "Kio", "Mio", "Gio", "Tio"];
    let mut valeur = octets as f64;
    let mut i = 0usize;
    while valeur >= 1024.0 && i + 1 < UNITES.len() {
        valeur /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{octets} o")
    } else {
        format!("{valeur:.1} {}", UNITES[i])
    }
}

/// Date locale lisible, ou un tiret quand elle est inconnue.
pub(crate) fn human_date(date: Option<chrono::DateTime<chrono::Utc>>) -> String {
    match date {
        Some(d) => d
            .with_timezone(&chrono::Local)
            .format("%d/%m/%Y %H:%M")
            .to_string(),
        None => "—".to_string(),
    }
}

/// Grand nombre abrégé (`1,2 M`).
fn nombre_court(n: i64) -> String {
    let a = n.unsigned_abs();
    if a >= 1_000_000_000 {
        format!("{:.1} G", n as f64 / 1e9)
    } else if a >= 1_000_000 {
        format!("{:.1} M", n as f64 / 1e6)
    } else if a >= 1_000 {
        format!("{:.1} k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// URL du dépôt source : une URL complète est conservée telle quelle, un
/// `propriétaire/dépôt` est résolu sur GitHub.
fn repo_url(raw: &str) -> String {
    let r = raw.trim();
    if r.starts_with("http://") || r.starts_with("https://") {
        r.to_string()
    } else {
        format!("https://github.com/{}", r.trim_start_matches('/'))
    }
}

pub(crate) fn couleur_ok(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(96, 190, 120)
    } else {
        egui::Color32::from_rgb(28, 128, 62)
    }
}

pub(crate) fn couleur_info(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(110, 165, 235)
    } else {
        egui::Color32::from_rgb(35, 96, 178)
    }
}

pub(crate) fn couleur_avertissement(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(230, 170, 70)
    } else {
        egui::Color32::from_rgb(168, 110, 10)
    }
}

pub(crate) fn couleur_erreur(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(232, 110, 110)
    } else {
        egui::Color32::from_rgb(178, 40, 40)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nom_derive_dune_reference() {
        assert_eq!(nom_court_depuis_reference("ghcr.io/o/depot:1.2"), "depot");
        assert_eq!(nom_court_depuis_reference("nginx"), "nginx");
        assert_eq!(
            nom_court_depuis_reference("docker.io/library/redis@sha256:aa"),
            "redis"
        );
    }

    #[test]
    fn nom_assaini_est_dns1123() {
        assert_eq!(sanitize_name("Mon API v2"), "mon-api-v2");
        assert_eq!(sanitize_name("---"), "application");
        assert_eq!(sanitize_name(""), "application");
        assert!(deploy::is_dns1123_label(&sanitize_name("Postgre SQL 16")));
    }

    #[test]
    fn requete_construite_sans_lignes_vides() {
        let mut form = DeployForm::default();
        form.request.name = "  api  ".to_string();
        form.request.image = " nginx:1.27 ".to_string();
        form.request.namespace = "   ".to_string();
        form.request.env = vec![
            ("A".to_string(), "1".to_string()),
            ("  ".to_string(), "x".to_string()),
        ];
        form.request.args = vec!["--verbose".to_string(), "  ".to_string()];
        form.node_selector = vec![
            ("kubernetes.io/os".to_string(), " linux ".to_string()),
            (" ".to_string(), "ignore".to_string()),
        ];
        let req = build_request(&form);
        assert_eq!(req.name, "api");
        assert_eq!(req.namespace, "default");
        assert_eq!(req.image, "nginx:1.27");
        assert_eq!(req.env.len(), 1);
        assert_eq!(req.args.len(), 1);
        assert_eq!(req.node_selector.len(), 1);
        assert_eq!(
            req.node_selector
                .get("kubernetes.io/os")
                .map(String::as_str),
            Some("linux")
        );
    }

    #[test]
    fn octets_lisibles() {
        assert_eq!(human_bytes(512), "512 o");
        assert_eq!(human_bytes(1536), "1.5 Kio");
        assert_eq!(human_bytes(-1), "—");
    }

    #[test]
    fn troncature_sur_frontiere_de_caractere() {
        assert_eq!(truncate("éééééé", 3), "éé…");
        assert_eq!(truncate("court", 10), "court");
    }

    #[test]
    fn url_de_depot() {
        assert_eq!(repo_url("nginx/nginx"), "https://github.com/nginx/nginx");
        assert_eq!(repo_url("https://gitlab.com/o/d"), "https://gitlab.com/o/d");
    }
}
