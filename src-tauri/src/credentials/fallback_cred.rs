//! Credential backend stub for platforms without a native secret store.
//! NexQ officially targets Windows (Credential Manager) and macOS (Keychain).

const UNSUPPORTED: &str = "No credential backend is available on this platform";

pub fn credential_write(_provider: &str, _key: &str) -> Result<(), String> {
    Err(UNSUPPORTED.to_string())
}

pub fn credential_read(_provider: &str) -> Result<Option<String>, String> {
    Err(UNSUPPORTED.to_string())
}

pub fn credential_delete(_provider: &str) -> Result<(), String> {
    Err(UNSUPPORTED.to_string())
}
