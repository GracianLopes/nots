//! Domain events and the event bus that carries them to the UI.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::model::TranscriptSegment;

/// State of a meeting recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RecordingState {
    Starting,
    Recording {
        started_at: DateTime<Utc>,
    },
    /// The microphone is recording, but system (loopback) audio could not be
    /// captured, so the capture is microphone-only.
    ///
    /// This is not a failure: the meeting is still being recorded. The UI shows
    /// `reason` as a non-blocking warning so the user knows the recording misses
    /// other participants' audio.
    SystemAudioDegraded {
        reason: String,
    },
    Stopped {
        duration_secs: u64,
    },
    Failed {
        error: String,
    },
}

/// State of background processing (model download, transcription /
/// summarization).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProcessingState {
    Downloading {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    Transcribing,
    Summarizing,
    Complete,
    Failed {
        error: String,
    },
}

/// A domain event emitted while a meeting is being recorded and processed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CoreEvent {
    Recording {
        meeting_id: Uuid,
        state: RecordingState,
    },
    TranscriptChunk {
        meeting_id: Uuid,
        segment: TranscriptSegment,
        /// Language detected for this chunk (e.g. `"en"`, `"hi"`, `"mr"`).
        ///
        /// `None` means detection did not run or the code could not be mapped.
        language: Option<String>,
    },
    TranscriptionProgress {
        meeting_id: Uuid,
        position_secs: f64,
        progress: f32,
    },
    Processing {
        meeting_id: Uuid,
        state: ProcessingState,
    },
}

impl CoreEvent {
    /// The meeting this event concerns.
    pub fn meeting_id(&self) -> Uuid {
        match self {
            CoreEvent::Recording { meeting_id, .. }
            | CoreEvent::TranscriptChunk { meeting_id, .. }
            | CoreEvent::TranscriptionProgress { meeting_id, .. }
            | CoreEvent::Processing { meeting_id, .. } => *meeting_id,
        }
    }
}

/// A cloneable handle for publishing and subscribing to domain events.
///
/// Backed by a `tokio` broadcast channel so multiple subscribers (UI, storage,
/// AI summarizer) each observe every event.
#[derive(Clone, Debug)]
pub struct EventBus {
    tx: broadcast::Sender<CoreEvent>,
}

impl EventBus {
    /// Create a bus with the given channel capacity (minimum one).
    pub fn new(capacity: usize) -> Self {
        Self {
            tx: broadcast::Sender::new(capacity.max(1)),
        }
    }

    /// Subscribe to events flowing through this bus.
    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.tx.subscribe()
    }

    /// Number of currently subscribed receivers.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Publish an event to all current receivers.
    ///
    /// Returns the number of receivers that observed the event. Failing to
    /// deliver to lagging or absent receivers is not an error.
    pub fn publish(&self, event: CoreEvent) -> usize {
        self.tx.send(event).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_round_trip_preserves_tag_and_meeting_id() {
        let id = Uuid::new_v4();
        let event = CoreEvent::Recording {
            meeting_id: id,
            state: RecordingState::Starting,
        };

        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["event"], "recording");
        assert_eq!(value["state"]["state"], "starting");

        let decoded: CoreEvent = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.meeting_id(), id);
    }

    #[test]
    fn meeting_id_is_exposed_for_all_variants() {
        let id = Uuid::new_v4();
        let events = vec![
            CoreEvent::Recording {
                meeting_id: id,
                state: RecordingState::Starting,
            },
            CoreEvent::TranscriptChunk {
                meeting_id: id,
                segment: TranscriptSegment {
                    seq: 1,
                    start: 0.0,
                    end: 1.0,
                    speaker: None,
                    text: "hello".into(),
                    confidence: None,
                },
                language: Some("en".into()),
            },
            CoreEvent::TranscriptionProgress {
                meeting_id: id,
                position_secs: 1.5,
                progress: 0.5,
            },
            CoreEvent::Processing {
                meeting_id: id,
                state: ProcessingState::Transcribing,
            },
        ];

        for event in events {
            assert_eq!(event.meeting_id(), id);
        }
    }

    #[test]
    fn publish_returns_receiver_count() {
        let bus = EventBus::new(16);
        let id = Uuid::new_v4();

        assert_eq!(
            bus.publish(CoreEvent::Processing {
                meeting_id: id,
                state: ProcessingState::Complete
            }),
            0
        );

        let mut rx = bus.subscribe();
        assert_eq!(
            bus.publish(CoreEvent::Processing {
                meeting_id: id,
                state: ProcessingState::Complete
            }),
            1
        );
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn recording_state_round_trips_with_timestamp() {
        let state = RecordingState::Recording {
            started_at: Utc::now(),
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["state"], "recording");
        assert!(value["started_at"].is_string());
    }

    #[test]
    fn recording_state_degraded_round_trips_reason() {
        let state = RecordingState::SystemAudioDegraded {
            reason: "PulseAudio monitor unavailable".into(),
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["state"], "system_audio_degraded");
        assert_eq!(value["reason"], "PulseAudio monitor unavailable");

        let decoded: RecordingState = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, state);
    }
}
