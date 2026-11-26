//! Session orchestration: audio capture -> STT -> per-mode pipelines.
//!
//! One session runs at a time. Data flow:
//!   capture thread --AudioChunk--> stt engine task --TranscriptSegment--> consumer task
//! The consumer stores final segments, emits events, and dispatches by mode:
//!   Dictation: buffer text; on stop, polish and insert into the app that had focus.
//!   Translate: translate each final segment into the target languages.
//!   Meeting:   translate, plus a timer that refreshes meeting notes.
//!   Agent:     each final microphone utterance becomes an agent task.

use crate::agent::AgentRuntime;
use crate::audio::{self, CaptureConfig, CaptureHandle};
use crate::events;
use crate::llm::LlmClient;
use crate::pipeline;
use crate::history::History;
use crate::settings::{LlmTask, SharedSettings, Settings};
use crate::stt;
use crate::types::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
use tokio::sync::{mpsc, Mutex, Notify, RwLock, Semaphore};
use tokio::task::JoinHandle;

/// A second dictate press within this time of the start cancels instead of stopping.
pub const CANCEL_WINDOW: Duration = Duration::from_millis(500);
/// A dictate key held at least this long is push-to-talk: release stops recording.
pub const HOLD_THRESHOLD: Duration = Duration::from_millis(300);
/// Pressed events closer together than this, with no release between, are key repeat.
const REPEAT_GAP: Duration = Duration::from_millis(1000);
/// How long the overlay stays up after a finished dictation.
const OVERLAY_DONE_HOLD: Duration = Duration::from_millis(1600);
/// After a stop, how long transcription of the audio already captured may run before the
/// rest is dropped. Dictation waits longer because its text is the result.
const DRAIN_LIVE: Duration = Duration::from_secs(5);
const DRAIN_DICTATION: Duration = Duration::from_secs(12);

pub struct AppCtx {
    pub app: AppHandle,
    pub settings: SharedSettings,
    pub config_dir: PathBuf,
    pub models_dir: PathBuf,
    pub transcript: SharedTranscript,
    pub notes: RwLock<Option<MeetingNotes>>,
    pub state: RwLock<AppState>,
    pub agent: Arc<AgentRuntime>,
    pub history: Arc<History>,
    session: Mutex<Option<ActiveSession>>,
    /// The stopped session still finishing its transcript. A new session waits for it.
    draining: Mutex<Option<JoinHandle<()>>>,
    /// Window that had focus when dictation started; dictation is inserted there.
    dictation_target: Mutex<Option<ActiveWindow>>,
    notes_lock: Mutex<()>,
    /// Bumped when a dictation starts or is cancelled. A finishing dictation only
    /// updates the phase while its own number is still current.
    dictation_seq: AtomicU64,
    /// Set while a dictation is cleaning with AI; notifying it skips to Basic.
    skip: std::sync::Mutex<Option<Arc<Notify>>>,
    /// True while the global Escape shortcut is registered.
    escape_registered: AtomicBool,
    /// True when Relay showed the overlay for a dictation (so it may hide it again).
    overlay_auto: AtomicBool,
    /// Mode the Live hotkey starts: the last non-dictation mode used (Transcribe at first).
    last_live_mode: std::sync::Mutex<Mode>,
}

struct ActiveSession {
    mode: Mode,
    capture: CaptureHandle,
    consumer: JoinHandle<()>,
    background: Vec<JoinHandle<()>>,
    dictation: Arc<Mutex<Vec<String>>>,
    /// Index of the first transcript segment of this session.
    first_segment: usize,
    started: std::time::Instant,
    /// Tells the speech engine to drop queued audio and abort the running chunk.
    cancel: Arc<AtomicBool>,
    /// Where the sources are recorded while the session runs, when audio is kept.
    record_prefix: Option<PathBuf>,
}

/// Input for the transform step shared by dictation, edits and transcripts.
pub struct TransformRequest {
    pub raw: String,
    pub cleanup: CleanupMode,
    pub instructions: String,
    pub match_app: bool,
    pub window: Option<ActiveWindow>,
    /// Notified to stop waiting for the AI and use the Basic result instead.
    pub skip: Option<Arc<Notify>>,
}

pub struct Transformed {
    pub text: String,
    /// Set when the transform fell back to a simpler mode.
    pub note: Option<String>,
}

impl AppCtx {
    pub fn new(
        app: AppHandle,
        settings: SharedSettings,
        config_dir: PathBuf,
        models_dir: PathBuf,
        transcript: SharedTranscript,
        agent: Arc<AgentRuntime>,
        history: Arc<History>,
    ) -> Arc<Self> {
        // Nothing holds the settings lock yet, so try_read succeeds.
        let live_mode = settings
            .try_read()
            .map(|s| s.live_mode)
            .ok()
            .filter(|m| !matches!(m, Mode::Idle | Mode::Dictation))
            .unwrap_or(Mode::Transcribe);
        Arc::new(Self {
            app,
            settings,
            config_dir,
            models_dir,
            transcript,
            notes: RwLock::new(None),
            state: RwLock::new(AppState::default()),
            agent,
            history,
            session: Mutex::new(None),
            draining: Mutex::new(None),
            dictation_target: Mutex::new(None),
            notes_lock: Mutex::new(()),
            dictation_seq: AtomicU64::new(0),
            skip: std::sync::Mutex::new(None),
            escape_registered: AtomicBool::new(false),
            overlay_auto: AtomicBool::new(false),
            last_live_mode: std::sync::Mutex::new(live_mode),
        })
    }

    /// Model client for a task. Picks Claude or the OpenAI-compatible server from settings.
    pub async fn llm(&self, task: LlmTask) -> anyhow::Result<LlmClient> {
        LlmClient::for_task(&*self.settings.read().await, task)
    }

    /// Remember the mode the Live hotkey starts, and save it to settings.json.
    /// Dictation and Idle are not live modes and are ignored.
    pub async fn set_live_mode(&self, mode: Mode) {
        if matches!(mode, Mode::Idle | Mode::Dictation) {
            return;
        }
        *self.last_live_mode.lock().unwrap_or_else(|p| p.into_inner()) = mode;
        let snapshot = {
            let mut s = self.settings.write().await;
            if s.live_mode == mode {
                return;
            }
            s.live_mode = mode;
            s.clone()
        };
        if let Err(e) = crate::settings::save(&self.config_dir, &snapshot) {
            tracing::warn!("could not save the live mode: {e:#}");
        }
    }

    pub fn emit_error(&self, scope: &str, err: impl std::fmt::Display) {
        let full = err.to_string();
        tracing::error!("{scope}: {full}");
        // The UI gets one short sentence; the full text stays in the log.
        let message = crate::friendly::friendly_error(&full);
        let _ = self.app.emit(
            events::APP_ERROR,
            serde_json::json!({ "scope": scope, "message": message }),
        );
    }

    pub async fn snapshot_state(&self) -> AppState {
        let mut st = self.state.read().await.clone();
        st.pending_approvals = self.agent.pending_approvals().len();
        st.active_tasks = self.agent.active_count();
        st
    }

    pub async fn publish_state(&self) {
        let st = self.snapshot_state().await;
        let _ = self.app.emit(events::APP_STATE, st);
    }

    pub async fn is_recording(&self) -> Option<Mode> {
        self.session.lock().await.as_ref().map(|s| s.mode)
    }

    /// Remember the focused window so dictation can be inserted there later.
    pub async fn remember_focus(&self) {
        let win = tokio::task::spawn_blocking(crate::os::active_window)
            .await
            .ok()
            .and_then(|r| r.ok())
            .filter(|w| w.hwnd != 0);
        *self.dictation_target.lock().await = win;
    }

    /// Mode and age of the running session, if any.
    pub async fn session_info(&self) -> Option<(Mode, Duration)> {
        self.session.lock().await.as_ref().map(|s| (s.mode, s.started.elapsed()))
    }

    fn current_seq(&self) -> u64 {
        self.dictation_seq.load(Ordering::SeqCst)
    }

    /// Set the phase and publish. `dictation` says whether this phase belongs to a
    /// dictation, which decides whether the global Escape shortcut is needed.
    async fn set_phase(&self, phase: Phase, detail: Option<String>) {
        let want_escape = {
            let mut st = self.state.write().await;
            apply_phase(&mut st, phase, detail);
            wants_escape(st.mode, st.phase, st.phase_detail.as_deref())
        };
        self.sync_escape(want_escape);
        self.publish_state().await;
    }

    /// Like `set_phase`, but ignored when a newer dictation has started or this one was cancelled.
    async fn set_phase_for(&self, seq: u64, phase: Phase, detail: Option<String>) {
        if seq != self.current_seq() {
            return;
        }
        self.set_phase(phase, detail).await;
    }

    fn sync_escape(&self, want: bool) {
        let was = self.escape_registered.swap(want, Ordering::SeqCst);
        if was == want {
            return;
        }
        let Ok(sc) = "Escape".parse::<Shortcut>() else { return };
        let gs = self.app.global_shortcut();
        if want {
            match gs.register(sc) {
                Ok(()) => tracing::debug!("Escape shortcut registered"),
                Err(e) => {
                    tracing::warn!("could not register Escape: {e}");
                    self.escape_registered.store(false, Ordering::SeqCst);
                }
            }
        } else {
            let _ = gs.unregister(sc);
            tracing::debug!("Escape shortcut released");
        }
    }

    /// Register Escape again after `register_hotkeys` cleared all shortcuts.
    pub fn restore_escape(&self) {
        if self.escape_registered.load(Ordering::SeqCst) {
            if let Ok(sc) = "Escape".parse::<Shortcut>() {
                let _ = self.app.global_shortcut().register(sc);
            }
        }
    }

    /// Show the overlay without taking focus, remembering whether Relay opened it.
    /// Dictation uses a small pill; Live uses a wider caption panel. The window is sized
    /// for the kind and placed at the bottom center of the screen.
    pub fn show_overlay(&self, kind: OverlayKind) {
        if let Some(w) = self.app.get_webview_window("overlay") {
            let (width, height) = kind.size();
            let _ = w.set_size(tauri::LogicalSize::new(width, height));
            place_overlay(&w);
            if !w.is_visible().unwrap_or(false) {
                let _ = w.show();
                self.overlay_auto.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Mode the Live hotkey should start.
    pub fn live_mode(&self) -> Mode {
        *self.last_live_mode.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Hide the overlay after a Live session, if Relay opened it.
    pub fn hide_overlay_after_live(&self) {
        self.hide_overlay_if_auto();
    }

    /// The user opened or closed the overlay by hand: stop managing it.
    pub fn overlay_taken_over(&self) {
        self.overlay_auto.store(false, Ordering::SeqCst);
    }

    fn hide_overlay_if_auto(&self) {
        if self.overlay_auto.swap(false, Ordering::SeqCst) {
            if let Some(w) = self.app.get_webview_window("overlay") {
                let _ = w.hide();
            }
        }
    }

    /// Escape pressed while a dictation is listening, cleaning, or showing an error.
    pub async fn escape(self: &Arc<Self>) {
        let (phase, mode) = {
            let st = self.state.read().await;
            (st.phase, st.mode)
        };
        match phase {
            Phase::Listening if mode == Mode::Dictation => {
                if let Err(e) = self.cancel_dictation().await {
                    self.emit_error("dictation", e);
                }
            }
            Phase::Cleaning => self.skip_cleanup(),
            Phase::Error => self.dismiss_error().await,
            _ => {}
        }
    }

    /// Stop capture and throw the dictation away: nothing is inserted or saved.
    pub async fn cancel_dictation(self: &Arc<Self>) -> anyhow::Result<()> {
        let sess = {
            let mut g = self.session.lock().await;
            if g.as_ref().map(|s| s.mode) == Some(Mode::Dictation) {
                g.take()
            } else {
                None
            }
        };
        let Some(sess) = sess else { return Ok(()) };
        self.dictation_seq.fetch_add(1, Ordering::SeqCst);
        sess.cancel.store(true, Ordering::Relaxed);
        let capture = sess.capture;
        let _ = tokio::task::spawn_blocking(move || capture.stop()).await;
        discard_recording(sess.record_prefix).await;
        sess.consumer.abort();
        for h in sess.background {
            h.abort();
        }
        *self.dictation_target.lock().await = None;
        {
            // Drop the segments this dictation added to the shared transcript.
            let mut t = self.transcript.write().await;
            t.segments.truncate(sess.first_segment);
        }
        let want_escape = {
            let mut st = self.state.write().await;
            st.mode = Mode::Idle;
            st.recording = false;
            apply_phase(&mut st, Phase::Idle, None);
            wants_escape(st.mode, st.phase, st.phase_detail.as_deref())
        };
        self.sync_escape(want_escape);
        self.publish_state().await;
        self.hide_overlay_if_auto();
        Ok(())
    }

    /// While AI cleanup runs, use the Basic result now. No effect in other phases.
    pub fn skip_cleanup(&self) {
        let n = self.skip.lock().ok().and_then(|g| g.clone());
        if let Some(n) = n {
            n.notify_one();
        }
    }

    /// Clear an error phase and hide the overlay if Relay opened it.
    pub async fn dismiss_error(&self) {
        let is_error = self.state.read().await.phase == Phase::Error;
        if is_error {
            self.set_phase(Phase::Idle, None).await;
            self.hide_overlay_if_auto();
        }
    }

    pub async fn start(self: &Arc<Self>, mode: Mode) -> anyhow::Result<()> {
        match self.start_inner(mode).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if mode == Mode::Dictation {
                    let seq = self.current_seq();
                    self.set_phase_for(seq, Phase::Error, Some(short_error(&e))).await;
                }
                Err(e)
            }
        }
    }

    async fn start_inner(self: &Arc<Self>, mode: Mode) -> anyhow::Result<()> {
        if mode == Mode::Idle {
            return self.stop().await;
        }
        if mode != Mode::Dictation {
            self.set_live_mode(mode).await;
        }
        if self.session.lock().await.is_some() {
            self.stop().await?;
        }
        // Let the previous session finish its transcript first, so its segments and the
        // new session's do not mix. This is bounded by the drain limits above.
        let prev = self.draining.lock().await.take();
        if let Some(prev) = prev {
            let _ = prev.await;
        }
        if mode == Mode::Dictation {
            self.dictation_seq.fetch_add(1, Ordering::SeqCst);
        }
        let settings = self.settings.read().await.clone();

        let mic_only = matches!(mode, Mode::Dictation | Mode::Agent);
        let cfg = CaptureConfig {
            capture_mic: mic_only || settings.capture_mic,
            mic_device: settings.mic_device.clone(),
            capture_system: !mic_only && settings.capture_system,
            system_device: settings.system_device.clone(),
            vad_threshold: settings.vad_threshold,
            max_chunk_ms: settings.max_chunk_ms,
            silence_ms: settings.silence_ms,
            record_prefix: settings.save_audio.then(|| {
                self.history
                    .audio_dir()
                    .join(format!("{}{}", audio::clip::SESSION_PREFIX, uuid::Uuid::new_v4()))
            }),
        };
        let record_prefix = cfg.record_prefix.clone();
        if !cfg.capture_mic && !cfg.capture_system {
            anyhow::bail!("No audio source enabled. Turn on the microphone or system audio in Settings.");
        }

        // Loading a Whisper model reads a few hundred MB; keep it off the async runtime.
        let cancel = Arc::new(AtomicBool::new(false));
        let engine = {
            let (s, dir, c) = (settings.clone(), self.models_dir.clone(), cancel.clone());
            tokio::task::spawn_blocking(move || stt::build_engine(&s, &dir, c)).await??
        };
        let (chunk_tx, chunk_rx) = mpsc::channel::<AudioChunk>(32);
        let (seg_tx, seg_rx) = mpsc::channel::<TranscriptSegment>(128);

        let app = self.app.clone();
        let level_cb: Box<dyn Fn(AudioSource, f32) + Send + Sync> = Box::new(move |source, rms| {
            let _ = app.emit(
                events::AUDIO_LEVEL,
                serde_json::json!({ "source": source, "rms": rms }),
            );
        });
        let capture = audio::start_capture(cfg, chunk_tx, level_cb)?;

        let ctx = self.clone();
        let mut background = Vec::new();
        background.push(tokio::spawn(async move {
            if let Err(e) = engine.run(chunk_rx, seg_tx).await {
                ctx.emit_error("transcription", e);
            }
        }));

        let dictation = Arc::new(Mutex::new(Vec::<String>::new()));
        let consumer = tokio::spawn(consume_segments(
            self.clone(),
            mode,
            settings.clone(),
            seg_rx,
            dictation.clone(),
        ));

        if mode == Mode::Meeting {
            let ctx = self.clone();
            let interval = settings.notes_interval_secs.max(30);
            background.push(tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(interval));
                tick.tick().await;
                loop {
                    tick.tick().await;
                    if let Err(e) = ctx.refresh_notes(false).await {
                        ctx.emit_error("notes", e);
                    }
                }
            }));
        }

        let first_segment = self.transcript.read().await.segments.len();
        *self.session.lock().await = Some(ActiveSession {
            mode,
            capture,
            consumer,
            background,
            dictation,
            first_segment,
            started: std::time::Instant::now(),
            cancel,
            record_prefix,
        });
        let want_escape = {
            let mut st = self.state.write().await;
            st.mode = mode;
            st.recording = true;
            st.session_started_at = Some(chrono::Local::now().to_rfc3339());
            st.last_error = None;
            apply_phase(&mut st, Phase::Listening, None);
            wants_escape(st.mode, st.phase, st.phase_detail.as_deref())
        };
        self.sync_escape(want_escape);
        self.publish_state().await;
        Ok(())
    }

    pub async fn stop(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(sess) = self.session.lock().await.take() else {
            return Ok(());
        };
        let is_dictation = sess.mode == Mode::Dictation;
        let seq = self.current_seq();
        let duration_ms = sess.started.elapsed().as_millis() as u64;
        // Show the stop right away. Live modes go idle; a dictation shows Transcribing while
        // its text is finished below.
        let want_escape = {
            let mut st = self.state.write().await;
            st.mode = Mode::Idle;
            st.recording = false;
            let phase = if is_dictation { Phase::Transcribing } else { Phase::Idle };
            apply_phase(&mut st, phase, None);
            wants_escape(st.mode, st.phase, st.phase_detail.as_deref())
        };
        self.sync_escape(want_escape);
        self.publish_state().await;

        // Finishing the transcript can take seconds, so it runs after the command returns.
        let ctx = self.clone();
        let handle = tokio::spawn(async move { ctx.finish_session(sess, seq, duration_ms).await });
        *self.draining.lock().await = Some(handle);
        Ok(())
    }

    /// Second half of a stop: end capture, give the speech engine a short time to transcribe
    /// what was already captured, then drop the rest and save the result.
    async fn finish_session(self: Arc<Self>, sess: ActiveSession, seq: u64, duration_ms: u64) {
        let limit = if sess.mode == Mode::Dictation { DRAIN_DICTATION } else { DRAIN_LIVE };
        // Stopping capture flushes the last utterance and drops the chunk sender, which ends
        // the speech engine, which drops the segment sender, which ends the consumer.
        // It joins the capture threads, so keep it off the async runtime.
        let capture = sess.capture;
        let _ = tokio::task::spawn_blocking(move || capture.stop()).await;
        let mut consumer = sess.consumer;
        if tokio::time::timeout(limit, &mut consumer).await.is_err() {
            tracing::warn!("transcription was still running {} s after stop; dropping the rest", limit.as_secs());
            sess.cancel.store(true, Ordering::Relaxed);
            if tokio::time::timeout(Duration::from_secs(2), &mut consumer).await.is_err() {
                consumer.abort();
            }
            self.emit_error(
                "transcription",
                "Speech recognition fell behind, so the audio from the last seconds before you stopped was not transcribed.",
            );
        }
        sess.cancel.store(true, Ordering::Relaxed);
        for h in sess.background {
            h.abort();
        }

        let rec = sess.record_prefix;
        match sess.mode {
            Mode::Dictation => {
                let raw = sess.dictation.lock().await.join(" ").trim().to_string();
                if raw.is_empty() {
                    // Nothing was said. Return to idle and let the overlay go.
                    discard_recording(rec).await;
                    self.set_phase_for(seq, Phase::Idle, None).await;
                    if seq == self.current_seq() {
                        self.hide_overlay_if_auto();
                    }
                } else {
                    let ctx = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = ctx.finish_dictation(raw, duration_ms, seq, rec.clone()).await {
                            ctx.emit_error("dictation", &e);
                            ctx.set_phase_for(seq, Phase::Error, Some(short_error(&e))).await;
                        }
                        discard_recording(rec).await;
                    });
                }
            }
            mode => {
                self.save_transcript(mode, sess.first_segment, duration_ms, rec.clone()).await;
                discard_recording(rec).await;
                if mode == Mode::Meeting {
                    let ctx = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = ctx.refresh_notes(false).await {
                            ctx.emit_error("notes", e);
                        }
                    });
                }
            }
        }
    }

    /// Mix the session's recorded sources into `<entry id>.wav` in the History audio folder.
    /// Returns the file name, or None when nothing was recorded or the mix failed.
    async fn keep_audio(&self, rec: &Option<PathBuf>, entry_id: &str) -> Option<String> {
        let prefix = rec.clone()?;
        let name = format!("{entry_id}.wav");
        let out = self.history.audio_dir().join(&name);
        let result = tokio::task::spawn_blocking(move || {
            let inputs = audio::clip::existing_sources(&prefix);
            audio::clip::mix(&inputs, &out)
        })
        .await;
        match result {
            Ok(Ok(ms)) if ms > 0 => Some(name),
            Ok(Ok(_)) => None,
            Ok(Err(e)) => {
                tracing::warn!("the audio for this entry was not kept: {e:#}");
                None
            }
            Err(e) => {
                tracing::warn!("the audio for this entry was not kept: {e}");
                None
            }
        }
    }

    /// The transform step: Off returns the text trimmed, Basic runs local rules,
    /// AI runs the dictation model with the instructions and falls back to Basic.
    pub async fn transform(&self, req: TransformRequest) -> Transformed {
        let raw = req.raw.trim();
        let settings = self.settings.read().await.clone();
        let lang = settings
            .source_language
            .as_deref()
            .map(stt::base_language)
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| "auto".into());
        let basic = || pipeline::cleanup::basic(raw, &lang);
        match req.cleanup {
            CleanupMode::Off => Transformed { text: raw.to_string(), note: None },
            CleanupMode::Basic => Transformed { text: basic(), note: None },
            CleanupMode::Ai => {
                let pctx = PolishContext {
                    instructions: req.instructions,
                    match_app: req.match_app,
                    window: req.window,
                    user_profile: settings.user_profile.clone(),
                };
                let polish = async {
                    let client = self.llm(LlmTask::Dictation).await?;
                    pipeline::polish(&client, raw, &pctx).await
                };
                // None means the user skipped the AI step.
                let result = match &req.skip {
                    Some(skip) => tokio::select! {
                        r = polish => Some(r),
                        _ = skip.notified() => None,
                    },
                    None => Some(polish.await),
                };
                match result {
                    None => Transformed {
                        text: basic(),
                        note: Some("Used Basic cleanup (AI skipped).".into()),
                    },
                    Some(Ok(text)) => Transformed { text, note: None },
                    Some(Err(e)) => {
                        tracing::warn!("AI cleanup failed, using Basic: {e:#}");
                        Transformed {
                            text: basic(),
                            note: Some(crate::friendly::cleanup_note(&format!("{e:#}"))),
                        }
                    }
                }
            }
        }
    }

    fn add_history(&self, entry: &HistoryEntry) {
        if let Err(e) = self.history.add(entry.clone()) {
            self.emit_error("history", e);
        }
        let _ = self.app.emit(events::HISTORY_CHANGED, ());
    }

    /// Run typed or pasted text through the transform and save it as an Edit.
    pub async fn run_edit(&self, raw: String, cleanup: CleanupMode, instructions: String) -> anyhow::Result<HistoryEntry> {
        if raw.trim().is_empty() {
            anyhow::bail!("There is no text to edit. Type, paste or dictate something first.");
        }
        // An edit has no target app yet, so there is no app tone hint.
        let out = self
            .transform(TransformRequest {
                raw: raw.clone(),
                cleanup,
                instructions: instructions.clone(),
                match_app: false,
                window: None,
                skip: None,
            })
            .await;
        let entry = HistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            kind: HistoryKind::Edit,
            created_at: chrono::Local::now().to_rfc3339(),
            raw: raw.trim().to_string(),
            text: out.text,
            cleanup,
            instructions,
            app: None,
            mode: None,
            duration_ms: None,
            note: out.note,
            inserted: false,
            audio_file: None,
        };
        self.add_history(&entry);
        Ok(entry)
    }

    /// Mark a history entry as inserted.
    pub fn mark_inserted(&self, id: &str) {
        if let Some(mut e) = self.history.list().into_iter().find(|e| e.id == id) {
            e.inserted = true;
            let _ = self.history.update(e);
            let _ = self.app.emit(events::HISTORY_CHANGED, ());
        }
    }

    async fn finish_dictation(
        self: &Arc<Self>,
        raw: String,
        duration_ms: u64,
        seq: u64,
        rec: Option<PathBuf>,
    ) -> anyhow::Result<()> {
        let settings = self.settings.read().await.clone();
        // Tone and insert target come from the app that had focus when dictation started.
        let window = self.dictation_target.lock().await.take();
        let target = window.as_ref().map(|w| w.hwnd);
        let app = window.as_ref().map(|w| w.process_name.clone()).filter(|p| !p.is_empty());

        // Register the skip handle before the phase is published, so an early Escape is not lost.
        let skip = Arc::new(Notify::new());
        if settings.cleanup == CleanupMode::Ai {
            if let Ok(mut g) = self.skip.lock() {
                *g = Some(skip.clone());
            }
        }
        let cleaning = match settings.cleanup {
            CleanupMode::Ai => Some("ai"),
            CleanupMode::Basic => Some("basic"),
            CleanupMode::Off => None,
        };
        if let Some(kind) = cleaning {
            self.set_phase_for(seq, Phase::Cleaning, Some(kind.into())).await;
        }
        let out = self
            .transform(TransformRequest {
                raw: raw.clone(),
                cleanup: settings.cleanup,
                instructions: settings.instructions.clone(),
                match_app: settings.auto_tone_from_app,
                window,
                skip: Some(skip),
            })
            .await;
        if let Ok(mut g) = self.skip.lock() {
            *g = None;
        }

        let mut insert_error: Option<String> = None;
        let mut inserted = false;
        if settings.auto_insert_dictation && !out.text.is_empty() {
            let text = out.text.clone();
            let method = settings.insert_method;
            let res = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                if let Some(h) = target {
                    let _ = crate::os::focus_window(h);
                    std::thread::sleep(Duration::from_millis(120));
                }
                crate::os::insert_text(&text, method)
            })
            .await?;
            match res {
                Ok(()) => inserted = true,
                Err(e) => {
                    insert_error = Some(format!("Could not insert: {}", short_error(&e)));
                    self.emit_error("insert", e);
                }
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let audio_file = self.keep_audio(&rec, &id).await;
        let entry = HistoryEntry {
            id,
            kind: HistoryKind::Dictation,
            created_at: chrono::Local::now().to_rfc3339(),
            raw,
            text: out.text,
            cleanup: settings.cleanup,
            instructions: settings.instructions.clone(),
            app: app.clone(),
            mode: Some(Mode::Dictation),
            duration_ms: Some(duration_ms),
            note: out.note,
            inserted,
            audio_file,
        };
        self.add_history(&entry);
        let _ = self.app.emit(events::DICTATION_RESULT, &entry);

        match insert_error {
            Some(reason) => self.set_phase_for(seq, Phase::Error, Some(reason)).await,
            None => {
                let label = app.as_deref().map(app_label).filter(|l| !l.is_empty());
                self.set_phase_for(seq, Phase::Done, label).await;
                // Hide the overlay a moment after Done, if Relay opened it and nothing newer started.
                let ctx = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(OVERLAY_DONE_HOLD).await;
                    let still_done = ctx.state.read().await.phase == Phase::Done;
                    if seq == ctx.current_seq() && still_done {
                        ctx.hide_overlay_if_auto();
                    }
                });
            }
        }
        Ok(())
    }

    /// Save the final segments of a finished live session as one Transcript entry.
    async fn save_transcript(&self, mode: Mode, first_segment: usize, duration_ms: u64, rec: Option<PathBuf>) {
        let segments = {
            let t = self.transcript.read().await;
            t.segments.get(first_segment..).map(|s| s.to_vec()).unwrap_or_default()
        };
        let lines: Vec<String> = segments
            .iter()
            .filter(|s| s.is_final && !s.text.trim().is_empty())
            .map(|s| format!("{}: {}", pipeline::speaker_label(s), s.text.trim()))
            .collect();
        if lines.is_empty() {
            return;
        }
        let text = lines.join("\n");
        let id = uuid::Uuid::new_v4().to_string();
        let audio_file = self.keep_audio(&rec, &id).await;
        let entry = HistoryEntry {
            id,
            kind: HistoryKind::Transcript,
            created_at: chrono::Local::now().to_rfc3339(),
            raw: text.clone(),
            text,
            cleanup: CleanupMode::Off,
            instructions: String::new(),
            app: None,
            mode: Some(mode),
            duration_ms: Some(duration_ms),
            note: None,
            inserted: false,
            audio_file,
        };
        self.add_history(&entry);
    }

    /// Refresh meeting notes from new final segments. `force` regenerates from scratch.
    pub async fn refresh_notes(self: &Arc<Self>, force: bool) -> anyhow::Result<MeetingNotes> {
        let _guard = self.notes_lock.lock().await;
        let segments = self.transcript.read().await.segments.clone();
        let prior = if force { None } else { self.notes.read().await.clone() };
        if let Some(p) = &prior {
            if p.covered_segments >= segments.len() {
                return Ok(p.clone());
            }
        }
        if segments.is_empty() {
            anyhow::bail!("The transcript is empty. Record a meeting first.");
        }
        let client = self.llm(LlmTask::Notes).await?;
        let notes =pipeline::meeting_notes(&client, &segments, prior.as_ref()).await?;

        let auto_tasks = self.settings.read().await.auto_extract_tasks;
        if auto_tasks {
            let known: Vec<String> = prior
                .as_ref()
                .map(|p| p.action_items.iter().map(|a| a.text.to_lowercase()).collect())
                .unwrap_or_default();
            // Action items become drafts in the backlog. An item whose title matches a
            // draft or an open task is skipped.
            let mut open = self.agent.open_title_keys().unwrap_or_default();
            let mut created = false;
            for item in notes.action_items.iter().filter(|a| !known.contains(&a.text.to_lowercase())) {
                let title = truncate(&item.text, 80);
                if !open.insert(crate::agent::runtime::title_key(&title)) {
                    continue;
                }
                let mut detail = format!("Action item from the meeting: {}", item.text);
                if let Some(o) = &item.owner {
                    detail.push_str(&format!("\nOwner: {o}"));
                }
                if let Some(d) = &item.due {
                    detail.push_str(&format!("\nDue: {d}"));
                }
                match self.agent.create_draft(title, detail, "meeting") {
                    Ok(_) => created = true,
                    Err(e) => self.emit_error("agent", e),
                }
            }
            if created {
                self.publish_state().await;
            }
        }

        *self.notes.write().await = Some(notes.clone());
        let _ = self.app.emit(events::NOTES_UPDATED, &notes);
        Ok(notes)
    }
}

async fn consume_segments(
    ctx: Arc<AppCtx>,
    mode: Mode,
    settings: Settings,
    mut rx: mpsc::Receiver<TranscriptSegment>,
    dictation: Arc<Mutex<Vec<String>>>,
) {
    let translate_limit = Arc::new(Semaphore::new(4));
    let wants_translation = matches!(mode, Mode::Translate | Mode::Meeting)
        && !settings.target_languages.is_empty();

    while let Some(seg) = rx.recv().await {
        tracing::debug!(source = ?seg.source, start_ms = seg.start_ms, "segment: {}", seg.text);
        let _ = ctx.app.emit(events::TRANSCRIPT_SEGMENT, &seg);
        if !seg.is_final || seg.text.trim().is_empty() {
            continue;
        }
        let recent: Vec<TranscriptSegment> = {
            let mut t = ctx.transcript.write().await;
            t.segments.push(seg.clone());
            let n = t.segments.len();
            t.segments[n.saturating_sub(7)..n.saturating_sub(1)].to_vec()
        };

        match mode {
            Mode::Dictation => dictation.lock().await.push(seg.text.trim().to_string()),
            Mode::Agent => {
                if seg.source == AudioSource::Microphone {
                    let text = seg.text.trim().to_string();
                    if let Err(e) = ctx.agent.create_task(truncate(&text, 80), text.clone(), "voice") {
                        ctx.emit_error("agent", e);
                    }
                    ctx.publish_state().await;
                }
            }
            Mode::Translate | Mode::Meeting if wants_translation => {
                let enabled = match seg.source {
                    AudioSource::Microphone => settings.translate_mic,
                    AudioSource::System => settings.translate_system,
                };
                if !enabled {
                    continue;
                }
                let ctx = ctx.clone();
                let targets = settings.target_languages.clone();
                let permit = translate_limit.clone();
                tokio::spawn(async move {
                    let _p = permit.acquire_owned().await;
                    let result = async {
                        let client = ctx.llm(LlmTask::Translation).await?;
                        pipeline::translate(&client, &seg, &targets, &recent).await
                    }
                    .await;
                    match result {
                        Ok(list) => {
                            let mut t = ctx.transcript.write().await;
                            for tr in list {
                                let _ = ctx.app.emit(events::TRANSLATION_SEGMENT, &tr);
                                t.translations.push(tr);
                            }
                        }
                        Err(e) => ctx.emit_error("translation", e),
                    }
                });
            }
            _ => {}
        }
    }
}

/// Which overlay layout a hotkey opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKind {
    /// Small pill for dictation into another app.
    Dictation,
    /// Wider caption panel for Transcribe, Translate, Meeting and Agent.
    Live,
}

impl OverlayKind {
    /// Window size in logical pixels. The frontend draws inside this box.
    pub fn size(self) -> (f64, f64) {
        match self {
            // One overlay design for both hotkeys: a pill with a text bubble above it.
            OverlayKind::Dictation | OverlayKind::Live => (560.0, 150.0),
        }
    }
}

/// Bottom center of the monitor under the cursor (or the primary one), above the taskbar.
fn place_overlay(w: &tauri::WebviewWindow) {
    let monitor = w
        .cursor_position()
        .ok()
        .and_then(|p| w.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| w.primary_monitor().ok().flatten());
    let (Some(m), Ok(size)) = (monitor, w.outer_size()) else { return };
    let area = m.work_area();
    let x = area.position.x + (area.size.width as i32 - size.width as i32) / 2;
    let margin = (16.0 * m.scale_factor()) as i32;
    let y = area.position.y + area.size.height as i32 - size.height as i32 - margin;
    let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
}

/// Remove a session's per-source recordings, off the async runtime.
async fn discard_recording(rec: Option<PathBuf>) {
    if let Some(prefix) = rec {
        let _ = tokio::task::spawn_blocking(move || audio::clip::discard(&prefix)).await;
    }
}

/// First line of an error, capped, for notes shown in the UI.
fn short_error(e: &anyhow::Error) -> String {
    crate::friendly::friendly_error(&format!("{e:#}"))
}

pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(3)).collect();
        out.push_str("...");
        out
    }
}

// ---------- Pure logic: phases and the dictate key ----------

/// Set the phase fields on `st`. Idle clears the detail.
pub fn apply_phase(st: &mut AppState, phase: Phase, detail: Option<String>) {
    st.phase = phase;
    st.phase_since = Some(chrono::Local::now().to_rfc3339());
    st.phase_detail = if phase == Phase::Idle { None } else { detail };
}

/// The global Escape shortcut is only taken while it does something: cancelling a
/// dictation that is listening, skipping AI cleanup, or dismissing an error.
pub fn wants_escape(mode: Mode, phase: Phase, detail: Option<&str>) -> bool {
    match phase {
        Phase::Listening => mode == Mode::Dictation,
        Phase::Cleaning => detail == Some("ai"),
        Phase::Error => true,
        _ => false,
    }
}

/// "outlook.exe" becomes "outlook".
pub fn app_label(process_name: &str) -> String {
    let p = process_name.trim();
    let lower = p.to_ascii_lowercase();
    lower.strip_suffix(".exe").map(|_| p[..p.len() - 4].to_string()).unwrap_or_else(|| p.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressAction {
    Start,
    Stop,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseAction {
    None,
    Stop,
}

/// Tracks the dictate hotkey between Pressed and Released events.
///
/// A press while idle starts recording. A release after at least `HOLD_THRESHOLD`
/// stops it (push-to-talk); a shorter release leaves it running (tap to toggle).
/// A press while recording stops it, or cancels it when it comes within
/// `CANCEL_WINDOW` of the start.
#[derive(Debug, Default)]
pub struct DictateKeys {
    down_since: Option<Instant>,
    last_event: Option<Instant>,
    started_here: bool,
}

impl DictateKeys {
    /// `since_start` is the age of the running dictation, or None when not dictating.
    pub fn press(&mut self, now: Instant, since_start: Option<Duration>) -> Option<PressAction> {
        if let (Some(_), Some(last)) = (self.down_since, self.last_event) {
            if now.saturating_duration_since(last) < REPEAT_GAP {
                // Key repeat while held.
                self.last_event = Some(now);
                return None;
            }
        }
        self.down_since = Some(now);
        self.last_event = Some(now);
        match since_start {
            None => {
                self.started_here = true;
                Some(PressAction::Start)
            }
            Some(age) => {
                self.started_here = false;
                Some(if age < CANCEL_WINDOW { PressAction::Cancel } else { PressAction::Stop })
            }
        }
    }

    pub fn release(&mut self, now: Instant) -> ReleaseAction {
        let Some(down) = self.down_since.take() else {
            return ReleaseAction::None;
        };
        self.last_event = None;
        let started_here = std::mem::take(&mut self.started_here);
        if started_here && now.saturating_duration_since(down) >= HOLD_THRESHOLD {
            ReleaseAction::Stop
        } else {
            ReleaseAction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn tap_starts_and_stays_recording() {
        let t0 = Instant::now();
        let mut k = DictateKeys::default();
        assert_eq!(k.press(t0, None), Some(PressAction::Start));
        assert_eq!(k.release(t0 + ms(120)), ReleaseAction::None);
    }

    #[test]
    fn hold_stops_on_release() {
        let t0 = Instant::now();
        let mut k = DictateKeys::default();
        assert_eq!(k.press(t0, None), Some(PressAction::Start));
        assert_eq!(k.release(t0 + ms(300)), ReleaseAction::Stop);
        assert_eq!(k.release(t0 + ms(400)), ReleaseAction::None);
    }

    #[test]
    fn just_under_threshold_is_a_tap() {
        let t0 = Instant::now();
        let mut k = DictateKeys::default();
        k.press(t0, None);
        assert_eq!(k.release(t0 + ms(299)), ReleaseAction::None);
    }

    #[test]
    fn second_press_stops_after_the_cancel_window() {
        let t0 = Instant::now();
        let mut k = DictateKeys::default();
        k.press(t0, None);
        k.release(t0 + ms(100));
        assert_eq!(k.press(t0 + ms(5000), Some(ms(4900))), Some(PressAction::Stop));
        // Releasing the stopping press never stops anything again.
        assert_eq!(k.release(t0 + ms(5600)), ReleaseAction::None);
    }

    #[test]
    fn quick_second_press_cancels() {
        let t0 = Instant::now();
        let mut k = DictateKeys::default();
        k.press(t0, None);
        k.release(t0 + ms(80));
        assert_eq!(k.press(t0 + ms(300), Some(ms(300))), Some(PressAction::Cancel));
        assert_eq!(k.release(t0 + ms(900)), ReleaseAction::None);
    }

    #[test]
    fn cancel_window_edge() {
        let t0 = Instant::now();
        let mut k = DictateKeys::default();
        assert_eq!(k.press(t0, Some(ms(499))), Some(PressAction::Cancel));
        k.release(t0);
        assert_eq!(k.press(t0 + ms(2000), Some(ms(500))), Some(PressAction::Stop));
    }

    #[test]
    fn key_repeat_is_ignored() {
        let t0 = Instant::now();
        let mut k = DictateKeys::default();
        assert_eq!(k.press(t0, None), Some(PressAction::Start));
        assert_eq!(k.press(t0 + ms(500), Some(ms(500))), None);
        assert_eq!(k.press(t0 + ms(530), Some(ms(530))), None);
        assert_eq!(k.release(t0 + ms(900)), ReleaseAction::Stop);
    }

    #[test]
    fn missed_release_does_not_block_the_next_press() {
        let t0 = Instant::now();
        let mut k = DictateKeys::default();
        k.press(t0, None);
        assert_eq!(k.press(t0 + ms(5000), Some(ms(5000))), Some(PressAction::Stop));
    }

    #[test]
    fn escape_is_taken_only_when_it_does_something() {
        assert!(wants_escape(Mode::Dictation, Phase::Listening, None));
        assert!(!wants_escape(Mode::Meeting, Phase::Listening, None));
        assert!(wants_escape(Mode::Idle, Phase::Cleaning, Some("ai")));
        assert!(!wants_escape(Mode::Idle, Phase::Cleaning, Some("basic")));
        assert!(wants_escape(Mode::Idle, Phase::Error, Some("x")));
        assert!(!wants_escape(Mode::Idle, Phase::Transcribing, None));
        assert!(!wants_escape(Mode::Idle, Phase::Done, None));
        assert!(!wants_escape(Mode::Idle, Phase::Idle, None));
    }

    #[test]
    fn phases_walk_and_idle_clears_detail() {
        let mut st = AppState::default();
        assert_eq!(st.phase, Phase::Idle);
        apply_phase(&mut st, Phase::Listening, None);
        apply_phase(&mut st, Phase::Transcribing, None);
        apply_phase(&mut st, Phase::Cleaning, Some("ai".into()));
        assert_eq!(st.phase_detail.as_deref(), Some("ai"));
        apply_phase(&mut st, Phase::Done, Some("outlook".into()));
        assert_eq!(st.phase, Phase::Done);
        assert!(st.phase_since.is_some());
        apply_phase(&mut st, Phase::Idle, Some("left over".into()));
        assert_eq!(st.phase_detail, None);
    }

    #[test]
    fn phase_serializes_camel_case() {
        assert_eq!(serde_json::to_string(&Phase::Transcribing).unwrap(), "\"transcribing\"");
        let json = serde_json::to_value(AppState::default()).unwrap();
        assert_eq!(json["phase"], "idle");
        assert!(json.get("phaseSince").is_some());
        assert!(json.get("phaseDetail").is_some());
    }

    #[test]
    fn app_label_drops_exe() {
        assert_eq!(app_label("outlook.exe"), "outlook");
        assert_eq!(app_label("Code.EXE"), "Code");
        assert_eq!(app_label("notepad"), "notepad");
        assert_eq!(app_label(""), "");
    }
}
