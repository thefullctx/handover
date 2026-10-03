# Handover

**See something → one hotkey → send it to the right AI agent.**

Handover is a lightweight, local-first, keyboard-first handoff layer between you and your
existing AI agents. It is **not** an AI agent, a chat application, or an orchestration platform.
It is a native utility: you see an error, press a hotkey, pick the agent that is already
running, and keep chatting with it in a couple of keystrokes.

```
⌘⇧A  →  pick agent  →  chat
```

The target is 2–3 keystrokes from seeing a problem to a successful handoff.

---

## What Handover does

1. You see something on your computer (an error, a log line, a file, a URL, a screenshot).
2. You press the global hotkey (default `⌘⇧A` / `Super+Shift+A`).
3. Handover detects the agents on your machine in a dropdown — grouped **Running now**
   (process running / session actively writing) and **Available** (installed), with
   official logos and an honest status light: **green** means the agent's **process is
   actually running** (or its session is actively writing) — not merely that a session
   file exists; **red** means installed but idle. The lights update **live** while the
   palette is open (4-second refresh). An agent whose binary is fine but whose *model
   provider* is unreachable shows **amber** plus the reason — you learn a send would
   fail before wasting one.
4. You pick an agent (arrow keys + Enter, or click) and the chat opens directly beneath
   the dropdown — resuming the agent's live session when it has one.
5. The composer opens clean (nothing is captured from the clipboard); type a message
   and press `Enter` (`Shift+Enter` for a new line). Handover delivers the message
   verbatim into that session. Dropping text or a file onto the palette pre-fills the
   composer.
6. The reply streams back in the thread, and you get confirmation the handoff succeeded.

Everything runs locally. Nothing is uploaded anywhere unless the agent you choose does so.

**Supported platforms:** Handover is developed and verified on **macOS** and
**Linux**. Windows paths are reserved in the config layout for future support,
but the desktop app (tray, hotkey, status lights) is not yet supported there —
the live-process check and approval injection are Unix-specific.

## Installation

### Prerequisites

- **Rust** (stable): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node.js** 20+ and npm (for the desktop UI)
- **macOS**: Xcode Command Line Tools (`xcode-select --install`)
- **Linux**: Tauri system dependencies — see
  [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

### Build

```bash
# Core, daemon, and CLI
cargo build --workspace

# Desktop app (Tauri). This builds the React UI first, then compiles the app.
cd apps/desktop && ./ui/node_modules/.bin/tauri build
```

The CLI lands at `target/debug/handover`, the daemon at `target/debug/handover-daemon`,
and the desktop app at `target/debug/handover-desktop` (release builds at
`target/release/`, with the macOS bundle at
`target/release/bundle/macos/Handover.app`).

> `cargo build -p handover-desktop` on its own compiles only the Rust side. It does
> **not** rebuild the UI, so it will happily bundle a stale `dist/`. Use
> `tauri build` (or `tauri dev`) for anything that should pick up UI changes.

### Run

```bash
# 1. Start the desktop app (menu-bar/tray icon + global hotkey)
target/debug/handover-desktop

#    or run just the daemon from a terminal:
target/debug/handover-daemon

# 2. Use the menu-bar icon or press ⌘⇧A / Super+Shift+A for the palette.
#    (Ad-hoc local builds: if the hotkey fails after a rebuild, toggle
#     Accessibility for this .app once, then fully Quit and relaunch.)
```

## Basic usage

- **Hotkey**: press `⌘⇧A` (macOS) / `Super+Shift+A` (Linux) — the palette appears
  with a clean composer (the clipboard is never read).
- **Drag & drop**: drag a file or selected text onto the palette to capture it — text files
  are read locally (≤ 1 MB, binary refused), images hand off by path, and a drop overlay
  appears while dragging. Dropped context pre-fills the chat composer.
- **Onboarding & cheat sheet**: the first launch shows a one-screen welcome (dismiss it by
  pressing the hotkey again — taught by doing); press `?` in the palette or the ⌘-key icon
  in the footer for the keyboard cheat sheet, also listed under Settings → General.
- **Settings** — open from the menu bar (**Handover → Settings…**, `⌘,`), the tray icon
  (**Settings…**), or the palette gear. General (appearance, **window opacity**,
  shortcut, launch at startup, notifications, keyboard shortcuts), Agents,
  Privacy, and About. Settings is created on demand (not at launch). On macOS the window
  uses Overlay traffic-light chrome over the sidebar.
- **Pick an agent** with the arrow keys and press `Enter` (or click). Every row is
  logo + name + one status dot: **green** when the agent's process is actually running
  (or its session is actively writing), **red** when installed but idle, and **amber**
  with a reason line when the binary is fine but its model provider is unreachable —
  e.g. your local model server isn't up. A blocked agent keeps its
  `blocked · approve?` line (the pending question on hover); Approve/Deny live in the
  chat's approval card. The lights refresh every few seconds while the palette is open —
  start an agent in a terminal and watch it flip red→green without closing anything.
- **Chat** — the composer opens clean (the clipboard is never captured; dropped text/files
  pre-fill it, editable, never sent silently): `Enter` sends verbatim (`Shift+Enter` is a
  newline), the reply streams
  inline, and every message resumes the same session so the agent keeps the conversation
  context. While waiting you see a quiet thinking animation; CLI session banners are
  filtered out of the stream.

The first time you chat with an agent it becomes your default — next time you can go straight
through: `⌘⇧A → Enter → Enter`.

## Handoff feedback

You are never left wondering what happened after the handoff:

- **Sending** — the in-flight turn shows a quiet thinking animation and a live elapsed timer
  (`Codex is working · 12s`), with the reply streaming inline as it arrives. Press `esc` to dismiss
  the palette — the handoff keeps running and a **native macOS notification** still fires on
  completion (the banner belongs to Handover; clicking it activates the app, not Script Editor).
- **Activity** — a collapsible **Show activity** section holds the raw agent output plus a phase
  timeline (Sending context → Agent responding → Wrapping up) and real measurements: the rendered
  **prompt size**, **time to first response**, and live **output volume**. The phase is *derived
  from the output stream*, not guessed: no output yet is "Sending context", flowing output is
  "Agent responding", and output that goes quiet for 3s is "Wrapping up" — reverting the moment
  output resumes, so a slow agent never looks finished. Every number is measured (prompt bytes in
  Rust, response/output timings from the event stream); nothing is estimated.
- **Completed** — the turn stays in the thread: the outcome (success/failure, agent, duration,
  human title) with the agent's **answer** as readable text, and raw stdout/stderr behind a
  disclosure. You can keep chatting in the same thread instead of starting over. The full
  **result panel** (Copy answer · Copy prompt · Send follow-up · Repeat with another agent ·
  Close) is one click away from *Recent handoffs*. Errors are plain-language and offer
  **Retry** — never raw stack traces.
- **Recent handoffs** — the tray/menu-bar menu has a *Recent Handoffs* submenu (most recent 10,
  session-only), and the palette's history view groups entries by **Today / Yesterday** with
  per-row **open · retry · duplicate · copy result · send to another agent**, plus a **Clear
  history** action. History is in-memory and dies with the app; persisted history (SQLite +
  retention) is planned.

## Supported agents

| Agent | Kind | Notes |
|---|---|---|
| Generic command | `command` | Any CLI agent — **`{PROMPT}`** / **`{PROMPT_FILE}`** substitution, or stdin. This is the most important adapter: it supports Hermes, Claude Code, Codex, OpenCode, Qwen Local, or anything else that can be invoked from a shell. |
| Session-aware | `session` | Resume into the live sessions of Claude Code, Codex, Droid, Oh My Pi (omp) and Hermes — see *Live sessions & status* below. |
| OpenAI-compatible endpoint | `openai_compatible` | Generic HTTP endpoint (planned). Handover stays a client, never a server. |

## Live sessions & status

Handover is also a *window onto* your agents, not just a door into them. When an agent
stores its sessions on disk, Handover discovers them **from filenames + mtimes only** and
shows you where the work is:

- **Status**: the dropdown groups agents under **Running now** — an agent appears there
  only while its **process is actually running** (or its session is actively writing), never
  merely because a session file exists. Each row is logo + name + one status dot: **green**
  when running (bright while actively writing, steady when the process is open but quiet),
  **red** once the process is gone, **amber** when the binary is fine but its model provider
  is unreachable, plus `blocked · approve?` when parked at a permission prompt. The lights
  refresh live while the palette is open.
- **Resume by id**: sending into a live session runs the agent's *resume* command with the
  session id (`codex exec resume <id> …`, `omp -p … -r <id>`, `hermes chat -Q -q … --reasoning none --resume <id>`,
  …) — the reply lands in the conversation you're already having, then comes back to the
  result panel like any handoff. Hermes runs with `-Q` (quiet) and `--reasoning none` so
  only the final response comes back — no banner, spinner, tool previews, or reasoning
  dump — and the palette strips any leftover CLI chrome (the `↪ restored workspace dir`
  notice, empty reasoning boxes, session-info footers) so the chat reads like a normal
   conversation. Resolution order: **explicit → pinned → freshest → fresh send**.
   A session owned by another live process (e.g. Hermes with its TUI open —
   "already has a live owner") refuses a second connection: auto-resolved
   sends fall back to a fresh session with a note saying so, while explicit
   and pinned targets fail plainly instead of landing somewhere unexpected.
- **Zero config**: the built-in catalog overlays discovery for claude / codex / droid / omp /
  hermes even when they are not in your config; configure one with the same id to override.
- **Explicit, never silent**: the live-session target is always shown before dispatch. A
  session that is *quiet* is labeled `last session`, never `live`.
- **Pinning**: `handover attach <agent>` (run inside the session) pins it so handoffs keep
  resuming into it; `handover attach --unpin` restores freshest-first.

Discovery per agent (verified on real agents):

| Agent | Session storage | Resume (headless) |
|---|---|---|
| **Claude Code** | `~/.claude/projects/<proj>/<uuid>.jsonl` | `claude -p "{PROMPT}" --resume {SESSION}` |
| **Codex** | `~/.codex/sessions/<Y>/<M>/<D>/rollout-<ts>-<uuid>.jsonl` | `codex exec resume {SESSION} --skip-git-repo-check "{PROMPT}"` |
| **Droid** | `~/.factory/sessions/<enc-cwd>/<uuid>.jsonl` | `droid exec -s {SESSION} "{PROMPT}"` |
| **Oh My Pi (omp)** | `~/.omp/agent/sessions/<enc-cwd>/<ts>_<id>.jsonl` | `omp -p "{PROMPT}" -r {SESSION}` |
| **Hermes** | SQLite (no glob) → `hermes sessions list` | `hermes chat -Q -q "{PROMPT}" --reasoning none --resume {SESSION}` — Handover ships a wrapper (`scripts/hermes-resume.sh`) that reads the session's actual provider + model from Hermes' `state.db` and passes both explicitly; plain `--resume` restores the model but keeps the ambient provider, which fails when they differ |

### Approvals (opt-in)

When an agent parks at a permission prompt, it is *quiet* — indistinguishable from idle by
mtime. For agents that opt in (`permission_marker` + `approval_channel` in config), Handover
reads **only the last few KB** of a live session's transcript looking for the marker — the
one documented privacy exception. When it finds the marker, the palette shows an approval
card with the agent's pending question (the marker line itself) and **Approve/Deny** — you
see *what* is being asked, not just that something is. Only the explicit button press
injects (a keystroke via `tty` or `tmux`), and the outcome is **verified**: success is
claimed only after the transcript is observed resuming; otherwise you get
`couldn't confirm — check the terminal`. This works for agents with session *files*
(codex / droid / omp); Hermes' approval prompts live in its TUI only.

`handover sessions` lists live sessions with their state (`working` / `idle` / `blocked`),
and `handover approve <agent> [id] [--deny]` acts from the CLI.

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
   - **Command** — how to invoke the agent, using `{PROMPT}` / `{PROMPT_FILE}` / stdin (below).
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

## CLI

The CLI is a thin client of the running daemon — it contains no application logic of its own.

```bash
handover status                 # daemon status + agent availability
handover agents                 # list configured agents
handover actions                # list available actions
handover send "Fix this error"  # send text
handover send ./error.log       # send a file
handover send ./screenshot.png  # send an image
cat error.log | handover        # pipe stdin (no subcommand = send)
```

Options for `send`:

```
--agent <id>        Agent to use (default: your per-action preference, else the default agent)
--action <id>       Action to use (default: ask)
--session <id>      Resume into this live session (overrides pinned/freshest)
--print-prompt      Print the rendered prompt without sending anything
```

Session commands:

```bash
handover sessions              # live sessions, freshest first, with activity/blocked state
handover attach <agent> [id]   # pin a session (run inside it; --tty records the approval target)
handover attach --unpin <agent># clear the pin (freshest-first resumes)
handover approve <agent> [id]  # approve a blocked session (--deny to deny)
```

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

## Configuration

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

## Privacy model

- **Local-first**: the daemon binds to `127.0.0.1` only. Nothing leaves your machine except
  what the agent you chose sends.
- **Local API auth**: mutating routes require the bearer token above so other local
  processes cannot silently trigger agents or quit the daemon.
- **Security exclusions**: files matching `privacy.excluded_paths` are **refused before being
  read** — `.env`, `*.pem`, `*.key`, credentials, `~/.ssh/*`, and anything else you add.
  Matching is case-insensitive. The final path component is opened with no-follow semantics
  (symlink leaves are refused). Intermediate directories are assumed trusted (typical
  single-user desktop); full component-wise `openat` hardening is not implemented yet.
- **No auto-uploads**: captures are never uploaded anywhere by Handover itself.
- **Size caps**: files larger than 1 MiB are not attached (path only, with a `file_note` in
  the prompt). CLI / stdin text larger than 1 MiB is truncated with a `text_note`.
- **No hidden scans**: Handover does not recursively scan projects or attach directories.
- **Session discovery is stat-only**: live sessions are found from filenames + mtimes —
  transcript contents are never read. The **one documented exception** is approval
  detection: agents that opt in with a `permission_marker` have only the last few KB of a
  live session's transcript tail-peeked for that marker; the buffer is dropped immediately
  and never stored or sent. A blocked session surfaces the marker line itself (shown on the
  palette's approval card) — the same single, narrow read, shown only to you in your own UI.

## Current status (V1 vertical slice)

Implemented and verified end-to-end:

- ✅ Background daemon with local HTTP API (`127.0.0.1:47444`)
- ✅ Global hotkey → chat-first palette (agent list → chat with the running agent); handoffs run off the UI thread
- ✅ Agent dropdown + chat merged surface: rows are logo + name + one status dot — green (process actually running / session actively writing), red (installed idle), amber (provider down, with reason); blocked sessions keep `blocked · approve?`; keyboard-first selection; lights refresh live while the palette is open
- ✅ Settings window (lazy-created; menu bar / `⌘,` / tray / palette gear) with Overlay traffic-light chrome: General (appearance, window opacity for palette + Settings, live shortcut re-register, launch at startup, notifications), Agents, Privacy, About
- ✅ Apple-native floating-glass UI: shared light/dark design tokens, SF Pro system
  font stack, large continuous radii, pill bars + circular controls, floating dock,
  metal H app icon + macOS template tray icon
- ✅ Standardized Capture object (drag-and-drop text/files, CLI/API captures) — the palette is chat-first and sends messages verbatim; the clipboard is never read
- ✅ Generic command agent adapter (`{PROMPT}`, `{PROMPT_FILE}`, stdin) + registry
- ✅ Concurrent agent stdout/stderr drain, output caps, process-group timeout (Unix)
- ✅ Chat delivers messages verbatim into the resumed session (`ask` template, no preamble) — the composer opens clean, dropped text/files pre-fill it, and nothing sends without an explicit `Enter`
- ✅ Live handoff feedback: stream-derived phase timeline (sending context → agent responding →
  wrapping up), collapsible activity with prompt-size / first-response / output stats, per-turn
  "Prompt that was sent" disclosure, completions that stay in the thread, with the full result
  panel (copy, follow-up, repeat, retry) one click from Recent handoffs
- ✅ First-run onboarding (teach by doing — the hotkey press dismisses it) with a "pick your
  assistants" step (detected agents, one-tap add) and a `?` keyboard cheat sheet
- ✅ Drag-and-drop capture (text, text files, images by path) via DOM events + native `tauri://drag-*` file events
- ✅ Session history grouped by Today/Yesterday with open · retry · duplicate · copy · send-to-another-agent, and Clear history
- ✅ Native macOS notifications (banners belong to Handover; clicking activates the app — `osascript` kept only as a permission-denied fallback)
- ✅ Privacy exclusions (case-insensitive), symlink-leaf refusal, path-only notes for large/binary files
- ✅ Local API bearer token for mutating routes; `GET /agents` redacts command lines
- ✅ Safe config recovery (parse errors backed up only after a successful rename)
- ✅ CLI: `status`, `agents`, `actions`, `sessions`, `send`, `attach`, `approve`, bare pipe / stdin
- ✅ Demo echo agent for end-to-end verification without a real agent
- ✅ Session-aware handoff: discovery (filenames+mtimes, cli-list for Hermes), freshest/pinned
  resolution, resume-by-id adapter, LIVE-vs-LAST wording, session chips + activity dots
- ✅ Status layer: working/idle from mtime deltas **plus a live process check** (`pgrep` on
  the agent's command — green only when the agent is genuinely running, e.g. Hermes with
  its TUI open reads online while a leftover OpenCode session file does not); blocked
  (approval) via the opt-in tail-peek; lights refresh live while the palette is open
- ✅ Provider health: the daemon sniffs each agent's model-provider endpoint (its own
  config — Hermes sessions via `state.db`, so resumed cloud sessions aren't mistaken for
  a down local server) and TCP-probes it cheaply. A doomed send is refused in under a
  second with a plain-language reason instead of hanging for the full timeout; the row
  shows amber with the reason before you send
- ✅ Approval layer: detect → inject (`tty`/`tmux`) → verify-after, fail soft; Approve/Deny in
  the palette and `handover approve` in the CLI
- ✅ Live chat in the palette: threaded conversation that resumes the SAME session (explicit
  `session_id` on every message), messages delivered verbatim (no action preamble — the
  exact text you type is exactly what the agent receives), inline streaming + per-turn
  "Prompt that was sent" transparency
- ✅ 202 Rust unit tests + 68 Vitest/Testing Library tests covering the keyboard-first flows
  and the session/approval/chat/activity layers; `cargo clippy --workspace --all-targets` is
  warning-free (enforced in CI — see [.github/workflows/ci.yml](.github/workflows/ci.yml))
- ✅ Production Content Security Policy (never disabled); prompt temp-cache dir overridable with `HANDOVER_PROMPT_CACHE_DIR` for sandboxed CI

Planned next: screenshot capture, OpenAI-compatible agent adapter, frontmost-app
context, component-wise `openat` path walks, and persisted SQLite history with 7-day
retention (session-only history exists today).

## Project layout

```
handover/
├── apps/
│   ├── desktop/          # Tauri app (tray, hotkey, palette) + React/TS UI
│   └── cli/              # handover CLI (thin client of the daemon)
├── crates/
│   ├── core/             # Capture, Actions, Agent trait, prompt rendering, exclusions
│   ├── config/           # TOML config, defaults, per-action preferences
│   ├── agents/           # Generic command agent + registry
│   └── daemon/           # Daemon service: HTTP API, capture, send orchestration
├── docs/                 # Sample config, icon source
└── scripts/              # smoke-test.sh (end-to-end), verify.sh (full release matrix)
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for the design, and
[CONTRIBUTING.md](CONTRIBUTING.md) to get involved.

## License

MIT
