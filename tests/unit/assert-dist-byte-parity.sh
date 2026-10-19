#!/bin/sh
# shellcheck shell=sh
#
# tests/unit/assert-dist-byte-parity.sh — offline tests for
# scripts/assert-dist-byte-parity.sh, the raw-byte dist/ comparison that
# scripts/smoke-packed-clean-room.sh Stage 7 uses to prove store-installed
# and linked-directory consumption emit identical bytes (issue #4231).
#
# The positive case alone would pass for a comparator that always exits 0,
# so every way two trees can differ gets its own negative control: one
# changed byte, a missing file, an extra file, a renamed file, a file swapped
# for a symlink, an empty tree, and a --require glob that matches nothing.
#
# Requires: sh, bash, mktemp, sha256sum. No network.
#
# Run:
#   sh tests/unit/assert-dist-byte-parity.sh

set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SELF_DIR/../.." && pwd)
SCRIPT="$REPO_ROOT/scripts/assert-dist-byte-parity.sh"

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# make_dist <dir>: a small dist/ shaped like a zfb build output.
make_dist() {
  mkdir -p "$1/assets" "$1/blog/post"
  printf '<html data-zfb-build="0123456789abcdef"></html>\n' >"$1/index.html"
  printf '<html>post</html>\n' >"$1/blog/post/index.html"
  printf 'x={build:"0123456789abcdef"};\n' >"$1/assets/islands-aaaa1111.js"
  printf 'body{}\n' >"$1/assets/styles-bbbb2222.css"
  printf 'hidden\n' >"$1/.hidden"
}

# expect <want-status> <label> <args...>
expect() {
  want=$1
  label=$2
  shift 2
  set +e
  out=$(bash "$SCRIPT" "$@" 2>&1)
  got=$?
  set -e
  if [ "$got" -eq "$want" ]; then
    pass "$label (exit $got)"
  else
    fail "$label: expected exit $want, got $got — output: $out"
  fi
}

reset_pair() {
  rm -rf "$WORK/a" "$WORK/b"
  make_dist "$WORK/a"
  make_dist "$WORK/b"
}

reset_pair
expect 0 "identical trees pass" --require 'assets/islands-*.js' "$WORK/a" "$WORK/b"

reset_pair
printf 'x={build:"0123456789abcdee"};\n' >"$WORK/b/assets/islands-aaaa1111.js"
expect 1 "one changed byte in the islands root fails" "$WORK/a" "$WORK/b"

reset_pair
printf 'hidden!\n' >"$WORK/b/.hidden"
expect 1 "a changed dotfile fails" "$WORK/a" "$WORK/b"

reset_pair
rm "$WORK/b/blog/post/index.html"
expect 1 "a file missing from the second tree fails" "$WORK/a" "$WORK/b"

reset_pair
printf 'extra\n' >"$WORK/b/assets/islands-chunk-cccc3333.js"
expect 1 "an extra file in the second tree fails" "$WORK/a" "$WORK/b"

reset_pair
mv "$WORK/b/assets/islands-aaaa1111.js" "$WORK/b/assets/islands-dddd4444.js"
expect 1 "same bytes under a different file name fails" "$WORK/a" "$WORK/b"

reset_pair
rm "$WORK/b/assets/styles-bbbb2222.css"
ln -s "$WORK/a/assets/styles-bbbb2222.css" "$WORK/b/assets/styles-bbbb2222.css"
expect 1 "a file replaced by a symlink to identical bytes fails" "$WORK/a" "$WORK/b"

rm -rf "$WORK/empty-a" "$WORK/empty-b"
mkdir -p "$WORK/empty-a" "$WORK/empty-b"
expect 1 "two empty trees fail instead of passing vacuously" "$WORK/empty-a" "$WORK/empty-b"

reset_pair
expect 1 "--require that matches nothing fails" --require 'assets/islands-*.mjs' "$WORK/a" "$WORK/b"

reset_pair
expect 0 "--require matches nested paths" --require 'blog/*/index.html' "$WORK/a" "$WORK/b"

expect 1 "a missing directory fails" "$WORK/a" "$WORK/does-not-exist"
expect 2 "a single operand is a usage error" "$WORK/a"
expect 2 "an unknown flag is a usage error" --bogus "$WORK/a" "$WORK/b"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
