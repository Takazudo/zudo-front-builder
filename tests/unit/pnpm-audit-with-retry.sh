#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/pnpm-audit-with-retry.sh — offline unit tests for
# scripts/pnpm-audit-with-retry.sh (the fail-closed retry/classification
# wrapper around `pnpm audit --prod --audit-level=high`, issue #2074).
#
# Runs entirely offline: `scripts/pnpm-audit-with-retry.sh` is bash; this
# harness is POSIX sh (health.yml + run-b4push.sh invoke `sh tests/unit/*.sh`),
# so the script under test is exercised as a subprocess via `bash "$SCRIPT"`
# (mirrors tests/unit/file-exam-issue.sh's integration-test style) with a
# fake `pnpm` shim placed first on PATH — no real network involved, and
# PNPM_AUDIT_RETRY_DELAYS="0 0 0" makes every scenario run in well under a
# second instead of the real 30s/2m/5m backoff.
#
# The fake shim's error text for the "infra failure" scenarios
# ([ERR_PNPM_AUDIT_BAD_RESPONSE] ...) is not a guess: it mirrors the exact
# code + message shape pnpm's own audit test suite asserts
# (pnpm/pnpm's pnpm11/deps/compliance/audit/test/index.ts) and the real-world
# npm-registry-retirement incident (pnpm/pnpm#11265, 2026-04) — see
# scripts/pnpm-audit-with-retry.sh's header comment for the full citation.
#
# Requires: sh, bash, awk, grep, mktemp.
#
# Run:
#   sh tests/unit/pnpm-audit-with-retry.sh

set -eu

# Run from the repo root regardless of the caller's cwd.
SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
cd "$REPO_ROOT"

SCRIPT="scripts/pnpm-audit-with-retry.sh"

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

if [ ! -f "$SCRIPT" ]; then
  fail "script not found: $SCRIPT"
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

# ── Static pins on the retry contract (issue #2074's review-amended
# contract: 4 attempts / 3 delays, 30s/2m/5m default) ───────────────────────
# These are cheap grep pins, not executions — actually sleeping the real
# defaults would make this "offline unit test" take 7.5 minutes.

if grep -q 'MAX_ATTEMPTS=4' "$SCRIPT"; then
  pass "contract: 4 attempts (MAX_ATTEMPTS=4)"
else
  fail "contract: expected MAX_ATTEMPTS=4 in $SCRIPT"
fi

if grep -q 'DELAYS=(30 120 300)' "$SCRIPT"; then
  pass "contract: default delays are 30s / 2m / 5m"
else
  fail "contract: expected default DELAYS=(30 120 300) in $SCRIPT"
fi

# ── Fake `pnpm` shim ─────────────────────────────────────────────────────────
#
# Only handles `pnpm audit ...` (the one invocation
# scripts/pnpm-audit-with-retry.sh makes). Behavior is selected by
# MOCK_PNPM_MODE; each call is recorded to MOCK_PNPM_CALL_LOG (one line per
# call) so tests can assert exactly how many attempts were made.

MOCK_BIN=$(mktemp -d)
trap 'rm -rf "$MOCK_BIN"' EXIT

cat >"$MOCK_BIN/pnpm" <<'MOCK_PNPM'
#!/bin/sh
if [ "${1:-}" != "audit" ]; then
  exit 0
fi
LOG="${MOCK_PNPM_CALL_LOG:-/dev/null}"
printf 'call\n' >>"$LOG"
CALL_NUM=$(awk 'END{print NR}' "$LOG")

case "${MOCK_PNPM_MODE:-}" in
  finding)
    # A genuine --audit-level=high finding. Deliberately carries no
    # ERR_PNPM_AUDIT_BAD_RESPONSE-shaped text anywhere.
    cat <<'EOF'
┌─────────────────────┬────────────────────────────────────┐
│ high                │ Prototype Pollution                 │
├─────────────────────┼────────────────────────────────────┤
│ Package              │ example-vulnerable-pkg              │
└─────────────────────┴────────────────────────────────────┘
1 vulnerabilities found
Severity: 1 high
EOF
    exit 1
    ;;
  unrecognized-error)
    # An unrecognized non-zero outcome that is NOT the captured infra
    # signature and NOT a finding-shaped table — proves the fail-closed
    # default (never guess "probably infrastructure" for the unknown).
    echo "ECONNRESET: socket hang up while contacting registry.npmjs.org" >&2
    exit 1
    ;;
  infra-always)
    echo '[ERR_PNPM_AUDIT_BAD_RESPONSE] The audit endpoint (at https://registry.npmjs.org/-/npm/v1/security/advisories/bulk) responded with 410: {"error":"This endpoint is being retired. Use the bulk advisory endpoint instead."}'
    exit 1
    ;;
  infra-then-success)
    SUCCEED_AT="${MOCK_PNPM_SUCCEED_AT:-3}"
    if [ "$CALL_NUM" -ge "$SUCCEED_AT" ]; then
      echo "No known vulnerabilities found"
      exit 0
    fi
    echo '[ERR_PNPM_AUDIT_BAD_RESPONSE] The audit endpoint (at https://registry.npmjs.org/-/npm/v1/security/advisories/bulk) responded with 500: {"message":"Something bad happened"}'
    exit 1
    ;;
  *)
    echo "test setup error: unset MOCK_PNPM_MODE" >&2
    exit 99
    ;;
esac
MOCK_PNPM
chmod +x "$MOCK_BIN/pnpm"

# Prepend the mock bin dir so `pnpm` resolves to the stub for the rest of
# this script; real bash/awk/etc. stay on PATH.
PATH="$MOCK_BIN:$PATH"
export PATH

call_count() {
  # awk 'END{print NR}' on a possibly-nonexistent/empty log both correctly
  # print 0, unlike `wc -l` (BSD wc pads its output and errors on a missing
  # file without a redirect guard).
  [ -f "$1" ] || { echo 0; return; }
  awk 'END{print NR}' "$1"
}

# ── Scenario 1: a real finding → immediate fail, no retry attempted ─────────

LOG=$(mktemp)
rm -f "$LOG"
set +e
OUTPUT_1=$(MOCK_PNPM_MODE=finding MOCK_PNPM_CALL_LOG="$LOG" PNPM_AUDIT_RETRY_DELAYS="0 0 0" \
  bash "$SCRIPT" 2>&1)
RC_1=$?
set -e

if [ "$RC_1" -ne 0 ]; then
  pass "finding: wrapper exits non-zero"
else
  fail "finding: expected non-zero exit, got 0"
fi

if [ "$(call_count "$LOG")" -eq 1 ]; then
  pass "finding: pnpm audit invoked exactly once (no retry)"
else
  fail "finding: expected exactly 1 pnpm invocation, got $(call_count "$LOG")"
fi

case "$OUTPUT_1" in
  *"failing immediately, no retry"*) pass "finding: immediate-fail message emitted" ;;
  *) fail "finding: expected an immediate-fail message, got: $OUTPUT_1" ;;
esac

case "$OUTPUT_1" in
  *"retrying in"*) fail "finding: should never mention retrying" ;;
  *) pass "finding: no retry language present" ;;
esac

# ── Scenario 1b: an unrecognized (non-finding-shaped) error also fails
# immediately — the fail-closed default must not special-case only
# finding-shaped output. ──────────────────────────────────────────────────

LOG=$(mktemp)
rm -f "$LOG"
set +e
OUTPUT_1B=$(MOCK_PNPM_MODE=unrecognized-error MOCK_PNPM_CALL_LOG="$LOG" PNPM_AUDIT_RETRY_DELAYS="0 0 0" \
  bash "$SCRIPT" 2>&1)
RC_1B=$?
set -e

if [ "$RC_1B" -ne 0 ]; then
  pass "unrecognized error: wrapper exits non-zero"
else
  fail "unrecognized error: expected non-zero exit, got 0"
fi

if [ "$(call_count "$LOG")" -eq 1 ]; then
  pass "unrecognized error: pnpm audit invoked exactly once (no retry)"
else
  fail "unrecognized error: expected exactly 1 pnpm invocation, got $(call_count "$LOG")"
fi

case "$OUTPUT_1B" in
  *"retrying in"*|*"ERR_PNPM_AUDIT_BAD_RESPONSE"*)
    fail "unrecognized error: must not be classified as infra (no retry language, no signature match), got: $OUTPUT_1B"
    ;;
  *)
    pass "unrecognized error: correctly stayed unclassified as infra"
    ;;
esac

# ── Scenario 2: recognized infra failure on every attempt → backoff
# observed, then exit 0 + ::warning:: annotation on exhaustion. ────────────

LOG=$(mktemp)
rm -f "$LOG"
set +e
OUTPUT_2=$(MOCK_PNPM_MODE=infra-always MOCK_PNPM_CALL_LOG="$LOG" PNPM_AUDIT_RETRY_DELAYS="0 0 0" \
  bash "$SCRIPT" 2>&1)
RC_2=$?
set -e

if [ "$RC_2" -eq 0 ]; then
  pass "infra exhaustion: wrapper exits 0 (non-blocking)"
else
  fail "infra exhaustion: expected exit 0, got $RC_2"
fi

if [ "$(call_count "$LOG")" -eq 4 ]; then
  pass "infra exhaustion: pnpm audit invoked exactly 4 times (all attempts used)"
else
  fail "infra exhaustion: expected exactly 4 pnpm invocations, got $(call_count "$LOG")"
fi

RETRY_LINES=$(printf '%s\n' "$OUTPUT_2" | grep -c 'retrying in 0s' || true)
if [ "$RETRY_LINES" -eq 3 ]; then
  pass "infra exhaustion: backoff sequence observed (3 retry announcements)"
else
  fail "infra exhaustion: expected 3 retry announcements, got $RETRY_LINES"
fi

case "$OUTPUT_2" in
  *"::warning::pnpm audit signal missing"*"ERR_PNPM_AUDIT_BAD_RESPONSE"*)
    pass "infra exhaustion: loud ::warning:: annotation emitted, naming the signature"
    ;;
  *)
    fail "infra exhaustion: expected a ::warning:: annotation naming ERR_PNPM_AUDIT_BAD_RESPONSE, got: $OUTPUT_2"
    ;;
esac

# Every attempt's full captured output must be printed, not just the last —
# the classification boundary has to be auditable after the fact.
FULL_OUTPUT_OCCURRENCES=$(printf '%s\n' "$OUTPUT_2" | grep -c 'responded with 410' || true)
if [ "$FULL_OUTPUT_OCCURRENCES" -eq 4 ]; then
  pass "infra exhaustion: complete captured output printed on all 4 attempts"
else
  fail "infra exhaustion: expected the captured output on all 4 attempts (4 occurrences), got $FULL_OUTPUT_OCCURRENCES"
fi

# ── Scenario 3: recognized infra failure, then success on a later attempt
# → success, with the earlier retries recorded. ─────────────────────────────

LOG=$(mktemp)
rm -f "$LOG"
set +e
OUTPUT_3=$(MOCK_PNPM_MODE=infra-then-success MOCK_PNPM_SUCCEED_AT=3 MOCK_PNPM_CALL_LOG="$LOG" \
  PNPM_AUDIT_RETRY_DELAYS="0 0 0" bash "$SCRIPT" 2>&1)
RC_3=$?
set -e

if [ "$RC_3" -eq 0 ]; then
  pass "recovers before exhaustion: wrapper exits 0"
else
  fail "recovers before exhaustion: expected exit 0, got $RC_3"
fi

if [ "$(call_count "$LOG")" -eq 3 ]; then
  pass "recovers before exhaustion: pnpm audit invoked exactly 3 times (stopped once it passed)"
else
  fail "recovers before exhaustion: expected exactly 3 pnpm invocations, got $(call_count "$LOG")"
fi

case "$OUTPUT_3" in
  *"passed on attempt 3/4"*) pass "recovers before exhaustion: success recorded on attempt 3/4" ;;
  *) fail "recovers before exhaustion: expected 'passed on attempt 3/4', got: $OUTPUT_3" ;;
esac

RETRY_LINES_3=$(printf '%s\n' "$OUTPUT_3" | grep -c 'retrying in 0s' || true)
if [ "$RETRY_LINES_3" -eq 2 ]; then
  pass "recovers before exhaustion: the 2 earlier retries are recorded"
else
  fail "recovers before exhaustion: expected 2 retry announcements, got $RETRY_LINES_3"
fi

case "$OUTPUT_3" in
  *"::warning::"*) fail "recovers before exhaustion: should not emit the exhaustion ::warning::" ;;
  *) pass "recovers before exhaustion: no exhaustion ::warning:: emitted" ;;
esac

# ── Env-var override validation: a malformed PNPM_AUDIT_RETRY_DELAYS must
# fail loudly before ever invoking pnpm. ────────────────────────────────────

LOG=$(mktemp)
rm -f "$LOG"
set +e
OUTPUT_4=$(MOCK_PNPM_MODE=infra-always MOCK_PNPM_CALL_LOG="$LOG" PNPM_AUDIT_RETRY_DELAYS="5 5" \
  bash "$SCRIPT" 2>&1)
RC_4=$?
set -e

if [ "$RC_4" -eq 2 ]; then
  pass "malformed override: wrapper exits 2"
else
  fail "malformed override: expected exit 2, got $RC_4"
fi

if [ "$(call_count "$LOG")" -eq 0 ]; then
  pass "malformed override: pnpm audit never invoked"
else
  fail "malformed override: expected 0 pnpm invocations, got $(call_count "$LOG")"
fi

case "$OUTPUT_4" in
  *"::error::PNPM_AUDIT_RETRY_DELAYS must specify exactly 3 delays"*)
    pass "malformed override: ::error:: annotation explains the requirement"
    ;;
  *)
    fail "malformed override: expected an ::error:: annotation, got: $OUTPUT_4"
    ;;
esac

# ── Summary ───────────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
