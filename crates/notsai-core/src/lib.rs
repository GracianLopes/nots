//! notsai-core — shared domain models, events, settings and service traits.
//!
//! This crate is intentionally dependency-light and framework-free so that
//! the rest of the workspace (storage, audio, transcription, AI, Tauri UI)
//! can share one vocabulary of types without coupling to each other.

#![forbid(unsafe_code)]

mod error;
mod events;
mod model;
mod settings;
mod traits;

pub use error::CoreError;
pub use events::{CoreEvent, EventBus, ProcessingState, RecordingState};
pub use model::{
    AiChatMessage, AiChatRequest, AiChatResponse, AiRole, AudioSource, Meeting, MeetingStatus,
    Note, NoteKind, SearchHit, TranscriptSegment, TranscriptionResult,
};
pub use settings::{
    AIMode, AppSettings, PrivacyLevel, ProviderConfig, ProviderKind, RetentionMode, SttLanguage,
    WhisperModel,
};
pub use traits::{
    AIProvider, MeetingRepository, NoteRepository, SettingsRepository, TranscriptRepository,
    TranscriptionEngine,
};

pub mod prelude {
    pub use crate::{
        AIMode, AiChatMessage, AiChatRequest, AiChatResponse, AiRole, AppSettings, AudioSource,
        CoreError, CoreEvent, EventBus, Meeting, MeetingRepository, MeetingStatus, Note, NoteKind,
        NoteRepository, PrivacyLevel, ProcessingState, ProviderConfig, ProviderKind,
        RecordingState, RetentionMode, SearchHit, SettingsRepository, SttLanguage,
        TranscriptRepository, TranscriptSegment, TranscriptionEngine, TranscriptionResult,
        WhisperModel,
    };
}
