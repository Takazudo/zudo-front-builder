#!/usr/bin/env bash
# #4172 noyalib 0.0.53 vs 0.0.55 comparison. Manager-only: it compiles.
#
# Builds an isolated detached scratch worktree of SOURCE_SHA, runs the control
# lane at the committed 0.0.53 pin, swaps only the scratch copy to the 0.0.55
# pair, verifies the exact two-entry lock delta before compiling, then runs
# the identical candidate lane. The branch's Cargo.toml / Cargo.lock are never
# touched. Every command's argv, output, exit and duration land in EVIDENCE.
#
# Env (all optional):
#   SOURCE_SHA        commit to evaluate (default: HEAD of this checkout)
#   EVIDENCE          fresh absolute directory outside the repo
#                     (default: $HOME/.cache/zfb-4172-yaml-evidence/<utc stamp>)
#   CARGO_TARGET_DIR  shared target (default: <main checkout>/target)
#   HEAVY_GUARD       guard script (default: $HOME/.claude/scripts/heavy-guard.sh)
#   MIN_FREE_GIB      disk preflight (default: 20)
#   KEEP_SCRATCH=1    keep the scratch worktree for inspection
#   CI_CANDIDATE_BUDGET_LOG  CI wasm-md budget-step log at the candidate pin,
#                     forwarded to `evaluate.py summarize`
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(git -C "$HERE" rev-parse --show-toplevel)
MAIN_ROOT=$(dirname "$(git -C "$REPO" rev-parse --path-format=absolute --git-common-dir)")
SOURCE_SHA=$(git -C "$REPO" rev-parse "${SOURCE_SHA:-HEAD}")
EVIDENCE=${EVIDENCE:-$HOME/.cache/zfb-4172-yaml-evidence/$(date -u +%Y%m%dT%H%M%SZ)}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$MAIN_ROOT/target}
HEAVY_GUARD=${HEAVY_GUARD:-$HOME/.claude/scripts/heavy-guard.sh}
MIN_FREE_GIB=${MIN_FREE_GIB:-20}
SCRATCH=$EVIDENCE/src
EVALUATE=(python3 -I "$HERE/evaluate.py")
CANDIDATE=0.0.55
# The probe's timing should reflect an optimized parser without rebuilding
# the whole dev graph in release; only noyalib's codegen changes.
PROBE_CONFIG='profile.dev.package.noyalib.opt-level=3'

die() { printf 'compare.sh: %s\n' "$*" >&2; exit 2; }

case $EVIDENCE in /*) ;; *) die "EVIDENCE must be absolute" ;; esac
case $EVIDENCE/ in "$MAIN_ROOT"/*) die "EVIDENCE must live outside the repository" ;; esac
[[ -e $EVIDENCE ]] && die "refusing to reuse evidence directory $EVIDENCE"
[[ -n ${CI:-} ]] && die "CI is set; the build's --allow-over-ceiling is refused under CI"
mkdir -p "$EVIDENCE"

# run <dir> <name> <cmd...>: record argv, combined output, exit and seconds.
run() {
  local dir=$1 name=$2
  shift 2
  { date -u +%FT%TZ; printf '%q ' "$@"; printf '\n'; } > "$dir/$name.command"
  local started=$SECONDS
  "$@" > "$dir/$name.log" 2>&1
  local code=$?
  printf '%s\n' "$code" > "$dir/$name.exit"
  printf '%s\n' "$((SECONDS - started))" > "$dir/$name.seconds"
  printf '[%s] %s/%s exit=%s (%ss)\n' "$(date -u +%TZ)" "$(basename "$dir")" "$name" "$code" "$((SECONDS - started))"
  return "$code"
}

guarded() {
  local dir=$1 name=$2
  shift 2
  if [[ $HEAVY_GUARD == none ]]; then
    run "$dir" "$name" "$@"
  else
    run "$dir" "$name" bash "$HEAVY_GUARD" --max-run 3600 --label "4172 $(basename "$dir") $name" -- "$@"
  fi
}

free_bytes() { df -Pk "$1" | awk 'NR == 2 { print $4 * 1024 }'; }

capture_toolchain() {
  local out=$1
  {
    echo "rustc: $(rustc -Vv 2>&1 | tr '\n' ';')"
    echo "cargo: $(cargo -V 2>&1)"
    echo "active-toolchain: $(rustup show active-toolchain 2>&1)"
    echo "wasm32: $(rustup target list --installed 2>&1 | grep -c '^wasm32-unknown-unknown$')"
    echo "wasm-bindgen: $(wasm-bindgen --version 2>&1)"
    echo "node: $(node --version 2>&1)"
    echo "pnpm: $(pnpm --version 2>&1)"
    echo "cargo-deny: $(cargo deny --version 2>&1)"
    echo "freeBytes: $(free_bytes "$CARGO_TARGET_DIR")"
  } > "$out"
}

preflight() {
  {
    echo "sourceSha: $SOURCE_SHA"
    echo "cargoTargetDir: $CARGO_TARGET_DIR"
    echo "heavyGuard: $HEAVY_GUARD"
    echo "uname: $(uname -a)"
    command -v sw_vers > /dev/null && echo "macos: $(sw_vers -productVersion)"
    echo "python: $(python3 --version 2>&1)"
    echo "git: $(git --version)"
  } > "$EVIDENCE/environment.txt"
  (cd "$REPO" && capture_toolchain "$EVIDENCE/toolchain-start.txt")
  cat "$EVIDENCE/toolchain-start.txt"
  local ok=0
  [[ $HEAVY_GUARD == none || -f $HEAVY_GUARD ]] || { echo "missing heavy guard $HEAVY_GUARD"; ok=1; }
  grep -q '^wasm32: 1$' "$EVIDENCE/toolchain-start.txt" || { echo "wasm32-unknown-unknown target missing"; ok=1; }
  grep -q '^wasm-bindgen: wasm-bindgen 0.2.121$' "$EVIDENCE/toolchain-start.txt" || { echo "wasm-bindgen 0.2.121 required"; ok=1; }
  local free
  free=$(free_bytes "$CARGO_TARGET_DIR")
  if (( free < MIN_FREE_GIB * 1024 * 1024 * 1024 )); then
    echo "disk: $free free bytes < ${MIN_FREE_GIB} GiB"
    ok=1
  fi
  return "$ok"
}

write_environment_json() {
  python3 -I - "$EVIDENCE" <<'PY'
import json, sys
from pathlib import Path
evidence = Path(sys.argv[1])
def fields(name):
    path = evidence / name
    if not path.is_file():
        return None
    return dict(line.split(': ', 1) for line in path.read_text().splitlines() if ': ' in line)
(evidence / 'environment.json').write_text(json.dumps({
    'host': fields('environment.txt'),
    'toolchainStart': fields('toolchain-start.txt'),
    'toolchainControl': fields('control/toolchain.txt'),
    'toolchainCandidate': fields('candidate/toolchain.txt'),
    'toolchainEnd': fields('toolchain-end.txt'),
}, indent=2) + '\n')
PY
}

cleanup() {
  write_environment_json
  if [[ ${KEEP_SCRATCH:-0} != 1 && -d $SCRATCH ]]; then
    git -C "$MAIN_ROOT" worktree remove --force "$SCRATCH" && echo "removed scratch worktree $SCRATCH"
  fi
}

lane() {
  local label=$1 dir=$EVIDENCE/$1
  mkdir -p "$dir/pack"
  capture_toolchain "$dir/toolchain.txt"
  run "$dir" tree-normal cargo tree --locked -e normal -i noyalib
  run "$dir" tree-features cargo tree --locked -e features -i noyalib
  guarded "$dir" harness cargo test --locked -p zfb-content --test yaml_differential_harness
  guarded "$dir" budgets cargo test --locked -p zfb-content --test frontmatter_resource_budgets
  guarded "$dir" error-messages cargo test --locked -p zfb-content --test error_messages
  guarded "$dir" content cargo test --locked -p zfb-content
  guarded "$dir" md-api-ast cargo test --locked -p zfb-md-wasm --test api --test parse_to_ast
  guarded "$dir" diagnostics cargo test --locked -p zfb --no-default-features --lib diagnostics::tests
  if guarded "$dir" probe-build cargo build --locked -p zfb-content --example yaml_4172_probe --config "$PROBE_CONFIG"; then
    cp "$CARGO_TARGET_DIR/debug/examples/yaml_4172_probe" "$dir/probe-bin"
    "$dir/probe-bin" observe > "$dir/probe-observe.jsonl" 2> "$dir/probe-observe.log"
    printf '%s\n' "$?" > "$dir/probe-observe.exit"
    guarded "$dir" flow "${EVALUATE[@]}" flow --probe "$dir/probe-bin" --out "$dir/flow.json"
  fi
  if guarded "$dir" wasm-build pnpm --filter @takazudo/zfb-md-wasm build --allow-over-ceiling; then
    cp -R crates/zfb-md-wasm/npm/dist "$dir/dist"
    guarded "$dir" wasm-npm-test pnpm --filter @takazudo/zfb-md-wasm test
    run "$dir" wasm-exports bash scripts/assert-zfb-md-wasm-exports.sh crates/zfb-md-wasm/npm/dist
    run "$dir" wasm-budget-strict node scripts/assert-zfb-md-wasm-budgets.mjs \
      --build-log "$dir/wasm-build.log" --dist crates/zfb-md-wasm/npm/dist
    if run "$dir" wasm-pack pnpm -C crates/zfb-md-wasm/npm pack --pack-destination "$dir/pack"; then
      mv "$dir"/pack/*.tgz "$dir/pack/zfb-md-wasm.tgz"
    fi
    # --update-manifest skips the CI-manifest comparison so this run checks
    # host bytes against the unchanged ceilings; the scratch manifest it
    # rewrites is restored right after and never leaves the scratch copy.
    run "$dir" wasm-budget-ceilings node scripts/assert-zfb-md-wasm-budgets.mjs \
      --build-log "$dir/wasm-build.log" --dist crates/zfb-md-wasm/npm/dist \
      --tarball "$dir/pack/zfb-md-wasm.tgz" --update-manifest
    git checkout -- crates/zfb-md-wasm/shipped-sizes.json
    run "$dir" sizes "${EVALUATE[@]}" sizes --label "$label" --budget-log "$dir/wasm-budget-ceilings.log" \
      --dist "$dir/dist" --tarball "$dir/pack/zfb-md-wasm.tgz" --out "$dir/sizes.json"
  fi
  printf '%s\n' "$(free_bytes "$CARGO_TARGET_DIR")" > "$dir/free-bytes-after"
}

summarize() {
  local args=(--evidence "$EVIDENCE" --out "$EVIDENCE/report.json")
  [[ -n ${CI_CANDIDATE_BUDGET_LOG:-} ]] && args+=(--ci-candidate-budget-log "$CI_CANDIDATE_BUDGET_LOG")
  write_environment_json
  "${EVALUATE[@]}" summarize "${args[@]}" | tee "$EVIDENCE/decision.txt"
  echo "evidence: $EVIDENCE"
}

echo "evidence: $EVIDENCE"
run "$EVIDENCE" preflight preflight || { cat "$EVIDENCE/preflight.log"; summarize; exit 1; }
run "$EVIDENCE" registry "${EVALUATE[@]}" registry --out "$EVIDENCE/registry.json" || { summarize; exit 1; }
run "$EVIDENCE" scratch-worktree git -C "$REPO" worktree add --detach "$SCRATCH" "$SOURCE_SHA" || { summarize; exit 1; }
trap cleanup EXIT
cd "$SCRATCH" || die "cannot enter $SCRATCH"
run "$EVIDENCE" pnpm-install pnpm install --frozen-lockfile --prefer-offline || { summarize; exit 1; }
git diff --exit-code --quiet -- pnpm-lock.yaml || die "pnpm install changed pnpm-lock.yaml"
cp "$HERE/probe.rs" crates/zfb-content/examples/yaml_4172_probe.rs
cp Cargo.toml "$EVIDENCE/control-Cargo.toml"
cp Cargo.lock "$EVIDENCE/control-Cargo.lock"
ZFB_RELEASE_VERSION=$(node -p "require('./crates/zfb-md-wasm/npm/package.json').version")
export ZFB_RELEASE_VERSION
echo "ZFB_RELEASE_VERSION=$ZFB_RELEASE_VERSION" >> "$EVIDENCE/environment.txt"

lane control

run "$EVIDENCE" pin "${EVALUATE[@]}" pin --checkout "$SCRATCH" || { summarize; exit 1; }
# Resolution only (no compile): moves the alias and, through its exact pin, the core.
run "$EVIDENCE" candidate-resolve cargo update -p noyalib-serde-yaml --precise "$CANDIDATE" || { summarize; exit 1; }
cp Cargo.toml "$EVIDENCE/candidate-Cargo.toml"
cp Cargo.lock "$EVIDENCE/candidate-Cargo.lock"
run "$EVIDENCE" lock-check "${EVALUATE[@]}" lock --control "$EVIDENCE/control-Cargo.lock" \
  --candidate "$EVIDENCE/candidate-Cargo.lock" || { summarize; exit 1; }

lane candidate

capture_toolchain "$EVIDENCE/toolchain-end.txt"
summarize
