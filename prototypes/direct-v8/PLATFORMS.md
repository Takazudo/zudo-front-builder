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
| `x86_64-unknown-linux-gnu` | Linux x86_64 | Pass, 491 rows | Pass at `bd4feaf` (Cloud Linux, Rust 1.97.0) and at `f217f4d` (opt-in CI lane, Rust 1.99.1); rerun per head | Pass at both (6 rows); Rolldown variant FAIL (see below) | Packed clean-room consumer pass at `f217f4d` (CI lane); shipping GLIBC floor NOT RUN | Blocked: lock not admitted |
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
python3 prototypes/direct-v8/prepare-icu.py
export ZFB_V8_ICU_DATA="$PWD/target/direct-v8-inputs/icu77/icudtl.dat"
python3 prototypes/direct-v8/qualify-target.py --target <triple> --build \
  --guard "$HOME/.claude/scripts/heavy-guard.sh" \
  --source-sha "$(git rev-parse HEAD)" --output target/direct-v8-qualify-<triple>
```

`qualify-target.py` refuses (exit 3, row `NOT RUN`) when the host is not native
for the target: a mismatched host, Rosetta, an arm64 process reporting an x86
CPU (QEMU user mode, best effort), or a binary header for another
architecture. `--build` runs the documented cargo build only after the source
check passes (exact HEAD, no modified or untracked files), so the binary is tied
to that source; a supplied `--binary` leaves the row PARTIAL at best. The row
records toolchain, binary hash/header/version, the pinned ICU hash, dynamic
library dependencies, the target's dependency closure, the actual CLI build/dev
lifecycle observer with its logs, and that the binary bytes did not change.
A check it cannot perform or finish is `NOT RUN`/`TIMEOUT` and makes the row
`PARTIAL`, never `PASS`. On a low-memory Mac set `CARGO_BUILD_JOBS=1` for the
build, per the root `CLAUDE.md`.

`--rolldown` qualifies `direct-v8,rolldown-prototype`: it builds that selection,
requires the binary to contain the Rolldown prototype crate (and refuses it
otherwise), and runs the lifecycle observer with `ZFB_ROLLDOWN_PROTOTYPE=1`. That observer is a small
build/dev smoke. The selected Rolldown coverage in #4302 (aliases and virtual
sources, copied Wasm, source maps, Worker output, hydration/CSS, declared
workers) still needs its own cases and stays NOT RUN.

## Opt-in CI lane

`.github/workflows/direct-v8.yml` records the Linux x86_64 row on a standard
GitHub-hosted runner. It runs only for a PR labelled `direct-v8` (and
`direct-v8-rolldown` for the Rolldown variant), or by manual dispatch once the
workflow is on the default branch. It is not a required check and never runs on
an unlabelled PR. Each run executes the offline controls, the five dependency
resolution rows, and `qualify-target.py --build` against the PR head SHA (not
the merge ref). It uploads `row.json`, `graph.json`, the build log and the
lifecycle logs, kept for 14 days. A CI row is evidence for that exact SHA only.
Linux arm64, Darwin and Windows native rows are not part of this lane.

## Rolldown findings

Linux x86_64, `direct-v8,rolldown-prototype` CLI built by the harness at
`bd4feaf` (SHA-256 `cefba156fc4b2895490553d1211075aad244ab7b9fda7280177442d321b4dfda`,
prototype crate present), run with `ZFB_ROLLDOWN_PROTOTYPE=1`: lifecycle
**FAIL**. Typed-config build and initial dev SSR pass; the failed-replacement row
times out waiting for the page's own error.

Minimal failing case: a page module whose first statement is an unconditional
top-level `throw` (`throw new Error('X'); export const prerender = false;
export default function Page() {...}`). Rolldown drops every declaration after
the throw but still emits them in the export list:

```js
throw new Error("CLI_REPLACEMENT_REJECTED");
//#endregion
export { entry_default as default, routes };
var entry_default;
```

`routes` is never declared, so the module fails to link
(`SyntaxError: Export 'routes' is not defined in module`) before the author's
error can run. The dev server keeps the previous renderer, so continuity holds,
but the diagnostic is lost. Any engine rejects this module at link time, so this
is Rolldown output, not a direct-V8 behavior. The esbuild default is unaffected.
The fix belongs to the Rolldown bundler owners (#318, #4123, #4124). This lane
records the case and does not change the bundler.

## Known gaps before a full row can pass

- Windows: `check-cli-lifecycle.py` relies on POSIX process groups and SIGINT.
  A Windows observer (console Ctrl-Break or job objects) is needed first.
- Packaging: the CI lane runs `scripts/smoke-packed-clean-room.sh` around the
  harness-built Linux x86_64 binary (local tarballs, `create-zfb` scaffold,
  site build). That binary is built natively on the runner, not with the
  shipping `cross` recipe, so it needs GLIBC 2.39 at `f217f4d`; the shipping
  GLIBC 2.34 floor for a direct-v8 binary is not qualified. Other carriers
  are NOT RUN.
- Size and provenance: the Cargo.lock delta (`zfb-js-host` adds the external
  `deno_core` edge) is not admitted by the Darwin/Linux contracts. Admission
  belongs to the security/Darwin evidence lane and the owner.
- Phase 2 evidence on the pre-merge head `e467b9b4` is not reused for the merged
  head's native rows.

## Native row run prompts (not authorized here)

Each prompt runs on one owner-approved host. `<CANDIDATE_SHA>` is the exact
#4307 head named in the #4302 comment that hands this off; never substitute a
newer head. No Actions dispatch, paid runner, publish or contract change.

Common steps, from a fresh clone at the candidate:

```sh
git checkout --detach <CANDIDATE_SHA> && git status --porcelain   # must print nothing
pnpm install --frozen-lockfile
python3 prototypes/direct-v8/prepare-icu.py
export ZFB_V8_ICU_DATA="$PWD/target/direct-v8-inputs/icu77/icudtl.dat"
python3 -m unittest discover -s prototypes/direct-v8 -p 'test_*.py'   # offline controls
```

| Host | Command | Expected | Stop and report |
| --- | --- | --- | --- |
| Linux arm64 (native, not QEMU) | `python3 prototypes/direct-v8/qualify-target.py --target aarch64-unknown-linux-gnu --build --guard <heavy-guard> --source-sha <CANDIDATE_SHA> --output target/direct-v8-qualify-linux-arm64` | exit 0, `PASS`; 10 checks true; lifecycle 6 rows; graph 489 rows | exit 3 (not native), any `false` check, build failure (keep `build.log`) |
| Apple Silicon Mac | same with `--target aarch64-apple-darwin`, `CARGO_BUILD_JOBS=1` | exit 0, `PASS`; graph 476 rows; `otool -L` lists only system libraries | exit 3, `false` check, compile timeout (= build incomplete, tests not run) |
| Intel Mac (not Rosetta) | same with `--target x86_64-apple-darwin`, `CARGO_BUILD_JOBS=1` | exit 0, `PASS`; graph 478 rows | exit 3 (Rosetta or arm64 binary), `false` check |
| Windows x64 | same with `--target x86_64-pc-windows-msvc` | exit 1, `PARTIAL` with lifecycle `NOT RUN` (no Windows observer yet); every other check true; graph 496 rows | any `false` check, build failure |

Return the whole output directory (`row.json`, `graph.json`, `build.log`,
`lifecycle/`) plus `uname -a`/`sw_vers`/`systeminfo` output. Rerun only with a
new output directory and keep the first attempt.
