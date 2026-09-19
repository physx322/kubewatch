//! Moteur de détection et d'application des mises à jour.
//!
//! Le moteur relie trois briques: le registre des clusters (`kubewatch-core`), le
//! hub d'images et de charts (`kubewatch-hub`) et le magasin d'état local
//! (surveillants, détections, historique, réglages).

use crate::error::{Error, Result};
use crate::github::{self, GithubClient};
use crate::model::{
    ReleaseInfo, RolloutResult, UpdateFinding, UpdateSeverity, UpdateSource, UpdaterStats,
    WatcherSpec,
};
use crate::policy;
use crate::rollout;
use crate::scan;
use crate::store::Store;
use crate::webhook::WebhookEvent;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use kubewatch_core::ClusterManager;
use kubewatch_hub::model::ImageRef;
use kubewatch_hub::HubClient;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

/// Nombre de surveillants vérifiés simultanément.
const CHECK_CONCURRENCY: usize = 6;

/// Intervalle plancher du planificateur, pour ne pas marteler les registres.
const MIN_INTERVAL_SECONDS: u64 = 60;

/// Intervalle utilisé quand aucun surveillant actif n'impose le sien.
const DEFAULT_INTERVAL_SECONDS: u64 = 300;

/// Nombre maximal de tags remontés d'un registre lors d'une vérification.
const TAG_PAGE: usize = 200;

/// Nombre maximal de releases GitHub examinées par vérification.
const RELEASE_PAGE: usize = 100;

/// Dépôt GitHub de KubeWatch lui-même, pour l'auto-mise à jour.
const SELF_OWNER: &str = "kubewatch-io";
const SELF_REPO: &str = "kubewatch";

/// Une version candidate, avec le tag d'origine et, le cas échéant, sa release.
struct Candidate {
    version: semver::Version,
    release: Option<ReleaseInfo>,
}

struct Inner {
    clusters: ClusterManager,
    hub: HubClient,
    store: Store,
    /// Client GitHub mis en cache, réémis dès que le jeton change dans les réglages.
    github: parking_lot::Mutex<Option<(Option<String>, GithubClient)>>,
    /// Horodatage de la dernière campagne de vérification complète.
    last_run: parking_lot::RwLock<Option<DateTime<Utc>>>,
}

/// Moteur de mise à jour, partagé par clonage (état interne en `Arc`).
#[derive(Clone)]
pub struct UpdateEngine {
    inner: Arc<Inner>,
}

impl UpdateEngine {
    /// Construit le moteur à partir du registre de clusters, du hub et du magasin.
    pub fn new(clusters: ClusterManager, hub: HubClient, store: Store) -> Self {
        Self {
            inner: Arc::new(Inner {
                clusters,
                hub,
                store,
                github: parking_lot::Mutex::new(None),
                last_run: parking_lot::RwLock::new(None),
            }),
        }
    }

    /// Magasin d'état (surveillants, détections, historique, réglages).
    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    /// Client GitHub courant, reconstruit uniquement si le jeton a changé.
    fn github_client(&self) -> Result<GithubClient> {
        let token = self.inner.store.settings().github_token;
        let mut guard = self.inner.github.lock();
        if let Some((cached, client)) = guard.as_ref() {
            if *cached == token {
                return Ok(client.clone());
            }
        }
        let client = GithubClient::new(token.clone())?;
        *guard = Some((token, client.clone()));
        Ok(client)
    }

    /// Client hub enrichi du jeton GitHub courant (utile pour ghcr.io).
    fn hub_client(&self) -> HubClient {
        self.inner
            .hub
            .clone()
            .with_github_token(self.inner.store.settings().github_token)
    }

    /// Vérifie un surveillant et renvoie la mise à jour disponible, s'il y en a une.
    ///
    /// Un cluster inconnu n'est pas une erreur fatale: le surveillant est ignoré et
    /// l'incident journalisé, pour qu'un cluster déconnecté ne bloque pas les autres.
    pub async fn check_watcher(&self, w: &WatcherSpec) -> Result<Option<UpdateFinding>> {
        let handle = match self.inner.clusters.get(&w.cluster) {
            Ok(h) => h,
            Err(err) => {
                tracing::warn!(
                    surveillant = %w.name, cluster = %w.cluster, erreur = %err,
                    "cluster introuvable: surveillant ignoré"
                );
                return Ok(None);
            }
        };

        // 1. Lire l'objet ciblé et en extraire l'image du conteneur surveillé.
        let r = rollout::resource_ref(
            &handle,
            &w.target.kind,
            w.target.namespace.as_deref(),
            &w.target.name,
        )?;
        let raw = kubewatch_core::resource::get_raw(&handle, &r)
            .await
            .map_err(|e| Error::Core(format!("lecture de {}/{}: {e}", r.kind, r.name)))?;

        let current_image =
            rollout::container_image(&raw, w.container.as_deref()).ok_or_else(|| {
                Error::NotFound(format!(
                    "conteneur « {} » introuvable dans {}/{}",
                    w.container.as_deref().unwrap_or("<premier>"),
                    r.kind,
                    r.name
                ))
            })?;

        // 2. Analyser l'image pour en tirer le tag courant.
        let image_ref = ImageRef::parse(&current_image)
            .map_err(|e| Error::Hub(format!("image « {current_image} » illisible: {e}")))?;
        let current_tag = image_ref.tag.clone();

        let tag_prefix = match &w.source {
            UpdateSource::GithubRelease { tag_prefix, .. } => tag_prefix.clone(),
            _ => None,
        };
        let current_version = current_tag
            .as_deref()
            .and_then(|t| github::normalize_version(t, tag_prefix.as_deref()));

        // 3. Rassembler les versions candidates auprès de la source configurée.
        let candidates = self.candidates_for(w, &current_image).await?;

        // 4. Laisser la politique trancher.
        let mut versions: Vec<semver::Version> =
            candidates.iter().map(|c| c.version.clone()).collect();
        versions.sort();
        versions.dedup();
        let selected = policy::select_update(current_version.as_ref(), &versions, &w.policy);

        let now = Utc::now();
        let mut observed_digest: Option<String> = None;

        let finding = match selected {
            Some(next) => Some(
                self.build_finding(
                    w,
                    &r,
                    &image_ref,
                    &current_image,
                    current_tag.as_deref(),
                    current_version.as_ref(),
                    &candidates,
                    next,
                    now,
                )
                .await,
            ),
            None => {
                // Pas de version exploitable dans le tag (`latest`, `stable`…): on
                // suit alors le digest, qui bouge lui aussi.
                if current_version.is_none() {
                    if let UpdateSource::ContainerRegistry { .. } = &w.source {
                        let (f, digest) = self
                            .digest_finding(w, &r, &image_ref, &current_image, now)
                            .await;
                        observed_digest = digest;
                        f
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        };

        // 5. Mémoriser le passage et l'état observé.
        let observed = current_version
            .as_ref()
            .map(|v| v.to_string())
            .or(observed_digest)
            .or_else(|| current_tag.clone())
            .unwrap_or_else(|| current_image.clone());

        let mut updated = w.clone();
        updated.last_checked_at = Some(now);
        updated.last_known_version = Some(observed);
        if let Err(err) = self.inner.store.upsert_watcher(updated) {
            tracing::warn!(surveillant = %w.name, erreur = %err,
                "état du surveillant non enregistré");
        }

        Ok(finding)
    }

    /// Interroge la source du surveillant et renvoie les versions candidates.
    async fn candidates_for(&self, w: &WatcherSpec, current_image: &str) -> Result<Vec<Candidate>> {
        let ignored: HashSet<&str> = w.policy.ignore.iter().map(String::as_str).collect();
        let mut out: Vec<Candidate> = Vec::new();

        match &w.source {
            UpdateSource::GithubRelease {
                owner,
                repo,
                tag_prefix,
            } => {
                let gh = self.github_client()?;
                for release in gh.list_releases(owner, repo, RELEASE_PAGE).await? {
                    if release.draft {
                        continue;
                    }
                    if release.prerelease && !w.policy.allow_prerelease {
                        continue;
                    }
                    if ignored.contains(release.tag.as_str()) {
                        continue;
                    }
                    if let Some(version) =
                        github::normalize_version(&release.tag, tag_prefix.as_deref())
                    {
                        out.push(Candidate {
                            version,
                            release: Some(release),
                        });
                    }
                }
            }
            UpdateSource::ContainerRegistry { image } => {
                // Le champ `image` prime, avec repli sur l'image réellement déployée.
                let reference = if image.trim().is_empty() {
                    current_image
                } else {
                    image.as_str()
                };
                let image_ref = ImageRef::parse(reference)
                    .map_err(|e| Error::Hub(format!("image « {reference} » illisible: {e}")))?;
                let hub = self.hub_client();
                let tags = hub
                    .list_tags(&image_ref, TAG_PAGE)
                    .await
                    .map_err(|e| Error::Hub(format!("tags de « {reference} »: {e}")))?;
                for tag in tags {
                    if ignored.contains(tag.name.as_str()) {
                        continue;
                    }
                    if let Some(version) = github::normalize_version(&tag.name, None) {
                        out.push(Candidate {
                            version,
                            release: None,
                        });
                    }
                }
            }
            UpdateSource::HelmChart { repo, chart } => {
                let hub = self.hub_client();
                let charts = hub
                    .chart_versions(repo, chart)
                    .await
                    .map_err(|e| Error::Hub(format!("versions du chart {repo}/{chart}: {e}")))?;
                for c in charts {
                    if ignored.contains(c.version.as_str()) {
                        continue;
                    }
                    if let Some(version) = github::normalize_version(&c.version, None) {
                        out.push(Candidate {
                            version,
                            release: None,
                        });
                    }
                }
            }
        }

        Ok(out)
    }

    /// Construit la détection pour une version retenue.
    #[allow(clippy::too_many_arguments)]
    async fn build_finding(
        &self,
        w: &WatcherSpec,
        r: &kubewatch_core::model::ResourceRef,
        image_ref: &ImageRef,
        current_image: &str,
        current_tag: Option<&str>,
        current_version: Option<&semver::Version>,
        candidates: &[Candidate],
        next: semver::Version,
        now: DateTime<Utc>,
    ) -> UpdateFinding {
        let release = candidates
            .iter()
            .find(|c| c.version == next && c.release.is_some())
            .or_else(|| candidates.iter().find(|c| c.version == next))
            .and_then(|c| c.release.clone());

        let hub = self.hub_client();
        let available_image =
            build_target_image(&hub, image_ref, current_image, current_tag, &next).await;

        let available_version = next.to_string();

        UpdateFinding {
            // Identifiant déterministe: deux passages successifs réconcilient la
            // même détection au lieu de l'empiler.
            id: UpdateFinding::deterministic_id(&w.id, &available_version),
            watcher_id: w.id.clone(),
            watcher_name: w.name.clone(),
            cluster: w.cluster.clone(),
            target_kind: r.kind.clone(),
            target_namespace: r.namespace.clone(),
            target_name: r.name.clone(),
            container: w.container.clone(),
            current_version: current_version
                .map(|v| v.to_string())
                .or_else(|| current_tag.map(str::to_string)),
            current_image: Some(current_image.to_string()),
            available_version,
            available_image: Some(available_image),
            source: w.source.clone(),
            severity: policy::severity(current_version, &next),
            release,
            detected_at: now,
            applied: false,
        }
    }

    /// Suivi par digest des tags mobiles (`latest`, `stable`…).
    ///
    /// Renvoie la détection éventuelle et le digest observé, à mémoriser comme
    /// nouvelle référence même lorsqu'il n'y a rien à signaler.
    async fn digest_finding(
        &self,
        w: &WatcherSpec,
        r: &kubewatch_core::model::ResourceRef,
        image_ref: &ImageRef,
        current_image: &str,
        now: DateTime<Utc>,
    ) -> (Option<UpdateFinding>, Option<String>) {
        let hub = self.hub_client();
        let digest = match hub.resolve_digest(image_ref).await {
            Ok(d) if !d.is_empty() => d,
            Ok(_) => return (None, None),
            Err(err) => {
                tracing::debug!(
                    surveillant = %w.name, image = %current_image, erreur = %err,
                    "digest non résolu: suivi du tag mobile impossible"
                );
                return (None, None);
            }
        };

        // Référence connue: uniquement un digest mémorisé lors d'un passage précédent.
        let baseline = w
            .last_known_version
            .as_deref()
            .filter(|b| b.contains(':') && b.starts_with("sha"));

        let finding = match baseline {
            Some(prev) if prev != digest => {
                // On épingle le digest: le tag seul ne déclencherait aucun redéploiement.
                let base = current_image.split('@').next().unwrap_or(current_image);
                Some(UpdateFinding {
                    id: UpdateFinding::deterministic_id(&w.id, &digest),
                    watcher_id: w.id.clone(),
                    watcher_name: w.name.clone(),
                    cluster: w.cluster.clone(),
                    target_kind: r.kind.clone(),
                    target_namespace: r.namespace.clone(),
                    target_name: r.name.clone(),
                    container: w.container.clone(),
                    current_version: Some(prev.to_string()),
                    current_image: Some(current_image.to_string()),
                    available_version: digest.clone(),
                    available_image: Some(format!("{base}@{digest}")),
                    source: w.source.clone(),
                    severity: UpdateSeverity::Unknown,
                    release: None,
                    detected_at: now,
                    applied: false,
                })
            }
            _ => None,
        };

        (finding, Some(digest))
    }

    /// Vérifie une liste de surveillants en parallèle; un échec isolé est journalisé
    /// et n'interrompt pas les autres.
    async fn check_many(&self, watchers: Vec<WatcherSpec>) -> Vec<UpdateFinding> {
        futures::stream::iter(watchers.into_iter().map(|w| {
            let this = self.clone();
            async move {
                match this.check_watcher(&w).await {
                    Ok(found) => found,
                    Err(err) => {
                        tracing::warn!(
                            surveillant = %w.name, id = %w.id, erreur = %err,
                            "vérification du surveillant en échec"
                        );
                        None
                    }
                }
            }
        }))
        .buffer_unordered(CHECK_CONCURRENCY)
        .collect::<Vec<Option<UpdateFinding>>>()
        .await
        .into_iter()
        .flatten()
        .collect()
    }

    /// Vérifie tous les surveillants actifs et met le magasin à jour.
    pub async fn check_all(&self) -> Result<Vec<UpdateFinding>> {
        let all = self.inner.store.watchers();
        let known_ids: Vec<String> = all.iter().map(|w| w.id.clone()).collect();
        let enabled: Vec<WatcherSpec> = all.into_iter().filter(|w| w.enabled).collect();
        let checked_ids: Vec<String> = enabled.iter().map(|w| w.id.clone()).collect();

        let fresh = self.check_many(enabled).await;
        *self.inner.last_run.write() = Some(Utc::now());
        self.merge_findings(fresh, &checked_ids, &known_ids)
    }

    /// Fusionne les détections fraîches avec celles déjà en magasin.
    ///
    /// Les détections des surveillants qui viennent d'être vérifiés sont remplacées,
    /// celles des surveillants supprimés sont écartées, et une détection identique
    /// déjà connue conserve son identifiant, sa date et son état « appliqué » pour
    /// que l'IHM garde des lignes stables.
    fn merge_findings(
        &self,
        mut fresh: Vec<UpdateFinding>,
        checked_ids: &[String],
        known_ids: &[String],
    ) -> Result<Vec<UpdateFinding>> {
        let existing = self.inner.store.findings();

        for f in fresh.iter_mut() {
            if let Some(prev) = existing.iter().find(|p| {
                p.watcher_id == f.watcher_id && p.available_version == f.available_version
            }) {
                f.id = prev.id.clone();
                f.detected_at = prev.detected_at;
                f.applied = prev.applied;
            }
        }

        let mut merged: Vec<UpdateFinding> = existing
            .into_iter()
            .filter(|f| !checked_ids.iter().any(|id| id == &f.watcher_id))
            .filter(|f| known_ids.iter().any(|id| id == &f.watcher_id))
            .collect();
        merged.extend(fresh.iter().cloned());
        merged.sort_by_key(|a| std::cmp::Reverse(a.detected_at));

        self.inner.store.set_findings(merged)?;
        Ok(fresh)
    }

    /// Applique une détection: remplace l'image, attend la convergence, historise.
    pub async fn apply(&self, finding_id: &str) -> Result<RolloutResult> {
        let finding = self
            .inner
            .store
            .get_finding(finding_id)
            .ok_or_else(|| Error::NotFound(format!("détection « {finding_id} » inconnue")))?;

        if finding.applied {
            return Err(Error::Invalid(format!(
                "la détection « {finding_id} » a déjà été appliquée"
            )));
        }

        let handle = self
            .inner
            .clusters
            .get(&finding.cluster)
            .map_err(|e| Error::Core(format!("cluster « {} »: {e}", finding.cluster)))?;

        let result = rollout::apply_finding(&handle, &finding).await?;

        // Relecture avant écriture: une vérification concurrente a pu modifier la liste.
        let mut latest = self.inner.store.findings();
        if let Some(f) = latest.iter_mut().find(|f| f.id == finding_id) {
            f.applied = true;
        }
        self.inner.store.set_findings(latest)?;
        self.inner.store.push_history(result.clone())?;

        Ok(result)
    }

    /// Applique les détections dont le surveillant autorise l'automatisme et dont
    /// la fenêtre de maintenance est ouverte.
    pub async fn apply_all_eligible(&self) -> Result<Vec<RolloutResult>> {
        let now = Utc::now();
        let watchers = self.inner.store.watchers();

        let eligible: Vec<String> = self
            .inner
            .store
            .findings()
            .into_iter()
            .filter(|f| !f.applied)
            .filter(|f| {
                watchers
                    .iter()
                    .find(|w| w.id == f.watcher_id)
                    .map(|w| {
                        w.enabled
                            && w.policy.auto_apply
                            && policy::in_maintenance_window(
                                w.policy.maintenance_window.as_deref(),
                                now,
                            )
                    })
                    .unwrap_or(false)
            })
            .map(|f| f.id)
            .collect();

        // Séquentiel volontairement: deux déploiements simultanés sur le même
        // cluster rendent les diagnostics illisibles.
        let mut out = Vec::new();
        for id in eligible {
            match self.apply(&id).await {
                Ok(result) => out.push(result),
                Err(err) => tracing::warn!(
                    detection = %id, erreur = %err,
                    "application automatique en échec"
                ),
            }
        }
        Ok(out)
    }

    /// Traite une livraison GitHub déjà authentifiée: ne revérifie que les
    /// surveillants dont la source pointe vers le dépôt concerné.
    pub async fn handle_webhook(&self, ev: &WebhookEvent) -> Result<Vec<UpdateFinding>> {
        let (owner, repo) = match ev {
            WebhookEvent::Release { owner, repo, .. } => (owner.as_str(), repo.as_str()),
            WebhookEvent::Push { owner, repo, .. } => (owner.as_str(), repo.as_str()),
            WebhookEvent::Ping => {
                tracing::info!("webhook GitHub: ping reçu");
                return Ok(Vec::new());
            }
            WebhookEvent::Other(name) => {
                tracing::debug!(evenement = %name, "webhook GitHub sans effet");
                return Ok(Vec::new());
            }
        };

        let all = self.inner.store.watchers();
        let known_ids: Vec<String> = all.iter().map(|w| w.id.clone()).collect();
        let matched: Vec<WatcherSpec> = all
            .into_iter()
            .filter(|w| w.enabled)
            .filter(|w| match &w.source {
                UpdateSource::GithubRelease {
                    owner: o, repo: rp, ..
                } => o.eq_ignore_ascii_case(owner) && rp.eq_ignore_ascii_case(repo),
                _ => false,
            })
            .collect();

        if matched.is_empty() {
            let depot = format!("{owner}/{repo}");
            tracing::debug!(depot = %depot, "webhook GitHub: aucun surveillant concerné");
            return Ok(Vec::new());
        }

        let checked_ids: Vec<String> = matched.iter().map(|w| w.id.clone()).collect();
        let fresh = self.check_many(matched).await;
        self.merge_findings(fresh, &checked_ids, &known_ids)
    }

    /// Intervalle du planificateur: le plus court des surveillants actifs, jamais
    /// sous le plancher de sécurité.
    fn scheduler_interval(&self) -> Duration {
        let seconds = self
            .inner
            .store
            .watchers()
            .iter()
            .filter(|w| w.enabled)
            .map(|w| w.policy.check_interval_seconds)
            .min()
            .unwrap_or(DEFAULT_INTERVAL_SECONDS)
            .max(MIN_INTERVAL_SECONDS);
        Duration::from_secs(seconds)
    }

    /// Lance la boucle de vérification périodique.
    ///
    /// La boucle ne remonte jamais d'erreur: tout échec est journalisé et le tour
    /// suivant repart proprement. Les réglages sont relus à chaque tour, ce qui
    /// permet d'activer ou de couper le planificateur sans redémarrage.
    pub fn spawn_scheduler(&self) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            tracing::info!("planificateur de mises à jour démarré");
            loop {
                tokio::time::sleep(this.scheduler_interval()).await;

                if !this.inner.store.settings().scheduler_enabled {
                    tracing::trace!("planificateur désactivé: tour ignoré");
                    continue;
                }

                match this.check_all().await {
                    Ok(found) if !found.is_empty() => {
                        tracing::info!(detections = found.len(), "vérification périodique terminée")
                    }
                    Ok(_) => tracing::debug!("vérification périodique: aucune mise à jour"),
                    Err(err) => {
                        tracing::warn!(erreur = %err, "vérification périodique en échec")
                    }
                }

                match this.apply_all_eligible().await {
                    Ok(applied) if !applied.is_empty() => tracing::info!(
                        appliquees = applied.len(),
                        "mises à jour appliquées automatiquement"
                    ),
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(erreur = %err, "application automatique en échec")
                    }
                }
            }
        })
    }

    /// Chiffres de synthèse pour le tableau de bord.
    pub async fn stats(&self) -> UpdaterStats {
        let watchers = self.inner.store.watchers();
        let enabled = watchers.iter().filter(|w| w.enabled).count();
        let findings = self.inner.store.findings().len();

        let last_run = (*self.inner.last_run.read())
            .or_else(|| watchers.iter().filter_map(|w| w.last_checked_at).max());

        let github_rate_remaining = match self.github_client() {
            Ok(gh) => gh.rate_limit_remaining().await,
            Err(err) => {
                tracing::debug!(erreur = %err, "quota GitHub indisponible");
                None
            }
        };

        UpdaterStats {
            watchers: watchers.len(),
            enabled,
            findings,
            last_run,
            github_rate_remaining,
        }
    }

    /// Vérifie si une version plus récente de KubeWatch est publiée.
    pub async fn check_self_update(current_version: &str) -> Result<Option<ReleaseInfo>> {
        let gh = GithubClient::new(std::env::var("GITHUB_TOKEN").ok())?;
        let Some(release) = gh.latest_release(SELF_OWNER, SELF_REPO).await? else {
            return Ok(None);
        };
        if release.draft || release.prerelease {
            return Ok(None);
        }

        let candidate = release
            .semver
            .as_deref()
            .and_then(|s| semver::Version::parse(s).ok())
            .or_else(|| github::normalize_version(&release.tag, None));

        let Some(candidate) = candidate else {
            return Ok(None);
        };

        match github::normalize_version(current_version, None) {
            Some(current) if candidate > current => Ok(Some(release)),
            Some(_) => Ok(None),
            // Version locale illisible (build de développement): on signale la release.
            None => Ok(Some(release)),
        }
    }
}

/// Décompose un tag en `(préfixe, variante)` autour de son noyau numérique.
///
/// « v1.2.3-alpine » donne `("v", "-alpine")`, « 1.27 » donne `("", "")`.
/// Un suffixe qui ressemble à une préversion (`-rc.1`, `-beta2`, `-1`) n'est pas
/// une variante: le reconduire tel quel ramènerait systématiquement vers des
/// préversions.
pub(crate) fn split_tag_decoration(tag: &str) -> (String, String) {
    let bytes = tag.as_bytes();
    let mut idx = 0usize;
    let mut prefix = String::new();

    if bytes.len() >= 2 && (bytes[0] == b'v' || bytes[0] == b'V') && bytes[1].is_ascii_digit() {
        prefix.push(bytes[0] as char);
        idx = 1;
    }

    let start = idx;
    while idx < bytes.len() && (bytes[idx].is_ascii_digit() || bytes[idx] == b'.') {
        idx += 1;
    }
    if idx == start {
        // Aucun noyau numérique: rien à préserver (« latest », « stable »…).
        return (String::new(), String::new());
    }
    // Un point final appartient au séparateur, pas au noyau.
    while idx > start && bytes[idx - 1] == b'.' {
        idx -= 1;
    }

    let variant = &tag[idx..];
    if variant.is_empty() || looks_like_prerelease(variant) {
        return (prefix, String::new());
    }
    (prefix, variant.to_string())
}

/// Un suffixe de tag désigne-t-il une préversion plutôt qu'une variante d'image ?
fn looks_like_prerelease(variant: &str) -> bool {
    let body = variant.trim_start_matches(['-', '_', '.']);
    let head: String = body
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_ascii_lowercase();

    if head.is_empty() {
        // « -1 », « -2.3 »: identifiant numérique de préversion au sens semver.
        return true;
    }
    matches!(
        head.as_str(),
        "rc" | "alpha"
            | "beta"
            | "pre"
            | "preview"
            | "dev"
            | "snapshot"
            | "nightly"
            | "canary"
            | "next"
            | "unstable"
    )
}

/// Compose le tag cible en préservant le préfixe « v » et la variante du tag courant.
///
/// Les formes décorées ne sont proposées qu'après vérification de leur existence
/// réelle dans le registre; sinon on retombe sur la forme nue (préfixe conservé),
/// qui est celle que publient la plupart des projets.
pub(crate) fn target_tag_candidates(
    current_tag: Option<&str>,
    next: &semver::Version,
) -> Vec<String> {
    let version = next.to_string();
    let (prefix, variant) = split_tag_decoration(current_tag.unwrap_or_default());

    let mut tags: Vec<String> = Vec::new();
    if !prefix.is_empty() && !variant.is_empty() {
        tags.push(format!("{prefix}{version}{variant}"));
    }
    if !prefix.is_empty() {
        tags.push(format!("{prefix}{version}"));
    }
    if !variant.is_empty() {
        tags.push(format!("{version}{variant}"));
    }
    tags.push(version);
    tags
}

/// Forme nue du tag cible: la version, précédée du préfixe « v » s'il était présent.
fn bare_target_tag(current_tag: Option<&str>, next: &semver::Version) -> String {
    let (prefix, _) = split_tag_decoration(current_tag.unwrap_or_default());
    format!("{prefix}{next}")
}

/// Construit la référence d'image cible complète.
async fn build_target_image(
    hub: &HubClient,
    image_ref: &ImageRef,
    current_image: &str,
    current_tag: Option<&str>,
    next: &semver::Version,
) -> String {
    let tags = target_tag_candidates(current_tag, next);

    // Un seul candidat: rien à vérifier, on évite un appel réseau inutile.
    let chosen = if tags.len() <= 1 {
        tags.first().cloned().unwrap_or_else(|| next.to_string())
    } else {
        match hub.list_tags(image_ref, TAG_PAGE).await {
            Ok(list) => {
                let known: HashSet<String> = list.into_iter().map(|t| t.name).collect();
                tags.iter()
                    .find(|t| known.contains(*t))
                    .cloned()
                    .unwrap_or_else(|| bare_target_tag(current_tag, next))
            }
            Err(err) => {
                tracing::debug!(
                    image = %current_image, erreur = %err,
                    "tags du registre indisponibles: repli sur la forme nue"
                );
                bare_target_tag(current_tag, next)
            }
        }
    };

    scan::replace_tag(current_image, &chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).expect("version de test valide")
    }

    #[test]
    fn decoration_prefixe_et_variante() {
        assert_eq!(
            split_tag_decoration("v1.2.3-alpine"),
            ("v".to_string(), "-alpine".to_string())
        );
        assert_eq!(
            split_tag_decoration("1.27-alpine3.20"),
            (String::new(), "-alpine3.20".to_string())
        );
        assert_eq!(
            split_tag_decoration("v3.1.0"),
            ("v".to_string(), String::new())
        );
        assert_eq!(split_tag_decoration("1.27"), (String::new(), String::new()));
        assert_eq!(
            split_tag_decoration("16.4-bookworm"),
            (String::new(), "-bookworm".to_string())
        );
    }

    #[test]
    fn decoration_ignore_les_preversions() {
        // « -rc.1 » n'est pas une variante d'image: on ne la reconduit pas.
        assert_eq!(
            split_tag_decoration("v2.0.0-rc.1"),
            ("v".to_string(), String::new())
        );
        assert_eq!(
            split_tag_decoration("1.0.0-beta3"),
            (String::new(), String::new())
        );
        assert_eq!(
            split_tag_decoration("2.4.0-1"),
            (String::new(), String::new())
        );
    }

    #[test]
    fn decoration_tag_mobile() {
        assert_eq!(
            split_tag_decoration("latest"),
            (String::new(), String::new())
        );
        assert_eq!(
            split_tag_decoration("stable"),
            (String::new(), String::new())
        );
        assert_eq!(split_tag_decoration(""), (String::new(), String::new()));
    }

    #[test]
    fn candidats_de_tag_ordonnes() {
        assert_eq!(
            target_tag_candidates(Some("v1.2.3-alpine"), &v("1.3.0")),
            vec!["v1.3.0-alpine", "v1.3.0", "1.3.0-alpine", "1.3.0"]
        );
        assert_eq!(
            target_tag_candidates(Some("v1.2.3"), &v("1.3.0")),
            vec!["v1.3.0", "1.3.0"]
        );
        assert_eq!(
            target_tag_candidates(Some("1.2.3-alpine"), &v("1.3.0")),
            vec!["1.3.0-alpine", "1.3.0"]
        );
        assert_eq!(
            target_tag_candidates(Some("1.2.3"), &v("1.3.0")),
            vec!["1.3.0"]
        );
        assert_eq!(target_tag_candidates(None, &v("1.3.0")), vec!["1.3.0"]);
    }

    #[test]
    fn forme_nue_conserve_le_prefixe() {
        assert_eq!(
            bare_target_tag(Some("v1.2.3-alpine"), &v("1.3.0")),
            "v1.3.0"
        );
        assert_eq!(bare_target_tag(Some("1.2.3-alpine"), &v("1.3.0")), "1.3.0");
        assert_eq!(bare_target_tag(None, &v("2.0.0")), "2.0.0");
    }

    #[test]
    fn remplacement_de_tag_dans_l_image() {
        assert_eq!(
            scan::replace_tag("ghcr.io/acme/app:v1.2.3-alpine", "v1.3.0-alpine"),
            "ghcr.io/acme/app:v1.3.0-alpine"
        );
        assert_eq!(scan::replace_tag("nginx:1.27", "1.28"), "nginx:1.28");
        assert_eq!(scan::replace_tag("nginx", "1.28"), "nginx:1.28");
        assert_eq!(
            scan::replace_tag("registry.local:5000/team/app:1.0.0", "1.1.0"),
            "registry.local:5000/team/app:1.1.0"
        );
        assert_eq!(
            scan::replace_tag(
                "acme/app:1.0.0@sha256:0000000000000000000000000000000000000000000000000000000000000000",
                "1.1.0"
            ),
            "acme/app:1.1.0"
        );
    }

    #[test]
    fn preversion_detectee() {
        assert!(looks_like_prerelease("-rc.1"));
        assert!(looks_like_prerelease("-beta"));
        assert!(looks_like_prerelease("-1"));
        assert!(!looks_like_prerelease("-alpine"));
        assert!(!looks_like_prerelease("-bookworm"));
        assert!(!looks_like_prerelease("-debian-12-r4"));
    }
}
