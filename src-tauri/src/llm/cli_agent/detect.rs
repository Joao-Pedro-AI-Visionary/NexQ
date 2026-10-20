//! Locate the `claude` binary and query version/login state (no model calls).

use super::command::{configure_command, sandbox_dir};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Find `claude.exe`: the native installer location first, then `where.exe`.
pub fn find_claude_binary() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let p = PathBuf::from(home).join(".local").join("bin").join("claude.exe");
        if p.is_file() {
            return Some(p);
        }
    }
    let mut cmd = std::process::Command::new("where.exe");
    cmd.arg("claude");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    first_exe_line(&String::from_utf8_lossy(&out.stdout))
}

fn first_exe_line(stdout: &str) -> Option<PathBuf> {
    stdout
        .lines()
        .map(str::trim)
        .find(|l| l.to_lowercase().ends_with(".exe"))
        .map(PathBuf::from)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeAuthStatus {
    pub logged_in: bool,
    pub auth_method: String,
    pub subscription_type: Option<String>,
    pub email: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAuth {
    #[serde(default)]
    logged_in: bool,
    #[serde(default)]
    auth_method: Option<String>,
    #[serde(default)]
    subscription_type: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

/// Parse the JSON printed by `claude auth status`.
pub fn parse_auth_status(json: &str) -> Result<ClaudeAuthStatus, String> {
    let raw: RawAuth =
        serde_json::from_str(json.trim()).map_err(|e| format!("invalid auth status JSON: {e}"))?;
    Ok(ClaudeAuthStatus {
        logged_in: raw.logged_in,
        auth_method: raw.auth_method.unwrap_or_default(),
        subscription_type: raw.subscription_type,
        email: raw.email,
    })
}

/// Run `claude --version` and `claude auth status` (10 s timeout each, no
/// console window, API-key env stripped). `auth status` exits 1 when logged
/// out but still prints JSON, so a non-zero exit is not an error by itself.
pub async fn claude_status(bin: &Path) -> Result<(String, ClaudeAuthStatus), String> {
    let version = run_probe(bin, &["--version"]).await?;
    let auth = run_probe(bin, &["auth", "status"]).await?;
    let status = parse_auth_status(&auth)?;
    Ok((version.trim().to_string(), status))
}

async fn run_probe(bin: &Path, args: &[&str]) -> Result<String, String> {
    let label = args.join(" ");
    let mut std_cmd = std::process::Command::new(bin);
    std_cmd.args(args);
    configure_command(&mut std_cmd, &sandbox_dir());
    let mut cmd = tokio::process::Command::from(std_cmd);
    cmd.kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
        .await
        .map_err(|_| format!("`claude {label}` timed out after 10 s"))?
        .map_err(|e| format!("failed to run `claude {label}`: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if stdout.trim().is_empty() && !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("`claude {label}` failed: {}", stderr.trim()));
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_status_parses_logged_in() {
        let s = parse_auth_status(
            r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max","email":"a@b"}"#,
        )
        .unwrap();
        assert!(s.logged_in);
        assert_eq!(s.auth_method, "claude.ai");
        assert_eq!(s.subscription_type.as_deref(), Some("max"));
        assert_eq!(s.email.as_deref(), Some("a@b"));
    }

    #[test]
    fn auth_status_parses_logged_out_and_rejects_garbage() {
        let s = parse_auth_status(r#"{"loggedIn":false}"#).unwrap();
        assert!(!s.logged_in);
        assert_eq!(s.subscription_type, None);
        assert!(parse_auth_status("nope").is_err());
    }

    #[test]
    fn where_output_picks_exe_line() {
        let out = "C:\\npm\\claude\r\nC:\\npm\\claude.cmd\r\nC:\\bin\\claude.exe\r\n";
        assert_eq!(first_exe_line(out), Some(PathBuf::from("C:\\bin\\claude.exe")));
        assert_eq!(first_exe_line("C:\\npm\\claude.cmd\r\n"), None);
    }
}
