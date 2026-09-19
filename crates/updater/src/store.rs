//! État persistant du module de mise à jour.
//!
//! Un unique fichier JSON contient les watchers, les constats, l'historique des
//! déploiements et les réglages. Il est chargé en mémoire derrière un
//! `RwLock`, et chaque mutation le réécrit **atomiquement** (écriture dans
//! `<chemin>.tmp` puis `rename`). Comme il contient un jeton GitHub et un secret
//! de webhook, il est créé en `0600` sur Unix et jamais renvoyé en clair à
//! l'API HTTP (voir [`Store::settings_redacted`]).

use crate::error::{Error, Result};
use crate::model::{RolloutResult, UpdateFinding, UpdatePolicy, WatcherSpec};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Nombre maximum d'entrées conservées dans l'historique.
const HISTORY_MAX: usize = 500;

/// Masque substitué aux secrets renvoyés à l'interface.
pub const SECRET_MASK: &str = "••••••";

/// Réglages globaux du module de mise à jour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdaterSettings {
    /// Jeton GitHub (augmente le quota d'API et donne accès aux dépôts privés).
    #[serde(default)]
    pub github_token: Option<String>,
    /// Secret partagé pour vérifier la signature des webhooks GitHub.
    #[serde(default)]
    pub webhook_secret: Option<String>,
    /// Politique appliquée aux nouveaux watchers.
    #[serde(default)]
    pub default_policy: UpdatePolicy,
    /// Active la vérification périodique en tâche de fond.
    #[serde(default = "default_scheduler_enabled")]
    pub scheduler_enabled: bool,
}

fn default_scheduler_enabled() -> bool {
    true
}

impl Default for UpdaterSettings {
    fn default() -> Self {
        Self {
            github_token: None,
            webhook_secret: None,
            default_policy: UpdatePolicy::default(),
            scheduler_enabled: true,
        }
    }
}

/// Contenu sérialisé du fichier d'état.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    #[serde(default)]
    watchers: Vec<WatcherSpec>,
    #[serde(default)]
    findings: Vec<UpdateFinding>,
    /// Historique, du plus ancien au plus récent.
    #[serde(default)]
    history: Vec<RolloutResult>,
    #[serde(default)]
    settings: UpdaterSettings,
}

#[derive(Debug)]
struct Inner {
    path: PathBuf,
    state: RwLock<State>,
}

/// Magasin d'état partagé, clonable et sûr entre tâches.
#[derive(Debug, Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

impl Store {
    /// Crée un magasin adossé à `path`. Aucune E/S n'est effectuée ici :
    /// appeler [`Store::load`] pour lire l'état existant.
    pub fn new(path: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                path,
                state: RwLock::new(State::default()),
            }),
        }
    }

    /// Chemin du fichier d'état.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Charge l'état depuis le disque.
    ///
    /// Un fichier absent ou corrompu n'est jamais fatal : l'état repart vide et
    /// un avertissement est journalisé.
    pub fn load(&self) -> Result<()> {
        let path = &self.inner.path;
        let raw = match std::fs::read(path) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(chemin = %path.display(), "aucun état updater, démarrage à vide");
                *self.inner.state.write() = State::default();
                return Ok(());
            }
            Err(e) => {
                tracing::warn!(
                    chemin = %path.display(),
                    erreur = %e,
                    "état updater illisible, démarrage à vide"
                );
                *self.inner.state.write() = State::default();
                return Ok(());
            }
        };

        match serde_json::from_slice::<State>(&raw) {
            Ok(mut s) => {
                truncate_history(&mut s.history);
                let watchers = s.watchers.len();
                let findings = s.findings.len();
                *self.inner.state.write() = s;
                tracing::debug!(watchers, findings, "état updater chargé");
            }
            Err(e) => {
                tracing::warn!(
                    chemin = %path.display(),
                    erreur = %e,
                    "état updater corrompu, démarrage à vide"
                );
                *self.inner.state.write() = State::default();
            }
        }
        Ok(())
    }

    /// Écrit l'état courant sur le disque, de façon atomique.
    pub fn save(&self) -> Result<()> {
        let bytes = {
            let guard = self.inner.state.read();
            serde_json::to_vec_pretty(&*guard)?
        };
        write_atomic(&self.inner.path, &bytes)
    }

    /// Applique une mutation puis persiste immédiatement.
    fn mutate<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut State) -> Result<T>,
    {
        let out = {
            let mut guard = self.inner.state.write();
            f(&mut guard)?
        };
        self.save()?;
        Ok(out)
    }

    // --- watchers -----------------------------------------------------------

    /// Liste tous les watchers enregistrés.
    pub fn watchers(&self) -> Vec<WatcherSpec> {
        self.inner.state.read().watchers.clone()
    }

    /// Crée ou remplace un watcher (clé : `id`).
    pub fn upsert_watcher(&self, w: WatcherSpec) -> Result<()> {
        if w.id.trim().is_empty() {
            return Err(Error::Invalid("identifiant de watcher vide".to_string()));
        }
        self.mutate(|s| {
            match s.watchers.iter_mut().find(|x| x.id == w.id) {
                Some(slot) => *slot = w,
                None => s.watchers.push(w),
            }
            Ok(())
        })
    }

    /// Supprime un watcher ainsi que les constats qui s'y rattachent.
    pub fn remove_watcher(&self, id: &str) -> Result<()> {
        self.mutate(|s| {
            let avant = s.watchers.len();
            s.watchers.retain(|w| w.id != id);
            if s.watchers.len() == avant {
                return Err(Error::NotFound(format!("watcher {id} introuvable")));
            }
            s.findings.retain(|f| f.watcher_id != id);
            Ok(())
        })
    }

    /// Récupère un watcher par son identifiant.
    pub fn get_watcher(&self, id: &str) -> Option<WatcherSpec> {
        self.inner
            .state
            .read()
            .watchers
            .iter()
            .find(|w| w.id == id)
            .cloned()
    }

    // --- constats -----------------------------------------------------------

    /// Liste les constats de mise à jour en attente.
    pub fn findings(&self) -> Vec<UpdateFinding> {
        self.inner.state.read().findings.clone()
    }

    /// Remplace l'intégralité des constats.
    pub fn set_findings(&self, f: Vec<UpdateFinding>) -> Result<()> {
        self.mutate(|s| {
            s.findings = f;
            Ok(())
        })
    }

    /// Récupère un constat par son identifiant.
    pub fn get_finding(&self, id: &str) -> Option<UpdateFinding> {
        self.inner
            .state
            .read()
            .findings
            .iter()
            .find(|f| f.id == id)
            .cloned()
    }

    // --- historique ---------------------------------------------------------

    /// Ajoute une entrée d'historique (tronqué à 500 entrées).
    pub fn push_history(&self, r: RolloutResult) -> Result<()> {
        self.mutate(|s| {
            s.history.push(r);
            truncate_history(&mut s.history);
            Ok(())
        })
    }

    /// Historique, du plus récent au plus ancien, borné à `limit` entrées.
    pub fn history(&self, limit: usize) -> Vec<RolloutResult> {
        let guard = self.inner.state.read();
        guard.history.iter().rev().take(limit).cloned().collect()
    }

    // --- réglages -----------------------------------------------------------

    /// Réglages bruts, **secrets inclus**. Réservé à l'usage interne (moteur,
    /// planificateur, vérification de signature) : ne jamais renvoyer ce résultat
    /// tel quel à l'API HTTP.
    pub fn settings(&self) -> UpdaterSettings {
        self.inner.state.read().settings.clone()
    }

    /// Réglages destinés à l'interface : les secrets non vides sont remplacés
    /// par [`SECRET_MASK`], les secrets absents restent `None`.
    pub fn settings_redacted(&self) -> UpdaterSettings {
        let mut s = self.settings();
        s.github_token = redact(s.github_token);
        s.webhook_secret = redact(s.webhook_secret);
        s
    }

    /// Enregistre de nouveaux réglages.
    ///
    /// Un secret égal à [`SECRET_MASK`] signifie « inchangé » : l'interface
    /// renvoie le masque qu'elle a reçu, on conserve donc la valeur existante.
    pub fn set_settings(&self, s: UpdaterSettings) -> Result<()> {
        self.mutate(|state| {
            let mut next = s;
            next.github_token =
                merge_secret(state.settings.github_token.clone(), next.github_token);
            next.webhook_secret =
                merge_secret(state.settings.webhook_secret.clone(), next.webhook_secret);
            state.settings = next;
            Ok(())
        })
    }
}

/// Remplace un secret non vide par le masque d'affichage.
fn redact(v: Option<String>) -> Option<String> {
    match v {
        Some(s) if !s.is_empty() => Some(SECRET_MASK.to_string()),
        other => other,
    }
}

/// Conserve l'ancien secret si le nouveau vaut exactement le masque.
fn merge_secret(ancien: Option<String>, nouveau: Option<String>) -> Option<String> {
    match nouveau {
        Some(n) if n == SECRET_MASK => ancien,
        Some(n) if n.trim().is_empty() => None,
        Some(n) => Some(n),
        None => None,
    }
}

/// Borne l'historique en conservant les entrées les plus récentes.
fn truncate_history(history: &mut Vec<RolloutResult>) {
    if history.len() > HISTORY_MAX {
        let excedent = history.len() - HISTORY_MAX;
        history.drain(0..excedent);
    }
}

/// Écrit `data` dans `path` sans jamais laisser de fichier partiellement écrit.
///
/// Le contenu part dans `<path>.tmp`, est synchronisé sur le disque, puis
/// renommé : `rename` est atomique sur le même système de fichiers.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::Store(format!("création de {} impossible: {e}", parent.display()))
            })?;
        }
    }

    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);

    let mut file = std::fs::File::create(&tmp)
        .map_err(|e| Error::Store(format!("écriture de {} impossible: {e}", tmp.display())))?;

    // Les permissions sont posées avant d'écrire le moindre octet de secret.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = file.set_permissions(std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!(
                chemin = %tmp.display(),
                erreur = %e,
                "permissions 0600 non appliquées au fichier d'état"
            );
        }
    }

    file.write_all(data)
        .map_err(|e| Error::Store(format!("écriture de {} impossible: {e}", tmp.display())))?;
    file.sync_all().map_err(|e| {
        Error::Store(format!(
            "synchronisation de {} impossible: {e}",
            tmp.display()
        ))
    })?;
    drop(file);

    std::fs::rename(&tmp, path).map_err(|e| {
        // Ne pas laisser traîner le fichier temporaire si le renommage échoue.
        let _ = std::fs::remove_file(&tmp);
        Error::Store(format!(
            "remplacement de {} impossible: {e}",
            path.display()
        ))
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{UpdateChannel, UpdateSource, WatchTarget};
    use chrono::Utc;

    fn store_temporaire() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("répertoire temporaire");
        let path = dir.path().join("sous/dossier/updater.json");
        let s = Store::new(path);
        s.load().expect("chargement initial");
        (dir, s)
    }

    fn watcher(id: &str, nom: &str) -> WatcherSpec {
        WatcherSpec {
            id: id.to_string(),
            name: nom.to_string(),
            enabled: true,
            cluster: "prod".to_string(),
            target: WatchTarget::new("Deployment", Some("web".to_string()), nom),
            container: None,
            source: UpdateSource::GithubRelease {
                owner: "kubewatch-io".to_string(),
                repo: "kubewatch".to_string(),
                tag_prefix: None,
            },
            policy: UpdatePolicy::default(),
            created_at: Utc::now(),
            last_checked_at: None,
            last_known_version: Some("1.0.0".to_string()),
        }
    }

    fn rollout(n: usize) -> RolloutResult {
        RolloutResult {
            finding_id: format!("f{n}"),
            target: WatchTarget::new("Deployment", Some("web".to_string()), "nginx"),
            previous_image: Some(format!("nginx:1.0.{n}")),
            new_image: format!("nginx:1.0.{}", n + 1),
            applied_at: Utc::now(),
            status: RolloutResult::STATUS_APPLIED.to_string(),
        }
    }

    fn finding(id: &str, watcher_id: &str) -> UpdateFinding {
        UpdateFinding {
            id: id.to_string(),
            watcher_id: watcher_id.to_string(),
            watcher_name: "nginx".to_string(),
            cluster: "prod".to_string(),
            target_kind: "Deployment".to_string(),
            target_namespace: Some("web".to_string()),
            target_name: "nginx".to_string(),
            container: None,
            current_version: Some("1.0.0".to_string()),
            current_image: Some("nginx:1.0.0".to_string()),
            available_version: "1.0.1".to_string(),
            available_image: Some("nginx:1.0.1".to_string()),
            source: UpdateSource::ContainerRegistry {
                image: "nginx".to_string(),
            },
            severity: crate::model::UpdateSeverity::Patch,
            release: None,
            detected_at: Utc::now(),
            applied: false,
        }
    }

    #[test]
    fn fichier_absent_donne_un_etat_vide() {
        let (_d, s) = store_temporaire();
        assert!(s.watchers().is_empty());
        assert!(s.findings().is_empty());
        assert!(s.history(10).is_empty());
        assert_eq!(s.settings(), UpdaterSettings::default());
        assert!(
            !s.path().exists(),
            "aucun fichier ne doit être créé au chargement"
        );
    }

    #[test]
    fn ecriture_puis_relecture() {
        let (dir, s) = store_temporaire();
        s.upsert_watcher(watcher("w1", "nginx")).expect("upsert");
        s.upsert_watcher(watcher("w2", "redis")).expect("upsert");
        s.set_findings(vec![finding("f1", "w1")]).expect("findings");
        s.push_history(rollout(1)).expect("historique");

        // Le dossier parent a bien été créé récursivement.
        assert!(
            s.path().exists(),
            "le fichier d'état doit exister après une mutation"
        );

        let relu = Store::new(dir.path().join("sous/dossier/updater.json"));
        relu.load().expect("relecture");
        assert_eq!(relu.watchers().len(), 2);
        assert_eq!(relu.get_watcher("w2").expect("w2").name, "redis");
        assert_eq!(relu.findings().len(), 1);
        assert_eq!(relu.history(10).len(), 1);
        assert_eq!(relu.history(10)[0].finding_id, "f1");
    }

    #[test]
    fn upsert_remplace_au_lieu_de_dupliquer() {
        let (_d, s) = store_temporaire();
        s.upsert_watcher(watcher("w1", "nginx")).expect("upsert");
        let mut modifie = watcher("w1", "nginx-v2");
        modifie.enabled = false;
        s.upsert_watcher(modifie).expect("upsert 2");
        assert_eq!(s.watchers().len(), 1);
        let w = s.get_watcher("w1").expect("w1");
        assert_eq!(w.name, "nginx-v2");
        assert!(!w.enabled);
    }

    #[test]
    fn upsert_refuse_un_identifiant_vide() {
        let (_d, s) = store_temporaire();
        let mut w = watcher("  ", "x");
        w.id = "  ".to_string();
        assert!(s.upsert_watcher(w).is_err());
    }

    #[test]
    fn suppression_retire_aussi_les_constats() {
        let (_d, s) = store_temporaire();
        s.upsert_watcher(watcher("w1", "nginx")).expect("upsert");
        s.upsert_watcher(watcher("w2", "redis")).expect("upsert");
        s.set_findings(vec![finding("f1", "w1"), finding("f2", "w2")])
            .expect("findings");

        s.remove_watcher("w1").expect("suppression");
        assert_eq!(s.watchers().len(), 1);
        let restants = s.findings();
        assert_eq!(restants.len(), 1);
        assert_eq!(restants[0].watcher_id, "w2");

        // Suppression d'un identifiant inconnu -> 404.
        let e = s.remove_watcher("inconnu").expect_err("doit échouer");
        assert_eq!(e.status_code(), 404);
    }

    #[test]
    fn historique_borne_a_cinq_cents_et_renvoye_du_plus_recent() {
        let (_d, s) = store_temporaire();
        for i in 0..520 {
            s.push_history(rollout(i)).expect("historique");
        }
        let h = s.history(1000);
        assert_eq!(h.len(), HISTORY_MAX);
        // Le plus récent en tête...
        assert_eq!(h[0].finding_id, "f519");
        // ...et les 20 plus anciens ont été évincés.
        assert_eq!(h[HISTORY_MAX - 1].finding_id, "f20");

        let court = s.history(3);
        assert_eq!(court.len(), 3);
        assert_eq!(court[0].finding_id, "f519");
        assert_eq!(court[2].finding_id, "f517");

        // La troncature survit à un rechargement.
        let relu = Store::new(s.path().to_path_buf());
        relu.load().expect("relecture");
        assert_eq!(relu.history(1000).len(), HISTORY_MAX);
    }

    #[test]
    fn reglages_masques_et_conserves() {
        let (_d, s) = store_temporaire();
        s.set_settings(UpdaterSettings {
            github_token: Some("ghp_secret".to_string()),
            webhook_secret: Some("whsec".to_string()),
            default_policy: UpdatePolicy {
                channel: UpdateChannel::Minor,
                ..Default::default()
            },
            scheduler_enabled: true,
        })
        .expect("réglages");

        let vus = s.settings_redacted();
        assert_eq!(vus.github_token.as_deref(), Some(SECRET_MASK));
        assert_eq!(vus.webhook_secret.as_deref(), Some(SECRET_MASK));
        assert_eq!(vus.default_policy.channel, UpdateChannel::Minor);

        // L'interface renvoie le masque : les secrets ne doivent pas être écrasés.
        let mut renvoi = vus.clone();
        renvoi.scheduler_enabled = false;
        s.set_settings(renvoi).expect("réglages 2");
        let clair = s.settings();
        assert_eq!(clair.github_token.as_deref(), Some("ghp_secret"));
        assert_eq!(clair.webhook_secret.as_deref(), Some("whsec"));
        assert!(!clair.scheduler_enabled);
    }

    #[test]
    fn reglages_effacables_et_remplacables() {
        let (_d, s) = store_temporaire();
        s.set_settings(UpdaterSettings {
            github_token: Some("ghp_a".to_string()),
            ..Default::default()
        })
        .expect("réglages");

        // Chaîne vide -> effacement.
        s.set_settings(UpdaterSettings {
            github_token: Some(String::new()),
            ..Default::default()
        })
        .expect("effacement");
        assert_eq!(s.settings().github_token, None);
        assert_eq!(s.settings_redacted().github_token, None);

        // Nouvelle valeur -> remplacement.
        s.set_settings(UpdaterSettings {
            github_token: Some("ghp_b".to_string()),
            ..Default::default()
        })
        .expect("remplacement");
        assert_eq!(s.settings().github_token.as_deref(), Some("ghp_b"));
    }

    #[test]
    fn fichier_corrompu_ne_plante_pas() {
        let dir = tempfile::tempdir().expect("répertoire temporaire");
        let path = dir.path().join("updater.json");
        std::fs::write(&path, b"{ ceci n'est pas du JSON").expect("écriture");
        let s = Store::new(path);
        s.load().expect("le chargement ne doit jamais échouer");
        assert!(s.watchers().is_empty());
        assert_eq!(s.settings(), UpdaterSettings::default());

        // Et le magasin reste utilisable : la prochaine écriture répare le fichier.
        s.upsert_watcher(watcher("w1", "nginx")).expect("upsert");
        let relu = Store::new(s.path().to_path_buf());
        relu.load().expect("relecture");
        assert_eq!(relu.watchers().len(), 1);
    }

    #[test]
    fn aucun_fichier_temporaire_ne_subsiste() {
        let (_d, s) = store_temporaire();
        s.upsert_watcher(watcher("w1", "nginx")).expect("upsert");
        let tmp = s.path().with_file_name("updater.json.tmp");
        assert!(
            !tmp.exists(),
            "le fichier temporaire doit avoir été renommé"
        );
    }

    #[cfg(unix)]
    #[test]
    fn permissions_restreintes_sur_unix() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, s) = store_temporaire();
        s.upsert_watcher(watcher("w1", "nginx")).expect("upsert");
        let mode = std::fs::metadata(s.path())
            .expect("métadonnées")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "mode inattendu: {:o}", mode & 0o777);
    }

    #[test]
    fn magasin_partageable_entre_clones() {
        let (_d, s) = store_temporaire();
        let clone = s.clone();
        clone
            .upsert_watcher(watcher("w1", "nginx"))
            .expect("upsert");
        assert_eq!(s.watchers().len(), 1, "les clones partagent le même état");
    }

    #[test]
    fn json_persiste_en_camel_case() {
        let (_d, s) = store_temporaire();
        s.upsert_watcher(watcher("w1", "nginx")).expect("upsert");
        let brut = std::fs::read_to_string(s.path()).expect("lecture");
        assert!(
            brut.contains("\"createdAt\""),
            "clé camelCase absente: {brut}"
        );
        assert!(brut.contains("\"schedulerEnabled\""));
    }
}
