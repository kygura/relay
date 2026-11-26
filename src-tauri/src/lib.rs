//! App entry: state setup, tray icon, global hotkeys and command registration.

pub mod agent;
pub mod audio;
pub mod commands;
pub mod events;
pub mod friendly;
pub mod history;
pub mod llm;
pub mod os;
pub mod pipeline;
pub mod session;
pub mod settings;
pub mod stt;
pub mod types;

use session::{AppCtx, DictateKeys, OverlayKind, PressAction, ReleaseAction};
use std::sync::Arc;
use std::time::Instant;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tokio::sync::{mpsc, RwLock};
use types::Mode;

pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn toggle_overlay_window(app: &AppHandle) {
    if let Some(ctx) = app.try_state::<Arc<AppCtx>>() {
        // The user is in control of the overlay now; do not hide it automatically.
        ctx.overlay_taken_over();
    }
    if let Some(w) = app.get_webview_window("overlay") {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            let _ = w.show();
        }
    }
}

/// Replace the registered global shortcuts with the ones in `settings`.
pub fn register_hotkeys(app: &AppHandle, settings: &settings::Settings) -> anyhow::Result<()> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    // unregister_all also dropped Escape; take it again if a dictation needs it.
    if let Some(ctx) = app.try_state::<Arc<AppCtx>>() {
        ctx.restore_escape();
    }
    for (name, combo) in [("toggle", &settings.hotkey_toggle), ("dictate", &settings.hotkey_dictate)] {
        if combo.trim().is_empty() {
            continue;
        }
        let sc: Shortcut = combo
            .parse()
            .map_err(|e| anyhow::anyhow!("Hotkey {name} ({combo}) is not valid: {e}"))?;
        gs.register(sc)
            .map_err(|e| anyhow::anyhow!("Could not register hotkey {combo}: {e}"))?;
    }
    Ok(())
}

/// One global shortcut event, queued so Pressed and Released are handled in order.
struct HotkeyEvent {
    shortcut: Shortcut,
    pressed: bool,
    at: Instant,
}

struct HotkeyQueue(mpsc::UnboundedSender<HotkeyEvent>);

fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, pressed: bool) {
    let escape = "Escape".parse::<Shortcut>().map(|e| e == *shortcut).unwrap_or(false);
    if escape {
        tracing::debug!(pressed, "Escape shortcut event");
        if pressed {
            if let Some(ctx) = app.try_state::<Arc<AppCtx>>().map(|s| s.inner().clone()) {
                tauri::async_runtime::spawn(async move { ctx.escape().await });
            }
        }
        return;
    }
    if let Some(q) = app.try_state::<HotkeyQueue>() {
        let _ = q.0.send(HotkeyEvent { shortcut: shortcut.clone(), pressed, at: Instant::now() });
    }
}

/// Handles queued hotkey events one at a time, so a release is never handled before
/// the start it belongs to has finished.
async fn hotkey_loop(_app: AppHandle,ctx: Arc<AppCtx>, mut rx: mpsc::UnboundedReceiver<HotkeyEvent>) {
    let mut dictate_key = DictateKeys::default();
    // Windows repeats a hotkey while it is held. Only the first press after a release counts.
    let mut live_key_down = false;
    while let Some(ev) = rx.recv().await {
        let s = ctx.settings.read().await.clone();
        let matches = |combo: &str| combo.parse::<Shortcut>().map(|c| c == ev.shortcut).unwrap_or(false);
        if matches(&s.hotkey_toggle) {
            if !ev.pressed {
                live_key_down = false;
                continue;
            }
            if live_key_down {
                continue;
            }
            live_key_down = true;
            // Live hotkey: stop a running live session, or start one in the last Live mode.
            // Only the overlay is shown; the main window stays where it is.
            match ctx.is_recording().await {
                Some(m) if m != Mode::Dictation => {
                    if let Err(e) = ctx.stop().await {
                        ctx.emit_error("session", e);
                    }
                    ctx.hide_overlay_after_live();
                }
                _ => {
                    let mode = ctx.live_mode();
                    match ctx.start(mode).await {
                        Ok(()) => ctx.show_overlay(OverlayKind::Live),
                        Err(e) => {
                            ctx.emit_error("session", e);
                            ctx.show_overlay(OverlayKind::Live);
                        }
                    }
                }
            }
        } else if matches(&s.hotkey_dictate) {
            if ev.pressed {
                let age = match ctx.session_info().await {
                    Some((Mode::Dictation, age)) => Some(age),
                    _ => None,
                };
                match dictate_key.press(ev.at, age) {
                    Some(PressAction::Start) => {
                        ctx.remember_focus().await;
                        if let Err(e) = ctx.start(Mode::Dictation).await {
                            ctx.emit_error("dictation", e);
                        }
                        // Shown after the start so the pill never shows a stale phase.
                        // Showing does not take focus from the target app.
                        ctx.show_overlay(OverlayKind::Dictation);
                    }
                    Some(PressAction::Stop) => {
                        if let Err(e) = ctx.stop().await {
                            ctx.emit_error("dictation", e);
                        }
                    }
                    Some(PressAction::Cancel) => {
                        if let Err(e) = ctx.cancel_dictation().await {
                            ctx.emit_error("dictation", e);
                        }
                    }
                    None => {}
                }
            } else if dictate_key.release(ev.at) == ReleaseAction::Stop {
                // Push-to-talk: the key was held, so releasing it ends the dictation.
                if ctx.is_recording().await == Some(Mode::Dictation) {
                    if let Err(e) = ctx.stop().await {
                        ctx.emit_error("dictation", e);
                    }
                }
            }
        }
    }
}
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "relay=info,relay_lib=info".into()),
        )
        .try_init();
    // The Deepgram websocket uses rustls 0.23, which needs a process-wide crypto provider.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    on_shortcut(app, shortcut, event.state() == ShortcutState::Pressed);
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle().clone();
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            std::fs::create_dir_all(&data_dir)?;

            let loaded = settings::load(&config_dir);
            if let Err(e) = register_hotkeys(&handle, &loaded) {
                tracing::warn!("{e}");
            }
            let shared_settings = Arc::new(RwLock::new(loaded));
            let transcript: types::SharedTranscript = Arc::new(RwLock::new(Default::default()));

            let store = agent::TaskStore::open(&data_dir.join("relay.db"))?;
            let runtime = agent::AgentRuntime::new(
                handle.clone(),
                store,
                shared_settings.clone(),
                transcript.clone(),
            );
            let models_dir = stt::models::models_dir(&data_dir);
            let history = Arc::new(history::History::open(&data_dir));
            // History clips play in the UI through the asset protocol; allow only their folder.
            std::fs::create_dir_all(history.audio_dir())?;
            if let Err(e) = app.asset_protocol_scope().allow_directory(history.audio_dir(), false) {
                tracing::warn!("History audio will not play: {e}");
            }
            let ctx = AppCtx::new(handle.clone(), shared_settings, config_dir, models_dir, transcript, runtime, history);
            app.manage(ctx.clone());

            // Hotkey events are queued and handled in order by one task.
            let (hk_tx, hk_rx) = mpsc::unbounded_channel::<HotkeyEvent>();
            app.manage(HotkeyQueue(hk_tx));
            tauri::async_runtime::spawn(hotkey_loop(handle.clone(), ctx, hk_rx));

            // The overlay is shown during dictation without being activated, so the
            // app being dictated into keeps keyboard focus.
            if let Some(w) = app.get_webview_window("overlay") {
                let _ = w.set_focusable(false);
            }

            let overlay = MenuItem::with_id(app, "overlay", "Show or hide overlay", true, None::<&str>)?;
            let open = MenuItem::with_id(app, "open", "Open Relay", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Relay", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&overlay, &open, &quit])?;
            let mut tray = TrayIconBuilder::with_id("relay-tray")
                .tooltip("Relay")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "overlay" => toggle_overlay_window(app),
                    "open" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window keeps Relay running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::set_api_key,
            commands::has_api_key,
            commands::list_audio_devices,
            commands::start_session,
            commands::stop_session,
            commands::get_state,
            commands::get_transcript,
            commands::clear_transcript,
            commands::run_edit,
            commands::history_list,
            commands::history_audio_dir,
            commands::history_delete,
            commands::history_clear,
            commands::insert_text,
            commands::export_transcript,
            commands::save_export,
            commands::generate_notes,
            commands::agent_create_task,
            commands::agent_run_task,
            commands::agent_update_task,
            commands::agent_delete_task,
            commands::agent_plan_tasks,
            commands::agent_apply_plan,
            commands::list_models,
            commands::agent_list_tasks,
            commands::agent_cancel_task,
            commands::agent_resolve_approval,
            commands::agent_pending_approvals,
            commands::agent_reply_escalation,
            commands::whisper_status,
            commands::download_whisper_model,
            commands::cancel_whisper_download,
            commands::test_llm,
            commands::set_live_mode,
            commands::show_main,
            commands::toggle_overlay,
            commands::cancel_dictation,
            commands::skip_cleanup,
            commands::dismiss_dictation_error,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Relay");
}

