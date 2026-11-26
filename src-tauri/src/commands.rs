//! Tauri commands. Each is a thin wrapper over AppCtx, the agent runtime or a module.
//! Errors cross to the frontend as strings.

use crate::session::AppCtx;
use crate::settings::{self, Settings};
use crate::types::*;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

type Ctx<'a> = State<'a, Arc<AppCtx>>;
type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

const PROVIDERS: [&str; 4] = ["anthropic", "openai", "deepgram", "openai-llm"];

#[tauri::command]
pub async fn get_settings(ctx: Ctx<'_>) -> CmdResult<Settings> {
    Ok(ctx.settings.read().await.clone())
}

#[tauri::command]
pub async fn save_settings(app: AppHandle, ctx: Ctx<'_>, mut settings: Settings) -> CmdResult<()> {
    // The live mode is owned by the backend. A stale copy from the form must not undo it.
    settings.live_mode = ctx.settings.read().await.live_mode;
    settings::save(&ctx.config_dir, &settings).map_err(err)?;
    let old = std::mem::replace(&mut *ctx.settings.write().await, settings.clone());
    if old.hotkey_toggle != settings.hotkey_toggle || old.hotkey_dictate != settings.hotkey_dictate {
        crate::register_hotkeys(&app, &settings).map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn set_api_key(provider: String, key: String) -> CmdResult<()> {
    if !PROVIDERS.contains(&provider.as_str()) {
        return Err(format!("Unknown provider {provider}"));
    }
    settings::set_api_key(&provider, key.trim()).map_err(err)
}

#[tauri::command]
pub async fn has_api_key(provider: String) -> CmdResult<bool> {
    Ok(settings::get_api_key(&provider).is_some())
}

#[tauri::command]
pub async fn list_audio_devices() -> CmdResult<Vec<AudioDeviceInfo>> {
    tokio::task::spawn_blocking(crate::audio::list_devices)
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
pub async fn start_session(ctx: Ctx<'_>, mode: Mode) -> CmdResult<()> {
    let ctx = ctx.inner().clone();
    if mode == Mode::Dictation {
        ctx.remember_focus().await;
    }
    ctx.start(mode).await.map_err(|e| {
        ctx.emit_error("session", &e);
        err(e)
    })
}

/// Remember the mode the Live hotkey starts, without starting a session.
#[tauri::command]
pub async fn set_live_mode(ctx: Ctx<'_>, mode: Mode) -> CmdResult<()> {
    if matches!(mode, Mode::Idle | Mode::Dictation) {
        return Err("Pick a live mode: transcribe, translate, meeting or agent.".into());
    }
    ctx.set_live_mode(mode).await;
    Ok(())
}

/// The given settings (the unsaved form) when present, otherwise the saved ones.
async fn settings_or_saved(ctx: &AppCtx, settings: Option<Settings>) -> Settings {
    match settings {
        Some(s) => s,
        None => ctx.settings.read().await.clone(),
    }
}

/// List the models Relay can offer now, from the stored keys and the servers that answer.
#[tauri::command]
pub async fn list_models(ctx: Ctx<'_>, settings: Option<Settings>) -> CmdResult<crate::llm::catalog::ModelCatalog> {
    let s = settings_or_saved(&ctx, settings).await;
    Ok(crate::llm::catalog::build(&s, &ctx.models_dir).await)
}

/// Send a tiny request to the given source and model. Uses the given settings (the
/// unsaved form) for addresses when present, otherwise the saved ones.
/// Returns "Connected (model)".
#[tauri::command]
pub async fn test_llm(ctx: Ctx<'_>, choice: settings::ModelChoice, settings: Option<Settings>) -> CmdResult<String> {
    let s = settings_or_saved(&ctx, settings).await;
    let client = crate::llm::LlmClient::for_choice(&s, &choice).map_err(|e| format!("{e:#}"))?;
    let req = crate::llm::LlmRequest {
        system: vec![],
        messages: vec![serde_json::json!({ "role": "user", "content": "Reply with the word ok." })],
        tools: vec![],
        max_tokens: 64,
        effort: crate::llm::Effort::Low,
        output_format: None,
    };
    client.complete(req).await.map_err(|e| format!("{e:#}"))?;
    Ok(format!("Connected ({})", client.model()))
}

#[tauri::command]
pub async fn stop_session(ctx: Ctx<'_>) -> CmdResult<()> {
    ctx.inner().clone().stop().await.map_err(err)
}

#[tauri::command]
pub async fn get_state(ctx: Ctx<'_>) -> CmdResult<AppState> {
    Ok(ctx.snapshot_state().await)
}

#[tauri::command]
pub async fn get_transcript(ctx: Ctx<'_>) -> CmdResult<TranscriptState> {
    Ok(ctx.transcript.read().await.clone())
}

#[tauri::command]
pub async fn clear_transcript(ctx: Ctx<'_>) -> CmdResult<()> {
    *ctx.transcript.write().await = TranscriptState::default();
    *ctx.notes.write().await = None;
    Ok(())
}

/// Run text through the transform (cleanup mode + instructions) and save it to history.
#[tauri::command]
pub async fn run_edit(
    ctx: Ctx<'_>,
    text: String,
    cleanup: CleanupMode,
    instructions: String,
) -> CmdResult<HistoryEntry> {
    ctx.run_edit(text, cleanup, instructions).await.map_err(err)
}

#[tauri::command]
pub async fn history_list(ctx: Ctx<'_>) -> CmdResult<Vec<HistoryEntry>> {
    Ok(ctx.history.list())
}

/// Folder that holds the History audio clips. The UI plays them through the asset protocol.
#[tauri::command]
pub async fn history_audio_dir(ctx: Ctx<'_>) -> CmdResult<String> {
    Ok(ctx.history.audio_dir().to_string_lossy().to_string())
}

#[tauri::command]
pub async fn history_delete(app: AppHandle, ctx: Ctx<'_>, id: String) -> CmdResult<()> {
    use tauri::Emitter;
    ctx.history.delete(&id).map_err(err)?;
    let _ = app.emit(crate::events::HISTORY_CHANGED, ());
    Ok(())
}

#[tauri::command]
pub async fn history_clear(app: AppHandle, ctx: Ctx<'_>) -> CmdResult<()> {
    use tauri::Emitter;
    ctx.history.clear().map_err(err)?;
    let _ = app.emit(crate::events::HISTORY_CHANGED, ());
    Ok(())
}

/// Insert text into the app that had focus before Relay. `history_id` marks that entry inserted.
#[tauri::command]
pub async fn insert_text(
    app: AppHandle,
    ctx: Ctx<'_>,
    text: String,
    history_id: Option<String>,
) -> CmdResult<()> {
    let method = ctx.settings.read().await.insert_method;
    // Hide our windows so focus returns to the app the user was working in.
    for label in ["main", "overlay"] {
        if let Some(w) = app.get_webview_window(label) {
            if w.is_focused().unwrap_or(false) {
                let _ = w.minimize();
            }
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    tokio::task::spawn_blocking(move || crate::os::insert_text(&text, method))
        .await
        .map_err(err)?
        .map_err(err)?;
    if let Some(id) = history_id {
        ctx.mark_inserted(&id);
    }
    Ok(())
}

#[tauri::command]
pub async fn export_transcript(
    ctx: Ctx<'_>,
    format: ExportFormat,
    include_translations: bool,
) -> CmdResult<String> {
    let t = ctx.transcript.read().await;
    Ok(crate::pipeline::export(&t.segments, &t.translations, format, include_translations))
}

#[tauri::command]
pub async fn save_export(
    ctx: Ctx<'_>,
    format: ExportFormat,
    include_translations: bool,
) -> CmdResult<String> {
    let content = {
        let t = ctx.transcript.read().await;
        crate::pipeline::export(&t.segments, &t.translations, format, include_translations)
    };
    let ext = match format {
        ExportFormat::Txt => "txt",
        ExportFormat::Markdown => "md",
        ExportFormat::Srt => "srt",
        ExportFormat::Vtt => "vtt",
        ExportFormat::Json => "json",
    };
    let dir = dirs::document_dir()
        .ok_or("Documents folder not found")?
        .join("Relay");
    std::fs::create_dir_all(&dir).map_err(err)?;
    let path = dir.join(format!(
        "transcript-{}.{ext}",
        chrono::Local::now().format("%Y-%m-%d-%H%M%S")
    ));
    std::fs::write(&path, content).map_err(err)?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn generate_notes(ctx: Ctx<'_>) -> CmdResult<MeetingNotes> {
    ctx.inner().clone().refresh_notes(false).await.map_err(err)
}

/// With `run` the task starts at once. Without it the task is stored as a draft.
#[tauri::command]
pub async fn agent_create_task(ctx: Ctx<'_>, title: String, detail: String, run: bool) -> CmdResult<Task> {
    let task = ctx.agent.create(title, detail, "user", run).map_err(err)?;
    ctx.publish_state().await;
    Ok(task)
}

/// Start a draft.
#[tauri::command]
pub async fn agent_run_task(ctx: Ctx<'_>, id: String) -> CmdResult<()> {
    ctx.agent.run_draft(&id).map_err(err)?;
    ctx.publish_state().await;
    Ok(())
}

#[tauri::command]
pub async fn agent_update_task(ctx: Ctx<'_>, id: String, title: String, detail: String) -> CmdResult<Task> {
    ctx.agent.update_draft(&id, &title, &detail).map_err(err)
}

/// Delete a draft or a finished task.
#[tauri::command]
pub async fn agent_delete_task(ctx: Ctx<'_>, id: String) -> CmdResult<()> {
    ctx.agent.delete_task(&id).map_err(err)?;
    ctx.publish_state().await;
    Ok(())
}

/// Ask the planner model to clean up, split and merge drafts. Changes nothing.
#[tauri::command]
pub async fn agent_plan_tasks(ctx: Ctx<'_>, ids: Vec<String>, instruction: String) -> CmdResult<crate::agent::planner::TaskPlan> {
    let drafts = ctx.agent.drafts(&ids).map_err(err)?;
    let s = ctx.settings.read().await.clone();
    let client = crate::llm::LlmClient::for_task(&s, settings::LlmTask::Planner).map_err(|e| format!("{e:#}"))?;
    crate::agent::planner::plan(&client, &drafts, &instruction)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Replace the drafts `ids` with the goals of `plan`, all or nothing.
#[tauri::command]
pub async fn agent_apply_plan(ctx: Ctx<'_>, ids: Vec<String>, plan: crate::agent::planner::TaskPlan) -> CmdResult<Vec<Task>> {
    let tasks = ctx.agent.apply_plan(&ids, &plan).map_err(err)?;
    ctx.publish_state().await;
    Ok(tasks)
}

#[tauri::command]
pub async fn agent_list_tasks(ctx: Ctx<'_>) -> CmdResult<Vec<Task>> {
    ctx.agent.list().map_err(err)
}

#[tauri::command]
pub async fn agent_cancel_task(ctx: Ctx<'_>, id: String) -> CmdResult<()> {
    ctx.agent.cancel(&id);
    ctx.publish_state().await;
    Ok(())
}

#[tauri::command]
pub async fn agent_resolve_approval(
    ctx: Ctx<'_>,
    id: String,
    approve: bool,
    remember: bool,
) -> CmdResult<()> {
    ctx.agent
        .resolve_approval(&id, approve, remember)
        .map_err(err)?;
    ctx.publish_state().await;
    Ok(())
}

#[tauri::command]
pub async fn agent_pending_approvals(ctx: Ctx<'_>) -> CmdResult<Vec<ApprovalRequest>> {
    Ok(ctx.agent.pending_approvals())
}

#[tauri::command]
pub async fn agent_reply_escalation(ctx: Ctx<'_>, task_id: String, text: String) -> CmdResult<()> {
    ctx.agent
        .reply_escalation(&task_id, &text)
        .map_err(err)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhisperStatus {
    /// False when the binary was built without the local-whisper feature.
    pub built: bool,
    pub models_dir: String,
    pub models: Vec<crate::stt::models::WhisperModelInfo>,
    /// Name of the model being downloaded, if any.
    pub downloading: Option<String>,
}

static DOWNLOAD_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static DOWNLOADING: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[tauri::command]
pub async fn whisper_status(ctx: Ctx<'_>) -> CmdResult<WhisperStatus> {
    Ok(WhisperStatus {
        built: crate::stt::LOCAL_WHISPER_BUILT,
        models_dir: ctx.models_dir.to_string_lossy().into_owned(),
        models: crate::stt::models::list(&ctx.models_dir),
        downloading: DOWNLOADING.lock().unwrap_or_else(|p| p.into_inner()).clone(),
    })
}

/// Download a catalog model. Progress arrives as `whisper-download` events
/// `{ name, done, total }`; the command resolves when the file is in place.
#[tauri::command]
pub async fn download_whisper_model(app: AppHandle, ctx: Ctx<'_>, name: String) -> CmdResult<()> {
    use std::sync::atomic::Ordering;
    use tauri::Emitter;
    {
        let mut d = DOWNLOADING.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(cur) = d.as_ref() {
            return Err(format!("{cur} is already downloading."));
        }
        *d = Some(name.clone());
    }
    DOWNLOAD_CANCEL.store(false, Ordering::Relaxed);
    let dir = ctx.models_dir.clone();
    let n = name.clone();
    let res = crate::stt::models::download(&dir, &name, &DOWNLOAD_CANCEL, |done, total| {
        let _ = app.emit(
            crate::events::WHISPER_DOWNLOAD,
            serde_json::json!({ "name": n, "done": done, "total": total }),
        );
    })
    .await;
    *DOWNLOADING.lock().unwrap_or_else(|p| p.into_inner()) = None;
    res.map(|_| ()).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn cancel_whisper_download() -> CmdResult<()> {
    DOWNLOAD_CANCEL.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub async fn show_main(app: AppHandle) -> CmdResult<()> {
    crate::show_main_window(&app);
    Ok(())
}

#[tauri::command]
pub async fn toggle_overlay(app: AppHandle) -> CmdResult<()> {
    crate::toggle_overlay_window(&app);
    Ok(())
}

/// Cancel a dictation that is listening: stop capture, discard the text, insert and save nothing.
#[tauri::command]
pub async fn cancel_dictation(ctx: Ctx<'_>) -> CmdResult<()> {
    ctx.inner().clone().cancel_dictation().await.map_err(err)
}

/// While AI cleanup runs, use the Basic result now and continue to insert.
#[tauri::command]
pub async fn skip_cleanup(ctx: Ctx<'_>) -> CmdResult<()> {
    ctx.skip_cleanup();
    Ok(())
}

/// Clear a dictation error from the overlay.
#[tauri::command]
pub async fn dismiss_dictation_error(ctx: Ctx<'_>) -> CmdResult<()> {
    ctx.dismiss_error().await;
    Ok(())
}
