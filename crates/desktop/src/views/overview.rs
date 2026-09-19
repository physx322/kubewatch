//! Écran « Vue d'ensemble » : l'état de santé du cluster en un coup d'œil.
//!
//! Cette vue ne fait **aucun** appel réseau : elle lit `AppState` et envoie des
//! `Command` au worker. La seule opération susceptible de bloquer — la boîte de
//! dialogue de sélection de fichier du système — est déportée dans un fil
//! dédié : le fil d'interface reste fluide même si l'utilisateur laisse la
//! boîte ouverte plusieurs minutes.
//!
//! # Hypothèses sur l'état partagé
//! En plus des champs listés dans le contrat, cette vue utilise
//! `st.overview: Option<ClusterOverview>` (la dernière synthèse reçue) et
//! `st.events: Vec<EventSummary>` (les derniers évènements du cluster).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};
use kubewatch_core::model::{ClusterOverview, EventSummary};

use super::resources::format_age;
use crate::backend::{Backend, Command};
use crate::icons;
use crate::state::{AppState, View};

/// Largeur d'une tuile de la grille, en points.
const TILE_WIDTH: f32 = 208.0;
/// Nombre maximal d'évènements affichés.
const MAX_EVENTS: usize = 200;
/// Vert « en bon état », lisible sur thème clair comme sombre.
const OK_COLOR: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);

// ---------------------------------------------------------------------------
// Sélecteur de fichier non bloquant
// ---------------------------------------------------------------------------

/// Chemin choisi par l'utilisateur, déposé par le fil de la boîte de dialogue
/// et relevé par le fil d'interface à l'image suivante.
static PICKED_KUBECONFIG: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Vrai tant qu'une boîte de dialogue est ouverte : évite d'en empiler deux.
static PICKER_OPEN: AtomicBool = AtomicBool::new(false);

/// Répertoire proposé à l'ouverture de la boîte de dialogue.
fn default_kube_dir() -> PathBuf {
    dirs::home_dir()
        .map(|home| home.join(".kube"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Ouvre le sélecteur de fichier dans un fil dédié.
///
/// La boîte de dialogue du système est synchrone : l'exécuter sur le fil
/// d'interface gèlerait la fenêtre. Le fil dédié dépose le résultat dans
/// [`PICKED_KUBECONFIG`] puis réveille l'interface.
fn open_kubeconfig_picker(ctx: egui::Context) {
    if PICKER_OPEN.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("kubewatch-file-dialog".to_string())
        .spawn(move || {
            let chosen = rfd::FileDialog::new()
                .set_title("Choisir un fichier kubeconfig")
                .add_filter("Kubeconfig", &["yaml", "yml", "conf", "config"])
                .add_filter("Tous les fichiers", &["*"])
                .set_directory(default_kube_dir())
                .pick_file();
            if let Some(path) = chosen {
                // Un verrou empoisonné reste exploitable : la donnée protégée
                // n'est qu'un chemin, aucun invariant n'a pu être rompu.
                let mut slot = match PICKED_KUBECONFIG.lock() {
                    Ok(slot) => slot,
                    Err(poisoned) => poisoned.into_inner(),
                };
                *slot = Some(path);
            }
            PICKER_OPEN.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    if spawned.is_err() {
        // Le fil n'a pas démarré : on relâche le verrou pour pouvoir réessayer.
        PICKER_OPEN.store(false, Ordering::SeqCst);
    }
}

/// Relève le chemin déposé par le fil de la boîte de dialogue, s'il y en a un.
fn take_picked_kubeconfig() -> Option<PathBuf> {
    let mut slot = match PICKED_KUBECONFIG.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    };
    slot.take()
}

// ---------------------------------------------------------------------------
// Mise en forme
// ---------------------------------------------------------------------------

/// Met en forme une quantité de CPU exprimée en millicores.
fn format_cpu(millis: f64) -> String {
    if millis >= 1000.0 {
        format!("{:.2} cœurs", millis / 1000.0)
    } else {
        format!("{millis:.0} m")
    }
}

/// Met en forme une quantité d'octets en unités binaires.
fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 6] = ["o", "Kio", "Mio", "Gio", "Tio", "Pio"];
    let mut value = bytes.max(0) as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let suffix = UNITS[unit];
    if unit == 0 {
        format!("{value:.0} {suffix}")
    } else {
        format!("{value:.1} {suffix}")
    }
}

/// Fraction bornée à `[0, 1]`, sûre même quand la capacité est nulle.
fn fraction(used: f64, capacity: f64) -> f32 {
    if capacity <= 0.0 {
        return 0.0;
    }
    (used / capacity).clamp(0.0, 1.0) as f32
}

/// Chiffre mis en avant dans une tuile.
fn big(text: String) -> RichText {
    RichText::new(text).size(22.0).strong()
}

/// Âge d'un évènement, en secondes.
fn event_age(event: &EventSummary) -> Option<i64> {
    let stamp = event.last_seen.or(event.first_seen)?;
    Some((chrono::Utc::now() - stamp).num_seconds().max(0))
}

// ---------------------------------------------------------------------------
// Actions différées
// ---------------------------------------------------------------------------

/// Intention exprimée pendant le rendu, appliquée une fois l'écran dessiné.
enum Action {
    /// Recharger la synthèse et les évènements.
    Refresh,
    /// Basculer vers l'écran des réglages.
    OpenSettings,
    /// Ouvrir la boîte de dialogue de sélection de fichier.
    PickKubeconfig,
    /// Importer le kubeconfig par défaut du système.
    ImportDefault,
}

// ---------------------------------------------------------------------------
// Point d'entrée de la vue
// ---------------------------------------------------------------------------

/// Dessine l'écran « Vue d'ensemble ».
pub fn show(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let ctx = ui.ctx().clone();

    // Le chemin choisi dans la boîte de dialogue arrive de façon asynchrone.
    if let Some(path) = take_picked_kubeconfig() {
        let id = st.next_id();
        st.pending
            .insert(id, format!("Import de {}", path.display()));
        backend.send(Command::ImportKubeconfig {
            id,
            path: Some(path),
            all_contexts: true,
        });
    }

    let mut actions: Vec<Action> = Vec::new();

    if st.clusters.is_empty() {
        welcome(ui, &mut actions);
    } else {
        header(ui, st, &mut actions);
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("kw_overview_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                body(ui, st, &mut actions);
            });
    }

    for action in actions {
        apply(&ctx, st, backend, action);
    }
}

// ---------------------------------------------------------------------------
// Écran d'accueil (aucun cluster enregistré)
// ---------------------------------------------------------------------------

/// Carte proposant une manière d'ajouter un cluster. Renvoie vrai si le bouton
/// a été actionné.
fn option_card(ui: &mut egui::Ui, title: &str, description: &str, button: &str) -> bool {
    let mut clicked = false;
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical(|ui| {
                ui.label(RichText::new(title).strong());
                ui.label(RichText::new(description).small().weak());
                ui.add_space(6.0);
                clicked = ui.button(button).clicked();
            });
        });
    ui.add_space(8.0);
    clicked
}

/// Accueil affiché tant qu'aucun cluster n'est enregistré.
fn welcome(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.add_space(32.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("KubeWatch").size(28.0).strong());
        ui.add_space(4.0);
        ui.label(RichText::new("Aucun cluster n'est encore enregistré.").weak());
        ui.label(RichText::new("Trois façons de commencer :").small().weak());
    });
    ui.add_space(18.0);

    ui.vertical_centered(|ui| {
        ui.allocate_ui_with_layout(
            egui::Vec2::new(460.0, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(460.0);

                if option_card(
                    ui,
                    "Importer le kubeconfig du système",
                    "Lit ~/.kube/config (ou $KUBECONFIG) et enregistre tous les contextes qui s'y trouvent.",
                    "Importer",
                ) {
                    actions.push(Action::ImportDefault);
                }

                if option_card(
                    ui,
                    "Choisir un fichier kubeconfig…",
                    "Sélectionner un fichier ailleurs sur le disque. L'interface reste utilisable pendant que la boîte de dialogue est ouverte.",
                    "Parcourir…",
                ) {
                    actions.push(Action::PickKubeconfig);
                }

                if option_card(
                    ui,
                    "Coller un kubeconfig ou ajouter un cluster distant",
                    "Coller le contenu d'un kubeconfig, ou saisir directement une URL de serveur d'API et un jeton.",
                    "Ouvrir les réglages",
                ) {
                    actions.push(Action::OpenSettings);
                }
            },
        );
    });
}

// ---------------------------------------------------------------------------
// Bandeau supérieur
// ---------------------------------------------------------------------------

/// Nom du cluster, état de la connexion, bouton de rafraîchissement et
/// horodatage de la dernière mise à jour.
fn header(ui: &mut egui::Ui, st: &AppState, actions: &mut Vec<Action>) {
    ui.horizontal_wrapped(|ui| {
        let name = st
            .current_cluster
            .clone()
            .unwrap_or_else(|| "Aucun cluster sélectionné".to_string());
        ui.label(RichText::new(name).size(20.0).strong());

        let current = st
            .clusters
            .iter()
            .find(|c| Some(c.name.as_str()) == st.current_cluster.as_deref());
        if let Some(info) = current {
            let error_color = ui.visuals().error_fg_color;
            let (state, color) = if info.connected {
                ("● connecté", OK_COLOR)
            } else {
                ("● injoignable", error_color)
            };
            ui.label(RichText::new(state).color(color).small());
            if !info.server.is_empty() {
                ui.label(RichText::new(info.server.as_str()).small().weak());
            }
            if let Some(version) = &info.version {
                ui.label(RichText::new(version.as_str()).small().weak());
            }
            if let Some(error) = &info.last_error {
                ui.label(RichText::new(error.as_str()).small().color(error_color))
                    .on_hover_text(error.as_str());
            }
        }

        if ui.button("Rafraîchir").clicked() {
            actions.push(Action::Refresh);
        }
        let elapsed = st.last_refresh.elapsed().as_secs() as i64;
        ui.label(
            RichText::new(format!("mis à jour il y a {}", format_age(Some(elapsed))))
                .small()
                .weak(),
        );
    });
}

// ---------------------------------------------------------------------------
// Corps de la vue
// ---------------------------------------------------------------------------

/// Tuiles, avertissements et évènements récents.
fn body(ui: &mut egui::Ui, st: &AppState, actions: &mut Vec<Action>) {
    let Some(overview) = st.overview.as_ref() else {
        ui.add_space(28.0);
        ui.vertical_centered(|ui| {
            if st.pending.is_empty() {
                ui.label(RichText::new("Aucune synthèse chargée pour ce cluster.").weak());
                ui.add_space(8.0);
                if ui.button("Charger la synthèse").clicked() {
                    actions.push(Action::Refresh);
                }
            } else {
                ui.label(
                    RichText::new(format!("{} Collecte de la synthèse…", icons::REFRESH)).weak(),
                );
            }
        });
        return;
    };

    if !overview.metrics_available {
        metrics_banner(ui);
    }
    tiles(ui, overview);
    warnings(ui, overview);
    ui.add_space(12.0);
    events(ui, st);
}

/// Bandeau expliquant l'absence de `metrics-server`.
fn metrics_banner(ui: &mut egui::Ui) {
    let color = ui.visuals().warn_fg_color;
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(icons::WARNING).color(color).size(16.0));
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("L'API metrics.k8s.io ne répond pas sur ce cluster.")
                            .color(color)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(
                            "metrics-server n'est probablement pas installé : les indicateurs \
                             CPU et mémoire resteront vides. Tout le reste fonctionne normalement.",
                        )
                        .small()
                        .weak(),
                    );
                });
            });
        });
    ui.add_space(10.0);
}

/// Tuile de la grille : un intitulé discret et un contenu dense.
fn tile(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(TILE_WIDTH);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).small().weak());
                ui.add_space(2.0);
                contents(ui);
            });
        });
}

/// Grille de tuiles : nœuds, pods, CPU, mémoire, namespaces, charges.
fn tiles(ui: &mut egui::Ui, overview: &ClusterOverview) {
    let bar_width = TILE_WIDTH - 4.0;
    let warn_color = ui.visuals().warn_fg_color;
    let error_color = ui.visuals().error_fg_color;

    ui.horizontal_wrapped(|ui| {
        // --- Nœuds ---------------------------------------------------------
        tile(ui, "Nœuds prêts", |ui| {
            ui.label(big(format!(
                "{} / {}",
                overview.nodes_ready, overview.nodes_total
            )));
            ui.add(
                egui::ProgressBar::new(fraction(
                    overview.nodes_ready as f64,
                    overview.nodes_total as f64,
                ))
                .desired_width(bar_width)
                .desired_height(8.0),
            );
            let absent = overview.nodes_total.saturating_sub(overview.nodes_ready);
            if absent > 0 {
                ui.label(
                    RichText::new(format!("{absent} nœud(s) non prêt(s)"))
                        .small()
                        .color(warn_color),
                );
            } else {
                ui.label(RichText::new("tous les nœuds répondent").small().weak());
            }
        });

        // --- Pods ----------------------------------------------------------
        tile(ui, "Pods", |ui| {
            ui.label(big(overview.pods_total.to_string()));
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{} actifs", overview.pods_running))
                        .small()
                        .color(OK_COLOR),
                );
                ui.label(
                    RichText::new(format!("{} en attente", overview.pods_pending))
                        .small()
                        .color(warn_color),
                );
                ui.label(
                    RichText::new(format!("{} en échec", overview.pods_failed))
                        .small()
                        .color(error_color),
                );
            });
        });

        // --- CPU -----------------------------------------------------------
        tile(ui, "CPU", |ui| {
            if overview.metrics_available {
                ui.label(big(format_cpu(overview.cpu_used_millis)));
                ui.add(
                    egui::ProgressBar::new(fraction(
                        overview.cpu_used_millis,
                        overview.cpu_capacity_millis,
                    ))
                    .desired_width(bar_width)
                    .desired_height(8.0),
                );
            } else {
                ui.label(big("—".to_string()));
                ui.add_space(10.0);
            }
            ui.label(
                RichText::new(format!(
                    "capacité {}",
                    format_cpu(overview.cpu_capacity_millis)
                ))
                .small()
                .weak(),
            );
        });

        // --- Mémoire -------------------------------------------------------
        tile(ui, "Mémoire", |ui| {
            if overview.metrics_available {
                ui.label(big(format_bytes(overview.memory_used_bytes)));
                ui.add(
                    egui::ProgressBar::new(fraction(
                        overview.memory_used_bytes as f64,
                        overview.memory_capacity_bytes as f64,
                    ))
                    .desired_width(bar_width)
                    .desired_height(8.0),
                );
            } else {
                ui.label(big("—".to_string()));
                ui.add_space(10.0);
            }
            ui.label(
                RichText::new(format!(
                    "capacité {}",
                    format_bytes(overview.memory_capacity_bytes)
                ))
                .small()
                .weak(),
            );
        });

        // --- Namespaces ----------------------------------------------------
        tile(ui, "Namespaces", |ui| {
            ui.label(big(overview.namespaces.to_string()));
            ui.label(RichText::new("cloisonnements déclarés").small().weak());
        });

        // --- Charges de travail --------------------------------------------
        tile(ui, "Charges de travail", |ui| {
            if overview.workloads.is_empty() {
                ui.label(big("0".to_string()));
                ui.label(RichText::new("aucune charge déclarée").small().weak());
            } else {
                let total: usize = overview.workloads.values().sum();
                ui.label(big(total.to_string()));
                for (kind, count) in &overview.workloads {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(kind.as_str()).small().weak());
                        ui.label(RichText::new(count.to_string()).small().strong());
                    });
                }
            }
        });
    });
}

/// Avertissements collectés pendant la synthèse.
fn warnings(ui: &mut egui::Ui, overview: &ClusterOverview) {
    if overview.warnings.is_empty() {
        return;
    }
    let color = ui.visuals().warn_fg_color;
    ui.add_space(12.0);
    ui.label(RichText::new("Avertissements").strong());
    ui.add_space(2.0);
    for warning in &overview.warnings {
        ui.label(RichText::new(format!("• {warning}")).color(color));
    }
}

/// Table compacte des évènements récents.
fn events(ui: &mut egui::Ui, st: &AppState) {
    ui.label(RichText::new("Évènements récents").strong());
    ui.add_space(2.0);

    if st.events.is_empty() {
        ui.label(
            RichText::new("Aucun évènement récent sur ce cluster.")
                .small()
                .weak(),
        );
        return;
    }

    let warn_color = ui.visuals().warn_fg_color;
    let rows: Vec<&EventSummary> = st.events.iter().take(MAX_EVENTS).collect();
    let titles = ["Âge", "Type", "Raison", "Objet", "Message"];

    TableBuilder::new(ui)
        .id_salt("kw_overview_events")
        .striped(true)
        .vscroll(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(70.0).at_least(50.0).clip(true))
        .column(Column::initial(80.0).at_least(60.0).clip(true))
        .column(Column::initial(170.0).at_least(90.0).clip(true))
        .column(Column::initial(230.0).at_least(120.0).clip(true))
        .column(Column::remainder().at_least(200.0).clip(true))
        .header(20.0, |mut header| {
            for title in titles {
                header.col(|ui| {
                    ui.label(RichText::new(title).small().strong());
                });
            }
        })
        .body(|body| {
            body.rows(18.0, rows.len(), |mut row| {
                let Some(event) = rows.get(row.index()) else {
                    return;
                };
                let warning = event.type_.eq_ignore_ascii_case("Warning");

                row.col(|ui| {
                    ui.label(RichText::new(format_age(event_age(event))).small().weak());
                });
                row.col(|ui| {
                    let text = RichText::new(event.type_.as_str()).small();
                    ui.label(if warning {
                        text.color(warn_color)
                    } else {
                        text.weak()
                    });
                });
                row.col(|ui| {
                    let reason = if event.count > 1 {
                        format!("{} ×{}", event.reason, event.count)
                    } else {
                        event.reason.clone()
                    };
                    let text = RichText::new(reason).small();
                    ui.add(
                        egui::Label::new(if warning {
                            text.color(warn_color)
                        } else {
                            text
                        })
                        .truncate()
                        .selectable(false),
                    );
                });
                row.col(|ui| {
                    let object = match (&event.involved_kind, &event.involved_name) {
                        (Some(kind), Some(name)) => format!("{kind}/{name}"),
                        (None, Some(name)) => name.clone(),
                        _ => event.name.clone(),
                    };
                    let label = match &event.namespace {
                        Some(ns) if !ns.is_empty() => format!("{ns}/{object}"),
                        _ => object,
                    };
                    ui.add(
                        egui::Label::new(RichText::new(label).small())
                            .truncate()
                            .selectable(false),
                    );
                });
                row.col(|ui| {
                    // Le message complet reste accessible en infobulle.
                    ui.add(
                        egui::Label::new(RichText::new(event.message.as_str()).small().weak())
                            .truncate()
                            .selectable(false),
                    )
                    .on_hover_text(event.message.as_str());
                });
            });
        });
}

// ---------------------------------------------------------------------------
// Application des actions
// ---------------------------------------------------------------------------

/// Applique une intention collectée pendant le rendu.
fn apply(ctx: &egui::Context, st: &mut AppState, backend: &Backend, action: Action) {
    match action {
        Action::Refresh => refresh(st, backend),
        Action::OpenSettings => st.view = View::Settings,
        Action::PickKubeconfig => open_kubeconfig_picker(ctx.clone()),
        Action::ImportDefault => {
            let id = st.next_id();
            st.pending
                .insert(id, "Import du kubeconfig du système".to_string());
            backend.send(Command::ImportKubeconfig {
                id,
                path: None,
                all_contexts: true,
            });
        }
    }
}

/// Recharge la synthèse et les évènements du cluster courant.
fn refresh(st: &mut AppState, backend: &Backend) {
    st.last_refresh = Instant::now();

    let Some(cluster) = st.current_cluster.clone() else {
        // Sans cluster courant, on se contente de rafraîchir la liste.
        let id = st.next_id();
        st.pending.insert(id, "Liste des clusters".to_string());
        backend.send(Command::RefreshClusters { id });
        return;
    };

    let id = st.next_id();
    st.pending.insert(id, format!("Synthèse de {cluster}"));
    backend.send(Command::LoadOverview {
        id,
        cluster: cluster.clone(),
    });

    let id = st.next_id();
    st.pending.insert(id, "Évènements récents".to_string());
    backend.send(Command::LoadEvents {
        id,
        cluster,
        namespace: None,
        involved: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lisible() {
        assert_eq!(format_cpu(0.0), "0 m");
        assert_eq!(format_cpu(250.0), "250 m");
        assert_eq!(format_cpu(1250.0), "1.25 cœurs");
    }

    #[test]
    fn octets_lisibles() {
        assert_eq!(format_bytes(0), "0 o");
        assert_eq!(format_bytes(-42), "0 o");
        assert_eq!(format_bytes(512), "512 o");
        assert_eq!(format_bytes(1024), "1.0 Kio");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 Gio");
    }

    #[test]
    fn fraction_bornee() {
        // Une capacité nulle ne doit jamais produire NaN ni infini.
        assert_eq!(fraction(10.0, 0.0), 0.0);
        assert_eq!(fraction(0.0, 100.0), 0.0);
        assert_eq!(fraction(50.0, 100.0), 0.5);
        // Une consommation supérieure à la capacité reste bornée à 1.
        assert_eq!(fraction(150.0, 100.0), 1.0);
    }

    #[test]
    fn age_d_evenement_sans_horodatage() {
        let event = EventSummary::default();
        assert!(event_age(&event).is_none());
        assert_eq!(format_age(event_age(&event)), "—");
    }

    #[test]
    fn age_d_evenement_recent() {
        let event = EventSummary {
            last_seen: Some(chrono::Utc::now() - chrono::Duration::seconds(30)),
            ..Default::default()
        };
        let age = event_age(&event).unwrap_or(-1);
        assert!((29..=31).contains(&age), "âge inattendu : {age}");
    }
}
