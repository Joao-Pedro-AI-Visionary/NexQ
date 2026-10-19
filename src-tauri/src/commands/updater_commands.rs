use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{command, AppHandle, Emitter, ExitRequestApi, Manager, State};
use tauri_plugin_store::StoreExt;
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::AppState;

/// Plugin-store file and keys for the updater preferences ("Automatic updates",
/// "Skip this version"). Written by the frontend (`useUpdater`); read at quit.
const SETTINGS_STORE: &str = "nexq-settings.json";
const AUTO_UPDATE_KEY: &str = "auto_update";
const SKIPPED_VERSION_KEY: &str = "skipped_version";

/// How long Restart waits for the last recording to finish saving.
const RECORDING_WAIT_LIMIT: Duration = Duration::from_secs(120);
/// How long quit (with an update to install) waits for the last recording.
const QUIT_RECORDING_WAIT_LIMIT: Duration = Duration::from_secs(600);

/// Set once quit has been deferred for a recording, so the second exit
/// request goes through even if the wait timed out.
static QUIT_WAITED_FOR_RECORDING: AtomicBool = AtomicBool::new(false);

/// Information about an available update.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub body: Option<String>,
    pub date: Option<String>,
}

impl UpdateInfo {
    fn from_update(update: &Update) -> Self {
        Self {
            version: update.version.clone(),
            body: update.body.clone(),
            date: update.date.map(|d| d.to_string()),
        }
    }
}

/// A downloaded update waiting to be installed (on Restart or on quit).
pub struct PendingUpdate {
    pub update: Update,
    pub bytes: Vec<u8>,
    pub info: UpdateInfo,
}

/// Check for an available update.
///
/// Returns `Some(UpdateInfo)` if a newer version is available, or `None` if the
/// app is already up-to-date.
#[command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app.updater().map_err(|e| format!("Failed to get updater: {}", e))?;

    let update = updater
        .check()
        .await
        .map_err(|e| format!("Failed to check for update: {}", e))?;

    Ok(update.as_ref().map(UpdateInfo::from_update))
}

/// Download the latest update in the background and keep the bytes in
/// `AppState.pending_update` (nothing is installed yet).
///
/// Emits `update_download_progress { chunk_length, content_length }` while
/// downloading, then `update_downloaded { version, body, date }` or
/// `update_download_failed { message }`.
#[command]
pub async fn download_update(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateInfo, String> {
    if state.update_downloading.swap(true, Ordering::SeqCst) {
        return Err("An update download is already in progress".to_string());
    }
    let result = download_into_pending(&app, &state).await;
    state.update_downloading.store(false, Ordering::SeqCst);
    if let Err(message) = &result {
        let _ = app.emit(
            "update_download_failed",
            serde_json::json!({ "message": message }),
        );
    }
    result
}

async fn download_into_pending(app: &AppHandle, state: &AppState) -> Result<UpdateInfo, String> {
    let updater = app.updater().map_err(|e| format!("Failed to get updater: {}", e))?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("Failed to check for update: {}", e))?
        .ok_or_else(|| "No update available".to_string())?;
    let info = UpdateInfo::from_update(&update);

    // Already downloaded this version — nothing to do.
    if let Ok(pending) = state.pending_update.lock() {
        if let Some(p) = pending.as_ref() {
            if p.info.version == info.version {
                return Ok(p.info.clone());
            }
        }
    }

    let progress_app = app.clone();
    let bytes = update
        .download(
            move |chunk_length, content_length| {
                let _ = progress_app.emit(
                    "update_download_progress",
                    serde_json::json!({
                        "chunk_length": chunk_length,
                        "content_length": content_length,
                    }),
                );
            },
            || {},
        )
        .await
        .map_err(|e| format!("Failed to download update: {}", e))?;

    log::info!("Update v{} downloaded ({} bytes)", info.version, bytes.len());

    *state
        .pending_update
        .lock()
        .map_err(|e| format!("Failed to lock pending update: {}", e))? = Some(PendingUpdate {
        update,
        bytes,
        info: info.clone(),
    });

    let _ = app.emit("update_downloaded", &info);
    Ok(info)
}

/// The downloaded update waiting to be installed, if any.
#[command]
pub async fn get_pending_update(state: State<'_, AppState>) -> Result<Option<UpdateInfo>, String> {
    let pending = state
        .pending_update
        .lock()
        .map_err(|e| format!("Failed to lock pending update: {}", e))?;
    Ok(pending.as_ref().map(|p| p.info.clone()))
}

/// Install the downloaded update now. On Windows the NSIS installer is launched
/// in passive mode (`/P /R`: progress only, relaunches NexQ) and this process
/// exits, so on success this never returns.
///
/// Waits (up to 2 min) for post-meeting recording pipelines first, so ending a
/// meeting right before the restart doesn't lose its recording.
/// Refuses while a meeting is active, or if the recording is still saving
/// after the wait.
#[command]
pub async fn install_update_now(state: State<'_, AppState>) -> Result<(), String> {
    if state.meeting_active.load(Ordering::SeqCst) {
        return Err("End the meeting before updating".to_string());
    }
    if !wait_for_recordings(&state, RECORDING_WAIT_LIMIT).await {
        return Err("Still saving the last recording — try again in a moment".to_string());
    }
    install_pending(&state)
}

/// Wait until no post-meeting recording pipeline is running. Returns false on timeout.
async fn wait_for_recordings(state: &AppState, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while state.recording_jobs.load(Ordering::SeqCst) > 0 {
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    true
}

fn install_pending(state: &AppState) -> Result<(), String> {
    let pending = state
        .pending_update
        .lock()
        .map_err(|e| format!("Failed to lock pending update: {}", e))?;
    let p = pending
        .as_ref()
        .ok_or_else(|| "No downloaded update to install".to_string())?;

    log::info!("Installing update v{}...", p.info.version);
    // The plugin's default `on_before_exit` hook runs `cleanup_before_exit`
    // (drops plugin stores, flushing pending saves; clears the tray icon)
    // before the process exits.
    p.update
        .install(&p.bytes)
        .map_err(|e| format!("Failed to install update: {}", e))
}

/// What to do with a downloaded update when the app is asked to exit.
#[derive(Debug, PartialEq, Eq)]
pub enum QuitAction {
    /// Exit normally without installing.
    Exit,
    /// Defer the exit until the last recording has finished saving.
    WaitForRecording,
    /// Install the update (the process exits inside the install).
    Install,
}

/// Installs only when an update is pending, no meeting is running, automatic
/// updates are on and the version wasn't skipped. A recording still being
/// saved defers the quit once; if it is still running after the wait, exit
/// without installing.
pub fn quit_action(
    pending: bool,
    meeting_active: bool,
    auto_update: bool,
    skipped: bool,
    recording_jobs: usize,
    already_waited: bool,
) -> QuitAction {
    if !pending || meeting_active || !auto_update || skipped {
        return QuitAction::Exit;
    }
    if recording_jobs > 0 {
        return if already_waited {
            QuitAction::Exit
        } else {
            QuitAction::WaitForRecording
        };
    }
    QuitAction::Install
}

/// `RunEvent::ExitRequested` handler (tray Quit calls `app.exit(0)`).
pub fn on_exit_requested(app: &AppHandle, api: &ExitRequestApi) {
    let state = app.state::<AppState>();

    let pending_version = state
        .pending_update
        .lock()
        .ok()
        .and_then(|p| p.as_ref().map(|p| p.info.version.clone()));
    let skipped = match &pending_version {
        Some(v) => skipped_version(app).as_deref() == Some(v.as_str()),
        None => false,
    };

    let action = quit_action(
        pending_version.is_some(),
        state.meeting_active.load(Ordering::SeqCst),
        auto_update_enabled(app),
        skipped,
        state.recording_jobs.load(Ordering::SeqCst),
        QUIT_WAITED_FOR_RECORDING.load(Ordering::SeqCst),
    );

    match action {
        QuitAction::Exit => {}
        QuitAction::Install => {
            if let Err(e) = install_pending(&state) {
                log::error!("Install on quit failed: {}", e);
            }
        }
        QuitAction::WaitForRecording => {
            log::info!("Quit deferred until the last recording finishes saving");
            QUIT_WAITED_FOR_RECORDING.store(true, Ordering::SeqCst);
            api.prevent_exit();
            for (_, window) in app.webview_windows() {
                let _ = window.hide();
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = app.state::<AppState>();
                if !wait_for_recordings(&state, QUIT_RECORDING_WAIT_LIMIT).await {
                    log::warn!("Recording still saving after the quit wait; exiting without installing");
                }
                app.exit(0);
            });
        }
    }
}

fn setting(app: &AppHandle, key: &str) -> Option<serde_json::Value> {
    app.store(SETTINGS_STORE).ok().and_then(|store| store.get(key))
}

/// "Automatic updates" preference; defaults to on when unset or unreadable.
fn auto_update_enabled(app: &AppHandle) -> bool {
    setting(app, AUTO_UPDATE_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

fn skipped_version(app: &AppHandle) -> Option<String> {
    setting(app, SKIPPED_VERSION_KEY).and_then(|v| v.as_str().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::{quit_action, QuitAction};

    #[test]
    fn installs_when_everything_allows_it() {
        assert_eq!(quit_action(true, false, true, false, 0, false), QuitAction::Install);
    }

    #[test]
    fn exits_without_pending_update() {
        assert_eq!(quit_action(false, false, true, false, 0, false), QuitAction::Exit);
    }

    #[test]
    fn never_installs_during_a_meeting() {
        assert_eq!(quit_action(true, true, true, false, 0, false), QuitAction::Exit);
    }

    #[test]
    fn respects_automatic_updates_off() {
        assert_eq!(quit_action(true, false, false, false, 0, false), QuitAction::Exit);
    }

    #[test]
    fn skipped_version_is_not_installed() {
        assert_eq!(quit_action(true, false, true, true, 0, false), QuitAction::Exit);
    }

    #[test]
    fn waits_for_a_recording_still_saving() {
        assert_eq!(
            quit_action(true, false, true, false, 1, false),
            QuitAction::WaitForRecording
        );
    }

    #[test]
    fn exits_without_installing_if_recording_outlasts_the_wait() {
        assert_eq!(quit_action(true, false, true, false, 1, true), QuitAction::Exit);
    }

    #[test]
    fn installs_after_the_wait_once_recording_is_saved() {
        assert_eq!(quit_action(true, false, true, false, 0, true), QuitAction::Install);
    }
}
