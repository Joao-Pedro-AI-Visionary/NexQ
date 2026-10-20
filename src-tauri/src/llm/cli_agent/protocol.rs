//! Claude Code `stream-json` protocol: parsing stdout lines, building stdin
//! messages and classifying error text.

use serde_json::{json, Value};

/// Token/cost accounting and final text for one finished turn.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TurnResult {
    pub text: String,
    pub is_error: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
}

/// One parsed stdout line.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    Init { session_id: String, model: String },
    TextDelta(String),
    TurnComplete(TurnResult),
    /// Any event NexQ does not act on (status, assistant, user, other deltas...).
    Other,
}

/// Parse one stdout line of `--output-format stream-json`.
/// Valid JSON of an unrecognised shape and blank lines yield `Other`; invalid
/// (e.g. truncated) JSON is an error the caller should log and skip.
pub fn parse_line(line: &str) -> Result<AgentEvent, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(AgentEvent::Other);
    }
    let v: Value = serde_json::from_str(line)
        .map_err(|e| format!("invalid stream-json line: {e}"))?;
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    let sub = v.get("subtype").and_then(Value::as_str).unwrap_or("");
    match (kind, sub) {
        ("system", "init") => Ok(AgentEvent::Init {
            session_id: str_field(&v, "session_id"),
            model: str_field(&v, "model"),
        }),
        ("stream_event", _) => {
            let delta = &v["event"]["delta"];
            if v["event"]["type"] == "content_block_delta" && delta["type"] == "text_delta" {
                if let Some(t) = delta["text"].as_str() {
                    return Ok(AgentEvent::TextDelta(t.to_string()));
                }
            }
            Ok(AgentEvent::Other)
        }
        ("result", _) => {
            let usage = &v["usage"];
            Ok(AgentEvent::TurnComplete(TurnResult {
                text: result_text(&v),
                is_error: v["is_error"].as_bool().unwrap_or(false),
                input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
                cache_read_tokens: usage["cache_read_input_tokens"].as_u64().unwrap_or(0),
                cost_usd: v["total_cost_usd"].as_f64().unwrap_or(0.0),
                duration_ms: v["duration_ms"].as_u64().unwrap_or(0),
            }))
        }
        _ => Ok(AgentEvent::Other),
    }
}

/// `result` text, or for error results that carry none (e.g. subtypes
/// `error_during_execution`, `error_max_turns`) the joined `errors` array,
/// falling back to the subtype so the failure stays classifiable.
fn result_text(v: &Value) -> String {
    if let Some(t) = v.get("result").and_then(Value::as_str) {
        return t.to_string();
    }
    if !v["is_error"].as_bool().unwrap_or(false) {
        return String::new();
    }
    let errors: Vec<String> = v["errors"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| e.as_str().map(str::to_string).unwrap_or_else(|| e.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if errors.is_empty() {
        str_field(v, "subtype")
    } else {
        errors.join("; ")
    }
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Build one stdin JSON line (no trailing newline). With `query == false` the
/// message is appended to the session context without triggering a model call.
pub fn user_message(text: &str, query: bool) -> String {
    let mut msg = json!({
        "type": "user",
        "message": { "role": "user", "content": text },
        "parent_tool_use_id": null,
    });
    if !query {
        msg["shouldQuery"] = Value::Bool(false);
    }
    msg.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentErrorKind {
    NotInstalled,
    NotLoggedIn,
    RateLimited,
    /// The conversation no longer fits the model's context window.
    ContextOverflow,
    Crashed,
    Other,
}

impl AgentErrorKind {
    /// Stable snake_case name, mirrored by the frontend (`error_kind`).
    pub fn as_str(self) -> &'static str {
        match self {
            AgentErrorKind::NotInstalled => "not_installed",
            AgentErrorKind::NotLoggedIn => "not_logged_in",
            AgentErrorKind::RateLimited => "rate_limited",
            AgentErrorKind::ContextOverflow => "context_overflow",
            AgentErrorKind::Crashed => "crashed",
            AgentErrorKind::Other => "other",
        }
    }
}

/// Map error text (a failed result, stderr, or spawn error) to a coarse kind.
pub fn classify_error(text: &str) -> AgentErrorKind {
    let t = text.to_lowercase();
    if t.contains("not logged in") || t.contains("/login") || t.contains("please log in") {
        AgentErrorKind::NotLoggedIn
    } else if t.contains("rate limit")
        || t.contains("usage limit")
        || (t.contains("limit reached") && !t.contains("context"))
        || t.contains("too many requests")
        || t.contains("per minute")
    {
        // Before the context patterns: "too many tokens per minute" is a rate limit.
        AgentErrorKind::RateLimited
    } else if t.contains("prompt is too long")
        || t.contains("input is too long")
        || t.contains("context length")
        || t.contains("context window")
        || t.contains("context limit")
        || t.contains("maximum context")
        || t.contains("too many tokens")
    {
        AgentErrorKind::ContextOverflow
    } else if t.contains("not installed")
        || t.contains("program not found")
        || t.contains("cannot find the file")
        || t.contains("(os error 2)")
    {
        AgentErrorKind::NotInstalled
    } else if t.contains("exited") || t.contains("crashed") || t.contains("broken pipe") {
        AgentErrorKind::Crashed
    } else {
        AgentErrorKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_init() {
        let l = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-haiku-4-5-20251001","tools":[],"mcp_servers":[]}"#;
        assert_eq!(
            parse_line(l).unwrap(),
            AgentEvent::Init { session_id: "s".into(), model: "claude-haiku-4-5-20251001".into() }
        );
    }

    #[test]
    fn parse_text_delta() {
        let l = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}}"#;
        assert_eq!(parse_line(l).unwrap(), AgentEvent::TextDelta("Hel".into()));
    }

    #[test]
    fn parse_result() {
        let l = r#"{"type":"result","subtype":"success","is_error":false,"result":"OK","total_cost_usd":0.0012,"duration_ms":900,"usage":{"input_tokens":612,"output_tokens":20,"cache_read_input_tokens":0}}"#;
        assert_eq!(
            parse_line(l).unwrap(),
            AgentEvent::TurnComplete(TurnResult {
                text: "OK".into(),
                is_error: false,
                input_tokens: 612,
                output_tokens: 20,
                cache_read_tokens: 0,
                cost_usd: 0.0012,
                duration_ms: 900,
            })
        );
    }

    #[test]
    fn parse_unknown_is_other() {
        assert_eq!(parse_line(r#"{"type":"system","subtype":"status"}"#).unwrap(), AgentEvent::Other);
        assert_eq!(parse_line(r#"{"type":"assistant","message":{}}"#).unwrap(), AgentEvent::Other);
        let non_text = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta"}}}"#;
        assert_eq!(parse_line(non_text).unwrap(), AgentEvent::Other);
        assert!(parse_line("not json").is_err());
    }

    #[test]
    fn user_message_query_false_has_should_query() {
        let v: Value = serde_json::from_str(&user_message("x", false)).unwrap();
        assert_eq!(v["shouldQuery"], false);
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["content"], "x");
        assert!(!user_message("x", false).contains('\n'));
        let q: Value = serde_json::from_str(&user_message("x", true)).unwrap();
        assert!(q.get("shouldQuery").is_none());
    }

    #[test]
    fn parse_blank_whitespace_and_truncated() {
        assert_eq!(parse_line("").unwrap(), AgentEvent::Other);
        assert_eq!(parse_line("  
").unwrap(), AgentEvent::Other);
        assert!(parse_line(r#"{"type":"stream_event","event":{"type":"content_blo"#).is_err());
    }

    #[test]
    fn parse_error_result_without_result_field() {
        let l = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["API Error: Rate limit reached","retry later"]}"#;
        match parse_line(l).unwrap() {
            AgentEvent::TurnComplete(r) => {
                assert!(r.is_error);
                assert_eq!(r.text, "API Error: Rate limit reached; retry later");
                assert_eq!(classify_error(&r.text), AgentErrorKind::RateLimited);
            }
            other => panic!("unexpected {other:?}"),
        }
        let bare = r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#;
        match parse_line(bare).unwrap() {
            AgentEvent::TurnComplete(r) => assert_eq!(r.text, "error_max_turns"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn classify_os_error_2_precisely() {
        assert_eq!(
            classify_error("program not found: The system cannot find the file specified. (os error 2)"),
            AgentErrorKind::NotInstalled
        );
        assert_eq!(classify_error("The directory name is invalid. (os error 267)"), AgentErrorKind::Other);
        assert_eq!(classify_error("Access is denied. (os error 5)"), AgentErrorKind::Other);
        assert_eq!(classify_error("pipe error (os error 232)"), AgentErrorKind::Other);
    }

    #[test]
    fn classify_not_logged_in() {
        assert_eq!(classify_error("Not logged in · Please run /login"), AgentErrorKind::NotLoggedIn);
    }

    #[test]
    fn error_kind_names_are_snake_case() {
        assert_eq!(AgentErrorKind::RateLimited.as_str(), "rate_limited");
        assert_eq!(AgentErrorKind::NotLoggedIn.as_str(), "not_logged_in");
        assert_eq!(AgentErrorKind::NotInstalled.as_str(), "not_installed");
        assert_eq!(AgentErrorKind::ContextOverflow.as_str(), "context_overflow");
        assert_eq!(AgentErrorKind::Crashed.as_str(), "crashed");
        assert_eq!(AgentErrorKind::Other.as_str(), "other");
    }

    #[test]
    fn classify_rate_limit() {
        assert_eq!(classify_error("You hit the Rate Limit"), AgentErrorKind::RateLimited);
        assert_eq!(classify_error("5-hour usage limit reached"), AgentErrorKind::RateLimited);
        assert_eq!(classify_error("boom"), AgentErrorKind::Other);
    }

    #[test]
    fn classify_context_overflow() {
        for text in [
            "Prompt is too long",
            "input length and `max_tokens` exceed context limit: 190000 + 32000 > 200000",
            "This model's maximum context length is 200000 tokens",
            "Context window exceeded",
            "Context limit reached",
        ] {
            assert_eq!(classify_error(text), AgentErrorKind::ContextOverflow, "{text}");
        }
        for text in [
            "Too many tokens per minute",
            "rate limit exceeded: too many tokens",
            "429 Too Many Requests",
        ] {
            assert_eq!(classify_error(text), AgentErrorKind::RateLimited, "{text}");
        }
    }
}
