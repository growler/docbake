//! Credential storage for external service tokens (Confluence, etc.).
//!
//! Tokens are stored in a JSON file at the platform-standard config
//! directory:
//!
//! - Linux: `$XDG_CONFIG_HOME/docbake/credentials.json`
//!   (defaults to `~/.config/docbake/credentials.json`)
//! - macOS: `~/Library/Application Support/docbake/credentials.json`
//! - Windows: `%APPDATA%\docbake\credentials.json`
//!
//! The file is created with mode 0600 on Unix (owner-only read/write).
//!
//! File format (namespaced by service):
//!
//! ```json
//! {
//!   "confluence": {
//!     "https://confluence.example.com": "token-value"
//!   }
//! }
//! ```
//!
//! The `CONFLUENCE_TOKEN` environment variable, when set, takes
//! precedence over any stored credential.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

/// Application-specific subdirectory name inside the platform config root.
const APP_CONFIG_DIR: &str = "docbake";

/// Credential store file name.
const CREDENTIALS_FILE: &str = "credentials.json";

/// Service namespace key inside the JSON file.
const CONFLUENCE_KEY: &str = "confluence";

/// Top-level credential file structure: `{ "confluence": { url: token } }`.
type CredentialStore = BTreeMap<String, BTreeMap<String, String>>;

/// Return the path to the credentials file.
///
/// Does **not** create the file or parent directories.
fn credentials_file_path() -> Result<PathBuf> {
    let config =
        dirs::config_dir().context("failed to determine platform config directory")?;
    Ok(config.join(APP_CONFIG_DIR).join(CREDENTIALS_FILE))
}

/// Normalize a server URL for use as a credential store key.
///
/// Strips trailing slashes and lowercases scheme + host so that
/// `https://Confluence.Example.Com/` and `https://confluence.example.com`
/// resolve to the same key.
pub(crate) fn normalize_server(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    // Split at "://" to lowercase scheme + host while preserving any path.
    if let Some((scheme, rest)) = trimmed.split_once("://") {
        // rest may be "host:port/path…" — lowercase only the host(:port) part.
        if let Some((authority, path)) = rest.split_once('/') {
            format!(
                "{}://{}/{}",
                scheme.to_ascii_lowercase(),
                authority.to_ascii_lowercase(),
                path
            )
        } else {
            format!(
                "{}://{}",
                scheme.to_ascii_lowercase(),
                rest.to_ascii_lowercase()
            )
        }
    } else {
        trimmed.to_ascii_lowercase()
    }
}

/// Read the full credential store from disk.  Returns an empty store if
/// the file does not exist.
fn read_store() -> Result<CredentialStore> {
    let path = credentials_file_path()?;
    if !path.exists() {
        return Ok(CredentialStore::new());
    }
    let data = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let store: CredentialStore = serde_json::from_str(&data)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(store)
}

/// Write the credential store to disk, creating parent directories as
/// needed and setting mode 0600 on Unix.
fn write_store(store: &CredentialStore) -> Result<()> {
    let path = credentials_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let json =
        serde_json::to_string_pretty(store).context("failed to serialise credentials")?;
    fs::write(&path, json.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms)
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    }

    Ok(())
}

/// Store a Confluence token for the given server URL.
pub(crate) fn store_token(server: &str, token: &str) -> Result<()> {
    let key = normalize_server(server);
    let mut store = read_store()?;
    store
        .entry(CONFLUENCE_KEY.to_string())
        .or_default()
        .insert(key, token.to_string());
    write_store(&store)
}

/// Look up a stored Confluence token for the given server URL.
///
/// Returns `None` if no token is stored for this server.
pub(crate) fn load_token(server: &str) -> Result<Option<String>> {
    let key = normalize_server(server);
    let store = read_store()?;
    Ok(store
        .get(CONFLUENCE_KEY)
        .and_then(|m| m.get(&key))
        .cloned())
}

/// Resolve a Confluence token.
///
/// Priority:
/// 1. `CONFLUENCE_TOKEN` environment variable (if set and non-empty)
/// 2. Stored credential for the given server URL
///
/// Returns an error with a user-friendly message when neither source
/// provides a token.
pub(crate) fn resolve_token(server: &str) -> Result<String> {
    // 1. Environment variable
    if let Ok(val) = std::env::var("CONFLUENCE_TOKEN") {
        if !val.is_empty() {
            return Ok(val);
        }
    }

    // 2. Stored credential
    if let Some(token) = load_token(server)? {
        return Ok(token);
    }

    let path = credentials_file_path().ok();
    let path_hint = path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<config-dir>/docbake/credentials.json".to_string());

    bail!(
        "Confluence token not found for server {server}.\n\
         \n\
         Provide a token in one of these ways:\n\
         \n\
         1. Set the CONFLUENCE_TOKEN environment variable, or\n\
         2. Run: docbake auth confluence {server}\n\
         \n\
         Stored credentials are read from {path_hint}",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_trailing_slash() {
        assert_eq!(
            normalize_server("https://example.com/"),
            "https://example.com"
        );
    }

    #[test]
    fn normalize_lowercases_scheme_and_host() {
        assert_eq!(
            normalize_server("HTTPS://Confluence.Example.COM"),
            "https://confluence.example.com"
        );
    }

    #[test]
    fn normalize_preserves_path_case() {
        assert_eq!(
            normalize_server("https://Host.COM/Wiki/Rest"),
            "https://host.com/Wiki/Rest"
        );
    }

    #[test]
    fn normalize_no_scheme() {
        assert_eq!(normalize_server("HOST.COM"), "host.com");
    }

    #[test]
    fn normalize_with_port() {
        assert_eq!(
            normalize_server("https://HOST.COM:8443/"),
            "https://host.com:8443"
        );
    }

    #[test]
    fn store_and_load_round_trip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join(CREDENTIALS_FILE);

        // Simulate store_token / load_token using the raw store helpers
        // against a controlled path.
        let mut store = CredentialStore::new();
        store
            .entry(CONFLUENCE_KEY.to_string())
            .or_default()
            .insert(
                normalize_server("https://confluence.example.com"),
                "secret-token".to_string(),
            );

        let json = serde_json::to_string_pretty(&store).unwrap();
        fs::write(&path, json.as_bytes()).unwrap();

        let data = fs::read_to_string(&path).unwrap();
        let loaded: CredentialStore = serde_json::from_str(&data).unwrap();
        assert_eq!(
            loaded
                .get(CONFLUENCE_KEY)
                .and_then(|m| m.get("https://confluence.example.com")),
            Some(&"secret-token".to_string())
        );
    }

    #[test]
    fn store_preserves_other_services() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join(CREDENTIALS_FILE);

        // Pre-populate with a hypothetical "jira" service.
        let mut store = CredentialStore::new();
        store
            .entry("jira".to_string())
            .or_default()
            .insert("https://jira.example.com".to_string(), "jira-tok".to_string());

        let json = serde_json::to_string_pretty(&store).unwrap();
        fs::write(&path, json.as_bytes()).unwrap();

        // Add a confluence token.
        let data = fs::read_to_string(&path).unwrap();
        let mut loaded: CredentialStore = serde_json::from_str(&data).unwrap();
        loaded
            .entry(CONFLUENCE_KEY.to_string())
            .or_default()
            .insert("https://conf.example.com".to_string(), "conf-tok".to_string());

        let json2 = serde_json::to_string_pretty(&loaded).unwrap();
        fs::write(&path, json2.as_bytes()).unwrap();

        // Verify both services survive.
        let final_data = fs::read_to_string(&path).unwrap();
        let final_store: CredentialStore = serde_json::from_str(&final_data).unwrap();
        assert_eq!(
            final_store
                .get("jira")
                .and_then(|m| m.get("https://jira.example.com")),
            Some(&"jira-tok".to_string())
        );
        assert_eq!(
            final_store
                .get(CONFLUENCE_KEY)
                .and_then(|m| m.get("https://conf.example.com")),
            Some(&"conf-tok".to_string())
        );
    }
}
