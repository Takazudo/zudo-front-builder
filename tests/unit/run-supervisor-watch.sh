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
# stub -- scripts/__tests__/fixtures/gh-stub.sh, SHARED with
# scripts/__tests__/harvest-supervisor-timelines.test.mjs (#2930) -- is
# passed through WATCH_GH -> the harvester's --gh flag, which is the same
# subprocess/argv contract that the vitest suite drives.
#
# Fixtures are real emissions, from two corpora with two different owners
# (#2930's addendum): the `[supervisor-timeline]` record shape is zfb's own
# contract and comes from scripts/__tests__/fixtures/supervisor-timeline-samples.txt,
# while the job-log envelope around it is GitHub's and is sliced out of a real
# captured REST body, scripts/__tests__/fixtures/rest-job-log-capture.log
# (#2931). Both are read via grep/sed rather than duplicated as literals here.
#
# Requires: sh, bash, node, mktemp, grep, sed, tail, wc. Unlike its siblings
# this is not sub-second: every case spawns the real harvester and summarizer.
#
# Run:
#   sh tests/unit/run-supervisor-watch.sh

set -eu

# Run from the repo root regardless of the caller's cwd.
SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
cd "$REPO_ROOT"

SCRIPT="scripts/run-supervisor-watch.sh"
WATCH_IMPL=${WATCH_IMPL:-sh}

case "$WATCH_IMPL" in
  sh) WATCH_COMMAND="bash $SCRIPT" ;;
  mjs)
    SCRIPT="scripts/supervisor-watch.mjs"
    WATCH_COMMAND="node $SCRIPT"
    ;;
  *)
    printf 'unknown WATCH_IMPL: %s (expected sh or mjs)\n' "$WATCH_IMPL" >&2
    exit 1
    ;;
esac

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
# The shared stub (scripts/__tests__/fixtures/gh-stub.sh, #2930) dispatches
# on argv exactly like the real thing: `run list`, and since #2931 the two
# REST calls `api repos/{owner}/{repo}/actions/runs/<id>/jobs?per_page=100`
# and `api repos/{owner}/{repo}/actions/jobs/<id>/logs`. Fixture data comes
# from files under
# $GH_STUB_FIXTURES_DIR, inherited from the environment the same way a real
# `gh` would inherit it. The watch never passes --branch (pass B is derived
# from pass A's saved logs), so the stub does not filter on it.
STUB="$REPO_ROOT/scripts/__tests__/fixtures/gh-stub.sh"

# ── Fixture builders ─────────────────────────────────────────────────────────

# The REST job-log body prefixes every line with the runner's own
# "<ISO timestamp>Z ", and `pnpm -r`'s reporter adds its ". test: " package
# label ahead of the tag — both are part of the real shape the summarizer
# must see through, and neither is invented here: the prefix is sliced off a
# record line in the captured body (see its loader for provenance), so these
# synthetic logs are wrapped in bytes GitHub actually emitted.
# -a: the capture carries raw ANSI escapes and non-ASCII glyphs, and under a
# C/POSIX locale (CI's default) grep would otherwise call it binary and print
# "Binary file matches" instead of the line.
CAPTURE_FILE="$REPO_ROOT/scripts/__tests__/fixtures/rest-job-log-capture.log"
LOG_PREFIX=$(grep -a -m1 '\[supervisor-timeline\] case=' "$CAPTURE_FILE" |
  sed 's/\[supervisor-timeline\].*//')
if [ -z "$LOG_PREFIX" ]; then
  fail "could not slice the REST log prefix out of $CAPTURE_FILE"
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

ENV_A="sha256:ed62f5285936a0ca"
ENV_B="sha256:173aae2cffffffff"

# The canonical up+boom sample (#2930): read once via grep/sed rather than
# duplicated as a literal here, so a field rename in the corpus needs no
# edit in this file. This case's own scenarios still need varied
# outcome/total/env/first-up-line values (the scope note in #2930 -- one
# corpus, not one construction site), so timeline_line() edits those four
# tokens into the template instead of hand-writing a new record per case.
SAMPLES_FILE="$REPO_ROOT/scripts/__tests__/fixtures/supervisor-timeline-samples.txt"
UP_BOOM_TEMPLATE=$(grep '^UP_BOOM_LINE=' "$SAMPLES_FILE" | sed 's/^UP_BOOM_LINE=//')

# timeline_line <outcome> <first-up-line> <total> <env-digest>
timeline_line() {
  printf '%s' "$LOG_PREFIX"
  printf '%s\n' "$UP_BOOM_TEMPLATE" | sed -E \
    -e "s/outcome=[^ ]+/outcome=$1/" \
    -e "s/first-up-line=[^ ]+/first-up-line=$2/" \
    -e "s/total=[^ ]+/total=$3/" \
    -e "s/env=[^ ]+/env=$4/"
}

# job_log <outcome> <first-up-line> <total> <env-digest> > file
job_log() {
  printf '%s' "$LOG_PREFIX"
  printf '##[section]Starting: Run tests\n'
  timeline_line "$@"
  printf '%s' "$LOG_PREFIX"
  printf 'PASS scripts/__tests__/docs-dev-supervisor.test.mjs\n'
}

# job_log_no_records > file
job_log_no_records() {
  printf '%s' "$LOG_PREFIX"
  printf '##[section]Starting: Run tests\n'
  printf '%s' "$LOG_PREFIX"
  printf 'PASS some-other.test.mjs\n'
}

# run_json <databaseId> <headBranch> [status] [event]
run_json() {
  printf '{"databaseId":%s,"headBranch":"%s","headSha":"0123456789abcdef0123456789abcdef01234567","conclusion":"success","status":"%s","createdAt":"2026-09-07T00:00:00Z","event":"%s","attempt":1}' \
    "$1" "$2" "${3:-completed}" "${4:-push}"
}

# jobs_json <jobId> [conclusion] — one page of the run-jobs endpoint, with a
# `health` job that started (a non-empty `steps` array is what tells the
# harvester it has a log). REST names the numeric id `id` (only `gh run list
# --json` calls it `databaseId`) and wraps the page in `total_count`. The
# conclusion decides whether a record-less log means the emitter went silent
# (`success`) or the job went red before the test step.
jobs_json() {
  printf '{"total_count":1,"jobs":[{"name":"health","id":%s,"conclusion":"%s","steps":[{"name":"Set up job"}]}]}' \
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
# run_watch <fixture-dir> [watch-tar] — the optional second arg feeds
# WATCH_TAR (#2934); defaults to the real `tar` on PATH, same as the script's
# own default, so every existing caller is unaffected. The archive-failure
# case below passes `false` to simulate a broken archive with no dedicated
# stub script.
#
# Returns the script's exit code in RC and its last stdout line in VERDICT_LINE.
run_watch() {
  RW_FIX="$1"
  RW_TAR="${2:-tar}"
  if GH_STUB_FIXTURES_DIR="$RW_FIX" \
    WATCH_GH="$STUB" \
    WATCH_OUT_DIR="$RW_FIX/out" \
    WATCH_RETRY_DELAY_MS=0 \
    WATCH_TAR="$RW_TAR" \
    GITHUB_OUTPUT="$RW_FIX/gh-output.txt" \
    GITHUB_STEP_SUMMARY="$RW_FIX/gh-step-summary.md" \
    $WATCH_COMMAND >"$RW_FIX/stdout.txt" 2>"$RW_FIX/stderr.txt"; then
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
# assert it. The script folds job-logs/ into job-logs.tar.gz and removes the
# plain directory once both passes are done reading it (#2934), so the
# saved logs are asserted inside the archive, and the plain directory's
# absence is asserted separately -- if it survived, upload-artifact would
# ship both and the artifact would grow instead of shrink.
if [ -f "$FIX/out/job-logs.tar.gz" ] \
  && tar -tzf "$FIX/out/job-logs.tar.gz" | grep -qx 'job-logs/run-1001-job-5001.log' \
  && tar -tzf "$FIX/out/job-logs.tar.gz" | grep -qx 'job-logs/run-1002-job-5002.log'; then
  pass 'green: pass A saved both health job logs under job-logs/'
else
  fail "green: expected job-logs.tar.gz containing both saved job logs, got: $(tar -tzf "$FIX/out/job-logs.tar.gz" 2>&1 | tr '\n' ' ')"
fi

if [ ! -e "$FIX/out/job-logs" ]; then
  pass 'green: the plain job-logs/ directory was removed after archiving'
else
  fail "green: expected job-logs/ to be removed after archiving, got: $(ls "$FIX/out/job-logs" | tr '\n' ' ')"
fi

# Pass B is the trunk subset of that harvest, selected by the manifest's
# branch token — one run here, never the PR run.
if [ "$(wc -l <"$FIX/out/main/runs.txt" | tr -d ' ')" -eq 1 ] && grep -q '^run=1001 ' "$FIX/out/main/runs.txt"; then
  pass 'green: pass B selected exactly the main run'
else
  fail "green: expected main/runs.txt to name run 1001 only, got: $(cat "$FIX/out/main/runs.txt")"
fi

# No second network harvest: each job log is fetched exactly once.
JOB_5001_LOG_CALL='api repos/{owner}/{repo}/actions/jobs/5001/logs'
if [ "$(grep -cF "$JOB_5001_LOG_CALL" "$FIX/calls.log")" -eq 1 ]; then
  pass 'green: the main job log was fetched once, not once per pass'
else
  fail "green: expected one fetch of job 5001, got: $(grep -cF "$JOB_5001_LOG_CALL" "$FIX/calls.log")"
fi

# Two metered GETs per run and no `gh run view` porcelain — the whole point
# of #2931, and invisible in every verdict above.
if [ "$(grep -c '^api ' "$FIX/calls.log")" -eq 4 ] && ! grep -q '^run view ' "$FIX/calls.log"; then
  pass 'green: two REST calls per run, no gh run view porcelain'
else
  fail "green: unexpected gh calls: $(tr '\n' '; ' <"$FIX/calls.log")"
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
if [ "$(wc -l <"$FIX/out/failed-runs.txt" | tr -d ' ')" -eq 1 ] && grep -q '^run=1002 .* job=5002 lines=1 failedRecords=1' "$FIX/out/failed-runs.txt"; then
  pass 'red R-A: failed-runs.txt names run 1002 once'
else
  fail "red R-A: expected one 'run=1002 ... failedRecords=1' line, got: $(cat "$FIX/out/failed-runs.txt")"
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

# ── archive failure: leaves job-logs/ intact and reports no verdict ─────────
#
# WATCH_TAR=false simulates a broken archive with no dedicated stub script
# (`false` ignores its argv and exits 1). The archiving block in
# run-supervisor-watch.sh must kill the whole script BEFORE $GITHUB_OUTPUT
# (and its `verdict=` line) is written -- a broken archive must never be
# reported as a quiet green week -- and it must never delete the source logs
# it failed to fold into an archive. It sits AFTER the provenance greps and
# the $GITHUB_STEP_SUMMARY render, though, so the R-A triage output a filed
# tracking issue points at still exists.

FIX=$(new_fixture_dir archive-failure)
printf '[%s,%s]' "$(run_json 1001 main)" "$(run_json 1002 feat/x)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
jobs_json 5002 >"$FIX/jobs-1002.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5002.log"
run_watch "$FIX" false

if [ "$RC" -ne 0 ]; then
  pass 'archive failure: the script exits non-zero'
else
  fail 'archive failure: expected a non-zero exit, got 0'
fi

if [ -z "$VERDICT_LINE" ]; then
  pass 'archive failure: no verdict line reached stdout'
else
  fail "archive failure: expected no stdout verdict line, got: $VERDICT_LINE"
fi

if ! grep -q '^verdict=' "$FIX/gh-output.txt" 2>/dev/null; then
  pass 'archive failure: $GITHUB_OUTPUT carries no verdict (never written)'
else
  fail "archive failure: unexpected \$GITHUB_OUTPUT: $(cat "$FIX/gh-output.txt")"
fi

if [ -f "$FIX/out/job-logs/run-1001-job-5001.log" ] && [ -f "$FIX/out/job-logs/run-1002-job-5002.log" ]; then
  pass 'archive failure: the source job logs are left intact'
else
  fail "archive failure: expected both source job logs to survive, got: $(ls "$FIX/out/job-logs" 2>&1 | tr '\n' ' ')"
fi

if [ ! -e "$FIX/out/job-logs.tar.gz" ]; then
  pass 'archive failure: no partial job-logs.tar.gz was left behind'
else
  fail 'archive failure: expected no job-logs.tar.gz to exist after a failed archive'
fi

# The provenance files and the job summary are the only thing the filed
# tracking issue's text points a reader at, so a broken archive must not take
# them with it -- which is why the archiving block runs after them.
if [ -f "$FIX/out/failed-runs.txt" ] && [ -f "$FIX/out/harvest-notices.txt" ] \
  && grep -q '^## Supervisor watch — ' "$FIX/gh-step-summary.md"; then
  pass 'archive failure: the provenance files and job summary survive'
else
  fail 'archive failure: expected the provenance files and $GITHUB_STEP_SUMMARY to be written before archiving'
fi

# ── archive failure, partial output: the truncated archive is removed ───────
#
# `false` never opens the output file, so it cannot show what a REAL tar
# failure leaves behind: `tar -czf` truncates its target the moment it
# starts, so a mid-write death (disk full on a 200-run harvest -- exactly
# when job-logs/ is largest) leaves a corrupt job-logs.tar.gz beside the
# intact job-logs/. upload-artifact's `if: always()` would then ship BOTH,
# which is the "artifact grows instead of shrinking" outcome the whole
# archiving step exists to avoid, with a corrupt file where triage expects
# the real one. This stub reproduces that shape.

FIX=$(new_fixture_dir archive-partial)
printf '[%s]' "$(run_json 1001 main)" >"$FIX/run-list.json"
jobs_json 5001 >"$FIX/jobs-1001.json"
job_log ok 430 540 "$ENV_A" >"$FIX/job-5001.log"

PARTIAL_TAR="$FIX/partial-tar.sh"
cat >"$PARTIAL_TAR" <<'PARTIAL_TAR_EOF'
#!/bin/sh
# Mimics `tar -czf <archive> ...` dying after it has already truncated and
# partly written its output: create the file, write garbage, fail.
while [ "$#" -gt 0 ]; do
  case "$1" in
    -czf)
      printf 'not a real gzip stream' >"$2"
      exit 1
      ;;
  esac
  shift
done
exit 1
PARTIAL_TAR_EOF
chmod +x "$PARTIAL_TAR"

run_watch "$FIX" "$PARTIAL_TAR"

if [ "$RC" -ne 0 ] && [ -z "$VERDICT_LINE" ] \
  && ! grep -q '^verdict=' "$FIX/gh-output.txt" 2>/dev/null; then
  pass 'archive partial: non-zero exit and no verdict'
else
  fail "archive partial: want a non-zero exit and no verdict, got rc=$RC verdict='$VERDICT_LINE'"
fi

if [ ! -e "$FIX/out/job-logs.tar.gz" ]; then
  pass 'archive partial: the truncated archive was removed'
else
  fail 'archive partial: expected the truncated job-logs.tar.gz to be removed'
fi

if [ -f "$FIX/out/job-logs/run-1001-job-5001.log" ]; then
  pass 'archive partial: the source job log is left intact'
else
  fail 'archive partial: expected the source job log to survive'
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
if [ "$FAIL" -gt 0 ]; then exit 1; fi
