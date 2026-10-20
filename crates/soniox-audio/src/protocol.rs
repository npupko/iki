//! Soniox realtime response folding. Final tokens are appended once; each
//! response replaces the provisional tail.

use serde::Deserialize;

use crate::DictationError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTranscript {
    pub committed: String,
    pub partial: String,
}

impl LiveTranscript {
    pub(crate) fn empty() -> Self {
        Self {
            committed: String::new(),
            partial: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptToken {
    pub text: String,
    pub is_final: bool,
}

/// Fold one server response into `state`.
///
/// Each `is_final` token is appended to `committed` a single time. `partial`
/// becomes only this response's non-final text, so a later response cannot
/// duplicate text already committed.
pub(crate) fn apply_response(state: &mut LiveTranscript, tokens: &[TranscriptToken]) {
    let mut partial = String::new();
    for token in tokens {
        if is_endpoint_marker(&token.text) {
            continue;
        }
        if token.is_final {
            state.committed.push_str(&token.text);
        } else {
            partial.push_str(&token.text);
        }
    }
    state.partial = partial;
}

/// `committed + partial`, trimmed. Empty input yields an empty string.
pub(crate) fn render_transcript(state: &LiveTranscript) -> String {
    let mut combined = String::with_capacity(state.committed.len() + state.partial.len());
    combined.push_str(&state.committed);
    combined.push_str(&state.partial);
    combined.trim().to_string()
}

#[derive(Debug, Deserialize)]
struct ServerMessage {
    #[serde(default)]
    tokens: Vec<RawToken>,
    #[serde(default)]
    finished: bool,
    #[serde(default)]
    error_type: Option<String>,
    #[serde(default)]
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawToken {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    is_final: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ingest {
    Continue,
    Finished,
}

pub(crate) fn ingest_message(
    state: &mut LiveTranscript,
    payload: &str,
) -> Result<Ingest, DictationError> {
    let message: ServerMessage = serde_json::from_str(payload)
        .map_err(|error| DictationError::Protocol(format!("invalid JSON: {error}")))?;
    let tokens = message
        .tokens
        .into_iter()
        .map(|token| TranscriptToken {
            text: token.text.unwrap_or_default(),
            is_final: token.is_final,
        })
        .collect::<Vec<_>>();
    apply_response(state, &tokens);
    if message.error_type.is_some() || message.error_message.is_some() {
        return Err(protocol_error(message.error_type, message.error_message));
    }
    if message.finished {
        Ok(Ingest::Finished)
    } else {
        Ok(Ingest::Continue)
    }
}

fn is_endpoint_marker(text: &str) -> bool {
    matches!(text, "<end>" | "<fin>")
}

fn protocol_error(error_type: Option<String>, error_message: Option<String>) -> DictationError {
    let detail = match (error_type, error_message) {
        (Some(kind), Some(message)) if !message.is_empty() => format!("{kind}: {message}"),
        (Some(kind), _) => kind,
        (None, Some(message)) => message,
        (None, None) => "unknown server error".to_string(),
    };
    DictationError::Protocol(detail)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_response, ingest_message, render_transcript, Ingest, LiveTranscript, TranscriptToken,
    };

    fn token(text: &str, is_final: bool) -> TranscriptToken {
        TranscriptToken {
            text: text.to_string(),
            is_final,
        }
    }

    fn empty() -> LiveTranscript {
        LiveTranscript::empty()
    }

    #[test]
    fn finals_append_once_and_later_partials_do_not_duplicate_committed() {
        let mut state = empty();
        apply_response(&mut state, &[token("Hello", true), token(" wor", false)]);
        assert_eq!(state.committed, "Hello");
        assert_eq!(state.partial, " wor");

        // A later response's non-finals replace `partial` and are not appended
        // onto text that was already committed.
        apply_response(&mut state, &[token(" world", false)]);
        assert_eq!(state.committed, "Hello");
        assert_eq!(state.partial, " world");

        apply_response(&mut state, &[token(" world", true), token("!", false)]);
        assert_eq!(state.committed, "Hello world");
        assert_eq!(state.partial, "!");

        apply_response(&mut state, &[token("?", false)]);
        assert_eq!(state.committed, "Hello world");
        assert_eq!(state.partial, "?");
        assert_eq!(render_transcript(&state), "Hello world?");
    }

    #[test]
    fn finals_in_one_response_append_in_order_and_clear_partial_when_absent() {
        let mut state = empty();
        apply_response(&mut state, &[token("ab", false)]);
        apply_response(&mut state, &[token("ab", true), token("c", true)]);
        assert_eq!(state.committed, "abc");
        assert_eq!(state.partial, "");
        // Applying a response that does not repeat those finals leaves them once.
        apply_response(&mut state, &[token("d", false)]);
        assert_eq!(state.committed, "abc");
        assert_eq!(state.partial, "d");
    }

    #[test]
    fn render_is_committed_plus_partial_trimmed() {
        let state = LiveTranscript {
            committed: "  hello".to_string(),
            partial: " world  ".to_string(),
        };
        assert_eq!(render_transcript(&state), "hello world");

        let blank = LiveTranscript {
            committed: "  ".to_string(),
            partial: "\n".to_string(),
        };
        assert_eq!(render_transcript(&blank), "");
        assert_eq!(render_transcript(&empty()), "");
    }

    #[test]
    fn json_responses_fold_the_same_way_and_surface_protocol_errors() {
        let mut state = empty();
        let first = ingest_message(
            &mut state,
            r#"{"tokens":[{"text":"Hello","is_final":true},{"text":" wor","is_final":false}]}"#,
        )
        .unwrap();
        assert_eq!(first, Ingest::Continue);
        assert_eq!(state.committed, "Hello");
        assert_eq!(state.partial, " wor");

        let second = ingest_message(
            &mut state,
            r#"{"tokens":[{"text":" world","is_final":false}],"final_audio_proc_ms":1}"#,
        )
        .unwrap();
        assert_eq!(second, Ingest::Continue);
        assert_eq!(state.committed, "Hello");
        assert_eq!(state.partial, " world");

        let done = ingest_message(
            &mut state,
            r#"{"tokens":[{"text":" world","is_final":true}],"finished":true}"#,
        )
        .unwrap();
        assert_eq!(done, Ingest::Finished);
        assert_eq!(render_transcript(&state), "Hello world");

        let marked = ingest_message(
            &mut state,
            r#"{"tokens":[{"text":"<end>","is_final":true},{"text":" more","is_final":true}],"finished":true}"#,
        )
        .unwrap();
        assert_eq!(marked, Ingest::Finished);
        assert_eq!(render_transcript(&state), "Hello world more");

        let error = ingest_message(
            &mut state,
            r#"{"tokens":[],"error_type":"service_unavailable","error_message":"try again"}"#,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "protocol: service_unavailable: try again"
        );
    }
}
