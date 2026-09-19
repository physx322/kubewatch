//! Visionneuse de journaux : anneau borné, filtre, suivi automatique du bas et
//! rendu virtualisé pour rester fluide sur plusieurs milliers de lignes.
//!
//! Le composant est autonome : il reçoit ses lignes une par une et ne connaît ni
//! l'état de l'application ni le backend.

use std::collections::VecDeque;

use crate::icons;

/// Nombre de lignes conservées par défaut.
pub const DEFAULT_MAX_LINES: usize = 5000;

/// En mode « retour à la ligne », le rendu ne peut plus être virtualisé : on
/// borne alors le nombre de lignes affichées pour préserver la fluidité.
const WRAP_MAX_ROWS: usize = 1000;

/// Visionneuse de journaux.
#[derive(Debug, Clone)]
pub struct LogView {
    /// Anneau des lignes reçues, de la plus ancienne à la plus récente.
    pub lines: VecDeque<String>,
    /// Suivi automatique : la vue reste collée au bas du flux.
    pub follow: bool,
    /// Filtre texte, insensible à la casse, appliqué à l'affichage seulement.
    pub filter: String,
    /// Retour à la ligne automatique.
    pub wrap: bool,
    /// Taille maximale de l'anneau.
    pub max_lines: usize,
}

impl Default for LogView {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            follow: true,
            filter: String::new(),
            wrap: false,
            max_lines: DEFAULT_MAX_LINES,
        }
    }
}

impl LogView {
    /// Visionneuse vide, suivi activé.
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Ajoute une ligne et évince les plus anciennes au-delà de `max_lines`.
    ///
    /// C'est le seul point d'entrée des données : l'anneau reste borné quoi
    /// qu'il arrive, un pod bavard ne peut pas saturer la mémoire.
    pub fn push(&mut self, line: String) {
        if self.max_lines == 0 {
            self.lines.clear();
            return;
        }
        self.lines.push_back(line);
        while self.lines.len() > self.max_lines {
            self.lines.pop_front();
        }
    }

    /// Vide l'anneau.
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Nombre de lignes conservées.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Vrai si aucune ligne n'a été reçue.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Restitue l'intégralité de l'anneau, une ligne par ligne de texte.
    pub fn export(&self) -> String {
        let mut out = String::new();
        for (index, line) in self.lines.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(line);
        }
        out
    }

    /// Indices des lignes correspondant au filtre courant.
    ///
    /// `None` signifie « toutes les lignes » : on évite ainsi de matérialiser un
    /// vecteur d'indices quand aucun filtre n'est saisi.
    fn matching(&self) -> Option<Vec<usize>> {
        let needle = self.filter.trim().to_lowercase();
        if needle.is_empty() {
            return None;
        }
        Some(
            self.lines
                .iter()
                .enumerate()
                .filter(|(_, line)| matches_filter(line, &needle))
                .map(|(index, _)| index)
                .collect(),
        )
    }

    /// Dessine la barre d'outils puis la liste des lignes.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.show_toolbar(ui);
        ui.separator();

        let matched = self.matching();
        let total = matched.as_ref().map_or(self.lines.len(), Vec::len);

        if total == 0 {
            ui.add_space(8.0);
            let message = if self.lines.is_empty() {
                "Aucune ligne pour l'instant."
            } else {
                "Aucune ligne ne correspond au filtre."
            };
            ui.label(egui::RichText::new(message).weak().italics());
            return;
        }

        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
        let row_height = ui.ctx().fonts_mut(|fonts| fonts.row_height(&font_id));
        let palette = Palette::from(ui.visuals());

        let follow = self.follow;
        let wrap = self.wrap;
        let lines = &self.lines;
        let matched_ref = matched.as_ref();

        // Les lignes de journal se lisent mieux serrées : on annule l'espacement
        // vertical, et `show_rows` doit le voir avant de calculer ses rangées.
        let geometry = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let area = egui::ScrollArea::vertical()
                    .stick_to_bottom(follow)
                    .auto_shrink([false, false])
                    .id_salt("kubewatch_log_view");

                if wrap {
                    let output = area.show(ui, |ui| {
                        let start = total.saturating_sub(WRAP_MAX_ROWS);
                        if start > 0 {
                            ui.label(
                                egui::RichText::new(format!(
                                    "… {start} ligne(s) plus ancienne(s) masquée(s) : \
                                     désactivez le retour à la ligne pour tout consulter."
                                ))
                                .weak()
                                .italics(),
                            );
                        }
                        for position in start..total {
                            let index = resolve(matched_ref, position);
                            if let Some(line) = index.and_then(|index| lines.get(index)) {
                                ui.add(
                                    egui::Label::new(rich_line(line, &font_id, &palette))
                                        .wrap()
                                        .selectable(true),
                                );
                            }
                        }
                    });
                    (output.state.offset.y, output.inner_rect, output.content_size)
                } else {
                    let output = area.show_rows(ui, row_height, total, |ui, range| {
                        for position in range {
                            let index = resolve(matched_ref, position);
                            if let Some(line) = index.and_then(|index| lines.get(index)) {
                                ui.add(
                                    egui::Label::new(rich_line(line, &font_id, &palette))
                                        .wrap_mode(egui::TextWrapMode::Extend)
                                        .selectable(true),
                                );
                            }
                        }
                    });
                    (output.state.offset.y, output.inner_rect, output.content_size)
                }
            })
            .inner;

        self.update_follow(ui, geometry);
    }

    /// Barre d'outils : suivre, retour à la ligne, effacer, filtrer, exporter.
    fn show_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.follow, "Suivre")
                .on_hover_text("Rester collé à la dernière ligne reçue");
            ui.checkbox(&mut self.wrap, "Retour à la ligne")
                .on_hover_text("Replier les lignes longues (limite l'historique affiché)");

            if ui.button("Effacer").clicked() {
                self.clear();
            }

            ui.label("Filtre :");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .id(egui::Id::new("kubewatch_log_filter"))
                    .hint_text("texte à rechercher")
                    .desired_width(180.0),
            );
            if !self.filter.is_empty() && ui.small_button(icons::CLOSE).clicked() {
                self.filter.clear();
            }

            if ui
                .button("Exporter")
                .on_hover_text("Copier toutes les lignes dans le presse-papiers")
                .clicked()
            {
                let contents = self.export();
                ui.ctx().copy_text(contents);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{} ligne(s)", self.lines.len()))
                        .weak()
                        .small(),
                );
            });
        });
    }

    /// Désactive le suivi dès que l'utilisateur remonte, le réactive lorsqu'il
    /// revient au bas de la liste.
    fn update_follow(&mut self, ui: &egui::Ui, geometry: (f32, egui::Rect, egui::Vec2)) {
        let (offset_y, inner_rect, content_size) = geometry;
        let at_bottom = offset_y + inner_rect.height() >= content_size.y - 4.0;

        let hovered = ui
            .ctx()
            .input(|input| input.pointer.hover_pos())
            .map(|position| inner_rect.contains(position))
            .unwrap_or(false);
        let scrolled_up = hovered && ui.ctx().input(|input| input.smooth_scroll_delta.y) > 0.5;

        if scrolled_up && !at_bottom {
            self.follow = false;
        } else if at_bottom {
            self.follow = true;
        }
    }
}

/// Traduit une position d'affichage en index dans l'anneau.
fn resolve(matched: Option<&Vec<usize>>, position: usize) -> Option<usize> {
    match matched {
        Some(indices) => indices.get(position).copied(),
        None => Some(position),
    }
}

/// Couleurs retenues pour la mise en évidence des niveaux de journal.
struct Palette {
    normal: egui::Color32,
    warning: egui::Color32,
    error: egui::Color32,
}

impl Palette {
    fn from(visuals: &egui::Visuals) -> Self {
        Self {
            normal: visuals.text_color(),
            warning: visuals.warn_fg_color,
            error: visuals.error_fg_color,
        }
    }
}

/// Met la ligne en forme, en signalant discrètement les erreurs et alertes.
fn rich_line(line: &str, font_id: &egui::FontId, palette: &Palette) -> egui::RichText {
    let color = match severity(line) {
        Severity::Error => palette.error,
        Severity::Warning => palette.warning,
        Severity::Normal => palette.normal,
    };
    egui::RichText::new(line).font(font_id.clone()).color(color)
}

/// Niveau déduit du contenu de la ligne.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Normal,
    Warning,
    Error,
}

/// Heuristique volontairement simple : seuls les mots-clés les plus courants
/// sont reconnus, sur le début de la ligne pour limiter le coût.
fn severity(line: &str) -> Severity {
    // Le découpage doit tomber sur une frontière de caractère : une ligne de
    // journal contient souvent de l'UTF-8, et trancher au milieu paniquerait.
    let mut end = line.len().min(200);
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    let head = &line[..end];
    const ERREURS: [&str; 5] = ["error", "erreur", "fatal", "panic", "\"level\":\"error\""];
    const ALERTES: [&str; 3] = ["warn", "attention", "deprecat"];

    for motif in ERREURS {
        if contains_ascii_ci(head, motif) {
            return Severity::Error;
        }
    }
    for motif in ALERTES {
        if contains_ascii_ci(head, motif) {
            return Severity::Warning;
        }
    }
    Severity::Normal
}

/// Recherche ASCII insensible à la casse, sans allocation.
fn contains_ascii_ci(haystack: &str, needle_lower: &str) -> bool {
    let hay = haystack.as_bytes();
    let needle = needle_lower.as_bytes();
    if needle.is_empty() {
        return true;
    }
    if needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

/// Applique le filtre à une ligne. `needle_lower` est déjà en minuscules.
///
/// Le cas ASCII, de loin le plus courant dans les journaux, ne provoque aucune
/// allocation ; le reste retombe sur une mise en minuscules complète.
pub fn matches_filter(line: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if needle_lower.is_ascii() && line.is_ascii() {
        contains_ascii_ci(line, needle_lower)
    } else {
        line.to_lowercase().contains(needle_lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anneau_borne_a_max_lines() {
        let mut view = LogView {
            max_lines: 3,
            ..LogView::new()
        };
        for index in 0..10 {
            view.push(format!("ligne {index}"));
        }
        assert_eq!(view.len(), 3);
        assert_eq!(view.lines.front().map(String::as_str), Some("ligne 7"));
        assert_eq!(view.lines.back().map(String::as_str), Some("ligne 9"));
    }

    #[test]
    fn anneau_de_taille_nulle_ne_conserve_rien() {
        let mut view = LogView {
            max_lines: 0,
            ..LogView::new()
        };
        view.push("perdue".to_string());
        assert!(view.is_empty());
    }

    #[test]
    fn export_restitue_les_lignes_dans_l_ordre() {
        let mut view = LogView::new();
        view.push("une".to_string());
        view.push("deux".to_string());
        view.push("trois".to_string());
        assert_eq!(view.export(), "une\ndeux\ntrois");
        view.clear();
        assert_eq!(view.export(), "");
    }

    #[test]
    fn filtre_insensible_a_la_casse() {
        assert!(matches_filter("Connexion REFUSÉE", "refus"));
        assert!(matches_filter("HTTP 500 Internal", "internal"));
        assert!(matches_filter("HTTP 500 Internal", "INTERNAL".to_lowercase().as_str()));
        assert!(!matches_filter("tout va bien", "erreur"));
        assert!(matches_filter("peu importe", ""));
    }

    #[test]
    fn indices_filtres_pointent_sur_les_bonnes_lignes() {
        let mut view = LogView::new();
        view.push("démarrage".to_string());
        view.push("ERROR connexion refusée".to_string());
        view.push("prêt".to_string());
        view.filter = "error".to_string();

        let matched = view.matching().expect("un filtre est actif");
        assert_eq!(matched, vec![1]);
        assert_eq!(resolve(Some(&matched), 0), Some(1));
        assert_eq!(resolve(Some(&matched), 1), None);
        assert_eq!(resolve(None, 2), Some(2));
    }

    #[test]
    fn niveaux_reconnus() {
        assert_eq!(severity("E0102 ERROR: boum"), Severity::Error);
        assert_eq!(severity("W0102 warning: attention"), Severity::Warning);
        assert_eq!(severity("I0102 tout va bien"), Severity::Normal);
        // La recherche est bornée au début de la ligne.
        let longue = format!("{}error", " ".repeat(400));
        assert_eq!(severity(&longue), Severity::Normal);
    }
}
