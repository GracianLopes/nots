# Privacy & Security

## Guarantees

1. **Raw-before-derived.** Recordings and transcripts are raw source data and
   are never deleted before successful processing — unless the user explicitly
   configures otherwise (retention modes).
2. **No secrets in logs.** Provider API keys, access tokens, raw audio, and
   transcripts are never written to logs.
3. **Consent gating.** The microphone is never activated and no cloud AI call
   is ever made without explicit, visible user consent.
4. **Local-first.** By default transcription and note generation run fully
   on-device. Cloud processing is opt-in.
5. **Loopback only.** Any future network server binds to localhost only, never
   to a public interface.

## Data location

- Meeting records/transcripts/models: application data directory
  (`<app_data>/meetings/...`, `<app_data>/models/...`).
- Settings (incl. provider keys): stored via the OS-secure Tauri store policy.

## Retention modes

- `FULL_RECORDING` — keep raw recording + transcript + notes.
- `SMART_NOTES` — keep transcript + notes, discard raw recording.
- `NOTES_ONLY` — keep derived notes only.