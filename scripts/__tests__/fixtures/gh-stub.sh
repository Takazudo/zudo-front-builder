#!/bin/sh
# scripts/__tests__/fixtures/gh-stub.sh
#
# Shared `gh` stub for scripts/__tests__/harvest-supervisor-timelines.test.mjs
# and tests/unit/run-supervisor-watch.sh (issue #2930). Dispatches on argv
# exactly like the real thing -- `run list`, `run view <id> --json jobs`,
# `run view --job <id> --log` -- and reads its fixture data from files under
# $GH_STUB_FIXTURES_DIR, inherited from the environment the same way a real
# `gh` invocation would inherit the shell's environment. Written in POSIX sh
# (no bashisms) so both harnesses can exec it directly: the vitest harness
# spawns it via node's execFile, the sh harness via bash's `command -v`-style
# lookup -- neither cares which shell wrote it, only that the shebang runs.
#
# Fixture files, all optional except run-list.json:
#   run-list.json           stdout for `gh run list`
#   run-list.fail           if present, `gh run list` fails every time
#   run-list.fail-once      if present, the FIRST `gh run list` call fails
#                           and this marker is consumed (removed), so a
#                           caller's single retry then sees the real fixture
#   jobs-<runId>.json       stdout for `gh run view <runId> --json jobs`
#   job-<jobId>.log         stdout for `gh run view --job <jobId> --log`
#   job-<jobId>.fail        if present, that job's log fetch fails every time
#   job-<jobId>.fail-once   if present, the FIRST fetch of that job's log
#                           fails and this marker is consumed (removed)
#
# Every invocation is appended to $GH_STUB_FIXTURES_DIR/calls.log, one line
# of "$*" per call, so callers can assert on the exact argv shape and call
# count.
#
# An invocation this stub does not recognise fails loudly (exit 1, naming
# the invocation on stderr) instead of returning a friendly default --
# silence here would let a real call-shape change (e.g. a future migration
# to `gh api ...`) look like it passed while exercising nothing.

set -eu

FIXDIR="${GH_STUB_FIXTURES_DIR:?GH_STUB_FIXTURES_DIR not set}"
printf '%s\n' "$*" >>"$FIXDIR/calls.log"

if [ "$1" = "run" ] && [ "$2" = "list" ]; then
  if [ -f "$FIXDIR/run-list.fail" ]; then
    echo "gh-stub: simulated run list failure" >&2
    exit 1
  fi
  if [ -f "$FIXDIR/run-list.fail-once" ]; then
    rm "$FIXDIR/run-list.fail-once"
    echo "gh-stub: simulated transient run list failure" >&2
    exit 1
  fi
  cat "$FIXDIR/run-list.json"
  exit 0
fi

if [ "$1" = "run" ] && [ "$2" = "view" ]; then
  if [ "$3" = "--job" ]; then
    jobId="$4"
    if [ -f "$FIXDIR/job-$jobId.fail" ]; then
      echo "gh-stub: simulated failure fetching job $jobId" >&2
      exit 1
    fi
    if [ -f "$FIXDIR/job-$jobId.fail-once" ]; then
      rm "$FIXDIR/job-$jobId.fail-once"
      echo "gh-stub: simulated transient failure fetching job $jobId" >&2
      exit 1
    fi
    cat "$FIXDIR/job-$jobId.log"
    exit 0
  fi
  runId="$3"
  cat "$FIXDIR/jobs-$runId.json"
  exit 0
fi

echo "gh-stub: unhandled invocation: $*" >&2
exit 1
