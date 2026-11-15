#!/usr/bin/env bash
set -euo pipefail

# scripts/select-md-wasm-rust-toolchain.sh
#
# Install the exact Rust toolchain named by crates/zfb-md-wasm/rust-toolchain-pin
# (plus the wasm32 target) and select it for the rest of the job through
# RUSTUP_TOOLCHAIN, which overrides rust-toolchain.toml's floating `stable`.
#
# health.yml's wasm-md job and release.yml's build-wasm-md job both compile
# with this pin, not floating stable. A floating `stable` let v6.1.0 measure
# on rustc 1.99.1 and build on 1.100.0 hours later. The selected rustc is
# checked below. shipped-sizes.json measuredWithRustc is a historical label
# and does not have to move when the pin changes.
#
# Only the components passed as arguments are installed: a later step that
# needs rustfmt or clippy must ask for it here.
#
# Usage: scripts/select-md-wasm-rust-toolchain.sh [extra-component ...]
# Writes RUSTUP_TOOLCHAIN to $GITHUB_ENV when that is set.

cd "$(dirname "$0")/.."

PIN_FILE=crates/zfb-md-wasm/rust-toolchain-pin
pin="$(tr -d '[:space:]' < "$PIN_FILE")"
if [[ ! "$pin" =~ ^1\.[0-9]+\.[0-9]+$ ]]; then
  echo "::error::$PIN_FILE must hold an exact rustc version like 1.100.0, got '$pin'" >&2
  exit 1
fi

components=()
for c in "$@"; do components+=(--component "$c"); done

rustup toolchain install "$pin" --profile minimal --no-self-update --target wasm32-unknown-unknown ${components[@]+"${components[@]}"}

actual="$(RUSTUP_TOOLCHAIN="$pin" rustc --version)"
case "$actual" in
  "rustc $pin "*) ;;
  *)
    echo "::error::expected rustc $pin from $PIN_FILE, got: $actual" >&2
    exit 1
    ;;
esac
echo "md-wasm Rust toolchain: $actual"

if [[ -n "${GITHUB_ENV:-}" ]]; then
  echo "RUSTUP_TOOLCHAIN=$pin" >> "$GITHUB_ENV"
fi
