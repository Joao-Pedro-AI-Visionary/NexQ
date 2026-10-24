//! Meeting detection: notice when a call app (Zoom, Teams, Google Meet,
//! Webex…) starts a meeting and offer to start NexQ, like Notion's desktop
//! meeting detection. Only *which process* holds an active microphone session
//! and its window titles are read — never any audio.
//!
//! A background thread samples every [`POLL_EVERY`]; [`rules::Tracker`] turns
//! samples into one prompt per call. The prompt is a small always-on-top
//! window (`meeting-prompt`, `index.html?view=meeting-prompt`) shown without
//! taking focus from the call.

pub mod probe;
pub mod rules;

use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{command, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::state::AppState;
pub use rules::{DetectedMeeting, MeetingDetectionConfig};

/// What the prompt window shows (mirrors `MeetingPromptInfo` in types.ts).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MeetingPromptInfo {
    pub meeting: DetectedMeeting,
    pub auto_dismiss_secs: u32,
    pub default_scenario: String,
}

fn prompt_info(meeting: DetectedMeeting) -> MeetingPromptInfo {
    let config = with_detector(|d| d.config.clone());
    MeetingPromptInfo { meeting, auto_dismiss_secs: config.auto_dismiss_secs, default_scenario: config.default_scenario }
}
use rules::{Tracker, TrackerEvent};

pub const PROMPT_WINDOW: &str = "meeting-prompt";
const POLL_EVERY: Duration = Duration::from_millis(1500);
const PROMPT_WIDTH: f64 = 480.0;
const PROMPT_HEIGHT: f64 = 104.0;
const PROMPT_TOP_MARGIN: f64 = 14.0;

struct Detector {
    config: MeetingDetectionConfig,
    tracker: Tracker,
}

static DETECTOR: Mutex<Option<Detector>> = Mutex::new(None);

fn with_detector<T>(f: impl FnOnce(&mut Detector) -> T) -> T {
    let mut guard = DETECTOR.lock().unwrap_or_else(|e| e.into_inner());
    let det = guard.get_or_insert_with(|| Detector { config: MeetingDetectionConfig::default(), tracker: Tracker::default() });
    f(det)
}

/// Start the background detector (once, from setup).
pub fn start(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new().name("meeting-detect".into()).spawn(move || {
        #[cfg(target_os = "windows")]
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        loop {
            std::thread::sleep(POLL_EVERY);
            let config = with_detector(|d| d.config.clone());
            let meeting_active = app.state::<AppState>().meeting_active.load(Ordering::SeqCst);
            // Sampling is skipped while off; while NexQ records the tracker still
            // follows the call so it doesn't prompt for it once NexQ stops.
            let seen = if config.enabled {
                rules::detect(&probe::capturing_processes(), &probe::visible_windows(), &config)
            } else {
                None
            };
            let events = with_detector(|d| d.tracker.tick(Instant::now(), seen, meeting_active));
            for event in events {
                handle(&app, event);
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("Meeting detection unavailable: {e}");
    }
}

fn handle(app: &AppHandle, event: TrackerEvent) {
    match event {
        TrackerEvent::Prompt(meeting) => {
            log::info!("Meeting detected: {} ({:?})", meeting.app_name, meeting.title);
            if let Err(e) = show_prompt(app) {
                log::warn!("Couldn't show the meeting prompt: {e}");
            }
            let _ = app.emit("meeting_detected", prompt_info(meeting));
        }
        TrackerEvent::Ended => {
            log::info!("Detected meeting ended");
            hide_prompt(app);
            let _ = app.emit("detected_meeting_ended", ());
        }
        TrackerEvent::Hide => hide_prompt(app),
    }
}

fn prompt_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window(PROMPT_WINDOW) {
        return Ok(w);
    }
    WebviewWindowBuilder::new(app, PROMPT_WINDOW, WebviewUrl::App("index.html?view=meeting-prompt".into()))
        .title("NexQ — meeting detected")
        .inner_size(PROMPT_WIDTH, PROMPT_HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build()
}

/// Show the prompt at the top center of the primary screen, without taking
/// focus from the call.
fn show_prompt(app: &AppHandle) -> tauri::Result<()> {
    let window = prompt_window(app)?;
    if let Some(monitor) = window.primary_monitor()?.or(window.current_monitor()?) {
        let scale = monitor.scale_factor();
        let size = monitor.size().to_logical::<f64>(scale);
        let origin = monitor.position().to_logical::<f64>(scale);
        let x = origin.x + (size.width - PROMPT_WIDTH) / 2.0;
        let y = origin.y + PROMPT_TOP_MARGIN;
        window.set_position(tauri::LogicalPosition::new(x, y))?;
    }
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE};
        let hwnd = window.hwnd()?;
        unsafe {
            let _ = ShowWindow(HWND(hwnd.0 as *mut _), SW_SHOWNOACTIVATE);
        }
        // Keep it above the (often full-screen) call window.
        window.set_always_on_top(true)?;
    }
    #[cfg(not(target_os = "windows"))]
    window.show()?;
    Ok(())
}

fn hide_prompt(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(PROMPT_WINDOW) {
        let _ = w.hide();
    }
}

/// Apply the user's settings (pushed by the frontend on load and on change).
#[command]
pub fn set_meeting_detection_config(app: AppHandle, config: MeetingDetectionConfig) {
    let turned_off = !config.enabled;
    with_detector(|d| d.config = config);
    if turned_off {
        hide_prompt(&app);
    }
}

/// The call the prompt is about (the prompt window asks on load).
#[command]
pub fn get_detected_meeting() -> Option<MeetingPromptInfo> {
    with_detector(|d| d.tracker.current().cloned()).map(prompt_info)
}

/// The prompt was dismissed, timed out or started a meeting.
#[command]
pub fn dismiss_meeting_prompt(app: AppHandle) {
    with_detector(|d| d.tracker.prompt_closed());
    hide_prompt(&app);
}
