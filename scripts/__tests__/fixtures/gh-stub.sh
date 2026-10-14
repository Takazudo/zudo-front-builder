#!/bin/sh
# scripts/__tests__/fixtures/gh-stub.sh
#
# Shared `gh` stub for the harvester and supervisor-watch suites (issue #2930).
# Dispatches on argv
# exactly like the real thing -- the REST run list
# `gh api repos/{owner}/{repo}/actions/workflows/health.yml/runs?...` (#3230),
# and (since #2931) the two per-run REST calls
# `gh api repos/{owner}/{repo}/actions/runs/<id>/jobs?per_page=100` and
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
#   run-list.json           the runs served by the list-workflow-runs endpoint
#                           `gh api repos/{owner}/{repo}/actions/workflows/health.yml/runs?...`
#                           Its REST paging/filtering lives in gh-stub-runs-page.mjs,
#                           which documents that endpoint's own markers (#3230)
#   run-list.fail           if present, every run-list call fails
#   run-list.fail-once      if present, the FIRST run-list call fails
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
#                           what a missing / deleted-unexpectedly job log
#                           looks like -- a 410 (below) is what an expired
#                           one looks like.
#   job-<jobId>.rate-limited
#                           if present, that job's log fetch fails with gh's
#                           primary rate-limit message (exit 1, HTTP 403)
#   job-<jobId>.blob-notfound
#                           if present, that job's log fetch reproduces the
#                           404 gh 2.88.1 printed for the #3230 cancelled
#                           jobs (captured 2026-10-12): the log endpoint's 302
#                           target (the Azure blob host) answers 404, so
#                           stderr is a bare "gh: HTTP 404" line followed by
#                           the blob's BlobNotFound XML, stdout is empty, and
#                           gh exits 1. Only the RequestId/Time values are
#                           placeholders.
#   job-<jobId>.blob-notfound-crlf
#                           the blob-notfound shape with CRLF line endings
#   job-<jobId>.secondary   if present, that job's log fetch fails every time
#                           with gh's secondary-rate-limit message (HTTP 403)
#   job-<jobId>.secondary-once
#                           the same, for the FIRST fetch only (consumed)
#   job-<jobId>.secondary-retry-after
#                           holds a number N: the FIRST fetch fails with the
#                           secondary-rate-limit message plus a
#                           "Retry-After: N" line (consumed)
#   job-<jobId>.blob-gone   if present, the same blob-host shape with
#                           "gh: HTTP 410" (an expired log reached via the
#                           302 target).
#   job-<jobId>.gone        if present, that job's log fetch reproduces gh's
#                           real 410 shape (#2995): the API error body on
#                           stdout, a "gh: Server Error (HTTP 410)" line on
#                           stderr, exit 1 (captured from `gh api` on
#                           2026-09-14). That is what a job log GitHub has
#                           already expired looks like.
#   job-<jobId>.gone-stderr-only
#                           if present, the same 410 exit but with nothing on
#                           stdout -- one of the two OR-sides the harvester's
#                           410 detection must recognise on its own.
#   job-<jobId>.gone-envelope-only
#                           if present, a *successful* exit (0) whose stdout
#                           is the 410 envelope with a numeric `410` status
#                           and nothing on stderr -- the other OR-side, and
#                           the shape `gh api` itself never produces this way,
#                           but which the harvester must still treat as gone
#                           rather than as harvested "data".
#   gh-help.fail            if present, `gh api --help` (the harvester's
#                           --allow-escape-sequences capability probe, #2986)
#                           fails every time: one line on stderr, exit 1.
#
# `GH_STUB_ESCAPE_GUARD` (default 1, mirroring the CI runner's gh >= 2.97):
# when "1", `gh api --help` advertises `--allow-escape-sequences` and a job
# log fetch containing a real ESC byte fails without the flag, reproducing
# gh 2.97's terminal-escape-sequence guard (#2986). When "0" (the developer
# machine's gh 2.88.1), `--help` does not advertise the flag and passing it
# anyway fails with "unknown flag" -- exactly what an out-of-date gh does.
#
# Every invocation is appended to $GH_STUB_FIXTURES_DIR/calls.log, one line
# of "$*" per call (full argv, including any --allow-escape-sequences), so
# callers can assert on the exact argv shape and call count.
#
# An invocation this stub does not recognise fails loudly (exit 1, naming
# the invocation on stderr) instead of returning a friendly default --
# silence here would let a real call-shape change look like it passed while
# exercising nothing. That is not hypothetical: it is exactly what caught
# #2931's migration from `gh run view` to these endpoints.

set -eu

FIXDIR="${GH_STUB_FIXTURES_DIR:?GH_STUB_FIXTURES_DIR not set}"
printf '%s\n' "$*" >>"$FIXDIR/calls.log"

# The run-list failure markers, checked before the REST runs endpoint is
# served (#3230 retired the `gh run list` call shape; the stub no longer
# answers it, so a regression back to it fails loudly below).
run_list_failure_markers() {
  if [ -f "$FIXDIR/run-list.fail" ]; then
    echo "gh-stub: simulated run list failure" >&2
    exit 1
  fi
  if [ -f "$FIXDIR/run-list.fail-once" ]; then
    rm "$FIXDIR/run-list.fail-once"
    echo "gh-stub: simulated transient run list failure" >&2
    exit 1
  fi
}

if [ "$1" = "api" ] && [ "$2" = "--help" ]; then
  if [ -f "$FIXDIR/gh-help.fail" ]; then
    echo "gh-stub: simulated api --help failure" >&2
    exit 1
  fi
  if [ "${GH_STUB_ESCAPE_GUARD:-1}" != "0" ]; then
    printf '      --allow-escape-sequences   Allow printing raw escape sequences to standard output\n'
  else
    printf '      --cache duration           Cache the response, e.g. "3600s", "60m", "1h"\n'
  fi
  exit 0
fi

if [ "$1" = "api" ]; then
  allowEscape=0
  if [ "$2" = "--allow-escape-sequences" ]; then
    allowEscape=1
    path="$3"
  else
    path="$2"
  fi
  case "$path" in
    */actions/workflows/*/runs\?*)
      run_list_failure_markers
      exec "${GH_STUB_NODE:-node}" "$(dirname "$0")/gh-stub-runs-page.mjs" "$path"
      ;;
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
      if [ -f "$FIXDIR/job-$jobId.rate-limited" ]; then
        printf '{"message":"API rate limit exceeded for installation ID 1.","status":"403"}'
        echo "gh: API rate limit exceeded for installation ID 1. (HTTP 403)" >&2
        exit 1
      fi
      secondary_msg='gh: You have exceeded a secondary rate limit. Please wait a few minutes before you try again. (HTTP 403)'
      if [ -f "$FIXDIR/job-$jobId.secondary" ]; then
        echo "$secondary_msg" >&2
        exit 1
      fi
      if [ -f "$FIXDIR/job-$jobId.secondary-once" ]; then
        rm "$FIXDIR/job-$jobId.secondary-once"
        echo "$secondary_msg" >&2
        exit 1
      fi
      if [ -f "$FIXDIR/job-$jobId.secondary-retry-after" ]; then
        retryAfter=$(cat "$FIXDIR/job-$jobId.secondary-retry-after")
        rm "$FIXDIR/job-$jobId.secondary-retry-after"
        printf '%s\nRetry-After: %s\n' "$secondary_msg" "$retryAfter" >&2
        exit 1
      fi
      if [ -f "$FIXDIR/job-$jobId.blob-notfound-crlf" ]; then
        printf 'gh: HTTP 404\r\n<?xml version="1.0" encoding="utf-8"?><Error><Code>BlobNotFound</Code><Message>The specified blob does not exist.\r\nRequestId:00000000-0000-0000-0000-000000000000\r\nTime:2026-10-12T05:10:00.0000000Z</Message></Error>' >&2
        exit 1
      fi
      if [ -f "$FIXDIR/job-$jobId.blob-notfound" ]; then
        printf 'gh: HTTP 404\n<?xml version="1.0" encoding="utf-8"?><Error><Code>BlobNotFound</Code><Message>The specified blob does not exist.\nRequestId:00000000-0000-0000-0000-000000000000\nTime:2026-10-12T05:10:00.0000000Z</Message></Error>' >&2
        exit 1
      fi
      if [ -f "$FIXDIR/job-$jobId.blob-gone" ]; then
        printf 'gh: HTTP 410\n<?xml version="1.0" encoding="utf-8"?><Error><Code>BlobNotFound</Code><Message>The specified blob does not exist.\nRequestId:00000000-0000-0000-0000-000000000000\nTime:2026-10-12T05:10:00.0000000Z</Message></Error>' >&2
        exit 1
      fi
      if [ -f "$FIXDIR/job-$jobId.notfound" ]; then
        printf '{"message":"Not Found","documentation_url":"https://docs.github.com/rest/actions/workflow-jobs#download-job-logs-for-a-workflow-run","status":"404"}'
        echo "gh: Not Found (HTTP 404)" >&2
        exit 1
      fi
      if [ -f "$FIXDIR/job-$jobId.gone" ]; then
        printf '{"message":"Server Error","documentation_url":"https://docs.github.com/rest/actions/workflow-jobs#download-job-logs-for-a-workflow-run","status":"410"}'
        echo "gh: Server Error (HTTP 410)" >&2
        exit 1
      fi
      if [ -f "$FIXDIR/job-$jobId.gone-stderr-only" ]; then
        echo "gh: Server Error (HTTP 410)" >&2
        exit 1
      fi
      if [ -f "$FIXDIR/job-$jobId.gone-envelope-only" ]; then
        printf '{"message":"Server Error","documentation_url":"https://docs.github.com/rest/actions/workflow-jobs#download-job-logs-for-a-workflow-run","status":410}'
        exit 0
      fi
      guard="${GH_STUB_ESCAPE_GUARD:-1}"
      if [ "$guard" = "0" ] && [ "$allowEscape" = "1" ]; then
        echo "unknown flag: --allow-escape-sequences" >&2
        exit 1
      fi
      if [ "$guard" != "0" ] && [ "$allowEscape" = "0" ]; then
        esc=$(printf '\033')
        if grep -q "$esc" "$FIXDIR/job-$jobId.log" 2>/dev/null; then
          echo "the response contains terminal escape sequences; pass --allow-escape-sequences to output it anyway" >&2
          exit 1
        fi
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
