#!/usr/bin/env bash
set -euo pipefail

# scripts/assert-zfb-md-wasm-graph.sh — zfb#2454.
#
# Check one zfb-md-wasm feature surface in an isolated target-specific Cargo
# graph. `cargo tree -p` is deliberately used for every invocation: a
# workspace-wide tree can unify compiler features from a sibling member and
# make an incomplete edge look valid.
#
# Usage:
#   scripts/assert-zfb-md-wasm-graph.sh [default|none|highlight|render|parse|pipeline]
#   scripts/assert-zfb-md-wasm-graph.sh --self-test
#
# With no argument, all six supported health configurations are checked. The
# `none` configuration is the still-supported bare --no-default-features
# surface; it intentionally has no zfb-content dependency.

cd "$(dirname "$0")/.."

TARGET="wasm32-unknown-unknown"
PACKAGE="zfb-md-wasm"

usage() {
  echo "usage: $0 [default|none|highlight|render|parse|pipeline]" >&2
  exit 2
}

config_args() {
  case "$1" in
    default) ;;
    none) printf '%s\n' --no-default-features ;;
    highlight | render | parse)
      printf '%s\n' --no-default-features --features "$1"
      ;;
    pipeline) printf '%s\n' --no-default-features --features pipeline ;;
    *) usage ;;
  esac
}

assert_config_args() {
  local expected
  expected="$(printf '%s\n' --no-default-features --features pipeline)"
  if [[ "$(config_args pipeline)" != "$expected" ]]; then
    echo 'ERROR: pipeline config must pass --no-default-features --features pipeline' >&2
    exit 1
  fi
  expected="$(printf '%s\n' --no-default-features --features highlight)"
  if [[ "$(config_args highlight)" != "$expected" ]]; then
    echo 'ERROR: highlight config arguments drifted' >&2
    exit 1
  fi
  echo 'OK: feature configuration argument self-test'
}

has_package() {
  local tree="$1" package="$2"
  grep -Eq "(^|[[:space:]])${package//./\\.} v[0-9]" <<<"$tree"
}

assert_present() {
  local config="$1" tree="$2" package="$3"
  if ! has_package "$tree" "$package"; then
    echo "ERROR: ${config} graph is missing required package '${package}'" >&2
    exit 1
  fi
}

assert_absent() {
  local config="$1" tree="$2" pattern="$3"
  local offending
  offending="$(grep -E "(^|[[:space:]])(${pattern}) v[0-9]" <<<"$tree" || true)"
  if [[ -n "$offending" ]]; then
    echo "ERROR: ${config} graph contains forbidden package(s) matching '${pattern}':" >&2
    echo "$offending" >&2
    exit 1
  fi
}

assert_graph() {
  local config="$1"
  shift
  local -a args=("$@")
  local tree

  echo "== cargo tree: zfb-md-wasm ${config} (${args[*]:-default features}) =="
  # `--prefix none` makes package-name matching independent of box-drawing
  # characters and keeps the captured graph useful in CI diagnostics.
  tree="$(cargo tree --target "$TARGET" -p "$PACKAGE" -e normal --prefix none "${args[@]}")"

  assert_present "$config" "$tree" "$PACKAGE"
  assert_absent "$config" "$tree" 'onig(_sys)?'
  assert_absent "$config" "$tree" 'tokio'
  assert_absent "$config" "$tree" 'deno(_[[:alnum:]_-]+)?'
  assert_absent "$config" "$tree" '(rusty_)?v8'

  case "$config" in
    default | pipeline)
      for package in zfb-render swc_core swc_ecma_parser swc_ecma_transforms_base swc_ecma_codegen syntect; do
        assert_present "$config" "$tree" "$package"
      done
      # The actual wasm-bindgen export inventory is checked after each
      # production build; this graph gate proves the compiler side is present.
      ;;
    highlight | render | parse)
      assert_present "$config" "$tree" syntect
      # Any SWC package is forbidden, not just the umbrella crate. This avoids
      # accepting a graph that replaced swc_core with standalone SWC crates.
      assert_absent "$config" "$tree" 'swc(_[[:alnum:]_-]+)?'
      assert_absent "$config" "$tree" 'zfb-render'
      ;;
    none)
      for package in zfb-content zfb-render syntect; do
        assert_absent "$config" "$tree" "$package"
      done
      assert_absent "$config" "$tree" 'swc(_[[:alnum:]_-]+)?'
      ;;
    *) usage ;;
  esac

  echo "OK: ${config} graph satisfies the locked wasm dependency contract"
}

assert_compiler_boundaries() {
  local content_tree render_features

  echo "== isolated compiler-off zfb-content graph =="
  content_tree="$(cargo tree --target "$TARGET" -p zfb-content -e normal --prefix none \
    --no-default-features --features syntect-fancy)"
  assert_present "zfb-content compiler-off" "$content_tree" zfb-content
  assert_present "zfb-content compiler-off" "$content_tree" syntect
  assert_absent "zfb-content compiler-off" "$content_tree" 'swc(_[[:alnum:]_-]+)?'
  assert_absent "zfb-content compiler-off" "$content_tree" 'zfb-render'
  assert_absent "zfb-content compiler-off" "$content_tree" 'onig(_sys)?'
  assert_absent "zfb-content compiler-off" "$content_tree" 'tokio'
  assert_absent "zfb-content compiler-off" "$content_tree" 'deno(_[[:alnum:]_-]+)?'
  assert_absent "zfb-content compiler-off" "$content_tree" '(rusty_)?v8'

  echo "== isolated zfb-render compiler edge =="
  # Feature edges are inspected in a package-only invocation. The compiler
  # edge must be present while the dependency's default feature must not be
  # re-enabled by Cargo's workspace resolver.
  render_features="$(cargo tree --target "$TARGET" -p zfb-render -e features --prefix none \
    --no-default-features --features syntect-fancy)"
  if ! grep -Fq 'zfb-content feature "compiler"' <<<"$render_features"; then
    echo 'ERROR: isolated zfb-render graph does not request zfb-content/compiler' >&2
    exit 1
  fi
  if grep -Fq 'zfb-content feature "default"' <<<"$render_features"; then
    echo 'ERROR: isolated zfb-render graph re-enabled zfb-content/default' >&2
    exit 1
  fi
  echo 'OK: zfb-render edge is default-features=false with zfb-content/compiler'
}

run_config() {
  case "$1" in
    default) assert_graph default ;;
    none) assert_graph none --no-default-features ;;
    highlight) assert_graph highlight --no-default-features --features highlight ;;
    render) assert_graph render --no-default-features --features render ;;
    parse) assert_graph parse --no-default-features --features parse ;;
    pipeline) assert_graph pipeline --no-default-features --features pipeline ;;
    *) usage ;;
  esac
}

main() {
  if [[ "$#" -eq 1 && "$1" == "--self-test" ]]; then
    assert_config_args
    return
  fi
  if [[ "$#" -gt 1 ]]; then
    usage
  fi

  if [[ "$#" -eq 0 ]]; then
    assert_config_args
    for config in default none highlight render parse pipeline; do
      run_config "$config"
    done
    assert_compiler_boundaries
    return
  fi

  run_config "$1"
  if [[ "$1" == default ]]; then
    assert_compiler_boundaries
  fi
}

main "$@"
