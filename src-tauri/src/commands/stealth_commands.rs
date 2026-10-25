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
        for (_, window) in &windows {
            track_visibility(window);
        }
        let result = transition(&windows, stealth_enabled(), enabled, apply_setting);
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
    track_visibility(window);
    if stealth_enabled() {
        if let Err(e) = apply_setting(window, true) {
            log::warn!("Stealth mode: couldn't hide window '{}': {e}", window.label());
        }
    }
}

/// Set a window's capture visibility for the stealth setting `on`.
///
/// Windows: only a window on screen is excluded. A hidden, transparent
/// WebView2 window that is excluded from capture leaves a blurred ghost of its
/// frame on the desktop (the overlay between meetings, the launcher during
/// one). [`track_visibility`] applies the setting as a window is shown and
/// clears it when the window is hidden.
fn apply_setting<R: Runtime>(window: &Window<R>, on: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let on = on && window.is_visible().unwrap_or(true);
    set_capture_excluded(window, on)
}

/// Keep a window's capture visibility in step with whether it is shown.
#[cfg(target_os = "windows")]
fn track_visibility<R: Runtime>(window: &Window<R>) {
    let Ok(hwnd) = window.hwnd() else { return };
    let hwnd = hwnd.0 as isize;
    let target = window.clone();
    // A subclass must be installed from the thread that owns the window.
    let installed = window.run_on_main_thread(move || {
        if let Err(e) = visibility_hook::install(hwnd) {
            log::warn!("Stealth mode: can't follow visibility of '{}': {e}", target.label());
        }
    });
    if let Err(e) = installed {
        log::warn!("Stealth mode: couldn't reach the main thread: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
fn track_visibility<R: Runtime>(_window: &Window<R>) {}

/// Live blur of the desktop behind `window` (the overlay's glass), only while
/// it is on screen. Windows keeps drawing an accent blur for a hidden window
/// whenever it recomposites it, which showed as a pinned blurred box where the
/// overlay sits between meetings.
#[cfg(target_os = "windows")]
pub fn blur_behind_while_visible<R: Runtime>(window: &Window<R>) {
    let Ok(hwnd) = window.hwnd() else { return };
    let hwnd = hwnd.0 as isize;
    let visible = window.is_visible().unwrap_or(false);
    let target = window.clone();
    let queued = window.run_on_main_thread(move || {
        if let Err(e) = visibility_hook::install(hwnd) {
            log::warn!("Can't follow visibility of '{}': {e}", target.label());
            return;
        }
        visibility_hook::blur_while_visible(hwnd, visible);
    });
    if let Err(e) = queued {
        log::warn!("Blur behind: couldn't reach the main thread: {e}");
    }
}

/// A window subclass that follows a window being shown and hidden: it excludes
/// the window from capture just before it is shown (when stealth mode is on)
/// and makes it capturable again once hidden, and turns its blur-behind on
/// and off with it.
#[cfg(target_os = "windows")]
mod visibility_hook {
    use std::sync::Mutex;

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, SWP_HIDEWINDOW, SWP_SHOWWINDOW, WINDOWPOS, WINDOW_DISPLAY_AFFINITY,
        WM_NCDESTROY, WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING,
    };

    const SUBCLASS_ID: usize = 0x4E58_5153; // "NXQS"
    const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;
    const WDA_NONE: u32 = 0;

    static HOOKED: Mutex<Vec<isize>> = Mutex::new(Vec::new());
    /// Windows whose desktop blur follows their visibility.
    static BLURRED: Mutex<Vec<isize>> = Mutex::new(Vec::new());

    /// Turn blur-behind on for `hwnd` while it's visible (main thread only).
    pub fn blur_while_visible(hwnd: isize, visible_now: bool) {
        let mut blurred = BLURRED.lock().unwrap_or_else(|e| e.into_inner());
        if !blurred.contains(&hwnd) {
            blurred.push(hwnd);
        }
        drop(blurred);
        set_blur(HWND(hwnd as *mut _), visible_now);
    }

    fn wants_blur(hwnd: HWND) -> bool {
        BLURRED.lock().unwrap_or_else(|e| e.into_inner()).contains(&(hwnd.0 as isize))
    }

    /// Live blur of what's behind the window, via the undocumented
    /// `SetWindowCompositionAttribute` (user32): `DwmEnableBlurBehindWindow`
    /// is a no-op on Windows 10/11, and this one composites the live desktop.
    /// The gradient alpha is 1 (0 would disable the blur); the overlay's CSS
    /// (`.overlay-bg`) provides the tint.
    fn set_blur(hwnd: HWND, on: bool) {
        use std::ffi::c_void;
        use windows::core::PCSTR;
        use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

        #[repr(C)]
        struct AccentPolicy {
            accent_state: u32,
            accent_flags: u32,
            gradient_color: u32,
            animation_id: u32,
        }

        #[repr(C)]
        struct WindowCompositionAttribData {
            attrib: u32,
            pv_data: *mut c_void,
            cb_data: usize,
        }

        const WCA_ACCENT_POLICY: u32 = 19;
        const ACCENT_DISABLED: u32 = 0;
        const ACCENT_ENABLE_BLURBEHIND: u32 = 3;

        // Not in user32.lib's import table: resolved at runtime.
        type SetWindowCompositionAttributeFn =
            unsafe extern "system" fn(HWND, *mut WindowCompositionAttribData) -> i32;

        unsafe {
            let Ok(module) = GetModuleHandleA(PCSTR(b"user32.dll\0".as_ptr())) else {
                return;
            };
            let Some(proc) = GetProcAddress(module, PCSTR(b"SetWindowCompositionAttribute\0".as_ptr()))
            else {
                return;
            };
            let set_window_composition_attribute: SetWindowCompositionAttributeFn =
                std::mem::transmute(proc);
            let mut policy = AccentPolicy {
                accent_state: if on { ACCENT_ENABLE_BLURBEHIND } else { ACCENT_DISABLED },
                accent_flags: 0,
                gradient_color: if on { 0x0100_0000 } else { 0 },
                animation_id: 0,
            };
            let mut data = WindowCompositionAttribData {
                attrib: WCA_ACCENT_POLICY,
                pv_data: &mut policy as *mut _ as *mut c_void,
                cb_data: std::mem::size_of::<AccentPolicy>(),
            };
            set_window_composition_attribute(hwnd, &mut data);
        }
    }

    /// Install once per window (page reloads call this again). Main thread only.
    pub fn install(hwnd: isize) -> Result<(), String> {
        let mut hooked = HOOKED.lock().unwrap_or_else(|e| e.into_inner());
        if hooked.contains(&hwnd) {
            return Ok(());
        }
        let ok = unsafe { SetWindowSubclass(HWND(hwnd as *mut _), Some(subclass_proc), SUBCLASS_ID, 0) };
        if !ok.as_bool() {
            return Err("SetWindowSubclass failed".into());
        }
        hooked.push(hwnd);
        Ok(())
    }

    fn set_affinity(hwnd: HWND, excluded: bool) {
        let affinity = WINDOW_DISPLAY_AFFINITY(if excluded { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
        if let Err(e) = unsafe { SetWindowDisplayAffinity(hwnd, affinity) } {
            log::warn!("Stealth mode: SetWindowDisplayAffinity failed: {e}");
        }
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        match msg {
            WM_WINDOWPOSCHANGING => {
                let pos = &*(lparam.0 as *const WINDOWPOS);
                // About to be shown: hide it from capture first when stealth
                // is on, so no frame of it reaches a screen share.
                if pos.flags.contains(SWP_SHOWWINDOW) && super::stealth_enabled() {
                    set_affinity(hwnd, true);
                }
                // About to be hidden: drop the blur first, or it stays on screen.
                if pos.flags.contains(SWP_HIDEWINDOW) && wants_blur(hwnd) {
                    set_blur(hwnd, false);
                }
            }
            // Hidden: nothing of it should stay excluded (or composited).
            WM_WINDOWPOSCHANGED => {
                let pos = &*(lparam.0 as *const WINDOWPOS);
                if pos.flags.contains(SWP_HIDEWINDOW) {
                    set_affinity(hwnd, false);
                }
                // Shown: the accent only takes on a visible window.
                if pos.flags.contains(SWP_SHOWWINDOW) && wants_blur(hwnd) {
                    set_blur(hwnd, true);
                }
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
                let mut hooked = HOOKED.lock().unwrap_or_else(|e| e.into_inner());
                hooked.retain(|&h| h != hwnd.0 as isize);
                drop(hooked);
                BLURRED.lock().unwrap_or_else(|e| e.into_inner()).retain(|&h| h != hwnd.0 as isize);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
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
    // Stealth is on from here, so windows shown later are hidden from capture.
    STEALTH.store(true, Ordering::SeqCst);
    for (_, window) in &windows {
        track_visibility(window);
    }
    match transition(&windows, false, true, apply_setting) {
        Ok(()) => log::info!("Stealth mode restored at startup"),
        Err(e) => {
            STEALTH.store(false, Ordering::SeqCst);
            log::warn!("Stealth mode: couldn't restore at startup: {e}");
        }
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
