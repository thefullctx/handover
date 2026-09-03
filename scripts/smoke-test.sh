#!/usr/bin/env bash
# End-to-end smoke test for the Handover vertical slice:
# daemon -> local HTTP API -> CLI -> generic command agent.
#
# Uses a throwaway HOME so it never touches your real config.
set -euo pipefail
cd "$(dirname "$0")/.."

export HANDOVER_PORT="${HANDOVER_PORT:-47445}"
# Preserve toolchain homes before redirecting HOME (otherwise rustup loses its default).
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export PATH="${CARGO_HOME}/bin:${PATH}"

echo "== build (daemon + cli) =="
cargo build -p handover-daemon -p handover-cli 2>&1 | tail -5

TEST_HOME="$(mktemp -d)"
export HOME="$TEST_HOME"

# Session-aware demo agent (Phase 6 coverage: sessions/attach/resume) driven
# by the fake-agent harness — a script that OWNS a session dir and appends to
# transcripts like a real agent (discovery -> resume -> receipt realism).
chmod +x scripts/fake-agent.sh
# The config's resume_command references the script by absolute path — copy
# it into the throwaway home so the daemon (and the fake agent's own writes)
# stay fully inside the test home.
cp scripts/fake-agent.sh "$TEST_HOME/fake-agent.sh"
mkdir -p "$TEST_HOME/fake-sessions"
export FAKE_AGENT_DIR="$TEST_HOME/fake-sessions"

for CFG in \
  "$TEST_HOME/Library/Application Support/handover/config.toml" \
  "$TEST_HOME/.config/handover/config.toml"; do
  mkdir -p "$(dirname "$CFG")"
  cat >"$CFG" <<EOF
[general]
quick_send = true

[[agents]]
id = "demo-echo"
name = "Echo (demo)"
kind = "command"
command = "sh -c 'tee \"\$HANDOVER_DEMO_SINK\"'"
description = "Records every handoff for end-to-end verification (demo only)."
timeout_secs = 60
enabled = true
demo = true

[[agents]]
id = "sess-echo"
name = "Session Echo (demo)"
kind = "session"
command = "sh -c 'tee /tmp/handover-sess-handoff.txt'"
resume_command = "$TEST_HOME/fake-agent.sh --resume {SESSION} {PROMPT}"
session_glob = "$TEST_HOME/fake-sessions/*/*.jsonl"
permission_marker = "[permission]"
approval_channel = "tty"
timeout_secs = 60
enabled = true
demo = true
EOF
done

echo "== start daemon on :$HANDOVER_PORT =="
./target/debug/handover-daemon --port "$HANDOVER_PORT" >"$TEST_HOME/daemon.log" 2>&1 &
DAEMON_PID=$!
trap 'kill "$DAEMON_PID" 2>/dev/null || true; rm -rf "$TEST_HOME"; rm -f "$HOME/.handover/demo-handoff.txt" || true' EXIT

for _ in $(seq 1 50); do
  if curl -sf "http://127.0.0.1:$HANDOVER_PORT/health" >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done

echo "== health =="
curl -sf "http://127.0.0.1:$HANDOVER_PORT/health"
echo

echo "== handover status =="
./target/debug/handover status

echo "== handover agents =="
./target/debug/handover agents

echo "== handover actions =="
./target/debug/handover actions

echo "== send via stdin (fix) =="
printf 'ECONNREFUSED 127.0.0.1:5432' | ./target/debug/handover send --action fix

echo "== verify demo agent recorded the handoff =="
cat "$HOME/.handover/demo-handoff.txt"

echo "== send a file (explain) =="
printf 'panic: something exploded\n' >"$TEST_HOME/error.log"
./target/debug/handover send "$TEST_HOME/error.log" --action explain

echo "== print prompt without sending =="
./target/debug/handover send --action ask --print-prompt <<< "hello"

echo "== excluded .env must be refused =="
printf 'SECRET=1\n' >"$TEST_HOME/.env"
if ./target/debug/handover send "$TEST_HOME/.env" --action ask >/dev/null 2>&1; then
  echo "FAIL: .env should have been refused"
  exit 1
else
  echo "OK: .env refused"
fi

echo "== locate API token (platform config dir under TEST HOME) =="
TOKEN_FILE=""
for candidate in \
  "$TEST_HOME/Library/Application Support/handover/api_token" \
  "$TEST_HOME/.config/handover/api_token"; do
  if [ -f "$candidate" ]; then
    TOKEN_FILE="$candidate"
    break
  fi
done
if [ -z "$TOKEN_FILE" ]; then
  echo "FAIL: api_token not created under test HOME"
  find "$TEST_HOME" -name api_token 2>/dev/null || true
  exit 1
fi
TOKEN="$(tr -d '[:space:]' <"$TOKEN_FILE")"
echo "token file: $TOKEN_FILE"

echo "== preferences (auth required) =="
# Unauthenticated POST must fail.
if curl -sf -X POST "http://127.0.0.1:$HANDOVER_PORT/preferences" \
  -d '{"action_id":"fix","agent_id":"demo-echo"}' >/dev/null 2>&1; then
  echo "FAIL: unauthenticated POST /preferences should be rejected"
  exit 1
else
  echo "OK: unauthenticated preferences rejected"
fi
curl -sf -X POST "http://127.0.0.1:$HANDOVER_PORT/preferences" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"action_id":"fix","agent_id":"demo-echo"}'
echo
curl -sf "http://127.0.0.1:$HANDOVER_PORT/preferences"
echo

echo "== create live session files via the fake agent (2222 freshest) =="
./scripts/fake-agent.sh init 11111111-0000-4000-8000-000000000000 >/dev/null
./scripts/fake-agent.sh init 22222222-0000-4000-8000-000000000000 >/dev/null
./scripts/fake-agent.sh reply 22222222-0000-4000-8000-000000000000 "ready" >/dev/null
# Make 1111 older so 2222 is the freshest (attach pins freshest).
touch -t 202608121400.00 "$TEST_HOME/fake-sessions/proj/11111111-0000-4000-8000-000000000000.jsonl" 2>/dev/null || true

echo "== handover sessions =="
./target/debug/handover sessions | tee "$TEST_HOME/sessions.out"
grep -q "sess-echo" "$TEST_HOME/sessions.out" || { echo "FAIL: sessions missing sess-echo"; exit 1; }
grep -q "22222222-0000-4000-8000-000000000000" "$TEST_HOME/sessions.out" || { echo "FAIL: freshest session missing"; exit 1; }

echo "== handover attach (pins the freshest session) =="
./target/debug/handover attach sess-echo | tee "$TEST_HOME/attach.out"
grep -q "22222222-0000-4000-8000-000000000000" "$TEST_HOME/attach.out" || { echo "FAIL: attach did not pin freshest"; exit 1; }

echo "== send resumes into the pinned session =="
printf 'keep going' | ./target/debug/handover send --agent sess-echo --action ask | tee "$TEST_HOME/send1.out"
grep -q "session: 22222222-0000-4000-8000-000000000000" "$TEST_HOME/send1.out" || { echo "FAIL: send did not resume pinned session"; exit 1; }
# The fake agent appended the prompt to the pinned session's transcript.
grep -q "keep going" "$TEST_HOME/fake-sessions/proj/22222222-0000-4000-8000-000000000000.jsonl" \
  || { echo "FAIL: resume did not append to the session transcript"; exit 1; }

echo "== send --session overrides the pin =="
printf 'pick this one' | ./target/debug/handover send --agent sess-echo --action ask \
  --session 11111111-0000-4000-8000-000000000000 | tee "$TEST_HOME/send2.out"
grep -q "session: 11111111-0000-4000-8000-000000000000" "$TEST_HOME/send2.out" || { echo "FAIL: --session did not override the pin"; exit 1; }

echo "== attach --unpin =="
./target/debug/handover attach --unpin sess-echo

# Unpinned: freshest-first resumes. The fake agent most recently wrote to
# 1111 (the --session send above), so 1111 is now the freshest — that IS
# freshest-first, with real agent activity driving the ordering.
echo "== send after unpin targets the freshest session again =="
printf 'again' | ./target/debug/handover send --agent sess-echo --action ask | tee "$TEST_HOME/send3.out"
grep -q "session: 11111111-0000-4000-8000-000000000000" "$TEST_HOME/send3.out" || { echo "FAIL: unpin did not restore freshest-first"; exit 1; }

echo "== approval: block the freshest session via the fake agent =="
./scripts/fake-agent.sh block 22222222-0000-4000-8000-000000000000
./target/debug/handover sessions | tee "$TEST_HOME/sessions-blocked.out"
grep -q "blocked" "$TEST_HOME/sessions-blocked.out" || { echo "FAIL: blocked state not shown"; exit 1; }

echo "== approval: record the real pty as the injection target =="
# The tty channel injects into a terminal device; a file path is (correctly)
# rejected. A pty fixture plays the session's terminal: keystrokes injected
# into it become transcript activity, which is exactly what approval
# verification watches.
PTY_SINK_PID=""
clean_pty_sink() { [ -n "$PTY_SINK_PID" ] && kill "$PTY_SINK_PID" 2>/dev/null || true; }
PTY_SINK_PID="$(python3 scripts/approval-pty-sink.py "$TEST_HOME/pty-dev" "$TEST_HOME/fake-sessions/proj/22222222-0000-4000-8000-000000000000.jsonl" >/dev/null 2>&1 & echo $!)"
for _ in $(seq 1 50); do
  [ -s "$TEST_HOME/pty-dev" ] && break
  sleep 0.1
done
PTY_DEV="$(cat "$TEST_HOME/pty-dev")"
./target/debug/handover attach sess-echo --tty "$PTY_DEV"

echo "== handover approve (transcript resumes -> verified) =="
./target/debug/handover approve sess-echo | tee "$TEST_HOME/approve.out"
grep -q "✓" "$TEST_HOME/approve.out" || { echo "FAIL: approve not verified: $(cat "$TEST_HOME/approve.out")"; exit 1; }

echo "== handover approve --deny =="
./scripts/fake-agent.sh block 22222222-0000-4000-8000-000000000000
./target/debug/handover approve sess-echo --deny | tee "$TEST_HOME/deny.out"
grep -q "✓" "$TEST_HOME/deny.out" || { echo "FAIL: deny not verified: $(cat "$TEST_HOME/deny.out")"; exit 1; }
clean_pty_sink

echo "== quit daemon =="
curl -sf -X POST "http://127.0.0.1:$HANDOVER_PORT/quit" \
  -H "Authorization: Bearer $TOKEN" || true
wait "$DAEMON_PID" 2>/dev/null || true

echo
echo "ALL SMOKE TESTS PASSED ✓"
