//! The transcript scope of a user action ("since my last answer", "the last
//! 5 minutes", "the whole meeting"), resolved by the frontend at request
//! time and applied here: a transcript window on the stateless path, a short
//! "## Transcript scope" note on the Claude session path (the session already
//! holds every line).

use serde::{Deserialize, Serialize};

/// Header of the session-path scope note.
pub const SCOPE_HEADER: &str = "## Transcript scope";

/// Which part of the meeting an action focuses on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    /// The action's own transcript window (today's behaviour).
    Default,
    /// Since the previous answer of any action.
    SinceLastAny,
    /// Since the previous answer of the same action.
    SinceLastSame,
    /// The last `minutes` minutes.
    Minutes,
    /// The whole meeting.
    All,
}

/// A scope resolved by the frontend (`ResolvedScope` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedScope {
    pub kind: ScopeKind,
    /// Epoch ms of the first segment to focus on (`since_*`, `minutes`).
    #[serde(default)]
    pub since_ms: Option<i64>,
    #[serde(default)]
    pub minutes: Option<u32>,
    /// Shown to the user ("Last 5 min", "Since last answer"...).
    #[serde(default)]
    pub label: String,
}

impl ResolvedScope {
    /// A scope that changes nothing (absent or `default`).
    pub fn is_default(scope: Option<&ResolvedScope>) -> bool {
        scope.map_or(true, |s| s.kind == ScopeKind::Default)
    }

    /// The label for the stream start payload; `None` for the default scope.
    pub fn label_of(scope: Option<&ResolvedScope>) -> Option<String> {
        scope
            .filter(|s| s.kind != ScopeKind::Default)
            .map(|s| s.label.clone())
            .filter(|l| !l.trim().is_empty())
    }
}

/// Which segments the stateless path keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptWindow {
    /// The last N seconds before the newest segment; 0 = everything.
    Seconds(u64),
    /// Everything from this epoch ms on.
    Since(i64),
}

/// The stateless transcript window for `scope`, given the action's own
/// window (`action_window_seconds`, 0 = all).
pub fn stateless_window(scope: Option<&ResolvedScope>, action_window_seconds: u64) -> TranscriptWindow {
    let Some(scope) = scope else {
        return TranscriptWindow::Seconds(action_window_seconds);
    };
    match scope.kind {
        ScopeKind::Default => TranscriptWindow::Seconds(action_window_seconds),
        ScopeKind::All => TranscriptWindow::Seconds(0),
        ScopeKind::Minutes => match scope.minutes.filter(|m| *m > 0) {
            Some(m) => TranscriptWindow::Seconds(u64::from(m) * 60),
            None => scope
                .since_ms
                .map_or(TranscriptWindow::Seconds(action_window_seconds), TranscriptWindow::Since),
        },
        ScopeKind::SinceLastAny | ScopeKind::SinceLastSame => scope
            .since_ms
            .map_or(TranscriptWindow::Seconds(0), TranscriptWindow::Since),
    }
}

/// Indexes of the segments (given by start time, in transcript order) whose
/// span overlaps the window. A segment spans from its start to the next
/// segment's start; the newest one is still open. So a long merged segment
/// that began before the cutoff but ran past it is kept.
pub fn overlapping(starts: &[u64], window: TranscriptWindow) -> Vec<usize> {
    let Some(&latest) = starts.iter().max() else {
        return Vec::new();
    };
    let cutoff = match window {
        TranscriptWindow::Seconds(0) => return (0..starts.len()).collect(),
        TranscriptWindow::Seconds(secs) => latest.saturating_sub(secs.saturating_mul(1000)),
        TranscriptWindow::Since(ms) => ms.max(0) as u64,
    };
    (0..starts.len())
        .filter(|&i| starts[i] >= cutoff || starts.get(i + 1).map_or(true, |&next| next > cutoff))
        .collect()
}

/// The window to report in the start payload (seconds, 0 = all).
pub fn effective_window_seconds(window: TranscriptWindow, starts: &[u64]) -> u64 {
    match window {
        TranscriptWindow::Seconds(secs) => secs,
        TranscriptWindow::Since(ms) => {
            let latest = starts.iter().copied().max().unwrap_or(0) as i64;
            // At least a second, so it never reads as "all".
            ((latest - ms).max(0) as u64).div_ceil(1000).max(1)
        }
    }
}

/// `[mm:ss]` of an epoch (or already relative) time, relative to the live
/// meeting start, like the transcript lines (minutes keep counting past 59).
fn clock(ms: i64, start_ms: i64) -> String {
    let secs = crate::llm::cli_agent::routing::relative_ms(ms, start_ms) / 1000;
    format!("[{:02}:{:02}]", secs / 60, secs % 60)
}

/// The body of the session-path `## Transcript scope` note; `None` for the
/// default scope (the session decides, as today).
pub fn session_scope_note(scope: Option<&ResolvedScope>, start_ms: i64) -> Option<String> {
    let scope = scope?;
    const BACKGROUND: &str = "earlier lines are background only.";
    match scope.kind {
        ScopeKind::Default => None,
        ScopeKind::All => Some("Base your answer on the whole meeting so far.".to_string()),
        ScopeKind::Minutes => {
            let minutes = scope.minutes.unwrap_or(0);
            let span = if minutes == 1 { "the last minute".to_string() } else { format!("the last {minutes} minutes") };
            Some(match scope.since_ms {
                Some(since) => format!(
                    "Base your answer only on the transcript from {} onward ({span}); {BACKGROUND}",
                    clock(since, start_ms)
                ),
                None => format!("Base your answer only on {span} of the transcript; {BACKGROUND}"),
            })
        }
        ScopeKind::SinceLastAny | ScopeKind::SinceLastSame => {
            let since_what = if scope.kind == ScopeKind::SinceLastAny {
                "your previous answer"
            } else {
                "your previous answer to this same action"
            };
            Some(match scope.since_ms {
                Some(since) => format!(
                    "Base your answer only on the transcript since {since_what}, from {} onward; {BACKGROUND}",
                    clock(since, start_ms)
                ),
                None => "Base your answer on the whole meeting so far.".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::cli_agent::context::Seg;
    use crate::llm::cli_agent::routing::compose_delta;

    const START: i64 = 1_700_000_000_000;

    fn scope(kind: ScopeKind, since_ms: Option<i64>, minutes: Option<u32>) -> ResolvedScope {
        ResolvedScope { kind, since_ms, minutes, label: "L".into() }
    }

    #[test]
    fn scope_deserializes_from_the_frontend_shape() {
        let s: ResolvedScope =
            serde_json::from_str(r#"{"kind":"minutes","since_ms":1700000300000,"minutes":5,"label":"Last 5 min"}"#).unwrap();
        assert_eq!(s, ResolvedScope { kind: ScopeKind::Minutes, since_ms: Some(1_700_000_300_000), minutes: Some(5), label: "Last 5 min".into() });
        let s: ResolvedScope = serde_json::from_str(r#"{"kind":"since_last_same","since_ms":null,"label":"x"}"#).unwrap();
        assert_eq!((s.kind, s.since_ms, s.minutes), (ScopeKind::SinceLastSame, None, None));
        for (json, kind) in [
            ("default", ScopeKind::Default),
            ("since_last_any", ScopeKind::SinceLastAny),
            ("all", ScopeKind::All),
        ] {
            let s: ResolvedScope = serde_json::from_str(&format!(r#"{{"kind":"{json}","since_ms":null}}"#)).unwrap();
            assert_eq!(s.kind, kind);
        }
        assert!(serde_json::from_str::<ResolvedScope>(r#"{"kind":"bogus"}"#).is_err());
    }

    #[test]
    fn default_scope_has_no_label_and_keeps_the_action_window() {
        let d = scope(ScopeKind::Default, None, None);
        assert!(ResolvedScope::is_default(None) && ResolvedScope::is_default(Some(&d)));
        assert_eq!(ResolvedScope::label_of(Some(&d)), None);
        assert_eq!(ResolvedScope::label_of(Some(&scope(ScopeKind::All, None, None))), Some("L".into()));
        assert_eq!(stateless_window(None, 300), TranscriptWindow::Seconds(300));
        assert_eq!(stateless_window(Some(&d), 30), TranscriptWindow::Seconds(30));
    }

    #[test]
    fn stateless_window_per_kind() {
        assert_eq!(stateless_window(Some(&scope(ScopeKind::All, None, None)), 300), TranscriptWindow::Seconds(0));
        assert_eq!(stateless_window(Some(&scope(ScopeKind::Minutes, Some(5), Some(10))), 300), TranscriptWindow::Seconds(600));
        assert_eq!(
            stateless_window(Some(&scope(ScopeKind::SinceLastAny, Some(START), None)), 300),
            TranscriptWindow::Since(START)
        );
        assert_eq!(
            stateless_window(Some(&scope(ScopeKind::SinceLastSame, None, None)), 300),
            TranscriptWindow::Seconds(0),
            "no previous answer: the whole meeting"
        );
    }

    #[test]
    fn overlap_keeps_a_long_segment_that_began_before_the_cutoff() {
        // Segments start at 0 s, 100 s (a long merged one) and 290 s.
        let starts = [0, 100_000, 290_000];
        // A 60 s window ends at 290 s: cutoff 230 s. The 100 s segment ran until 290 s.
        assert_eq!(overlapping(&starts, TranscriptWindow::Seconds(60)), vec![1, 2]);
        assert_eq!(overlapping(&starts, TranscriptWindow::Seconds(0)), vec![0, 1, 2]);
        assert_eq!(overlapping(&starts, TranscriptWindow::Since(100_000)), vec![1, 2]);
        assert_eq!(overlapping(&starts, TranscriptWindow::Since(50_000)), vec![0, 1, 2]);
        // Nothing new since: the newest (still open) segment is the overlap.
        assert_eq!(overlapping(&starts, TranscriptWindow::Since(400_000)), vec![2]);
        assert!(overlapping(&[], TranscriptWindow::Seconds(60)).is_empty());
    }

    #[test]
    fn effective_window_reports_seconds() {
        assert_eq!(effective_window_seconds(TranscriptWindow::Seconds(600), &[0]), 600);
        assert_eq!(effective_window_seconds(TranscriptWindow::Since(10_000), &[0, 70_500]), 61);
        assert_eq!(effective_window_seconds(TranscriptWindow::Since(90_000), &[0, 70_500]), 1);
    }

    #[test]
    fn session_note_per_kind() {
        assert_eq!(session_scope_note(None, START), None);
        assert_eq!(session_scope_note(Some(&scope(ScopeKind::Default, None, None)), START), None);
        assert_eq!(
            session_scope_note(Some(&scope(ScopeKind::All, None, None)), START).unwrap(),
            "Base your answer on the whole meeting so far."
        );
        assert_eq!(
            session_scope_note(Some(&scope(ScopeKind::Minutes, Some(START + 750_000), Some(5))), START).unwrap(),
            "Base your answer only on the transcript from [12:30] onward (the last 5 minutes); earlier lines are background only."
        );
        assert_eq!(
            session_scope_note(Some(&scope(ScopeKind::SinceLastAny, Some(START + 842_000), None)), START).unwrap(),
            "Base your answer only on the transcript since your previous answer, from [14:02] onward; earlier lines are background only."
        );
        assert!(session_scope_note(Some(&scope(ScopeKind::SinceLastSame, Some(START + 3_725_000), None)), START)
            .unwrap()
            .contains("previous answer to this same action, from [62:05] onward"));
    }

    #[test]
    fn compose_delta_carries_the_scope_section_before_the_task() {
        let seg = Seg { id: "s1".into(), text: "We ship Friday".into(), speaker: "Them".into(), timestamp_ms: 61_000 };
        let note = session_scope_note(Some(&scope(ScopeKind::All, None, None)), START).unwrap();
        let p = compose_delta(&[seg], &[("## Relevant document excerpts", ""), (SCOPE_HEADER, &note)], "Recap.", None, None);
        assert_eq!(
            p,
            "## New transcript since last request\n[01:01] [s1] Them: We ship Friday\n\n\
             ## Transcript scope\nBase your answer on the whole meeting so far.\n\n\
             ## Task instructions\nRecap."
        );
        // Default scope: no section at all.
        let p = compose_delta(&[], &[(SCOPE_HEADER, "")], "Recap.", None, None);
        assert_eq!(p, "## Task instructions\nRecap.");
    }
}
