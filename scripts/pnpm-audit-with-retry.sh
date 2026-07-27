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
#   (pnpm11/deps/compliance/audit/test/index.ts, lines 553/580/607) asserts
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
#   (Sibling AuditEndpointNotExistsError — a 404 from the endpoint — is also
#   registry-side but is deliberately NOT allowlisted: it fails the gate.)
#
# WHY THE SIGNATURE ALONE IS NOT ENOUGH (fresh-context review, epic #2071):
# a bare substring match over the whole captured output can be satisfied by
# text this repo does not control. `pnpm audit` renders each advisory's
# `title` verbatim from the GitHub Advisory Database, so an advisory whose
# title happened to contain the signature name would make a GENUINE
# high-severity finding classify as infrastructure — retried, then passed
# with exit 0. That was demonstrated with a local shim during review.
# The mitigation is FINDING_MARKERS below: pnpm THROWS on
# AUDIT_BAD_RESPONSE before any advisory is rendered, so a findings table
# and this error are mutually exclusive in one run. Requiring the absence of
# findings-shaped output therefore costs no false reds, and any collision
# resolves toward "genuine" — the fail-closed direction.
#
# CONTRACT NOTE (security-audit.yml): that workflow's file-or-close-issue /
# notify jobs read `needs.audit.result`, i.e. "real audit signal" there means
# job failure — including its "Close tracking issue (green)" step, which
# closes an open tracking issue when the job succeeds. An infra-exhaustion
# exit-0 there would be a FALSE GREEN that closes the very paper trail issue
# #1394 created that lane to keep, and the "the weekly schedule re-checks
# independently" justification is circular when the caller IS the weekly
# schedule. So exhaustion behaviour is per-caller, not baked in:
# PNPM_AUDIT_EXHAUSTION_EXIT defaults to 1 (fail closed) and pr-checks.yml
# opts into 0 explicitly, which also keeps the relaxation visible at the call
# site of the required gate rather than buried in this script.
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

# FINDING_MARKERS: text pnpm emits only when it actually rendered advisories.
# Presence of ANY of these means the run produced a real audit verdict, so it
# is NEVER infrastructure — regardless of what else the output contains. See
# "WHY THE SIGNATURE ALONE IS NOT ENOUGH" in the header.
FINDING_MARKERS=(
  "vulnerabilities found"
  "Severity:"
)

# Exhaustion behaviour is the caller's decision — see the CONTRACT NOTE above.
# 1 (default) = fail closed. 0 = exit 0 with a loud annotation, which only the
# PR gate opts into.
EXHAUSTION_EXIT="${PNPM_AUDIT_EXHAUSTION_EXIT:-1}"
if [[ "$EXHAUSTION_EXIT" != "0" && "$EXHAUSTION_EXIT" != "1" ]]; then
  echo "::error::PNPM_AUDIT_EXHAUSTION_EXIT must be 0 or 1 (got '${EXHAUSTION_EXIT}')" >&2
  exit 2
fi

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

# Validate VALUES, not just the count: a non-numeric entry would make `sleep`
# fail non-fatally (set -e is deliberately off) and silently collapse the
# whole backoff to zero, which is precisely what makes an exhaustion exit
# easiest to reach.
for delay_value in "${DELAYS[@]}"; do
  if [[ ! "$delay_value" =~ ^[0-9]+$ ]]; then
    echo "::error::PNPM_AUDIT_RETRY_DELAYS entries must be non-negative integers (got '${delay_value}' in '${DELAYS_OVERRIDE}')" >&2
    exit 2
  fi
done

# is_infra_failure <captured-output>
#
# True (0) only when the output contains one of the recognized, captured
# infra signatures AND contains no findings-shaped output. False (1) for
# everything else, including output this script has never seen — the
# fail-closed default.
#
# The findings check runs FIRST and is decisive: an advisory `title` comes
# verbatim from the registry, so the signature string can appear inside a
# real findings table. Treating that as infrastructure would retry a genuine
# high-severity finding and then pass the gate.
is_infra_failure() {
  local output="$1" sig marker
  for marker in "${FINDING_MARKERS[@]}"; do
    if grep -qF -- "$marker" <<<"$output"; then
      return 1
    fi
  done
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
    if [[ "$EXHAUSTION_EXIT" -eq 0 ]]; then
      echo "::warning::pnpm audit signal missing — registry unavailable (ERR_PNPM_AUDIT_BAD_RESPONSE) after ${MAX_ATTEMPTS} attempts; weekly security-audit.yml re-checks independently"
      exit 0
    fi
    echo "::error::pnpm audit signal missing — registry unavailable (ERR_PNPM_AUDIT_BAD_RESPONSE) after ${MAX_ATTEMPTS} attempts; failing closed (PNPM_AUDIT_EXHAUSTION_EXIT=1)"
    exit 1
  fi

  delay="${DELAYS[$((attempt - 1))]}"
  echo "pnpm audit attempt ${attempt}/${MAX_ATTEMPTS} hit a recognized infrastructure failure (ERR_PNPM_AUDIT_BAD_RESPONSE) — retrying in ${delay}s..."
  sleep "$delay"
  attempt=$((attempt + 1))
done
