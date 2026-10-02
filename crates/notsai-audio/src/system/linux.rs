//! Linux system-audio capture via PipeWire's PulseAudio-compatible layer.
//!
//! PipeWire ships a PulseAudio shim, so the same `pactl`/`parec` pair works on
//! PipeWire and on legacy PulseAudio hosts. We resolve the default sink, find
//! its `.monitor` source, and stream raw `s16le` frames from `parec` on a
//! reader thread, forwarding `Vec<u8>` chunks over a channel.
//!
//! Capture is considered started once `parec` is confirmed alive, which is not
//! the same as the monitor producing audio. On this host PipeWire needs roughly
//! two seconds to attach the monitor before its first bytes arrive, delivered
//! as a short burst, after which the stream runs at about real time. Callers
//! must therefore tolerate a silent warm-up before asserting that system audio
//! is flowing.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::{SYSTEM_CHANNELS, SYSTEM_SAMPLE_RATE};

/// Bytes in one interleaved `s16le` frame (2 channels x 2 bytes).
const FRAME_BYTES: usize = 2 * SYSTEM_CHANNELS as usize;

/// How long to give `parec` to fail before trusting a successful start.
///
/// A rejected flag or an unknown device makes `parec` exit within a few
/// milliseconds. Without this grace period such a failure is invisible: the
/// spawn succeeds, the stream stays silent, and the user only finds out much
/// later that the recording never captured system audio.
const STARTUP_GRACE: Duration = Duration::from_millis(150);

/// How long the live-capture test waits for the monitor's first whole frame.
///
/// The monitor needs roughly two seconds to attach under PipeWire, so a fixed
/// short sleep would fail while the sink is playing. The bound stays small so a
/// genuinely silent monitor still fails promptly instead of hanging the suite.
#[cfg(test)]
const FIRST_FRAME_BUDGET: Duration = Duration::from_secs(5);

/// Upper bound on retained `parec` stderr.
///
/// The buffer only feeds a short diagnostic, so it keeps a tail rather than
/// growing for the life of the recording.
const STDERR_LIMIT: usize = 8 * 1024;

/// Live `parec` process plus the thread feeding the mixer.
pub(crate) struct Backend {
    receiver: Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    child: Option<Child>,
    reader: Option<JoinHandle<()>>,
    /// Drains `parec` stderr so the process can never block on a full pipe;
    /// joined during shutdown alongside the stdout reader.
    stderr_reader: Option<JoinHandle<()>>,
    /// Bytes left over from a chunk that did not end on a frame boundary.
    ///
    /// Pipe reads return arbitrary byte counts, so a chunk can split an
    /// `s16le` frame in half. The remainder is carried into the next chunk
    /// instead of being dropped or decoded at a shifted offset.
    partial: Vec<u8>,
}

impl Backend {
    /// Resolve the default sink's monitor and spawn `parec` on it.
    pub(crate) fn start() -> Result<Self, String> {
        let monitor = default_monitor_source().ok_or_else(|| {
            "no default system output monitor found (is PipeWire or PulseAudio running?)"
                .to_string()
        })?;

        let mut child = Command::new("parec")
            .args(parec_args(&monitor))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to launch parec: {error}"))?;

        // Drain stderr before anything else can block. An undrained stderr pipe
        // would let a chatty or failing process fill the buffer and wedge
        // `parec`, so this pump is a correctness mechanism, not just
        // diagnostics. Starting it first also means an early failure is
        // already captured by the time we look for it.
        let stderr_sink = Arc::new(Mutex::new(Vec::new()));
        let stderr_reader = {
            let stderr = match child.stderr.take() {
                Some(stderr) => stderr,
                None => {
                    reap(&mut child);
                    return Err("parec produced no stderr".to_string());
                }
            };
            let sink = Arc::clone(&stderr_sink);
            match thread::Builder::new()
                .name("notsai-system-stderr".into())
                .spawn(move || pump_stderr(stderr, sink))
            {
                Ok(handle) => handle,
                Err(error) => {
                    reap(&mut child);
                    return Err(format!("failed to spawn parec stderr thread: {error}"));
                }
            }
        };

        if let Err(error) = confirm_started(&mut child, &stderr_sink) {
            let _ = child.wait();
            let _ = stderr_reader.join();
            return Err(error);
        }

        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                reap(&mut child);
                let _ = stderr_reader.join();
                return Err("parec produced no stdout".to_string());
            }
        };

        let (sender, receiver) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let reader = match thread::Builder::new()
            .name("notsai-system-capture".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || pump(stdout, sender, stop)
            }) {
            Ok(handle) => handle,
            Err(error) => {
                reap(&mut child);
                let _ = stderr_reader.join();
                return Err(format!("failed to spawn system reader thread: {error}"));
            }
        };

        Ok(Self {
            receiver,
            stop,
            child: Some(child),
            reader: Some(reader),
            stderr_reader: Some(stderr_reader),
            partial: Vec::new(),
        })
    }

    /// Signal shutdown, join the reader, and return every byte it delivered.
    pub(crate) fn stop(mut self) -> Vec<Vec<u8>> {
        self.shutdown();
        self.drain()
    }

    /// Take every queued raw-PCM chunk, realigned to whole `s16le` frames.
    ///
    /// Pipe reads return arbitrary byte counts, so consecutive chunks can each
    /// start mid-frame. All queued bytes are consolidated into one buffer before
    /// the trailing partial frame is split off, which guarantees the caller
    /// never decodes at a shifted sample offset. The leftover is carried into
    /// the next call. The result is empty when no whole frame has arrived.
    pub(crate) fn drain(&mut self) -> Vec<Vec<u8>> {
        let mut buffer = std::mem::take(&mut self.partial);
        let mut received = false;
        while let Ok(chunk) = self.receiver.try_recv() {
            buffer.extend_from_slice(&chunk);
            received = true;
        }

        if !received && buffer.is_empty() {
            return Vec::new();
        }

        let (aligned, leftover) = split_frames(&buffer);
        self.partial = leftover.to_vec();

        if aligned.is_empty() {
            return Vec::new();
        }
        vec![aligned.to_vec()]
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(stderr_reader) = self.stderr_reader.take() {
            let _ = stderr_reader.join();
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Command-line arguments that put `parec` into raw interleaved `s16le`.
///
/// The output format is selected with `--format` only. An explicit
/// `--file-format` is not used: it makes `parec` reject the stream on hosts
/// that do not recognise the name, which would abort capture entirely.
fn parec_args(monitor: &str) -> Vec<String> {
    vec![
        format!("--device={monitor}"),
        format!("--rate={SYSTEM_SAMPLE_RATE}"),
        format!("--channels={SYSTEM_CHANNELS}"),
        "--format=s16le".to_string(),
    ]
}

/// Give `parec` a moment to fail, then surface an early exit as an error.
///
/// A live process keeps streaming, so any exit inside the grace window means the
/// arguments or device were rejected. Returning that as `Err` lets the caller
/// degrade to microphone-only instead of recording silence.
fn confirm_started(child: &mut Child, stderr: &Arc<Mutex<Vec<u8>>>) -> Result<(), String> {
    thread::sleep(STARTUP_GRACE);
    match child.try_wait() {
        Ok(Some(status)) => Err(format!(
            "parec exited immediately with {status}: {}",
            stderr_text(stderr)
        )),
        Ok(None) => Ok(()),
        Err(error) => Err(format!("failed to inspect parec: {error}")),
    }
}

/// Kill and reap a child whose start path is still being assembled.
fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Split a byte run into whole `s16le` frames plus the trailing partial frame.
fn split_frames(buffer: &[u8]) -> (&[u8], &[u8]) {
    let whole = buffer.len() - buffer.len() % FRAME_BYTES;
    buffer.split_at(whole)
}

/// Copy `parec` stdout into the channel until the process dies or we stop.
fn pump(mut stdout: impl Read, sender: Sender<Vec<u8>>, stop: Arc<AtomicBool>) {
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match stdout.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if sender.send(buffer[..read].to_vec()).is_err() {
                    return;
                }
            }
        }
    }
}

/// Drain `parec` stderr into a bounded tail until the pipe closes.
fn pump_stderr(mut stderr: impl Read, sink: Arc<Mutex<Vec<u8>>>) {
    let mut buffer = vec![0u8; 1024];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if let Ok(mut tail) = sink.lock() {
                    append_stderr(&mut tail, &buffer[..read]);
                }
            }
        }
    }
}

/// Append to the stderr tail, discarding the oldest bytes past the limit.
fn append_stderr(tail: &mut Vec<u8>, bytes: &[u8]) {
    tail.extend_from_slice(bytes);
    if tail.len() > STDERR_LIMIT {
        let excess = tail.len() - STDERR_LIMIT;
        tail.drain(..excess);
    }
}

/// Render the stderr tail for a diagnostic, with a fallback when it is empty.
fn stderr_text(sink: &Arc<Mutex<Vec<u8>>>) -> String {
    let raw = sink.lock().map(|tail| tail.clone()).unwrap_or_default();
    let text = String::from_utf8_lossy(&raw).trim().to_string();
    if text.is_empty() {
        "no diagnostic output".to_string()
    } else {
        text
    }
}

/// Name of the default sink's monitor source, e.g. `alsa_card.monitor`.
fn default_monitor_source() -> Option<String> {
    let sink = Command::new("pactl")
        .args(["get-default-sink"])
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|name| !name.is_empty())?;

    Some(format!("{sink}.monitor"))
}

#[cfg(test)]
mod tests {
    use super::{
        append_stderr, confirm_started, default_monitor_source, parec_args, pump_stderr, Backend,
        STDERR_LIMIT,
    };
    use std::process::{Command, Stdio};
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::FIRST_FRAME_BUDGET;
    use super::FRAME_BYTES;

    /// Build a childless backend plus the sender so tests can add more chunks.
    fn backend_with(chunks: Vec<Vec<u8>>) -> (Backend, Sender<Vec<u8>>) {
        let (sender, receiver) = channel();
        for chunk in chunks {
            sender.send(chunk).expect("receiver is alive");
        }
        let backend = Backend {
            receiver,
            stop: Arc::new(AtomicBool::new(false)),
            child: None,
            reader: None,
            stderr_reader: None,
            partial: Vec::new(),
        };
        (backend, sender)
    }

    /// A run of `s16le` frames whose samples are distinct per frame.
    fn frames(count: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(count * FRAME_BYTES);
        for index in 0..count {
            for channel in 0..2 {
                let value = (index as i16) * 100 + channel as i16;
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes
    }

    /// The host may have no audio server, so only assert the shape of a
    /// discovered monitor rather than requiring one to exist.
    #[test]
    fn monitor_source_is_derived_from_a_sink_name() {
        if let Some(monitor) = default_monitor_source() {
            assert!(monitor.ends_with(".monitor"), "{monitor}");
        }
    }

    /// Drain until at least one whole frame has arrived or `budget` elapses.
    ///
    /// Returns whether a frame was seen. Chunks are appended to `captured`
    /// whether or not audio shows up, so a silent monitor still produces the
    /// bytes it did manage to emit.
    fn drain_until_audio(backend: &mut Backend, captured: &mut Vec<u8>, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        loop {
            for chunk in backend.drain() {
                captured.extend_from_slice(&chunk);
            }
            if captured.len() >= FRAME_BYTES {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            thread::sleep(remaining.min(Duration::from_millis(20)));
        }
    }

    /// End-to-end check against the host audio server, skipped when there is no
    /// default sink (CI containers, headless agents).
    ///
    /// This is the regression guard for the `--file-format=-` bug: that flag
    /// made `parec` exit immediately, so the failure shows up as a startup
    /// error rather than as silence. An idle sink legitimately produces no
    /// bytes, so this asserts the process survives startup, stops cleanly, and
    /// that any bytes it does emit are frame-aligned.
    ///
    /// Set `NOTSAI_REQUIRE_SYSTEM_AUDIO=1` to additionally demand non-empty
    /// audio. That is only meaningful while something is actually playing on
    /// the default sink, since an IDLE monitor emits nothing.
    #[test]
    fn real_capture_starts_and_yields_frame_aligned_pcm() {
        if default_monitor_source().is_none() {
            eprintln!("skipping: no default system output monitor on this host");
            return;
        }

        let mut backend = Backend::start().expect("parec should start against the default monitor");
        let mut captured = Vec::new();

        if std::env::var_os("NOTSAI_REQUIRE_SYSTEM_AUDIO").is_some() {
            // PipeWire keeps a freshly opened monitor silent for roughly two
            // seconds before it starts streaming, so the strict check has to
            // wait for real audio rather than for a fixed duration.
            let heard_audio = drain_until_audio(&mut backend, &mut captured, FIRST_FRAME_BUDGET);
            let drained = backend.stop();
            for chunk in drained {
                captured.extend_from_slice(&chunk);
            }

            assert!(
                heard_audio,
                "expected live system audio within {:?} while the sink is playing, got {} bytes",
                FIRST_FRAME_BUDGET,
                captured.len()
            );
        } else {
            thread::sleep(Duration::from_millis(500));
            for chunk in backend.drain() {
                captured.extend_from_slice(&chunk);
            }
            let drained = backend.stop();
            for chunk in drained {
                captured.extend_from_slice(&chunk);
            }
        }

        assert_eq!(
            captured.len() % FRAME_BYTES,
            0,
            "captured PCM must be frame-aligned"
        );
    }

    #[test]
    fn parec_selects_raw_s16le_without_a_file_format_flag() {
        let args = parec_args("alsa_card.monitor");
        assert_eq!(
            args,
            vec![
                "--device=alsa_card.monitor".to_string(),
                "--rate=48000".to_string(),
                "--channels=2".to_string(),
                "--format=s16le".to_string(),
            ]
        );
        // `--file-format` is what made `parec` abort with "Unknown file
        // format -", so it must never reappear.
        assert!(
            !args.iter().any(|arg| arg.starts_with("--file-format")),
            "{args:?}"
        );
    }

    #[test]
    fn stderr_tail_keeps_only_the_latest_bytes() {
        let mut tail = Vec::new();
        append_stderr(&mut tail, &vec![b'a'; STDERR_LIMIT]);
        assert_eq!(tail.len(), STDERR_LIMIT);
        assert!(tail.iter().all(|&byte| byte == b'a'));

        let marker = b"tail-marker";
        append_stderr(&mut tail, marker);
        // The tail never grows past the limit.
        assert_eq!(tail.len(), STDERR_LIMIT);
        let text = String::from_utf8_lossy(&tail).to_string();
        assert!(text.ends_with("tail-marker"), "{text}");
        // Exactly the oldest bytes were evicted to make room for the marker.
        let a_count = tail.iter().take_while(|&&byte| byte == b'a').count();
        assert_eq!(a_count, STDERR_LIMIT - marker.len());
    }

    #[test]
    fn startup_check_reports_an_immediate_exit_with_stderr() {
        let mut child = Command::new("sh")
            .args(["-c", "printf 'boom: nope' >&2; exit 3"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sh");

        let sink = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("piped stderr");
        let reader = thread::spawn({
            let sink = Arc::clone(&sink);
            move || pump_stderr(stderr, sink)
        });

        let error = confirm_started(&mut child, &sink)
            .expect_err("a process that already exited must be reported");
        let _ = reader.join();

        assert!(error.contains("exited immediately"), "{error}");
        assert!(error.contains("boom: nope"), "{error}");
    }

    #[test]
    fn startup_check_passes_while_the_process_is_alive() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 5"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sh");

        let sink = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("piped stderr");
        let reader = thread::spawn({
            let sink = Arc::clone(&sink);
            move || pump_stderr(stderr, sink)
        });

        confirm_started(&mut child, &sink).expect("a live process is a healthy start");

        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
    }

    #[test]
    fn drain_returns_nothing_when_no_data_arrived() {
        let (mut backend, _sender) = backend_with(Vec::new());
        assert!(backend.drain().is_empty());
    }

    #[test]
    fn drain_consolidates_chunks_split_mid_frame() {
        // Split a run of whole frames at offsets that are not multiples of the
        // frame size, mimicking arbitrary pipe reads.
        let stream = frames(8);
        let cuts = [3, 10, 11, 29];
        let mut chunks = Vec::new();
        let mut offset = 0;
        for cut in cuts {
            chunks.push(stream[offset..cut].to_vec());
            offset = cut;
        }
        chunks.push(stream[offset..].to_vec());

        let (mut backend, _sender) = backend_with(chunks);
        let drained = backend.drain();

        // Everything is delivered as one frame-aligned buffer.
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0], stream);
        assert_eq!(drained[0].len() % FRAME_BYTES, 0);
    }

    #[test]
    fn drain_reassembles_a_frame_split_across_polls() {
        let stream = frames(4);
        // Fewer bytes than one whole frame, so nothing can be emitted yet.
        let (mut backend, sender) = backend_with(vec![stream[..3].to_vec()]);

        // Not even one whole frame yet, so nothing is emitted.
        assert!(backend.drain().is_empty());
        // A poll with no new data must not drop the carried bytes.
        assert!(backend.drain().is_empty());

        sender.send(stream[3..].to_vec()).expect("backend is alive");
        let drained = backend.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0], stream);
    }

    #[test]
    fn drain_keeps_incomplete_trailing_frame_for_the_next_poll() {
        let stream = frames(3);
        // Two whole frames plus the first two bytes of a third.
        let (mut backend, sender) = backend_with(vec![stream[..2 * FRAME_BYTES + 2].to_vec()]);
        let drained = backend.drain();
        assert_eq!(drained, vec![stream[..2 * FRAME_BYTES].to_vec()]);

        // Only the two bytes the backend is not already holding are sent, and
        // together with the held ones they complete the third frame.
        sender
            .send(stream[2 * FRAME_BYTES + 2..].to_vec())
            .expect("backend is alive");
        let drained = backend.drain();
        assert_eq!(drained, vec![stream[2 * FRAME_BYTES..].to_vec()]);
    }

    #[test]
    fn stop_returns_frames_delivered_before_shutdown() {
        let stream = frames(3);
        let (backend, _sender) = backend_with(vec![stream[..7].to_vec()]);
        let drained = backend.stop();
        // The trailing partial frame has no successor, so it is dropped.
        assert_eq!(drained, vec![stream[..FRAME_BYTES].to_vec()]);
    }
}
