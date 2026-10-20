//! Default-input capture. Audio is downmixed to mono, resampled to 16 kHz, and
//! queued as little-endian s16. The cpal stream stays on this thread so the
//! handle itself does not have to be tied to the CoreAudio thread.

use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};

use crate::resample::LinearResampler;
use crate::state::{f32_to_i16, Shared, TARGET_RATE};
use crate::DictationError;

pub(crate) struct MicCapture {
    stop_tx: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl MicCapture {
    pub(crate) fn start(shared: Arc<Shared>) -> Result<Self, DictationError> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("soniox-mic".to_string())
            .spawn(move || {
                let stream = match open_input(shared) {
                    Ok(stream) => stream,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                if ready_tx.send(Ok(())).is_err() {
                    drop(stream);
                    return;
                }
                // Park until `finish` or drop disconnects the sender.
                let _ = stop_rx.recv();
                drop(stream);
            })
            .map_err(|error| {
                DictationError::Microphone(format!("failed to start capture thread: {error}"))
            })?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop_tx: Some(stop_tx),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                drop(stop_tx);
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                drop(stop_tx);
                let _ = thread.join();
                Err(DictationError::Microphone(
                    "capture thread exited before the microphone was ready".to_string(),
                ))
            }
        }
    }

    pub(crate) fn stop(&mut self) {
        self.stop_tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for MicCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn open_input(shared: Arc<Shared>) -> Result<cpal::Stream, DictationError> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| DictationError::Microphone("no default input device".to_string()))?;
    let supported = device
        .default_input_config()
        .map_err(|error| DictationError::Microphone(error.to_string()))?;
    let sample_format = supported.sample_format();
    let sample_rate = supported.sample_rate();
    let channels = usize::from(supported.channels());
    if sample_rate == 0 || channels == 0 {
        return Err(DictationError::Microphone(
            "default input device reported an empty format".to_string(),
        ));
    }
    let config: StreamConfig = supported.into();
    let stream = match sample_format {
        SampleFormat::I8 => build_stream::<i8>(&device, config, channels, sample_rate, shared)?,
        SampleFormat::I16 => build_stream::<i16>(&device, config, channels, sample_rate, shared)?,
        SampleFormat::I32 => build_stream::<i32>(&device, config, channels, sample_rate, shared)?,
        SampleFormat::I64 => build_stream::<i64>(&device, config, channels, sample_rate, shared)?,
        SampleFormat::U8 => build_stream::<u8>(&device, config, channels, sample_rate, shared)?,
        SampleFormat::U16 => build_stream::<u16>(&device, config, channels, sample_rate, shared)?,
        SampleFormat::U32 => build_stream::<u32>(&device, config, channels, sample_rate, shared)?,
        SampleFormat::U64 => build_stream::<u64>(&device, config, channels, sample_rate, shared)?,
        SampleFormat::F32 => build_stream::<f32>(&device, config, channels, sample_rate, shared)?,
        SampleFormat::F64 => build_stream::<f64>(&device, config, channels, sample_rate, shared)?,
        other => {
            return Err(DictationError::Microphone(format!(
                "unsupported sample format {other}"
            )));
        }
    };
    stream
        .play()
        .map_err(|error| DictationError::Microphone(error.to_string()))?;
    Ok(stream)
}

fn build_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    channels: usize,
    sample_rate: u32,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, DictationError>
where
    T: SizedSample + Copy,
    f32: FromSample<T>,
{
    let mut resampler = LinearResampler::new(sample_rate, TARGET_RATE)
        .ok_or_else(|| DictationError::Microphone(format!("cannot resample {sample_rate} Hz")))?;
    let mut pending = Vec::<i16>::new();
    let error_shared = Arc::clone(&shared);
    let error_callback = move |error: cpal::Error| {
        error_shared.set_mic_error(error.to_string());
    };
    device
        .build_input_stream(
            config,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                if channels == 0 || data.is_empty() {
                    return;
                }
                let frames = data.len() / channels;
                let mut peak = 0.0_f32;
                pending.clear();
                for frame in 0..frames {
                    let mut sum = 0.0_f32;
                    let offset = frame * channels;
                    for channel in 0..channels {
                        sum += <f32 as Sample>::from_sample(data[offset + channel]);
                    }
                    let mono = sum / channels as f32;
                    peak = peak.max(mono.abs());
                    resampler.push(mono, |sample| pending.push(f32_to_i16(sample)));
                }
                shared.push_level(peak);
                shared.push_pcm(&pending);
            },
            error_callback,
            None,
        )
        .map_err(|error| DictationError::Microphone(error.to_string()))
}
