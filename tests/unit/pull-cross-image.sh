#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/pull-cross-image.sh — offline tests for scripts/pull-cross-image.sh.
#
# A fake `docker` on PATH records each pull and fails a configurable number of
# times, so no registry or Docker daemon is involved.
#
# Run:
#   sh tests/unit/pull-cross-image.sh

set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
SCRIPT="$REPO_ROOT/scripts/pull-cross-image.sh"

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK/bin"
cat >"$WORK/bin/docker" <<'SH'
#!/bin/sh
echo "$*" >>"$FAKE_DOCKER_LOG"
n=$(wc -l <"$FAKE_DOCKER_LOG")
[ "$n" -gt "${FAKE_DOCKER_FAILS:-0}" ]
SH
chmod +x "$WORK/bin/docker"

run() {
  : >"$WORK/log"
  PATH="$WORK/bin:$PATH" FAKE_DOCKER_LOG="$WORK/log" PULL_RETRY_DELAY_S=0 \
    CROSS_TOML="$REPO_ROOT/Cross.toml" bash "$SCRIPT" "$@" >"$WORK/out" 2>&1
}

pinned=$(grep -E '^image = "ghcr.io/cross-rs/aarch64' "$REPO_ROOT/Cross.toml" | sed 's/^image = "//; s/"$//')

if FAKE_DOCKER_FAILS=0 run aarch64-unknown-linux-gnu && [ "$(cat "$WORK/log")" = "pull $pinned" ]; then
  pass "pulls the pinned aarch64 digest from Cross.toml"
else
  fail "pulls the pinned aarch64 digest from Cross.toml"
fi

if FAKE_DOCKER_FAILS=2 run aarch64-unknown-linux-gnu && [ "$(wc -l <"$WORK/log")" -eq 3 ]; then
  pass "retries a failed pull and succeeds on attempt 3"
else
  fail "retries a failed pull and succeeds on attempt 3"
fi

if ! FAKE_DOCKER_FAILS=3 run aarch64-unknown-linux-gnu && [ "$(wc -l <"$WORK/log")" -eq 3 ] \
  && grep -q '::error::' "$WORK/out"; then
  pass "fails after exhausting attempts"
else
  fail "fails after exhausting attempts"
fi

if run x86_64-unknown-linux-gnu && [ ! -s "$WORK/log" ]; then
  pass "is a no-op for a target with no pinned image"
else
  fail "is a no-op for a target with no pinned image"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
