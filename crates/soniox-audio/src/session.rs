//! Soniox realtime WebSocket. The microphone buffers PCM until the config
//! text frame has been written; that write is the ready point (no server ack).

use std::time::Duration;

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

const SONIOX_URL: &str = "wss://stt-rt.soniox.com/transcribe-websocket";
const CONFIG_JSON: &str = concat!(
    r#"{"model":"stt-rt-v5","audio_format":"pcm_s16le","#,
    r#""sample_rate":16000,"num_channels":1,"enable_endpoint_detection":true}"#
);
const FINISH_TIMEOUT: Duration = Duration::from_secs(8);

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub(crate) async fn run_session(
    api_key: String,
    shared: std::sync::Arc<Shared>,
    ready_tx: oneshot::Sender<Result<(), DictationError>>,
    mut end_rx: oneshot::Receiver<()>,
) -> Result<String, DictationError> {
    // A key-up during connect must not discard the samples already queued.
    // Finish the handshake, then the loop below sees `end_rx` and flushes.
    let socket = match connect_soniox(&api_key).await {
        Ok(socket) => socket,
        Err(error) => {
            let _ = ready_tx.send(Err(error.clone()));
            return Err(error);
        }
    };

    let (mut write, mut read) = socket.split();
    if let Err(error) = write.send(Message::text(CONFIG_JSON)).await {
        let error = network(&error, &api_key);
        let _ = ready_tx.send(Err(error.clone()));
        return Err(error);
    }
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

    let waited = timeout(
        FINISH_TIMEOUT,
        wait_until_finished(&mut write, &mut read, &shared, &api_key),
    )
    .await;
    let result = match waited {
        Ok(result) => result,
        Err(_elapsed) => Ok(shared.render()),
    };
    finish_result(result, &shared)
}

fn finish_result(
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

async fn connect_soniox(api_key: &str) -> Result<Socket, DictationError> {
    // rustls 0.23 will not negotiate TLS until a process provider is installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut request = SONIOX_URL
        .into_client_request()
        .map_err(|error| network(&error, api_key))?;
    let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|_| {
        DictationError::Network("API key contains invalid header characters".to_string())
    })?;
    authorization.set_sensitive(true);
    request
        .headers_mut()
        .insert(HeaderName::from_static("authorization"), authorization);
    match connect_async(request).await {
        Ok((socket, _response)) => Ok(socket),
        Err(error) => Err(network(&error, api_key)),
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

fn redact(message: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        message.to_string()
    } else {
        message.replace(api_key, "[redacted]")
    }
}
