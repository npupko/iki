//! Default-microphone capture streamed to the Soniox realtime WebSocket.
//!
//! Call [`DictationHandle::start`] from a Tokio runtime. No audio leaves the
//! machine until the config text frame has been written; samples recorded
//! during connect are queued (at most about ten seconds).

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
}

impl DictationHandle {
    /// Opens the mic and the Soniox socket. Buffers mic audio until the socket is ready, then streams.
    pub async fn start(config: DictationConfig) -> Result<Self, DictationError> {
        if config.api_key.is_empty() {
            return Err(DictationError::MissingApiKey);
        }

        let shared = Arc::new(Shared::new());
        let mic = MicCapture::start(Arc::clone(&shared))?;
        let (ready_tx, ready_rx) = oneshot::channel();
        let (end_tx, end_rx) = oneshot::channel();
        let api_key = config.api_key;
        let session_shared = Arc::clone(&shared);
        let session = tokio::spawn(async move {
            session::run_session(api_key, session_shared, ready_tx, end_rx).await
        });

        let mut guard = StartGuard {
            mic: Some(mic),
            session: Some(session),
            armed: true,
        };

        match ready_rx.await {
            Ok(Ok(())) => {
                let mic = guard.mic.take().expect("mic still held");
                let session = guard.session.take().expect("session still held");
                guard.armed = false;
                Ok(Self {
                    shared,
                    mic,
                    end_tx: Some(end_tx),
                    session: Some(session),
                })
            }
            Ok(Err(error)) => {
                if let Some(mic) = guard.mic.as_mut() {
                    mic.stop();
                }
                if let Some(session) = guard.session.take() {
                    let _ = session.await;
                }
                guard.armed = false;
                Err(error)
            }
            Err(_dropped) => {
                if let Some(mic) = guard.mic.as_mut() {
                    mic.stop();
                }
                let joined = if let Some(session) = guard.session.take() {
                    session.abort();
                    Some(session.await)
                } else {
                    None
                };
                guard.armed = false;
                match joined {
                    Some(Ok(Err(error))) => Err(error),
                    Some(Ok(Ok(_))) => Err(DictationError::Network(
                        "session ended before it was ready".to_string(),
                    )),
                    _ => Err(DictationError::Cancelled),
                }
            }
        }
    }

    /// Latest mic level, 0.0..=1.0, safe from any thread.
    pub fn level(&self) -> f32 {
        self.shared.level()
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

struct StartGuard {
    mic: Option<MicCapture>,
    session: Option<JoinHandle<Result<String, DictationError>>>,
    armed: bool,
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(mic) = self.mic.as_mut() {
            mic.stop();
        }
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
