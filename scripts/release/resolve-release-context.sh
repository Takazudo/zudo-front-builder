#!/usr/bin/env bash
# resolve-release-context.sh — release.yml `release-context` resolver (#4690).
#
# Maps the trigger to one release context before any checkout of the source:
#   release: published            → the event's tag
#   workflow_dispatch + release_tag (from refs/heads/main only)
#                                 → recovery of that existing tag; with
#                                   finalize_only=true, finalization-only recovery
#   workflow_dispatch from refs/tags/v*  → that tag (backward compatible)
#   workflow_dispatch, no tag     → branch dry run only (dry_run=true required)
#
# Env in: EVENT_NAME EVENT_RELEASE_TAG REQUESTED_TAG DRY_RUN FINALIZE_ONLY
#         TRIGGER_REF TRIGGER_SHA GITHUB_OUTPUT
# Writes: release_tag checkout_ref is_recovery is_finalize
set -euo pipefail

RELEASE_TAG=""
CHECKOUT_REF="${TRIGGER_SHA:?}"
IS_RECOVERY=false
IS_FINALIZE=false

if [[ "${FINALIZE_ONLY:-false}" == "true" ]]; then
  if [[ "${EVENT_NAME:-}" != "workflow_dispatch" || -z "${REQUESTED_TAG:-}" ]]; then
    echo "::error::finalize_only=true requires a workflow_dispatch with release_tag=vX.Y.Z."
    exit 1
  fi
fi

if [[ "${EVENT_NAME:-}" == "release" ]]; then
  RELEASE_TAG="${EVENT_RELEASE_TAG:-}"
elif [[ "${EVENT_NAME:-}" == "workflow_dispatch" ]]; then
  if [[ -n "${REQUESTED_TAG:-}" ]]; then
    if [[ "${TRIGGER_REF:-}" != "refs/heads/main" ]]; then
      echo "::error::Tag recovery must run the reviewed workflow from refs/heads/main; got '${TRIGGER_REF:-}'."
      exit 1
    fi
    RELEASE_TAG="$REQUESTED_TAG"
    IS_RECOVERY=true
    if [[ "${FINALIZE_ONLY:-false}" == "true" ]]; then
      IS_FINALIZE=true
    fi
  elif [[ "${TRIGGER_REF:-}" =~ ^refs/tags/v ]]; then
    # Backward-compatible manual dispatch for tags containing this
    # implementation. Existing broken tags dispatch main with release_tag.
    RELEASE_TAG="${TRIGGER_REF#refs/tags/}"
  elif [[ "${DRY_RUN:-}" != "true" ]]; then
    echo "::error::A real manual release requires release_tag=vX.Y.Z and must be dispatched from main."
    echo "::error::Branch smoke tests must use dry_run=true."
    exit 1
  fi
else
  echo "::error::Unsupported release trigger: '${EVENT_NAME:-}'."
  exit 1
fi

if [[ -n "$RELEASE_TAG" ]]; then
  if [[ ! "$RELEASE_TAG" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-(next|beta|rc)\.(0|[1-9][0-9]*))?$ ]]; then
    echo "::error::Release tag '${RELEASE_TAG}' is not an allowed v* semver tag."
    exit 1
  fi
  CHECKOUT_REF="$RELEASE_TAG"
fi

{
  echo "release_tag=$RELEASE_TAG"
  echo "checkout_ref=$CHECKOUT_REF"
  echo "is_recovery=$IS_RECOVERY"
  echo "is_finalize=$IS_FINALIZE"
} >>"${GITHUB_OUTPUT:?}"
SUFFIX=""
if [[ "$IS_FINALIZE" == "true" ]]; then SUFFIX=" [finalize only]"; fi
echo "Resolved source ref: ${CHECKOUT_REF}${RELEASE_TAG:+ (release ${RELEASE_TAG})}${SUFFIX}"
