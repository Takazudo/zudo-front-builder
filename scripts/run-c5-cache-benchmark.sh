#!/usr/bin/env bash
# Run issue #2727's 18 measured cells plus two unmeasured warm primes.

set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <evidence-dir>" >&2
    exit 2
fi

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
EVIDENCE_DIR=$1
TARGET_DIR=${ZFB_C5_TARGET_DIR:-/tmp/zfb-2726-target}
JUNIT_PATH=${ZFB_C5_JUNIT_PATH:-"$TARGET_DIR/nextest/ci/junit.xml"}
SAMPLES=3
RESULTS="$EVIDENCE_DIR/results.tsv"
PRIMES="$EVIDENCE_DIR/primes.tsv"
RUN_BINARIES=${ZFB_C5_BINARIES:-"build_cleans_outdir build_package_routes"}
read -r -a BINARIES <<<"$RUN_BINARIES"

if [[ ${#BINARIES[@]} -eq 0 ]]; then
    echo "ZFB_C5_BINARIES must name at least one assigned test binary" >&2
    exit 2
fi
for binary in "${BINARIES[@]}"; do
    case "$binary" in
        build_cleans_outdir|build_package_routes) ;;
        *)
            echo "unsupported benchmark binary: $binary" >&2
            exit 2
            ;;
    esac
done

mkdir -p "$EVIDENCE_DIR"
if [[ ${ZFB_C5_APPEND_RESULTS:-0} != 1 || ! -f "$RESULTS" ]]; then
    printf 'binary\tcell\tsample\troot\tseconds\tstatus\n' >"$RESULTS"
fi
if [[ ${ZFB_C5_APPEND_RESULTS:-0} != 1 || ! -f "$PRIMES" ]]; then
    printf 'binary\troot\tseconds\tstatus\n' >"$PRIMES"
fi

assert_cache_populated() {
    local root=$1
    local cache_root="$root/zfb-embedded-node-modules-cache/schema-1"
    local marker tree
    marker=$(find "$cache_root/done" -maxdepth 1 -type f -name '*.done' -print -quit 2>/dev/null || true)
    tree=$(find "$cache_root/trees" -mindepth 2 -maxdepth 2 -type d -name node_modules -print -quit 2>/dev/null || true)
    if [[ -z "$marker" || -z "$tree" ]]; then
        echo "cache root is not demonstrably populated: $root" >&2
        find "$root" -maxdepth 5 -print 2>/dev/null | sort >&2 || true
        exit 1
    fi
}

assert_empty_root() {
    local root=$1
    local entry
    entry=$(find "$root" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null || true)
    if [[ -n "$entry" ]]; then
        echo "expected fresh empty root: $root" >&2
        find "$root" -maxdepth 2 -print >&2
        exit 1
    fi
}

cache_has_entry() {
    local root=$1
    local cache_root="$root/zfb-embedded-node-modules-cache/schema-1"
    local marker tree
    marker=$(find "$cache_root/done" -maxdepth 1 -type f -name '*.done' -print -quit 2>/dev/null || true)
    tree=$(find "$cache_root/trees" -mindepth 2 -maxdepth 2 -type d -name node_modules -print -quit 2>/dev/null || true)
    [[ -n "$marker" && -n "$tree" ]]
}

new_root() {
    local label=$1
    mktemp -d "/tmp/zfb-2727-c5-${label}.XXXXXX"
}

run_invocation() {
    local binary=$1
    local cell=$2
    local sample=$3
    local root=$4
    local evidence_binary=${5:-$binary}
    local id="${evidence_binary}.${cell}.${sample}"
    local junit="$JUNIT_PATH"
    local copy="$EVIDENCE_DIR/${id}.junit.xml"
    local log="$EVIDENCE_DIR/${id}.nextest.log"
    local meta="$EVIDENCE_DIR/${id}.meta"
    local status sum

    case "$cell" in
        on-cold)
            assert_empty_root "$root"
            ;;
        on-warm)
            assert_cache_populated "$root"
            ;;
        off)
            assert_empty_root "$root"
            ;;
        prime)
            assert_empty_root "$root"
            ;;
        *)
            echo "unknown cell: $cell" >&2
            exit 2
            ;;
    esac

    rm -f "$junit"
    {
        printf 'binary=%s\nevidence_binary=%s\ncell=%s\nsample=%s\nroot=%s\n' \
            "$binary" "$evidence_binary" "$cell" "$sample" "$root"
        printf 'git_head=%s\n' "$(git -C "$ROOT_DIR" rev-parse HEAD)"
        printf 'nextest=%s\n' "$(cargo nextest --version | head -1)"
        printf 'cargo_target_dir=%s\ncargo_build_jobs=4\n' "$TARGET_DIR"
        printf 'started_utc=%s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    } >"$meta"

    set +e
    if [[ "$cell" == off ]]; then
        env -u ZFB_EMBEDDED_NODE_MODULES_CACHE \
            TMPDIR="$root" CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
            cargo nextest run --profile ci -p zfb --test "$binary" --no-fail-fast \
            >"$log" 2>&1
    else
        TMPDIR="$root" CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$TARGET_DIR" \
            ZFB_EMBEDDED_NODE_MODULES_CACHE=1 \
            cargo nextest run --profile ci -p zfb --test "$binary" --no-fail-fast \
            >"$log" 2>&1
    fi
    status=$?
    set -e

    printf 'finished_utc=%s\nstatus=%s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$status" >>"$meta"
    if [[ $status -ne 0 ]]; then
        echo "invocation failed ($id); see $log" >&2
        [[ -f "$junit" ]] && cp "$junit" "$copy"
        exit "$status"
    fi
    [[ -f "$junit" ]] || { echo "missing JUnit: $junit" >&2; exit 1; }
    cp "$junit" "$copy"
    sum=$("$ROOT_DIR/scripts/sum-junit-times.sh" "$copy")
    if [[ "$cell" == on-cold ]]; then
        if cache_has_entry "$root"; then
            printf 'cache_after=populated\n' >>"$meta"
        else
            printf 'cache_after=empty\n' >>"$meta"
        fi
    fi
    if [[ "$cell" == prime ]]; then
        printf '%s\t%s\t%s\t%s\n' "$evidence_binary" "$root" "$sum" "$status" >>"$PRIMES"
    else
        printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
            "$evidence_binary" "$cell" "$sample" "$root" "$sum" "$status" >>"$RESULTS"
    fi
    printf 'seconds=%s\n' "$sum" >>"$meta"
    echo "completed $id: ${sum}s"
}

prime_warm_root() {
    local binary=$1
    local root=$2
    if [[ "$binary" == build_package_routes ]]; then
        # Every route fixture pre-links node_modules, so the C4 call-site guard
        # deliberately bypasses the cache. Reuse the assigned binary only as a
        # control, and use the sibling build binary to publish the same vendor
        # cache key into this route binary's dedicated warm parent.
        run_invocation build_cleans_outdir prime 0 "$root" build_package_routes-via-build_cleans_outdir
    else
        run_invocation "$binary" prime 0 "$root"
    fi
    assert_cache_populated "$root"
}

for binary in "${BINARIES[@]}"; do
    warm_root=$(new_root "${binary}-warm")
    prime_warm_root "$binary" "$warm_root"

    for sample in $(seq 1 "$SAMPLES"); do
        case "$sample" in
            1) cells=(off on-cold on-warm) ;;
            2) cells=(on-cold on-warm off) ;;
            3) cells=(on-warm off on-cold) ;;
        esac
        for cell in "${cells[@]}"; do
            if [[ "$cell" == on-warm ]]; then
                root="$warm_root"
            else
                root=$(new_root "${binary}-${cell}-s${sample}")
            fi
            run_invocation "$binary" "$cell" "$sample" "$root"
        done
    done
done

echo "benchmark complete; results: $RESULTS"
