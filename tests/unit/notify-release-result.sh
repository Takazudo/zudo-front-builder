#!/usr/bin/env bash
# The offline CI suite invokes every test with sh. Re-exec for Bash syntax.
if [ "${ZFB_NOTIFY_TEST_BASH:-}" != 1 ]; then
  export ZFB_NOTIFY_TEST_BASH=1
  exec bash "$0" "$@"
fi
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/curl" <<'CURL'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$SINK"
CURL
chmod +x "$tmp/curl"
export PATH="$tmp:$PATH" SINK="$tmp/sink"
export IFTTT_PROD_NOTIFY=https://fake.invalid/hook RAW_TAG=v5.2.1 GITHUB_SHA_VAL=abcdef123456
export SERVER_URL=https://github.com REPO=owner/repo RUN_ID=123 DRY_RUN=false
export RELEASE_CONTEXT_RESULT=success DETECT_MAC_LOCAL_RESULT=success PREPARE_MATRIX_RESULT=success
export BUILD_RESULT=success BUILD_WASM_MD_RESULT=success RELEASE_ASSETS_RESULT=success PUBLISH_RESULT=success SMOKE_CLEAN_ROOM_RESULT=success

run_case() {
  rm -f "$SINK"
  bash "$ROOT/scripts/notify-release-result.sh" > "$tmp/output"
}
run_case
[[ ! -e "$SINK" ]] || { echo 'successful release notified' >&2; exit 1; }
for pair in \
  RELEASE_CONTEXT_RESULT:release-context DETECT_MAC_LOCAL_RESULT:detect-mac-local \
  PREPARE_MATRIX_RESULT:prepare-matrix BUILD_RESULT:build BUILD_WASM_MD_RESULT:build-wasm-md \
  RELEASE_ASSETS_RESULT:release-assets PUBLISH_RESULT:publish SMOKE_CLEAN_ROOM_RESULT:smoke-clean-room; do
  variable=${pair%%:*}
  job=${pair#*:}
  for result in failure cancelled; do
    export "$variable=$result"
    run_case
    [[ -e "$SINK" ]] && grep -Fq "$job $result" "$SINK" && grep -Fq 'https://github.com/owner/repo/actions/runs/123' "$SINK"
    export "$variable=success"
  done
done
BUILD_RESULT=cancelled RELEASE_ASSETS_RESULT=skipped PUBLISH_RESULT=skipped SMOKE_CLEAN_ROOM_RESULT=skipped run_case
[[ -e "$SINK" ]] && grep -Fq 'build cancelled' "$SINK" && grep -Fq 'release-assets, publish, smoke-clean-room' "$SINK"
DRY_RUN=true SMOKE_CLEAN_ROOM_RESULT=skipped run_case
[[ ! -e "$SINK" ]] || { echo 'successful dry run notified' >&2; exit 1; }
DRY_RUN=true BUILD_WASM_MD_RESULT=failure SMOKE_CLEAN_ROOM_RESULT=skipped run_case
[[ -e "$SINK" ]] && grep -Fq 'build-wasm-md failure' "$SINK"
DRY_RUN=true BUILD_RESULT=cancelled RELEASE_ASSETS_RESULT=skipped PUBLISH_RESULT=skipped SMOKE_CLEAN_ROOM_RESULT=skipped run_case
[[ -e "$SINK" ]] && grep -Fq 'build cancelled' "$SINK"
RELEASE_ASSETS_RESULT=skipped run_case
[[ -e "$SINK" ]] && grep -Fq 'required stage skipped' "$SINK"
echo 'release notification cases passed'
