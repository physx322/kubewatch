//! Fournisseur Anthropic : API Messages en flux, appels d'outils, refus.
//!
//! Référence : `POST {base}/v1/messages` avec `stream: true`, en-têtes
//! `x-api-key` et `anthropic-version: 2023-06-01`. Le flux SSE enchaîne
//! `message_start`, des blocs (`content_block_start` / `_delta` / `_stop`),
//! `message_delta` (raison d'arrêt, jetons produits) puis `message_stop`.
//!
//! Le profil peut aussi emprunter le compte de Claude Code ([`crate::claude_code`]) :
//! le jeton part alors en `Authorization: Bearer`, avec l'en-tête bêta
//! `anthropic-beta: oauth-2025-04-20`, à la place de `x-api-key`.

use std::collections::BTreeMap;

use futures::StreamExt;
use serde_json::{json, Value};

use crate::claude_code::{self, CredentialKind};
use crate::config::ProviderProfile;
use crate::error::{Error, Result};
use crate::message::{ChatMessage, ChatRequest, Part, Role, StopReason, StreamEvent, Turn, Usage};
use crate::provider::{ensure_success, error_message, http_client, ModelInfo, Sink};
use crate::sse::SseParser;

/// Version d'API demandée.
pub const API_VERSION: &str = "2023-06-01";

/// En-tête bêta du repli serveur en cas de refus (`fallbacks: "default"`).
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// Manière de présenter l'identifiant au serveur.
#[derive(Clone)]
enum Auth {
    /// Clé d'API classique : en-tête `x-api-key`.
    ApiKey(String),
    /// Jeton de compte : `Authorization: Bearer` et en-tête bêta OAuth.
    Bearer(String),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::ApiKey(_) => f.write_str("ApiKey(masquée)"),
            Auth::Bearer(_) => f.write_str("Bearer(masqué)"),
        }
    }
}

/// Client Anthropic.
#[derive(Debug, Clone)]
pub struct AnthropicClient {
    http: reqwest::Client,
    base: String,
    auth: Auth,
    model: String,
}

impl AnthropicClient {
    /// Construit le client ; l'identifiant et le modèle sont obligatoires.
    ///
    /// L'identifiant vient du compte de Claude Code quand le profil le demande,
    /// et il est relu à chaque construction : Claude Code renouvelle son jeton
    /// dans notre dos, on prend toujours le dernier écrit sur le disque.
    pub fn new(profile: &ProviderProfile) -> Result<Self> {
        let auth = if profile.uses_claude_code() {
            let c = claude_code::credential().map_err(|e| match e {
                Error::Config(m) => Error::Config(format!(
                    "le profil « {} » emprunte le compte de Claude Code, mais {m}",
                    profile.name
                )),
                other => other,
            })?;
            match c.kind {
                CredentialKind::Oauth => Auth::Bearer(c.token),
                CredentialKind::ApiKey => Auth::ApiKey(c.token),
            }
        } else {
            Auth::ApiKey(
                profile
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .ok_or_else(|| {
                        Error::Config(format!(
                            "le profil « {} » n'a pas de clé d'API Anthropic",
                            profile.name
                        ))
                    })?
                    .to_string(),
            )
        };
        let model = profile.model.trim().to_string();
        if model.is_empty() {
            return Err(Error::Config(format!(
                "le profil « {} » ne précise pas de modèle",
                profile.name
            )));
        }
        Ok(Self {
            http: http_client()?,
            base: profile.endpoint(),
            auth,
            model,
        })
    }

    /// Modèle demandé.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// En-têtes communs. `fallback` n'est vrai que pour `/v1/messages`, seul
    /// endroit où le repli serveur a un sens.
    ///
    /// Les bêtas voyagent dans un seul en-tête séparé par des virgules :
    /// deux `anthropic-beta` distincts ne sont pas garantis d'être fusionnés.
    fn headers(&self, r: reqwest::RequestBuilder, fallback: bool) -> reqwest::RequestBuilder {
        let r = match &self.auth {
            Auth::ApiKey(k) => r.header("x-api-key", k),
            Auth::Bearer(t) => r.header("authorization", format!("Bearer {t}")),
        }
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json");
        match self.betas(fallback) {
            Some(v) => r.header("anthropic-beta", v),
            None => r,
        }
    }

    /// Bêtas à annoncer pour cette requête.
    fn betas(&self, fallback: bool) -> Option<String> {
        let mut betas: Vec<&str> = Vec::new();
        if matches!(self.auth, Auth::Bearer(_)) {
            betas.push(claude_code::OAUTH_BETA);
        }
        if fallback && wants_fallbacks(&self.model) {
            betas.push(FALLBACK_BETA);
        }
        (!betas.is_empty()).then(|| betas.join(","))
    }

    /// Modèles accessibles avec cette clé.
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/v1/models?limit=1000", self.base);
        let resp = self.headers(self.http.get(url), false).send().await?;
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
                    .and_then(Value::as_str)
                    .map(str::to_string);
                Some(ModelInfo { id, display_name })
            })
            .collect())
    }

    /// Un tour de conversation en flux.
    pub async fn stream_turn(&self, req: &ChatRequest, sink: &Sink<'_>) -> Result<Turn> {
        let body = build_request(&self.model, req);
        let r = self
            .headers(self.http.post(format!("{}/v1/messages", self.base)), true)
            .header("accept", "text/event-stream");
        let resp = r.json(&body).send().await?;
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
        Ok(acc.finish())
    }
}

/// Vrai pour les modèles où le repli serveur en cas de refus est défini.
pub fn wants_fallbacks(model: &str) -> bool {
    let m = model.trim();
    m.starts_with("claude-opus-5")
        || m.starts_with("claude-fable")
        || m.starts_with("claude-mythos")
}

/// Corps de la requête `/v1/messages`.
pub fn build_request(model: &str, req: &ChatRequest) -> Value {
    let max_tokens = if req.max_output_tokens == 0 {
        32_000
    } else {
        req.max_output_tokens
    };
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": to_messages(&req.messages),
    });
    if !req.system.trim().is_empty() {
        body["system"] = json!(req.system);
    }
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema,
                    })
                })
                .collect(),
        );
    }
    if req.show_thinking {
        body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
    }
    if wants_fallbacks(model) {
        body["fallbacks"] = json!("default");
    }
    body
}

/// Convertit l'historique en messages Anthropic.
///
/// Les messages consécutifs de même rôle sont fusionnés, les textes vides
/// écartés (l'API refuse un bloc de texte vide) et les résultats d'outils
/// deviennent des blocs `tool_result` d'un message utilisateur.
pub fn to_messages(messages: &[ChatMessage]) -> Vec<Value> {
    let mut out: Vec<(Role, Vec<Value>)> = Vec::new();
    for m in messages {
        let blocks: Vec<Value> = m.parts.iter().filter_map(part_to_block).collect();
        if blocks.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some((role, existing)) if *role == m.role => existing.extend(blocks),
            _ => out.push((m.role, blocks)),
        }
    }
    out.into_iter()
        .map(|(role, content)| {
            json!({
                "role": match role { Role::User => "user", Role::Assistant => "assistant" },
                "content": content,
            })
        })
        .collect()
}

fn part_to_block(p: &Part) -> Option<Value> {
    match p {
        Part::Text { text } => {
            if text.trim().is_empty() {
                None
            } else {
                Some(json!({ "type": "text", "text": text }))
            }
        }
        Part::ToolCall { id, name, input } => Some(json!({
            "type": "tool_use", "id": id, "name": name, "input": input,
        })),
        Part::ToolResult {
            call_id,
            content,
            is_error,
            ..
        } => {
            let mut b = json!({
                "type": "tool_result",
                "tool_use_id": call_id,
                "content": if content.is_empty() { "(vide)" } else { content.as_str() },
            });
            if *is_error {
                b["is_error"] = json!(true);
            }
            Some(b)
        }
    }
}

/// Bloc de contenu en cours de réception.
#[derive(Debug)]
enum Block {
    Text(String),
    Tool {
        id: String,
        name: String,
        json: String,
    },
    Ignored,
}

/// Reconstitue un tour à partir des évènements du flux.
#[derive(Debug, Default)]
struct Accumulator {
    blocks: BTreeMap<u64, Block>,
    parts: Vec<Part>,
    stop_reason: Option<StopReason>,
    usage: Usage,
    model: Option<String>,
    refusal_category: Option<String>,
}

impl Accumulator {
    /// Traite un évènement ; renvoie `true` quand le message est terminé.
    fn handle(&mut self, data: &str, sink: &Sink<'_>) -> Result<bool> {
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                return Err(Error::Protocol(format!(
                    "évènement SSE illisible ({e}) : {}",
                    crate::message::truncate(data, 200)
                )))
            }
        };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "message_start" => {
                let msg = v.get("message").cloned().unwrap_or(Value::Null);
                self.model = msg.get("model").and_then(Value::as_str).map(str::to_string);
                if let Some(u) = msg.get("usage") {
                    self.usage.input_tokens = u64_of(u, "input_tokens")
                        + u64_of(u, "cache_read_input_tokens")
                        + u64_of(u, "cache_creation_input_tokens");
                }
            }
            "content_block_start" => {
                let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                let cb = v.get("content_block").cloned().unwrap_or(Value::Null);
                let block = match cb.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => Block::Text(
                        cb.get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    ),
                    "tool_use" => {
                        let id = cb
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let name = cb
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        sink(StreamEvent::ToolCallStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        Block::Tool {
                            id,
                            name,
                            json: String::new(),
                        }
                    }
                    "fallback" => {
                        let from = cb
                            .pointer("/from/model")
                            .and_then(Value::as_str)
                            .unwrap_or("?");
                        let to = cb
                            .pointer("/to/model")
                            .and_then(Value::as_str)
                            .unwrap_or("?");
                        tracing::info!(de = from, vers = to, "repli de modèle après refus");
                        Block::Ignored
                    }
                    _ => Block::Ignored,
                };
                self.blocks.insert(index, block);
            }
            "content_block_delta" => {
                let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                let delta = v.get("delta").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        if let Some(Block::Text(t)) = self.blocks.get_mut(&index) {
                            t.push_str(text);
                        } else {
                            self.blocks.insert(index, Block::Text(text.to_string()));
                        }
                        if !text.is_empty() {
                            sink(StreamEvent::TextDelta {
                                text: text.to_string(),
                            });
                        }
                    }
                    "input_json_delta" => {
                        let partial = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if let Some(Block::Tool { id, json, .. }) = self.blocks.get_mut(&index) {
                            json.push_str(partial);
                            if !partial.is_empty() {
                                sink(StreamEvent::ToolCallDelta {
                                    id: id.clone(),
                                    partial_json: partial.to_string(),
                                });
                            }
                        }
                    }
                    "thinking_delta" => {
                        let text = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                        if !text.is_empty() {
                            sink(StreamEvent::ThinkingDelta {
                                text: text.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(block) = self.blocks.remove(&index) {
                    match block {
                        Block::Text(text) => {
                            if !text.is_empty() {
                                self.parts.push(Part::Text { text });
                            }
                        }
                        Block::Tool { id, name, json } => {
                            let input = parse_tool_input(&json);
                            sink(StreamEvent::ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                            });
                            self.parts.push(Part::ToolCall { id, name, input });
                        }
                        Block::Ignored => {}
                    }
                }
            }
            "message_delta" => {
                let delta = v.get("delta").cloned().unwrap_or(Value::Null);
                if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(map_stop(reason));
                }
                let details = delta
                    .get("stop_details")
                    .or_else(|| v.get("stop_details"))
                    .cloned()
                    .unwrap_or(Value::Null);
                if let Some(cat) = details.get("category").and_then(Value::as_str) {
                    self.refusal_category = Some(cat.to_string());
                }
                if let Some(u) = v.get("usage") {
                    let out = u64_of(u, "output_tokens");
                    if out > 0 {
                        self.usage.output_tokens = out;
                    }
                }
            }
            "message_stop" => return Ok(true),
            "error" => {
                let message = error_message(data);
                return Err(Error::Api { status: 0, message });
            }
            _ => {}
        }
        Ok(false)
    }

    fn finish(mut self) -> Turn {
        // Blocs jamais clos (flux coupé) : on garde ce qui est lisible.
        let leftovers: Vec<(u64, Block)> = std::mem::take(&mut self.blocks).into_iter().collect();
        for (_, b) in leftovers {
            match b {
                Block::Text(t) if !t.is_empty() => self.parts.push(Part::Text { text: t }),
                Block::Tool { id, name, json } if !name.is_empty() => {
                    self.parts.push(Part::ToolCall {
                        id,
                        name,
                        input: parse_tool_input(&json),
                    });
                }
                _ => {}
            }
        }
        let has_calls = self
            .parts
            .iter()
            .any(|p| matches!(p, Part::ToolCall { .. }));
        let stop_reason = match self.stop_reason {
            Some(s) => s,
            None if has_calls => StopReason::ToolUse,
            None => StopReason::EndTurn,
        };
        Turn {
            parts: self.parts,
            stop_reason,
            usage: self.usage,
            model: self.model,
            refusal_category: self.refusal_category,
        }
    }
}

fn u64_of(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn map_stop(reason: &str) -> StopReason {
    match reason {
        "end_turn" | "stop_sequence" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "refusal" => StopReason::Refusal,
        _ => StopReason::Other,
    }
}

/// Arguments d'un outil : JSON décodé, objet vide si rien n'a été reçu, texte
/// brut si le JSON est invalide (le modèle verra l'erreur au tour suivant).
pub(crate) fn parse_tool_input(raw: &str) -> Value {
    let t = raw.trim();
    if t.is_empty() {
        return json!({});
    }
    serde_json::from_str(t).unwrap_or_else(|_| Value::String(t.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolSpec;

    #[test]
    fn en_tetes_selon_l_identifiant() {
        let client = |auth: Auth| AnthropicClient {
            http: http_client().unwrap(),
            base: "https://api.anthropic.com".to_string(),
            auth,
            model: "claude-opus-5".to_string(),
        };

        let c = client(Auth::ApiKey("sk-ant-secret".into()));
        let r = c.headers(c.http.post("https://x/"), true).build().unwrap();
        assert_eq!(r.headers()["x-api-key"], "sk-ant-secret");
        assert!(!r.headers().contains_key("authorization"));
        assert_eq!(r.headers()["anthropic-version"], API_VERSION);
        assert_eq!(r.headers()["anthropic-beta"], FALLBACK_BETA);

        let c = client(Auth::Bearer("oat-secret".into()));
        let r = c.headers(c.http.post("https://x/"), true).build().unwrap();
        assert_eq!(r.headers()["authorization"], "Bearer oat-secret");
        assert!(!r.headers().contains_key("x-api-key"));
        assert_eq!(
            r.headers()["anthropic-beta"].to_str().unwrap(),
            format!("{},{}", claude_code::OAUTH_BETA, FALLBACK_BETA),
            "un seul en-tête, bêtas séparés par une virgule"
        );

        // Liste des modèles : pas de repli serveur, mais toujours la bêta OAuth.
        let r = c.headers(c.http.get("https://x/"), false).build().unwrap();
        assert_eq!(r.headers()["anthropic-beta"], claude_code::OAUTH_BETA);

        // Le Debug du client ne laisse pas fuir le secret dans les journaux.
        assert!(!format!("{c:?}").contains("oat-secret"));
    }

    #[test]
    fn requete_minimale_et_options() {
        let req = ChatRequest {
            system: String::new(),
            messages: vec![ChatMessage::user("Bonjour")],
            tools: vec![],
            max_output_tokens: 0,
            show_thinking: false,
        };
        let b = build_request("claude-haiku-4-5", &req);
        assert_eq!(b["max_tokens"], 32_000);
        assert!(b.get("system").is_none());
        assert!(b.get("tools").is_none());
        assert!(b.get("thinking").is_none());
        assert!(
            b.get("fallbacks").is_none(),
            "pas de repli hors Opus 5 / Fable"
        );
        assert_eq!(b["messages"][0]["role"], "user");
        assert_eq!(b["messages"][0]["content"][0]["text"], "Bonjour");

        let req = ChatRequest {
            system: "Tu es KubeWatch.".into(),
            tools: vec![ToolSpec {
                name: "list".into(),
                description: "liste".into(),
                input_schema: json!({"type":"object","properties":{}}),
            }],
            max_output_tokens: 1000,
            show_thinking: true,
            ..req
        };
        let b = build_request("claude-opus-5", &req);
        assert_eq!(b["system"], "Tu es KubeWatch.");
        assert_eq!(b["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(b["thinking"]["type"], "adaptive");
        assert_eq!(b["fallbacks"], "default");
        assert_eq!(b["max_tokens"], 1000);
        assert!(wants_fallbacks("claude-fable-5-1"));
        assert!(!wants_fallbacks("claude-sonnet-5"));
    }

    #[test]
    fn conversion_des_messages_avec_outils() {
        let msgs = vec![
            ChatMessage::user("Combien de pods ?"),
            ChatMessage {
                role: Role::Assistant,
                parts: vec![
                    Part::Text { text: "".into() },
                    Part::ToolCall {
                        id: "toolu_1".into(),
                        name: "list_resources".into(),
                        input: json!({"kind": "pods"}),
                    },
                ],
            },
            ChatMessage {
                role: Role::User,
                parts: vec![Part::ToolResult {
                    call_id: "toolu_1".into(),
                    name: "list_resources".into(),
                    content: "3 pods".into(),
                    is_error: false,
                }],
            },
            // Deux messages utilisateur consécutifs : fusionnés.
            ChatMessage::user("Et les nœuds ?"),
        ];
        let out = to_messages(&msgs);
        assert_eq!(out.len(), 3);
        assert_eq!(out[1]["role"], "assistant");
        assert_eq!(
            out[1]["content"].as_array().unwrap().len(),
            1,
            "texte vide écarté"
        );
        assert_eq!(out[1]["content"][0]["type"], "tool_use");
        assert_eq!(out[2]["role"], "user");
        assert_eq!(out[2]["content"][0]["type"], "tool_result");
        assert_eq!(out[2]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(out[2]["content"][1]["text"], "Et les nœuds ?");
    }

    #[test]
    fn accumulation_d_un_flux() {
        let events: Vec<StreamEvent> = {
            let collected = std::sync::Mutex::new(Vec::new());
            let sink = |e: StreamEvent| collected.lock().unwrap().push(e);
            let mut acc = Accumulator::default();
            let script = [
                r#"{"type":"message_start","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"cache_read_input_tokens":5}}}"#,
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Je "}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"regarde."}}"#,
                r#"{"type":"content_block_stop","index":0}"#,
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"list","input":{}}}"#,
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"kind\":"}}"#,
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"pods\"}"}}"#,
                r#"{"type":"content_block_stop","index":1}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42}}"#,
                r#"{"type":"message_stop"}"#,
            ];
            let mut done = false;
            for s in script {
                done = acc.handle(s, &sink).unwrap();
            }
            assert!(done);
            let turn = acc.finish();
            assert_eq!(turn.stop_reason, StopReason::ToolUse);
            assert_eq!(turn.usage.input_tokens, 15);
            assert_eq!(turn.usage.output_tokens, 42);
            assert_eq!(turn.model.as_deref(), Some("claude-opus-5"));
            assert_eq!(turn.parts.len(), 2);
            assert_eq!(
                turn.parts[1],
                Part::ToolCall {
                    id: "toolu_1".into(),
                    name: "list".into(),
                    input: json!({"kind": "pods"})
                }
            );
            collected.into_inner().unwrap()
        };
        let kinds: Vec<&str> = events
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
    fn refus_et_erreur_dans_le_flux() {
        let sink = |_e: StreamEvent| {};
        let mut acc = Accumulator::default();
        acc.handle(
            r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"}},"usage":{"output_tokens":0}}"#,
            &sink,
        )
        .unwrap();
        let turn = acc.finish();
        assert_eq!(turn.stop_reason, StopReason::Refusal);
        assert_eq!(turn.refusal_category.as_deref(), Some("cyber"));

        let mut acc = Accumulator::default();
        let err = acc
            .handle(
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
                &sink,
            )
            .unwrap_err();
        assert!(matches!(err, Error::Api { status: 0, ref message } if message == "Overloaded"));
    }

    #[test]
    fn arguments_d_outil_robustes() {
        assert_eq!(parse_tool_input(""), json!({}));
        assert_eq!(parse_tool_input(r#"{"a":1}"#), json!({"a": 1}));
        assert_eq!(parse_tool_input("{pas du json"), json!("{pas du json"));
    }
}
