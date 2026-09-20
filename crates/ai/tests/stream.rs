//! Tests de bout en bout : un faux serveur HTTP rejoue des flux SSE
//! enregistrés, et l'on vérifie la reconstitution des tours, l'exécution des
//! outils et le contenu des requêtes renvoyées au fournisseur.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use kubewatch_ai::{
    run, ChatMessage, ChatRequest, Error, Part, Provider, ProviderKind, ProviderProfile,
    RunOptions, StopReason, StreamEvent, ToolExecutor, ToolSpec,
};

/// Réponse HTTP rejouée par le faux serveur.
struct Canned {
    status: u16,
    content_type: &'static str,
    body: String,
}

fn sse(body: &str) -> Canned {
    Canned {
        status: 200,
        content_type: "text/event-stream",
        body: body.to_string(),
    }
}

/// Démarre un serveur qui répond aux requêtes dans l'ordre, puis s'arrête.
/// Renvoie l'adresse de base et les corps de requête capturés.
async fn fake_server(responses: Vec<Canned>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();
    tokio::spawn(async move {
        for r in responses {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut head_end = None;
            loop {
                let mut tmp = [0u8; 4096];
                let n = sock.read(&mut tmp).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if head_end.is_none() {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        head_end = Some(i + 4);
                    }
                }
                if let Some(h) = head_end {
                    let head = String::from_utf8_lossy(&buf[..h]).to_string();
                    let len = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            (k.trim().eq_ignore_ascii_case("content-length"))
                                .then(|| v.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if buf.len() >= h + len {
                        let body = &buf[h..h + len];
                        if !body.is_empty() {
                            if let Ok(v) = serde_json::from_slice::<Value>(body) {
                                cap.lock().unwrap().push(v);
                            }
                        }
                        break;
                    }
                }
            }
            let resp = format!(
                "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                r.status,
                r.content_type,
                r.body.len(),
                r.body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.shutdown().await.ok();
        }
    });
    (format!("http://{addr}"), captured)
}

fn profile(kind: ProviderKind, base: &str, model: &str) -> ProviderProfile {
    ProviderProfile {
        id: "p".into(),
        name: "test".into(),
        kind,
        base_url: Some(base.to_string()),
        api_key: Some("sk-test".into()),
        model: model.into(),
        max_output_tokens: Some(500),
        show_thinking: false,
    }
}

struct FakeTools;

impl ToolExecutor for FakeTools {
    fn call<'a>(
        &'a self,
        name: &'a str,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            match name {
                "list_resources" => Ok(format!(
                    "3 {} dans {}",
                    input["kind"].as_str().unwrap_or("?"),
                    input["namespace"].as_str().unwrap_or("tous les namespaces")
                )),
                other => Err(format!("outil inconnu : {other}")),
            }
        })
    }
}

fn tools() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "list_resources".into(),
        description: "Liste des objets".into(),
        input_schema: json!({"type":"object","properties":{"kind":{"type":"string"}}}),
    }]
}

fn kinds(events: &[StreamEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|e| match e {
            StreamEvent::TextDelta { .. } => "text",
            StreamEvent::ThinkingDelta { .. } => "think",
            StreamEvent::ToolCallStart { .. } => "start",
            StreamEvent::ToolCallDelta { .. } => "delta",
            StreamEvent::ToolCall { .. } => "call",
            StreamEvent::ToolResult { .. } => "result",
            StreamEvent::TurnEnd { .. } => "end",
            StreamEvent::Error { .. } => "error",
        })
        .collect()
}

const ANTHROPIC_TOOL_TURN: &str = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"model\":\"claude-opus-5\",\"usage\":{\"input_tokens\":12}}}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Je liste les pods.\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"list_resources\",\"input\":{}}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"kind\\\":\\\"pods\\\"}\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":30}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

const ANTHROPIC_FINAL_TURN: &str = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"m2\",\"model\":\"claude-opus-5\",\"usage\":{\"input_tokens\":40}}}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Il y a 3 pods.\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":8}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

#[tokio::test]
async fn anthropic_boucle_complete_avec_outil() {
    let (base, captured) =
        fake_server(vec![sse(ANTHROPIC_TOOL_TURN), sse(ANTHROPIC_FINAL_TURN)]).await;
    let provider =
        Provider::from_profile(&profile(ProviderKind::Anthropic, &base, "claude-opus-5")).unwrap();
    let events = Mutex::new(Vec::new());
    let sink = |e: StreamEvent| events.lock().unwrap().push(e);
    let req = ChatRequest {
        system: "Tu es KubeWatch.".into(),
        messages: vec![ChatMessage::user("Combien de pods ?")],
        tools: tools(),
        max_output_tokens: 500,
        show_thinking: false,
    };
    let out = run(
        &provider,
        &req,
        Some(&FakeTools),
        RunOptions::default(),
        &sink,
    )
    .await
    .unwrap();

    assert_eq!(out.rounds, 2);
    assert_eq!(out.stop_reason, StopReason::EndTurn);
    assert_eq!(out.usage.input_tokens, 52);
    assert_eq!(out.usage.output_tokens, 38);
    assert_eq!(out.messages.len(), 3);
    assert_eq!(out.messages[2].text(), "Il y a 3 pods.");
    match &out.messages[1].parts[0] {
        Part::ToolResult {
            call_id, content, ..
        } => {
            assert_eq!(call_id, "toolu_1");
            assert_eq!(content, "3 pods dans tous les namespaces");
        }
        other => panic!("inattendu : {other:?}"),
    }
    assert_eq!(
        kinds(&events.lock().unwrap()),
        ["text", "start", "delta", "call", "end", "result", "text", "end"]
    );

    // La seconde requête renvoie bien le résultat au modèle, et porte le repli.
    let reqs = captured.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0]["system"], "Tu es KubeWatch.");
    assert_eq!(reqs[0]["fallbacks"], "default");
    assert_eq!(reqs[0]["tools"][0]["name"], "list_resources");
    let second = &reqs[1]["messages"];
    assert_eq!(second.as_array().unwrap().len(), 3);
    assert_eq!(second[1]["content"][1]["type"], "tool_use");
    assert_eq!(second[2]["content"][0]["type"], "tool_result");
    assert_eq!(second[2]["content"][0]["tool_use_id"], "toolu_1");
}

const OPENAI_TOOL_TURN: &str = "data: {\"id\":\"c1\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Je regarde.\"},\"finish_reason\":null}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",\"function\":{\"name\":\"list_resources\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"kind\\\":\\\"pods\\\",\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"namespace\\\":\\\"prod\\\"}\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":15,\"completion_tokens\":9}}\n\n\
data: [DONE]\n\n";

const OPENAI_FINAL_TURN: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"3 pods en prod.\"},\"finish_reason\":null}]}\r\n\r\n\
data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n\
data: [DONE]\r\n\r\n";

#[tokio::test]
async fn openai_boucle_complete_avec_outil() {
    let (base, captured) = fake_server(vec![sse(OPENAI_TOOL_TURN), sse(OPENAI_FINAL_TURN)]).await;
    let provider = Provider::from_profile(&profile(ProviderKind::OpenAi, &base, "gpt-5")).unwrap();
    let events = Mutex::new(Vec::new());
    let sink = |e: StreamEvent| events.lock().unwrap().push(e);
    let req = ChatRequest {
        system: String::new(),
        messages: vec![ChatMessage::user("Pods en prod ?")],
        tools: tools(),
        max_output_tokens: 0,
        show_thinking: false,
    };
    let out = run(
        &provider,
        &req,
        Some(&FakeTools),
        RunOptions::default(),
        &sink,
    )
    .await
    .unwrap();
    assert_eq!(out.rounds, 2);
    assert_eq!(out.messages[2].text(), "3 pods en prod.");
    match &out.messages[1].parts[0] {
        Part::ToolResult { content, .. } => assert_eq!(content, "3 pods dans prod"),
        other => panic!("inattendu : {other:?}"),
    }
    assert_eq!(
        kinds(&events.lock().unwrap()),
        ["text", "start", "delta", "delta", "call", "end", "result", "text", "end"]
    );
    let reqs = captured.lock().unwrap();
    assert_eq!(reqs[0]["max_completion_tokens"], 32_000);
    assert!(reqs[0].get("system").is_none());
    let second = &reqs[1]["messages"];
    assert_eq!(second[1]["tool_calls"][0]["id"], "call_a");
    assert_eq!(second[2]["role"], "tool");
    assert_eq!(second[2]["tool_call_id"], "call_a");
}

#[tokio::test]
async fn compatible_sans_cle_et_liste_de_modeles() {
    let models = Canned {
        status: 200,
        content_type: "application/json",
        body: r#"{"object":"list","data":[{"id":"qwen2.5-7b","object":"model"},{"id":"llama-3","object":"model"}]}"#.into(),
    };
    let (base, _) = fake_server(vec![models]).await;
    let mut p = profile(ProviderKind::OpenAiCompatible, &base, "qwen2.5-7b");
    p.api_key = None;
    let provider = Provider::from_profile(&p).unwrap();
    let list = provider.list_models().await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, "qwen2.5-7b");
}

#[tokio::test]
async fn anthropic_liste_de_modeles() {
    let models = Canned {
        status: 200,
        content_type: "application/json",
        body: r#"{"data":[{"id":"claude-opus-5","display_name":"Claude Opus 5","type":"model"}],"has_more":false}"#.into(),
    };
    let (base, _) = fake_server(vec![models]).await;
    let provider =
        Provider::from_profile(&profile(ProviderKind::Anthropic, &base, "claude-opus-5")).unwrap();
    let list = provider.list_models().await.unwrap();
    assert_eq!(list[0].display_name.as_deref(), Some("Claude Opus 5"));
}

#[tokio::test]
async fn erreur_http_lisible() {
    let denied = Canned {
        status: 401,
        content_type: "application/json",
        body: r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#.into(),
    };
    let (base, _) = fake_server(vec![denied]).await;
    let provider =
        Provider::from_profile(&profile(ProviderKind::Anthropic, &base, "claude-opus-5")).unwrap();
    let sink = |_e: StreamEvent| {};
    let req = ChatRequest {
        messages: vec![ChatMessage::user("x")],
        ..Default::default()
    };
    let err = run(&provider, &req, None, RunOptions::default(), &sink)
        .await
        .unwrap_err();
    assert!(err.is_auth());
    assert!(
        matches!(err, Error::Api { status: 401, ref message } if message == "invalid x-api-key")
    );
}

#[tokio::test]
async fn plafond_de_tours_d_outils() {
    // Le modèle réclame un outil à chaque tour : avec max_rounds = 1, l'appel
    // n'est pas exécuté et la boucle s'arrête proprement.
    let (base, captured) = fake_server(vec![sse(ANTHROPIC_TOOL_TURN)]).await;
    let provider =
        Provider::from_profile(&profile(ProviderKind::Anthropic, &base, "claude-opus-5")).unwrap();
    let events = Mutex::new(Vec::new());
    let sink = |e: StreamEvent| events.lock().unwrap().push(e);
    let req = ChatRequest {
        messages: vec![ChatMessage::user("x")],
        tools: tools(),
        ..Default::default()
    };
    let out = run(
        &provider,
        &req,
        Some(&FakeTools),
        RunOptions { max_rounds: 1 },
        &sink,
    )
    .await
    .unwrap();
    assert_eq!(out.rounds, 1);
    assert_eq!(out.stop_reason, StopReason::Other);
    assert_eq!(
        captured.lock().unwrap().len(),
        1,
        "le modèle n'est pas relancé"
    );
    let evs = events.lock().unwrap();
    assert!(matches!(evs.last(), Some(StreamEvent::Error { .. })));
    assert!(evs
        .iter()
        .any(|e| matches!(e, StreamEvent::ToolResult { is_error: true, .. })));
}

#[tokio::test]
async fn profil_incomplet_refuse() {
    let mut p = profile(
        ProviderKind::Anthropic,
        "http://127.0.0.1:9",
        "claude-opus-5",
    );
    p.api_key = None;
    assert!(matches!(Provider::from_profile(&p), Err(Error::Config(_))));
    let mut p = profile(ProviderKind::OpenAiCompatible, "http://127.0.0.1:9", "");
    p.api_key = None;
    assert!(matches!(Provider::from_profile(&p), Err(Error::Config(_))));
}
