#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/check-sse-endpoint.sh — offline fixture tests for
# scripts/check-sse-endpoint.mjs (issue #2644).
#
# Every case invokes the real Node guard against a throwaway crates/ tree.
# There is no network, cargo invocation, runtime server, or browser involved:
# this is a source-level invariant check. The fixtures cover both legitimate
# homes, the one reasoned allowlist exception, a forbidden test literal, and a
# stale allowlist entry.
#
# Run:
#   sh tests/unit/check-sse-endpoint.sh

set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
SCRIPT="$REPO_ROOT/scripts/check-sse-endpoint.mjs"

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

if [ ! -f "$SCRIPT" ]; then
  fail "guard script not found: $SCRIPT"
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# make_base_fixture <name> <allowlist_contents>
#
# The built-in allowlist is intentionally exercised in every fixture that is
# expected to be green. The source-home file proves the two directory prefixes
# are accepted; the embed probe is the sole non-subscriber exception.
make_base_fixture() {
  fixture="$WORK/$1"
  mkdir -p \
    "$fixture/crates/zfb-server/src" \
    "$fixture/crates/zfb-test-utils/src" \
    "$fixture/crates/zfb-server/tests"

  printf 'const RELOAD_ROUTE: &str = "__zfb/reload";\n' \
    >"$fixture/crates/zfb-server/src/routes.rs"
  printf 'pub const SSE_ROUTE: &str = "__zfb/reload";\n' \
    >"$fixture/crates/zfb-test-utils/src/sse_client.rs"
  printf '%s\n' "$2" >"$fixture/crates/zfb-server/tests/embed_lifecycle_smoke.rs"
  printf '%s\n' "$fixture"
}

run_guard() {
  fixture_root="$1"
  output_file="$2"
  if CHECK_SSE_ENDPOINT_CRATES_ROOT="$fixture_root/crates" \
    node "$SCRIPT" >"$output_file" 2>&1; then
    return 0
  else
    status=$?
    return "$status"
  fi
}

# ── Case 1: compliant fixture passes ───────────────────────────────────────
FIXTURE1=$(make_base_fixture case-1 'let _ = "__zfb/reload"; // bounded probe')
OUTPUT1="$WORK/case-1.out"
if run_guard "$FIXTURE1" "$OUTPUT1"; then
  pass "case 1: compliant source homes + reasoned allowlist pass"
else
  fail "case 1: compliant fixture unexpectedly failed:\n$(cat "$OUTPUT1")"
fi

# ── Case 2: forbidden literal is actually red ──────────────────────────────
FIXTURE2=$(make_base_fixture case-2 'let _ = "__zfb/reload"; // bounded probe')
printf 'let _forbidden = "__zfb/reload";\n' \
  >"$FIXTURE2/crates/zfb-server/tests/forbidden_subscription.rs"
OUTPUT2="$WORK/case-2.out"
if run_guard "$FIXTURE2" "$OUTPUT2"; then
  fail "case 2: forbidden tests/ literal unexpectedly passed"
else
  pass "case 2: forbidden tests/ literal exits non-zero"
  if grep -Fq "crates/zfb-server/tests/forbidden_subscription.rs" "$OUTPUT2" && \
    grep -Fq ".timeout()" "$OUTPUT2" && \
    grep -Fq "whole streaming response" "$OUTPUT2" && \
    grep -Fq "15s" "$OUTPUT2" && \
    grep -Fq "zfb_test_utils::open_sse()" "$OUTPUT2"; then
    pass "case 2: failure names file and teaches the SSE timeout remedy"
  else
    fail "case 2: failure omitted file/timeout/open_sse guidance:\n$(cat "$OUTPUT2")"
  fi

  # Remove the planted violation and run the same fixture again. This is the
  # explicit restore-to-green proof required by issue #2644, not merely a
  # separate green fixture made before the red run.
  rm -f "$FIXTURE2/crates/zfb-server/tests/forbidden_subscription.rs"
  OUTPUT2_RESTORED="$WORK/case-2-restored.out"
  if run_guard "$FIXTURE2" "$OUTPUT2_RESTORED"; then
    pass "case 2: restoring the forbidden fixture returns the guard to green"
  else
    fail "case 2: restored fixture unexpectedly failed:\n$(cat "$OUTPUT2_RESTORED")"
  fi
fi

# ── Case 3: the allowlisted violation passes ───────────────────────────────
FIXTURE3=$(make_base_fixture case-3 'let _ = "__zfb/reload"; // bounded HTTP 404 probe')
OUTPUT3="$WORK/case-3.out"
if run_guard "$FIXTURE3" "$OUTPUT3"; then
  pass "case 3: sole reasoned allowlist violation passes"
else
  fail "case 3: allowlisted fixture unexpectedly failed:\n$(cat "$OUTPUT3")"
fi

# ── Case 4: stale allowlist is actually red ────────────────────────────────
FIXTURE4=$(make_base_fixture case-4 'let _unrelated = "not the endpoint";')
OUTPUT4="$WORK/case-4.out"
if run_guard "$FIXTURE4" "$OUTPUT4"; then
  fail "case 4: stale allowlist entry unexpectedly passed"
else
  pass "case 4: stale allowlist entry exits non-zero"
  if grep -Fq "stale allowlist entry crates/zfb-server/tests/embed_lifecycle_smoke.rs" "$OUTPUT4"; then
    pass "case 4: stale failure names the allowlist entry"
  else
    fail "case 4: failure did not identify the stale allowlist entry:\n$(cat "$OUTPUT4")"
  fi
fi

# ── Summary ─────────────────────────────────────────────────────────────────
printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
