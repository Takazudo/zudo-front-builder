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
export RELEASE_CONTEXT_RESULT=success SOURCE_CI_RESULT=success PROVENANCE_PREFLIGHT_RESULT=success ADMIT_DARWIN_RESULT=success
export BUILD_RESULT=success BUILD_WASM_MD_RESULT=success RELEASE_ASSETS_RESULT=success PUBLISH_RESULT=success SMOKE_CLEAN_ROOM_RESULT=success
export FINALIZE_RESULT=skipped FINALIZE_ONLY=false PACK_RESULT=success MD_WASM_ASSET_RESULT=success FINALIZE_ASSETS_RESULT=skipped

run_case() {
  rm -f "$SINK"
  bash "$ROOT/scripts/notify-release-result.sh" > "$tmp/output"
}
run_case
[[ ! -e "$SINK" ]] || { echo 'successful release notified' >&2; exit 1; }
for pair in \
  RELEASE_CONTEXT_RESULT:release-context SOURCE_CI_RESULT:source-ci \
  PROVENANCE_PREFLIGHT_RESULT:provenance-preflight ADMIT_DARWIN_RESULT:admit-darwin \
  BUILD_RESULT:build BUILD_WASM_MD_RESULT:build-wasm-md PACK_RESULT:pack MD_WASM_ASSET_RESULT:md-wasm-asset \
  RELEASE_ASSETS_RESULT:release-assets PUBLISH_RESULT:publish SMOKE_CLEAN_ROOM_RESULT:smoke-clean-room; do
  variable=${pair%%:*}
  job=${pair#*:}
  for result in failure cancelled; do
    export "$variable=$result"
    run_case
    [[ -e "$SINK" ]]
    grep -Fq "$job $result" "$SINK"
    grep -Fq 'https://github.com/owner/repo/actions/runs/123' "$SINK"
    export "$variable=success"
  done
done
BUILD_RESULT=cancelled RELEASE_ASSETS_RESULT=skipped PUBLISH_RESULT=skipped SMOKE_CLEAN_ROOM_RESULT=skipped run_case
[[ -e "$SINK" ]]
grep -Fq 'build cancelled' "$SINK"
grep -Fq 'release-assets, publish, smoke-clean-room' "$SINK"
DRY_RUN=true SMOKE_CLEAN_ROOM_RESULT=skipped run_case
[[ ! -e "$SINK" ]] || { echo 'successful dry run notified' >&2; exit 1; }
DRY_RUN=true BUILD_WASM_MD_RESULT=failure SMOKE_CLEAN_ROOM_RESULT=skipped run_case
[[ -e "$SINK" ]]
grep -Fq 'build-wasm-md failure' "$SINK"
DRY_RUN=true BUILD_RESULT=cancelled RELEASE_ASSETS_RESULT=skipped PUBLISH_RESULT=skipped SMOKE_CLEAN_ROOM_RESULT=skipped run_case
[[ -e "$SINK" ]]
grep -Fq 'build cancelled' "$SINK"
RELEASE_ASSETS_RESULT=skipped run_case
[[ -e "$SINK" ]]
grep -Fq 'required stage skipped' "$SINK"
# A normal run whose finalize job is skipped is not incomplete; a normal run
# whose finalize RAN and failed is.
FINALIZE_RESULT=failure run_case
[[ -e "$SINK" ]]
grep -Fq 'finalize failure' "$SINK"
# Finalize-only (#4690): build/build-wasm-md/release-assets/publish skips are
# expected; a skipped finalize or smoke is not.
finalize_case() {
  FINALIZE_ONLY=true BUILD_RESULT=skipped BUILD_WASM_MD_RESULT=skipped RELEASE_ASSETS_RESULT=skipped \
    PACK_RESULT=skipped PUBLISH_RESULT=skipped MD_WASM_ASSET_RESULT=skipped run_case
}
FINALIZE_RESULT=success FINALIZE_ASSETS_RESULT=success finalize_case
[[ ! -e "$SINK" ]] || { echo 'successful finalize notified' >&2; exit 1; }
FINALIZE_RESULT=success FINALIZE_ASSETS_RESULT=skipped finalize_case
[[ -e "$SINK" ]]
grep -Fq 'skipped: finalize-assets' "$SINK"
# A tagged dry run skips every write job and the smoke, nothing else.
DRY_RUN=true PUBLISH_RESULT=skipped MD_WASM_ASSET_RESULT=skipped SMOKE_CLEAN_ROOM_RESULT=skipped run_case
[[ ! -e "$SINK" ]] || { echo 'successful dry run (write jobs skipped) notified' >&2; exit 1; }
DRY_RUN=true PACK_RESULT=skipped PUBLISH_RESULT=skipped SMOKE_CLEAN_ROOM_RESULT=skipped run_case
[[ -e "$SINK" ]]
grep -Fq 'skipped: pack' "$SINK"
FINALIZE_RESULT=skipped finalize_case
[[ -e "$SINK" ]]
grep -Fq 'skipped: finalize' "$SINK"
FINALIZE_RESULT=failure SMOKE_CLEAN_ROOM_RESULT=skipped finalize_case
[[ -e "$SINK" ]]
grep -Fq 'finalize failure' "$SINK"
ADMIT_DARWIN_RESULT=failure BUILD_RESULT=skipped RELEASE_ASSETS_RESULT=skipped PUBLISH_RESULT=skipped SMOKE_CLEAN_ROOM_RESULT=skipped run_case
[[ -e "$SINK" ]]
grep -Fq 'admit-darwin failure' "$SINK"
# npm still processing (publish job exit 3): distinct from a plain publish failure.
PUBLISH_RESULT=failure NPM_PROCESSING=true NPM_PROCESSING_PACKAGES=@takazudo/zfb-linux-arm64-gnu@6.1.3 \
  FINALIZE_RESULT=skipped run_case
[[ -e "$SINK" ]]
grep -Fq 'publish pending: npm accepted but is still processing @takazudo/zfb-linux-arm64-gnu@6.1.3' "$SINK"
! grep -Fq 'publish failure' "$SINK"
PUBLISH_RESULT=failure NPM_PROCESSING=false run_case
[[ -e "$SINK" ]]
grep -Fq 'publish failure' "$SINK"
PUBLISH_RESULT=failure run_case
grep -Fq 'publish failure' "$SINK"
echo 'release notification cases passed'
