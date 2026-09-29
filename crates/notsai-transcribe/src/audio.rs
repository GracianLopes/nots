//! Decodes audio files into raw PCM samples for the transcription engines.
//!
//! Decoding is delegated to the `ffmpeg` command-line tool, which reads any
//! container/codec the host has built (m4a, MP3, WAV, Ogg, ...) and resamples
//! it to mono 16 kHz `f32le` samples — the input format expected by
//! whisper.cpp.
//!
//! Headerless raw `f32le` PCM (`.raw`, `.pcm`, `.f32le`, `.f32`) cannot be
//! auto-detected by ffmpeg, so such files are decoded explicitly as mono
//! 16 kHz `f32le` instead of guessing from the extension's default demuxer.

use std::path::Path;

use notsai_core::CoreError;

/// The sample rate (Hz) that whisper.cpp expects.
pub const SAMPLE_RATE: u32 = 16_000;

/// The number of bytes in one `f32` sample.
const SAMPLE_BYTES: usize = std::mem::size_of::<f32>();

/// Decodes `path` into mono 16 kHz `f32` PCM samples.
///
/// The file is decoded through `ffmpeg -v error -hide_banner -nostdin -i
/// <path> -f f32le -ac 1 -ar 16000 -acodec pcm_f32le -`. Headerless raw
/// `f32le` PCM files (`.raw`, `.pcm`, `.f32le`, `.f32`) instead get explicit
/// input options `-f f32le -ar 16000 -ac 1` before `-i`, since ffmpeg cannot
/// infer their format from the file itself. A trailing partial sample produced
/// by some encoders is discarded. The caller is expected to provide a file
/// that exists and is decodable; anything else maps to a descriptive
/// [`CoreError`].
pub fn decode(path: &Path) -> Result<Vec<f32>, CoreError> {
    let mut command = std::process::Command::new("ffmpeg");
    command
        .arg("-v")
        .arg("error")
        .arg("-hide_banner")
        .arg("-nostdin");

    if is_raw_pcm(path) {
        command
            .arg("-f")
            .arg("f32le")
            .arg("-ar")
            .arg(SAMPLE_RATE.to_string())
            .arg("-ac")
            .arg("1");
    }

    let output = command
        .arg("-i")
        .arg(path)
        .arg("-f")
        .arg("f32le")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg(SAMPLE_RATE.to_string())
        .arg("-acodec")
        .arg("pcm_f32le")
        .arg("-")
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CoreError::transcription(
                    "ffmpeg was not found on this system; install ffmpeg to decode audio",
                )
            } else {
                CoreError::Io(error)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(CoreError::transcription(format!(
            "ffmpeg failed to decode {}: {}",
            path.display(),
            if stderr.is_empty() {
                "unknown error"
            } else {
                &stderr
            }
        )));
    }

    if output.stdout.is_empty() {
        return Err(CoreError::invalid_input(format!(
            "{} decoded to zero audio samples",
            path.display()
        )));
    }

    Ok(bytes_to_samples(&output.stdout))
}

/// Converts raw little-endian `f32` bytes into PCM samples.
///
/// A trailing fragment that does not fill a full sample is dropped, matching
/// how most decoders emit a final partial block.
fn bytes_to_samples(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(SAMPLE_BYTES)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("exact f32 chunk")))
        .collect()
}

/// Whether `path` points at a headerless raw `f32le` PCM file, which ffmpeg
/// cannot auto-detect and must be told the input format for.
fn is_raw_pcm(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "raw" | "pcm" | "f32le" | "f32"
    )
}

pub fn normalize(samples: &mut [f32]) {
    let Some((rms, peak)) = levels(samples) else {
        return;
    };
    if rms <= 0.002 || rms >= 0.05 {
        return;
    }
    let mut gain = (0.05 / rms).min(16.0);
    if peak * gain > 0.95 {
        gain = (0.95 / peak).clamp(1.0, gain);
    }
    if gain <= 1.0 {
        return;
    }
    for sample in samples.iter_mut() {
        *sample *= gain;
    }
}

fn levels(samples: &[f32]) -> Option<(f32, f32)> {
    if samples.is_empty() {
        return None;
    }
    let mut sum_squares = 0.0_f64;
    let mut peak = 0.0_f32;
    for &sample in samples {
        sum_squares += (sample as f64) * (sample as f64);
        let magnitude = sample.abs();
        if magnitude > peak {
            peak = magnitude;
        }
    }
    let rms = (sum_squares / samples.len() as f64).sqrt() as f32;
    Some((rms, peak))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amplitude: f32, length: usize) -> Vec<f32> {
        (0..length)
            .map(|i| {
                let phase = 2.0 * std::f32::consts::PI * 440.0 * i as f32 / SAMPLE_RATE as f32;
                amplitude * phase.sin()
            })
            .collect()
    }

    #[test]
    fn normalize_boosts_quiet_signal() {
        let mut samples = sine(0.01, SAMPLE_RATE as usize);
        normalize(&mut samples);
        let (rms, _) = levels(&samples).expect("non-empty");
        assert!(rms > 0.03, "quiet signal not boosted enough: {rms}");
        let peak = samples.iter().fold(0.0_f32, |peak, &s| peak.max(s.abs()));
        assert!(peak <= 0.95, "normalized signal clips: peak {peak}");
    }

    #[test]
    fn normalize_leaves_healthy_levels_untouched() {
        let samples = sine(0.1, SAMPLE_RATE as usize);
        let mut normalized = samples.clone();
        normalize(&mut normalized);
        assert_eq!(normalized, samples);
    }

    #[test]
    fn normalize_ignores_silence() {
        let samples = vec![0.0_f32; SAMPLE_RATE as usize];
        let mut normalized = samples.clone();
        normalize(&mut normalized);
        assert_eq!(normalized, samples);
    }

    #[test]
    fn normalize_ignores_empty_input() {
        let mut samples: Vec<f32> = Vec::new();
        normalize(&mut samples);
        assert!(samples.is_empty());
    }

    #[test]
    fn normalize_caps_gain_to_protect_peaks() {
        let mut samples = sine(0.01, SAMPLE_RATE as usize);
        let last = samples.len() - 1;
        samples[last] = 0.3;
        let frame = samples.clone();
        normalize(&mut samples);
        let peak = samples.iter().fold(0.0_f32, |peak, &s| peak.max(s.abs()));
        assert!(peak <= 0.951, "peak guard failed: peak {peak}");
        assert_ne!(samples, frame, "expected gain to be applied");
    }

    fn ffmpeg_present() -> bool {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok()
    }

    #[test]
    fn decode_of_missing_file_errors() {
        let missing = std::env::temp_dir().join("notsai-no-such-file.m4a");
        let error = decode(&missing).unwrap_err();
        assert!(
            error.to_string().contains("ffmpeg"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn sample_rate_is_whisper_compatible() {
        assert_eq!(SAMPLE_RATE, 16_000);
    }

    #[test]
    fn bytes_to_samples_decodes_le_f32() {
        let bytes = [0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(bytes_to_samples(&bytes), vec![1.0_f32, 0.0_f32]);
    }

    #[test]
    fn bytes_to_samples_drops_trailing_partial() {
        let bytes = [0x00, 0x00, 0x80, 0x3f, 0xab];
        assert_eq!(bytes_to_samples(&bytes), vec![1.0_f32]);
    }

    #[test]
    fn decode_reads_raw_f32_pcm() {
        if !ffmpeg_present() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }

        let dir = std::env::temp_dir().join(format!("notsai-audio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("input.raw");

        let expected: Vec<f32> = std::iter::repeat_with(|| 0.0).take(16_000).collect();
        let bytes: Vec<u8> = expected
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        std::fs::write(&path, bytes).unwrap();

        let samples = decode(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(samples.len(), expected.len());
    }
}
