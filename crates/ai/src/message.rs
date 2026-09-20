//! Modèle de conversation commun à tous les fournisseurs.
//!
//! Chaque fournisseur convertit ces types dans son propre dialecte JSON
//! (`anthropic.rs`, `openai.rs`) ; l'application et l'interface ne voient que
//! ceux-ci.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Auteur d'un message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// L'utilisateur — ou, pour les résultats d'outils, l'application.
    User,
    /// Le modèle.
    Assistant,
}

/// Fragment d'un message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Part {
    /// Texte brut (Markdown côté modèle).
    Text {
        /// Contenu.
        text: String,
    },
    /// Demande d'appel d'outil émise par le modèle.
    ToolCall {
        /// Identifiant attribué par le fournisseur, repris dans le résultat.
        id: String,
        /// Nom de l'outil.
        name: String,
        /// Arguments, conformes au schéma déclaré.
        input: Value,
    },
    /// Résultat d'un appel d'outil, renvoyé au modèle.
    ToolResult {
        /// Identifiant de l'appel.
        call_id: String,
        /// Nom de l'outil (pour l'affichage ; les API n'en ont pas besoin).
        name: String,
        /// Résultat textuel.
        content: String,
        /// Vrai si l'exécution a échoué.
        #[serde(default)]
        is_error: bool,
    },
}

/// Message d'une conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    /// Auteur.
    pub role: Role,
    /// Fragments, dans l'ordre.
    pub parts: Vec<Part>,
}

impl ChatMessage {
    /// Message utilisateur ne contenant qu'un texte.
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            parts: vec![Part::Text { text: text.into() }],
        }
    }

    /// Message de l'assistant ne contenant qu'un texte.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            parts: vec![Part::Text { text: text.into() }],
        }
    }

    /// Concaténation des fragments textuels.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for p in &self.parts {
            if let Part::Text { text } = p {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
        out
    }

    /// Appels d'outils portés par le message.
    pub fn tool_calls(&self) -> Vec<(&str, &str, &Value)> {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall { id, name, input } => Some((id.as_str(), name.as_str(), input)),
                _ => None,
            })
            .collect()
    }
}

/// Description d'un outil proposé au modèle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    /// Nom, tel que le modèle le désignera.
    pub name: String,
    /// Description en langage naturel : quand et comment l'utiliser.
    pub description: String,
    /// Schéma JSON des arguments (`type: object`).
    pub input_schema: Value,
}

/// Raison de la fin d'un tour du modèle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    /// Le modèle a fini de répondre.
    EndTurn,
    /// Le modèle attend le résultat d'un ou plusieurs outils.
    ToolUse,
    /// La sortie a été tronquée par `max_tokens`.
    MaxTokens,
    /// Le fournisseur a refusé de répondre (filtres de sécurité).
    Refusal,
    /// Toute autre raison (pause, filtre de contenu…).
    Other,
}

/// Jetons consommés.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Jetons d'entrée, cache compris.
    pub input_tokens: u64,
    /// Jetons produits.
    pub output_tokens: u64,
}

impl Usage {
    /// Cumule une autre mesure.
    pub fn add(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// Évènement d'un flux de réponse, dans l'ordre d'arrivée.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StreamEvent {
    /// Fragment de texte de la réponse.
    TextDelta {
        /// Texte à ajouter.
        text: String,
    },
    /// Fragment du raisonnement, quand le fournisseur le fournit.
    ThinkingDelta {
        /// Texte à ajouter.
        text: String,
    },
    /// Le modèle commence à composer un appel d'outil.
    ToolCallStart {
        /// Identifiant de l'appel.
        id: String,
        /// Nom de l'outil.
        name: String,
    },
    /// Fragment des arguments JSON d'un appel d'outil.
    ToolCallDelta {
        /// Identifiant de l'appel.
        id: String,
        /// Fragment JSON brut.
        partial_json: String,
    },
    /// Appel d'outil complet, arguments décodés.
    ToolCall {
        /// Identifiant de l'appel.
        id: String,
        /// Nom de l'outil.
        name: String,
        /// Arguments.
        input: Value,
    },
    /// Résultat d'un outil exécuté par l'application.
    ToolResult {
        /// Identifiant de l'appel.
        call_id: String,
        /// Nom de l'outil.
        name: String,
        /// Résultat.
        content: String,
        /// Vrai si l'outil a échoué.
        is_error: bool,
    },
    /// Fin d'un tour du modèle.
    TurnEnd {
        /// Pourquoi le tour s'est arrêté.
        stop_reason: StopReason,
        /// Consommation du tour.
        usage: Usage,
        /// Modèle qui a réellement répondu (peut différer en cas de repli).
        model: Option<String>,
        /// Catégorie du refus, s'il y en a un.
        refusal_category: Option<String>,
    },
    /// Erreur survenue en cours de route ; le flux s'arrête.
    Error {
        /// Message lisible.
        message: String,
    },
}

/// Tour complet du modèle, reconstitué à partir du flux.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    /// Fragments produits (texte et appels d'outils).
    pub parts: Vec<Part>,
    /// Raison de l'arrêt.
    pub stop_reason: StopReason,
    /// Consommation.
    pub usage: Usage,
    /// Modèle ayant répondu.
    pub model: Option<String>,
    /// Catégorie du refus, le cas échéant.
    pub refusal_category: Option<String>,
}

/// Requête de complétion.
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    /// Instructions système (persona, contexte du cluster).
    pub system: String,
    /// Historique complet, du plus ancien au plus récent.
    pub messages: Vec<ChatMessage>,
    /// Outils proposés ; vide = aucun.
    pub tools: Vec<ToolSpec>,
    /// Plafond de jetons produits ; 0 = valeur par défaut du fournisseur.
    pub max_output_tokens: u32,
    /// Demander au fournisseur d'exposer son raisonnement (Claude).
    pub show_thinking: bool,
}

/// Raccourcit un texte pour un message d'erreur.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texte_et_appels_d_un_message() {
        let m = ChatMessage {
            role: Role::Assistant,
            parts: vec![
                Part::Text {
                    text: "Je regarde.".into(),
                },
                Part::ToolCall {
                    id: "c1".into(),
                    name: "list_pods".into(),
                    input: serde_json::json!({"namespace": "default"}),
                },
            ],
        };
        assert_eq!(m.text(), "Je regarde.");
        let calls = m.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "list_pods");
    }

    #[test]
    fn serialisation_camel_case_des_evenements() {
        let ev = StreamEvent::ToolCallDelta {
            id: "c1".into(),
            partial_json: "{\"a\"".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"toolCallDelta\""), "{json}");
        assert!(json.contains("\"partialJson\""), "{json}");

        let p = Part::ToolResult {
            call_id: "c1".into(),
            name: "x".into(),
            content: "ok".into(),
            is_error: false,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"callId\""), "{json}");
        assert!(json.contains("\"isError\""), "{json}");
    }

    #[test]
    fn troncature() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdefgh", 3), "abc…");
    }
}
