#!/usr/bin/env bash
set -euo pipefail
# Dedicated graph: never build this cdylib with --workspace (which enables V8).
target="${1:?usage: build-css-node.sh TARGET [cargo|cross]}"
builder="${2:-cargo}"
case "$target" in
  aarch64-apple-darwin) carrier=darwin-arm64; library=libzfb_css_node.dylib ;;
  x86_64-apple-darwin) carrier=darwin-x64; library=libzfb_css_node.dylib ;;
  aarch64-unknown-linux-gnu) carrier=linux-arm64-gnu; library=libzfb_css_node.so ;;
  x86_64-unknown-linux-gnu) carrier=linux-x64-gnu; library=libzfb_css_node.so ;;
  x86_64-pc-windows-msvc) carrier=win32-x64-msvc; library=zfb_css_node.dll ;;
  *) echo "Unsupported CSS addon target: $target" >&2; exit 2 ;;
esac
"$builder" rustc -p zfb-css-node --lib --no-default-features --release --target "$target" --crate-type cdylib
target_dir=$(cargo metadata --no-deps --format-version 1 | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>process.stdout.write(JSON.parse(s).target_directory))')
artifact="$target_dir/$target/release/$library"
if [[ "$target" == *linux* ]]; then
  maximum=$(objdump -T "$artifact" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -V | tail -1)
  [[ -n "$maximum" ]] || { echo 'Cannot inspect addon glibc symbols' >&2; exit 1; }
  [[ $(printf '%s\n' "$maximum" GLIBC_2.34 | sort -V | tail -1) == GLIBC_2.34 ]] || {
    echo "CSS addon requires $maximum, exceeds GLIBC_2.34" >&2; exit 1;
  }
fi
cp "$artifact" "packages/zfb-$carrier/zfb-css.node"
chmod 0644 "packages/zfb-$carrier/zfb-css.node"
