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
#   1. Shell-script syntax check (bash -n)        — near-free, no compilation
#   2. cargo fmt --check                          — near-free, no compilation
#   3. pnpm format:check (prettier + mdx)         — fast
#   4. pnpm typecheck (TS, --if-present)          — fast
#   5. pnpm -r test (vitest)                      — fast
#   6. cargo clippy -D warnings                   — fast on a WARM tree
#   7. cargo test --workspace                     — opt-in only (B4PUSH_FULL=1)
#
# Env overrides:
#   B4PUSH_SKIP_CLIPPY=1   — skip clippy (step 6); use on a cold tree to stay bounded
#   B4PUSH_SKIP_JS_TEST=1  — skip the vitest suites (step 5)
#   B4PUSH_FULL=1          — additionally run `cargo test --workspace` (step 7)

START_TIME=$(date +%s)
FAILURES=()
CURRENT_STEP=0

step() {
  CURRENT_STEP=$((CURRENT_STEP + 1))
  echo ""
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "▶ Step $CURRENT_STEP: $1"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
}

pass() { echo "✅ $1"; }
fail() { echo "❌ $1"; FAILURES+=("$1"); }
skip() { echo "⏭  $1 (skipped)"; }

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

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

# ── Step 2: Rust formatting ───────────────────────────
step "Rust formatting (cargo fmt --check)"
if cargo fmt --all --check; then
  pass "cargo fmt"
else
  fail "cargo fmt (run \`cargo fmt --all\` to fix)"
fi

# ── Step 3: JS/MDX formatting ─────────────────────────
step "JS/MDX formatting (pnpm format:check)"
if pnpm format:check; then
  pass "format:check"
else
  fail "format:check (run \`pnpm format\` to fix)"
fi

# ── Step 4: TypeScript typecheck ──────────────────────
step "TypeScript typecheck (pnpm -r typecheck)"
if pnpm -r --if-present typecheck; then
  pass "typecheck"
else
  fail "typecheck"
fi

# ── Step 5: JS test suites (vitest) ───────────────────
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

# ── Step 6: Rust lint (clippy) ────────────────────────
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

# ── Step 7: Full Rust test suite (opt-in) ─────────────
# Off by default — this is what CI's health.yml exists to run. Turn it on for a
# thorough local pass when you have a warm tree and time to spare.
step "Full Rust test suite (cargo test --workspace)"
if [ "${B4PUSH_FULL:-}" = "1" ]; then
  if cargo test --workspace; then
    pass "cargo test --workspace"
  else
    fail "cargo test --workspace"
  fi
else
  skip "cargo test --workspace (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
fi

# ── Summary ──────────────────────────────────────────
END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  b4push SUMMARY (${DURATION}s)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

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
