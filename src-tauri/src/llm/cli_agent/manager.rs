//! `AgentSessionManager`: the Claude CLI sessions NexQ keeps open, keyed by
//! meeting scope and model tier.
//!
//! - Lazy start: `ensure` spawns (once per key, even under concurrent calls)
//!   and preloads a session, and restarts one that died.
//! - Each session has a delta cursor so a request only sends transcript
//!   segments the session has not seen yet; it resets when the session is
//!   (re)started.
//! - Past-meeting sessions close after 15 min without AI activity
//!   (`reap_idle`, driven by `run_idle_reaper` every 60 s).
//! - Every state transition is reported through a [`StatusSink`]
//!   (`agent_session_status` in the app).
//!
//! Sessions are stored as `Arc<dyn SessionHandle>` and created by a
//! [`SessionSpawner`], so the manager is testable without a process.

use super::command::SessionConfig;
use super::context::DeltaCursor;
use super::detect::find_claude_binary;
use super::protocol::TurnResult;
use super::session::{AgentError, ClaudeSession};
use crate::llm::provider::CancelToken;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Past-meeting sessions idle this long are closed.
pub const PAST_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How often the idle reaper runs.
pub const REAP_INTERVAL: Duration = Duration::from_secs(60);
/// Tauri event carrying [`AgentSessionStatus`].
pub const STATUS_EVENT: &str = "agent_session_status";

/// What a session is for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SessionScope {
    /// The running meeting (by meeting id).
    Live(String),
    /// A past meeting open in the meeting view (by meeting id).
    Past(String),
    /// Callers with no meeting (translation, knowledge-base test, wizard).
    Utility,
}

impl SessionScope {
    fn label(&self) -> &'static str {
        match self {
            SessionScope::Live(_) => "live",
            SessionScope::Past(_) => "past",
            SessionScope::Utility => "utility",
        }
    }

    fn meeting_id(&self) -> Option<String> {
        match self {
            SessionScope::Live(id) | SessionScope::Past(id) => Some(id.clone()),
            SessionScope::Utility => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tier {
    Fast,
    Deep,
}

impl Tier {
    fn label(self) -> &'static str {
        match self {
            Tier::Fast => "fast",
            Tier::Deep => "deep",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionKey {
    pub scope: SessionScope,
    pub tier: Tier,
}

/// Claude CLI model settings (persisted by the settings UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeCliConfig {
    pub fast_model: String,
    pub fast_effort: String,
    pub deep_model: String,
    pub deep_effort: String,
    /// Provider id used when Claude is unavailable; `None` = no fallback.
    pub fallback_provider: Option<String>,
    /// Model used on the fallback provider; `None` = its first listed model.
    pub fallback_model: Option<String>,
}

impl Default for ClaudeCliConfig {
    fn default() -> Self {
        Self {
            fast_model: "haiku".into(),
            fast_effort: "low".into(),
            deep_model: "sonnet".into(),
            deep_effort: "medium".into(),
            fallback_provider: None,
            fallback_model: None,
        }
    }
}

impl ClaudeCliConfig {
    /// `(model, effort)` for a tier.
    pub fn model_for(&self, tier: Tier) -> (&str, &str) {
        match tier {
            Tier::Fast => (&self.fast_model, &self.fast_effort),
            Tier::Deep => (&self.deep_model, &self.deep_effort),
        }
    }
}

/// Which tier serves `mode` (an intelligence mode such as `"Assist"` or
/// `"Recap"`) in `scope`: live meetings use Deep only for Recap, past meetings
/// always use Deep, utility calls use Fast. Identical Fast/Deep settings
/// always resolve to Fast so only one session runs.
pub fn tier_for(mode: &str, scope: &SessionScope, cfg: &ClaudeCliConfig) -> Tier {
    if cfg.model_for(Tier::Fast) == cfg.model_for(Tier::Deep) {
        return Tier::Fast;
    }
    match scope {
        SessionScope::Live(_) if mode == "Recap" => Tier::Deep,
        SessionScope::Live(_) | SessionScope::Utility => Tier::Fast,
        SessionScope::Past(_) => Tier::Deep,
    }
}

/// Cumulative token/cost accounting for one scope (estimate from the CLI).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Payload of the `agent_session_status` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSessionStatus {
    /// `"live"` | `"past"` | `"utility"`.
    pub scope: String,
    pub meeting_id: Option<String>,
    /// `"fast"` | `"deep"`.
    pub tier: String,
    /// `"starting"` | `"ready"` | `"busy"` | `"error"` | `"closed"`.
    pub state: String,
    pub model: String,
    /// Readable reason for `error`.
    pub detail: Option<String>,
    /// Cumulative usage of the whole scope (all tiers).
    pub usage: SessionUsage,
}

/// Used only when `ensure` has to spawn a session.
#[derive(Debug, Clone, Default)]
pub struct SessionInit {
    pub system_prompt: String,
    /// Context messages appended (`shouldQuery:false`) right after spawn.
    pub preload: Vec<String>,
}

/// Streaming text callback. A named alias so `async_trait` leaves the
/// higher-ranked `&str` lifetime alone.
pub type OnDelta = Box<dyn Fn(&str) + Send + Sync>;

/// What the manager needs from a running session ([`ClaudeSession`] in the
/// app, fakes in tests).
#[async_trait]
pub trait SessionHandle: Send + Sync {
    async fn append_context(&self, text: &str) -> Result<(), AgentError>;
    async fn ask(
        &self,
        prompt: &str,
        on_delta: OnDelta,
        cancel: Arc<CancelToken>,
    ) -> Result<TurnResult, AgentError>;
    fn is_alive(&self) -> bool;
    async fn close(&self);
    fn model(&self) -> String;
}

#[async_trait]
impl SessionHandle for ClaudeSession {
    async fn append_context(&self, text: &str) -> Result<(), AgentError> {
        ClaudeSession::append_context(self, text).await
    }

    async fn ask(
        &self,
        prompt: &str,
        on_delta: OnDelta,
        cancel: Arc<CancelToken>,
    ) -> Result<TurnResult, AgentError> {
        ClaudeSession::ask(self, prompt, on_delta, cancel).await
    }

    fn is_alive(&self) -> bool {
        ClaudeSession::is_alive(self)
    }

    async fn close(&self) {
        ClaudeSession::close(self).await
    }

    fn model(&self) -> String {
        ClaudeSession::model(self).to_string()
    }
}

/// Starts sessions for the manager.
#[async_trait]
pub trait SessionSpawner: Send + Sync {
    async fn spawn(&self, cfg: SessionConfig) -> Result<Arc<dyn SessionHandle>, AgentError>;
}

/// Spawns real `claude.exe` sessions (binary resolved on every spawn, so a
/// fresh install is picked up without restarting NexQ).
pub struct ClaudeSpawner;

#[async_trait]
impl SessionSpawner for ClaudeSpawner {
    async fn spawn(&self, cfg: SessionConfig) -> Result<Arc<dyn SessionHandle>, AgentError> {
        let bin = tokio::task::spawn_blocking(find_claude_binary)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| {
                AgentError::Spawn("Claude CLI not installed: claude.exe not found".into())
            })?;
        Ok(Arc::new(ClaudeSession::spawn(bin, cfg).await?))
    }
}

/// Receives every session status transition.
pub trait StatusSink: Send + Sync {
    fn emit(&self, status: &AgentSessionStatus);
}

/// Drops statuses (before the app handle exists, and in tests).
pub struct NoopStatusSink;

impl StatusSink for NoopStatusSink {
    fn emit(&self, _status: &AgentSessionStatus) {}
}

/// Emits `agent_session_status` to every window.
pub struct TauriStatusSink(pub tauri::AppHandle);

impl StatusSink for TauriStatusSink {
    fn emit(&self, status: &AgentSessionStatus) {
        use tauri::Emitter;
        if let Err(e) = self.0.emit(STATUS_EVENT, status) {
            log::warn!("cannot emit {STATUS_EVENT}: {e}");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Starting,
    Ready,
    Busy,
    Error,
    Closed,
}

impl SessionState {
    fn label(self) -> &'static str {
        match self {
            SessionState::Starting => "starting",
            SessionState::Ready => "ready",
            SessionState::Busy => "busy",
            SessionState::Error => "error",
            SessionState::Closed => "closed",
        }
    }
}

struct Entry {
    handle: Arc<dyn SessionHandle>,
    state: SessionState,
    detail: Option<String>,
    last_used: Instant,
    in_flight: usize,
    /// Turns finished on this session (any outcome).
    turns: usize,
}

pub struct AgentSessionManager {
    spawner: Arc<dyn SessionSpawner>,
    sink: Arc<dyn StatusSink>,
    config: Arc<RwLock<ClaudeCliConfig>>,
    entries: Mutex<HashMap<SessionKey, Entry>>,
    /// Serializes spawning per key so concurrent `ensure`s start one session.
    spawn_locks: Mutex<HashMap<SessionKey, Arc<AsyncMutex<()>>>>,
    cursors: Mutex<HashMap<SessionKey, Arc<AsyncMutex<DeltaCursor>>>>,
    usage: Mutex<HashMap<SessionScope, SessionUsage>>,
    shutting_down: AtomicBool,
}

impl AgentSessionManager {
    pub fn new(
        spawner: Arc<dyn SessionSpawner>,
        sink: Arc<dyn StatusSink>,
        config: Arc<RwLock<ClaudeCliConfig>>,
    ) -> Self {
        Self {
            spawner,
            sink,
            config,
            entries: Mutex::new(HashMap::new()),
            spawn_locks: Mutex::new(HashMap::new()),
            cursors: Mutex::new(HashMap::new()),
            usage: Mutex::new(HashMap::new()),
            shutting_down: AtomicBool::new(false),
        }
    }

    /// The running session for `key`, spawning it (with `init`) if there is
    /// none or it died. Concurrent calls for one key start a single session.
    /// A (re)started session gets a fresh delta cursor.
    pub async fn ensure(
        &self,
        key: &SessionKey,
        init: SessionInit,
    ) -> Result<Arc<dyn SessionHandle>, AgentError> {
        let lock = self.spawn_lock(key);
        let _spawning = lock.lock().await;
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(shut_down_error());
        }

        let stale = {
            let mut entries = self.entries.lock().unwrap();
            match entries.get_mut(key) {
                Some(entry) if entry.handle.is_alive() && !restart_after_error(key, entry) => {
                    entry.last_used = Instant::now();
                    return Ok(entry.handle.clone());
                }
                Some(_) => entries.remove(key),
                None => None,
            }
        };
        if let Some(dead) = stale {
            dead.handle.close().await;
        }

        let (model, effort) = {
            let cfg = self.config.read().unwrap();
            let (m, e) = cfg.model_for(key.tier);
            (m.to_string(), e.to_string())
        };
        self.emit(key, SessionState::Starting, &model, None);
        let cfg = SessionConfig {
            model: model.clone(),
            effort,
            system_prompt: init.system_prompt,
        };
        let handle = match self.spawner.spawn(cfg).await {
            Ok(handle) => handle,
            Err(e) => {
                self.emit(key, SessionState::Error, &model, Some(e.to_string()));
                return Err(e);
            }
        };
        for message in &init.preload {
            if let Err(e) = handle.append_context(message).await {
                handle.close().await;
                self.emit(key, SessionState::Error, &model, Some(e.to_string()));
                return Err(e);
            }
        }
        // `close_all` ran while this session was starting: don't leak it.
        if self.shutting_down.load(Ordering::SeqCst) {
            handle.close().await;
            self.emit(key, SessionState::Closed, &model, None);
            return Err(shut_down_error());
        }

        self.cursors.lock().unwrap().insert(
            key.clone(),
            Arc::new(AsyncMutex::new(DeltaCursor::default())),
        );
        self.entries.lock().unwrap().insert(
            key.clone(),
            Entry {
                handle: handle.clone(),
                state: SessionState::Ready,
                detail: None,
                last_used: Instant::now(),
                in_flight: 0,
                turns: 0,
            },
        );
        self.emit(key, SessionState::Ready, &handle.model(), None);
        Ok(handle)
    }

    /// Ask on the session `ensure` started for `key`, reporting busy, then
    /// ready (or error), and adding the turn's usage to the scope's total.
    pub async fn ask(
        &self,
        key: &SessionKey,
        prompt: &str,
        on_delta: OnDelta,
        cancel: Arc<CancelToken>,
    ) -> Result<TurnResult, AgentError> {
        let handle = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.get_mut(key).ok_or_else(|| {
                AgentError::Crashed("no Claude session is running for this request".into())
            })?;
            entry.in_flight += 1;
            entry.last_used = Instant::now();
            entry.state = SessionState::Busy;
            entry.detail = None;
            entry.handle.clone()
        };
        self.emit(key, SessionState::Busy, &handle.model(), None);

        // Settles in_flight/state even if this future is dropped mid-turn.
        let mut turn = TurnGuard {
            manager: self,
            key,
            handle: handle.clone(),
            outcome: TurnOutcome::Abandoned,
        };
        let result = handle.ask(prompt, on_delta, cancel).await;

        if let Ok(done) = &result {
            let mut usage = self.usage.lock().unwrap();
            let total = usage.entry(key.scope.clone()).or_default();
            total.input_tokens += done.input_tokens;
            total.output_tokens += done.output_tokens;
            total.cost_usd += done.cost_usd;
        }
        turn.outcome = match &result {
            Ok(_) => TurnOutcome::Succeeded,
            Err(e) if e.is_cancel() => TurnOutcome::Cancelled,
            Err(e) => TurnOutcome::Failed(e.to_string()),
        };
        drop(turn);
        result
    }

    /// End of one `ask` on `handle`: release its in-flight slot and move the
    /// session to ready/error, unless the entry was closed or replaced.
    fn finish_turn(
        &self,
        key: &SessionKey,
        handle: &Arc<dyn SessionHandle>,
        outcome: &TurnOutcome,
    ) {
        let transition = {
            let mut entries = self.entries.lock().unwrap();
            match entries.get_mut(key) {
                Some(entry) if same_session(&entry.handle, handle) => {
                    entry.in_flight = entry.in_flight.saturating_sub(1);
                    entry.turns += 1;
                    entry.last_used = Instant::now();
                    let idle = entry.in_flight == 0;
                    match outcome {
                        // A success proves the session healthy again.
                        TurnOutcome::Succeeded if idle => {
                            entry.state = SessionState::Ready;
                            entry.detail = None;
                        }
                        TurnOutcome::Cancelled | TurnOutcome::Abandoned
                            if idle && entry.state == SessionState::Busy =>
                        {
                            entry.state = SessionState::Ready;
                        }
                        TurnOutcome::Failed(reason) => {
                            entry.state = SessionState::Error;
                            entry.detail = Some(reason.clone());
                        }
                        _ => {}
                    }
                    Some((entry.state, entry.detail.clone()))
                }
                _ => None,
            }
        };
        if let Some((state, detail)) = transition {
            self.emit(key, state, &handle.model(), detail);
        }
    }

    /// The delta cursor of `key`'s current session. Hold the guard while
    /// taking new segments and sending them, so concurrent requests on one
    /// session never send the same segment twice.
    pub async fn cursor(&self, key: &SessionKey) -> OwnedMutexGuard<DeltaCursor> {
        let cursor = self
            .cursors
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_default()
            .clone();
        cursor.lock_owned().await
    }

    /// Record AI activity on `key` (postpones the idle timeout).
    pub fn touch(&self, key: &SessionKey) {
        if let Some(entry) = self.entries.lock().unwrap().get_mut(key) {
            entry.last_used = Instant::now();
        }
    }

    /// Close every tier of `scope` (meeting ended / left the meeting view). A
    /// session still starting for that scope is waited for and closed too.
    pub async fn close_scope(&self, scope: &SessionScope) {
        for tier in [Tier::Fast, Tier::Deep] {
            let key = SessionKey {
                scope: scope.clone(),
                tier,
            };
            let lock = self.spawn_lock(&key);
            let _spawning = lock.lock().await;
            self.close_key(&key).await;
        }
        self.usage.lock().unwrap().remove(scope);
    }

    /// Close every session (app exit). Sessions still starting are closed as
    /// soon as they come up, and no new session starts afterwards.
    pub async fn close_all(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let drained: Vec<(SessionKey, Entry)> = self.entries.lock().unwrap().drain().collect();
        self.cursors.lock().unwrap().clear();
        futures::future::join_all(drained.iter().map(|(_, entry)| entry.handle.close())).await;
        for (key, entry) in &drained {
            self.emit(key, SessionState::Closed, &entry.handle.model(), None);
        }
        self.usage.lock().unwrap().clear();
    }

    /// Close past-meeting sessions with no AI activity for 15 min (and no turn
    /// in flight). Keys busy starting a session are skipped this round.
    pub async fn reap_idle(&self, now: Instant) {
        // Check and remove under one lock: an `ask` or `ensure` that touches
        // the entry either happens first (not stale) or finds it gone.
        let stale: Vec<(SessionKey, Entry)> = {
            let mut entries = self.entries.lock().unwrap();
            let keys: Vec<SessionKey> = entries
                .iter()
                .filter(|(key, entry)| is_stale(key, entry, now))
                .map(|(key, _)| key.clone())
                .collect();
            let mut cursors = self.cursors.lock().unwrap();
            keys.into_iter()
                .filter_map(|key| {
                    cursors.remove(&key);
                    entries.remove_entry(&key)
                })
                .collect()
        };
        for (key, entry) in stale {
            log::info!("closing idle Claude session for {:?}", key.scope);
            self.close_removed(&key, entry).await;
        }
    }

    /// Turns finished on `key`'s current session (0 when none is running).
    pub fn turns(&self, key: &SessionKey) -> usize {
        self.entries
            .lock()
            .unwrap()
            .get(key)
            .map(|e| e.turns)
            .unwrap_or(0)
    }

    /// Take `key`'s session out of service if no turn is in flight, so the
    /// next `ensure` starts a fresh one. Reports `closed` and returns the
    /// handle for the caller to `close()` (off the request's critical path).
    /// Scope usage is kept.
    pub fn retire_if_idle(&self, key: &SessionKey) -> Option<Arc<dyn SessionHandle>> {
        let entry = {
            let mut entries = self.entries.lock().unwrap();
            match entries.get(key) {
                Some(e) if e.in_flight == 0 => entries.remove(key)?,
                _ => return None,
            }
        };
        self.cursors.lock().unwrap().remove(key);
        self.emit(key, SessionState::Closed, &entry.handle.model(), None);
        Some(entry.handle)
    }

    /// Cumulative usage of `scope` across tiers and restarts.
    pub fn usage(&self, scope: &SessionScope) -> SessionUsage {
        self.usage
            .lock()
            .unwrap()
            .get(scope)
            .copied()
            .unwrap_or_default()
    }

    /// Current status of every open session.
    pub fn snapshot(&self) -> Vec<AgentSessionStatus> {
        let mut rows: Vec<(SessionKey, SessionState, String, Option<String>)> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|(key, e)| (key.clone(), e.state, e.handle.model(), e.detail.clone()))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows.into_iter()
            .map(|(key, state, model, detail)| self.status(&key, state, &model, detail))
            .collect()
    }

    fn spawn_lock(&self, key: &SessionKey) -> Arc<AsyncMutex<()>> {
        self.spawn_locks
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_default()
            .clone()
    }

    async fn close_key(&self, key: &SessionKey) {
        let removed = self.entries.lock().unwrap().remove(key);
        self.cursors.lock().unwrap().remove(key);
        if let Some(entry) = removed {
            self.close_removed(key, entry).await;
        }
    }

    /// Close an entry (and cursor) already taken out of the maps.
    async fn close_removed(&self, key: &SessionKey, entry: Entry) {
        entry.handle.close().await;
        self.emit(key, SessionState::Closed, &entry.handle.model(), None);
    }

    fn status(
        &self,
        key: &SessionKey,
        state: SessionState,
        model: &str,
        detail: Option<String>,
    ) -> AgentSessionStatus {
        AgentSessionStatus {
            scope: key.scope.label().to_string(),
            meeting_id: key.scope.meeting_id(),
            tier: key.tier.label().to_string(),
            state: state.label().to_string(),
            model: model.to_string(),
            detail,
            usage: self.usage(&key.scope),
        }
    }

    fn emit(&self, key: &SessionKey, state: SessionState, model: &str, detail: Option<String>) {
        self.sink.emit(&self.status(key, state, model, detail));
    }
}

/// How an `ask` ended, for [`TurnGuard`].
enum TurnOutcome {
    /// The caller's future was dropped mid-turn.
    Abandoned,
    Succeeded,
    Cancelled,
    Failed(String),
}

/// Settles a turn's in-flight slot and state when dropped, so a caller that
/// drops `ask` (select!, timeout, abort) never leaves a session stuck busy.
struct TurnGuard<'a> {
    manager: &'a AgentSessionManager,
    key: &'a SessionKey,
    handle: Arc<dyn SessionHandle>,
    outcome: TurnOutcome,
}

impl Drop for TurnGuard<'_> {
    fn drop(&mut self) {
        self.manager
            .finish_turn(self.key, &self.handle, &self.outcome);
    }
}

/// Drive `reap_idle` every [`REAP_INTERVAL`] for the app's lifetime.
pub async fn run_idle_reaper(manager: Arc<AgentSessionManager>) {
    let mut tick = tokio::time::interval(REAP_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        manager.reap_idle(Instant::now()).await;
    }
}

/// A Utility session left in `error` by a failed turn (e.g. context too
/// long) is replaced on its next use: Utility requests are unrelated, so
/// nothing is lost. Meeting sessions keep theirs; restarting them means
/// replaying context, which the caller (Task 15) owns.
fn restart_after_error(key: &SessionKey, entry: &Entry) -> bool {
    key.scope == SessionScope::Utility
        && entry.state == SessionState::Error
        && entry.in_flight == 0
}

fn is_stale(key: &SessionKey, entry: &Entry, now: Instant) -> bool {
    matches!(key.scope, SessionScope::Past(_))
        && entry.in_flight == 0
        && now.saturating_duration_since(entry.last_used) >= PAST_IDLE_TIMEOUT
}

/// Same session object (compares data pointers; vtable pointers may differ).
fn same_session(a: &Arc<dyn SessionHandle>, b: &Arc<dyn SessionHandle>) -> bool {
    std::ptr::eq(Arc::as_ptr(a) as *const (), Arc::as_ptr(b) as *const ())
}

fn shut_down_error() -> AgentError {
    AgentError::Spawn("NexQ is shutting down".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct FakeSession {
        model: String,
        alive: AtomicBool,
        closed: AtomicBool,
        appended: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl SessionHandle for FakeSession {
        async fn append_context(&self, text: &str) -> Result<(), AgentError> {
            self.appended.lock().unwrap().push(text.to_string());
            Ok(())
        }

        async fn ask(
            &self,
            prompt: &str,
            on_delta: OnDelta,
            _cancel: Arc<CancelToken>,
        ) -> Result<TurnResult, AgentError> {
            if let Some(msg) = prompt.strip_prefix("fail:") {
                return Err(AgentError::Crashed(msg.to_string()));
            }
            if prompt == "block" {
                // A turn that never finishes (until the caller drops it).
                std::future::pending::<()>().await;
            }
            if prompt == "yield" {
                // Still in flight across a scheduler turn.
                tokio::task::yield_now().await;
            }
            on_delta(prompt);
            Ok(TurnResult {
                text: prompt.to_string(),
                input_tokens: 10,
                output_tokens: 2,
                cost_usd: 0.5,
                ..Default::default()
            })
        }

        fn is_alive(&self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }

        async fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
            self.alive.store(false, Ordering::SeqCst);
        }

        fn model(&self) -> String {
            self.model.clone()
        }
    }

    #[derive(Default)]
    struct FakeSpawner {
        spawned: Mutex<Vec<(SessionConfig, Arc<FakeSession>)>>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl SessionSpawner for FakeSpawner {
        async fn spawn(&self, cfg: SessionConfig) -> Result<Arc<dyn SessionHandle>, AgentError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            let s = Arc::new(FakeSession {
                model: cfg.model.clone(),
                alive: AtomicBool::new(true),
                closed: AtomicBool::new(false),
                appended: Mutex::new(Vec::new()),
            });
            self.spawned.lock().unwrap().push((cfg, s.clone()));
            Ok(s)
        }
    }

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<AgentSessionStatus>>);

    impl StatusSink for RecordingSink {
        fn emit(&self, status: &AgentSessionStatus) {
            self.0.lock().unwrap().push(status.clone());
        }
    }

    impl RecordingSink {
        fn states(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|s| s.state.clone())
                .collect()
        }
    }

    fn setup() -> (AgentSessionManager, Arc<FakeSpawner>, Arc<RecordingSink>) {
        let spawner = Arc::new(FakeSpawner::default());
        let sink = Arc::new(RecordingSink::default());
        let mgr = AgentSessionManager::new(
            spawner.clone(),
            sink.clone(),
            Arc::new(RwLock::new(ClaudeCliConfig::default())),
        );
        (mgr, spawner, sink)
    }

    fn key(scope: SessionScope, tier: Tier) -> SessionKey {
        SessionKey { scope, tier }
    }

    fn init(preload: &[&str]) -> SessionInit {
        SessionInit {
            system_prompt: "sp".into(),
            preload: preload.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn no_delta() -> OnDelta {
        Box::new(|_| {})
    }

    #[test]
    fn tier_routing() {
        let cfg = ClaudeCliConfig::default();
        let live = SessionScope::Live("m1".into());
        let past = SessionScope::Past("m1".into());
        assert_eq!(tier_for("Recap", &live, &cfg), Tier::Deep);
        assert_eq!(tier_for("Assist", &live, &cfg), Tier::Fast);
        assert_eq!(tier_for("Assist", &past, &cfg), Tier::Deep);
        assert_eq!(tier_for("Recap", &SessionScope::Utility, &cfg), Tier::Fast);
        let same = ClaudeCliConfig {
            deep_model: "haiku".into(),
            deep_effort: "low".into(),
            ..ClaudeCliConfig::default()
        };
        assert_eq!(tier_for("Recap", &live, &same), Tier::Fast);
        assert_eq!(tier_for("Assist", &past, &same), Tier::Fast);
    }

    #[test]
    fn default_config_is_haiku_low_sonnet_medium() {
        let cfg = ClaudeCliConfig::default();
        assert_eq!(cfg.model_for(Tier::Fast), ("haiku", "low"));
        assert_eq!(cfg.model_for(Tier::Deep), ("sonnet", "medium"));
        assert_eq!(cfg.fallback_provider, None);
        let partial: ClaudeCliConfig = serde_json::from_str(r#"{"deep_model":"opus"}"#).unwrap();
        assert_eq!(partial.model_for(Tier::Deep), ("opus", "medium"));
    }

    #[test]
    fn status_serializes_snake_case() {
        let s = AgentSessionStatus {
            scope: "live".into(),
            meeting_id: Some("m1".into()),
            tier: "fast".into(),
            state: "ready".into(),
            model: "haiku".into(),
            detail: None,
            usage: SessionUsage {
                input_tokens: 1,
                output_tokens: 2,
                cost_usd: 0.5,
            },
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["meeting_id"], "m1");
        assert_eq!(v["usage"]["input_tokens"], 1);
        assert_eq!(v["usage"]["output_tokens"], 2);
        assert_eq!(v["usage"]["cost_usd"], 0.5);
    }

    #[tokio::test]
    async fn ensure_spawns_once_preloads_and_restarts_dead() {
        let (mgr, spawner, sink) = setup();
        let k = key(SessionScope::Live("m1".into()), Tier::Deep);
        let (a, b) = tokio::join!(
            mgr.ensure(&k, init(&["doc1", "doc2"])),
            mgr.ensure(&k, init(&["other"]))
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(
            spawner.calls.load(Ordering::SeqCst),
            1,
            "concurrent ensure spawned twice"
        );
        assert_eq!(a.model(), "sonnet");
        assert_eq!(b.model(), "sonnet");
        {
            let spawned = spawner.spawned.lock().unwrap();
            let (cfg, first) = &spawned[0];
            assert_eq!(
                (cfg.model.as_str(), cfg.effort.as_str()),
                ("sonnet", "medium")
            );
            assert_eq!(cfg.system_prompt, "sp");
            assert_eq!(*first.appended.lock().unwrap(), vec!["doc1", "doc2"]);
        }
        assert_eq!(sink.states(), vec!["starting", "ready"]);

        // The cursor advances, and resets when the session is restarted.
        let segs = vec![crate::llm::cli_agent::context::Seg {
            id: "s1".into(),
            text: "a".into(),
            speaker: "You".into(),
            timestamp_ms: 0,
        }];
        assert_eq!(mgr.cursor(&k).await.take_new(&segs).len(), 1);
        assert_eq!(mgr.cursor(&k).await.sent(), 1);

        spawner.spawned.lock().unwrap()[0]
            .1
            .alive
            .store(false, Ordering::SeqCst);
        mgr.ensure(&k, init(&["doc3"])).await.unwrap();
        assert_eq!(spawner.calls.load(Ordering::SeqCst), 2);
        assert!(spawner.spawned.lock().unwrap()[0]
            .1
            .closed
            .load(Ordering::SeqCst));
        assert_eq!(
            *spawner.spawned.lock().unwrap()[1]
                .1
                .appended
                .lock()
                .unwrap(),
            vec!["doc3"]
        );
        assert_eq!(mgr.cursor(&k).await.sent(), 0);
    }

    #[tokio::test]
    async fn ask_reports_busy_ready_and_accumulates_usage() {
        let (mgr, _spawner, sink) = setup();
        let scope = SessionScope::Past("m2".into());
        let k = key(scope.clone(), Tier::Deep);
        assert!(matches!(
            mgr.ask(&k, "q", no_delta(), Arc::new(CancelToken::new()))
                .await,
            Err(AgentError::Crashed(_))
        ));
        mgr.ensure(&k, init(&[])).await.unwrap();
        let got = Arc::new(Mutex::new(String::new()));
        let sinkbuf = got.clone();
        let r = mgr
            .ask(
                &k,
                "q1",
                Box::new(move |d| sinkbuf.lock().unwrap().push_str(d)),
                Arc::new(CancelToken::new()),
            )
            .await
            .unwrap();
        assert_eq!(r.text, "q1");
        assert_eq!(*got.lock().unwrap(), "q1");
        mgr.ask(&k, "q2", no_delta(), Arc::new(CancelToken::new()))
            .await
            .unwrap();
        assert_eq!(
            mgr.usage(&scope),
            SessionUsage {
                input_tokens: 20,
                output_tokens: 4,
                cost_usd: 1.0
            }
        );
        assert_eq!(
            sink.states(),
            vec!["starting", "ready", "busy", "ready", "busy", "ready"]
        );
        let last = sink.0.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.scope, "past");
        assert_eq!(last.meeting_id.as_deref(), Some("m2"));
        assert_eq!(last.tier, "deep");
        assert_eq!(last.model, "sonnet");
        assert_eq!(last.usage.input_tokens, 20);

        let err = mgr
            .ask(&k, "fail:boom", no_delta(), Arc::new(CancelToken::new()))
            .await;
        assert!(err.is_err());
        let snap = mgr.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].state, "error");
        assert!(snap[0].detail.as_deref().unwrap().contains("boom"));
    }

    #[tokio::test]
    async fn close_scope_closes_every_tier_of_that_scope_only() {
        let (mgr, spawner, sink) = setup();
        let live = SessionScope::Live("m1".into());
        mgr.ensure(&key(live.clone(), Tier::Fast), init(&[]))
            .await
            .unwrap();
        mgr.ensure(&key(live.clone(), Tier::Deep), init(&[]))
            .await
            .unwrap();
        mgr.ensure(&key(SessionScope::Utility, Tier::Fast), init(&[]))
            .await
            .unwrap();
        mgr.close_scope(&live).await;
        let closed: Vec<bool> = spawner
            .spawned
            .lock()
            .unwrap()
            .iter()
            .map(|(_, s)| s.closed.load(Ordering::SeqCst))
            .collect();
        assert_eq!(closed, vec![true, true, false]);
        assert_eq!(sink.states().iter().filter(|s| *s == "closed").count(), 2);
        let snap = mgr.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].scope, "utility");

        mgr.close_all().await;
        assert!(mgr.snapshot().is_empty());
        assert!(spawner.spawned.lock().unwrap()[2]
            .1
            .closed
            .load(Ordering::SeqCst));
        assert!(mgr
            .ensure(&key(SessionScope::Utility, Tier::Fast), init(&[]))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn reap_idle_closes_only_stale_past() {
        let (mgr, spawner, _sink) = setup();
        let stale = key(SessionScope::Past("old".into()), Tier::Deep);
        let fresh = key(SessionScope::Past("new".into()), Tier::Deep);
        let live = key(SessionScope::Live("m1".into()), Tier::Fast);
        let utility = key(SessionScope::Utility, Tier::Fast);
        for k in [&stale, &fresh, &live, &utility] {
            mgr.ensure(k, init(&[])).await.unwrap();
        }
        // 20 min from now: everything is 20 min idle except `fresh` (1 min).
        let now = Instant::now() + Duration::from_secs(20 * 60);
        mgr.entries
            .lock()
            .unwrap()
            .get_mut(&fresh)
            .unwrap()
            .last_used = now - Duration::from_secs(60);
        mgr.reap_idle(now).await;

        let closed: Vec<bool> = spawner
            .spawned
            .lock()
            .unwrap()
            .iter()
            .map(|(_, s)| s.closed.load(Ordering::SeqCst))
            .collect();
        assert_eq!(closed, vec![true, false, false, false]);
        let mut left: Vec<String> = mgr.snapshot().into_iter().map(|s| s.scope).collect();
        left.sort();
        assert_eq!(left, vec!["live", "past", "utility"]);

        // Touch keeps a past session alive.
        mgr.touch(&fresh);
        mgr.reap_idle(Instant::now() + Duration::from_secs(14 * 60))
            .await;
        assert!(!spawner.spawned.lock().unwrap()[1]
            .1
            .closed
            .load(Ordering::SeqCst));
    }

    fn in_flight(mgr: &AgentSessionManager, k: &SessionKey) -> usize {
        mgr.entries.lock().unwrap()[k].in_flight
    }

    #[tokio::test]
    async fn dropped_ask_releases_in_flight_and_busy() {
        let (mgr, spawner, sink) = setup();
        let k = key(SessionScope::Past("m3".into()), Tier::Deep);
        mgr.ensure(&k, init(&[])).await.unwrap();
        let dropped = tokio::time::timeout(
            Duration::from_millis(50),
            mgr.ask(&k, "block", no_delta(), Arc::new(CancelToken::new())),
        )
        .await;
        assert!(
            dropped.is_err(),
            "blocking ask should have been dropped by the timeout"
        );
        assert_eq!(in_flight(&mgr, &k), 0);
        assert_eq!(mgr.snapshot()[0].state, "ready");
        assert_eq!(sink.states(), vec!["starting", "ready", "busy", "ready"]);
        // No longer pinned in flight, so the reaper can close it once idle.
        mgr.reap_idle(Instant::now() + Duration::from_secs(20 * 60))
            .await;
        assert!(spawner.spawned.lock().unwrap()[0]
            .1
            .closed
            .load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn reap_idle_keeps_past_session_with_turn_in_flight() {
        let (mgr, spawner, _sink) = setup();
        let k = key(SessionScope::Past("m4".into()), Tier::Deep);
        mgr.ensure(&k, init(&[])).await.unwrap();
        let now = Instant::now() + Duration::from_secs(20 * 60);
        let ask = mgr.ask(&k, "block", no_delta(), Arc::new(CancelToken::new()));
        tokio::pin!(ask);
        tokio::select! {
            biased;
            _ = &mut ask => panic!("blocking ask finished"),
            _ = async {
                // Let the ask register as in flight, then reap 20 min later.
                tokio::task::yield_now().await;
                assert_eq!(in_flight(&mgr, &k), 1);
                mgr.reap_idle(now).await;
            } => {}
        }
        assert!(!spawner.spawned.lock().unwrap()[0]
            .1
            .closed
            .load(Ordering::SeqCst));
        assert_eq!(mgr.snapshot().len(), 1);
    }

    #[tokio::test]
    async fn success_after_concurrent_failure_returns_to_ready() {
        let (mgr, _spawner, _sink) = setup();
        let k = key(SessionScope::Utility, Tier::Fast);
        mgr.ensure(&k, init(&[])).await.unwrap();
        let cancel = || Arc::new(CancelToken::new());
        let (ok, failed) = tokio::join!(mgr.ask(&k, "yield", no_delta(), cancel()), async {
            // Fails while the first turn is still in flight.
            assert_eq!(in_flight(&mgr, &k), 1);
            mgr.ask(&k, "fail:boom", no_delta(), cancel()).await
        });
        assert!(ok.is_ok() && failed.is_err());
        let snap = mgr.snapshot();
        assert_eq!(snap[0].state, "ready");
        assert_eq!(snap[0].detail, None);
    }

    #[tokio::test]
    async fn ensure_replaces_errored_utility_session_only() {
        let (mgr, spawner, _sink) = setup();
        let utility = key(SessionScope::Utility, Tier::Fast);
        let live = key(SessionScope::Live("m1".into()), Tier::Fast);
        for k in [&utility, &live] {
            mgr.ensure(k, init(&[])).await.unwrap();
            let failed = mgr
                .ask(k, "fail:prompt too long", no_delta(), Arc::new(CancelToken::new()))
                .await;
            assert!(failed.is_err());
        }
        assert_eq!(spawner.calls.load(Ordering::SeqCst), 2);

        // Utility: the errored (still alive) session is closed and replaced.
        mgr.ensure(&utility, init(&[])).await.unwrap();
        assert_eq!(spawner.calls.load(Ordering::SeqCst), 3);
        assert!(spawner.spawned.lock().unwrap()[0].1.closed.load(Ordering::SeqCst));
        // Live: the caller owns the restart (context replay); it is reused.
        mgr.ensure(&live, init(&[])).await.unwrap();
        assert_eq!(spawner.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn turns_count_and_retire_if_idle() {
        let (mgr, spawner, sink) = setup();
        let k = key(SessionScope::Utility, Tier::Fast);
        assert_eq!(mgr.turns(&k), 0);
        assert!(mgr.retire_if_idle(&k).is_none());
        mgr.ensure(&k, init(&[])).await.unwrap();
        for q in ["a", "b"] {
            mgr.ask(&k, q, no_delta(), Arc::new(CancelToken::new())).await.unwrap();
        }
        assert_eq!(mgr.turns(&k), 2);

        let retired = mgr.retire_if_idle(&k).expect("idle session is retired");
        assert_eq!(sink.states().last().map(String::as_str), Some("closed"));
        assert!(mgr.snapshot().is_empty());
        assert_eq!(mgr.turns(&k), 0);
        assert_eq!(mgr.usage(&SessionScope::Utility).input_tokens, 20, "usage kept");
        retired.close().await;

        mgr.ensure(&k, init(&[])).await.unwrap();
        assert_eq!(spawner.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retire_if_idle_skips_session_with_turn_in_flight() {
        let (mgr, _spawner, _sink) = setup();
        let k = key(SessionScope::Utility, Tier::Fast);
        mgr.ensure(&k, init(&[])).await.unwrap();
        let blocked = mgr.ask(&k, "block", no_delta(), Arc::new(CancelToken::new()));
        tokio::pin!(blocked);
        assert!(futures::poll!(blocked.as_mut()).is_pending());
        assert!(mgr.retire_if_idle(&k).is_none());
        assert_eq!(mgr.snapshot().len(), 1);
    }
}
