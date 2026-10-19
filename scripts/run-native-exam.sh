#!/usr/bin/env bash
set -euo pipefail
lane=${1:?shard-1..4 or doctests}
case "$lane" in shard-[1-4]|doctests) ;; *) exit 2 ;; esac
out="target/native-exam"
mkdir -p "$out"
python3 scripts/native-exam.py metadata "$out/metadata.json"
started=$SECONDS
if [[ "$lane" == doctests ]]; then
  cargo test --locked --workspace --doc -- --list > "$out/full-list.txt" 2>&1
  set +e
  cargo test --locked --workspace --doc -- --format pretty 2>&1 | tee "$out/run.log"
  statuses=("${PIPESTATUS[@]}")
else
  cargo nextest list --locked --workspace --profile ci --run-ignored all --message-format json > "$out/full.json"
  cargo nextest list --locked --workspace --profile ci --partition "hash:${lane#shard-}/4" --message-format json > "$out/selected.json"
  # Never accept a cached report from an earlier run as this shard's evidence.
  rm -f target/nextest/ci/junit.xml
  set +e
  cargo nextest run --locked --workspace --profile ci --partition "hash:${lane#shard-}/4" --no-fail-fast --retries 0 2>&1 | tee "$out/run.log"
  statuses=("${PIPESTATUS[@]}")
fi
set -e
rc=${statuses[0]}
printf '%s\n' "$rc" > "$out/exit-code.txt"
python3 - "$out" "$lane" "$rc" "$((SECONDS - started))" "${statuses[1]}" <<'PY'
import json, pathlib, sys
out, lane, rc, elapsed, tee_rc = sys.argv[1:]
p = pathlib.Path(out)
(p / 'result.json').write_text(json.dumps({'lane': lane, 'rc': int(rc), 'completed': int(tee_rc) == 0}) + '\n')
timings = json.loads((p / 'timings.json').read_text())
timings['inventory_and_tests_seconds'] = int(elapsed)
(p / 'timings.json').write_text(json.dumps(timings) + '\n')
PY
if [[ "$lane" != doctests && -f target/nextest/ci/junit.xml ]]; then
  cp target/nextest/ci/junit.xml "$out/junit.xml"
fi
# The aggregate owns the verdict, independently of the wrapper's green exit.
exit 0
