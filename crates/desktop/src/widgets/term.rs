//! Émulateur de terminal minimal, destiné à `kubectl exec`.
//!
//! Il gère ce dont un shell interactif a réellement besoin : décodage UTF-8 en
//! flux, un sous-ensemble des séquences ANSI (SGR, effacements, positionnement
//! du curseur), une grille dimensionnée d'après la police, et la traduction des
//! évènements clavier d'egui en octets.
//!
//! Le composant est autonome : il ne connaît ni l'état de l'application ni le
//! backend. Il renvoie ce que l'utilisateur a produit, à charge de l'appelant de
//! l'envoyer au cluster.

use std::collections::VecDeque;

/// Nombre de lignes conservées, écran visible compris.
pub const SCROLLBACK: usize = 2000;

/// Nombre maximal de commandes mémorisées dans l'historique local.
const MAX_HISTORY: usize = 200;

/// Bornes de la grille, pour ne jamais demander au cluster une taille absurde.
const MIN_COLS: u16 = 8;
const MAX_COLS: u16 = 500;
const MIN_ROWS: u16 = 2;
const MAX_ROWS: u16 = 300;

/// Taille initiale, avant le premier calcul d'après la fenêtre.
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// Ce que l'utilisateur a produit pendant une image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalInput {
    /// Octets à transmettre au processus distant.
    Bytes(Vec<u8>),
    /// La grille a changé de taille : il faut prévenir le pseudo-terminal.
    Resize {
        /// Nombre de colonnes.
        cols: u16,
        /// Nombre de lignes.
        rows: u16,
    },
}

/// Attributs d'affichage d'une cellule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellStyle {
    /// Index 0..16 dans la palette ANSI, `None` pour la couleur par défaut.
    pub fg: Option<u8>,
    /// Index 0..16 dans la palette ANSI, `None` pour le fond par défaut.
    pub bg: Option<u8>,
    /// Gras (et, pour les couleurs sombres, passage à la variante vive).
    pub bold: bool,
    /// Vidéo inverse.
    pub inverse: bool,
}

/// Un caractère et ses attributs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cell {
    ch: char,
    style: CellStyle,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: CellStyle::default(),
        }
    }
}

/// État de l'automate d'analyse des séquences d'échappement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParserState {
    /// Texte courant.
    Ground,
    /// `ESC` reçu.
    Escape,
    /// `ESC` suivi d'un octet intermédiaire (jeu de caractères, etc.).
    EscapeIntermediate,
    /// `ESC [` : séquence de contrôle, paramètres en cours d'accumulation.
    Csi,
    /// Chaîne de commande (`OSC`, `DCS`, `APC`, `PM`) à ignorer jusqu'au terminateur.
    Osc,
    /// `ESC` reçu à l'intérieur d'une chaîne de commande : `\` la termine.
    OscEscape,
}

/// Émulateur de terminal.
#[derive(Debug, Clone)]
pub struct Terminal {
    /// Lignes, de la plus ancienne à la plus récente. L'écran est la fenêtre
    /// des `rows` dernières lignes.
    lines: VecDeque<Vec<Cell>>,
    cursor_row: usize,
    cursor_col: usize,
    style: CellStyle,
    cols: u16,
    rows: u16,
    /// Octets d'un caractère UTF-8 encore incomplet, conservés d'une trame à
    /// l'autre : c'est ce qui évite le caractère coupé en deux.
    partial: Vec<u8>,
    state: ParserState,
    params: String,
    id: egui::Id,
    history: Vec<String>,
    history_position: Option<usize>,
    current_line: String,
    /// Historique local géré par le client.
    ///
    /// Désactivé par défaut : sur un pseudo-terminal, c'est le shell distant qui
    /// tient l'historique, et les flèches doivent lui parvenir telles quelles.
    pub local_history: bool,
    /// Taille de la police, en points.
    pub font_size: f32,
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminal {
    /// Terminal vide de 80×24.
    pub fn new() -> Self {
        Self::with_id("kubewatch_terminal")
    }

    /// Terminal identifié, pour en afficher plusieurs dans la même fenêtre.
    pub fn with_id(id: impl std::hash::Hash + std::fmt::Debug) -> Self {
        Self {
            lines: VecDeque::new(),
            cursor_row: 0,
            cursor_col: 0,
            style: CellStyle::default(),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            partial: Vec::new(),
            state: ParserState::Ground,
            params: String::new(),
            id: egui::Id::new(id),
            history: Vec::new(),
            history_position: None,
            current_line: String::new(),
            local_history: false,
            font_size: 13.0,
        }
    }

    /// Taille courante de la grille : `(colonnes, lignes)`.
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Impose une taille de grille (bornée), sans émettre d'évènement.
    pub fn set_size(&mut self, cols: u16, rows: u16) {
        self.cols = cols.clamp(MIN_COLS, MAX_COLS);
        self.rows = rows.clamp(MIN_ROWS, MAX_ROWS);
    }

    /// Vide l'écran, l'historique de défilement et l'état de l'automate.
    ///
    /// L'historique des commandes est conservé.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.style = CellStyle::default();
        self.partial.clear();
        self.state = ParserState::Ground;
        self.params.clear();
    }

    /// Nombre de lignes actuellement mémorisées.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Restitue l'écran et son historique sous forme de texte brut.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (index, line) in self.lines.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            for cell in line {
                out.push(cell.ch);
            }
            while out.ends_with(' ') {
                out.pop();
            }
        }
        out
    }

    // ---------------------------------------------------------------- entrée

    /// Absorbe des octets venus du cluster.
    ///
    /// Le décodage UTF-8 est fait en flux : un caractère multi-octets coupé
    /// entre deux trames est reconstitué, jamais remplacé par des losanges.
    pub fn feed(&mut self, data: &[u8]) {
        if data.is_empty() && self.partial.is_empty() {
            return;
        }

        let mut buffer = std::mem::take(&mut self.partial);
        buffer.extend_from_slice(data);

        let mut offset = 0usize;
        while offset < buffer.len() {
            let rest = &buffer[offset..];
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    for character in text.chars() {
                        self.process_char(character);
                    }
                    offset = buffer.len();
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    if valid > 0 {
                        if let Ok(text) = std::str::from_utf8(&rest[..valid]) {
                            for character in text.chars() {
                                self.process_char(character);
                            }
                        }
                    }
                    match error.error_len() {
                        // Fin de trame au milieu d'un caractère : on garde la
                        // queue pour la trame suivante.
                        None => {
                            offset += valid;
                            break;
                        }
                        Some(invalid) => {
                            offset += valid + invalid;
                            self.process_char(char::REPLACEMENT_CHARACTER);
                        }
                    }
                }
            }
        }

        self.partial = buffer.split_off(offset.min(buffer.len()));
        // Un caractère UTF-8 fait au plus quatre octets : au-delà, la queue est
        // du bruit, on l'abandonne plutôt que de la laisser grossir.
        if self.partial.len() > 3 {
            self.partial.clear();
        }
    }

    /// Fait avancer l'automate d'un caractère.
    fn process_char(&mut self, character: char) {
        match self.state {
            ParserState::Ground => self.ground(character),
            ParserState::Escape => self.escape(character),
            ParserState::EscapeIntermediate => self.state = ParserState::Ground,
            ParserState::Csi => self.csi(character),
            ParserState::Osc => match character {
                '\u{7}' => self.state = ParserState::Ground,
                '\u{1b}' => self.state = ParserState::OscEscape,
                _ => {}
            },
            ParserState::OscEscape => match character {
                '\\' => self.state = ParserState::Ground,
                '\u{1b}' => {}
                _ => self.state = ParserState::Osc,
            },
        }
    }

    /// Texte courant et codes de contrôle simples.
    fn ground(&mut self, character: char) {
        match character {
            '\u{1b}' => self.state = ParserState::Escape,
            '\r' => self.cursor_col = 0,
            '\n' => self.line_feed(),
            '\u{8}' => self.cursor_col = self.cursor_col.saturating_sub(1),
            '\t' => {
                let last = self.cols.max(1) as usize - 1;
                self.cursor_col = ((self.cursor_col / 8 + 1) * 8).min(last);
            }
            // Cloche et autres codes de contrôle : ignorés en silence.
            character if (character as u32) < 0x20 || character == '\u{7f}' => {}
            character => self.put(character),
        }
    }

    /// Octet qui suit immédiatement `ESC`.
    fn escape(&mut self, character: char) {
        self.state = match character {
            '[' => {
                self.params.clear();
                ParserState::Csi
            }
            ']' | 'P' | 'X' | '^' | '_' => ParserState::Osc,
            // Octet intermédiaire : la séquence tient en deux caractères de plus.
            character if ('\u{20}'..='\u{2f}').contains(&character) => {
                ParserState::EscapeIntermediate
            }
            // ESC 7, ESC M, ESC =… : reconnues et ignorées proprement.
            _ => ParserState::Ground,
        };
    }

    /// Accumulation puis exécution d'une séquence `ESC [`.
    fn csi(&mut self, character: char) {
        if ('\u{20}'..='\u{3f}').contains(&character) {
            // Garde-fou : une séquence saine ne dépasse jamais quelques octets.
            if self.params.len() < 64 {
                self.params.push(character);
            }
            return;
        }
        if ('\u{40}'..='\u{7e}').contains(&character) {
            let params = std::mem::take(&mut self.params);
            self.execute_csi(&params, character);
        }
        self.params.clear();
        self.state = ParserState::Ground;
    }

    /// Applique une séquence de contrôle reconnue ; ignore les autres.
    fn execute_csi(&mut self, params: &str, final_byte: char) {
        // Séquences privées (`?`, `>`, `<`, `=`) : masquage du curseur, modes
        // divers. Rien à faire ici, mais surtout rien à afficher.
        if params.starts_with(['?', '>', '<', '=']) {
            return;
        }

        match final_byte {
            'm' => apply_sgr(&mut self.style, params),
            'J' => self.erase_display(param_at(params, 0, 0)),
            'K' => self.erase_line(param_at(params, 0, 0)),
            'H' | 'f' => {
                let row = param_at(params, 0, 1).max(1) - 1;
                let column = param_at(params, 1, 1).max(1) - 1;
                self.move_to(row, column);
            }
            'A' => {
                let count = param_at(params, 0, 1).max(1);
                let top = self.screen_top();
                self.cursor_row = self.cursor_row.saturating_sub(count).max(top);
            }
            'B' => {
                let count = param_at(params, 0, 1).max(1);
                let row = self.cursor_row + count;
                self.move_to_absolute(row);
            }
            'C' => {
                let count = param_at(params, 0, 1).max(1);
                let last = self.cols.max(1) as usize - 1;
                self.cursor_col = (self.cursor_col + count).min(last);
            }
            'D' => {
                let count = param_at(params, 0, 1).max(1);
                self.cursor_col = self.cursor_col.saturating_sub(count);
            }
            'G' => {
                let column = param_at(params, 0, 1).max(1) - 1;
                let last = self.cols.max(1) as usize - 1;
                self.cursor_col = column.min(last);
            }
            'd' => {
                let row = param_at(params, 0, 1).max(1) - 1;
                let column = self.cursor_col;
                self.move_to(row, column);
            }
            // Toute autre séquence est ignorée, jamais affichée en clair.
            _ => {}
        }
    }

    // ------------------------------------------------------------- la grille

    /// Index de la première ligne visible.
    fn screen_top(&self) -> usize {
        self.lines.len().saturating_sub(self.rows.max(1) as usize)
    }

    /// Garantit que la ligne `row` existe, puis borne l'historique.
    fn ensure_line(&mut self, row: usize) {
        while self.lines.len() <= row {
            self.lines.push_back(Vec::new());
        }
        while self.lines.len() > SCROLLBACK {
            self.lines.pop_front();
            self.cursor_row = self.cursor_row.saturating_sub(1);
        }
    }

    /// Passage à la ligne suivante, colonne inchangée.
    fn line_feed(&mut self) {
        self.cursor_row += 1;
        let row = self.cursor_row;
        self.ensure_line(row);
    }

    /// Positionne le curseur sur une ligne absolue de la mémoire.
    ///
    /// `ensure_line` peut élaguer l'historique et décaler `cursor_row` en
    /// conséquence : c'est voulu, le curseur suit le contenu.
    fn move_to_absolute(&mut self, row: usize) {
        self.cursor_row = row;
        self.ensure_line(row);
    }

    /// Positionne le curseur, coordonnées relatives à l'écran visible.
    fn move_to(&mut self, row: usize, column: usize) {
        let last_row = self.rows.max(1) as usize - 1;
        let last_column = self.cols.max(1) as usize - 1;
        let target = self.screen_top() + row.min(last_row);
        self.move_to_absolute(target);
        self.cursor_col = column.min(last_column);
    }

    /// Écrit un caractère à la position du curseur, avec repli en fin de ligne.
    fn put(&mut self, character: char) {
        let last_column = self.cols.max(1) as usize;
        if self.cursor_col >= last_column {
            self.line_feed();
            self.cursor_col = 0;
        }

        let requested_row = self.cursor_row;
        self.ensure_line(requested_row);
        // L'élagage de l'historique a pu décaler le curseur : on le relit.
        let row = self.cursor_row;
        let column = self.cursor_col;
        let style = self.style;
        if let Some(line) = self.lines.get_mut(row) {
            while line.len() <= column {
                line.push(Cell::default());
            }
            line[column] = Cell { ch: character, style };
        }
        self.cursor_col += 1;
    }

    /// Effacement d'écran : `0` du curseur à la fin, `1` du début au curseur,
    /// `2` et `3` la totalité de l'écran visible.
    fn erase_display(&mut self, mode: usize) {
        let top = self.screen_top();
        let end = self.lines.len();
        match mode {
            0 => {
                let column = self.cursor_col;
                if let Some(line) = self.lines.get_mut(self.cursor_row) {
                    line.truncate(column);
                }
                for index in (self.cursor_row + 1)..end {
                    if let Some(line) = self.lines.get_mut(index) {
                        line.clear();
                    }
                }
            }
            1 => {
                for index in top..self.cursor_row {
                    if let Some(line) = self.lines.get_mut(index) {
                        line.clear();
                    }
                }
                let column = self.cursor_col;
                if let Some(line) = self.lines.get_mut(self.cursor_row) {
                    for cell in line.iter_mut().take(column + 1) {
                        *cell = Cell::default();
                    }
                }
            }
            2 | 3 => {
                for index in top..end {
                    if let Some(line) = self.lines.get_mut(index) {
                        line.clear();
                    }
                }
            }
            _ => {}
        }
    }

    /// Effacement de ligne : `0` du curseur à la fin, `1` du début au curseur,
    /// `2` la ligne entière.
    fn erase_line(&mut self, mode: usize) {
        let column = self.cursor_col;
        if let Some(line) = self.lines.get_mut(self.cursor_row) {
            match mode {
                0 => line.truncate(column),
                1 => {
                    for cell in line.iter_mut().take(column + 1) {
                        *cell = Cell::default();
                    }
                }
                2 => line.clear(),
                _ => {}
            }
        }
    }

    // -------------------------------------------------------------- affichage

    /// Dessine le terminal et renvoie ce que l'utilisateur a produit.
    ///
    /// Aucune opération bloquante : la méthode se contente de lire l'état local
    /// et les évènements clavier de l'image courante.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Vec<TerminalInput> {
        let mut outputs: Vec<TerminalInput> = Vec::new();
        let ctx = ui.ctx().clone();
        let font_id = egui::FontId::monospace(self.font_size);
        let row_height = ctx.fonts_mut(|fonts| fonts.row_height(&font_id)).max(1.0);
        let glyph_width = ctx.fonts_mut(|fonts| fonts.glyph_width(&font_id, 'M')).max(1.0);
        let focused = ctx.memory(|memory| memory.has_focus(self.id));

        self.show_toolbar(ui, focused);

        // Taille de la grille déduite de la police et de la place disponible.
        let available = ui.available_size();
        let cols = ((available.x - 16.0) / glyph_width).floor().max(1.0) as u16;
        let rows = ((available.y - 12.0) / row_height).floor().max(1.0) as u16;
        let cols = cols.clamp(MIN_COLS, MAX_COLS);
        let rows = rows.clamp(MIN_ROWS, MAX_ROWS);
        if cols != self.cols || rows != self.rows {
            self.cols = cols;
            self.rows = rows;
            outputs.push(TerminalInput::Resize { cols, rows });
        }

        let palette = ansi_palette(ui.visuals().dark_mode);
        let default_fg = ui.visuals().text_color();
        let default_bg = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(0x12, 0x14, 0x18)
        } else {
            egui::Color32::from_rgb(0xfa, 0xfa, 0xfa)
        };

        let total = self.lines.len().max(1);
        let cursor_row = self.cursor_row;
        let cursor_col = self.cursor_col;
        let lines = &self.lines;

        let frame = egui::Frame::new()
            .fill(default_bg)
            .stroke(if focused {
                egui::Stroke::new(1.0, ui.visuals().selection.stroke.color)
            } else {
                ui.visuals().widgets.noninteractive.bg_stroke
            })
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(egui::Margin::same(6));

        let frame_response = frame.show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .id_salt("kubewatch_terminal_scroll")
                .show_rows(ui, row_height, total, |ui, range| {
                    for index in range {
                        let cursor = if focused && index == cursor_row {
                            Some(cursor_col)
                        } else {
                            None
                        };
                        let job = row_job(
                            lines.get(index),
                            &font_id,
                            &palette,
                            default_fg,
                            default_bg,
                            cursor,
                        );
                        ui.add(
                            egui::Label::new(job)
                                .wrap_mode(egui::TextWrapMode::Extend)
                                .selectable(true),
                        );
                    }
                });
        });

        let response = ui.interact(
            frame_response.response.rect,
            self.id,
            egui::Sense::click(),
        );
        if response.clicked() {
            response.request_focus();
        }

        if focused {
            // Sans ce verrou, Tab, les flèches et Échap déplaceraient le focus
            // au lieu de parvenir au shell distant.
            ctx.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    self.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
            let events = ctx.input(|input| input.events.clone());
            let bytes = self.consume_events(&events);
            if !bytes.is_empty() {
                outputs.push(TerminalInput::Bytes(bytes));
            }
        }

        outputs
    }

    /// Barre d'outils compacte : état du focus, taille, historique, effacement.
    fn show_toolbar(&mut self, ui: &mut egui::Ui, focused: bool) {
        ui.horizontal(|ui| {
            let (cols, rows) = (self.cols, self.rows);
            ui.label(
                egui::RichText::new(format!("{cols}×{rows}"))
                    .small()
                    .weak(),
            );
            ui.separator();
            if ui.small_button("Effacer").clicked() {
                self.clear();
            }
            if ui
                .small_button("Copier")
                .on_hover_text("Copie l'écran et son historique en texte brut")
                .clicked()
            {
                ui.ctx().copy_text(self.to_text());
            }
            ui.checkbox(&mut self.local_history, "Historique local")
                .on_hover_text(
                    "Les flèches Haut/Bas rejouent les commandes saisies ici, \
                     au lieu d'être transmises au shell distant.",
                );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let message = if focused {
                    "clavier actif"
                } else {
                    "cliquer pour saisir"
                };
                ui.label(egui::RichText::new(message).small().weak());
            });
        });
    }

    /// Traduit les évènements clavier de l'image en octets à transmettre.
    fn consume_events(&mut self, events: &[egui::Event]) -> Vec<u8> {
        let mut bytes: Vec<u8> = Vec::new();

        for event in events {
            match event {
                egui::Event::Text(text) => {
                    bytes.extend_from_slice(text.as_bytes());
                    self.current_line.push_str(text);
                }
                egui::Event::Paste(text) => {
                    bytes.extend_from_slice(text.as_bytes());
                    self.current_line.push_str(text);
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    // L'historique local intercepte les flèches verticales.
                    if self.local_history
                        && !modifiers.ctrl
                        && !modifiers.alt
                        && matches!(*key, egui::Key::ArrowUp | egui::Key::ArrowDown)
                    {
                        if let Some(replacement) = self.navigate_history(*key) {
                            bytes.extend_from_slice(&replacement);
                        }
                        continue;
                    }

                    if let Some(translated) = key_to_bytes(*key, *modifiers) {
                        bytes.extend_from_slice(&translated);
                        self.track_line(*key, *modifiers);
                    }
                }
                _ => {}
            }
        }

        bytes
    }

    /// Tient à jour la ligne en cours de saisie, pour alimenter l'historique.
    fn track_line(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        match key {
            egui::Key::Enter => {
                let line = std::mem::take(&mut self.current_line).trim().to_string();
                self.history_position = None;
                if line.is_empty() {
                    return;
                }
                if self.history.last().map(String::as_str) == Some(line.as_str()) {
                    return;
                }
                self.history.push(line);
                while self.history.len() > MAX_HISTORY {
                    self.history.remove(0);
                }
            }
            egui::Key::Backspace => {
                self.current_line.pop();
            }
            egui::Key::C | egui::Key::U if modifiers.ctrl => {
                self.current_line.clear();
                self.history_position = None;
            }
            _ => {}
        }
    }

    /// Remplace la ligne en cours par une entrée de l'historique local.
    ///
    /// Renvoie les octets à envoyer : autant d'effacements que de caractères
    /// déjà saisis, suivis de la commande retrouvée.
    fn navigate_history(&mut self, key: egui::Key) -> Option<Vec<u8>> {
        if self.history.is_empty() {
            return None;
        }

        let next_position = match (key, self.history_position) {
            (egui::Key::ArrowUp, None) => Some(self.history.len() - 1),
            (egui::Key::ArrowUp, Some(0)) => Some(0),
            (egui::Key::ArrowUp, Some(position)) => Some(position - 1),
            (egui::Key::ArrowDown, None) => return None,
            (egui::Key::ArrowDown, Some(position)) => {
                if position + 1 < self.history.len() {
                    Some(position + 1)
                } else {
                    None
                }
            }
            _ => return None,
        };

        let replacement = match next_position {
            Some(position) => self.history.get(position).cloned().unwrap_or_default(),
            None => String::new(),
        };

        let mut bytes = vec![0x7f; self.current_line.chars().count()];
        bytes.extend_from_slice(replacement.as_bytes());

        self.history_position = next_position;
        self.current_line = replacement;
        Some(bytes)
    }
}

/// Lit le n-ième paramètre d'une séquence CSI, avec valeur par défaut.
fn param_at(params: &str, index: usize, default: usize) -> usize {
    match params.split(';').nth(index) {
        Some(raw) if !raw.trim().is_empty() => raw.trim().parse().unwrap_or(default),
        _ => default,
    }
}

/// Applique une séquence SGR (`ESC [ … m`) à un style.
///
/// Sont gérés : la remise à zéro, le gras, la vidéo inverse, les seize couleurs
/// de premier plan et de fond. Les couleurs étendues (`38`/`48`) sont consommées
/// sans être appliquées, afin de ne pas déborder sur les paramètres suivants.
pub fn apply_sgr(style: &mut CellStyle, params: &str) {
    if params.trim().is_empty() {
        *style = CellStyle::default();
        return;
    }

    let mut fields = params.split(';');
    while let Some(raw) = fields.next() {
        // Un paramètre vide vaut zéro, conformément à ECMA-48.
        let code: u16 = raw.trim().parse().unwrap_or(0);
        match code {
            0 => *style = CellStyle::default(),
            1 => style.bold = true,
            22 => style.bold = false,
            7 => style.inverse = true,
            27 => style.inverse = false,
            30..=37 => style.fg = Some((code - 30) as u8),
            39 => style.fg = None,
            40..=47 => style.bg = Some((code - 40) as u8),
            49 => style.bg = None,
            90..=97 => style.fg = Some((code - 90 + 8) as u8),
            100..=107 => style.bg = Some((code - 100 + 8) as u8),
            38 | 48 => match fields.next().and_then(|value| value.trim().parse::<u16>().ok()) {
                Some(5) => {
                    fields.next();
                }
                Some(2) => {
                    fields.next();
                    fields.next();
                    fields.next();
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// Traduit une touche en octets pour le pseudo-terminal.
///
/// Renvoie `None` pour les caractères imprimables : ceux-là arrivent par
/// [`egui::Event::Text`], qui tient compte de la disposition du clavier.
pub fn key_to_bytes(key: egui::Key, modifiers: egui::Modifiers) -> Option<Vec<u8>> {
    use egui::Key;

    if modifiers.ctrl || modifiers.mac_cmd {
        if let Some(offset) = letter_offset(key) {
            return Some(vec![offset + 1]);
        }
        match key {
            Key::Space => return Some(vec![0x00]),
            Key::OpenBracket => return Some(vec![0x1b]),
            Key::Backslash => return Some(vec![0x1c]),
            Key::CloseBracket => return Some(vec![0x1d]),
            _ => {}
        }
    }

    let bytes: &[u8] = match key {
        Key::Enter => b"\r",
        Key::Backspace => b"\x7f",
        Key::Tab => b"\t",
        Key::Escape => b"\x1b",
        Key::ArrowUp => b"\x1b[A",
        Key::ArrowDown => b"\x1b[B",
        Key::ArrowRight => b"\x1b[C",
        Key::ArrowLeft => b"\x1b[D",
        Key::Home => b"\x1b[H",
        Key::End => b"\x1b[F",
        Key::Insert => b"\x1b[2~",
        Key::Delete => b"\x1b[3~",
        Key::PageUp => b"\x1b[5~",
        Key::PageDown => b"\x1b[6~",
        _ => return None,
    };
    Some(bytes.to_vec())
}

/// Rang alphabétique d'une touche lettre, base zéro.
fn letter_offset(key: egui::Key) -> Option<u8> {
    use egui::Key;
    let offset = match key {
        Key::A => 0,
        Key::B => 1,
        Key::C => 2,
        Key::D => 3,
        Key::E => 4,
        Key::F => 5,
        Key::G => 6,
        Key::H => 7,
        Key::I => 8,
        Key::J => 9,
        Key::K => 10,
        Key::L => 11,
        Key::M => 12,
        Key::N => 13,
        Key::O => 14,
        Key::P => 15,
        Key::Q => 16,
        Key::R => 17,
        Key::S => 18,
        Key::T => 19,
        Key::U => 20,
        Key::V => 21,
        Key::W => 22,
        Key::X => 23,
        Key::Y => 24,
        Key::Z => 25,
        _ => return None,
    };
    Some(offset)
}

/// Palette ANSI seize couleurs, déclinée pour le thème clair et le thème sombre.
fn ansi_palette(dark_mode: bool) -> [egui::Color32; 16] {
    let rgb = egui::Color32::from_rgb;
    if dark_mode {
        [
            rgb(0x2e, 0x34, 0x40),
            rgb(0xbf, 0x61, 0x6a),
            rgb(0xa3, 0xbe, 0x8c),
            rgb(0xeb, 0xcb, 0x8b),
            rgb(0x81, 0xa1, 0xc1),
            rgb(0xb4, 0x8e, 0xad),
            rgb(0x88, 0xc0, 0xd0),
            rgb(0xe5, 0xe9, 0xf0),
            rgb(0x4c, 0x56, 0x6a),
            rgb(0xd0, 0x87, 0x70),
            rgb(0xb9, 0xd4, 0xa4),
            rgb(0xf0, 0xd9, 0xa4),
            rgb(0x9c, 0xba, 0xd8),
            rgb(0xc9, 0xa7, 0xc4),
            rgb(0xa3, 0xd8, 0xe4),
            rgb(0xff, 0xff, 0xff),
        ]
    } else {
        [
            rgb(0x21, 0x25, 0x2b),
            rgb(0xc2, 0x1c, 0x1c),
            rgb(0x1f, 0x7a, 0x3d),
            rgb(0x94, 0x6c, 0x00),
            rgb(0x1e, 0x5c, 0xa8),
            rgb(0x8a, 0x3f, 0xa0),
            rgb(0x0f, 0x74, 0x86),
            rgb(0x4a, 0x50, 0x57),
            rgb(0x6b, 0x72, 0x7a),
            rgb(0xe0, 0x3b, 0x3b),
            rgb(0x2b, 0x9a, 0x55),
            rgb(0xb8, 0x8a, 0x10),
            rgb(0x2b, 0x77, 0xc8),
            rgb(0xa8, 0x59, 0xbd),
            rgb(0x17, 0x92, 0xa6),
            rgb(0x1a, 0x1d, 0x22),
        ]
    }
}

/// Résout les couleurs effectives d'un style.
fn resolve_colors(
    style: CellStyle,
    is_cursor: bool,
    palette: &[egui::Color32; 16],
    default_fg: egui::Color32,
    default_bg: egui::Color32,
) -> (egui::Color32, egui::Color32) {
    let mut index = style.fg;
    // Le gras ravive les huit couleurs sombres, comme sur un vrai terminal.
    if style.bold {
        if let Some(value) = index {
            if value < 8 {
                index = Some(value + 8);
            }
        }
    }

    let mut foreground = index
        .map(|value| palette[(value as usize) % palette.len()])
        .unwrap_or(default_fg);
    let mut background = style
        .bg
        .map(|value| palette[(value as usize) % palette.len()])
        .unwrap_or(default_bg);

    if style.inverse != is_cursor {
        std::mem::swap(&mut foreground, &mut background);
    }
    (foreground, background)
}

/// Compose la mise en page d'une ligne de la grille.
///
/// Les cellules de même style sont regroupées en un seul segment, pour ne pas
/// produire un segment de texte par caractère.
fn row_job(
    cells: Option<&Vec<Cell>>,
    font_id: &egui::FontId,
    palette: &[egui::Color32; 16],
    default_fg: egui::Color32,
    default_bg: egui::Color32,
    cursor: Option<usize>,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job.break_on_newline = false;
    job.keep_trailing_whitespace = true;

    let empty: Vec<Cell> = Vec::new();
    let cells = cells.unwrap_or(&empty);

    let mut run = String::new();
    let mut run_key: Option<(CellStyle, bool)> = None;

    let push_run = |job: &mut egui::text::LayoutJob,
                    run: &mut String,
                    key: Option<(CellStyle, bool)>| {
        if run.is_empty() {
            return;
        }
        let (style, is_cursor) = key.unwrap_or_default();
        let (foreground, background) =
            resolve_colors(style, is_cursor, palette, default_fg, default_bg);
        job.append(
            run,
            0.0,
            egui::text::TextFormat {
                font_id: font_id.clone(),
                color: foreground,
                background,
                ..Default::default()
            },
        );
        run.clear();
    };

    for (index, cell) in cells.iter().enumerate() {
        let key = (cell.style, cursor == Some(index));
        if run_key != Some(key) {
            push_run(&mut job, &mut run, run_key);
            run_key = Some(key);
        }
        run.push(cell.ch);
    }

    // Curseur au-delà du dernier caractère écrit : on complète la ligne.
    if let Some(column) = cursor {
        if column >= cells.len() {
            let padding = column - cells.len();
            if padding > 0 {
                let key = (CellStyle::default(), false);
                if run_key != Some(key) {
                    push_run(&mut job, &mut run, run_key);
                    run_key = Some(key);
                }
                for _ in 0..padding {
                    run.push(' ');
                }
            }
            let key = (CellStyle::default(), true);
            push_run(&mut job, &mut run, run_key);
            run_key = Some(key);
            run.push(' ');
        }
    }

    push_run(&mut job, &mut run, run_key);

    // Une ligne vide doit tout de même occuper une hauteur de ligne, sans quoi
    // les rangées du rendu virtuel se décaleraient.
    if job.text.is_empty() {
        job.append(
            " ",
            0.0,
            egui::text::TextFormat {
                font_id: font_id.clone(),
                color: default_fg,
                ..Default::default()
            },
        );
    }

    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal() -> Terminal {
        let mut term = Terminal::new();
        term.set_size(20, 5);
        term
    }

    #[test]
    fn utf8_coupe_entre_deux_trames() {
        let mut term = terminal();
        let texte = "héllo → café";
        let octets = texte.as_bytes();

        // On découpe octet par octet : le pire cas possible.
        for octet in octets {
            term.feed(&[*octet]);
        }
        assert_eq!(term.to_text(), texte);
    }

    #[test]
    fn utf8_coupe_au_milieu_d_un_caractere() {
        let mut term = terminal();
        // « é » s'encode 0xC3 0xA9 : la trame s'arrête au milieu.
        term.feed(&[b'c', b'a', b'f', 0xC3]);
        assert_eq!(term.to_text(), "caf");
        term.feed(&[0xA9]);
        assert_eq!(term.to_text(), "café");

        // Un caractère sur quatre octets, découpé en trois morceaux.
        let mut term = terminal();
        let emoji = "🚀".as_bytes().to_vec();
        term.feed(&emoji[..1]);
        term.feed(&emoji[1..3]);
        term.feed(&emoji[3..]);
        assert_eq!(term.to_text(), "🚀");
    }

    #[test]
    fn octets_invalides_remplaces_sans_perdre_la_suite() {
        let mut term = terminal();
        term.feed(&[b'a', 0xFF, b'b']);
        assert_eq!(term.to_text(), "a\u{fffd}b");
    }

    #[test]
    fn sgr_couleurs_gras_et_remise_a_zero() {
        let mut style = CellStyle::default();

        apply_sgr(&mut style, "1;31");
        assert!(style.bold);
        assert_eq!(style.fg, Some(1));

        apply_sgr(&mut style, "42");
        assert_eq!(style.bg, Some(2));
        assert_eq!(style.fg, Some(1), "le fond ne touche pas au premier plan");

        apply_sgr(&mut style, "39");
        assert_eq!(style.fg, None);

        apply_sgr(&mut style, "96");
        assert_eq!(style.fg, Some(14), "les couleurs vives commencent à l'index 8");

        apply_sgr(&mut style, "0");
        assert_eq!(style, CellStyle::default());

        // Paramètre absent : équivaut à une remise à zéro.
        style.bold = true;
        apply_sgr(&mut style, "");
        assert_eq!(style, CellStyle::default());

        // Un champ vide vaut zéro.
        style.bold = true;
        apply_sgr(&mut style, ";");
        assert_eq!(style, CellStyle::default());

        // Couleur étendue : consommée sans déborder sur le paramètre suivant.
        apply_sgr(&mut style, "38;5;200;1");
        assert!(style.bold, "le gras qui suit doit être appliqué");
        apply_sgr(&mut style, "0;38;2;10;20;30;4");
        assert_eq!(style.fg, None);
    }

    #[test]
    fn traduction_des_touches_en_octets() {
        use egui::{Key, Modifiers};

        assert_eq!(key_to_bytes(Key::Enter, Modifiers::NONE), Some(b"\r".to_vec()));
        assert_eq!(
            key_to_bytes(Key::Backspace, Modifiers::NONE),
            Some(vec![0x7f])
        );
        assert_eq!(key_to_bytes(Key::Tab, Modifiers::NONE), Some(b"\t".to_vec()));
        assert_eq!(
            key_to_bytes(Key::ArrowUp, Modifiers::NONE),
            Some(vec![0x1b, b'[', b'A'])
        );
        assert_eq!(
            key_to_bytes(Key::ArrowDown, Modifiers::NONE),
            Some(vec![0x1b, b'[', b'B'])
        );
        assert_eq!(
            key_to_bytes(Key::ArrowRight, Modifiers::NONE),
            Some(vec![0x1b, b'[', b'C'])
        );
        assert_eq!(
            key_to_bytes(Key::ArrowLeft, Modifiers::NONE),
            Some(vec![0x1b, b'[', b'D'])
        );

        // Ctrl+lettre produit l'octet de contrôle correspondant.
        assert_eq!(key_to_bytes(Key::C, Modifiers::CTRL), Some(vec![0x03]));
        assert_eq!(key_to_bytes(Key::A, Modifiers::CTRL), Some(vec![0x01]));
        assert_eq!(key_to_bytes(Key::D, Modifiers::CTRL), Some(vec![0x04]));
        assert_eq!(key_to_bytes(Key::Z, Modifiers::CTRL), Some(vec![0x1a]));
        assert_eq!(key_to_bytes(Key::Space, Modifiers::CTRL), Some(vec![0x00]));

        // Les caractères imprimables passent par `Event::Text`.
        assert_eq!(key_to_bytes(Key::A, Modifiers::NONE), None);
        assert_eq!(key_to_bytes(Key::Num1, Modifiers::NONE), None);
    }

    #[test]
    fn sequences_ansi_de_base() {
        let mut term = terminal();
        term.feed(b"bonjour\r\nligne deux");
        assert_eq!(term.to_text(), "bonjour\nligne deux");

        // Retour chariot puis réécriture.
        let mut term = terminal();
        term.feed(b"abcdef\rXY");
        assert_eq!(term.to_text(), "XYcdef");

        // Retour arrière.
        let mut term = terminal();
        term.feed(b"abc\x08Z");
        assert_eq!(term.to_text(), "abZ");

        // Effacement de fin de ligne.
        let mut term = terminal();
        term.feed(b"abcdef\r\x1b[3C\x1b[K");
        assert_eq!(term.to_text(), "abc");

        // Positionnement absolu du curseur.
        let mut term = terminal();
        term.feed(b"\x1b[1;1Habc\x1b[1;1HX");
        assert_eq!(term.to_text(), "Xbc");
    }

    #[test]
    fn sequences_inconnues_ignorees_sans_affichage() {
        let mut term = terminal();
        // Masquage du curseur, mode crochet collé, titre de fenêtre, jeu de
        // caractères : rien ne doit apparaître à l'écran.
        term.feed(b"\x1b[?25l\x1b[?2004h\x1b]0;titre\x07\x1b(Bok");
        assert_eq!(term.to_text(), "ok");

        // Terminateur de chaîne « ESC \ » au lieu de la cloche.
        let mut term = terminal();
        term.feed(b"\x1b]0;titre\x1b\\suite");
        assert_eq!(term.to_text(), "suite");
    }

    #[test]
    fn effacement_d_ecran() {
        let mut term = terminal();
        term.feed(b"une\r\ndeux\r\ntrois");
        term.feed(b"\x1b[2J");
        assert_eq!(term.to_text().trim(), "");
        // L'écran est vide mais le nombre de lignes est conservé.
        assert_eq!(term.line_count(), 3);
    }

    #[test]
    fn historique_de_defilement_borne() {
        let mut term = terminal();
        for index in 0..(SCROLLBACK + 50) {
            term.feed(format!("ligne {index}\r\n").as_bytes());
        }
        assert!(
            term.line_count() <= SCROLLBACK,
            "l'historique dépasse la borne : {}",
            term.line_count()
        );
    }

    #[test]
    fn taille_bornee() {
        let mut term = Terminal::new();
        term.set_size(0, 0);
        assert_eq!(term.size(), (MIN_COLS, MIN_ROWS));
        term.set_size(u16::MAX, u16::MAX);
        assert_eq!(term.size(), (MAX_COLS, MAX_ROWS));
    }

    #[test]
    fn repli_en_fin_de_ligne() {
        let mut term = Terminal::new();
        term.set_size(8, 4);
        term.feed(b"0123456789");
        assert_eq!(term.to_text(), "01234567\n89");
    }
}
