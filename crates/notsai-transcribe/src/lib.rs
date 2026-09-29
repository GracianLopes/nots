//! notsai-transcribe — local speech-to-text (whisper.cpp via `whisper-rs`).
//!
//! Produces [transcript chunks] with per-segment timing and the language of
//! the audio (initial support: English, Hindi, Marathi). Models are
//! downloaded at runtime into the app data dir with explicit user consent.

#![forbid(unsafe_code)]

mod audio;
mod engine;
mod model;
mod model_manager;

pub use engine::WhisperEngine;
pub use model::{
    language_code, model_download_url, model_file_name, model_languages, model_size_bytes,
    MODELS_DIR,
};
pub use model_manager::{Consent, ModelManager};
