//! Lecture et écriture des objets Kubernetes.

use serde::Serialize;
use tauri::State;

use kubewatch_core::apply::{self, ApplyOptions, ApplyOutcome};
use kubewatch_core::model::{
    ClusterOverview, ContainerInfo, EventSummary, ListOptions, MetricsSample, ObjectListPage,
    ObjectSummary, ResourceKind, ResourceRef,
};
use kubewatch_core::{events, metrics, resource, ClusterHandle};

use super::fail;
use crate::state::AppState;

/// Nombre d'évènements Kubernetes récupérés.
const EVENTS_LIMIT: u32 = 300;

/// Types lus par la topologie, par nom pluriel.
const GRAPH_KINDS: [&str; 11] = [
    "pods",
    "replicasets",
    "deployments",
    "statefulsets",
    "daemonsets",
    "jobs",
    "cronjobs",
    "services",
    "ingresses",
    "persistentvolumeclaims",
    "nodes",
];
const GRAPH_PAGE: u32 = 1_000;
const GRAPH_MAX_PER_KIND: usize = 5_000;

/// Différence entre le cluster et un document d'un manifeste.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffItem {
    /// Ressource visée.
    pub resource: ResourceRef,
    /// Différence au format texte unifié.
    pub diff: String,
}

/// Instantané de la topologie.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSnapshot {
    /// Objets lus, tous types confondus.
    pub objects: Vec<ObjectSummary>,
    /// Types qui n'ont pas pu être lus, avec la cause.
    pub warnings: Vec<String>,
}

/// Mesures de consommation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsBundle {
    /// Par nœud.
    pub nodes: Vec<MetricsSample>,
    /// Par pod.
    pub pods: Vec<MetricsSample>,
}

#[tauri::command]
pub async fn cluster_overview(
    state: State<'_, AppState>,
    cluster: String,
) -> Result<ClusterOverview, String> {
    let h = state.handle(&cluster)?;
    metrics::overview(&h)
        .await
        .map_err(|e| fail(&format!("synthèse de « {} » impossible", h.name), e))
}

#[tauri::command]
pub fn list_kinds(
    state: State<'_, AppState>,
    cluster: String,
) -> Result<Vec<ResourceKind>, String> {
    let h = state.handle(&cluster)?;
    let mut kinds: Vec<ResourceKind> = {
        let guard = h.catalog.read();
        guard.listable().into_iter().cloned().collect()
    };
    kinds.sort_by_key(|k| k.full_name());
    Ok(kinds)
}

#[tauri::command]
pub async fn list_namespaces(
    state: State<'_, AppState>,
    cluster: String,
) -> Result<Vec<String>, String> {
    let h = state.handle(&cluster)?;
    let kind = h
        .resolve_kind("namespaces")
        .map_err(|e| fail("namespaces introuvables", e))?;
    let opts = ListOptions {
        limit: Some(1_000),
        ..Default::default()
    };
    let page = resource::list(&h, &kind, &opts)
        .await
        .map_err(|e| fail("lecture des namespaces impossible", e))?;
    let mut namespaces: Vec<String> = page.items.into_iter().map(|i| i.name).collect();
    namespaces.sort();
    Ok(namespaces)
}

#[tauri::command]
pub async fn list_resources(
    state: State<'_, AppState>,
    cluster: String,
    kind: String,
    opts: ListOptions,
) -> Result<ObjectListPage, String> {
    let h = state.handle(&cluster)?;
    let wanted = match kind.trim() {
        "" => "pods",
        other => other,
    };
    let resolved = h
        .resolve_kind(wanted)
        .map_err(|e| fail(&format!("type « {wanted} » inconnu"), e))?;
    resource::list(&h, &resolved, &opts)
        .await
        .map_err(|e| fail(&format!("listing des « {wanted} » impossible"), e))
}

#[tauri::command]
pub async fn load_graph(
    state: State<'_, AppState>,
    cluster: String,
    namespace: Option<String>,
) -> Result<GraphSnapshot, String> {
    let h = state.handle(&cluster)?;
    let ns = namespace
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (objects, warnings) = read_graph(&h, ns).await;
    if objects.is_empty() && warnings.len() == GRAPH_KINDS.len() {
        return Err(format!(
            "topologie de « {} » illisible : {}",
            h.name,
            warnings.join(" ; ")
        ));
    }
    Ok(GraphSnapshot { objects, warnings })
}

/// Lit tous les types de la topologie en parallèle, paginés jusqu'au bout.
async fn read_graph(
    h: &ClusterHandle,
    namespace: Option<&str>,
) -> (Vec<ObjectSummary>, Vec<String>) {
    let reads = GRAPH_KINDS.iter().map(|plural| async move {
        let kind = h
            .resolve_kind(plural)
            .map_err(|e| format!("{plural} : {e}"))?;
        let mut items: Vec<ObjectSummary> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let opts = ListOptions {
                namespace: namespace.map(str::to_string),
                limit: Some(GRAPH_PAGE),
                continue_token: token.take(),
                ..Default::default()
            };
            let page = resource::list(h, &kind, &opts)
                .await
                .map_err(|e| format!("{plural} : {e}"))?;
            items.extend(page.items);
            token = page.continue_token;
            if token.is_none() || items.len() >= GRAPH_MAX_PER_KIND {
                break;
            }
        }
        Ok::<_, String>(items)
    });
    let mut objects = Vec::new();
    let mut warnings = Vec::new();
    for result in futures::future::join_all(reads).await {
        match result {
            Ok(items) => objects.extend(items),
            Err(w) => warnings.push(w),
        }
    }
    (objects, warnings)
}

#[tauri::command]
pub async fn get_yaml(
    state: State<'_, AppState>,
    cluster: String,
    reference: ResourceRef,
) -> Result<String, String> {
    let h = state.handle(&cluster)?;
    resource::get_yaml(&h, &reference)
        .await
        .map_err(|e| fail(&format!("lecture de {} impossible", reference.display()), e))
}

#[tauri::command]
pub async fn apply_yaml(
    state: State<'_, AppState>,
    cluster: String,
    yaml: String,
    dry_run: bool,
    force: bool,
    namespace: Option<String>,
) -> Result<ApplyOutcome, String> {
    let h = state.handle(&cluster)?;
    let opts = ApplyOptions {
        force,
        dry_run,
        default_namespace: namespace.filter(|n| !n.trim().is_empty()),
        ..Default::default()
    };
    apply::apply_yaml(&h, &yaml, &opts)
        .await
        .map_err(|e| fail("application impossible", e))
}

#[tauri::command]
pub async fn diff_yaml(
    state: State<'_, AppState>,
    cluster: String,
    yaml: String,
    namespace: Option<String>,
) -> Result<Vec<DiffItem>, String> {
    let h = state.handle(&cluster)?;
    let opts = ApplyOptions {
        default_namespace: namespace.filter(|n| !n.trim().is_empty()),
        ..Default::default()
    };
    let items = apply::diff_yaml(&h, &yaml, &opts)
        .await
        .map_err(|e| fail("calcul des différences impossible", e))?;
    Ok(items
        .into_iter()
        .map(|(resource, diff)| DiffItem { resource, diff })
        .collect())
}

#[tauri::command]
pub async fn delete_yaml(
    state: State<'_, AppState>,
    cluster: String,
    yaml: String,
    namespace: Option<String>,
) -> Result<ApplyOutcome, String> {
    let h = state.handle(&cluster)?;
    let opts = ApplyOptions {
        default_namespace: namespace.filter(|n| !n.trim().is_empty()),
        ..Default::default()
    };
    apply::delete_yaml(&h, &yaml, &opts)
        .await
        .map_err(|e| fail("suppression impossible", e))
}

#[tauri::command]
pub async fn replace_yaml(
    state: State<'_, AppState>,
    cluster: String,
    reference: ResourceRef,
    yaml: String,
) -> Result<(), String> {
    let h = state.handle(&cluster)?;
    resource::replace_yaml(&h, &reference, &yaml)
        .await
        .map_err(|e| {
            fail(
                &format!("remplacement de {} impossible", reference.display()),
                e,
            )
        })
}

#[tauri::command]
pub async fn delete_resource(
    state: State<'_, AppState>,
    cluster: String,
    reference: ResourceRef,
    propagation: Option<String>,
) -> Result<(), String> {
    let h = state.handle(&cluster)?;
    resource::delete(&h, &reference, propagation.as_deref())
        .await
        .map_err(|e| {
            fail(
                &format!("suppression de {} impossible", reference.display()),
                e,
            )
        })
}

#[tauri::command]
pub async fn scale_resource(
    state: State<'_, AppState>,
    cluster: String,
    reference: ResourceRef,
    replicas: i32,
) -> Result<(), String> {
    let h = state.handle(&cluster)?;
    resource::scale(&h, &reference, replicas)
        .await
        .map_err(|e| fail("changement d'échelle impossible", e))
}

#[tauri::command]
pub async fn restart_resource(
    state: State<'_, AppState>,
    cluster: String,
    reference: ResourceRef,
) -> Result<(), String> {
    let h = state.handle(&cluster)?;
    resource::restart(&h, &reference)
        .await
        .map_err(|e| fail("redémarrage impossible", e))
}

#[tauri::command]
pub async fn set_image(
    state: State<'_, AppState>,
    cluster: String,
    reference: ResourceRef,
    container: Option<String>,
    image: String,
) -> Result<(), String> {
    let h = state.handle(&cluster)?;
    resource::set_image(&h, &reference, container.as_deref(), &image)
        .await
        .map_err(|e| fail("changement d'image impossible", e))
}

#[tauri::command]
pub async fn rollback_resource(
    state: State<'_, AppState>,
    cluster: String,
    reference: ResourceRef,
) -> Result<(), String> {
    let h = state.handle(&cluster)?;
    resource::rollback(&h, &reference)
        .await
        .map_err(|e| fail("retour arrière impossible", e))
}

#[tauri::command]
pub async fn cordon_node(
    state: State<'_, AppState>,
    cluster: String,
    node: String,
    on: bool,
) -> Result<(), String> {
    let h = state.handle(&cluster)?;
    resource::cordon(&h, &node, on)
        .await
        .map_err(|e| fail("opération sur le nœud impossible", e))
}

/// Vide un nœud ; renvoie les pods évincés (`namespace/nom`).
#[tauri::command]
pub async fn drain_node(
    state: State<'_, AppState>,
    cluster: String,
    node: String,
) -> Result<Vec<String>, String> {
    let h = state.handle(&cluster)?;
    resource::drain(&h, &node, None)
        .await
        .map_err(|e| fail(&format!("vidage du nœud « {node} » impossible"), e))
}

#[tauri::command]
pub async fn load_events(
    state: State<'_, AppState>,
    cluster: String,
    namespace: Option<String>,
    involved: Option<ResourceRef>,
) -> Result<Vec<EventSummary>, String> {
    let h = state.handle(&cluster)?;
    let ns = namespace
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    events::recent(&h, ns, involved.as_ref(), EVENTS_LIMIT)
        .await
        .map_err(|e| fail("lecture des évènements impossible", e))
}

#[tauri::command]
pub async fn load_containers(
    state: State<'_, AppState>,
    cluster: String,
    pod: ResourceRef,
) -> Result<Vec<ContainerInfo>, String> {
    let h = state.handle(&cluster)?;
    resource::containers(&h, &pod)
        .await
        .map_err(|e| fail(&format!("conteneurs de {} illisibles", pod.display()), e))
}

#[tauri::command]
pub async fn load_metrics(
    state: State<'_, AppState>,
    cluster: String,
    namespace: Option<String>,
) -> Result<MetricsBundle, String> {
    let h = state.handle(&cluster)?;
    let ns = namespace
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let nodes = metrics::node_metrics(&h.client).await;
    let pods = metrics::pod_metrics(&h.client, ns).await;
    match (nodes, pods) {
        (Err(en), Err(ep)) => Err(format!(
            "métriques indisponibles (l'API metrics.k8s.io répond-elle ?) : nœuds — {en} ; \
             pods — {ep}"
        )),
        (nodes, pods) => Ok(MetricsBundle {
            nodes: nodes.unwrap_or_else(|e| {
                tracing::warn!(erreur = %e, "métriques des nœuds indisponibles");
                Vec::new()
            }),
            pods: pods.unwrap_or_else(|e| {
                tracing::warn!(erreur = %e, "métriques des pods indisponibles");
                Vec::new()
            }),
        }),
    }
}
