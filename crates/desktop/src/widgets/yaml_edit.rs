//! Éditeur de manifestes YAML : police à chasse fixe, gouttière de numéros de
//! ligne, coloration syntaxique et validation multi-documents.
//!
//! Le composant est autonome : il détient son texte et ne connaît ni l'état de
//! l'application ni le backend.

use std::sync::Arc;

use serde::Deserialize;

/// Une indentation vaut deux espaces, comme dans les manifestes Kubernetes.
const INDENT: &str = "  ";

/// Au-delà de cette taille, la coloration syntaxique est désactivée pour ne pas
/// pénaliser la fluidité de la saisie.
const MAX_HIGHLIGHT_BYTES: usize = 200_000;

/// Éditeur de texte YAML.
#[derive(Debug, Clone)]
pub struct YamlEditor {
    /// Contenu édité.
    pub text: String,
    /// Taille de la police, en points.
    pub font_size: f32,
    /// Coloration syntaxique active.
    pub highlight: bool,
    /// Gouttière de numéros de ligne visible.
    pub line_numbers: bool,
}

impl Default for YamlEditor {
    fn default() -> Self {
        Self {
            text: String::new(),
            font_size: 13.0,
            highlight: true,
            line_numbers: true,
        }
    }
}

impl YamlEditor {
    /// Éditeur vide.
    pub fn new() -> Self {
        Self::default()
    }

    /// Éditeur pré-rempli.
    pub fn with_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// Remplace le contenu.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }

    /// Vrai si l'éditeur ne contient que des blancs.
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Nombre de lignes affichées (une ligne vide finale compte).
    pub fn line_count(&self) -> usize {
        self.text.split('\n').count().max(1)
    }

    /// Analyse le contenu en multi-documents et renvoie le nombre de documents
    /// non vides, ou un message d'erreur en français.
    pub fn validate(&self) -> Result<usize, String> {
        if self.is_blank() {
            return Err("Le document est vide.".to_string());
        }

        let mut documents = 0usize;
        for document in serde_yaml_ng::Deserializer::from_str(&self.text) {
            match serde_yaml_ng::Value::deserialize(document) {
                // Un document vide (séparateur `---` isolé) est ignoré sans erreur.
                Ok(serde_yaml_ng::Value::Null) => continue,
                Ok(_) => documents += 1,
                Err(error) => return Err(describe_error(&error)),
            }
        }

        if documents == 0 {
            return Err(
                "Aucun document exploitable : le YAML ne contient que des sections vides."
                    .to_string(),
            );
        }
        Ok(documents)
    }

    /// Dessine l'éditeur et renvoie la réponse de la zone de saisie.
    ///
    /// `id` distingue plusieurs éditeurs affichés dans la même fenêtre.
    pub fn show(&mut self, ui: &mut egui::Ui, id: &str) -> egui::Response {
        let text_id = egui::Id::new("kubewatch_yaml_edit").with(id);
        let font_id = egui::FontId::monospace(self.font_size);
        let ctx = ui.ctx().clone();

        // Maj+Tab : désindentation maison. egui désindente par pas de quatre
        // espaces alors que nos manifestes en utilisent deux ; on intercepte donc
        // la touche avant que la zone de saisie ne la voie. La touche Tab simple,
        // elle, est laissée à egui puis convertie en espaces après coup.
        if ctx.memory(|memory| memory.has_focus(text_id))
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab))
        {
            self.dedent_current_line(&ctx, text_id);
        }

        // Style dédié : la gouttière, la coloration et la saisie doivent partager
        // exactement la même police, sans quoi les lignes se décalent.
        let mut style: egui::Style = (**ui.style()).clone();
        style.override_font_id = None;
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, font_id.clone());
        let style = Arc::new(style);
        let theme = egui_extras::syntax_highlighting::CodeTheme::from_style(&style);

        let row_height = ctx.fonts_mut(|fonts| fonts.row_height(&font_id));
        let glyph_width = ctx
            .fonts_mut(|fonts| fonts.glyph_width(&font_id, 'M'))
            .max(1.0);

        let line_count = self.line_count();
        let longest = self
            .text
            .split('\n')
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        let digits = line_count.to_string().chars().count().max(2);
        let gutter_width = if self.line_numbers {
            digits as f32 * glyph_width + 12.0
        } else {
            0.0
        };

        let available_width = ui.available_width();
        let available_height = ui.available_height();
        let visible_rows = ((available_height - 16.0) / row_height).max(3.0) as usize;
        let desired_rows = visible_rows.max(line_count).min(100_000);
        let desired_width =
            (longest as f32 * glyph_width + 16.0).max(available_width - gutter_width - 24.0);

        let highlight_enabled = self.highlight && self.text.len() <= MAX_HIGHLIGHT_BYTES;
        let layout_style = Arc::clone(&style);
        let layout_font = font_id.clone();
        let mut layouter =
            move |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _wrap_width: f32| {
                let mut job = if highlight_enabled {
                    egui_extras::syntax_highlighting::highlight(
                        ui.ctx(),
                        &layout_style,
                        &theme,
                        buffer.as_str(),
                        "yaml",
                    )
                } else {
                    egui::text::LayoutJob::simple(
                        buffer.as_str().to_owned(),
                        layout_font.clone(),
                        ui.visuals().text_color(),
                        f32::INFINITY,
                    )
                };
                // Aucun retour à la ligne automatique : une ligne logique occupe
                // exactement une ligne affichée, c'est ce qui garde la gouttière
                // alignée sur le texte.
                job.wrap.max_width = f32::INFINITY;
                job.break_on_newline = true;
                ui.painter().layout_job(job)
            };

        let frame = egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(egui::Margin::same(6));

        let line_numbers = self.line_numbers;
        let text = &mut self.text;

        let response = frame
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .id_salt((id, "kubewatch_yaml_scroll"))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if line_numbers {
                                show_gutter(ui, line_count, gutter_width, &font_id, row_height);
                            }
                            ui.add(
                                egui::TextEdit::multiline(text)
                                    .id(text_id)
                                    .font(font_id.clone())
                                    .lock_focus(true)
                                    // Pas de cadre ni de marge : c'est le cadre
                                    // extérieur qui habille l'éditeur, et la
                                    // gouttière doit démarrer à la même ligne de
                                    // base que le texte.
                                    .frame(egui::Frame::NONE)
                                    .margin(egui::Margin::ZERO)
                                    .desired_rows(desired_rows)
                                    .desired_width(desired_width)
                                    .layouter(&mut layouter),
                            )
                        })
                        .inner
                    })
                    .inner
            })
            .inner;

        // egui insère une tabulation littérale ; on la remplace par deux espaces
        // en recalant le curseur d'autant.
        if self.text.contains('\t') {
            self.expand_tabs(&ctx, text_id);
        }

        response
    }

    /// Remplace les tabulations par [`INDENT`] et recale le curseur.
    fn expand_tabs(&mut self, ctx: &egui::Context, id: egui::Id) {
        let cursor = egui::TextEdit::load_state(ctx, id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index.0);

        let tabs_before = match cursor {
            Some(position) => self
                .text
                .chars()
                .take(position)
                .filter(|character| *character == '\t')
                .count(),
            None => 0,
        };

        self.text = self.text.replace('\t', INDENT);

        if let Some(position) = cursor {
            let shift = tabs_before * INDENT.chars().count().saturating_sub(1);
            set_cursor(ctx, id, position + shift);
        }
    }

    /// Retire une indentation au début de la ligne du curseur.
    fn dedent_current_line(&mut self, ctx: &egui::Context, id: egui::Id) {
        let Some(cursor) = egui::TextEdit::load_state(ctx, id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index.0)
        else {
            return;
        };

        let mut characters: Vec<char> = self.text.chars().collect();
        let cursor = cursor.min(characters.len());

        let mut line_start = cursor;
        while line_start > 0 && characters[line_start - 1] != '\n' {
            line_start -= 1;
        }

        let width = INDENT.chars().count();
        let removed = if characters.get(line_start) == Some(&'\t') {
            1
        } else {
            let mut count = 0usize;
            while count < width && characters.get(line_start + count) == Some(&' ') {
                count += 1;
            }
            count
        };

        if removed == 0 {
            return;
        }

        characters.drain(line_start..line_start + removed);
        self.text = characters.into_iter().collect();

        let new_cursor = if cursor >= line_start + removed {
            cursor - removed
        } else {
            line_start
        };
        set_cursor(ctx, id, new_cursor);
    }
}

/// Positionne le curseur de la zone de saisie sur l'index de caractère donné.
fn set_cursor(ctx: &egui::Context, id: egui::Id, position: usize) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        let cursor = egui::text::CCursor::new(position);
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(cursor)));
        egui::TextEdit::store_state(ctx, id, state);
    }
}

/// Dessine la gouttière des numéros de ligne.
///
/// Chaque numéro occupe exactement une hauteur de ligne, peint à la main : c'est
/// ce qui garantit l'alignement au pixel avec les lignes de la zone de saisie.
fn show_gutter(
    ui: &mut egui::Ui,
    line_count: usize,
    width: f32,
    font_id: &egui::FontId,
    row_height: f32,
) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.set_min_width(width);
        ui.set_max_width(width);
        let color = ui.visuals().weak_text_color();
        ui.vertical(|ui| {
            for number in 1..=line_count {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(width, row_height),
                    egui::Sense::hover(),
                );
                ui.painter().text(
                    egui::pos2(rect.right() - 6.0, rect.top()),
                    egui::Align2::RIGHT_TOP,
                    number,
                    font_id.clone(),
                    color,
                );
            }
        });
    });
}

/// Traduit une erreur `serde_yaml_ng` en message français localisé.
fn describe_error(error: &serde_yaml_ng::Error) -> String {
    match error.location() {
        Some(location) => format!(
            "YAML invalide (ligne {}, colonne {}) : {}",
            location.line(),
            location.column(),
            error
        ),
        None => format!("YAML invalide : {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compte_les_documents_non_vides() {
        let editor = YamlEditor::with_text(
            "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: demo\n---\n\
             apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: reglages\n",
        );
        assert_eq!(editor.validate(), Ok(2));
    }

    #[test]
    fn ignore_les_documents_vides() {
        let editor = YamlEditor::with_text("---\n---\napiVersion: v1\nkind: Pod\n---\n");
        assert_eq!(editor.validate(), Ok(1));
    }

    #[test]
    fn refuse_un_texte_vide() {
        let editor = YamlEditor::with_text("   \n\n");
        let erreur = editor.validate().unwrap_err();
        assert!(erreur.contains("vide"), "message inattendu : {erreur}");
    }

    #[test]
    fn signale_une_erreur_de_syntaxe_en_francais() {
        let editor = YamlEditor::with_text("kind: Pod\n  mauvaise: indentation\n");
        let erreur = editor.validate().unwrap_err();
        assert!(
            erreur.starts_with("YAML invalide"),
            "message inattendu : {erreur}"
        );
    }

    #[test]
    fn compte_les_lignes_avec_saut_final() {
        assert_eq!(YamlEditor::with_text("a").line_count(), 1);
        assert_eq!(YamlEditor::with_text("a\n").line_count(), 2);
        assert_eq!(YamlEditor::with_text("a\nb\nc").line_count(), 3);
        assert_eq!(YamlEditor::new().line_count(), 1);
    }
}
