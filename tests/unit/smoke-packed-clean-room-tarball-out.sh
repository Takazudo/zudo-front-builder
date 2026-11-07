#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/smoke-packed-clean-room-tarball-out.sh — offline tests for the
# opt-in ZFB_SMOKE_TARBALL_OUT export in scripts/smoke-packed-clean-room.sh
# (issue #4636), which hands the packed tarballs to the non-required
# esbuild-free artifact job. Same technique as
# tests/unit/smoke-packed-clean-room-dist-out.sh: static pins on the script
# text plus executing the REAL export block, extracted from the script.
#
# Requires: sh, bash, awk, mktemp, grep. No network.
#
# Run:
#   sh tests/unit/smoke-packed-clean-room-tarball-out.sh

set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
SCRIPT="$REPO_ROOT/scripts/smoke-packed-clean-room.sh"

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

# The export runs only after every assertion: below the final success line.
DONE_LINE=$(grep -n 'All packed-clean-room smoke assertions passed' "$SCRIPT" | head -1 | cut -d: -f1)
FIRST_USE=$(grep -n 'ZFB_SMOKE_TARBALL_OUT' "$SCRIPT" | head -1 | cut -d: -f1)
if [ -n "$DONE_LINE" ] && [ -n "$FIRST_USE" ] && [ "$FIRST_USE" -gt "$DONE_LINE" ]; then
  pass "ZFB_SMOKE_TARBALL_OUT first appears after the final assertion (line $FIRST_USE > $DONE_LINE)"
else
  fail "ZFB_SMOKE_TARBALL_OUT must only appear after the final assertion (first use ${FIRST_USE:-none}, success line ${DONE_LINE:-none})"
fi

if sed 's/[[:space:]]*#.*$//' "$SCRIPT" | grep -q 'ZFB_SMOKE_TARBALL_OUT:-'; then
  pass "the export guard uses the \${ZFB_SMOKE_TARBALL_OUT:-} form"
else
  fail "no ZFB_SMOKE_TARBALL_OUT:- guard in the code — a bare reference aborts under set -u"
fi

EXPORT_BLOCK=$(awk '
  /ZFB_SMOKE_TARBALL_OUT:-/ { grab = 1 }
  grab { print }
  grab && $0 ~ /^fi$/ { exit }
' "$SCRIPT")

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

if [ -z "$EXPORT_BLOCK" ]; then
  fail "could not extract the ZFB_SMOKE_TARBALL_OUT export block"
else
  RUNNER="$WORK/run-export-block.sh"
  {
    printf '#!/bin/bash\nset -euo pipefail\npass() { :; }\n'
    printf '%s\n' "$EXPORT_BLOCK"
  } >"$RUNNER"
  mkdir -p "$WORK/tarballs"
  printf 'a' >"$WORK/tarballs/takazudo-zfb-0.0.0.tgz"
  printf 'b' >"$WORK/tarballs/create-zfb-0.0.0.tgz"

  if TARBALL_DIR="$WORK/tarballs" ZFB_SMOKE_TARBALL_OUT="$WORK/out/tarballs" bash "$RUNNER" &&
    [ -f "$WORK/out/tarballs/takazudo-zfb-0.0.0.tgz" ] && [ -f "$WORK/out/tarballs/create-zfb-0.0.0.tgz" ]; then
    pass "positive copy: the real export block copies every tarball"
  else
    fail "positive copy: the real export block did not copy the tarballs"
  fi

  if UNSET_OUT=$(TARBALL_DIR="$WORK/tarballs" env -u ZFB_SMOKE_TARBALL_OUT bash "$RUNNER" 2>&1) && [ -z "$UNSET_OUT" ]; then
    pass "negative path: unset ZFB_SMOKE_TARBALL_OUT is a silent no-op under set -u"
  else
    fail "negative path: unset export block failed or printed: $UNSET_OUT"
  fi

  if FAIL_OUT=$(TARBALL_DIR="$WORK/no-such-dir" ZFB_SMOKE_TARBALL_OUT="$WORK/out2" bash "$RUNNER" 2>&1); then
    case "$FAIL_OUT" in
      *"::warning::failed to export packed tarballs"*) pass "failed copy: warns and still exits 0 (non-fatal)" ;;
      *) fail "failed copy: exited 0 without the warning: $FAIL_OUT" ;;
    esac
  else
    fail "failed copy: the export block exited non-zero, which would redden Scaffold E2E"
  fi
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
