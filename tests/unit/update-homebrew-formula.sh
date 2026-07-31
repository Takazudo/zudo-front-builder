#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/update-homebrew-formula.sh — offline tests for the tap-checkout
# handling in scripts/update-homebrew-formula.sh.
#
# POSIX sh, NOT bash: health.yml runs every tests/unit/*.sh with `sh`, which is
# dash on the ubuntu runner, so the shebang is ignored and bashisms fail there
# even when the file runs clean under bash locally. (`set -o pipefail` is the
# one that bit — on macOS /bin/sh is bash in POSIX mode and accepts it.)
# Verify with `dash tests/unit/update-homebrew-formula.sh`, not just bash.
#
# Regression origin: the script used to write Formula/zfb.rb BEFORE touching
# git, so on a host with no tap checkout it manufactured a plain directory and
# then died at `git add` with "not a git repository" — leaving a stub that made
# every later run fail identically. Hit for real on the v1.0.0 release
# (2026-07-31), the first release for which this stable-gated script ever ran.
# Nothing tested it, which is why it shipped.
#
# Runs entirely offline:
#   - ZFB_SHA256_SOURCE_DIR feeds checksums from local fixture files (no curl).
#   - ZFB_TAP_REMOTE points at a local bare repo (no network, no SSH).
#
# Run:
#   bash tests/unit/update-homebrew-formula.sh

set -eu

PASS=0
FAIL=0

pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/update-homebrew-formula.sh"

if [ ! -x "$SCRIPT" ]; then
  printf 'FAIL: %s is missing or not executable\n' "$SCRIPT"
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

VERSION="9.9.9"

# ── Checksum fixtures ─────────────────────────────────────────────────────────

SHA_DIR="${WORK}/sha256"
mkdir -p "$SHA_DIR"
for triple in aarch64-apple-darwin x86_64-apple-darwin \
              aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
  # 64 hex chars — the script validates this shape.
  printf '%s  zfb-%s-%s.tar.gz\n' \
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" \
    "$VERSION" "$triple" > "${SHA_DIR}/zfb-${VERSION}-${triple}.tar.gz.sha256"
done

# ── A local bare repo standing in for the real tap remote ─────────────────────

BARE="${WORK}/homebrew-tap.git"
git init -q --bare "$BARE"

SEED="${WORK}/seed"
git init -q "$SEED"
git -C "$SEED" config user.email "test@example.com"
git -C "$SEED" config user.name "test"
mkdir -p "${SEED}/Formula"
printf 'placeholder\n' > "${SEED}/Formula/zfb.rb"
git -C "$SEED" add -A
git -C "$SEED" commit -qm "seed"
git -C "$SEED" branch -M main
git -C "$SEED" remote add origin "$BARE"
git -C "$SEED" push -q origin main

run_script() {
  # Never inherit a real remote or real checksums into a test.
  ZFB_SHA256_SOURCE_DIR="$SHA_DIR" ZFB_TAP_REMOTE="$BARE" \
    bash "$SCRIPT" "$VERSION" --tap-path "$1" ${2:+"$2"} 2>&1
}

# ── Case 1: tap absent → cloned, formula written, no stub ─────────────────────

TAP1="${WORK}/case1/homebrew-tap"
if OUT_1="$(run_script "$TAP1")"; then RC_1=0; else RC_1=$?; fi

if [ "$RC_1" -eq 0 ] && [ -d "${TAP1}/.git" ] && grep -q "version \"${VERSION}\"" "${TAP1}/Formula/zfb.rb"; then
  pass "absent tap is cloned and the formula is written into a real git repo"
else
  fail "absent tap: expected clone + formula, got exit=${RC_1} git=$([ -d "${TAP1}/.git" ] && echo yes || echo no)
$OUT_1"
fi

# ── Case 2: non-git stub → hard error, and the stub is NOT written into ───────
#
# This is the exact v1.0.0 failure. The old script would have overwritten
# Formula/zfb.rb here and then died at `git add`.

TAP2="${WORK}/case2/homebrew-tap"
mkdir -p "${TAP2}/Formula"
printf 'STUB\n' > "${TAP2}/Formula/zfb.rb"

if OUT_2="$(run_script "$TAP2")"; then RC_2=0; else RC_2=$?; fi

if [ "$RC_2" -ne 0 ] \
  && printf '%s' "$OUT_2" | grep -q "not a git repository" \
  && [ "$(cat "${TAP2}/Formula/zfb.rb")" = "STUB" ]; then
  pass "non-git stub errors out and the formula file is left untouched"
else
  fail "non-git stub: expected non-zero exit + untouched file, got exit=${RC_2} content='$(cat "${TAP2}/Formula/zfb.rb")'
$OUT_2"
fi

# ── Case 3: the failure mode must not be order-dependent ──────────────────────
#
# Guards the actual regression: the tap is validated BEFORE the formula is
# written. A fresh non-git dir must never come back holding a generated formula.

TAP3="${WORK}/case3/homebrew-tap"
mkdir -p "$TAP3"
printf 'unrelated\n' > "${TAP3}/README.md"

if OUT_3="$(run_script "$TAP3")"; then RC_3=0; else RC_3=$?; fi

if [ "$RC_3" -ne 0 ] && [ ! -e "${TAP3}/Formula/zfb.rb" ]; then
  pass "no formula is written into a non-git directory (write happens after validation)"
else
  fail "expected no Formula/zfb.rb in a non-git dir, got exit=${RC_3} exists=$([ -e "${TAP3}/Formula/zfb.rb" ] && echo yes || echo no)
$OUT_3"
fi

# ── Case 4: valid checkout + --push commits to the (local) remote ─────────────

TAP4="${WORK}/case4/homebrew-tap"
git clone -q "$BARE" "$TAP4"
git -C "$TAP4" config user.email "test@example.com"
git -C "$TAP4" config user.name "test"

if OUT_4="$(run_script "$TAP4" --push)"; then RC_4=0; else RC_4=$?; fi

PUSHED="$(git -C "$BARE" log -1 --format=%s 2>/dev/null || echo "")"
if [ "$RC_4" -eq 0 ] && [ "$PUSHED" = "zfb ${VERSION}" ]; then
  pass "--push commits and pushes to the tap remote"
else
  fail "--push: expected remote HEAD 'zfb ${VERSION}', got exit=${RC_4} subject='${PUSHED}'
$OUT_4"
fi

# ── Summary ───────────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
