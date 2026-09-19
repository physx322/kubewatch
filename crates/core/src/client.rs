//! Établissement de la connexion à un cluster Kubernetes.
//!
//! Quatre modes sont supportés :
//! * [`ConnectionSpec::Kubeconfig`] — un fichier kubeconfig sur disque (chemin explicite,
//!   `$KUBECONFIG` ou `~/.kube/config`) et, éventuellement, un contexte précis ;
//! * [`ConnectionSpec::Inline`] — le contenu YAML d'un kubeconfig collé par l'utilisateur ;
//! * [`ConnectionSpec::Remote`] — une URL de serveur d'API avec jeton et/ou certificats client ;
//! * [`ConnectionSpec::InCluster`] — le `ServiceAccount` monté dans le pod courant.

use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{ContextInfo, VersionInfo};

/// Namespace employé quand ni le contexte ni l'appelant n'en imposent un.
pub const FALLBACK_NAMESPACE: &str = "default";

/// Manière de joindre un cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ConnectionSpec {
    /// Kubeconfig lu sur le disque.
    #[serde(rename_all = "camelCase")]
    Kubeconfig {
        /// Chemin du fichier ; `None` signifie `$KUBECONFIG` puis `~/.kube/config`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
        /// Contexte à charger ; `None` signifie le `current-context` du fichier.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<String>,
    },
    /// Kubeconfig fourni directement sous forme de texte YAML.
    #[serde(rename_all = "camelCase")]
    Inline {
        /// Contenu YAML complet du kubeconfig.
        yaml: String,
        /// Contexte à charger ; `None` signifie le `current-context` du document.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<String>,
    },
    /// Serveur d'API joint directement, sans kubeconfig.
    #[serde(rename_all = "camelCase")]
    Remote {
        /// URL du serveur d'API (`https://hôte:6443`).
        server: String,
        /// Jeton porteur (`ServiceAccount`, OIDC déjà échangé...).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        /// Autorité de certification au format PEM.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ca_cert_pem: Option<String>,
        /// Certificat client au format PEM.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_cert_pem: Option<String>,
        /// Clé privée client au format PEM.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_key_pem: Option<String>,
        /// Désactive la vérification du certificat serveur (déconseillé).
        #[serde(default)]
        insecure_skip_tls_verify: bool,
        /// Namespace par défaut.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        namespace: Option<String>,
        /// Proxy HTTP/SOCKS5 à traverser.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proxy_url: Option<String>,
    },
    /// Configuration injectée par Kubernetes dans le pod courant.
    InCluster,
}

impl ConnectionSpec {
    /// Spécification par défaut : kubeconfig usuel, contexte courant.
    pub fn default_kubeconfig() -> Self {
        ConnectionSpec::Kubeconfig {
            path: None,
            context: None,
        }
    }

    /// Contexte visé par cette spécification, quand la notion a un sens.
    pub fn context(&self) -> Option<&str> {
        match self {
            ConnectionSpec::Kubeconfig { context, .. } | ConnectionSpec::Inline { context, .. } => {
                context.as_deref()
            }
            _ => None,
        }
    }

    /// Libellé court pour les journaux et l'interface.
    pub fn label(&self) -> String {
        match self {
            ConnectionSpec::Kubeconfig { path, context } => {
                let file = path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "kubeconfig par défaut".to_string());
                match context {
                    Some(c) => format!("{file} (contexte {c})"),
                    None => file,
                }
            }
            ConnectionSpec::Inline { context, .. } => match context {
                Some(c) => format!("kubeconfig en ligne (contexte {c})"),
                None => "kubeconfig en ligne".to_string(),
            },
            ConnectionSpec::Remote { server, .. } => server.clone(),
            ConnectionSpec::InCluster => "in-cluster".to_string(),
        }
    }
}

/// Chemin du kubeconfig par défaut : première entrée de `$KUBECONFIG`, sinon `~/.kube/config`.
pub fn default_kubeconfig_path() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("KUBECONFIG") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        if let Some(first) = raw.split(sep).map(str::trim).find(|p| !p.is_empty()) {
            return Some(PathBuf::from(first));
        }
    }
    let home = dirs::home_dir()?;
    let path = home.join(".kube").join("config");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Lit un kubeconfig depuis un chemin explicite, ou depuis `$KUBECONFIG` / `~/.kube/config`.
pub fn read_kubeconfig(path: Option<&Path>) -> Result<Kubeconfig> {
    match path {
        Some(p) => Kubeconfig::read_from(p)
            .map_err(|e| Error::KubeConfig(format!("lecture de {} impossible: {e}", p.display()))),
        None => Kubeconfig::read()
            .map_err(|e| Error::KubeConfig(format!("kubeconfig par défaut illisible: {e}"))),
    }
}

/// Énumère les contextes d'un kubeconfig, en marquant le contexte courant.
pub fn list_contexts(path: Option<&Path>) -> Result<Vec<ContextInfo>> {
    let kc = read_kubeconfig(path)?;
    Ok(contexts_of(&kc))
}

/// Énumère les contextes d'un kubeconfig déjà chargé.
pub fn contexts_of(kc: &Kubeconfig) -> Vec<ContextInfo> {
    let current = kc.current_context.as_deref();
    kc.contexts
        .iter()
        .map(|named| {
            let ctx = named.context.as_ref();
            let cluster = ctx.map(|c| c.cluster.clone()).unwrap_or_default();
            let server = kc
                .clusters
                .iter()
                .find(|c| c.name == cluster)
                .and_then(|c| c.cluster.as_ref())
                .and_then(|c| c.server.clone());
            ContextInfo {
                name: named.name.clone(),
                cluster,
                user: ctx.and_then(|c| c.user.clone()),
                namespace: ctx.and_then(|c| c.namespace.clone()),
                server,
                current: current == Some(named.name.as_str()),
            }
        })
        .collect()
}

/// URL du serveur d'API visé par une spécification, au mieux de ce qui est connaissable
/// sans se connecter.
///
/// `kube::Client` ne réexpose pas l'URL de sa configuration : on la retrouve donc depuis la
/// spécification d'origine (kubeconfig, URL explicite ou variables d'environnement du pod).
pub fn server_url(spec: &ConnectionSpec) -> Option<String> {
    match spec {
        ConnectionSpec::Remote { server, .. } => Some(normalize_server_url(server)),
        ConnectionSpec::InCluster => {
            let host = std::env::var("KUBERNETES_SERVICE_HOST").ok()?;
            let port =
                std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".to_string());
            // Une adresse IPv6 doit être encadrée de crochets dans une URL.
            let host = if host.contains(':') && !host.starts_with('[') {
                format!("[{host}]")
            } else {
                host
            };
            Some(format!("https://{host}:{port}"))
        }
        ConnectionSpec::Kubeconfig { path, context } => {
            let kc = read_kubeconfig(path.as_deref()).ok()?;
            server_of_context(&kc, context.as_deref())
        }
        ConnectionSpec::Inline { yaml, context } => {
            let kc = Kubeconfig::from_yaml(yaml).ok()?;
            server_of_context(&kc, context.as_deref())
        }
    }
}

/// URL du cluster associé à un contexte d'un kubeconfig déjà chargé.
pub fn server_of_context(kc: &Kubeconfig, context: Option<&str>) -> Option<String> {
    let name = context
        .map(str::to_string)
        .or_else(|| kc.current_context.clone())
        .or_else(|| kc.contexts.first().map(|c| c.name.clone()))?;
    let ctx = kc
        .contexts
        .iter()
        .find(|c| c.name == name)?
        .context
        .as_ref()?;
    kc.clusters
        .iter()
        .find(|c| c.name == ctx.cluster)?
        .cluster
        .as_ref()?
        .server
        .clone()
}

/// Ouvre une connexion et renvoie le client ainsi que le namespace par défaut effectif.
pub async fn connect(spec: &ConnectionSpec) -> Result<(Client, String)> {
    let config = build_config(spec).await?;
    let namespace = if config.default_namespace.is_empty() {
        FALLBACK_NAMESPACE.to_string()
    } else {
        config.default_namespace.clone()
    };
    let client = Client::try_from(config).map_err(Error::from_kube)?;
    Ok((client, namespace))
}

/// Construit la configuration `kube` correspondant à une spécification, sans se connecter.
pub async fn build_config(spec: &ConnectionSpec) -> Result<Config> {
    match spec {
        ConnectionSpec::Kubeconfig { path, context } => {
            let kc = read_kubeconfig(path.as_deref())?;
            config_from_kubeconfig(kc, context.as_deref()).await
        }
        ConnectionSpec::Inline { yaml, context } => {
            let kc = Kubeconfig::from_yaml(yaml)
                .map_err(|e| Error::KubeConfig(format!("kubeconfig en ligne invalide: {e}")))?;
            config_from_kubeconfig(kc, context.as_deref()).await
        }
        ConnectionSpec::Remote {
            server,
            token,
            ca_cert_pem,
            client_cert_pem,
            client_key_pem,
            insecure_skip_tls_verify,
            namespace,
            proxy_url,
        } => remote_config(
            server,
            token.as_deref(),
            ca_cert_pem.as_deref(),
            client_cert_pem.as_deref(),
            client_key_pem.as_deref(),
            *insecure_skip_tls_verify,
            namespace.as_deref(),
            proxy_url.as_deref(),
        ),
        ConnectionSpec::InCluster => Config::incluster()
            .map_err(|e| Error::KubeConfig(format!("configuration in-cluster absente: {e}"))),
    }
}

/// Construit une configuration à partir d'un kubeconfig chargé et d'un contexte optionnel.
async fn config_from_kubeconfig(kc: Kubeconfig, context: Option<&str>) -> Result<Config> {
    if let Some(name) = context {
        if !kc.contexts.iter().any(|c| c.name == name) {
            let dispo: Vec<&str> = kc.contexts.iter().map(|c| c.name.as_str()).collect();
            return Err(Error::NotFound(format!(
                "contexte « {name} » introuvable (contextes disponibles: {})",
                if dispo.is_empty() {
                    "aucun".to_string()
                } else {
                    dispo.join(", ")
                }
            )));
        }
    }
    if context.is_none() && kc.current_context.is_none() && kc.contexts.len() != 1 {
        return Err(Error::KubeConfig(
            "aucun contexte courant dans le kubeconfig: précisez-en un".to_string(),
        ));
    }
    // Quand le fichier n'a pas de current-context mais un unique contexte, on le choisit.
    let context = context
        .map(str::to_string)
        .or_else(|| kc.current_context.clone())
        .or_else(|| kc.contexts.first().map(|c| c.name.clone()));

    let opts = KubeConfigOptions {
        context,
        cluster: None,
        user: None,
    };
    Config::from_custom_kubeconfig(kc, &opts)
        .await
        .map_err(|e| Error::KubeConfig(format!("kubeconfig inutilisable: {e}")))
}

/// Construit une configuration pour un serveur d'API joint directement.
#[allow(clippy::too_many_arguments)]
fn remote_config(
    server: &str,
    token: Option<&str>,
    ca_cert_pem: Option<&str>,
    client_cert_pem: Option<&str>,
    client_key_pem: Option<&str>,
    insecure_skip_tls_verify: bool,
    namespace: Option<&str>,
    proxy_url: Option<&str>,
) -> Result<Config> {
    let url = normalize_server_url(server);
    let uri: http::Uri = url
        .parse()
        .map_err(|e| Error::Invalid(format!("URL de serveur invalide « {server} »: {e}")))?;

    let mut config = Config::new(uri);
    config.accept_invalid_certs = insecure_skip_tls_verify;
    config.default_namespace = namespace
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(FALLBACK_NAMESPACE)
        .to_string();

    if let Some(proxy) = proxy_url.map(str::trim).filter(|p| !p.is_empty()) {
        config.proxy_url = Some(
            proxy
                .parse::<http::Uri>()
                .map_err(|e| Error::Invalid(format!("URL de proxy invalide « {proxy} »: {e}")))?,
        );
    }

    if let Some(ca) = ca_cert_pem.map(str::trim).filter(|c| !c.is_empty()) {
        let der = pem_to_der(ca, "CERTIFICATE")?;
        if der.is_empty() {
            return Err(Error::Invalid(
                "l'autorité de certification fournie ne contient aucun bloc CERTIFICATE"
                    .to_string(),
            ));
        }
        config.root_cert = Some(der);
    }

    // `AuthInfo` porte des secrets (`secrecy::SecretString`) que l'on ne peut pas construire
    // sans dépendre de `secrecy`; on passe donc par sa représentation serde, qui est la même
    // que celle d'un kubeconfig (`token`, `client-certificate-data`, `client-key-data`).
    let mut auth = serde_json::Map::new();
    if let Some(t) = token.map(str::trim).filter(|t| !t.is_empty()) {
        auth.insert(
            "token".to_string(),
            serde_json::Value::String(t.to_string()),
        );
    }
    match (client_cert_pem, client_key_pem) {
        (Some(cert), Some(key)) => {
            auth.insert(
                "client-certificate-data".to_string(),
                serde_json::Value::String(BASE64.encode(normalize_pem(cert)?.as_bytes())),
            );
            auth.insert(
                "client-key-data".to_string(),
                serde_json::Value::String(BASE64.encode(normalize_pem(key)?.as_bytes())),
            );
        }
        (Some(_), None) => {
            return Err(Error::Invalid(
                "certificat client fourni sans clé privée".to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(Error::Invalid(
                "clé privée cliente fournie sans certificat".to_string(),
            ));
        }
        (None, None) => {}
    }
    if auth.is_empty() {
        tracing::warn!(
            server = %url,
            "connexion distante sans jeton ni certificat client: le serveur refusera probablement les requêtes"
        );
    }
    config.auth_info = serde_json::from_value(serde_json::Value::Object(auth))
        .map_err(|e| Error::Invalid(format!("identifiants inutilisables: {e}")))?;

    Ok(config)
}

/// Complète une URL de serveur dépourvue de schéma et supprime la barre oblique finale.
pub fn normalize_server_url(server: &str) -> String {
    let trimmed = server.trim();
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let cleaned = with_scheme.trim_end_matches('/');
    if cleaned.is_empty() {
        with_scheme
    } else {
        cleaned.to_string()
    }
}

/// Renvoie un PEM propre, en acceptant aussi une version encodée en base64 (style kubeconfig).
pub fn normalize_pem(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::Invalid("certificat PEM vide".to_string()));
    }
    if trimmed.contains("-----BEGIN") {
        return Ok(trimmed.to_string());
    }
    // Certains utilisateurs collent le contenu de `certificate-authority-data`, déjà en base64.
    let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    let decoded = BASE64
        .decode(compact.as_bytes())
        .map_err(|e| Error::Invalid(format!("bloc PEM ou base64 invalide: {e}")))?;
    let text = String::from_utf8(decoded)
        .map_err(|_| Error::Invalid("bloc PEM invalide: contenu non textuel".to_string()))?;
    if text.contains("-----BEGIN") {
        Ok(text.trim().to_string())
    } else {
        Err(Error::Invalid(
            "aucun bloc PEM « -----BEGIN ... ----- » trouvé".to_string(),
        ))
    }
}

/// Décode tous les blocs PEM d'un type donné en DER.
///
/// `label` vaut par exemple `CERTIFICATE`; passer une chaîne vide accepte n'importe quel bloc.
pub fn pem_to_der(input: &str, label: &str) -> Result<Vec<Vec<u8>>> {
    let pem = normalize_pem(input)?;
    let mut blocks = Vec::new();
    let mut rest = pem.as_str();

    while let Some(begin) = rest.find("-----BEGIN ") {
        let after_begin = &rest[begin + "-----BEGIN ".len()..];
        let Some(header_end) = after_begin.find("-----") else {
            return Err(Error::Invalid(
                "bloc PEM tronqué: en-tête non terminé".to_string(),
            ));
        };
        let kind = after_begin[..header_end].trim().to_string();
        let body_start = begin + "-----BEGIN ".len() + header_end + "-----".len();
        let footer = format!("-----END {kind}-----");
        let Some(body_len) = rest[body_start..].find(&footer) else {
            return Err(Error::Invalid(format!(
                "bloc PEM « {kind} » sans marqueur de fin"
            )));
        };
        let body = &rest[body_start..body_start + body_len];
        if label.is_empty() || kind.eq_ignore_ascii_case(label) {
            let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
            let der = BASE64.decode(compact.as_bytes()).map_err(|e| {
                Error::Invalid(format!(
                    "corps base64 invalide dans le bloc « {kind} »: {e}"
                ))
            })?;
            blocks.push(der);
        }
        rest = &rest[body_start + body_len + footer.len()..];
    }

    if blocks.is_empty() {
        return Err(Error::Invalid(format!(
            "aucun bloc PEM « {} » exploitable",
            if label.is_empty() {
                "quelconque"
            } else {
                label
            }
        )));
    }
    Ok(blocks)
}

/// Interroge `GET /version` sur le serveur d'API.
pub async fn server_version(client: &Client) -> Result<VersionInfo> {
    let req = http::Request::get("/version").body(Vec::new())?;
    let raw: serde_json::Value = client.request(req).await.map_err(Error::from_kube)?;
    let field = |k: &str| {
        raw.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Ok(VersionInfo {
        major: field("major"),
        minor: field("minor"),
        git_version: field("gitVersion"),
        platform: field("platform"),
    })
}

/// Sonde la santé du serveur d'API : `GET /livez`, puis `GET /healthz` en repli.
pub async fn healthz(client: &Client) -> Result<bool> {
    for path in ["/livez", "/healthz"] {
        let req = http::Request::get(path).body(Vec::new())?;
        match client.request_text(req).await {
            Ok(body) => return Ok(body.trim().eq_ignore_ascii_case("ok")),
            Err(e) => {
                tracing::debug!(path, error = %e, "sonde de santé en échec, tentative suivante");
            }
        }
    }
    // Dernier recours : si `/version` répond, le serveur est joignable.
    match server_version(client).await {
        Ok(_) => Ok(true),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Corps base64 de « hello ».
    const PEM_UN_BLOC: &str = "-----BEGIN CERTIFICATE-----\naGVsbG8=\n-----END CERTIFICATE-----\n";
    const PEM_DEUX_BLOCS: &str = concat!(
        "-----BEGIN CERTIFICATE-----\naGVsbG8=\n-----END CERTIFICATE-----\n",
        "-----BEGIN CERTIFICATE-----\nd29ybGQ=\n-----END CERTIFICATE-----\n"
    );

    #[test]
    fn pem_un_bloc() {
        let der = pem_to_der(PEM_UN_BLOC, "CERTIFICATE").expect("décodage");
        assert_eq!(der.len(), 1);
        assert_eq!(der[0], b"hello");
    }

    #[test]
    fn pem_plusieurs_blocs() {
        let der = pem_to_der(PEM_DEUX_BLOCS, "CERTIFICATE").expect("décodage");
        assert_eq!(der.len(), 2);
        assert_eq!(der[0], b"hello");
        assert_eq!(der[1], b"world");
    }

    #[test]
    fn pem_encode_en_base64() {
        let encode = BASE64.encode(PEM_UN_BLOC.as_bytes());
        let der = pem_to_der(&encode, "CERTIFICATE").expect("décodage");
        assert_eq!(der[0], b"hello");
        assert!(normalize_pem(&encode).expect("pem").contains("-----BEGIN"));
    }

    #[test]
    fn pem_filtre_par_etiquette() {
        let mixte = concat!(
            "-----BEGIN RSA PRIVATE KEY-----\naGVsbG8=\n-----END RSA PRIVATE KEY-----\n",
            "-----BEGIN CERTIFICATE-----\nd29ybGQ=\n-----END CERTIFICATE-----\n"
        );
        let certs = pem_to_der(mixte, "CERTIFICATE").expect("décodage");
        assert_eq!(certs.len(), 1);
        assert_eq!(certs[0], b"world");
        let tous = pem_to_der(mixte, "").expect("décodage");
        assert_eq!(tous.len(), 2);
    }

    #[test]
    fn pem_invalides() {
        assert!(pem_to_der("", "CERTIFICATE").is_err());
        assert!(pem_to_der("pas du pem", "CERTIFICATE").is_err());
        assert!(pem_to_der("-----BEGIN CERTIFICATE-----\naGVsbG8=\n", "CERTIFICATE").is_err());
        assert!(pem_to_der(PEM_UN_BLOC, "PRIVATE KEY").is_err());
    }

    #[test]
    fn normalisation_url_serveur() {
        assert_eq!(normalize_server_url("1.2.3.4:6443"), "https://1.2.3.4:6443");
        assert_eq!(
            normalize_server_url(" https://api.example.com:6443/ "),
            "https://api.example.com:6443"
        );
        assert_eq!(
            normalize_server_url("http://localhost:8080"),
            "http://localhost:8080"
        );
    }

    #[test]
    fn configuration_distante_avec_jeton() {
        let cfg = remote_config(
            "api.example.com:6443",
            Some("jeton-secret"),
            Some(PEM_UN_BLOC),
            None,
            None,
            false,
            Some("prod"),
            None,
        )
        .expect("configuration");
        assert_eq!(cfg.default_namespace, "prod");
        assert!(!cfg.accept_invalid_certs);
        assert_eq!(cfg.root_cert.as_ref().map(|c| c.len()), Some(1));
        assert_eq!(cfg.cluster_url.to_string(), "https://api.example.com:6443/");
        // Le jeton est bien posé (on ne le relit pas, il est protégé par `secrecy`).
        assert!(cfg.auth_info.token.is_some());
    }

    #[test]
    fn configuration_distante_certificat_sans_cle() {
        let err = remote_config(
            "https://api.example.com:6443",
            None,
            None,
            Some(PEM_UN_BLOC),
            None,
            false,
            None,
            None,
        )
        .expect_err("doit refuser");
        assert_eq!(err.status_code(), 400);
    }

    #[test]
    fn configuration_distante_avec_certificats_client() {
        let cfg = remote_config(
            "https://api.example.com:6443",
            None,
            None,
            Some(PEM_UN_BLOC),
            Some(PEM_DEUX_BLOCS),
            true,
            None,
            Some("socks5://127.0.0.1:1080"),
        )
        .expect("configuration");
        assert!(cfg.accept_invalid_certs);
        assert_eq!(cfg.default_namespace, FALLBACK_NAMESPACE);
        assert!(cfg.proxy_url.is_some());
        assert!(cfg.auth_info.client_certificate_data.is_some());
        assert!(cfg.auth_info.client_key_data.is_some());
        // Les données doivent être le PEM encodé en base64, comme dans un kubeconfig.
        let encode = cfg.auth_info.client_certificate_data.clone().expect("cert");
        let decode = BASE64.decode(encode.as_bytes()).expect("base64");
        assert!(String::from_utf8(decode)
            .expect("utf8")
            .contains("-----BEGIN CERTIFICATE-----"));
    }

    #[test]
    fn specification_serialise_en_camel_case() {
        let spec = ConnectionSpec::Remote {
            server: "https://api:6443".into(),
            token: Some("t".into()),
            ca_cert_pem: None,
            client_cert_pem: None,
            client_key_pem: None,
            insecure_skip_tls_verify: true,
            namespace: Some("prod".into()),
            proxy_url: None,
        };
        let v = serde_json::to_value(&spec).expect("sérialisation");
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("remote"));
        assert_eq!(
            v.get("insecureSkipTlsVerify").and_then(|t| t.as_bool()),
            Some(true)
        );
        assert!(v.get("caCertPem").is_none());
        let back: ConnectionSpec = serde_json::from_value(v).expect("désérialisation");
        assert_eq!(back, spec);

        let in_cluster: ConnectionSpec =
            serde_json::from_str(r#"{"type":"inCluster"}"#).expect("désérialisation");
        assert_eq!(in_cluster, ConnectionSpec::InCluster);

        let kubeconfig: ConnectionSpec =
            serde_json::from_str(r#"{"type":"kubeconfig","context":"prod"}"#)
                .expect("désérialisation");
        assert_eq!(kubeconfig.context(), Some("prod"));
    }

    #[test]
    fn contextes_dun_kubeconfig() {
        let yaml = r#"
apiVersion: v1
kind: Config
current-context: prod
clusters:
- name: prod-cluster
  cluster:
    server: https://prod:6443
- name: dev-cluster
  cluster:
    server: https://dev:6443
contexts:
- name: prod
  context:
    cluster: prod-cluster
    user: admin
    namespace: production
- name: dev
  context:
    cluster: dev-cluster
    user: dev
users:
- name: admin
  user: {}
- name: dev
  user: {}
"#;
        let kc = Kubeconfig::from_yaml(yaml).expect("kubeconfig");
        let ctx = contexts_of(&kc);
        assert_eq!(ctx.len(), 2);
        let prod = ctx
            .iter()
            .find(|c| c.name == "prod")
            .expect("contexte prod");
        assert!(prod.current);
        assert_eq!(prod.cluster, "prod-cluster");
        assert_eq!(prod.namespace.as_deref(), Some("production"));
        assert_eq!(prod.server.as_deref(), Some("https://prod:6443"));
        let dev = ctx.iter().find(|c| c.name == "dev").expect("contexte dev");
        assert!(!dev.current);
        assert!(dev.namespace.is_none());
    }

    #[tokio::test]
    async fn contexte_inconnu_refuse() {
        let yaml = r#"
apiVersion: v1
kind: Config
current-context: prod
clusters: []
contexts:
- name: prod
  context:
    cluster: c
users: []
"#;
        let kc = Kubeconfig::from_yaml(yaml).expect("kubeconfig");
        let err = config_from_kubeconfig(kc, Some("absent"))
            .await
            .expect_err("doit échouer");
        assert_eq!(err.status_code(), 404);
    }
}
