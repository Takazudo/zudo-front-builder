#!/usr/bin/env bash
set -euo pipefail
#
# apply-main-ruleset.sh — (re-)create the GitHub ruleset on `main` that
# codifies the always-run status checks as required-before-merge, checked in
# here for reproducibility (issue #1333).
#
# What this requires (only checks that run on EVERY PR, unconditionally —
# no path filters, no workflow_run/tag-only triggers):
#   - health                              (.github/workflows/health.yml)
#   - build (no-v8)                       (.github/workflows/health.yml)
#   - Build binary (ubuntu-22.04)         (.github/workflows/node-free-smoke.yml)
#   - Smoke amd64 (local mode)            (.github/workflows/node-free-smoke.yml)
#   - Smoke amd64 TS-config (local mode)  (.github/workflows/node-free-smoke.yml)
#   - Smoke arm64 (local mode)            (.github/workflows/node-free-smoke.yml)
#   - Smoke arm64 TS-config (local mode)  (.github/workflows/node-free-smoke.yml)
#   - pnpm audit (prod)                   (.github/workflows/pr-checks.yml)
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
# Usage:
#   scripts/apply-main-ruleset.sh            # create/update against the real repo
#   scripts/apply-main-ruleset.sh --dry-run  # print the JSON body only

REPO="Takazudo/zudo-front-builder"

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
          { "context": "Build binary (ubuntu-22.04)" },
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

echo "$BODY" | gh api --method POST "repos/${REPO}/rulesets" --input -
