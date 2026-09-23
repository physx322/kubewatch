//! Profils de fournisseurs et réglages de l'assistant.
//!
//! Un *profil* décrit comment joindre un fournisseur : famille d'API, adresse,
//! clé, modèle. L'utilisateur peut en enregistrer plusieurs (un Claude, un
//! ChatGPT, un LM Studio local…) et en désigner un comme actif.

use serde::{Deserialize, Serialize};

/// Famille d'API d'un fournisseur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    /// Anthropic — API Messages.
    Anthropic,
    /// OpenAI — API Chat Completions.
    OpenAi,
    /// Tout serveur parlant le dialecte OpenAI : LM Studio, Ollama, llama.cpp,
    /// Jan, vLLM, ou un proxy.
    OpenAiCompatible,
}

impl ProviderKind {
    /// Libellé français.
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "Anthropic (Claude)",
            ProviderKind::OpenAi => "OpenAI (ChatGPT)",
            ProviderKind::OpenAiCompatible => "Compatible OpenAI (LM Studio, Ollama…)",
        }
    }

    /// Adresse de base employée quand le profil n'en précise pas.
    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::OpenAi => "https://api.openai.com/v1",
            // Port par défaut du serveur local de LM Studio.
            ProviderKind::OpenAiCompatible => "http://localhost:1234/v1",
        }
    }

    /// Modèle proposé à la création d'un profil.
    pub fn default_model(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "claude-opus-5",
            ProviderKind::OpenAi => "gpt-5",
            ProviderKind::OpenAiCompatible => "",
        }
    }

    /// Plafond de sortie par défaut. Les serveurs locaux ont souvent une fenêtre
    /// de contexte réduite : on reste modeste pour ne pas provoquer de rejet.
    pub fn default_max_output_tokens(self) -> u32 {
        match self {
            ProviderKind::Anthropic | ProviderKind::OpenAi => 32_000,
            ProviderKind::OpenAiCompatible => 4_096,
        }
    }

    /// Vrai si une clé d'API est indispensable.
    pub fn requires_api_key(self) -> bool {
        !matches!(self, ProviderKind::OpenAiCompatible)
    }
}

/// Profil de connexion à un fournisseur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProfile {
    /// Identifiant stable (UUID).
    pub id: String,
    /// Nom affiché.
    pub name: String,
    /// Famille d'API.
    pub kind: ProviderKind,
    /// Adresse de base ; `None` ou vide = valeur par défaut de la famille.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Clé d'API, en clair dans le fichier `0600`. Jamais renvoyée à l'interface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Identifiant du modèle.
    pub model: String,
    /// Plafond de jetons produits ; `None` = défaut de la famille.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Demander l'affichage du raisonnement (Claude 4.6 et suivants).
    #[serde(default)]
    pub show_thinking: bool,
    /// Reprendre les identifiants laissés par Claude Code dans `~/.claude`
    /// plutôt qu'une clé enregistrée ici. Anthropic seulement.
    #[serde(default)]
    pub use_claude_code: bool,
}

impl ProviderProfile {
    /// Adresse de base effective, sans barre oblique finale.
    pub fn endpoint(&self) -> String {
        let raw = self
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.kind.default_base_url());
        raw.trim_end_matches('/').to_string()
    }

    /// Plafond de sortie effectif.
    pub fn max_output_tokens(&self) -> u32 {
        self.max_output_tokens
            .filter(|n| *n > 0)
            .unwrap_or_else(|| self.kind.default_max_output_tokens())
    }

    /// Vrai si une clé est renseignée.
    pub fn has_api_key(&self) -> bool {
        self.api_key
            .as_deref()
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false)
    }

    /// Vrai si le profil s'appuie sur le compte de Claude Code.
    pub fn uses_claude_code(&self) -> bool {
        self.use_claude_code && matches!(self.kind, ProviderKind::Anthropic)
    }

    /// Vrai si le profil est utilisable en l'état : une clé, un compte Claude
    /// Code, ou un fournisseur qui n'en demande pas.
    pub fn is_usable(&self) -> bool {
        self.has_api_key() || self.uses_claude_code() || !self.kind.requires_api_key()
    }
}

/// Réglages complets, tels que persistés.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettings {
    /// Profils enregistrés.
    pub profiles: Vec<ProviderProfile>,
    /// Profil employé par l'assistant.
    pub active_profile: Option<String>,
    /// Proposer au modèle les outils de lecture du cluster.
    pub tools_enabled: bool,
    /// Nombre maximal de tours d'outils par question.
    pub max_tool_rounds: u32,
    /// Instructions ajoutées au message système.
    pub extra_instructions: String,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            profiles: Vec::new(),
            active_profile: None,
            tools_enabled: true,
            max_tool_rounds: 8,
            extra_instructions: String::new(),
        }
    }
}

impl AiSettings {
    /// Profil actif, s'il existe encore.
    pub fn active(&self) -> Option<&ProviderProfile> {
        let id = self.active_profile.as_deref()?;
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Vue sans secret.
    pub fn view(&self) -> SettingsView {
        SettingsView {
            profiles: self.profiles.iter().map(ProfileView::from).collect(),
            active_profile: self.active_profile.clone(),
            tools_enabled: self.tools_enabled,
            max_tool_rounds: self.max_tool_rounds,
            extra_instructions: self.extra_instructions.clone(),
        }
    }
}

/// Profil tel que présenté à l'interface : la clé n'y figure jamais.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    /// Identifiant.
    pub id: String,
    /// Nom affiché.
    pub name: String,
    /// Famille d'API.
    pub kind: ProviderKind,
    /// Adresse de base effective.
    pub base_url: String,
    /// Modèle.
    pub model: String,
    /// Vrai si une clé est enregistrée.
    pub api_key_set: bool,
    /// Plafond de sortie effectif.
    pub max_output_tokens: u32,
    /// Affichage du raisonnement.
    pub show_thinking: bool,
    /// Vrai si le profil emprunte les identifiants de Claude Code.
    pub use_claude_code: bool,
}

impl From<&ProviderProfile> for ProfileView {
    fn from(p: &ProviderProfile) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            kind: p.kind,
            base_url: p.endpoint(),
            model: p.model.clone(),
            api_key_set: p.has_api_key(),
            max_output_tokens: p.max_output_tokens(),
            show_thinking: p.show_thinking,
            use_claude_code: p.uses_claude_code(),
        }
    }
}

/// Réglages tels que présentés à l'interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    /// Profils, sans secret.
    pub profiles: Vec<ProfileView>,
    /// Profil actif.
    pub active_profile: Option<String>,
    /// Outils proposés au modèle.
    pub tools_enabled: bool,
    /// Tours d'outils maximum.
    pub max_tool_rounds: u32,
    /// Instructions supplémentaires.
    pub extra_instructions: String,
}

/// Création ou modification d'un profil.
///
/// `api_key` : `None` = inchangée, `Some("")` = effacée, `Some(clé)` = remplacée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdate {
    /// Identifiant du profil à modifier ; `None` = création.
    #[serde(default)]
    pub id: Option<String>,
    /// Nom affiché.
    pub name: String,
    /// Famille d'API.
    pub kind: ProviderKind,
    /// Adresse de base ; vide = défaut.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Clé d'API (voir la sémantique ci-dessus).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Modèle.
    pub model: String,
    /// Plafond de sortie ; `None` = défaut.
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    /// Affichage du raisonnement.
    #[serde(default)]
    pub show_thinking: bool,
    /// Employer les identifiants de Claude Code au lieu d'une clé.
    #[serde(default)]
    pub use_claude_code: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adresse_effective_et_plafond() {
        let mut p = ProviderProfile {
            id: "1".into(),
            name: "Local".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Some("http://127.0.0.1:1234/v1/".into()),
            api_key: None,
            model: "qwen".into(),
            max_output_tokens: None,
            show_thinking: false,
            use_claude_code: false,
        };
        assert_eq!(p.endpoint(), "http://127.0.0.1:1234/v1");
        assert_eq!(p.max_output_tokens(), 4_096);
        assert!(!p.has_api_key());

        p.base_url = Some("   ".into());
        assert_eq!(p.endpoint(), "http://localhost:1234/v1");
        p.max_output_tokens = Some(0);
        assert_eq!(p.max_output_tokens(), 4_096);
        p.max_output_tokens = Some(800);
        assert_eq!(p.max_output_tokens(), 800);
    }

    #[test]
    fn la_vue_ne_contient_pas_la_cle() {
        let p = ProviderProfile {
            id: "1".into(),
            name: "Claude".into(),
            kind: ProviderKind::Anthropic,
            base_url: None,
            api_key: Some("sk-ant-secret".into()),
            model: "claude-opus-5".into(),
            max_output_tokens: None,
            show_thinking: true,
            use_claude_code: false,
        };
        let v = ProfileView::from(&p);
        assert!(v.api_key_set);
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("secret"));
        assert!(json.contains("\"apiKeySet\":true"));
        assert_eq!(v.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn profil_adosse_au_compte_claude_code() {
        let mut p = ProviderProfile {
            id: "1".into(),
            name: "Compte Claude".into(),
            kind: ProviderKind::Anthropic,
            base_url: None,
            api_key: None,
            model: "claude-opus-5".into(),
            max_output_tokens: None,
            show_thinking: false,
            use_claude_code: true,
        };
        assert!(!p.has_api_key());
        assert!(p.uses_claude_code());
        assert!(p.is_usable(), "le compte remplace la clé");
        assert!(ProfileView::from(&p).use_claude_code);

        // Le drapeau ne vaut que pour Anthropic.
        p.kind = ProviderKind::OpenAi;
        assert!(!p.uses_claude_code());
        assert!(!p.is_usable());

        // Un fichier écrit avant cette version reste lisible.
        let ancien = r#"{"id":"2","name":"X","kind":"anthropic","model":"claude-opus-5"}"#;
        let p: ProviderProfile = serde_json::from_str(ancien).unwrap();
        assert!(!p.use_claude_code);
    }

    #[test]
    fn reglages_par_defaut_et_profil_actif() {
        let s = AiSettings::default();
        assert!(s.tools_enabled);
        assert_eq!(s.max_tool_rounds, 8);
        assert!(s.active().is_none());
        let parsed: AiSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, s);
    }
}
