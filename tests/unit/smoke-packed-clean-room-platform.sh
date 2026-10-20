#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/smoke-packed-clean-room-platform.sh — offline tests for the
# platform selection in scripts/smoke-packed-clean-room.sh (#4231 item 2: the
# Stage 7 store-install vs `link:` byte parity check on Darwin).
#
# Never runs the script end to end (that needs a freshly built zfb binary).
# It checks the argument guards, which all exit before any pack/install work:
#   - an unknown ZFB_SMOKE_PLATFORM is a usage error (exit 2);
#   - a platform whose os/arch is not the host's is refused (exit 2), so a
#     mismatched host cannot silently skip the optional platform package;
#   - the matching platform's binary and CSS addon env vars are required, and
#     the default platform keeps the legacy ZFB_LINUX_X64_* names;
#   - static pin: no code line outside the platform table hard-codes a
#     platform package name.
#
# Requires: sh, bash, grep, uname. No network.
#
# Run:
#   sh tests/unit/smoke-packed-clean-room-platform.sh

set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
SCRIPT="$REPO_ROOT/scripts/smoke-packed-clean-room.sh"

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

run_script() {
  # run_script <expected-exit> <expected-stderr-substring> [VAR=value ...]
  expected_rc=$1
  expected_msg=$2
  shift 2
  set +e
  out=$(cd "$REPO_ROOT" && env -i PATH="$PATH" HOME="${HOME:-/}" "$@" bash "$SCRIPT" 2>&1)
  rc=$?
  set -e
  if [ "$rc" = "$expected_rc" ] && printf '%s' "$out" | grep -qF -- "$expected_msg"; then
    return 0
  fi
  printf '  got rc=%s output: %s\n' "$rc" "$out"
  return 1
}

if run_script 2 "unsupported ZFB_SMOKE_PLATFORM 'bogus'" ZFB_SMOKE_PLATFORM=bogus; then
  pass "an unknown ZFB_SMOKE_PLATFORM exits 2"
else
  fail "an unknown ZFB_SMOKE_PLATFORM exits 2"
fi

# Pick a supported platform that is NOT the host's.
HOST="$(uname -s)/$(uname -m)"
case "$HOST" in
  Darwin/arm64) OTHER=linux-x64-gnu; OTHER_BIN=ZFB_LINUX_X64_BINARY; OTHER_ADDON=ZFB_LINUX_X64_CSS_ADDON ;;
  *) OTHER=darwin-arm64; OTHER_BIN=ZFB_DARWIN_ARM64_BINARY; OTHER_ADDON=ZFB_DARWIN_ARM64_CSS_ADDON ;;
esac
if run_script 2 "ZFB_SMOKE_PLATFORM=$OTHER needs a" \
  ZFB_SMOKE_PLATFORM="$OTHER" "$OTHER_BIN=/nonexistent" "$OTHER_ADDON=/nonexistent"; then
  pass "a platform that does not match the host ($HOST) is refused with exit 2"
else
  fail "a platform that does not match the host ($HOST) is refused with exit 2"
fi

# Platform matching this host, to exercise the env requirements.
case "$HOST" in
  Linux/x86_64) HOSTP=linux-x64-gnu; BIN=ZFB_LINUX_X64_BINARY; ADDON=ZFB_LINUX_X64_CSS_ADDON ;;
  Darwin/arm64) HOSTP=darwin-arm64; BIN=ZFB_DARWIN_ARM64_BINARY; ADDON=ZFB_DARWIN_ARM64_CSS_ADDON ;;
  Darwin/x86_64) HOSTP=darwin-x64; BIN=ZFB_DARWIN_X64_BINARY; ADDON=ZFB_DARWIN_X64_CSS_ADDON ;;
  *) HOSTP=; BIN=; ADDON= ;;
esac
if [ -n "$HOSTP" ]; then
  if run_script 2 "$BIN env var is required" ZFB_SMOKE_PLATFORM="$HOSTP"; then
    pass "$HOSTP requires $BIN"
  else
    fail "$HOSTP requires $BIN"
  fi
  if run_script 2 "$ADDON env var is required" ZFB_SMOKE_PLATFORM="$HOSTP" "$BIN=/nonexistent"; then
    pass "$HOSTP requires $ADDON"
  else
    fail "$HOSTP requires $ADDON"
  fi
  if run_script 2 "$BIN does not exist" ZFB_SMOKE_PLATFORM="$HOSTP" "$BIN=/nonexistent" "$ADDON=/nonexistent"; then
    pass "$HOSTP rejects a missing binary path"
  else
    fail "$HOSTP rejects a missing binary path"
  fi
else
  printf 'SKIP: host %s has no supported platform entry; env-requirement cases not run\n' "$HOST"
fi

if [ "$HOST" = "Linux/x86_64" ]; then
  if run_script 2 "ZFB_LINUX_X64_BINARY env var is required"; then
    pass "the default platform is linux-x64-gnu with the legacy ZFB_LINUX_X64_* names"
  else
    fail "the default platform is linux-x64-gnu with the legacy ZFB_LINUX_X64_* names"
  fi
fi

# Static pin: platform package names appear only in the comments and the
# platform table / default, never in the stages' code.
HARDCODED=$(grep -nE 'zfb-(linux|darwin|win32)-' "$SCRIPT" | grep -vE '^[0-9]+:[[:space:]]*#' |
  grep -vE 'HOST_OS=' || true)
if [ -z "$HARDCODED" ]; then
  pass "no code line outside the platform table hard-codes a platform package name"
else
  fail "platform package name hard-coded outside the table: $HARDCODED"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
