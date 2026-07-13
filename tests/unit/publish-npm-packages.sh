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
trap 'rm -rf "$MOCK_BIN" "${PUB_LOG:-}" "${DISTTAG_LOG:-}"' EXIT

cat >"$MOCK_BIN/npm" <<'MOCK_NPM'
#!/bin/sh
# Mock npm: `view <name@version> version`, `publish` (cwd package), `dist-tag add`.
case "$1" in
  view)
    spec="$2"
    for e in $MOCK_EXISTING; do
      if [ "$e" = "$spec" ]; then
        printf '%s\n' "${spec##*@}"   # version part (after the last @)
        exit 0
      fi
    done
    exit 1   # not found — npm view exits non-zero
    ;;
  publish)
    name=$(node -p "require('./package.json').name")
    version=$(node -p "require('./package.json').version")
    printf '%s@%s\n' "$name" "$version" >>"$MOCK_PUBLISH_LOG"
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
fi
exit 0
MOCK_PNPM

chmod +x "$MOCK_BIN/npm" "$MOCK_BIN/pnpm"

V=$(node -p "require('./packages/zfb/package.json').version")
ALL_SPECS="@takazudo/zfb-darwin-arm64@$V @takazudo/zfb-darwin-x64@$V @takazudo/zfb-linux-arm64-gnu@$V @takazudo/zfb-linux-x64-gnu@$V @takazudo/zfb-win32-x64-msvc@$V @takazudo/zfb@$V @takazudo/zfb-runtime@$V @takazudo/zfb-adapter-cloudflare@$V create-zfb@$V @takazudo/zfb-md-wasm@$V"

# log_has <name> — true iff the publish log has a line for exactly <name>
# (matching <name> at line start followed by '@' or end-of-line, so
# "@takazudo/zfb" does not match "@takazudo/zfb-runtime").
log_has() {
  grep -Eq "^$1(@|$)" "$PUB_LOG"
}

# Case 1 — idempotent re-run: 8 packages already published (incl.
# @takazudo/zfb-md-wasm), only @takazudo/zfb-runtime + create-zfb missing.
# Expect exit 0 and ONLY the two missing ones published.
PUB_LOG=$(mktemp)
: >"$PUB_LOG"
if PATH="$MOCK_BIN:$PATH" \
   DIST_TAG=next \
   MOCK_PUBLISH_LOG="$PUB_LOG" \
   MOCK_EXISTING="@takazudo/zfb-darwin-arm64@$V @takazudo/zfb-darwin-x64@$V @takazudo/zfb-linux-arm64-gnu@$V @takazudo/zfb-linux-x64-gnu@$V @takazudo/zfb-win32-x64-msvc@$V @takazudo/zfb@$V @takazudo/zfb-adapter-cloudflare@$V @takazudo/zfb-md-wasm@$V" \
   bash "$SCRIPT" all-provenance >/dev/null 2>&1; then
  if log_has "@takazudo/zfb-runtime" && log_has "create-zfb" \
     && ! log_has "@takazudo/zfb" && ! log_has "@takazudo/zfb-adapter-cloudflare" \
     && ! log_has "@takazudo/zfb-darwin-x64" && ! log_has "@takazudo/zfb-md-wasm"; then
    pass "integration: idempotent re-run publishes ONLY the 2 missing packages"
  else
    fail "integration: idempotent re-run published the wrong set: $(tr '\n' ' ' <"$PUB_LOG")"
  fi
else
  fail "integration: idempotent re-run exited non-zero"
fi

# Case 2 — fresh publish: nothing on the registry → all 10 packages published
# (5 platform via npm publish, 5 non-platform via pnpm publish — the 5th
# non-platform package being @takazudo/zfb-md-wasm, zfb#1579).
PUB_LOG=$(mktemp)
: >"$PUB_LOG"
if PATH="$MOCK_BIN:$PATH" \
   DIST_TAG=next \
   MOCK_PUBLISH_LOG="$PUB_LOG" \
   MOCK_EXISTING="" \
   bash "$SCRIPT" all-provenance >/dev/null 2>&1; then
  COUNT=$(grep -c . "$PUB_LOG" || true)
  if [ "$COUNT" -eq 10 ] && log_has "@takazudo/zfb-darwin-x64" && log_has "create-zfb" && log_has "@takazudo/zfb-runtime" && log_has "@takazudo/zfb-md-wasm"; then
    pass "integration: fresh registry publishes all 10 packages"
  else
    fail "integration: fresh publish count=$COUNT (want 10): $(tr '\n' ' ' <"$PUB_LOG")"
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

# ── Summary ───────────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
