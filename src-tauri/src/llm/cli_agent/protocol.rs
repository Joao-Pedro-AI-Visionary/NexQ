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
/// Valid JSON of an unrecognised shape yields `Other`; invalid JSON is an error.
pub fn parse_line(line: &str) -> Result<AgentEvent, String> {
    let v: Value = serde_json::from_str(line.trim())
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
                text: str_field(&v, "result"),
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
    Crashed,
    Other,
}

/// Map error text (a failed result, stderr, or spawn error) to a coarse kind.
pub fn classify_error(text: &str) -> AgentErrorKind {
    let t = text.to_lowercase();
    if t.contains("not logged in") || t.contains("/login") || t.contains("please log in") {
        AgentErrorKind::NotLoggedIn
    } else if t.contains("rate limit")
        || t.contains("usage limit")
        || t.contains("limit reached")
        || t.contains("too many requests")
    {
        AgentErrorKind::RateLimited
    } else if t.contains("not installed")
        || t.contains("program not found")
        || t.contains("cannot find the file")
        || t.contains("os error 2")
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
    fn classify_not_logged_in() {
        assert_eq!(classify_error("Not logged in · Please run /login"), AgentErrorKind::NotLoggedIn);
    }

    #[test]
    fn classify_rate_limit() {
        assert_eq!(classify_error("You hit the Rate Limit"), AgentErrorKind::RateLimited);
        assert_eq!(classify_error("5-hour usage limit reached"), AgentErrorKind::RateLimited);
        assert_eq!(classify_error("boom"), AgentErrorKind::Other);
    }
}
