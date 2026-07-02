#!/usr/bin/env bash
set -euo pipefail
#
# apply-main-ruleset.sh — (re-)create the GitHub ruleset on `main` that
# codifies the always-run status checks as required-before-merge, checked in
# here for reproducibility (issue #1333).
#
# What this requires (only checks that run on EVERY PR, unconditionally —
# no path filters, no workflow_run/tag-only triggers):
#   - health                                        (.github/workflows/health.yml)
#   - build (no-v8)                                 (.github/workflows/health.yml)
#   - Build binary (x86_64-unknown-linux-gnu)       (.github/workflows/node-free-smoke.yml)
#   - Build binary (aarch64-unknown-linux-gnu)      (.github/workflows/node-free-smoke.yml)
#   - Smoke amd64 (local mode)                      (.github/workflows/node-free-smoke.yml)
#   - Smoke amd64 TS-config (local mode)            (.github/workflows/node-free-smoke.yml)
#   - Smoke arm64 (local mode)                      (.github/workflows/node-free-smoke.yml)
#   - Smoke arm64 TS-config (local mode)            (.github/workflows/node-free-smoke.yml)
#   - pnpm audit (prod)                             (.github/workflows/pr-checks.yml)
#
# Build binary's two matrix legs both build on the ubuntu-22.04 runner (the
# GLIBC 2.35 floor host, and the arm64 leg cross-compiles rather than using a
# native arm64 runner — see node-free-smoke.yml comments), so the check name
# is derived from matrix.platform.target rather than matrix.platform.runner —
# otherwise both legs would render the identical check context "Build binary
# (ubuntu-22.04)" and a ruleset couldn't require them independently.
#
# Deliberately NOT required:
#   - "Build docs site" (.github/workflows/pr-checks.yml) — sibling issue
#     #1336 is making this job path-filtered to docs/** changes. A required
#     check that stops running on non-docs PRs hangs those PRs forever, so
#     it is excluded here on purpose. If a future change makes it
#     unconditional again, add it back.
#   - "Smoke released (install.sh)" — only fires via workflow_run after a
#     release tag, never on a normal PR; requiring it would block all PRs.
#   - anything from docs-pr-preview.yml / docs-deploy.yml / actionlint.yml /
#     security-audit.yml — all already path-filtered, tag-only, or
#     schedule-only, so none of them run on every PR.
#
# Bypass: repo-admin (RepositoryRole actor_id=5, the built-in "admin" role —
# there is no OrganizationAdmin equivalent on a user-owned repo) with
# bypass_mode "always", so `/l-make-release`'s direct version-bump push to
# main (SKILL.md Step 6) keeps working. required_status_checks blocks a
# directly-pushed commit too (it has no passing checks recorded for its
# SHA), so without this bypass entry direct pushes to main would break.
#
# NOTE (2026-07-03): the actor_id=5 "admin" RepositoryRole mapping is not
# officially documented by GitHub (see
# https://github.com/github/rest-api-description/issues/4406) but is
# consistently reported by the community and third-party providers
# (Terraform/Pulumi GitHub providers). The live push-bypass verification
# described in issue #1333 / the PR description has NOT been performed as
# part of this change — see that verification note before relying on this.
#
# Idempotency: this script looks up the ruleset by name before writing. If
# "main-required-status-checks" already exists, it PUTs an update to that
# ruleset's id instead of POSTing a new one — re-running this script is safe
# and does not create duplicate rulesets or fail on a name collision.
#
# Usage:
#   scripts/apply-main-ruleset.sh            # create/update against the real repo
#   scripts/apply-main-ruleset.sh --dry-run  # print the JSON body only (no API calls)

REPO="Takazudo/zudo-front-builder"
RULESET_NAME="main-required-status-checks"

BODY=$(cat <<'JSON'
{
  "name": "main-required-status-checks",
  "target": "branch",
  "enforcement": "active",
  "conditions": {
    "ref_name": {
      "include": ["refs/heads/main"],
      "exclude": []
    }
  },
  "rules": [
    {
      "type": "required_status_checks",
      "parameters": {
        "required_status_checks": [
          { "context": "health" },
          { "context": "build (no-v8)" },
          { "context": "Build binary (x86_64-unknown-linux-gnu)" },
          { "context": "Build binary (aarch64-unknown-linux-gnu)" },
          { "context": "Smoke amd64 (local mode)" },
          { "context": "Smoke amd64 TS-config (local mode)" },
          { "context": "Smoke arm64 (local mode)" },
          { "context": "Smoke arm64 TS-config (local mode)" },
          { "context": "pnpm audit (prod)" }
        ],
        "strict_required_status_checks_policy": false
      }
    }
  ],
  "bypass_actors": [
    {
      "actor_id": 5,
      "actor_type": "RepositoryRole",
      "bypass_mode": "always"
    }
  ]
}
JSON
)

if [ "${1:-}" = "--dry-run" ]; then
  echo "$BODY"
  exit 0
fi

EXISTING_ID=$(gh api "repos/${REPO}/rulesets" --jq ".[] | select(.name == \"${RULESET_NAME}\") | .id" | head -n1)

if [ -n "$EXISTING_ID" ]; then
  echo "Ruleset '${RULESET_NAME}' already exists (id ${EXISTING_ID}) — updating in place." >&2
  echo "$BODY" | gh api --method PUT "repos/${REPO}/rulesets/${EXISTING_ID}" --input -
else
  echo "Ruleset '${RULESET_NAME}' not found — creating." >&2
  echo "$BODY" | gh api --method POST "repos/${REPO}/rulesets" --input -
fi
