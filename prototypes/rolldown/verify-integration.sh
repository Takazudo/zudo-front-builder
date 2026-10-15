#!/usr/bin/env bash
# Exact-source experimental integration evidence; no publication or re-pin.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${ZFB_ESBUILD_BIN:?absolute pinned esbuild executable required}"
: "${ZFB_PROTOTYPE_TOOLS:?prepared Hono and Playwright tools required}"
mkdir -p target/rolldown-integration-evidence
run() {
  local name="$1"
  shift
  printf '\nRunning %s\n' "$name"
  "$@" 2>&1 | tee "target/rolldown-integration-evidence/$name.log"
}
unset ZFB_ROLLDOWN_PROTOTYPE
{
  git rev-parse HEAD
  rustc -vV
  node -v
  pnpm --version
  sha256sum Cargo.lock pnpm-lock.yaml "$ZFB_ESBUILD_BIN"
} > target/rolldown-integration-evidence/identity.txt
run format cargo fmt --all --check
run feature-absent-build cargo build --locked -p zfb --bin zfb
cp target/debug/zfb target/rolldown-integration-evidence/zfb-feature-absent
sha256sum target/rolldown-integration-evidence/zfb-feature-absent > target/rolldown-integration-evidence/default-binary.txt
run feature-absent-islands cargo test --locked -p zfb-islands --test integration -- --ignored
run feature-absent-client cargo test --locked -p zfb --test client_bundling_cross_pipeline real_binary_build_and_dev_cover_client_bundling_contract -- --ignored --exact
cargo tree --locked -p zfb -e normal,build,features > target/rolldown-integration-evidence/default-graph.txt
cargo tree --locked -p zfb --no-default-features -e normal,build,features > target/rolldown-integration-evidence/no-v8-graph.txt
if grep -q rolldown target/rolldown-integration-evidence/{default,no-v8}-graph.txt; then
  echo 'Unexpected native dependency in ordinary graph' >&2
  exit 1
fi
run no-v8-check cargo check --locked -p zfb --no-default-features --tests
run wind cargo test --locked -p zudo-wind --lib
run preview-tests node --test docs/scripts/__tests__/wind-preview-assets.test.mjs docs/scripts/__tests__/generate-wind-guide-previews.test.mjs
run previews node docs/scripts/wind-preview-assets.mjs --compiler "$PWD/target/debug/zfb" --check
run wind-en node docs/scripts/generate-wind-reference.mjs --locale en --check
run wind-ja node docs/scripts/generate-wind-reference.mjs --locale ja --check
run wasm-docs node scripts/assert-md-wasm-size-docs.mjs
run native-build cargo build --locked -p zfb -p zfb-build -p zfb-islands -p zfb-rolldown-prototype --features zfb/rolldown-prototype,zfb-build/rolldown-prototype,zfb-islands/rolldown-prototype,zfb-rolldown-prototype/native --bin zfb --example rolldown_production --example rolldown_ssr --example rolldown_browser --example prepared --example plugin_prebundle
sha256sum target/debug/zfb > target/rolldown-integration-evidence/native-binary.txt
cargo tree --locked -p zfb --features rolldown-prototype -e normal,build,features > target/rolldown-integration-evidence/native-graph.txt
for backend in esbuild rolldown; do
  unset ZFB_ROLLDOWN_PROTOTYPE
  if [[ "$backend" == rolldown ]]; then export ZFB_ROLLDOWN_PROTOTYPE=1; fi
  run "$backend-islands" cargo test --locked -p zfb-islands --features rolldown-prototype --test integration -- --ignored
  run "$backend-client" cargo test --locked -p zfb --features rolldown-prototype --test client_bundling_cross_pipeline real_binary_build_and_dev_cover_client_bundling_contract -- --ignored --exact
  run "$backend-prepared" cargo test --locked -p zfb-build --features rolldown-prototype --test bundler_css_modules --test bundler_workspace_pkg_alias --test bundler_main_fields --test bundler_exact_match_resolution
  run "$backend-stage-escape" cargo test --locked -p zfb-build --features rolldown-prototype --test bundler_root_workspace_stage_escape_audit_armed_regression -- --ignored
done
unset ZFB_ROLLDOWN_PROTOTYPE
run client-library cargo test --locked -p zfb-islands --features rolldown-prototype --lib
run prepare node prototypes/rolldown/prepare.mjs
run prepared-browser-ssr node prototypes/rolldown/verify.mjs
run production node prototypes/rolldown/production.mjs
run client-browser node prototypes/rolldown/client-scripts.mjs
git diff --exit-code
test -z "$(git status --porcelain --untracked-files=no)"
