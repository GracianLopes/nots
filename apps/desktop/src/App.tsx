import { useEffect, useState } from "react";
import type {
  AppSettings,
  Meeting,
  Note,
  NoteKind,
  PingStatus,
  ProviderConfig,
  ProviderKind,
  SttLanguage,
} from "@notsai/shared-types";
import {
  createMeeting,
  deleteMeeting,
  exportNotes,
  generateNotes,
  getSettings,
  listMeetings,
  listNotes,
  onProcessingState,
  onRecordingState,
  onTranscriptChunk,
  onTranscriptionProgress,
  ping,
  recordMeeting,
  readRecordingBytes,
  setSettings as saveSettings,
  stopRecording,
  transcribeMeeting,
} from "@notsai/api-client";

import { formatBytes, formatDuration } from "./format";

const PROVIDER_LABELS: Record<ProviderKind, string> = {
  gemini: "Gemini",
  groq: "Groq",
  open_router: "OpenRouter",
  local: "Local",
};

const NOTE_KIND_LABELS: Record<NoteKind, string> = {
  summary: "Summary",
  topic: "Topic",
  decision: "Decision",
  action_item: "Action",
  question: "Question",
};

const STT_LANGUAGE_LABELS: Record<SttLanguage, string> = {
  auto: "Auto (detect & switch)",
  en: "English (auto-detect)",
  hi: "Hindi",
  mr: "Marathi",
};

function upsertMeeting(
  meetings: Meeting[],
  updated: Meeting,
): Meeting[] {
  const exists = meetings.some((meeting) => meeting.id === updated.id);
  if (!exists) {
    return [...meetings, updated];
  }
  return meetings.map((meeting) =>
    meeting.id === updated.id ? updated : meeting,
  );
}

function describeError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "unknown error";
}

export default function App() {
  const [backendOnline, setBackendOnline] = useState<PingStatus | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [liveTranscript, setLiveTranscript] = useState<string[]>([]);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [startingRecordingId, setStartingRecordingId] = useState<
    string | null
  >(null);
  const [downloadProgress, setDownloadProgress] = useState<{
    meetingId: string;
    downloaded: number;
    total: number;
  } | null>(null);
  const [transcribeProgress, setTranscribeProgress] = useState<{
    meetingId: string;
    progress: number;
    positionSecs: number;
  } | null>(null);
  const [activeRecordingId, setActiveRecordingId] = useState<string | null>(
    null,
  );
  const [playingId, setPlayingId] = useState<string | null>(null);
  const [audioUrls, setAudioUrls] = useState<Record<string, string>>({});
  const [newMeetingTitle, setNewMeetingTitle] = useState("");
  const [selectedNotes, setSelectedNotes] = useState<{
    meetingId: string;
    title: string;
    notes: Note[];
  } | null>(null);
  const [exportPath, setExportPath] = useState<string | null>(null);
  const [providerDraft, setProviderDraft] = useState<{
    active: ProviderKind | null;
    providers: ProviderConfig[];
  } | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const [status, currentSettings, existing] = await Promise.all([
          ping(),
          getSettings(),
          listMeetings(),
        ]);
        if (cancelled) return;
        setBackendOnline(status);
        setSettings(currentSettings);
        setMeetings(existing);
        setErrorMsg(null);
      } catch (err) {
        console.error("failed to reach backend", err);
        setErrorMsg(describeError(err));
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (settings === null) {
      setProviderDraft(null);
      return;
    }
    setProviderDraft({
      active: settings.active_provider,
      providers: settings.providers.map((provider) => ({ ...provider })),
    });
  }, [settings]);

  useEffect(() => {
    const unlistenChunk = onTranscriptChunk((event) => {
      setLiveTranscript((prev) => [...prev, event.segment.text]);
    });
    const unlistenRecording = onRecordingState((event) => {
      setActiveRecordingId(
        event.state.state === "recording" ? event.meeting_id : null,
      );
    });
    const unlistenProgress = onTranscriptionProgress((event) => {
      setTranscribeProgress({
        meetingId: event.meeting_id,
        progress: event.progress,
        positionSecs: event.position_secs,
      });
    });
    const unlistenProcessing = onProcessingState((event) => {
      if (event.state.state === "downloading") {
        setDownloadProgress({
          meetingId: event.meeting_id,
          downloaded: event.state.downloaded_bytes,
          total: event.state.total_bytes,
        });
      } else if (
        event.state.state === "complete" ||
        event.state.state === "failed"
      ) {
        setBusyId(null);
        setDownloadProgress(null);
        setTranscribeProgress(null);
      }
    });
    return () => {
      void unlistenChunk.then((fn) => fn());
      void unlistenRecording.then((fn) => fn());
      void unlistenProgress.then((fn) => fn());
      void unlistenProcessing.then((fn) => fn());
    };
  }, []);

  const handleToggleAiConsent = async (enabled: boolean) => {
    if (settings === null) return;
    setErrorMsg(null);
    try {
      const updated = await saveSettings({ ...settings, ai_consent: enabled });
      setSettings(updated);
    } catch (err) {
      console.error("update ai_consent failed", err);
      setErrorMsg(describeError(err));
    }
  };

  const handleSetSttLanguage = async (language: SttLanguage) => {
    if (settings === null) return;
    setErrorMsg(null);
    try {
      const updated = await saveSettings({ ...settings, stt_language: language });
      setSettings(updated);
    } catch (err) {
      console.error("update stt_language failed", err);
      setErrorMsg(describeError(err));
    }
  };

  const handleUpdateProvider = (
    kind: ProviderKind,
    patch: Partial<Pick<ProviderConfig, "model" | "api_key" | "base_url">>,
  ) => {
    setProviderDraft((draft) => {
      if (draft === null) return draft;
      return {
        ...draft,
        providers: draft.providers.map((provider) =>
          provider.kind === kind ? { ...provider, ...patch } : provider,
        ),
      };
    });
  };

  const handleSaveProviderSettings = async () => {
    if (settings === null || providerDraft === null) return;
    setErrorMsg(null);
    try {
      const updated = await saveSettings({
        ...settings,
        active_provider: providerDraft.active,
        providers: providerDraft.providers,
      });
      setSettings(updated);
    } catch (err) {
      console.error("update provider settings failed", err);
      setErrorMsg(describeError(err));
    }
  };

  const handleTranscribe = async (meeting: Meeting) => {
    setBusyId(meeting.id);
    setErrorMsg(null);
    try {
      const updated = await transcribeMeeting(meeting.id);
      setMeetings((prev) => upsertMeeting(prev, updated));
    } catch (err) {
      console.error("transcribe failed", err);
      setErrorMsg(describeError(err));
    } finally {
      setBusyId(null);
      setDownloadProgress(null);
      setTranscribeProgress(null);
    }
  };

  const handleGenerateNotes = async (meeting: Meeting) => {
    setBusyId(meeting.id);
    setErrorMsg(null);
    try {
      const updated = await generateNotes(meeting.id);
      setMeetings((prev) => upsertMeeting(prev, updated));
      await loadNotes(meeting.id, updated.title);
    } catch (err) {
      console.error("generate notes failed", err);
      setErrorMsg(describeError(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleSaveNotes = async (meeting: Meeting) => {
    setBusyId(meeting.id);
    setErrorMsg(null);
    try {
      const savedPath = await exportNotes(meeting.id);
      setExportPath(savedPath);
    } catch (err) {
      console.error("export notes failed", err);
      setErrorMsg(describeError(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleRecordMeeting = async (meeting: Meeting) => {
    setStartingRecordingId(meeting.id);
    setErrorMsg(null);
    try {
      const updated = await recordMeeting(meeting.id);
      setMeetings((prev) => upsertMeeting(prev, updated));
    } catch (err) {
      console.error("record meeting failed", err);
      setErrorMsg(describeError(err));
    } finally {
      setStartingRecordingId(null);
    }
  };

  const handleStopRecording = async () => {
    setBusyId(null);
    setErrorMsg(null);
    try {
      const updated = await stopRecording();
      setMeetings((prev) => upsertMeeting(prev, updated));
      setLiveTranscript([]);
    } catch (err) {
      console.error("stop recording failed", err);
      setErrorMsg(describeError(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleCreateMeeting = async () => {
    const title = newMeetingTitle.trim();
    if (title === "") return;
    setBusyId("new");
    setErrorMsg(null);
    try {
      const created = await createMeeting(title);
      setMeetings((prev) => upsertMeeting(prev, created));
      setNewMeetingTitle("");
    } catch (err) {
      console.error("create meeting failed", err);
      setErrorMsg(describeError(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleDeleteMeeting = async (meeting: Meeting) => {
    setBusyId(meeting.id);
    setErrorMsg(null);
    try {
      await deleteMeeting(meeting.id, true);
      setMeetings((prev) => prev.filter((m) => m.id !== meeting.id));
      const path = meeting.recording_path;
      if (path != null) {
        setAudioUrls((prev) => {
          const url = prev[path];
          if (url != null) URL.revokeObjectURL(url);
          const next = { ...prev };
          delete next[path];
          return next;
        });
      }
      if (selectedNotes?.meetingId === meeting.id) {
        setSelectedNotes(null);
      }
    } catch (err) {
      console.error("delete meeting failed", err);
      setErrorMsg(describeError(err));
    } finally {
      setBusyId(null);
      setConfirmDeleteId(null);
    }
  };

  const handleViewNotes = async (meeting: Meeting) => {
    await loadNotes(meeting.id, meeting.title);
  };

  const loadNotes = async (meetingId: string, title: string) => {
    setErrorMsg(null);
    try {
      const notes = await listNotes(meetingId);
      setSelectedNotes({ meetingId, title, notes });
    } catch (err) {
      console.error("list notes failed", err);
      setErrorMsg(describeError(err));
    }
  };

  const handleTogglePlayback = async (meeting: Meeting) => {
    if (playingId === meeting.id) {
      setPlayingId(null);
      return;
    }
    const path = meeting.recording_path;
    if (path == null) return;
    setErrorMsg(null);
    try {
      let url = audioUrls[path];
      if (url == null) {
        const bytes = await readRecordingBytes(path);
        const blob = new Blob([new Uint8Array(bytes)], { type: "audio/mp4" });
        url = URL.createObjectURL(blob);
        setAudioUrls((prev) => {
          const stale = prev[path];
          if (stale != null) URL.revokeObjectURL(stale);
          return { ...prev, [path]: url };
        });
      }
      setPlayingId(meeting.id);
    } catch (err) {
      console.error("load recording failed", err);
      setErrorMsg(describeError(err));
    }
  };

  const totalRecordedSecs = meetings.reduce(
    (sum, meeting) => sum + (meeting.duration_secs ?? 0),
    0,
  );

  return (
    <main className="shell">
      <header className="shell__header">
        <h1>notsAI</h1>
        <p className="shell__subtitle">AI meeting assistant</p>
      </header>

      {errorMsg !== null && (
        <section className="card card--error" role="alert">
          <h2>Error</h2>
          <p>{errorMsg}</p>
        </section>
      )}

      {exportPath !== null && (
        <section className="card" role="status">
          <h2>Notes saved</h2>
          <p className="ok">{exportPath}</p>
        </section>
      )}

      <section className="status-grid">
        <div className="card">
          <h2>Backend</h2>
          {backendOnline ? (
            <p className="ok">
              online · v{backendOnline.version} · {backendOnline.receiverCount} receiver(s)
            </p>
          ) : (
            <p className="pending">contacting backend…</p>
          )}
        </div>
        <div className="card">
          <h2>Settings</h2>
          <p>
            {settings
              ? `${settings.ai_mode} · ${settings.whisper_model}`
              : "loading…"}
          </p>
          <label className="settings-field">
            Transcript language
            <select
              value={settings?.stt_language ?? "en"}
              disabled={settings === null || busyId != null}
              onChange={(event) =>
                void handleSetSttLanguage(event.target.value as SttLanguage)
              }
            >
              {(Object.keys(STT_LANGUAGE_LABELS) as SttLanguage[]).map(
                (lang) => (
                  <option key={lang} value={lang}>
                    {STT_LANGUAGE_LABELS[lang]}
                  </option>
                ),
              )}
            </select>
          </label>
          <label className="settings-toggle">
            <input
              type="checkbox"
              checked={settings?.ai_consent ?? false}
              disabled={settings === null || busyId != null}
              onChange={(event) =>
                void handleToggleAiConsent(event.target.checked)
              }
            />
            Allow AI model access to transcripts
          </label>
        </div>
        <div className="card">
          <h2>Meetings</h2>
          <p className="muted">
            {meetings.length} recorded · {formatDuration(totalRecordedSecs)} total
          </p>
        </div>
      </section>

      <section className="card">
        <h2>AI provider</h2>
        <div className="settings-head">
          <p className="muted">
            Provider used when generating notes; local requires a custom base URL.
          </p>
          <button
            type="button"
            disabled={busyId != null || providerDraft === null}
            onClick={() => void handleSaveProviderSettings()}
          >
            {busyId != null ? "Working…" : "Save"}
          </button>
        </div>
        {providerDraft === null ? (
          <p className="muted">loading provider settings…</p>
        ) : (
          <div className="provider-grid">
            {providerDraft.providers.map((provider) => (
              <div
                key={provider.kind}
                className={
                  providerDraft.active === provider.kind
                    ? "provider provider--active"
                    : "provider"
                }
              >
                <div className="provider__head">
                  <label>
                    <input
                      type="radio"
                      name="active_provider"
                      checked={providerDraft.active === provider.kind}
                      disabled={busyId != null}
                      onChange={() =>
                        setProviderDraft((draft) =>
                          draft === null
                            ? draft
                            : { ...draft, active: provider.kind },
                        )
                      }
                    />
                    {PROVIDER_LABELS[provider.kind]}
                  </label>
                </div>
                <label className="provider__field">
                  Model
                  <input
                    type="text"
                    value={provider.model}
                    disabled={busyId != null}
                    onChange={(event) =>
                      handleUpdateProvider(provider.kind, {
                        model: event.target.value,
                      })
                    }
                  />
                </label>
                <label className="provider__field">
                  API key
                  <input
                    type="password"
                    value={provider.api_key ?? ""}
                    disabled={busyId != null}
                    placeholder="sk-…"
                    onChange={(event) =>
                      handleUpdateProvider(provider.kind, {
                        api_key: event.target.value === "" ? undefined : event.target.value,
                      })
                    }
                  />
                </label>
                <label className="provider__field">
                  Base URL
                  <input
                    type="text"
                    value={provider.base_url ?? ""}
                    disabled={busyId != null}
                    placeholder="e.g. http://localhost:11434/v1"
                    onChange={(event) =>
                      handleUpdateProvider(provider.kind, {
                        base_url: event.target.value === "" ? undefined : event.target.value,
                      })
                    }
                  />
                </label>
              </div>
            ))}
          </div>
        )}
      </section>

      <section className="card">
        <h2>Meetings</h2>
        <div className="meeting-create">
          <input
            type="text"
            value={newMeetingTitle}
            placeholder="New meeting title…"
            disabled={busyId != null}
            onChange={(event) => setNewMeetingTitle(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                void handleCreateMeeting();
              }
            }}
          />
          <button
            type="button"
            disabled={busyId != null || newMeetingTitle.trim() === ""}
            onClick={() => void handleCreateMeeting()}
          >
            {busyId === "new" ? "Creating…" : "Create"}
          </button>
        </div>
        {meetings.length === 0 ? (
          <p className="muted">no meetings yet</p>
        ) : (
          <ul className="meeting-list">
            {meetings.map((meeting) => (
              <li key={meeting.id} className="meeting">
                <div className="meeting__main">
                  <strong>{meeting.title}</strong>
                  <span className="meeting__meta">
                    <span className={`status status--${meeting.status}`}>
                      {meeting.status}
                    </span>
                    {meeting.duration_secs != null && (
                      <span>{formatDuration(meeting.duration_secs)}</span>
                    )}
                  </span>
                </div>
                <div className="meeting__actions">
                  {(meeting.status === "created" ||
                    meeting.status === "recorded" ||
                    meeting.status === "failed") && (
                    <button
                      type="button"
                      disabled={
                        activeRecordingId != null || startingRecordingId != null
                      }
                      onClick={() => void handleRecordMeeting(meeting)}
                    >
                      {startingRecordingId === meeting.id
                        ? "Working…"
                        : "Record"}
                    </button>
                  )}
                  {activeRecordingId === meeting.id && (
                    <button
                      type="button"
                      disabled={busyId != null}
                      onClick={() => void handleStopRecording()}
                    >
                      Stop
                    </button>
                  )}
                  {meeting.status === "recorded" && (
                    <button
                      type="button"
                      disabled={busyId != null}
                      onClick={() => void handleTranscribe(meeting)}
                    >
                      {busyId === meeting.id &&
                      transcribeProgress?.meetingId === meeting.id
                        ? (() => {
                            const pct = Math.floor(transcribeProgress.progress);
                            const remaining =
                              transcribeProgress.progress > 0
                                ? (transcribeProgress.positionSecs *
                                    (100 - transcribeProgress.progress)) /
                                  transcribeProgress.progress
                                : 0;
                            return `${pct}% · ~${formatDuration(
                              Math.ceil(remaining),
                            )} left`;
                          })()
                        : busyId === meeting.id &&
                            downloadProgress?.meetingId === meeting.id
                          ? (() => {
                              const pct =
                                downloadProgress.total > 0
                                  ? Math.floor(
                                      (downloadProgress.downloaded /
                                        downloadProgress.total) *
                                        100,
                                    )
                                  : 0;
                              return `${pct}% · ${formatBytes(downloadProgress.downloaded)} / ${formatBytes(downloadProgress.total)}`;
                            })()
                          : busyId === meeting.id
                            ? "Working…"
                            : "Transcribe"}
                    </button>
                  )}
                  {(meeting.status === "ready" ||
                    meeting.status === "processing") && (
                    <button
                      type="button"
                      disabled={busyId != null}
                      onClick={() => void handleGenerateNotes(meeting)}
                    >
                      {busyId === meeting.id ? "Working…" : "Generate notes"}
                    </button>
                  )}
                  {meeting.status === "ready" && (
                    <button
                      type="button"
                      className="ghost"
                      onClick={() => void handleViewNotes(meeting)}
                    >
                      Notes
                    </button>
                  )}
                  {meeting.status === "ready" && (
                    <button
                      type="button"
                      disabled={busyId != null}
                      onClick={() => void handleSaveNotes(meeting)}
                    >
                      {busyId === meeting.id ? "Working…" : "Save notes"}
                    </button>
                  )}
                  {confirmDeleteId === meeting.id ? (
                    <>
                      <span className="confirm-hint">Delete all data?</span>
                      <button
                        type="button"
                        className="danger"
                        disabled={
                          busyId != null || activeRecordingId === meeting.id
                        }
                        onClick={() => void handleDeleteMeeting(meeting)}
                      >
                        {busyId === meeting.id ? "Deleting…" : "Confirm"}
                      </button>
                      <button
                        type="button"
                        className="ghost"
                        disabled={busyId != null}
                        onClick={() => setConfirmDeleteId(null)}
                      >
                        Cancel
                      </button>
                    </>
                  ) : (
                    <button
                      type="button"
                      className="ghost"
                      disabled={
                        busyId != null || activeRecordingId === meeting.id
                      }
                      onClick={() => setConfirmDeleteId(meeting.id)}
                    >
                      Delete
                    </button>
                  )}
                  {meeting.recording_path != null && (
                    <button
                      type="button"
                      className="ghost"
                      disabled={busyId != null}
                      onClick={() => void handleTogglePlayback(meeting)}
                    >
                      {playingId === meeting.id ? "Hide audio" : "Play"}
                    </button>
                  )}
                </div>
                {playingId === meeting.id && meeting.recording_path != null && (
                  <div className="meeting__player">
                    <audio
                      controls
                      preload="metadata"
                      src={audioUrls[meeting.recording_path]}
                      style={{ width: "100%" }}
                    >
                      Your browser does not support audio playback.
                    </audio>
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="card">
        <h2>Notes</h2>
        {selectedNotes === null ? (
          <p className="muted">select a ready meeting to see its notes</p>
        ) : (
          <div className="notes">
            <p className="notes__title">{selectedNotes.title}</p>
            {selectedNotes.notes.length === 0 ? (
              <p className="muted">no notes for this meeting yet</p>
            ) : (
              <ul className="notes__list">
                {selectedNotes.notes.map((note) => (
                  <li key={note.id} className="note">
                    <span className="note__kind">
                      {NOTE_KIND_LABELS[note.kind]}
                    </span>
                    <p className="note__content">{note.content}</p>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
      </section>

      <section className="card">
        <h2>Live transcript</h2>
        {liveTranscript.length === 0 ? (
          <p className="muted">no transcript data yet</p>
        ) : (
          <ul className="transcript">
            {liveTranscript.map((text, index) => (
              <li key={index}>{text}</li>
            ))}
          </ul>
        )}
      </section>
    </main>
  );
}