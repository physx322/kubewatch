//! Client GitHub REST v3 : releases, tags et quota d'API.
//!
//! Aucune dépendance à Octocrab : on reste sur `reqwest` avec les en-têtes
//! recommandés par GitHub (`Accept`, `X-GitHub-Api-Version`, `User-Agent`).
//! Le jeton est optionnel ; sans jeton le quota anonyme est de 60 requêtes/heure,
//! ce qui suffit pour quelques watchers mais pas au-delà.

use crate::error::{Error, Result};
use crate::model::{ReleaseAsset, ReleaseInfo};
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Racine de l'API REST GitHub.
const API_BASE: &str = "https://api.github.com";
/// Version d'API épinglée, comme recommandé par GitHub.
const API_VERSION: &str = "2022-11-28";
/// Type MIME des réponses JSON de l'API v3.
const ACCEPT: &str = "application/vnd.github+json";
/// Nombre maximum d'éléments par page côté GitHub.
const MAX_PER_PAGE: usize = 100;

/// Agent utilisateur exigé par GitHub sur toutes les requêtes.
fn user_agent() -> String {
    format!("kubewatch/{}", env!("CARGO_PKG_VERSION"))
}

/// Client HTTP vers l'API GitHub.
#[derive(Debug, Clone)]
pub struct GithubClient {
    http: reqwest::Client,
    token: Option<String>,
}

impl GithubClient {
    /// Construit le client. Le jeton vide est traité comme absent.
    pub fn new(token: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(user_agent())
            .timeout(std::time::Duration::from_secs(20))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()?;
        let token = token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        Ok(Self { http, token })
    }

    /// Indique si un jeton est configuré.
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// Exécute un GET et renvoie `None` sur 404.
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Option<T>> {
        let url = format!("{API_BASE}{path}");
        let mut req = self
            .http
            .get(&url)
            .header("Accept", ACCEPT)
            .header("X-GitHub-Api-Version", API_VERSION);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        if !query.is_empty() {
            req = req.query(query);
        }

        let resp = req.send().await?;
        let status = resp.status().as_u16();

        if status == 404 {
            return Ok(None);
        }
        if status == 401 {
            return Err(Error::Auth(
                "jeton GitHub invalide ou expiré (401)".to_string(),
            ));
        }
        if status == 403 || status == 429 {
            return Err(rate_limit_error(&resp, status));
        }
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Other(format!(
                "GitHub {status} sur {path}: {}",
                truncate(&body, 300)
            )));
        }

        let body = resp.bytes().await?;
        let value = serde_json::from_slice::<T>(&body)?;
        Ok(Some(value))
    }

    /// Dernière release publiée d'un dépôt.
    ///
    /// `/releases/latest` ignore les pré-versions et renvoie 404 quand le dépôt
    /// ne publie que des tags : on se rabat alors sur `/releases` puis `/tags`.
    pub async fn latest_release(&self, owner: &str, repo: &str) -> Result<Option<ReleaseInfo>> {
        let (owner, repo) = sanitize_repo(owner, repo)?;
        let path = format!("/repos/{owner}/{repo}/releases/latest");
        if let Some(r) = self.get_json::<GhRelease>(&path, &[]).await? {
            return Ok(Some(r.into_model()));
        }

        // Repli 1 : la liste complète des releases (inclut brouillons et pré-versions).
        let releases = self.list_releases(&owner, &repo, MAX_PER_PAGE).await?;
        if let Some(best) = pick_best_release(&releases) {
            return Ok(Some(best));
        }

        // Repli 2 : le dépôt n'a que des tags.
        let tags = self.list_tags(&owner, &repo, MAX_PER_PAGE).await?;
        Ok(pick_best_tag(&tags))
    }

    /// Liste les releases, de la plus récente à la plus ancienne.
    pub async fn list_releases(
        &self,
        owner: &str,
        repo: &str,
        limit: usize,
    ) -> Result<Vec<ReleaseInfo>> {
        let (owner, repo) = sanitize_repo(owner, repo)?;
        let path = format!("/repos/{owner}/{repo}/releases");
        let pages: Vec<GhRelease> = self.paginate(&path, limit).await?;
        Ok(pages.into_iter().map(GhRelease::into_model).collect())
    }

    /// Liste les noms de tags, du plus récent au plus ancien selon GitHub.
    pub async fn list_tags(&self, owner: &str, repo: &str, limit: usize) -> Result<Vec<String>> {
        let (owner, repo) = sanitize_repo(owner, repo)?;
        let path = format!("/repos/{owner}/{repo}/tags");
        let pages: Vec<GhTag> = self.paginate(&path, limit).await?;
        Ok(pages.into_iter().map(|t| t.name).collect())
    }

    /// Nombre de requêtes restantes sur le quota `core`, ou `None` si inconnu.
    pub async fn rate_limit_remaining(&self) -> Option<u64> {
        match self.get_json::<GhRateLimit>("/rate_limit", &[]).await {
            Ok(Some(rl)) => rl
                .resources
                .core
                .map(|c| c.remaining)
                .or(rl.rate.map(|r| r.remaining)),
            Ok(None) => None,
            Err(e) => {
                tracing::debug!(erreur = %e, "lecture du quota GitHub impossible");
                None
            }
        }
    }

    /// Indique si le dépôt existe et est visible avec le jeton courant.
    pub async fn repo_exists(&self, owner: &str, repo: &str) -> Result<bool> {
        let (owner, repo) = sanitize_repo(owner, repo)?;
        let path = format!("/repos/{owner}/{repo}");
        Ok(self
            .get_json::<serde_json::Value>(&path, &[])
            .await?
            .is_some())
    }

    /// Parcourt une collection paginée jusqu'à `limit` éléments.
    async fn paginate<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        limit: usize,
    ) -> Result<Vec<T>> {
        let limit = limit.clamp(1, 1000);
        let mut out: Vec<T> = Vec::new();
        let mut page = 1usize;
        while out.len() < limit {
            let per_page = (limit - out.len()).min(MAX_PER_PAGE);
            let query = [
                ("per_page", per_page.to_string()),
                ("page", page.to_string()),
            ];
            let batch: Vec<T> = match self.get_json::<Vec<T>>(path, &query).await? {
                Some(b) => b,
                None => break,
            };
            let received = batch.len();
            out.extend(batch);
            if received < per_page || received == 0 {
                break;
            }
            page += 1;
            if page > 20 {
                break;
            }
        }
        out.truncate(limit);
        Ok(out)
    }
}

/// Construit l'erreur de quota à partir des en-têtes de la réponse.
fn rate_limit_error(resp: &reqwest::Response, status: u16) -> Error {
    let header = |name: &str| -> Option<String> {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    };

    if let Some(retry) = header("retry-after").and_then(|v| v.trim().parse::<u64>().ok()) {
        return Error::RateLimited(Some(retry));
    }

    let remaining = header("x-ratelimit-remaining").and_then(|v| v.trim().parse::<i64>().ok());
    if status == 429 || remaining == Some(0) {
        let wait = header("x-ratelimit-reset")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .map(|reset| (reset - Utc::now().timestamp()).clamp(0, 86_400) as u64);
        return Error::RateLimited(wait);
    }

    Error::Auth(format!(
        "accès refusé par GitHub ({status}) : jeton absent ou droits insuffisants"
    ))
}

/// Choisit la meilleure release d'une liste : d'abord les versions stables,
/// puis, à défaut, les pré-versions. Les brouillons sont toujours écartés.
fn pick_best_release(releases: &[ReleaseInfo]) -> Option<ReleaseInfo> {
    let usable: Vec<&ReleaseInfo> = releases.iter().filter(|r| !r.draft).collect();
    if usable.is_empty() {
        return None;
    }
    let stable: Vec<&ReleaseInfo> = usable.iter().copied().filter(|r| !r.prerelease).collect();
    let pool = if stable.is_empty() { usable } else { stable };

    let by_semver = pool
        .iter()
        .filter_map(|r| r.parsed_semver().map(|v| (v, *r)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, r)| r);

    by_semver.or_else(|| pool.first().copied()).cloned()
}

/// Choisit le meilleur tag d'une liste et le transforme en release synthétique.
fn pick_best_tag(tags: &[String]) -> Option<ReleaseInfo> {
    let best = tags
        .iter()
        .filter_map(|t| normalize_version(t, None).map(|v| (v, t)))
        .max_by(|a, b| a.0.cmp(&b.0));

    match best {
        Some((v, tag)) => {
            let mut r = ReleaseInfo::from_tag(tag.clone());
            r.prerelease = !v.pre.is_empty();
            r.semver = Some(v.to_string());
            Some(r)
        }
        None => tags.first().map(|t| ReleaseInfo::from_tag(t.clone())),
    }
}

/// Coupe une chaîne pour les messages d'erreur.
fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

/// Vérifie et nettoie un couple propriétaire/dépôt avant de l'insérer dans une URL.
fn sanitize_repo(owner: &str, repo: &str) -> Result<(String, String)> {
    let clean = |s: &str, what: &str| -> Result<String> {
        let s = s.trim().trim_matches('/');
        if s.is_empty() {
            return Err(Error::Invalid(format!("{what} GitHub vide")));
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(Error::Invalid(format!(
                "{what} GitHub invalide: {s:?} (caractères autorisés: a-z 0-9 - _ .)"
            )));
        }
        Ok(s.to_string())
    };
    Ok((clean(owner, "propriétaire")?, clean(repo, "dépôt")?))
}

/// Analyse une référence de dépôt GitHub.
///
/// Accepte `owner/repo`, `https://github.com/owner/repo(.git)`,
/// `github.com/owner/repo`, `git@github.com:owner/repo.git`, ainsi que les URL
/// pointant plus profond dans le dépôt (`.../owner/repo/releases/tag/v1`).
pub fn parse_repo(s: &str) -> Result<(String, String)> {
    let raw = s.trim();
    if raw.is_empty() {
        return Err(Error::Invalid("référence de dépôt GitHub vide".to_string()));
    }

    let mut rest = raw;
    for prefix in ["https://", "http://", "git+https://", "ssh://"] {
        if let Some(r) = rest.strip_prefix(prefix) {
            rest = r;
            break;
        }
    }
    if let Some(r) = rest.strip_prefix("git@github.com:") {
        rest = r;
    }
    for prefix in ["www.github.com/", "github.com/", "api.github.com/repos/"] {
        if let Some(r) = rest.strip_prefix(prefix) {
            rest = r;
            break;
        }
    }
    rest = rest.trim_start_matches('/');
    // Retire une éventuelle query string ou ancre.
    rest = rest.split(['?', '#']).next().unwrap_or(rest);

    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let owner = parts
        .next()
        .ok_or_else(|| Error::Invalid(format!("dépôt GitHub illisible: {raw:?}")))?;
    let repo = parts
        .next()
        .ok_or_else(|| Error::Invalid(format!("dépôt GitHub illisible: {raw:?}")))?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);

    sanitize_repo(owner, repo)
}

/// Préfixes de tag couramment utilisés et retirés d'office.
const KNOWN_PREFIXES: &[&str] = &[
    "v", "ver", "version", "r", "rel", "release", "releases", "tag", "build",
];

/// Mots-clés qui ne sont jamais des versions.
const NOT_VERSIONS: &[&str] = &[
    "latest", "main", "master", "stable", "edge", "dev", "devel", "nightly", "head", "canary",
    "rolling", "current", "next", "beta", "alpha", "rc", "snapshot", "unstable", "test", "slim",
    "alpine", "bookworm", "bullseye", "buster", "focal", "jammy", "noble",
];

/// Convertit un tag amont en version semver comparable.
///
/// Gère `v1.2.3`, `1.2.3`, `release-1.2.3`, `1.2` (→ `1.2.0`), `1` (→ `1.0.0`),
/// `v1.2.3-rc.1`, `1.2.3+build` et `1.27.4-alpine` (suffixe traité comme
/// pré-version). Rejette `latest`, `main`, `stable`, `edge`, les digests sha256
/// et les dates du type `20240101`.
pub fn normalize_version(tag: &str, prefix: Option<&str>) -> Option<semver::Version> {
    let mut s = tag.trim();
    if s.is_empty() {
        return None;
    }

    // 1. Retrait du préfixe configuré par l'utilisateur.
    if let Some(p) = prefix.map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(r) = s.strip_prefix(p) {
            s = r;
        }
    }
    s = s.trim().trim_start_matches(['-', '_', '/']);
    if s.is_empty() {
        return None;
    }

    let lower = s.to_ascii_lowercase();

    // 2. Rejets francs.
    if lower.starts_with("sha256:") || lower.starts_with("sha512:") {
        return None;
    }
    if is_hex_digest(&lower) {
        return None;
    }
    if NOT_VERSIONS.contains(&lower.as_str()) {
        return None;
    }
    if is_datelike(&lower) {
        return None;
    }

    // 3. Retrait d'un préfixe alphabétique connu (`v1.2.3`, `release-1.2.3`).
    let core_str = strip_known_prefix(s)?;
    if core_str.is_empty() {
        return None;
    }

    // 4. Découpage `coeur[-préversion][+métadonnées]`.
    let (without_build, build) = match core_str.split_once('+') {
        Some((a, b)) => (a, Some(b)),
        None => (core_str, None),
    };
    let (numeric, pre) = match without_build.split_once('-') {
        Some((a, b)) => (a, Some(b)),
        None => (without_build, None),
    };
    if numeric.is_empty() {
        return None;
    }
    if is_datelike(numeric) {
        return None;
    }

    // 5. Le cœur doit être 1 à 3 composantes numériques.
    let parts: Vec<&str> = numeric.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut nums: Vec<u64> = Vec::with_capacity(3);
    for p in &parts {
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        // Une composante unique très longue est une date ou un horodatage.
        if parts.len() == 1 && p.len() >= 5 {
            return None;
        }
        nums.push(p.parse::<u64>().ok()?);
    }
    while nums.len() < 3 {
        nums.push(0);
    }

    // 6. Reconstruction d'une chaîne semver stricte, puis analyse.
    let mut candidate = format!("{}.{}.{}", nums[0], nums[1], nums[2]);
    if let Some(pre) = pre.map(str::trim).filter(|p| !p.is_empty()) {
        candidate.push('-');
        candidate.push_str(pre);
    }
    if let Some(build) = build.map(str::trim).filter(|b| !b.is_empty()) {
        candidate.push('+');
        candidate.push_str(build);
    }

    if let Ok(v) = semver::Version::parse(&candidate) {
        return Some(v);
    }

    // Dernier recours : pré-version ou métadonnées non conformes (caractères
    // interdits, zéros de tête). On assainit plutôt que d'abandonner la version.
    let sanitized_pre = pre.map(sanitize_identifier).filter(|p| !p.is_empty());
    let mut candidate = format!("{}.{}.{}", nums[0], nums[1], nums[2]);
    if let Some(p) = &sanitized_pre {
        candidate.push('-');
        candidate.push_str(p);
    }
    semver::Version::parse(&candidate).ok()
}

/// Retire un préfixe alphabétique reconnu ; renvoie `None` si le préfixe est
/// inconnu (ex. `alpine3.18` n'est pas la version 3.18 de quoi que ce soit).
fn strip_known_prefix(s: &str) -> Option<&str> {
    let first_digit = s.find(|c: char| c.is_ascii_digit())?;
    if first_digit == 0 {
        return Some(s);
    }
    let head = &s[..first_digit];
    let key: String = head
        .trim_end_matches(['-', '_', '/', '.', ' '])
        .to_ascii_lowercase();
    if key.is_empty() {
        // Le tag commençait par un séparateur : on l'ignore.
        return Some(&s[first_digit..]);
    }
    if KNOWN_PREFIXES.contains(&key.as_str()) {
        Some(&s[first_digit..])
    } else {
        None
    }
}

/// Assainit un identifiant de pré-version pour le rendre conforme à semver.
fn sanitize_identifier(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split('.') {
        let cleaned: String = part
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let cleaned = cleaned.trim_matches('-').to_string();
        if cleaned.is_empty() {
            continue;
        }
        // Un identifiant numérique ne doit pas avoir de zéro de tête.
        let fixed = if cleaned.len() > 1
            && cleaned.starts_with('0')
            && cleaned.chars().all(|c| c.is_ascii_digit())
        {
            cleaned.trim_start_matches('0').to_string()
        } else {
            cleaned
        };
        if fixed.is_empty() {
            out.push("0".to_string());
        } else {
            out.push(fixed);
        }
    }
    out.join(".")
}

/// Détecte un digest hexadécimal (empreinte d'image ou SHA de commit).
fn is_hex_digest(s: &str) -> bool {
    let len = s.len();
    (len == 7 || len == 8 || len == 12 || len == 32 || len == 40 || len == 64)
        && s.chars().all(|c| c.is_ascii_hexdigit())
        && s.chars().any(|c| c.is_ascii_alphabetic())
}

/// Détecte une date compacte (`20240101`) ou étendue (`2024-01-01`).
fn is_datelike(s: &str) -> bool {
    let digits: Vec<char> = s.chars().collect();
    if digits.len() == 8 && digits.iter().all(|c| c.is_ascii_digit()) {
        return true;
    }
    if s.len() == 10 {
        let b = s.as_bytes();
        let sep = b[4];
        if (sep == b'-' || sep == b'.' || sep == b'_') && b[7] == sep {
            return s.chars().enumerate().all(|(i, c)| {
                if i == 4 || i == 7 {
                    true
                } else {
                    c.is_ascii_digit()
                }
            });
        }
    }
    false
}

// ---------------------------------------------------------------------------
// DTO GitHub (structure du JSON amont, jamais exposée à l'API HTTP)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<DateTime<Utc>>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

impl GhRelease {
    fn into_model(self) -> ReleaseInfo {
        let semver = normalize_version(&self.tag_name, None).map(|v| v.to_string());
        ReleaseInfo {
            tag: self.tag_name,
            name: self.name,
            body: self.body,
            published_at: self.published_at,
            prerelease: self.prerelease,
            draft: self.draft,
            html_url: self.html_url,
            semver,
            assets: self.assets.into_iter().map(GhAsset::into_model).collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    #[serde(default)]
    size: i64,
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    content_type: Option<String>,
}

impl GhAsset {
    fn into_model(self) -> ReleaseAsset {
        ReleaseAsset {
            name: self.name,
            size: self.size,
            download_url: self.browser_download_url,
            content_type: self.content_type,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhTag {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhRateLimit {
    #[serde(default)]
    resources: GhRateResources,
    #[serde(default)]
    rate: Option<GhRateCore>,
}

#[derive(Debug, Default, Deserialize)]
struct GhRateResources {
    #[serde(default)]
    core: Option<GhRateCore>,
}

#[derive(Debug, Deserialize)]
struct GhRateCore {
    #[serde(default)]
    remaining: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).expect("version de test valide")
    }

    // --- normalize_version : cas acceptés -----------------------------------

    #[test]
    fn nv_v_prefixe() {
        assert_eq!(normalize_version("v1.2.3", None), Some(v("1.2.3")));
    }

    #[test]
    fn nv_v_majuscule() {
        assert_eq!(normalize_version("V3.4.5", None), Some(v("3.4.5")));
    }

    #[test]
    fn nv_sans_prefixe() {
        assert_eq!(normalize_version("1.2.3", None), Some(v("1.2.3")));
    }

    #[test]
    fn nv_prefixe_release() {
        assert_eq!(normalize_version("release-1.2.3", None), Some(v("1.2.3")));
        assert_eq!(normalize_version("rel_2.0.0", None), Some(v("2.0.0")));
        assert_eq!(normalize_version("version/4.5.6", None), Some(v("4.5.6")));
    }

    #[test]
    fn nv_deux_composantes() {
        assert_eq!(normalize_version("1.2", None), Some(v("1.2.0")));
    }

    #[test]
    fn nv_une_composante() {
        assert_eq!(normalize_version("1", None), Some(v("1.0.0")));
        assert_eq!(normalize_version("v16", None), Some(v("16.0.0")));
    }

    #[test]
    fn nv_prerelease_pointee() {
        let r = normalize_version("v1.2.3-rc.1", None).expect("rc");
        assert_eq!(r, v("1.2.3-rc.1"));
        assert!(!r.pre.is_empty());
    }

    #[test]
    fn nv_metadonnees_build() {
        let r = normalize_version("1.2.3+build", None).expect("build");
        assert_eq!(r.major, 1);
        assert_eq!(r.build.as_str(), "build");
    }

    #[test]
    fn nv_suffixe_distribution_devient_prerelease() {
        let r = normalize_version("1.27.4-alpine", None).expect("alpine");
        assert_eq!((r.major, r.minor, r.patch), (1, 27, 4));
        assert_eq!(r.pre.as_str(), "alpine");
        // La pré-version est bien classée avant la version stable.
        assert!(r < v("1.27.4"));
    }

    #[test]
    fn nv_deux_composantes_avec_suffixe() {
        assert_eq!(
            normalize_version("1.2-alpine", None),
            Some(v("1.2.0-alpine"))
        );
    }

    #[test]
    fn nv_espaces_ignores() {
        assert_eq!(normalize_version("  v2.0.0  ", None), Some(v("2.0.0")));
    }

    #[test]
    fn nv_zero() {
        assert_eq!(normalize_version("v0.0.0", None), Some(v("0.0.0")));
    }

    #[test]
    fn nv_prefixe_utilisateur() {
        assert_eq!(
            normalize_version("app-v1.2.3", Some("app-")),
            Some(v("1.2.3"))
        );
        assert_eq!(
            normalize_version("postgres-16", Some("postgres-")),
            Some(v("16.0.0"))
        );
        assert_eq!(
            normalize_version("kubewatch/v1.0.0", Some("kubewatch/")),
            Some(v("1.0.0"))
        );
    }

    #[test]
    fn nv_prefixe_utilisateur_absent_reste_analysable() {
        assert_eq!(normalize_version("v1.2.3", Some("app-")), Some(v("1.2.3")));
    }

    #[test]
    fn nv_zeros_de_tete_dans_la_prerelease() {
        // "01" n'est pas un identifiant semver valide : on l'assainit en "1".
        let r = normalize_version("1.2.3-01", None).expect("prerelease numérique");
        assert_eq!((r.major, r.minor, r.patch), (1, 2, 3));
        assert!(!r.pre.is_empty());
    }

    // --- normalize_version : cas rejetés ------------------------------------

    #[test]
    fn nv_rejette_latest() {
        assert_eq!(normalize_version("latest", None), None);
    }

    #[test]
    fn nv_rejette_branches() {
        assert_eq!(normalize_version("main", None), None);
        assert_eq!(normalize_version("master", None), None);
        assert_eq!(normalize_version("stable", None), None);
        assert_eq!(normalize_version("edge", None), None);
        assert_eq!(normalize_version("nightly", None), None);
    }

    #[test]
    fn nv_rejette_digest() {
        assert_eq!(
            normalize_version(
                "sha256:9f5b0c0c0d1e2f3a4b5c6d7e8f90112233445566778899aabbccddeeff001122",
                None
            ),
            None
        );
        assert_eq!(
            normalize_version(
                "9f5b0c0c0d1e2f3a4b5c6d7e8f90112233445566778899aabbccddeeff001122",
                None
            ),
            None
        );
        assert_eq!(normalize_version("a1b2c3d", None), None);
    }

    #[test]
    fn nv_rejette_dates() {
        assert_eq!(normalize_version("20240101", None), None);
        assert_eq!(normalize_version("2024-01-01", None), None);
        assert_eq!(normalize_version("v20240101", None), None);
    }

    #[test]
    fn nv_rejette_chaine_vide() {
        assert_eq!(normalize_version("", None), None);
        assert_eq!(normalize_version("   ", None), None);
        assert_eq!(normalize_version("v", None), None);
    }

    #[test]
    fn nv_rejette_quatre_composantes() {
        assert_eq!(normalize_version("1.2.3.4", None), None);
    }

    #[test]
    fn nv_rejette_prefixe_inconnu() {
        // "alpine3.18" désigne une distribution, pas la version 3.18 du produit.
        assert_eq!(normalize_version("alpine3.18", None), None);
        assert_eq!(normalize_version("debian11", None), None);
    }

    #[test]
    fn nv_rejette_composantes_non_numeriques() {
        assert_eq!(normalize_version("1.x.3", None), None);
        assert_eq!(normalize_version("1..3", None), None);
    }

    #[test]
    fn nv_ordre_total_coherent() {
        let mut versions: Vec<semver::Version> =
            ["v1.0.0", "1.10.0", "1.2.0", "2.0.0-rc.1", "2.0.0"]
                .iter()
                .filter_map(|t| normalize_version(t, None))
                .collect();
        versions.sort();
        let rendus: Vec<String> = versions.iter().map(|v| v.to_string()).collect();
        assert_eq!(
            rendus,
            vec!["1.0.0", "1.2.0", "1.10.0", "2.0.0-rc.1", "2.0.0"]
        );
    }

    // --- parse_repo ---------------------------------------------------------

    #[test]
    fn parse_repo_formes_acceptees() {
        let attendu = ("kubewatch-io".to_string(), "kubewatch".to_string());
        for entree in [
            "kubewatch-io/kubewatch",
            "  kubewatch-io/kubewatch  ",
            "https://github.com/kubewatch-io/kubewatch",
            "https://github.com/kubewatch-io/kubewatch.git",
            "https://www.github.com/kubewatch-io/kubewatch/",
            "github.com/kubewatch-io/kubewatch",
            "git@github.com:kubewatch-io/kubewatch.git",
            "https://github.com/kubewatch-io/kubewatch/releases/tag/v1.0.0",
            "https://api.github.com/repos/kubewatch-io/kubewatch",
            "https://github.com/kubewatch-io/kubewatch?tab=readme",
        ] {
            assert_eq!(
                parse_repo(entree).expect(entree),
                attendu,
                "entrée: {entree}"
            );
        }
    }

    #[test]
    fn parse_repo_rejets() {
        assert!(parse_repo("").is_err());
        assert!(parse_repo("kubewatch").is_err());
        assert!(parse_repo("owner/").is_err());
        assert!(parse_repo("own er/repo").is_err());
    }

    // --- sélection de release ----------------------------------------------

    #[test]
    fn best_release_prefere_la_plus_haute_stable() {
        let mk = |tag: &str, pre: bool, draft: bool| {
            let mut r = ReleaseInfo::from_tag(tag);
            r.prerelease = pre;
            r.draft = draft;
            r.semver = normalize_version(tag, None).map(|v| v.to_string());
            r
        };
        let releases = vec![
            mk("v2.0.0-rc.1", true, false),
            mk("v1.9.0", false, false),
            mk("v1.10.0", false, false),
            mk("v3.0.0", false, true),
        ];
        let best = pick_best_release(&releases).expect("une release");
        assert_eq!(best.tag, "v1.10.0");
    }

    #[test]
    fn best_release_se_rabat_sur_les_preversions() {
        let mut pre = ReleaseInfo::from_tag("v2.0.0-rc.1");
        pre.prerelease = true;
        pre.semver = Some("2.0.0-rc.1".into());
        let best = pick_best_release(&[pre]).expect("une pré-version");
        assert_eq!(best.tag, "v2.0.0-rc.1");
    }

    #[test]
    fn best_release_vide() {
        assert!(pick_best_release(&[]).is_none());
        let mut draft = ReleaseInfo::from_tag("v1.0.0");
        draft.draft = true;
        assert!(pick_best_release(&[draft]).is_none());
    }

    #[test]
    fn best_tag_choisit_le_maximum_semver() {
        let tags = vec![
            "v1.2.0".to_string(),
            "latest".to_string(),
            "v1.10.0".to_string(),
            "v1.9.0".to_string(),
        ];
        let best = pick_best_tag(&tags).expect("un tag");
        assert_eq!(best.tag, "v1.10.0");
        assert_eq!(best.semver.as_deref(), Some("1.10.0"));
    }

    #[test]
    fn best_tag_sans_semver_renvoie_le_premier() {
        let tags = vec!["latest".to_string(), "main".to_string()];
        assert_eq!(pick_best_tag(&tags).expect("repli").tag, "latest");
        assert!(pick_best_tag(&[]).is_none());
    }

    // --- divers -------------------------------------------------------------

    #[test]
    fn client_ignore_un_jeton_vide() {
        let c = GithubClient::new(Some("   ".to_string())).expect("client");
        assert!(!c.has_token());
        let c = GithubClient::new(Some("ghp_x".to_string())).expect("client");
        assert!(c.has_token());
        let c = GithubClient::new(None).expect("client");
        assert!(!c.has_token());
    }

    #[test]
    fn user_agent_porte_la_version() {
        let ua = user_agent();
        assert!(ua.starts_with("kubewatch/"), "agent inattendu: {ua}");
    }

    #[test]
    fn sanitize_repo_refuse_les_traversees() {
        assert!(sanitize_repo("owner", "../evil").is_err());
        assert!(sanitize_repo("owner", "repo/sub").is_err());
        assert!(sanitize_repo("", "repo").is_err());
    }

    #[test]
    fn troncature_des_messages() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate(&"x".repeat(20), 5), "xxxxx…");
    }

    #[test]
    fn release_github_convertie() {
        let json = serde_json::json!({
            "tag_name": "v1.4.2",
            "name": "1.4.2",
            "body": "notes",
            "published_at": "2026-01-15T10:00:00Z",
            "prerelease": false,
            "draft": false,
            "html_url": "https://github.com/o/r/releases/tag/v1.4.2",
            "assets": [{
                "name": "kubewatch-linux",
                "size": 1234,
                "browser_download_url": "https://example.invalid/bin",
                "content_type": "application/octet-stream"
            }]
        });
        let gh: GhRelease = serde_json::from_value(json).expect("désérialisation");
        let r = gh.into_model();
        assert_eq!(r.tag, "v1.4.2");
        assert_eq!(r.semver.as_deref(), Some("1.4.2"));
        assert_eq!(r.assets.len(), 1);
        assert_eq!(r.assets[0].download_url, "https://example.invalid/bin");
        assert!(r.published_at.is_some());
    }

    #[test]
    fn release_github_champs_absents() {
        let json = serde_json::json!({ "tag_name": "nightly" });
        let gh: GhRelease = serde_json::from_value(json).expect("désérialisation minimale");
        let r = gh.into_model();
        assert_eq!(r.tag, "nightly");
        assert!(r.semver.is_none());
        assert!(r.assets.is_empty());
    }

    #[test]
    fn quota_deserialise() {
        let json = serde_json::json!({
            "resources": { "core": { "remaining": 4987 } },
            "rate": { "remaining": 4987 }
        });
        let rl: GhRateLimit = serde_json::from_value(json).expect("quota");
        assert_eq!(rl.resources.core.map(|c| c.remaining), Some(4987));
    }
}
