//! Files under `~/Library/Application Support/Iki`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn app_support_file(name: &str) -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
    PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("Iki")
        .join(name)
}

/// Write to a sibling `.tmp` file, then rename it over `path`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = with_suffix(path, ".tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Move a file that could not be parsed out of the way, so the next write
/// does not replace the only copy of the user's data.
pub fn set_aside_unreadable(path: &Path) {
    let aside = with_suffix(path, &format!(".unreadable-{}", now_ms()));
    match fs::rename(path, &aside) {
        Ok(()) => eprintln!(
            "iki: {} could not be read; kept it as {}",
            path.display(),
            aside.display()
        ),
        Err(error) => eprintln!("iki: {}: {error}", path.display()),
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}
