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

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/demo-dark.gif">
    <source media="(prefers-color-scheme: light)" srcset="docs/demo-light.gif">
    <img src="docs/demo-light.gif" width="520" alt="Handover in use: ⌘⇧A opens the palette, a build log is dropped onto it, Claude Code is picked from the agent list, and the reply comes back.">
  </picture>
  <br>
  <sub>Handover in use</sub>
</p>

**Handover is a community project.** It's MIT-licensed and open to contributors: see
[Help wanted](#help-wanted).

---

## How it works

1. You see something on your computer: an error, a log line, a file, a URL.
2. Press the global hotkey (`⌘⇧A` / `Super+Shift+A`). The palette opens with a clean
   composer; the clipboard is never read.
3. Optionally, drop a file or text onto the palette to pre-fill your message.
4. Pick an agent. Agents on your machine are grouped **Running now** and **Available**,
   each with a status light: **green** when its process is actually running, **red** when
   installed but idle, **amber** when its model provider is unreachable.
5. The chat opens beneath the picker, resuming the agent's live session when it has one.
   `Enter` sends your message verbatim.
6. A thinking animation shows while the agent works, then its reply lands in the
   thread. After the first time it's
   `⌘⇧A → Enter → Enter`.

Everything runs locally. Nothing is uploaded anywhere unless the agent you choose does so.

More: [Using the palette](docs/usage.md) · [Live sessions & approvals](docs/sessions.md)

## Installation

### macOS — download the app

Grab **Handover-<version>-macOS-universal.zip** from the
[releases page](https://github.com/thefullctx/handover/releases), unzip it, and
drag `Handover.app` into Applications. One binary covers both Apple Silicon and
Intel.

The first launch will say the developer cannot be verified. **This is expected** —
the release is not signed with an Apple Developer certificate. Right-click the app,
choose **Open**, and confirm; that warning does not come back.

Then give it Accessibility permission (System Settings → Privacy & Security →
Accessibility) so the global hotkey works.

> Building from source instead? See [Prerequisites](#prerequisites) below.

### Prerequisites

Only needed if you are building from source.

- **Rust** (stable): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node.js** 22.12+ and npm (for the desktop UI)
- **macOS**: Xcode Command Line Tools (`xcode-select --install`)
- **Linux**: Tauri system dependencies — see
  [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

### Build

```bash
# Core, daemon, and CLI
cargo build --workspace

# Desktop app (Tauri). This builds the React UI first, then compiles the app.
cd apps/desktop/src-tauri && ../ui/node_modules/.bin/tauri build
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

**Supported platforms:** Handover is developed and verified on **macOS** and
**Linux**. Windows paths are reserved in the config layout for future support,
but the desktop app (tray, hotkey, status lights) is not yet supported there —
the live-process check and approval injection are Unix-specific.

## Supported agents

| Agent | One-click add (Settings → Agents) | Live sessions (resume) |
|---|---|---|
| Claude Code | — | ✓ |
| Codex | ✓ | ✓ |
| Droid | — | ✓ |
| Oh My Pi (omp) | ✓ | ✓ |
| Hermes | ✓ | ✓ |
| OpenCode | ✓ | — |
| Any other CLI agent | **Add custom agent…** (`{PROMPT}`, `{PROMPT_FILE}` or stdin) | — |

An OpenAI-compatible endpoint adapter is planned. See [Adding an agent](docs/configuration.md#adding-an-agent)
and [Live sessions & approvals](docs/sessions.md).

## Privacy

- **Local-first:** the daemon binds to `127.0.0.1` only, and mutating API routes need a
  local bearer token.
- **Secrets are refused:** `.env`, `*.pem`, `*.key`, credentials, `~/.ssh/*` and anything
  you add are refused before being read.
- **No hidden reads:** the clipboard is never read, projects are never scanned, and
  nothing is uploaded by Handover itself.
- **Stat-only session discovery:** live sessions are found from filenames and timestamps,
  with one documented, opt-in exception for approval prompts.

Full details: [Privacy model](docs/privacy.md).

## Help wanted

Handover started as a solo project and is now open for the community to shape. Good places
to start:

- **Agent adapters:** support a new agent, or update one after its CLI changed
  ([guide](CONTRIBUTING.md#adding-or-updating-an-agent-adapter)).
- **UX:** the palette, onboarding and settings.
- **Packaging:** installers and release builds.
- **The ugly parts:** rough edges, bugs, anything unfinished.

Open an issue, send a PR, or fork it and make it your own. Start with
[CONTRIBUTING.md](CONTRIBUTING.md).

## Documentation

- [Using the palette](docs/usage.md): chat, drag & drop, handoff feedback, history
- [Live sessions & approvals](docs/sessions.md)
- [Configuration](docs/configuration.md): config file, agents, actions, API token
- [CLI](docs/cli.md)
- [Privacy model](docs/privacy.md)
- [Status](docs/status.md): everything implemented and verified so far
- [ARCHITECTURE.md](ARCHITECTURE.md) · [CONTRIBUTING.md](CONTRIBUTING.md)

## Status

V1 vertical slice, developed and verified on macOS and Linux. See the
[full status list](docs/status.md).

Planned next: screenshot capture, OpenAI-compatible agent adapter, frontmost-app
context, component-wise `openat` path walks, and persisted SQLite history with 7-day
retention (session-only history exists today).

## License

MIT
