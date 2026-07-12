#!/usr/bin/env bash
#
# publish-npm-packages.sh — publish every zfb workspace package to npm,
# idempotently and tolerant of npm's "double-PUT" race.
#
# WHY THIS EXISTS (incident: v0.1.0-next.42 partial publish)
# ---------------------------------------------------------------------------
# npm's publish PUT is not transactional from the client's point of view. A
# network blip can lose the success ACK *after* the registry has already stored
# the version; the client then retries and the registry answers:
#
#     403 ... You cannot publish over the previously published versions: X.Y.Z
#
# Under `set -e` that 403 aborted the whole release job mid-way, so v0.1.0-next.42
# published 7 of 9 packages and left @takazudo/zfb-runtime + create-zfb behind —
# and the job could NOT be safely re-run, because every already-published package
# 403s again on a retry. This script removes both failure modes by making each
# package's publish:
#
#   1. IDEMPOTENT       — skip if <name>@<version> is already on the registry, so
#                         a partial publish can simply be re-run to completion.
#   2. CONFLICT-TOLERANT — treat the SPECIFIC "cannot publish over the previously
#                         published versions" / EPUBLISHCONFLICT error (and a
#                         post-failure registry recheck) as a SKIP, while still
#                         FAILING the job on any other publish error (auth,
#                         validation, hard network failure, ...).
#
# Platform packages (zfb-darwin-*, zfb-linux-*, zfb-win32-*) are published with
# `npm publish` (NOT pnpm): pnpm pack normalises file modes to 0644, stripping
# the 0755 exec bit off the `zfb` binary; npm preserves it (issue #447 / #441).
# The `( cd "$dir" && npm publish ... )` invocation below is kept byte-identical
# to the original for that reason.
#
# Non-platform packages (@takazudo/zfb, @takazudo/zfb-runtime,
# @takazudo/zfb-adapter-cloudflare, create-zfb) are published with
# `pnpm -r --filter <name> publish` so pnpm rewrites their `workspace:*` deps to
# concrete versions at pack time. Published one package per call (a recursive
# publish scoped to a single --filter) rather than one bulk `pnpm -r publish`
# over the whole set, so the idempotent/tolerant wrapper applies per package and
# a mid-list conflict cannot abort the rest.
#
# Usage:
#   scripts/publish-npm-packages.sh <all-provenance|mac-local>
#     all-provenance — every package (incl. zfb-darwin-x64) was built in CI, so
#                      all carry OIDC-attested --provenance.
#     mac-local      — zfb-darwin-x64 was built locally (outside this GHA job) and
#                      cannot carry OIDC provenance, so it is published WITHOUT
#                      --provenance; every other package keeps --provenance.
#
# Required env:
#   DIST_TAG          npm dist-tag to publish under (next | latest).
#   NODE_AUTH_TOKEN   npm auth token (set by the calling workflow step's env).
# Optional env:
#   ALSO_LATEST       "1" → after publishing, also advance the `latest` dist-tag
#                     (prerelease dual-tag policy, see #481). Delegated to
#                     scripts/advance-latest-dist-tag.sh, which has its own retry.
#   PUBLISH_RECHECK_ATTEMPTS / PUBLISH_RECHECK_DELAY
#                     post-failure registry-recheck tuning (defaults 3 / 2s);
#                     the test suite sets DELAY=0 to stay fast.
#
# Must be run from the repository root (it `cd`s into packages/* and calls
# scripts/advance-latest-dist-tag.sh by relative path).
set -euo pipefail

# Platform packages — npm publish, order irrelevant (no workspace:* deps).
PLATFORM_DIRS=(
  packages/zfb-darwin-arm64
  packages/zfb-darwin-x64
  packages/zfb-linux-arm64-gnu
  packages/zfb-linux-x64-gnu
  packages/zfb-win32-x64-msvc
)

# Non-platform packages — pnpm publish (workspace:* rewrite). Listed in
# topological order: @takazudo/zfb is published before create-zfb (which depends
# on it). npm does not validate dependency existence at publish time, so order
# is not required for success, but matching pnpm -r's topological default keeps
# the published set self-consistent at every intermediate moment.
#
# crates/zfb-md-wasm/npm (zfb#1579) is the first NONPLATFORM_DIRS occupant
# outside packages/* — it lives in the `crates/*/npm` workspace glob
# (pnpm-workspace.yaml), which `pnpm -r --filter <name>` resolves the same
# way regardless of the path prefix (verified locally: `pnpm -r --filter
# "@takazudo/zfb-md-wasm" exec pwd` resolves to crates/zfb-md-wasm/npm). It
# has no workspace:* deps of its own, so the rewrite is a no-op for it, but
# it goes through the same idempotent/tolerant/dist-tag machinery as every
# other entry here.
NONPLATFORM_DIRS=(
  packages/zfb
  packages/zfb-runtime
  packages/zfb-adapter-cloudflare
  packages/create-zfb
  crates/zfb-md-wasm/npm
)

# Read a field from a package's manifest (name | version). Deriving the package
# name from the manifest — rather than hard-coding it in a --filter string —
# guarantees the pnpm filter always matches an existing package.
manifest_field() {
  node -p "require('./$1/package.json').$2"
}

# True when the npm error text is specifically "this exact version already
# exists". Matching the phrase (NOT a bare E403) avoids swallowing a genuine
# permissions 403 ("You do not have permission to publish ...").
is_publish_conflict() {
  printf '%s' "$1" | grep -qiE 'cannot publish over the previously published versions|EPUBLISHCONFLICT'
}

# Single-shot registry lookup: echoes the version if <name>@<version> is on the
# registry, nothing otherwise. `|| true` so a not-found (npm view exits non-zero)
# does not trip `set -e`.
registry_version() {
  npm view "$1@$2" version 2>/dev/null || true
}

# Post-failure recovery probe (with brief backoff for registry propagation lag):
# returns 0 if <name>@<version> is on the registry within the recheck budget.
registry_has_version() {
  local name="$1" version="$2" attempt delay
  delay="${PUBLISH_RECHECK_DELAY:-2}"
  for attempt in $(seq 1 "${PUBLISH_RECHECK_ATTEMPTS:-3}"); do
    if [[ "$(registry_version "$name" "$version")" == "$version" ]]; then
      return 0
    fi
    if [[ "$delay" -gt 0 ]]; then
      sleep "$delay"
    fi
  done
  return 1
}

# True (and logs) when <name>@<version> is already on the registry — the caller
# then skips the publish entirely (idempotent re-run path).
should_skip_publish() {
  local name="$1" version="$2"
  if [[ -n "$(registry_version "$name" "$version")" ]]; then
    echo "✓ ${name}@${version} already on the registry — skipping publish."
    return 0
  fi
  return 1
}

# Decide what a publish exit status means.
#   status 0                    → success.
#   conflict error text         → already published (double-PUT race) → tolerate.
#   non-conflict error, but the version IS now on the registry
#                               → lost-ACK race (server committed, client died) → tolerate.
#   any other error             → real failure → return the status (fail the job).
classify_publish_result() {
  local name="$1" version="$2" status="$3" outfile="$4"
  if [[ "$status" -eq 0 ]]; then
    echo "✓ published ${name}@${version}."
    return 0
  fi
  if is_publish_conflict "$(cat "$outfile")"; then
    echo "✓ ${name}@${version} already published (publish-conflict / double-PUT race) — tolerating as skip."
    return 0
  fi
  if registry_has_version "$name" "$version"; then
    echo "⚠ publish of ${name}@${version} exited ${status} without a conflict message, but the version IS now on the registry (lost-ACK race) — tolerating as skip."
    return 0
  fi
  echo "::error::publish failed for ${name}@${version} (exit ${status}); not a publish-conflict and the version is not on the registry — failing the release."
  return "$status"
}

# Run a publish command (passed as argv), capturing combined output for
# classification. The pipeline MUST run under `set +e`: a non-zero publish would
# otherwise abort the job via pipefail before we capture ${PIPESTATUS[0]}.
# PIPESTATUS is read immediately after the pipeline, before any other command
# resets it.
_run_and_classify() {
  local name="$1" version="$2"; shift 2
  local tmp status rc
  tmp="$(mktemp)"
  echo "→ publishing ${name}@${version} (dist-tag ${DIST_TAG}) ..."
  set +e
  "$@" 2>&1 | tee "$tmp"
  status=${PIPESTATUS[0]}
  set -e
  classify_publish_result "$name" "$version" "$status" "$tmp"
  rc=$?
  rm -f "$tmp"
  return "$rc"
}

# Platform publish runner — kept byte-identical to the original invocation:
# `npm publish` (not pnpm) run inside the package dir, so the 0755 exec bit on
# the binary survives (issue #447 / #441). $2 ("" | "--provenance") is
# intentionally word-split.
# shellcheck disable=SC2086
_npm_publish_dir() {
  ( cd "$1" && npm publish --tag "$DIST_TAG" --access public $2 )
}

publish_platform_packages() {
  local mode="$1" dir name version prov
  for dir in "${PLATFORM_DIRS[@]}"; do
    name="$(manifest_field "$dir" name)"
    version="$(manifest_field "$dir" version)"
    if should_skip_publish "$name" "$version"; then
      continue
    fi
    prov="--provenance"
    if [[ "$mode" == "mac-local" && "$dir" == "packages/zfb-darwin-x64" ]]; then
      # Locally-built macOS-x64 binary: no GHA OIDC context → omit --provenance.
      prov=""
      echo "→ ${name}: locally-built macOS-x64 binary (no OIDC) — publishing WITHOUT --provenance."
    fi
    _run_and_classify "$name" "$version" _npm_publish_dir "$dir" "$prov"
  done
}

publish_nonplatform_packages() {
  local dir name version extra_flags
  for dir in "${NONPLATFORM_DIRS[@]}"; do
    name="$(manifest_field "$dir" name)"
    version="$(manifest_field "$dir" version)"
    if should_skip_publish "$name" "$version"; then
      continue
    fi
    extra_flags=()
    if [[ "$dir" == "crates/zfb-md-wasm/npm" ]]; then
      # @takazudo/zfb-md-wasm's `prepublishOnly` hook runs `pnpm build && pnpm
      # test`, and `build` needs the Rust wasm32 toolchain + a pinned
      # wasm-bindgen-cli — neither is installed in this job (unlike the other
      # NONPLATFORM_DIRS entries, whose `build`/`prepublishOnly` are plain
      # tsc/vitest, already satisfied here). release.yml's build-wasm-md job
      # builds dist/ separately and it's already downloaded into
      # crates/zfb-md-wasm/npm/dist before this script runs (see release.yml's
      # publish job) — --ignore-scripts skips the redundant, here-unrunnable
      # rebuild and publishes that pre-built dist/ directly.
      extra_flags=(--ignore-scripts)
    fi
    # `pnpm -r --filter "$name"` scopes a recursive publish to exactly this one
    # package (verified: packs only "$name", and -r keeps the filter semantics
    # explicit and version-robust). --fail-if-no-match makes a name that matches
    # no workspace package a loud failure, never a silent no-op.
    _run_and_classify "$name" "$version" \
      pnpm -r --filter "$name" --fail-if-no-match publish \
      --tag "$DIST_TAG" --no-git-checks --access public --provenance "${extra_flags[@]}"
  done
}

# Prerelease dual-tag (#481): advance `latest` alongside `next` while still in
# the prerelease phase. Runs only after every package is published (idempotently),
# so a partial publish never clobbers `latest`. advance-latest-dist-tag.sh has its
# own retry/backoff.
maybe_advance_latest() {
  if [[ "${ALSO_LATEST:-}" != "1" ]]; then
    return 0
  fi
  local zfb_semver
  zfb_semver="$(manifest_field packages/zfb version)"
  echo "== Prerelease dual-tag: advancing 'latest' to ${zfb_semver} =="
  bash scripts/advance-latest-dist-tag.sh "$zfb_semver"
}

main() {
  local mode="${1:-}"
  case "$mode" in
    all-provenance | mac-local) ;;
    *)
      echo "::error::usage: $0 <all-provenance|mac-local> (got: '${mode}')"
      return 2
      ;;
  esac
  : "${DIST_TAG:?DIST_TAG must be set (the npm dist-tag, e.g. next|latest)}"
  if [[ ! -f packages/zfb/package.json ]]; then
    echo "::error::must be run from the repo root (packages/zfb/package.json not found in $(pwd))"
    return 2
  fi

  echo "== Publishing all workspace packages (mode=${mode}, dist-tag=${DIST_TAG}) =="
  publish_platform_packages "$mode"
  publish_nonplatform_packages
  maybe_advance_latest
  echo "== Publish complete: every package is on the registry (newly published or already present). =="
}

# Sourced by the test suite (tests/unit/publish-npm-packages.sh) to exercise the
# functions in isolation — only run main() when executed directly.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
