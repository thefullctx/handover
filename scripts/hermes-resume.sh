#!/bin/sh
# Hermes resume wrapper for Handover.
#
# Why this exists: `hermes chat --resume <id>` restores the session's MODEL
# but keeps the AMBIENT provider from config.yaml whenever the session lacks
# TUI gateway-runtime metadata (true for every CLI-created session). If your
# config default is a local model server and the session was using a cloud
# provider (or vice versa), the resume dials the WRONG endpoint — you get
# either "HTTP 401: Model X is not supported" or "API call failed after 3
# retries: Connection error".
#
# Fix: read the session's actual model + provider from Hermes' own state.db
# and pass BOTH explicitly. Falls back to a plain resume when the row has no
# provider info (fresh/legacy sessions where ambient defaults are correct).
#
# Usage (matches Handover's resume_command template):
#   hermes-resume.sh <SESSION_ID> "<PROMPT>"
set -eu

SESSION="${1:-}"
PROMPT="${2:-}"
if [ -z "$SESSION" ] || [ -z "$PROMPT" ]; then
    echo "usage: hermes-resume.sh <SESSION_ID> \"<PROMPT>\"" >&2
    exit 2
fi

# Resolve the hermes binary portably: PATH first, then the common install
# locations (mirrors daemon resolve_cli_program). Never hard-code a $HOME.
resolve_hermes() {
    if command -v hermes >/dev/null 2>&1; then
        command -v hermes
        return 0
    fi
    home="${HOME:-}"
    for dir in "$home/.local/bin" "$home/bin" "/opt/homebrew/bin" "/usr/local/bin"; do
        if [ -n "$dir" ] && [ -x "$dir/hermes" ] && [ -f "$dir/hermes" ]; then
            printf '%s\n' "$dir/hermes"
            return 0
        fi
    done
    return 1
}

if ! HERMES="$(resolve_hermes)"; then
    echo "hermes-resume.sh: could not find the \`hermes\` binary (PATH, ~/.local/bin, ~/bin, /opt/homebrew/bin, /usr/local/bin)." >&2
    exit 127
fi

# Session ids are timestamp-prefixed tokens (YYYYMMDD_HHMMSS_...): only allow
# a safe alphabet for the DB lookup. Anything else skips the sniff and falls
# back to a plain resume — the id itself is still passed as argv (no shell),
# so this only guards the SQL string.
row=""
DB_HOME="${HOME:-}"
DB="$DB_HOME/.hermes/state.db"
if [ -n "$DB_HOME" ] && [ -f "$DB" ] && command -v sqlite3 >/dev/null 2>&1; then
    case "$SESSION" in
        *[!A-Za-z0-9._-]*)
            row=""
            ;;
        *)
            # Escape single quotes by doubling (SQL string literal rule).
            esc_session=$(printf '%s' "$SESSION" | sed "s/'/''/g")
            row=$(sqlite3 -readonly "$DB" \
                "SELECT COALESCE(model,''),COALESCE(billing_provider,'')
                 FROM sessions WHERE id='$esc_session' LIMIT 1;" 2>/dev/null) || row=""
            ;;
    esac
fi

MODEL=$(printf '%s' "$row" | cut -d'|' -f1)
PROVIDER=$(printf '%s' "$row" | cut -d'|' -f2)

set --
[ -n "$PROVIDER" ] && set -- "$@" --provider "$PROVIDER"
[ -n "$MODEL" ] && set -- "$@" -m "$MODEL"

exec "$HERMES" chat -Q -q "$PROMPT" --reasoning none "$@" --resume "$SESSION"
