#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/run-supervisor-watch.sh — offline tests for
# scripts/run-supervisor-watch.sh (the two-pass body of
# .github/workflows/supervisor-watch.yml, issue #2918).
#
# Runs entirely offline. The script under test is bash; this harness is POSIX
# sh because run-b4push.sh and health.yml execute every tests/unit/*.sh with
# `sh` (mirrors tests/unit/file-exam-issue.sh). Real `gh` is never invoked: a
# stub is passed through WATCH_GH -> the harvester's --gh flag, which is the
# same subprocess/argv contract that
# scripts/__tests__/harvest-supervisor-timelines.test.mjs drives.
#
# Fixtures are real emissions: the `[supervisor-timeline]` record shape and
# the `gh run view --job <id> --log` line prefix are copied from
# scripts/__tests__/supervisor-timeline-summary.test.mjs (UP_BOOM_LINE) and
# scripts/__tests__/harvest-supervisor-timelines.test.mjs (LOG_PREFIX), not
# hand-invented.
#
# Requires: sh, bash, node, jq, mktemp, grep, tail.
#
# Run:
#   sh tests/unit/run-supervisor-watch.sh

set -eu

# Run from the repo root regardless of the caller's cwd.
SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
cd "$REPO_ROOT"

SCRIPT="scripts/run-supervisor-watch.sh"

PASS=0
FAIL=0
pass() {
  printf 'PASS: %s\n' "$1"
  PASS=$((PASS + 1))
}
fail() {
  printf 'FAIL: %s\n' "$1"
  FAIL=$((FAIL + 1))
}

if [ ! -f "$SCRIPT" ]; then
  fail "script not found: $SCRIPT"
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

TMPROOT=$(mktemp -d)
trap 'rm -rf "$TMPROOT"' EXIT

# ── Stub `gh` ────────────────────────────────────────────────────────────────
#
# Dispatches on argv exactly like the real thing: `run list` (honouring
# --branch by filtering headBranch, so pass B really is a narrower
# population), `run view <id> --json jobs`, `run view --job <id> --log`.
# Fixture data comes from files under $WATCH_TEST_FIXTURES_DIR, inherited from
# the environment the same way a real `gh` would inherit it.
STUB="$TMPROOT/gh-stub.sh"
cat >"$STUB" <<'GH_STUB'
#!/bin/sh
set -eu
FIXDIR="${WATCH_TEST_FIXTURES_DIR:?WATCH_TEST_FIXTURES_DIR not set}"
printf '%s\n' "$*" >>"$FIXDIR/calls.log"

if [ "$1" = "run" ] && [ "$2" = "list" ]; then
  if [ -f "$FIXDIR/run-list.fail" ]; then
    echo "gh-stub: simulated run list failure" >&2
    exit 1
  fi
  branch=""
  while [ "$#" -gt 0 ]; do
    if [ "$1" = "--branch" ]; then branch="${2:-}"; fi
    shift
  done
  if [ -n "$branch" ]; then
    jq --arg b "$branch" '[.[] | select(.headBranch == $b)]' "$FIXDIR/run-list.json"
  else
    cat "$FIXDIR/run-list.json"
  fi
  exit 0
fi

if [ "$1" = "run" ] && [ "$2" = "view" ]; then
  if [ "$3" = "--job" ]; then
    if [ -f "$FIXDIR/job-$4.fail" ]; then
      echo "gh-stub: simulated failure fetching job $4" >&2
      exit 1
    fi
    cat "$FIXDIR/job-$4.log"
    exit 0
  fi
  cat "$FIXDIR/jobs-$3.json"
  exit 0
fi

echo "gh-stub: unhandled invocation: $*" >&2
exit 1
GH_STUB
chmod +x "$STUB"

# ── Fixture builders ─────────────────────────────────────────────────────────

# gh's `--log` output prefixes every line with "<job>\t<step>\t<timestamp> ",
# and `pnpm -r`'s reporter adds its own ". test: " package label ahead of the
# tag — both are part of the real shape the summarizer must see through.
LOG_PREFIX_FMT='health\tUNKNOWN STEP\t2026-09-06T23:06:25.83Z . test: '

ENV_A="sha256:ed62f5285936a0ca"
ENV_B="sha256:173aae2cffffffff"

# timeline_line <outcome> <first-up-line> <total> <env-digest>
timeline_line() {
  printf "$LOG_PREFIX_FMT"
  printf '[supervisor-timeline] case=up+boom outcome=%s total=%s runner=pnpm zudoDoc=5.15.0 runParallel=sha256:646f90cc300185cb fixtureShape=sha256:2d146d48587c00f5 env=%s supervisor-spawned=1 first-stdout-byte=354 first-up-line=%s marker-file-created=437 first-stderr-byte=505 supervisor-error-line=505 supervisor-closed=540 sibling-death=540\n' \
    "$1" "$3" "$4" "$2"
}

# job_log <outcome> <first-up-line> <total> <env-digest> > file
job_log() {
  printf "$LOG_PREFIX_FMT"
  printf '##[section]Starting: Run tests\n'
  timeline_line "$@"
  printf "$LOG_PREFIX_FMT"
  printf 'PASS scripts/__tests__/docs-dev-supervisor.test.mjs\n'
}

# job_log_no_records > file
job_log_no_records() {
  printf "$LOG_PREFIX_FMT"
  printf '##[section]Starting: Run tests\n'
  printf "$LOG_PREFIX_FMT"
  printf 'PASS some-other.test.mjs\n'
}

# run_json <databaseId> <headBranch>
run_json() {
  printf '{"databaseId":%s,"headBranch":"%s","headSha":"0123456789abcdef0123456789abcdef01234567","conclusion":"success","status":"completed","createdAt":"2026-09-07T00:00:00Z","event":"push","attempt":1}' \
    "$1" "$2"
}

# jobs_json <jobId> — the run's job list, with a `health` job that started
# (a non-empty `steps` array is what tells the harvester it has a log).
jobs_json() {
  printf '{"jobs":[{"name":"health","databaseId":%s,"conclusion":"success","steps":[{"name":"Set up job"}]}]}' "$1"
}

# new_fixture_dir <case-name> — fresh fixture + output tree per case.
new_fixture_dir() {
  FIX="$TMPROOT/$1"
  mkdir -p "$FIX"
  : >"$FIX/calls.log"
  printf '[]' >"$FIX/run-list.json"
  echo "$FIX"
}

# ── Runner ───────────────────────────────────────────────────────────────────
#
# Returns the script's exit code in RC and its last stdout line in VERDICT_LINE.
run_watch() {
  RW_FIX="$1"
  if WATCH_TEST_FIXTURES_DIR="$RW_FIX" \
    WATCH_GH="$STUB" \
    WATCH_OUT_DIR="$RW_FIX/out" \
    GITHUB_OUTPUT="$RW_FIX/gh-output.txt" \
    GITHUB_STEP_SUMMARY="$RW_FIX/gh-step-summary.md" \
    bash "$SCRIPT" >"$RW_FIX/stdout.txt" 2>"$RW_FIX/stderr.txt"; then
    RC=0
  else
    RC=$?
  fi
  VERDICT_LINE=$(tail -n 1 "$RW_FIX/stdout.txt")
}

# assert_case <desc> <fixture-dir> <expected-rc> <expected-verdict>
assert_case() {
  AC_DESC="$1"
  AC_FIX="$2"
  AC_RC="$3"
  AC_VERDICT="$4"

  run_watch "$AC_FIX"

  if [ "$RC" -eq "$AC_RC" ]; then
    pass "$AC_DESC: exit $AC_RC"
  else
    fail "$AC_DESC: want exit $AC_RC, got $RC (stderr: $(tail -n 3 "$AC_FIX/stderr.txt" | tr '\n' ' '))"
  fi

  case "$VERDICT_LINE" in
    "verdict=$AC_VERDICT "*)
      pass "$AC_DESC: verdict=$AC_VERDICT"
      ;;
    *)
      fail "$AC_DESC: want a 'verdict=$AC_VERDICT ...' last line, got '$VERDICT_LINE'"
      ;;
  esac

  if [ -f "$AC_FIX/out/all/summary.txt" ] && [ -f "$AC_FIX/out/main/summary.txt" ]; then
    pass "$AC_DESC: both summary.txt files written"
  else
    fail "$AC_DESC: expected out/all/summary.txt and out/main/summary.txt"
  fi
}

# ── green: two runs, one on main and one on a PR branch, identical identity ──

FIX=$(new_fixture_dir green)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1002 feat/x)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
jobs_json 5002 >"$FIX/jobs-1002.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5002.log"
assert_case 'green: healthy population on both branches' "$FIX" 0 green

if [ -f "$FIX/out/failed-runs.txt" ] && [ ! -s "$FIX/out/failed-runs.txt" ]; then
  pass 'green: failed-runs.txt written and empty'
else
  fail "green: expected an empty failed-runs.txt, got: $(cat "$FIX/out/failed-runs.txt" 2>&1)"
fi

# The workflow reads verdict/detail straight out of $GITHUB_OUTPUT and renders
# $GITHUB_STEP_SUMMARY, so both handoffs are asserted rather than assumed.
if grep -q '^verdict=green$' "$FIX/gh-output.txt" && grep -q '^detail=all=ok:0/0 main=ok:0/0$' "$FIX/gh-output.txt"; then
  pass 'green: $GITHUB_OUTPUT carries verdict and detail'
else
  fail "green: unexpected \$GITHUB_OUTPUT: $(cat "$FIX/gh-output.txt")"
fi

if grep -q '^## Supervisor watch — green$' "$FIX/gh-step-summary.md"; then
  pass 'green: $GITHUB_STEP_SUMMARY carries the verdict heading'
else
  fail "green: \$GITHUB_STEP_SUMMARY missing the verdict heading"
fi

# ── no-data: zero enumerated runs is neutral, not red ────────────────────────

FIX=$(new_fixture_dir no-data)
printf '[]' >"$FIX/run-list.json"
assert_case 'no-data: gh run list returns no runs' "$FIX" 1 no-data

# ── red R-A: an outcome=failed record anywhere, including a PR branch ────────

FIX=$(new_fixture_dir red-r-a)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1002 feat/x)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
jobs_json 5002 >"$FIX/jobs-1002.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"
job_log failed 430 540 "$ENV_A" >"$FIX/job-5002.log"
assert_case 'red R-A: outcome=failed on a PR branch' "$FIX" 2 red

# failed-runs.txt is asserted for existence only: pointing it at the specific
# run needs the per-run `failed=<m>` manifest count a sibling sub-issue is
# adding to the harvester. Today's harvester only emits a `failed=` count on
# its final summary line (a PARTIAL harvest), which the partial-harvest case
# below exercises.
if [ -f "$FIX/out/failed-runs.txt" ]; then
  pass 'red R-A: failed-runs.txt written'
else
  fail 'red R-A: expected out/failed-runs.txt'
fi

# ── red R-B: max pre-UP reaches 0.75 x the 10s budget ────────────────────────

FIX=$(new_fixture_dir red-r-b)
printf '[%s]' "$(run_json 1001 main)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
job_log ok 7600 7900 "$ENV_A" >"$FIX/job-5001.log"
assert_case 'red R-B: pre-UP 7600ms >= 7500ms boundary' "$FIX" 2 red

# ── red infra: `gh run list` itself fails (harvester exit 64) ────────────────

FIX=$(new_fixture_dir red-infra)
printf '[%s]' "$(run_json 1001 main)" >"$FIX/run-list.json"
: >"$FIX/run-list.fail"
assert_case 'red infra: gh run list fails' "$FIX" 2 red

# ── red telemetry-vanished: runs enumerated, zero records ────────────────────

FIX=$(new_fixture_dir red-vanished)
printf '[%s]' "$(run_json 1001 main)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
job_log_no_records >"$FIX/job-5001.log"
assert_case 'red telemetry-vanished: runs>0 but no records' "$FIX" 2 red

# ── red partial harvest: a job log that cannot be fetched ────────────────────

FIX=$(new_fixture_dir red-partial)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1002 feat/x)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
jobs_json 5002 >"$FIX/jobs-1002.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"
: >"$FIX/job-5002.fail"
assert_case 'red partial: one job log unfetchable (harvester exit 3)' "$FIX" 2 red

if grep -q 'failed=1' "$FIX/out/failed-runs.txt"; then
  pass 'red partial: failed-runs.txt captured the failed= manifest line'
else
  fail "red partial: expected a failed= line, got: $(cat "$FIX/out/failed-runs.txt")"
fi

# ── identity drift confined to a PR branch is green ──────────────────────────
#
# Pass A allow-lists every identity field (the all-branch population is mixed
# by design); pass B sees main alone, which stays single-valued.

FIX=$(new_fixture_dir drift-pr-only)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1002 feat/x)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
jobs_json 5002 >"$FIX/jobs-1002.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"
job_log ok 430 540 "$ENV_B" >"$FIX/job-5002.log"
assert_case 'green: env drift confined to a PR branch' "$FIX" 0 green

# ── identity drift on main is red ────────────────────────────────────────────

FIX=$(new_fixture_dir drift-main)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1003 main)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
jobs_json 5003 >"$FIX/jobs-1003.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"
job_log ok 430 540 "$ENV_B" >"$FIX/job-5003.log"
assert_case 'red: env drift between two main runs' "$FIX" 2 red

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
if [ "$FAIL" -gt 0 ]; then exit 1; fi
