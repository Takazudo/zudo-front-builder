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
# The `( cd "$dir" && npm publish ... )` invocation below keeps that shape
# (npm, inside the package dir) for that reason.
#
# Non-platform packages (@takazudo/zfb-slugify, @takazudo/zfb,
# @takazudo/zfb-runtime,
# @takazudo/zfb-adapter-cloudflare, create-zfb) are published with
# `pnpm -r --filter <name> publish` so pnpm rewrites their `workspace:*` deps to
# concrete versions at pack time. Published one package per call (a recursive
# publish scoped to a single --filter) rather than one bulk `pnpm -r publish`
# over the whole set, so the idempotent/tolerant wrapper applies per package and
# a mid-list conflict cannot abort the rest.
#
# Usage:
#   scripts/publish-npm-packages.sh <all-provenance|mac-local|recovery-no-provenance>
#     all-provenance — every package (incl. both darwin carriers) was built in CI,
#                      so all carry OIDC-attested provenance (`--provenance`).
#     mac-local      — the platform packages named by ZFB_LOCAL_BUILT_PACKAGES
#                      were built locally (outside this GHA job). Their bytes are
#                      not described by this job's OIDC identity, so they are
#                      published with provenance explicitly suppressed
#                      (`--provenance=false`); every other package keeps
#                      `--provenance`.
#     recovery-no-provenance
#                    — a fixed workflow running from main built an older verified
#                      release tag. GitHub OIDC identifies the main workflow SHA,
#                      not that tag SHA, so every package is published with
#                      provenance explicitly suppressed.
#
# Authentication is GitHub OIDC trusted publishing ONLY. No stored npm token is
# read or accepted: scripts/npm-oidc-preflight.mjs refuses to continue when the
# job has no GitHub ID-token request variables or when any stored npm credential
# is visible (npm and pnpm silently fall back to such a credential when the
# OIDC exchange fails, so one would hide a trusted-publisher misconfiguration).
#
# Provenance is EXPLICIT in every mode, never left to a client default: npm
# 12.2.1 and pnpm 12.8.2 both auto-generate provenance for a public package
# published through OIDC from a public repository when no flag is given. Each
# publish therefore passes `--provenance` or `--provenance=false`. For pnpm,
# `NPM_CONFIG_PROVENANCE=false` alone was measured NOT to suppress that
# auto-provenance (pnpm 12.8.2 against a local fake registry/OIDC endpoint,
# 2026-11-12); only the `--provenance=false` flag does. The env var is still set
# on suppressed publishes for npm's benefit.
#
# The two provenance-omitting modes additionally emit a one-shot trust-downgrade
# advisory (::warning + job summary) — see emit_trust_downgrade_advisory below.
#
# Required env:
#   DIST_TAG          npm dist-tag to publish under (next | latest).
#   ACTIONS_ID_TOKEN_REQUEST_URL / ACTIONS_ID_TOKEN_REQUEST_TOKEN
#                     provided by GitHub Actions when the job has
#                     `permissions: id-token: write`.
# Optional env:
#   ALSO_LATEST       "1" → after publishing, also advance the `latest` dist-tag
#                     (prerelease dual-tag policy, see #481). Delegated to
#                     scripts/advance-latest-dist-tag.sh, which has its own retry.
#   ZFB_LOCAL_BUILT_PACKAGES
#                     mac-local only: whitespace/comma-separated platform package
#                     dirs that were built outside this job. Defaults to
#                     `packages/zfb-darwin-x64` (the historical single-carrier
#                     escape hatch). Every entry must be a known platform dir;
#                     setting it in any other mode is refused.
#   ZFB_PROVENANCE_EXPECTATIONS_FILE
#                     when set, receives one TSV line per package:
#                     `<name>\t<version>\t<attested|none>\t<outcome>`; read by
#                     scripts/verify-npm-provenance-expectations.mjs after publish.
#   PUBLISH_RECHECK_ATTEMPTS / PUBLISH_RECHECK_DELAY
#                     post-failure registry-recheck tuning (defaults 3 / 2s);
#                     the test suite sets DELAY=0 to stay fast.
#   ZFB_ALLOW_PROVENANCE_DOWNGRADE
#                     Required, set to "1", for the mac-local and
#                     recovery-no-provenance modes (A2, #3140) — release.yml
#                     sets it only after its provenance-downgrade preflight
#                     passed (no downgrade, or an explicitly acknowledged one).
#                     Not required for all-provenance, and refusing to publish
#                     without it does not change the all-provenance path.
#
# Must be run from the repository root (it `cd`s into packages/* and calls
# scripts/advance-latest-dist-tag.sh by relative path).
set -euo pipefail

# Resolve companion control scripts relative to this file. During a main-based
# recovery this helper is invoked from a separate `.release-control` checkout
# while package manifests/binaries remain in the verified tag checkout.
PUBLISH_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Platform packages — npm publish, order irrelevant (no workspace:* deps).
PLATFORM_DIRS=(
  packages/zfb-darwin-arm64
  packages/zfb-darwin-x64
  packages/zfb-linux-arm64-gnu
  packages/zfb-linux-x64-gnu
  packages/zfb-win32-x64-msvc
)

# Non-platform packages — pnpm publish (workspace:* rewrite). Listed in
# topological order: @takazudo/zfb-slugify precedes @takazudo/zfb, and zfb is
# published before create-zfb (which depends on it). npm does not validate
# dependency existence at publish time, so order is not required for success,
# but matching pnpm -r's topological default keeps the published set
# self-consistent at every intermediate moment.
#
# @takazudo/zfb-md-wasm is a lockstep non-platform package that lives outside
# packages/* under the `crates/*/npm` workspace glob (pnpm-workspace.yaml).
# `pnpm -r --filter <name>` resolves it by manifest name regardless of path
# prefix (verified locally: `pnpm -r --filter "@takazudo/zfb-md-wasm" exec pwd`
# resolves to crates/zfb-md-wasm/npm), so it belongs in the same
# idempotent/tolerant/dist-tag publish machinery as the packages/* entries.
# Platform carriers built outside this job (mac-local only); filled by
# prepare_local_built_packages from ZFB_LOCAL_BUILT_PACKAGES.
LOCAL_BUILT_DIRS=()

NONPLATFORM_DIRS=(
  packages/zfb-slugify
  packages/zfb
  packages/zfb-runtime
  packages/zfb-adapter-cloudflare
  packages/create-zfb
  crates/zfb-md-wasm/npm
)

# release.yml's main-based recovery checks out a current release-control tree
# beside an older tag's package tree. Older tags predate zfb-slugify, so that
# one newly-added package is absent from the target tree and may be omitted
# only for recovery. Every other inventory entry is required; validate the
# complete list before publishing any platform package so a malformed target
# cannot partially ship.
prepare_nonplatform_inventory() {
  local mode="$1"
  local dir
  local -a present=()
  for dir in "${NONPLATFORM_DIRS[@]}"; do
    if [[ -f "$dir/package.json" ]]; then
      present+=("$dir")
    elif [[ "$dir" == "packages/zfb-slugify" && "$mode" == "recovery-no-provenance" ]]; then
      echo "ℹ target tree predates @takazudo/zfb-slugify; omitting it from this recovery publish."
    else
      echo "::error::publish target is missing expected package manifest: $dir/package.json"
      return 1
    fi
  done
  NONPLATFORM_DIRS=("${present[@]}")
}

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

# ── Explicit per-package provenance ─────────────────────────────────────────

# Parse ZFB_LOCAL_BUILT_PACKAGES (whitespace/comma separated) into
# LOCAL_BUILT_DIRS. Only meaningful for mac-local; every entry must be a known
# platform carrier so a typo cannot silently leave a local binary attested.
prepare_local_built_packages() {
  local mode="$1" raw entry known dir
  raw="${ZFB_LOCAL_BUILT_PACKAGES-}"
  LOCAL_BUILT_DIRS=()
  if [[ "$mode" != "mac-local" ]]; then
    if [[ -n "${raw//[[:space:],]/}" ]]; then
      echo "::error::ZFB_LOCAL_BUILT_PACKAGES is set ('${raw}') but mode is '${mode}'; only mac-local publishes locally-built carriers. Refusing to publish."
      return 1
    fi
    return 0
  fi
  if [[ -z "${raw//[[:space:],]/}" ]]; then
    raw="packages/zfb-darwin-x64"
  fi
  for entry in ${raw//,/ }; do
    entry="${entry%/}"
    known=0
    for dir in "${PLATFORM_DIRS[@]}"; do
      [[ "$dir" == "$entry" ]] && known=1
    done
    if [[ "$known" -ne 1 ]]; then
      echo "::error::ZFB_LOCAL_BUILT_PACKAGES entry '${entry}' is not a platform package dir (${PLATFORM_DIRS[*]}). Refusing to publish."
      return 1
    fi
    LOCAL_BUILT_DIRS+=("$entry")
  done
}

is_local_built_dir() {
  local dir
  for dir in "${LOCAL_BUILT_DIRS[@]}"; do
    [[ "$dir" == "$1" ]] && return 0
  done
  return 1
}

# Echo `attested` or `none` for a package dir under <mode>. The single source of
# truth for both the publish flag and the post-publish expectation map.
expected_provenance() {
  local dir="$1" mode="$2"
  case "$mode" in
    recovery-no-provenance) echo none ;;
    mac-local)
      if is_local_built_dir "$dir"; then echo none; else echo attested; fi
      ;;
    *) echo attested ;;
  esac
}

# The explicit CLI flag for an expectation (same spelling for npm and pnpm).
provenance_flag() {
  if [[ "$1" == "attested" ]]; then
    echo "--provenance"
  else
    echo "--provenance=false"
  fi
}

record_expectation() {
  local name="$1" version="$2" expected="$3" outcome="$4"
  if [[ -n "${ZFB_PROVENANCE_EXPECTATIONS_FILE:-}" ]]; then
    printf '%s\t%s\t%s\t%s\n' "$name" "$version" "$expected" "$outcome" >>"$ZFB_PROVENANCE_EXPECTATIONS_FILE"
  fi
}

# Fail-closed auth guard shared with the dist-tag helpers: GitHub OIDC must be
# available and no stored npm credential may be visible.
assert_npm_oidc_only() {
  "${ZFB_NPM_OIDC_PREFLIGHT_NODE:-node}" "${PUBLISH_SCRIPT_DIR}/npm-oidc-preflight.mjs"
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

# Platform publish runner: `npm publish` (not pnpm) run inside the package dir,
# so the 0755 exec bit on the binary survives (issue #447 / #441). $2 is the
# explicit provenance flag; a suppressed publish also sets
# NPM_CONFIG_PROVENANCE=false so no npm config layer can re-enable it.
_npm_publish_dir() {
  local dir="$1" flag="$2"
  if [[ "$flag" == "--provenance=false" ]]; then
    ( cd "$dir" && NPM_CONFIG_PROVENANCE=false npm publish --tag "$DIST_TAG" --access public "$flag" )
  else
    ( cd "$dir" && npm publish --tag "$DIST_TAG" --access public "$flag" )
  fi
}

# Non-platform publish runner (pnpm rewrites workspace:* deps). The flag is
# what actually suppresses pnpm's OIDC auto-provenance; see the header.
_pnpm_publish() {
  local flag="$1"; shift
  if [[ "$flag" == "--provenance=false" ]]; then
    NPM_CONFIG_PROVENANCE=false pnpm "$@" "$flag"
  else
    pnpm "$@" "$flag"
  fi
}

# Preflight every carrier before the first irreversible publication.
# Recovery of older tags has no CSS export and needs no new artifact.
preflight_css_artifacts() {
  local dir
  if node -e 'process.exit(require("./packages/zfb/package.json").publishConfig?.exports?.["./css"] ? 0 : 1)'; then
    for dir in "${PLATFORM_DIRS[@]}"; do
      [[ -f "$dir/zfb-css.node" ]] || { echo "Missing native CSS artifact: $dir/zfb-css.node" >&2; return 1; }
    done
  fi
}

publish_platform_packages() {
  local mode="$1" dir name version expected
  for dir in "${PLATFORM_DIRS[@]}"; do
    name="$(manifest_field "$dir" name)"
    version="$(manifest_field "$dir" version)"
    expected="$(expected_provenance "$dir" "$mode")"
    if should_skip_publish "$name" "$version"; then
      record_expectation "$name" "$version" "$expected" already-present
      continue
    fi
    if [[ "$expected" == "none" ]]; then
      if [[ "$mode" == "recovery-no-provenance" ]]; then
        echo "→ ${name}: main-based tag recovery — publishing with provenance suppressed (--provenance=false)."
      else
        echo "→ ${name}: locally-built carrier (not built by this job) — publishing with provenance suppressed (--provenance=false)."
      fi
    fi
    _run_and_classify "$name" "$version" _npm_publish_dir "$dir" "$(provenance_flag "$expected")"
    record_expectation "$name" "$version" "$expected" publish-attempted
  done
}

publish_nonplatform_packages() {
  local mode="$1" dir name version extra_flags expected
  for dir in "${NONPLATFORM_DIRS[@]}"; do
    name="$(manifest_field "$dir" name)"
    version="$(manifest_field "$dir" version)"
    expected="$(expected_provenance "$dir" "$mode")"
    if should_skip_publish "$name" "$version"; then
      record_expectation "$name" "$version" "$expected" already-present
      if [[ "$dir" == "crates/zfb-md-wasm/npm" ]]; then
        verify_published_md_wasm "$version" "$mode"
      fi
      continue
    fi
    extra_flags=()
    if [[ "$expected" == "none" ]]; then
      echo "→ ${name}: main-based tag recovery — publishing with provenance suppressed (--provenance=false)."
    fi
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
      _pnpm_publish "$(provenance_flag "$expected")" \
      -r --filter "$name" --fail-if-no-match publish \
      --tag "$DIST_TAG" --no-git-checks --access public \
      "${extra_flags[@]}"
    record_expectation "$name" "$version" "$expected" publish-attempted
    if [[ "$dir" == "crates/zfb-md-wasm/npm" ]]; then
      verify_published_md_wasm "$version" "$mode"
    fi
  done
}

# This runs after every successful MD/WASM outcome: a fresh publish, initial
# registry skip, conflict, or lost ACK. A digest mismatch is a hard failure,
# never the ordinary "publish now" return from should_skip_publish.
verify_published_md_wasm() {
  local version="$1" mode="$2"
  local recovery_flags=()
  if [[ "$mode" == "recovery-no-provenance" ]]; then
    recovery_flags=(--allow-missing-manifest)
  fi
  "${ZFB_MD_WASM_VERIFY_NODE:-node}" "${PUBLISH_SCRIPT_DIR}/verify-zfb-md-wasm-published-digests.mjs" \
    --version "$version" \
    --manifest "${ZFB_MD_WASM_MANIFEST:-crates/zfb-md-wasm/npm/dist/shipped-artifacts.json}" \
    "${recovery_flags[@]}"
}

# Trust-downgrade advisory (issue #2623).
# npm records a provenance attestation per PUBLISHED VERSION, and versions are
# immutable. So a package published WITHOUT --provenance after earlier versions
# carried it is a permanent *trust downgrade* for that package: a consumer with
# pnpm's opt-in `trustPolicy: no-downgrade` cannot install it at all, and gets
# ERR_PNPM_TRUST_DOWNGRADE ("possible package takeover").
#
# The setting is opt-in (default `off`), added in pnpm 10.21. Measured against the
# live registry on 2026-08-27 (pnpm 10.25.0, 11.1.0, 11.2.0, 11.3.0), the blast
# radius differs by version, which is why v2.12.0 went unnoticed:
#   - fresh resolve  — fails on every version that has the setting.
#   - --frozen-lockfile — SUCCEEDS up to pnpm 11.1 ("resolution step is skipped"),
#     FAILS from pnpm 11.2, which re-applies the active supply-chain policies to
#     every entry of an already-resolved lockfile. So an older CI job keeps going
#     green while an 11.2+ one breaks on the same committed lockfile.
# Note a package that never had provenance cannot "downgrade" — the check needs an
# earlier attested version to compare against. That is why only 5 of the 6 v2.12.0
# entries were flagged: zfb-darwin-x64 had no attestation through 2.12.0.
#
# The two provenance-omitting modes cannot avoid this (that is the point: neither
# has an OIDC identity that would truthfully describe the artifacts). What they
# CAN do is say so loudly, once, so the release operator schedules the only
# remedy that exists — a subsequent normally-attested release.
emit_trust_downgrade_advisory() {
  local mode="$1" scope dir
  case "$mode" in
    recovery-no-provenance) scope="EVERY package in this release" ;;
    mac-local)
      scope=""
      for dir in "${LOCAL_BUILT_DIRS[@]}"; do
        scope="${scope:+${scope}, }$(manifest_field "$dir" name)"
      done
      ;;
    *) return 0 ;;
  esac

  echo "::warning title=npm provenance trust downgrade::${scope} is being published WITHOUT npm provenance. Consumers who enable pnpm's trustPolicy=no-downgrade will fail to install this version (ERR_PNPM_TRUST_DOWNGRADE) on a fresh resolve, and on pnpm 11.2+ even with --frozen-lockfile. npm versions are immutable, so the ONLY remedy is a follow-up release published through the normal tag/release path with provenance restored. See RELEASE_DAY_CHECKLIST.md and issue #2623."

  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
      echo "### :warning: npm provenance trust downgrade"
      echo
      echo "**${scope}** is published **without** npm provenance in this run (mode \`${mode}\`)."
      echo
      echo "Consumers who enable pnpm's \`trustPolicy: no-downgrade\` will get"
      echo "\`ERR_PNPM_TRUST_DOWNGRADE\`, because earlier versions of the same package carried a"
      echo "provenance attestation. This blocks a fresh resolve on every pnpm version, and on"
      echo "pnpm 11.2+ it also blocks \`--frozen-lockfile\` (11.2 added a verification pass"
      echo "over already-resolved lockfile entries; earlier versions skip them)."
      echo
      echo "npm versions are immutable. **Required follow-up:** ship the next patch through the"
      echo "normal tag/release path so provenance is restored, or publish explicit consumer"
      echo "guidance. See \`RELEASE_DAY_CHECKLIST.md\` and issue #2623."
    } >>"$GITHUB_STEP_SUMMARY" || true # an advisory must never fail the publish job
  fi
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
  bash "${PUBLISH_SCRIPT_DIR}/advance-latest-dist-tag.sh" "$zfb_semver"
}

main() {
  local mode="${1:-}"
  case "$mode" in
    all-provenance | mac-local | recovery-no-provenance) ;;
    *)
      echo "::error::usage: $0 <all-provenance|mac-local|recovery-no-provenance> (got: '${mode}')"
      return 2
      ;;
  esac
  : "${DIST_TAG:?DIST_TAG must be set (the npm dist-tag, e.g. next|latest)}"
  if [[ ! -f packages/zfb/package.json ]]; then
    echo "::error::must be run from the repo root (packages/zfb/package.json not found in $(pwd))"
    return 2
  fi
  if ! prepare_nonplatform_inventory "$mode"; then
    return 1
  fi

  # Defense in depth (A2, #3140): these two modes publish at least one package
  # WITHOUT provenance, which is a permanent trust downgrade for a package that
  # already carries an earlier attestation. release.yml runs a preflight
  # (scripts/check-provenance-preflight.mjs) before any build leg / before this
  # script runs, and sets this env var only when that preflight passed — no
  # downgrade found, or a downgrade found and explicitly acknowledged. Refuse
  # to publish anything if it is missing, so a caller that bypasses the
  # workflow's preflight (a hand-run invocation, a future refactor that drops
  # the wiring) cannot silently ship an unacknowledged downgrade.
  case "$mode" in
    mac-local | recovery-no-provenance)
      if [[ "${ZFB_ALLOW_PROVENANCE_DOWNGRADE:-}" != "1" ]]; then
        echo "::error::mode '${mode}' publishes at least one package WITHOUT npm provenance, which is a permanent trust downgrade for consumers with pnpm's trustPolicy=no-downgrade. Set ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 only after that downgrade has been explicitly acknowledged (see release.yml's provenance-downgrade preflight). Refusing to publish."
        return 1
      fi
      ;;
  esac

  if ! prepare_local_built_packages "$mode"; then
    return 2
  fi
  preflight_css_artifacts
  # OIDC-only auth (fail closed) before the first registry call of any kind.
  if ! assert_npm_oidc_only; then
    echo "::error::npm OIDC preflight refused; nothing was published."
    return 1
  fi
  if [[ -n "${ZFB_PROVENANCE_EXPECTATIONS_FILE:-}" ]]; then
    : >"$ZFB_PROVENANCE_EXPECTATIONS_FILE"
  fi

  echo "== Publishing all workspace packages (mode=${mode}, dist-tag=${DIST_TAG}) =="
  emit_trust_downgrade_advisory "$mode"
  publish_platform_packages "$mode"
  publish_nonplatform_packages "$mode"
  maybe_advance_latest
  echo "== Publish complete: every package is on the registry (newly published or already present). =="
}

# Sourced by the test suite (tests/unit/publish-npm-packages.sh) to exercise the
# functions in isolation — only run main() when executed directly.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
