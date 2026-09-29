//! Row decoding helpers and value converters shared by repository
//! implementations.

use std::path::PathBuf;

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::sqlite::{Sqlite, SqliteRow};
use sqlx::{Decode, Row, Type};
use uuid::Uuid;

use notsai_core::{
    CoreError, Meeting, MeetingStatus, Note, NoteKind, SearchHit, TranscriptSegment,
};

/// Read a single column from a row, mapping lookup/decoding failures into a
/// storage error.
pub(crate) fn get<'r, T>(row: &'r SqliteRow, column: &str) -> Result<T, CoreError>
where
    T: Decode<'r, Sqlite> + Type<Sqlite>,
{
    row.try_get(column)
        .map_err(|e| CoreError::storage(format!("failed to read column `{column}`: {e}")))
}

/// Format a timestamp for storage as an RFC 3339 string with nanosecond
/// precision.
pub(crate) fn ts_to_string(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

/// Parse a stored RFC 3339 timestamp back into a UTC timestamp.
pub(crate) fn ts_from_string(value: &str) -> Result<DateTime<Utc>, CoreError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| CoreError::storage(format!("invalid timestamp `{value}`: {e}")))
}

/// Stored snake_case marker for a meeting status.
pub(crate) fn meeting_status_marker(status: MeetingStatus) -> &'static str {
    match status {
        MeetingStatus::Created => "created",
        MeetingStatus::Recording => "recording",
        MeetingStatus::Recorded => "recorded",
        MeetingStatus::Transcribing => "transcribing",
        MeetingStatus::Processing => "processing",
        MeetingStatus::Ready => "ready",
        MeetingStatus::Failed => "failed",
    }
}

/// Parse a stored snake_case marker back into a meeting status.
pub(crate) fn meeting_status_from_marker(value: &str) -> Result<MeetingStatus, CoreError> {
    match value {
        "created" => Ok(MeetingStatus::Created),
        "recording" => Ok(MeetingStatus::Recording),
        "recorded" => Ok(MeetingStatus::Recorded),
        "transcribing" => Ok(MeetingStatus::Transcribing),
        "processing" => Ok(MeetingStatus::Processing),
        "ready" => Ok(MeetingStatus::Ready),
        "failed" => Ok(MeetingStatus::Failed),
        other => Err(CoreError::storage(format!(
            "unknown meeting status `{other}`"
        ))),
    }
}

/// Stored snake_case marker for a note kind.
pub(crate) fn note_kind_marker(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::Summary => "summary",
        NoteKind::Topic => "topic",
        NoteKind::Decision => "decision",
        NoteKind::ActionItem => "action_item",
        NoteKind::Question => "question",
    }
}

/// Parse a stored snake_case marker back into a note kind.
pub(crate) fn note_kind_from_marker(value: &str) -> Result<NoteKind, CoreError> {
    match value {
        "summary" => Ok(NoteKind::Summary),
        "topic" => Ok(NoteKind::Topic),
        "decision" => Ok(NoteKind::Decision),
        "action_item" => Ok(NoteKind::ActionItem),
        "question" => Ok(NoteKind::Question),
        other => Err(CoreError::storage(format!("unknown note kind `{other}`"))),
    }
}

/// Parse a stored UUID string.
fn uuid_from_string(value: &str) -> Result<Uuid, CoreError> {
    Uuid::parse_str(value).map_err(|e| CoreError::storage(format!("invalid uuid `{value}`: {e}")))
}

/// Convert a stored row into a [`Meeting`].
pub(crate) fn row_to_meeting(row: &SqliteRow) -> Result<Meeting, CoreError> {
    Ok(Meeting {
        id: uuid_from_string(&get::<String>(row, "id")?)?,
        title: get::<String>(row, "title")?,
        status: meeting_status_from_marker(&get::<String>(row, "status")?)?,
        started_at: ts_from_string(&get::<String>(row, "started_at")?)?,
        ended_at: get::<Option<String>>(row, "ended_at")?
            .map(|v| ts_from_string(&v))
            .transpose()?,
        duration_secs: get::<Option<i64>>(row, "duration_secs")?.map(|v| v as u64),
        recording_path: get::<Option<String>>(row, "recording_path")?.map(PathBuf::from),
        transcript_path: get::<Option<String>>(row, "transcript_path")?.map(PathBuf::from),
        last_error: get::<Option<String>>(row, "last_error")?,
        created_at: ts_from_string(&get::<String>(row, "created_at")?)?,
        updated_at: ts_from_string(&get::<String>(row, "updated_at")?)?,
    })
}

/// Convert a stored row into a [`Note`].
pub(crate) fn row_to_note(row: &SqliteRow) -> Result<Note, CoreError> {
    Ok(Note {
        id: uuid_from_string(&get::<String>(row, "id")?)?,
        meeting_id: uuid_from_string(&get::<String>(row, "meeting_id")?)?,
        kind: note_kind_from_marker(&get::<String>(row, "kind")?)?,
        content: get::<String>(row, "content")?,
        seq: get::<i64>(row, "seq")?,
        created_at: ts_from_string(&get::<String>(row, "created_at")?)?,
    })
}

/// Convert a stored row into a [`TranscriptSegment`].
pub(crate) fn row_to_transcript_segment(row: &SqliteRow) -> Result<TranscriptSegment, CoreError> {
    Ok(TranscriptSegment {
        seq: get::<i64>(row, "seq")? as u64,
        start: get::<f64>(row, "start")?,
        end: get::<f64>(row, "end")?,
        speaker: get::<Option<String>>(row, "speaker")?,
        text: get::<String>(row, "text")?,
        confidence: get::<Option<f32>>(row, "confidence")?,
    })
}

/// Convert a stored search row into a [`SearchHit`].
///
/// The search queries alias their columns as `meeting_id`, `meeting_title`
/// and `snippet`, so both the transcript and title queries decode through the
/// same helper.
pub(crate) fn search_hit_from_row(row: &SqliteRow) -> Result<SearchHit, CoreError> {
    Ok(SearchHit {
        meeting_id: uuid_from_string(&get::<String>(row, "meeting_id")?)?,
        meeting_title: get::<String>(row, "meeting_title")?,
        snippet: get::<String>(row, "snippet")?,
    })
}

/// Build an FTS5 query expression from a free-text query.
///
/// Each non-empty term is wrapped in quoted prefix syntax (`"term*"`) and the
/// terms are joined with `AND`, so the whole query must match.
pub(crate) fn fts_query(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|term| term.trim_matches('"'))
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{term}*\""))
        .collect()
}
