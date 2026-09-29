//! SQLite-backed repository implementations and raw artifact management.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Utc;
use sqlx::sqlite::Sqlite;
use sqlx::SqlitePool;
use uuid::Uuid;

use notsai_core::{
    CoreError, Meeting, MeetingRepository, Note, NoteKind, NoteRepository, SearchHit,
    TranscriptRepository, TranscriptSegment,
};

use crate::db::sqlite_error;
use crate::models::{
    fts_query, meeting_status_marker, note_kind_marker, row_to_meeting, row_to_note,
    row_to_transcript_segment, search_hit_from_row, ts_to_string,
};

/// Name of the raw recording file stored under a meeting's artifact directory.
pub const RECORDING_FILENAME: &str = "recording.m4a";

/// Name of the raw transcript file stored under a meeting's artifact directory.
pub const TRANSCRIPT_FILENAME: &str = "transcript.json";

/// Upper bound on the number of search hits returned for a single query.
pub const MAX_SEARCH_HITS: usize = 50;

/// Sqlite-backed store implementing the notsAI repository traits.
pub struct SqliteStore {
    pool: SqlitePool,
    meetings_dir: Option<PathBuf>,
}

impl SqliteStore {
    /// Open the store on the database at `db_path`, optionally managing raw
    /// artifacts under `meetings_dir`.
    pub async fn open(db_path: &Path, meetings_dir: Option<PathBuf>) -> Result<Self, CoreError> {
        let pool = crate::db::open(db_path).await?;
        Ok(Self { pool, meetings_dir })
    }

    /// Open an ephemeral in-memory store with no artifact directory.
    pub async fn open_in_memory() -> Result<Self, CoreError> {
        let pool = crate::db::open_in_memory().await?;
        Ok(Self {
            pool,
            meetings_dir: None,
        })
    }

    /// The underlying connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The configured raw-artifact directory, if any.
    pub fn meetings_dir(&self) -> Option<&Path> {
        self.meetings_dir.as_deref()
    }

    /// The directory that would hold raw artifacts for `id`, if configured.
    pub fn artifact_dir(&self, id: Uuid) -> Option<PathBuf> {
        self.meetings_dir
            .as_ref()
            .map(|dir| dir.join(id.to_string()))
    }

    /// Whether a meeting with the given id exists.
    async fn meeting_exists(&self, id: Uuid) -> Result<bool, CoreError> {
        let present: i64 =
            sqlx::query_scalar::<Sqlite, i64>("SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?)")
                .bind(id.to_string())
                .fetch_one(&self.pool)
                .await
                .map_err(sqlite_error)?;
        Ok(present != 0)
    }

    /// The next note sequence number for a meeting.
    async fn next_note_seq(&self, meeting_id: Uuid) -> Result<i64, CoreError> {
        let max: i64 = sqlx::query_scalar::<Sqlite, i64>(
            "SELECT COALESCE(MAX(seq), 0) FROM notes WHERE meeting_id = ?",
        )
        .bind(meeting_id.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(sqlite_error)?;
        Ok(max + 1)
    }

    /// Remove the raw recording and transcript files for a meeting, then the
    /// meeting's artifact directory. Directory removal is best-effort: a
    /// directory that still holds files we did not write is left in place
    /// without failing the deletion.
    async fn remove_raw_artifacts(&self, id: Uuid) -> Result<(), CoreError> {
        let Some(dir) = self.artifact_dir(id) else {
            return Ok(());
        };
        remove_file_if_present(&dir.join(RECORDING_FILENAME)).await?;
        remove_file_if_present(&dir.join(TRANSCRIPT_FILENAME)).await?;
        match tokio::fs::remove_dir(&dir).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) if dir.exists() => Ok(()),
            Err(e) => Err(CoreError::Io(e)),
        }
    }
}

/// Render an optional path as its display string for storage.
fn path_to_sql(path: Option<&Path>) -> Option<String> {
    path.map(|p| p.display().to_string())
}

/// Remove a file, treating an already-missing file as success.
async fn remove_file_if_present(path: &Path) -> Result<(), CoreError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CoreError::Io(e)),
    }
}

/// Insert a transcript segment, ignoring duplicates on the `(meeting_id, seq)`
/// primary key so re-transcription retries are idempotent.
async fn insert_segment<'e, E>(
    executor: E,
    meeting_id: Uuid,
    segment: &TranscriptSegment,
) -> Result<(), CoreError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    sqlx::query::<Sqlite>(
        "INSERT OR IGNORE INTO transcript_segments \
         (meeting_id, seq, start, end, speaker, text, confidence) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(meeting_id.to_string())
    .bind(segment.seq as i64)
    .bind(segment.start)
    .bind(segment.end)
    .bind(segment.speaker.as_deref())
    .bind(&segment.text)
    .bind(segment.confidence)
    .execute(executor)
    .await
    .map_err(sqlite_error)?;
    Ok(())
}

#[async_trait]
impl MeetingRepository for SqliteStore {
    async fn create(&self, meeting: &Meeting) -> Result<(), CoreError> {
        sqlx::query::<Sqlite>(
            "INSERT INTO meetings \
             (id, title, status, started_at, ended_at, duration_secs, \
              recording_path, transcript_path, last_error, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(meeting.id.to_string())
        .bind(&meeting.title)
        .bind(meeting_status_marker(meeting.status))
        .bind(ts_to_string(meeting.started_at))
        .bind(meeting.ended_at.map(ts_to_string))
        .bind(meeting.duration_secs.map(|v| v as i64))
        .bind(path_to_sql(meeting.recording_path.as_deref()))
        .bind(path_to_sql(meeting.transcript_path.as_deref()))
        .bind(meeting.last_error.as_deref())
        .bind(ts_to_string(meeting.created_at))
        .bind(ts_to_string(meeting.updated_at))
        .execute(&self.pool)
        .await
        .map_err(sqlite_error)?;
        Ok(())
    }

    async fn get(&self, id: Uuid) -> Result<Option<Meeting>, CoreError> {
        let row = sqlx::query::<Sqlite>(
            "SELECT id, title, status, started_at, ended_at, duration_secs, \
             recording_path, transcript_path, last_error, created_at, updated_at \
             FROM meetings WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(sqlite_error)?;
        row.as_ref().map(row_to_meeting).transpose()
    }

    async fn list(&self) -> Result<Vec<Meeting>, CoreError> {
        let rows = sqlx::query::<Sqlite>(
            "SELECT id, title, status, started_at, ended_at, duration_secs, \
             recording_path, transcript_path, last_error, created_at, updated_at \
             FROM meetings ORDER BY started_at DESC, created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(sqlite_error)?;
        rows.iter().map(row_to_meeting).collect()
    }

    async fn update(&self, meeting: &Meeting) -> Result<(), CoreError> {
        let result = sqlx::query::<Sqlite>(
            "UPDATE meetings \
             SET title = ?, status = ?, started_at = ?, ended_at = ?, \
                 duration_secs = ?, recording_path = ?, transcript_path = ?, \
                 last_error = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(&meeting.title)
        .bind(meeting_status_marker(meeting.status))
        .bind(ts_to_string(meeting.started_at))
        .bind(meeting.ended_at.map(ts_to_string))
        .bind(meeting.duration_secs.map(|v| v as i64))
        .bind(path_to_sql(meeting.recording_path.as_deref()))
        .bind(path_to_sql(meeting.transcript_path.as_deref()))
        .bind(meeting.last_error.as_deref())
        .bind(ts_to_string(meeting.updated_at))
        .bind(meeting.id.to_string())
        .execute(&self.pool)
        .await
        .map_err(sqlite_error)?;

        if result.rows_affected() == 0 {
            if !self.meeting_exists(meeting.id).await? {
                return Err(CoreError::not_found(format!(
                    "meeting {id} not found",
                    id = meeting.id
                )));
            }
            tracing::info!(id = %meeting.id, "update wrote no rows");
        }
        Ok(())
    }

    async fn delete(&self, id: Uuid, delete_raw: bool) -> Result<(), CoreError> {
        if !self.meeting_exists(id).await? {
            return Err(CoreError::not_found(format!("meeting {id} not found")));
        }
        let result = sqlx::query::<Sqlite>("DELETE FROM meetings WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(sqlite_error)?;
        if result.rows_affected() == 0 {
            return Err(CoreError::not_found(format!("meeting {id} not found")));
        }
        if delete_raw {
            self.remove_raw_artifacts(id).await?;
        }
        Ok(())
    }

    async fn search(&self, query: &str) -> Result<Vec<SearchHit>, CoreError> {
        let terms = fts_query(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let match_expr = terms.join(" AND ");
        let limit = MAX_SEARCH_HITS as i64;

        let transcript_rows = sqlx::query::<Sqlite>(
            "SELECT ts.meeting_id AS meeting_id, \
                    m.title AS meeting_title, \
                    snippet(transcripts_fts, 1, '[', ']', '...', 32) AS snippet \
             FROM transcripts_fts \
             JOIN transcript_segments ts ON ts.rowid = transcripts_fts.rowid \
             JOIN meetings m ON m.id = ts.meeting_id \
             WHERE transcripts_fts MATCH ? \
             ORDER BY bm25(transcripts_fts) \
             LIMIT ?",
        )
        .bind(&match_expr)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlite_error)?;

        let title_rows = sqlx::query::<Sqlite>(
            "SELECT m.id AS meeting_id, \
                    m.title AS meeting_title, \
                    snippet(meetings_fts, 0, '[', ']', '...', 32) AS snippet \
             FROM meetings_fts \
             JOIN meetings m ON m.rowid = meetings_fts.rowid \
             WHERE meetings_fts MATCH ? \
             ORDER BY bm25(meetings_fts) \
             LIMIT ?",
        )
        .bind(&match_expr)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlite_error)?;

        let mut hits: Vec<SearchHit> = Vec::with_capacity(MAX_SEARCH_HITS);
        let mut seen: HashSet<Uuid> = HashSet::with_capacity(MAX_SEARCH_HITS);
        for row in transcript_rows.iter().chain(&title_rows) {
            if seen.insert(row_to_search_hit_id(row)?) {
                hits.push(search_hit_from_row(row)?);
            }
        }
        hits.truncate(MAX_SEARCH_HITS);
        Ok(hits)
    }
}

/// Read the meeting id from a search result row for deduplication.
fn row_to_search_hit_id(row: &sqlx::sqlite::SqliteRow) -> Result<Uuid, CoreError> {
    let value: String = crate::models::get(row, "meeting_id")?;
    Uuid::parse_str(&value).map_err(|e| CoreError::storage(format!("invalid uuid `{value}`: {e}")))
}

#[async_trait]
impl NoteRepository for SqliteStore {
    async fn add(
        &self,
        meeting_id: Uuid,
        kind: NoteKind,
        content: &str,
    ) -> Result<Note, CoreError> {
        if !self.meeting_exists(meeting_id).await? {
            return Err(CoreError::not_found(format!(
                "meeting {meeting_id} not found"
            )));
        }
        let seq = self.next_note_seq(meeting_id).await?;
        let note = Note {
            id: Uuid::new_v4(),
            meeting_id,
            kind,
            content: content.to_string(),
            seq,
            created_at: Utc::now(),
        };
        sqlx::query::<Sqlite>(
            "INSERT INTO notes (id, meeting_id, kind, content, seq, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(note.id.to_string())
        .bind(note.meeting_id.to_string())
        .bind(note_kind_marker(note.kind))
        .bind(&note.content)
        .bind(note.seq)
        .bind(ts_to_string(note.created_at))
        .execute(&self.pool)
        .await
        .map_err(sqlite_error)?;
        Ok(note)
    }

    async fn list(&self, meeting_id: Uuid) -> Result<Vec<Note>, CoreError> {
        let rows = sqlx::query::<Sqlite>(
            "SELECT id, meeting_id, kind, content, seq, created_at \
             FROM notes WHERE meeting_id = ? ORDER BY seq ASC",
        )
        .bind(meeting_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(sqlite_error)?;
        rows.iter().map(row_to_note).collect()
    }

    async fn delete_for_meeting(&self, meeting_id: Uuid) -> Result<(), CoreError> {
        sqlx::query::<Sqlite>("DELETE FROM notes WHERE meeting_id = ?")
            .bind(meeting_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(sqlite_error)?;
        Ok(())
    }
}

#[async_trait]
impl TranscriptRepository for SqliteStore {
    async fn append_segment(
        &self,
        meeting_id: Uuid,
        segment: &TranscriptSegment,
    ) -> Result<(), CoreError> {
        if !self.meeting_exists(meeting_id).await? {
            return Err(CoreError::not_found(format!(
                "meeting {meeting_id} not found"
            )));
        }
        insert_segment(&self.pool, meeting_id, segment).await
    }

    async fn append_segments(
        &self,
        meeting_id: Uuid,
        segments: &[TranscriptSegment],
    ) -> Result<(), CoreError> {
        if !self.meeting_exists(meeting_id).await? {
            return Err(CoreError::not_found(format!(
                "meeting {meeting_id} not found"
            )));
        }
        let mut tx = self.pool.begin().await.map_err(sqlite_error)?;
        for segment in segments {
            insert_segment(&mut *tx, meeting_id, segment).await?;
        }
        tx.commit().await.map_err(sqlite_error)
    }

    async fn list(&self, meeting_id: Uuid) -> Result<Vec<TranscriptSegment>, CoreError> {
        let rows = sqlx::query::<Sqlite>(
            "SELECT seq, start, end, speaker, text, confidence \
             FROM transcript_segments WHERE meeting_id = ? ORDER BY seq ASC",
        )
        .bind(meeting_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(sqlite_error)?;
        rows.iter().map(row_to_transcript_segment).collect()
    }

    async fn delete_for_meeting(&self, meeting_id: Uuid) -> Result<(), CoreError> {
        sqlx::query::<Sqlite>("DELETE FROM transcript_segments WHERE meeting_id = ?")
            .bind(meeting_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(sqlite_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{DateTime, Utc};
    use notsai_core::{CoreError, MeetingStatus, NoteKind};

    use super::*;

    /// A unique scratch directory under the system temp directory.
    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("notsai-storage-{name}-{}", Uuid::new_v4()))
    }

    /// Parse an RFC 3339 timestamp into a UTC timestamp.
    fn ts(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    /// A meeting started at the given time.
    fn meeting_at(title: &str, started_at: DateTime<Utc>) -> Meeting {
        let mut meeting = Meeting::new(title);
        meeting.started_at = started_at;
        meeting.updated_at = started_at;
        meeting
    }

    /// A transcript segment with the given sequence and text.
    fn segment(seq: u64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            seq,
            start: 0.0,
            end: 1.0,
            speaker: None,
            text: text.to_string(),
            confidence: Some(1.0),
        }
    }

    #[tokio::test]
    async fn meeting_crud_roundtrip() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let mut meeting = meeting_at("standup", ts("2026-09-17T08:30:00Z"));
        meeting.recording_path = Some(PathBuf::from("/tmp/standup.m4a"));
        meeting.duration_secs = Some(150);

        MeetingRepository::create(&store, &meeting).await.unwrap();

        let loaded = MeetingRepository::get(&store, meeting.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.title, meeting.title);
        assert_eq!(loaded.started_at, meeting.started_at);
        assert_eq!(loaded.recording_path, meeting.recording_path);
        assert_eq!(loaded.duration_secs, Some(150));
        assert_eq!(loaded.status, meeting.status);
        assert!(MeetingRepository::get(&store, Uuid::new_v4())
            .await
            .unwrap()
            .is_none());

        meeting.status = MeetingStatus::Ready;
        meeting.ended_at = Some(ts("2026-09-17T09:00:00Z"));
        meeting.duration_secs = Some(1800);
        meeting.updated_at = Utc::now();
        MeetingRepository::update(&store, &meeting).await.unwrap();

        let reloaded = MeetingRepository::get(&store, meeting.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.status, MeetingStatus::Ready);
        assert_eq!(reloaded.ended_at, meeting.ended_at);
        assert_eq!(reloaded.duration_secs, Some(1800));
    }

    #[tokio::test]
    async fn list_orders_newest_first() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let earliest = meeting_at("earliest", ts("2026-09-01T00:00:00Z"));
        let latest = meeting_at("latest", ts("2026-09-17T00:00:00Z"));
        let middle = meeting_at("middle", ts("2026-09-10T00:00:00Z"));

        MeetingRepository::create(&store, &earliest).await.unwrap();
        MeetingRepository::create(&store, &latest).await.unwrap();
        MeetingRepository::create(&store, &middle).await.unwrap();

        let titles: Vec<String> = MeetingRepository::list(&store)
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.title)
            .collect();
        assert_eq!(titles, vec!["latest", "middle", "earliest"]);
    }

    #[tokio::test]
    async fn update_missing_meeting_returns_not_found() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let meeting = Meeting::new("ghost");
        let err = MeetingRepository::update(&store, &meeting)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn update_without_changes_still_returns_ok() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let meeting = Meeting::new("stable");
        MeetingRepository::create(&store, &meeting).await.unwrap();
        MeetingRepository::update(&store, &meeting).await.unwrap();
        assert!(MeetingRepository::get(&store, meeting.id)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn delete_missing_meeting_returns_not_found() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let err = MeetingRepository::delete(&store, Uuid::new_v4(), true)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_with_raw_removes_artifacts_and_cascades() {
        let dir = temp_dir("delete-raw");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let store = SqliteStore::open(&dir.join("db.sqlite3"), Some(dir.clone()))
            .await
            .unwrap();
        let meeting = meeting_at("flagged", ts("2026-09-17T09:00:00Z"));
        MeetingRepository::create(&store, &meeting).await.unwrap();
        NoteRepository::add(&store, meeting.id, NoteKind::Decision, "delete me")
            .await
            .unwrap();

        let artifact_dir = store.artifact_dir(meeting.id).unwrap();
        tokio::fs::create_dir_all(&artifact_dir).await.unwrap();
        tokio::fs::write(artifact_dir.join(RECORDING_FILENAME), b"recorded")
            .await
            .unwrap();
        tokio::fs::write(artifact_dir.join(TRANSCRIPT_FILENAME), b"{}")
            .await
            .unwrap();

        MeetingRepository::delete(&store, meeting.id, true)
            .await
            .unwrap();

        assert!(MeetingRepository::get(&store, meeting.id)
            .await
            .unwrap()
            .is_none());
        assert!(NoteRepository::list(&store, meeting.id)
            .await
            .unwrap()
            .is_empty());
        assert!(!artifact_dir.join(RECORDING_FILENAME).exists());
        assert!(!artifact_dir.join(TRANSCRIPT_FILENAME).exists());
        assert!(!artifact_dir.exists());
    }

    #[tokio::test]
    async fn delete_without_raw_keeps_artifacts() {
        let dir = temp_dir("delete-keep");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let store = SqliteStore::open(&dir.join("db.sqlite3"), Some(dir.clone()))
            .await
            .unwrap();
        let meeting = meeting_at("keep raw", ts("2026-09-17T10:00:00Z"));
        MeetingRepository::create(&store, &meeting).await.unwrap();

        let artifact_dir = store.artifact_dir(meeting.id).unwrap();
        tokio::fs::create_dir_all(&artifact_dir).await.unwrap();
        let recording = artifact_dir.join(RECORDING_FILENAME);
        tokio::fs::write(&recording, b"recorded").await.unwrap();

        MeetingRepository::delete(&store, meeting.id, false)
            .await
            .unwrap();

        assert!(MeetingRepository::get(&store, meeting.id)
            .await
            .unwrap()
            .is_none());
        assert!(recording.exists());
        assert!(artifact_dir.exists());
    }

    #[tokio::test]
    async fn notes_sequence_and_clear() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let meeting = meeting_at("notes", ts("2026-09-17T11:00:00Z"));
        MeetingRepository::create(&store, &meeting).await.unwrap();

        let n1 = NoteRepository::add(&store, meeting.id, NoteKind::Summary, "overview")
            .await
            .unwrap();
        let n2 = NoteRepository::add(&store, meeting.id, NoteKind::ActionItem, "ship it")
            .await
            .unwrap();
        let n3 = NoteRepository::add(&store, meeting.id, NoteKind::Question, "when?")
            .await
            .unwrap();
        assert_eq!(n1.seq, 1);
        assert_eq!(n2.seq, 2);
        assert_eq!(n3.seq, 3);

        let notes = NoteRepository::list(&store, meeting.id).await.unwrap();
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0].content, "overview");
        assert_eq!(notes[1].kind, NoteKind::ActionItem);

        let err = NoteRepository::add(&store, Uuid::new_v4(), NoteKind::Topic, "x")
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));

        NoteRepository::delete_for_meeting(&store, meeting.id)
            .await
            .unwrap();
        assert!(NoteRepository::list(&store, meeting.id)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn transcript_segments_append_idempotent_and_clear() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let meeting = meeting_at("transcript", ts("2026-09-17T12:00:00Z"));
        MeetingRepository::create(&store, &meeting).await.unwrap();

        let seg1 = segment(1, "hello world");
        let seg2 = segment(2, "goodbye");
        TranscriptRepository::append_segments(
            &store,
            meeting.id,
            &[seg1.clone(), seg2.clone(), seg1.clone()],
        )
        .await
        .unwrap();

        let listed = TranscriptRepository::list(&store, meeting.id)
            .await
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].seq, 1);
        assert_eq!(listed[0].text, "hello world");
        assert_eq!(listed[1].text, "goodbye");

        let err = TranscriptRepository::append_segment(&store, Uuid::new_v4(), &seg1)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));

        TranscriptRepository::delete_for_meeting(&store, meeting.id)
            .await
            .unwrap();
        assert!(TranscriptRepository::list(&store, meeting.id)
            .await
            .unwrap()
            .is_empty());

        TranscriptRepository::append_segment(&store, meeting.id, &seg1)
            .await
            .unwrap();
        assert_eq!(
            TranscriptRepository::list(&store, meeting.id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn in_memory_store_has_no_meetings_dir() {
        let store = SqliteStore::open_in_memory().await.unwrap();
        let meeting = Meeting::new("temp");
        MeetingRepository::create(&store, &meeting).await.unwrap();
        assert_eq!(
            MeetingRepository::get(&store, meeting.id)
                .await
                .unwrap()
                .unwrap()
                .title,
            "temp"
        );
        assert!(store.meetings_dir().is_none());
        assert!(store.artifact_dir(meeting.id).is_none());
    }

    #[tokio::test]
    async fn search_finds_transcript_and_title_hits() {
        let store = SqliteStore::open_in_memory().await.unwrap();

        let title_meeting = meeting_at("quarterly deadlines review", ts("2026-09-16T08:00:00Z"));
        MeetingRepository::create(&store, &title_meeting)
            .await
            .unwrap();

        let transcript_meeting = Meeting::new("team sync");
        MeetingRepository::create(&store, &transcript_meeting)
            .await
            .unwrap();
        TranscriptRepository::append_segments(
            &store,
            transcript_meeting.id,
            &[
                segment(1, "lets review the deadlines"),
                segment(2, "the deadlines moved"),
            ],
        )
        .await
        .unwrap();

        let hits = MeetingRepository::search(&store, "deadlines")
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        let titles: Vec<&str> = hits.iter().map(|h| h.meeting_title.as_str()).collect();
        assert!(titles.contains(&"quarterly deadlines review"));
        assert!(titles.contains(&"team sync"));
        for hit in &hits {
            assert!(hit.snippet.contains("deadlines"));
        }

        let none = MeetingRepository::search(&store, "  ").await.unwrap();
        assert!(none.is_empty());
    }
}
