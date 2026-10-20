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
| `x86_64-unknown-linux-gnu` | Linux x86_64 | Pass, 491 rows | Pass at `bd4feaf` (Cloud Linux, Rust 1.97.0) and at `f217f4d` (opt-in CI lane, Rust 1.99.1); rerun per head | Pass at both (6 rows); Rolldown variant FAIL at `349b112` (see below) | Shipping `cross` release binary needs at most GLIBC 2.34 and passes the packed clean-room consumer at `89f1281` (CI lane) | Blocked: lock not admitted |
| `aarch64-unknown-linux-gnu` | Linux arm64 | Pass, 489 rows | Pass at `fa1aaf6`, `89f1281` and `349b112` (CI lane, `ubuntu-24.04-arm`, Rust 1.99.1) | Pass (6 rows); Rolldown variant FAIL at `349b112` (same case as x64) | NOT RUN | NOT RUN |
| `aarch64-apple-darwin` | Apple Silicon | Pass, 476 rows | NOT RUN | NOT RUN | NOT RUN | Blocked: lock not admitted |
| `x86_64-apple-darwin` | Intel Mac (not Rosetta) | Pass, 478 rows | NOT RUN | NOT RUN | NOT RUN | NOT RUN |
| `x86_64-pc-windows-msvc` | Windows x64 | Pass, 496 rows (494 resolved on the Windows host) | Pass at `fa1aaf6`, `89f1281` and `349b112` (CI lane, `windows-latest`, Rust 1.99.1); row PASS at `349b112` (PARTIAL before the Windows observer) | Pass at `349b112` (6 rows, console Ctrl-C exit 0); Rolldown variant FAIL at `349b112` (see below) | Native `cargo` release binary passes the packed clean-room consumer at `349b112` (CI lane) | NOT RUN |

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
build/dev smoke.

## CLI interoperability cases

`check-cli-cases.py` runs the actual binary over `site/` and small fixtures it
writes itself, one row per case, and records every row even when an earlier one
fails. The CI lane runs it on every host after the row, under the default
bundler and, on the Rolldown leg, with `--rolldown` (`ZFB_ROLLDOWN_PROTOTYPE=1`).
Any failing row turns that step red. The browser rows need an installed
Chromium (`--chromium`, installed by the lane with Playwright, never downloaded
by the harness); without one they are listed as NOT RUN, not passed.

| Row | What it proves |
| --- | --- |
| `build-islands-css` | island marker, CSS module class defined in an emitted stylesheet, emitted islands script |
| `build-mdx-content` | collection route rendered from MDX |
| `build-wasm-ssr` | an SSR page instantiates its imported Wasm module |
| `build-wasm-copied` | the Wasm module is copied beside `bundle.mjs`, which references it |
| `build-source-map` | the SSR bundle's linked map names an authored page |
| `build-source-map-positions` | authored text in the SSR bundle (`CSS module marker`, `Count: `) maps back to its authored file and to a line inside the JSX element holding it |
| `browser-hydration` | Chromium loads the built site: the Counter island renders `Count: 0`, its CSS module colour applies, a click makes it `Count: 1`, with no page or console errors |
| `build-tsconfig-alias` | a page import through a tsconfig `paths` alias |
| `build-plugin-virtual-module` | a `zfb.config.ts` plugin's `addVirtualModule` value reaches both the SSR page and a client script importing it |
| `build-client-script` | a `*.client.ts` entry outside any island is emitted once as `assets/client/case-<hash>.js` and the page's `clientScript()` URL is rewritten to it |
| `browser-client-script` | Chromium runs that client script, which writes the virtual module's value into the page |
| `build-module-worker` | an island's `new URL(..., import.meta.url)` worker is emitted once and referenced with its `?v=` digest |
| `build-missing-export-rejected` | negative control: a missing export fails the build with a missing-export diagnostic naming it |
| `dev-ssr-request` | `zfb dev` renders a `prerender = false` page afresh for each request and serves the MDX and Wasm routes, then exits 0 on Ctrl-C/SIGINT |

First run of the original 9 rows at `060cd31` (run 39409736005 default,
39409734870 Rolldown):

| Host | Default bundler | Rolldown (`--rolldown`) |
| --- | --- | --- |
| Linux x86_64 | 9/9 PASS (`zfb` `da781dd7…`) | 9/9 PASS (`d5445603…`) |
| Linux arm64 | 9/9 PASS (`1ddeece7…`) | 9/9 PASS (`2d1bd5aa…`) |
| Windows x64 | 9/9 PASS (`zfb.exe` `c8f6ea05…`) | 0/9: every build stops on the `\\?\` verbatim path, in the islands entry's import (`UNRESOLVED_IMPORT '\\?\C:\…\components\counter.tsx'`) as well as the SSR entry (`UNRESOLVED_ENTRY //?/C:/…/entry.mjs`); so the missing-export control never reaches its diagnostic and the dev server serves only its 404 page |

All 14 rows at `f01c149` (run 39443278121 default, 39443291442 Rolldown):

| Host | Default bundler | Rolldown (`--rolldown`) |
| --- | --- | --- |
| Linux x86_64 | 14/14 PASS (`zfb` `839cf4f8…`, Chromium 149.0.7827.55) | 12/12 PASS (`04a28378…`); both browser rows NOT RUN |
| Linux arm64 | 14/14 PASS (`c76be595…`, Chromium 149.0.7827.0) | 12/12 PASS (`b90585f9…`); both browser rows NOT RUN |
| Windows x64 | 11/14 (`zfb.exe` `45ed6528…`): the plugin build fails, so `build-plugin-virtual-module` fails and the two client-script rows, which share that fixture, cannot run their checks; the other 11, `browser-hydration` included, PASS | 0/12, the verbatim path as above; both browser rows NOT RUN |

The Windows plugin failure is the direct-V8 host, not Rolldown: `renderer step
failed … embedded V8 host: no in-memory source for
file:///?/C:/…/zfb-bundler-*/.zfb-virtual-*.mjs`. The host was asked for
the virtual module's file under the `\\?\` verbatim shadow directory, and that
URL matches neither its in-memory modules nor its alias targets (read from the
message and `crates/zfb-js-host/src/modules.rs`; not reproduced on a Windows
host here). The same
verbatim directory reaches the shipped Windows `bundle.mjs.map`, whose sources
read `file:////%3F/C:/…/zfb-bundler-*/pages/index.tsx` (positions are still
right). The browser rows were NOT RUN on the Rolldown legs because the Chromium
install step was skipped after the failed lifecycle row; that step, and the
Linux packaging steps the packed consumer with Rolldown needs, now run after a
failed row too.

Darwin is NOT RUN. On the Rolldown leg the lane also builds the Linux x86_64
packed carrier's scaffolded site with `packed-consumer.py --rolldown`, which
requires the Rolldown backend's own `[zfb-timing] rolldown-prototype:` line as
proof that Rolldown bundled it. That is not run on Windows, where every Rolldown
build already stops on the verbatim path above. Still not covered by any
actual-CLI case: source maps for client or island bundles (production ships
none), and failure and replacement under Rolldown beyond the lifecycle observer,
which stops at the known `routes` case below.

## Opt-in CI lane

`.github/workflows/direct-v8.yml` records native rows on standard
GitHub-hosted runners: Linux x86_64 (`ubuntu-latest`), Linux arm64
(`ubuntu-24.04-arm`) and Windows x64 (`windows-latest`). Its row jobs run only for a PR labelled `direct-v8` (and
`direct-v8-rolldown` for the Rolldown variant), or by manual dispatch once the
workflow is on the default branch. It is not a required check. On other PRs only
a small `plan` job runs and the row job reports as skipped. Each row run:

- runs the offline controls and the five dependency resolution rows;
- runs `qualify-target.py --build` against the PR head SHA (not the merge ref);
- records the native binary's highest GLIBC symbol (not gated: the binary is
  built on the runner, not with the shipping `cross` recipe);
- on Linux x86_64, builds the release binary with the shipping `cross` recipe
  (Amazon Linux 2023 image) and fails if it needs more than GLIBC 2.34, then
  runs `scripts/smoke-packed-clean-room.sh` with that binary and the
  `cross`-built CSS addon (local tarballs, `create-zfb` scaffold, site build);
- on Windows x64, builds the release binary and CSS addon natively with cargo,
  as `release.yml` does but with the opt-in direct-v8 selection and pinned ICU
  that `release.yml` does not build, and runs `packed-consumer.py` (the cross-platform
  counterpart of the smoke script, which stays the Linux required check).

On Windows the lifecycle observer stops `zfb dev` with a real console Ctrl-C:
`console-ctrl-c.py` runs it on a private hidden console, because Windows cannot
deliver Ctrl-C to a process group and Ctrl-Break would end `zfb dev` without
its shutdown path.

It uploads the graph rows, `row.json`, the build, lifecycle and packed
clean-room logs, the CLI case reports and logs, the GLIBC record and disk snapshots, kept for 14 days. The
runner uses the current stable Rust, so the exact toolchain of a row comes from
its `row.json`, and a CI row is evidence for that exact SHA and toolchain only.
Darwin rows need a Mac runner, which is not approved, and are not part of this
lane; nor is the arm64 shipping `cross` recipe.

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
records the case and does not change the bundler. At `349b112` the CI lane
reproduces it on Linux x64 and Linux arm64.

Windows x64 at `349b112` (CI lane): `zfb build` fails before the lifecycle
cases with `[UNRESOLVED_ENTRY] Cannot resolve entry module
//?/C:/Users/runneradmin/AppData/Local/Temp/zfb-bundler-*/entry.mjs`. The
shadow parent is canonicalized (`usable_shadow_parent_candidate` in
`crates/zfb-build/src/bundler.rs`), which on Windows yields a `\\?\` verbatim
path; esbuild accepts it (the default row passes), but the Rolldown request
decoder passes it on as the entry and cwd and Rolldown cannot resolve it. A
likely fix is to drop the verbatim prefix for disk paths in the prototype's
request decoder; it belongs to the same owners and is not applied here.

## Known gaps before a full row can pass

- Packaging: Linux x86_64 (shipping `cross` recipe, GLIBC 2.34 at `89f1281`)
  and Windows x64 (native release recipe at `349b112`) have packed clean-room
  consumer rows. The arm64 `cross` recipe (QEMU) and the Darwin carriers are
  NOT RUN.
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
| Windows x64 | same with `--target x86_64-pc-windows-msvc` | exit 0, `PASS`; 10 checks true; lifecycle 6 rows; graph 494 rows on the Windows host | any `false` check, build failure |

Return the whole output directory (`row.json`, `graph.json`, `build.log`,
`lifecycle/`) plus `uname -a`/`sw_vers`/`systeminfo` output. Rerun only with a
new output directory and keep the first attempt.
