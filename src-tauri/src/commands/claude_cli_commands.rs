//! Claude CLI (subscription) provider: detection, model settings, session
//! status and the login terminal.

use serde::{Deserialize, Serialize};
use tauri::{command, State};

use crate::llm::cli_agent::detect::{claude_status, find_claude_binary, ClaudeAuthStatus};
use crate::llm::cli_agent::manager::{AgentSessionStatus, ClaudeCliConfig};
use crate::state::AppState;

/// Effort levels the CLI accepts for `--effort`.
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Installation and login state of the local `claude` CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCliStatus {
    pub installed: bool,
    pub binary_path: Option<String>,
    pub version: Option<String>,
    /// `None` when not installed or the `auth status` probe failed.
    pub auth: Option<ClaudeAuthStatus>,
}

async fn resolve_binary() -> Option<std::path::PathBuf> {
    tokio::task::spawn_blocking(find_claude_binary)
        .await
        .ok()
        .flatten()
}

/// Detect `claude.exe`, its version and login state (no model call).
#[command]
pub async fn claude_cli_status() -> Result<ClaudeCliStatus, String> {
    let Some(bin) = resolve_binary().await else {
        return Ok(ClaudeCliStatus {
            installed: false,
            binary_path: None,
            version: None,
            auth: None,
        });
    };
    let (version, auth) = match claude_status(&bin).await {
        Ok((version, auth)) => (Some(version), Some(auth)),
        Err(e) => {
            log::warn!("Claude CLI status probe failed: {e}");
            (None, None)
        }
    };
    Ok(ClaudeCliStatus {
        installed: true,
        binary_path: Some(bin.to_string_lossy().into_owned()),
        version,
        auth,
    })
}

/// A model alias or id (`haiku`, `claude-haiku-4-5-20251001`, `opus[1m]`).
/// Rejecting a leading `-` keeps it from being read as a CLI flag.
fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && !model.starts_with('-')
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._:[]".contains(c))
}

fn validate_config(config: &ClaudeCliConfig) -> Result<(), String> {
    for (tier, model, effort) in [
        ("Fast", &config.fast_model, &config.fast_effort),
        ("Deep", &config.deep_model, &config.deep_effort),
    ] {
        if !valid_model(model) {
            return Err(format!("Invalid {tier} model: {model:?}"));
        }
        if !EFFORTS.contains(&effort.as_str()) {
            return Err(format!("Invalid {tier} effort: {effort:?}"));
        }
    }
    Ok(())
}

/// Replace the Claude CLI model settings. Running sessions keep their model;
/// the change applies to sessions started afterwards.
#[command]
pub async fn set_claude_cli_config(
    config: ClaudeCliConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    validate_config(&config)?;
    let mut current = state
        .claude_cli_config
        .write()
        .map_err(|_| "Claude CLI config lock poisoned".to_string())?;
    *current = config;
    Ok(())
}

/// Status of every open Claude session.
#[command]
pub async fn get_agent_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<AgentSessionStatus>, String> {
    Ok(state.agent_sessions.snapshot())
}

/// The command line handed to `cmd.exe`: `start` opens a new console window
/// running `claude` (kept open by `/k` so the user sees the result).
fn login_command_line(claude: &str) -> String {
    format!(r#"/d /c start "Claude login" cmd.exe /k "{claude}""#)
}

/// Open a visible terminal running `claude` so the user can log in. NexQ
/// reads nothing from it; credentials stay between the user and the CLI.
#[command]
pub async fn open_claude_login_terminal() -> Result<(), String> {
    let claude = resolve_binary()
        .await
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "claude".to_string());
    let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
    let mut cmd = std::process::Command::new(comspec);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Exact quoting for cmd.exe; this outer cmd has no window of its own
        // (CREATE_NO_WINDOW), `start` gives `claude` a fresh visible console.
        cmd.raw_arg(login_command_line(&claude))
            .creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    cmd.arg(login_command_line(&claude));
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open a terminal: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_cli_config_validation() {
        assert!(validate_config(&ClaudeCliConfig::default()).is_ok());
        let full_id = ClaudeCliConfig {
            deep_model: "claude-opus-4-1[1m]".into(),
            deep_effort: "xhigh".into(),
            ..Default::default()
        };
        assert!(validate_config(&full_id).is_ok());
        for bad in [
            ClaudeCliConfig { fast_model: "".into(), ..Default::default() },
            ClaudeCliConfig { fast_model: "--bare".into(), ..Default::default() },
            ClaudeCliConfig { deep_model: "son net".into(), ..Default::default() },
            ClaudeCliConfig { fast_effort: "ultra".into(), ..Default::default() },
        ] {
            assert!(validate_config(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn claude_cli_login_command_line_quotes_path() {
        assert_eq!(
            login_command_line(r"C:\Users\A B\.local\bin\claude.exe"),
            r#"/d /c start "Claude login" cmd.exe /k "C:\Users\A B\.local\bin\claude.exe""#
        );
    }

    #[test]
    fn claude_cli_status_serializes_snake_case() {
        let status = ClaudeCliStatus {
            installed: true,
            binary_path: Some("c".into()),
            version: Some("2.1".into()),
            auth: Some(ClaudeAuthStatus {
                logged_in: true,
                auth_method: "claude.ai".into(),
                subscription_type: Some("max".into()),
                email: None,
            }),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["binary_path"], "c");
        assert_eq!(json["auth"]["logged_in"], true);
        assert_eq!(json["auth"]["subscription_type"], "max");
    }
}
