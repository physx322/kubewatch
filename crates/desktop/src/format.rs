//! Formatage des valeurs affichées à l'écran : âges, tailles, CPU, pourcentages,
//! dates et troncature.
//!
//! Toutes les fonctions de ce module sont pures et ne paniquent jamais : elles
//! sont appelées à chaque image de rendu, sur des données venues du serveur
//! d'API (donc potentiellement absurdes : horloges décalées, quantités
//! négatives, compteurs énormes). Le pire cas produit un tiret cadratin.
//!
//! Ce module est une boîte à outils complète et testée : toutes ses fonctions
//! ne sont pas encore appelées par une vue (les tailles et les pourcentages
//! n'intéressent que les écrans de mesures), d'où la tolérance au code non
//! utilisé — les retirer reviendrait à retirer une unité déjà vérifiée.
#![allow(dead_code)]

use chrono::{DateTime, Local, Utc};

/// Nombre de secondes dans une minute.
const MINUTE: i64 = 60;
/// Nombre de secondes dans une heure.
const HOUR: i64 = 60 * MINUTE;
/// Nombre de secondes dans un jour.
const DAY: i64 = 24 * HOUR;
/// Nombre de secondes dans une année (365 jours, comme `kubectl`).
const YEAR: i64 = 365 * DAY;

/// Valeur affichée quand la donnée est absente ou inexploitable.
pub const UNKNOWN: &str = "—";

/// Formate un âge en secondes à la manière de `kubectl`.
///
/// Exemples : `10s`, `5m`, `3h20m`, `12d`, `2y64d`.
///
/// Les âges négatifs (horloge du poste en avance sur celle du cluster) sont
/// ramenés à `0s` plutôt que d'afficher une valeur absurde.
pub fn age(seconds: i64) -> String {
    let total = seconds.max(0);

    if total < MINUTE {
        return format!("{total}s");
    }

    if total < HOUR {
        let minutes = total / MINUTE;
        let rest = total % MINUTE;
        // En dessous de dix minutes, la seconde reste une information utile.
        return if minutes < 10 && rest > 0 {
            format!("{minutes}m{rest}s")
        } else {
            format!("{minutes}m")
        };
    }

    if total < DAY {
        let hours = total / HOUR;
        let minutes = (total % HOUR) / MINUTE;
        return if hours < 8 && minutes > 0 {
            format!("{hours}h{minutes}m")
        } else {
            format!("{hours}h")
        };
    }

    if total < YEAR {
        let days = total / DAY;
        let hours = (total % DAY) / HOUR;
        return if days < 8 && hours > 0 {
            format!("{days}d{hours}h")
        } else {
            format!("{days}d")
        };
    }

    let years = total / YEAR;
    let days = (total % YEAR) / DAY;
    if days > 0 {
        format!("{years}y{days}d")
    } else {
        format!("{years}y")
    }
}

/// Formate l'âge écoulé depuis un horodatage, ou `—` s'il est absent.
pub fn age_since(created: Option<DateTime<Utc>>) -> String {
    match created {
        None => UNKNOWN.to_string(),
        Some(t) => age((Utc::now() - t).num_seconds()),
    }
}

/// Formate un nombre d'octets avec les préfixes binaires de Kubernetes.
///
/// Exemples : `512o`, `1.5Ki`, `64Mi`, `3.2Gi`, `-2Ki`.
pub fn bytes(n: i64) -> String {
    const UNITS: [&str; 6] = ["Ki", "Mi", "Gi", "Ti", "Pi", "Ei"];

    let sign = if n < 0 { "-" } else { "" };
    // Passage par i128 : `i64::MIN.abs()` déborderait.
    let magnitude = (n as i128).unsigned_abs();

    if magnitude < 1024 {
        return format!("{sign}{magnitude}o");
    }

    let mut value = magnitude as f64 / 1024.0;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    // Évite un affichage du type « 1024Ki » quand l'arrondi franchit le palier.
    if value >= 1023.95 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    let text = if value < 10.0 {
        let rounded = (value * 10.0).round() / 10.0;
        if rounded.fract().abs() < 1e-9 {
            format!("{}", rounded as i64)
        } else {
            format!("{rounded:.1}")
        }
    } else {
        format!("{}", value.round() as i64)
    };

    format!("{sign}{text}{}", UNITS[unit])
}

/// Formate une consommation CPU exprimée en millicores.
///
/// En dessous d'un cœur on reste en millicores (`250m`) ; au-delà on affiche
/// des cœurs avec au plus deux décimales (`1.5`, `2`, `12.25`).
pub fn cpu(millis: f64) -> String {
    if !millis.is_finite() {
        return UNKNOWN.to_string();
    }

    let millis = millis.max(0.0);
    if millis < 1000.0 {
        return format!("{}m", millis.round() as i64);
    }

    let cores = millis / 1000.0;
    let rounded = (cores * 100.0).round() / 100.0;
    if rounded.fract().abs() < 1e-9 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

/// Formate un ratio en pourcentage, avec une décimale sous 10 %.
///
/// Renvoie `—` si le total est nul, négatif ou non fini : c'est le cas quand
/// `metrics.k8s.io` n'est pas installé sur le cluster.
pub fn percent(used: f64, total: f64) -> String {
    if !used.is_finite() || !total.is_finite() || total <= 0.0 {
        return UNKNOWN.to_string();
    }

    let ratio = (used / total * 100.0).clamp(0.0, 9999.0);
    if ratio >= 10.0 {
        format!("{ratio:.0} %")
    } else {
        format!("{ratio:.1} %")
    }
}

/// Tronque une chaîne à `max` caractères, points de suspension compris.
///
/// La coupe se fait sur une frontière de caractère : un nom contenant des
/// accents ou des idéogrammes ne peut pas produire de chaîne invalide.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }

    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Formate un horodatage dans le fuseau local, ou `—` s'il est absent.
pub fn timestamp(t: Option<DateTime<Utc>>) -> String {
    match t {
        None => UNKNOWN.to_string(),
        Some(t) => t
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
    }
}

/// Formate une quantité d'éléments avec son libellé accordé.
///
/// Exemple : `1 pod`, `3 pods`.
pub fn plural(count: usize, singular: &str) -> String {
    if count > 1 {
        format!("{count} {singular}s")
    } else {
        format!("{count} {singular}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_secondes() {
        assert_eq!(age(0), "0s");
        assert_eq!(age(1), "1s");
        assert_eq!(age(10), "10s");
        assert_eq!(age(59), "59s");
    }

    #[test]
    fn age_negatif_est_ramene_a_zero() {
        assert_eq!(age(-1), "0s");
        assert_eq!(age(-86_400), "0s");
        assert_eq!(age(i64::MIN), "0s");
    }

    #[test]
    fn age_minutes() {
        assert_eq!(age(60), "1m");
        assert_eq!(age(65), "1m5s");
        assert_eq!(age(300), "5m");
        assert_eq!(age(305), "5m5s");
        // Au-delà de dix minutes, on abandonne les secondes.
        assert_eq!(age(632), "10m");
        assert_eq!(age(3599), "59m");
    }

    #[test]
    fn age_heures() {
        assert_eq!(age(3600), "1h");
        assert_eq!(age(12_000), "3h20m");
        // Au-delà de huit heures, on abandonne les minutes.
        assert_eq!(age(8 * 3600 + 1800), "8h");
        assert_eq!(age(86_399), "23h");
    }

    #[test]
    fn age_jours() {
        assert_eq!(age(86_400), "1d");
        assert_eq!(age(86_400 + 7200), "1d2h");
        // Au-delà de huit jours, on abandonne les heures.
        assert_eq!(age(12 * 86_400 + 3600), "12d");
        assert_eq!(age(364 * 86_400), "364d");
    }

    #[test]
    fn age_annees() {
        assert_eq!(age(365 * 86_400), "1y");
        assert_eq!(age(2 * 365 * 86_400 + 64 * 86_400), "2y64d");
        // Très grand : aucune multiplication, donc aucun débordement.
        let enorme = age(i64::MAX);
        assert!(enorme.ends_with('y') || enorme.contains('y'));
    }

    #[test]
    fn bytes_octets() {
        assert_eq!(bytes(0), "0o");
        assert_eq!(bytes(1), "1o");
        assert_eq!(bytes(512), "512o");
        assert_eq!(bytes(1023), "1023o");
    }

    #[test]
    fn bytes_paliers_binaires() {
        assert_eq!(bytes(1024), "1Ki");
        assert_eq!(bytes(1536), "1.5Ki");
        assert_eq!(bytes(1024 * 1024), "1Mi");
        assert_eq!(bytes(64 * 1024 * 1024), "64Mi");
        assert_eq!(bytes(1024 * 1024 * 1024), "1Gi");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024 + 200 * 1024 * 1024), "3.2Gi");
        assert_eq!(bytes(1024_i64.pow(4)), "1Ti");
        assert_eq!(bytes(1024_i64.pow(5)), "1Pi");
        assert_eq!(bytes(1024_i64.pow(6)), "1Ei");
    }

    #[test]
    fn bytes_negatifs() {
        assert_eq!(bytes(-1), "-1o");
        assert_eq!(bytes(-2048), "-2Ki");
        // Le cas qui déborde si l'on prend la valeur absolue en i64.
        assert_eq!(bytes(i64::MIN), "-8Ei");
    }

    #[test]
    fn bytes_tres_grand() {
        assert_eq!(bytes(i64::MAX), "8Ei");
        // Pas de saut d'unité fantaisiste juste sous un palier.
        assert_eq!(bytes(1024 * 1024 - 1), "1Mi");
        assert_eq!(bytes(1024 * 1024 - 1024 * 100), "924Ki");
    }

    #[test]
    fn cpu_millicores_et_coeurs() {
        assert_eq!(cpu(0.0), "0m");
        assert_eq!(cpu(250.0), "250m");
        assert_eq!(cpu(999.4), "999m");
        assert_eq!(cpu(1000.0), "1");
        assert_eq!(cpu(1500.0), "1.5");
        assert_eq!(cpu(12_250.0), "12.25");
        assert_eq!(cpu(-5.0), "0m");
        assert_eq!(cpu(f64::NAN), UNKNOWN);
    }

    #[test]
    fn percent_borne_et_absent() {
        assert_eq!(percent(50.0, 200.0), "25 %");
        assert_eq!(percent(1.0, 200.0), "0.5 %");
        assert_eq!(percent(0.0, 0.0), UNKNOWN);
        assert_eq!(percent(1.0, -1.0), UNKNOWN);
        assert_eq!(percent(f64::INFINITY, 1.0), UNKNOWN);
    }

    #[test]
    fn truncate_sur_frontiere_de_caractere() {
        assert_eq!(truncate("kube-system", 32), "kube-system");
        assert_eq!(truncate("kube-system", 6), "kube-…");
        assert_eq!(truncate("", 4), "");
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(truncate("abcdef", 1), "…");
        // Caractères multi-octets : la chaîne produite reste valide.
        let accents = truncate("déploiement-éphémère", 5);
        assert_eq!(accents.chars().count(), 5);
        assert!(accents.ends_with('…'));
    }

    #[test]
    fn timestamp_absent() {
        assert_eq!(timestamp(None), UNKNOWN);
        let t = DateTime::<Utc>::from_timestamp(0, 0).expect("horodatage valide");
        // Le fuseau local dépend de la machine : on vérifie la forme, pas la valeur.
        assert_eq!(timestamp(Some(t)).len(), "2026-09-18 11:22:33".len());
    }

    #[test]
    fn plural_accorde_le_libelle() {
        assert_eq!(plural(0, "pod"), "0 pod");
        assert_eq!(plural(1, "pod"), "1 pod");
        assert_eq!(plural(3, "pod"), "3 pods");
    }
}
