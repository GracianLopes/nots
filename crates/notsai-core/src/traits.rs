//! Service traits implemented by the notsai-* crates.

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    AiChatRequest, AiChatResponse, AppSettings, AudioSource, CoreError, EventBus, Meeting, Note,
    NoteKind, ProviderKind, SearchHit, TranscriptSegment, TranscriptionResult,
};

/// Persistence for meeting records.
#[async_trait]
pub trait MeetingRepository: Send + Sync {
    /// Insert a new meeting.
    async fn create(&self, meeting: &Meeting) -> Result<(), CoreError>;

    /// Look up a single meeting by id.
    async fn get(&self, id: Uuid) -> Result<Option<Meeting>, CoreError>;

    /// List all meetings, most recent first.
    async fn list(&self) -> Result<Vec<Meeting>, CoreError>;

    /// Persist changes to an existing meeting.
    async fn update(&self, meeting: &Meeting) -> Result<(), CoreError>;

    /// Delete a meeting and its derived data.
    ///
    /// When `delete_raw` is true the raw recording and transcript files are
    /// removed as well; otherwise only database rows are deleted.
    async fn delete(&self, id: Uuid, delete_raw: bool) -> Result<(), CoreError>;

    /// Full-text search across meeting titles and transcripts.
    async fn search(&self, query: &str) -> Result<Vec<SearchHit>, CoreError>;
}

/// Persistence for structured notes.
#[async_trait]
pub trait NoteRepository: Send + Sync {
    /// Add a note of the given kind to a meeting.
    async fn add(&self, meeting_id: Uuid, kind: NoteKind, content: &str)
        -> Result<Note, CoreError>;

    /// List a meeting's notes in sequence order.
    async fn list(&self, meeting_id: Uuid) -> Result<Vec<Note>, CoreError>;

    /// Remove all notes for a meeting.
    async fn delete_for_meeting(&self, meeting_id: Uuid) -> Result<(), CoreError>;
}

/// Persistence for transcript segments.
#[async_trait]
pub trait TranscriptRepository: Send + Sync {
    /// Append a single transcript segment.
    async fn append_segment(
        &self,
        meeting_id: Uuid,
        segment: &TranscriptSegment,
    ) -> Result<(), CoreError>;

    /// Append multiple transcript segments in one operation.
    async fn append_segments(
        &self,
        meeting_id: Uuid,
        segments: &[TranscriptSegment],
    ) -> Result<(), CoreError>;

    /// List a meeting's transcript segments in sequence order.
    async fn list(&self, meeting_id: Uuid) -> Result<Vec<TranscriptSegment>, CoreError>;

    /// Remove all transcript segments for a meeting.
    async fn delete_for_meeting(&self, meeting_id: Uuid) -> Result<(), CoreError>;
}

/// Load/save of application settings.
#[async_trait]
pub trait SettingsRepository: Send + Sync {
    /// Load persisted settings, falling back to defaults.
    async fn load(&self) -> Result<AppSettings, CoreError>;

    /// Persist settings.
    async fn save(&self, settings: &AppSettings) -> Result<(), CoreError>;
}

/// Turns audio into a transcript, emitting progress along the way.
#[async_trait]
pub trait TranscriptionEngine: Send + Sync {
    /// Transcribe the given audio source for a meeting, publishing progress on
    /// the bus.
    async fn transcribe(
        &self,
        meeting_id: Uuid,
        source: AudioSource,
        event_bus: &EventBus,
    ) -> Result<TranscriptionResult, CoreError>;
}

/// A cloud AI provider that turns transcripts into notes.
#[async_trait]
pub trait AIProvider: Send + Sync {
    /// The provider kind this implementation talks to.
    fn kind(&self) -> ProviderKind;

    /// Complete a chat request against the provider.
    async fn chat(&self, request: AiChatRequest) -> Result<AiChatResponse, CoreError>;
}
