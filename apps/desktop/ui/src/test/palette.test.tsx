import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "../App";
import { makeAgents, makeFailedOutcome, makeOutcome, makePayload, makeSession } from "./fixtures";

// ---------------------------------------------------------------------------
// Tauri mock — components call the typed wrappers in ../lib/tauri; we mock
// that module (with the same argument shapes as the real wrappers) so the
// palette runs headless in jsdom.
// ---------------------------------------------------------------------------
const tauri = vi.hoisted(() => {
  const invoke = vi.fn(
    async (_cmd: string, ..._: unknown[]): Promise<unknown> => {
      throw new Error(`unhandled invoke: ${_cmd}`);
    }
  );
  /** Registered event listeners, keyed by event name (so tests can fire
   *  `palette:open`, `tauri://drag-drop`, … as the Rust side would). */
  const listeners: Record<string, (e: { payload?: unknown }) => void> = {};
  const listen = vi.fn(
    async (event: string, cb: (e: { payload?: unknown }) => void) => {
      listeners[event] = cb;
      return () => {
        delete listeners[event];
      };
    }
  );
  return {
    invoke,
    listen,
    listeners,
    getPalettePayload: () => invoke("palette_payload"),
    getRecentHandoffs: () => invoke("recent_handoffs"),
    sendHandoff: (
      actionId: string,
      agentId: string,
      capture: unknown,
      sessionId?: unknown
    ) => invoke("send_handoff", { actionId, agentId, capture, sessionId: sessionId ?? null }),
    hidePalette: () => invoke("hide_palette"),
    copyText: (text: string) => invoke("copy_text", { text }),
    notifyResult: (title: string, body: string) => invoke("notify_result", { title, body }),
    openSettingsWindow: () => invoke("open_settings_window"),
    clearHistory: () => invoke("clear_history"),
    getAppInfo: () => invoke("app_info"),
    getConfiguredAgents: () => invoke("configured_agents"),
    getAvailableAgents: () => invoke("available_agents"),
    listAgentStatuses: () => invoke("list_agents"),
    addAgent: (config: unknown) => invoke("add_agent", config),
    removeAgent: (id: string) => invoke("remove_agent", { id }),
    makeDefaultAgent: (id: string) => invoke("set_default_agent", { id }),
    setAgentEnabled: (id: string, enabled: boolean) =>
      invoke("set_agent_enabled", { id, enabled }),
    configureAgent: (id: string, binaryPath: string | null) =>
      invoke("configure_agent", { id, binaryPath }),
    setQuickSend: (enabled: boolean) => invoke("set_quick_send", { enabled }),
    setNotifications: (enabled: boolean) => invoke("set_notifications", { enabled }),
    setAppearance: (appearance: string) => invoke("set_appearance", { appearance }),
    setShortcut: (shortcut: string) => invoke("set_shortcut", { shortcut }),
    setLaunchAtStartup: (enabled: boolean) => invoke("set_launch_at_startup", { enabled }),
    setExcludedPaths: (patterns: string[]) => invoke("set_excluded_paths", { patterns }),
    revealConfig: () => invoke("reveal_config"),
    takeSettingsTab: () => invoke("take_settings_tab"),
    readDroppedFiles: (paths: string[]) => invoke("read_dropped_files", { paths }),
    approveSession: (agentId: string, sessionId: string, approve: boolean) =>
      invoke("approve_session", { agentId, sessionId, approve }),
  };
});
vi.mock("../lib/tauri", () => tauri);

/** A promise we can resolve on demand (for in-flight handoffs). */
function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

const sentCalls = () => tauri.invoke.mock.calls.filter((c) => c[0] === "send_handoff");

/** Mutable payload for tests that change what the daemon reports mid-flight
 *  (e.g. an agent flipping from idle to running while the sheet is open). */
const payloadRef = { current: makePayload() };

async function renderApp(
  payload = makePayload(),
  history: unknown[] = [],
  handlers: Record<string, unknown> = {}
) {
  tauri.invoke.mockImplementation(async (cmd: string) => {
    if (cmd in handlers) return handlers[cmd];
    switch (cmd) {
      case "palette_payload":
        return payload;
      case "recent_handoffs":
        return history;
      case "notify_result":
      case "hide_palette":
      case "copy_text":
      case "open_settings_window":
      case "clear_history":
        return undefined;
      case "read_dropped_files":
        return [];
      default:
        throw new Error(`unhandled invoke: ${cmd}`);
    }
  });
  tauri.invoke.mockClear();
  const user = userEvent.setup();
  render(<App />);
  // The palette lands on the merged picker surface (dropdown + chat beneath).
  await waitFor(() => expect(screen.getByTestId("agent-dropdown-trigger")).toBeInTheDocument());
  return user;
}

async function openDropdown(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByTestId("agent-dropdown-trigger"));
  await waitFor(() => expect(screen.getByRole("listbox", { name: "Agents" })).toBeInTheDocument());
}

/** Opens the picker and chooses an agent by name — the chat appears directly
 *  beneath the trigger (no navigation). */
async function chooseAgent(
  user: ReturnType<typeof userEvent.setup>,
  name: RegExp
) {
  await openDropdown(user);
  await user.click(screen.getByRole("option", { name }));
  await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
}

describe("palette — agent dropdown picker (groups, status, logos)", () => {
  it("opens clean (no clipboard capture), trigger shows a placeholder, menu groups Running now / Available", async () => {
    const user = await renderApp(
      makePayload({ sessions: [makeSession({ agent_id: "hermes", activity: "working" })] })
    );
    // Clipboard capture was removed — nothing pre-fills the composer line.
    expect(screen.queryByTestId("captured-line")).not.toBeInTheDocument();

    // The picker starts closed with a placeholder trigger.
    expect(screen.queryByRole("listbox", { name: "Agents" })).not.toBeInTheDocument();
    expect(screen.getByTestId("agent-dropdown-trigger")).toHaveTextContent("Choose an agent…");

    // Groups appear only while the menu is open, in order.
    await openDropdown(user);
    const labels = screen.getAllByText(/Running now|Available/);
    expect(labels[0]).toHaveTextContent("Running now");
    expect(labels[1]).toHaveTextContent("Available");
    // Hermes is running (live session); Codex is available but idle-less.
    expect(screen.getByRole("option", { name: /Hermes/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Codex/ })).toBeInTheDocument();
    // Rows are lean — no descriptions, no text status chips.
    expect(screen.queryByText(/Local coding agent/)).not.toBeInTheDocument();
    expect(screen.queryByText(/OpenAI Codex CLI/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Ready/)).not.toBeInTheDocument();
    // Each row carries exactly one light: green = genuinely alive right now,
    // red = not running (a leftover session file doesn't count).
    const hermesDot = screen
      .getByRole("option", { name: /Hermes/ })
      .querySelector(".status-dot");
    expect(hermesDot).toHaveClass("ok");
    const codexDot = screen
      .getByRole("option", { name: /Codex/ })
      .querySelector(".status-dot");
    expect(codexDot).toHaveClass("err");

    void user;
  });

  it("choosing an agent opens its chat directly beneath the trigger — no navigation away", async () => {
    const user = await renderApp();
    await chooseAgent(user, /Hermes/);
    // The trigger names the agent being chatted with…
    expect(screen.getByTestId("agent-dropdown-trigger")).toHaveTextContent("Hermes");
    // …and shows only a light, no text chip (Hermes isn't running → red).
    const triggerDot = screen
      .getByTestId("agent-dropdown-trigger")
      .querySelector(".status-dot");
    expect(triggerDot).toHaveClass("err");
    expect(screen.getByTestId("agent-dropdown-trigger")).not.toHaveTextContent(/live|last|Ready/);
    // …and the chat lives on the same surface (the picker is still mounted).
    expect(screen.getByTestId("agent-dropdown-trigger")).toBeInTheDocument();
    expect(screen.getByLabelText("Chat message")).toBeInTheDocument();
  });

  it("the trigger's light is green only while the agent runs in the background", async () => {
    const user = await renderApp(
      makePayload({
        agents: makeAgents([{ status: { available: true, detail: "found", running: true } }]),
        sessions: [makeSession({ agent_id: "hermes", activity: "idle" })],
      })
    );
    await chooseAgent(user, /Hermes/);
    // Hermes' process is genuinely alive (quiet session doesn't matter).
    const triggerDot = screen
      .getByTestId("agent-dropdown-trigger")
      .querySelector(".status-dot");
    expect(triggerDot).toHaveClass("ok");
  });

  it("labels a quiet session honestly: green dot only when the process is alive", async () => {
    const user = await renderApp(
      makePayload({
        sessions: [
          makeSession({ agent_id: "hermes", activity: "idle" }),
          makeSession({ agent_id: "codex", activity: "idle", blocked: true, session_id: "b-1" }),
        ],
      })
    );
    await openDropdown(user);
    // No text chips anywhere — the light is the only status cue.
    expect(screen.queryByText(/last ·/)).not.toBeInTheDocument();
    expect(screen.queryByText(/live ·/)).not.toBeInTheDocument();
    // A quiet session on an idle agent reads RED (not running right now).
    const hermesDot = screen
      .getByRole("option", { name: /Hermes/ })
      .querySelector(".status-dot");
    expect(hermesDot).toHaveClass("err");
    // Blocked keeps its words.
    expect(screen.getByText(/blocked · approve\?/)).toBeInTheDocument();
    void user;
  });

  it("keeps a quiet session on an idle agent under Available — Running now is for live processes only", async () => {
    // Codex has a session file but its process isn't running → it must not
    // read as "Running now"; it sits under Available with an honest "last" chip.
    const user = await renderApp(
      makePayload({ sessions: [makeSession({ agent_id: "codex", activity: "idle", session_id: "c-1" })] })
    );
    await openDropdown(user);
    const labels = screen.getAllByText(/Running now|Available/);
    expect(labels).toHaveLength(1);
    expect(labels[0]).toHaveTextContent("Available");
    // The quiet session reads honestly (red light) and stays resumable.
    const codexDot = screen
      .getByRole("option", { name: /Codex/ })
      .querySelector(".status-dot");
    expect(codexDot).toHaveClass("err");
    expect(screen.getByRole("option", { name: /Codex/ })).toHaveAttribute("aria-disabled", "false");
    void user;
  });

  it("puts an agent whose process is running under Running now even when quiet", async () => {
    // Hermes is open (process running) but its session is quiet → Running now.
    const user = await renderApp(
      makePayload({
        agents: makeAgents([{ status: { available: true, detail: "found", running: true } }]),
        sessions: [makeSession({ agent_id: "hermes", activity: "idle" })],
      })
    );
    await openDropdown(user);
    const labels = screen.getAllByText(/Running now|Available/);
    expect(labels[0]).toHaveTextContent("Running now");
    expect(screen.getByRole("option", { name: /Hermes/ })).toBeInTheDocument();
    void user;
  });

  it("shows unavailable agents as a non-activatable 'Not found' group", async () => {
    const agents = [
      ...makeAgents(),
      {
        meta: { id: "claude", name: "Claude Code", description: "Anthropic's coding agent", kind: "command", demo: false },
        status: { available: false, running: false, detail: "not found" },
      },
    ];
    const user = await renderApp(makePayload({ agents }));
    await openDropdown(user);
    // Group header + per-row chip both say "Not found".
    expect(screen.getAllByText("Not found").length).toBeGreaterThan(0);
    const claude = screen.getByRole("option", { name: /Claude Code/ });
    expect(claude).toHaveAttribute("aria-disabled", "true");
    await user.click(claude);
    // Unavailable agents cannot be chatted with.
    expect(screen.queryByTestId("chat")).not.toBeInTheDocument();
    void user;
  });

  it("closes on Escape without dismissing the whole palette, then again to dismiss", async () => {
    const user = await renderApp();
    await openDropdown(user);
    await user.keyboard("{Escape}");
    await waitFor(() =>
      expect(screen.queryByRole("listbox", { name: "Agents" })).not.toBeInTheDocument()
    );
    // The sheet is still up (the palette did not hide).
    expect(screen.getByTestId("agent-dropdown-trigger")).toBeInTheDocument();
    expect(tauri.invoke).not.toHaveBeenCalledWith("hide_palette");
    // Second Escape — nothing open — dismisses the palette itself.
    await user.keyboard("{Escape}");
    await waitFor(() => expect(tauri.invoke).toHaveBeenCalledWith("hide_palette"));
  });

  it("navigates the menu with the arrow keys; Enter chooses the highlighted agent", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    // ArrowDown opens the picker (keyboard-first).
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(screen.getByRole("listbox", { name: "Agents" })).toBeInTheDocument());
    // Alphabetical order: Codex first, Hermes second (no recents yet).
    expect(screen.getByRole("option", { name: /Codex/ })).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowDown}{Enter}");
    await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
    expect(screen.getByTestId("agent-dropdown-trigger")).toHaveTextContent("Hermes");
    expect(screen.getByLabelText("Chat message")).toBeInTheDocument();
  });

  it("flips a dot red→green live while open, without losing dropdown or chat state", async () => {
    // Real timers, short poll: the interval is injectable so this test never
    // touches fake timers (a timeout there poisons the whole suite).
    const w = window as unknown as { __handoverStatusPollMs?: number };
    w.__handoverStatusPollMs = 40;
    try {
      const user = userEvent.setup();
      tauri.invoke.mockImplementation(async (cmd: string) => {
        switch (cmd) {
          case "palette_payload":
            return payloadRef.current;
          case "recent_handoffs":
            return [];
          case "notify_result":
          case "hide_palette":
          case "copy_text":
          case "open_settings_window":
            return undefined;
          default:
            throw new Error(`unhandled invoke: ${cmd}`);
        }
      });
      render(<App />);
      await waitFor(() =>
        expect(screen.getByTestId("agent-dropdown-trigger")).toBeInTheDocument()
      );

      // Open the picker; Codex starts idle → its row light must be RED.
      await openDropdown(user);
      let codexDot = screen
        .getByRole("option", { name: /Codex/ })
        .querySelector(".status-dot");
      expect(codexDot).toHaveClass("err");

      // The agent "starts" elsewhere (daemon now reports running)…
      payloadRef.current = makePayload({
        agents: makeAgents([
          {},
          { status: { available: true, detail: "found", running: true } },
        ]),
      });
      // …within a poll tick the SAME open menu shows GREEN…
      await waitFor(
        () => {
          codexDot = screen
            .getByRole("option", { name: /Codex/ })
            .querySelector(".status-dot");
          expect(codexDot).toHaveClass("ok");
        },
        { timeout: 3000 }
      );
      // …and the menu stayed open through the refresh — no hide/reopen.
      expect(screen.getByRole("listbox", { name: "Agents" })).toBeInTheDocument();

      // And choosing an agent still works right after a tick.
      await user.click(screen.getByRole("option", { name: /Codex/ }));
      await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());

      // A poll while chatting keeps the thread intact (payload swaps only).
      const composer = screen.getByLabelText("Chat message");
      await user.click(composer);
      await user.keyboard("hello there");
      await waitFor(() => {
        expect(screen.getByLabelText("Chat message")).toHaveValue("hello there");
        expect(screen.getByTestId("chat")).toBeInTheDocument();
      });
    } finally {
      delete w.__handoverStatusPollMs;
    }
  });

  it("flags an installed agent whose model provider is down — amber + reason", async () => {
    const agents = [
      ...makeAgents(),
      {
        meta: { id: "opencode", name: "OpenCode", description: "", kind: "command", demo: false },
        status: {
          available: true,
          running: false,
          detail: "found",
          provider_down: "provider down (http://127.0.0.1:8001/v1)",
        },
      },
    ];
    const user = await renderApp(makePayload({ agents }));
    await openDropdown(user);
    // Amber light + the plain-language reason under the name…
    expect(screen.getByText(/provider down/)).toBeInTheDocument();
    const ocDot = screen
      .getByRole("option", { name: /OpenCode/ })
      .querySelector(".status-dot");
    expect(ocDot).toHaveClass("warn");
    // …and choosing it still works (the send will fail fast with this reason).
    await user.click(screen.getByRole("option", { name: /OpenCode/ }));
    await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
  });

  it("offers Settings as the setup path when no agents exist", async () => {
    const user = await renderApp(makePayload({ agents: [] }));
    await openDropdown(user);
    await user.click(screen.getByRole("button", { name: /Open Settings/ }));
    expect(tauri.invoke).toHaveBeenCalledWith("open_settings_window");
  });
});

describe("palette — chat with an agent (verbatim, session-aware)", () => {
  it("starts the composer empty and sends a typed message verbatim as 'ask'", async () => {
    const user = await renderApp(makePayload({ quick_send: false }), [], {
      send_handoff: makeOutcome(),
    });
    await chooseAgent(user, /Codex/); // first agent alphabetically

    // No clipboard capture → the composer opens empty.
    const composer = screen.getByLabelText("Chat message");
    expect(composer).toHaveValue("");
    await user.click(composer);
    await user.keyboard("panic: something exploded");

    await user.keyboard("{Enter}");
    await waitFor(() => expect(sentCalls()).toHaveLength(1));
    expect(sentCalls()[0][1]).toMatchObject({ actionId: "ask", agentId: "codex" });
    expect((sentCalls()[0][1] as { capture: { content: { text: string } } }).capture.content.text).toBe(
      "panic: something exploded"
    );
    // The reply lands in the thread.
    await waitFor(() =>
      expect(within(screen.getByTestId("chat-thread")).getByText(/Fixed the panic by adding a null check/)).toBeInTheDocument()
    );
  });

  it("lets the daemon resolve the first message, then pins follow-ups to the adopted session", async () => {
    const SID = "20260814_031532_0bb565";
    const user = await renderApp(
      makePayload({
        agents: makeAgents([{ status: { available: true, detail: "found", running: true } }]),
        sessions: [makeSession({ agent_id: "hermes", activity: "idle", session_id: SID })],
      }),
      [],
      { send_handoff: makeOutcome({ session_id: SID }) }
    );
    await chooseAgent(user, /Hermes/);

    // The composer opens clean (no clipboard prefill) — type a message.
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(sentCalls()).toHaveLength(1));
    // First message carries no session: the daemon resolves freshest-first
    // itself, so an owned live session falls back to fresh instead of failing.
    expect(sentCalls()[0][1]).toMatchObject({ actionId: "ask", agentId: "hermes", sessionId: null });

    // The adopted session anchors follow-ups explicitly.
    await waitFor(() =>
      expect(within(screen.getByTestId("chat-thread")).getByText(/Fixed the panic by adding a null check/)).toBeInTheDocument()
    );
    await user.click(composer);
    await user.keyboard("and then?");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(sentCalls()).toHaveLength(2));
    expect(sentCalls()[1][1]).toMatchObject({ actionId: "ask", agentId: "hermes", sessionId: SID });
  });

  it("shows the reply duration as a chip on the agent's message", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    await waitFor(() =>
      expect(within(screen.getByTestId("chat-thread")).getByText(/Fixed the panic by adding a null check/)).toBeInTheDocument()
    );
    // makeOutcome's receipt duration_ms = 45000 → "0:45".
    expect(screen.getByText("0:45")).toBeInTheDocument();
  });

  it("shows a pixel-grid thinking state before the stream starts", async () => {
    const pending = deferred<unknown>();
    const user = await renderApp(makePayload(), [], { send_handoff: pending.promise });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    const inflight = await screen.findByTestId("chat-inflight");
    // No streamed output yet → the loader + "Thinking…" state.
    expect(within(inflight).getByText(/Thinking/)).toBeInTheDocument();
    expect(inflight.querySelector(".pixel-loader")).not.toBeNull();
    pending.resolve(makeOutcome());
    await waitFor(() => expect(screen.queryByTestId("chat-inflight")).not.toBeInTheDocument());
  });

  it("keeps a failed exchange in the thread instead of bouncing out", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeFailedOutcome() });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    await waitFor(() =>
      expect(screen.getByText(/did not respond within/)).toBeInTheDocument()
    );
    expect(screen.getByTestId("chat")).toBeInTheDocument();
    expect(document.querySelectorAll(".chat-bubble.user")).toHaveLength(1);
  });

  it("streams the reply inline while a message is in flight", async () => {
    const pending = deferred<unknown>();
    const user = await renderApp(makePayload(), [], { send_handoff: pending.promise });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");

    const inflight = await screen.findByTestId("chat-inflight");
    expect(within(inflight).getByText(/Codex is working/)).toBeInTheDocument();
    tauri.listeners["handoff:output"]({
      payload: { id: "handoff-0", stream: "stdout", chunk: "Working through" },
    });
    await waitFor(() =>
      expect(within(inflight).getByText(/Working through/)).toBeInTheDocument()
    );

    pending.resolve(makeOutcome());
    await waitFor(() => expect(screen.queryByTestId("chat-inflight")).not.toBeInTheDocument());
  });

  it("switching agents in the dropdown swaps the thread beneath the trigger", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("first conversation");
    await user.keyboard("{Enter}");
    await waitFor(() =>
      expect(within(screen.getByTestId("chat-thread")).getByText(/Fixed the panic by adding a null check/)).toBeInTheDocument()
    );

    // Pick another agent — same surface, fresh thread.
    await openDropdown(user);
    await user.click(screen.getByRole("option", { name: /Hermes/ }));
    await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
    expect(screen.getByText(/Say hello to Hermes/)).toBeInTheDocument();
    expect(within(screen.getByTestId("chat-thread")).queryByText(/Fixed the panic/)).not.toBeInTheDocument();
  });
});

describe("palette — handoff activity (real measurements, no guessing)", () => {
  /** Starts a chat and returns with a message in flight. */
  async function sendPending(user: ReturnType<typeof userEvent.setup>) {
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    await screen.findByTestId("chat-inflight");
  }

  it("shows the daemon-measured prompt size and an unmeasured first-response stat", async () => {
    const pending = deferred<unknown>();
    const user = await renderApp(makePayload(), [], { send_handoff: pending.promise });
    await sendPending(user);

    // The daemon reports the rendered prompt's byte size — the UI never
    // re-derives it, it just shows the number Rust measured.
    tauri.listeners["handoff:started"]({ payload: { id: "handoff-9", prompt_len: 2048 } });
    const activity = await screen.findByTestId("chat-activity");
    await waitFor(() =>
      expect(screen.getByTestId("stat-prompt-size")).toHaveTextContent("2.0 KB")
    );
    // Nothing has come back yet — "not measured", not a fake zero.
    expect(screen.getByTestId("stat-first-response")).toHaveTextContent("—");
    // The phase starts at "Sending context" (no output observed yet).
    expect(within(activity).getByText("Sending context")).toHaveAttribute("data-active", "true");
    expect(within(activity).getByText("Agent responding")).toHaveAttribute("data-active", "false");

    pending.resolve(makeOutcome());
    await waitFor(() => expect(screen.queryByTestId("chat-inflight")).not.toBeInTheDocument());
  });

  it("advances the phase and records first-response time from the output stream", async () => {
    const pending = deferred<unknown>();
    const user = await renderApp(makePayload(), [], { send_handoff: pending.promise });
    await sendPending(user);
    tauri.listeners["handoff:started"]({ payload: { id: "handoff-9", prompt_len: 512 } });

    // Output arrives → the phase is "Agent responding" and the time to first
    // response becomes a real measurement.
    tauri.listeners["handoff:output"]({
      payload: { id: "handoff-9", stream: "stdout", chunk: "Working through it" },
    });
    await waitFor(() =>
      expect(screen.getByText("Agent responding")).toHaveAttribute("data-active", "true")
    );
    expect(screen.getByTestId("stat-first-response")).not.toHaveTextContent("—");
    // Output volume grows with the stream (18 bytes of real text).
    expect(screen.getByTestId("stat-output-volume")).toHaveTextContent("18 B");
    expect(screen.getByTestId("activity-output")).toHaveTextContent("Working through it");

    pending.resolve(makeOutcome());
    await waitFor(() => expect(screen.queryByTestId("chat-inflight")).not.toBeInTheDocument());
    // Finished: every timeline step is behind us, none is "active".
    const steps = screen.getByTestId("activity-phases");
    expect(steps.querySelectorAll('[data-active="true"]')).toHaveLength(0);
    expect(screen.getAllByText(/Wrapping up/).length).toBeGreaterThan(0);
  });

  it("reports the completed handoff's receipt output as the output volume", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    await waitFor(() =>
      expect(within(screen.getByTestId("chat-thread")).getByText(/Fixed the panic by adding a null check/)).toBeInTheDocument()
    );
    // The receipt's stdout is the evidence, and its byte length is the stat.
    const activity = screen.getByTestId("chat-activity");
    expect(within(activity).getByText(/Fixed the panic/)).toBeInTheDocument();
    expect(screen.getByTestId("stat-output-volume")).toHaveTextContent(/B|KB/);
  });

  it("shows the exact prompt sent, not just what was typed", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    await waitFor(() =>
      expect(within(screen.getByTestId("chat-thread")).getByText(/Fixed the panic by adding a null check/)).toBeInTheDocument()
    );

    // Transparency: the rendered prompt the agent received is one click away.
    const details = screen.getByText("Prompt that was sent").closest("details");
    expect(details).not.toBeNull();
    // The bubble itself shows the capture; the disclosure shows what the agent
    // ACTUALLY got (the capture wrapped by the action template).
    const bubble = screen.getByText("Prompt that was sent").closest(".chat-bubble.user");
    expect(bubble).toHaveTextContent("panic: something exploded");
    expect(details!.querySelector("pre")).toHaveTextContent(
      "Investigate this issue and fix it. Context: panic: something exploded"
    );
  });
});

describe("palette — blocked sessions (approve/deny, explicit only)", () => {
  it("flags a blocked agent in the dropdown with its pending question", async () => {
    const user = await renderApp(
      makePayload({
        sessions: [
          makeSession({
            agent_id: "hermes",
            activity: "idle",
            blocked: true,
            session_id: "b-1",
            blocked_detail: "[permission] approve shell command: deploy?",
          }),
        ],
      })
    );
    await openDropdown(user);
    expect(screen.getByText(/blocked · approve\?/)).toBeInTheDocument();
    // The blocked entry carries its question in the title tooltip.
    expect(screen.getByTitle("[permission] approve shell command: deploy?")).toBeInTheDocument();
    // No decision happens from the picker alone — approval lives in the chat.
    expect(sentCalls()).toHaveLength(0);
    expect(tauri.invoke).not.toHaveBeenCalledWith(
      "approve_session",
      expect.objectContaining({ agentId: "hermes" })
    );
    void user;
  });

  it("offers Approve/Deny in the chat when chatting a blocked agent", async () => {
    const user = await renderApp(
      makePayload({
        sessions: [makeSession({ agent_id: "hermes", activity: "idle", blocked: true, session_id: "b-1" })],
      })
    );
    await chooseAgent(user, /Hermes/);
    expect(within(screen.getByTestId("approval-card")).getByText(/Hermes needs approval/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Deny" })).toBeInTheDocument();
  });

  it("approves only on the explicit button press", async () => {
    const user = await renderApp(
      makePayload({
        sessions: [makeSession({ agent_id: "hermes", activity: "idle", blocked: true, session_id: "b-1" })],
      }),
      [],
      { approve_session: { verified: true, message: "Approved — the agent is working again." } }
    );
    await chooseAgent(user, /Hermes/);
    await user.click(screen.getByRole("button", { name: "Approve" }));
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("approve_session", {
        agentId: "hermes",
        sessionId: "b-1",
        approve: true,
      })
    );
    await waitFor(() =>
      expect(screen.getByTestId("approval-note")).toHaveTextContent(/agent is working again/)
    );
  });

  it("surfaces the fail-soft message instead of claiming success", async () => {
    const user = await renderApp(
      makePayload({
        sessions: [makeSession({ agent_id: "hermes", activity: "idle", blocked: true, session_id: "b-1" })],
      }),
      [],
      {
        approve_session: {
          verified: false,
          message: "Decision sent, but couldn't confirm the agent resumed. Check the terminal.",
        },
      }
    );
    await chooseAgent(user, /Hermes/);
    await user.click(screen.getByRole("button", { name: "Deny" }));
    await waitFor(() =>
      expect(screen.getByTestId("approval-note")).toHaveTextContent(/couldn't confirm/)
    );
  });

  it("shows the agent's pending question on the blocked dropdown entry", async () => {
    const user = await renderApp(
      makePayload({
        sessions: [
          makeSession({
            agent_id: "hermes",
            activity: "idle",
            blocked: true,
            session_id: "b-1",
            blocked_detail: "[permission] approve shell command: deploy?",
          }),
        ],
      })
    );
    await openDropdown(user);
    expect(screen.getByTitle("[permission] approve shell command: deploy?")).toBeInTheDocument();
    void user;
  });

  it("chat shows an approval card with what the agent is asking", async () => {
    const user = await renderApp(
      makePayload({
        sessions: [
          makeSession({
            agent_id: "hermes",
            activity: "idle",
            blocked: true,
            session_id: "b-1",
            blocked_detail: "[permission] approve shell command: deploy?",
          }),
        ],
      })
    );
    await chooseAgent(user, /Hermes/);
    const card = screen.getByTestId("approval-card");
    expect(within(card).getByText(/Hermes needs approval/)).toBeInTheDocument();
    expect(within(card).getByText(/approve shell command: deploy\?/)).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: "Approve" })).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: "Deny" })).toBeInTheDocument();
  });
});

describe("palette — history and completion actions", () => {
  it("opens a result from history and keeps chatting with that agent", async () => {
    const user = await renderApp(makePayload(), [makeOutcome()]);
    await user.click(screen.getByRole("button", { name: "Recent handoffs" }));
    await waitFor(() => expect(screen.getByTestId("history-list")).toBeInTheDocument());

    await user.click(screen.getByTestId("history-open-handoff-1"));
    await waitFor(() => expect(screen.getByTestId("result-panel")).toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: /Keep chatting/ }));
    await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
    expect(screen.getByTestId("agent-dropdown-trigger")).toHaveTextContent("Hermes");
  });

  it("retry re-opens a chat pre-filled with the original context", async () => {
    const user = await renderApp(makePayload(), [makeOutcome()]);
    await user.click(screen.getByRole("button", { name: "Recent handoffs" }));
    await waitFor(() => expect(screen.getByTestId("history-list")).toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: "Retry handoff-1" }));
    await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
    expect(screen.getByLabelText("Chat message")).toHaveValue("panic: something exploded");
  });

  it("'Another agent' reopens the picker with the context pre-filled", async () => {
    const user = await renderApp(makePayload(), [makeOutcome()]);
    await user.click(screen.getByRole("button", { name: "Recent handoffs" }));
    await waitFor(() => expect(screen.getByTestId("history-list")).toBeInTheDocument());
    await user.click(screen.getByTestId("history-open-handoff-1"));
    await waitFor(() => expect(screen.getByTestId("result-panel")).toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: /Another agent/ }));
    // Back on the picker — already open, ready to choose, draft announced.
    await waitFor(() =>
      expect(screen.getByRole("listbox", { name: "Agents" })).toBeInTheDocument()
    );
    expect(screen.getByTestId("captured-line")).toBeInTheDocument();
  });
});

describe("palette — first-run onboarding (teach by doing)", () => {
  it("presents the welcome on the first open; the shortcut press dismisses it", async () => {
    localStorage.clear();
    const user = await renderApp();
    // First hotkey press → the welcome overlay, not the sheet.
    tauri.listeners["palette:open"]({ payload: null });
    await waitFor(() => expect(screen.getByTestId("welcome")).toBeInTheDocument());
    expect(screen.getByText("Welcome to Handover")).toBeInTheDocument();

    // Pressing the shortcut again IS the dismissal — and it's remembered.
    tauri.listeners["palette:open"]({ payload: null });
    await waitFor(() => expect(screen.queryByTestId("welcome")).not.toBeInTheDocument());
    expect(localStorage.getItem("handover.welcome.seen")).toBe("1");
    expect(screen.getByTestId("agent-dropdown-trigger")).toBeInTheDocument();
    void user;
  });

  it("Start dismisses the welcome and it never returns", async () => {
    localStorage.clear();
    const user = await renderApp();
    tauri.listeners["palette:open"]({ payload: null });
    await screen.findByTestId("welcome");
    await user.click(screen.getByRole("button", { name: /Start/ }));
    await waitFor(() => expect(screen.queryByTestId("welcome")).not.toBeInTheDocument());
    expect(localStorage.getItem("handover.welcome.seen")).toBe("1");
    // A later open goes straight to the sheet.
    tauri.listeners["palette:open"]({ payload: null });
    expect(screen.queryByTestId("welcome")).not.toBeInTheDocument();
  });

  it("offers one-tap add for detected agents in the pick-your-assistants step", async () => {
    localStorage.clear();
    const user = await renderApp(makePayload(), [], {
      configure_agent: null,
      available_agents: [
        {
          id: "codex",
          name: "Codex",
          description: "OpenAI Codex CLI (non-interactive).",
          command_template: "{BIN} exec \"{PROMPT}\"",
          binary_name: "codex",
          timeout_secs: 300,
          detected_path: "/opt/homebrew/bin/codex",
          configured: false,
          signed_in: true,
        },
        {
          id: "omp",
          name: "OMP",
          description: "OMP coding agent (non-interactive, -p).",
          command_template: "{BIN} -p \"{PROMPT}\"",
          binary_name: "omp",
          timeout_secs: 300,
          detected_path: null,
          configured: false,
          signed_in: null,
        },
      ],
    });
    tauri.listeners["palette:open"]({ payload: null });
    await screen.findByTestId("welcome-pickers");

    // Detected agent → Add button. Not-found one is visible but disabled.
    const addCodex = screen.getByRole("button", { name: /Add Codex/ });
    expect(addCodex).toBeEnabled();
    expect(screen.getByRole("button", { name: /OMP/ })).toBeDisabled();

    await user.click(addCodex);
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("configure_agent", { id: "codex", binaryPath: null })
    );
    // The row flips to Added without leaving the welcome.
    expect(screen.queryByRole("button", { name: /Add Codex/ })).not.toBeInTheDocument();
  });

  it("skips the picker step entirely when the gallery is empty or all added", async () => {
    localStorage.clear();
    await renderApp(makePayload(), [], { available_agents: [] });
    tauri.listeners["palette:open"]({ payload: null });
    await screen.findByTestId("welcome");
    expect(screen.queryByTestId("welcome-pickers")).not.toBeInTheDocument();
  });
});

describe("palette — keyboard cheat sheet", () => {
  it("opens from the header tools button, lists the real shortcut, closes on Escape", async () => {
    const user = await renderApp();
    await user.click(screen.getByRole("button", { name: /Keyboard shortcuts/ }));
    const sheet = screen.getByTestId("shortcut-sheet");
    expect(within(sheet).getByText("⌘⇧A")).toBeInTheDocument();
    expect(within(sheet).getByText("Chat with the selected agent")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByTestId("shortcut-sheet")).not.toBeInTheDocument();
  });
});

describe("palette — drag and drop capture (pre-fills the chat)", () => {
  it("captures dropped text as the composer draft with a drop overlay while dragging", async () => {
    const user = await renderApp(makePayload());

    const root = document.querySelector<HTMLElement>(".palette")!;
    fireEvent.dragEnter(root, { dataTransfer: { getData: () => "", files: [] } });
    expect(screen.getByTestId("drop-overlay")).toBeInTheDocument();
    fireEvent.drop(root, { dataTransfer: { getData: () => "fatal: disk full", files: [] } });

    await waitFor(() =>
      expect(screen.getByTestId("captured-line")).toHaveTextContent(/fatal: disk full/)
    );
    // Choosing an agent hands the dropped text to its composer.
    await chooseAgent(user, /Codex/);
    expect(screen.getByLabelText("Chat message")).toHaveValue("fatal: disk full");
  });

  it("captures a dropped file via the tauri://drag-drop event", async () => {
    const user = await renderApp(makePayload(), [], {
      read_dropped_files: [
        {
          path: "/tmp/error.log",
          name: "error.log",
          size: 42,
          kind: "text",
          text: "panic: disk full",
          note: null,
        },
      ],
    });
    tauri.listeners["tauri://drag-drop"]({ payload: { paths: ["/tmp/error.log"] } });
    await waitFor(() =>
      expect(screen.getByTestId("captured-line")).toHaveTextContent(/panic: disk full/)
    );
    expect(tauri.invoke).toHaveBeenCalledWith(
      "read_dropped_files",
      expect.objectContaining({ paths: ["/tmp/error.log"] })
    );
    void user;
  });

  it("explains in plain language when a dropped file can't be read", async () => {
    const user = await renderApp(makePayload(), [], {
      read_dropped_files: [
        {
          path: "/tmp/app.bin",
          name: "app.bin",
          size: 999,
          kind: "other",
          text: null,
          note: "binary file — not read",
        },
      ],
    });
    tauri.listeners["tauri://drag-drop"]({ payload: { paths: ["/tmp/app.bin"] } });
    await waitFor(() => expect(screen.getByTestId("drop-error")).toBeInTheDocument());
    expect(screen.getByText(/binary file/)).toBeInTheDocument();
    void user;
  });
});
