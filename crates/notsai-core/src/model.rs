//! Core entities and data-transfer types used across notsAI crates.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Lifecycle status of a meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingStatus {
    Created,
    Recording,
    Recorded,
    Transcribing,
    Processing,
    Ready,
    Failed,
}

/// A single meeting session with its associated artifact paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meeting {
    pub id: Uuid,
    pub title: String,
    pub status: MeetingStatus,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_secs: Option<u64>,
    pub recording_path: Option<PathBuf>,
    pub transcript_path: Option<PathBuf>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Meeting {
    /// Create a new, not-yet-started meeting.
    pub fn new(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            status: MeetingStatus::Created,
            started_at: now,
            ended_at: None,
            duration_secs: None,
            recording_path: None,
            transcript_path: None,
            last_error: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Kind of a structured note derived from a meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    Summary,
    Topic,
    Decision,
    ActionItem,
    Question,
}

/// A structured note belonging to a meeting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: Uuid,
    pub meeting_id: Uuid,
    pub kind: NoteKind,
    pub content: String,
    pub seq: i64,
    pub created_at: DateTime<Utc>,
}

/// One segment of a meeting transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub seq: u64,
    pub start: f64,
    pub end: f64,
    pub speaker: Option<String>,
    pub text: String,
    pub confidence: Option<f32>,
}

/// The result of transcribing an audio source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub segments: Vec<TranscriptSegment>,
    pub language: Option<String>,
}

/// Where audio for a meeting comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum AudioSource {
    /// A previously captured audio file on disk.
    File { path: PathBuf },
}

impl AudioSource {
    /// The on-disk path behind this source.
    pub fn path(&self) -> &Path {
        match self {
            AudioSource::File { path } => path,
        }
    }
}

/// A search result pointing at a matching meeting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub meeting_id: Uuid,
    pub meeting_title: String,
    pub snippet: String,
}

/// Who authored a chat message passed to an AI provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiRole {
    System,
    User,
    Assistant,
}

/// One message in an AI chat exchange.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiChatMessage {
    pub role: AiRole,
    pub content: String,
}

/// A chat-completion request sent to an AI provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiChatRequest {
    pub model: String,
    pub messages: Vec<AiChatMessage>,
    pub max_tokens: Option<u32>,
}

/// A chat-completion response from an AI provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiChatResponse {
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_new_sets_sane_defaults() {
        let meeting = Meeting::new("standup");
        assert_eq!(meeting.title, "standup");
        assert_eq!(meeting.status, MeetingStatus::Created);
        assert!(meeting.ended_at.is_none());
        assert_eq!(meeting.duration_secs, None);
    }

    #[test]
    fn note_kind_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&NoteKind::ActionItem).unwrap(),
            "\"action_item\""
        );
    }

    #[test]
    fn meeting_status_round_trips() {
        for status in [
            MeetingStatus::Created,
            MeetingStatus::Recording,
            MeetingStatus::Recorded,
            MeetingStatus::Transcribing,
            MeetingStatus::Processing,
            MeetingStatus::Ready,
            MeetingStatus::Failed,
        ] {
            let encoded = serde_json::to_string(&status).unwrap();
            let decoded: MeetingStatus = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, status);
        }
    }

    #[test]
    fn audio_source_round_trips() {
        let source = AudioSource::File {
            path: PathBuf::from("/tmp/a.m4a"),
        };
        let value = serde_json::to_value(&source).unwrap();
        assert_eq!(value["source"], "file");
        assert_eq!(value["path"], "/tmp/a.m4a");

        let decoded: AudioSource = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.path(), Path::new("/tmp/a.m4a"));
    }

    #[test]
    fn transcript_segment_round_trips() {
        let segment = TranscriptSegment {
            seq: 1,
            start: 0.0,
            end: 2.5,
            speaker: Some("A".into()),
            text: "hello".into(),
            confidence: Some(0.98),
        };
        let value = serde_json::to_value(&segment).unwrap();
        let decoded: TranscriptSegment = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, segment);
    }
}
