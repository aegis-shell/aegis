//! Model provider abstraction: shared conversation types, the [`Provider`]
//! trait, and the Anthropic / OpenAI-compatible implementations.

mod anthropic;
mod openai;
mod sse;

pub use anthropic::AnthropicProvider;
pub use openai::OpenAiProvider;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::config::{ProviderKind, ResolvedProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// Base64-encoded image payload attached to a message or tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        text: String,
        image: Option<ImageData>,
        is_error: bool,
    },
    Image(ImageData),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text(text.into())],
        }
    }

    /// Concatenated text of every text block, for display.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// One tool the model may call, advertised in its native schema format.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub model: String,
    pub max_tokens: u32,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Incremental decode of a provider stream, for live display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    TextDelta(String),
    ToolUseStart { id: String, name: String },
    ToolUseInputDelta(String),
    ToolUseEnd,
    Done,
}

/// A streaming chat-completions endpoint. Implemented with native async
/// fns; callers select the implementation statically or via [`AnyProvider`].
/// The returned future borrows `on_event` and is intentionally not `Send`:
/// the agent loop awaits it sequentially on one task.
#[allow(async_fn_in_trait)]
pub trait Provider {
    async fn stream(
        &self,
        request: &Request,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<Response, ProviderError>;
}

/// Config-dispatched provider used by the agent loop.
pub enum AnyProvider {
    Anthropic(AnthropicProvider),
    OpenAi(OpenAiProvider),
}

impl AnyProvider {
    pub fn from_config(
        config: &ResolvedProvider,
        http: reqwest::Client,
    ) -> Result<Self, ProviderError> {
        let api_key = config
            .api_key()
            .ok_or_else(|| ProviderError::MissingApiKey(config.api_key_env.clone()))?;
        Ok(match config.kind {
            ProviderKind::Anthropic => Self::Anthropic(AnthropicProvider::new(
                http,
                config.base_url.clone(),
                api_key,
            )),
            ProviderKind::OpenAiCompatible => {
                Self::OpenAi(OpenAiProvider::new(http, config.base_url.clone(), api_key))
            }
        })
    }
}

impl Provider for AnyProvider {
    async fn stream(
        &self,
        request: &Request,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<Response, ProviderError> {
        match self {
            Self::Anthropic(provider) => provider.stream(request, on_event).await,
            Self::OpenAi(provider) => provider.stream(request, on_event).await,
        }
    }
}

/// Map a provider-native finish reason onto the shared vocabulary.
pub(crate) fn stop_reason(raw: Option<&str>) -> StopReason {
    match raw {
        Some("end_turn" | "stop_sequence" | "stop") => StopReason::EndTurn,
        Some("tool_use" | "tool_calls") => StopReason::ToolUse,
        Some("max_tokens" | "length") => StopReason::MaxTokens,
        other => StopReason::Other(other.unwrap_or("unknown").to_string()),
    }
}

/// Bound an error body so a provider failure stays readable on a terminal.
pub(crate) fn capped(body: &str) -> String {
    body.chars().take(2000).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("API key environment variable {0} is unset or empty")]
    MissingApiKey(String),
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("provider API returned status {status}: {body}")]
    Api { status: u16, body: String },
    #[error("malformed provider stream: {0}")]
    Sse(String),
    #[error("provider JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}
