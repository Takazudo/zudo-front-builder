#!/usr/bin/env bash
#
# scripts/check-exam-ignore-parity.sh — mechanical reconciliation guard for
# issue #2072.
#
# crates/CLAUDE.md's "`#[ignore]` manifest table" section requires every
# `env-gate:`/`heavy:`-tagged `#[ignore]`d Rust test to be reachable by
# EITHER an exact-name entry in exam.yml's weekly quarantine-heavy filterset
# OR a health.yml `-- --ignored` T1 step whose scope actually covers it. That
# rule had drifted before (issue #2058) because nothing recomputed it — the
# crates/CLAUDE.md manifest table was a point-in-time manual audit, not a live
# check. This script recomputes the diff FRESH every time it runs, by
# actually parsing:
#
#   1. every `#[ignore = "<tag>: ..."]`d test under crates/ (same convention
#      as crates/CLAUDE.md's own audit grep: `grep -rn '#\[ignore = "' crates/ |
#      grep -v ':[0-9]*: *//'` — a `//`-commented mention of the attribute
#      text does not count as a real `#[ignore]`);
#   2. exam.yml's quarantine-heavy `-E '...'` filterset (every `test(=NAME)`
#      entry outside a YAML `#` comment line — the header comment's own
#      `test(=NAME)` placeholder example is excluded this way, not by
#      hard-coding "NAME" as a special case);
#   3. health.yml's ignored-test steps, by actually parsing both the legacy
#      `cargo test -p <pkg> [--test <bin> | --tests | --lib <module>::]
#      -- --ignored` scopes and workspace nextest `--run-ignored ignored-only`
#      filtersets containing exact `package(...) & binary(=...)` or
#      package-scoped `test(=...)` predicates — NOT by grepping for a similarly
#      named step. A step whose *name* happens to mention a test but whose
#      command scopes a different package/binary/module must NOT count as
#      covering it; see tests/unit/check-exam-ignore-parity.sh's decoys.
#
# Every ignored test then falls into exactly one of four outcomes:
#
#   - COVERED-BY-EXAM         — its exact qualified name is in exam.yml's
#                                filterset.
#   - COVERED-BY-HEALTH-SCOPE — a health.yml step's actual scope (package +
#                                integration-test binary, or package + lib
#                                module prefix) reaches it.
#   - EXCEPTION(<tag>)        — the test's tag is a legitimate exception
#                                (currently `verification:`; `pending-feature:`
#                                joins the day one exists) — read from the
#                                tag prefix as DATA, never a hard-coded test
#                                name.
#   - UNCOVERED                — none of the above. Exits non-zero.
#
# Rust module-path derivation (for lib-test files under crates/<pkg>/src/):
# this script assumes the codebase's established convention of ONE top-level,
# unindented `#[cfg(test)] mod tests { ... }` block per file wrapping all of
# that file's unit tests (true for both current lib-test files — verified
# during #2072's implementation: `grep -c '^mod tests {' <file>` == 1 in
# each, with no other top-level `mod` after it and no close-then-reopen
# before EOF). A test is treated as inside `tests::` when the nearest
# `mod tests {` line ABOVE it starts at column 0; this is a simplifying
# assumption, not a general Rust module resolver — if it ever produces a
# wrong qualified name the failure mode is safely fail-closed (reported
# UNCOVERED, not silently mis-covered), forcing a human look rather than
# masking real drift.
#
# Paths are overridable via env vars so the offline fixture-based unit test
# (tests/unit/check-exam-ignore-parity.sh) can point this script at a small
# synthetic tree instead of the real repo.

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT_DIR=$(cd "$SCRIPT_DIR/.." && pwd)

CRATES_ROOT="${CHECK_EXAM_IGNORE_PARITY_CRATES_ROOT:-$ROOT_DIR/crates}"
CRATES_ROOT="${CRATES_ROOT%/}"
EXAM_WORKFLOW="${CHECK_EXAM_IGNORE_PARITY_EXAM_WORKFLOW:-$ROOT_DIR/.github/workflows/exam.yml}"
HEALTH_WORKFLOW="${CHECK_EXAM_IGNORE_PARITY_HEALTH_WORKFLOW:-$ROOT_DIR/.github/workflows/health.yml}"

for required in "$CRATES_ROOT" "$EXAM_WORKFLOW" "$HEALTH_WORKFLOW"; do
  if [[ ! -e "$required" ]]; then
    echo "FAIL: expected path does not exist: $required" >&2
    exit 1
  fi
done

# Tag prefixes exempt from the "must be reachable by exam.yml or health.yml"
# rule — DATA read from the tag text, never a hard-coded test name. Mirrors
# crates/CLAUDE.md's `#[ignore]` taxonomy: `verification:` is a one-time proof, and
# a future `pending-feature:` test is blocked on an unimplemented product
# feature, not on scheduling.
EXCEPTION_TAGS="verification pending-feature"

list_contains() {
  local needle="$1" list="$2" item
  for item in $list; do
    [[ "$item" == "$needle" ]] && return 0
  done
  return 1
}

# ── 1. exam.yml quarantine-heavy filterset ──────────────────────────────────

# bash 3.2 (stock macOS /bin/bash) has no associative arrays, so the dedup set
# is a newline-delimited list instead, with exact-line (never substring)
# membership checks via `grep -qxF` — see exam_set_contains below.
EXAM_SET_LIST=""
while IFS= read -r name; do
  [[ -n "$name" ]] && EXAM_SET_LIST="$EXAM_SET_LIST
$name"
done < <(grep -vE '^[[:space:]]*#' "$EXAM_WORKFLOW" \
  | grep -oE 'test\(=[A-Za-z0-9_:]+\)' \
  | sed -E 's/test\(=([A-Za-z0-9_:]+)\)/\1/' \
  | sort -u || true)

exam_set_contains() {
  # -x: whole-line match only. A bare `grep -qF` would let one test name
  # match as a substring of another (e.g. "foo" inside "foo_extended").
  # A here-string, not a `printf | grep -q` pipe: under `set -o pipefail`
  # (this script has `set -euo pipefail`), grep -q can exit as soon as it
  # finds a match, SIGPIPE-killing a still-writing producer on the other
  # end of a real pipe — pipefail then reports the pipeline as failed even
  # though the match was found, misclassifying a covered test as UNCOVERED
  # (reproduces with a large enough exam.yml filterset). A here-string has
  # no live producer process to SIGPIPE, so this can't happen.
  grep -qxF -- "$1" <<< "$EXAM_SET_LIST"
}

# ── 2. health.yml ignored-test step scopes ──────────────────────────────────

HEALTH_PKG=()
HEALTH_KIND=()   # int_all | int_named | lib_all | lib_scoped
HEALTH_TARGET=() # binary name (int_named) or module prefix (lib_scoped); empty otherwise

while IFS= read -r line; do
  pkg=""
  if [[ "$line" =~ -p[[:space:]]+([A-Za-z0-9_-]+) ]]; then
    pkg="${BASH_REMATCH[1]}"
  else
    continue
  fi

  if [[ "$line" =~ --tests([[:space:]]|$) ]]; then
    HEALTH_PKG+=("$pkg"); HEALTH_KIND+=("int_all"); HEALTH_TARGET+=("")
  elif [[ "$line" =~ --test[[:space:]]+([A-Za-z0-9_-]+) ]]; then
    HEALTH_PKG+=("$pkg"); HEALTH_KIND+=("int_named"); HEALTH_TARGET+=("${BASH_REMATCH[1]}")
  elif [[ "$line" =~ --lib[[:space:]]+([A-Za-z0-9_:]+):: ]]; then
    HEALTH_PKG+=("$pkg"); HEALTH_KIND+=("lib_scoped"); HEALTH_TARGET+=("${BASH_REMATCH[1]}")
  elif [[ "$line" =~ --lib([[:space:]]|$) ]]; then
    HEALTH_PKG+=("$pkg"); HEALTH_KIND+=("lib_all"); HEALTH_TARGET+=("")
  fi
done < <(grep -vE '^[[:space:]]*#' "$HEALTH_WORKFLOW" \
  | grep -E 'cargo test.*-- --ignored' || true)

# Workspace-resolved health lanes use exact nextest filter predicates rather
# than Cargo package scopes. Keep package identity attached to every predicate:
# a same-named test or binary in another package is a decoy, not coverage.
# Supported shapes deliberately match nextest's exact-name forms used here:
#
#   (package(pkg) & binary(=integration_target))
#   package(pkg) & (
#     test(=fully::qualified::name) | binary(=integration_target)
#   )
#
# A predicate outside a package conjunction fails closed. This is a small
# workflow parser, not a general nextest filter-expression evaluator.
HEALTH_NEXTEST_BINARY_PKG=()
HEALTH_NEXTEST_BINARY_TARGET=()
HEALTH_NEXTEST_TEST_PKG=()
HEALTH_NEXTEST_TEST_TARGET=()

in_nextest_filter=0
active_nextest_pkg=""
while IFS= read -r line; do
  if ((in_nextest_filter == 0)); then
    if [[ "$line" == *"cargo nextest run --workspace"* \
      && "$line" == *"--run-ignored ignored-only"* \
      && "$line" == *"-E '"* ]]; then
      in_nextest_filter=1
      active_nextest_pkg=""
    fi
    continue
  fi

  # The workflow uses a single-quoted multiline filter expression. Its closing
  # quote is on a line of its own before any tee/redirection suffix.
  trimmed="${line#"${line%%[![:space:]]*}"}"
  if [[ "$trimmed" == "'"* ]]; then
    in_nextest_filter=0
    active_nextest_pkg=""
    continue
  fi
  [[ "$trimmed" == \#* ]] && continue

  line_pkg=""
  if [[ "$line" =~ package\(([A-Za-z0-9_-]+)\) ]]; then
    line_pkg="${BASH_REMATCH[1]}"
  fi

  predicate_pkg="$line_pkg"
  [[ -z "$predicate_pkg" ]] && predicate_pkg="$active_nextest_pkg"

  if [[ -n "$predicate_pkg" && "$line" =~ binary\(=([A-Za-z0-9_-]+)\) ]]; then
    HEALTH_NEXTEST_BINARY_PKG+=("$predicate_pkg")
    HEALTH_NEXTEST_BINARY_TARGET+=("${BASH_REMATCH[1]}")
  fi
  if [[ -n "$predicate_pkg" && "$line" =~ test\(=([A-Za-z0-9_:]+)\) ]]; then
    HEALTH_NEXTEST_TEST_PKG+=("$predicate_pkg")
    HEALTH_NEXTEST_TEST_TARGET+=("${BASH_REMATCH[1]}")
  fi

  # Only a package conjunction that opens a following group establishes
  # context for later predicate lines. A one-line package/binary conjunction
  # must not leak its package into the next expression.
  if [[ -n "$line_pkg" && "$line" =~ \&[[:space:]]*\([[:space:]]*$ ]]; then
    active_nextest_pkg="$line_pkg"
  elif [[ "$trimmed" == ")"* ]]; then
    active_nextest_pkg=""
  fi
done < "$HEALTH_WORKFLOW"

health_scope_covers() {
  local pkg="$1" kind="$2" binary="$3" qualified="$4"
  local idx
  for ((idx = 0; idx < ${#HEALTH_PKG[@]}; idx++)); do
    [[ "${HEALTH_PKG[$idx]}" == "$pkg" ]] || continue
    case "${HEALTH_KIND[$idx]}" in
      int_all)
        [[ "$kind" == "integration" ]] && return 0
        ;;
      int_named)
        [[ "$kind" == "integration" && "$binary" == "${HEALTH_TARGET[$idx]}" ]] && return 0
        ;;
      lib_scoped)
        if [[ "$kind" == "lib" ]]; then
          case "$qualified" in
            "${HEALTH_TARGET[$idx]}::"*) return 0 ;;
          esac
        fi
        ;;
      lib_all)
        [[ "$kind" == "lib" ]] && return 0
        ;;
    esac
  done

  if [[ "$kind" == "integration" ]]; then
    for ((idx = 0; idx < ${#HEALTH_NEXTEST_BINARY_PKG[@]}; idx++)); do
      if [[ "${HEALTH_NEXTEST_BINARY_PKG[$idx]}" == "$pkg" \
        && "${HEALTH_NEXTEST_BINARY_TARGET[$idx]}" == "$binary" ]]; then
        return 0
      fi
    done
  fi

  for ((idx = 0; idx < ${#HEALTH_NEXTEST_TEST_PKG[@]}; idx++)); do
    if [[ "${HEALTH_NEXTEST_TEST_PKG[$idx]}" == "$pkg" \
      && "${HEALTH_NEXTEST_TEST_TARGET[$idx]}" == "$qualified" ]]; then
      return 0
    fi
  done
  return 1
}

# ── 3. walk crates/ for #[ignore = "..."]d tests ────────────────────────────

COVERED_EXAM=0
COVERED_HEALTH=0
EXCEPTIONS=0
UNCOVERED=0

while IFS= read -r -d '' file; do
  # bash 3.2 has no `mapfile`/`readarray`; slurp the file into a plain indexed
  # array by hand instead. The `|| [[ -n "$line" ]]` tail preserves a final
  # unterminated line (read fails at EOF but still populates $line).
  LINES=()
  while IFS= read -r line || [[ -n "$line" ]]; do
    LINES+=("$line")
  done < "$file"
  total=${#LINES[@]}

  rel="${file#"$CRATES_ROOT"/}"
  if [[ "$rel" =~ ^([^/]+)/tests/([^/]+)\.rs$ ]]; then
    pkg="${BASH_REMATCH[1]}"; binary="${BASH_REMATCH[2]}"; kind="integration"
  elif [[ "$rel" =~ ^([^/]+)/src/(.+)\.rs$ ]]; then
    pkg="${BASH_REMATCH[1]}"; relmod="${BASH_REMATCH[2]}"; kind="lib"
    module_path="${relmod//\//::}"
    binary=""
  else
    continue
  fi

  for ((i = 0; i < total; i++)); do
    line="${LINES[$i]}"
    trimmed="${line#"${line%%[![:space:]]*}"}"
    [[ "$line" == *'#[ignore = "'* ]] || continue
    [[ "$trimmed" == //* ]] && continue

    tag="?"
    if [[ "$line" =~ \#\[ignore\ =\ \"([a-z-]+): ]]; then
      tag="${BASH_REMATCH[1]}"
    fi

    # the attribute may span multiple `\`-continued lines; find where it closes
    j=$i
    while [[ "${LINES[$j]}" != *'"]'* ]] && ((j < i + 8 && j < total - 1)); do
      j=$((j + 1))
    done

    fn_name=""
    for ((k = j + 1; k < total && k < j + 8; k++)); do
      if [[ "${LINES[$k]}" =~ fn[[:space:]]+([A-Za-z0-9_]+) ]]; then
        fn_name="${BASH_REMATCH[1]}"
        break
      fi
    done
    [[ -z "$fn_name" ]] && fn_name="<unresolved-fn-near-line-$((i + 1))>"

    if [[ "$kind" == "integration" ]]; then
      qualified="$fn_name"
    else
      mod_tests_prefix=""
      for ((m = i; m >= 0; m--)); do
        if [[ "${LINES[$m]}" =~ ^mod[[:space:]]+tests[[:space:]]*\{ ]]; then
          mod_tests_prefix="tests::"
          break
        fi
      done
      qualified="${module_path}::${mod_tests_prefix}${fn_name}"
    fi

    loc="$file:$((i + 1))"

    if list_contains "$tag" "$EXCEPTION_TAGS"; then
      echo "EXCEPTION($tag): $qualified ($loc)"
      EXCEPTIONS=$((EXCEPTIONS + 1))
    elif exam_set_contains "$qualified"; then
      echo "COVERED-BY-EXAM: $qualified ($loc)"
      COVERED_EXAM=$((COVERED_EXAM + 1))
    elif health_scope_covers "$pkg" "$kind" "$binary" "$qualified"; then
      echo "COVERED-BY-HEALTH-SCOPE: $qualified ($loc)"
      COVERED_HEALTH=$((COVERED_HEALTH + 1))
    else
      echo "UNCOVERED: $qualified ($loc) [tag=$tag, pkg=$pkg]"
      UNCOVERED=$((UNCOVERED + 1))
    fi
  done
done < <(find "$CRATES_ROOT" -type f -name '*.rs' -print0 | sort -z)

TOTAL=$((COVERED_EXAM + COVERED_HEALTH + EXCEPTIONS + UNCOVERED))
printf '\n%d tests checked: %d covered-by-exam, %d covered-by-health-scope, %d legitimate exceptions, %d UNCOVERED\n' \
  "$TOTAL" "$COVERED_EXAM" "$COVERED_HEALTH" "$EXCEPTIONS" "$UNCOVERED"

# A guard that silently "passes" because it found nothing to check is worse
# than no guard at all (e.g. CRATES_ROOT pointed somewhere with no .rs files,
# or the #[ignore = "..."] pattern stopped matching). Treat zero as a hard
# failure, not a clean pass.
if ((TOTAL == 0)); then
  echo "FAIL: found zero #[ignore = \"...\"]d tests under $CRATES_ROOT — the scan is broken, not clean." >&2
  exit 1
fi

# Cross-check against crates/CLAUDE.md's own raw audit grep (`grep -rn '#\[ignore =
# "' crates/ | grep -v ':[0-9]*: *//'`). The walk above only recognizes
# `<pkg>/tests/<bin>.rs` and `<pkg>/src/**.rs` — a future #[ignore]d test
# placed under e.g. `<pkg>/examples/` or `<pkg>/benches/` would silently
# fall through the `continue` above and vanish from TOTAL instead of
# reporting UNCOVERED. This catches that gap by demanding the two counts
# agree, rather than trusting the walk's own path patterns are exhaustive.
RAW_COUNT=$(grep -rn '#\[ignore = "' "$CRATES_ROOT" | grep -vc ':[0-9]*: *//' || true)
if ((RAW_COUNT != TOTAL)); then
  echo "FAIL: raw audit grep found $RAW_COUNT #[ignore = \"...\"] attributes under $CRATES_ROOT but the walk only classified $TOTAL — a test likely sits outside <pkg>/tests/ or <pkg>/src/ and is silently unchecked. Widen the walk's path patterns." >&2
  exit 1
fi

if ((UNCOVERED > 0)); then
  echo "FAIL: $UNCOVERED ignored test(s) reachable by neither exam.yml nor health.yml — see UNCOVERED lines above." >&2
  exit 1
fi

echo "PASS: every env-gate:/heavy:-tagged #[ignore]d test is reachable by exam.yml or health.yml."
