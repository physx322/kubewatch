//! Sessions `exec` interactives dans un conteneur (websocket SPDY/v5).
//!
//! Le serveur HTTP relaie les octets d'un websocket client vers [`ExecSession::stdin`]
//! et renvoie au client tout ce qui sort de [`ExecSession::output`]. Le
//! redimensionnement du terminal passe par [`ExecSession::resize`].

use std::sync::Arc;

use futures::channel::mpsc as futures_mpsc;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams, AttachedProcess, TerminalSize};
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cluster::ClusterHandle;
use crate::error::{Error, Result};
use crate::model::ResourceRef;

/// Interpréteurs testés, dans l'ordre, lorsqu'aucune commande n'est fournie.
pub const DEFAULT_SHELL_PROBE: &[&str] = &["/bin/bash", "/bin/sh", "/bin/ash"];

/// Taille des tampons de lecture/écriture (8 Kio).
const CHUNK_SIZE: usize = 8 * 1024;

/// Profondeur des files entre le serveur HTTP et le conteneur.
const CHANNEL_CAPACITY: usize = 256;

/// Poignée clonable permettant de piloter une session déjà démarrée.
///
/// Elle est fournie séparément de [`ExecSession`] pour que le serveur puisse
/// déplacer le récepteur de sortie dans une tâche dédiée tout en conservant la
/// possibilité de redimensionner ou d'interrompre la session.
#[derive(Clone)]
pub struct ExecControl {
    state: Arc<SessionState>,
}

impl ExecControl {
    /// Notifie le conteneur d'un changement de taille du terminal.
    pub fn resize(&self, cols: u16, rows: u16) {
        self.state.resize(cols, rows);
    }

    /// Interrompt la session et libère toutes les tâches associées.
    pub fn abort(&self) {
        self.state.abort();
    }

    /// Indique si la session a déjà été interrompue.
    pub fn is_aborted(&self) -> bool {
        self.state.process.lock().is_none()
    }
}

/// Session `exec` active sur un conteneur.
pub struct ExecSession {
    /// Canal d'entrée: tout ce qui y est poussé est écrit sur le stdin du conteneur.
    pub stdin: mpsc::Sender<Vec<u8>>,
    /// Canal de sortie: multiplexage de stdout et stderr du conteneur.
    pub output: mpsc::Receiver<Vec<u8>>,
    /// Poignée de pilotage clonable (redimensionnement, interruption).
    pub control: ExecControl,
}

impl ExecSession {
    /// Notifie le conteneur d'un changement de taille du terminal.
    ///
    /// Sans option TTY, l'appel est simplement ignoré.
    pub fn resize(&self, cols: u16, rows: u16) {
        self.control.resize(cols, rows);
    }

    /// Interrompt la session et libère toutes les tâches associées.
    pub fn abort(&self) {
        self.control.abort();
    }

    /// Renvoie une poignée de pilotage clonable.
    pub fn control(&self) -> ExecControl {
        self.control.clone()
    }
}

/// État partagé entre la session et ses poignées de pilotage.
struct SessionState {
    process: Mutex<Option<AttachedProcess>>,
    resize_tx: Mutex<Option<futures_mpsc::Sender<TerminalSize>>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl SessionState {
    fn resize(&self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        let mut guard = self.resize_tx.lock();
        if let Some(tx) = guard.as_mut() {
            // Une file pleine signifie simplement qu'un redimensionnement plus
            // récent est déjà en attente: on peut ignorer celui-ci.
            if let Err(err) = tx.try_send(TerminalSize {
                width: cols,
                height: rows,
            }) {
                if err.is_disconnected() {
                    *guard = None;
                }
            }
        }
    }

    fn abort(&self) {
        // Couper d'abord la connexion distante…
        if let Some(process) = self.process.lock().take() {
            process.abort();
        }
        // …puis les pompes locales.
        for task in self.tasks.lock().drain(..) {
            task.abort();
        }
        let _ = self.resize_tx.lock().take();
    }
}

impl Drop for SessionState {
    fn drop(&mut self) {
        self.abort();
    }
}

/// Démarre une session `exec` dans un conteneur.
///
/// Une `command` vide déclenche la détection automatique d'un interpréteur
/// parmi [`DEFAULT_SHELL_PROBE`].
pub async fn start(
    h: &ClusterHandle,
    pod: &ResourceRef,
    container: Option<&str>,
    command: Vec<String>,
    tty: bool,
) -> Result<ExecSession> {
    let name = pod.name.trim();
    if name.is_empty() {
        return Err(Error::Invalid("nom de pod vide".to_string()));
    }
    let namespace = pod
        .namespace
        .as_ref()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string())
        .unwrap_or_else(|| h.default_namespace.clone());

    let api: Api<Pod> = Api::namespaced(h.client.clone(), &namespace);

    let command = normalize_command(command);

    // Avec un TTY, stdout et stderr sont fusionnés par le serveur: demander
    // stderr en plus ferait rejeter la requête.
    let mut params = AttachParams::default()
        .stdin(true)
        .stdout(true)
        .stderr(!tty)
        .tty(tty)
        .max_stdin_buf_size(CHUNK_SIZE)
        .max_stdout_buf_size(CHUNK_SIZE)
        .max_stderr_buf_size(CHUNK_SIZE);

    if let Some(c) = container.map(str::trim).filter(|c| !c.is_empty()) {
        params = params.container(c);
    }

    let mut process = api.exec(name, command, &params).await?;

    let stdin_writer = process.stdin();
    let stdout_reader = process.stdout();
    let stderr_reader = process.stderr();
    let resize_tx = process.terminal_size();

    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(CHANNEL_CAPACITY);
    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(CHANNEL_CAPACITY);

    let mut tasks: Vec<JoinHandle<()>> = Vec::with_capacity(3);

    if let Some(mut writer) = stdin_writer {
        tasks.push(tokio::spawn(async move {
            while let Some(chunk) = stdin_rx.recv().await {
                if chunk.is_empty() {
                    continue;
                }
                if writer.write_all(&chunk).await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
            // La fermeture du canal vaut EOF pour le processus distant.
            let _ = writer.shutdown().await;
        }));
    }

    if let Some(reader) = stdout_reader {
        tasks.push(spawn_pump(reader, output_tx.clone()));
    }
    if let Some(reader) = stderr_reader {
        tasks.push(spawn_pump(reader, output_tx.clone()));
    }
    drop(output_tx);

    let state = Arc::new(SessionState {
        process: Mutex::new(Some(process)),
        resize_tx: Mutex::new(resize_tx),
        tasks: Mutex::new(tasks),
    });

    Ok(ExecSession {
        stdin: stdin_tx,
        output: output_rx,
        control: ExecControl { state },
    })
}

/// Pompe un flux de sortie du conteneur vers le canal de sortie de la session.
fn spawn_pump<R>(mut reader: R, tx: mpsc::Sender<Vec<u8>>) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buf = vec![0u8; CHUNK_SIZE];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

/// Normalise la commande demandée; une commande vide devient une détection
/// d'interpréteur basée sur [`DEFAULT_SHELL_PROBE`].
fn normalize_command(command: Vec<String>) -> Vec<String> {
    let cleaned: Vec<String> = command
        .into_iter()
        .filter(|c| !c.trim().is_empty())
        .collect();

    if cleaned.is_empty() {
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            shell_probe_script(),
        ]
    } else {
        cleaned
    }
}

/// Script `sh` qui essaie chaque interpréteur de [`DEFAULT_SHELL_PROBE`] puis
/// retombe sur le comportement générique `bash` sinon `sh`.
fn shell_probe_script() -> String {
    let mut script = String::new();
    for shell in DEFAULT_SHELL_PROBE {
        script.push_str("[ -x ");
        script.push_str(shell);
        script.push_str(" ] && exec ");
        script.push_str(shell);
        script.push_str("; ");
    }
    script.push_str("command -v bash >/dev/null 2>&1 && exec bash || exec sh");
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commande_vide_declenche_la_detection() {
        let c = normalize_command(Vec::new());
        assert_eq!(c.len(), 3);
        assert_eq!(c[0], "/bin/sh");
        assert_eq!(c[1], "-c");
        assert!(c[2].contains("exec bash"));
        assert!(c[2].ends_with("exec sh"));
    }

    #[test]
    fn commande_blanche_traitee_comme_vide() {
        let c = normalize_command(vec!["  ".to_string(), "\t".to_string()]);
        assert_eq!(c[0], "/bin/sh");
    }

    #[test]
    fn commande_explicite_conservee() {
        let c = normalize_command(vec!["ls".to_string(), "-la".to_string()]);
        assert_eq!(c, vec!["ls".to_string(), "-la".to_string()]);
    }

    #[test]
    fn script_couvre_tous_les_interpreteurs() {
        let s = shell_probe_script();
        for shell in DEFAULT_SHELL_PROBE {
            assert!(s.contains(shell), "{shell} absent de {s}");
        }
    }

    #[test]
    fn interpreteurs_par_defaut_non_vides() {
        assert!(DEFAULT_SHELL_PROBE.contains(&"/bin/sh"));
        assert!(DEFAULT_SHELL_PROBE.iter().all(|s| s.starts_with('/')));
    }
}
