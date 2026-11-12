//! OpenAI-compatible chat completions client (D1, D17).
//!
//! The engine talks to exactly one provider: any endpoint that implements the
//! OpenAI chat-completions API. The client supports:
//!
//! - non-streaming completions,
//! - server-sent-event streaming,
//! - structured output via `response_format: json_schema`, with a fallback to
//!   JSON-only mode plus a repair pass when the endpoint does not support it,
//! - bounded exponential backoff on `429`, `5xx`, and timeouts.
//!
//! API keys are never logged. URLs in error messages are redacted.

use std::collections::VecDeque;
use std::fmt;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::{stream, Stream, StreamExt};
use regex::Regex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Config, ConfigError};
use crate::redact::redact_url;

/// Default request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Default number of retries after the first attempt.
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// Default delay before the first retry.
pub const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(200);

/// Default cap on the exponential backoff delay.
pub const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(5);

const JSON_ONLY_INSTRUCTION: &str =
    "Return only a single JSON object. Do not wrap it in markdown fences and do not add commentary.";

const FALLBACK_REPAIR_PROMPT: &str =
    "You repair malformed JSON. Return only the corrected JSON object.";

/// A chat message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System instructions.
    System,
    /// User input.
    User,
    /// Model output.
    Assistant,
}

/// One message in a chat completion request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Who produced the message.
    pub role: Role,
    /// Message text.
    pub content: String,
}

impl ChatMessage {
    /// Build a system message.
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    /// Build a user message.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    /// Build an assistant message.
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// The `response_format` field of a chat completion request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Ask the model for any valid JSON object.
    JsonObject,
    /// Ask the model for JSON matching a schema.
    JsonSchema {
        /// The schema wrapper.
        json_schema: JsonSchema,
    },
}

/// A JSON schema supplied to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonSchema {
    /// Name of the schema.
    pub name: String,
    /// The JSON schema itself.
    pub schema: Value,
    /// Whether the model must follow the schema strictly.
    #[serde(default)]
    pub strict: bool,
}

/// A chat completion request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatRequest {
    /// Conversation messages.
    pub messages: Vec<ChatMessage>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Maximum number of tokens to generate.
    pub max_tokens: Option<u32>,
    /// Structured-output request.
    pub response_format: Option<ResponseFormat>,
}

impl ChatRequest {
    /// Build a request from messages.
    #[must_use]
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            ..Self::default()
        }
    }
}

/// Token usage reported by the provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the prompt.
    #[serde(default)]
    pub prompt_tokens: u32,
    /// Tokens in the completion.
    #[serde(default)]
    pub completion_tokens: u32,
    /// Total tokens.
    #[serde(default)]
    pub total_tokens: u32,
}

/// A non-streaming chat completion response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatResponse {
    /// The assistant message content.
    pub content: String,
    /// Why generation stopped, if reported.
    pub finish_reason: Option<String>,
    /// Token usage, if reported.
    pub usage: Option<Usage>,
}

/// One streamed chunk of a chat completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamChunk {
    /// Text appended by this chunk.
    pub delta: String,
    /// Why generation stopped, if this is the final chunk.
    pub finish_reason: Option<String>,
}

/// Bounded exponential backoff settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Number of retries after the first attempt.
    pub max_retries: u32,
    /// Delay before the first retry.
    pub base_delay: Duration,
    /// Maximum delay between attempts.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            base_delay: DEFAULT_BASE_DELAY,
            max_delay: DEFAULT_MAX_DELAY,
        }
    }
}

impl RetryPolicy {
    /// The delay before retry number `attempt` (zero-based).
    #[must_use]
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let factor = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
        self.base_delay
            .checked_mul(factor)
            .unwrap_or(self.max_delay)
            .min(self.max_delay)
    }
}

/// An OpenAI-compatible chat completions client.
#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    retry: RetryPolicy,
    timeout: Duration,
}

impl fmt::Debug for LlmClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmClient")
            .field("base_url", &redact_url(&self.base_url))
            .field("model", &self.model)
            .field("has_api_key", &self.api_key.is_some())
            .field("retry", &self.retry)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl LlmClient {
    /// Build a client for `base_url` and `model`.
    ///
    /// `model` is required; an empty model is a clear error. A missing API key
    /// is allowed here because local endpoints do not need one. Use
    /// [`LlmClient::from_config`] to apply the configured key-resolution rules.
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Result<Self, LlmError> {
        let model = model.into();
        if model.trim().is_empty() {
            return Err(LlmError::MissingModel);
        }
        let http = reqwest::Client::builder()
            .build()
            .map_err(|error| LlmError::Http(error.without_url()))?;
        Ok(Self {
            http,
            base_url: base_url.into(),
            model,
            api_key,
            retry: RetryPolicy::default(),
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Build a client from resolved engine configuration.
    ///
    /// The model is required and the API key follows the documented resolution
    /// order. Local endpoints are allowed to have no key.
    pub fn from_config(config: &Config) -> Result<Self, LlmError> {
        let model = config.model().trim();
        if model.is_empty() {
            return Err(LlmError::MissingModel);
        }
        let api_key = config.resolve_api_key().map_err(|error| match error {
            ConfigError::MissingApiKey { base_url } => LlmError::MissingApiKey {
                base_url: redact_url(&base_url),
            },
            other => LlmError::Config(other.to_string()),
        })?;
        Self::new(config.base_url(), model, api_key)
    }

    /// Replace the retry policy.
    #[must_use]
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Replace the per-request timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The configured model identifier.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The configured base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The chat-completions endpoint.
    #[must_use]
    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    /// Run a non-streaming chat completion.
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let body = ChatRequestBody {
            model: &self.model,
            messages: &request.messages,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            response_format: request.response_format.as_ref(),
            stream: false,
        };
        let response = self.send(&body, false).await?;
        let payload: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|error| LlmError::Http(error.without_url()))?;
        let choice =
            payload.choices.into_iter().next().ok_or_else(|| {
                LlmError::InvalidResponse("response contained no choices".to_owned())
            })?;
        let content = choice
            .message
            .and_then(|message| message.content)
            .ok_or_else(|| LlmError::InvalidResponse("response contained no content".to_owned()))?;
        Ok(ChatResponse {
            content,
            finish_reason: choice.finish_reason,
            usage: payload.usage,
        })
    }

    /// Start a streaming chat completion.
    ///
    /// The returned stream yields text deltas in order. Retries apply only to
    /// establishing the stream; an error after the first chunk is returned to
    /// the caller.
    pub async fn chat_stream(&self, request: &ChatRequest) -> Result<ChatStream, LlmError> {
        let body = ChatRequestBody {
            model: &self.model,
            messages: &request.messages,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            response_format: request.response_format.as_ref(),
            stream: true,
        };
        let response = self.send(&body, true).await?;
        Ok(ChatStream::new(response))
    }

    /// Run a chat completion and deserialize the result as JSON.
    ///
    /// The client first asks for `response_format: json_schema`. If the
    /// endpoint rejects structured output, it falls back to JSON-only mode. If
    /// the returned text is not valid JSON, one repair pass is attempted using
    /// the embedded repair prompt.
    pub async fn complete_json<T: DeserializeOwned>(
        &self,
        messages: Vec<ChatMessage>,
        schema_name: &str,
        schema: Value,
    ) -> Result<T, LlmError> {
        let formats = [
            Some(ResponseFormat::JsonSchema {
                json_schema: JsonSchema {
                    name: schema_name.to_owned(),
                    schema: schema.clone(),
                    strict: true,
                },
            }),
            Some(ResponseFormat::JsonObject),
            None,
        ];

        let mut last_error = None;
        let mut response = None;
        for (index, format) in formats.into_iter().enumerate() {
            let mut request = ChatRequest {
                messages: messages.clone(),
                response_format: format,
                ..ChatRequest::default()
            };
            if index > 0 {
                request
                    .messages
                    .insert(0, ChatMessage::system(JSON_ONLY_INSTRUCTION));
            }
            match self.chat(&request).await {
                Ok(value) => {
                    response = Some(value);
                    break;
                }
                Err(error) if is_unsupported_format(&error) => {
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }

        let response = match response {
            Some(response) => response,
            None => {
                return Err(last_error.unwrap_or_else(|| {
                    LlmError::InvalidResponse("no response format was accepted".to_owned())
                }))
            }
        };

        match serde_json::from_str::<T>(&response.content) {
            Ok(value) => Ok(value),
            Err(first_error) => {
                let repaired = self.repair_json(&response.content, &schema).await?;
                serde_json::from_str::<T>(&repaired).map_err(|second_error| LlmError::InvalidJson {
                    message: format!(
                        "invalid JSON ({first_error}); repair failed ({second_error})"
                    ),
                    body: redact_urls(&response.content),
                })
            }
        }
    }

    async fn repair_json(&self, malformed: &str, schema: &Value) -> Result<String, LlmError> {
        let repair_prompt = crate::prompts::embedded(crate::prompts::REPAIR)
            .map_or(FALLBACK_REPAIR_PROMPT, |prompt| prompt.contents);
        let messages = vec![
            ChatMessage::system(repair_prompt),
            ChatMessage::user(format!("Schema:\n{schema}\n\nMalformed JSON:\n{malformed}")),
        ];
        let request = ChatRequest::new(messages);
        let response = self.chat(&request).await?;
        Ok(response.content)
    }

    async fn send(
        &self,
        body: &ChatRequestBody<'_>,
        stream: bool,
    ) -> Result<reqwest::Response, LlmError> {
        let url = self.endpoint();
        let mut attempt = 0u32;
        loop {
            let mut builder = self
                .http
                .post(&url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .timeout(self.timeout)
                .json(body);
            if stream {
                builder = builder.header(reqwest::header::ACCEPT, "text/event-stream");
            }
            if let Some(key) = &self.api_key {
                builder = builder.bearer_auth(key);
            }

            match builder.send().await {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return Ok(response);
                    }
                    let retry_after = retry_after(&response);
                    let body = response.text().await.unwrap_or_default();
                    let retryable = status.as_u16() == 429 || status.is_server_error();
                    if retryable && attempt < self.retry.max_retries {
                        let delay = retry_after.unwrap_or_else(|| self.retry.delay_for(attempt));
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    let body = redact_urls(&body);
                    if is_unsupported_status(status.as_u16(), &body) {
                        return Err(LlmError::UnsupportedResponseFormat {
                            status: status.as_u16(),
                            body,
                        });
                    }
                    return Err(LlmError::Status {
                        status: status.as_u16(),
                        body,
                    });
                }
                Err(error) => {
                    let retryable = error.is_timeout() || error.is_connect();
                    if retryable && attempt < self.retry.max_retries {
                        tokio::time::sleep(self.retry.delay_for(attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(if error.is_timeout() {
                        LlmError::Timeout
                    } else {
                        LlmError::Http(error.without_url())
                    });
                }
            }
        }
    }
}

#[derive(Serialize)]
struct ChatRequestBody<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a ResponseFormat>,
    stream: bool,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: Option<ChatChoiceMessage>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct StreamResponse {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: Option<StreamDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

type ByteStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>;
type ChunkStream = Pin<Box<dyn Stream<Item = Result<StreamChunk, LlmError>> + Send>>;

/// A stream of [`StreamChunk`]s from a chat completion.
pub struct ChatStream {
    inner: ChunkStream,
}

impl fmt::Debug for ChatStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatStream").finish_non_exhaustive()
    }
}

impl ChatStream {
    fn new(response: reqwest::Response) -> Self {
        struct State {
            bytes: ByteStream,
            buffer: Vec<u8>,
            pending: VecDeque<Result<StreamChunk, LlmError>>,
            done: bool,
        }

        let state = State {
            bytes: Box::pin(response.bytes_stream()),
            buffer: Vec::new(),
            pending: VecDeque::new(),
            done: false,
        };

        let inner = stream::unfold(state, |mut state| async move {
            loop {
                if let Some(item) = state.pending.pop_front() {
                    return Some((item, state));
                }
                if state.done {
                    return None;
                }

                while let Some(position) = state.buffer.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = state.buffer.drain(..=position).collect();
                    let line = String::from_utf8_lossy(&line[..line.len().saturating_sub(1)]);
                    if let Some(item) = process_sse_line(&line, &mut state.done) {
                        state.pending.push_back(item);
                    }
                    if state.done {
                        break;
                    }
                }
                if let Some(item) = state.pending.pop_front() {
                    return Some((item, state));
                }
                if state.done {
                    return None;
                }

                match state.bytes.next().await {
                    Some(Ok(bytes)) => state.buffer.extend_from_slice(&bytes),
                    Some(Err(error)) => {
                        state.done = true;
                        return Some((Err(LlmError::Http(error.without_url())), state));
                    }
                    None => {
                        if !state.buffer.is_empty() {
                            let line = std::mem::take(&mut state.buffer);
                            let line = String::from_utf8_lossy(&line);
                            if let Some(item) = process_sse_line(&line, &mut state.done) {
                                state.pending.push_back(item);
                            }
                        }
                        state.done = true;
                        if let Some(item) = state.pending.pop_front() {
                            return Some((item, state));
                        }
                        return None;
                    }
                }
            }
        });

        Self {
            inner: Box::pin(inner),
        }
    }
}

impl Stream for ChatStream {
    type Item = Result<StreamChunk, LlmError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

fn process_sse_line(line: &str, done: &mut bool) -> Option<Result<StreamChunk, LlmError>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" {
        *done = true;
        return None;
    }
    match serde_json::from_str::<StreamResponse>(data) {
        Ok(response) => {
            let choice = response.choices.into_iter().next()?;
            let delta = choice
                .delta
                .and_then(|delta| delta.content)
                .unwrap_or_default();
            if delta.is_empty() && choice.finish_reason.is_none() {
                return None;
            }
            Some(Ok(StreamChunk {
                delta,
                finish_reason: choice.finish_reason,
            }))
        }
        Err(error) => Some(Err(LlmError::Stream(format!(
            "invalid stream chunk: {error}"
        )))),
    }
}

fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    let value = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?;
    let seconds = value.trim().parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds))
}

fn is_unsupported_status(status: u16, body: &str) -> bool {
    if !matches!(status, 400 | 404 | 422) {
        return false;
    }
    let body = body.to_ascii_lowercase();
    body.contains("response_format") || body.contains("json_schema") || body.contains("unsupported")
}

fn is_unsupported_format(error: &LlmError) -> bool {
    match error {
        LlmError::UnsupportedResponseFormat { .. } => true,
        LlmError::Status { status, .. } => matches!(status, 400 | 404 | 422),
        _ => false,
    }
}

fn redact_urls(text: &str) -> String {
    static URL_RE: OnceLock<Regex> = OnceLock::new();
    let regex = URL_RE.get_or_init(|| {
        Regex::new(r#"https?://[^\s"'<>]+"#).expect("URL redaction regex compiles")
    });
    regex.replace_all(text, "«url»").into_owned()
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Errors produced by the LLM client.
#[derive(Debug)]
pub enum LlmError {
    /// `provider.model` was not configured.
    MissingModel,
    /// A remote endpoint was configured without an API key.
    MissingApiKey {
        /// The endpoint that needs a key.
        base_url: String,
    },
    /// Configuration could not be resolved.
    Config(String),
    /// The HTTP request failed.
    Http(reqwest::Error),
    /// The request timed out.
    Timeout,
    /// The endpoint returned a non-success status.
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body, with URLs redacted.
        body: String,
    },
    /// The endpoint rejected structured output.
    UnsupportedResponseFormat {
        /// HTTP status code.
        status: u16,
        /// Response body, with URLs redacted.
        body: String,
    },
    /// The response did not have the expected shape.
    InvalidResponse(String),
    /// The model did not return valid JSON, even after repair.
    InvalidJson {
        /// Description of the parse failure.
        message: String,
        /// The malformed body, with URLs redacted.
        body: String,
    },
    /// A streamed chunk could not be parsed.
    Stream(String),
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingModel => write!(
                f,
                "provider.model is required; set it in ~/.config/laudit/config.toml or .laudit/config.toml"
            ),
            Self::MissingApiKey { base_url } => write!(
                f,
                "no API key found for {base_url}; set LAUDIT_API_KEY or provider.api_key_env, or use a local endpoint"
            ),
            Self::Config(message) => write!(f, "invalid LLM configuration: {message}"),
            Self::Http(error) => write!(f, "HTTP request failed: {}", redact_urls(&error.to_string())),
            Self::Timeout => write!(f, "LLM request timed out"),
            Self::Status { status, body } => write!(
                f,
                "LLM request failed with status {status}: {}",
                redact_urls(&truncate(body, 2000))
            ),
            Self::UnsupportedResponseFormat { status, body } => write!(
                f,
                "LLM endpoint rejected structured output with status {status}: {}",
                redact_urls(&truncate(body, 2000))
            ),
            Self::InvalidResponse(message) => write!(f, "invalid LLM response: {message}"),
            Self::InvalidJson { message, body } => write!(
                f,
                "invalid JSON from LLM: {message}: {}",
                redact_urls(&truncate(body, 2000))
            ),
            Self::Stream(message) => write!(f, "invalid LLM stream: {}", redact_urls(message)),
        }
    }
}

impl std::error::Error for LlmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(error) => Some(error),
            Self::MissingModel
            | Self::MissingApiKey { .. }
            | Self::Config(_)
            | Self::Timeout
            | Self::Status { .. }
            | Self::UnsupportedResponseFormat { .. }
            | Self::InvalidResponse(_)
            | Self::InvalidJson { .. }
            | Self::Stream(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_is_bounded_exponential() {
        let policy = RetryPolicy {
            max_retries: 5,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
        };
        assert_eq!(policy.delay_for(0), Duration::from_millis(100));
        assert_eq!(policy.delay_for(1), Duration::from_millis(200));
        assert_eq!(policy.delay_for(2), Duration::from_millis(400));
        assert_eq!(policy.delay_for(3), Duration::from_millis(500));
        assert_eq!(policy.delay_for(30), Duration::from_millis(500));
    }

    #[test]
    fn missing_model_is_a_clear_error() {
        let error = LlmClient::new("http://localhost:11434/v1", "  ", None)
            .expect_err("empty model should fail");
        assert!(matches!(error, LlmError::MissingModel));
        assert!(error.to_string().contains("model"));
    }

    #[test]
    fn endpoint_joins_base_url() {
        let client = LlmClient::new("http://localhost:11434/v1/", "m", None).expect("client");
        assert_eq!(
            client.endpoint(),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn debug_does_not_expose_the_api_key() {
        let client = LlmClient::new(
            "https://api.example.com/v1",
            "m",
            Some("secret-key".to_owned()),
        )
        .expect("client");
        let debug = format!("{client:?}");
        assert!(!debug.contains("secret-key"));
        assert!(debug.contains("has_api_key: true"));
    }

    #[test]
    fn error_messages_redact_urls() {
        let error = LlmError::Status {
            status: 500,
            body: "failed at https://api.example.com/v1/chat/completions".to_owned(),
        };
        let message = error.to_string();
        assert!(!message.contains("api.example.com"));
        assert!(message.contains("«url»"));
    }

    #[test]
    fn response_format_serializes_to_openai_shape() {
        let format = ResponseFormat::JsonSchema {
            json_schema: JsonSchema {
                name: "model".to_owned(),
                schema: serde_json::json!({"type": "object"}),
                strict: true,
            },
        };
        let value = serde_json::to_value(format).expect("serialize");
        assert_eq!(value["type"], "json_schema");
        assert_eq!(value["json_schema"]["name"], "model");
        assert_eq!(value["json_schema"]["strict"], true);
        assert_eq!(value["json_schema"]["schema"]["type"], "object");

        let value = serde_json::to_value(ResponseFormat::JsonObject).expect("serialize");
        assert_eq!(value["type"], "json_object");
    }

    #[test]
    fn sse_lines_are_parsed() {
        let mut done = false;
        let chunk = process_sse_line(
            r#"data: {"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#,
            &mut done,
        )
        .expect("chunk")
        .expect("ok");
        assert_eq!(chunk.delta, "hello");
        assert!(!done);

        assert!(process_sse_line("data: [DONE]", &mut done).is_none());
        assert!(done);
        assert!(process_sse_line(": keep-alive", &mut done).is_none());
    }
}
