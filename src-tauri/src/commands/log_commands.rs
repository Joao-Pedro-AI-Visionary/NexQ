//! Diagnostics: the app log file (see `crate::logging`).

use serde::Serialize;
use tauri::{command, AppHandle, Manager};

use crate::logging;

#[derive(Debug, Clone, Serialize)]
pub struct LogInfo {
    /// Folder holding `nexq.log` and its rotated copies.
    pub dir: String,
    /// Full path of the current log file.
    pub file: String,
    pub verbose: bool,
}

/// Start the log file in the OS log folder, with the saved verbosity.
pub fn start(app: &AppHandle) {
    let verbose = app
        .path()
        .app_data_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(dir.join(FRONTEND_STORE)).ok())
        .map_or(false, |text| saved_verbose(&text));
    match app.path().app_log_dir() {
        Ok(dir) => logging::open_dir(dir, verbose),
        Err(e) => log::warn!("No log folder available: {e}"),
    }
    log::info!(
        "System: {} {} ({}), locale {}{}",
        tauri_plugin_os::platform(),
        tauri_plugin_os::version(),
        tauri_plugin_os::arch(),
        tauri_plugin_os::locale().unwrap_or_else(|| "unknown".into()),
        if verbose { ", verbose logging on" } else { "" }
    );
}

/// The frontend config store file (src/stores/configStore.ts `STORE_FILE`).
const FRONTEND_STORE: &str = "config.json";

fn saved_verbose(store_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(store_json)
        .ok()
        .and_then(|v| v.get("verboseLogging").and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

#[command]
pub fn get_log_info() -> Result<LogInfo, String> {
    let dir = logging::dir().ok_or("Logging hasn't started")?;
    Ok(LogInfo {
        dir: dir.display().to_string(),
        file: dir.join(logging::LOG_FILE).display().to_string(),
        verbose: logging::verbose(),
    })
}

/// Verbose logging adds our debug lines and dependencies' info lines.
#[command]
pub fn set_verbose_logging(enabled: bool) {
    logging::set_verbose(enabled);
    log::info!("Verbose logging {}", if enabled { "on" } else { "off" });
}

#[command]
pub fn open_log_folder() -> Result<(), String> {
    let dir = logging::dir().ok_or("Logging hasn't started")?;
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't open {}: {e}", dir.display()))
}

/// The end of the current log, for "Copy recent logs".
#[command]
pub fn get_recent_logs(max_lines: Option<usize>) -> Result<String, String> {
    logging::tail(max_lines.unwrap_or(400).clamp(1, 5000))
}

/// A frontend error or warning, written to the same log file.
#[command]
pub fn write_frontend_log(level: String, source: String, message: String) {
    let level = match level.as_str() {
        "error" => log::Level::Error,
        "warn" => log::Level::Warn,
        "debug" => log::Level::Debug,
        _ => log::Level::Info,
    };
    logging::frontend(level, &source, &message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_saved_verbosity() {
        assert!(saved_verbose(r#"{"verboseLogging": true}"#));
        assert!(!saved_verbose(r#"{"verboseLogging": false}"#));
        assert!(!saved_verbose(r#"{}"#));
        assert!(!saved_verbose("not json"));
    }
}
