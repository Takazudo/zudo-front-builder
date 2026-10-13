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
TMP_DIR=$(mktemp -d)
cleanup() { rm -rf "$TMP_DIR"; }
trap cleanup EXIT
mkdir "$TMP_DIR/bin"
GH_CALL_LOG="$TMP_DIR/gh-calls"
export GH_CALL_LOG
cat > "$TMP_DIR/bin/gh" <<'SHEOF'
#!/bin/sh
printf '%s\n' "$*" >> "$GH_CALL_LOG"
case "$1 $2" in
  'issue list')
    if [ "${GH_MODE:-}" = list-failure ]; then exit 1; fi
    if [ "${GH_MODE:-}" = comment-failure ]; then
      printf '[{"number":42,"title":"flaky: zfb-diagnostics::tests::zzz_temp_flaky_capture_for_fixture"}]\n'
    else
      printf '[{"number":99,"title":"flaky: zfb-diagnostics::tests::zzz_temp_flaky_capture_for_fixture extra"}]\n'
    fi
    ;;
  'issue create'|'issue comment') echo 'mock read-only token' >&2; exit 1 ;;
esac
SHEOF
chmod +x "$TMP_DIR/bin/gh"
PATH="$TMP_DIR/bin:$PATH"
export PATH

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

CLEAN_FIXTURE="$TMP_DIR/clean.xml"

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
  *"No flakes found in inspected reports (coverage=complete; valid-reports=1; testcases=1)."*)
    pass "clean JUnit report (no flaky testcases): correctly reports none found"
    ;;
  *)
    fail "clean JUnit report: expected qualified inspected-report summary, got: $OUTPUT_3"
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
  *"unavailable (missing)"*"coverage=not-evaluated"*"no usable JUnit evidence"*)
    pass "missing JUnit file: distinguishes unavailable evidence from zero flakes"
    ;;
  *)
    fail "missing JUnit file: expected not-evaluated evidence, got: $OUTPUT_4"
    ;;
esac

# These synthetic standard JUnit shapes test the evidence classifications;
# nextest-specific retry detection above still uses the captured report.
cat > "$TMP_DIR/nested.xml" <<'XMLEOF'
<testsuite name="outer" tests="4" failures="1" skipped="1">
  <testcase name="outer-pass" classname="sample"/>
  <testsuite name="inner" tests="3" failures="1" skipped="1">
    <testcase name="failure" classname="sample"><failure/></testcase>
    <testcase name="error" classname="sample"><error/></testcase>
    <testcase name="skip" classname="sample"><skipped/></testcase>
  </testsuite>
</testsuite>
XMLEOF
printf '<testsuite name="empty" tests="0"/>\n' > "$TMP_DIR/zero.xml"
printf '<testsuites/>\n' > "$TMP_DIR/empty-container.xml"
printf '<testsuite><testcase>\n' > "$TMP_DIR/malformed.xml"
printf '<?xml version="1.0" encoding="NO-SUCH-ENCODING"?><testsuite/>\n' > "$TMP_DIR/unknown-encoding.xml"
printf '<?xml version="1.0" encoding="UTF-32"?><testsuite/>\n' > "$TMP_DIR/unsupported-encoding.xml"
printf '<document><testsuite><testcase/></testsuite></document>\n' > "$TMP_DIR/non-junit.xml"
printf '<testsuites><testcase name="orphan"/></testsuites>\n' > "$TMP_DIR/invalid-structure.xml"
printf '<testsuite><unknown/></testsuite>\n' > "$TMP_DIR/unknown-structure.xml"
printf '<testsuite><testcase><flakyFailure/></testcase></testsuite>\n' > "$TMP_DIR/invalid-identity.xml"
printf '<testsuite><testcase name="sample" classname="c"><unknown/></testcase></testsuite>\n' > "$TMP_DIR/unknown-testcase.xml"
printf '<testsuite><testcase name="sample" classname="c"><wrapper><flakyFailure/></wrapper></testcase></testsuite>\n' > "$TMP_DIR/wrapped-retry.xml"
printf '<testsuite><testcase name="sample" classname="c"><wrapper><failure/></wrapper></testcase></testsuite>\n' > "$TMP_DIR/wrapped-failure.xml"
printf '<testsuite><testcase name="sample" classname="c"><wrapper><skipped/></wrapper></testcase></testsuite>\n' > "$TMP_DIR/wrapped-skipped.xml"
printf '<testsuite><testcase name="sample" classname="c"><system-out><flakyFailure/></system-out></testcase></testsuite>\n' > "$TMP_DIR/misplaced-retry.xml"
printf '<testsuite><testcase name="sample" classname="c"><failure><unknown/></failure></testcase></testsuite>\n' > "$TMP_DIR/unknown-outcome.xml"

run_dry() {
  if OUTPUT=$(DRY_RUN=1 "$SCRIPT" "$@" 2>&1); then STATUS=0; else STATUS=$?; fi
  if [ "$STATUS" -eq 0 ]; then pass "$CASE: best-effort exit 0"; else fail "$CASE: exit $STATUS: $OUTPUT"; fi
  case "$OUTPUT" in
    *'No flaky tests detected.'*) fail "$CASE: unqualified clean message" ;;
    *) pass "$CASE: no unqualified clean message" ;;
  esac
}
contains() {
  case "$OUTPUT" in
    *"$1"*) pass "$CASE: $2" ;;
    *) fail "$CASE: missing '$1': $OUTPUT" ;;
  esac
}
no_issues() {
  case "$OUTPUT" in
    *'gh '*|*'::warning::Flaky test detected'*) fail "$CASE: fake flaky issue or finding: $OUTPUT" ;;
    *) pass "$CASE: no fake flaky issues" ;;
  esac
}
not_clean() {
  case "$OUTPUT" in
    *'No flakes found'*|*'No flaky tests detected.'*) fail "$CASE: unavailable evidence reported as clean: $OUTPUT" ;;
    *) pass "$CASE: no clean finding from unavailable evidence" ;;
  esac
}

CASE='captured fixture counts'
OUTPUT=$OUTPUT_1
contains 'suites=1; testcases=15; failures=0; errors=0; skipped=0; flaky-tests=1' 'captured retry is separate from final failures'

CASE='clean evidence'
run_dry "$CLEAN_FIXTURE"
contains 'coverage=complete; valid-reports=1/1' 'complete inspected coverage'
no_issues

CASE='nested suites with failed and skipped cases'
run_dry "$TMP_DIR/nested.xml"
contains 'suites=2; testcases=4; failures=1; errors=1; skipped=1; flaky-tests=0' 'nested cases counted once, outcomes distinct'
contains 'Zero flakes does not establish that all tests passed.' 'does not claim a successful test run'
no_issues

for KIND in malformed unknown-encoding unsupported-encoding non-junit invalid-structure unknown-structure invalid-identity \
  unknown-testcase wrapped-retry wrapped-failure wrapped-skipped misplaced-retry unknown-outcome; do
  CASE="$KIND evidence"
  run_dry "$TMP_DIR/$KIND.xml"
  contains 'invalid evidence' 'explicit invalid diagnostic'
  contains 'coverage=not-evaluated; valid-reports=0/1' 'not evaluated'
  contains "$TMP_DIR/$KIND.xml" 'names invalid input'
  no_issues
  not_clean
done

for KIND in missing unreadable; do
  CASE="all $KIND"
  if [ "$KIND" = missing ]; then INPUT="$TMP_DIR/missing.xml"; else INPUT="$TMP_DIR"; fi
  run_dry "$INPUT"
  contains "unavailable ($KIND" 'explicit unavailable diagnostic'
  contains 'no usable JUnit evidence' 'not evaluated'
  contains "$INPUT" 'names unavailable input'
  no_issues
  not_clean
done

for KIND in zero empty-container; do
  CASE="$KIND report"
  run_dry "$TMP_DIR/$KIND.xml"
  contains 'zero-test evidence' 'explicit zero-test evidence'
  contains 'coverage=not-evaluated; valid-reports=1/1; zero-test-reports=1;' 'valid empty report counts'
  contains 'valid reports contain zero testcases' 'not evaluated'
  no_issues
  not_clean
done

for KIND in missing malformed unknown-encoding unsupported-encoding zero; do
  CASE="valid plus $KIND"
  run_dry "$CLEAN_FIXTURE" "$TMP_DIR/$KIND.xml"
  contains 'coverage=partial;' 'partial coverage'
  contains 'No flakes found in inspected reports (coverage=partial;' 'qualified partial finding'
  contains "$TMP_DIR/$KIND.xml" 'names evidence gap'
  no_issues
done

for KIND in missing malformed unknown-encoding unsupported-encoding wrapped-retry; do
  CASE="duplicate real flakes plus $KIND"
  run_dry "$FIXTURE" "$FIXTURE" "$TMP_DIR/$KIND.xml"
  contains 'coverage=partial; valid-reports=2/3' 'partial coverage preserves valid reports'
  contains 'testcases=30; failures=0; errors=0; skipped=0; flaky-tests=1' 'deduped identity across repeated reports'
  contains "Flaky test detected (pass-on-retry): $EXPECTED_TEST_ID" 'real flake retained'
  COUNT=$(printf '%s\n' "$OUTPUT" | grep -c 'would create issue' || true)
  if [ "$COUNT" -eq 1 ]; then pass "$CASE: filed once"; else fail "$CASE: $COUNT creates"; fi
  COUNT=$(printf '%s\n' "$OUTPUT" | grep -c '::warning::Flaky test detected' || true)
  if [ "$COUNT" -eq 1 ]; then pass "$CASE: warned once"; else fail "$CASE: $COUNT warnings"; fi
done

CASE='exact title dedup'
OUTPUT=$(DRY_RUN=1 FLAKY_EXISTING_OPEN_ISSUES="flaky: $EXPECTED_TEST_ID extra" "$SCRIPT" "$FIXTURE" 2>&1)
contains 'would create issue' 'similar title is not reused'

if [ ! -e "$GH_CALL_LOG" ]; then
  pass 'DRY_RUN is fully offline, including dedup and label calls'
else
  fail "DRY_RUN called gh: $(cat "$GH_CALL_LOG")"
fi

# A command seam injects an extraction crash without any production-only flag.
# Even partial stdout from the failed extractor must not become a fake issue.
mkdir "$TMP_DIR/parser-bin"
cat > "$TMP_DIR/parser-bin/python3" <<'SHEOF'
#!/bin/sh
echo 'fake::partial-extraction'
exit 123
SHEOF
chmod +x "$TMP_DIR/parser-bin/python3"
CASE='extractor failure'
if OUTPUT=$(PATH="$TMP_DIR/parser-bin:$PATH" DRY_RUN=1 "$SCRIPT" "$FIXTURE" 2>&1); then
  pass "$CASE: best-effort exit 0"
else
  fail "$CASE: unexpectedly failed"
fi
contains 'JUnit extraction unavailable or failed' 'failure cannot become clean evidence'
contains "$FIXTURE" 'names extraction input'
no_issues
not_clean

# These writes run only against the local stub, proving read-only-token behavior
# and live-path exact-title dedup without touching GitHub.
for MODE in create-failure comment-failure list-failure; do
  CASE="gh $MODE"
  : > "$GH_CALL_LOG"
  if OUTPUT=$(GH_MODE="$MODE" DRY_RUN=0 "$SCRIPT" "$FIXTURE" 2>&1); then
    pass "$CASE: best-effort exit 0"
  else
    fail "$CASE: unexpectedly failed"
  fi
  contains 'Flaky test detected (pass-on-retry)' 'real finding remains visible'
  case "$MODE" in
    create-failure)
      contains 'could not file flaky-test issue' 'write failure diagnostic'
      if grep -q '^issue create ' "$GH_CALL_LOG" && ! grep -q '^issue comment ' "$GH_CALL_LOG"; then
        pass "$CASE: near title not reused"
      else fail "$CASE: wrong write: $(cat "$GH_CALL_LOG")"; fi
      ;;
    comment-failure)
      contains 'could not comment on flaky-test issue #42' 'write failure diagnostic'
      if grep -q '^issue comment 42 ' "$GH_CALL_LOG" && ! grep -q '^issue create ' "$GH_CALL_LOG"; then
        pass "$CASE: exact open title reused"
      else fail "$CASE: wrong write: $(cat "$GH_CALL_LOG")"; fi
      ;;
    list-failure)
      contains 'could not check existing flaky-test issue' 'lookup failure diagnostic'
      if ! grep -Eq '^issue (create|comment) ' "$GH_CALL_LOG"; then
        pass "$CASE: no duplicate write after unavailable dedup"
      else fail "$CASE: unexpected write: $(cat "$GH_CALL_LOG")"; fi
      ;;
  esac
done

# ── Summary ──────────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
