CREATE TABLE meetings (
    id               TEXT PRIMARY KEY NOT NULL,
    title            TEXT NOT NULL,
    status           TEXT NOT NULL,
    started_at       TEXT NOT NULL,
    ended_at         TEXT,
    duration_secs    INTEGER,
    recording_path   TEXT,
    transcript_path  TEXT,
    last_error       TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);

CREATE TABLE notes (
    id          TEXT PRIMARY KEY NOT NULL,
    meeting_id  TEXT NOT NULL,
    kind        TEXT NOT NULL,
    content     TEXT NOT NULL,
    seq         INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings (id) ON DELETE CASCADE
);

CREATE TABLE transcript_segments (
    meeting_id  TEXT NOT NULL,
    seq         INTEGER NOT NULL,
    start       REAL NOT NULL,
    end         REAL NOT NULL,
    speaker     TEXT,
    text        TEXT NOT NULL,
    confidence  REAL,
    PRIMARY KEY (meeting_id, seq),
    FOREIGN KEY (meeting_id) REFERENCES meetings (id) ON DELETE CASCADE
);

CREATE INDEX notes_meeting_seq_idx ON notes (meeting_id, seq);
CREATE INDEX transcript_segments_meeting_idx ON transcript_segments (meeting_id);

CREATE VIRTUAL TABLE meetings_fts USING fts5 (
    title,
    content = 'meetings',
    content_rowid = 'rowid'
);

CREATE VIRTUAL TABLE transcripts_fts USING fts5 (
    meeting_id UNINDEXED,
    text,
    content = 'transcript_segments',
    content_rowid = 'rowid'
);

CREATE TRIGGER meetings_fts_after_insert
AFTER INSERT ON meetings BEGIN
    INSERT INTO meetings_fts (rowid, title) VALUES (new.rowid, new.title);
END;

CREATE TRIGGER meetings_fts_after_delete
AFTER DELETE ON meetings BEGIN
    INSERT INTO meetings_fts (meetings_fts, rowid, title)
    VALUES ('delete', old.rowid, old.title);
END;

CREATE TRIGGER meetings_fts_after_update
AFTER UPDATE ON meetings BEGIN
    INSERT INTO meetings_fts (meetings_fts, rowid, title)
    VALUES ('delete', old.rowid, old.title);
    INSERT INTO meetings_fts (rowid, title) VALUES (new.rowid, new.title);
END;

CREATE TRIGGER transcripts_fts_after_insert
AFTER INSERT ON transcript_segments BEGIN
    INSERT INTO transcripts_fts (rowid, meeting_id, text)
    VALUES (new.rowid, new.meeting_id, new.text);
END;

CREATE TRIGGER transcripts_fts_after_delete
AFTER DELETE ON transcript_segments BEGIN
    INSERT INTO transcripts_fts (transcripts_fts, rowid, meeting_id, text)
    VALUES ('delete', old.rowid, old.meeting_id, old.text);
END;

CREATE TRIGGER transcripts_fts_after_update
AFTER UPDATE ON transcript_segments BEGIN
    INSERT INTO transcripts_fts (transcripts_fts, rowid, meeting_id, text)
    VALUES ('delete', old.rowid, old.meeting_id, old.text);
    INSERT INTO transcripts_fts (rowid, meeting_id, text)
    VALUES (new.rowid, new.meeting_id, new.text);
END;