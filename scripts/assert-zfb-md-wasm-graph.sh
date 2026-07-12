#!/usr/bin/env bash
set -euo pipefail

# scripts/assert-zfb-md-wasm-graph.sh — zfb#1576 (epic zfb#1572).
#
# Asserts that the wasm32-unknown-unknown dependency graph of the
# `zfb-md-wasm` crate stays free of native-only crates that would break the
# wasm build (or silently re-enter it through a future dependency edit):
#
#   - onig_sys   — C oniguruma; excluded via the syntect-fancy backend
#                  selection (zfb#1573) forwarded through zfb-render's
#                  syntect-fancy feature (zfb#1576).
#   - deno_core  — V8 embed; excluded via zfb-render `default-features =
#                  false` (the `embed_v8` gate).
#   - tokio      — deno_core's event loop; same gate as deno_core.
#
# `cargo tree --target …` resolves the graph without building (the wasm
# toolchain target does not need to be installed) — only NORMAL edges are
# checked (`-e normal`): dev-deps never ship in the cdylib and build-deps
# compile for the HOST, not for wasm.
#
# Usage: scripts/assert-zfb-md-wasm-graph.sh
# Runs from any cwd; exits non-zero (with the offending lines) on violation.
# Wired into CI by the epic's wave-4 CI sub-issue (zfb#1579); until then run
# it manually alongside `cargo check --target wasm32-unknown-unknown -p
# zfb-md-wasm`.

cd "$(dirname "$0")/.."

tree="$(cargo tree --target wasm32-unknown-unknown -p zfb-md-wasm -e normal)"

# cargo tree prints one crate per line as "<box-drawing><space><name> v<semver>";
# match the exact package names so e.g. `tokio-util` could not false-positive.
if offending="$(grep -E ' (onig_sys|deno_core|tokio) v[0-9]' <<<"$tree")"; then
  echo "ERROR: forbidden native-only crate(s) in the zfb-md-wasm wasm graph:" >&2
  echo "$offending" >&2
  exit 1
fi

echo "OK: zfb-md-wasm wasm32-unknown-unknown graph contains no onig_sys / deno_core / tokio"
