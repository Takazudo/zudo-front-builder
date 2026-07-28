#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/check-exam-ignore-parity.sh — offline tests for
# scripts/check-exam-ignore-parity.sh (issue #2072).
#
# Runs entirely offline against a small synthetic fixture
# (tests/fixtures/exam-ignore-parity/), never against a real cargo/nextest
# invocation. The fixture is hand-authored (not captured, unlike
# report-flaky-tests.sh's JUnit fixture) because it exercises the guard
# script's OWN parsing logic against deliberately small inputs, not a
# third-party tool's output format.
#
# The fixture is built to demonstrate all FOUR possible outcomes per the
# review-amended guard contract on issue #2072:
#   - covered_by_exam_test   (crates/fixture-pkg/tests/alpha_integration.rs)
#     -> COVERED-BY-EXAM: its exact name is in the fixture exam.yml filterset.
#   - covered_by_health_test (crates/fixture-pkg/tests/beta_integration.rs)
#     -> COVERED-BY-HEALTH-SCOPE via an explicit `--test beta_integration`
#        health.yml step scope.
#   - lib_covered_test       (crates/fixture-pkg/src/mod_a/mod_b.rs)
#     -> COVERED-BY-HEALTH-SCOPE via a `--lib mod_a::mod_b::` module-prefix
#        health.yml step scope (the lib-test counterpart to the case above).
#   - verification_helper_test (crates/fixture-pkg/tests/gamma_integration.rs)
#     -> EXCEPTION(verification): exempted by tag alone, not by test name.
#   - uncovered_test         (crates/fixture-pkg/tests/delta_integration.rs)
#     -> UNCOVERED: absent from exam.yml AND from any health.yml step whose
#        *scope* actually reaches it. The fixture health.yml also carries a
#        DECOY step named to mention "delta" whose `run:` line actually
#        scopes beta_integration again — proving the guard performs real
#        scope parsing rather than a loose name-similarity grep (a grep
#        against step *names* would wrongly call this "covered").
#
# Run:
#   sh tests/unit/check-exam-ignore-parity.sh

set -eu

ROOT_DIR=$(cd "$(dirname "$0")/../.." && pwd)
SCRIPT="$ROOT_DIR/scripts/check-exam-ignore-parity.sh"
FIXTURE_DIR="$ROOT_DIR/tests/fixtures/exam-ignore-parity"

PASS=0
FAIL=0

pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

if [ ! -x "$SCRIPT" ]; then
  fail "script missing or not executable: $SCRIPT"
fi

if [ ! -d "$FIXTURE_DIR" ]; then
  fail "fixture dir missing: $FIXTURE_DIR"
fi

if [ "$FAIL" -gt 0 ]; then
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

# ── Run 1: synthetic fixture — must demonstrate all four outcomes and exit
# non-zero because it deliberately contains one uncovered env-gate test ─────

FIXTURE_OUTPUT=$(
  CHECK_EXAM_IGNORE_PARITY_CRATES_ROOT="$FIXTURE_DIR/crates" \
  CHECK_EXAM_IGNORE_PARITY_EXAM_WORKFLOW="$FIXTURE_DIR/exam.yml" \
  CHECK_EXAM_IGNORE_PARITY_HEALTH_WORKFLOW="$FIXTURE_DIR/health.yml" \
  "$SCRIPT" 2>&1
) && FIXTURE_RC=0 || FIXTURE_RC=$?

if [ "$FIXTURE_RC" -ne 0 ]; then
  pass "fixture run: exits non-zero (found the deliberately uncovered test)"
else
  fail "fixture run: expected non-zero exit, got 0"
fi

# Outcome 1: covered-by-exam
case "$FIXTURE_OUTPUT" in
  *"COVERED-BY-EXAM: covered_by_exam_test "*)
    pass "fixture run: covered_by_exam_test reported COVERED-BY-EXAM"
    ;;
  *)
    fail "fixture run: expected COVERED-BY-EXAM for covered_by_exam_test"
    ;;
esac

# Outcome 2: covered-by-health-scope, via an explicit --test binary scope
case "$FIXTURE_OUTPUT" in
  *"COVERED-BY-HEALTH-SCOPE: covered_by_health_test "*)
    pass "fixture run: covered_by_health_test reported COVERED-BY-HEALTH-SCOPE (--test scope)"
    ;;
  *)
    fail "fixture run: expected COVERED-BY-HEALTH-SCOPE for covered_by_health_test"
    ;;
esac

# Outcome 2b: covered-by-health-scope, via a --lib module-prefix scope (the
# lib-test counterpart — proves the guard parses BOTH scope shapes health.yml
# actually uses, not just the integration-test one).
case "$FIXTURE_OUTPUT" in
  *"COVERED-BY-HEALTH-SCOPE: mod_a::mod_b::tests::lib_covered_test "*)
    pass "fixture run: lib_covered_test reported COVERED-BY-HEALTH-SCOPE (--lib module-prefix scope)"
    ;;
  *)
    fail "fixture run: expected COVERED-BY-HEALTH-SCOPE for mod_a::mod_b::tests::lib_covered_test"
    ;;
esac

# Outcome 3: legitimate exception by tag
case "$FIXTURE_OUTPUT" in
  *"EXCEPTION(verification): verification_helper_test "*)
    pass "fixture run: verification_helper_test reported EXCEPTION(verification)"
    ;;
  *)
    fail "fixture run: expected EXCEPTION(verification) for verification_helper_test"
    ;;
esac

# Outcome 4: uncovered — and NOT fooled by the decoy step whose name mentions
# "delta" but whose run: line scopes a different binary.
case "$FIXTURE_OUTPUT" in
  *"UNCOVERED: uncovered_test "*)
    pass "fixture run: uncovered_test reported UNCOVERED (decoy step did not fool the scope parser)"
    ;;
  *)
    fail "fixture run: expected UNCOVERED for uncovered_test"
    ;;
esac

case "$FIXTURE_OUTPUT" in
  *"5 tests checked: 1 covered-by-exam, 2 covered-by-health-scope, 1 legitimate exceptions, 1 UNCOVERED"*)
    pass "fixture run: summary line matches the expected 1/2/1/1 split"
    ;;
  *)
    fail "fixture run: unexpected summary line: $FIXTURE_OUTPUT"
    ;;
esac

# ── Run 2: the real repo — must exit 0 (the fresh reconciliation done for
# issue #2072 found zero drift: every env-gate:/heavy:-tagged #[ignore]d
# test in crates/ is reachable by exam.yml or health.yml). This is the
# guard's actual job, not just a fixture demonstration. ─────────────────────

REAL_OUTPUT=$("$SCRIPT" 2>&1) && REAL_RC=0 || REAL_RC=$?

if [ "$REAL_RC" -eq 0 ]; then
  pass "real-repo run: exits 0 (no drift between crates/ and exam.yml/health.yml)"
else
  fail "real-repo run: expected exit 0, got $REAL_RC. Output:
$REAL_OUTPUT"
fi

case "$REAL_OUTPUT" in
  # "UNCOVERED:" (with the colon) only appears as a per-test line prefix —
  # the summary line's own "0 UNCOVERED" has no trailing colon, so this
  # cannot false-positive on a clean summary the way a bare "UNCOVERED"
  # substring check would.
  *"UNCOVERED:"*)
    fail "real-repo run: found at least one UNCOVERED test — see output above"
    ;;
  *)
    pass "real-repo run: no UNCOVERED lines in output"
    ;;
esac

# ── Summary ──────────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
