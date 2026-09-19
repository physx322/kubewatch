//! Opérations CRUD génériques sur les ressources Kubernetes.
//!
//! Tout passe par l'API dynamique (`Api<DynamicObject>`) obtenue depuis le
//! [`ClusterHandle`], ce qui permet de manipuler aussi bien les types natifs
//! que les CRD découvertes à l'exécution.

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use k8s_openapi::api::apps::v1::{ControllerRevision, ReplicaSet};
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::api::{
    Api, DeleteParams, DynamicObject, EvictParams, ListParams, Patch, PatchParams, PostParams,
    PropagationPolicy, ResourceExt,
};
use serde_json::{Map, Value};

use crate::cluster::ClusterHandle;
use crate::error::{Error, Result};
use crate::model::{
    ContainerInfo, ListOptions, ObjectListPage, ObjectSummary, OwnerRef, ResourceKind,
    ResourceRef,
};

/// Gestionnaire de champs utilisé pour tous les patches émis par KubeWatch.
pub const FIELD_MANAGER: &str = "kubewatch";
/// Annotation posée sur le modèle de pod lors d'un redémarrage déclenché ici.
pub const RESTART_ANNOTATION: &str = "kubewatch.io/restartedAt";
/// Annotation de révision posée par le contrôleur de déploiement.
const REVISION_ANNOTATION: &str = "deployment.kubernetes.io/revision";
/// Annotation volumineuse et inutile dans les listes : on la retire des résumés.
const LAST_APPLIED_ANNOTATION: &str = "kubectl.kubernetes.io/last-applied-configuration";

// ---------------------------------------------------------------------------
// Helpers JSON (publics : réutilisés par `apply.rs` et `events.rs`)
// ---------------------------------------------------------------------------

/// Descend dans un objet JSON en suivant un chemin de clés.
/// Renvoie `None` si le chemin n'existe pas ou pointe sur `null`.
pub fn dig<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for key in path {
        cur = cur.get(*key)?;
    }
    if cur.is_null() {
        None
    } else {
        Some(cur)
    }
}

/// Lit une chaîne à un chemin donné.
pub fn dig_str<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    dig(v, path)?.as_str()
}

/// Lit un entier à un chemin donné.
pub fn dig_i64(v: &Value, path: &[&str]) -> Option<i64> {
    dig(v, path)?.as_i64()
}

/// Lit un booléen à un chemin donné.
pub fn dig_bool(v: &Value, path: &[&str]) -> Option<bool> {
    dig(v, path)?.as_bool()
}

/// Lit un tableau à un chemin donné (tranche vide si absent).
pub fn dig_arr<'a>(v: &'a Value, path: &[&str]) -> &'a [Value] {
    match dig(v, path).and_then(|x| x.as_array()) {
        Some(a) => a.as_slice(),
        None => &[],
    }
}

/// Convertit un horodatage RFC 3339 en `DateTime<Utc>`.
pub fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Convertit un `Timestamp` jiff (k8s-openapi 0.28) en `DateTime<Utc>` chrono.
pub fn jiff_to_chrono(ts: k8s_openapi::jiff::Timestamp) -> Option<DateTime<Utc>> {
    let nanos = ts.subsec_nanosecond();
    let nanos = if nanos < 0 { 0u32 } else { nanos as u32 };
    DateTime::<Utc>::from_timestamp(ts.as_second(), nanos)
}

/// `apiVersion` complet pour un groupe/version donné.
pub fn api_version_of(group: &str, version: &str) -> String {
    if group.is_empty() {
        version.to_string()
    } else {
        format!("{group}/{version}")
    }
}

/// Découpe un `apiVersion` en `(group, version)`.
pub fn split_api_version(api_version: &str) -> (String, String) {
    match api_version.split_once('/') {
        Some((g, v)) => (g.to_string(), v.to_string()),
        None => (String::new(), api_version.to_string()),
    }
}

fn map_of(v: Option<&Value>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(Value::Object(m)) = v {
        for (k, val) in m {
            let s = match val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out.insert(k.clone(), s);
        }
    }
    out
}

fn count_keys(v: &Value, path: &[&str]) -> usize {
    dig(v, path)
        .and_then(|x| x.as_object())
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Condition d'un objet Kubernetes (`.status.conditions[]`).
struct Cond {
    status: String,
    reason: String,
}

fn find_condition(obj: &Value, kind: &str) -> Option<Cond> {
    dig_arr(obj, &["status", "conditions"])
        .iter()
        .find(|c| dig_str(c, &["type"]) == Some(kind))
        .map(|c| Cond {
            status: dig_str(c, &["status"]).unwrap_or_default().to_string(),
            reason: dig_str(c, &["reason"]).unwrap_or_default().to_string(),
        })
}

fn images_at(obj: &Value, path: &[&str]) -> Vec<String> {
    dig_arr(obj, path)
        .iter()
        .filter_map(|c| c.get("image").and_then(|i| i.as_str()))
        .map(|s| s.to_string())
        .collect()
}

/// Propriétaires déclarés dans `metadata.ownerReferences`.
fn owners_of(obj: &Value) -> Vec<OwnerRef> {
    dig_arr(obj, &["metadata", "ownerReferences"])
        .iter()
        .filter_map(|o| {
            let kind = o.get("kind")?.as_str()?;
            let name = o.get("name")?.as_str()?;
            Some(OwnerRef {
                kind: kind.to_string(),
                name: name.to_string(),
                uid: o.get("uid").and_then(|u| u.as_str()).map(str::to_string),
                controller: o
                    .get("controller")
                    .and_then(|c| c.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// Ajoute une valeur non vide à la liste, sans doublon.
fn push_unique(list: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !list.iter().any(|x| x == value) {
        list.push(value.to_string());
    }
}

/// Objets référencés par un `PodSpec` : volumes persistants, ConfigMaps, Secrets.
#[derive(Debug, Default)]
struct PodDeps {
    claims: Vec<String>,
    config_maps: Vec<String>,
    secrets: Vec<String>,
}

/// Relève tout ce qu'un pod monte ou injecte : volumes (y compris projetés),
/// `envFrom` et `valueFrom` de chaque conteneur, conteneurs d'init compris.
fn pod_dependencies(obj: &Value) -> PodDeps {
    let mut d = PodDeps::default();
    for v in dig_arr(obj, &["spec", "volumes"]) {
        if let Some(c) = dig_str(v, &["persistentVolumeClaim", "claimName"]) {
            push_unique(&mut d.claims, c);
        }
        if let Some(c) = dig_str(v, &["configMap", "name"]) {
            push_unique(&mut d.config_maps, c);
        }
        if let Some(s) = dig_str(v, &["secret", "secretName"]) {
            push_unique(&mut d.secrets, s);
        }
        for src in dig_arr(v, &["projected", "sources"]) {
            if let Some(c) = dig_str(src, &["configMap", "name"]) {
                push_unique(&mut d.config_maps, c);
            }
            if let Some(s) = dig_str(src, &["secret", "name"]) {
                push_unique(&mut d.secrets, s);
            }
        }
    }
    // Conteneurs d'init d'abord : c'est l'ordre dans lequel ils s'exécutent.
    for path in [&["spec", "initContainers"][..], &["spec", "containers"][..]] {
        for c in dig_arr(obj, path) {
            for e in dig_arr(c, &["envFrom"]) {
                if let Some(n) = dig_str(e, &["configMapRef", "name"]) {
                    push_unique(&mut d.config_maps, n);
                }
                if let Some(n) = dig_str(e, &["secretRef", "name"]) {
                    push_unique(&mut d.secrets, n);
                }
            }
            for e in dig_arr(c, &["env"]) {
                if let Some(n) = dig_str(e, &["valueFrom", "configMapKeyRef", "name"]) {
                    push_unique(&mut d.config_maps, n);
                }
                if let Some(n) = dig_str(e, &["valueFrom", "secretKeyRef", "name"]) {
                    push_unique(&mut d.secrets, n);
                }
            }
        }
    }
    d
}

/// Chemin JSON du `PodSpec` interne à un objet, s'il en possède un.
fn pod_spec_path(v: &Value) -> Option<Vec<&'static str>> {
    if dig(v, &["spec", "template", "spec", "containers"]).is_some() {
        return Some(vec!["spec", "template", "spec"]);
    }
    if dig(
        v,
        &["spec", "jobTemplate", "spec", "template", "spec", "containers"],
    )
    .is_some()
    {
        return Some(vec!["spec", "jobTemplate", "spec", "template", "spec"]);
    }
    if dig(v, &["spec", "containers"]).is_some() {
        return Some(vec!["spec"]);
    }
    None
}

/// Durée lisible (style `kubectl`) à partir d'un nombre de secondes.
fn human_duration(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{}s", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}j{}h", s / 86_400, (s % 86_400) / 3600)
    }
}

// ---------------------------------------------------------------------------
// Résolution des types et construction des API dynamiques
// ---------------------------------------------------------------------------

/// Retrouve le [`ResourceKind`] correspondant à une référence.
fn resolve_ref_kind(h: &ClusterHandle, r: &ResourceRef) -> Result<ResourceKind> {
    {
        let catalog = h.catalog.read();
        let exact = catalog.kinds.iter().find(|k| {
            k.kind == r.kind
                && k.group == r.group
                && (r.version.is_empty() || k.version == r.version)
        });
        if let Some(k) = exact {
            return Ok(k.clone());
        }
        let by_group = catalog
            .kinds
            .iter()
            .find(|k| k.kind.eq_ignore_ascii_case(&r.kind) && k.group == r.group);
        if let Some(k) = by_group {
            return Ok(k.clone());
        }
        let by_plural = catalog
            .kinds
            .iter()
            .find(|k| !r.plural.is_empty() && k.plural == r.plural && k.group == r.group);
        if let Some(k) = by_plural {
            return Ok(k.clone());
        }
    }
    if !r.kind.is_empty() {
        return h.resolve_kind(&r.kind);
    }
    if !r.plural.is_empty() {
        return h.resolve_kind(&r.plural);
    }
    Err(Error::NotFound(
        "référence de ressource sans type ni pluriel".to_string(),
    ))
}

/// Construit l'API dynamique pour un type, en appliquant le namespace par défaut.
fn api_for_kind(
    h: &ClusterHandle,
    kind: &ResourceKind,
    namespace: Option<&str>,
) -> Api<DynamicObject> {
    if kind.namespaced {
        let ns = namespace
            .filter(|s| !s.is_empty())
            .unwrap_or(h.default_namespace.as_str());
        h.api_for(kind, Some(ns))
    } else {
        h.api_for(kind, None)
    }
}

fn api_for_ref(h: &ClusterHandle, r: &ResourceRef) -> Result<(ResourceKind, Api<DynamicObject>)> {
    if r.name.trim().is_empty() {
        return Err(Error::Invalid(
            "le nom de la ressource est obligatoire".to_string(),
        ));
    }
    let kind = resolve_ref_kind(h, r)?;
    let api = api_for_kind(h, &kind, r.namespace.as_deref());
    Ok((kind, api))
}

fn namespace_of(h: &ClusterHandle, kind: &ResourceKind, r: &ResourceRef) -> Option<String> {
    if !kind.namespaced {
        return None;
    }
    Some(
        r.namespace
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| h.default_namespace.clone()),
    )
}

/// Vrai si l'erreur indique l'absence de la sous-ressource visée.
fn subresource_missing(e: &kube::Error) -> bool {
    matches!(e, kube::Error::Api(s) if s.code == 404 || s.code == 405 || s.code == 400)
}

// ---------------------------------------------------------------------------
// Lecture
// ---------------------------------------------------------------------------

/// Liste paginée d'un type de ressource.
pub async fn list(
    h: &ClusterHandle,
    kind: &ResourceKind,
    opts: &ListOptions,
) -> Result<ObjectListPage> {
    let ns = if kind.namespaced {
        opts.namespace.as_deref().filter(|s| !s.is_empty())
    } else {
        None
    };
    let api = h.api_for(kind, ns);

    let mut lp = ListParams::default();
    if let Some(sel) = opts.label_selector.as_deref().filter(|s| !s.is_empty()) {
        lp = lp.labels(sel);
    }
    if let Some(sel) = opts.field_selector.as_deref().filter(|s| !s.is_empty()) {
        lp = lp.fields(sel);
    }
    if let Some(limit) = opts.limit.filter(|n| *n > 0) {
        lp = lp.limit(limit);
    }
    if let Some(tok) = opts.continue_token.as_deref().filter(|s| !s.is_empty()) {
        lp = lp.continue_token(tok);
    }

    let page = api.list(&lp).await?;
    let api_version = api_version_of(&kind.group, &kind.version);
    let mut items = Vec::with_capacity(page.items.len());
    for obj in &page.items {
        let raw = serde_json::to_value(obj)?;
        items.push(summarize(&kind.kind, &api_version, &raw));
    }

    Ok(ObjectListPage {
        items,
        continue_token: page
            .metadata
            .continue_
            .clone()
            .filter(|s| !s.trim().is_empty()),
        remaining: page.metadata.remaining_item_count,
    })
}

/// Objet brut (JSON) tel que renvoyé par l'API serveur.
pub async fn get_raw(h: &ClusterHandle, r: &ResourceRef) -> Result<Value> {
    let (kind, api) = api_for_ref(h, r)?;
    let obj = api.get(&r.name).await?;
    let mut raw = serde_json::to_value(&obj)?;
    if let Some(m) = raw.as_object_mut() {
        m.entry("apiVersion").or_insert_with(|| {
            Value::String(api_version_of(&kind.group, &kind.version))
        });
        m.entry("kind")
            .or_insert_with(|| Value::String(kind.kind.clone()));
    }
    Ok(raw)
}

/// Objet au format YAML, débarrassé des `managedFields` (illisibles).
pub async fn get_yaml(h: &ClusterHandle, r: &ResourceRef) -> Result<String> {
    let mut raw = get_raw(h, r).await?;
    if let Some(md) = raw.get_mut("metadata").and_then(|m| m.as_object_mut()) {
        md.remove("managedFields");
    }
    serde_yaml_ng::to_string(&raw).map_err(|e| Error::Yaml(format!("sérialisation YAML : {e}")))
}

/// Détail des conteneurs d'un pod (init compris).
pub async fn containers(h: &ClusterHandle, pod: &ResourceRef) -> Result<Vec<ContainerInfo>> {
    let raw = get_raw(h, pod).await?;
    Ok(containers_from_pod(&raw))
}

fn container_state(status: Option<&Value>) -> String {
    let Some(st) = status else {
        return "Unknown".to_string();
    };
    if let Some(w) = dig(st, &["state", "waiting"]) {
        return w
            .get("reason")
            .and_then(|x| x.as_str())
            .unwrap_or("Waiting")
            .to_string();
    }
    if let Some(t) = dig(st, &["state", "terminated"]) {
        return t
            .get("reason")
            .and_then(|x| x.as_str())
            .unwrap_or("Terminated")
            .to_string();
    }
    if dig(st, &["state", "running"]).is_some() {
        return "Running".to_string();
    }
    "Unknown".to_string()
}

fn containers_from_pod(raw: &Value) -> Vec<ContainerInfo> {
    let mut out = Vec::new();
    let statuses = dig_arr(raw, &["status", "containerStatuses"]);
    let init_statuses = dig_arr(raw, &["status", "initContainerStatuses"]);

    fn push(
        specs: &[Value],
        statuses: &[Value],
        init: bool,
        out: &mut Vec<ContainerInfo>,
    ) {
        for c in specs {
            let name = c.get("name").and_then(|n| n.as_str()).unwrap_or_default();
            let st = statuses
                .iter()
                .find(|s| s.get("name").and_then(|n| n.as_str()) == Some(name));
            out.push(ContainerInfo {
                name: name.to_string(),
                image: st
                    .and_then(|s| s.get("image"))
                    .and_then(|i| i.as_str())
                    .or_else(|| c.get("image").and_then(|i| i.as_str()))
                    .unwrap_or_default()
                    .to_string(),
                ready: st
                    .and_then(|s| s.get("ready"))
                    .and_then(|r| r.as_bool())
                    .unwrap_or(false),
                restart_count: st
                    .and_then(|s| s.get("restartCount"))
                    .and_then(|r| r.as_i64())
                    .unwrap_or(0),
                state: container_state(st),
                init,
            });
        }
    }

    push(
        dig_arr(raw, &["spec", "initContainers"]),
        init_statuses,
        true,
        &mut out,
    );
    push(
        dig_arr(raw, &["spec", "containers"]),
        statuses,
        false,
        &mut out,
    );
    out
}

// ---------------------------------------------------------------------------
// Écriture
// ---------------------------------------------------------------------------

/// Supprime une ressource. `propagation` vaut `Foreground`, `Background` ou `Orphan`.
pub async fn delete(h: &ClusterHandle, r: &ResourceRef, propagation: Option<&str>) -> Result<()> {
    let (_, api) = api_for_ref(h, r)?;
    let mut dp = DeleteParams::default();
    if let Some(p) = propagation.filter(|s| !s.is_empty()) {
        dp.propagation_policy = Some(match p.to_ascii_lowercase().as_str() {
            "foreground" => PropagationPolicy::Foreground,
            "background" => PropagationPolicy::Background,
            "orphan" => PropagationPolicy::Orphan,
            other => {
                return Err(Error::Invalid(format!(
                    "politique de propagation inconnue : « {other} » (attendu : foreground, background ou orphan)"
                )))
            }
        });
    }
    api.delete(&r.name, &dp).await?;
    Ok(())
}

/// Change le nombre de répliques, via la sous-ressource `scale` si elle existe.
pub async fn scale(h: &ClusterHandle, r: &ResourceRef, replicas: i32) -> Result<()> {
    if replicas < 0 {
        return Err(Error::Invalid(
            "le nombre de répliques doit être positif ou nul".to_string(),
        ));
    }
    let (_, api) = api_for_ref(h, r)?;
    let body = serde_json::json!({ "spec": { "replicas": replicas } });
    let pp = PatchParams::apply(FIELD_MANAGER);
    match api.patch_scale(&r.name, &pp, &Patch::Merge(&body)).await {
        Ok(_) => Ok(()),
        Err(e) if subresource_missing(&e) => {
            api.patch(&r.name, &pp, &Patch::Merge(&body)).await?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Redémarre un déploiement en modifiant une annotation du modèle de pod.
pub async fn restart(h: &ClusterHandle, r: &ResourceRef) -> Result<()> {
    let (kind, api) = api_for_ref(h, r)?;
    if matches!(kind.kind.as_str(), "Pod" | "Job" | "CronJob") {
        return Err(Error::Unsupported(format!(
            "le redémarrage progressif ne s'applique pas au type « {} »",
            kind.kind
        )));
    }
    let raw = serde_json::to_value(&api.get(&r.name).await?)?;
    if dig(&raw, &["spec", "template", "spec"]).is_none() {
        return Err(Error::Unsupported(format!(
            "« {} » ne possède pas de modèle de pod : redémarrage impossible",
            kind.kind
        )));
    }

    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut annotations = Map::new();
    annotations.insert(RESTART_ANNOTATION.to_string(), Value::String(now));
    let patch = nest(
        &["spec", "template", "metadata"],
        Value::Object({
            let mut m = Map::new();
            m.insert("annotations".to_string(), Value::Object(annotations));
            m
        }),
    );
    api.patch(
        &r.name,
        &PatchParams::apply(FIELD_MANAGER),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

/// Imbrique `leaf` sous le chemin donné : `["a","b"] -> {"a":{"b":leaf}}`.
fn nest(path: &[&str], leaf: Value) -> Value {
    let mut node = leaf;
    for key in path.iter().rev() {
        let mut m = Map::new();
        m.insert((*key).to_string(), node);
        node = Value::Object(m);
    }
    node
}

/// Remplace intégralement une ressource à partir d'un manifeste YAML.
pub async fn replace_yaml(h: &ClusterHandle, r: &ResourceRef, yaml: &str) -> Result<()> {
    let (kind, api) = api_for_ref(h, r)?;
    let docs = crate::apply::split_documents(yaml)?;
    if docs.len() != 1 {
        return Err(Error::Invalid(format!(
            "le remplacement attend exactement un document YAML, {} reçu(s)",
            docs.len()
        )));
    }
    let mut doc = docs.into_iter().next().unwrap_or(Value::Null);

    let doc_name = dig_str(&doc, &["metadata", "name"]).unwrap_or_default().to_string();
    if !doc_name.is_empty() && doc_name != r.name {
        return Err(Error::Invalid(format!(
            "le manifeste décrit « {doc_name} » alors que la cible est « {} »",
            r.name
        )));
    }

    let live = api.get(&r.name).await?;
    let live_raw = serde_json::to_value(&live)?;

    {
        let obj = doc
            .as_object_mut()
            .ok_or_else(|| Error::Invalid("le manifeste n'est pas un objet YAML".to_string()))?;
        let md = obj
            .entry("metadata")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| Error::Invalid("« metadata » doit être un objet".to_string()))?;
        md.insert("name".to_string(), Value::String(r.name.clone()));
        if let Some(ns) = namespace_of(h, &kind, r) {
            md.insert("namespace".to_string(), Value::String(ns));
        }
        // La version de ressource est indispensable pour un remplacement sûr.
        if md.get("resourceVersion").and_then(|v| v.as_str()).is_none() {
            if let Some(rv) = dig_str(&live_raw, &["metadata", "resourceVersion"]) {
                md.insert("resourceVersion".to_string(), Value::String(rv.to_string()));
            }
        }
        md.remove("managedFields");
    }

    let obj: DynamicObject = serde_json::from_value(doc)?;
    let pp = PostParams {
        dry_run: false,
        field_manager: Some(FIELD_MANAGER.to_string()),
    };
    api.replace(&r.name, &pp, &obj).await?;
    Ok(())
}

/// Change l'image d'un conteneur (ou du seul conteneur présent).
pub async fn set_image(
    h: &ClusterHandle,
    r: &ResourceRef,
    container: Option<&str>,
    image: &str,
) -> Result<()> {
    let image = image.trim();
    if image.is_empty() {
        return Err(Error::Invalid(
            "l'image cible ne peut pas être vide".to_string(),
        ));
    }
    let (kind, api) = api_for_ref(h, r)?;
    let raw = serde_json::to_value(&api.get(&r.name).await?)?;
    let path = pod_spec_path(&raw).ok_or_else(|| {
        Error::Unsupported(format!(
            "« {} » ne contient aucun conteneur modifiable",
            kind.kind
        ))
    })?;

    let mut mains: Vec<Value> = dig(&raw, &path)
        .and_then(|s| s.get("containers"))
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    let mut inits: Vec<Value> = dig(&raw, &path)
        .and_then(|s| s.get("initContainers"))
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

    let names: Vec<String> = mains
        .iter()
        .chain(inits.iter())
        .filter_map(|c| c.get("name").and_then(|n| n.as_str()))
        .map(|s| s.to_string())
        .collect();
    if names.is_empty() {
        return Err(Error::Invalid(format!(
            "aucun conteneur trouvé sur « {} »",
            r.name
        )));
    }

    let target = match container.map(str::trim).filter(|s| !s.is_empty()) {
        Some(c) => {
            if !names.iter().any(|n| n == c) {
                return Err(Error::Invalid(format!(
                    "conteneur « {c} » introuvable ; conteneurs disponibles : {}",
                    names.join(", ")
                )));
            }
            c.to_string()
        }
        None => {
            if mains.len() == 1 {
                names
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "".to_string())
            } else {
                return Err(Error::Invalid(format!(
                    "plusieurs conteneurs présents : précisez lequel parmi {}",
                    names.join(", ")
                )));
            }
        }
    };

    let mut touched_main = false;
    for c in mains.iter_mut() {
        if c.get("name").and_then(|n| n.as_str()) == Some(target.as_str()) {
            if let Some(o) = c.as_object_mut() {
                o.insert("image".to_string(), Value::String(image.to_string()));
                touched_main = true;
            }
        }
    }
    let mut touched_init = false;
    if !touched_main {
        for c in inits.iter_mut() {
            if c.get("name").and_then(|n| n.as_str()) == Some(target.as_str()) {
                if let Some(o) = c.as_object_mut() {
                    o.insert("image".to_string(), Value::String(image.to_string()));
                    touched_init = true;
                }
            }
        }
    }
    if !touched_main && !touched_init {
        return Err(Error::Invalid(format!(
            "conteneur « {target} » introuvable ; conteneurs disponibles : {}",
            names.join(", ")
        )));
    }

    // Un patch de fusion JSON remplace les tableaux : on renvoie donc la liste
    // complète, image mise à jour incluse, afin de ne rien perdre.
    let mut leaf = Map::new();
    if touched_main {
        leaf.insert("containers".to_string(), Value::Array(mains));
    }
    if touched_init {
        leaf.insert("initContainers".to_string(), Value::Array(inits));
    }
    let patch = nest(&path, Value::Object(leaf));

    api.patch(
        &r.name,
        &PatchParams::apply(FIELD_MANAGER),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

/// Retour à la révision précédente.
pub async fn rollback(h: &ClusterHandle, r: &ResourceRef) -> Result<()> {
    let (kind, api) = api_for_ref(h, r)?;
    let ns = namespace_of(h, &kind, r).ok_or_else(|| {
        Error::Unsupported(format!(
            "le retour arrière ne s'applique qu'aux ressources de namespace (« {} »)",
            kind.kind
        ))
    })?;
    match kind.kind.as_str() {
        "Deployment" => rollback_deployment(h, &api, &ns, &r.name).await,
        "DaemonSet" | "StatefulSet" => rollback_controller_revision(h, &api, &ns, &r.name).await,
        other => Err(Error::Unsupported(format!(
            "le retour arrière n'est pas pris en charge pour le type « {other} »"
        ))),
    }
}

fn revision_of(v: &Value) -> i64 {
    dig(v, &["metadata", "annotations"])
        .and_then(|a| a.get(REVISION_ANNOTATION))
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
}

async fn rollback_deployment(
    h: &ClusterHandle,
    api: &Api<DynamicObject>,
    ns: &str,
    name: &str,
) -> Result<()> {
    let live = api.get(name).await?;
    let mut live_raw = serde_json::to_value(&live)?;
    let uid = dig_str(&live_raw, &["metadata", "uid"])
        .unwrap_or_default()
        .to_string();
    let current_rev = dig(&live_raw, &["metadata", "annotations"])
        .and_then(|a| a.get(REVISION_ANNOTATION))
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse::<i64>().ok());

    let rs_api: Api<ReplicaSet> = Api::namespaced(h.client.clone(), ns);
    let list = rs_api.list(&ListParams::default()).await?;
    let mut owned: Vec<(i64, Value)> = Vec::new();
    for rs in &list.items {
        let raw = serde_json::to_value(rs)?;
        let is_owned = dig_arr(&raw, &["metadata", "ownerReferences"])
            .iter()
            .any(|o| o.get("uid").and_then(|x| x.as_str()) == Some(uid.as_str()));
        if !is_owned || uid.is_empty() {
            continue;
        }
        let rev = revision_of(&raw);
        owned.push((rev, raw));
    }
    owned.sort_by_key(|(rev, _)| *rev);

    let target = match current_rev {
        Some(cur) => owned.iter().rev().find(|(rev, _)| *rev < cur),
        None => {
            if owned.len() >= 2 {
                owned.get(owned.len() - 2)
            } else {
                None
            }
        }
    };
    let (_, rs) = target.ok_or_else(|| {
        Error::Unsupported(format!(
            "aucune révision antérieure conservée pour le déploiement « {name} »"
        ))
    })?;

    let mut template = dig(rs, &["spec", "template"]).cloned().ok_or_else(|| {
        Error::Invalid("le ReplicaSet de la révision précédente n'a pas de modèle de pod".to_string())
    })?;
    if let Some(labels) = template
        .get_mut("metadata")
        .and_then(|m| m.get_mut("labels"))
        .and_then(|l| l.as_object_mut())
    {
        labels.remove("pod-template-hash");
    }

    {
        let spec = live_raw
            .get_mut("spec")
            .and_then(|s| s.as_object_mut())
            .ok_or_else(|| Error::Invalid("déploiement sans « spec »".to_string()))?;
        spec.insert("template".to_string(), template);
    }
    if let Some(md) = live_raw.get_mut("metadata").and_then(|m| m.as_object_mut()) {
        md.remove("managedFields");
    }

    let obj: DynamicObject = serde_json::from_value(live_raw)?;
    let pp = PostParams {
        dry_run: false,
        field_manager: Some(FIELD_MANAGER.to_string()),
    };
    api.replace(name, &pp, &obj).await?;
    Ok(())
}

async fn rollback_controller_revision(
    h: &ClusterHandle,
    api: &Api<DynamicObject>,
    ns: &str,
    name: &str,
) -> Result<()> {
    let live = api.get(name).await?;
    let live_raw = serde_json::to_value(&live)?;
    let uid = dig_str(&live_raw, &["metadata", "uid"])
        .unwrap_or_default()
        .to_string();
    if uid.is_empty() {
        return Err(Error::Invalid(
            "impossible de déterminer l'identifiant de la ressource".to_string(),
        ));
    }
    let current = dig_str(&live_raw, &["status", "updateRevision"])
        .or_else(|| dig_str(&live_raw, &["status", "currentRevision"]))
        .unwrap_or_default()
        .to_string();

    let cr_api: Api<ControllerRevision> = Api::namespaced(h.client.clone(), ns);
    let list = cr_api.list(&ListParams::default()).await?;
    let mut owned: Vec<(i64, String, Value)> = Vec::new();
    for cr in &list.items {
        let is_owned = cr
            .owner_references()
            .iter()
            .any(|o| o.uid.as_str() == uid.as_str());
        if !is_owned {
            continue;
        }
        let Some(data) = cr.data.as_ref().map(|d| d.0.clone()) else {
            continue;
        };
        owned.push((cr.revision, cr.name_any(), data));
    }
    owned.sort_by_key(|(rev, _, _)| *rev);

    let target = if !current.is_empty() {
        owned.iter().rev().find(|(_, n, _)| n != &current)
    } else if owned.len() >= 2 {
        owned.get(owned.len() - 2)
    } else {
        None
    };
    let (_, _, data) = target.ok_or_else(|| {
        Error::Unsupported(format!(
            "aucune ControllerRevision antérieure disponible pour « {name} »"
        ))
    })?;

    // `kubectl rollout undo` applique la donnée de révision comme patch
    // stratégique : on reproduit exactement ce comportement.
    api.patch(
        name,
        &PatchParams::apply(FIELD_MANAGER),
        &Patch::Strategic(data),
    )
    .await?;
    Ok(())
}

/// Rend un nœud non ordonnançable (`on = true`) ou de nouveau ordonnançable.
pub async fn cordon(h: &ClusterHandle, node: &str, on: bool) -> Result<()> {
    if node.trim().is_empty() {
        return Err(Error::Invalid("nom de nœud vide".to_string()));
    }
    let nodes: Api<Node> = Api::all(h.client.clone());
    let patch = serde_json::json!({ "spec": { "unschedulable": on } });
    nodes
        .patch(
            node,
            &PatchParams::apply(FIELD_MANAGER),
            &Patch::Merge(&patch),
        )
        .await?;
    Ok(())
}

/// Vide un nœud : cordon puis éviction des pods éligibles.
/// Renvoie la liste `namespace/nom` des pods effectivement évincés.
pub async fn drain(
    h: &ClusterHandle,
    node: &str,
    grace_seconds: Option<u32>,
) -> Result<Vec<String>> {
    cordon(h, node, true).await?;

    let pods: Api<Pod> = Api::all(h.client.clone());
    let lp = ListParams::default().fields(&format!("spec.nodeName={node}"));
    let list = pods.list(&lp).await?;

    let mut evicted = Vec::new();
    for pod in &list.items {
        let name = pod.name_any();
        let ns = pod
            .namespace()
            .unwrap_or_else(|| h.default_namespace.clone());

        if pod.metadata.deletion_timestamp.is_some() {
            continue;
        }
        // Pods miroir (statiques) : gérés par le kubelet, non évinçables.
        if pod.annotations().contains_key("kubernetes.io/config.mirror") {
            continue;
        }
        // Pods de DaemonSet : recréés immédiatement, on les ignore.
        if pod
            .owner_references()
            .iter()
            .any(|o| o.kind.as_str() == "DaemonSet")
        {
            continue;
        }
        if let Some(status) = pod.status.as_ref() {
            if matches!(status.phase.as_deref(), Some("Succeeded") | Some("Failed")) {
                continue;
            }
        }

        let api: Api<Pod> = Api::namespaced(h.client.clone(), &ns);
        let ep = EvictParams {
            delete_options: Some(DeleteParams {
                grace_period_seconds: grace_seconds,
                ..Default::default()
            }),
            post_options: PostParams::default(),
        };
        match api.evict(&name, &ep).await {
            Ok(_) => evicted.push(format!("{ns}/{name}")),
            Err(e) => {
                tracing::warn!(
                    pod = %format!("{ns}/{name}"),
                    erreur = %e,
                    "éviction refusée (budget de disruption ou webhook)"
                );
            }
        }
    }
    Ok(evicted)
}

// ---------------------------------------------------------------------------
// Résumés par type
// ---------------------------------------------------------------------------

/// Construit une ligne de tableau exploitable pour n'importe quel objet.
pub fn summarize(kind: &str, api_version: &str, obj: &Value) -> ObjectSummary {
    let created_at = dig_str(obj, &["metadata", "creationTimestamp"]).and_then(parse_time);
    let age_seconds = created_at.map(|c| (Utc::now() - c).num_seconds().max(0));
    let mut annotations = map_of(dig(obj, &["metadata", "annotations"]));
    annotations.remove(LAST_APPLIED_ANNOTATION);
    let terminating = dig(obj, &["metadata", "deletionTimestamp"]).is_some();

    let mut s = ObjectSummary {
        name: dig_str(obj, &["metadata", "name"])
            .unwrap_or_default()
            .to_string(),
        namespace: dig_str(obj, &["metadata", "namespace"]).map(|x| x.to_string()),
        kind: kind.to_string(),
        api_version: api_version.to_string(),
        uid: dig_str(obj, &["metadata", "uid"]).map(|x| x.to_string()),
        created_at,
        age_seconds,
        status: String::new(),
        ready: None,
        restarts: None,
        node: None,
        images: Vec::new(),
        labels: map_of(dig(obj, &["metadata", "labels"])),
        annotations,
        owners: owners_of(obj),
        extra: Map::new(),
    };

    match kind {
        "Pod" => summarize_pod(obj, terminating, &mut s),
        "Deployment" | "StatefulSet" | "ReplicaSet" | "ReplicationController" => {
            summarize_replicated(obj, terminating, &mut s)
        }
        "DaemonSet" => summarize_daemonset(obj, terminating, &mut s),
        "Job" => summarize_job(obj, &mut s),
        "CronJob" => summarize_cronjob(obj, &mut s),
        "Service" => summarize_service(obj, &mut s),
        "Ingress" => summarize_ingress(obj, &mut s),
        "Node" => summarize_node(obj, &mut s),
        "Namespace" => {
            s.status = dig_str(obj, &["status", "phase"])
                .unwrap_or(if terminating { "Terminating" } else { "Active" })
                .to_string();
        }
        "PersistentVolumeClaim" => summarize_pvc(obj, &mut s),
        "PersistentVolume" => summarize_pv(obj, &mut s),
        "Secret" => summarize_secret(obj, &mut s),
        "ConfigMap" => summarize_configmap(obj, &mut s),
        _ => summarize_generic(obj, &mut s),
    }

    if s.images.is_empty() {
        s.images = images_at(obj, &["spec", "template", "spec", "containers"]);
        if s.images.is_empty() {
            s.images = images_at(obj, &["spec", "containers"]);
        }
        if s.images.is_empty() {
            s.images = images_at(
                obj,
                &["spec", "jobTemplate", "spec", "template", "spec", "containers"],
            );
        }
    }
    if terminating && !s.status.starts_with("Terminating") && kind != "Namespace" {
        s.status = "Terminating".to_string();
    }
    if s.status.is_empty() {
        s.status = "Active".to_string();
    }
    s
}

fn summarize_pod(obj: &Value, terminating: bool, s: &mut ObjectSummary) {
    let spec_containers = dig_arr(obj, &["spec", "containers"]);
    let spec_inits = dig_arr(obj, &["spec", "initContainers"]);
    let statuses = dig_arr(obj, &["status", "containerStatuses"]);
    let init_statuses = dig_arr(obj, &["status", "initContainerStatuses"]);

    s.images = spec_containers
        .iter()
        .filter_map(|c| c.get("image").and_then(|i| i.as_str()))
        .map(|x| x.to_string())
        .collect();
    s.node = dig_str(obj, &["spec", "nodeName"]).map(|x| x.to_string());

    let total = if statuses.is_empty() {
        spec_containers.len()
    } else {
        statuses.len()
    };
    let ready_count = statuses
        .iter()
        .filter(|c| c.get("ready").and_then(|r| r.as_bool()).unwrap_or(false))
        .count();
    s.ready = Some(format!("{ready_count}/{total}"));
    s.restarts = Some(
        statuses
            .iter()
            .chain(init_statuses.iter())
            .filter_map(|c| c.get("restartCount").and_then(|r| r.as_i64()))
            .sum(),
    );

    let phase = dig_str(obj, &["status", "phase"]).unwrap_or("Unknown");
    let pod_reason = dig_str(obj, &["status", "reason"]).unwrap_or_default();
    let mut reason = if pod_reason.is_empty() {
        phase.to_string()
    } else {
        pod_reason.to_string()
    };

    // Phase d'initialisation (logique alignée sur le printer de kubectl).
    let init_total = spec_inits.len().max(init_statuses.len());
    let mut initializing = false;
    for (i, cs) in init_statuses.iter().enumerate() {
        let terminated = dig(cs, &["state", "terminated"]);
        let started = cs.get("started").and_then(|x| x.as_bool()).unwrap_or(false);
        let ready = cs.get("ready").and_then(|x| x.as_bool()).unwrap_or(false);
        if let Some(t) = terminated {
            if t.get("exitCode").and_then(|e| e.as_i64()) == Some(0) {
                continue;
            }
            let tr = t.get("reason").and_then(|r| r.as_str()).unwrap_or_default();
            reason = if !tr.is_empty() {
                format!("Init:{tr}")
            } else if let Some(sig) = t.get("signal").and_then(|x| x.as_i64()).filter(|v| *v != 0) {
                format!("Init:Signal:{sig}")
            } else {
                format!(
                    "Init:ExitCode:{}",
                    t.get("exitCode").and_then(|e| e.as_i64()).unwrap_or(-1)
                )
            };
            initializing = true;
            break;
        }
        // Conteneur d'init « sidecar » démarré et prêt : on continue.
        if started && ready {
            continue;
        }
        let wr = dig_str(cs, &["state", "waiting", "reason"]).unwrap_or_default();
        if !wr.is_empty() && wr != "PodInitializing" {
            reason = format!("Init:{wr}");
            initializing = true;
            break;
        }
        reason = format!("Init:{i}/{init_total}");
        initializing = true;
        break;
    }

    let initialized = find_condition(obj, "Initialized")
        .map(|c| c.status == "True")
        .unwrap_or(false);
    if !initializing || initialized {
        let mut has_running = false;
        for cs in statuses.iter().rev() {
            let waiting_reason = dig_str(cs, &["state", "waiting", "reason"]).unwrap_or_default();
            let terminated = dig(cs, &["state", "terminated"]);
            if !waiting_reason.is_empty() {
                reason = waiting_reason.to_string();
            } else if let Some(t) = terminated {
                let tr = t.get("reason").and_then(|r| r.as_str()).unwrap_or_default();
                reason = if !tr.is_empty() {
                    tr.to_string()
                } else if let Some(sig) =
                    t.get("signal").and_then(|x| x.as_i64()).filter(|v| *v != 0)
                {
                    format!("Signal:{sig}")
                } else {
                    format!(
                        "ExitCode:{}",
                        t.get("exitCode").and_then(|e| e.as_i64()).unwrap_or(-1)
                    )
                };
            } else if cs.get("ready").and_then(|r| r.as_bool()).unwrap_or(false)
                && dig(cs, &["state", "running"]).is_some()
            {
                has_running = true;
            }
        }
        if reason == "Completed" && has_running {
            let pod_ready = find_condition(obj, "Ready")
                .map(|c| c.status == "True")
                .unwrap_or(false);
            reason = if pod_ready { "Running" } else { "NotReady" }.to_string();
        }
    }

    if terminating {
        reason = if pod_reason == "NodeLost" {
            "Unknown".to_string()
        } else if !matches!(phase, "Succeeded" | "Failed") {
            "Terminating".to_string()
        } else {
            reason
        };
    }
    s.status = reason;

    let extra = &mut s.extra;
    extra.insert("phase".to_string(), Value::String(phase.to_string()));
    if let Some(ip) = dig_str(obj, &["status", "podIP"]) {
        extra.insert("podIP".to_string(), Value::String(ip.to_string()));
    }
    if let Some(ip) = dig_str(obj, &["status", "hostIP"]) {
        extra.insert("hostIP".to_string(), Value::String(ip.to_string()));
    }
    if let Some(q) = dig_str(obj, &["status", "qosClass"]) {
        extra.insert("qosClass".to_string(), Value::String(q.to_string()));
    }
    if let Some(sa) = dig_str(obj, &["spec", "serviceAccountName"]) {
        extra.insert("serviceAccount".to_string(), Value::String(sa.to_string()));
    }
    if let Some(n) = dig_str(obj, &["status", "nominatedNodeName"]) {
        extra.insert("nominatedNodeName".to_string(), Value::String(n.to_string()));
    }
    extra.insert(
        "containers".to_string(),
        Value::from(spec_containers.len() as i64),
    );

    // Dépendances du pod, pour l'écran de topologie.
    let deps = pod_dependencies(obj);
    if !deps.claims.is_empty() {
        extra.insert("volumeClaims".to_string(), Value::from(deps.claims));
    }
    if !deps.config_maps.is_empty() {
        extra.insert("configMaps".to_string(), Value::from(deps.config_maps));
    }
    if !deps.secrets.is_empty() {
        extra.insert("secrets".to_string(), Value::from(deps.secrets));
    }
}

fn summarize_replicated(obj: &Value, terminating: bool, s: &mut ObjectSummary) {
    let desired = dig_i64(obj, &["spec", "replicas"])
        .or_else(|| dig_i64(obj, &["status", "replicas"]))
        .unwrap_or(0);
    let ready = dig_i64(obj, &["status", "readyReplicas"]).unwrap_or(0);
    let updated = dig_i64(obj, &["status", "updatedReplicas"]).unwrap_or(0);
    let available = dig_i64(obj, &["status", "availableReplicas"]).unwrap_or(0);
    let current = dig_i64(obj, &["status", "replicas"]).unwrap_or(0);

    s.ready = Some(format!("{ready}/{desired}"));
    s.extra
        .insert("desired".to_string(), Value::from(desired));
    s.extra.insert("current".to_string(), Value::from(current));
    s.extra.insert("readyReplicas".to_string(), Value::from(ready));
    s.extra.insert("upToDate".to_string(), Value::from(updated));
    s.extra
        .insert("available".to_string(), Value::from(available));
    if let Some(strategy) = dig_str(obj, &["spec", "strategy", "type"]) {
        s.extra
            .insert("strategy".to_string(), Value::String(strategy.to_string()));
    }
    if let Some(rev) = dig(obj, &["metadata", "annotations"])
        .and_then(|a| a.get(REVISION_ANNOTATION))
        .and_then(|x| x.as_str())
    {
        s.extra
            .insert("revision".to_string(), Value::String(rev.to_string()));
    }

    let mut status = if terminating {
        "Terminating".to_string()
    } else if desired == 0 {
        "Scaled to zero".to_string()
    } else if ready >= desired {
        "Ready".to_string()
    } else {
        "Progressing".to_string()
    };
    if let Some(c) = find_condition(obj, "Progressing") {
        if c.status == "False" && !c.reason.is_empty() {
            status = c.reason;
        }
    }
    if let Some(c) = find_condition(obj, "ReplicaFailure") {
        if c.status == "True" {
            status = if c.reason.is_empty() {
                "ReplicaFailure".to_string()
            } else {
                format!("ReplicaFailure: {}", c.reason)
            };
        }
    }
    s.status = status;
}

fn summarize_daemonset(obj: &Value, terminating: bool, s: &mut ObjectSummary) {
    let desired = dig_i64(obj, &["status", "desiredNumberScheduled"]).unwrap_or(0);
    let current = dig_i64(obj, &["status", "currentNumberScheduled"]).unwrap_or(0);
    let ready = dig_i64(obj, &["status", "numberReady"]).unwrap_or(0);
    let updated = dig_i64(obj, &["status", "updatedNumberScheduled"]).unwrap_or(0);
    let available = dig_i64(obj, &["status", "numberAvailable"]).unwrap_or(0);
    let misscheduled = dig_i64(obj, &["status", "numberMisscheduled"]).unwrap_or(0);

    s.ready = Some(format!("{ready}/{desired}"));
    s.extra.insert("desired".to_string(), Value::from(desired));
    s.extra.insert("current".to_string(), Value::from(current));
    s.extra.insert("readyReplicas".to_string(), Value::from(ready));
    s.extra.insert("upToDate".to_string(), Value::from(updated));
    s.extra
        .insert("available".to_string(), Value::from(available));
    s.extra
        .insert("misscheduled".to_string(), Value::from(misscheduled));
    if let Some(sel) = dig(obj, &["spec", "template", "spec", "nodeSelector"]) {
        s.extra.insert("nodeSelector".to_string(), sel.clone());
    }

    s.status = if terminating {
        "Terminating".to_string()
    } else if desired == 0 {
        "Not scheduled".to_string()
    } else if ready >= desired && updated >= desired {
        "Ready".to_string()
    } else {
        "Progressing".to_string()
    };
}

fn summarize_job(obj: &Value, s: &mut ObjectSummary) {
    let succeeded = dig_i64(obj, &["status", "succeeded"]).unwrap_or(0);
    let failed = dig_i64(obj, &["status", "failed"]).unwrap_or(0);
    let active = dig_i64(obj, &["status", "active"]).unwrap_or(0);
    let completions = dig_i64(obj, &["spec", "completions"]).unwrap_or(1);

    s.ready = Some(format!("{succeeded}/{completions}"));
    s.extra
        .insert("completions".to_string(), Value::String(format!("{succeeded}/{completions}")));
    s.extra.insert("active".to_string(), Value::from(active));
    s.extra
        .insert("succeeded".to_string(), Value::from(succeeded));
    s.extra.insert("failed".to_string(), Value::from(failed));

    let start = dig_str(obj, &["status", "startTime"]).and_then(parse_time);
    let end = dig_str(obj, &["status", "completionTime"]).and_then(parse_time);
    if let Some(st) = start {
        let stop = end.unwrap_or_else(Utc::now);
        let secs = (stop - st).num_seconds().max(0);
        s.extra
            .insert("duration".to_string(), Value::String(human_duration(secs)));
        s.extra
            .insert("durationSeconds".to_string(), Value::from(secs));
    }

    s.status = if find_condition(obj, "Complete").map(|c| c.status == "True") == Some(true) {
        "Complete".to_string()
    } else if let Some(c) = find_condition(obj, "Failed").filter(|c| c.status == "True") {
        if c.reason.is_empty() {
            "Failed".to_string()
        } else {
            format!("Failed: {}", c.reason)
        }
    } else if find_condition(obj, "Suspended").map(|c| c.status == "True") == Some(true) {
        "Suspended".to_string()
    } else if active > 0 {
        "Running".to_string()
    } else {
        "Pending".to_string()
    };
}

fn summarize_cronjob(obj: &Value, s: &mut ObjectSummary) {
    let suspend = dig_bool(obj, &["spec", "suspend"]).unwrap_or(false);
    let active = dig_arr(obj, &["status", "active"]).len() as i64;
    if let Some(sched) = dig_str(obj, &["spec", "schedule"]) {
        s.extra
            .insert("schedule".to_string(), Value::String(sched.to_string()));
    }
    if let Some(tz) = dig_str(obj, &["spec", "timeZone"]) {
        s.extra
            .insert("timeZone".to_string(), Value::String(tz.to_string()));
    }
    s.extra.insert("suspend".to_string(), Value::Bool(suspend));
    s.extra.insert("active".to_string(), Value::from(active));
    if let Some(t) = dig_str(obj, &["status", "lastScheduleTime"]) {
        s.extra
            .insert("lastSchedule".to_string(), Value::String(t.to_string()));
    }
    if let Some(t) = dig_str(obj, &["status", "lastSuccessfulTime"]) {
        s.extra
            .insert("lastSuccessful".to_string(), Value::String(t.to_string()));
    }
    s.ready = Some(format!("{active} actif(s)"));
    s.status = if suspend {
        "Suspended".to_string()
    } else if active > 0 {
        "Running".to_string()
    } else {
        "Scheduled".to_string()
    };
}

fn summarize_service(obj: &Value, s: &mut ObjectSummary) {
    let svc_type = dig_str(obj, &["spec", "type"]).unwrap_or("ClusterIP");
    let cluster_ip = dig_str(obj, &["spec", "clusterIP"]).unwrap_or("None");

    let mut external: Vec<String> = dig_arr(obj, &["spec", "externalIPs"])
        .iter()
        .filter_map(|x| x.as_str())
        .map(|x| x.to_string())
        .collect();
    for ing in dig_arr(obj, &["status", "loadBalancer", "ingress"]) {
        if let Some(ip) = ing.get("ip").and_then(|x| x.as_str()) {
            external.push(ip.to_string());
        } else if let Some(h) = ing.get("hostname").and_then(|x| x.as_str()) {
            external.push(h.to_string());
        }
    }
    if let Some(en) = dig_str(obj, &["spec", "externalName"]) {
        external.push(en.to_string());
    }

    let ports: Vec<String> = dig_arr(obj, &["spec", "ports"])
        .iter()
        .map(|p| {
            let port = p.get("port").and_then(|x| x.as_i64()).unwrap_or(0);
            let proto = p.get("protocol").and_then(|x| x.as_str()).unwrap_or("TCP");
            match p.get("nodePort").and_then(|x| x.as_i64()) {
                Some(np) if np > 0 => format!("{port}:{np}/{proto}"),
                _ => format!("{port}/{proto}"),
            }
        })
        .collect();

    s.extra
        .insert("type".to_string(), Value::String(svc_type.to_string()));
    s.extra
        .insert("clusterIP".to_string(), Value::String(cluster_ip.to_string()));
    s.extra.insert(
        "externalIP".to_string(),
        Value::String(if external.is_empty() {
            "<none>".to_string()
        } else {
            external.join(",")
        }),
    );
    s.extra
        .insert("ports".to_string(), Value::String(ports.join(",")));
    if let Some(sel) = dig(obj, &["spec", "selector"]) {
        s.extra.insert("selector".to_string(), sel.clone());
    }
    s.ready = Some(format!("{} port(s)", ports.len()));

    s.status = if svc_type == "LoadBalancer" && external.is_empty() {
        "Pending".to_string()
    } else {
        svc_type.to_string()
    };
}

fn summarize_ingress(obj: &Value, s: &mut ObjectSummary) {
    let class = dig_str(obj, &["spec", "ingressClassName"])
        .map(|x| x.to_string())
        .or_else(|| {
            dig(obj, &["metadata", "annotations"])
                .and_then(|a| a.get("kubernetes.io/ingress.class"))
                .and_then(|x| x.as_str())
                .map(|x| x.to_string())
        })
        .unwrap_or_else(|| "<none>".to_string());

    let hosts: Vec<String> = dig_arr(obj, &["spec", "rules"])
        .iter()
        .filter_map(|r| r.get("host").and_then(|h| h.as_str()))
        .map(|x| x.to_string())
        .collect();

    let mut address: Vec<String> = Vec::new();
    for ing in dig_arr(obj, &["status", "loadBalancer", "ingress"]) {
        if let Some(ip) = ing.get("ip").and_then(|x| x.as_str()) {
            address.push(ip.to_string());
        } else if let Some(h) = ing.get("hostname").and_then(|x| x.as_str()) {
            address.push(h.to_string());
        }
    }
    let tls = !dig_arr(obj, &["spec", "tls"]).is_empty();

    // Services visés, pour l'écran de topologie : backend par défaut puis
    // chaque chemin. `serviceName` est la forme de l'ancien `extensions/v1beta1`.
    let mut backends: Vec<String> = Vec::new();
    if let Some(n) = dig_str(obj, &["spec", "defaultBackend", "service", "name"]) {
        push_unique(&mut backends, n);
    }
    for rule in dig_arr(obj, &["spec", "rules"]) {
        for path in dig_arr(rule, &["http", "paths"]) {
            if let Some(n) = dig_str(path, &["backend", "service", "name"])
                .or_else(|| dig_str(path, &["backend", "serviceName"]))
            {
                push_unique(&mut backends, n);
            }
        }
    }
    s.extra
        .insert("backends".to_string(), Value::from(backends));

    s.extra.insert("class".to_string(), Value::String(class));
    s.extra
        .insert("hosts".to_string(), Value::String(hosts.join(",")));
    s.extra
        .insert("address".to_string(), Value::String(address.join(",")));
    s.extra.insert(
        "ports".to_string(),
        Value::String(if tls { "80,443".to_string() } else { "80".to_string() }),
    );
    s.extra.insert("tls".to_string(), Value::Bool(tls));
    s.ready = Some(format!("{} hôte(s)", hosts.len()));
    s.status = if address.is_empty() {
        "Pending".to_string()
    } else {
        "Ready".to_string()
    };
}

fn summarize_node(obj: &Value, s: &mut ObjectSummary) {
    let mut roles: Vec<String> = Vec::new();
    for (k, v) in s.labels.clone() {
        if let Some(role) = k.strip_prefix("node-role.kubernetes.io/") {
            if !role.is_empty() {
                roles.push(role.to_string());
            }
        } else if k == "kubernetes.io/role" && !v.is_empty() {
            roles.push(v);
        }
    }
    roles.sort();
    roles.dedup();
    if roles.is_empty() {
        roles.push("<none>".to_string());
    }

    let ready_cond = find_condition(obj, "Ready");
    let mut status = match ready_cond.as_ref().map(|c| c.status.as_str()) {
        Some("True") => "Ready".to_string(),
        Some("False") => "NotReady".to_string(),
        _ => "Unknown".to_string(),
    };
    let unschedulable = dig_bool(obj, &["spec", "unschedulable"]).unwrap_or(false);
    if unschedulable {
        status.push_str(",SchedulingDisabled");
    }

    let problems: Vec<String> = dig_arr(obj, &["status", "conditions"])
        .iter()
        .filter(|c| {
            dig_str(c, &["type"]) != Some("Ready") && dig_str(c, &["status"]) == Some("True")
        })
        .filter_map(|c| dig_str(c, &["type"]))
        .map(|x| x.to_string())
        .collect();

    let mut internal_ip = String::new();
    let mut external_ip = String::new();
    for addr in dig_arr(obj, &["status", "addresses"]) {
        match addr.get("type").and_then(|x| x.as_str()) {
            Some("InternalIP") => {
                internal_ip = addr
                    .get("address")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string()
            }
            Some("ExternalIP") => {
                external_ip = addr
                    .get("address")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string()
            }
            _ => {}
        }
    }

    let taints: Vec<String> = dig_arr(obj, &["spec", "taints"])
        .iter()
        .map(|t| {
            let key = t.get("key").and_then(|x| x.as_str()).unwrap_or_default();
            let value = t.get("value").and_then(|x| x.as_str()).unwrap_or_default();
            let effect = t.get("effect").and_then(|x| x.as_str()).unwrap_or_default();
            if value.is_empty() {
                format!("{key}:{effect}")
            } else {
                format!("{key}={value}:{effect}")
            }
        })
        .collect();

    let version = dig_str(obj, &["status", "nodeInfo", "kubeletVersion"]).unwrap_or_default();
    s.extra
        .insert("roles".to_string(), Value::String(roles.join(",")));
    s.extra
        .insert("version".to_string(), Value::String(version.to_string()));
    s.extra
        .insert("internalIP".to_string(), Value::String(internal_ip));
    s.extra
        .insert("externalIP".to_string(), Value::String(external_ip));
    s.extra.insert(
        "os".to_string(),
        Value::String(
            dig_str(obj, &["status", "nodeInfo", "osImage"])
                .unwrap_or_default()
                .to_string(),
        ),
    );
    s.extra.insert(
        "kernel".to_string(),
        Value::String(
            dig_str(obj, &["status", "nodeInfo", "kernelVersion"])
                .unwrap_or_default()
                .to_string(),
        ),
    );
    s.extra.insert(
        "containerRuntime".to_string(),
        Value::String(
            dig_str(obj, &["status", "nodeInfo", "containerRuntimeVersion"])
                .unwrap_or_default()
                .to_string(),
        ),
    );
    s.extra.insert(
        "architecture".to_string(),
        Value::String(
            dig_str(obj, &["status", "nodeInfo", "architecture"])
                .unwrap_or_default()
                .to_string(),
        ),
    );
    s.extra.insert(
        "taints".to_string(),
        Value::Array(taints.into_iter().map(Value::String).collect()),
    );
    s.extra
        .insert("schedulable".to_string(), Value::Bool(!unschedulable));
    if let Some(c) = dig(obj, &["status", "capacity"]) {
        s.extra.insert("capacity".to_string(), c.clone());
    }
    if let Some(a) = dig(obj, &["status", "allocatable"]) {
        s.extra.insert("allocatable".to_string(), a.clone());
    }
    if !problems.is_empty() {
        s.extra.insert(
            "conditions".to_string(),
            Value::Array(problems.into_iter().map(Value::String).collect()),
        );
    }

    s.ready = Some(
        ready_cond
            .map(|c| c.status)
            .unwrap_or_else(|| "Unknown".to_string()),
    );
    s.status = status;
}

fn summarize_pvc(obj: &Value, s: &mut ObjectSummary) {
    let phase = dig_str(obj, &["status", "phase"]).unwrap_or("Pending");
    let capacity = dig_str(obj, &["status", "capacity", "storage"])
        .or_else(|| dig_str(obj, &["spec", "resources", "requests", "storage"]))
        .unwrap_or_default();
    let modes: Vec<String> = dig_arr(obj, &["spec", "accessModes"])
        .iter()
        .filter_map(|x| x.as_str())
        .map(|x| x.to_string())
        .collect();

    s.extra
        .insert("capacity".to_string(), Value::String(capacity.to_string()));
    s.extra.insert(
        "storageClass".to_string(),
        Value::String(
            dig_str(obj, &["spec", "storageClassName"])
                .unwrap_or_default()
                .to_string(),
        ),
    );
    s.extra
        .insert("accessModes".to_string(), Value::String(modes.join(",")));
    s.extra.insert(
        "volume".to_string(),
        Value::String(
            dig_str(obj, &["spec", "volumeName"])
                .unwrap_or_default()
                .to_string(),
        ),
    );
    s.extra.insert(
        "volumeMode".to_string(),
        Value::String(
            dig_str(obj, &["spec", "volumeMode"])
                .unwrap_or_default()
                .to_string(),
        ),
    );
    s.ready = Some(capacity.to_string());
    s.status = phase.to_string();
}

fn summarize_pv(obj: &Value, s: &mut ObjectSummary) {
    let phase = dig_str(obj, &["status", "phase"]).unwrap_or("Available");
    let capacity = dig_str(obj, &["spec", "capacity", "storage"]).unwrap_or_default();
    let claim = match (
        dig_str(obj, &["spec", "claimRef", "namespace"]),
        dig_str(obj, &["spec", "claimRef", "name"]),
    ) {
        (Some(ns), Some(n)) => format!("{ns}/{n}"),
        (None, Some(n)) => n.to_string(),
        _ => String::new(),
    };
    let modes: Vec<String> = dig_arr(obj, &["spec", "accessModes"])
        .iter()
        .filter_map(|x| x.as_str())
        .map(|x| x.to_string())
        .collect();

    s.extra
        .insert("capacity".to_string(), Value::String(capacity.to_string()));
    s.extra.insert("claim".to_string(), Value::String(claim));
    s.extra.insert(
        "reclaimPolicy".to_string(),
        Value::String(
            dig_str(obj, &["spec", "persistentVolumeReclaimPolicy"])
                .unwrap_or_default()
                .to_string(),
        ),
    );
    s.extra.insert(
        "storageClass".to_string(),
        Value::String(
            dig_str(obj, &["spec", "storageClassName"])
                .unwrap_or_default()
                .to_string(),
        ),
    );
    s.extra
        .insert("accessModes".to_string(), Value::String(modes.join(",")));
    if let Some(r) = dig_str(obj, &["status", "reason"]) {
        s.extra
            .insert("reason".to_string(), Value::String(r.to_string()));
    }
    s.ready = Some(capacity.to_string());
    s.status = phase.to_string();
}

fn summarize_secret(obj: &Value, s: &mut ObjectSummary) {
    // Jamais de valeurs : uniquement le nombre de clés.
    let keys = count_keys(obj, &["data"]) + count_keys(obj, &["stringData"]);
    let kind = dig_str(obj, &["type"]).unwrap_or("Opaque");
    s.extra
        .insert("type".to_string(), Value::String(kind.to_string()));
    s.extra.insert("keys".to_string(), Value::from(keys as i64));
    s.ready = Some(format!("{keys} clé(s)"));
    s.status = format!("{kind} · {keys} clé(s)");
}

fn summarize_configmap(obj: &Value, s: &mut ObjectSummary) {
    // Jamais de valeurs : uniquement le nombre de clés.
    let keys = count_keys(obj, &["data"]) + count_keys(obj, &["binaryData"]);
    s.extra.insert("keys".to_string(), Value::from(keys as i64));
    s.ready = Some(format!("{keys} clé(s)"));
    s.status = format!("{keys} clé(s)");
}

fn summarize_generic(obj: &Value, s: &mut ObjectSummary) {
    if let Some(phase) = dig_str(obj, &["status", "phase"]) {
        s.status = phase.to_string();
        return;
    }
    if let Some(state) = dig_str(obj, &["status", "state"]) {
        s.status = state.to_string();
        return;
    }
    for wanted in ["Ready", "Available", "Established", "Succeeded", "Healthy"] {
        if let Some(c) = find_condition(obj, wanted) {
            s.status = match c.status.as_str() {
                "True" => wanted.to_string(),
                "False" if !c.reason.is_empty() => c.reason,
                "False" => format!("Not{wanted}"),
                _ => "Unknown".to_string(),
            };
            return;
        }
    }
    // Dernier recours : première condition vraie.
    if let Some(c) = dig_arr(obj, &["status", "conditions"])
        .iter()
        .find(|c| dig_str(c, &["status"]) == Some("True"))
    {
        if let Some(t) = dig_str(c, &["type"]) {
            s.status = t.to_string();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resume_pod_pret() {
        let pod = json!({
            "metadata": {
                "name": "web-1", "namespace": "prod", "uid": "u-1",
                "creationTimestamp": "2020-01-01T00:00:00Z",
                "labels": {"app": "web"},
                "annotations": {"kubectl.kubernetes.io/last-applied-configuration": "{}"}
            },
            "spec": {
                "nodeName": "node-a",
                "serviceAccountName": "web",
                "containers": [
                    {"name": "web", "image": "nginx:1.27"},
                    {"name": "sidecar", "image": "envoy:1.30"}
                ]
            },
            "status": {
                "phase": "Running",
                "podIP": "10.0.0.5",
                "qosClass": "Burstable",
                "conditions": [{"type": "Ready", "status": "True"}],
                "containerStatuses": [
                    {"name": "web", "ready": true, "restartCount": 2, "state": {"running": {}}},
                    {"name": "sidecar", "ready": false, "restartCount": 1,
                     "state": {"waiting": {"reason": "CrashLoopBackOff"}}}
                ]
            }
        });
        let s = summarize("Pod", "v1", &pod);
        assert_eq!(s.name, "web-1");
        assert_eq!(s.namespace.as_deref(), Some("prod"));
        assert_eq!(s.status, "CrashLoopBackOff");
        assert_eq!(s.ready.as_deref(), Some("1/2"));
        assert_eq!(s.restarts, Some(3));
        assert_eq!(s.node.as_deref(), Some("node-a"));
        assert_eq!(s.images, vec!["nginx:1.27", "envoy:1.30"]);
        assert!(s.age_seconds.unwrap_or(0) > 0);
        // L'annotation volumineuse doit être retirée du résumé.
        assert!(s.annotations.is_empty());
        assert_eq!(s.extra.get("podIP").and_then(|v| v.as_str()), Some("10.0.0.5"));
    }

    #[test]
    fn resume_pod_initialisation_et_suppression() {
        let pod = json!({
            "metadata": {"name": "job-pod"},
            "spec": {
                "initContainers": [{"name": "i1", "image": "busybox"}, {"name": "i2", "image": "busybox"}],
                "containers": [{"name": "main", "image": "app:1"}]
            },
            "status": {
                "phase": "Pending",
                "initContainerStatuses": [
                    {"name": "i1", "restartCount": 0, "state": {"terminated": {"exitCode": 0}}},
                    {"name": "i2", "restartCount": 0, "state": {"waiting": {"reason": "PodInitializing"}}}
                ],
                "containerStatuses": []
            }
        });
        let s = summarize("Pod", "v1", &pod);
        assert_eq!(s.status, "Init:1/2");

        let terminating = json!({
            "metadata": {"name": "p", "deletionTimestamp": "2024-01-01T00:00:00Z"},
            "spec": {"containers": [{"name": "c", "image": "i"}]},
            "status": {"phase": "Running", "containerStatuses": [
                {"name": "c", "ready": true, "restartCount": 0, "state": {"running": {}}}]}
        });
        assert_eq!(summarize("Pod", "v1", &terminating).status, "Terminating");
    }

    #[test]
    fn resume_deployment() {
        let dep = json!({
            "metadata": {"name": "api", "namespace": "default",
                         "annotations": {"deployment.kubernetes.io/revision": "7"}},
            "spec": {"replicas": 3, "strategy": {"type": "RollingUpdate"},
                     "template": {"spec": {"containers": [{"name": "api", "image": "ghcr.io/acme/api:2.1.0"}]}}},
            "status": {"replicas": 3, "readyReplicas": 3, "updatedReplicas": 3, "availableReplicas": 3,
                       "conditions": [{"type": "Available", "status": "True"}]}
        });
        let s = summarize("Deployment", "apps/v1", &dep);
        assert_eq!(s.status, "Ready");
        assert_eq!(s.ready.as_deref(), Some("3/3"));
        assert_eq!(s.images, vec!["ghcr.io/acme/api:2.1.0"]);
        assert_eq!(s.extra.get("upToDate").and_then(|v| v.as_i64()), Some(3));
        assert_eq!(s.extra.get("revision").and_then(|v| v.as_str()), Some("7"));

        let bloque = json!({
            "metadata": {"name": "api"},
            "spec": {"replicas": 3},
            "status": {"readyReplicas": 1,
                       "conditions": [{"type": "Progressing", "status": "False",
                                       "reason": "ProgressDeadlineExceeded"}]}
        });
        assert_eq!(
            summarize("Deployment", "apps/v1", &bloque).status,
            "ProgressDeadlineExceeded"
        );
    }

    #[test]
    fn resume_service() {
        let svc = json!({
            "metadata": {"name": "front"},
            "spec": {"type": "LoadBalancer", "clusterIP": "10.96.0.10",
                     "selector": {"app": "front"},
                     "ports": [{"port": 80, "nodePort": 30080, "protocol": "TCP"},
                               {"port": 443, "protocol": "TCP"}]},
            "status": {"loadBalancer": {"ingress": [{"ip": "203.0.113.7"}]}}
        });
        let s = summarize("Service", "v1", &svc);
        assert_eq!(s.status, "LoadBalancer");
        assert_eq!(s.extra.get("ports").and_then(|v| v.as_str()), Some("80:30080/TCP,443/TCP"));
        assert_eq!(s.extra.get("externalIP").and_then(|v| v.as_str()), Some("203.0.113.7"));
        assert_eq!(s.extra.get("clusterIP").and_then(|v| v.as_str()), Some("10.96.0.10"));

        let pending = json!({
            "metadata": {"name": "front"},
            "spec": {"type": "LoadBalancer", "clusterIP": "10.96.0.10", "ports": []},
            "status": {}
        });
        assert_eq!(summarize("Service", "v1", &pending).status, "Pending");
    }

    #[test]
    fn resume_node() {
        let node = json!({
            "metadata": {"name": "worker-1",
                         "labels": {"node-role.kubernetes.io/worker": "",
                                    "kubernetes.io/os": "linux"}},
            "spec": {"unschedulable": true,
                     "taints": [{"key": "dedicated", "value": "gpu", "effect": "NoSchedule"}]},
            "status": {
                "conditions": [{"type": "Ready", "status": "True"},
                               {"type": "MemoryPressure", "status": "False"}],
                "addresses": [{"type": "InternalIP", "address": "192.168.1.10"}],
                "nodeInfo": {"kubeletVersion": "v1.31.2", "osImage": "Debian 12",
                             "architecture": "amd64"},
                "capacity": {"cpu": "8", "memory": "32Gi"}
            }
        });
        let s = summarize("Node", "v1", &node);
        assert_eq!(s.status, "Ready,SchedulingDisabled");
        assert_eq!(s.extra.get("roles").and_then(|v| v.as_str()), Some("worker"));
        assert_eq!(s.extra.get("version").and_then(|v| v.as_str()), Some("v1.31.2"));
        assert_eq!(s.extra.get("internalIP").and_then(|v| v.as_str()), Some("192.168.1.10"));
        assert_eq!(s.extra.get("schedulable").and_then(|v| v.as_bool()), Some(false));
        let taints = s.extra.get("taints").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        assert_eq!(taints.len(), 1);
        assert_eq!(taints[0].as_str(), Some("dedicated=gpu:NoSchedule"));
    }

    #[test]
    fn resume_secret_sans_valeurs() {
        let secret = json!({
            "metadata": {"name": "tls"},
            "type": "kubernetes.io/tls",
            "data": {"tls.crt": "QUJD", "tls.key": "REVG"}
        });
        let s = summarize("Secret", "v1", &secret);
        assert_eq!(s.extra.get("keys").and_then(|v| v.as_i64()), Some(2));
        let rendu = serde_json::to_string(&s.extra).unwrap_or_default();
        assert!(!rendu.contains("QUJD"), "aucune valeur de secret ne doit fuiter");
    }

    #[test]
    fn conteneurs_du_pod() {
        let pod = json!({
            "spec": {
                "initContainers": [{"name": "init", "image": "busybox:1"}],
                "containers": [{"name": "app", "image": "app:1"}]
            },
            "status": {
                "initContainerStatuses": [{"name": "init", "ready": true, "restartCount": 0,
                                           "state": {"terminated": {"reason": "Completed", "exitCode": 0}}}],
                "containerStatuses": [{"name": "app", "ready": true, "restartCount": 4,
                                       "image": "app:1", "state": {"running": {}}}]
            }
        });
        let c = containers_from_pod(&pod);
        assert_eq!(c.len(), 2);
        assert!(c[0].init);
        assert_eq!(c[0].state, "Completed");
        assert!(!c[1].init);
        assert_eq!(c[1].state, "Running");
        assert_eq!(c[1].restart_count, 4);
    }

    #[test]
    fn chemin_du_pod_spec() {
        let dep = json!({"spec": {"template": {"spec": {"containers": []}}}});
        assert_eq!(pod_spec_path(&dep), Some(vec!["spec", "template", "spec"]));
        let cj = json!({"spec": {"jobTemplate": {"spec": {"template": {"spec": {"containers": []}}}}}});
        assert_eq!(
            pod_spec_path(&cj),
            Some(vec!["spec", "jobTemplate", "spec", "template", "spec"])
        );
        let pod = json!({"spec": {"containers": []}});
        assert_eq!(pod_spec_path(&pod), Some(vec!["spec"]));
        assert_eq!(pod_spec_path(&json!({"spec": {}})), None);
    }

    #[test]
    fn imbrication_de_patch() {
        let leaf = json!({"replicas": 2});
        let nested = nest(&["spec", "template"], leaf);
        assert_eq!(nested, json!({"spec": {"template": {"replicas": 2}}}));
    }

    #[test]
    fn duree_lisible() {
        assert_eq!(human_duration(45), "45s");
        assert_eq!(human_duration(125), "2m5s");
        assert_eq!(human_duration(7300), "2h1m");
        assert_eq!(human_duration(90_000), "1j1h");
    }
}

#[cfg(test)]
mod topology_tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn proprietaires_extraits() {
        let pod = json!({
            "metadata": {
                "name": "web-abc-1", "namespace": "app",
                "ownerReferences": [
                    { "kind": "ReplicaSet", "name": "web-abc", "uid": "u1", "controller": true },
                    { "kind": "Foo", "name": "bar" }
                ]
            },
            "spec": { "containers": [ { "name": "c", "image": "nginx" } ] }
        });
        let s = summarize("Pod", "v1", &pod);
        assert_eq!(s.owners.len(), 2);
        assert_eq!(s.owners[0].kind, "ReplicaSet");
        assert_eq!(s.owners[0].name, "web-abc");
        assert_eq!(s.owners[0].uid.as_deref(), Some("u1"));
        assert!(s.owners[0].controller);
        assert!(!s.owners[1].controller);
        // Sans propriétaire, la liste est vide et omise de la sérialisation.
        let seul = summarize("Pod", "v1", &json!({ "metadata": { "name": "x" } }));
        assert!(seul.owners.is_empty());
    }

    #[test]
    fn dependances_du_pod() {
        let pod = json!({
            "metadata": { "name": "db-0", "namespace": "app" },
            "spec": {
                "volumes": [
                    { "name": "data", "persistentVolumeClaim": { "claimName": "data-db-0" } },
                    { "name": "cfg", "configMap": { "name": "db-config" } },
                    { "name": "tls", "secret": { "secretName": "db-tls" } },
                    { "name": "proj", "projected": { "sources": [
                        { "configMap": { "name": "db-config" } },
                        { "secret": { "name": "db-token" } }
                    ] } }
                ],
                "initContainers": [ { "name": "init", "image": "busybox",
                    "envFrom": [ { "secretRef": { "name": "db-init" } } ] } ],
                "containers": [ { "name": "db", "image": "postgres",
                    "envFrom": [ { "configMapRef": { "name": "db-env" } } ],
                    "env": [
                        { "name": "PW", "valueFrom": { "secretKeyRef": { "name": "db-pw", "key": "p" } } },
                        { "name": "H", "valueFrom": { "configMapKeyRef": { "name": "db-config", "key": "h" } } }
                    ] } ]
            }
        });
        let s = summarize("Pod", "v1", &pod);
        let list = |k: &str| -> Vec<String> {
            s.extra[k]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(list("volumeClaims"), vec!["data-db-0"]);
        assert_eq!(list("configMaps"), vec!["db-config", "db-env"]);
        assert_eq!(
            list("secrets"),
            vec!["db-tls", "db-token", "db-init", "db-pw"]
        );

        // Un pod sans dépendance ne porte pas ces clés.
        let nu = summarize(
            "Pod",
            "v1",
            &json!({ "metadata": { "name": "nu" }, "spec": {} }),
        );
        assert!(nu.extra.get("volumeClaims").is_none());
        assert!(nu.extra.get("secrets").is_none());
    }

    #[test]
    fn backends_de_l_ingress() {
        let ing = json!({
            "metadata": { "name": "web", "namespace": "app" },
            "spec": {
                "defaultBackend": { "service": { "name": "fallback", "port": { "number": 80 } } },
                "rules": [
                    { "host": "a.example", "http": { "paths": [
                        { "path": "/", "backend": { "service": { "name": "web", "port": { "number": 80 } } } },
                        { "path": "/api", "backend": { "service": { "name": "api", "port": { "number": 80 } } } },
                        { "path": "/old", "backend": { "serviceName": "legacy", "servicePort": 80 } }
                    ] } },
                    { "host": "b.example", "http": { "paths": [
                        { "path": "/", "backend": { "service": { "name": "web", "port": { "number": 80 } } } }
                    ] } }
                ]
            }
        });
        let s = summarize("Ingress", "networking.k8s.io/v1", &ing);
        let backends: Vec<&str> = s.extra["backends"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(backends, vec!["fallback", "web", "api", "legacy"]);
    }
}
