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
cd apps/desktop && ./ui/node_modules/.bin/tauri dev
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
   # From apps/desktop (so src-tauri/tauri.conf.json is discovered)
   cd apps/desktop
   ./ui/node_modules/.bin/tauri dev
   ```

   Or split terminals: Vite in `apps/desktop/ui`, `cargo run` in `apps/desktop/src-tauri`.
   Note: `beforeDevCommand` / `beforeBuildCommand` run from `apps/desktop` (the config's
   parent directory), **not** from `apps/desktop/ui` — that is why they are written as
   `npm --prefix ui run …`. A bare `npm run build` there walks up to the repo root and
   fails with `ENOENT: no such file or directory, open '…/handover/package.json'`.

   Global hotkey: use `on_shortcut` only (it already registers). Never call
   `register()` for the same accelerator — Carbon rejects the duplicate.
   Ad-hoc macOS rebuilds sometimes need Accessibility re-toggled for the new
   `.app` binary after a rebuild (System Settings → Privacy & Security →
   Accessibility); fully Quit and relaunch after changing it.

7. **Desktop release bundle** (macOS `.app`):

   ```bash
   cd apps/desktop
   ./ui/node_modules/.bin/tauri build --ci
   # Output: target/release/bundle/macos/Handover.app
   ```

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
   - CLI: `status`, `agents`, `actions`, `send`, `sessions`, `attach`, `approve`, and
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

### Known dependency advisories

`npm audit` reports two moderate advisories in `vitest` (GHSA-82fw-gwwq-j7x9, path
traversal / arbitrary file read in `@vitest/mocker`). This is accepted deliberately:

- **Dev-only.** Vitest never ships in the app bundle; it is not in the Tauri artifact.
- **The vulnerable path is not used.** The advisory covers *redirect* mocks. Every
  `vi.mock` in this repo passes a factory (`vi.mock("../lib/tauri", () => tauri)`),
  which is a different code path.
- **No patched 3.x exists.** `vitest@3.2.7` pins `@vitest/mocker` to exactly `3.2.7`,
  so the only upstream fix is `vitest@5` — two majors away. Forcing a 5.x mocker under
  a 3.x runner via `overrides` is not a safe shortcut.

Revisit when either becomes true: a test starts using a redirect mock, or we migrate to
vitest 5 for other reasons. CI runs `npm audit --omit=dev` as a **non-blocking** step —
production dependencies only, so real risk stays visible without this noise.

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
