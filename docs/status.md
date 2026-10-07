# Current status (V1 vertical slice)

Implemented and verified end-to-end:

- ✅ Background daemon with local HTTP API (`127.0.0.1:47444`)
- ✅ Global hotkey → chat-first palette (agent list → chat with the running agent); handoffs run off the UI thread
- ✅ Agent dropdown + chat merged surface: rows are logo + name + one status dot — green (process actually running / session actively writing), red (installed idle), amber (provider down, with reason); blocked sessions keep `blocked · approve?`; keyboard-first selection; lights refresh live while the palette is open
- ✅ Settings window (lazy-created; menu bar / `⌘,` / tray / palette gear) with Overlay traffic-light chrome: General (appearance, window opacity for palette + Settings, live shortcut re-register, launch at startup, notifications), Agents, Privacy, About
- ✅ Monochrome UI matching the website's palette: shared light/dark design tokens,
  Geist / Geist Mono, a solid 16px card with hairline borders, 8–12px controls, mono
  labels and meta, header tools, "the pass" app icon + macOS template tray icon
  (brand kit in `brand/`)
- ✅ Standardized Capture object (drag-and-drop text/files, CLI/API captures) — the palette is chat-first and sends messages verbatim; the clipboard is never read
- ✅ Generic command agent adapter (`{PROMPT}`, `{PROMPT_FILE}`, stdin) + registry
- ✅ Concurrent agent stdout/stderr drain, output caps, process-group timeout (Unix)
- ✅ Chat delivers messages verbatim into the resumed session (`ask` template, no preamble) — the composer opens clean, dropped text/files pre-fill it, and nothing sends without an explicit `Enter`
- ✅ Live handoff feedback: stream-derived phase timeline (sending context → agent responding →
  wrapping up), per-turn collapsible activity (each exchange reports its own
  prompt-size / first-response / output stats), per-turn
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
  exact text you type is exactly what the agent receives), thinking animation until the
  finished reply + per-turn
  "Prompt that was sent" transparency
- ✅ 218 Rust unit tests + 67 Vitest/Testing Library tests covering the keyboard-first flows
  and the session/approval/chat/activity layers; `cargo clippy --workspace --all-targets` is
  warning-free (enforced in CI — see [.github/workflows/ci.yml](../.github/workflows/ci.yml))
- ✅ Production Content Security Policy (never disabled); prompt temp-cache dir overridable with `HANDOVER_PROMPT_CACHE_DIR` for sandboxed CI

Planned next: screenshot capture, OpenAI-compatible agent adapter, frontmost-app
context, component-wise `openat` path walks, and persisted SQLite history with 7-day
retention (session-only history exists today).

← [Back to the README](../README.md)
