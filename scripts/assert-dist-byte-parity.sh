#!/usr/bin/env bash
set -euo pipefail

# scripts/assert-dist-byte-parity.sh
#
# Asserts that two built dist/ trees are byte-identical: the same set of
# relative file paths, and the same raw SHA-256 for every file. Used by
# scripts/smoke-packed-clean-room.sh (Stage 7) to prove that a site built
# against workspace-linked packages and the same site built against packed
# tarballs emit the same bytes (issue #4231: the islands root must be
# content-addressable, so consumption mode must not leak into it).
#
# Raw bytes only — never the island-size normalizer, which deliberately
# erases the build token and so cannot prove token equality.
#
# Usage:
#   scripts/assert-dist-byte-parity.sh [--require <glob>]... <dist-a> <dist-b>
#
# --require <glob>  a path glob, relative to the dist root (matched with
#                   bash `[[ == ]]`, so `*` also crosses `/`), that must match
#                   at least one file in <dist-a>. Guards against a vacuous
#                   pass, e.g. two builds that both silently emitted no
#                   islands bundle. Repeatable.
#
# Exit status: 0 identical; 1 a difference or a missing required file;
# 2 usage error.

usage() {
  echo "usage: $0 [--require <glob>]... <dist-a> <dist-b>" >&2
  exit 2
}

REQUIRED_GLOBS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --require)
      [[ $# -ge 2 ]] || usage
      REQUIRED_GLOBS+=("$2")
      shift 2
      ;;
    --) shift; break ;;
    -*) usage ;;
    *) break ;;
  esac
done
[[ $# -eq 2 ]] || usage

DIST_A="$1"
DIST_B="$2"
for d in "$DIST_A" "$DIST_B"; do
  if [[ ! -d "$d" ]]; then
    echo "[FAIL] not a directory: $d" >&2
    exit 1
  fi
done

# One "<sha256>  <relative path>" line per regular file, sorted bytewise so
# the manifest order cannot depend on the host locale. Symlinks and other
# non-regular entries are listed by type so a file-vs-symlink swap is a
# difference too.
manifest() {
  (
    cd "$1"
    find . -mindepth 1 \( -type f -o -type l \) -print0 | LC_ALL=C sort -z |
      while IFS= read -r -d '' path; do
        rel="${path#./}"
        if [[ -L "$path" ]]; then
          printf 'symlink:%s  %s\n' "$(readlink "$path")" "$rel"
        else
          printf '%s  %s\n' "$(sha256sum "$path" | cut -d' ' -f1)" "$rel"
        fi
      done
  )
}

MANIFEST_A="$(manifest "$DIST_A")"
MANIFEST_B="$(manifest "$DIST_B")"

if [[ -z "$MANIFEST_A" ]]; then
  echo "[FAIL] $DIST_A contains no files" >&2
  exit 1
fi

for glob in ${REQUIRED_GLOBS[@]+"${REQUIRED_GLOBS[@]}"}; do
  found=0
  while IFS= read -r line; do
    rel="${line#*  }"
    # shellcheck disable=SC2053 # $glob is intentionally a pattern
    if [[ "$rel" == $glob ]]; then
      found=1
      break
    fi
  done <<<"$MANIFEST_A"
  if [[ $found -eq 0 ]]; then
    echo "[FAIL] no file in $DIST_A matches required glob: $glob" >&2
    exit 1
  fi
done

FILE_COUNT="$(printf '%s\n' "$MANIFEST_A" | wc -l | tr -d ' ')"

if [[ "$MANIFEST_A" == "$MANIFEST_B" ]]; then
  printf '[PASS] %s and %s are byte-identical (%s files)\n' "$DIST_A" "$DIST_B" "$FILE_COUNT"
  printf '%s\n' "$MANIFEST_A" | grep -E '  assets/islands-[^/]*\.js$' | sed 's/^/  /' || true
  exit 0
fi

echo "[FAIL] $DIST_A and $DIST_B differ (manifest diff, '<' = first, '>' = second):" >&2
diff <(printf '%s\n' "$MANIFEST_A") <(printf '%s\n' "$MANIFEST_B") >&2 || true
exit 1
