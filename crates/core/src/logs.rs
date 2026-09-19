//! Journaux (logs) des conteneurs: instantané ponctuel et diffusion continue.
//!
//! On passe ici par l'API typée `Api<Pod>` — et non par `DynamicObject` — car les
//! sous-ressources `log` / `log_stream` ne sont exposées que pour les pods.

use std::pin::Pin;

use futures::stream::{Stream, StreamExt};
use futures::AsyncBufReadExt;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, LogParams};
use serde::{Deserialize, Serialize};

use crate::cluster::ClusterHandle;
use crate::error::{Error, Result};
use crate::model::ResourceRef;

/// Nombre de lignes renvoyées par défaut lorsque l'appelant n'impose rien.
pub const DEFAULT_TAIL_LINES: i64 = 500;

/// Plafond de taille pour un instantané, afin de ne jamais charger un journal
/// gigantesque en mémoire (10 Mio).
const SNAPSHOT_LIMIT_BYTES: i64 = 10 * 1024 * 1024;

/// Flux de lignes de journal renvoyé par [`stream`].
pub type LogStream = Pin<Box<dyn Stream<Item = Result<String>> + Send>>;

/// Options de lecture des journaux d'un conteneur.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogOptions {
    /// Conteneur ciblé; `None` = conteneur unique du pod.
    #[serde(default)]
    pub container: Option<String>,
    /// Suivre le flux en continu (`kubectl logs -f`).
    #[serde(default)]
    pub follow: bool,
    /// Nombre de lignes à récupérer depuis la fin du journal.
    #[serde(default)]
    pub tail_lines: Option<i64>,
    /// Ne renvoyer que les lignes des N dernières secondes.
    #[serde(default)]
    pub since_seconds: Option<i64>,
    /// Préfixer chaque ligne d'un horodatage RFC3339.
    #[serde(default)]
    pub timestamps: bool,
    /// Lire le journal de l'instance précédente du conteneur (après un crash).
    #[serde(default)]
    pub previous: bool,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            container: None,
            follow: false,
            tail_lines: Some(DEFAULT_TAIL_LINES),
            since_seconds: None,
            timestamps: false,
            previous: false,
        }
    }
}

impl LogOptions {
    /// Construit les paramètres kube correspondants.
    ///
    /// `follow` n'est jamais activé avec `previous`: l'API du serveur rejette la
    /// combinaison (le conteneur précédent est par définition terminé).
    fn to_params(&self, follow: bool, limit_bytes: Option<i64>) -> LogParams {
        let container = self
            .container
            .as_ref()
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .map(|c| c.to_string());

        LogParams {
            container,
            follow: follow && !self.previous,
            limit_bytes,
            pretty: false,
            previous: self.previous,
            since_seconds: self.since_seconds.filter(|s| *s > 0),
            since_time: None,
            tail_lines: self.tail_lines.filter(|n| *n >= 0),
            timestamps: self.timestamps,
        }
    }
}

/// Résout l'API pods à utiliser pour la référence donnée.
fn pods_api(h: &ClusterHandle, pod: &ResourceRef) -> Result<Api<Pod>> {
    if pod.name.trim().is_empty() {
        return Err(Error::Invalid("nom de pod vide".to_string()));
    }
    let namespace = pod
        .namespace
        .as_ref()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string())
        .unwrap_or_else(|| h.default_namespace.clone());

    Ok(Api::namespaced(h.client.clone(), &namespace))
}

/// Récupère un instantané du journal (pas de suivi continu).
pub async fn snapshot(h: &ClusterHandle, pod: &ResourceRef, o: &LogOptions) -> Result<String> {
    let api = pods_api(h, pod)?;
    let params = o.to_params(false, Some(SNAPSHOT_LIMIT_BYTES));
    let text = api.logs(pod.name.trim(), &params).await?;
    Ok(text)
}

/// Ouvre un flux de lignes de journal.
///
/// Le flux se termine proprement (sans erreur) lorsque le conteneur s'arrête ou
/// que le pod disparaît: la connexion HTTP est alors coupée par l'API server et
/// l'on traite ce cas comme une fin de flux normale.
pub async fn stream(h: &ClusterHandle, pod: &ResourceRef, o: LogOptions) -> Result<LogStream> {
    let api = pods_api(h, pod)?;
    let params = o.to_params(true, None);
    let reader = api.log_stream(pod.name.trim(), &params).await?;

    // On encapsule immédiatement dans un flux `Unpin` afin de pouvoir le piloter
    // depuis `unfold` sans contrainte de projection.
    let lines: Pin<Box<dyn Stream<Item = std::io::Result<String>> + Send>> =
        Box::pin(reader.lines());

    let out = futures::stream::unfold(Some(lines), |state| async move {
        let mut lines = match state {
            Some(lines) => lines,
            None => return None,
        };
        match lines.next().await {
            Some(Ok(line)) => Some((Ok(line), Some(lines))),
            // Coupure de connexion = fin de flux normale (pod supprimé, conteneur terminé).
            Some(Err(err)) if is_clean_disconnect(&err) => None,
            // Toute autre erreur est remontée une fois, puis le flux se termine.
            Some(Err(err)) => Some((Err(Error::from(err)), None)),
            None => None,
        }
    });

    Ok(Box::pin(out))
}

/// Détermine si une erreur d'E/S traduit une simple fermeture de connexion.
fn is_clean_disconnect(err: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    if matches!(
        err.kind(),
        UnexpectedEof | ConnectionAborted | ConnectionReset | BrokenPipe | NotConnected
    ) {
        return true;
    }
    // hyper enveloppe certaines fins de corps dans une erreur « Other ».
    let text = err.to_string().to_ascii_lowercase();
    text.contains("incompletemessage")
        || text.contains("incomplete message")
        || text.contains("connection closed")
        || text.contains("channel closed")
        || text.contains("body write aborted")
        || text.contains("end of file")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_par_defaut() {
        let o = LogOptions::default();
        assert_eq!(o.tail_lines, Some(DEFAULT_TAIL_LINES));
        assert!(!o.follow);
        assert!(!o.previous);
    }

    #[test]
    fn previous_desactive_le_suivi() {
        let o = LogOptions {
            previous: true,
            follow: true,
            ..Default::default()
        };
        let p = o.to_params(true, None);
        assert!(!p.follow, "follow doit être désactivé avec previous");
        assert!(p.previous);
    }

    #[test]
    fn conteneur_vide_ignore() {
        let o = LogOptions {
            container: Some("   ".to_string()),
            ..Default::default()
        };
        assert!(o.to_params(false, None).container.is_none());

        let o = LogOptions {
            container: Some(" web ".to_string()),
            ..Default::default()
        };
        assert_eq!(o.to_params(false, None).container.as_deref(), Some("web"));
    }

    #[test]
    fn valeurs_negatives_ignorees() {
        let o = LogOptions {
            tail_lines: Some(-5),
            since_seconds: Some(0),
            ..Default::default()
        };
        let p = o.to_params(false, None);
        assert_eq!(p.tail_lines, None);
        assert_eq!(p.since_seconds, None);
    }

    #[test]
    fn instantane_limite_la_taille() {
        let p = LogOptions::default().to_params(false, Some(SNAPSHOT_LIMIT_BYTES));
        assert_eq!(p.limit_bytes, Some(SNAPSHOT_LIMIT_BYTES));
    }

    #[test]
    fn deconnexions_propres_detectees() {
        use std::io::{Error as IoError, ErrorKind};
        assert!(is_clean_disconnect(&IoError::new(
            ErrorKind::UnexpectedEof,
            "eof"
        )));
        assert!(is_clean_disconnect(&IoError::other(
            "hyper::Error(IncompleteMessage)"
        )));
        assert!(!is_clean_disconnect(&IoError::new(
            ErrorKind::InvalidData,
            "utf8 invalide"
        )));
    }

    #[test]
    fn serialisation_camel_case() {
        let o = LogOptions {
            tail_lines: Some(10),
            since_seconds: Some(30),
            timestamps: true,
            ..Default::default()
        };
        let j = serde_json::to_string(&o).expect("sérialisation");
        assert!(j.contains("\"tailLines\":10"), "{j}");
        assert!(j.contains("\"sinceSeconds\":30"), "{j}");
        assert!(j.contains("\"timestamps\":true"), "{j}");
    }
}
