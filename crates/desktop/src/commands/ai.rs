//! Assistant IA : réglages, profils, modèles et conversation en flux.

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use kubewatch_ai::{
    ChatMessage, ChatRequest, Error as AiError, ModelInfo, ProfileUpdate, ProfileView, Provider,
    ProviderProfile, RunOptions, SettingsView, StopReason, StreamEvent, ToolExecutor, Usage,
};

use crate::assistant::{self, AiContext, KubeTools};
use crate::state::AppState;

/// Bilan d'une question à l'assistant.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatOutcome {
    /// Messages à ajouter à l'historique (tours de l'assistant, résultats d'outils).
    pub messages: Vec<ChatMessage>,
    /// Raison de l'arrêt.
    pub stop_reason: StopReason,
    /// Consommation cumulée.
    pub usage: Usage,
    /// Modèle ayant répondu.
    pub model: Option<String>,
    /// Catégorie du refus, le cas échéant.
    pub refusal_category: Option<String>,
}

#[tauri::command]
pub fn ai_settings(state: State<'_, AppState>) -> SettingsView {
    state.ai.view()
}

#[tauri::command]
pub fn ai_upsert_profile(
    state: State<'_, AppState>,
    update: ProfileUpdate,
) -> Result<ProfileView, String> {
    state.ai.upsert_profile(update).map_err(describe)
}

#[tauri::command]
pub fn ai_remove_profile(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.ai.remove_profile(&id).map_err(describe)
}

#[tauri::command]
pub fn ai_set_active(state: State<'_, AppState>, id: Option<String>) -> Result<(), String> {
    state.ai.set_active(id.as_deref()).map_err(describe)
}

#[tauri::command]
pub fn ai_set_general(
    state: State<'_, AppState>,
    tools_enabled: bool,
    max_tool_rounds: u32,
    extra_instructions: String,
) -> Result<SettingsView, String> {
    state
        .ai
        .set_general(tools_enabled, max_tool_rounds, extra_instructions)
        .map_err(describe)?;
    Ok(state.ai.view())
}

/// Modèles proposés par le fournisseur décrit par `draft`.
///
/// Sert aussi de test de connexion. Si `draft.id` désigne un profil enregistré
/// et qu'aucune clé n'est fournie, la clé enregistrée est réutilisée.
#[tauri::command]
pub async fn ai_list_models(
    state: State<'_, AppState>,
    draft: ProfileUpdate,
) -> Result<Vec<ModelInfo>, String> {
    let stored_key = draft
        .id
        .as_deref()
        .and_then(|id| state.ai.profile(Some(id)).ok())
        .and_then(|p| p.api_key);
    let api_key = match draft.api_key.as_deref().map(str::trim) {
        Some(k) if !k.is_empty() => Some(k.to_string()),
        _ => stored_key,
    };
    let profile = ProviderProfile {
        id: draft.id.clone().unwrap_or_default(),
        name: if draft.name.trim().is_empty() {
            draft.kind.label().to_string()
        } else {
            draft.name.clone()
        },
        kind: draft.kind,
        base_url: draft.base_url.clone(),
        api_key,
        // Le modèle n'est pas nécessaire pour lister ; on met un jalon.
        model: if draft.model.trim().is_empty() {
            "-".to_string()
        } else {
            draft.model.clone()
        },
        max_output_tokens: None,
        show_thinking: false,
    };
    let provider = Provider::from_profile(&profile).map_err(describe)?;
    let mut models = provider.list_models().await.map_err(describe)?;
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

/// Pose une question à l'assistant ; la réponse arrive par `on_event`.
#[tauri::command]
pub async fn ai_chat(
    state: State<'_, AppState>,
    chat_id: u64,
    messages: Vec<ChatMessage>,
    context: AiContext,
    profile_id: Option<String>,
    on_event: Channel<StreamEvent>,
) -> Result<ChatOutcome, String> {
    let profile = state.ai.profile(profile_id.as_deref()).map_err(describe)?;
    let settings = state.ai.settings();
    let provider = Provider::from_profile(&profile).map_err(describe)?;

    let cluster = context
        .cluster
        .clone()
        .filter(|c| !c.trim().is_empty())
        .or_else(|| state.clusters.current_name());
    let tools_enabled = settings.tools_enabled;
    let system = assistant::system_prompt(
        &context,
        cluster.as_deref(),
        &settings.extra_instructions,
        tools_enabled,
    );
    let request = ChatRequest {
        system,
        messages,
        tools: if tools_enabled {
            assistant::tool_specs()
        } else {
            Vec::new()
        },
        max_output_tokens: profile.max_output_tokens(),
        show_thinking: profile.show_thinking,
    };
    let opts = RunOptions {
        max_rounds: settings.max_tool_rounds,
    };
    let tools = KubeTools::new(state.clusters.clone(), cluster, context.namespace.clone());

    let task = tokio::spawn(async move {
        let sink = move |ev: StreamEvent| {
            let _ = on_event.send(ev);
        };
        let executor: Option<&dyn ToolExecutor> = if tools_enabled { Some(&tools) } else { None };
        kubewatch_ai::run(&provider, &request, executor, opts, &sink).await
    });
    state.chats.lock().insert(chat_id, task.abort_handle());
    let result = task.await;
    state.chats.lock().remove(&chat_id);

    match result {
        Ok(Ok(out)) => Ok(ChatOutcome {
            messages: out.messages,
            stop_reason: out.stop_reason,
            usage: out.usage,
            model: out.model,
            refusal_category: out.refusal_category,
        }),
        Ok(Err(e)) => Err(describe(e)),
        Err(join) if join.is_cancelled() => Err("réponse annulée".to_string()),
        Err(join) => Err(format!("erreur interne de l'assistant : {join}")),
    }
}

/// Interrompt une question en cours ; vrai si elle existait.
#[tauri::command]
pub fn ai_cancel(state: State<'_, AppState>, chat_id: u64) -> bool {
    match state.chats.lock().remove(&chat_id) {
        Some(h) => {
            h.abort();
            true
        }
        None => false,
    }
}

/// Message d'erreur lisible pour l'interface.
fn describe(e: AiError) -> String {
    match &e {
        AiError::Api { status: 401 | 403, message } => {
            format!("clé d'API refusée par le fournisseur ({message}). Vérifiez-la dans les réglages de l'assistant.")
        }
        AiError::Api { status: 404, message } => {
            format!("adresse ou modèle introuvable ({message}). Vérifiez l'adresse de base et le nom du modèle.")
        }
        AiError::Api { status: 429, message } => {
            format!("quota ou débit dépassé chez le fournisseur ({message}). Réessayez dans un instant.")
        }
        AiError::Http(inner) if inner.is_connect() => {
            "fournisseur injoignable : vérifiez l'adresse de base, et que le serveur local (LM Studio, Ollama…) est bien démarré.".to_string()
        }
        AiError::Http(inner) if inner.is_timeout() => {
            "le fournisseur n'a pas répondu à temps.".to_string()
        }
        _ => e.to_string(),
    }
}
