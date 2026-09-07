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
# ONE HARVEST, TWO PASSES, because one population cannot answer both questions:
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
#     steering env change). Run with a bare --strict — no allow-list. Its
#     input is NOT a second network harvest: pass A already saved every
#     fetched job log under job-logs/ and its manifest names each run's
#     branch, so the `main` subset is selected from those files. That keeps
#     the two passes on one enumeration (a run finishing between two harvests
#     could otherwise be judged in one population only) and halves the gh
#     traffic inside the job's timeout.
#
# NEVER `harvest | summarize`. Even under `set -o pipefail` it is the
# RIGHTMOST non-zero code that wins, so a harvester 64 (expired token, bad
# flag) would reach this script as the summarizer's 1 ("no data") and be
# classified as a quiet week. The harvester's own doc comment spells this out.
# The harvest therefore lands in a file first and keeps its own exit code.
#
# NO-DATA IS NOT RED, BUT VANISHED TELEMETRY IS. The harvester keeps those
# apart by exit code (see its header): 4 means nothing was harvestable (zero
# runs enumerated, or every run skipped — in progress, no `health` job, red
# before the test step), which is a quiet week; 1 means at least one green
# `health` job was harvested and carried no records, which is the emitter
# (or the parser) going silent — exactly the silent no-op #2902 exists to
# prevent, so it is RED.
#
# Exit codes (informational — the workflow keys on the `verdict=` token it
# writes to $GITHUB_OUTPUT, never on this code, because a `set -e` death
# inside this script also exits 1 and must not read as a quiet week):
#   0  green    — pass A ok (pass B may legitimately be `empty` on a quiet trunk week)
#   1  no-data  — pass A harvested nothing
#   2  red      — either pass is red
#
# Env:
#   WATCH_OUT_DIR     output tree (default ./supervisor-watch-out; gitignored)
#   WATCH_SINCE       when set, passed to the harvester as --since; when UNSET the
#                     flag is OMITTED so the harvester's own default applies (a
#                     rolling window floored at IDENTITY_CONTRACT_EPOCH — keeping
#                     that in one place is the point of omitting it)
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

# Every identity field, read from the summarizer itself so a field added
# there cannot leave this list behind and start tripping pass A weekly. The
# path travels via the environment, not argv: the summarizer runs its CLI
# when `process.argv[1]` is its own path.
ALLOW_DRIFT=$(SUMMARIZE_PATH="$SUMMARIZE" node -e 'import(process.env.SUMMARIZE_PATH).then((m) => process.stdout.write(m.IDENTITY_FIELDS.join(",")))')

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

# ── Pass A: harvest everything once, summarize with drift allow-listed ───────

log "==> pass A (all branches): harvesting"
set +e
node "$HARVEST" \
  ${HARVEST_COMMON[@]+"${HARVEST_COMMON[@]}"} \
  --save-dir "$OUT/job-logs" \
  >"$OUT/all/timelines.txt" 2>"$OUT/all/manifest.txt"
HRC_A=$?
set -e

# The summarizer ALWAYS runs, even after a failed harvest: an empty
# timelines.txt simply yields its exit 1, and running it unconditionally
# keeps summary.txt present for the job summary and the artifact.
log "==> pass A (all branches): summarizing"
set +e
node "$SUMMARIZE" --strict --allow-drift "$ALLOW_DRIFT" \
  <"$OUT/all/timelines.txt" >"$OUT/all/summary.txt" 2>&1
SRC_A=$?
set -e

# ok    — harvest complete with records, no strict finding
# empty — harvester 4: nothing harvestable
# red   — everything else: harvester 1 (green health jobs, zero records),
#         3 (partial), 64 (usage/gh); summarizer 2 (R-A/R-B/non-allow-listed
#         drift), 64 (parse), or 1 with records present (none for the
#         summarizer's default `--case`, which a sample this size never lacks)
if [ "$HRC_A" -eq 0 ] && [ "$SRC_A" -eq 0 ]; then
  STATUS_A=ok
elif [ "$HRC_A" -eq 4 ]; then
  STATUS_A=empty
else
  STATUS_A=red
fi
log "==> pass A (all branches): status=$STATUS_A hrc=$HRC_A src=$SRC_A"

# ── Pass B: the trunk subset of pass A's saved logs, strict identity ────────

# Harvested manifest lines only (`lines=` — skipped/errored runs have no
# usable log), on the trunk branch. Fixed-string match on the branch token so
# a branch name is never read as a regex.
grep -E '^run=[0-9]+ .* job=[0-9]+ lines=[0-9]+ ' "$OUT/all/manifest.txt" 2>/dev/null \
  | grep -F -- " branch=$MAIN_BRANCH " >"$OUT/main/runs.txt" || true

MAIN_LOGS=()
MAIN_COUNT=0
while IFS= read -r line; do
  run_id=${line#run=}
  run_id=${run_id%% *}
  job_id=${line##* job=}
  job_id=${job_id%% *}
  MAIN_LOGS+=("$OUT/job-logs/run-$run_id-job-$job_id.log")
  MAIN_COUNT=$((MAIN_COUNT + 1))
done <"$OUT/main/runs.txt"

if [ "$MAIN_COUNT" -eq 0 ]; then
  printf 'no %s runs were harvested in this window (see all/manifest.txt)\n' "$MAIN_BRANCH" \
    >"$OUT/main/summary.txt"
  SRC_B=-
  STATUS_B=empty
else
  log "==> pass B ($MAIN_BRANCH only): summarizing $MAIN_COUNT saved job log(s)"
  set +e
  node "$SUMMARIZE" --strict "${MAIN_LOGS[@]}" >"$OUT/main/summary.txt" 2>&1
  SRC_B=$?
  set -e
  if [ "$SRC_B" -eq 0 ]; then
    STATUS_B=ok
  else
    STATUS_B=red
  fi
fi
log "==> pass B ($MAIN_BRANCH only): status=$STATUS_B runs=$MAIN_COUNT src=$SRC_B"

# ── Provenance for triage ────────────────────────────────────────────────────

# Per-run manifest lines whose `failed=<m>` record count is non-zero: these
# name the run (and saved job log) holding an R-A diagnostic block. The final
# summary line's `failed=<f>` is a different counter (runs the harvester could
# not fetch/parse) and is deliberately not matched here — it does not name a
# run and describes a harvest problem, which the `error=` lines below cover.
grep -E '^run=[0-9]+ .* failed=[1-9]' "$OUT/all/manifest.txt" >"$OUT/failed-runs.txt" || true
grep -E '^run=[0-9]+ .* error=' "$OUT/all/manifest.txt" >"$OUT/harvest-errors.txt" || true
# The effective window, the --limit cap notice, and a future-`--since` warning.
grep -E '^(window|notice|warning):' "$OUT/all/manifest.txt" >"$OUT/harvest-notices.txt" || true

VERDICT=green
EXIT_CODE=0
if [ "$STATUS_A" = red ] || [ "$STATUS_B" = red ]; then
  VERDICT=red
  EXIT_CODE=2
elif [ "$STATUS_A" = empty ]; then
  VERDICT=no-data
  EXIT_CODE=1
fi

# all=<status>:<harvester rc>/<summarizer rc> main=<status>:<runs>/<summarizer rc>
DETAIL="all=$STATUS_A:$HRC_A/$SRC_A main=$STATUS_B:$MAIN_COUNT/$SRC_B"

if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    printf 'verdict=%s\n' "$VERDICT"
    printf 'detail=%s\n' "$DETAIL"
  } >>"$GITHUB_OUTPUT"
fi

print_file_or_none() {
  if [ -s "$1" ]; then
    cat "$1"
  else
    printf 'none\n'
  fi
}

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    printf '## Supervisor watch — %s\n\n' "$VERDICT"
    printf '`verdict=%s %s`\n\n' "$VERDICT" "$DETAIL"

    printf '### Runs with failed supervisor records (R-A: read the saved job log)\n\n'
    printf '```\n'
    print_file_or_none "$OUT/failed-runs.txt"
    printf '```\n\n'

    printf '### Runs the harvester could not fetch or parse\n\n'
    printf '```\n'
    print_file_or_none "$OUT/harvest-errors.txt"
    printf '```\n\n'

    printf '### Harvest window and notices\n\n'
    printf '```\n'
    print_file_or_none "$OUT/harvest-notices.txt"
    printf -- '%s\n' "$(tail -n 1 "$OUT/all/manifest.txt" 2>/dev/null || true)"
    printf '```\n\n'

    printf '### Summary — all branches (identity drift allow-listed)\n\n'
    printf '```\n'
    cat "$OUT/all/summary.txt"
    printf '```\n\n'

    printf '### Summary — %s only (strict identity, %s run(s))\n\n' "$MAIN_BRANCH" "$MAIN_COUNT"
    printf '```\n'
    cat "$OUT/main/summary.txt"
    printf '```\n'
  } >>"$GITHUB_STEP_SUMMARY"
fi

# Last stdout line, by contract — the unit test reads it.
printf 'verdict=%s %s\n' "$VERDICT" "$DETAIL"
exit "$EXIT_CODE"
