//! notsAI Tauri 2 backend: application state, the IPC command surface and the
//! event plumbing that carries domain events from core to the desktop UI.
//!
//! The `notsai-transcribe` pipeline is wired up (record → transcribe → ready)
//! and the `notsai-ai` generation layer (transcript → structured notes) is
//! connected through the `generate_notes` command. This crate establishes
//! persistent storage, settings and meeting lifecycle commands plus the event
//! forwarder the frontend listens to.

#![forbid(unsafe_code)]

use std::{fs, path::PathBuf, sync::Mutex};

use chrono::Utc;
use tauri::{Emitter, Manager, State};
use tokio::sync::broadcast;
use uuid::Uuid;

use notsai_ai::{provider_from_config, Summarizer};
use notsai_audio::{ffmpeg, wav, AudioError, MicRecorder};
use notsai_core::{
    AppSettings, AudioSource, CoreError, CoreEvent, EventBus, Meeting, MeetingRepository,
    MeetingStatus, Note, NoteRepository, ProcessingState, RecordingState, SearchHit,
    SettingsRepository, TranscriptRepository, TranscriptSegment, TranscriptionEngine,
};
use notsai_storage::{
    FileSettingsRepository, SqliteStore, RECORDING_FILENAME, TRANSCRIPT_FILENAME,
};
use notsai_transcribe::{Consent, ModelManager, WhisperEngine, MODELS_DIR};

/// Filename of the SQLite database inside the app data directory.
const DB_FILENAME: &str = "notsai.db";

/// Capacity of the domain event bus; sets the forwarder's lag tolerance.
const EVENT_CAPACITY: usize = 256;

/// Application state shared with Tauri commands.
pub struct AppState {
    store: SqliteStore,
    settings_repo: FileSettingsRepository,
    event_bus: EventBus,
    /// The single active recording (meeting id plus recorder), if any.
    recorder: Mutex<Option<(Uuid, MicRecorder)>>,
}

/// Payload returned by the `ping` command, used by the frontend to confirm the
/// backend and event pipeline are alive.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingStatus {
    ok: bool,
    version: String,
    receiver_count: usize,
}

/// Errors surfaced to the IPC layer. Serialized with an externally stable
/// shape so the frontend API client can match on `kind` (rust module name) to
/// decide how to handle a failure.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum CommandError {
    InvalidInput(String),
    NotFound(String),
    Storage(String),
    Internal(String),
}

impl From<CoreError> for CommandError {
    fn from(err: CoreError) -> Self {
        match err {
            CoreError::InvalidInput(msg) => CommandError::InvalidInput(msg),
            CoreError::NotFound(msg) => CommandError::NotFound(msg),
            CoreError::Storage(msg) => CommandError::Storage(msg),
            CoreError::Io(io) => CommandError::Storage(io.to_string()),
            other => CommandError::Internal(other.to_string()),
        }
    }
}

impl From<AudioError> for CommandError {
    fn from(err: AudioError) -> Self {
        match err {
            AudioError::NoInputDevice => {
                CommandError::InvalidInput("no default input device found".into())
            }
            other => CommandError::Internal(other.to_string()),
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            CommandError::NotFound(msg) => write!(f, "not found: {msg}"),
            CommandError::Storage(msg) => write!(f, "storage error: {msg}"),
            CommandError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for CommandError {}

#[tauri::command]
async fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, CommandError> {
    Ok(state.settings_repo.load().await?)
}

#[tauri::command]
async fn set_settings(
    settings: AppSettings,
    state: State<'_, AppState>,
) -> Result<AppSettings, CommandError> {
    state.settings_repo.save(&settings).await?;
    Ok(state.settings_repo.load().await?)
}

#[tauri::command]
async fn create_meeting(
    title: String,
    state: State<'_, AppState>,
) -> Result<Meeting, CommandError> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(CoreError::invalid_input("title must not be empty").into());
    }
    let meeting = Meeting::new(title);
    state.store.create(&meeting).await?;
    Ok(meeting)
}

#[tauri::command]
async fn get_meeting(id: Uuid, state: State<'_, AppState>) -> Result<Meeting, CommandError> {
    state
        .store
        .get(id)
        .await?
        .ok_or(CoreError::not_found(format!("meeting {id}")))
        .map_err(CommandError::from)
}

#[tauri::command]
async fn list_meetings(state: State<'_, AppState>) -> Result<Vec<Meeting>, CommandError> {
    Ok(MeetingRepository::list(&state.store).await?)
}

#[tauri::command]
async fn delete_meeting(
    id: Uuid,
    delete_raw: bool,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    state.store.delete(id, delete_raw).await?;
    Ok(())
}

#[tauri::command]
async fn search_meetings(
    query: String,
    state: State<'_, AppState>,
) -> Result<Vec<SearchHit>, CommandError> {
    Ok(state.store.search(&query).await?)
}

#[tauri::command]
async fn list_notes(
    meeting_id: Uuid,
    state: State<'_, AppState>,
) -> Result<Vec<Note>, CommandError> {
    Ok(NoteRepository::list(&state.store, meeting_id).await?)
}

/// Export a meeting's structured notes to a Markdown file on disk and return
/// the written path. The file lands in the platform Downloads directory,
/// falling back to an `exports` folder inside the app data directory, and is
/// then revealed in the system file manager (best-effort).
#[tauri::command]
async fn export_notes(
    meeting_id: Uuid,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, CommandError> {
    let meeting = state
        .store
        .get(meeting_id)
        .await?
        .ok_or(CoreError::not_found(format!("meeting {meeting_id}")))?;

    let notes = NoteRepository::list(&state.store, meeting_id).await?;
    if notes.is_empty() {
        return Err(CoreError::invalid_input("meeting has no notes to export").into());
    }

    let export_dir = match app.path().download_dir() {
        Ok(dir) => dir,
        Err(_) => app
            .path()
            .app_data_dir()
            .map(|dir| dir.join("exports"))
            .map_err(|_| CoreError::internal("failed to resolve export directory"))?,
    };
    fs::create_dir_all(&export_dir).map_err(|err| {
        CommandError::Storage(format!(
            "failed to create export directory {export_dir:?}: {err}"
        ))
    })?;

    let path = export_dir.join(format!(
        "notes-{meeting_id}-{}.md",
        Utc::now().format("%Y%m%d-%H%M%S")
    ));

    let mut markdown = format!(
        "# {}\n\nExported: {}\n\n",
        meeting.title,
        Utc::now().to_rfc3339()
    );
    for note in &notes {
        markdown.push_str(&format!("## {:?}\n\n{}\n\n", note.kind, note.content));
    }

    fs::write(&path, &markdown)
        .map_err(|err| CommandError::Storage(format!("failed to write notes file: {err}")))?;

    use tauri_plugin_opener::OpenerExt;
    if let Err(err) = app.opener().reveal_item_in_dir(&path) {
        tracing::warn!(
            path = %path.display(),
            error = %err,
            "failed to reveal exported notes in file manager"
        );
    }

    Ok(path.to_string_lossy().into_owned())
}

/// Start recording audio for a meeting through the default microphone. Emits
/// `recording_state` events as the recording begins and later stops.
#[tauri::command]
async fn record_meeting(id: Uuid, state: State<'_, AppState>) -> Result<Meeting, CommandError> {
    let mut meeting = state
        .store
        .get(id)
        .await?
        .ok_or(CoreError::not_found(format!("meeting {id}")))?;

    if state.recorder.lock().unwrap().is_some() {
        return Err(CoreError::invalid_input("a recording is already active").into());
    }

    meeting.status = MeetingStatus::Recording;
    meeting.last_error = None;
    meeting.updated_at = Utc::now();
    state.store.update(&meeting).await?;
    state.event_bus.publish(CoreEvent::Recording {
        meeting_id: id,
        state: RecordingState::Starting,
    });

    let recorder = match MicRecorder::start() {
        Ok(recorder) => recorder,
        Err(err) => {
            mark_recording_failed(&state, &mut meeting, &err.to_string()).await;
            return Err(CommandError::from(err));
        }
    };
    state.recorder.lock().unwrap().replace((id, recorder));
    state.event_bus.publish(CoreEvent::Recording {
        meeting_id: id,
        state: RecordingState::Recording {
            started_at: Utc::now(),
        },
    });

    Ok(meeting)
}

/// Stop the active recording (if any), write the raw WAV capture into the
/// meeting's artifact directory, transcode it to M4A and flip the meeting to
/// `Recorded`. Emits a `recording_state` event carrying the final duration.
#[tauri::command]
async fn stop_recording(state: State<'_, AppState>) -> Result<Meeting, CommandError> {
    let (id, recorder) = state
        .recorder
        .lock()
        .unwrap()
        .take()
        .ok_or(CoreError::invalid_input("no active recording"))?;

    let mut meeting = state
        .store
        .get(id)
        .await?
        .ok_or(CoreError::not_found(format!("meeting {id}")))?;

    let pcm = match recorder.stop_async().await {
        Ok(pcm) => pcm,
        Err(err) => {
            mark_recording_failed(&state, &mut meeting, &err.to_string()).await;
            return Err(CommandError::from(err));
        }
    };

    let artifact_dir = state
        .store
        .artifact_dir(id)
        .expect("store opened without a meetings dir");

    if let Err(err) = fs::create_dir_all(&artifact_dir) {
        let failure = err.to_string();
        mark_recording_failed(&state, &mut meeting, &failure).await;
        return Err(CommandError::from(CoreError::from(err)));
    }

    let temp_wav = artifact_dir.join("recording.wav");
    if let Err(err) = wav::write_s16le(&temp_wav, pcm.sample_rate, pcm.channels, &pcm.samples) {
        let _ = fs::remove_file(&temp_wav);
        let failure = err.to_string();
        mark_recording_failed(&state, &mut meeting, &failure).await;
        return Err(CommandError::from(AudioError::from(err)));
    }

    let recording_path = artifact_dir.join(RECORDING_FILENAME);
    if let Err(err) = ffmpeg::transcode_to_m4a_denoised(&temp_wav, &recording_path).await {
        let _ = fs::remove_file(&temp_wav);
        let failure = err.to_string();
        mark_recording_failed(&state, &mut meeting, &failure).await;
        return Err(CommandError::from(err));
    }
    let _ = fs::remove_file(&temp_wav);

    let duration_secs = pcm.duration_secs().round() as u64;
    meeting.status = MeetingStatus::Recorded;
    meeting.recording_path = Some(recording_path);
    meeting.ended_at = Some(Utc::now());
    meeting.duration_secs = Some(duration_secs);
    meeting.updated_at = Utc::now();
    state.store.update(&meeting).await?;
    state.event_bus.publish(CoreEvent::Recording {
        meeting_id: id,
        state: RecordingState::Stopped { duration_secs },
    });

    Ok(meeting)
}

#[tauri::command]
async fn ping(state: State<'_, AppState>) -> Result<PingStatus, CommandError> {
    Ok(PingStatus {
        ok: true,
        version: env!("CARGO_PKG_VERSION").to_string(),
        receiver_count: state.event_bus.receiver_count(),
    })
}

/// Transcribe a recorded meeting: run the whisper model over the recording,
/// persist the resulting segments, write the transcript artifact to disk and
/// flip the meeting into `Ready`. Any step that fails leaves the meeting in
/// `Failed` with a stored `last_error` before returning the error.
#[tauri::command]
async fn transcribe_meeting(id: Uuid, state: State<'_, AppState>) -> Result<Meeting, CommandError> {
    let mut meeting = state
        .store
        .get(id)
        .await?
        .ok_or(CoreError::not_found(format!("meeting {id}")))?;

    let recording_path = meeting.recording_path.clone().ok_or_else(|| {
        CoreError::invalid_input(format!("meeting {id} has no recording to transcribe"))
    })?;

    let settings = state.settings_repo.load().await?;

    let models_dir = state
        .store
        .meetings_dir()
        .expect("store opened without a meetings dir")
        .join(MODELS_DIR);
    let engine = WhisperEngine::new(
        settings.whisper_model,
        settings.stt_language,
        ModelManager::new(models_dir),
        Consent::Granted,
    );

    meeting.status = MeetingStatus::Transcribing;
    meeting.last_error = None;
    meeting.updated_at = Utc::now();
    state.store.update(&meeting).await?;

    let transcript = match engine
        .transcribe(
            id,
            AudioSource::File {
                path: recording_path,
            },
            &state.event_bus,
        )
        .await
    {
        Ok(transcript) => transcript,
        Err(err) => {
            mark_failed(&state, &mut meeting, &err).await;
            return Err(CommandError::from(err));
        }
    };

    if let Err(err) = state.store.append_segments(id, &transcript.segments).await {
        mark_failed(&state, &mut meeting, &err).await;
        return Err(CommandError::from(err));
    }

    let artifact_dir = state
        .store
        .artifact_dir(id)
        .expect("store opened without a meetings dir");
    let transcript_path = match write_transcript_artifact(artifact_dir, &transcript.segments) {
        Ok(path) => path,
        Err(err) => {
            mark_failed(&state, &mut meeting, &err).await;
            return Err(CommandError::from(err));
        }
    };

    meeting.status = MeetingStatus::Ready;
    meeting.transcript_path = Some(transcript_path);
    meeting.updated_at = Utc::now();
    state.store.update(&meeting).await?;

    Ok(meeting)
}

/// Write the transcript artifact (`transcript.json`) into the meeting's
/// artifact directory, creating the directory first.
fn write_transcript_artifact(
    dir: PathBuf,
    segments: &[TranscriptSegment],
) -> Result<PathBuf, CoreError> {
    fs::create_dir_all(&dir).map_err(CoreError::from)?;
    let bytes = serde_json::to_vec_pretty(segments).map_err(CoreError::from)?;
    let path = dir.join(TRANSCRIPT_FILENAME);
    fs::write(&path, bytes).map_err(CoreError::from)?;
    Ok(path)
}

/// Generate structured notes for a recorded meeting: run the configured AI
/// provider over the transcript segments, persist the resulting notes, flip
/// the meeting into `Ready` and emit progress events. Any step that fails
/// leaves the meeting in `Failed` with a stored `last_error` before returning
/// the error.
#[tauri::command]
async fn generate_notes(id: Uuid, state: State<'_, AppState>) -> Result<Meeting, CommandError> {
    let mut meeting = state
        .store
        .get(id)
        .await?
        .ok_or(CoreError::not_found(format!("meeting {id}")))?;

    let settings = state.settings_repo.load().await?;
    if !settings.ai_consent {
        return Err(CoreError::invalid_input(
            "AI notes require user consent (ai_consent) before processing",
        )
        .into());
    }

    let active_kind = settings
        .active_provider
        .or_else(|| settings.providers.first().map(|provider| provider.kind))
        .ok_or_else(|| CoreError::invalid_input("no AI provider configured for notes"))?;

    let provider_config = settings.provider(active_kind).ok_or_else(|| {
        CoreError::invalid_input(format!("provider {active_kind:?} is not configured"))
    })?;

    let segments = TranscriptRepository::list(&state.store, id).await?;
    if segments.is_empty() {
        return Err(CoreError::invalid_input(format!(
            "meeting {id} has no transcript to summarize"
        ))
        .into());
    }

    // Replace any notes from a previous run before regenerating.
    if let Err(err) = notsai_core::NoteRepository::delete_for_meeting(&state.store, id).await {
        mark_failed(&state, &mut meeting, &err).await;
        return Err(CommandError::from(err));
    }

    meeting.status = MeetingStatus::Processing;
    meeting.last_error = None;
    meeting.updated_at = Utc::now();
    state.store.update(&meeting).await?;
    state.event_bus.publish(CoreEvent::Processing {
        meeting_id: id,
        state: ProcessingState::Summarizing,
    });

    let summarizer = Summarizer::new(
        provider_from_config(provider_config)?,
        provider_config.model.clone(),
    );

    let notes = match summarizer.summarize(&segments).await {
        Ok(notes) => notes,
        Err(err) => {
            mark_failed(&state, &mut meeting, &err).await;
            state.event_bus.publish(CoreEvent::Processing {
                meeting_id: id,
                state: ProcessingState::Failed {
                    error: err.to_string(),
                },
            });
            return Err(CommandError::from(err));
        }
    };

    for (kind, content) in notes {
        if let Err(err) = state.store.add(id, kind, &content).await {
            mark_failed(&state, &mut meeting, &err).await;
            state.event_bus.publish(CoreEvent::Processing {
                meeting_id: id,
                state: ProcessingState::Failed {
                    error: err.to_string(),
                },
            });
            return Err(CommandError::from(err));
        }
    }

    meeting.status = MeetingStatus::Ready;
    meeting.updated_at = Utc::now();
    state.store.update(&meeting).await?;
    state.event_bus.publish(CoreEvent::Processing {
        meeting_id: id,
        state: ProcessingState::Complete,
    });

    Ok(meeting)
}

/// Read the raw bytes of a meeting's recorded audio into memory so the
/// frontend can play it from a Blob URL. The asset protocol backing
/// `convertFileSrc` is unreliable for media playback under WebKitGTK, so
/// recordings are shipped over IPC instead. Files are small (AAC/m4a, ~1MB),
/// which keeps the `Vec<u8>` -> JSON `number[]` serialization acceptable.
#[tauri::command]
async fn read_recording_bytes(path: String) -> Result<Vec<u8>, CommandError> {
    fs::read(&path)
        .map_err(|err| CommandError::Storage(format!("failed to read recording at {path}: {err}")))
}

/// Transition a meeting into `Failed`, record the failure reason and persist
/// the change. Persistence is best-effort: if the update itself fails we can
/// do nothing more than log it.
async fn mark_failed(state: &AppState, meeting: &mut Meeting, failure: &CoreError) {
    meeting.status = MeetingStatus::Failed;
    meeting.last_error = Some(failure.to_string());
    meeting.updated_at = Utc::now();
    if let Err(err) = state.store.update(meeting).await {
        tracing::error!(
            meeting_id = %meeting.id,
            error = %err,
            "failed to persist failure state for meeting"
        );
    }
}

/// Like `mark_failed`, but for recording failures: it also emits a
/// `recording_state` failure event so the UI learns why recording stopped.
async fn mark_recording_failed(state: &AppState, meeting: &mut Meeting, failure: &str) {
    meeting.status = MeetingStatus::Failed;
    meeting.last_error = Some(failure.to_string());
    meeting.updated_at = Utc::now();
    if let Err(err) = state.store.update(meeting).await {
        tracing::error!(
            meeting_id = %meeting.id,
            error = %err,
            "failed to persist recording failure state for meeting"
        );
    }
    state.event_bus.publish(CoreEvent::Recording {
        meeting_id: meeting.id,
        state: RecordingState::Failed {
            error: failure.to_string(),
        },
    });
}

/// Map a domain event to the JS event name the frontend subscribes to, then
/// emit the full event (its `event` tag is included in the serde payload).
fn forward_event(app: &tauri::AppHandle, event: CoreEvent) {
    let name = match event {
        CoreEvent::Recording { .. } => "recording_state",
        CoreEvent::TranscriptChunk { .. } => "transcript_chunk",
        CoreEvent::TranscriptionProgress { .. } => "transcription_progress",
        CoreEvent::Processing { .. } => "processing_state",
    };
    if let Err(err) = app.emit(name, &event) {
        tracing::error!(
            meeting_id = %event.meeting_id(),
            event = name,
            error = %err,
            "failed to forward core event to UI"
        );
    }
}

/// Run a dedicated forwarding loop on its own thread.
///
/// The broadcast receiver and the `AppHandle` are both `Send`, so they move
/// into a plain thread. A `current_thread` Tokio runtime can only be driven
/// from the thread that created it, so the loop builds its own runtime inside
/// the thread and `block_on`s it - it must never be moved between threads.
fn spawn_event_forwarder(app: tauri::AppHandle, mut rx: broadcast::Receiver<CoreEvent>) {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::error!(error = %err, "failed to build event forwarder runtime");
                return;
            }
        };
        runtime.block_on(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => forward_event(&app, event),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    });
}

/// Application entry point called from `main`.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&app_data_dir)?;

            let event_bus = EventBus::new(EVENT_CAPACITY);

            // Open the SQLite store on a temporary current-thread runtime (the
            // database pool outlives it). The runtime must be dropped here so a
            // `current_thread` runtime is never moved across threads.
            let store = {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let db_path = app_data_dir.join(DB_FILENAME);
                runtime.block_on(SqliteStore::open(&db_path, Some(app_data_dir.clone())))?
            };

            let settings_repo = FileSettingsRepository::in_dir(&app_data_dir);

            let forwarder_app = app.handle().clone();
            let forwarder_rx = event_bus.subscribe();
            spawn_event_forwarder(forwarder_app, forwarder_rx);

            app.manage(AppState {
                store,
                settings_repo,
                event_bus,
                recorder: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            create_meeting,
            get_meeting,
            list_meetings,
            delete_meeting,
            search_meetings,
            list_notes,
            export_notes,
            record_meeting,
            stop_recording,
            ping,
            transcribe_meeting,
            generate_notes,
            read_recording_bytes
        ])
        .run(tauri::generate_context!())
        .expect("error while running notsAI");
}
