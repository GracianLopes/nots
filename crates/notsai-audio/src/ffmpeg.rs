//! `ffmpeg` subprocess helpers for encoding and transcoding audio.

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

use crate::error::AudioError;

/// Environment variable that overrides the `ffmpeg` binary to use.
pub const FFMPEG_ENV: &str = "NOTSAI_FFMPEG";

/// The `ffmpeg` binary path, honouring [`FFMPEG_ENV`] and falling back to
/// a PATH lookup.
pub fn ffmpeg_path() -> std::path::PathBuf {
    std::env::var_os(FFMPEG_ENV)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("ffmpeg"))
}

/// Whether an `ffmpeg` binary is available to run.
pub fn ffmpeg_available() -> bool {
    std::process::Command::new(ffmpeg_path())
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Transcode any audio file to a 16 kHz mono signed 16-bit PCM WAV, the format
/// the local transcriber consumes.
pub async fn transcode_to_16k_wav(input: &Path, output: &Path) -> Result<(), AudioError> {
    let input = input.to_string_lossy().into_owned();
    let output = output.to_string_lossy().into_owned();
    run_ffmpeg(&[
        "-y",
        "-i",
        &input,
        "-ac",
        "1",
        "-ar",
        "16000",
        "-c:a",
        "pcm_s16le",
        &output,
    ])
    .await
}

/// Encode any audio file as AAC in an M4A container for the recording store.
pub async fn transcode_to_m4a(input: &Path, output: &Path) -> Result<(), AudioError> {
    let input = input.to_string_lossy().into_owned();
    let output = output.to_string_lossy().into_owned();
    run_ffmpeg(&["-y", "-i", &input, "-c:a", "aac", "-b:a", "128k", &output]).await
}

/// Lightweight background-noise reduction applied while encoding an M4A for
/// the recording store. Uses a static, non-adaptive filter chain so it cannot
/// damage speech: a 100 Hz high-pass removes rumble/hum and `afftdn` with
/// conservative settings suppresses stationary noise.
pub async fn transcode_to_m4a_denoised(input: &Path, output: &Path) -> Result<(), AudioError> {
    let input = input.to_string_lossy().into_owned();
    let output = output.to_string_lossy().into_owned();
    run_ffmpeg(&[
        "-y",
        "-i",
        &input,
        "-af",
        "highpass=f=100,afftdn=nf=-20:nr=10",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        &output,
    ])
    .await
}

async fn run_ffmpeg(args: &[&str]) -> Result<(), AudioError> {
    let output = Command::new(ffmpeg_path())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => AudioError::FfmpegNotFound,
            _ => AudioError::Io(error),
        })?;

    if !output.status.success() {
        return Err(AudioError::FfmpegFailed {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::PathBuf;

    use uuid::Uuid;

    use super::*;
    use crate::dsp;
    use crate::wav;

    fn temp_dir() -> PathBuf {
        env::temp_dir().join(format!("notsai-audio-ffmpeg-test-{}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn transcode_16k_wav_downsamples_input() {
        if !ffmpeg_available() {
            return;
        }
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.wav");
        let output = dir.join("out.wav");

        let samples: Vec<f32> = (0..32_000)
            .map(|i| ((i as f32 / 32_000.0) * std::f32::consts::TAU).sin())
            .collect();
        wav::write_s16le(&input, 32_000, 1, &samples).unwrap();

        transcode_to_16k_wav(&input, &output).await.unwrap();
        assert!(output.exists());
        let (rate, channels, read) = wav::read_s16(&output).unwrap();
        assert_eq!(rate, dsp::TRANSCRIBE_SAMPLE_RATE);
        assert_eq!(channels, 1);
        assert_eq!(read.len(), 16_000);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn transcode_m4a_produces_file() {
        if !ffmpeg_available() {
            return;
        }
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.wav");
        let output = dir.join("out.m4a");

        wav::write_s16le(&input, dsp::TRANSCRIBE_SAMPLE_RATE, 1, &[0.0f32; 1600]).unwrap();

        transcode_to_m4a(&input, &output).await.unwrap();
        assert!(output.metadata().unwrap().len() > 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn transcode_m4a_denoised_produces_file() {
        if !ffmpeg_available() {
            return;
        }
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.wav");
        let output = dir.join("out.m4a");

        wav::write_s16le(&input, dsp::TRANSCRIBE_SAMPLE_RATE, 1, &[0.0f32; 1600]).unwrap();

        transcode_to_m4a_denoised(&input, &output).await.unwrap();
        assert!(output.metadata().unwrap().len() > 0);

        let _ = fs::remove_dir_all(&dir);
    }
}
