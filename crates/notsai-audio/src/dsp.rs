//! Small digital-signal helpers used before transcription.

/// Sample rate (Hz) expected by the transcriber and produced by
/// [`TranscribePcm`](crate::TranscribePcm).
pub const TRANSCRIBE_SAMPLE_RATE: u32 = 16_000;

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
}
