#!/usr/bin/env bash
set -euo pipefail

# scripts/smoke-clean-room.sh
#
# Post-publish clean-room smoke: installs create-zfb@<dist-tag> from the real
# registry in a temp dir (no workspace context), scaffolds a project, runs
# `pnpm build` (which calls `zfb build`), and asserts dist/ is populated.
#
# Extracted verbatim from release.yml's `smoke-clean-room` job (issue #1342,
# stage 1 — pure refactor, no logic change) so the same shell logic can be
# reused by both the release-time smoke and a future scheduled drift-net exam.
#
# This is an ALERTING GUARD, not a gate — when invoked from release.yml,
# publish has already happened. Catches the #481-class (broken dist-tag
# pointing at a bad binary — the smoke downloads and runs the just-published
# binary) and the #482-class (missing scaffold dependencies such as
# @takazudo/zfb-runtime). Does NOT catch the #463-class (undeclared esbuild
# peer-dep) because esbuild reaches the build transitively via
# @takazudo/zfb-runtime, so a clean-room pnpm install still resolves it
# through the dependency tree.
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

# Wait until the runner-arch platform tarball is actually fetchable.
# npm metadata propagates faster than the ~78 MB binary tarballs; when
# a tarball hasn't propagated yet, npm SILENTLY SKIPS the optionalDep
# install and the `zfb` launcher fails at runtime.  `npm pack --dry-run`
# forces npm to resolve and fetch the actual tarball (not just metadata),
# confirming it is available before we invoke `npx create-zfb`.
# The runner for this job is ubuntu-latest (linux x64).
echo "Waiting for @takazudo/zfb-linux-x64-gnu@${DIST_TAG} tarball to be fetchable..."
max_attempts=6
delay=10
for attempt in $(seq 1 $max_attempts); do
  if npm pack --dry-run "@takazudo/zfb-linux-x64-gnu@${DIST_TAG}" > /dev/null 2>&1; then
    echo "Registry resolved @takazudo/zfb-linux-x64-gnu@${DIST_TAG} tarball (attempt ${attempt})"
    break
  fi
  if [[ "$attempt" -eq "$max_attempts" ]]; then
    echo "::error::@takazudo/zfb-linux-x64-gnu@${DIST_TAG} tarball did not become fetchable after ${max_attempts} attempts."
    exit 1
  fi
  echo "  Not yet available (attempt ${attempt}/${max_attempts}); retrying in ${delay}s..."
  sleep "$delay"
  delay=$(( delay * 2 ))
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

# ── Assert dist/ is populated ───────────────────────────────────────────────

# find -type f is more robust than `ls -A` (handles dotfile-only outputs).
DIST_FILE=$(find "$SMOKE_DIR/smoke-site/dist" -type f | head -1)
if [[ -z "$DIST_FILE" ]]; then
  echo "::error::smoke-clean-room: dist/ is empty after zfb build — scaffold or build is broken."
  exit 1
fi
FILE_COUNT=$(find "$SMOKE_DIR/smoke-site/dist" -type f | wc -l)
echo "dist/ OK: ${FILE_COUNT} file(s) found. First: $DIST_FILE"
