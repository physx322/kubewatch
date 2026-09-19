//! Application de la politique de mise à jour : choix de la version cible,
//! qualification du saut de version et fenêtre de maintenance.

use crate::github::normalize_version;
use crate::model::{UpdateChannel, UpdatePolicy, UpdateSeverity};
use chrono::{DateTime, Timelike, Utc};
use semver::{Version, VersionReq};

/// Choisit la meilleure version applicable parmi `candidates`.
///
/// Les filtres sont appliqués dans cet ordre :
/// 1. canal `Pinned` → aucune mise à jour ;
/// 2. liste `ignore` (comparaison sur la chaîne du tag **et** sur la version) ;
/// 3. contrainte semver éventuelle ;
/// 4. pré-versions, sauf si elles sont autorisées ;
/// 5. amplitude du saut autorisée par le canal.
///
/// Renvoie le **maximum** des candidats retenus, ou `None` si aucun n'est
/// strictement supérieur à la version courante.
pub fn select_update(
    current: Option<&Version>,
    candidates: &[Version],
    p: &UpdatePolicy,
) -> Option<Version> {
    if p.channel == UpdateChannel::Pinned {
        return None;
    }

    let req: Option<VersionReq> = match p.constraint.as_deref().map(str::trim) {
        Some(c) if !c.is_empty() => match VersionReq::parse(c) {
            Ok(r) => Some(r),
            Err(e) => {
                // Une contrainte malformée ne doit pas bloquer silencieusement :
                // on l'ignore en le signalant plutôt que de refuser toute mise à jour.
                tracing::warn!(contrainte = %c, erreur = %e, "contrainte semver invalide, ignorée");
                None
            }
        },
        _ => None,
    };

    let prerelease_ok = p.prerelease_allowed();

    candidates
        .iter()
        .filter(|v| !is_ignored(v, &p.ignore))
        .filter(|v| req.as_ref().map(|r| r.matches(v)).unwrap_or(true))
        .filter(|v| prerelease_ok || v.pre.is_empty())
        .filter(|v| channel_allows(p.channel, current, v))
        .max()
        .cloned()
}

/// Indique si la version est explicitement exclue par la politique.
///
/// La comparaison porte à la fois sur la chaîne brute (le tag tel qu'écrit par
/// l'utilisateur, ex. `"v1.2.3"`) et sur la version normalisée.
fn is_ignored(v: &Version, ignore: &[String]) -> bool {
    let rendu = v.to_string();
    ignore.iter().any(|raw| {
        let raw = raw.trim();
        if raw.is_empty() {
            return false;
        }
        if raw == rendu {
            return true;
        }
        normalize_version(raw, None)
            .map(|iv| &iv == v)
            .unwrap_or(false)
    })
}

/// Vérifie que le saut `current -> candidate` reste dans le canal autorisé.
fn channel_allows(channel: UpdateChannel, current: Option<&Version>, candidate: &Version) -> bool {
    let cur = match current {
        // Sans version courante connue, seuls les filtres précédents s'appliquent :
        // on accepte tout candidat et on renverra le maximum.
        None => return channel != UpdateChannel::Pinned,
        Some(c) => c,
    };

    if candidate <= cur {
        return false;
    }

    match channel {
        UpdateChannel::Pinned => false,
        // Même majeure et même mineure : seul le correctif progresse.
        UpdateChannel::Patch => candidate.major == cur.major && candidate.minor == cur.minor,
        // Même majeure : (mineure, correctif) progresse.
        UpdateChannel::Minor => candidate.major == cur.major,
        // Tout saut strictement supérieur.
        UpdateChannel::Major | UpdateChannel::Prerelease => true,
    }
}

/// Qualifie l'ampleur du saut de version.
pub fn severity(current: Option<&Version>, next: &Version) -> UpdateSeverity {
    let cur = match current {
        Some(c) => c,
        None => return UpdateSeverity::Unknown,
    };
    if next.major > cur.major {
        UpdateSeverity::Major
    } else if next.major == cur.major && next.minor > cur.minor {
        UpdateSeverity::Minor
    } else if next.major == cur.major && next.minor == cur.minor && next.patch > cur.patch {
        UpdateSeverity::Patch
    } else if next > cur {
        // Progression uniquement sur la pré-version (ex. 1.2.3-rc.1 -> 1.2.3-rc.2).
        UpdateSeverity::Patch
    } else {
        UpdateSeverity::Unknown
    }
}

/// Indique si l'instant `now` (UTC) tombe dans la fenêtre de maintenance.
///
/// Format attendu : `"HH:MM-HH:MM"`. Une fenêtre à cheval sur minuit
/// (`"22:00-04:00"`) est gérée. Absence de fenêtre ou format invalide →
/// `true` : une configuration malformée ne doit jamais bloquer les mises à jour.
pub fn in_maintenance_window(window: Option<&str>, now: DateTime<Utc>) -> bool {
    let raw = match window.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => w,
        None => return true,
    };

    let (start, end) = match parse_window(raw) {
        Some(w) => w,
        None => {
            tracing::warn!(fenetre = %raw, "fenêtre de maintenance illisible, ignorée");
            return true;
        }
    };

    if start == end {
        // Fenêtre dégénérée : on considère la journée entière.
        return true;
    }

    let minutes = now.hour() * 60 + now.minute();
    if start < end {
        minutes >= start && minutes <= end
    } else {
        // Fenêtre à cheval sur minuit.
        minutes >= start || minutes <= end
    }
}

/// Analyse `"HH:MM-HH:MM"` en minutes depuis minuit.
fn parse_window(raw: &str) -> Option<(u32, u32)> {
    let (a, b) = raw.split_once('-')?;
    Some((parse_hhmm(a)?, parse_hhmm(b)?))
}

/// Analyse `"HH:MM"` en minutes depuis minuit.
fn parse_hhmm(raw: &str) -> Option<u32> {
    let raw = raw.trim();
    let (h, m) = raw.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn v(s: &str) -> Version {
        Version::parse(s).expect("version de test valide")
    }

    fn vs(list: &[&str]) -> Vec<Version> {
        list.iter().map(|s| v(s)).collect()
    }

    fn policy(channel: UpdateChannel) -> UpdatePolicy {
        UpdatePolicy {
            channel,
            ..Default::default()
        }
    }

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 18, h, m, 0)
            .single()
            .expect("instant de test valide")
    }

    // --- canal Pinned -------------------------------------------------------

    #[test]
    fn pinned_ne_propose_jamais_rien() {
        let c = vs(&["1.0.1", "2.0.0", "1.1.0"]);
        assert_eq!(
            select_update(Some(&v("1.0.0")), &c, &policy(UpdateChannel::Pinned)),
            None
        );
        // Même sans version courante.
        assert_eq!(
            select_update(None, &c, &policy(UpdateChannel::Pinned)),
            None
        );
    }

    // --- canal Patch --------------------------------------------------------

    #[test]
    fn patch_reste_sur_la_meme_mineure() {
        let c = vs(&["1.2.1", "1.2.5", "1.3.0", "2.0.0"]);
        let got = select_update(Some(&v("1.2.3")), &c, &policy(UpdateChannel::Patch));
        assert_eq!(got, Some(v("1.2.5")));
    }

    #[test]
    fn patch_ignore_les_versions_inferieures() {
        let c = vs(&["1.2.0", "1.2.1", "1.2.2"]);
        assert_eq!(
            select_update(Some(&v("1.2.3")), &c, &policy(UpdateChannel::Patch)),
            None
        );
    }

    #[test]
    fn patch_ignore_la_version_identique() {
        let c = vs(&["1.2.3"]);
        assert_eq!(
            select_update(Some(&v("1.2.3")), &c, &policy(UpdateChannel::Patch)),
            None
        );
    }

    // --- canal Minor --------------------------------------------------------

    #[test]
    fn minor_reste_sur_la_meme_majeure() {
        let c = vs(&["1.2.5", "1.9.0", "1.10.2", "2.0.0"]);
        let got = select_update(Some(&v("1.2.3")), &c, &policy(UpdateChannel::Minor));
        assert_eq!(got, Some(v("1.10.2")));
    }

    #[test]
    fn minor_accepte_aussi_un_correctif() {
        let c = vs(&["1.2.4"]);
        assert_eq!(
            select_update(Some(&v("1.2.3")), &c, &policy(UpdateChannel::Minor)),
            Some(v("1.2.4"))
        );
    }

    // --- canal Major --------------------------------------------------------

    #[test]
    fn major_accepte_tout_saut_superieur() {
        let c = vs(&["1.2.4", "1.9.0", "3.1.0"]);
        let got = select_update(Some(&v("1.2.3")), &c, &policy(UpdateChannel::Major));
        assert_eq!(got, Some(v("3.1.0")));
    }

    #[test]
    fn major_ecarte_les_preversions_par_defaut() {
        let c = vs(&["2.0.0", "3.0.0-rc.1"]);
        let got = select_update(Some(&v("1.0.0")), &c, &policy(UpdateChannel::Major));
        assert_eq!(got, Some(v("2.0.0")));
    }

    // --- canal Prerelease ---------------------------------------------------

    #[test]
    fn prerelease_accepte_les_preversions() {
        let c = vs(&["2.0.0", "3.0.0-rc.1"]);
        let got = select_update(Some(&v("1.0.0")), &c, &policy(UpdateChannel::Prerelease));
        assert_eq!(got, Some(v("3.0.0-rc.1")));
    }

    #[test]
    fn prerelease_progresse_entre_deux_rc() {
        let c = vs(&["2.0.0-rc.2", "2.0.0-rc.1"]);
        let got = select_update(
            Some(&v("2.0.0-rc.1")),
            &c,
            &policy(UpdateChannel::Prerelease),
        );
        assert_eq!(got, Some(v("2.0.0-rc.2")));
    }

    // --- allow_prerelease ---------------------------------------------------

    #[test]
    fn allow_prerelease_ouvre_les_preversions_sur_minor() {
        let c = vs(&["1.3.0", "1.4.0-beta.1"]);
        let stricte = UpdatePolicy {
            channel: UpdateChannel::Minor,
            ..Default::default()
        };
        assert_eq!(
            select_update(Some(&v("1.2.0")), &c, &stricte),
            Some(v("1.3.0"))
        );
        let souple = UpdatePolicy {
            allow_prerelease: true,
            ..stricte
        };
        assert_eq!(
            select_update(Some(&v("1.2.0")), &c, &souple),
            Some(v("1.4.0-beta.1"))
        );
    }

    // --- current = None -----------------------------------------------------

    #[test]
    fn sans_version_courante_on_prend_le_maximum_autorise() {
        let c = vs(&["1.2.3", "2.0.0", "1.9.9"]);
        assert_eq!(
            select_update(None, &c, &policy(UpdateChannel::Patch)),
            Some(v("2.0.0"))
        );
        assert_eq!(
            select_update(None, &c, &policy(UpdateChannel::Major)),
            Some(v("2.0.0"))
        );
    }

    #[test]
    fn sans_version_courante_les_preversions_restent_filtrees() {
        let c = vs(&["1.0.0", "2.0.0-rc.1"]);
        assert_eq!(
            select_update(None, &c, &policy(UpdateChannel::Minor)),
            Some(v("1.0.0"))
        );
        assert_eq!(
            select_update(None, &c, &policy(UpdateChannel::Prerelease)),
            Some(v("2.0.0-rc.1"))
        );
    }

    #[test]
    fn liste_vide() {
        assert_eq!(
            select_update(Some(&v("1.0.0")), &[], &policy(UpdateChannel::Major)),
            None
        );
        assert_eq!(
            select_update(None, &[], &policy(UpdateChannel::Major)),
            None
        );
    }

    // --- ignore -------------------------------------------------------------

    #[test]
    fn ignore_sur_la_version_normalisee() {
        let c = vs(&["1.2.4", "1.2.5"]);
        let p = UpdatePolicy {
            channel: UpdateChannel::Patch,
            ignore: vec!["1.2.5".into()],
            ..Default::default()
        };
        assert_eq!(select_update(Some(&v("1.2.3")), &c, &p), Some(v("1.2.4")));
    }

    #[test]
    fn ignore_sur_la_chaine_du_tag() {
        let c = vs(&["1.2.4", "1.2.5"]);
        // L'utilisateur a saisi le tag tel qu'il apparaît en amont.
        let p = UpdatePolicy {
            channel: UpdateChannel::Patch,
            ignore: vec!["v1.2.5".into()],
            ..Default::default()
        };
        assert_eq!(select_update(Some(&v("1.2.3")), &c, &p), Some(v("1.2.4")));
    }

    #[test]
    fn ignore_peut_tout_exclure() {
        let c = vs(&["1.2.4", "1.2.5"]);
        let p = UpdatePolicy {
            channel: UpdateChannel::Patch,
            ignore: vec!["v1.2.5".into(), "1.2.4".into(), "  ".into()],
            ..Default::default()
        };
        assert_eq!(select_update(Some(&v("1.2.3")), &c, &p), None);
    }

    // --- constraint ---------------------------------------------------------

    #[test]
    fn contrainte_semver_filtre() {
        let c = vs(&["1.2.4", "1.5.0", "2.0.0"]);
        let p = UpdatePolicy {
            channel: UpdateChannel::Major,
            constraint: Some(">=1.2, <2".into()),
            ..Default::default()
        };
        assert_eq!(select_update(Some(&v("1.2.3")), &c, &p), Some(v("1.5.0")));
    }

    #[test]
    fn contrainte_caret() {
        let c = vs(&["1.2.4", "2.0.0"]);
        let p = UpdatePolicy {
            channel: UpdateChannel::Major,
            constraint: Some("^1".into()),
            ..Default::default()
        };
        assert_eq!(select_update(Some(&v("1.2.3")), &c, &p), Some(v("1.2.4")));
    }

    #[test]
    fn contrainte_invalide_est_ignoree_et_ne_bloque_pas() {
        let c = vs(&["1.2.4"]);
        let p = UpdatePolicy {
            channel: UpdateChannel::Patch,
            constraint: Some("pas une contrainte".into()),
            ..Default::default()
        };
        assert_eq!(select_update(Some(&v("1.2.3")), &c, &p), Some(v("1.2.4")));
    }

    #[test]
    fn contrainte_vide_est_ignoree() {
        let c = vs(&["1.2.4"]);
        let p = UpdatePolicy {
            channel: UpdateChannel::Patch,
            constraint: Some("   ".into()),
            ..Default::default()
        };
        assert_eq!(select_update(Some(&v("1.2.3")), &c, &p), Some(v("1.2.4")));
    }

    // --- matrice combinée ---------------------------------------------------

    #[test]
    fn matrice_des_canaux() {
        let current = v("1.2.3");
        let c = vs(&["1.2.4", "1.3.0", "2.0.0", "2.1.0-rc.1"]);
        let cas = [
            (UpdateChannel::Patch, Some("1.2.4")),
            (UpdateChannel::Minor, Some("1.3.0")),
            (UpdateChannel::Major, Some("2.0.0")),
            (UpdateChannel::Prerelease, Some("2.1.0-rc.1")),
            (UpdateChannel::Pinned, None),
        ];
        for (channel, attendu) in cas {
            let got = select_update(Some(&current), &c, &policy(channel));
            assert_eq!(
                got.as_ref().map(|x| x.to_string()),
                attendu.map(|s| s.to_string()),
                "canal {channel:?}"
            );
        }
    }

    // --- severity -----------------------------------------------------------

    #[test]
    fn severite_majeure_mineure_correctif() {
        let cur = v("1.2.3");
        assert_eq!(severity(Some(&cur), &v("2.0.0")), UpdateSeverity::Major);
        assert_eq!(severity(Some(&cur), &v("1.3.0")), UpdateSeverity::Minor);
        assert_eq!(severity(Some(&cur), &v("1.2.4")), UpdateSeverity::Patch);
    }

    #[test]
    fn severite_inconnue_sans_version_courante() {
        assert_eq!(severity(None, &v("1.2.3")), UpdateSeverity::Unknown);
    }

    #[test]
    fn severite_inconnue_sur_regression() {
        let cur = v("2.0.0");
        assert_eq!(severity(Some(&cur), &v("1.9.0")), UpdateSeverity::Unknown);
        assert_eq!(severity(Some(&cur), &v("2.0.0")), UpdateSeverity::Unknown);
    }

    #[test]
    fn severite_entre_preversions() {
        let cur = v("2.0.0-rc.1");
        assert_eq!(
            severity(Some(&cur), &v("2.0.0-rc.2")),
            UpdateSeverity::Patch
        );
        assert_eq!(severity(Some(&cur), &v("2.0.0")), UpdateSeverity::Patch);
    }

    // --- fenêtre de maintenance ---------------------------------------------

    #[test]
    fn fenetre_absente_toujours_ouverte() {
        assert!(in_maintenance_window(None, at(13, 0)));
        assert!(in_maintenance_window(Some(""), at(13, 0)));
        assert!(in_maintenance_window(Some("   "), at(13, 0)));
    }

    #[test]
    fn fenetre_simple_dans_la_journee() {
        let f = Some("02:00-04:00");
        assert!(in_maintenance_window(f, at(2, 0)));
        assert!(in_maintenance_window(f, at(3, 30)));
        assert!(in_maintenance_window(f, at(4, 0)));
        assert!(!in_maintenance_window(f, at(1, 59)));
        assert!(!in_maintenance_window(f, at(4, 1)));
        assert!(!in_maintenance_window(f, at(13, 0)));
    }

    #[test]
    fn fenetre_a_cheval_sur_minuit() {
        let f = Some("22:00-04:00");
        assert!(in_maintenance_window(f, at(22, 0)));
        assert!(in_maintenance_window(f, at(23, 59)));
        assert!(in_maintenance_window(f, at(0, 0)));
        assert!(in_maintenance_window(f, at(3, 59)));
        assert!(in_maintenance_window(f, at(4, 0)));
        assert!(!in_maintenance_window(f, at(4, 1)));
        assert!(!in_maintenance_window(f, at(12, 0)));
        assert!(!in_maintenance_window(f, at(21, 59)));
    }

    #[test]
    fn fenetre_degeneree_est_ouverte() {
        assert!(in_maintenance_window(Some("03:00-03:00"), at(15, 0)));
    }

    #[test]
    fn fenetre_invalide_ne_bloque_pas_et_ne_panique_pas() {
        for f in [
            "abc",
            "25:00-26:00",
            "02:00",
            "-",
            "02:00-",
            "-04:00",
            "02:60-04:00",
            "2h-4h",
            "02:00-04:00-06:00",
            "::",
            "999999999999999999999:00-01:00",
        ] {
            assert!(in_maintenance_window(Some(f), at(13, 0)), "fenêtre: {f}");
        }
    }

    #[test]
    fn fenetre_tolere_les_espaces() {
        assert!(in_maintenance_window(Some(" 02:00 - 04:00 "), at(3, 0)));
        assert!(!in_maintenance_window(Some(" 02:00 - 04:00 "), at(5, 0)));
    }
}
