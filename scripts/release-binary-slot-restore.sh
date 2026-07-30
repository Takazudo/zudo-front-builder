#!/usr/bin/env bash
# scripts/release-binary-slot-restore.sh
#
# Save/restore helpers for the two shared, arch-unqualified vendor binary
# slots that crates/zfb/build.rs's download_binaries() stages at:
#   - crates/zfb/binaries/esbuild/esbuild
#   - crates/zfb/binaries/tailwindcss-v4
#
# scripts/build-macos-x64-local.sh cross-builds zfb for x86_64-apple-darwin
# from an arm64 host. build.rs correctly detects that the staged arm64
# binaries fail the x64 SHA-256 pins and overwrites BOTH slots with
# darwin-x64 binaries — correct for the cross-build itself, but it leaves
# the shared repo slots wrong-arch for the host afterward until something
# re-runs the build script natively. That is the pollution source behind
# issue #2178 (a fresh worktree's test lookups walk up to the main repo's
# slot and find the wrong architecture).
#
# These functions are pure file operations, deliberately NOT a post-build
# native `cargo check -p zfb` heal (which could trigger a binary
# re-download over the network). This file defines functions ONLY — no
# top-level executable code — so it is always safe to `source` it, both
# from build-macos-x64-local.sh and from
# tests/unit/release-binary-slot-restore.sh, which exercises it directly
# against a throwaway fixture tree.
#
# restore_binary_slots removes its backup directory once every slot restore
# has been attempted, so repeated local release builds don't accumulate
# stale copies of the ~75 MB tailwindcss-v4 slot under $TMPDIR.
#
# Known blind spot (documented, not engineered around): a concurrent native
# build writing to a slot between save_binary_slots and restore_binary_slots
# will have its freshly staged output clobbered by the restore.

# Slot paths, relative to the repo root. Callers must cd there first (both
# build-macos-x64-local.sh and the offline test do, matching how the rest of
# build-macos-x64-local.sh already addresses these slots relative to
# workspace_root).
BINARY_SLOT_PATHS=(
  "crates/zfb/binaries/esbuild/esbuild"
  "crates/zfb/binaries/tailwindcss-v4"
)

# Set by save_binary_slots(); restore_binary_slots() reads it and removes it
# once every slot has been restored (or a restore has been attempted).
BINARY_SLOT_BACKUP_DIR=""

# save_binary_slots — snapshot every path in BINARY_SLOT_PATHS into a fresh
# mktemp -d backup directory. `cp -p` preserves bytes AND permissions/
# timestamps. A slot that does not exist yet is simply not backed up — its
# absence IS the recorded pre-build state (see restore_binary_slots below).
save_binary_slots() {
  BINARY_SLOT_BACKUP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/zfb-binary-slot-backup.XXXXXX")"
  local idx=0
  for slot in "${BINARY_SLOT_PATHS[@]}"; do
    if [[ -e "$slot" ]]; then
      cp -p "$slot" "${BINARY_SLOT_BACKUP_DIR}/slot_${idx}"
    fi
    idx=$((idx + 1))
  done
}

# restore_binary_slots — put every slot back exactly as save_binary_slots()
# found it: byte-identical with permissions restored if a backup exists,
# deleted (removing whatever the cross-build left behind) if the slot was
# absent pre-build. Always attempts every slot rather than stopping at the
# first failure; reports each individual failure to stderr and returns
# non-zero if any slot could not be restored.
restore_binary_slots() {
  if [[ -z "$BINARY_SLOT_BACKUP_DIR" ]]; then
    echo "ERROR: restore_binary_slots called before save_binary_slots" >&2
    return 1
  fi
  local failed=0
  local idx=0
  for slot in "${BINARY_SLOT_PATHS[@]}"; do
    local backup="${BINARY_SLOT_BACKUP_DIR}/slot_${idx}"
    if [[ -f "$backup" ]]; then
      if ! cp -p "$backup" "$slot"; then
        echo "ERROR: failed to restore binary slot to its pre-build state: $slot" >&2
        failed=1
      fi
    elif [[ -e "$slot" ]]; then
      if ! rm -f "$slot"; then
        echo "ERROR: failed to remove cross-build leftover at: $slot" >&2
        failed=1
      fi
    fi
    idx=$((idx + 1))
  done
  # The backup has served its purpose (every slot restore was attempted) —
  # remove it so repeated local release builds don't accumulate stale copies
  # of the ~75 MB tailwindcss-v4 slot under $TMPDIR (codex review finding).
  rm -rf "$BINARY_SLOT_BACKUP_DIR"
  BINARY_SLOT_BACKUP_DIR=""
  return "$failed"
}
