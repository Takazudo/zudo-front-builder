#!/usr/bin/env bash
set -euo pipefail

# scripts/verify-vendor-override.sh
#
# Proves the ZFB_ESBUILD_BIN / ZFB_TAILWIND_BIN vendor-override contract
# (see BUILDING.md, "ZFB_ESBUILD_BIN / ZFB_TAILWIND_BIN override contract")
# end-to-end on a CLEAN checkout — not the developer's own worktree, which
# may already have binaries staged under crates/zfb/binaries/.
#
# What this proves:
#   1. `git archive HEAD` into a scratch dir contains NO gitignored binary
#      slots (crates/zfb/binaries/esbuild/esbuild,
#      crates/zfb/binaries/tailwindcss-v4) — a genuinely clean tree.
#   2. node_modules is provisioned by symlinking the source checkout's own
#      node_modules/ (an unrelated prerequisite: build.rs also embeds
#      framework packages from node_modules/.pnpm — see
#      `embed_framework_packages` in crates/zfb/build.rs). This must NOT
#      go through the override mechanism.
#   3. With both ZFB_ESBUILD_BIN and ZFB_TAILWIND_BIN pointed at the
#      dev machine's already-staged binaries (absolute paths), a
#      `cargo check -p zfb --no-default-features --offline` in that clean
#      tree succeeds with zero downloads and stages the exact override
#      bytes into $OUT_DIR/vendor/bin/.
#   4. A bogus (non-existent) override path fails fast with the direct
#      path + var-naming error from `validate_override_path` in build.rs,
#      not a generic/opaque failure.
#
# `--no-default-features` disables the `embed_v8` feature (see
# crates/zfb/Cargo.toml) so this check does not pay the V8 compile cost;
# build.rs (the thing under test) runs unconditionally regardless of that
# feature.
#
# Unix hosts only (macOS/Linux, matching the CI runners this repo uses) —
# relies on `ln -s`, `shasum`, and unsuffixed binary names as staged by
# build.rs on those two platform families. Not Windows-portable.
#
# Prerequisite: a prior `cargo build --workspace` (or equivalent) in
# $REPO_ROOT. This both stages the default override sources under
# crates/zfb/binaries/ AND warms $CARGO_HOME's registry/git caches for
# every workspace dependency — the isolated `cargo check --offline` run
# below resolves crates from that cache, not the network, so a genuinely
# empty $CARGO_HOME will fail at dependency resolution before the
# override behavior is ever exercised.
#
# Usage:
#   scripts/verify-vendor-override.sh
#
# Optional env (defaults resolve the override sources from this checkout's
# own crates/zfb/binaries/ staging slots — run `cargo build --workspace`
# first if they are not populated):
#   ZFB_VERIFY_ESBUILD_SRC   — absolute path to a pre-verified esbuild binary
#   ZFB_VERIFY_TAILWIND_SRC  — absolute path to a pre-verified tailwindcss-v4 binary

REPO_ROOT="$(git rev-parse --show-toplevel)"

ESBUILD_SRC="${ZFB_VERIFY_ESBUILD_SRC:-$REPO_ROOT/crates/zfb/binaries/esbuild/esbuild}"
TAILWIND_SRC="${ZFB_VERIFY_TAILWIND_SRC:-$REPO_ROOT/crates/zfb/binaries/tailwindcss-v4}"

pass() { printf '[PASS] %s\n' "$1"; }
fail() { printf '[FAIL] %s\n' "$1" >&2; exit 1; }

if [[ ! -f "$ESBUILD_SRC" ]]; then
  fail "override source esbuild binary not found at $ESBUILD_SRC — run \`cargo build --workspace\` in $REPO_ROOT first (this also warms \$CARGO_HOME's registry cache, required for the --offline check below) (or set ZFB_VERIFY_ESBUILD_SRC)."
fi
if [[ ! -f "$TAILWIND_SRC" ]]; then
  fail "override source tailwindcss binary not found at $TAILWIND_SRC — run \`cargo build --workspace\` in $REPO_ROOT first (this also warms \$CARGO_HOME's registry cache, required for the --offline check below) (or set ZFB_VERIFY_TAILWIND_SRC)."
fi

# ── Scratch dirs ─────────────────────────────────────────────────────────

CLEAN_DIR=$(mktemp -d)
ISOLATED_TARGET_DIR=$(mktemp -d)

cleanup() {
  rm -rf "$CLEAN_DIR" "$ISOLATED_TARGET_DIR"
}
trap cleanup EXIT

# ── Step 1: clean-tree checkout via git archive ─────────────────────────

echo "Archiving HEAD into $CLEAN_DIR ..."
git -C "$REPO_ROOT" archive HEAD | tar -x -C "$CLEAN_DIR"

if [[ -e "$CLEAN_DIR/crates/zfb/binaries/esbuild/esbuild" ]]; then
  fail "clean-tree control failed: crates/zfb/binaries/esbuild/esbuild is present in a fresh git archive — it should be gitignored, not tracked."
fi
pass "git archive HEAD contains no staged esbuild binary slot"

if [[ -e "$CLEAN_DIR/crates/zfb/binaries/tailwindcss-v4" ]]; then
  fail "clean-tree control failed: crates/zfb/binaries/tailwindcss-v4 is present in a fresh git archive — it should be gitignored, not tracked."
fi
pass "git archive HEAD contains no staged tailwindcss binary slot"

# ── Step 2: provision node_modules (unrelated prerequisite) ─────────────
#
# build.rs's embed_framework_packages() reads node_modules/.pnpm/ for
# preact/preact-render-to-string/hono regardless of the override paths, so
# the clean tree needs a node_modules/ — but it must come from a symlink to
# the already-`pnpm install`-ed source checkout, never through
# ZFB_ESBUILD_BIN/ZFB_TAILWIND_BIN (those two only ever touch the esbuild
# and tailwindcss binary slots).

if [[ ! -d "$REPO_ROOT/node_modules/.pnpm" ]]; then
  fail "source checkout has no node_modules/.pnpm — run \`pnpm install --frozen-lockfile\` in $REPO_ROOT first."
fi
ln -s "$REPO_ROOT/node_modules" "$CLEAN_DIR/node_modules"
pass "node_modules symlinked from source checkout ($REPO_ROOT/node_modules)"

# ── Step 3: offline cargo check with both overrides set ─────────────────

echo "Running cargo check -p zfb --no-default-features --offline with both overrides set ..."
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

CHECK_LOG=$(mktemp)
set +e
(
  cd "$CLEAN_DIR" \
    && CARGO_HOME="$CARGO_HOME" \
       CARGO_TARGET_DIR="$ISOLATED_TARGET_DIR" \
       HTTP_PROXY="http://127.0.0.1:1" \
       HTTPS_PROXY="http://127.0.0.1:1" \
       ZFB_ESBUILD_BIN="$ESBUILD_SRC" \
       ZFB_TAILWIND_BIN="$TAILWIND_SRC" \
       cargo check -p zfb --no-default-features --offline
) >"$CHECK_LOG" 2>&1
CHECK_STATUS=$?
set -e

cat "$CHECK_LOG"

if [[ "$CHECK_STATUS" -ne 0 ]]; then
  rm -f "$CHECK_LOG"
  fail "cargo check -p zfb --no-default-features --offline failed with valid overrides set (exit $CHECK_STATUS) — see output above."
fi
pass "cargo check -p zfb --no-default-features --offline succeeded offline with both overrides set"

if grep -q "Downloading" "$CHECK_LOG"; then
  rm -f "$CHECK_LOG"
  fail "build.rs emitted a download warning even though both ZFB_ESBUILD_BIN and ZFB_TAILWIND_BIN overrides were set — override should skip the download path entirely."
fi
pass "no vendor download occurred (no \"Downloading\" line in build script output)"
rm -f "$CHECK_LOG"

# ── Step 4: staged vendor bytes hash-equal the override sources ─────────

STAGED_ESBUILD=$(find "$ISOLATED_TARGET_DIR" -type f -path '*/build/zfb-*/out/vendor/bin/esbuild' | head -1)
STAGED_TAILWIND=$(find "$ISOLATED_TARGET_DIR" -type f -path '*/build/zfb-*/out/vendor/bin/tailwindcss-v4' | head -1)

if [[ -z "$STAGED_ESBUILD" ]]; then
  fail "could not find staged esbuild binary under \$OUT_DIR/vendor/bin/ in $ISOLATED_TARGET_DIR"
fi
if [[ -z "$STAGED_TAILWIND" ]]; then
  fail "could not find staged tailwindcss-v4 binary under \$OUT_DIR/vendor/bin/ in $ISOLATED_TARGET_DIR"
fi

ESBUILD_SRC_SHA=$(shasum -a 256 "$ESBUILD_SRC" | awk '{print $1}')
STAGED_ESBUILD_SHA=$(shasum -a 256 "$STAGED_ESBUILD" | awk '{print $1}')
if [[ "$ESBUILD_SRC_SHA" != "$STAGED_ESBUILD_SHA" ]]; then
  fail "staged esbuild bytes ($STAGED_ESBUILD, sha256 $STAGED_ESBUILD_SHA) do not match the override source ($ESBUILD_SRC, sha256 $ESBUILD_SRC_SHA)."
fi
pass "staged esbuild bytes hash-equal the ZFB_ESBUILD_BIN override source (sha256 $ESBUILD_SRC_SHA)"

TAILWIND_SRC_SHA=$(shasum -a 256 "$TAILWIND_SRC" | awk '{print $1}')
STAGED_TAILWIND_SHA=$(shasum -a 256 "$STAGED_TAILWIND" | awk '{print $1}')
if [[ "$TAILWIND_SRC_SHA" != "$STAGED_TAILWIND_SHA" ]]; then
  fail "staged tailwindcss-v4 bytes ($STAGED_TAILWIND, sha256 $STAGED_TAILWIND_SHA) do not match the override source ($TAILWIND_SRC, sha256 $TAILWIND_SRC_SHA)."
fi
pass "staged tailwindcss-v4 bytes hash-equal the ZFB_TAILWIND_BIN override source (sha256 $TAILWIND_SRC_SHA)"

# ── Step 5: bogus override path fails with the direct error ─────────────

BOGUS_PATH="$CLEAN_DIR/does-not-exist-binary"
echo "Running cargo check -p zfb --no-default-features --offline with a bogus ZFB_ESBUILD_BIN ..."

BOGUS_LOG=$(mktemp)
set +e
(
  cd "$CLEAN_DIR" \
    && CARGO_HOME="$CARGO_HOME" \
       CARGO_TARGET_DIR="$ISOLATED_TARGET_DIR" \
       HTTP_PROXY="http://127.0.0.1:1" \
       HTTPS_PROXY="http://127.0.0.1:1" \
       ZFB_ESBUILD_BIN="$BOGUS_PATH" \
       ZFB_TAILWIND_BIN="$TAILWIND_SRC" \
       cargo check -p zfb --no-default-features --offline
) >"$BOGUS_LOG" 2>&1
BOGUS_STATUS=$?
set -e

cat "$BOGUS_LOG"

if [[ "$BOGUS_STATUS" -eq 0 ]]; then
  rm -f "$BOGUS_LOG"
  fail "cargo check succeeded with a bogus ZFB_ESBUILD_BIN ($BOGUS_PATH) — build.rs should have rejected it."
fi
pass "cargo check fails when ZFB_ESBUILD_BIN points at a non-existent path"

if ! grep -q "ZFB_ESBUILD_BIN" "$BOGUS_LOG"; then
  rm -f "$BOGUS_LOG"
  fail "failure output does not name ZFB_ESBUILD_BIN — expected the direct var-naming error from validate_override_path()."
fi
pass "failure output names ZFB_ESBUILD_BIN"

if ! grep -q "$BOGUS_PATH" "$BOGUS_LOG"; then
  rm -f "$BOGUS_LOG"
  fail "failure output does not include the bogus path ($BOGUS_PATH) — expected the direct path in the error message."
fi
pass "failure output includes the bogus override path"
rm -f "$BOGUS_LOG"

echo "verify-vendor-override.sh: all assertions passed."
