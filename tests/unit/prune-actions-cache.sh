#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/prune-actions-cache.sh — offline fixture test for the guarded
# Actions-cache prune script (issue #2686).
#
# The fixture uses the paginated/slurped shape emitted by
# `gh api repos/owner/repo/actions/caches --paginate --slurp`.  The two
# 2,000,000,000-byte entries deliberately share one key while differing by
# ref: main must be PROTECTED and the merged pull ref must be PRUNE.  A fake
# gh executable supplies the cache page and PR states and records every
# attempted DELETE, so the bare invocation proves that dry-run is read-only.
#
# Run:
#   sh tests/unit/prune-actions-cache.sh

set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
cd "$REPO_ROOT"

SCRIPT="scripts/prune-actions-cache.sh"
FIXTURE="tests/fixtures/prune-actions-cache.json"

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

if [ ! -x "$SCRIPT" ]; then
  fail "script missing or not executable: $SCRIPT"
fi
if [ ! -f "$FIXTURE" ]; then
  fail "fixture missing: $FIXTURE"
fi
if [ "$FAIL" -gt 0 ]; then
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

MOCK_BIN=$(mktemp -d)
MOCK_LOG=$(mktemp)
trap 'rm -rf "$MOCK_BIN" "$MOCK_LOG"; rm -f "${BROKEN_FIXTURE:-}"' EXIT

cat >"$MOCK_BIN/gh" <<'MOCK_GH'
#!/bin/sh
set -eu

LOG=${MOCK_GH_LOG:?}

case "${1:-}" in
  api)
    case "$*" in
      *actions/caches*)
        case "$*" in
          *DELETE*)
            printf '%s\n' "$*" >>"$LOG"
            ;;
          *)
            cat "$MOCK_CACHE_FIXTURE"
            ;;
        esac
        ;;
      *)
        printf 'unexpected gh api invocation: %s\n' "$*" >&2
        exit 1
        ;;
    esac
    ;;
  pr)
    if [ "${2:-}" != view ]; then
      printf 'unexpected gh pr invocation: %s\n' "$*" >&2
      exit 1
    fi
    case "${3:-}" in
      123) printf 'MERGED\n' ;;
      456) printf 'OPEN\n' ;;
      *)
        printf 'unknown fixture PR: %s\n' "${3:-<missing>}" >&2
        exit 1
        ;;
    esac
    ;;
  *)
    printf 'unexpected gh invocation: %s\n' "$*" >&2
    exit 1
    ;;
esac
MOCK_GH
chmod +x "$MOCK_BIN/gh"

run_dry_run() {
  MOCK_CACHE_FIXTURE="$REPO_ROOT/$FIXTURE" MOCK_GH_LOG="$MOCK_LOG" \
    REPO=owner/repo PATH="$MOCK_BIN:$PATH" "$REPO_ROOT/$SCRIPT" 2>&1
}

OUTPUT=$(run_dry_run)

if printf '%s\n' "$OUTPUT" | grep -Eq 'PROTECTED[[:space:]]+1001[[:space:]]+refs/heads/main[[:space:]]+v2-zfb-workspace-all-targets-Linux-x64-identical-key'; then
    pass "identical-key main cache is classified PROTECTED by ref"
else
    fail "expected cache id 1001 on refs/heads/main to be PROTECTED"
fi

if printf '%s\n' "$OUTPUT" | grep -Eq 'PRUNE[[:space:]]+1002[[:space:]]+refs/pull/123/merge[[:space:]]+v2-zfb-workspace-all-targets-Linux-x64-identical-key'; then
    pass "identical-key merged-PR cache is classified PRUNE by ref and PR state"
else
    fail "expected cache id 1002 on merged refs/pull/123/merge to be PRUNE"
fi

if printf '%s\n' "$OUTPUT" | grep -Eq 'SKIP[[:space:]]+1003[[:space:]]+refs/pull/456/merge.*PR #456 is OPEN'; then
    pass "open-PR cache is explicitly SKIP"
else
    fail "expected open PR #456 cache to be explicitly SKIP"
fi

case "$OUTPUT" in
  *"Total before: 4700000000 bytes"*"Total after (dry-run projection): 2700000000 bytes"*)
    pass "dry-run prints before and projected-after totals"
    ;;
  *)
    fail "expected before=4700000000 and projected after=2700000000 totals"
    ;;
esac

if [ ! -s "$MOCK_LOG" ]; then
  pass "bare invocation performs no DELETE request"
else
  fail "bare invocation attempted a mutation: $(cat "$MOCK_LOG")"
fi

# A custom protect pattern must protect the matching ref while the mandatory
# main protection remains in force independently of that pattern.
: >"$MOCK_LOG"
OUTPUT_CUSTOM=$(MOCK_CACHE_FIXTURE="$REPO_ROOT/$FIXTURE" MOCK_GH_LOG="$MOCK_LOG" \
  REPO=owner/repo PATH="$MOCK_BIN:$PATH" "$REPO_ROOT/$SCRIPT" \
  --protect-pattern '^refs/heads/(main|release)$' 2>&1)

if printf '%s\n' "$OUTPUT_CUSTOM" | grep -Eq 'PROTECTED[[:space:]]+1004[[:space:]]+refs/heads/release.*matched protect pattern'; then
    pass "custom protect pattern produces an explicit PROTECTED verdict"
else
    fail "expected refs/heads/release cache to match custom protect pattern"
fi

# An unavailable/unknown PR state must fail closed and explain the SKIP rather
# than silently treating the ref as stale.  The fixture is extended only in a
# temporary file so the captured identical-key fixture remains unchanged.
BROKEN_FIXTURE=$(mktemp)
jq '. [0].actions_caches += [{"id": 1005, "ref": "refs/pull/999/merge", "key": "unknown-pr-cache", "size_in_bytes": 50000000}]' \
  "$FIXTURE" >"$BROKEN_FIXTURE"
set +e
OUTPUT_BROKEN=$(MOCK_CACHE_FIXTURE="$BROKEN_FIXTURE" MOCK_GH_LOG="$MOCK_LOG" \
  REPO=owner/repo PATH="$MOCK_BIN:$PATH" "$REPO_ROOT/$SCRIPT" 2>&1)
RC_BROKEN=$?
set -e

if [ "$RC_BROKEN" -ne 0 ] && printf '%s\n' "$OUTPUT_BROKEN" | grep -q 'PR #999 state query failed'; then
  pass "unknown PR state fails closed with an explicit state-query error"
else
  fail "expected unknown PR state to fail closed with a visible error (rc=$RC_BROKEN)"
fi

if [ ! -s "$MOCK_LOG" ]; then
  pass "classification failure prevents all deletion attempts"
else
  fail "classification failure attempted a mutation: $(cat "$MOCK_LOG")"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
