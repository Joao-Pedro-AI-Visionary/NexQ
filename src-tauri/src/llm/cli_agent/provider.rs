//! `ClaudeCliProvider`: the `LLMProvider` face of the Claude CLI, so every
//! existing caller works with the user's subscription on day one.
//!
//! `stream_completion` runs on the shared Utility Fast session. That session
//! outlives any single request, so each call's system prompt travels inside
//! the user message as an `## Instructions` block instead of the session's
//! system prompt. The `model` argument is not used: the session model comes
//! from `ClaudeCliConfig` (Fast tier).
//!
//! The Utility session is recycled once its context grows past
//! [`UTILITY_MAX_CONTEXT_TOKENS`], after [`UTILITY_MAX_TURNS`] turns, or right
//! after a CLI error, so unrelated requests never pile up into one huge
//! conversation.
//!
//! The provider holds no state besides the shared model catalog. It reaches the session manager through the
//! `AppHandle` every `stream_completion` call already receives, so
//! `create_provider` needs no access to `AppState`.

use super::detect::{claude_status, find_claude_binary, ClaudeLogin, API_BILLED_NOTE};
use super::models::ModelCatalog;
use super::manager::{AgentSessionManager, OnDelta, SessionInit, SessionKey, SessionScope, Tier};
use super::protocol::{AgentErrorKind, TurnResult};
use super::session::AgentError;
use crate::llm::provider::{
    emit_end, emit_error, emit_token, is_cancelled, CancelToken, CompletionStats,
    GenerationParams, LLMError, LLMMessage, LLMProvider, ModelInfo, TokenSink,
};
use crate::state::AppState;
use std::sync::Arc;
use std::time::Instant;
use tauri::Manager;

/// System prompt of the Utility session. Requests are unrelated to each other
/// (translation, knowledge-base test, wizard), so earlier turns must not leak
/// into later answers.
pub const UTILITY_SYSTEM_PROMPT: &str = "You are the assistant inside NexQ, a meeting \
assistant app. Every user message is a separate, self-contained request: follow its \
## Instructions section when present and answer only that request. Do not refer to or \
rely on earlier messages.";

/// The Utility session is replaced once a turn's context (input + cache
/// reads) reaches this size, so cost and latency stay flat and the CLI never
/// hits its context limit.
pub const UTILITY_MAX_CONTEXT_TOKENS: u64 = 60_000;
/// ... or after this many turns, whichever comes first.
pub const UTILITY_MAX_TURNS: usize = 30;

/// Static model list (CLI aliases that always resolve to the latest model),
/// used until the CLI has reported its catalog.
const MODELS: [(&str, &str); 4] = [
    ("haiku", "Claude Haiku"),
    ("sonnet", "Claude Sonnet"),
    ("opus", "Claude Opus"),
    ("fable", "Claude Fable"),
];

pub struct ClaudeCliProvider {
    catalog: Arc<ModelCatalog>,
}

impl ClaudeCliProvider {
    /// Lists the app-wide catalog the CLI reported.
    pub fn new() -> Self {
        Self::with_catalog(ModelCatalog::global())
    }

    pub fn with_catalog(catalog: Arc<ModelCatalog>) -> Self {
        Self { catalog }
    }
}

impl Default for ClaudeCliProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// The session serving every caller without a meeting scope.
pub fn utility_key() -> SessionKey {
    SessionKey {
        scope: SessionScope::Utility,
        tier: Tier::Fast,
    }
}

/// One user message for the session: system messages become a leading
/// `## Instructions` block, the other messages follow in order.
pub fn build_utility_prompt(messages: &[LLMMessage]) -> String {
    let instructions: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "system" && !m.content.trim().is_empty())
        .map(|m| m.content.as_str())
        .collect();
    let body: Vec<&str> = messages
        .iter()
        .filter(|m| m.role != "system")
        .map(|m| m.content.as_str())
        .collect();
    let body = body.join("\n\n");
    if instructions.is_empty() {
        body
    } else {
        format!("## Instructions\n\n{}\n\n{}", instructions.join("\n\n"), body)
    }
}

/// Start (or reuse) the Utility Fast session and ask `prompt` on it.
pub async fn ask_utility(
    manager: &AgentSessionManager,
    prompt: &str,
    on_delta: OnDelta,
    cancel: Arc<CancelToken>,
) -> Result<TurnResult, AgentError> {
    let key = utility_key();
    let init = SessionInit {
        system_prompt: UTILITY_SYSTEM_PROMPT.to_string(),
        preload: Vec::new(),
    };
    // Not raced against `cancel`: dropping a spawn mid-way would leave the
    // session status stuck on "starting". Cancellation is checked right after.
    manager.ensure(&key, init).await?;
    if cancel.is_cancelled() {
        return Err(AgentError::Cancelled);
    }
    let result = manager.ask(&key, prompt, on_delta, cancel).await;

    // Recycle a grown session, or one a CLI error (e.g. context too long)
    // left in `error` while its process lives on. The entry goes now; the
    // process is closed in the background so this request isn't delayed.
    let recycle = match &result {
        Ok(turn) => needs_recycle(turn, manager.turns(&key)),
        Err(AgentError::Cli(..)) => true,
        Err(_) => false,
    };
    if recycle {
        if let Some(old) = manager.retire_if_idle(&key) {
            tauri::async_runtime::spawn(async move { old.close().await });
        }
    }
    result
}

/// True when the Utility session should be replaced after `turn`, its
/// `turns`-th finished turn.
fn needs_recycle(turn: &TurnResult, turns: usize) -> bool {
    turn.input_tokens + turn.cache_read_tokens >= UTILITY_MAX_CONTEXT_TOKENS
        || turns >= UTILITY_MAX_TURNS
}

/// Map a session error onto the provider error the callers already handle.
pub fn to_llm_error(err: AgentError) -> LLMError {
    if err.is_cancel() {
        return LLMError::Cancelled;
    }
    let message = err.to_string();
    match err.kind() {
        AgentErrorKind::NotInstalled => LLMError::NotConfigured(message),
        AgentErrorKind::NotLoggedIn => LLMError::AuthError(message),
        _ => LLMError::ProviderError(message),
    }
}

/// `latency_ms` is the whole call as the user felt it (cold start, queueing).
fn stats_from(turn: &TurnResult, latency_ms: u64) -> CompletionStats {
    let prompt_tokens = turn.input_tokens + turn.cache_read_tokens;
    CompletionStats {
        prompt_tokens,
        completion_tokens: turn.output_tokens,
        total_tokens: prompt_tokens + turn.output_tokens,
        latency_ms,
    }
}

#[async_trait::async_trait]
impl LLMProvider for ClaudeCliProvider {
    fn provider_name(&self) -> &str {
        "claude_cli"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LLMError> {
        if let Some(models) = self.catalog.get().filter(|m| !m.is_empty()) {
            return Ok(models
                .into_iter()
                .map(|m| ModelInfo {
                    id: m.value,
                    name: m.display_name,
                    provider: "claude_cli".to_string(),
                    context_window: None,
                })
                .collect());
        }
        Ok(MODELS
            .iter()
            .map(|(id, name)| ModelInfo {
                id: id.to_string(),
                name: name.to_string(),
                provider: "claude_cli".to_string(),
                context_window: None,
            })
            .collect())
    }

    /// `Ok(true)` when `claude.exe` is installed and logged in to a Claude
    /// subscription. A login that bills an API account still works for
    /// requests, but is reported as an error here, never as connected.
    async fn test_connection(&self) -> Result<bool, LLMError> {
        let bin = tokio::task::spawn_blocking(find_claude_binary)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| LLMError::NotConfigured("Claude CLI is not installed".to_string()))?;
        let (_version, auth) = claude_status(&bin)
            .await
            .map_err(LLMError::ConnectionFailed)?;
        match auth.login {
            ClaudeLogin::Subscription => Ok(true),
            ClaudeLogin::LoggedOut => Err(LLMError::AuthError(
                "Claude CLI is not logged in. Run `claude` in a terminal to log in.".to_string(),
            )),
            // Usable, but never reported as connected to the subscription.
            ClaudeLogin::ApiBilled => Err(LLMError::AuthError(API_BILLED_NOTE.to_string())),
        }
    }

    async fn stream_completion(
        &self,
        messages: Vec<LLMMessage>,
        _model: &str,
        params: GenerationParams,
        app_handle: tauri::AppHandle,
        sink: Option<TokenSink>,
    ) -> Result<CompletionStats, LLMError> {
        let start = Instant::now();
        let manager = match app_handle.try_state::<AppState>() {
            Some(state) => state.agent_sessions.clone(),
            None => {
                let msg = "Claude CLI sessions are not initialized";
                emit_error(&app_handle, &params, msg);
                return Err(LLMError::NotConfigured(msg.to_string()));
            }
        };
        let cancel = params
            .cancel
            .clone()
            .unwrap_or_else(|| Arc::new(CancelToken::new()));
        let prompt = build_utility_prompt(&messages);

        let on_delta: OnDelta = {
            let app = app_handle.clone();
            let params = params.clone();
            Box::new(move |token: &str| {
                if !token.is_empty() && !is_cancelled(&params) {
                    emit_token(&app, &params, token, sink.as_ref());
                }
            })
        };

        match ask_utility(&manager, &prompt, on_delta, cancel).await {
            Ok(turn) => {
                if is_cancelled(&params) {
                    return Err(LLMError::Cancelled);
                }
                let stats = stats_from(&turn, start.elapsed().as_millis() as u64);
                emit_end(&app_handle, &params, stats.total_tokens, stats.latency_ms);
                Ok(stats)
            }
            Err(e) => {
                let err = to_llm_error(e);
                // A stopped stream already got its end from `cancel_generation`.
                if matches!(err, LLMError::Cancelled) || is_cancelled(&params) {
                    return Err(LLMError::Cancelled);
                }
                emit_error(&app_handle, &params, &err.to_string());
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::cli_agent::command::SessionConfig;
    use crate::llm::cli_agent::models::ClaudeModelInfo;
    use crate::llm::cli_agent::manager::{
        ClaudeCliConfig, NoopStatusSink, SessionHandle, SessionSpawner,
    };
    use async_trait::async_trait;
    use std::sync::{Mutex, RwLock};

    fn msg(role: &str, content: &str) -> LLMMessage {
        LLMMessage {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    #[tokio::test]
    async fn claude_cli_list_models_static() {
        let models = ClaudeCliProvider::with_catalog(Arc::new(ModelCatalog::default()))
            .list_models()
            .await
            .unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["haiku", "sonnet", "opus", "fable"]);
        let names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Claude Haiku", "Claude Sonnet", "Claude Opus", "Claude Fable"]);
        assert!(models.iter().all(|m| m.provider == "claude_cli"));
    }

    #[tokio::test]
    async fn claude_cli_list_models_uses_the_cli_catalog() {
        let catalog = Arc::new(ModelCatalog::default());
        let provider = ClaudeCliProvider::with_catalog(catalog.clone());
        // Nothing probed yet: the static aliases.
        assert_eq!(provider.list_models().await.unwrap().len(), 4);
        catalog.set(vec![ClaudeModelInfo {
            value: "claude-opus-4-8".into(),
            resolved_model: "claude-opus-4-8".into(),
            display_name: "Opus 4.8".into(),
            description: String::new(),
            supports_effort: true,
            effort_levels: vec!["low".into()],
        }]);
        let models = provider.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!((models[0].id.as_str(), models[0].name.as_str()), ("claude-opus-4-8", "Opus 4.8"));
        assert_eq!(models[0].provider, "claude_cli");
    }

    #[test]
    fn claude_cli_prompt_prepends_instructions() {
        let prompt = build_utility_prompt(&[msg("system", "Be brief."), msg("user", "Hi")]);
        assert_eq!(prompt, "## Instructions\n\nBe brief.\n\nHi");
        // No system message: the user text goes as-is.
        assert_eq!(build_utility_prompt(&[msg("user", "Translate x")]), "Translate x");
    }

    #[test]
    fn claude_cli_errors_map_to_llm_errors() {
        assert!(matches!(to_llm_error(AgentError::Cancelled), LLMError::Cancelled));
        assert!(matches!(
            to_llm_error(AgentError::Spawn("Claude CLI not installed: claude.exe not found".into())),
            LLMError::NotConfigured(_)
        ));
        assert!(matches!(
            to_llm_error(AgentError::Cli(AgentErrorKind::NotLoggedIn, "Not logged in".into())),
            LLMError::AuthError(_)
        ));
        assert!(matches!(
            to_llm_error(AgentError::Cli(AgentErrorKind::RateLimited, "usage limit".into())),
            LLMError::ProviderError(_)
        ));
    }

    /// Records what the manager spawned and asked; streams two deltas.
    #[derive(Default)]
    struct Recorder {
        configs: Mutex<Vec<SessionConfig>>,
        prompts: Mutex<Vec<String>>,
    }

    struct FakeSession(Arc<Recorder>);

    #[async_trait]
    impl SessionHandle for FakeSession {
        async fn append_context(&self, _text: &str) -> Result<(), AgentError> {
            Ok(())
        }
        async fn ask(
            &self,
            prompt: &str,
            on_delta: OnDelta,
            cancel: Arc<CancelToken>,
        ) -> Result<TurnResult, AgentError> {
            self.0.prompts.lock().unwrap().push(prompt.to_string());
            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }
            if prompt == "cli-error" {
                return Err(AgentError::Cli(AgentErrorKind::Other, "Prompt is too long".into()));
            }
            if prompt == "big" {
                return Ok(TurnResult {
                    input_tokens: 50_000,
                    cache_read_tokens: 10_000,
                    ..Default::default()
                });
            }
            on_delta("Hel");
            on_delta("lo");
            Ok(TurnResult {
                text: "Hello".into(),
                input_tokens: 12,
                output_tokens: 2,
                duration_ms: 40,
                ..Default::default()
            })
        }
        fn is_alive(&self) -> bool {
            true
        }
        async fn close(&self) {}
        fn model(&self) -> String {
            "haiku".into()
        }
    }

    struct FakeSpawner(Arc<Recorder>);

    #[async_trait]
    impl SessionSpawner for FakeSpawner {
        async fn spawn(&self, cfg: SessionConfig) -> Result<Arc<dyn SessionHandle>, AgentError> {
            self.0.configs.lock().unwrap().push(cfg);
            Ok(Arc::new(FakeSession(self.0.clone())))
        }
    }

    fn manager(rec: &Arc<Recorder>) -> AgentSessionManager {
        AgentSessionManager::new(
            Arc::new(FakeSpawner(rec.clone())),
            Arc::new(NoopStatusSink),
            Arc::new(RwLock::new(ClaudeCliConfig::default())),
        )
    }

    #[tokio::test]
    async fn claude_cli_ask_utility_reuses_one_fast_session() {
        let rec = Arc::new(Recorder::default());
        let mgr = manager(&rec);
        let streamed = Arc::new(Mutex::new(String::new()));

        for _ in 0..2 {
            let out = streamed.clone();
            let on_delta: OnDelta = Box::new(move |t: &str| out.lock().unwrap().push_str(t));
            let turn = ask_utility(&mgr, "q", on_delta, Arc::new(CancelToken::new()))
                .await
                .unwrap();
            assert_eq!(stats_from(&turn, 5).total_tokens, 14);
        }

        let configs = rec.configs.lock().unwrap();
        assert_eq!(configs.len(), 1, "the Utility session is spawned once");
        assert_eq!(configs[0].model, "haiku");
        assert_eq!(configs[0].effort.as_deref(), Some("low"));
        assert_eq!(configs[0].system_prompt, UTILITY_SYSTEM_PROMPT);
        assert_eq!(rec.prompts.lock().unwrap().len(), 2);
        assert_eq!(streamed.lock().unwrap().as_str(), "HelloHello");
        assert_eq!(mgr.usage(&SessionScope::Utility).input_tokens, 24);
    }

    #[tokio::test]
    async fn claude_cli_ask_utility_cancelled_before_ask() {
        let rec = Arc::new(Recorder::default());
        let mgr = manager(&rec);
        let cancel = Arc::new(CancelToken::new());
        cancel.cancel();
        let err = ask_utility(&mgr, "q", Box::new(|_: &str| {}), cancel)
            .await
            .unwrap_err();
        assert_eq!(err, AgentError::Cancelled);
        assert!(rec.prompts.lock().unwrap().is_empty(), "nothing is sent once cancelled");
    }

    async fn ask(mgr: &AgentSessionManager, prompt: &str) -> Result<TurnResult, AgentError> {
        ask_utility(mgr, prompt, Box::new(|_: &str| {}), Arc::new(CancelToken::new())).await
    }

    #[tokio::test]
    async fn claude_cli_utility_recycles_after_context_threshold() {
        let rec = Arc::new(Recorder::default());
        let mgr = manager(&rec);
        ask(&mgr, "q").await.unwrap();
        ask(&mgr, "big").await.unwrap(); // 50k input + 10k cache reads
        assert!(mgr.snapshot().is_empty(), "grown session is retired at once");
        ask(&mgr, "q").await.unwrap();
        assert_eq!(rec.configs.lock().unwrap().len(), 2, "next call starts a fresh session");
    }

    #[tokio::test]
    async fn claude_cli_utility_recycles_after_cli_error() {
        let rec = Arc::new(Recorder::default());
        let mgr = manager(&rec);
        assert!(matches!(ask(&mgr, "cli-error").await, Err(AgentError::Cli(..))));
        ask(&mgr, "q").await.unwrap();
        assert_eq!(rec.configs.lock().unwrap().len(), 2, "errored session is replaced");
    }

    #[tokio::test]
    async fn claude_cli_utility_recycles_after_max_turns() {
        let rec = Arc::new(Recorder::default());
        let mgr = manager(&rec);
        for _ in 0..UTILITY_MAX_TURNS {
            ask(&mgr, "q").await.unwrap();
        }
        assert_eq!(rec.configs.lock().unwrap().len(), 1);
        ask(&mgr, "q").await.unwrap();
        assert_eq!(rec.configs.lock().unwrap().len(), 2);
    }

    #[test]
    fn claude_cli_stats_count_cache_reads_as_prompt() {
        let turn = TurnResult {
            input_tokens: 10,
            cache_read_tokens: 90,
            output_tokens: 5,
            duration_ms: 1,
            ..Default::default()
        };
        let stats = stats_from(&turn, 1500);
        assert_eq!(stats.prompt_tokens, 100);
        assert_eq!(stats.total_tokens, 105);
        assert_eq!(stats.latency_ms, 1500);
    }
}
