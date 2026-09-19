//! Bandeaux de notification éphémères, empilés en bas à droite de la fenêtre.
//!
//! Le composant est autonome : il ne connaît ni `AppState` ni le backend, il reçoit
//! ses messages par appel et se dessine dans une `egui::Area` ancrée.
//! Un clic sur un bandeau copie son message dans le presse-papiers, la croix le ferme.

use std::time::{Duration, Instant};

use crate::icons;

/// Durée de vie par défaut d'un bandeau.
pub const DEFAULT_TTL: Duration = Duration::from_secs(5);

/// Durée de vie des erreurs : plus longue, on veut avoir le temps de les lire et de les copier.
pub const ERROR_TTL: Duration = Duration::from_secs(10);

/// Durée du fondu de sortie, en secondes.
const FADE_SECONDS: f32 = 0.5;

/// Nombre maximal de bandeaux empilés simultanément.
const MAX_VISIBLE: usize = 6;

/// Largeur maximale d'un bandeau, en points.
const MAX_WIDTH: f32 = 380.0;

/// Nature d'un bandeau, qui détermine sa couleur, son icône et sa durée de vie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToastKind {
    /// Information neutre.
    Info,
    /// Opération réussie.
    Success,
    /// Avertissement : l'opération a abouti mais mérite un regard.
    Warning,
    /// Échec.
    Error,
}

impl ToastKind {
    /// Pictogramme affiché à gauche du message.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Info => icons::INFO,
            Self::Success => icons::SUCCESS,
            Self::Warning => icons::WARNING,
            Self::Error => icons::ERROR,
        }
    }

    /// Durée de vie recommandée pour cette nature de bandeau.
    pub fn ttl(self) -> Duration {
        match self {
            Self::Error => ERROR_TTL,
            _ => DEFAULT_TTL,
        }
    }

    /// Couleur d'accentuation, dérivée du thème courant pour rester lisible
    /// en mode clair comme en mode sombre.
    pub fn color(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            Self::Info => visuals.hyperlink_color,
            Self::Success => {
                if visuals.dark_mode {
                    egui::Color32::from_rgb(0x5c, 0xd6, 0x8a)
                } else {
                    egui::Color32::from_rgb(0x1b, 0x84, 0x43)
                }
            }
            Self::Warning => visuals.warn_fg_color,
            Self::Error => visuals.error_fg_color,
        }
    }
}

/// Un bandeau et son horodatage de création.
#[derive(Debug, Clone)]
pub struct Toast {
    /// Nature du bandeau.
    pub kind: ToastKind,
    /// Texte affiché, et copié dans le presse-papiers en cas de clic.
    pub message: String,
    /// Instant de création, qui sert de base au calcul de la durée restante.
    pub created: Instant,
    /// Durée de vie avant disparition.
    pub ttl: Duration,
}

impl Toast {
    /// Construit un bandeau avec la durée de vie recommandée pour sa nature.
    pub fn new(kind: ToastKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            created: Instant::now(),
            ttl: kind.ttl(),
        }
    }

    /// Durée restante avant disparition, nulle si le bandeau est périmé.
    pub fn remaining(&self, now: Instant) -> Duration {
        self.ttl
            .checked_sub(now.saturating_duration_since(self.created))
            .unwrap_or(Duration::ZERO)
    }

    /// Vrai lorsque le bandeau a dépassé sa durée de vie.
    pub fn is_expired(&self, now: Instant) -> bool {
        self.remaining(now).is_zero()
    }

    /// Opacité du bandeau : pleine, puis fondu linéaire sur la dernière demi-seconde.
    pub fn opacity(&self, now: Instant) -> f32 {
        let remaining = self.remaining(now).as_secs_f32();
        if remaining >= FADE_SECONDS {
            1.0
        } else {
            (remaining / FADE_SECONDS).clamp(0.0, 1.0)
        }
    }
}

/// Pile de bandeaux. Les plus anciens sont en haut, les plus récents en bas.
#[derive(Debug, Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    /// Pile vide.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute un bandeau d'information.
    pub fn info(&mut self, msg: impl Into<String>) {
        self.push(Toast::new(ToastKind::Info, msg));
    }

    /// Ajoute un bandeau de succès.
    pub fn success(&mut self, msg: impl Into<String>) {
        self.push(Toast::new(ToastKind::Success, msg));
    }

    /// Ajoute un bandeau d'avertissement.
    pub fn warning(&mut self, msg: impl Into<String>) {
        self.push(Toast::new(ToastKind::Warning, msg));
    }

    /// Ajoute un bandeau d'erreur.
    pub fn error(&mut self, msg: impl Into<String>) {
        self.push(Toast::new(ToastKind::Error, msg));
    }

    /// Ajoute un bandeau déjà construit et borne la pile à [`MAX_VISIBLE`].
    pub fn push(&mut self, toast: Toast) {
        self.items.push(toast);
        while self.items.len() > MAX_VISIBLE {
            self.items.remove(0);
        }
    }

    /// Nombre de bandeaux en attente d'affichage.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Vrai si aucun bandeau n'est en cours.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Vide la pile.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Accès en lecture aux bandeaux, utile pour les tests et l'introspection.
    pub fn items(&self) -> &[Toast] {
        &self.items
    }

    /// Retire les bandeaux périmés. Renvoie le nombre de bandeaux restants.
    pub fn prune(&mut self, now: Instant) -> usize {
        self.items.retain(|toast| !toast.is_expired(now));
        self.items.len()
    }

    /// Dessine la pile dans une zone ancrée en bas à droite de la fenêtre.
    ///
    /// La méthode ne bloque jamais : elle se contente de lire l'état local et
    /// demande un rafraîchissement tant qu'un bandeau est vivant, afin que le
    /// fondu de sortie s'anime même sans interaction de l'utilisateur.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let now = Instant::now();
        if self.prune(now) == 0 {
            return;
        }

        let ctx = ui.ctx().clone();
        // Sans cette demande, l'interface reste figée et le bandeau ne disparaîtrait
        // qu'au prochain évènement utilisateur.
        ctx.request_repaint_after(Duration::from_millis(50));

        let mut to_close: Option<usize> = None;
        let mut to_copy: Option<String> = None;

        let items = &self.items;
        egui::Area::new(egui::Id::new("kubewatch_toasts"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .interactable(true)
            .show(&ctx, |ui| {
                ui.set_max_width(MAX_WIDTH);
                for (index, toast) in items.iter().enumerate() {
                    let outcome = show_one(ui, toast, now);
                    match outcome {
                        ToastOutcome::Closed => to_close = Some(index),
                        ToastOutcome::Copied => to_copy = Some(toast.message.clone()),
                        ToastOutcome::None => {}
                    }
                }
            });

        if let Some(index) = to_close {
            if index < self.items.len() {
                self.items.remove(index);
            }
        }
        if let Some(message) = to_copy {
            ctx.copy_text(message);
        }
    }
}

/// Ce que l'utilisateur a fait d'un bandeau pendant l'image courante.
enum ToastOutcome {
    /// Rien.
    None,
    /// La croix (ou un clic sur la croix) a fermé le bandeau.
    Closed,
    /// Le corps du bandeau a été cliqué : message copié.
    Copied,
}

/// Dessine un bandeau et renvoie l'action de l'utilisateur.
fn show_one(ui: &mut egui::Ui, toast: &Toast, now: Instant) -> ToastOutcome {
    let opacity = toast.opacity(now);
    let accent = toast.kind.color(ui.visuals());
    let background = ui.visuals().window_fill;
    let text_color = ui.visuals().text_color();

    let inner = ui.scope(|ui| {
        ui.set_opacity(opacity);
        ui.set_max_width(MAX_WIDTH);
        egui::Frame::new()
            .fill(background)
            .stroke(egui::Stroke::new(1.0, accent))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .outer_margin(egui::Margin {
                left: 0,
                right: 0,
                top: 0,
                bottom: 6,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(toast.kind.icon())
                            .color(accent)
                            .strong(),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&toast.message).color(text_color),
                        )
                        .wrap()
                        .selectable(false),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close = ui.small_button(icons::CLOSE).on_hover_text("Fermer");
                        (close.rect, close.clicked())
                    })
                    .inner
                })
                .inner
            })
    });

    let (close_rect, close_clicked) = inner.inner.inner;
    if close_clicked {
        return ToastOutcome::Closed;
    }
    // Le corps entier du bandeau est cliquable : le clic copie le message.
    // La croix est dessinée avant cette zone, l'ordre de priorité du test de survol
    // n'est donc pas garanti : on départage nous-mêmes avec la position du pointeur.
    let body = inner
        .response
        .interact(egui::Sense::click())
        .on_hover_text("Cliquer pour copier le message");

    if body.clicked() {
        let over_close = body
            .interact_pointer_pos()
            .map(|pos| close_rect.contains(pos))
            .unwrap_or(false);
        if over_close {
            return ToastOutcome::Closed;
        }
        return ToastOutcome::Copied;
    }
    ToastOutcome::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duree_de_vie_par_nature() {
        assert_eq!(ToastKind::Info.ttl(), DEFAULT_TTL);
        assert_eq!(ToastKind::Success.ttl(), DEFAULT_TTL);
        assert_eq!(ToastKind::Warning.ttl(), DEFAULT_TTL);
        assert_eq!(ToastKind::Error.ttl(), ERROR_TTL);
    }

    #[test]
    fn fondu_sur_la_derniere_demi_seconde() {
        let created = Instant::now();
        let toast = Toast {
            kind: ToastKind::Info,
            message: "test".to_string(),
            created,
            ttl: Duration::from_secs(5),
        };
        assert_eq!(toast.opacity(created), 1.0);
        assert_eq!(toast.opacity(created + Duration::from_secs(4)), 1.0);
        let moitie = toast.opacity(created + Duration::from_millis(4750));
        assert!((moitie - 0.5).abs() < 0.05, "opacité inattendue : {moitie}");
        assert_eq!(toast.opacity(created + Duration::from_secs(5)), 0.0);
        assert_eq!(toast.opacity(created + Duration::from_secs(9)), 0.0);
    }

    #[test]
    fn peremption_et_purge() {
        let mut toasts = Toasts::new();
        let base = Instant::now();
        toasts.push(Toast {
            kind: ToastKind::Info,
            message: "vieux".to_string(),
            created: base,
            ttl: Duration::from_secs(1),
        });
        toasts.push(Toast {
            kind: ToastKind::Error,
            message: "récent".to_string(),
            created: base,
            ttl: Duration::from_secs(30),
        });
        assert_eq!(toasts.len(), 2);
        assert_eq!(toasts.prune(base + Duration::from_secs(2)), 1);
        assert_eq!(toasts.items()[0].message, "récent");
    }

    #[test]
    fn pile_bornee() {
        let mut toasts = Toasts::new();
        for i in 0..(MAX_VISIBLE + 4) {
            toasts.info(format!("message {i}"));
        }
        assert_eq!(toasts.len(), MAX_VISIBLE);
        // Les plus anciens ont été évincés, le dernier ajouté est conservé.
        assert_eq!(
            toasts.items()[MAX_VISIBLE - 1].message,
            format!("message {}", MAX_VISIBLE + 3)
        );
    }
}
