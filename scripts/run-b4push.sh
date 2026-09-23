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
#   2. Offline shell unit tests (tests/unit/*.sh)   — near-free, mostly sub-second; one
#      step per test file, mirrors health.yml:47-50
#   3. cargo machete --with-metadata (optional local tool) — fast
#   4. cargo fmt --check                          — near-free, no compilation
#   5. pnpm format:check (prettier + mdx)         — fast
#   6. node scripts/assert-md-wasm-size-docs.mjs — fast
#   7. node scripts/check-sse-endpoint.mjs — fast, offline source guard
#   8. pnpm typecheck:workspace (excluding examples + zfb-md-wasm) — fast
#   9. pnpm test:workspace (vitest; excluding zfb-md-wasm) — fast
#  10. cargo clippy -D warnings                   — fast on a WARM tree
#  11. Regenerate + diff syntect syntax-set.packdump       — opt-in (B4PUSH_FULL=1)
#  12. cargo nextest run --workspace (or cargo test)       — opt-in (B4PUSH_FULL=1)
#  13. cargo test --workspace --doc (nextest branch only)  — opt-in (B4PUSH_FULL=1)
#  14. cargo nextest run -p zfb-md-extras --features test-utils — opt-in (B4PUSH_FULL=1)
#  15. cargo clippy -p zfb-md-extras --features test-utils — opt-in (B4PUSH_FULL=1)
#  16. cargo check --no-default-features -p zfb --tests    — opt-in (B4PUSH_FULL=1)
#  17. cargo test -p zfb-islands --tests -- --ignored (esbuild env-gate)  — opt-in (B4PUSH_FULL=1)
#  18. cargo test -p zfb --test client_bundling_cross_pipeline -- --ignored (esbuild env-gate) — opt-in (B4PUSH_FULL=1)
#  19. cargo test -p zfb-css --test integration -- --ignored (tailwindcss env-gate)     — opt-in (B4PUSH_FULL=1)
#  20. cargo test -p zfb-build --test prod_asset_graph_e2e -- --ignored (tailwindcss env-gate) — opt-in (B4PUSH_FULL=1)
#  21. cargo test -p zfb --lib commands::build:: -- --ignored (command-layer env-gates) — opt-in (B4PUSH_FULL=1)
#
# Steps 12 and 14 use cargo-nextest (nextest's DEFAULT profile, retries = 0) when
# it is installed, matching CI's runner; they fall back to plain `cargo test`
# when nextest is absent (issue #1340). Step 13 exists ONLY in the nextest branch:
# nextest does NOT run doctests, so — mirroring health.yml's separate doc step —
# a `cargo test --workspace --doc` follows the nextest workspace run to keep
# doctest coverage. The plain-`cargo test --workspace` fallback already runs
# doctests as part of that same command, so it needs no separate step.
#
# Steps 14-16 mirror the health.yml lanes that steps 1-12 alone cannot reproduce
# (issue #1332): the zfb-md-extras `test-utils`-gated suite + its scoped clippy
# (health.yml:165,169), and the V8-off `build-no-v8` job's cargo check
# (health.yml:219). Without them, "B4PUSH_FULL=1 pnpm b4push passed" did not
# imply "health.yml will pass."
#
# Steps 17-21 maintain parity with health.yml's dedicated env-gated lanes. The
# zfb-islands esbuild-gated lane (added by #1337) had no B4PUSH_FULL
# counterpart, so a green B4PUSH_FULL=1 run did not imply that lane would pass
# in CI — the exact
# gap #1332 closed for the md-extras/no-v8 lanes, reopened when #1337 added the
# islands step to health.yml without updating b4push. Step 17 restores it;
# step 18 mirrors #1504's real-zfb cross-pipeline acceptance lane.
# Steps 19-21 cover the 3 tailwindcss-v4 env-gate locations that #1393 wired
# into health.yml for the first time; step 21 also co-runs the two esbuild
# command-layer env-gates, so it pins both binary slots. All 5 steps are
# guarded on their staged binary existing (a tree built with
# B4PUSH_SKIP_CLIPPY=1, or one that has never run
# `cargo build --workspace --all-targets`, may not have triggered
# crates/zfb/build.rs's binary download yet) and set their env var to an
# ABSOLUTE path, same reasoning as health.yml's ZFB_ESBUILD_BIN/ZFB_TAILWIND_BIN
# wiring: `cargo test` runs each test binary with its CWD set to the owning
# package directory, not the workspace root, so the crates' relative default
# binary_path never resolves.
#
# Env overrides:
#   B4PUSH_SKIP_CLIPPY=1   — skip clippy (step 10); use on a cold tree to stay bounded
#   B4PUSH_SKIP_JS_TEST=1  — skip the vitest suites (step 9)
#   B4PUSH_FULL=1          — additionally run steps 11-21 (syntect packdump
#                            freshness gate, full workspace test,
#                            zfb-md-extras test-utils lane, no-V8 cargo check,
#                            esbuild + tailwindcss env-gate suites)
#   HEAVY_GUARD=/path      — override the machine-wide heavy-guard executable

START_TIME=$(date +%s)
FAILURES=()
CURRENT_STEP=0
CURRENT_STEP_NAME=""
STEP_START_TIME=0
LAST_DURATION=0
STEP_NAMES=()
STEP_DURATIONS=()
CONTENTIONS=()
HEAVY_GUARD_ACTIVE=0

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

# Queue only full Rust suites and heavy integration/asset lanes. The guard is
# optional: CI and machines without an installed guard run commands directly.
# An explicit HEAVY_GUARD wins over the installed Claude/Codex paths; if it is
# configured but cannot execute, preserve its status instead of silently
# falling back to an unguarded run.
run_heavy() {
  local guard="${HEAVY_GUARD:-}"
  local candidate
  local status

  HEAVY_GUARD_ACTIVE=0

  if [ -z "$guard" ] && [ -n "${HOME:-}" ]; then
    for candidate in "$HOME/.claude/scripts/heavy-guard.sh" "$HOME/.codex/scripts/heavy-guard.sh"; do
      if [ -x "$candidate" ]; then
        guard="$candidate"
        break
      fi
    done
  fi

  if [ -n "${CI:-}" ] || [ -z "$guard" ]; then
    "$@"
    status=$?
  else
    HEAVY_GUARD_ACTIVE=1
    "$guard" -- "$@"
    status=$?
  fi
  return "$status"
}

heavy() { run_heavy "$@"; }

# Run an environment-injected command through the same queue without using an
# assignment prefix on the function call (Bash would scope state set by the
# function and hide whether a guard actually returned exit 75).
heavy_env() {
  local -a command_env=()
  while [ "$1" != "--" ]; do
    command_env+=("$1")
    shift
  done
  shift
  run_heavy env "${command_env[@]}" "$@"
}

# Exit 75 means queue contention and the command never ran. Keep it out of the
# check-failure list and make the overall b4push result non-green below.
heavy_failure() {
  local label="$1"
  local status="$2"

  if [ "$status" -eq 75 ] && [ "$HEAVY_GUARD_ACTIVE" -eq 1 ]; then
    record_duration
    echo "⏸ heavy-guard contention: $label was not run (exit 75)"
    CONTENTIONS+=("$label")
  else
    fail "$label"
  fi
}

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
# offline and mostly sub-second by design (issue #1332) — mirrors health.yml's
# offline shell unit-test step,
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

# ── Cargo dependency audit ────────────────────────────
# cargo-machete is optional for local development. CI provisions an exact
# version in health.yml, while a fresh local checkout should remain usable
# without the optional binary.
step "Dead-dependency check (cargo-machete)"
if command -v cargo-machete >/dev/null 2>&1; then
  if cargo machete --with-metadata; then
    pass "cargo machete"
  else
    fail "cargo machete"
  fi
else
  skip "cargo machete (cargo-machete not installed)"
fi

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

# ── MD/Wasm documentation size guard ───────────────────
step "MD/Wasm documentation size guard"
if node scripts/assert-md-wasm-size-docs.mjs; then
  pass "MD/Wasm documentation size guard"
else
  fail "MD/Wasm documentation size guard"
fi

# ── SSE endpoint literal guard ─────────────────────────
# Issue #2644: the reqwest `.timeout()` API bounds an entire streaming
# response, so hand-written `/__zfb/reload` subscribers can time out against
# the server's 15s keep-alive. The source guard forces tests to use the
# timeout-aware zfb_test_utils::open_sse() helper. Keep this next to the other
# cheap, offline source/documentation guards and mirror it in health.yml.
step "SSE endpoint literal guard"
if node scripts/check-sse-endpoint.mjs; then
  pass "SSE endpoint literal guard"
else
  fail "SSE endpoint literal guard"
fi

# ── TypeScript typecheck ───────────────────────────────
# The canonical `pnpm typecheck:workspace` lane excludes @takazudo/zfb-md-wasm
# to match health.yml's own typecheck step:
# its src/index.ts type-imports the wasm-bindgen-generated glue, which does not
# exist until the crate is compiled for wasm32-unknown-unknown. Its own build
# runs `tsc`, so coverage is relocated rather than lost.
step "TypeScript typecheck (pnpm typecheck:workspace)"
if pnpm typecheck:workspace; then
  pass "typecheck"
else
  fail "typecheck"
fi

# ── JS test suites (vitest) ────────────────────────────
# Same @takazudo/zfb-md-wasm exclusion as health.yml's test step: its tests
# import the built dist/index.js, which needs a wasm build b4push never runs.
# Without this, b4push fails on a fresh checkout (issue #2043).
step "JS test suites (pnpm test:workspace)"
if [ "${B4PUSH_SKIP_JS_TEST:-}" = "1" ]; then
  skip "JS tests (B4PUSH_SKIP_JS_TEST=1)"
else
  # `pnpm test:workspace` owns --include-workspace-root, so the root vitest
  # suite covering scripts/** runs here exactly as it does in health.yml.
  if pnpm test:workspace; then
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

# ── Syntect syntax-set.packdump freshness gate (opt-in) ──
# Mirrors health.yml's source-versus-generated drift gate (issue #1871). The
# generator changes to its crate root and serializes normalized relative paths,
# so the tracked dump stays byte-identical across checkout locations.
step "Syntect syntax-set.packdump freshness gate"
if [ "${B4PUSH_FULL:-}" = "1" ]; then
  if heavy cargo run -p zfb-content --bin generate_syntax_dump --features generate-syntax-dump; then
    if git diff --exit-code -- crates/zfb-content/assets/syntax-set.packdump; then
      pass "Syntect syntax-set.packdump freshness gate"
    else
      fail "Syntect syntax-set.packdump freshness gate"
    fi
  else
    heavy_failure "Syntect syntax-set.packdump freshness gate" "$?"
  fi
else
  skip "Syntect syntax-set.packdump freshness gate (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
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
  if heavy "${WS_TEST_CMD[@]}"; then
    pass "${WS_TEST_CMD[*]}"
  else
    heavy_failure "${WS_TEST_CMD[*]}" "$?"
  fi
else
  skip "${WS_TEST_CMD[*]} (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
fi

# ── Rust doctests (opt-in, nextest branch only) ────────
# cargo-nextest does NOT run doctests, so when the workspace lane above used
# `cargo nextest run` it left doctest examples uncovered. Mirror health.yml's
# separate `cargo test --workspace --doc` step (issue #1340) so a local
# B4PUSH_FULL pass can't go green while a doctest is broken — only to fail later
# in CI. This step is emitted ONLY in the nextest branch: the plain
# `cargo test --workspace` fallback already runs doctests as part of that same
# command, so a second doc-only run there would be redundant.
if [ "$HAVE_NEXTEST" = "1" ]; then
  step "Rust doctests (cargo test --workspace --doc)"
  if [ "${B4PUSH_FULL:-}" = "1" ]; then
    if heavy cargo test --workspace --doc; then
      pass "cargo test --workspace --doc"
    else
      heavy_failure "cargo test --workspace --doc" "$?"
    fi
  else
    skip "cargo test --workspace --doc (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
  fi
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
  if heavy "${MDX_TEST_CMD[@]}"; then
    pass "${MDX_TEST_CMD[*]}"
  else
    heavy_failure "${MDX_TEST_CMD[*]}" "$?"
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

# ── zfb-islands esbuild env-gate suite (opt-in) ─────────
# Restores b4push/health.yml parity (issue #1393) — see the header comment for
# why. Guarded on the staged binary existing: a tree that never ran
# `cargo build --workspace --all-targets` (e.g. B4PUSH_SKIP_CLIPPY=1 on a cold
# checkout) may not have triggered crates/zfb/build.rs's esbuild download yet.
ESBUILD_SLOT="${ROOT_DIR}/crates/zfb/binaries/esbuild/esbuild"
step "zfb-islands esbuild env-gate suite (cargo test -p zfb-islands --tests -- --ignored)"
if [ "${B4PUSH_FULL:-}" = "1" ]; then
  if [ -x "$ESBUILD_SLOT" ]; then
    if heavy_env "ZFB_ESBUILD_BIN=$ESBUILD_SLOT" -- cargo test -p zfb-islands --tests -- --ignored; then
      pass "zfb-islands esbuild env-gate suite"
    else
      heavy_failure "zfb-islands esbuild env-gate suite" "$?"
    fi
  else
    skip "zfb-islands esbuild env-gate suite ($ESBUILD_SLOT not staged — run \`cargo build --workspace --all-targets\` first)"
  fi
else
  skip "zfb-islands esbuild env-gate suite (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
fi

# ── zfb cross-pipeline esbuild env-gate (opt-in) ─────────
# Mirrors health.yml's dedicated #1504 acceptance lane. It is intentionally
# separate from the zfb-islands suite because it spawns the real zfb binary,
# runs both build and dev --port 0, and exercises SSR plus both client pipelines.
step "zfb client-bundling cross-pipeline acceptance test"
if [ "${B4PUSH_FULL:-}" = "1" ]; then
  if [ -x "$ESBUILD_SLOT" ]; then
    if heavy_env "ZFB_ESBUILD_BIN=$ESBUILD_SLOT" -- cargo test -p zfb --test client_bundling_cross_pipeline -- --ignored; then
      pass "zfb client-bundling cross-pipeline acceptance test"
    else
      heavy_failure "zfb client-bundling cross-pipeline acceptance test" "$?"
    fi
  else
    skip "zfb client-bundling cross-pipeline acceptance test ($ESBUILD_SLOT not staged — run \`cargo build --workspace --all-targets\` first)"
  fi
else
  skip "zfb client-bundling cross-pipeline acceptance test (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
fi

# ── tailwindcss-v4 env-gate suites (opt-in) ─────────────
# The 3 tests health.yml (issue #1393) now runs in CI — see that workflow's
# comment for why these were silently skipped for so long despite the binary
# already being staged. Same staged-binary guard and ABSOLUTE-path env var as
# the esbuild step above.
TAILWIND_SLOT="${ROOT_DIR}/crates/zfb/binaries/tailwindcss-v4"

step "zfb-css tailwindcss-v4 env-gate test (cargo test -p zfb-css --test integration -- --ignored)"
if [ "${B4PUSH_FULL:-}" = "1" ]; then
  if [ -x "$TAILWIND_SLOT" ]; then
    if heavy_env "ZFB_TAILWIND_BIN=$TAILWIND_SLOT" -- cargo test -p zfb-css --test integration -- --ignored; then
      pass "zfb-css tailwindcss-v4 env-gate test"
    else
      heavy_failure "zfb-css tailwindcss-v4 env-gate test" "$?"
    fi
  else
    skip "zfb-css tailwindcss-v4 env-gate test ($TAILWIND_SLOT not staged — run \`cargo build --workspace --all-targets\` first)"
  fi
else
  skip "zfb-css tailwindcss-v4 env-gate test (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
fi

step "zfb-build tailwindcss-v4 env-gate test (cargo test -p zfb-build --test prod_asset_graph_e2e -- --ignored)"
if [ "${B4PUSH_FULL:-}" = "1" ]; then
  if [ -x "$TAILWIND_SLOT" ]; then
    if heavy_env "ZFB_TAILWIND_BIN=$TAILWIND_SLOT" -- cargo test -p zfb-build --test prod_asset_graph_e2e -- --ignored; then
      pass "zfb-build tailwindcss-v4 env-gate test"
    else
      heavy_failure "zfb-build tailwindcss-v4 env-gate test" "$?"
    fi
  else
    skip "zfb-build tailwindcss-v4 env-gate test ($TAILWIND_SLOT not staged — run \`cargo build --workspace --all-targets\` first)"
  fi
else
  skip "zfb-build tailwindcss-v4 env-gate test (set B4PUSH_FULL=1 to run; CI runs it on every PR)"
fi

step "zfb command-layer env-gates (cargo test -p zfb --lib commands::build:: -- --ignored)"
if [ "${B4PUSH_FULL:-}" = "1" ]; then
  if [ -x "$ESBUILD_SLOT" ] && [ -x "$TAILWIND_SLOT" ]; then
    if heavy_env "ZFB_ESBUILD_BIN=$ESBUILD_SLOT" "ZFB_TAILWIND_BIN=$TAILWIND_SLOT" -- cargo test -p zfb --lib commands::build:: -- --ignored; then
      pass "zfb command-layer env-gates"
    else
      heavy_failure "zfb command-layer env-gates" "$?"
    fi
  else
    skip "zfb command-layer env-gates (esbuild and/or tailwind slot not staged — run \`cargo build --workspace --all-targets\` first)"
  fi
else
  skip "zfb command-layer env-gates (set B4PUSH_FULL=1 to run; CI runs them on every PR)"
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

if [ ${#FAILURES[@]} -eq 0 ] && [ ${#CONTENTIONS[@]} -eq 0 ]; then
  echo "✅ All checks passed (or skipped). Safe to push."
  echo "   Reminder: health.yml is the authoritative gate — b4push is the fast subset."
  exit 0
fi

if [ ${#FAILURES[@]} -gt 0 ]; then
  echo "❌ ${#FAILURES[@]} check(s) failed:"
  for f in "${FAILURES[@]}"; do
    echo "   - $f"
  done
fi

if [ ${#CONTENTIONS[@]} -gt 0 ]; then
  echo "⏸ ${#CONTENTIONS[@]} heavy step(s) were not run because the heavy-guard queue was contended (exit 75):"
  for c in "${CONTENTIONS[@]}"; do
    echo "   - $c"
  done
fi

if [ ${#FAILURES[@]} -eq 0 ] && [ ${#CONTENTIONS[@]} -gt 0 ]; then
  exit 75
fi

exit 1
