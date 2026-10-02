//! System ("loopback") audio capture for the desktop platforms we target.
//!
//! The mixer needs a second source next to the microphone: whatever the user
//! is hearing on the default output device. Each platform gets its own backend
//! behind one small API so the recorder can stay platform agnostic:
//!
//! - Linux talks to PipeWire through its PulseAudio-compatible layer
//!   (`pactl`/`parec`), which works on both PipeWire and plain PulseAudio hosts
//!   and avoids linking `libpipewire`.
//! - macOS and Windows are stubs for now; they report themselves unavailable so
//!   the recorder degrades to microphone-only capture and surfaces a warning.
//!
//! Backends deliver raw interleaved PCM bytes; decoding and mono downmixing
//! happen in [`crate::dsp`].

/// Backends for the supported platforms. Each exposes a `Backend` type.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as backend;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as backend;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
use unsupported as backend;

/// Sample rate requested from the system backend.
///
/// The mixer resamples to its own rate anyway, so this only needs to be
/// reasonable; 48 kHz matches the microphone on most hosts.
pub const SYSTEM_SAMPLE_RATE: u32 = 48_000;

/// Channel count requested from the system backend.
///
/// 2 is a safe middle ground: it captures stereo output and is downmixed to
/// mono downstream, and it is available even on mono-only setups.
pub const SYSTEM_CHANNELS: u16 = 2;

/// A live capture of the default system output.
///
/// The backend runs on its own thread, so pulling samples is just
/// [`Self::drain`]. Dropping the source shuts the backend down.
pub struct SystemAudioSource {
    inner: backend::Backend,
}

impl SystemAudioSource {
    /// Start capturing the default system output device.
    ///
    /// Returns a human-readable reason when the platform is unsupported or the
    /// backend cannot start. Callers treat that as a soft failure: recording
    /// continues with microphone audio only.
    pub fn start() -> Result<Self, String> {
        Ok(Self {
            inner: backend::Backend::start()?,
        })
    }

    /// Sample rate of the samples produced by [`Self::drain`].
    pub fn sample_rate(&self) -> u32 {
        SYSTEM_SAMPLE_RATE
    }

    /// Take every chunk captured so far as mono `f32` samples.
    ///
    /// Returns an empty vector when nothing new has arrived, which is the
    /// normal case for most polls.
    pub fn drain(&mut self) -> Vec<f32> {
        let mut mono = Vec::new();
        for chunk in self.inner.drain() {
            mono.extend_from_slice(&crate::dsp::s16le_to_mono_f32(&chunk, SYSTEM_CHANNELS));
        }
        mono
    }

    /// Drain once more and shut the backend down, returning any final samples.
    pub fn stop(mut self) -> Vec<f32> {
        let mut mono = self.drain();
        for chunk in self.inner.stop() {
            mono.extend_from_slice(&crate::dsp::s16le_to_mono_f32(&chunk, SYSTEM_CHANNELS));
        }
        mono
    }
}
