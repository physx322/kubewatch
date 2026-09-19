//! Icônes de l'interface.
//!
//! La police Phosphor (crate `egui-phosphor`, variante *Regular*) est ajoutée
//! au démarrage en repli de la famille proportionnelle : un glyphe d'icône se
//! mêle alors à n'importe quel texte — bouton, libellé, titre — et prend la
//! taille et la couleur du texte qui l'entoure. La police est embarquée dans le
//! binaire, aucun fichier n'est lu sur le disque.
//!
//! Les vues n'emploient jamais un nom Phosphor directement : elles passent par
//! les constantes ci-dessous, nommées par leur **rôle**. Changer d'icône pour
//! un rôle se fait donc en un seul endroit, et deux rôles voisins peuvent
//! partager un glyphe sans que le code des vues le sache.

use egui_phosphor::regular as ph;

/// Ajoute la police d'icônes aux définitions de polices d'egui.
pub fn install(fonts: &mut egui::FontDefinitions) {
    egui_phosphor::add_to_fonts(fonts, egui_phosphor::Variant::Regular);
}

// --- Navigation --------------------------------------------------------------

/// Vue d'ensemble : une jauge.
pub const OVERVIEW: &str = ph::GAUGE;
/// Ressources : un tableau.
pub const RESOURCES: &str = ph::TABLE;
/// Topologie : un graphe de nœuds.
pub const GRAPH: &str = ph::GRAPH;
/// Console YAML : des accolades.
pub const YAML: &str = ph::BRACKETS_CURLY;
/// Déployer : une fusée.
pub const DEPLOY: &str = ph::ROCKET_LAUNCH;
/// Catalogue : une boutique.
pub const HUB: &str = ph::STOREFRONT;
/// Mises à jour : une flèche montante cerclée.
pub const UPDATES: &str = ph::ARROW_CIRCLE_UP;
/// Réglages : un engrenage.
pub const SETTINGS: &str = ph::GEAR_SIX;

// --- Actions -----------------------------------------------------------------

/// Rafraîchir, recharger, chargement en cours.
pub const REFRESH: &str = ph::ARROWS_CLOCKWISE;
/// Fermer un panneau, retirer une ligne, effacer un filtre.
pub const CLOSE: &str = ph::X;
/// Passer au thème clair.
pub const SUN: &str = ph::SUN;
/// Passer au thème sombre.
pub const MOON: &str = ph::MOON;
/// Revenir à l'étape précédente.
pub const BACK: &str = ph::ARROW_LEFT;
/// Tri croissant.
pub const SORT_ASC: &str = ph::CARET_UP;
/// Tri décroissant.
pub const SORT_DESC: &str = ph::CARET_DOWN;
/// Élément courant dans une liste.
pub const CURRENT: &str = ph::CARET_RIGHT;
/// Étoiles d'une image ou d'un chart.
pub const STAR: &str = ph::STAR;
/// Téléchargements d'une image.
pub const DOWNLOADS: &str = ph::DOWNLOAD_SIMPLE;
/// Cadrer tout le graphe.
pub const FIT: &str = ph::ARROWS_OUT;
/// Centrer la vue sur un objet.
pub const CENTER: &str = ph::CROSSHAIR;
/// Épingler un nœud.
pub const PIN: &str = ph::PUSH_PIN;
/// Copier dans le presse-papiers.
pub const COPY: &str = ph::COPY;
/// Inspecter, ouvrir le détail.
pub const INSPECT: &str = ph::MAGNIFYING_GLASS;

// --- États -------------------------------------------------------------------

/// Information neutre.
pub const INFO: &str = ph::INFO;
/// Réussite, verdict favorable.
pub const SUCCESS: &str = ph::CHECK_CIRCLE;
/// Avertissement.
pub const WARNING: &str = ph::WARNING;
/// Échec.
pub const ERROR: &str = ph::X_CIRCLE;
/// Interdit, capacité dépassée.
pub const BLOCKED: &str = ph::PROHIBIT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_police_est_installee_en_repli_du_proportionnel() {
        let mut fonts = egui::FontDefinitions::default();
        install(&mut fonts);
        let family = &fonts.families[&egui::FontFamily::Proportional];
        assert!(family.iter().any(|name| name == "phosphor"));
        // En second : le texte ordinaire reste rendu par la police d'egui.
        assert_ne!(family.first().map(String::as_str), Some("phosphor"));
    }

    #[test]
    fn les_icones_sont_des_glyphes_prives_uniques() {
        let all = [
            OVERVIEW, RESOURCES, GRAPH, YAML, DEPLOY, HUB, UPDATES, SETTINGS, REFRESH, CLOSE, SUN,
            MOON, BACK, SORT_ASC, SORT_DESC, CURRENT, STAR, DOWNLOADS, FIT, CENTER, PIN, COPY,
            INSPECT, INFO, SUCCESS, WARNING, ERROR, BLOCKED,
        ];
        for icon in all {
            let c = icon.chars().next().expect("un glyphe");
            assert_eq!(icon.chars().count(), 1);
            assert!(
                ('\u{E000}'..='\u{F8FF}').contains(&c),
                "zone privée : {c:?}"
            );
        }
        // Les huit écrans se distinguent au premier coup d'œil.
        let nav = [
            OVERVIEW, RESOURCES, GRAPH, YAML, DEPLOY, HUB, UPDATES, SETTINGS,
        ];
        for (i, a) in nav.iter().enumerate() {
            for b in nav.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }
}
