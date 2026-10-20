//! Test double for `claude.exe` speaking the `stream-json` protocol, used by
//! `tests/claude_session.rs`. It accepts (and ignores) the real launch args.
//!
//! Configuration travels inside the system prompt file (`--system-prompt-file`),
//! one `key=value` directive per line, because the session strips/controls the
//! child environment and parallel tests cannot share process env vars:
//! - `fake_mode=normal|slow|crash_mid_turn|hang` (default `normal`)
//! - `fake_record=<path>`: every stdin line received is appended to this file.
//!
//! Behaviour:
//! - Startup: a blank line and a truncated JSON line (the reader must skip
//!   both). Like the real CLI, `system/init` is printed only once a stdin
//!   message arrives, and again before every turn. A missing system prompt file
//!   exits with code 3.
//! - `shouldQuery:false` user message: an empty `result` at once, no deltas.
//! - Query `hello`: deltas `Hello`, ` world`. Query `fail:<msg>`: an error
//!   result with text `<msg>`. Any other query `q`: `answer:<q>` in 3-char deltas.
//! - `slow`: one delta per character, 100 ms apart.
//! - `crash_mid_turn`: exits with code 1 right after the first delta.
//! - `hang`: after the first delta, sleeps forever without exiting or
//!   printing anything (a wedged CLI).
//! - stdin EOF: exits 0.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::time::Duration;

fn emit(out: &mut impl Write, v: &Value) {
    writeln!(out, "{v}").unwrap();
    out.flush().unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg_after = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let model = arg_after("--model").unwrap_or_default();
    let prompt_path = arg_after("--system-prompt-file").unwrap_or_default();
    let prompt = match std::fs::read_to_string(&prompt_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("fake_claude: cannot read system prompt file {prompt_path:?}: {e}");
            std::process::exit(3);
        }
    };
    let directive = |key: &str| {
        prompt.lines().find_map(|l| {
            l.trim()
                .strip_prefix(&format!("{key}="))
                .map(str::to_string)
        })
    };
    let mode = directive("fake_mode").unwrap_or_else(|| "normal".into());
    let record = directive("fake_record");

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out).unwrap();
    writeln!(out, r#"{{"type":"system","subtype":"ini"#).unwrap();
    out.flush().unwrap();
    let init = json!({"type":"system","subtype":"init","session_id":"fake-session","model":model,"tools":[],"mcp_servers":[]});
    eprintln!("fake_claude: ready (mode={mode})");

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if let Some(path) = &record {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(f, "{line}").unwrap();
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if msg["type"] != "user" {
            continue;
        }
        emit(&mut out, &init);
        if msg["shouldQuery"] == false {
            emit(
                &mut out,
                &json!({"type":"result","subtype":"success","is_error":false,"result":"","usage":{"input_tokens":0,"output_tokens":0}}),
            );
            continue;
        }
        let content = msg["message"]["content"].as_str().unwrap_or("").to_string();
        if let Some(err) = content.strip_prefix("fail:") {
            emit(
                &mut out,
                &json!({"type":"result","subtype":"success","is_error":true,"result":err}),
            );
            continue;
        }
        let answer = if content == "hello" {
            "Hello world".to_string()
        } else {
            format!("answer:{content}")
        };
        let chars: Vec<char> = answer.chars().collect();
        let deltas: Vec<String> = if content == "hello" {
            vec!["Hello".into(), " world".into()]
        } else if mode == "slow" {
            chars.iter().map(|c| c.to_string()).collect()
        } else {
            chars.chunks(3).map(|c| c.iter().collect()).collect()
        };
        emit(
            &mut out,
            &json!({"type":"system","subtype":"status","status":"requesting"}),
        );
        emit(
            &mut out,
            &json!({"type":"stream_event","event":{"type":"message_start"}}),
        );
        for d in deltas {
            emit(
                &mut out,
                &json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":d}}}),
            );
            if mode == "crash_mid_turn" {
                eprintln!("fake_claude: crashing mid-turn");
                std::process::exit(1);
            }
            if mode == "hang" {
                eprintln!("fake_claude: hanging mid-turn");
                loop {
                    std::thread::sleep(Duration::from_secs(3600));
                }
            }
            if mode == "slow" {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        emit(
            &mut out,
            &json!({"type":"assistant","message":{"content":[{"type":"text","text":answer}]}}),
        );
        emit(
            &mut out,
            &json!({"type":"result","subtype":"success","is_error":false,"result":answer,"total_cost_usd":0.0001,"duration_ms":5,"usage":{"input_tokens":10,"output_tokens":chars.len()}}),
        );
    }
}
