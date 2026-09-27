#!/usr/bin/env bash
set -euo pipefail

# zfb#2454: inspect each wasm-bindgen glue module after the production build.
# The TypeScript entry tests cover wrapper values; this gate independently
# proves the cdylib-generated callable exports match the feature matrix and
# each resource directory is closed to exactly four generated files.

dist="${1:-}"
if [[ -z "$dist" || "$#" -ne 1 ]]; then
  echo "usage: $0 <dist-directory>" >&2
  exit 2
fi

expected_files() {
  local stem="$1" workerd="$2"
  local files=(
    "${stem}_bg.wasm"
    "${stem}_bg.wasm.d.ts"
    "${stem}_glue.zfb-resource.d.mts"
    "${stem}_glue.zfb-resource.mjs"
  )
  if [[ "$workerd" == "1" ]]; then
    files+=(
      "${stem}_glue.zfb-factory.d.mts"
      "${stem}_glue.zfb-factory.mjs"
    )
  fi
  printf '%s\n' "${files[@]}" | sort
}

assert_artifact() {
  local label="$1" dir="$2" stem="$3" workerd="$4"
  shift 4
  local path="$dist/$dir" declaration actual expected
  [[ -d "$path" ]] || { echo "ERROR: missing ${label} resource directory: $path" >&2; exit 1; }
  actual="$(find "$path" -mindepth 1 -maxdepth 1 -exec basename {} \; | sort)"
  expected="$(expected_files "$stem" "$workerd")"
  if [[ "$actual" != "$expected" ]]; then
    echo "ERROR: ${label} resource set is not closed" >&2
    echo "expected:" >&2; echo "$expected" >&2
    echo "received:" >&2; echo "$actual" >&2
    exit 1
  fi

  declaration="$path/${stem}_glue.zfb-resource.d.mts"
  [[ -s "$declaration" ]] || { echo "ERROR: missing generated glue declaration: $declaration" >&2; exit 1; }
  for name in initSync version __forceTrapForTests "$@"; do
    # wasm-bindgen emits named declarations in the .d.mts footer even though
    # the .mjs implementation exports them from a generated export object.
    if ! grep -Eq "^export function ${name}[[:space:]]*\\(" "$declaration"; then
      echo "ERROR: ${label} glue is missing export ${name}" >&2
      exit 1
    fi
  done
  for name in compile renderHtml parseToAst highlightCode; do
    local should_have=0
    for expected_name in "$@"; do
      [[ "$expected_name" == "$name" ]] && should_have=1
    done
    if [[ "$should_have" -eq 0 ]] && grep -Eq "^export function ${name}[[:space:]]*\\(" "$declaration"; then
      echo "ERROR: ${label} glue unexpectedly exports ${name}" >&2
      exit 1
    fi
  done
  if [[ "$workerd" == "1" ]]; then
    local factory="$path/${stem}_glue.zfb-factory.mjs" factory_declaration="$path/${stem}_glue.zfb-factory.d.mts" export_count
    [[ -s "$factory" ]] || { echo "ERROR: missing generated factory: $factory" >&2; exit 1; }
    [[ -s "$factory_declaration" ]] || { echo "ERROR: missing generated factory declaration: $factory_declaration" >&2; exit 1; }
    if ! grep -Eq '^export function createGlue[[:space:]]*\(' "$factory"; then
      echo "ERROR: ${label} factory does not export createGlue" >&2
      exit 1
    fi
    export_count="$(grep -Eo '(^|[;[:space:]])export([[:space:]]|\{|\*)' "$factory" | wc -l | tr -d '[:space:]' || true)"
    if [[ "$export_count" -ne 1 ]]; then
      echo "ERROR: ${label} factory must export only createGlue" >&2
      exit 1
    fi
    if ! grep -Eq '^export function createGlue[[:space:]]*\(' "$factory_declaration"; then
      echo "ERROR: ${label} factory declaration is missing createGlue" >&2
      exit 1
    fi
  fi
  echo "OK: ${label} generated exports and closed resource set"
}

assert_artifact "default (.)" wasm zfb_md_wasm 0 compile renderHtml parseToAst highlightCode
assert_artifact "highlight (./highlight)" wasm-highlight zfb_md_wasm_highlight 1 highlightCode
assert_artifact "render (./render)" wasm-render zfb_md_wasm_render 0 renderHtml
assert_artifact "parse (./parse)" wasm-parse zfb_md_wasm_parse 1 parseToAst
