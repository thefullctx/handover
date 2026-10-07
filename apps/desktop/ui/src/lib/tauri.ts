// Typed wrappers around the Tauri IPC surface. Components never call
// `invoke` directly — they use these functions, which keeps the presentation
// layer decoupled from IPC details and gives tests a single module to mock.
//
// Every function maps 1:1 to a Rust `#[tauri::command]` in
// apps/desktop/src-tauri/src/main.rs.
//
// IMPORTANT: Tauri's command macro converts Rust `snake_case` params to
// `lowerCamelCase` IPC keys by default (tauri-macros `ArgumentCase::Camel`).
// So the payload keys here MUST stay camelCase (`actionId`, `binaryPath`,
// `workingDir`, …) even though the Rust signatures read `action_id`,
// `binary_path`, `working_dir`. Sending snake_case breaks every multi-word
// command with "missing required key <camelName>". Do not "fix" this.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AgentConfig,
  ApprovalResult,
  AppInfo,
  Capture,
  DroppedFile,
  GalleryAgent,
  LiveSession,
  PalettePayload,
  SendOutcome,
} from "./types";

export { invoke, listen };

// --- palette ---------------------------------------------------------------

export function getPalettePayload(): Promise<PalettePayload> {
  return invoke<PalettePayload>("palette_payload");
}

export function getRecentHandoffs(): Promise<SendOutcome[]> {
  return invoke<SendOutcome[]>("recent_handoffs");
}

export function sendHandoff(
  actionId: string,
  agentId: string,
  capture: Capture | null,
  sessionId?: string | null
): Promise<SendOutcome> {
  return invoke<SendOutcome>("send_handoff", {
    actionId,
    agentId,
    capture,
    sessionId: sessionId ?? null,
  });
}

export function rememberPreference(actionId: string, agentId: string): Promise<void> {
  return invoke("set_preference", { actionId, agentId });
}

export function hidePalette(): Promise<void> {
  return invoke("hide_palette");
}

export function copyText(text: string): Promise<void> {
  return invoke("copy_text", { text });
}

/** Resizes the palette window to `height` points (width is fixed). `compact`
 *  marks the picker-only size, which the palette reopens at next time. */
export function setPaletteHeight(height: number, compact: boolean): Promise<void> {
  return invoke("set_palette_height", { height, compact });
}

/** Opens an http(s) link from a reply in the default browser (Rust refuses
 *  every other scheme). */
export function openUrl(url: string): Promise<void> {
  return invoke("open_url", { url });
}

export function notifyResult(title: string, body: string): Promise<void> {
  return invoke("notify_result", { title, body });
}

export function openSettingsWindow(): Promise<void> {
  return invoke("open_settings_window");
}

export function clearHistory(): Promise<void> {
  return invoke("clear_history");
}

/** Reads files dragged onto the palette (the webview can't open local paths). */
export function readDroppedFiles(paths: string[]): Promise<DroppedFile[]> {
  return invoke<DroppedFile[]>("read_dropped_files", { paths });
}

// --- settings --------------------------------------------------------------

export function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("app_info");
}

export function getConfiguredAgents(): Promise<AgentConfig[]> {
  return invoke<AgentConfig[]>("configured_agents");
}

export function getAvailableAgents(): Promise<GalleryAgent[]> {
  return invoke<GalleryAgent[]>("available_agents");
}

export function addAgent(config: {
  id: string;
  name: string;
  command: string;
  description: string | null;
  workingDir: string | null;
  timeoutSecs: number | null;
  enabled: boolean;
}): Promise<void> {
  return invoke("add_agent", config);
}

export function removeAgent(id: string): Promise<void> {
  return invoke("remove_agent", { id });
}

export function makeDefaultAgent(id: string): Promise<void> {
  return invoke("set_default_agent", { id });
}

/** Visibility switch: disabled agents stay configured but leave the palette. */
export function setAgentEnabled(id: string, enabled: boolean): Promise<void> {
  return invoke("set_agent_enabled", { id, enabled });
}

export function configureAgent(id: string, binaryPath: string | null): Promise<void> {
  return invoke("configure_agent", { id, binaryPath });
}

export function setQuickSend(enabled: boolean): Promise<void> {
  return invoke("set_quick_send", { enabled });
}

export function setNotifications(enabled: boolean): Promise<void> {
  return invoke("set_notifications", { enabled });
}

export function setAppearance(appearance: string): Promise<void> {
  return invoke("set_appearance", { appearance });
}

/** Persist UI opacity (0.4–1.0) for palette + Settings; returns clamped value. */
export function setUiOpacity(opacity: number): Promise<number> {
  return invoke("set_ui_opacity", { opacity });
}

export function setShortcut(shortcut: string): Promise<void> {
  return invoke("set_shortcut", { shortcut });
}

export function setLaunchAtStartup(enabled: boolean): Promise<void> {
  return invoke("set_launch_at_startup", { enabled });
}

export function setExcludedPaths(patterns: string[]): Promise<void> {
  return invoke("set_excluded_paths", { patterns });
}

export function revealConfig(): Promise<void> {
  return invoke("reveal_config");
}

export function takeSettingsTab(): Promise<unknown> {
  return invoke("take_settings_tab");
}

/** Live agent sessions for the Settings "Detect session" preview. */
export function getLiveSessions(): Promise<LiveSession[]> {
  return invoke<LiveSession[]>("live_sessions");
}

/** Approve/deny a blocked session (opt-in per agent; fail soft). */
export function approveSession(
  agentId: string,
  sessionId: string,
  approve: boolean
): Promise<ApprovalResult> {
  return invoke<ApprovalResult>("approve_session", { agentId, sessionId, approve });
}
