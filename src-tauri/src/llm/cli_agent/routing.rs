//! Request routing for the Claude CLI provider.
//!
//! - [`choose_route`]: a request goes to a warm meeting session, to the
//!   fallback provider (Claude unavailable), or down today's stateless path
//!   (any other provider, or no meeting scope).
//! - [`HealthTracker`]: whether Claude is usable; set unavailable after a
//!   failed restart, back to healthy by the background re-probe.
//! - [`MeetingSessions`]: what each meeting session was given (system prompt,
//!   preload, every delta sent), so a crashed session can be restarted with
//!   the same context.
//! - [`ask_meeting_session`]: send one delta on a meeting session, restarting
//!   it once on a crash, timeout, logout or rate limit.

use super::context::{build_delta_prompt, build_document_preload, pack_messages, DeltaCursor, Seg};
use super::detect::{claude_auth_status, find_claude_binary};
use super::manager::{
    tier_for, AgentSessionManager, ClaudeCliConfig, OnDelta, SessionInit, SessionKey,
    SessionScope, Tier,
};
use super::protocol::{AgentErrorKind, TurnResult};
use super::session::AgentError;
use crate::context::token_counter::count_tokens;
use crate::llm::provider::CancelToken;
use crate::llm::{ProviderConfig, ProviderType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Context documents up to this many tokens are preloaded into a live
/// session; beyond it the session runs RAG retrieval per request instead.
pub const DOC_PRELOAD_TOKEN_CAP: usize = 100_000;
/// Largest single preload message (characters).
pub const PRELOAD_MESSAGE_CHARS: usize = 100_000;
/// How often an unavailable Claude is re-probed.
pub const HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(60);
/// Tauri event carrying [`AgentFallbackPayload`].
pub const FALLBACK_EVENT: &str = "agent_fallback";

/// Payload of `agent_fallback`: Claude is unavailable and requests now go to
/// `provider` (the fallback provider id).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentFallbackPayload {
    pub reason: String,
    pub provider: String,
}

/// Whether Claude can take requests right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeHealth {
    Healthy,
    Unavailable(AgentErrorKind),
}

/// Where one `generate_assist` request goes.
#[derive(Debug, Clone)]
pub enum Route {
    /// The warm Claude session of a meeting.
    ClaudeSession(SessionKey),
    /// Claude is unavailable: today's full-prompt path on this provider.
    Fallback(ProviderConfig),
    /// Today's full-prompt path on the active provider.
    Stateless,
}

/// Pick the route of one request. Only the Claude CLI provider has sessions,
/// and only meeting scopes (live or past) use them; Utility requests go
/// through the provider's own `stream_completion`. When Claude is unavailable
/// the request goes to the fallback provider, or, with none configured, down
/// the stateless path, where the Claude provider reports the reason.
pub fn choose_route(
    active: &ProviderType,
    claude_health: ClaudeHealth,
    scope: &SessionScope,
    mode: &str,
    cfg: &ClaudeCliConfig,
) -> Route {
    if *active != ProviderType::ClaudeCli {
        return Route::Stateless;
    }
    match claude_health {
        ClaudeHealth::Unavailable(_) => match &cfg.fallback_provider {
            Some(provider) => Route::Fallback(fallback_config(provider)),
            None => Route::Stateless,
        },
        ClaudeHealth::Healthy => match scope {
            SessionScope::Utility => Route::Stateless,
            SessionScope::Live(_) | SessionScope::Past(_) => Route::ClaudeSession(SessionKey {
                scope: scope.clone(),
                tier: tier_for(mode, scope, cfg),
            }),
        },
    }
}

/// A provider config naming only the type; the caller adds the API key.
pub fn fallback_config(provider: &str) -> ProviderConfig {
    ProviderConfig {
        provider_type: provider.to_string(),
        api_key: None,
        base_url: None,
        auth_type: None,
        auth_value: None,
        auth_header: None,
    }
}

/// Claude's health plus whether the current outage was already announced
/// with `agent_fallback` (one toast per outage).
pub struct HealthTracker {
    state: Mutex<(ClaudeHealth, bool)>,
}

impl Default for HealthTracker {
    fn default() -> Self {
        Self {
            state: Mutex::new((ClaudeHealth::Healthy, false)),
        }
    }
}

impl HealthTracker {
    pub fn get(&self) -> ClaudeHealth {
        self.state.lock().unwrap().0
    }

    pub fn mark_unavailable(&self, kind: AgentErrorKind) {
        self.state.lock().unwrap().0 = ClaudeHealth::Unavailable(kind);
    }

    /// Back to healthy. True when Claude was unavailable.
    pub fn mark_healthy(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        let was_down = state.0 != ClaudeHealth::Healthy;
        *state = (ClaudeHealth::Healthy, false);
        was_down
    }

    /// True the first time it is called during an outage.
    pub fn take_fallback_notice(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        let first = !state.1;
        state.1 = true;
        first
    }
}

/// What one meeting session holds, so it can be restarted with it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionRecord {
    pub system_prompt: String,
    /// Context given at the session's first start (documents, transcript...).
    pub preload: Vec<String>,
    /// Every delta the session received since, in order.
    pub replay: Vec<String>,
    /// Final transcript segments the session holds (preloaded + sent).
    pub segments_sent: usize,
    /// Context documents were over the cap: requests run RAG retrieval.
    pub rag_mode: bool,
}

impl SessionRecord {
    /// System prompt and context for (re)starting the session: the preload
    /// followed by every delta sent so far (packed into few messages).
    fn restart_init(&self) -> SessionInit {
        let mut preload = self.preload.clone();
        preload.extend(pack_messages(&self.replay, PRELOAD_MESSAGE_CHARS));
        SessionInit {
            system_prompt: self.system_prompt.clone(),
            preload,
        }
    }
}

/// [`SessionRecord`]s of the open meeting sessions.
#[derive(Default)]
pub struct MeetingSessions {
    records: Mutex<HashMap<SessionKey, SessionRecord>>,
}

impl MeetingSessions {
    pub fn insert(&self, key: SessionKey, record: SessionRecord) {
        self.records.lock().unwrap().insert(key, record);
    }

    pub fn contains(&self, key: &SessionKey) -> bool {
        self.records.lock().unwrap().contains_key(key)
    }

    pub fn get(&self, key: &SessionKey) -> Option<SessionRecord> {
        self.records.lock().unwrap().get(key).cloned()
    }

    /// Forget every tier of `scope` (meeting ended / meeting view left).
    pub fn remove_scope(&self, scope: &SessionScope) {
        self.records.lock().unwrap().retain(|key, _| key.scope != *scope);
    }

    fn restart_init(&self, key: &SessionKey) -> Option<SessionInit> {
        self.records.lock().unwrap().get(key).map(SessionRecord::restart_init)
    }

    fn segments_sent(&self, key: &SessionKey) -> usize {
        self.records
            .lock()
            .unwrap()
            .get(key)
            .map(|r| r.segments_sent)
            .unwrap_or(0)
    }

    fn record_sent(&self, key: &SessionKey, prompt: &str, segments_sent: usize) {
        if let Some(record) = self.records.lock().unwrap().get_mut(key) {
            record.replay.push(prompt.to_string());
            record.segments_sent = segments_sent;
        }
    }
}

/// Errors worth one session restart (the context is replayed).
pub fn is_restartable(err: &AgentError) -> bool {
    matches!(
        err,
        AgentError::Crashed(_)
            | AgentError::Timeout
            | AgentError::Cli(AgentErrorKind::NotLoggedIn | AgentErrorKind::RateLimited, _)
    )
}

/// The outage an error means, or `None` for errors that are not about
/// Claude being unavailable (a stop, or an error result such as a too-long
/// prompt), which are reported as they are.
pub fn unavailable_kind(err: &AgentError) -> Option<AgentErrorKind> {
    match err {
        AgentError::Cancelled | AgentError::Cli(AgentErrorKind::Other, _) => None,
        AgentError::Timeout => Some(AgentErrorKind::Crashed),
        other => Some(other.kind()),
    }
}

/// Short readable reason for the fallback toast.
pub fn reason_label(kind: AgentErrorKind) -> &'static str {
    match kind {
        AgentErrorKind::NotInstalled => "not installed",
        AgentErrorKind::NotLoggedIn => "not logged in",
        AgentErrorKind::RateLimited => "usage limit reached",
        AgentErrorKind::Crashed => "session crashed",
        AgentErrorKind::Other => "unavailable",
    }
}

/// Point `cursor` at the `known` segments the session holds. The manager
/// resets a cursor whenever its session (re)starts; the record knows what the
/// (replayed) context contains.
fn sync_cursor(cursor: &mut DeltaCursor, finals: &[Seg], known: usize) {
    if cursor.sent() != known {
        *cursor = DeltaCursor::default();
        cursor.take_new(&finals[..known.min(finals.len())]);
    }
}

/// Send one request on the meeting session `key` (whose record the caller
/// inserted): ensure the session, take the final segments it has not seen,
/// build the prompt with `compose(new_segments)`, report it with
/// `on_sent(prompt, model, new_segment_count)` and ask.
///
/// A crash, timeout, logout or rate limit restarts the session once with its
/// preload plus every earlier delta and asks again. A delivered prompt (also
/// a stopped one) is recorded for replay; a failed one is rolled back so its
/// segments go with the next request.
pub async fn ask_meeting_session(
    manager: &AgentSessionManager,
    sessions: &MeetingSessions,
    key: &SessionKey,
    finals: &[Seg],
    compose: impl FnOnce(&[Seg]) -> String,
    on_sent: impl FnOnce(&str, &str, usize),
    make_on_delta: impl Fn() -> OnDelta,
    cancel: Arc<CancelToken>,
) -> Result<TurnResult, AgentError> {
    let mut compose = Some(compose);
    let mut on_sent = Some(on_sent);
    let mut prompt: Option<String> = None;
    let mut restarted = false;
    loop {
        let init = sessions.restart_init(key).ok_or_else(|| {
            AgentError::Crashed("no Claude session was started for this meeting".into())
        })?;
        let attempt = async {
            let handle = manager.ensure(key, init).await?;
            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }
            let mut cursor = manager.cursor(key).await;
            sync_cursor(&mut cursor, finals, sessions.segments_sent(key));
            let before = cursor.clone();
            let new = cursor.take_new(finals);
            if prompt.is_none() {
                let build = compose.take().expect("compose runs once");
                prompt = Some(build(new));
            }
            let text = prompt.clone().unwrap_or_default();
            if let Some(report) = on_sent.take() {
                report(&text, &handle.model(), new.len());
            }
            let result = manager.ask(key, &text, make_on_delta(), cancel.clone()).await;
            match &result {
                Ok(_) | Err(AgentError::Cancelled) => sessions.record_sent(key, &text, cursor.sent()),
                Err(_) => *cursor = before,
            }
            result
        };
        match attempt.await {
            Err(e) if !restarted && is_restartable(&e) && !cancel.is_cancelled() => {
                log::warn!("Claude session {key:?} failed ({e}); restarting it once");
                restarted = true;
                if let Some(old) = manager.retire_if_idle(key) {
                    tokio::spawn(async move { old.close().await });
                }
            }
            outcome => return outcome,
        }
    }
}

/// Session system prompt for a live meeting: the scenario prompt, the
/// speaker context, the global custom instructions and how the session's
/// messages are laid out.
pub fn live_system_prompt(scenario_prompt: &str, speaker_context: &str, custom_instructions: &str) -> String {
    let base = if scenario_prompt.trim().is_empty() {
        "You are NexQ, a real-time AI meeting assistant. Help the user follow the \
         meeting and respond well: accurate, concise, actionable."
    } else {
        scenario_prompt.trim()
    };
    let mut parts = vec![base.to_string()];
    if !speaker_context.trim().is_empty() {
        parts.push(speaker_context.trim().to_string());
    }
    if !custom_instructions.trim().is_empty() {
        parts.push(format!("Additional Instructions: {}", custom_instructions.trim()));
    }
    parts.push(LIVE_SESSION_RULES.to_string());
    parts.join("\n\n")
}

/// Session system prompt for a finished meeting open in the meeting view.
pub fn past_system_prompt(custom_instructions: &str) -> String {
    let mut parts = vec![PAST_SESSION_PROMPT.to_string()];
    if !custom_instructions.trim().is_empty() {
        parts.push(format!("Additional Instructions: {}", custom_instructions.trim()));
    }
    parts.join("\n\n")
}

const LIVE_SESSION_RULES: &str = "\
## How this session works
You are connected to a live meeting through NexQ. Context documents and the \
transcript arrive as messages. Each request carries only the transcript lines \
that are new since the previous request ([mm:ss] Speaker: text), then \
\"## Task instructions\" and, when present, the question. Treat every \
transcript line you have received as one continuous meeting. Follow the task \
instructions of the current message only; they replace those of earlier \
messages. Answer directly, without preamble, and never repeat the transcript \
back. Respond in the language used in the meeting.";

const PAST_SESSION_PROMPT: &str = "\
You are NexQ's assistant for a meeting that has already ended. Its summary \
(when one exists) and full transcript, with speakers and [mm:ss] timestamps, \
arrive as context messages. Each request carries \"## Task instructions\" and, \
when present, a question. Follow the task instructions of the current message \
only; they replace those of earlier messages. Base your answers on this \
meeting, answer directly without preamble, and respond in the language used \
in the meeting.";

/// Preload messages for the context documents, and whether the session must
/// use RAG retrieval instead (documents over [`DOC_PRELOAD_TOKEN_CAP`]).
pub fn document_preload(documents: &str) -> (Vec<String>, bool) {
    if documents.trim().is_empty() {
        return (Vec::new(), false);
    }
    if count_tokens(documents) > DOC_PRELOAD_TOKEN_CAP {
        return (Vec::new(), true);
    }
    (build_document_preload(documents, PRELOAD_MESSAGE_CHARS), false)
}

/// The user message of one request: the new transcript lines, optional
/// extra sections (document excerpts from RAG, segment-id transcript), then
/// the task instructions and the question(s).
pub fn compose_delta(
    new_segments: &[Seg],
    extra_sections: &[(&str, &str)],
    task_instructions: &str,
    question: Option<&str>,
    detected_question: Option<&str>,
) -> String {
    let mut parts = vec![build_delta_prompt(new_segments, "", None, None)];
    for (header, body) in extra_sections {
        if !body.trim().is_empty() {
            parts.push(format!("{header}\n{}", body.trim()));
        }
    }
    parts.push(build_delta_prompt(&[], task_instructions, question, detected_question));
    parts.retain(|p| !p.is_empty());
    parts.join("\n\n")
}

/// `live:<meeting>:fast` style label for logs and the AI Log.
pub fn session_key_label(key: &SessionKey) -> String {
    let tier = match key.tier {
        Tier::Fast => "fast",
        Tier::Deep => "deep",
    };
    match &key.scope {
        SessionScope::Live(id) => format!("live:{id}:{tier}"),
        SessionScope::Past(id) => format!("past:{id}:{tier}"),
        SessionScope::Utility => format!("utility:{tier}"),
    }
}

/// `You` for the mic party, the label otherwise.
pub fn speaker_label(speaker: &str) -> &str {
    match speaker {
        "User" => "You",
        other => other,
    }
}

/// Milliseconds since `start_ms` for a wall-clock timestamp; timestamps
/// already relative (smaller than the start) are kept.
pub fn relative_ms(timestamp_ms: i64, start_ms: i64) -> u64 {
    let ts = timestamp_ms.max(0);
    if start_ms > 0 && ts >= start_ms {
        (ts - start_ms) as u64
    } else {
        ts as u64
    }
}

/// Final segments from `generate_assist`'s `transcript_segments` JSON, with
/// timestamps relative to `start_ms`. Blank segments are skipped.
pub fn segments_from_json(json: &str, start_ms: i64) -> Vec<Seg> {
    #[derive(Deserialize)]
    struct Raw {
        text: String,
        speaker: String,
        timestamp_ms: i64,
    }
    let raw: Vec<Raw> = match serde_json::from_str(json) {
        Ok(raw) => raw,
        Err(e) => {
            log::warn!("Failed to parse transcript segments: {e}");
            return Vec::new();
        }
    };
    raw.into_iter()
        .filter(|s| !s.text.trim().is_empty())
        .map(|s| Seg {
            speaker: speaker_label(&s.speaker).to_string(),
            timestamp_ms: relative_ms(s.timestamp_ms, start_ms),
            text: s.text,
        })
        .collect()
}

/// No-cost check that Claude is installed and logged in.
pub async fn probe_claude() -> Result<(), AgentErrorKind> {
    let bin = tokio::task::spawn_blocking(find_claude_binary)
        .await
        .ok()
        .flatten()
        .ok_or(AgentErrorKind::NotInstalled)?;
    let auth = claude_auth_status(&bin)
        .await
        .map_err(|_| AgentErrorKind::Other)?;
    if auth.logged_in {
        Ok(())
    } else {
        Err(AgentErrorKind::NotLoggedIn)
    }
}

/// While Claude is the active provider and unavailable, re-probe it every
/// [`HEALTH_PROBE_INTERVAL`] (no model call) and mark it healthy again once
/// it is installed and logged in, so requests switch back to it.
pub async fn run_health_probe(app: tauri::AppHandle) {
    use tauri::Manager;
    let mut tick = tokio::time::interval(HEALTH_PROBE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let Some(state) = app.try_state::<crate::state::AppState>() else {
            continue;
        };
        if state.claude_health.get() == ClaudeHealth::Healthy || !claude_is_active(&state) {
            continue;
        }
        if probe_claude().await.is_ok() && state.claude_health.mark_healthy() {
            log::info!("Claude CLI is available again; requests use it again");
        }
    }
}

/// True when the active LLM provider is the Claude CLI.
pub fn claude_is_active(state: &crate::state::AppState) -> bool {
    state
        .llm
        .as_ref()
        .and_then(|llm| llm.lock().ok())
        .and_then(|router| router.active_provider_type().cloned())
        .map(|pt| pt == ProviderType::ClaudeCli)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::cli_agent::command::SessionConfig;
    use crate::llm::cli_agent::manager::{NoopStatusSink, SessionHandle, SessionSpawner};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::RwLock;

    fn live() -> SessionScope {
        SessionScope::Live("m1".into())
    }

    #[test]
    fn route_claude_healthy_live_assist() {
        let route = choose_route(
            &ProviderType::ClaudeCli,
            ClaudeHealth::Healthy,
            &live(),
            "Assist",
            &ClaudeCliConfig::default(),
        );
        match route {
            Route::ClaudeSession(key) => {
                assert_eq!(key, SessionKey { scope: live(), tier: Tier::Fast })
            }
            other => panic!("expected a Claude session, got {other:?}"),
        }
    }

    #[test]
    fn route_unavailable_with_fallback() {
        let cfg = ClaudeCliConfig {
            fallback_provider: Some("ollama".into()),
            ..Default::default()
        };
        let route = choose_route(
            &ProviderType::ClaudeCli,
            ClaudeHealth::Unavailable(AgentErrorKind::NotLoggedIn),
            &live(),
            "Assist",
            &cfg,
        );
        match route {
            Route::Fallback(p) => {
                assert_eq!(p.provider_type, "ollama");
                assert_eq!(p.api_key, None);
            }
            other => panic!("expected the fallback, got {other:?}"),
        }
    }

    #[test]
    fn route_unavailable_no_fallback() {
        let route = choose_route(
            &ProviderType::ClaudeCli,
            ClaudeHealth::Unavailable(AgentErrorKind::RateLimited),
            &live(),
            "Assist",
            &ClaudeCliConfig::default(),
        );
        assert!(matches!(route, Route::Stateless), "{route:?}");
    }

    #[test]
    fn route_non_claude_provider() {
        let route = choose_route(
            &ProviderType::Ollama,
            ClaudeHealth::Healthy,
            &live(),
            "Assist",
            &ClaudeCliConfig::default(),
        );
        assert!(matches!(route, Route::Stateless), "{route:?}");
    }

    #[test]
    fn routing_recap_deep_past_deep_utility_stateless() {
        let cfg = ClaudeCliConfig::default();
        let key = |scope: &SessionScope, mode: &str| {
            match choose_route(&ProviderType::ClaudeCli, ClaudeHealth::Healthy, scope, mode, &cfg) {
                Route::ClaudeSession(k) => Some(k.tier),
                _ => None,
            }
        };
        assert_eq!(key(&live(), "Recap"), Some(Tier::Deep));
        assert_eq!(key(&SessionScope::Past("p".into()), "MeetingSummary"), Some(Tier::Deep));
        assert_eq!(key(&SessionScope::Utility, "Assist"), None);
    }

    #[test]
    fn routing_health_tracker_announces_once_per_outage() {
        let health = HealthTracker::default();
        assert_eq!(health.get(), ClaudeHealth::Healthy);
        assert!(!health.mark_healthy());
        health.mark_unavailable(AgentErrorKind::NotLoggedIn);
        assert_eq!(health.get(), ClaudeHealth::Unavailable(AgentErrorKind::NotLoggedIn));
        assert!(health.take_fallback_notice());
        assert!(!health.take_fallback_notice());
        assert!(health.mark_healthy());
        health.mark_unavailable(AgentErrorKind::RateLimited);
        assert!(health.take_fallback_notice(), "a new outage is announced again");
    }

    #[test]
    fn routing_error_classes() {
        assert!(is_restartable(&AgentError::Crashed("x".into())));
        assert!(is_restartable(&AgentError::Timeout));
        assert!(is_restartable(&AgentError::Cli(AgentErrorKind::NotLoggedIn, "x".into())));
        assert!(is_restartable(&AgentError::Cli(AgentErrorKind::RateLimited, "x".into())));
        assert!(!is_restartable(&AgentError::Cli(AgentErrorKind::Other, "too long".into())));
        assert!(!is_restartable(&AgentError::Spawn("not installed".into())));
        assert!(!is_restartable(&AgentError::Cancelled));

        assert_eq!(unavailable_kind(&AgentError::Cancelled), None);
        assert_eq!(unavailable_kind(&AgentError::Cli(AgentErrorKind::Other, "x".into())), None);
        assert_eq!(
            unavailable_kind(&AgentError::Spawn("Claude CLI not installed".into())),
            Some(AgentErrorKind::NotInstalled)
        );
        assert_eq!(unavailable_kind(&AgentError::Timeout), Some(AgentErrorKind::Crashed));
        assert_eq!(
            unavailable_kind(&AgentError::Crashed("exit code 1".into())),
            Some(AgentErrorKind::Crashed)
        );
    }

    #[test]
    fn routing_document_preload_respects_cap() {
        assert_eq!(document_preload("  "), (Vec::new(), false));
        let (msgs, rag) = document_preload("## a.md (md)\nhello");
        assert!(!rag);
        assert_eq!(msgs, vec!["## Context documents\n## a.md (md)\nhello"]);
        let huge = "word ".repeat(DOC_PRELOAD_TOKEN_CAP); // ~125k tokens
        assert_eq!(document_preload(&huge), (Vec::new(), true));
    }

    #[test]
    fn routing_compose_delta_orders_sections() {
        let seg = Seg { text: "We ship Friday".into(), speaker: "Them".into(), timestamp_ms: 61_000 };
        let p = compose_delta(
            &[seg],
            &[("## Relevant document excerpts", "chunk 1"), ("## Empty", " ")],
            "Suggest a reply.",
            None,
            Some("When?"),
        );
        assert_eq!(
            p,
            "## New transcript since last request\n[01:01] Them: We ship Friday\n\n\
             ## Relevant document excerpts\nchunk 1\n\n\
             ## Task instructions\nSuggest a reply.\n\n\
             ## Detected question\nWhen?"
        );
        assert_eq!(compose_delta(&[], &[], "Do it.", Some("Q"), None), "## Task instructions\nDo it.\n\n## Question\nQ");
    }

    #[test]
    fn routing_segments_from_json_relative_and_labelled() {
        let json = r#"[{"id":"a","text":"Hi","speaker":"User","timestamp_ms":1700000065000},
                       {"text":"  ","speaker":"Them","timestamp_ms":1700000066000},
                       {"text":"Yo","speaker":"Them","timestamp_ms":1700000070000}]"#;
        let segs = segments_from_json(json, 1_700_000_000_000);
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[0].speaker.as_str(), segs[0].timestamp_ms), ("You", 65_000));
        assert_eq!((segs[1].speaker.as_str(), segs[1].timestamp_ms), ("Them", 70_000));
        assert!(segments_from_json("not json", 0).is_empty());
        assert_eq!(relative_ms(5_000, 1_700_000_000_000), 5_000, "already relative");
    }

    #[test]
    fn routing_system_prompts() {
        let p = live_system_prompt("", "", "Be brief.");
        assert!(p.starts_with("You are NexQ, a real-time AI meeting assistant."));
        assert!(p.contains("Additional Instructions: Be brief."));
        assert!(p.contains("## How this session works"));
        let p = live_system_prompt("Interview coach.", "Them = interviewer", "");
        assert!(p.starts_with("Interview coach.\n\nThem = interviewer\n\n## How"));
        assert!(past_system_prompt("x").ends_with("Additional Instructions: x"));
        assert_eq!(
            session_key_label(&SessionKey { scope: live(), tier: Tier::Fast }),
            "live:m1:fast"
        );
    }

    // ── ask_meeting_session with a fake spawner ─────────────────────────────

    /// Scripted session: `fail_next` makes the next ask fail with that error.
    #[derive(Default)]
    struct Script {
        appended: Mutex<Vec<Vec<String>>>,
        prompts: Mutex<Vec<String>>,
        fail_next: Mutex<Vec<AgentError>>,
        spawns: AtomicUsize,
    }

    struct FakeSession {
        script: Arc<Script>,
        index: usize,
        alive: AtomicBool,
    }

    #[async_trait]
    impl SessionHandle for FakeSession {
        async fn append_context(&self, text: &str) -> Result<(), AgentError> {
            self.script.appended.lock().unwrap()[self.index].push(text.to_string());
            Ok(())
        }
        async fn ask(
            &self,
            prompt: &str,
            on_delta: OnDelta,
            _cancel: Arc<CancelToken>,
        ) -> Result<TurnResult, AgentError> {
            self.script.prompts.lock().unwrap().push(prompt.to_string());
            let fail = {
                let mut queue = self.script.fail_next.lock().unwrap();
                (!queue.is_empty()).then(|| queue.remove(0))
            };
            if let Some(err) = fail {
                if matches!(err, AgentError::Crashed(_)) {
                    self.alive.store(false, Ordering::SeqCst);
                }
                return Err(err);
            }
            on_delta("ok");
            Ok(TurnResult { text: "ok".into(), input_tokens: 7, output_tokens: 1, ..Default::default() })
        }
        fn is_alive(&self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
        async fn close(&self) {
            self.alive.store(false, Ordering::SeqCst);
        }
        fn model(&self) -> String {
            "haiku".into()
        }
    }

    struct FakeSpawner(Arc<Script>);

    #[async_trait]
    impl SessionSpawner for FakeSpawner {
        async fn spawn(&self, _cfg: SessionConfig) -> Result<Arc<dyn SessionHandle>, AgentError> {
            let index = self.0.spawns.fetch_add(1, Ordering::SeqCst);
            self.0.appended.lock().unwrap().push(Vec::new());
            Ok(Arc::new(FakeSession { script: self.0.clone(), index, alive: AtomicBool::new(true) }))
        }
    }

    fn seg(text: &str) -> Seg {
        Seg { text: text.into(), speaker: "Them".into(), timestamp_ms: 0 }
    }

    fn setup(preload: &[&str]) -> (AgentSessionManager, MeetingSessions, Arc<Script>, SessionKey) {
        let script = Arc::new(Script::default());
        let manager = AgentSessionManager::new(
            Arc::new(FakeSpawner(script.clone())),
            Arc::new(NoopStatusSink),
            Arc::new(RwLock::new(ClaudeCliConfig::default())),
        );
        let key = SessionKey { scope: live(), tier: Tier::Fast };
        let sessions = MeetingSessions::default();
        sessions.insert(
            key.clone(),
            SessionRecord {
                system_prompt: "sp".into(),
                preload: preload.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
        );
        (manager, sessions, script, key)
    }

    async fn ask(
        manager: &AgentSessionManager,
        sessions: &MeetingSessions,
        key: &SessionKey,
        finals: &[Seg],
        sent: &Mutex<Vec<(String, usize)>>,
    ) -> Result<TurnResult, AgentError> {
        ask_meeting_session(
            manager,
            sessions,
            key,
            finals,
            |new| compose_delta(new, &[], "Help.", None, None),
            |prompt, model, n| {
                assert_eq!(model, "haiku");
                sent.lock().unwrap().push((prompt.to_string(), n));
            },
            || Box::new(|_: &str| {}),
            Arc::new(CancelToken::new()),
        )
        .await
    }

    #[tokio::test]
    async fn routing_session_sends_only_new_segments() {
        let (manager, sessions, script, key) = setup(&["docs"]);
        let sent = Mutex::new(Vec::new());
        let mut finals = vec![seg("a"), seg("b")];
        ask(&manager, &sessions, &key, &finals, &sent).await.unwrap();
        finals.push(seg("c"));
        ask(&manager, &sessions, &key, &finals, &sent).await.unwrap();
        ask(&manager, &sessions, &key, &finals, &sent).await.unwrap();

        let prompts = script.prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 3);
        assert!(prompts[0].contains("Them: a\n[00:00] Them: b"));
        assert!(prompts[1].contains("Them: c") && !prompts[1].contains("Them: a"));
        assert!(!prompts[2].contains("## New transcript"), "nothing new: {}", prompts[2]);
        let counts: Vec<usize> = sent.lock().unwrap().iter().map(|(_, n)| *n).collect();
        assert_eq!(counts, vec![2, 1, 0]);
        assert_eq!(script.spawns.load(Ordering::SeqCst), 1);
        assert_eq!(script.appended.lock().unwrap()[0], vec!["docs"]);
        let record = sessions.get(&key).unwrap();
        assert_eq!(record.segments_sent, 3);
        assert_eq!(record.replay.len(), 3);
        assert_eq!(manager.usage(&live()).input_tokens, 21, "usage counted per turn");
    }

    #[tokio::test]
    async fn routing_crash_restarts_once_with_replay() {
        let (manager, sessions, script, key) = setup(&["docs"]);
        let sent = Mutex::new(Vec::new());
        let finals = vec![seg("a")];
        ask(&manager, &sessions, &key, &finals, &sent).await.unwrap();

        let finals = vec![seg("a"), seg("b")];
        script.fail_next.lock().unwrap().push(AgentError::Crashed("exit code 1".into()));
        ask(&manager, &sessions, &key, &finals, &sent).await.unwrap();

        assert_eq!(script.spawns.load(Ordering::SeqCst), 2, "restarted once");
        let prompts = script.prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 3);
        assert_eq!(prompts[1], prompts[2], "the same delta is resent");
        assert!(prompts[2].contains("Them: b") && !prompts[2].contains("Them: a"));
        // The new session got the preload plus the first delta, not the failed one.
        let replayed = script.appended.lock().unwrap()[1].clone();
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0], "docs");
        assert_eq!(replayed[1], prompts[0]);
        assert_eq!(sent.lock().unwrap().len(), 2, "one start event per request");
        assert_eq!(sessions.get(&key).unwrap().segments_sent, 2);
    }

    #[tokio::test]
    async fn routing_second_failure_rolls_back_cursor_and_errors() {
        let (manager, sessions, script, key) = setup(&[]);
        let sent = Mutex::new(Vec::new());
        let finals = vec![seg("a")];
        for _ in 0..2 {
            script
                .fail_next
                .lock()
                .unwrap()
                .push(AgentError::Cli(AgentErrorKind::NotLoggedIn, "Not logged in".into()));
        }
        let err = ask(&manager, &sessions, &key, &finals, &sent).await.unwrap_err();
        assert_eq!(unavailable_kind(&err), Some(AgentErrorKind::NotLoggedIn));
        assert_eq!(script.spawns.load(Ordering::SeqCst), 2, "one restart only");
        let record = sessions.get(&key).unwrap();
        assert_eq!(record.segments_sent, 0, "undelivered segments are rolled back");
        assert!(record.replay.is_empty());

        // Healthy again: the next request carries the rolled-back segment.
        ask(&manager, &sessions, &key, &finals, &sent).await.unwrap();
        let prompts = script.prompts.lock().unwrap().clone();
        assert!(prompts.last().unwrap().contains("Them: a"));
    }

    #[tokio::test]
    async fn routing_repeated_crash_takes_fallback_route() {
        // Claude exits mid-turn, and again after the restart: the request is
        // an outage, and with a fallback configured the fallback route wins.
        let (manager, sessions, script, key) = setup(&["docs"]);
        let sent = Mutex::new(Vec::new());
        for _ in 0..2 {
            script.fail_next.lock().unwrap().push(AgentError::Crashed("exit code 1".into()));
        }
        let err = ask(&manager, &sessions, &key, &[seg("a")], &sent).await.unwrap_err();
        let kind = unavailable_kind(&err).expect("a repeated crash is an outage");
        assert_eq!(kind, AgentErrorKind::Crashed);
        let cfg = ClaudeCliConfig { fallback_provider: Some("ollama".into()), ..Default::default() };
        let route = choose_route(&ProviderType::ClaudeCli, ClaudeHealth::Unavailable(kind), &key.scope, "Assist", &cfg);
        assert!(matches!(&route, Route::Fallback(p) if p.provider_type == "ollama"), "{route:?}");
    }

    #[tokio::test]
    async fn routing_other_cli_error_is_not_retried() {
        let (manager, sessions, script, key) = setup(&[]);
        let sent = Mutex::new(Vec::new());
        script
            .fail_next
            .lock()
            .unwrap()
            .push(AgentError::Cli(AgentErrorKind::Other, "Prompt is too long".into()));
        let err = ask(&manager, &sessions, &key, &[seg("a")], &sent).await.unwrap_err();
        assert!(matches!(err, AgentError::Cli(AgentErrorKind::Other, _)));
        assert_eq!(script.spawns.load(Ordering::SeqCst), 1);
        assert_eq!(unavailable_kind(&err), None);
    }

    #[tokio::test]
    async fn routing_cancel_before_send_sends_nothing() {
        let (manager, sessions, script, key) = setup(&[]);
        let cancel = Arc::new(CancelToken::new());
        cancel.cancel();
        let err = ask_meeting_session(
            &manager,
            &sessions,
            &key,
            &[seg("a")],
            |_| String::from("p"),
            |_, _, _| panic!("nothing is sent once cancelled"),
            || Box::new(|_: &str| {}),
            cancel,
        )
        .await
        .unwrap_err();
        assert_eq!(err, AgentError::Cancelled);
        assert!(script.prompts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn routing_restarted_elsewhere_resyncs_cursor_from_record() {
        // A session the reaper closed (or that died between requests) comes
        // back with preload + replay; the cursor follows the record.
        let (manager, sessions, script, key) = setup(&["docs"]);
        let sent = Mutex::new(Vec::new());
        let finals = vec![seg("a")];
        ask(&manager, &sessions, &key, &finals, &sent).await.unwrap();
        manager.close_scope(&live()).await;
        let finals = vec![seg("a"), seg("b")];
        ask(&manager, &sessions, &key, &finals, &sent).await.unwrap();
        assert_eq!(script.spawns.load(Ordering::SeqCst), 2);
        let last = script.prompts.lock().unwrap().last().cloned().unwrap();
        assert!(last.contains("Them: b") && !last.contains("Them: a"), "{last}");
        assert_eq!(script.appended.lock().unwrap()[1].len(), 2);
    }

    #[tokio::test]
    async fn routing_missing_record_is_an_error() {
        let (manager, sessions, _script, key) = setup(&[]);
        sessions.remove_scope(&live());
        let sent = Mutex::new(Vec::new());
        assert!(matches!(
            ask(&manager, &sessions, &key, &[], &sent).await,
            Err(AgentError::Crashed(_))
        ));
    }
}
