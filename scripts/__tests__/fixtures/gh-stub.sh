#!/bin/sh
# scripts/__tests__/fixtures/gh-stub.sh
#
# Shared `gh` stub for the harvester and supervisor-watch suites (issue #2930).
# Dispatches on argv
# exactly like the real thing -- `run list`, and (since #2931) the two REST
# calls `gh api repos/{owner}/{repo}/actions/runs/<id>/jobs?per_page=100` and
# `gh api repos/{owner}/{repo}/actions/jobs/<id>/logs` -- and reads its
# fixture data from files under $GH_STUB_FIXTURES_DIR, inherited from the
# environment the same way a real `gh` invocation would inherit the shell's
# environment. Written in POSIX sh (no bashisms) so both harnesses can exec
# it directly: the vitest harness spawns it via node's execFile, the sh
# harness via bash's `command -v`-style lookup -- neither cares which shell
# wrote it, only that the shebang runs.
#
# The `{owner}`/`{repo}` placeholders are left unexpanded on purpose: the
# real gh resolves them locally, so a stubbed call never needs a repo and the
# fixtures stay independent of which checkout runs them.
#
# Fixture files, all optional except run-list.json:
#   run-list.json           stdout for `gh run list`
#   run-list.fail           if present, `gh run list` fails every time
#   run-list.fail-once      if present, the FIRST `gh run list` call fails
#                           and this marker is consumed (removed), so a
#                           caller's single retry then sees the real fixture
#   jobs-<runId>.json       stdout for the run-jobs endpoint, page 1
#   jobs-<runId>-p<n>.json  stdout for page <n> (n >= 2) of that endpoint
#   job-<jobId>.log         stdout for the job-logs endpoint
#   job-<jobId>.fail        if present, that job's log fetch fails every time
#   job-<jobId>.fail-once   if present, the FIRST fetch of that job's log
#                           fails and this marker is consumed (removed)
#   job-<jobId>.notfound    if present, that job's log fetch reproduces gh's
#                           real 404 shape: the API error body on stdout, a
#                           "gh: Not Found (HTTP 404)" line on stderr, exit 1
#                           (captured from `gh api` on 2026-09-07). That is
#                           what a missing or expired job log looks like.
#
# Every invocation is appended to $GH_STUB_FIXTURES_DIR/calls.log, one line
# of "$*" per call, so callers can assert on the exact argv shape and call
# count.
#
# An invocation this stub does not recognise fails loudly (exit 1, naming
# the invocation on stderr) instead of returning a friendly default --
# silence here would let a real call-shape change look like it passed while
# exercising nothing. That is not hypothetical: it is exactly what caught
# #2931's migration from `gh run view` to these endpoints.

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

if [ "$1" = "api" ]; then
  path="$2"
  case "$path" in
    */actions/jobs/*/logs)
      jobId=${path#*/actions/jobs/}
      jobId=${jobId%/logs}
      if [ -f "$FIXDIR/job-$jobId.fail" ]; then
        echo "gh-stub: simulated failure fetching job $jobId" >&2
        exit 1
      fi
      if [ -f "$FIXDIR/job-$jobId.fail-once" ]; then
        rm "$FIXDIR/job-$jobId.fail-once"
        echo "gh-stub: simulated transient failure fetching job $jobId" >&2
        exit 1
      fi
      if [ -f "$FIXDIR/job-$jobId.notfound" ]; then
        printf '{"message":"Not Found","documentation_url":"https://docs.github.com/rest/actions/workflow-jobs#download-job-logs-for-a-workflow-run","status":"404"}'
        echo "gh: Not Found (HTTP 404)" >&2
        exit 1
      fi
      cat "$FIXDIR/job-$jobId.log"
      exit 0
      ;;
    */actions/runs/*/jobs\?*)
      runId=${path#*/actions/runs/}
      runId=${runId%%/jobs\?*}
      query=${path#*/jobs\?}
      # Page 1 carries only `per_page=`; matching on the "&page=" separator
      # rather than on a bare "page=" keeps `per_page` from reading as one.
      case "$query" in
        *"&page="*)
          page=${query##*&page=}
          page=${page%%&*}
          ;;
        *) page=1 ;;
      esac
      if [ "$page" = "1" ]; then
        cat "$FIXDIR/jobs-$runId.json"
      else
        cat "$FIXDIR/jobs-$runId-p$page.json"
      fi
      exit 0
      ;;
  esac
fi

echo "gh-stub: unhandled invocation: $*" >&2
exit 1
