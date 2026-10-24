//! Which call app is in a meeting, from what the OS reports: the processes
//! holding an active microphone session and the titles of their windows.
//! Pure functions, so the rules are testable without Zoom installed.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// User settings for meeting detection (mirrors `MeetingDetectionConfig` in types.ts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeetingDetectionConfig {
    pub enabled: bool,
    pub zoom: bool,
    pub teams: bool,
    pub google_meet: bool,
    pub webex: bool,
    pub slack: bool,
    pub discord: bool,
    /// Seconds before an unanswered prompt goes away (0 = stays until the call ends).
    pub auto_dismiss_secs: u32,
    /// AI scenario a meeting started from the prompt uses (`AIScenario`).
    pub default_scenario: String,
}

impl Default for MeetingDetectionConfig {
    /// Off until the frontend pushes the saved settings at startup.
    fn default() -> Self {
        Self {
            enabled: false,
            zoom: true,
            teams: true,
            google_meet: true,
            webex: true,
            slack: false,
            discord: false,
            auto_dismiss_secs: 60,
            default_scenario: "team_meeting".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallApp {
    Zoom,
    Teams,
    GoogleMeet,
    Webex,
    Slack,
    Discord,
}

impl CallApp {
    pub fn id(self) -> &'static str {
        match self {
            CallApp::Zoom => "zoom",
            CallApp::Teams => "teams",
            CallApp::GoogleMeet => "google_meet",
            CallApp::Webex => "webex",
            CallApp::Slack => "slack",
            CallApp::Discord => "discord",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CallApp::Zoom => "Zoom",
            CallApp::Teams => "Microsoft Teams",
            CallApp::GoogleMeet => "Google Meet",
            CallApp::Webex => "Webex",
            CallApp::Slack => "Slack",
            CallApp::Discord => "Discord",
        }
    }

    fn enabled_in(self, cfg: &MeetingDetectionConfig) -> bool {
        match self {
            CallApp::Zoom => cfg.zoom,
            CallApp::Teams => cfg.teams,
            CallApp::GoogleMeet => cfg.google_meet,
            CallApp::Webex => cfg.webex,
            CallApp::Slack => cfg.slack,
            CallApp::Discord => cfg.discord,
        }
    }
}

/// A visible top-level window: its process's executable (lowercase file
/// name), title and window class.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub exe: String,
    pub title: String,
    pub class: String,
}

/// A meeting in progress in a call app (sent to the frontend as `DetectedMeeting`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DetectedMeeting {
    /// `CallApp::id`.
    pub app: String,
    pub app_name: String,
    /// The meeting's title when the app shows one in a window title.
    pub title: Option<String>,
}

impl DetectedMeeting {
    fn new(app: CallApp, title: Option<String>) -> Self {
        Self { app: app.id().to_string(), app_name: app.name().to_string(), title }
    }
}

const BROWSERS: &[&str] = &[
    "chrome.exe", "msedge.exe", "firefox.exe", "brave.exe", "opera.exe", "vivaldi.exe", "arc.exe",
];

fn native_app(exe: &str) -> Option<CallApp> {
    Some(match exe {
        "zoom.exe" => CallApp::Zoom,
        "ms-teams.exe" | "teams.exe" | "msteams.exe" => CallApp::Teams,
        "ciscocollabhost.exe" | "webex.exe" | "webexmta.exe" | "atmgr.exe" => CallApp::Webex,
        "slack.exe" => CallApp::Slack,
        "discord.exe" => CallApp::Discord,
        _ => return None,
    })
}

/// The meeting in progress, if a call app is using the microphone.
/// `capturing` holds the lowercase executable names of processes with an
/// active microphone session (NexQ itself excluded).
pub fn detect(capturing: &[String], windows: &[WindowInfo], cfg: &MeetingDetectionConfig) -> Option<DetectedMeeting> {
    if !cfg.enabled {
        return None;
    }
    fn windows_of<'a>(windows: &'a [WindowInfo], exe: &'a str) -> impl Iterator<Item = &'a WindowInfo> + 'a {
        windows.iter().filter(move |w| w.exe == exe)
    }

    // Desktop apps first: their mic session is the meeting itself.
    for exe in capturing {
        if let Some(app) = native_app(exe) {
            if !app.enabled_in(cfg) {
                continue;
            }
            let title = match app {
                CallApp::Zoom => windows_of(windows, exe).find_map(|w| zoom_title(&w.title)),
                CallApp::Teams => windows_of(windows, exe).find_map(|w| teams_title(&w.title)),
                CallApp::Webex => windows_of(windows, exe).find_map(|w| webex_title(&w.title)),
                _ => None,
            };
            return Some(DetectedMeeting::new(app, title));
        }
    }

    // Browsers use the mic for many things: only a meeting tab counts. The
    // window title is the active tab's.
    for exe in capturing.iter().filter(|e| BROWSERS.contains(&e.as_str())) {
        for w in windows_of(windows, exe) {
            let page = strip_browser_suffix(&w.title);
            if let Some(title) = google_meet_title(&page) {
                if cfg.google_meet {
                    return Some(DetectedMeeting::new(CallApp::GoogleMeet, title));
                }
            } else if page.contains("Microsoft Teams") {
                if cfg.teams {
                    return Some(DetectedMeeting::new(CallApp::Teams, teams_title(&page)));
                }
            } else if page.contains("Zoom Meeting") || page.contains("Zoom Webinar") {
                if cfg.zoom {
                    return Some(DetectedMeeting::new(CallApp::Zoom, zoom_title(&page)));
                }
            } else if page.contains("Webex") && cfg.webex {
                return Some(DetectedMeeting::new(CallApp::Webex, webex_title(&page)));
            }
        }
    }
    None
}

fn clean(title: &str) -> String {
    title.replace(['\u{200b}', '\u{200e}', '\u{200f}'], "").trim().to_string()
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim().trim_matches(|c: char| c == '-' || c == '–' || c == '|' || c == ':').trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// "Weekly sync - Google Chrome", "X and 2 more pages - Personal - Microsoft Edge",
/// "X — Mozilla Firefox" → the page title.
fn strip_browser_suffix(title: &str) -> String {
    let mut t = clean(title);
    const NAMES: &[&str] = &["Google Chrome", "Microsoft Edge", "Mozilla Firefox", "Brave", "Opera", "Vivaldi", "Arc"];
    for name in NAMES {
        for sep in [" - ", " — ", " – "] {
            if let Some(stripped) = t.strip_suffix(&format!("{sep}{name}")) {
                t = stripped.to_string();
                // Edge adds the profile name before its own: "… - Personal - Microsoft Edge".
                if *name == "Microsoft Edge" {
                    if let Some(i) = t.rfind(" - ") {
                        t.truncate(i);
                    }
                }
            }
        }
    }
    if let Some(i) = t.find(" and ").filter(|&i| t[i..].contains(" more page")) {
        t.truncate(i);
    }
    t.trim().to_string()
}

/// Google Meet tab: "Meet - abc-defg-hij" (no title) or "Meet – Weekly sync".
/// `Some(None)`: a Meet call without a title; `None`: not a Meet tab.
fn google_meet_title(page: &str) -> Option<Option<String>> {
    let rest = page
        .strip_prefix("Meet - ")
        .or_else(|| page.strip_prefix("Meet – "))
        .or_else(|| page.strip_prefix("Meet — "));
    let rest = match rest {
        Some(r) => r,
        None if page.contains("Google Meet") => return Some(non_empty(&page.replace("Google Meet", ""))),
        None => return None,
    };
    let is_code = {
        let parts: Vec<&str> = rest.split('-').collect();
        parts.len() == 3
            && [3, 4, 3].iter().zip(&parts).all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_lowercase()))
    };
    Some(if is_code { None } else { non_empty(rest) })
}

/// Zoom shows "Zoom Meeting" (sometimes with the meeting ID); a topic, when a
/// client version puts one in the title, is whatever remains.
fn zoom_title(title: &str) -> Option<String> {
    let t = clean(title);
    if !(t.contains("Zoom Meeting") || t.contains("Zoom Webinar")) {
        return None;
    }
    let mut t = t;
    if let Some(i) = t.find("Meeting ID") {
        t.truncate(i);
    }
    non_empty(&t.replace("Zoom Meeting", "").replace("Zoom Webinar", "").replace("Zoom", ""))
}

/// "Weekly sync | Microsoft Teams" → "Weekly sync"; the main window's pages
/// ("Chat | Ann | Microsoft Teams", "Calendar | Microsoft Teams") are not meetings.
fn teams_title(title: &str) -> Option<String> {
    const PAGES: &[&str] = &[
        "Chat", "Activity", "Calendar", "Teams", "Calls", "Files", "Apps", "Communities", "OneDrive", "Copilot",
        "Search", "Microsoft Teams", "Teams and channels", "Assignments", "Settings",
    ];
    let t = clean(title);
    let first = t.split(" | ").next().unwrap_or("").trim();
    if !t.contains("Microsoft Teams") || PAGES.contains(&first) {
        return None;
    }
    non_empty(first)
}

fn webex_title(title: &str) -> Option<String> {
    let t = clean(title);
    let rest = t.replace("Cisco Webex Meetings", "").replace("Webex Meetings", "").replace("Webex", "");
    non_empty(&rest)
}

/// What to do after a detection tick.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackerEvent {
    /// A meeting started (held for `CONFIRM_FOR`) and NexQ isn't recording: offer to start.
    Prompt(DetectedMeeting),
    /// The call ended (no mic session for `END_GRACE`).
    Ended,
    /// NexQ started a meeting: the offer is moot.
    Hide,
}

/// The mic session must last this long before prompting (no flash on short checks).
pub const CONFIRM_FOR: Duration = Duration::from_secs(3);
/// A call counts as ended after its mic session has been gone this long
/// (rejoins and device switches drop it briefly).
pub const END_GRACE: Duration = Duration::from_secs(10);

/// Turns detection samples into prompt / end events: one prompt per call.
#[derive(Debug, Default)]
pub struct Tracker {
    pending: Option<(String, Instant)>,
    active: Option<DetectedMeeting>,
    gone_since: Option<Instant>,
    prompt_open: bool,
}

impl Tracker {
    pub fn current(&self) -> Option<&DetectedMeeting> {
        self.active.as_ref()
    }

    /// The prompt was dismissed or acted on.
    pub fn prompt_closed(&mut self) {
        self.prompt_open = false;
    }

    pub fn tick(&mut self, now: Instant, seen: Option<DetectedMeeting>, meeting_active: bool) -> Vec<TrackerEvent> {
        let mut events = Vec::new();
        if meeting_active && self.prompt_open {
            self.prompt_open = false;
            events.push(TrackerEvent::Hide);
        }
        match (seen, &mut self.active) {
            (Some(seen), Some(active)) if seen.app == active.app => {
                self.gone_since = None;
                // A title can show up after the meeting window opens.
                if active.title.is_none() && seen.title.is_some() {
                    active.title = seen.title;
                }
            }
            (seen, Some(_)) => {
                let gone = *self.gone_since.get_or_insert(now);
                if now.duration_since(gone) >= END_GRACE {
                    self.active = None;
                    self.gone_since = None;
                    self.pending = None;
                    if self.prompt_open {
                        self.prompt_open = false;
                    }
                    events.push(TrackerEvent::Ended);
                    if let Some(seen) = seen {
                        self.pending = Some((seen.app, now));
                    }
                }
            }
            (Some(seen), None) => match &self.pending {
                Some((app, since)) if *app == seen.app => {
                    if now.duration_since(*since) >= CONFIRM_FOR {
                        self.pending = None;
                        self.active = Some(seen.clone());
                        if !meeting_active {
                            self.prompt_open = true;
                            events.push(TrackerEvent::Prompt(seen));
                        }
                    }
                }
                _ => self.pending = Some((seen.app, now)),
            },
            (None, None) => self.pending = None,
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MeetingDetectionConfig {
        MeetingDetectionConfig { enabled: true, ..Default::default() }
    }

    fn win(exe: &str, title: &str) -> WindowInfo {
        WindowInfo { exe: exe.into(), title: title.into(), class: String::new() }
    }

    fn caps(exes: &[&str]) -> Vec<String> {
        exes.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn zoom_on_the_mic_is_a_meeting_with_its_title_when_shown() {
        let d = detect(&caps(&["zoom.exe"]), &[win("zoom.exe", "Zoom Workplace"), win("zoom.exe", "Zoom Meeting")], &cfg()).unwrap();
        assert_eq!((d.app.as_str(), d.app_name.as_str(), d.title.as_deref()), ("zoom", "Zoom", None));
        let d = detect(&caps(&["zoom.exe"]), &[win("zoom.exe", "Zoom Meeting - Design review")], &cfg()).unwrap();
        assert_eq!(d.title.as_deref(), Some("Design review"));
        assert_eq!(zoom_title("Zoom Meeting ID: 812 345 6789"), None);
    }

    #[test]
    fn teams_meeting_window_gives_the_title_and_pages_do_not() {
        let windows = [win("ms-teams.exe", "Chat | Ann Lee | Microsoft Teams"), win("ms-teams.exe", "Weekly sync | Microsoft Teams")];
        let d = detect(&caps(&["ms-teams.exe"]), &windows, &cfg()).unwrap();
        assert_eq!((d.app.as_str(), d.title.as_deref()), ("teams", Some("Weekly sync")));
        assert_eq!(teams_title("Calendar | Microsoft Teams"), None);
    }

    #[test]
    fn browsers_count_only_with_a_meeting_tab() {
        let meet = detect(&caps(&["chrome.exe"]), &[win("chrome.exe", "Meet – Q4 planning - Google Chrome")], &cfg()).unwrap();
        assert_eq!((meet.app.as_str(), meet.title.as_deref()), ("google_meet", Some("Q4 planning")));
        let code = detect(&caps(&["msedge.exe"]), &[win("msedge.exe", "Meet - abc-defg-hij and 2 more pages - Personal - Microsoft\u{200b} Edge")], &cfg()).unwrap();
        assert_eq!((code.app.as_str(), code.title), ("google_meet", None));
        assert_eq!(detect(&caps(&["chrome.exe"]), &[win("chrome.exe", "Quora - Google Chrome")], &cfg()), None);
    }

    #[test]
    fn disabled_apps_and_detection_are_ignored() {
        let off = MeetingDetectionConfig { zoom: false, ..cfg() };
        assert_eq!(detect(&caps(&["zoom.exe"]), &[], &off), None);
        assert_eq!(detect(&caps(&["zoom.exe"]), &[], &MeetingDetectionConfig::default()), None);
        assert_eq!(detect(&caps(&["discord.exe"]), &[], &cfg()), None); // off by default
        assert_eq!(detect(&caps(&["obs64.exe"]), &[], &cfg()), None);
    }

    fn zoom() -> Option<DetectedMeeting> {
        Some(DetectedMeeting::new(CallApp::Zoom, None))
    }

    #[test]
    fn prompts_once_after_the_call_holds_then_ends_after_grace() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let mut tr = Tracker::default();
        assert!(tr.tick(s(0), zoom(), false).is_empty());
        assert!(tr.tick(s(2), zoom(), false).is_empty());
        assert_eq!(tr.tick(s(3), zoom(), false), vec![TrackerEvent::Prompt(zoom().unwrap())]);
        assert!(tr.tick(s(5), zoom(), false).is_empty()); // once per call
        assert!(tr.tick(s(6), None, false).is_empty()); // brief drop
        assert!(tr.tick(s(8), zoom(), false).is_empty());
        assert!(tr.tick(s(9), None, false).is_empty());
        assert_eq!(tr.tick(s(19), None, false), vec![TrackerEvent::Ended]);
        assert!(tr.current().is_none());
    }

    #[test]
    fn no_prompt_while_nexq_records_and_a_new_meeting_hides_it() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let mut tr = Tracker::default();
        tr.tick(s(0), zoom(), true);
        assert!(tr.tick(s(4), zoom(), true).is_empty());

        let mut tr = Tracker::default();
        tr.tick(s(0), zoom(), false);
        assert!(matches!(tr.tick(s(4), zoom(), false)[..], [TrackerEvent::Prompt(_)]));
        assert_eq!(tr.tick(s(5), zoom(), true), vec![TrackerEvent::Hide]);
    }

    #[test]
    fn a_title_that_appears_later_is_kept() {
        let t0 = Instant::now();
        let mut tr = Tracker::default();
        tr.tick(t0, zoom(), false);
        tr.tick(t0 + CONFIRM_FOR, zoom(), false);
        let titled = Some(DetectedMeeting::new(CallApp::Zoom, Some("Standup".into())));
        tr.tick(t0 + CONFIRM_FOR + Duration::from_secs(1), titled, false);
        assert_eq!(tr.current().unwrap().title.as_deref(), Some("Standup"));
    }
}
