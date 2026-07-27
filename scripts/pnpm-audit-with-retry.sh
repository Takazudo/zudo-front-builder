#!/usr/bin/env bash
set -uo pipefail

# scripts/pnpm-audit-with-retry.sh — shared retry/classification wrapper
# around `pnpm audit --prod --audit-level=high`, factored out of
# .github/workflows/pr-checks.yml and .github/workflows/security-audit.yml
# (issue #2074, same shared-script pattern as scripts/file-exam-issue.sh).
#
# WHY: pnpm's audit command talks to the npm registry's advisories endpoint.
# A registry-side decode/transport failure there is NOT a real advisory
# finding, but the retry loop this replaces (2 attempts, flat 30s) treated it
# exactly like one — a registry incident that can't clear in 30s failed the
# gate the same way a genuine vulnerability would.
#
# CLASSIFICATION IS FAIL-CLOSED (CLAUDE.md "never game the gate"): only the
# recognized, CAPTURED infra signature(s) in INFRA_SIGNATURES below are
# retried. Any other non-zero outcome — a genuine `--audit-level=high`
# finding, or any error this script doesn't recognize — fails IMMEDIATELY
# with NO retry. The classifier must never guess "probably infrastructure"
# for something unrecognized.
#
# Recognized infra signature: ERR_PNPM_AUDIT_BAD_RESPONSE
#   pnpm's own error code for "the registry's advisories endpoint replied
#   with something the client couldn't use" (non-2xx status, invalid JSON,
#   or an unexpected response shape). Captured from pnpm's own source, not
#   hand-authored: pnpm/pnpm's audit test suite
#   (pnpm11/deps/compliance/audit/test/index.ts, commit 5ed47dcf) asserts
#   `err.code === 'ERR_PNPM_AUDIT_BAD_RESPONSE'` for exactly these cases, and
#   a real-world incident (pnpm/pnpm#11265, 2026-04: the npm registry retired
#   its legacy audit endpoints and returned 410) shows the exact message
#   shape CI will actually see:
#     ERR_PNPM_AUDIT_BAD_RESPONSE  The audit endpoint (at
#     https://registry.npmjs.org/-/npm/v1/security/advisories/bulk)
#     responded with 410: {"error":"This endpoint is being retired. ..."}
#   pnpm's default CLI reporter (pnpm11/cli/default-reporter/src/
#   reportError.ts, formatErrorSummary) prints the code in brackets —
#   "[ERR_PNPM_AUDIT_BAD_RESPONSE] <message>" — so grepping for the bare code
#   name catches that rendering regardless of color-code stripping in CI.
#   No other transport/decode signature was found captured anywhere, so none
#   is included — a narrower allowlist is safer than a guessed-broad one.
#
# CONTRACT NOTE (security-audit.yml): that workflow's file-or-close-issue /
# notify jobs read `needs.audit.result`, i.e. "real audit signal" there means
# job failure. An infra-exhaustion exit-0 (below) is deliberately invisible
# to that machinery — that is fine, because it is not a real audit signal:
# the weekly schedule re-checks independently within days. A genuine finding
# still fails the job exactly as before, immediately, no retry.
#
# Retry contract — FOUR attempts, THREE delays (issue #2074's review-amended
# contract): attempt 1 -> (infra failure) wait 30s -> attempt 2 -> wait 2m ->
# attempt 3 -> wait 5m -> attempt 4 (final). On infra exhaustion at attempt 4,
# this script exits 0 with a loud ::warning:: annotation instead of failing
# the job. The complete captured audit output is printed on EVERY attempt
# (not just the last), so the classification boundary stays auditable from
# the raw job log after the fact.
#
# Sleep durations are overridable via PNPM_AUDIT_RETRY_DELAYS (3
# space-separated seconds values) so the offline unit test
# (tests/unit/pnpm-audit-with-retry.sh) can run with zero-length delays
# instead of the real 30s/2m/5m.
#
# Usage: scripts/pnpm-audit-with-retry.sh
#   Runs `pnpm audit --prod --audit-level=high` in the current directory.

MAX_ATTEMPTS=4

# INFRA_SIGNATURES: the fail-closed allowlist of recognized, captured
# infrastructure error signatures. Add a new entry ONLY with a captured
# real-world sample and a comment citing its source (see header above) — per
# the epic's gate-semantics flag, this list is re-reviewed with fresh context
# before merge, so keep it small and named.
INFRA_SIGNATURES=(
  "ERR_PNPM_AUDIT_BAD_RESPONSE"
)

DELAYS_OVERRIDE="${PNPM_AUDIT_RETRY_DELAYS:-}"
if [[ -n "$DELAYS_OVERRIDE" ]]; then
  read -r -a DELAYS <<<"$DELAYS_OVERRIDE"
else
  DELAYS=(30 120 300)
fi

if [[ "${#DELAYS[@]}" -ne 3 ]]; then
  echo "::error::PNPM_AUDIT_RETRY_DELAYS must specify exactly 3 delays (got ${#DELAYS[@]}): '${DELAYS_OVERRIDE}'" >&2
  exit 2
fi

# is_infra_failure <captured-output>
#
# True (0) only when the output contains one of the recognized, captured
# infra signatures above. False (1) for everything else, including output
# this script has never seen — the fail-closed default.
is_infra_failure() {
  local output="$1" sig
  for sig in "${INFRA_SIGNATURES[@]}"; do
    if grep -qF -- "$sig" <<<"$output"; then
      return 0
    fi
  done
  return 1
}

attempt=1
while :; do
  echo "::group::pnpm audit attempt ${attempt}/${MAX_ATTEMPTS}"
  output=$(pnpm audit --prod --audit-level=high 2>&1)
  rc=$?
  printf '%s\n' "$output"
  echo "::endgroup::"

  if [[ "$rc" -eq 0 ]]; then
    echo "pnpm audit passed on attempt ${attempt}/${MAX_ATTEMPTS}."
    exit 0
  fi

  if ! is_infra_failure "$output"; then
    echo "::error::pnpm audit failed on attempt ${attempt}/${MAX_ATTEMPTS} with a genuine (non-infra) result — failing immediately, no retry."
    exit "$rc"
  fi

  if [[ "$attempt" -ge "$MAX_ATTEMPTS" ]]; then
    echo "::warning::pnpm audit signal missing — registry unavailable (ERR_PNPM_AUDIT_BAD_RESPONSE) after ${MAX_ATTEMPTS} attempts; weekly security-audit.yml re-checks independently"
    exit 0
  fi

  delay="${DELAYS[$((attempt - 1))]}"
  echo "pnpm audit attempt ${attempt}/${MAX_ATTEMPTS} hit a recognized infrastructure failure (ERR_PNPM_AUDIT_BAD_RESPONSE) — retrying in ${delay}s..."
  sleep "$delay"
  attempt=$((attempt + 1))
done
