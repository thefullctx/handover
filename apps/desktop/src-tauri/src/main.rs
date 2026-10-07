//! Handover desktop app.
//!
//! The app embeds the daemon crate in-process (so the CLI can talk to it via
//! the local HTTP API), shows a tray/menu-bar icon, registers the global
//! hotkey and drives the command palette window.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use handover_core::action::{builtin_actions, Action};
use handover_core::agent::AgentMetaStatus;
use handover_core::capture::Capture;
use handover_core::session::LiveSession;
use handover_daemon::{
    api, capture as dcap, complete_approval, complete_send, compute_agents_status,
    compute_live_sessions, notify, shared, Daemon, SendOutcome, SharedDaemon,
};

/// Everything the palette needs to render in one shot.
#[derive(Clone, serde::Serialize)]
struct PalettePayload {
    actions: Vec<Action>,
    agents: Vec<AgentMetaStatus>,
    preferences: HashMap<String, String>,
    /// When true, selecting an action with a remembered preferred agent
    /// skips the agent picker (config: `general.quick_send`).
    quick_send: bool,
    /// The default agent (first enabled) — shown as a badge in the picker.
    default_agent_id: Option<String>,
    /// Platform config path for empty-state / error copy.
    config_path: String,
    /// The configured accelerator (e.g. `CmdOrCtrl+Shift+A`) — the palette
    /// shows the *real* shortcut in onboarding, the cheat sheet and empty
    /// states, not a hard-coded one.
    shortcut: String,
    /// Live agent sessions (freshest-first, per-session working/idle
    /// activity) — the status layer: the palette shows where the work is.
    sessions: Vec<LiveSession>,
}

/// The default hotkey: ⌘⇧A on macOS, Super+Shift+A on Linux.
fn hotkey() -> Shortcut {
    Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyA)
}

/// Parses the configured accelerator (e.g. `CmdOrCtrl+Shift+A`) into a
/// [`Shortcut`], falling back to the default hotkey when unparsable.
fn configured_hotkey(accelerator: &str) -> Shortcut {
    Shortcut::from_str(accelerator).unwrap_or_else(|_| hotkey())
}

/// Registers (or re-registers) the global hotkey with the palette handler.
///
/// Important: `on_shortcut` already registers with the OS — do **not** also
/// call `register()` or Carbon `RegisterEventHotKey` fails on the duplicate
/// (we used to show a misleading “grant Accessibility” banner for that).
fn install_hotkey(app: &AppHandle, shortcut: Shortcut) -> Result<Shortcut, String> {
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                open_palette(app);
            }
        })
        .map_err(|e| format!("could not register global shortcut: {e}"))?;
    Ok(shortcut)
}

/// Human-facing copy when global-shortcut registration fails.
fn hotkey_failure_message(err: &str) -> String {
    let lower = err.to_lowercase();
    if lower.contains("already") {
        return format!(
            "Global shortcut is already in use by another app ({err}). \
             Change it in Settings → General, or free the shortcut elsewhere."
        );
    }
    if lower.contains("access")
        || lower.contains("permission")
        || lower.contains("denied")
        || lower.contains("not authorized")
    {
        return format!(
            "Global shortcut unavailable ({err}). \
             System Settings → Privacy & Security → Accessibility: enable Handover, \
             then Quit and relaunch."
        );
    }
    format!("Global shortcut unavailable: {err}")
}

/// `0:45`-style duration for tray labels and history rows.
fn format_duration(ms: u64) -> String {
    let total = ms / 1000;
    format!("{}:{:02}", total / 60, total % 60)
}

fn main() {
    env_logger::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            // macOS: a menu-bar utility, no dock icon.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Embedded daemon (owns config, registry, capture, handoffs).
            let daemon = Daemon::load()
                .map_err(|e| std::io::Error::other(format!("daemon init failed: {e}")))?;
            let port = daemon.port;
            let shared_state = shared(daemon);
            let shutdown = Arc::new(AtomicBool::new(false));
            if let Err(e) = api::serve(shared_state.clone(), port, Arc::clone(&shutdown)) {
                log::warn!(
                    "{e} (the CLI will use the other instance — handoffs here won't appear in tray history)"
                );
            }
            // Persisted appearance + shortcut, captured before `shared_state`
            // is moved into app state (used to theme/register below).
            let (appearance, daemon_config_shortcut) = {
                let d = shared_state.lock();
                (d.config.general.appearance, d.config.general.shortcut.clone())
            };
            app.manage(shared_state);
            app.manage(PendingSettingsTab::default());
            app.manage(PaletteCompactHeight::default());

            // Settings is *not* created at launch — only when the user opens
            // it. That avoids a second WebKit process (and the system
            // "running in the background" noise) while the menu-bar app is idle.
            // open_settings() builds it on demand.

            // Palette window, hidden until the hotkey is pressed. Sized for
            // the glass card + floating dock. WebKit stays for this window
            // only — that is what powers the UI (cannot be removed).
            let _window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Handover")
                .inner_size(PALETTE_WIDTH, PALETTE_DEFAULT_COMPACT_HEIGHT)
                .resizable(false)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .transparent(true)
                .shadow(false)
                .center()
                .visible(false)
                .build()?;

            // Apply the persisted appearance to native chrome; frontend
            // mirrors it into CSS from `app_info` / `appearance:changed`.
            for (_, window) in app.webview_windows() {
                let _ = window.set_theme(theme_for(appearance));
            }

            // Tray / menu-bar icon: monochrome template derived from the H mark.
            // `icon_as_template(true)` lets macOS tint it for light/dark menu bars
            // (Dock/app icon stays the full-color metal asset in icons/icon.*).
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!(
                "../icons/trayTemplate@2x.png"
            ))
            .expect("embedded tray template icon");

            let _tray = TrayIconBuilder::with_id("main")
                .icon(tray_icon)
                .icon_as_template(true)
                .tooltip("Handover")
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quick_capture" => open_palette(app),
                    "settings" => open_settings(app, None),
                    "about_handover" => open_settings(app, Some("about")),
                    "recent_all" => open_history(app, None),
                    // Rows carry the outcome's stable id (e.g. `recent_handoff-3`),
                    // so clicks resolve correctly even if the history shifted
                    // since the menu was built.
                    id if id.starts_with("recent_") => {
                        open_history(app, Some(id.trim_start_matches("recent_").to_string()));
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            rebuild_tray_menu(app.handle());

            // macOS menu bar (the menu next to the Apple logo). The palette is
            // a borderless window, so this is where Settings… / About live —
            // the standard place users expect them.
            let handover_menu = SubmenuBuilder::new(app, "Handover")
                .item(
                    &MenuItemBuilder::with_id("about_handover", "About Handover")
                        .build(app)?,
                )
                .separator()
                .item(
                    &MenuItemBuilder::with_id("settings", "Settings…")
                        .accelerator("CmdOrCtrl+,")
                        .build(app)?,
                )
                .separator()
                .item(
                    &MenuItemBuilder::with_id("recent_handoffs", "Recent Handoffs…")
                        .build(app)?,
                )
                .separator()
                .item(&PredefinedMenuItem::quit(app, None)?)
                .build()?;

            let edit_menu = SubmenuBuilder::new(app, "Edit")
                .item(&PredefinedMenuItem::undo(app, None)?)
                .item(&PredefinedMenuItem::redo(app, None)?)
                .separator()
                .item(&PredefinedMenuItem::cut(app, None)?)
                .item(&PredefinedMenuItem::copy(app, None)?)
                .item(&PredefinedMenuItem::paste(app, None)?)
                .item(&PredefinedMenuItem::select_all(app, None)?)
                .build()?;

            let window_menu = SubmenuBuilder::new(app, "Window")
                .item(&PredefinedMenuItem::minimize(app, None)?)
                .item(&PredefinedMenuItem::close_window(app, None)?)
                .build()?;

            let menu = MenuBuilder::new(app)
                .item(&handover_menu)
                .item(&edit_menu)
                .item(&window_menu)
                .build()?;
            app.set_menu(menu)?;

            app.on_menu_event(|app, event| match event.id().as_ref() {
                "about_handover" => open_settings(app, Some("about")),
                "settings" => open_settings(app, None),
                "recent_handoffs" => open_history(app, None),
                _ => {}
            });

            // Global hotkey from the configurable accelerator (default ⌘⇧A).
            // Ad-hoc rebuilds often need Accessibility re-enabled for the new binary.
            let shortcut = configured_hotkey(&daemon_config_shortcut);
            match install_hotkey(app.handle(), shortcut) {
                Ok(s) => log::info!("global shortcut registered: {s}"),
                Err(e) => {
                    log::error!("could not register global shortcut: {e}");
                    native_notify(
                        app.handle(),
                        "Handover",
                        &hotkey_failure_message(&e),
                        true, // critical — never suppressed by the notifications toggle
                    );
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            palette_payload,
            read_dropped_files,
            list_actions,
            list_agents,
            configured_agents,
            add_agent,
            remove_agent,
            app_info,
            set_quick_send,
            set_appearance,
            set_ui_opacity,
            set_shortcut,
            set_launch_at_startup,
            set_notifications,
            set_excluded_paths,
            clear_history,
            take_settings_tab,
            available_agents,
            configure_agent,
            set_default_agent,
            set_agent_enabled,
            open_settings_window,
            reveal_config,
            live_sessions,
            approve_session,
            send_handoff,
            recent_handoffs,
            set_preference,
            copy_text,
            open_url,
            set_palette_height,
            hide_palette,
            notify_result
        ])
        // Closing Settings destroys the window (frees its WebKit helpers) instead
        // of leaving a hidden second webview alive in the background.
        .on_window_event(|window, event| {
            if window.label() == "settings" {
                if let tauri::WindowEvent::CloseRequested { .. } = event {
                    // Allow the default close → Destroyed; next open rebuilds.
                    log::debug!("settings window close requested — webview will be destroyed");
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Handover");
}

/// Re-broadcasts the persisted live settings (window opacity + appearance) to
/// every webview. The palette and Settings windows keep their CSS variables
/// in memory for the process lifetime and normally stay in sync via the
/// `ui-opacity:changed` / `appearance:changed` events — but a missed event
/// (hidden window, boot race) would leave a window showing a stale value.
/// Re-emitting on every open makes both windows resync from the source of
/// truth, so the palette always matches the Settings slider.
fn sync_live_settings(app: &AppHandle) {
    let state = app.state::<SharedDaemon>();
    let daemon = state.lock();
    let _ = app.emit(
        "ui-opacity:changed",
        handover_config::clamp_ui_opacity(daemon.config.general.ui_opacity),
    );
    let _ = app.emit(
        "appearance:changed",
        daemon.config.general.appearance.as_str(),
    );
}

/// Shows + focuses the palette window and tells the frontend to refresh.
/// Opens clean — no clipboard capture (removed by request).
fn open_palette(app: &AppHandle) {
    // The palette opens clean — no clipboard capture (removed by request).
    // The palette is a long-running (hidden) window — resync opacity/appearance
    // so it always reflects the current Settings values when shown.
    sync_live_settings(app);
    let _ = app.emit("palette:open", ());
    if let Some(window) = app.get_webview_window("main") {
        // The palette always reopens on the compact picker; size it before
        // showing so it never flashes the previous (expanded) size.
        let height = app
            .try_state::<PaletteCompactHeight>()
            .map(|h| *h.0.lock().unwrap_or_else(|e| e.into_inner()))
            .unwrap_or(PALETTE_DEFAULT_COMPACT_HEIGHT);
        let _ = window.set_size(tauri::LogicalSize::new(PALETTE_WIDTH, height));
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.unminimize();
    }
}

/// Opens the palette in the history view — focused on one handoff when a
/// stable id is given (tray rows), otherwise the plain list ("Show all…").
fn open_history(app: &AppHandle, handoff_id: Option<String>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.unminimize();
    }
    match handoff_id {
        Some(id) => {
            let _ = app.emit("palette:handoff", id);
        }
        None => {
            let _ = app.emit("palette:history", ());
        }
    }
}

/// Rebuilds the tray menu so "Recent Handoffs" matches the session history.
/// Called at startup and after every completed handoff (the menu is static
/// between handoffs, so the index in each row id stays valid).
fn rebuild_tray_menu(app: &AppHandle) {
    let recent = app.state::<SharedDaemon>().lock().history.recent();

    let title =
        MenuItem::with_id(app, "title", "Handover", false, None::<&str>).expect("title item");
    let quick = MenuItem::with_id(app, "quick_capture", "Open Handover", true, None::<&str>)
        .expect("quick item");
    let sep = PredefinedMenuItem::separator(app).expect("separator");
    let settings =
        MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>).expect("settings item");
    let about = MenuItem::with_id(app, "about_handover", "About Handover…", true, None::<&str>)
        .expect("about item");
    let quit =
        MenuItem::with_id(app, "quit", "Quit Handover", true, None::<&str>).expect("quit item");

    let mut recent_items: Vec<Box<dyn IsMenuItem<tauri::Wry>>> = if recent.is_empty() {
        vec![Box::new(
            MenuItem::with_id(app, "recent_none", "No handoffs yet", false, None::<&str>)
                .expect("recent item"),
        )]
    } else {
        recent
            .iter()
            .map(|o| {
                let dur = o
                    .receipt
                    .as_ref()
                    .map(|r| format_duration(r.duration_ms))
                    .unwrap_or_default();
                let status = if o.ok { "✓" } else { "✕" };
                let label = format!("{dur} · {} · {} {status}", o.agent_name, o.action_id);
                Box::new(
                    MenuItem::with_id(app, format!("recent_{}", o.id), label, true, None::<&str>)
                        .expect("recent item"),
                ) as Box<dyn IsMenuItem<tauri::Wry>>
            })
            .collect()
    };
    recent_items.push(Box::new(
        MenuItem::with_id(app, "recent_all", "Show all…", true, None::<&str>)
            .expect("show all item"),
    ));
    let submenu_refs: Vec<&dyn IsMenuItem<tauri::Wry>> =
        recent_items.iter().map(|b| b.as_ref()).collect();
    let recent_sub =
        Submenu::with_items(app, "Recent Handoffs", true, &submenu_refs).expect("recent submenu");

    let top_refs: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![
        &title,
        &quick,
        &sep,
        &recent_sub,
        &sep,
        &settings,
        &about,
        &sep,
        &quit,
    ];
    let menu = Menu::with_items(app, &top_refs).expect("tray menu");
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_menu(Some(menu));
    }
}

// ---------------------------------------------------------------------------
// Commands (called from the React palette)
// ---------------------------------------------------------------------------

#[tauri::command]
async fn palette_payload(state: tauri::State<'_, SharedDaemon>) -> Result<PalettePayload, String> {
    // Cheap half under one brief lock: registry detection (no subprocesses),
    // config reads, snapshots. The probe half — live-process checks (pgrep/ps
    // subprocesses), provider-health probes, glob walks, cli-list subprocesses
    // — runs in spawn_blocking so a slow agent (a hung cli-list has a 5s
    // budget) can never block a tokio worker thread and delay palette opens.
    let daemon = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (agents_snapshot, sessions_snapshot, payload) = {
            let daemon = daemon.lock();
            (
                daemon.agents_status_snapshot(),
                daemon.live_sessions_snapshot(),
                PalettePayload {
                    actions: builtin_actions(),
                    agents: Vec::new(),
                    preferences: daemon.config.preferences.clone(),
                    quick_send: daemon.config.general.quick_send,
                    default_agent_id: daemon.registry.default_id(),
                    config_path: daemon.config_path.display().to_string(),
                    shortcut: daemon.config.general.shortcut.clone(),
                    sessions: Vec::new(),
                },
            )
        };
        Ok(PalettePayload {
            agents: compute_agents_status(agents_snapshot),
            sessions: compute_live_sessions(&sessions_snapshot),
            ..payload
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Live agent sessions for the Settings → Agents "Detect session" preview.
/// Runs off the command thread: cli-list agents (Hermes) spawn a process.
#[tauri::command]
async fn live_sessions(state: tauri::State<'_, SharedDaemon>) -> Result<Vec<LiveSession>, String> {
    let daemon = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Snapshot under the lock (cheap config reads), scan off it: glob
        // walks and cli-list subprocesses must not block other commands.
        let snapshot = {
            let d = daemon.lock();
            d.live_sessions_snapshot()
        };
        Ok(compute_live_sessions(&snapshot))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Approves or denies a blocked session (Phase 6.5, opt-in per agent). The
/// daemon re-checks the session is still blocked, injects via the configured
/// channel, and verifies the transcript resumes — fail soft, never a false
/// success. Runs off the command thread (injection + verify polling).
///
/// Two-phase: plan under the daemon lock (fast — config validation),
/// complete + execute without it (slow — liveness re-check, tail peek,
/// injection + up to 10s verify polling). The lock must not be held during
/// the scan or polling, or every other API call, palette open, and handoff
/// in flight would block.
#[tauri::command]
async fn approve_session(
    state: tauri::State<'_, SharedDaemon>,
    agent_id: String,
    session_id: String,
    approve: bool,
) -> Result<handover_daemon::approval::ApprovalResult, String> {
    let daemon = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Phase 1: plan under the lock (fast — config validation only).
        let plan = {
            let d = daemon.lock();
            d.resolve_approval_plan(&agent_id, &session_id, approve)
                .map_err(|e| e.to_string())?
        };
        // Phase 2: complete + execute without the lock (slow — session scan,
        // injection + verify polling).
        let req = complete_approval(plan).map_err(|e| e.to_string())?;
        handover_daemon::execute_approval(&req, handover_daemon::approval::APPROVAL_VERIFY_BUDGET)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A file dropped onto the palette, read for handoff.
///
/// `kind` is `text` (read into `text`), `image` (recognized by extension —
/// the frontend hands off its path so path-aware agents can open it), or
/// `other` (binary / too large / unreadable — `note` explains why).
#[derive(Clone, serde::Serialize)]
struct DroppedFile {
    path: String,
    name: String,
    size: u64,
    kind: String,
    text: Option<String>,
    note: Option<String>,
}

/// The capture size cap for dropped text, matching the daemon's clipboard cap.
const DROP_TEXT_MAX_BYTES: u64 = 1024 * 1024;

/// Reads files dragged onto the palette. The webview can't open local paths,
/// so the frontend forwards the `tauri://drag-drop` paths here.
///
/// Privacy discipline is IDENTICAL to daemon file capture: the exclusion list
/// is checked BEFORE anything is opened, and the read itself goes through
/// `handover_daemon::read_file_nofollow` (leaf symlinks refused, size-capped
/// on the same handle). Dropping a `.env`, `id_rsa`, or a symlink to either is
/// refused with a plain-language note — never read into the composer.
#[tauri::command]
fn read_dropped_files(
    state: tauri::State<SharedDaemon>,
    paths: Vec<String>,
) -> Result<Vec<DroppedFile>, String> {
    let patterns = state.lock().config.privacy.excluded_paths.clone();
    let mut out = Vec::new();
    for p in paths {
        let path = std::path::PathBuf::from(&p);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("dropped file")
            .to_string();
        // 1. Exclusion check first — refuse before ANY read, matching the
        //    documented privacy rule (case-insensitive matching inside).
        if handover_core::exclusions::is_excluded_path(&path, &patterns) {
            out.push(DroppedFile {
                path: p.clone(),
                name,
                size: 0,
                kind: "other".into(),
                text: None,
                note: Some(format!(
                    "refused — `{}` matches your excluded paths (Privacy)",
                    p
                )),
            });
            continue;
        }
        // 2. Symlink-safe, size-capped read via the shared daemon reader.
        match handover_daemon::read_file_nofollow(&path, DROP_TEXT_MAX_BYTES) {
            Err(e) => {
                out.push(DroppedFile {
                    path: p,
                    name,
                    size: 0,
                    kind: "other".into(),
                    text: None,
                    note: Some(e),
                });
            }
            Ok((meta, bytes)) => {
                let size = meta.len();
                if !meta.is_file() {
                    out.push(DroppedFile {
                        path: p,
                        name,
                        size,
                        kind: "other".into(),
                        text: None,
                        note: Some("not a file".into()),
                    });
                    continue;
                }
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase());
                let is_image = matches!(
                    ext.as_deref(),
                    Some(
                        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "tiff" | "bmp"
                    )
                );
                if is_image {
                    out.push(DroppedFile {
                        path: p,
                        name,
                        size,
                        kind: "image".into(),
                        text: None,
                        note: None,
                    });
                    continue;
                }
                // `None` body = over the size cap (reader returns meta only).
                let Some(bytes) = bytes else {
                    out.push(DroppedFile {
                        path: p,
                        name,
                        size,
                        kind: "other".into(),
                        text: None,
                        note: Some("larger than 1 MB — not read".into()),
                    });
                    continue;
                };
                // Binary sniff: refuse files with NUL bytes so we never hand a
                // wall of garbage to an agent.
                if bytes.contains(&0) {
                    out.push(DroppedFile {
                        path: p,
                        name,
                        size,
                        kind: "other".into(),
                        text: None,
                        note: Some("binary file — not read".into()),
                    });
                    continue;
                }
                out.push(DroppedFile {
                    path: p,
                    name,
                    size,
                    kind: "text".into(),
                    text: Some(String::from_utf8_lossy(&bytes).into_owned()),
                    note: None,
                });
            }
        }
    }
    Ok(out)
}

#[tauri::command]
fn list_actions() -> Vec<Action> {
    builtin_actions()
}

#[tauri::command]
async fn list_agents(
    state: tauri::State<'_, SharedDaemon>,
) -> Result<Vec<AgentMetaStatus>, String> {
    let daemon = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Snapshot under the lock (cheap), probe off it: the live-process
        // check + provider-health probe must not block other commands.
        let snapshot = {
            let d = daemon.lock();
            d.agents_status_snapshot()
        };
        Ok(compute_agents_status(snapshot))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// All configured agents (including disabled ones) for the management UI.
#[tauri::command]
fn configured_agents(state: tauri::State<SharedDaemon>) -> Vec<handover_config::AgentConfig> {
    state.lock().configured_agents()
}

/// Adds an agent from the palette's management form. Persists to config.toml
/// and makes it available immediately (no restart).
#[tauri::command]
#[allow(clippy::too_many_arguments)] // mirrors the AgentConfig fields 1:1
fn add_agent(
    state: tauri::State<SharedDaemon>,
    id: String,
    name: String,
    command: String,
    description: Option<String>,
    working_dir: Option<String>,
    timeout_secs: Option<u64>,
    enabled: bool,
) -> Result<(), String> {
    let agent = handover_config::AgentConfig {
        id,
        name,
        kind: handover_config::AgentKind::Command,
        command,
        description,
        working_dir,
        env: Default::default(),
        timeout_secs,
        enabled,
        demo: false,
        default_action: None,
        session_glob: None,
        session_cli_list: None,
        resume_command: None,
        permission_marker: None,
        approval_channel: None,
        approval_target: None,
    };
    state.lock().add_agent(agent).map_err(|e| e.to_string())
}

/// Removes a configured agent by id. Persists the config and forgets any
/// per-action preferences that pointed at it.
#[tauri::command]
fn remove_agent(state: tauri::State<SharedDaemon>, id: String) -> Result<(), String> {
    state.lock().remove_agent(&id).map_err(|e| e.to_string())
}

/// Shared config for the Settings window (built hidden; opened on demand).
///
/// The title bar is hidden in favor of `TitleBarStyle::Overlay`: the traffic
/// lights stay native (red/yellow/green, drawn over the sidebar), while the
/// frontend provides the top drag strip — the macOS pattern of the window
/// chrome being part of the UI. Requires the `macos-private-api` feature,
/// which is enabled in Cargo.toml.
fn settings_window_builder(
    app: &AppHandle,
) -> WebviewWindowBuilder<'_, tauri::Wry, tauri::AppHandle<tauri::Wry>> {
    // Empty title: Overlay title bar still shows traffic lights, but no
    // "Handover Settings" caption in the top-left (brand lives in the sidebar).
    // Transparent so `--ui-opacity` glass fills match the palette window.
    let builder = WebviewWindowBuilder::new(app, "settings", WebviewUrl::default())
        .title("")
        .inner_size(620.0, 680.0)
        .min_inner_size(480.0, 520.0)
        .resizable(true)
        .decorations(true)
        .transparent(true)
        .visible(false);
    // Overlay title bar is a macOS-only Tauri API — the method does not exist
    // on other targets, so it cannot sit in the chain unconditionally. Linux
    // and Windows keep standard decorations (see reveal_config's platform
    // handling for the same shape of split).
    #[cfg(target_os = "macos")]
    let builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay);
    builder
}

/// Shows (and creates, if needed) the Settings window, optionally switching
/// to a specific tab ("agents" | "general" | "about").
fn open_settings(app: &AppHandle, tab: Option<&str>) {
    // Record the requested tab first so a fast open (window still loading,
    // or created on demand) cannot lose the request to an event race.
    if let Some(tab) = tab {
        if let Some(pending) = app.try_state::<PendingSettingsTab>() {
            *pending.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(tab.to_string());
        }
    }
    let window = match app.get_webview_window("settings") {
        Some(w) => w,
        None => match settings_window_builder(app).build() {
            Ok(w) => {
                // Match palette appearance for the newly created chrome.
                let state = app.state::<SharedDaemon>();
                let daemon = state.lock();
                let _ = w.set_theme(theme_for(daemon.config.general.appearance));
                w
            }
            Err(e) => {
                log::error!("could not create settings window: {e}");
                return;
            }
        },
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
    // Resync from the source of truth in case this window predates a change
    // made elsewhere (e.g. an edited config), so the slider always reflects
    // the same value the palette is showing.
    sync_live_settings(app);
    if let Some(tab) = tab {
        let _ = app.emit_to("settings", "settings:tab", tab);
    }
}

/// A tab requested by the app menu / tray before the Settings window's
/// frontend was ready (e.g. "About Handover" while the window loads). The
/// frontend pulls it once on mount and clears it, so the request is never
/// lost to an event race.
#[derive(Default)]
struct PendingSettingsTab(Mutex<Option<String>>);

/// Palette window width (points). The height follows the UI: compact on the
/// plain picker, expanded once the agent menu, a chat or a panel needs room.
const PALETTE_WIDTH: f64 = 520.0;
/// Height bounds the frontend may request, so a bad measurement can never
/// produce an unusable window.
const PALETTE_MIN_HEIGHT: f64 = 120.0;
const PALETTE_MAX_HEIGHT: f64 = 900.0;
/// Before the frontend has measured anything (first open).
const PALETTE_DEFAULT_COMPACT_HEIGHT: f64 = 210.0;

/// The last compact height the palette reported. Applied before the window is
/// shown, so it opens at its compact size instead of flashing a taller one.
struct PaletteCompactHeight(Mutex<f64>);

impl Default for PaletteCompactHeight {
    fn default() -> Self {
        Self(Mutex::new(PALETTE_DEFAULT_COMPACT_HEIGHT))
    }
}

/// Clamps a requested palette height to the allowed range (NaN → default).
fn clamp_palette_height(height: f64) -> f64 {
    if height.is_finite() {
        height.clamp(PALETTE_MIN_HEIGHT, PALETTE_MAX_HEIGHT)
    } else {
        PALETTE_DEFAULT_COMPACT_HEIGHT
    }
}

/// Resizes the palette window to `height` (width fixed). `compact` marks the
/// picker-only size, remembered for the next open.
#[tauri::command]
fn set_palette_height(
    app: AppHandle,
    remembered: tauri::State<PaletteCompactHeight>,
    height: f64,
    compact: bool,
) -> Result<(), String> {
    let height = clamp_palette_height(height);
    if compact {
        *remembered.0.lock().unwrap_or_else(|e| e.into_inner()) = height;
    }
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "palette window not found".to_string())?;
    window
        .set_size(tauri::LogicalSize::new(PALETTE_WIDTH, height))
        .map_err(|e| e.to_string())
}

/// App metadata for the Settings window (About / General / Privacy tabs).
#[derive(Clone, serde::Serialize)]
struct AppInfo {
    version: String,
    config_path: String,
    /// The configured accelerator, e.g. `CmdOrCtrl+Shift+A`.
    shortcut: String,
    quick_send: bool,
    notifications: bool,
    /// Whether the app actually launches at startup (from the autostart
    /// plugin — the source of truth once the user has toggled it).
    launch_at_startup: bool,
    /// "system" | "light" | "dark" — the persisted UI appearance.
    appearance: String,
    /// Palette + Settings surface opacity (0.4–1.0).
    ui_opacity: f64,
    /// The privacy excluded-path globs (Settings → Privacy editor).
    excluded_paths: Vec<String>,
}

#[tauri::command]
fn app_info(app: AppHandle, state: tauri::State<SharedDaemon>) -> AppInfo {
    let daemon = state.lock();
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        config_path: daemon.config_path.display().to_string(),
        shortcut: daemon.config.general.shortcut.clone(),
        quick_send: daemon.config.general.quick_send,
        notifications: daemon.config.general.notifications,
        launch_at_startup: app.autolaunch().is_enabled().unwrap_or(false),
        appearance: daemon.config.general.appearance.as_str().to_string(),
        ui_opacity: handover_config::clamp_ui_opacity(daemon.config.general.ui_opacity),
        excluded_paths: daemon.config.privacy.excluded_paths.clone(),
    }
}

/// Persists the quick-send toggle from the Settings window.
#[tauri::command]
fn set_quick_send(state: tauri::State<SharedDaemon>, enabled: bool) -> Result<(), String> {
    state.lock().set_quick_send(enabled);
    Ok(())
}

/// Persists the notifications toggle from the Settings window.
#[tauri::command]
fn set_notifications(state: tauri::State<SharedDaemon>, enabled: bool) -> Result<(), String> {
    state.lock().set_notifications(enabled);
    Ok(())
}

/// Changes the global hotkey: persists the accelerator, unregisters the old
/// shortcut and registers the new one. On registration failure the old
/// shortcut is restored so the app never ends up without a hotkey.
#[tauri::command]
fn set_shortcut(
    app: AppHandle,
    state: tauri::State<SharedDaemon>,
    shortcut: String,
) -> Result<(), String> {
    let parsed = Shortcut::from_str(&shortcut)
        .map_err(|e| format!("`{shortcut}` is not a valid shortcut ({e})"))?;
    let old = {
        let daemon = state.lock();
        daemon.config.general.shortcut.clone()
        // NOTE: nothing is persisted here. The config is only updated AFTER
        // the new accelerator registers successfully — otherwise a failed
        // registration would leave a broken shortcut in config.toml and the
        // next launch could come up without a global hotkey.
    };
    if old == shortcut {
        return Ok(()); // same shortcut — nothing to re-register
    }
    // Unregister the old binding first, then install the new one; restore the
    // old on failure so the hotkey is never lost.
    let old_parsed = configured_hotkey(&old);
    let _ = app.global_shortcut().unregister(old_parsed);
    if let Err(e) = install_hotkey(&app, parsed) {
        let _ = install_hotkey(&app, configured_hotkey(&old));
        return Err(e);
    }
    // Registration succeeded — only now persist.
    state.lock().set_shortcut(&shortcut);
    Ok(())
}

/// Toggles launch-at-startup via the autostart plugin and mirrors the value
/// into the config for reference.
#[tauri::command]
fn set_launch_at_startup(
    app: AppHandle,
    state: tauri::State<SharedDaemon>,
    enabled: bool,
) -> Result<(), String> {
    if enabled {
        app.autolaunch()
            .enable()
            .map_err(|e| format!("could not enable launch at startup: {e}"))?;
    } else {
        app.autolaunch()
            .disable()
            .map_err(|e| format!("could not disable launch at startup: {e}"))?;
    }
    state.lock().set_launch_at_startup(enabled);
    Ok(())
}

/// Persists the privacy excluded-path glob list (Settings → Privacy).
#[tauri::command]
fn set_excluded_paths(
    state: tauri::State<SharedDaemon>,
    patterns: Vec<String>,
) -> Result<(), String> {
    state.lock().set_excluded_paths(patterns);
    Ok(())
}

/// Clears the session handoff history (Settings → Privacy / palette).
#[tauri::command]
fn clear_history(state: tauri::State<SharedDaemon>) -> Result<(), String> {
    state.lock().clear_history();
    Ok(())
}

/// Maps the persisted appearance to a Tauri window theme (`None` = follow
/// the system).
fn theme_for(appearance: handover_config::Appearance) -> Option<tauri::Theme> {
    match appearance {
        handover_config::Appearance::System => None,
        handover_config::Appearance::Light => Some(tauri::Theme::Light),
        handover_config::Appearance::Dark => Some(tauri::Theme::Dark),
    }
}

/// Persists the UI appearance (system/light/dark) and applies it to every
/// window immediately: native chrome via `set_theme`, CSS via the emitted
/// event the frontend listens for.
#[tauri::command]
fn set_appearance(
    app: AppHandle,
    state: tauri::State<SharedDaemon>,
    appearance: String,
) -> Result<(), String> {
    // Parse via `Appearance::from_str` — the single source for the
    // value↔enum mapping (mirrors `as_str`, used by `app_info`).
    let parsed: handover_config::Appearance = appearance.parse()?;
    state.lock().set_appearance(parsed);
    for (_, window) in app.webview_windows() {
        let _ = window.set_theme(theme_for(parsed));
    }
    // Emit the normalized value so listeners always see `system|light|dark`.
    let _ = app.emit("appearance:changed", parsed.as_str());
    Ok(())
}

/// Persists UI surface opacity (0.4–1.0) for palette + Settings and broadcasts
/// so every webview updates `--ui-opacity` live.
#[tauri::command]
fn set_ui_opacity(
    app: AppHandle,
    state: tauri::State<SharedDaemon>,
    opacity: f64,
) -> Result<f64, String> {
    let clamped = handover_config::clamp_ui_opacity(opacity);
    state.lock().set_ui_opacity(clamped);
    let _ = app.emit("ui-opacity:changed", clamped);
    Ok(clamped)
}

/// One-shot read of the pending Settings tab (see `PendingSettingsTab`).
#[tauri::command]
fn take_settings_tab(state: tauri::State<PendingSettingsTab>) -> Option<String> {
    state.0.lock().ok().and_then(|mut p| p.take())
}

/// Curated agent gallery: known agents with binary detection + config state,
/// for the Settings → Agents tab.
#[tauri::command]
fn available_agents(
    state: tauri::State<SharedDaemon>,
) -> Vec<handover_daemon::catalog::GalleryAgent> {
    let daemon = state.lock();
    let configured: Vec<String> = daemon.config.agents.iter().map(|a| a.id.clone()).collect();
    handover_daemon::catalog::gallery(&configured)
}

/// Replaces a leading bare `<binary_name>` token in a command with the
/// resolved binary path (e.g. `codex exec …` → `/path/to/codex exec …`).
/// Used to pre-fill session-aware `resume_command`s from the builtin catalog
/// with the user's actual binary location.
fn substitute_binary(cmd: &str, binary_name: &str, bin: &str) -> String {
    match cmd.strip_prefix(binary_name) {
        Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => {
            format!("{bin}{rest}")
        }
        _ => cmd.to_string(),
    }
}

/// Adds a catalog agent to the config, auto-writing the recommended command.
/// `binary_path` is the user's one-time input when the binary was not detected
/// (a path or a bare name; `~` is expanded). Persists immediately.
///
/// Catalog entries with a session-aware counterpart in the builtin catalog
/// (claude/codex/droid/omp/hermes) are written as session agents with the
/// resume command + session discovery pre-filled — adding one via the gallery
/// makes it session-aware immediately, never silently command-only.
#[tauri::command]
fn configure_agent(
    state: tauri::State<SharedDaemon>,
    id: String,
    binary_path: Option<String>,
) -> Result<handover_config::AgentConfig, String> {
    let entry = handover_daemon::catalog::agent_catalog()
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| format!("Unknown agent `{id}`."))?;
    // An explicit user-supplied path must resolve exactly — never silently
    // substitute the detected binary when the user's input was wrong. Without
    // a hint we fall back to detection (candidate paths + PATH).
    let bin = match binary_path.as_deref() {
        Some(hint) if !hint.trim().is_empty() => {
            handover_daemon::catalog::resolve_explicit(hint.trim()).ok_or_else(|| {
                format!(
                    "Could not find the `{}` binary at `{}`.",
                    entry.name,
                    hint.trim()
                )
            })?
        }
        _ => handover_daemon::catalog::resolve_binary(&entry, None).ok_or_else(|| {
            format!(
                "Could not find the `{}` binary. Enter its path (e.g. `~/.local/bin/{}`).",
                entry.name, entry.binary_name
            )
        })?,
    };
    let bin_str = bin.to_string_lossy().to_string();
    let command = handover_daemon::catalog::build_command(&entry.command_template, &bin_str);
    // Built-in session defaults (Phase 0 table): pre-fill the discovery
    // mechanism + resume command so gallery-added agents are session-aware.
    let session_defaults = handover_config::builtin_session_agents()
        .into_iter()
        .find(|a| a.id == entry.id);
    let agent = handover_config::AgentConfig {
        id: entry.id.clone(),
        name: entry.name.clone(),
        kind: if session_defaults.is_some() {
            handover_config::AgentKind::Session
        } else {
            handover_config::AgentKind::Command
        },
        command,
        description: Some(entry.description.clone()),
        working_dir: std::env::var("HOME").ok(),
        env: Default::default(),
        timeout_secs: Some(entry.timeout_secs),
        enabled: true,
        demo: false,
        default_action: None,
        session_glob: session_defaults
            .as_ref()
            .and_then(|s| s.session_glob.clone()),
        session_cli_list: session_defaults
            .as_ref()
            .and_then(|s| s.session_cli_list.clone()),
        resume_command: session_defaults.as_ref().and_then(|s| {
            s.resume_command
                .as_ref()
                .map(|rc| substitute_binary(rc, &entry.binary_name, &bin_str))
        }),
        permission_marker: None,
        approval_channel: None,
        approval_target: None,
    };
    state
        .lock()
        .add_agent(agent.clone())
        .map_err(|e| e.to_string())?;
    Ok(agent)
}

/// Makes `id` the default agent (first enabled), persisting the reorder.
#[tauri::command]
fn set_default_agent(state: tauri::State<SharedDaemon>, id: String) -> Result<(), String> {
    state
        .lock()
        .set_default_agent(&id)
        .map_err(|e| e.to_string())
}

/// Enables or disables a configured agent. Disabled agents remain configured
/// but disappear from the palette dropdown (a visibility switch, not a delete).
#[tauri::command]
fn set_agent_enabled(
    state: tauri::State<SharedDaemon>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .lock()
        .set_agent_enabled(&id, enabled)
        .map_err(|e| e.to_string())
}

/// Opens the Settings window (invoked by the palette's gear button).
#[tauri::command]
fn open_settings_window(app: AppHandle) {
    open_settings(&app, None);
}

/// Reveals the config file in the platform file manager.
#[tauri::command]
fn reveal_config(state: tauri::State<SharedDaemon>) -> Result<(), String> {
    let path = state.lock().config_path.clone();
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("could not reveal config: {e}"))?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .arg("/select,")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("could not reveal config: {e}"))?;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let dir = path.parent().unwrap_or(&path);
        std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map_err(|e| format!("could not reveal config: {e}"))?;
    }
    Ok(())
}

/// Run the agent handoff off the UI/command thread.
///
/// A synchronous command would freeze the palette for the full agent runtime
/// (Hermes/Codex can take minutes). Plan under the daemon lock (fast —
/// capture enrichment + config resolution), complete + `spawn_blocking` for
/// the provider probe, session scan and process wait. The completed outcome
/// is recorded into the session history and the tray menu is rebuilt.
#[tauri::command]
async fn send_handoff(
    app: AppHandle,
    state: tauri::State<'_, SharedDaemon>,
    action_id: String,
    agent_id: Option<String>,
    capture: Option<Capture>,
    session_id: Option<String>,
) -> Result<SendOutcome, String> {
    let capture =
        capture.ok_or_else(|| "No capture available. Copy something first.".to_string())?;
    let (agent, request) = {
        // Plan under the lock (fast), complete off it (slow — provider
        // probe, session scan). Holding the lock across the scan would block
        // every other command.
        let plan = {
            let daemon = state.lock();
            daemon
                .resolve_send_plan(
                    &action_id,
                    agent_id.as_deref(),
                    session_id.as_deref(),
                    capture,
                )
                .map_err(|e| e.to_string())?
        };
        complete_send(plan).map_err(|e| e.to_string())?
    };
    // Stable id known before execution so streamed events can be correlated
    // with the eventual outcome by the palette.
    let handoff_id = handover_daemon::next_handoff_id();
    // Live output: forward every stdout/stderr chunk to the palette as it
    // arrives (from the agent's drain threads), tagged with the handoff id.
    // `app` is Send + Sync, so emitting from the drain threads is safe.
    let stream_app = app.clone();
    let stream_id = handoff_id.clone();
    let stream: Option<handover_core::agent::OutputSink> = Some(Arc::new(move |channel, chunk| {
        let channel = match channel {
            handover_core::agent::OutputChannel::Stdout => "stdout",
            handover_core::agent::OutputChannel::Stderr => "stderr",
        };
        let _ = stream_app.emit(
            "handoff:output",
            serde_json::json!({ "id": stream_id, "stream": channel, "chunk": chunk }),
        );
    }));

    // The palette's activity section shows the *rendered* prompt size as a
    // real measurement (the prompt is built here, in Rust — the frontend
    // never guesses). Byte length (`len()`), so the frontend's "KB" label is
    // honest for non-ASCII text too. Emitted before execution so the stat is
    // there from the first frame of the sending screen.
    let _ = app.emit(
        "handoff:started",
        serde_json::json!({
            "id": handoff_id,
            "prompt_len": request.prompt.len(),
        }),
    );

    let outcome = tauri::async_runtime::spawn_blocking(move || {
        handover_daemon::execute_handoff(agent, request, stream, handoff_id)
    })
    .await
    .map_err(|e| format!("handoff task failed: {e}"))?;

    // Session history + tray submenu (both take their own locks — quick).
    state.lock().history.record(outcome.clone());
    rebuild_tray_menu(&app);
    Ok(outcome)
}

#[tauri::command]
fn recent_handoffs(state: tauri::State<SharedDaemon>) -> Vec<SendOutcome> {
    state.lock().history.recent()
}

#[tauri::command]
fn set_preference(state: tauri::State<SharedDaemon>, action_id: String, agent_id: String) {
    state.lock().set_preference(&action_id, &agent_id);
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    dcap::set_clipboard_text(&text).map_err(|e| e.to_string())
}

/// True for a link a chat reply may open: plain `http(s)://` only, with no
/// whitespace or control characters. Never `file:`, `javascript:` or app
/// schemes — the text comes from agent output, not from the user.
fn is_openable_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    matches!(rest, Some(r) if !r.is_empty())
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Opens a link from a chat reply in the default browser. Called only from an
/// explicit click; the URL is passed as one argument, never through a shell.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    let url = url.trim();
    if !is_openable_url(url) {
        return Err("Only web links (http or https) can be opened.".to_string());
    }
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    cmd.arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open the link: {e}"))
}

#[tauri::command]
fn hide_palette(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

#[tauri::command]
fn notify_result(app: AppHandle, title: String, body: String) {
    // Handoff-milestone notifications honor the General → Notifications toggle.
    native_notify(&app, &title, &body, false);
}

/// Posts a desktop notification. Prefers native macOS notifications (Tauri
/// notification plugin → UNUserNotificationCenter), so the banner belongs to
/// Handover and clicking it activates the app — not Script Editor (which is
/// what `osascript display notification` gets attributed to).
///
/// `critical` notifications (shortcut registration failures, permission
/// warnings) bypass the General → Notifications toggle — the user needs to
/// know even when banners are off.
///
/// Falls back to the daemon's `osascript`/`notify-send` path when the user
/// denied notification permission or the native show failed — better than
/// no notification at all.
fn native_notify(app: &AppHandle, title: &str, body: &str, critical: bool) {
    if !critical {
        // Honor the General → Notifications toggle: when disabled, handoff
        // notifications are suppressed (critical errors still log).
        if let Some(state) = app.try_state::<SharedDaemon>() {
            if !state.lock().config.general.notifications {
                log::info!("notification suppressed (General → Notifications is off)");
                return;
            }
        }
    }
    use tauri_plugin_notification::NotificationExt;

    let granted = match app.notification().permission_state() {
        Ok(tauri_plugin_notification::PermissionState::Granted) => true,
        Ok(_) => matches!(
            app.notification().request_permission(),
            Ok(tauri_plugin_notification::PermissionState::Granted)
        ),
        Err(_) => false,
    };

    if granted {
        if app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .is_ok()
        {
            return;
        }
        // Native show failed — log why, then fall back rather than go silent.
        log::warn!("native notification failed; falling back to osascript/notify-send");
    } else {
        // User denied the permission prompt (or the plugin is unavailable).
        // The fallback keeps Handover usable — at the cost of the banner
        // being attributed to Script Editor when clicked.
        log::warn!("notification permission not granted; falling back to osascript/notify-send");
    }
    notify::notify(title, body);
}

#[cfg(test)]
mod tests {
    use super::{clamp_palette_height, is_openable_url, PALETTE_MAX_HEIGHT, PALETTE_MIN_HEIGHT};

    #[test]
    fn palette_heights_are_clamped_to_a_usable_range() {
        assert_eq!(clamp_palette_height(240.0), 240.0);
        assert_eq!(clamp_palette_height(10.0), PALETTE_MIN_HEIGHT);
        assert_eq!(clamp_palette_height(5000.0), PALETTE_MAX_HEIGHT);
        assert_eq!(clamp_palette_height(f64::NAN), 210.0);
    }

    #[test]
    fn only_plain_web_links_can_be_opened_from_a_reply() {
        assert!(is_openable_url("https://github.com/thefullctx/handover"));
        assert!(is_openable_url("http://127.0.0.1:8001/v1"));
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "vscode://open?file=x",
            "https://",
            "https://example.com/a b",
            "https://example.com/\nrm",
            "-a Calculator",
        ] {
            assert!(!is_openable_url(bad), "should refuse: {bad:?}");
        }
    }
}
