#!/usr/bin/env bash
set -euo pipefail

# scripts/smoke-clean-room.sh
#
# Post-publish clean-room smoke: resolves create-zfb@<dist-tag> from the real
# registry, then installs that exact version in the clean-room project.
# In a temp dir (no workspace context), it scaffolds a project, runs
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
# The v4.1.0 failure was release run 37618923936 (Node 22.23.3 / npm 10.9.9):
# its Linux-x64 tarball outlasted the old retries; that run does not establish a cached-404 cause.
# npm 10.9.9 bundles make-fetch-happen 14.0.3, which stores only 200/301/308 responses;
# stale successful packuments or registry/CDN caching remain plausible propagation causes.
# Two tiers separate metadata lag from tarball lag: npm view finds the exact URL, then cache-free curl checks HTTP availability.
# The one-byte probe proves availability only; the later scaffold install remains the tarball integrity check.
#
# Usage:
#   DIST_TAG=next EXPECTED_VERSION=2.20.3 scripts/smoke-clean-room.sh
#
# Required env:
#   DIST_TAG — npm dist-tag to resolve (e.g. "latest" or "next").
# Optional EXPECTED_VERSION pins a release smoke to the verified release tag.
# Optional SMOKE_PROPAGATION_DEADLINE_SECONDS (default 900) bounds version and
# platform propagation together; SMOKE_NOW_CMD can inject an epoch-seconds clock.

: "${DIST_TAG:?DIST_TAG env var is required (e.g. DIST_TAG=next)}"
semver_re='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?(\+[0-9A-Za-z][0-9A-Za-z.-]*)?$'
if [[ -n "${EXPECTED_VERSION:-}" && ! "$EXPECTED_VERSION" =~ $semver_re ]]; then
  echo "::error::Invalid EXPECTED_VERSION: ${EXPECTED_VERSION}" >&2
  exit 1
fi

# ── Start one deadline for version, metadata, and every platform tarball ──

deadline_raw=${SMOKE_PROPAGATION_DEADLINE_SECONDS-900}
if [[ ! "$deadline_raw" =~ ^[0-9]+$ || ${#deadline_raw} -gt 18 ]]; then
  echo "::error::SMOKE_PROPAGATION_DEADLINE_SECONDS must be a positive base-10 integer (1..999999999999999999); got '${deadline_raw}'." >&2
  exit 1
fi
deadline_seconds=$((10#$deadline_raw))
if (( deadline_seconds <= 0 )); then
  echo "::error::SMOKE_PROPAGATION_DEADLINE_SECONDS must be greater than 0; got '${deadline_raw}'." >&2
  exit 1
fi

SMOKE_NOW_CMD=${SMOKE_NOW_CMD:-date}
now_seconds() { "$SMOKE_NOW_CMD" +%s; }
if ! PROPAGATION_STARTED_AT=$(now_seconds) || [[ ! "$PROPAGATION_STARTED_AT" =~ ^[0-9]+$ ]]; then
  echo "::error::SMOKE_NOW_CMD must print epoch seconds; got '${PROPAGATION_STARTED_AT:-unavailable}'." >&2
  exit 1
fi
PROPAGATION_DEADLINE=$((PROPAGATION_STARTED_AT + deadline_seconds))
propagation_elapsed() {
  local now
  now=$(now_seconds)
  echo $((now - PROPAGATION_STARTED_AT))
}
propagation_remaining() {
  local now remaining
  now=$(now_seconds)
  remaining=$((PROPAGATION_DEADLINE - now))
  (( remaining > 0 )) || remaining=0
  echo "$remaining"
}
sleep_within_deadline() {
  local requested=$1 remaining
  remaining=$(propagation_remaining)
  (( remaining > 0 )) || return 0
  (( requested > remaining )) && requested=$remaining
  echo "  Retrying in ${requested}s..."
  sleep "$requested"
}

# ── Wait for registry propagation and verify dist-tag resolves ─────────────

echo "Waiting for create-zfb@${DIST_TAG} to appear on the registry..."
while :; do
  remaining=$(propagation_remaining)
  if (( remaining == 0 )); then
    elapsed=$(propagation_elapsed)
    echo "::error::create-zfb@${DIST_TAG} did not resolve to ${EXPECTED_VERSION:-a valid version} before the shared propagation deadline (elapsed ${elapsed}s, last response: ${RESOLVED:-unavailable})." >&2
    exit 1
  fi
  if ! RESOLVED=$(npm view "create-zfb@${DIST_TAG}" version 2>/dev/null); then
    RESOLVED=""
  fi
  remaining=$(propagation_remaining)
  if (( remaining == 0 )); then
    elapsed=$(propagation_elapsed)
    echo "::error::create-zfb@${DIST_TAG} version metadata arrived after the shared propagation deadline (elapsed ${elapsed}s, last response: ${RESOLVED:-unavailable})." >&2
    exit 1
  fi
  if [[ "$RESOLVED" =~ $semver_re && ( -z "${EXPECTED_VERSION:-}" || "$RESOLVED" == "$EXPECTED_VERSION" ) ]]; then
    echo "Registry resolved create-zfb@${DIST_TAG} -> ${RESOLVED}"
    break
  fi
  version_failure=${RESOLVED:-unavailable}
  elapsed=$(propagation_elapsed)
  echo "  Version metadata unavailable (elapsed ${elapsed}s, last: ${version_failure}); waiting within the shared deadline."
  sleep_within_deadline 10
done
VERSION=${EXPECTED_VERSION:-$RESOLVED}

# Wait until EVERY platform's optionalDependency tarball is available, not
# only the tarball for the current runner. npm may silently skip an unavailable
# optionalDependency, leaving that platform with a broken `zfb` launcher.
# Each platform's metadata lookup yields its exact URL; an uncached ranged
# curl request probes that URL from this runner regardless of its OS.
PLATFORM_PACKAGES=(
  "@takazudo/zfb-darwin-arm64"
  "@takazudo/zfb-darwin-x64"
  "@takazudo/zfb-linux-arm64-gnu"
  "@takazudo/zfb-linux-x64-gnu"
  "@takazudo/zfb-win32-x64-msvc"
)
for pkg in "${PLATFORM_PACKAGES[@]}"; do
  echo "Waiting for ${pkg}@${VERSION} tarball to be fetchable..."
  metadata_url=""
  last_metadata_failure="npm view failed"
  while [[ -z "$metadata_url" ]]; do
    remaining=$(propagation_remaining)
    if (( remaining == 0 )); then
      elapsed=$(propagation_elapsed)
      echo "::error::${pkg}@${VERSION} metadata unavailable (elapsed ${elapsed}s, last: ${last_metadata_failure})." >&2
      exit 1
    fi
    if metadata_url=$(npm view "${pkg}@${VERSION}" dist.tarball 2>/dev/null); then
      if [[ -z "${metadata_url//[[:space:]]/}" ]]; then
        metadata_url=""
        last_metadata_failure="empty dist.tarball"
      fi
    else
      metadata_url=""
      last_metadata_failure="npm view failed"
    fi
    remaining=$(propagation_remaining)
    if [[ -n "$metadata_url" ]] && (( remaining == 0 )); then
      elapsed=$(propagation_elapsed)
      echo "::error::${pkg}@${VERSION} metadata resolved after the shared propagation deadline (elapsed ${elapsed}s; last: dist.tarball URL returned too late)." >&2
      exit 1
    fi
    if [[ -z "$metadata_url" ]]; then
      elapsed=$(propagation_elapsed)
      echo "  ${pkg}@${VERSION} metadata unavailable (elapsed ${elapsed}s, last: ${last_metadata_failure})"
      if (( remaining == 0 )); then
        echo "::error::${pkg}@${VERSION} metadata unavailable (elapsed ${elapsed}s, last: ${last_metadata_failure})." >&2
        exit 1
      fi
      sleep_within_deadline 10
    fi
  done

  last_http_code=000
  while :; do
    remaining=$(propagation_remaining)
    if (( remaining == 0 )); then
      elapsed=$(propagation_elapsed)
      echo "::error::${pkg}@${VERSION} metadata present, tarball unavailable (elapsed ${elapsed}s, last HTTP ${last_http_code})." >&2
      exit 1
    fi
    curl_status=0
    http_code=$(curl -fsSL --connect-timeout 10 --max-time 60 -r 0-0 -o /dev/null -w '%{http_code}' "$metadata_url" 2>/dev/null) || curl_status=$?
    remaining=$(propagation_remaining)
    if (( curl_status == 0 )) && [[ "$http_code" == 200 || "$http_code" == 206 ]]; then
      if (( remaining == 0 )); then
        elapsed=$(propagation_elapsed)
        echo "::error::${pkg}@${VERSION} tarball probe finished after the shared propagation deadline (elapsed ${elapsed}s, last HTTP ${http_code})." >&2
        exit 1
      fi
      echo "Registry tarball probe passed for ${pkg}@${VERSION} (HTTP ${http_code})"
      break
    fi
    last_http_code=${http_code:-000}
    elapsed=$(propagation_elapsed)
    echo "  ${pkg}@${VERSION} metadata present, tarball unavailable (elapsed ${elapsed}s, last HTTP ${last_http_code})"
    if (( remaining == 0 )); then
      echo "::error::${pkg}@${VERSION} metadata present, tarball unavailable (elapsed ${elapsed}s, last HTTP ${last_http_code})." >&2
      exit 1
    fi
    sleep_within_deadline 10
  done
done

# ── Scaffold project with the pinned create-zfb version ─────────────────────

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
  if npx --yes "create-zfb@${VERSION}" smoke-site; then
    break
  fi
  if [[ "$scaffold_attempt" -eq 3 ]]; then
    echo "::error::npx create-zfb@${VERSION} failed after 3 attempts."
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

# The generated manifest and registry may have drifted independently of the
# pinned generator. Check the installed packages and the binary we will run.
for pkg in @takazudo/zfb @takazudo/zfb-runtime; do
  manifest="node_modules/${pkg}/package.json"
  if [[ ! -f "$manifest" ]]; then
    echo "::error::Missing installed ${pkg} package." >&2
    exit 1
  fi
  installed=$(node -p "require('./${manifest}').version")
  if [[ "$installed" != "$VERSION" ]]; then
    echo "::error::Installed ${pkg}@${installed}; expected ${VERSION}." >&2
    exit 1
  fi
done
cli_version=$(pnpm exec zfb -V)
if [[ "$cli_version" != "zfb ${VERSION}" ]]; then
  echo "::error::Installed zfb CLI reports '${cli_version}'; expected 'zfb ${VERSION}'." >&2
  exit 1
fi

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
# utility CSS classes, so the literal tag string is not in the HTML.

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
