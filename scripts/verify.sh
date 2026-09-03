#!/usr/bin/env bash
# verify.sh — full verification matrix for Handover, from repo root:
#   fmt --check, clippy, workspace tests, UI lint, UI tests, UI build, smoke.
#
# Run everything a release needs before shipping: `bash scripts/verify.sh`.
# Each step reports PASS/FAIL; the script exits non-zero if any step failed.
set -uo pipefail
cd "$(dirname "$0")/.."
LOG="$(mktemp)"
trap 'rm -f "$LOG"' EXIT
FAILED=()

step() { printf '\n== %s ==\n' "$1"; }
pass()  { printf 'PASS: %s\n' "$1"; }
fail()  { printf 'FAIL: %s\n' "$1"; FAILED+=("$1"); }

# -- Rust --------------------------------------------------------------
step "cargo fmt --check"
if cargo fmt --check; then pass "fmt"; else fail "cargo fmt --check"; fi

step "cargo clippy"
cargo clippy --workspace --all-targets >"$LOG" 2>&1
if [ $? -ne 0 ]; then
  fail "clippy"
elif grep -E '^warning|^error' "$LOG" >/dev/null; then
  fail "clippy"
else
  pass "clippy"
fi

step "cargo test --workspace"
cargo test --workspace >"$LOG" 2>&1
if [ $? -ne 0 ]; then
  fail "workspace tests"
else
  grep '^test result:' "$LOG" | sort | uniq -c | sed 's/^/  /'
  pass "workspace tests"
fi

# -- UI ----------------------------------------------------------------
UI="$(pwd)/apps/desktop/ui"
step "ui lint (eslint)"
if (cd "$UI" && npm run lint); then pass "ui lint"; else fail "ui lint"; fi

step "ui tests (vitest)"
if (cd "$UI" && npx vitest run); then pass "ui tests"; else fail "ui tests"; fi

step "ui build (tsc + vite)"
if (cd "$UI" && npm run build); then pass "ui build"; else fail "ui build"; fi

# -- end-to-end --------------------------------------------------------
step "smoke test"
if bash scripts/smoke-test.sh; then pass "smoke"; else fail "smoke"; fi

# -- summary ------------------------------------------------------------
printf '\n'
if [ "${#FAILED[@]}" -eq 0 ]; then
  echo "ALL VERIFICATION PASSED ✓"
  exit 0
else
  echo "FAILED STEPS: ${FAILED[*]}"
  exit 1
fi
