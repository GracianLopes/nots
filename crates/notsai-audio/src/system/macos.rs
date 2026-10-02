//! macOS system-audio capture stub.
//!
//! Real capture needs ScreenCaptureKit (`SCStream` with an audio-only
//! configuration) plus the `NSAudioCaptureUsageDescription` / screen-recording
//! TCC prompt. Until that lands, capture reports itself unavailable so the
//! recorder degrades to microphone-only audio with a visible warning instead
//! of failing the whole recording.

/// Unavailable on this platform; see the module docs.
pub(crate) struct Backend;

impl Backend {
    pub(crate) fn start() -> Result<Self, String> {
        Err("system audio capture is not implemented on macOS yet".to_string())
    }

    pub(crate) fn drain(&mut self) -> Vec<Vec<u8>> {
        Vec::new()
    }

    pub(crate) fn stop(self) -> Vec<Vec<u8>> {
        Vec::new()
    }
}
