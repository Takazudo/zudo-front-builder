#!/usr/bin/env bash
# Report a failed or incomplete release without treating a mode's expected
# skips as failures: a dry run skips the smoke, a normal/recovery run skips
# finalize, and a finalize-only run (#4690) skips build, build-wasm-md,
# release-assets and publish.
set -euo pipefail

jobs=(release-context source-ci provenance-preflight admit-darwin build build-wasm-md release-assets publish finalize smoke-clean-room)
results=("$RELEASE_CONTEXT_RESULT" "$SOURCE_CI_RESULT" "$PROVENANCE_PREFLIGHT_RESULT" "$ADMIT_DARWIN_RESULT" "$BUILD_RESULT" "$BUILD_WASM_MD_RESULT" "$RELEASE_ASSETS_RESULT" "$PUBLISH_RESULT" "$FINALIZE_RESULT" "$SMOKE_CLEAN_ROOM_RESULT")

expected_skip() {
  local job="$1"
  if [[ "${DRY_RUN:-}" == true && "$job" == smoke-clean-room ]]; then
    return 0
  fi
  if [[ "${FINALIZE_ONLY:-}" == true ]]; then
    [[ "$job" == build || "$job" == build-wasm-md || "$job" == release-assets || "$job" == publish ]]
  else
    [[ "$job" == finalize ]]
  fi
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
