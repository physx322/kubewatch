//! Réception des webhooks GitHub: vérification de la signature HMAC-SHA256 et
//! interprétation de la charge utile.
//!
//! GitHub signe chaque livraison avec le secret configuré sur le webhook et place
//! l'empreinte dans l'en-tête `X-Hub-Signature-256` au format `sha256=<hex>`.
//! La comparaison doit se faire en temps constant pour ne pas laisser fuir
//! l'empreinte attendue octet par octet.

use crate::error::{Error, Result};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Longueur d'une empreinte HMAC-SHA256, en octets.
const DIGEST_LEN: usize = 32;

/// Préfixe imposé par GitHub dans l'en-tête `X-Hub-Signature-256`.
const SIG_PREFIX: &[u8] = b"sha256=";

/// Longueur totale attendue de l'en-tête: `sha256=` + 64 caractères hexadécimaux.
const SIG_HEADER_LEN: usize = SIG_PREFIX.len() + DIGEST_LEN * 2;

/// Convertit un caractère hexadécimal en sa valeur numérique.
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Décode une chaîne hexadécimale en octets. Renvoie `None` si la longueur est
/// impaire ou si un caractère n'est pas hexadécimal.
fn decode_hex(src: &[u8]) -> Option<Vec<u8>> {
    if src.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(src.len() / 2);
    for pair in src.chunks_exact(2) {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

/// Vérifie la signature d'une livraison GitHub.
///
/// - `secret`: le secret partagé configuré côté GitHub et côté KubeWatch;
/// - `body`: le corps HTTP **brut**, tel que reçu (aucune re-sérialisation);
/// - `signature_header`: la valeur de `X-Hub-Signature-256`, format `sha256=<hex>`.
///
/// Un secret vide, un en-tête malformé, tronqué ou non hexadécimal renvoient
/// `false`. La comparaison finale se fait en temps constant sur les octets décodés;
/// la longueur est validée avant, ce qui ne divulgue rien de plus que la taille
/// de l'en-tête fourni par l'appelant lui-même.
pub fn verify_signature(secret: &[u8], body: &[u8], signature_header: &str) -> bool {
    // Un secret vide désactiverait de fait la vérification: on refuse.
    if secret.is_empty() {
        return false;
    }

    // On travaille sur les octets pour éviter toute question de frontière UTF-8.
    let header = signature_header.trim().as_bytes();
    if header.len() != SIG_HEADER_LEN {
        return false;
    }
    let (prefix, hex_part) = header.split_at(SIG_PREFIX.len());
    if !prefix.eq_ignore_ascii_case(SIG_PREFIX) {
        return false;
    }

    let provided = match decode_hex(hex_part) {
        Some(v) => v,
        None => return false,
    };
    if provided.len() != DIGEST_LEN {
        return false;
    }

    let mut mac = match HmacSha256::new_from_slice(secret) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    let expected = mac.finalize().into_bytes();

    // Comparaison à temps constant: les deux tranches font exactement DIGEST_LEN.
    bool::from(expected.as_slice().ct_eq(provided.as_slice()))
}

/// Évènement GitHub reconnu par KubeWatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WebhookEvent {
    /// Une release a été publiée (`action == "published"` uniquement).
    #[serde(rename_all = "camelCase")]
    Release {
        owner: String,
        repo: String,
        tag: String,
        prerelease: bool,
    },
    /// Un push sur une référence (branche ou tag).
    #[serde(rename_all = "camelCase")]
    Push {
        owner: String,
        repo: String,
        #[serde(rename = "ref")]
        ref_: String,
    },
    /// Livraison de test envoyée à la création du webhook.
    Ping,
    /// Évènement reconnu mais non exploité (le libellé est conservé pour les journaux).
    Other(String),
}

/// Extrait `(owner, repo)` du bloc `repository` d'une charge utile GitHub.
///
/// GitHub renseigne `repository.owner.login` pour les évènements applicatifs et
/// `repository.owner.name` pour certaines charges utiles de push; on accepte les
/// deux, et à défaut on retombe sur `repository.full_name`.
fn repository_of(payload: &serde_json::Value) -> Result<(String, String)> {
    let repo_node = payload
        .get("repository")
        .ok_or_else(|| Error::Invalid("charge utile GitHub sans champ `repository`".to_string()))?;

    let owner = repo_node
        .get("owner")
        .and_then(|o| {
            o.get("login")
                .and_then(serde_json::Value::as_str)
                .or_else(|| o.get("name").and_then(serde_json::Value::as_str))
        })
        .map(str::to_string);

    let name = repo_node
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    if let (Some(owner), Some(name)) = (owner.clone(), name.clone()) {
        if !owner.is_empty() && !name.is_empty() {
            return Ok((owner, name));
        }
    }

    // Repli sur `full_name` de la forme "owner/repo".
    if let Some(full) = repo_node
        .get("full_name")
        .and_then(serde_json::Value::as_str)
    {
        if let Some((o, r)) = full.split_once('/') {
            if !o.is_empty() && !r.is_empty() {
                return Ok((o.to_string(), r.to_string()));
            }
        }
    }

    Err(Error::Invalid(
        "impossible de déterminer le dépôt (owner/repo) de la charge utile GitHub".to_string(),
    ))
}

/// Interprète une livraison GitHub à partir du nom d'évènement (`X-GitHub-Event`)
/// et du corps brut.
///
/// Seules les releases effectivement publiées produisent un
/// [`WebhookEvent::Release`]; les autres actions (`created`, `edited`, `deleted`…)
/// donnent un [`WebhookEvent::Other`] afin de ne déclencher aucune vérification.
pub fn parse_event(event_name: &str, body: &[u8]) -> Result<WebhookEvent> {
    let name = event_name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return Err(Error::Invalid(
            "en-tête X-GitHub-Event absent ou vide".to_string(),
        ));
    }
    if name == "ping" {
        return Ok(WebhookEvent::Ping);
    }

    // Les autres évènements exigent une charge utile JSON exploitable.
    let payload: serde_json::Value = serde_json::from_slice(body)?;

    match name.as_str() {
        "release" => {
            let action = payload
                .get("action")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if action != "published" {
                // Édition, suppression, brouillon: rien à faire.
                return Ok(WebhookEvent::Other(format!("release:{action}")));
            }
            let (owner, repo) = repository_of(&payload)?;
            let release = payload.get("release").ok_or_else(|| {
                Error::Invalid("évènement `release` sans champ `release`".to_string())
            })?;
            let tag = release
                .get("tag_name")
                .and_then(serde_json::Value::as_str)
                .filter(|t| !t.is_empty())
                .ok_or_else(|| {
                    Error::Invalid("évènement `release` sans `release.tag_name`".to_string())
                })?
                .to_string();
            let prerelease = release
                .get("prerelease")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            Ok(WebhookEvent::Release {
                owner,
                repo,
                tag,
                prerelease,
            })
        }
        "push" => {
            let (owner, repo) = repository_of(&payload)?;
            let ref_ = payload
                .get("ref")
                .and_then(serde_json::Value::as_str)
                .filter(|r| !r.is_empty())
                .ok_or_else(|| Error::Invalid("évènement `push` sans champ `ref`".to_string()))?
                .to_string();
            Ok(WebhookEvent::Push { owner, repo, ref_ })
        }
        other => Ok(WebhookEvent::Other(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vecteur de référence documenté par GitHub.
    const SECRET: &[u8] = b"It's a Secret to Everybody";
    const BODY: &[u8] = b"Hello, World!";
    const GOOD_SIG: &str =
        "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";

    #[test]
    fn signature_valide() {
        assert!(verify_signature(SECRET, BODY, GOOD_SIG));
    }

    #[test]
    fn signature_majuscules_acceptee() {
        let upper = format!(
            "SHA256={}",
            "757107EA0EB2509FC211221CCE984B8A37570B6D7586C22C46F4379C8B043E17"
        );
        assert!(verify_signature(SECRET, BODY, &upper));
    }

    #[test]
    fn signature_espaces_autour_acceptee() {
        let padded = format!("  {GOOD_SIG}  ");
        assert!(verify_signature(SECRET, BODY, &padded));
    }

    #[test]
    fn mauvais_secret_refuse() {
        assert!(!verify_signature(b"mauvais secret", BODY, GOOD_SIG));
    }

    #[test]
    fn corps_altere_refuse() {
        assert!(!verify_signature(SECRET, b"Hello, World?", GOOD_SIG));
    }

    #[test]
    fn secret_vide_refuse() {
        // Même avec une signature « correcte » pour un secret vide, on refuse.
        let mut mac = HmacSha256::new_from_slice(b"").expect("HMAC accepte une clé vide");
        mac.update(BODY);
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(!verify_signature(b"", BODY, &format!("sha256={hex}")));
    }

    #[test]
    fn entete_tronquee_refusee() {
        let truncated = &GOOD_SIG[..GOOD_SIG.len() - 4];
        assert!(!verify_signature(SECRET, BODY, truncated));
        assert!(!verify_signature(SECRET, BODY, "sha256="));
        assert!(!verify_signature(SECRET, BODY, ""));
    }

    #[test]
    fn entete_trop_longue_refusee() {
        assert!(!verify_signature(SECRET, BODY, &format!("{GOOD_SIG}00")));
    }

    #[test]
    fn entete_non_hexadecimale_refusee() {
        let bad = "sha256=zz7107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert_eq!(bad.len(), GOOD_SIG.len());
        assert!(!verify_signature(SECRET, BODY, bad));
    }

    #[test]
    fn mauvais_prefixe_refuse() {
        let bad = "sha512=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert!(!verify_signature(SECRET, BODY, bad));
        // `sha1=` n'a pas la bonne longueur totale non plus.
        assert!(!verify_signature(SECRET, BODY, "sha1=757107ea"));
    }

    #[test]
    fn entete_multioctets_ne_panique_pas() {
        // Des caractères UTF-8 multi-octets ne doivent pas provoquer de panique.
        assert!(!verify_signature(SECRET, BODY, "sha256=é́é́é́é́é́é́é́é́"));
        assert!(!verify_signature(SECRET, BODY, "ééééééé=0000"));
    }

    #[test]
    fn decode_hex_rejette_longueur_impaire() {
        assert!(decode_hex(b"abc").is_none());
        assert_eq!(decode_hex(b"00ff"), Some(vec![0x00, 0xff]));
        assert_eq!(decode_hex(b"AB"), Some(vec![0xab]));
    }

    #[test]
    fn ping_sans_corps_json() {
        assert_eq!(parse_event("ping", b"").expect("ping"), WebhookEvent::Ping);
        assert_eq!(
            parse_event("PING", b"{}").expect("ping majuscules"),
            WebhookEvent::Ping
        );
    }

    #[test]
    fn evenement_vide_rejete() {
        assert!(parse_event("   ", b"{}").is_err());
    }

    #[test]
    fn release_publiee() {
        let body = br#"{
          "action": "published",
          "release": {
            "tag_name": "v1.4.2",
            "name": "Release 1.4.2",
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/grafana/grafana/releases/tag/v1.4.2"
          },
          "repository": {
            "name": "grafana",
            "full_name": "grafana/grafana",
            "owner": { "login": "grafana", "id": 7195757, "type": "Organization" }
          },
          "sender": { "login": "octocat" }
        }"#;
        let ev = parse_event("release", body).expect("release analysable");
        assert_eq!(
            ev,
            WebhookEvent::Release {
                owner: "grafana".to_string(),
                repo: "grafana".to_string(),
                tag: "v1.4.2".to_string(),
                prerelease: false,
            }
        );
    }

    #[test]
    fn release_prerelease_marquee() {
        let body = br#"{
          "action": "published",
          "release": { "tag_name": "v2.0.0-rc.1", "prerelease": true },
          "repository": { "name": "traefik", "owner": { "login": "traefik" } }
        }"#;
        match parse_event("release", body).expect("release analysable") {
            WebhookEvent::Release {
                tag, prerelease, ..
            } => {
                assert_eq!(tag, "v2.0.0-rc.1");
                assert!(prerelease);
            }
            other => panic!("attendu une release, obtenu {other:?}"),
        }
    }

    #[test]
    fn release_non_publiee_ignoree() {
        let body = br#"{
          "action": "edited",
          "release": { "tag_name": "v1.4.2", "prerelease": false },
          "repository": { "name": "grafana", "owner": { "login": "grafana" } }
        }"#;
        assert_eq!(
            parse_event("release", body).expect("analysable"),
            WebhookEvent::Other("release:edited".to_string())
        );
    }

    #[test]
    fn release_sans_tag_rejetee() {
        let body = br#"{
          "action": "published",
          "release": { "prerelease": false },
          "repository": { "name": "grafana", "owner": { "login": "grafana" } }
        }"#;
        assert!(parse_event("release", body).is_err());
    }

    #[test]
    fn push_sur_une_branche() {
        let body = br#"{
          "ref": "refs/heads/main",
          "before": "0000000000000000000000000000000000000000",
          "after": "6113728f27ae82c7b1a177c8d03f9e96e0adf246",
          "repository": {
            "name": "kubewatch",
            "full_name": "kubewatch-io/kubewatch",
            "owner": { "name": "kubewatch-io", "email": "dev@example.org" }
          },
          "pusher": { "name": "octocat" }
        }"#;
        assert_eq!(
            parse_event("push", body).expect("push analysable"),
            WebhookEvent::Push {
                owner: "kubewatch-io".to_string(),
                repo: "kubewatch".to_string(),
                ref_: "refs/heads/main".to_string(),
            }
        );
    }

    #[test]
    fn push_repli_sur_full_name() {
        let body = br#"{
          "ref": "refs/tags/v3.1.0",
          "repository": { "full_name": "go-gitea/gitea" }
        }"#;
        assert_eq!(
            parse_event("push", body).expect("push analysable"),
            WebhookEvent::Push {
                owner: "go-gitea".to_string(),
                repo: "gitea".to_string(),
                ref_: "refs/tags/v3.1.0".to_string(),
            }
        );
    }

    #[test]
    fn evenement_inconnu_conserve_son_nom() {
        assert_eq!(
            parse_event("workflow_run", b"{}").expect("analysable"),
            WebhookEvent::Other("workflow_run".to_string())
        );
    }

    #[test]
    fn corps_non_json_rejete() {
        assert!(parse_event("push", b"ceci n'est pas du JSON").is_err());
    }
}
