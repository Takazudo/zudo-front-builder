#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/install-prerelease.sh — offline fixture tests for the
# latest-prerelease tag-resolution logic in install.sh.
#
# Runs entirely offline: the pipeline logic is extracted inline and fed from
# shell fixture variables — no curl invocation.
# Requires: sh, awk, grep, sed (jq optional — both paths tested).
#
# Run:
#   sh tests/unit/install-prerelease.sh

set -eu

PASS=0
FAIL=0

pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

# ── Fixture ───────────────────────────────────────────────────────────────────
#
# Four entries:
#   v0.3.0        — stable (prerelease: false)
#   v0.2.1        — stable (prerelease: false)
#   v0.3.0-beta.1 — prerelease (prerelease: true)  ← expected winner
#   v0.2.0-rc.1}x — prerelease but tag_name contains '}', injected to prove the
#                   awk RS='}' path breaks on this input (documented brittle edge)
#
# The prerelease entry appears AFTER the two stable entries to confirm the
# implementation scans the full list rather than relying on array order alone.

FIXTURE='[
  {
    "tag_name": "v0.3.0",
    "prerelease": false,
    "name": "zfb v0.3.0"
  },
  {
    "tag_name": "v0.2.1",
    "prerelease": false,
    "name": "zfb v0.2.1"
  },
  {
    "tag_name": "v0.3.0-beta.1",
    "prerelease": true,
    "name": "zfb v0.3.0-beta.1"
  },
  {
    "tag_name": "v0.2.0-rc.1}x",
    "prerelease": true,
    "name": "zfb v0.2.0-rc.1}x"
  }
]'

EXPECTED_TAG="v0.3.0-beta.1"

# ── jq path ───────────────────────────────────────────────────────────────────

if command -v jq >/dev/null 2>&1; then
  # Extract using the same expression as install.sh's jq branch.
  _jq_tag=$(printf '%s' "$FIXTURE" \
    | jq -r 'map(select(.prerelease == true))[0].tag_name // empty')

  if [ "$_jq_tag" = "$EXPECTED_TAG" ]; then
    pass "jq path: extracted correct prerelease tag ($EXPECTED_TAG)"
  else
    fail "jq path: expected '$EXPECTED_TAG', got '$_jq_tag'"
  fi
else
  printf 'SKIP: jq not available — jq path test skipped\n'
fi

# ── awk path on clean input (no '}' in tag names) ────────────────────────────

# Build a fixture that omits the brace-injected entry so the awk path succeeds.
FIXTURE_CLEAN='[
  {
    "tag_name": "v0.3.0",
    "prerelease": false,
    "name": "zfb v0.3.0"
  },
  {
    "tag_name": "v0.2.1",
    "prerelease": false,
    "name": "zfb v0.2.1"
  },
  {
    "tag_name": "v0.3.0-beta.1",
    "prerelease": true,
    "name": "zfb v0.3.0-beta.1"
  }
]'

_awk_tag=$(printf '%s' "$FIXTURE_CLEAN" \
  | awk -v RS='}' '/"prerelease":[[:space:]]*true/ { print; exit }' \
  | grep -m1 '"tag_name":' \
  | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/')

if [ "$_awk_tag" = "$EXPECTED_TAG" ]; then
  pass "awk path (clean input): extracted correct prerelease tag ($EXPECTED_TAG)"
else
  fail "awk path (clean input): expected '$EXPECTED_TAG', got '$_awk_tag'"
fi

# ── awk path on brace-injected input (documented brittle behavior) ────────────
#
# When a tag_name contains '}', the awk RS='}' pipeline splits the JSON object
# at that byte.  The "prerelease": true field and the "tag_name" field end up in
# separate awk records, so awk exits after matching the first split record and
# the subsequent grep-sed produces no output.
#
# To expose this, the brace-injected entry must be the FIRST prerelease in the
# fixture (awk hits it, exits, and the tag_name field — in the next record — is
# never seen).  The test asserts empty output, which causes the fail-loud
# empty-check in install.sh to fire.

FIXTURE_BRACE='[
  {
    "tag_name": "v0.3.0",
    "prerelease": false,
    "name": "zfb v0.3.0"
  },
  {
    "tag_name": "v0.2.1",
    "prerelease": false,
    "name": "zfb v0.2.1"
  },
  {
    "tag_name": "v0.2.0-rc.1}x",
    "prerelease": true,
    "name": "zfb v0.2.0-rc.1}x"
  },
  {
    "tag_name": "v0.3.0-beta.1",
    "prerelease": true,
    "name": "zfb v0.3.0-beta.1"
  }
]'

_awk_brace_tag=$(printf '%s' "$FIXTURE_BRACE" \
  | awk -v RS='}' '/"prerelease":[[:space:]]*true/ { print; exit }' \
  | grep -m1 '"tag_name":' \
  | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/') || true

if [ -z "$_awk_brace_tag" ]; then
  pass "awk path (brace-injected): correctly produces empty output (fail-loud path fires)"
else
  fail "awk path (brace-injected): expected empty, got '$_awk_brace_tag'"
fi

# ── Summary ───────────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
