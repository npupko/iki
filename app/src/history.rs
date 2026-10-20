//! Local transcript history for Iki.
//!
//! Stored at `~/Library/Application Support/Iki/history.json`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::storage::{app_support_file, now_ms, write_atomic};

const MAX_ENTRIES: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub at_ms: u64,
    pub text: String,
}

pub fn record(text: &str) {
    record_to(&history_path(), text);
}

pub fn load() -> Vec<Entry> {
    load_from(&history_path())
}

/// `(transcript count, whitespace-separated word count)`.
pub fn stats(entries: &[Entry]) -> (usize, usize) {
    let words = entries.iter().map(|entry| word_count(&entry.text)).sum();
    (entries.len(), words)
}

fn history_path() -> PathBuf {
    app_support_file("history.json")
}

fn record_to(path: &Path, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    with_lock(|| {
        let mut entries = read_entries(path);
        entries.push(Entry {
            at_ms: now_ms(),
            text: text.to_string(),
        });
        if entries.len() > MAX_ENTRIES {
            let extra = entries.len() - MAX_ENTRIES;
            entries.drain(0..extra);
        }
        if let Err(error) = write_atomic(path, encode(&entries).as_bytes()) {
            eprintln!("iki history: {error}");
        }
    });
}

fn load_from(path: &Path) -> Vec<Entry> {
    with_lock(|| read_entries(path))
}

fn with_lock<T>(body: impl FnOnce() -> T) -> T {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    body()
}

fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

fn read_entries(path: &Path) -> Vec<Entry> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(),
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Vec::new();
    };
    if text.trim().is_empty() {
        return Vec::new();
    }
    parse_entries(&text).unwrap_or_default()
}

fn encode(entries: &[Entry]) -> String {
    let mut out = String::with_capacity(entries.len().saturating_mul(48));
    out.push('[');
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"at_ms\":");
        out.push_str(&entry.at_ms.to_string());
        out.push_str(",\"text\":");
        push_json_string(&mut out, &entry.text);
        out.push('}');
    }
    out.push(']');
    out
}

fn push_json_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn parse_entries(text: &str) -> Option<Vec<Entry>> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        index: 0,
    };
    if !parser.eat(b'[') {
        return None;
    }
    let mut entries = Vec::new();
    if parser.eat(b']') {
        parser.skip_ws();
        return if parser.index == parser.bytes.len() {
            Some(entries)
        } else {
            None
        };
    }
    loop {
        entries.push(parser.parse_entry()?);
        if parser.eat(b',') {
            continue;
        }
        if parser.eat(b']') {
            break;
        }
        return None;
    }
    parser.skip_ws();
    if parser.index == parser.bytes.len() {
        Some(entries)
    } else {
        None
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.index += 1;
        }
    }

    fn eat(&mut self, expected: u8) -> bool {
        self.skip_ws();
        if self.peek() == Some(expected) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn parse_entry(&mut self) -> Option<Entry> {
        if !self.eat(b'{') {
            return None;
        }
        let mut at_ms = None;
        let mut text = None;
        if self.eat(b'}') {
            return None;
        }
        loop {
            let key = self.parse_string()?;
            if !self.eat(b':') {
                return None;
            }
            match key.as_str() {
                "at_ms" if at_ms.is_none() => at_ms = Some(self.parse_u64()?),
                "text" if text.is_none() => text = Some(self.parse_string()?),
                _ => return None,
            }
            if self.eat(b',') {
                continue;
            }
            if self.eat(b'}') {
                break;
            }
            return None;
        }
        Some(Entry {
            at_ms: at_ms?,
            text: text?,
        })
    }

    fn parse_u64(&mut self) -> Option<u64> {
        self.skip_ws();
        let start = self.index;
        let first = self.peek()?;
        if !first.is_ascii_digit() {
            return None;
        }
        self.index += 1;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.index += 1;
        }
        let digits = &self.bytes[start..self.index];
        if digits.len() > 1 && digits[0] == b'0' {
            return None;
        }
        std::str::from_utf8(digits).ok()?.parse().ok()
    }

    fn parse_string(&mut self) -> Option<String> {
        self.skip_ws();
        if self.peek() != Some(b'"') {
            return None;
        }
        self.index += 1;
        let mut out = String::new();
        loop {
            match self.peek()? {
                b'"' => {
                    self.index += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.index += 1;
                    let escaped = self.peek()?;
                    self.index += 1;
                    match escaped {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.parse_unicode_escape()?),
                        _ => return None,
                    }
                }
                byte if byte < 0x20 => return None,
                _ => {
                    let (ch, len) = decode_utf8_char(&self.bytes[self.index..])?;
                    out.push(ch);
                    self.index += len;
                }
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Option<char> {
        let unit = self.parse_hex4()?;
        if (0xD800..=0xDBFF).contains(&unit) {
            if self.peek() != Some(b'\\') {
                return None;
            }
            self.index += 1;
            if self.peek() != Some(b'u') {
                return None;
            }
            self.index += 1;
            let low = self.parse_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return None;
            }
            let code = 0x10000 + (((unit as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
            char::from_u32(code)
        } else if (0xDC00..=0xDFFF).contains(&unit) {
            None
        } else {
            char::from_u32(unit as u32)
        }
    }

    fn parse_hex4(&mut self) -> Option<u16> {
        if self.index + 4 > self.bytes.len() {
            return None;
        }
        let hex = std::str::from_utf8(&self.bytes[self.index..self.index + 4]).ok()?;
        self.index += 4;
        u16::from_str_radix(hex, 16).ok()
    }
}

fn decode_utf8_char(bytes: &[u8]) -> Option<(char, usize)> {
    let width = match bytes.first()? {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return None,
    };
    if bytes.len() < width {
        return None;
    }
    let ch = std::str::from_utf8(&bytes[..width]).ok()?.chars().next()?;
    Some((ch, width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempHist {
        path: PathBuf,
    }

    impl TempHist {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("iki-history-{nanos}-{n}"));
            fs::create_dir_all(&dir).unwrap();
            Self {
                path: dir.join("history.json"),
            }
        }
    }

    impl Drop for TempHist {
        fn drop(&mut self) {
            if let Some(dir) = self.path.parent() {
                let _ = fs::remove_dir_all(dir);
            }
        }
    }

    #[test]
    fn json_round_trip_escapes_and_order() {
        let temp = TempHist::new();
        let text = "say \"hi\"\\path\nline\tend\u{0001} globe \u{1F30E}";
        record_to(&temp.path, "  \n\t");
        record_to(&temp.path, "");
        assert!(!temp.path.exists());
        record_to(&temp.path, text);
        record_to(&temp.path, "  second  ");
        record_to(&temp.path, " \t ");
        let loaded = load_from(&temp.path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].text, text);
        assert_eq!(loaded[1].text, "  second  ");
        assert!(loaded[1].at_ms >= loaded[0].at_ms);
        assert_eq!(stats(&loaded), (2, 7));
    }

    #[test]
    fn parses_hand_written_json_and_rejects_corrupt() {
        let temp = TempHist::new();
        assert!(load_from(&temp.path).is_empty());

        fs::write(
            &temp.path,
            "[\n  {\n    \"text\" : \"\\u0041\\uD83C\\uDF0D\",\n    \"at_ms\" : 1700000000000\n  }\n]\n",
        )
        .unwrap();
        let loaded = load_from(&temp.path);
        assert_eq!(
            loaded,
            vec![Entry {
                at_ms: 1_700_000_000_000,
                text: "A🌍".to_string(),
            }]
        );

        for corrupt in [
            b"{not json".as_slice(),
            br#"[{"at_ms":1}]"#,
            br#"[{"text":"x"}]"#,
            br#"[{"at_ms":01,"text":"x"}]"#,
            br#"[{"at_ms":1,"text":"x"},]"#,
            b"\xff\xfe",
            b"",
            br#"{"at_ms":1,"text":"x"}"#,
        ] {
            fs::write(&temp.path, corrupt).unwrap();
            assert!(load_from(&temp.path).is_empty(), "{corrupt:?}");
        }
    }

    #[test]
    fn caps_at_five_hundred_newest() {
        let temp = TempHist::new();
        for index in 0..501 {
            record_to(&temp.path, &format!("n{index}"));
        }
        let loaded = load_from(&temp.path);
        assert_eq!(loaded.len(), 500);
        assert_eq!(loaded[0].text, "n1");
        assert_eq!(loaded[499].text, "n500");
        assert_eq!(stats(&loaded).0, 500);
        assert_eq!(stats(&loaded).1, 500);
    }

    #[test]
    fn word_count_splits_on_whitespace_only() {
        assert_eq!(stats(&[]), (0, 0));
        assert_eq!(
            stats(&[
                Entry {
                    at_ms: 1,
                    text: "hello, world".into(),
                },
                Entry {
                    at_ms: 2,
                    text: "  one   two\nthree ".into(),
                },
                Entry {
                    at_ms: 3,
                    text: "   ".into(),
                },
            ]),
            (3, 5)
        );
    }

    #[test]
    fn creates_missing_parent_directory() {
        let temp = TempHist::new();
        let path = temp
            .path
            .parent()
            .unwrap()
            .join("nested")
            .join("history.json");
        record_to(&path, "hello");
        let loaded = load_from(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].text, "hello");
        let _ = fs::remove_dir_all(temp.path.parent().unwrap().join("nested"));
    }
}
