#!/usr/bin/env bash
# Invoke at each pin under the manager guard. Missing tools are NOT RUN (127).
set -uo pipefail
cd "$(dirname "$0")/../.."
: "${YAML_EVIDENCE:?set external evidence directory}"
: "${YAML_PIN_LABEL:?baseline or candidate}"
status=0
run() {
  local name=$1 result=0
  shift
  "$@" 2>&1 | tee "$YAML_EVIDENCE/$YAML_PIN_LABEL-$name.log"
  local statuses=("${PIPESTATUS[@]}")
  result=${statuses[0]}
  printf '%s\n' "$result" > "$YAML_EVIDENCE/$YAML_PIN_LABEL-$name.exit"
  if (( result != 0 || statuses[1] != 0 )); then status=1; fi
}
run normal-tree cargo tree --locked -e normal -i noyalib
run feature-tree cargo tree --locked -e features -i noyalib
run license cargo deny list
run deny cargo deny check
exit "$status"
