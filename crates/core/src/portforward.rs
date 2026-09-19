//! Redirection de ports (`port-forward`) d'un pod vers un port TCP local.
//!
//! Un écouteur TCP local est ouvert; chaque connexion entrante ouvre sa propre
//! redirection vers le pod, puis les deux extrémités sont recopiées l'une dans
//! l'autre. Cela permet plusieurs connexions simultanées sur le même port local.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::api::core::v1::Pod;
use kube::api::Api;
use parking_lot::Mutex;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::cluster::ClusterHandle;
use crate::error::{Error, Result};
use crate::model::ResourceRef;

/// Délai maximal d'attente du message d'erreur renvoyé par l'API server à la
/// fermeture d'une redirection.
const ERROR_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Redirection de port active. La redirection s'arrête à l'appel de [`ForwardHandle::stop`]
/// ou lorsque la dernière poignée est libérée. 
#[derive(Clone)]
pub struct ForwardHandle {
    /// Port local réellement attribué (jamais 0).
    pub local_port: u16,
    /// Port écouté dans le pod.
    pub remote_port: u16,
    /// Pod ciblé.
    pub pod: ResourceRef,
    inner: Arc<ForwardInner>,
}

impl ForwardHandle {
    /// Arrête la boucle d'acceptation et coupe les connexions en cours.
    pub fn stop(&self) {
        self.inner.stop();
    }

    /// Indique si la redirection accepte encore des connexions.
    pub fn is_running(&self) -> bool {
        !self.inner.stopped.load(Ordering::SeqCst)
    }

    /// Adresse locale complète à laquelle se connecter.
    pub fn local_addr(&self) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, self.local_port))
    }
}

impl std::fmt::Debug for ForwardHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForwardHandle")
            .field("local_port", &self.local_port)
            .field("remote_port", &self.remote_port)
            .field("pod", &self.pod.name)
            .field("running", &self.is_running())
            .finish()
    }
}

struct ForwardInner {
    shutdown: Arc<Notify>,
    stopped: AtomicBool,
    // La boucle d'acceptation ne détient qu'un `Arc<Notify>`: pas de cycle de
    // références, donc `Drop` est bien exécuté à la libération de la poignée.
    task: Mutex<Option<JoinHandle<()>>>,
}

impl ForwardInner {
    fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        // `notify_one` conserve une autorisation même si la boucle n'attend pas
        // encore: l'arrêt ne peut donc pas être perdu.
        self.shutdown.notify_one();
    }
}

impl Drop for ForwardInner {
    fn drop(&mut self) {
        self.stop();
        if let Some(task) = self.task.lock().take() {
            task.abort();
        }
    }
}

/// Ouvre une redirection du port `remote_port` du pod vers `local_port`.
///
/// `local_port == 0` demande à l'OS de choisir un port libre; le port réellement
/// attribué est renvoyé dans [`ForwardHandle::local_port`].
pub async fn forward(
    h: &ClusterHandle,
    pod: &ResourceRef,
    local_port: u16,
    remote_port: u16,
) -> Result<ForwardHandle> {
    let name = pod.name.trim().to_string();
    if name.is_empty() {
        return Err(Error::Invalid("nom de pod vide".to_string()));
    }
    if remote_port == 0 {
        return Err(Error::Invalid(
            "port distant invalide: attendu entre 1 et 65535".to_string(),
        ));
    }

    let namespace = pod
        .namespace
        .as_ref()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string())
        .unwrap_or_else(|| h.default_namespace.clone());

    let api: Api<Pod> = Api::namespaced(h.client.clone(), &namespace);

    // Vérification préalable: un pod inexistant ou non démarré donnerait des
    // erreurs incompréhensibles à chaque connexion.
    let target = api.get(&name).await?;
    let phase = target
        .status
        .as_ref()
        .and_then(|s| s.phase.clone())
        .unwrap_or_else(|| "Unknown".to_string());
    if phase != "Running" {
        return Err(Error::Invalid(format!(
            "le pod {namespace}/{name} n'est pas en cours d'exécution (phase: {phase})"
        )));
    }

    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, local_port))).await?;
    let bound_port = listener.local_addr()?.port();

    let shutdown = Arc::new(Notify::new());
    let task = tokio::spawn(accept_loop(
        api,
        name,
        namespace,
        remote_port,
        listener,
        Arc::clone(&shutdown),
    ));

    Ok(ForwardHandle {
        local_port: bound_port,
        remote_port,
        pod: pod.clone(),
        inner: Arc::new(ForwardInner {
            shutdown,
            stopped: AtomicBool::new(false),
            task: Mutex::new(Some(task)),
        }),
    })
}

/// Boucle d'acceptation des connexions locales.
async fn accept_loop(
    api: Api<Pod>,
    pod_name: String,
    namespace: String,
    remote_port: u16,
    listener: TcpListener,
    shutdown: Arc<Notify>,
) {
    let mut connections: Vec<JoinHandle<()>> = Vec::new();

    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                tracing::debug!(
                    "port-forward {namespace}/{pod_name}:{remote_port}: arrêt demandé"
                );
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((socket, peer)) => {
                        let api = api.clone();
                        let pod_name = pod_name.clone();
                        let namespace = namespace.clone();
                        connections.push(tokio::spawn(async move {
                            if let Err(err) =
                                pipe_connection(api, &pod_name, remote_port, socket).await
                            {
                                tracing::warn!(
                                    "port-forward {namespace}/{pod_name}:{remote_port} \
                                     (client {peer}): {err}"
                                );
                            }
                        }));
                    }
                    Err(err) => {
                        tracing::warn!(
                            "port-forward {namespace}/{pod_name}: échec d'acceptation: {err}"
                        );
                        break;
                    }
                }
            }
        }

        connections.retain(|task| !task.is_finished());
    }

    for task in connections {
        task.abort();
    }
}

/// Relie une connexion locale à une nouvelle redirection vers le pod.
async fn pipe_connection(
    api: Api<Pod>,
    pod_name: &str,
    remote_port: u16,
    mut socket: TcpStream,
) -> Result<()> {
    let mut forwarder = api.portforward(pod_name, &[remote_port]).await?;
    let mut upstream = forwarder.take_stream(remote_port).ok_or_else(|| {
        Error::Other(format!(
            "le pod {pod_name} n'a pas ouvert de flux pour le port {remote_port}"
        ))
    })?;
    let error_rx = forwarder.take_error(remote_port);

    let outcome = tokio::io::copy_bidirectional(&mut socket, &mut upstream).await;

    // Libérer le flux avant d'attendre: la tâche de redirection ne se termine
    // qu'une fois toutes ses extrémités relâchées.
    drop(upstream);

    if let Some(rx) = error_rx {
        if let Ok(Some(message)) = tokio::time::timeout(ERROR_DRAIN_TIMEOUT, rx).await {
            tracing::warn!("port-forward {pod_name}:{remote_port}: {message}");
        }
    }
    forwarder.abort();

    match outcome {
        Ok(_) => Ok(()),
        Err(err) if is_clean_close(&err) => Ok(()),
        Err(err) => Err(Error::from(err)),
    }
}

/// Une coupure côté client ou côté pod n'est pas une erreur exploitable.
fn is_clean_close(err: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(
        err.kind(),
        BrokenPipe | ConnectionAborted | ConnectionReset | NotConnected | UnexpectedEof
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fermetures_propres_detectees() {
        use std::io::{Error as IoError, ErrorKind};
        assert!(is_clean_close(&IoError::new(ErrorKind::BrokenPipe, "x")));
        assert!(is_clean_close(&IoError::new(
            ErrorKind::ConnectionReset,
            "x"
        )));
        assert!(!is_clean_close(&IoError::new(
            ErrorKind::PermissionDenied,
            "x"
        )));
    }

    #[tokio::test]
    async fn port_zero_attribue_par_l_os() {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .expect("écoute locale");
        let port = listener.local_addr().expect("adresse locale").port();
        assert_ne!(port, 0, "l'OS doit attribuer un port concret");
    }

    #[test]
    fn arret_idempotent() {
        let inner = ForwardInner {
            shutdown: Arc::new(Notify::new()),
            stopped: AtomicBool::new(false),
            task: Mutex::new(None),
        };
        assert!(!inner.stopped.load(Ordering::SeqCst));
        inner.stop();
        assert!(inner.stopped.load(Ordering::SeqCst));
        inner.stop();
        assert!(inner.stopped.load(Ordering::SeqCst));
    }
}
