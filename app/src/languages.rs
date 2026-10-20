//! Languages Iki sends as Soniox `language_hints`, in list order.
//!
//! A missing file means English only. `language_hints_strict` is set by the
//! session when this list is non-empty.
//! https://soniox.com/docs/stt/concepts/language-hints
//! https://soniox.com/docs/stt/concepts/language-restrictions
//! https://soniox.com/docs/stt/concepts/supported-languages

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Soniox real-time languages, ISO 639-1 codes, in the documented list order.
const CATALOG: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("sq", "Albanian"),
    ("ar", "Arabic"),
    ("az", "Azerbaijani"),
    ("eu", "Basque"),
    ("be", "Belarusian"),
    ("bn", "Bengali"),
    ("bs", "Bosnian"),
    ("bg", "Bulgarian"),
    ("ca", "Catalan"),
    ("zh", "Chinese"),
    ("hr", "Croatian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("nl", "Dutch"),
    ("en", "English"),
    ("et", "Estonian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("gl", "Galician"),
    ("de", "German"),
    ("el", "Greek"),
    ("gu", "Gujarati"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hu", "Hungarian"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("kn", "Kannada"),
    ("kk", "Kazakh"),
    ("ko", "Korean"),
    ("lv", "Latvian"),
    ("lt", "Lithuanian"),
    ("mk", "Macedonian"),
    ("ms", "Malay"),
    ("ml", "Malayalam"),
    ("mr", "Marathi"),
    ("no", "Norwegian"),
    ("fa", "Persian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("pa", "Punjabi"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sr", "Serbian"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("es", "Spanish"),
    ("sw", "Swahili"),
    ("sv", "Swedish"),
    ("tl", "Tagalog"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("th", "Thai"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("vi", "Vietnamese"),
    ("cy", "Welsh"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageError {
    Unknown,
    Duplicate,
    Last,
}

impl LanguageError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unknown => "That language is not supported",
            Self::Duplicate => "That language is already in the list",
            Self::Last => "Keep at least one language",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Store {
    codes: Vec<String>,
}

impl Store {
    fn english() -> Self {
        Self {
            codes: vec!["en".to_string()],
        }
    }

    fn add(&mut self, code: &str) -> Result<(), LanguageError> {
        let code = canonical(code);
        if !known(&code) {
            return Err(LanguageError::Unknown);
        }
        if self.codes.iter().any(|existing| existing == &code) {
            return Err(LanguageError::Duplicate);
        }
        self.codes.push(code);
        Ok(())
    }

    fn move_up(&mut self, index: usize) {
        if index > 0 && index < self.codes.len() {
            self.codes.swap(index, index - 1);
        }
    }

    fn move_down(&mut self, index: usize) {
        if index + 1 < self.codes.len() {
            self.codes.swap(index, index + 1);
        }
    }

    fn remove(&mut self, index: usize) -> Result<(), LanguageError> {
        if self.codes.len() <= 1 {
            return Err(LanguageError::Last);
        }
        if index < self.codes.len() {
            self.codes.remove(index);
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct FileShape {
    #[serde(default)]
    codes: Vec<String>,
}

pub fn name(code: &str) -> &'static str {
    let code = canonical(code);
    CATALOG
        .iter()
        .find(|(candidate, _)| *candidate == code)
        .map(|(_, name)| *name)
        .unwrap_or("Unknown")
}

pub fn catalog() -> &'static [(&'static str, &'static str)] {
    CATALOG
}

pub fn codes() -> Vec<String> {
    with_lock(|| read_store(&languages_path()).codes)
}

pub fn add(code: &str) -> Result<(), LanguageError> {
    mutate_result(|store| store.add(code))
}

pub fn move_up(index: usize) {
    mutate(|store| store.move_up(index));
}

pub fn move_down(index: usize) {
    mutate(|store| store.move_down(index));
}

pub fn remove(index: usize) -> Result<(), LanguageError> {
    mutate_result(|store| store.remove(index))
}

fn canonical(code: &str) -> String {
    code.trim().to_ascii_lowercase()
}

fn known(code: &str) -> bool {
    CATALOG.iter().any(|(candidate, _)| *candidate == code)
}

fn normalize(codes: Vec<String>) -> Vec<String> {
    let mut kept = Vec::new();
    for code in codes {
        let code = canonical(&code);
        if known(&code) && !kept.iter().any(|existing| existing == &code) {
            kept.push(code);
        }
    }
    if kept.is_empty() {
        vec!["en".to_string()]
    } else {
        kept
    }
}

fn languages_path() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
    PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("Iki")
        .join("languages.json")
}

fn with_lock<T>(body: impl FnOnce() -> T) -> T {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    body()
}

fn mutate(body: impl FnOnce(&mut Store)) {
    let _ = mutate_result(|store| {
        body(store);
        Ok::<(), LanguageError>(())
    });
}

fn mutate_result<T>(
    body: impl FnOnce(&mut Store) -> Result<T, LanguageError>,
) -> Result<T, LanguageError> {
    with_lock(|| {
        let path = languages_path();
        let mut store = read_store(&path);
        let result = body(&mut store)?;
        if let Err(error) = write_store(&path, &store) {
            eprintln!("iki languages: {error}");
        }
        Ok(result)
    })
}

fn read_store(path: &Path) -> Store {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Store::english(),
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Store::english();
    };
    if text.trim().is_empty() {
        return Store::english();
    }
    serde_json::from_str::<FileShape>(&text)
        .map(|file| Store {
            codes: normalize(file.codes),
        })
        .unwrap_or_else(|_| Store::english())
}

fn write_store(path: &Path, store: &Store) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let bytes = serde_json::to_vec_pretty(&FileShape {
        codes: store.codes.clone(),
    })
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let tmp = tmp_path(path);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_includes_english_and_russian() {
        assert_eq!(name("en"), "English");
        assert_eq!(name("ru"), "Russian");
        assert_eq!(CATALOG.len(), 60);
    }

    #[test]
    fn normalize_keeps_order_and_drops_unknown_duplicates() {
        assert_eq!(
            normalize(vec![
                "EN".into(),
                "ru".into(),
                "ru".into(),
                "nope".into(),
                "  ".into(),
            ]),
            vec!["en".to_string(), "ru".to_string()]
        );
        assert_eq!(normalize(vec![]), vec!["en".to_string()]);
    }

    #[test]
    fn order_add_and_last_language_rules() {
        let mut store = Store::english();
        store.add("ru").unwrap();
        assert_eq!(store.codes, vec!["en".to_string(), "ru".to_string()]);
        assert_eq!(store.add("RU"), Err(LanguageError::Duplicate));
        assert_eq!(store.add("zz"), Err(LanguageError::Unknown));
        store.move_down(0);
        assert_eq!(store.codes, vec!["ru".to_string(), "en".to_string()]);
        store.move_up(1);
        assert_eq!(store.codes, vec!["en".to_string(), "ru".to_string()]);
        store.move_up(0);
        assert_eq!(store.codes, vec!["en".to_string(), "ru".to_string()]);
        store.remove(1).unwrap();
        assert_eq!(store.remove(0), Err(LanguageError::Last));
        assert_eq!(store.codes, vec!["en".to_string()]);
    }

    #[test]
    fn file_round_trip_preserves_priority() {
        let path = std::env::temp_dir().join(format!(
            "iki-languages-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));
        let store = Store {
            codes: vec!["en".to_string(), "ru".to_string()],
        };
        write_store(&path, &store).unwrap();
        let loaded = read_store(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(loaded.codes, vec!["en".to_string(), "ru".to_string()]);
    }
}
