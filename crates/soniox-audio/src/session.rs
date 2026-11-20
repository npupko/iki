//! Soniox realtime WebSocket. The microphone buffers PCM until the config
//! text frame has been written; that write is the ready point (no server ack).

use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use tokio::sync::oneshot;
use tokio::time::{timeout, MissedTickBehavior};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::protocol::{ingest_message, Ingest};
use crate::state::Shared;
use crate::DictationError;

pub(crate) const SONIOX_URL: &str = "wss://stt-rt.soniox.com/transcribe-websocket";
/// Bounds the handshake plus the config frame. It stays under the ten seconds
/// of audio the mic queue holds, so nothing recorded meanwhile is dropped.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// A handshake that fails on the transport (or with a 5xx) is tried once more
/// after this pause, inside [`CONNECT_TIMEOUT`].
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(300);
/// Soniox rejects a `context` object over 8,000 tokens, which the docs
/// approximate as 10,000 characters, and then closes the socket. Stay under
/// that approximation so a long dictionary cannot fail the handshake.
/// https://soniox.com/docs/stt/concepts/context
const CONTEXT_CHAR_BUDGET: usize = 9_000;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Runs until Soniox finishes. The time after key-up is bounded by the
/// caller, in [`crate::DictationHandle::finish`].
pub(crate) async fn run_session(
    url: &str,
    api_key: String,
    terms: Vec<String>,
    languages: Vec<String>,
    shared: std::sync::Arc<Shared>,
    ready_tx: oneshot::Sender<Result<(), DictationError>>,
    mut end_rx: oneshot::Receiver<()>,
) -> Result<String, DictationError> {
    // A key-up during connect must not discard the samples already queued.
    // Finish the handshake, then the loop below sees `end_rx` and flushes.
    let config = config_json(&terms, &languages);
    let opened = timeout(CONNECT_TIMEOUT, open_stream(url, &api_key, config))
        .await
        .unwrap_or_else(|_elapsed| {
            Err(DictationError::Network(
                "timed out connecting to Soniox".to_string(),
            ))
        });
    let (mut write, mut read) = match opened {
        Ok(halves) => halves,
        Err(error) => {
            let _ = ready_tx.send(Err(error.clone()));
            return Err(error);
        }
    };
    let _ = ready_tx.send(Ok(()));

    let mut pace = tokio::time::interval(Duration::from_millis(20));
    pace.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;
            _ = &mut end_rx => break,
            incoming = read.next() => {
                match handle_incoming(&mut write, incoming, &shared, &api_key).await {
                    Ok(Ingest::Continue) => {}
                    Ok(Ingest::Finished) => return finish_result(Ok(shared.render()), &shared),
                    Err(error) => return Err(error),
                }
            }
            _ = pace.tick() => {
                if let Err(error) = send_full_chunks(&mut write, &shared, &api_key).await {
                    return Err(error);
                }
            }
        }
    }

    if let Err(error) = flush_audio(&mut write, &shared, &api_key).await {
        return Err(error);
    }
    // An empty binary frame is silence, not end-of-stream. Soniox ends on "".
    if let Err(error) = write.send(Message::text("")).await {
        return Err(network(&error, &api_key));
    }
    let result = wait_until_finished(&mut write, &mut read, &shared, &api_key).await;
    finish_result(result, &shared)
}

/// Keeps whatever was transcribed when the network fails or times out.
pub(crate) fn finish_result(
    result: Result<String, DictationError>,
    shared: &Shared,
) -> Result<String, DictationError> {
    match result {
        Ok(text) if text.is_empty() => match shared.mic_error() {
            Some(message) => Err(DictationError::Microphone(message)),
            None => Ok(text),
        },
        Ok(text) => Ok(text),
        Err(DictationError::Network(message)) => {
            let text = shared.render();
            if text.is_empty() {
                Err(DictationError::Network(message))
            } else {
                Ok(text)
            }
        }
        Err(error) => Err(error),
    }
}

async fn wait_until_finished<W, R>(
    write: &mut W,
    read: &mut R,
    shared: &Shared,
    api_key: &str,
) -> Result<String, DictationError>
where
    W: Sink<Message, Error = WsError> + Unpin,
    R: Stream<Item = Result<Message, WsError>> + Unpin,
{
    loop {
        match read.next().await {
            Some(Ok(Message::Text(text))) => match ingest_owned(&text, shared, api_key)? {
                Ingest::Continue => {}
                Ingest::Finished => return Ok(shared.render()),
            },
            Some(Ok(Message::Ping(payload))) => {
                write.send(Message::Pong(payload)).await.map_err(|error| {
                    DictationError::Network(redact(&error.to_string(), api_key))
                })?;
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) | None => {
                // The server closes after `finished`. If that frame was missed,
                // keep whatever transcript we already folded.
                return Ok(shared.render());
            }
            Some(Ok(_)) => {
                return Err(DictationError::Protocol(
                    "unexpected binary frame from Soniox".to_string(),
                ));
            }
            Some(Err(error)) => return Err(network(&error, api_key)),
        }
    }
}

async fn handle_incoming<W>(
    write: &mut W,
    incoming: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    shared: &Shared,
    api_key: &str,
) -> Result<Ingest, DictationError>
where
    W: Sink<Message, Error = WsError> + Unpin,
{
    let message = match incoming {
        Some(Ok(message)) => message,
        Some(Err(error)) => return Err(network(&error, api_key)),
        None => return Err(DictationError::Network("connection closed".to_string())),
    };
    match message {
        Message::Text(text) => ingest_owned(&text, shared, api_key),
        Message::Ping(payload) => {
            write
                .send(Message::Pong(payload))
                .await
                .map_err(|error| network(&error, api_key))?;
            Ok(Ingest::Continue)
        }
        Message::Pong(_) => Ok(Ingest::Continue),
        Message::Close(frame) => {
            let reason = frame
                .map(|frame| frame.reason.to_string())
                .unwrap_or_default();
            if reason.is_empty() {
                Err(DictationError::Network("connection closed".to_string()))
            } else {
                Err(DictationError::Network(format!(
                    "connection closed: {reason}"
                )))
            }
        }
        Message::Binary(_) => Err(DictationError::Protocol(
            "unexpected binary frame from Soniox".to_string(),
        )),
        _ => Err(DictationError::Protocol(
            "unexpected websocket frame from Soniox".to_string(),
        )),
    }
}

fn ingest_owned(text: &str, shared: &Shared, api_key: &str) -> Result<Ingest, DictationError> {
    shared
        .with_transcript(|state| ingest_message(state, text))
        .map_err(|error| redact_error(error, api_key))
}

async fn send_full_chunks<W>(
    write: &mut W,
    shared: &Shared,
    api_key: &str,
) -> Result<(), DictationError>
where
    W: Sink<Message, Error = WsError> + Unpin,
{
    // Bound the burst so a 10s pre-roll does not stall response reads.
    for _ in 0..5 {
        let Some(chunk) = shared.pop_chunk(true) else {
            break;
        };
        write
            .send(Message::binary(chunk))
            .await
            .map_err(|error| network(&error, api_key))?;
    }
    Ok(())
}

async fn flush_audio<W>(write: &mut W, shared: &Shared, api_key: &str) -> Result<(), DictationError>
where
    W: Sink<Message, Error = WsError> + Unpin,
{
    while let Some(chunk) = shared.pop_chunk(false) {
        write
            .send(Message::binary(chunk))
            .await
            .map_err(|error| network(&error, api_key))?;
    }
    Ok(())
}

async fn open_stream(
    url: &str,
    api_key: &str,
    config: String,
) -> Result<(SplitSink<Socket, Message>, SplitStream<Socket>), DictationError> {
    let socket = connect_with_retry(url, api_key).await?;
    let (mut write, read) = socket.split();
    write
        .send(Message::text(config))
        .await
        .map_err(|error| network(&error, api_key))?;
    Ok((write, read))
}

async fn connect_with_retry(url: &str, api_key: &str) -> Result<Socket, DictationError> {
    // rustls 0.23 will not negotiate TLS until a process provider is installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
    match connect_async(soniox_request(url, api_key)?).await {
        Ok((socket, _response)) => return Ok(socket),
        Err(error) if is_transient(&error) => tokio::time::sleep(CONNECT_RETRY_DELAY).await,
        Err(error) => return Err(network(&error, api_key)),
    }
    connect_async(soniox_request(url, api_key)?)
        .await
        .map(|(socket, _response)| socket)
        .map_err(|error| network(&error, api_key))
}

fn soniox_request(
    url: &str,
    api_key: &str,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, DictationError> {
    let mut request = url
        .into_client_request()
        .map_err(|error| network(&error, api_key))?;
    let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|_| {
        DictationError::Network("API key contains invalid header characters".to_string())
    })?;
    authorization.set_sensitive(true);
    request
        .headers_mut()
        .insert(HeaderName::from_static("authorization"), authorization);
    Ok(request)
}

/// Transport failures and server-side 5xx are worth one more try. A 4xx
/// (such as a rejected key) or a bad URL fails the same way every time.
fn is_transient(error: &WsError) -> bool {
    match error {
        WsError::Io(_) | WsError::Tls(_) => true,
        WsError::Http(response) => response.status().is_server_error(),
        _ => false,
    }
}

fn network(error: &impl std::fmt::Display, api_key: &str) -> DictationError {
    DictationError::Network(redact(&error.to_string(), api_key))
}

fn redact_error(error: DictationError, api_key: &str) -> DictationError {
    match error {
        DictationError::Network(message) => DictationError::Network(redact(&message, api_key)),
        DictationError::Protocol(message) => DictationError::Protocol(redact(&message, api_key)),
        DictationError::Microphone(message) => {
            DictationError::Microphone(redact(&message, api_key))
        }
        other => other,
    }
}

/// First WebSocket text frame. The API key stays on the connection header.
/// `context` is omitted until there is at least one term. Terms are the
/// documented vocabulary channel; this does not send `translation_terms`.
/// Language codes are sent, in the given order, as `language_hints`. A
/// non-empty list also sets `language_hints_strict`, which asks Soniox to
/// stay inside that set. https://soniox.com/docs/stt/concepts/language-hints
pub(crate) fn config_json(terms: &[String], languages: &[String]) -> String {
    let terms = limit_terms(terms);
    let languages = limit_languages(languages);
    let mut value = serde_json::json!({
        "model": "stt-rt-v6",
        "audio_format": "pcm_s16le",
        "sample_rate": 16000,
        "num_channels": 1,
        "enable_endpoint_detection": true,
    });
    if !languages.is_empty() {
        value["language_hints"] = serde_json::json!(languages);
        value["language_hints_strict"] = serde_json::json!(true);
    }
    if !terms.is_empty() {
        value["context"] = serde_json::json!({ "terms": terms });
    }
    value.to_string()
}

fn limit_languages(languages: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    for language in languages {
        let language = language.trim().to_ascii_lowercase();
        if language.is_empty() || kept.iter().any(|kept: &String| kept == &language) {
            continue;
        }
        kept.push(language);
    }
    kept
}

/// Keep earlier terms and skip any later term that would push the serialized
/// `context` object past [`CONTEXT_CHAR_BUDGET`].
/// The length is kept as a running sum (the empty object plus each encoded
/// term and its comma), so each term is serialized once.
fn limit_terms(terms: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    let mut len = context_object_len(&[]);
    for term in terms {
        if term.is_empty() {
            continue;
        }
        let item = serde_json::Value::from(term.as_str()).to_string().len()
            + usize::from(!kept.is_empty());
        if len + item > CONTEXT_CHAR_BUDGET {
            continue;
        }
        len += item;
        kept.push(term.clone());
    }
    kept
}

fn context_object_len(terms: &[&str]) -> usize {
    serde_json::json!({ "terms": terms }).to_string().len()
}

fn redact(message: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        message.to_string()
    } else {
        message.replace(api_key, "[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use tokio::sync::oneshot;
    use tokio_tungstenite::tungstenite::http::Response;
    use tokio_tungstenite::tungstenite::Error as WsError;

    use super::{
        config_json, context_object_len, is_transient, limit_terms, run_session,
        CONNECT_RETRY_DELAY, CONTEXT_CHAR_BUDGET,
    };
    use crate::state::Shared;
    use crate::DictationError;

    /// The network error messages from the ready channel and the session result.
    async fn session_errors(url: &str) -> [String; 2] {
        let (ready_tx, ready_rx) = oneshot::channel();
        let (_end_tx, end_rx) = oneshot::channel();
        let result = run_session(
            url,
            "key".into(),
            Vec::new(),
            Vec::new(),
            Arc::new(Shared::new()),
            ready_tx,
            end_rx,
        )
        .await;
        let ready = ready_rx.await.expect("ready is always sent");
        [ready.unwrap_err(), result.unwrap_err()].map(|error| match error {
            DictationError::Network(message) => message,
            other => panic!("expected a network error, got {other:?}"),
        })
    }

    #[tokio::test(start_paused = true)]
    async fn a_handshake_that_never_answers_times_out() {
        // Bound but never accepted: the TCP connect lands in the backlog and
        // the HTTP upgrade response never comes.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        for message in session_errors(&url).await {
            assert!(message.contains("timed out"));
        }
        drop(listener);
    }

    #[tokio::test]
    async fn a_refused_connection_is_tried_twice_then_fails() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        drop(listener);
        let started = Instant::now();
        let messages = session_errors(&url).await;
        assert!(started.elapsed() >= CONNECT_RETRY_DELAY);
        for message in messages {
            assert!(!message.contains("timed out"));
        }
    }

    #[test]
    fn only_transport_failures_and_5xx_are_retried() {
        let http = |status: u16| {
            WsError::Http(Box::new(
                Response::builder().status(status).body(None).unwrap(),
            ))
        };
        assert!(!is_transient(&http(401)));
        assert!(!is_transient(&http(403)));
        assert!(is_transient(&http(503)));
        assert!(is_transient(&WsError::Io(std::io::Error::from(
            std::io::ErrorKind::ConnectionRefused
        ))));
        assert!(!is_transient(&WsError::ConnectionClosed));
    }

    #[test]
    fn empty_terms_omit_context_and_keep_the_pcm_config() {
        let value: serde_json::Value =
            serde_json::from_str(&config_json(&[], &["en".into()])).unwrap();
        assert_eq!(value["model"], "stt-rt-v6");
        assert_eq!(value["audio_format"], "pcm_s16le");
        assert_eq!(value["sample_rate"], 16000);
        assert_eq!(value["num_channels"], 1);
        assert_eq!(value["enable_endpoint_detection"], true);
        assert_eq!(value["language_hints"], serde_json::json!(["en"]));
        assert_eq!(value["language_hints_strict"], true);
        assert!(value.get("context").is_none());
        assert!(value.get("api_key").is_none());
        assert!(value.get("translation").is_none());
        assert!(value.get("translation_terms").is_none());
        assert!(value.get("enable_speaker_diarization").is_none());
        assert!(value.get("enable_language_identification").is_none());
    }

    #[test]
    fn language_hints_keep_the_given_order_and_drop_duplicates() {
        let value: serde_json::Value = serde_json::from_str(&config_json(
            &[],
            &["en".into(), "ru".into(), "EN".into(), "  ".into()],
        ))
        .unwrap();
        assert_eq!(value["language_hints"], serde_json::json!(["en", "ru"]));
        assert_eq!(value["language_hints_strict"], true);
    }

    #[test]
    fn no_languages_omit_hints() {
        let value: serde_json::Value = serde_json::from_str(&config_json(&[], &[])).unwrap();
        assert!(value.get("language_hints").is_none());
        assert!(value.get("language_hints_strict").is_none());
    }

    #[test]
    fn terms_are_a_context_string_array_and_quotes_round_trip() {
        let value: serde_json::Value = serde_json::from_str(&config_json(
            &["Iki".into(), "say \"hi\"".into()],
            &["en".into()],
        ))
        .unwrap();
        assert_eq!(
            value["context"]["terms"],
            serde_json::json!(["Iki", "say \"hi\""])
        );
        assert!(value["context"].get("general").is_none());
        assert!(value["context"].get("text").is_none());
        assert!(value["context"].get("translation_terms").is_none());
    }

    #[test]
    fn terms_stop_before_the_documented_context_budget() {
        let terms: Vec<String> = (0..700)
            .map(|index| format!("term-{index:04}-name"))
            .collect();
        let kept = limit_terms(&terms);
        assert!(kept.len() < terms.len());
        assert_eq!(kept[0], "term-0000-name");
        assert!(
            context_object_len(&kept.iter().map(String::as_str).collect::<Vec<_>>())
                <= CONTEXT_CHAR_BUDGET
        );
        let value: serde_json::Value =
            serde_json::from_str(&config_json(&terms, &["en".into()])).unwrap();
        let sent = value["context"]["terms"].as_array().unwrap();
        assert_eq!(sent.len(), kept.len());
        assert!(serde_json::json!({ "terms": sent }).to_string().len() <= CONTEXT_CHAR_BUDGET);
    }

    #[test]
    fn running_length_matches_reserializing_every_time() {
        let terms: Vec<String> = (0..900)
            .map(|index| match index % 4 {
                0 => format!("say \"hi\" {index}"),
                1 => format!("Привет-{index}"),
                2 => format!("back\\slash\t{index}"),
                _ => "x".repeat(index % 50 + 1),
            })
            .collect();
        let mut oracle: Vec<&str> = Vec::new();
        for term in &terms {
            oracle.push(term);
            if context_object_len(&oracle) > CONTEXT_CHAR_BUDGET {
                oracle.pop();
            }
        }
        assert_eq!(limit_terms(&terms), oracle);
    }
}
