use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{command, AppHandle, Manager, Runtime, Window};

use crate::state::AppState;

/// Stealth mode ("Hide from screen share"): every NexQ window stays visible on
/// the user's own display but is left out of screen sharing, recordings and
/// screenshots.
///
/// - Windows: `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`
///   (Windows 10 version 2004 and later).
/// - macOS: `NSWindow.sharingType = NSWindowSharingNone`. Apps that capture
///   through ScreenCaptureKit on macOS 14+ may still see the window.
static STEALTH: AtomicBool = AtomicBool::new(false);

/// Whether stealth mode is on (read when windows are created and menus built).
pub fn stealth_enabled() -> bool {
    STEALTH.load(Ordering::SeqCst)
}

/// What this platform offers, for the Settings note.
#[derive(Debug, Clone, Serialize)]
pub struct StealthSupport {
    pub supported: bool,
    /// Caveat to show next to the toggle, if any.
    pub note: Option<String>,
}

fn support() -> StealthSupport {
    if cfg!(target_os = "windows") {
        StealthSupport {
            supported: true,
            note: Some("Requires Windows 10 version 2004 or later.".to_string()),
        }
    } else if cfg!(target_os = "macos") {
        StealthSupport {
            supported: true,
            note: Some(
                "Some recorders on macOS 14 and later (ScreenCaptureKit) can still capture windows."
                    .to_string(),
            ),
        }
    } else {
        StealthSupport {
            supported: false,
            note: Some("Not available on this platform.".to_string()),
        }
    }
}

#[command]
pub fn get_stealth_support() -> StealthSupport {
    support()
}

/// Serializes stealth transitions (and new windows picking up the setting).
static TRANSITION: Mutex<()> = Mutex::new(());

/// Turn stealth mode on or off for every NexQ window (and for windows opened
/// later, see [`apply_to_new_window`]). All or nothing: if a window refuses,
/// the windows already changed are put back and the setting stays as it was.
/// Rebuilds the tray menu so its check mark follows.
#[command]
pub async fn set_stealth_mode(app: AppHandle, enabled: bool) -> Result<(), String> {
    if enabled && !support().supported {
        return Err("Hiding from screen share isn't available on this platform".to_string());
    }
    let result = {
        let _guard = TRANSITION.lock().unwrap_or_else(|e| e.into_inner());
        let windows: Vec<(String, Window)> = app
            .webview_windows()
            .into_iter()
            .map(|(label, w)| (label, w.as_ref().window()))
            .collect();
        let hidden: Vec<&Window> = windows
            .iter()
            .filter(|(_, w)| !w.is_visible().unwrap_or(true))
            .map(|(_, w)| w)
            .collect();
        let result = transition(&windows, stealth_enabled(), enabled, |w, on| set_capture_excluded(w, on));
        // Changing capture visibility must never put a hidden window (e.g. the
        // overlay between meetings) on screen.
        for window in hidden {
            if window.is_visible().unwrap_or(false) {
                log::warn!("Stealth change showed hidden window '{}'; hiding it again", window.label());
                let _ = window.hide();
            }
        }
        if result.is_ok() {
            STEALTH.store(enabled, Ordering::SeqCst);
            log::info!(
                "Stealth mode {}",
                if enabled { "on: windows hidden from screen capture" } else { "off" }
            );
        }
        result
    };

    let meeting_active = app.state::<AppState>().meeting_active.load(Ordering::SeqCst);
    if let Err(e) = crate::commands::tray_commands::rebuild_tray_menu(app.clone(), meeting_active).await {
        log::warn!("Failed to refresh the tray menu after a stealth change: {e}");
    }
    result
}

/// Apply `enabled` to every window; on the first failure, restore `previous`
/// on the windows already changed. The error says whether that worked.
fn transition<W>(
    windows: &[(String, W)],
    previous: bool,
    enabled: bool,
    apply: impl Fn(&W, bool) -> Result<(), String>,
) -> Result<(), String> {
    for (i, (label, window)) in windows.iter().enumerate() {
        if let Err(e) = apply(window, enabled) {
            let not_restored: Vec<&str> = windows[..i]
                .iter()
                .filter(|(_, w)| apply(w, previous).is_err())
                .map(|(l, _)| l.as_str())
                .collect();
            let state = if previous { "hidden from" } else { "visible in" };
            return Err(if not_restored.is_empty() {
                format!("Couldn't change screen-share visibility ({label}: {e}); NexQ is still {state} screen share")
            } else {
                format!(
                    "Couldn't change screen-share visibility ({label}: {e}), and these windows couldn't be put back: {}. Toggle it again or restart NexQ",
                    not_restored.join(", ")
                )
            });
        }
    }
    Ok(())
}

/// Apply the current stealth setting to a window that was just created
/// (e.g. the dev log window), so it is never captured before it is hidden.
pub fn apply_to_new_window<R: Runtime>(window: &Window<R>) {
    let _guard = TRANSITION.lock().unwrap_or_else(|e| e.into_inner());
    if stealth_enabled() {
        if let Err(e) = set_capture_excluded(window, true) {
            log::warn!("Stealth mode: couldn't hide window '{}': {e}", window.label());
        }
    }
}

/// At startup, before the webviews load their settings: hide the windows
/// right away when stealth mode was saved on (the frontend's `config.json`
/// store, key `stealthMode`), so reopening NexQ mid-share never shows them.
pub fn restore_on_startup(app: &AppHandle) {
    let saved_on = app
        .path()
        .app_data_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(dir.join(FRONTEND_STORE)).ok())
        .map_or(false, |text| saved_stealth(&text));
    if !saved_on || !support().supported {
        return;
    }
    let _guard = TRANSITION.lock().unwrap_or_else(|e| e.into_inner());
    let windows: Vec<(String, Window)> = app
        .webview_windows()
        .into_iter()
        .map(|(label, w)| (label, w.as_ref().window()))
        .collect();
    match transition(&windows, false, true, |w, on| set_capture_excluded(w, on)) {
        Ok(()) => {
            STEALTH.store(true, Ordering::SeqCst);
            log::info!("Stealth mode restored at startup");
        }
        Err(e) => log::warn!("Stealth mode: couldn't restore at startup: {e}"),
    }
}

/// The frontend config store file (src/stores/configStore.ts `STORE_FILE`).
const FRONTEND_STORE: &str = "config.json";

fn saved_stealth(store_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(store_json)
        .ok()
        .and_then(|v| v.get("stealthMode").and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn set_capture_excluded<R: Runtime>(window: &Window<R>, excluded: bool) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WINDOW_DISPLAY_AFFINITY};

    // WDA_EXCLUDEFROMCAPTURE = 0x11 (Windows 10 v2004+), WDA_NONE = 0.
    const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;
    const WDA_NONE: u32 = 0;

    let hwnd = window.hwnd().map_err(|e| format!("no window handle: {e}"))?;
    let affinity = WINDOW_DISPLAY_AFFINITY(if excluded { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
    unsafe { SetWindowDisplayAffinity(HWND(hwnd.0 as *mut _), affinity) }
        .map_err(|e| format!("SetWindowDisplayAffinity failed: {e}"))
}

#[cfg(target_os = "macos")]
fn set_capture_excluded<R: Runtime>(window: &Window<R>, excluded: bool) -> Result<(), String> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    // NSWindowSharingType: 0 = None (left out of capture), 1 = ReadOnly (default).
    let sharing_type: usize = if excluded { 0 } else { 1 };
    let target = window.clone();
    // AppKit must only be touched from the main thread.
    window
        .run_on_main_thread(move || match target.ns_window() {
            Ok(ptr) if !ptr.is_null() => {
                // SAFETY: `ptr` is the live NSWindow backing this Tauri window.
                let ns_window: &AnyObject = unsafe { &*(ptr as *const AnyObject) };
                let _: () = unsafe { msg_send![ns_window, setSharingType: sharing_type] };
            }
            Ok(_) => log::warn!("Stealth mode: NSWindow pointer was null for '{}'", target.label()),
            Err(e) => log::warn!("Stealth mode: no NSWindow for '{}': {e}", target.label()),
        })
        .map_err(|e| format!("couldn't reach the main thread: {e}"))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn set_capture_excluded<R: Runtime>(_window: &Window<R>, excluded: bool) -> Result<(), String> {
    if excluded {
        Err("not supported on this platform".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Fake windows: (fails_when_set_to, current state).
    fn windows(specs: &[(&str, Option<bool>)]) -> Vec<(String, (Option<bool>, RefCell<bool>))> {
        specs
            .iter()
            .map(|(l, fail)| (l.to_string(), (*fail, RefCell::new(false))))
            .collect()
    }

    fn apply(w: &(Option<bool>, RefCell<bool>), on: bool) -> Result<(), String> {
        if w.0 == Some(on) {
            return Err("refused".into());
        }
        *w.1.borrow_mut() = on;
        Ok(())
    }

    #[test]
    fn all_windows_change_together() {
        let ws = windows(&[("launcher", None), ("overlay", None)]);
        assert!(transition(&ws, false, true, apply).is_ok());
        assert!(ws.iter().all(|(_, w)| *w.1.borrow()));
    }

    #[test]
    fn a_refusal_rolls_back_the_windows_already_changed() {
        let ws = windows(&[("launcher", None), ("overlay", Some(true))]);
        let err = transition(&ws, false, true, apply).unwrap_err();
        assert!(err.contains("still visible in"), "{err}");
        assert!(ws.iter().all(|(_, w)| !*w.1.borrow()));
    }

    #[test]
    fn a_failed_rollback_names_the_windows_left_changed() {
        // launcher accepts "on" but refuses going back to "off".
        let ws = windows(&[("launcher", Some(false)), ("overlay", Some(true))]);
        let err = transition(&ws, false, true, apply).unwrap_err();
        assert!(err.contains("couldn't be put back: launcher"), "{err}");
    }

    #[test]
    fn saved_setting_is_read_from_the_frontend_store() {
        assert!(saved_stealth(r#"{"stealthMode":true,"theme":"dark"}"#));
        assert!(!saved_stealth(r#"{"stealthMode":false}"#));
        assert!(!saved_stealth("{}"));
        assert!(!saved_stealth("not json"));
    }
}
