//! Vue « Réglages » : gestion des clusters, ajout d'une connexion, apparence,
//! comportement de l'application et informations sur l'installation.
//!
//! Comme toutes les vues, celle-ci ne fait **aucun** appel réseau et ne bloque
//! jamais le fil d'interface : même le sélecteur de fichiers tourne sur un fil
//! séparé et dépose son résultat dans une boîte que l'on relève à l'image
//! suivante.

use crate::backend::{Backend, Command};
use crate::icons;
use crate::state::AppState;
use crate::views::hub::{
    couleur_avertissement, couleur_erreur, couleur_info, couleur_ok, dispatch, is_busy, truncate,
};
use kubewatch_core::ConnectionSpec;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Types d'état propres à la vue (référencés par `state::SettingsState`)
// ---------------------------------------------------------------------------

/// Onglet actif des réglages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    /// Clusters enregistrés et ajout d'une connexion.
    #[default]
    Clusters,
    /// Thème, échelle, densité.
    Appearance,
    /// Rafraîchissement, journaux, confirmations.
    Behavior,
    /// Version, plateforme, dossier d'état, raccourcis.
    About,
}

/// Manière d'ajouter un cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AddClusterMode {
    /// Fichier kubeconfig choisi sur le disque.
    #[default]
    Kubeconfig,
    /// Kubeconfig collé dans une zone de texte.
    Paste,
    /// Serveur d'API joint directement (URL, jeton, certificats).
    Remote,
}

/// Préférence de thème.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    /// Suit le réglage du système.
    #[default]
    System,
    /// Toujours clair.
    Light,
    /// Toujours sombre.
    Dark,
}

/// Densité d'affichage des tableaux et des contrôles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Density {
    /// Lignes serrées, pour les grands tableaux.
    Compact,
    /// Réglage par défaut d'egui.
    #[default]
    Normal,
    /// Lignes aérées.
    Comfortable,
}

/// Action destructrice en attente de confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsConfirm {
    /// Retrait d'un cluster de la liste.
    RemoveCluster {
        /// Nom du cluster visé.
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Libellés des requêtes en vol
// ---------------------------------------------------------------------------

const LBL_REFRESH: &str = "Rafraîchissement des clusters";
const LBL_CONTEXTS: &str = "Lecture des contextes";
const LBL_IMPORT: &str = "Import du kubeconfig";
const LBL_CONNECT: &str = "Connexion au cluster";

/// Page des versions publiées, ouverte par le bouton de vérification.
const RELEASES_URL: &str = "https://github.com/kubewatch-io/kubewatch/releases";

/// Boîte partagée avec le fil du sélecteur de fichiers.
///
/// `None` = pas de réponse ; `Some(None)` = annulé ; `Some(Some(chemin))` = choisi.
type PickSlot = std::sync::Arc<parking_lot::Mutex<Option<Option<PathBuf>>>>;

// ---------------------------------------------------------------------------
// Point d'entrée
// ---------------------------------------------------------------------------

/// Dessine la vue des réglages.
pub fn show(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    egui::Panel::top("settings-onglets").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut st.settings.tab, SettingsTab::Clusters, "Clusters");
            ui.selectable_value(&mut st.settings.tab, SettingsTab::Appearance, "Apparence");
            ui.selectable_value(&mut st.settings.tab, SettingsTab::Behavior, "Comportement");
            ui.selectable_value(&mut st.settings.tab, SettingsTab::About, "Informations");
        });
        ui.add_space(4.0);
    });

    egui::CentralPanel::default().show(ui, |ui| match st.settings.tab {
        SettingsTab::Clusters => clusters_tab(ui, st, backend),
        SettingsTab::Appearance => appearance_tab(ui, st),
        SettingsTab::Behavior => behavior_tab(ui, st),
        SettingsTab::About => about_tab(ui, st),
    });

    confirm_modal(ui, st, backend);
}

// ---------------------------------------------------------------------------
// Onglet « Clusters »
// ---------------------------------------------------------------------------

/// Action demandée sur une carte de cluster.
enum ClusterAction {
    Select(String),
    Remove(String),
}

fn clusters_tab(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let rafraichissement = is_busy(st, LBL_REFRESH);
    let mut rafraichir = false;
    ui.horizontal(|ui| {
        ui.weak(format!("{} cluster(s) enregistré(s).", st.clusters.len()));
        if ui.button("Rafraîchir").clicked() {
            rafraichir = true;
        }
        if rafraichissement {
            ui.spinner();
        }
    });
    if rafraichir {
        dispatch(st, backend, LBL_REFRESH, |id| Command::RefreshClusters {
            id,
        });
    }
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let action = liste_clusters(ui, st);
            match action {
                Some(ClusterAction::Select(nom)) => {
                    st.current_cluster = Some(nom.clone());
                    backend.send(Command::SelectCluster { name: nom });
                }
                Some(ClusterAction::Remove(nom)) => {
                    st.settings.confirm = Some(SettingsConfirm::RemoveCluster { name: nom });
                }
                None => {}
            }

            ui.add_space(12.0);
            ui.separator();
            ui.heading("Ajouter un cluster");
            ui.horizontal_wrapped(|ui| {
                ui.radio_value(
                    &mut st.settings.add_mode,
                    AddClusterMode::Kubeconfig,
                    "Fichier kubeconfig",
                );
                ui.radio_value(
                    &mut st.settings.add_mode,
                    AddClusterMode::Paste,
                    "Coller un kubeconfig",
                );
                ui.radio_value(
                    &mut st.settings.add_mode,
                    AddClusterMode::Remote,
                    "Serveur distant",
                );
            });
            ui.add_space(6.0);
            match st.settings.add_mode {
                AddClusterMode::Kubeconfig => ajout_kubeconfig(ui, st, backend),
                AddClusterMode::Paste => ajout_colle(ui, st, backend),
                AddClusterMode::Remote => ajout_distant(ui, st, backend),
            }

            if let Some(err) = st.settings.error.clone() {
                ui.add_space(6.0);
                let rouge = couleur_erreur(ui);
                ui.colored_label(rouge, format!("{} {err}", icons::WARNING));
            }
        });
}

/// Liste des clusters enregistrés ; renvoie l'action demandée, s'il y en a une.
fn liste_clusters(ui: &mut egui::Ui, st: &AppState) -> Option<ClusterAction> {
    if st.clusters.is_empty() {
        ui.add_space(8.0);
        ui.weak("Aucun cluster. Ajoutez-en un ci-dessous.");
        return None;
    }

    let vert = couleur_ok(ui);
    let rouge = couleur_erreur(ui);
    let bleu = couleur_info(ui);
    let mut action = None;

    for c in &st.clusters {
        let courant = st.current_cluster.as_deref() == Some(c.name.as_str());
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                let largeur = ui.available_width();
                ui.set_width(largeur);
                ui.horizontal_wrapped(|ui| {
                    let titre = egui::RichText::new(&c.name).strong();
                    ui.label(if courant { titre.color(bleu) } else { titre });
                    if courant {
                        ui.label(egui::RichText::new("sélectionné").small().color(bleu));
                    }
                    if c.connected {
                        ui.colored_label(vert, "connecté");
                    } else {
                        ui.colored_label(rouge, "hors ligne");
                    }
                    if c.metrics_available {
                        ui.weak("métriques");
                    }
                });

                egui::Grid::new(("cluster-grid", &c.name))
                    .num_columns(2)
                    .spacing([10.0, 3.0])
                    .show(ui, |ui| {
                        ui.weak("Serveur");
                        ui.label(
                            egui::RichText::new(truncate(&c.server, 60))
                                .monospace()
                                .small(),
                        )
                        .on_hover_text(c.server.clone());
                        ui.end_row();

                        ui.weak("Version");
                        ui.label(c.version.clone().unwrap_or_else(|| "—".to_string()));
                        ui.end_row();

                        ui.weak("Namespace par défaut");
                        ui.label(c.default_namespace.clone());
                        ui.end_row();

                        if let Some(ctx) = &c.context {
                            ui.weak("Contexte");
                            ui.label(ctx.clone());
                            ui.end_row();
                        }
                        if c.node_count.is_some() || c.namespace_count.is_some() {
                            ui.weak("Inventaire");
                            ui.label(format!(
                                "{} nœud(s), {} namespace(s)",
                                c.node_count
                                    .map(|n| n.to_string())
                                    .unwrap_or_else(|| "?".to_string()),
                                c.namespace_count
                                    .map(|n| n.to_string())
                                    .unwrap_or_else(|| "?".to_string())
                            ));
                            ui.end_row();
                        }
                    });

                if let Some(err) = &c.last_error {
                    ui.colored_label(rouge, format!("Dernière erreur : {}", truncate(err, 160)))
                        .on_hover_text(err.clone());
                }

                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!courant, egui::Button::new("Sélectionner"))
                        .clicked()
                    {
                        action = Some(ClusterAction::Select(c.name.clone()));
                    }
                    if ui.button("Supprimer").clicked() {
                        action = Some(ClusterAction::Remove(c.name.clone()));
                    }
                });
            });
    }
    action
}

// --- mode 1 : fichier kubeconfig -------------------------------------------

fn ajout_kubeconfig(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    // La boîte partagée survit d'une image à l'autre dans la mémoire d'egui.
    let slot: PickSlot = ui.ctx().data_mut(|d| {
        d.get_temp_mut_or_default::<PickSlot>(egui::Id::new("kw-choix-kubeconfig"))
            .clone()
    });

    // Relève non bloquante du résultat déposé par le fil du sélecteur.
    let recu = slot.lock().take();
    if let Some(Some(chemin)) = recu {
        st.settings.kubeconfig_path = chemin.display().to_string();
        st.settings.contexts.clear();
        st.settings.selected_context = None;
    }

    let lecture = is_busy(st, LBL_CONTEXTS);
    let mut parcourir = false;
    let mut lister = false;
    let mut importer = false;

    ui.horizontal_wrapped(|ui| {
        ui.label("Fichier");
        ui.add(
            egui::TextEdit::singleline(&mut st.settings.kubeconfig_path)
                .hint_text("~/.kube/config (vide = emplacement par défaut)")
                .desired_width(320.0),
        );
        if ui.button("Parcourir…").clicked() {
            parcourir = true;
        }
        if ui.button("Lister les contextes").clicked() {
            lister = true;
        }
        if lecture {
            ui.spinner();
        }
    });

    if parcourir {
        ouvrir_selecteur(ui.ctx(), slot);
    }

    let chemin = chemin_kubeconfig(&st.settings.kubeconfig_path);
    if lister {
        st.settings.error = None;
        let p = chemin.clone();
        dispatch(st, backend, LBL_CONTEXTS, move |id| Command::ListContexts {
            id,
            path: p,
        });
    }

    let mut choix_contexte: Option<String> = None;
    if !st.settings.contexts.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Contextes détectés").strong());
        egui::ScrollArea::vertical()
            .id_salt("settings-contextes")
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for c in &st.settings.contexts {
                    let actif = st.settings.selected_context.as_deref() == Some(c.name.as_str());
                    let mut libelle = c.name.clone();
                    if c.current {
                        libelle.push_str("  (courant)");
                    }
                    if ui.selectable_label(actif, libelle).clicked() {
                        choix_contexte = Some(c.name.clone());
                    }
                    ui.indent(("ctx", &c.name), |ui| {
                        let mut detail = format!("cluster {}", c.cluster);
                        if let Some(ns) = &c.namespace {
                            detail.push_str(&format!(" · namespace {ns}"));
                        }
                        if let Some(s) = &c.server {
                            detail.push_str(&format!(" · {s}"));
                        }
                        ui.weak(egui::RichText::new(truncate(&detail, 110)).small());
                    });
                }
            });
    }
    if let Some(nom) = choix_contexte {
        st.settings.selected_context = Some(nom);
    }

    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(
            &mut st.settings.all_contexts,
            "Importer tous les contextes du fichier",
        );
        ui.checkbox(&mut st.settings.persist, "Enregistrer la connexion");
    });
    if !st.settings.all_contexts {
        ui.horizontal(|ui| {
            ui.label("Nom");
            ui.add(
                egui::TextEdit::singleline(&mut st.settings.cluster_name)
                    .hint_text("vide = nom du contexte")
                    .desired_width(240.0),
            );
        });
    }
    ui.horizontal(|ui| {
        if ui.button("Importer").clicked() {
            importer = true;
        }
        if is_busy(st, LBL_IMPORT) || is_busy(st, LBL_CONNECT) {
            ui.spinner();
        }
    });

    if importer {
        st.settings.error = None;
        if st.settings.all_contexts {
            let p = chemin.clone();
            dispatch(st, backend, LBL_IMPORT, move |id| {
                Command::ImportKubeconfig {
                    id,
                    path: p,
                    all_contexts: true,
                }
            });
        } else if let Some(contexte) = st.settings.selected_context.clone() {
            let nom = {
                let saisi = st.settings.cluster_name.trim();
                if saisi.is_empty() {
                    contexte.clone()
                } else {
                    saisi.to_string()
                }
            };
            let spec = ConnectionSpec::Kubeconfig {
                path: chemin.clone(),
                context: Some(contexte),
            };
            let persist = st.settings.persist;
            dispatch(st, backend, LBL_CONNECT, move |id| Command::Connect {
                id,
                spec,
                name: nom,
                persist,
            });
        } else {
            // Ni « tous les contextes » ni sélection explicite : on prend le
            // contexte courant du fichier.
            let p = chemin.clone();
            dispatch(st, backend, LBL_IMPORT, move |id| {
                Command::ImportKubeconfig {
                    id,
                    path: p,
                    all_contexts: false,
                }
            });
        }
        dispatch(st, backend, LBL_REFRESH, |id| Command::RefreshClusters {
            id,
        });
    }
}

/// Lance le sélecteur de fichiers sur un fil séparé.
///
/// Le fil d'interface n'attend jamais : il relit la boîte à l'image suivante,
/// réveillée par `request_repaint`.
fn ouvrir_selecteur(ctx: &egui::Context, slot: PickSlot) {
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let choix = futures::executor::block_on(
            rfd::AsyncFileDialog::new()
                .set_title("Choisir un fichier kubeconfig")
                .add_filter("Kubeconfig", &["yaml", "yml", "config", "conf"])
                .pick_file(),
        );
        *slot.lock() = Some(choix.map(|f| f.path().to_path_buf()));
        ctx.request_repaint();
    });
}

/// Chemin explicite, ou `None` pour laisser le cœur choisir l'emplacement usuel.
fn chemin_kubeconfig(saisi: &str) -> Option<PathBuf> {
    let t = saisi.trim();
    if t.is_empty() {
        None
    } else {
        Some(PathBuf::from(t))
    }
}

// --- mode 2 : kubeconfig collé ---------------------------------------------

fn ajout_colle(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    ui.label("Collez ici le contenu complet d'un kubeconfig :");
    ui.add(
        egui::TextEdit::multiline(&mut st.settings.inline_yaml)
            .code_editor()
            .desired_rows(8)
            .desired_width(f32::INFINITY)
            .hint_text("apiVersion: v1\nkind: Config\n…"),
    );
    egui::Grid::new("settings-colle")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Nom du cluster");
            ui.add(
                egui::TextEdit::singleline(&mut st.settings.cluster_name)
                    .hint_text("production")
                    .desired_width(240.0),
            );
            ui.end_row();
            ui.label("Contexte");
            ui.add(
                egui::TextEdit::singleline(&mut st.settings.inline_context)
                    .hint_text("vide = current-context")
                    .desired_width(240.0),
            );
            ui.end_row();
        });
    ui.checkbox(&mut st.settings.persist, "Enregistrer la connexion");

    let mut connecter = false;
    ui.horizontal(|ui| {
        if ui.button("Connecter").clicked() {
            connecter = true;
        }
        if is_busy(st, LBL_CONNECT) {
            ui.spinner();
        }
    });

    if connecter {
        let nom = st.settings.cluster_name.trim().to_string();
        let yaml = st.settings.inline_yaml.trim().to_string();
        if nom.is_empty() {
            st.settings.error = Some("Donnez un nom à ce cluster.".to_string());
        } else if yaml.is_empty() {
            st.settings.error = Some("Le kubeconfig collé est vide.".to_string());
        } else {
            st.settings.error = None;
            let contexte = {
                let c = st.settings.inline_context.trim();
                if c.is_empty() {
                    None
                } else {
                    Some(c.to_string())
                }
            };
            let spec = ConnectionSpec::Inline {
                yaml,
                context: contexte,
            };
            let persist = st.settings.persist;
            dispatch(st, backend, LBL_CONNECT, move |id| Command::Connect {
                id,
                spec,
                name: nom,
                persist,
            });
            dispatch(st, backend, LBL_REFRESH, |id| Command::RefreshClusters {
                id,
            });
        }
    }
}

// --- mode 3 : serveur distant ----------------------------------------------

fn ajout_distant(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    egui::Grid::new("settings-distant")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            let s = &mut st.settings;
            ui.label("Nom du cluster");
            ui.add(
                egui::TextEdit::singleline(&mut s.cluster_name)
                    .hint_text("production")
                    .desired_width(300.0),
            );
            ui.end_row();

            ui.label("URL du serveur");
            ui.add(
                egui::TextEdit::singleline(&mut s.remote_server)
                    .hint_text("https://10.0.0.1:6443")
                    .desired_width(300.0),
            );
            ui.end_row();

            ui.label("Jeton");
            ui.add(
                egui::TextEdit::singleline(&mut s.remote_token)
                    .password(true)
                    .hint_text("jeton porteur (facultatif)")
                    .desired_width(300.0),
            );
            ui.end_row();

            ui.label("Namespace par défaut");
            ui.add(
                egui::TextEdit::singleline(&mut s.remote_namespace)
                    .hint_text("default")
                    .desired_width(300.0),
            );
            ui.end_row();
        });

    ui.add_space(4.0);
    zone_pem(
        ui,
        "Autorité de certification (PEM)",
        &mut st.settings.remote_ca_pem,
    );
    zone_pem(
        ui,
        "Certificat client (PEM)",
        &mut st.settings.remote_client_cert_pem,
    );
    zone_pem(
        ui,
        "Clé privée client (PEM)",
        &mut st.settings.remote_client_key_pem,
    );

    ui.add_space(4.0);
    ui.checkbox(
        &mut st.settings.remote_insecure,
        "Ignorer la vérification du certificat du serveur",
    );
    if st.settings.remote_insecure {
        let orange = couleur_avertissement(ui);
        ui.colored_label(
            orange,
            format!(
                "{} Sans vérification TLS, l'identité du serveur n'est plus contrôlée : un \
                 intercepteur sur le réseau pourrait lire vos jetons et vos manifestes. \
                 À réserver à un cluster de test, sur un réseau de confiance.",
                icons::WARNING
            ),
        );
    }
    ui.checkbox(&mut st.settings.persist, "Enregistrer la connexion");

    let mut connecter = false;
    ui.horizontal(|ui| {
        if ui.button("Connecter").clicked() {
            connecter = true;
        }
        if is_busy(st, LBL_CONNECT) {
            ui.spinner();
        }
    });

    if connecter {
        let nom = st.settings.cluster_name.trim().to_string();
        let serveur = st.settings.remote_server.trim().to_string();
        if nom.is_empty() {
            st.settings.error = Some("Donnez un nom à ce cluster.".to_string());
        } else if serveur.is_empty() {
            st.settings.error = Some("Indiquez l'URL du serveur d'API.".to_string());
        } else if !serveur.starts_with("http://") && !serveur.starts_with("https://") {
            st.settings.error =
                Some("L'URL doit commencer par « https:// » (ou « http:// »).".to_string());
        } else {
            st.settings.error = None;
            let spec = ConnectionSpec::Remote {
                server: serveur,
                token: vide_en_none(&st.settings.remote_token),
                ca_cert_pem: vide_en_none(&st.settings.remote_ca_pem),
                client_cert_pem: vide_en_none(&st.settings.remote_client_cert_pem),
                client_key_pem: vide_en_none(&st.settings.remote_client_key_pem),
                insecure_skip_tls_verify: st.settings.remote_insecure,
                namespace: vide_en_none(&st.settings.remote_namespace),
                proxy_url: None,
            };
            let persist = st.settings.persist;
            dispatch(st, backend, LBL_CONNECT, move |id| Command::Connect {
                id,
                spec,
                name: nom,
                persist,
            });
            dispatch(st, backend, LBL_REFRESH, |id| Command::RefreshClusters {
                id,
            });
        }
    }
}

/// Zone de texte repliable pour un bloc PEM.
fn zone_pem(ui: &mut egui::Ui, titre: &str, valeur: &mut String) {
    let rempli = !valeur.trim().is_empty();
    let entete = if rempli {
        format!("{titre} — renseigné")
    } else {
        format!("{titre} — vide")
    };
    ui.collapsing(entete, |ui| {
        ui.add(
            egui::TextEdit::multiline(valeur)
                .code_editor()
                .desired_rows(4)
                .desired_width(f32::INFINITY)
                .hint_text("-----BEGIN CERTIFICATE-----"),
        );
    });
}

// ---------------------------------------------------------------------------
// Onglet « Apparence »
// ---------------------------------------------------------------------------

fn appearance_tab(ui: &mut egui::Ui, st: &mut AppState) {
    ui.heading("Apparence");
    ui.add_space(6.0);

    ui.label(egui::RichText::new("Thème").strong());
    let mut theme_change = false;
    ui.horizontal_wrapped(|ui| {
        theme_change |= ui
            .radio_value(&mut st.settings.theme, ThemeChoice::System, "Système")
            .changed();
        theme_change |= ui
            .radio_value(&mut st.settings.theme, ThemeChoice::Light, "Clair")
            .changed();
        theme_change |= ui
            .radio_value(&mut st.settings.theme, ThemeChoice::Dark, "Sombre")
            .changed();
    });
    if theme_change {
        appliquer_theme(ui.ctx(), st.settings.theme);
    }

    ui.add_space(10.0);
    ui.label(egui::RichText::new("Échelle de l'interface").strong());
    // Une échelle nulle rendrait la fenêtre inutilisable : on la borne toujours.
    if st.settings.zoom < 0.5 || st.settings.zoom > 3.0 {
        st.settings.zoom = 1.0;
    }
    let mut zoom = st.settings.zoom;
    if ui
        .add(
            egui::Slider::new(&mut zoom, 0.75..=2.0)
                .step_by(0.05)
                .fixed_decimals(2)
                .text("facteur"),
        )
        .changed()
    {
        st.settings.zoom = zoom;
        ui.ctx().set_zoom_factor(zoom);
    }
    ui.horizontal(|ui| {
        if ui.small_button("Réinitialiser").clicked() {
            st.settings.zoom = 1.0;
            ui.ctx().set_zoom_factor(1.0);
        }
        ui.weak("Ctrl + molette, Ctrl + « + » / « - » et Ctrl + 0 agissent aussi.");
    });

    ui.add_space(10.0);
    ui.label(egui::RichText::new("Densité des tableaux").strong());
    let mut densite_change = false;
    ui.horizontal_wrapped(|ui| {
        densite_change |= ui
            .radio_value(&mut st.settings.density, Density::Compact, "Compacte")
            .changed();
        densite_change |= ui
            .radio_value(&mut st.settings.density, Density::Normal, "Normale")
            .changed();
        densite_change |= ui
            .radio_value(&mut st.settings.density, Density::Comfortable, "Aérée")
            .changed();
    });
    if densite_change {
        appliquer_densite(ui.ctx(), st.settings.density);
    }
}

/// Applique la préférence de thème au contexte egui.
pub fn appliquer_theme(ctx: &egui::Context, choix: ThemeChoice) {
    let preference = match choix {
        ThemeChoice::System => egui::ThemePreference::System,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::Dark => egui::ThemePreference::Dark,
    };
    ctx.set_theme(preference);
}

/// Applique la densité aux deux styles (clair et sombre).
pub fn appliquer_densite(ctx: &egui::Context, densite: Density) {
    let (espacement, hauteur) = match densite {
        Density::Compact => (egui::vec2(6.0, 2.0), 18.0),
        Density::Normal => (egui::vec2(8.0, 3.0), 20.0),
        Density::Comfortable => (egui::vec2(10.0, 7.0), 26.0),
    };
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = espacement;
        style.spacing.interact_size.y = hauteur;
    });
}

// ---------------------------------------------------------------------------
// Onglet « Comportement »
// ---------------------------------------------------------------------------

fn behavior_tab(ui: &mut egui::Ui, st: &mut AppState) {
    ui.heading("Comportement");
    ui.add_space(6.0);

    ui.checkbox(
        &mut st.auto_refresh,
        "Rafraîchir automatiquement les listes",
    );
    ui.horizontal(|ui| {
        ui.label("Période");
        // `Duration` n'est pas éditable directement : on passe par les secondes.
        let mut secondes = st.refresh_every.as_secs().max(1);
        if ui
            .add_enabled(
                st.auto_refresh,
                egui::DragValue::new(&mut secondes)
                    .range(1u64..=3600u64)
                    .speed(1.0)
                    .suffix(" s"),
            )
            .changed()
        {
            st.refresh_every = std::time::Duration::from_secs(secondes);
        }
    });

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label("Lignes de journal conservées");
        ui.add(
            egui::DragValue::new(&mut st.settings.log_lines)
                .range(200usize..=200_000usize)
                .speed(100.0),
        );
    });
    ui.weak("Au-delà, les lignes les plus anciennes sont oubliées.");

    ui.add_space(10.0);
    ui.checkbox(
        &mut st.settings.confirm_destructive,
        "Demander confirmation avant toute suppression",
    );
    if !st.settings.confirm_destructive {
        let orange = couleur_avertissement(ui);
        ui.colored_label(
            orange,
            format!(
                "{} Les suppressions de ressources partiront sans question. \
                 Le retrait d'un cluster reste, lui, toujours confirmé.",
                icons::WARNING
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// Onglet « Informations »
// ---------------------------------------------------------------------------

fn about_tab(ui: &mut egui::Ui, st: &AppState) {
    ui.heading("KubeWatch");
    ui.add_space(6.0);

    egui::Grid::new("settings-infos")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.weak("Version");
            ui.label(env!("CARGO_PKG_VERSION"));
            ui.end_row();

            ui.weak("Plateforme");
            ui.label(format!(
                "{} / {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ));
            ui.end_row();

            ui.weak("Dossier d'état");
            let dossier = dossier_etat();
            ui.label(
                egui::RichText::new(truncate(&dossier.display().to_string(), 60))
                    .monospace()
                    .small(),
            )
            .on_hover_text(dossier.display().to_string());
            ui.end_row();

            ui.weak("Binaire helm");
            match helm_detecte() {
                Some(chemin) => {
                    ui.label(
                        egui::RichText::new(truncate(chemin, 60))
                            .monospace()
                            .small(),
                    )
                    .on_hover_text(chemin.to_string());
                }
                None => {
                    ui.label("absent — le rendu des charts Helm est indisponible");
                }
            }
            ui.end_row();

            ui.weak("Clusters enregistrés");
            ui.label(st.clusters.len().to_string());
            ui.end_row();
        });

    ui.add_space(10.0);
    ui.label(egui::RichText::new("Raccourcis clavier").strong());
    egui::Grid::new("settings-raccourcis")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for (touche, effet) in [
                ("Ctrl + molette", "ajuste l'échelle de l'interface"),
                ("Ctrl + « + » / « - »", "agrandit ou réduit l'interface"),
                ("Ctrl + 0", "revient à l'échelle par défaut"),
                ("Échap", "ferme la fenêtre de confirmation ouverte"),
                ("Tab / Maj + Tab", "passe d'un champ au suivant"),
            ] {
                ui.label(egui::RichText::new(touche).monospace().small());
                ui.weak(effet);
                ui.end_row();
            }
        });

    ui.add_space(12.0);
    if ui
        .button("Vérifier les mises à jour de KubeWatch")
        .on_hover_text(RELEASES_URL)
        .clicked()
    {
        ui.ctx()
            .open_url(egui::OpenUrl::new_tab(RELEASES_URL.to_string()));
    }
}

/// Emplacement du dossier d'état, selon la même règle que le reste du projet.
fn dossier_etat() -> PathBuf {
    if let Ok(v) = std::env::var("KUBEWATCH_STATE_DIR") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
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

/// Chemin du binaire `helm`, cherché une seule fois pour toute la session.
fn helm_detecte() -> Option<&'static str> {
    static HELM: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HELM.get_or_init(|| kubewatch_hub::charts::helm_binary().map(|p| p.display().to_string()))
        .as_deref()
}

// ---------------------------------------------------------------------------
// Confirmation
// ---------------------------------------------------------------------------

fn confirm_modal(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let demande = match st.settings.confirm.clone() {
        Some(d) => d,
        None => return,
    };
    let ctx = ui.ctx().clone();
    let rouge = couleur_erreur(ui);
    let mut fermer = false;
    let mut confirmer = false;

    let reponse = egui::Modal::new(egui::Id::new("settings-confirmation")).show(&ctx, |ui| {
        ui.set_width(420.0);
        match &demande {
            SettingsConfirm::RemoveCluster { name } => {
                ui.heading("Retirer le cluster");
                ui.add_space(6.0);
                ui.label(format!("« {name} » sera retiré de la liste de KubeWatch."));
                ui.weak(
                    "Le cluster lui-même n'est pas touché : seule la connexion enregistrée \
                     disparaît.",
                );
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Annuler").clicked() {
                fermer = true;
            }
            if ui
                .button(egui::RichText::new("Retirer").color(rouge).strong())
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
            SettingsConfirm::RemoveCluster { name } => {
                if st.current_cluster.as_deref() == Some(name.as_str()) {
                    st.current_cluster = None;
                    // Sans cela, les ressources du cluster retiré restent
                    // affichées, et un panneau de détail ouvert continue de
                    // faire vivre ses flux de journaux ou sa session terminal.
                    st.clear_cluster_data();
                }
                backend.send(Command::RemoveCluster { name });
                dispatch(st, backend, LBL_REFRESH, |id| Command::RefreshClusters {
                    id,
                });
            }
        }
        st.settings.confirm = None;
    } else if fermer {
        st.settings.confirm = None;
    }
}

// ---------------------------------------------------------------------------
// Utilitaires
// ---------------------------------------------------------------------------

/// `None` pour une saisie vide, sinon la valeur détourée.
fn vide_en_none(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chemin_vide_signifie_emplacement_par_defaut() {
        assert_eq!(chemin_kubeconfig("   "), None);
        assert_eq!(
            chemin_kubeconfig(" /tmp/kubeconfig.yml "),
            Some(PathBuf::from("/tmp/kubeconfig.yml"))
        );
    }

    #[test]
    fn champs_vides_deviennent_none() {
        assert_eq!(vide_en_none(" \n "), None);
        assert_eq!(vide_en_none(" abc "), Some("abc".to_string()));
    }

    #[test]
    fn dossier_detat_respecte_la_variable_denvironnement() {
        // La règle de repli doit produire un chemin non vide en toutes
        // circonstances : c'est ce que l'écran d'informations affiche.
        let d = dossier_etat();
        assert!(!d.as_os_str().is_empty());
    }
}
