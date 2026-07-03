#!/usr/bin/env bash
#
# scripts/report-flaky-tests.sh — flaky-test telemetry (issue #1341).
#
# CLAUDE.md's flaky-test policy says "pass-on-retry is a triage signal, not a
# success — record it and schedule the fix." Issue #1340 made cargo-nextest
# RECORD that signal (CI profile `retries = 1`, JUnit output). This script is
# the CONSUMER: it parses the JUnit report(s) nextest writes, finds every test
# that failed then passed on retry ("flaky" — nextest/quick-junit nests a
# <flakyFailure> element inside such a <testcase>, verified against a real
# captured report at tests/fixtures/flaky-junit-example.xml), and for each one:
#
#   1. emits a `::warning::` GitHub Actions annotation
#   2. files a deduped tracking issue (label `flaky`, title `flaky: <test-id>`)
#      — or comments on the existing OPEN one instead of duplicating it
#
# Usage:
#   scripts/report-flaky-tests.sh <junit.xml> [<junit.xml> ...]
#
# Env:
#   DRY_RUN=1              — run the full detection + dedup logic but only
#                             PRINT the `gh issue create`/`gh issue comment`
#                             command instead of executing it. No network
#                             calls are made in this mode (see
#                             FLAKY_EXISTING_OPEN_ISSUES below) — this is what
#                             tests/unit/report-flaky-tests.sh exercises.
#   FLAKY_EXISTING_OPEN_ISSUES
#                          — (DRY_RUN only) newline-separated list of exact
#                            issue titles to treat as "already open", so the
#                            dedup check ("does this issue already exist?")
#                            is mockable/injectable for an offline test
#                            instead of requiring a live `gh issue list` call.
#   GH_TOKEN               — required by `gh` whenever the workflow's default
#                             permissions don't include write access (health.yml
#                             sets this explicitly on the job/step that calls
#                             this script — see CLAUDE.md's Permissions note).
#   GITHUB_RUN_URL          — URL of the CI run, linked in the issue body.
#   GITHUB_REPOSITORY       — owner/repo, passed to `gh --repo` when set.
#
# Fork-PR tokens are read-only: a fork PR's GITHUB_TOKEN cannot create issues
# or comments. Every `gh issue create`/`gh issue comment` call below is
# best-effort — a permissions failure emits a `::warning::` and this script
# still exits 0, so it never breaks the CI run.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

DRY_RUN="${DRY_RUN:-0}"
LABEL="flaky"
RUN_URL="${GITHUB_RUN_URL:-<no CI run URL — run locally>}"

GH_REPO_ARGS=()
if [ -n "${GITHUB_REPOSITORY:-}" ]; then
  GH_REPO_ARGS=(--repo "$GITHUB_REPOSITORY")
fi

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <junit.xml> [<junit.xml> ...]" >&2
  exit 1
fi

# ── Ensure the `flaky` label exists (idempotent, best-effort) ───────────────
# Needs issues:write; on a fork PR (read-only token) this silently no-ops via
# `|| true`, matching the fork-PR best-effort pattern used throughout.
ensure_label() {
  if [ "$DRY_RUN" = "1" ]; then
    echo "[DRY_RUN] gh label create \"$LABEL\" --description \"Quarantined flaky test — tracked per the zudo-test-wisdom quarantine pipeline\" --color \"fbca04\""
    return 0
  fi
  gh label create "$LABEL" \
    --description "Quarantined flaky test — tracked per the zudo-test-wisdom quarantine pipeline" \
    --color "fbca04" \
    "${GH_REPO_ARGS[@]}" 2>/dev/null || true
}

# ── Extract flaky testcases from the given JUnit XML file(s) ────────────────
# Prints one line per flaky testcase as "<classname>\t<name>". A testcase is
# flaky iff it has a nested <flakyFailure> element — nextest's marker for
# "failed on an earlier try, passed on the last try" (see the captured
# fixture for the real shape). Missing/unparsable files are skipped, not
# fatal — an early compile failure can leave no JUnit report at all, and this
# script must never be why the CI run breaks.
extract_flaky() {
  python3 - "$@" <<'PYEOF'
import sys
import xml.etree.ElementTree as ET

for path in sys.argv[1:]:
    try:
        tree = ET.parse(path)
    except (FileNotFoundError, ET.ParseError, OSError):
        continue
    root = tree.getroot()
    suites = [root] if root.tag == "testsuite" else root.findall(".//testsuite")
    for suite in suites:
        for tc in suite.findall("testcase"):
            if tc.find("flakyFailure") is not None:
                classname = tc.get("classname", "")
                name = tc.get("name", "")
                print(f"{classname}\t{name}")
PYEOF
}

# ── Dedup check: does an OPEN issue with this exact title already exist? ────
# Prints the issue number (or nothing) on stdout.
#
# DRY_RUN mode never touches the network: it is mocked via
# FLAKY_EXISTING_OPEN_ISSUES so the offline unit test can simulate "this is
# the second run, the issue from the first run already exists" without a
# live `gh issue list` call.
issue_number_for_title() {
  local title="$1"
  if [ "$DRY_RUN" = "1" ]; then
    if printf '%s\n' "${FLAKY_EXISTING_OPEN_ISSUES:-}" | grep -Fxq "$title"; then
      echo "DRY_RUN_EXISTING"
    fi
    return 0
  fi
  gh issue list --state open --label "$LABEL" \
    --search "in:title \"$title\"" \
    --json number,title \
    "${GH_REPO_ARGS[@]}" 2>/dev/null \
    | python3 -c '
import json, sys
title = sys.argv[1]
try:
    issues = json.load(sys.stdin)
except Exception:
    issues = []
for issue in issues:
    if issue.get("title") == title:
        print(issue["number"])
        break
' "$title"
}

FLAKY_LIST="$(extract_flaky "$@" | sort -u)"

if [ -z "$FLAKY_LIST" ]; then
  echo "No flaky tests detected."
  exit 0
fi

ensure_label

printf '%s\n' "$FLAKY_LIST" | while IFS=$'\t' read -r classname name; do
  [ -z "$classname" ] && [ -z "$name" ] && continue
  test_id="${classname}::${name}"
  title="flaky: ${test_id}"

  echo "::warning::Flaky test detected (pass-on-retry): ${test_id} — run: ${RUN_URL}"

  existing_number="$(issue_number_for_title "$title")"

  if [ -n "$existing_number" ]; then
    body="Flaky again.

Detected by \`scripts/report-flaky-tests.sh\` (issue #1341) — this test failed on an earlier try and passed on retry under cargo-nextest's CI retry budget (\`retries = 1\`, see \`.config/nextest.toml\`).

Run: ${RUN_URL}"
    if [ "$DRY_RUN" = "1" ]; then
      echo "[DRY_RUN] would comment on existing issue for \"$title\": gh issue comment $existing_number --body \"...\""
    else
      gh issue comment "$existing_number" --body "$body" "${GH_REPO_ARGS[@]}" \
        || echo "::warning::could not comment on flaky-test issue #$existing_number for ${test_id} (likely a fork PR with read-only token)"
    fi
  else
    body="A test failed on its first attempt and passed on retry under cargo-nextest's CI retry budget.

Detected by \`scripts/report-flaky-tests.sh\` (issue #1341, per CLAUDE.md's flaky-test policy: \"pass-on-retry is a triage signal, not a success\").

Run: ${RUN_URL}"
    if [ "$DRY_RUN" = "1" ]; then
      echo "[DRY_RUN] would create issue: gh issue create --title \"$title\" --body \"...\" --label \"$LABEL\""
    else
      gh issue create --title "$title" --body "$body" --label "$LABEL" "${GH_REPO_ARGS[@]}" \
        || echo "::warning::could not file flaky-test issue for ${test_id} (likely a fork PR with read-only token)"
    fi
  fi
done

# This script must never be the reason the CI run goes red (every `gh` call
# above is already best-effort-guarded) — exit 0 unconditionally rather than
# propagate whatever exit status the last loop iteration happened to leave.
exit 0
