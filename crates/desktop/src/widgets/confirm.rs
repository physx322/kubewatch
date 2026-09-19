//! Boîte de confirmation modale, garde-fou des opérations destructrices.
//!
//! Le composant est autonome : il reçoit l'état de la boîte et renvoie l'action
//! à exécuter lorsque l'utilisateur valide. Il n'exécute rien lui-même.

use kubewatch_core::model::ResourceRef;

use crate::icons;

/// Largeur maximale de la boîte, en points.
const MAX_WIDTH: f32 = 460.0;

/// Ce qu'il faut faire lorsque l'utilisateur confirme.
///
/// La variante ne porte que l'identité de la cible : c'est à l'appelant de
/// traduire cela en `Command` pour le backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    /// Supprimer une ressource du cluster.
    DeleteResource(ResourceRef),
    /// Vider un nœud de ses pods.
    DrainNode(String),
    /// Redémarrer progressivement une charge de travail.
    RestartWorkload(ResourceRef),
    /// Ramener une charge de travail à sa révision précédente.
    RollbackWorkload(ResourceRef),
    /// Porter une charge de travail au nombre de répliques indiqué.
    ScaleWorkload(ResourceRef, i32),
}

/// État d'une boîte de confirmation en cours d'affichage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    /// Titre de la boîte.
    pub title: String,
    /// Corps explicatif, affiché sous le titre.
    pub body: String,
    /// Vrai pour une opération destructrice : bouton rouge et ton plus ferme.
    pub danger: bool,
    /// Si renseigné, l'utilisateur doit saisir exactement ce texte pour valider.
    pub require_text: Option<String>,
    /// Ce que l'utilisateur a saisi jusqu'ici.
    pub typed: String,
    /// Action renvoyée à la validation.
    pub on_confirm: ConfirmAction,
}

impl Confirm {
    /// Boîte simple, non destructrice, sans saisie de confirmation.
    pub fn new(
        title: impl Into<String>,
        body: impl Into<String>,
        on_confirm: ConfirmAction,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            danger: false,
            require_text: None,
            typed: String::new(),
            on_confirm,
        }
    }

    /// Marque la boîte comme destructrice.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// Exige la saisie exacte de `expected` avant de pouvoir valider.
    pub fn require_text(mut self, expected: impl Into<String>) -> Self {
        self.require_text = Some(expected.into());
        self
    }

    /// Vrai lorsque la validation est permise.
    ///
    /// Sans texte exigé, la boîte est toujours validable ; sinon la saisie doit
    /// correspondre au caractère près (espaces de bordure mis à part).
    pub fn is_ready(&self) -> bool {
        match &self.require_text {
            None => true,
            Some(expected) => self.typed.trim() == expected.trim(),
        }
    }

    /// Libellé du bouton de validation.
    pub fn confirm_label(&self) -> &'static str {
        if self.danger {
            "Supprimer"
        } else {
            "Confirmer"
        }
    }
}

/// Affiche la boîte si elle existe et renvoie l'action validée.
///
/// La boîte se ferme dans les trois cas : validation, annulation (bouton, clic
/// hors de la boîte ou touche Échap). Seule la validation renvoie une action.
pub fn show(ctx: &egui::Context, confirm: &mut Option<Confirm>) -> Option<ConfirmAction> {
    let mut outcome: Option<ConfirmAction> = None;
    let mut close = false;

    if let Some(state) = confirm.as_mut() {
        // Échap annule : on lit la touche avant de dessiner pour que la boîte
        // ne se referme pas sur l'image suivante seulement.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }

        let style = ctx.global_style();
        let accent = if state.danger {
            style.visuals.error_fg_color
        } else {
            style.visuals.text_color()
        };

        let response = egui::Modal::new(egui::Id::new("kubewatch_confirm")).show(ctx, |ui| {
            ui.set_max_width(MAX_WIDTH);

            ui.horizontal(|ui| {
                if state.danger {
                    ui.label(egui::RichText::new(icons::WARNING).color(accent).heading());
                }
                ui.label(egui::RichText::new(state.title.as_str()).heading().color(accent));
            });
            ui.add_space(6.0);
            ui.add(egui::Label::new(state.body.as_str()).wrap());

            if let Some(expected) = state.require_text.clone() {
                ui.add_space(10.0);
                ui.label(format!(
                    "Pour confirmer, saisissez exactement : {expected}"
                ));
                ui.add(
                    egui::TextEdit::singleline(&mut state.typed)
                        .id(egui::Id::new("kubewatch_confirm_saisie"))
                        .hint_text(expected.as_str())
                        .desired_width(f32::INFINITY),
                );
                if !state.is_ready() && !state.typed.is_empty() {
                    ui.label(
                        egui::RichText::new("La saisie ne correspond pas encore.")
                            .small()
                            .color(ui.visuals().warn_fg_color),
                    );
                }
            }

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(8.0);

            let ready = state.is_ready();
            let mut validated = false;
            let mut cancelled = false;

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut confirm_button = egui::Button::new(
                    egui::RichText::new(state.confirm_label()).color(egui::Color32::WHITE),
                );
                if state.danger {
                    confirm_button = confirm_button.fill(ui.visuals().error_fg_color);
                }
                if ui.add_enabled(ready, confirm_button).clicked() {
                    validated = true;
                }
                if ui.button("Annuler").clicked() {
                    cancelled = true;
                }
            });

            // Entrée valide, mais seulement si la boîte est prête : c'est le
            // garde-fou qui évite les suppressions par réflexe.
            if ready && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                validated = true;
            }

            (validated, cancelled)
        });

        let (validated, cancelled) = response.inner;
        if validated {
            outcome = Some(state.on_confirm.clone());
        }
        if cancelled || response.backdrop_response.clicked() {
            close = true;
        }
    }

    if outcome.is_some() || close {
        *confirm = None;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sans_texte_exige_la_boite_est_prete() {
        let confirm = Confirm::new("Titre", "Corps", ConfirmAction::DrainNode("noeud-1".into()));
        assert!(confirm.is_ready());
        assert_eq!(confirm.confirm_label(), "Confirmer");
    }

    #[test]
    fn le_texte_exige_doit_correspondre_exactement() {
        let mut confirm = Confirm::new(
            "Supprimer le nœud",
            "Cette opération est irréversible.",
            ConfirmAction::DrainNode("node-1".to_string()),
        )
        .danger()
        .require_text("node-1");

        assert!(!confirm.is_ready());
        confirm.typed = "node".to_string();
        assert!(!confirm.is_ready());
        confirm.typed = "node-2".to_string();
        assert!(!confirm.is_ready());
        confirm.typed = "  node-1  ".to_string();
        assert!(confirm.is_ready(), "les espaces de bordure sont tolérés");
        confirm.typed = "node-1".to_string();
        assert!(confirm.is_ready());
        assert_eq!(confirm.confirm_label(), "Supprimer");
    }

    #[test]
    fn action_porte_la_reference_complete() {
        let reference = ResourceRef {
            group: "apps".to_string(),
            version: "v1".to_string(),
            kind: "Deployment".to_string(),
            plural: "deployments".to_string(),
            namespace: Some("prod".to_string()),
            name: "api".to_string(),
        };
        let action = ConfirmAction::DeleteResource(reference.clone());
        assert_eq!(action, ConfirmAction::DeleteResource(reference));
    }
}
