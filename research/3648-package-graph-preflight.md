# Package graph preflight performance (#3648)

Evidence date: 2026-10-05. [Issue #3648](https://github.com/Takazudo/zudo-front-builder/issues/3648) requires a build to complete or produce a finite diagnostic for the reported package graph.

## Provenance

- Exact source package: `zudolab/zudo-doc` commit `ac3c4bd8eb11d257b7246815146f4b8f47f598a6`, rebuilt with pnpm 10.30.3 using `install --ignore-scripts --frozen-lockfile` and `build:workspace`.
- External site: one `pages/index.tsx` with an unused named `createChrome` import from `@takazudo/zudo-doc/chrome`, `{ wind: false }`, and a symlink to the rebuilt package.
- Final observed [Health run 37257224904](https://github.com/Takazudo/zudo-front-builder/actions/runs/37257224904): PR head `8d90856fbea913ebdb2bc54c386e2c6f5dfeeee6`, tested CI merge SHA `7e74f44783dc578c0f805d9c7562e818da466c27` (recorded by `source-sha.txt`).
- Artifact: `package-graph-3648-evidence`, containing scanner/build logs and exit statuses, focused test log/JUnit, supported-graph logs, and emitted HTML. The temporary collection workflow was removed after these observations.

## Observations

| Input / version | Result |
| --- | --- |
| Exact graph, released Darwin 3.2.0 | Still before the bundler-input timing marker at the intentional 25-second local cap. The source issue also records a 60-second cap. Neither cancellation is a build verdict. |
| Exact graph, first factory/AST cache candidate on [Linux CI](https://github.com/Takazudo/zudo-front-builder/actions/runs/37254396167) | Scanner exceeded 90 seconds. Fixed-candidate snapshots showed 89–99% CPU in repeated `NestedFinder` AST walks, with roughly 70 MB RSS. |
| Exact graph, lexical binding index (`3303d765`, [CI run](https://github.com/Takazudo/zudo-front-builder/actions/runs/37255598614)) | Scanner finished in 10,720 ms with the source-located unsupported-target diagnostic below. All 35 then-current registration tests passed. |
| Exact graph, final observed CI merge | Scanner finished in 3,500 ms, exit 1. Full `zfb build` also returned exit 1 with the same finite diagnostic. |
| Supported graph, final observed CI merge | A separate first-party graph with 256 nested helpers and a statically imported named client `Counter` scanned in 17 ms (one island). Full build produced one nonempty HTML page in 0.47 seconds. |
| Registration tests, final observed CI merge | 38/38 passed, including resolver/AST-work budgets, full wrapper-summary priming, lexical shadowing, cycles, default wrapper forms, and the dependency-member diagnostic. |

These are individual debug-build observations, not a cross-platform benchmark. The local Darwin baseline and Linux CI timings are not directly comparable. Full Health was still running when this report was finalized; the focused results do not claim a green whole-workspace suite.

## Fix and retained limitation

Registration now shares immutable parsed modules, classifies factory calls once per module, indexes lexical declaration facts and return candidates, and primes each module once. It preserves lexical IDs, the existing alias-cycle guards and purity checks, and source-located errors. Operation-budget tests cover repeated queries and full nested-wrapper summaries without machine-speed assertions.

The real package does **not** build successfully under the existing static-island contract. Its emitted `doc-body-end-islands/design-token-panel-island.js` assigns `DesignTokenPanelBootstrap` from `deps.DesignTokenPanelBootstrap` and uses that runtime member value as an Island child. Both the final scanner and full CLI reject it at line 21, column 7:

```text
target DesignTokenPanelBootstrap has unsupported initializer
```

The diagnostic includes the named-function / `"use client"` rewrite guidance. This is the permitted finite-diagnostic outcome for #3648. Supporting arbitrary dependency-member initializers is outside this fix; upstream must use supported static target wiring or pursue that capability separately. An unused import still makes the module reachable to static boundary analysis.

CI accepted only success, or exit 1 with this exact source/target diagnostic and rewrite hint. Timeouts, unrelated errors/exit codes, and successful builds without HTML were rejected. The supported broad graph independently had to scan **and build successfully**.

## Reproduction entry points

The retained [package graph probe](../crates/zfb-islands/examples/package_graph_probe.rs) documents exact-source setup and invokes the same filesystem scanner preflight. Build the probe separately from a bounded scan so compilation cannot consume the diagnostic deadline. Full `zfb build` is a separate integration observation. On guarded development machines, use the shared heavy guard for builds and probes.

The final focused CI command was:

```sh
cargo nextest run --workspace --profile ci -E 'package(zfb-islands) and test(scanner::registration::tests)'
```

The temporary exact-source and supported-fixture collection recipe is preserved in [the tested workflow revision](https://github.com/Takazudo/zudo-front-builder/blob/8d90856fbea913ebdb2bc54c386e2c6f5dfeeee6/.github/workflows/health.yml). It is no longer wired into Health; the original workflow and ordinary workspace test gate are restored.
