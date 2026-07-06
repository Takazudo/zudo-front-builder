#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/install-default-version.sh — offline test for install.sh's
# default (no ZFB_VERSION set) version-resolution branch, resolve_tag()'s
# `if [ -z "$_version" ]; then ... fi` case (install.sh ~lines 86-94).
#
# Unlike tests/unit/install-prerelease.sh (which re-implements the
# latest-prerelease extraction pipeline inline from fixture strings), this
# test exercises install.sh's ACTUAL resolve_tag() function: it sources a
# throwaway copy of install.sh with the trailing `main` invocation stripped
# (install.sh itself is untouched — main() would otherwise run for real on
# source), then calls resolve_tag with a mock `curl` on PATH. Mocking
# happens at the network boundary the API_BASE constant points to — install.sh
# always invokes exactly `curl -fsSL "${API_BASE}/latest"` in this branch, so
# the mock curl only needs to answer that one call.
#
# The "current reality" case matters because it is a real, verified bug users
# hit TODAY: GitHub's GET /repos/:owner/:repo/releases/latest endpoint
# explicitly excludes prereleases/drafts and returns HTTP 404 when no
# qualifying release exists — which is exactly this repo's state right now
# (every release published so far is a prerelease, e.g. v0.1.0-next.76).
# Verified live via `gh api repos/Takazudo/zudo-front-builder/releases/latest`,
# which returns:
#   {"message":"Not Found","documentation_url":"https://docs.github.com/rest/releases/releases#get-the-latest-release","status":"404"}
# curl -f suppresses the response body on an HTTP error status, so this test's
# mock (mode=no_stable_release) reproduces that: empty stdout, non-zero exit.
# Proves resolve_tag's `if [ -z "$_tag" ]; then err ...; fi` fail-loud path
# fires instead of silently proceeding with a bogus tag. This is the failure
# mode users currently walk into running the installer with no ZFB_VERSION
# set (fixed docs-side by issue #1396, which points users at
# ZFB_VERSION=latest-prerelease instead).
#
# A second case (mode=stable_release) is a forward-looking contrast: once a
# stable release exists, the same extraction correctly resolves its tag — so
# this test doesn't just pin today's failure, it also guards the eventual
# success path from regressing.
#
# Requires: sh, bash (resolve_tag is sourced and called under bash — same
# sourcing technique as tests/unit/publish-npm-packages.sh), mktemp, grep.
# Runs entirely offline — curl is mocked; no real network call is made.
#
# Run:
#   sh tests/unit/install-default-version.sh

set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
cd "$REPO_ROOT"

SCRIPT="install.sh"

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

if [ ! -f "$SCRIPT" ]; then
  fail "script not found: $SCRIPT"
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# install.sh's last line is a bare, unconditional `main` invocation (it has no
# BASH_SOURCE-style sourcing guard, unlike scripts/publish-npm-packages.sh) —
# strip exactly that line so sourcing the copy only defines functions/
# constants. Matched by exact line content (`^main$`), not line position, so
# this does not silently no-op if install.sh ever grows a trailing blank line.
if [ "$(grep -c '^main$' "$SCRIPT")" -ne 1 ]; then
  fail "install.sh does not have exactly one bare 'main' invocation line — this test's assumption (see comment above) is stale, update it"
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi
INSTALL_LIB="$WORK/install-lib.sh"
grep -v '^main$' "$SCRIPT" >"$INSTALL_LIB"

MOCK_BIN="$WORK/bin"
mkdir -p "$MOCK_BIN"

cat >"$MOCK_BIN/curl" <<'MOCK_CURL'
#!/bin/sh
# Mock curl for install.sh's resolve_tag() default branch: that branch always
# invokes `curl -fsSL "${API_BASE}/latest"` with no other query, so this mock
# ignores its arguments entirely and answers based on MOCK_CURL_MODE alone.
case "${MOCK_CURL_MODE:-}" in
  no_stable_release)
    # Real curl -f suppresses the response body on an HTTP error status
    # (here, GitHub's real 404 for "no non-prerelease release exists").
    exit 22
    ;;
  stable_release)
    printf '%s' '{"tag_name":"v1.0.0","prerelease":false,"name":"zfb v1.0.0"}'
    exit 0
    ;;
  *)
    echo "mock curl: unset/unknown MOCK_CURL_MODE" >&2
    exit 1
    ;;
esac
MOCK_CURL
chmod +x "$MOCK_BIN/curl"

# ── Case 1: current reality — no stable release exists (404) ───────────────

STDERR_1="$WORK/stderr_1"
if OUT_1=$(PATH="$MOCK_BIN:$PATH" MOCK_CURL_MODE=no_stable_release INSTALL_LIB="$INSTALL_LIB" \
  bash -c '. "$INSTALL_LIB"; resolve_tag' 2>"$STDERR_1"); then
  RC_1=0
else
  RC_1=$?
fi

if [ "$RC_1" -eq 1 ] && [ -z "${OUT_1:-}" ] && grep -q "could not resolve the latest release tag from GitHub API" "$STDERR_1"; then
  pass "resolve_tag (no ZFB_VERSION, no stable release): fails loudly with the expected error"
else
  fail "resolve_tag (no ZFB_VERSION, no stable release): expected exit 1 + empty stdout + fail-loud stderr, got exit=$RC_1 stdout='${OUT_1:-}' stderr='$(cat "$STDERR_1")'"
fi

# ── Case 2: contrast — a stable release exists ──────────────────────────────

STDERR_2="$WORK/stderr_2"
if OUT_2=$(PATH="$MOCK_BIN:$PATH" MOCK_CURL_MODE=stable_release INSTALL_LIB="$INSTALL_LIB" \
  bash -c '. "$INSTALL_LIB"; resolve_tag' 2>"$STDERR_2"); then
  RC_2=0
else
  RC_2=$?
fi

if [ "$RC_2" -eq 0 ] && [ "$OUT_2" = "v1.0.0" ]; then
  pass "resolve_tag (no ZFB_VERSION, stable release exists): resolves the correct tag"
else
  fail "resolve_tag (no ZFB_VERSION, stable release exists): expected exit 0 + 'v1.0.0', got exit=$RC_2 stdout='${OUT_2:-}' stderr='$(cat "$STDERR_2")'"
fi

# ── Summary ───────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
