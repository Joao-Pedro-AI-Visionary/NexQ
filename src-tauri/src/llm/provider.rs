use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use thiserror::Error;

/// Per-request generation parameters.
/// `None` fields mean "use provider default" — they won't be sent in the HTTP body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationParams {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    /// Gemini context cache name (e.g. "cachedContents/abc123").
    /// When set, GeminiClient uses the cached content instead of re-sending documents.
    /// Ignored by all other providers.
    pub cache_name: Option<String>,
    /// When true, enable provider-native web search/grounding.
    /// Gemini: adds the `google_search` tool. OpenRouter: appends `:online` to the model id.
    /// Ignored by providers without a native web search mechanism.
    pub enable_web_search: bool,
    /// Identifies this stream. Echoed in every `llm_stream_*` payload so listeners
    /// only react to their own request.
    pub request_id: Option<String>,
    /// When true, tokens go only to the caller's `TokenSink` and no `llm_stream_*`
    /// event is emitted (used by internal callers such as LLM translation).
    pub silent: bool,
    /// Cooperative cancel flag. Providers check it before emitting each token and
    /// return `LLMError::Cancelled` once it is set. Never serialized.
    #[serde(skip)]
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Default for GenerationParams {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            cache_name: None,
            enable_web_search: false,
            request_id: None,
            silent: false,
            cancel: None,
        }
    }
}

/// True when the caller asked this stream to stop.
pub fn is_cancelled(params: &GenerationParams) -> bool {
    params
        .cancel
        .as_ref()
        .map(|flag| flag.load(Ordering::SeqCst))
        .unwrap_or(false)
}

/// `Err(LLMError::Cancelled)` when the stream was cancelled; providers call this
/// before emitting each token. A cancelled stream emits no `llm_stream_error` and
/// no `llm_stream_end` of its own: `cancel_generation` already emitted the end.
pub fn check_cancelled(params: &GenerationParams) -> Result<(), LLMError> {
    if is_cancelled(params) {
        Err(LLMError::Cancelled)
    } else {
        Ok(())
    }
}

/// Callback that receives each streamed token for an in-process caller.
pub type TokenSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Where a streamed token is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Only the caller's sink receives it (silent stream).
    Sink,
    /// The global `llm_stream_token` event carries it (a sink, if any, also receives it).
    Global,
}

/// Feeds `token` to the sink (if any) and decides whether it must also be
/// emitted globally. Kept free of `AppHandle` so it can be unit-tested.
fn route_token(params: &GenerationParams, token: &str, sink: Option<&TokenSink>) -> Route {
    if let Some(sink) = sink {
        sink(token);
    }
    if params.silent {
        Route::Sink
    } else {
        Route::Global
    }
}

/// Delivers one streamed token: to the sink, and as `llm_stream_token` unless the stream is silent.
pub fn emit_token(app: &AppHandle, params: &GenerationParams, token: &str, sink: Option<&TokenSink>) {
    if route_token(params, token, sink) == Route::Global {
        let _ = app.emit(
            "llm_stream_token",
            StreamTokenPayload {
                token: token.to_string(),
                request_id: params.request_id.clone(),
            },
        );
    }
}

/// Emits `llm_stream_end` for this request (nothing for silent streams).
pub fn emit_end(app: &AppHandle, params: &GenerationParams, total_tokens: u64, latency_ms: u64) {
    if params.silent {
        return;
    }
    let _ = app.emit(
        "llm_stream_end",
        StreamEndPayload {
            total_tokens,
            latency_ms,
            request_id: params.request_id.clone(),
            cancelled: false,
        },
    );
}

/// Emits `llm_stream_error` for this request (nothing for silent streams — the
/// caller gets the error from the returned `Result`).
pub fn emit_error(app: &AppHandle, params: &GenerationParams, message: &str) {
    if params.silent {
        return;
    }
    let _ = app.emit(
        "llm_stream_error",
        StreamErrorPayload {
            message: message.to_string(),
            request_id: params.request_id.clone(),
        },
    );
}

/// Emits `llm_stream_sources` for this request (nothing for silent streams).
pub fn emit_sources(app: &AppHandle, params: &GenerationParams, sources: Vec<StreamSource>) {
    if params.silent {
        return;
    }
    let _ = app.emit(
        "llm_stream_sources",
        StreamSourcesPayload {
            sources,
            request_id: params.request_id.clone(),
        },
    );
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionStats {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Error)]
pub enum LLMError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Provider not configured: {0}")]
    NotConfigured(String),

    #[error("Authentication failed: {0}")]
    AuthError(String),

    #[error("Stream cancelled")]
    Cancelled,

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Provider error: {0}")]
    ProviderError(String),
}

impl Serialize for LLMError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Event payloads emitted during streaming
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamStartPayload {
    pub mode: String,
    pub model: String,
    pub provider: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub include_transcript: bool,
    pub include_rag: bool,
    pub include_instructions: bool,
    pub include_question: bool,
    // New fields for AI log enrichment
    pub temperature: f64,
    pub rag_query: Option<String>,
    pub rag_chunks: Vec<RagChunkInfo>,
    pub rag_chunks_filtered: usize,
    pub rag_total_candidates: usize,
    pub transcript_window_seconds: u64,
    pub transcript_segments_count: usize,
    pub transcript_segments_total: usize,
    pub request_id: Option<String>,
}

/// Metadata about a single RAG chunk, sent to the frontend for AI log display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagChunkInfo {
    pub source: String,
    pub chunk_index: usize,
    pub text: String,
    pub normalized_score: f64,
    pub raw_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamTokenPayload {
    pub token: String,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEndPayload {
    pub total_tokens: u64,
    pub latency_ms: u64,
    pub request_id: Option<String>,
    /// True when the stream was stopped by the user rather than finishing.
    #[serde(default)]
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamErrorPayload {
    pub message: String,
    pub request_id: Option<String>,
}

/// A single web search result/source surfaced by provider-native grounding (e.g. Gemini).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamSource {
    pub title: String,
    pub url: String,
}

/// Emitted once, after streaming completes, when the provider returned grounding sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamSourcesPayload {
    pub sources: Vec<StreamSource>,
    pub request_id: Option<String>,
}

/// Trait that all LLM providers implement.
#[async_trait::async_trait]
pub trait LLMProvider: Send + Sync {
    /// Returns the name of this provider (e.g., "openai", "ollama")
    fn provider_name(&self) -> &str;

    /// Lists available models for this provider
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LLMError>;

    /// Tests whether the provider is reachable and properly configured
    async fn test_connection(&self) -> Result<bool, LLMError>;

    /// Streams a completion response. Tokens, end and errors go through
    /// `emit_token` / `emit_end` / `emit_error`, which tag events with
    /// `params.request_id` and stay silent when `params.silent` is set.
    /// Events: "llm_stream_token", "llm_stream_end", "llm_stream_error"
    /// ("llm_stream_start" is emitted by the caller).
    async fn stream_completion(
        &self,
        messages: Vec<LLMMessage>,
        model: &str,
        params: GenerationParams,
        app_handle: tauri::AppHandle,
        sink: Option<TokenSink>,
    ) -> Result<CompletionStats, LLMError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn collecting_sink() -> (TokenSink, Arc<Mutex<String>>) {
        let collected = Arc::new(Mutex::new(String::new()));
        let target = collected.clone();
        let sink: TokenSink = Arc::new(move |token: &str| {
            target.lock().unwrap().push_str(token);
        });
        (sink, collected)
    }

    #[test]
    fn silent_params_route_tokens_to_sink_only() {
        let (sink, collected) = collecting_sink();
        let silent = GenerationParams {
            silent: true,
            request_id: Some("req-1".to_string()),
            ..Default::default()
        };

        assert_eq!(route_token(&silent, "hi", Some(&sink)), Route::Sink);
        assert_eq!(collected.lock().unwrap().as_str(), "hi");

        let global = GenerationParams {
            silent: false,
            ..Default::default()
        };
        assert_eq!(route_token(&global, "!", Some(&sink)), Route::Global);
        // A sink attached to a non-silent stream still receives tokens.
        assert_eq!(collected.lock().unwrap().as_str(), "hi!");
    }

    #[test]
    fn is_cancelled_follows_the_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        let params = GenerationParams {
            cancel: Some(flag.clone()),
            ..Default::default()
        };
        assert!(!is_cancelled(&params));
        flag.store(true, Ordering::SeqCst);
        assert!(is_cancelled(&params));
        assert!(matches!(check_cancelled(&params), Err(LLMError::Cancelled)));
        assert!(!is_cancelled(&GenerationParams::default()));
        assert!(check_cancelled(&GenerationParams::default()).is_ok());
    }

    #[test]
    fn stream_end_payload_cancelled_defaults_to_false() {
        let payload: StreamEndPayload =
            serde_json::from_str(r#"{"total_tokens":1,"latency_ms":2,"request_id":null}"#).unwrap();
        assert!(!payload.cancelled);
    }

    /// Mirrors a provider's stream loop (chunk -> lines -> SSE events -> token) over a
    /// canned OpenAI-style body and stops as soon as the flag is set.
    #[test]
    fn stream_loop_stops_after_cancel_flag_set_mid_stream() {
        use crate::llm::stream_parser::{LineBuffer, SSEParser};

        let chunks = [
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}

",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}

",
            "data: [DONE]

",
        ];
        let flag = Arc::new(AtomicBool::new(false));
        let params = GenerationParams {
            cancel: Some(flag.clone()),
            ..Default::default()
        };

        let mut collected: Vec<String> = Vec::new();
        let mut line_buffer = LineBuffer::new();
        let result: Result<(), LLMError> = (|| {
            for chunk in chunks {
                check_cancelled(&params)?;
                for line in line_buffer.push(chunk) {
                    for event in SSEParser::parse_chunk(&line).into_iter().flatten() {
                        if let Some(token) = SSEParser::extract_openai_token(&event) {
                            check_cancelled(&params)?;
                            collected.push(token);
                            // The user presses Stop right after the first token.
                            flag.store(true, Ordering::SeqCst);
                        }
                    }
                }
            }
            Ok(())
        })();

        assert_eq!(collected, vec!["Hel".to_string()]);
        assert!(matches!(result, Err(LLMError::Cancelled)));
    }
}
