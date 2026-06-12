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
#   5c. Edit content/posts/hello.md in place and poll /posts/hello/ until the
#       edit marker appears (dev hot-reload assertion, #1022).
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

# ── Step 1b: Drop page fixtures ─────────────────────────────────────────────
# Add a .md page and a .html page so Step 2's build exercises #408 and #409.
#
# pages/about.md — heading, bold span, relative link.
# The pipeline converts `**bold**` to `<strong>bold</strong>` and keeps the
# `[link](./other)` href as `./other` (no link-rewriting configured in the
# node-free template's zfb.config.json).
cd "$SITE_DIR"
printf '%s\n' \
    '---' \
    'title: About' \
    'lang: en' \
    '---' \
    '' \
    '# Hello' \
    '' \
    'This is **bold** text.' \
    '' \
    'See [link](./other).' \
    > pages/about.md

# pages/legal.html — verbatim HTML body with YAML frontmatter that must be stripped.
printf '%s\n' \
    '---' \
    'title: Legal' \
    '---' \
    '<!doctype html>' \
    '<html lang="en">' \
    '<head><meta charset="utf-8"><title>Legal</title></head>' \
    '<body><h1>Legal Notice</h1><p>This is the legal page verbatim content.</p></body>' \
    '</html>' \
    > pages/legal.html

pass "page fixtures written"

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

# Body markdown must render to HTML. `**node-free**` in hello.md must appear
# as `<strong>node-free</strong>`, not as literal asterisks.
if ! grep -qF '<strong>node-free</strong>' dist/posts/hello/index.html; then
    fail "dist/posts/hello/index.html does not contain rendered body markup: <strong>node-free</strong>"
fi
pass "dist/posts/hello/index.html contains rendered body markup"

# ── Step 2b: Assert .md page (#408) ─────────────────────────────────────────
# dist/about/index.html must exist and contain the rendered markdown body.
# Assertions are pinned to the exact tags emitted by the pipeline (observed
# by running `zfb build` locally on a fixture with the same source content).

if [ ! -f dist/about/index.html ]; then
    fail "dist/about/index.html not found after build (#408 regression)"
fi
pass "dist/about/index.html exists"

# `# Hello` → <h1>Hello</h1>  (MDX heading compile)
if ! grep -qF '<h1>Hello</h1>' dist/about/index.html; then
    fail "dist/about/index.html does not contain <h1>Hello</h1> (markdown heading must be compiled)"
fi
pass "dist/about/index.html contains <h1>Hello</h1>"

# `**bold**` → <strong>bold</strong>  (GFM strong emphasis)
if ! grep -qF '<strong>bold</strong>' dist/about/index.html; then
    fail "dist/about/index.html does not contain <strong>bold</strong> (markdown strong must be compiled)"
fi
pass "dist/about/index.html contains <strong>bold</strong>"

# `[link](./other)` → <a href="./other">link</a>
# The node-free template has no resolve_markdown_links config, so the href
# is emitted verbatim as ./other (observed locally).
if ! grep -qF '<a href="./other">link</a>' dist/about/index.html; then
    fail "dist/about/index.html does not contain <a href=\"./other\">link</a>"
fi
pass "dist/about/index.html contains <a href=\"./other\">link</a>"

# ── Step 2c: Assert .html page (#409) ───────────────────────────────────────
# dist/legal/index.html must exist, have its YAML frontmatter stripped, and
# contain the verbatim HTML body.

if [ ! -f dist/legal/index.html ]; then
    fail "dist/legal/index.html not found after build (#409 regression)"
fi
pass "dist/legal/index.html exists"

# Frontmatter must be stripped.
if grep -q '^title: Legal' dist/legal/index.html; then
    fail "dist/legal/index.html contains raw frontmatter (strip_static_html_frontmatter not applied)"
fi
pass "dist/legal/index.html has no raw frontmatter"

# Verbatim HTML body must be present.
if ! grep -qF '<h1>Legal Notice</h1>' dist/legal/index.html; then
    fail "dist/legal/index.html does not contain verbatim <h1>Legal Notice</h1>"
fi
pass "dist/legal/index.html contains verbatim HTML body"

# ── Step 3: Start dev server in the background ───────────────────────────────

printf '==> zfb dev (background, port %d)\n' "$PORT"
DEV_LOG=$(mktemp)
zfb dev --port "$PORT" 2>"$DEV_LOG" &
DEV_PID=$!

# Register cleanup so the dev server is killed even if assertions below fail.
# On a failing exit, dump the dev server's captured stderr first — every
# assertion below runs against the live server, so its log is the primary
# diagnostic for any failure from this point on.
cleanup() {
    STATUS=$?
    if kill -0 "$DEV_PID" 2>/dev/null; then
        kill "$DEV_PID"
    fi
    if [ "$STATUS" -ne 0 ] && [ -s "$DEV_LOG" ]; then
        printf '%s\n' '--- zfb dev stderr (last 50 lines) ---' >&2
        tail -n 50 "$DEV_LOG" >&2
    fi
    rm -f "$DEV_LOG"
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

# ── Step 5b: Assert dev server serves dynamic /posts/hello/ slug ─────────────
# Verifies #502: paths()-expanded dynamic routes are served at HTTP 200 by
# `zfb dev`. The trailing-slash form is used here, symmetric to the build-side
# assertion for dist/posts/hello/index.html above.

printf '==> curling http://localhost:%d/posts/hello/ (dynamic slug)\n' "$PORT"
DEV_POST_RESPONSE=$(curl --retry 30 --retry-connrefused --retry-delay 1 -fsS "http://localhost:${PORT}/posts/hello/")

if ! printf '%s' "$DEV_POST_RESPONSE" | grep -q "$EXPECTED_POST_TITLE"; then
    fail "dev server /posts/hello/ does not contain expected post title: $EXPECTED_POST_TITLE"
fi
pass "dev server served dynamic slug /posts/hello/ with expected post title"

# ── Step 5c: Edit content file and assert dev server reflects the change ──────
# Appends a unique marker to the seed post and polls /posts/hello/ until the
# marker appears in the response body (proving dev-mode hot-reload reaches the
# release binary's serve path). The initial curl assertions above guarantee the
# server is fully up before we touch the file.
#
# Poll deadline: 60 s wall-clock — well within the 6-minute job timeout. The
# deadline is checked against `date +%s` (not an attempt counter) so a hung
# curl (bounded at --max-time 5) cannot stretch the wait past the budget.
# On timeout, fail() exits non-zero and the cleanup trap dumps dev stderr.

EDIT_MARKER="smoke-edit-marker-$$"
printf '==> appending edit marker to content/posts/hello.md\n'
printf '\n%s\n' "$EDIT_MARKER" >> "${SITE_DIR}/content/posts/hello.md"

EDIT_DEADLINE=60
printf '==> polling http://localhost:%d/posts/hello/ for edit marker (deadline %ds)\n' "$PORT" "$EDIT_DEADLINE"
EDIT_START=$(date +%s)
EDIT_FOUND=0
while :; do
    EDIT_RESPONSE=$(curl -fsS --max-time 5 "http://localhost:${PORT}/posts/hello/" 2>/dev/null || true)
    if printf '%s' "$EDIT_RESPONSE" | grep -q "$EDIT_MARKER"; then
        EDIT_FOUND=1
        break
    fi
    if [ $(( $(date +%s) - EDIT_START )) -ge "$EDIT_DEADLINE" ]; then
        break
    fi
    sleep 1
done

if [ "$EDIT_FOUND" -eq 0 ]; then
    fail "dev server did not reflect the content edit within ${EDIT_DEADLINE}s (marker: $EDIT_MARKER)"
fi
pass "dev server reflected content edit (marker served after ~$(( $(date +%s) - EDIT_START ))s)"

# cleanup trap kills dev server on exit.
printf '==> All smoke assertions passed.\n'
