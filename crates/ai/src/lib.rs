//! Assistant IA de KubeWatch : dialogue en flux avec un modèle de langage,
//! appels d'outils compris, quel que soit le fournisseur.
//!
//! Trois familles de fournisseurs sont prises en charge :
//!
//! * **Anthropic** (Claude) — API Messages, flux SSE, blocs `tool_use` ;
//! * **OpenAI** (ChatGPT) — API Chat Completions, flux SSE, `tool_calls` ;
//! * **compatible OpenAI** — même dialecte, adressé à un serveur local
//!   (LM Studio, Ollama, llama.cpp, Jan…) ou à un proxy d'entreprise.
//!
//! Le crate ne sait rien de Kubernetes : les outils sont décrits par un
//! [`ToolSpec`] et exécutés par un [`ToolExecutor`] fourni par l'application.
//! Les clés d'API sont conservées par [`AiStore`] dans le dossier d'état, en
//! `0600`, et ne sortent jamais vers l'interface.
#![forbid(unsafe_code)]

pub mod agent;
pub mod anthropic;
pub mod config;
pub mod error;
pub mod message;
pub mod openai;
pub mod provider;
pub mod sse;
pub mod store;

pub use agent::{run, RunOptions, RunOutcome, ToolExecutor};
pub use config::{
    AiSettings, ProfileUpdate, ProfileView, ProviderKind, ProviderProfile, SettingsView,
};
pub use error::{Error, Result};
pub use message::{
    ChatMessage, ChatRequest, Part, Role, StopReason, StreamEvent, ToolSpec, Turn, Usage,
};
pub use provider::{ModelInfo, Provider, Sink};
pub use store::AiStore;
