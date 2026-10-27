#[cfg(target_os = "windows")]
mod windows_cred;

#[cfg(target_os = "macos")]
mod macos_keychain;

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
mod fallback_cred;

#[cfg(target_os = "windows")]
use self::windows_cred as backend;

#[cfg(target_os = "macos")]
use self::macos_keychain as backend;

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
use self::fallback_cred as backend;

/// Credential manager for secure API key storage.
/// Delegates to the platform-specific credential backend
/// (Windows Credential Manager, macOS Keychain, or a stub on other platforms).
pub struct CredentialManager;

impl Default for CredentialManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialManager {
    pub fn new() -> Self {
        Self
    }

    /// Store an API key for the given provider.
    pub fn store_key(&self, provider: &str, key: &str) -> Result<(), String> {
        let provider = Self::normalize_provider(provider);
        backend::credential_write(&provider, key)
    }

    /// Retrieve an API key for the given provider. Returns None if not stored.
    pub fn get_key(&self, provider: &str) -> Result<Option<String>, String> {
        let provider = Self::normalize_provider(provider);
        match backend::credential_read(&provider)? {
            Some(k) => Ok(Some(k)),
            None => Ok(Self::env_key(&provider)),
        }
    }

    /// Fallback: read the key from the environment when the credential store
    /// has none. `NEXQ_<PROVIDER>_API_KEY` always works; a few providers also
    /// accept their usual variable (GROQ_API_KEY, OPENAI_API_KEY, ...).
    fn env_key(provider: &str) -> Option<String> {
        let mut names = vec![format!("NEXQ_{}_API_KEY", provider.to_uppercase())];
        let common = match provider {
            "groq_whisper" | "groq" => Some("GROQ_API_KEY"),
            "openai" | "whisper_api" => Some("OPENAI_API_KEY"),
            "anthropic" => Some("ANTHROPIC_API_KEY"),
            "deepgram" => Some("DEEPGRAM_API_KEY"),
            "gemini" => Some("GEMINI_API_KEY"),
            _ => None,
        };
        if let Some(c) = common {
            names.push(c.to_string());
        }
        names
            .iter()
            .filter_map(|n| std::env::var(n).ok())
            .map(|v| v.trim().to_string())
            .find(|v| !v.is_empty())
    }

    /// Delete the API key for the given provider.
    pub fn delete_key(&self, provider: &str) -> Result<(), String> {
        let provider = Self::normalize_provider(provider);
        backend::credential_delete(&provider)
    }

    /// Check whether an API key exists for the given provider.
    pub fn has_key(&self, provider: &str) -> Result<bool, String> {
        Ok(self.get_key(provider)?.is_some())
    }

    /// Normalize provider name to lowercase for consistent key naming.
    fn normalize_provider(provider: &str) -> String {
        provider.to_lowercase().trim().to_string()
    }
}
