#!/usr/bin/env bash
# Manager invokes this whole script through heavy-guard, never in a child lane.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${YAML_EVIDENCE:?set an absolute fresh evidence directory outside this checkout}"
: "${CARGO_TARGET_DIR:?manager supplies one shared task target, never a child target}"
helper=research/4049-yaml-evaluation/evaluate.py
python3 "$helper" snapshot --evidence "$YAML_EVIDENCE"
restore() {
  python3 "$helper" restore --evidence "$YAML_EVIDENCE"
  python3 "$helper" verify --evidence "$YAML_EVIDENCE"
}
trap restore EXIT
run() {
  local name=$1
  shift
  local result=0
  { date -u +%FT%TZ; printf '%q ' "$@"; printf '\n'; } > "$YAML_EVIDENCE/$name.command"
  set +e
  "$@" 2>&1 | tee "$YAML_EVIDENCE/$name.log"
  local statuses=("${PIPESTATUS[@]}")
  set -e
  result=${statuses[0]}
  printf '%s\n' "$result" > "$YAML_EVIDENCE/$name.exit"
  if (( statuses[1] != 0 )); then return "${statuses[1]}"; fi
  return "$result"
}
run baseline-normal-tree cargo tree --locked -e normal -i noyalib
run baseline-feature-tree cargo tree --locked -e features -i noyalib
python3 "$helper" registry --evidence "$YAML_EVIDENCE" > "$YAML_EVIDENCE/start-registry.json"
python3 "$helper" pin --evidence "$YAML_EVIDENCE"
run candidate-resolve cargo update -p noyalib-serde-yaml --precise "$(python3 -c 'import json; print(json.load(open("research/4049-yaml-evaluation/provenance.json"))["candidate"])')"
run candidate-lock python3 "$helper" lock --evidence "$YAML_EVIDENCE"
cp Cargo.lock "$YAML_EVIDENCE/candidate-Cargo.lock"
run candidate-harness cargo test --locked -p zfb-content --test yaml_differential_harness
run candidate-content cargo test --locked -p zfb-content
run candidate-md-pins cargo test --locked -p zfb-md-wasm --test api --test parse_to_ast
run candidate-diagnostics cargo test --locked -p zfb --no-default-features --lib diagnostics::tests
run candidate-content-check cargo check --locked -p zfb-content
run candidate-md-check cargo check --locked -p zfb-md-wasm
run candidate-cli-check cargo check --locked -p zfb --no-default-features
run candidate-normal-tree cargo tree --locked -e normal -i noyalib
run candidate-feature-tree cargo tree --locked -e features -i noyalib
printf '%s\n' 'P1 commands passed; dependency/license audit remains separate and required before Phase 2.'
