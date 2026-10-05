# Live sessions, status & approvals

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

## Approvals (opt-in)

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

Agents, resume commands and approval fields are configured in [configuration.md](configuration.md#session-aware-agent-fields).

← [Back to the README](../README.md)
