#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/file-exam-issue.sh — offline unit + integration tests for
# scripts/file-exam-issue.sh (the dedup tracking-issue filer for T3
# scheduled-exam workflows, issue #1342).
#
# Runs entirely offline: `scripts/file-exam-issue.sh` is bash; this harness is
# POSIX sh (health.yml + run-b4push.sh invoke `sh tests/unit/*.sh`), so the
# bash functions are exercised by sourcing the script inside `bash -c`
# (mirrors tests/unit/publish-npm-packages.sh). All `gh` calls are replaced by
# a mock executable placed first on PATH — no real network or GitHub auth
# involved. `jq` is the only real tool consulted (already a hard CI
# dependency elsewhere in this repo, e.g. release.yml's prepare-matrix job).
#
# Fixtures: the JSON shapes below were captured from REAL `gh` CLI output
# against this repo (`gh issue list --json number,title,labels,state` and
# `gh label list --json name,color`, both run read-only on 2026-07-03),
# reduced to the fields scripts/file-exam-issue.sh actually requests
# (number,title / name) — not hand-invented schemas.
#
# Requires: sh, bash, jq, mktemp, grep.
#
# Run:
#   sh tests/unit/file-exam-issue.sh

set -eu

# Run from the repo root regardless of the caller's cwd.
SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
cd "$REPO_ROOT"

SCRIPT="scripts/file-exam-issue.sh"

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

# assert_stdout <desc> <expected-stdout> <bash-snippet>
assert_stdout() {
  AS_DESC="$1"
  AS_WANT="$2"
  AS_SNIPPET="$3"
  AS_GOT=$(bash -c ". ./$SCRIPT; set +e; ${AS_SNIPPET}" 2>/dev/null || true)
  if [ "$AS_GOT" = "$AS_WANT" ]; then
    pass "$AS_DESC"
  else
    fail "$AS_DESC (want '$AS_WANT', got '$AS_GOT')"
  fi
}

# ── Mock `gh` on PATH ────────────────────────────────────────────────────────
#
# Real fixtures captured from `gh issue list --repo Takazudo/zudo-front-builder
# --state open --label sub --json number,title,labels,state --limit 3` and
# `gh label list --repo Takazudo/zudo-front-builder --json name,color --limit 5`
# (read-only, 2026-07-03), reduced to the fields this script actually requests.

MOCK_BIN=$(mktemp -d)
trap 'rm -rf "$MOCK_BIN"' EXIT

cat >"$MOCK_BIN/gh" <<'MOCK_GH'
#!/bin/sh
# Mock gh: label list/create, issue list/create/comment/close.
# Read paths (list) print canned JSON from MOCK_LABELS_JSON/MOCK_ISSUES_JSON.
# Mutating paths (create/comment/close) append the full argv to MOCK_GH_LOG.
LOG="${MOCK_GH_LOG:-/dev/null}"
# $* already includes the subcommand words ("label create ...", "issue
# comment 1400 ...", etc.) — log it as-is rather than prefixing again.
case "$1" in
  label)
    case "$2" in
      list) printf '%s' "$MOCK_LABELS_JSON" ;;
      create) printf '%s\n' "$*" >>"$LOG" ;;
    esac
    ;;
  issue)
    case "$2" in
      list) printf '%s' "$MOCK_ISSUES_JSON" ;;
      create) printf '%s\n' "$*" >>"$LOG" ;;
      comment) printf '%s\n' "$*" >>"$LOG" ;;
      close) printf '%s\n' "$*" >>"$LOG" ;;
    esac
    ;;
esac
exit 0
MOCK_GH
chmod +x "$MOCK_BIN/gh"

# Prepend the mock bin dir so `gh` resolves to the stub for the rest of this
# script; real jq/bash/etc. stay on PATH.
PATH="$MOCK_BIN:$PATH"
export PATH

# Real captured shape: `gh label list --json name,color` WITHOUT exam-failure.
LABELS_WITHOUT_EXAM='[{"color":"d73a4a","name":"bug"},{"color":"0075ca","name":"documentation"},{"color":"cfd3d7","name":"duplicate"},{"color":"a2eeef","name":"enhancement"},{"color":"7057ff","name":"good first issue"}]'
# Same shape, with exam-failure already present.
LABELS_WITH_EXAM='[{"color":"d73a4a","name":"bug"},{"color":"B60205","name":"exam-failure"}]'

# Real captured shape: `gh issue list --json number,title` — one entry whose
# title exactly matches issue_title("Drift net"), and an empty-list case.
ISSUES_WITH_MATCH='[{"number":1400,"title":"[exam-failure] Drift net is failing"}]'
ISSUES_NO_MATCH='[{"number":1349,"title":"[Test & CD Quality][Sub] Some unrelated issue"}]'
ISSUES_EMPTY='[]'

# ── issue_title — deterministic per-workflow title ──────────────────────────

assert_stdout 'issue_title: deterministic title format' \
  '[exam-failure] Drift net is failing' \
  'issue_title "Drift net"'

# ── ensure_label ─────────────────────────────────────────────────────────────

GH_LOG=$(mktemp)
: >"$GH_LOG"
assert_exit 'ensure_label: label already present → no gh label create call' 0 \
  "MOCK_LABELS_JSON='$LABELS_WITH_EXAM' MOCK_GH_LOG='$GH_LOG' ensure_label owner/repo"
if [ -s "$GH_LOG" ]; then
  fail "ensure_label: label already present should not call gh label create ($(cat "$GH_LOG"))"
else
  pass "ensure_label: label already present made no mutating call"
fi

GH_LOG=$(mktemp)
: >"$GH_LOG"
assert_exit 'ensure_label: label absent → gh label create called' 0 \
  "MOCK_LABELS_JSON='$LABELS_WITHOUT_EXAM' MOCK_GH_LOG='$GH_LOG' ensure_label owner/repo"
if grep -q '^label create.*exam-failure' "$GH_LOG"; then
  pass "ensure_label: label absent created 'exam-failure'"
else
  fail "ensure_label: expected 'gh label create ... exam-failure', got: $(cat "$GH_LOG")"
fi

GH_LOG=$(mktemp)
: >"$GH_LOG"
assert_exit 'ensure_label: DRY_RUN=1 + label absent → no gh label create call' 0 \
  "DRY_RUN=1 MOCK_LABELS_JSON='$LABELS_WITHOUT_EXAM' MOCK_GH_LOG='$GH_LOG' ensure_label owner/repo"
if [ -s "$GH_LOG" ]; then
  fail "ensure_label: DRY_RUN=1 should skip gh label create ($(cat "$GH_LOG"))"
else
  pass "ensure_label: DRY_RUN=1 made no mutating call"
fi

# ── find_existing_issue ──────────────────────────────────────────────────────

assert_stdout 'find_existing_issue: exact title match → returns issue number' \
  '1400' \
  "MOCK_ISSUES_JSON='$ISSUES_WITH_MATCH' find_existing_issue owner/repo '[exam-failure] Drift net is failing'"

assert_stdout 'find_existing_issue: no title match → empty' \
  '' \
  "MOCK_ISSUES_JSON='$ISSUES_NO_MATCH' find_existing_issue owner/repo '[exam-failure] Drift net is failing'"

assert_stdout 'find_existing_issue: empty issue list → empty' \
  '' \
  "MOCK_ISSUES_JSON='$ISSUES_EMPTY' find_existing_issue owner/repo '[exam-failure] Drift net is failing'"

# ── file_failure ──────────────────────────────────────────────────────────────

GH_LOG=$(mktemp)
: >"$GH_LOG"
assert_exit 'file_failure: no existing issue → gh issue create called' 0 \
  "MOCK_LABELS_JSON='$LABELS_WITH_EXAM' MOCK_ISSUES_JSON='$ISSUES_EMPTY' MOCK_GH_LOG='$GH_LOG' RUN_URL='https://example.test/run/1' LEG='ubuntu-latest' file_failure owner/repo 'Drift net'"
if grep -q '^issue create.*--title \[exam-failure\] Drift net is failing' "$GH_LOG"; then
  pass "file_failure: created issue with the deterministic title"
else
  fail "file_failure: expected 'gh issue create --title [exam-failure] Drift net is failing ...', got: $(cat "$GH_LOG")"
fi
if grep -q 'issue comment' "$GH_LOG"; then
  fail "file_failure: should not have also called gh issue comment"
else
  pass "file_failure: did not call gh issue comment when creating"
fi

GH_LOG=$(mktemp)
: >"$GH_LOG"
assert_exit 'file_failure: existing open issue → gh issue comment on that number' 0 \
  "MOCK_LABELS_JSON='$LABELS_WITH_EXAM' MOCK_ISSUES_JSON='$ISSUES_WITH_MATCH' MOCK_GH_LOG='$GH_LOG' RUN_URL='https://example.test/run/2' LEG='macos-15' file_failure owner/repo 'Drift net'"
if grep -q '^issue comment 1400' "$GH_LOG"; then
  pass "file_failure: commented on existing issue #1400"
else
  fail "file_failure: expected 'gh issue comment 1400 ...', got: $(cat "$GH_LOG")"
fi
if grep -q 'issue create' "$GH_LOG"; then
  fail "file_failure: should not have also called gh issue create"
else
  pass "file_failure: did not call gh issue create when one already exists"
fi

GH_LOG=$(mktemp)
: >"$GH_LOG"
assert_exit 'file_failure: DRY_RUN=1 → no mutating gh calls' 0 \
  "DRY_RUN=1 MOCK_LABELS_JSON='$LABELS_WITHOUT_EXAM' MOCK_ISSUES_JSON='$ISSUES_EMPTY' MOCK_GH_LOG='$GH_LOG' file_failure owner/repo 'Drift net'"
if [ -s "$GH_LOG" ]; then
  fail "file_failure: DRY_RUN=1 should make no gh mutating calls ($(cat "$GH_LOG"))"
else
  pass "file_failure: DRY_RUN=1 made no mutating calls (label create AND issue create both skipped)"
fi

# ── close_on_green ────────────────────────────────────────────────────────────

GH_LOG=$(mktemp)
: >"$GH_LOG"
assert_exit 'close_on_green: existing open issue → gh issue close on that number' 0 \
  "MOCK_ISSUES_JSON='$ISSUES_WITH_MATCH' MOCK_GH_LOG='$GH_LOG' RUN_URL='https://example.test/run/3' close_on_green owner/repo 'Drift net'"
if grep -q '^issue close 1400' "$GH_LOG"; then
  pass "close_on_green: closed issue #1400"
else
  fail "close_on_green: expected 'gh issue close 1400 ...', got: $(cat "$GH_LOG")"
fi

GH_LOG=$(mktemp)
: >"$GH_LOG"
assert_exit 'close_on_green: no existing issue → no-op, exit 0' 0 \
  "MOCK_ISSUES_JSON='$ISSUES_EMPTY' MOCK_GH_LOG='$GH_LOG' close_on_green owner/repo 'Drift net'"
if [ -s "$GH_LOG" ]; then
  fail "close_on_green: no existing issue should make no gh call ($(cat "$GH_LOG"))"
else
  pass "close_on_green: no existing issue made no gh call"
fi

GH_LOG=$(mktemp)
: >"$GH_LOG"
assert_exit 'close_on_green: DRY_RUN=1 → no gh issue close call' 0 \
  "DRY_RUN=1 MOCK_ISSUES_JSON='$ISSUES_WITH_MATCH' MOCK_GH_LOG='$GH_LOG' close_on_green owner/repo 'Drift net'"
if [ -s "$GH_LOG" ]; then
  fail "close_on_green: DRY_RUN=1 should skip gh issue close ($(cat "$GH_LOG"))"
else
  pass "close_on_green: DRY_RUN=1 made no gh call"
fi

# ── main() argument / env validation ─────────────────────────────────────────

assert_exit 'main: missing workflow-name argument → exit 2' 2 \
  'REPO=owner/repo main'

assert_exit 'main: unrecognized second argument → exit 2' 2 \
  'REPO=owner/repo main "Drift net" --bogus'

assert_exit 'main: missing REPO and GITHUB_REPOSITORY → exit 2' 2 \
  'unset REPO GITHUB_REPOSITORY 2>/dev/null; main "Drift net"'

# ── Integration: whole-script runs with mock gh on PATH ─────────────────────

GH_LOG=$(mktemp)
: >"$GH_LOG"
if REPO=owner/repo \
   MOCK_LABELS_JSON="$LABELS_WITH_EXAM" \
   MOCK_ISSUES_JSON="$ISSUES_EMPTY" \
   MOCK_GH_LOG="$GH_LOG" \
   RUN_URL='https://example.test/run/4' \
   LEG='ubuntu-latest' \
   bash "$SCRIPT" "Drift net" >/dev/null 2>&1; then
  if grep -q '^issue create.*exam-failure' "$GH_LOG"; then
    pass "integration: failure mode end-to-end creates the tracking issue"
  else
    fail "integration: failure mode did not create the tracking issue: $(cat "$GH_LOG")"
  fi
else
  fail "integration: failure mode invocation exited non-zero"
fi

GH_LOG=$(mktemp)
: >"$GH_LOG"
if REPO=owner/repo \
   MOCK_ISSUES_JSON="$ISSUES_WITH_MATCH" \
   MOCK_GH_LOG="$GH_LOG" \
   RUN_URL='https://example.test/run/5' \
   bash "$SCRIPT" "Drift net" --green >/dev/null 2>&1; then
  if grep -q '^issue close 1400' "$GH_LOG"; then
    pass "integration: --green mode end-to-end closes the tracking issue"
  else
    fail "integration: --green mode did not close the tracking issue: $(cat "$GH_LOG")"
  fi
else
  fail "integration: --green mode invocation exited non-zero"
fi

# ── Summary ───────────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
