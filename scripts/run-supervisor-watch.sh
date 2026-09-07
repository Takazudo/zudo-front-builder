#!/usr/bin/env bash
set -euo pipefail

# scripts/run-supervisor-watch.sh — the two-pass body of .github/workflows/supervisor-watch.yml
# (issue #2918, epic #2915).
#
# Automates the reopen trigger of #2887: harvest the recent `health.yml`
# `[supervisor-timeline]` population (emitted since #2902/PR #2906) and run
# #2887's pre-registered R-A/R-B rules over it via the summarizer's --strict
# mode, so a real supervisor failure or a budget that has quietly become too
# tight becomes loud on a schedule instead of waiting for someone to remember
# to harvest by hand.
#
# TWO PASSES, because one population cannot answer both questions:
#
#   Pass A — ALL branches. Catches R-A (`outcome=failed` anywhere) and R-B
#     (max pre-UP >= 0.75 x budget). A red `health` on a PR is easy to re-run
#     past and forget, so PR runs must be in scope here. This population is
#     heterogeneous BY DESIGN (different zudo-doc versions, fixture edits,
#     steering env, per-run `env` digests), so every identity field is passed
#     to --allow-drift: drift is still reported, it just must not be the thing
#     that trips --strict on a population that is expected to be mixed.
#
#   Pass B — `main` only. This is where identity drift is a real signal: the
#     trunk population should be single-valued, so a second value means
#     something moved (zudo-doc bump, fixture edit, node/runner-image bump,
#     steering env change). Run with a bare --strict — no allow-list.
#
# NEVER `harvest | summarize`. Even under `set -o pipefail` it is the
# RIGHTMOST non-zero code that wins, so a harvester 64 (expired token, bad
# flag) would reach this script as the summarizer's 1 ("no data") and be
# classified as a quiet week. The harvester's own doc comment spells this out.
# Each pass therefore harvests into a file first and keeps both exit codes.
#
# NO-DATA IS NOT RED, BUT VANISHED TELEMETRY IS. `hrc=1` (zero records) means
# two very different things depending on the manifest's final line:
#   - `runs=0` — no `health` runs were enumerated at all. A genuinely quiet
#     week (or a repo-wide CI pause). Neutral: verdict `no-data`, exit 1.
#   - `runs>0` — runs existed and yielded no records. Either the emitter
#     stopped (the `ZFB_SUPERVISOR_TIMELINE=1` step regressed) or the
#     harvester broke. That is exactly the silent-no-op failure mode #2902
#     exists to prevent, so it is RED.
#
# Exit codes (the workflow reads them; see supervisor-watch.yml):
#   0  green    — pass A ok (pass B may legitimately be `empty` on a quiet trunk week)
#   1  no-data  — pass A enumerated zero runs
#   2  red      — either pass is red
#
# Env:
#   WATCH_OUT_DIR     output tree (default ./supervisor-watch-out); subdirs all/, main/, job-logs/
#   WATCH_SINCE       when set, passed to the harvester as --since; when UNSET the
#                     flag is OMITTED so the harvester's own default window applies
#                     (a sibling sub-issue is making that default a rolling window —
#                     hardcoding one here would fight it)
#   WATCH_GH          alternate `gh` executable, passed as --gh (used by the unit test)
#   WATCH_MAIN_BRANCH trunk branch for pass B (default main)
#
# Also honours GITHUB_OUTPUT / GITHUB_STEP_SUMMARY when set, so the workflow
# step needs no reformatting logic of its own.

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/.." && pwd)

HARVEST="$REPO_ROOT/scripts/harvest-supervisor-timelines.mjs"
SUMMARIZE="$REPO_ROOT/scripts/supervisor-timeline-summary.mjs"

OUT="${WATCH_OUT_DIR:-./supervisor-watch-out}"
MAIN_BRANCH="${WATCH_MAIN_BRANCH:-main}"

# Every identity field: see the pass A rationale in the header.
ALLOW_DRIFT="runner,zudoDoc,runParallel,fixtureShape,env"

mkdir -p "$OUT/all" "$OUT/main" "$OUT/job-logs"

# Shared harvester flags. Built as an array so a value containing spaces
# survives; expanded with the `${arr[@]+...}` guard because a bare
# `"${arr[@]}"` on an EMPTY array is an unbound-variable error under `set -u`
# in bash 3.2 (still /bin/bash on macOS, where the dry run is done).
HARVEST_COMMON=()
if [ -n "${WATCH_SINCE:-}" ]; then
  HARVEST_COMMON+=(--since "$WATCH_SINCE")
fi
if [ -n "${WATCH_GH:-}" ]; then
  HARVEST_COMMON+=(--gh "$WATCH_GH")
fi

log() { printf '%s\n' "$*" >&2; }

# manifest_enumerated_no_runs <manifest-path>
#
# True when the harvester's final summary line reports zero enumerated runs.
# Only that line starts with `runs=`; per-run lines start with `run=` and a
# usage error starts with `usage error:` — so a harvester that died before
# enumerating anything correctly fails this test and stays red.
manifest_enumerated_no_runs() {
  local last
  last=$(tail -n 1 "$1" 2>/dev/null || true)
  case "$last" in
    "runs=0" | "runs=0 "*) return 0 ;;
    *) return 1 ;;
  esac
}

# classify_pass <hrc> <src> <manifest-path> -> ok | empty | red
classify_pass() {
  local hrc="$1" src="$2" manifest="$3"
  if [ "$hrc" -eq 0 ] && [ "$src" -eq 0 ]; then
    printf 'ok'
  elif [ "$hrc" -eq 1 ] && manifest_enumerated_no_runs "$manifest"; then
    printf 'empty'
  else
    printf 'red'
  fi
}

# run_pass <subdir> <label> [extra harvester args...] -- [extra summarizer args...]
#
# Sets PASS_HRC / PASS_SRC / PASS_STATUS. The summarizer ALWAYS runs, even
# after a failed harvest: an empty timelines.txt simply yields its exit 1, and
# running it unconditionally keeps summary.txt present for the job summary and
# the artifact in every branch of the logic.
run_pass() {
  local subdir="$1" label="$2"
  shift 2

  local harvest_extra=()
  while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do
    harvest_extra+=("$1")
    shift
  done
  shift || true
  # `local x=("$@")` on an empty "$@" is an unbound-variable error under
  # `set -u` in bash 3.2, so build it conditionally rather than directly.
  local summarize_extra=()
  if [ "$#" -gt 0 ]; then
    summarize_extra=("$@")
  fi

  local dir="$OUT/$subdir"

  log "==> pass $label: harvesting"
  set +e
  node "$HARVEST" \
    ${HARVEST_COMMON[@]+"${HARVEST_COMMON[@]}"} \
    ${harvest_extra[@]+"${harvest_extra[@]}"} \
    >"$dir/timelines.txt" 2>"$dir/manifest.txt"
  PASS_HRC=$?
  set -e

  log "==> pass $label: summarizing"
  set +e
  node "$SUMMARIZE" --strict \
    ${summarize_extra[@]+"${summarize_extra[@]}"} \
    <"$dir/timelines.txt" >"$dir/summary.txt" 2>&1
  PASS_SRC=$?
  set -e

  PASS_STATUS=$(classify_pass "$PASS_HRC" "$PASS_SRC" "$dir/manifest.txt")
  log "==> pass $label: status=$PASS_STATUS hrc=$PASS_HRC src=$PASS_SRC"
}

run_pass all "A (all branches)" --save-dir "$OUT/job-logs" -- --allow-drift "$ALLOW_DRIFT"
HRC_A=$PASS_HRC
SRC_A=$PASS_SRC
STATUS_A=$PASS_STATUS

run_pass main "B ($MAIN_BRANCH only)" --branch "$MAIN_BRANCH" --
HRC_B=$PASS_HRC
SRC_B=$PASS_SRC
STATUS_B=$PASS_STATUS

# Provenance for R-A triage: which run's saved job log holds the diagnostic
# block. Matched tolerantly (`failed=[1-9]` anywhere on the line) so this
# works both against today's manifest — where only the final summary line
# carries a `failed=` count, i.e. a PARTIAL harvest — and against the sibling
# sub-issue's addition of a per-run `failed=<m>` record count.
{
  grep -E 'failed=[1-9]' "$OUT/all/manifest.txt" || true
  grep -E 'failed=[1-9]' "$OUT/main/manifest.txt" || true
} >"$OUT/failed-runs.txt"

VERDICT=green
EXIT_CODE=0
if [ "$STATUS_A" = red ] || [ "$STATUS_B" = red ]; then
  VERDICT=red
  EXIT_CODE=2
elif [ "$STATUS_A" = empty ]; then
  VERDICT=no-data
  EXIT_CODE=1
fi

DETAIL="all=$STATUS_A:$HRC_A/$SRC_A main=$STATUS_B:$HRC_B/$SRC_B"

if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    printf 'verdict=%s\n' "$VERDICT"
    printf 'detail=%s\n' "$DETAIL"
  } >>"$GITHUB_OUTPUT"
fi

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    printf '## Supervisor watch — %s\n\n' "$VERDICT"
    printf '`verdict=%s %s`\n\n' "$VERDICT" "$DETAIL"

    printf '### Runs with failed supervisor records\n\n'
    printf '```\n'
    if [ -s "$OUT/failed-runs.txt" ]; then
      cat "$OUT/failed-runs.txt"
    else
      printf 'none\n'
    fi
    printf '```\n\n'

    printf '### Harvest manifests (final line)\n\n'
    printf -- '- all: `%s`\n' "$(tail -n 1 "$OUT/all/manifest.txt" 2>/dev/null || true)"
    printf -- '- %s: `%s`\n\n' "$MAIN_BRANCH" "$(tail -n 1 "$OUT/main/manifest.txt" 2>/dev/null || true)"

    printf '### Summary — all branches (identity drift allow-listed)\n\n'
    printf '```\n'
    cat "$OUT/all/summary.txt"
    printf '```\n\n'

    printf '### Summary — %s only (strict identity)\n\n' "$MAIN_BRANCH"
    printf '```\n'
    cat "$OUT/main/summary.txt"
    printf '```\n'
  } >>"$GITHUB_STEP_SUMMARY"
fi

# Last stdout line, by contract — the workflow and the unit test both read it.
printf 'verdict=%s %s\n' "$VERDICT" "$DETAIL"
exit "$EXIT_CODE"
