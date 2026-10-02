//! Small digital-signal helpers used before transcription.

use std::collections::VecDeque;

/// Sample rate (Hz) expected by the transcriber and produced by
/// [`TranscribePcm`](crate::TranscribePcm).
pub const TRANSCRIBE_SAMPLE_RATE: u32 = 16_000;

/// Sample rate (Hz) of the mixed microphone/system recording.
pub const MIX_SAMPLE_RATE: u32 = 48_000;

/// Gain applied to microphone audio before mixing.
pub const MIC_GAIN: f32 = 0.6;

/// Gain applied to system (loopback) audio before mixing.
pub const SYSTEM_GAIN: f32 = 1.0;

/// How far a single source may run ahead of the microphone, in samples at
/// [`MIX_SAMPLE_RATE`], before the oldest samples are dropped to bound latency.
const MAX_SOURCE_LEAD: usize = MIX_SAMPLE_RATE as usize;

/// Downmix interleaved samples to a single channel by averaging each frame.
///
/// Samples with fewer than two channels are returned unchanged.
pub fn interleaved_to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let channels = channels as usize;
    let frames = samples.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for frame in samples.chunks_exact(channels) {
        mono.push(frame.iter().sum::<f32>() / channels as f32);
    }
    mono
}

/// Resample a mono signal with linear interpolation.
///
/// When the source and target rates match, the source is returned verbatim.
/// Downsampling keeps every `to_rate / from_rate`-th output aligned with the
/// original grid, which makes the result deterministic and testable.
pub fn resample_linear(src: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if src.is_empty() || from_rate == 0 || to_rate == 0 || from_rate == to_rate {
        return src.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = ((src.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 / ratio;
        let floor = pos.floor() as usize;
        let ceil = (floor + 1).min(src.len() - 1);
        let frac = (pos - floor as f64) as f32;
        let a = src[floor];
        let b = src[ceil];
        out.push(a + (b - a) * frac);
    }
    out
}

/// Decode interleaved little-endian 16-bit PCM into mono `f32` samples.
///
/// System loopback backends hand out raw `s16le` frames; this converts them in
/// one pass and averages each frame so the mixer only ever sees mono. Trailing
/// bytes that do not form a complete frame are ignored, so a short read is
/// never misaligned.
pub fn s16le_to_mono_f32(bytes: &[u8], channels: u16) -> Vec<f32> {
    let channels = usize::from(channels.max(1));
    let frame_bytes = channels * 2;
    let frames = bytes.len() / frame_bytes;
    let mut mono = Vec::with_capacity(frames);
    for frame in bytes[..frames * frame_bytes].chunks_exact(frame_bytes) {
        let sum = frame
            .chunks_exact(2)
            .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / f32::from(i16::MAX))
            .sum::<f32>();
        mono.push(sum / channels as f32);
    }
    mono
}

/// Chunked linear resampler that keeps phase across calls.
///
/// Live capture delivers samples in callback-sized blocks whose size does not
/// divide the resampling ratio, so a stateful position is required to avoid
/// the discontinuities a per-chunk [`resample_linear`] would introduce.
/// Feeding the same signal in different chunk sizes yields the same output.
pub struct StreamingResampler {
    from_rate: u32,
    to_rate: u32,
    /// Unconsumed input samples; `pos` indexes into this buffer.
    pending: VecDeque<f32>,
    /// Fractional read position of the next output sample within `pending`.
    pos: f64,
}

impl StreamingResampler {
    /// Create a resampler converting from `from_rate` to `to_rate`.
    ///
    /// A zero rate or a matching rate disables interpolation; input is then
    /// passed through unchanged.
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        Self {
            from_rate,
            to_rate,
            pending: VecDeque::new(),
            pos: 0.0,
        }
    }

    /// `true` when the configured rates differ, so interpolation will run.
    pub fn is_active(&self) -> bool {
        self.from_rate != 0 && self.to_rate != 0 && self.from_rate != self.to_rate
    }

    /// Resample the next block of mono samples.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if !self.is_active() {
            return input.to_vec();
        }

        let ratio = f64::from(self.from_rate) / f64::from(self.to_rate);
        self.pending.extend(input.iter().copied());

        let mut out = Vec::with_capacity((input.len() as f64 / ratio) as usize + 2);
        let last = self.pending.len().saturating_sub(1);
        while self.pos < last as f64 {
            let index = self.pos.floor() as usize;
            let frac = (self.pos - index as f64) as f32;
            let current = self.pending[index];
            let next = self.pending[index + 1];
            out.push(current + (next - current) * frac);
            self.pos += ratio;
        }

        // `pos` overshoots `last` by up to one ratio step, so clamp the drain
        // to what is actually buffered.
        let consumed = (self.pos.floor() as usize).min(self.pending.len());
        if consumed > 0 {
            self.pending.drain(..consumed);
            self.pos -= consumed as f64;
        }
        out
    }
}

/// Saturating limiter for the summed signal.
///
/// `tanh` is close to the identity below `0.5` and asymptotes at `±1`, so it
/// removes summation clipping artefacts without audibly compressing speech.
pub fn soft_clip(sample: f32) -> f32 {
    sample.tanh()
}

/// Combines a microphone stream and a system-audio stream into one mono signal.
///
/// Sources may arrive at different native rates and with different timing.
/// Each source is resampled to [`MIX_SAMPLE_RATE`] and buffered; the
/// microphone acts as the clock, so a missing system buffer yields silence for
/// that frame instead of stretching or dropping microphone audio.
pub struct Mixer {
    mic_gain: f32,
    system_gain: f32,
    mic_resampler: StreamingResampler,
    system_resampler: StreamingResampler,
    mic: VecDeque<f32>,
    system: VecDeque<f32>,
}

impl Mixer {
    /// Create a mixer targeting `out_rate` with the default gains.
    pub fn new(out_rate: u32, mic_rate: u32, system_rate: u32) -> Self {
        Self::with_gains(out_rate, mic_rate, system_rate, MIC_GAIN, SYSTEM_GAIN)
    }

    /// Create a mixer with explicit per-source gains.
    pub fn with_gains(
        out_rate: u32,
        mic_rate: u32,
        system_rate: u32,
        mic_gain: f32,
        system_gain: f32,
    ) -> Self {
        Self {
            mic_gain,
            system_gain,
            mic_resampler: StreamingResampler::new(mic_rate, out_rate),
            system_resampler: StreamingResampler::new(system_rate, out_rate),
            mic: VecDeque::new(),
            system: VecDeque::new(),
        }
    }

    /// Queue a block of mono microphone samples and return any mixed output.
    pub fn push_mic(&mut self, samples: &[f32]) -> Vec<f32> {
        self.mic.extend(self.mic_resampler.process(samples));
        self.mix()
    }

    /// Queue a block of mono system samples and return any mixed output.
    pub fn push_system(&mut self, samples: &[f32]) -> Vec<f32> {
        self.system.extend(self.system_resampler.process(samples));
        self.trim_system_lead();
        Vec::new()
    }

    /// Mix as many frames as the microphone has queued.
    fn mix(&mut self) -> Vec<f32> {
        let frames = self.mic.len();
        if frames == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(frames);
        for index in 0..frames {
            let mic = self.mic[index] * self.mic_gain;
            let system = self.system.get(index).copied().unwrap_or(0.0) * self.system_gain;
            out.push(soft_clip(mic + system));
        }
        self.mic.drain(..frames);
        let consumed = frames.min(self.system.len());
        self.system.drain(..consumed);
        out
    }

    /// Drop system audio that has drifted too far ahead of the microphone.
    fn trim_system_lead(&mut self) {
        if self.system.len() > MAX_SOURCE_LEAD {
            self.system.drain(..self.system.len() - MAX_SOURCE_LEAD);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_downmix_averages_stereo_frames() {
        let samples = [1.0, 0.0, 3.0, 1.0];
        assert_eq!(interleaved_to_mono(&samples, 2), vec![0.5, 2.0]);
    }

    #[test]
    fn mono_downmix_passes_mono_through() {
        let samples = [0.2, 0.5, -0.1];
        assert_eq!(interleaved_to_mono(&samples, 1), samples);
    }

    #[test]
    fn resample_same_rate_is_identity() {
        let src: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        assert_eq!(resample_linear(&src, 48_000, 48_000), src);
    }

    #[test]
    fn resample_downsampling_places_grid_points() {
        let src = (0..48_000)
            .map(|i| (i as f32 * 0.01).cos())
            .collect::<Vec<_>>();
        let out = resample_linear(&src, 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        for i in (0..out.len()).step_by(997) {
            let original = i * 3;
            assert!((out[i] - src[original]).abs() < 1e-4);
        }
    }

    #[test]
    fn resample_empty_input_stays_empty() {
        assert!(resample_linear(&[], 48_000, 16_000).is_empty());
    }

    #[test]
    fn s16le_decodes_and_downmixes_stereo_frames() {
        // Two stereo frames at +1.0/0.0 and 0.0/+0.5, which downmix to +0.5
        // and +0.25.
        let bytes = [0xFF, 0x7F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40];
        let mono = s16le_to_mono_f32(&bytes, 2);
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.5).abs() < 1e-3, "{}", mono[0]);
        assert!((mono[1] - 0.25).abs() < 1e-3, "{}", mono[1]);
    }

    #[test]
    fn s16le_ignores_trailing_partial_frame() {
        // One complete mono frame plus a single stray byte.
        let bytes = [0x00, 0x40, 0xFF];
        let mono = s16le_to_mono_f32(&bytes, 1);
        assert_eq!(mono.len(), 1);
        assert!((mono[0] - 0.5).abs() < 1e-3, "{}", mono[0]);
    }

    #[test]
    fn s16le_handles_empty_and_zero_channels() {
        assert!(s16le_to_mono_f32(&[], 2).is_empty());
        // A zero channel count must not divide by zero.
        assert_eq!(s16le_to_mono_f32(&[0x00, 0x40], 0).len(), 1);
    }

    #[test]
    fn streaming_resampler_matches_batch_resample() {
        let src = (0..48_000)
            .map(|i| (i as f32 * 0.01).cos())
            .collect::<Vec<_>>();

        let mut streaming = StreamingResampler::new(48_000, 16_000);
        let mut out = Vec::new();
        for chunk in src.chunks(1_000) {
            out.extend(streaming.process(chunk));
        }

        let expected = resample_linear(&src, 48_000, 16_000);
        assert!(
            out.len().abs_diff(expected.len()) <= 1,
            "chunked output must not drift"
        );
        for (actual, want) in out.iter().zip(expected.iter()) {
            assert!((actual - want).abs() < 1e-4, "{actual} vs {want}");
        }
    }

    #[test]
    fn streaming_resampler_chunk_size_does_not_change_output() {
        let src = (0..9_000)
            .map(|i| (i as f32 * 0.003).sin())
            .collect::<Vec<_>>();

        let run = |chunk_size: usize| {
            let mut resampler = StreamingResampler::new(48_000, 44_100);
            let mut out = Vec::new();
            for chunk in src.chunks(chunk_size) {
                out.extend(resampler.process(chunk));
            }
            out
        };

        let small = run(64);
        let large = run(4_096);
        assert_eq!(small.len(), large.len());
        for (a, b) in small.iter().zip(large.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn streaming_resampler_passes_through_matching_rates() {
        let mut resampler = StreamingResampler::new(48_000, 48_000);
        assert!(!resampler.is_active());
        assert_eq!(resampler.process(&[0.25, -0.5]), vec![0.25, -0.5]);
    }

    #[test]
    fn soft_clip_limits_summation_overflow() {
        assert!(soft_clip(10.0) <= 1.0);
        assert!(soft_clip(-10.0) >= -1.0);
        assert!((soft_clip(0.25) - 0.25).abs() < 1e-2);
    }

    #[test]
    fn mixer_outputs_mic_alone_when_system_is_silent() {
        let mut mixer = Mixer::new(MIX_SAMPLE_RATE, MIX_SAMPLE_RATE, MIX_SAMPLE_RATE);
        let out = mixer.push_mic(&[0.5, -0.5]);
        assert_eq!(out.len(), 2);
        for (sample, mic) in out.iter().zip([0.5f32, -0.5]) {
            let expected = soft_clip(mic * MIC_GAIN);
            assert!((sample - expected).abs() < 1e-3, "{sample} vs {expected}");
        }
    }

    #[test]
    fn mixer_sums_both_sources_and_zero_fills_gaps() {
        let mut mixer = Mixer::new(MIX_SAMPLE_RATE, MIX_SAMPLE_RATE, MIX_SAMPLE_RATE);
        mixer.push_system(&[1.0, 1.0, 1.0, 1.0]);
        let out = mixer.push_mic(&[0.5, 0.5, 0.5, 0.5]);
        assert_eq!(out.len(), 4);
        for sample in &out {
            let expected = soft_clip(0.5 * MIC_GAIN + SYSTEM_GAIN);
            assert!((sample - expected).abs() < 1e-3);
        }
    }

    #[test]
    fn mixer_resamples_system_source_to_output_rate() {
        // A 24 kHz source must emit half as many samples as a 48 kHz mic,
        // so the first mic frames land on already-resampled system audio.
        let mut mixer = Mixer::new(MIX_SAMPLE_RATE, MIX_SAMPLE_RATE, 24_000);
        let system = vec![1.0f32; 96];
        mixer.push_system(&system);
        let out = mixer.push_mic(&[0.0; 48]);
        assert_eq!(out.len(), 48);
        assert!((out[0] - soft_clip(SYSTEM_GAIN)).abs() < 1e-3);
    }

    #[test]
    fn mixer_does_not_grow_without_bound() {
        let mut mixer = Mixer::new(MIX_SAMPLE_RATE, MIX_SAMPLE_RATE, MIX_SAMPLE_RATE);
        for _ in 0..8 {
            mixer.push_system(&vec![0.1; MIX_SAMPLE_RATE as usize]);
        }
        // Trimming keeps the lead bounded to one second of audio.
        assert!(mixer.system.len() <= MAX_SOURCE_LEAD);
    }
}
