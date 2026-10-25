//! Application log: a rotating file in the OS log folder, plus stderr.
//!
//! Callers never block: `log` macros format the line and hand it to a writer
//! thread (which redacts and writes it) over a bounded channel (lines are dropped, and counted, if it ever
//! fills). Audio callbacks and STT loops can log without stalling.
//!
//! - Windows: `%LOCALAPPDATA%\com.nexq.app\logs\nexq.log`
//! - macOS: `~/Library/Logs/com.nexq.app/nexq.log`
//!
//! The file rotates at `MAX_FILE_BYTES`, keeping `KEPT_FILES` older files, so
//! logs never take more than ~20 MB. Lines are scrubbed of anything that looks
//! like an API key or bearer token before they are written.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::time::Duration;

use log::{Level, LevelFilter, Log, Metadata, Record};

pub const LOG_FILE: &str = "nexq.log";
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const KEPT_FILES: usize = 4;
const CHANNEL_CAPACITY: usize = 8192;
/// Our own crate's targets; dependencies only log warnings and errors.
const APP_TARGET: &str = "nexq";

static SENDER: OnceLock<SyncSender<Msg>> = OnceLock::new();
static DIR: OnceLock<PathBuf> = OnceLock::new();
static VERBOSE: AtomicBool = AtomicBool::new(false);
static DROPPED: AtomicU64 = AtomicU64::new(0);

enum Msg {
    Line { text: String, urgent: bool },
    /// The log folder is known (after Tauri's path resolver is available).
    Open(PathBuf),
    /// Flush everything written so far, then acknowledge.
    Flush(mpsc::Sender<()>),
}

struct Logger;

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        enabled(metadata.target(), metadata.level(), VERBOSE.load(Ordering::Relaxed))
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let text = format_line(
            &chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            record.level(),
            record.target(),
            &record.args().to_string(),
        );
        send_line(text, record.level() == Level::Error);
    }

    fn flush(&self) {
        flush(Duration::from_secs(2));
    }
}

fn enabled(target: &str, level: Level, verbose: bool) -> bool {
    let ours = target == APP_TARGET || target.starts_with("nexq_lib") || target.starts_with("nexq::");
    let max = match (ours, verbose) {
        (true, true) => Level::Debug,
        (true, false) => Level::Info,
        // Dependencies: their warnings only, or info when diagnosing.
        (false, true) => Level::Info,
        (false, false) => Level::Warn,
    };
    level <= max
}

fn format_line(time: &str, level: Level, target: &str, message: &str) -> String {
    let target = target.strip_prefix("nexq_lib::").unwrap_or(target);
    format!("{time} {level:<5} [{target}] {message}\n")
}

fn send_line(text: String, urgent: bool) {
    let Some(tx) = SENDER.get() else { return };
    match tx.try_send(Msg::Line { text, urgent }) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

/// Install the logger. Call once, first thing in `run()`. Lines logged before
/// [`open_dir`] are kept in memory and written once the folder is known.
pub fn init() {
    let (tx, rx) = mpsc::sync_channel(CHANNEL_CAPACITY);
    if SENDER.set(tx).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("log-writer".into())
        .spawn(move || writer_loop(rx));
    if spawned.is_err() || log::set_logger(&Logger).is_err() {
        return;
    }
    // The logger filters by target itself; this is the ceiling for all of them.
    log::set_max_level(LevelFilter::Debug);
    install_panic_hook();
}

/// Start writing to `dir/nexq.log` (Tauri's `app_log_dir`).
pub fn open_dir(dir: PathBuf, verbose: bool) {
    set_verbose(verbose);
    let _ = DIR.set(dir.clone());
    if let Some(tx) = SENDER.get() {
        let _ = tx.send(Msg::Open(dir));
    }
}

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

pub fn dir() -> Option<&'static Path> {
    DIR.get().map(PathBuf::as_path)
}

/// Write everything queued so far to disk, waiting up to `timeout`.
pub fn flush(timeout: Duration) {
    let Some(tx) = SENDER.get() else { return };
    let (ack_tx, ack_rx) = mpsc::channel();
    if tx.send(Msg::Flush(ack_tx)).is_ok() {
        let _ = ack_rx.recv_timeout(timeout);
    }
}

/// A line from the frontend (console errors, failed actions).
pub fn frontend(level: Level, source: &str, message: &str) {
    let target = format!("ui:{}", source.chars().take(40).collect::<String>());
    if !enabled(APP_TARGET, level, verbose()) {
        return;
    }
    let message: String = message.chars().take(4000).collect();
    let text = format_line(
        &chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        level,
        &target,
        &message,
    );
    send_line(text, level == Level::Error);
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        let location = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        log::error!("Panic in thread '{name}'{location}: {payload}");
        flush(Duration::from_millis(500));
        previous(info);
    }));
}

// ── Writer thread ───────────────────────────────────────────────────────

struct Sink {
    path: PathBuf,
    file: BufWriter<File>,
    size: u64,
}

impl Sink {
    fn open(dir: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(LOG_FILE);
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { path, file: BufWriter::new(file), size })
    }

    fn write(&mut self, text: &str) {
        if self.size + text.len() as u64 > MAX_FILE_BYTES && self.size > 0 {
            self.rotate();
        }
        if self.file.write_all(text.as_bytes()).is_ok() {
            self.size += text.len() as u64;
        }
    }

    fn rotate(&mut self) {
        let _ = self.file.flush();
        rotate_files(&self.path, KEPT_FILES);
        if let Ok(file) = OpenOptions::new().create(true).write(true).truncate(true).open(&self.path) {
            self.file = BufWriter::new(file);
            self.size = 0;
        }
    }
}

/// `nexq.log` → `nexq.1.log` → … → `nexq.{kept}.log` (the oldest is removed).
fn rotate_files(path: &Path, kept: usize) {
    let numbered = |n: usize| path.with_extension(format!("{n}.log"));
    let _ = fs::remove_file(numbered(kept));
    for n in (1..kept).rev() {
        let _ = fs::rename(numbered(n), numbered(n + 1));
    }
    let _ = fs::rename(path, numbered(1));
}

fn writer_loop(rx: Receiver<Msg>) {
    let mut sink: Option<Sink> = None;
    // Lines logged before the folder is known (capped).
    let mut early: Vec<String> = Vec::new();
    let mut dirty = false;
    let mut reported_drops = 0u64;
    loop {
        let msg = match rx.recv_timeout(Duration::from_millis(1000)) {
            Ok(msg) => Some(msg),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match msg {
            Some(Msg::Line { text, urgent }) => {
                // Redacted here, off the logging thread.
                let text = redact(&text);
                let _ = std::io::stderr().write_all(text.as_bytes());
                match sink.as_mut() {
                    Some(s) => {
                        s.write(&text);
                        dirty = true;
                        if urgent {
                            let _ = s.file.flush();
                            dirty = false;
                        }
                    }
                    None if early.len() < 2000 => early.push(text),
                    None => {}
                }
            }
            Some(Msg::Open(dir)) => match Sink::open(&dir) {
                Ok(mut s) => {
                    s.write(&session_header());
                    for line in early.drain(..) {
                        s.write(&line);
                    }
                    let _ = s.file.flush();
                    sink = Some(s);
                }
                Err(e) => eprintln!("NexQ: can't open the log file in {}: {e}", dir.display()),
            },
            Some(Msg::Flush(ack)) => {
                if let Some(s) = sink.as_mut() {
                    let _ = s.file.flush();
                }
                dirty = false;
                let _ = ack.send(());
            }
            // Idle: write out what's buffered so the file is current.
            None => {
                let dropped = DROPPED.load(Ordering::Relaxed);
                if let (Some(s), true) = (sink.as_mut(), dropped > reported_drops) {
                    s.write(&format!(
                        "{} WARN  [logging] {} log lines were dropped (logging faster than the disk)\n",
                        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                        dropped - reported_drops
                    ));
                    reported_drops = dropped;
                    dirty = true;
                }
                if dirty {
                    if let Some(s) = sink.as_mut() {
                        let _ = s.file.flush();
                    }
                    dirty = false;
                }
            }
        }
    }
    if let Some(mut s) = sink {
        let _ = s.file.flush();
    }
}

fn session_header() -> String {
    format!(
        "\n===== NexQ v{} started {} | {} {} =====\n",
        env!("CARGO_PKG_VERSION"),
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S %:z"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

/// The last `max_lines` lines of the current log file.
pub fn tail(max_lines: usize) -> Result<String, String> {
    let dir = dir().ok_or("Logging hasn't started")?;
    flush(Duration::from_secs(2));
    let text = fs::read_to_string(dir.join(LOG_FILE)).map_err(|e| e.to_string())?;
    Ok(last_lines(&text, max_lines).to_string())
}

fn last_lines(text: &str, max_lines: usize) -> &str {
    let trimmed = text.trim_end_matches('\n');
    let mut start = trimmed.len();
    for _ in 0..max_lines {
        match trimmed[..start].rfind('\n') {
            Some(i) => start = i,
            None => return trimmed,
        }
    }
    &trimmed[start + 1..]
}

// ── Redaction ───────────────────────────────────────────────────────────

/// Prefixes of provider API keys (OpenAI/Anthropic `sk-`, Groq `gsk_`,
/// Google `AIza`, OpenRouter `sk-or-`, Deepgram/xAI style `xai-`).
const KEY_PREFIXES: &[&str] = &["sk-", "gsk_", "AIza", "xai-"];
/// Names whose value is a secret: `name=value`, `name: value`, `"name":"value"`.
const SECRET_NAMES: &[&str] = &["api_key", "apikey", "api-key", "access_token", "token", "authorization", "password", "secret", "key"];
const REDACTED: &str = "[redacted]";

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~' | '+' | '/' | '=')
}

/// Mask anything that looks like a credential.
pub fn redact(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let chars: Vec<char> = message.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if let Some(skip) = secret_at(&chars, i, &mut out) {
            i = skip;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// If a secret starts at `i`, write its masked form to `out` and return the index after it.
fn secret_at(chars: &[char], i: usize, out: &mut String) -> Option<usize> {
    let at_word_start = i == 0 || !is_token_char(chars[i - 1]);
    if !at_word_start || !chars[i].is_ascii_alphabetic() {
        return None;
    }
    let rest: String = chars[i..chars.len().min(i + 16)].iter().collect();
    let lower = rest.to_ascii_lowercase();

    // `Bearer <token>` / `Basic <token>`
    for scheme in ["bearer ", "basic "] {
        if lower.starts_with(scheme) {
            let start = i + scheme.len();
            let end = token_end(chars, start);
            if end - start >= 8 {
                out.extend(&chars[i..start]);
                out.push_str(REDACTED);
                return Some(end);
            }
        }
    }

    // Known key formats.
    for prefix in KEY_PREFIXES {
        if rest.starts_with(prefix) {
            let end = token_end(chars, i);
            if end - i >= 20 {
                out.push_str(REDACTED);
                return Some(end);
            }
        }
    }

    // `name=value` / `name: value` / `"name": "value"`
    for name in SECRET_NAMES {
        if !lower.starts_with(name) {
            continue;
        }
        let mut j = i + name.len();
        if j < chars.len() && (is_token_char(chars[j]) && chars[j] != '=') {
            continue; // part of a longer word (e.g. "keyboard")
        }
        while j < chars.len() && matches!(chars[j], '"' | '\'') {
            j += 1;
        }
        while j < chars.len() && chars[j] == ' ' {
            j += 1;
        }
        if j >= chars.len() || !matches!(chars[j], '=' | ':') {
            continue;
        }
        j += 1;
        while j < chars.len() && matches!(chars[j], ' ' | '"' | '\'') {
            j += 1;
        }
        let end = token_end(chars, j);
        // Short or descriptive values ("present", "MISSING") aren't secrets.
        if end - j >= 12 {
            out.extend(&chars[i..j]);
            out.push_str(REDACTED);
            return Some(end);
        }
    }
    None
}

fn token_end(chars: &[char], start: usize) -> usize {
    let mut end = start;
    while end < chars.len() && is_token_char(chars[end]) {
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_keys_and_tokens() {
        assert_eq!(redact("using key sk-proj-abcdefghijklmnop1234567890"), "using key [redacted]");
        assert_eq!(redact("Authorization: Bearer abc.def-ghi_jkl123"), "Authorization: Bearer [redacted]");
        assert_eq!(redact("url?key=AIzaSyA1234567890abcdefghij&x=1"), "url?key=[redacted]&x=1");
        assert_eq!(redact(r#"{"api_key": "gsk_0123456789abcdefghijkl"}"#), r#"{"api_key": "[redacted]"}"#);
        assert_eq!(redact("token=abcdefghijklmnopqrstuvwx"), "token=[redacted]");
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        for s in [
            "Deepgram key for 'you': present",
            "Groq key for 'them': MISSING",
            "keyboard shortcut CmdOrCtrl+Shift+H registered",
            "risk-free sk-short",
            "Claude session context full (tokens: 190000)",
            "System audio loopback ACTIVE on 'Speakers (Realtek)'",
        ] {
            assert_eq!(redact(s), s);
        }
    }

    #[test]
    fn filters_dependencies_harder_than_our_code() {
        assert!(enabled("nexq_lib::audio", Level::Info, false));
        assert!(!enabled("nexq_lib::audio", Level::Debug, false));
        assert!(enabled("nexq_lib::audio", Level::Debug, true));
        assert!(!enabled("reqwest::connect", Level::Info, false));
        assert!(enabled("reqwest::connect", Level::Warn, false));
        assert!(enabled("reqwest::connect", Level::Info, true));
        assert!(!enabled("reqwest::connect", Level::Debug, true));
    }

    #[test]
    fn formats_one_line_per_record() {
        assert_eq!(
            format_line("2026-10-24 10:00:00.000", Level::Warn, "nexq_lib::stt::deepgram", "closed"),
            "2026-10-24 10:00:00.000 WARN  [stt::deepgram] closed\n"
        );
    }

    #[test]
    fn rotates_and_keeps_a_fixed_number_of_files() {
        let dir = std::env::temp_dir().join(format!("nexq-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(LOG_FILE);
        for n in 0..6 {
            fs::write(&path, format!("gen {n}")).unwrap();
            rotate_files(&path, 3);
        }
        assert!(!path.exists());
        assert_eq!(fs::read_to_string(dir.join("nexq.1.log")).unwrap(), "gen 5");
        assert_eq!(fs::read_to_string(dir.join("nexq.3.log")).unwrap(), "gen 3");
        assert!(!dir.join("nexq.4.log").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The only test that installs the global logger.
    #[test]
    fn writes_redacted_lines_to_the_file_after_it_opens() {
        let dir = std::env::temp_dir().join(format!("nexq-log-e2e-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        init();
        log::info!("before the folder is known");
        open_dir(dir.clone(), false);
        log::info!("key sk-ant-api03-abcdefghijklmnopqrstuvwxyz");
        log::debug!("hidden unless verbose");
        frontend(Level::Error, "overlay", "Unhandled rejection: boom");
        let text = tail(50).unwrap();
        assert!(text.contains(&format!("===== NexQ v{}", env!("CARGO_PKG_VERSION"))));
        assert!(text.contains("before the folder is known"));
        assert!(text.contains("key [redacted]"));
        assert!(!text.contains("abcdefghijklmnop"));
        assert!(!text.contains("hidden unless verbose"));
        assert!(text.contains("ERROR [ui:overlay] Unhandled rejection: boom"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_returns_the_last_lines() {
        assert_eq!(last_lines("a\nb\nc\n", 2), "b\nc");
        assert_eq!(last_lines("a\nb\n", 5), "a\nb");
        assert_eq!(last_lines("", 3), "");
    }
}
