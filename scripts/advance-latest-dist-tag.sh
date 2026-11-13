#!/usr/bin/env bash
# Advance npm channel dist-tags (`latest`, `next`) for all zfb workspace packages
# to the given version. Called by scripts/promote-release-channel.sh after every
# package is present-verified (#4717); never from the publish loop.
#
# Usage: bash scripts/advance-latest-dist-tag.sh [--tag latest|next]... <semver>
#   --tag     a channel to move; repeatable, applied in the order given.
#             Default: latest only (the original single-tag behavior).
#   <semver>  the version string to tag (e.g. 1.2.3)
#
# Auth: GitHub OIDC trusted publishing only, through npm >= 12.2.0 (the first
# npm whose OIDC exchange covers `dist-tag add`; the trusted publisher must also
# have "Allow npm dist-tag" enabled). No stored token is read or accepted —
# scripts/npm-oidc-preflight.mjs refuses to run otherwise.
#
# Two phases, so a guard refusal can never leave a half-moved channel tag:
#   1. plan  — read every requested tag of every package and apply its guards.
#              Any refusal or unreadable tag stops here with NOTHING written.
#   2. write — read-compare-write per package with retry: re-read the tag,
#              re-apply the guards on that fresh state, `dist-tag add`, and
#              re-read to confirm it landed.
# npm has no compare-and-swap for dist-tags and no multi-package transaction, so
# a write that keeps failing (or races another writer) after the plan passed can
# still leave the channel split; that is reported as an error and is repaired by
# re-running, because every step is idempotent.
#
# Guards:
#   latest  never moves to a LOWER version (semver precedence), and never from a
#           stable version to a prerelease (#4690).
#   next    only to a `*-next.*`, `*-beta.*` or `*-rc.*` prerelease that is
#           strictly greater than the current `next` (or when `next` is absent);
#           never backwards, never to a stable version (retire-next-dist-tag.mjs
#           handles graduation).
#
# Env (tests): ADVANCE_LATEST_RETRY_DELAY  initial backoff seconds (default 5).
#              ZFB_NPM_OIDC_PREFLIGHT_NODE  node binary for the preflight (an
#              absolute path keeps version-manager shims from re-ordering PATH
#              ahead of the test's npm stub).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Every published package. Must match PUBLISHED_PACKAGES in
# retire-next-dist-tag.mjs (drift guard: scripts/__tests__/retire-next-dist-tag.test.mjs).
PACKAGES=(
  "@takazudo/zfb-darwin-arm64"
  "@takazudo/zfb-darwin-x64"
  "@takazudo/zfb-linux-arm64-gnu"
  "@takazudo/zfb-linux-x64-gnu"
  "@takazudo/zfb-win32-x64-msvc"
  "@takazudo/zfb-slugify"
  "@takazudo/zfb"
  "@takazudo/zfb-runtime"
  "@takazudo/zfb-adapter-cloudflare"
  "create-zfb"
  "@takazudo/zfb-md-wasm"
)

PRERELEASE_CHANNEL_RE='-(next|beta|rc)\.'

usage() {
  echo "::error::usage: advance-latest-dist-tag.sh [--tag latest|next]... <semver>"
  exit 1
}

TAGS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag)
      [[ $# -ge 2 ]] || usage
      TAGS+=("$2")
      shift 2
      ;;
    -*) usage ;;
    *) break ;;
  esac
done
[[ $# -eq 1 ]] || usage
ZFB_SEMVER="$1"
if [[ -z "$ZFB_SEMVER" ]]; then
  echo "::error::advance-latest-dist-tag.sh: missing required argument <semver>"
  exit 1
fi
if [[ ${#TAGS[@]} -eq 0 ]]; then
  TAGS=(latest)
fi
SEEN_TAGS=" "
for tag in "${TAGS[@]}"; do
  case "$tag" in
    latest | next) ;;
    *)
      echo "::error::advance-latest-dist-tag.sh: unsupported channel tag '${tag}' (latest or next only)."
      exit 1
      ;;
  esac
  if [[ "$SEEN_TAGS" == *" ${tag} "* ]]; then
    echo "::error::advance-latest-dist-tag.sh: channel tag '${tag}' given twice."
    exit 1
  fi
  SEEN_TAGS="${SEEN_TAGS}${tag} "
  if [[ "$tag" == next && ! "$ZFB_SEMVER" =~ $PRERELEASE_CHANNEL_RE ]]; then
    echo "::error::advance-latest-dist-tag.sh: refusing to point 'next' at ${ZFB_SEMVER}: 'next' only carries *-next.*, *-beta.* or *-rc.* prereleases. No dist-tag was moved."
    exit 1
  fi
done

if ! "${ZFB_NPM_OIDC_PREFLIGHT_NODE:-node}" "${SCRIPT_DIR}/npm-oidc-preflight.mjs"; then
  echo "::error::advance-latest-dist-tag.sh: npm OIDC preflight refused; no dist-tag was touched."
  exit 1
fi

echo "Channel promotion: moving ${TAGS[*]} to ${ZFB_SEMVER} for all ${#PACKAGES[@]} packages."

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

# Echo the package's current <tag> ("" when the tag is absent); non-zero exit on
# a registry/network failure so absence is never confused with error.
_read_tag() {
  npm view "$1" "dist-tags.$2" --prefer-online
}

# _guard <pkg> <tag> <ver> <current> → 0 move, 2 already there, 1 refuse.
_guard() {
  local pkg="$1" tag="$2" ver="$3" current="$4" cmp
  if [[ "$current" == "$ver" ]]; then
    return 2
  fi
  if [[ -z "$current" ]]; then
    return 0
  fi
  if ! cmp="$(_semver_cmp "$ver" "$current")"; then
    echo "::error::${pkg}: cannot compare ${ver} with current ${tag} '${current}' — refusing to move '${tag}' on a guess."
    return 1
  fi
  if [[ "$cmp" -lt 0 ]]; then
    echo "::error::${pkg}: current ${tag}=${current} is newer than ${ver} — refusing to move '${tag}' backwards."
    return 1
  fi
  case "$tag" in
    latest)
      # The dual-tag phase ends at the first stable `latest`; a prerelease
      # must never replace it, even a numerically higher one (#4690).
      if [[ "$current" != *-* && "$ver" == *-* ]]; then
        echo "::error::${pkg}: current latest=${current} is a stable release — refusing to move 'latest' to the prerelease ${ver}."
        return 1
      fi
      ;;
    next)
      if [[ "$cmp" -le 0 ]]; then
        echo "::error::${pkg}: current next=${current} is not lower than ${ver} — 'next' only moves strictly forward."
        return 1
      fi
      ;;
  esac
  return 0
}

# Phase 1: read + guard every (tag, package) pair without writing anything.
# Appends "<tag> <pkg>" to PLANNED for each pair that needs a write.
PLANNED=()
_plan_one() {
  local pkg="$1" tag="$2" ver="$3"
  local max_attempts=5
  local delay="${ADVANCE_LATEST_RETRY_DELAY:-5}"
  local attempt current rc
  for attempt in $(seq 1 $max_attempts); do
    if current="$(_read_tag "$pkg" "$tag")"; then
      current="${current//[[:space:]]/}"
      rc=0
      _guard "$pkg" "$tag" "$ver" "$current" || rc=$?
      case "$rc" in
        0)
          echo "  plan: ${pkg}: ${tag} ${current:-<absent>} → ${ver}"
          PLANNED+=("${tag} ${pkg}")
          return 0
          ;;
        2)
          echo "  plan: ${pkg}: ${tag} is already ${ver} — nothing to do"
          return 0
          ;;
        *) return 1 ;;
      esac
    fi
    echo "  ${pkg}: could not read dist-tags.${tag} (registry error, attempt ${attempt}/${max_attempts})."
    if [[ "$attempt" -lt "$max_attempts" ]]; then
      sleep "$delay"
      delay=$((delay * 2))
    fi
  done
  echo "::error::${pkg}: dist-tags.${tag} unreadable after ${max_attempts} attempts."
  return 1
}

_tag_with_retry() {
  local pkg="$1"
  local ver="$2"
  local tag="$3"
  local max_attempts=5
  local delay="${ADVANCE_LATEST_RETRY_DELAY:-5}"
  local attempt current after rc
  for attempt in $(seq 1 $max_attempts); do
    if current="$(_read_tag "$pkg" "$tag")"; then
      current="${current//[[:space:]]/}"
      rc=0
      _guard "$pkg" "$tag" "$ver" "$current" || rc=$?
      if [[ "$rc" -eq 2 ]]; then
        echo "  ${pkg}: ${tag} is already ${ver} — nothing to do (attempt ${attempt})"
        return 0
      fi
      if [[ "$rc" -ne 0 ]]; then
        return 1
      fi
      if npm dist-tag add "${pkg}@${ver}" "$tag"; then
        if after="$(_read_tag "$pkg" "$tag")" && [[ "${after//[[:space:]]/}" == "$ver" ]]; then
          echo "  dist-tag add ${pkg}@${ver} ${tag} — OK, confirmed by re-read (attempt ${attempt})"
          return 0
        fi
        echo "  ${pkg}: dist-tag add reported success but a re-read shows ${tag}='${after:-<unreadable>}' (propagation lag or a concurrent change)."
      fi
    else
      echo "  ${pkg}: could not read current dist-tags (registry error)."
    fi
    if [[ "$attempt" -lt "$max_attempts" ]]; then
      echo "  ${pkg}: attempt ${attempt}/${max_attempts} failed; retrying in ${delay}s..."
      sleep "$delay"
      delay=$((delay * 2))
    fi
  done
  echo "::error::advancing ${tag} for ${pkg}@${ver} failed after ${max_attempts} attempts."
  echo "::error::Manual remediation (maintainer's own interactive npm session): npm dist-tag add ${pkg}@${ver} ${tag}"
  return 1
}

PLAN_FAILED=0
for tag in "${TAGS[@]}"; do
  for pkg in "${PACKAGES[@]}"; do
    _plan_one "$pkg" "$tag" "$ZFB_SEMVER" || PLAN_FAILED=1
  done
done
if [[ "$PLAN_FAILED" -ne 0 ]]; then
  echo "::error::Channel promotion refused before any write (see above): no dist-tag was moved, so no channel is half-moved."
  exit 1
fi

FAILED=0
for entry in "${PLANNED[@]+"${PLANNED[@]}"}"; do
  tag="${entry%% *}"
  pkg="${entry#* }"
  _tag_with_retry "$pkg" "$ZFB_SEMVER" "$tag" || FAILED=1
done

if [[ "$FAILED" -ne 0 ]]; then
  echo "::error::One or more packages did not get ${TAGS[*]}=${ZFB_SEMVER} (see above). The packages are published but the channel tag is split across packages; re-run the promotion (release.yml finalize_only) to complete it — every step is idempotent."
  exit 1
fi
echo "Channel promotion complete: all packages now have ${TAGS[*]}=${ZFB_SEMVER} (each confirmed by re-read)."
