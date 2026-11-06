#!/usr/bin/env bash
# Report a failed or incomplete release without treating dry-run smoke skips as failures.
set -euo pipefail

jobs=(release-context detect-mac-local prepare-matrix build build-wasm-md release-assets publish smoke-clean-room)
results=("$RELEASE_CONTEXT_RESULT" "$DETECT_MAC_LOCAL_RESULT" "$PREPARE_MATRIX_RESULT" "$BUILD_RESULT" "$BUILD_WASM_MD_RESULT" "$RELEASE_ASSETS_RESULT" "$PUBLISH_RESULT" "$SMOKE_CLEAN_ROOM_RESULT")
problems=()
skipped=()
all_skipped=()
for i in "${!jobs[@]}"; do
  result="${results[$i]}"
  if [[ "$result" == failure || "$result" == cancelled ]]; then
    problems+=("${jobs[$i]} ${result}")
  fi
  if [[ "$result" == skipped ]]; then
    all_skipped+=("${jobs[$i]}")
    if [[ !( "${DRY_RUN:-}" == true && "${jobs[$i]}" == smoke-clean-room ) ]]; then
      skipped+=("${jobs[$i]}")
    fi
  fi
done
if [[ ${#problems[@]} -eq 0 && ${#skipped[@]} -gt 0 ]]; then
  problems+=("required stage skipped")
fi
if [[ ${#problems[@]} -eq 0 ]]; then
  echo "Release workflow completed successfully${DRY_RUN:+ (dry_run=$DRY_RUN)}; no notification."
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
