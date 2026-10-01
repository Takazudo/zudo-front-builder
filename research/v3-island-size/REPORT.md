# V3 island size baseline (#3468)

Measured product source: `897f288c013e24ff4fde01483a55d8a1945fc964` on `base/v3-decisions`; package versions `@takazudo/zfb@3.0.0` and `@takazudo/zfb-runtime@3.0.0`. This is a controlled **current-source** baseline, not a reproduction of the historical #3383 v2/v3 recipes. The source includes the prerequisite server-rendering fix for duplicated Show/For module instances discovered by the real consumer run. No size optimization has been applied in this report.

## Shipped JS from real `zfb build`

| Case | Workspace raw / gzip | Packed raw / gzip | Packed initial | Packed later |
| --- | ---: | ---: | ---: | ---: |
| no-island | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| event-only | 49,830 / 17,004 | 49,892 / 17,027 | 49,892 / 17,027 | 0 / 0 |
| scalar-signal | 49,838 / 17,009 | 49,900 / 17,037 | 49,900 / 17,037 | 0 / 0 |
| show-for | 49,985 / 17,082 | 50,047 / 17,111 | 50,047 / 17,111 | 0 / 0 |
| model | 49,829 / 17,012 | 49,891 / 17,045 | 49,891 / 17,045 | 0 / 0 |
| blog-theme | 50,180 / 17,138 | 50,242 / 17,166 | 50,242 / 17,166 | 0 / 0 |
| json-api | 49,984 / 17,096 | 50,046 / 17,127 | 50,046 / 17,127 | 0 / 0 |
| multi-island | 50,106 / 17,103 | 50,168 / 17,132 | 50,168 / 17,132 | 0 / 0 |

Each of the eight workspace and packed consumer builds was run twice. The runner compared SHA-256 inventories of **every file** in `dist`, including HTML, and failed on any difference. Both paths passed. No-island emitted no islands JS. Every island case emitted one entry and no shared or lazy chunks, so initial equals all shipped and later is zero. The reported gzip size sums per-file `node:zlib` `gzipSync` at level 9, `mtime: 0`; actual HTTP compression may vary.

| Case | Packed real entry | Raw | Gzip | SHA-256 |
| --- | --- | ---: | ---: | --- |
| event-only | `assets/islands-ee166548.js` | 49,892 | 17,027 | `ee166548706a55bc6bfd24679271c55196cef612fa292968f9260736f0a15cd1` |
| scalar-signal | `assets/islands-9865d925.js` | 49,900 | 17,037 | `9865d9250672d851c5a96be799268782b14b85c1281db0c8594211392e6d1792` |
| show-for | `assets/islands-26995749.js` | 50,047 | 17,111 | `269957496c715fd41a9fb176f431a1eb91ebd1f12b69993ed878826d2d2eb083` |
| model | `assets/islands-83d45382.js` | 49,891 | 17,045 | `83d45382ceb9b00dcab9d79c7705af60a9ce2429b0fbb8d8a1f857aa5e2533de` |
| blog-theme | `assets/islands-f0cad77c.js` | 50,242 | 17,166 | `f0cad77c76bb57ef53e345a8bb3dce53f15d6367b156159d68d5cbf0592441d9` |
| json-api | `assets/islands-6e8dbc5b.js` | 50,046 | 17,127 | `6e8dbc5bc2dda6441c57402d7f25f838aed78ba3d4994599b75bdbb4ef02659e` |
| multi-island | `assets/islands-af50e300.js` | 50,168 | 17,132 | `af50e300d1bcf09d2f2b7743c3f6afc256e9a48189ae5ef5cbe296ce21eaf4e4` |

The two-island scalar-plus-theme fixture ships **268 raw / 95 gzip bytes** more than scalar alone in packed mode (workspace: 268 / 94). The single entry deduplicates the shared runtime. Packed builds are consistently 62 raw bytes above workspace builds for each nonempty case; gzip differences are 23–33 bytes. Their module resolution paths differ, so use packed results for a published-consumer gate.

## Provenance and replay

- `@takazudo/zfb` tarball SHA-256: `85a92e2958d538d5831348e58094f1319740dbfcd826a3ee58c6a35ef6c8f93d`; `@takazudo/zfb-runtime` tarball SHA-256: `5d8cb3fec9990118a6d69a32f2c38a6719fc83c40318f31f7e674776848a3acd`. Packed consumers linked their extracted packages, including `dist/zudo-react/client.js` and `dist/server.js`.
- Lockfile-installed `hono@4.12.25` was linked into both consumer modes: package manifest SHA-256 `272ab6b5a27aec8f30fac242dfec9b87dcf69015411affcca7e1c193eab11993`. The packed runtime declares `^4.12.25`.
- Rust `zfb` CLI binary SHA-256: `87e048cc0925adc9718b44803a7a524a933e5c4d3e71e29b952cbbb9c3a21e93`. Its fresh guarded `cargo build -p zfb` passed after one environment timeout and retry. Source TS packages were rebuilt and repacked at the recorded source. The runner checks the SHA syntax and records binary/package digests; it cannot cryptographically prove build provenance from the SHA alone.
- esbuild `0.25.12` binary SHA-256: `3e030ee2aa86ad3c33e5e95ae0e53bb03de40e0da35c9b1180a67de4a497cae5`. Node `v24.14.0`, zlib `1.3.1-e00f703`. `pnpm-lock.yaml` SHA-256 `69f78b69412b6aaff5dfffa53c37fa15fe54e24da713f583f0ecdd0dabdddbfc`; `Cargo.lock` SHA-256 `43326bc0848833aef50bc2df736f502a1052798bca5e199bc5758e7aaef5b60a`.
- Exact install/build/pack/replay commands and fixture definitions are in [README.md](./README.md). Ignored local outputs: `results/real-workspace/`, `results/real-packed/`, `results/modeled-workspace/`, and `results/modeled-packed/`, each with `measurement.json` and `report.md`. Real pass directories preserve emitted HTML and JS; modeled pass directories preserve esbuild `metafile.json`. Re-run the README commands to regenerate them. Fixture SHA-256 values are recorded in both modeled JSON reports.

## Reachability diagnostics from esbuild sidecar

The sidecar reproduces production minification, browser platform, automatic JSX, production/dev defines, splitting, `--keep-names`, and registration semantics, but its entry path and build identity differ from the real CLI. Raw bytes match the real packed bundle for each case; gzip can differ by a few bytes. Use the real CLI table above for the baseline. esbuild `bytesInOutput` is attribution within a bundled output, **not** independent savings from removing a module.

For the packed event-only sidecar, major retained inputs in its `islands.js` output are:

| Input | Retained bytes in output |
| --- | ---: |
| hydrate (`zudo-react/hydrate.js`) | 19,403 |
| forms (`zudo-react/forms.js`) | 7,402 |
| island runtime (`dist/runtime.js`) | 4,955 |
| vocabulary (`zudo-react/vocabulary.js`) | 4,463 |
| structure (`zudo-react/structure.js`) | 939 |

The event-only, scalar, Show/For, and model outputs all retain nearly the same runtime/forms/structure input set. Their packed raw totals vary by only 0–155 bytes from event-only even though the fixture code differs. This shows that the current hydration import graph retains structural and form code for the simplest island; it does **not** establish that these features can be separated safely. The scalar sidecar without global `--keep-names` was 2,815 raw / 1,157 gzip bytes smaller, but that flag affects identity and error behavior across the entire graph and is not an approved optimization.

The measured sidecar metafiles are generated without changing the bundler's private resource metafile, resource read-back, or stage-escape audit. The decision below (#3469) ranks bounded options and specifies the downstream contract.

## Decision (#3469): preserve the measured baseline

**Select zero optimizations. No reduction has been achieved.** The measured graph does not establish a safe, localized removable feature. Proceed with #3470 as verification of this no-product-code-change conclusion, then #3471 as baseline protection. This does not claim that the runtime cannot become smaller; a feature architecture or identity redesign needs its own scope and evidence. No Preact parity target is adopted.

Decision source: `e82190e04021b37f663c32d39fc7ddbfe9a5314e`. Product TS sources, islands bundler sources, and both lockfiles have no diff from measured source `897f288c013e24ff4fde01483a55d8a1945fc964`. Review read all four prerequisite JSON reports, production source, and the packed sidecar attribution. A lightweight artifact audit recomputed raw/gzip/SHA-256 for 56 emitted JS artifacts (seven nonempty fixtures × two passes × four modes) and checked 28 sidecar metafile output totals. All matched. This audit reused saved artifacts; it did not claim a new CLI build or browser run.

### Verification on unchanged product source (#3470)

The guarded real and modeled workspace/packed matrices were replayed at `d2edf94000d4700742aa56ef21f75904e23344f0`, after the #3469 decision. Relative to baseline source `897f288c013e24ff4fde01483a55d8a1945fc964`, the product TS sources, islands esbuild invocations, fixtures, runners, and lockfiles are unchanged. Each real consumer mode built all eight fixtures twice and produced the same full-`dist` inventories as its baseline run. The matrix below gives **all shipped raw / gzip bytes**; `before` is the baseline and `after` is this verification replay.

| Case | Workspace before | Workspace after | Packed before | Packed after |
| --- | ---: | ---: | ---: | ---: |
| no-island | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| event-only | 49,830 / 17,004 | 49,830 / 17,004 | 49,892 / 17,027 | 49,892 / 17,027 |
| scalar-signal | 49,838 / 17,009 | 49,838 / 17,009 | 49,900 / 17,037 | 49,900 / 17,037 |
| show-for | 49,985 / 17,082 | 49,985 / 17,082 | 50,047 / 17,111 | 50,047 / 17,111 |
| model | 49,829 / 17,012 | 49,829 / 17,012 | 49,891 / 17,045 | 49,891 / 17,045 |
| blog-theme | 50,180 / 17,138 | 50,180 / 17,138 | 50,242 / 17,166 | 50,242 / 17,166 |
| json-api | 49,984 / 17,096 | 49,984 / 17,096 | 50,046 / 17,127 | 50,046 / 17,127 |
| multi-island | 50,106 / 17,103 | 50,106 / 17,103 | 50,168 / 17,132 | 50,168 / 17,132 |

Every nonempty real build still has one initial entry and no later chunk, so initial equals all shipped and later remains 0 / 0. No-island still emits no JS. The fresh replay's 28 emitted JS artifacts (seven nonempty fixtures × two passes × two modes) match their recorded raw, gzip, and SHA-256 values. Both modeled result sets, including the scalar `--keep-names` probe, are identical to baseline. In event-only, scalar, Show/For, and model, both modeled modes still retain hydrate, forms, runtime, vocabulary, and structure; the packed event-only attribution remains 19,403, 7,402, 4,955, 4,463, and 939 bytes respectively. This is retained-byte attribution, not removable savings.

The replay used `@takazudo/zfb@3.0.0` and `@takazudo/zfb-runtime@3.0.0` tarballs with SHA-256 `85a92e2958d538d5831348e58094f1319740dbfcd826a3ee58c6a35ef6c8f93d` and `5d8cb3fec9990118a6d69a32f2c38a6719fc83c40318f31f7e674776848a3acd`. The fresh CLI digest was `87e048cc0925adc9718b44803a7a524a933e5c4d3e71e29b952cbbb9c3a21e93`, esbuild `0.25.12` digest `3e030ee2aa86ad3c33e5e95ae0e53bb03de40e0da35c9b1180a67de4a497cae5`, with Node `v24.14.0`, zlib `1.3.1-e00f703`, Hono `4.12.25`, and unchanged recorded fixture/runner/lockfile hashes. Ignored replay artifacts are in `results/verification-3470-{real,modeled}-{workspace,packed}/`. The seven focused test files passed (163 tests). No product code change or byte reduction was selected; the zero-growth ceilings remain those in `decision.json`.

### Ranked options and rejection evidence

These are retained-byte rankings, not additive removable-byte estimates. The event-only packed sidecar is the reference. Inspecting event, scalar, Show/For, and model graphs confirms the same core module set.

| Candidate / exact scope | Observed contribution | Decision and correctness risk |
| --- | ---: | --- |
| `packages/zfb/src/zudo-react/hydrate.ts`: `render`, `element`, `compare`, `compareChildren`, `validate`, `execute` | 19,403 raw | Reject removal/specialized lightweight hydration. Parser position rules, preflight comparison, fail-closed ownership and adoption are required even for event-only islands. Runtime component output is not statically bounded by the fixture's imports. No independently measured safe carve-out exists. |
| `packages/zfb/src/zudo-react/forms.ts`: `prepareForms`, `reconcileForms`, `activateForms` | 7,402 raw | Defer feature isolation. `execute`, structural replacements, and `installOwned` call these paths; `element` gathers form plans for every element. Separating model bindings still must preserve uncontrolled form adoption, reset/radio ownership, model/IME, cleanup, and dynamically created controls. Module contribution is not demonstrated savings. |
| `packages/zfb/src/runtime.ts`: scheduling, ownership and dev warning branches | 4,955 raw | Reject wholesale removal; scheduling, identity, props and root disposal are production behavior. Dev `console.warn` text is already absent from the packed bundle. Four guarded `process.env` reads remain across this module and `types.ts`; one is inside `warnIfNestedIsland`. They are not a demonstrated safe win: moving the define-able expression ahead of the guards can throw in unbundled browsers without `process`; sharing/caching a dev flag changes when environment values are read. No numeric saving is asserted for an untested rewrite. |
| `packages/zfb/src/zudo-react/vocabulary.ts`: tag/attribute sets, `isDialectProp`, `dialectSuggestion`, `reactiveModelSuggestion` | 4,463 raw | Reject production stripping. The tables enforce supported parser/attribute and dialect rules, with error suggestions shared by SSR and client. `root.ts` reporting (1,055 raw) also remains live production behavior: event/update/cleanup failures and custom reporters. These are not dev-only imports. A representation-only table refactor is a deferred measurement candidate, not an approved change. |
| Global `--keep-names` in the islands esbuild invocation | 2,815 raw / 1,157 gzip probe delta, scalar only | Reject. This is the only measured removal delta, and it changes the name protocol. Scanner/SSR/client `displayName ?? name`, generated registration, diagnostics, and minified identity tests must agree. Stable-ID redesign/API changes are outside this epic. |
| `packages/zfb/src/zudo-react/structure.ts`: Show/For, keyed validation and view | 939 raw | Reject removal or lazy split. `hydrate.ts` directly dispatches Show/For, owns replacement scopes and preserves focus/selection during keyed moves. A lazy chunk must remain in the all-shipped total and does not establish a total-byte saving. |

Two bounded research candidates may be proposed later: (1) preserve all guarded-environment behavior while eliminating residual dev-branch reachability in `runtime.ts`/`types.ts`; (2) compress the internal vocabulary table representation while preserving every accepted/rejected prop and diagnostic. Each needs an isolated before/after probe and behavior checks before selection. Neither authorizes implementation in #3470. Larger form isolation and stable-ID redesign remain separate future work.

### Numeric acceptance and reproducibility allowance

The **target and ceiling are the current raw/gzip values** in the first table, individually for all eight fixtures and for both workspace and packed consumers. The reduction target is **0 bytes**, and the allowed growth is **0 raw / 0 gzip bytes per fixture**. No-island must remain exactly 0 / 0 and emit no JavaScript. [decision.json](./decision.json) records every ceiling, the fixture version/hashes, runner hashes, compression, and toolchain. `check-budget.mjs` enforces this contract from the complete saved artifacts. Packed numbers govern the published consumer; workspace numbers additionally prevent resolution-path regressions.

The zero allowance follows two byte-identical full-dist builds per fixture. There is no observed run-to-run variation from which to justify a percentage or fixed padding. The 62 raw-byte workspace/packed gap is a different resolution graph, and sidecar gzip differences are a different generated entry; neither is noise allowance. A decrease may pass a ceiling but must still receive behavioral review. Count every emitted entry/shared/lazy JS file once, gzip each file before summing, and report initial/later/all-shipped separately. Moving bytes into later chunks cannot satisfy the gate.

Pin Node `24.14.0`, its recorded zlib `1.3.1-e00f703`, esbuild `0.25.12`, lockfile-installed Hono `4.12.25`, and fixture version `v3-islands-1`. Build and pack both matching `3.0.0` packages from the tested source. Tarball/CLI/esbuild digests are run provenance, not permanent digest ceilings across platforms or legitimate source changes. Lockfile or fixture/toolchain changes require an explicit reviewed contract migration; never silently regenerate thresholds. A version mismatch or missing measurement is an actionable failure, not a passing skip.

Only Darwin arm64 has been measured. Linux equivalence is still unvalidated. Before relying on CI enforcement, replay the unchanged baseline twice on the Linux runner with the pinned Node/zlib/esbuild versions and compare both passes for each fixture, package resolution, emitted totals, and retained modules. If Linux differs, independently review an explicit platform baseline with the measured delta and retained-module explanation; do not add speculative slack or call it a reduction. If source-independent variation exists even with pins, derive any future allowance from repeated measurements in that environment. This is a remaining validation requirement, not a claim of cross-platform reproducibility.

### Regression guard implementation (#3471)

The root Vitest suite exercises a pure validator against temporary saved measurement directories. It covers exact ceilings; raw-only and gzip-only overflow; missing, duplicate, and unknown fixtures; missing and unexpected artifact files; stale hashes and metadata; wrong toolchain versions; a mismatched requested source SHA; an incompressible payload appended to an actual temporary JavaScript artifact with refreshed metadata; and a lazy chunk counted in the all-shipped ceiling.

`measure-real.mjs` records SHA-256 values for all seven fixture files and both measurement runners, plus fresh CLI, esbuild, zfb tarball, runtime tarball, and lockfile digests and explicit two-pass inventory verification. It preserves the complete `dist` tree for both passes. The guard reads those saved bytes, checks each file against the recorded dist inventory, independently recomputes every emitted JS chunk's hash/raw/gzip values, verifies the pass inventories match, and sums entry, shared, and lazy chunks. The no-island fixture fails if any JavaScript is emitted. A smoke-only `--case` report cannot pass the full gate.

The existing `health` job now appends Node 24.14.0 after its native-binary upload, asserts the pinned zlib version, rebuilds and packs both packages, then runs complete workspace and packed real matrices and the guard. Earlier health tests stay on Node 22; no new workflow, check name, ruleset, browser lane, or release gate is added. Linux equivalence is still unvalidated, so the first Linux run must establish baseline parity before the team relies on the ceiling as a merge gate.

### Downstream execution and review target

#3470 verifies the fallback by comparing the baseline source/graphs, running the scoped checks named in its updated issue, and recording a before/after matrix from unchanged product code. Use the README's real workspace/packed and modeled replay commands through the heavy guard for CLI runs; manager owns heavy and browser checks. Roll back any unauthorized product edit, fixture simplification, dropped behavior, changed name/minification flag, uncounted chunk, or gate relaxation. If a candidate becomes supported by new evidence, revise this decision with independent review before implementing it.

For #3471, the L3 check is in **the existing `.github/workflows/health.yml` `health` job**, appended after its native-binary upload. That job already builds the real CLI and provisions esbuild; the browser-only workflow does not. The final step boundary uses pinned Node 24.14.0 and its recorded zlib, rebuilds/packs the TS packages, and runs the complete real workspace and packed matrix plus the guard. The first Linux execution must compare against the unchanged baseline before the team relies on CI enforcement. No new required check, branch ruleset, browser lane, md-wasm gate, or release gate was added. A fresh-context review is still required for the gate implementation, threshold assertions, runner hash update, and CI placement.

The focused topic PR targets `base/v3-decisions`; the epic PR targets `main`. Review this decision for truthful no-reduction wording, required behavior, per-fixture ceilings, and the Linux verification condition. Review #3471 additionally for complete fixture/chunk inventory, failures on missing evidence, a deliberately oversized emitted payload, toolchain pinning, and unchanged repository governance. Update EN/JA `docs/src/content/docs{,-ja}/concepts/islands.mdx` with actual coverage and limits; do not claim only toggle code ships.

The manager applies the prepared #3470/#3471 bodies and reports this decision in epic #3460 before resolving #3469. No GitHub bodies were mutated by the worker.
