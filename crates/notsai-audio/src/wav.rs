//! WAV encoding and decoding on top of `hound`.

use std::path::Path;

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

/// Write `samples` (mono or interleaved) as a signed 16-bit PCM WAV.
pub fn write_s16le(
    path: impl AsRef<Path>,
    sample_rate: u32,
    channels: u16,
    samples: &[f32],
) -> Result<(), hound::Error> {
    let spec = WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec)?;
    for &sample in samples {
        writer.write_sample(clamp_to_i16(sample))?;
    }
    writer.finalize()?;
    Ok(())
}

/// Read a WAV file, returning `(sample_rate, channels, samples)`.
pub fn read_s16(path: impl AsRef<Path>) -> Result<(u32, u16, Vec<i16>), hound::Error> {
    let mut reader = WavReader::open(path)?;
    let spec = reader.spec();
    let samples = reader.samples::<i16>().collect::<Result<Vec<_>, _>>()?;
    Ok((spec.sample_rate, spec.channels, samples))
}

fn clamp_to_i16(sample: f32) -> i16 {
    let clamped = sample.clamp(-1.0, 1.0);
    (clamped * (i16::MAX as f32 + 1.0)) as i16
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::PathBuf;

    use uuid::Uuid;

    use super::*;
    use crate::dsp;

    fn temp_wav() -> PathBuf {
        let dir = env::temp_dir().join(format!("notsai-audio-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("tone.wav")
    }

    #[test]
    fn s16_wave_round_trips() {
        let path = temp_wav();
        let samples: Vec<f32> = (0..160)
            .map(|i| ((i as f32 / 160.0) * std::f32::consts::TAU).sin())
            .collect();

        write_s16le(&path, dsp::TRANSCRIBE_SAMPLE_RATE, 1, &samples).unwrap();
        let (rate, channels, read) = read_s16(&path).unwrap();

        assert_eq!(rate, dsp::TRANSCRIBE_SAMPLE_RATE);
        assert_eq!(channels, 1);
        assert_eq!(read.len(), samples.len());
        assert!((read[0] as f32 / i16::MAX as f32 - samples[0]).abs() < 0.002);

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn clamp_keeps_samples_in_range() {
        assert_eq!(clamp_to_i16(2.0), i16::MAX);
        assert_eq!(clamp_to_i16(-2.0), i16::MIN);
        assert_eq!(clamp_to_i16(0.0), 0);
    }
}
