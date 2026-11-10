//! The Soniox key. The one saved in the Keychain wins, then `SONIOX_API_KEY`,
//! then `fnox get SONIOX_API_KEY`. Each take reads `current()`, so a key saved
//! from the Soniox Key window is used without a relaunch.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::RwLock;
use std::time::Duration;

use security_framework::passwords::{
    delete_generic_password, generic_password, set_generic_password, PasswordOptions,
};

use crate::storage;

const SERVICE: &str = "com.pupko.iki";
const ACCOUNT: &str = "soniox_api_key";
/// Plaintext file earlier builds read. Imported into the Keychain once.
const LEGACY_FILE: &str = "soniox_api_key";
/// `GET /v1/models` needs a valid key and has no side effects.
/// https://docs.soniox.com/stt/api-reference/models/get_models
const VERIFY_URL: &str = "https://api.soniox.com/v1/models";
/// Keychain's `errSecItemNotFound`.
const ITEM_NOT_FOUND: i32 = -25300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Keychain,
    Environment,
    Fnox,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Resolved {
    key: String,
    source: Source,
}

static CURRENT: RwLock<Option<Resolved>> = RwLock::new(None);

/// Imports the legacy file, then picks the key. Call once at launch.
pub fn load() {
    let saved = match keychain_read() {
        Some(key) => Some(key),
        None => import_legacy_file(&storage::app_support_file(LEGACY_FILE), |key| {
            keychain_write(key).is_ok()
        }),
    };
    set_current(resolve(saved, env_key, fnox_key));
}

/// The key the next take uses.
pub fn current() -> Option<String> {
    read_current().as_ref().map(|resolved| resolved.key.clone())
}

pub fn current_source() -> Option<Source> {
    read_current().as_ref().map(|resolved| resolved.source)
}

/// Saves `key` in the Keychain and uses it from the next take on.
pub fn save(key: &str) -> Result<(), String> {
    keychain_write(key)?;
    set_current(Some(Resolved {
        key: key.to_string(),
        source: Source::Keychain,
    }));
    Ok(())
}

/// Deletes the saved key, then falls back to the environment or fnox.
pub fn remove() -> Result<(), String> {
    match delete_generic_password(SERVICE, ACCOUNT) {
        Ok(()) => {}
        Err(error) if error.code() == ITEM_NOT_FOUND => {}
        Err(error) => return Err(keychain_message(error)),
    }
    set_current(resolve(None, env_key, fnox_key));
    Ok(())
}

fn read_current() -> std::sync::RwLockReadGuard<'static, Option<Resolved>> {
    CURRENT
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn set_current(resolved: Option<Resolved>) {
    *CURRENT
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = resolved;
}

/// The saved key first. The environment and fnox are only asked when needed,
/// since fnox runs a process.
fn resolve(
    keychain: Option<String>,
    env: impl FnOnce() -> Option<String>,
    fnox: impl FnOnce() -> Option<String>,
) -> Option<Resolved> {
    if let Some(key) = non_empty(keychain) {
        return Some(Resolved {
            key,
            source: Source::Keychain,
        });
    }
    if let Some(key) = non_empty(env()) {
        return Some(Resolved {
            key,
            source: Source::Environment,
        });
    }
    non_empty(fnox()).map(|key| Resolved {
        key,
        source: Source::Fnox,
    })
}

fn non_empty(key: Option<String>) -> Option<String> {
    let key = key?.trim().to_string();
    (!key.is_empty()).then_some(key)
}

/// Moves the key from the legacy plaintext file into the Keychain. The file is
/// deleted only once `store` succeeds. Returns the imported key.
fn import_legacy_file(path: &Path, store: impl FnOnce(&str) -> bool) -> Option<String> {
    let key = non_empty(std::fs::read_to_string(path).ok())?;
    if !store(&key) {
        return None;
    }
    if let Err(error) = std::fs::remove_file(path) {
        eprintln!("iki: {}: {error}", path.display());
    }
    Some(key)
}

fn keychain_read() -> Option<String> {
    match generic_password(PasswordOptions::new_generic_password(SERVICE, ACCOUNT)) {
        Ok(bytes) => non_empty(String::from_utf8(bytes).ok()),
        Err(error) if error.code() == ITEM_NOT_FOUND => None,
        Err(error) => {
            eprintln!("iki: keychain: {}", keychain_message(error));
            None
        }
    }
}

fn keychain_write(key: &str) -> Result<(), String> {
    set_generic_password(SERVICE, ACCOUNT, key.as_bytes()).map_err(|error| {
        let message = keychain_message(error);
        eprintln!("iki: keychain: {message}");
        message
    })
}

fn keychain_message(error: security_framework::base::Error) -> String {
    error
        .message()
        .unwrap_or_else(|| format!("OSStatus {}", error.code()))
}

fn env_key() -> Option<String> {
    std::env::var("SONIOX_API_KEY").ok()
}

/// `fnox get SONIOX_API_KEY`. Finder launches have a short PATH, so also try the mise shim.
fn fnox_key() -> Option<String> {
    let fnox = fnox_binary()?;
    let output = Command::new(fnox)
        .args(["get", "SONIOX_API_KEY", "--non-interactive"])
        .env("FNOX_NON_INTERACTIVE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    non_empty(String::from_utf8(output.stdout).ok())
}

fn fnox_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("fnox");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let home = std::env::var_os("HOME")?;
    let home = PathBuf::from(home);
    [
        home.join(".local/share/mise/shims/fnox"),
        home.join(".local/share/mise/installs/fnox/latest/.mise-bins/fnox"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    Rejected,
    Unreachable(String),
}

/// Asks Soniox whether it accepts `key`. Nothing is stored.
pub async fn verify(key: &str) -> Verdict {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(error) => return Verdict::Unreachable(redact(&error.to_string(), key)),
    };
    match client.get(VERIFY_URL).bearer_auth(key).send().await {
        Ok(response) => verdict_for_status(response.status().as_u16()),
        Err(error) => Verdict::Unreachable(redact(&error.to_string(), key)),
    }
}

fn verdict_for_status(status: u16) -> Verdict {
    match status {
        200..=299 => Verdict::Accepted,
        401 | 403 => Verdict::Rejected,
        other => Verdict::Unreachable(format!("HTTP {other}")),
    }
}

fn redact(message: &str, key: &str) -> String {
    if key.is_empty() {
        message.to_string()
    } else {
        message.replace(key, "[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::{import_legacy_file, redact, resolve, verdict_for_status, Source, Verdict};
    use std::cell::Cell;

    fn some(key: &str) -> Option<String> {
        Some(key.to_string())
    }

    #[test]
    fn the_saved_key_wins_and_fnox_is_not_run() {
        let resolved = resolve(some("saved"), || some("env"), || panic!("fnox ran")).unwrap();
        assert_eq!(resolved.key, "saved");
        assert_eq!(resolved.source, Source::Keychain);
    }

    #[test]
    fn the_environment_comes_before_fnox() {
        let resolved = resolve(None, || some(" env \n"), || panic!("fnox ran")).unwrap();
        assert_eq!(resolved.key, "env");
        assert_eq!(resolved.source, Source::Environment);
    }

    #[test]
    fn blank_keys_fall_through_to_fnox() {
        let resolved = resolve(some("  "), || some(""), || some("from fnox")).unwrap();
        assert_eq!(resolved.key, "from fnox");
        assert_eq!(resolved.source, Source::Fnox);
        assert!(resolve(None, || None, || None).is_none());
    }

    #[test]
    fn status_codes_map_to_verdicts() {
        assert_eq!(verdict_for_status(200), Verdict::Accepted);
        assert_eq!(verdict_for_status(401), Verdict::Rejected);
        assert_eq!(verdict_for_status(403), Verdict::Rejected);
        assert_eq!(
            verdict_for_status(500),
            Verdict::Unreachable("HTTP 500".into())
        );
    }

    #[test]
    fn errors_never_carry_the_key() {
        assert_eq!(
            redact("bad header: Bearer sk-123", "sk-123"),
            "bad header: Bearer [redacted]"
        );
    }

    fn temp_file(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("iki-key-{name}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("soniox_api_key")
    }

    #[test]
    fn the_legacy_file_is_imported_then_deleted() {
        let path = temp_file("import");
        std::fs::write(&path, "  sk-legacy\n").unwrap();
        let stored = Cell::new(None);
        let imported = import_legacy_file(&path, |key| {
            stored.set(Some(key.to_string()));
            true
        });
        assert_eq!(imported.as_deref(), Some("sk-legacy"));
        assert_eq!(stored.take().as_deref(), Some("sk-legacy"));
        assert!(!path.exists());
    }

    #[test]
    fn the_legacy_file_stays_when_the_keychain_refuses_it() {
        let path = temp_file("refused");
        std::fs::write(&path, "sk-legacy").unwrap();
        assert!(import_legacy_file(&path, |_| false).is_none());
        assert!(path.exists());
    }

    #[test]
    fn a_missing_or_blank_legacy_file_stores_nothing() {
        let path = temp_file("blank");
        assert!(import_legacy_file(&path, |_| panic!("stored")).is_none());
        std::fs::write(&path, "\n").unwrap();
        assert!(import_legacy_file(&path, |_| panic!("stored")).is_none());
    }
}
