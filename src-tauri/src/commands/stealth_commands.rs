use tauri::{command, AppHandle, Manager};

/// Enable or disable stealth mode on the overlay window.
///
/// When enabled, the overlay window is excluded from screen captures and recordings
/// using the Windows API `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`.
///
/// This prevents the overlay from appearing in screenshots, screen recordings, or
/// screen-sharing sessions (e.g., Zoom, Teams, Discord).
#[command]
pub async fn set_stealth_mode(app: AppHandle, enabled: bool) -> Result<(), String> {
    let _overlay = app
        .get_webview_window("overlay")
        .ok_or_else(|| "Overlay window not found".to_string())?;

    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowDisplayAffinity, WINDOW_DISPLAY_AFFINITY,
        };

        // Obtain the HWND from the overlay window.
        // In Tauri 2, hwnd() is a direct method on WebviewWindow.
        let hwnd_raw = _overlay
            .hwnd()
            .map_err(|e| format!("Failed to get window handle: {}", e))?;

        let hwnd = HWND(hwnd_raw.0 as *mut _);

        // WDA_EXCLUDEFROMCAPTURE = 0x00000011 (Windows 10 v2004+)
        // WDA_NONE = 0x00000000
        let affinity = if enabled {
            WINDOW_DISPLAY_AFFINITY(0x00000011)
        } else {
            WINDOW_DISPLAY_AFFINITY(0x00000000)
        };

        unsafe {
            SetWindowDisplayAffinity(hwnd, affinity)
                .map_err(|e| format!("Failed to set display affinity: {}", e))?;
        }

        log::info!(
            "Stealth mode {}: overlay window {} from capture",
            if enabled { "enabled" } else { "disabled" },
            if enabled { "excluded" } else { "included" }
        );
    }

    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;

        // NSWindowSharingType: 0 = NSWindowSharingNone (excluded from capture),
        // 1 = NSWindowSharingReadOnly, 2 = NSWindowSharingReadWrite.
        // Setting `sharingType = NSWindowSharingNone` hides the window from screen
        // recording / screen sharing (Zoom, Meet, Teams, QuickTime, screenshots)
        // while it stays visible on the physical display — the macOS analogue of
        // Windows' SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE).
        let sharing_type: usize = if enabled { 0 } else { 1 };
        let overlay = _overlay.clone();

        // AppKit must only be touched from the main thread.
        app.run_on_main_thread(move || match overlay.ns_window() {
            Ok(ptr) if !ptr.is_null() => {
                let window: &AnyObject = unsafe { &*(ptr as *const AnyObject) };
                // SAFETY: `ptr` is the live NSWindow backing the Tauri overlay window.
                unsafe {
                    let _: () = msg_send![window, setSharingType: sharing_type];
                }
                log::info!(
                    "Stealth mode {}: macOS overlay NSWindow.sharingType = {}",
                    if sharing_type == 0 { "enabled" } else { "disabled" },
                    sharing_type
                );
            }
            Ok(_) => log::warn!("Stealth mode: overlay NSWindow pointer was null"),
            Err(e) => log::warn!("Stealth mode: failed to get overlay NSWindow: {}", e),
        })
        .map_err(|e| format!("Failed to dispatch stealth change to main thread: {}", e))?;
    }

    // Other platforms (Linux): stealth is not implemented.
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        if enabled {
            log::warn!("Stealth mode is only supported on Windows and macOS");
            return Err("Stealth mode is only supported on Windows and macOS".to_string());
        }
    }

    Ok(())
}
