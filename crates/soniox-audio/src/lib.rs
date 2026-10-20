//! Default-microphone capture streamed to the Soniox realtime WebSocket.
//!
//! Call [`DictationHandle::start`] from a Tokio runtime. The handle returns
//! once the microphone is playing. Samples recorded while the socket connects
//! are queued (at most about ten seconds) and sent after the config frame.

mod audio;
mod protocol;
mod resample;
mod session;
mod state;

use std::sync::Arc;

use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::audio::MicCapture;
use crate::state::Shared;

pub use crate::protocol::LiveTranscript;

#[derive(Debug)]
pub struct DictationConfig {
    pub api_key: String,
    /// Spellings Soniox should recognize. Sent as `context.terms` when non-empty.
    pub terms: Vec<String>,
    /// ISO codes in priority order. Sent as `language_hints` when non-empty.
    pub languages: Vec<String>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum DictationError {
    #[error("missing API key")]
    MissingApiKey,
    #[error("microphone: {0}")]
    Microphone(String),
    #[error("network: {0}")]
    Network(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("cancelled")]
    Cancelled,
}

pub struct DictationHandle {
    shared: Arc<Shared>,
    mic: MicCapture,
    end_tx: Option<oneshot::Sender<()>>,
    session: Option<JoinHandle<Result<String, DictationError>>>,
    ready_rx: Option<oneshot::Receiver<Result<(), DictationError>>>,
}

impl DictationHandle {
    /// Opens the microphone and returns as soon as it is playing.
    /// The Soniox socket connects on a background task. [`Self::connect_error`]
    /// reports a failed handshake while the caller is still holding the key.
    pub async fn start(config: DictationConfig) -> Result<Self, DictationError> {
        if config.api_key.is_empty() {
            return Err(DictationError::MissingApiKey);
        }

        let shared = Arc::new(Shared::new());
        let mic = MicCapture::start(Arc::clone(&shared))?;
        let (ready_tx, ready_rx) = oneshot::channel();
        let (end_tx, end_rx) = oneshot::channel();
        let api_key = config.api_key;
        let terms = config.terms;
        let languages = config.languages;
        let session_shared = Arc::clone(&shared);
        let session = tokio::spawn(async move {
            session::run_session(api_key, terms, languages, session_shared, ready_tx, end_rx).await
        });

        Ok(Self {
            shared,
            mic,
            end_tx: Some(end_tx),
            session: Some(session),
            ready_rx: Some(ready_rx),
        })
    }

    /// `Some` when the socket failed. `None` while it is still connecting, or after it is up.
    /// If the ready channel closes without a value, [`Self::finish`] reports that error.
    pub fn connect_error(&mut self) -> Option<DictationError> {
        let rx = self.ready_rx.as_mut()?;
        match rx.try_recv() {
            Ok(Ok(())) => {
                self.ready_rx = None;
                None
            }
            Ok(Err(error)) => {
                self.ready_rx = None;
                Some(error)
            }
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => {
                self.ready_rx = None;
                None
            }
        }
    }

    /// Latest mic peak, 0.0..=1.0, safe from any thread.
    pub fn level(&self) -> f32 {
        self.shared.level()
    }

    /// Mean linear RMS since the last call, 0.0..=1.0. This is the amplitude
    /// the pill turns into dBFS. Safe from any thread.
    pub fn rms(&self) -> f32 {
        self.shared.take_rms()
    }

    /// Committed finals plus the current non-final hypothesis. Safe from any thread.
    pub fn transcript(&self) -> LiveTranscript {
        self.shared.transcript()
    }

    /// Stops the mic, ends the Soniox stream, waits for the finished response (timeout 8s),
    /// and returns the trimmed full transcript (`committed + partial`).
    pub async fn finish(mut self) -> Result<String, DictationError> {
        self.mic.stop();
        if let Some(end_tx) = self.end_tx.take() {
            let _ = end_tx.send(());
        }
        let mut guard = TaskGuard {
            task: self.session.take(),
            disarm: false,
        };
        let result = if let Some(task) = guard.task.as_mut() {
            match task.await {
                Ok(result) => result,
                Err(error) if error.is_cancelled() => Err(DictationError::Cancelled),
                Err(error) => Err(DictationError::Network(format!(
                    "session task failed: {error}"
                ))),
            }
        } else {
            Ok(self.shared.render())
        };
        guard.disarm = true;
        result
    }
}

impl Drop for DictationHandle {
    fn drop(&mut self) {
        self.mic.stop();
        self.end_tx.take();
        if let Some(session) = self.session.take() {
            session.abort();
        }
    }
}

struct TaskGuard {
    task: Option<JoinHandle<Result<String, DictationError>>>,
    disarm: bool,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        if self.disarm {
            return;
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{DictationConfig, DictationError, DictationHandle};

    #[tokio::test]
    async fn empty_api_key_is_rejected_before_the_microphone_opens() {
        let started = DictationHandle::start(DictationConfig {
            api_key: String::new(),
            terms: Vec::new(),
            languages: Vec::new(),
        })
        .await;
        match started {
            Err(error) => {
                assert!(matches!(error, DictationError::MissingApiKey));
                assert_eq!(error.to_string(), "missing API key");
            }
            Ok(_) => panic!("empty API key must fail before the microphone opens"),
        }
    }
}
