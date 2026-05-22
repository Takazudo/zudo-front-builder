#!/bin/sh
# shellcheck shell=sh
#
# run.sh — smoke assertions for the node-free zfb distribution.
#
# Steps:
#   1. Scaffold a new site with the node-free template.
#   2. Build the site and assert dist/index.html exists with expected content,
#      AND that the `posts` content collection produced dist/posts/<slug>/index.html
#      with the post title from the markdown frontmatter.
#   3. Start zfb dev in the background on the default port (3000).
#   4. Poll until the dev server is ready (no fixed sleeps).
#   5. Assert HTTP 200 + expected content from the dev server.
#   6. Kill the dev server and exit cleanly.

set -eu

# zfb new rejects absolute paths and path separators (validate_project_name in
# crates/zfb/src/commands/new.rs), so the site must be scaffolded under a
# pre-chosen workdir using a single-segment relative name.
SITE_WORKDIR="/tmp"
SITE_NAME="my-site"
SITE_DIR="${SITE_WORKDIR}/${SITE_NAME}"
PORT=3000
# Stable substring from the node-free template's index page.
# Must match the rendered output of crates/zfb/templates/node-free/pages/index.tsx.
EXPECTED_CONTENT="node-free"
# Title from the seed post `content/posts/hello.md`. Surfaces in both
# `dist/index.html` (the homepage post list) and the dedicated per-post page
# `dist/posts/hello/index.html`. The presence of this string in the per-post
# HTML proves that `getCollection("posts")` resolved correctly under the
# embedded V8 host (i.e. without a Node `node:fs` fallback).
EXPECTED_POST_TITLE="Hello, zfb"

pass() { printf '[PASS] %s\n' "$1"; }
fail() { printf '[FAIL] %s\n' "$1" >&2; exit 1; }

# ── Step 1: Scaffold ─────────────────────────────────────────────────────────

printf '==> zfb new %s --template node-free (cwd: %s)\n' "$SITE_NAME" "$SITE_WORKDIR"
cd "$SITE_WORKDIR"
# Idempotency for local re-runs; CI runs in a fresh container so SITE_DIR is absent.
rm -rf "$SITE_NAME"
zfb new "$SITE_NAME" --template node-free
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

# Homepage must list the seed post (proving the index page's
# `getCollection("posts")` call resolved entries from the snapshot).
if ! grep -q "$EXPECTED_POST_TITLE" dist/index.html; then
    fail "dist/index.html does not contain expected post title: $EXPECTED_POST_TITLE"
fi
pass "dist/index.html lists the seed post"

# The dynamic `pages/posts/[slug].tsx` route must materialise into a
# per-post HTML file. Static-site-generation expands `[slug]` via `paths()`,
# which calls `getCollection("posts")` — so a missing file or missing
# title here means the snapshot didn't reach the route at SSG time.
if [ ! -f dist/posts/hello/index.html ]; then
    fail "dist/posts/hello/index.html not found after build"
fi
pass "dist/posts/hello/index.html exists"

if ! grep -q "$EXPECTED_POST_TITLE" dist/posts/hello/index.html; then
    fail "dist/posts/hello/index.html does not contain post title: $EXPECTED_POST_TITLE"
fi
pass "dist/posts/hello/index.html contains the post title"

# The post body must be rendered as HTML via `<post.Content components={...} />`,
# NOT as raw markdown source. `hello.md` contains `**node-free**` (bold). After
# Content rendering, this surfaces as `<strong>node-free</strong>` (the
# defaultComponents `<strong>` passthrough). If the page accidentally renders
# `post.body` as text, the output would contain the literal `**node-free**`
# instead. Asserting on the rendered `<strong>` tag locks the Content-rendering
# pattern in and guards against a regression to raw-body rendering.
if ! grep -q "<strong>node-free</strong>" dist/posts/hello/index.html; then
    fail "dist/posts/hello/index.html does not render markdown body as HTML (expected <strong>node-free</strong>)"
fi
pass "dist/posts/hello/index.html renders markdown body through entry.Content"

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
