#!/bin/sh
# shellcheck shell=sh
#
# run.sh — smoke assertions for the node-free zfb distribution.
#
# Steps:
#   1. Scaffold a new site with the node-free template.
#   2. Build the site and assert dist/index.html exists with expected content.
#   3. Start zfb dev in the background on the default port (3000).
#   4. Poll until the dev server is ready (no fixed sleeps).
#   5. Assert HTTP 200 + expected content from the dev server.
#   6. Kill the dev server and exit cleanly.

set -eu

SITE_DIR="/tmp/my-site"
PORT=3000
# Stable substring from the node-free template's index page.
# Must match the rendered output of crates/zfb/templates/node-free/pages/index.tsx.
EXPECTED_CONTENT="node-free"

pass() { printf '[PASS] %s\n' "$1"; }
fail() { printf '[FAIL] %s\n' "$1" >&2; exit 1; }

# ── Step 1: Scaffold ─────────────────────────────────────────────────────────

printf '==> zfb new %s --template node-free\n' "$SITE_DIR"
zfb new "$SITE_DIR" --template node-free
pass "scaffold"

# ── Step 2: Build ────────────────────────────────────────────────────────────

printf '==> zfb build (cwd: %s)\n' "$SITE_DIR"
cd "$SITE_DIR"
zfb build

if [ ! -f dist/index.html ]; then
    fail "dist/index.html not found after build"
fi
pass "dist/index.html exists"

if ! grep -q "$EXPECTED_CONTENT" dist/index.html; then
    fail "dist/index.html does not contain expected content: $EXPECTED_CONTENT"
fi
pass "dist/index.html contains expected content"

# ── Step 3: Start dev server in the background ───────────────────────────────

printf '==> zfb dev (background, port %d)\n' "$PORT"
zfb dev --port "$PORT" &
DEV_PID=$!

# Register cleanup so the dev server is killed even if assertions below fail.
cleanup() {
    if kill -0 "$DEV_PID" 2>/dev/null; then
        kill "$DEV_PID"
    fi
}
trap cleanup EXIT

# ── Step 4: Poll until the dev server responds ───────────────────────────────
# --retry-connrefused: keep retrying on ECONNREFUSED (server not bound yet).
# --retry-delay 1:     wait 1 second between attempts.
# --retry 30:          up to 30 retries (30 s total), satisfying the spec
#                      "poll, do NOT sleep 2" guideline.
# -fsS:                fail on non-2xx, suppress progress, show errors.

printf '==> polling http://localhost:%d/ (up to 30 retries)\n' "$PORT"
DEV_RESPONSE=$(curl --retry 30 --retry-connrefused --retry-delay 1 -fsS "http://localhost:${PORT}/")

# ── Step 5: Assert dev server response ──────────────────────────────────────

if ! printf '%s' "$DEV_RESPONSE" | grep -q "$EXPECTED_CONTENT"; then
    fail "dev server response does not contain expected content: $EXPECTED_CONTENT"
fi
pass "dev server returned expected content"

# cleanup trap kills dev server on exit.
printf '==> All smoke assertions passed.\n'
