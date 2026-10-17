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

# ── Inspect evidence and extract flaky testcases ──────────────────────────
# Prints one deduped test identity per line on stdout; evidence diagnostics
# go to stderr so they cannot be mistaken for test identities. A testcase is
# flaky iff it has a direct <flakyFailure> element without a final failure.
# This is nextest's "failed earlier, passed on the last try" marker (see the captured
# fixture for the real shape). Missing/invalid inputs reduce coverage, never
# establish a clean result. Counts describe observed elements, not suite
# aggregate attributes (which overlap in nested suites).
extract_flaky() {
  python3 - "$@" <<'PYEOF'
import sys
import xml.etree.ElementTree as ET

def report(message):
    print(message, file=sys.stderr)

valid = zero = suites_count = testcases = failures = errors = skipped = 0
flaky = set()
outcomes = {"failure", "error", "skipped", "flakyFailure", "rerunFailure"}
output_tags = {"system-out", "system-err"}
for path in sys.argv[1:]:
    try:
        tree = ET.parse(path)
    except FileNotFoundError:
        report(f"JUnit input {path!r}: unavailable (missing)")
        continue
    except (ET.ParseError, LookupError, ValueError) as exc:
        report(f"JUnit input {path!r}: invalid evidence (malformed XML: {exc})")
        continue
    except OSError as exc:
        report(f"JUnit input {path!r}: unavailable (unreadable: {exc})")
        continue
    root = tree.getroot()
    parents = {child: parent for parent in root.iter() for child in parent}
    if root.tag not in ("testsuite", "testsuites") or any(
        (node.tag == "testcase" and parents.get(node).tag != "testsuite")
        or (node.tag == "testcase" and any(
            child.tag not in outcomes | output_tags | {"properties"}
            for child in node
        ))
        or (node.tag in outcomes and parents[node].tag != "testcase")
        or (node.tag in outcomes and any(
            child.tag not in output_tags for child in node
        ))
        # nextest retains the first failure plus rerunFailure for failed retries.
        # Neither an orphan retry nor contradictory pass-on-retry is usable evidence.
        or (node.tag == "testcase" and node.find("rerunFailure") is not None
            and node.find("failure") is None and node.find("error") is None)
        or (node.tag == "testcase" and node.find("flakyFailure") is not None
            and any(node.find(tag) is not None
                    for tag in ("failure", "error", "skipped", "rerunFailure")))
        or (node.tag == "testcase" and node.find("flakyFailure") is not None
            and not (node.get("classname") or node.get("name")))
        or (node is not root and node.tag in ("testsuite", "testsuites")
            and parents[node].tag not in ("testsuite", "testsuites"))
        or (node.tag in ("testsuite", "testsuites") and any(
            child.tag not in (
                "testsuite", "testsuites", "properties", "system-out", "system-err",
                *(("testcase",) if node.tag == "testsuite" else ()),
            ) for child in node
        ))
        for node in root.iter()
    ):
        report(f"JUnit input {path!r}: invalid evidence (unsupported JUnit root/structure)")
        continue
    suites = list(root.iter("testsuite"))
    cases = [tc for suite in suites for tc in suite.findall("testcase")]
    valid += 1
    zero += not cases
    suites_count += len(suites)
    testcases += len(cases)
    failed = sum(tc.find("failure") is not None for tc in cases)
    errored = sum(tc.find("error") is not None for tc in cases)
    ignored = sum(tc.find("skipped") is not None for tc in cases)
    failures += failed
    errors += errored
    skipped += ignored
    report(f"JUnit input {path!r}: {'valid' if cases else 'zero-test evidence'}; "
           f"suites={len(suites)}; testcases={len(cases)}; failures={failed}; "
           f"errors={errored}; skipped={ignored}")
    for tc in cases:
        if tc.find("flakyFailure") is not None:
            classname = tc.get("classname", "")
            name = tc.get("name", "")
            flaky.add(f"{classname}::{name}")

coverage = "complete" if valid == len(sys.argv) - 1 and not zero else "partial"
if not testcases:
    coverage = "not-evaluated"
report(f"JUnit evidence: coverage={coverage}; valid-reports={valid}/{len(sys.argv) - 1}; "
       f"zero-test-reports={zero}; suites={suites_count}; testcases={testcases}; "
       f"failures={failures}; errors={errors}; skipped={skipped}; flaky-tests={len(flaky)}")
if not valid:
    report("Flaky tests not evaluated: no usable JUnit evidence.")
elif not testcases:
    report("Flaky tests not evaluated: valid reports contain zero testcases.")
elif not flaky:
    report(f"No flakes found in inspected reports (coverage={coverage}; "
           f"valid-reports={valid}; testcases={testcases}). "
           "Zero flakes does not establish that all tests passed.")
for test_id in sorted(flaky):
    print(test_id)
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
    sys.exit(1)
for issue in issues:
    if issue.get("title") == title:
        print(issue["number"])
        break
' "$title"
}

if ! FLAKY_LIST="$(extract_flaky "$@")"; then
  echo "::warning::Flaky tests not evaluated: JUnit extraction unavailable or failed; no reliable extraction result."
  printf 'Unavailable extraction input: %q\n' "$@"
  exit 0
fi

if [ -z "$FLAKY_LIST" ]; then
  exit 0
fi

ensure_label

printf '%s\n' "$FLAKY_LIST" | while IFS= read -r test_id; do
  title="flaky: ${test_id}"

  echo "::warning::Flaky test detected (pass-on-retry): ${test_id} — run: ${RUN_URL}"

  if ! existing_number="$(issue_number_for_title "$title")"; then
    echo "::warning::could not check existing flaky-test issue for ${test_id}; issue write not attempted"
    continue
  fi

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
