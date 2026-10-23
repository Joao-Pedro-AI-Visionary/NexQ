//! macOS Keychain-backed credential storage.
//!
//! Uses the `security` CLI shipped with every macOS install, so no extra crate is
//! required. Key naming matches the Windows backend: generic-password items with
//! service `NexQ:{provider}` and account `{provider}`.

use std::process::{Command, Output};

const SECURITY_BIN: &str = "/usr/bin/security";
/// `security` exit code for `errSecItemNotFound`.
const ERR_SEC_ITEM_NOT_FOUND: i32 = 44;

fn service_name(provider: &str) -> String {
    format!("NexQ:{}", provider)
}

fn run(args: &[&str]) -> Result<Output, String> {
    Command::new(SECURITY_BIN)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run `security`: {}", e))
}

/// Write (or update) a credential.
pub fn credential_write(provider: &str, key: &str) -> Result<(), String> {
    let service = service_name(provider);
    // -U updates the item in place if it already exists.
    let out = run(&[
        "add-generic-password",
        "-U",
        "-a",
        provider,
        "-s",
        service.as_str(),
        "-w",
        key,
    ])?;
    if !out.status.success() {
        return Err(format!(
            "Keychain write failed for {}: {}",
            provider,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Read a credential. Returns None if it does not exist.
pub fn credential_read(provider: &str) -> Result<Option<String>, String> {
    let service = service_name(provider);
    let out = run(&[
        "find-generic-password",
        "-a",
        provider,
        "-s",
        service.as_str(),
        "-w",
    ])?;
    if out.status.success() {
        let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return Ok(if value.is_empty() { None } else { Some(value) });
    }
    if out.status.code() == Some(ERR_SEC_ITEM_NOT_FOUND) {
        return Ok(None);
    }
    Err(format!(
        "Keychain read failed for {}: {}",
        provider,
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

/// Delete a credential. Deleting a missing item is not an error.
pub fn credential_delete(provider: &str) -> Result<(), String> {
    let service = service_name(provider);
    let out = run(&[
        "delete-generic-password",
        "-a",
        provider,
        "-s",
        service.as_str(),
    ])?;
    if out.status.success() || out.status.code() == Some(ERR_SEC_ITEM_NOT_FOUND) {
        Ok(())
    } else {
        Err(format!(
            "Keychain delete failed for {}: {}",
            provider,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}
