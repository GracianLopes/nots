// Shared domain types mirroring `crates/notsai-core` serde contracts.
//
// Mapping conventions:
//   Uuid / DateTime<Utc> -> string (UUID, ISO-8601)
//   PathBuf             -> string
//   i64/u64/f64/f32     -> number

// ---- meetings ----

export type MeetingStatus =
  | "created"
  | "recording"
  | "recorded"
  | "transcribing"
  | "processing"
  | "ready"
  | "failed";

export interface Meeting {
  id: string;
  title: string;
  status: MeetingStatus;
  started_at: string;
  ended_at?: string;
  duration_secs?: number;
  recording_path?: string;
  transcript_path?: string;
  last_error?: string;
  created_at: string;
  updated_at: string;
}

// ---- notes ----

export type NoteKind =
  | "summary"
  | "topic"
  | "decision"
  | "action_item"
  | "question";

export interface Note {
  id: string;
  meeting_id: string;
  kind: NoteKind;
  content: string;
  seq: number;
  created_at: string;
}

// ---- transcription ----

export interface TranscriptSegment {
  seq: number;
  start: number;
  end: number;
  speaker?: string;
  text: string;
  confidence?: number;
}

export interface TranscriptionResult {
  segments: TranscriptSegment[];
  language?: string;
}

// ---- search ----

export interface SearchHit {
  meeting_id: string;
  meeting_title: string;
  snippet: string;
}

// ---- audio source ----

export type AudioSource = { source: "file"; path: string };

// ---- AI ----

export type AiRole = "system" | "user" | "assistant";

export interface AiChatMessage {
  role: AiRole;
  content: string;
}

export interface AiChatRequest {
  model: string;
  messages: AiChatMessage[];
  max_tokens?: number;
}

export interface AiChatResponse {
  content: string;
}

// ---- settings ----

export type RetentionMode = "full_recording" | "smart_notes" | "notes_only";

export type AIMode = "cloud" | "local" | "hybrid";

export type PrivacyLevel = "maximum_privacy" | "balanced" | "maximum_speed";

// Local means a local OpenAI-compatible endpoint (e.g. Ollama or LM Studio).
export type ProviderKind = "gemini" | "groq" | "open_router" | "local";

export type SttLanguage = "auto" | "en" | "hi" | "mr";

export type WhisperModel =
  | "tiny"
  | "base"
  | "small"
  | "medium"
  | "large_v3";

export interface ProviderConfig {
  kind: ProviderKind;
  model: string;
  api_key?: string;
  /** Optional custom base URL for OpenAI-compatible local backends (e.g. Ollama or LM Studio). */
  base_url?: string;
}

export interface AppSettings {
  retention: RetentionMode;
  ai_mode: AIMode;
  stt_language: SttLanguage;
  whisper_model: WhisperModel;
  default_title: string;
  providers: ProviderConfig[];
  /** Provider used for AI notes; `null` falls back to the first configured provider. */
  active_provider: ProviderKind | null;
  /** User consent for AI notes processing; default off (record-only). */
  ai_consent: boolean;
}

// ---- event payloads ----

export type RecordingState =
  | { state: "starting" }
  | { state: "recording"; started_at: string }
  /**
   * The microphone is recording, but system audio could not be captured, so
   * this recording is microphone-only. Not a failure: `reason` is shown as a
   * non-blocking warning.
   */
  | { state: "system_audio_degraded"; reason: string }
  | { state: "stopped"; duration_secs: number }
  | { state: "failed"; error: string };

export type ProcessingState =
  | { state: "downloading"; downloaded_bytes: number; total_bytes: number }
  | { state: "transcribing" }
  | { state: "summarizing" }
  | { state: "complete" }
  | { state: "failed"; error: string };

export type CoreEvent =
  | {
      event: "recording";
      meeting_id: string;
      state: RecordingState;
    }
  | {
      event: "transcript_chunk";
      meeting_id: string;
      segment: TranscriptSegment;
      language?: string;
    }
  | {
      event: "transcription_progress";
      meeting_id: string;
      position_secs: number;
      progress: number;
    }
  | {
      event: "processing";
      meeting_id: string;
      state: ProcessingState;
    };

// ---- IPC ----

export type CommandErrorKind = "invalid_input" | "not_found" | "storage" | "internal";

export interface CommandError {
  kind: CommandErrorKind;
  message: string;
}

export interface PingStatus {
  ok: boolean;
  version: string;
  receiverCount: number;
}