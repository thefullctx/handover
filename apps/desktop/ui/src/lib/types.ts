// Shared types mirroring the Rust side (snake_case from serde). Both the
// palette (App) and the Settings window import from here so the two windows
// can never drift apart.

export type ContentKind = "text" | "image" | "file" | "url" | "terminal" | "mixed";

export interface CaptureContent {
  type: ContentKind;
  text?: string | null;
  path?: string | null;
  items?: string[] | null;
}

export interface Capture {
  id: string;
  timestamp: string;
  source: { type: string; application?: string | null };
  content: CaptureContent;
  metadata: Record<string, string>;
}

export interface Action {
  id: string;
  name: string;
  description: string;
  prompt_template: string;
  icon?: string | null;
}

export interface AgentStatus {
  available: boolean;
  detail: string;
  /** True when the agent's process is running right now (best-effort
   *  live-process check by the daemon) — the status light's "online" cue. */
  running: boolean;
  /** Set when the binary is fine but its model provider is unreachable —
   *  a send would fail; the reason is shown instead of wasting one. */
  provider_down?: string | null;
}

export interface AgentMeta {
  id: string;
  name: string;
  description: string;
  kind: string;
  config_summary?: string | null;
  demo?: boolean;
}

export interface AgentMetaStatus {
  meta: AgentMeta;
  status: AgentStatus;
}

/** Mirrors `handover_config::AgentConfig` (snake_case from serde). */
export interface AgentConfig {
  id: string;
  name: string;
  kind: string;
  command: string;
  description?: string | null;
  working_dir?: string | null;
  env: Record<string, string>;
  timeout_secs?: number | null;
  enabled: boolean;
  demo: boolean;
  default_action?: string | null;
  /** Session-aware agents: glob over session transcripts (filename = id). */
  session_glob?: string | null;
  /** Session-aware agents without a filesystem glob (Hermes SQLite → CLI list). */
  session_cli_list?: string[] | null;
  /** Command run to resume into a live session; contains `{SESSION}`. */
  resume_command?: string | null;
  /** Approval layer (Phase 6.5, opt-in): tail-peek marker for blocked sessions. */
  permission_marker?: string | null;
  /** Approval layer: `tty` | `tmux` | `agent`. */
  approval_channel?: string | null;
}

/** What an agent is doing right now — derived from transcript mtime only. */
export type ActivityState = "working" | "idle";

/** Mirrors `handover_core::session::LiveSession` (snake_case from serde). */
export interface LiveSession {
  agent_id: string;
  session_id: string;
  /** ISO timestamp of last transcript write. */
  updated_at: string;
  /** Transcript path, when the agent persists sessions as files. */
  path?: string | null;
  /** Transcript size in bytes (stat-only size proxy). */
  size_bytes?: number | null;
  activity: ActivityState;
  /** True when the session is parked at an approval prompt. Only set for
   *  agents with a `permission_marker` configured (the one documented
   *  privacy exception — a tail-peek of the last few KB). `null`/absent =
   *  the agent is not approval-aware. */
  blocked?: boolean | null;
  /** The marker line itself (the prompt the agent is asking) when the
   *  session is blocked — the approval card shows WHAT is being asked.
   *  `null`/absent when not blocked. */
  blocked_detail?: string | null;
}

/** Result of an approve/deny action: `verified` is only true when the
 *  transcript was observed resuming (fail soft, never a false success). */
export interface ApprovalResult {
  verified: boolean;
  message: string;
}

export interface PalettePayload {
  actions: Action[];
  agents: AgentMetaStatus[];
  preferences: Record<string, string>;
  /** When true, selecting an action with a remembered preferred agent
   *  shows the quick-send confirmation instead of the agent picker. */
  quick_send: boolean;
  /** The default agent (first enabled) — badge in the agent picker. */
  default_agent_id: string | null;
  /** Platform config path for empty-state / error copy. */
  config_path: string;
  /** The configured accelerator (e.g. `CmdOrCtrl+Shift+A`) — shown in the
   *  palette's onboarding, cheat sheet and empty states. */
  shortcut: string;
  /** Live agent sessions (freshest-first, per-session activity state). */
  sessions: LiveSession[];
}

/** A file dropped onto the palette, read for handoff (see `read_dropped_files`). */
export interface DroppedFile {
  path: string;
  name: string;
  size: number;
  /** "text" (read into `text`) | "image" | "other" (see `note`). */
  kind: string;
  text: string | null;
  note: string | null;
}

export interface SendReceipt {
  ok: boolean;
  detail: string;
  duration_ms: number;
  stdout?: string | null;
  stderr?: string | null;
  /** Session the reply landed in (resume-by-id handoffs). */
  session_id?: string | null;
}

export interface SendOutcome {
  /** Stable, session-unique id (e.g. `handoff-3`). */
  id: string;
  ok: boolean;
  agent_id: string;
  agent_name: string;
  action_id: string;
  /** The capture that was handed off — enables retry / duplicate / re-send. */
  capture?: Capture | null;
  /** ISO timestamp of completion — history groups entries by day. */
  created_at?: string | null;
  prompt: string;
  receipt: SendReceipt | null;
  error?: string | null;
  /** The live session the reply landed in (session-aware agents). */
  session_id?: string | null;
}

/** One exchange in the palette's live chat thread. */
export interface ChatTurn {
  /** The exact rendered prompt that was sent — what the agent actually
   *  received (shown expandable per turn: nothing is ever hidden). */
  prompt: string;
  outcome: SendOutcome;
}

export interface AppInfo {
  version: string;
  config_path: string;
  shortcut: string;
  quick_send: boolean;
  notifications: boolean;
  launch_at_startup: boolean;
  appearance: string;
  /** Palette + Settings glass opacity, 0.4–1.0. */
  ui_opacity: number;
  excluded_paths: string[];
}

/** Mirrors `handover_daemon::catalog::GalleryAgent`. */
export interface GalleryAgent {
  id: string;
  name: string;
  description: string;
  command_template: string;
  binary_name: string;
  timeout_secs: number;
  detected_path: string | null;
  configured: boolean;
  /** Read-only sign-in probe: true = credential marker exists,
      false = not signed in, null = no known marker. */
  signed_in: boolean | null;
}

export type PaletteStep =
  | "loading"
  | "home"
  | "chat"
  | "done"
  | "error"
  | "history";

export type SettingsTab = "general" | "agents" | "privacy" | "about";

export const WELCOME_STORAGE_KEY = "handover.welcome.seen";
