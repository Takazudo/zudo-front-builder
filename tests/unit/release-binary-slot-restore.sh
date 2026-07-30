#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/release-binary-slot-restore.sh — offline unit tests for
# scripts/release-binary-slot-restore.sh, the save/restore helpers
# scripts/build-macos-x64-local.sh uses to keep the shared, arch-unqualified
# vendor binary slots (crates/zfb/binaries/esbuild/esbuild,
# crates/zfb/binaries/tailwindcss-v4) byte- and permission-identical to their
# pre-cross-build state (issue #2189, fixing the pollution source behind
# #2178).
#
# Runs entirely offline against throwaway fixture trees under mktemp -d — it
# never touches the real crates/zfb/binaries/ slots and never invokes cargo.
# The library under test is bash (arrays, [[ ]], local), so each case is
# driven through `bash -c` sourcing the library directly, mirroring
# tests/unit/publish-npm-packages.sh's sourcing technique. The library has no
# top-level executable code, so sourcing it is always safe.
#
# Requires: sh, bash, mktemp, cp, chmod. No network.
#
# Run:
#   sh tests/unit/release-binary-slot-restore.sh

set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)

LIB="$REPO_ROOT/scripts/release-binary-slot-restore.sh"

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

if [ ! -f "$LIB" ]; then
  fail "library not found: $LIB"
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

# make_fixture <dir> — creates the two slot paths (arm64 placeholder content,
# mode 0755) under a fresh throwaway "repo root" directory.
make_fixture() {
  fixture_dir="$1"
  mkdir -p "$fixture_dir/crates/zfb/binaries/esbuild"
  printf 'arm64-esbuild-fixture-bytes' >"$fixture_dir/crates/zfb/binaries/esbuild/esbuild"
  chmod 0755 "$fixture_dir/crates/zfb/binaries/esbuild/esbuild"
  printf 'arm64-tailwind-fixture-bytes' >"$fixture_dir/crates/zfb/binaries/tailwindcss-v4"
  chmod 0755 "$fixture_dir/crates/zfb/binaries/tailwindcss-v4"
}

# ── Case 1: both slots present pre-build — restore puts back byte-identical
# content AND the executable permission bit, after the "cross-build" leaves
# different bytes AND a different (non-executable) mode ────────────────────

WORK1=$(mktemp -d)
make_fixture "$WORK1"

bash -c '
  set -eu
  cd "'"$WORK1"'"
  . "'"$LIB"'"
  save_binary_slots
  printf "x64-esbuild-bytes" > crates/zfb/binaries/esbuild/esbuild
  chmod 0644 crates/zfb/binaries/esbuild/esbuild
  printf "x64-tailwind-bytes" > crates/zfb/binaries/tailwindcss-v4
  chmod 0644 crates/zfb/binaries/tailwindcss-v4
  restore_binary_slots
'

if [ "$(cat "$WORK1/crates/zfb/binaries/esbuild/esbuild")" = "arm64-esbuild-fixture-bytes" ]; then
  pass "case 1: esbuild slot content restored byte-identical"
else
  fail "case 1: esbuild slot content NOT restored (got: $(cat "$WORK1/crates/zfb/binaries/esbuild/esbuild"))"
fi

if [ "$(cat "$WORK1/crates/zfb/binaries/tailwindcss-v4")" = "arm64-tailwind-fixture-bytes" ]; then
  pass "case 1: tailwindcss-v4 slot content restored byte-identical"
else
  fail "case 1: tailwindcss-v4 slot content NOT restored (got: $(cat "$WORK1/crates/zfb/binaries/tailwindcss-v4"))"
fi

if [ -x "$WORK1/crates/zfb/binaries/esbuild/esbuild" ]; then
  pass "case 1: esbuild slot executable bit restored"
else
  fail "case 1: esbuild slot executable bit NOT restored"
fi

if [ -x "$WORK1/crates/zfb/binaries/tailwindcss-v4" ]; then
  pass "case 1: tailwindcss-v4 slot executable bit restored"
else
  fail "case 1: tailwindcss-v4 slot executable bit NOT restored"
fi

rm -rf "$WORK1"

# ── Case 2: a slot absent pre-build — the cross-build creates it, restore
# deletes it (the pre-absent invariant: leave slots exactly as found) ───────

WORK2=$(mktemp -d)
mkdir -p "$WORK2/crates/zfb/binaries/esbuild"
printf 'arm64-esbuild-fixture-bytes' >"$WORK2/crates/zfb/binaries/esbuild/esbuild"
chmod 0755 "$WORK2/crates/zfb/binaries/esbuild/esbuild"
# tailwindcss-v4 deliberately absent pre-build.

bash -c '
  set -eu
  cd "'"$WORK2"'"
  . "'"$LIB"'"
  save_binary_slots
  printf "x64-tailwind-bytes" > crates/zfb/binaries/tailwindcss-v4
  chmod 0755 crates/zfb/binaries/tailwindcss-v4
  restore_binary_slots
'

if [ ! -e "$WORK2/crates/zfb/binaries/tailwindcss-v4" ]; then
  pass "case 2: pre-absent tailwindcss-v4 slot restored to absent (cross-build leftover removed)"
else
  fail "case 2: pre-absent tailwindcss-v4 slot still present after restore"
fi

if [ "$(cat "$WORK2/crates/zfb/binaries/esbuild/esbuild")" = "arm64-esbuild-fixture-bytes" ]; then
  pass "case 2: untouched esbuild slot still intact"
else
  fail "case 2: untouched esbuild slot content changed unexpectedly"
fi

rm -rf "$WORK2"

# ── Case 3: EXIT-trap failure path — a mid-build failure must still restore
# both slots AND preserve the original (non-zero) exit status ──────────────

WORK3=$(mktemp -d)
make_fixture "$WORK3"

if bash -c '
  set -eu
  cd "'"$WORK3"'"
  . "'"$LIB"'"
  save_binary_slots
  _test_trap() {
    local exit_status=$?
    restore_binary_slots || true
    exit "$exit_status"
  }
  trap _test_trap EXIT
  printf "x64-partial-esbuild-bytes" > crates/zfb/binaries/esbuild/esbuild
  chmod 0644 crates/zfb/binaries/esbuild/esbuild
  exit 7
'; then
  RC3=0
else
  RC3=$?
fi

if [ "$RC3" -eq 7 ]; then
  pass "case 3: EXIT trap preserves the original (non-zero) exit status"
else
  fail "case 3: expected exit status 7 from the EXIT trap, got $RC3"
fi

if [ "$(cat "$WORK3/crates/zfb/binaries/esbuild/esbuild")" = "arm64-esbuild-fixture-bytes" ]; then
  pass "case 3: EXIT trap restored esbuild slot content on the failure path"
else
  fail "case 3: EXIT trap did NOT restore esbuild slot content on the failure path"
fi

if [ -x "$WORK3/crates/zfb/binaries/esbuild/esbuild" ]; then
  pass "case 3: EXIT trap restored esbuild slot executable bit on the failure path"
else
  fail "case 3: EXIT trap did NOT restore esbuild slot executable bit on the failure path"
fi

rm -rf "$WORK3"

# ── Summary ──────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
