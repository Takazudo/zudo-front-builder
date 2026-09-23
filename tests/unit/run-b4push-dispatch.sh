#!/bin/sh
# Offline dispatch tests for scripts/run-b4push.sh's optional heavy-guard queue.
# The script is copied into an isolated fixture and every external command it
# dispatches is stubbed; no Cargo build or test suite is run.

set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
SOURCE_SCRIPT="$REPO_ROOT/scripts/run-b4push.sh"
SYSTEM_PATH=
TEMP_ROOT=$(mktemp -d)
trap 'rm -rf "$TEMP_ROOT"' EXIT HUP INT TERM

PASS=0
FAIL=0
CASE_DIR=
FIXTURE_ROOT=
FIXTURE_BIN=
FIXTURE_HOME=
RUN_STATUS=0
RUN_OUTPUT=

pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

assert_status() {
  if [ "$2" -eq "$3" ]; then pass "$1"; else fail "$1 (expected $2, got $3)"; fi
}

assert_contains() {
  if grep -F "$2" "$1" >/dev/null 2>&1; then pass "$3"; else fail "$3 (missing: $2)"; fi
}

assert_not_contains() {
  if grep -F "$2" "$1" >/dev/null 2>&1; then fail "$3 (unexpected: $2)"; else pass "$3"; fi
}

setup_fixture() {
  CASE_DIR="$TEMP_ROOT/$1"
  FIXTURE_ROOT="$CASE_DIR/repo"
  FIXTURE_BIN="$CASE_DIR/bin"
  FIXTURE_HOME="$CASE_DIR/home"
  SYSTEM_PATH="$CASE_DIR/system-bin"
  mkdir -p "$FIXTURE_ROOT/scripts" "$FIXTURE_ROOT/tests/unit" \
    "$FIXTURE_ROOT/crates/zfb/binaries/esbuild" "$FIXTURE_ROOT/crates/zfb/binaries" \
    "$FIXTURE_BIN" "$FIXTURE_HOME" "$SYSTEM_PATH"
  for system_command in bash dirname env; do
    ln -s "$(command -v "$system_command")" "$SYSTEM_PATH/$system_command"
  done
  cp "$SOURCE_SCRIPT" "$FIXTURE_ROOT/scripts/run-b4push.sh"
  : > "$FIXTURE_ROOT/install.sh"
  : > "$FIXTURE_ROOT/crates/zfb/binaries/esbuild/esbuild"
  : > "$FIXTURE_ROOT/crates/zfb/binaries/tailwindcss-v4"
  chmod +x "$FIXTURE_ROOT/crates/zfb/binaries/esbuild/esbuild" \
    "$FIXTURE_ROOT/crates/zfb/binaries/tailwindcss-v4"

  cat > "$FIXTURE_BIN/cargo" <<'MOCK_CARGO'
#!/bin/sh
printf 'CALL: cargo %s\n' "$*" >> "$MOCK_CARGO_LOG"
printf 'ENV: ZFB_ESBUILD_BIN=%s ZFB_TAILWIND_BIN=%s\n' \
  "${ZFB_ESBUILD_BIN:-}" "${ZFB_TAILWIND_BIN:-}" >> "$MOCK_CARGO_ENV_LOG"
if [ -n "${MOCK_CARGO_FAIL_MATCH:-}" ]; then
  case " $* " in
    *"$MOCK_CARGO_FAIL_MATCH"*) exit "${MOCK_CARGO_FAIL_STATUS:-1}" ;;
  esac
fi
exit 0
MOCK_CARGO

  cat > "$FIXTURE_BIN/cargo-nextest" <<'MOCK_NEXTEST'
#!/bin/sh
exit 0
MOCK_NEXTEST

  cat > "$FIXTURE_BIN/pnpm" <<'MOCK_PNPM'
#!/bin/sh
printf 'CALL: pnpm %s\n' "$*" >> "$MOCK_PNPM_LOG"
exit 0
MOCK_PNPM

  cat > "$FIXTURE_BIN/node" <<'MOCK_NODE'
#!/bin/sh
exit 0
MOCK_NODE

  cat > "$FIXTURE_BIN/git" <<'MOCK_GIT'
#!/bin/sh
printf 'CALL: git %s\n' "$*" >> "$MOCK_GIT_LOG"
if [ "${1:-}" = "diff" ]; then exit "${MOCK_GIT_DIFF_STATUS:-0}"; fi
exit 0
MOCK_GIT

  cat > "$FIXTURE_BIN/sh" <<'MOCK_SH'
#!/bin/sh
# The fixture contains no shell test files. Keep this dispatch stub in place
# so additions to the fixture cannot recursively invoke the test harness.
exit 0
MOCK_SH

  cat > "$FIXTURE_BIN/date" <<'MOCK_DATE'
#!/bin/sh
printf '100000\n'
MOCK_DATE

  chmod +x "$FIXTURE_BIN/cargo" "$FIXTURE_BIN/cargo-nextest" "$FIXTURE_BIN/pnpm" \
    "$FIXTURE_BIN/node" "$FIXTURE_BIN/git" "$FIXTURE_BIN/sh" "$FIXTURE_BIN/date"
}

make_guard() {
  mkdir -p "$(dirname "$1")"
  cat > "$1" <<'MOCK_GUARD'
#!/bin/sh
case "$0" in
  */.claude/scripts/heavy-guard.sh) log_file="$MOCK_CLAUDE_LOG" ;;
  */.codex/scripts/heavy-guard.sh) log_file="$MOCK_CODEX_LOG" ;;
  *) log_file="$MOCK_EXPLICIT_LOG" ;;
esac
printf 'CALL: %s\n' "$*" >> "$log_file"
if [ "${1:-}" != "--" ]; then exit 91; fi
shift
if [ -n "${MOCK_GUARD_EXIT_MATCH:-}" ]; then
  case " $* " in
    *"$MOCK_GUARD_EXIT_MATCH"*) exit "${MOCK_GUARD_EXIT_STATUS:-1}" ;;
  esac
fi
"$@"
exit $?
MOCK_GUARD
  chmod +x "$1"
}

run_b4push() {
  RUN_OUTPUT="$CASE_DIR/output.log"
  if env -i \
    PATH="$FIXTURE_BIN:$SYSTEM_PATH" \
    HOME="$FIXTURE_HOME" \
    MOCK_CARGO_LOG="$CASE_DIR/cargo.log" \
    MOCK_CARGO_ENV_LOG="$CASE_DIR/cargo-env.log" \
    MOCK_PNPM_LOG="$CASE_DIR/pnpm.log" \
    MOCK_GIT_LOG="$CASE_DIR/git.log" \
    MOCK_EXPLICIT_LOG="$CASE_DIR/explicit-guard.log" \
    MOCK_CLAUDE_LOG="$CASE_DIR/claude-guard.log" \
    MOCK_CODEX_LOG="$CASE_DIR/codex-guard.log" \
    "$@" \
    bash "$FIXTURE_ROOT/scripts/run-b4push.sh" > "$RUN_OUTPUT" 2>&1; then
    RUN_STATUS=0
  else
    RUN_STATUS=$?
  fi
}

# Explicit override wins over both installed paths, and command arguments plus
# binary environment injection reach the guard and the cargo process intact.
setup_fixture explicit-nextest
make_guard "$FIXTURE_HOME/.claude/scripts/heavy-guard.sh"
make_guard "$FIXTURE_HOME/.codex/scripts/heavy-guard.sh"
make_guard "$CASE_DIR/selected-guard"
run_b4push HEAVY_GUARD="$CASE_DIR/selected-guard" B4PUSH_FULL=1 \
  B4PUSH_SKIP_CLIPPY=1 B4PUSH_SKIP_JS_TEST=1
assert_status "explicit override b4push succeeds" 0 "$RUN_STATUS"
assert_contains "$CASE_DIR/explicit-guard.log" 'CALL: -- cargo nextest run --workspace' "nextest workspace args preserved"
assert_contains "$CASE_DIR/explicit-guard.log" 'CALL: -- cargo test --workspace --doc' "nextest doctest args preserved"
assert_contains "$CASE_DIR/explicit-guard.log" "CALL: -- env ZFB_ESBUILD_BIN=$FIXTURE_ROOT/crates/zfb/binaries/esbuild/esbuild cargo test -p zfb-islands --tests -- --ignored" "esbuild lane args and env preserved"
assert_contains "$CASE_DIR/explicit-guard.log" "CALL: -- env ZFB_TAILWIND_BIN=$FIXTURE_ROOT/crates/zfb/binaries/tailwindcss-v4 cargo test -p zfb-css --test integration -- --ignored" "tailwind lane args and env preserved"
assert_contains "$CASE_DIR/explicit-guard.log" "CALL: -- env ZFB_ESBUILD_BIN=$FIXTURE_ROOT/crates/zfb/binaries/esbuild/esbuild ZFB_TAILWIND_BIN=$FIXTURE_ROOT/crates/zfb/binaries/tailwindcss-v4 cargo test -p zfb --lib commands::build:: -- --ignored" "dual-binary lane args and env preserved"
assert_contains "$CASE_DIR/cargo-env.log" "ENV: ZFB_ESBUILD_BIN=$FIXTURE_ROOT/crates/zfb/binaries/esbuild/esbuild ZFB_TAILWIND_BIN=$FIXTURE_ROOT/crates/zfb/binaries/tailwindcss-v4" "binary environment reaches cargo"
assert_not_contains "$CASE_DIR/claude-guard.log" 'CALL:' "explicit guard takes precedence over Claude"
assert_not_contains "$CASE_DIR/codex-guard.log" 'CALL:' "explicit guard takes precedence over Codex"
assert_not_contains "$CASE_DIR/explicit-guard.log" 'CALL: -- cargo clippy' "lint stays outside the queue"
assert_not_contains "$CASE_DIR/explicit-guard.log" 'CALL: -- cargo check' "no-V8 check stays outside the queue"
assert_contains "$RUN_OUTPUT" 'JS tests (B4PUSH_SKIP_JS_TEST=1)' "JS skip flag remains effective"
assert_contains "$RUN_OUTPUT" 'clippy (B4PUSH_SKIP_CLIPPY=1)' "clippy skip flag remains effective"
assert_not_contains "$CASE_DIR/pnpm.log" 'CALL: pnpm test:workspace' "JS suite command remains skipped"

# With no explicit override, Claude is discovered before Codex. Absence of
# cargo-nextest retains the cargo test fallback and its embedded doctests.
setup_fixture discovery-fallback
rm "$FIXTURE_BIN/cargo-nextest"
make_guard "$FIXTURE_HOME/.claude/scripts/heavy-guard.sh"
make_guard "$FIXTURE_HOME/.codex/scripts/heavy-guard.sh"
run_b4push B4PUSH_FULL=1 B4PUSH_SKIP_CLIPPY=1 B4PUSH_SKIP_JS_TEST=1
assert_status "installed guard discovery b4push succeeds" 0 "$RUN_STATUS"
assert_contains "$CASE_DIR/claude-guard.log" 'CALL: -- cargo test --workspace' "Claude guard discovered first"
assert_not_contains "$CASE_DIR/codex-guard.log" 'CALL:' "Codex guard is not selected when Claude guard exists"
assert_contains "$CASE_DIR/cargo.log" 'CALL: cargo test --workspace' "plain cargo test fallback preserved"
assert_not_contains "$CASE_DIR/claude-guard.log" 'cargo test --workspace --doc' "fallback does not add redundant doctest command"

# Codex is selected when the earlier Claude path is absent.
setup_fixture codex-discovery
rm "$FIXTURE_BIN/cargo-nextest"
make_guard "$FIXTURE_HOME/.codex/scripts/heavy-guard.sh"
run_b4push B4PUSH_FULL=1 B4PUSH_SKIP_CLIPPY=1 B4PUSH_SKIP_JS_TEST=1
assert_status "Codex fallback b4push succeeds" 0 "$RUN_STATUS"
assert_contains "$CASE_DIR/codex-guard.log" 'CALL: -- cargo test --workspace' "Codex guard discovered when Claude guard is absent"

# A machine without an installed guard runs all queued commands directly.
setup_fixture missing-guard
rm "$FIXTURE_BIN/cargo-nextest"
run_b4push B4PUSH_FULL=1 B4PUSH_SKIP_CLIPPY=1 B4PUSH_SKIP_JS_TEST=1
assert_status "missing guard b4push succeeds directly" 0 "$RUN_STATUS"
assert_contains "$CASE_DIR/cargo.log" 'CALL: cargo test --workspace' "missing guard executes queued command directly"
assert_not_contains "$CASE_DIR/explicit-guard.log" 'CALL:' "missing guard does not invoke a guard"

# CI bypasses a configured guard while preserving direct execution.
setup_fixture ci-bypass
make_guard "$CASE_DIR/selected-guard"
run_b4push CI=true HEAVY_GUARD="$CASE_DIR/selected-guard" B4PUSH_FULL=1 \
  B4PUSH_SKIP_CLIPPY=1 B4PUSH_SKIP_JS_TEST=1
assert_status "CI bypass b4push succeeds directly" 0 "$RUN_STATUS"
assert_contains "$CASE_DIR/cargo.log" 'CALL: cargo nextest run --workspace' "CI executes the queued command directly"
assert_not_contains "$CASE_DIR/explicit-guard.log" 'CALL:' "CI does not invoke the configured guard"

# A command's non-contention error remains an ordinary failed check.
setup_fixture command-failure
make_guard "$CASE_DIR/selected-guard"
run_b4push HEAVY_GUARD="$CASE_DIR/selected-guard" B4PUSH_FULL=1 \
  B4PUSH_SKIP_CLIPPY=1 B4PUSH_SKIP_JS_TEST=1 \
  MOCK_CARGO_FAIL_MATCH='nextest run --workspace' MOCK_CARGO_FAIL_STATUS=23
assert_status "cargo failure returns b4push failure" 1 "$RUN_STATUS"
assert_contains "$RUN_OUTPUT" '❌ 1 check(s) failed:' "cargo failure appears in failure summary"
assert_not_contains "$RUN_OUTPUT" 'heavy-guard contention' "cargo failure is not described as contention"

# A configured guard that cannot execute is a failed step; do not fall back to
# a discovered guard or launch the command outside the queue.
setup_fixture guard-execution-failure
make_guard "$FIXTURE_HOME/.claude/scripts/heavy-guard.sh"
: > "$CASE_DIR/non-executable-guard"
chmod 644 "$CASE_DIR/non-executable-guard"
run_b4push HEAVY_GUARD="$CASE_DIR/non-executable-guard" B4PUSH_FULL=1 \
  B4PUSH_SKIP_CLIPPY=1 B4PUSH_SKIP_JS_TEST=1
assert_status "unusable explicit guard returns failure" 1 "$RUN_STATUS"
assert_contains "$RUN_OUTPUT" 'check(s) failed:' "guard execution error appears as failure"
assert_not_contains "$CASE_DIR/claude-guard.log" 'CALL:' "bad explicit override does not fall back to installed guard"
assert_not_contains "$CASE_DIR/cargo.log" 'CALL: cargo test --workspace' "bad explicit override never runs a heavy command unguarded"
assert_not_contains "$CASE_DIR/cargo.log" 'CALL: cargo run -p zfb-content' "bad explicit override never runs asset generation unguarded"
assert_not_contains "$RUN_OUTPUT" 'heavy-guard contention' "guard execution error is not contention"

# Exit 75 from the guard means that step never ran. It stays out of the
# assertion/check failure count, makes b4push non-green, and reaches callers.
setup_fixture contention
make_guard "$CASE_DIR/selected-guard"
run_b4push HEAVY_GUARD="$CASE_DIR/selected-guard" B4PUSH_FULL=1 \
  B4PUSH_SKIP_CLIPPY=1 B4PUSH_SKIP_JS_TEST=1 \
  MOCK_GUARD_EXIT_MATCH='cargo nextest run --workspace' MOCK_GUARD_EXIT_STATUS=75
assert_status "guard contention exit 75 is preserved" 75 "$RUN_STATUS"
assert_contains "$RUN_OUTPUT" 'heavy-guard contention: cargo nextest run --workspace was not run (exit 75)' "contention identifies command as not run"
assert_contains "$RUN_OUTPUT" 'heavy step(s) were not run because the heavy-guard queue was contended' "contention summary is explicit"
assert_not_contains "$RUN_OUTPUT" 'check(s) failed:' "contention is not reported as assertion/check failure"
assert_not_contains "$RUN_OUTPUT" 'All checks passed' "contention cannot produce a passing summary"
assert_not_contains "$CASE_DIR/cargo.log" 'CALL: cargo nextest run --workspace' "contended command is never launched"

# The syntax-asset command can pass through the guard and then fail its diff
# check. The diff's status 75 is not guard contention because the command ran.
setup_fixture diff-failure
make_guard "$CASE_DIR/selected-guard"
run_b4push HEAVY_GUARD="$CASE_DIR/selected-guard" B4PUSH_FULL=1 \
  B4PUSH_SKIP_CLIPPY=1 B4PUSH_SKIP_JS_TEST=1 MOCK_GIT_DIFF_STATUS=75
assert_status "post-command diff failure is ordinary failure" 1 "$RUN_STATUS"
assert_contains "$CASE_DIR/explicit-guard.log" 'CALL: -- cargo run -p zfb-content --bin generate_syntax_dump --features generate-syntax-dump' "syntax asset generation runs through guard"
assert_contains "$CASE_DIR/git.log" 'CALL: git diff --exit-code -- crates/zfb-content/assets/syntax-set.packdump' "syntax asset diff remains outside guard"
assert_contains "$RUN_OUTPUT" 'check(s) failed:' "diff failure is in check failure summary"
assert_not_contains "$RUN_OUTPUT" 'heavy-guard contention' "post-command diff status is not contention"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
