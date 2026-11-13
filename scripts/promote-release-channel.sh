#!/usr/bin/env bash
# Post-verification channel promotion (#4717): the ONLY place release.yml moves
# `latest` / `next`. Every package is published under the `release-pending`
# holding tag (scripts/publish-npm-packages.sh), so a package that npm keeps in
# pre-publication processing can never leave a half-moved channel tag.
#
# Usage: bash scripts/promote-release-channel.sh <version>
#
# 1. Gate: all 11 packages must be present-verified for the release source
#    commit (scripts/release/npm-package-state.mjs --require-all-verified).
#    A `processing`, absent, unverified or uncertain package refuses the whole
#    promotion and leaves every channel tag untouched.
# 2. Channel: stable → `latest`; `*-next.*` / `*-beta.*` / `*-rc.*` → `next`,
#    plus `latest` when ALSO_LATEST=1 (prerelease dual-tag, #481).
# 3. Move: scripts/advance-latest-dist-tag.sh plans every (tag, package) pair
#    before its first write, so a guard refusal writes nothing.
#
# Env:
#   DIST_TAG                     next | latest (release.yml "Determine dist-tag");
#                                must agree with <version>.
#   ALSO_LATEST                  "1" → also move `latest` for a prerelease.
#   ZFB_RELEASE_SOURCE_COMMIT    40-hex release source commit.
#   ZFB_DARWIN_ADMISSION_VERDICT optional admission verdict JSON for the
#                                Darwin carriers' identity check.
#   ZFB_NPM_PACKAGE_STATE_NODE   node binary for npm-package-state.mjs (tests).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PRERELEASE_CHANNEL_RE='-(next|beta|rc)\.'
EXPECTED_PACKAGE_COUNT=11

VERSION="${1:-}"
if [[ -z "$VERSION" || $# -ne 1 ]]; then
  echo "::error::usage: promote-release-channel.sh <version>"
  exit 1
fi

if [[ "$VERSION" =~ $PRERELEASE_CHANNEL_RE ]]; then
  CHANNEL=next
else
  CHANNEL=latest
fi
if [[ "${DIST_TAG:-}" != "$CHANNEL" ]]; then
  echo "::error::promote-release-channel.sh: DIST_TAG='${DIST_TAG:-}' does not match ${VERSION} (expected '${CHANNEL}'). No dist-tag was moved."
  exit 1
fi
if [[ ! "${ZFB_RELEASE_SOURCE_COMMIT:-}" =~ ^[0-9a-f]{40}$ ]]; then
  echo "::error::promote-release-channel.sh: ZFB_RELEASE_SOURCE_COMMIT must be the 40-hex release source commit (got '${ZFB_RELEASE_SOURCE_COMMIT:-}'). No dist-tag was moved."
  exit 1
fi

TAG_ARGS=(--tag "$CHANNEL")
if [[ "$CHANNEL" == next && "${ALSO_LATEST:-}" == "1" ]]; then
  TAG_ARGS+=(--tag latest)
fi

echo "== Channel promotion gate: every package present-verified for ${VERSION} @ ${ZFB_RELEASE_SOURCE_COMMIT} =="
STATE_ARGS=(--version "$VERSION" --source-commit "$ZFB_RELEASE_SOURCE_COMMIT" --require-all-verified --tsv)
if [[ -n "${ZFB_DARWIN_ADMISSION_VERDICT:-}" ]]; then
  STATE_ARGS+=(--admission "$ZFB_DARWIN_ADMISSION_VERDICT")
fi
STATE_RC=0
STATES="$("${ZFB_NPM_PACKAGE_STATE_NODE:-node}" "${SCRIPT_DIR}/release/npm-package-state.mjs" "${STATE_ARGS[@]}")" || STATE_RC=$?
printf '%s\n' "$STATES" | sed 's/^/  | /'

TOTAL="$(printf '%s\n' "$STATES" | grep -c . || true)"
VERIFIED="$(printf '%s\n' "$STATES" | awk -F '\t' '$2 == "present-verified"' | grep -c . || true)"
if [[ "$STATE_RC" -ne 0 || "$TOTAL" -ne "$EXPECTED_PACKAGE_COUNT" || "$VERIFIED" -ne "$EXPECTED_PACKAGE_COUNT" ]]; then
  PROCESSING="$(printf '%s\n' "$STATES" | awk -F '\t' '$2 == "processing" { print $1 }' | paste -sd, - || true)"
  if [[ -n "$PROCESSING" ]]; then
    echo "::error title=npm still processing::${PROCESSING} not yet visible on npm. '${CHANNEL}' was NOT moved on any package; it moves for all ${EXPECTED_PACKAGE_COUNT} together once a finalize_only run sees every package present-verified."
  fi
  echo "::error::Channel promotion refused: ${VERIFIED}/${EXPECTED_PACKAGE_COUNT} packages present-verified (state helper exit ${STATE_RC}, ${TOTAL} rows). No dist-tag was moved."
  exit 1
fi

bash "${SCRIPT_DIR}/advance-latest-dist-tag.sh" "${TAG_ARGS[@]}" "$VERSION"
