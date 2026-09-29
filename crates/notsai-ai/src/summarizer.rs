//! Incremental meeting summarization behind a rolling context window.
//!
//! [`Summarizer`] converts a transcript into structured [`NoteKind`] notes.
//! Instead of handing the whole transcript to a provider in one request, the
//! transcript is split into character-bounded windows. Each window beyond the
//! first is prefixed by a running plain-text summary of everything that came
//! before, so only the rolling summary plus the current window is ever sent.
//! The final pass asks for a JSON array of notes, which is parsed leniently.

use notsai_core::{
    AIProvider, AiChatMessage, AiChatRequest, AiChatResponse, AiRole, CoreError, NoteKind,
    ProviderKind, TranscriptSegment,
};

/// Default maximum characters handed to the provider in a single request.
const WINDOW_CHARS: usize = 12_000;

const ROLLUP_SYSTEM: &str = "\
You are summarizing a meeting transcript one window at a time.
You receive a portion of the transcript and, when present, a previous summary.
Produce a concise plain-text rolling summary of everything covered so far.
Keep decisions, action items and open questions; drop small talk.
Output plain prose only - no json, no markdown fences, no bullet symbols.";

const NOTES_SYSTEM: &str = "\
You turn the given rolling meeting summary into structured notes.
Return ONLY a JSON array, no prose and no markdown fences.
Each element is an object with exactly two fields:
\"kind\": one of \"summary\", \"topic\", \"decision\", \"action_item\", \"question\"
\"content\": a short factual string.
Rules:
- include exactly one element with kind \"summary\"
- \"topic\" for subjects discussed
- \"decision\" for decisions reached
- \"action_item\" for assigned tasks, naming the owner when known
- \"question\" for open questions
- skip small talk";

const ROLLUP_MAX_TOKENS: u32 = 1024;
const NOTES_MAX_TOKENS: u32 = 2048;

/// Summarizes a transcript via a rolling context window.
pub struct Summarizer {
    provider: Box<dyn AIProvider>,
    model: String,
    window_chars: usize,
}

impl Summarizer {
    /// Build a summarizer over `provider` with the default window size.
    pub fn new(provider: Box<dyn AIProvider>, model: impl Into<String>) -> Self {
        Self::with_window(provider, model, WINDOW_CHARS)
    }

    /// Build a summarizer with an explicit context window (used by tests).
    pub fn with_window(
        provider: Box<dyn AIProvider>,
        model: impl Into<String>,
        window_chars: usize,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            window_chars,
        }
    }

    /// The provider backend this summarizer talks to.
    pub fn provider_kind(&self) -> ProviderKind {
        self.provider.kind()
    }

    /// Summarize a full transcript into structured notes.
    pub async fn summarize(
        &self,
        segments: &[TranscriptSegment],
    ) -> Result<Vec<(NoteKind, String)>, CoreError> {
        if segments.is_empty() {
            return Err(CoreError::invalid_input(
                "no transcript segments to summarize",
            ));
        }

        let transcript = join_transcript(segments);
        let mut rolling: Option<String> = None;
        for window in chunk_text(&transcript, self.window_chars) {
            rolling = Some(self.roll_up(&window, rolling.as_deref()).await?);
        }

        let rolling = rolling.expect("a non-empty transcript produces at least one window");
        let notes = self.extract_notes(&rolling).await?;
        parse_notes(&notes)
    }

    /// Fold one transcript window into the running summary.
    async fn roll_up(&self, window: &str, previous: Option<&str>) -> Result<String, CoreError> {
        let mut messages = vec![AiChatMessage {
            role: AiRole::System,
            content: ROLLUP_SYSTEM.to_string(),
        }];
        if let Some(previous) = previous {
            messages.push(AiChatMessage {
                role: AiRole::Assistant,
                content: previous.to_string(),
            });
        }
        messages.push(AiChatMessage {
            role: AiRole::User,
            content: window.to_string(),
        });

        let response = self.chat(messages, Some(ROLLUP_MAX_TOKENS)).await?;
        let content = response.content.trim().to_string();
        if content.is_empty() {
            return Err(CoreError::ai_request(
                "summarizer received an empty rolling summary from the provider",
            ));
        }
        Ok(content)
    }

    /// Turn the final rolling summary into a raw JSON notes document.
    async fn extract_notes(&self, rolling: &str) -> Result<String, CoreError> {
        let messages = vec![
            AiChatMessage {
                role: AiRole::System,
                content: NOTES_SYSTEM.to_string(),
            },
            AiChatMessage {
                role: AiRole::User,
                content: format!("Transcript:\n\n{rolling}"),
            },
        ];
        let response = self.chat(messages, Some(NOTES_MAX_TOKENS)).await?;
        Ok(strip_fences(&response.content))
    }

    async fn chat(
        &self,
        messages: Vec<AiChatMessage>,
        max_tokens: Option<u32>,
    ) -> Result<AiChatResponse, CoreError> {
        self.provider
            .chat(AiChatRequest {
                model: self.model.clone(),
                messages,
                max_tokens,
            })
            .await
    }
}

/// Serialize transcript segments into `[speaker]: text` lines.
fn join_transcript(segments: &[TranscriptSegment]) -> String {
    let mut out = String::new();
    for segment in segments {
        if let Some(speaker) = &segment.speaker {
            out.push_str(&format!("[{speaker}]: {}\n", segment.text));
        } else {
            out.push_str(&segment.text);
            out.push('\n');
        }
    }
    out
}

/// Split `text` into windows of at most `window_chars` characters.
///
/// Lines are kept whole whenever they fit; a single line longer than the
/// window is hard-split at UTF-8 character boundaries so the slices always
/// contain complete characters.
fn chunk_text(text: &str, window_chars: usize) -> Vec<String> {
    if window_chars == 0 {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;

    for line in text.lines() {
        let line_len = line.chars().count();
        if line_len > window_chars {
            if current_len > 0 {
                chunks.push(std::mem::take(&mut current));
                current_len = 0;
            }
            append_char_chunks(line, window_chars, &mut chunks);
            continue;
        }

        let separator = usize::from(current_len > 0);
        if current_len + separator + line_len > window_chars {
            chunks.push(std::mem::take(&mut current));
            current_len = 0;
        }
        if current_len > 0 {
            current.push('\n');
            current_len += 1;
        }
        current.push_str(line);
        current_len += line_len;
    }

    if current_len > 0 {
        chunks.push(current);
    }
    chunks
}

/// Split an over-long line into `window_chars`-bounded character chunks.
fn append_char_chunks(line: &str, window_chars: usize, out: &mut Vec<String>) {
    if window_chars == 0 {
        out.push(line.to_string());
        return;
    }
    let mut remaining = line;
    while !remaining.is_empty() {
        let boundary = char_boundary_at(remaining, window_chars);
        debug_assert!(boundary > 0, "a positive window always advances");
        let (chunk, rest) = remaining.split_at(boundary);
        out.push(chunk.to_string());
        remaining = rest;
    }
}

/// Byte index of the char boundary `count` chars into `text` (or `text.len()`).
fn char_boundary_at(text: &str, count: usize) -> usize {
    text.char_indices()
        .nth(count)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

/// Remove a leading/trailing triple-backtick fence when the model adds one.
fn strip_fences(raw: &str) -> String {
    let mut lines: Vec<&str> = raw.lines().collect();
    if lines
        .first()
        .is_some_and(|line| line.trim().starts_with("```"))
    {
        lines.remove(0);
    }
    if lines
        .last()
        .is_some_and(|line| line.trim().starts_with("```"))
    {
        lines.pop();
    }
    lines.join("\n")
}

/// Extract the outermost JSON array from `text`.
///
/// The scan understands JSON strings (so `]` and `[` inside strings are
/// ignored) and nested arrays. Returns `None` when no balanced `[...]` pair is
/// found.
fn extract_json_array(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut array_start: Option<usize> = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, &byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' => {
                if array_start.is_none() {
                    array_start = Some(index);
                }
                depth += 1;
            }
            b']' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let start = array_start?;
                    return Some(&text[start..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse a provider JSON array into usable notes, dropping malformed items.
fn parse_notes(raw: &str) -> Result<Vec<(NoteKind, String)>, CoreError> {
    let array = extract_json_array(raw).ok_or_else(|| {
        CoreError::ai_request("provider response did not contain a JSON array of notes")
    })?;

    let value: serde_json::Value = serde_json::from_str(array)
        .map_err(|err| CoreError::ai_request(format!("notes JSON parse failed: {err}")))?;
    let items = value
        .as_array()
        .ok_or_else(|| CoreError::ai_request("notes JSON value was not an array"))?;

    let mut notes = Vec::new();
    for item in items {
        let Some(kind) = item
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_note_kind)
        else {
            continue;
        };
        let Some(content) = item.get("content").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let content = content.trim();
        if content.is_empty() {
            continue;
        }
        notes.push((kind, content.to_string()));
    }

    if notes.is_empty() {
        return Err(CoreError::ai_request("provider returned no usable notes"));
    }
    Ok(notes)
}

/// Parse a JSON string into a [`NoteKind`]; `None` for unknown/other kinds.
fn parse_note_kind(raw: &str) -> Option<NoteKind> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct FakeProvider {
        requests: Arc<Mutex<Vec<AiChatRequest>>>,
        responses: Arc<Mutex<VecDeque<String>>>,
    }

    impl FakeProvider {
        fn new(responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                responses: Arc::new(Mutex::new(responses.into_iter().map(Into::into).collect())),
            }
        }

        fn chat_requests(&self) -> Vec<AiChatRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl AIProvider for FakeProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Local
        }

        async fn chat(&self, request: AiChatRequest) -> Result<AiChatResponse, CoreError> {
            self.requests.lock().unwrap().push(request.clone());
            let content = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default();
            Ok(AiChatResponse { content })
        }
    }

    fn segment(speaker: Option<&str>, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            seq: 0,
            start: 0.0,
            end: 0.0,
            speaker: speaker.map(ToOwned::to_owned),
            text: text.to_string(),
            confidence: None,
        }
    }

    const NOTES_JSON: &str = r#"[{"kind":"summary","content":"Team aligned"},
{"kind":"action_item","content":"Ship v2 by Friday"},
{"kind":"bogus","content":"dropped"}]"#;

    #[test]
    fn join_transcript_prepends_speaker_when_present() {
        let joined = join_transcript(&[segment(Some("Alice"), "First."), segment(None, "Second.")]);
        assert_eq!(joined, "[Alice]: First.\nSecond.\n");
    }

    #[test]
    fn chunk_text_keeps_lines_whole_within_window() {
        let chunks = chunk_text("aaaa\nbbbb\ncccc\ndddd", 10);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "aaaa\nbbbb");
        assert_eq!(chunks[1], "cccc\ndddd");
    }

    #[test]
    fn chunk_text_splits_long_line_at_char_boundaries() {
        let chunks = chunk_text("a😀bcdef", 2);
        assert_eq!(chunks, vec!["a😀", "bc", "de", "f"]);
    }

    #[test]
    fn char_boundary_at_respects_multibyte_chars() {
        assert_eq!(char_boundary_at("a😀b", 0), 0);
        assert_eq!(char_boundary_at("a😀b", 2), 5);
        assert_eq!(char_boundary_at("a😀b", 100), 6);
    }

    #[test]
    fn extract_json_array_handles_strings_and_nesting() {
        assert_eq!(
            extract_json_array(r#"prefix[1,[2,3]]suffix"#),
            Some("[1,[2,3]]")
        );
        assert_eq!(
            extract_json_array(r#"[{"kind":"x}]"},{"kind":"y"}]"#),
            Some(r#"[{"kind":"x}]"},{"kind":"y"}]"#)
        );
        assert_eq!(extract_json_array("[]"), Some("[]"));
        assert_eq!(extract_json_array("no array here"), None);
    }

    #[test]
    fn strip_fences_removes_fences_with_language_tag() {
        assert_eq!(strip_fences("```json\n[1]\n```"), "[1]");
        assert_eq!(strip_fences("```\n[1]\n```"), "[1]");
        assert_eq!(strip_fences("[1]"), "[1]");
    }

    #[test]
    fn parse_notes_accepts_valid_items_and_drops_bad_ones() {
        let notes = parse_notes(NOTES_JSON).unwrap();
        assert_eq!(
            notes,
            vec![
                (NoteKind::Summary, "Team aligned".to_string()),
                (NoteKind::ActionItem, "Ship v2 by Friday".to_string()),
            ]
        );
    }

    #[test]
    fn parse_notes_errors_without_array() {
        assert!(parse_notes("plain text, no json").is_err());
        assert!(parse_notes("[]").is_err());
        assert!(parse_notes("[not json]").is_err());
    }

    #[test]
    fn parse_note_kind_round_trips_snake_case() {
        assert_eq!(parse_note_kind("action_item"), Some(NoteKind::ActionItem));
        assert_eq!(parse_note_kind("summary"), Some(NoteKind::Summary));
        assert_eq!(parse_note_kind("bogus"), None);
    }

    #[test]
    fn summarize_rejects_empty_transcript() {
        let provider = FakeProvider::new(Vec::<String>::new());
        let summarizer = Summarizer::new(Box::new(provider), "test-model");
        let result = futures_block_on(summarizer.summarize(&[]));
        assert!(matches!(result, Err(CoreError::InvalidInput(_))));
    }

    #[test]
    fn summarize_rolls_summary_between_windows_and_returns_notes() {
        let fake = FakeProvider::new(vec![
            "rolling-one".to_string(),
            "rolling-two".to_string(),
            NOTES_JSON.to_string(),
        ]);
        let summarizer = Summarizer::with_window(Box::new(fake.clone()), "test-model", 200);

        let long_line = "discussion ".repeat(12); // 144 chars
        let segments = vec![
            segment(Some("Alice"), &long_line),
            segment(Some("Bob"), &long_line),
        ];

        let notes = futures_block_on(summarizer.summarize(&segments)).unwrap();

        assert_eq!(notes.len(), 2);
        assert!(notes
            .iter()
            .all(|(kind, _)| kind == &NoteKind::Summary || kind == &NoteKind::ActionItem));
        assert!(notes.contains(&(NoteKind::Summary, "Team aligned".to_string())));

        let requests = fake.chat_requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].max_tokens, Some(1024));
        let roles: Vec<AiRole> = requests[0]
            .messages
            .iter()
            .map(|message| message.role)
            .collect();
        assert_eq!(roles, vec![AiRole::System, AiRole::User]);
        let roles: Vec<AiRole> = requests[1]
            .messages
            .iter()
            .map(|message| message.role)
            .collect();
        assert_eq!(roles, vec![AiRole::System, AiRole::Assistant, AiRole::User]);
        assert_eq!(requests[1].messages[1].content, "rolling-one");
        assert_eq!(requests[2].max_tokens, Some(2048));
    }

    fn futures_block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }
}
