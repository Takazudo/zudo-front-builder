#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/report-flaky-tests.sh — offline tests for the JUnit-parsing +
# dedup logic in scripts/report-flaky-tests.sh (issue #1341).
#
# Runs entirely offline against a REAL captured nextest JUnit fixture
# (tests/fixtures/flaky-junit-example.xml). Per the zudo-test-wisdom rule that
# fixtures for third-party tool output must be CAPTURED, not hand-authored,
# this fixture was produced by intentionally running a test designed to fail
# once then pass (a file-marker gated panic) under
# `cargo nextest run --profile ci -p zfb-diagnostics`, then deleting the
# artificial test after capturing its JUnit output. This is what proves the
# parser actually understands nextest's real <flakyFailure> element nesting,
# not a guess at the format.
#
# DRY_RUN=1 makes the script print the exact `gh issue create`/`gh issue
# comment` command it would run instead of executing it, so no network is
# needed here. The dedup ("does an open issue already exist?") check is
# itself mocked via FLAKY_EXISTING_OPEN_ISSUES rather than a live
# `gh issue list` call — see scripts/report-flaky-tests.sh.
#
# Run:
#   sh tests/unit/report-flaky-tests.sh

set -eu

ROOT_DIR=$(cd "$(dirname "$0")/../.." && pwd)
SCRIPT="$ROOT_DIR/scripts/report-flaky-tests.sh"
FIXTURE="$ROOT_DIR/tests/fixtures/flaky-junit-example.xml"

PASS=0
FAIL=0

pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

if [ ! -x "$SCRIPT" ]; then
  fail "script missing or not executable: $SCRIPT"
fi

if [ ! -f "$FIXTURE" ]; then
  fail "fixture missing: $FIXTURE"
fi

if [ "$FAIL" -gt 0 ]; then
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

EXPECTED_TEST_ID="zfb-diagnostics::tests::zzz_temp_flaky_capture_for_fixture"

# ── First run: no pre-existing issue → should create exactly one ───────────

OUTPUT_1=$(DRY_RUN=1 GITHUB_RUN_URL="https://example.invalid/run/1" "$SCRIPT" "$FIXTURE" 2>&1)

case "$OUTPUT_1" in
  *"::warning::Flaky test detected"*"$EXPECTED_TEST_ID"*)
    pass "first run: ::warning:: annotation emitted for $EXPECTED_TEST_ID"
    ;;
  *)
    fail "first run: expected a ::warning:: annotation mentioning $EXPECTED_TEST_ID"
    ;;
esac

CREATE_COUNT_1=$(printf '%s\n' "$OUTPUT_1" | grep -c 'would create issue' || true)
COMMENT_COUNT_1=$(printf '%s\n' "$OUTPUT_1" | grep -c 'would comment on existing issue' || true)

if [ "$CREATE_COUNT_1" -eq 1 ] && [ "$COMMENT_COUNT_1" -eq 0 ]; then
  pass "first run: exactly one 'would create' command, no comment"
else
  fail "first run: expected 1 create / 0 comment, got $CREATE_COUNT_1 create / $COMMENT_COUNT_1 comment"
fi

case "$OUTPUT_1" in
  *"flaky: $EXPECTED_TEST_ID"*)
    pass "first run: correct test-id extracted into the issue title"
    ;;
  *)
    fail "first run: expected issue title 'flaky: $EXPECTED_TEST_ID' in output"
    ;;
esac

# The fixture also has 14 non-flaky passing testcases (no <flakyFailure>
# child) — none of them must be reported.
NON_FLAKY_LEAK=$(printf '%s\n' "$OUTPUT_1" | grep -c 'render_framed_clamps_window_at_file_start' || true)
if [ "$NON_FLAKY_LEAK" -eq 0 ]; then
  pass "first run: non-flaky testcases in the fixture are correctly ignored"
else
  fail "first run: a non-flaky testcase leaked into the flaky report"
fi

# ── Second run: existing OPEN issue with this exact title → dedup to a
# comment, not a second create. FLAKY_EXISTING_OPEN_ISSUES simulates the
# issue the first run above would have filed — offline, no `gh issue list`
# network call involved. ─────────────────────────────────────────────────

OUTPUT_2=$(DRY_RUN=1 GITHUB_RUN_URL="https://example.invalid/run/2" \
  FLAKY_EXISTING_OPEN_ISSUES="flaky: $EXPECTED_TEST_ID" \
  "$SCRIPT" "$FIXTURE" 2>&1)

CREATE_COUNT_2=$(printf '%s\n' "$OUTPUT_2" | grep -c 'would create issue' || true)
COMMENT_COUNT_2=$(printf '%s\n' "$OUTPUT_2" | grep -c 'would comment on existing issue' || true)

if [ "$CREATE_COUNT_2" -eq 0 ] && [ "$COMMENT_COUNT_2" -eq 1 ]; then
  pass "second run: dedups to exactly one comment, no duplicate create"
else
  fail "second run: expected 0 create / 1 comment, got $CREATE_COUNT_2 create / $COMMENT_COUNT_2 comment"
fi

# ── A JUnit report with no flaky testcases must produce no gh command ──────
# (This is a small synthetic snippet, not a captured fixture: the property
# under test — "a plain <testcase> with no <flakyFailure> child is not
# flaky" — does not depend on any nextest-specific quirk, so it does not need
# to come from a real run. The real-format contract for flaky detection
# itself is proven above against the captured fixture.)

CLEAN_FIXTURE=$(mktemp)
cleanup() { rm -f "$CLEAN_FIXTURE"; }
trap cleanup EXIT

cat > "$CLEAN_FIXTURE" <<'XMLEOF'
<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="zfb-nextest-ci" tests="1" failures="0" errors="0">
    <testsuite name="zfb-diagnostics" tests="1" disabled="0" errors="0" failures="0">
        <testcase name="tests::some_passing_test" classname="zfb-diagnostics" time="0.01">
        </testcase>
    </testsuite>
</testsuites>
XMLEOF

OUTPUT_3=$(DRY_RUN=1 "$SCRIPT" "$CLEAN_FIXTURE" 2>&1)

case "$OUTPUT_3" in
  *"No flaky tests detected."*)
    pass "clean JUnit report (no flaky testcases): correctly reports none found"
    ;;
  *)
    fail "clean JUnit report: expected 'No flaky tests detected.', got: $OUTPUT_3"
    ;;
esac

# ── Missing JUnit file must not crash the script (best-effort, CI must not
# break on this step) ───────────────────────────────────────────────────────

OUTPUT_4=$(DRY_RUN=1 "$SCRIPT" "$ROOT_DIR/tests/fixtures/does-not-exist.xml" 2>&1)
EXIT_4=$?

if [ "$EXIT_4" -eq 0 ]; then
  pass "missing JUnit file: script exits 0 (never breaks the CI run)"
else
  fail "missing JUnit file: expected exit 0, got $EXIT_4"
fi

case "$OUTPUT_4" in
  *"No flaky tests detected."*)
    pass "missing JUnit file: reports no flaky tests instead of erroring"
    ;;
  *)
    fail "missing JUnit file: expected 'No flaky tests detected.', got: $OUTPUT_4"
    ;;
esac

# ── Summary ──────────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
