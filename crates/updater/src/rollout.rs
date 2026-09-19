//! Application d'une mise à jour sur une charge de travail, attente de la
//! convergence du déploiement et retour arrière.

use crate::error::{Error, Result};
use crate::model::{RolloutResult, RolloutStatus, UpdateFinding};
use crate::scan::watch_target;
use chrono::Utc;
use kubewatch_core::model::ResourceRef;
use kubewatch_core::{resource, ClusterHandle};
use std::time::Duration;

/// Période d'interrogation de l'état du déploiement.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Délai maximal d'attente de la convergence d'un déploiement.
const ROLLOUT_TIMEOUT: Duration = Duration::from_secs(300);

/// Nombre d'erreurs consécutives de lecture au-delà duquel on déclare l'échec.
const MAX_CONSECUTIVE_ERRORS: u32 = 5;

/// Construit une référence de ressource complète à partir d'un simple type.
///
/// Le type est résolu par la découverte du cluster (accepte « deploy », « Deployment »,
/// « deployments.apps »…). Le namespace n'est renseigné que pour les types namespacés,
/// avec repli sur le namespace par défaut du cluster.
pub(crate) fn resource_ref(
    h: &ClusterHandle,
    kind: &str,
    namespace: Option<&str>,
    name: &str,
) -> Result<ResourceRef> {
    let rk = h
        .resolve_kind(kind)
        .map_err(|e| Error::Core(format!("type « {kind} » non résolu: {e}")))?;

    let namespace = namespace.filter(|n| !n.is_empty());
    let ns = if rk.namespaced {
        Some(
            namespace
                .map(str::to_string)
                .unwrap_or_else(|| h.default_namespace.clone()),
        )
    } else {
        None
    };

    Ok(ResourceRef {
        group: rk.group,
        version: rk.version,
        kind: rk.kind,
        plural: rk.plural,
        namespace: ns,
        name: name.to_string(),
    })
}

/// Chemins JSON menant au `PodSpec` selon le type de charge de travail.
const POD_SPEC_PATHS: [&[&str]; 3] = [
    // Deployment, StatefulSet, DaemonSet, ReplicaSet, Job.
    &["spec", "template", "spec"],
    // CronJob.
    &["spec", "jobTemplate", "spec", "template", "spec"],
    // Pod nu.
    &["spec"],
];

/// Renvoie les conteneurs (initialisation puis applicatifs) d'un objet Kubernetes brut.
pub(crate) fn containers_of(obj: &serde_json::Value) -> Vec<&serde_json::Value> {
    for path in POD_SPEC_PATHS {
        let Some(pod_spec) = path.iter().try_fold(obj, |acc, key| acc.get(*key)) else {
            continue;
        };
        let mut out: Vec<&serde_json::Value> = Vec::new();
        for key in ["initContainers", "containers"] {
            if let Some(arr) = pod_spec.get(key).and_then(serde_json::Value::as_array) {
                out.extend(arr.iter());
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    Vec::new()
}

/// Extrait l'image d'un conteneur donné (ou du premier conteneur si aucun nom n'est fourni).
pub(crate) fn container_image(obj: &serde_json::Value, wanted: Option<&str>) -> Option<String> {
    let containers = containers_of(obj);
    let chosen = match wanted.filter(|w| !w.is_empty()) {
        Some(w) => containers
            .iter()
            .find(|c| c.get("name").and_then(serde_json::Value::as_str) == Some(w))?,
        None => containers.first()?,
    };
    chosen
        .get("image")
        .and_then(serde_json::Value::as_str)
        .filter(|i| !i.is_empty())
        .map(str::to_string)
}

/// Lit l'image courante d'un conteneur directement dans le cluster.
pub(crate) async fn current_container_image(
    h: &ClusterHandle,
    r: &ResourceRef,
    container: Option<&str>,
) -> Result<Option<String>> {
    let raw = resource::get_raw(h, r)
        .await
        .map_err(|e| Error::Core(format!("lecture de {}/{}: {e}", r.kind, r.name)))?;
    Ok(container_image(&raw, container))
}

/// Lit un entier signé à un chemin JSON donné.
fn field_i64(obj: &serde_json::Value, path: &[&str]) -> Option<i64> {
    path.iter()
        .try_fold(obj, |acc, key| acc.get(*key))?
        .as_i64()
}

/// Même chose, ramené à un `i32` (les compteurs de réplicas de Kubernetes).
fn field_i32(obj: &serde_json::Value, path: &[&str]) -> Option<i32> {
    field_i64(obj, path).map(|n| n as i32)
}

/// Calcule l'état d'avancement d'un déploiement à partir de l'objet brut.
///
/// Les DaemonSets exposent des compteurs différents (`numberReady`,
/// `desiredNumberScheduled`) des Deployments et StatefulSets (`readyReplicas`,
/// `updatedReplicas`, `replicas`). On vérifie aussi que le contrôleur a bien pris
/// en compte la dernière génération du manifeste, sans quoi les compteurs
/// décrivent encore l'ancienne version.
pub(crate) fn status_from_object(kind: &str, obj: &serde_json::Value) -> RolloutStatus {
    let (ready, updated, desired) = if kind.eq_ignore_ascii_case("DaemonSet") {
        (
            field_i32(obj, &["status", "numberReady"]).unwrap_or(0),
            field_i32(obj, &["status", "updatedNumberScheduled"]).unwrap_or(0),
            field_i32(obj, &["status", "desiredNumberScheduled"]).unwrap_or(0),
        )
    } else {
        let desired = field_i32(obj, &["spec", "replicas"])
            .or_else(|| field_i32(obj, &["status", "replicas"]))
            .unwrap_or(1);
        (
            field_i32(obj, &["status", "readyReplicas"]).unwrap_or(0),
            field_i32(obj, &["status", "updatedReplicas"]).unwrap_or(0),
            desired,
        )
    };

    // Tant que `observedGeneration` est en retard, le statut décrit l'ancienne révision.
    let observed = match (
        field_i64(obj, &["metadata", "generation"]),
        field_i64(obj, &["status", "observedGeneration"]),
    ) {
        (Some(generation), Some(observed)) => observed >= generation,
        _ => true,
    };

    let converged = if desired <= 0 {
        true
    } else {
        ready >= desired && updated >= desired
    };
    let available = observed && converged;

    let message = if !observed {
        format!(
            "{kind}: modification enregistrée, en attente de prise en compte par le contrôleur."
        )
    } else if desired <= 0 {
        format!("{kind}: aucun réplica attendu (mise à l'échelle à zéro).")
    } else if available {
        format!("{kind}: {ready}/{desired} réplicas prêts, {updated} à jour — déploiement terminé.")
    } else {
        format!(
            "{kind}: {ready}/{desired} réplicas prêts, {updated} à jour — déploiement en cours."
        )
    };

    RolloutStatus {
        ready_replicas: ready,
        updated_replicas: updated,
        desired_replicas: desired,
        available,
        message,
    }
}

/// État d'avancement du déploiement d'une charge de travail.
pub async fn status(
    h: &ClusterHandle,
    namespace: Option<&str>,
    kind: &str,
    name: &str,
) -> Result<RolloutStatus> {
    let r = resource_ref(h, kind, namespace, name)?;
    let raw = resource::get_raw(h, &r)
        .await
        .map_err(|e| Error::Core(format!("lecture de {}/{}: {e}", r.kind, r.name)))?;
    Ok(status_from_object(&r.kind, &raw))
}

/// Attend la convergence du déploiement et renvoie le statut à consigner.
///
/// Ne renvoie jamais d'erreur: l'image a déjà été modifiée dans le cluster, le
/// résultat doit décrire ce qui s'est passé, pas interrompre l'appelant. Les
/// statuts d'échec sont préfixés par [`RolloutResult::STATUS_FAILED`] pour que
/// [`RolloutResult::succeeded`] les reconnaisse.
async fn wait_for_rollout(h: &ClusterHandle, r: &ResourceRef, success: &str) -> String {
    let deadline = tokio::time::Instant::now() + ROLLOUT_TIMEOUT;
    let mut consecutive_errors: u32 = 0;

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        match resource::get_raw(h, r).await {
            Ok(obj) => {
                consecutive_errors = 0;
                let st = status_from_object(&r.kind, &obj);
                if st.available {
                    tracing::info!(ressource = %r.name, kind = %r.kind, etat = %st.message,
                        "déploiement convergé");
                    return success.to_string();
                }
            }
            Err(err) => {
                consecutive_errors += 1;
                tracing::warn!(ressource = %r.name, kind = %r.kind, erreur = %err,
                    tentatives = consecutive_errors,
                    "état du déploiement illisible");
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    return format!(
                        "{}: état du déploiement illisible",
                        RolloutResult::STATUS_FAILED
                    );
                }
            }
        }

        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(ressource = %r.name, kind = %r.kind,
                "délai de convergence dépassé");
            return format!(
                "{}: délai de convergence dépassé après {} s",
                RolloutResult::STATUS_FAILED,
                ROLLOUT_TIMEOUT.as_secs()
            );
        }
    }
}

/// Applique une détection de mise à jour: remplace l'image puis attend la convergence.
pub async fn apply_finding(h: &ClusterHandle, f: &UpdateFinding) -> Result<RolloutResult> {
    let r = resource_ref(
        h,
        &f.target_kind,
        f.target_namespace.as_deref(),
        &f.target_name,
    )?;
    let container = f.container.as_deref();

    let new_image = match f.available_image.as_deref().map(str::trim) {
        Some(i) if !i.is_empty() => i.to_string(),
        _ => {
            return Err(Error::Invalid(format!(
                "la détection {} ne porte aucune image cible exploitable",
                f.id
            )))
        }
    };

    // Image en place avant modification: celle relevée à la détection, sinon
    // relue dans le cluster pour que le retour arrière reste possible.
    let previous_image = match f.current_image.as_deref().map(str::trim) {
        Some(i) if !i.is_empty() => Some(i.to_string()),
        _ => current_container_image(h, &r, container)
            .await
            .unwrap_or(None),
    };

    if previous_image.as_deref() == Some(new_image.as_str()) {
        return Err(Error::Invalid(format!(
            "l'image {new_image} est déjà celle en place sur {}/{}",
            r.kind, r.name
        )));
    }

    resource::set_image(h, &r, container, &new_image)
        .await
        .map_err(|e| {
            Error::Core(format!(
                "mise à jour de l'image de {}/{}: {e}",
                r.kind, r.name
            ))
        })?;

    tracing::info!(
        cluster = %h.name, kind = %r.kind, ressource = %r.name,
        image = %new_image, "image mise à jour, attente de la convergence"
    );

    let status = wait_for_rollout(h, &r, RolloutResult::STATUS_APPLIED).await;

    Ok(RolloutResult {
        finding_id: f.id.clone(),
        target: watch_target(r.namespace.clone(), &r.kind, &r.name),
        previous_image,
        new_image,
        applied_at: Utc::now(),
        status,
    })
}

/// Revient à la révision précédente d'une charge de travail.
pub async fn rollback(
    h: &ClusterHandle,
    namespace: Option<&str>,
    kind: &str,
    name: &str,
) -> Result<RolloutResult> {
    let r = resource_ref(h, kind, namespace, name)?;

    let previous_image = current_container_image(h, &r, None).await.unwrap_or(None);

    resource::rollback(h, &r)
        .await
        .map_err(|e| Error::Core(format!("retour arrière de {}/{}: {e}", r.kind, r.name)))?;

    tracing::info!(
        cluster = %h.name, kind = %r.kind, ressource = %r.name,
        "retour arrière demandé, attente de la convergence"
    );

    let status = wait_for_rollout(h, &r, RolloutResult::STATUS_ROLLED_BACK).await;

    let new_image = current_container_image(h, &r, None)
        .await
        .unwrap_or(None)
        .or_else(|| previous_image.clone())
        .unwrap_or_default();

    Ok(RolloutResult {
        // Convention de `RolloutResult`: chaîne vide pour un retour arrière manuel.
        finding_id: String::new(),
        target: watch_target(r.namespace.clone(), &r.kind, &r.name),
        previous_image,
        new_image,
        applied_at: Utc::now(),
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn conteneurs_d_un_deployment() {
        let obj = json!({
            "kind": "Deployment",
            "spec": { "template": { "spec": {
                "initContainers": [{ "name": "init-db", "image": "busybox:1.36" }],
                "containers": [
                    { "name": "app", "image": "ghcr.io/acme/app:v1.2.3" },
                    { "name": "sidecar", "image": "envoyproxy/envoy:v1.31.0" }
                ]
            }}}
        });
        let containers = containers_of(&obj);
        assert_eq!(containers.len(), 3);
        assert_eq!(
            container_image(&obj, None).as_deref(),
            Some("busybox:1.36"),
            "sans nom, on prend le premier conteneur listé"
        );
        assert_eq!(
            container_image(&obj, Some("sidecar")).as_deref(),
            Some("envoyproxy/envoy:v1.31.0")
        );
        assert_eq!(container_image(&obj, Some("absent")), None);
    }

    #[test]
    fn conteneurs_d_un_cronjob() {
        let obj = json!({
            "kind": "CronJob",
            "spec": { "jobTemplate": { "spec": { "template": { "spec": {
                "containers": [{ "name": "backup", "image": "acme/backup:2.1.0" }]
            }}}}}
        });
        assert_eq!(
            container_image(&obj, Some("backup")).as_deref(),
            Some("acme/backup:2.1.0")
        );
    }

    #[test]
    fn conteneurs_d_un_pod_nu() {
        let obj = json!({
            "kind": "Pod",
            "spec": { "containers": [{ "name": "c", "image": "nginx:1.27" }] }
        });
        assert_eq!(container_image(&obj, None).as_deref(), Some("nginx:1.27"));
    }

    #[test]
    fn statut_deployment_converge() {
        let obj = json!({
            "metadata": { "generation": 4 },
            "spec": { "replicas": 3 },
            "status": { "observedGeneration": 4, "readyReplicas": 3, "updatedReplicas": 3, "replicas": 3 }
        });
        let st = status_from_object("Deployment", &obj);
        assert!(st.available);
        assert_eq!(st.ready_replicas, 3);
        assert_eq!(st.updated_replicas, 3);
        assert_eq!(st.desired_replicas, 3);
        assert!(st.message.contains("terminé"));
    }

    #[test]
    fn statut_deployment_en_cours() {
        let obj = json!({
            "metadata": { "generation": 5 },
            "spec": { "replicas": 3 },
            "status": { "observedGeneration": 5, "readyReplicas": 1, "updatedReplicas": 1 }
        });
        let st = status_from_object("Deployment", &obj);
        assert!(!st.available);
        assert!(st.message.contains("en cours"));
    }

    #[test]
    fn statut_generation_non_observee() {
        // Les compteurs décrivent encore l'ancienne révision: pas de convergence.
        let obj = json!({
            "metadata": { "generation": 6 },
            "spec": { "replicas": 2 },
            "status": { "observedGeneration": 5, "readyReplicas": 2, "updatedReplicas": 2 }
        });
        let st = status_from_object("Deployment", &obj);
        assert!(!st.available);
        assert!(st.message.contains("attente"));
    }

    #[test]
    fn statut_daemonset() {
        let obj = json!({
            "status": { "numberReady": 4, "updatedNumberScheduled": 4, "desiredNumberScheduled": 4 }
        });
        let st = status_from_object("DaemonSet", &obj);
        assert!(st.available);
        assert_eq!(st.desired_replicas, 4);

        let partiel = json!({
            "status": { "numberReady": 2, "updatedNumberScheduled": 3, "desiredNumberScheduled": 4 }
        });
        assert!(!status_from_object("DaemonSet", &partiel).available);
    }

    #[test]
    fn statut_mise_a_l_echelle_a_zero() {
        let obj = json!({ "spec": { "replicas": 0 }, "status": {} });
        let st = status_from_object("Deployment", &obj);
        assert!(st.available);
        assert_eq!(st.desired_replicas, 0);
        assert!(st.message.contains("zéro"));
    }
}
