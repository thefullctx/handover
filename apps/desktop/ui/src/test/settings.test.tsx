import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import SettingsApp from "../Settings";
import GeneralSection from "../settings/GeneralSection";
import { makeAgentConfig, makeInfo, makeSession } from "./fixtures";

const tauri = vi.hoisted(() => {
  const invoke = vi.fn(
    async (_cmd: string, ..._: unknown[]): Promise<unknown> => {
      throw new Error(`unhandled invoke: ${_cmd}`);
    }
  );
  const listen = vi.fn(async () => () => {});
  return {
    invoke,
    listen,
    getPalettePayload: () => invoke("palette_payload"),
    getRecentHandoffs: () => invoke("recent_handoffs"),
    sendHandoff: (actionId: string, agentId: string, capture: unknown) =>
      invoke("send_handoff", { actionId, agentId, capture }),
    rememberPreference: (actionId: string, agentId: string) =>
      invoke("set_preference", { actionId, agentId }),
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
    setUiOpacity: (opacity: number) => invoke("set_ui_opacity", { opacity }),
    setShortcut: (shortcut: string) => invoke("set_shortcut", { shortcut }),
    setLaunchAtStartup: (enabled: boolean) => invoke("set_launch_at_startup", { enabled }),
    setExcludedPaths: (patterns: string[]) => invoke("set_excluded_paths", { patterns }),
    revealConfig: () => invoke("reveal_config"),
    takeSettingsTab: () => invoke("take_settings_tab"),
    getLiveSessions: () => invoke("live_sessions"),
  };
});
vi.mock("../lib/tauri", () => tauri);

function installSettingsMock(agents = [makeAgentConfig()]) {
  tauri.invoke.mockImplementation(async (cmd: string, args?: unknown) => {
    switch (cmd) {
      case "app_info":
        return makeInfo();
      case "configured_agents":
        return agents;
      case "available_agents":
        return [];
      case "take_settings_tab":
        return null;
      case "add_agent":
      case "remove_agent":
      case "set_default_agent":
      case "set_agent_enabled":
      case "set_shortcut":
      case "set_launch_at_startup":
      case "set_notifications":
      case "set_quick_send":
      case "set_appearance":
      case "set_ui_opacity":
        // Mirrors the real backend: returns the (clamped) value it persisted.
        return cmd === "set_ui_opacity"
          ? (args as { opacity?: number } | undefined)?.opacity ?? 0.8
          : undefined;
      case "set_excluded_paths":
      case "reveal_config":
        return undefined;
      default:
        throw new Error(`unhandled invoke: ${cmd}`);
    }
  });
  tauri.invoke.mockClear();
}

describe("Settings — agent setup (initial setup flow)", () => {
  it("opens the Agents tab and adds a custom agent through the form", async () => {
    installSettingsMock([]);
    const user = userEvent.setup();
    render(<SettingsApp />);
    await waitFor(() => expect(screen.getByText("How Handover behaves")).toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: /Agents/ }));
    await user.click(screen.getByRole("button", { name: /Add custom agent/ }));

    await user.type(screen.getByLabelText(/^Name/), "My Agent");
    await user.type(screen.getByLabelText(/^id/), "my-agent");
    await user.type(screen.getByLabelText(/^Command/), "my-agent --prompt PROMPT");
    await user.click(screen.getByRole("button", { name: /Save agent/ }));

    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith(
        "add_agent",
        expect.objectContaining({ id: "my-agent", name: "My Agent", command: "my-agent --prompt PROMPT" })
      )
    );
  });

  it("validates the custom-agent id format and never submits", async () => {
    installSettingsMock([]);
    const user = userEvent.setup();
    render(<SettingsApp />);
    await waitFor(() => expect(screen.getByText("How Handover behaves")).toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: /Agents/ }));
    await user.click(screen.getByRole("button", { name: /Add custom agent/ }));
    await user.type(screen.getByLabelText(/^Name/), "Bad Id");
    await user.type(screen.getByLabelText(/^id/), "Bad Id!");
    await user.type(screen.getByLabelText(/^Command/), "cat");
    await user.click(screen.getByRole("button", { name: /Save agent/ }));

    expect(
      screen.getByText("id may only contain lowercase letters, digits and dashes.")
    ).toBeInTheDocument();
    expect(tauri.invoke).not.toHaveBeenCalledWith("add_agent", expect.anything());
  });

  it("applies the opacity slider live and persists the clamped value", async () => {
    installSettingsMock();
    const user = userEvent.setup();
    render(<SettingsApp />);
    await waitFor(() => expect(screen.getByText("How Handover behaves")).toBeInTheDocument());

    // Starts at the persisted value (fixture ui_opacity 1 → 100%).
    const slider = screen.getByLabelText("Window opacity");
    expect(slider).toHaveValue("100");

    fireEvent.change(slider, { target: { value: "60" } });
    // Instant: this window's CSS var updates and the label reads 60%.
    expect(document.documentElement.style.getPropertyValue("--ui-opacity")).toBe("0.6");
    expect(screen.getByText("60%")).toBeInTheDocument();

    // The debounced persist reaches the backend with the clamped value — the
    // same value the palette receives via the ui-opacity:changed broadcast.
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("set_ui_opacity", { opacity: 0.6 })
    );
    void user;
  });

  it("removes an agent only after a confirming second click (⋯ menu)", async () => {
    installSettingsMock();
    const user = userEvent.setup();
    render(<SettingsApp />);
    await waitFor(() => expect(screen.getByText("How Handover behaves")).toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: /Agents/ }));
    // Remove lives inside the card's overflow menu.
    await user.click(await screen.findByRole("button", { name: "More actions for My Agent" }));
    await user.click(screen.getByRole("menuitem", { name: /Remove/ }));
    // Armed — not yet removed.
    expect(tauri.invoke).not.toHaveBeenCalledWith("remove_agent", expect.anything());
    await user.click(await screen.findByRole("menuitem", { name: /Confirm remove/ }));
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("remove_agent", { id: "my-agent" })
    );
  });

  it("toggles an agent's palette visibility and persists it (optimistic)", async () => {
    // Mutable mirror of the daemon's agent list — the real backend returns
    // the updated state on the next configured_agents call after a toggle.
    const agents = [makeAgentConfig()];
    installSettingsMock(agents);
    tauri.invoke.mockImplementation(async (cmd: string, invokeArgs?: unknown) => {
      switch (cmd) {
        case "app_info":
          return makeInfo();
        case "configured_agents":
          return agents;
        case "available_agents":
          return [];
        case "take_settings_tab":
          return null;
        case "set_agent_enabled": {
          const { id, enabled } = invokeArgs as { id: string; enabled: boolean };
          const a = agents.find((x) => x.id === id);
          if (a) a.enabled = enabled;
          return undefined;
        }
        default:
          throw new Error(`unhandled invoke: ${cmd}`);
      }
    });
    const user = userEvent.setup();
    render(<SettingsApp />);
    await waitFor(() => expect(screen.getByText("How Handover behaves")).toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: /Agents/ }));
    const toggle = await screen.findByRole("switch", { name: "My Agent in palette" });
    expect(toggle).toBeChecked();

    // OFF → hidden from the palette, config kept. The backend gets the new state.
    await user.click(toggle);
    expect(toggle).not.toBeChecked(); // optimistic flip
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("set_agent_enabled", {
        id: "my-agent",
        enabled: false,
      })
    );

    // And back on.
    await user.click(toggle);
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("set_agent_enabled", {
        id: "my-agent",
        enabled: true,
      })
    );
  });

  it("rolls the visibility toggle back when the daemon rejects the change", async () => {
    installSettingsMock();
    tauri.invoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "app_info":
          return makeInfo();
        case "configured_agents":
          return [makeAgentConfig()];
        case "available_agents":
          return [];
        case "take_settings_tab":
          return null;
        case "set_agent_enabled":
          throw new Error("daemon exploded");
        default:
          throw new Error(`unhandled invoke: ${cmd}`);
      }
    });
    const user = userEvent.setup();
    render(<SettingsApp />);
    await waitFor(() => expect(screen.getByText("How Handover behaves")).toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: /Agents/ }));
    const toggle = await screen.findByRole("switch", { name: "My Agent in palette" });
    await user.click(toggle);
    // The optimistic flip is undone and the error surfaces.
    await waitFor(() => expect(toggle).toBeChecked());
    expect(await screen.findByText(/daemon exploded/)).toBeInTheDocument();
  });

  it("shows signed-in status lines from the gallery's read-only credential probe", async () => {
    installSettingsMock([]);
    tauri.invoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "app_info":
          return makeInfo();
        case "configured_agents":
          return [];
        case "available_agents":
          return [
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
          ];
        case "take_settings_tab":
          return null;
        default:
          throw new Error(`unhandled invoke: ${cmd}`);
      }
    });
    const user = userEvent.setup();
    render(<SettingsApp />);
    await waitFor(() => expect(screen.getByText("How Handover behaves")).toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: /Agents/ }));
    expect(await screen.findByTestId("agent-card-codex")).toBeInTheDocument();
    // One quiet status line per card — no chip soup.
    expect(screen.getByText("Installed · signed in")).toBeInTheDocument();
    expect(screen.getByText("Not detected")).toBeInTheDocument();
    // Not-detected cards offer Configure… instead of Add.
    expect(screen.queryByText("not signed in")).not.toBeInTheDocument();
  });

  it("keeps commands hidden until View command expands them", async () => {
    installSettingsMock();
    const user = userEvent.setup();
    render(<SettingsApp />);
    await waitFor(() => expect(screen.getByText("How Handover behaves")).toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: /Agents/ }));
    const cmd = 'my-agent --prompt "{PROMPT}"';
    expect(screen.queryByText(cmd)).not.toBeInTheDocument();

    await user.click(await screen.findByRole("button", { name: "More actions for My Agent" }));
    await user.click(screen.getByRole("menuitem", { name: /View command/ }));
    expect(await screen.findByTestId("cmd-details-my-agent")).toBeInTheDocument();
    expect(screen.getByText(cmd)).toBeInTheDocument();
  });
});

describe("Settings — session-aware agents", () => {
  it("shows session config and detects live sessions on demand", async () => {
    const sessionAgent = makeAgentConfig({
      id: "codex",
      name: "Codex",
      kind: "session",
      command: "codex exec \"{PROMPT}\"",
      session_glob: "~/.codex/sessions/*/*/*/*.jsonl",
      resume_command: "codex exec resume {SESSION} \"{PROMPT}\"",
    });
    installSettingsMock([sessionAgent]);
    tauri.invoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "app_info":
          return makeInfo();
        case "configured_agents":
          return [sessionAgent];
        case "available_agents":
          return [];
        case "take_settings_tab":
          return null;
        case "live_sessions":
          return [
            makeSession({ agent_id: "codex", activity: "working" }),
            makeSession({ agent_id: "codex", activity: "idle", session_id: "old-1" }),
          ];
        default:
          throw new Error(`unhandled invoke: ${cmd}`);
      }
    });
    const user = userEvent.setup();
    render(<SettingsApp />);
    await waitFor(() => expect(screen.getByText("How Handover behaves")).toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: /Agents/ }));

    // Session plumbing is out of sight: the card shows a quiet status line,
    // and Detect session lives in the ⋯ menu.
    expect(screen.getByText(/In palette/)).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "More actions for Codex" }));
    await user.click(screen.getByRole("menuitem", { name: /Detect session/ }));

    await waitFor(() => expect(tauri.invoke).toHaveBeenCalledWith("live_sessions"));
    expect(await screen.findByText(/live ·/)).toBeInTheDocument();
    expect(screen.getByText(/last ·/)).toBeInTheDocument();
  });

  it("hides the Detect menu item for plain command agents", async () => {
    installSettingsMock();
    const user = userEvent.setup();
    render(<SettingsApp />);
    await waitFor(() => expect(screen.getByText("How Handover behaves")).toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: /Agents/ }));
    await user.click(await screen.findByRole("button", { name: "More actions for My Agent" }));
    expect(screen.queryByRole("menuitem", { name: /Detect session/ })).not.toBeInTheDocument();
    expect(tauri.invoke).not.toHaveBeenCalledWith("live_sessions");
  });
});

describe("Settings — General controls", () => {
  function Harness() {
    const [info, setInfo] = useState(makeInfo());
    const [error, setError] = useState("");
    return (
      <>
        <GeneralSection info={info} onInfoChange={(p) => setInfo((i) => ({ ...i, ...p }))} onError={setError} />
        {error && <p data-testid="general-error">{error}</p>}
      </>
    );
  }

  it("records a new global shortcut and persists it", async () => {
    tauri.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "set_shortcut") return undefined;
      throw new Error(`unhandled: ${cmd}`);
    });
    const user = userEvent.setup();
    render(<Harness />);
    expect(screen.getByTestId("shortcut-display")).toHaveTextContent("⌘⇧A");

    await user.click(screen.getByRole("button", { name: /Change/ }));
    await user.click(screen.getByTestId("shortcut-record"));
    await user.keyboard("{Control>}K{/Control}");

    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith(
        "set_shortcut",
        expect.objectContaining({ shortcut: expect.stringContaining("K") })
      )
    );
  });

  it("toggles launch-at-startup and notifications", async () => {
    tauri.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "set_launch_at_startup" || cmd === "set_notifications") return undefined;
      throw new Error(`unhandled: ${cmd}`);
    });
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole("switch", { name: /Launch at startup/ }));
    expect(tauri.invoke).toHaveBeenCalledWith("set_launch_at_startup", { enabled: true });

    await user.click(screen.getByRole("switch", { name: /Notifications/ }));
    expect(tauri.invoke).toHaveBeenCalledWith("set_notifications", { enabled: false });
  });

  it("lists keyboard shortcuts with the configured glyph", async () => {
    installSettingsMock();
    render(<SettingsApp />);
    await waitFor(() =>
      expect(screen.getByText("Keyboard shortcuts")).toBeInTheDocument()
    );
    expect(screen.getByText("Open the palette from anywhere")).toBeInTheDocument();
    // The cheat-sheet grid renders the *configured* shortcut (⌘⇧A from
    // CmdOrCtrl+Shift+A in the fixture) — not a hard-coded one.
    const grid = screen.getByLabelText("Keyboard shortcuts");
    expect(within(grid).getByText("⌘⇧A")).toBeInTheDocument();
  });
});
