#!/usr/bin/env bash
set -uo pipefail

# b4push — bounded local quality gate, run before pushing the zfb workspace.
#
# This is the T4 "local heavy lane" from the zudo-test-wisdom strategy: a fast,
# convenience pass — NOT the authoritative gate. The authoritative gate is CI
# (.github/workflows/health.yml), which runs the full `cargo test --workspace`.
# See the "Testing" section of CLAUDE.md.
#
# Why the full Rust test suite is NOT here by default: zfb embeds V8 (rusty_v8),
# whose first compile is 15–30 min. Putting a cold `cargo test --workspace` in a
# pre-push pass would blow the bounded budget, so b4push runs the fast checks and
# leaves the full workspace test to CI (or to `B4PUSH_FULL=1`, below).
#
# Step order (cheap → expensive):
#   1. Shell-script syntax check (bash -n)          — near-free, no compilation
#   2. Offline shell unit tests (tests/unit/*.sh)   — near-free, sub-second; one
#      step per test file, mirrors health.yml:47-50
#   3. cargo fmt --check                          — near-free, no compilation
#   4. pnpm format:check (prettier + mdx)         — fast
#   5. pnpm typecheck (TS, --if-present)          — fast
#   6. pnpm -r test (vitest)                      — fast
#   7. cargo clippy -D warnings                   — fast on a WARM tree
#   8. cargo nextest run --workspace (or cargo test)       — opt-in (B4PUSH_FULL=1)
#   9. cargo nextest run -p zfb-md-extras --features test-utils — opt-in (B4PUSH_FULL=1)
#  10. cargo clippy -p zfb-md-extras --features test-utils — opt-in (B4PUSH_FULL=1)
#  11. cargo check --no-default-features -p zfb --tests    — opt-in (B4PUSH_FULL=1)
#
# Steps 8-9 use cargo-nextest (nextest's DEFAULT profile, retries = 0) when it
# is installed, matching CI's runner; they fall back to plain `cargo test` when
# nextest is absent (issue #1340).
#
# Steps 9-11 mirror the health.yml lanes that steps 1-8 alone cannot reproduce
# (issue #1332): the zfb-md-extras `test-utils`-gated suite + its scoped clippy
# (health.yml:165,169), and the V8-off `build-no-v8` job's cargo check
# (health.yml:219). Without them, "B4PUSH_FULL=1 pnpm b4push passed" did not
# imply "health.yml will pass."
#
# Env overrides:
#   B4PUSH_SKIP_CLIPPY=1   — skip clippy (step 7); use on a cold tree to stay bounded
#   B4PUSH_SKIP_JS_TEST=1  — skip the vitest suites (step 6)
#   B4PUSH_FULL=1          — additionally run steps 8-11 (full workspace test,
#                            zfb-md-extras test-utils lane, no-V8 cargo check)

START_TIME=$(date +%s)
FAILURES=()
CURRENT_STEP=0
CURRENT_STEP_NAME=""
STEP_START_TIME=0
LAST_DURATION=0
STEP_NAMES=()
STEP_DURATIONS=()

step() {
  CURRENT_STEP=$((CURRENT_STEP + 1))
  CURRENT_STEP_NAME="$1"
  STEP_START_TIME=$(date +%s)
  echo ""
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "▶ Step $CURRENT_STEP: $1"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
}

# Records the elapsed time for the current step into the summary tables.
# Called exactly once per step, from pass/fail/skip.
record_duration() {
  local end
  end=$(date +%s)
  LAST_DURATION=$((end - STEP_START_TIME))
  STEP_NAMES+=("$CURRENT_STEP_NAME")
  STEP_DURATIONS+=("$LAST_DURATION")
}

pass() { record_duration; echo "✅ $1 (${LAST_DURATION}s)"; }
fail() { record_duration; echo "❌ $1 (${LAST_DURATION}s)"; FAILURES+=("$1"); }
skip() { record_duration; echo "⏭  $1 (skipped, ${LAST_DURATION}s)"; }

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

# Prefer cargo-nextest for the B4PUSH_FULL Rust lanes (issue #1340): it matches
# what CI runs and serializes the heavy V8/esbuild e2e binaries via the
# `e2e-heavy` test-group in .config/nextest.toml. We use nextest's DEFAULT
# profile locally (retries = 0 — local dev does not want CI's retry budget
# masking a genuine failure), unlike CI's `--profile ci` (retries = 1). If
# nextest is not installed we fall back to plain `cargo test`, so this stays
# tolerant of a dev box without nextest.
if command -v cargo-nextest >/dev/null 2>&1; then
  HAVE_NEXTEST=1
else
  HAVE_NEXTEST=0
fi

# ── Step 1: Shell-script syntax check ─────────────────
# Mirrors health.yml: parse-check install.sh, scripts/*.sh, tests/unit/*.sh.
step "Shell-script syntax check (bash -n)"
SH_OK=1
for f in install.sh scripts/*.sh tests/unit/*.sh; do
  [ -e "$f" ] || continue
  if ! bash -n "$f"; then
    echo "  syntax error in $f"
    SH_OK=0
  fi
done
if [ "$SH_OK" -eq 1 ]; then
  pass "Shell-script syntax"
else
  fail "Shell-script syntax"
fi

# ── Step 2+: Offline shell unit tests ─────────────────
# Actually EXECUTE tests/unit/*.sh (not just bash -n parse it above). These are
# offline and sub-second by design (issue #1332) — mirrors health.yml:47-50,
# which runs them via `sh "$t"`. One step per file so a failing test is
# individually attributable in the summary.
for t in tests/unit/*.sh; do
  [ -e "$t" ] || continue
  step "Offline shell unit test: $(basename "$t")"
  if sh "$t"; then
    pass "$(basename "$t")"
  else
    fail "$(basename "$t")"
  fi
done

# ── Rust formatting ────────────────────────────────────
step "Rust formatting (cargo fmt --check)"
if cargo fmt --all --check; then
  pass "cargo fmt"
else
  fail "cargo fmt (run \`cargo fmt --all\` to fix)"
fi

# ── JS/MDX formatting ──────────────────────────────────
step "JS/MDX formatting (pnpm format:check)"
if pnpm format:check; then
  pass "format:check"
else
  fail "format:check (run \`pnpm format\` to fix)"
fi

# ── TypeScript typecheck ───────────────────────────────
step "TypeScript typecheck (pnpm -r typecheck)"
if pnpm -r --if-present typecheck; then
  pass "typecheck"
else
  fail "typecheck"
fi

# ── JS test suites (vitest) ────────────────────────────
step "JS test suites (pnpm -r test)"
if [ "${B4PUSH_SKIP_JS_TEST:-}" = "1" ]; then
  skip "JS tests (B4PUSH_SKIP_JS_TEST=1)"
else
  if pnpm -r test; then
    pass "JS tests"
  else
    fail "JS tests"
  fi
fi

# ── Rust lint (clippy) ─────────────────────────────────
# Fast on a warm tree; on a cold tree it triggers the V8 first-compile, so this
# step is skippable to keep the pass bounded. CI runs it unconditionally.
step "Rust lint (cargo clippy -D warnings)"
if [ "${B4PUSH_SKIP_CLIPPY:-}" = "1" ]; then
  skip "clippy (B4PUSH_SKIP_CLIPPY=1)"
else
  if cargo clippy --workspace --all-targets -- -D warnings; then
    pass "clippy"
  else
    fail "clippy"
  fi
fi

# ── Full Rust test suite (opt-in) ──────────────────────
# Off by default — this is what CI's health.yml exists to run. Turn it on for a
# thorough local pass when you have a warm tree and time to spare. Uses nextest
# (default profile, retries = 0) when installed, else plain cargo test.
if [ "$HAVE_NEXTEST" = "1" ]; then
  WS_TEST_CMD=(cargo nextest run --workspace)
else
  WS_TEST_CMD=(cargo test --workspace)
fi
step "Full Rust test suite (${WS_TEST_CMD[*]})"
if [ "${B4PUSH_FULL:-}" = "1" ]; then
  if "${WS_TEST_CMD[@]}"; then
    pass "${WS_TEST_CMD[*]}"
  else
    fail "${WS_TEST_CMD[*]}"
  fi
else
  skip "${WS_TEST_CMD[*]} (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
fi

# ── zfb-md-extras test-utils suite (opt-in) ────────────
# `test-utils` is not in the default feature set, so the workspace test lane
# above silently skips every [[test]] target that requires it (issue #1091,
# mirrored in health.yml). Without this step, B4PUSH_FULL=1 could pass locally
# while health.yml still fails on this lane (issue #1332). nextest picks up the
# required-features-gated targets when the feature is enabled (verified in
# #1340), so this uses the same runner as the workspace lane above.
if [ "$HAVE_NEXTEST" = "1" ]; then
  MDX_TEST_CMD=(cargo nextest run -p zfb-md-extras --features test-utils)
else
  MDX_TEST_CMD=(cargo test -p zfb-md-extras --features test-utils)
fi
step "zfb-md-extras test-utils suite (${MDX_TEST_CMD[*]})"
if [ "${B4PUSH_FULL:-}" = "1" ]; then
  if "${MDX_TEST_CMD[@]}"; then
    pass "${MDX_TEST_CMD[*]}"
  else
    fail "${MDX_TEST_CMD[*]}"
  fi
else
  skip "zfb-md-extras test-utils suite (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
fi

# ── zfb-md-extras test-utils lint (opt-in) ─────────────
# Lints the test-utils-gated targets, excluded from the workspace clippy step
# above for the same reason as cargo test (mirrors health.yml:169).
step "zfb-md-extras test-utils lint (cargo clippy -p zfb-md-extras --features test-utils)"
if [ "${B4PUSH_FULL:-}" = "1" ]; then
  if cargo clippy -p zfb-md-extras --features test-utils --all-targets -- -D warnings; then
    pass "cargo clippy -p zfb-md-extras --features test-utils"
  else
    fail "cargo clippy -p zfb-md-extras --features test-utils"
  fi
else
  skip "zfb-md-extras test-utils lint (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
fi

# ── No-V8 cargo check (opt-in) ──────────────────────────
# Mirrors the build-no-v8 job's cargo check (health.yml:219), which proves the
# cfg(feature = "embed_v8") gates still compile with V8 off. Not covered by any
# other step above — all of them build with default features (V8 on).
step "No-V8 cargo check (cargo check --no-default-features -p zfb --tests)"
if [ "${B4PUSH_FULL:-}" = "1" ]; then
  if cargo check --no-default-features -p zfb --tests; then
    pass "cargo check --no-default-features -p zfb --tests"
  else
    fail "cargo check --no-default-features -p zfb --tests"
  fi
else
  skip "No-V8 cargo check (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
fi

# ── Summary ──────────────────────────────────────────
END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  b4push SUMMARY (${DURATION}s)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "  Step durations:"
for i in "${!STEP_NAMES[@]}"; do
  printf "   %-6ss  %s\n" "${STEP_DURATIONS[$i]}" "${STEP_NAMES[$i]}"
done
echo ""

if [ ${#FAILURES[@]} -eq 0 ]; then
  echo "✅ All checks passed (or skipped). Safe to push."
  echo "   Reminder: health.yml is the authoritative gate — b4push is the fast subset."
  exit 0
else
  echo "❌ ${#FAILURES[@]} check(s) failed:"
  for f in "${FAILURES[@]}"; do
    echo "   - $f"
  done
  exit 1
fi
