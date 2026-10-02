//! Live microphone capture via `cpal`.
//!
//! The device callback runs on an audio thread and only pushes sample chunks
//! through an `mpsc` channel; a worker thread drains the channel into a
//! contiguous buffer. Recording is open ended and stops when [`MicRecorder::stop`]
//! is called, at which point the captured PCM is returned for downmix and
//! resampling. For an MVP the full buffer is held in memory.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use std::vec::Vec;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tracing::error;

use crate::dsp::{self, Mixer, MIX_SAMPLE_RATE};
use crate::error::AudioError;
use crate::system::SystemAudioSource;

/// Raw interleaved microphone samples at the device's native rate.
#[derive(Debug)]
pub struct CapturedPcm {
    /// Sample rate (Hz) of the raw capture.
    pub sample_rate: u32,
    /// Number of interleaved channels in `samples`.
    pub channels: u16,
    /// Interleaved `f32` samples in `[-1, 1]`.
    pub samples: Vec<f32>,
}

impl CapturedPcm {
    /// Length of the capture in seconds.
    pub fn duration_secs(&self) -> f64 {
        let frames = self.samples.len() / usize::from(self.channels);
        frames as f64 / f64::from(self.sample_rate)
    }

    /// Produce the 16 kHz mono input the transcriber expects.
    pub fn to_transcribe_pcm(&self) -> TranscribePcm {
        let mono = dsp::interleaved_to_mono(&self.samples, self.channels);
        let samples = dsp::resample_linear(&mono, self.sample_rate, dsp::TRANSCRIBE_SAMPLE_RATE);
        TranscribePcm {
            sample_rate: dsp::TRANSCRIBE_SAMPLE_RATE,
            samples,
        }
    }
}

/// Mono 16 kHz PCM ready for the transcriber.
#[derive(Debug)]
pub struct TranscribePcm {
    /// Always [`dsp::TRANSCRIBE_SAMPLE_RATE`].
    pub sample_rate: u32,
    /// Mono `f32` samples in `[-1, 1]`.
    pub samples: Vec<f32>,
}

/// A live microphone recording that is owned until [`stop`](Self::stop)
/// produces the captured audio.
pub struct MicRecorder {
    handle: Option<JoinHandle<Result<CapturedPcm, String>>>,
    stop: Arc<AtomicBool>,
}

/// Everything [`capture_loop`] needs to build and run the input stream.
struct CaptureConfig {
    device: cpal::Device,
    config: cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    channels: cpal::ChannelCount,
    sample_rate: u32,
}

impl MicRecorder {
    /// Start capturing from the host's default input device.
    ///
    /// Fails if no input device exists or its stream cannot be configured.
    pub fn start() -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(AudioError::NoInputDevice)?;
        let config = device
            .default_input_config()
            .map_err(|error| AudioError::Device(error.to_string()))?;

        let sample_format = config.sample_format();
        let channels = config.channels();
        let sample_rate = config.sample_rate().0;

        let (sender, receiver): (Sender<Vec<f32>>, Receiver<Vec<f32>>) = channel();
        let stop = Arc::new(AtomicBool::new(false));

        let handle = thread::Builder::new()
            .name("notsai-capture".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || {
                    capture_loop(
                        CaptureConfig {
                            device: device.clone(),
                            config: config.into(),
                            sample_format,
                            channels,
                            sample_rate,
                        },
                        sender,
                        receiver,
                        stop,
                    )
                }
            })
            .map_err(AudioError::Io)?;

        Ok(Self {
            handle: Some(handle),
            stop,
        })
    }

    /// Stop recording and return the captured audio.
    ///
    /// Blocking; prefer running this off the async runtime or via
    /// [`Self::stop_async`].
    pub fn stop(mut self) -> Result<CapturedPcm, AudioError> {
        self.stop.store(true, Ordering::Relaxed);
        match self.handle.take() {
            Some(handle) => join_capture(handle),
            None => Err(AudioError::Capture("recorder was already stopped".into())),
        }
    }

    /// Stop recording on the blocking thread pool and await the audio.
    pub async fn stop_async(self) -> Result<CapturedPcm, AudioError> {
        tokio::task::spawn_blocking(move || self.stop())
            .await
            .map_err(|error| AudioError::Capture(format!("stop worker failed: {error}")))?
    }
}

impl Drop for MicRecorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn join_capture(
    handle: JoinHandle<Result<CapturedPcm, String>>,
) -> Result<CapturedPcm, AudioError> {
    match handle.join() {
        Ok(result) => result.map_err(AudioError::Capture),
        Err(payload) => Err(AudioError::Capture(format!(
            "capture worker panicked: {payload:?}"
        ))),
    }
}

/// Build and start the microphone input stream, pushing `f32` blocks to `sender`.
///
/// Shared by [`MicRecorder`] and [`MixedRecorder`]: the two differ only in what
/// they do with a callback's block, not in how the stream is created.
fn play_input(cfg: &CaptureConfig, sender: &Sender<Vec<f32>>) -> Result<cpal::Stream, String> {
    let error_fn = |error| error!(?error, "capture stream error");

    let stream = match cfg.sample_format {
        cpal::SampleFormat::F32 => build_input::<f32>(&cfg.device, &cfg.config, sender, error_fn),
        cpal::SampleFormat::I16 => build_input::<i16>(&cfg.device, &cfg.config, sender, error_fn),
        cpal::SampleFormat::U16 => build_input::<u16>(&cfg.device, &cfg.config, sender, error_fn),
        other => return Err(format!("unsupported sample format: {other:?}")),
    }
    .map_err(|error| error.to_string())?;

    stream
        .play()
        .map_err(|error| format!("failed to start capture: {error}"))?;

    Ok(stream)
}

fn capture_loop(
    cfg: CaptureConfig,
    sender: Sender<Vec<f32>>,
    receiver: Receiver<Vec<f32>>,
    stop: Arc<AtomicBool>,
) -> Result<CapturedPcm, String> {
    // Hold the stream for the life of the recording.
    let stream = play_input(&cfg, &sender)?;

    let mut samples: Vec<f32> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        drain_chunks(&receiver, &mut samples);
        thread::sleep(Duration::from_millis(20));
    }
    drain_chunks(&receiver, &mut samples);
    let _ = stream.pause();

    Ok(CapturedPcm {
        sample_rate: cfg.sample_rate,
        channels: cfg.channels,
        samples,
    })
}

fn build_input<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sender: &Sender<Vec<f32>>,
    error_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    <T as cpal::Sample>::Float: Into<f32>,
{
    let sender = sender.clone();
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mut buf: Vec<f32> = Vec::with_capacity(data.len());
            for &sample in data {
                let value: f32 = sample.to_float_sample().into();
                buf.push(value);
            }
            let _ = sender.send(buf);
        },
        error_fn,
        None,
    )
}

fn drain_chunks(receiver: &Receiver<Vec<f32>>, samples: &mut Vec<f32>) {
    while let Ok(chunk) = receiver.try_recv() {
        samples.extend_from_slice(&chunk);
    }
}

/// What [`MixedRecorder::start`] hands back: the recorder plus the reason
/// system audio is missing, if it is.
pub struct MixedStart {
    /// The running recorder. Stop it to obtain the mixed audio.
    pub recorder: MixedRecorder,
    /// `Ok(())` when system audio is flowing, otherwise why it is not. Never
    /// fatal: the recorder captures microphone audio either way.
    pub system_audio: Result<(), String>,
}

/// A live recording that mixes microphone audio with system loopback audio.
///
/// The microphone defines the timeline, since it is the one source we can
/// always count on. A dedicated mixer thread owns both capture streams, pulls
/// chunks as they arrive, and appends the sum to the running buffer. A silent
/// or unavailable system source therefore contributes zeros rather than
/// stalling or desynchronising the recording.
pub struct MixedRecorder {
    handle: Option<JoinHandle<Result<CapturedPcm, String>>>,
    stop: Arc<AtomicBool>,
}

impl MixedRecorder {
    /// Start recording, attempting system capture on the way.
    ///
    /// Fails only when the microphone is unavailable or its stream cannot be
    /// built. When system capture cannot start, the returned
    /// [`MixedStart::system_audio`] explains why and the recording proceeds
    /// with microphone audio alone.
    pub fn start() -> Result<MixedStart, AudioError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(AudioError::NoInputDevice)?;
        let input_config = device
            .default_input_config()
            .map_err(|error| AudioError::Device(error.to_string()))?;

        let sample_format = input_config.sample_format();
        let channels = input_config.channels();
        let sample_rate = input_config.sample_rate().0;

        let system = SystemAudioSource::start();
        let system_audio = match &system {
            Ok(_) => Ok(()),
            Err(reason) => Err(reason.clone()),
        };

        let (sender, receiver): (Sender<Vec<f32>>, Receiver<Vec<f32>>) = channel();
        let stop = Arc::new(AtomicBool::new(false));

        let handle = thread::Builder::new()
            .name("notsai-mix".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || {
                    mix_loop(
                        CaptureConfig {
                            device,
                            config: input_config.into(),
                            sample_format,
                            channels,
                            sample_rate,
                        },
                        sender,
                        receiver,
                        system.ok(),
                        stop,
                    )
                }
            })
            .map_err(AudioError::Io)?;

        Ok(MixedStart {
            recorder: MixedRecorder {
                handle: Some(handle),
                stop,
            },
            system_audio,
        })
    }

    /// Stop recording and return the mixed mono audio at
    /// [`dsp::MIX_SAMPLE_RATE`].
    pub fn stop(mut self) -> Result<CapturedPcm, AudioError> {
        self.stop.store(true, Ordering::Relaxed);
        match self.handle.take() {
            Some(handle) => join_capture(handle),
            None => Err(AudioError::Capture("recorder was already stopped".into())),
        }
    }

    /// Stop recording on the blocking thread pool and await the audio.
    pub async fn stop_async(self) -> Result<CapturedPcm, AudioError> {
        tokio::task::spawn_blocking(move || self.stop())
            .await
            .map_err(|error| AudioError::Capture(format!("stop worker failed: {error}")))?
    }
}

impl Drop for MixedRecorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn mix_loop(
    cfg: CaptureConfig,
    sender: Sender<Vec<f32>>,
    receiver: Receiver<Vec<f32>>,
    mut system: Option<SystemAudioSource>,
    stop: Arc<AtomicBool>,
) -> Result<CapturedPcm, String> {
    // Hold the stream on this thread for the life of the recording.
    let stream = play_input(&cfg, &sender)?;

    let system_rate = system
        .as_ref()
        .map(|source| source.sample_rate())
        .unwrap_or(MIX_SAMPLE_RATE);
    let mut mixer = Mixer::new(MIX_SAMPLE_RATE, cfg.sample_rate, system_rate);
    let mut samples: Vec<f32> = Vec::new();

    while !stop.load(Ordering::Relaxed) {
        if let Some(source) = system.as_mut() {
            let system_samples = source.drain();
            if !system_samples.is_empty() {
                mixer.push_system(&system_samples);
            }
        }

        // The microphone is the clock: only queued mic frames advance time.
        while let Ok(chunk) = receiver.try_recv() {
            let mono = dsp::interleaved_to_mono(&chunk, cfg.channels);
            samples.extend(mixer.push_mic(&mono));
        }

        thread::sleep(Duration::from_millis(20));
    }

    // Shut the system backend down first, then push everything it delivered so
    // the trailing system frames are handed to the mixer while the microphone
    // still has a tail to clock them against. Any system audio that outran the
    // microphone tail is left unclocked and dropped by ignoring the tail output
    // of `push_system`, which matches the rule that only the microphone
    // advances time.
    if let Some(source) = system.take() {
        mixer.push_system(&source.stop());
    }
    while let Ok(chunk) = receiver.try_recv() {
        let mono = dsp::interleaved_to_mono(&chunk, cfg.channels);
        samples.extend(mixer.push_mic(&mono));
    }
    let _ = stream.pause();

    Ok(CapturedPcm {
        sample_rate: MIX_SAMPLE_RATE,
        channels: 1,
        samples,
    })
}
