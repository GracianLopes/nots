//! Audio capture and processing for notsAI meetings.
//!
//! This crate is responsible for:
//! - capturing microphone audio during a meeting ([`MicRecorder`]),
//! - capturing system (loopback) audio where the platform supports it and
//!   mixing it with the microphone ([`MixedRecorder`]),
//! - converting captures to the 16 kHz mono PCM the transcriber consumes
//!   ([`CapturedPcm::to_transcribe_pcm`]),
//! - encoding the raw capture to M4A for the recording store
//!   ([`ffmpeg::transcode_to_m4a`]) and any recording store format that needs
//!   writing to disk ([`wav`]).
//!
//! [`MixedRecorder`] is the preferred entry point for meetings: it always
//! captures the microphone and treats system audio as optional, reporting why
//! it is missing through [`MixedStart::system_audio`] instead of failing the
//! recording. System capture is implemented on Linux; other platforms report
//! themselves unavailable and degrade to microphone-only audio. Imported
//! recordings enter through [`ffmpeg::transcode_to_16k_wav`].

#![forbid(unsafe_code)]

pub mod dsp;
pub mod error;
pub mod ffmpeg;
pub mod recorder;
pub mod system;
pub mod wav;

pub use error::AudioError;
pub use recorder::{CapturedPcm, MicRecorder, MixedRecorder, MixedStart, TranscribePcm};
