# Toolchain modernization: stage 6 integrated handoff (#3560)

Final ledger for epic #3544. It compares the integrated tree with the stage 0 ledger
([`BASELINE.md`](./BASELINE.md), `baseline.json`) and cites the stage evidence
([`STAGE-1-PNPM.md`](./STAGE-1-PNPM.md) to [`STAGE-5-TS7.md`](./STAGE-5-TS7.md)) instead of
repeating it. Every CI number below comes from the integrated head, not from a stage branch.

## Integrated head a9023b76 is green on every lane that ran

| Item | Value |
| --- | --- |
| Baseline | `89b333c9662229ef18ff6aa3cdff5f6f23558eba` |
| Integrated head (CI-tested) | `a9023b7637075eeac3ad0d28a64a2d98201250b5` on `base/sweep-261001-toolchain-modernization`, epic PR [#3629](https://github.com/Takazudo/zudo-front-builder/pull/3629) |
| This stage | adds this file only; no source, config, lockfile or gate change |
| Local host | macOS 26.6.1 arm64, 8 GiB, Node v24.14.0, shared and loaded (1-minute load 6 to 28 during the checks) |
| CI host | `ubuntu-latest` x64, Node 22.23.3 (general jobs) and 24.14.0 (island-size step) |

Stage merges on the epic base: `de067148` (0), `8ca7ebcd` (1), `e2469233` + `b7c3c921` (2),
`bb5ee698` + `1bf44d5e` (3), `00b8c4fd` (4), `e5522f61` (5), `a9023b76` (Linux island-size
rebaseline).

## Versions before and after

| Component | Baseline `89b333c9` | Integrated `a9023b76` |
| --- | --- | --- |
| pnpm | 11.3.0 | **12.8.2**, exact `packageManager`; `pnpm/action-setup` v6.0.8 → v6.1.0 (`ea17c68d`) at all 24 sites |
| Contributor Node | root `engines` `>=22.0.0` | `^22.18.0 \|\| ^24.11.0 \|\| >=26.0.0` (the Vite+ range); CI unchanged at 22.23.3 and 24.14.0 |
| TypeScript | 5.9.3 (`^5.9.0`) | **7.0.2** exact in the 5 compiler importers; `typescript-6.0` (6.0.3) and `typescript-5.9` (5.9.3) aliases probe consumers in zfb, the adapter and md-wasm |
| Test runner | Vitest 2.1.9 on Vite 5.4.21 | **`vite-plus` 1.0.0** (Vitest 5.0.1, `vite` → `@voidzero-dev/vite-plus-core` 1.0.0 through the workspace catalog) |
| Formatter (code) | Prettier 3.8.3 | **Oxfmt 0.70.0** via `vp fmt`, configured in the `fmt` block of `vite.config.mjs` |
| Formatter (MD/MDX) | `pnpm dlx @takazudo/mdx-formatter@1.2.1` | `@takazudo/mdx-formatter` 1.2.1, exact root devDependency, still the only MD/MDX formatter |
| Rust, esbuild, Hono | stable, 0.25.12, 4.12.25 | unchanged; `Cargo.lock` byte-identical (`62cb0693…0b21d`) |

### Lockfile hash chain

| Point | `pnpm-lock.yaml` SHA-256 |
| --- | --- |
| Baseline | `919bc08a976ef1bc03bf6bb465dff020bff8b0c91c86d8eca04ebde4c192a0be` |
| Stage 1 (pnpm 12) | `c8b344b9cb340f5185412a53300cfc3444f3a8569794db24b16c1472ac8f852b` |
| Stage 2 (TS 6 + 5.9 alias) | `024082220a02df2a4c4deb5bd5455af776b78c0c0dd7cf4a5064d12c5388f6e2` |
| Stage 3A (Vitest 4.1.11) | `1d26a679e95af209cc2c53dc86e72e0900cbf2354c4b69358f0f03f0d87cdcb5` |
| Stage 3B (Vite+) | `69255f60af5d64b7e1300d7276bf9beb2736b29b10ced6616dee90237a0e0b28` |
| Stage 4 (Oxfmt, local mdx-formatter) | `781439c545bccb5fc18bbd3fae1b80276b5efbe6a696574a8bb208f750b145a5` |
| Stage 5 / integrated (TS 7) | `af4dc9f5a8bd02096ffaea0998067f3898469a78930af35e69a99e5085c956b5` |

The md-wasm `vite-6.4.3` fixture lockfile is unchanged (`7d2fbc43…1c44c8`). Both island-size
contracts record the integrated `pnpmLockSha256` and the unchanged Cargo hash.

## Installation passes fresh, repeated and non-interactive

All local, stdin closed, on the integrated head:

| Check | Result |
| --- | --- |
| Fresh `pnpm install --frozen-lockfile` in a clean `git archive` copy | exit 0 in 9.1 s from a warm store; lockfile unchanged; only `allowBuilds` packages ran scripts (sharp and lefthook printed); lefthook's postinstall reports "not a git repository" outside git and continues |
| Repeat frozen and plain installs in the worktree | "Lockfile is up to date" / "Already up to date"; `pnpm-lock.yaml` byte-identical |
| Embedded Hono tree (`build.rs` filter reimplemented) | 563 files in both the fresh copy and the worktree, same tree digest, real directory under `node_modules/.pnpm/hono@4.12.25` (stage 1 proved 563 files and byte identity against pnpm 11) |
| Resolved tools | `pnpm --version` 12.8.2 (auto-switch); `tsc -v` 7.0.2 in all 5 compiler importers; `vp` 1.0.0; `mdx-formatter` 1.2.1 |

## Every CI lane matches its baseline result

Runs on `a9023b76` (`https://github.com/Takazudo/zudo-front-builder/actions/runs/<id>`), compared
with the baseline runs listed in `BASELINE.md`:

| Lane | Run | Integrated result | Baseline |
| --- | --- | --- | --- |
| workspace Rust (`nextest --profile ci`) | `37201113148` | 6,474 run / 6,474 passed / 83 skipped, 0 FLAKY, 321 s | 6,474 / 6,474 / 83, 509 s |
| env gates (`--run-ignored ignored-only`) | `37201113148` | 51 passed / 1,598 skipped | 51 / 1,598 |
| doctests | `37201113148` | 21 binaries, 3 passed, 7 ignored | same |
| md-extras (`test-utils`) | `37201113148` | 457 passed | 457 |
| `pnpm format:check` | `37201113148` | Oxfmt "856 files"; mdx-formatter clean | Prettier + `dlx` pass |
| `pnpm typecheck:workspace` | `37201113148` | pass | pass |
| `pnpm test:workspace` | `37201113148` | 7 projects, 103 files / 1,645 cases, all passed (detail below) | 101 / 1,614 |
| adapter packed types | `37201113148` | strict tsc 7.0.2, 6.0.3 and 5.9.3 passed | strict tsc 5.9.3 |
| island size, Linux x64 | `37201113148` | "passed for all eight fixtures in workspace and packed modes" | pass |
| build (no-v8) | `37201113148` | 1,242 cases passed | 1,242 |
| wasm-md, 6 legs | `37201113148` | Rust cases default 75, pipeline 75, parse 41, render 23, highlight 14, none 8; JS lane 14 files / 234 cases; tarball-only consumer passes | same counts |
| wasm-md browser | `37201113148` | 1 passed | 1 passed |
| binary builds | `37201113117` | x86_64 highest symbol `GLIBC_2.34`, aarch64 `GLIBC_2.30` (floor 2.34 enforced) | pass |
| local smokes, AL2023 | `37201113117` | amd64/arm64, both TS-config, and the Amazon Linux 2023 (glibc 2.34) smoke all pass | pass |
| Scaffold E2E (packed) | `37201113117` | packed clean room passes; strict-pnpm consumer (#3473/#3484) passes with no root Hono link | pass |
| Built-site smoke (L4) | `37201113117` | 8 passed | 8 |
| Showcase preview | `37201113117` | success (deploy and notifiers skipped: push-only) | same |
| router-chromium | `37201113121` | 24 passed | 24 |
| zudo-react-browser | `37201113110` | 17 passed (fixture compiled by tsc 7.0.2) | 17 |
| wind-computed-style | `37201113169` | 18 passed | 18 |
| Docs Checks | `37201113123` | 609 pages, `zfb check` "2 collections and tsc — no errors" (docs tsc 7.0.2), `check:html` pass, island guard 8 route/marker pairs, wrangler pin 4.85.0 | 609 pages, same checks |
| PR Preview | `37201113176` | success | success |
| `pnpm audit (prod)` | `37201113118` | pass; 1 low, 10 moderate | pass; 1 low, 10 moderate |
| actionlint | `37201113109` | success | success |

CI wall times differ between hosted runners and are not a speed claim for any lane.

Earlier health runs on this branch were red and are superseded by the head run: `37193703443`
(`e2469233`, `check_passes_with_sdk_wasm_import_types` under TS6, fixed by `b7c3c921`) and
`37194540459`, `37196200902`, `37199159073` (Linux island-size gzip, fixed by `a9023b76`).

### Workspace suite identities: exact per-file parity

A per-file JSON inventory of every `test:workspace` project on the integrated head, diffed with
`baseline.json` → `vitestFiles`:

| Change | Files | Cases |
| --- | ---: | ---: |
| Baseline | 101 | 1,614 |
| `scripts/__tests__/package-tsc.test.mjs` added (stage 2) | +1 | +7 |
| `scripts/__tests__/formatter-ownership.test.mjs` added (stage 4) | +1 | +23 |
| `check-wrangler-pin.test.mjs` two-document lockfile case (stage 1) | 0 | +1 |
| **Integrated** | **103** | **1,645** |

No other file gained or lost a case, nothing was skipped, and no file ran twice. CI per project:
root 25 / 536, zfb 49 / 720, runtime 17 / 300, adapter 5 / 40, create-zfb 2 / 24, docs 5 / 25.
The config still carries the stage 3 contract: root `scripts` 5 s, `scripts-subprocess` exactly the
four `SUBPROCESS_SUITES` at 90 s, adapter 19 s, md-wasm 15 s, the per-file environment docblocks,
and the compound zfb fixture compiles and adapter consumer/packed checks.

The three `[supervisor-timeline]` records carry the same identity as the baseline run
`37188000560`: `runner=pnpm`, `zudoDoc=5.27.0`, `runParallel=sha256:646f90cc300185cb`,
`fixtureShape=sha256:2d146d48587c00f5`, `env=v1:sha256:3bac6cb07bb8c4b1`. The strict `main`
supervisor watch therefore sees no identity drift from this epic.

### #3619 regressions rerun on the integrated tree

| Regression | Evidence |
| --- | --- |
| #3620 extractor offsets (`crates/zudo-wind/tests/extract_offsets.rs`) | all 8 tests PASS in nextest run `37201113148`, including `quoted_island_marker_with_multibyte_text_before_embedded_class` and `multibyte_characters_at_every_position_never_panic` |
| #3621 rawHtml guard (`raw-html-reserved.test.ts`) | passes inside the zfb project in CI; locally 54 / 54 |
| #3625 combined regression | the Rust case above, and "rawHtml combined #3569/#3570 payload › renders the quoted text exactly with no island marker" (local and CI) |
| #3621 browser half | `raw-html-markers.chromium.spec.mjs` passes in zudo-react-browser `37201113110` |
| #3623 glibc 2.34 | AL2023 smoke and the `GLIBC_2.34` floor check in `37201113117` |
| island-size contracts | Linux x64 passes (above). Darwin arm64 is not enforced in CI and is not remeasured (see Follow-ups) |

## Package outputs keep their inventories

Packed locally from a clean `dist/` (`pnpm -C <pkg> pack`, pnpm 12.8.2, tsc 7.0.2) and compared with
`baseline.json` → `packedTarballs`:

| Tarball | Files | Unpacked bytes (baseline) | Differences | tgz SHA-256 |
| --- | ---: | --- | --- | --- |
| `takazudo-zfb-3.1.0.tgz` | 120 / 120 | 978,492 (978,455) | 9 `.js.map`, `package.json` | `77eec5c3f9114d992665c0599f019e7c9c67763f6bacc3afec73522f0b160848` |
| `takazudo-zfb-runtime-3.1.0.tgz` | 46 / 46 | 446,281 (446,278) | 4 `.js.map`, `swap-functions.d.ts`, `package.json` | `785999f06399bd3a2552ea82c431611d17241e573c49c4290b208dfc36ed5a6d` |
| `takazudo-zfb-adapter-cloudflare-3.1.0.tgz` | 15 / 15 | 73,407 (73,312) | `package.json` | `ac1afd014777c7432ea46fd51cde091e65b6cafe7862692e77178f50f1641dbd` |
| `create-zfb-3.1.0.tgz` | 7 / 7 | 11,124 (11,120) | `package.json` | `5f7096519cea61c778f13d216b6bb7aec4b3423f8fc7601039947e74f699af9c` |
| `zfb-md-wasm.tgz` | not packed locally | | CI's tarball-only consumer and browser job pass | |

Every shipped `.js`, `.mjs` and `.d.ts` keeps its baseline size, apart from the one declaration
stage 5 proved type-equal (`saveFocus: typeof saveFocus`). Map changes are column segments only
(stage 5). The unpacked totals equal stage 5's exactly. Against the baseline manifests, only
`scripts` (`vitest` → `vp test`) and `devDependencies` (`typescript` 7.0.2 and its aliases,
`vite-plus`) changed; `exports`, `files`, `publishConfig`, `dependencies`, `peerDependencies` and
`engines` are unchanged, and the packed manifests carry `"vite-plus": "1.0.0"`, not `catalog:`.
Published consumers are not told to install TS7; TS 5.9 remains the probed floor.

The Linux island-size raw totals are unchanged. `a9023b76` raised seven gzip ceilings by 1 to 4
bytes to the exact totals failed run `37199159073` measured (workspace model +4, blog-theme +1,
multi-island +2; packed show-for +1, blog-theme +2, json-api +1, multi-island +1). The cause is
content-hash strings that move with each lockfile change; the README records it. No other ceiling
or provenance assertion was loosened.

## Same-host timings: faster type checks, builds and format checks

Three rounds alternated a `git archive` of the baseline (installed with pnpm 11.3.0) and the
integrated worktree, swapping the order each round, warm caches, load 6.1 to 8.8:

| Command | Baseline runs (s) | Median | Integrated runs (s) | Median | Ratio |
| --- | --- | ---: | --- | ---: | ---: |
| `pnpm typecheck:workspace` | 8.93 / 4.58 / 4.58 | 4.58 | 1.88 / 1.52 / 1.49 | 1.52 | 3.0x |
| `pnpm format:check` | 22.92 / 10.09 / 9.72 | 10.09 | 4.67 / 4.44 / 4.92 | 4.67 | 2.2x |
| zfb + runtime + adapter `build` | 3.54 / 5.12 / 3.84 | 3.84 | 1.56 / 1.58 / 1.44 | 1.56 | 2.5x |
| `pnpm --filter create-zfb test` (startup proxy) | 5.19 / 1.98 / 2.06 | 2.06 | 2.40 / 2.11 / 1.85 | 2.11 | neutral |

The baseline's first `format:check` includes a cold `dlx` fetch, which the local mdx-formatter
removes. Stage 3 found the root `scripts` project execution-bound and neutral; stage 5 measured
the compiler alone at 3.4x to 5.1x with about half the peak RSS. None of this is a claim about
the Rust `zfb build`.

## Compatibility settings retained

From the Vite+ migration (stage 3), each still present and still needed for parity:
`clearMocks: false` in every Vitest config, `sharedViteServer: false` at the root, and
`extends: false` on both root inline projects. Kept with comments: the `vite` catalog override and
the `vite`-only `peerDependencyRules`. Removed: the migrator's `minimumReleaseAgeExclude` (no-op
under `minimumReleaseAge: 0`), its `vite.config.ts` lint/test blocks, and its Prettier rewrite
(Prettier was replaced on its own in stage 4). Removed for pnpm 12: `confirmModulesPurge`, which
pnpm 12 rejects. No `ignoreDeprecations` was added anywhere.

## CI wiring and security stayed intact

- `actionlint` passes; every `uses:` line is pinned to a 40-character SHA.
- Health's `wasm_md` detector lists `vite.config.mjs`, `vitest.config.mjs`, `scripts/package-tsc.mjs`,
  `pnpm-workspace.yaml` (the catalog) and `pnpm-lock.yaml`. A catalog change always moves the
  lockfile, which the router, zudo-react, wind and docs filters already watch.
- `wasm-md browser` skipped on the earlier red stage runs only because it `needs: health`; it ran
  and passed on the green head.
- Required statuses are unchanged; no workflow trigger or required-check name moved.
- The audit result is identical to the baseline.

## Cleanup and failure propagation

- Both locally failed `test:workspace` runs exited 1 through `ERR_PNPM_RECURSIVE_RUN_FIRST_FAIL`.
  No `run-parallel`, history-server, `vp`/Vitest or pnpm process from this worktree remained after
  them or after the passing runs.
- `docs-dev-supervisor` (SIGINT/SIGTERM, exit status, descendant reaping) passes in CI and locally.

## Follow-ups

| Issue | State | What remains |
| --- | --- | --- |
| [#3626](https://github.com/Takazudo/zudo-front-builder/issues/3626) | open | Remeasure the Darwin arm64 contract `decision.json`. It records the integrated lock hash but its totals predate this epic, so its gzip values are expected to move by bytes as Linux did. CI does not enforce it |
| [#3630](https://github.com/Takazudo/zudo-front-builder/issues/3630) | open, `deferred-verification` | Vitest 4+ makes the root 5 s timeout bind synchronous tests. Reproduced again here under load 7 to 28: `smoke-clean-room` once, `island-size-budget` once; the root project alone passes 25 / 536. CI passed |
| [#3631](https://github.com/Takazudo/zudo-front-builder/issues/3631) | open | Public decision on the `create-zfb` template's `baseUrl` and `typescript ^5.6.0` before scaffolds move to TS 6+ |
| zudo-doc upstream `baseUrl` | **no issue filed** | `@takazudo/zudo-doc@5.27.0/tsconfig.base.json` sets `baseUrl`; `docs/tsconfig.json` inlines it without `baseUrl` (stage 2) and must be re-diffed on a zudo-doc bump |
| pnpm 12.9.x | none needed | 12.8.2 was chosen over a then 12-hour-old 12.9.1; moving later is optional |

## Not verified, and why

| Evidence | Status |
| --- | --- |
| Local `test:workspace` green | **Not obtained.** Two runs on the loaded 8 GiB host failed only by the #3630 5 s timeout, on different cases; CI is the evidence |
| Local cargo, nextest, doctests, env gates, md-wasm build, docs strict build, browser lanes | Not run (8 GiB host rule); the Linux CI runs above stand in |
| macOS heavy lanes (`exam.yml`: nextest + doctests on macOS) | Not run on this branch. It is schedule/dispatch only; the newest macOS evidence predates the epic |
| `test:webkit-back` (T4, Mac only) and `test:wind-real-build` (local only) | Not run; neither has a CI home |
| Darwin island-size contract | Not remeasured (#3626) |
| Windows | No lane exists |
| pnpm 12 `publish` with provenance, and `release.yml` on pnpm 12 / action-setup v6.1.0 | Runs only at release |
| `pnpm docs:dev` with the real zudo-doc stack | Not started; the supervisor tests cover ZFB + history startup, signals and reaping with the docs workspace's `run-parallel` |
| drift-net, security-audit, supervisor-watch, yaml-candidate-watch | Scheduled only; not triggered by this branch |
