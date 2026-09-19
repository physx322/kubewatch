//! Console YAML : éditeur multi-documents, application, comparaison, suppression.
//!
//! Aucune opération bloquante n'est faite ici. Les sélecteurs de fichiers `rfd`
//! tournent sur un fil dédié et sont relevés image par image : l'interface reste
//! fluide même si l'utilisateur laisse la fenêtre de dialogue ouverte. Les
//! échanges avec le cluster passent exclusivement par des `Command`.
//!
//! # Mémoire locale
//! `AppState` est un contrat figé : il ne porte ni l'historique des applications
//! ni l'état de la boîte de confirmation propre à cette vue. Les deux vivent
//! donc ici, dans des variables de module, et ne sortent jamais de la console.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Color32, RichText};

use kubewatch_core::apply::{ApplyAction, ApplyOutcome};

use crate::backend::{Backend, Command};
use crate::format;
use crate::state::AppState;
use crate::theme::{self, Palette};
use crate::widgets::yaml_edit::YamlEditor;

/// Nombre d'applications conservées dans l'historique local.
const HISTORIQUE_MAX: usize = 10;

/// Mot à saisir pour confirmer une suppression.
const MOT_CONFIRMATION: &str = "supprimer";

/// Une application passée, rappelable dans l'éditeur.
#[derive(Clone, Debug)]
struct Entree {
    /// Date de l'envoi.
    at: chrono::DateTime<chrono::Local>,
    /// Manifeste envoyé.
    yaml: String,
    /// Bilan lisible, complété à la réception de la réponse.
    resume: String,
    /// Vrai tant que la réponse n'est pas arrivée.
    en_cours: bool,
}

/// Les dix dernières applications, la plus récente en tête.
static HISTORIQUE: parking_lot::Mutex<Vec<Entree>> = parking_lot::Mutex::new(Vec::new());

/// Texte saisi dans la boîte de confirmation ; `None` quand elle est fermée.
static CONFIRMATION: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

/// Sélecteur « Ouvrir » en cours, relevé image par image (jamais bloquant).
static OUVERTURE: parking_lot::Mutex<Option<Receiver<Option<PathBuf>>>> =
    parking_lot::Mutex::new(None);

/// Sélecteur « Enregistrer » en cours, avec le résultat de l'écriture.
static ENREGISTREMENT: parking_lot::Mutex<Option<Receiver<Result<PathBuf, String>>>> =
    parking_lot::Mutex::new(None);

// ---------------------------------------------------------------------------
// Point d'entrée
// ---------------------------------------------------------------------------

/// Dessine la console YAML.
pub fn show(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let palette = theme::palette(st.settings.dark);

    traiter_fichiers_deposes(ui, st);
    traiter_dialogues(ui, st);
    traiter_collage(ui, st);
    consigner(st);

    // L'éditeur est monté autour du texte de l'état : `AppState` stocke un
    // `String`, pas le composant.
    let mut editeur = YamlEditor {
        text: std::mem::take(&mut st.yaml_console.source),
        ..YamlEditor::default()
    };
    let validation = editeur.validate();

    egui::Panel::top("yaml-console-outils").show(ui, |ui| {
        ui.add_space(2.0);
        barre_outils(ui, st, &mut editeur, &palette);
        ui.add_space(2.0);
        actions(ui, st, backend, &editeur.text, &validation, &palette);
        ui.add_space(2.0);
    });

    egui::Panel::bottom("yaml-console-resultats")
        .resizable(true)
        .default_size(240.0)
        .min_size(90.0)
        .show(ui, |ui| {
            resultats(ui, st, &mut editeur, &palette);
        });

    indication_depot(ui, &palette);
    let _ = editeur.show(ui, "yaml-console");

    // Le texte, retouches comprises, retourne dans l'état.
    st.yaml_console.source = std::mem::take(&mut editeur.text);

    modale_suppression(ui, st, backend, &palette);
}

// ---------------------------------------------------------------------------
// Barre d'outils
// ---------------------------------------------------------------------------

fn barre_outils(ui: &mut egui::Ui, st: &mut AppState, editeur: &mut YamlEditor, palette: &Palette) {
    let ouverture_en_cours = OUVERTURE.lock().is_some();
    let enregistrement_en_cours = ENREGISTREMENT.lock().is_some();
    let namespaces = st.namespaces.clone();

    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!ouverture_en_cours, egui::Button::new("Ouvrir…"))
            .on_hover_text("Charger un manifeste .yaml ou .yml depuis le disque")
            .clicked()
        {
            ouvrir_dialogue();
        }

        if ui
            .add_enabled(
                !enregistrement_en_cours && !editeur.is_blank(),
                egui::Button::new("Enregistrer…"),
            )
            .on_hover_text("Écrire le contenu de l'éditeur dans un fichier")
            .clicked()
        {
            let nom = st
                .yaml_console
                .path
                .as_ref()
                .and_then(|p| p.file_name().and_then(|n| n.to_str()))
                .unwrap_or("manifeste.yaml")
                .to_string();
            enregistrer_dialogue(editeur.text.clone(), nom);
        }

        if ui
            .button("Coller")
            .on_hover_text(
                "Le presse-papiers ne se lit qu'au moment d'un vrai collage : \
                 appuyez sur Ctrl+V, hors de l'éditeur, pour remplacer le manifeste.",
            )
            .clicked()
        {
            st.yaml_console.output =
                "Appuyez sur Ctrl+V pour coller le presse-papiers dans l'éditeur.".to_string();
        }

        if ui
            .add_enabled(!editeur.text.is_empty(), egui::Button::new("Effacer"))
            .clicked()
        {
            editeur.set_text("");
            st.yaml_console.path = None;
            st.yaml_console.output = "Éditeur vidé.".to_string();
        }

        ui.separator();

        // Namespace appliqué aux documents qui n'en déclarent pas.
        let choisi = st
            .yaml_console
            .namespace
            .clone()
            .unwrap_or_else(|| "(celui du manifeste)".to_string());
        egui::ComboBox::from_id_salt("yaml-console-namespace")
            .selected_text(choisi)
            .width(200.0)
            .show_ui(ui, |ui| {
                let aucun = st.yaml_console.namespace.is_none();
                if ui.selectable_label(aucun, "(celui du manifeste)").clicked() {
                    st.yaml_console.namespace = None;
                }
                for ns in &namespaces {
                    let actif = st.yaml_console.namespace.as_deref() == Some(ns.as_str());
                    if ui.selectable_label(actif, ns.as_str()).clicked() {
                        st.yaml_console.namespace = Some(ns.clone());
                    }
                }
            });
        ui.label(
            RichText::new("namespace par défaut")
                .small()
                .color(palette.muted),
        );

        ui.separator();
        ui.checkbox(&mut st.yaml_console.dry_run, "Dry-run")
            .on_hover_text("« Appliquer » n'écrit rien : le serveur se contente de valider");
        ui.checkbox(&mut st.yaml_console.force, "Force")
            .on_hover_text("Reprendre les champs détenus par un autre gestionnaire");
    });

    if let Some(chemin) = st.yaml_console.path.clone() {
        ui.label(
            RichText::new(format!("Fichier : {}", chemin.display()))
                .small()
                .color(palette.muted),
        );
    }
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn actions(
    ui: &mut egui::Ui,
    st: &mut AppState,
    backend: &Backend,
    texte: &str,
    validation: &Result<usize, String>,
    palette: &Palette,
) {
    let en_vol = st.yaml_console.pending.is_some();
    let cluster = st.current_cluster.clone();
    let pret = validation.is_ok() && !en_vol && cluster.is_some();

    let mut verifier = false;
    let mut essai = false;
    let mut comparer = false;
    let mut appliquer = false;
    let mut supprimer = false;

    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Vérifier")
            .on_hover_text("Validation locale : découpage et comptage des documents")
            .clicked()
        {
            verifier = true;
        }
        if ui
            .add_enabled(pret, egui::Button::new("Dry-run"))
            .on_hover_text("Envoyer au serveur en simulation, sans rien écrire")
            .clicked()
        {
            essai = true;
        }
        if ui
            .add_enabled(pret, egui::Button::new("Diff"))
            .on_hover_text("Comparer le manifeste à l'état actuel du cluster")
            .clicked()
        {
            comparer = true;
        }
        if ui
            .add_enabled(pret, egui::Button::new(RichText::new("Appliquer").strong()))
            .on_hover_text("Appliquer le manifeste au cluster")
            .clicked()
        {
            appliquer = true;
        }
        if ui
            .add_enabled(
                pret,
                egui::Button::new(RichText::new("Supprimer").color(palette.error)),
            )
            .on_hover_text("Supprimer du cluster les ressources décrites par le manifeste")
            .clicked()
        {
            supprimer = true;
        }
        if en_vol {
            ui.spinner();
        }

        ui.separator();
        match validation {
            Ok(documents) => {
                ui.colored_label(
                    palette.ok,
                    RichText::new(format::plural(*documents, "document")).small(),
                );
            }
            Err(message) => {
                ui.add(
                    egui::Label::new(RichText::new(message.as_str()).color(palette.error).small())
                        .truncate(),
                );
            }
        }
        if cluster.is_none() {
            ui.colored_label(
                palette.warn,
                RichText::new("aucun cluster sélectionné").small(),
            );
        }
    });

    if verifier {
        st.yaml_console.output = match validation {
            Ok(documents) => format!(
                "Manifeste valide : {}.",
                format::plural(*documents, "document")
            ),
            Err(message) => message.clone(),
        };
    }

    let Some(cluster) = cluster else {
        return;
    };
    if essai {
        envoyer_apply(st, backend, &cluster, texte, true);
    }
    if appliquer {
        let dry_run = st.yaml_console.dry_run;
        envoyer_apply(st, backend, &cluster, texte, dry_run);
    }
    if comparer {
        st.yaml_console.outcome = None;
        let id = st.begin("Comparaison du manifeste");
        st.yaml_console.pending = Some(id);
        st.yaml_console.output = "Comparaison en cours…".to_string();
        backend.send(Command::DiffYaml {
            id,
            cluster,
            yaml: texte.to_string(),
            namespace: st.yaml_console.namespace.clone(),
        });
    }
    if supprimer {
        *CONFIRMATION.lock() = Some(String::new());
    }
}

/// Envoie une application (réelle ou simulée) et ouvre une entrée d'historique.
fn envoyer_apply(st: &mut AppState, backend: &Backend, cluster: &str, texte: &str, dry_run: bool) {
    st.yaml_console.diff.clear();
    st.yaml_console.outcome = None;

    let libelle = if dry_run {
        "Simulation du manifeste"
    } else {
        "Application du manifeste"
    };
    let id = st.begin(libelle);
    st.yaml_console.pending = Some(id);
    st.yaml_console.output = format!("{libelle}…");

    {
        let mut historique = HISTORIQUE.lock();
        historique.insert(
            0,
            Entree {
                at: chrono::Local::now(),
                yaml: texte.to_string(),
                resume: if dry_run {
                    "simulation en cours…".to_string()
                } else {
                    "application en cours…".to_string()
                },
                en_cours: true,
            },
        );
        historique.truncate(HISTORIQUE_MAX);
    }

    backend.send(Command::ApplyYaml {
        id,
        cluster: cluster.to_string(),
        yaml: texte.to_string(),
        dry_run,
        force: st.yaml_console.force,
        namespace: st.yaml_console.namespace.clone(),
    });
}

/// Envoie la suppression une fois la confirmation obtenue.
fn envoyer_suppression(st: &mut AppState, backend: &Backend, texte: &str) {
    let Some(cluster) = st.current_cluster.clone() else {
        st.yaml_console.output = "Aucun cluster sélectionné.".to_string();
        return;
    };
    st.yaml_console.diff.clear();
    st.yaml_console.outcome = None;

    let id = st.begin("Suppression du manifeste");
    st.yaml_console.pending = Some(id);
    st.yaml_console.output = "Suppression en cours…".to_string();
    backend.send(Command::DeleteYaml {
        id,
        cluster,
        yaml: texte.to_string(),
        namespace: st.yaml_console.namespace.clone(),
    });
}

// ---------------------------------------------------------------------------
// Zone de résultat : compte rendu, bilan, diff, historique
// ---------------------------------------------------------------------------

fn resultats(ui: &mut egui::Ui, st: &mut AppState, editeur: &mut YamlEditor, palette: &Palette) {
    let bilan = st.yaml_console.outcome.clone();
    let differences = st.yaml_console.diff.clone();
    let compte_rendu = st.yaml_console.output.clone();
    let historique = HISTORIQUE.lock().clone();

    let mut rappel: Option<String> = None;
    let mut effacer = false;

    ui.horizontal(|ui| {
        ui.label(RichText::new("Résultats").strong());
        if (bilan.is_some() || !differences.is_empty())
            && ui.small_button("Effacer les résultats").clicked()
        {
            effacer = true;
        }
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("yaml-console-resultats-defilement")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !compte_rendu.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(compte_rendu.as_str())
                            .small()
                            .color(palette.muted),
                    )
                    .wrap(),
                );
                ui.add_space(4.0);
            }

            // --- Bilan d'application --------------------------------------
            if let Some(bilan) = bilan.as_ref() {
                ui.label(RichText::new(resume_bilan(bilan)).strong());
                egui::Grid::new("yaml-console-bilan")
                    .num_columns(3)
                    .spacing([10.0, 3.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for item in &bilan.items {
                            let (libelle, couleur) = action_affichage(palette, item.action);
                            ui.colored_label(couleur, RichText::new(libelle).small().strong());
                            ui.label(
                                RichText::new(format!(
                                    "{} {}",
                                    item.resource.kind,
                                    item.resource.display()
                                ))
                                .monospace()
                                .small(),
                            );
                            let message = item.message.clone().unwrap_or_default();
                            ui.add(
                                egui::Label::new(
                                    RichText::new(message).small().color(palette.muted),
                                )
                                .wrap(),
                            );
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
            }

            // --- Comparaison ------------------------------------------------
            if !differences.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "Diff : {}",
                        format::plural(differences.len(), "ressource")
                    ))
                    .strong(),
                );
                for (reference, diff) in &differences {
                    let titre = format!("{} {}", reference.kind, reference.display());
                    egui::CollapsingHeader::new(titre)
                        .id_salt(format!("diff-{}-{}", reference.kind, reference.display()))
                        .default_open(differences.len() == 1)
                        .show(ui, |ui| {
                            if diff.trim().is_empty() {
                                ui.label(
                                    RichText::new("Aucune différence.")
                                        .small()
                                        .color(palette.muted),
                                );
                                return;
                            }
                            for ligne in diff.lines() {
                                ui.label(
                                    RichText::new(ligne)
                                        .monospace()
                                        .small()
                                        .color(couleur_ligne_diff(palette, ligne)),
                                );
                            }
                        });
                }
                ui.add_space(6.0);
            }

            if bilan.is_none() && differences.is_empty() && compte_rendu.is_empty() {
                ui.label(
                    RichText::new(
                        "Aucun résultat. Utilisez « Vérifier », « Dry-run », « Diff » ou « Appliquer ».",
                    )
                    .small()
                    .color(palette.muted),
                );
                ui.add_space(6.0);
            }

            // --- Historique --------------------------------------------------
            egui::CollapsingHeader::new(format!(
                "Historique des applications ({})",
                historique.len()
            ))
            .id_salt("yaml-console-historique")
            .default_open(false)
            .show(ui, |ui| {
                if historique.is_empty() {
                    ui.label(
                        RichText::new("Aucune application enregistrée.")
                            .small()
                            .color(palette.muted),
                    );
                    return;
                }
                for entree in historique.iter() {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(entree.at.format("%d/%m %H:%M:%S").to_string())
                                .monospace()
                                .small(),
                        );
                        ui.add(
                            egui::Label::new(RichText::new(entree.resume.as_str()).small())
                                .truncate(),
                        );
                        if ui
                            .small_button("Rappeler")
                            .on_hover_text("Recharger ce manifeste dans l'éditeur")
                            .clicked()
                        {
                            rappel = Some(entree.yaml.clone());
                        }
                    });
                }
            });
        });

    if let Some(texte) = rappel {
        editeur.set_text(texte);
        st.yaml_console.path = None;
        st.yaml_console.output = "Manifeste rappelé depuis l'historique.".to_string();
    }
    if effacer {
        st.yaml_console.outcome = None;
        st.yaml_console.diff.clear();
        st.yaml_console.output.clear();
    }
}

/// Complète l'entrée d'historique ouverte lorsque la réponse est arrivée.
fn consigner(st: &mut AppState) {
    if st.yaml_console.pending.is_some() {
        return;
    }
    let Some(bilan) = st.yaml_console.outcome.as_ref() else {
        return;
    };
    let resume = resume_bilan(bilan);
    let mut historique = HISTORIQUE.lock();
    if let Some(entree) = historique.iter_mut().find(|e| e.en_cours) {
        entree.resume = resume;
        entree.en_cours = false;
    }
}

/// Synthèse chiffrée d'un bilan d'application.
fn resume_bilan(bilan: &ApplyOutcome) -> String {
    let mut crees = 0usize;
    let mut configures = 0usize;
    let mut inchanges = 0usize;
    let mut essais = 0usize;
    let mut supprimes = 0usize;
    let mut echecs = 0usize;
    for item in &bilan.items {
        match item.action {
            ApplyAction::Created => crees += 1,
            ApplyAction::Configured => configures += 1,
            ApplyAction::Unchanged => inchanges += 1,
            ApplyAction::DryRun => essais += 1,
            ApplyAction::Deleted => supprimes += 1,
            ApplyAction::Failed => echecs += 1,
        }
    }
    let echecs = echecs.max(bilan.failed);

    let mut parties: Vec<String> = Vec::new();
    if crees > 0 {
        parties.push(format!("{crees} créé(s)"));
    }
    if configures > 0 {
        parties.push(format!("{configures} configuré(s)"));
    }
    if inchanges > 0 {
        parties.push(format!("{inchanges} inchangé(s)"));
    }
    if essais > 0 {
        parties.push(format!("{essais} en essai"));
    }
    if supprimes > 0 {
        parties.push(format!("{supprimes} supprimé(s)"));
    }
    if echecs > 0 {
        parties.push(format!("{echecs} échec(s)"));
    }

    let total = format::plural(bilan.items.len(), "document");
    if parties.is_empty() {
        format!("{total} traité(s)")
    } else {
        format!("{total} : {}", parties.join(", "))
    }
}

/// Libellé et couleur d'une action d'application.
fn action_affichage(palette: &Palette, action: ApplyAction) -> (&'static str, Color32) {
    match action {
        ApplyAction::Created => ("créé", palette.ok),
        ApplyAction::Configured => ("configuré", palette.info),
        ApplyAction::Unchanged => ("inchangé", palette.muted),
        ApplyAction::DryRun => ("essai", palette.warn),
        ApplyAction::Deleted => ("supprimé", palette.accent),
        ApplyAction::Failed => ("échec", palette.error),
    }
}

/// Couleur d'une ligne de diff unifié.
fn couleur_ligne_diff(palette: &Palette, ligne: &str) -> Color32 {
    if ligne.starts_with("+++") || ligne.starts_with("---") {
        palette.muted
    } else if ligne.starts_with('+') {
        palette.ok
    } else if ligne.starts_with('-') {
        palette.error
    } else if ligne.starts_with("@@") {
        palette.info
    } else {
        palette.muted
    }
}

// ---------------------------------------------------------------------------
// Confirmation de suppression
// ---------------------------------------------------------------------------

/// Boîte modale exigeant la saisie du mot « supprimer ».
///
/// Elle est propre à la console : la suppression porte sur un manifeste entier,
/// pas sur un objet identifié, et n'entre donc pas dans le moule des
/// confirmations par ressource.
fn modale_suppression(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend, palette: &Palette) {
    let Some(mut saisie) = CONFIRMATION.lock().clone() else {
        return;
    };
    let ctx = ui.ctx().clone();
    let mut annuler = false;
    let mut confirmer = false;

    let reponse = egui::Modal::new(egui::Id::new("yaml-console-confirmation")).show(&ctx, |ui| {
        ui.set_min_width(420.0);
        ui.heading("Supprimer les ressources du manifeste");
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                "Chaque ressource décrite par le manifeste sera supprimée du cluster. \
                 Cette action est irréversible.",
            )
            .wrap(),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!("Saisissez « {MOT_CONFIRMATION} » pour confirmer."))
                .small()
                .color(palette.muted),
        );
        ui.add(
            egui::TextEdit::singleline(&mut saisie)
                .hint_text(MOT_CONFIRMATION)
                .desired_width(220.0),
        );
        let valide = saisie.trim().eq_ignore_ascii_case(MOT_CONFIRMATION);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Annuler").clicked() {
                annuler = true;
            }
            if ui
                .add_enabled(
                    valide,
                    egui::Button::new(RichText::new("Supprimer").color(palette.error).strong()),
                )
                .clicked()
            {
                confirmer = true;
            }
        });
    });

    if reponse.should_close() {
        annuler = true;
    }
    if annuler || confirmer {
        *CONFIRMATION.lock() = None;
    } else {
        *CONFIRMATION.lock() = Some(saisie);
    }
    if confirmer {
        let texte = st.yaml_console.source.clone();
        envoyer_suppression(st, backend, &texte);
    }
}

// ---------------------------------------------------------------------------
// Fichiers : glisser-déposer, dialogues non bloquants, collage
// ---------------------------------------------------------------------------

/// Charge un fichier .yaml/.yml déposé sur la fenêtre.
fn traiter_fichiers_deposes(ui: &egui::Ui, st: &mut AppState) {
    let fichiers = ui.ctx().input(|i| i.raw.dropped_files.clone());
    for fichier in fichiers {
        let chemin = fichier.path().to_path_buf();
        let extension = chemin
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension != "yaml" && extension != "yml" {
            st.yaml_console.output = format!(
                "Fichier ignoré : {} (extension .yaml ou .yml attendue).",
                chemin.display()
            );
            continue;
        }
        match fichier.bytes() {
            Ok(octets) => match String::from_utf8(octets) {
                Ok(contenu) => {
                    st.yaml_console.source = contenu;
                    st.yaml_console.path = Some(chemin.clone());
                    st.yaml_console.output = format!("Chargé : {}", chemin.display());
                }
                Err(_) => {
                    st.yaml_console.output =
                        format!("{} n'est pas un fichier texte UTF-8.", chemin.display());
                }
            },
            Err(erreur) => {
                st.yaml_console.output =
                    format!("Lecture impossible de {} : {erreur}", chemin.display());
            }
        }
    }
}

/// Bandeau discret pendant le survol d'un fichier au-dessus de la fenêtre.
fn indication_depot(ui: &mut egui::Ui, palette: &Palette) {
    if ui.ctx().input(|i| i.raw.hovered_files.len()) > 0 {
        ui.colored_label(
            palette.accent,
            RichText::new("Déposez un fichier .yaml ou .yml pour le charger").small(),
        );
    }
}

/// Ouvre le sélecteur de fichier sur un fil dédié.
fn ouvrir_dialogue() {
    let mut garde = OUVERTURE.lock();
    if garde.is_some() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    *garde = Some(rx);
    drop(garde);

    std::thread::spawn(move || {
        let choix = rfd::FileDialog::new()
            .set_title("Ouvrir un manifeste")
            .add_filter("Manifestes YAML", &["yaml", "yml"])
            .pick_file();
        // L'échec d'envoi signifie simplement que l'application s'est arrêtée.
        let _ = tx.send(choix);
    });
}

/// Ouvre le sélecteur d'enregistrement sur un fil dédié et écrit le fichier.
fn enregistrer_dialogue(contenu: String, nom: String) {
    let mut garde = ENREGISTREMENT.lock();
    if garde.is_some() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    *garde = Some(rx);
    drop(garde);

    std::thread::spawn(move || {
        let resultat = match rfd::FileDialog::new()
            .set_title("Enregistrer le manifeste")
            .set_file_name(nom)
            .add_filter("Manifestes YAML", &["yaml", "yml"])
            .save_file()
        {
            Some(chemin) => std::fs::write(&chemin, contenu.as_bytes())
                .map(|()| chemin.clone())
                .map_err(|e| format!("Écriture impossible de {} : {e}", chemin.display())),
            // Chaîne vide : dialogue annulé, ce n'est pas une erreur.
            None => Err(String::new()),
        };
        let _ = tx.send(resultat);
    });
}

/// Relève les dialogues terminés, sans jamais attendre.
fn traiter_dialogues(ui: &egui::Ui, st: &mut AppState) {
    let mut en_cours = false;

    // Ouverture.
    {
        let mut garde = OUVERTURE.lock();
        let mut termine = false;
        if let Some(rx) = garde.as_ref() {
            match rx.try_recv() {
                Ok(Some(chemin)) => {
                    termine = true;
                    match std::fs::read_to_string(&chemin) {
                        Ok(contenu) => {
                            st.yaml_console.source = contenu;
                            st.yaml_console.path = Some(chemin.clone());
                            st.yaml_console.output = format!("Chargé : {}", chemin.display());
                        }
                        Err(erreur) => {
                            st.yaml_console.output =
                                format!("Lecture impossible de {} : {erreur}", chemin.display());
                        }
                    }
                }
                Ok(None) => {
                    termine = true;
                    st.yaml_console.output = "Ouverture annulée.".to_string();
                }
                Err(TryRecvError::Empty) => en_cours = true,
                Err(TryRecvError::Disconnected) => termine = true,
            }
        }
        if termine {
            *garde = None;
        }
    }

    // Enregistrement.
    {
        let mut garde = ENREGISTREMENT.lock();
        let mut termine = false;
        if let Some(rx) = garde.as_ref() {
            match rx.try_recv() {
                Ok(Ok(chemin)) => {
                    termine = true;
                    st.yaml_console.path = Some(chemin.clone());
                    st.yaml_console.output = format!("Enregistré : {}", chemin.display());
                }
                Ok(Err(message)) => {
                    termine = true;
                    st.yaml_console.output = if message.is_empty() {
                        "Enregistrement annulé.".to_string()
                    } else {
                        message
                    };
                }
                Err(TryRecvError::Empty) => en_cours = true,
                Err(TryRecvError::Disconnected) => termine = true,
            }
        }
        if termine {
            *garde = None;
        }
    }

    // Tant qu'un dialogue tourne, il faut continuer à dessiner pour le relever.
    if en_cours {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    }
}

/// Récupère un collage clavier quand aucun champ de saisie n'a le focus.
fn traiter_collage(ui: &egui::Ui, st: &mut AppState) {
    if ui.ctx().memory(|m| m.focused()).is_some() {
        // Un champ de saisie a le focus : il gère lui-même le collage.
        return;
    }
    let colle: Option<String> = ui.ctx().input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Paste(texte) => Some(texte.clone()),
            _ => None,
        })
    });
    if let Some(texte) = colle {
        if !texte.trim().is_empty() {
            st.yaml_console.source = texte;
            st.yaml_console.path = None;
            st.yaml_console.output = "Presse-papiers collé dans l'éditeur.".to_string();
        }
    }
}
