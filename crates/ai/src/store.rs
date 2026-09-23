//! Persistance des réglages de l'assistant : `<dossier d'état>/ai.json`.
//!
//! Le fichier contient les clés d'API en clair ; il est écrit de manière
//! atomique et, sous Unix, en `0600` dans un dossier `0700`, comme les autres
//! fichiers d'état de KubeWatch.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::claude_code;
use crate::config::{
    AiSettings, ProfileUpdate, ProfileView, ProviderKind, ProviderProfile, SettingsView,
};
use crate::error::{Error, Result};

/// Nom du fichier dans le dossier d'état.
pub const FILE_NAME: &str = "ai.json";

/// Nom donné au profil créé à partir du compte de Claude Code.
pub const CLAUDE_CODE_PROFILE: &str = "Compte Claude (Claude Code)";

/// Réglages de l'assistant, en mémoire et sur disque.
#[derive(Debug)]
pub struct AiStore {
    path: PathBuf,
    settings: RwLock<AiSettings>,
    load_error: Option<String>,
}

impl AiStore {
    /// Ouvre le fichier du dossier d'état donné.
    ///
    /// Un fichier absent donne des réglages vides ; un fichier illisible aussi,
    /// mais la cause est conservée dans [`AiStore::load_error`] pour être
    /// signalée à l'utilisateur plutôt que d'empêcher l'application de démarrer.
    pub fn open(state_dir: &Path) -> Self {
        let path = state_dir.join(FILE_NAME);
        let (settings, load_error) = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<AiSettings>(&bytes) {
                Ok(s) => (s, None),
                Err(e) => {
                    tracing::warn!(fichier = %path.display(), erreur = %e, "réglages IA illisibles");
                    (AiSettings::default(), Some(e.to_string()))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (AiSettings::default(), None),
            Err(e) => {
                tracing::warn!(fichier = %path.display(), erreur = %e, "réglages IA illisibles");
                (AiSettings::default(), Some(e.to_string()))
            }
        };
        Self {
            path,
            settings: RwLock::new(settings),
            load_error,
        }
    }

    /// Chemin du fichier.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Cause de l'échec de lecture initial, le cas échéant.
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// Copie des réglages complets, secrets compris.
    pub fn settings(&self) -> AiSettings {
        self.settings.read().expect("verrou empoisonné").clone()
    }

    /// Vue sans secret, pour l'interface.
    pub fn view(&self) -> SettingsView {
        self.settings.read().expect("verrou empoisonné").view()
    }

    /// Profil actif, s'il y en a un.
    pub fn active_profile(&self) -> Option<ProviderProfile> {
        self.settings
            .read()
            .expect("verrou empoisonné")
            .active()
            .cloned()
    }

    /// Profil désigné, ou profil actif si `id` est `None`.
    pub fn profile(&self, id: Option<&str>) -> Result<ProviderProfile> {
        let s = self.settings.read().expect("verrou empoisonné");
        match id {
            Some(id) => s
                .profiles
                .iter()
                .find(|p| p.id == id)
                .cloned()
                .ok_or_else(|| Error::Config(format!("profil « {id} » inconnu"))),
            None => s.active().cloned().ok_or_else(|| {
                Error::Config(
                    "aucun fournisseur d'IA n'est configuré : ajoutez-en un dans les réglages"
                        .to_string(),
                )
            }),
        }
    }

    /// Crée ou modifie un profil, puis enregistre.
    pub fn upsert_profile(&self, update: ProfileUpdate) -> Result<ProfileView> {
        let name = update.name.trim().to_string();
        if name.is_empty() {
            return Err(Error::Config("le nom du profil est vide".to_string()));
        }
        let model = update.model.trim().to_string();
        let base_url = update
            .base_url
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty());
        if let Some(u) = &base_url {
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                return Err(Error::Config(format!(
                    "l'adresse « {u} » doit commencer par http:// ou https://"
                )));
            }
        }

        let mut s = self.settings.write().expect("verrou empoisonné");
        let view = match update
            .id
            .as_deref()
            .map(str::trim)
            .filter(|i| !i.is_empty())
        {
            Some(id) => {
                let p = s
                    .profiles
                    .iter_mut()
                    .find(|p| p.id == id)
                    .ok_or_else(|| Error::Config(format!("profil « {id} » inconnu")))?;
                p.name = name;
                p.kind = update.kind;
                p.base_url = base_url;
                p.model = model;
                p.max_output_tokens = update.max_output_tokens.filter(|n| *n > 0);
                p.show_thinking = update.show_thinking;
                p.use_claude_code = update.use_claude_code;
                match update.api_key.as_deref() {
                    None => {}
                    Some("") => p.api_key = None,
                    Some(k) => p.api_key = Some(k.trim().to_string()),
                }
                ProfileView::from(&*p)
            }
            None => {
                let p = ProviderProfile {
                    id: uuid::Uuid::new_v4().to_string(),
                    name,
                    kind: update.kind,
                    base_url,
                    api_key: update
                        .api_key
                        .map(|k| k.trim().to_string())
                        .filter(|k| !k.is_empty()),
                    model,
                    max_output_tokens: update.max_output_tokens.filter(|n| *n > 0),
                    show_thinking: update.show_thinking,
                    use_claude_code: update.use_claude_code,
                };
                let view = ProfileView::from(&p);
                // Le premier profil créé devient actif : l'assistant est utilisable
                // sans clic supplémentaire.
                if s.active_profile.is_none() {
                    s.active_profile = Some(p.id.clone());
                }
                s.profiles.push(p);
                view
            }
        };
        self.persist(&s)?;
        Ok(view)
    }

    /// Crée — ou réactive — le profil adossé au compte de Claude Code.
    ///
    /// Le modèle et l'adresse de base sont repris des réglages de Claude Code
    /// quand celui-ci en impose ; le profil devient actif.
    pub fn use_claude_code(&self) -> Result<ProfileView> {
        let account = claude_code::account().ok_or_else(|| {
            Error::Config(format!(
                "aucun identifiant de Claude Code dans {} : lancez « claude » et connectez-vous",
                claude_code::dir_label()
            ))
        })?;

        let mut s = self.settings.write().expect("verrou empoisonné");
        let id = match s.profiles.iter().find(|p| p.uses_claude_code()) {
            Some(p) => p.id.clone(),
            None => {
                let p = ProviderProfile {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: CLAUDE_CODE_PROFILE.to_string(),
                    kind: ProviderKind::Anthropic,
                    base_url: account.base_url.clone(),
                    api_key: None,
                    model: account
                        .model
                        .clone()
                        .unwrap_or_else(|| ProviderKind::Anthropic.default_model().to_string()),
                    max_output_tokens: None,
                    show_thinking: false,
                    use_claude_code: true,
                };
                let id = p.id.clone();
                s.profiles.push(p);
                id
            }
        };
        s.active_profile = Some(id.clone());
        let view = s
            .profiles
            .iter()
            .find(|p| p.id == id)
            .map(ProfileView::from)
            .expect("le profil vient d'être ajouté");
        self.persist(&s)?;
        Ok(view)
    }

    /// Premier démarrage : reprendre le compte de Claude Code s'il y en a un.
    ///
    /// On ne touche à rien dès qu'un profil existe : les réglages de
    /// l'utilisateur priment. Un échec n'est pas bloquant, l'assistant se
    /// configure aussi à la main.
    pub fn seed_from_claude_code(&self) -> Option<ProfileView> {
        if !self
            .settings
            .read()
            .expect("verrou empoisonné")
            .profiles
            .is_empty()
        {
            return None;
        }
        match self.use_claude_code() {
            Ok(v) => {
                tracing::info!(profil = %v.name, modele = %v.model, "compte de Claude Code repris");
                Some(v)
            }
            Err(e) => {
                tracing::debug!(erreur = %e, "pas de compte de Claude Code à reprendre");
                None
            }
        }
    }

    /// Supprime un profil, puis enregistre.
    pub fn remove_profile(&self, id: &str) -> Result<()> {
        let mut s = self.settings.write().expect("verrou empoisonné");
        let before = s.profiles.len();
        s.profiles.retain(|p| p.id != id);
        if s.profiles.len() == before {
            return Err(Error::Config(format!("profil « {id} » inconnu")));
        }
        if s.active_profile.as_deref() == Some(id) {
            s.active_profile = s.profiles.first().map(|p| p.id.clone());
        }
        self.persist(&s)
    }

    /// Désigne le profil actif (`None` = aucun), puis enregistre.
    pub fn set_active(&self, id: Option<&str>) -> Result<()> {
        let mut s = self.settings.write().expect("verrou empoisonné");
        if let Some(id) = id {
            if !s.profiles.iter().any(|p| p.id == id) {
                return Err(Error::Config(format!("profil « {id} » inconnu")));
            }
        }
        s.active_profile = id.map(str::to_string);
        self.persist(&s)
    }

    /// Enregistre les réglages généraux.
    pub fn set_general(
        &self,
        tools_enabled: bool,
        max_tool_rounds: u32,
        extra_instructions: String,
    ) -> Result<()> {
        let mut s = self.settings.write().expect("verrou empoisonné");
        s.tools_enabled = tools_enabled;
        s.max_tool_rounds = max_tool_rounds.clamp(1, 50);
        s.extra_instructions = extra_instructions.trim().to_string();
        self.persist(&s)
    }

    /// Écriture atomique : fichier temporaire à côté, permissions, renommage.
    fn persist(&self, s: &AiSettings) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        let tmp = self.path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(s)?;
        {
            let mut f = std::fs::File::create(&tmp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            use std::io::Write as _;
            f.write_all(&data)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderKind;

    fn update(name: &str, kind: ProviderKind, key: Option<&str>) -> ProfileUpdate {
        ProfileUpdate {
            id: None,
            name: name.into(),
            kind,
            base_url: None,
            api_key: key.map(str::to_string),
            model: kind.default_model().into(),
            max_output_tokens: None,
            show_thinking: false,
            use_claude_code: false,
        }
    }

    #[test]
    fn creation_activation_et_persistance() {
        let dir = tempfile::tempdir().unwrap();
        let store = AiStore::open(dir.path());
        assert!(store.load_error().is_none());
        assert!(store.active_profile().is_none());

        let v = store
            .upsert_profile(update("Claude", ProviderKind::Anthropic, Some("sk-1")))
            .unwrap();
        assert!(v.api_key_set);
        assert_eq!(store.view().active_profile.as_deref(), Some(v.id.as_str()));

        // Relecture depuis le disque : la clé est bien là, mais pas dans la vue.
        let again = AiStore::open(dir.path());
        let p = again.profile(None).unwrap();
        assert_eq!(p.api_key.as_deref(), Some("sk-1"));
        let json = serde_json::to_string(&again.view()).unwrap();
        assert!(!json.contains("sk-1"));
    }

    #[test]
    fn modification_sans_toucher_a_la_cle_puis_effacement() {
        let dir = tempfile::tempdir().unwrap();
        let store = AiStore::open(dir.path());
        let v = store
            .upsert_profile(update("GPT", ProviderKind::OpenAi, Some("sk-2")))
            .unwrap();

        let mut u = update("GPT renommé", ProviderKind::OpenAi, None);
        u.id = Some(v.id.clone());
        u.model = "gpt-5-mini".into();
        let v2 = store.upsert_profile(u).unwrap();
        assert!(v2.api_key_set, "None ne doit pas effacer la clé");
        assert_eq!(v2.name, "GPT renommé");
        assert_eq!(
            store.profile(Some(&v.id)).unwrap().api_key.as_deref(),
            Some("sk-2")
        );

        let mut u = update("GPT", ProviderKind::OpenAi, Some(""));
        u.id = Some(v.id.clone());
        let v3 = store.upsert_profile(u).unwrap();
        assert!(!v3.api_key_set, "une chaîne vide efface la clé");
    }

    #[test]
    fn suppression_reassigne_le_profil_actif() {
        let dir = tempfile::tempdir().unwrap();
        let store = AiStore::open(dir.path());
        let a = store
            .upsert_profile(update("A", ProviderKind::OpenAiCompatible, None))
            .unwrap();
        let b = store
            .upsert_profile(update("B", ProviderKind::OpenAiCompatible, None))
            .unwrap();
        assert_eq!(store.view().active_profile.as_deref(), Some(a.id.as_str()));
        store.remove_profile(&a.id).unwrap();
        assert_eq!(store.view().active_profile.as_deref(), Some(b.id.as_str()));
        assert!(store.remove_profile("inconnu").is_err());
        assert!(store.set_active(Some("inconnu")).is_err());
        store.set_active(None).unwrap();
        assert!(store.profile(None).is_err());
    }

    #[test]
    fn validation_des_entrees() {
        let dir = tempfile::tempdir().unwrap();
        let store = AiStore::open(dir.path());
        assert!(store
            .upsert_profile(update("   ", ProviderKind::Anthropic, None))
            .is_err());
        let mut u = update("X", ProviderKind::OpenAiCompatible, None);
        u.base_url = Some("localhost:1234".into());
        assert!(store.upsert_profile(u).is_err());
        store
            .set_general(false, 500, "  sois bref  ".into())
            .unwrap();
        let v = store.view();
        assert!(!v.tools_enabled);
        assert_eq!(v.max_tool_rounds, 50);
        assert_eq!(v.extra_instructions, "sois bref");
    }

    #[test]
    fn reprise_du_compte_claude_code() {
        let maison = tempfile::tempdir().unwrap();
        let claude = maison.path().join(claude_code::DIR_NAME);
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join(claude_code::CREDENTIALS_FILE),
            format!(
                r#"{{"claudeAiOauth":{{"accessToken":"sk-ant-oat","expiresAt":{}}}}}"#,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64
                    + 3_600_000
            ),
        )
        .unwrap();
        // On détourne la détection vers ce faux dossier le temps du test.
        std::env::set_var(claude_code::DIR_ENV, &claude);

        let dir = tempfile::tempdir().unwrap();
        let store = AiStore::open(dir.path());
        let v = store.seed_from_claude_code().expect("compte détecté");
        assert!(v.use_claude_code);
        assert!(
            !v.api_key_set,
            "aucune clé n'est recopiée dans nos réglages"
        );
        assert_eq!(v.model, "claude-opus-5");
        assert_eq!(store.view().active_profile.as_deref(), Some(v.id.as_str()));

        // Deux appels ne font pas deux profils.
        let encore = store.use_claude_code().unwrap();
        assert_eq!(encore.id, v.id);
        assert_eq!(store.view().profiles.len(), 1);

        // Le jeton n'est jamais recopié dans le fichier de réglages.
        let ecrit = std::fs::read_to_string(store.path()).unwrap();
        assert!(!ecrit.contains("sk-ant-oat"));
        assert!(ecrit.contains("\"useClaudeCode\": true"));

        // Des réglages déjà peuplés ne sont pas touchés.
        let autre = tempfile::tempdir().unwrap();
        let store = AiStore::open(autre.path());
        store
            .upsert_profile(update("A", ProviderKind::OpenAiCompatible, None))
            .unwrap();
        assert!(store.seed_from_claude_code().is_none());

        std::env::remove_var(claude_code::DIR_ENV);
    }

    #[cfg(unix)]
    #[test]
    fn fichier_en_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = AiStore::open(dir.path());
        store
            .upsert_profile(update("A", ProviderKind::Anthropic, Some("k")))
            .unwrap();
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "mode inattendu : {:o}", mode & 0o777);
    }

    #[test]
    fn fichier_corrompu_signale_sans_bloquer() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), b"{ pas du json").unwrap();
        let store = AiStore::open(dir.path());
        assert!(store.load_error().is_some());
        assert!(store.view().profiles.is_empty());
    }
}
