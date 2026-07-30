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
BACKUP_DIR_FILE1=$(mktemp)

bash -c '
  set -eu
  cd "'"$WORK1"'"
  . "'"$LIB"'"
  save_binary_slots
  printf "x64-esbuild-bytes" > crates/zfb/binaries/esbuild/esbuild
  chmod 0644 crates/zfb/binaries/esbuild/esbuild
  printf "x64-tailwind-bytes" > crates/zfb/binaries/tailwindcss-v4
  chmod 0644 crates/zfb/binaries/tailwindcss-v4
  echo "$BINARY_SLOT_BACKUP_DIR" > "'"$BACKUP_DIR_FILE1"'"
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

BACKUP_DIR1=$(cat "$BACKUP_DIR_FILE1")
if [ -n "$BACKUP_DIR1" ] && [ ! -e "$BACKUP_DIR1" ]; then
  pass "case 1: backup directory removed after restore (no stale temp copy left behind)"
else
  fail "case 1: backup directory still present after restore: $BACKUP_DIR1"
fi
rm -f "$BACKUP_DIR_FILE1"

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
BACKUP_DIR_FILE3=$(mktemp)

if bash -c '
  set -eu
  cd "'"$WORK3"'"
  . "'"$LIB"'"
  save_binary_slots
  echo "$BINARY_SLOT_BACKUP_DIR" > "'"$BACKUP_DIR_FILE3"'"
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

BACKUP_DIR3=$(cat "$BACKUP_DIR_FILE3")
if [ -n "$BACKUP_DIR3" ] && [ ! -e "$BACKUP_DIR3" ]; then
  pass "case 3: backup directory removed by the EXIT trap's restore on the failure path"
else
  fail "case 3: backup directory still present after the EXIT trap's restore: $BACKUP_DIR3"
fi
rm -f "$BACKUP_DIR_FILE3"

rm -rf "$WORK3"

# ── Case 4: a restore that FAILS must RETAIN the backup directory (it is then
# the only surviving copy of the original slot bytes), name the retained path
# in its failure report, and still return non-zero ─────────────────────────
#
# The failure is forced deterministically and WITHOUT relying on file modes
# (a root/CI uid would bypass a chmod-based guard): after save_binary_slots
# has snapshotted both slots, the esbuild slot's parent DIRECTORY is replaced
# by a regular file, so `cp -p <backup> crates/zfb/binaries/esbuild/esbuild`
# fails with ENOTDIR ("Not a directory") for every uid.

WORK4=$(mktemp -d)
make_fixture "$WORK4"
BACKUP_DIR_FILE4=$(mktemp)
BACKUP_DIR_AFTER_FILE4=$(mktemp)
STDERR_FILE4=$(mktemp)

if bash -c '
  set -eu
  cd "'"$WORK4"'"
  . "'"$LIB"'"
  save_binary_slots
  echo "$BINARY_SLOT_BACKUP_DIR" > "'"$BACKUP_DIR_FILE4"'"
  printf "x64-tailwind-bytes" > crates/zfb/binaries/tailwindcss-v4
  chmod 0644 crates/zfb/binaries/tailwindcss-v4
  # Replace the esbuild slot directory with a regular file → the restore cp
  # into crates/zfb/binaries/esbuild/esbuild cannot succeed (ENOTDIR).
  rm -rf crates/zfb/binaries/esbuild
  printf "not-a-directory" > crates/zfb/binaries/esbuild
  rc=0
  restore_binary_slots || rc=$?
  echo "$BINARY_SLOT_BACKUP_DIR" > "'"$BACKUP_DIR_AFTER_FILE4"'"
  exit "$rc"
' 2>"$STDERR_FILE4"; then
  RC4=0
else
  RC4=$?
fi

if [ "$RC4" -ne 0 ]; then
  pass "case 4: restore_binary_slots reports failure (non-zero return) when a slot cannot be restored"
else
  fail "case 4: restore_binary_slots returned 0 despite an unrestorable slot"
fi

BACKUP_DIR4=$(cat "$BACKUP_DIR_FILE4")
if [ -n "$BACKUP_DIR4" ] && [ -d "$BACKUP_DIR4" ]; then
  pass "case 4: backup directory RETAINED after a failed restore"
else
  fail "case 4: backup directory destroyed after a failed restore: $BACKUP_DIR4"
fi

if [ -f "$BACKUP_DIR4/slot_0" ] &&
  [ "$(cat "$BACKUP_DIR4/slot_0")" = "arm64-esbuild-fixture-bytes" ]; then
  pass "case 4: retained backup still holds the original esbuild slot bytes"
else
  fail "case 4: retained backup lost the original esbuild slot bytes"
fi

if grep -q -- "$BACKUP_DIR4" "$STDERR_FILE4"; then
  pass "case 4: failure report names the retained backup directory path"
else
  fail "case 4: failure report does NOT name the retained backup path ($BACKUP_DIR4)"
fi

if [ "$(cat "$BACKUP_DIR_AFTER_FILE4")" = "$BACKUP_DIR4" ]; then
  pass "case 4: BINARY_SLOT_BACKUP_DIR left set after a failed restore (a retry is still possible)"
else
  fail "case 4: BINARY_SLOT_BACKUP_DIR cleared after a failed restore — a retry could not find the backup"
fi

# The still-restorable slot must have been restored anyway (restore attempts
# every slot rather than stopping at the first failure).
if [ "$(cat "$WORK4/crates/zfb/binaries/tailwindcss-v4")" = "arm64-tailwind-fixture-bytes" ]; then
  pass "case 4: the other slot was still restored despite the failing one"
else
  fail "case 4: the other slot was not restored after the first slot failed"
fi

rm -rf "$BACKUP_DIR4"
rm -f "$BACKUP_DIR_FILE4" "$BACKUP_DIR_AFTER_FILE4" "$STDERR_FILE4"
rm -rf "$WORK4"

# ── Summary ──────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
