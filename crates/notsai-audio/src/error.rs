//! Error type for the audio layer.

use std::io;

/// Errors produced while capturing, encoding, or converting audio.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// No default microphone was found on this host.
    #[error("no default input device found")]
    NoInputDevice,

    /// The default input device could not be configured.
    #[error("audio device error: {0}")]
    Device(String),

    /// A capture stream could not be built for the default input device.
    #[error("failed to build input stream: {0}")]
    StreamBuild(String),

    /// A capture stream could not be started.
    #[error("failed to start capture stream: {0}")]
    StreamPlay(String),

    /// The capture worker thread terminated without producing audio.
    #[error("capture worker ended unexpectedly: {0}")]
    Capture(String),

    /// `ffmpeg` is required but was not found on the system.
    #[error("ffmpeg not found on PATH (set NOTSAI_FFMPEG to its location)")]
    FfmpegNotFound,

    /// `ffmpeg` ran but exited unsuccessfully.
    #[error("ffmpeg exited with {code:?}: {stderr}")]
    FfmpegFailed {
        /// Process exit code, if reported.
        code: Option<i32>,
        /// Stderr captured from the failed run.
        stderr: String,
    },

    /// WAV writing or reading failed.
    #[error("wave error: {0}")]
    Wav(#[from] hound::Error),

    /// An underlying I/O failure.
    #[error(transparent)]
    Io(#[from] io::Error),
}
