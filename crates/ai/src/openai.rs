//! Fournisseur OpenAI et compatibles : API Chat Completions en flux.
//!
//! Le même client sert pour OpenAI et pour les serveurs locaux qui imitent son
//! API (LM Studio, Ollama, llama.cpp, Jan, vLLM…). Différences prises en
//! compte : les serveurs locaux acceptent `max_tokens` plutôt que
//! `max_completion_tokens`, omettent parfois l'identifiant des appels
//! d'outils, et exposent le raisonnement dans `reasoning_content`.

use std::collections::BTreeMap;

use futures::StreamExt;
use serde_json::{json, Value};

use crate::anthropic::parse_tool_input;
use crate::config::{ProviderKind, ProviderProfile};
use crate::error::{Error, Result};
use crate::message::{ChatMessage, ChatRequest, Part, Role, StopReason, StreamEvent, Turn, Usage};
use crate::provider::{ensure_success, error_message, http_client, ModelInfo, Sink};
use crate::sse::SseParser;

/// Client OpenAI ou compatible.
#[derive(Debug, Clone)]
pub struct OpenAiClient {
    http: reqwest::Client,
    base: String,
    api_key: Option<String>,
    model: String,
    /// Vrai pour un serveur compatible (dialecte « classique »).
    compat: bool,
}

impl OpenAiClient {
    /// Construit le client. La clé est obligatoire pour OpenAI, facultative
    /// pour un serveur compatible.
    pub fn new(profile: &ProviderProfile) -> Result<Self> {
        let api_key = profile
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string);
        let compat = profile.kind == ProviderKind::OpenAiCompatible;
        if !compat && api_key.is_none() {
            return Err(Error::Config(format!(
                "le profil « {} » n'a pas de clé d'API OpenAI",
                profile.name
            )));
        }
        let model = profile.model.trim().to_string();
        if model.is_empty() {
            return Err(Error::Config(format!(
                "le profil « {} » ne précise pas de modèle : choisissez-en un dans la liste",
                profile.name
            )));
        }
        Ok(Self {
            http: http_client()?,
            base: profile.endpoint(),
            api_key,
            model,
            compat,
        })
    }

    /// Modèle demandé.
    pub fn model(&self) -> &str {
        &self.model
    }

    fn auth(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(k) => r.bearer_auth(k),
            None => r,
        }
    }

    /// Modèles exposés par le serveur.
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let resp = self
            .auth(self.http.get(format!("{}/models", self.base)))
            .send()
            .await?;
        let resp = ensure_success(resp).await?;
        let body: Value = resp.json().await?;
        let data = body
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Protocol("liste de modèles sans champ data".to_string()))?;
        Ok(data
            .iter()
            .filter_map(|m| {
                let id = m.get("id")?.as_str()?.to_string();
                let display_name = m
                    .get("display_name")
                    .or_else(|| m.get("name"))
                    .and_then(Value::as_str)
                    .filter(|n| *n != id)
                    .map(str::to_string);
                Some(ModelInfo { id, display_name })
            })
            .collect())
    }

    /// Un tour de conversation en flux.
    pub async fn stream_turn(&self, req: &ChatRequest, sink: &Sink<'_>) -> Result<Turn> {
        let body = build_request(&self.model, self.compat, req);
        let resp = self
            .auth(self.http.post(format!("{}/chat/completions", self.base)))
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await?;
        let resp = ensure_success(resp).await?;

        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::new();
        let mut acc = Accumulator::default();
        'outer: while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            for ev in parser.push(&chunk) {
                if acc.handle(&ev.data, sink)? {
                    break 'outer;
                }
            }
        }
        if let Some(ev) = parser.finish() {
            acc.handle(&ev.data, sink)?;
        }
        Ok(acc.finish(sink))
    }
}

/// Corps de la requête `/chat/completions`.
pub fn build_request(model: &str, compat: bool, req: &ChatRequest) -> Value {
    let max_tokens = if req.max_output_tokens == 0 {
        if compat {
            4_096
        } else {
            32_000
        }
    } else {
        req.max_output_tokens
    };
    let mut messages = Vec::new();
    if !req.system.trim().is_empty() {
        messages.push(json!({ "role": "system", "content": req.system }));
    }
    messages.extend(to_messages(&req.messages));

    let mut body = json!({
        "model": model,
        "stream": true,
        "stream_options": { "include_usage": true },
        "messages": messages,
    });
    if compat {
        body["max_tokens"] = json!(max_tokens);
    } else {
        body["max_completion_tokens"] = json!(max_tokens);
    }
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema,
                        }
                    })
                })
                .collect(),
        );
    }
    body
}

/// Convertit l'historique en messages OpenAI.
///
/// Les résultats d'outils deviennent des messages `tool`, placés avant tout
/// texte utilisateur du même message : l'API exige qu'ils suivent
/// immédiatement l'appel de l'assistant.
pub fn to_messages(messages: &[ChatMessage]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in messages {
        match m.role {
            Role::User => {
                let mut text = String::new();
                for p in &m.parts {
                    match p {
                        Part::ToolResult {
                            call_id, content, ..
                        } => out.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": if content.is_empty() { "(vide)" } else { content.as_str() },
                        })),
                        Part::Text { text: t } => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                        Part::ToolCall { .. } => {}
                    }
                }
                if !text.trim().is_empty() {
                    out.push(json!({ "role": "user", "content": text }));
                }
            }
            Role::Assistant => {
                let text = m.text();
                let calls: Vec<Value> = m
                    .tool_calls()
                    .into_iter()
                    .map(|(id, name, input)| {
                        json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": input.to_string() },
                        })
                    })
                    .collect();
                if text.trim().is_empty() && calls.is_empty() {
                    continue;
                }
                let mut msg = json!({
                    "role": "assistant",
                    "content": if text.trim().is_empty() { Value::Null } else { Value::String(text) },
                });
                if !calls.is_empty() {
                    msg["tool_calls"] = Value::Array(calls);
                }
                out.push(msg);
            }
        }
    }
    out
}

/// Appel d'outil en cours de réception.
#[derive(Debug, Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
    started: bool,
}

/// Reconstitue un tour à partir des morceaux du flux.
#[derive(Debug, Default)]
struct Accumulator {
    text: String,
    calls: BTreeMap<u64, PendingCall>,
    finish_reason: Option<String>,
    usage: Usage,
    model: Option<String>,
}

impl Accumulator {
    /// Traite un évènement ; renvoie `true` à `[DONE]`.
    fn handle(&mut self, data: &str, sink: &Sink<'_>) -> Result<bool> {
        if data.trim() == "[DONE]" {
            return Ok(true);
        }
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                return Err(Error::Protocol(format!(
                    "morceau SSE illisible ({e}) : {}",
                    crate::message::truncate(data, 200)
                )))
            }
        };
        if let Some(err) = v.get("error") {
            let message = error_message(&err.to_string());
            return Err(Error::Api { status: 0, message });
        }
        if let Some(m) = v.get("model").and_then(Value::as_str) {
            if self.model.is_none() {
                self.model = Some(m.to_string());
            }
        }
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            let prompt = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
            let completion = u
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if prompt > 0 {
                self.usage.input_tokens = prompt;
            }
            if completion > 0 {
                self.usage.output_tokens = completion;
            }
        }
        let Some(choice) = v
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        else {
            return Ok(false);
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_string());
        }
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                self.text.push_str(text);
                sink(StreamEvent::TextDelta {
                    text: text.to_string(),
                });
            }
        }
        for key in ["reasoning_content", "reasoning"] {
            if let Some(text) = delta.get(key).and_then(Value::as_str) {
                if !text.is_empty() {
                    sink(StreamEvent::ThinkingDelta {
                        text: text.to_string(),
                    });
                }
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for (pos, tc) in calls.iter().enumerate() {
                let index = tc
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(pos as u64);
                let entry = self.calls.entry(index).or_default();
                if let Some(id) = tc.get("id").and_then(Value::as_str) {
                    if entry.id.is_empty() {
                        entry.id = id.to_string();
                    }
                }
                let func = tc.get("function").cloned().unwrap_or(Value::Null);
                if let Some(name) = func.get("name").and_then(Value::as_str) {
                    if entry.name.is_empty() {
                        entry.name = name.to_string();
                    }
                }
                if !entry.started && !entry.name.is_empty() {
                    if entry.id.is_empty() {
                        entry.id = format!("call_{index}_{}", short_uuid());
                    }
                    entry.started = true;
                    sink(StreamEvent::ToolCallStart {
                        id: entry.id.clone(),
                        name: entry.name.clone(),
                    });
                }
                if let Some(args) = func.get("arguments").and_then(Value::as_str) {
                    if !args.is_empty() {
                        entry.arguments.push_str(args);
                        if entry.started {
                            sink(StreamEvent::ToolCallDelta {
                                id: entry.id.clone(),
                                partial_json: args.to_string(),
                            });
                        }
                    }
                }
            }
        }
        Ok(false)
    }

    fn finish(self, sink: &Sink<'_>) -> Turn {
        let mut parts = Vec::new();
        if !self.text.is_empty() {
            parts.push(Part::Text { text: self.text });
        }
        for (index, mut call) in self.calls {
            if call.name.is_empty() {
                continue;
            }
            if call.id.is_empty() {
                call.id = format!("call_{index}_{}", short_uuid());
            }
            let input = parse_tool_input(&call.arguments);
            sink(StreamEvent::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                input: input.clone(),
            });
            parts.push(Part::ToolCall {
                id: call.id,
                name: call.name,
                input,
            });
        }
        let has_calls = parts.iter().any(|p| matches!(p, Part::ToolCall { .. }));
        let stop_reason = match self.finish_reason.as_deref() {
            _ if has_calls => StopReason::ToolUse,
            Some("stop") | None => StopReason::EndTurn,
            Some("length") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::Refusal,
            Some(_) => StopReason::Other,
        };
        Turn {
            parts,
            stop_reason,
            usage: self.usage,
            model: self.model,
            refusal_category: None,
        }
    }
}

fn short_uuid() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolSpec;

    #[test]
    fn requete_openai_et_compatible() {
        let req = ChatRequest {
            system: "Système.".into(),
            messages: vec![ChatMessage::user("Salut")],
            tools: vec![ToolSpec {
                name: "get_events".into(),
                description: "évènements".into(),
                input_schema: json!({"type":"object","properties":{"namespace":{"type":"string"}}}),
            }],
            max_output_tokens: 0,
            show_thinking: false,
        };
        let b = build_request("gpt-5", false, &req);
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1]["content"], "Salut");
        assert_eq!(b["max_completion_tokens"], 32_000);
        assert!(b.get("max_tokens").is_none());
        assert_eq!(b["tools"][0]["type"], "function");
        assert_eq!(b["tools"][0]["function"]["name"], "get_events");
        assert_eq!(b["stream_options"]["include_usage"], true);

        let b = build_request("qwen2.5", true, &req);
        assert_eq!(b["max_tokens"], 4_096);
        assert!(b.get("max_completion_tokens").is_none());
    }

    #[test]
    fn conversion_des_messages_avec_outils() {
        let msgs = vec![
            ChatMessage::user("Q"),
            ChatMessage {
                role: Role::Assistant,
                parts: vec![Part::ToolCall {
                    id: "call_1".into(),
                    name: "list".into(),
                    input: json!({"kind":"pods"}),
                }],
            },
            ChatMessage {
                role: Role::User,
                parts: vec![
                    Part::ToolResult {
                        call_id: "call_1".into(),
                        name: "list".into(),
                        content: "".into(),
                        is_error: false,
                    },
                    Part::Text {
                        text: "merci".into(),
                    },
                ],
            },
            ChatMessage::assistant("Voilà."),
        ];
        let out = to_messages(&msgs);
        assert_eq!(out.len(), 5);
        assert_eq!(out[1]["role"], "assistant");
        assert!(out[1]["content"].is_null());
        assert_eq!(
            out[1]["tool_calls"][0]["function"]["arguments"],
            r#"{"kind":"pods"}"#
        );
        assert_eq!(out[2]["role"], "tool");
        assert_eq!(out[2]["content"], "(vide)");
        assert_eq!(out[3]["role"], "user");
        assert_eq!(out[4]["content"], "Voilà.");
    }

    #[test]
    fn accumulation_d_un_flux_avec_appel_d_outil() {
        let collected = std::sync::Mutex::new(Vec::new());
        let sink = |e: StreamEvent| collected.lock().unwrap().push(e);
        let mut acc = Accumulator::default();
        let script = [
            r#"{"id":"x","model":"gpt-5","choices":[{"index":0,"delta":{"role":"assistant","content":"Je "},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"vérifie."},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_9","type":"function","function":{"name":"list","arguments":""}}]},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"kind\""}}]},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"pods\"}"}}]},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":7}}"#,
            "[DONE]",
        ];
        let mut done = false;
        for s in script {
            done = acc.handle(s, &sink).unwrap();
        }
        assert!(done);
        let turn = acc.finish(&sink);
        assert_eq!(turn.stop_reason, StopReason::ToolUse);
        assert_eq!(turn.usage.input_tokens, 20);
        assert_eq!(turn.usage.output_tokens, 7);
        assert_eq!(turn.model.as_deref(), Some("gpt-5"));
        assert_eq!(
            turn.parts[0],
            Part::Text {
                text: "Je vérifie.".into()
            }
        );
        assert_eq!(
            turn.parts[1],
            Part::ToolCall {
                id: "call_9".into(),
                name: "list".into(),
                input: json!({"kind":"pods"})
            }
        );
        let kinds: Vec<&str> = collected
            .lock()
            .unwrap()
            .iter()
            .map(|e| match e {
                StreamEvent::TextDelta { .. } => "text",
                StreamEvent::ToolCallStart { .. } => "start",
                StreamEvent::ToolCallDelta { .. } => "delta",
                StreamEvent::ToolCall { .. } => "call",
                _ => "autre",
            })
            .collect();
        assert_eq!(kinds, ["text", "text", "start", "delta", "delta", "call"]);
    }

    #[test]
    fn appel_sans_identifiant_et_raisonnement() {
        let collected = std::sync::Mutex::new(Vec::new());
        let sink = |e: StreamEvent| collected.lock().unwrap().push(e);
        let mut acc = Accumulator::default();
        acc.handle(
            r#"{"choices":[{"delta":{"reasoning_content":"hmm","tool_calls":[{"function":{"name":"overview","arguments":"{}"}}]}}]}"#,
            &sink,
        )
        .unwrap();
        acc.handle(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            &sink,
        )
        .unwrap();
        let turn = acc.finish(&sink);
        assert_eq!(
            turn.stop_reason,
            StopReason::ToolUse,
            "un appel présent prime sur stop"
        );
        match &turn.parts[0] {
            Part::ToolCall { id, name, .. } => {
                assert!(id.starts_with("call_0_"));
                assert_eq!(name, "overview");
            }
            other => panic!("inattendu : {other:?}"),
        }
        assert!(matches!(
            collected.lock().unwrap()[0],
            StreamEvent::ThinkingDelta { .. }
        ));
    }

    #[test]
    fn erreur_dans_le_flux_et_troncature() {
        let sink = |_e: StreamEvent| {};
        let mut acc = Accumulator::default();
        let err = acc
            .handle(
                r#"{"error":{"message":"context length exceeded","type":"server_error"}}"#,
                &sink,
            )
            .unwrap_err();
        assert!(
            matches!(err, Error::Api { status: 0, ref message } if message == "context length exceeded")
        );

        let mut acc = Accumulator::default();
        acc.handle(
            r#"{"choices":[{"delta":{"content":"a"},"finish_reason":"length"}]}"#,
            &sink,
        )
        .unwrap();
        assert_eq!(acc.finish(&sink).stop_reason, StopReason::MaxTokens);
    }
}
