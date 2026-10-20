//! `ClaudeSession`: one persistent, hidden `claude.exe` process speaking
//! `stream-json`, with turns serialized FIFO.
//!
//! Three dedicated threads own the pipes (same shape as `stt/sherpa_sidecar.rs`):
//! a stdin writer fed by a channel, a stdout reader that parses JSONL into a
//! tokio channel of [`AgentEvent`]s, and a stderr reader that logs lines (and
//! keeps a short tail for crash reasons).
//!
//! The turn lock is a fair (FIFO) `tokio::sync::Mutex` around the event
//! receiver: whoever holds it owns the current turn and its events. Each turn
//! runs in a spawned driver task that holds the lock until the turn's `result`
//! arrives, so a cancelled or dropped `ask` never leaves stale events behind for
//! the next turn.

use super::command::{build_args, configure_command, sandbox_dir, SessionConfig};
use super::protocol::{
    classify_error, parse_line, user_message, AgentErrorKind, AgentEvent, TurnResult,
};
use crate::llm::provider::CancelToken;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex as AsyncMutex, OwnedMutexGuard};

const INIT_TIMEOUT: Duration = Duration::from_secs(20);
/// `shouldQuery:false` turns make no model call and answer at once; this only
/// guards against a hung process.
const APPEND_TIMEOUT: Duration = Duration::from_secs(30);
const CLOSE_GRACE: Duration = Duration::from_secs(2);
const STDERR_TAIL_LINES: usize = 20;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AgentError {
    /// The process could not be started (binary missing, bad path, ...).
    #[error("could not start Claude: {0}")]
    Spawn(String),
    /// The process exited (or its pipes broke) while it was needed.
    #[error("Claude session crashed: {0}")]
    Crashed(String),
    #[error("Claude did not respond in time")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    /// A `result` with `is_error: true`, classified from its text.
    #[error("{1}")]
    Cli(AgentErrorKind, String),
}

impl AgentError {
    /// Coarse kind for status reasons and fallback decisions. A missing binary
    /// (`io::ErrorKind::NotFound` at spawn) is always `NotInstalled`; crash text
    /// that names a known cause (e.g. "not logged in" on stderr) keeps it.
    pub fn kind(&self) -> AgentErrorKind {
        match self {
            AgentError::Spawn(msg) => classify_error(msg),
            AgentError::Crashed(msg) => match classify_error(msg) {
                AgentErrorKind::Other => AgentErrorKind::Crashed,
                kind => kind,
            },
            AgentError::Timeout | AgentError::Cancelled => AgentErrorKind::Other,
            AgentError::Cli(kind, _) => *kind,
        }
    }
}

/// State shared with the pipe threads and turn drivers.
struct Shared {
    child: Mutex<Option<Child>>,
    alive: AtomicBool,
    /// Set by `close`/drop so in-flight turns report a deliberate shutdown.
    closed: AtomicBool,
    stderr_tail: Mutex<VecDeque<String>>,
    stderr_done: AtomicBool,
    exit_status: Mutex<Option<String>>,
}

impl Shared {
    /// Readable reason for an unexpected exit: exit status + last stderr lines.
    fn crash_reason(&self) -> String {
        if self.closed.load(Ordering::SeqCst) {
            return "session closed".to_string();
        }
        let status = self
            .exit_status
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "pipe closed".to_string());
        let tail: Vec<String> = self.stderr_tail.lock().unwrap().iter().cloned().collect();
        if tail.is_empty() {
            format!("claude exited unexpectedly ({status})")
        } else {
            format!(
                "claude exited unexpectedly ({status}): {}",
                tail.join(" | ")
            )
        }
    }
}

enum TurnMsg {
    Delta(String),
    Done(Result<TurnResult, AgentError>),
}

pub struct ClaudeSession {
    model: String,
    shared: Arc<Shared>,
    /// `None` once closed; dropping the sender closes the child's stdin.
    stdin_tx: Mutex<Option<std_mpsc::Sender<String>>>,
    /// The FIFO turn lock; the holder owns the current turn's events.
    turn: Arc<AsyncMutex<mpsc::UnboundedReceiver<AgentEvent>>>,
    prompt_file: PathBuf,
}

impl ClaudeSession {
    /// Start `bin` with the exact launch args, the system prompt written to a
    /// unique temp file (deleted on close/drop), and wait up to 20 s for
    /// `system/init`. The caller resolves `bin` (detection is synchronous).
    ///
    /// The CLI prints `system/init` only after it reads its first stdin message
    /// (and again at the start of every turn), so the session is primed with an
    /// empty `shouldQuery:false` message: no model call, just init + an empty
    /// `result`.
    pub async fn spawn(bin: PathBuf, cfg: SessionConfig) -> Result<ClaudeSession, AgentError> {
        let prompt_file =
            std::env::temp_dir().join(format!("nexq-claude-sp-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&prompt_file, &cfg.system_prompt).map_err(|e| {
            AgentError::Spawn(format!(
                "cannot write system prompt file {}: {e}",
                prompt_file.display()
            ))
        })?;

        let args = build_args(&cfg, &prompt_file);
        let started = tokio::task::spawn_blocking(move || start_process(&bin, &args))
            .await
            .map_err(|e| AgentError::Spawn(format!("spawn task failed: {e}")))
            .and_then(|r| r);
        let (shared, stdin_tx, events) = match started {
            Ok(parts) => parts,
            Err(e) => {
                let _ = std::fs::remove_file(&prompt_file);
                return Err(e);
            }
        };

        let session = ClaudeSession {
            model: cfg.model,
            shared,
            stdin_tx: Mutex::new(Some(stdin_tx)),
            turn: Arc::new(AsyncMutex::new(events)),
            prompt_file,
        };

        let init = tokio::time::timeout(INIT_TIMEOUT, session.prime())
            .await
            .unwrap_or(Err(AgentError::Timeout));

        match init {
            Ok(()) => Ok(session),
            Err(e) => {
                session.close().await;
                Err(e)
            }
        }
    }

    /// Append `text` to the session context without a model call
    /// (`shouldQuery:false`) and wait for its empty `result`.
    pub async fn append_context(&self, text: &str) -> Result<(), AgentError> {
        let mut rx = self.start_turn(text, false, None).await?;
        let wait = async {
            loop {
                match rx.recv().await {
                    Some(TurnMsg::Delta(_)) => {}
                    Some(TurnMsg::Done(r)) => return r,
                    None => return Err(AgentError::Crashed(self.shared.crash_reason())),
                }
            }
        };
        let result = tokio::time::timeout(APPEND_TIMEOUT, wait)
            .await
            .unwrap_or(Err(AgentError::Timeout));
        into_outcome(result).map(|_| ())
    }

    /// Ask one question; text deltas go to `on_delta` as they stream. Turns are
    /// FIFO per session. When `cancel` fires, deltas stop at once and this
    /// returns `Err(Cancelled)`; the turn is drained in the background until its
    /// `result`, and only then may the next turn start.
    pub async fn ask(
        &self,
        prompt: &str,
        on_delta: Box<dyn Fn(&str) + Send + Sync>,
        cancel: Arc<CancelToken>,
    ) -> Result<TurnResult, AgentError> {
        let mut rx = self.start_turn(prompt, true, Some(&cancel)).await?;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(AgentError::Cancelled),
                msg = rx.recv() => match msg {
                    Some(TurnMsg::Delta(text)) => on_delta(&text),
                    Some(TurnMsg::Done(result)) => return into_outcome(result),
                    None => return Err(AgentError::Crashed(self.shared.crash_reason())),
                },
            }
        }
    }

    pub fn is_alive(&self) -> bool {
        if !self.shared.alive.load(Ordering::SeqCst) {
            return false;
        }
        match self.shared.child.lock().unwrap().as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Close stdin, give the process 2 s to exit, then kill it. Idempotent.
    pub async fn close(&self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        self.stdin_tx.lock().unwrap().take();
        let deadline = Instant::now() + CLOSE_GRACE;
        loop {
            let exited = match self.shared.child.lock().unwrap().as_mut() {
                Some(child) => !matches!(child.try_wait(), Ok(None)),
                None => true,
            };
            if exited || Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if let Some(mut child) = self.shared.child.lock().unwrap().take() {
            if matches!(child.try_wait(), Ok(None)) {
                log::warn!("claude session did not exit within 2 s of stdin close; killing");
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        self.shared.alive.store(false, Ordering::SeqCst);
        remove_prompt_file(&self.prompt_file);
    }

    /// The model this session was started with (as configured, e.g. `haiku`).
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Send the priming message and wait for `system/init` and its `result`.
    async fn prime(&self) -> Result<(), AgentError> {
        let mut events = self.turn.lock().await;
        self.send_line(user_message("", false))?;
        let mut init_seen = false;
        loop {
            match events.recv().await {
                Some(AgentEvent::Init { session_id, model }) => {
                    log::info!("claude session {session_id} ready (model {model})");
                    init_seen = true;
                }
                Some(AgentEvent::TurnComplete(result)) if init_seen => {
                    return into_outcome(Ok(result)).map(|_| ());
                }
                Some(AgentEvent::TurnComplete(_)) => {
                    return Err(AgentError::Spawn(
                        "claude sent a result before system/init".into(),
                    ));
                }
                Some(_) => {}
                None => return Err(AgentError::Crashed(self.shared.crash_reason())),
            }
        }
    }

    fn send_line(&self, line: String) -> Result<(), AgentError> {
        let sent = match self.stdin_tx.lock().unwrap().as_ref() {
            Some(tx) => tx.send(line).is_ok(),
            None => false,
        };
        if sent {
            Ok(())
        } else {
            Err(AgentError::Crashed(self.shared.crash_reason()))
        }
    }

    /// Take the turn lock (FIFO), send one user message and spawn the driver
    /// that forwards this turn's events until its `result`, holding the lock.
    async fn start_turn(
        &self,
        text: &str,
        query: bool,
        cancel: Option<&CancelToken>,
    ) -> Result<mpsc::UnboundedReceiver<TurnMsg>, AgentError> {
        let guard = match cancel {
            Some(token) => tokio::select! {
                biased;
                _ = token.cancelled() => return Err(AgentError::Cancelled),
                guard = self.turn.clone().lock_owned() => guard,
            },
            None => self.turn.clone().lock_owned().await,
        };
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(AgentError::Cancelled);
        }
        if !self.is_alive() {
            return Err(AgentError::Crashed(self.shared.crash_reason()));
        }
        self.send_line(user_message(text, query))?;
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(drive_turn(guard, tx, self.shared.clone()));
        Ok(rx)
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        self.stdin_tx.lock().unwrap().take();
        if let Some(mut child) = self.shared.child.lock().unwrap().take() {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
        }
        self.shared.alive.store(false, Ordering::SeqCst);
        remove_prompt_file(&self.prompt_file);
    }
}

/// Forward one turn's deltas until its `result` (or process exit), then
/// release the turn lock. Keeps draining after the receiver is gone.
async fn drive_turn(
    mut events: OwnedMutexGuard<mpsc::UnboundedReceiver<AgentEvent>>,
    tx: mpsc::UnboundedSender<TurnMsg>,
    shared: Arc<Shared>,
) {
    let outcome = loop {
        match events.recv().await {
            Some(AgentEvent::TextDelta(text)) => {
                let _ = tx.send(TurnMsg::Delta(text));
            }
            Some(AgentEvent::TurnComplete(result)) => break Ok(result),
            Some(_) => {}
            None => break Err(AgentError::Crashed(shared.crash_reason())),
        }
    };
    let _ = tx.send(TurnMsg::Done(outcome));
}

fn into_outcome(result: Result<TurnResult, AgentError>) -> Result<TurnResult, AgentError> {
    let result = result?;
    if result.is_error {
        Err(AgentError::Cli(classify_error(&result.text), result.text))
    } else {
        Ok(result)
    }
}

fn remove_prompt_file(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!("cannot remove system prompt file {}: {e}", path.display());
        }
    }
}

type Started = (
    Arc<Shared>,
    std_mpsc::Sender<String>,
    mpsc::UnboundedReceiver<AgentEvent>,
);

/// Spawn the child (no shell, no window, sandbox cwd, stripped env) and its
/// three pipe threads. Blocking; run on a blocking thread.
fn start_process(bin: &Path, args: &[String]) -> Result<Started, AgentError> {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    configure_command(&mut cmd, &sandbox_dir());
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AgentError::Spawn(format!("Claude CLI not installed: {} ({e})", bin.display()))
        } else {
            AgentError::Spawn(format!("failed to start {}: {e}", bin.display()))
        }
    })?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let shared = Arc::new(Shared {
        child: Mutex::new(Some(child)),
        alive: AtomicBool::new(true),
        closed: AtomicBool::new(false),
        stderr_tail: Mutex::new(VecDeque::new()),
        stderr_done: AtomicBool::new(false),
        exit_status: Mutex::new(None),
    });
    let (stdin_tx, stdin_rx) = std_mpsc::channel::<String>();
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    let spawn_thread = |name: &str, f: Box<dyn FnOnce() + Send>| {
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(f)
            .map_err(|e| AgentError::Spawn(format!("cannot start {name} thread: {e}")))
    };

    spawn_thread(
        "claude-stdin",
        Box::new(move || {
            let mut stdin = stdin;
            for line in stdin_rx {
                let written = stdin
                    .write_all(line.as_bytes())
                    .and_then(|_| stdin.write_all(b"\n"))
                    .and_then(|_| stdin.flush());
                if let Err(e) = written {
                    log::warn!("claude stdin write failed: {e}");
                    break;
                }
            }
            // Dropping `stdin` here closes the pipe, which ends the process.
        }),
    )?;

    let err_shared = shared.clone();
    spawn_thread(
        "claude-stderr",
        Box::new(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                let line = line.trim_end().to_string();
                if line.is_empty() {
                    continue;
                }
                log::debug!("claude stderr: {line}");
                let mut tail = err_shared.stderr_tail.lock().unwrap();
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            err_shared.stderr_done.store(true, Ordering::SeqCst);
        }),
    )?;

    let out_shared = shared.clone();
    spawn_thread(
        "claude-stdout",
        Box::new(move || {
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(e) => {
                        log::warn!("claude stdout read failed: {e}");
                        break;
                    }
                };
                match parse_line(&line) {
                    Ok(AgentEvent::Other) => {}
                    Ok(event) => {
                        let _ = event_tx.send(event);
                    }
                    Err(e) => log::warn!("claude stdout: skipping unparseable line ({e})"),
                }
            }
            record_exit(&out_shared);
            out_shared.alive.store(false, Ordering::SeqCst);
            // `event_tx` drops here: pending turns see the channel close.
        }),
    )?;

    Ok((shared, stdin_tx, event_rx))
}

/// After stdout EOF: record the exit status and let stderr finish (both
/// bounded, so a lingering grandchild holding a pipe cannot hang us).
fn record_exit(shared: &Shared) {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let status = match shared.child.lock().unwrap().as_mut() {
            Some(child) => child.try_wait().ok().flatten(),
            None => break,
        };
        if let Some(status) = status {
            *shared.exit_status.lock().unwrap() = Some(match status.code() {
                Some(code) => format!("exit code {code}"),
                None => "terminated".to_string(),
            });
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    while !shared.stderr_done.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_maps_spawn_crash_and_cli_errors() {
        assert_eq!(
            AgentError::Spawn("Claude CLI not installed: C:\\x\\claude.exe".into()).kind(),
            AgentErrorKind::NotInstalled
        );
        assert_eq!(
            AgentError::Crashed("claude exited unexpectedly (exit code 1)".into()).kind(),
            AgentErrorKind::Crashed
        );
        assert_eq!(
            AgentError::Crashed("claude exited unexpectedly (exit code 1): Not logged in".into())
                .kind(),
            AgentErrorKind::NotLoggedIn
        );
        assert_eq!(
            AgentError::Cli(AgentErrorKind::RateLimited, "x".into()).kind(),
            AgentErrorKind::RateLimited
        );
    }

    #[test]
    fn error_result_becomes_classified_cli_error() {
        let r = TurnResult {
            text: "Not logged in · Please run /login".into(),
            is_error: true,
            ..Default::default()
        };
        assert_eq!(
            into_outcome(Ok(r)),
            Err(AgentError::Cli(
                AgentErrorKind::NotLoggedIn,
                "Not logged in · Please run /login".into()
            ))
        );
    }
}
