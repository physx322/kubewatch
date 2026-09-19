//! Lecture des évènements récents du cluster (`core/v1 Event`).
//!
//! L'API serveur ne trie pas les évènements : on en récupère une fenêtre plus
//! large que demandé, on trie du plus récent au plus ancien, puis on tronque.

use k8s_openapi::api::core::v1::Event;
use kube::api::{Api, ListParams};

use crate::cluster::ClusterHandle;
use crate::error::Result;
use crate::model::{EventSummary, ResourceRef};
use crate::resource::jiff_to_chrono;

/// Plafond absolu de lignes renvoyées.
const MAX_LIMIT: u32 = 1_000;
/// Plafond du nombre d'évènements récupérés avant tri.
const FETCH_CAP: u32 = 2_000;

/// Évènements les plus récents, éventuellement restreints à un namespace
/// et/ou à un objet précis.
pub async fn recent(
    h: &ClusterHandle,
    namespace: Option<&str>,
    involved: Option<&ResourceRef>,
    limit: u32,
) -> Result<Vec<EventSummary>> {
    let limit = limit.clamp(1, MAX_LIMIT);

    // Le namespace de l'objet visé prime sur celui passé en paramètre.
    let ns = involved
        .and_then(|r| r.namespace.as_deref())
        .or(namespace)
        .filter(|s| !s.is_empty() && *s != "*");

    let api: Api<Event> = match ns {
        Some(n) => Api::namespaced(h.client.clone(), n),
        None => Api::all(h.client.clone()),
    };

    let mut selectors: Vec<String> = Vec::new();
    if let Some(r) = involved {
        if !r.name.is_empty() {
            selectors.push(format!("involvedObject.name={}", r.name));
        }
        if !r.kind.is_empty() {
            selectors.push(format!("involvedObject.kind={}", r.kind));
        }
        if let Some(n) = r.namespace.as_deref().filter(|s| !s.is_empty()) {
            selectors.push(format!("involvedObject.namespace={n}"));
        }
    }

    // Sans filtre d'objet, on élargit la fenêtre pour que le tri soit pertinent.
    let fetch = if selectors.is_empty() {
        limit.saturating_mul(5).min(FETCH_CAP).max(limit)
    } else {
        limit
    };

    let mut lp = ListParams::default().limit(fetch);
    if !selectors.is_empty() {
        lp = lp.fields(&selectors.join(","));
    }

    let page = api.list(&lp).await?;
    let mut out: Vec<EventSummary> = page.items.iter().map(to_summary).collect();
    sort_recent_first(&mut out);
    out.truncate(limit as usize);
    Ok(out)
}

/// Tri décroissant : dernière occurrence, puis première, puis nom.
fn sort_recent_first(events: &mut [EventSummary]) {
    events.sort_by(|a, b| {
        b.last_seen
            .cmp(&a.last_seen)
            .then_with(|| b.first_seen.cmp(&a.first_seen))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Convertit un évènement Kubernetes en résumé exposable par l'API HTTP.
fn to_summary(e: &Event) -> EventSummary {
    let last = e
        .last_timestamp
        .as_ref()
        .map(|t| t.0)
        .and_then(jiff_to_chrono)
        .or_else(|| {
            e.event_time
                .as_ref()
                .map(|t| t.0)
                .and_then(jiff_to_chrono)
        })
        .or_else(|| {
            e.series
                .as_ref()
                .and_then(|s| s.last_observed_time.as_ref())
                .map(|t| t.0)
                .and_then(jiff_to_chrono)
        })
        .or_else(|| {
            e.metadata
                .creation_timestamp
                .as_ref()
                .map(|t| t.0)
                .and_then(jiff_to_chrono)
        });

    let first = e
        .first_timestamp
        .as_ref()
        .map(|t| t.0)
        .and_then(jiff_to_chrono)
        .or(last);

    let source = e
        .source
        .as_ref()
        .and_then(
            |s| match (s.component.as_deref(), s.host.as_deref()) {
                (Some(c), Some(host)) => Some(format!("{c}, {host}")),
                (Some(c), None) => Some(c.to_string()),
                (None, Some(host)) => Some(host.to_string()),
                (None, None) => None,
            },
        )
        .or_else(|| {
            e.reporting_component
                .clone()
                .filter(|s| !s.trim().is_empty())
        });

    EventSummary {
        namespace: e.metadata.namespace.clone(),
        name: e.metadata.name.clone().unwrap_or_default(),
        reason: e.reason.clone().unwrap_or_default(),
        message: e.message.clone().unwrap_or_default(),
        type_: e
            .type_
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Normal".to_string()),
        count: e
            .count
            .or_else(|| e.series.as_ref().and_then(|s| s.count))
            .unwrap_or(1),
        first_seen: first,
        last_seen: last,
        involved_kind: e.involved_object.kind.clone(),
        involved_name: e.involved_object.name.clone(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(name: &str, last: &str, reason: &str) -> Event {
        serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "Event",
            "metadata": {"name": name, "namespace": "prod"},
            "involvedObject": {"kind": "Pod", "name": "web-1", "namespace": "prod"},
            "reason": reason,
            "message": "message de test",
            "type": "Warning",
            "count": 3,
            "firstTimestamp": "2024-05-01T10:00:00Z",
            "lastTimestamp": last,
            "source": {"component": "kubelet", "host": "node-a"}
        }))
        .expect("évènement de test valide")
    }

    #[test]
    fn conversion_en_resume() {
        let s = to_summary(&event("e1", "2024-05-01T12:00:00Z", "BackOff"));
        assert_eq!(s.name, "e1");
        assert_eq!(s.namespace.as_deref(), Some("prod"));
        assert_eq!(s.reason, "BackOff");
        assert_eq!(s.type_, "Warning");
        assert_eq!(s.count, 3);
        assert_eq!(s.involved_kind.as_deref(), Some("Pod"));
        assert_eq!(s.involved_name.as_deref(), Some("web-1"));
        assert_eq!(s.source.as_deref(), Some("kubelet, node-a"));
        let last = s.last_seen.expect("horodatage présent");
        assert_eq!(last.to_rfc3339(), "2024-05-01T12:00:00+00:00");
        assert!(s.first_seen.expect("première occurrence") < last);
    }

    #[test]
    fn tri_du_plus_recent_au_plus_ancien() {
        let mut list = vec![
            to_summary(&event("a", "2024-05-01T10:00:00Z", "R1")),
            to_summary(&event("c", "2024-05-01T14:00:00Z", "R3")),
            to_summary(&event("b", "2024-05-01T12:00:00Z", "R2")),
        ];
        sort_recent_first(&mut list);
        let noms: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(noms, vec!["c", "b", "a"]);
    }
}
