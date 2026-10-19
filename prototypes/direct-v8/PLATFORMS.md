# Phase 3 platform and interoperability ledger

Tracking: #4302 (after #4301). This work follows
`CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions`.
Owner of this ledger: the opt-in direct-V8 lane (PR #4307 branch). Nothing here
changes the default engine or bundler, publishes packages, or dispatches Actions.

## Shipping targets

The rows mirror the `release.yml` build matrix. Release currently builds the
Linux rows with `cross` (arm64 under QEMU) and the others on native runners;
that is the release recipe, not native qualification evidence for this candidate.

| Target | Native host | Dependency resolution (Cloud) | Native build, link, load | CLI lifecycle | Packaging / installed consumer | Size / provenance |
| --- | --- | --- | --- | --- | --- | --- |
| `x86_64-unknown-linux-gnu` | Linux x86_64 | Pass, 491 rows | Phase 2 Linux evidence in PR #4307 only; NOT RUN for the merged head | NOT RUN for the merged head | NOT RUN | Blocked: lock not admitted |
| `aarch64-unknown-linux-gnu` | Linux arm64 | Pass, 489 rows | NOT RUN | NOT RUN | NOT RUN | NOT RUN |
| `aarch64-apple-darwin` | Apple Silicon | Pass, 476 rows | NOT RUN | NOT RUN | NOT RUN | Blocked: lock not admitted |
| `x86_64-apple-darwin` | Intel Mac (not Rosetta) | Pass, 478 rows | NOT RUN | NOT RUN | NOT RUN | NOT RUN |
| `x86_64-pc-windows-msvc` | Windows x64 | Pass, 496 rows | NOT RUN | NOT RUN (no Windows observer) | NOT RUN | NOT RUN |

Dependency resolution means `record-graph.py --cli --target <triple>` found one
`v8 v147.4.0` engine and no `deno_*` or `serde_v8` crate in the selected
normal/build closure. It builds, links and executes nothing, so it is not a
native row. The same check with `--extra-feature rolldown-prototype` also passes
for every target (Linux x64 599, Linux arm64 596, Darwin arm64 584, Darwin x64
587, Windows 605 rows). Source for these rows: the main merge into the #4307
lane (see the PR comment for the exact commit); Cargo.lock SHA-256
`07fa645050cc0db69ad75f2bc5b824a7cd558dc101f3619089e83d397003e4f9`.

## Recording a native row

On the matching host, from a clean checkout of the candidate commit:

```sh
export ZFB_V8_ICU_DATA="$PWD/target/direct-v8-inputs/icu77/icudtl.dat"   # prepare-icu.py
cargo build --locked -p zfb --no-default-features --features direct-v8 --bin zfb
python3 prototypes/direct-v8/qualify-target.py --target <triple> --binary target/debug/zfb \
  --source-sha "$(git rev-parse HEAD)" --output target/direct-v8-qualify-<triple>
```

`qualify-target.py` refuses (exit 3, row `NOT RUN`) when the host is not native
for the target, including Rosetta. It records source cleanliness, toolchain,
binary hash/version, ICU hash, dynamic library dependencies, the target's
dependency closure, the actual CLI build/dev lifecycle observer, and that the
binary bytes did not change. A check it cannot perform is `NOT RUN`/`PARTIAL`,
never a pass. Heavy builds still go through the machine-wide heavy guard, and
the Mac-specific limits in the root `CLAUDE.md` apply.

`--rolldown` qualifies a binary built with `direct-v8,rolldown-prototype` and runs
the lifecycle observer with `ZFB_ROLLDOWN_PROTOTYPE=1`. That observer is a small
build/dev smoke. The selected Rolldown coverage in #4302 (aliases and virtual
sources, copied Wasm, source maps, Worker output, hydration/CSS, declared
workers) still needs its own cases and stays NOT RUN.

## Known gaps before a full row can pass

- Windows: `check-cli-lifecycle.py` relies on POSIX process groups and SIGINT.
  A Windows observer (console Ctrl-Break or job objects) is needed first.
- Packaging: no harness yet assembles a local carrier package around the opt-in
  binary and runs the existing launcher in a clean consumer. The existing
  `scripts/smoke-packed-clean-room.sh` is the starting point.
- Size and provenance: the Cargo.lock delta (`zfb-js-host` adds the external
  `deno_core` edge) is not admitted by the Darwin/Linux contracts. Admission
  belongs to the security/Darwin evidence lane and the owner.
- Phase 2 evidence on the pre-merge head `e467b9b4` is not reused for the merged
  head's native rows.
