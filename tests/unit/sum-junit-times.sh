#!/bin/sh
#
# Offline test for scripts/sum-junit-times.sh (issue #2716).
#
# The checked-in fixture is a captured nextest report. Its 15 testcase times
# total 0.258s, and its nested retry failure adds 0.013s, so the expected
# retry-inclusive wall time is 0.271s.

set -eu

ROOT_DIR=$(cd "$(dirname "$0")/../.." && pwd)
SCRIPT="$ROOT_DIR/scripts/sum-junit-times.sh"
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

ACTUAL=$("$SCRIPT" "$FIXTURE")
if [ "$ACTUAL" = "0.271000" ]; then
    pass "fixture includes testcase time and nested flakyFailure retry time"
else
    fail "expected retry-inclusive fixture wall time 0.271000s, got $ACTUAL"
fi

# Exercise more than one retry marker on one testcase; each marker contributes
# to the total rather than only the first nested flakyFailure.
MULTI_FIXTURE=$(mktemp)
cleanup() { rm -f "$MULTI_FIXTURE"; }
trap cleanup EXIT
cat > "$MULTI_FIXTURE" <<'XMLEOF'
<?xml version="1.0" encoding="UTF-8"?>
<testsuites>
  <testsuite name="example">
    <testcase classname="example" name="retrying" time="1.25">
      <flakyFailure time="0.25" />
      <flakyFailure time="0.50" />
    </testcase>
    <testcase classname="example" name="passing" time="0.50" />
  </testsuite>
</testsuites>
XMLEOF

ACTUAL_MULTI=$("$SCRIPT" "$MULTI_FIXTURE")
if [ "$ACTUAL_MULTI" = "2.500000" ]; then
    pass "all nested flakyFailure retry times are included"
else
    fail "expected all-retry total 2.500000s, got $ACTUAL_MULTI"
fi

# The reader accepts more than one report, as C5 may collect one report per
# binary/configuration. A second fixture doubles both testcase and retry time.
ACTUAL_TWO=$("$SCRIPT" "$FIXTURE" "$FIXTURE")
if [ "$ACTUAL_TWO" = "0.542000" ]; then
    pass "multiple JUnit reports are summed"
else
    fail "expected two-report total 0.542000s, got $ACTUAL_TWO"
fi

if "$SCRIPT" "$ROOT_DIR/tests/fixtures/does-not-exist.xml" >/dev/null 2>&1; then
    fail "missing JUnit report must fail instead of reporting zero"
else
    pass "missing JUnit report fails loudly"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
