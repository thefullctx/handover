import type {
  AgentConfig,
  AgentMetaStatus,
  AppInfo,
  Capture,
  LiveSession,
  PalettePayload,
  SendOutcome,
} from "../lib/types";

export function makeCapture(overrides: Partial<Capture> = {}): Capture {
  return {
    id: "cap-1",
    timestamp: "2026-08-11T10:00:00Z",
    source: { type: "clipboard", application: "Terminal" },
    content: { type: "text", text: "panic: something exploded" },
    metadata: {},
    ...overrides,
  };
}

export function makeAgents(overrides: Partial<AgentMetaStatus>[] = []): AgentMetaStatus[] {
  const base: AgentMetaStatus[] = [
    {
      meta: {
        id: "hermes",
        name: "Hermes",
        description: "Local coding agent",
        kind: "command",
        demo: false,
      },
      status: { available: true, detail: "found", running: false },
    },
    {
      meta: {
        id: "codex",
        name: "Codex",
        description: "OpenAI Codex CLI",
        kind: "command",
        demo: false,
      },
      status: { available: true, detail: "found", running: false },
    },
  ];
  return base.map((a, i) => (overrides[i] ? { ...a, ...overrides[i] } : a));
}

/** A live session (freshest-first semantics: first entry per agent wins). */
export function makeSession(overrides: Partial<LiveSession> = {}): LiveSession {
  return {
    agent_id: "hermes",
    session_id: "20260812_130220_dbf5cf",
    updated_at: new Date().toISOString(),
    path: null,
    size_bytes: null,
    activity: "idle",
    ...overrides,
  };
}

export function makePayload(overrides: Partial<PalettePayload> = {}): PalettePayload {
  return {
    actions: [
      { id: "fix", name: "Fix this", description: "Investigate this issue and fix it.", prompt_template: "x", icon: null },
      { id: "investigate", name: "Investigate", description: "Find the root cause. Do not modify files yet.", prompt_template: "x", icon: null },
      { id: "explain", name: "Explain", description: "Explain this clearly.", prompt_template: "x", icon: null },
      { id: "write_tests", name: "Write tests", description: "Create appropriate tests.", prompt_template: "x", icon: null },
      { id: "ask", name: "Ask", description: "Tell me what you recommend.", prompt_template: "x", icon: null },
    ],
    agents: makeAgents(),
    preferences: {},
    quick_send: true,
    default_agent_id: "hermes",
    config_path: "~/.config/handover/config.toml",
    shortcut: "CmdOrCtrl+Shift+A",
    sessions: [],
    ...overrides,
  };
}

export function makeOutcome(overrides: Partial<SendOutcome> = {}): SendOutcome {
  return {
    id: "handoff-1",
    ok: true,
    agent_id: "hermes",
    agent_name: "Hermes",
    action_id: "fix",
    capture: makeCapture(),
    created_at: new Date().toISOString(),
    prompt: "Investigate this issue and fix it.\nContext: panic: something exploded",
    receipt: {
      ok: true,
      detail: "ok",
      duration_ms: 45000,
      stdout: "Fixed the panic by adding a null check.\nDone.",
      stderr: "",
    },
    error: null,
    ...overrides,
  };
}

export function makeFailedOutcome(overrides: Partial<SendOutcome> = {}): SendOutcome {
  return makeOutcome({
    ok: false,
    receipt: null,
    error: "Agent `hermes` did not respond within 120s.",
    ...overrides,
  });
}

export function makeInfo(overrides: Partial<AppInfo> = {}): AppInfo {
  return {
    version: "0.1.0",
    config_path: "~/Library/Application Support/handover/config.toml",
    shortcut: "CmdOrCtrl+Shift+A",
    quick_send: true,
    notifications: true,
    launch_at_startup: false,
    appearance: "system",
    ui_opacity: 1,
    excluded_paths: [".env", ".env.*", "*.pem", "~/.ssh/*"],
    ...overrides,
  };
}

export function makeAgentConfig(overrides: Partial<AgentConfig> = {}): AgentConfig {
  return {
    id: "my-agent",
    name: "My Agent",
    kind: "command",
    command: 'my-agent --prompt "{PROMPT}"',
    description: null,
    working_dir: null,
    env: {},
    timeout_secs: null,
    enabled: true,
    demo: false,
    default_action: null,
    ...overrides,
  };
}
