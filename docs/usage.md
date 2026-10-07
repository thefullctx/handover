# Using the palette

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
  e.g. your local model server isn't up. On the right, a small detail says what you'd
  resume into: `working`, `session · <time>` for a quiet session, or `installed` when
  there is none yet. A blocked agent keeps its
  `blocked · approve?` line (the pending question on hover); Approve/Deny live in the
  chat's approval card. The lights refresh every few seconds while the palette is open —
  start an agent in a terminal and watch it flip red→green without closing anything.
- **Chat** — the composer opens clean (the clipboard is never captured; dropped text/files
  pre-fill it, editable, never sent silently): `Enter` sends verbatim (`Shift+Enter` is a
  newline), and every message resumes the same session so the agent keeps the
  conversation context. While waiting you see only a quiet thinking animation, then the
  finished reply; progress text, thoughts and CLI banners the agent prints along the way
  are never shown in the thread.

The first time you chat with an agent it becomes your default — next time you can go straight
through: `⌘⇧A → Enter → Enter`.

## Handoff feedback

You are never left wondering what happened after the handoff:

- **Sending** — the in-flight turn shows a quiet thinking animation and a live elapsed timer
  (`Codex is working · 12s`); the reply appears once the agent finishes. The raw output
  stream is under **Show activity** if you want it. Press `esc` to dismiss
  the palette — the handoff keeps running and a **native macOS notification** still fires on
  completion (the banner belongs to Handover; clicking it activates the app, not Script Editor).
- **Activity** — every exchange carries its own collapsible **Show activity** section holding the raw agent output plus a phase
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

← [Back to the README](../README.md)
