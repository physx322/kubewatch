//! Panneau de détail de la ressource sélectionnée.
//!
//! Le panneau est créé ici (`egui::Panel::right("detail")`) : l'appelant se
//! contente d'invoquer [`show`] après la vue courante. Rien n'est dessiné tant
//! qu'aucune ressource n'est sélectionnée.
//!
//! Discipline des flux — règle absolue : les onglets « Journaux » et
//! « Terminal » ouvrent une connexion vers le cluster. Elle est **toujours**
//! refermée lorsque l'on quitte l'onglet, que l'on change de conteneur ou
//! d'options, que l'on change de ressource, ou que l'on ferme le panneau. Un
//! flux oublié laisse une connexion ouverte sur le cluster.
//!
//! Cette vue ne fait aucun appel réseau : elle lit `AppState` et envoie des
//! `Command` au worker.
//!
//! # Mémoire locale
//! `AppState` est un contrat figé et ne porte pas de marqueur « cette requête a
//! déjà été émise ». Le suivi ci-dessous, purement local à la vue, évite de
//! redemander les mêmes données à chaque image lorsque la réponse est vide
//! (un objet sans évènement, par exemple).

use egui::{Color32, RichText};

use kubewatch_core::apply::split_documents;
use kubewatch_core::logs::DEFAULT_TAIL_LINES;
use kubewatch_core::model::{ObjectSummary, ResourceRef};

use crate::backend::{Backend, Command};
use crate::format;
use crate::icons;
use crate::state::{AppState, DetailTab};
use crate::theme::{self, Palette};
use crate::widgets::log_view::LogView;
use crate::widgets::term::TerminalInput;
use crate::widgets::yaml_edit::YamlEditor;

/// Longueur au-delà de laquelle la valeur d'une annotation est repliée.
const ANNOTATION_MAX: usize = 160;

/// Ce que la vue a déjà demandé pour l'objet observé.
///
/// Sans ce suivi, un objet dépourvu d'évènements provoquerait une requête par
/// image : la réponse vide ne laisse aucune trace dans `AppState`.
#[derive(Debug, Clone, Default)]
struct Suivi {
    /// Objet auquel se rapportent les marqueurs ci-dessous.
    reference: Option<ResourceRef>,
    /// Onglet dessiné à l'image précédente.
    tab: Option<DetailTab>,
    /// Manifeste demandé.
    yaml: bool,
    /// Évènements demandés.
    events: bool,
    /// Conteneurs demandés.
    containers: bool,
}

impl Suivi {
    /// Valeur initiale, utilisable dans un `static`.
    const VIDE: Self = Self {
        reference: None,
        tab: None,
        yaml: false,
        events: false,
        containers: false,
    };

    /// Oublie tout ce qui se rapportait à l'objet précédent.
    fn oublier(&mut self, reference: Option<ResourceRef>) {
        self.reference = reference;
        self.yaml = false;
        self.events = false;
        self.containers = false;
    }
}

/// Suivi des requêtes déjà émises (voir [`Suivi`]).
static SUIVI: parking_lot::Mutex<Suivi> = parking_lot::Mutex::new(Suivi::VIDE);

// ---------------------------------------------------------------------------
// Point d'entrée
// ---------------------------------------------------------------------------

/// Dessine le panneau de détail si une ressource est sélectionnée.
pub fn show(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let Some(reference) = synchroniser(st, backend) else {
        return;
    };

    egui::Panel::right("detail")
        .resizable(true)
        .default_size(540.0)
        .min_size(340.0)
        .show(ui, |ui| {
            panneau(ui, st, backend, &reference);
        });
}

/// Aligne le panneau sur la sélection courante et renvoie l'objet à afficher.
///
/// Les flux sont fermés **avant** que `DetailState::focus` n'efface leurs
/// identifiants : sans cela, la connexion resterait ouverte côté cluster.
fn synchroniser(st: &mut AppState, backend: &Backend) -> Option<ResourceRef> {
    let selection = st.selected.clone();
    let precedente = SUIVI.lock().reference.clone();

    if selection != precedente {
        stop_streams(st, backend);
        SUIVI.lock().oublier(selection.clone());
        match selection.clone() {
            Some(reference) => st.detail.focus(reference),
            None => {
                st.detail.reset_data();
                st.detail.reference = None;
                st.detail.close();
            }
        }
    }

    if !st.detail.open {
        return None;
    }
    selection
}

/// Contenu du panneau : en-tête, résumé, onglets, corps de l'onglet actif.
fn panneau(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend, reference: &ResourceRef) {
    let palette = theme::palette(st.settings.dark);
    let mut fermer = false;

    ui.horizontal(|ui| {
        if ui
            .button(RichText::new(icons::CLOSE).strong())
            .on_hover_text("Fermer le panneau et arrêter les flux en cours")
            .clicked()
        {
            fermer = true;
        }
        ui.add(egui::Label::new(RichText::new(reference.name.as_str()).heading()).truncate());
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(reference.kind.as_str()).color(palette.accent));
        ui.label(
            RichText::new(reference.api_version())
                .small()
                .color(palette.muted),
        );
        let portee = match reference.namespace.as_deref() {
            Some(ns) if !ns.is_empty() => format!("· namespace {ns}"),
            _ => "· portée cluster".to_string(),
        };
        ui.label(RichText::new(portee).small().color(palette.muted));
    });

    if fermer {
        stop_streams(st, backend);
        st.detail.reset_data();
        st.detail.reference = None;
        st.detail.close();
        st.selected = None;
        SUIVI.lock().oublier(None);
        return;
    }

    let Some(cluster) = st.current_cluster.clone() else {
        ui.add_space(6.0);
        ui.colored_label(palette.warn, "Aucun cluster sélectionné.");
        return;
    };

    // Le manifeste alimente aussi le résumé (propriétaires) : on le demande dès
    // l'ouverture du panneau, quel que soit l'onglet actif.
    demander_yaml(st, backend, &cluster, reference, false);

    ui.add_space(4.0);
    resume(ui, st, backend, reference, &palette);
    ui.add_space(4.0);
    ui.separator();

    // --- Onglets ----------------------------------------------------------
    let precedent = st.detail.tab;
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut st.detail.tab, DetailTab::Yaml, "YAML");
        ui.selectable_value(&mut st.detail.tab, DetailTab::Events, "Évènements");
        ui.selectable_value(&mut st.detail.tab, DetailTab::Containers, "Conteneurs");
        ui.selectable_value(&mut st.detail.tab, DetailTab::Logs, "Journaux");
        ui.selectable_value(&mut st.detail.tab, DetailTab::Exec, "Terminal");
    });

    let vu = SUIVI.lock().tab;
    let change = vu != Some(st.detail.tab);
    if change {
        // On quitte un onglet : ce qu'il avait ouvert doit être refermé.
        match precedent {
            DetailTab::Logs if st.detail.tab != DetailTab::Logs => stop_logs(st, backend),
            DetailTab::Exec if st.detail.tab != DetailTab::Exec => stop_exec(st, backend),
            _ => {}
        }
        SUIVI.lock().tab = Some(st.detail.tab);
        // Ouverture de l'onglet « Journaux » : le flux démarre tout de suite.
        if st.detail.tab == DetailTab::Logs && est_pod(reference) && st.detail.log_stream.is_none()
        {
            demander_conteneurs(st, backend, &cluster, reference, false);
            start_logs(st, backend, &cluster, reference);
        }
    }

    ui.add_space(4.0);

    match st.detail.tab {
        DetailTab::Yaml => onglet_yaml(ui, st, backend, &cluster, reference, &palette),
        DetailTab::Events => {
            demander_evenements(st, backend, &cluster, reference, false);
            onglet_evenements(ui, st, backend, &cluster, reference, &palette);
        }
        DetailTab::Containers => {
            demander_conteneurs(st, backend, &cluster, reference, false);
            onglet_conteneurs(ui, st, backend, &cluster, reference, &palette);
        }
        DetailTab::Logs => {
            demander_conteneurs(st, backend, &cluster, reference, false);
            onglet_journaux(ui, st, backend, &cluster, reference, &palette);
        }
        DetailTab::Exec => {
            demander_conteneurs(st, backend, &cluster, reference, false);
            onglet_terminal(ui, st, backend, &cluster, reference, &palette);
        }
    }
}

// ---------------------------------------------------------------------------
// Résumé
// ---------------------------------------------------------------------------

/// Propriétaire déclaré dans `metadata.ownerReferences`.
#[derive(Clone, Debug)]
struct Proprietaire {
    api_version: String,
    kind: String,
    name: String,
    controller: bool,
}

/// Bloc repliable : métadonnées, labels, annotations, propriétaires.
fn resume(
    ui: &mut egui::Ui,
    st: &mut AppState,
    backend: &Backend,
    reference: &ResourceRef,
    palette: &Palette,
) {
    // Tout est copié avant d'entrer dans les fermetures : l'état ne change
    // qu'à la sortie, ce qui garde les emprunts simples.
    let ligne = ligne_courante(st, reference);
    let document = premier_document(&st.detail.yaml);
    let mut navigation: Option<Proprietaire> = None;
    let mut annotations_ouvertes = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(egui::Id::new("detail-annotations-ouvertes")))
        .unwrap_or(false);

    egui::CollapsingHeader::new(RichText::new("Résumé").strong())
        .id_salt("detail-resume")
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new("detail-meta")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    champ(ui, palette, "Nom", &reference.name);
                    champ(ui, palette, "Kind", &reference.kind);
                    champ(ui, palette, "apiVersion", &reference.api_version());
                    champ(
                        ui,
                        palette,
                        "Namespace",
                        reference
                            .namespace
                            .as_deref()
                            .filter(|ns| !ns.is_empty())
                            .unwrap_or("— (portée cluster)"),
                    );

                    if let Some(r) = ligne.as_ref() {
                        champ(ui, palette, "Création", &format::timestamp(r.created_at));
                        let age = match r.age_seconds {
                            Some(secondes) => format::age(secondes),
                            None => format::age_since(r.created_at),
                        };
                        champ(ui, palette, "Âge", &age);
                        champ(
                            ui,
                            palette,
                            "UID",
                            r.uid.as_deref().unwrap_or(format::UNKNOWN),
                        );
                        if !r.status.is_empty() {
                            ui.label(RichText::new("Statut").small().color(palette.muted));
                            ui.colored_label(
                                theme::status_color(palette, &r.status),
                                r.status.as_str(),
                            );
                            ui.end_row();
                        }
                        if let Some(ready) = r.ready.as_deref() {
                            champ(ui, palette, "Prêts", ready);
                        }
                        if let Some(restarts) = r.restarts {
                            champ(ui, palette, "Redémarrages", &restarts.to_string());
                        }
                        if let Some(node) = r.node.as_deref() {
                            champ(ui, palette, "Nœud", node);
                        }
                    } else if let Some(doc) = document.as_ref() {
                        // La ligne du tableau n'est plus disponible : on se rabat
                        // sur les métadonnées du manifeste.
                        if let Some(uid) = chaine(doc, &["metadata", "uid"]) {
                            champ(ui, palette, "UID", &uid);
                        }
                        if let Some(date) = chaine(doc, &["metadata", "creationTimestamp"]) {
                            champ(ui, palette, "Création", &date);
                        }
                    }
                });

            // --- Labels ---------------------------------------------------
            let labels = ligne.as_ref().map(|r| r.labels.clone()).unwrap_or_default();
            ui.add_space(4.0);
            if labels.is_empty() {
                ui.label(RichText::new("Aucun label.").small().color(palette.muted));
            } else {
                egui::CollapsingHeader::new(format!("Labels ({})", labels.len()))
                    .id_salt("detail-labels")
                    .default_open(true)
                    .show(ui, |ui| {
                        egui::Grid::new("detail-labels-grid")
                            .num_columns(2)
                            .spacing([10.0, 2.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for (cle, valeur) in &labels {
                                    ui.label(RichText::new(cle.as_str()).monospace().small());
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(valeur.as_str()).monospace().small(),
                                        )
                                        .wrap(),
                                    );
                                    ui.end_row();
                                }
                            });
                    });
            }

            // --- Annotations ----------------------------------------------
            let annotations = ligne
                .as_ref()
                .map(|r| r.annotations.clone())
                .unwrap_or_default();
            if !annotations.is_empty() {
                let volumineuses = annotations
                    .values()
                    .any(|v| v.chars().count() > ANNOTATION_MAX);
                egui::CollapsingHeader::new(format!("Annotations ({})", annotations.len()))
                    .id_salt("detail-annotations")
                    .default_open(false)
                    .show(ui, |ui| {
                        if volumineuses {
                            let libelle = if annotations_ouvertes {
                                "Replier les valeurs longues"
                            } else {
                                "Tout afficher"
                            };
                            if ui.small_button(libelle).clicked() {
                                annotations_ouvertes = !annotations_ouvertes;
                            }
                        }
                        egui::Grid::new("detail-annotations-grid")
                            .num_columns(2)
                            .spacing([10.0, 2.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for (cle, valeur) in &annotations {
                                    ui.label(RichText::new(cle.as_str()).monospace().small());
                                    let texte = if annotations_ouvertes {
                                        valeur.clone()
                                    } else {
                                        format::truncate(valeur, ANNOTATION_MAX)
                                    };
                                    ui.add(
                                        egui::Label::new(RichText::new(texte).monospace().small())
                                            .wrap(),
                                    );
                                    ui.end_row();
                                }
                            });
                    });
            }

            // --- Propriétaires ---------------------------------------------
            ui.add_space(4.0);
            match document.as_ref() {
                Some(doc) => {
                    let liste = proprietaires(doc);
                    if liste.is_empty() {
                        ui.label(
                            RichText::new("Aucun propriétaire déclaré.")
                                .small()
                                .color(palette.muted),
                        );
                    } else {
                        ui.label(RichText::new("Propriétaires").strong());
                        for p in liste {
                            ui.horizontal(|ui| {
                                let libelle = format!("{} / {}", p.kind, p.name);
                                if ui
                                    .link(RichText::new(libelle).monospace().small())
                                    .on_hover_text("Ouvrir l'objet parent")
                                    .clicked()
                                {
                                    navigation = Some(p.clone());
                                }
                                if p.controller {
                                    ui.label(
                                        RichText::new("contrôleur").small().color(palette.muted),
                                    );
                                }
                            });
                        }
                    }
                }
                None => {
                    ui.label(
                        RichText::new("Propriétaires : lecture du manifeste en cours…")
                            .small()
                            .color(palette.muted),
                    );
                }
            }
        });

    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new("detail-annotations-ouvertes"),
            annotations_ouvertes,
        );
    });

    if let Some(p) = navigation {
        naviguer(st, backend, reference, &p);
    }
}

/// Bascule la sélection vers l'objet parent et recharge la liste correspondante.
fn naviguer(st: &mut AppState, backend: &Backend, courant: &ResourceRef, parent: &Proprietaire) {
    let (groupe, version) = match parent.api_version.split_once('/') {
        Some((g, v)) => (g.to_string(), v.to_string()),
        None => (String::new(), parent.api_version.clone()),
    };

    let connu = st
        .kinds
        .iter()
        .find(|k| k.kind == parent.kind && k.group == groupe)
        .cloned();
    let pluriel = connu
        .as_ref()
        .map(|k| k.plural.clone())
        .unwrap_or_else(|| format!("{}s", parent.kind.to_lowercase()));
    let namespaced = connu
        .as_ref()
        .map(|k| k.namespaced)
        .unwrap_or_else(|| courant.namespace.is_some());

    let cible = ResourceRef {
        group: groupe,
        version,
        kind: parent.kind.clone(),
        plural: pluriel.clone(),
        namespace: if namespaced {
            courant.namespace.clone()
        } else {
            None
        },
        name: parent.name.clone(),
    };

    st.selected_kind = pluriel;
    st.selected = Some(cible);

    // Le tableau doit suivre, sinon la ligne du parent n'y figure pas.
    if let Some(cluster) = st.current_cluster.clone() {
        let opts = st.list_options();
        let kind = st.selected_kind.clone();
        let id = st.begin(format!("Chargement de {kind}"));
        backend.send(Command::ListResources {
            id,
            cluster,
            kind,
            opts,
        });
    }
}

// ---------------------------------------------------------------------------
// Onglet « YAML »
// ---------------------------------------------------------------------------

fn onglet_yaml(
    ui: &mut egui::Ui,
    st: &mut AppState,
    backend: &Backend,
    cluster: &str,
    reference: &ResourceRef,
    palette: &Palette,
) {
    // L'éditeur est reconstruit à chaque image autour du texte de l'état : le
    // contrat de `AppState` stocke un `String`, pas le composant.
    let mut editeur = YamlEditor {
        text: std::mem::take(&mut st.detail.yaml),
        ..YamlEditor::default()
    };
    let validation = editeur.validate();
    let vide = editeur.is_blank();
    let en_vol = st.detail.yaml_pending.is_some();
    let modifie = st.detail.yaml_edited;

    let mut recharger = false;
    let mut appliquer = false;

    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Recharger")
            .on_hover_text("Relire l'objet depuis le cluster et abandonner les retouches")
            .clicked()
        {
            recharger = true;
        }
        if ui
            .add_enabled(!vide, egui::Button::new("Copier"))
            .on_hover_text("Copier le manifeste dans le presse-papiers")
            .clicked()
        {
            ui.ctx().copy_text(editeur.text.clone());
        }
        if ui
            .add_enabled(
                validation.is_ok() && !en_vol,
                egui::Button::new(RichText::new("Appliquer").strong()),
            )
            .on_hover_text("Remplacer l'objet dans le cluster par ce manifeste")
            .clicked()
        {
            appliquer = true;
        }
        if en_vol {
            ui.spinner();
            ui.label(
                RichText::new("lecture en cours…")
                    .small()
                    .color(palette.muted),
            );
        }
        if modifie {
            ui.colored_label(palette.warn, RichText::new("modifié").small());
        }
    });

    match &validation {
        Ok(documents) => {
            ui.colored_label(
                palette.ok,
                RichText::new(format!("{documents} document(s) valide(s)")).small(),
            );
        }
        Err(message) => {
            ui.add(
                egui::Label::new(
                    RichText::new(message.as_str())
                        .color(if vide { palette.muted } else { palette.error })
                        .small(),
                )
                .wrap(),
            );
        }
    }

    ui.separator();
    let reponse = editeur.show(ui, "detail-yaml");

    // On rend le texte à l'état, modifications comprises.
    let texte = std::mem::take(&mut editeur.text);
    if reponse.changed() {
        st.detail.yaml_edited = true;
    }
    st.detail.yaml = texte.clone();

    if recharger {
        st.detail.yaml_edited = false;
        demander_yaml(st, backend, cluster, reference, true);
    }
    if appliquer {
        let id = st.begin(format!("Application de {}", reference.display()));
        backend.send(Command::ReplaceYaml {
            id,
            cluster: cluster.to_string(),
            reference: reference.clone(),
            yaml: texte,
        });
    }
}

// ---------------------------------------------------------------------------
// Onglet « Évènements »
// ---------------------------------------------------------------------------

fn onglet_evenements(
    ui: &mut egui::Ui,
    st: &mut AppState,
    backend: &Backend,
    cluster: &str,
    reference: &ResourceRef,
    palette: &Palette,
) {
    let evenements = st.detail.events.clone();
    let en_vol = st.detail.events_pending.is_some();
    let mut recharger = false;

    ui.horizontal(|ui| {
        if ui.button("Actualiser").clicked() {
            recharger = true;
        }
        ui.label(
            RichText::new(format::plural(evenements.len(), "évènement"))
                .small()
                .color(palette.muted),
        );
        if en_vol {
            ui.spinner();
        }
    });
    ui.separator();

    if evenements.is_empty() {
        let message = if en_vol {
            "Chargement des évènements…"
        } else {
            "Aucun évènement pour cet objet."
        };
        ui.label(RichText::new(message).small().color(palette.muted));
    } else {
        let maintenant = chrono::Utc::now();
        egui::ScrollArea::vertical()
            .id_salt("detail-events")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("detail-events-grid")
                    .num_columns(4)
                    .spacing([10.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for entete in ["Type", "Raison", "Âge", "Message"] {
                            ui.label(RichText::new(entete).small().strong());
                        }
                        ui.end_row();

                        for e in &evenements {
                            ui.colored_label(
                                couleur_type_evenement(palette, &e.type_),
                                RichText::new(e.type_.as_str()).small(),
                            );
                            ui.label(RichText::new(e.reason.as_str()).monospace().small());
                            let age = match e.last_seen.or(e.first_seen) {
                                Some(t) => format::age((maintenant - t).num_seconds()),
                                None => format::UNKNOWN.to_string(),
                            };
                            let age = if e.count > 1 {
                                format!("{age} (×{})", e.count)
                            } else {
                                age
                            };
                            ui.label(RichText::new(age).small());
                            ui.add(
                                egui::Label::new(RichText::new(e.message.as_str()).small()).wrap(),
                            );
                            ui.end_row();
                        }
                    });
            });
    }

    if recharger {
        demander_evenements(st, backend, cluster, reference, true);
    }
}

// ---------------------------------------------------------------------------
// Onglet « Conteneurs »
// ---------------------------------------------------------------------------

fn onglet_conteneurs(
    ui: &mut egui::Ui,
    st: &mut AppState,
    backend: &Backend,
    cluster: &str,
    reference: &ResourceRef,
    palette: &Palette,
) {
    if !est_pod(reference) {
        ui.colored_label(
            palette.warn,
            "La liste des conteneurs n'existe que pour les pods.",
        );
        return;
    }

    let conteneurs = st.detail.containers.clone();
    let choisi = st.detail.container.clone();
    let mut recharger = false;
    let mut selection: Option<String> = None;

    ui.horizontal(|ui| {
        if ui.button("Actualiser").clicked() {
            recharger = true;
        }
        ui.label(
            RichText::new(format::plural(conteneurs.len(), "conteneur"))
                .small()
                .color(palette.muted),
        );
    });
    ui.separator();

    if conteneurs.is_empty() {
        ui.label(
            RichText::new("Aucun conteneur connu pour l'instant.")
                .small()
                .color(palette.muted),
        );
    } else {
        egui::ScrollArea::vertical()
            .id_salt("detail-containers")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("detail-containers-grid")
                    .num_columns(6)
                    .spacing([10.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for entete in ["Nom", "Image", "Prêt", "Redém.", "État", ""] {
                            ui.label(RichText::new(entete).small().strong());
                        }
                        ui.end_row();

                        for c in &conteneurs {
                            let nom = if c.init {
                                format!("{} (init)", c.name)
                            } else {
                                c.name.clone()
                            };
                            let actif = choisi.as_deref() == Some(c.name.as_str());
                            let nom = if actif {
                                format!("{} {nom}", icons::CURRENT)
                            } else {
                                nom
                            };
                            ui.label(RichText::new(nom).monospace().small());
                            ui.add(
                                egui::Label::new(
                                    RichText::new(c.image.as_str()).monospace().small(),
                                )
                                .truncate(),
                            );
                            if c.ready {
                                ui.colored_label(palette.ok, "oui");
                            } else {
                                ui.colored_label(palette.warn, "non");
                            }
                            ui.label(RichText::new(c.restart_count.to_string()).small());
                            ui.colored_label(
                                theme::status_color(palette, etat_court(&c.state)),
                                RichText::new(c.state.as_str()).small(),
                            );
                            if ui
                                .small_button("Choisir")
                                .on_hover_text(
                                    "Cibler ce conteneur pour les journaux et le terminal",
                                )
                                .clicked()
                            {
                                selection = Some(c.name.clone());
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    if let Some(nom) = selection {
        // Changer de conteneur invalide le flux en cours : on le referme.
        stop_logs(st, backend);
        st.detail.container = Some(nom);
    }
    if recharger {
        demander_conteneurs(st, backend, cluster, reference, true);
    }
}

// ---------------------------------------------------------------------------
// Onglet « Journaux »
// ---------------------------------------------------------------------------

fn onglet_journaux(
    ui: &mut egui::Ui,
    st: &mut AppState,
    backend: &Backend,
    cluster: &str,
    reference: &ResourceRef,
    palette: &Palette,
) {
    if !est_pod(reference) {
        ui.colored_label(palette.warn, "Les journaux n'existent que pour les pods.");
        return;
    }

    let conteneurs = st.detail.containers.clone();
    let mut demarrer = false;
    let mut arreter = false;
    let mut changement = false;

    ui.horizontal_wrapped(|ui| {
        let choisi = st
            .detail
            .container
            .clone()
            .unwrap_or_else(|| "(automatique)".to_string());
        egui::ComboBox::from_id_salt("detail-log-container")
            .selected_text(choisi)
            .width(180.0)
            .show_ui(ui, |ui| {
                let auto = st.detail.container.is_none();
                if ui.selectable_label(auto, "(automatique)").clicked() && !auto {
                    st.detail.container = None;
                    changement = true;
                }
                for c in &conteneurs {
                    let actif = st.detail.container.as_deref() == Some(c.name.as_str());
                    let libelle = if c.init {
                        format!("{} (init)", c.name)
                    } else {
                        c.name.clone()
                    };
                    if ui.selectable_label(actif, libelle).clicked() && !actif {
                        st.detail.container = Some(c.name.clone());
                        changement = true;
                    }
                }
            });

        if ui
            .checkbox(&mut st.detail.log_options.follow, "Flux continu")
            .on_hover_text("Rester connecté et recevoir les nouvelles lignes (kubectl logs -f)")
            .changed()
        {
            changement = true;
        }
        if ui
            .checkbox(&mut st.detail.log_options.timestamps, "Horodatage")
            .changed()
        {
            changement = true;
        }
        if ui
            .checkbox(&mut st.detail.log_options.previous, "Instance précédente")
            .on_hover_text("Journal du conteneur avant son dernier redémarrage")
            .changed()
        {
            changement = true;
        }

        let mut lignes = st
            .detail
            .log_options
            .tail_lines
            .unwrap_or(DEFAULT_TAIL_LINES);
        if ui
            .add(
                egui::DragValue::new(&mut lignes)
                    .range(10..=100_000)
                    .speed(10.0)
                    .suffix(" lignes"),
            )
            .on_hover_text("Nombre de lignes lues depuis la fin du journal")
            .changed()
        {
            st.detail.log_options.tail_lines = Some(lignes.max(1));
            changement = true;
        }
    });

    ui.horizontal_wrapped(|ui| {
        if st.detail.log_stream.is_some() {
            if ui.button("Arrêter").clicked() {
                arreter = true;
            }
            ui.colored_label(palette.ok, RichText::new("flux ouvert").small());
        } else {
            if ui.button("Démarrer").clicked() {
                demarrer = true;
            }
            ui.colored_label(palette.muted, RichText::new("flux arrêté").small());
        }
    });

    ui.separator();

    // Le composant est monté autour des données de l'état, puis rendu.
    let mut vue = LogView {
        lines: std::mem::take(&mut st.detail.log_lines),
        follow: st.detail.log_stick_to_bottom,
        filter: std::mem::take(&mut st.detail.log_filter),
        wrap: st.detail.log_wrap,
        max_lines: st.detail.log_max_lines.max(1),
    };
    vue.show(ui);
    st.detail.log_lines = std::mem::take(&mut vue.lines);
    st.detail.log_filter = std::mem::take(&mut vue.filter);
    st.detail.log_stick_to_bottom = vue.follow;
    st.detail.log_wrap = vue.wrap;

    if arreter {
        stop_logs(st, backend);
    }
    if demarrer {
        start_logs(st, backend, cluster, reference);
    }
    // Changer de conteneur ou d'options impose de refermer le flux courant
    // avant d'en ouvrir un nouveau : sinon la connexion précédente fuit.
    if changement && st.detail.log_stream.is_some() {
        stop_logs(st, backend);
        start_logs(st, backend, cluster, reference);
    }
}

// ---------------------------------------------------------------------------
// Onglet « Terminal »
// ---------------------------------------------------------------------------

fn onglet_terminal(
    ui: &mut egui::Ui,
    st: &mut AppState,
    backend: &Backend,
    cluster: &str,
    reference: &ResourceRef,
    palette: &Palette,
) {
    if !est_pod(reference) {
        ui.colored_label(palette.warn, "Le terminal n'existe que pour les pods.");
        return;
    }

    let conteneurs = st.detail.containers.clone();
    let mut connecter = false;
    let mut deconnecter = false;
    let mut changement = false;

    ui.horizontal_wrapped(|ui| {
        let choisi = st
            .detail
            .container
            .clone()
            .unwrap_or_else(|| "(automatique)".to_string());
        egui::ComboBox::from_id_salt("detail-exec-container")
            .selected_text(choisi)
            .width(180.0)
            .show_ui(ui, |ui| {
                let auto = st.detail.container.is_none();
                if ui.selectable_label(auto, "(automatique)").clicked() && !auto {
                    st.detail.container = None;
                    changement = true;
                }
                for c in conteneurs.iter().filter(|c| !c.init) {
                    let actif = st.detail.container.as_deref() == Some(c.name.as_str());
                    if ui.selectable_label(actif, c.name.as_str()).clicked() && !actif {
                        st.detail.container = Some(c.name.clone());
                        changement = true;
                    }
                }
            });

        ui.add(
            egui::TextEdit::singleline(&mut st.detail.exec_command)
                .desired_width(160.0)
                .hint_text("/bin/sh"),
        );

        if st.detail.exec_session.is_some() {
            if ui.button("Déconnecter").clicked() {
                deconnecter = true;
            }
            ui.colored_label(palette.ok, RichText::new("session ouverte").small());
        } else {
            if ui.button("Connecter").clicked() {
                connecter = true;
            }
            ui.colored_label(palette.muted, RichText::new("session fermée").small());
        }
    });

    ui.separator();

    // Sans session ouverte, la grille reste visible mais inerte : on le dit,
    // plutôt que de laisser croire à un terminal muet.
    if st.detail.exec_session.is_none() && st.detail.terminal.line_count() == 0 {
        ui.label(
            RichText::new("Choisissez un conteneur puis cliquez sur « Connecter ».")
                .small()
                .color(palette.muted),
        );
    }

    // --- Grille --------------------------------------------------------------
    // L'émulateur porte le rendu ANSI, la taille déduite de la police et la
    // traduction des touches. Il ne connaît pas le cluster : il renvoie les
    // octets frappés et les changements de grille, que l'on relaie tels quels.
    let sorties = st.detail.terminal.show(ui);

    if let Some(id) = st.detail.exec_session {
        for sortie in sorties {
            match sortie {
                TerminalInput::Bytes(data) => backend.send(Command::ExecInput { id, data }),
                TerminalInput::Resize { cols, rows } => {
                    backend.send(Command::ExecResize { id, cols, rows });
                }
            }
        }
    }

    // Changer de conteneur en cours de session ferme la session : on ne laisse
    // jamais un `exec` orphelin derrière soi.
    if changement && st.detail.exec_session.is_some() {
        stop_exec(st, backend);
    }
    if deconnecter {
        stop_exec(st, backend);
    }
    if connecter {
        start_exec(st, backend, cluster, reference);
    }
}

// ---------------------------------------------------------------------------
// Chargements et flux
// ---------------------------------------------------------------------------

/// Demande le manifeste de l'objet, une seule fois sauf demande explicite.
fn demander_yaml(
    st: &mut AppState,
    backend: &Backend,
    cluster: &str,
    reference: &ResourceRef,
    force: bool,
) {
    {
        let mut suivi = SUIVI.lock();
        if !force && (suivi.yaml || st.detail.yaml_pending.is_some()) {
            return;
        }
        suivi.yaml = true;
    }
    let id = st.begin(format!("Lecture du YAML de {}", reference.display()));
    st.detail.yaml_pending = Some(id);
    backend.send(Command::GetYaml {
        id,
        cluster: cluster.to_string(),
        reference: reference.clone(),
    });
}

/// Demande les évènements liés à l'objet.
fn demander_evenements(
    st: &mut AppState,
    backend: &Backend,
    cluster: &str,
    reference: &ResourceRef,
    force: bool,
) {
    {
        let mut suivi = SUIVI.lock();
        if !force && (suivi.events || st.detail.events_pending.is_some()) {
            return;
        }
        suivi.events = true;
    }
    let id = st.begin(format!("Évènements de {}", reference.display()));
    st.detail.events_pending = Some(id);
    backend.send(Command::LoadEvents {
        id,
        cluster: cluster.to_string(),
        namespace: reference.namespace.clone(),
        involved: Some(reference.clone()),
    });
}

/// Demande la liste des conteneurs du pod.
fn demander_conteneurs(
    st: &mut AppState,
    backend: &Backend,
    cluster: &str,
    reference: &ResourceRef,
    force: bool,
) {
    if !est_pod(reference) {
        return;
    }
    {
        let mut suivi = SUIVI.lock();
        if !force && suivi.containers {
            return;
        }
        suivi.containers = true;
    }
    let id = st.begin(format!("Conteneurs de {}", reference.display()));
    backend.send(Command::LoadContainers {
        id,
        cluster: cluster.to_string(),
        pod: reference.clone(),
    });
}

/// Ouvre un flux de journaux (après avoir fermé un éventuel flux précédent).
fn start_logs(st: &mut AppState, backend: &Backend, cluster: &str, reference: &ResourceRef) {
    stop_logs(st, backend);

    let mut opts = st.detail.log_options.clone();
    opts.container = st.detail.container.clone();
    // Le serveur d'API refuse `follow` sur l'instance précédente : elle est terminée.
    if opts.previous {
        opts.follow = false;
    }
    if opts.tail_lines.is_none() {
        opts.tail_lines = Some(DEFAULT_TAIL_LINES);
    }

    let id = st.begin(format!("Journaux de {}", reference.display()));
    st.detail.log_stream = Some(id);
    st.detail.log_lines.clear();
    backend.send(Command::StartLogs {
        id,
        cluster: cluster.to_string(),
        pod: reference.clone(),
        opts,
    });
}

/// Ferme le flux de journaux s'il y en a un.
fn stop_logs(st: &mut AppState, backend: &Backend) {
    if let Some(id) = st.detail.log_stream.take() {
        let _ = st.finish(id);
        backend.send(Command::StopLogs { id });
    }
}

/// Ouvre une session interactive (après avoir fermé une éventuelle session).
fn start_exec(st: &mut AppState, backend: &Backend, cluster: &str, reference: &ResourceRef) {
    stop_exec(st, backend);

    let commande = decouper_commande(&st.detail.exec_command);
    let id = st.begin(format!("Terminal sur {}", reference.display()));
    st.detail.exec_session = Some(id);
    backend.send(Command::StartExec {
        id,
        cluster: cluster.to_string(),
        pod: reference.clone(),
        container: st.detail.container.clone(),
        command: commande,
    });
    // L'émulateur n'annonce une taille que lorsqu'elle change : la session qui
    // s'ouvre doit donc recevoir la taille courante explicitement, sans quoi le
    // shell distant garderait un 80×24 par défaut. Le worker mémorise cette
    // taille si elle arrive avant l'ouverture effective de la session.
    let (cols, rows) = st.detail.terminal.size();
    backend.send(Command::ExecResize { id, cols, rows });
}

/// Ferme la session interactive s'il y en a une.
fn stop_exec(st: &mut AppState, backend: &Backend) {
    if let Some(id) = st.detail.exec_session.take() {
        let _ = st.finish(id);
        backend.send(Command::StopExec { id });
    }
}

/// Ferme tous les flux ouverts par le panneau.
fn stop_streams(st: &mut AppState, backend: &Backend) {
    stop_logs(st, backend);
    stop_exec(st, backend);
}

// ---------------------------------------------------------------------------
// Utilitaires
// ---------------------------------------------------------------------------

/// Vrai si la référence désigne un pod.
fn est_pod(reference: &ResourceRef) -> bool {
    reference.kind.eq_ignore_ascii_case("Pod")
}

/// Retrouve la ligne du tableau correspondant à la ressource sélectionnée.
fn ligne_courante(st: &AppState, reference: &ResourceRef) -> Option<ObjectSummary> {
    st.rows
        .iter()
        .find(|r| {
            r.name == reference.name
                && r.namespace == reference.namespace
                && (r.kind.is_empty() || r.kind == reference.kind)
        })
        .cloned()
}

/// Premier document JSON du manifeste affiché, s'il est analysable.
fn premier_document(texte: &str) -> Option<serde_json::Value> {
    if texte.trim().is_empty() {
        return None;
    }
    let mut documents = split_documents(texte).ok()?;
    if documents.is_empty() {
        None
    } else {
        Some(documents.remove(0))
    }
}

/// Lit une chaîne au bout d'un chemin de clés JSON.
fn chaine(valeur: &serde_json::Value, chemin: &[&str]) -> Option<String> {
    let mut courant = valeur;
    for cle in chemin {
        courant = courant.get(cle)?;
    }
    courant.as_str().map(str::to_string)
}

/// Extrait les `ownerReferences` du manifeste.
fn proprietaires(document: &serde_json::Value) -> Vec<Proprietaire> {
    let Some(liste) = document
        .get("metadata")
        .and_then(|m| m.get("ownerReferences"))
        .and_then(|o| o.as_array())
    else {
        return Vec::new();
    };
    liste
        .iter()
        .filter_map(|o| {
            let kind = o.get("kind").and_then(|v| v.as_str())?.to_string();
            let name = o.get("name").and_then(|v| v.as_str())?.to_string();
            let api_version = o
                .get("apiVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("v1")
                .to_string();
            let controller = o
                .get("controller")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            Some(Proprietaire {
                api_version,
                kind,
                name,
                controller,
            })
        })
        .collect()
}

/// Découpe la commande saisie ; `/bin/sh` par défaut.
fn decouper_commande(saisie: &str) -> Vec<String> {
    let morceaux: Vec<String> = saisie.split_whitespace().map(str::to_string).collect();
    if morceaux.is_empty() {
        vec!["/bin/sh".to_string()]
    } else {
        morceaux
    }
}

/// Ligne « clé / valeur » d'une grille.
fn champ(ui: &mut egui::Ui, palette: &Palette, cle: &str, valeur: &str) {
    ui.label(RichText::new(cle).small().color(palette.muted));
    ui.add(egui::Label::new(RichText::new(valeur).monospace().small()).wrap());
    ui.end_row();
}

/// Premier mot d'un état de conteneur (`waiting: CrashLoopBackOff` → `CrashLoopBackOff`).
fn etat_court(etat: &str) -> &str {
    match etat.split_once(':') {
        Some((_, reste)) => reste.trim(),
        None => etat.trim(),
    }
}

/// Couleur associée au type d'un évènement Kubernetes.
fn couleur_type_evenement(palette: &Palette, type_: &str) -> Color32 {
    if type_.eq_ignore_ascii_case("Warning") {
        palette.warn
    } else if type_.eq_ignore_ascii_case("Error") {
        palette.error
    } else {
        palette.info
    }
}
