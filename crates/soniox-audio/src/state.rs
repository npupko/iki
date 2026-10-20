//! Shared mic level, PCM queue, and transcript. The capture callback and the
//! socket task both touch this, so the queue is a short mutex section.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use crate::protocol::{render_transcript, LiveTranscript};

pub(crate) const TARGET_RATE: u32 = 16_000;
pub(crate) const CHUNK_SAMPLES: usize = 1_600;
pub(crate) const CHUNK_BYTES: usize = CHUNK_SAMPLES * 2;
/// About ten seconds of 16 kHz mono s16le.
pub(crate) const MAX_PCM_BYTES: usize = TARGET_RATE as usize * 2 * 10;

pub(crate) struct Shared {
    pcm: Mutex<VecDeque<u8>>,
    level: AtomicU32,
    transcript: Mutex<LiveTranscript>,
    mic_error: Mutex<Option<String>>,
}

impl Shared {
    pub(crate) fn new() -> Self {
        Self {
            pcm: Mutex::new(VecDeque::with_capacity(TARGET_RATE as usize * 2)),
            level: AtomicU32::new(0.0_f32.to_bits()),
            transcript: Mutex::new(LiveTranscript::empty()),
            mic_error: Mutex::new(None),
        }
    }

    pub(crate) fn push_level(&self, peak: f32) {
        let peak = sanitize_level(peak);
        let previous = sanitize_level(f32::from_bits(self.level.load(Ordering::Relaxed)));
        let next = smooth_level(previous, peak);
        self.level.store(next.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn level(&self) -> f32 {
        sanitize_level(f32::from_bits(self.level.load(Ordering::Relaxed)))
    }

    pub(crate) fn push_pcm(&self, samples: &[i16]) {
        if samples.is_empty() {
            return;
        }
        let mut queue = lock(&self.pcm);
        for sample in samples {
            let bytes = sample.to_le_bytes();
            queue.push_back(bytes[0]);
            queue.push_back(bytes[1]);
        }
        let overflow = queue.len().saturating_sub(MAX_PCM_BYTES);
        let overflow = overflow - (overflow % 2);
        if overflow > 0 {
            queue.drain(..overflow);
        }
    }

    /// One chunk, or `None` when a full chunk is required and not yet buffered.
    pub(crate) fn pop_chunk(&self, require_full: bool) -> Option<Vec<u8>> {
        let mut queue = lock(&self.pcm);
        if queue.is_empty() || (require_full && queue.len() < CHUNK_BYTES) {
            return None;
        }
        let mut count = CHUNK_BYTES.min(queue.len());
        count -= count % 2;
        if count == 0 {
            return None;
        }
        Some(queue.drain(..count).collect())
    }

    pub(crate) fn transcript(&self) -> LiveTranscript {
        lock(&self.transcript).clone()
    }

    pub(crate) fn with_transcript<R>(&self, body: impl FnOnce(&mut LiveTranscript) -> R) -> R {
        body(&mut lock(&self.transcript))
    }

    pub(crate) fn render(&self) -> String {
        render_transcript(&self.transcript())
    }

    pub(crate) fn set_mic_error(&self, message: String) {
        let mut slot = lock(&self.mic_error);
        if slot.is_none() {
            *slot = Some(message);
        }
    }

    pub(crate) fn mic_error(&self) -> Option<String> {
        lock(&self.mic_error).clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn sanitize_level(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Instant attack, slow release. Peak is the recent absolute sample magnitude.
pub(crate) fn smooth_level(previous: f32, peak: f32) -> f32 {
    let peak = sanitize_level(peak);
    let previous = sanitize_level(previous);
    let next = if peak >= previous {
        peak
    } else {
        previous * 0.85 + peak * 0.15
    };
    sanitize_level(next)
}

pub(crate) fn f32_to_i16(sample: f32) -> i16 {
    let clamped = if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (clamped * 32767.0).round() as i16
}

#[cfg(test)]
mod tests {
    use super::{f32_to_i16, smooth_level, Shared, CHUNK_BYTES, MAX_PCM_BYTES};

    #[test]
    fn level_attacks_immediately_and_stays_in_range() {
        assert_eq!(smooth_level(0.0, 0.4), 0.4);
        assert!(smooth_level(0.8, 0.2) < 0.8);
        assert!(smooth_level(0.8, 0.2) > 0.2);
        assert_eq!(smooth_level(0.2, 5.0), 1.0);
        assert_eq!(smooth_level(f32::NAN, 0.3), 0.3);
    }

    #[test]
    fn pcm_queue_drops_the_oldest_audio_past_ten_seconds() {
        let shared = Shared::new();
        let mut samples = vec![1_i16; 8];
        samples.extend(std::iter::repeat(2_i16).take(MAX_PCM_BYTES / 2));
        shared.push_pcm(&samples);
        let queued = {
            let queue = shared.pcm.lock().unwrap();
            assert!(queue.len() <= MAX_PCM_BYTES);
            assert_eq!(queue.len() % 2, 0);
            queue.len()
        };
        assert_eq!(queued, MAX_PCM_BYTES);
        let chunk = shared.pop_chunk(true).expect("full chunk");
        assert_eq!(chunk.len(), CHUNK_BYTES);
        // The eight leading 1s were the oldest samples and must have been dropped.
        assert_eq!(i16::from_le_bytes([chunk[0], chunk[1]]), 2);
    }

    #[test]
    fn pcm_conversion_clamps_to_s16() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), 32767);
        assert_eq!(f32_to_i16(-1.0), -32767);
        assert_eq!(f32_to_i16(2.0), 32767);
        assert_eq!(f32_to_i16(f32::NAN), 0);
    }
}
