#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/smoke-packed-clean-room-dist-out.sh — offline tests for the
# opt-in ZFB_SMOKE_DIST_OUT export added to
# scripts/smoke-packed-clean-room.sh (issue #2280), which lets the
# create-zfb showcase deploy reuse the exact dist/ this required check
# already builds instead of scaffolding+building it a second time.
#
# Never runs scripts/smoke-packed-clean-room.sh end-to-end (that needs a
# freshly built Rust zfb binary and is far too heavy for a unit test). Two
# things are checked instead:
#   - static grep/awk pins on the script text: the guard uses the `:-` form
#     (required under `set -euo pipefail`, or the script aborts with
#     "unbound variable" on every PR that backs this required check), the
#     export block is the LAST block in the file, and nothing above Stage 6
#     mentions ZFB_SMOKE_DIST_OUT (the opt-in must not leak into any
#     assertion stage above it);
#   - a positive copy test that extracts the REAL export block's source text
#     out of the script (not a reimplementation) and executes it via `bash`
#     against a throwaway fixture "dist/", mirroring
#     tests/unit/release-binary-slot-restore.sh's source-the-real-code
#     technique.
#
# Requires: sh, bash, awk, mktemp, diff, grep. No network.
#
# Run:
#   sh tests/unit/smoke-packed-clean-room-dist-out.sh

set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)

SCRIPT="$REPO_ROOT/scripts/smoke-packed-clean-room.sh"

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

if [ ! -f "$SCRIPT" ]; then
  fail "script not found: $SCRIPT"
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

# ── Assertion 1: the export block is the FINAL block of the script ─────────
# Compares the line number of the block's closing `fi` against the line
# number of the last non-blank, non-comment line in the whole file. If
# anything meaningful were appended after the export block, the two would
# diverge.

LAST_CONTENT_LINE=$(awk '
  {
    t = $0
    gsub(/^[ \t]+/, "", t)
    if (t != "" && substr(t, 1, 1) != "#") last = NR
  }
  END { print last + 0 }
' "$SCRIPT")

BLOCK_END_LINE=$(awk '
  /ZFB_SMOKE_DIST_OUT:-/ { grab = 1 }
  grab && $0 ~ /^fi$/ { print NR; exit }
' "$SCRIPT")

if [ -n "$BLOCK_END_LINE" ] && [ "$LAST_CONTENT_LINE" = "$BLOCK_END_LINE" ]; then
  pass "the ZFB_SMOKE_DIST_OUT export block is the final block of the script (closing 'fi' at line $BLOCK_END_LINE = last content line)"
else
  fail "the ZFB_SMOKE_DIST_OUT export block is NOT the final block (block ends at line ${BLOCK_END_LINE:-<not found>}, last content line is $LAST_CONTENT_LINE)"
fi

# ── Assertion 2: guarded by the `:-` form ───────────────────────────────────
# A bare $ZFB_SMOKE_DIST_OUT reference would abort the script with "unbound
# variable" under `set -euo pipefail` when the var is unset — and this
# script backs the required "Scaffold E2E" status check.

# Checked on CODE ONLY (comments stripped): a `grep` over the whole file
# would be satisfied by a comment that merely mentions the `:-` form, so it
# would keep passing the moment someone added a bare reference next to the
# guarded one. Assertion 5 below then EXECUTES the unset path, which is the
# property that actually matters.

SCRIPT_CODE=$(sed 's/[[:space:]]*#.*$//' "$SCRIPT")

if printf '%s\n' "$SCRIPT_CODE" | grep -q 'ZFB_SMOKE_DIST_OUT:-'; then
  pass "the export guard uses the \${ZFB_SMOKE_DIST_OUT:-} form (in code, not just a comment)"
else
  fail "no ZFB_SMOKE_DIST_OUT:- guard found in the code of $SCRIPT — a bare reference would abort under set -u"
fi

# Deliberately NOT asserting "no bare $ZFB_SMOKE_DIST_OUT anywhere": the
# uses inside the `if [[ -n "${VAR:-}" ]]` body are bare on purpose and are
# correct, because that branch only runs when the variable is set. What
# matters is the unset path, and assertion 5 below executes it.

# ── Assertion 3: no line above Stage 6 mentions ZFB_SMOKE_DIST_OUT ─────────
# The export must not leak into (or gate) any of the Stage 1-6 assertions —
# it only fires after all of them have already passed.

STAGE_6_LINE=$(grep -n 'Stage 6:' "$SCRIPT" | head -1 | cut -d: -f1)

if [ -z "$STAGE_6_LINE" ]; then
  fail "could not locate the 'Stage 6:' marker comment in $SCRIPT"
else
  if sed -n "1,${STAGE_6_LINE}p" "$SCRIPT" | grep -q 'ZFB_SMOKE_DIST_OUT'; then
    fail "ZFB_SMOKE_DIST_OUT is referenced above Stage 6 (line $STAGE_6_LINE) — it must only appear after every assertion has passed"
  else
    pass "no line above Stage 6 (line $STAGE_6_LINE) mentions ZFB_SMOKE_DIST_OUT"
  fi
fi

# ── Assertion 4: positive copy test ─────────────────────────────────────────
# Extracts the REAL export block's source text (not a reimplementation) and
# runs it via bash against a throwaway fixture SITE_DIR/dist, asserting the
# export reproduces its contents — including a hidden dotfile, since `cp -R
# src/. dest/` (not `cp -R src/* dest/`) is what makes that happen.

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

FIXTURE_SITE_DIR="$WORK/fixture-site"
DEST_DIR="$WORK/dest-parent/showcase-dist"

mkdir -p "$FIXTURE_SITE_DIR/dist/assets"
printf '<html><body>fixture dist</body></html>\n' >"$FIXTURE_SITE_DIR/dist/index.html"
printf 'body { color: red; }\n' >"$FIXTURE_SITE_DIR/dist/assets/style.css"
printf 'hidden-marker\n' >"$FIXTURE_SITE_DIR/dist/.hidden-file"

EXPORT_BLOCK=$(awk '
  /ZFB_SMOKE_DIST_OUT:-/ { grab = 1 }
  grab { print }
  grab && $0 ~ /^fi$/ { exit }
' "$SCRIPT")

if [ -z "$EXPORT_BLOCK" ]; then
  fail "could not extract the ZFB_SMOKE_DIST_OUT export block from $SCRIPT"
else
  EXPORT_RUNNER="$WORK/run-export-block.sh"
  {
    printf '#!/bin/bash\n'
    printf 'set -euo pipefail\n'
    # The real script's pass() prints a [PASS] line; stub it so this test's
    # own output stays readable.
    printf 'pass() { :; }\n'
    printf '%s\n' "$EXPORT_BLOCK"
  } >"$EXPORT_RUNNER"
  chmod +x "$EXPORT_RUNNER"

  if SITE_DIR="$FIXTURE_SITE_DIR" ZFB_SMOKE_DIST_OUT="$DEST_DIR" bash "$EXPORT_RUNNER"; then
    pass "positive copy: the real export block ran without error against a fixture dist/"
  else
    fail "positive copy: the real export block exited non-zero"
  fi

  if [ -d "$DEST_DIR" ] && diff -r "$FIXTURE_SITE_DIR/dist" "$DEST_DIR" >/dev/null 2>&1; then
    pass "positive copy: exported dir reproduces the fixture dist/ contents"
  else
    fail "positive copy: exported dir does not match the fixture dist/ contents"
  fi

  if [ -f "$DEST_DIR/.hidden-file" ]; then
    pass "positive copy: hidden dotfiles are copied too (cp -R src/. dest/)"
  else
    fail "positive copy: hidden dotfile missing from the exported dir"
  fi

  # ── Assertion 5: the NEGATIVE path, executed ─────────────────────────────
  # This is the property the whole guard exists for, and the one a static
  # grep cannot prove: with ZFB_SMOKE_DIST_OUT unset, the block must be a
  # true no-op under `set -euo pipefail` — no "unbound variable", exit 0,
  # and nothing written. A regression here aborts the REQUIRED "Scaffold
  # E2E" check on every PR in the repo, so it is worth actually running.

  UNSET_SENTINEL="$WORK/must-not-exist"
  UNSET_OUT=$(SITE_DIR="$FIXTURE_SITE_DIR" ZFB_SMOKE_DIST_OUT= \
    env -u ZFB_SMOKE_DIST_OUT bash "$EXPORT_RUNNER" 2>&1)
  UNSET_STATUS=$?

  if [ "$UNSET_STATUS" -eq 0 ]; then
    pass "negative path: the export block exits 0 when ZFB_SMOKE_DIST_OUT is unset"
  else
    fail "negative path: the export block exited $UNSET_STATUS with the var unset — output: $UNSET_OUT"
  fi

  case "$UNSET_OUT" in
    *"unbound variable"*)
      fail "negative path: the export block hit 'unbound variable' under set -u — this would abort the required Scaffold E2E check"
      ;;
    *)
      pass "negative path: no 'unbound variable' error with ZFB_SMOKE_DIST_OUT unset"
      ;;
  esac

  if [ ! -e "$UNSET_SENTINEL" ] && [ -z "$UNSET_OUT" ]; then
    pass "negative path: the export block is silent and writes nothing when unset"
  else
    fail "negative path: the export block produced output or wrote files when unset — output: $UNSET_OUT"
  fi
fi

# ── Summary ──────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
