//! Métriques d'usage (`metrics.k8s.io`) et vue d'ensemble du cluster.
//!
//! `metrics.k8s.io` n'est pas décrit par `k8s-openapi`: on interroge donc l'API
//! agrégée en requête brute et on désérialise dans des structures locales.

use std::collections::BTreeMap;
use std::fmt::Debug;

use chrono::{DateTime, Utc};
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{Namespace, Node, Pod};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::api::{Api, ListParams, ResourceExt};
use kube::Client;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::cluster::ClusterHandle;
use crate::error::{Error, Result};
use crate::model::{ClusterOverview, MetricsSample};

/// Racine de l'API agrégée des métriques.
const METRICS_GROUP: &str = "/apis/metrics.k8s.io";
/// Version utilisée pour les métriques.
const METRICS_V1BETA1: &str = "/apis/metrics.k8s.io/v1beta1";

/// Taille d'une page lors des listages complets.
const PAGE_SIZE: u32 = 500;
/// Garde-fou: nombre maximal d'objets chargés pour un listage complet.
const MAX_ITEMS: usize = 20_000;
/// Nombre maximal d'avertissements remontés dans une vue d'ensemble.
const MAX_WARNINGS: usize = 25;

// ---------------------------------------------------------------------------
// Analyse des quantités Kubernetes
// ---------------------------------------------------------------------------

/// Convertit une quantité Kubernetes en unité de base (cœurs pour le CPU,
/// octets pour la mémoire).
///
/// Gère les suffixes décimaux (`n`, `u`, `m`, aucun, `k`, `M`, `G`, `T`, `P`, `E`),
/// les suffixes binaires (`Ki`, `Mi`, `Gi`, `Ti`, `Pi`, `Ei`) et la notation
/// exponentielle (`1e3`, `1.5E-2`).
pub fn parse_quantity(q: &str) -> Option<f64> {
    let text = q.trim();
    if text.is_empty() {
        return None;
    }
    let bytes = text.as_bytes();
    let mut idx = 0usize;

    if bytes[idx] == b'+' || bytes[idx] == b'-' {
        idx += 1;
    }

    let int_start = idx;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    let int_len = idx - int_start;

    let mut frac_len = 0usize;
    if idx < bytes.len() && bytes[idx] == b'.' {
        idx += 1;
        let frac_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        frac_len = idx - frac_start;
    }

    if int_len == 0 && frac_len == 0 {
        return None;
    }

    let mantissa: f64 = text[..idx].parse().ok()?;
    let suffix = &text[idx..];
    if suffix.is_empty() {
        return Some(mantissa);
    }

    // Exposant décimal: `e`/`E` suivi d'un entier signé. Attention, un `E` seul
    // est le suffixe « exa », pas un exposant.
    let suffix_bytes = suffix.as_bytes();
    if suffix_bytes[0] == b'e' || suffix_bytes[0] == b'E' {
        let rest = &suffix[1..];
        let rest_bytes = rest.as_bytes();
        let digits_start =
            if !rest_bytes.is_empty() && (rest_bytes[0] == b'+' || rest_bytes[0] == b'-') {
                1
            } else {
                0
            };
        if rest_bytes.len() > digits_start
            && rest_bytes[digits_start..].iter().all(u8::is_ascii_digit)
        {
            let exponent: i32 = rest.parse().ok()?;
            return Some(mantissa * 10f64.powi(exponent));
        }
    }

    let factor = match suffix {
        "n" => 1e-9,
        "u" | "µ" => 1e-6,
        "m" => 1e-3,
        "k" | "K" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        "P" => 1e15,
        "E" => 1e18,
        "Ki" => 1024f64,
        "Mi" => 1024f64 * 1024f64,
        "Gi" => 1024f64 * 1024f64 * 1024f64,
        "Ti" => 1024f64.powi(4),
        "Pi" => 1024f64.powi(5),
        "Ei" => 1024f64.powi(6),
        _ => return None,
    };

    Some(mantissa * factor)
}

/// Convertit une quantité CPU en millicœurs (`"100m"` -> `100.0`, `"1"` -> `1000.0`).
pub fn parse_cpu_millis(q: &str) -> Option<f64> {
    let cores = parse_quantity(q)?;
    if !cores.is_finite() {
        return None;
    }
    Some(cores * 1000.0)
}

/// Convertit une quantité mémoire en octets (`"2Gi"` -> `2147483648`).
pub fn parse_memory_bytes(q: &str) -> Option<i64> {
    let bytes = parse_quantity(q)?;
    if !bytes.is_finite() {
        return None;
    }
    let rounded = bytes.round();
    if rounded < i64::MIN as f64 || rounded > i64::MAX as f64 {
        return None;
    }
    Some(rounded as i64)
}

// ---------------------------------------------------------------------------
// Représentation brute de metrics.k8s.io
// ---------------------------------------------------------------------------

/// Remplace `null` par la valeur par défaut: l'API server sérialise volontiers
/// des champs de liste ou d'objet à `null` plutôt que de les omettre.
fn null_to_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Default, Deserialize)]
struct RawMeta {
    #[serde(default, deserialize_with = "null_to_default")]
    name: String,
    #[serde(default)]
    namespace: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawUsage {
    #[serde(default)]
    cpu: Option<String>,
    #[serde(default)]
    memory: Option<String>,
}

impl RawUsage {
    fn cpu_millis(&self) -> f64 {
        self.cpu
            .as_deref()
            .and_then(parse_cpu_millis)
            .unwrap_or(0.0)
    }

    fn memory_bytes(&self) -> i64 {
        self.memory
            .as_deref()
            .and_then(parse_memory_bytes)
            .unwrap_or(0)
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawList<T> {
    #[serde(
        default,
        deserialize_with = "null_to_default",
        bound(deserialize = "T: Deserialize<'de>")
    )]
    items: Vec<T>,
}

#[derive(Debug, Default, Deserialize)]
struct RawNodeMetrics {
    #[serde(default, deserialize_with = "null_to_default")]
    metadata: RawMeta,
    #[serde(default)]
    timestamp: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "null_to_default")]
    usage: RawUsage,
}

#[derive(Debug, Default, Deserialize)]
struct RawContainerMetrics {
    #[serde(default, deserialize_with = "null_to_default")]
    name: String,
    #[serde(default, deserialize_with = "null_to_default")]
    usage: RawUsage,
}

#[derive(Debug, Default, Deserialize)]
struct RawPodMetrics {
    #[serde(default, deserialize_with = "null_to_default")]
    metadata: RawMeta,
    #[serde(default)]
    timestamp: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "null_to_default")]
    containers: Vec<RawContainerMetrics>,
}

#[derive(Debug, Default, Deserialize)]
struct RawApiGroup {
    #[serde(default, deserialize_with = "null_to_default")]
    versions: Vec<serde_json::Value>,
}

/// Exécute une requête brute `GET` sur l'API server.
async fn raw_get<T: DeserializeOwned>(client: &Client, path: &str) -> Result<T> {
    let request = http::Request::get(path)
        .body(Vec::new())
        .map_err(|err| Error::Other(format!("requête interne invalide ({path}): {err}")))?;
    let value = client.request::<T>(request).await?;
    Ok(value)
}

/// Indique si `metrics-server` (ou un équivalent) répond.
///
/// Ne propage jamais d'erreur: une absence de métriques n'est pas une panne.
pub async fn metrics_available(client: &Client) -> bool {
    // Chemin nominal: une seule requête suffit quand metrics-server répond.
    if raw_get::<RawList<RawNodeMetrics>>(client, &format!("{METRICS_V1BETA1}/nodes?limit=1"))
        .await
        .is_ok()
    {
        return true;
    }

    // Sinon, distinguer « groupe agrégé absent » de « lecture des nœuds
    // interdite par RBAC ».
    let group_registered = raw_get::<RawApiGroup>(client, METRICS_GROUP)
        .await
        .map(|group| !group.versions.is_empty())
        .unwrap_or(false);
    if !group_registered {
        return false;
    }

    raw_get::<RawList<RawPodMetrics>>(client, &format!("{METRICS_V1BETA1}/pods?limit=1"))
        .await
        .is_ok()
}

/// Consommation courante de chaque nœud.
pub async fn node_metrics(client: &Client) -> Result<Vec<MetricsSample>> {
    let list: RawList<RawNodeMetrics> =
        raw_get(client, &format!("{METRICS_V1BETA1}/nodes")).await?;
    Ok(list
        .items
        .into_iter()
        .map(|item| MetricsSample {
            name: item.metadata.name,
            namespace: None,
            container: None,
            cpu_millis: item.usage.cpu_millis(),
            memory_bytes: item.usage.memory_bytes(),
            timestamp: item.timestamp,
        })
        .collect())
}

/// Consommation courante des pods.
///
/// Pour chaque pod, un échantillon agrégé (`container == None`) est produit,
/// suivi d'un échantillon par conteneur.
pub async fn pod_metrics(client: &Client, namespace: Option<&str>) -> Result<Vec<MetricsSample>> {
    let path = match namespace.map(str::trim).filter(|n| !n.is_empty()) {
        Some(ns) => {
            if !is_valid_namespace(ns) {
                return Err(Error::Invalid(format!("nom de namespace invalide: {ns}")));
            }
            format!("{METRICS_V1BETA1}/namespaces/{ns}/pods")
        }
        None => format!("{METRICS_V1BETA1}/pods"),
    };

    let list: RawList<RawPodMetrics> = raw_get(client, &path).await?;

    let mut samples = Vec::with_capacity(list.items.len() * 2);
    for item in list.items {
        let pod_name = item.metadata.name;
        let pod_namespace = item.metadata.namespace;

        let mut cpu_total = 0.0f64;
        let mut memory_total = 0i64;
        let mut per_container = Vec::with_capacity(item.containers.len());

        for container in item.containers {
            let cpu = container.usage.cpu_millis();
            let memory = container.usage.memory_bytes();
            cpu_total += cpu;
            memory_total = memory_total.saturating_add(memory);
            per_container.push(MetricsSample {
                name: pod_name.clone(),
                namespace: pod_namespace.clone(),
                container: Some(container.name),
                cpu_millis: cpu,
                memory_bytes: memory,
                timestamp: item.timestamp,
            });
        }

        samples.push(MetricsSample {
            name: pod_name,
            namespace: pod_namespace,
            container: None,
            cpu_millis: cpu_total,
            memory_bytes: memory_total,
            timestamp: item.timestamp,
        });
        samples.append(&mut per_container);
    }

    Ok(samples)
}

fn is_valid_namespace(ns: &str) -> bool {
    !ns.is_empty()
        && ns.len() <= 253
        && ns
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

// ---------------------------------------------------------------------------
// Vue d'ensemble du cluster
// ---------------------------------------------------------------------------

/// Agrège l'état global d'un cluster.
///
/// Aucune sous-requête en échec ne fait échouer l'ensemble: les problèmes
/// rencontrés sont consignés dans `warnings`.
pub async fn overview(h: &ClusterHandle) -> Result<ClusterOverview> {
    let client = &h.client;
    let mut warnings: Vec<String> = Vec::new();

    // --- Nœuds -------------------------------------------------------------
    let mut nodes_total = 0usize;
    let mut nodes_ready = 0usize;
    let mut cpu_capacity_millis = 0.0f64;
    let mut memory_capacity_bytes = 0i64;

    let nodes_api: Api<Node> = Api::all(client.clone());
    match list_all(&nodes_api).await {
        Ok(nodes) => {
            nodes_total = nodes.len();
            for node in &nodes {
                if node_is_ready(node) {
                    nodes_ready += 1;
                } else {
                    push_warning(
                        &mut warnings,
                        format!("nœud {} non prêt (NotReady)", node.name_any()),
                    );
                }
                if let Some(cpu) = node_resource(node, "cpu") {
                    cpu_capacity_millis += cpu * 1000.0;
                }
                if let Some(memory) = node_resource(node, "memory") {
                    memory_capacity_bytes = memory_capacity_bytes.saturating_add(memory as i64);
                }
            }
        }
        Err(err) => push_warning(
            &mut warnings,
            format!("listage des nœuds impossible: {err}"),
        ),
    }

    // --- Namespaces --------------------------------------------------------
    let namespaces_api: Api<Namespace> = Api::all(client.clone());
    let namespaces = match count_all(&namespaces_api).await {
        Ok(count) => count,
        Err(err) => {
            push_warning(
                &mut warnings,
                format!("listage des namespaces impossible: {err}"),
            );
            0
        }
    };

    // --- Pods --------------------------------------------------------------
    let mut pods_total = 0usize;
    let mut pods_running = 0usize;
    let mut pods_pending = 0usize;
    let mut pods_failed = 0usize;

    let pods_api: Api<Pod> = Api::all(client.clone());
    match list_all(&pods_api).await {
        Ok(pods) => {
            pods_total = pods.len();
            for pod in &pods {
                match pod_phase(pod) {
                    "Running" => pods_running += 1,
                    "Pending" => pods_pending += 1,
                    "Failed" => pods_failed += 1,
                    _ => {}
                }
                for reason in problematic_containers(pod) {
                    push_warning(
                        &mut warnings,
                        format!(
                            "pod {}/{} en {reason}",
                            pod.namespace().unwrap_or_else(|| "-".to_string()),
                            pod.name_any()
                        ),
                    );
                }
            }
        }
        Err(err) => push_warning(&mut warnings, format!("listage des pods impossible: {err}")),
    }

    // --- Charges de travail ------------------------------------------------
    let mut workloads: BTreeMap<String, usize> = BTreeMap::new();

    count_into::<Deployment>(client, "Deployment", &mut workloads, &mut warnings).await;
    count_into::<StatefulSet>(client, "StatefulSet", &mut workloads, &mut warnings).await;
    count_into::<DaemonSet>(client, "DaemonSet", &mut workloads, &mut warnings).await;
    count_into::<ReplicaSet>(client, "ReplicaSet", &mut workloads, &mut warnings).await;
    count_into::<Job>(client, "Job", &mut workloads, &mut warnings).await;
    count_into::<CronJob>(client, "CronJob", &mut workloads, &mut warnings).await;

    // --- Consommation ------------------------------------------------------
    let mut cpu_used_millis = 0.0f64;
    let mut memory_used_bytes = 0i64;
    let metrics_available_flag = metrics_available(client).await;

    if metrics_available_flag {
        match node_metrics(client).await {
            Ok(samples) if !samples.is_empty() => {
                for sample in samples {
                    cpu_used_millis += sample.cpu_millis;
                    memory_used_bytes = memory_used_bytes.saturating_add(sample.memory_bytes);
                }
            }
            Ok(_) => {
                // Pas de métriques de nœud lisibles: on retombe sur les pods.
                match pod_metrics(client, None).await {
                    Ok(samples) => {
                        for sample in samples.iter().filter(|s| s.container.is_none()) {
                            cpu_used_millis += sample.cpu_millis;
                            memory_used_bytes =
                                memory_used_bytes.saturating_add(sample.memory_bytes);
                        }
                    }
                    Err(err) => push_warning(
                        &mut warnings,
                        format!("métriques des pods indisponibles: {err}"),
                    ),
                }
            }
            Err(err) => push_warning(
                &mut warnings,
                format!("métriques des nœuds indisponibles: {err}"),
            ),
        }
    } else {
        push_warning(
            &mut warnings,
            "metrics-server indisponible: consommation CPU/mémoire non mesurée".to_string(),
        );
    }

    Ok(ClusterOverview {
        nodes_total,
        nodes_ready,
        pods_total,
        pods_running,
        pods_pending,
        pods_failed,
        namespaces,
        cpu_capacity_millis,
        cpu_used_millis,
        memory_capacity_bytes,
        memory_used_bytes,
        workloads,
        warnings,
        metrics_available: metrics_available_flag,
    })
}

fn push_warning(warnings: &mut Vec<String>, message: String) {
    if warnings.len() < MAX_WARNINGS {
        warnings.push(message);
    } else if warnings.len() == MAX_WARNINGS {
        warnings.push("… avertissements supplémentaires tronqués".to_string());
    }
}

/// Un nœud est prêt si sa condition `Ready` vaut `True`.
fn node_is_ready(node: &Node) -> bool {
    node.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .map(|conditions| {
            conditions
                .iter()
                .any(|c| c.type_ == "Ready" && c.status == "True")
        })
        .unwrap_or(false)
}

/// Capacité (ou à défaut allocatable) d'un nœud pour une ressource donnée.
fn node_resource(node: &Node, key: &str) -> Option<f64> {
    let status = node.status.as_ref()?;
    read_quantity(status.capacity.as_ref(), key)
        .or_else(|| read_quantity(status.allocatable.as_ref(), key))
}

fn read_quantity(map: Option<&BTreeMap<String, Quantity>>, key: &str) -> Option<f64> {
    map?.get(key).and_then(|q| parse_quantity(&q.0))
}

fn pod_phase(pod: &Pod) -> &str {
    pod.status
        .as_ref()
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("Unknown")
}

/// Raisons d'attente notables des conteneurs d'un pod.
fn problematic_containers(pod: &Pod) -> Vec<String> {
    const NOTABLE: &[&str] = &[
        "CrashLoopBackOff",
        "ImagePullBackOff",
        "ErrImagePull",
        "CreateContainerConfigError",
    ];

    let Some(status) = pod.status.as_ref() else {
        return Vec::new();
    };

    let mut reasons = Vec::new();
    let all = status
        .container_statuses
        .iter()
        .flatten()
        .chain(status.init_container_statuses.iter().flatten());

    for container in all {
        let reason = container
            .state
            .as_ref()
            .and_then(|s| s.waiting.as_ref())
            .and_then(|w| w.reason.as_deref());
        if let Some(reason) = reason {
            if NOTABLE.contains(&reason) && !reasons.iter().any(|r| r == reason) {
                reasons.push(reason.to_string());
            }
        }
    }
    reasons
}

/// Compte une catégorie de charge de travail sans faire échouer la vue d'ensemble.
async fn count_into<K>(
    client: &Client,
    label: &str,
    workloads: &mut BTreeMap<String, usize>,
    warnings: &mut Vec<String>,
) where
    K: kube::Resource + Clone + DeserializeOwned + Debug,
    <K as kube::Resource>::DynamicType: Default,
{
    let api: Api<K> = Api::all(client.clone());
    match count_all(&api).await {
        Ok(count) => {
            workloads.insert(label.to_string(), count);
        }
        Err(err) => push_warning(
            warnings,
            format!("comptage des ressources {label} impossible: {err}"),
        ),
    }
}

/// Compte les objets d'un type sans les télécharger tous quand c'est possible.
async fn count_all<K>(api: &Api<K>) -> Result<usize>
where
    K: Clone + DeserializeOwned + Debug,
{
    let page = api.list(&ListParams::default().limit(1)).await?;

    if let Some(remaining) = page.metadata.remaining_item_count {
        if remaining >= 0 {
            return Ok(page.items.len() + remaining as usize);
        }
    }

    let has_more = page
        .metadata
        .continue_
        .as_deref()
        .map(|token| !token.is_empty())
        .unwrap_or(false);
    if !has_more {
        return Ok(page.items.len());
    }

    Ok(list_all(api).await?.len())
}

/// Listage paginé complet, avec garde-fou sur le nombre d'objets.
async fn list_all<K>(api: &Api<K>) -> Result<Vec<K>>
where
    K: Clone + DeserializeOwned + Debug,
{
    let mut items: Vec<K> = Vec::new();
    let mut continue_token: Option<String> = None;

    loop {
        let params = ListParams {
            limit: Some(PAGE_SIZE),
            continue_token: continue_token.clone(),
            ..Default::default()
        };
        let mut page = api.list(&params).await?;
        items.append(&mut page.items);

        continue_token = page.metadata.continue_.filter(|token| !token.is_empty());

        if continue_token.is_none() || items.len() >= MAX_ITEMS {
            break;
        }
    }

    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "attendu {expected}, obtenu {actual}"
        );
    }

    #[test]
    fn quantites_sans_suffixe() {
        approx(parse_quantity("0").expect("0"), 0.0);
        approx(parse_quantity("1").expect("1"), 1.0);
        approx(parse_quantity("1.5").expect("1.5"), 1.5);
        approx(parse_quantity("128974848").expect("octets"), 128_974_848.0);
        approx(parse_quantity("-2").expect("négatif"), -2.0);
        approx(parse_quantity("+3").expect("signe explicite"), 3.0);
        approx(parse_quantity("  42  ").expect("espaces"), 42.0);
    }

    #[test]
    fn quantites_decimales() {
        approx(parse_quantity("100m").expect("100m"), 0.1);
        approx(parse_quantity("1n").expect("1n"), 1e-9);
        approx(parse_quantity("1u").expect("1u"), 1e-6);
        approx(parse_quantity("1k").expect("1k"), 1000.0);
        approx(parse_quantity("1M").expect("1M"), 1e6);
        approx(parse_quantity("1G").expect("1G"), 1e9);
        approx(parse_quantity("1T").expect("1T"), 1e12);
        approx(parse_quantity("1P").expect("1P"), 1e15);
        approx(parse_quantity("1E").expect("1E"), 1e18);
    }

    #[test]
    fn quantites_binaires() {
        approx(parse_quantity("1Ki").expect("1Ki"), 1024.0);
        approx(parse_quantity("500Mi").expect("500Mi"), 524_288_000.0);
        approx(parse_quantity("2Gi").expect("2Gi"), 2_147_483_648.0);
        approx(parse_quantity("1Ti").expect("1Ti"), 1_099_511_627_776.0);
        approx(parse_quantity("1Pi").expect("1Pi"), 1024f64.powi(5));
        approx(parse_quantity("1Ei").expect("1Ei"), 1024f64.powi(6));
    }

    #[test]
    fn notation_exponentielle() {
        approx(parse_quantity("1e3").expect("1e3"), 1000.0);
        approx(parse_quantity("1E3").expect("1E3"), 1000.0);
        approx(parse_quantity("1.5e2").expect("1.5e2"), 150.0);
        approx(parse_quantity("2e-3").expect("2e-3"), 0.002);
        approx(parse_quantity("5e+2").expect("5e+2"), 500.0);
        // `E` seul reste le suffixe exa, pas un exposant tronqué.
        approx(parse_quantity("2E").expect("2E"), 2e18);
    }

    #[test]
    fn quantites_invalides() {
        assert!(parse_quantity("").is_none());
        assert!(parse_quantity("   ").is_none());
        assert!(parse_quantity("abc").is_none());
        assert!(parse_quantity("m").is_none());
        assert!(parse_quantity("-").is_none());
        assert!(parse_quantity(".").is_none());
        assert!(parse_quantity("12Xi").is_none());
        assert!(parse_quantity("1.2.3").is_none());
        assert!(parse_quantity("1 Gi").is_none());
    }

    #[test]
    fn cpu_en_millicoeurs() {
        approx(parse_cpu_millis("100m").expect("100m"), 100.0);
        approx(parse_cpu_millis("1").expect("1"), 1000.0);
        approx(parse_cpu_millis("1.5").expect("1.5"), 1500.0);
        approx(parse_cpu_millis("0").expect("0"), 0.0);
        approx(parse_cpu_millis("1e3").expect("1e3"), 1_000_000.0);
        approx(parse_cpu_millis("250m").expect("250m"), 250.0);
        approx(parse_cpu_millis("1500m").expect("1500m"), 1500.0);
        // metrics-server renvoie des nanocœurs.
        approx(parse_cpu_millis("123456789n").expect("nanos"), 123.456789);
        assert!(parse_cpu_millis("nope").is_none());
    }

    #[test]
    fn memoire_en_octets() {
        assert_eq!(parse_memory_bytes("0"), Some(0));
        assert_eq!(parse_memory_bytes("1"), Some(1));
        assert_eq!(parse_memory_bytes("1.5"), Some(2)); // arrondi au plus proche
        assert_eq!(parse_memory_bytes("2Gi"), Some(2_147_483_648));
        assert_eq!(parse_memory_bytes("500Mi"), Some(524_288_000));
        assert_eq!(parse_memory_bytes("1e3"), Some(1000));
        assert_eq!(parse_memory_bytes("128974848"), Some(128_974_848));
        assert_eq!(parse_memory_bytes("2210472Ki"), Some(2_263_523_328));
        assert_eq!(parse_memory_bytes("1Mi"), Some(1_048_576));
        assert!(parse_memory_bytes("").is_none());
        assert!(parse_memory_bytes("1Gigo").is_none());
    }

    #[test]
    fn usage_brut_tolerant() {
        let usage = RawUsage {
            cpu: Some("250m".to_string()),
            memory: Some("512Mi".to_string()),
        };
        approx(usage.cpu_millis(), 250.0);
        assert_eq!(usage.memory_bytes(), 536_870_912);

        let vide = RawUsage::default();
        approx(vide.cpu_millis(), 0.0);
        assert_eq!(vide.memory_bytes(), 0);

        let illisible = RawUsage {
            cpu: Some("???".to_string()),
            memory: Some("???".to_string()),
        };
        approx(illisible.cpu_millis(), 0.0);
        assert_eq!(illisible.memory_bytes(), 0);
    }

    #[test]
    fn deserialisation_metrics_noeuds() {
        let json = r#"{
            "kind": "NodeMetricsList",
            "items": [
                {
                    "metadata": {"name": "node-1"},
                    "timestamp": "2026-09-18T10:00:00Z",
                    "usage": {"cpu": "137669144n", "memory": "2210472Ki"}
                }
            ]
        }"#;
        let list: RawList<RawNodeMetrics> = serde_json::from_str(json).expect("désérialisation");
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].metadata.name, "node-1");
        approx(list.items[0].usage.cpu_millis(), 137.669144);
        assert_eq!(list.items[0].usage.memory_bytes(), 2_263_523_328);
        assert!(list.items[0].timestamp.is_some());
    }

    #[test]
    fn deserialisation_metrics_pods() {
        let json = r#"{
            "items": [
                {
                    "metadata": {"name": "web-1", "namespace": "prod"},
                    "containers": [
                        {"name": "app", "usage": {"cpu": "10m", "memory": "64Mi"}},
                        {"name": "sidecar", "usage": {"cpu": "5m", "memory": "32Mi"}}
                    ]
                }
            ]
        }"#;
        let list: RawList<RawPodMetrics> = serde_json::from_str(json).expect("désérialisation");
        assert_eq!(list.items[0].containers.len(), 2);
        assert_eq!(list.items[0].metadata.namespace.as_deref(), Some("prod"));
        assert!(list.items[0].timestamp.is_none());
    }

    #[test]
    fn champs_nuls_toleres() {
        // L'API server sérialise volontiers `items`, `containers` ou `usage` à `null`.
        let json = r#"{"items": null}"#;
        let list: RawList<RawNodeMetrics> = serde_json::from_str(json).expect("items null");
        assert!(list.items.is_empty());

        let json = r#"{"items": [{"metadata": null, "usage": null, "containers": null}]}"#;
        let list: RawList<RawPodMetrics> = serde_json::from_str(json).expect("champs nuls");
        assert_eq!(list.items.len(), 1);
        assert!(list.items[0].metadata.name.is_empty());
        assert!(list.items[0].containers.is_empty());

        let json = r#"{"items": [{"usage": {"cpu": null, "memory": null}}]}"#;
        let list: RawList<RawNodeMetrics> = serde_json::from_str(json).expect("usage nul");
        assert_eq!(list.items[0].usage.memory_bytes(), 0);
    }

    #[test]
    fn liste_vide_toleree() {
        let list: RawList<RawNodeMetrics> = serde_json::from_str("{}").expect("objet vide");
        assert!(list.items.is_empty());
    }

    #[test]
    fn validation_namespace() {
        assert!(is_valid_namespace("default"));
        assert!(is_valid_namespace("kube-system"));
        assert!(is_valid_namespace("a.b-1"));
        assert!(!is_valid_namespace(""));
        assert!(!is_valid_namespace("Prod"));
        assert!(!is_valid_namespace("../etc"));
        assert!(!is_valid_namespace("a b"));
    }

    #[test]
    fn avertissements_tronques() {
        let mut warnings = Vec::new();
        for i in 0..100 {
            push_warning(&mut warnings, format!("alerte {i}"));
        }
        assert_eq!(warnings.len(), MAX_WARNINGS + 1);
        assert!(warnings.last().expect("dernier").contains("tronqués"));
    }
}
