#!/usr/bin/env bash
# verify-release-source.sh — release.yml `release-context` source verification
# (#4690). Run inside the SOURCE checkout (cwd). Records the source/control
# split every later job relies on:
#   source commit  — the immutable commit the release builds and publishes
#   source tree    — `git rev-parse <commit>^{tree}`
#   control SHA    — github.sha, the commit whose workflow and helper scripts run
#   workflow ref   — github.workflow_ref
#
# For a tagged context it also requires: the tag ref was fetched and peels to
# the checked-out commit; packages/zfb's version is the tag's; a published,
# non-draft GitHub Release exists for the tag; and a commit-SHA targetCommitish
# resolves to the same commit.
#
# Env in: RELEASE_TAG (empty = branch dry run) CONTROL_SHA WORKFLOW_REF
#         GITHUB_OUTPUT [GITHUB_STEP_SUMMARY]
# Writes: checkout_sha source_commit source_tree release_version control_sha workflow_ref
set -euo pipefail

CHECKOUT_SHA=$(git rev-parse HEAD)
SOURCE_TREE=$(git rev-parse "${CHECKOUT_SHA}^{tree}")
RELEASE_TAG="${RELEASE_TAG:-}"
RELEASE_VERSION=""
RELEASE_URL=""

if [[ -n "$RELEASE_TAG" ]]; then
  if ! git show-ref --verify --quiet "refs/tags/${RELEASE_TAG}"; then
    echo "::error::Tag ref refs/tags/${RELEASE_TAG} was not fetched."
    exit 1
  fi
  TAG_SHA=$(git rev-parse "refs/tags/${RELEASE_TAG}^{commit}")
  if [[ "$CHECKOUT_SHA" != "$TAG_SHA" ]]; then
    echo "::error::Checked-out commit ${CHECKOUT_SHA} does not match ${RELEASE_TAG} (${TAG_SHA})."
    exit 1
  fi

  EXPECTED_VERSION="${RELEASE_TAG#v}"
  ACTUAL_VERSION=$(node -p "require('./packages/zfb/package.json').version")
  if [[ "$ACTUAL_VERSION" != "$EXPECTED_VERSION" ]]; then
    echo "::error::packages/zfb version '${ACTUAL_VERSION}' does not match tag '${RELEASE_TAG}'."
    exit 1
  fi
  RELEASE_VERSION="$EXPECTED_VERSION"

  if ! RELEASE_JSON=$(gh release view "$RELEASE_TAG" \
    --json tagName,isDraft,publishedAt,targetCommitish,url); then
    echo "::error::No GitHub Release exists for ${RELEASE_TAG}."
    exit 1
  fi
  ACTUAL_TAG=$(jq -r '.tagName' <<<"$RELEASE_JSON")
  IS_DRAFT=$(jq -r '.isDraft' <<<"$RELEASE_JSON")
  PUBLISHED_AT=$(jq -r '.publishedAt // empty' <<<"$RELEASE_JSON")
  TARGET_COMMITISH=$(jq -r '.targetCommitish // empty' <<<"$RELEASE_JSON")
  RELEASE_URL=$(jq -r '.url' <<<"$RELEASE_JSON")
  if [[ "$ACTUAL_TAG" != "$RELEASE_TAG" || "$IS_DRAFT" != "false" || -z "$PUBLISHED_AT" ]]; then
    echo "::error::${RELEASE_TAG} must identify an existing published, non-draft GitHub Release."
    exit 1
  fi
  if [[ "$TARGET_COMMITISH" =~ ^[0-9a-fA-F]{7,40}$ ]]; then
    if ! TARGET_SHA=$(git rev-parse --verify --quiet "${TARGET_COMMITISH}^{commit}"); then
      echo "::error::Release target ${TARGET_COMMITISH} is not a commit in this checkout."
      exit 1
    fi
    if [[ "$TARGET_SHA" != "$TAG_SHA" ]]; then
      echo "::error::Release target ${TARGET_COMMITISH} (${TARGET_SHA}) does not match tag ${RELEASE_TAG} (${TAG_SHA})."
      exit 1
    fi
  fi
fi

{
  echo "checkout_sha=$CHECKOUT_SHA"
  echo "source_commit=$CHECKOUT_SHA"
  echo "source_tree=$SOURCE_TREE"
  echo "release_version=$RELEASE_VERSION"
  echo "control_sha=${CONTROL_SHA:-}"
  echo "workflow_ref=${WORKFLOW_REF:-}"
} >>"${GITHUB_OUTPUT:?}"

if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    echo "### Release source and control"
    echo
    echo "| | |"
    echo "| --- | --- |"
    echo "| Release tag | \`${RELEASE_TAG:-(none — branch dry run)}\` |"
    echo "| Source commit | \`${CHECKOUT_SHA}\` |"
    echo "| Source tree | \`${SOURCE_TREE}\` |"
    echo "| Control SHA (workflow + helper scripts) | \`${CONTROL_SHA:-}\` |"
    echo "| Workflow ref | \`${WORKFLOW_REF:-}\` |"
    if [[ -n "$RELEASE_URL" ]]; then echo "| Release | ${RELEASE_URL} |"; fi
  } >>"$GITHUB_STEP_SUMMARY"
fi
echo "Verified source ${CHECKOUT_SHA} (tree ${SOURCE_TREE})${RELEASE_TAG:+ for ${RELEASE_TAG}} | control ${CONTROL_SHA:-?} | ${WORKFLOW_REF:-?}"
