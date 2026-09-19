//! Écran « Ressources » : le cœur de l'application.
//!
//! Tableau **virtuel** (seules les lignes visibles sont dessinées) capable
//! d'afficher plusieurs milliers d'objets sans perte de fluidité, colonnes
//! adaptées au kind sélectionné, tri par clic sur l'en-tête, filtre appliqué
//! côté interface, menu contextuel par ligne et raccourcis clavier.
//!
//! Cette vue ne fait **aucun** appel réseau : elle lit `AppState` et envoie des
//! `Command` au worker via `Backend::send`, qui ne bloque jamais.
//!
//! # Hypothèses sur l'état partagé
//! En plus des champs listés dans le contrat, cette vue utilise
//! `st.continue_token: Option<String>` — le jeton de pagination de la dernière
//! page reçue, `None` quand la liste est complète.

use std::cmp::Ordering;
use std::time::Instant;

use egui::{Key, Modifiers, RichText};
use egui_extras::{Column, TableBuilder};
use kubewatch_core::logs::LogOptions;
use kubewatch_core::model::{ListOptions, ObjectSummary, ResourceKind, ResourceRef};

use crate::backend::{Backend, Command};
use crate::icons;
use crate::state::{AppState, View};
use crate::theme;
use crate::widgets::confirm::{Confirm, ConfirmAction};

/// Hauteur d'une ligne du tableau, en points.
const ROW_HEIGHT: f32 = 22.0;
/// Hauteur de la ligne d'en-tête.
const HEADER_HEIGHT: f32 = 24.0;
/// Nombre d'objets demandés au serveur par page.
const PAGE_SIZE: u32 = 500;

/// Préfixe posé sur le libellé d'une requête de pagination.
///
/// Quand le libellé enregistré dans `st.pending` commence par ce marqueur, la
/// page reçue doit être **concaténée** aux lignes déjà affichées et non les
/// remplacer. C'est la seule façon pour la boucle d'évènements de distinguer
/// « première page » de « page suivante ».
pub const APPEND_MARKER: &str = "+";

/// Sections du sélecteur de kind, dans l'ordre d'affichage.
const SECTIONS: [&str; 8] = [
    "Charges de travail",
    "Réseau",
    "Configuration",
    "Stockage",
    "Autorisations",
    "Cluster",
    "Extensions",
    "Autres",
];

// ---------------------------------------------------------------------------
// Colonnes
// ---------------------------------------------------------------------------

/// Contenu d'une colonne du tableau.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    Name,
    Namespace,
    Ready,
    Status,
    Restarts,
    Age,
    Node,
    Images,
    /// Colonne lue dans `ObjectSummary::extra`, par clé.
    Extra(&'static str),
}

/// Description d'une colonne : intitulé, contenu et largeur souhaitée.
struct ColumnSpec {
    title: &'static str,
    cell: Cell,
    width: f32,
    /// Vrai si la colonne absorbe l'espace restant.
    grow: bool,
}

/// Colonne de largeur fixe.
fn col(title: &'static str, cell: Cell, width: f32) -> ColumnSpec {
    ColumnSpec {
        title,
        cell,
        width,
        grow: false,
    }
}

/// Colonne élastique.
fn grow(title: &'static str, cell: Cell, width: f32) -> ColumnSpec {
    ColumnSpec {
        title,
        cell,
        width,
        grow: true,
    }
}

/// Colonnes adaptées au kind sélectionné.
///
/// `with_namespace` n'est vrai que lorsque l'affichage couvre tous les
/// namespaces : afficher la colonne sinon serait du bruit.
fn columns_for(kind: &str, with_namespace: bool) -> Vec<ColumnSpec> {
    // `selected_kind` peut valoir « deployments » ou « deployments.apps ».
    let base = kind.split('.').next().unwrap_or(kind).to_ascii_lowercase();

    let mut cols = vec![grow("Nom", Cell::Name, 200.0)];
    if with_namespace {
        cols.push(col("Namespace", Cell::Namespace, 130.0));
    }

    match base.as_str() {
        "pods" => {
            cols.push(col("Prêt", Cell::Ready, 55.0));
            cols.push(col("Statut", Cell::Status, 115.0));
            cols.push(col("Redém.", Cell::Restarts, 60.0));
            cols.push(col("Âge", Cell::Age, 75.0));
            cols.push(col("Nœud", Cell::Node, 150.0));
            cols.push(grow("Images", Cell::Images, 180.0));
        }
        "deployments" | "statefulsets" | "replicasets" | "daemonsets" => {
            cols.push(col("Prêt", Cell::Ready, 65.0));
            cols.push(col("À jour", Cell::Extra("upToDate"), 65.0));
            cols.push(col("Disponible", Cell::Extra("available"), 80.0));
            cols.push(col("Statut", Cell::Status, 115.0));
            cols.push(col("Âge", Cell::Age, 75.0));
        }
        "services" => {
            cols.push(col("Type", Cell::Extra("type"), 110.0));
            cols.push(col("ClusterIP", Cell::Extra("clusterIP"), 130.0));
            cols.push(grow("Ports", Cell::Extra("ports"), 150.0));
            cols.push(col("Âge", Cell::Age, 75.0));
        }
        "nodes" => {
            cols.push(col("Statut", Cell::Status, 115.0));
            cols.push(col("Rôles", Cell::Extra("roles"), 140.0));
            cols.push(col("Âge", Cell::Age, 75.0));
            cols.push(col("Version", Cell::Extra("version"), 110.0));
        }
        "ingresses" => {
            cols.push(col("Classe", Cell::Extra("class"), 100.0));
            cols.push(grow("Hôtes", Cell::Extra("hosts"), 200.0));
            cols.push(col("Adresse", Cell::Extra("address"), 150.0));
            cols.push(col("Âge", Cell::Age, 75.0));
        }
        "jobs" => {
            cols.push(col("Complétions", Cell::Extra("completions"), 95.0));
            cols.push(col("Durée", Cell::Extra("duration"), 85.0));
            cols.push(col("Statut", Cell::Status, 115.0));
            cols.push(col("Âge", Cell::Age, 75.0));
        }
        _ => {
            cols.push(col("Statut", Cell::Status, 140.0));
            cols.push(col("Âge", Cell::Age, 75.0));
        }
    }
    cols
}

// ---------------------------------------------------------------------------
// Mise en forme des cellules
// ---------------------------------------------------------------------------

/// Met en forme un âge en secondes à la manière de `kubectl` : `45s`, `3h20m`,
/// `12j04h`. Partagé avec l'écran « Vue d'ensemble ».
pub(crate) fn format_age(seconds: Option<i64>) -> String {
    let Some(secs) = seconds else {
        return "—".to_string();
    };
    let secs = secs.max(0);
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{}m{:02}s", minutes, secs % 60);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{}h{:02}m", hours, minutes % 60);
    }
    let days = hours / 24;
    if days < 365 {
        return format!("{}j{:02}h", days, hours % 24);
    }
    format!("{}a{}j", days / 365, days % 365)
}

/// Rend lisible une valeur JSON issue de `ObjectSummary::extra`.
fn extra_text(item: &ObjectSummary, key: &str) -> String {
    use serde_json::Value;
    match item.extra.get(key) {
        None | Some(Value::Null) => "—".to_string(),
        Some(Value::String(s)) if s.is_empty() => "—".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => (if *b { "oui" } else { "non" }).to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Array(items)) => {
            if items.is_empty() {
                "—".to_string()
            } else {
                items
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        }
        Some(other) => other.to_string(),
    }
}

/// Texte affiché dans une cellule.
fn cell_text(item: &ObjectSummary, cell: Cell) -> String {
    match cell {
        Cell::Name => item.name.clone(),
        Cell::Namespace => item.namespace.clone().unwrap_or_else(|| "—".to_string()),
        Cell::Ready => item.ready.clone().unwrap_or_else(|| "—".to_string()),
        Cell::Status => item.status.clone(),
        Cell::Restarts => item
            .restarts
            .map(|r| r.to_string())
            .unwrap_or_else(|| "—".to_string()),
        Cell::Age => format_age(item.age_seconds),
        Cell::Node => item.node.clone().unwrap_or_else(|| "—".to_string()),
        Cell::Images => {
            if item.images.is_empty() {
                "—".to_string()
            } else {
                item.images.join(", ")
            }
        }
        Cell::Extra(key) => extra_text(item, key),
    }
}

/// Proportion de conteneurs prêts, déduite du compteur `prêts/total`.
///
/// Sert uniquement au tri ; renvoie `-1.0` quand l'information manque afin que
/// ces lignes se regroupent en tête du tri croissant.
fn ready_ratio(item: &ObjectSummary) -> f64 {
    let Some(raw) = item.ready.as_deref() else {
        return -1.0;
    };
    let Some((left, right)) = raw.split_once('/') else {
        return -1.0;
    };
    let (Ok(ready), Ok(total)) = (left.trim().parse::<f64>(), right.trim().parse::<f64>()) else {
        return -1.0;
    };
    if total <= 0.0 {
        return -1.0;
    }
    ready / total
}

/// Comparaison de deux lignes sur une colonne donnée.
fn compare(a: &ObjectSummary, b: &ObjectSummary, cell: Cell) -> Ordering {
    match cell {
        // `None` (âge inconnu) est renvoyé en fin de tri croissant.
        Cell::Age => a
            .age_seconds
            .unwrap_or(i64::MAX)
            .cmp(&b.age_seconds.unwrap_or(i64::MAX)),
        Cell::Restarts => a.restarts.unwrap_or(-1).cmp(&b.restarts.unwrap_or(-1)),
        Cell::Ready => ready_ratio(a)
            .partial_cmp(&ready_ratio(b))
            .unwrap_or(Ordering::Equal),
        other => cell_text(a, other)
            .to_lowercase()
            .cmp(&cell_text(b, other).to_lowercase()),
    }
}

/// Filtre insensible à la casse sur le nom, le namespace et le statut.
fn matches_filter(item: &ObjectSummary, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    item.name.to_lowercase().contains(needle)
        || item
            .namespace
            .as_deref()
            .is_some_and(|ns| ns.to_lowercase().contains(needle))
        || item.status.to_lowercase().contains(needle)
}

/// Indices de `st.rows` à afficher, filtrés puis triés.
///
/// Le filtre et le tri sont appliqués ici, côté interface : aucune commande
/// n'est envoyée au serveur pour cela.
fn visible_order(st: &AppState, columns: &[ColumnSpec]) -> Vec<usize> {
    let needle = st.filter.trim().to_lowercase();
    let mut order: Vec<usize> = st
        .rows
        .iter()
        .enumerate()
        .filter(|(_, item)| matches_filter(item, &needle))
        .map(|(index, _)| index)
        .collect();

    let cell = columns.get(st.sort.0).map(|c| c.cell).unwrap_or(Cell::Name);
    let ascending = st.sort.1;

    order.sort_by(|left, right| {
        // Les indices proviennent de `st.rows.iter().enumerate()` : ils sont
        // valides par construction.
        let (a, b) = (&st.rows[*left], &st.rows[*right]);
        let ordering = compare(a, b, cell)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.namespace.cmp(&b.namespace));
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    order
}

// ---------------------------------------------------------------------------
// Références d'objets
// ---------------------------------------------------------------------------

/// Kind actuellement sélectionné, retrouvé dans la liste découverte.
fn current_kind(st: &AppState) -> Option<&ResourceKind> {
    st.kinds
        .iter()
        .find(|k| k.plural == st.selected_kind || k.full_name() == st.selected_kind)
}

/// Construit une référence complète à partir d'une ligne du tableau.
fn make_ref(st: &AppState, item: &ObjectSummary) -> ResourceRef {
    if let Some(kind) = current_kind(st) {
        return ResourceRef::from_kind(kind, item.namespace.as_deref(), &item.name);
    }
    // Repli : la découverte n'a pas encore abouti, on reconstruit depuis l'objet.
    let (group, version) = match item.api_version.split_once('/') {
        Some((g, v)) => (g.to_string(), v.to_string()),
        None => (String::new(), item.api_version.clone()),
    };
    ResourceRef {
        group,
        version,
        kind: item.kind.clone(),
        plural: st.selected_kind.clone(),
        namespace: item.namespace.clone(),
        name: item.name.clone(),
    }
}

/// Vrai si la ligne et la référence désignent le même objet.
fn same_object(item: &ObjectSummary, reference: &ResourceRef) -> bool {
    item.name == reference.name && item.namespace == reference.namespace
}

/// Vrai pour un pod : conditionne les entrées « Journaux » et « Terminal ».
fn is_pod(reference: &ResourceRef) -> bool {
    reference.kind.eq_ignore_ascii_case("Pod")
}

/// Vrai pour un nœud du cluster, seul kind qui accepte cordon et vidange.
fn is_node(reference: &ResourceRef) -> bool {
    reference.kind.eq_ignore_ascii_case("Node")
}

/// Vrai pour les objets qui acceptent un changement de nombre de répliques.
fn is_scalable(reference: &ResourceRef) -> bool {
    matches!(
        reference.kind.as_str(),
        "Deployment" | "StatefulSet" | "ReplicaSet" | "ReplicationController"
    )
}

/// Vrai pour les objets qui acceptent un redémarrage progressif et un rollback.
fn is_rollable(reference: &ResourceRef) -> bool {
    matches!(
        reference.kind.as_str(),
        "Deployment" | "StatefulSet" | "DaemonSet"
    )
}

// ---------------------------------------------------------------------------
// Actions différées
// ---------------------------------------------------------------------------

/// Intention exprimée pendant le rendu, appliquée une fois le tableau dessiné.
///
/// Le rendu ne tient que des emprunts partagés sur `AppState` ; les mutations
/// sont regroupées à la fin de l'image.
enum Action {
    Sort(usize),
    SelectKind(String),
    SelectNamespace(Option<String>),
    Select(ResourceRef),
    Open(ResourceRef),
    ShowYaml(ResourceRef),
    Edit(ResourceRef),
    Logs(ResourceRef),
    Terminal(ResourceRef),
    Restart(ResourceRef),
    Rollback(ResourceRef),
    Delete(ResourceRef),
    AskScale(ResourceRef, i32),
    AskImage(ResourceRef, String),
    Cordon(ResourceRef, bool),
    Drain(ResourceRef),
    ClearFilter,
    Refresh,
    LoadMore,
}

// ---------------------------------------------------------------------------
// Boîtes de dialogue (état conservé dans la mémoire d'egui)
// ---------------------------------------------------------------------------

/// Saisie du nombre de répliques.
#[derive(Clone, Default)]
struct ScaleDialog {
    open: bool,
    reference: ResourceRef,
    text: String,
}

/// Saisie d'une nouvelle image de conteneur.
#[derive(Clone, Default)]
struct ImageDialog {
    open: bool,
    reference: ResourceRef,
    container: String,
    image: String,
}

/// Lit un état transitoire dans la mémoire d'egui.
fn load_temp<T>(ctx: &egui::Context, id: egui::Id) -> T
where
    T: Clone + Default + Send + Sync + 'static,
{
    ctx.data(|d| d.get_temp::<T>(id)).unwrap_or_default()
}

/// Enregistre un état transitoire dans la mémoire d'egui.
fn store_temp<T>(ctx: &egui::Context, id: egui::Id, value: T)
where
    T: Clone + Send + Sync + 'static,
{
    ctx.data_mut(|d| d.insert_temp(id, value));
}

// ---------------------------------------------------------------------------
// Point d'entrée de la vue
// ---------------------------------------------------------------------------

/// Dessine l'écran « Ressources ».
pub fn show(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let ctx = ui.ctx().clone();
    let mut actions: Vec<Action> = Vec::new();

    let scale_id = egui::Id::new("kw_scale_dialog");
    let image_id = egui::Id::new("kw_image_dialog");
    let mut scale: ScaleDialog = load_temp(&ctx, scale_id);
    let mut image: ImageDialog = load_temp(&ctx, image_id);

    // « / » place le focus dans le champ de filtre. Consommé avant le rendu de
    // la barre d'outils pour que la touche ne soit pas insérée dans le champ.
    let focus_filter = wants_filter_focus(&ctx, &scale, &image);

    toolbar(ui, st, focus_filter, &mut actions);

    let with_namespace = st.namespace.is_none();
    let columns = columns_for(&st.selected_kind, with_namespace);
    let order = visible_order(st, &columns);

    counters(ui, st, order.len(), &mut actions);
    ui.separator();

    // Raccourcis clavier : traités avant le tableau pour que la ligne
    // sélectionnée au clavier soit mise en évidence dès cette image.
    let mut scroll_to: Option<usize> = None;
    handle_keys(
        &ctx,
        st,
        &order,
        &scale,
        &image,
        &mut actions,
        &mut scroll_to,
    );

    if st.current_cluster.is_none() {
        empty_no_cluster(ui);
    } else if st.rows.is_empty() {
        empty_no_object(ui, st, &mut actions);
    } else if order.is_empty() {
        empty_filtered(ui, st, &mut actions);
    } else {
        table(ui, st, &columns, &order, scroll_to, &mut actions);
    }

    for action in actions {
        apply(st, backend, action, &mut scale, &mut image);
    }

    scale_modal(&ctx, st, backend, &mut scale);
    image_modal(&ctx, st, backend, &mut image);

    store_temp(&ctx, scale_id, scale);
    store_temp(&ctx, image_id, image);

    auto_refresh(&ctx, st, backend);
}

// ---------------------------------------------------------------------------
// Barre d'outils
// ---------------------------------------------------------------------------

/// Barre d'outils : kind, namespace, filtre, sélecteur de labels, rafraîchir.
fn toolbar(ui: &mut egui::Ui, st: &mut AppState, focus_filter: bool, actions: &mut Vec<Action>) {
    ui.horizontal_wrapped(|ui| {
        kind_selector(ui, st, actions);
        namespace_selector(ui, st, actions);

        ui.label("Filtre :");
        let filter = ui.add(
            egui::TextEdit::singleline(&mut st.filter)
                .id(egui::Id::new("kw_filter_field"))
                .desired_width(170.0)
                .hint_text("nom, namespace, statut"),
        );
        if focus_filter {
            filter.request_focus();
        }
        if !st.filter.is_empty() && ui.small_button(icons::CLOSE).clicked() {
            st.filter.clear();
        }

        ui.label("Labels :");
        let selector = ui.add(
            egui::TextEdit::singleline(&mut st.label_selector)
                .id(egui::Id::new("kw_label_selector_field"))
                .desired_width(180.0)
                .hint_text("app=web,tier!=db"),
        );
        // Le sélecteur de labels est évalué par le serveur : il faut relister.
        let validated = selector.lost_focus() && ui.ctx().input(|i| i.key_pressed(Key::Enter));
        if validated {
            actions.push(Action::Refresh);
        }

        if ui.button("Rafraîchir").clicked() {
            actions.push(Action::Refresh);
        }
    });
}

/// Vrai si le kind correspond à la recherche saisie dans le sélecteur.
fn kind_matches(kind: &ResourceKind, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    kind.kind.to_lowercase().contains(needle)
        || kind.plural.to_lowercase().contains(needle)
        || kind.group.to_lowercase().contains(needle)
        || kind
            .short_names
            .iter()
            .any(|s| s.to_lowercase().contains(needle))
}

/// Section de classement d'un kind dans le sélecteur.
fn kind_section(kind: &ResourceKind) -> &'static str {
    match kind.plural.as_str() {
        "pods"
        | "deployments"
        | "statefulsets"
        | "daemonsets"
        | "replicasets"
        | "jobs"
        | "cronjobs"
        | "replicationcontrollers" => "Charges de travail",
        "services" | "ingresses" | "ingressclasses" | "endpoints" | "endpointslices"
        | "networkpolicies" => "Réseau",
        "configmaps"
        | "secrets"
        | "serviceaccounts"
        | "resourcequotas"
        | "limitranges"
        | "horizontalpodautoscalers"
        | "poddisruptionbudgets" => "Configuration",
        "persistentvolumes"
        | "persistentvolumeclaims"
        | "storageclasses"
        | "volumeattachments"
        | "csidrivers"
        | "csinodes" => "Stockage",
        "roles" | "rolebindings" | "clusterroles" | "clusterrolebindings" => "Autorisations",
        "nodes"
        | "namespaces"
        | "events"
        | "componentstatuses"
        | "apiservices"
        | "customresourcedefinitions" => "Cluster",
        _ => {
            if kind.group.is_empty() {
                "Autres"
            } else {
                "Extensions"
            }
        }
    }
}

/// Sélecteur de kind : liste recherchable, groupée par section.
fn kind_selector(ui: &mut egui::Ui, st: &mut AppState, actions: &mut Vec<Action>) {
    let search_id = egui::Id::new("kw_kind_search");
    let label = current_kind(st)
        .map(|k| k.kind.clone())
        .unwrap_or_else(|| st.selected_kind.clone());
    let mut chosen: Option<String> = None;

    egui::ComboBox::from_id_salt("kw_kind_combo")
        .selected_text(label)
        .width(200.0)
        .height(460.0)
        .show_ui(ui, |ui| {
            let mut needle: String = load_temp(ui.ctx(), search_id);
            let response = ui.add(
                egui::TextEdit::singleline(&mut needle)
                    .desired_width(180.0)
                    .hint_text("rechercher un type…"),
            );
            if response.changed() {
                store_temp(ui.ctx(), search_id, needle.clone());
            }
            ui.separator();

            let low = needle.trim().to_lowercase();
            let mut shown = 0usize;
            for section in SECTIONS {
                let mut header_drawn = false;
                for kind in st.kinds.iter().filter(|k| kind_section(k) == section) {
                    if !kind_matches(kind, &low) {
                        continue;
                    }
                    if !header_drawn {
                        ui.add_space(2.0);
                        ui.label(RichText::new(section).small().weak());
                        header_drawn = true;
                    }
                    let selected = kind.plural == st.selected_kind;
                    let entry = ui
                        .selectable_label(selected, kind.kind.as_str())
                        .on_hover_text(hint_for(kind));
                    if entry.clicked() {
                        chosen = Some(kind.plural.clone());
                        ui.close();
                    }
                    shown += 1;
                }
            }
            if shown == 0 {
                ui.label(RichText::new("Aucun type ne correspond.").weak());
            }
        });

    if let Some(plural) = chosen {
        actions.push(Action::SelectKind(plural));
    }
}

/// Infobulle d'un kind : nom canonique, abréviations, portée.
fn hint_for(kind: &ResourceKind) -> String {
    let mut text = kind.full_name();
    if !kind.short_names.is_empty() {
        text.push_str(&format!("  ({})", kind.short_names.join(", ")));
    }
    text.push_str(if kind.namespaced {
        "\nPortée : namespace"
    } else {
        "\nPortée : cluster"
    });
    text
}

/// Sélecteur de namespace, « Tous » compris.
fn namespace_selector(ui: &mut egui::Ui, st: &mut AppState, actions: &mut Vec<Action>) {
    let label = st
        .namespace
        .clone()
        .unwrap_or_else(|| "Tous les namespaces".to_string());
    let mut chosen: Option<Option<String>> = None;

    egui::ComboBox::from_id_salt("kw_namespace_combo")
        .selected_text(label)
        .width(190.0)
        .height(460.0)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(st.namespace.is_none(), "Tous les namespaces")
                .clicked()
            {
                chosen = Some(None);
                ui.close();
            }
            ui.separator();
            if st.namespaces.is_empty() {
                ui.label(RichText::new("Aucun namespace connu.").weak());
            }
            for namespace in &st.namespaces {
                let selected = st.namespace.as_deref() == Some(namespace.as_str());
                if ui.selectable_label(selected, namespace.as_str()).clicked() {
                    chosen = Some(Some(namespace.clone()));
                    ui.close();
                }
            }
        });

    if let Some(namespace) = chosen {
        actions.push(Action::SelectNamespace(namespace));
    }
}

/// Deuxième ligne : compteurs, pagination, rafraîchissement automatique.
fn counters(ui: &mut egui::Ui, st: &mut AppState, visible: usize, actions: &mut Vec<Action>) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{} / {} objet(s)", visible, st.rows.len()))
                .small()
                .weak(),
        );

        if !st.pending.is_empty() {
            ui.label(
                RichText::new(format!("{} chargement…", icons::REFRESH))
                    .small()
                    .weak(),
            );
        }

        if st.continue_token.is_some() && ui.small_button("Charger la suite").clicked() {
            actions.push(Action::LoadMore);
        }

        ui.checkbox(&mut st.auto_refresh, "Rafraîchissement automatique");
        if st.auto_refresh {
            let seconds = st.refresh_every.as_secs().max(1);
            ui.label(
                RichText::new(format!("toutes les {seconds} s"))
                    .small()
                    .weak(),
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Tableau
// ---------------------------------------------------------------------------

/// Tableau virtuel : seules les lignes visibles sont dessinées.
fn table(
    ui: &mut egui::Ui,
    st: &AppState,
    columns: &[ColumnSpec],
    order: &[usize],
    scroll_to: Option<usize>,
    actions: &mut Vec<Action>,
) {
    let available = ui.available_height().max(ROW_HEIGHT * 3.0);
    // La palette dépend du thème choisi dans les réglages : elle est résolue
    // une fois par image, pas une fois par cellule.
    let palette = theme::palette(st.settings.dark);

    let mut builder = TableBuilder::new(ui)
        .id_salt("kw_resources_table")
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .auto_shrink([false, false])
        .min_scrolled_height(0.0)
        .max_scroll_height(available);

    if let Some(row) = scroll_to {
        builder = builder.scroll_to_row(row, Some(egui::Align::Center));
    }
    for spec in columns {
        let column = if spec.grow {
            Column::remainder().at_least(spec.width).clip(true)
        } else {
            Column::initial(spec.width).at_least(45.0).clip(true)
        };
        builder = builder.column(column);
    }

    let (sort_column, ascending) = st.sort;

    builder
        .header(HEADER_HEIGHT, |mut header| {
            for (index, spec) in columns.iter().enumerate() {
                header.col(|ui| {
                    let arrow = if index == sort_column {
                        if ascending {
                            icons::SORT_ASC
                        } else {
                            icons::SORT_DESC
                        }
                    } else {
                        ""
                    };
                    let text = RichText::new(format!("{} {arrow}", spec.title)).strong();
                    let button = ui.add(egui::Button::new(text).frame(false));
                    if button.clicked() {
                        actions.push(Action::Sort(index));
                    }
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, order.len(), |mut row| {
                let Some(&index) = order.get(row.index()) else {
                    return;
                };
                let Some(item) = st.rows.get(index) else {
                    return;
                };
                let reference = make_ref(st, item);
                row.set_selected(st.selected.as_ref() == Some(&reference));

                for spec in columns {
                    row.col(|ui| cell_ui(ui, &palette, item, spec.cell));
                }

                let response = row.response();
                if response.clicked() {
                    actions.push(Action::Select(reference.clone()));
                }
                if response.double_clicked() {
                    actions.push(Action::Open(reference.clone()));
                }
                response.context_menu(|ui| {
                    // Un clic droit sélectionne aussi la ligne visée.
                    if st.selected.as_ref() != Some(&reference) {
                        actions.push(Action::Select(reference.clone()));
                    }
                    context_menu(ui, &reference, item, actions);
                });
            });
        });
}

/// Contenu d'une cellule.
fn cell_ui(ui: &mut egui::Ui, palette: &theme::Palette, item: &ObjectSummary, cell: Cell) {
    match cell {
        Cell::Name => {
            ui.add(
                egui::Label::new(RichText::new(item.name.as_str()).strong())
                    .truncate()
                    .selectable(false),
            );
        }
        Cell::Status => {
            let color = theme::status_color(palette, item.status.as_str());
            ui.add(
                egui::Label::new(RichText::new(item.status.as_str()).color(color))
                    .truncate()
                    .selectable(false),
            );
        }
        other => {
            ui.add(
                egui::Label::new(cell_text(item, other))
                    .truncate()
                    .selectable(false),
            );
        }
    }
}

/// Menu contextuel d'une ligne.
fn context_menu(
    ui: &mut egui::Ui,
    reference: &ResourceRef,
    item: &ObjectSummary,
    actions: &mut Vec<Action>,
) {
    ui.set_min_width(190.0);
    let danger = ui.visuals().error_fg_color;
    let pod = is_pod(reference);
    let scalable = is_scalable(reference);
    let rollable = is_rollable(reference);

    if ui.button("Voir le YAML").clicked() {
        actions.push(Action::ShowYaml(reference.clone()));
        ui.close();
    }
    if ui.button("Modifier").clicked() {
        actions.push(Action::Edit(reference.clone()));
        ui.close();
    }

    ui.separator();

    if ui.add_enabled(pod, egui::Button::new("Journaux")).clicked() {
        actions.push(Action::Logs(reference.clone()));
        ui.close();
    }
    if ui.add_enabled(pod, egui::Button::new("Terminal")).clicked() {
        actions.push(Action::Terminal(reference.clone()));
        ui.close();
    }

    ui.separator();

    if ui
        .add_enabled(rollable, egui::Button::new("Redémarrer"))
        .clicked()
    {
        actions.push(Action::Restart(reference.clone()));
        ui.close();
    }
    if ui
        .add_enabled(scalable, egui::Button::new("Scaler…"))
        .clicked()
    {
        let current = item
            .extra
            .get("desired")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);
        // Le nombre de répliques tient largement dans un i32 côté API.
        let current = current.clamp(0, i32::MAX as i64) as i32;
        actions.push(Action::AskScale(reference.clone(), current));
        ui.close();
    }
    if ui.button("Changer l'image…").clicked() {
        let current = item.images.first().cloned().unwrap_or_default();
        actions.push(Action::AskImage(reference.clone(), current));
        ui.close();
    }
    if ui
        .add_enabled(rollable, egui::Button::new("Rollback"))
        .clicked()
    {
        actions.push(Action::Rollback(reference.clone()));
        ui.close();
    }

    if is_node(reference) {
        ui.separator();

        // `schedulable` est posé par kubewatch_core::resource::summarize pour les
        // nœuds. En son absence on suppose le nœud ordonnançable : proposer
        // « Autoriser » sur un nœud déjà ouvert est sans effet, l'inverse ne l'est pas.
        let schedulable = item
            .extra
            .get("schedulable")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let label = if schedulable {
            "Interdire l'ordonnancement (cordon)"
        } else {
            "Autoriser l'ordonnancement (uncordon)"
        };
        if ui.button(label).clicked() {
            actions.push(Action::Cordon(reference.clone(), schedulable));
            ui.close();
        }
        if ui
            .button(RichText::new("Vider le nœud (drain)…").color(danger))
            .clicked()
        {
            actions.push(Action::Drain(reference.clone()));
            ui.close();
        }
    }

    ui.separator();

    if ui
        .button(RichText::new("Supprimer").color(danger))
        .clicked()
    {
        actions.push(Action::Delete(reference.clone()));
        ui.close();
    }
}

// ---------------------------------------------------------------------------
// États vides
// ---------------------------------------------------------------------------

/// Aucun cluster sélectionné.
fn empty_no_cluster(ui: &mut egui::Ui) {
    ui.add_space(32.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("Aucun cluster sélectionné.").weak());
        ui.label(
            RichText::new("Choisissez un cluster dans la barre latérale.")
                .small()
                .weak(),
        );
    });
}

/// Libellé lisible du kind courant, au singulier.
fn kind_label(st: &AppState) -> String {
    current_kind(st)
        .map(|k| k.kind.clone())
        .unwrap_or_else(|| st.selected_kind.clone())
}

/// Le serveur n'a renvoyé aucun objet.
fn empty_no_object(ui: &mut egui::Ui, st: &AppState, actions: &mut Vec<Action>) {
    ui.add_space(32.0);
    let kind = kind_label(st);
    let namespace = st.namespace.clone();
    ui.vertical_centered(|ui| {
        let message = match &namespace {
            Some(ns) => format!("Aucun objet « {kind} » dans le namespace {ns}."),
            None => format!("Aucun objet « {kind} » dans ce cluster."),
        };
        ui.label(RichText::new(message).weak());
        ui.add_space(8.0);
        if namespace.is_some() && ui.button("Élargir à tous les namespaces").clicked() {
            actions.push(Action::SelectNamespace(None));
        }
        if !st.label_selector.trim().is_empty() {
            ui.label(
                RichText::new(format!(
                    "Un sélecteur de labels est actif : {}",
                    st.label_selector
                ))
                .small()
                .weak(),
            );
        }
        if ui.button("Rafraîchir").clicked() {
            actions.push(Action::Refresh);
        }
    });
}

/// Des objets existent mais le filtre les masque tous.
fn empty_filtered(ui: &mut egui::Ui, st: &AppState, actions: &mut Vec<Action>) {
    ui.add_space(32.0);
    let filter = st.filter.clone();
    let total = st.rows.len();
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(format!(
                "Aucun des {total} objets ne correspond au filtre « {filter} »."
            ))
            .weak(),
        );
        ui.add_space(8.0);
        if ui.button("Effacer le filtre").clicked() {
            actions.push(Action::ClearFilter);
        }
    });
}

// ---------------------------------------------------------------------------
// Clavier
// ---------------------------------------------------------------------------

/// Vrai si un champ de saisie ou une boîte modale accapare le clavier.
fn keyboard_busy(ctx: &egui::Context, scale: &ScaleDialog, image: &ImageDialog) -> bool {
    scale.open || image.open || ctx.memory(|m| m.focused()).is_some()
}

/// Consomme « / » pour donner le focus au champ de filtre.
fn wants_filter_focus(ctx: &egui::Context, scale: &ScaleDialog, image: &ImageDialog) -> bool {
    if keyboard_busy(ctx, scale, image) {
        return false;
    }
    ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Slash))
}

/// Flèches, Entrée, Suppr et F5.
fn handle_keys(
    ctx: &egui::Context,
    st: &mut AppState,
    order: &[usize],
    scale: &ScaleDialog,
    image: &ImageDialog,
    actions: &mut Vec<Action>,
    scroll_to: &mut Option<usize>,
) {
    // La boîte de confirmation gère elle-même son clavier.
    if st.confirm.is_some() || keyboard_busy(ctx, scale, image) {
        return;
    }

    let (up, down, enter, delete, refresh) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Delete),
            i.consume_key(Modifiers::NONE, Key::F5),
        )
    });

    if refresh {
        actions.push(Action::Refresh);
    }

    let position = st.selected.as_ref().and_then(|selected| {
        order.iter().position(|&index| {
            st.rows
                .get(index)
                .is_some_and(|item| same_object(item, selected))
        })
    });

    if (up || down) && !order.is_empty() {
        let next = match position {
            Some(current) if down => (current + 1).min(order.len() - 1),
            Some(current) => current.saturating_sub(1),
            None => 0,
        };
        if let Some(&index) = order.get(next) {
            let reference = st.rows.get(index).map(|item| make_ref(st, item));
            if let Some(reference) = reference {
                st.selected = Some(reference);
                *scroll_to = Some(next);
            }
        }
    }

    if let Some(selected) = st.selected.clone() {
        if enter {
            actions.push(Action::Open(selected.clone()));
        }
        if delete {
            actions.push(Action::Delete(selected));
        }
    }
}

// ---------------------------------------------------------------------------
// Commandes
// ---------------------------------------------------------------------------

/// Demande une page d'objets. `continue_token` non nul = page suivante.
fn request_list(st: &mut AppState, backend: &Backend, continue_token: Option<String>) {
    let Some(cluster) = st.current_cluster.clone() else {
        return;
    };
    let kind = st.selected_kind.clone();
    let trimmed = st.label_selector.trim();
    let label_selector = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    let appending = continue_token.is_some();
    let opts = ListOptions {
        namespace: st.namespace.clone(),
        label_selector,
        field_selector: None,
        limit: Some(PAGE_SIZE),
        continue_token,
    };

    let id = st.next_id();
    let label = if appending {
        format!("{APPEND_MARKER}Suite de {kind}")
    } else {
        format!("Liste de {kind}")
    };
    st.pending.insert(id, label);
    st.last_refresh = Instant::now();
    backend.send(Command::ListResources {
        id,
        cluster,
        kind,
        opts,
    });
}

/// Demande le YAML d'un objet.
fn request_yaml(st: &mut AppState, backend: &Backend, reference: &ResourceRef) {
    let Some(cluster) = st.current_cluster.clone() else {
        return;
    };
    let id = st.next_id();
    st.pending
        .insert(id, format!("YAML de {}", reference.display()));
    backend.send(Command::GetYaml {
        id,
        cluster,
        reference: reference.clone(),
    });
}

/// Demande la liste des conteneurs d'un pod.
fn request_containers(st: &mut AppState, backend: &Backend, pod: &ResourceRef) {
    let Some(cluster) = st.current_cluster.clone() else {
        return;
    };
    let id = st.next_id();
    st.pending
        .insert(id, format!("Conteneurs de {}", pod.display()));
    backend.send(Command::LoadContainers {
        id,
        cluster,
        pod: pod.clone(),
    });
}

/// Commande de shell par défaut : `bash` s'il existe, `sh` sinon.
fn default_shell() -> Vec<String> {
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "command -v bash >/dev/null 2>&1 && exec bash || exec sh".to_string(),
    ]
}

/// Ouvre la boîte de confirmation. **Toute** action destructive passe par ici :
/// l'utilisateur doit resaisir le nom de l'objet avant que l'intention ne parte.
///
/// La vue ne construit pas la commande : elle décrit l'intention, que la boucle
/// principale traduit en `Command` une fois la saisie validée. C'est ce qui
/// garantit qu'une confirmation abandonnée ne laisse aucune requête en vol.
fn ask_confirm(
    st: &mut AppState,
    title: &str,
    message: String,
    object: &str,
    danger: bool,
    action: ConfirmAction,
) {
    let mut boite = Confirm::new(title, message, action).require_text(object);
    if danger {
        boite = boite.danger();
    }
    st.confirm = Some(boite);
}

/// Applique une intention collectée pendant le rendu.
fn apply(
    st: &mut AppState,
    backend: &Backend,
    action: Action,
    scale: &mut ScaleDialog,
    image: &mut ImageDialog,
) {
    match action {
        Action::Sort(column) => {
            if st.sort.0 == column {
                st.sort.1 = !st.sort.1;
            } else {
                st.sort = (column, true);
            }
        }
        Action::SelectKind(plural) => {
            if plural != st.selected_kind {
                st.selected_kind = plural;
                st.selected = None;
                st.rows.clear();
                st.continue_token = None;
                request_list(st, backend, None);
            }
        }
        Action::SelectNamespace(namespace) => {
            if namespace != st.namespace {
                st.namespace = namespace;
                st.selected = None;
                st.rows.clear();
                st.continue_token = None;
                request_list(st, backend, None);
            }
        }
        Action::Select(reference) => {
            st.selected = Some(reference);
        }
        Action::Open(reference) => {
            st.selected = Some(reference.clone());
            request_yaml(st, backend, &reference);
            if is_pod(&reference) {
                request_containers(st, backend, &reference);
            }
        }
        Action::ShowYaml(reference) => {
            st.selected = Some(reference.clone());
            request_yaml(st, backend, &reference);
        }
        Action::Edit(reference) => {
            st.selected = Some(reference.clone());
            request_yaml(st, backend, &reference);
            st.view = View::Yaml;
        }
        Action::Logs(reference) => {
            st.selected = Some(reference.clone());
            request_containers(st, backend, &reference);
            let Some(cluster) = st.current_cluster.clone() else {
                return;
            };
            let id = st.next_id();
            st.pending
                .insert(id, format!("Journaux de {}", reference.display()));
            backend.send(Command::StartLogs {
                id,
                cluster,
                pod: reference,
                opts: LogOptions {
                    follow: true,
                    tail_lines: Some(500),
                    ..Default::default()
                },
            });
        }
        Action::Terminal(reference) => {
            st.selected = Some(reference.clone());
            request_containers(st, backend, &reference);
            let Some(cluster) = st.current_cluster.clone() else {
                return;
            };
            let id = st.next_id();
            st.pending
                .insert(id, format!("Terminal sur {}", reference.display()));
            backend.send(Command::StartExec {
                id,
                cluster,
                pod: reference,
                container: None,
                command: default_shell(),
            });
        }
        Action::Restart(reference) => {
            if st.current_cluster.is_none() {
                return;
            }
            let message = format!(
                "Redémarrage progressif de {} ({}). Les pods seront recréés un à un.",
                reference.display(),
                reference.kind
            );
            let name = reference.name.clone();
            ask_confirm(
                st,
                "Redémarrer",
                message,
                &name,
                false,
                ConfirmAction::RestartWorkload(reference),
            );
        }
        Action::Rollback(reference) => {
            if st.current_cluster.is_none() {
                return;
            }
            let message = format!(
                "Retour de {} à la révision précédente. L'opération est irréversible.",
                reference.display()
            );
            let name = reference.name.clone();
            ask_confirm(
                st,
                "Rollback",
                message,
                &name,
                false,
                ConfirmAction::RollbackWorkload(reference),
            );
        }
        // `cordon` est réversible et ne déplace aucun pod : pas de confirmation.
        // Le booléen porte la valeur à envoyer — vrai pour interdire
        // l'ordonnancement, ce qui correspond au cas « le nœud est encore ouvert ».
        Action::Cordon(reference, interdire) => {
            let Some(cluster) = st.current_cluster.clone() else {
                return;
            };
            let id = st.next_id();
            let verbe = if interdire { "Cordon" } else { "Uncordon" };
            st.pending
                .insert(id, format!("{verbe} sur {}", reference.name));
            backend.send(Command::Cordon {
                id,
                cluster,
                node: reference.name.clone(),
                on: interdire,
            });
        }
        // `drain` évince tous les pods du nœud : destructif, donc resaisie du nom.
        Action::Drain(reference) => {
            if st.current_cluster.is_none() {
                return;
            }
            let message = format!(
                "Vidange de {} : le nœud est d'abord fermé à l'ordonnancement,                  puis tous ses pods sont évincés. Les charges non répliquées                  seront interrompues.",
                reference.name
            );
            let name = reference.name.clone();
            ask_confirm(
                st,
                "Vider le nœud",
                message,
                &name,
                true,
                ConfirmAction::DrainNode(name.clone()),
            );
        }
        Action::Delete(reference) => {
            if st.current_cluster.is_none() {
                return;
            }
            let message = format!(
                "Suppression définitive de {} ({}) et de ses dépendances.",
                reference.display(),
                reference.kind
            );
            let name = reference.name.clone();
            ask_confirm(
                st,
                "Supprimer",
                message,
                &name,
                true,
                ConfirmAction::DeleteResource(reference),
            );
        }
        Action::AskScale(reference, current) => {
            scale.open = true;
            scale.text = current.to_string();
            scale.reference = reference;
        }
        Action::AskImage(reference, current) => {
            image.open = true;
            image.container.clear();
            image.image = current;
            image.reference = reference;
        }
        Action::ClearFilter => st.filter.clear(),
        Action::Refresh => {
            st.continue_token = None;
            prime(st, backend);
            request_list(st, backend, None);
        }
        Action::LoadMore => {
            let token = st.continue_token.clone();
            if token.is_some() {
                // Le jeton est consommé : la page suivante en fournira un neuf.
                st.continue_token = None;
                request_list(st, backend, token);
            }
        }
    }
}

/// Charge ce qui manque pour que l'écran soit utilisable (kinds, namespaces).
fn prime(st: &mut AppState, backend: &Backend) {
    let Some(cluster) = st.current_cluster.clone() else {
        return;
    };
    if st.kinds.is_empty() {
        let id = st.next_id();
        st.pending.insert(id, "Types de ressources".to_string());
        backend.send(Command::LoadKinds {
            id,
            cluster: cluster.clone(),
        });
    }
    if st.namespaces.is_empty() {
        let id = st.next_id();
        st.pending.insert(id, "Namespaces".to_string());
        backend.send(Command::LoadNamespaces { id, cluster });
    }
}

/// Relance périodiquement le listing quand le rafraîchissement automatique est
/// actif. Le garde-fou `pending.is_empty()` évite d'empiler les requêtes et de
/// doubler un rafraîchissement déclenché ailleurs dans l'application.
fn auto_refresh(ctx: &egui::Context, st: &mut AppState, backend: &Backend) {
    if !st.auto_refresh || st.current_cluster.is_none() {
        return;
    }
    if st.last_refresh.elapsed() >= st.refresh_every {
        if st.pending.is_empty() {
            st.continue_token = None;
            request_list(st, backend, None);
        } else {
            // Une requête est déjà en vol : on retentera à la prochaine échéance.
            st.last_refresh = Instant::now();
        }
    }
    ctx.request_repaint_after(st.refresh_every);
}

// ---------------------------------------------------------------------------
// Boîtes modales
// ---------------------------------------------------------------------------

/// Boîte « Redimensionner ».
fn scale_modal(
    ctx: &egui::Context,
    st: &mut AppState,
    backend: &Backend,
    dialog: &mut ScaleDialog,
) {
    if !dialog.open {
        return;
    }
    let mut validate = false;
    let mut cancel = false;
    let mut parsed: Option<i32> = None;

    let response = egui::Modal::new(egui::Id::new("kw_scale_modal")).show(ctx, |ui| {
        let danger = ui.visuals().error_fg_color;
        ui.set_min_width(360.0);
        ui.heading("Redimensionner");
        ui.label(
            RichText::new(format!(
                "{} · {}",
                dialog.reference.kind,
                dialog.reference.display()
            ))
            .small()
            .weak(),
        );
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.label("Nombre de répliques :");
            let field = ui.add(
                egui::TextEdit::singleline(&mut dialog.text)
                    .desired_width(70.0)
                    .hint_text("0"),
            );
            parsed = dialog.text.trim().parse::<i32>().ok().filter(|n| *n >= 0);
            let submitted = field.lost_focus() && ui.ctx().input(|i| i.key_pressed(Key::Enter));
            if submitted && parsed.is_some() {
                validate = true;
            }
        });

        if parsed.is_none() {
            ui.label(RichText::new("Entier positif ou nul attendu.").color(danger));
        } else if parsed == Some(0) {
            ui.label(RichText::new("Zéro réplique : la charge sera arrêtée.").color(danger));
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(parsed.is_some(), egui::Button::new("Appliquer"))
                .clicked()
            {
                validate = true;
            }
            if ui.button("Annuler").clicked() {
                cancel = true;
            }
        });
    });

    if response.should_close() {
        cancel = true;
    }

    if validate {
        if let (Some(replicas), Some(cluster)) = (parsed, st.current_cluster.clone()) {
            let reference = dialog.reference.clone();
            if replicas == 0 {
                // Arrêter complètement une charge est destructif : on confirme.
                let message = format!(
                    "{} passera à zéro réplique : tous ses pods seront arrêtés.",
                    reference.display()
                );
                let name = reference.name.clone();
                ask_confirm(
                    st,
                    "Arrêter la charge",
                    message,
                    &name,
                    false,
                    ConfirmAction::ScaleWorkload(reference, replicas),
                );
            } else {
                let id = st.next_id();
                st.pending.insert(
                    id,
                    format!("{} → {} réplique(s)", reference.display(), replicas),
                );
                backend.send(Command::Scale {
                    id,
                    cluster,
                    reference,
                    replicas,
                });
            }
        }
        dialog.open = false;
    }
    if cancel {
        dialog.open = false;
    }
}

/// Boîte « Changer l'image ».
fn image_modal(
    ctx: &egui::Context,
    st: &mut AppState,
    backend: &Backend,
    dialog: &mut ImageDialog,
) {
    if !dialog.open {
        return;
    }
    let mut validate = false;
    let mut cancel = false;

    let response = egui::Modal::new(egui::Id::new("kw_image_modal")).show(ctx, |ui| {
        ui.set_min_width(460.0);
        ui.heading("Changer l'image");
        ui.label(
            RichText::new(format!(
                "{} · {}",
                dialog.reference.kind,
                dialog.reference.display()
            ))
            .small()
            .weak(),
        );
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.label("Conteneur :");
            ui.add(
                egui::TextEdit::singleline(&mut dialog.container)
                    .desired_width(160.0)
                    .hint_text("laisser vide = tous"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Image :");
            let field = ui.add(
                egui::TextEdit::singleline(&mut dialog.image)
                    .desired_width(300.0)
                    .hint_text("nginx:1.27-alpine"),
            );
            let submitted = field.lost_focus() && ui.ctx().input(|i| i.key_pressed(Key::Enter));
            if submitted && !dialog.image.trim().is_empty() {
                validate = true;
            }
        });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let ready = !dialog.image.trim().is_empty();
            if ui
                .add_enabled(ready, egui::Button::new("Appliquer"))
                .clicked()
            {
                validate = true;
            }
            if ui.button("Annuler").clicked() {
                cancel = true;
            }
        });
    });

    if response.should_close() {
        cancel = true;
    }

    if validate {
        let new_image = dialog.image.trim().to_string();
        if let Some(cluster) = st.current_cluster.clone() {
            if !new_image.is_empty() {
                let container = {
                    let name = dialog.container.trim();
                    if name.is_empty() {
                        None
                    } else {
                        Some(name.to_string())
                    }
                };
                let reference = dialog.reference.clone();
                let id = st.next_id();
                st.pending
                    .insert(id, format!("{} → {}", reference.display(), new_image));
                backend.send(Command::SetImage {
                    id,
                    cluster,
                    reference,
                    container,
                    image: new_image,
                });
            }
        }
        dialog.open = false;
    }
    if cancel {
        dialog.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pod(name: &str, namespace: &str, status: &str, age: i64) -> ObjectSummary {
        ObjectSummary {
            name: name.to_string(),
            namespace: Some(namespace.to_string()),
            kind: "Pod".to_string(),
            api_version: "v1".to_string(),
            age_seconds: Some(age),
            status: status.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn age_lisible() {
        assert_eq!(format_age(None), "—");
        assert_eq!(format_age(Some(-5)), "0s");
        assert_eq!(format_age(Some(45)), "45s");
        assert_eq!(format_age(Some(3 * 60 + 5)), "3m05s");
        assert_eq!(format_age(Some(3 * 3600 + 20 * 60)), "3h20m");
        assert_eq!(format_age(Some(12 * 86400 + 4 * 3600)), "12j04h");
    }

    #[test]
    fn filtre_insensible_a_la_casse() {
        let item = pod("web-1", "production", "Running", 10);
        assert!(matches_filter(&item, ""));
        assert!(matches_filter(&item, "web"));
        assert!(matches_filter(&item, "prod"));
        assert!(matches_filter(&item, "running"));
        assert!(!matches_filter(&item, "kube-system"));
    }

    #[test]
    fn colonnes_adaptees_au_kind() {
        let pods = columns_for("pods", true);
        assert_eq!(pods[0].title, "Nom");
        assert_eq!(pods[1].title, "Namespace");
        assert!(pods.iter().any(|c| c.cell == Cell::Restarts));

        // Sans la colonne namespace quand un seul namespace est affiché.
        let pods = columns_for("pods", false);
        assert!(!pods.iter().any(|c| c.cell == Cell::Namespace));

        // Le suffixe de groupe ne doit pas empêcher la reconnaissance du kind.
        let deploys = columns_for("deployments.apps", false);
        assert!(deploys.iter().any(|c| c.cell == Cell::Extra("upToDate")));

        let inconnu = columns_for("widgets.example.com", true);
        assert_eq!(inconnu.len(), 4);
    }

    #[test]
    fn tri_par_age_puis_par_nom() {
        let a = pod("b", "default", "Running", 10);
        let b = pod("a", "default", "Running", 100);
        assert_eq!(compare(&a, &b, Cell::Age), Ordering::Less);
        assert_eq!(compare(&a, &b, Cell::Name), Ordering::Greater);
    }

    #[test]
    fn ratio_de_disponibilite() {
        let mut item = pod("web", "default", "Running", 1);
        item.ready = Some("2/3".to_string());
        assert!((ready_ratio(&item) - 2.0 / 3.0).abs() < 1e-9);
        item.ready = Some("cassé".to_string());
        assert_eq!(ready_ratio(&item), -1.0);
        item.ready = Some("1/0".to_string());
        assert_eq!(ready_ratio(&item), -1.0);
        item.ready = None;
        assert_eq!(ready_ratio(&item), -1.0);
    }

    #[test]
    fn valeurs_extra_lisibles() {
        let mut item = pod("web", "default", "Running", 1);
        item.extra
            .insert("upToDate".to_string(), serde_json::json!(3));
        item.extra
            .insert("tls".to_string(), serde_json::json!(true));
        item.extra
            .insert("hosts".to_string(), serde_json::json!(""));
        item.extra.insert(
            "taints".to_string(),
            serde_json::json!(["a=b:NoSchedule", "c=d:NoExecute"]),
        );
        assert_eq!(extra_text(&item, "upToDate"), "3");
        assert_eq!(extra_text(&item, "tls"), "oui");
        assert_eq!(extra_text(&item, "hosts"), "—");
        assert_eq!(extra_text(&item, "absent"), "—");
        assert_eq!(extra_text(&item, "taints"), "a=b:NoSchedule, c=d:NoExecute");
    }
}
