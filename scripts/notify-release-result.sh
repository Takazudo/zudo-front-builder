#!/usr/bin/env bash
# Report a failed or incomplete release without treating a mode's expected
# skips as failures (#4690):
#   normal/recovery  skips finalize and finalize-assets;
#   finalize-only    skips build, build-wasm-md, release-assets, pack, publish
#                    and md-wasm-asset;
#   dry run          additionally skips every write job (publish,
#                    md-wasm-asset, finalize-assets) and the smoke.
set -euo pipefail

jobs=(release-context source-ci provenance-preflight admit-darwin build build-wasm-md release-assets pack publish md-wasm-asset finalize finalize-assets smoke-clean-room)
results=("$RELEASE_CONTEXT_RESULT" "$SOURCE_CI_RESULT" "$PROVENANCE_PREFLIGHT_RESULT" "$ADMIT_DARWIN_RESULT" "$BUILD_RESULT" "$BUILD_WASM_MD_RESULT" "$RELEASE_ASSETS_RESULT" "$PACK_RESULT" "$PUBLISH_RESULT" "$MD_WASM_ASSET_RESULT" "$FINALIZE_RESULT" "$FINALIZE_ASSETS_RESULT" "$SMOKE_CLEAN_ROOM_RESULT")

expected_skip() {
  local job="$1"
  if [[ "${DRY_RUN:-}" == true ]]; then
    case "$job" in
      smoke-clean-room | publish | md-wasm-asset | finalize-assets) return 0 ;;
    esac
  fi
  if [[ "${FINALIZE_ONLY:-}" == true ]]; then
    case "$job" in
      build | build-wasm-md | release-assets | pack | publish | md-wasm-asset) return 0 ;;
    esac
    return 1
  fi
  [[ "$job" == finalize || "$job" == finalize-assets ]]
}

problems=()
skipped=()
all_skipped=()
for i in "${!jobs[@]}"; do
  result="${results[$i]}"
  if [[ "$result" == failure || "$result" == cancelled ]]; then
    problems+=("${jobs[$i]} ${result}")
  fi
  if [[ "$result" == skipped ]] && ! expected_skip "${jobs[$i]}"; then
    all_skipped+=("${jobs[$i]}")
    skipped+=("${jobs[$i]}")
  fi
done
if [[ ${#problems[@]} -eq 0 && ${#skipped[@]} -gt 0 ]]; then
  problems+=("required stage skipped")
fi
if [[ ${#problems[@]} -eq 0 ]]; then
  echo "Release workflow completed successfully${DRY_RUN:+ (dry_run=$DRY_RUN)}${FINALIZE_ONLY:+ (finalize_only=$FINALIZE_ONLY)}; no notification."
  exit 0
fi

printf -v problem_text '%s, ' "${problems[@]}"
message="Release incomplete (${problem_text%, })"
if [[ ${#all_skipped[@]} -gt 0 ]]; then
  printf -v skipped_text '%s, ' "${all_skipped[@]}"
  message+="; skipped: ${skipped_text%, }"
fi
echo "$message"
if [[ -z "${IFTTT_PROD_NOTIFY:-}" ]]; then
  echo "No notification endpoint configured."
  exit 0
fi
run_url="${SERVER_URL}/${REPO}/actions/runs/${RUN_ID}"
tag_label="${RAW_TAG:-${GITHUB_SHA_VAL:0:7}}"
payload=$(jq -n --arg message "$message" --arg tag "$tag_label" --arg url "$run_url" \
  '{value1:$message,value2:$tag,value3:$url}')
curl -sSf --max-time 10 -X POST "$IFTTT_PROD_NOTIFY" \
  -H 'Content-Type: application/json' -d "$payload" || echo "::warning::IFTTT notification failed"
