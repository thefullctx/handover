import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "../App";
import { makeAgents, makeFailedOutcome, makeOutcome, makePayload, makeSession } from "./fixtures";

// ---------------------------------------------------------------------------
// Tauri mock
// ---------------------------------------------------------------------------
const tauri = vi.hoisted(() => {
  const invoke = vi.fn(
    async (_cmd: string, ..._: unknown[]): Promise<unknown> => {
      throw new Error(`unhandled invoke: ${_cmd}`);
    }
  );
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
    openUrl: (url: string) => invoke("open_url", { url }),
    setPaletteHeight: (height: number, compact: boolean) =>
      invoke("set_palette_height", { height, compact }),
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

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

const sentCalls = () => tauri.invoke.mock.calls.filter((c) => c[0] === "send_handoff");

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
      case "set_palette_height":
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
  // Palette lands unfolded immediately with the filter input ready.
  await waitFor(() => expect(screen.getByTestId("agent-filter")).toBeInTheDocument());
  return user;
}

/** Chooses an agent directly from the always-unfolded picker. */
async function chooseAgent(
  user: ReturnType<typeof userEvent.setup>,
  name: RegExp
) {
  await user.click(screen.getByRole("option", { name }));
  await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
}

describe("palette — unfolded agent picker (groups, filter, status, logos)", () => {
  it("opens immediately unfolded without any dropdown click, and uses panel height for chat", async () => {
    const { PANEL_CARD_HEIGHT } = await import("../lib/usePaletteSize");
    const sizes = () =>
      tauri.invoke.mock.calls
        .filter((c) => c[0] === "set_palette_height")
        .map((c) => c[1] as { height: number; compact: boolean });
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });

    // Unfolded right from start: listbox is visible without clicking anything.
    expect(screen.getByRole("listbox", { name: "Agents" })).toBeInTheDocument();
    await waitFor(() => expect(sizes().some((c) => c.compact)).toBe(true));

    // Choosing an agent moves cleanly to the panel height for chat.
    await chooseAgent(user, /Codex/);
    await waitFor(() => expect(sizes().some((c) => !c.compact)).toBe(true));
    const panelSize = sizes().filter((c) => !c.compact).slice(-1)[0];
    expect(panelSize.height).toBeGreaterThanOrEqual(PANEL_CARD_HEIGHT);
  });

  it("shows each agent's session state on the right", async () => {
    await renderApp(
      makePayload({ sessions: [makeSession({ agent_id: "hermes", activity: "working" })] })
    );
    expect(screen.getByTestId("agent-hermes")).toHaveTextContent(/Working/);
    expect(screen.getByTestId("agent-codex")).toHaveTextContent(/Installed/);
  });

  it("labels an idle session with its time instead of 'Working'", async () => {
    await renderApp(
      makePayload({ sessions: [makeSession({ agent_id: "codex", activity: "idle" })] })
    );
    expect(screen.getByTestId("agent-codex")).toHaveTextContent(/Session · /);
    expect(screen.getByTestId("agent-codex")).not.toHaveTextContent(/Working/);
  });

  it("opens clean (no clipboard capture), shows filter input, and groups Running now / Available", async () => {
    await renderApp(
      makePayload({ sessions: [makeSession({ agent_id: "hermes", activity: "working" })] })
    );
    expect(screen.queryByTestId("captured-line")).not.toBeInTheDocument();
    expect(screen.getByTestId("agent-filter")).toHaveAttribute(
      "placeholder",
      "Choose an agent or type to filter…"
    );

    const labels = screen.getAllByText(/Running now|Available/);
    expect(labels[0]).toHaveTextContent("Running now");
    expect(labels[1]).toHaveTextContent("Available");
    expect(screen.getByRole("option", { name: /Hermes/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Codex/ })).toBeInTheDocument();

    // Running = green ok dot; installed but idle = neutral idle dot (never red).
    const hermesDot = screen
      .getByRole("option", { name: /Hermes/ })
      .querySelector(".status-dot");
    expect(hermesDot).toHaveClass("ok");
    const codexDot = screen
      .getByRole("option", { name: /Codex/ })
      .querySelector(".status-dot");
    expect(codexDot).toHaveClass("idle");
  });

  it("filters agents live from the top input, launches on Enter, and clears on Escape", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    const filter = screen.getByTestId("agent-filter");

    await user.type(filter, "herm");
    expect(screen.getByRole("option", { name: /Hermes/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Codex/ })).not.toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Hermes/ })).toHaveAttribute("aria-selected", "true");

    // First Escape clears only the filter, keeping the palette open.
    await user.keyboard("{Escape}");
    expect(filter).toHaveValue("");
    expect(screen.getByRole("option", { name: /Codex/ })).toBeInTheDocument();
    expect(tauri.invoke).not.toHaveBeenCalledWith("hide_palette");

    // Typing a non-matching query offers Clear filter.
    await user.type(filter, "zzzz");
    const filterEmpty = screen.getByTestId("filter-empty");
    expect(filterEmpty).toBeInTheDocument();
    await user.click(within(filterEmpty).getByRole("button", { name: "Clear filter" }));
    expect(screen.getByRole("option", { name: /Codex/ })).toBeInTheDocument();

    // Filtering and pressing Enter goes straight to chat.
    await user.type(filter, "herm");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
    expect(screen.getByTestId("chat-agent-switcher")).toHaveTextContent("Hermes");
  });

  it("choosing an agent opens its chat with a compact header switcher", async () => {
    const user = await renderApp();
    await chooseAgent(user, /Hermes/);
    const switcher = screen.getByTestId("chat-agent-switcher");
    expect(switcher).toHaveTextContent("Hermes");
    expect(switcher.querySelector(".status-dot")).toHaveClass("idle");
    expect(switcher).not.toHaveTextContent(/live|last|Ready/);
    expect(screen.getByLabelText("Chat message")).toBeInTheDocument();
  });

  it("the chat switcher's light is green only while the agent runs in the background", async () => {
    const user = await renderApp(
      makePayload({
        agents: makeAgents([{ status: { available: true, detail: "found", running: true } }]),
        sessions: [makeSession({ agent_id: "hermes", activity: "idle" })],
      })
    );
    await chooseAgent(user, /Hermes/);
    expect(screen.getByTestId("chat-agent-switcher").querySelector(".status-dot")).toHaveClass("ok");
  });

  it("labels a quiet session honestly: green dot only when the process is alive", async () => {
    await renderApp(
      makePayload({
        sessions: [
          makeSession({ agent_id: "hermes", activity: "idle" }),
          makeSession({ agent_id: "codex", activity: "idle", blocked: true, session_id: "b-1" }),
        ],
      })
    );
    expect(screen.queryByText(/last ·/)).not.toBeInTheDocument();
    expect(screen.queryByText(/live ·/)).not.toBeInTheDocument();
    const hermesDot = screen
      .getByRole("option", { name: /Hermes/ })
      .querySelector(".status-dot");
    expect(hermesDot).toHaveClass("idle");
    expect(screen.getByText(/blocked · approve\?/)).toBeInTheDocument();
  });

  it("keeps a quiet session on an idle agent under Available — Running now is for live processes only", async () => {
    await renderApp(
      makePayload({ sessions: [makeSession({ agent_id: "codex", activity: "idle", session_id: "c-1" })] })
    );
    const labels = screen.getAllByText(/Running now|Available/);
    expect(labels).toHaveLength(1);
    expect(labels[0]).toHaveTextContent("Available");
    const codexDot = screen
      .getByRole("option", { name: /Codex/ })
      .querySelector(".status-dot");
    expect(codexDot).toHaveClass("idle");
    expect(screen.getByRole("option", { name: /Codex/ })).toHaveAttribute("aria-disabled", "false");
  });

  it("puts an agent whose process is running under Running now even when quiet", async () => {
    await renderApp(
      makePayload({
        agents: makeAgents([{ status: { available: true, detail: "found", running: true } }]),
        sessions: [makeSession({ agent_id: "hermes", activity: "idle" })],
      })
    );
    const labels = screen.getAllByText(/Running now|Available/);
    expect(labels[0]).toHaveTextContent("Running now");
    expect(screen.getByRole("option", { name: /Hermes/ })).toBeInTheDocument();
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
    expect(screen.getAllByText("Not found").length).toBeGreaterThan(0);
    const claude = screen.getByRole("option", { name: /Claude Code/ });
    expect(claude).toHaveAttribute("aria-disabled", "true");
    await user.click(claude);
    expect(screen.queryByTestId("chat")).not.toBeInTheDocument();
  });

  it("returns from chat to the unfolded list on Escape, then dismisses on second Escape", async () => {
    const user = await renderApp();
    await chooseAgent(user, /Codex/);
    await user.keyboard("{Escape}");
    await waitFor(() =>
      expect(screen.getByRole("listbox", { name: "Agents" })).toBeInTheDocument()
    );
    expect(tauri.invoke).not.toHaveBeenCalledWith("hide_palette");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(tauri.invoke).toHaveBeenCalledWith("hide_palette"));
  });

  it("navigates the unfolded list with arrow keys right from the filter input; Enter chooses the highlighted agent", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    // Alphabetical order: Codex first, Hermes second.
    expect(screen.getByRole("option", { name: /Codex/ })).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowDown}{Enter}");
    await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
    expect(screen.getByTestId("chat-agent-switcher")).toHaveTextContent("Hermes");
    expect(screen.getByLabelText("Chat message")).toBeInTheDocument();
  });

  it("wires combobox ARIA attributes on the filter input and skips unavailable agents on arrows", async () => {
    const agents = [
      ...makeAgents(),
      {
        meta: { id: "claude", name: "Claude Code", description: "Anthropic's coding agent", kind: "command", demo: false },
        status: { available: false, running: false, detail: "not found" },
      },
    ];
    const user = await renderApp(makePayload({ agents }));
    const filter = screen.getByTestId("agent-filter");
    expect(filter).toHaveAttribute("role", "combobox");
    expect(filter).toHaveAttribute("aria-controls", "agent-picker-list");
    expect(filter).toHaveAttribute("aria-expanded", "true");
    expect(filter).toHaveAttribute("aria-activedescendant", "agent-option-codex");

    // Codex -> Hermes -> wraps back to Codex, skipping unavailable Claude Code.
    await user.keyboard("{ArrowDown}");
    expect(filter).toHaveAttribute("aria-activedescendant", "agent-option-hermes");
    await user.keyboard("{ArrowDown}");
    expect(filter).toHaveAttribute("aria-activedescendant", "agent-option-codex");
    await user.keyboard("{ArrowUp}");
    expect(filter).toHaveAttribute("aria-activedescendant", "agent-option-hermes");
  });

  it("flips a dot idle→green live while open, without losing list or chat state", async () => {
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
          case "set_palette_height":
          case "open_settings_window":
            return undefined;
          default:
            throw new Error(`unhandled invoke: ${cmd}`);
        }
      });
      render(<App />);
      await waitFor(() =>
        expect(screen.getByRole("listbox", { name: "Agents" })).toBeInTheDocument()
      );

      let codexDot = screen
        .getByRole("option", { name: /Codex/ })
        .querySelector(".status-dot");
      expect(codexDot).toHaveClass("idle");

      payloadRef.current = makePayload({
        agents: makeAgents([
          {},
          { status: { available: true, detail: "found", running: true } },
        ]),
      });
      await waitFor(
        () => {
          codexDot = screen
            .getByRole("option", { name: /Codex/ })
            .querySelector(".status-dot");
          expect(codexDot).toHaveClass("ok");
        },
        { timeout: 3000 }
      );
      expect(screen.getByRole("listbox", { name: "Agents" })).toBeInTheDocument();

      await user.click(screen.getByRole("option", { name: /Codex/ }));
      await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());

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
    expect(screen.getByText(/provider down/)).toBeInTheDocument();
    const ocDot = screen
      .getByRole("option", { name: /OpenCode/ })
      .querySelector(".status-dot");
    expect(ocDot).toHaveClass("warn");
    await user.click(screen.getByRole("option", { name: /OpenCode/ }));
    await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
  });

  it("offers Settings as the setup path when no agents exist", async () => {
    const user = await renderApp(makePayload({ agents: [] }));
    await user.click(screen.getByRole("button", { name: /Open Settings/ }));
    expect(tauri.invoke).toHaveBeenCalledWith("open_settings_window");
  });
});

describe("palette — chat with an agent (verbatim, session-aware)", () => {
  it("starts the composer empty and sends a typed message verbatim as 'ask'", async () => {
    const user = await renderApp(makePayload({ quick_send: false }), [], {
      send_handoff: makeOutcome(),
    });
    await chooseAgent(user, /Codex/);

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
    await waitFor(() =>
      expect(screen.getByTestId("chat-reply")).toHaveTextContent(/Fixed the panic by adding a null check/)
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

    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(sentCalls()).toHaveLength(1));
    expect(sentCalls()[0][1]).toMatchObject({ actionId: "ask", agentId: "hermes", sessionId: null });

    await waitFor(() =>
      expect(screen.getByTestId("chat-reply")).toHaveTextContent(/Fixed the panic by adding a null check/)
    );
    await user.click(composer);
    await user.keyboard("and then?");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(sentCalls()).toHaveLength(2));
    expect(sentCalls()[1][1]).toMatchObject({ actionId: "ask", agentId: "hermes", sessionId: SID });
  });

  it("shows the reply duration on the agent's message", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    await waitFor(() =>
      expect(screen.getByTestId("chat-reply")).toHaveTextContent(/Fixed the panic by adding a null check/)
    );
    expect(screen.getByText("0:45")).toBeInTheDocument();
  });

  it("shows a circular pulsing thinking state before the stream starts", async () => {
    const pending = deferred<unknown>();
    const user = await renderApp(makePayload(), [], { send_handoff: pending.promise });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    const inflight = await screen.findByTestId("chat-inflight");
    expect(within(inflight).getByText(/Thinking/)).toBeInTheDocument();
    expect(inflight.querySelector(".thinking-dots")).not.toBeNull();
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
    expect(document.querySelectorAll(".chat-message")).toHaveLength(1);
  });

  it("shows only the thinking state while a message is in flight", async () => {
    const pending = deferred<unknown>();
    const user = await renderApp(makePayload(), [], { send_handoff: pending.promise });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");

    const inflight = await screen.findByTestId("chat-inflight");
    expect(within(inflight).getByText(/Codex is working/)).toBeInTheDocument();
    tauri.listeners["handoff:started"]({ payload: { id: "handoff-0", prompt_len: 128 } });
    tauri.listeners["handoff:output"]({
      payload: { id: "handoff-0", stream: "stdout", chunk: "Working through" },
    });
    await waitFor(() =>
      expect(screen.getByTestId("chat-activity-output-volume")).toHaveTextContent("15 B")
    );
    const thinking = within(inflight).getByTestId("chat-thinking");
    expect(thinking).toHaveTextContent("Thinking…");
    expect(thinking).not.toHaveTextContent(/Working through/);
    const activity = within(inflight).getByTestId("chat-activity");
    expect(activity).not.toHaveAttribute("open");
    expect(within(inflight).getByText(/Working through/).closest('[data-testid="chat-activity"]')).toBe(
      activity
    );

    pending.resolve(makeOutcome());
    await waitFor(() => expect(screen.queryByTestId("chat-inflight")).not.toBeInTheDocument());
  });

  it("switching agents in the chat header switcher swaps the thread in place", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("first conversation");
    await user.keyboard("{Enter}");
    await waitFor(() =>
      expect(screen.getByTestId("chat-reply")).toHaveTextContent(/Fixed the panic by adding a null check/)
    );

    await user.click(screen.getByTestId("chat-agent-switcher"));
    const menu = await screen.findByRole("menu", { name: "Switch agent" });
    await user.click(within(menu).getByRole("menuitem", { name: /Hermes/ }));
    await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
    expect(screen.getByText(/Say hello to Hermes/)).toBeInTheDocument();
    expect(within(screen.getByTestId("chat-thread")).queryByText(/Fixed the panic/)).not.toBeInTheDocument();
  });
});

describe("palette — handoff activity (real measurements, no guessing)", () => {
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

    tauri.listeners["handoff:started"]({ payload: { id: "handoff-9", prompt_len: 2048 } });
    const activity = await screen.findByTestId("chat-activity");
    await waitFor(() =>
      expect(screen.getByTestId("chat-activity-prompt-size")).toHaveTextContent("2.0 KB")
    );
    expect(screen.getByTestId("chat-activity-first-response")).toHaveTextContent("—");
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

    tauri.listeners["handoff:output"]({
      payload: { id: "handoff-9", stream: "stdout", chunk: "Working through it" },
    });
    await waitFor(() =>
      expect(screen.getByText("Agent responding")).toHaveAttribute("data-active", "true")
    );
    expect(screen.getByTestId("chat-activity-first-response")).not.toHaveTextContent("—");
    expect(screen.getByTestId("chat-activity-output-volume")).toHaveTextContent("18 B");
    expect(screen.getByTestId("chat-activity-output")).toHaveTextContent("Working through it");

    pending.resolve(makeOutcome());
    await waitFor(() => expect(screen.queryByTestId("chat-inflight")).not.toBeInTheDocument());
    const steps = screen.getByTestId("chat-activity-0-phases");
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
      expect(screen.getByTestId("chat-reply")).toHaveTextContent(/Fixed the panic by adding a null check/)
    );
    const activity = screen.getByTestId("chat-activity-0");
    expect(within(activity).getByText(/Fixed the panic/)).toBeInTheDocument();
    expect(screen.getByTestId("chat-activity-0-output-volume")).toHaveTextContent(/B|KB/);
  });

  it("shows your message in a clean bubble without a label or prompt disclosure", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    await waitFor(() =>
      expect(screen.getByTestId("chat-reply")).toHaveTextContent(/Fixed the panic by adding a null check/)
    );

    const message = document.querySelector(".chat-message.chat-bubble") as HTMLElement;
    expect(message).toHaveTextContent(/^panic: something exploded$/);
    expect(screen.queryByText("You")).not.toBeInTheDocument();
    expect(screen.queryByText("Prompt that was sent")).not.toBeInTheDocument();
  });

  it("copies a reply and retries the same message from the line under it", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");
    await user.click(composer);
    await user.keyboard("what's next");
    await user.keyboard("{Enter}");
    await waitFor(() =>
      expect(screen.getByTestId("chat-reply")).toHaveTextContent(/Fixed the panic by adding a null check/)
    );
    const meta = screen.getByTestId("reply-meta-0");

    await user.click(within(meta).getByRole("button", { name: /Copy/ }));
    const copies = tauri.invoke.mock.calls.filter((c) => c[0] === "copy_text");
    expect(copies[copies.length - 1]?.[1]).toEqual({
      text: "Fixed the panic by adding a null check.\nDone.",
    });
    expect(within(meta).getByRole("button", { name: /Copied/ })).toBeInTheDocument();

    const before = sentCalls().length;
    await user.click(within(meta).getByRole("button", { name: /Retry/ }));
    await waitFor(() => expect(sentCalls().length).toBe(before + 1));
    const sent = sentCalls();
    const retried = sent[sent.length - 1]?.[1] as { capture: { content: { text: string } } };
    expect(retried.capture.content.text).toBe("panic: something exploded");
  });

  it("does not carry the previous handoff's prompt size into the next one", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");

    await user.click(composer);
    await user.keyboard("first");
    await user.keyboard("{Enter}");
    await screen.findByTestId("chat-inflight");
    tauri.listeners["handoff:started"]({ payload: { id: "handoff-1", prompt_len: 4096 } });
    await waitFor(() =>
      expect(screen.getByTestId("chat-activity-prompt-size")).toHaveTextContent("4.0 KB")
    );
    await waitFor(() => expect(screen.queryByTestId("chat-inflight")).not.toBeInTheDocument());

    await user.click(composer);
    await user.keyboard("second");
    await user.keyboard("{Enter}");
    await screen.findByTestId("chat-inflight");
    expect(screen.getByTestId("chat-activity-prompt-size")).toHaveTextContent("—");
  });

  it("drops chunks from a stale handoff that arrive after the next send began", async () => {
    const user = await renderApp(makePayload(), [], { send_handoff: makeOutcome() });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");

    await user.click(composer);
    await user.keyboard("first");
    await user.keyboard("{Enter}");
    await screen.findByTestId("chat-inflight");
    tauri.listeners["handoff:started"]({ payload: { id: "handoff-1", prompt_len: 128 } });
    await waitFor(() => expect(screen.queryByTestId("chat-inflight")).not.toBeInTheDocument());

    await user.click(composer);
    await user.keyboard("second");
    await user.keyboard("{Enter}");
    await screen.findByTestId("chat-inflight");

    tauri.listeners["handoff:output"]({
      payload: { id: "handoff-1", stream: "stdout", chunk: "STALE OUTPUT" },
    });
    await waitFor(() =>
      expect(screen.getByTestId("chat-activity-output-volume")).not.toHaveTextContent("STALE")
    );
    expect(screen.queryByText("STALE OUTPUT")).not.toBeInTheDocument();

    tauri.listeners["handoff:started"]({ payload: { id: "handoff-2", prompt_len: 128 } });
    tauri.listeners["handoff:output"]({
      payload: { id: "handoff-2", stream: "stdout", chunk: "FRESH OUTPUT" },
    });
    await waitFor(() =>
      expect(screen.getByTestId("chat-activity-output-volume")).toHaveTextContent(/12 B/)
    );
    await waitFor(() => expect(screen.queryByTestId("chat-inflight")).not.toBeInTheDocument());
  });

  it("gives every turn its own activity panel with its own measurements", async () => {
    const first = deferred<unknown>();
    const user = await renderApp(makePayload(), [], { send_handoff: first.promise });
    await chooseAgent(user, /Codex/);
    const composer = screen.getByLabelText("Chat message");

    await user.click(composer);
    await user.keyboard("first");
    await user.keyboard("{Enter}");
    await screen.findByTestId("chat-inflight");
    tauri.listeners["handoff:started"]({ payload: { id: "handoff-1", prompt_len: 1024 } });
    tauri.listeners["handoff:output"]({
      payload: { id: "handoff-1", stream: "stdout", chunk: "FIRST REPLY" },
    });
    first.resolve(makeOutcome({ id: "handoff-1", receipt: { ok: true, detail: "ok", duration_ms: 1000, stdout: "FIRST REPLY", stderr: "" } }));
    await waitFor(() => expect(screen.queryByTestId("chat-inflight")).not.toBeInTheDocument());

    const second = deferred<unknown>();
    tauri.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "send_handoff") return second.promise;
      if (cmd === "palette_payload") return makePayload();
      if (cmd === "recent_handoffs") return [];
      if (["notify_result", "hide_palette", "copy_text", "set_palette_height", "open_settings_window", "clear_history"].includes(cmd)) return undefined;
      if (cmd === "read_dropped_files") return [];
      throw new Error(`unhandled invoke: ${cmd}`);
    });
    await user.click(composer);
    await user.keyboard("second");
    await user.keyboard("{Enter}");
    await screen.findByTestId("chat-inflight");
    tauri.listeners["handoff:started"]({ payload: { id: "handoff-2", prompt_len: 3072 } });
    tauri.listeners["handoff:output"]({
      payload: { id: "handoff-2", stream: "stdout", chunk: "SECOND REPLY, LONGER" },
    });
    second.resolve(makeOutcome({ id: "handoff-2", receipt: { ok: true, detail: "ok", duration_ms: 2000, stdout: "SECOND REPLY, LONGER", stderr: "" } }));
    await waitFor(() => expect(screen.queryByTestId("chat-inflight")).not.toBeInTheDocument());

    expect(screen.getByTestId("chat-activity-0")).toBeInTheDocument();
    expect(screen.getByTestId("chat-activity-1")).toBeInTheDocument();
    expect(screen.getByTestId("chat-activity-0-prompt-size")).toHaveTextContent("1.0 KB");
    expect(screen.getByTestId("chat-activity-1-prompt-size")).toHaveTextContent("3.0 KB");
    expect(screen.getByTestId("chat-activity-0-output")).toHaveTextContent("FIRST REPLY");
    expect(screen.getByTestId("chat-activity-0-output")).not.toHaveTextContent("SECOND REPLY");
    expect(screen.getByTestId("chat-activity-1-output")).toHaveTextContent("SECOND REPLY, LONGER");
  });
});

describe("palette — blocked sessions (approve/deny, explicit only)", () => {
  it("flags a blocked agent in the unfolded list with its pending question", async () => {
    await renderApp(
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
    expect(screen.getByText(/blocked · approve\?/)).toBeInTheDocument();
    expect(screen.getByTitle("[permission] approve shell command: deploy?")).toBeInTheDocument();
    expect(sentCalls()).toHaveLength(0);
    expect(tauri.invoke).not.toHaveBeenCalledWith(
      "approve_session",
      expect.objectContaining({ agentId: "hermes" })
    );
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
    expect(screen.getByTestId("chat-agent-switcher")).toHaveTextContent("Hermes");
  });

  it("retry re-opens a chat pre-filled with the original context", async () => {
    const user = await renderApp(makePayload(), [makeOutcome()]);
    await user.click(screen.getByRole("button", { name: "Recent handoffs" }));
    await waitFor(() => expect(screen.getByTestId("history-list")).toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: "Retry handoff-1" }));
    await waitFor(() => expect(screen.getByTestId("chat")).toBeInTheDocument());
    expect(screen.getByLabelText("Chat message")).toHaveValue("panic: something exploded");
  });

  it("'Another agent' reopens the unfolded picker with the context pre-filled", async () => {
    const user = await renderApp(makePayload(), [makeOutcome()]);
    await user.click(screen.getByRole("button", { name: "Recent handoffs" }));
    await waitFor(() => expect(screen.getByTestId("history-list")).toBeInTheDocument());
    await user.click(screen.getByTestId("history-open-handoff-1"));
    await waitFor(() => expect(screen.getByTestId("result-panel")).toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: /Another agent/ }));
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
    tauri.listeners["palette:open"]({ payload: null });
    await waitFor(() => expect(screen.getByTestId("welcome")).toBeInTheDocument());
    expect(screen.getByText("Welcome to Handover")).toBeInTheDocument();

    tauri.listeners["palette:open"]({ payload: null });
    await waitFor(() => expect(screen.queryByTestId("welcome")).not.toBeInTheDocument());
    expect(localStorage.getItem("handover.welcome.seen")).toBe("1");
    expect(screen.getByTestId("agent-filter")).toBeInTheDocument();
    expect(screen.getByRole("listbox", { name: "Agents" })).toBeInTheDocument();
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

    const addCodex = screen.getByRole("button", { name: /Add Codex/ });
    expect(addCodex).toBeEnabled();
    expect(screen.getByRole("button", { name: /OMP/ })).toBeDisabled();

    await user.click(addCodex);
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("configure_agent", { id: "codex", binaryPath: null })
    );
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
