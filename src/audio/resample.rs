use anyhow::{Result, anyhow};
use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

/// Wraps rubato 2.0's FFT-based resampler for streaming use.
///
/// `rubato` operates on fixed-size chunks. This wrapper handles buffering
/// input samples until a full chunk is available, then resamples in bulk.
/// All buffers are pre-allocated — zero heap allocation in the hot path.
pub struct StreamResampler {
    resampler: Fft<f32>,
    input_buf: Vec<f32>,
    output_buf: Vec<f32>,
    chunk_staging: Vec<f32>,
    process_output: Vec<f32>,
    chunk_size: usize,
}

impl StreamResampler {
    /// Create a resampler from `from_rate` to `to_rate`.
    /// `chunk_size` controls the internal processing block size — smaller = lower
    /// latency but slightly more overhead. 480 (one RNNoise frame) is a good default.
    pub fn new(from_rate: usize, to_rate: usize, chunk_size: usize) -> Result<Self> {
        // 1 channel mono, 1 sub-chunk, fixed input size
        let resampler =
            Fft::new(from_rate, to_rate, chunk_size, 1, 1, FixedSync::Input).map_err(|e| {
                anyhow!("Failed to create {from_rate} Hz to {to_rate} Hz resampler: {e}")
            })?;
        let max_output = resampler.output_frames_max();
        Ok(Self {
            resampler,
            input_buf: Vec::with_capacity(chunk_size * 2),
            output_buf: vec![0.0f32; max_output],
            chunk_staging: vec![0.0f32; chunk_size],
            process_output: Vec::with_capacity(max_output * 4),
            chunk_size,
        })
    }

    /// Feed input samples and get resampled output.
    /// May return empty if not enough input has accumulated for a full chunk.
    /// Returns a slice into a pre-allocated buffer — zero heap allocation.
    pub fn process(&mut self, input: &[f32]) -> &[f32] {
        self.input_buf.extend_from_slice(input);
        self.process_output.clear();

        while self.input_buf.len() >= self.chunk_size {
            // Copy chunk into staging buffer instead of drain().collect()
            self.chunk_staging
                .copy_from_slice(&self.input_buf[..self.chunk_size]);
            self.input_buf.drain(..self.chunk_size);

            // Ensure output buffer is large enough
            let needed = self.resampler.output_frames_next();
            if self.output_buf.len() < needed {
                self.output_buf.resize(needed, 0.0);
            }

            // rubato 2.0: InterleavedSlice adapters for mono (1 channel)
            let input_adapter =
                InterleavedSlice::new(&self.chunk_staging, 1, self.chunk_size).unwrap();
            let mut output_adapter =
                InterleavedSlice::new_mut(&mut self.output_buf, 1, needed).unwrap();

            match self
                .resampler
                .process_into_buffer(&input_adapter, &mut output_adapter, None)
            {
                Ok((_in_frames, out_frames)) => {
                    self.process_output
                        .extend_from_slice(&self.output_buf[..out_frames]);
                }
                Err(e) => {
                    tracing::warn!("Resampler error: {e}");
                }
            }
        }

        &self.process_output
    }

    /// Reset internal state (call when restarting pipeline).
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.resampler.reset();
        self.input_buf.clear();
    }
}

/// Simple linear interpolation resampler for output-side resampling where
/// latency matters less and chunk alignment is awkward.
pub fn resample_linear_into(input: &[f32], from_rate: f64, to_rate: f64, out: &mut Vec<f32>) {
    if input.is_empty() {
        return;
    }
    let ratio = from_rate / to_rate;
    let out_len = (input.len() as f64 / ratio).ceil() as usize;
    out.reserve(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let lo = src.floor() as usize;
        let hi = (lo + 1).min(input.len() - 1);
        let t = (src - lo as f64) as f32;
        out.push(input[lo] * (1.0 - t) + input[hi] * t);
    }
}

#[cfg(test)]
mod tests {
    use super::StreamResampler;

    #[test]
    fn continuous_resampling_keeps_buffers_bounded() {
        for (source, target) in [(44_100, 48_000), (96_000, 48_000), (48_000, 44_100)] {
            let mut r = StreamResampler::new(source, target, 480).unwrap();
            let input_capacity = r.input_buf.capacity();
            let output_capacity = r.process_output.capacity();
            for _ in 0..10_000 {
                assert!(r.process(&[0.1; 480]).iter().all(|s| s.is_finite()));
                assert!(r.input_buf.len() < 480);
                assert_eq!(r.input_buf.capacity(), input_capacity);
                assert_eq!(r.process_output.capacity(), output_capacity);
            }
        }
    }

    #[test]
    fn streaming_resampler_buffers_fft_startup_and_preserves_the_44100_to_48000_rate() {
        let mut resampler = StreamResampler::new(44_100, 48_000, 480)
            .expect("44.1 kHz to 48 kHz resampler should initialize");
        let input: Vec<f32> = (0..480)
            .map(|index| (index as f32 * 0.02).sin() * 0.25)
            .collect();

        // Rubato's first FFT window is larger than this input block at 44.1 kHz,
        // so the streaming adapter must retain it until a complete window exists.
        assert!(resampler.process(&input).is_empty());

        let mut output_frames = 0;
        for _ in 1..64 {
            let output = resampler.process(&input);
            assert!(output.iter().all(|sample| sample.is_finite()));
            output_frames += output.len();
        }

        assert!(
            output_frames > 0,
            "the stream should emit after its FFT window fills"
        );
        let expected_frames = 480 * 64 * 48_000 / 44_100;
        assert!(
            output_frames.abs_diff(expected_frames) <= 640,
            "output count {output_frames} should track 44.1 kHz -> 48 kHz within one FFT window of {expected_frames} frames"
        );
    }
}
