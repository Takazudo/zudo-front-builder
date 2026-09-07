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
# Requires: sh, bash, node, mktemp, grep, tail, wc. Unlike its siblings this
# is not sub-second: every case spawns the real harvester and summarizer.
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
# Dispatches on argv exactly like the real thing: `run list`, `run view <id>
# --json jobs`, `run view --job <id> --log`. Fixture data comes from files
# under $WATCH_TEST_FIXTURES_DIR, inherited from the environment the same way
# a real `gh` would inherit it. The watch never passes --branch (pass B is
# derived from pass A's saved logs), so the stub does not filter on it.
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
  cat "$FIXDIR/run-list.json"
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

# run_json <databaseId> <headBranch> [status] [event]
run_json() {
  printf '{"databaseId":%s,"headBranch":"%s","headSha":"0123456789abcdef0123456789abcdef01234567","conclusion":"success","status":"%s","createdAt":"2026-09-07T00:00:00Z","event":"%s","attempt":1}' \
    "$1" "$2" "${3:-completed}" "${4:-push}"
}

# jobs_json <jobId> [conclusion] — the run's job list, with a `health` job
# that started (a non-empty `steps` array is what tells the harvester it has
# a log). The conclusion decides whether a record-less log means the emitter
# went silent (`success`) or the job went red before the test step.
jobs_json() {
  printf '{"jobs":[{"name":"health","databaseId":%s,"conclusion":"%s","steps":[{"name":"Set up job"}]}]}' \
    "$1" "${2:-success}"
}

# new_fixture_dir <case-name> — fresh fixture + output tree per case.
new_fixture_dir() {
  NFD_DIR="$TMPROOT/$1"
  mkdir -p "$NFD_DIR"
  : >"$NFD_DIR/calls.log"
  printf '[]' >"$NFD_DIR/run-list.json"
  echo "$NFD_DIR"
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

# assert_detail <desc> <fixture-dir> <expected-detail>
assert_detail() {
  if grep -q "^detail=$3\$" "$2/gh-output.txt"; then
    pass "$1: detail=$3"
  else
    fail "$1: want detail=$3, got: $(grep '^detail=' "$2/gh-output.txt")"
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
if grep -q '^verdict=green$' "$FIX/gh-output.txt"; then
  pass 'green: $GITHUB_OUTPUT carries the verdict'
else
  fail "green: unexpected \$GITHUB_OUTPUT: $(cat "$FIX/gh-output.txt")"
fi
assert_detail green "$FIX" 'all=ok:0/0 main=ok:1/0'

if grep -q '^## Supervisor watch — green$' "$FIX/gh-step-summary.md"; then
  pass 'green: $GITHUB_STEP_SUMMARY carries the verdict heading'
else
  fail "green: \$GITHUB_STEP_SUMMARY missing the verdict heading"
fi

# Pass A's --save-dir is the artifact an R-A triage actually reads AND pass
# B's input; a regression that dropped the flag would still go green, so
# assert it.
if [ -f "$FIX/out/job-logs/run-1001-job-5001.log" ] && [ -f "$FIX/out/job-logs/run-1002-job-5002.log" ]; then
  pass 'green: pass A saved both health job logs under job-logs/'
else
  fail "green: expected saved job logs, got: $(ls "$FIX/out/job-logs" 2>&1 | tr '\n' ' ')"
fi

# Pass B is the trunk subset of that harvest, selected by the manifest's
# branch token — one run here, never the PR run.
if [ "$(wc -l <"$FIX/out/main/runs.txt" | tr -d ' ')" -eq 1 ] && grep -q '^run=1001 ' "$FIX/out/main/runs.txt"; then
  pass 'green: pass B selected exactly the main run'
else
  fail "green: expected main/runs.txt to name run 1001 only, got: $(cat "$FIX/out/main/runs.txt")"
fi

# No second network harvest: each job log is fetched exactly once.
if [ "$(grep -c 'run view --job 5001 --log' "$FIX/calls.log")" -eq 1 ]; then
  pass 'green: the main job log was fetched once, not once per pass'
else
  fail "green: expected one fetch of job 5001, got: $(grep -c 'run view --job 5001 --log' "$FIX/calls.log")"
fi

if grep -q '^window: since=' "$FIX/out/harvest-notices.txt"; then
  pass 'green: harvest-notices.txt carries the effective window'
else
  fail "green: expected a window: line in harvest-notices.txt, got: $(cat "$FIX/out/harvest-notices.txt")"
fi

# ── no-data: zero enumerated runs is neutral, not red ────────────────────────

FIX=$(new_fixture_dir no-data)
printf '[]' >"$FIX/run-list.json"
assert_case 'no-data: gh run list returns no runs' "$FIX" 1 no-data
assert_detail no-data "$FIX" 'all=empty:4/1 main=empty:0/-'

# ── no-data: every enumerated run skipped (still in progress at cron time) ───

FIX=$(new_fixture_dir no-data-skipped)
printf '[%s]' "$(run_json 1001 main in_progress)" >"$FIX/run-list.json"
assert_case 'no-data: the only run is still in progress' "$FIX" 1 no-data

# ── green: main went red before the test step, a PR run is healthy ───────────
#
# A record-less log from a failed health job is not vanished telemetry; the
# harvester skips it, so pass B has nothing to judge and stays `empty`.

FIX=$(new_fixture_dir main-red-before-tests)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1002 feat/x)" >"$FIX/run-list.json"
jobs_json 5001 failure >"$FIX/jobs-1001.json"
jobs_json 5002 >"$FIX/jobs-1002.json"
job_log_no_records >"$FIX/job-5001.log"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5002.log"
assert_case 'green: main health red before the test step, PR healthy' "$FIX" 0 green
assert_detail 'green (main red before tests)' "$FIX" 'all=ok:0/0 main=empty:0/-'

# ── red R-A: an outcome=failed record anywhere, including a PR branch ────────

FIX=$(new_fixture_dir red-r-a)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1002 feat/x)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
jobs_json 5002 >"$FIX/jobs-1002.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"
job_log failed 430 540 "$ENV_A" >"$FIX/job-5002.log"
assert_case 'red R-A: outcome=failed on a PR branch' "$FIX" 2 red

# failed-runs.txt is the R-A provenance: it must name the run holding the
# diagnostic block, exactly once.
if [ "$(wc -l <"$FIX/out/failed-runs.txt" | tr -d ' ')" -eq 1 ] && grep -q '^run=1002 .* job=5002 lines=1 failed=1' "$FIX/out/failed-runs.txt"; then
  pass 'red R-A: failed-runs.txt names run 1002 once'
else
  fail "red R-A: expected one 'run=1002 ... failed=1' line, got: $(cat "$FIX/out/failed-runs.txt")"
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
assert_detail 'red infra' "$FIX" 'all=red:64/1 main=empty:0/-'

# ── red telemetry-vanished: a green health job with zero records ─────────────

FIX=$(new_fixture_dir red-vanished)
printf '[%s]' "$(run_json 1001 main)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
job_log_no_records >"$FIX/job-5001.log"
assert_case 'red telemetry-vanished: green health job, no records' "$FIX" 2 red
# The record-less green run IS harvested (that is the finding), so pass B
# sees it too and names the silence on main.
assert_detail 'red telemetry-vanished' "$FIX" 'all=red:1/1 main=silent:1/1'

# ── red silent-on-main: one green main job with no records beside an emitting one
#
# The harvester's no-records exit is population-level, so this population is
# exit 0 and pass A is ok; pass B must still catch the single silent trunk job.

FIX=$(new_fixture_dir red-silent-main)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1003 main)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
jobs_json 5003 >"$FIX/jobs-1003.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"
job_log_no_records >"$FIX/job-5003.log"
assert_case 'red silent-on-main: one green main job with zero records' "$FIX" 2 red
assert_detail 'red silent-on-main' "$FIX" 'all=ok:0/0 main=silent:2/0'

if grep -q '^run=1003 .* lines=0 ' "$FIX/out/silent-runs.txt"; then
  pass 'red silent-on-main: silent-runs.txt names run 1003'
else
  fail "red silent-on-main: expected run 1003 in silent-runs.txt, got: $(cat "$FIX/out/silent-runs.txt")"
fi

# ── green: a silent PR-branch job is listed but not judged ───────────────────

FIX=$(new_fixture_dir green-silent-pr)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1002 feat/pre-emitter)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
jobs_json 5002 >"$FIX/jobs-1002.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"
job_log_no_records >"$FIX/job-5002.log"
assert_case 'green: silent green job on a PR branch only' "$FIX" 0 green
assert_detail 'green (silent PR job)' "$FIX" 'all=ok:0/0 main=ok:1/0'

if grep -q '^run=1002 .* lines=0 ' "$FIX/out/silent-runs.txt"; then
  pass 'green (silent PR job): silent-runs.txt still lists run 1002'
else
  fail "green (silent PR job): expected run 1002 in silent-runs.txt, got: $(cat "$FIX/out/silent-runs.txt")"
fi

# ── green: a fork PR whose head branch is named main is not trunk ───────────
#
# health.yml's only trunk trigger is push; pass B must key on the event too,
# or a contributor's fork `main` (different identity) reddens the strict pass.

FIX=$(new_fixture_dir fork-main-pr)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1004 main completed pull_request)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
jobs_json 5004 >"$FIX/jobs-1004.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"
job_log ok 430 540 "$ENV_B" >"$FIX/job-5004.log"
assert_case 'green: pull_request run from a fork branch named main is not trunk' "$FIX" 0 green
assert_detail 'green (fork main PR)' "$FIX" 'all=ok:0/0 main=ok:1/0'

# ── red partial harvest: a job log that cannot be fetched ────────────────────

FIX=$(new_fixture_dir red-partial)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1002 feat/x)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
jobs_json 5002 >"$FIX/jobs-1002.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"
: >"$FIX/job-5002.fail"
assert_case 'red partial: one job log unfetchable (harvester exit 3)' "$FIX" 2 red

# A fetch failure is a harvest problem, not an R-A record: it belongs in
# harvest-errors.txt and must not masquerade as a failed supervisor run.
if [ ! -s "$FIX/out/failed-runs.txt" ] && grep -q '^run=1002 .* error=' "$FIX/out/harvest-errors.txt"; then
  pass 'red partial: the unfetchable run is in harvest-errors.txt, not failed-runs.txt'
else
  fail "red partial: failed-runs=$(cat "$FIX/out/failed-runs.txt") harvest-errors=$(cat "$FIX/out/harvest-errors.txt")"
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
assert_detail 'red (drift on main)' "$FIX" 'all=ok:0/0 main=red:2/2'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
if [ "$FAIL" -gt 0 ]; then exit 1; fi
