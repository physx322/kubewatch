//! Point d'entrée commun aux fournisseurs.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::anthropic::AnthropicClient;
use crate::config::{ProviderKind, ProviderProfile};
use crate::error::{Error, Result};
use crate::message::{truncate, ChatRequest, StreamEvent, Turn};
use crate::openai::OpenAiClient;

/// Modèle proposé par un fournisseur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// Identifiant à employer dans les requêtes.
    pub id: String,
    /// Nom lisible, quand le fournisseur en donne un.
    pub display_name: Option<String>,
}

/// Réceptacle des évènements de flux.
pub type Sink<'a> = dyn Fn(StreamEvent) + Send + Sync + 'a;

/// Client d'un fournisseur, construit à partir d'un profil.
#[derive(Debug, Clone)]
pub enum Provider {
    /// Anthropic.
    Anthropic(AnthropicClient),
    /// OpenAI ou compatible.
    OpenAi(OpenAiClient),
}

impl Provider {
    /// Construit le client correspondant au profil.
    pub fn from_profile(profile: &ProviderProfile) -> Result<Self> {
        match profile.kind {
            ProviderKind::Anthropic => Ok(Provider::Anthropic(AnthropicClient::new(profile)?)),
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => {
                Ok(Provider::OpenAi(OpenAiClient::new(profile)?))
            }
        }
    }

    /// Modèle demandé.
    pub fn model(&self) -> &str {
        match self {
            Provider::Anthropic(c) => c.model(),
            Provider::OpenAi(c) => c.model(),
        }
    }

    /// Modèles disponibles.
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        match self {
            Provider::Anthropic(c) => c.list_models().await,
            Provider::OpenAi(c) => c.list_models().await,
        }
    }

    /// Un tour de conversation, en flux.
    pub async fn stream_turn(&self, req: &ChatRequest, sink: &Sink<'_>) -> Result<Turn> {
        match self {
            Provider::Anthropic(c) => c.stream_turn(req, sink).await,
            Provider::OpenAi(c) => c.stream_turn(req, sink).await,
        }
    }
}

/// Client HTTP partagé : délai de connexion court, délai total généreux pour
/// les longues réponses en flux.
pub(crate) fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(900))
        .user_agent(concat!("kubewatch-ai/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// Convertit une réponse HTTP d'erreur en [`Error::Api`], en extrayant le
/// message des formes `{"error":{"message":…}}` (Anthropic, OpenAI) et
/// `{"message":…}`.
pub(crate) async fn api_error(resp: reqwest::Response) -> Error {
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    Error::Api {
        status,
        message: error_message(&text),
    }
}

/// Message d'erreur lisible extrait d'un corps de réponse.
pub(crate) fn error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        if let Some(e) = v.get("error") {
            if let Some(m) = e.get("message").and_then(Value::as_str) {
                return m.to_string();
            }
            if let Some(m) = e.as_str() {
                return m.to_string();
            }
        }
        if let Some(m) = v.get("message").and_then(Value::as_str) {
            return m.to_string();
        }
    }
    let t = body.trim();
    if t.is_empty() {
        "réponse vide".to_string()
    } else {
        truncate(t, 400)
    }
}

/// Réponse HTTP correcte, ou erreur lisible.
pub(crate) async fn ensure_success(resp: reqwest::Response) -> Result<reqwest::Response> {
    if resp.status().is_success() {
        Ok(resp)
    } else {
        Err(api_error(resp).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_du_message_d_erreur() {
        assert_eq!(
            error_message(
                r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#
            ),
            "invalid x-api-key"
        );
        assert_eq!(
            error_message(
                r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#
            ),
            "Incorrect API key provided"
        );
        assert_eq!(
            error_message(r#"{"message":"model not found"}"#),
            "model not found"
        );
        assert_eq!(
            error_message("<html>502 Bad Gateway</html>"),
            "<html>502 Bad Gateway</html>"
        );
        assert_eq!(error_message("   "), "réponse vide");
    }
}
