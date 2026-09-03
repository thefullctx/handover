#!/usr/bin/env bash
# Fake-agent harness for CI — closes the "test realism gap".
#
# A script that OWNS a session dir the way a real agent does: it creates
# session transcripts, appends to them (JSONL, like real agent transcripts),
# answers a prompt, and can emit a permission marker. This lets the smoke
# test drive discovery -> resume -> receipt and approval detect -> inject ->
# verify against a transcript that actually grows — not just canned echoes.
#
# As an agent command in config.toml (env FAKE_AGENT_DIR = session root):
#   fake-agent.sh "{PROMPT}"                  # fresh send: writes to the
#                                             #   freshest session transcript
#   fake-agent.sh --resume <id> "{PROMPT}"    # resume: appends to that
#                                             #   session, echoes RESUME:<id>
#
# As a fixture creator:
#   fake-agent.sh init <session-id>           # create an empty transcript
#   fake-agent.sh touch <session-id>          # bump mtime (activity)
#   fake-agent.sh block <session-id>          # append a permission marker
#   fake-agent.sh reply <session-id> [text]   # append an assistant line
#
# Sessions live at $FAKE_AGENT_DIR/<scope>/<session-id>.jsonl. The freshest
# session is the default target for fresh sends (mirrors real discovery).
# This is a test fixture: it quotes JSON trivially, so keep prompts simple.
set -euo pipefail

FAKE_AGENT_DIR="${FAKE_AGENT_DIR:-$HOME/.fake-agent}"
SCOPE="${FAKE_AGENT_SCOPE:-proj}"

sess_path() { printf '%s/%s/%s.jsonl' "$FAKE_AGENT_DIR" "$SCOPE" "$1"; }

freshest() {
  find "$FAKE_AGENT_DIR/$SCOPE" -name '*.jsonl' -type f -print0 2>/dev/null \
    | xargs -0 ls -t 2>/dev/null | head -1 || true
}

json_str() { printf '%s' "$1" | sed 's/"/\\"/g'; }

cmd="${1:-}"
shift || true
case "$cmd" in
  init)
    id="${1:?init <session-id>}"
    mkdir -p "$FAKE_AGENT_DIR/$SCOPE"
    : > "$(sess_path "$id")"
    printf '%s\n' "$id"
    ;;
  touch)
    id="${1:?touch <session-id>}"
    touch "$(sess_path "$id")"
    ;;
  block)
    id="${1:?block <session-id>}"
    printf 'assistant: needs input\n[permission] approve shell command?\n' >> "$(sess_path "$id")"
    ;;
  reply)
    id="${1:?reply <session-id>}"
    printf 'assistant: %s\n' "${2:-done}" >> "$(sess_path "$id")"
    ;;
  --resume)
    id="${1:?--resume <session-id> <prompt>}"
    prompt="${2:-}"
    mkdir -p "$FAKE_AGENT_DIR/$SCOPE"
    printf '{"role":"user","content":"%s"}\n' "$(json_str "$prompt")" >> "$(sess_path "$id")"
    printf 'RESUME:%s\n' "$id"
    ;;
  *)
    # Fresh send: the first positional IS the prompt; write to the freshest
    # session (or create a new one), like a real agent would.
    prompt="$cmd"
    target="$(freshest)"
    if [ -z "$target" ]; then
      mkdir -p "$FAKE_AGENT_DIR/$SCOPE"
      target="$(sess_path "fresh-$(date +%s)")"
      : > "$target"
    fi
    printf '{"role":"user","content":"%s"}\n' "$(json_str "$prompt")" >> "$target"
    printf 'FRESH:%s\n' "$(basename "$target" .jsonl)"
    ;;
esac
