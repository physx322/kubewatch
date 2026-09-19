//! Application de manifestes YAML (`kubectl apply` côté serveur), suppression
//! par manifeste et calcul de différences avant application.

use kube::api::{Api, DeleteParams, DynamicObject, Patch, PatchParams, PostParams};
use kube::core::GroupVersionKind;
use kube::discovery::{ApiResource, Scope};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::cluster::ClusterHandle;
use crate::discovery::ResourceCatalog;
use crate::error::{Error, Result};
use crate::model::ResourceRef;
use crate::resource::{dig_str, split_api_version, FIELD_MANAGER};

/// Nombre maximal de cellules de la table LCS avant repli sur un diff grossier.
const MAX_DIFF_CELLS: usize = 4_000_000;
/// Lignes de contexte autour de chaque bloc modifié.
const DIFF_CONTEXT: usize = 3;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Options d'application d'un manifeste.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOptions {
    /// Gestionnaire de champs déclaré auprès de l'API serveur.
    pub field_manager: String,
    /// Reprend de force les champs détenus par un autre gestionnaire.
    pub force: bool,
    /// Simulation : rien n'est persisté côté serveur.
    pub dry_run: bool,
    /// Namespace appliqué aux documents qui n'en précisent pas.
    pub default_namespace: Option<String>,
    /// Utilise l'application côté serveur (recommandé).
    pub server_side: bool,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            field_manager: FIELD_MANAGER.to_string(),
            force: false,
            dry_run: false,
            default_namespace: None,
            server_side: true,
        }
    }
}

/// Effet observé pour un document du manifeste.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplyAction {
    /// La ressource n'existait pas et vient d'être créée.
    Created,
    /// La ressource existait et a été modifiée.
    Configured,
    /// La ressource était déjà conforme au manifeste.
    Unchanged,
    /// Simulation : aucune écriture réelle.
    DryRun,
    /// La ressource a été supprimée.
    Deleted,
    /// Le document n'a pas pu être traité.
    Failed,
}

/// Résultat pour un document du manifeste.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyItem {
    /// Ressource visée par le document.
    pub resource: ResourceRef,
    /// Effet observé.
    pub action: ApplyAction,
    /// Détail complémentaire (raison d'un échec, précision).
    pub message: Option<String>,
}

/// Bilan global d'une application de manifeste.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcome {
    /// Un élément par document traité, dans l'ordre du manifeste.
    pub items: Vec<ApplyItem>,
    /// Nombre de documents en échec.
    pub failed: usize,
}

// ---------------------------------------------------------------------------
// Découpage des manifestes
// ---------------------------------------------------------------------------

/// Découpe un manifeste multi-documents en objets JSON.
///
/// Les documents vides ou nuls sont ignorés ; un document dépourvu de
/// `apiVersion` ou de `kind` est rejeté avec son numéro d'ordre. Les listes
/// (`kind: List`) sont dépliées.
pub fn split_documents(yaml: &str) -> Result<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    let mut index = 0usize;

    for raw in serde_yaml_ng::Deserializer::from_str(yaml) {
        index += 1;
        let value = serde_yaml_ng::Value::deserialize(raw)
            .map_err(|e| Error::Yaml(format!("document {index} : YAML invalide ({e})")))?;
        if matches!(value, serde_yaml_ng::Value::Null) {
            continue;
        }
        let json: Value = serde_json::to_value(&value).map_err(|e| {
            Error::Yaml(format!(
                "document {index} : conversion YAML → JSON impossible ({e})"
            ))
        })?;

        let Some(obj) = json.as_object() else {
            return Err(Error::Invalid(format!(
                "document {index} : un manifeste doit être une association clé/valeur"
            )));
        };
        if obj.is_empty() {
            continue;
        }

        let kind = obj.get("kind").and_then(|v| v.as_str()).unwrap_or_default();
        let api_version = obj
            .get("apiVersion")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        // Liste enveloppante : on déplie ses éléments.
        if kind == "List" {
            if let Some(items) = obj.get("items").and_then(|v| v.as_array()) {
                for (n, item) in items.iter().enumerate() {
                    let ok = item
                        .get("apiVersion")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.trim().is_empty())
                        && item
                            .get("kind")
                            .and_then(|v| v.as_str())
                            .is_some_and(|s| !s.trim().is_empty());
                    if !ok {
                        return Err(Error::Invalid(format!(
                            "document {index}, élément {} : « apiVersion » ou « kind » manquant",
                            n + 1
                        )));
                    }
                    out.push(item.clone());
                }
                continue;
            }
        }

        if api_version.trim().is_empty() || kind.trim().is_empty() {
            return Err(Error::Invalid(format!(
                "document {index} : champ « apiVersion » ou « kind » manquant"
            )));
        }
        out.push(json);
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Résolution du type et construction de l'API
// ---------------------------------------------------------------------------

/// Référence minimale déduite d'un document, utilisée aussi en cas d'échec.
fn bare_ref(doc: &Value) -> ResourceRef {
    let (group, version) = split_api_version(dig_str(doc, &["apiVersion"]).unwrap_or_default());
    ResourceRef {
        group,
        version,
        kind: dig_str(doc, &["kind"]).unwrap_or_default().to_string(),
        plural: String::new(),
        namespace: dig_str(doc, &["metadata", "namespace"]).map(|s| s.to_string()),
        name: dig_str(doc, &["metadata", "name"])
            .or_else(|| dig_str(doc, &["metadata", "generateName"]))
            .unwrap_or_default()
            .to_string(),
    }
}

/// Retrouve la ressource d'API correspondant à un GVK, via le catalogue puis
/// par interrogation directe du serveur.
async fn resolve_api_resource(
    h: &ClusterHandle,
    group: &str,
    version: &str,
    kind: &str,
) -> Result<(ApiResource, bool)> {
    {
        let catalog = h.catalog.read();
        if let Some(k) = catalog
            .kinds
            .iter()
            .find(|k| k.kind == kind && k.group == group && k.version == version)
        {
            return Ok((ResourceCatalog::api_resource(k), k.namespaced));
        }
    }

    let gvk = GroupVersionKind::gvk(group, version, kind);
    match kube::discovery::pinned_kind(&h.client, &gvk).await {
        Ok((ar, caps)) => Ok((ar, caps.scope == Scope::Namespaced)),
        Err(e) => {
            let catalog = h.catalog.read();
            if let Some(k) = catalog
                .kinds
                .iter()
                .find(|k| k.kind == kind && k.group == group)
            {
                return Ok((ResourceCatalog::api_resource(k), k.namespaced));
            }
            Err(Error::Discovery(format!(
                "type « {kind} » ({}) inconnu du cluster : {e}",
                if group.is_empty() {
                    version.to_string()
                } else {
                    format!("{group}/{version}")
                }
            )))
        }
    }
}

/// Contexte résolu d'un document : API prête à l'emploi et référence complète.
struct Resolved {
    api: Api<DynamicObject>,
    reference: ResourceRef,
    /// Le document, complété du namespace effectif.
    doc: Value,
    /// Vrai si le nom doit être généré par le serveur (`generateName`).
    generated_name: bool,
}

async fn resolve_document(
    h: &ClusterHandle,
    doc: &Value,
    opts: &ApplyOptions,
) -> Result<Resolved> {
    let api_version = dig_str(doc, &["apiVersion"]).unwrap_or_default().to_string();
    let (group, version) = split_api_version(&api_version);
    let kind = dig_str(doc, &["kind"]).unwrap_or_default().to_string();

    let (ar, namespaced) = resolve_api_resource(h, &group, &version, &kind).await?;

    let name = dig_str(doc, &["metadata", "name"])
        .unwrap_or_default()
        .to_string();
    let generate_name = dig_str(doc, &["metadata", "generateName"])
        .unwrap_or_default()
        .to_string();
    let generated_name = name.is_empty() && !generate_name.is_empty();
    if name.is_empty() && generate_name.is_empty() {
        return Err(Error::Invalid(format!(
            "{kind} : « metadata.name » est obligatoire"
        )));
    }

    let namespace = if namespaced {
        Some(
            dig_str(doc, &["metadata", "namespace"])
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    opts.default_namespace
                        .clone()
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or_else(|| h.default_namespace.clone()),
        )
    } else {
        None
    };

    let mut doc = doc.clone();
    {
        let root = doc
            .as_object_mut()
            .ok_or_else(|| Error::Invalid("manifeste invalide".to_string()))?;
        let md = root
            .entry("metadata")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| Error::Invalid("« metadata » doit être un objet".to_string()))?;
        match &namespace {
            Some(ns) => {
                md.insert("namespace".to_string(), Value::String(ns.clone()));
            }
            None => {
                md.remove("namespace");
            }
        }
        md.remove("managedFields");
    }

    let api: Api<DynamicObject> = match &namespace {
        Some(ns) => Api::namespaced_with(h.client.clone(), ns, &ar),
        None => Api::all_with(h.client.clone(), &ar),
    };

    let reference = ResourceRef {
        group: ar.group.clone(),
        version: ar.version.clone(),
        kind: ar.kind.clone(),
        plural: ar.plural.clone(),
        namespace,
        name: if name.is_empty() {
            generate_name.clone()
        } else {
            name
        },
    };

    Ok(Resolved {
        api,
        reference,
        doc,
        generated_name,
    })
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// Applique un manifeste YAML, document par document.
///
/// Une erreur sur un document n'interrompt jamais les suivants : elle est
/// consignée dans l'élément correspondant et comptabilisée dans `failed`.
pub async fn apply_yaml(h: &ClusterHandle, yaml: &str, opts: &ApplyOptions) -> Result<ApplyOutcome> {
    let docs = split_documents(yaml)?;
    let mut items = Vec::with_capacity(docs.len());
    let mut failed = 0usize;

    for doc in &docs {
        match apply_one(h, doc, opts).await {
            Ok(item) => {
                if item.action == ApplyAction::Failed {
                    failed += 1;
                }
                items.push(item);
            }
            Err(e) => {
                failed += 1;
                items.push(ApplyItem {
                    resource: bare_ref(doc),
                    action: ApplyAction::Failed,
                    message: Some(e.to_string()),
                });
            }
        }
    }

    Ok(ApplyOutcome { items, failed })
}

async fn apply_one(h: &ClusterHandle, doc: &Value, opts: &ApplyOptions) -> Result<ApplyItem> {
    let resolved = resolve_document(h, doc, opts).await?;
    let Resolved {
        api,
        reference,
        doc,
        generated_name,
    } = resolved;

    let object: DynamicObject = serde_json::from_value(doc.clone())?;

    // Nom généré par le serveur : seule la création est possible.
    if generated_name {
        let pp = PostParams {
            dry_run: opts.dry_run,
            field_manager: Some(opts.field_manager.clone()),
        };
        let created = api.create(&pp, &object).await?;
        let mut reference = reference;
        if let Some(assigned) = created.metadata.name.clone() {
            reference.name = assigned;
        }
        return Ok(ApplyItem {
            resource: reference,
            action: if opts.dry_run {
                ApplyAction::DryRun
            } else {
                ApplyAction::Created
            },
            message: None,
        });
    }

    let before = api.get_opt(&reference.name).await?;
    let before_version = before
        .as_ref()
        .and_then(|o| o.metadata.resource_version.clone());

    let after = if opts.server_side {
        let mut pp = PatchParams::apply(&opts.field_manager);
        if opts.force {
            pp = pp.force();
        }
        if opts.dry_run {
            pp = pp.dry_run();
        }
        api.patch(&reference.name, &pp, &Patch::Apply(&object))
            .await?
    } else if before.is_none() {
        let pp = PostParams {
            dry_run: opts.dry_run,
            field_manager: Some(opts.field_manager.clone()),
        };
        api.create(&pp, &object).await?
    } else {
        let pp = PatchParams {
            dry_run: opts.dry_run,
            field_manager: Some(opts.field_manager.clone()),
            ..Default::default()
        };
        api.patch(&reference.name, &pp, &Patch::Merge(&doc)).await?
    };

    let action = if opts.dry_run {
        ApplyAction::DryRun
    } else if before.is_none() {
        ApplyAction::Created
    } else if after.metadata.resource_version == before_version {
        ApplyAction::Unchanged
    } else {
        ApplyAction::Configured
    };

    Ok(ApplyItem {
        resource: reference,
        action,
        message: None,
    })
}

// ---------------------------------------------------------------------------
// Suppression par manifeste
// ---------------------------------------------------------------------------

/// Supprime toutes les ressources décrites par un manifeste.
pub async fn delete_yaml(
    h: &ClusterHandle,
    yaml: &str,
    opts: &ApplyOptions,
) -> Result<ApplyOutcome> {
    let docs = split_documents(yaml)?;
    let mut items = Vec::with_capacity(docs.len());
    let mut failed = 0usize;

    for doc in &docs {
        match delete_one(h, doc, opts).await {
            Ok(item) => items.push(item),
            Err(e) => {
                failed += 1;
                items.push(ApplyItem {
                    resource: bare_ref(doc),
                    action: ApplyAction::Failed,
                    message: Some(e.to_string()),
                });
            }
        }
    }

    Ok(ApplyOutcome { items, failed })
}

async fn delete_one(h: &ClusterHandle, doc: &Value, opts: &ApplyOptions) -> Result<ApplyItem> {
    let resolved = resolve_document(h, doc, opts).await?;
    if resolved.generated_name {
        return Err(Error::Invalid(
            "impossible de supprimer une ressource désignée par « generateName »".to_string(),
        ));
    }

    let dp = DeleteParams {
        dry_run: opts.dry_run,
        ..Default::default()
    };
    match resolved.api.delete(&resolved.reference.name, &dp).await {
        Ok(_) => Ok(ApplyItem {
            resource: resolved.reference,
            action: if opts.dry_run {
                ApplyAction::DryRun
            } else {
                ApplyAction::Deleted
            },
            message: None,
        }),
        Err(kube::Error::Api(status)) if status.is_not_found() => Ok(ApplyItem {
            resource: resolved.reference,
            action: ApplyAction::Deleted,
            message: Some("ressource déjà absente".to_string()),
        }),
        Err(e) => Err(e.into()),
    }
}

// ---------------------------------------------------------------------------
// Différences
// ---------------------------------------------------------------------------

/// Calcule, pour chaque document, la différence entre l'état courant du cluster
/// et l'état qu'obtiendrait une application (simulation côté serveur).
pub async fn diff_yaml(
    h: &ClusterHandle,
    yaml: &str,
    opts: &ApplyOptions,
) -> Result<Vec<(ResourceRef, String)>> {
    let docs = split_documents(yaml)?;
    let mut out = Vec::with_capacity(docs.len());

    for doc in &docs {
        match diff_one(h, doc, opts).await {
            Ok(pair) => out.push(pair),
            Err(e) => out.push((
                bare_ref(doc),
                format!("# différence indisponible : {e}\n"),
            )),
        }
    }
    Ok(out)
}

async fn diff_one(
    h: &ClusterHandle,
    doc: &Value,
    opts: &ApplyOptions,
) -> Result<(ResourceRef, String)> {
    let resolved = resolve_document(h, doc, opts).await?;
    let Resolved {
        api,
        reference,
        doc,
        generated_name,
    } = resolved;

    if generated_name {
        return Ok((
            reference,
            "# ressource à nom généré : toujours créée, aucune comparaison possible\n".to_string(),
        ));
    }

    let object: DynamicObject = serde_json::from_value(doc)?;
    let live = api.get_opt(&reference.name).await?;

    let mut pp = PatchParams::apply(&opts.field_manager).dry_run();
    if opts.force {
        pp = pp.force();
    }
    let simulated = api
        .patch(&reference.name, &pp, &Patch::Apply(&object))
        .await?;

    let live_yaml = match &live {
        Some(o) => to_stable_yaml(&serde_json::to_value(o)?)?,
        None => String::new(),
    };
    let next_yaml = to_stable_yaml(&serde_json::to_value(&simulated)?)?;

    let label = match &reference.namespace {
        Some(ns) => format!("{}/{} ({ns})", reference.kind, reference.name),
        None => format!("{}/{}", reference.kind, reference.name),
    };
    let text = unified_diff(
        &format!("cluster {label}"),
        &format!("manifeste {label}"),
        &live_yaml,
        &next_yaml,
    );
    Ok((reference, text))
}

/// Retire les champs volatils puis sérialise en YAML.
fn to_stable_yaml(value: &Value) -> Result<String> {
    let cleaned = sanitize(value);
    serde_yaml_ng::to_string(&cleaned)
        .map_err(|e| Error::Yaml(format!("sérialisation YAML impossible : {e}")))
}

/// Supprime les champs gérés par le serveur, sans intérêt dans un diff.
fn sanitize(value: &Value) -> Value {
    let mut out = value.clone();
    let Some(root) = out.as_object_mut() else {
        return out;
    };
    root.remove("status");

    if let Some(md) = root.get_mut("metadata").and_then(|m| m.as_object_mut()) {
        for key in [
            "resourceVersion",
            "uid",
            "generation",
            "managedFields",
            "creationTimestamp",
            "selfLink",
        ] {
            md.remove(key);
        }
        let drop_annotations = match md.get_mut("annotations").and_then(|a| a.as_object_mut()) {
            Some(ann) => {
                ann.remove("kubectl.kubernetes.io/last-applied-configuration");
                ann.is_empty()
            }
            None => false,
        };
        if drop_annotations {
            md.remove("annotations");
        }
    }
    out
}

/// Opération élémentaire d'un diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Del,
    Ins,
}

/// Script d'édition minimal entre deux suites de lignes (LCS).
fn diff_ops(a: &[&str], b: &[&str]) -> Vec<(Op, usize, usize)> {
    let n = a.len();
    let m = b.len();
    let mut ops: Vec<(Op, usize, usize)> = Vec::new();

    let mut pre = 0usize;
    while pre < n && pre < m && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0usize;
    while suf < n - pre && suf < m - pre && a[n - 1 - suf] == b[m - 1 - suf] {
        suf += 1;
    }
    for i in 0..pre {
        ops.push((Op::Eq, i, i));
    }

    let ax = &a[pre..n - suf];
    let bx = &b[pre..m - suf];
    let n2 = ax.len();
    let m2 = bx.len();

    if n2 == 0 || m2 == 0 || n2.saturating_mul(m2) > MAX_DIFF_CELLS {
        for i in 0..n2 {
            ops.push((Op::Del, pre + i, pre));
        }
        for j in 0..m2 {
            ops.push((Op::Ins, pre + n2, pre + j));
        }
    } else {
        let w = m2 + 1;
        let mut dp = vec![0u32; (n2 + 1) * w];
        for i in (0..n2).rev() {
            for j in (0..m2).rev() {
                dp[i * w + j] = if ax[i] == bx[j] {
                    dp[(i + 1) * w + j + 1] + 1
                } else {
                    dp[(i + 1) * w + j].max(dp[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0usize, 0usize);
        while i < n2 && j < m2 {
            if ax[i] == bx[j] {
                ops.push((Op::Eq, pre + i, pre + j));
                i += 1;
                j += 1;
            } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
                ops.push((Op::Del, pre + i, pre + j));
                i += 1;
            } else {
                ops.push((Op::Ins, pre + i, pre + j));
                j += 1;
            }
        }
        while i < n2 {
            ops.push((Op::Del, pre + i, pre + j));
            i += 1;
        }
        while j < m2 {
            ops.push((Op::Ins, pre + i, pre + j));
            j += 1;
        }
    }

    for k in 0..suf {
        ops.push((Op::Eq, n - suf + k, m - suf + k));
    }
    ops
}

/// Diff unifié minimal, écrit à la main (aucune dépendance externe).
fn unified_diff(old_label: &str, new_label: &str, old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let ops = diff_ops(&a, &b);

    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| o.0 != Op::Eq)
        .map(|(i, _)| i)
        .collect();
    if changed.is_empty() {
        return String::new();
    }

    let last = ops.len().saturating_sub(1);
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut start = changed[0].saturating_sub(DIFF_CONTEXT);
    let mut end = (changed[0] + DIFF_CONTEXT).min(last);
    for &c in changed.iter().skip(1) {
        if c.saturating_sub(DIFF_CONTEXT) <= end + 1 {
            end = (c + DIFF_CONTEXT).min(last);
        } else {
            groups.push((start, end));
            start = c.saturating_sub(DIFF_CONTEXT);
            end = (c + DIFF_CONTEXT).min(last);
        }
    }
    groups.push((start, end));

    let mut out = String::new();
    out.push_str("--- ");
    out.push_str(old_label);
    out.push('\n');
    out.push_str("+++ ");
    out.push_str(new_label);
    out.push('\n');

    for (s, e) in groups {
        let a_start = ops[s].1;
        let b_start = ops[s].2;
        let mut a_count = 0usize;
        let mut b_count = 0usize;
        let mut body = String::new();
        for op in &ops[s..=e] {
            match op.0 {
                Op::Eq => {
                    a_count += 1;
                    b_count += 1;
                    body.push(' ');
                    body.push_str(a.get(op.1).copied().unwrap_or_default());
                    body.push('\n');
                }
                Op::Del => {
                    a_count += 1;
                    body.push('-');
                    body.push_str(a.get(op.1).copied().unwrap_or_default());
                    body.push('\n');
                }
                Op::Ins => {
                    b_count += 1;
                    body.push('+');
                    body.push_str(b.get(op.2).copied().unwrap_or_default());
                    body.push('\n');
                }
            }
        }
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            a_start + 1,
            a_count,
            b_start + 1,
            b_count
        ));
        out.push_str(&body);
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decoupe_multi_documents() {
        let yaml = r#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: cm-a
---
# uniquement un commentaire
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: dep-a
  namespace: prod
---
"#;
        let docs = split_documents(yaml).expect("découpage valide");
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0]["kind"], json!("ConfigMap"));
        assert_eq!(docs[1]["metadata"]["namespace"], json!("prod"));
    }

    #[test]
    fn rejette_document_sans_kind() {
        let yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n---\nmetadata:\n  name: b\n";
        let err = split_documents(yaml).expect_err("le second document est invalide");
        let msg = err.to_string();
        assert!(msg.contains('2'), "le numéro de document doit apparaître : {msg}");
    }

    #[test]
    fn rejette_document_sans_api_version() {
        let yaml = "kind: ConfigMap\nmetadata:\n  name: a\n";
        assert!(split_documents(yaml).is_err());
    }

    #[test]
    fn ignore_documents_vides() {
        assert!(split_documents("").expect("vide accepté").is_empty());
        assert!(split_documents("---\n---\n").expect("vide accepté").is_empty());
        assert!(split_documents("# rien\n").expect("vide accepté").is_empty());
    }

    #[test]
    fn deplie_les_listes() {
        let yaml = r#"
apiVersion: v1
kind: List
items:
  - apiVersion: v1
    kind: Service
    metadata:
      name: s1
  - apiVersion: v1
    kind: Service
    metadata:
      name: s2
"#;
        let docs = split_documents(yaml).expect("liste dépliée");
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[1]["metadata"]["name"], json!("s2"));
    }

    #[test]
    fn nettoyage_des_champs_volatils() {
        let v = json!({
            "apiVersion": "v1", "kind": "ConfigMap",
            "metadata": {
                "name": "cm", "uid": "x", "resourceVersion": "42", "generation": 3,
                "creationTimestamp": "2024-01-01T00:00:00Z", "managedFields": [{"manager": "k"}],
                "annotations": {"kubectl.kubernetes.io/last-applied-configuration": "{}"}
            },
            "status": {"phase": "Active"},
            "data": {"a": "1"}
        });
        let c = sanitize(&v);
        assert!(c.get("status").is_none());
        let md = c.get("metadata").and_then(|m| m.as_object()).expect("metadata");
        for k in ["uid", "resourceVersion", "generation", "creationTimestamp", "managedFields", "annotations"] {
            assert!(md.get(k).is_none(), "« {k} » aurait dû être retiré");
        }
        assert_eq!(c["data"]["a"], json!("1"));
    }

    #[test]
    fn diff_identique_est_vide() {
        let s = "a\nb\nc\n";
        assert_eq!(unified_diff("x", "y", s, s), "");
    }

    #[test]
    fn diff_unifie_simple() {
        let old = "l1\nl2\nl3\nl4\nl5\n";
        let new = "l1\nl2\nMODIFIE\nl4\nl5\n";
        let d = unified_diff("avant", "apres", old, new);
        assert!(d.starts_with("--- avant\n+++ apres\n"), "{d}");
        assert!(d.contains("-l3\n"), "{d}");
        assert!(d.contains("+MODIFIE\n"), "{d}");
        assert!(d.contains("@@ "), "{d}");
        assert!(d.contains(" l2\n"), "contexte manquant : {d}");
    }

    #[test]
    fn diff_creation_complete() {
        let d = unified_diff("vide", "nouveau", "", "a\nb\n");
        assert!(d.contains("+a\n") && d.contains("+b\n"), "{d}");
        assert!(
            !d.lines().any(|l| l.starts_with('-') && !l.starts_with("---")),
            "aucune suppression attendue : {d}"
        );
    }

    #[test]
    fn diff_suppression_complete() {
        let d = unified_diff("avant", "vide", "a\nb\n", "");
        assert!(d.contains("-a\n") && d.contains("-b\n"), "{d}");
    }

    #[test]
    fn options_par_defaut() {
        let o = ApplyOptions::default();
        assert_eq!(o.field_manager, FIELD_MANAGER);
        assert!(o.server_side);
        assert!(!o.force);
        assert!(!o.dry_run);
        assert!(o.default_namespace.is_none());
    }

    #[test]
    fn serialisation_des_actions() {
        assert_eq!(serde_json::to_string(&ApplyAction::DryRun).unwrap_or_default(), "\"dryRun\"");
        assert_eq!(serde_json::to_string(&ApplyAction::Unchanged).unwrap_or_default(), "\"unchanged\"");
    }
}
