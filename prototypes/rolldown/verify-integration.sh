#!/usr/bin/env bash
# Exact-source native Rolldown integration evidence; no publication or re-pin.
# Default features select the direct-v8 JS host (#4390); the SSR probes run on it.
# Rolldown is the only bundler: there is no esbuild backend or feature-absent build.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${ZFB_PROTOTYPE_TOOLS:?prepared Hono and Playwright tools required}"
mkdir -p target/rolldown-integration-evidence
failed_suites=()
run() {
  local name="$1"
  shift
  printf '\nRunning %s\n' "$name"
  "$@" 2>&1 | tee "target/rolldown-integration-evidence/$name.log"
}
# The zfb CLI rejects both retired backend selectors, so never forward them.
unset ZFB_ROLLDOWN_PROTOTYPE ZFB_BUNDLER
{
  git rev-parse HEAD
  rustc -vV
  node -v
  pnpm --version
  sha256sum Cargo.lock pnpm-lock.yaml
} > target/rolldown-integration-evidence/identity.txt
run format cargo fmt --all --check
cargo tree --locked -p zfb --no-default-features -e normal,build,features > target/rolldown-integration-evidence/no-v8-graph.txt
run no-v8-check cargo check --locked -p zfb --no-default-features --tests
run wind cargo test --locked -p zudo-wind --lib
run preview-tests node --test docs/scripts/__tests__/wind-preview-assets.test.mjs docs/scripts/__tests__/generate-wind-guide-previews.test.mjs
run previews node docs/scripts/wind-preview-assets.mjs --compiler "$PWD/target/debug/zfb" --check
run wind-en node docs/scripts/generate-wind-reference.mjs --locale en --check
run wind-ja node docs/scripts/generate-wind-reference.mjs --locale ja --check
run wasm-docs node scripts/assert-md-wasm-size-docs.mjs
run native-build cargo build --locked -p zfb -p zfb-build -p zfb-islands -p zfb-rolldown-prototype --features zfb-rolldown-prototype/native --bin zfb --example rolldown_production --example rolldown_ssr --example rolldown_browser --example prepared --example plugin_prebundle
sha256sum target/debug/zfb > target/rolldown-integration-evidence/native-binary.txt
cargo tree --locked -p zfb -e normal,build,features > target/rolldown-integration-evidence/native-graph.txt
if ! grep -q rolldown target/rolldown-integration-evidence/native-graph.txt; then
  echo 'Native Rolldown dependency missing from the default zfb graph' >&2
  exit 1
fi
run rolldown-islands cargo test --locked -p zfb-islands --test integration -- --ignored
run rolldown-client cargo test --locked -p zfb --test client_bundling_cross_pipeline real_binary_build_and_dev_cover_client_bundling_contract -- --ignored --exact
# Finish every prepared-contract binary and the independent browser/client
# checks even when one contract fails. The final result remains a failure.
if ! run rolldown-prepared cargo test --locked -p zfb-build --no-fail-fast --test bundler_css_modules --test bundler_workspace_pkg_alias --test bundler_main_fields --test bundler_exact_match_resolution; then
  failed_suites+=(rolldown-prepared)
fi
run rolldown-stage-escape cargo test --locked -p zfb-build --test bundler_root_workspace_stage_escape_audit_armed_regression -- --ignored
run client-library cargo test --locked -p zfb-islands --lib
run prepare node prototypes/rolldown/prepare.mjs
run prepared-browser-ssr node prototypes/rolldown/verify.mjs
run production node prototypes/rolldown/production.mjs
run client-browser node prototypes/rolldown/client-scripts.mjs
git diff --exit-code
test -z "$(git status --porcelain --untracked-files=no)"
if (( ${#failed_suites[@]} )); then
  printf 'Failed integration suites: %s\n' "${failed_suites[*]}" >&2
  exit 1
fi
