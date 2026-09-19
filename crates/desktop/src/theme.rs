//! Thème de l'application : deux palettes complètes (claire et sombre), les
//! `egui::Visuals` associés, les tailles de texte et la couleur sémantique des
//! statuts Kubernetes.
//!
//! Rien ici ne dépend de l'état de l'application : ce module est une table de
//! correspondance, appelée à chaque image de rendu, et testable sans fenêtre.

use std::collections::BTreeMap;

use egui::style::{Selection, WidgetVisuals, Widgets};
use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, Style, TextStyle, Visuals};

/// Palette sémantique : les six couleurs utilisées pour porter du sens.
///
/// Elles ne servent jamais de décoration. Une couleur dans KubeWatch dit
/// toujours quelque chose sur l'état d'un objet du cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Tout va bien : `Running`, `Ready`, `Bound`, `Active`.
    pub ok: Color32,
    /// Transitoire : `Pending`, `ContainerCreating`, `Terminating`.
    pub warn: Color32,
    /// Anormal : `CrashLoopBackOff`, `Failed`, `ImagePullBackOff`, `OOMKilled`.
    pub error: Color32,
    /// Terminé sans incident : `Completed`, `Succeeded`.
    pub info: Color32,
    /// Information secondaire : statut inconnu, valeur absente, aide.
    pub muted: Color32,
    /// Couleur d'accentuation : sélection, liens, éléments actifs.
    pub accent: Color32,
}

/// Palette du thème sombre : teintes vives sur fond ardoise.
const DARK: Palette = Palette {
    ok: Color32::from_rgb(74, 222, 128),
    warn: Color32::from_rgb(251, 191, 36),
    error: Color32::from_rgb(248, 113, 113),
    info: Color32::from_rgb(96, 165, 250),
    muted: Color32::from_rgb(148, 158, 173),
    accent: Color32::from_rgb(56, 189, 248),
};

/// Palette du thème clair : teintes sombres et saturées, lisibles sur blanc.
const LIGHT: Palette = Palette {
    ok: Color32::from_rgb(21, 128, 61),
    warn: Color32::from_rgb(180, 83, 9),
    error: Color32::from_rgb(185, 28, 28),
    info: Color32::from_rgb(29, 78, 216),
    muted: Color32::from_rgb(107, 114, 128),
    accent: Color32::from_rgb(3, 105, 161),
};

/// Renvoie la palette sémantique adaptée au thème demandé.
pub fn palette(dark: bool) -> Palette {
    if dark {
        DARK
    } else {
        LIGHT
    }
}

/// Associe une couleur à un statut Kubernetes.
///
/// La comparaison ignore la casse et les espaces. Les statuts inconnus
/// tombent sur `muted` : mieux vaut une couleur neutre qu'une couleur fausse.
pub fn status_color(p: &Palette, status: &str) -> Color32 {
    let s = status.trim();
    if s.is_empty() {
        return p.muted;
    }

    // Correspondances exactes d'abord : ce sont les statuts que l'on connaît.
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "running" | "ready" | "bound" | "active" | "available" | "healthy" | "true"
        | "established" | "synced" => return p.ok,
        "pending" | "containercreating" | "terminating" | "podinitializing" | "init"
        | "progressing" | "notready" | "waiting" | "released" | "unknown" => return p.warn,
        "crashloopbackoff"
        | "error"
        | "failed"
        | "imagepullbackoff"
        | "errimagepull"
        | "oomkilled"
        | "evicted"
        | "unhealthy"
        | "createcontainererror"
        | "invalidimagename"
        | "false"
        | "lost" => return p.error,
        "completed" | "succeeded" | "suspended" | "disabled" => return p.info,
        _ => {}
    }

    // Repli heuristique pour les variantes que le serveur invente (`Init:0/2`,
    // `RunContainerError`, `BackOff`...). On reste prudent : seuls des motifs
    // sans ambiguïté déclenchent une couleur.
    if lower.contains("backoff") || lower.contains("error") || lower.contains("fail") {
        return p.error;
    }
    if lower.starts_with("init:") || lower.contains("creating") || lower.contains("terminat") {
        return p.warn;
    }

    p.muted
}

/// Tailles de texte de l'application, pour les deux thèmes.
///
/// Aucune police externe n'est chargée : on se contente des familles
/// embarquées par egui, dont on fixe les corps.
pub fn text_styles() -> BTreeMap<TextStyle, FontId> {
    BTreeMap::from([
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Heading,
            FontId::new(19.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        ),
    ])
}

/// Construit les `Visuals` complets du thème demandé.
///
/// On part des visuels par défaut d'egui pour hériter des champs sans intérêt
/// éditorial (ombres, forme des poignées), puis on redéfinit toute la
/// colorimétrie : fonds, panneaux, bordures, sélection et états des widgets.
pub fn visuals(dark: bool) -> Visuals {
    let p = palette(dark);

    if dark {
        Visuals {
            widgets: dark_widgets(),
            selection: Selection {
                bg_fill: Color32::from_rgb(26, 79, 120),
                stroke: Stroke::new(1.0, Color32::from_rgb(168, 214, 255)),
            },
            hyperlink_color: p.accent,
            faint_bg_color: Color32::from_rgb(30, 34, 41),
            extreme_bg_color: Color32::from_rgb(14, 16, 20),
            code_bg_color: Color32::from_rgb(26, 30, 36),
            warn_fg_color: p.warn,
            error_fg_color: p.error,
            window_corner_radius: CornerRadius::same(8),
            window_fill: Color32::from_rgb(26, 30, 36),
            window_stroke: Stroke::new(1.0, Color32::from_rgb(56, 63, 74)),
            menu_corner_radius: CornerRadius::same(6),
            panel_fill: Color32::from_rgb(21, 24, 29),
            striped: true,
            slider_trailing_fill: true,
            ..Visuals::dark()
        }
    } else {
        Visuals {
            widgets: light_widgets(),
            selection: Selection {
                bg_fill: Color32::from_rgb(190, 219, 250),
                stroke: Stroke::new(1.0, Color32::from_rgb(3, 105, 161)),
            },
            hyperlink_color: p.accent,
            faint_bg_color: Color32::from_rgb(240, 242, 246),
            extreme_bg_color: Color32::from_rgb(255, 255, 255),
            code_bg_color: Color32::from_rgb(236, 239, 243),
            warn_fg_color: p.warn,
            error_fg_color: p.error,
            window_corner_radius: CornerRadius::same(8),
            window_fill: Color32::from_rgb(252, 252, 253),
            window_stroke: Stroke::new(1.0, Color32::from_rgb(206, 211, 219)),
            menu_corner_radius: CornerRadius::same(6),
            panel_fill: Color32::from_rgb(244, 246, 249),
            striped: true,
            slider_trailing_fill: true,
            ..Visuals::light()
        }
    }
}

/// États des widgets en thème sombre.
fn dark_widgets() -> Widgets {
    Widgets {
        noninteractive: WidgetVisuals {
            bg_fill: Color32::from_rgb(26, 30, 36),
            weak_bg_fill: Color32::from_rgb(26, 30, 36),
            bg_stroke: Stroke::new(1.0, Color32::from_rgb(48, 54, 64)),
            corner_radius: CornerRadius::same(5),
            fg_stroke: Stroke::new(1.0, Color32::from_rgb(198, 206, 217)),
            expansion: 0.0,
        },
        inactive: WidgetVisuals {
            bg_fill: Color32::from_rgb(46, 52, 62),
            weak_bg_fill: Color32::from_rgb(38, 43, 52),
            bg_stroke: Stroke::new(1.0, Color32::from_rgb(58, 65, 77)),
            corner_radius: CornerRadius::same(5),
            fg_stroke: Stroke::new(1.0, Color32::from_rgb(214, 221, 230)),
            expansion: 0.0,
        },
        hovered: WidgetVisuals {
            bg_fill: Color32::from_rgb(60, 68, 81),
            weak_bg_fill: Color32::from_rgb(52, 59, 70),
            bg_stroke: Stroke::new(1.0, Color32::from_rgb(88, 98, 114)),
            corner_radius: CornerRadius::same(5),
            fg_stroke: Stroke::new(1.5, Color32::from_rgb(240, 245, 250)),
            expansion: 1.0,
        },
        active: WidgetVisuals {
            bg_fill: Color32::from_rgb(26, 112, 168),
            weak_bg_fill: Color32::from_rgb(22, 96, 145),
            bg_stroke: Stroke::new(1.0, DARK.accent),
            corner_radius: CornerRadius::same(5),
            fg_stroke: Stroke::new(2.0, Color32::from_rgb(248, 251, 255)),
            expansion: 1.0,
        },
        open: WidgetVisuals {
            bg_fill: Color32::from_rgb(42, 48, 58),
            weak_bg_fill: Color32::from_rgb(36, 41, 50),
            bg_stroke: Stroke::new(1.0, Color32::from_rgb(70, 78, 92)),
            corner_radius: CornerRadius::same(5),
            fg_stroke: Stroke::new(1.0, Color32::from_rgb(222, 228, 236)),
            expansion: 0.0,
        },
    }
}

/// États des widgets en thème clair.
fn light_widgets() -> Widgets {
    Widgets {
        noninteractive: WidgetVisuals {
            bg_fill: Color32::from_rgb(250, 251, 252),
            weak_bg_fill: Color32::from_rgb(250, 251, 252),
            bg_stroke: Stroke::new(1.0, Color32::from_rgb(210, 215, 222)),
            corner_radius: CornerRadius::same(5),
            fg_stroke: Stroke::new(1.0, Color32::from_rgb(40, 46, 56)),
            expansion: 0.0,
        },
        inactive: WidgetVisuals {
            bg_fill: Color32::from_rgb(230, 234, 239),
            weak_bg_fill: Color32::from_rgb(237, 240, 244),
            bg_stroke: Stroke::new(1.0, Color32::from_rgb(199, 206, 215)),
            corner_radius: CornerRadius::same(5),
            fg_stroke: Stroke::new(1.0, Color32::from_rgb(33, 39, 48)),
            expansion: 0.0,
        },
        hovered: WidgetVisuals {
            bg_fill: Color32::from_rgb(214, 221, 230),
            weak_bg_fill: Color32::from_rgb(223, 229, 236),
            bg_stroke: Stroke::new(1.0, Color32::from_rgb(158, 168, 182)),
            corner_radius: CornerRadius::same(5),
            fg_stroke: Stroke::new(1.5, Color32::from_rgb(16, 21, 29)),
            expansion: 1.0,
        },
        active: WidgetVisuals {
            bg_fill: Color32::from_rgb(163, 203, 238),
            weak_bg_fill: Color32::from_rgb(183, 216, 244),
            bg_stroke: Stroke::new(1.0, LIGHT.accent),
            corner_radius: CornerRadius::same(5),
            fg_stroke: Stroke::new(2.0, Color32::from_rgb(8, 13, 21)),
            expansion: 1.0,
        },
        open: WidgetVisuals {
            bg_fill: Color32::from_rgb(226, 231, 237),
            weak_bg_fill: Color32::from_rgb(233, 237, 242),
            bg_stroke: Stroke::new(1.0, Color32::from_rgb(196, 203, 212)),
            corner_radius: CornerRadius::same(5),
            fg_stroke: Stroke::new(1.0, Color32::from_rgb(30, 36, 45)),
            expansion: 0.0,
        },
    }
}

/// Applique les espacements communs aux deux thèmes.
fn apply_spacing(style: &mut Style) {
    style.text_styles = text_styles();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(9.0, 4.0);
    style.spacing.indent = 16.0;
    style.spacing.interact_size.y = 24.0;
    style.spacing.combo_width = 180.0;
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.menu_margin = egui::Margin::same(6);
    // Animations courtes : l'interface doit paraître instantanée, pas molle.
    style.animation_time = 0.08;
}

/// Installe les deux thèmes dans le contexte sans toucher à la préférence.
///
/// Les deux jeux de `Visuals` sont toujours posés : basculer clair/sombre ne
/// coûte ensuite qu'un changement de préférence, sans reconstruire le style.
/// C'est ce qui permet de suivre le thème du système sans à-coup.
pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, visuals(true));
    ctx.set_visuals_of(egui::Theme::Light, visuals(false));
    ctx.all_styles_mut(apply_spacing);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_deux_palettes_different() {
        let d = palette(true);
        let l = palette(false);
        assert_ne!(d.ok, l.ok);
        assert_ne!(d.accent, l.accent);
    }

    #[test]
    fn statuts_sains() {
        let p = palette(true);
        for s in ["Running", "ready", "Bound", "ACTIVE"] {
            assert_eq!(status_color(&p, s), p.ok, "statut sain : {s}");
        }
    }

    #[test]
    fn statuts_transitoires() {
        let p = palette(true);
        for s in ["Pending", "ContainerCreating", "Terminating", "Init:0/2"] {
            assert_eq!(status_color(&p, s), p.warn, "statut transitoire : {s}");
        }
    }

    #[test]
    fn statuts_en_erreur() {
        let p = palette(false);
        for s in [
            "CrashLoopBackOff",
            "Error",
            "Failed",
            "ImagePullBackOff",
            "OOMKilled",
            "RunContainerError",
        ] {
            assert_eq!(status_color(&p, s), p.error, "statut en erreur : {s}");
        }
    }

    #[test]
    fn statuts_termines_et_inconnus() {
        let p = palette(true);
        assert_eq!(status_color(&p, "Completed"), p.info);
        assert_eq!(status_color(&p, "Succeeded"), p.info);
        assert_eq!(status_color(&p, ""), p.muted);
        assert_eq!(status_color(&p, "Zoulou"), p.muted);
    }

    #[test]
    fn visuals_coherents_avec_le_theme() {
        assert!(visuals(true).dark_mode);
        assert!(!visuals(false).dark_mode);
        assert_ne!(visuals(true).panel_fill, visuals(false).panel_fill);
    }

    #[test]
    fn tailles_de_texte_definies() {
        let styles = text_styles();
        assert!(styles.contains_key(&TextStyle::Body));
        assert!(styles.contains_key(&TextStyle::Monospace));
        assert!(styles[&TextStyle::Heading].size > styles[&TextStyle::Body].size);
    }
}
