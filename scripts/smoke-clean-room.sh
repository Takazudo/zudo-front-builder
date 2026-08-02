#!/usr/bin/env bash
set -euo pipefail

# scripts/smoke-clean-room.sh
#
# Post-publish clean-room smoke: installs create-zfb@<dist-tag> from the real
# registry in a temp dir (no workspace context), scaffolds a project, runs
# `pnpm build` (which calls `zfb build`), and asserts dist/ is populated.
#
# Extracted from release.yml's `smoke-clean-room` job (issue #1342) so the
# same shell logic is reused by both the release-time smoke (via
# reusable-smoke-clean-room.yml) and the weekly drift-net.yml off-release
# exam.
#
# This is an ALERTING GUARD, not a gate — when invoked from release.yml,
# publish has already happened. Catches the #481-class (broken dist-tag
# pointing at a bad binary — the smoke downloads and runs the just-published
# binary), the #482-class (missing scaffold dependencies such as
# @takazudo/zfb-runtime), and the #1325-class (a platform's optionalDependency
# tarball not yet propagated to the registry — npm SILENTLY SKIPS an
# optionalDep whose tarball 404s, so on any given platform the failure only
# shows up as a broken `zfb` launcher at install time, not as an install
# error). Does NOT catch the #463-class (undeclared esbuild peer-dep) because
# esbuild reaches the build transitively via @takazudo/zfb-runtime, so a
# clean-room pnpm install still resolves it through the dependency tree.
#
# Usage:
#   DIST_TAG=next scripts/smoke-clean-room.sh
#
# Required env:
#   DIST_TAG — npm dist-tag to install (e.g. "latest" or "next").

: "${DIST_TAG:?DIST_TAG env var is required (e.g. DIST_TAG=next)}"

# ── Wait for registry propagation and verify dist-tag resolves ─────────────

echo "Waiting for create-zfb@${DIST_TAG} to appear on the registry..."
max_attempts=6
delay=10
for attempt in $(seq 1 $max_attempts); do
  RESOLVED=$(npm view "create-zfb@${DIST_TAG}" version 2>/dev/null || echo "")
  if [[ -n "$RESOLVED" ]]; then
    echo "Registry resolved create-zfb@${DIST_TAG} -> ${RESOLVED} (attempt ${attempt})"
    break
  fi
  if [[ "$attempt" -eq "$max_attempts" ]]; then
    echo "::error::create-zfb@${DIST_TAG} did not appear on the registry after ${max_attempts} attempts."
    exit 1
  fi
  echo "  Not yet available (attempt ${attempt}/${max_attempts}); retrying in ${delay}s..."
  sleep "$delay"
  delay=$(( delay * 2 ))
done

# Wait until EVERY platform's optionalDependency tarball is actually
# fetchable — not just the tarball the current runner needs. npm metadata
# propagates faster than the ~78 MB binary tarballs; when a tarball hasn't
# propagated yet, npm SILENTLY SKIPS the optionalDep install and the `zfb`
# launcher fails at runtime on THAT platform (the #1325 incident class).
# `npm pack --dry-run` forces npm to resolve and fetch the actual tarball
# (not just metadata), confirming it is available.
#
# `npm pack --dry-run` only touches the registry (no OS-specific behavior),
# so this loop probes all 5 platforms from a single runner regardless of
# which OS is executing this script — that's the whole point: today's job
# only ever ran on ubuntu-latest, so nothing ever probed whether e.g. the
# darwin-arm64 or win32 tarball had propagated. This loop closes that gap.
PLATFORM_PACKAGES=(
  "@takazudo/zfb-darwin-arm64"
  "@takazudo/zfb-darwin-x64"
  "@takazudo/zfb-linux-arm64-gnu"
  "@takazudo/zfb-linux-x64-gnu"
  "@takazudo/zfb-win32-x64-msvc"
)
for pkg in "${PLATFORM_PACKAGES[@]}"; do
  echo "Waiting for ${pkg}@${DIST_TAG} tarball to be fetchable..."
  max_attempts=6
  delay=10
  for attempt in $(seq 1 $max_attempts); do
    if npm pack --dry-run "${pkg}@${DIST_TAG}" > /dev/null 2>&1; then
      echo "Registry resolved ${pkg}@${DIST_TAG} tarball (attempt ${attempt})"
      break
    fi
    if [[ "$attempt" -eq "$max_attempts" ]]; then
      echo "::error::${pkg}@${DIST_TAG} tarball did not become fetchable after ${max_attempts} attempts."
      exit 1
    fi
    echo "  Not yet available (attempt ${attempt}/${max_attempts}); retrying in ${delay}s..."
    sleep "$delay"
    delay=$(( delay * 2 ))
  done
done

# ── Scaffold project with create-zfb@<dist-tag> ─────────────────────────────

# Work in a temp dir completely outside the checked-out workspace to
# avoid pnpm picking up the monorepo's pnpm-workspace.yaml.
SMOKE_DIR=$(mktemp -d)
echo "Scaffolding into $SMOKE_DIR ..."
cd "$SMOKE_DIR"
# Belt-and-suspenders: retry the scaffold up to 3 times with backoff in
# case of a momentary registry blip after the tarball-propagation wait.
# `create-zfb <name>` is the non-interactive form (positional arg, no prompts).
# --yes suppresses any interactive npm/npx prompts.
scaffold_delay=15
for scaffold_attempt in 1 2 3; do
  if npx --yes "create-zfb@${DIST_TAG}" smoke-site; then
    break
  fi
  if [[ "$scaffold_attempt" -eq 3 ]]; then
    echo "::error::npx create-zfb@${DIST_TAG} failed after 3 attempts."
    exit 1
  fi
  echo "  Scaffold attempt ${scaffold_attempt}/3 failed; retrying in ${scaffold_delay}s..."
  sleep "$scaffold_delay"
  scaffold_delay=$(( scaffold_delay * 2 ))
done
echo "Scaffold complete. Contents of $SMOKE_DIR/smoke-site:"
ls -la "$SMOKE_DIR/smoke-site"

# ── Install scaffolded project dependencies ─────────────────────────────────

cd "$SMOKE_DIR/smoke-site"
pnpm install

# ── Build scaffolded project ────────────────────────────────────────────────

cd "$SMOKE_DIR/smoke-site"
pnpm build

# ── Assert dist/ is populated and contains expected template content ───────
#
# Mirrors the assertion style used by tests/smoke/node-free/run.sh: beyond
# "dist/ is non-empty", grep the rendered HTML for markers that can only be
# present if the content pipeline actually ran (not just that some file
# happened to be written). `create-zfb <name>` (no --template) scaffolds the
# "basic-blog" template (crates/zfb/src/cli.rs default_value), whose
# pages/index.tsx renders an <h1> reading "basic-blog" and lists every post
# in the `blog` content collection, including the seed post's frontmatter
# title "Hello, zfb" (content/blog/hello-zfb.mdx) — so both markers only
# appear if getCollection("blog") resolved AND the page template rendered
# correctly. Both greps match on text, not markup: the heading carries
# Tailwind utility classes, so the literal tag string is not in the HTML.

pass() { printf '[PASS] %s\n' "$1"; }
fail() { printf '[FAIL] %s\n' "$1" >&2; exit 1; }

DIST_INDEX="$SMOKE_DIR/smoke-site/dist/index.html"

# find -type f is more robust than `ls -A` (handles dotfile-only outputs).
DIST_FILE=$(find "$SMOKE_DIR/smoke-site/dist" -type f | head -1)
if [[ -z "$DIST_FILE" ]]; then
  fail "smoke-clean-room: dist/ is empty after zfb build — scaffold or build is broken."
fi
FILE_COUNT=$(find "$SMOKE_DIR/smoke-site/dist" -type f | wc -l)
pass "dist/ is populated (${FILE_COUNT} file(s) found, first: $DIST_FILE)"

if [[ ! -f "$DIST_INDEX" ]]; then
  fail "smoke-clean-room: dist/index.html not found after build."
fi
pass "dist/index.html exists"

if ! grep -q "basic-blog" "$DIST_INDEX"; then
  fail "smoke-clean-room: dist/index.html does not contain expected content: basic-blog"
fi
pass "dist/index.html contains expected content (basic-blog)"

if ! grep -q "Hello, zfb" "$DIST_INDEX"; then
  fail "smoke-clean-room: dist/index.html does not list the seed post title: Hello, zfb"
fi
pass "dist/index.html lists the seed post (Hello, zfb)"
