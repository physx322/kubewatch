//! Vue « Mises à jour » : surveillance des versions amont, détections,
//! application des mises à jour, inventaire du cluster et réglages de l'updater.
//!
//! Comme toutes les vues, celle-ci ne fait **aucun** appel réseau : elle lit
//! [`AppState`] et envoie des [`Command`] au backend.

use crate::backend::{Backend, Command};
use crate::icons;
use crate::state::AppState;
use crate::views::hub::{
    couleur_avertissement, couleur_erreur, couleur_ok, dispatch, human_date, is_busy, truncate,
};
use kubewatch_updater::model::{
    UpdateChannel, UpdatePolicy, UpdateSeverity, UpdateSource, WatchTarget, WatcherSpec,
};
use kubewatch_updater::store::{UpdaterSettings, SECRET_MASK};

// ---------------------------------------------------------------------------
// Types d'état propres à la vue (référencés par `state::UpdatesState`)
// ---------------------------------------------------------------------------

/// Onglet actif.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdatesTab {
    /// Les surveillances déclarées.
    #[default]
    Watchers,
    /// Les mises à jour détectées.
    Findings,
    /// Inventaire du cluster et suggestions de surveillances.
    Discovery,
    /// Historique des déploiements de mise à jour.
    History,
    /// Réglages du module de mise à jour.
    Settings,
}

/// Les trois origines possibles d'une version amont, mutuellement exclusives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    /// Releases (ou tags) d'un dépôt GitHub.
    #[default]
    Github,
    /// Tags d'une image de conteneur.
    Image,
    /// Versions d'un chart Helm.
    Chart,
}

/// Formulaire de création / modification d'une surveillance.
#[derive(Debug, Clone, Default)]
pub struct WatcherForm {
    /// Le panneau est visible.
    pub open: bool,
    /// Identifiant du watcher modifié ; `None` pour une création.
    pub editing: Option<String>,
    /// Nom lisible.
    pub name: String,
    /// Surveillance active.
    pub enabled: bool,
    /// Cluster porteur de la cible.
    pub cluster: String,
    /// Kind de l'objet surveillé.
    pub kind: String,
    /// Namespace de l'objet (vide = portée cluster).
    pub namespace: String,
    /// Nom de l'objet surveillé.
    pub target_name: String,
    /// Conteneur ciblé (vide = premier conteneur).
    pub container: String,
    /// Mode de source retenu.
    pub source_mode: SourceMode,
    /// Propriétaire du dépôt GitHub.
    pub gh_owner: String,
    /// Nom du dépôt GitHub.
    pub gh_repo: String,
    /// Préfixe de tag à retirer avant l'analyse semver.
    pub gh_tag_prefix: String,
    /// Référence d'image suivie.
    pub image: String,
    /// Dépôt de charts Helm.
    pub chart_repo: String,
    /// Nom du chart Helm.
    pub chart_name: String,
    /// Politique de sélection et d'application.
    pub policy: UpdatePolicy,
    /// Contrainte semver saisie (vide = aucune).
    pub constraint: String,
    /// Exclusions, une par ligne.
    pub ignore_text: String,
    /// Fenêtre de maintenance `HH:MM-HH:MM` (vide = aucune).
    pub maintenance_window: String,
    /// Dernier message de validation.
    pub error: Option<String>,
}

/// Une action destructrice ou risquée en attente de confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatesConfirm {
    /// Suppression d'une surveillance.
    RemoveWatcher {
        /// Identifiant du watcher.
        id: String,
        /// Nom affiché dans la demande de confirmation.
        name: String,
    },
    /// Application d'une mise à jour détectée.
    ApplyFinding {
        /// Identifiant du constat.
        id: String,
        /// Cible lisible (`namespace/Kind/nom`).
        target: String,
        /// Image exacte qui sera déployée.
        image: String,
    },
}

// ---------------------------------------------------------------------------
// Libellés des requêtes en vol
// ---------------------------------------------------------------------------

const LBL_LIST: &str = "Chargement des surveillances";
const LBL_CHECK: &str = "Vérification des versions";
const LBL_SAVE: &str = "Enregistrement de la surveillance";
const LBL_REMOVE: &str = "Suppression de la surveillance";
const LBL_APPLY: &str = "Application de la mise à jour";
const LBL_HISTORY: &str = "Chargement de l'historique";
const LBL_SCAN: &str = "Inventaire du cluster";
const LBL_SUGGEST: &str = "Recherche de surveillances";
const LBL_SETTINGS: &str = "Chargement des réglages";
const LBL_SAVE_SETTINGS: &str = "Enregistrement des réglages";

// ---------------------------------------------------------------------------
// Point d'entrée
// ---------------------------------------------------------------------------

/// Dessine la vue des mises à jour.
pub fn show(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    // Premier affichage : on demande l'état persistant une seule fois.
    if !st.updates.loaded {
        st.updates.loaded = true;
        dispatch(st, backend, LBL_LIST, |id| Command::UpdatesList { id });
        dispatch(st, backend, LBL_SETTINGS, |id| Command::UpdatesSettings {
            id,
        });
    }

    let verification = is_busy(st, LBL_CHECK);
    let mut action_barre: Option<BarAction> = None;

    egui::Panel::top("updates-onglets").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut st.updates.tab, UpdatesTab::Watchers, "Surveillances");
            ui.selectable_value(&mut st.updates.tab, UpdatesTab::Findings, "Détections");
            ui.selectable_value(&mut st.updates.tab, UpdatesTab::Discovery, "Découverte");
            ui.selectable_value(&mut st.updates.tab, UpdatesTab::History, "Historique");
            ui.selectable_value(&mut st.updates.tab, UpdatesTab::Settings, "Réglages");
            ui.separator();
            if ui
                .add_enabled(!verification, egui::Button::new("Vérifier maintenant"))
                .on_hover_text("Interroge toutes les sources amont des surveillances actives.")
                .clicked()
            {
                action_barre = Some(BarAction::Check);
            }
            if ui.button("Rafraîchir").clicked() {
                action_barre = Some(BarAction::Refresh);
            }
            if verification {
                ui.spinner();
                ui.weak("vérification en cours…");
            }
        });
        ui.add_space(4.0);
    });

    match action_barre {
        Some(BarAction::Check) => {
            dispatch(st, backend, LBL_CHECK, |id| Command::UpdatesCheck { id });
        }
        Some(BarAction::Refresh) => {
            dispatch(st, backend, LBL_LIST, |id| Command::UpdatesList { id });
            dispatch(st, backend, LBL_HISTORY, |id| Command::UpdatesHistory {
                id,
            });
        }
        None => {}
    }

    if st.updates.form.open {
        egui::Panel::right("updates-formulaire")
            .resizable(true)
            .default_size(400.0)
            .min_size(320.0)
            .show(ui, |ui| watcher_form(ui, st, backend));
    }

    egui::CentralPanel::default().show(ui, |ui| match st.updates.tab {
        UpdatesTab::Watchers => watchers_tab(ui, st, backend),
        UpdatesTab::Findings => findings_tab(ui, st, backend),
        UpdatesTab::Discovery => discovery_tab(ui, st, backend),
        UpdatesTab::History => history_tab(ui, st, backend),
        UpdatesTab::Settings => settings_tab(ui, st, backend),
    });

    confirm_modal(ui, st, backend);
}

/// Action demandée depuis la barre d'outils.
enum BarAction {
    Check,
    Refresh,
}

// ---------------------------------------------------------------------------
// Onglet « Surveillances »
// ---------------------------------------------------------------------------

/// Action demandée sur une ligne du tableau des surveillances.
enum RowAction {
    Toggle(usize),
    Edit(usize),
    Delete(usize),
}

fn watchers_tab(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let chargement = is_busy(st, LBL_LIST);
    let mut nouveau = false;
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut st.updates.filter)
                .hint_text("Filtrer par nom, cluster ou cible…")
                .desired_width(260.0),
        );
        if ui.button("Nouvelle surveillance").clicked() {
            nouveau = true;
        }
        if chargement {
            ui.spinner();
        }
    });
    if nouveau {
        let cluster = st.cluster().unwrap_or_default().to_string();
        st.updates.form = nouveau_formulaire(cluster, st.updates.settings.as_ref());
    }
    ui.separator();

    if st.updates.watchers.is_empty() {
        ui.add_space(12.0);
        ui.weak(
            "Aucune surveillance déclarée. Utilisez « Nouvelle surveillance », ou l'onglet \
             « Découverte » pour en proposer automatiquement à partir du cluster.",
        );
        return;
    }

    let filtre = st.updates.filter.trim().to_lowercase();
    let mut action: Option<RowAction> = None;
    let couleur_active = couleur_ok(ui);
    let couleur_inactive = ui.visuals().weak_text_color();

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("updates-watchers")
                .num_columns(10)
                .striped(true)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for titre in [
                        "État",
                        "Nom",
                        "Cluster",
                        "Cible",
                        "Conteneur",
                        "Source",
                        "Canal",
                        "Version connue",
                        "Dernière vérification",
                        "Actions",
                    ] {
                        ui.label(egui::RichText::new(titre).strong().small());
                    }
                    ui.end_row();

                    for (i, w) in st.updates.watchers.iter().enumerate() {
                        if !correspond(&filtre, w) {
                            continue;
                        }
                        if w.enabled {
                            ui.colored_label(couleur_active, "actif");
                        } else {
                            ui.colored_label(couleur_inactive, "en pause");
                        }
                        ui.label(truncate(&w.name, 28))
                            .on_hover_text(w.name.clone());
                        ui.label(truncate(&w.cluster, 18));
                        ui.label(truncate(&w.target.label(), 38))
                            .on_hover_text(w.target.label());
                        ui.label(w.container.clone().unwrap_or_else(|| "—".to_string()));
                        ui.label(truncate(&w.source.label(), 36))
                            .on_hover_text(w.source.label());
                        ui.label(canal_libelle(w.policy.channel));
                        ui.label(
                            w.last_known_version
                                .clone()
                                .unwrap_or_else(|| "—".to_string()),
                        );
                        ui.label(human_date(w.last_checked_at));
                        ui.horizontal(|ui| {
                            let bascule = if w.enabled { "Suspendre" } else { "Activer" };
                            if ui.small_button(bascule).clicked() {
                                action = Some(RowAction::Toggle(i));
                            }
                            if ui.small_button("Modifier").clicked() {
                                action = Some(RowAction::Edit(i));
                            }
                            if ui.small_button("Supprimer").clicked() {
                                action = Some(RowAction::Delete(i));
                            }
                        });
                        ui.end_row();
                    }
                });
        });

    // La surveillance visée est copiée avant toute écriture dans `st` : la vue
    // n'a plus alors qu'une valeur détachée entre les mains.
    let vise = match &action {
        Some(RowAction::Toggle(i)) | Some(RowAction::Edit(i)) | Some(RowAction::Delete(i)) => {
            st.updates.watchers.get(*i).cloned()
        }
        None => None,
    };
    match (action, vise) {
        (Some(RowAction::Toggle(_)), Some(mut copie)) => {
            copie.enabled = !copie.enabled;
            dispatch(st, backend, LBL_SAVE, move |id| {
                Command::UpdatesUpsertWatcher {
                    id,
                    watcher: Box::new(copie),
                }
            });
        }
        (Some(RowAction::Edit(_)), Some(w)) => {
            st.updates.form = formulaire_depuis(&w);
        }
        (Some(RowAction::Delete(_)), Some(w)) => {
            st.updates.confirm = Some(UpdatesConfirm::RemoveWatcher {
                id: w.id,
                name: w.name,
            });
        }
        _ => {}
    }
}

/// Vrai si la surveillance satisfait le filtre textuel (déjà en minuscules).
fn correspond(filtre: &str, w: &WatcherSpec) -> bool {
    if filtre.is_empty() {
        return true;
    }
    w.name.to_lowercase().contains(filtre)
        || w.cluster.to_lowercase().contains(filtre)
        || w.target.label().to_lowercase().contains(filtre)
        || w.source.label().to_lowercase().contains(filtre)
}

// ---------------------------------------------------------------------------
// Onglet « Détections »
// ---------------------------------------------------------------------------

fn findings_tab(ui: &mut egui::Ui, st: &mut AppState, _backend: &Backend) {
    let application = is_busy(st, LBL_APPLY);
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!(
            "{} mise(s) à jour détectée(s).",
            st.updates.findings.len()
        ));
        if application {
            ui.spinner();
        }
    });
    ui.separator();

    if st.updates.findings.is_empty() {
        ui.add_space(12.0);
        ui.weak("Rien à signaler : aucune version plus récente n'a été détectée.");
        return;
    }

    let couleur_deja = couleur_ok(ui);
    let mut demande: Option<UpdatesConfirm> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in &st.updates.findings {
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        let largeur = ui.available_width();
                        ui.set_width(largeur);
                        ui.horizontal_wrapped(|ui| {
                            let (libelle, couleur) = severite_style(f.severity);
                            ui.label(
                                egui::RichText::new(libelle)
                                    .small()
                                    .strong()
                                    .color(egui::Color32::WHITE)
                                    .background_color(couleur),
                            );
                            ui.label(egui::RichText::new(&f.watcher_name).strong());
                            ui.weak(f.cluster.clone());
                            ui.weak(f.target().label());
                            if let Some(c) = &f.container {
                                ui.weak(format!("conteneur {c}"));
                            }
                        });

                        ui.horizontal_wrapped(|ui| {
                            let actuelle = f
                                .current_version
                                .clone()
                                .or_else(|| f.current_image.clone())
                                .unwrap_or_else(|| "version inconnue".to_string());
                            ui.label(egui::RichText::new(actuelle).monospace());
                            ui.label("→");
                            ui.label(
                                egui::RichText::new(&f.available_version)
                                    .monospace()
                                    .strong(),
                            );
                            ui.weak(format!("détecté le {}", human_date(Some(f.detected_at))));
                        });

                        if let Some(image) = &f.available_image {
                            ui.label(egui::RichText::new(image).monospace().small());
                        }

                        // Les notes de version viennent de GitHub : texte brut,
                        // jamais interprété comme du Markdown ni comme un lien.
                        if let Some(release) = &f.release {
                            if let Some(corps) =
                                release.body.as_ref().filter(|b| !b.trim().is_empty())
                            {
                                let titre =
                                    release.name.clone().unwrap_or_else(|| release.tag.clone());
                                ui.collapsing(format!("Notes de version — {titre}"), |ui| {
                                    egui::ScrollArea::vertical()
                                        .id_salt(("notes", &f.id))
                                        .max_height(220.0)
                                        .show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new(texte_brut(corps))
                                                    .monospace()
                                                    .small(),
                                            );
                                        });
                                });
                            }
                            if let Some(url) = &release.html_url {
                                ui.hyperlink_to("Page de la release", url);
                            }
                        }

                        ui.horizontal(|ui| {
                            if f.applied {
                                ui.colored_label(couleur_deja, "déjà appliquée");
                            } else if ui
                                .add_enabled(!application, egui::Button::new("Appliquer"))
                                .clicked()
                            {
                                demande = Some(UpdatesConfirm::ApplyFinding {
                                    id: f.id.clone(),
                                    target: f.target().label(),
                                    image: f
                                        .available_image
                                        .clone()
                                        .unwrap_or_else(|| f.available_version.clone()),
                                });
                            }
                        });
                    });
            }
        });

    if let Some(d) = demande {
        st.updates.confirm = Some(d);
    }
}

// ---------------------------------------------------------------------------
// Onglet « Découverte »
// ---------------------------------------------------------------------------

fn discovery_tab(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let scan_en_cours = is_busy(st, LBL_SCAN);
    let suggest_en_cours = is_busy(st, LBL_SUGGEST);
    let cluster = st.cluster().map(|c| c.to_string());

    let mut scanner = false;
    let mut suggerer = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("Namespace");
        ui.add(
            egui::TextEdit::singleline(&mut st.updates.scan_namespace)
                .hint_text("tous")
                .desired_width(160.0),
        );
        if ui
            .add_enabled(
                cluster.is_some() && !scan_en_cours,
                egui::Button::new("Scanner le cluster"),
            )
            .clicked()
        {
            scanner = true;
        }
        if ui
            .add_enabled(
                cluster.is_some() && !suggest_en_cours,
                egui::Button::new("Suggérer des surveillances"),
            )
            .clicked()
        {
            suggerer = true;
        }
        if scan_en_cours || suggest_en_cours {
            ui.spinner();
        }
    });

    if cluster.is_none() {
        let couleur = couleur_avertissement(ui);
        ui.colored_label(
            couleur,
            "Sélectionnez un cluster dans les réglages pour explorer ses charges de travail.",
        );
        return;
    }

    let namespace = {
        let ns = st.updates.scan_namespace.trim();
        if ns.is_empty() {
            None
        } else {
            Some(ns.to_string())
        }
    };
    if scanner {
        if let Some(c) = cluster.clone() {
            let ns = namespace.clone();
            dispatch(st, backend, LBL_SCAN, move |id| Command::UpdatesScan {
                id,
                cluster: c,
                namespace: ns,
            });
        }
    }
    if suggerer {
        if let Some(c) = cluster.clone() {
            let ns = namespace.clone();
            dispatch(st, backend, LBL_SUGGEST, move |id| {
                Command::UpdatesSuggest {
                    id,
                    cluster: c,
                    namespace: ns,
                }
            });
        }
    }

    ui.separator();

    // --- suggestions
    if !st.updates.suggestions.is_empty() {
        // Le tableau de cases à cocher suit la liste : on le remet à la bonne
        // taille avant de l'indexer.
        let n = st.updates.suggestions.len();
        st.updates.suggestion_keep.resize(n, true);

        ui.label(
            egui::RichText::new(format!("{n} surveillance(s) proposée(s)"))
                .heading()
                .size(15.0),
        );
        let mut tout = false;
        let mut rien = false;
        let mut enregistrer = false;
        ui.horizontal(|ui| {
            if ui.small_button("Tout cocher").clicked() {
                tout = true;
            }
            if ui.small_button("Tout décocher").clicked() {
                rien = true;
            }
            if ui.button("Enregistrer la sélection").clicked() {
                enregistrer = true;
            }
        });
        if tout {
            st.updates
                .suggestion_keep
                .iter_mut()
                .for_each(|k| *k = true);
        }
        if rien {
            st.updates
                .suggestion_keep
                .iter_mut()
                .for_each(|k| *k = false);
        }

        egui::ScrollArea::vertical()
            .id_salt("updates-suggestions")
            .max_height(240.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("updates-suggestions-grid")
                    .num_columns(4)
                    .striped(true)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        for (i, w) in st.updates.suggestions.iter().enumerate() {
                            if let Some(garde) = st.updates.suggestion_keep.get_mut(i) {
                                ui.checkbox(garde, "");
                            } else {
                                ui.label("");
                            }
                            ui.label(truncate(&w.name, 30));
                            ui.label(truncate(&w.target.label(), 40));
                            ui.label(truncate(&w.source.label(), 40));
                            ui.end_row();
                        }
                    });
            });

        if enregistrer {
            let retenus: Vec<WatcherSpec> = st
                .updates
                .suggestions
                .iter()
                .enumerate()
                .filter(|(i, _)| st.updates.suggestion_keep.get(*i).copied().unwrap_or(false))
                .map(|(_, w)| w.clone())
                .collect();
            for w in retenus {
                dispatch(st, backend, LBL_SAVE, move |id| {
                    Command::UpdatesUpsertWatcher {
                        id,
                        watcher: Box::new(w),
                    }
                });
            }
            st.updates.suggestions.clear();
            st.updates.suggestion_keep.clear();
            dispatch(st, backend, LBL_LIST, |id| Command::UpdatesList { id });
        }
        ui.separator();
    }

    // --- inventaire des images déployées
    if st.updates.workload_images.is_empty() {
        ui.add_space(8.0);
        ui.weak("Lancez un scan pour lister les images déployées dans le cluster.");
        return;
    }

    ui.label(
        egui::RichText::new(format!(
            "{} image(s) déployée(s)",
            st.updates.workload_images.len()
        ))
        .heading()
        .size(15.0),
    );
    egui::ScrollArea::both()
        .id_salt("updates-inventaire")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("updates-inventaire-grid")
                .num_columns(6)
                .striped(true)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for titre in [
                        "Namespace",
                        "Type",
                        "Nom",
                        "Conteneur",
                        "Image",
                        "Dépôt source probable",
                    ] {
                        ui.label(egui::RichText::new(titre).strong().small());
                    }
                    ui.end_row();
                    for w in &st.updates.workload_images {
                        ui.label(w.namespace.clone().unwrap_or_else(|| "—".to_string()));
                        ui.label(w.kind.clone());
                        ui.label(truncate(&w.name, 30));
                        ui.label(truncate(&w.container, 24));
                        ui.label(egui::RichText::new(truncate(&w.image, 46)).monospace())
                            .on_hover_text(w.image.clone());
                        ui.label(
                            w.source_repo_guess
                                .clone()
                                .unwrap_or_else(|| "—".to_string()),
                        );
                        ui.end_row();
                    }
                });
        });
}

// ---------------------------------------------------------------------------
// Onglet « Historique »
// ---------------------------------------------------------------------------

fn history_tab(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let chargement = is_busy(st, LBL_HISTORY);
    let mut recharger = false;
    ui.horizontal(|ui| {
        ui.weak("Déploiements de mise à jour effectués par KubeWatch.");
        if ui.small_button("Recharger").clicked() {
            recharger = true;
        }
        if chargement {
            ui.spinner();
        }
    });
    if recharger {
        dispatch(st, backend, LBL_HISTORY, |id| Command::UpdatesHistory {
            id,
        });
    }
    ui.separator();

    if st.updates.history.is_empty() {
        ui.add_space(12.0);
        ui.weak("Aucun déploiement enregistré.");
        return;
    }

    let vert = couleur_ok(ui);
    let rouge = couleur_erreur(ui);
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("updates-historique")
                .num_columns(5)
                .striped(true)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for titre in [
                        "Date",
                        "Cible",
                        "Image précédente",
                        "Nouvelle image",
                        "Statut",
                    ] {
                        ui.label(egui::RichText::new(titre).strong().small());
                    }
                    ui.end_row();
                    // Du plus récent au plus ancien.
                    for r in st.updates.history.iter().rev() {
                        ui.label(human_date(Some(r.applied_at)));
                        ui.label(truncate(&r.target.label(), 40));
                        ui.label(
                            egui::RichText::new(truncate(
                                r.previous_image.as_deref().unwrap_or("—"),
                                40,
                            ))
                            .monospace(),
                        );
                        ui.label(egui::RichText::new(truncate(&r.new_image, 40)).monospace())
                            .on_hover_text(r.new_image.clone());
                        let couleur = if r.succeeded() { vert } else { rouge };
                        ui.colored_label(couleur, truncate(&r.status, 40))
                            .on_hover_text(r.status.clone());
                        ui.end_row();
                    }
                });
        });
}

// ---------------------------------------------------------------------------
// Onglet « Réglages de l'updater »
// ---------------------------------------------------------------------------

fn settings_tab(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let chargement = is_busy(st, LBL_SETTINGS) || is_busy(st, LBL_SAVE_SETTINGS);
    let mut recharger = false;
    ui.horizontal(|ui| {
        ui.weak("Réglages du module de mise à jour.");
        if ui.small_button("Recharger").clicked() {
            recharger = true;
        }
        if chargement {
            ui.spinner();
        }
    });
    if recharger {
        st.updates.github_token_input = None;
        st.updates.webhook_secret_input = None;
        dispatch(st, backend, LBL_SETTINGS, |id| Command::UpdatesSettings {
            id,
        });
    }
    ui.separator();

    if st.updates.settings.is_none() {
        ui.add_space(12.0);
        ui.weak("Réglages non chargés.");
        return;
    }

    let mut enregistrer = false;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // --- secrets
            ui.label(egui::RichText::new("Secrets").strong());
            ui.weak(
                "Les secrets sont conservés côté application et ne sont jamais réaffichés en \
                 clair : le masque signifie « inchangé ».",
            );
            let jeton_actuel = st
                .updates
                .settings
                .as_ref()
                .and_then(|s| s.github_token.clone());
            champ_secret(
                ui,
                "Jeton GitHub",
                jeton_actuel.as_deref(),
                &mut st.updates.github_token_input,
            );
            let secret_actuel = st
                .updates
                .settings
                .as_ref()
                .and_then(|s| s.webhook_secret.clone());
            champ_secret(
                ui,
                "Secret de webhook",
                secret_actuel.as_deref(),
                &mut st.updates.webhook_secret_input,
            );

            ui.add_space(8.0);
            ui.separator();

            // --- ordonnanceur et politique par défaut
            if let Some(reglages) = st.updates.settings.as_mut() {
                ui.checkbox(
                    &mut reglages.scheduler_enabled,
                    "Vérification périodique en tâche de fond",
                );
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Politique par défaut").strong());
                ui.weak("Appliquée aux nouvelles surveillances.");
                editeur_politique(ui, "updater-politique", &mut reglages.default_policy);
            }

            ui.add_space(10.0);
            if ui.button("Enregistrer les réglages").clicked() {
                enregistrer = true;
            }
        });

    if enregistrer {
        if let Some(base) = st.updates.settings.clone() {
            let mut suivant = base;
            // Un champ non modifié repart avec le masque : le magasin comprend
            // « inchangé » et conserve la valeur existante.
            if let Some(saisi) = st.updates.github_token_input.clone() {
                suivant.github_token = Some(saisi);
            }
            if let Some(saisi) = st.updates.webhook_secret_input.clone() {
                suivant.webhook_secret = Some(saisi);
            }
            st.updates.github_token_input = None;
            st.updates.webhook_secret_input = None;
            dispatch(st, backend, LBL_SAVE_SETTINGS, move |id| {
                Command::UpdatesSaveSettings {
                    id,
                    settings: Box::new(suivant),
                }
            });
        }
    }
}

/// Champ de secret : la valeur stockée n'est jamais réaffichée, seul un
/// remplacement explicite produit une nouvelle valeur.
///
/// Aucune de ces chaînes n'est journalisée.
fn champ_secret(ui: &mut egui::Ui, titre: &str, actuel: Option<&str>, saisie: &mut Option<String>) {
    let mut activer = false;
    let mut annuler = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(titre);
        if let Some(tampon) = saisie.as_mut() {
            ui.add(
                egui::TextEdit::singleline(tampon)
                    .password(true)
                    .hint_text("nouvelle valeur (vide = effacer)")
                    .desired_width(220.0),
            );
            if ui.small_button("Annuler").clicked() {
                annuler = true;
            }
        } else {
            // Le magasin renvoie déjà un masque : on n'affiche jamais autre chose.
            if actuel.is_some_and(|v| !v.is_empty()) {
                ui.label(egui::RichText::new(SECRET_MASK).monospace());
            } else {
                ui.weak("non défini");
            }
            if ui.small_button("Modifier").clicked() {
                activer = true;
            }
        }
    });
    if activer {
        *saisie = Some(String::new());
    }
    if annuler {
        *saisie = None;
    }
}

// ---------------------------------------------------------------------------
// Formulaire de surveillance
// ---------------------------------------------------------------------------

fn watcher_form(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let titre = if st.updates.form.editing.is_some() {
        "Modifier la surveillance"
    } else {
        "Nouvelle surveillance"
    };
    ui.horizontal(|ui| {
        ui.heading(titre);
        if ui
            .small_button(icons::CLOSE)
            .on_hover_text("Fermer")
            .clicked()
        {
            st.updates.form.open = false;
        }
    });
    if !st.updates.form.open {
        return;
    }
    ui.separator();

    let noms_clusters: Vec<String> = st.clusters.iter().map(|c| c.name.clone()).collect();
    let mut valider = false;
    egui::ScrollArea::vertical()
        .id_salt("updates-form-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let f = &mut st.updates.form;

            ui.label(egui::RichText::new("Cible").strong());
            egui::Grid::new("updates-form-cible")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Nom");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.name)
                            .hint_text("nginx production")
                            .desired_width(220.0),
                    );
                    ui.end_row();

                    ui.label("Cluster");
                    egui::ComboBox::from_id_salt("updates-form-cluster")
                        .selected_text(if f.cluster.is_empty() {
                            "—".to_string()
                        } else {
                            f.cluster.clone()
                        })
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for nom in &noms_clusters {
                                ui.selectable_value(&mut f.cluster, nom.clone(), nom);
                            }
                        });
                    ui.end_row();

                    ui.label("Kind");
                    egui::ComboBox::from_id_salt("updates-form-kind")
                        .selected_text(if f.kind.is_empty() {
                            "—".to_string()
                        } else {
                            f.kind.clone()
                        })
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for k in ["Deployment", "StatefulSet", "DaemonSet", "CronJob"] {
                                ui.selectable_value(&mut f.kind, k.to_string(), k);
                            }
                        });
                    ui.end_row();

                    ui.label("Namespace");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.namespace)
                            .hint_text("default")
                            .desired_width(220.0),
                    );
                    ui.end_row();

                    ui.label("Nom de l'objet");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.target_name)
                            .hint_text("mon-api")
                            .desired_width(220.0),
                    );
                    ui.end_row();

                    ui.label("Conteneur");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.container)
                            .hint_text("premier conteneur")
                            .desired_width(220.0),
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.label(egui::RichText::new("Source des versions").strong());
            ui.horizontal_wrapped(|ui| {
                ui.radio_value(&mut f.source_mode, SourceMode::Github, "Release GitHub");
                ui.radio_value(&mut f.source_mode, SourceMode::Image, "Image");
                ui.radio_value(&mut f.source_mode, SourceMode::Chart, "Chart Helm");
            });
            egui::Grid::new("updates-form-source")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| match f.source_mode {
                    SourceMode::Github => {
                        ui.label("Propriétaire");
                        ui.add(
                            egui::TextEdit::singleline(&mut f.gh_owner)
                                .hint_text("kubernetes")
                                .desired_width(220.0),
                        );
                        ui.end_row();
                        ui.label("Dépôt");
                        ui.add(
                            egui::TextEdit::singleline(&mut f.gh_repo)
                                .hint_text("ingress-nginx")
                                .desired_width(220.0),
                        );
                        ui.end_row();
                        ui.label("Préfixe de tag");
                        ui.add(
                            egui::TextEdit::singleline(&mut f.gh_tag_prefix)
                                .hint_text("controller-v")
                                .desired_width(220.0),
                        );
                        ui.end_row();
                    }
                    SourceMode::Image => {
                        ui.label("Image");
                        ui.add(
                            egui::TextEdit::singleline(&mut f.image)
                                .hint_text("nginx:1.27")
                                .desired_width(220.0),
                        );
                        ui.end_row();
                    }
                    SourceMode::Chart => {
                        ui.label("Dépôt de charts");
                        ui.add(
                            egui::TextEdit::singleline(&mut f.chart_repo)
                                .hint_text("https://charts.bitnami.com/bitnami")
                                .desired_width(220.0),
                        );
                        ui.end_row();
                        ui.label("Chart");
                        ui.add(
                            egui::TextEdit::singleline(&mut f.chart_name)
                                .hint_text("postgresql")
                                .desired_width(220.0),
                        );
                        ui.end_row();
                    }
                });

            ui.add_space(8.0);
            ui.label(egui::RichText::new("Politique").strong());
            ui.checkbox(&mut f.enabled, "Surveillance active");
            editeur_politique(ui, "updates-form-politique", &mut f.policy);

            egui::Grid::new("updates-form-politique-texte")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Contrainte semver");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.constraint)
                            .hint_text(">=1.2, <2")
                            .desired_width(220.0),
                    );
                    ui.end_row();

                    ui.label("Fenêtre de maintenance");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.maintenance_window)
                            .hint_text("01:00-05:00 (UTC)")
                            .desired_width(220.0),
                    );
                    ui.end_row();
                });
            ui.label("Exclusions (une par ligne)");
            ui.add(
                egui::TextEdit::multiline(&mut f.ignore_text)
                    .hint_text("1.2.3\nlatest")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );

            if let Some(err) = f.error.clone() {
                ui.add_space(6.0);
                let rouge = couleur_erreur(ui);
                ui.colored_label(rouge, format!("{} {err}", icons::WARNING));
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Enregistrer").clicked() {
                    valider = true;
                }
                if ui.button("Annuler").clicked() {
                    f.open = false;
                }
            });
        });

    if valider {
        let existant = st
            .updates
            .form
            .editing
            .as_ref()
            .and_then(|id| st.updates.watchers.iter().find(|w| &w.id == id))
            .cloned();
        match construire_watcher(&st.updates.form, existant.as_ref()) {
            Ok(spec) => {
                st.updates.form.error = None;
                st.updates.form.open = false;
                dispatch(st, backend, LBL_SAVE, move |id| {
                    Command::UpdatesUpsertWatcher {
                        id,
                        watcher: Box::new(spec),
                    }
                });
                dispatch(st, backend, LBL_LIST, |id| Command::UpdatesList { id });
            }
            Err(msg) => st.updates.form.error = Some(msg),
        }
    }
}

/// Éditeur commun de politique (canal, préversions, automatisme, période).
fn editeur_politique(ui: &mut egui::Ui, sel: &str, politique: &mut UpdatePolicy) {
    ui.push_id(sel, |ui| {
        egui::Grid::new("politique-grid")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Canal");
                egui::ComboBox::from_id_salt("canal")
                    .selected_text(canal_libelle(politique.channel))
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for c in [
                            UpdateChannel::Major,
                            UpdateChannel::Minor,
                            UpdateChannel::Patch,
                            UpdateChannel::Prerelease,
                            UpdateChannel::Pinned,
                        ] {
                            ui.selectable_value(&mut politique.channel, c, canal_libelle(c));
                        }
                    });
                ui.end_row();

                ui.label("Période de vérification");
                ui.add(
                    egui::DragValue::new(&mut politique.check_interval_seconds)
                        .range(60u64..=604_800u64)
                        .speed(60.0)
                        .suffix(" s"),
                );
                ui.end_row();
            });
        ui.checkbox(&mut politique.allow_prerelease, "Accepter les pré-versions");
        ui.checkbox(
            &mut politique.auto_apply,
            "Appliquer automatiquement les mises à jour détectées",
        );
    });
}

/// Formulaire vierge, préréglé sur le cluster courant et la politique par défaut.
fn nouveau_formulaire(cluster: String, reglages: Option<&UpdaterSettings>) -> WatcherForm {
    WatcherForm {
        open: true,
        editing: None,
        name: String::new(),
        enabled: true,
        cluster,
        kind: "Deployment".to_string(),
        namespace: String::new(),
        target_name: String::new(),
        container: String::new(),
        source_mode: SourceMode::Image,
        gh_owner: String::new(),
        gh_repo: String::new(),
        gh_tag_prefix: String::new(),
        image: String::new(),
        chart_repo: String::new(),
        chart_name: String::new(),
        policy: reglages
            .map(|s| s.default_policy.clone())
            .unwrap_or_default(),
        constraint: String::new(),
        ignore_text: String::new(),
        maintenance_window: String::new(),
        error: None,
    }
}

/// Formulaire prérempli à partir d'une surveillance existante.
fn formulaire_depuis(w: &WatcherSpec) -> WatcherForm {
    let mut f = WatcherForm {
        open: true,
        editing: Some(w.id.clone()),
        name: w.name.clone(),
        enabled: w.enabled,
        cluster: w.cluster.clone(),
        kind: w.target.kind.clone(),
        namespace: w.target.namespace.clone().unwrap_or_default(),
        target_name: w.target.name.clone(),
        container: w.container.clone().unwrap_or_default(),
        source_mode: SourceMode::Image,
        gh_owner: String::new(),
        gh_repo: String::new(),
        gh_tag_prefix: String::new(),
        image: String::new(),
        chart_repo: String::new(),
        chart_name: String::new(),
        policy: w.policy.clone(),
        constraint: w.policy.constraint.clone().unwrap_or_default(),
        ignore_text: w.policy.ignore.join("\n"),
        maintenance_window: w.policy.maintenance_window.clone().unwrap_or_default(),
        error: None,
    };
    match &w.source {
        UpdateSource::GithubRelease {
            owner,
            repo,
            tag_prefix,
        } => {
            f.source_mode = SourceMode::Github;
            f.gh_owner = owner.clone();
            f.gh_repo = repo.clone();
            f.gh_tag_prefix = tag_prefix.clone().unwrap_or_default();
        }
        UpdateSource::ContainerRegistry { image } => {
            f.source_mode = SourceMode::Image;
            f.image = image.clone();
        }
        UpdateSource::HelmChart { repo, chart } => {
            f.source_mode = SourceMode::Chart;
            f.chart_repo = repo.clone();
            f.chart_name = chart.clone();
        }
    }
    f
}

/// Construit la surveillance à enregistrer, ou explique en français ce qui manque.
fn construire_watcher(
    f: &WatcherForm,
    existant: Option<&WatcherSpec>,
) -> Result<WatcherSpec, String> {
    let name = f.name.trim();
    if name.is_empty() {
        return Err("Donnez un nom à la surveillance.".to_string());
    }
    let cluster = f.cluster.trim();
    if cluster.is_empty() {
        return Err("Choisissez le cluster qui porte la cible.".to_string());
    }
    let kind = f.kind.trim();
    if kind.is_empty() {
        return Err("Choisissez le type de l'objet surveillé.".to_string());
    }
    let target_name = f.target_name.trim();
    if target_name.is_empty() {
        return Err("Indiquez le nom de l'objet surveillé.".to_string());
    }

    let source = match f.source_mode {
        SourceMode::Github => {
            let owner = f.gh_owner.trim();
            let repo = f.gh_repo.trim();
            if owner.is_empty() || repo.is_empty() {
                return Err("Renseignez le propriétaire et le nom du dépôt GitHub.".to_string());
            }
            UpdateSource::GithubRelease {
                owner: owner.to_string(),
                repo: repo.to_string(),
                tag_prefix: vide_en_none(&f.gh_tag_prefix),
            }
        }
        SourceMode::Image => {
            let image = f.image.trim();
            if image.is_empty() {
                return Err("Indiquez la référence d'image à suivre.".to_string());
            }
            UpdateSource::ContainerRegistry {
                image: image.to_string(),
            }
        }
        SourceMode::Chart => {
            let repo = f.chart_repo.trim();
            let chart = f.chart_name.trim();
            if repo.is_empty() || chart.is_empty() {
                return Err("Renseignez le dépôt et le nom du chart Helm.".to_string());
            }
            UpdateSource::HelmChart {
                repo: repo.to_string(),
                chart: chart.to_string(),
            }
        }
    };

    let mut policy = f.policy.clone();
    policy.constraint = vide_en_none(&f.constraint);
    policy.maintenance_window = vide_en_none(&f.maintenance_window);
    policy.ignore = f
        .ignore_text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    policy.check_interval_seconds = policy.check_interval_seconds.max(60);

    let target = WatchTarget {
        namespace: vide_en_none(&f.namespace),
        kind: kind.to_string(),
        name: target_name.to_string(),
    };

    Ok(WatcherSpec {
        id: existant
            .map(|w| w.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        name: name.to_string(),
        enabled: f.enabled,
        cluster: cluster.to_string(),
        target,
        container: vide_en_none(&f.container),
        source,
        policy,
        // Une modification conserve l'historique de la surveillance.
        created_at: existant
            .map(|w| w.created_at)
            .unwrap_or_else(chrono::Utc::now),
        last_checked_at: existant.and_then(|w| w.last_checked_at),
        last_known_version: existant.and_then(|w| w.last_known_version.clone()),
    })
}

// ---------------------------------------------------------------------------
// Confirmation
// ---------------------------------------------------------------------------

fn confirm_modal(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let demande = match st.updates.confirm.clone() {
        Some(d) => d,
        None => return,
    };
    let ctx = ui.ctx().clone();
    let rouge = couleur_erreur(ui);
    let mut fermer = false;
    let mut confirmer = false;

    let reponse = egui::Modal::new(egui::Id::new("updates-confirmation")).show(&ctx, |ui| {
        ui.set_width(440.0);
        match &demande {
            UpdatesConfirm::RemoveWatcher { name, .. } => {
                ui.heading("Supprimer la surveillance");
                ui.add_space(6.0);
                ui.label(format!(
                    "« {name} » sera supprimée, ainsi que ses détections en attente."
                ));
                ui.weak("Aucun objet du cluster n'est modifié.");
            }
            UpdatesConfirm::ApplyFinding { target, image, .. } => {
                ui.heading("Appliquer la mise à jour");
                ui.add_space(6.0);
                ui.label(format!("Cible : {target}"));
                ui.horizontal_wrapped(|ui| {
                    ui.label("Image déployée :");
                    ui.label(egui::RichText::new(image).monospace().strong());
                });
                ui.weak("Le déploiement sera mis à jour immédiatement.");
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Annuler").clicked() {
                fermer = true;
            }
            if ui
                .button(egui::RichText::new("Confirmer").color(rouge).strong())
                .clicked()
            {
                confirmer = true;
            }
        });
    });

    if reponse.should_close() {
        fermer = true;
    }

    if confirmer {
        match demande {
            UpdatesConfirm::RemoveWatcher { id, .. } => {
                dispatch(st, backend, LBL_REMOVE, move |rid| {
                    Command::UpdatesRemoveWatcher {
                        id: rid,
                        watcher_id: id,
                    }
                });
                dispatch(st, backend, LBL_LIST, |rid| Command::UpdatesList {
                    id: rid,
                });
            }
            UpdatesConfirm::ApplyFinding { id, .. } => {
                dispatch(st, backend, LBL_APPLY, move |rid| Command::UpdatesApply {
                    id: rid,
                    finding_id: id,
                });
            }
        }
        st.updates.confirm = None;
    } else if fermer {
        st.updates.confirm = None;
    }
}

// ---------------------------------------------------------------------------
// Utilitaires
// ---------------------------------------------------------------------------

/// Libellé français d'un canal de mise à jour.
fn canal_libelle(c: UpdateChannel) -> &'static str {
    match c {
        UpdateChannel::Major => "majeure",
        UpdateChannel::Minor => "mineure",
        UpdateChannel::Patch => "correctif",
        UpdateChannel::Prerelease => "pré-version",
        UpdateChannel::Pinned => "figée",
    }
}

/// Libellé et couleur du badge de sévérité.
fn severite_style(s: UpdateSeverity) -> (&'static str, egui::Color32) {
    match s {
        UpdateSeverity::Major => (" MAJEURE ", egui::Color32::from_rgb(190, 50, 50)),
        UpdateSeverity::Minor => (" MINEURE ", egui::Color32::from_rgb(200, 130, 25)),
        UpdateSeverity::Patch => (" CORRECTIF ", egui::Color32::from_rgb(45, 135, 70)),
        UpdateSeverity::Unknown => (" INCONNUE ", egui::Color32::from_rgb(110, 110, 110)),
    }
}

/// `None` pour une saisie vide, sinon la valeur détourée.
fn vide_en_none(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Neutralise un texte venu d'Internet avant affichage.
///
/// Les notes de version proviennent de GitHub : elles sont affichées telles
/// quelles, sans interprétation Markdown. On retire seulement les caractères de
/// contrôle (qui casseraient le rendu) et on borne la longueur.
fn texte_brut(brut: &str) -> String {
    const MAX: usize = 20_000;
    let mut out = String::with_capacity(brut.len().min(MAX));
    for c in brut.chars() {
        if out.len() >= MAX {
            out.push_str("\n… (notes tronquées)");
            break;
        }
        if c == '\n' || c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formulaire_valide() -> WatcherForm {
        let mut f = nouveau_formulaire("prod".to_string(), None);
        f.name = "nginx".to_string();
        f.kind = "Deployment".to_string();
        f.namespace = "web".to_string();
        f.target_name = "nginx".to_string();
        f.source_mode = SourceMode::Image;
        f.image = "nginx:1.27".to_string();
        f
    }

    #[test]
    fn construction_dun_watcher_complet() {
        let f = formulaire_valide();
        let w = construire_watcher(&f, None).expect("le formulaire est valide");
        assert_eq!(w.name, "nginx");
        assert_eq!(w.cluster, "prod");
        assert_eq!(w.target.label(), "web/Deployment/nginx");
        assert!(matches!(w.source, UpdateSource::ContainerRegistry { .. }));
        assert!(w.enabled);
    }

    #[test]
    fn champs_manquants_refuses_en_francais() {
        let mut f = formulaire_valide();
        f.name = "   ".to_string();
        let e = construire_watcher(&f, None).expect_err("nom vide");
        assert!(e.contains("nom"));

        let mut f = formulaire_valide();
        f.image = String::new();
        let e = construire_watcher(&f, None).expect_err("image vide");
        assert!(e.contains("image"));

        let mut f = formulaire_valide();
        f.source_mode = SourceMode::Github;
        let e = construire_watcher(&f, None).expect_err("dépôt vide");
        assert!(e.contains("dépôt"));
    }

    #[test]
    fn modification_conserve_identifiant_et_historique() {
        let f = formulaire_valide();
        let initial = construire_watcher(&f, None).expect("création");
        let mut f2 = formulaire_depuis(&initial);
        f2.name = "nginx renommé".to_string();
        let modifie = construire_watcher(&f2, Some(&initial)).expect("modification");
        assert_eq!(modifie.id, initial.id);
        assert_eq!(modifie.created_at, initial.created_at);
        assert_eq!(modifie.name, "nginx renommé");
    }

    #[test]
    fn aller_retour_des_trois_sources() {
        for source in [
            UpdateSource::GithubRelease {
                owner: "o".into(),
                repo: "r".into(),
                tag_prefix: Some("v".into()),
            },
            UpdateSource::ContainerRegistry {
                image: "nginx".into(),
            },
            UpdateSource::HelmChart {
                repo: "https://charts".into(),
                chart: "redis".into(),
            },
        ] {
            let w = WatcherSpec::new(
                "w",
                "prod",
                WatchTarget::new("Deployment", Some("ns".into()), "app"),
                source.clone(),
                UpdatePolicy::default(),
            );
            let f = formulaire_depuis(&w);
            let retour = construire_watcher(&f, Some(&w)).expect("reconstruction");
            assert_eq!(retour.source, source);
        }
    }

    #[test]
    fn politique_normalisee() {
        let mut f = formulaire_valide();
        f.constraint = "  ".to_string();
        f.ignore_text = "1.0.0\n\n  2.0.0  \n".to_string();
        f.policy.check_interval_seconds = 5;
        let w = construire_watcher(&f, None).expect("valide");
        assert!(w.policy.constraint.is_none());
        assert_eq!(w.policy.ignore, vec!["1.0.0", "2.0.0"]);
        assert_eq!(w.policy.check_interval_seconds, 60);
    }

    #[test]
    fn notes_de_version_bornees_et_nettoyees() {
        let brut = format!("ligne\u{0007}1\nligne2{}", "x".repeat(30_000));
        let net = texte_brut(&brut);
        assert!(!net.contains('\u{0007}'));
        assert!(net.contains('\n'));
        assert!(net.ends_with("(notes tronquées)"));
    }

    #[test]
    fn filtre_insensible_a_la_casse() {
        let w = WatcherSpec::new(
            "NGINX Prod",
            "Production",
            WatchTarget::new("Deployment", Some("web".into()), "nginx"),
            UpdateSource::ContainerRegistry {
                image: "nginx".into(),
            },
            UpdatePolicy::default(),
        );
        assert!(correspond("", &w));
        assert!(correspond("nginx", &w));
        assert!(correspond("production", &w));
        assert!(!correspond("redis", &w));
    }
}
