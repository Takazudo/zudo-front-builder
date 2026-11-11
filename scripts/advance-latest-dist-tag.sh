#!/usr/bin/env bash
# Advance the npm `latest` dist-tag for all zfb workspace packages to the given version.
# Called from scripts/publish-npm-packages.sh after a successful publish when ALSO_LATEST=1.
#
# Usage: bash scripts/advance-latest-dist-tag.sh <semver>
#   <semver>  — the version string to tag as latest (e.g. 1.2.3); passed as $1
#
# Auth: GitHub OIDC trusted publishing only, through npm >= 12.2.0 (the first
# npm whose OIDC exchange covers `dist-tag add`; the trusted publisher must also
# have "Allow npm dist-tag" enabled). No stored token is read or accepted —
# scripts/npm-oidc-preflight.mjs refuses to run otherwise.
#
# Read-compare-write per package: read the current `latest`, refuse to move it
# to a LOWER version (semver precedence), skip when it already points at
# <semver>, and re-read after the write to confirm it landed. npm has no
# compare-and-swap for dist-tags, so a write racing another writer between the
# read and the add cannot be prevented — the post-write re-read detects it.
#
# Env (tests): ADVANCE_LATEST_RETRY_DELAY  initial backoff seconds (default 5).
#              ZFB_NPM_OIDC_PREFLIGHT_NODE  node binary for the preflight (an
#              absolute path keeps version-manager shims from re-ordering PATH
#              ahead of the test's npm stub).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ZFB_SEMVER="${1:-}"
if [[ -z "$ZFB_SEMVER" ]]; then
  echo "::error::advance-latest-dist-tag.sh: missing required argument <semver>"
  exit 1
fi

if ! "${ZFB_NPM_OIDC_PREFLIGHT_NODE:-node}" "${SCRIPT_DIR}/npm-oidc-preflight.mjs"; then
  echo "::error::advance-latest-dist-tag.sh: npm OIDC preflight refused; 'latest' was not touched."
  exit 1
fi

echo "Dual-tag: advancing 'latest' to ${ZFB_SEMVER} for all packages."

# Echo -1/0/1 for semver precedence of $1 vs $2 (same implementation as
# retire-next-dist-tag.mjs); exits non-zero on an unparseable version.
# The module path travels in the environment, NOT argv: with `node -e`,
# process.argv[1] is the first argument, and retire-next-dist-tag.mjs runs its
# CLI (a live sweep) when argv[1] names it.
_semver_cmp() {
  ZFB_SEMVER_MODULE="${SCRIPT_DIR}/retire-next-dist-tag.mjs" node --input-type=module -e '
    import { pathToFileURL } from "node:url";
    const [a, b] = process.argv.slice(1);
    const { compareSemver } = await import(pathToFileURL(process.env.ZFB_SEMVER_MODULE).href);
    console.log(compareSemver(a, b));
  ' "$1" "$2"
}

# Echo the package's current `latest` ("" when the tag is absent); non-zero
# exit on a registry/network failure so absence is never confused with error.
_read_latest() {
  npm view "$1" dist-tags.latest --prefer-online
}

_tag_with_retry() {
  local pkg="$1"
  local ver="$2"
  local max_attempts=5
  local delay="${ADVANCE_LATEST_RETRY_DELAY:-5}"
  local attempt current after cmp
  for attempt in $(seq 1 $max_attempts); do
    if current="$(_read_latest "$pkg")"; then
      current="${current//[[:space:]]/}"
      if [[ "$current" == "$ver" ]]; then
        echo "  ${pkg}: latest is already ${ver} — nothing to do (attempt ${attempt})"
        return 0
      fi
      if [[ -n "$current" ]]; then
        if ! cmp="$(_semver_cmp "$ver" "$current")"; then
          echo "::error::${pkg}: cannot compare ${ver} with current latest '${current}' — refusing to move 'latest' on a guess."
          return 1
        fi
        if [[ "$cmp" -lt 0 ]]; then
          echo "::error::${pkg}: current latest=${current} is newer than ${ver} — refusing to move 'latest' backwards."
          return 1
        fi
      fi
      if npm dist-tag add "${pkg}@${ver}" latest; then
        if after="$(_read_latest "$pkg")" && [[ "${after//[[:space:]]/}" == "$ver" ]]; then
          echo "  dist-tag add ${pkg}@${ver} latest — OK, confirmed by re-read (attempt ${attempt})"
          return 0
        fi
        echo "  ${pkg}: dist-tag add reported success but a re-read shows latest='${after:-<unreadable>}' (propagation lag or a concurrent change)."
      fi
    else
      echo "  ${pkg}: could not read current dist-tags (registry error)."
    fi
    if [[ "$attempt" -lt "$max_attempts" ]]; then
      echo "  ${pkg}: attempt ${attempt}/${max_attempts} failed; retrying in ${delay}s..."
      sleep "$delay"
      delay=$(( delay * 2 ))
    fi
  done
  echo "::error::advancing latest for ${pkg}@${ver} failed after ${max_attempts} attempts."
  echo "::error::Manual remediation (maintainer's own interactive npm session): npm dist-tag add ${pkg}@${ver} latest"
  return 1
}

FAILED=0
_tag_with_retry "@takazudo/zfb-darwin-arm64" "$ZFB_SEMVER"   || FAILED=1
_tag_with_retry "@takazudo/zfb-darwin-x64" "$ZFB_SEMVER"     || FAILED=1
_tag_with_retry "@takazudo/zfb-linux-arm64-gnu" "$ZFB_SEMVER" || FAILED=1
_tag_with_retry "@takazudo/zfb-linux-x64-gnu" "$ZFB_SEMVER"  || FAILED=1
_tag_with_retry "@takazudo/zfb-win32-x64-msvc" "$ZFB_SEMVER" || FAILED=1
_tag_with_retry "@takazudo/zfb-slugify" "$ZFB_SEMVER"    || FAILED=1
_tag_with_retry "@takazudo/zfb" "$ZFB_SEMVER"                || FAILED=1
_tag_with_retry "@takazudo/zfb-runtime" "$ZFB_SEMVER"        || FAILED=1
_tag_with_retry "@takazudo/zfb-adapter-cloudflare" "$ZFB_SEMVER" || FAILED=1
_tag_with_retry "create-zfb" "$ZFB_SEMVER"                   || FAILED=1
_tag_with_retry "@takazudo/zfb-md-wasm" "$ZFB_SEMVER"        || FAILED=1

if [[ "$FAILED" -ne 0 ]]; then
  echo "::error::One or more packages did not get latest=${ZFB_SEMVER} (see above). The packages are published but 'latest' was not advanced everywhere."
  exit 1
fi
echo "Dual-tag complete: all packages now have latest=${ZFB_SEMVER} (each confirmed by re-read)."
