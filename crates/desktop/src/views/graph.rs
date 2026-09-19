//! Écran « Topologie » : le graphe des objets du cluster et de leurs liens.
//!
//! Chaque objet est un nœud, chaque relation connue un lien :
//!
//! | Lien | Source → cible | D'où il vient |
//! | --- | --- | --- |
//! | possède | Deployment → ReplicaSet → Pod, StatefulSet → Pod, DaemonSet → Pod, CronJob → Job → Pod | `metadata.ownerReferences` |
//! | sélectionne | Service → Pod | `spec.selector` confronté aux labels du pod |
//! | route | Ingress → Service | backends des règles et backend par défaut |
//! | planifie | Pod → Nœud | `spec.nodeName` |
//! | monte | Pod → PersistentVolumeClaim | volumes du pod |
//! | utilise | Pod → ConfigMap / Secret | volumes, `envFrom`, `valueFrom` |
//!
//! Cette vue ne fait **aucun** appel réseau : elle lit l'instantané déposé
//! dans `AppState::graph` par la boucle d'évènements et n'envoie qu'une seule
//! commande, `LoadGraph`, qui lit tous les types utiles en parallèle.
//!
//! La disposition est calculée ici, sur le fil d'interface :
//!
//! * **par couches** — une colonne par famille d'objets, ordre des lignes par
//!   barycentre pour limiter les croisements, puis alignement vertical de
//!   chaque nœud sur ses voisins ; déterministe et lisible ;
//! * **organique** — placement par forces (répulsion entre nœuds, attraction
//!   le long des liens), quelques itérations par image jusqu'à stabilisation.
//!
//! Les nœuds déplacés à la main restent épinglés jusqu'à « Réorganiser ». Les
//! positions survivent aux rafraîchissements : seul un objet nouveau déclenche
//! un nouveau placement.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use egui::emath::TSTransform;
use egui::epaint::CubicBezierShape;
use egui::text::{Galley, LayoutJob, TextWrapping};
use egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, RichText, Sense, Shape, Stroke,
    StrokeKind, Vec2, Visuals,
};
use kubewatch_core::model::{ObjectSummary, ResourceRef};
use serde_json::Value;

use crate::backend::{Backend, Command};
use crate::format;
use crate::icons;
use crate::state::{AppState, RequestId, View};
use crate::theme::{self, Palette};

/// Libellé posé sur la requête de lecture de la topologie.
pub const LBL_GRAPH: &str = "Topologie";

/// Largeur d'un nœud, en points de scène.
const NODE_W: f32 = 196.0;
/// Hauteur d'un nœud, en points de scène.
const NODE_H: f32 = 46.0;
/// Taille d'un nœud.
const NODE_SIZE: Vec2 = Vec2::new(NODE_W, NODE_H);
/// Espace horizontal entre deux colonnes.
const COL_GAP: f32 = 120.0;
/// Pas vertical entre deux nœuds d'une même colonne.
const ROW_PITCH: f32 = NODE_H + 16.0;
/// Bornes du zoom de la scène.
const ZOOM_MIN: f32 = 0.08;
const ZOOM_MAX: f32 = 2.5;
/// Nombre d'images pendant lesquelles la disposition organique se stabilise.
const SETTLE_STEPS: u32 = 220;
/// Itérations de forces calculées par image.
const ITER_PER_FRAME: usize = 3;
/// Distance d'équilibre de la disposition organique.
const ORGANIC_K: f32 = 170.0;
/// Au-delà de ce nombre de nœuds, la disposition organique (quadratique) est
/// refusée : la disposition par couches reste la seule proposée.
const ORGANIC_MAX_NODES: usize = 1_200;
/// Transparence appliquée aux nœuds et liens hors du voisinage mis en avant.
const DIM: f32 = 0.32;
/// Sensibilité de la molette : un cran de 50 points change l'échelle d'environ 13 %.
const WHEEL_ZOOM: f32 = 0.0025;
/// En dessous de cette échelle, une carte ne porte plus que son nom.
const ZOOM_NAME_ONLY: f32 = 0.55;
/// En dessous de cette échelle, une carte ne porte plus aucun texte.
const ZOOM_NO_TEXT: f32 = 0.3;
/// Taille minimale du nom en zoom arrière, en points : lisible ou rien.
const MIN_NAME_FONT: f32 = 9.0;

// ---------------------------------------------------------------------------
// Modèle du graphe
// ---------------------------------------------------------------------------

/// Familles d'objets représentées, dans l'ordre des colonnes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeKind {
    Ingress,
    Service,
    Deployment,
    StatefulSet,
    DaemonSet,
    CronJob,
    ReplicaSet,
    Job,
    Pod,
    VolumeClaim,
    ConfigMap,
    Secret,
    Node,
}

impl NodeKind {
    /// Famille correspondant à un kind Kubernetes, `None` s'il n'est pas représenté.
    pub fn from_kind(kind: &str) -> Option<Self> {
        Some(match kind {
            "Ingress" => NodeKind::Ingress,
            "Service" => NodeKind::Service,
            "Deployment" => NodeKind::Deployment,
            "StatefulSet" => NodeKind::StatefulSet,
            "DaemonSet" => NodeKind::DaemonSet,
            "CronJob" => NodeKind::CronJob,
            "ReplicaSet" => NodeKind::ReplicaSet,
            "Job" => NodeKind::Job,
            "Pod" => NodeKind::Pod,
            "PersistentVolumeClaim" => NodeKind::VolumeClaim,
            "ConfigMap" => NodeKind::ConfigMap,
            "Secret" => NodeKind::Secret,
            "Node" => NodeKind::Node,
            _ => return None,
        })
    }

    /// Kind Kubernetes.
    pub fn kind(self) -> &'static str {
        match self {
            NodeKind::Ingress => "Ingress",
            NodeKind::Service => "Service",
            NodeKind::Deployment => "Deployment",
            NodeKind::StatefulSet => "StatefulSet",
            NodeKind::DaemonSet => "DaemonSet",
            NodeKind::CronJob => "CronJob",
            NodeKind::ReplicaSet => "ReplicaSet",
            NodeKind::Job => "Job",
            NodeKind::Pod => "Pod",
            NodeKind::VolumeClaim => "PersistentVolumeClaim",
            NodeKind::ConfigMap => "ConfigMap",
            NodeKind::Secret => "Secret",
            NodeKind::Node => "Node",
        }
    }

    /// Libellé court affiché sur le nœud.
    pub fn label(self) -> &'static str {
        match self {
            NodeKind::VolumeClaim => "PVC",
            NodeKind::Node => "Nœud",
            other => other.kind(),
        }
    }

    /// Nom pluriel de la ressource, pour construire une référence.
    pub fn plural(self) -> &'static str {
        match self {
            NodeKind::Ingress => "ingresses",
            NodeKind::Service => "services",
            NodeKind::Deployment => "deployments",
            NodeKind::StatefulSet => "statefulsets",
            NodeKind::DaemonSet => "daemonsets",
            NodeKind::CronJob => "cronjobs",
            NodeKind::ReplicaSet => "replicasets",
            NodeKind::Job => "jobs",
            NodeKind::Pod => "pods",
            NodeKind::VolumeClaim => "persistentvolumeclaims",
            NodeKind::ConfigMap => "configmaps",
            NodeKind::Secret => "secrets",
            NodeKind::Node => "nodes",
        }
    }

    /// Groupe et version d'API usuels, utilisés quand la découverte n'a pas abouti.
    pub fn group_version(self) -> (&'static str, &'static str) {
        match self {
            NodeKind::Ingress => ("networking.k8s.io", "v1"),
            NodeKind::Deployment
            | NodeKind::StatefulSet
            | NodeKind::DaemonSet
            | NodeKind::ReplicaSet => ("apps", "v1"),
            NodeKind::CronJob | NodeKind::Job => ("batch", "v1"),
            _ => ("", "v1"),
        }
    }

    /// Colonne de la disposition par couches.
    pub fn column(self) -> usize {
        match self {
            NodeKind::Ingress => 0,
            NodeKind::Service => 1,
            NodeKind::Deployment
            | NodeKind::StatefulSet
            | NodeKind::DaemonSet
            | NodeKind::CronJob => 2,
            NodeKind::ReplicaSet | NodeKind::Job => 3,
            NodeKind::Pod => 4,
            NodeKind::VolumeClaim | NodeKind::ConfigMap | NodeKind::Secret => 5,
            NodeKind::Node => 6,
        }
    }
}

/// Couches affichées. Chaque case masque ou montre une famille d'objets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layers {
    /// Ingress.
    pub ingress: bool,
    /// Services.
    pub services: bool,
    /// Deployments, StatefulSets, DaemonSets, CronJobs, Jobs.
    pub workloads: bool,
    /// ReplicaSets : masqués par défaut, le Deployment est relié directement à ses pods.
    pub replicasets: bool,
    /// Pods.
    pub pods: bool,
    /// PersistentVolumeClaims.
    pub storage: bool,
    /// ConfigMaps et Secrets : masqués par défaut, ils encombrent vite.
    pub config: bool,
    /// Nœuds du cluster.
    pub nodes: bool,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            ingress: true,
            services: true,
            workloads: true,
            replicasets: false,
            pods: true,
            storage: true,
            config: false,
            nodes: true,
        }
    }
}

impl Layers {
    /// Vrai si la famille est affichée.
    pub fn shows(&self, kind: NodeKind) -> bool {
        match kind {
            NodeKind::Ingress => self.ingress,
            NodeKind::Service => self.services,
            NodeKind::Deployment
            | NodeKind::StatefulSet
            | NodeKind::DaemonSet
            | NodeKind::CronJob
            | NodeKind::Job => self.workloads,
            NodeKind::ReplicaSet => self.replicasets,
            NodeKind::Pod => self.pods,
            NodeKind::VolumeClaim => self.storage,
            NodeKind::ConfigMap | NodeKind::Secret => self.config,
            NodeKind::Node => self.nodes,
        }
    }
}

/// Algorithme de placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutMode {
    /// Une colonne par famille, lignes ordonnées pour limiter les croisements.
    #[default]
    Layered,
    /// Placement par forces.
    Organic,
}

/// Identité d'un nœud : famille, namespace, nom.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeKey {
    /// Famille.
    pub kind: NodeKind,
    /// Namespace, absent pour les nœuds du cluster.
    pub namespace: Option<String>,
    /// Nom de l'objet.
    pub name: String,
}

impl NodeKey {
    /// Construit une clé.
    pub fn new(kind: NodeKind, namespace: Option<String>, name: impl Into<String>) -> Self {
        Self {
            kind,
            namespace: if kind == NodeKind::Node {
                None
            } else {
                namespace
            },
            name: name.into(),
        }
    }
}

/// Nature d'un lien.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    /// Propriétaire → possédé (`ownerReferences`).
    Owns,
    /// Service → pod sélectionné.
    Selects,
    /// Ingress → service.
    Routes,
    /// Pod → nœud.
    Schedules,
    /// Pod → volume.
    Mounts,
    /// Pod → ConfigMap ou Secret.
    Uses,
}

impl EdgeKind {
    /// Libellé affiché dans la légende et les infobulles.
    pub fn label(self) -> &'static str {
        match self {
            EdgeKind::Owns => "possède",
            EdgeKind::Selects => "sélectionne",
            EdgeKind::Routes => "route vers",
            EdgeKind::Schedules => "planifié sur",
            EdgeKind::Mounts => "monte",
            EdgeKind::Uses => "utilise",
        }
    }
}

/// Un nœud du graphe : l'essentiel de l'objet, suffisant pour le dessiner.
#[derive(Debug, Clone)]
pub struct GraphNode {
    /// Identité.
    pub key: NodeKey,
    /// Statut synthétique.
    pub status: String,
    /// Compteur `prêts/total`.
    pub ready: Option<String>,
    /// Redémarrages cumulés.
    pub restarts: Option<i64>,
    /// Images des conteneurs.
    pub images: Vec<String>,
    /// Nœud hébergeant l'objet.
    pub node: Option<String>,
    /// Âge en secondes.
    pub age_seconds: Option<i64>,
    /// `apiVersion` de l'objet listé, vide pour un nœud fictif.
    pub api_version: String,
    /// Vrai si l'objet a été listé ; faux s'il n'est connu que par référence
    /// (ConfigMap cité par un pod, nœud interdit par les droits...).
    pub known: bool,
}

impl GraphNode {
    /// Nœud construit depuis un objet listé.
    fn from_summary(kind: NodeKind, o: &ObjectSummary) -> Self {
        Self {
            key: NodeKey::new(kind, o.namespace.clone(), o.name.clone()),
            status: o.status.clone(),
            ready: o.ready.clone(),
            restarts: o.restarts,
            images: o.images.clone(),
            node: o.node.clone(),
            age_seconds: o.age_seconds,
            api_version: o.api_version.clone(),
            known: true,
        }
    }

    /// Nœud fictif : référencé, mais non listé.
    fn placeholder(key: NodeKey) -> Self {
        Self {
            key,
            status: String::new(),
            ready: None,
            restarts: None,
            images: Vec::new(),
            node: None,
            age_seconds: None,
            api_version: String::new(),
            known: false,
        }
    }
}

/// Un lien orienté entre deux nœuds, par index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphEdge {
    /// Index du nœud source.
    pub from: usize,
    /// Index du nœud cible.
    pub to: usize,
    /// Nature du lien.
    pub kind: EdgeKind,
}

/// Le graphe : nœuds, liens, index par clé et voisinages.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    /// Nœuds.
    pub nodes: Vec<GraphNode>,
    /// Liens.
    pub edges: Vec<GraphEdge>,
    /// Index d'un nœud par sa clé.
    pub index: HashMap<NodeKey, usize>,
    /// Voisins de chaque nœud, dans les deux sens.
    pub adjacency: Vec<Vec<usize>>,
}

impl Graph {
    /// Ajoute un nœud, ou renvoie l'index du nœud déjà présent.
    fn push(&mut self, node: GraphNode) -> usize {
        if let Some(&i) = self.index.get(&node.key) {
            return i;
        }
        let i = self.nodes.len();
        self.index.insert(node.key.clone(), i);
        self.nodes.push(node);
        i
    }

    /// Index du nœud, créé fictif s'il n'existe pas.
    fn ensure(&mut self, key: &NodeKey) -> usize {
        match self.index.get(key) {
            Some(&i) => i,
            None => self.push(GraphNode::placeholder(key.clone())),
        }
    }

    /// Ajoute un lien, en ignorant les boucles.
    fn link(&mut self, from: usize, to: usize, kind: EdgeKind) {
        if from != to {
            self.edges.push(GraphEdge { from, to, kind });
        }
    }

    /// Retire les nœuds sans aucun lien.
    fn drop_isolated(&mut self) {
        let mut degree = vec![0usize; self.nodes.len()];
        for e in &self.edges {
            degree[e.from] += 1;
            degree[e.to] += 1;
        }
        let mut remap = vec![usize::MAX; self.nodes.len()];
        let mut kept = Vec::with_capacity(self.nodes.len());
        for (i, node) in self.nodes.drain(..).enumerate() {
            if degree[i] > 0 {
                remap[i] = kept.len();
                kept.push(node);
            }
        }
        self.nodes = kept;
        for e in &mut self.edges {
            e.from = remap[e.from];
            e.to = remap[e.to];
        }
        self.index = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.key.clone(), i))
            .collect();
    }

    /// Dédoublonne les liens et calcule les voisinages.
    fn finish(&mut self) {
        let mut seen: HashSet<(usize, usize, EdgeKind)> = HashSet::new();
        self.edges.retain(|e| seen.insert((e.from, e.to, e.kind)));
        self.adjacency = vec![Vec::new(); self.nodes.len()];
        for e in &self.edges {
            self.adjacency[e.from].push(e.to);
            self.adjacency[e.to].push(e.from);
        }
    }

    /// Nœud désigné par une clé.
    pub fn node(&self, key: &NodeKey) -> Option<&GraphNode> {
        self.index.get(key).map(|&i| &self.nodes[i])
    }
}

/// Vrai si tous les couples du sélecteur se retrouvent dans les labels.
///
/// Un sélecteur vide ne sélectionne rien : c'est le comportement d'un service
/// sans sélecteur (headless piloté à la main, `ExternalName`).
pub fn selector_matches(
    selector: &serde_json::Map<String, Value>,
    labels: &std::collections::BTreeMap<String, String>,
) -> bool {
    !selector.is_empty()
        && selector.iter().all(|(k, v)| {
            v.as_str()
                .map(|wanted| labels.get(k).map(String::as_str) == Some(wanted))
                .unwrap_or(false)
        })
}

/// Tableau JSON de chaînes, ou rien.
fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Nombre de répliques voulu par un contrôleur.
fn desired(o: &ObjectSummary) -> i64 {
    o.extra.get("desired").and_then(Value::as_i64).unwrap_or(0)
}

/// Construit le graphe à partir d'un instantané d'objets.
///
/// Les ReplicaSets masqués sont « repliés » : leurs pods sont rattachés au
/// Deployment qui les possède. Un ReplicaSet sans contrôleur reste visible
/// quoi qu'il arrive : c'est alors lui la charge de travail.
pub fn build_graph(objects: &[ObjectSummary], layers: &Layers, hide_isolated: bool) -> Graph {
    let raws: Vec<(NodeKind, &ObjectSummary)> = objects
        .iter()
        .filter_map(|o| NodeKind::from_kind(&o.kind).map(|k| (k, o)))
        .collect();

    let mut by_key: HashMap<NodeKey, usize> = HashMap::with_capacity(raws.len());
    for (i, (k, o)) in raws.iter().enumerate() {
        by_key.insert(NodeKey::new(*k, o.namespace.clone(), o.name.clone()), i);
    }

    // ReplicaSets possédant au moins un pod : les seuls qui méritent un nœud
    // quand la couche est affichée (chaque rollout laisse derrière lui des
    // ReplicaSets vides).
    let mut rs_with_pods: HashSet<usize> = HashSet::new();
    for (k, o) in &raws {
        if *k != NodeKind::Pod {
            continue;
        }
        for owner in &o.owners {
            if owner.kind == "ReplicaSet" {
                let key = NodeKey::new(
                    NodeKind::ReplicaSet,
                    o.namespace.clone(),
                    owner.name.clone(),
                );
                if let Some(&i) = by_key.get(&key) {
                    rs_with_pods.insert(i);
                }
            }
        }
    }

    let visible: Vec<bool> = raws
        .iter()
        .enumerate()
        .map(|(i, (k, o))| {
            if !layers.shows(*k) {
                return *k == NodeKind::ReplicaSet && !o.owners.iter().any(|w| w.controller);
            }
            if *k == NodeKind::ReplicaSet {
                return rs_with_pods.contains(&i) || desired(o) > 0;
            }
            true
        })
        .collect();

    let mut graph = Graph::default();
    let mut node_of: Vec<Option<usize>> = vec![None; raws.len()];
    for (i, (k, o)) in raws.iter().enumerate() {
        if visible[i] {
            node_of[i] = Some(graph.push(GraphNode::from_summary(*k, o)));
        }
    }

    // Propriété : on remonte la chaîne tant que le propriétaire est masqué.
    for (i, (_, o)) in raws.iter().enumerate() {
        let Some(child) = node_of[i] else {
            continue;
        };
        for owner in &o.owners {
            let Some(owner_kind) = NodeKind::from_kind(&owner.kind) else {
                continue;
            };
            // Pods statiques : possédés par leur nœud, le lien de
            // planification dit déjà tout.
            if owner_kind == NodeKind::Node {
                continue;
            }
            let mut cur = by_key
                .get(&NodeKey::new(
                    owner_kind,
                    o.namespace.clone(),
                    owner.name.clone(),
                ))
                .copied();
            let mut depth = 0;
            while let Some(idx) = cur {
                if let Some(parent) = node_of[idx] {
                    graph.link(parent, child, EdgeKind::Owns);
                    break;
                }
                depth += 1;
                if depth > 3 {
                    break;
                }
                let (_, parent_obj) = raws[idx];
                cur = parent_obj
                    .owners
                    .iter()
                    .find(|w| w.controller)
                    .or_else(|| parent_obj.owners.first())
                    .and_then(|w| {
                        NodeKind::from_kind(&w.kind).map(|wk| {
                            NodeKey::new(wk, parent_obj.namespace.clone(), w.name.clone())
                        })
                    })
                    .and_then(|key| by_key.get(&key).copied());
            }
        }
    }

    // Services → pods, par sélecteur.
    if layers.services && layers.pods {
        for (si, (sk, so)) in raws.iter().enumerate() {
            if *sk != NodeKind::Service {
                continue;
            }
            let Some(src) = node_of[si] else {
                continue;
            };
            let Some(selector) = so.extra.get("selector").and_then(Value::as_object) else {
                continue;
            };
            if selector.is_empty() {
                continue;
            }
            for (pi, (pk, po)) in raws.iter().enumerate() {
                if *pk != NodeKind::Pod || po.namespace != so.namespace {
                    continue;
                }
                let Some(dst) = node_of[pi] else {
                    continue;
                };
                if selector_matches(selector, &po.labels) {
                    graph.link(src, dst, EdgeKind::Selects);
                }
            }
        }
    }

    // Ingress → services. Un backend absent devient un nœud fictif : c'est
    // précisément le genre d'erreur que l'on vient voir ici.
    if layers.ingress && layers.services {
        for (i, (k, o)) in raws.iter().enumerate() {
            if *k != NodeKind::Ingress {
                continue;
            }
            let Some(src) = node_of[i] else {
                continue;
            };
            for name in strings(o.extra.get("backends")) {
                let dst = graph.ensure(&NodeKey::new(NodeKind::Service, o.namespace.clone(), name));
                graph.link(src, dst, EdgeKind::Routes);
            }
        }
    }

    // Pod → nœud, volumes, configuration.
    for (i, (k, o)) in raws.iter().enumerate() {
        if *k != NodeKind::Pod {
            continue;
        }
        let Some(src) = node_of[i] else {
            continue;
        };
        if layers.nodes {
            if let Some(name) = o.node.as_deref().filter(|n| !n.is_empty()) {
                let dst = graph.ensure(&NodeKey::new(NodeKind::Node, None, name));
                graph.link(src, dst, EdgeKind::Schedules);
            }
        }
        if layers.storage {
            for claim in strings(o.extra.get("volumeClaims")) {
                let dst = graph.ensure(&NodeKey::new(
                    NodeKind::VolumeClaim,
                    o.namespace.clone(),
                    claim,
                ));
                graph.link(src, dst, EdgeKind::Mounts);
            }
        }
        if layers.config {
            for name in strings(o.extra.get("configMaps")) {
                let dst = graph.ensure(&NodeKey::new(
                    NodeKind::ConfigMap,
                    o.namespace.clone(),
                    name,
                ));
                graph.link(src, dst, EdgeKind::Uses);
            }
            for name in strings(o.extra.get("secrets")) {
                let dst = graph.ensure(&NodeKey::new(NodeKind::Secret, o.namespace.clone(), name));
                graph.link(src, dst, EdgeKind::Uses);
            }
        }
    }

    if hide_isolated {
        graph.drop_isolated();
    }
    graph.finish();
    graph
}

// ---------------------------------------------------------------------------
// Dispositions
// ---------------------------------------------------------------------------

/// Clé de tri initial d'un nœud : namespace, famille, nom.
fn order_key(node: &GraphNode) -> (String, NodeKind, String) {
    (
        node.key.namespace.clone().unwrap_or_default(),
        node.key.kind,
        node.key.name.clone(),
    )
}

/// Disposition par couches.
///
/// 1. Une colonne par famille présente (les colonnes vides sont resserrées).
/// 2. Quatre balayages de barycentre pour limiter les croisements.
/// 3. Six tours d'alignement vertical : chaque nœud vise la hauteur moyenne
///    de ses voisins, sans jamais chevaucher celui du dessus.
///
/// Les nœuds épinglés conservent leur position.
pub fn layered_layout(
    graph: &Graph,
    positions: &mut HashMap<NodeKey, Pos2>,
    pinned: &HashSet<NodeKey>,
) {
    let n = graph.nodes.len();
    if n == 0 {
        return;
    }

    let used: Vec<usize> = graph
        .nodes
        .iter()
        .map(|x| x.key.kind.column())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let col: Vec<usize> = graph
        .nodes
        .iter()
        .map(|x| {
            used.iter()
                .position(|c| *c == x.key.kind.column())
                .unwrap_or(0)
        })
        .collect();

    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); used.len()];
    for (i, c) in col.iter().enumerate() {
        columns[*c].push(i);
    }
    for column in &mut columns {
        column.sort_by(|a, b| order_key(&graph.nodes[*a]).cmp(&order_key(&graph.nodes[*b])));
    }

    let mut rank = vec![0f32; n];
    let refresh = |columns: &[Vec<usize>], rank: &mut [f32]| {
        for column in columns {
            for (r, i) in column.iter().enumerate() {
                rank[*i] = r as f32;
            }
        }
    };
    refresh(&columns, &mut rank);

    // Barycentre : alterner gauche→droite et droite→gauche.
    for sweep in 0..4 {
        let forward = sweep % 2 == 0;
        let order: Vec<usize> = if forward {
            (0..columns.len()).collect()
        } else {
            (0..columns.len()).rev().collect()
        };
        for c in order {
            let mut keyed: Vec<(usize, f32)> = columns[c]
                .iter()
                .map(|&i| {
                    let neighbours: Vec<f32> = graph.adjacency[i]
                        .iter()
                        .filter(|&&j| if forward { col[j] < c } else { col[j] > c })
                        .map(|&j| rank[j])
                        .collect();
                    let key = if neighbours.is_empty() {
                        rank[i]
                    } else {
                        neighbours.iter().sum::<f32>() / neighbours.len() as f32
                    };
                    (i, key)
                })
                .collect();
            keyed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
            columns[c] = keyed.into_iter().map(|(i, _)| i).collect();
            refresh(&columns, &mut rank);
        }
    }

    // Alignement vertical sur les voisins.
    let mut y: Vec<f32> = rank.iter().map(|r| r * ROW_PITCH).collect();
    for _ in 0..6 {
        for column in &mut columns {
            let mut wanted: Vec<(usize, f32)> = column
                .iter()
                .map(|&i| {
                    let neighbours = &graph.adjacency[i];
                    let target = if neighbours.is_empty() {
                        y[i]
                    } else {
                        neighbours.iter().map(|&j| y[j]).sum::<f32>() / neighbours.len() as f32
                    };
                    (i, target)
                })
                .collect();
            wanted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));

            let mut cursor = f32::NEG_INFINITY;
            let mut placed_sum = 0.0;
            let mut wanted_sum = 0.0;
            for (i, target) in &wanted {
                let placed = target.max(cursor);
                y[*i] = placed;
                cursor = placed + ROW_PITCH;
                placed_sum += placed;
                wanted_sum += target;
            }
            // Recentrer la colonne sur ce qu'elle visait : l'empilement ne
            // pousse que vers le bas, on compense.
            let shift = (wanted_sum - placed_sum) / wanted.len().max(1) as f32;
            for (i, _) in &wanted {
                y[*i] += shift;
            }
            *column = wanted.into_iter().map(|(i, _)| i).collect();
        }
    }

    let top = y.iter().copied().fold(f32::INFINITY, f32::min);
    for (i, node) in graph.nodes.iter().enumerate() {
        if pinned.contains(&node.key) && positions.contains_key(&node.key) {
            continue;
        }
        positions.insert(
            node.key.clone(),
            Pos2::new(col[i] as f32 * (NODE_W + COL_GAP), y[i] - top),
        );
    }
}

/// Une itération de la disposition organique (Fruchterman–Reingold).
///
/// Les nœuds sont larges : la distance horizontale est comptée à moitié pour
/// que deux voisins de gauche à droite se repoussent assez.
pub fn organic_step(
    graph: &Graph,
    positions: &mut HashMap<NodeKey, Pos2>,
    pinned: &HashSet<NodeKey>,
    temperature: f32,
) {
    let n = graph.nodes.len();
    if n < 2 {
        return;
    }
    let mut p: Vec<Pos2> = graph
        .nodes
        .iter()
        .map(|x| positions.get(&x.key).copied().unwrap_or(Pos2::ZERO))
        .collect();
    let centers: Vec<Pos2> = p.iter().map(|q| *q + NODE_SIZE / 2.0).collect();
    let mut disp = vec![Vec2::ZERO; n];
    let k = ORGANIC_K;

    for i in 0..n {
        for j in (i + 1)..n {
            let real = centers[i] - centers[j];
            let scaled = Vec2::new(real.x * 0.5, real.y);
            let len = scaled.length().max(1.0);
            if len > 900.0 {
                continue;
            }
            let dir = real / real.length().max(1.0);
            let push = dir * (k * k / len);
            disp[i] += push;
            disp[j] -= push;
        }
    }
    for e in &graph.edges {
        let d = centers[e.from] - centers[e.to];
        let len = d.length().max(1.0);
        let pull = d / len * (len * len / k);
        disp[e.from] -= pull;
        disp[e.to] += pull;
    }
    let centroid = centers.iter().fold(Vec2::ZERO, |acc, c| acc + c.to_vec2()) / n as f32;
    for (i, c) in centers.iter().enumerate() {
        disp[i] -= (c.to_vec2() - centroid) * 0.03;
    }

    for (i, node) in graph.nodes.iter().enumerate() {
        if pinned.contains(&node.key) {
            continue;
        }
        let l = disp[i].length();
        if l > 0.01 {
            p[i] += disp[i] / l * l.min(temperature);
        }
        positions.insert(node.key.clone(), p[i]);
    }
}

/// Rectangle englobant tous les nœuds.
fn bounds(graph: &Graph, positions: &HashMap<NodeKey, Pos2>) -> Rect {
    let mut rect = Rect::NOTHING;
    for node in &graph.nodes {
        if let Some(p) = positions.get(&node.key) {
            rect = rect.union(Rect::from_min_size(*p, NODE_SIZE));
        }
    }
    if rect.is_finite() && rect.is_positive() {
        rect
    } else {
        Rect::from_min_size(Pos2::ZERO, NODE_SIZE)
    }
}

// ---------------------------------------------------------------------------
// État porté par `AppState`
// ---------------------------------------------------------------------------

/// État de l'écran « Topologie ».
///
/// Les préférences (couches, disposition) survivent au changement de cluster ;
/// tout le reste est effacé par [`GraphState::clear`].
#[derive(Debug)]
pub struct GraphState {
    /// Dernier instantané reçu.
    pub objects: Vec<ObjectSummary>,
    /// Types qui n'ont pas pu être lus, avec la cause.
    pub warnings: Vec<String>,
    /// Vrai une fois un instantané reçu.
    pub loaded: bool,
    /// Vrai dès qu'une lecture a été demandée : évite de redemander à chaque
    /// image quand la première tentative a échoué.
    pub attempted: bool,
    /// Lecture en cours.
    pub pending: Option<RequestId>,
    /// Couches affichées.
    pub layers: Layers,
    /// Disposition choisie.
    pub mode: LayoutMode,
    /// Masquer les objets sans aucun lien.
    pub hide_isolated: bool,
    /// Afficher la légende.
    pub show_legend: bool,
    /// Fenêtre de la scène (pan et zoom), en coordonnées de scène.
    pub scene_rect: Rect,
    /// Graphe construit depuis `objects` et `layers`.
    pub graph: Graph,
    /// Position du coin supérieur gauche de chaque nœud.
    pub positions: HashMap<NodeKey, Pos2>,
    /// Nœuds déplacés à la main, que les dispositions ne touchent plus.
    pub pinned: HashSet<NodeKey>,
    /// Nœud sélectionné, dont le voisinage est mis en avant.
    pub focus: Option<NodeKey>,
    /// Vrai si le graphe doit être reconstruit avant le prochain dessin.
    pub dirty: bool,
    /// Vrai si la vue doit cadrer tout le graphe au prochain dessin.
    pub fit_requested: bool,
    /// Images restantes de stabilisation de la disposition organique.
    pub settle: u32,
}

impl Default for GraphState {
    fn default() -> Self {
        Self {
            objects: Vec::new(),
            warnings: Vec::new(),
            loaded: false,
            attempted: false,
            pending: None,
            layers: Layers::default(),
            mode: LayoutMode::default(),
            hide_isolated: false,
            show_legend: false,
            scene_rect: Rect::ZERO,
            graph: Graph::default(),
            positions: HashMap::new(),
            pinned: HashSet::new(),
            focus: None,
            dirty: false,

            fit_requested: false,
            settle: 0,
        }
    }
}

impl GraphState {
    /// Dépose un instantané reçu du cluster.
    pub fn receive(&mut self, objects: Vec<ObjectSummary>, warnings: Vec<String>) {
        self.objects = objects;
        self.warnings = warnings;
        self.loaded = true;
        self.pending = None;
        self.dirty = true;
    }

    /// Oublie tout ce qui dépend du cluster ; les préférences restent.
    pub fn clear(&mut self) {
        *self = Self {
            layers: self.layers,
            mode: self.mode,
            hide_isolated: self.hide_isolated,
            show_legend: self.show_legend,
            ..Self::default()
        };
    }

    /// Reconstruit le graphe et place les nœuds nouveaux.
    fn rebuild(&mut self) {
        self.dirty = false;
        self.graph = build_graph(&self.objects, &self.layers, self.hide_isolated);

        let keys: HashSet<&NodeKey> = self.graph.nodes.iter().map(|n| &n.key).collect();
        self.positions.retain(|k, _| keys.contains(k));
        self.pinned.retain(|k| keys.contains(k));
        let focus_alive = self.focus.as_ref().is_none_or(|f| keys.contains(f));
        if !focus_alive {
            self.focus = None;
        }

        let fresh = self
            .graph
            .nodes
            .iter()
            .any(|n| !self.positions.contains_key(&n.key));
        if !fresh {
            return;
        }
        let first = self.positions.is_empty();
        match self.mode {
            LayoutMode::Layered => {
                layered_layout(&self.graph, &mut self.positions, &self.pinned);
            }
            LayoutMode::Organic => {
                // Les nouveaux venus partent de leur place « par couches »,
                // les autres ne bougent pas : les forces feront le reste.
                let mut seed = HashMap::new();
                layered_layout(&self.graph, &mut seed, &HashSet::new());
                for node in &self.graph.nodes {
                    if !self.positions.contains_key(&node.key) {
                        let p = seed.get(&node.key).copied().unwrap_or(Pos2::ZERO);
                        self.positions.insert(node.key.clone(), p);
                    }
                }
                self.settle = SETTLE_STEPS;
            }
        }
        if first {
            self.fit_requested = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Actions différées
// ---------------------------------------------------------------------------

/// Intention exprimée pendant le rendu, appliquée une fois l'écran dessiné.
enum Action {
    /// Relire la topologie.
    Refresh,
    /// Ouvrir le panneau de détail sur un nœud.
    Open(NodeKey),
    /// Afficher l'objet dans l'écran Ressources.
    Locate(NodeKey),
    /// Centrer la vue sur un nœud, sans changer le zoom.
    Center(NodeKey),
    /// Épingler ou détacher un nœud.
    TogglePin(NodeKey),
    /// Copier un texte dans le presse-papiers.
    Copy(String),
    /// Cadrer tout le graphe.
    Fit,
    /// Recalculer la disposition et détacher tous les nœuds.
    Relayout,
}

// ---------------------------------------------------------------------------
// Point d'entrée
// ---------------------------------------------------------------------------

/// Demande un instantané de la topologie au backend.
///
/// Appelée par la vue à sa première apparition et par la boucle principale à
/// chaque rafraîchissement (`Ctrl+R`, rafraîchissement automatique). Une
/// lecture déjà en vol n'est pas doublée.
pub fn request(st: &mut AppState, backend: &Backend) {
    let Some(cluster) = st.current_cluster.clone() else {
        return;
    };
    if st.graph.pending.is_some() {
        return;
    }
    let id = st.next_id();
    st.pending.insert(id, LBL_GRAPH.to_string());
    st.graph.pending = Some(id);
    st.graph.attempted = true;
    st.last_refresh = Instant::now();
    backend.send(Command::LoadGraph {
        id,
        cluster,
        namespace: st.namespace.clone(),
    });
}

/// Dessine l'écran « Topologie ».
pub fn show(ui: &mut egui::Ui, st: &mut AppState, backend: &Backend) {
    let ctx = ui.ctx().clone();
    let palette = theme::palette(st.settings.dark);
    let mut actions: Vec<Action> = Vec::new();

    if st.current_cluster.is_none() {
        empty_no_cluster(ui, &palette);
        return;
    }
    if !st.graph.attempted && st.graph.pending.is_none() {
        request(st, backend);
    }
    if st.graph.dirty {
        st.graph.rebuild();
    }

    toolbar(ui, st, &palette, &mut actions);
    ui.separator();

    if !st.graph.loaded {
        empty_loading(ui, st, &palette, &mut actions);
    } else if st.graph.graph.nodes.is_empty() {
        empty_no_object(ui, st, &palette, &mut actions);
    } else {
        canvas(ui, st, &palette, &mut actions);
    }

    for action in actions {
        apply(&ctx, st, backend, action);
    }
}

// ---------------------------------------------------------------------------
// Barre d'outils
// ---------------------------------------------------------------------------

/// Barre d'outils : rafraîchir, disposition, cadrage, couches, légende.
fn toolbar(ui: &mut egui::Ui, st: &mut AppState, palette: &Palette, actions: &mut Vec<Action>) {
    let nodes = st.graph.graph.nodes.len();
    let edges = st.graph.graph.edges.len();
    let organic_ok = nodes <= ORGANIC_MAX_NODES;
    let warnings = st.graph.warnings.clone();
    let g = &mut st.graph;

    ui.horizontal_wrapped(|ui| {
        if ui
            .button(format!("{} Rafraîchir", icons::REFRESH))
            .on_hover_text("Ctrl+R")
            .clicked()
        {
            actions.push(Action::Refresh);
        }
        if g.pending.is_some() {
            ui.spinner();
        }
        ui.separator();

        ui.label(RichText::new("Disposition :").small().color(palette.muted));
        let before = g.mode;
        ui.selectable_value(&mut g.mode, LayoutMode::Layered, "Couches")
            .on_hover_text("Une colonne par famille d'objets, de l'Ingress au nœud");
        ui.add_enabled_ui(organic_ok, |ui| {
            ui.selectable_value(&mut g.mode, LayoutMode::Organic, "Organique")
                .on_hover_text(if organic_ok {
                    "Placement par forces : les objets liés se rapprochent".to_string()
                } else {
                    format!("Indisponible au-delà de {ORGANIC_MAX_NODES} objets")
                });
        });
        if g.mode != before {
            actions.push(Action::Relayout);
        }
        ui.separator();

        if ui
            .button(format!("{} Ajuster", icons::FIT))
            .on_hover_text("Cadrer tout le graphe dans la fenêtre")
            .clicked()
        {
            actions.push(Action::Fit);
        }
        if ui
            .button("Réorganiser")
            .on_hover_text("Recalculer la disposition et détacher les nœuds déplacés à la main")
            .clicked()
        {
            actions.push(Action::Relayout);
        }
        ui.separator();

        ui.label(
            RichText::new(format!(
                "{} · {}",
                format::plural(nodes, "objet"),
                format::plural(edges, "lien")
            ))
            .small()
            .color(palette.muted),
        );
        if !warnings.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{} {} non lu(s)",
                    icons::WARNING,
                    format::plural(warnings.len(), "type")
                ))
                .small()
                .color(palette.warn),
            )
            .on_hover_text(warnings.join("\n"));
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Couches :").small().color(palette.muted));
        let mut changed = false;
        let l = &mut g.layers;
        changed |= ui.toggle_value(&mut l.ingress, "Ingress").changed();
        changed |= ui.toggle_value(&mut l.services, "Services").changed();
        changed |= ui
            .toggle_value(&mut l.workloads, "Charges")
            .on_hover_text("Deployments, StatefulSets, DaemonSets, CronJobs, Jobs")
            .changed();
        changed |= ui
            .toggle_value(&mut l.replicasets, "ReplicaSets")
            .on_hover_text("Masqués, les Deployments sont reliés directement à leurs pods")
            .changed();
        changed |= ui.toggle_value(&mut l.pods, "Pods").changed();
        changed |= ui
            .toggle_value(&mut l.storage, "Volumes")
            .on_hover_text("PersistentVolumeClaims montés par les pods")
            .changed();
        changed |= ui
            .toggle_value(&mut l.config, "Config")
            .on_hover_text("ConfigMaps et Secrets référencés par les pods")
            .changed();
        changed |= ui.toggle_value(&mut l.nodes, "Nœuds").changed();
        ui.separator();
        changed |= ui
            .toggle_value(&mut g.hide_isolated, "Masquer les isolés")
            .on_hover_text("Ne garder que les objets reliés à au moins un autre")
            .changed();
        ui.toggle_value(&mut g.show_legend, "Légende");
        if changed {
            g.dirty = true;
        }
    });

    if g.show_legend {
        legend(ui, palette);
    }
}

/// Légende : couleurs de statut et styles de lien.
fn legend(ui: &mut egui::Ui, palette: &Palette) {
    ui.horizontal_wrapped(|ui| {
        let swatch = |ui: &mut egui::Ui, color: Color32, text: &str| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(2), color);
            ui.label(RichText::new(text).small().color(palette.muted));
        };
        swatch(ui, palette.ok, "en marche");
        swatch(ui, palette.warn, "transitoire");
        swatch(ui, palette.error, "en erreur");
        swatch(ui, palette.info, "terminé");
        swatch(ui, palette.muted, "inconnu ou non listé");
        ui.separator();
        let line = |ui: &mut egui::Ui, dash: Option<(f32, f32)>, text: &str| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(28.0, 10.0), Sense::hover());
            let a = rect.left_center();
            let b = rect.right_center();
            let stroke = Stroke::new(1.5, palette.muted);
            match dash {
                None => {
                    ui.painter().line_segment([a, b], stroke);
                }
                Some((d, gap)) => {
                    ui.painter()
                        .extend(Shape::dashed_line(&[a, b], stroke, d, gap));
                }
            }
            ui.label(RichText::new(text).small().color(palette.muted));
        };
        line(ui, None, EdgeKind::Owns.label());
        line(
            ui,
            Some((6.0, 3.0)),
            &format!(
                "{} · {}",
                EdgeKind::Selects.label(),
                EdgeKind::Routes.label()
            ),
        );
        line(
            ui,
            Some((2.0, 3.0)),
            &format!(
                "{} · {} · {}",
                EdgeKind::Schedules.label(),
                EdgeKind::Mounts.label(),
                EdgeKind::Uses.label()
            ),
        );
        ui.separator();
        ui.label(
            RichText::new(
                "clic : détail · glisser : déplacer · molette : zoom · fond : déplacer la vue",
            )
            .small()
            .color(palette.muted),
        );
    });
}

// ---------------------------------------------------------------------------
// États vides
// ---------------------------------------------------------------------------

/// Aucun cluster sélectionné.
fn empty_no_cluster(ui: &mut egui::Ui, palette: &Palette) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("Aucun cluster sélectionné").heading());
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Choisissez un cluster dans la barre supérieure, ou ajoutez-en un dans Réglages.",
            )
            .color(palette.muted),
        );
    });
}

/// Aucun instantané encore reçu.
fn empty_loading(ui: &mut egui::Ui, st: &AppState, palette: &Palette, actions: &mut Vec<Action>) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        if st.graph.pending.is_some() {
            ui.spinner();
            ui.add_space(6.0);
            ui.label(RichText::new("Lecture de la topologie…").color(palette.muted));
        } else {
            ui.label(RichText::new("La topologie n'a pas encore été lue").heading());
            ui.add_space(6.0);
            if ui.button("Charger la topologie").clicked() {
                actions.push(Action::Refresh);
            }
        }
    });
}

/// Instantané reçu, mais rien à dessiner.
fn empty_no_object(ui: &mut egui::Ui, st: &AppState, palette: &Palette, actions: &mut Vec<Action>) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        let scope = st
            .namespace
            .as_deref()
            .map(|ns| format!("dans le namespace « {ns} »"))
            .unwrap_or_else(|| "dans ce cluster".to_string());
        ui.label(RichText::new(format!("Aucun objet à représenter {scope}")).heading());
        ui.add_space(6.0);
        if st.graph.objects.is_empty() {
            ui.label(
                RichText::new("Aucun pod, service ni charge de travail n'a été listé.")
                    .color(palette.muted),
            );
        } else {
            ui.label(
                RichText::new(format!(
                    "{} lu(s), mais aucun ne correspond aux couches affichées.",
                    format::plural(st.graph.objects.len(), "objet")
                ))
                .color(palette.muted),
            );
        }
        ui.add_space(6.0);
        if ui.button("Relire").clicked() {
            actions.push(Action::Refresh);
        }
    });
}

// ---------------------------------------------------------------------------
// La scène
// ---------------------------------------------------------------------------

/// Apparence d'un nœud pour cette image.
#[derive(Clone, Copy)]
struct Look {
    focused: bool,
    hovered: bool,
    dim: bool,
}

/// Vrai si le nœud répond au filtre de la barre supérieure.
fn node_matches(node: &GraphNode, needle: &str) -> bool {
    let hit = |s: &str| s.to_ascii_lowercase().contains(needle);
    hit(&node.key.name)
        || node.key.namespace.as_deref().is_some_and(hit)
        || hit(&node.status)
        || hit(node.key.kind.label())
        || hit(node.key.kind.kind())
}

/// Transformation scène → écran qui cadre `scene_rect` dans `outer` : l'échelle
/// qui fait tenir le rectangle, bornée, puis un centrage. Même règle que
/// `egui::Scene`, dont on reprend la fenêtre sans reprendre le rendu.
fn fit_transform(outer: Rect, scene_rect: Rect) -> TSTransform {
    let scale = (outer.size() / scene_rect.size())
        .min_elem()
        .clamp(ZOOM_MIN, ZOOM_MAX);
    TSTransform::from_translation(outer.center().to_vec2() - scale * scene_rect.center().to_vec2())
        * TSTransform::from_scaling(scale)
}

/// Taille de police pour une taille de base et un zoom, arrondie au demi-point :
/// l'atlas de glyphes ne se remplit pas d'une taille nouvelle à chaque image.
fn font_size(base: f32, zoom: f32) -> f32 {
    ((base * zoom) * 2.0).round().max(2.0) / 2.0
}

/// Met en page un texte sur une ligne, tronqué avec « … » s'il dépasse la largeur.
fn truncated(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = TextWrapping::truncate_at_width(max_width.max(1.0));
    painter.layout_job(job)
}

/// Dessine le graphe et traite les interactions.
///
/// Tout est dessiné en coordonnées **écran** : cartes, liens et textes sont
/// projetés par la transformation courante et rendus à leur taille finale. C'est
/// ce qui garde le texte net à tous les zooms — une couche mise à l'échelle par
/// egui agrandirait des glyphes déjà rastérisés à la taille de base.
fn canvas(ui: &mut egui::Ui, st: &mut AppState, palette: &Palette, actions: &mut Vec<Action>) {
    let filter = st.filter.trim().to_ascii_lowercase();
    let visuals = ui.visuals().clone();
    let outer = ui.available_rect_before_wrap();
    if !outer.is_positive() {
        return;
    }
    let g = &mut st.graph;

    if g.mode == LayoutMode::Organic && g.settle > 0 {
        let temperature = 4.0 + 60.0 * (g.settle as f32 / SETTLE_STEPS as f32);
        for _ in 0..ITER_PER_FRAME {
            organic_step(&g.graph, &mut g.positions, &g.pinned, temperature);
        }
        g.settle -= 1;
        ui.ctx().request_repaint();
    }

    if g.fit_requested || !(g.scene_rect.is_finite() && g.scene_rect.is_positive()) {
        g.fit_requested = false;
        // Cadrer sans jamais dépasser l'échelle 1:1 : un petit graphe agrandi
        // ne serait que du texte flou.
        let fit = bounds(&g.graph, &g.positions).expand(80.0);
        g.scene_rect = Rect::from_center_size(fit.center(), fit.size().max(outer.size()));
    }

    // --- Fond : glisser déplace la vue, la molette (ou le pincement) zoome
    // autour du pointeur.
    let background = ui.allocate_rect(outer, Sense::click_and_drag());
    let mut to_screen = fit_transform(outer, g.scene_rect);
    let mut moved = false;
    if background.dragged() {
        to_screen.translation += background.drag_delta();
        moved = true;
    }
    if background.contains_pointer() {
        if let Some(pointer) = ui.input(|i| i.pointer.latest_pos()) {
            let wheel = ui.input(|i| i.smooth_scroll_delta().y);
            let pinch = ui.input(|i| i.zoom_delta());
            let factor = pinch * (wheel * WHEEL_ZOOM).exp();
            let target = (to_screen.scaling * factor).clamp(ZOOM_MIN, ZOOM_MAX);
            let applied = target / to_screen.scaling;
            if (applied - 1.0).abs() > 1e-4 {
                let at = pointer.to_vec2();
                to_screen = TSTransform::from_translation(at)
                    * TSTransform::from_scaling(applied)
                    * TSTransform::from_translation(-at)
                    * to_screen;
                moved = true;
            }
        }
    }
    if moved {
        g.scene_rect = to_screen.inverse() * outer;
    }
    let zoom = to_screen.scaling;
    let mode = g.mode;

    let GraphState {
        graph,
        positions,
        pinned,
        focus,
        ..
    } = g;
    let n = graph.nodes.len();

    // --- Interactions : avant tout dessin, pour connaître le survol. Les
    // cartes sont enregistrées après le fond, donc au-dessus de lui.
    let mut hovered: Option<usize> = None;
    for (i, node) in graph.nodes.iter().enumerate() {
        let pos = positions.get(&node.key).copied().unwrap_or(Pos2::ZERO);
        let hit = (to_screen * Rect::from_min_size(pos, NODE_SIZE)).intersect(outer);
        if !hit.is_positive() {
            continue;
        }
        let resp = ui.interact(hit, ui.id().with(&node.key), Sense::click_and_drag());

        if resp.dragged() {
            let delta = resp.drag_delta();
            if delta != Vec2::ZERO {
                positions.insert(node.key.clone(), pos + delta / zoom);
                pinned.insert(node.key.clone());
            }
        }
        if resp.hovered() || resp.dragged() {
            hovered = Some(i);
        }
        if resp.clicked() || resp.double_clicked() {
            *focus = Some(node.key.clone());
            actions.push(Action::Open(node.key.clone()));
        }

        let key = node.key.clone();
        let is_pinned = pinned.contains(&key);
        resp.context_menu(|ui| {
            ui.set_min_width(180.0);
            if ui
                .button(format!("{} Ouvrir le détail", icons::INSPECT))
                .clicked()
            {
                actions.push(Action::Open(key.clone()));
                ui.close();
            }
            if ui
                .button(format!("{} Voir dans Ressources", icons::RESOURCES))
                .clicked()
            {
                actions.push(Action::Locate(key.clone()));
                ui.close();
            }
            if ui
                .button(format!("{} Centrer la vue ici", icons::CENTER))
                .clicked()
            {
                actions.push(Action::Center(key.clone()));
                ui.close();
            }
            if ui
                .button(format!(
                    "{} {}",
                    icons::PIN,
                    if is_pinned { "Détacher" } else { "Épingler" }
                ))
                .clicked()
            {
                actions.push(Action::TogglePin(key.clone()));
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{} Copier le nom", icons::COPY))
                .clicked()
            {
                actions.push(Action::Copy(key.name.clone()));
                ui.close();
            }
        });
        resp.on_hover_ui(|ui| tooltip(ui, node, palette));
    }

    // --- Mise en avant : nœud survolé, sinon nœud sélectionné.
    let active: Option<usize> =
        hovered.or_else(|| focus.as_ref().and_then(|k| graph.index.get(k).copied()));
    let neighbourhood: Option<Vec<bool>> = active.map(|a| {
        let mut keep = vec![false; n];
        keep[a] = true;
        for &j in &graph.adjacency[a] {
            keep[j] = true;
        }
        keep
    });
    let matched: Option<Vec<bool>> = (!filter.is_empty()).then(|| {
        let direct: Vec<bool> = graph
            .nodes
            .iter()
            .map(|x| node_matches(x, &filter))
            .collect();
        (0..n)
            .map(|i| direct[i] || graph.adjacency[i].iter().any(|&j| direct[j]))
            .collect()
    });
    let dimmed = |i: usize| -> bool {
        neighbourhood.as_ref().is_some_and(|k| !k[i]) || matched.as_ref().is_some_and(|m| !m[i])
    };

    let rects: Vec<Rect> = graph
        .nodes
        .iter()
        .map(|x| {
            to_screen
                * Rect::from_min_size(
                    positions.get(&x.key).copied().unwrap_or(Pos2::ZERO),
                    NODE_SIZE,
                )
        })
        .collect();

    // --- Liens, sous les cartes. Ceux hors de l'écran ne sont pas calculés.
    let painter = ui.painter().with_clip_rect(outer);
    let base = palette.muted.gamma_multiply(0.6);
    for e in &graph.edges {
        if !rects[e.from].union(rects[e.to]).intersects(outer) {
            continue;
        }
        let touched = active.is_some_and(|a| e.from == a || e.to == a);
        let (color, width) = if touched {
            (palette.accent, 2.2)
        } else if dimmed(e.from) || dimmed(e.to) {
            (base.gamma_multiply(DIM), 1.0)
        } else {
            (base, 1.3)
        };
        let stroke = Stroke::new((width * zoom).max(0.8), color);
        draw_edge(
            &painter,
            rects[e.from],
            rects[e.to],
            e.kind,
            mode,
            zoom,
            stroke,
        );
    }

    // --- Cartes.
    for (i, node) in graph.nodes.iter().enumerate() {
        if !rects[i].intersects(outer) {
            continue;
        }
        let look = Look {
            focused: focus.as_ref() == Some(&node.key),
            hovered: hovered == Some(i),
            dim: dimmed(i),
        };
        draw_node(&painter, rects[i], node, look, palette, &visuals, zoom);
    }

    if background.clicked() {
        st.graph.focus = None;
    }
}

/// Trace un lien entre deux cartes, en coordonnées écran.
fn draw_edge(
    painter: &Painter,
    from: Rect,
    to: Rect,
    kind: EdgeKind,
    mode: LayoutMode,
    zoom: f32,
    stroke: Stroke,
) {
    let points: Vec<Pos2> = match mode {
        LayoutMode::Layered => {
            let rightwards = to.center().x >= from.center().x;
            let (a, b) = if rightwards {
                (from.right_center(), to.left_center())
            } else {
                (from.left_center(), to.right_center())
            };
            let sign = if rightwards { 1.0 } else { -1.0 };
            let dx = ((b.x - a.x).abs() * 0.5).max(30.0 * zoom);
            let c1 = Pos2::new(a.x + sign * dx, a.y);
            let c2 = Pos2::new(b.x - sign * dx, b.y);
            CubicBezierShape::from_points_stroke(
                [a, c1, c2, b],
                false,
                Color32::TRANSPARENT,
                Stroke::NONE,
            )
            .flatten(Some(0.5))
        }
        LayoutMode::Organic => vec![from.center(), to.center()],
    };
    match kind {
        EdgeKind::Owns => {
            painter.add(Shape::line(points, stroke));
        }
        EdgeKind::Selects | EdgeKind::Routes => {
            painter.extend(Shape::dashed_line(
                &points,
                stroke,
                (7.0 * zoom).max(3.0),
                (4.0 * zoom).max(2.0),
            ));
        }
        EdgeKind::Schedules | EdgeKind::Mounts | EdgeKind::Uses => {
            painter.extend(Shape::dashed_line(
                &points,
                stroke,
                (2.0 * zoom).max(1.5),
                (4.0 * zoom).max(2.0),
            ));
        }
    }
}

/// Dessine une carte en coordonnées écran : cadre, bande de statut, puis, selon
/// l'échelle, tout le texte, le nom seul, ou rien.
fn draw_node(
    painter: &Painter,
    rect: Rect,
    node: &GraphNode,
    look: Look,
    palette: &Palette,
    visuals: &Visuals,
    zoom: f32,
) {
    let alpha = if look.dim { DIM } else { 1.0 };
    let fill = if look.focused {
        visuals.selection.bg_fill
    } else if look.hovered {
        visuals.widgets.hovered.bg_fill
    } else {
        visuals.widgets.inactive.bg_fill
    };
    let mut stroke = if look.focused {
        Stroke::new(2.0, palette.accent)
    } else if look.hovered {
        visuals.widgets.hovered.bg_stroke
    } else {
        visuals.widgets.noninteractive.bg_stroke
    };
    stroke.color = stroke.color.gamma_multiply(alpha);
    stroke.width = (stroke.width * zoom).max(1.0);
    let radius = (6.0 * zoom).round().clamp(1.0, 24.0) as u8;
    painter.rect(
        rect,
        CornerRadius::same(radius),
        fill.gamma_multiply(alpha),
        stroke,
        StrokeKind::Inside,
    );

    let status_color = if node.known {
        theme::status_color(palette, &node.status)
    } else {
        palette.muted
    }
    .gamma_multiply(alpha);
    let inset = zoom.max(1.0);
    let stripe = Rect::from_min_max(
        rect.min + Vec2::splat(inset),
        Pos2::new(
            rect.min.x + inset + (4.0 * zoom).max(2.0),
            rect.max.y - inset,
        ),
    );
    painter.rect_filled(
        stripe,
        CornerRadius {
            nw: radius.saturating_sub(1),
            sw: radius.saturating_sub(1),
            ne: 0,
            se: 0,
        },
        status_color,
    );

    if zoom < ZOOM_NO_TEXT {
        return;
    }

    let text_color = visuals.text_color().gamma_multiply(alpha);
    let muted = palette.muted.gamma_multiply(alpha);
    let left = rect.min.x + 12.0 * zoom;
    let right = rect.max.x - 8.0 * zoom;
    let pad_y = 6.0 * zoom;

    if zoom < ZOOM_NAME_ONLY {
        // Zoom arrière : le nom seul, à une taille qui reste lisible.
        let font = FontId::proportional(font_size(13.0, zoom).max(MIN_NAME_FONT));
        let galley = truncated(painter, &node.key.name, font, text_color, right - left);
        let at = Align2::LEFT_CENTER.anchor_size(Pos2::new(left, rect.center().y), galley.size());
        painter.galley(at.min, galley, text_color);
        return;
    }

    let small = FontId::proportional(font_size(10.5, zoom));
    let body = FontId::proportional(font_size(13.0, zoom));
    let gap = 6.0 * zoom;

    // Ligne du haut : compteur à droite, famille et namespace à gauche.
    let mut family_right = right;
    if let Some(ready) = &node.ready {
        let galley = truncated(painter, ready, small.clone(), muted, (right - left) * 0.4);
        let at = Align2::RIGHT_TOP.anchor_size(Pos2::new(right, rect.min.y + pad_y), galley.size());
        painter.galley(at.min, galley, muted);
        family_right = at.min.x - gap;
    }
    let family = match &node.key.namespace {
        Some(ns) => format!("{} · {}", node.key.kind.label(), ns),
        None => node.key.kind.label().to_string(),
    };
    let galley = truncated(painter, &family, small.clone(), muted, family_right - left);
    let at = Align2::LEFT_TOP.anchor_size(Pos2::new(left, rect.min.y + pad_y), galley.size());
    painter.galley(at.min, galley, muted);

    // Ligne du bas : statut à droite, nom à gauche, tronqué avant le statut.
    let status_text = if node.known {
        node.status.as_str()
    } else {
        "non listé"
    };
    let galley = truncated(
        painter,
        status_text,
        small,
        status_color,
        (right - left) * 0.45,
    );
    let at = Align2::RIGHT_BOTTOM.anchor_size(Pos2::new(right, rect.max.y - pad_y), galley.size());
    painter.galley(at.min, galley, status_color);
    let name_right = at.min.x - gap;

    let galley = truncated(painter, &node.key.name, body, text_color, name_right - left);
    let at = Align2::LEFT_BOTTOM.anchor_size(Pos2::new(left, rect.max.y - pad_y), galley.size());
    painter.galley(at.min, galley, text_color);
}

/// Infobulle d'un nœud.
fn tooltip(ui: &mut egui::Ui, node: &GraphNode, palette: &Palette) {
    ui.set_max_width(380.0);
    ui.label(RichText::new(node.key.name.as_str()).strong());
    let scope = match &node.key.namespace {
        Some(ns) => format!("{} · namespace {ns}", node.key.kind.kind()),
        None => format!("{} · portée cluster", node.key.kind.kind()),
    };
    ui.label(RichText::new(scope).small().color(palette.muted));

    if !node.known {
        ui.add_space(4.0);
        ui.colored_label(
            palette.warn,
            "Référencé par un autre objet, mais non listé : couche masquée, objet absent ou droits insuffisants.",
        );
        return;
    }

    ui.add_space(4.0);
    egui::Grid::new(("kw_graph_tip", &node.key))
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, label: &str, value: String, color: Option<Color32>| {
                ui.label(RichText::new(label).small().color(palette.muted));
                match color {
                    Some(c) => ui.label(RichText::new(value).small().color(c)),
                    None => ui.label(RichText::new(value).small()),
                };
                ui.end_row();
            };
            row(
                ui,
                "Statut",
                node.status.clone(),
                Some(theme::status_color(palette, &node.status)),
            );
            if let Some(ready) = &node.ready {
                row(ui, "Prêt", ready.clone(), None);
            }
            if let Some(restarts) = node.restarts.filter(|r| *r > 0) {
                row(ui, "Redémarrages", restarts.to_string(), Some(palette.warn));
            }
            if let Some(n) = &node.node {
                row(ui, "Nœud", n.clone(), None);
            }
            if let Some(age) = node.age_seconds {
                row(ui, "Âge", format::age(age), None);
            }
            for (i, image) in node.images.iter().take(3).enumerate() {
                row(
                    ui,
                    if i == 0 { "Images" } else { "" },
                    format::truncate(image, 60),
                    None,
                );
            }
            if node.images.len() > 3 {
                row(
                    ui,
                    "",
                    format!("… et {} autre(s)", node.images.len() - 3),
                    None,
                );
            }
        });
    ui.add_space(4.0);
    ui.label(
        RichText::new("clic : détail · glisser : déplacer · clic droit : menu")
            .small()
            .weak(),
    );
}

// ---------------------------------------------------------------------------
// Application des actions
// ---------------------------------------------------------------------------

/// Référence complète d'un nœud, d'après la découverte ou, à défaut, d'après
/// les groupes d'API usuels.
fn reference_for(st: &AppState, node: &GraphNode) -> ResourceRef {
    let kind = node.key.kind;
    let found = st
        .kinds
        .iter()
        .find(|k| k.kind == kind.kind() && k.api_version() == node.api_version)
        .or_else(|| st.kinds.iter().find(|k| k.kind == kind.kind()));
    if let Some(k) = found {
        return ResourceRef::from_kind(k, node.key.namespace.as_deref(), &node.key.name);
    }
    let (group, version) = kind.group_version();
    ResourceRef {
        group: group.to_string(),
        version: version.to_string(),
        kind: kind.kind().to_string(),
        plural: kind.plural().to_string(),
        namespace: node.key.namespace.clone(),
        name: node.key.name.clone(),
    }
}

/// Applique une intention exprimée pendant le rendu.
fn apply(ctx: &egui::Context, st: &mut AppState, backend: &Backend, action: Action) {
    match action {
        Action::Refresh => request(st, backend),

        Action::Open(key) => {
            let Some(reference) = st
                .graph
                .graph
                .node(&key)
                .map(|node| reference_for(st, node))
            else {
                return;
            };
            st.graph.focus = Some(key);
            st.selected = Some(reference);
        }

        Action::Locate(key) => {
            let Some(reference) = st
                .graph
                .graph
                .node(&key)
                .map(|node| reference_for(st, node))
            else {
                return;
            };
            // Nom du type tel que l'écran Ressources le compare : pluriel
            // seul pour le groupe core, `pluriel.groupe` sinon.
            st.selected_kind = if reference.group.is_empty() {
                reference.plural.clone()
            } else {
                format!("{}.{}", reference.plural, reference.group)
            };
            st.rows.clear();
            st.continue_token = None;
            st.selected = None;
            st.filter = key.name.clone();
            st.view = View::Resources;
            // L'écran Ressources ne relance pas le listing de lui-même.
            if let Some(cluster) = st.current_cluster.clone() {
                let kind = st.selected_kind.clone();
                let opts = st.list_options();
                let id = st.next_id();
                st.pending.insert(id, format!("Liste de {kind}"));
                backend.send(Command::ListResources {
                    id,
                    cluster,
                    kind,
                    opts,
                });
            }
        }

        Action::Center(key) => {
            if let Some(p) = st.graph.positions.get(&key) {
                let center = *p + NODE_SIZE / 2.0;
                let size = st.graph.scene_rect.size();
                st.graph.scene_rect = Rect::from_center_size(center, size);
            }
        }

        Action::TogglePin(key) => {
            if !st.graph.pinned.remove(&key) {
                st.graph.pinned.insert(key);
            }
        }

        Action::Copy(text) => ctx.copy_text(text),

        Action::Fit => st.graph.fit_requested = true,

        Action::Relayout => {
            st.graph.pinned.clear();
            st.graph.positions.clear();
            st.graph.dirty = true;
            st.graph.fit_requested = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use kubewatch_core::model::OwnerRef;
    use serde_json::json;

    use super::*;

    fn object(kind: &str, ns: Option<&str>, name: &str) -> ObjectSummary {
        ObjectSummary {
            name: name.to_string(),
            namespace: ns.map(str::to_string),
            kind: kind.to_string(),
            api_version: "v1".to_string(),
            status: "Running".to_string(),
            ..Default::default()
        }
    }

    fn owned(kind: &str, ns: &str, name: &str, owner_kind: &str, owner: &str) -> ObjectSummary {
        let mut o = object(kind, Some(ns), name);
        o.owners = vec![OwnerRef {
            kind: owner_kind.to_string(),
            name: owner.to_string(),
            uid: None,
            controller: true,
        }];
        o
    }

    fn labelled(mut o: ObjectSummary, labels: &[(&str, &str)]) -> ObjectSummary {
        o.labels = labels
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>();
        o
    }

    fn key(kind: NodeKind, ns: Option<&str>, name: &str) -> NodeKey {
        NodeKey::new(kind, ns.map(str::to_string), name)
    }

    fn has_edge(g: &Graph, from: &NodeKey, to: &NodeKey, kind: EdgeKind) -> bool {
        let (Some(&a), Some(&b)) = (g.index.get(from), g.index.get(to)) else {
            return false;
        };
        g.edges
            .iter()
            .any(|e| e.from == a && e.to == b && e.kind == kind)
    }

    fn chaine() -> Vec<ObjectSummary> {
        let mut deploy = object("Deployment", Some("app"), "web");
        deploy.extra.insert("desired".into(), json!(2));
        let mut rs = owned("ReplicaSet", "app", "web-abc", "Deployment", "web");
        rs.extra.insert("desired".into(), json!(2));
        let pod1 = labelled(
            owned("Pod", "app", "web-abc-1", "ReplicaSet", "web-abc"),
            &[("app", "web")],
        );
        let pod2 = labelled(
            owned("Pod", "app", "web-abc-2", "ReplicaSet", "web-abc"),
            &[("app", "web")],
        );
        let mut svc = object("Service", Some("app"), "web");
        svc.extra.insert("selector".into(), json!({ "app": "web" }));
        let mut ing = object("Ingress", Some("app"), "web");
        ing.extra
            .insert("backends".into(), json!(["web", "absent"]));
        vec![deploy, rs, pod1, pod2, svc, ing]
    }

    #[test]
    fn replicaset_replie_relie_le_deployment_aux_pods() {
        let g = build_graph(&chaine(), &Layers::default(), false);
        assert!(!g
            .index
            .contains_key(&key(NodeKind::ReplicaSet, Some("app"), "web-abc")));
        assert!(has_edge(
            &g,
            &key(NodeKind::Deployment, Some("app"), "web"),
            &key(NodeKind::Pod, Some("app"), "web-abc-1"),
            EdgeKind::Owns
        ));
    }

    #[test]
    fn replicaset_affiche_s_intercale() {
        let layers = Layers {
            replicasets: true,
            ..Layers::default()
        };
        let g = build_graph(&chaine(), &layers, false);
        let rs = key(NodeKind::ReplicaSet, Some("app"), "web-abc");
        assert!(has_edge(
            &g,
            &key(NodeKind::Deployment, Some("app"), "web"),
            &rs,
            EdgeKind::Owns
        ));
        assert!(has_edge(
            &g,
            &rs,
            &key(NodeKind::Pod, Some("app"), "web-abc-2"),
            EdgeKind::Owns
        ));
        assert!(!has_edge(
            &g,
            &key(NodeKind::Deployment, Some("app"), "web"),
            &key(NodeKind::Pod, Some("app"), "web-abc-2"),
            EdgeKind::Owns
        ));
    }

    #[test]
    fn service_selectionne_par_labels_et_ingress_route() {
        let mut objets = chaine();
        objets.push(labelled(
            object("Pod", Some("app"), "autre"),
            &[("app", "autre")],
        ));
        let g = build_graph(&objets, &Layers::default(), false);
        let svc = key(NodeKind::Service, Some("app"), "web");
        assert!(has_edge(
            &g,
            &svc,
            &key(NodeKind::Pod, Some("app"), "web-abc-1"),
            EdgeKind::Selects
        ));
        assert!(!has_edge(
            &g,
            &svc,
            &key(NodeKind::Pod, Some("app"), "autre"),
            EdgeKind::Selects
        ));
        assert!(has_edge(
            &g,
            &key(NodeKind::Ingress, Some("app"), "web"),
            &svc,
            EdgeKind::Routes
        ));
        // Le backend absent existe comme nœud fictif : l'erreur se voit.
        let absent = g
            .node(&key(NodeKind::Service, Some("app"), "absent"))
            .expect("nœud fictif");
        assert!(!absent.known);
    }

    #[test]
    fn dependances_du_pod_et_isoles() {
        let mut pod = object("Pod", Some("app"), "db-0");
        pod.node = Some("node-a".into());
        pod.extra
            .insert("volumeClaims".into(), json!(["data-db-0"]));
        pod.extra.insert("configMaps".into(), json!(["db-config"]));
        pod.extra.insert("secrets".into(), json!(["db-secret"]));
        let seul = object("Service", Some("app"), "seul");
        let objets = vec![pod, object("Node", None, "node-a"), seul];

        let layers = Layers {
            config: true,
            ..Layers::default()
        };
        let g = build_graph(&objets, &layers, false);
        let pod_key = key(NodeKind::Pod, Some("app"), "db-0");
        assert!(has_edge(
            &g,
            &pod_key,
            &key(NodeKind::Node, None, "node-a"),
            EdgeKind::Schedules
        ));
        assert!(has_edge(
            &g,
            &pod_key,
            &key(NodeKind::VolumeClaim, Some("app"), "data-db-0"),
            EdgeKind::Mounts
        ));
        assert!(has_edge(
            &g,
            &pod_key,
            &key(NodeKind::ConfigMap, Some("app"), "db-config"),
            EdgeKind::Uses
        ));
        assert!(has_edge(
            &g,
            &pod_key,
            &key(NodeKind::Secret, Some("app"), "db-secret"),
            EdgeKind::Uses
        ));
        assert!(g
            .index
            .contains_key(&key(NodeKind::Service, Some("app"), "seul")));

        let g = build_graph(&objets, &layers, true);
        assert!(!g
            .index
            .contains_key(&key(NodeKind::Service, Some("app"), "seul")));
        assert_eq!(g.adjacency.len(), g.nodes.len());
    }

    #[test]
    fn selecteur_vide_ne_selectionne_rien() {
        let labels: BTreeMap<String, String> = [("app".to_string(), "web".to_string())].into();
        assert!(!selector_matches(&serde_json::Map::new(), &labels));
        let sel = json!({ "app": "web" });
        assert!(selector_matches(sel.as_object().unwrap(), &labels));
        let sel = json!({ "app": "web", "tier": "db" });
        assert!(!selector_matches(sel.as_object().unwrap(), &labels));
    }

    #[test]
    fn disposition_par_couches_sans_chevauchement() {
        let g = build_graph(&chaine(), &Layers::default(), false);
        let mut positions = HashMap::new();
        layered_layout(&g, &mut positions, &HashSet::new());
        assert_eq!(positions.len(), g.nodes.len());

        // Deux nœuds d'une même colonne sont séparés d'au moins un pas.
        for (i, a) in g.nodes.iter().enumerate() {
            for b in g.nodes.iter().skip(i + 1) {
                let pa = positions[&a.key];
                let pb = positions[&b.key];
                if (pa.x - pb.x).abs() < 1.0 {
                    assert!(
                        (pa.y - pb.y).abs() >= ROW_PITCH - 0.5,
                        "{} et {} se chevauchent",
                        a.key.name,
                        b.key.name
                    );
                }
            }
        }
        // L'Ingress est à gauche du Deployment, lui-même à gauche des pods.
        let x = |k: NodeKind, name: &str| positions[&key(k, Some("app"), name)].x;
        assert!(x(NodeKind::Ingress, "web") < x(NodeKind::Service, "web"));
        assert!(x(NodeKind::Service, "web") < x(NodeKind::Deployment, "web"));
        assert!(x(NodeKind::Deployment, "web") < x(NodeKind::Pod, "web-abc-1"));
    }

    #[test]
    fn disposition_organique_bouge_puis_respecte_les_epingles() {
        let g = build_graph(&chaine(), &Layers::default(), false);
        let mut positions = HashMap::new();
        layered_layout(&g, &mut positions, &HashSet::new());
        let pinned_key = key(NodeKind::Deployment, Some("app"), "web");
        let before = positions[&pinned_key];
        let pinned: HashSet<NodeKey> = [pinned_key.clone()].into();
        for _ in 0..20 {
            organic_step(&g, &mut positions, &pinned, 30.0);
        }
        assert_eq!(positions[&pinned_key], before);
        assert!(positions
            .values()
            .all(|p| p.x.is_finite() && p.y.is_finite()));
    }

    #[test]
    fn familles_et_references() {
        for kind in [
            NodeKind::Ingress,
            NodeKind::Service,
            NodeKind::Deployment,
            NodeKind::StatefulSet,
            NodeKind::DaemonSet,
            NodeKind::CronJob,
            NodeKind::ReplicaSet,
            NodeKind::Job,
            NodeKind::Pod,
            NodeKind::VolumeClaim,
            NodeKind::ConfigMap,
            NodeKind::Secret,
            NodeKind::Node,
        ] {
            assert_eq!(NodeKind::from_kind(kind.kind()), Some(kind));
            assert!(!kind.plural().is_empty());
        }
        assert_eq!(NodeKind::from_kind("Zoulou"), None);
        // Un nœud du cluster n'a jamais de namespace, quoi qu'on lui passe.
        let k = NodeKey::new(NodeKind::Node, Some("default".into()), "n1");
        assert!(k.namespace.is_none());
    }

    #[test]
    fn etat_efface_sans_perdre_les_preferences() {
        let mut st = GraphState {
            hide_isolated: true,
            mode: LayoutMode::Organic,
            ..GraphState::default()
        };
        st.receive(chaine(), vec!["ingresses : interdit".into()]);
        assert!(st.loaded && st.dirty);
        st.rebuild();
        assert!(!st.dirty);
        assert!(!st.graph.nodes.is_empty());
        st.clear();
        assert!(!st.loaded && st.objects.is_empty() && st.positions.is_empty());
        assert!(st.hide_isolated);
        assert_eq!(st.mode, LayoutMode::Organic);
    }
}
