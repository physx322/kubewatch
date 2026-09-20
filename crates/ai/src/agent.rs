//! Boucle d'agent : tours du modèle et exécution des outils, jusqu'à la
//! réponse finale ou au plafond de tours.

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

use crate::error::Result;
use crate::message::{ChatMessage, ChatRequest, Part, Role, StopReason, StreamEvent, Usage};
use crate::provider::{Provider, Sink};

/// Exécuteur d'outils fourni par l'application.
///
/// `Ok(texte)` est renvoyé au modèle comme résultat ; `Err(texte)` comme
/// résultat en erreur (le modèle peut alors corriger ses arguments).
pub trait ToolExecutor: Send + Sync {
    /// Exécute l'outil `name` avec les arguments `input`.
    fn call<'a>(
        &'a self,
        name: &'a str,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<String, String>> + Send + 'a>>;
}

/// Réglages d'une exécution.
#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    /// Nombre maximal de tours du modèle (chaque tour peut appeler des outils).
    pub max_rounds: u32,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self { max_rounds: 8 }
    }
}

/// Résultat d'une exécution.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    /// Messages produits, à ajouter à l'historique : tours de l'assistant et
    /// messages de résultats d'outils, dans l'ordre.
    pub messages: Vec<ChatMessage>,
    /// Raison de l'arrêt du dernier tour.
    pub stop_reason: StopReason,
    /// Consommation cumulée.
    pub usage: Usage,
    /// Nombre de tours effectués.
    pub rounds: u32,
    /// Modèle ayant répondu au dernier tour.
    pub model: Option<String>,
    /// Catégorie du refus, le cas échéant.
    pub refusal_category: Option<String>,
}

/// Mène la conversation jusqu'à une réponse finale.
///
/// Les outils ne sont proposés au modèle que si un exécuteur est fourni. Au
/// plafond de tours, les appels en attente reçoivent un résultat d'erreur et la
/// boucle s'arrête sans relancer le modèle.
pub async fn run(
    provider: &Provider,
    request: &ChatRequest,
    executor: Option<&dyn ToolExecutor>,
    opts: RunOptions,
    sink: &Sink<'_>,
) -> Result<RunOutcome> {
    let max_rounds = opts.max_rounds.max(1);
    let mut history = request.messages.clone();
    let mut appended = Vec::new();
    let mut usage = Usage::default();
    let mut rounds = 0u32;

    loop {
        rounds += 1;
        let req = ChatRequest {
            system: request.system.clone(),
            messages: history.clone(),
            tools: if executor.is_some() {
                request.tools.clone()
            } else {
                Vec::new()
            },
            max_output_tokens: request.max_output_tokens,
            show_thinking: request.show_thinking,
        };
        let turn = provider.stream_turn(&req, sink).await?;
        usage.add(turn.usage);
        sink(StreamEvent::TurnEnd {
            stop_reason: turn.stop_reason,
            usage: turn.usage,
            model: turn.model.clone(),
            refusal_category: turn.refusal_category.clone(),
        });

        let assistant = ChatMessage {
            role: Role::Assistant,
            parts: turn.parts,
        };
        let calls: Vec<(String, String, Value)> = assistant
            .tool_calls()
            .into_iter()
            .map(|(id, name, input)| (id.to_string(), name.to_string(), input.clone()))
            .collect();
        history.push(assistant.clone());
        appended.push(assistant);

        let done = |stop_reason: StopReason, appended: Vec<ChatMessage>| RunOutcome {
            messages: appended,
            stop_reason,
            usage,
            rounds,
            model: turn.model.clone(),
            refusal_category: turn.refusal_category.clone(),
        };

        if calls.is_empty() {
            return Ok(done(turn.stop_reason, appended));
        }
        let Some(exec) = executor else {
            return Ok(done(turn.stop_reason, appended));
        };

        let mut results = Vec::with_capacity(calls.len());
        let exhausted = rounds >= max_rounds;
        for (id, name, input) in calls {
            let (content, is_error) = if exhausted {
                (
                    format!(
                        "Plafond de {max_rounds} tours d'outils atteint : l'appel n'a pas été \
                         exécuté. Réponds avec ce que tu sais déjà."
                    ),
                    true,
                )
            } else {
                match exec.call(&name, input).await {
                    Ok(c) => (c, false),
                    Err(e) => (e, true),
                }
            };
            sink(StreamEvent::ToolResult {
                call_id: id.clone(),
                name: name.clone(),
                content: content.clone(),
                is_error,
            });
            results.push(Part::ToolResult {
                call_id: id,
                name,
                content,
                is_error,
            });
        }
        let msg = ChatMessage {
            role: Role::User,
            parts: results,
        };
        history.push(msg.clone());
        appended.push(msg);

        if exhausted {
            sink(StreamEvent::Error {
                message: format!(
                    "Nombre maximal de tours d'outils atteint ({max_rounds}) : la réponse est \
                     interrompue. Reposez la question, ou augmentez la limite dans les réglages."
                ),
            });
            return Ok(done(StopReason::Other, appended));
        }
    }
}
