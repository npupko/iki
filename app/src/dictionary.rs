//! Personal dictionary for Iki.
//!
//! Soniox `context.terms` is the spelling to recognize, not a find-and-replace
//! list. A vocabulary word contributes its phrase. A correction contributes
//! its replacement, never the misspelling. That is the documented way to keep
//! a name's spelling and casing, and Soniox has no replacements field.
//! https://soniox.com/docs/stt/concepts/context
//!
//! The finished transcript is still rewritten locally, and the pass rewrites
//! it: longest phrase first, case-insensitive, whole word, and `don't` is left
//! alone. A later correction wins when two rows fold to the same phrase.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::storage::{app_support_file, now_ms, set_aside_unreadable, write_atomic};

const MAX_CHARS: usize = 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    StarredFirst,
    Alphabetical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictError {
    Empty,
    TooLong,
    AlreadyExists,
    CorrectionExists,
    Missing,
}

impl DictError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "Enter a word",
            Self::TooLong => "Words can be at most 60 characters",
            Self::AlreadyExists => "That word is already in your vocabulary",
            Self::CorrectionExists => "That correction already exists",
            Self::Missing => "That word is no longer in your vocabulary",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub phrase: String,
    pub replacement: Option<String>,
    pub starred: bool,
    pub created_ms: u64,
    pub modified_ms: u64,
    pub frequency_used: u32,
    pub last_used_ms: Option<u64>,
}

impl Entry {
    pub fn label(&self) -> String {
        match &self.replacement {
            Some(replacement) => format!("{} → {replacement}", self.phrase),
            None => self.phrase.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileShape {
    #[serde(default)]
    sort: Sort,
    #[serde(default)]
    entries: Vec<StoredEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredEntry {
    phrase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replacement: Option<String>,
    #[serde(default)]
    starred: bool,
    #[serde(default)]
    created_ms: u64,
    #[serde(default)]
    modified_ms: u64,
    #[serde(default)]
    frequency_used: u32,
    #[serde(default)]
    last_used_ms: Option<u64>,
}

#[derive(Clone, Debug)]
struct Store {
    sort: Sort,
    entries: Vec<Entry>,
}

impl Store {
    fn empty() -> Self {
        Self {
            sort: Sort::Newest,
            entries: Vec::new(),
        }
    }
}

pub fn sort() -> Sort {
    with_lock(|| read_store(&dict_path()).sort)
}

pub fn set_sort(sort: Sort) {
    mutate(|store| store.sort = sort);
}

/// Total entry count and the entries matching `query`, from one read.
pub fn listing(query: &str) -> (usize, Vec<Entry>) {
    with_lock(|| {
        let store = read_store(&dict_path());
        (store.entries.len(), store.visible(query))
    })
}

pub fn find(phrase: &str) -> Option<Entry> {
    with_lock(|| {
        let store = read_store(&dict_path());
        store
            .entries
            .iter()
            .find(|entry| entry.phrase == phrase)
            .cloned()
    })
}

pub fn add_word(word: &str) -> Result<(), DictError> {
    mutate_result(|store| store.add_word(word))
}

pub fn add_correction(misspelling: &str, spelling: &str) -> Result<(), DictError> {
    mutate_result(|store| store.add_correction(misspelling, spelling))
}

pub fn update(original: &str, phrase: &str, replacement: Option<&str>) -> Result<(), DictError> {
    mutate_result(|store| store.update(original, phrase, replacement))
}

pub fn remove(phrase: &str) {
    mutate(|store| {
        store.remove(phrase);
    });
}

pub fn toggle_starred(phrase: &str) {
    mutate(|store| store.toggle_starred(phrase));
}

/// Spellings to send as Soniox `context.terms`, starred first.
///
/// A vocabulary row sends its phrase. A correction sends the replacement.
/// The misspelling is not a term. The session layer trims this list to the
/// documented context budget.
pub fn recognition_terms() -> Vec<String> {
    with_lock(|| read_store(&dict_path()).recognition_terms())
}

/// Count dictionary uses in a finished transcript, then return it with the
/// corrections applied. One read, and one write only when a count changed.
pub fn process_transcript(text: &str) -> String {
    with_lock(|| {
        let path = dict_path();
        // An unreadable file yields no entries, so nothing is counted or written.
        let mut store = read_store(&path);
        if store.note_uses(text) {
            if let Err(error) = write_store(&path, &store) {
                eprintln!("iki dictionary: {error}");
            }
        }
        store.apply(text)
    })
}

fn dict_path() -> PathBuf {
    app_support_file("dictionary.json")
}

fn with_lock<T>(body: impl FnOnce() -> T) -> T {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    body()
}

fn mutate(body: impl FnOnce(&mut Store)) {
    let _ = mutate_result(|store| {
        body(store);
        Ok::<(), DictError>(())
    });
}

fn mutate_result<T>(body: impl FnOnce(&mut Store) -> Result<T, DictError>) -> Result<T, DictError> {
    with_lock(|| {
        let path = dict_path();
        let mut store = read_store_for_write(&path);
        let result = body(&mut store)?;
        if let Err(error) = write_store(&path, &store) {
            eprintln!("iki dictionary: {error}");
        }
        Ok(result)
    })
}

fn read_store(path: &Path) -> Store {
    load_store(path).unwrap_or_else(|Unreadable| Store::empty())
}

/// Like [`read_store`], but a file that exists and cannot be parsed is set
/// aside first, so the write that follows cannot erase the user's words.
fn read_store_for_write(path: &Path) -> Store {
    load_store(path).unwrap_or_else(|Unreadable| {
        set_aside_unreadable(path);
        Store::empty()
    })
}

struct Unreadable;

fn load_store(path: &Path) -> Result<Store, Unreadable> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Store::empty()),
        Err(_) => return Err(Unreadable),
    };
    let text = String::from_utf8(bytes).map_err(|_| Unreadable)?;
    if text.trim().is_empty() {
        return Ok(Store::empty());
    }
    serde_json::from_str::<FileShape>(&text)
        .map(|file| Store {
            sort: file.sort,
            entries: file
                .entries
                .into_iter()
                .filter_map(|entry| {
                    let phrase = trim(&entry.phrase);
                    if phrase.is_empty() {
                        return None;
                    }
                    let replacement = entry.replacement.and_then(|value| {
                        let value = trim(&value);
                        if value.is_empty() {
                            None
                        } else {
                            Some(value)
                        }
                    });
                    Some(Entry {
                        phrase,
                        replacement,
                        starred: entry.starred,
                        created_ms: entry.created_ms,
                        modified_ms: entry.modified_ms,
                        frequency_used: entry.frequency_used,
                        last_used_ms: entry.last_used_ms,
                    })
                })
                .collect(),
        })
        .map_err(|_| Unreadable)
}

fn write_store(path: &Path, store: &Store) -> io::Result<()> {
    let shape = FileShape {
        sort: store.sort,
        entries: store
            .entries
            .iter()
            .map(|entry| StoredEntry {
                phrase: entry.phrase.clone(),
                replacement: entry.replacement.clone(),
                starred: entry.starred,
                created_ms: entry.created_ms,
                modified_ms: entry.modified_ms,
                frequency_used: entry.frequency_used,
                last_used_ms: entry.last_used_ms,
            })
            .collect(),
    };
    let bytes = serde_json::to_vec_pretty(&shape)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_atomic(path, &bytes)
}

impl Store {
    fn add_word(&mut self, word: &str) -> Result<(), DictError> {
        let phrase = prepare(word)?;
        if self.find(&phrase).is_some() {
            return Err(DictError::AlreadyExists);
        }
        self.entries.push(new_entry(phrase, None));
        Ok(())
    }

    fn add_correction(&mut self, misspelling: &str, spelling: &str) -> Result<(), DictError> {
        let phrase = prepare(misspelling)?;
        let spelling = prepare(spelling)?;
        if let Some(index) = self.find(&phrase) {
            if self.entries[index].replacement.is_some() {
                return Err(DictError::CorrectionExists);
            }
            self.entries[index].replacement = Some(spelling);
            self.entries[index].modified_ms = now_ms();
            return Ok(());
        }
        self.entries.push(new_entry(phrase, Some(spelling)));
        Ok(())
    }

    fn update(
        &mut self,
        original: &str,
        phrase: &str,
        replacement: Option<&str>,
    ) -> Result<(), DictError> {
        let Some(index) = self.find(original) else {
            return Err(DictError::Missing);
        };
        let phrase = prepare(phrase)?;
        let replacement = match replacement {
            Some(value) => Some(prepare(value)?),
            None => None,
        };
        if let Some(other) = self.find(&phrase) {
            if other != index {
                return Err(
                    if replacement.is_some() && self.entries[other].replacement.is_some() {
                        DictError::CorrectionExists
                    } else {
                        DictError::AlreadyExists
                    },
                );
            }
        }
        let entry = &mut self.entries[index];
        entry.phrase = phrase;
        entry.replacement = replacement;
        entry.modified_ms = now_ms();
        Ok(())
    }

    fn remove(&mut self, phrase: &str) {
        if let Some(index) = self.find(phrase) {
            self.entries.remove(index);
        }
    }

    fn toggle_starred(&mut self, phrase: &str) {
        if let Some(index) = self.find(phrase) {
            let entry = &mut self.entries[index];
            entry.starred = !entry.starred;
        }
    }

    fn visible(&self, query: &str) -> Vec<Entry> {
        let query = query.trim().to_lowercase();
        let mut entries: Vec<Entry> = self
            .entries
            .iter()
            .filter(|entry| {
                if query.is_empty() {
                    return true;
                }
                entry.phrase.to_lowercase().contains(&query)
                    || entry
                        .replacement
                        .as_ref()
                        .is_some_and(|value| value.to_lowercase().contains(&query))
            })
            .cloned()
            .collect();
        match self.sort {
            Sort::Newest => entries.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms)),
            Sort::Oldest => entries.sort_by(|a, b| a.modified_ms.cmp(&b.modified_ms)),
            Sort::StarredFirst => entries.sort_by(|a, b| {
                b.starred
                    .cmp(&a.starred)
                    .then(b.modified_ms.cmp(&a.modified_ms))
            }),
            Sort::Alphabetical => entries.sort_by(|a, b| {
                a.phrase
                    .to_lowercase()
                    .cmp(&b.phrase.to_lowercase())
                    .then_with(|| {
                        a.replacement
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .cmp(&b.replacement.as_deref().unwrap_or("").to_lowercase())
                    })
            }),
        }
        entries
    }

    fn recognition_terms(&self) -> Vec<String> {
        struct Candidate {
            spelling: String,
            starred: bool,
            frequency: u32,
            modified_ms: u64,
        }
        let mut by_key: Vec<Candidate> = Vec::new();
        for entry in &self.entries {
            let spelling = entry
                .replacement
                .clone()
                .unwrap_or_else(|| entry.phrase.clone());
            if spelling.is_empty() {
                continue;
            }
            let key = spelling.to_lowercase();
            if let Some(existing) = by_key
                .iter_mut()
                .find(|item| item.spelling.to_lowercase() == key)
            {
                let prefer = (entry.starred, entry.frequency_used, entry.modified_ms)
                    > (existing.starred, existing.frequency, existing.modified_ms);
                if prefer {
                    existing.spelling = spelling;
                    existing.starred = entry.starred;
                    existing.frequency = entry.frequency_used;
                    existing.modified_ms = entry.modified_ms;
                } else if entry.starred {
                    existing.starred = true;
                }
                existing.frequency = existing.frequency.max(entry.frequency_used);
            } else {
                by_key.push(Candidate {
                    spelling,
                    starred: entry.starred,
                    frequency: entry.frequency_used,
                    modified_ms: entry.modified_ms,
                });
            }
        }
        by_key.sort_by(|a, b| {
            b.starred
                .cmp(&a.starred)
                .then(b.frequency.cmp(&a.frequency))
                .then_with(|| a.spelling.to_lowercase().cmp(&b.spelling.to_lowercase()))
        });
        by_key.into_iter().map(|item| item.spelling).collect()
    }

    fn apply(&self, text: &str) -> String {
        // Build one case-folded map. A later row overwrites the same
        // key, then the keys are matched longest first. The stored replacement
        // is written as-is and is not scanned again.
        let mut rules: Vec<(String, Vec<char>, String)> = Vec::new();
        for entry in &self.entries {
            let Some(replacement) = entry.replacement.clone() else {
                continue;
            };
            let phrase: Vec<char> = entry.phrase.chars().collect();
            if phrase.is_empty() {
                continue;
            }
            let key = case_fold(&phrase);
            if let Some(existing) = rules.iter_mut().find(|rule| rule.0 == key) {
                existing.1 = phrase;
                existing.2 = replacement;
            } else {
                rules.push((key, phrase, replacement));
            }
        }
        rules.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::new();
        let mut index = 0;
        while index < chars.len() {
            let boundary = index == 0 || !is_word_char(chars[index - 1]);
            if boundary {
                if let Some((len, replacement)) =
                    rules.iter().find_map(|(_, phrase, replacement)| {
                        let end = index + phrase.len();
                        if end > chars.len() || !eq_ignore_case(&chars[index..end], phrase) {
                            return None;
                        }
                        if end < chars.len() && is_word_char(chars[end]) {
                            return None;
                        }
                        if blocked_contraction(&chars, end) {
                            return None;
                        }
                        Some((phrase.len(), replacement.as_str()))
                    })
                {
                    out.push_str(replacement);
                    index += len;
                    continue;
                }
            }
            out.push(chars[index]);
            index += 1;
        }
        out
    }

    /// A correction also counts its spelling. That spelling is sent to Soniox
    /// as a term, so the misspelling alone stops showing up once it works.
    fn note_uses(&mut self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let now = now_ms();
        let mut changed = false;
        for entry in &mut self.entries {
            let phrase: Vec<char> = entry.phrase.chars().collect();
            if phrase.is_empty() {
                continue;
            }
            let mut count = count_matches(&chars, &phrase);
            if let Some(replacement) = &entry.replacement {
                let spelling: Vec<char> = replacement.chars().collect();
                if !spelling.is_empty() && case_fold(&spelling) != case_fold(&phrase) {
                    count = count.saturating_add(count_matches(&chars, &spelling));
                }
            }
            if count == 0 {
                continue;
            }
            entry.frequency_used = entry.frequency_used.saturating_add(count);
            entry.last_used_ms = Some(now);
            changed = true;
        }
        changed
    }

    fn find(&self, phrase: &str) -> Option<usize> {
        let key = trim(phrase).to_lowercase();
        self.entries
            .iter()
            .position(|entry| entry.phrase.to_lowercase() == key)
    }
}

fn new_entry(phrase: String, replacement: Option<String>) -> Entry {
    let now = now_ms();
    Entry {
        phrase,
        replacement,
        starred: false,
        created_ms: now,
        modified_ms: now,
        frequency_used: 0,
        last_used_ms: None,
    }
}

fn prepare(raw: &str) -> Result<String, DictError> {
    let phrase = trim(raw);
    if phrase.is_empty() || !meaningful(&phrase) {
        return Err(DictError::Empty);
    }
    if phrase.chars().count() > MAX_CHARS {
        return Err(DictError::TooLong);
    }
    Ok(phrase)
}

fn meaningful(text: &str) -> bool {
    text.chars().any(|c| c.is_alphanumeric())
}

fn trim(text: &str) -> String {
    text.trim_matches(is_edge_space).to_string()
}

fn is_edge_space(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t'
            | '\n'
            | '\u{0b}'
            | '\u{0c}'
            | '\r'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
            | '\u{ad}'
            | '\u{061c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn case_fold(chars: &[char]) -> String {
    chars.iter().flat_map(|c| c.to_lowercase()).collect()
}

/// A word character is `\p{L}`, `\p{N}`, or `_`. `is_alphanumeric` matches
/// that set for letters and numbers, including Cyrillic.
fn is_word_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '‘' | '’' | '´' | '`' | '′')
}

fn blocked_contraction(chars: &[char], end: usize) -> bool {
    if end + 1 >= chars.len() {
        return false;
    }
    if !is_apostrophe(chars[end]) || !chars[end + 1].eq_ignore_ascii_case(&'t') {
        return false;
    }
    end + 2 >= chars.len() || !chars[end + 2].is_alphanumeric()
}

fn eq_ignore_case(text: &[char], phrase: &[char]) -> bool {
    if text.len() != phrase.len() {
        return false;
    }
    text.iter().zip(phrase.iter()).all(|(left, right)| {
        left == right || left.to_lowercase().to_string() == right.to_lowercase().to_string()
    })
}

fn count_matches(chars: &[char], phrase: &[char]) -> u32 {
    let mut index = 0;
    let mut count = 0u32;
    while index < chars.len() {
        let end = index + phrase.len();
        let boundary = index == 0 || !is_word_char(chars[index - 1]);
        if boundary
            && end <= chars.len()
            && eq_ignore_case(&chars[index..end], phrase)
            && (end == chars.len() || !is_word_char(chars[end]))
            && !blocked_contraction(chars, end)
        {
            count = count.saturating_add(1);
            index = end;
        } else {
            index += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(phrase: &str, replacement: Option<&str>, starred: bool, frequency: u32) -> Entry {
        Entry {
            phrase: phrase.to_string(),
            replacement: replacement.map(str::to_string),
            starred,
            created_ms: 1,
            modified_ms: frequency as u64 + 1,
            frequency_used: frequency,
            last_used_ms: None,
        }
    }

    #[test]
    fn rejects_blank_punctuation_and_overlong_words() {
        let mut store = Store::empty();
        assert_eq!(store.add_word("   "), Err(DictError::Empty));
        assert_eq!(store.add_word("!!!"), Err(DictError::Empty));
        assert_eq!(store.add_word(&"a".repeat(61)), Err(DictError::TooLong));
        assert!(store.add_word("  Soniox  ").is_ok());
        assert_eq!(store.entries[0].phrase, "Soniox");
    }

    #[test]
    fn duplicate_words_and_corrections_fold_case() {
        let mut store = Store::empty();
        store.add_word("Soniox").unwrap();
        assert_eq!(store.add_word("soniox"), Err(DictError::AlreadyExists));
        store.add_correction("soniox", "Soniox").unwrap();
        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0].replacement.as_deref(), Some("Soniox"));
        assert_eq!(
            store.add_correction("Soniox", "Sonio"),
            Err(DictError::CorrectionExists)
        );
    }

    #[test]
    fn recognition_terms_use_the_desired_spelling_and_put_starred_first() {
        let mut store = Store::empty();
        store.entries.push(entry("teh", Some("the"), false, 1));
        store.entries.push(entry("Soniox", None, false, 9));
        store
            .entries
            .push(entry("iki", Some("Iki"), true, 0));
        store.entries.push(entry("Iki", None, false, 3));
        let terms = store.recognition_terms();
        assert_eq!(
            terms,
            vec![
                "Iki".to_string(),
                "Soniox".to_string(),
                "the".to_string()
            ]
        );
        assert!(!terms.iter().any(|term| term == "teh"));
    }

    #[test]
    fn corrections_replace_whole_words_longest_first_and_skip_contractions() {
        let mut store = Store::empty();
        store.entries.push(entry("new", Some("NEW"), false, 0));
        store
            .entries
            .push(entry("new york", Some("New York"), false, 0));
        store.entries.push(entry("don", Some("DON"), false, 0));
        let text = store.apply("Meet in new york, don, and don't.");
        assert_eq!(text, "Meet in New York, DON, and don't.");
        assert_eq!(store.apply("don_t and a_don"), "don_t and a_don");
    }

    #[test]
    fn a_later_correction_wins_and_russian_stays_a_whole_word() {
        let mut store = Store::empty();
        store.entries.push(entry("teh", Some("the"), false, 0));
        store.entries.push(entry("TEH", Some("THE"), false, 0));
        store
            .entries
            .push(entry("привет", Some("здравствуйте"), false, 0));
        assert_eq!(store.apply("say teh"), "say THE");
        assert_eq!(store.apply("скажи Привет."), "скажи здравствуйте.");
        assert_eq!(store.apply("приветствие"), "приветствие");
    }

    #[test]
    fn uses_add_the_count_from_the_dictated_text() {
        let mut store = Store::empty();
        store.entries.push(entry("Soniox", None, false, 1));
        assert!(store.note_uses("Soniox and soniox"));
        assert_eq!(store.entries[0].frequency_used, 3);
        assert!(store.entries[0].last_used_ms.is_some());
        assert!(!store.note_uses("nothing"));
    }

    #[test]
    fn a_correction_counts_its_spelling_too() {
        let mut store = Store::empty();
        store.entries.push(entry("teh", Some("the"), false, 0));
        store.entries.push(entry("soniox", Some("Soniox"), false, 0));
        assert!(store.note_uses("the cat and teh dog"));
        assert_eq!(store.entries[0].frequency_used, 2);
        assert!(store.note_uses("Soniox"));
        assert_eq!(store.entries[1].frequency_used, 1);
    }

    #[test]
    fn an_unreadable_file_is_set_aside_before_a_write() {
        let dir = std::env::temp_dir().join(format!(
            "iki-dictionary-bad-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dictionary.json");
        fs::write(&path, br#"{"entries":[{"phrase":"Soniox","sort":"most_used"}],"sort":"most_used"}"#)
            .unwrap();
        assert!(read_store(&path).entries.is_empty());
        assert!(path.exists(), "a plain read must not move the file");
        let store = read_store_for_write(&path);
        assert!(store.entries.is_empty());
        let kept: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|item| item.file_name().to_string_lossy().into_owned())
            .collect();
        let _ = fs::remove_dir_all(&dir);
        assert!(!kept.iter().any(|name| name == "dictionary.json"));
        assert!(kept
            .iter()
            .any(|name| name.starts_with("dictionary.json.unreadable-")));
    }

    #[test]
    fn entries_without_timestamps_still_load() {
        let path = std::env::temp_dir().join(format!(
            "iki-dictionary-old-{}-{}.json",
            std::process::id(),
            now_ms()
        ));
        fs::write(&path, br#"{"entries":[{"phrase":"Soniox"}]}"#).unwrap();
        let loaded = read_store(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].phrase, "Soniox");
    }

    #[test]
    fn search_sort_and_file_round_trip() {
        let mut store = Store::empty();
        store.add_word("beta").unwrap();
        store.add_correction("alfa miss", "Alfa").unwrap();
        store.toggle_starred("beta");
        store.sort = Sort::Alphabetical;
        let shown = store.visible("alfa");
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].phrase, "alfa miss");

        let path = std::env::temp_dir().join(format!(
            "iki-dictionary-{}-{}.json",
            std::process::id(),
            now_ms()
        ));
        write_store(&path, &store).unwrap();
        let loaded = read_store(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(loaded.sort, Sort::Alphabetical);
        assert_eq!(loaded.entries.len(), 2);
        assert!(loaded
            .entries
            .iter()
            .any(|entry| entry.starred && entry.phrase == "beta"));
    }
}
