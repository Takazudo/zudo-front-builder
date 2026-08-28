#!/usr/bin/env bash
set -Eeuo pipefail

# scripts/prune-actions-cache.sh — guarded Actions-cache pruning (issue #2686).
#
# Actions caches are scoped by both cache key and ref.  In particular, the
# same key can have a large, valuable entry on refs/heads/main and a stale
# entry on refs/pull/<number>/merge.  This script therefore classifies and
# deletes by the cache id returned by the API, never by key alone.
#
# Usage:
#   scripts/prune-actions-cache.sh [--protect-pattern <ERE>]
#   scripts/prune-actions-cache.sh --yes [--protect-pattern <ERE>]
#
# The default is a read-only dry-run.  --yes is the only way to enable DELETE
# requests.  The mandatory refs/heads/main protection cannot be overridden;
# --protect-pattern (or ACTIONS_CACHE_PROTECT_PATTERN) adds more protected
# refs and is an extended regular expression matched against the whole ref.
# REPO or GITHUB_REPOSITORY supplies the owner/repo slug.  If neither is set,
# the script asks `gh repo view` for the current repository.

DEFAULT_PROTECT_PATTERN='^refs/heads/main$'

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

YES=0
REPO="${REPO:-${GITHUB_REPOSITORY:-}}"
PROTECT_PATTERN="${PROTECT_PATTERN:-${ACTIONS_CACHE_PROTECT_PATTERN:-$DEFAULT_PROTECT_PATTERN}}"

usage() {
  cat >&2 <<'USAGE'
Usage: scripts/prune-actions-cache.sh [--yes] [--protect-pattern <ERE>]

List Actions caches and print a guarded prune plan.  The default is a
read-only dry-run; pass --yes to perform DELETE requests for stale caches.

Options:
  --yes                    Enable deletion of classified stale caches.
  --protect-pattern ERE    Protect additional refs matching this ERE.
  -h, --help               Show this help.

Environment:
  REPO or GITHUB_REPOSITORY  owner/repo (otherwise `gh repo view` is used).
  PROTECT_PATTERN or ACTIONS_CACHE_PROTECT_PATTERN
                            Additional protected-ref ERE.
USAGE
}

die() {
  printf '::error::%s\n' "$1" >&2
  exit 2
}

one_line() {
  # API errors and cache fields should not break the per-entry table layout.
  printf '%s' "$1" | tr '\r\n\t' '   ' | sed 's/[[:space:]][[:space:]]*/ /g'
}

validate_protect_pattern() {
  local regex_rc

  # A variable on the right-hand side is intentionally used here: it keeps
  # user-supplied ERE metacharacters data rather than shell syntax.
  if [[ '' =~ $PROTECT_PATTERN ]]; then
    regex_rc=0
  else
    regex_rc=$?
  fi
  if [[ "$regex_rc" -eq 2 ]]; then
    die "invalid --protect-pattern / PROTECT_PATTERN ERE: $PROTECT_PATTERN"
  fi
}

resolve_repo() {
  if [[ -z "$REPO" ]]; then
    if ! REPO="$(gh repo view --json nameWithOwner --jq '.nameWithOwner' 2>&1)"; then
      die "REPO or GITHUB_REPOSITORY must be set, and gh repo view could not resolve the current repository: $(one_line "$REPO")"
    fi
  fi

  if [[ ! "$REPO" =~ ^[[:alnum:]_.-]+/[[:alnum:]_.-]+$ ]]; then
    die "repository must be an owner/repo slug (got: $REPO)"
  fi
}

fetch_cache_records() {
  local cache_json="$1"

  if ! jq -e '
    if type == "array" then
      all(.[]; type == "object" and ((.actions_caches // null) | type == "array"))
    else
      type == "object" and ((.actions_caches // null) | type == "array")
    end
  ' <<<"$cache_json" >/dev/null; then
    die 'Actions-cache API returned an unexpected JSON shape (expected paginated objects with actions_caches arrays)'
  fi

  # `gh api --paginate --slurp` returns an outer array of page objects.  The
  # non-slurped object form is accepted as well so a fixture or older gh
  # wrapper remains easy to use offline.
  jq -r '
    (if type == "array" then .[] else . end)
    | .actions_caches[]
    | [(.id // ""), (.ref // ""), (.key // ""), (.size_in_bytes // "")]
    | map(if . == null then "" else tostring end)
    | @tsv
  ' <<<"$cache_json"
}

get_pr_state() {
  local pull_number="$1"
  local raw state

  if ! raw="$(gh pr view "$pull_number" --repo "$REPO" --json state --jq '.state' 2>&1)"; then
    printf '%s\n' "$(one_line "$raw")"
    return 1
  fi

  state="$(printf '%s' "$raw" | tr -d '\r\n' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' | tr '[:lower:]' '[:upper:]')"
  case "$state" in
    OPEN|CLOSED|MERGED)
      printf '%s\n' "$state"
      return 0
      ;;
    *)
      printf 'unexpected PR state %s\n' "$(one_line "$raw")"
      return 1
      ;;
  esac
}

print_row() {
  local verdict="$1" id="$2" ref="$3" key="$4" size="$5" detail="$6"
  printf '%-10s %-14s %-30s %-72s %-16s %s\n' \
    "$verdict" "$id" "$ref" "$key" "$size" "$detail"
}

main() {
  while [[ "$#" -gt 0 ]]; do
    case "$1" in
      --yes)
        YES=1
        ;;
      --protect-pattern)
        [[ "$#" -ge 2 ]] || die '--protect-pattern requires an ERE argument'
        PROTECT_PATTERN="$2"
        shift
        ;;
      --protect-pattern=*)
        PROTECT_PATTERN="${1#*=}"
        ;;
      -h|--help)
        usage
        return 0
        ;;
      *)
        usage
        die "unknown argument: $1"
        ;;
    esac
    shift
  done

  cd "$ROOT_DIR"
  command -v gh >/dev/null 2>&1 || die 'gh CLI is required'
  command -v jq >/dev/null 2>&1 || die 'jq is required'
  resolve_repo
  validate_protect_pattern

  local mode='DRY-RUN (read-only; pass --yes to delete)'
  if [[ "$YES" -eq 1 ]]; then
    mode='LIVE (--yes; DELETE requests enabled)'
  fi

  local cache_json records_file
  if ! cache_json="$(gh api "repos/${REPO}/actions/caches" --paginate --slurp 2>&1)"; then
    die "could not list Actions caches for ${REPO}: $(one_line "$cache_json")"
  fi

  records_file="$(mktemp)"
  trap 'rm -f "${records_file:-}"' EXIT
  fetch_cache_records "$cache_json" >"$records_file"

  printf 'Actions-cache prune for %s\n' "$REPO"
  printf 'Mode: %s\n' "$mode"
  printf 'Protected refs: refs/heads/main (mandatory), plus ERE %s\n' "$PROTECT_PATTERN"
  printf '\n'
  printf '%-10s %-14s %-30s %-72s %-16s %s\n' \
    'VERDICT' 'CACHE_ID' 'REF' 'KEY' 'SIZE_BYTES' 'DETAIL'

  local before_total=0
  local candidate_total=0
  local failures=0
  local row_count=0
  local id ref key size verdict detail pull_number state
  local -a prune_ids=()
  local -a prune_sizes=()

  while IFS=$'\t' read -r id ref key size; do
    row_count=$((row_count + 1))

    # Missing or non-numeric fields indicate that the API response is not safe
    # to act on.  Show the row and fail closed before any --yes deletion.
    if [[ ! "$id" =~ ^[0-9]+$ || -z "$ref" || -z "$key" || ! "$size" =~ ^[0-9]+$ ]]; then
      print_row 'SKIP' "${id:-<missing>}" "${ref:-<missing>}" "${key:-<missing>}" "${size:-<missing>}" 'malformed cache record; no deletion'
      failures=$((failures + 1))
      continue
    fi

    before_total=$((before_total + size))
    verdict='SKIP'
    detail='not a refs/pull/<number> ref'

    # Main is mandatory protection even if a caller supplies a narrower custom
    # pattern.  Matching is by ref, deliberately not by key.
    if [[ "$ref" == 'refs/heads/main' ]]; then
      verdict='PROTECTED'
      detail='mandatory main-ref protection'
    elif [[ "$ref" =~ $PROTECT_PATTERN ]]; then
      verdict='PROTECTED'
      detail='matched protect pattern'
    elif [[ "$ref" =~ ^refs/pull/([0-9]+)(/.*)?$ ]]; then
      pull_number="${BASH_REMATCH[1]}"
      if state="$(get_pr_state "$pull_number")"; then
        case "$state" in
          CLOSED|MERGED)
            verdict='PRUNE'
            detail="PR #${pull_number} is ${state}; delete by cache id"
            prune_ids+=("$id")
            prune_sizes+=("$size")
            candidate_total=$((candidate_total + size))
            ;;
          OPEN)
            verdict='SKIP'
            detail="PR #${pull_number} is OPEN; never delete open-PR cache"
            ;;
        esac
      else
        verdict='SKIP'
        detail="PR #${pull_number} state query failed: $(one_line "$state")"
        failures=$((failures + 1))
      fi
    fi

    print_row "$verdict" "$id" "$ref" "$key" "$size" "$detail"
  done <"$records_file"

  if [[ "$row_count" -eq 0 ]]; then
    printf '%s\n' 'No Actions-cache entries returned.'
  fi

  printf '\nTotal before: %s bytes\n' "$before_total"

  if [[ "$failures" -gt 0 ]]; then
    printf '::error::%s cache record(s) could not be classified safely; no deletions attempted\n' "$failures" >&2
    printf 'Total after (no deletion): %s bytes\n' "$before_total"
    return 1
  fi

  local projected_after=$((before_total - candidate_total))
  if [[ "$YES" -eq 0 ]]; then
    printf 'Total after (dry-run projection): %s bytes\n' "$projected_after"
    printf 'Dry-run complete: no caches deleted; %s byte(s) would be pruned.\n' "$candidate_total"
    return 0
  fi

  local deleted_total=0
  local delete_failures=0
  local index cache_id cache_size
  for index in "${!prune_ids[@]}"; do
    cache_id="${prune_ids[$index]}"
    cache_size="${prune_sizes[$index]}"
    if gh api --method DELETE "repos/${REPO}/actions/caches/${cache_id}" --silent >/dev/null 2>&1; then
      printf 'Deleted cache id=%s (%s bytes)\n' "$cache_id" "$cache_size"
      deleted_total=$((deleted_total + cache_size))
    else
      printf '::error::failed to delete cache id=%s; no retry or key-based fallback\n' "$cache_id" >&2
      delete_failures=$((delete_failures + 1))
    fi
  done

  printf 'Total after: %s bytes\n' "$((before_total - deleted_total))"
  if [[ "$delete_failures" -gt 0 ]]; then
    printf '::error::%s cache deletion(s) failed\n' "$delete_failures" >&2
    return 1
  fi
  printf 'Prune complete: %s byte(s) deleted.\n' "$deleted_total"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
