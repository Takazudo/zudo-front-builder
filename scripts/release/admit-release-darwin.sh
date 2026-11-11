#!/usr/bin/env bash
# admit-release-darwin.sh — the release.yml `admit-darwin` job body (#4690).
#
# Downloads exactly the ten owner-Mac Darwin inputs (five per target) from the
# GitHub Release into a fresh directory and runs the admission verifier on them.
# Hosted macOS is never used: when an input is missing or rejected, the remedy
# is the owner's Mac (scripts/build-darwin-local.sh), printed below.
#
# Usage:
#   admit-release-darwin.sh --tag vX.Y.Z --version X.Y.Z --source-commit SHA \
#     --source-tree TREE --repo-root SOURCE_CHECKOUT --out DIR [--control-dir SCRIPTS_DIR]
#
# --control-dir is the scripts/ directory holding admit-darwin-artifacts.mjs and
# darwin-artifact-contract.mjs (release.yml passes the control checkout, since a
# recovery tag predates them). Defaults to this script's parent scripts/ dir.
#
# Writes DIR/inputs/<the ten assets> and DIR/verdict.json.
# Exit 0 admitted, 1 missing or rejected, 2 usage or download error.
set -euo pipefail

usage() {
  echo "usage: $0 --tag vX.Y.Z --version X.Y.Z --source-commit SHA --source-tree TREE --repo-root DIR --out DIR [--control-dir DIR]" >&2
  exit 2
}

TAG="" VERSION="" SOURCE_COMMIT="" SOURCE_TREE="" REPO_ROOT="" OUT="" CONTROL_DIR=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag) TAG="${2:-}"; shift 2 ;;
    --version) VERSION="${2:-}"; shift 2 ;;
    --source-commit) SOURCE_COMMIT="${2:-}"; shift 2 ;;
    --source-tree) SOURCE_TREE="${2:-}"; shift 2 ;;
    --repo-root) REPO_ROOT="${2:-}"; shift 2 ;;
    --out) OUT="${2:-}"; shift 2 ;;
    --control-dir) CONTROL_DIR="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$TAG" && -n "$VERSION" && -n "$SOURCE_COMMIT" && -n "$SOURCE_TREE" && -n "$REPO_ROOT" && -n "$OUT" ]] || usage
[[ "$TAG" == "v${VERSION}" ]] || { echo "::error::tag ${TAG} does not match version ${VERSION}" >&2; exit 2; }
if [[ -z "$CONTROL_DIR" ]]; then
  CONTROL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi

remedy() {
  echo "::error::Darwin release inputs for ${TAG} are $1. Hosted macOS is not used for releases."
  echo "::error::On the owner's Apple Silicon Mac, at a clean checkout of ${SOURCE_COMMIT}, run:"
  echo "::error::  scripts/build-darwin-local.sh --source ${SOURCE_COMMIT} --tag ${TAG} --upload ${TAG}"
  echo "::error::(the Release must still be a draft for --upload), then publish the Release or re-run this workflow."
}

if [[ -e "$OUT" ]] && [[ -n "$(ls -A "$OUT" 2>/dev/null)" ]]; then
  echo "::error::--out ${OUT} must be absent or empty (fresh admission directory)." >&2
  exit 2
fi
INPUTS="$OUT/inputs"
mkdir -p "$INPUTS"

EXPECTED=()
for TRIPLE in aarch64-apple-darwin x86_64-apple-darwin; do
  while IFS= read -r NAME; do
    [[ -n "$NAME" ]] && EXPECTED+=("$NAME")
  done < <(node "$CONTROL_DIR/darwin-artifact-contract.mjs" asset-names --version "$VERSION" --target "$TRIPLE")
done
if [[ ${#EXPECTED[@]} -ne 10 ]]; then
  echo "::error::expected 10 Darwin input names, contract produced ${#EXPECTED[@]}" >&2
  exit 2
fi

if ! ASSETS=$(gh release view "$TAG" --json assets --jq '.assets[].name'); then
  echo "::error::cannot read the assets of GitHub Release ${TAG}." >&2
  exit 2
fi

MISSING=()
for NAME in "${EXPECTED[@]}"; do
  grep -Fxq -- "$NAME" <<<"$ASSETS" || MISSING+=("$NAME")
done
if [[ ${#MISSING[@]} -gt 0 ]]; then
  for NAME in "${MISSING[@]}"; do echo "::error::missing Darwin input: ${NAME}"; done
  remedy "missing (${#MISSING[@]} of 10)"
  exit 1
fi

PATTERNS=()
for NAME in "${EXPECTED[@]}"; do PATTERNS+=(--pattern "$NAME"); done
if ! gh release download "$TAG" "${PATTERNS[@]}" --dir "$INPUTS"; then
  echo "::error::downloading the Darwin inputs of ${TAG} failed." >&2
  exit 2
fi

# Exactly the ten names, nothing else (a glob pattern must not widen the set).
GOT=$(cd "$INPUTS" && find . -mindepth 1 -maxdepth 1 | sed 's|^\./||' | LC_ALL=C sort)
WANT=$(printf '%s\n' "${EXPECTED[@]}" | LC_ALL=C sort)
if [[ "$GOT" != "$WANT" ]]; then
  echo "::error::downloaded Darwin input set differs from the expected ten:" >&2
  diff <(echo "$WANT") <(echo "$GOT") >&2 || true
  exit 2
fi

set +e
node "$CONTROL_DIR/admit-darwin-artifacts.mjs" --dir "$INPUTS" --version "$VERSION" \
  --source-commit "$SOURCE_COMMIT" --source-tree "$SOURCE_TREE" --repo-root "$REPO_ROOT" \
  --json-out "$OUT/verdict.json" >/dev/null
STATUS=$?
set -e
if [[ "$STATUS" -eq 0 ]]; then
  echo "Darwin inputs for ${TAG} admitted (source ${SOURCE_COMMIT}, tree ${SOURCE_TREE})."
  exit 0
fi
if [[ "$STATUS" -eq 1 ]]; then
  remedy "rejected by scripts/admit-darwin-artifacts.mjs (see REJECT lines above and verdict.json)"
  exit 1
fi
echo "::error::admission verifier failed with exit ${STATUS} (usage or I/O error)." >&2
exit 2
