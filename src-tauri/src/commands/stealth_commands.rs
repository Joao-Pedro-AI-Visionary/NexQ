use std::sync::atomic::{AtomicBool, Ordering};

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

/// Turn stealth mode on or off for every NexQ window (and for windows opened
/// later, see [`apply_to_new_window`]). Rebuilds the tray menu so its check
/// mark follows.
#[command]
pub async fn set_stealth_mode(app: AppHandle, enabled: bool) -> Result<(), String> {
    if enabled && !support().supported {
        return Err("Hiding from screen share isn't available on this platform".to_string());
    }
    STEALTH.store(enabled, Ordering::SeqCst);

    let mut errors = Vec::new();
    for (label, window) in app.webview_windows() {
        if let Err(e) = set_capture_excluded(&window.as_ref().window(), enabled) {
            errors.push(format!("{label}: {e}"));
        }
    }
    log::info!(
        "Stealth mode {}",
        if enabled { "on: windows hidden from screen capture" } else { "off" }
    );

    let meeting_active = app.state::<AppState>().meeting_active.load(Ordering::SeqCst);
    if let Err(e) = crate::commands::tray_commands::rebuild_tray_menu(app.clone(), meeting_active).await {
        log::warn!("Failed to refresh the tray menu after a stealth change: {e}");
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("Couldn't hide every window from capture ({})", errors.join("; ")))
    }
}

/// Apply the current stealth setting to a window that was just created
/// (e.g. the dev log window), so it is never captured before it is hidden.
pub fn apply_to_new_window<R: Runtime>(window: &Window<R>) {
    if stealth_enabled() {
        if let Err(e) = set_capture_excluded(window, true) {
            log::warn!("Stealth mode: couldn't hide window '{}': {e}", window.label());
        }
    }
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
