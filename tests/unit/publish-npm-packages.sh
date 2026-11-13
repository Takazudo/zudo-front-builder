#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/publish-npm-packages.sh — offline unit + integration tests for
# scripts/publish-npm-packages.sh (the idempotent, conflict-tolerant npm publish
# orchestrator added after the v0.1.0-next.42 partial-publish incident).
#
# Runs entirely offline. `scripts/publish-npm-packages.sh` is bash; this harness
# is POSIX sh (health.yml + run-b4push.sh invoke `sh tests/unit/*.sh`), so the
# bash functions are exercised by sourcing the script inside `bash -c` and the
# whole-script integration runs use mock `npm`/`pnpm` on PATH. `node` is the only
# real tool consulted (reads local package.json — no network).
#
# Requires: sh, bash, node, mktemp, grep.
#
# Run:
#   sh tests/unit/publish-npm-packages.sh

set -eu

# Run from the repo root regardless of the caller's cwd — the script under test
# cd's into packages/* and reads packages/zfb/package.json.
SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
cd "$REPO_ROOT"

SCRIPT="$REPO_ROOT/scripts/publish-npm-packages.sh"
REAL_SCRIPT="$SCRIPT"
NODE_EXE=$(node -p 'process.execPath')

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

if [ ! -f "$SCRIPT" ]; then
  fail "script not found: $SCRIPT"
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

# assert_exit <desc> <expected-exit> <bash-snippet>
# Sources the script (defines its functions; main() is NOT run thanks to the
# BASH_SOURCE guard), neutralises set -e, then runs the snippet. The snippet's
# exit status is compared to <expected-exit>. Output is suppressed.
assert_exit() {
  AE_DESC="$1"
  AE_WANT="$2"
  AE_SNIPPET="$3"
  # `if` so a non-zero exit (an expected outcome under test) does not trip the
  # harness's own `set -e`.
  if bash -c ". \"$SCRIPT\"; set +e; ${AE_SNIPPET}" >/dev/null 2>&1; then
    AE_GOT=0
  else
    AE_GOT=$?
  fi
  if [ "$AE_GOT" -eq "$AE_WANT" ]; then
    pass "$AE_DESC"
  else
    fail "$AE_DESC (want exit $AE_WANT, got $AE_GOT)"
  fi
}

# ── is_publish_conflict — matches the SPECIFIC conflict phrase only ────────────

assert_exit 'is_publish_conflict: "cannot publish over the previously published versions" → match' 0 \
  'is_publish_conflict "npm error code E403 You cannot publish over the previously published versions: 1.2.3"'

assert_exit 'is_publish_conflict: EPUBLISHCONFLICT → match' 0 \
  'is_publish_conflict "npm error code EPUBLISHCONFLICT"'

assert_exit 'is_publish_conflict: permissions 403 → NO match (not swallowed)' 1 \
  'is_publish_conflict "npm error 403 Forbidden You do not have permission to publish \"pkg\""'

assert_exit 'is_publish_conflict: generic network error → NO match' 1 \
  'is_publish_conflict "npm error network request to https://registry.npmjs.org failed ETIMEDOUT"'

assert_exit 'is_publish_conflict: empty → NO match' 1 \
  'is_publish_conflict ""'

# ── should_skip_publish — skip ONLY for present-verified (#4690) ──────────────
# package_state is the seam to scripts/release/npm-package-state.mjs; the node
# helper itself is covered by scripts/__tests__/npm-package-state.test.mjs.

assert_exit 'should_skip_publish: present-verified → skip (0)' 0 \
  'package_state() { echo present-verified; }; should_skip_publish foo 1.2.3'

assert_exit 'should_skip_publish: processing → refuse (3) with do-not-republish message' 3 \
  'package_state() { echo processing; }; out=$(should_skip_publish foo 1.2.3 2>&1); r=$?; case "$out" in *"npm is still processing foo@1.2.3 — do not republish"*finalize_only*) exit $r ;; *) exit 99 ;; esac'

assert_exit 'should_skip_publish: absent → publish (1)' 1 \
  'package_state() { echo absent; }; should_skip_publish foo 1.2.3'

assert_exit 'should_skip_publish: present-unverified → refuse (3)' 3 \
  'package_state() { echo present-unverified; }; should_skip_publish foo 1.2.3'

assert_exit 'should_skip_publish: uncertain → refuse (3)' 3 \
  'package_state() { echo uncertain; }; should_skip_publish foo 1.2.3'

assert_exit 'package_state: helper failure → uncertain' 0 \
  'ZFB_NPM_PACKAGE_STATE_NODE=false; [ "$(package_state foo 1.2.3 2>/dev/null)" = "uncertain none" ]'

assert_exit 'package_state: unparseable helper output → uncertain' 0 \
  'ZFB_NPM_PACKAGE_STATE_NODE=echo; [ "$(package_state foo 1.2.3 2>/dev/null)" = "uncertain none" ]'

# The helper reports `processing` (#4715/#4716); package_state must hand it to
# should_skip_publish unchanged — never downgraded to absent/uncertain.
assert_exit 'package_state: helper processing state is passed through' 0 \
  'stub=$(mktemp); printf "#!/bin/sh\nprintf \"foo\\tprocessing\\taccepted 2026-11-13\\tnone\\n\"\n" >"$stub"; chmod +x "$stub"; ZFB_NPM_PACKAGE_STATE_NODE=$stub; got=$(package_state foo 1.2.3 2>/dev/null); rm -f "$stub"; [ "$got" = "processing none" ]'

# ── classify_publish_result ───────────────────────────────────────────────────

assert_exit 'classify: exit 0 → success (0)' 0 \
  'classify_publish_result foo 1.2.3 0 /dev/null'

assert_exit 'classify: conflict text + present-verified → tolerate (0)' 0 \
  'f=$(mktemp); printf "%s" "remote: You cannot publish over the previously published versions: 1.2.3" >"$f"; package_state() { echo present-verified; }; PUBLISH_RECHECK_DELAY=0 classify_publish_result foo 1.2.3 1 "$f"; r=$?; rm -f "$f"; exit $r'

assert_exit 'classify: exit 0 → outcome publish-accepted' 0 \
  'classify_publish_result foo 1.2.3 0 /dev/null >/dev/null; [ "$PUBLISH_OUTCOME" = publish-accepted ]'

assert_exit 'is_publish_staged: E409 "previously staged version" → match' 0 \
  'is_publish_staged "npm error code E409
npm error 409 Conflict - PUT https://registry.npmjs.org/@takazudo%2fzfb - Cannot publish over previously staged version: 6.1.3"'

assert_exit 'is_publish_staged: published-versions conflict → no match' 1 \
  'is_publish_staged "You cannot publish over the previously published versions: 1.2.3"'

assert_exit 'classify: E409 previously staged → processing, success, no registry recheck' 0 \
  'f=$(mktemp); printf "%s" "npm error code E409 Cannot publish over previously staged version: 1.2.3" >"$f"; package_state() { echo "recheck must not run" >&2; echo absent; return 9; }; classify_publish_result foo 1.2.3 1 "$f" >/dev/null 2>&1; r=$?; rm -f "$f"; [ "$r" -eq 0 ] && [ "$PUBLISH_OUTCOME" = processing ]'

assert_exit 'classify: conflict + present-verified → outcome already-present' 0 \
  'f=$(mktemp); printf "%s" "EPUBLISHCONFLICT" >"$f"; package_state() { echo present-verified; }; PUBLISH_RECHECK_DELAY=0 classify_publish_result foo 1.2.3 1 "$f" >/dev/null; r=$?; rm -f "$f"; [ "$r" -eq 0 ] && [ "$PUBLISH_OUTCOME" = already-present ]'

assert_exit 'classify: genuine publish failure (E401) still fails with no outcome' 1 \
  'f=$(mktemp); printf "%s" "npm error code E401 Unable to authenticate" >"$f"; package_state() { echo absent; }; PUBLISH_RECHECK_DELAY=0 classify_publish_result foo 1.2.3 1 "$f" >/dev/null 2>&1; r=$?; [ -z "$PUBLISH_OUTCOME" ] || exit 7; rm -f "$f"; [ "$r" -ne 0 ] && exit 1; exit 0'

assert_exit 'classify: conflict text but present-unverified → fail (1)' 1 \
  'f=$(mktemp); printf "%s" "EPUBLISHCONFLICT" >"$f"; package_state() { echo present-unverified; }; PUBLISH_RECHECK_DELAY=0 classify_publish_result foo 1.2.3 1 "$f"; r=$?; rm -f "$f"; exit $r'

assert_exit 'classify: conflict text but version absent after recheck → fail (1)' 1 \
  'f=$(mktemp); printf "%s" "EPUBLISHCONFLICT" >"$f"; package_state() { echo absent; }; PUBLISH_RECHECK_DELAY=0 PUBLISH_RECHECK_ATTEMPTS=2 classify_publish_result foo 1.2.3 1 "$f"; r=$?; rm -f "$f"; exit $r'

assert_exit 'classify: conflict text but registry uncertain → fail (1)' 1 \
  'f=$(mktemp); printf "%s" "EPUBLISHCONFLICT" >"$f"; package_state() { echo uncertain; }; PUBLISH_RECHECK_DELAY=0 PUBLISH_RECHECK_ATTEMPTS=2 classify_publish_result foo 1.2.3 1 "$f"; r=$?; rm -f "$f"; exit $r'

assert_exit 'classify: non-conflict error BUT version landed verified (lost-ACK) → tolerate (0)' 0 \
  'f=$(mktemp); printf "%s" "socket hang up ECONNRESET" >"$f"; package_state() { echo present-verified; }; PUBLISH_RECHECK_DELAY=0 classify_publish_result foo 1.2.3 1 "$f"; r=$?; rm -f "$f"; exit $r'

assert_exit 'classify: real error AND version absent → fail (1)' 1 \
  'f=$(mktemp); printf "%s" "403 You do not have permission to publish" >"$f"; package_state() { echo absent; }; PUBLISH_RECHECK_DELAY=0 PUBLISH_RECHECK_ATTEMPTS=2 classify_publish_result foo 1.2.3 1 "$f"; r=$?; rm -f "$f"; exit $r'

if OUT=$(bash -c ". \"$SCRIPT\"; set +e; f=\$(mktemp); printf '%s\n' 'npm error code E503 upstream registry exploded' >\"\$f\"; package_state() { echo uncertain; }; PUBLISH_RECHECK_DELAY=0 PUBLISH_RECHECK_ATTEMPTS=1 classify_publish_result foo 1.2.3 7 \"\$f\"" 2>&1); then
  fail "classify: uncertain recheck should fail"
elif printf '%s' "$OUT" | grep -Fq 'registry state after recheck: uncertain' \
  && printf '%s' "$OUT" | grep -Fq 'npm error code E503 upstream registry exploded'; then
  pass "classify: failure repeats the original publish error text and the recheck state"
else
  fail "classify: failure output lost the original error: $OUT"
fi

# ── Integration: whole-script runs with mock npm/pnpm on PATH ─────────────────

# Build a throwaway PATH dir holding mock `npm` and `pnpm` executables.
MOCK_BIN=$(mktemp -d)
trap 'rm -rf "$MOCK_BIN" "${TEST_HOME:-}" "${PROV_LOG:-}" "${EXPECT_FILE:-}" "${PUB_LOG:-}" "${DISTTAG_LOG:-}" "${ARGS_LOG:-}" "${DIGEST_FIXTURE:-}" "${DC_GH_LOG:-}" "${DC_PUB_LOG:-}" "${DC_OUTPUT:-}" "${DC_MARKER:-}" "${LEGACY_TARGET:-}" "${LEGACY_OUTPUT:-}" "${LEGACY_PUB_LOG:-}" "${PUBLISH_TARGET:-}"' EXIT HUP INT TERM

# The OIDC preflight (scripts/npm-oidc-preflight.mjs) runs for real against
# these mocks: dummy GitHub ID-token request variables, an empty HOME so the
# developer's own ~/.npmrc never leaks into a case, and every stored-credential
# variable removed. Individual cases re-add one to prove the refusal.
ACTIONS_ID_TOKEN_REQUEST_URL='https://oidc.invalid/token?api-version=2.0'
ACTIONS_ID_TOKEN_REQUEST_TOKEN='dummy-request-token'
TEST_HOME=$(mktemp -d)
HOME=$TEST_HOME
export ACTIONS_ID_TOKEN_REQUEST_URL ACTIONS_ID_TOKEN_REQUEST_TOKEN HOME
# Absolute node: a version-manager shim (nodenv) would put the real npm ahead of
# the mock on the preflight's PATH.
ZFB_NPM_OIDC_PREFLIGHT_NODE=$NODE_EXE
ADVANCE_LATEST_RETRY_DELAY=0
export ZFB_NPM_OIDC_PREFLIGHT_NODE ADVANCE_LATEST_RETRY_DELAY
unset NODE_AUTH_TOKEN NPM_TOKEN NPM_CONFIG_USERCONFIG npm_config_userconfig XDG_CONFIG_HOME \
  ZFB_LOCAL_BUILT_PACKAGES ZFB_PROVENANCE_EXPECTATIONS_FILE ZFB_ALLOW_PROVENANCE_DOWNGRADE 2>/dev/null || true

cat >"$MOCK_BIN/npm" <<'MOCK_NPM'
#!/bin/sh
# Mock npm: `view <name@version> version`, `publish` (cwd package), `dist-tag add`,
# plus the preflight's `--version` and `config list`.
printf 'npm %s\n' "$*" >>"${MOCK_CALLS_LOG:-/dev/null}"
case "$1" in
  --version)
    printf '%s\n' "${MOCK_NPM_VERSION:-12.2.1}"
    exit 0
    ;;
  config)
    printf '%s\n' "${MOCK_NPM_CONFIG_LIST:-; \"user\" config from \$HOME/.npmrc}"
    exit 0
    ;;
  view)
    spec="$2"
    case "${3:-}" in dist-tags.*)
      # Mock dist-tag state: one file per package under MOCK_DISTTAG_DIR
      # (`latest`: <pkg>; any other tag: <tag>.<pkg>).
      tag="${3#dist-tags.}"
      prefix=""; [ "$tag" = latest ] || prefix="$tag."
      f="${MOCK_DISTTAG_DIR:-/nonexistent}/${prefix}$(printf '%s' "$spec" | tr '/@' '__')"
      [ -f "$f" ] && cat "$f"
      exit 0
      ;;
    esac
    if [ "$spec" = "@takazudo/zfb-md-wasm@${MOCK_VERSION:-}" ] && [ -f "${MOCK_PUBLISHED_MARKER:-/dev/null}" ]; then
      printf '%s\n' "$MOCK_VERSION"
      exit 0
    fi
    for e in $MOCK_EXISTING; do
      if [ "$e" = "$spec" ]; then
        printf '%s\n' "${spec##*@}"   # version part (after the last @)
        exit 0
      fi
    done
    exit 1   # not found — npm view exits non-zero
    ;;
  pack)
    if [ -n "${MOCK_PACK_MISSES_FILE:-}" ]; then
      misses=$(cat "$MOCK_PACK_MISSES_FILE")
      if [ "$misses" -gt 0 ]; then
        echo $((misses - 1)) >"$MOCK_PACK_MISSES_FILE"
        echo "npm error code ${MOCK_PACK_MISS_CODE:-ETARGET}" >&2
        echo "npm error notarget No matching version found for $2." >&2
        exit 1
      fi
    fi
    destination="$4"
    cp "$MOCK_PACK_TARBALL" "$destination/published.tgz"
    printf 'published.tgz\n'
    ;;
  publish)
    # `npm publish <tarball>`: the harness's fake tarballs hold "name<TAB>version".
    IFS="$(printf '\t')" read -r name version <"$2"
    printf '%s@%s\n' "$name" "$version" >>"$MOCK_PUBLISH_LOG"
    printf 'npm %s\n' "$*" >>"${MOCK_ARGS_LOG:-/dev/null}"
    printf '%s %s NPM_CONFIG_PROVENANCE=%s\n' "$name" "$*" "${NPM_CONFIG_PROVENANCE:-unset}" >>"${MOCK_PROV_LOG:-/dev/null}"
    if [ "$name" = "@takazudo/zfb-md-wasm" ]; then
      case "${MOCK_MD_RESULT:-success}" in
        # A real double-PUT conflict means the version IS on the registry.
        conflict) touch "$MOCK_PUBLISHED_MARKER"; echo 'EPUBLISHCONFLICT' >&2; exit 1 ;;
        lost) touch "$MOCK_PUBLISHED_MARKER"; echo 'ETIMEDOUT' >&2; exit 1 ;;
      esac
    fi
    exit 0
    ;;
  dist-tag)
    printf 'dist-tag %s\n' "$*" >>"${MOCK_DISTTAG_LOG:-/dev/null}"
    if [ "$2" = add ] && [ -n "${MOCK_DISTTAG_DIR:-}" ]; then
      spec="$3"; pkg="${spec%@*}"; ver="${spec##*@}"
      prefix=""; [ "${4:-latest}" = latest ] || prefix="$4."
      printf '%s\n' "$ver" >"$MOCK_DISTTAG_DIR/${prefix}$(printf '%s' "$pkg" | tr '/@' '__')"
    fi
    exit 0
    ;;
  *) exit 0 ;;
esac
MOCK_NPM

cat >"$MOCK_BIN/pnpm" <<'MOCK_PNPM'
#!/bin/sh
# Mock pnpm: record the --filter target of a `publish` invocation.
printf 'pnpm %s\n' "$*" >>"${MOCK_CALLS_LOG:-/dev/null}"
original_args="$*"
filter=""
is_publish=0
while [ $# -gt 0 ]; do
  case "$1" in
    --filter) filter="$2"; shift ;;
    publish) is_publish=1 ;;
  esac
  shift
done
if [ "$is_publish" -eq 1 ]; then
  printf '%s\n' "$filter" >>"$MOCK_PUBLISH_LOG"
  printf 'pnpm %s\n' "$original_args" >>"${MOCK_ARGS_LOG:-/dev/null}"
  printf '%s %s NPM_CONFIG_PROVENANCE=%s\n' "$filter" "$original_args" "${NPM_CONFIG_PROVENANCE:-unset}" >>"${MOCK_PROV_LOG:-/dev/null}"
fi
exit 0
MOCK_PNPM

chmod +x "$MOCK_BIN/npm" "$MOCK_BIN/pnpm"

cat >"$MOCK_BIN/gh" <<'MOCK_GH'
#!/bin/sh
printf '%s\n' "$*" >>"$MOCK_GH_LOG"
# The digest asset is reconciled, never clobbered: report an asset-less Release.
if [ "$1" = release ] && [ "$2" = view ]; then printf '{"assets":[]}\n'; fi
MOCK_GH
chmod +x "$MOCK_BIN/gh"

# Existing publish/provenance cases predate the manifest and test unrelated
# behavior. The dedicated digest cases below replace this with the real Node.
cat >"$MOCK_BIN/verify-noop" <<'MOCK_VERIFY'
#!/bin/sh
exit 0
MOCK_VERIFY
chmod +x "$MOCK_BIN/verify-noop"
ZFB_MD_WASM_VERIFY_NODE="$MOCK_BIN/verify-noop"
export ZFB_MD_WASM_VERIFY_NODE
# The tarball-content re-verification (verify-release-tarballs.mjs) needs real
# tarballs; the harness's fake ones are text. A dedicated case below runs the
# real verifier against a real, tampered tarball set.
ZFB_TARBALL_VERIFY_NODE="$MOCK_BIN/verify-noop"
export ZFB_TARBALL_VERIFY_NODE

# Mock for scripts/release/npm-package-state.mjs (invoked as
# "$ZFB_NPM_PACKAGE_STATE_NODE <script> --version V --source-commit S --package N --tsv").
# Presence comes from the mock npm's `view` (MOCK_EXISTING, the md-wasm lost-ACK
# marker); MOCK_STATE_UNVERIFIED / MOCK_STATE_UNCERTAIN name packages whose
# present version cannot be tied to the source commit / whose read fails.
cat >"$MOCK_BIN/state-node" <<'MOCK_STATE'
#!/bin/sh
shift
version="" name=""
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version="$2"; shift ;;
    --package) name="$2"; shift ;;
  esac
  shift
done
printf 'state %s@%s\n' "$name" "$version" >>"${MOCK_STATE_LOG:-/dev/null}"
for u in ${MOCK_STATE_UNCERTAIN:-}; do
  if [ "$u" = "$name" ]; then printf '%s\tuncertain\tGET failed: HTTP 503\n' "$name"; exit 0; fi
done
if npm view "$name@$version" version >/dev/null 2>&1; then
  for u in ${MOCK_STATE_UNVERIFIED:-}; do
    if [ "$u" = "$name" ]; then printf '%s\tpresent-unverified\tgitHead deadbeef is not source\n' "$name"; exit 0; fi
  done
  observed=none
  for a in ${MOCK_STATE_ATTESTED:-}; do [ "$a" = "$name" ] && observed=attested; done
  printf '%s\tpresent-verified\tgitHead\t%s\n' "$name" "$observed"
else
  printf '%s\tabsent\tE404 on 3 read(s)\n' "$name"
fi
MOCK_STATE
chmod +x "$MOCK_BIN/state-node"
ZFB_NPM_PACKAGE_STATE_NODE="$MOCK_BIN/state-node"
ZFB_RELEASE_SOURCE_COMMIT=0123456789abcdef0123456789abcdef01234567
export ZFB_NPM_PACKAGE_STATE_NODE ZFB_RELEASE_SOURCE_COMMIT

# The publish helper only publishes the pack job's tarballs (#4690). Every
# whole-script case runs through this wrapper, which plays the pack job for the
# current package tree: one fake tarball per package (content "name<TAB>version",
# so the mock npm can identify it) plus manifest.json with the mode's
# provenance decisions. Sourcing the wrapper sources the real script.
cat >"$MOCK_BIN/gen-manifest.mjs" <<'GEN_MANIFEST'
import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
const [out, mode, localRaw] = process.argv.slice(2);
const platform = ["packages/zfb-darwin-arm64", "packages/zfb-darwin-x64", "packages/zfb-linux-arm64-gnu", "packages/zfb-linux-x64-gnu", "packages/zfb-win32-x64-msvc"];
const dirs = [...platform, "packages/zfb-slugify", "packages/zfb", "packages/zfb-runtime", "packages/zfb-adapter-cloudflare", "packages/create-zfb", "crates/zfb-md-wasm/npm"];
let local = String(localRaw ?? "").split(/[\s,]+/).map((d) => d.replace(/\/$/, "")).filter(Boolean);
if (mode === "mac-local" && local.length === 0) local = ["packages/zfb-darwin-x64"];
const packages = [];
for (const dir of dirs) {
  if (!existsSync(join(dir, "package.json"))) continue;
  const { name, version } = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  const file = `${name.replace(/^@/, "").replace("/", "-")}-${version}.tgz`;
  const bytes = Buffer.from(`${name}\t${version}\n`);
  writeFileSync(join(out, file), bytes);
  const provenance = mode === "recovery-no-provenance" || (mode === "mac-local" && local.includes(dir)) ? "none" : "attested";
  packages.push({ name, version, dir, file, integrity: `sha512-${createHash("sha512").update(bytes).digest("base64")}`, provenance });
}
writeFileSync(join(out, "manifest.json"), JSON.stringify({ schema: "zfb-npm-tarballs/v1", mode, version: packages[0]?.version, packages }, null, 2));
GEN_MANIFEST
SCRIPT="$MOCK_BIN/publish-wrapper.sh"
cat >"$SCRIPT" <<WRAPPER
#!/usr/bin/env bash
if [[ "\${BASH_SOURCE[0]}" != "\$0" ]]; then
  . "$REAL_SCRIPT"
  return 0
fi
if [[ -z "\${ZFB_PUBLISH_TARBALLS_DIR:-}" ]]; then
  gen_dir="\$(mktemp -d)"
  "$NODE_EXE" "$MOCK_BIN/gen-manifest.mjs" "\$gen_dir" "\${1:-}" "\${ZFB_LOCAL_BUILT_PACKAGES-}" || exit 98
  export ZFB_PUBLISH_TARBALLS_DIR="\$gen_dir"
fi
rc=0
bash "$REAL_SCRIPT" "\$@" || rc=\$?
rm -rf "\${gen_dir:-/nonexistent-zfb-gen}"
exit "\$rc"
WRAPPER
chmod +x "$SCRIPT"

# Whole-script cases use an isolated target tree. The real control script and
# companion verifiers remain under REPO_ROOT, just as recovery separates them
# from the target package tree. Native placeholder bytes only test file presence;
# they never enter real carrier slots or establish native build/load evidence.
PUBLISH_TARGET=$(mktemp -d)
PLATFORM_PACKAGE_DIRS='
packages/zfb-darwin-arm64
packages/zfb-darwin-x64
packages/zfb-linux-arm64-gnu
packages/zfb-linux-x64-gnu
packages/zfb-win32-x64-msvc
'
for dir in $PLATFORM_PACKAGE_DIRS packages/zfb-slugify packages/zfb packages/zfb-runtime packages/zfb-adapter-cloudflare packages/create-zfb crates/zfb-md-wasm/npm; do
  mkdir -p "$PUBLISH_TARGET/$dir"
  cp "$REPO_ROOT/$dir/package.json" "$PUBLISH_TARGET/$dir/package.json"
done
for dir in $PLATFORM_PACKAGE_DIRS; do
  printf 'offline presence fixture; not a native addon\n' >"$PUBLISH_TARGET/$dir/zfb-css.node"
done
cd "$PUBLISH_TARGET"

V=$(node -p "require('./packages/zfb/package.json').version")
ALL_SPECS="@takazudo/zfb-darwin-arm64@$V @takazudo/zfb-darwin-x64@$V @takazudo/zfb-linux-arm64-gnu@$V @takazudo/zfb-linux-x64-gnu@$V @takazudo/zfb-win32-x64-msvc@$V @takazudo/zfb-slugify@$V @takazudo/zfb@$V @takazudo/zfb-runtime@$V @takazudo/zfb-adapter-cloudflare@$V create-zfb@$V @takazudo/zfb-md-wasm@$V"

# log_has <name> — true iff the publish log has a line for exactly <name>
# (matching <name> at line start followed by '@' or end-of-line, so
# "@takazudo/zfb" does not match "@takazudo/zfb-runtime").
log_has() {
  grep -Eq "^$1(@|$)" "$PUB_LOG"
}

# The CSS-addon and carrier-binary preflight moved to the pack job with the
# rest of package assembly (scripts/release/pack-release-tarballs.mjs, #4690);
# scripts/__tests__/pack-release-tarballs.test.mjs covers it.

# Case 1 — idempotent re-run: 9 packages already published (incl.
# @takazudo/zfb-slugify and @takazudo/zfb-md-wasm), only @takazudo/zfb-runtime
# + create-zfb missing.
# Expect exit 0 and ONLY the two missing ones published.
PUB_LOG=$(mktemp)
: >"$PUB_LOG"
if PATH="$MOCK_BIN:$PATH" \
   DIST_TAG=next \
   MOCK_PUBLISH_LOG="$PUB_LOG" \
   MOCK_EXISTING="@takazudo/zfb-darwin-arm64@$V @takazudo/zfb-darwin-x64@$V @takazudo/zfb-linux-arm64-gnu@$V @takazudo/zfb-linux-x64-gnu@$V @takazudo/zfb-win32-x64-msvc@$V @takazudo/zfb-slugify@$V @takazudo/zfb@$V @takazudo/zfb-adapter-cloudflare@$V @takazudo/zfb-md-wasm@$V" \
   bash "$SCRIPT" all-provenance >/dev/null 2>&1; then
  if log_has "@takazudo/zfb-runtime" && log_has "create-zfb" \
     && ! log_has "@takazudo/zfb" && ! log_has "@takazudo/zfb-slugify" \
     && ! log_has "@takazudo/zfb-adapter-cloudflare" \
     && ! log_has "@takazudo/zfb-darwin-x64" && ! log_has "@takazudo/zfb-md-wasm"; then
    pass "integration: idempotent re-run publishes ONLY the 2 missing packages"
  else
    fail "integration: idempotent re-run published the wrong set: $(tr '\n' ' ' <"$PUB_LOG")"
  fi
else
  fail "integration: idempotent re-run exited non-zero"
fi

# Case 2 — fresh publish: nothing on the registry → all 11 packages published
# (5 platform via npm publish, 6 non-platform via pnpm publish).
PUB_LOG=$(mktemp)
: >"$PUB_LOG"
if PATH="$MOCK_BIN:$PATH" \
   DIST_TAG=next \
   MOCK_PUBLISH_LOG="$PUB_LOG" \
   MOCK_EXISTING="" \
   bash "$SCRIPT" all-provenance >/dev/null 2>&1; then
  COUNT=$(grep -c . "$PUB_LOG" || true)
  SLUGIFY_LINE=$(grep -n -m 1 -E '^@takazudo/zfb-slugify(@|$)' "$PUB_LOG" | cut -d: -f1)
  ZFB_LINE=$(grep -n -m 1 -E '^@takazudo/zfb(@|$)' "$PUB_LOG" | cut -d: -f1)
  if [ "$COUNT" -eq 11 ] && log_has "@takazudo/zfb-darwin-x64" && log_has "create-zfb" && log_has "@takazudo/zfb-runtime" && log_has "@takazudo/zfb-md-wasm" \
     && [ -n "$SLUGIFY_LINE" ] && [ -n "$ZFB_LINE" ] && [ "$SLUGIFY_LINE" -lt "$ZFB_LINE" ]; then
    pass "integration: fresh registry publishes all 11 packages with slugify before zfb"
  else
    fail "integration: fresh publish count=$COUNT (want 11; slugify line=$SLUGIFY_LINE, zfb line=$ZFB_LINE): $(tr '\n' ' ' <"$PUB_LOG")"
  fi
else
  fail "integration: fresh publish exited non-zero"
fi

# Case 3 — holding tag (#4717): every publish goes out under `release-pending`,
# and the publish script never moves a channel tag itself, even with
# ALSO_LATEST=1 (promotion happens after verification, in
# scripts/promote-release-channel.sh).
PUB_LOG=$(mktemp)
ARGS_LOG=$(mktemp)
DISTTAG_LOG=$(mktemp)
DISTTAG_DIR=$(mktemp -d)
: >"$PUB_LOG"
: >"$ARGS_LOG"
: >"$DISTTAG_LOG"
if PATH="$MOCK_BIN:$PATH" \
   DIST_TAG=next \
   ALSO_LATEST=1 \
   MOCK_DISTTAG_DIR="$DISTTAG_DIR" \
   MOCK_PUBLISH_LOG="$PUB_LOG" \
   MOCK_ARGS_LOG="$ARGS_LOG" \
   MOCK_DISTTAG_LOG="$DISTTAG_LOG" \
   MOCK_EXISTING="" \
   bash "$SCRIPT" all-provenance >/dev/null 2>&1; then
  PUBLISHES=$(grep -c 'publish' "$ARGS_LOG" || true)
  HELD=$(grep -c -- '--tag release-pending' "$ARGS_LOG" || true)
  if [ "$PUBLISHES" -eq 11 ] && [ "$HELD" -eq 11 ] && [ ! -s "$DISTTAG_LOG" ] \
     && [ -z "$(ls -A "$DISTTAG_DIR")" ]; then
    pass "integration: all 11 publishes use --tag release-pending and no channel tag moves (even with ALSO_LATEST=1)"
  else
    fail "integration: holding tag (publishes=$PUBLISHES held=$HELD disttag=$(tr '\n' ' ' <"$DISTTAG_LOG")): $(tr '\n' ' ' <"$ARGS_LOG")"
  fi
else
  fail "integration: holding-tag run exited non-zero"
fi
rm -rf "$DISTTAG_DIR"

# Case 4 — bad mode argument is rejected (exit 2).
if PATH="$MOCK_BIN:$PATH" DIST_TAG=next bash "$SCRIPT" bogus-mode >/dev/null 2>&1; then
  fail "integration: bad mode argument should have failed"
else
  RC=$?
  if [ "$RC" -eq 2 ]; then
    pass "integration: bad mode argument rejected (exit 2)"
  else
    fail "integration: bad mode argument exit $RC (want 2)"
  fi
fi

# Case 5 — main-based tag recovery publishes every package without a misleading
# provenance flag (the workflow OIDC SHA is main, while artifacts come from the
# verified release tag SHA).
PUB_LOG=$(mktemp)
ARGS_LOG=$(mktemp)
: >"$PUB_LOG"
: >"$ARGS_LOG"
if PATH="$MOCK_BIN:$PATH" \
   DIST_TAG=latest \
   MOCK_PUBLISH_LOG="$PUB_LOG" \
   MOCK_ARGS_LOG="$ARGS_LOG" \
   MOCK_EXISTING="" \
   ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 \
   bash "$SCRIPT" recovery-no-provenance >/dev/null 2>&1; then
  COUNT=$(grep -c . "$PUB_LOG" || true)
  SUPPRESSED=$(grep -c -- '--provenance=false' "$ARGS_LOG" || true)
  if [ "$COUNT" -eq 11 ] && [ "$SUPPRESSED" -eq 11 ] && ! grep -Eq -- '--provenance( |$)' "$ARGS_LOG"; then
    pass "integration: recovery publishes all 11 packages with explicit --provenance=false"
  else
    fail "integration: recovery count=$COUNT (want 11), suppressed=$SUPPRESSED (want 11): $(tr '\n' ' ' <"$ARGS_LOG")"
  fi
else
  fail "integration: recovery-no-provenance run exited non-zero"
fi

# Case 6 — an older release-tag package tree has no slugify directory. The
# release-control script must omit only this newly-added package and still
# publish the older tag's ten packages completely.
LEGACY_TARGET=$(mktemp -d)
LEGACY_OUTPUT=$(mktemp)
LEGACY_PUB_LOG=$(mktemp)
: >"$LEGACY_PUB_LOG"
LEGACY_PACKAGE_DIRS='
packages/zfb-darwin-arm64
packages/zfb-darwin-x64
packages/zfb-linux-arm64-gnu
packages/zfb-linux-x64-gnu
packages/zfb-win32-x64-msvc
packages/zfb
packages/zfb-runtime
packages/zfb-adapter-cloudflare
packages/create-zfb
crates/zfb-md-wasm/npm
'
for dir in $LEGACY_PACKAGE_DIRS; do
  mkdir -p "$LEGACY_TARGET/$dir"
  cp "$dir/package.json" "$LEGACY_TARGET/$dir/package.json"
done
# A genuine pre-CSS recovery target has neither public CSS export nor native
# addons. Removing both export maps keeps the historical package contract honest.
LEGACY_TARGET="$LEGACY_TARGET" "$NODE_EXE" --input-type=module <<'LEGACY_MANIFEST'
import { readFileSync, writeFileSync } from 'node:fs';
const path = `${process.env.LEGACY_TARGET}/packages/zfb/package.json`;
const manifest = JSON.parse(readFileSync(path, 'utf8'));
delete manifest.exports?.['./css'];
delete manifest.publishConfig?.exports?.['./css'];
writeFileSync(path, JSON.stringify(manifest, null, 2) + '\n');
LEGACY_MANIFEST
if (
  cd "$LEGACY_TARGET"
  PATH="$MOCK_BIN:$PATH" \
    DIST_TAG=latest \
    MOCK_PUBLISH_LOG="$LEGACY_PUB_LOG" \
    MOCK_EXISTING="" \
    ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 \
    ZFB_MD_WASM_VERIFY_NODE="$MOCK_BIN/verify-noop" \
    bash "$SCRIPT" recovery-no-provenance
) >"$LEGACY_OUTPUT" 2>&1; then
  COUNT=$(grep -c . "$LEGACY_PUB_LOG" || true)
  if [ "$COUNT" -eq 10 ] && ! grep -Eq '^@takazudo/zfb-slugify(@|$)' "$LEGACY_PUB_LOG" \
     && grep -Fq 'target tree predates @takazudo/zfb-slugify' "$LEGACY_OUTPUT"; then
    pass "integration: older release tag omits absent slugify and publishes all 10 available packages"
  else
    fail "integration: older tag recovery count=$COUNT (want 10), slugify published=$(grep -Eq '^@takazudo/zfb-slugify(@|$)' "$LEGACY_PUB_LOG" && echo yes || echo no), output=$(tail -4 "$LEGACY_OUTPUT" | tr '\n' ' ')"
  fi
else
  fail "integration: older release tag recovery exited non-zero: $(tail -6 "$LEGACY_OUTPUT" | tr '\n' ' ')"
fi

# The same old target must fail closed for normal publishes; only recovery may
# omit the new package, and refusal must happen before any platform upload.
assert_missing_slugify_refused() {
  LEGACY_MODE="$1"
  : >"$LEGACY_PUB_LOG"
  : >"$LEGACY_OUTPUT"
  LEGACY_RC=0
  (
    cd "$LEGACY_TARGET"
    PATH="$MOCK_BIN:$PATH" \
      DIST_TAG=latest \
      MOCK_PUBLISH_LOG="$LEGACY_PUB_LOG" \
      MOCK_EXISTING="" \
      ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 \
      bash "$SCRIPT" "$LEGACY_MODE"
  ) >"$LEGACY_OUTPUT" 2>&1 || LEGACY_RC=$?

  if [ "$LEGACY_RC" -ne 0 ] && [ ! -s "$LEGACY_PUB_LOG" ] \
     && grep -Fq 'publish target is missing expected package' "$LEGACY_OUTPUT"; then
    pass "integration: $LEGACY_MODE refuses a missing slugify manifest before any publish"
  else
    fail "integration: $LEGACY_MODE missing-slugify refusal exit=$LEGACY_RC, publishes=$(cat "$LEGACY_PUB_LOG" | tr '\n' ' '), output=$(tail -4 "$LEGACY_OUTPUT" | tr '\n' ' ')"
  fi
}

assert_missing_slugify_refused all-provenance
assert_missing_slugify_refused mac-local

# Case 7 — trust-downgrade advisory (#2623). Both provenance-omitting modes must
# announce the downgrade loudly ONCE (::warning + job summary) so the release
# operator schedules the follow-up attested release; the fully-attested mode must
# stay silent, or the warning trains operators to ignore it.
# assert_advisory <desc> <mode> <want: yes|no> <scope-substring>
assert_advisory() {
  AD_DESC="$1"; AD_MODE="$2"; AD_WANT="$3"; AD_SCOPE="$4"
  AD_OUT=$(mktemp)
  AD_SUMMARY=$(mktemp)
  : >"$AD_SUMMARY"
  PATH="$MOCK_BIN:$PATH" \
  DIST_TAG=latest \
  MOCK_PUBLISH_LOG=/dev/null \
  MOCK_EXISTING="$ALL_SPECS" \
  GITHUB_STEP_SUMMARY="$AD_SUMMARY" \
  ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 \
    bash "$SCRIPT" "$AD_MODE" >"$AD_OUT" 2>&1 || true

  if grep -q '::warning title=npm provenance trust downgrade::' "$AD_OUT"; then
    AD_GOT=yes
  else
    AD_GOT=no
  fi

  if [ "$AD_GOT" != "$AD_WANT" ]; then
    fail "$AD_DESC (advisory emitted=$AD_GOT, want=$AD_WANT)"
  elif [ "$AD_WANT" = no ]; then
    if [ -s "$AD_SUMMARY" ]; then
      fail "$AD_DESC (no advisory expected, but the job summary was written)"
    else
      pass "$AD_DESC"
    fi
  elif ! grep -Fq "$AD_SCOPE" "$AD_OUT"; then
    fail "$AD_DESC (advisory did not name scope '$AD_SCOPE')"
  elif [ "$(grep -c '::warning title=npm provenance trust downgrade::' "$AD_OUT")" -ne 1 ]; then
    fail "$AD_DESC (advisory emitted more than once — it must be one-shot, not per-package)"
  elif ! grep -Fq 'ERR_PNPM_TRUST_DOWNGRADE' "$AD_SUMMARY"; then
    fail "$AD_DESC (job summary missing the consumer-facing error name)"
  else
    pass "$AD_DESC"
  fi
  rm -f "$AD_OUT" "$AD_SUMMARY"
}

assert_advisory "advisory: recovery-no-provenance warns for every package" \
  recovery-no-provenance yes "EVERY package in this release"
assert_advisory "advisory: mac-local warns, scoped to zfb-darwin-x64" \
  mac-local yes "@takazudo/zfb-darwin-x64"
assert_advisory "advisory: all-provenance stays silent" \
  all-provenance no ""

# ── ZFB_ALLOW_PROVENANCE_DOWNGRADE gate (A2, #3140) ─────────────────────────────
# Defense in depth: mac-local / recovery-no-provenance modes must refuse to
# publish ANYTHING unless the release.yml preflight has cleared them and set
# this env var to exactly "1". No mock npm/pnpm call may happen — the gate must
# fire before any publish, not merely alongside one.
# assert_downgrade_gate <desc> <mode> <env-assignment-or-empty> <want: blocked|allowed>
assert_downgrade_gate() {
  DG_DESC="$1"; DG_MODE="$2"; DG_ENV="$3"; DG_WANT="$4"
  DG_PUB_LOG=$(mktemp)
  : >"$DG_PUB_LOG"
  DG_RC=0
  if [ -n "$DG_ENV" ]; then
    PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$DG_PUB_LOG" MOCK_EXISTING="" \
      env "$DG_ENV" bash "$SCRIPT" "$DG_MODE" >/dev/null 2>&1 || DG_RC=$?
  else
    PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$DG_PUB_LOG" MOCK_EXISTING="" \
      bash "$SCRIPT" "$DG_MODE" >/dev/null 2>&1 || DG_RC=$?
  fi

  if [ "$DG_WANT" = blocked ]; then
    if [ "$DG_RC" -ne 0 ] && [ ! -s "$DG_PUB_LOG" ]; then
      pass "$DG_DESC"
    else
      fail "$DG_DESC (exit=$DG_RC, publish log $([ -s "$DG_PUB_LOG" ] && echo non-empty || echo empty))"
    fi
  else
    if [ "$DG_RC" -eq 0 ] && [ -s "$DG_PUB_LOG" ]; then
      pass "$DG_DESC"
    else
      fail "$DG_DESC (exit=$DG_RC, publish log $([ -s "$DG_PUB_LOG" ] && echo non-empty || echo empty))"
    fi
  fi
  rm -f "$DG_PUB_LOG"
}

assert_downgrade_gate "gate: mac-local WITHOUT the env var is blocked before any publish" \
  mac-local "" blocked
assert_downgrade_gate "gate: mac-local with the env var set to a non-1 value is blocked" \
  mac-local "ZFB_ALLOW_PROVENANCE_DOWNGRADE=true" blocked
assert_downgrade_gate "gate: mac-local WITH ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 proceeds" \
  mac-local "ZFB_ALLOW_PROVENANCE_DOWNGRADE=1" allowed
assert_downgrade_gate "gate: recovery-no-provenance WITHOUT the env var is blocked before any publish" \
  recovery-no-provenance "" blocked
assert_downgrade_gate "gate: recovery-no-provenance WITH ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 proceeds" \
  recovery-no-provenance "ZFB_ALLOW_PROVENANCE_DOWNGRADE=1" allowed
assert_downgrade_gate "gate: all-provenance is unaffected (no env var needed)" \
  all-provenance "" allowed

# ── OIDC-only auth preflight ─────────────────────────────────────────────────
# Every refusal must happen before any publish. `env -u`/assignment via `env`
# keeps each case's credential local to that one run.
# assert_oidc_refusal <desc> <env-args...>
assert_oidc_refusal() {
  OR_DESC="$1"; shift
  OR_PUB_LOG=$(mktemp)
  OR_OUT=$(mktemp)
  : >"$OR_PUB_LOG"
  OR_RC=0
  env "$@" PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$OR_PUB_LOG" MOCK_EXISTING="" \
    bash "$SCRIPT" all-provenance >"$OR_OUT" 2>&1 || OR_RC=$?
  if [ "$OR_RC" -ne 0 ] && [ ! -s "$OR_PUB_LOG" ] && grep -q 'npm OIDC preflight' "$OR_OUT"; then
    pass "$OR_DESC"
  else
    fail "$OR_DESC (exit=$OR_RC, publishes=$(tr '\n' ' ' <"$OR_PUB_LOG"), output=$(tail -3 "$OR_OUT" | tr '\n' ' '))"
  fi
  rm -f "$OR_PUB_LOG" "$OR_OUT"
}

assert_oidc_refusal "oidc: NODE_AUTH_TOKEN present → refused before any publish" NODE_AUTH_TOKEN=npm_dummy
assert_oidc_refusal "oidc: empty NODE_AUTH_TOKEN (setup-node slot) → refused" NODE_AUTH_TOKEN=
assert_oidc_refusal "oidc: NPM_TOKEN present → refused" NPM_TOKEN=npm_dummy
assert_oidc_refusal "oidc: npm_config_//registry.npmjs.org/:_authToken present → refused" 'npm_config_//registry.npmjs.org/:_authToken=npm_dummy'
assert_oidc_refusal "oidc: missing ACTIONS_ID_TOKEN_REQUEST_URL → refused" -u ACTIONS_ID_TOKEN_REQUEST_URL
assert_oidc_refusal "oidc: missing ACTIONS_ID_TOKEN_REQUEST_TOKEN → refused" -u ACTIONS_ID_TOKEN_REQUEST_TOKEN
assert_oidc_refusal "oidc: npm config list reports an _authToken → refused" \
  'MOCK_NPM_CONFIG_LIST=//registry.npmjs.org/:_authToken = (protected)'
assert_oidc_refusal "oidc: npm older than 12.2.0 → refused" MOCK_NPM_VERSION=11.20.0
printf '//registry.npmjs.org/:_authToken=${NODE_AUTH_TOKEN}\n' >"$PUBLISH_TARGET/.npmrc"
assert_oidc_refusal "oidc: project .npmrc with an _authToken line → refused"
rm -f "$PUBLISH_TARGET/.npmrc"

# ── Explicit provenance per package, per mode ────────────────────────────────
# prov_of <name> → attested | none | missing | ambiguous, from the mock's
# per-publish line "<name> <args...> NPM_CONFIG_PROVENANCE=<value>".
prov_of() {
  PO_LINE=$(grep -E "^$1 " "$PROV_LOG" || true)
  if [ -z "$PO_LINE" ]; then echo missing; return; fi
  if printf '%s\n' "$PO_LINE" | grep -Eq -- ' --provenance( |$)' \
     && ! printf '%s\n' "$PO_LINE" | grep -q -- '--provenance=false' \
     && printf '%s\n' "$PO_LINE" | grep -q 'NPM_CONFIG_PROVENANCE=unset$'; then
    echo attested
  elif printf '%s\n' "$PO_LINE" | grep -q -- ' --provenance=false' \
     && ! printf '%s\n' "$PO_LINE" | grep -Eq -- ' --provenance( |$)' \
     && printf '%s\n' "$PO_LINE" | grep -q 'NPM_CONFIG_PROVENANCE=false$'; then
    echo none
  else
    echo ambiguous
  fi
}

ALL_NAMES="@takazudo/zfb-darwin-arm64 @takazudo/zfb-darwin-x64 @takazudo/zfb-linux-arm64-gnu @takazudo/zfb-linux-x64-gnu @takazudo/zfb-win32-x64-msvc @takazudo/zfb-slugify @takazudo/zfb @takazudo/zfb-runtime @takazudo/zfb-adapter-cloudflare create-zfb @takazudo/zfb-md-wasm"

# assert_prov_mode <desc> <mode> <local-built-or-empty> <space-separated names expected none>
assert_prov_mode() {
  PM_DESC="$1"; PM_MODE="$2"; PM_LOCAL="$3"; PM_NONE="$4"
  PROV_LOG=$(mktemp)
  EXPECT_FILE=$(mktemp)
  : >"$PROV_LOG"
  PM_RC=0
  PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG=/dev/null MOCK_EXISTING="" \
    MOCK_PROV_LOG="$PROV_LOG" ZFB_PROVENANCE_EXPECTATIONS_FILE="$EXPECT_FILE" \
    ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 ZFB_LOCAL_BUILT_PACKAGES="$PM_LOCAL" \
    bash "$SCRIPT" "$PM_MODE" >/dev/null 2>&1 || PM_RC=$?
  PM_BAD=""
  for n in $ALL_NAMES; do
    want=attested
    for x in $PM_NONE; do [ "$x" = "$n" ] && want=none; done
    got=$(prov_of "$n")
    exp=$(awk -F '\t' -v n="$n" '$1 == n { print $3 }' "$EXPECT_FILE")
    if [ "$got" != "$want" ] || [ "$exp" != "$want" ]; then
      PM_BAD="$PM_BAD $n(flag=$got,map=$exp,want=$want)"
    fi
  done
  if [ "$PM_RC" -eq 0 ] && [ -z "$PM_BAD" ] && [ "$(grep -c . "$EXPECT_FILE")" -eq 11 ]; then
    pass "$PM_DESC"
  else
    fail "$PM_DESC (exit=$PM_RC, mismatches:$PM_BAD)"
  fi
}

assert_prov_mode "provenance: all-provenance passes --provenance for all 11" all-provenance "" ""
assert_prov_mode "provenance: recovery passes --provenance=false (+env) for all 11" recovery-no-provenance "" "$ALL_NAMES"
assert_prov_mode "provenance: mac-local default suppresses only zfb-darwin-x64" mac-local "" "@takazudo/zfb-darwin-x64"
assert_prov_mode "provenance: mac-local with two darwin carriers suppresses exactly both" mac-local \
  "packages/zfb-darwin-arm64,packages/zfb-darwin-x64" "@takazudo/zfb-darwin-arm64 @takazudo/zfb-darwin-x64"

# Already-present packages are still recorded in the expectation map.
EXPECT_FILE=$(mktemp)
PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG=/dev/null MOCK_EXISTING="$ALL_SPECS" \
  MOCK_STATE_ATTESTED="$ALL_NAMES" \
  ZFB_PROVENANCE_EXPECTATIONS_FILE="$EXPECT_FILE" bash "$SCRIPT" all-provenance >/dev/null 2>&1 || true
if [ "$(grep -c "	attested	already-present$" "$EXPECT_FILE")" -eq 11 ]; then
  pass "provenance: already-present skips are recorded as expectations"
else
  fail "provenance: already-present expectation map: $(tr '\n' '|' <"$EXPECT_FILE")"
fi

# Mixed state (#4690 M2): a normal publish attested five packages and then
# failed; a recovery run (all --provenance=false) resumes it. The already-present
# five must keep their OBSERVED expectation (attested), the rest are none.
MIXED_PRESENT="@takazudo/zfb-linux-arm64-gnu @takazudo/zfb-linux-x64-gnu @takazudo/zfb-win32-x64-msvc @takazudo/zfb-slugify @takazudo/zfb"
MIXED_SPECS=""
for n in $MIXED_PRESENT; do MIXED_SPECS="$MIXED_SPECS $n@$V"; done
EXPECT_FILE=$(mktemp)
MX_RC=0
PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG=/dev/null MOCK_EXISTING="$MIXED_SPECS" \
  MOCK_STATE_ATTESTED="$MIXED_PRESENT" ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 \
  ZFB_PROVENANCE_EXPECTATIONS_FILE="$EXPECT_FILE" bash "$SCRIPT" recovery-no-provenance >/dev/null 2>&1 || MX_RC=$?
MX_BAD=""
for n in $ALL_NAMES; do
  want="none	publish-accepted"
  for p in $MIXED_PRESENT; do [ "$p" = "$n" ] && want="attested	already-present"; done
  got=$(awk -F '\t' -v n="$n" '$1 == n { print $3 "\t" $4 }' "$EXPECT_FILE")
  [ "$got" = "$want" ] || MX_BAD="$MX_BAD $n($got)"
done
if [ "$MX_RC" -eq 0 ] && [ -z "$MX_BAD" ]; then
  pass "provenance: partial normal publish resumed by recovery records observed attested + new none"
else
  fail "provenance: mixed partial+recovery rc=$MX_RC mismatches:$MX_BAD"
fi

# The pack manifest's provenance decision must match this mode's derivation.
PD_DIR=$(mktemp -d)
"$NODE_EXE" "$MOCK_BIN/gen-manifest.mjs" "$PD_DIR" all-provenance ""
PD_LOG=$(mktemp)
PD_OUT=$(mktemp)
PD_RC=0
PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$PD_LOG" MOCK_EXISTING="" \
  ZFB_PUBLISH_TARBALLS_DIR="$PD_DIR" bash "$SCRIPT" all-provenance >/dev/null 2>&1 || PD_RC=$?
# Tamper: claim the manifest is mac-local with x64 unattested, publish as all-provenance.
sed -i.bak 's/"mode": "all-provenance"/"mode": "mac-local"/' "$PD_DIR/manifest.json"
PD2_RC=0
PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$PD_LOG" MOCK_EXISTING="" \
  ZFB_PUBLISH_TARBALLS_DIR="$PD_DIR" bash "$SCRIPT" all-provenance >"$PD_OUT" 2>&1 || PD2_RC=$?
if [ "$PD_RC" -eq 0 ] && [ "$PD2_RC" -ne 0 ] && grep -Fq 'was packed for mode mac-local' "$PD_OUT"; then
  pass "manifest: a manifest packed for another mode is refused"
else
  fail "manifest: mode mismatch rc=$PD_RC/$PD2_RC out=$(tail -2 "$PD_OUT" | tr '\n' ' ')"
fi
PD3_DIR=$(mktemp -d)
"$NODE_EXE" "$MOCK_BIN/gen-manifest.mjs" "$PD3_DIR" mac-local "packages/zfb-darwin-x64"
: >"$PD_LOG"
PD3_RC=0
PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$PD_LOG" MOCK_EXISTING="" \
  ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 ZFB_LOCAL_BUILT_PACKAGES="packages/zfb-darwin-arm64 packages/zfb-darwin-x64" \
  ZFB_PUBLISH_TARBALLS_DIR="$PD3_DIR" bash "$SCRIPT" mac-local >"$PD_OUT" 2>&1 || PD3_RC=$?
if [ "$PD3_RC" -ne 0 ] && [ ! -s "$PD_LOG" ] && grep -Fq "@takazudo/zfb-darwin-arm64: the pack manifest decided provenance 'attested'" "$PD_OUT"; then
  pass "manifest: a provenance decision that disagrees with the mode is refused before any publish"
else
  fail "manifest: decision mismatch rc=$PD3_RC publishes=$(tr '\n' ' ' <"$PD_LOG") out=$(tail -3 "$PD_OUT" | tr '\n' ' ')"
fi
# A tarball whose bytes changed after packing is refused before any publish.
PD4_DIR=$(mktemp -d)
"$NODE_EXE" "$MOCK_BIN/gen-manifest.mjs" "$PD4_DIR" all-provenance ""
printf 'tampered\n' >>"$PD4_DIR/takazudo-zfb-$V.tgz"
: >"$PD_LOG"
PD4_RC=0
PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$PD_LOG" MOCK_EXISTING="" \
  ZFB_PUBLISH_TARBALLS_DIR="$PD4_DIR" bash "$SCRIPT" all-provenance >"$PD_OUT" 2>&1 || PD4_RC=$?
if [ "$PD4_RC" -ne 0 ] && [ ! -s "$PD_LOG" ] && grep -Fq 'differs from the manifest' "$PD_OUT"; then
  pass "manifest: a tarball whose sha512 differs from the manifest is refused before any publish"
else
  fail "manifest: integrity tamper rc=$PD4_RC publishes=$(tr '\n' ' ' <"$PD_LOG")"
fi
rm -rf "$PD_DIR" "$PD3_DIR" "$PD4_DIR" "$PD_LOG" "$PD_OUT"

# assert_local_built_refused <desc> <mode> <value>
assert_local_built_refused() {
  LB_PUB_LOG=$(mktemp)
  : >"$LB_PUB_LOG"
  LB_RC=0
  PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$LB_PUB_LOG" MOCK_EXISTING="" \
    ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 ZFB_LOCAL_BUILT_PACKAGES="$3" \
    bash "$SCRIPT" "$2" >/dev/null 2>&1 || LB_RC=$?
  if [ "$LB_RC" -ne 0 ] && [ ! -s "$LB_PUB_LOG" ]; then
    pass "$1"
  else
    fail "$1 (exit=$LB_RC, publishes=$(tr '\n' ' ' <"$LB_PUB_LOG"))"
  fi
  rm -f "$LB_PUB_LOG"
}
assert_local_built_refused "local-built: unknown package dir is refused" mac-local "packages/zfb-darwin-x64 packages/zfb"
assert_local_built_refused "local-built: set under all-provenance is refused" all-provenance "packages/zfb-darwin-x64"
assert_local_built_refused "local-built: set under recovery is refused" recovery-no-provenance "packages/zfb-darwin-arm64"

# Two-darwin mac-local still needs the downgrade acknowledgement.
DG2_LOG=$(mktemp)
: >"$DG2_LOG"
DG2_RC=0
PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$DG2_LOG" MOCK_EXISTING="" \
  ZFB_LOCAL_BUILT_PACKAGES="packages/zfb-darwin-arm64 packages/zfb-darwin-x64" \
  bash "$SCRIPT" mac-local >/dev/null 2>&1 || DG2_RC=$?
if [ "$DG2_RC" -ne 0 ] && [ ! -s "$DG2_LOG" ]; then
  pass "gate: two-darwin mac-local without ZFB_ALLOW_PROVENANCE_DOWNGRADE is blocked"
else
  fail "gate: two-darwin mac-local without the env var (exit=$DG2_RC)"
fi
rm -f "$DG2_LOG"

# The advisory names every locally-built carrier.
ADV2_OUT=$(mktemp)
PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG=/dev/null MOCK_EXISTING="$ALL_SPECS" \
  ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 ZFB_LOCAL_BUILT_PACKAGES="packages/zfb-darwin-arm64 packages/zfb-darwin-x64" \
  bash "$SCRIPT" mac-local >"$ADV2_OUT" 2>&1 || true
if grep '::warning title=npm provenance trust downgrade::' "$ADV2_OUT" | grep -q '@takazudo/zfb-darwin-arm64, @takazudo/zfb-darwin-x64'; then
  pass "advisory: two-darwin mac-local names both carriers"
else
  fail "advisory: two-darwin mac-local scope: $(grep '::warning' "$ADV2_OUT" | cut -c1-200)"
fi
rm -f "$ADV2_OUT"

# ── advance-latest-dist-tag.sh read-compare-write ───────────────────────────
ADV_SCRIPT="$REPO_ROOT/scripts/advance-latest-dist-tag.sh"

# run_advance <target> <seed-latest-or-empty> [advance args...] → sets ADV_RC,
# ADV_OUT, ADV_DIR, ADV_LOG. ADV_SEED_NEXT seeds `next` the same way.
run_advance() {
  ADV_DIR=$(mktemp -d)
  ADV_LOG=$(mktemp)
  ADV_OUT=$(mktemp)
  : >"$ADV_LOG"
  ADV_TARGET="$1"
  if [ -n "$2" ]; then
    for n in $ALL_NAMES; do printf '%s\n' "$2" >"$ADV_DIR/$(printf '%s' "$n" | tr '/@' '__')"; done
  fi
  if [ -n "${ADV_SEED_NEXT:-}" ]; then
    for n in $ALL_NAMES; do printf '%s\n' "$ADV_SEED_NEXT" >"$ADV_DIR/next.$(printf '%s' "$n" | tr '/@' '__')"; done
  fi
  shift 2
  ADV_RC=0
  PATH="${ADV_PATH:-$MOCK_BIN}:$PATH" MOCK_DISTTAG_DIR="$ADV_DIR" MOCK_DISTTAG_LOG="$ADV_LOG" \
    bash "$ADV_SCRIPT" "$@" "$ADV_TARGET" >"$ADV_OUT" 2>&1 || ADV_RC=$?
}

run_advance 1.2.0-next.3 1.2.0-next.2
if [ "$ADV_RC" -eq 0 ] && [ "$(grep -c 'dist-tag add' "$ADV_LOG")" -eq 11 ] \
   && [ "$(cat "$ADV_DIR/_takazudo_zfb")" = 1.2.0-next.3 ]; then
  pass "advance-latest: moves latest forward for all 11 and confirms by re-read"
else
  fail "advance-latest: forward move (exit=$ADV_RC, adds=$(grep -c 'dist-tag add' "$ADV_LOG"))"
fi

run_advance 1.2.0-next.3 1.2.0-next.10
if [ "$ADV_RC" -eq 0 ] && [ ! -s "$ADV_LOG" ] && grep -q "'latest' already points past 1.2.0-next.3" "$ADV_OUT"; then
  pass "advance-latest: a newer latest (next.10 > next.3) is left alone on every package, no write, exit 0"
else
  fail "advance-latest: regression guard (exit=$ADV_RC, writes=$(tr '\n' ' ' <"$ADV_LOG"))"
fi

run_advance 1.2.0-next.3 1.1.0
if [ "$ADV_RC" -ne 0 ] && [ ! -s "$ADV_LOG" ] && grep -q 'is a stable release — refusing to move .latest. to the prerelease' "$ADV_OUT"; then
  pass "advance-latest: never moves a stable latest (1.1.0) to a higher prerelease (1.2.0-next.3), no write"
else
  fail "advance-latest: stable→prerelease guard (exit=$ADV_RC, writes=$(tr '\n' ' ' <"$ADV_LOG"))"
fi

run_advance 1.2.0-next.3 1.2.0-next.3
if [ "$ADV_RC" -eq 0 ] && [ ! -s "$ADV_LOG" ]; then
  pass "advance-latest: latest already at target → no write"
else
  fail "advance-latest: idempotent (exit=$ADV_RC, writes=$(tr '\n' ' ' <"$ADV_LOG"))"
fi

RACY_BIN=$(mktemp -d)
cp "$MOCK_BIN/npm" "$RACY_BIN/npm-real"
cat >"$RACY_BIN/npm" <<'RACY'
#!/bin/sh
"$(dirname "$0")/npm-real" "$@"
rc=$?
if [ "$1" = dist-tag ] && [ "$2" = add ] && [ -n "${MOCK_RACE_VERSION:-}" ]; then
  printf '%s\n' "$MOCK_RACE_VERSION" >"$MOCK_DISTTAG_DIR/_takazudo_zfb"
fi
exit $rc
RACY
chmod +x "$RACY_BIN/npm" "$RACY_BIN/npm-real"
ADV_PATH="$RACY_BIN:$MOCK_BIN" MOCK_RACE_VERSION=1.2.0-next.9
export MOCK_RACE_VERSION
run_advance 1.2.0-next.3 1.2.0-next.2
unset MOCK_RACE_VERSION ADV_PATH
if [ "$ADV_RC" -eq 0 ] && [ "$(cat "$ADV_DIR/_takazudo_zfb")" = 1.2.0-next.9 ] \
   && grep -q 'is never moved backwards' "$ADV_OUT"; then
  pass "advance-latest: a concurrent newer latest seen on re-read is detected and never overwritten"
else
  fail "advance-latest: concurrent change (exit=$ADV_RC, zfb latest=$(cat "$ADV_DIR/_takazudo_zfb"), out=$(grep -i 'zfb:' "$ADV_OUT" | head -3 | tr '\n' ' '))"
fi
rm -rf "$RACY_BIN"

ADV_RC=0
PATH="$MOCK_BIN:$PATH" NODE_AUTH_TOKEN=npm_dummy MOCK_DISTTAG_LOG="$ADV_LOG" \
  bash "$ADV_SCRIPT" 1.2.3 >"$ADV_OUT" 2>&1 || ADV_RC=$?
: >"$ADV_LOG"
if [ "$ADV_RC" -ne 0 ] && grep -q 'npm OIDC preflight' "$ADV_OUT"; then
  pass "advance-latest: refuses to run with a stored token in the environment"
else
  fail "advance-latest: token refusal (exit=$ADV_RC)"
fi

# ── next channel guards (#4717) ─────────────────────────────────────────────
ADV_SEED_NEXT=1.2.0-next.2
run_advance 1.2.0-next.3 "" --tag next
if [ "$ADV_RC" -eq 0 ] && [ "$(grep -c 'dist-tag add' "$ADV_LOG")" -eq 11 ] \
   && ! grep -q ' latest$' "$ADV_LOG" \
   && [ "$(cat "$ADV_DIR/next._takazudo_zfb")" = 1.2.0-next.3 ] && [ ! -f "$ADV_DIR/_takazudo_zfb" ]; then
  pass "advance next: moves next forward for all 11 and leaves latest alone"
else
  fail "advance next: forward move (exit=$ADV_RC, writes=$(tr '\n' ' ' <"$ADV_LOG"))"
fi

ADV_SEED_NEXT=1.2.0-next.10
run_advance 1.2.0-next.3 "" --tag next
if [ "$ADV_RC" -eq 0 ] && [ ! -s "$ADV_LOG" ] && grep -q "'next' already points past 1.2.0-next.3" "$ADV_OUT"; then
  pass "advance next: never moves next backwards, no write, exit 0"
else
  fail "advance next: backwards guard (exit=$ADV_RC, writes=$(tr '\n' ' ' <"$ADV_LOG"))"
fi
unset ADV_SEED_NEXT

run_advance 1.2.0-next.3 "" --tag next
if [ "$ADV_RC" -eq 0 ] && [ "$(grep -c 'dist-tag add' "$ADV_LOG")" -eq 11 ]; then
  pass "advance next: an absent next is created"
else
  fail "advance next: absent next (exit=$ADV_RC)"
fi

run_advance 1.2.0 "" --tag next
if [ "$ADV_RC" -ne 0 ] && [ ! -s "$ADV_LOG" ] && grep -q "only carries" "$ADV_OUT"; then
  pass "advance next: refuses to point next at a stable version, no write"
else
  fail "advance next: stable guard (exit=$ADV_RC, writes=$(tr '\n' ' ' <"$ADV_LOG"))"
fi

run_advance 1.2.0-alpha.1 "" --tag next
if [ "$ADV_RC" -ne 0 ] && [ ! -s "$ADV_LOG" ]; then
  pass "advance next: refuses a prerelease outside next/beta/rc, no write"
else
  fail "advance next: alpha guard (exit=$ADV_RC)"
fi

# Plan-before-write: one package's guard refusal on `latest` stops the whole
# run before ANY write, including the `next` moves that would have passed.
ADV_SEED_NEXT=1.2.0-next.2
ADV_DIR_SEED_ZFB=1.1.0
run_advance_partial() {
  ADV_DIR=$(mktemp -d)
  ADV_LOG=$(mktemp)
  ADV_OUT=$(mktemp)
  for n in $ALL_NAMES; do
    f=$(printf '%s' "$n" | tr '/@' '__')
    printf '%s\n' 1.2.0-next.2 >"$ADV_DIR/$f"
    printf '%s\n' 1.2.0-next.2 >"$ADV_DIR/next.$f"
  done
  printf '%s\n' "$ADV_DIR_SEED_ZFB" >"$ADV_DIR/_takazudo_zfb-md-wasm"
  ADV_RC=0
  PATH="$MOCK_BIN:$PATH" MOCK_DISTTAG_DIR="$ADV_DIR" MOCK_DISTTAG_LOG="$ADV_LOG" \
    bash "$ADV_SCRIPT" --tag next --tag latest 1.2.0-next.3 >"$ADV_OUT" 2>&1 || ADV_RC=$?
}
run_advance_partial
if [ "$ADV_RC" -ne 0 ] && [ ! -s "$ADV_LOG" ] && grep -q 'no dist-tag was moved' "$ADV_OUT" \
   && [ "$(cat "$ADV_DIR/next._takazudo_zfb")" = 1.2.0-next.2 ]; then
  pass "advance: one refused package stops every write — no half-moved channel"
else
  fail "advance: plan-before-write (exit=$ADV_RC, writes=$(tr '\n' ' ' <"$ADV_LOG"))"
fi
unset ADV_SEED_NEXT ADV_DIR_SEED_ZFB

run_advance 1.2.0-next.3 "" --tag latest --tag latest
if [ "$ADV_RC" -ne 0 ] && [ ! -s "$ADV_LOG" ]; then
  pass "advance: a duplicated --tag is refused"
else
  fail "advance: duplicate tag (exit=$ADV_RC)"
fi

# ── promote-release-channel.sh: the all-verified gate (#4717) ────────────────
PROMOTE_SCRIPT="$REPO_ROOT/scripts/promote-release-channel.sh"
# Stub for npm-package-state.mjs --require-all-verified --tsv: all 11 rows,
# MOCK_PROMOTE_PROCESSING names packages still in npm processing.
cat >"$MOCK_BIN/promote-state-node" <<'MOCK_PSTATE'
#!/bin/sh
printf 'promote-state %s\n' "$*" >>"${MOCK_STATE_LOG:-/dev/null}"
rc=0
for n in $MOCK_ALL_NAMES; do
  state=present-verified
  for p in ${MOCK_PROMOTE_PROCESSING:-}; do [ "$p" = "$n" ] && state=processing; done
  [ "$state" = present-verified ] || rc=1
  printf '%s\t%s\tgitHead\tnone\n' "$n" "$state"
done
exit $rc
MOCK_PSTATE
chmod +x "$MOCK_BIN/promote-state-node"

# run_promote <version> <seed-latest> [env...] → PR_RC, PR_OUT, PR_DIR, PR_LOG
run_promote() {
  PR_DIR=$(mktemp -d)
  PR_LOG=$(mktemp)
  PR_OUT=$(mktemp)
  PR_VER="$1"; PR_SEED="$2"; shift 2
  for n in $ALL_NAMES; do printf '%s\n' "$PR_SEED" >"$PR_DIR/$(printf '%s' "$n" | tr '/@' '__')"; done
  PR_RC=0
  env "$@" PATH="$MOCK_BIN:$PATH" MOCK_DISTTAG_DIR="$PR_DIR" MOCK_DISTTAG_LOG="$PR_LOG" \
    MOCK_ALL_NAMES="$ALL_NAMES" ZFB_NPM_PACKAGE_STATE_NODE="$MOCK_BIN/promote-state-node" \
    bash "$PROMOTE_SCRIPT" "$PR_VER" >"$PR_OUT" 2>&1 || PR_RC=$?
}

run_promote 1.3.0 1.2.0 DIST_TAG=latest
if [ "$PR_RC" -eq 0 ] && [ "$(grep -c 'dist-tag add .* latest$' "$PR_LOG")" -eq 11 ] \
   && [ "$(cat "$PR_DIR/_takazudo_zfb-linux-arm64-gnu")" = 1.3.0 ]; then
  pass "promote: stable moves latest on all 11 once every package is present-verified"
else
  fail "promote: stable (exit=$PR_RC, writes=$(tr '\n' ' ' <"$PR_LOG"), out=$(tail -3 "$PR_OUT" | tr '\n' ' '))"
fi

run_promote 1.3.0 1.2.0 DIST_TAG=latest MOCK_PROMOTE_PROCESSING=@takazudo/zfb-linux-arm64-gnu
if [ "$PR_RC" -ne 0 ] && [ ! -s "$PR_LOG" ] && grep -q 'npm still processing' "$PR_OUT" \
   && [ "$(cat "$PR_DIR/_takazudo_zfb")" = 1.2.0 ]; then
  pass "promote: a processing package leaves latest untouched on every package"
else
  fail "promote: processing gate (exit=$PR_RC, writes=$(tr '\n' ' ' <"$PR_LOG"))"
fi

run_promote 1.3.0-next.2 1.3.0-next.1 DIST_TAG=next ALSO_LATEST=1
if [ "$PR_RC" -eq 0 ] && [ "$(grep -c 'dist-tag add .* next$' "$PR_LOG")" -eq 11 ] \
   && [ "$(grep -c 'dist-tag add .* latest$' "$PR_LOG")" -eq 11 ] \
   && [ "$(cat "$PR_DIR/next._takazudo_zfb")" = 1.3.0-next.2 ] \
   && [ "$(cat "$PR_DIR/_takazudo_zfb")" = 1.3.0-next.2 ]; then
  pass "promote: prerelease moves next and, under ALSO_LATEST, latest after verification"
else
  fail "promote: prerelease + ALSO_LATEST (exit=$PR_RC, writes=$(tr '\n' ' ' <"$PR_LOG"))"
fi

run_promote 1.3.0-next.2 1.2.0 DIST_TAG=next
if [ "$PR_RC" -eq 0 ] && [ "$(grep -c 'dist-tag add .* next$' "$PR_LOG")" -eq 11 ] \
   && ! grep -q ' latest$' "$PR_LOG" && [ "$(cat "$PR_DIR/_takazudo_zfb")" = 1.2.0 ]; then
  pass "promote: prerelease without ALSO_LATEST moves next only"
else
  fail "promote: prerelease only (exit=$PR_RC, writes=$(tr '\n' ' ' <"$PR_LOG"))"
fi

run_promote 1.3.0-next.2 1.3.0-next.1 DIST_TAG=next ALSO_LATEST=1 MOCK_PROMOTE_PROCESSING="create-zfb @takazudo/zfb-md-wasm"
if [ "$PR_RC" -ne 0 ] && [ ! -s "$PR_LOG" ]; then
  pass "promote: prerelease with processing packages moves neither next nor latest"
else
  fail "promote: prerelease processing gate (exit=$PR_RC, writes=$(tr '\n' ' ' <"$PR_LOG"))"
fi

run_promote 1.3.0 1.2.0 DIST_TAG=next
if [ "$PR_RC" -ne 0 ] && [ ! -s "$PR_LOG" ]; then
  pass "promote: refuses a DIST_TAG that disagrees with the version"
else
  fail "promote: DIST_TAG mismatch (exit=$PR_RC)"
fi

run_promote 1.3.0 1.2.0 DIST_TAG=latest ZFB_RELEASE_SOURCE_COMMIT=
if [ "$PR_RC" -ne 0 ] && [ ! -s "$PR_LOG" ]; then
  pass "promote: refuses without the release source commit"
else
  fail "promote: missing source commit (exit=$PR_RC)"
fi

# ── Published MD/WASM decision seam and actual upload gate ──────────────────

DIGEST_FIXTURE=$(mktemp -d)
mkdir -p "$DIGEST_FIXTURE/packed/package/dist"
cp crates/zfb-md-wasm/npm/package.json "$DIGEST_FIXTURE/packed/package/package.json"
MOCK_VERSION=$V
export MOCK_VERSION
DIGEST_FIXTURE="$DIGEST_FIXTURE" node --input-type=module <<'FIXTURE'
import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
const root = process.env.DIGEST_FIXTURE;
const paths = [
  ['.', 'dist/wasm/zfb_md_wasm_bg.wasm'],
  ['./highlight', 'dist/wasm-highlight/zfb_md_wasm_highlight_bg.wasm'],
  ['./render', 'dist/wasm-render/zfb_md_wasm_render_bg.wasm'],
  ['./parse', 'dist/wasm-parse/zfb_md_wasm_parse_bg.wasm'],
];
const artifacts = paths.map(([entry, path]) => {
  const bytes = Buffer.from(`fixture:${entry}:${process.env.MOCK_VERSION}`);
  const file = join(root, 'packed/package', path);
  mkdirSync(join(file, '..'), { recursive: true });
  writeFileSync(file, bytes);
  return { entry, path, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
});
const manifest = JSON.stringify({ schemaVersion: 1, name: '@takazudo/zfb-md-wasm', version: process.env.MOCK_VERSION, artifacts }) + '\n';
writeFileSync(join(root, 'manifest.json'), manifest);
writeFileSync(join(root, 'packed/package/dist/shipped-artifacts.json'), manifest);
FIXTURE
tar -czf "$DIGEST_FIXTURE/good.tgz" -C "$DIGEST_FIXTURE/packed" package
mkdir -p "$DIGEST_FIXTURE/bad/package"
cp -R "$DIGEST_FIXTURE/packed/package/." "$DIGEST_FIXTURE/bad/package/"
printf 'wrong published bytes' >"$DIGEST_FIXTURE/bad/package/dist/wasm/zfb_md_wasm_bg.wasm"
tar -czf "$DIGEST_FIXTURE/bad.tgz" -C "$DIGEST_FIXTURE/bad" package

# Each case runs the real publish decision, then the same verifier with --upload
# that release.yml invokes. The mock gh log records the actual upload command.
assert_digest_case() {
  DC_DESC=$1 DC_EXISTING=$2 DC_RESULT=$3 DC_TARBALL=$4 DC_MANIFEST=$5 DC_WANT=$6
  DC_GH_LOG=$(mktemp)
  DC_PUB_LOG=$(mktemp)
  DC_OUTPUT=$(mktemp)
  DC_MARKER=$(mktemp)
  rm -f "$DC_MARKER"
  : >"$DC_GH_LOG"
  DC_RC=0
  DC_MODE=all-provenance
  if [ "$DC_WANT" = older ]; then DC_MODE=recovery-no-provenance; fi
  (
    if [ "$DC_WANT" = older ]; then cd "$LEGACY_TARGET"; fi
    PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_EXISTING="$DC_EXISTING" \
    MOCK_MD_RESULT="$DC_RESULT" MOCK_PACK_TARBALL="$DC_TARBALL" \
    MOCK_PUBLISH_LOG="$DC_PUB_LOG" MOCK_GH_LOG="$DC_GH_LOG" \
    MOCK_PUBLISHED_MARKER="$DC_MARKER" ZFB_MD_WASM_MANIFEST="$DC_MANIFEST" \
    ZFB_MD_WASM_VERIFY_NODE="$NODE_EXE" \
    ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 PUBLISH_RECHECK_DELAY=0 \
    bash "$SCRIPT" "$DC_MODE"
  ) >"$DC_OUTPUT" 2>&1 || DC_RC=$?
  DC_PUBLISH_RC=$DC_RC
  DC_UPLOAD_RC=0
  if [ "$DC_PUBLISH_RC" -eq 0 ] || [ "$DC_WANT" = mismatch ]; then
    DC_EXTRA_ARGS=
    if [ "$DC_WANT" = older ]; then DC_EXTRA_ARGS=--allow-missing-manifest; fi
    PATH="$MOCK_BIN:$PATH" MOCK_PACK_TARBALL="$DC_TARBALL" MOCK_GH_LOG="$DC_GH_LOG" \
      "$NODE_EXE" "$REPO_ROOT/scripts/verify-zfb-md-wasm-published-digests.mjs" --version "$V" \
      --manifest "$DC_MANIFEST" --tag "v$V" --upload $DC_EXTRA_ARGS >>"$DC_OUTPUT" 2>&1 || DC_UPLOAD_RC=$?
    if [ "$DC_RC" -eq 0 ]; then DC_RC=$DC_UPLOAD_RC; fi
  fi
  case "$DC_WANT" in
    upload)
      if [ "$DC_RC" -eq 0 ] && grep -q "^release upload v$V --repo .*/zfb-md-wasm-$V-shipped-artifacts.json$" "$DC_GH_LOG" && ! grep -q -- '--clobber' "$DC_GH_LOG"; then
        pass "$DC_DESC"
      else
        fail "$DC_DESC (exit=$DC_RC, gh=$(cat "$DC_GH_LOG"), output=$(tail -4 "$DC_OUTPUT" | tr '\n' ' '))"
      fi ;;
    mismatch)
      if [ "$DC_PUBLISH_RC" -ne 0 ] && [ "$DC_UPLOAD_RC" -ne 0 ] && ! grep -q '^release upload' "$DC_GH_LOG" && grep -q 'published bytes differ' "$DC_OUTPUT"; then
        pass "$DC_DESC"
      else
        fail "$DC_DESC (exit=$DC_RC, gh=$(cat "$DC_GH_LOG"), output=$(tail -4 "$DC_OUTPUT" | tr '\n' ' '))"
      fi ;;
    older)
      if [ "$DC_RC" -eq 0 ] && [ ! -s "$DC_GH_LOG" ] && grep -q 'Digests are NOT verified' "$DC_OUTPUT"; then
        pass "$DC_DESC"
      else
        fail "$DC_DESC (exit=$DC_RC, gh=$(cat "$DC_GH_LOG"), output=$(tail -4 "$DC_OUTPUT" | tr '\n' ' '))"
      fi ;;
  esac
  rm -f "$DC_GH_LOG" "$DC_PUB_LOG" "$DC_OUTPUT" "$DC_MARKER"
}

assert_digest_case 'digest: fresh publish matches and uploads' '' success "$DIGEST_FIXTURE/good.tgz" "$DIGEST_FIXTURE/manifest.json" upload
assert_digest_case 'digest: initial registry skip matches and uploads' "@takazudo/zfb-md-wasm@$V" success "$DIGEST_FIXTURE/good.tgz" "$DIGEST_FIXTURE/manifest.json" upload
assert_digest_case 'digest: publish conflict matches and uploads' '' conflict "$DIGEST_FIXTURE/good.tgz" "$DIGEST_FIXTURE/manifest.json" upload
assert_digest_case 'digest: lost ACK matches and uploads' '' lost "$DIGEST_FIXTURE/good.tgz" "$DIGEST_FIXTURE/manifest.json" upload
assert_digest_case 'digest: published mismatch blocks gh upload' '' success "$DIGEST_FIXTURE/bad.tgz" "$DIGEST_FIXTURE/manifest.json" mismatch
assert_digest_case 'digest: older tag without manifest annotates skip and has no upload' '' success "$DIGEST_FIXTURE/good.tgz" "$DIGEST_FIXTURE/absent.json" older

# Registry read-after-write lag (#4122): the verifier retries "not visible yet"
# and fails with a distinct message once its budget runs out.
assert_visibility_case() {
  VC_DESC=$1 VC_MISSES=$2 VC_ATTEMPTS=$3 VC_WANT=$4
  VC_MISSES_FILE=$(mktemp)
  VC_OUTPUT=$(mktemp)
  echo "$VC_MISSES" >"$VC_MISSES_FILE"
  VC_RC=0
  PATH="$MOCK_BIN:$PATH" MOCK_PACK_TARBALL="$DIGEST_FIXTURE/good.tgz" \
    MOCK_PACK_MISSES_FILE="$VC_MISSES_FILE" \
    ZFB_MD_WASM_VISIBILITY_ATTEMPTS="$VC_ATTEMPTS" ZFB_MD_WASM_VISIBILITY_DELAY_MS=0 \
    "$NODE_EXE" "$REPO_ROOT/scripts/verify-zfb-md-wasm-published-digests.mjs" --version "$V" \
    --manifest "$DIGEST_FIXTURE/manifest.json" >"$VC_OUTPUT" 2>&1 || VC_RC=$?
  case "$VC_WANT" in
    verified)
      if [ "$VC_RC" -eq 0 ] && grep -q 'Verified all four' "$VC_OUTPUT" \
        && [ "$(grep -c 'not visible on the registry yet' "$VC_OUTPUT")" -eq "$VC_MISSES" ]; then
        pass "$VC_DESC"
      else
        fail "$VC_DESC (exit=$VC_RC, output=$(tail -4 "$VC_OUTPUT" | tr '\n' ' '))"
      fi ;;
    invisible)
      if [ "$VC_RC" -ne 0 ] && grep -q 'never became visible' "$VC_OUTPUT" \
        && ! grep -q 'published bytes differ' "$VC_OUTPUT"; then
        pass "$VC_DESC"
      else
        fail "$VC_DESC (exit=$VC_RC, output=$(tail -4 "$VC_OUTPUT" | tr '\n' ' '))"
      fi ;;
  esac
  rm -f "$VC_MISSES_FILE" "$VC_OUTPUT"
}

assert_visibility_case 'digest: ETARGET right after publish is retried, then verified' 3 5 verified
assert_visibility_case 'digest: version never visible fails as registry lag, not mismatch' 9 3 invisible

# ── Per-package registry state: partial publish + resume (#4690) ─────────────
# A resumed release skips only present-verified versions, publishes absent
# ones, and stops before any further publish on present-unverified/uncertain.

# assert_state_run <desc> <want: ok|fail> <existing-specs> <unverified> <uncertain> <must-publish> <must-not-publish> [output-grep]
assert_state_run() {
  SR_DESC="$1" SR_WANT="$2" SR_EXISTING="$3" SR_UNVERIFIED="$4" SR_UNCERTAIN="$5" SR_MUST="$6" SR_MUST_NOT="$7" SR_GREP="${8:-}"
  SR_LOG=$(mktemp)
  SR_OUT=$(mktemp)
  SR_RC=0
  PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$SR_LOG" MOCK_EXISTING="$SR_EXISTING" \
    MOCK_STATE_UNVERIFIED="$SR_UNVERIFIED" MOCK_STATE_UNCERTAIN="$SR_UNCERTAIN" \
    ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 \
    ZFB_LOCAL_BUILT_PACKAGES="packages/zfb-darwin-arm64 packages/zfb-darwin-x64" \
    bash "$SCRIPT" mac-local >"$SR_OUT" 2>&1 || SR_RC=$?
  SR_OK=1
  if [ "$SR_WANT" = ok ] && [ "$SR_RC" -ne 0 ]; then SR_OK=0; fi
  if [ "$SR_WANT" = fail ] && [ "$SR_RC" -eq 0 ]; then SR_OK=0; fi
  PUB_LOG="$SR_LOG"
  for n in $SR_MUST; do log_has "$n" || SR_OK=0; done
  for n in $SR_MUST_NOT; do ! log_has "$n" || SR_OK=0; done
  if [ -n "$SR_GREP" ] && ! grep -Fq "$SR_GREP" "$SR_OUT"; then SR_OK=0; fi
  if [ "$SR_OK" -eq 1 ]; then
    pass "$SR_DESC"
  else
    fail "$SR_DESC (rc=$SR_RC, published: $(tr '\n' ' ' <"$SR_LOG"), out: $(tail -5 "$SR_OUT" | tr '\n' ' '))"
  fi
  rm -f "$SR_LOG" "$SR_OUT"
}

PRESENT_EXCEPT_RUNTIME="@takazudo/zfb-darwin-arm64@$V @takazudo/zfb-darwin-x64@$V @takazudo/zfb-linux-arm64-gnu@$V @takazudo/zfb-linux-x64-gnu@$V @takazudo/zfb-win32-x64-msvc@$V @takazudo/zfb-slugify@$V @takazudo/zfb@$V @takazudo/zfb-adapter-cloudflare@$V create-zfb@$V @takazudo/zfb-md-wasm@$V"

assert_state_run "state: resume skips present-verified and publishes only the absent package" ok \
  "$PRESENT_EXCEPT_RUNTIME" "" "" "@takazudo/zfb-runtime" "@takazudo/zfb @takazudo/zfb-darwin-x64 create-zfb"

assert_state_run "state: one present-unverified package stops the run before later publishes" fail \
  "$PRESENT_EXCEPT_RUNTIME" "@takazudo/zfb" "" "" "@takazudo/zfb-runtime create-zfb" \
  "@takazudo/zfb@$V registry state is present-unverified"

assert_state_run "state: an unverified Darwin carrier blocks the whole publish" fail \
  "@takazudo/zfb-darwin-x64@$V" "@takazudo/zfb-darwin-x64" "" "@takazudo/zfb-darwin-arm64" \
  "@takazudo/zfb-darwin-x64 @takazudo/zfb-linux-x64-gnu @takazudo/zfb" "registry state is present-unverified"

assert_state_run "state: registry uncertainty fails closed with nothing skipped or published past it" fail \
  "" "" "@takazudo/zfb-darwin-arm64" "" "@takazudo/zfb-darwin-arm64 @takazudo/zfb-darwin-x64 @takazudo/zfb" \
  "registry state is uncertain"

assert_state_run "state: fresh mac-local publish of all 11 with both Darwin carriers" ok \
  "" "" "" "@takazudo/zfb-darwin-arm64 @takazudo/zfb-darwin-x64 @takazudo/zfb create-zfb @takazudo/zfb-md-wasm" ""

# A missing or malformed source commit is refused before any registry call.
for SC_VALUE in "" "abc123" "0123456789ABCDEF0123456789ABCDEF01234567"; do
  SC_CALLS=$(mktemp)
  SC_RC=0
  PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG=/dev/null MOCK_EXISTING="" \
    MOCK_CALLS_LOG="$SC_CALLS" ZFB_RELEASE_SOURCE_COMMIT="$SC_VALUE" \
    bash "$SCRIPT" all-provenance >/dev/null 2>&1 || SC_RC=$?
  if [ "$SC_RC" -eq 2 ] && ! grep -q publish "$SC_CALLS"; then
    pass "state: source commit '$SC_VALUE' refused (exit 2) before any publish"
  else
    fail "state: source commit '$SC_VALUE' rc=$SC_RC calls=$(tr '\n' ' ' <"$SC_CALLS")"
  fi
  rm -f "$SC_CALLS"
done

# A lost ACK on the first publish followed by an unverified recheck fails and
# keeps npm's original error text.
LA_OUT=$(mktemp)
LA_RC=0
PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG=/dev/null MOCK_EXISTING="" \
  MOCK_MD_RESULT=lost MOCK_PUBLISHED_MARKER="$PUBLISH_TARGET/la-marker" MOCK_VERSION="$V" \
  MOCK_STATE_UNVERIFIED="@takazudo/zfb-md-wasm" PUBLISH_RECHECK_DELAY=0 \
  bash "$SCRIPT" all-provenance >"$LA_OUT" 2>&1 || LA_RC=$?
if [ "$LA_RC" -ne 0 ] && grep -Fq 'registry state after recheck: present-unverified' "$LA_OUT" \
   && grep -Fq '| ETIMEDOUT' "$LA_OUT"; then
  pass "state: lost ACK with an unverified registry version fails with the original error"
else
  fail "state: lost-ACK unverified rc=$LA_RC out=$(tail -6 "$LA_OUT" | tr '\n' ' ')"
fi
rm -f "$LA_OUT" "$PUBLISH_TARGET/la-marker"

# ── Publish re-inspects tarball content (#4690 NEW-m1) ───────────────────────
# A tampered Darwin binary inside a self-consistent pack manifest (integrity
# recomputed, as a compromised pack job could) is refused by the publish helper
# itself, before any npm call, using only the admission verdict.
TV_BASE=$(mktemp -d)
TV_OUT=$(mktemp)
TV_LOG=$(mktemp)
TV_CALLS=$(mktemp)
TV_BASE="$TV_BASE" FIXTURE="$REPO_ROOT/scripts/__tests__/helpers/release-tarballs-fixture.mjs" \
  "$NODE_EXE" --input-type=module <<'TV_FIXTURE'
const { makeTarballSet, replaceTarball, V } = await import(process.env.FIXTURE);
makeTarballSet({ base: process.env.TV_BASE });
replaceTarball(`${process.env.TV_BASE}/tarballs`, "@takazudo/zfb-darwin-x64", [
  { name: "package/package.json", data: Buffer.from(JSON.stringify({ name: "@takazudo/zfb-darwin-x64", version: V })), mode: 0o644 },
  { name: "package/zfb", data: Buffer.from("evil binary"), mode: 0o755 },
  { name: "package/zfb-css.node", data: Buffer.from("css @takazudo/zfb-darwin-x64"), mode: 0o644 },
]);
TV_FIXTURE
TV_RC=0
PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$TV_LOG" MOCK_CALLS_LOG="$TV_CALLS" MOCK_EXISTING="" \
  ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 ZFB_LOCAL_BUILT_PACKAGES="packages/zfb-darwin-arm64 packages/zfb-darwin-x64" \
  ZFB_PUBLISH_TARBALLS_DIR="$TV_BASE/tarballs" ZFB_RELEASE_VERSION=6.1.0 \
  ZFB_DARWIN_ADMISSION_VERDICT="$TV_BASE/verdict.json" ZFB_MD_WASM_BUILD_MANIFEST="$TV_BASE/build-shipped-artifacts.json" \
  ZFB_TARBALL_VERIFY_NODE="$NODE_EXE" \
  bash "$SCRIPT" mac-local >"$TV_OUT" 2>&1 || TV_RC=$?
if [ "$TV_RC" -ne 0 ] && [ ! -s "$TV_LOG" ] && ! grep -q '^npm publish' "$TV_CALLS" \
   && grep -Eq '@takazudo/zfb-darwin-x64: package/zfb sha256 [0-9a-f]+ != admitted' "$TV_OUT" \
   && grep -Fq 'tarball content re-verification failed; nothing was published' "$TV_OUT"; then
  pass "publish: a tampered Darwin binary in a self-consistent manifest is refused before any npm publish"
else
  fail "publish: tampered Darwin tarball rc=$TV_RC publishes=$(tr '\n' ' ' <"$TV_LOG") out=$(tail -4 "$TV_OUT" | tr '\n' ' ')"
fi
# The same set fails on a release-version mismatch before the verifier runs.
TV2_RC=0
PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_PUBLISH_LOG="$TV_LOG" MOCK_EXISTING="" \
  ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 ZFB_LOCAL_BUILT_PACKAGES="packages/zfb-darwin-arm64 packages/zfb-darwin-x64" \
  ZFB_PUBLISH_TARBALLS_DIR="$TV_BASE/tarballs" ZFB_RELEASE_VERSION=6.1.1 \
  bash "$SCRIPT" mac-local >"$TV_OUT" 2>&1 || TV2_RC=$?
if [ "$TV2_RC" -ne 0 ] && [ ! -s "$TV_LOG" ] && grep -Fq 'is not the release version 6.1.1' "$TV_OUT"; then
  pass "publish: a tarball set for another version is refused before any npm publish"
else
  fail "publish: version mismatch rc=$TV2_RC out=$(tail -3 "$TV_OUT" | tr '\n' ' ')"
fi
rm -rf "$TV_BASE" "$TV_OUT" "$TV_LOG" "$TV_CALLS"

# ── Summary ───────────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
