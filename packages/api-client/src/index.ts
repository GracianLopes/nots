import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type {
  AppSettings,
  AiChatMessage,
  AiChatRequest,
  AiChatResponse,
  AIMode,
  AudioSource,
  CommandError,
  CommandErrorKind,
  CoreEvent,
  Meeting,
  MeetingStatus,
  Note,
  NoteKind,
  PingStatus,
  PrivacyLevel,
  ProcessingState,
  ProviderConfig,
  ProviderKind,
  RecordingState,
  RetentionMode,
  SearchHit,
  SttLanguage,
  TranscriptSegment,
  TranscriptionResult,
  WhisperModel,
} from "@notsai/shared-types";

import type {
  AppSettings,
  Meeting,
  SearchHit,
  Note,
  PingStatus,
  CoreEvent,
} from "@notsai/shared-types";

// ---- commands ----

export function getSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_settings");
}

export function setSettings(settings: AppSettings): Promise<AppSettings> {
  return invoke<AppSettings>("set_settings", { settings });
}

export function createMeeting(title: string): Promise<Meeting> {
  return invoke<Meeting>("create_meeting", { title });
}

export function getMeeting(id: string): Promise<Meeting> {
  return invoke<Meeting>("get_meeting", { id });
}

export function listMeetings(): Promise<Meeting[]> {
  return invoke<Meeting[]>("list_meetings");
}

export function deleteMeeting(id: string, deleteRaw: boolean): Promise<void> {
  return invoke<void>("delete_meeting", { id, deleteRaw });
}

export function searchMeetings(query: string): Promise<SearchHit[]> {
  return invoke<SearchHit[]>("search_meetings", { query });
}

export function recordMeeting(id: string): Promise<Meeting> {
  return invoke<Meeting>("record_meeting", { id });
}

export function stopRecording(): Promise<Meeting> {
  return invoke<Meeting>("stop_recording");
}

export function listNotes(meetingId: string): Promise<Note[]> {
  return invoke<Note[]>("list_notes", { meetingId });
}

export function transcribeMeeting(id: string): Promise<Meeting> {
  return invoke<Meeting>("transcribe_meeting", { id });
}

export function exportNotes(meetingId: string): Promise<string> {
  return invoke<string>("export_notes", { meetingId });
}

export function generateNotes(id: string): Promise<Meeting> {
  return invoke<Meeting>("generate_notes", { id });
}

export function ping(): Promise<PingStatus> {
  return invoke<PingStatus>("ping");
}

export function readRecordingBytes(path: string): Promise<number[]> {
  return invoke<number[]>("read_recording_bytes", { path });
}

// ---- events ----

export type RecordingEvent = Extract<CoreEvent, { event: "recording" }>;
export type TranscriptChunkEvent = Extract<CoreEvent, { event: "transcript_chunk" }>;
export type TranscriptionProgressEvent = Extract<
  CoreEvent,
  { event: "transcription_progress" }
>;
export type ProcessingEvent = Extract<CoreEvent, { event: "processing" }>;

export function onRecordingState(
  handler: (event: RecordingEvent) => void,
): Promise<UnlistenFn> {
  return listen<RecordingEvent>("recording_state", ({ payload }) => handler(payload));
}

export function onTranscriptChunk(
  handler: (event: TranscriptChunkEvent) => void,
): Promise<UnlistenFn> {
  return listen<TranscriptChunkEvent>("transcript_chunk", ({ payload }) =>
    handler(payload),
  );
}

export function onTranscriptionProgress(
  handler: (event: TranscriptionProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<TranscriptionProgressEvent>(
    "transcription_progress",
    ({ payload }) => handler(payload),
  );
}

export function onProcessingState(
  handler: (event: ProcessingEvent) => void,
): Promise<UnlistenFn> {
  return listen<ProcessingEvent>("processing_state", ({ payload }) => handler(payload));
}