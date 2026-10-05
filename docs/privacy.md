# Privacy model

- **Local-first**: the daemon binds to `127.0.0.1` only. Nothing leaves your machine except
  what the agent you chose sends.
- **Local API auth**: mutating routes require the bearer token (see [Local API token](configuration.md#local-api-token)) so other local
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

← [Back to the README](../README.md)
