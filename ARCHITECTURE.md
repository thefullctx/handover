# Architecture

Handover is a **local handoff layer**: it captures something you see, renders a prompt from
an action template, and hands it to an agent you already have. It is a thin utility, not a
platform — the architecture enforces that by keeping the core free of UI, networking, and
platform concerns, and by making every client (GUI, CLI) a thin consumer of the same core.

## Design principles

1. **One core, many clients.** All application logic lives in the `handover-daemon` and
   `handover-core` crates. The GUI calls the daemon in-process; the CLI calls it over a
   local HTTP API. No client duplicates logic.
2. **The UI talks to abstractions, not agents.** The palette renders `Action`s and
   `AgentMetaStatus`es. It never hard-codes a specific agent or prompt.
3. **Local-first and private by default.** The API binds to `127.0.0.1`. Privacy exclusions
   are enforced at the core layer, before any file is read. No cloud, no accounts, no
   telemetry.
4. **Platform concerns are isolated.** Hotkeys, clipboard, notifications, and paths live
   behind small interfaces so macOS and Linux share one core. On macOS the desktop app
   posts notifications natively via `tauri-plugin-notification`
   (UNUserNotificationCenter) so banners belong to Handover; the daemon's `osascript`
   path is kept only as a permission-denied fallback.
5. **Smallest vertical slice first.** The first milestone — hotkey → palette → chat →
   generic command agent → send → notification — is fully working before any additional
   capture types or adapters are layered on. (Clipboard capture was built early and later
   removed: the palette opens clean and never reads the clipboard.)

## Layering

```
┌──────────────────────────────────────────────────────────┐
│ Clients                                                   │
│   apps/desktop  (Tauri: tray, hotkey, palette window)     │
│   apps/cli      (handover ... thin HTTP client)          │
├──────────────────────────────────────────────────────────┤
│ handover-daemon                                          │
│   owns Config + AgentRegistry + last capture              │
│   orchestrates: normalize capture → enrich → render → send│
│   session discovery + pin state (0600 session_state.json) │
│   approval: detect (opt-in tail-peek) → inject → verify   │
│   serves local HTTP API (127.0.0.1:47444)                 │
├──────────────────────────────────────────────────────────┤
│ handover-agents                                          │
│   GenericCommandAgent ({PROMPT} / {PROMPT_FILE} / stdin)  │
│   SessionAgent (resume-by-id via {SESSION})               │
│   AgentRegistry (detect / send / status)                  │
├──────────────────────────────────────────────────────────┤
│ handover-config                                          │
│   TOML config, defaults, per-action preferences, privacy, │
│   session fields (glob/cli-list/resume/approval), catalog │
├──────────────────────────────────────────────────────────┤
│ handover-core                                            │
│   Capture model, Action templates, Agent trait,           │
│   prompt rendering, security exclusions,                  │
│   session discovery (glob, id extraction, activity),      │
│   approval tail-peek (pure, testable)                     │
└──────────────────────────────────────────────────────────┘
```

## Core abstractions

### Capture

The standardized internal representation of whatever the user saw:

```rust
struct Capture {
    id: Uuid,
    timestamp: DateTime<Utc>,
    source: CaptureSource,      // { type: text|image|file|url|terminal|manual, application }
    content: CaptureContent,    // { type: text|image|file|url|terminal|mixed, text, path }
    metadata: HashMap<String, String>,  // working dir, git branch, file notes, ...
}
```

All capture sources (dropped text/files, images, URLs, terminal, CLI/stdin — the clipboard
is never read) normalize into this one shape. `Capture::normalize()` is applied on every
handoff so stored/rendered forms are consistent.

### Action

A named prompt template. The UI and CLI enumerate actions from the core; prompts are never
reconstructed in the UI.

```rust
struct Action {
    id: String,            // "fix"
    name: String,          // "Fix this"
    description: String,   // "Investigate this issue and fix it."
    prompt_template: String,
}
```

Built-ins: `fix`, `investigate`, `explain`, `write_tests`, `ask`.

### Agent

Every integration (including future Hermes, Claude Code, Codex, OpenCode, and
OpenAI-compatible endpoints) sits behind the `Agent` trait. Nothing in the UI depends on a
specific agent type.

```rust
trait Agent {
    fn meta(&self) -> AgentMeta;
    fn detect(&self) -> Availability;  // is the command/endpoint present?
    fn send(&self, request: &AgentRequest) -> Result<SendReceipt, AgentError>;
    fn status(&self) -> AgentMetaStatus;
}
```

**Agent resolution order** for a handoff:

1. Explicit agent choice (UI/CLI).
2. Per-action preference (`[preferences] fix = "hermes"`).
3. Default agent (first configured).

**Session resolution order** (session-aware agents only):

1. Explicit session override (`handover send --session <id>`).
2. The pinned session for that agent (`handover attach`), while it stays live.
3. The freshest live session (mtime / id-encoded timestamp).
4. None → a fresh send, identical to today's flow.

A resume refused because another live process owns the session ("already
has a live owner") falls back to a fresh send when the target came from
step 3, with the fallback noted on the receipt; steps 1–2 never fall back
silently and fail with a plain-language error instead.

### Session-aware agent

`kind = "session"` agents behave exactly like `command` agents for fresh sends, and add
resume-by-id when the request carries a `SessionTarget` (`AgentRequest.session`): the
agent runs its configured `resume_command` with `{SESSION}` substituted instead of the
plain command, reusing the same drain / timeout / process-group machinery. The reply still
streams back, so the result panel, live progress, history and retry keep working — only
which conversation the context lands in changes. The receipt records the session the
reply actually landed in (`SendOutcome.session_id`), so a rotated id is never lost.

### Session discovery robustness

Glob-based discovery (`session_glob`) scans filenames + mtimes only. CLI-list discovery
(`session_cli_list`, used by Hermes — SQLite, no filesystem glob) runs the command from
the daemon, resolved to an absolute path, under a **5s kill-it-deadline** (a hung listing
fails soft: the agent reports no live sessions rather than stalling the scan), and the
Hermes sqlite sniff is likewise bounded (3s) — a discovery command can never wedge the
daemon. Finder-launched apps inherit a **restricted PATH**, so a bare binary name like
`hermes` would silently discover nothing: `resolve_cli_program()` resolves bare names in
the inherited PATH plus `~/.local/bin`, `~/bin` and the Homebrew dirs before spawning
(absolute paths pass through unchanged).

### Live chat (palette)

The palette's "Keep chatting" opens a threaded conversation that keeps resuming the SAME
session: every message is sent through `send_handoff` with the explicit `session_id`
(never left to freshest-resolution), so the agent keeps the conversation context
in-session. Two deliberate design points:

- **Verbatim delivery**: a capture carrying `metadata.chat == "true"` (set only by the
  chat composer) renders its prompt as the capture text alone — the action preamble is
  skipped. Without this, "Analyze the provided context and tell me what you
  recommend." wrapped around a casual question made agents answer
  "I don't see any new context in your message" instead of conversationally.
- **Transparency**: every turn shows the exact rendered prompt under an expandable
  "Prompt that was sent" disclosure — nothing the agent receives is hidden.

Failures render in-thread; while a message is in flight the bubble shows only a thinking
animation, and the finished reply replaces it.

### Generic command agent

The linchpin of the V1 slice. A user configures any command and Handover invokes it:

```toml
[[agents]]
id = "my-agent"
kind = "command"
command = "my-agent --prompt \"{PROMPT}\""
```

Substitution rules:

- `{PROMPT}` → the rendered prompt, shell-quoted.
- `{PROMPT_FILE}` → path to a temp file containing the prompt, shell-quoted.
- Neither present → prompt piped on stdin.

`detect()` checks whether the executable is on `PATH` (or the file exists) so the UI can show
"available / not available" before the user tries to send.

## Handoff flow

```
Capture (dropped text/file, CLI args, stdin)
   │   (the clipboard is never read)
   ▼
normalize() ──────────────────────────────┐
   │                                      │
enrich_file_capture()  ── privacy check ──┤─ is_excluded_path() → REFUSE
   │   O_NOFOLLOW leaf open, size cap,    │
   │   binary sniffing, file_note meta    │
   ▼                                      ▼
render_action_prompt(action, capture)  ──►  metadata (cwd, git branch, file_note) appended
   │
   ▼
resolve session (chat) — explicit session_id → pinned → freshest → none (fresh send)
   │
   ▼
agent.send(AgentRequest { capture, action, prompt, session })
   │   (stdout/stderr drained concurrently; process group killed on timeout)
   ▼
SendOutcome { id, ok, agent_id, agent_name, action_id, capture, created_at, prompt, receipt, error }
```

Image captures hand off by path BY DESIGN: the privacy exclusions still run against the path
(path-only checks, no file opened), but the bytes are never read or binary-sniffed, so image
handoffs carry no `file_note`. Only `File` captures are read (≤ 1 MiB) and sniffed.

Errors are humanized (`AgentError` → plain messages; never raw stack traces in the UI).

On the desktop app, resolve happens under the daemon mutex (fast); `agent.send` runs on a
blocking thread pool so the palette stays responsive for multi-minute agents.

### Live feedback while sending

The palette never blocks on an agent: the daemon streams every stdout/stderr chunk over a
`handoff:output` event as it arrives, and a `handoff:started` event reports the rendered
prompt's byte length the moment the send begins. The chat view derives an **observable
phase** from that stream — *Sending context* (no output yet) → *Agent responding* (chunks
flowing) → *Wrapping up* (stream idle >3s, reverting if output resumes) — and renders it as
a quiet phase timeline inside a collapsible **Show activity** section, next to real
measurements: prompt size (measured in Rust), time to first response and output volume
(measured from the event stream). Nothing here is estimated or animated on a timer; a stat
that has not been observed yet renders as `—`, not `0`. The in-flight bubble shows a live
elapsed timer and a thinking animation only — streamed text (progress, thoughts, partial
reply) feeds the measurements and the collapsed activity output, never the bubble. Escape dismisses the palette but never cancels a
handoff in flight.

**Each exchange owns its own panel.** The live panel belongs to the turn in flight; when
that handoff completes, its two stream-only measurements are snapshotted onto the turn
(`promptBytes` from Rust, `firstResponseMs` from the stream) and the panel is re-rendered
in place. The other two stats are *derived* at render time from the outcome's receipt, so
they are never stored stale. Time to first response is the reason any of this is stored at
all: it only exists during the stream and cannot be recovered from the outcome afterwards.
One global panel was wrong here — it made turn 1 report turn 2's numbers.

Every turn also carries a **"Prompt that was sent"** disclosure: the exact rendered prompt
the agent received (which may differ from what was typed — a chat message goes through the
`ask` template, a captured file adds context headers), not just the text in the bubble.

Completed handoffs land in the same thread, outcome-first in shape — the agent's answer as
readable text, raw stdout/stderr and the prompt in expandable details — and the full
**result panel** (copy, follow-up, repeat, retry) is reachable from the history view.

### Session history

Every completed handoff is recorded in `HandoffHistory`, a session-only in-memory ring
buffer (most recent 10, newest first, behind its own lock so recording never blocks a
running agent). `SendOutcome` carries a stable `id` (`handoff-N`), the driving
`action_id`, the original `capture`, and a `created_at` timestamp so results can be
referenced by identity, grouped by day, and re-sent.

Consumers of the buffer:

- **Palette** — the history view groups entries by Today / Yesterday, with per-row
  **open · retry · duplicate · copy result · send to another agent** and a **Clear
  history** action.
- **Tray menu** — a *Recent Handoffs* submenu (rebuilt after each handoff) whose rows open
the matching result in the palette.
- **HTTP API** — `GET /history` (authenticated) returns the buffer for CLI/API clients.

History is **not persisted**; it dies with the process. SQLite persistence with retention
(`history_retention_days`) is planned.

## Session discovery, status & approvals

**Discovery** is stat-only: glob expansion over well-known session directories (or a
`cli-list` command for agents like Hermes whose sessions live in a database). Only
filenames + mtimes are read; the filename *is* the session id (per-agent extraction
rules: bare uuid, `rollout-<ts>-<uuid>`, `<ts>_<id>`), and Hermes session ids encode their
creation timestamp (`YYYYMMDD_HHMMSS_…`) as a freshness fallback. Glob expansion never
follows symlinks (a planted session file cannot redirect a handoff).

**Status** is a derived field: `working` when the transcript mtime is within the 30s
window, else `idle` (within the 7-day staleness cutoff). It reaches the palette as
`PalettePayload.sessions` on every open. On top of the session signal, each agent carries
an `AgentStatus.running` flag from a live process check (`pgrep` on the agent's configured
command, `sh -c` wrappers skipped, whole-path-component matching so `omp` never lights up
for `MTLCompilerService`): the UI's green status dot requires the process to actually be
running (or the session actively writing), so a leftover session file for a dead agent
reads offline and an open TUI (Hermes) reads online even during a quiet stretch.

**Live refresh:** while the palette is visible (home/chat steps), the frontend re-fetches
the payload every `STATUS_POLL_MS` (4s; injectable for tests). The merge is status-only —
open menu state, chat thread, and composer draft survive every tick — and polling stops on
history/result screens or when hidden.

**Provider health** (`daemon/src/provider_health.rs`): an installed binary can still fail
every send when its model provider is unreachable (a local model server that isn't up).
The daemon sniffs each agent's endpoint from its own config — grep of Hermes'
`config.yaml`, Codex/OpenCode config files — and, for Hermes, first reads the freshest
session's actual `billing_base_url` from `~/.hermes/state.db` (read-only sqlite3 under a
3s deadline — the sniff is a status read and can never hang the sender), because
a resumed session restores ITS provider, not the config default. Portless URLs (cloud
endpoints like `https://…/v1`) default to their scheme port (80/443) so they parse and
probe instead of silently falling back to a stale config default. The endpoint is re-sniffed
every ~30s — so switching a session between providers mid-flight is followed within a couple
of status polls — and probed with a sub-second TCP connect whose verdict is cached ~10s;
unrecognized setups fail open. Results:
`AgentStatus.provider_down` (amber dot + reason in the dropdown) and a **fail-fast refusal**
in `resolve_send` ("Cannot hand off to X: provider down (…)") instead of a full agent-timeout
hang. The send-time gate (`send_readiness`) probes the endpoint the send will
actually use — the target session's own provider for resumes (looked up by
session id), the ambient default for fresh sends — so a cloud-backed TUI
session stays sendable while a dead local default refuses fresh sends fast
instead of dying after retries. Model names are never displayed or chosen by Handover.

**Approvals** are the one deliberate privacy exception. An agent parked at a permission
prompt is *quiet*, indistinguishable from idle by mtime. Agents that opt in
(`permission_marker` + `approval_channel`) have only the last few KB of a live
session's transcript read for the marker (`core::approval::tail_contains_marker`); the
buffer is dropped immediately. Approval runs in two phases so injection + verification
polling never block the palette, the API, or handoffs in flight:

1. **Resolve** (under the daemon lock, fast): validate the agent's config (marker,
   channel, target), find the live session, re-check the session is *still* blocked
   (never inject into a session that moved on), build the injection command. Returns an
   `ApprovalRequest`.
2. **Execute** (without the daemon lock, slow): run the keystroke injection via the
   configured channel (`tty` device write / `tmux send-keys`; the `agent` channel is
   stubbed and fails loud), then poll the transcript for signs of life (marker gone or
   mtime advanced) within a 10s budget. Success is only claimed when verified; otherwise
   the caller gets "couldn't confirm — check the terminal".

Approval needs a transcript *file* — cli-list agents (Hermes) and remote/container
sessions are out of scope by design.

### Per-agent session facts (Phase 0)

The per-agent facts discovery is built on, verified on-machine 2026-08-14
(web research + `scripts/probe-sessions.sh`, which reads filenames + mtimes
only) and **re-verified 2026-10-04** (see "Adapter compatibility" below for the
per-agent verified versions). This table is the **maintenance point for agent
CLI drift**: keep it in sync with `SESSION_ID_RULES` in
`handover-core`, `builtin_session_agents()` in `handover-config`, and the
`ADAPTER_COMPAT` declaration.

| agent | sessions on disk | resume (interactive) | resume (headless) | streams |
|---|---|---|---|---|
| claude | `~/.claude/projects/<proj>/<uuid>.jsonl` (filename = id) | `claude --resume <id>` / `claude --continue` | `claude -p "{P}" --resume <id>` | `-p` streams |
| codex | `~/.codex/sessions/<Y>/<M>/<D>/rollout-<ts>-<uuid>.jsonl` (trailing uuid = id) | `codex resume <id>` / `codex resume` (freshest) | `codex exec resume <id>` / `codex exec --last` | exec streams, `-o json` |
| droid | `~/.factory/sessions/<enc-cwd>/<uuid>.jsonl` (per-session `.settings.json` sibling) | `droid --resume [id]` (`-r`, defaults to last modified) | `droid exec -s <id> "{P}"` / `droid exec -s <id> -f file` | `-o stream-json` / `-o json` |
| omp | `~/.omp/agent/sessions/<enc-cwd>/<ts>_<sessionId>.jsonl` (id after last `_`) + breadcrumbs `terminal-sessions/<tid>` | `omp -r <id>` (id prefix OK, picker if omitted) / `omp -c` | `omp -p "{P}" -r <id>` / `omp -p "{P}" -c` | `-p` streams, `--print-thoughts` |
| hermes | SQLite `~/.hermes/state.db` (FTS5) — no fs glob; use `hermes sessions list`. Id format `20260812_130220_dbf5cf` | `hermes --resume <id>` (`-r`) / `--continue` (`-c`) by id or title | `hermes chat -Q -q "{P}" --resume <id>` / `--resume latest --in <dir>` | `chat -q` streams; `-Q` prints only the final reply |

**Verified 2026-10-04** (`<agent> --version` + `scripts/probe-sessions.sh`, filenames
and mtimes only): claude 2.1.288, codex-cli 0.160.0, droid 0.233.0, omp 18.5.0,
opencode 2.0.19. **hermes** was verified separately on 2026-10-06 (0.21.5),
once its launcher could be run again; `hermes chat -q` now seeds an interactive
session on a TTY, so headless resumes pass `-Q` (or `--oneshot`). Per-agent versions are recorded in `ADAPTER_COMPAT`
(`handover-config`); see "Adapter compatibility" below.

#### Adapter compatibility

An "adapter" here is **not code** — it is the set of assumptions Handover makes
about one agent's CLI, spread across five tables keyed by the agent id string:

| Surface | Where |
|---|---|
| Session catalog (`command`, `resume_command`, discovery) | `builtin_session_agents()` in `handover-config` |
| Gallery catalog (`binary_name`, `candidate_paths`, `command_template`) | `agent_catalog()` in `handover-daemon` |
| Session-id rules (filename → id) | `SESSION_ID_RULES` in `handover-core` |
| Rotated-id shapes (hermes ts-hex vs uuid) | `ID_SHAPES` in `handover-agents` |
| Provider-config locations | `ENDPOINT_SNIFFERS` in `handover-daemon` |

When an agent changes its CLI, those assumptions go stale **silently**: a
renamed resume flag or a changed session filename still exits 0, so the handoff
reports *"Handed off successfully"* while the context landed in the wrong
conversation. Nothing in the architecture can catch that automatically without
probing every agent's `--version` on every status poll (a subprocess per agent
per 4s UI refresh) — deliberately not done.

Instead, `ADAPTER_COMPAT` in `handover-config::adapter` **declares** which agent
version each adapter was verified against, on what date, and which exact surface
would break. It is surfaced as `AgentMeta.compat` and printed by
`handover agents`, so a drift bug report carries evidence ("verified against
0.160.0" next to the reporter's actual version) instead of a guess.

This is a declaration, not a version-management system: no ranges, no resolver,
no dependency graph. An unverified adapter says `unverified` rather than
guessing (none currently; `hermes` was until 2026-10-06). The enforcement
is in tests — see CONTRIBUTING.md, "Updating an agent adapter".

Key findings from the verification:

- **Hermes is SQLite, not files** — session discovery must shell out to
  `hermes sessions list` (read-only metadata) or query the DB; the
  "glob filename = id" trick does not apply. Everything else globs.
- **Session-id extraction is not uniform**: the codex id is the trailing uuid
  after `rollout-<ts>-`; the omp id is the part after the last `_`; droid and
  claude are bare uuids; hermes ids are ts-based strings. `omp --profile`
  relocates the whole `~/.omp/agent` tree, so profile-aware sessions would
  need the profile path, not a fixed glob.
- **omp writes terminal breadcrumbs** (`terminal-sessions/<terminal-id>`:
  cwd + session file + optional `fresh` line) — a potential stronger "the
  session the user was just in" signal than raw mtime.
- **droid**: `droid search` exists for session search; exec streams structured
  output; permission model via `--auto` levels + `--skip-permissions-unsafe`
  (an approval-layer surface).
- **hermes**: `hermes approvals` subcommand + status bar show session state;
  `--yolo` auto-approves. Approval-layer facts (marker formats, tty-injection
  viability) still need per-agent manual verification with a real running
  agent.

## Config recovery

`Config::ensure` / `ensure_at`:

| Situation | Behavior |
|---|---|
| Missing file | Write defaults |
| TOML parse error | Rename to `*.toml.bak` **only if rename succeeds**, then write defaults. If rename fails, leave the file untouched and use in-memory defaults |
| Other I/O (permissions, etc.) | Leave the file untouched; use in-memory defaults |

Prompt files for `{PROMPT_FILE}` live under a private cache directory (`…/handover/prompts`),
mode `0600`, with RAII cleanup on every exit path. The location is overridable with
`HANDOVER_PROMPT_CACHE_DIR` so the test suite can run in sandboxed / CI environments
without touching the user's real cache.

## Lock discipline

One `Mutex<Daemon>` guards all mutable state, so the rule is simple and absolute: **never
hold it across a filesystem walk, a subprocess, or an agent run.** Every slow path is split
in two — take a cheap snapshot under the lock (`agents_status_snapshot`,
`live_sessions_snapshot`, `resolve_send_plan`, `resolve_approval_plan`), drop the guard, then
do the slow work (`compute_agents_status`, `compute_live_sessions`, `complete_send`,
`complete_approval`, `execute_handoff`). Holding the lock across any of those would freeze
`/health`, `/status`, the palette, and every handoff in flight.

Access always goes through `SharedDaemon::lock()`, never `.0.lock().unwrap()`: a panic
while the lock is held poisons it for the rest of the process, and since each HTTP request
runs on its own thread that would turn one bad thread into a permanently broken API. The
helper recovers the guard and logs the poisoning instead.

## Process model

- **Desktop app**: embeds the daemon crate in-process, serves the local HTTP API on
  127.0.0.1:47444, owns the tray/menu-bar icon and the global hotkey, and shows the palette
  window on demand. Settings is **lazy-created** (built on first open, destroyed on close)
  so idle runs keep a single WebKit UI. No dock icon on macOS (menu-bar utility /
  `ActivationPolicy::Accessory`). Long handoffs use async Tauri commands + `spawn_blocking`.
  Expect Apple WebKit helper processes (`WebKit.Networking`, etc.) while the UI is loaded —
  that is normal for Tauri, not a second app.
- **Daemon alone**: `handover-daemon` runs the same crate as a standalone background
  process (useful on Linux without a tray, or when only the CLI is wanted).
- **CLI**: talks HTTP to whichever process is serving the API (loads `api_token`
  automatically). If a port is already bound by another instance, the desktop app logs a
  warning and lets the CLI use that instance.

## Privacy enforcement points

| Layer | Mechanism |
|---|---|
| Config | `[privacy] excluded_paths` glob list, extensible by the user |
| Core | `is_excluded_path()` matches path against exclusions **before any read** (case-insensitive) |
| Daemon | File captures **and palette drops**: exclusion check → resolve-path check → `O_NOFOLLOW` open of the **leaf** → size cap → binary/UTF-8 sniffing. Intermediate directory TOCTOU is out of scope (trusted parents / single-user desktop). |
| Network | API bound to loopback only; mutating routes require a local bearer token; `GET /agents` redacts command lines to `"configured command"`; unauthenticated GET bursts are bounded by an in-flight request cap (overflow → 503) |
| Session discovery | filenames + mtimes only; symlinks never followed; cli-list (Hermes) is a read-only metadata listing — **never** transcript contents |
| Approval (opt-in) | the ONE exception: only agents with a `permission_marker` configured have the last 8 KB of a live session's transcript tail-peeked for that marker; buffer dropped immediately, never stored or sent; `blocked` stays `None` for every other agent |

## The HTTP API (127.0.0.1:47444)

| Method | Path | Auth | Purpose |
|---|---|---|---|
| GET | `/health` | no | daemon liveness |
| GET | `/status` | no | version, port, platform, action/agent counts, config path, `max_agent_timeout_secs`, `default_agent_timeout_secs`, `auth_required` |
| GET | `/actions` | no | available actions |
| GET | `/agents` | no | configured agents + availability status |
| GET | `/preferences` | no | read per-action agent preferences |
| POST | `/preferences` | **yes** | set per-action agent preferences |
| POST | `/render` | **yes** | render a prompt without sending |
| POST | `/send` | **yes** | perform a handoff (`action_id`, optional `agent_id`, optional `session_id`, `capture`) — the outcome is recorded into the session history |
| GET | `/history` | **yes** | recent handoffs this session, newest first (capped at 10; contains full prompts + replies, hence auth) |
| GET | `/sessions` | **yes** | live sessions, freshest first, with activity + optional `blocked` state (reveals which agents are running — hence auth) |
| POST | `/sessions/pin` | **yes** | pin a session (`agent_id`, optional `session_id` — pins the freshest when absent; optional `tty` records the approval target) |
| POST | `/sessions/unpin` | **yes** | clear a pin (`agent_id`) |
| POST | `/sessions/approve` | **yes** | approve/deny a blocked session (`agent_id`, `session_id`, `approve`) — re-check, inject, verify |
| POST | `/quit` | **yes** | stop the daemon |

Mutating routes require `Authorization: Bearer <token>` (or `X-Handover-Token`).
The token is stored next to the platform config as `api_token` (mode `0600`);
`config.toml` itself is also written owner-only (`0600`), since it can carry
agent commands and env vars.

The CLI uses this API exclusively (and loads the token automatically). The port can be
overridden with `HANDOVER_PORT`; the token with `HANDOVER_TOKEN`.

## Desktop UI

Two windows, one frontend (the React bundle routes by window label):

- **Palette** (`main`) — a borderless, always-on-top, centered window sized for the
  handoff sheet + floating dock; shown by the global hotkey, hidden on Escape. With no
  native chrome, the header strip doubles as the window drag region
  (`data-tauri-drag-region`). A handoff in flight is never cancelled by hiding it.
- **Settings** (`settings`) — created **only when opened** (menu / `⌘,` / tray / gear).
  Closing it destroys the webview. On macOS it uses `TitleBarStyle::Overlay` (traffic
  lights over the sidebar brand strip / drag region), gated by `html.is-mac`.

**Global hotkey:** registered with `on_shortcut` alone (that call already binds
Carbon/global-hotkey). Do **not** also call `register()` for the same shortcut — double
registration fails with `RegisterEventHotKey failed for KeyA` and used to be misread as
an Accessibility issue.

**Tray icon (macOS):** monochrome **template** asset (`trayTemplate@2x.png`) with
`icon_as_template(true)` so the menu bar tints it for light/dark. Dock / `.icns` stay
the full-color app mark. Both are generated from the brand kit (`brand/`, see its
README), not edited by hand.

**Glass / opacity:** CSS tokens use `rgba(..., var(--ui-opacity))`. `general.ui_opacity`
(0.4–1.0) is edited in Settings → General and broadcast as `ui-opacity:changed` so
palette and Settings stay in sync. Both windows are transparent so the alpha is visible.

The UI is chat-first: the palette opens on a single merged surface — `AgentDropdown`
(trigger pill naming the current agent; menu rows are logo + name + one status dot,
green/red/amber, with `blocked · approve?` or the provider-down reason as the only
sub-lines) directly above `ChatView` (threaded chat that resumes the agent's live
session, composer opens clean — the clipboard is never read, only dropped text/files
pre-fill it — `Enter` sends / `Shift+Enter` newline, inline Approve/Deny for blocked
sessions, and a thinking-only in-flight bubble — no streamed text in the thread). Picking an agent
swaps the thread in place; there is no separate landing screen (`ChatHome` was removed).
`ResultPanel`, `HistoryList`, `WelcomeOverlay`, `ShortcutSheet` and `ErrorState`
round out the rest, all over a typed IPC layer (`lib/tauri.ts`), with a design-token CSS
split (`tokens.css` / `palette.css` / `settings.css` / `fonts.css`). The visual language —
an Apple-native floating glass utility (very large continuous radii, pill bars, circular
icon targets, quiet grayscale materials with inset highlights) in the brand's monochrome
palette (ink accent in light mode, snow in dark, colour only for status) and Geist /
Geist Mono (bundled, OFL) — lives entirely in the tokens; components consume tokens and
never hard-code colors. The brand mark is drawn inline (`HandoverMark` in `Icons.tsx`).
First-run onboarding is a teach-by-doing overlay (dismissed by the hotkey again); a `?`
cheat sheet lists the shortcuts; drag-and-drop capture accepts text via DOM events and
file paths via Tauri's native `tauri://drag-enter/leave/drop` events (WKWebView blocks
`dataTransfer.files`).

**Event contract** (Rust → frontend):

| Event | Payload | Meaning |
|---|---|---|
| `palette:open` | – | global hotkey pressed — show the sheet |
| `palette:history` / `palette:handoff` | `id?` | open the history view / a specific handoff |
| `handoff:started` | `{ id, prompt_len }` | send began — rendered prompt byte size |
| `handoff:output` | `{ id, stream, chunk }` | live stdout/stderr chunk |
| `appearance:changed` | `system \| light \| dark` | re-theme CSS tokens live |
| `ui-opacity:changed` | `f64` (0.4–1.0) | update `--ui-opacity` on every webview |
| `settings:tab` | `tab` | switch the Settings tab |
| `tauri://drag-enter/leave/drop` | `{ paths }` (drop) | external file drags |

The production bundle ships a Content Security Policy (`app.security.csp`) — never
disabled.

## Why not …?

- **A backend server?** The core loop works fully offline. A server would violate the
  local-first principle and the "utility, not platform" constraint.
- **An agent orchestration system?** Handover hands off to one agent per action. No chains,
  no routing graphs, no autonomous loops.
- **Big dependencies?** The daemon's only HTTP stack is `tiny_http` + `ureq` (client in CLI).
  `chrono`, `serde`, `uuid`, `toml` for the rest. Heavier things (image handling) are added
  only when a feature actually needs them (image captures).
