# Architecture

## Overview

notsAI is a Tauri 2 (Rust + React) desktop application. The Rust core runs
in-process and is exposed to the React UI through typed Tauri commands and
events. There is no HTTP/HTTP sidecar in the MVP — an Axum/PostgreSQL server is
a later phase.

## Repository layout

```
apps/desktop/            Tauri 2 host + React/TS/Tailwind/shadcn-ui UI
packages/shared-types/   TypeScript types mirroring the Rust schema
packages/api-client/     typed Tauri-invoke bridge wrapper
crates/notsai-core/      domain models, event bus, service layer
crates/notsai-storage/   SQLx + SQLite, migrations, FTS5, file layout
crates/notsai-audio/     cpal microphone capture, rodio playback, ffmpeg import
crates/notsai-transcribe/ STTProvider trait + whisper.cpp engine + model manager
crates/notsai-ai/        AIProvider trait; Gemini/Groq/OpenRouter; incremental summarizer
crates/notsai-tauri/     Tauri command/event bridge (thin, no business logic)
```

## Data model

- **Raw data** lives on disk under `<app_data>/meetings/<meeting_id>/`:
  `recording.m4a`, `transcript.json` (or `.jsonl` chunks), imported sources.
- **Derived data** (notes, topics, decisions, action items, questions) lives in
  SQLite. This separation enforces the retention model: deleting "derived"
  never touches raw data, and deleting "raw" only happens after processing
  succeeds (unless the user configures otherwise).

## Event pipeline

Core emits events over a `tokio::sync::broadcast` channel:
`transcript_chunk`, `transcription_progress`, `processing_state`,
`recording_state`. The Tauri bridge forwards these to the UI as Tauri events.

AI processing is **incremental**: a rolling context is maintained and
summarized continuously. The full transcript is never dumped into a single
LLM call.

## Privacy

- AI mode: CLOUD / LOCAL / HYBRID with a strict privacy hierarchy
  (Maximum Privacy / Balanced / Maximum Speed).
- Explicit consent gates before microphone/recording and before any cloud AI
  call.
- Provider keys are stored, never logged, and never returned in full to the UI.

## Phases

1. Core MVP (this phase) — desktop notes, meetings, recording, local STT,
   cloud summarization, meeting history, Markdown export, FTS5 search.
2. Intelligent meetings — richer structure (decisions/action items/questions),
   incremental AI, speaker diarization.
3. Multimodal — screen capture + slide understanding, video embedding.
4. Meeting agent — join meetings, act on action items.
5. Cross-device — mobile app, account/sync, Axum/PostgreSQL server.
6. AI platform.
7. Extensions.