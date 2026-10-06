# Contributing

Thanks for helping with Handover! The project is small on purpose, and it stays that way
only if every addition earns its place. Please read this before opening an issue or PR.

For **current product behavior** (config paths, privacy, CLI, API), prefer
[README.md](README.md) and [ARCHITECTURE.md](ARCHITECTURE.md).

## Ground rules

1. **Keep the core loop sacred.** The product is *see something → hotkey → action → agent →
   send*. Anything that makes that loop slower, heavier, or more complicated is suspect.
2. **No scope creep.** No accounts, subscriptions, analytics, cloud sync, telemetry, or
   agent orchestration. The one deliberate exception is the palette's **live chat** — a
   local, threaded conversation that resumes the user's own agent session (see
   ARCHITECTURE.md, "Live chat"). It exists because the user asked for a conversation
   window, not because Handover is becoming a chat product: no chat accounts, no
   cloud routing, no multi-agent orchestration. Keep it that way.
3. **Local-first.** The daemon binds to `127.0.0.1` only. Privacy exclusions are a core
   architectural principle, not an afterthought.
4. **UI talks to abstractions.** New agents, actions, or capture types must flow through the
   core abstractions (`Agent`, `Action`, `Capture`). Never hard-code a specific agent or a
   prompt string into the UI or CLI.
5. **Prefer the smallest real implementation.** If a feature can't work fully yet, implement
   the smallest version that actually works and document the limitation. No placeholders
   marked as complete.
6. **Test as you go.** Core logic (capture, actions, prompt rendering, exclusions, config,
   agent behavior) needs unit tests. Platform behavior (hotkey, clipboard, tray) needs
   manual verification and is kept behind isolated platform modules.
7. **Never block the UI on agent runtime.** Long handoffs must run off the UI/command
   thread (e.g. `spawn_blocking` from an async Tauri command). Synchronous agent waits
   freeze the palette.

## Repository layout

```
apps/desktop          Tauri app: tray, global hotkey, palette + settings windows, React/TS UI
                      (ui/src/components, ui/src/styles, ui/src/lib, ui/src/test)
apps/cli              handover CLI — thin HTTP client of the daemon
crates/core           Capture, Action, Agent trait, prompt rendering, exclusions (pure)
crates/config         TOML config, defaults, per-action preferences, privacy rules
crates/agents         Generic command agent + registry
crates/daemon         Orchestration + local HTTP API (also embedded by the desktop app)
docs/sample-config.toml   Portable sample config (placeholders; not personal machines)
scripts/              smoke-test.sh (end-to-end), verify.sh (full release matrix)
```

## Setting up

```bash
# Rust toolchain + npm deps
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cd apps/desktop/ui && npm ci && cd ../../..

# Build workspace (CLI, daemon, libraries)
cargo build --workspace

# Desktop app. `tauri build` / `tauri dev` run the UI build for you
# (beforeBuildCommand / beforeDevCommand), so a separate `npm run build`
# is only needed when you want the UI bundle without the Rust compile.
cd apps/desktop/src-tauri && ../ui/node_modules/.bin/tauri dev
```

macOS: `xcode-select --install`. Linux: install the
[Tauri system dependencies](https://v2.tauri.app/start/prerequisites/) for your distro.

### Config and API token locations

Config and token are created on first run under the **platform** config dir (via `dirs`):

| Platform | Config | API token |
|---|---|---|
| macOS | `~/Library/Application Support/handover/config.toml` | same dir, `api_token` |
| Linux | `~/.config/handover/config.toml` | same dir, `api_token` |
| Windows | `%APPDATA%\handover\config.toml` | same dir, `api_token` |

**Local HTTP API:** mutating routes (`POST /send`, `/preferences`, `/quit`, …) require
`Authorization: Bearer <token>`. The CLI loads the token from the file automatically
(or from `HANDOVER_TOKEN`). `/health` and most GETs stay open. Desktop palette handoffs
are in-process and do not need the token.

When calling the API with `curl`, pass the token explicitly:

```bash
TOKEN=$(tr -d '[:space:]' < "$HOME/Library/Application Support/handover/api_token")
curl -s -H "Authorization: Bearer $TOKEN" -X POST http://127.0.0.1:47444/quit
```

## Development workflow

0. **CI** — every push and PR runs the same matrix as `scripts/verify.sh`:
   `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`,
   `cargo test --workspace` (macOS + Linux), the UI lint/typecheck/tests/build,
   and the end-to-end smoke test. See [.github/workflows/ci.yml](.github/workflows/ci.yml).
   If you add a verification step, add it in both places.

1. **Unit tests** — run the fast ones while iterating:

   ```bash
   cargo test -p handover-core -p handover-config -p handover-agents -p handover-daemon
   # or everything:
   cargo test --workspace --lib
   ```

   One daemon test shells out to real macOS AppleScript (`notify.rs`, `osascript`); it is
   opt-in because it can error under load or in headless sessions. Run it explicitly with
   `HANDOVER_TEST_OSASCRIPT=1`; otherwise it prints a skip notice.

   **Critical — tests must never write the real user config.**

   - `Daemon` persists preferences to `daemon.config_path`, not always the platform path.
   - In tests, build daemons with a **temp** config path (see existing `test_daemon` helpers).
   - Prefer `Config::save_to`, `Config::ensure_at`, `Config::ensure_api_token_at` over
     `Config::save()` / `Config::ensure()` when exercising persistence in unit tests.
   - A regression test asserts `set_preference` does not mutate the real platform file.

2. **Vertical slice smoke test** — verifies daemon → API → CLI → demo agent end-to-end
   (throwaway `HOME`, temporary port, API token under that home, cleans up after itself).
   The script preserves `CARGO_HOME` / `RUSTUP_HOME` so rustup still works after
   rebinding `HOME`:

   ```bash
   ./scripts/smoke-test.sh
   ```

   The prompt temp-cache location is overridable with `HANDOVER_PROMPT_CACHE_DIR` — set
   it when running tests in a sandbox / CI so the suite never touches the user's real
   cache. (Daemon unit tests pin this automatically; you only need it for custom CI setups.)

3. **Clippy** must be warning-free across the workspace:

   ```bash
   cargo clippy --workspace --all-targets   # expect: no warnings
   ```

   `cargo fmt --check` must also pass — run `cargo fmt` before sharing a change (the
   toolchain is pinned to 1.97.1 by `rust-toolchain.toml`; rustup fetches it
   automatically).

4. **UI lint + tests** (Vitest + Testing Library, jsdom — no Tauri runtime needed):

   ```bash
   cd apps/desktop/ui
   npm run lint && npm run build   # eslint (flat config) + tsc
   npx vitest run
   ```

   `src/test/palette.test.tsx` stubs `lib/tauri` and records registered event listeners,
   so tests fire `palette:open`, `handoff:output`, `handoff:started`, `tauri://drag-drop`,
   … exactly as the Rust side would. Cover the chat-first flows: the dropdown rows carry
   one status dot each (green running / red idle / amber provider-down, freshest session
   wins, blocked · approve?), arrow-key navigation into the chat beneath the dropdown,
   verbatim
   `ask` sends with the explicit session id, clean composer (no clipboard capture),
   Enter-sends / Shift+Enter-newline keys,
   composer pre-fill from dropped context, in-thread failures and inline streaming,
   blocked-session Approve/Deny (explicit press, verified vs fail-soft notes), history
   (open / retry / another agent), first-run onboarding, drop capture, and the `?`
   cheat sheet.

6. **Desktop app (dev)** — UI on Vite; Rust via Tauri:

   ```bash
   # From src-tauri, the same directory the release workflow uses
   cd apps/desktop/src-tauri
   ../ui/node_modules/.bin/tauri dev
   ```

   Or split terminals: Vite in `apps/desktop/ui`, `cargo run` in `apps/desktop/src-tauri`.
   Note: the Tauri CLI runs `beforeDevCommand` / `beforeBuildCommand` from the app
   directory it detects, and that depends on where you launch it. From `src-tauri` it is
   `apps/desktop`, which is what `npm --prefix ui run …` expects. From `apps/desktop`
   the CLI finds `ui/package.json` and runs the hooks from `ui/` instead, so
   `--prefix ui` resolves to `ui/ui` and fails with `ENOENT … ui/ui/package.json`.

   Global hotkey: use `on_shortcut` only (it already registers). Never call
   `register()` for the same accelerator — Carbon rejects the duplicate.
   Ad-hoc macOS rebuilds sometimes need Accessibility re-toggled for the new
   `.app` binary after a rebuild (System Settings → Privacy & Security →
   Accessibility); fully Quit and relaunch after changing it.

7. **Desktop release bundle** (macOS `.app`):

   ```bash
   cd apps/desktop/src-tauri
   ../ui/node_modules/.bin/tauri build --ci
   # Output: target/release/bundle/macos/Handover.app
   ```

   **Do not hand-upload this to a release.** Published artifacts come from
   `.github/workflows/release.yml` on a `v*` tag, so the binary is built by CI
   from the tagged commit and gated by fmt + clippy + the full test suite. A
   locally built `.app` may work and still be the wrong bytes.

   To cut a release:

   ```bash
   # 1. Make sure version strings agree (Cargo.toml, tauri.conf.json, ui/package.json)
   # 2. Tag and push — the tag IS the version
   git tag v0.1.0 && git push origin v0.1.0
   # 3. Watch the Release workflow; re-run it from the Actions tab if needed
   ```

   The bundle is `universal-apple-darwin` (Intel + Apple Silicon) and **ad-hoc
   signed** (`"signingIdentity": "-"` in tauri.conf.json) because universal
   binaries cannot be lipo-merged unsigned. Ad-hoc asserts no developer identity,
   so Gatekeeper warns on first launch — users right-click → Open. Only a real
   Developer ID + notarization removes that warning.

8. **Manual verification checklist** (for anything touching platform layers):
   - global hotkey opens the palette instantly
   - the palette opens clean — the clipboard is never read, and the composer is empty
     until you type or drop context
   - status dots are honest: a green light means the agent's process is actually running
     (or its session is actively writing), never just a leftover session file
   - action + agent selection shows a spinner while the agent runs (UI stays responsive)
   - the palette stays open during the handoff with a live elapsed timer and the reply
     streaming inline; on completion the finished turn stays in the thread (you can keep
     chatting), and the full result panel (Copy answer / Copy prompt / Close) opens from
     *Recent handoffs*
   - tray menu → *Recent Handoffs* lists the last 10 handoffs; clicking a row reopens that
     result in the palette (matches by stable handoff id, not position)
   - handoff completes with a notification; on macOS the banner belongs to Handover
     (native, via `tauri-plugin-notification`) and clicking it activates the app — not
     Script Editor. `osascript` is only a fallback when the user denies permission.
   - first-run onboarding appears once and the hotkey press (or Start) dismisses it; it
     includes a "pick your assistants" step (detected gallery agents, one-tap add, skipped
     when there is nothing to pick); `?` opens the cheat sheet without hijacking typing in
     the search box
   - the palette opens on a dropdown + chat merged surface: rows are logo + name + one
     status dot (green running / red idle / amber provider-down with reason; blocked rows
     keep `blocked · approve?`); arrow keys + Enter open a chat directly beneath the
     dropdown; unavailable agents are non-activatable "Not found" rows
   - the chat composer opens clean (no clipboard capture) and pre-fills only from dropped
     context; nothing sends without an explicit Enter (Shift+Enter is a newline); the
     message goes verbatim into the resumed session
   - dragging text or a file onto the palette captures it (drop overlay while dragging);
     unreadable drops explain why in plain language
   - while sending, the status sentence evolves (sending context → responding → wrapping
     up) and the in-flight turn's activity section shows the phase timeline + prompt /
     first-response / output stats; after it completes, every turn keeps its own panel
     with its own numbers (two exchanges in one thread must not share figures)
   - Settings: opens on demand (not pre-created at launch); traffic-light chrome on
     macOS; appearance and window-opacity re-theme live on palette + Settings; the
     configurable shortcut re-registers without a relaunch; launch-at-startup and
     notifications toggles stick
   - Agents tab: one unified adaptive card per agent — quiet status line
     (`Installed · signed in` / `Not detected` / `In palette · default`), green
     Add for not-added entries, in-palette toggle for added ones; Make default,
     Detect session, View command and Remove live in the card's ⋯ menu (OFF on
     the toggle hides the agent from the dropdown while keeping its config)
   - unavailable agent shows a clear "not available" state (no crash, no stack trace)
   - excluded files (e.g. a `.env`) and symlinks to secrets are refused
   - CLI: `status`, `agents` (each row ends with the `adapter:` line naming the
     agent version that adapter was verified against), `actions`, `send`,
     `sessions`, `attach`, `approve`, and
     `cat x | handover` / `printf x | handover`
   - unauthenticated `POST` to the API is rejected; authenticated CLI still works
   - `GET /history` and `GET /sessions` require the bearer token (401 without it)
   - session-aware flows (real agent, e.g. codex/droid/omp): `handover sessions` lists the
     live session freshest-first; sending resumes into it (verify it appears in the TUI on
     switch-back); `handover attach` pins it and `--unpin` restores freshest-first;
     two-open-sessions ambiguity picks the freshest
   - LIVE vs LAST wording: a quiet session is labeled `last session`, never `live`
   - approval cycle (real agent with transcript files): marker-configured agent parks at a
     permission prompt → palette/CLI shows `blocked` → Approve/Deny → verified result
     (agent resumed) or the fail-soft "couldn't confirm — check the terminal" path

## Adding or updating an agent adapter

An adapter is **not code** — it is the set of assumptions Handover makes about
one agent's CLI. Those assumptions live in five places, all keyed by the agent
id string:

| What it holds | File | Function |
|---|---|---|
| Session discovery + resume command | `crates/config/src/config.rs` | `builtin_session_agents()` |
| Binary name, install paths, send command | `crates/daemon/src/catalog.rs` | `agent_catalog()` |
| How a session filename becomes a session id | `crates/core/src/session.rs` | `SESSION_ID_RULES` |
| Whether an id is hermes-style or a uuid | `crates/agents/src/session_agent.rs` | `ID_SHAPES` |
| Where the agent's model endpoint is configured | `crates/daemon/src/provider_health.rs` | `ENDPOINT_SNIFFERS` |

You normally touch only the first two. See ARCHITECTURE.md, "Adapter
compatibility", for the design.

When an agent changes its CLI, those assumptions go stale **silently**: a renamed
resume flag or a changed session filename still exits 0, so Handover reports
*"Handed off successfully"* while the context landed in the wrong conversation.
Handover therefore **declares** which agent version each adapter was verified
against, in `ADAPTER_COMPAT` in `crates/config/src/adapter.rs`.

### Adding a brand-new agent

1. Add it to `builtin_session_agents()` (`crates/config/src/config.rs`) — give
   it an `id`, a fresh-send `command`, and, if the agent has resumable sessions,
   a `resume_command` containing `{SESSION}` plus exactly one of `session_glob`
   / `session_cli_list`.
2. Add it to `agent_catalog()` (`crates/daemon/src/catalog.rs`) so Settings can
   offer it, with the `binary_name` and the `{BIN} …` command template.
3. Add an `ADAPTER_COMPAT` entry (`crates/config/src/adapter.rs`):

   ```rust
   AdapterCompat {
       agent_id: "my-agent",              // must match the id used above
       verified_agent_version: Some("1.2.3"), // what you actually ran
       verified_on: "2026-10-04",         // today, ISO YYYY-MM-DD
       assumes: "sessions at ~/.my-agent/sessions/<uuid>.jsonl (filename IS \
                 the session id); resume via `my-agent -r <id> \"{PROMPT}\"`",
   },
   ```

4. Add a row to the "Per-agent session facts" table in ARCHITECTURE.md.

Then run `cargo test -p handover-config -p handover-daemon` — the drift guards
below will tell you if you missed a step.

### Updating an existing agent after its CLI changed

Do all of this in the **same** change:

1. **Update the adapter itself** — the command, `resume_command`, session glob /
   CLI list, or the per-agent rule — for the agent's *current* CLI.
2. **Update that agent's `ADAPTER_COMPAT` entry** — the verified version, the
   `verified_on` date, and the `assumes` line naming what you re-checked. Get the
   version from `<agent> --version`, and confirm session layout with
   `scripts/probe-sessions.sh` (reads filenames + mtimes only). Don't guess.
3. **Update the "Per-agent session facts" table** in ARCHITECTURE.md if the
   session layout, resume flags or streaming behaviour changed.
4. **Never invent a version.** If you cannot run the agent, set
   `verified_agent_version: None` and keep the previous date. An honest
   `unverified` is the whole point — that is the current state of `hermes`.

### What `ADAPTER_COMPAT` is, and is not

It is a **historical record and a piece of evidence**: "these assumptions were
true, on this date, for this agent version". It is **not** a guarantee — it is
never checked against the agent you have installed, and Handover will still run
against a newer or older agent than the one recorded. That mismatch is exactly
what a drift report needs to surface, so don't try to "fix" it in code.

**Not wanted:** semver ranges, a version resolver, `--version` probing at
runtime, or any dependency-management layer. Probing would mean a subprocess per
agent on every 4s status poll. The declaration plus these tests is the entire
mechanism.

### The drift guards

Tests fail if the adapter and its declaration disagree, in either direction:

- **Missing declaration** — every `builtin_session_agents()` id, every
  `agent_catalog()` id, and every agent in the per-agent tables
  `SESSION_ID_RULES` (filename→session id), `ID_SHAPES` (non-UUID session ids)
  and `ENDPOINT_SNIFFERS` (provider-config locations) must have an
  `ADAPTER_COMPAT` entry. Adding an adapter without one fails `cargo test`.
- **Orphan declaration** — every `ADAPTER_COMPAT` entry must correspond to an
  agent Handover actually ships an adapter for, so a removed adapter can't leave
  a stale "adapter: …" claim behind.
- **Malformed entry** — non-empty `assumes`, a real ISO date, and unique ids.
- `AgentMeta.compat` is populated for bundled agents and **absent** for a user's
  own command agent (which has no bundled assumptions to drift), including one
  that reuses a bundled id but runs a different program (`compat_for_config`).

If you report an agent that misbehaves, include its `--version` output and the
`adapter:` line from `handover agents` — that pair is what identifies a stale
adapter. The *Agent adapter drift* issue template asks for exactly these.

## Privacy / security expectations for changes

- Exclusions are case-insensitive; file attach opens the **leaf** with `O_NOFOLLOW`.
- Do not put secrets in agent `command` strings; `GET /agents` redacts summaries to
  `"configured command"` over HTTP (desktop in-process may still show more detail).
- Prompt temp files live under a private app cache dir with mode `0600` and RAII cleanup.
- Config recovery: only replace a broken on-disk config **after** a successful rename to
  `*.toml.bak` (parse errors only). I/O failures must not clobber the file.
- **Session discovery is stat-only**: filenames + mtimes, never transcript contents.
  Session ids come from filenames; Hermes freshness comes from the id-encoded timestamp
  (`YYYYMMDD_HHMMSS_…`), not from reading the database.
- **The approval tail-peek is the ONE documented exception** — and it must stay narrow:
  only agents that opt in with a `permission_marker` are ever peeked at, only the last
  few KB of a *live* session's transcript, only for that marker, and the buffer is
  dropped immediately (never stored, logged, or sent). The one addition is the marker
  line itself being surfaced to the palette (`blocked_detail`) so the approval card can
  show what the agent is asking — the same narrow read, displayed only in the user's own
  UI, never persisted. New code must not widen this.
- Approval action must stay **verify-after, fail-soft**: success is claimed only when the
  transcript is observed resuming; a timeout returns "couldn't confirm — check the
  terminal". Never claim success without verification, and never inject into a session
  that is no longer blocked (re-check before injecting).

### Dependency advisories

`npm audit` is clean, and CI runs `npm audit --omit=dev --audit-level=high` as a
**non-blocking** step — it reports, it does not gate.

This was not always true. Two moderate advisories in vitest
(GHSA-82fw-gwwq-j7x9, path traversal / arbitrary file read in `@vitest/mocker`)
were carried for several releases rather than fixed, because the only upstream
fix was a two-major jump. They were cleared when the UI moved to vitest 5,
which also raised the floor to Node 22.12. If a future major does the same,
weigh it the same way: a dev-only advisory in a code path the repo does not use
is not worth an unbounded migration, but it should be a decision, not a drift.

For Rust, Dependabot tracks RustSec and opens the upgrade PR — see
`.github/dependabot.yml`. One advisory is currently open with **no upstream fix
available**, so it cannot be closed by a version bump:

- `GHSA-wrw7-89jp-8q8g` (medium) — unsoundness in `glib::VariantStrIter`.
  `glib` 0.18.5 arrives transitively through Tauri, and Handover does not use
  `VariantStrIter`; there is no patched release to move to. Dependabot will
  raise it when upstream ships one.

## Coding style

- Follow the existing layout: pure logic in `core`, orchestration in `daemon`, platform in
  the desktop app, and nothing duplicated in the CLI.
- Errors are humanized before they reach a user: plain messages, no raw stack traces.
- Agent process I/O: drain stdout/stderr concurrently, cap stored output, kill the process
  group on timeout (Unix).
- Add tests beside the code (`#[cfg(test)] mod tests`), matching the existing style.
- Keep documentation current: README (user-facing), ARCHITECTURE (design), this file.

## Pull requests

1. Small, focused changes — one feature or fix per PR.
2. Include tests where core logic changes.
3. Run `bash scripts/verify.sh` (fmt, clippy, workspace tests, UI lint/tests/build,
   smoke) before proposing a change — it is the full release matrix. (The repo is not
   under git; share patches or a diff.)
4. Mention what you verified manually (especially platform-specific behavior).
5. Do not commit personal `config.toml` contents, tokens, or machine-specific paths into
   `docs/sample-config.toml`.

## Reporting bugs

Include: OS, Handover version (`handover status`), config (redact secrets), what you did,
what you expected, and what happened. If the daemon logs to a terminal, include the relevant
lines. Never include `.env` contents, `api_token`, or credentials in an issue.
