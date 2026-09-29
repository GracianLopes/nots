# notsAI

AI meeting assistant that **understands the meeting, not merely transcribes it**.

notsAI joins supported meetings, captures audio, understands shared screens, and
produces structured, hallucination-free AI notes: decisions, action items,
questions, and topics — fast and private.

## Status

Phase 1 (Core MVP) — desktop app in active development.

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for build/run instructions and
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for design.

## Principles

1. **Understand, don't merely transcribe.** Recordings/transcripts are *raw*
   data. AI notes are *derived* data. Raw data is never deleted before
   successful processing unless the user explicitly configures deletion.
2. **Privacy first.** Everything stays on-device by default. Cloud AI is
   opt-in, gated by consent, and respects a strict privacy hierarchy.
3. **No hallucinations.** Only what is actually in the meeting goes into notes;
   unknown owners/deadlines are marked "not specified".
4. **MVP discipline.** Fast, correct, local. No over-engineering (no vector DB,
   no microservices, no Docker in the MVP).

## License

MIT