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

SCRIPT="scripts/publish-npm-packages.sh"
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
  if bash -c ". ./$SCRIPT; set +e; ${AE_SNIPPET}" >/dev/null 2>&1; then
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

# ── should_skip_publish — skip iff already on the registry ─────────────────────

assert_exit 'should_skip_publish: version present → skip (0)' 0 \
  'npm() { echo "1.2.3"; }; should_skip_publish foo 1.2.3'

assert_exit 'should_skip_publish: version absent → do not skip (1)' 1 \
  'npm() { return 1; }; should_skip_publish foo 1.2.3'

# ── classify_publish_result ───────────────────────────────────────────────────

assert_exit 'classify: exit 0 → success (0)' 0 \
  'classify_publish_result foo 1.2.3 0 /dev/null'

assert_exit 'classify: conflict text → tolerate (0)' 0 \
  'f=$(mktemp); printf "%s" "remote: You cannot publish over the previously published versions: 1.2.3" >"$f"; classify_publish_result foo 1.2.3 1 "$f"; r=$?; rm -f "$f"; exit $r'

assert_exit 'classify: non-conflict error BUT version landed (lost-ACK) → tolerate (0)' 0 \
  'f=$(mktemp); printf "%s" "socket hang up ECONNRESET" >"$f"; npm() { echo "1.2.3"; }; PUBLISH_RECHECK_DELAY=0 classify_publish_result foo 1.2.3 1 "$f"; r=$?; rm -f "$f"; exit $r'

assert_exit 'classify: real error AND version NOT on registry → fail (1)' 1 \
  'f=$(mktemp); printf "%s" "403 You do not have permission to publish" >"$f"; npm() { return 1; }; PUBLISH_RECHECK_DELAY=0 PUBLISH_RECHECK_ATTEMPTS=2 classify_publish_result foo 1.2.3 1 "$f"; r=$?; rm -f "$f"; exit $r'

# ── Integration: whole-script runs with mock npm/pnpm on PATH ─────────────────

# Build a throwaway PATH dir holding mock `npm` and `pnpm` executables.
MOCK_BIN=$(mktemp -d)
trap 'rm -rf "$MOCK_BIN" "${PUB_LOG:-}" "${DISTTAG_LOG:-}" "${ARGS_LOG:-}" "${DIGEST_FIXTURE:-}" "${DC_GH_LOG:-}" "${DC_PUB_LOG:-}" "${DC_OUTPUT:-}" "${DC_MARKER:-}" "${LEGACY_TARGET:-}" "${LEGACY_OUTPUT:-}" "${LEGACY_PUB_LOG:-}"' EXIT HUP INT TERM

cat >"$MOCK_BIN/npm" <<'MOCK_NPM'
#!/bin/sh
# Mock npm: `view <name@version> version`, `publish` (cwd package), `dist-tag add`.
case "$1" in
  view)
    spec="$2"
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
    destination="$4"
    cp "$MOCK_PACK_TARBALL" "$destination/published.tgz"
    printf 'published.tgz\n'
    ;;
  publish)
    name=$(node -p "require('./package.json').name")
    version=$(node -p "require('./package.json').version")
    printf '%s@%s\n' "$name" "$version" >>"$MOCK_PUBLISH_LOG"
    printf 'npm %s\n' "$*" >>"${MOCK_ARGS_LOG:-/dev/null}"
    exit 0
    ;;
  dist-tag)
    printf 'dist-tag %s\n' "$*" >>"${MOCK_DISTTAG_LOG:-/dev/null}"
    exit 0
    ;;
  *) exit 0 ;;
esac
MOCK_NPM

cat >"$MOCK_BIN/pnpm" <<'MOCK_PNPM'
#!/bin/sh
# Mock pnpm: record the --filter target of a `publish` invocation.
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
  if [ "$filter" = "@takazudo/zfb-md-wasm" ]; then
    case "${MOCK_MD_RESULT:-success}" in
      conflict) echo 'EPUBLISHCONFLICT' >&2; exit 1 ;;
      lost) touch "$MOCK_PUBLISHED_MARKER"; echo 'ETIMEDOUT' >&2; exit 1 ;;
    esac
  fi
fi
exit 0
MOCK_PNPM

chmod +x "$MOCK_BIN/npm" "$MOCK_BIN/pnpm"

cat >"$MOCK_BIN/gh" <<'MOCK_GH'
#!/bin/sh
printf '%s\n' "$*" >>"$MOCK_GH_LOG"
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

V=$(node -p "require('./packages/zfb/package.json').version")
ALL_SPECS="@takazudo/zfb-darwin-arm64@$V @takazudo/zfb-darwin-x64@$V @takazudo/zfb-linux-arm64-gnu@$V @takazudo/zfb-linux-x64-gnu@$V @takazudo/zfb-win32-x64-msvc@$V @takazudo/zfb-slugify@$V @takazudo/zfb@$V @takazudo/zfb-runtime@$V @takazudo/zfb-adapter-cloudflare@$V create-zfb@$V @takazudo/zfb-md-wasm@$V"

# log_has <name> — true iff the publish log has a line for exactly <name>
# (matching <name> at line start followed by '@' or end-of-line, so
# "@takazudo/zfb" does not match "@takazudo/zfb-runtime").
log_has() {
  grep -Eq "^$1(@|$)" "$PUB_LOG"
}

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

# Case 3 — dual-tag wiring: everything already published (all skipped) AND
# ALSO_LATEST=1 → the script still reaches maybe_advance_latest, which invokes
# scripts/advance-latest-dist-tag.sh (→ mock `npm dist-tag add`). Expect exit 0
# and a non-empty dist-tag log.
PUB_LOG=$(mktemp)
DISTTAG_LOG=$(mktemp)
: >"$PUB_LOG"
: >"$DISTTAG_LOG"
if PATH="$MOCK_BIN:$PATH" \
   DIST_TAG=next \
   ALSO_LATEST=1 \
   MOCK_PUBLISH_LOG="$PUB_LOG" \
   MOCK_DISTTAG_LOG="$DISTTAG_LOG" \
   MOCK_EXISTING="$ALL_SPECS" \
   bash "$SCRIPT" all-provenance >/dev/null 2>&1; then
  if [ ! -s "$PUB_LOG" ] && [ -s "$DISTTAG_LOG" ]; then
    pass "integration: ALSO_LATEST=1 advances 'latest' even when all publishes are skipped"
  else
    fail "integration: dual-tag wiring (publog empty? $([ -s "$PUB_LOG" ] && echo no || echo yes); disttag ran? $([ -s "$DISTTAG_LOG" ] && echo yes || echo no))"
  fi
else
  fail "integration: dual-tag run exited non-zero"
fi

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
  if [ "$COUNT" -eq 11 ] && ! grep -q -- '--provenance' "$ARGS_LOG"; then
    pass "integration: recovery publishes all 11 packages without --provenance"
  else
    fail "integration: recovery count=$COUNT (want 11) or found --provenance: $(tr '\n' ' ' <"$ARGS_LOG")"
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
if (
  cd "$LEGACY_TARGET"
  PATH="$MOCK_BIN:$PATH" \
    DIST_TAG=latest \
    MOCK_PUBLISH_LOG="$LEGACY_PUB_LOG" \
    MOCK_EXISTING="" \
    ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 \
    ZFB_MD_WASM_VERIFY_NODE="$MOCK_BIN/verify-noop" \
    bash "$REPO_ROOT/$SCRIPT" recovery-no-provenance
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
      bash "$REPO_ROOT/$SCRIPT" "$LEGACY_MODE"
  ) >"$LEGACY_OUTPUT" 2>&1 || LEGACY_RC=$?

  if [ "$LEGACY_RC" -ne 0 ] && [ ! -s "$LEGACY_PUB_LOG" ] \
     && grep -Fq 'publish target is missing expected package manifest: packages/zfb-slugify/package.json' "$LEGACY_OUTPUT"; then
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
  PATH="$MOCK_BIN:$PATH" DIST_TAG=latest MOCK_EXISTING="$DC_EXISTING" \
    MOCK_MD_RESULT="$DC_RESULT" MOCK_PACK_TARBALL="$DC_TARBALL" \
    MOCK_PUBLISH_LOG="$DC_PUB_LOG" MOCK_GH_LOG="$DC_GH_LOG" \
    MOCK_PUBLISHED_MARKER="$DC_MARKER" ZFB_MD_WASM_MANIFEST="$DC_MANIFEST" \
    ZFB_MD_WASM_VERIFY_NODE="$NODE_EXE" \
    ZFB_ALLOW_PROVENANCE_DOWNGRADE=1 PUBLISH_RECHECK_DELAY=0 \
    bash "$SCRIPT" "$DC_MODE" >"$DC_OUTPUT" 2>&1 || DC_RC=$?
  DC_PUBLISH_RC=$DC_RC
  DC_UPLOAD_RC=0
  if [ "$DC_PUBLISH_RC" -eq 0 ] || [ "$DC_WANT" = mismatch ]; then
    DC_EXTRA_ARGS=
    if [ "$DC_WANT" = older ]; then DC_EXTRA_ARGS=--allow-missing-manifest; fi
    PATH="$MOCK_BIN:$PATH" MOCK_PACK_TARBALL="$DC_TARBALL" MOCK_GH_LOG="$DC_GH_LOG" \
      "$NODE_EXE" scripts/verify-zfb-md-wasm-published-digests.mjs --version "$V" \
      --manifest "$DC_MANIFEST" --tag "v$V" --upload $DC_EXTRA_ARGS >>"$DC_OUTPUT" 2>&1 || DC_UPLOAD_RC=$?
    if [ "$DC_RC" -eq 0 ]; then DC_RC=$DC_UPLOAD_RC; fi
  fi
  case "$DC_WANT" in
    upload)
      if [ "$DC_RC" -eq 0 ] && grep -q "^release upload v$V .* --clobber$" "$DC_GH_LOG"; then
        pass "$DC_DESC"
      else
        fail "$DC_DESC (exit=$DC_RC, gh=$(cat "$DC_GH_LOG"), output=$(tail -4 "$DC_OUTPUT" | tr '\n' ' '))"
      fi ;;
    mismatch)
      if [ "$DC_PUBLISH_RC" -ne 0 ] && [ "$DC_UPLOAD_RC" -ne 0 ] && [ ! -s "$DC_GH_LOG" ] && grep -q 'published bytes differ' "$DC_OUTPUT"; then
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

# ── Summary ───────────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
