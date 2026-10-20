//! Streaming linear resampler. One output sample is emitted at input position
//! `n * from_rate / to_rate`, interpolated between the surrounding input samples.

use std::collections::VecDeque;

#[derive(Debug)]
pub(crate) struct LinearResampler {
    from_rate: u32,
    to_rate: u32,
    next_out: u64,
    /// Absolute input index of `samples[0]`.
    base: u64,
    samples: VecDeque<f32>,
}

impl LinearResampler {
    pub(crate) fn new(from_rate: u32, to_rate: u32) -> Option<Self> {
        if from_rate == 0 || to_rate == 0 {
            return None;
        }
        Some(Self {
            from_rate,
            to_rate,
            next_out: 0,
            base: 0,
            samples: VecDeque::new(),
        })
    }

    pub(crate) fn push(&mut self, sample: f32, mut emit: impl FnMut(f32)) {
        self.samples.push_back(sample);
        self.drain(&mut emit);
    }

    fn drain(&mut self, emit: &mut impl FnMut(f32)) {
        let from = u128::from(self.from_rate);
        let to = u128::from(self.to_rate);
        loop {
            let numerator = u128::from(self.next_out) * from;
            let index = (numerator / to) as u64;
            let fraction = (numerator % to) as u64;
            if index < self.base {
                self.next_out += 1;
                continue;
            }
            let relative = (index - self.base) as usize;
            let value = if fraction == 0 {
                if relative >= self.samples.len() {
                    break;
                }
                self.samples[relative]
            } else if relative + 1 >= self.samples.len() {
                break;
            } else {
                let left = self.samples[relative];
                let right = self.samples[relative + 1];
                let t = fraction as f32 / self.to_rate as f32;
                left + (right - left) * t
            };
            emit(value);
            self.next_out += 1;
            self.discard_consumed();
        }
    }

    fn discard_consumed(&mut self) {
        let from = u128::from(self.from_rate);
        let to = u128::from(self.to_rate);
        let numerator = u128::from(self.next_out) * from;
        let index = (numerator / to) as u64;
        if index <= self.base {
            return;
        }
        let drop_count = ((index - self.base) as usize).min(self.samples.len());
        if drop_count == 0 {
            return;
        }
        self.samples.drain(..drop_count);
        self.base += drop_count as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::LinearResampler;

    fn resample(from: u32, to: u32, input: &[f32]) -> Vec<f32> {
        let mut resampler = LinearResampler::new(from, to).expect("rates");
        let mut output = Vec::new();
        for sample in input {
            resampler.push(*sample, |value| output.push(value));
        }
        output
    }

    #[test]
    fn identity_rate_emits_each_input_sample() {
        let input = [0.0, 0.25, -0.5, 1.0];
        assert_eq!(resample(16_000, 16_000, &input), input);
    }

    #[test]
    fn integer_downsample_keeps_every_third_sample() {
        let input: Vec<f32> = (0..9).map(|value| value as f32).collect();
        assert_eq!(resample(48_000, 16_000, &input), vec![0.0, 3.0, 6.0]);
    }

    #[test]
    fn upsample_interpolates_between_neighbors() {
        // 16 kHz -> 48 kHz places outputs at 0, 1/3, 2/3, 1, ...
        let output = resample(16_000, 48_000, &[0.0, 3.0]);
        assert_eq!(output.len(), 4);
        assert!((output[0] - 0.0).abs() < 1e-5);
        assert!((output[1] - 1.0).abs() < 1e-5);
        assert!((output[2] - 2.0).abs() < 1e-5);
        assert!((output[3] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn constant_signal_stays_constant_when_downsampled() {
        let input = vec![0.4_f32; 48];
        let output = resample(48_000, 16_000, &input);
        assert!(!output.is_empty());
        assert!(output.iter().all(|sample| (*sample - 0.4).abs() < 1e-5));
    }
}
