//! Streaming linear resampler for device/48 kHz conversion.
//!
//! Device sample rates are often 44.1 kHz while Opus wants 48 kHz. This
//! resampler keeps a fractional read position and the last input sample across
//! callbacks, so it can process arbitrary device block sizes without gaps.

/// Streaming linear resampler (mono sample stream).
pub struct LinearResampler {
    ratio: f64,
    position: f64,
    last: f32,
    scratch: Vec<f32>,
}

impl LinearResampler {
    /// Create a resampler from `in_rate` to `out_rate`.
    ///
    /// Returns `None` when both rates are equal (bypass) or invalid.
    pub fn new(in_rate: u32, out_rate: u32) -> Option<Self> {
        if in_rate == 0 || out_rate == 0 || in_rate == out_rate {
            return None;
        }
        Some(Self {
            ratio: in_rate as f64 / out_rate as f64,
            position: 0.0,
            last: 0.0,
            scratch: Vec::new(),
        })
    }

    /// Resample a block; returns a slice valid until the next call.
    pub fn process(&mut self, input: &[f32]) -> &[f32] {
        self.scratch.clear();
        if input.is_empty() {
            return &self.scratch;
        }

        // `position` is relative to the start of this input block. A negative
        // position means the interpolation point lies between the previous
        // block's last sample and this block's first sample.
        while self.position < input.len() as f64 {
            let index = self.position.floor();
            let fraction = (self.position - index) as f32;
            let index = index as isize;
            let left = if index <= 0 {
                self.last
            } else {
                input[(index - 1) as usize]
            };
            let right_index = index as usize;
            let right = input.get(right_index).copied().unwrap_or(left);
            self.scratch.push(left + (right - left) * fraction);
            self.position += self.ratio;
        }

        self.position -= input.len() as f64;
        self.last = *input.last().unwrap_or(&self.last);
        &self.scratch
    }

    /// Discard fractional state (for example after a device route change).
    pub fn reset(&mut self) {
        self.position = 0.0;
        self.last = 0.0;
        self.scratch.clear();
    }
}

/// Convert device channel counts to the configured channel count.
///
/// Mono output averages all input channels; stereo output duplicates mono or
/// takes the first two channels. Any input/output pair works; extra output
/// channels are filled with the last available input channel.
pub fn map_channels(input: &[f32], input_channels: usize, output_channels: usize) -> Vec<f32> {
    let in_ch = input_channels.max(1);
    let out_ch = output_channels.max(1);
    if in_ch == out_ch {
        return input.to_vec();
    }
    let frames = input.len() / in_ch;
    let mut output = Vec::with_capacity(frames * out_ch);
    for frame in 0..frames {
        let base = frame * in_ch;
        if out_ch == 1 {
            let sum: f32 = input[base..base + in_ch].iter().sum();
            output.push(sum / in_ch as f32);
        } else {
            for channel in 0..out_ch {
                let source = if in_ch == 1 { 0 } else { channel.min(in_ch - 1) };
                output.push(input[base + source]);
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| (i as f32 * 0.1).sin())
            .collect()
    }

    #[test]
    fn no_resample_when_rates_equal() {
        assert!(LinearResampler::new(48_000, 48_000).is_none());
        assert!(LinearResampler::new(0, 48_000).is_none());
    }

    #[test]
    fn upsamples_441_to_48() {
        let mut resampler = LinearResampler::new(44_100, 48_000).unwrap();
        let input = tone(441);
        let output = resampler.process(&input).to_vec();
        assert!((output.len() as i32 - 480).abs() <= 2);
    }

    #[test]
    fn downsamples_48_to_441() {
        let mut resampler = LinearResampler::new(48_000, 44_100).unwrap();
        let input = tone(480);
        let output = resampler.process(&input).to_vec();
        assert!((output.len() as i32 - 441).abs() <= 2);
    }

    #[test]
    fn streaming_blocks_match_single_block_length() {
        let mut whole = LinearResampler::new(44_100, 48_000).unwrap();
        let input = tone(882);
        let whole_len = whole.process(&input).len();

        let mut split = LinearResampler::new(44_100, 48_000).unwrap();
        let mut split_len = 0;
        for block in input.chunks(147) {
            split_len += split.process(block).len();
        }
        assert!((whole_len as i32 - split_len as i32).abs() <= 2);
    }

    #[test]
    fn mono_downmix_averages_channels() {
        let input = vec![1.0, 3.0, 2.0, 4.0];
        assert_eq!(map_channels(&input, 2, 1), vec![2.0, 3.0]);
    }

    #[test]
    fn mono_duplicates_to_stereo() {
        let input = vec![0.5, -0.5];
        assert_eq!(map_channels(&input, 1, 2), vec![0.5, 0.5, -0.5, -0.5]);
    }
}
