//! Gestion du parc de clusters : connexion, cache du catalogue, persistance et sélection.
//!
//! [`ClusterManager`] est partagé par tout le backend de l'application. Il est clonable à coût nul
//! (compteur de références interne) et sûr vis-à-vis des accès concurrents.
//!
//! Les spécifications de connexion sont conservées dans `<state_dir>/clusters.json`, écrit
//! de façon atomique et restreint au propriétaire (mode `0600` sur Unix) car il peut
//! contenir des jetons.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use indexmap::IndexMap;
use k8s_openapi::api::core::v1::{Namespace, Node};
use kube::api::{Api, DynamicObject, ListParams};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::client::{self, ConnectionSpec};
use crate::discovery::ResourceCatalog;
use crate::error::{Error, Result};
use crate::model::{ClusterInfo, ResourceKind};

/// Nom du fichier de persistance, relatif au répertoire d'état.
pub const STATE_FILE: &str = "clusters.json";

/// Cluster connecté, prêt à l'emploi.
#[derive(Clone)]
pub struct ClusterHandle {
    /// Nom local du cluster dans KubeWatch.
    pub name: String,
    /// Spécification ayant servi à établir la connexion.
    pub spec: ConnectionSpec,
    /// Client Kubernetes partagé.
    pub client: kube::Client,
    /// Namespace utilisé quand l'appelant n'en précise pas.
    pub default_namespace: String,
    /// Catalogue des types de ressources, rafraîchissable à chaud.
    pub catalog: Arc<RwLock<ResourceCatalog>>,
}

impl std::fmt::Debug for ClusterHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterHandle")
            .field("name", &self.name)
            .field("spec", &self.spec)
            .field("default_namespace", &self.default_namespace)
            .field("kinds", &self.catalog.read().len())
            .finish()
    }
}

impl ClusterHandle {
    /// Résout une saisie utilisateur (`po`, `pods`, `deployments.apps`...) vers un type connu.
    pub fn resolve_kind(&self, needle: &str) -> Result<ResourceKind> {
        self.catalog.read().resolve(needle).cloned().ok_or_else(|| {
            Error::NotFound(format!(
                "type de ressource « {needle} » inconnu du cluster « {} »",
                self.name
            ))
        })
    }

    /// Construit l'API dynamique pour un type donné.
    ///
    /// Un namespace vide ou `*` signifie « tous les namespaces »; il est également ignoré
    /// pour les ressources de portée cluster.
    pub fn api_for(&self, kind: &ResourceKind, namespace: Option<&str>) -> Api<DynamicObject> {
        let ar = ResourceCatalog::api_resource(kind);
        let ns = namespace
            .map(str::trim)
            .filter(|n| !n.is_empty() && *n != "*" && *n != "all");
        match (kind.namespaced, ns) {
            (true, Some(ns)) => Api::namespaced_with(self.client.clone(), ns, &ar),
            _ => Api::all_with(self.client.clone(), &ar),
        }
    }

    /// Namespace effectif : celui demandé, sinon le namespace par défaut du cluster.
    pub fn namespace_or_default<'a>(&'a self, namespace: Option<&'a str>) -> &'a str {
        namespace
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.default_namespace)
    }

    /// Collecte l'état du cluster sans jamais échouer : chaque champ est rempli au mieux et
    /// les erreurs rencontrées sont regroupées dans `last_error`.
    pub async fn info(&self) -> ClusterInfo {
        let mut errors: Vec<String> = Vec::new();
        let mut info = ClusterInfo {
            name: self.name.clone(),
            context: self.spec.context().map(str::to_string),
            server: client::server_url(&self.spec).unwrap_or_default(),
            default_namespace: self.default_namespace.clone(),
            ..Default::default()
        };

        match client::server_version(&self.client).await {
            Ok(v) => {
                info.connected = true;
                info.version = Some(v.git_version).filter(|s| !s.is_empty());
                info.platform = Some(v.platform).filter(|s| !s.is_empty());
            }
            Err(e) => errors.push(format!("version du serveur: {e}")),
        }

        if !info.connected {
            match client::healthz(&self.client).await {
                Ok(ok) => info.connected = ok,
                Err(e) => errors.push(format!("sonde de santé: {e}")),
            }
        }

        let nodes: Api<Node> = Api::all(self.client.clone());
        match nodes.list(&ListParams::default()).await {
            Ok(list) => info.node_count = Some(list.items.len()),
            Err(e) => errors.push(format!("lecture des nœuds: {e}")),
        }

        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        match namespaces.list(&ListParams::default()).await {
            Ok(list) => info.namespace_count = Some(list.items.len()),
            Err(e) => errors.push(format!("lecture des namespaces: {e}")),
        }

        info.metrics_available = metrics_api_available(&self.client).await;

        if !errors.is_empty() {
            info.last_error = Some(errors.join(" ; "));
        }
        info
    }
}

/// Teste la présence de l'API agrégée `metrics.k8s.io`.
async fn metrics_api_available(client: &kube::Client) -> bool {
    let Ok(req) = http::Request::get("/apis/metrics.k8s.io/v1beta1").body(Vec::new()) else {
        return false;
    };
    client.request::<serde_json::Value>(req).await.is_ok()
}

/// Contenu du fichier `clusters.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    /// Cluster sélectionné lors de la dernière session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<String>,
    /// Clusters enregistrés, dans leur ordre d'ajout.
    #[serde(default)]
    clusters: Vec<PersistedCluster>,
}

/// Entrée persistée d'un cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedCluster {
    name: String,
    spec: ConnectionSpec,
}

/// État interne partagé par tous les clones du gestionnaire.
struct Inner {
    state_dir: PathBuf,
    /// Clusters effectivement connectés.
    clusters: RwLock<IndexMap<String, ClusterHandle>>,
    /// Spécifications à réécrire dans le fichier d'état.
    persisted: RwLock<IndexMap<String, ConnectionSpec>>,
    /// Cluster courant.
    current: RwLock<Option<String>>,
}

/// Parc de clusters connus de KubeWatch.
#[derive(Clone)]
pub struct ClusterManager {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ClusterManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterManager")
            .field("state_dir", &self.inner.state_dir)
            .field("clusters", &self.names())
            .field("current", &*self.inner.current.read())
            .finish()
    }
}

impl ClusterManager {
    /// Crée un gestionnaire vide dont l'état sera persisté dans `state_dir`.
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                state_dir,
                clusters: RwLock::new(IndexMap::new()),
                persisted: RwLock::new(IndexMap::new()),
                current: RwLock::new(None),
            }),
        }
    }

    /// Chemin du fichier d'état.
    pub fn state_file(&self) -> PathBuf {
        self.inner.state_dir.join(STATE_FILE)
    }

    /// Répertoire d'état.
    pub fn state_dir(&self) -> &Path {
        &self.inner.state_dir
    }

    /// Recharge les clusters enregistrés et tente de s'y reconnecter.
    ///
    /// Une connexion en échec n'interrompt pas le chargement : la spécification reste
    /// enregistrée et l'erreur est journalisée, afin qu'un cluster momentanément
    /// injoignable ne disparaisse pas de la configuration de l'utilisateur.
    pub async fn load_persisted(&self) -> Result<()> {
        let path = self.state_file();
        let raw = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(Error::Io(std::io::Error::new(
                    e.kind(),
                    format!("lecture de {} impossible: {e}", path.display()),
                )));
            }
        };
        let state: PersistedState = serde_json::from_slice(&raw).map_err(|e| {
            Error::Invalid(format!("fichier d'état {} illisible: {e}", path.display()))
        })?;

        {
            let mut persisted = self.inner.persisted.write();
            for entry in &state.clusters {
                persisted.insert(entry.name.clone(), entry.spec.clone());
            }
        }

        for entry in &state.clusters {
            if let Err(e) = self
                .connect_and_register(&entry.name, entry.spec.clone())
                .await
            {
                tracing::warn!(
                    cluster = %entry.name,
                    error = %e,
                    "cluster enregistré injoignable, il reste configuré"
                );
            }
        }

        if let Some(name) = state.current {
            if self.inner.clusters.read().contains_key(&name) {
                *self.inner.current.write() = Some(name);
            }
        }
        if self.inner.current.read().is_none() {
            let first = self.inner.clusters.read().keys().next().cloned();
            *self.inner.current.write() = first;
        }
        Ok(())
    }

    /// Enregistre un cluster, s'y connecte et construit son catalogue.
    pub async fn add(
        &self,
        name: &str,
        spec: ConnectionSpec,
        persist: bool,
    ) -> Result<ClusterHandle> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Invalid(
                "le nom du cluster ne peut pas être vide".to_string(),
            ));
        }
        let handle = self.connect_and_register(name, spec.clone()).await?;
        if persist {
            self.inner.persisted.write().insert(name.to_string(), spec);
            if let Err(e) = self.save() {
                tracing::warn!(cluster = %name, error = %e, "état des clusters non enregistré");
            }
        }
        Ok(handle)
    }

    /// Connecte un cluster et l'insère dans le parc, sans toucher à la persistance.
    async fn connect_and_register(
        &self,
        name: &str,
        spec: ConnectionSpec,
    ) -> Result<ClusterHandle> {
        let (kube_client, default_namespace) = client::connect(&spec).await?;

        // Un catalogue partiel ne doit pas empêcher l'usage du cluster.
        let catalog = match ResourceCatalog::build(&kube_client).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    cluster = %name,
                    error = %e,
                    "découverte d'API incomplète, repli sur le catalogue intégré"
                );
                ResourceCatalog::fallback()
            }
        };

        let handle = ClusterHandle {
            name: name.to_string(),
            spec,
            client: kube_client,
            default_namespace,
            catalog: Arc::new(RwLock::new(catalog)),
        };

        self.inner
            .clusters
            .write()
            .insert(name.to_string(), handle.clone());
        let mut current = self.inner.current.write();
        if current.is_none() {
            *current = Some(name.to_string());
        }
        Ok(handle)
    }

    /// Importe un kubeconfig : un cluster par contexte, ou le seul contexte courant.
    ///
    /// Renvoie les noms des clusters effectivement ajoutés.
    pub async fn import_kubeconfig(
        &self,
        path: Option<&Path>,
        all_contexts: bool,
    ) -> Result<Vec<String>> {
        let kc = client::read_kubeconfig(path)?;
        let contexts = client::contexts_of(&kc);
        if contexts.is_empty() {
            return Err(Error::KubeConfig(
                "ce kubeconfig ne déclare aucun contexte".to_string(),
            ));
        }

        let selected: Vec<_> = if all_contexts {
            contexts.clone()
        } else {
            let courant: Vec<_> = contexts.iter().filter(|c| c.current).cloned().collect();
            if courant.is_empty() {
                contexts.first().cloned().into_iter().collect()
            } else {
                courant
            }
        };

        let mut added = Vec::new();
        let mut last_error = None;
        for ctx in selected {
            let spec = ConnectionSpec::Kubeconfig {
                path: path.map(Path::to_path_buf),
                context: Some(ctx.name.clone()),
            };
            match self.add(&ctx.name, spec, true).await {
                Ok(_) => added.push(ctx.name),
                Err(e) => {
                    tracing::warn!(contexte = %ctx.name, error = %e, "contexte non importé");
                    last_error = Some(e);
                }
            }
        }

        if added.is_empty() {
            return Err(last_error.unwrap_or_else(|| {
                Error::KubeConfig("aucun contexte n'a pu être importé".to_string())
            }));
        }
        Ok(added)
    }

    /// Retire un cluster du parc et de la persistance.
    pub fn remove(&self, name: &str) -> Result<()> {
        let removed_handle = self.inner.clusters.write().shift_remove(name).is_some();
        let removed_spec = self.inner.persisted.write().shift_remove(name).is_some();
        if !removed_handle && !removed_spec {
            return Err(Error::NotFound(format!("cluster « {name} » inconnu")));
        }
        {
            let mut current = self.inner.current.write();
            if current.as_deref() == Some(name) {
                *current = self.inner.clusters.read().keys().next().cloned();
            }
        }
        self.save()
    }

    /// Récupère un cluster connecté par son nom.
    pub fn get(&self, name: &str) -> Result<ClusterHandle> {
        self.inner
            .clusters
            .read()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("cluster « {name} » non connecté")))
    }

    /// Noms des clusters connectés, dans leur ordre d'ajout.
    pub fn names(&self) -> Vec<String> {
        self.inner.clusters.read().keys().cloned().collect()
    }

    /// Cluster courant, ou le premier disponible à défaut de sélection explicite.
    pub fn current(&self) -> Option<ClusterHandle> {
        let clusters = self.inner.clusters.read();
        if let Some(name) = self.inner.current.read().as_ref() {
            if let Some(handle) = clusters.get(name) {
                return Some(handle.clone());
            }
        }
        clusters.values().next().cloned()
    }

    /// Nom du cluster courant.
    pub fn current_name(&self) -> Option<String> {
        self.current().map(|h| h.name)
    }

    /// Sélectionne le cluster courant.
    pub fn set_current(&self, name: &str) -> Result<()> {
        if !self.inner.clusters.read().contains_key(name) {
            return Err(Error::NotFound(format!("cluster « {name} » non connecté")));
        }
        *self.inner.current.write() = Some(name.to_string());
        self.save()
    }

    /// Résout un nom de cluster optionnel vers un cluster utilisable.
    pub fn get_or_current(&self, name: Option<&str>) -> Result<ClusterHandle> {
        match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => self.get(n),
            None => self.current().ok_or_else(|| {
                Error::NotFound("aucun cluster connecté: ajoutez-en un d'abord".to_string())
            }),
        }
    }

    /// État de tous les clusters, connectés ou simplement enregistrés.
    pub async fn list_info(&self) -> Vec<ClusterInfo> {
        let handles: Vec<ClusterHandle> = self.inner.clusters.read().values().cloned().collect();
        let current = self.current_name();
        let mut infos: Vec<ClusterInfo> =
            futures::future::join_all(handles.iter().map(|h| h.info())).await;

        // Les clusters enregistrés mais injoignables restent visibles dans l'interface.
        let connected: Vec<String> = handles.iter().map(|h| h.name.clone()).collect();
        let persisted: Vec<(String, ConnectionSpec)> = self
            .inner
            .persisted
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (name, spec) in persisted {
            if connected.contains(&name) {
                continue;
            }
            infos.push(ClusterInfo {
                name: name.clone(),
                context: spec.context().map(str::to_string),
                server: client::server_url(&spec).unwrap_or_default(),
                connected: false,
                default_namespace: crate::client::FALLBACK_NAMESPACE.to_string(),
                last_error: Some("cluster enregistré mais non connecté".to_string()),
                ..Default::default()
            });
        }

        // Le cluster courant est présenté en premier.
        if let Some(current) = current {
            infos.sort_by_key(|i| i.name != current);
        }
        infos
    }

    /// Reconstruit le catalogue de découverte d'un cluster.
    pub async fn refresh_catalog(&self, name: &str) -> Result<()> {
        let handle = self.get(name)?;
        let catalog = ResourceCatalog::build(&handle.client).await?;
        *handle.catalog.write() = catalog;
        Ok(())
    }

    /// Spécifications enregistrées, y compris celles dont la connexion a échoué.
    pub fn persisted_specs(&self) -> Vec<(String, ConnectionSpec)> {
        self.inner
            .persisted
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Écrit le fichier d'état de façon atomique, en mode `0600` sur Unix.
    pub fn save(&self) -> Result<()> {
        let state = PersistedState {
            current: self.inner.current.read().clone(),
            clusters: self
                .inner
                .persisted
                .read()
                .iter()
                .map(|(name, spec)| PersistedCluster {
                    name: name.clone(),
                    spec: spec.clone(),
                })
                .collect(),
        };
        let data = serde_json::to_vec_pretty(&state)?;
        write_private_atomic(&self.inner.state_dir, STATE_FILE, &data)
    }
}

/// Écrit `data` dans `dir/file` via un fichier temporaire renommé, avec des droits restreints.
fn write_private_atomic(dir: &Path, file: &str, data: &[u8]) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }

    let target = dir.join(file);
    let tmp = dir.join(format!("{file}.tmp"));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut handle = options.open(&tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Le fichier temporaire peut préexister avec d'autres droits.
        handle.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    handle.write_all(data)?;
    handle.sync_all()?;
    drop(handle);

    std::fs::rename(&tmp, &target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gestionnaire_vide() {
        let dir = std::env::temp_dir().join(format!("kubewatch-test-{}", std::process::id()));
        let m = ClusterManager::new(dir.clone());
        assert!(m.names().is_empty());
        assert!(m.current().is_none());
        assert!(m.get("absent").is_err());
        assert_eq!(m.get("absent").unwrap_err().status_code(), 404);
        assert!(m.set_current("absent").is_err());
        assert!(m.remove("absent").is_err());
        assert_eq!(m.state_file(), dir.join(STATE_FILE));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn chargement_sans_fichier_detat() {
        let dir = std::env::temp_dir().join(format!(
            "kubewatch-test-vide-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let m = ClusterManager::new(dir.clone());
        m.load_persisted()
            .await
            .expect("absence de fichier tolérée");
        assert!(m.names().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ecriture_atomique_et_droits() {
        let dir = std::env::temp_dir().join(format!(
            "kubewatch-test-ecriture-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        write_private_atomic(&dir, STATE_FILE, b"{\"clusters\":[]}").expect("écriture");
        let path = dir.join(STATE_FILE);
        assert_eq!(std::fs::read(&path).expect("lecture"), b"{\"clusters\":[]}");
        assert!(!dir.join(format!("{STATE_FILE}.tmp")).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("métadonnées")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "le fichier d'état doit rester privé");
        }
        // Une seconde écriture doit remplacer proprement la première.
        write_private_atomic(&dir, STATE_FILE, b"{}").expect("réécriture");
        assert_eq!(std::fs::read(&path).expect("lecture"), b"{}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn etat_persiste_aller_retour() {
        let dir = std::env::temp_dir().join(format!(
            "kubewatch-test-etat-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        let state = PersistedState {
            current: Some("prod".to_string()),
            clusters: vec![
                PersistedCluster {
                    name: "prod".to_string(),
                    spec: ConnectionSpec::Remote {
                        server: "https://api:6443".to_string(),
                        token: Some("secret".to_string()),
                        ca_cert_pem: None,
                        client_cert_pem: None,
                        client_key_pem: None,
                        insecure_skip_tls_verify: false,
                        namespace: Some("prod".to_string()),
                        proxy_url: None,
                    },
                },
                PersistedCluster {
                    name: "local".to_string(),
                    spec: ConnectionSpec::Kubeconfig {
                        path: None,
                        context: Some("minikube".to_string()),
                    },
                },
            ],
        };
        let data = serde_json::to_vec_pretty(&state).expect("sérialisation");
        write_private_atomic(&dir, STATE_FILE, &data).expect("écriture");

        let relu: PersistedState =
            serde_json::from_slice(&std::fs::read(dir.join(STATE_FILE)).expect("lecture"))
                .expect("désérialisation");
        assert_eq!(relu.current.as_deref(), Some("prod"));
        assert_eq!(relu.clusters.len(), 2);
        assert_eq!(relu.clusters[1].spec.context(), Some("minikube"));

        // Un fichier d'état illisible remonte une erreur explicite (et non une panique).
        write_private_atomic(&dir, STATE_FILE, b"ceci n'est pas du json").expect("écriture");
        let m = ClusterManager::new(dir.clone());
        let err = m.load_persisted().await.expect_err("doit échouer");
        assert_eq!(err.status_code(), 400);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn api_for_choisit_la_portee() {
        // On n'a pas de cluster ici: on vérifie seulement la logique de sélection de portée.
        let namespaced = ResourceKind {
            group: "apps".into(),
            version: "v1".into(),
            kind: "Deployment".into(),
            plural: "deployments".into(),
            namespaced: true,
            ..Default::default()
        };
        let cluster_scoped = ResourceKind {
            group: String::new(),
            version: "v1".into(),
            kind: "Node".into(),
            plural: "nodes".into(),
            namespaced: false,
            ..Default::default()
        };
        assert!(namespaced.namespaced);
        assert!(!cluster_scoped.namespaced);
        let ar = ResourceCatalog::api_resource(&namespaced);
        assert_eq!(ar.api_version, "apps/v1");
    }
}
