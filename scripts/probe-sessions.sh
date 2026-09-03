#!/usr/bin/env bash
# probe-sessions.sh — Phase 0 dev tool for session-aware handoff.
#
# Lists the most recently touched agent session files on this machine, one
# line per session, freshest first. Used to verify the session-discovery
# assumptions in docs/new_idea_for_handover.txt before any product code is written.
#
# PRIVACY: this tool reads FILENAMES and MTIMES only. It never opens or reads
# session/transcript contents. The one exception is Hermes, which stores
# sessions in a SQLite DB (~/.hermes/state.db) with no filesystem glob — for
# it we shell out to `hermes sessions list` (a read-only metadata listing)
# instead of touching the DB ourselves.
#
# Usage: scripts/probe-sessions.sh [--limit N] [agent ...]
#   --limit N   show at most N sessions per agent (default 5)
#   agent ...   only probe the named agents (default: all known)
set -euo pipefail

LIMIT=5
AGENTS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --limit) LIMIT="$2"; shift 2 ;;
    -h|--help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) AGENTS+=("$1"); shift ;;
  esac
done
if [ "${#AGENTS[@]}" -eq 0 ]; then
  AGENTS=(claude codex droid omp hermes)
fi

HOME_DIR="${HOME:-$HOME}"

# mtime_seconds FILE — prints the file's mtime as epoch seconds (BSD/GNU).
mtime_seconds() {
  if stat -f '%m' "$1" >/dev/null 2>&1; then
    stat -f '%m' "$1"
  else
    stat -c '%Y' "$1"
  fi
}

# fmt_time EPOCH — prints an ISO-ish local timestamp.
fmt_time() {
  if date -r "$1" '+%Y-%m-%d %H:%M:%S' >/dev/null 2>&1; then
    date -r "$1" '+%Y-%m-%d %H:%M:%S'
  else
    date -d "@$1" '+%Y-%m-%d %H:%M:%S'
  fi
}

# session_id_for_agent AGENT FILENAME — best-effort session-id extraction
# from the filename. Kept next to the per-agent glob so the catalog table in
# the docs stays the single source of truth.
session_id_for_agent() {
  local agent="$1" name="$2"
  case "$agent" in
    claude)
      # <uuid>.jsonl  ->  uuid
      basename "$name" .jsonl
      ;;
    codex)
      # rollout-<ts>-<uuid>.jsonl  ->  <uuid> (uuid part after the timestamp)
      basename "$name" .jsonl | sed -E 's/^rollout-[0-9T:+-]+-//'
      ;;
    droid)
      # <uuid>.jsonl  ->  uuid
      basename "$name" .jsonl
      ;;
    omp)
      # <timestamp>_<sessionId>.jsonl  ->  <sessionId> (part after the last _)
      basename "$name" .jsonl | sed -E 's/^[^_]+_[0-9a-f-]+_//; s/^[^_]+_//'
      ;;
    *) basename "$name" ;;
  esac
}

# probe_glob AGENT GLOB — expand a glob, sort by mtime desc, print top N.
probe_glob() {
  local agent="$1" glob="$2"
  local files=()
  # shellcheck disable=SC2086
  while IFS= read -r f; do files+=("$f"); done < <(find $glob -type f 2>/dev/null || true)
  if [ "${#files[@]}" -eq 0 ]; then
    echo "  (no sessions found)"
    return
  fi
  local i m
  for f in "${files[@]}"; do
    m="$(mtime_seconds "$f")"
    printf '%s\t%s\t%s\n' "$m" "$f" "$(session_id_for_agent "$agent" "$f")"
  done | sort -rn | head -n "$LIMIT" | while IFS=$'\t' read -r m f sid; do
    printf '  %s  %s  %s\n' "$(fmt_time "$m")" "$sid" "$f"
  done
}

probe_hermes() {
  if ! command -v hermes >/dev/null 2>&1; then
    echo "  (hermes not installed)"
    return
  fi
  # Hermes keeps sessions in ~/.hermes/state.db (SQLite, FTS5) — no
  # filesystem glob. Read-only metadata listing via the CLI itself.
  if hermes sessions list 2>/dev/null | head -n "$((LIMIT + 1))" | tail -n "$LIMIT"; then
    :
  else
    echo "  (hermes sessions list failed — needs manual check)"
  fi
}

for agent in "${AGENTS[@]}"; do
  echo "== $agent =="
  case "$agent" in
    claude)  probe_glob claude "$HOME_DIR/.claude/projects/*/*.jsonl" ;;
    codex)   probe_glob codex "$HOME_DIR/.codex/sessions/*/*/*/*.jsonl" ;;
    droid)   probe_glob droid "$HOME_DIR/.factory/sessions/*/*.jsonl" ;;
    omp)     probe_glob omp "$HOME_DIR/.omp/agent/sessions/*/*.jsonl" ;;
    hermes)  probe_hermes ;;
    *) echo "  (unknown agent: $agent)" ;;
  esac
  echo
done
