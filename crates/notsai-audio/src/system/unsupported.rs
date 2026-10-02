//! System-audio capture stub for platforms with no backend.
//!
//! Linux, macOS, and Windows each have a module; this covers anything else so
//! the crate still builds. Capture reports itself unavailable and the recorder
//! degrades to microphone-only audio with a visible warning.

/// Unavailable on this platform.
pub(crate) struct Backend;

impl Backend {
    pub(crate) fn start() -> Result<Self, String> {
        Err(format!(
            "system audio capture is not implemented on {}",
            std::env::consts::OS
        ))
    }

    pub(crate) fn drain(&mut self) -> Vec<Vec<u8>> {
        Vec::new()
    }

    pub(crate) fn stop(self) -> Vec<Vec<u8>> {
        Vec::new()
    }
}
