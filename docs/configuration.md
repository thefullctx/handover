# Configuration

## Agents

Configure agents in the platform config file (created on first run):

| Platform | Path |
|---|---|
| **macOS** | `~/Library/Application Support/handover/config.toml` |
| **Linux** | `~/.config/handover/config.toml` |
| **Windows** | `%APPDATA%\handover\config.toml` |

Example:

```toml
[[agents]]
id = "hermes"
name = "Hermes"
kind = "command"
command = "hermes --prompt \"{PROMPT}\""
description = "Local coding agent"
working_dir = "~/Projects"
timeout_secs = 300

[[agents]]
id = "claude"
name = "Claude Code"
kind = "command"
command = "claude -p \"{PROMPT_FILE}\""
```

Placeholder behavior:

- `{PROMPT}` — replaced with the rendered prompt (shell-quoted).
- `{PROMPT_FILE}` — replaced with the path to a temp file containing the prompt (for agents
  that accept a file).
- If the command contains neither placeholder, the prompt is piped on stdin.

### Adding an agent

The desktop app has a built-in agent list — no config editing needed. Open
**Settings → Agents** (menu bar, `⌘,`, tray menu, or the palette gear):

1. **Pick a known agent.** Handover ships with known agents (Hermes, Codex,
   OpenCode, OMP) and automatically detects which are installed on your Mac.
   Each agent is one card with a quiet status line:
   - `Installed · signed in` / `Installed · not signed in` → one click on **Add**
     writes the recommended command (including session discovery + resume
     support) into your config. The sign-in probe only checks for each agent's
     own credential file (e.g. `~/.codex/auth.json`) — Handover never reads or
     sends credentials. *Not signed in* means: run that CLI's login command once.
   - `Not detected` → install the agent first, or click **Configure…** and
     enter its binary path once.
2. Or click **Add custom agent…** for anything else and fill in the form:
   - **Name** — display name (e.g. `Hermes`).
   - **id** — lowercase letters, digits and dashes (e.g. `hermes`); used by preferences and the CLI.
   - **Command** — how to invoke the agent, using `{PROMPT}` / `{PROMPT_FILE}` / stdin (see placeholder behavior above).
   - **Description / Working directory / Timeout (seconds)** — optional but recommended. Give
     long-running agents (Hermes, Codex, OpenCode, …) a generous timeout (`300`+); the default
     is 120s.
3. **Save agent** — it is persisted to `config.toml` and available immediately, no restart.

First-run onboarding includes the same list: detected agents appear right in
the welcome overlay with a one-tap **Add**, so a new user goes from launch to a
working palette without ever opening Settings.

Every added card carries an **in-palette toggle switch**: ON = it appears in
the palette's agent dropdown, OFF = hidden while keeping its full config
(command, env, preferences) — parking an agent never deletes anything. The ⋯
menu on each card holds Make default, Detect session, View command (commands
are hidden by default), and Remove. The built-in demo agent sits muted at the
bottom of the list.

You can do the same by editing the config file (example above) and relaunching the app. If an
`[[agents]]` block fails validation (e.g. a `resume_command` without `{SESSION}`, or both
`session_glob` and `session_cli_list` set), Handover skips that agent and logs a warning —
the rest of the config still loads. The
first enabled agent is the default. The built-in `demo` agent only echoes handoffs into a
private file (`~/.handover/demo-handoff.txt`) — it exists so the loop can be verified
end-to-end without a real agent; once you add your own, it stops being the default.

## Actions

The palette is **chat-first**: there are no action buttons — a message goes to the agent
verbatim. The configurable action templates below still exist for the CLI/API
(`handover send --action fix …`), and chat messages are delivered via the `ask` template
(verbatim — no preamble is added).

Actions are configurable prompt templates, not hard-coded strings sprinkled through the UI.

| id | name | prompt |
|---|---|---|
| `fix` | Fix this | Investigate this issue and fix it. |
| `investigate` | Investigate | Investigate this issue and determine the root cause. Do not modify files yet. |
| `explain` | Explain | Explain this clearly and identify the likely cause. |
| `write_tests` | Write tests | Create appropriate tests for this issue. |
| `ask` | Ask | Analyze the provided context and tell me what you recommend. |

## Config file

All configuration lives in the platform config directory (see table above; created with
sensible defaults on first run). Structure:

```toml
[general]
shortcut = "CmdOrCtrl+Shift+A"   # Settings → General (⌘⇧A / Super+Shift+A by default)
launch_at_startup = false        # Settings → General (autostart plugin)
quick_send = true                # legacy: the chat-first palette has no agent picker to skip (kept for config compat)
notifications = true             # desktop banners for handoff milestones
appearance = "system"            # "system" | "light" | "dark"
ui_opacity = 1.0                 # palette + Settings glass opacity (0.4–1.0); Settings slider
history_retention_days = 7       # planned: persisted history/SQLite (session history exists in-memory)

[[agents]]
# ... as above ...

[preferences]                    # action id -> preferred agent id
fix = "hermes"
explain = "qwen-local"

[privacy]
excluded_paths = [".env", ".env.*", "*.pem", "*.key", "*.p12", "*.pfx", "credentials*", "id_rsa", "id_ed25519", "~/.ssh/*"]
excluded_apps = []               # planned: frontmost-app filtering not implemented yet
```

### Session-aware agent fields

Session-aware agents (`kind = "session"`) accept the additive fields below — a plain
`command` agent is unaffected by their absence:

```toml
[[agents]]
id = "codex"
name = "Codex"
kind = "session"
command = "codex exec \"{PROMPT}\""
# Discovery: glob over session transcripts (filename → session id).
session_glob = "~/.codex/sessions/*/*/*/*.jsonl"
# Hermes (SQLite) has no glob — list sessions via a CLI command instead.
# session_cli_list = ["hermes", "sessions", "list", "--source", "cli", "--limit", "20"]
# Resume command: must contain {SESSION}.
resume_command = "codex exec resume {SESSION} \"{PROMPT}\""
# Approval layer (opt-in): tail-peek marker + inject channel + target.
# permission_marker = "[permission]"
# approval_channel = "tty"        # "tty" | "tmux" | "agent" (agent unimplemented)
# approval_target = "/dev/ttys002"  # set automatically by `handover attach`
timeout_secs = 300
```

### Local API token

Mutating HTTP routes (`POST /send`, `/preferences`, `/quit`, …) require a bearer token.
On first run Handover writes a private token file next to the config:

| Platform | Token file |
|---|---|
| **macOS** | `~/Library/Application Support/handover/api_token` |
| **Linux** | `~/.config/handover/api_token` |

The CLI reads this file automatically (or use `HANDOVER_TOKEN`). `/health` stays open
without auth for process probes. The desktop app talks to the daemon in-process and does
not need the token for palette handoffs.

For long handoffs, the CLI waits for the daemon’s max agent `timeout_secs` plus 60 seconds
of grace (at least 180s). Override with `HANDOVER_TIMEOUT_SECS` (minimum 30).

← [Back to the README](../README.md)
