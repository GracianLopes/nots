//! Pluggable AI backends behind the [`AIProvider`] trait.
//!
//! [`provider_from_config`] builds the wire backend matched to a
//! [`ProviderConfig`]: an OpenAI-chat-completions client for Groq, OpenRouter
//! and local OpenAI-compatible endpoints (Ollama, LM Studio), and a client
//! for the Gemini REST `generateContent` API. All backends speak HTTP through
//! `reqwest` and never log request bodies or API keys.

use std::time::Duration;

use async_trait::async_trait;
use notsai_core::{
    AIProvider, AiChatRequest, AiChatResponse, AiRole, CoreError, ProviderConfig, ProviderKind,
};

/// Default endpoint for the OpenAI-compatible Groq API.
const GROQ_BASE_URL: &str = "https://api.groq.com/openai/v1";
/// Default endpoint for the OpenAI-compatible OpenRouter API.
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
/// Default endpoint for the Gemini REST API.
const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Build the backend matched to `config.kind`.
pub fn provider_from_config(config: &ProviderConfig) -> Result<Box<dyn AIProvider>, CoreError> {
    match config.kind {
        ProviderKind::Gemini => Ok(Box::new(GeminiProvider::from_config(config)?)),
        ProviderKind::Groq | ProviderKind::OpenRouter | ProviderKind::Local => {
            Ok(Box::new(OpenAiCompatibleProvider::from_config(config)?))
        }
    }
}

/// Map an [`AiRole`] onto the wire-level role string used by OpenAI-compatible
/// chat-completions and Gemini request bodies.
fn chat_role(role: AiRole) -> &'static str {
    match role {
        AiRole::System => "system",
        AiRole::User => "user",
        AiRole::Assistant => "assistant",
    }
}

fn build_client() -> Result<reqwest::Client, CoreError> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| CoreError::ai_request(format!("failed to build http client: {err}")))
}

/// An OpenAI chat-completions backend used for Groq, OpenRouter and local
/// OpenAI-compatible endpoints.
pub struct OpenAiCompatibleProvider {
    kind: ProviderKind,
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl OpenAiCompatibleProvider {
    fn from_config(config: &ProviderConfig) -> Result<Self, CoreError> {
        let base_url = match config.kind {
            ProviderKind::Groq => GROQ_BASE_URL.to_string(),
            ProviderKind::OpenRouter => OPENROUTER_BASE_URL.to_string(),
            ProviderKind::Local => config.base_url.clone().ok_or_else(|| {
                CoreError::invalid_input("local AI provider requires a base_url".to_string())
            })?,
            ProviderKind::Gemini => {
                return Err(CoreError::invalid_input(
                    "gemini does not speak OpenAI chat-completions".to_string(),
                ))
            }
        };
        Ok(Self {
            kind: config.kind,
            base_url,
            api_key: config.api_key.clone(),
            client: build_client()?,
        })
    }
}

#[async_trait]
impl AIProvider for OpenAiCompatibleProvider {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    async fn chat(&self, request: AiChatRequest) -> Result<AiChatResponse, CoreError> {
        let messages: Vec<ChatMessage> = request
            .messages
            .iter()
            .map(|message| ChatMessage {
                role: chat_role(message.role).to_string(),
                content: message.content.clone(),
            })
            .collect();

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let mut builder = self.client.post(&url).json(&ChatCompletionRequest {
            model: request.model,
            messages,
            max_tokens: request.max_tokens,
        });
        if let Some(api_key) = &self.api_key {
            builder = builder.bearer_auth(api_key);
        }

        let response = builder.send().await.map_err(|err| {
            CoreError::ai_request(format!("{:?} request failed: {err}", self.kind))
        })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(CoreError::ai_request(format!(
                "{:?} returned {status}: {body}",
                self.kind
            )));
        }

        let payload: ChatCompletionResponse = response.json().await.map_err(|err| {
            CoreError::ai_request(format!("{:?} response parse failed: {err}", self.kind))
        })?;

        let content = payload
            .choices
            .first()
            .map(|choice| choice.message.content.clone())
            .unwrap_or_default();

        Ok(AiChatResponse { content })
    }
}

/// Google Gemini backend speaking the REST `generateContent` API.
pub struct GeminiProvider {
    model: String,
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl GeminiProvider {
    fn from_config(config: &ProviderConfig) -> Result<Self, CoreError> {
        let api_key = config.api_key.clone().ok_or_else(|| {
            CoreError::invalid_input("gemini AI provider requires an api_key".to_string())
        })?;
        let base_url = config
            .base_url
            .clone()
            .unwrap_or_else(|| GEMINI_BASE_URL.to_string());
        Ok(Self {
            model: config.model.clone(),
            base_url,
            api_key,
            client: build_client()?,
        })
    }
}

#[async_trait]
impl AIProvider for GeminiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Gemini
    }

    async fn chat(&self, request: AiChatRequest) -> Result<AiChatResponse, CoreError> {
        let mut system_instruction: Vec<String> = Vec::new();
        let mut contents: Vec<GeminiContent> = Vec::new();

        for message in &request.messages {
            match message.role {
                AiRole::System => system_instruction.push(message.content.clone()),
                AiRole::User => contents.push(GeminiContent::new("user", &message.content)),
                AiRole::Assistant => contents.push(GeminiContent::new("model", &message.content)),
            }
        }

        let url = format!(
            "{}/models/{}:generateContent",
            self.base_url.trim_end_matches('/'),
            self.model
        );
        let response = self
            .client
            .post(&url)
            .query(&[("key", self.api_key.as_str())])
            .json(&GeminiRequest {
                system_instruction: if system_instruction.is_empty() {
                    None
                } else {
                    Some(GeminiContent::raw(system_instruction))
                },
                contents,
                generation_config: request.max_tokens.map(|max| GeminiGenerationConfig {
                    max_output_tokens: max,
                }),
            })
            .send()
            .await
            .map_err(|err| CoreError::ai_request(format!("gemini request failed: {err}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(CoreError::ai_request(format!(
                "gemini returned {status}: {body}"
            )));
        }

        let payload: GeminiResponse = response
            .json()
            .await
            .map_err(|err| CoreError::ai_request(format!("gemini response parse failed: {err}")))?;

        let content = payload
            .candidates
            .first()
            .and_then(|candidate| candidate.content.as_ref())
            .map(|content| {
                content
                    .parts
                    .iter()
                    .map(|part| part.text.as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        Ok(AiChatResponse { content })
    }
}

#[derive(Debug, serde::Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, serde::Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Debug, serde::Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionMessage,
}

#[derive(Debug, serde::Deserialize)]
struct ChatCompletionMessage {
    content: String,
}

#[derive(Debug, serde::Serialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

impl GeminiContent {
    fn new(role: &str, text: &str) -> Self {
        Self {
            role: Some(role.to_string()),
            parts: vec![GeminiPart {
                text: text.to_string(),
            }],
        }
    }

    fn raw(parts: Vec<String>) -> Self {
        Self {
            role: None,
            parts: parts.into_iter().map(|text| GeminiPart { text }).collect(),
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct GeminiPart {
    text: String,
}

#[derive(Debug, serde::Serialize)]
struct GeminiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, serde::Serialize)]
struct GeminiGenerationConfig {
    max_output_tokens: u32,
}

#[derive(Debug, serde::Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
}

#[derive(Debug, serde::Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiResponseContent>,
}

#[derive(Debug, serde::Deserialize)]
struct GeminiResponseContent {
    parts: Vec<GeminiResponsePart>,
}

#[derive(Debug, serde::Deserialize)]
struct GeminiResponsePart {
    #[serde(default)]
    text: String,
}
