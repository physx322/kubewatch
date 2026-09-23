//! Identifiants de Claude Code : lecture du dossier `~/.claude`.
//!
//! Claude Code, la ligne de commande d'Anthropic, dépose dans ce dossier les
//! jetons du compte connecté (`.credentials.json`) et ses réglages
//! (`settings.json`). KubeWatch les relit pour que l'assistant fonctionne sans
//! ressaisir de clé : c'est le compte Claude déjà connecté qui répond.
//!
//! Ordre retenu, le compte d'abord :
//!
//! 1. `.credentials.json` — jeton du compte, présenté en `Authorization: Bearer`
//!    accompagné de l'en-tête bêta [`OAUTH_BETA`] ;
//! 2. `settings.json` — bloc `env`, `ANTHROPIC_API_KEY` ou `ANTHROPIC_AUTH_TOKEN` ;
//! 3. l'environnement du processus, mêmes variables.
//!
//! Un jeton échu cède la place à une clé, quand il y en a une. Rien n'est écrit
//! dans le dossier, et le jeton ne sort jamais vers l'interface : seule une
//! description sans secret ([`Account`]) est publiée.
//!
//! Le `apiKeyHelper` des réglages de Claude Code n'est pas honoré : il faudrait
//! exécuter une commande arbitraire au démarrage de l'application.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

/// Variable qui déplace le dossier de configuration de Claude Code.
pub const DIR_ENV: &str = "CLAUDE_CONFIG_DIR";
/// Nom du dossier sous le répertoire personnel.
pub const DIR_NAME: &str = ".claude";
/// Jetons du compte, écrits par `claude` à la connexion.
pub const CREDENTIALS_FILE: &str = ".credentials.json";
/// Réglages de Claude Code ; son bloc `env` peut porter une clé d'API.
pub const SETTINGS_FILE: &str = "settings.json";
/// Configuration générale, à côté du dossier : courriel et organisation.
pub const CONFIG_FILE: &str = ".claude.json";
/// En-tête bêta exigé par l'API quand on présente un jeton de compte.
pub const OAUTH_BETA: &str = "oauth-2025-04-20";

/// Marge avant l'échéance : un jeton qui expire dans la minute est considéré
/// comme échu, le temps qu'une réponse un peu longue se termine.
const MARGE_MS: i64 = 60_000;

/// Nature de l'identifiant trouvé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialKind {
    /// Jeton du compte Claude, présenté en `Authorization: Bearer`.
    Oauth,
    /// Clé d'API classique, présentée en `x-api-key`.
    ApiKey,
}

impl CredentialKind {
    /// Libellé français.
    pub fn label(self) -> &'static str {
        match self {
            CredentialKind::Oauth => "compte Claude",
            CredentialKind::ApiKey => "clé d'API",
        }
    }
}

/// Identifiant utilisable. Le `Debug` masque le jeton.
#[derive(Clone)]
pub struct Credential {
    /// Comment le présenter au serveur.
    pub kind: CredentialKind,
    /// Le secret lui-même.
    pub token: String,
    /// Échéance en millisecondes depuis l'époque Unix, quand elle est connue.
    pub expires_at: Option<i64>,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("kind", &self.kind)
            .field("token", &"(masqué)")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl Credential {
    /// Vrai si l'échéance est connue et dépassée, marge comprise.
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(ms) => ms - MARGE_MS <= now_ms(),
            None => false,
        }
    }
}

/// Ce que l'on a trouvé, sans le secret : montrable à l'interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// Dossier examiné.
    pub dir: String,
    /// Fichier ou variable d'où vient l'identifiant.
    pub source: String,
    /// Nature de l'identifiant.
    pub kind: CredentialKind,
    /// Courriel du compte, si `.claude.json` le donne.
    pub email: Option<String>,
    /// Organisation du compte, si elle est connue.
    pub organization: Option<String>,
    /// Formule (`max`, `pro`…), telle qu'écrite par Claude Code.
    pub subscription: Option<String>,
    /// Échéance du jeton, en millisecondes depuis l'époque Unix.
    pub expires_at: Option<i64>,
    /// Vrai si le jeton est échu : il faut relancer `claude` pour le renouveler.
    pub expired: bool,
    /// Adresse de base imposée par les réglages de Claude Code, le cas échéant.
    pub base_url: Option<String>,
    /// Modèle choisi dans Claude Code, s'il s'agit d'un identifiant complet.
    pub model: Option<String>,
}

/// Résultat complet de la détection.
#[derive(Debug, Clone)]
pub struct Found {
    /// Description publiable.
    pub account: Account,
    /// Identifiant à présenter au serveur.
    pub credential: Credential,
}

/// Dossier de configuration de Claude Code ; `CLAUDE_CONFIG_DIR` l'emporte.
pub fn dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os(DIR_ENV) {
        let p = PathBuf::from(d);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    Some(dirs::home_dir()?.join(DIR_NAME))
}

/// Chemin du dossier tel qu'affiché dans les messages.
pub fn dir_label() -> String {
    dir()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| format!("~/{DIR_NAME}"))
}

/// Identifiants de Claude Code, dossier puis environnement du processus.
pub fn find() -> Option<Found> {
    let dir = dir()?;
    match find_in(&dir) {
        // Jeton échu : une clé prise dans l'environnement vaut mieux que rien,
        // mais on garde la découverte pour pouvoir expliquer l'échéance.
        Some(f) if f.credential.is_expired() => Some(from_env(&dir).unwrap_or(f)),
        Some(f) => Some(f),
        None => from_env(&dir),
    }
}

/// Description du compte détecté, sans secret.
pub fn account() -> Option<Account> {
    find().map(|f| f.account)
}

/// Identifiant à présenter à l'API, ou une erreur expliquant quoi faire.
pub fn credential() -> Result<Credential> {
    let found = find().ok_or_else(|| {
        Error::Config(format!(
            "aucun identifiant de Claude Code dans {} : lancez « claude » et connectez-vous, \
             ou renseignez une clé d'API dans le profil",
            dir_label()
        ))
    })?;
    if found.credential.is_expired() {
        return Err(Error::Config(format!(
            "le jeton du compte Claude ({}) a expiré : relancez « claude » pour le renouveler",
            found.account.source
        )));
    }
    Ok(found.credential)
}

/// Identifiants lus dans un dossier `.claude` donné.
///
/// Le compte connecté passe avant une clé d'API : c'est lui que l'on veut
/// employer. Une clé prend le relais si le jeton du compte est échu.
pub fn find_in(dir: &Path) -> Option<Found> {
    let settings = read_json(&dir.join(SETTINGS_FILE)).unwrap_or(Value::Null);
    let config = read_json(&dir.join(CONFIG_FILE))
        .or_else(|| dir.parent().and_then(|p| read_json(&p.join(CONFIG_FILE))))
        .unwrap_or(Value::Null);

    let base = Account {
        dir: dir.display().to_string(),
        source: String::new(),
        kind: CredentialKind::ApiKey,
        email: str_at(&config, &["oauthAccount", "emailAddress"]),
        organization: str_at(&config, &["oauthAccount", "organizationName"]),
        subscription: None,
        expires_at: None,
        expired: false,
        base_url: str_at(&settings, &["env", "ANTHROPIC_BASE_URL"]),
        model: model_id(&settings),
    };

    let mut candidates: Vec<Found> = Vec::new();

    // 1. Le compte connecté.
    if let Some(creds) = read_json(&dir.join(CREDENTIALS_FILE)) {
        if let Some(token) = str_at(&creds, &["claudeAiOauth", "accessToken"]) {
            let expires_at = creds
                .pointer("/claudeAiOauth/expiresAt")
                .and_then(Value::as_i64);
            let credential = Credential {
                kind: CredentialKind::Oauth,
                token,
                expires_at,
            };
            let account = Account {
                source: CREDENTIALS_FILE.to_string(),
                kind: CredentialKind::Oauth,
                subscription: str_at(&creds, &["claudeAiOauth", "subscriptionType"]),
                expires_at,
                expired: credential.is_expired(),
                ..base.clone()
            };
            candidates.push(Found {
                account,
                credential,
            });
        }
    }

    // 2. Une clé posée dans les réglages de Claude Code.
    for (name, kind) in ENV_NAMES {
        if let Some(token) = str_at(&settings, &["env", name]) {
            candidates.push(Found {
                account: Account {
                    source: format!("{SETTINGS_FILE} ({name})"),
                    kind,
                    ..base.clone()
                },
                credential: Credential {
                    kind,
                    token,
                    expires_at: None,
                },
            });
        }
    }

    if let Some(i) = candidates.iter().position(|c| !c.account.expired) {
        return Some(candidates.swap_remove(i));
    }
    candidates.into_iter().next()
}

/// Variables reconnues, dans l'ordre, comme le fait Claude Code.
const ENV_NAMES: [(&str, CredentialKind); 2] = [
    ("ANTHROPIC_API_KEY", CredentialKind::ApiKey),
    ("ANTHROPIC_AUTH_TOKEN", CredentialKind::Oauth),
];

/// Identifiant porté par l'environnement du processus.
fn from_env(dir: &Path) -> Option<Found> {
    let (name, kind, token) = ENV_NAMES.iter().find_map(|(name, kind)| {
        let v = std::env::var(name).ok()?;
        let v = v.trim();
        (!v.is_empty()).then(|| (*name, *kind, v.to_string()))
    })?;
    Some(Found {
        account: Account {
            dir: dir.display().to_string(),
            source: format!("variable {name}"),
            kind,
            email: None,
            organization: None,
            subscription: None,
            expires_at: None,
            expired: false,
            base_url: None,
            model: None,
        },
        credential: Credential {
            kind,
            token,
            expires_at: None,
        },
    })
}

/// Modèle des réglages de Claude Code, uniquement s'il s'agit d'un identifiant
/// complet : ses alias (« sonnet », « opusplan »…) ne valent rien pour l'API.
fn model_id(settings: &Value) -> Option<String> {
    str_at(settings, &["model"])
        .or_else(|| str_at(settings, &["env", "ANTHROPIC_MODEL"]))
        .filter(|m| m.starts_with("claude-"))
}

/// Contenu JSON d'un fichier ; absent ou illisible donne `None`.
fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(fichier = %path.display(), erreur = %e, "fichier de Claude Code illisible");
            None
        }
    }
}

/// Chaîne non vide à l'emplacement donné.
fn str_at(v: &Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    let s = cur.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Horloge, en millisecondes depuis l'époque Unix.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ecrire(path: &Path, contenu: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contenu).unwrap();
    }

    fn dans(ms: i64) -> i64 {
        now_ms() + ms
    }

    #[test]
    fn dossier_absent_ou_vide() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_in(&dir.path().join("rien")).is_none());
        assert!(find_in(dir.path()).is_none());
    }

    #[test]
    fn le_compte_connecte_passe_en_premier() {
        let maison = tempfile::tempdir().unwrap();
        let claude = maison.path().join(DIR_NAME);
        ecrire(
            &claude.join(CREDENTIALS_FILE),
            &format!(
                r#"{{"claudeAiOauth":{{"accessToken":"sk-ant-oat-x","expiresAt":{},"subscriptionType":"max"}}}}"#,
                dans(3_600_000)
            ),
        );
        ecrire(
            &claude.join(SETTINGS_FILE),
            r#"{"model":"claude-sonnet-5","env":{"ANTHROPIC_API_KEY":"sk-ant-cle"}}"#,
        );
        ecrire(
            &maison.path().join(CONFIG_FILE),
            r#"{"oauthAccount":{"emailAddress":"moi@example.com","organizationName":"Acme"}}"#,
        );

        let f = find_in(&claude).unwrap();
        assert_eq!(f.credential.kind, CredentialKind::Oauth);
        assert_eq!(f.credential.token, "sk-ant-oat-x");
        assert!(!f.credential.is_expired());
        assert_eq!(f.account.source, CREDENTIALS_FILE);
        assert_eq!(f.account.subscription.as_deref(), Some("max"));
        assert_eq!(f.account.email.as_deref(), Some("moi@example.com"));
        assert_eq!(f.account.organization.as_deref(), Some("Acme"));
        assert_eq!(f.account.model.as_deref(), Some("claude-sonnet-5"));
        assert!(f.account.base_url.is_none());

        // Ni le jeton ni la clé ne doivent apparaître dans ce qui part à l'interface.
        let json = serde_json::to_string(&f.account).unwrap();
        assert!(!json.contains("sk-ant"));
        // Le Debug non plus.
        assert!(!format!("{:?}", f.credential).contains("sk-ant"));
    }

    #[test]
    fn un_jeton_echu_cede_la_place_a_la_cle() {
        let claude = tempfile::tempdir().unwrap();
        ecrire(
            &claude.path().join(CREDENTIALS_FILE),
            &format!(
                r#"{{"claudeAiOauth":{{"accessToken":"vieux","expiresAt":{}}}}}"#,
                dans(-1_000)
            ),
        );
        ecrire(
            claude.path().join(SETTINGS_FILE).as_path(),
            r#"{"model":"opusplan","env":{"ANTHROPIC_AUTH_TOKEN":"jeton","ANTHROPIC_BASE_URL":"https://proxy.interne/"}}"#,
        );
        let f = find_in(claude.path()).unwrap();
        assert_eq!(f.credential.token, "jeton");
        assert_eq!(f.credential.kind, CredentialKind::Oauth);
        assert_eq!(
            f.account.base_url.as_deref(),
            Some("https://proxy.interne/")
        );
        assert!(f.account.model.is_none(), "« opusplan » est un alias");
    }

    #[test]
    fn un_jeton_echu_seul_est_signale() {
        let claude = tempfile::tempdir().unwrap();
        ecrire(
            &claude.path().join(CREDENTIALS_FILE),
            &format!(
                r#"{{"claudeAiOauth":{{"accessToken":"vieux","expiresAt":{}}}}}"#,
                dans(30_000)
            ),
        );
        let f = find_in(claude.path()).unwrap();
        assert!(f.account.expired, "la marge d'une minute s'applique");
        assert!(f.credential.is_expired());
    }

    #[test]
    fn fichiers_illisibles_ou_incomplets() {
        let claude = tempfile::tempdir().unwrap();
        ecrire(&claude.path().join(CREDENTIALS_FILE), "{ pas du json");
        ecrire(&claude.path().join(SETTINGS_FILE), r#"{"env":{}}"#);
        assert!(find_in(claude.path()).is_none());

        ecrire(
            &claude.path().join(CREDENTIALS_FILE),
            r#"{"claudeAiOauth":{"accessToken":"   "}}"#,
        );
        assert!(find_in(claude.path()).is_none(), "jeton vide = rien");
    }
}
