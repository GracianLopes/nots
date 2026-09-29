//! Audio capture and processing for notsAI meetings.
//!
//! This crate is responsible for:
//! - capturing microphone audio during a meeting ([`MicRecorder`]),
//! - converting captures to the 16 kHz mono PCM the transcriber consumes
//!   ([`CapturedPcm::to_transcribe_pcm`]),
//! - encoding the raw capture to M4A for the recording store
//!   ([`ffmpeg::transcode_to_m4a`]) and any recording store format that needs
//!   writing to disk ([`wav`]).
//!
//! Audio is captured from the default input device only; system-audio
//! loopback is intentionally not supported. Imported recordings enter through
//! [`ffmpeg::transcode_to_16k_wav`].

#![forbid(unsafe_code)]

pub mod dsp;
pub mod error;
pub mod ffmpeg;
pub mod recorder;
pub mod wav;

pub use error::AudioError;
pub use recorder::{CapturedPcm, MicRecorder, TranscribePcm};
