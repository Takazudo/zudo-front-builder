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
# PUBLISH FROM PRE-PACKED TARBALLS ONLY (#4690). The job that holds the npm
# OIDC credential (`id-token: write`) never installs dependencies or runs
# package build/test code. release.yml's `pack` job (no id-token) builds and
# packs every package with scripts/release/pack-release-tarballs.mjs — `npm pack`
# for platform carriers (keeps the binary's 0755 bit, #447/#441) and `pnpm pack`
# for workspace packages (rewrites `workspace:*`) — and writes manifest.json:
# `{mode, packages: [{name, version, dir, file, integrity, provenance}]}`. This
# script verifies every tarball's sha512 against that manifest, checks each
# provenance decision against the mode, and runs `npm publish <tgz>` per package
# (one package per call, so the idempotent/tolerant wrapper applies per package
# and a mid-list conflict cannot abort the rest).
#
# Usage:
#   ZFB_PUBLISH_TARBALLS_DIR=<dir with manifest.json> \
#     scripts/publish-npm-packages.sh <all-provenance|mac-local|recovery-no-provenance>
#     all-provenance — every package (incl. both darwin carriers) was built in CI,
#                      so all carry OIDC-attested provenance (`--provenance`).
#                      UNUSED by release.yml since #4690: no Darwin carrier is
#                      built in CI any more. Kept only for hand-run/test use.
#     mac-local      — release.yml's normal mode: the platform packages named by
#                      ZFB_LOCAL_BUILT_PACKAGES (release.yml passes both Darwin
#                      carriers, admitted from owner-Mac Release assets)
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
# publish therefore passes `--provenance` or `--provenance=false`, and a
# suppressed publish also sets `NPM_CONFIG_PROVENANCE=false`. (pnpm no longer
# publishes; it needed the flag, not just the env var — measured 2026-11-12.)
#
# The two provenance-omitting modes additionally emit a one-shot trust-downgrade
# advisory (::warning + job summary) — see emit_trust_downgrade_advisory below.
#
# Required env:
#   DIST_TAG          npm dist-tag to publish under (next | latest).
#   ZFB_PUBLISH_TARBALLS_DIR
#                     the pack job's output: manifest.json + one .tgz per package
#                     (+ zfb-md-wasm-shipped-artifacts.json when the tree has it).
#   ZFB_RELEASE_SOURCE_COMMIT
#                     40-hex release source commit; every skip decision must
#                     tie the registry version to it (#4690).
#   ACTIONS_ID_TOKEN_REQUEST_URL / ACTIONS_ID_TOKEN_REQUEST_TOKEN
#                     provided by GitHub Actions when the job has
#                     `permissions: id-token: write`.
# Optional env:
#   ZFB_DARWIN_ADMISSION_VERDICT
#                     admission verdict JSON (scripts/admit-darwin-artifacts.mjs);
#                     a present Darwin carrier must then match its digests.
#   ZFB_NPM_PACKAGE_STATE_NODE
#                     node binary for npm-package-state.mjs (tests: a stub).
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
#                     An already-present package records what the registry
#                     OBSERVABLY carries (attestation to the source commit →
#                     attested, else none), so a partial normal publish resumed
#                     by recovery verifies honestly.
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
# Needs no repository checkout: package identity comes from the manifest, and
# companion scripts resolve relative to this file.
set -euo pipefail

# Companion control scripts resolve relative to this file.
PUBLISH_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Platform package dirs (the ZFB_LOCAL_BUILT_PACKAGES vocabulary).
PLATFORM_DIRS=(
  packages/zfb-darwin-arm64
  packages/zfb-darwin-x64
  packages/zfb-linux-arm64-gnu
  packages/zfb-linux-x64-gnu
  packages/zfb-win32-x64-msvc
)

# The complete publish set, in publication order: platform carriers first, then
# @takazudo/zfb-slugify before @takazudo/zfb before create-zfb (matching pnpm's
# topological default so the published set is self-consistent at every moment).
# A recovery of a tag that predates @takazudo/zfb-slugify may omit only it.
EXPECTED_DIRS=(
  "${PLATFORM_DIRS[@]}"
  packages/zfb-slugify
  packages/zfb
  packages/zfb-runtime
  packages/zfb-adapter-cloudflare
  packages/create-zfb
  crates/zfb-md-wasm/npm
)

# Platform carriers built outside this workflow (mac-local only); filled by
# prepare_local_built_packages from ZFB_LOCAL_BUILT_PACKAGES.
LOCAL_BUILT_DIRS=()

# Manifest rows: "name<TAB>version<TAB>dir<TAB>file<TAB>integrity<TAB>provenance".
TARBALL_ROWS=()

tarballs_dir() {
  printf '%s' "${ZFB_PUBLISH_TARBALLS_DIR:?ZFB_PUBLISH_TARBALLS_DIR must point at the pack job output}"
}

# Read manifest.json into TARBALL_ROWS; refuse a manifest for another mode.
load_tarball_manifest() {
  local mode="$1" dir rows
  dir="$(tarballs_dir)"
  if [[ ! -f "$dir/manifest.json" ]]; then
    echo "::error::no tarball manifest at $dir/manifest.json (the pack job's output). Refusing to publish."
    return 1
  fi
  # shellcheck disable=SC2016 # JavaScript template literals, not shell expansions.
  if ! rows="$(MANIFEST="$dir/manifest.json" WANT_MODE="$mode" node -e '
    const m = JSON.parse(require("node:fs").readFileSync(process.env.MANIFEST, "utf8"));
    if (m.schema !== "zfb-npm-tarballs/v1") throw new Error(`manifest schema is ${m.schema}`);
    if (m.mode !== process.env.WANT_MODE) throw new Error(`manifest was packed for mode ${m.mode}, not ${process.env.WANT_MODE}`);
    for (const p of m.packages) {
      for (const k of ["name", "version", "dir", "file", "integrity", "provenance"]) {
        if (typeof p[k] !== "string" || !p[k] || /[\t\n]/.test(p[k])) throw new Error(`manifest entry ${p.name}: bad ${k}`);
      }
      if (p.file.includes("/")) throw new Error(`manifest entry ${p.name}: file must be a basename`);
      console.log([p.name, p.version, p.dir, p.file, p.integrity, p.provenance].join("\t"));
    }
  ')"; then
    echo "::error::tarball manifest $dir/manifest.json is invalid (see above). Refusing to publish."
    return 1
  fi
  TARBALL_ROWS=()
  while IFS= read -r line; do
    [[ -n "$line" ]] && TARBALL_ROWS+=("$line")
  done <<<"$rows"
}

row_field() {
  local row="$1" index="$2"
  awk -F '\t' -v i="$index" '{ print $i }' <<<"$row"
}

tarball_field_by_dir() {
  local dir="$1" field="$2" row
  for row in "${TARBALL_ROWS[@]}"; do
    if [[ "$(row_field "$row" 3)" == "$dir" ]]; then
      case "$field" in
        name) row_field "$row" 1 ;;
        version) row_field "$row" 2 ;;
      esac
      return 0
    fi
  done
  echo "$dir"
}

# The manifest must hold exactly the expected set in order (recovery alone may
# omit @takazudo/zfb-slugify), one lockstep version, and tarballs whose sha512
# equals the recorded integrity. Checked before any registry call.
validate_tarball_set() {
  local mode="$1" row dir want version="" i=0 file integrity actual
  local -a want_dirs=()
  for dir in "${EXPECTED_DIRS[@]}"; do
    if [[ "$dir" == "packages/zfb-slugify" && "$mode" == "recovery-no-provenance" ]] \
      && ! printf '%s\n' "${TARBALL_ROWS[@]}" | awk -F '\t' '{ print $3 }' | grep -Fxq "$dir"; then
      echo "ℹ target tree predates @takazudo/zfb-slugify; omitting it from this recovery publish."
      continue
    fi
    want_dirs+=("$dir")
  done
  if [[ "${#TARBALL_ROWS[@]}" -ne "${#want_dirs[@]}" ]]; then
    echo "::error::publish target is missing expected package tarballs: manifest has ${#TARBALL_ROWS[@]} entries, expected ${#want_dirs[@]} (${want_dirs[*]})."
    return 1
  fi
  for want in "${want_dirs[@]}"; do
    row="${TARBALL_ROWS[$i]}"
    i=$((i + 1))
    if [[ "$(row_field "$row" 3)" != "$want" ]]; then
      echo "::error::publish target is missing expected package $want (manifest entry $i is $(row_field "$row" 3))."
      return 1
    fi
    if [[ -z "$version" ]]; then version="$(row_field "$row" 2)"; fi
    if [[ "$(row_field "$row" 2)" != "$version" ]]; then
      echo "::error::$(row_field "$row" 1) is $(row_field "$row" 2), not the lockstep $version."
      return 1
    fi
    file="$(tarballs_dir)/$(row_field "$row" 4)"
    integrity="$(row_field "$row" 5)"
    if [[ ! -f "$file" ]]; then
      echo "::error::tarball $file is missing."
      return 1
    fi
    actual="sha512-$(node -e 'process.stdout.write(require("node:crypto").createHash("sha512").update(require("node:fs").readFileSync(process.argv[1])).digest("base64"))' "$file")"
    if [[ "$actual" != "$integrity" ]]; then
      echo "::error::tarball $(row_field "$row" 4) integrity $actual differs from the manifest's $integrity. Refusing to publish."
      return 1
    fi
  done
}

# True when the npm error text is specifically "this exact version already
# exists". Matching the phrase (NOT a bare E403) avoids swallowing a genuine
# permissions 403 ("You do not have permission to publish ...").
is_publish_conflict() {
  printf '%s' "$1" | grep -qiE 'cannot publish over the previously published versions|EPUBLISHCONFLICT'
}

# Registry state of <name>@<version>, bound to the release source commit, from
# scripts/release/npm-package-state.mjs (#4690). Echoes "<state> <observed>":
#   state    absent | present-verified | present-unverified | uncertain
#   observed attested | none — the provenance the registry version carries
#            for the source commit (an attestation whose gitCommit is it).
# A skip is allowed ONLY for present-verified. `absent` needs definitive 404s on
# bounded re-reads; network/5xx/auth failures are `uncertain`. When the local
# tarball integrity is known, an exact registry dist.integrity match is the
# strongest verification; otherwise the attestation/gitHead/admitted-Darwin
# paths apply (a recovery rebuild differs byte-wise). The helper's one-line
# detail goes to stderr so the job log keeps the reason.
package_state() {
  local name="$1" version="$2" integrity="${3:-}" line state detail observed
  local -a args=(--version "$version" --source-commit "${ZFB_RELEASE_SOURCE_COMMIT:-}" --package "$name" --tsv)
  if [[ -n "${ZFB_DARWIN_ADMISSION_VERDICT:-}" ]]; then
    args+=(--admission "$ZFB_DARWIN_ADMISSION_VERDICT")
  fi
  if [[ -n "$integrity" ]]; then
    args+=(--tarball-integrity "$integrity")
  fi
  if ! line="$("${ZFB_NPM_PACKAGE_STATE_NODE:-node}" "${PUBLISH_SCRIPT_DIR}/release/npm-package-state.mjs" "${args[@]}")"; then
    echo "  state of ${name}@${version}: helper failed — treating as uncertain" >&2
    echo "uncertain none"
    return 0
  fi
  state="$(printf '%s\n' "$line" | awk -F '\t' -v n="$name" '$1 == n { print $2; exit }')"
  detail="$(printf '%s\n' "$line" | awk -F '\t' -v n="$name" '$1 == n { print $3; exit }')"
  observed="$(printf '%s\n' "$line" | awk -F '\t' -v n="$name" '$1 == n { print $4; exit }')"
  case "$state" in
    absent | present-verified | present-unverified | uncertain) ;;
    *)
      echo "  state of ${name}@${version}: unparseable helper output '${line}' — treating as uncertain" >&2
      state=uncertain
      ;;
  esac
  [[ "$observed" == attested ]] || observed=none
  echo "  state of ${name}@${version}: ${state}${detail:+ (${detail})}" >&2
  echo "$state $observed"
}

# Post-failure recheck (with brief backoff for registry propagation lag): echoes
# the final "<state> <observed>". Stops early on present-verified (tolerate) and
# on present-unverified (a definitive identity mismatch — retrying cannot fix it).
recheck_state() {
  local name="$1" version="$2" integrity="${3:-}" _attempt delay result="uncertain none"
  delay="${PUBLISH_RECHECK_DELAY:-2}"
  for _attempt in $(seq 1 "${PUBLISH_RECHECK_ATTEMPTS:-3}"); do
    result="$(package_state "$name" "$version" "$integrity")"
    case "${result%% *}" in
      present-verified | present-unverified) break ;;
    esac
    if [[ "$delay" -gt 0 ]]; then
      sleep "$delay"
    fi
  done
  echo "$result"
}

# Pre-publish decision. Returns 0 → skip (present-verified), 1 → publish
# (absent), 3 → refuse (present-unverified or uncertain: fail closed, nothing is
# published over or around an unidentified version). Sets SKIP_OBSERVED to the
# registry version's observed provenance for the expectation map.
SKIP_OBSERVED=""
should_skip_publish() {
  local name="$1" version="$2" integrity="${3:-}" result state
  result="$(package_state "$name" "$version" "$integrity")"
  state="${result%% *}"
  SKIP_OBSERVED="${result#* }"
  [[ "$SKIP_OBSERVED" == "$result" ]] && SKIP_OBSERVED=""
  case "$state" in
    present-verified)
      echo "✓ ${name}@${version} already on the registry and verified against the source commit — skipping publish."
      return 0
      ;;
    absent) return 1 ;;
    *)
      echo "::error::${name}@${version} registry state is ${state}; refusing to skip or publish it (fail closed)."
      return 3
      ;;
  esac
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
# truth for the publish flag; the pack job's manifest decision must agree.
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

# Every manifest decision must equal this script's own derivation.
validate_provenance_decisions() {
  local mode="$1" row dir decided want bad=0
  for row in "${TARBALL_ROWS[@]}"; do
    dir="$(row_field "$row" 3)"
    decided="$(row_field "$row" 6)"
    want="$(expected_provenance "$dir" "$mode")"
    if [[ "$decided" != "$want" ]]; then
      echo "::error::$(row_field "$row" 1): the pack manifest decided provenance '${decided}', but mode ${mode} requires '${want}'."
      bad=1
    fi
  done
  return "$bad"
}

# The explicit CLI flag for an expectation.
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
#   conflict error text, or any other error
#                               → recheck the registry state: tolerate as a skip
#                                 ONLY when it is present-verified (double-PUT
#                                 race / lost ACK of our own publish).
#   anything else               → real failure → return the status (fail the
#                                 job), repeating the original error text.
classify_publish_result() {
  local name="$1" version="$2" status="$3" outfile="$4" integrity="${5:-}" result state kind
  if [[ "$status" -eq 0 ]]; then
    echo "✓ published ${name}@${version}."
    return 0
  fi
  if is_publish_conflict "$(cat "$outfile")"; then
    kind="publish-conflict / double-PUT race"
  else
    kind="lost-ACK race"
  fi
  result="$(recheck_state "$name" "$version" "$integrity")"
  state="${result%% *}"
  if [[ "$state" == present-verified ]]; then
    echo "✓ ${name}@${version}: publish exited ${status} but the version is on the registry and verified against the source commit (${kind}) — tolerating as skip."
    return 0
  fi
  echo "::error::publish failed for ${name}@${version} (exit ${status}); registry state after recheck: ${state} — failing the release. Original publish output (tail):"
  tail -n 20 "$outfile" | sed 's/^/  | /'
  return "$status"
}

# Run a publish command (passed as argv), capturing combined output for
# classification. The pipeline MUST run under `set +e`: a non-zero publish would
# otherwise abort the job via pipefail before we capture ${PIPESTATUS[0]}.
_run_and_classify() {
  local name="$1" version="$2" integrity="$3"; shift 3
  local tmp status rc
  tmp="$(mktemp)"
  echo "→ publishing ${name}@${version} (dist-tag ${DIST_TAG}) ..."
  set +e
  "$@" 2>&1 | tee "$tmp"
  status=${PIPESTATUS[0]}
  set -e
  classify_publish_result "$name" "$version" "$status" "$tmp" "$integrity"
  rc=$?
  rm -f "$tmp"
  return "$rc"
}

# `npm publish <tarball>` with the explicit provenance flag. A tarball publish
# runs no package lifecycle scripts; --ignore-scripts makes that explicit. A
# suppressed publish also sets NPM_CONFIG_PROVENANCE=false so no npm config
# layer can re-enable it.
_npm_publish_tarball() {
  local file="$1" flag="$2"
  if [[ "$flag" == "--provenance=false" ]]; then
    NPM_CONFIG_PROVENANCE=false npm publish "$file" --tag "$DIST_TAG" --access public --ignore-scripts "$flag"
  else
    npm publish "$file" --tag "$DIST_TAG" --access public --ignore-scripts "$flag"
  fi
}

publish_tarballs() {
  local mode="$1" row name version dir file integrity expected skip_rc
  for row in "${TARBALL_ROWS[@]}"; do
    name="$(row_field "$row" 1)"
    version="$(row_field "$row" 2)"
    dir="$(row_field "$row" 3)"
    file="$(tarballs_dir)/$(row_field "$row" 4)"
    integrity="$(row_field "$row" 5)"
    expected="$(row_field "$row" 6)"
    skip_rc=0
    should_skip_publish "$name" "$version" "$integrity" || skip_rc=$?
    if [[ "$skip_rc" -eq 0 ]]; then
      # Record what the registry observably carries, not this mode's intent.
      record_expectation "$name" "$version" "${SKIP_OBSERVED:-$expected}" already-present
      if [[ "$dir" == "crates/zfb-md-wasm/npm" ]]; then
        verify_published_md_wasm "$version" "$mode"
      fi
      continue
    elif [[ "$skip_rc" -ne 1 ]]; then
      return 1
    fi
    if [[ "$expected" == "none" ]]; then
      if [[ "$mode" == "recovery-no-provenance" ]]; then
        echo "→ ${name}: main-based tag recovery — publishing with provenance suppressed (--provenance=false)."
      else
        echo "→ ${name}: locally-built carrier (not built by this workflow) — publishing with provenance suppressed (--provenance=false)."
      fi
    fi
    _run_and_classify "$name" "$version" "$integrity" _npm_publish_tarball "$file" "$(provenance_flag "$expected")"
    record_expectation "$name" "$version" "$expected" publish-attempted
    if [[ "$dir" == "crates/zfb-md-wasm/npm" ]]; then
      verify_published_md_wasm "$version" "$mode"
    fi
  done
}

# This runs after every successful MD/WASM outcome: a fresh publish, initial
# registry skip, conflict, or lost ACK. A digest mismatch is a hard failure,
# never the ordinary "publish now" return from should_skip_publish. The pack
# job ships the tree's manifest beside the tarballs.
verify_published_md_wasm() {
  local version="$1" mode="$2"
  local recovery_flags=()
  if [[ "$mode" == "recovery-no-provenance" ]]; then
    recovery_flags=(--allow-missing-manifest)
  fi
  "${ZFB_MD_WASM_VERIFY_NODE:-node}" "${PUBLISH_SCRIPT_DIR}/verify-zfb-md-wasm-published-digests.mjs" \
    --version "$version" \
    --manifest "${ZFB_MD_WASM_MANIFEST:-$(tarballs_dir)/zfb-md-wasm-shipped-artifacts.json}" \
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
        scope="${scope:+${scope}, }$(tarball_field_by_dir "$dir" name)"
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
  zfb_semver="$(tarball_field_by_dir packages/zfb version)"
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

  # Defense in depth (A2, #3140): these two modes publish at least one package
  # WITHOUT provenance, which is a permanent trust downgrade for a package that
  # already carries an earlier attestation. release.yml runs a preflight
  # (scripts/check-provenance-preflight.mjs) before any build and sets this env
  # var only when it passed — no downgrade found, or one acknowledged by the
  # owner. Refuse to publish anything if it is missing.
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
  # Every skip decision binds the registry state to the release source commit
  # (#4690); without it nothing could ever be present-verified, so refuse early.
  if [[ ! "${ZFB_RELEASE_SOURCE_COMMIT:-}" =~ ^[0-9a-f]{40}$ ]]; then
    echo "::error::ZFB_RELEASE_SOURCE_COMMIT must be the 40-hex release source commit (got '${ZFB_RELEASE_SOURCE_COMMIT:-}'). Refusing to publish."
    return 2
  fi
  load_tarball_manifest "$mode" || return 1
  validate_tarball_set "$mode" || return 1
  validate_provenance_decisions "$mode" || return 1
  # OIDC-only auth (fail closed) before the first registry call of any kind.
  if ! assert_npm_oidc_only; then
    echo "::error::npm OIDC preflight refused; nothing was published."
    return 1
  fi
  if [[ -n "${ZFB_PROVENANCE_EXPECTATIONS_FILE:-}" ]]; then
    : >"$ZFB_PROVENANCE_EXPECTATIONS_FILE"
  fi

  echo "== Publishing ${#TARBALL_ROWS[@]} packed tarballs (mode=${mode}, dist-tag=${DIST_TAG}) =="
  emit_trust_downgrade_advisory "$mode"
  publish_tarballs "$mode"
  maybe_advance_latest
  echo "== Publish complete: every package is on the registry (newly published or already present). =="
}

# Sourced by the test suite (tests/unit/publish-npm-packages.sh) to exercise the
# functions in isolation — only run main() when executed directly.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
