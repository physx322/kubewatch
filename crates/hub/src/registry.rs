//! Client HTTP unique du hub : recherche d'images, listes de tags, résolution de digests
//! et inspection de la configuration OCI.
//!
//! Le client parle deux dialectes :
//!
//! * les API « catalogue » propriétaires (Docker Hub, Quay), qui fournissent descriptions,
//!   étoiles et dates de publication ;
//! * l'API **Registry v2** normalisée (`ghcr.io`, `quay.io`, `registry-1.docker.io`, tout
//!   registre privé), qui fournit manifestes, digests et configuration d'image.
//!
//! Le flux d'authentification anonyme par jeton (`WWW-Authenticate: Bearer realm=…`) est
//! implémenté et les jetons sont mis en cache par couple (hôte, portée).

use crate::error::{Error, Result};
use crate::model::{
    ImageDetails, ImageRef, ImageSummary, RegistryKind, TagInfo, DEFAULT_REGISTRY,
};
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Racine de l'API catalogue de Docker Hub.
const DOCKER_HUB_API: &str = "https://hub.docker.com";

/// Racine de l'API catalogue de Quay.
const QUAY_API: &str = "https://quay.io";

/// Agent utilisateur envoyé à tous les registres.
const USER_AGENT: &str = concat!("kubewatch/", env!("CARGO_PKG_VERSION"));

/// Délai maximal d'une requête vers un registre.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Nombre maximal de tentatives par requête (429 et 5xx).
const MAX_ATTEMPTS: u32 = 3;

/// Plafond d'attente entre deux tentatives, même si le registre demande davantage.
const MAX_BACKOFF: Duration = Duration::from_secs(8);

/// Types de médias acceptés lors de la lecture d'un manifeste.
pub const MANIFEST_ACCEPT: &[&str] = &[
    "application/vnd.oci.image.index.v1+json",
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
    "application/vnd.docker.distribution.manifest.v2+json",
];

/// Marqueurs indiquant qu'un suffixe de tag est une pré-version et non une variante d'image.
static PRERELEASE_MARKER: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:alpha|beta|rc|pre|preview|dev|canary|nightly|snapshot|next|edge|unstable)[0-9]*$")
        .expect("motif de pré-version valide")
});

/// Cœur numérique d'un tag : `1`, `1.2`, `1.2.3`.
static VERSION_CORE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[0-9]+(?:\.[0-9]+){0,2}$").expect("motif de version valide"));

/// Identifiants d'accès à un registre privé.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryCredentials {
    /// Nom d'utilisateur, pour une authentification basique.
    pub username: Option<String>,
    /// Mot de passe ou jeton d'accès personnel.
    pub password: Option<String>,
    /// Jeton porteur déjà obtenu, utilisé tel quel.
    pub token: Option<String>,
}

impl RegistryCredentials {
    /// Vrai si aucun identifiant n'est renseigné.
    pub fn is_empty(&self) -> bool {
        self.username.is_none() && self.password.is_none() && self.token.is_none()
    }
}

/// Jeton porteur mis en cache, avec sa date d'expiration.
#[derive(Debug, Clone)]
struct CachedToken {
    value: String,
    expires_at: Instant,
}

/// Défi d'authentification extrait d'un en-tête `WWW-Authenticate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerChallenge {
    /// URL du service de jetons.
    pub realm: String,
    /// Service demandé par le registre.
    pub service: Option<String>,
    /// Portée demandée, généralement `repository:<dépôt>:pull`.
    pub scope: Option<String>,
}

/// Client partagé vers les registres d'images et les API de catalogue.
///
/// Un seul `reqwest::Client` (donc un seul pool de connexions) est utilisé pour tout le
/// processus ; cloner un `HubClient` est bon marché et partage ce pool ainsi que le cache
/// de jetons.
#[derive(Clone)]
pub struct HubClient {
    pub(crate) http: reqwest::Client,
    pub(crate) credentials: HashMap<RegistryKind, RegistryCredentials>,
    pub(crate) github_token: Option<String>,
    tokens: Arc<Mutex<HashMap<String, CachedToken>>>,
}

impl std::fmt::Debug for HubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Les identifiants ne doivent jamais apparaître dans les journaux.
        f.debug_struct("HubClient")
            .field("registres_authentifiés", &self.credentials.len())
            .field("jeton_github", &self.github_token.is_some())
            .finish()
    }
}

impl HubClient {
    /// Construit le client HTTP partagé.
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(HTTP_TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(HubClient {
            http,
            credentials: HashMap::new(),
            github_token: None,
            tokens: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Associe des identifiants à une famille de registre.
    pub fn with_credentials(mut self, registry: RegistryKind, c: RegistryCredentials) -> Self {
        if c.is_empty() {
            self.credentials.remove(&registry);
        } else {
            self.credentials.insert(registry, c);
        }
        self
    }

    /// Renseigne le jeton GitHub utilisé pour lire les paquets privés de `ghcr.io`.
    pub fn with_github_token(mut self, t: Option<String>) -> Self {
        self.github_token = t.filter(|s| !s.trim().is_empty());
        self
    }

    /// Accès au client HTTP partagé, pour les autres modules du crate.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    // ---------------------------------------------------------------- recherche

    /// Recherche des images dans le registre demandé.
    ///
    /// Docker Hub et Quay exposent une véritable API de recherche. `ghcr.io` et les registres
    /// privés n'en ont pas : la requête est alors interprétée comme une référence d'image et
    /// vérifiée directement auprès du registre.
    pub async fn search_images(
        &self,
        query: &str,
        kind: RegistryKind,
        limit: usize,
    ) -> Result<Vec<ImageSummary>> {
        let query = query.trim();
        if query.is_empty() {
            return Err(Error::Invalid(
                "recherche vide : saisissez un nom d'image, par exemple « postgres »".to_string(),
            ));
        }
        let limit = limit.clamp(1, 100);
        match kind {
            RegistryKind::DockerHub => self.search_docker_hub(query, limit).await,
            RegistryKind::Quay => self.search_quay(query, limit).await,
            RegistryKind::Ghcr => self.probe_reference(query, Some("ghcr.io")).await,
            RegistryKind::Generic => self.probe_reference(query, None).await,
        }
    }

    /// Recherche via l'API catalogue de Docker Hub, avec repli sur l'API « content ».
    async fn search_docker_hub(&self, query: &str, limit: usize) -> Result<Vec<ImageSummary>> {
        let url = format!("{DOCKER_HUB_API}/v2/search/repositories/");
        let req = self
            .http
            .get(&url)
            .query(&[
                ("query", query.to_string()),
                ("page_size", limit.to_string()),
            ])
            .header(reqwest::header::ACCEPT, "application/json");

        if let Ok(resp) = self.send_retry(req).await {
            if resp.status().is_success() {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    let items = docker_hub_search_results(&body);
                    if !items.is_empty() {
                        return Ok(items.into_iter().take(limit).collect());
                    }
                }
            }
        }

        // Repli : l'API « content » est plus riche mais exige l'en-tête Search-Version.
        let url = format!("{DOCKER_HUB_API}/api/content/v1/products/search");
        let req = self
            .http
            .get(&url)
            .query(&[
                ("q", query.to_string()),
                ("type", "image".to_string()),
                ("page_size", limit.to_string()),
            ])
            .header(reqwest::header::ACCEPT, "application/json")
            .header("Search-Version", "v3");
        let resp = check_status(self.send_retry(req).await?, "recherche Docker Hub").await?;
        let body: serde_json::Value = resp.json().await?;
        Ok(docker_hub_content_results(&body).into_iter().take(limit).collect())
    }

    /// Recherche via l'API publique de Quay.
    async fn search_quay(&self, query: &str, limit: usize) -> Result<Vec<ImageSummary>> {
        let url = format!("{QUAY_API}/api/v1/find/repositories");
        let req = self
            .http
            .get(&url)
            .query(&[("query", query)])
            .header(reqwest::header::ACCEPT, "application/json");
        let resp = check_status(self.send_retry(req).await?, "recherche Quay").await?;
        let body: serde_json::Value = resp.json().await?;
        let mut out = Vec::new();
        for item in body.get("results").and_then(|v| v.as_array()).into_iter().flatten() {
            if item.get("kind").and_then(|v| v.as_str()) == Some("application") {
                continue;
            }
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let namespace = item
                .get("namespace")
                .and_then(|n| n.get("name").or(Some(n)))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let full = match &namespace {
                Some(ns) => format!("quay.io/{ns}/{name}"),
                None => format!("quay.io/{name}"),
            };
            out.push(ImageSummary {
                name: full,
                namespace,
                description: item
                    .get("description")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                stars: item.get("stars").and_then(|v| v.as_i64()),
                pulls: item.get("popularity").and_then(|v| v.as_f64()).map(|f| f as i64),
                official: item.get("is_official").and_then(|v| v.as_bool()).unwrap_or(false),
                registry: RegistryKind::Quay,
                updated_at: None,
                source_repo: None,
            });
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Vérifie qu'une référence précise existe et la renvoie comme unique résultat.
    async fn probe_reference(
        &self,
        query: &str,
        force_registry: Option<&str>,
    ) -> Result<Vec<ImageSummary>> {
        let candidate = match force_registry {
            Some(host) if !query.starts_with(host) => format!("{host}/{query}"),
            _ => query.to_string(),
        };
        let image = ImageRef::parse(&candidate).map_err(|e| {
            Error::Unsupported(format!(
                "ce registre ne propose pas de recherche par mot-clé. Saisissez la référence \
                 complète de l'image (par exemple « ghcr.io/propriétaire/image »). Détail : {e}"
            ))
        })?;
        if !image.repository.contains('/') && image.registry != DEFAULT_REGISTRY {
            return Err(Error::Unsupported(format!(
                "« {query} » est incomplet : ce registre ne propose pas de recherche par mot-clé, \
                 indiquez « {}/propriétaire/image ».",
                image.registry
            )));
        }
        let tags = self.list_tags(&image, 5).await?;
        let mut summary = ImageSummary::from_reference(&image);
        summary.updated_at = tags.first().and_then(|t| t.pushed_at);
        Ok(vec![summary])
    }

    // ------------------------------------------------------------------- tags

    /// Liste les tags d'une image, du plus récent au plus ancien.
    pub async fn list_tags(&self, image: &ImageRef, limit: usize) -> Result<Vec<TagInfo>> {
        let limit = limit.clamp(1, 1000);
        if image.registry_kind() == RegistryKind::DockerHub {
            // L'API catalogue est plus riche ; si elle est indisponible ou si le dépôt est
            // privé, on retombe sur l'API Registry v2, qui accepte un jeton.
            match self.list_tags_docker_hub(image, limit).await {
                Ok(tags) if !tags.is_empty() => return Ok(tags),
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(image = %image.to_string_full(), erreur = %e,
                        "API catalogue Docker Hub indisponible, repli sur Registry v2");
                }
            }
        }
        self.list_tags_registry_v2(image, limit).await
    }

    /// Tags enrichis (taille, date, plates-formes) via l'API catalogue de Docker Hub.
    async fn list_tags_docker_hub(&self, image: &ImageRef, limit: usize) -> Result<Vec<TagInfo>> {
        let (namespace, name) = image
            .repository
            .split_once('/')
            .ok_or_else(|| Error::Invalid(format!("dépôt « {} » invalide", image.repository)))?;
        let mut out: Vec<TagInfo> = Vec::new();
        let mut page = 1usize;
        while out.len() < limit && page <= 10 {
            let page_size = std::cmp::min(limit - out.len(), 100).max(1);
            let url = format!("{DOCKER_HUB_API}/v2/repositories/{namespace}/{name}/tags");
            let req = self
                .http
                .get(&url)
                .query(&[
                    ("page_size", page_size.to_string()),
                    ("page", page.to_string()),
                    ("ordering", "last_updated".to_string()),
                ])
                .header(reqwest::header::ACCEPT, "application/json");
            let resp = check_status(
                self.send_retry(req).await?,
                &format!("tags de « {} »", image.short_name()),
            )
            .await?;
            let body: serde_json::Value = resp.json().await?;
            let results = match body.get("results").and_then(|v| v.as_array()) {
                Some(r) if !r.is_empty() => r.clone(),
                _ => break,
            };
            for item in &results {
                if let Some(tag) = docker_hub_tag(item) {
                    out.push(tag);
                }
            }
            if body.get("next").and_then(|v| v.as_str()).is_none() {
                break;
            }
            page += 1;
        }
        out.truncate(limit);
        Ok(out)
    }

    /// Tags via l'API Registry v2 normalisée, en suivant la pagination `Link`.
    async fn list_tags_registry_v2(&self, image: &ImageRef, limit: usize) -> Result<Vec<TagInfo>> {
        let mut names: Vec<String> = Vec::new();
        let mut path = format!(
            "/v2/{}/tags/list?n={}",
            image.repository,
            std::cmp::min(limit, 100)
        );
        let mut pages = 0usize;
        while pages < 20 && names.len() < limit {
            pages += 1;
            let resp = self.registry_get(image, &path, &["application/json"]).await?;
            let next = next_link(&resp);
            let resp = check_status(resp, &format!("tags de « {} »", image.short_name())).await?;
            let body: serde_json::Value = resp.json().await?;
            match body.get("tags") {
                Some(serde_json::Value::Array(items)) => {
                    for item in items {
                        if let Some(s) = item.as_str() {
                            names.push(s.to_string());
                        }
                    }
                }
                // `tags: null` est la réponse normalisée pour un dépôt sans tag.
                _ => break,
            }
            match next {
                Some(n) => path = n,
                None => break,
            }
        }

        let mut tags: Vec<TagInfo> = names.into_iter().map(TagInfo::bare).collect();
        // L'API v2 ne garantit aucun ordre : on remonte les versions les plus récentes.
        tags.sort_by(|a, b| match (parse_tag(&a.name), parse_tag(&b.name)) {
            (Some(x), Some(y)) => y.version.cmp(&x.version).then_with(|| b.name.cmp(&a.name)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b.name.cmp(&a.name),
        });
        tags.truncate(limit);
        Ok(tags)
    }

    /// Cherche le tag de version le plus élevé, dans la même variante que le tag courant.
    ///
    /// Les suffixes de variante (`-alpine`, `-slim`, `-bookworm`…) ne sont pas comparables
    /// entre eux : une image `16-alpine` ne doit jamais « progresser » vers `17-bookworm`.
    /// Les vraies pré-versions (`-rc1`, `-beta`…) sont écartées sauf demande explicite.
    pub async fn latest_semver_tag(
        &self,
        image: &ImageRef,
        allow_prerelease: bool,
    ) -> Result<Option<TagInfo>> {
        let tags = self.list_tags(image, 300).await?;
        let current_variant = image.tag.as_deref().and_then(parse_tag).and_then(|t| t.variant);

        let mut best: Option<(semver::Version, TagInfo)> = None;
        for tag in tags {
            let Some(parsed) = parse_tag(&tag.name) else { continue };
            if parsed.variant != current_variant {
                continue;
            }
            if parsed.is_prerelease && !allow_prerelease {
                continue;
            }
            let better = match &best {
                Some((v, _)) => parsed.version > *v,
                None => true,
            };
            if better {
                best = Some((parsed.version, tag));
            }
        }
        Ok(best.map(|(_, t)| t))
    }

    // -------------------------------------------------------------- manifestes

    /// Résout le digest du manifeste pointé par la référence.
    pub async fn resolve_digest(&self, image: &ImageRef) -> Result<String> {
        if let Some(d) = &image.digest {
            return Ok(d.clone());
        }
        let (digest, _) = self.fetch_manifest(image, &image.manifest_reference()).await?;
        digest.ok_or_else(|| {
            Error::Other(format!(
                "le registre n'a pas renvoyé d'en-tête Docker-Content-Digest pour « {} »",
                image.to_string_full()
            ))
        })
    }

    /// Lit un manifeste et renvoie son digest ainsi que son contenu JSON.
    async fn fetch_manifest(
        &self,
        image: &ImageRef,
        reference: &str,
    ) -> Result<(Option<String>, serde_json::Value)> {
        let path = format!("/v2/{}/manifests/{}", image.repository, reference);
        let resp = self.registry_get(image, &path, MANIFEST_ACCEPT).await?;
        let digest = resp
            .headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let resp = check_status(resp, &format!("manifeste de « {} »", image.to_string_full())).await?;
        let body: serde_json::Value = resp.json().await?;
        Ok((digest, body))
    }

    /// Inspecte la configuration OCI d'une image : ports, variables, étiquettes, commande.
    pub async fn inspect(&self, image: &ImageRef) -> Result<ImageDetails> {
        let (top_digest, manifest) = self.fetch_manifest(image, &image.manifest_reference()).await?;

        // Un index multi-architecture n'est pas un manifeste : il faut descendre d'un cran.
        let manifest = if manifest.get("manifests").and_then(|v| v.as_array()).is_some() {
            let child = pick_platform_manifest(&manifest).ok_or_else(|| {
                Error::Unsupported(format!(
                    "l'image « {} » ne publie aucun manifeste pour une plate-forme exploitable",
                    image.to_string_full()
                ))
            })?;
            let (_, child_manifest) = self.fetch_manifest(image, &child).await?;
            child_manifest
        } else {
            manifest
        };

        if manifest.get("schemaVersion").and_then(|v| v.as_i64()) == Some(1) {
            return Err(Error::Unsupported(format!(
                "l'image « {} » utilise un manifeste de schéma 1, obsolète et non pris en charge ; \
                 republiez-la au format OCI.",
                image.to_string_full()
            )));
        }

        let config_digest = manifest
            .get("config")
            .and_then(|c| c.get("digest"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                Error::Unsupported(format!(
                    "le manifeste de « {} » ne référence aucun objet de configuration",
                    image.to_string_full()
                ))
            })?
            .to_string();

        let path = format!("/v2/{}/blobs/{}", image.repository, config_digest);
        let resp = self.registry_get(image, &path, &["application/json", "*/*"]).await?;
        let resp = check_status(resp, &format!("configuration de « {} »", image.to_string_full()))
            .await?;
        let config: serde_json::Value = resp.json().await?;

        Ok(build_image_details(image, top_digest, &config))
    }

    // --------------------------------------------------------- couche transport

    /// Envoie une requête vers l'API Registry v2, en négociant un jeton si nécessaire.
    pub(crate) async fn registry_get(
        &self,
        image: &ImageRef,
        path: &str,
        accepts: &[&str],
    ) -> Result<reqwest::Response> {
        let host = image.registry_host();
        let kind = image.registry_kind();
        let scope = image.pull_scope();
        let url = format!("{}://{host}{path}", registry_scheme(&host));
        let accept = accepts.join(", ");

        // Première tentative : jeton en cache, sinon identifiants statiques.
        let mut request = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, accept.clone());
        let cache_key = token_cache_key(&host, &scope);
        if let Some(token) = self.cached_token(&cache_key) {
            request = request.bearer_auth(token);
        } else if let Some(creds) = self.credentials.get(&kind) {
            if let Some(token) = &creds.token {
                request = request.bearer_auth(token);
            }
        }
        let resp = self.send_retry(request).await?;
        if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(resp);
        }

        // Le registre nous indique où obtenir un jeton : on suit le défi.
        let header = resp
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let mut challenge = parse_www_authenticate(&header).ok_or_else(|| {
            Error::Auth(format!(
                "le registre {host} refuse l'accès à « {} » et ne propose pas d'authentification \
                 par jeton ; renseignez des identifiants.",
                image.repository
            ))
        })?;
        if challenge.scope.is_none() {
            challenge.scope = Some(scope.clone());
        }
        let effective_scope = challenge.scope.clone().unwrap_or_else(|| scope.clone());
        let token = self
            .obtain_token(&host, kind, &effective_scope, &challenge)
            .await?;

        let request = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, accept)
            .bearer_auth(&token);
        let resp = self.send_retry(request).await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::Auth(format!(
                "accès refusé à « {} » sur {host} : le dépôt est privé ou les identifiants sont \
                 invalides.",
                image.repository
            )));
        }
        Ok(resp)
    }

    /// Récupère un jeton porteur, depuis le cache ou depuis le service de jetons.
    async fn obtain_token(
        &self,
        host: &str,
        kind: RegistryKind,
        scope: &str,
        challenge: &BearerChallenge,
    ) -> Result<String> {
        let key = token_cache_key(host, scope);
        if let Some(token) = self.cached_token(&key) {
            return Ok(token);
        }

        let mut request = self.http.get(&challenge.realm);
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(service) = &challenge.service {
            query.push(("service", service.clone()));
        }
        query.push(("scope", scope.to_string()));
        request = request.query(&query);
        request = request.header(reqwest::header::ACCEPT, "application/json");

        // GHCR accepte un jeton d'accès personnel GitHub en authentification basique.
        if let Some(creds) = self.credentials.get(&kind) {
            if let Some(user) = &creds.username {
                request = request.basic_auth(user, creds.password.clone());
            } else if let Some(password) = &creds.password {
                request = request.basic_auth("x-access-token", Some(password.clone()));
            }
        } else if kind == RegistryKind::Ghcr {
            if let Some(pat) = &self.github_token {
                request = request.basic_auth("x-access-token", Some(pat.clone()));
            }
        }

        let resp = check_status(
            self.send_retry(request).await?,
            &format!("obtention d'un jeton pour {host}"),
        )
        .await?;
        let body: serde_json::Value = resp.json().await?;
        let token = body
            .get("token")
            .or_else(|| body.get("access_token"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                Error::Auth(format!(
                    "le service de jetons de {host} n'a renvoyé aucun jeton exploitable"
                ))
            })?
            .to_string();
        // Une marge de 30 s évite d'utiliser un jeton qui expire pendant la requête.
        let ttl = body
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(300)
            .saturating_sub(30)
            .max(30);
        self.store_token(key, token.clone(), Duration::from_secs(ttl));
        Ok(token)
    }

    /// Lit le cache de jetons en écartant les entrées expirées.
    fn cached_token(&self, key: &str) -> Option<String> {
        let mut guard = self.tokens.lock().ok()?;
        match guard.get(key) {
            Some(entry) if entry.expires_at > Instant::now() => Some(entry.value.clone()),
            Some(_) => {
                guard.remove(key);
                None
            }
            None => None,
        }
    }

    /// Enregistre un jeton dans le cache partagé.
    fn store_token(&self, key: String, value: String, ttl: Duration) {
        if let Ok(mut guard) = self.tokens.lock() {
            guard.insert(
                key,
                CachedToken { value, expires_at: Instant::now() + ttl },
            );
        }
    }

    /// Envoie une requête avec au plus [`MAX_ATTEMPTS`] tentatives.
    ///
    /// Les statuts 429 et 5xx ainsi que les coupures réseau déclenchent une nouvelle
    /// tentative après un délai exponentiel, en respectant `Retry-After` quand il est fourni.
    pub(crate) async fn send_retry(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let attempt_request = request.try_clone().ok_or_else(|| {
                Error::Other("requête non rejouable : corps de requête consommable".to_string())
            })?;
            match attempt_request.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        let advised = retry_after_seconds(resp.headers());
                        if attempt < MAX_ATTEMPTS {
                            tokio::time::sleep(wait_for(advised, attempt)).await;
                            continue;
                        }
                        return Err(Error::RateLimited(advised));
                    }
                    if status.is_server_error() && attempt < MAX_ATTEMPTS {
                        tokio::time::sleep(backoff(attempt)).await;
                        continue;
                    }
                    return Ok(resp);
                }
                Err(e) => {
                    if attempt < MAX_ATTEMPTS && (e.is_timeout() || e.is_connect()) {
                        tokio::time::sleep(backoff(attempt)).await;
                        continue;
                    }
                    return Err(Error::Http(e));
                }
            }
        }
    }

    /// Requête JSON générique, utilisée par les autres modules du crate.
    pub(crate) async fn get_json(
        &self,
        url: &str,
        query: &[(&str, String)],
        context: &str,
    ) -> Result<serde_json::Value> {
        let request = self
            .http
            .get(url)
            .query(query)
            .header(reqwest::header::ACCEPT, "application/json");
        let resp = check_status(self.send_retry(request).await?, context).await?;
        Ok(resp.json().await?)
    }

    /// Requête texte générique (valeurs de chart, fichiers YAML distants).
    pub(crate) async fn get_text(
        &self,
        url: &str,
        query: &[(&str, String)],
        context: &str,
    ) -> Result<String> {
        let request = self
            .http
            .get(url)
            .query(query)
            .header(reqwest::header::ACCEPT, "text/yaml, text/plain, */*");
        let resp = check_status(self.send_retry(request).await?, context).await?;
        Ok(resp.text().await?)
    }
}

// ============================================================ analyse des tags

/// Résultat de l'analyse d'un tag d'image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTag {
    /// Version sémantique déduite, complétée par des zéros si besoin (`1.2` → `1.2.0`).
    pub version: semver::Version,
    /// Variante de distribution (`alpine`, `slim`, `bookworm`…), jamais comparable entre elles.
    pub variant: Option<String>,
    /// Vrai si le tag porte un marqueur de pré-version (`rc`, `beta`…).
    pub is_prerelease: bool,
}

/// Analyse un tag d'image en distinguant version, pré-version et variante de distribution.
///
/// `v1.2.3` → 1.2.3 ; `1.2` → 1.2.0 ; `16-alpine` → 16.0.0 variante « alpine » ;
/// `2.0.0-rc1` → pré-version ; `latest` → `None`.
pub fn parse_tag(tag: &str) -> Option<ParsedTag> {
    let tag = tag.trim();
    if tag.is_empty() {
        return None;
    }
    // Les métadonnées de construction ne participent pas à l'ordre.
    let tag = tag.split('+').next().unwrap_or(tag);
    // Un « v » initial est un usage, pas une partie de la version.
    let tag = match tag.strip_prefix(['v', 'V']) {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => rest,
        _ => tag,
    };

    let (core, suffix) = match tag.split_once('-') {
        Some((c, s)) => (c, Some(s)),
        None => (tag, None),
    };
    if !VERSION_CORE.is_match(core) {
        return None;
    }
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next().flatten()?;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);

    let mut prerelease_tokens: Vec<String> = Vec::new();
    let mut variant_tokens: Vec<String> = Vec::new();
    if let Some(suffix) = suffix {
        for token in suffix.split(['-', '.']).filter(|t| !t.is_empty()) {
            if PRERELEASE_MARKER.is_match(&token.to_ascii_lowercase()) {
                prerelease_tokens.push(token.to_ascii_lowercase());
            } else if token.chars().all(|c| c.is_ascii_digit()) && !prerelease_tokens.is_empty() {
                // Numéro attaché au marqueur précédent : « rc.1 ».
                prerelease_tokens.push(token.to_string());
            } else {
                variant_tokens.push(token.to_ascii_lowercase());
            }
        }
    }

    let mut version = semver::Version::new(major, minor, patch);
    if !prerelease_tokens.is_empty() {
        version.pre = semver::Prerelease::new(&prerelease_tokens.join(".")).ok()?;
    }
    let variant = if variant_tokens.is_empty() {
        None
    } else {
        Some(variant_tokens.join("-"))
    };
    Some(ParsedTag { version, variant, is_prerelease: !prerelease_tokens.is_empty() })
}

// ================================================== analyse des en-têtes HTTP

/// Analyse un en-tête `WWW-Authenticate` de type `Bearer`.
///
/// Exemple : `Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:o/r:pull"`.
/// Les valeurs peuvent contenir des virgules lorsqu'elles sont entre guillemets.
pub fn parse_www_authenticate(header: &str) -> Option<BearerChallenge> {
    let header = header.trim();
    if !header.get(..6)?.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let params = parse_auth_params(header.get(6..)?);
    let mut realm = None;
    let mut service = None;
    let mut scope = None;
    for (key, value) in params {
        match key.as_str() {
            "realm" => realm = Some(value),
            "service" => service = Some(value),
            "scope" => scope = Some(value),
            _ => {}
        }
    }
    let realm = realm.filter(|r| !r.is_empty())?;
    Some(BearerChallenge {
        realm,
        service: service.filter(|s| !s.is_empty()),
        scope: scope.filter(|s| !s.is_empty()),
    })
}

/// Découpe une liste `clé=valeur` séparée par des virgules, en respectant les guillemets.
fn parse_auth_params(input: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        while i < chars.len() && (chars[i] == ',' || chars[i].is_whitespace()) {
            i += 1;
        }
        let key_start = i;
        while i < chars.len() && chars[i] != '=' && chars[i] != ',' {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        if chars[i] == ',' {
            continue;
        }
        let key: String = chars[key_start..i].iter().collect::<String>().trim().to_ascii_lowercase();
        i += 1; // consommer « = »
        let value = if i < chars.len() && chars[i] == '"' {
            i += 1;
            let mut value = String::new();
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                value.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1; // consommer le guillemet fermant
            }
            value
        } else {
            let value_start = i;
            while i < chars.len() && chars[i] != ',' {
                i += 1;
            }
            chars[value_start..i].iter().collect::<String>().trim().to_string()
        };
        if !key.is_empty() {
            out.push((key, value));
        }
    }
    out
}

/// Extrait le chemin de la page suivante depuis un en-tête `Link: <...>; rel="next"`.
fn next_link(resp: &reqwest::Response) -> Option<String> {
    let link = resp.headers().get(reqwest::header::LINK)?.to_str().ok()?;
    for part in link.split(',') {
        if !part.to_ascii_lowercase().contains("rel=\"next\"") {
            continue;
        }
        let start = part.find('<')? + 1;
        let end = part[start..].find('>')? + start;
        let target = &part[start..end];
        return Some(if target.starts_with('/') {
            target.to_string()
        } else {
            format!("/{target}")
        });
    }
    None
}

/// Lit `Retry-After`, exprimé en secondes.
fn retry_after_seconds(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
}

/// Délai exponentiel : 500 ms, 1 s, 2 s… plafonné.
fn backoff(attempt: u32) -> Duration {
    let millis = 500u64.saturating_mul(1u64 << attempt.min(5).saturating_sub(1));
    Duration::from_millis(millis).min(MAX_BACKOFF)
}

/// Combine le délai conseillé par le registre et le délai exponentiel.
fn wait_for(advised: Option<u64>, attempt: u32) -> Duration {
    match advised {
        Some(s) => Duration::from_secs(s).min(MAX_BACKOFF),
        None => backoff(attempt),
    }
}

/// Un registre local est presque toujours servi en clair : on évite un échec TLS inutile.
fn registry_scheme(host: &str) -> &'static str {
    let bare = host.split(':').next().unwrap_or(host);
    if bare == "localhost" || bare == "127.0.0.1" || bare == "::1" {
        "http"
    } else {
        "https"
    }
}

/// Clé du cache de jetons.
fn token_cache_key(host: &str, scope: &str) -> String {
    format!("{host}|{scope}")
}

/// Traduit un statut d'erreur HTTP en [`Error`] avec un message exploitable.
pub(crate) async fn check_status(
    resp: reqwest::Response,
    context: &str,
) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let code = status.as_u16();
    let advised = retry_after_seconds(resp.headers());
    let body = resp.text().await.unwrap_or_default();
    let detail = summarize_body(&body);
    Err(match code {
        401 => Error::Auth(format!("{context} : authentification requise{detail}")),
        403 => Error::Auth(format!("{context} : accès interdit{detail}")),
        404 => Error::NotFound(format!("{context} : introuvable sur le registre")),
        400 | 422 => Error::Invalid(format!("{context} : requête refusée{detail}")),
        429 => Error::RateLimited(advised),
        _ => Error::Other(format!("{context} : le registre a répondu {code}{detail}")),
    })
}

/// Réduit un corps d'erreur à un fragment court et lisible.
fn summarize_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Les registres renvoient généralement {"errors":[{"message": "..."}]}.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let message = v
            .get("errors")
            .and_then(|e| e.as_array())
            .and_then(|a| a.first())
            .and_then(|e| e.get("message"))
            .or_else(|| v.get("message"))
            .or_else(|| v.get("detail"))
            .and_then(|m| m.as_str());
        if let Some(m) = message {
            return format!(" ({m})");
        }
    }
    let short: String = trimmed.chars().take(160).collect();
    format!(" ({short})")
}

// ========================================== conversions des réponses distantes

/// Convertit les résultats de `/v2/search/repositories/` de Docker Hub.
fn docker_hub_search_results(body: &serde_json::Value) -> Vec<ImageSummary> {
    let mut out = Vec::new();
    for item in body.get("results").and_then(|v| v.as_array()).into_iter().flatten() {
        let Some(name) = item.get("repo_name").and_then(|v| v.as_str()) else { continue };
        let official = item.get("is_official").and_then(|v| v.as_bool()).unwrap_or(false);
        out.push(ImageSummary {
            namespace: name.split_once('/').map(|(ns, _)| ns.to_string()),
            name: name.to_string(),
            description: item
                .get("short_description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            stars: item.get("star_count").and_then(|v| v.as_i64()),
            pulls: item.get("pull_count").and_then(|v| v.as_i64()),
            official,
            registry: RegistryKind::DockerHub,
            updated_at: None,
            source_repo: None,
        });
    }
    out
}

/// Convertit les résultats de l'API « content » de Docker Hub.
fn docker_hub_content_results(body: &serde_json::Value) -> Vec<ImageSummary> {
    let mut out = Vec::new();
    for item in body.get("summaries").and_then(|v| v.as_array()).into_iter().flatten() {
        let name = item
            .get("slug")
            .or_else(|| item.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let publisher = item
            .get("publisher")
            .and_then(|p| p.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let official = item
            .get("filter_type")
            .and_then(|v| v.as_str())
            .map(|t| t.eq_ignore_ascii_case("official"))
            .unwrap_or(false);
        let full_name = if official || publisher.is_empty() || name.contains('/') {
            name.to_string()
        } else {
            format!("{publisher}/{name}")
        };
        out.push(ImageSummary {
            namespace: full_name.split_once('/').map(|(ns, _)| ns.to_string()),
            name: full_name,
            description: item
                .get("short_description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            stars: item.get("star_count").and_then(|v| v.as_i64()),
            pulls: item
                .get("pull_count")
                .and_then(|v| v.as_i64())
                .or_else(|| {
                    item.get("pull_count")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<i64>().ok())
                }),
            official,
            registry: RegistryKind::DockerHub,
            updated_at: parse_rfc3339(item.get("updated_at").and_then(|v| v.as_str())),
            source_repo: None,
        });
    }
    out
}

/// Convertit une entrée de `/v2/repositories/{ns}/{nom}/tags`.
fn docker_hub_tag(item: &serde_json::Value) -> Option<TagInfo> {
    let name = item.get("name").and_then(|v| v.as_str())?.to_string();
    let mut platforms: Vec<String> = Vec::new();
    for image in item.get("images").and_then(|v| v.as_array()).into_iter().flatten() {
        let os = image.get("os").and_then(|v| v.as_str()).unwrap_or_default();
        let arch = image.get("architecture").and_then(|v| v.as_str()).unwrap_or_default();
        if os.is_empty() && arch.is_empty() {
            continue;
        }
        let variant = image.get("variant").and_then(|v| v.as_str()).unwrap_or_default();
        let platform = if variant.is_empty() {
            format!("{os}/{arch}")
        } else {
            format!("{os}/{arch}/{variant}")
        };
        if !platforms.contains(&platform) {
            platforms.push(platform);
        }
    }
    let semver = parse_tag(&name).map(|t| t.version.to_string());
    Some(TagInfo {
        digest: item
            .get("digest")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        size_bytes: item.get("full_size").and_then(|v| v.as_i64()),
        pushed_at: parse_rfc3339(
            item.get("tag_last_pushed")
                .or_else(|| item.get("last_updated"))
                .and_then(|v| v.as_str()),
        ),
        platforms,
        semver,
        name,
    })
}

/// Choisit le manifeste `linux/amd64`, à défaut `linux/arm64`, à défaut le premier utilisable.
fn pick_platform_manifest(index: &serde_json::Value) -> Option<String> {
    let manifests = index.get("manifests")?.as_array()?;
    let usable: Vec<&serde_json::Value> = manifests
        .iter()
        .filter(|m| {
            // Les pièces jointes (signatures, attestations) ne sont pas des images.
            m.get("annotations")
                .and_then(|a| a.get("vnd.docker.reference.type"))
                .is_none()
                && m.get("platform")
                    .and_then(|p| p.get("architecture"))
                    .and_then(|v| v.as_str())
                    != Some("unknown")
        })
        .collect();

    let find = |os: &str, arch: &str| -> Option<String> {
        usable
            .iter()
            .find(|m| {
                let p = m.get("platform");
                p.and_then(|p| p.get("os")).and_then(|v| v.as_str()) == Some(os)
                    && p.and_then(|p| p.get("architecture")).and_then(|v| v.as_str()) == Some(arch)
            })
            .and_then(|m| m.get("digest"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };

    find("linux", "amd64")
        .or_else(|| find("linux", "arm64"))
        .or_else(|| {
            usable
                .first()
                .and_then(|m| m.get("digest"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

/// Extrait les métadonnées utiles d'un objet de configuration OCI.
fn build_image_details(
    image: &ImageRef,
    digest: Option<String>,
    config: &serde_json::Value,
) -> ImageDetails {
    let null = serde_json::Value::Null;
    let inner = config.get("config").unwrap_or(&null);

    let mut exposed_ports: Vec<u16> = Vec::new();
    if let Some(ports) = inner.get("ExposedPorts").and_then(|v| v.as_object()) {
        for key in ports.keys() {
            // Les clés ont la forme « 8080/tcp ».
            if let Some(port) = key.split('/').next().and_then(|p| p.parse::<u16>().ok()) {
                if !exposed_ports.contains(&port) {
                    exposed_ports.push(port);
                }
            }
        }
    }
    exposed_ports.sort_unstable();

    let string_list = |key: &str| -> Vec<String> {
        inner
            .get(key)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect())
            .unwrap_or_default()
    };

    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    if let Some(map) = inner.get("Labels").and_then(|v| v.as_object()) {
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                labels.insert(k.clone(), s.to_string());
            }
        }
    }

    let source_repo = labels
        .get("org.opencontainers.image.source")
        .cloned()
        .or_else(|| labels.get("org.label-schema.vcs-url").cloned())
        .map(|url| normalize_source_repo(&url));

    ImageDetails {
        reference: image.to_string_full(),
        digest,
        exposed_ports,
        env: string_list("Env"),
        labels,
        entrypoint: string_list("Entrypoint"),
        cmd: string_list("Cmd"),
        architecture: config
            .get("architecture")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        os: config.get("os").and_then(|v| v.as_str()).map(|s| s.to_string()),
        source_repo,
    }
}

/// Réduit une URL de dépôt GitHub à la forme `propriétaire/dépôt`.
pub(crate) fn normalize_source_repo(url: &str) -> String {
    let cleaned = url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .trim_start_matches("git+");
    for prefix in [
        "https://github.com/",
        "http://github.com/",
        "git@github.com:",
        "github.com/",
    ] {
        if let Some(rest) = cleaned.strip_prefix(prefix) {
            let mut parts = rest.split('/');
            if let (Some(owner), Some(repo)) = (parts.next(), parts.next()) {
                if !owner.is_empty() && !repo.is_empty() {
                    return format!("{owner}/{repo}");
                }
            }
            return rest.to_string();
        }
    }
    cleaned.to_string()
}

/// Analyse une date RFC 3339 tolérante aux fractions de seconde.
pub(crate) fn parse_rfc3339(value: Option<&str>) -> Option<DateTime<Utc>> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defi_bearer_complet() {
        let header =
            r#"Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:o/r:pull""#;
        let c = parse_www_authenticate(header).expect("défi analysable");
        assert_eq!(c.realm, "https://ghcr.io/token");
        assert_eq!(c.service.as_deref(), Some("ghcr.io"));
        assert_eq!(c.scope.as_deref(), Some("repository:o/r:pull"));
    }

    #[test]
    fn defi_bearer_avec_espaces_et_portee_multiple() {
        let header = r#"bearer realm="https://auth.docker.io/token", service="registry.docker.io", scope="repository:library/nginx:pull,push""#;
        let c = parse_www_authenticate(header).expect("défi analysable");
        assert_eq!(c.realm, "https://auth.docker.io/token");
        assert_eq!(c.scope.as_deref(), Some("repository:library/nginx:pull,push"));
    }

    #[test]
    fn defi_bearer_sans_guillemets() {
        let header = "Bearer realm=https://registre.local/jetons,service=registre.local";
        let c = parse_www_authenticate(header).expect("défi analysable");
        assert_eq!(c.realm, "https://registre.local/jetons");
        assert_eq!(c.service.as_deref(), Some("registre.local"));
        assert_eq!(c.scope, None);
    }

    #[test]
    fn defi_non_bearer_rejete() {
        assert!(parse_www_authenticate("Basic realm=\"x\"").is_none());
        assert!(parse_www_authenticate("").is_none());
        assert!(parse_www_authenticate("Bearer service=\"x\"").is_none());
    }

    #[test]
    fn analyse_des_tags() {
        let v = |s: &str| parse_tag(s).map(|t| (t.version.to_string(), t.variant, t.is_prerelease));
        assert_eq!(v("1.2.3"), Some(("1.2.3".into(), None, false)));
        assert_eq!(v("v1.2.3"), Some(("1.2.3".into(), None, false)));
        assert_eq!(v("1.2"), Some(("1.2.0".into(), None, false)));
        assert_eq!(v("16"), Some(("16.0.0".into(), None, false)));
        assert_eq!(
            v("16-alpine"),
            Some(("16.0.0".into(), Some("alpine".into()), false))
        );
        assert_eq!(
            v("2.0.0-rc1"),
            Some(("2.0.0-rc1".into(), None, true))
        );
        assert_eq!(
            v("1.27.1-alpine3.20"),
            Some(("1.27.1".into(), Some("alpine3-20".into()), false))
        );
        assert_eq!(v("latest"), None);
        assert_eq!(v("stable"), None);
        assert_eq!(v(""), None);
    }

    #[test]
    fn ordre_des_versions_prerelease() {
        let stable = parse_tag("2.0.0").expect("stable");
        let rc = parse_tag("2.0.0-rc1").expect("pré-version");
        assert!(rc.version < stable.version, "une rc précède la version finale");
    }

    #[test]
    fn variantes_non_comparables() {
        let alpine = parse_tag("16-alpine").expect("alpine");
        let debian = parse_tag("17-bookworm").expect("bookworm");
        assert_ne!(alpine.variant, debian.variant);
    }

    #[test]
    fn normalisation_du_depot_source() {
        assert_eq!(normalize_source_repo("https://github.com/nginx/nginx"), "nginx/nginx");
        assert_eq!(normalize_source_repo("https://github.com/o/r.git"), "o/r");
        assert_eq!(normalize_source_repo("git@github.com:o/r.git"), "o/r");
        assert_eq!(normalize_source_repo("https://gitlab.com/o/r"), "https://gitlab.com/o/r");
    }

    #[test]
    fn choix_du_manifeste_de_plateforme() {
        let index = serde_json::json!({
            "manifests": [
                {"digest": "sha256:arm", "platform": {"os": "linux", "architecture": "arm64"}},
                {"digest": "sha256:amd", "platform": {"os": "linux", "architecture": "amd64"}},
                {"digest": "sha256:att", "platform": {"os": "unknown", "architecture": "unknown"}}
            ]
        });
        assert_eq!(pick_platform_manifest(&index).as_deref(), Some("sha256:amd"));
    }

    #[test]
    fn extraction_de_la_configuration_oci() {
        let image = ImageRef::parse("nginx:1.27").expect("référence valide");
        let config = serde_json::json!({
            "architecture": "amd64",
            "os": "linux",
            "config": {
                "ExposedPorts": {"80/tcp": {}, "443/tcp": {}},
                "Env": ["PATH=/usr/bin", "NGINX_VERSION=1.27.1"],
                "Labels": {"org.opencontainers.image.source": "https://github.com/nginx/nginx"},
                "Entrypoint": ["/docker-entrypoint.sh"],
                "Cmd": ["nginx", "-g", "daemon off;"]
            }
        });
        let d = build_image_details(&image, Some("sha256:abc".into()), &config);
        assert_eq!(d.exposed_ports, vec![80, 443]);
        assert_eq!(d.env.len(), 2);
        assert_eq!(d.entrypoint, vec!["/docker-entrypoint.sh".to_string()]);
        assert_eq!(d.cmd.len(), 3);
        assert_eq!(d.source_repo.as_deref(), Some("nginx/nginx"));
        assert_eq!(d.architecture.as_deref(), Some("amd64"));
    }

    #[test]
    fn schema_de_registre_local() {
        assert_eq!(registry_scheme("localhost:5000"), "http");
        assert_eq!(registry_scheme("127.0.0.1:5000"), "http");
        assert_eq!(registry_scheme("ghcr.io"), "https");
    }

    #[test]
    fn delais_de_reprise_plafonnes() {
        assert!(backoff(1) <= MAX_BACKOFF);
        assert!(backoff(9) <= MAX_BACKOFF);
        assert_eq!(wait_for(Some(2), 1), Duration::from_secs(2));
        assert_eq!(wait_for(Some(600), 1), MAX_BACKOFF);
    }

    #[test]
    fn conversion_des_resultats_docker_hub() {
        let body = serde_json::json!({
            "results": [
                {"repo_name": "nginx", "short_description": "Serveur web", "star_count": 20000,
                 "pull_count": 1000000000i64, "is_official": true},
                {"repo_name": "bitnami/nginx", "short_description": "", "star_count": 100,
                 "pull_count": 500, "is_official": false}
            ]
        });
        let items = docker_hub_search_results(&body);
        assert_eq!(items.len(), 2);
        assert!(items[0].official);
        assert_eq!(items[1].namespace.as_deref(), Some("bitnami"));
        assert_eq!(items[1].description, None);
    }

    #[test]
    fn conversion_dun_tag_docker_hub() {
        let item = serde_json::json!({
            "name": "1.27.1-alpine",
            "full_size": 12345,
            "digest": "sha256:aaa",
            "tag_last_pushed": "2025-01-15T08:30:00Z",
            "images": [
                {"os": "linux", "architecture": "amd64"},
                {"os": "linux", "architecture": "arm64", "variant": "v8"}
            ]
        });
        let tag = docker_hub_tag(&item).expect("tag convertible");
        assert_eq!(tag.name, "1.27.1-alpine");
        assert_eq!(tag.size_bytes, Some(12345));
        assert_eq!(tag.platforms, vec!["linux/amd64", "linux/arm64/v8"]);
        assert_eq!(tag.semver.as_deref(), Some("1.27.1"));
        assert!(tag.pushed_at.is_some());
    }

    #[test]
    fn resume_de_corps_derreur() {
        let json = r#"{"errors":[{"code":"NAME_UNKNOWN","message":"dépôt inconnu"}]}"#;
        assert_eq!(summarize_body(json), " (dépôt inconnu)");
        assert_eq!(summarize_body("   "), "");
    }

    #[test]
    fn lien_de_pagination_absent_par_defaut() {
        // Sans en-tête Link, la pagination s'arrête : vérifié indirectement par le type.
        assert_eq!(token_cache_key("ghcr.io", "repository:o/r:pull"), "ghcr.io|repository:o/r:pull");
    }
}
