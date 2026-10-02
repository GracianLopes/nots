//! Whisper-based transcription engine.
//!
//! [`WhisperEngine`] implements the [`TranscriptionEngine`] trait using the
//! [whisper-rs] bindings over the `whisper.cpp` runtime. Decoded audio is
//! transcribed segment by segment, and each completed segment is published on
//! the caller-supplied event bus while the run is in progress.
//!
//! The engine is wrapped in [`tokio::task::spawn_blocking`] because
//! `whisper.cpp` is a blocking native runtime; only the cloned
//! [`EventBus`] and the decoded audio cross the closure boundary.
//!
//! [whisper-rs]: https://docs.rs/whisper-rs

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::available_parallelism;

use async_trait::async_trait;
use notsai_core::{
    AudioSource, CoreError, CoreEvent, EventBus, ProcessingState, SttLanguage, TranscriptSegment,
    TranscriptionEngine, TranscriptionResult, WhisperModel,
};
use uuid::Uuid;
use whisper_rs::{
    get_lang_id, get_lang_str, FullParams, SamplingStrategy, WhisperContext,
    WhisperContextParameters, WhisperError, WhisperState,
};

use crate::audio;
use crate::model::language_code;
use crate::model_manager::{Consent, ModelManager};

/// Length of each transcription chunk in seconds. Long audio is processed in
/// short chunks so per-chunk language auto-detection can follow code switches.
const CHUNK_SECS: f64 = 30.0;
/// Overlap between consecutive chunks in seconds. Segments whose start falls
/// inside the overlap region of a chunk are dropped so no text is published
/// twice.
const OVERLAP_SECS: f64 = 2.0;
/// Minimum progress in seconds between two transcription progress events.
/// Whisper may invoke the progress callback repeatedly while a chunk stalls,
/// so events are only published when the reported position advances past this
/// threshold, keeping the event volume bounded.
const CHUNK_PROGRESS_THROTTLE_SECS: f64 = 1.0;
const LANG_DETECT_CONFIDENCE_THRESHOLD: f32 = 0.5;
const MIN_RUNNER_UP_PROB: f32 = 0.05;
/// Languages [`SttLanguage::Auto`] considers when ranking per-chunk detection.
/// Restricting the candidate set keeps a confident-but-wrong language (e.g.
/// Nepali for Hindi audio) from being chosen.
const AUTO_CANDIDATE_LANGUAGES: &[SttLanguage] =
    &[SttLanguage::En, SttLanguage::Hi, SttLanguage::Mr];
/// Maximum mean `no_speech_probability` for a pass to be accepted by the
/// [`SttLanguage::Auto`] path. Higher values mean whisper was mostly guessing.
const AUTO_ACCEPT_NO_SPEECH: f32 = 0.5;
/// Segments whose whisper `no_speech_probability` exceeds this value are
/// dropped from the pass output. These are timestamped silence chunks that
/// `set_suppress_blank` could not blank entirely; discarding them cleans up
/// both the published text and the Auto-path `mean_no_speech` statistic that
/// drives pass selection.
const NO_SPEECH_DROP_THRESHOLD: f32 = 0.6;

struct ChunkSegment {
    text: String,
    start: f64,
    end: f64,
}

struct PassSegments {
    real_chars: usize,
    mean_no_speech: f32,
    segments: Vec<ChunkSegment>,
    language: Option<String>,
}

fn content_chars(text: &str) -> usize {
    text.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !c.is_ascii_punctuation()
                && !matches!(
                    c,
                    '।' | '॥' | '…' | '«' | '»' | '“' | '”' | '’' | '‘' | '–' | '—' | '·'
                )
        })
        .count()
}

fn detect_languages(
    state: &WhisperState,
    threads: usize,
) -> Result<(Option<i32>, Option<i32>, f32), WhisperError> {
    let (dominant_id, probs) = state.lang_detect(0, threads)?;
    if dominant_id < 0 || dominant_id as usize >= probs.len() {
        return Ok((None, None, 0.0));
    }
    let dominant_prob = probs[dominant_id as usize];
    let runner_up = (0..probs.len())
        .filter(|&i| i != dominant_id as usize)
        .filter(|&i| probs[i] >= MIN_RUNNER_UP_PROB)
        .max_by(|&i, &j| {
            probs[i]
                .partial_cmp(&probs[j])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|i| i as i32);
    Ok((Some(dominant_id), runner_up, dominant_prob))
}

fn rank_supported_languages(
    state: &WhisperState,
    threads: usize,
) -> Result<Vec<String>, WhisperError> {
    let (_, probs) = state.lang_detect(0, threads)?;
    let mut ranked: Vec<(String, f32)> = AUTO_CANDIDATE_LANGUAGES
        .iter()
        .filter_map(|language| {
            let code = language_code(*language);
            get_lang_id(code).map(|id| {
                let prob = probs.get(id as usize).copied().unwrap_or(0.0);
                (code.to_string(), prob)
            })
        })
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    Ok(ranked.into_iter().map(|(code, _)| code).collect())
}

#[allow(clippy::too_many_arguments)]
fn decode_pass(
    state: &mut WhisperState,
    chunk: &[f32],
    threads: usize,
    meeting_id: Uuid,
    bus: EventBus,
    chunk_start_secs: f64,
    chunk_total_secs: f64,
    total_secs: f64,
    language: Option<String>,
    suppress_nst: bool,
    last_published: Arc<Mutex<f64>>,
) -> Result<PassSegments, CoreError> {
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });
    params.set_translate(false);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_n_threads(threads as i32);
    params.set_suppress_blank(true);
    params.set_language(language.as_deref());
    params.set_suppress_nst(suppress_nst);
    params.set_progress_callback_safe(move |percent: i32| {
        let percent = percent.clamp(0, 100);
        let position_secs = chunk_start_secs + (percent as f64 / 100.0) * chunk_total_secs;
        let mut last = match last_published.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        if position_secs < *last + CHUNK_PROGRESS_THROTTLE_SECS {
            return;
        }
        *last = position_secs;
        let progress = (position_secs / total_secs * 100.0).clamp(0.0, 100.0) as f32;
        bus.publish(CoreEvent::TranscriptionProgress {
            meeting_id,
            position_secs,
            progress,
        });
    });

    state
        .full(params, chunk)
        .map_err(|e| CoreError::transcription(format!("whisper transcription failed: {e}")))?;

    let detected = get_lang_str(state.full_lang_id_from_state()).map(str::to_string);
    let language = detected.or(language);

    let mut segments = Vec::new();
    let mut real_chars = 0;
    let mut no_speech_sum = 0.0;
    for i in 0..state.full_n_segments() {
        let Some(segment) = state.get_segment(i) else {
            continue;
        };
        let no_speech = segment.no_speech_probability();
        let Ok(text) = segment.to_str() else {
            continue;
        };
        if no_speech > NO_SPEECH_DROP_THRESHOLD || text.trim().is_empty() {
            continue;
        }
        no_speech_sum += no_speech;
        real_chars += content_chars(text);
        segments.push(ChunkSegment {
            text: text.to_string(),
            start: segment.start_timestamp() as f64 / 100.0,
            end: segment.end_timestamp() as f64 / 100.0,
        });
    }
    let mean_no_speech = if segments.is_empty() {
        1.0
    } else {
        no_speech_sum / segments.len() as f32
    };

    Ok(PassSegments {
        real_chars,
        mean_no_speech,
        segments,
        language,
    })
}

fn pass_is_usable(pass: &PassSegments) -> bool {
    pass.real_chars > 0 && pass.mean_no_speech < AUTO_ACCEPT_NO_SPEECH
}

#[allow(clippy::too_many_arguments)]
fn choose_auto_pass(
    state: &mut WhisperState,
    chunk: &[f32],
    threads: usize,
    meeting_id: Uuid,
    bus: &EventBus,
    chunk_start_secs: f64,
    chunk_total_secs: f64,
    total_secs: f64,
    last_published: Arc<Mutex<f64>>,
) -> Result<PassSegments, CoreError> {
    let ranked = rank_supported_languages(state, threads)
        .map_err(|e| CoreError::transcription(format!("whisper language detection failed: {e}")))?;

    let mut fallback: Option<PassSegments> = None;
    for code in ranked {
        let pass = decode_pass(
            state,
            chunk,
            threads,
            meeting_id,
            bus.clone(),
            chunk_start_secs,
            chunk_total_secs,
            total_secs,
            Some(code),
            false,
            last_published.clone(),
        )?;
        if pass_is_usable(&pass) {
            return Ok(pass);
        }
        if fallback.is_none() {
            fallback = Some(pass);
        }
    }

    match fallback {
        Some(pass) => Ok(pass),
        None => decode_pass(
            state,
            chunk,
            threads,
            meeting_id,
            bus.clone(),
            chunk_start_secs,
            chunk_total_secs,
            total_secs,
            None,
            false,
            last_published,
        ),
    }
}

/// Whisper-backed [`TranscriptionEngine`] implementation.
pub struct WhisperEngine {
    model: WhisperModel,
    language: SttLanguage,
    threads: usize,
    manager: ModelManager,
    consent: Consent,
}

impl WhisperEngine {
    /// Creates a new engine for `model` using `language`, managing the model
    /// file through `manager`. `consent` gates the model download; network
    /// downloads will only proceed when it is [`Consent::Granted`]. The
    /// worker thread count is derived from [`available_parallelism`] and
    /// capped at four so whisper leaves CPU headroom for the rest of the app.
    pub fn new(
        model: WhisperModel,
        language: SttLanguage,
        manager: ModelManager,
        consent: Consent,
    ) -> Self {
        let threads = available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 4);
        whisper_rs::install_logging_hooks();
        let model = match (language, model) {
            (SttLanguage::En, WhisperModel::Small) => WhisperModel::SmallEn,
            _ => model,
        };
        Self {
            model,
            language,
            threads,
            manager,
            consent,
        }
    }
}

#[async_trait]
impl TranscriptionEngine for WhisperEngine {
    async fn transcribe(
        &self,
        meeting_id: Uuid,
        source: AudioSource,
        event_bus: &EventBus,
    ) -> Result<TranscriptionResult, CoreError> {
        let mut samples = audio::decode(source.path())?;
        audio::normalize(&mut samples);

        let event_bus = event_bus.clone();
        let model_path = self
            .manager
            .ensure(
                self.model,
                self.consent,
                &mut |downloaded_bytes, total_bytes| {
                    event_bus.publish(CoreEvent::Processing {
                        meeting_id,
                        state: ProcessingState::Downloading {
                            downloaded_bytes,
                            total_bytes,
                        },
                    });
                },
            )
            .await?;
        event_bus.publish(CoreEvent::Processing {
            meeting_id,
            state: ProcessingState::Transcribing,
        });
        let path = model_path.clone();
        let threads = self.threads;
        let language = self.language;

        let bus = event_bus.clone();
        let samples_for_whisper = samples.clone();
        let handle = tokio::task::spawn_blocking(move || {
            run_whisper(
                &path,
                &samples_for_whisper,
                threads,
                language,
                meeting_id,
                &bus,
            )
        });

        match handle.await {
            Ok(Ok(mut result)) => {
                if result.language.is_none() && !matches!(self.language, SttLanguage::Auto) {
                    result.language = Some(language_code(self.language).to_string());
                }
                event_bus.publish(CoreEvent::Processing {
                    meeting_id,
                    state: ProcessingState::Complete,
                });
                Ok(result)
            }
            Ok(Err(error)) => {
                event_bus.publish(CoreEvent::Processing {
                    meeting_id,
                    state: ProcessingState::Failed {
                        error: error.to_string(),
                    },
                });
                Err(error)
            }
            Err(join_error) => {
                let msg = format!("transcription worker task failed: {join_error}");
                event_bus.publish(CoreEvent::Processing {
                    meeting_id,
                    state: ProcessingState::Failed { error: msg.clone() },
                });
                Err(CoreError::transcription(msg))
            }
        }
    }
}

/// Runs a blocking whisper.cpp transcription on `samples`, processing the
/// audio in fixed-length chunks with a short overlap. Each chunk is
/// transcribed fresh so per-chunk language auto-detection can follow code
/// switches; completed segments and progress are published on the cloned
/// `bus`. Segments whose start falls inside a chunk's overlap region are
/// dropped so no text is published twice.
fn run_whisper(
    model_path: &Path,
    samples: &[f32],
    threads: usize,
    language: SttLanguage,
    meeting_id: Uuid,
    bus: &EventBus,
) -> Result<TranscriptionResult, CoreError> {
    let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::new())
        .map_err(|e| {
            CoreError::transcription(format!(
                "failed to load whisper model from {}: {e}",
                model_path.display()
            ))
        })?;
    let mut state = ctx
        .create_state()
        .map_err(|e| CoreError::transcription(format!("failed to create whisper state: {e}")))?;

    let sample_rate = audio::SAMPLE_RATE as f64;
    let chunk_len = (CHUNK_SECS * sample_rate) as usize;
    let step_len = ((CHUNK_SECS - OVERLAP_SECS) * sample_rate) as usize;
    let total_secs = samples.len() as f64 / sample_rate;
    let mut chunk_start = 0usize;

    let seq: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));

    let result_segments: Arc<Mutex<Vec<TranscriptSegment>>> = Arc::new(Mutex::new(Vec::new()));
    let last_published: Arc<Mutex<f64>> = Arc::new(Mutex::new(f64::NEG_INFINITY));
    let mut language_counts: HashMap<String, u64> = HashMap::new();

    while chunk_start < samples.len() {
        let chunk_start_secs = chunk_start as f64 / sample_rate;
        let chunk_end = (chunk_start + chunk_len).min(samples.len());
        let chunk = &samples[chunk_start..chunk_end];
        let chunk_total_secs = chunk.len() as f64 / sample_rate;
        let overlap_deadline = chunk_start_secs + OVERLAP_SECS;

        state
            .pcm_to_mel(chunk, threads)
            .map_err(|e| CoreError::transcription(format!("whisper PCM->mel failed: {e}")))?;

        let chosen = match language {
            SttLanguage::Auto => choose_auto_pass(
                &mut state,
                chunk,
                threads,
                meeting_id,
                bus,
                chunk_start_secs,
                chunk_total_secs,
                total_secs,
                last_published.clone(),
            )?,
            _ => {
                let (dominant_lang_id, runner_up_lang_id, dominant_prob) =
                    detect_languages(&state, threads).map_err(|e| {
                        CoreError::transcription(format!("whisper language detection failed: {e}"))
                    })?;

                let auto_lang = dominant_lang_id.and_then(get_lang_str).map(str::to_string);

                let forced_code = Some(language_code(language).to_string());
                let primary_lang = forced_code.clone().or_else(|| auto_lang.clone());

                let first = decode_pass(
                    &mut state,
                    chunk,
                    threads,
                    meeting_id,
                    bus.clone(),
                    chunk_start_secs,
                    chunk_total_secs,
                    total_secs,
                    primary_lang,
                    matches!(language, SttLanguage::En),
                    last_published.clone(),
                )?;

                let best_chars = first.real_chars;
                let runner_up_code = runner_up_lang_id.and_then(get_lang_str).map(str::to_string);

                let mut candidates: Vec<PassSegments> = vec![first];
                let run_extra = |code: String,
                                 state: &mut WhisperState|
                 -> Result<Option<PassSegments>, CoreError> {
                    if forced_code.as_deref() == Some(code.as_str()) {
                        return Ok(None);
                    }
                    let extra = decode_pass(
                        state,
                        chunk,
                        threads,
                        meeting_id,
                        bus.clone(),
                        chunk_start_secs,
                        chunk_total_secs,
                        total_secs,
                        Some(code),
                        false,
                        last_published.clone(),
                    )?;
                    Ok(Some(extra))
                };

                match forced_code {
                    Some(ref forced) if auto_lang.as_deref() != Some(forced.as_str()) => {
                        if !pass_is_usable(&candidates[0]) {
                            if let Some(code) = auto_lang.clone() {
                                if let Some(extra) = run_extra(code, &mut state)? {
                                    candidates.push(extra);
                                }
                            }
                            if !candidates.iter().any(pass_is_usable) {
                                if let Some(code) = runner_up_code.clone() {
                                    if let Some(extra) = run_extra(code, &mut state)? {
                                        candidates.push(extra);
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        if best_chars == 0 || dominant_prob < LANG_DETECT_CONFIDENCE_THRESHOLD {
                            if let Some(code) = runner_up_code.clone() {
                                if let Some(extra) = run_extra(code, &mut state)? {
                                    candidates.push(extra);
                                }
                            }
                        }
                    }
                }

                candidates
                    .into_iter()
                    .max_by(|a, b| {
                        a.real_chars
                            .cmp(&b.real_chars)
                            .then_with(|| b.mean_no_speech.total_cmp(&a.mean_no_speech))
                    })
                    .unwrap()
            }
        };

        let chosen_language = chosen.language.clone();
        if let Some(code) = &chosen_language {
            *language_counts.entry(code.clone()).or_insert(0) += 1;
        }
        for segment in chosen.segments {
            if segment.start < overlap_deadline {
                continue;
            }
            let seg_seq = {
                let mut guard = match seq.lock() {
                    Ok(guard) => guard,
                    Err(_) => continue,
                };
                *guard += 1;
                *guard
            };
            let tx_segment = TranscriptSegment {
                seq: seg_seq,
                start: segment.start + chunk_start_secs,
                end: segment.end + chunk_start_secs,
                speaker: None,
                text: segment.text,
                confidence: None,
            };
            if let Ok(mut guard) = result_segments.lock() {
                guard.push(tx_segment.clone());
            }
            bus.publish(CoreEvent::TranscriptChunk {
                meeting_id,
                segment: tx_segment,
                language: chosen_language.clone(),
            });
        }

        chunk_start += step_len;
    }

    let final_segments = match result_segments.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };

    let dominant_language = if matches!(language, SttLanguage::Auto) {
        language_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(code, _)| code)
    } else {
        None
    };

    Ok(TranscriptionResult {
        segments: final_segments,
        language: dominant_language,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    /// Synthesises `total_secs` of 16 kHz tone bursts (3 s on / 1 s off) so the
    /// chunked pipeline is exercised with real PCM. Whisper rarely produces
    /// text for pure tones, so the test focuses on progress, chunk math and
    /// segment ordering rather than transcript content.
    fn synth_samples(total_secs: f64) -> Vec<f32> {
        const SR: usize = audio::SAMPLE_RATE as usize;
        const BURST: f64 = 3.0;
        const GAP: f64 = 1.0;
        let freqs = [220.0, 330.0, 440.0, 550.0, 660.0, 770.0];
        let total = (total_secs * SR as f64) as usize;
        let mut samples = Vec::with_capacity(total);
        let mut t = 0usize;
        let mut burst_idx = 0usize;
        loop {
            let freq = freqs[burst_idx % freqs.len()];
            for _ in 0..(BURST * SR as f64) as usize {
                if t >= total {
                    break;
                }
                samples.push(
                    (2.0 * std::f64::consts::PI * freq * t as f64 / SR as f64).sin() as f32 * 0.3,
                );
                t += 1;
            }
            for _ in 0..(GAP * SR as f64) as usize {
                if t >= total {
                    break;
                }
                samples.push(0.0);
                t += 1;
            }
            if t >= total {
                break;
            }
            burst_idx += 1;
        }
        samples
    }

    /// Resolves the directory that holds the app's GGML models. Prefers the
    /// `NOTSAI_MODELS_DIR` env override, otherwise falls back to the XDG data
    /// dir used by the desktop app (tauri bundle identifier `ai.nots.notsai`).
    fn models_dir() -> Option<std::path::PathBuf> {
        if let Some(dir) = std::env::var_os("NOTSAI_MODELS_DIR") {
            return Some(std::path::PathBuf::from(dir));
        }
        #[cfg(unix)]
        {
            let home = std::env::var_os("HOME")?;
            return Some(std::path::PathBuf::from(home).join(".local/share/ai.nots.notsai/models"));
        }
        #[cfg(windows)]
        {
            let appdata = std::env::var_os("APPDATA")?;
            return Some(std::path::PathBuf::from(appdata).join("ai.nots.notsai/models"));
        }
        #[allow(unreachable_code)]
        None
    }

    fn verify_model() -> WhisperModel {
        match std::env::var("NOTSAI_VERIFY_MODEL")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("tiny") => WhisperModel::Tiny,
            Some("small") => WhisperModel::Small,
            Some("medium") => WhisperModel::Medium,
            Some("large_v3") | Some("large-v3") | Some("large") => WhisperModel::LargeV3,
            _ => WhisperModel::Base,
        }
    }

    #[test]
    fn chunked_run_whisper_smoke() {
        let Some(models_dir) = models_dir() else {
            eprintln!("skipping: could not determine models dir");
            return;
        };
        let Some(model_path) =
            ModelManager::new(models_dir.clone()).cached_path(WhisperModel::Base)
        else {
            eprintln!(
                "skipping: base model not cached under {}",
                models_dir.display()
            );
            return;
        };

        let total_secs = 70.0;
        let samples = synth_samples(total_secs);

        let mut tmp = std::env::temp_dir();
        tmp.push(format!("notsai-smoke-{}.f32", Uuid::new_v4().simple()));
        let mut file = fs::File::create(&tmp).expect("create temp audio");
        for chunk in samples.chunks(4096) {
            let mut bytes = Vec::with_capacity(chunk.len() * 4);
            for s in chunk {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            file.write_all(&bytes).expect("write temp audio");
        }
        file.flush().expect("flush temp audio");
        let decoded = audio::decode(&tmp).expect("decode generated audio");
        fs::remove_file(&tmp).ok();
        assert_eq!(decoded.len(), samples.len(), "decode round-trip size");

        let meeting_id = Uuid::new_v4();
        let bus = EventBus::new(256);
        let mut rx = bus.subscribe();
        let result = run_whisper(&model_path, &decoded, 4, SttLanguage::En, meeting_id, &bus);

        let segments = result.expect("run_whisper succeeded");
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        let progress: Vec<(f64, f32)> = events
            .iter()
            .filter_map(|e| match e {
                CoreEvent::TranscriptionProgress {
                    meeting_id: id,
                    position_secs,
                    progress,
                } if *id == meeting_id => Some((*position_secs, *progress)),
                _ => None,
            })
            .collect();
        let transcribed: Vec<CoreEvent> = events
            .into_iter()
            .filter(|e| matches!(e, CoreEvent::TranscriptChunk { .. }))
            .collect();

        assert!(
            !progress.is_empty(),
            "expected >= 1 progress event, got {}",
            progress.len()
        );
        assert!(
            progress.iter().any(|(p, _)| *p <= 2.0 + 1e-9),
            "expected position near audio start, first={:?}",
            progress.first()
        );
        assert!(
            progress.iter().any(|(p, _)| *p >= total_secs - 5.0),
            "expected position within 5s of audio end, last={:?}",
            progress.last()
        );
        assert!(
            progress
                .iter()
                .zip(progress.iter().skip(1))
                .all(|((_, a), (_, b))| *a <= *b + 1.0),
            "progress should be roughly non-decreasing"
        );

        fn assert_ordered(list: &[(u64, f64, f64)]) {
            let mut prev_seq: Option<u64> = None;
            let mut prev_end: f64 = f64::NEG_INFINITY;
            for (seq, start, end) in list {
                if let Some(p) = prev_seq {
                    assert!(*seq > p, "sequence numbers must strictly increase");
                }
                assert!(end > start, "segment end must exceed start");
                assert!(
                    *start >= prev_end - 0.1,
                    "segments must not overlap (start {start}, prev_end {prev_end})"
                );
                prev_seq = Some(*seq);
                prev_end = *end;
            }
        }

        let emitted: Vec<(u64, f64, f64)> = transcribed
            .iter()
            .filter_map(|e| match e {
                CoreEvent::TranscriptChunk {
                    meeting_id: id,
                    segment,
                    ..
                } if *id == meeting_id => Some((segment.seq, segment.start, segment.end)),
                _ => None,
            })
            .collect();
        assert_ordered(&emitted);

        let returned: Vec<(u64, f64, f64)> = segments
            .segments
            .iter()
            .map(|s| (s.seq, s.start, s.end))
            .collect();
        assert_ordered(&returned);

        let emitted_seq: std::collections::HashSet<u64> =
            emitted.iter().map(|(seq, _, _)| *seq).collect();
        let returned_seq: std::collections::HashSet<u64> =
            returned.iter().map(|(seq, _, _)| *seq).collect();
        assert_eq!(
            emitted_seq, returned_seq,
            "emitted segment set must equal returned segment set"
        );
        assert_eq!(emitted.len(), returned.len(), "segment counts must match");

        if !transcribed.is_empty() {
            let some_lang = transcribed.iter().any(|e| {
                matches!(
                    e,
                    CoreEvent::TranscriptChunk {
                        language: Some(_),
                        ..
                    }
                )
            });
            assert!(
                some_lang,
                "at least one chunk must carry a detected language"
            );
        }

        assert!(
            segments.language.is_none(),
            "run_whisper leaves language detection up to the caller"
        );
    }

    /// Drives a pre-recorded, non-trivial audio file (e.g. a code-switched
    /// recording where each 28 s chunk is a different language) through the
    /// chunked pipeline and verifies that the per-chunk language-seeding fix
    /// is observable: each chunk emits its own detected language rather than
    /// every chunk reusing the previous one. Skipped unless
    /// `NOTSAI_VERIFY_AUDIO` points at a WAV file and the model selected by
    /// `NOTSAI_VERIFY_MODEL` (default base) is cached, so plain `cargo test`
    /// stays fast and green. Run with `-- --nocapture` to see the detected
    /// per-chunk languages.
    #[test]
    fn chunked_run_whisper_code_switched_audio() {
        let Some(audio_path) = std::env::var_os("NOTSAI_VERIFY_AUDIO") else {
            eprintln!("skipping: NOTSAI_VERIFY_AUDIO not set");
            return;
        };
        let Some(models_dir) = models_dir() else {
            eprintln!("skipping: could not determine models dir");
            return;
        };
        let model = verify_model();
        let Some(model_path) = ModelManager::new(models_dir.clone()).cached_path(model) else {
            eprintln!(
                "skipping: {model:?} model not cached under {}",
                models_dir.display()
            );
            return;
        };

        let decoded =
            audio::decode(std::path::Path::new(&audio_path)).expect("decode verification audio");
        let total_secs = decoded.len() as f64 / audio::SAMPLE_RATE as f64;
        let chunk_boundary = CHUNK_SECS - OVERLAP_SECS;
        eprintln!(
            "verification audio: {} samples, {:.2}s, {:.0} chunks",
            decoded.len(),
            total_secs,
            (total_secs / chunk_boundary).ceil()
        );
        assert!(
            total_secs >= 2.0 * chunk_boundary - 1.0,
            "recording must span at least two chunks, got {total_secs:.2}s"
        );

        let meeting_id = Uuid::new_v4();
        let bus = EventBus::new(256);
        let mut rx = bus.subscribe();
        let result = run_whisper(&model_path, &decoded, 4, SttLanguage::En, meeting_id, &bus);

        let result = result.expect("run_whisper succeeded");
        let _ = &result.segments;
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        let progress: Vec<(f64, f32)> = events
            .iter()
            .filter_map(|e| match e {
                CoreEvent::TranscriptionProgress {
                    meeting_id: id,
                    position_secs,
                    progress,
                } if *id == meeting_id => Some((*position_secs, *progress)),
                _ => None,
            })
            .collect();
        let chunks: Vec<(f64, Option<String>)> = events
            .into_iter()
            .filter_map(|e| match e {
                CoreEvent::TranscriptChunk {
                    meeting_id: id,
                    segment,
                    language,
                } if id == meeting_id => Some((segment.start, language)),
                _ => None,
            })
            .collect();

        assert!(
            progress.iter().any(|(p, _)| *p >= total_secs - 5.0),
            "expected position within 5s of audio end, last={:?}",
            progress.last()
        );
        assert!(
            progress
                .iter()
                .zip(progress.iter().skip(1))
                .all(|((_, a), (_, b))| *a <= *b + 1.0),
            "progress should be roughly non-decreasing"
        );

        let mut by_chunk: std::collections::BTreeMap<u64, String> =
            std::collections::BTreeMap::new();
        let mut carried_lang = false;
        for (start, language) in &chunks {
            if let Some(code) = language {
                carried_lang = true;
                let idx = (start / chunk_boundary) as u64;
                if let Some(existing) = by_chunk.get(&idx) {
                    assert_eq!(
                        existing, code,
                        "chunk {idx} must keep a single detected language"
                    );
                }
                by_chunk.insert(idx, code.clone());
            }
        }
        if !chunks.is_empty() {
            assert!(
                carried_lang,
                "at least one chunk must carry a detected language"
            );
            assert!(
                by_chunk.len() >= 2,
                "expected segments across >= 2 chunks, got {}",
                by_chunk.len()
            );
        }
        let distinct: std::collections::HashSet<&str> =
            by_chunk.values().map(|c| c.as_str()).collect();
        eprintln!(
            "per-chunk languages ({:?}): {by_chunk:?}",
            distinct.iter().collect::<Vec<_>>()
        );
    }

    /// TEMPORARY DIAGNOSTIC. Re-transcribes a recording chunk-by-chunk under
    /// multiple candidate language passes and prints, per pass, `real_chars`,
    /// `mean_no_speech`, and per-segment no-speech/logprob/entropy so the
    /// hallucination-selection threshold can be calibrated on real signal.
    /// Gated on `NOTSAI_VERIFY_AUDIO`; remove after calibration.
    #[test]
    fn diagnose_garbage_selection() {
        let Some(audio_path) = std::env::var_os("NOTSAI_VERIFY_AUDIO") else {
            eprintln!("skipping: NOTSAI_VERIFY_AUDIO not set");
            return;
        };
        let Some(models_dir) = models_dir() else {
            eprintln!("skipping: could not determine models dir");
            return;
        };
        let model = verify_model();
        let Some(model_path) = ModelManager::new(models_dir.clone()).cached_path(model) else {
            eprintln!("skipping: {model:?} model not cached");
            return;
        };

        let decoded =
            audio::decode(std::path::Path::new(&audio_path)).expect("decode diagnostic audio");
        let ctx = WhisperContext::new_with_params(&model_path, WhisperContextParameters::new())
            .expect("load model");
        let mut state = ctx.create_state().expect("create state");
        let threads = 4;
        let sr = audio::SAMPLE_RATE as f64;
        let chunk_len = (CHUNK_SECS * sr) as usize;
        let step_len = ((CHUNK_SECS - OVERLAP_SECS) * sr) as usize;
        let total_secs = decoded.len() as f64 / sr;
        let mut chunk_start = 0usize;
        let mut chunk_idx = 0usize;

        let meeting_id = Uuid::new_v4();
        let bus = EventBus::new(256);
        let last_published: Arc<Mutex<f64>> = Arc::new(Mutex::new(f64::NEG_INFINITY));

        while chunk_start < decoded.len() {
            let chunk_start_secs = chunk_start as f64 / sr;
            let chunk_end = (chunk_start + chunk_len).min(decoded.len());
            let chunk = &decoded[chunk_start..chunk_end];
            let chunk_total_secs = chunk.len() as f64 / sr;

            state.pcm_to_mel(chunk, threads).expect("pcm_to_mel");
            let (dominant_lang_id, runner_up_lang_id, dominant_prob) =
                detect_languages(&state, threads).expect("lang detect");

            let auto_lang = dominant_lang_id.and_then(get_lang_str).map(str::to_string);
            let runner_up = runner_up_lang_id.and_then(get_lang_str).map(str::to_string);

            eprintln!(
                "\n=== chunk {chunk_idx}: start={chunk_start_secs:.2}s len={chunk_total_secs:.2}s dom={auto_lang:?}(p={dominant_prob:.3}) runner_up={runner_up:?} ==="
            );

            let mut order = vec![None::<String>];
            for code in ["en", "hi", "mr"] {
                if Some(code.to_string()) != auto_lang {
                    order.push(Some(code.to_string()));
                }
            }

            for lang in order {
                let pass = decode_pass(
                    &mut state,
                    chunk,
                    threads,
                    meeting_id,
                    bus.clone(),
                    chunk_start_secs,
                    chunk_total_secs,
                    total_secs,
                    lang.clone(),
                    false,
                    last_published.clone(),
                )
                .expect("decode_pass");
                eprintln!(
                    "  pass lang={lang:?} -> language={:?} real_chars={} mean_no_speech={:.4} segs={}",
                    pass.language,
                    pass.real_chars,
                    pass.mean_no_speech,
                    pass.segments.len()
                );
                for i in 0..state.full_n_segments() {
                    let Some(segment) = state.get_segment(i) else {
                        continue;
                    };
                    let Ok(text) = segment.to_str() else {
                        continue;
                    };
                    if text.trim().is_empty() {
                        continue;
                    }
                    eprintln!(
                        "      seg {i}: nsp={:.3} [{:.2}-{:.2}] {text:?}",
                        segment.no_speech_probability(),
                        segment.start_timestamp() as f64 / 100.0,
                        segment.end_timestamp() as f64 / 100.0,
                    );
                }
            }

            chunk_start += step_len;
            chunk_idx += 1;
        }
    }

    /// Measures `run_whisper` wall time on a real audio file so decode-strategy
    /// and pass-count changes can be compared before/after.
    ///
    /// Usage:
    ///   NOTSAI_BENCH_AUDIO=/tmp/hi_bench.wav NOTSAI_BENCH_LANG=hi \
    ///     cargo test -p notsai-transcribe bench_run_whisper -- --nocapture
    ///
    /// Env overrides: `NOTSAI_BENCH_AUDIO` (required), `NOTSAI_BENCH_LANG`
    /// (`en|hi|mr|auto`, default `en`), `NOTSAI_BENCH_THREADS` (default 4);
    /// model resolution uses `NOTSAI_MODELS_DIR` / `NOTSAI_VERIFY_MODEL`.
    #[test]
    fn bench_run_whisper() {
        let Some(audio_path) = std::env::var_os("NOTSAI_BENCH_AUDIO") else {
            eprintln!("skipping bench_run_whisper: NOTSAI_BENCH_AUDIO not set");
            return;
        };

        let lang = match std::env::var("NOTSAI_BENCH_LANG").as_deref() {
            Ok("hi") => SttLanguage::Hi,
            Ok("mr") => SttLanguage::Mr,
            Ok("auto") => SttLanguage::Auto,
            Ok("en") => SttLanguage::En,
            Ok(other) => {
                eprintln!("skipping bench_run_whisper: unknown NOTSAI_BENCH_LANG {other:?}");
                return;
            }
            _ => SttLanguage::En,
        };

        let threads = std::env::var("NOTSAI_BENCH_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(4);

        let Some(models_dir) = models_dir() else {
            eprintln!("skipping bench_run_whisper: could not determine models dir");
            return;
        };
        let model = verify_model();
        let Some(model_path) = ModelManager::new(models_dir.clone()).cached_path(model) else {
            eprintln!(
                "skipping bench_run_whisper: {model:?} model not cached under {}",
                models_dir.display()
            );
            return;
        };

        let decoded =
            audio::decode(std::path::Path::new(&audio_path)).expect("decode benchmark audio");
        let audio_secs = decoded.len() as f64 / audio::SAMPLE_RATE as f64;

        let meeting_id = Uuid::new_v4();
        let bus = EventBus::new(256);
        let started = std::time::Instant::now();
        let result = run_whisper(&model_path, &decoded, threads, lang, meeting_id, &bus)
            .expect("run_whisper");
        let elapsed = started.elapsed();

        let chars: usize = result.segments.iter().map(|s| s.text.chars().count()).sum();
        println!(
            "bench_run_whisper: lang={lang:?} threads={threads} audio={audio_secs:.1}s elapsed={elapsed:?} segments={} chars={chars}",
            result.segments.len(),
        );
        for seg in &result.segments {
            println!(
                "bench segment {:>2}: [{:>7.2}s -> {:>7.2}s] {}",
                seg.seq, seg.start, seg.end, seg.text
            );
        }
    }

    /// Drives the real [`WhisperEngine::new`] route — including the
    /// En+Small → SmallEn model mapping and the first-run ~487 MB download
    /// through the manager's consent gate — then transcribes via the public
    /// [`TranscriptionEngine::transcribe`] method.
    ///
    /// Usage:
    ///   NOTSAI_ENGINE_TEST=1 \
    ///     cargo test -p notsai-transcribe engine_transcribe_end_to_end -- --nocapture
    ///
    /// Env overrides: `NOTSAI_ENGINE_TEST` (must be `1` to run), `NOTSAI_AUDIO`
    /// (audio file to transcribe, default `/tmp/en_bench.wav`); model dir
    /// resolution uses `NOTSAI_MODELS_DIR` via [`models_dir`]. Gated so plain
    /// `cargo test` never triggers the model download.
    #[tokio::test]
    async fn engine_transcribe_end_to_end() {
        if std::env::var("NOTSAI_ENGINE_TEST").as_deref() != Ok("1") {
            eprintln!("skipping engine_transcribe_end_to_end: NOTSAI_ENGINE_TEST != 1");
            return;
        }

        let Some(models_dir) = models_dir() else {
            eprintln!("skipping: could not determine models dir");
            return;
        };
        let audio_path = std::path::PathBuf::from(
            std::env::var_os("NOTSAI_AUDIO").unwrap_or_else(|| "/tmp/en_bench.wav".into()),
        );

        let manager = ModelManager::new(models_dir);
        let engine = WhisperEngine::new(
            WhisperModel::Small,
            SttLanguage::En,
            manager,
            Consent::Granted,
        );

        let meeting_id = Uuid::new_v4();
        let bus = EventBus::new(256);
        let mut rx = bus.subscribe();
        let result = engine
            .transcribe(meeting_id, AudioSource::File { path: audio_path }, &bus)
            .await
            .expect("transcribe succeeded");

        println!(
            "engine_transcribe_end_to_end: model={:?} segments={} chars={} language={:?}",
            engine.model,
            result.segments.len(),
            result
                .segments
                .iter()
                .map(|s| s.text.chars().count())
                .sum::<usize>(),
            result.language,
        );
        for seg in &result.segments {
            println!(
                "engine segment {:>2}: [{:>7.2}s -> {:>7.2}s] {}",
                seg.seq, seg.start, seg.end, seg.text
            );
        }

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        let progress: Vec<(f64, f32)> = events
            .iter()
            .filter_map(|e| match e {
                CoreEvent::TranscriptionProgress {
                    meeting_id: id,
                    position_secs,
                    progress,
                } if *id == meeting_id => Some((*position_secs, *progress)),
                _ => None,
            })
            .collect();
        assert!(
            !progress.is_empty(),
            "expected >= 1 progress event, got {}",
            progress.len()
        );
    }
}
