# Toolchain modernization: stage 0 baseline ledger (#3554)

Ledger for epic #3544. Every later stage (#3555 to #3560) compares against this file and
`baseline.json` beside it (per-file Vitest inventory, packed-tarball contents, raw local timings).
No tooling was upgraded in this stage.

## Baseline revision is 89b333c9, CI-verified green

| Item                     | Value                                                                                                                                                                                                                                                                                                                                       |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Baseline SHA             | `89b333c9662229ef18ff6aa3cdff5f6f23558eba` (tip of `base/sweep-261001` = start of `base/sweep-261001-toolchain-modernization`)                                                                                                                                                                                                               |
| Planning SHA (#3544)     | `baac44eac12d300d68fd8742c585567ea24e6aa9` (`main`, v3.1.0). Since then the sweep landed product fixes (#3541, #3548, #3619/#3628); none touched the contributor-tool surfaces this epic changes                                                                                                                                         |
| CI evidence              | PR #3628 final runs on head `3e8e4d719b57d9e2e4c6a8968f3c0fcc43704aa4`, 2026-10-04 08:09Z: 34 jobs, 30 success, 4 skipped by design, 0 failed                                                                                                                                                                                            |
| Tree equivalence         | The `pull_request` merge ref combined `3e8e4d71` with `base/sweep-261001@9bbe077d`. That base was unchanged from 05:44Z until the merge, and `89b333c9` has exactly those two parents. So the tree CI tested is the baseline tree (inferred from parentage; GitHub no longer serves the merge ref)                                       |
| Super-PR runs on 89b333c9 | At capture time (about 09:00Z) every workflow had passed except `health`, which was still running. Later stages may cite it once it finishes                                                                                                                                                                                                |

CI run IDs (all `https://github.com/Takazudo/zudo-front-builder/actions/runs/<id>`):

| Workflow                              | Run           | Jobs (result)                                                                                                                                                                                         |
| ------------------------------------- | ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| health                                | `37188000560` | changed-files, health, build (no-v8), wasm-md ×6 legs, wasm-md browser: all success                                                                                                                   |
| Binary smoke, scaffold E2E & showcase | `37188000646` | 2 binary builds, 4 local-mode smokes, AL2023 glibc-floor smoke, Scaffold E2E (packed tarballs), Built-site browser smoke, Showcase preview: success. Smoke released, showcase deploy, 2 notifiers: skipped (push/schedule-only) |
| Docs Checks                           | `37188000662` | changed-files (docs), Build docs site, Docs gate: success                                                                                                                                             |
| PR Preview (Cloudflare Workers)       | `37188000673` | Build docs site, Upload preview: success                                                                                                                                                              |
| PR Checks                             | `37188000555` | pnpm audit (prod): success                                                                                                                                                                            |
| router-chromium                       | `37188000602` | success                                                                                                                                                                                               |
| zudo-react-browser                    | `37188000550` | success                                                                                                                                                                                               |
| wind-computed-style                   | `37188000627` | success                                                                                                                                                                                               |
| actionlint                            | `37188000664` | success                                                                                                                                                                                               |

Earlier runs on this branch (`ff44e9a7`, `409f7dc9`) were red, and the head commits fixed them. They
are not pre-existing defects of the baseline.

## Existing defects: none open in the baseline gates

- **#3473** (strict-pnpm consumer cannot resolve the runtime's transitive Hono) is closed. Its fix,
  PR #3548 (head `f23cd91f2d22b1d4184d6aabe9acd3a9bde3d337`), merged into `base/sweep-261001` on
  2026-10-01 as `968dda1e`, an ancestor of the baseline. The fix is present; it has not reached
  `main`.
- **#3480** (dangling `bundle-runtime.mjs.map` reference in the emitted Worker) was closed on
  2026-10-04 by #3622 under epic #3619 / PR #3628. Fix commits `13fddbb8` and `e18b5503` are
  ancestors of the baseline.
- Every local light check below exited 0. No failing check was observed at the baseline.

## Versions: installed baseline and selected candidates

### Installed (baseline)

| Tool                        | Declared                                     | Resolved / observed                                                                                                         |
| --------------------------- | -------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| pnpm                        | `packageManager: pnpm@11.3.0`, engines `>=11.0.0` | 11.3.0 (local and CI). `reusable-smoke-clean-room.yml` also hard-codes `version: 11.3.0`                                   |
| Node (contributor)          | root engines `>=22.0.0`                      | CI general jobs: `node-version: 22` resolving to 22.23.3. CI size job: `24.14.0`. Local: v24.14.0                         |
| TypeScript                  | `^5.9.0` (zfb, runtime, adapter, md-wasm, docs); template `^5.6.0` | 5.9.3 (single resolved copy)                                                                     |
| Vitest                      | `^2.1.9` (root and all 6 packages)           | 2.1.9; transitive vite 5.4.21; happy-dom 15.11.7                                                                            |
| Prettier                    | `^3.8.3`                                     | 3.8.3                                                                                                                       |
| mdx-formatter               | `pnpm dlx @takazudo/mdx-formatter@1.2.1`     | 1.2.1 (not in the lockfile; bin `mdx-formatter`)                                                                            |
| lefthook, Playwright        | `^2.1.6`, `^1.49.0`                          | 2.1.6, 1.61.0                                                                                                               |
| Rust                        | `rust-toolchain.toml` channel `stable`       | Local cargo/rustc 1.94.0 (Homebrew)                                                                                          |
| Lockfiles (SHA-256)         | `pnpm-lock.yaml` (lockfileVersion 9.0)       | `919bc08a976ef1bc03bf6bb465dff020bff8b0c91c86d8eca04ebde4c192a0be`                                                          |
|                             | `Cargo.lock`                                 | `62cb06933c5048c43f20ed696f53dfc5ba57b444876396e3822b860b2bc0b21d`                                                          |
|                             | `crates/zfb-md-wasm/npm/test/fixtures/vite-6.4.3/pnpm-lock.yaml` | `7d2fbc43d627f2077602f5667e7260565c72186915cf0242afbbfc2d931b44c8`                                      |

Local host: macOS 26.6.1 (Darwin 25.6.0, arm64), Apple A18 Pro, 6 cores, 8 GiB, shared with
other sessions. Load average was 4.9 to 7.3 for every measurement.

### Candidates, checked against the npm registry on 2026-10-04

| Component      | Selection                                     | Evidence and reason                                                                                                                                                                                                                                                                                                                                  |
| -------------- | --------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| TypeScript 6   | **`typescript@6.0.3`** (selected)             | Only 6.0.2 and 6.0.3 exist. 6.0.3 (2026-04-16) is the final 6.x patch. Engines `>=14.17`; `bin.tsc` and no `exports` map, so `typescript/bin/tsc` still resolves                                                                                                                                                                                     |
| Vitest 4.1+    | **`vitest@4.1.11`** (selected)                | `V4` dist-tag, final 4.1.x (2026-08-18). Engines `^20 \|\| ^22 \|\| >=24`. Peers: `vite ^6 \|\| ^7 \|\| ^8`, `happy-dom *`. The resolved vite 5.4.21 must move to 6 or later. `defineWorkspace` is gone in 4.x, so `vitest.workspace.mjs` becomes `test.projects`                                                                               |
| pnpm 12        | candidate `12.9.1` (stage #3555 pins)         | `latest`/`latest-12` is now **12.9.1** (2026-10-03T20:48Z, under one day old at capture). The planning candidate 12.8.1 was superseded by 12.8.2 (09-30) and 12.9.0 (10-02). Stage 1 should pin the newest 12.x that passes its install matrix and record its age; the repo sets `minimumReleaseAge: 0`                                       |
| Vite+          | candidate `vite-plus@1.0.0` (still `latest`)  | Published 2026-09-28. Engines `^22.18.0 \|\| ^24.11.0 \|\| >=26.0.0`; CI's 22.23.3 and local 24.14.0 both satisfy it. Bundles exact `vitest 5.0.1`, `oxfmt =0.70.0` (standalone oxfmt `latest` is 0.71.0), `oxlint =1.85.0`, and `vite` aliased to `@voidzero-dev/vite-plus-core@1.0.0`. Bins `vp`, `vpr`. Vitest's own `latest` is 5.0.3 |
| TypeScript 7   | candidate `typescript@7.0.2` (still `latest`) | Native per-platform optional dependencies. The `exports` map exposes `.`, `./package.json` and `./unstable/*` only, not `./bin/tsc` (Finding A below). `bin.tsc` still points at `bin/tsc`                                                                                                                                                          |
| mdx-formatter  | keep `1.2.1`                                  | `latest` is now 1.3.0 (2026-09-25). Per #3544, move 1.2.1 to an exact devDependency without upgrading it                                                                                                                                                                                                                                         |

Published-consumer TypeScript range: no package README, docs page or `peerDependencies` declares
one. The only declaration is the `create-zfb` template's `typescript: ^5.6.0`, which today resolves
to 5.9.x. Later stages must keep TS 5.9 consumers working as a non-regression probe and must not
silently require TS7. The adapter `check-packed-types`, the zudo-react packed check and the md-wasm
`packed-consumer` fixture all currently compile with the workspace's 5.9.3.

## Validation lanes: suite identities and counts at the baseline

### `pnpm test:workspace` covers 7 Vitest projects, 101 files and 1,614 cases with no skips (CI Linux)

The command is `pnpm -r --include-workspace-root --filter '!@takazudo/zfb-md-wasm' test`. Its
scope is 11 of the 12 workspace projects; 6 of them define a `test` script.

| Project                           | Config                                                  | Default env | Timeout                     | Files | Cases | Compound steps around Vitest                                                                                     |
| --------------------------------- | ------------------------------------------------------- | ----------- | --------------------------- | ----: | ----: | ---------------------------------------------------------------------------------------------------------------- |
| root `scripts`                    | `vitest.workspace.mjs` extends `vitest.config.mjs`       | node        | 5 s (Vitest default)        |    19 |   407 | none                                                                                                             |
| root `scripts-subprocess`         | same, carved out by `exclude`                           | node        | **90 s** (`testTimeout: 90_000`) |  4 |    98 | none                                                                                                             |
| `packages/zfb`                    | `vitest.config.ts`                                      | happy-dom   | 5 s                         |    49 |   720 | `vitest run && tsc -p` zudo-react-fixture, then zudo-react-dev-fixture, then zudo-react-sdk-fixture              |
| `packages/zfb-runtime`            | `vitest.config.ts`                                      | node        | 5 s                         |    17 |   300 | none                                                                                                             |
| `packages/zfb-adapter-cloudflare` | `vitest.config.ts`                                      | node        | **19 s**                    |     5 |    40 | `typecheck:consumer` (tsc), then `vitest run`, then `test:packed-types` (`pnpm build` + `check-packed-types.mjs`) |
| `packages/create-zfb`             | `vitest.config.mjs`                                     | node        | 5 s                         |     2 |    24 | none                                                                                                             |
| `docs`                            | `vitest.config.ts` (alias `zfb/config`)                 | node        | 5 s                         |     5 |    25 | none                                                                                                             |

The md-wasm project is excluded from this lane and runs in the `wasm-md (default)` CI leg: config
`crates/zfb-md-wasm/npm/vitest.config.ts`, node env, **15 s** timeout, **14 files, 234 cases**.
Its compound chain is `pretest` (`assert-consumer-artifacts.mjs`), then `typecheck:consumer` (tsc),
then `vitest run`, after a separate `node scripts/build.mjs` build that runs
`node_modules/.bin/tsc`.

Facts from #3554 checked against the executable config: root ordinary 5 s, the four named
subprocess suites at 90 s, SDK happy-dom, runtime/create-zfb/docs/adapter/md-wasm node, adapter
19 s and md-wasm 15 s all hold. The subprocess project holds exactly `docs-dev-supervisor`,
`harvest-supervisor-timelines`, `supervisor-watch-handoff` and `supervisor-watch`. Each suite ran
once, so neither project dropped or duplicated a suite. The two projects together hold all 23
`scripts/__tests__/*.test.mjs` files.

Per-file and per-suite overrides that must survive the Vitest 4 / Vite+ migration:

- `packages/zfb`: 15 files declare `// @vitest-environment node`: content-render-markers-zudo-react,
  island-owned-factory, and 13 under `zudo-react/` (acceptance-r-a01, acceptance-r-a03-logic,
  escape, forms-server, jsx-contract-matrix, pre-leading-lf, props-transport, raw-text, reactive,
  scheduler, scope, server, structure-server). Two files restate `happy-dom` explicitly:
  runtime-lifecycle and runtime-persist-plan.
- `packages/zfb-runtime`: 10 files override the node default with `@vitest-environment happy-dom`:
  client-router-split, plus 9 under `client-router/` (events, history-safe,
  persist-island-lifecycle, prefetch, router-init-gating, router-srcdoc-guard, router-vt-history,
  router, swap-functions).
- Suite-level timeouts:
  - `docs-dev-supervisor` has a describe-level `{ timeout: 90_000 }`, inside a
    `describe.skipIf(!supervisorRunnable)`. That skip fires only without `docs/node_modules` or on
    win32, and it throws under `CI` when `run-parallel` is missing.
  - `harvest-supervisor-timelines` has a describe-level 20 s timeout.
  - `packages/zfb` `launcher.test.ts` has a 15 s per-test timeout.
  - md-wasm `workerd.test.ts` has two 30 s per-test timeouts.
- `ZFB_SUPERVISOR_TIMELINE=1` is set on the health `test:workspace` step. Each CI run emits
  `[supervisor-timeline]` lines.

### Other lanes, all from the same CI runs

| Lane                                         | Command / job                                                       | Baseline result                                                                                                                                                                                                                                                                             |
| -------------------------------------------- | ------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| typecheck                                    | `pnpm typecheck:workspace` (health)                                 | 3 projects: adapter (`tsc --noEmit` + `typecheck:consumer`), zfb (`tsc --noEmit`), runtime (`tsc --noEmit`). `!./examples/*` matched nothing ("No projects matched")                                                                                                                       |
| format                                       | `pnpm format:check` (health)                                        | pass (`prettier --check` on `**/*.{js,mjs,cjs,ts,tsx,json,yml,yaml}`, then `mdx-formatter@1.2.1 --check '**/*.{md,mdx}'` through `dlx`)                                                                                                                                                   |
| env-gate (esbuild-only, ignored tests)       | `cargo nextest run --workspace --profile ci --run-ignored ignored-only -E …` | 51 run / 51 passed / 1,598 skipped (staged esbuild 0.25.12)                                                                                                                                                                                                                        |
| workspace Rust                               | `cargo nextest run --workspace --profile ci`                        | 6,474 run / 6,474 passed / 83 skipped, 508.7 s, no FLAKY retries                                                                                                                                                                                                                            |
| doctests                                     | `cargo test --workspace --doc`                                      | 21 binaries, 3 passed, 7 ignored                                                                                                                                                                                                                                                            |
| md-extras                                    | `cargo nextest run -p zfb-md-extras --features test-utils --profile ci` | 457 passed                                                                                                                                                                                                                                                                              |
| build (no-v8)                                | release build, `--tests` check, `zfb-render` syntect-onig, `zfb --lib`  | pass; 1,242 test cases                                                                                                                                                                                                                                                                  |
| wasm-md matrix (6 legs)                      | check + clippy + `cargo test -p zfb-md-wasm` per feature leg, plus graph assertion | all pass. Rust cases: default/pipeline 75, parse 41, render 23, highlight 14, none 8. The default leg also runs the JS lane (14 files / 234 cases), the export assertion and the budgets                                                                                   |
| wasm-md browser                              | Vite 6.4.3 packed fixture (dev + preview), native `zfb build`, Playwright | `md-wasm-browser.chromium.spec.mjs` 1 passed                                                                                                                                                                                                                                          |
| island size (Node 24.14.0)                   | health size step: build + `pnpm pack` zfb/runtime, `check-budget.mjs` | pass                                                                                                                                                                                                                                                                                      |
| router-chromium (L4)                         | `pnpm test:router-chromium`                                         | 24 passed, 8 specs: router 9, traverse 5, real-islands 4, livereload-backnav 2, back-race, form-submit, island-persist and sdk-persist 1 each                                                                                                                                             |
| zudo-react-browser (L4)                      | `pnpm test:zudo-react-browser`                                      | 17 passed, 16 specs (smoke 2, the rest 1 each)                                                                                                                                                                                                                                              |
| wind-computed-style (L5)                     | `pnpm test:wind-computed-style`                                     | 18 passed, 9 specs: w-a01 1, w-a02 1, w-a03 4, w-a05 3, w-a07 1, w-a08 2, w-a09 2, w-placement 3, wind-raw-route 1                                                                                                                                                                         |
| built-site smoke (L4)                        | node-free-smoke                                                     | 8 passed: hydration 2, lifecycle 2, runtime-identity 2, identity 1, navigation 1                                                                                                                                                                                                            |
| scaffold E2E / local smokes                  | node-free-smoke                                                     | pass. The packed-tarball scaffold built 6 + 1 pages; amd64/arm64, TS-config and AL2023 glibc-2.34 smokes passed                                                                                                                                                                             |
| docs                                         | docs-checks: `pnpm --filter docs build`, `check`, `check:html`, `check:islands`, `check:wrangler-pin` | 609 pages built in 44.45 s; `zfb check` "2 collections and tsc, no errors"                                                                                                                                                                               |
| security                                     | `pnpm audit --prod` (pr-checks)                                     | pass                                                                                                                                                                                                                                                                                        |

## Package output inventory at the baseline

Packed with `pnpm -C <pkg> pack` (pnpm 11.3.0, tsc 5.9.3, Node 24.14.0, Darwin arm64) from freshly
built `dist/` with no orphaned outputs. md-wasm comes from the CI artifact
`zfb-md-wasm-packed-tarball` of run `37188000560`. Full per-file paths and sizes are in
`baseline.json` → `packedTarballs`.

| Tarball                                     | Files | Unpacked bytes | tgz bytes | SHA-256 (tgz)                                                      | Published 3.1.0 (files / bytes) |
| ------------------------------------------- | ----: | -------------: | --------: | ------------------------------------------------------------------ | ------------------------------- |
| `takazudo-zfb-3.1.0.tgz`                    |   120 |        978,455 |   267,310 | `d4ad27eebe8c174786934e2de5bf9c4c633e94a62f5fda1653c534e61e2aa580` | 111 / 876,904                   |
| `takazudo-zfb-runtime-3.1.0.tgz`            |    46 |        446,278 |   133,889 | `3892d15234f54aeae1c994c8d5abae9f5f24ffbb24ec0e2692d3fad58236010d` | 46 / 434,764                    |
| `takazudo-zfb-adapter-cloudflare-3.1.0.tgz` |    15 |         73,312 |    18,316 | `49ecbb1877f31cbe45f97f4e340a89bd1a76af8f0c67aac9a4738091a4adbdb6` | 15 / 71,405                     |
| `create-zfb-3.1.0.tgz`                      |     7 |         11,120 |     4,981 | `626ede23c92044d8136bf6e3d77d89489c449f56607018f5465a24e40ce66088` | 7 / 11,120                      |
| `zfb-md-wasm.tgz` (CI, Linux)               |    68 |      8,159,151 | 3,782,106 | `4fb6c3344f09865754cbf4c54307ce3fd96a226293e98c697cf8c0744d02d102` | 68 / 8,167,695                  |

The difference from published 3.1.0 is unreleased sweep work. Compare later stages with this table,
not with the registry. The five platform packages each contain the native binary plus LICENSE,
README.md and package.json; registry 3.1.0 holds 4 files and 99 to 111 MB each. A local pack
without release binaries is meaningless, so they are inventoried from the registry only.

## Local measurements on a loaded host (reproducible commands)

Commands ran in the foreground from the worktree root at `89b333c9`, warm caches. Each set of
commands ran in alternating order, 3 rounds. Raw rows are in `baseline.json` → `localTimings`.

| Command                                                                                                | Runs (s)            | Median (s) |
| ------------------------------------------------------------------------------------------------------ | ------------------- | ---------: |
| `pnpm typecheck:workspace`                                                                             | 7.02 / 4.61 / 4.09  |   **4.61** |
| `pnpm format:check`                                                                                    | 9.28 / 9.80 / 10.04 |   **9.80** |
| `pnpm format:check:ts` (Prettier)                                                                      | 5.78 / 5.80 / 6.04  |   **5.80** |
| `pnpm format:check:mdx` (`dlx` mdx-formatter, cached)                                                  | 3.79 / 3.99 / 4.13  |   **3.99** |
| `pnpm --filter @takazudo/zfb --filter @takazudo/zfb-runtime --filter @takazudo/zfb-adapter-cloudflare build` | 3.76 / 5.48 / 3.43 | **3.76** |
| `pnpm --filter create-zfb test` (Vitest startup proxy, 24 cases)                                       | 4.28 / 1.76 / 1.76  |   **1.76** |

For comparison, CI health on Linux x64 reported these Vitest durations: root 9.14 s, zfb 9.93 s,
runtime 4.10 s, docs 2.04 s, adapter 1.79 s, create-zfb 0.64 s. Later stages must alternate
baseline and candidate on one host and report medians and load. These numbers are not a Rust
speedup claim.

## Static inventory

### Manifests

- **Workspace (12 projects)**: root; `docs`; 9 under `packages/*` (`zfb`, `zfb-runtime`,
  `zfb-adapter-cloudflare`, `create-zfb`, and the 5 platform packages); `crates/zfb-md-wasm/npm`.
  `examples/*` is in `pnpm-workspace.yaml` but holds only a README. Workspace settings:
  `linkWorkspacePackages: false`, `autoInstallPeers`, `engineStrict`, `confirmModulesPurge: false`
  (a pnpm 11 non-TTY workaround), `minimumReleaseAge: 0`, overrides `devalue ^5.8.1` and
  `hono >=4.12.25`, and `allowBuilds` for esbuild, lefthook, sharp and workerd.
- **Intentional registry pins**: `docs` pins published `@takazudo/zfb*` 2.20.2 and `zudo-doc` /
  `history-server` 5.27.0, which is the frozen docs host. `wrangler` is 4.85.0 at the root and in
  docs, and hardcoded in the showcase jobs.
- **Isolated non-workspace consumers**:
  - `tests/md-wasm-browser-smoke` installs a `file:` tgz with `--ignore-workspace --lockfile=false`.
  - `crates/zfb-md-wasm/npm/test/fixtures/vite-6.4.3` has its own lockfile.
  - `crates/zfb-md-wasm/npm/test/fixtures/packed-consumer` is a tsconfig-only fixture.
  - `crates/zfb/templates/basic-blog` is the `create-zfb` template, with `workspace:*` deps and
    `typescript ^5.6.0`.
- **Rust test-fixture manifests**: 41 further `package.json` files under
  `crates/zfb-config-loader/tests/fixtures/**`, `crates/zfb-islands/fixtures/**` and
  `crates/zfb/tests/fixtures/**`. They are resolver/bundler data and must not be touched by a
  formatter or a migrator.
- **Native embedding**: `crates/zfb/build.rs` reads `node_modules/.pnpm/<name>@<ver>*/node_modules/<name>`
  (Hono 4.12.25). This depends on the pnpm store layout.

### tsconfig graph and compiler invocations

- `tsconfig.base.json` sets ES2022, ESNext, Bundler, strict, `noUncheckedIndexedAccess`,
  `isolatedModules` and `skipLibCheck`. It has no `baseUrl` or `types`. It is extended by zfb,
  zfb-runtime, adapter, md-wasm and the zfb SDK fixture.
- Standalone configs are `tsconfig.zudo-react-fixture.json` (the dev fixture extends it),
  `tests/zudo-react-browser/fixtures`, md-wasm `packed-consumer` and the template.
- **`baseUrl` sites** (TS7 deprecation):
  - `docs/tsconfig.json`, and its inherited `@takazudo/zudo-doc@5.27.0/tsconfig.base.json`, which
    also sets `baseUrl: "."`. The pinned upstream config therefore carries `baseUrl` too.
    Finding B needs a narrow fix or a scoped exception.
  - `packages/zfb-adapter-cloudflare/tsconfig.consumer.json`. #3544 does not name this site; it
    is a contributor compiler config and in scope.
  - `crates/zfb/templates/basic-blog/tsconfig.json`, which ships to users. Changing it is a public
    consumer change that needs explicit review.
  - Six Rust fixture tsconfigs under `crates/zfb/tests/fixtures/**`. They test ZFB's own alias
    contract; do not change them.
- Emit: `tsc` for zfb, zfb-runtime and the adapter, with `cp` of `worker-wrapper.mjs` and
  `emit-worker.mjs` for the adapter. md-wasm runs `scripts/build.mjs`, which calls
  `node_modules/.bin/tsc`.
- **TS7-sensitive compiler launches**:
  - Finding A: `packages/zfb-adapter-cloudflare/scripts/check-packed-types.mjs:66` uses
    `require.resolve("typescript/bin/tsc")`, which TS 7.0.2's `exports` map blocks.
  - Additional site: `packages/zfb/scripts/zudo-react-packed.mjs:105` joins
    `node_modules/typescript/bin/tsc` and runs it with `process.execPath`. A plain path join is not
    blocked by `exports`, but whether the TS7 `bin/tsc` shim can be run with node needs verifying.
    `tests/zudo-react-browser/build-fixture.mjs` calls this script.
  - `crates/zfb-md-wasm/npm/test/package-browser.test.ts:130` runs `node_modules/.bin/tsc`.
  - `tests/zudo-react-browser/build-fixture.mjs:345` runs `pnpm --filter @takazudo/zfb exec tsc`.
  - Rust `zfb check` (`crates/zfb/src/commands/check.rs`) resolves `<project>/node_modules/.bin/tsc`
    and then `PATH`. This is product behavior, not contributor tooling.
- No script imports the TypeScript compiler API (`from "typescript"` or `require("typescript")`).

### Formatter callers and hooks

- Prettier is called directly by:
  - `package.json` `format:ts` / `format:check:ts`.
  - `lefthook.yml` pre-commit `pnpm exec prettier --write {staged_files}` with `stage_fixed`.
  - The regeneration instructions in `crates/zudo-wind/README.md:30` and
    `crates/zudo-wind/tests/catalog_export.rs:5`.
  - `.claude/skills/l-make-release/SKILL.md:389`.
- mdx-formatter 1.2.1 is called through `pnpm dlx` by `format:mdx`, `format:check:mdx` and the
  lefthook pre-commit `format-mdx` hook.
- `scripts/run-b4push.sh` calls `pnpm format:check`, `typecheck:workspace` and `test:workspace`.
- Config files are `.prettierrc.json` (semi, double quotes, trailing comma `all`, printWidth 100),
  `.prettierignore` (which protects `expected.d.ts`, the md-roundtrip fixtures, generated wasm glue
  and `pnpm-lock.yaml`), `.mdx-formatter.json` (with its block components and the `HtmlPreview`
  multiline exclusion) and `.editorconfig`.
- Hooks: lefthook installs pre-commit through `prepare`. `scripts/install-git-hooks.sh` installs
  the direct `.git/hooks/pre-push` worktree guard, which lefthook does not manage.

### CI bootstrap sites and path filters

- `pnpm/action-setup@0e279bb9` (v6.0.8) has 24 call sites. 23 read `packageManager`; only
  `reusable-smoke-clean-room.yml` pins `version: 11.3.0`.
  - Per file: node-free-smoke 5, health 4, release 3, docs-deploy 2, exam 2, and 1 each in
    docs-checks, docs-pr-preview, pr-checks, reusable-smoke-clean-room, router-chromium,
    security-audit, wind-computed-style and zudo-react-browser.
- `actions/setup-node` has 30 call sites. Every one uses Node `22` except the health island-size
  job, which uses `24.14.0`. Several Node-only jobs disable the pnpm cache because pnpm is never
  set up there: drift-net, supervisor-watch, yaml-candidate-watch, and the wrangler jobs in
  docs-deploy and docs-pr-preview.
- PR workflows trigger on `pull_request` for `main` and `base/**`; node-free-smoke triggers on every `pull_request` without a branch filter.
- Path filters that tooling stages must keep complete:
  - The health `changed-files` → `wasm_md` filter lists `package.json`, `pnpm-workspace.yaml`,
    `pnpm-lock.yaml`, `.npmrc`, `tsconfig.base.json`, `vitest.config.mjs`, `.prettierrc.json`,
    `.prettierignore`, `.editorconfig`, `.mdx-formatter.json` and `lefthook.yml`. It does **not**
    list `vitest.workspace.mjs`. A future `vite.config.*` and Oxfmt config must be added here.
  - The docs-checks `docs` filter covers `docs/**`, the lockfile, the workspace file, `package.json`
    and `.npmrc`.
  - router-chromium, zudo-react-browser and wind-computed-style filter on `pnpm-lock.yaml`,
    `package.json` and package paths. router-chromium also filters on `tsconfig.base.json`.
- Lanes with no CI wiring: `test:wind-real-build` (local Chromium W-A06 check) and
  `test:webkit-back` (T4, Mac only). `tests/scanner-boundary-acceptance` is driven by the Rust
  test `scanner_boundary_acceptance.rs`.

## Rebaselines and documentation drift for later stages

- **Lockfile provenance**: `research/v3-island-size/decision.json` and `decision-linux-x64.json`
  pin `pnpmLockSha256` to the baseline lock hash and `cargoLockSha256` to the Cargo hash, with
  Node v24.14.0 and esbuild 0.25.12. `check-budget.mjs` (around line 447) fails on any lock hash
  mismatch. Every stage that changes `pnpm-lock.yaml` therefore needs the reviewed island-size
  rebaseline procedure, not a loosened check.
- **md-wasm sizes**: refresh only from CI, never from a Mac (see `crates/zfb-md-wasm/LOCAL-VS-CI-SIZES.md`).
- **Documentation drift, deferred to a later stage**: root `CLAUDE.md:190` says `scripts-subprocess`
  "raises it to 60 s", but the executable config is 90 s. The prose should be fixed, not the
  timeout. Also, the `vitest.workspace.mjs` comment cites "all 19 scripts/\_\_tests\_\_ files";
  there are now 23.
- Local Node is 24.14.0 while CI general jobs use 22.x. Vite+ engines need ≥22.18.0, which CI's
  22.23.3 meets. Stage 1 or 3 must still decide whether to pin an explicit version everywhere.

## Evidence that is unavailable here

| Evidence                                                                                                  | Status                                                                                                                                                                                         |
| --------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Local heavy runs (`test:workspace`, nextest, doctests, env-gate, docs build, browser lanes, md-wasm build) | **Not run locally.** This 8 GiB shared host cannot finish them (owner-approved adaptation). Linux x64 CI results above stand in                                                                  |
| Darwin heavy lanes at `89b333c9`                                                                          | **Unavailable.** The newest macOS evidence is `exam.yml` run `37097088467` on `main@baac44ea` (success, 2026-10-03), the planning base rather than this baseline                               |
| `test:webkit-back` (T4, Mac only), `test:wind-real-build` (local only)                                    | **Not run.** Neither has a CI home                                                                                                                                                             |
| Windows                                                                                                   | **Unavailable.** No Windows test lane exists in the repo                                                                                                                                      |
| Local md-wasm package build / pack                                                                        | **Not run.** Mac codegen differs from CI, so the CI tarball is used                                                                                                                            |
| Platform-binary package packs                                                                             | Inventoried from registry 3.1.0 only                                                                                                                                                          |
| Super-PR `health` run `37189993349` on `89b333c9`                                                         | Still in progress at capture time; tree-equivalent evidence comes from run `37188000560`                                                                                                       |
| drift-net / security-audit (scheduled)                                                                    | Last runs predate the sweep: drift-net `36666284458` on `main@82193109` and security-audit `36393454183` on `main@6ed8738f`, both success                                                     |
