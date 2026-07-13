#!/usr/bin/env bash
# Advance the npm `latest` dist-tag for all zfb workspace packages to the given version.
# Called from release.yml after a successful publish when ALSO_LATEST=1.
#
# Usage: bash scripts/advance-latest-dist-tag.sh <semver>
#   <semver>  — the version string to tag as latest (e.g. 1.2.3); passed as $1
#
# NODE_AUTH_TOKEN must be set in the environment (inherited from the calling
# GitHub Actions step env, which sets it via secrets.NPM_TOKEN).
set -euo pipefail

ZFB_SEMVER="${1:-}"
if [[ -z "$ZFB_SEMVER" ]]; then
  echo "::error::advance-latest-dist-tag.sh: missing required argument <semver>"
  exit 1
fi

echo "Dual-tag: advancing 'latest' to ${ZFB_SEMVER} for all packages."

_tag_with_retry() {
  local pkg="$1"
  local ver="$2"
  local max_attempts=5
  local delay=5
  for attempt in $(seq 1 $max_attempts); do
    if npm dist-tag add "${pkg}@${ver}" latest; then
      echo "  dist-tag add ${pkg}@${ver} latest — OK (attempt ${attempt})"
      return 0
    fi
    echo "  dist-tag add ${pkg}@${ver} latest — attempt ${attempt}/${max_attempts} failed; retrying in ${delay}s..."
    sleep "$delay"
    delay=$(( delay * 2 ))
  done
  echo "::error::dist-tag add failed for ${pkg}@${ver} after ${max_attempts} attempts."
  echo "::error::Manual remediation: npm dist-tag add ${pkg}@${ver} latest"
  return 1
}

FAILED=0
_tag_with_retry "@takazudo/zfb-darwin-arm64" "$ZFB_SEMVER"   || FAILED=1
_tag_with_retry "@takazudo/zfb-darwin-x64" "$ZFB_SEMVER"     || FAILED=1
_tag_with_retry "@takazudo/zfb-linux-arm64-gnu" "$ZFB_SEMVER" || FAILED=1
_tag_with_retry "@takazudo/zfb-linux-x64-gnu" "$ZFB_SEMVER"  || FAILED=1
_tag_with_retry "@takazudo/zfb-win32-x64-msvc" "$ZFB_SEMVER" || FAILED=1
_tag_with_retry "@takazudo/zfb" "$ZFB_SEMVER"                || FAILED=1
_tag_with_retry "@takazudo/zfb-runtime" "$ZFB_SEMVER"        || FAILED=1
_tag_with_retry "@takazudo/zfb-adapter-cloudflare" "$ZFB_SEMVER" || FAILED=1
_tag_with_retry "create-zfb" "$ZFB_SEMVER"                   || FAILED=1
_tag_with_retry "@takazudo/zfb-md-wasm" "$ZFB_SEMVER"        || FAILED=1

if [[ "$FAILED" -ne 0 ]]; then
  echo "::error::One or more 'npm dist-tag add ... latest' calls failed (see above). The packages are published but 'latest' was not advanced. Re-run the manual remediation commands listed above."
  exit 1
fi
echo "Dual-tag complete: all packages now have latest=${ZFB_SEMVER}."
