# notsAI - Project Context

## Purpose
macOS native voice notes app: record meetings, transcribe locally, then run AI notes on the transcript. This file records project state and decisions. Keep it updated after every codebase change (see AGENTS.md).

## Workspace
- Root: /home/pluto/Desktop/notsAI
- Cargo workspace: resolver 2, edition 2021, rust-version 1.80.
- No notsai-cli crate. The app/command surface lives in crates/notsai-tauri (src/lib.rs, src/main.rs, src/build.rs).

## Crates
- notsai-core: models, settings, traits, events, errors.
- notsai-storage: SqliteStore (sqlx), file layout helpers.
- notsai-audio: recording capture and WAV decode (cpal, hound, rodio).
- notsai-transcribe: WhisperEngine (whisper-rs), audio decode, model manager.
- notsai-ai: AI notes provisioning (provider abstraction + Summarizer with chunk/window logic).
- notsai-tauri: Tauri 2 command surface + app shell.
- apps/desktop (@notsai/desktop): React frontend (Vite + React + TS). Tauri launched from here via `pnpm tauri dev` (TAURI_APP_PATH=../../crates/notsai-tauri).

## Architecture
- Tauri 2 (Rust backend + React frontend). No HTTP sidecar in MVP.
- Raw data at <app_data>/meetings/<meeting_id>/: recording.m4a, transcript.json.
- Events over tokio::sync::broadcast (EVENT_CAPACITY = 256): transcript_chunk, transcription_progress, processing_state, recording_state.
- AppState { store: SqliteStore, settings_repo: FileSettingsRepository, event_bus: EventBus }.

## MVP Status
- Settings CRUD: done (get_settings, set_settings).
- Meetings CRUD + search: done (create_meeting, get_meeting, list_meetings, delete_meeting, search_meetings).
- Notes CRUD: list/get/add/export (list_notes, export_notes).
- ping: done (PingStatus { ok, version, receiver_count }).
- Transcription wiring (WhisperEngine -> segments -> transcript.json -> status Ready/Failed): backend done (transcribe_meeting); frontend wrapper + UI done (Transcribe button on "recorded" meetings).
- AI notes wiring (provider_from_config -> Summarizer -> notes -> status Ready/Failed): backend done (generate_notes, requires ai_consent; replaces prior notes before regenerating); frontend wrapper + UI done (Generate notes button on ready/processing meetings, Notes viewer).
- AI provider settings UI: done (desktop "AI provider" card: per-provider model/api_key/base_url inputs, active_provider radio, Save writes whole settings blob; see latest save point).
- Recording: backend done (record_meeting + stop_recording commands, recording_state events via mark_recording_failed; AppState.recorder holds active MicRecorder, raw capture written to <artifact_dir>/recording.wav then transcoded to recording.m4a); frontend wrapper + UI done (record/stop buttons, live transcript rendering via transcript_chunk + recording_state events, activeRecordingId state).
- Model download progress rendering: done (processing_state Downloading { downloaded_bytes, total_bytes } -> desktop inline progress row showing pct + formatBytes(downloaded) / formatBytes(total), cleared on complete/failed).
- Frontend wiring note: @notsai/api-client publishes to dist/ (main/types point at dist/index.js/d.ts); desktop resolves via dist, so after adding/editing a wrapper run `pnpm --filter @notsai/api-client build` before desktop typecheck.

## Domain Facts
- MeetingStatus: Created | Recording | Recorded | Transcribing | Processing | Ready | Failed (serde snake_case).
- Meeting::new(title) -> status Created + new Uuid.
- RetentionMode: FullRecording (default) | SmartNotes | NotesOnly.
- AIMode: Cloud | Local (default) | Hybrid.
- PrivacyLevel: MaximumPrivacy (default) | Balanced | MaximumSpeed.
- From<AIMode> for PrivacyLevel: Local->MaximumPrivacy, Hybrid->Balanced, Cloud->MaximumSpeed.
- ProviderKind: Local | Gemini | Groq | OpenRouter.

## Command Surface (notsai-tauri)
get_settings, set_settings, create_meeting, get_meeting, list_meetings, delete_meeting, search_meetings, list_notes, transcribe_meeting, generate_notes, record_meeting, stop_recording, ping. Helpers: forward_event, spawn_event_forwarder, write_transcript_artifact, mark_failed, mark_recording_failed. Setup/run builder chain.

## Development Gates
Docs: docs/DEVELOPMENT.md. Order:
1. pnpm lint && pnpm typecheck
2. pnpm test
3. cargo fmt --all --check
4. cargo clippy --workspace -- -D warnings
5. cargo test --workspace

## Current Work
Transcription + AI notes are wired end-to-end (backend commands, api-client wrappers, desktop UI in App.tsx), as are recording controls (record/stop), live transcript rendering, transcription model-download progress rendering, and the AI provider settings UI (per-provider model/api_key/base_url + active_provider radio). All verification gates currently green (see Session Save Point). "Generate notes" now only needs the user to supply valid provider config (an API key for cloud providers or a reachable local endpoint) after enabling consent. This session added three requested increments (see Save Point later 8): (A) two-step delete confirmation in the desktop UI, (B) faster transcription (thread cap clamped to 8) plus percent + remaining-time progress, and (C) `SttLanguage::Auto` = "Auto (detect & switch)" which per-chunk restricts candidates to {en,hi,mr}, decodes the top-ranked usable one, and reports the dominant language from per-chunk tallies. Next up: user in-app verification of Auto with a code-switched recording; optional backlog remains adding fr/es/ur/ar/ja via the forced-code pattern at Marathi-parity accuracy. Update this file whenever the codebase changes.

## Session Save Point (2026-09-20)
State verified this session:
- Not a git repo (git status/log fail with "not a git repository"); no commit/build/release unless explicitly asked.
- Create/delete meeting frontend increment VERIFIED end-to-end:
  - Runtime IPC contract confirmed from Tauri vendor sources: tauri-macros defaults to ArgumentCase::Camel (tauri-macros-2.6.3/src/command/wrapper.rs:51, key converted via to_lower_camel_case at 505-512) and the key is used via payload.get(key) in tauri-2.11.5/src/ipc/command.rs:97. So snake_case Rust params (delete_raw, meeting_id) map exactly to camelCase JS args (deleteRaw, meetingId). No api-client fix needed.
  - packages/api-client/dist/index.js already carries createMeeting(title) (line 10) and deleteMeeting(id, deleteRaw) (line 19); wrappers at src/index.ts:51 and :63.
  - App.tsx wiring confirmed: imports (10-11), handleCreateMeeting (167, trims/guards empty + busyId sentinel "new"), handleDeleteMeeting (182, deleteRaw: true), busyId (54), newMeetingTitle (58), create input/button (251-269), per-meeting Delete disabled while busy/recording (295-298).
  - Store delete semantics + tests live in crates/notsai-storage/src/store.rs: delete() at 241 with `if delete_raw` at 253; tests delete_missing_meeting_returns_not_found (554), delete_with_raw_removes_artifacts_and_cascades (563), delete_without_raw_keeps_artifacts (602).
  - Live boot check: `timeout 100 pnpm tauri dev` from apps/desktop → Vite ready, cargo incremental build finished in 1.53s, notsai-tauri binary ran with no panic/handler/runtime errors (timeout SIGTERM was the only terminator). No code changes, so gates remain green.
- Gates were green before this verification: pnpm lint/typecheck/vitest (3/3); cargo fmt --all --check; cargo clippy --all-targets -D warnings; cargo test --workspace = 74 tests, 0 failures (clippy fix previously applied at crates/notsai-ai/src/summarizer.rs:377).
- Tooling facts: display available (DISPLAY=:0, WAYLAND_DISPLAY=wayland-0). Playwright/webapp-testing NOT feasible for Tauri native window E2E (can't attach to native window; @tauri-apps/api invoke fails in plain browser) — backend verified via store unit tests + dev boot + IPC contract instead.
- Recording wiring completed (Todo 5): record/stop buttons (App.tsx 308-320) call handleRecordMeeting (155) / handleStopRecording (167); activeRecordingId state (60-62) guards buttons while recording; live transcript renders from onTranscriptChunk (410-416); onRecordingState wired (97-101). Wrappers verified at packages/api-client/src/index.ts: recordMeeting (71), stopRecording (75), onRecordingState (105), onTranscriptChunk (111), onTranscriptionProgress (119), onProcessingState (128).

## Session Save Point (2026-09-20, later)
- Todo 5 (model-download progress rendering) DONE + gates re-verified:
  - Backend emits ProcessingState::Downloading { downloaded_bytes: u64, total_bytes: u64 } (crates/notsai-core/src/events.rs:24-27); events keep snake_case field names (not case-converted like command args) — App.tsx reads event.state.downloaded_bytes / total_bytes and checks state === "downloading".
  - Desktop renders inline progress row (pct + `formatBytes(downloaded)` / `formatBytes(total)`) from downloadProgress state (55-59); row cleared on complete/failed.
  - Decision: formatBytes keeps one decimal for units > 0 (toFixed(1)) — tests lock "142.0 MB", "1.5 KB", "1.0 GB" (apps/desktop/src/format.test.ts, vitest 6/6).
  - Clippy fixes in crates/notsai-transcribe/src/model_manager.rs: progress callbacks changed from `&mut dyn FnMut(u64, u64)` to `&mut (dyn FnMut + Send)` in ensure/download/report_progress (closures capture only Send values); plus two `&file_name` -> `file_name` (lines 110/161) and saturating pct calc (~214).
  - Gates ALL green: pnpm typecheck/lint; vitest 6/6; cargo fmt --all --check; cargo clippy --workspace --all-targets; cargo test --workspace = 78 tests, 0 failures. Context.md MVP Status + Current Work updated accordingly.

## Session Save Point (2026-09-20, later 2) — "Generate notes" failure fixed (consent UI + error surfacing)
- Problem: generate_notes hard-fails with "AI notes require user consent (ai_consent)" because ai_consent defaults to false (settings.rs:117/137) and the desktop UI offered no way to enable it; backend errors were only console.error'd, so failures were invisible to users.
- Root cause confirmed: crates/notsai-tauri/src/lib.rs generate_notes (388) gates on settings.ai_consent (395-401). Secondary gap (still open): active_provider is None by default → falls back to providers.first() = Gemini (gemini-2.5-flash) with api_key: None (settings.rs:146-173; all default providers have api_key None), so the LLM call fails even after consent is granted.
- Frontend fix in apps/desktop/src/App.tsx:
  - Imported api-client setSettings aliased as `saveSettings` (import `setSettings as saveSettings`) to avoid colliding with the React state setter of the same name (TS6133/TS2345 otherwise).
  - Added `describeError(err)` helper (Error message / string / "unknown error").
  - Added `errorMsg` state + handleToggleAiConsent(enabled) (guards settings===null; clear err; await saveSettings({...settings, ai_consent: enabled}); setSettings(updated)).
  - All handlers (load effect, handleTranscribe, handleGenerateNotes, handleRecordMeeting, handleStopRecording, handleCreateMeeting, handleDeleteMeeting, loadNotes) now setErrorMsg(null) at start and setErrorMsg(describeError(err)) on catch (console.error kept as-is).
  - Error banner JSX (`section.card.card--error[role=alert]` with h2 "Error" + errorMsg) rendered after header.
  - Settings card now includes consent checkbox (label.settings-toggle): checked=settings?.ai_consent ?? false, disabled while settings null or busyId != null, labeled "Allow AI model access to transcripts".
- CSS added to apps/desktop/src/styles.css: .card--error (border + p color #f87171), .settings-toggle (flex/gap/margin-top/font-size/cursor), input accent-color var(--accent), disabled styles.
- Gates ALL green after fix: pnpm typecheck; pnpm lint; vitest 6/6. Cargo untouched this increment.
- OPEN GAP for next increment: without provider config UI (api_key/active_provider) or a reachable local Ollama endpoint, "Generate notes" still fails at the LLM call after consent is on. Decide: add provider settings UI, or document required manual config.

## Session Save Point (2026-09-20, later 3) — AI provider settings UI (closes the open gap)
- Problem: open gap from previous increment — active_provider defaults to None (settings.rs) so backend falls back to providers.first() = Gemini (gemini-2.5-flash) with api_key: None, so the LLM call fails even after consent. Users had no UI to set api_key/model/base_url or pick an active provider.
- Backend/contract facts confirmed (no Rust changes): ProviderConfig = { kind: ProviderKind; model: string; api_key?: string; base_url?: string } (shared-types/src/index.ts 118-124); AppSettings 126-137; ProviderKind = "gemini" | "groq" | "open_router" | "local". set_settings replaces the whole settings blob and returns AppSettings (api-client setSettings alias 47-49).
- Frontend fix in apps/desktop/src/App.tsx (pure UI; Rust + TS already carried the data):
  - providerDraft state { active: ProviderKind | null; providers: ProviderConfig[] } | null, synced from settings via useEffect (shallow-clone providers; null when settings null; empty api_key/base_url -> undefined -> omitted -> Rust None).
  - handleUpdateProvider(kind, patch) functional-updates the matching provider in draft; handleSaveProviderSettings persists { ...settings, active_provider: draft.active, providers: draft.providers } via saveSettings and updates settings/errorMsg.
  - "AI provider" card (full-width .card) after status-grid before Meetings: settings-head (h2 + Save, disabled while busyId != null || providerDraft === null), provider-grid of .provider/.provider--active cards with radio name="active_provider", Model text input, API key password input (placeholder "sk-…"), Base URL text input (placeholder "e.g. http://localhost:11434/v1"); all inputs + radios disabled while busyId != null; PROVIDER_LABELS = Gemini/Groq/OpenRouter/Local; null draft shows muted "loading provider settings…".
- CSS in apps/desktop/src/styles.css: .settings-head, .provider-grid (repeat(auto-fit, minmax(240px, 1fr))), .provider, .provider--active, .provider__head, .provider__field + focus/disabled rules.
- Gates ALL green after fix: pnpm typecheck (tsc --noEmit clean); pnpm lint (eslint clean); vitest 6/6 (format.test.ts). Cargo untouched this increment.
- Remaining "Generate notes" requirement is now user config: enable consent + enter a valid provider (cloud api_key or reachable local endpoint) via the new UI.

## Session Save Point (2026-09-20, later 4) — chunked Whisper transcription (fixes long code-switched audio)
- Problem: ~6 min recordings of code-switched Hindi/Marathi/English failed (or dropped text) because transcribe() ran ONE whole-file `full()` with a forced language, and a redundant whole-file `pcm_to_mel` + `lang_detect` pre-pass pegged every CPU core.
- Fix in crates/notsai-transcribe/src/engine.rs (all edits applied; workspace compiles, tests green):
  - Worker thread cap in `WhisperEngine::new`: `available_parallelism().map(|n| n.get()).unwrap_or(4).min(4).max(1)`.
  - `run_whisper` rewritten to a chunked loop. Signature (no language param): `fn run_whisper(model_path: &Path, samples: &[f32], threads: usize, meeting_id: Uuid, bus: &EventBus) -> Result<TranscriptionResult, CoreError>`.
  - Consts: CHUNK_SECS = 30.0, OVERLAP_SECS = 2.0, SAMPLE_RATE = 16000. `chunk_len = (CHUNK_SECS * sample_rate) as usize`; `step_len = ((CHUNK_SECS - OVERLAP_SECS) * sample_rate) as usize`; loop `while chunk_start < samples.len() { ... chunk_start += step_len; }`.
  - Per chunk: fresh `FullParams` (Greedy { best_of: 1 }, set_translate(false), print flags off, `set_n_threads(threads as i32)`, `set_language(None)` = auto-detect, set_suppress_blank(true)) re-registering both callbacks; then `state.full(params, chunk)`.
  - Segments whose start < overlap_deadline (= chunk_start_secs + OVERLAP_SECS) are skipped in the live callback AND the final `state.as_iter()` pass → no duplicated text; timestamps are absolute (data.start_timestamp / 100.0 + chunk_start_secs; note whisper timestamps are centiseconds).
  - Shared `seq: Arc<Mutex<u64>>` gives monotonic segment sequence across chunks; `lang_cell: Arc<Mutex<Option<String>>>` updated after each `full()` from `get_lang_str(state.full_lang_id_from_state())` and read by the live callback (stale-by-one per chunk accepted).
  - Progress callback maps per-chunk percent to cumulative `position_secs = chunk_start_secs + (pct/100)*chunk_total_secs`; `progress = (position_secs/total_secs*100).clamp(0,100)`.
  - Returns `TranscriptionResult { segments, language: None }`; `transcribe()` fills the language fallback with `language_code(self.language).to_string()` (String, not &str). SUPERSEDED for Auto: since Save Point later 8, `transcribe()` skips this fallback when `self.language == SttLanguage::Auto` (Auto never stamps the literal `"auto"`); `run_whisper` also now takes the language and computes `dominant_language` from per-chunk tallies for Auto.
- Verified whisper.cpp semantics before writing: `full()` recomputes mel per call and clears `state->result_all` at call start (6798-6801), so `as_iter()` must be read immediately per chunk; callbacks read `params` at call time (7000-7004 etc.) so per-chunk callback re-registration works.
- Gates: `cargo check -p notsai-transcribe`, `cargo check -p notsai-tauri` both clean; `cargo test --workspace` green (grep for FAILED/error = none). No front-end changes → pnpm gates untouched.
- Next verification if it comes up: run a real long/ code-switched recording through the engine to confirm chunk continuity / language switches; otherwise next backlog increment.

## Session Save Point (2026-09-20, later 5) — chunked smoke test passes end to end (per-chunk language seeding)
- Problem: `chunked_run_whisper_smoke` (engine.rs:493) asserted ≥1 emitted `TranscriptChunk` with `language: Some(_)`, but `lang_cell` was set only AFTER each `state.full()` returned, while segment callbacks fire DURING `full()` and read `lang_cell` while it was still `None`. In the failing run only chunk 1 emitted segments (later chunks hit "single timestamp ending - skip entire chunk", `seek = 1399`), so every emitted chunk carried `None`.
- Fix in crates/notsai-transcribe/src/engine.rs (edit applied; no code comments added per AGENTS.md): immediately before `state.full(params, chunk)`, seed `lang_cell` via mel-based auto-detect:
  ```
  if let Ok(lang_id) = state.pcm_to_mel(chunk, threads).and_then(|_| state.lang_detect(0, threads)) {
      if let Some(code) = get_lang_str(lang_id.0) {
          if let Ok(mut guard) = lang_cell.lock() { *guard = Some(code.to_string()); }
      }
  }
  ```
  The post-`full()` `get_lang_str(state.full_lang_id_from_state())` block is retained. Whether whisper.cpp reuses/recomputes mel inside `full()` is irrelevant: `lang_detect` runs on the mel just computed from the current chunk, before `full()`.
- whisper-rs 0.16.0 API surface confirmed at ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/whisper-rs-0.16.0/src/whisper_state/mod.rs: `pcm_to_mel` (line 56), `lang_detect(&self, offset_ms, threads) -> Result<(i32, Vec<f32>), WhisperError>` (line 205, requires pcm_to_mel/set_mel first), `full` (line 292), `full_lang_id_from_state` (line 333). NOTE: no high-level `State::detect_language` method exists (earlier summary was inaccurate); the method is `SessionState::lang_detect`. Sys binding `whisper_lang_auto_detect_with_state` confirmed in whisper-rs-sys-0.15.0 bindings.rs:5247.
- Gates ALL green: `cargo test -p notsai-transcribe chunked_run_whisper_smoke -- --nocapture` passes (~26s, all assertions incl. language: Some(_) at line 493); `cargo test --workspace` green (79 tests: 11+9+20+17+22, 0 failures); `cargo build -p notsai-transcribe` clean (no warnings). No front-end changes → pnpm gates untouched.
- Next: real long/code-switched recording through the engine to confirm chunk continuity / language switches, or next backlog increment.

## Session Save Point (2026-09-20, later 6) — first TranscriptionProgress event fixed (smoke test green)
- Problem: `chunked_run_whisper_smoke` (engine.rs:437) failed with `expected position near audio start, first=Some((27.900000000000002, 39.857143))` (assert `any p <= 2.0 + 1e-9`). Chunk 1's first progress callback (percent 0 -> position 0.0) was suppressed by the throttle because `last_published_position` was initialized to `0.0` and the guard is `position >= last + CHUNK_PROGRESS_THROTTLE_SECS (1.0)`; whisper's next callback for the failing 70s sine tone lands at ~93% of chunk 1 (27.9s), so the first published event was 27.9s.
- Fix in crates/notsai-transcribe/src/engine.rs:183 (one-line): `let last_published_position: Arc<Mutex<f64>> = Arc::new(Mutex::new(f64::NEG_INFINITY));` so the run's very first progress callback always publishes (position 0.0), satisfying the ≤2s bound; monotonicity preserved (next events >= 27.9s).
- Confirmed via /tmp/notsai-verify/smoke.txt decoder logs for 70s tone: chunk decode attempts start `seek = 2800, seek_delta = 2800`, fail at temperature 0.00 (avg_logprobs -1.13 < -1.00), retry 0.20, then `single timestamp ending - skip entire chunk` (`seek = 2999, seek_delta = 199`; last chunk `seek = 1399`). Progress callbacks are coarse/jumpy on skipped chunks; whisper-rs whisper.cpp was NOT located under `~/.cargo/registry/src/*/whisper-rs-*/src/whisper.cpp` (glob mismatch) — not needed for this fix.
- Gates ALL green after fix: `cargo test -p notsai-transcribe chunked_run_whisper_smoke -- --nocapture` PASSED (28.61s); verify regression `NOTSAI_VERIFY_AUDIO=/tmp/notsai-verify/notsai-code-switched.wav cargo test -p notsai-transcribe chunked_run_whisper_code_switched_audio -- --nocapture` PASSED (24.87s, per-chunk languages {} ok); `cargo test --workspace` green (notsai-transcribe 23 passed incl. smoke; notsai-ai 11, notsai-audio 9, notsai-core 20, notsai-storage 17; tauri libs 0 each). No front-end changes -> pnpm gates untouched.
- Constraints preserved: EventBus drain NOT made Lagged-tolerant; EVENT_CAPACITY still 256.

## Session Save Point (2026-09-20, later 7) — input normalization added (quiets/garbles fix)
- Problem: real-world recordings of code-switched Hindi/Marathi/English (rider speaking from a chair, notebook on lap) garbled English and never auto-switched/transcribed Hi/Mr. Prior chunked fix (Save Point 4/5) made auto-detect correct, but weak base ASR still fails when audio level is too low for `lang_detect` + recognizer.
- Root cause finalized: transcription does NOT lock to English. `run_whisper` sets `params.set_language(None)`; per-chunk auto-detect seeds via `pcm_to_mel` + `lang_detect` then `full_lang_id_from_state` and only works on multilingual models (all models multilingual). Missing Hi/Mr = audio too quiet, not a language config issue.
- Fix (implemented in crates/notsai-transcribe/src/audio.rs + engine.rs):
  - `pub fn normalize(samples: &mut [f32])` + private `fn levels(samples: &[f32]) -> Option<(f32, f32)>` in audio.rs. Semantics: skip if rms <= 0.002 (silence/dead) or rms >= 0.05 (healthy); else `gain = (0.05 / rms).min(16.0)`; peak guard: if `peak * gain > 0.95` then `gain = (0.95 / peak).clamp(1.0, gain)`; if `gain <= 1.0` return untouched; else multiply every sample. Only boosts, never attenuates, never amplifies silence/noise floor. RMS = sqrt(sum_sq/len) in f64; peak = max abs; `levels` returns None for empty slice. No code comments (AGENTS.md).
  - `transcribe()` in engine.rs now: `let mut samples = audio::decode(source.path())?;` then `audio::normalize(&mut samples);` BEFORE the existing `samples.clone()` into `spawn_blocking`, so only one boosted copy is made (~230MB for 1h at 16kHz f32; no extra full-audio copy). `run_whisper` signature untouched → direct `run_whisper` unit tests don't exercise normalize (accepted; covered by audio.rs unit tests + verify gate).
  - 5 normalize unit tests added in audio.rs tests module before `fn ffmpeg_present()`: `normalize_boosts_quiet_signal` (sine amp 0.01 → rms > 0.03, peak ≤ 0.95), `normalize_leaves_healthy_levels_untouched` (sine amp 0.1 → bit-for-bit unchanged), `normalize_ignores_silence` (zeros unchanged), `normalize_ignores_empty_input` (empty no-op), `normalize_caps_gain_to_protect_peaks` (quiet sine + one sample 0.3 → peak ≤ 0.951, gain applied). Helper `fn sine(amplitude: f32, length: usize) -> Vec<f32>` (440Hz at SAMPLE_RATE).
- Gates ALL green after fix: `cargo fmt --all`; `cargo test -p notsai-transcribe` → 28 passed, 0 failed (5 new normalize tests + smoke + existing); verify regression `NOTSAI_VERIFY_AUDIO=/tmp/notsai-verify/notsai-code-switched.wav cargo test -p notsai-transcribe chunked_run_whisper_code_switched_audio -- --nocapture` PASSED (1 passed, 27 filtered, 25.53s); `cargo test --workspace` green (notsai-transcribe 28 passed, 0 failed; no FAILED/error across workspace). No front-end changes → pnpm gates untouched.
- Optional companion NOT chosen: upgrade to the Small multilingual model (better Hi/Mr recognition) when transcription quality still trails expectations after normalize; model manager already downloads + caches on demand (`NOTSAI_MODELS_DIR` / `~/.local/share/ai.not.notsai/models`).

## Session Save Point (2026-09-21) — Hi/Mr verification verdict + empty-transcript root cause (no code change)
- Verification session: validated Hindi/Marathi recognition quality (Small vs Base) on /tmp/notsai-verify/notsai-code-switched.wav (83.95s, music intro ~15s then code-switched speech) and its 28s slices hi-28 / mr-28 / en-28. All downloads/tests local/offline; models dir holds ggml-small.bin (487.01 MB) alongside ggml-base.bin.
- Decision: KEEP WhisperModel::Base as the code default. Small is available on demand via NOTSAI_VERIFY_MODEL / the model manager if Hi/Mr quality must improve.
- Empty transcript chunk stream ROOT CAUSE: chunk 0 (~15s instrumental intro) decodes as "(upbeat music)" — auto-detect en (p=0.718), single music segment. That segment is dropped (chunk-relative start=0 < overlap_deadline 2.0s kills dedup AND position < deadline skips), so chunk 0 emits no TranscriptChunk; when later chunks hit "single timestamp ending - skip entire chunk" the same callbacks yield nothing. NOT a language-lock bug: run_whisper keeps set_language(None).
- Probe evidence (all temp probes removed after use):
  - Chunk-level Base decode: chunk 0 en (p=0.718) "(upbeat music)"; chunk 1 en (p=0.196) "(speaking in foreign language)"; chunk 2 ro (p=0.188) temp-1.00 gibberish, failed avg_logprobs -3.21385 < -1.00000.
  - RMS banding: chunk 0 ≈ 0.20–0.45 pulsing (music), speech chunks ≈ 0.02–0.30 → chunk 0 is music.
  - Forced-language diagnostic (NOTSAI_VERIFY_MODEL=ggml-small.bin; 9 decodes of the 28s slices; test ok, 282.29s): mr-28 forced hi AND forced mr → real Devanagari (~250 of ~282 chars) → Hi/Mr texts ARE recognized by Small; prod failure is auto-detect dominant-language mispick, not missing grammar. hi-28 forced hi/mr → near-zero non-space chars → that "Hindi" slice is largely music/no-speech (corroborates chunk-0-is-music). en-28 forced en → 1357 chars → normal English decode.
- Conclusion: missing Hi/Mr = auto-detect limitation + dominant-music/quiet chunks, not a config/engine bug. Engine behavior correct as implemented.
- Cleanup + gates: removed forced_language_diagnostic test + all temporary probes from crates/notsai-transcribe/src/engine.rs (tests mod intact; file now 664 lines); `cargo fmt`; `cargo test -p notsai-transcribe` → 28 passed, 0 failed (chunked_run_whisper_smoke PASSED, 26.91s). No other code changes → pnpm gates untouched.

## Session Save Point (2026-09-21, later 2) — runner-up language retry pass for low-confidence/misfired chunks
- Goal (from earlier Hi/Mr investigations): when a chunk's auto-detected dominant language is low-confidence (`dominant_prob < 0.5`, LANG_DETECT_CONFIDENCE_THRESHOLD) OR the auto-language pass produced no real content (`real_chars == 0`), re-decode the chunk forcing the runner-up language, then commit/publish only the better pass.
- Implementation in crates/notsai-transcribe/src/engine.rs (all edits applied; build + tests green):
  - Consts `LANG_DETECT_CONFIDENCE_THRESHOLD = 0.5`, `MIN_RUNNER_UP_PROB = 0.05`; structs `ChunkSegment { text, start, end }`, `PassSegments { real_chars, mean_no_speech, segments, language }`; helper `content_chars` (excludes whitespace/punctuation incl. `। ॥ … « » ” ’ ‘ – — ·`); `detect_languages(&state, threads)` -> `(Option<i32> dominant_id, Option<i32> runner_up, f32 dominant_prob)` returning the argmax pair; `decode_pass(state, chunk, threads, meeting_id, bus, chunk_start_secs, chunk_total_secs, total_secs, language, last_published)` -> `Result<PassSegments, CoreError>`.
  - `decode_pass` builds its own `FullParams` (whisper-rs `full(&mut self, params, ...)` consumes params by value), sets `set_language(language.as_deref())`, registers ONE progress callback, runs `state.full(params, chunk)`, then reports chunk language as `get_lang_str(state.full_lang_id_from_state()).or(language)`, and iterates `state.full_n_segments()`/`get_segment(i)` collecting non-empty segments (timestamps `/100.0` = centiseconds).
  - `run_whisper` chunk loop: per chunk `pcm_to_mel` -> `detect_languages` -> first `decode_pass` with `auto_lang`; retry when `first.real_chars == 0 || dominant_prob < threshold` and runner-up prob >= MIN_RUNNER_UP_PROB, second pass forces `Some(code)`; chosen = larger `real_chars`, else tie-broken by lower `mean_no_speech`, else first; commit loop unchanged (`overlap_deadline` skip, `seq`, `CoreEvent::TranscriptChunk { language: chosen.language }`, meeting-level `language: None`).
  - FIXED progress regression: the retry pass previously republished progress from ~`chunk_start_secs + 1.0`, regressing below already-published progress and failing the test's "roughly non-decreasing" assertion. Now a single shared `last_published: Arc<Mutex<f64>>` (init `f64::NEG_INFINITY`) is created once in `run_whisper` and passed into BOTH `decode_pass` calls; callback locks it and skips while `position_secs < *last + CHUNK_PROGRESS_THROTTLE_SECS`.
  - Cleanup: removed never-read `no_speech_probability` field from `ChunkSegment` (mean_no_speech computed during decode via `segment.no_speech_probability()` method) -> zero warnings on build.
- whisper-rs 0.16.0 API notes reaffirmed: `WhisperSegment::start_timestamp()`/`end_timestamp()` are METHODS (E0615 if called as field); `FullParams::set_language(Option<&str>)`.
- Gates ALL green after fix: `cargo build -p notsai-transcribe` clean (no warnings); `cargo test -p notsai-transcribe` -> 28 passed, 0 failed (~32-38s, includes chunked_run_whisper_smoke with progress monotonicity assertion). No front-end changes -> pnpm gates untouched.
- Optional companion NOT changed: engine still defaults to Base model (Small on demand via NOTSAI_VERIFY_MODEL / model manager) if Hi/Mr quality must improve further.

## Session Save Point (2026-09-21, later) — context.md correction
- Correction: the previous "no code changes" save point was inaccurate; the language wiring described in the save point below IS applied and verified in the code. This entry documents the correction only (no code change).
- State continues: not a git repo; Base remains the default Whisper model (Small on demand via NOTSAI_VERIFY_MODEL / model manager).
- Gates: cargo test -p notsai-transcribe 28 passed/0 failed; pnpm gates untouched (no front-end changes).
- Next: pick next backlog increment; optional companion remains upgrading to Small multilingual model when Hi/Mr quality trails expectations.

## Session Save Point (2026-09-21, later 3) — configured SttLanguage wired into per-chunk decode (Hi/Mr forced, En unchanged)
- Goal: the earlier runner-up retry (later 2) only forced the configured code on a RETRY after a low-confidence/empty auto-detect pass; a confident-but-wrong auto-detect (e.g. dominant en on a Hi/Mr chunk, dominant_prob >= threshold) never triggered it, so the configured language never reached the decoder on such chunks. Now the configured SttLanguage is forced into the FIRST decode pass for Hi/Mr; En keeps the pure auto-detect path.
- Implementation in crates/notsai-transcribe/src/engine.rs (applied; build + tests verified):
  - `run_whisper` now takes `language: SttLanguage` (line 297); `transcribe` passes `self.language` (line 252) and still stamps `result.language = Some(language_code(self.language))` (lines 257-259).
  - Chunk loop: `detect_languages` (334-337) -> `auto_lang` from dominant id (339-341) -> `forced_code = match language { SttLanguage::En => None, other => Some(language_code(other).to_string()) }` (343-346) -> `primary_lang = forced_code.clone().or_else(|| auto_lang.clone())` (347) -> first `decode_pass(..., primary_lang, ...)` (349-360).
  - `decode_pass` signature `language: Option<String>` (104) with `params.set_language(language.as_deref())` (114); post-`full()` language report `get_lang_str(state.full_lang_id_from_state()).or(language)` (138).
  - Runner-up handling: `best_chars = first.real_chars` (362), `runner_up_code` (363-365); extra passes run via closure only when `forced_code.as_deref() != Some(code)` (369-372) so the forced Hi/Mr pass is never duplicated and En keeps auto + runner-up retry behavior.
  - Fixed borrow-checker E0502 by restructuring the extra-pass closure; removed now-unneeded `mut`/`drop(run_extra)` (zero-warning build). Commit/publish loop unchanged (overlap_deadline skip, seq, TranscriptChunk { language: chosen.language }, meeting-level `language: None`).
- Gates: `cargo build -p notsai-transcribe` clean (no warnings); `cargo test -p notsai-transcribe` -> 28 passed, 0 failed.
- Verify gate `chunked_run_whisper_code_switched_audio` NOT re-run: env-gated on NOTSAI_VERIFY_AUDIO and /tmp/notsai-verify/notsai-code-switched.wav was lost to a /tmp clear. Next manual gate: restore that WAV, then `NOTSAI_VERIFY_AUDIO=<wav> NOTSAI_VERIFY_MODEL=small cargo test -p notsai-transcribe chunked_run_whisper_code_switched_audio -- --nocapture`.
- Optional companion unchanged: Base default; Small on demand via NOTSAI_VERIFY_MODEL / model manager.

## Session Save Point (2026-09-21) — recording playback in desktop app (asset protocol + Play/Hide audio)
- Goal: play back a meeting's recording in-app. Recordings are stored as `<app_data>/meetings/<meeting_id>/recording.m4a` on disk (raw capture .wav is transcoded, wav not kept), so playback needs a way for the frontend to fetch/stream that file.
- Decision: enable Tauri 2's `asset` protocol (`asset:` URLs via `convertFileSrc`) instead of adding an IPC playback command (which would require streaming binary data over IPC and extra custom binary IPC plumbing). `capabilities/default.json` (`["core:default", "opener:default"]`) needs NO permission change for asset URLs.
- Backend change (crates/notsai-tauri/tauri.conf.json, line 26): `"security": { "csp": null, "assetProtocol": { "enable": true, "scope": ["$APPDATA/**"] } }`. Identifier is `ai.nots.notsai`, so `$APPDATA` = `~/.local/share/ai.nots.notsai` on Linux, which covers the recordings dir. CSP stays null (dev-only MVP; no remote content).
- Frontend change (apps/desktop/src/App.tsx, line refs grep-verified):
  - `import { convertFileSrc } from "@tauri-apps/api/core"` (line 28); dep `@tauri-apps/api` already at ^2.11.1 (not pinned in an effort to stay flexible; add-only change).
  - `const recordingSource = (path: string) => convertFileSrc(path, "asset")` helper (lines 66-67).
  - `playingId: string | null` state (line 85); toggle handler inside the meeting row's body.
  - Per-meeting-row toggle button (line 594): label `playingId === m.id ? "Hide audio" : "Play"`, disabled while child busy, toggles `playingId` null <-> m.id.
  - Expandable player (lines 598-602): only rendered when `playingId === m.id && m.recording_path != null`; `<audio controls preload="metadata" style={{ width: "100%" }}>` with `<source src={recordingSource(m.recording_path)} type="audio/mp4" />` so the audio controls widget is the player (not an always-mounted list of players).
- Frontend styling (apps/desktop/src/styles.css, line 202): `.meeting__player { margin-top: 0.75rem; width: 100%; }` (spacing + full-width audio controls).
- Shared type already supported it: `Meeting.recording_path?: string` exists (packages/shared-types/src/index.ts ~26); `transcribe_meeting` writes `recording_path` into Meeting (crates/notsai-tauri/src/lib.rs ~259-270) and reads it back (~296-335), so no type/backend command changes were needed.
- Gates: `pnpm --filter @notsai/desktop typecheck` (tsc --noEmit) PASSED clean. Not yet runtime-verified (typecheck cannot validate asset-protocol file serving or the audio element); next manual gate is running `pnpm --filter @notsai/desktop tauri dev` and playing a recording.
- Constraint: convertFileSrc returns `asset:` URLs in the webview; on the native tauri window these resolve via the protocol scope above. If playback fails at runtime, re-check the scope match against the on-disk app-data path.
## Runtime Verification Follow-up (2026-09-21) — boot/render confirmed; audio needs human
- Outcome of the manual gate from the playback save point (run `pnpm --filter @notsai/desktop tauri dev` + play a recording): runtime BOOT/REnder confirmed headlessly; audible playback + Play/Hide UI pixel confirmation NOT automatable here.
- Method: app relaunched under forced X11 (`GDK_BACKEND=x11`), running pid 420568, X11 client window 0x1600003 (Position 360,170, 1200x800 on 1920x1080). Fullscreen screenshot `/tmp/opencode/notsai-screen.png` (1,476,751 bytes, Sep 21 16:50; `spectacle -b -n -f -o`, since `import`/`magic import` are broken).
- OCR (tesseract; native `/etc/alternatives/tesseract` lacked eng.traineddata -> bundle at `/opt/autopsy/autopsy/Tesseract-OCR/tessdata` as TESSDATA_PREFIX): confirmed the app window renders real content — week-view header `Monday, 21 September 2026` — and is NOT a blank window. The complex weekly-timeline grid + dark theme (mean brightness 73.8, 91% dark pixels) defeats row-level OCR: no meeting-row text, Play/Hide/Sound labels, or audio widget could be extracted across psm 3/6/11, crop+2x upscale, negate, Otsu threshold 112, and horizontal banding.
- Sonic check not possible headlessly (no reliable audio capture of the virtual sink verified in this session), so audible playback of the 3 recordings (recording.m4a each) and the visible Play/Hide toggle remain pending HUMAN confirmation.
- To confirm visually, user can check `/tmp/opencode/notsai-screen.png` or the live window; if playback fails at runtime, first re-check assetProtocol scope `$APPDATA/**` vs on-disk app-data path per the playback save point constraint.

## Verify Follow-up (2026-09-21) — chunked_run_whisper_code_switched_audio PASSED
- Outcome of the pending gate recorded in the 2026-09-20 later-6 save point: the regenerated 121.46s code-switched WAV was restored, and `NOTSAI_VERIFY_AUDIO=/tmp/notsai-verify/notsai-code-switched.wav NOTSAI_VERIFY_MODEL=small cargo test -p notsai-transcribe chunked_run_whisper_code_switched_audio -- --nocapture` PASSED (~147.48s).
- Per-chunk languages: chunk 0 "en", chunk 1 "en" -> `by_chunk.len() == 2 >= 2`, so the multi-chunk language-detection gate (`by_chunk.len() >= 2` in engine.rs:815) is met. Progress-monotonic and end-time asserts also green.
- Known limitation (unchanged): whisper auto-detect still labels the hi/mr espeak slices "en" (only 2 of 5 chunks carried a language segment; the rest hit the `single timestamp ending - skip entire chunk` path with seek=2999, seek_delta=311). Small recognizes Devanagari correctly when language is forced, so prod failure remains auto-detect mispick, not a language lock. No assertion requires distinct languages; the structural multi-chunk gate is the acceptance bar.
- Test audio: /tmp/notsai-verify/notsai-code-switched.wav = 121.46s (1943359 samples, 3886796 B), 5 chunks at chunk_boundary=28 (CHUNK_SECS=30.0, OVERLAP_SECS=2.0); built by concatenating 9 clips (en->sil->hi->sil->en->sil->mr->sil->en), each padded via `ffmpeg -af "adelay=1000,apad=pad_dur=1000ms" -ar 16000 -ac 1 -c:a pcm_s16le`.
- Supersedes the older "24.87s, per-chunk languages {} ok" entry and the earlier FAILED run ("got 1") on the old 75.3s WAV; the /tmp-cleared-WAV blocker from the earlier gate note is resolved.

## Session Save Point (2026-09-21, later 4) — stt_language dropdown added to desktop Settings UI
- User-reported real bug: spoke Marathi (mixed with English/Hindi), clicked transcribe, transcript showed only "LIVE TRANSCRIPT" plus the identical English line "i would like to transcribe this in my language." repeated 8 times — Marathi not transcribed at all. Root cause: the user's test ran with the default `stt_language = en` (auto-detect; there was NO UI to set it), and the Base model hallucinated a repeated English tail on audio it could not auto-detect. Enforced-language wiring for Hi/Mr was already in the backend (Save Point later 3), but unreachable from the UI.
- Frontend change in apps/desktop/src/App.tsx (typecheck + lint clean):
  - Imported `SttLanguage` into the `@notsai/shared-types` type import (line 2-10).
  - Added `STT_LANGUAGE_LABELS: Record<SttLanguage, string> = { en: "English (auto-detect)", hi: "Hindi", mr: "Marathi" }` after NOTE_KIND_LABELS.
  - Added `handleSetSttLanguage(language)` mirroring `handleToggleAiConsent` (guards settings===null; `saveSettings({ ...settings, stt_language })`; setSettings(updated); setErrorMsg on catch).
  - Settings card: status line now shows `${ai_mode} · ${whisper_model}` (dropped raw stt_language since it's now a picker); added `<label className="settings-field">Transcript language<select>` with options from STT_LANGUAGE_LABELS, value = settings?.stt_language ?? "en", disabled while settings null or busyId != null, onChange -> handleSetSttLanguage.
- CSS in apps/desktop/src/styles.css: `.settings-field` (flex/gap/margin-top/font-size) + `.settings-field select` (padding, radius, border, background `var(--surface, #1e1e2e)`, color inherit, cursor) + `:disabled` (opacity 0.5, not-allowed).
- Gates: `pnpm run typecheck` (tsc --noEmit) and `pnpm run lint` (eslint src) both clean in apps/desktop. Cargo untouched this increment.
- User-verify step: run `pnpm tauri dev` from apps/desktop (`TAURI_APP_PATH=../../crates/notsai-tauri` baked into the tauri script), select Hindi or Marathi in Settings, record, transcribe. En keeps auto-detect; Hi/Mr forces the configured code into every chunk decode (first pass + runner-up retry).
## User-Verify Follow-up (2026-09-21) — Marathi now transcribes (accuracy rough)
- Outcome of the user-verify step in Save Point later 4: user ran `pnpm tauri dev`, selected Marathi in Settings -> Transcript language, recorded mixed speech, and Marathi IS now transcribed (no more repeated-English hallucination). Status: core bug FIXED in-app.
- Caveat from user: Marathi accuracy is "not that accurate" but "definitely better than before". Hindi untested yet; assumed identical to Marathi (same forced-code path, Hi/Mr behave the same).
- Next-step decision pending: optional companion from earlier save points — upgrade default Whisper model from Base to Small when Hi/Mr quality trails expectations (Small recognized Devanagari correctly when forced in the code-switched verify gate). No new code changed this increment.

## Session Save Point (2026-09-21, later 5) — user verification + multi-language plan + model-default finding
- User verification recorded: Marathi now transcribes in-app via the Settings -> Transcript language picker (Save Point later 4). It renders as Latin-script transliteration of Marathi/English with rough accuracy ("not that accurate ... definitely better than before") and NO repeated-English hallucination lines. Hindi untested but uses the exact same forced-code path, so treated as identical.
- NEW user planning directive (no implementation now): a FUTURE update should add French, Spanish, Urdu/Arabic, and Japanese, and each must work at least as well as current Marathi/Hindi accuracy — current Marathi accuracy is effectively the parity baseline. All four are whisper.cpp-supported language codes (fr/es/ur/ar/ja). Adding one language = one new SttLanguage enum variant in packages/shared-types/src/index.ts (~109) + whisper tag in `language_code()` (crates/notsai-transcribe/src/engine.rs) + STT_LANGUAGE_LABELS entry (apps/desktop/src/App.tsx). They should follow the forced-code pattern (like Hi/Mr), NOT auto-detect, since auto-detect caused the repeated-English hallucination. Urdu/Arabic code-switching accuracy is materially harder at Base size, so model size directly affects the parity requirement.
- Model-default finding (CORRECTS earlier "Base remains the default" entries): `WhisperModel::default()` is ALREADY `Small` in code (crates/notsai-core/src/settings.rs:73-80 `#[default] Small`; test :200 asserts it), yet the user's running app used Base. Hypothesis: persisted `settings.json` pins `"whisper_model": "base"`; `#[serde(default)]` only applies when the field is MISSING, so changing the Rust default does not change a stored value. Confirm via FileSettingsRepository (crates/notsai-storage/src/settings.rs, SETTINGS_FILENAME="settings.json", load() :44, save(); tests :94/:102). Runtime path: crates/notsai-tauri/src/lib.rs:308 settings_repo.load() -> :316 settings.whisper_model -> WhisperEngine::new; FileSettingsRepository::in_dir(app_data_dir) :597. No UI exists to change the model; App.tsx:407 only displays `${ai_mode} · ${whisper_model}`. engine.rs:558 `WhisperModel::Base` is test-only.
- Accuracy decision still PENDING with user: accept Base accuracy (close workstream) vs. upgrade the deployed default to Small (recommended — ~487MB model, slower decode; stronger floor for future fr/es/ur/ar/ja parity). No code changed until user chooses.
- Next: user decision on accuracy path; if upgrade, first read notsai-storage/src/settings.rs to confirm the persisted pin + any migration/versioning, then implement so the running app actually uses Small.

## Session Save Point (2026-09-21, later 6) — persisted whisper_model migrated Base -> Small (deployed default is now Small)
- User decision (from later 5): upgrade the deployed/de facto default to Small. `WhisperModel::default()` was already `Small` in code but the running app used Base because persisted `settings.json` pins `"whisper_model": "base"` and `#[serde(default)]` only applies to MISSING fields, never to a stored value.
- Fix in crates/notsai-storage/src/settings.rs (load(), lines 53-58): value-based idempotent migration — `let mut settings: AppSettings = serde_json::from_slice(&bytes)?; if settings.whisper_model == WhisperModel::Base { settings.whisper_model = WhisperModel::Small; tokio::fs::write(&self.path, serde_json::to_vec_pretty(&settings)?).await?; }`. Rewrites via direct `tokio::fs::write`, NOT `self.save()` (the mutex is already held inside `load()`; `tokio::sync::Mutex` is not reentrant). No `settings_version` field added: it would break `missing_fields_fall_back_to_defaults` (deserializing `{}` must equal `AppSettings::default()`) and would be dropped by the frontend TS round-trip.
- E0282 fix: explicit `let mut settings: AppSettings` annotation once field access was added to the deserialize result.
- New test `load_upgrades_legacy_base_model_to_small` (settings.rs:98-110): writes `{"whisper_model": "base"}`, loads, asserts `WhisperModel::Small` AND on-disk `"whisper_model": "small"`. Earlier botched edit on `load_missing_file_returns_defaults` repaired (`let dir = temp_dir("missing");` restored).
- Gates ALL green: `cargo test -p notsai-storage` -> 18 passed, 0 failed (incl. new migration test); `NOTSAI_VERIFY_MODEL=small cargo test` -> full workspace green (all `test result: ok`, 0 failed; a `test result` line showed 20 for one crate vs 18 for `-p notsai-storage` — minor counting difference, all ok; NOTSAI_VERIFY_MODEL is irrelevant to storage, kept as an extra gate). No front-end changes -> pnpm gates untouched.
- Runtime effect: next app start, `settings_repo.load()` (crates/notsai-tauri/src/lib.rs:308) migrates the persisted `settings.json` `base` -> `small`, so the running app uses Small. ~466MB model download on first run if ggml-small.bin is not already cached in the models dir (it is present from earlier verify work: 487.01 MB).
- User-verify step: run `pnpm tauri dev` from apps/desktop, re-record + transcribe Marathi (and Hindi) — expect better Devanagari accuracy than the Base-era "Latin transliteration, rough" result.

## Session Save Point (2026-09-21, later 7) — persisted settings.json confirmed: migration effective on disk (deployed model = Small)
- Goal: confirm the migration from Save Point later 6 actually rewrote the persisted file (a transient/app-start was all that was needed).
- Confirmed: `/home/pluto/.local/share/ai.nots.notsai/settings.json` now reads `"whisper_model": "small"` (plus `"retention": "full_recording"`, `"ai_mode": "local"`, `"stt_language": "en"`, `"ai_consent": true`, 4 default providers, `"active_provider": null`). So `settings_repo.load()` ran, detected the legacy `base`, and rewrote to `small` — the running app now uses Small.
- PATH CORRECTION: the app-data dir is `/home/pluto/.local/share/ai.nots.notsai` (from tauri identifier `ai.nots.notsai`), NOT `ai.not.notsai` as written in Save Points 2026-09-20 later 7 (line 152) and 2026-09-21 later 6. Corrected here; treat `ai.not.notsai` references as a typo.
- Dir contents (full listing): `settings.json`, `notsai.db`, `notsai.db-wal`, `notsai.db-shm`, `models`, `storage`, `mediakeys`, `WebKitCache`, `CacheStorage`, `hsts-storage.sqlite`, and 3 meeting UUID dirs (`116ca0f8-…`, `4bb3e2c4-…`, `67d9ff3f-…`).
- Intermittent-access episode RESOLVED: earlier `ls`/`stat`/Python `os.listdir` alternately hit ENOENT on the dir and file; it was transient (stale/negative dentry or momentary cache state) — a fresh Python `os.path.exists` + `open` now succeeds. Root filesystem `/home` is btrfs on `/dev/nvme0n1p2` with `btrfs device stats` all zero (no read/write/corruption/generation errors), so no FS integrity concern.
- Gates NOT re-run: this was a read-only confirmation of on-disk state; no code changed since the all-green gate run in Save Point later 6, so re-running cargo/pnpm gates would be redundant (AGENTS.md rule 3).
- Remaining: HUMAN in-app verify of Small (re-record Marathi + Hindi, `pnpm tauri dev`) — not automatable. Optional backlog: add fr/es/ur/ar/ja via the forced-code pattern at Marathi-parity accuracy.

## Session Save Point (2026-09-21, later 8) — three user-requested increments: delete confirm, faster transcription + ETA, auto language detection/switching
- Three user requests implemented this session. All Rust AND frontend gates green; no committed build/release (not a git repo).

### Increment A — delete confirmation (Task 1)
- Problem: deleting a meeting was a single irreversible click.
- Frontend-only. `confirmDeleteId` state added in apps/desktop/src/App.tsx (~106). The per-meeting Delete button now arms an inline two-step confirm ("Delete?" + confirm/cancel) instead of deleting immediately; `setConfirmDeleteId(null)` clears it after confirm/cancel. Delete remains disabled while `busyId != null` or recording.
- CSS in apps/desktop/src/styles.css: `button.danger { background:#f87171; color:#111216 }` and `.confirm-hint { color:#f87171; font-size:0.8rem; align-self:center }`.
- `handleDeleteMeeting` still calls `deleteMeeting(id, deleteRaw: true)` (Save Point 2026-09-20 behavior unchanged).

### Increment B — faster transcription + percent/remaining-time progress (Task 2)
- Perf: worker threads capped in `WhisperEngine::new` to `.clamp(1, 8)` (was 4) — machine has 16 logical cores (Ryzen 7 4800H), so 8 roughly halves wall-clock decode vs the old cap while leaving headroom; decode still runs inside `spawn_blocking` (UI thread never blocked).
- Progress: backend already emits per-chunk cumulative `transcription_progress` percent. Desktop added `transcribeProgress` state (App.tsx ~86) and renders it next to the transcribe control: `${pct}% · ~${formatDuration(secsRemaining)}` (~621-631). `formatDuration` is imported from the existing `src/format.ts` (already unit-tested).
- No API/event contract change: reuses the existing `onTranscriptionProgress` binding in packages/api-client (Save Point 2026-09-20 later 5).

### Increment C — Auto language detection + switching (Task 3)
- Problem: `stt_language` had no way to follow a speaker who code-switches mid-meeting; `En` auto-detect produced repeated-English hallucinations (Save Point later 4/5) and forced `Hi`/`Mr` locks one language for the whole file.
- New variant `SttLanguage::Auto` (serde `"auto"`, label "Auto (detect & switch)") in crates/notsai-core/src/settings.rs (~line 70); existing `En` keeps its legacy unbounded auto-detect behavior; `Hi`/`Mr` still force their code on EVERY chunk. Persisted default remains `en` (settings.json unchanged), so the feature is opt-in via the picker.
- `language_code()` in crates/notsai-transcribe/src/model.rs gained `Auto => "auto"`; test asserts it. At this time `model_languages(_model)` returned `&[En, Hi, Mr]` (not consumed by the frontend); SUPERSEDED by Save Point 2026-09-29 — `SmallEn` now returns `&[En]`, all other models `&[En, Hi, Mr]`.
- Auto algorithm in crates/notsai-transcribe/src/engine.rs (bounded to ~1 decode in the common case to preserve Task 2 speed):
  - Consts: `AUTO_CANDIDATE_LANGUAGES = &[En, Hi, Mr]` (~51), `AUTO_ACCEPT_NO_SPEECH = 0.5` (~55).
  - `rank_supported_languages(state, threads)` (~104): calls `state.lang_detect(0, threads)`, keeps only `AUTO_CANDIDATE_LANGUAGES`, sorts by probability desc. Called from `choose_auto_pass` (~221-222).
  - `pass_is_usable(pass)` (~205): `pass.real_chars > 0 && pass.mean_no_speech < AUTO_ACCEPT_NO_SPEECH`.
  - `choose_auto_pass(...)` (~210): ranks candidates; loops them best-first calling `decode_pass(..., Some(code), last_published.clone())`; returns the first usable pass. Remembers the first non-usable attempt as `fallback`; after the loop, if no candidate was usable, returns that fallback, else (ranked empty) decodes with `None`.
  - `run_whisper` chunk loop (~429): `state.pcm_to_mel(...)` once per chunk, then `let chosen = match language { SttLanguage::Auto => choose_auto_pass(...)?, _ => { ...existing forced/auto path... } }`.
  - `language_counts: HashMap<String, u64>` (~415) tallied per accepted chunk. Return (~571-572): `dominant_language = if matches!(language, SttLanguage::Auto) { language_counts.into_iter().max_by_key(...).map(...) } else { None }`.
  - `transcribe()` fallback (engine.rs ~350): stamps `language_code(self.language)` ONLY when `!matches!(self.language, SttLanguage::Auto)` and the result is `None` — so Auto never persists the literal `"auto"`.
- Frontend: packages/shared-types/src/index.ts `SttLanguage` is `"auto" | "en" | "hi" | "mr"` (~109); App.tsx `STT_LANGUAGE_LABELS` gained `auto: "Auto (detect & switch)"` (~48-53); dropdown is rendered from `Object.keys(STT_LANGUAGE_LABELS)` with value `settings?.stt_language ?? "en"`.
- **CRITICAL build fact re-confirmed:** apps/desktop resolves `@notsai/shared-types` / `@notsai/api-client` via `dist/` (`main`/`types` in each package.json) under `moduleResolution: "bundler"` with no source path mapping. After editing shared-types you MUST rebuild dist before `pnpm typecheck` reflects it:
  `pnpm --filter @notsai/shared-types build && pnpm --filter @notsai/api-client build`.
  This session `pnpm typecheck` initially failed (TS2353 `'auto' does not exist in type 'Record<SttLanguage, string>'`) purely from stale dist; it passed after the rebuild.
- Gates re-run this session and ALL GREEN: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (transcribe 29 tests, 0 failures); `pnpm lint`; `pnpm test` (vitest 6/6); `pnpm typecheck` (after dist rebuild).
- Notes: two `#[allow(clippy::too_many_arguments)]` attributes were added on `decode_pass`/`choose_auto_pass` (lint did not actually fire; harmless). Test-harness (ignored debug) single-line `.and_then(get_lang_str).map(str::to_string)` sites were formatted to satisfy fmt.
- User-verify step (not automatable): run `pnpm tauri dev`, set Settings -> Transcript language = "Auto (detect & switch)", record a code-switched meeting, transcribe, and confirm chunks switch language (vs. a single repeated English tail).

## Session Save Point (2026-09-21, later session — first save point) — two bug fixes: record-during-transcribe was blocked, transcription CPU spike
- Two regressions fixed this session; all gates re-run and green. No committed build/release (not a git repo).

### Fix 1 — Record no longer blocked while a previous meeting is still transcribing
- Symptom: pressing Record on another meeting while a transcription was running did nothing.
- Backend lock RULED OUT: `record_meeting` (crates/notsai-tauri/src/lib.rs:176) only locks the mic recorder slot (`AppState` 38-43); `transcribe_meeting` (lib.rs:297) never holds it — the backend was fine.
- Root cause: frontend global `busyId` gated the Record button, and `handleRecordMeeting` set it for the whole async span.
- Fix (frontend-only, apps/desktop/src/App.tsx): new dedicated `startingRecordingId` state (~81), set/cleared only inside `handleRecordMeeting` (~270-282); `handleRecordMeeting` no longer touches `busyId`. Record button disabled when `activeRecordingId != null || startingRecordingId != null` (~603) and labelled "Working…" while `startingRecordingId === meeting.id` (~607). Transcribe/generate-notes/settings/new-meeting/delete remain `busyId`-gated; `activeRecordingId` (from `onRecordingState` subscription, import ~20) and `handleDeleteMeeting` calling `deleteMeeting(id, deleteRaw: true)` unchanged.

### Fix 2 — transcription CPU spike (~100%, UX lag)
- Root cause: `WhisperEngine::new` (crates/notsai-transcribe/src/engine.rs:278-296) capped worker threads at `.clamp(1, 8)` (set in the earlier `later 8` session) — on this 16-logical-core Ryzen 7 4800H that left the system pegged during decode.
- Fix: `.clamp(1, 4)` (engine.rs:287) with doc comment "capped at four so whisper leaves CPU headroom for the rest of the app." Decode still runs inside `spawn_blocking` (UI thread never blocked).

### Validation
- Full gates re-run, ALL GREEN: `pnpm lint`; `pnpm typecheck`; `pnpm test` (vitest 6/6); `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (29 transcribe tests, 0 failures, ~36s incl. real-model smoke test). `pnpm --filter @notsai/desktop build` sanity pass (32 modules, 156.89 kB JS / 4.94 kB CSS).
- No API/event contract change; no shared-types/api-client edit, so no dist rebuild required this session.
- **Correction:** `Session Save Point (2026-09-21, later 8)` Increment B text said threads clamped to `(1, 8)`; the CURRENT state is `clamp(1, 4)` — this save point supersedes that statement.
- User-verify step (not automatable, pending): `pnpm tauri dev` — (a) start a recording while a live transcription is in progress (must succeed), and (b) observe CPU during transcription (should stay well under 100%).

## Session Save Point (2026-09-22) — background-noise removal (Part A: ffmpeg denoise, Part B: whisper no-speech suppression)
- Design: user chose "Both (Recommended)" — static ffmpeg denoise at capture/transcode + whisper no-speech-token suppression on the decode path. Delivery deliberately kept the ffmpeg chain light (non-adaptive, cannot damage speech). All gates re-run and green; no committed build/release (not a git repo).

### Part A — ffmpeg denoise during WAV→m4a transcode
- ffmpeg n9.0.1; filters `acompressor`, `afftdn`, `anlmdn`, `highpass`, `lowpass`, `speechnorm` verified available.
- NEW `ffmpeg::transcode_to_m4a_denoised(input, output)` in crates/notsai-audio/src/ffmpeg.rs:64, placed right after the generic `transcode_to_m4a` (line 57) which is UNCHANGED (still used by its own tests). Reuses `run_ffmpeg` (82) with args `-af highpass=f=100,afftdn=nf=-20:nr=10`, `-c:a aac`, `-b:a 128k`. 100 Hz high-pass removes rumble/hum; conservative `afftdn` suppresses stationary noise without damaging speech.
- Wiring: `stop_recording` in crates/notsai-tauri/src/lib.rs:260 now calls `ffmpeg::transcode_to_m4a_denoised(&temp_wav, &recording_path)` (was `transcode_to_m4a`).
- Test: `ffmpeg::tests::transcode_m4a_denoised_produces_file` (ffmpeg.rs:175) — skips if `!ffmpeg_available()`, temp dir, 1600-sample silence written via `wav::write_s16le`, asserts output file length > 0.

### Part B — whisper no-speech-token suppression in decode
- whisper-rs pinned at **0.16.0** (workspace Cargo.toml line 37; crates/notsai-transcribe/Cargo.toml line 20). Verified API surface in `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/whisper-rs-0.16.0/src/whisper_params.rs`: whisper-rs has `set_suppress_blank`, `set_suppress_nst`, `set_no_speech_thold`, but NO `set_suppress_non_speech_tokens`/`set_no_speech_threshold` — so suppression is implemented manually by segment filtering.
- NEW const `NO_SPEECH_DROP_THRESHOLD: f32 = 0.6` in crates/notsai-transcribe/src/engine.rs:61 (after `AUTO_ACCEPT_NO_SPEECH` 0.5, line 55).
- `decode_pass` segment loop (~178-204): skips a segment when `no_speech > NO_SPEECH_DROP_THRESHOLD || text.trim().is_empty()` BEFORE accumulating into `no_speech_sum`/`real_chars`/`segments`. `no_speech_sum` (190) only aggregates kept segments; `mean_no_speech` unchanged (still 1.0 when empty). This cleans both the published text and the Auto-path `mean_no_speech` statistic that drives pass selection.
- No API/event contract change; no shared-types/api-client edit, so no dist rebuild required.

### Validation
- Full gates re-run, ALL GREEN: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (transcribe 29, audio 10 incl. new `transcode_m4a_denoised_produces_file`, plus 11/20/18, 0 failures); `pnpm lint`; `pnpm typecheck`; `pnpm test` (vitest 6/6). Full cargo test log at `/home/pluto/.local/share/opencode/tool-output/tool_0c944b3e1001MmG1a25Xe1ZpM9`.
- User-verify step (not automatable, pending): `pnpm tauri dev` — record a meeting with background noise (e.g. fan/room tone), transcribe, and confirm (a) output transcript has no silence/chatty blank segments and (b) recording audio is noticeably denoised vs before. Re-confirm Auto language detection (Increment C) at the same time.

## Session Save Point (2026-09-22, later session) — Hindi transcription accuracy (beam search) + transcription wall-time fix (skip redundant decode pass)
- Two issues reported by the user on a 6-minute Hindi call: (1) Hindi transcription was inaccurate, (2) transcription took 10–15 min wall time. Fix target: switch forced-language decode to beam search for accuracy AND gate the redundant second "runner-up" decode pass so wall time drops. All gates re-run and green; no committed build/release (not a git repo).

### Fix 1 — Hindi accuracy: beam search on the decode path
- `decode_pass` params changed to `SamplingStrategy::BeamSearch { beam_size: 5, patience: -1.0 }` for ALL passes (engine.rs:142). Previously greedy. This applies to every language path (forced + auto), so Hindi/Sanskrit script decode now gets beam search without a language-specific fork. Temperature fallback chain (0.00 → 0.40 → 0.60) is unchanged; on the espeak bench audio the beam hits a temperature fallback and emits the normal decoder warning logs (not failures).

### Fix 2 — wall time: force-language path skips the redundant runner-up pass
- The ORIGINAL algorithm decoded `first` (prime candidate) then, in the forced `Hi`/`Mr`/`En` branch, decided on extra passes using the old `Auto`-style heuristic. Root cause of the 10–15 min wall time: the forced-language path (engine.rs:502-518) still ran extra auto runner-up decodes even when the forced fork fixed the language.
- Fix: the `Some(ref forced)` branch is now a **no-op when `pass_is_usable(&candidates[0])`** (engine.rs:504 → skip the whole body). Extra decodes only run when the first (forced-language) pass is NOT usable: auto-lang extra if `auto_lang != forced`, then runner-up if still no usable pass. Both `run_extra` closures already bail when the code equals the forced code.
- `pass_is_usable` (engine.rs:215) unchanged: `real_chars > 0 && mean_no_speech < AUTO_ACCEPT_NO_SPEECH(0.5)`.
- Result: forced-language transcription is now ~1 decode per chunk (was up to 2-3).

### Bench evidence (post-fix, NOTSAI_BENCH_LANG=hi)
- 71 s Hindi WAV (`/tmp/hi_bench.wav`), 4 threads, beam: `bench_run_whisper: lang=Hi threads=4 audio=71s elapsed=65.75s segments=11 chars=339`. Log shows ONLY `_LANG_hi` decodes per chunk — the auto/runner-up pass gate worked (forced pass was usable every chunk).
- Projection: the reported wall time was for a ~6-min (360 s) call. At ~21.9 s decode per 71 s of audio (≈0.31× realtime), 360 s ≈ 112 s ≈ ~2 min — far under the reported 10–15 min. NOTE: model-file load and per-meeting overhead excluded from bench `elapsed` (bench excludes `cached_path`/`verify_model`, both run before `Instant::now()`; whisper init inside `run_whisper` IS included).
- Caveat: TRUE pre-fix baseline was never measured; treat the user-reported 10–15 min as the baseline. If a differential number is later needed, revert-baseline run is possible but was explicitly skipped this session.
- Accuracy (qualitative, from user's original report) is addressed by beam search; bench digits are not transcript-accuracy-graded. If Hindi accuracy is STILL insufficient after this fix, next lever is model size: current `settings.json` has `whisper_model: "small"` (cached `/tmp/notsai-verify/ggml-small.bin`, 487.6 MB). Recommend `"medium"` (~1.5 GB, needs re-verify/redownload) for materially better Devanagari accuracy at higher CPU cost.

### Clippy / hygiene
- Clippy fix applied: engine.rs:510 `candidates.iter().any(|c| pass_is_usable(c))` → `candidates.iter().any(pass_is_usable)`.
- No API/event contract change; no shared-types/api-client edit, so no dist rebuild required.

### Validation
- Full gates re-run, ALL GREEN: `cargo fmt --all`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (30 passed, 0 failed — transcribe 29 + audio 10 + 11/20/18; cargo test log at `/home/pluto/.local/share/opencode/tool-output/tool_0c9979d7a001UDIKHfZFDjxNyg`, result ok line 4950; bench run `tool_0c993c06e001v0dLTe47whoaiA`: 1 passed/29 filtered/66.04 s; prior 28-pass log `tool_0c37237660011Dh6SnmKADXKe7`). `pnpm lint`; `pnpm typecheck`; `pnpm test` (vitest 6/6).
- Bench env contract: `NOTSAI_BENCH_AUDIO` (required), `NOTSAI_BENCH_LANG` (`en|hi|mr|auto` → SttLanguage, default `En`), `NOTSAI_BENCH_THREADS` (default 4); model via `NOTSAI_MODELS_DIR`/`NOTSAI_VERIFY_MODEL`. `NOTSAI_BENCH_AUDIO` unset during `cargo test` → bench prints skip and passes (counted in the 30).

## Session Save Point (2026-09-27) — Save Notes / fill export command
- New Tauri command `export_notes(meeting_id)` in `crates/notsai-tauri/src/lib.rs` (fn at :178, registered in `invoke_handler` at :683 between `list_notes,` and `record_meeting,`). Exports the meeting's notes as a Markdown file and best-effort reveals it in the file manager. Dialog plugin deliberately NOT added.
- Behavior (verified against source):
  - Meeting must exist (`get` → NotFound via `CoreError::not_found`) and must have ≥1 note (`NoteRepository::list` empty → InvalidInput `"meeting has no notes to export"`). It does NOT fall back to exporting raw transcript.
  - Export dir: `app.path().download_dir()` when it resolves, else `app_data_dir()/exports` (dir-resolve failure → Internal). `fs::create_dir_all` + `fs::write` failures → `CommandError::Storage`.
  - Filename `notes-<meeting_id>-<YYYYMMDD-HHMMSS>.md` (UTC). Content: `# <title>` + `Exported: <rfc3339>` + one `## <kind>` section per note (kind rendered via `Debug`).
  - Reveal is best-effort and non-blocking: `use tauri_plugin_opener::OpenerExt;` then `app.opener().reveal_item_in_dir(&path)` (method is on `Opener<R>`, NOT the AppHandle — this was the E0599 + unused-import-pattern fix); failures only `tracing::warn!`.
  - Capabilities `crates/notsai-tauri/capabilities/default.json` unchanged (`core:default` + `opener:default`); NO `opener:allow-reveal-item-in-dir` needed — clippy/test/typecheck all passed without it.
- Frontend + api-client: `packages/api-client/src` added `exportNotes(meetingId: string): Promise<string>` → `invoke<string>("export_notes", { meetingId })` and **dist rebuilt** (`pnpm --filter @notsai/api-client build`) — source already exported it; stale dist caused apps/desktop TS2305 during typecheck. `apps/desktop/src/App.tsx` wired: `exportNotes` import (line 15), `exportPath` state, `handleSaveNotes`, "Save notes" button (enabled when `meeting.status === "ready"`, label "Working…" while busy), "Notes saved" card showing the exported path.
- Reconfirmed build fact (same as shared-types): apps/desktop resolves `@notsai/api-client` via `dist/` (`main`/`types` in package.json); after editing `packages/api-client/src` you MUST rebuild its dist before `pnpm typecheck` reflects it.
- Known non-blocking nuance (left as-is, user didn't ask): `exportPath` is not reset on a later failed export, so a stale "Notes saved" card can sit next to a new error message.

### Validation
- Gates re-run, ALL GREEN: `cargo fmt --all`; `cargo clippy -- -D warnings` (bare `-D` without `--` is rejected as an unexpected argument but still compiles as a check — always use `-- -D warnings`); `cargo test` in `crates/notsai-tauri` (0 tests). `pnpm lint` (apps/desktop clean); `pnpm typecheck` at root green for shared-types/api-client/apps/desktop (TS2305 gone after dist rebuild).
- Verification artifacts: `packages/api-client/dist/index.d.ts:15` exports `exportNotes`; `dist/index.js:37` defines it.
- Run the app with `pnpm tauri dev` (not a git repo — no commits).
- User-verify step (not automatable): `pnpm tauri dev` → open a meeting with notes at status Ready → click "Save notes" → confirm the `.md` lands in OS Downloads (or app-data `exports` fallback) with the right filename/content, the "Notes saved" card shows the path, and the file manager reveals it (best-effort).

## Session Save Point (2026-09-29) — English transcription accuracy: explicit-En → ggml-small.en (SmallEn) + forced-language decode
- User asked to improve English transcription accuracy ("not 100 percent accurate in english"), approved "yes then implement it"; medium opt-in skipped; En optimizations apply ONLY when the user explicitly selects English — `hi`/`mr`/`auto` paths untouched; no UI/frontend/settings changes; LargeV3 unaffected (whisper.cpp ships no `.en` large model). Not a git repo.
- Mapping: `crates/notsai-transcribe/src/engine.rs:304` `(SttLanguage::En, WhisperModel::Small) => WhisperModel::SmallEn` inside `WhisperEngine::new`. This is the ONLY code path that selects SmallEn (call site `crates/notsai-tauri/src/lib.rs:379`). The bench calls `verify_model()` + `run_whisper` directly and bypasses `WhisperEngine::new`, so Part B is covered by unit tests only.
- Forced-language decode: explicit-En path passes `Some("en")` + `suppress_nst(true)`; `decode_pass` (`engine.rs:130`) applies `set_language` (:153), `set_suppress_nst` (:154), `set_suppress_blank(true)` (:152), beam 5 / patience -1.0, translate/print_* false.
- `model.rs`: `model_languages(SmallEn)` returns `&[SttLanguage::En]` (all other models `&[En, Hi, Mr]`); SmallEn = `"ggml-small.en.bin"`, size `487_614_201`; unit tests `small_en_supports_only_english` and `sizes_grow_with_model_quality` (asserts small_en < small).
- Bench evidence (`espeak-ng` synthetic 4-sentence English, `/tmp/en_bench.wav`, 648,344 B @ 22050 Hz; `NOTSAI_BENCH_AUDIO=... NOTSAI_BENCH_LANG=... NOTSAI_VERIFY_MODEL=... cargo test -p notsai-transcribe bench_run_whisper -- --nocapture`):
  - (a) `small` + `en`: `elapsed=19.210388195s segments=4 chars=213`
  - (b) `base` + `en`: `elapsed=6.041013165s`
  - (c) `base` + `auto`: `elapsed=6.164560766s`
  - All three produced byte-identical 213-char transcripts. Seg 2 mishearing "hold"→"fold" was identical across ALL configs — a whisper model characteristic on that synthetic audio, not a path/language artifact.
- Caveats: synthetic espeak-ng audio is too clean to discriminate base vs small; no natural English recording available; the ~487 MB `ggml-small.en.bin` is downloaded on first use (download-on-first-use path exists but has never run; `models_dir()` = `NOTSAI_MODELS_DIR` or `~/.local/share/ai.nots.notsai/models`, currently only ggml-base + ggml-small).
- Validation: gates were green at implementation time (prior session); this session was docs-only (context.md edit fixing the stale Increment C note — "At this time `model_languages(_model)` returned `&[En, Hi, Mr]` (not consumed by the frontend)" is SUPERSEDED by this save point: `SmallEn` now returns `&[En]`).
- User-verify step (not automatable): `pnpm tauri dev` → select English → transcribe a natural English recording → (a) accuracy acceptable, (b) first use downloads small.en (487 MB) then subsequent runs are fast. Pending user decision: trigger that download now for an end-to-end SmallEn check, or accept unit-test coverage + download-on-first-use.

## Next (open) workstream
- User-verify Save Notes (2026-09-27): `pnpm tauri dev` → open a Ready meeting with notes → "Save notes" → confirm `.md` file, "Notes saved" card, reveal (details in the save point above). Follow-up options if the user wants more: add a "Save transcript" variant, per-meeting export history, or a folder picker.
- Hindi accuracy (beam search) + forced-language wall-time fix (skip redundant pass) implemented, bench 65.75 s for 71 s audio, gates green, context recorded. Awaiting user-verify: transcribe a real ~6-min Hindi recording and confirm both (a) transcript accuracy is acceptable and (b) wall time is well under 10 min; if accuracy still lacking, bump `whisper_model` small → medium.
- English accuracy (2026-09-29): explicit-En → `ggml-small.en` (SmallEn) + forced-`en` decode implemented; bench-verified for Part A (see save point above). OPEN: user decision on triggering the ~487 MB small.en download for an end-to-end SmallEn check vs. accepting unit-test coverage + download-on-first-use; then user-verify with a natural English recording (`pnpm tauri dev` → English).
