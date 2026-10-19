use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{command, AppHandle, Emitter, Manager, State};
use tauri_plugin_store::StoreExt;
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::AppState;

/// Plugin-store file and keys for the updater preferences ("Automatic updates",
/// "Skip this version"). Written by the frontend (`useUpdater`); read at quit.
const SETTINGS_STORE: &str = "nexq-settings.json";
const AUTO_UPDATE_KEY: &str = "auto_update";
const SKIPPED_VERSION_KEY: &str = "skipped_version";

const RECORDING_WAIT_LIMIT: Duration = Duration::from_secs(120);

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
/// downloading and `update_downloaded { version, body, date }` when done.
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
#[command]
pub async fn install_update_now(state: State<'_, AppState>) -> Result<(), String> {
    let deadline = Instant::now() + RECORDING_WAIT_LIMIT;
    while state.recording_jobs.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    install_pending(&state)
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

/// Called when the app is about to exit. Installs a downloaded update when
/// automatic updates are on, the version wasn't skipped, and no meeting is
/// running. On success the process exits inside this call; otherwise the
/// normal exit continues.
pub fn install_on_quit(app: &AppHandle) {
    let state = app.state::<AppState>();

    let pending_version = state
        .pending_update
        .lock()
        .ok()
        .and_then(|p| p.as_ref().map(|p| p.info.version.clone()));
    let Some(pending_version) = pending_version else {
        return;
    };
    if state.meeting_active.load(Ordering::SeqCst) {
        log::info!("Update pending but a meeting is active; not installing on quit");
        return;
    }
    if !auto_update_enabled(app) {
        log::info!("Update pending but automatic updates are off; not installing on quit");
        return;
    }
    if skipped_version(app).as_deref() == Some(pending_version.as_str()) {
        log::info!("Update v{} was skipped; not installing on quit", pending_version);
        return;
    }

    if let Err(e) = install_pending(&state) {
        log::error!("Install on quit failed: {}", e);
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
