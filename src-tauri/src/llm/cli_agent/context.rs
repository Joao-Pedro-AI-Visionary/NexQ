//! Pure builders for session-aware Claude CLI requests: the per-session delta
//! cursor, the preload messages a fresh session is given (`shouldQuery:false`)
//! and the delta prompt each AI task sends.

use std::collections::HashSet;

/// One final transcript segment. `id` is the segment's stable id (the
/// transcript store / database id). `timestamp_ms` is milliseconds since the
/// meeting started (callers convert from wall-clock time), shown as `[mm:ss]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Seg {
    pub id: String,
    pub text: String,
    pub speaker: String,
    pub timestamp_ms: u64,
}

/// Which final segments (by id) a session has already been sent. Identity,
/// not position: an interim line that finalizes after later lines were sent,
/// or a different (e.g. shorter) list of the same meeting, never makes a
/// segment go twice or be skipped.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeltaCursor {
    sent: HashSet<String>,
}

impl DeltaCursor {
    /// A cursor that has already sent `ids`.
    pub fn with_sent(ids: HashSet<String>) -> Self {
        Self { sent: ids }
    }

    /// Every segment of `finals` not sent yet (each id once), oldest first,
    /// marking them sent.
    pub fn take_new(&mut self, finals: &[Seg]) -> Vec<Seg> {
        let mut new: Vec<Seg> = finals
            .iter()
            .filter(|s| self.sent.insert(s.id.clone()))
            .cloned()
            .collect();
        new.sort_by_key(|s| s.timestamp_ms);
        new
    }

    /// Number of segments already sent.
    pub fn sent(&self) -> usize {
        self.sent.len()
    }

    /// Ids of the segments already sent.
    pub fn sent_ids(&self) -> &HashSet<String> {
        &self.sent
    }
}

const SUMMARY_HEADER: &str = "## Meeting summary";
const TRANSCRIPT_HEADER: &str = "## Meeting transcript";
const DOCUMENTS_HEADER: &str = "## Context documents";
/// Room kept in every chunk for " (part N of M)" and the newline after the header.
const PART_SUFFIX_RESERVE: usize = 32;

/// Build the context messages for a fresh session: the summary (if any) and
/// the transcript as `[mm:ss] Speaker: text` lines (`[mm:ss] [id] Speaker:
/// text` with `with_ids`, so answers can cite segments), each message at
/// most `max_chars` characters (headers included). Empty input gives no
/// messages.
pub fn build_preload_messages(
    transcript: &[Seg],
    summary: Option<&str>,
    max_chars: usize,
    with_ids: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(summary) = summary.map(str::trim).filter(|s| !s.is_empty()) {
        push_chunked(
            &mut out,
            SUMMARY_HEADER,
            summary.lines().map(str::to_string),
            max_chars,
        );
    }
    if !transcript.is_empty() {
        push_chunked(
            &mut out,
            TRANSCRIPT_HEADER,
            transcript.iter().map(|seg| {
                if with_ids && !seg.id.is_empty() {
                    format_line_with_id(seg)
                } else {
                    format_line(seg)
                }
            }),
            max_chars,
        );
    }
    out
}

/// Context documents as preload messages of at most `max_chars` characters
/// each, under a `## Context documents` header. Blank text gives none.
pub fn build_document_preload(documents: &str, max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    if !documents.trim().is_empty() {
        push_chunked(
            &mut out,
            DOCUMENTS_HEADER,
            documents.trim().lines().map(str::to_string),
            max_chars,
        );
    }
    out
}

/// Pack `messages` (in order) into as few messages as possible, joined by a
/// blank line, each at most `max_chars` characters unless a single message
/// is already longer (it is then kept whole).
pub fn pack_messages(messages: &[String], max_chars: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for message in messages {
        let len = message.chars().count();
        if !current.is_empty() && current_len + 2 + len > max_chars {
            out.push(std::mem::take(&mut current));
            current_len = 0;
        }
        if !current.is_empty() {
            current.push_str("\n\n");
            current_len += 2;
        }
        current.push_str(message);
        current_len += len;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// The user message for one AI task on a session that already holds the
/// meeting: only the new transcript (section omitted when empty), the task
/// instructions and the question(s). Blank optional parts are left out.
pub fn build_delta_prompt(
    new_segments: &[Seg],
    task_instructions: &str,
    question: Option<&str>,
    detected_question: Option<&str>,
) -> String {
    let mut sections = Vec::new();
    if !new_segments.is_empty() {
        let lines: Vec<String> = new_segments.iter().map(format_line).collect();
        sections.push(format!(
            "## New transcript since last request\n{}",
            lines.join("\n")
        ));
    }
    for (header, body) in [
        ("## Task instructions", Some(task_instructions)),
        ("## Question", question),
        ("## Detected question", detected_question),
    ] {
        if let Some(body) = body.map(str::trim).filter(|b| !b.is_empty()) {
            sections.push(format!("{header}\n{body}"));
        }
    }
    sections.join("\n\n")
}

/// `[mm:ss] Speaker: text` on one line (minutes keep counting past 59).
fn format_line(seg: &Seg) -> String {
    let secs = seg.timestamp_ms / 1000;
    let text = seg.text.trim().replace(['\r', '\n'], " ");
    format!(
        "[{:02}:{:02}] {}: {}",
        secs / 60,
        secs % 60,
        seg.speaker,
        text
    )
}

/// Transcript lines, one per segment: `[mm:ss] Speaker: text`, or
/// `[mm:ss] [id] Speaker: text` with `with_ids` (segments without an id
/// keep the short form).
pub fn transcript_lines(segments: &[Seg], with_ids: bool) -> String {
    segments
        .iter()
        .map(|seg| {
            if with_ids && !seg.id.is_empty() {
                format_line_with_id(seg)
            } else {
                format_line(seg)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `[mm:ss] [id] Speaker: text` on one line.
fn format_line_with_id(seg: &Seg) -> String {
    let line = format_line(seg);
    match line.split_once("] ") {
        Some((time, rest)) => format!("{time}] [{}] {rest}", seg.id),
        None => line,
    }
}

/// Pack `lines` into messages of at most `max_chars` characters, each starting
/// with `header` (plus "(part i of n)" when there are several). A line longer
/// than the room left after the header is split. `max_chars` must exceed the
/// header plus its part suffix (callers use ~100k).
fn push_chunked(
    out: &mut Vec<String>,
    header: &str,
    lines: impl Iterator<Item = String>,
    max_chars: usize,
) {
    let reserve = header.chars().count() + PART_SUFFIX_RESERVE;
    debug_assert!(
        max_chars > reserve,
        "max_chars {max_chars} leaves no room after the {reserve}-char header"
    );
    let budget = max_chars.saturating_sub(reserve).max(1);
    let mut bodies: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    let mut started = false;
    for line in lines {
        for piece in split_to_fit(&line, budget) {
            let piece_len = piece.chars().count();
            if started && current_len + 1 + piece_len > budget {
                bodies.push(std::mem::take(&mut current));
                current_len = 0;
                started = false;
            }
            if started {
                current.push('\n');
                current_len += 1;
            }
            current.push_str(&piece);
            current_len += piece_len;
            started = true;
        }
    }
    if started {
        bodies.push(current);
    }
    let n = bodies.len();
    for (i, body) in bodies.into_iter().enumerate() {
        let head = if n == 1 {
            header.to_string()
        } else {
            format!("{header} (part {} of {n})", i + 1)
        };
        out.push(format!("{head}\n{body}"));
    }
}

/// `line` as pieces of at most `budget` characters.
fn split_to_fit(line: &str, budget: usize) -> Vec<String> {
    if line.chars().count() <= budget {
        return vec![line.to_string()];
    }
    let chars: Vec<char> = line.chars().collect();
    chars.chunks(budget).map(|c| c.iter().collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, speaker: &str, timestamp_ms: u64) -> Seg {
        Seg {
            id: format!("{speaker}-{timestamp_ms}"),
            text: text.into(),
            speaker: speaker.into(),
            timestamp_ms,
        }
    }

    #[test]
    fn cursor_sends_only_new() {
        let (a, b, c) = (seg("a", "You", 0), seg("b", "Them", 1), seg("c", "You", 2));
        let mut cursor = DeltaCursor::default();
        assert_eq!(cursor.take_new(&[a.clone(), b.clone()]), vec![a.clone(), b.clone()]);
        assert_eq!(cursor.take_new(&[a.clone(), b.clone(), c.clone()]), vec![c.clone()]);
        assert_eq!(cursor.sent(), 3);
        assert!(cursor.take_new(&[a, b, c]).is_empty());
    }

    #[test]
    fn cursor_sends_a_late_finalized_line_exactly_once() {
        // "Them" asks a question; its interim finalizes in place *after* two
        // later "You" lines were already final and sent.
        let you1 = seg("Sure", "You", 5_000);
        let you2 = seg("Go ahead", "You", 7_000);
        let question = seg("What is your notice period?", "Them", 4_000);
        let mut cursor = DeltaCursor::default();
        assert_eq!(cursor.take_new(&[you1.clone(), you2.clone()]), vec![you1.clone(), you2.clone()]);
        // The store keeps the question at its original position, before the sent lines.
        let finals = vec![question.clone(), you1.clone(), you2.clone()];
        assert_eq!(cursor.take_new(&finals), vec![question.clone()]);
        let later = seg("Two weeks", "You", 9_000);
        let finals = vec![question, you1, you2, later.clone()];
        assert_eq!(cursor.take_new(&finals), vec![later], "nothing duplicated or skipped");
    }

    #[test]
    fn cursor_shorter_different_list_resends_nothing() {
        let (a, b, c) = (seg("a", "You", 0), seg("b", "Them", 1), seg("c", "You", 2));
        let mut cursor = DeltaCursor::default();
        cursor.take_new(&[a.clone(), b.clone(), c.clone()]);
        // e.g. the meeting view sends its own (merged, shorter) copy of the transcript.
        assert!(cursor.take_new(&[a, c]).is_empty());
        assert_eq!(cursor.sent(), 3);
    }

    #[test]
    fn cursor_new_segments_come_oldest_first_and_once() {
        let (a, b) = (seg("a", "You", 2_000), seg("b", "Them", 1_000));
        let mut cursor = DeltaCursor::default();
        assert_eq!(cursor.take_new(&[a.clone(), b.clone(), a.clone()]), vec![b, a]);
    }

    #[test]
    fn preload_empty() {
        assert_eq!(
            build_preload_messages(&[], None, 100_000, false),
            Vec::<String>::new()
        );
        assert_eq!(
            build_preload_messages(&[], Some("  \n "), 100_000, false),
            Vec::<String>::new()
        );
    }

    #[test]
    fn preload_labels_speaker_and_time() {
        let msgs = build_preload_messages(
            &[
                seg("Hello there", "You", 65_400),
                seg("Hi\nagain", "Them", 3_725_000),
            ],
            Some("Short summary."),
            100_000,
            false,
        );
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], "## Meeting summary\nShort summary.");
        assert_eq!(
            msgs[1],
            "## Meeting transcript\n[01:05] You: Hello there\n[62:05] Them: Hi again"
        );
    }

    #[test]
    fn transcript_lines_with_and_without_ids() {
        let segs = [seg("Hi", "You", 1_000), Seg { id: String::new(), ..seg("Yo", "Them", 2_000) }];
        assert_eq!(transcript_lines(&segs, false), "[00:01] You: Hi\n[00:02] Them: Yo");
        assert_eq!(transcript_lines(&segs, true), "[00:01] [You-1000] You: Hi\n[00:02] Them: Yo");
    }

    #[test]
    fn preload_with_ids_cites_segments() {
        let msgs = build_preload_messages(&[seg("Hello", "You", 65_400)], None, 100_000, true);
        assert_eq!(msgs, vec!["## Meeting transcript\n[01:05] [You-65400] You: Hello"]);
    }

    #[test]
    fn preload_chunks_long_transcript() {
        let segs: Vec<Seg> = (0..30_000u64)
            .map(|i| {
                let text = format!("segment number {i:06} with filler text..");
                assert_eq!(text.len(), 40);
                seg(&text, if i % 2 == 0 { "You" } else { "Them" }, i * 1_000)
            })
            .collect();
        let msgs = build_preload_messages(&segs, None, 100_000, false);
        assert!(
            msgs.len() > 1,
            "expected several chunks, got {}",
            msgs.len()
        );
        for (i, m) in msgs.iter().enumerate() {
            let n = m.chars().count();
            assert!(n <= 100_000, "chunk {i} has {n} chars");
            assert!(m.starts_with(&format!(
                "## Meeting transcript (part {} of {})\n",
                i + 1,
                msgs.len()
            )));
        }
        let lines: Vec<&str> = msgs.iter().flat_map(|m| m.lines().skip(1)).collect();
        assert_eq!(lines.len(), segs.len());
        for (line, s) in lines.iter().zip(&segs) {
            let secs = s.timestamp_ms / 1000;
            let expected = format!(
                "[{:02}:{:02}] {}: {}",
                secs / 60,
                secs % 60,
                s.speaker,
                s.text
            );
            assert_eq!(*line, expected);
        }
    }

    #[test]
    fn preload_splits_a_line_longer_than_the_cap() {
        let long = "y".repeat(450);
        let msgs = build_preload_messages(&[seg(&long, "You", 0)], None, 200, false);
        assert!(msgs.len() > 1);
        assert!(msgs.iter().all(|m| m.chars().count() <= 200));
        let body: String = msgs.iter().flat_map(|m| m.lines().skip(1)).collect();
        assert_eq!(body, format!("[00:00] You: {long}"));
    }

    #[test]
    fn document_preload_chunks_under_header() {
        assert!(build_document_preload(" 
 ", 1_000).is_empty());
        let docs = (0..50).map(|i| format!("line {i:02} of the doc")).collect::<Vec<_>>().join("
");
        let msgs = build_document_preload(&docs, 300);
        assert!(msgs.len() > 1);
        assert!(msgs.iter().all(|m| m.chars().count() <= 300));
        assert!(msgs[0].starts_with("## Context documents (part 1 of "));
        let body: Vec<&str> = msgs.iter().flat_map(|m| m.lines().skip(1)).collect();
        assert_eq!(body.join("
"), docs);
    }

    #[test]
    fn pack_messages_joins_up_to_the_cap() {
        let m = |s: &str| s.to_string();
        assert!(pack_messages(&[], 10).is_empty());
        assert_eq!(pack_messages(&[m("aaa"), m("bbb"), m("ccc")], 8), vec!["aaa

bbb", "ccc"]);
        // A message longer than the cap is kept whole.
        assert_eq!(pack_messages(&[m("a"), m("0123456789"), m("b")], 5), vec!["a", "0123456789", "b"]);
    }

    #[test]
    fn delta_prompt_omits_empty_transcript_section() {
        let p = build_delta_prompt(&[], "Answer briefly.", Some("What next?"), None);
        assert!(!p.contains("## New transcript since last request"));
        assert_eq!(
            p,
            "## Task instructions\nAnswer briefly.\n\n## Question\nWhat next?"
        );
    }

    #[test]
    fn delta_prompt_has_all_sections_in_order() {
        let p = build_delta_prompt(
            &[seg("We ship Friday", "Them", 61_000)],
            "Suggest a reply.",
            Some("  "),
            Some("When do we ship?"),
        );
        assert_eq!(
            p,
            "## New transcript since last request\n[01:01] Them: We ship Friday\n\n\
             ## Task instructions\nSuggest a reply.\n\n\
             ## Detected question\nWhen do we ship?"
        );
    }
}
