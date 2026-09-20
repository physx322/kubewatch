//! Flux vers l'interface : journaux et sessions interactives.
//!
//! Chaque flux reçoit un identifiant ; l'interface le rappelle pour envoyer des
//! octets, redimensionner le terminal ou interrompre le flux. Les lignes de
//! journal sont regroupées par lots (au plus toutes les 40 ms) pour ne pas
//! saturer le pont IPC quand un pod est bavard.

use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use futures::StreamExt;
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use kubewatch_core::exec::{self, ExecSession};
use kubewatch_core::logs::{self, LogOptions};
use kubewatch_core::model::ResourceRef;

use crate::state::{AppState, StreamEntry};

/// Délai maximal de rétention d'un lot de lignes.
const FLUSH_EVERY: Duration = Duration::from_millis(40);
/// Taille de lot au-delà de laquelle on envoie sans attendre.
const MAX_BATCH: usize = 500;

/// Évènement d'un flux de journaux.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LogEvent {
    /// Un lot de lignes.
    Lines {
        /// Lignes, dans l'ordre.
        lines: Vec<String>,
    },
    /// Fin du flux ; `error` explique une interruption anormale.
    Ended {
        /// Cause, le cas échéant.
        error: Option<String>,
    },
}

/// Évènement d'une session interactive.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ExecEvent {
    /// Octets produits par le conteneur (stdout et stderr mêlés), en base64.
    Output {
        /// Données encodées.
        data: String,
    },
    /// Session close.
    Ended {
        /// Raison, le cas échéant.
        message: Option<String>,
    },
}

#[tauri::command]
pub async fn start_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    cluster: String,
    pod: ResourceRef,
    opts: LogOptions,
    on_event: Channel<LogEvent>,
) -> Result<u64, String> {
    let h = state.handle(&cluster)?;
    let id = state.next_id();
    let task = tokio::spawn(async move {
        pump_logs(&h, &pod, opts, &on_event).await;
        if let Some(st) = app.try_state::<AppState>() {
            st.streams.lock().remove(&id);
        }
    });
    state.streams.lock().insert(
        id,
        StreamEntry {
            task: task.abort_handle(),
            stdin: None,
            control: None,
        },
    );
    Ok(id)
}

async fn pump_logs(
    h: &kubewatch_core::ClusterHandle,
    pod: &ResourceRef,
    opts: LogOptions,
    out: &Channel<LogEvent>,
) {
    let mut stream = match logs::stream(h, pod, opts).await {
        Ok(s) => s,
        Err(e) => {
            let _ = out.send(LogEvent::Ended {
                error: Some(format!("journaux de {} indisponibles : {e}", pod.display())),
            });
            return;
        }
    };
    let mut buf: Vec<String> = Vec::new();
    let mut error = None;
    loop {
        let next = if buf.is_empty() {
            stream.next().await
        } else {
            match tokio::time::timeout(FLUSH_EVERY, stream.next()).await {
                Ok(next) => next,
                Err(_) => {
                    if out
                        .send(LogEvent::Lines {
                            lines: std::mem::take(&mut buf),
                        })
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
            }
        };
        match next {
            Some(Ok(line)) => {
                buf.push(line);
                if buf.len() >= MAX_BATCH
                    && out
                        .send(LogEvent::Lines {
                            lines: std::mem::take(&mut buf),
                        })
                        .is_err()
                {
                    return;
                }
            }
            Some(Err(e)) => {
                error = Some(format!("flux de journaux interrompu : {e}"));
                break;
            }
            None => break,
        }
    }
    if !buf.is_empty() {
        let _ = out.send(LogEvent::Lines { lines: buf });
    }
    let _ = out.send(LogEvent::Ended { error });
}

#[tauri::command]
pub fn stop_logs(state: State<'_, AppState>, id: u64) -> bool {
    state.abort_stream(id)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_exec(
    app: AppHandle,
    state: State<'_, AppState>,
    cluster: String,
    pod: ResourceRef,
    container: Option<String>,
    command: Vec<String>,
    cols: u16,
    rows: u16,
    on_event: Channel<ExecEvent>,
) -> Result<u64, String> {
    let h = state.handle(&cluster)?;
    let container = container.filter(|c| !c.trim().is_empty());
    let session = exec::start(&h, &pod, container.as_deref(), command, true)
        .await
        .map_err(|e| format!("session sur {} impossible : {e}", pod.display()))?;
    let ExecSession {
        stdin,
        mut output,
        control,
    } = session;
    if cols > 0 && rows > 0 {
        control.resize(cols, rows);
    }
    let id = state.next_id();
    let control_task = control.clone();
    let task = tokio::spawn(async move {
        while let Some(data) = output.recv().await {
            if on_event
                .send(ExecEvent::Output {
                    data: B64.encode(&data),
                })
                .is_err()
            {
                break;
            }
        }
        control_task.abort();
        let _ = on_event.send(ExecEvent::Ended { message: None });
        if let Some(st) = app.try_state::<AppState>() {
            st.streams.lock().remove(&id);
        }
    });
    state.streams.lock().insert(
        id,
        StreamEntry {
            task: task.abort_handle(),
            stdin: Some(stdin),
            control: Some(control),
        },
    );
    Ok(id)
}

#[tauri::command]
pub async fn exec_input(state: State<'_, AppState>, id: u64, data: String) -> Result<(), String> {
    let bytes = B64
        .decode(data.as_bytes())
        .map_err(|e| format!("données de session illisibles : {e}"))?;
    let tx = state
        .streams
        .lock()
        .get(&id)
        .and_then(|e| e.stdin.clone())
        .ok_or_else(|| "aucune session interactive sous cet identifiant".to_string())?;
    tx.send(bytes)
        .await
        .map_err(|_| "la session interactive est close".to_string())
}

#[tauri::command]
pub fn exec_resize(state: State<'_, AppState>, id: u64, cols: u16, rows: u16) -> bool {
    let control = state
        .streams
        .lock()
        .get(&id)
        .and_then(|e| e.control.clone());
    match control {
        Some(c) => {
            c.resize(cols, rows);
            true
        }
        None => false,
    }
}

#[tauri::command]
pub fn stop_exec(state: State<'_, AppState>, id: u64) -> bool {
    state.abort_stream(id)
}
