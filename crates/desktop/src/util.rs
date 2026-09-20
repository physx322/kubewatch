//! Utilitaires : dossier d'état, fournisseur cryptographique.

use std::path::PathBuf;

/// Dossier d'état de KubeWatch : `<data_dir>/kubewatch`, avec repli local.
///
/// `KUBEWATCH_STATE_DIR` le remplace, pour isoler un profil de test. C'est le
/// dossier des versions précédentes : les clusters enregistrés et l'état du
/// moteur de mise à jour d'une installation existante sont repris tels quels.
pub fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("KUBEWATCH_STATE_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Some(dir) = dirs::data_dir() {
        return dir.join("kubewatch");
    }
    if let Some(dir) = dirs::home_dir() {
        return dir.join(".kubewatch");
    }
    PathBuf::from(".kubewatch")
}

/// Installe le fournisseur rustls `ring` comme fournisseur par défaut.
///
/// Le binaire contient deux fournisseurs (`ring` via `kube`, `aws-lc-rs` via
/// `reqwest`) ; sans choix explicite, la première connexion TLS panique.
pub fn install_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// Raccourcit un texte, en gardant la fin quand `keep_tail` est vrai.
pub fn clip(text: &str, max_chars: usize, keep_tail: bool) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    if keep_tail {
        let skipped = count - max_chars;
        let tail: String = text.chars().skip(skipped).collect();
        format!("[… {skipped} caractères omis au début …]\n{tail}")
    } else {
        let head: String = text.chars().take(max_chars).collect();
        format!("{head}\n[… {} caractères omis …]", count - max_chars)
    }
}

/// Âge lisible : `3d4h`, `2h15m`, `48m`, `20s`.
pub fn humanize_age(seconds: i64) -> String {
    let s = seconds.max(0);
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3_600, (s % 3_600) / 60);
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        assert_eq!(humanize_age(20), "20s");
        assert_eq!(humanize_age(125), "2m");
        assert_eq!(humanize_age(3_600 * 2 + 900), "2h15m");
        assert_eq!(humanize_age(86_400 * 3 + 3_600 * 4), "3d4h");
        assert_eq!(humanize_age(-5), "0s");
    }

    #[test]
    fn coupe_tete_ou_queue() {
        assert_eq!(clip("abc", 5, false), "abc");
        assert!(clip("abcdef", 3, false).starts_with("abc\n"));
        assert!(clip("abcdef", 3, true).ends_with("\ndef"));
    }
}
