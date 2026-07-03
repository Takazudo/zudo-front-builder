#!/usr/bin/env bash
set -euo pipefail

# scripts/file-exam-issue.sh — dedup tracking-issue filer for T3 "exam"
# (scheduled drift-net / heavy-lane) workflows, per the zudo-test-wisdom T3
# exam pattern (see CLAUDE.md "Testing" section).
#
# Files or appends to a SINGLE deduped tracking issue PER WORKFLOW (not per
# run): one open issue stays open while the workflow is red, gets a comment
# on each subsequent red run, and is closed automatically on the next green
# run (--green mode).
#
# Usage:
#   scripts/file-exam-issue.sh <workflow-name>            # failure: file/append
#   scripts/file-exam-issue.sh <workflow-name> --green    # success: close if open
#
# Required env:
#   GH_TOKEN — token for `gh` (caller must set `env: GH_TOKEN: ${{ github.token }}`
#              and job-level `permissions: issues: write`).
#   REPO     — "owner/repo" slug. Defaults to $GITHUB_REPOSITORY.
#
# Optional env (failure mode):
#   RUN_URL  — link to the failing run, embedded in the issue/comment body.
#   LEG      — which leg/job failed (e.g. "ubuntu-latest" or "macos-15").
#   DRY_RUN=1 — print intended gh mutations instead of executing them. Read-only
#               lookups (label list, issue list) still run so the dry-run output
#               reflects real state.
#
# Label: exam-failure (created on the repo if missing).

LABEL="exam-failure"
LABEL_COLOR="B60205"
LABEL_DESCRIPTION="Automated T3 scheduled-exam workflow failure tracking (zudo-test-wisdom)"

log() { printf '%s\n' "$1" >&2; }

# issue_title <workflow-name>
#
# Deterministic per-workflow title so lookups don't depend on GitHub's search
# index (which can lag) — find_existing_issue does an exact string compare
# against this instead of `gh issue list --search`.
issue_title() {
  printf '[exam-failure] %s is failing' "$1"
}

# ensure_label <repo>
#
# Idempotent: only calls `gh label create` when the label is actually absent.
ensure_label() {
  local repo="$1"
  if gh label list --repo "$repo" --json name | jq -r '.[].name' | grep -Fxq "$LABEL"; then
    return 0
  fi
  log "Label '$LABEL' not found on $repo — creating it."
  if [[ "${DRY_RUN:-}" == "1" ]]; then
    log "DRY_RUN=1 — skipping: gh label create $LABEL"
    return 0
  fi
  gh label create "$LABEL" --repo "$repo" --color "$LABEL_COLOR" --description "$LABEL_DESCRIPTION"
}

# find_existing_issue <repo> <title>
#
# Prints the issue number of the open, exam-failure-labeled issue whose title
# exactly matches <title>, or nothing if none exists.
find_existing_issue() {
  local repo="$1" title="$2"
  gh issue list --repo "$repo" --state open --label "$LABEL" --json number,title \
    | jq -r --arg t "$title" '.[] | select(.title == $t) | .number' | head -1
}

# file_failure <repo> <workflow-name>
#
# Creates a new tracking issue, or comments on the existing open one.
file_failure() {
  local repo="$1" workflow="$2"
  local run_url="${RUN_URL:-unknown}" leg="${LEG:-unknown}"
  local title body existing

  title=$(issue_title "$workflow")
  ensure_label "$repo"
  existing=$(find_existing_issue "$repo" "$title")

  body=$(printf 'Failing leg: %s\nRun: %s\n' "$leg" "$run_url")

  if [[ -n "$existing" ]]; then
    log "Existing open tracking issue #$existing found — appending comment."
    if [[ "${DRY_RUN:-}" == "1" ]]; then
      log "DRY_RUN=1 — skipping: gh issue comment $existing --body '$body'"
      return 0
    fi
    gh issue comment "$existing" --repo "$repo" --body "$body"
  else
    log "No open tracking issue found for '$workflow' — creating a new one."
    if [[ "${DRY_RUN:-}" == "1" ]]; then
      log "DRY_RUN=1 — skipping: gh issue create --title '$title' --label $LABEL"
      return 0
    fi
    gh issue create --repo "$repo" --title "$title" --body "$body" --label "$LABEL"
  fi
}

# close_on_green <repo> <workflow-name>
#
# Closes the open tracking issue (if any) with a recovery comment. No-op if
# no open tracking issue exists — a green run with nothing to close is not an
# error.
close_on_green() {
  local repo="$1" workflow="$2"
  local run_url="${RUN_URL:-unknown}"
  local title existing body

  title=$(issue_title "$workflow")
  existing=$(find_existing_issue "$repo" "$title")

  if [[ -z "$existing" ]]; then
    log "No open tracking issue for '$workflow' — nothing to close."
    return 0
  fi

  body=$(printf 'Workflow recovered (green run): %s\n' "$run_url")
  log "Closing tracking issue #$existing (workflow recovered)."
  if [[ "${DRY_RUN:-}" == "1" ]]; then
    log "DRY_RUN=1 — skipping: gh issue close $existing --comment '$body'"
    return 0
  fi
  gh issue close "$existing" --repo "$repo" --comment "$body"
}

main() {
  local workflow="${1:-}" mode="${2:-}"
  if [[ -z "$workflow" ]]; then
    echo "Usage: $0 <workflow-name> [--green]" >&2
    exit 2
  fi
  if [[ -n "$mode" && "$mode" != "--green" ]]; then
    echo "Usage: $0 <workflow-name> [--green]" >&2
    exit 2
  fi

  local repo="${REPO:-${GITHUB_REPOSITORY:-}}"
  if [[ -z "$repo" ]]; then
    echo "::error::REPO or GITHUB_REPOSITORY must be set" >&2
    exit 2
  fi

  if [[ "$mode" == "--green" ]]; then
    close_on_green "$repo" "$workflow"
  else
    file_failure "$repo" "$workflow"
  fi
}

# BASH_SOURCE guard: allows tests/unit/file-exam-issue.sh to `source` this
# script (to unit-test the functions above in isolation) without also
# running main() and its network-touching gh calls.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
