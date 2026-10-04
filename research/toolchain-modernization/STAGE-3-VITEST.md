# Toolchain modernization: stage 3 Vitest 4.1 and Vite+ evidence (#3557)

Compared with the stage 0 ledger ([`BASELINE.md`](./BASELINE.md), `baseline.json`) and the stage 2
state (`e2469233`: 7 projects, 102 files, 1,622 cases). Local host as recorded there (macOS 26.6.1
arm64, 8 GiB, Node v24.14.0, pnpm 12.8.2, shared and loaded: load averages 8 to 39 during this
stage). Every command ran in the foreground with stdin closed (`</dev/null`) and `CI` unset.

Two separately committed checkpoints:

| Checkpoint | Commit | Runner | `pnpm-lock.yaml` SHA-256 |
| --- | --- | --- | --- |
| Stage 2 base | `e2469233` | Vitest 2.1.9 on Vite 5.4.21 | `024082220a02df2a4c4deb5bd5455af776b78c0c0dd7cf4a5064d12c5388f6e2` |
| A: Vitest prerequisite | `a373044f` | Vitest 4.1.11 on Vite 8.3.2 | `1d26a679e95af209cc2c53dc86e72e0900cbf2354c4b69358f0f03f0d87cdcb5` |
| B: Vite+ | `95c7d23e` | `vite-plus` 1.0.0 (Vitest 5.0.1, core 1.0.0) | `69255f60af5d64b7e1300d7276bf9beb2736b29b10ced6616dee90237a0e0b28` |

Both island-size contracts record the checkpoint B hash. Each checkpoint's repeat `pnpm install` and
`pnpm install --frozen-lockfile` left the lockfile byte-identical. Hono 4.12.25 is unchanged.

## Vitest 4.1.11 resolves Vite 8.3.2 and needed one test fix

`vitest@4.1.11` (the stage 0 selection) declares `vite ^6 || ^7 || ^8`; pnpm resolved 8.3.2, which
also satisfies the Vite+ migrator's "Vite 8+ and Vitest 4.1+" prerequisite. Vite only enters the
graph through the test runner. The docs host's product dependencies (`@takazudo/zfb*` 2.20.2,
zudo-doc 5.27.0) do not depend on Vite. The md-wasm `vite-6.4.3` browser fixture has its own
lockfile and workspace marker, so neither checkpoint touches it.

- `vitest.workspace.mjs` used `defineWorkspace`, which Vitest 4 removed. Its two projects moved into
  `test.projects` in the root `vitest.config.mjs`. The projects are inline and do not extend the
  root config, so `scripts-subprocess` now `include`s exactly `SUBPROCESS_SUITES` and `scripts`
  excludes them. The old `exclude`-only carving and its extglob existed only to dodge `extends`
  array concatenation.
- The fake-timer defaults changed. Vitest 2 faked `setTimeout`, `clearTimeout`, `setInterval`,
  `clearInterval`, `setImmediate`, `clearImmediate` and `Date`. Vitest 3+ fakes every available
  timer API except `nextTick` and `queueMicrotask`, including `performance`,
  `requestAnimationFrame` and idle callbacks. The three projects that call `vi.useFakeTimers()` (zfb,
  zfb-runtime, create-zfb) now pin the Vitest 2 list explicitly. All tests also passed without it.
- One test failed: `runtime.test.ts` "keeps the marker absent until a visible island intersects"
  passed an arrow function to `vi.fn` and the runtime calls it with `new`. Vitest 4 mocks are only
  constructible from a constructible implementation, so the mock now uses `function`. The assertions
  are unchanged.

## Vite+ 1.0.0 migrated cleanly once its Prettier step was reverted

`vite-plus@1.0.0` is still `latest` (published 2026-09-28). Engines `^22.18.0 || ^24.11.0 ||
>=26.0.0`. It bundles exact `vitest 5.0.1`, `oxfmt =0.70.0` and `oxlint =1.85.0`, and aliases
`vite` to `@voidzero-dev/vite-plus-core@1.0.0`.

Help came from `pnpm dlx --package=vite-plus@1.0.0 vp migrate --help`. Its `--hooks` setup is the
default in non-interactive mode. The migration ran from the workspace root against checkpoint A's
manifests, lockfile and installed packages, with no Vite+ preinstalled in the workspace:

```sh
npx -y pnpm@12.8.2 dlx --package=vite-plus@1.0.0 \
  vp migrate --no-interactive --no-agent --no-editor --no-hooks </dev/null
```

It reported "1 config update applied, 124 files had imports rewritten", "Prettier migrated to Oxfmt"
and installed dependencies. **No CLI flag disables its Prettier step.** The migrator detected
`.prettierrc.json`, deleted it, wrote an `fmt` block into a new root `vite.config.ts`, rewrote
`format:ts`/`format:check:ts` to `vp fmt`, removed `prettier`, and formatted the changed files. The
clean pre-migration commit made this reviewable. No `.md`/`.mdx` file, fixture, template, golden file
or `.prettierignore` entry changed.

| Migrator change | Disposition |
| --- | --- |
| `vitest` dependency → `"vite-plus": "catalog:"` in 7 manifests (root, docs, zfb, zfb-runtime, adapter, create-zfb, md-wasm) | **kept** |
| `catalog: { vite: npm:@voidzero-dev/vite-plus-core@1.0.0, vite-plus: 1.0.0 }`, override `vite@*: catalog:` | **kept**, commented in `pnpm-workspace.yaml` |
| `peerDependencyRules` accepting any `vite` peer (the core alias reports 1.0.0) | **kept**, scoped to `vite` only |
| `minimumReleaseAgeExclude` for Vite+/Oxc/Vitest packages | **removed**: a no-op under the existing `minimumReleaseAge: 0` |
| `vitest run`/`vitest` → `vp test run`/`vp test` in 7 `test`/`test:watch` scripts | **kept**. Only the Vitest token changed in the compound zfb, adapter and md-wasm scripts |
| `vitest` → `vite-plus/test` (117 files) and `vitest/config` → `vite-plus` (7 configs) | **kept**. Import lines only, verified by diff |
| `.vitest/` added to `.gitignore` (Vitest 5 artifact directory) | **kept** |
| Root `vite.config.ts` with `fmt` (Oxfmt), `lint` (Oxlint `typeAware`/`typeCheck`, `prefer-vite-plus-imports`) and `test.clearMocks` | **removed**. Oxfmt is stage 4 (#3558); a broad Oxlint scope is out of scope; `vp test` reads `vitest.config.*` |
| `.prettierrc.json` deleted, `prettier` removed, `format:*:ts` → `vp fmt` | **reverted**. Prettier stays at 3.8.3, the baseline version, restored in the lockfile |
| Oxfmt re-wrap of one line in `crates/zfb-md-wasm/npm/test/workerd.test.ts` | **reverted** |
| Generic agent/editor/hook setup | not run (`--no-agent --no-editor --no-hooks`); Lefthook and the custom pre-push hook untouched |

### Compatibility settings retained for first validation

| Setting | Where | Kept because |
| --- | --- | --- |
| `clearMocks: false` | every config (root and both root projects, zfb, zfb-runtime, adapter, create-zfb, docs, md-wasm) | Vitest 5 clears mock history before each test; no suite was audited for reliance on earlier calls |
| `sharedViteServer: false` | root | Vitest 5 shares one Vite server across inline projects |
| `extends: false` | both root inline projects | Vitest 5 inline projects inherit the root config by default. Inheritance would merge arrays into `include`/`exclude` and could run a suite twice |

None was removed. Removing one needs a separate change with suite-parity evidence.

### Review items: none needed a source change

| Item | Finding |
| --- | --- |
| `node-runtime` on `packages/create-zfb` (`>=18`) and md-wasm (`>=20`) | public consumer `engines` are left as they are. The contributor runtime is the root `engines.node` (below) |
| `unawaited-assertion` md-wasm `api.test.ts:484-485` | false positive: both `expect(...).rejects` sit inside an awaited `Promise.all` |
| `class-mock` `runtime.test.ts:1102` | the `IntersectionObserver` mock from checkpoint A; it returns a plain object and nothing checks `instanceof` |
| `static-list` in `gh-stub.sh`, `publish-npm-packages.sh`, `run-b4push.sh` | false positives: `gh run list` and prose. No `vitest list` caller exists |
| `dom-global` `tests/built-site-smoke/lifecycle.chromium.spec.mjs:20` | a Playwright spec in a real browser, not a Vitest DOM environment |
| `.prettierignore` vs `ignorePatterns` warning | belongs to stage 4 |

`pnpm peers check` reports one unmet optional peer: `html-validate@10.17.0` wants `vitest` `^1 || ^2 ||
^3 || ^4.0.1` and sees 5.0.1. Nothing imports `html-validate/vitest`; the CLI is unaffected.

## Contributor Node is now `^22.18.0 || ^24.11.0 || >=26.0.0`

The root `engines.node` moved from `>=22.0.0` to the Vite+ range. With `engineStrict: true`, an older
contributor Node fails `pnpm install` instead of failing later inside `vp`. `BUILDING.md` and
`CONTRIBUTING.md` now state that floor. Published `engines` are unchanged (zfb, zfb-runtime, the
adapter `>=22.0.0`; create-zfb `>=18`; md-wasm `>=20.0.0`). CI keeps `node-version: 22`, which
setup-node resolves to the newest 22.x (22.23.3), and the island-size job's measurement runtime stays
24.14.0. Both are inside the range, so no workflow changed.

## Suite parity holds at both checkpoints: 7 projects, 102 files, 1,622 cases, each run once

A scratch reporter (onFinished for Vitest 2, onTestRunEnd for 4/5) recorded each project's
resolved config and every case's full name, mode and state. Each project ran directly with its
runner. Every file ran exactly once, nothing was skipped, and every project matches file for file.

| Project | Files | Cases | Env | `testTimeout` | Exact title match (2 → Vite+) |
| --- | ---: | ---: | --- | ---: | --- |
| root `scripts` | 20 | 415 | node | 5,000 | all |
| root `scripts-subprocess` | 4 | 98 | node | 90,000 | 3 of 4 |
| `packages/zfb` | 49 | 720 | happy-dom | 5,000 | 45 of 49 |
| `packages/zfb-runtime` | 17 | 300 | node | 5,000 | all |
| `packages/zfb-adapter-cloudflare` | 5 | 40 | node | 19,000 | all |
| `packages/create-zfb` | 2 | 24 | node | 5,000 | all |
| `docs` | 5 | 25 | node | 5,000 | all |

The five title mismatches are formatting of parameterized titles only. Case counts per file are
identical and the names match after normalizing quotes, truncation and `+0`. Affected files:
`iframe-shell` (`%j` now prints symbol keys and `undefined`), `dialect-diagnostics`,
`jsx-contract-matrix` and `md-roundtrip` (`%s` drops quotes and truncates at a different width), and
`supervisor-watch` (`0` instead of `+0`).

Other resolved settings are unchanged from Vitest 2: `hookTimeout` 10,000, `pool: forks`,
`isolate: true`, `clearMocks`/`mockReset`/`restoreMocks`/`unstubGlobals`/`unstubEnvs` false,
`globals` false, no retry. The fake-timer `toFake` list matches Vitest 2 in the three projects that
use fake timers. No config declares reporters, coverage or browser mode.

Per-file environments still apply under Vite+. All 27 docblocks (zfb 15 node + 2 explicit happy-dom,
zfb-runtime 10 happy-dom) are untouched. A temporary probe (deleted) confirmed `document` is
undefined under a zfb `@vitest-environment node` docblock, defined under the zfb default and under a
zfb-runtime `happy-dom` docblock. Per-suite timeouts are unchanged in source: `docs-dev-supervisor`
90 s, `harvest-supervisor-timelines` 20 s, `launcher.test.ts` 15 s, md-wasm `workerd.test.ts`
30 s. Test-file diffs are import lines plus the one mock fix.

md-wasm (excluded from `test:workspace`) keeps node and 15 s. `vp test list` with Vitest 5's static
parser collects the same 14 files. Running it needs the wasm build, so its 234 cases are left to CI.

The `[supervisor-timeline]` identity is unchanged: `runner=pnpm`, `zudoDoc=5.27.0`, and the
`runParallel`, `fixtureShape` and `env=v1:sha256:bccd2371eea473c3` digests matched under Vitest 2 and
Vite+, with the same three cases emitted.

## Vitest 4+ makes the root 5 s timeout bind synchronous tests (regression risk)

**This changes what the 5 s means.** Vitest 2's `withTimeout` was a bare `Promise.race`, so a test
blocked synchronously (for example in `execFileSync`) could not time out at any host load. The
`SUBPROCESS_SUITES` split rested on that: #3061 measured `changelog-layout` at 30.1 s on a saturated
host, passing. `@vitest/runner` 4.1.11 adds a check after the test returns, `if (now() - startTime >=
timeout) rejectTimeoutError()` (vitest-dev/vitest#2920). Vitest 5.0.1 behaves the same. The ordinary
root suites now fail once they pass 5 s wall-clock.

| Evidence (this host) | Result |
| --- | --- |
| Vitest 2 baseline, all six projects run concurrently (load 9.6) | `island-size-budget` "different platform" **7,214 ms** and "stale file hashes" 5,249 ms, both **passed** |
| Root `scripts` project alone, 3 alternating rounds | max 2,579 ms (Vitest 2) vs 2,394 ms (Vite+). The same tests, no slowdown |
| Canonical `pnpm test:workspace`, Vitest 4.1.11, two runs (load 9 to 16) | both **failed**: `island-size-budget` "different platform" timed out at 5,000 ms |
| Canonical `pnpm test:workspace`, Vite+, run 1 (load 9) | **failed**: two `smoke-clean-room` cases timed out at 5,000 ms |
| Canonical `pnpm test:workspace`, Vite+, run 2 (load 28) | **passed**: 7 projects, 102 files, 1,622 cases, 38.0 s |

The tests are no slower. Concurrent workspace runs on a loaded host have always pushed them past 5
s, and Vitest 2 hid that. The timeouts stay at their contract values (root 5 s, subprocess 90 s,
adapter 19 s, md-wasm 15 s). Under repository rule 8, raising one needs a linked issue and a value
taken from a measured distribution, so this stage leaves the decision to the manager. CI (Linux x64,
root project 9.14 s total at the baseline) has more headroom but can see the same failure. The
config comment and `CLAUDE.md` now state the Vitest 4+ semantics, and `CLAUDE.md`'s stale "60 s"
now reads 90 s.

## Gates pass locally on the Vite+ checkpoint

| Command | Result |
| --- | --- |
| `pnpm format:check` (Prettier 3.8.3 + mdx-formatter 1.2.1) | pass, 11.4 s |
| `pnpm typecheck:workspace` | pass, 6.9 s |
| `ZFB_SUPERVISOR_TIMELINE=1 pnpm test:workspace` | pass on run 2 (above); run 1 hit the timeout semantics above |
| Compound steps inside it | `vp test run` then the three zudo-react fixture `tsc -p` compiles (zfb); `typecheck:consumer`, `vp test run`, `test:packed-types` passing strict tsc 6.0.3 and 5.9.3 (adapter) |
| `pnpm --filter @takazudo/zfb --filter @takazudo/zfb-runtime --filter @takazudo/zfb-adapter-cloudflare build` | pass, 6.9 s |

`--include-workspace-root` and the `!@takazudo/zfb-md-wasm` filter in `test:workspace` are unchanged.
`test:md-wasm` still builds and then tests. `health.yml` and `scripts/run-b4push.sh` still call the
same `pnpm test:workspace`.

## Startup is faster or neutral (loaded host, alternated)

Three rounds alternated the stage 2 base (Vitest 2, a temporary detached worktree at `e2469233`) and
checkpoint B on this host, warm caches, load 13.8 to 22.3:

| Command | Vitest 2 runs (s) | Median | Vite+ runs (s) | Median |
| --- | --- | ---: | --- | ---: |
| `pnpm --filter create-zfb test` (startup proxy, 24 cases) | 7.20 / 4.85 / 5.14 | **5.14** | 3.00 / 3.01 / 3.15 | **3.01** |
| `pnpm --filter @takazudo/zfb-runtime exec <runner>` (300 cases) | 12.47 / 8.00 / 7.96 | **8.00** | 6.66 / 7.05 / 7.27 | **7.05** |
| `pnpm exec <runner> --project scripts` (415 cases) | 18.65 / 17.98 / 16.05 | **17.98** | 19.47 / 18.66 / 17.38 | **18.66** |

These are noisy single-host numbers, not a speed claim. The root project is execution-bound and
neutral.

## CI path filters need no change

No new tooling config file remains: `vite.config.ts` was dropped and the runner reads the existing
`vitest.config.*` files. `vitest.workspace.mjs` was never in a filter. Health's `wasm_md` filter
already lists `package.json`, `pnpm-workspace.yaml` (the catalog and override),
`pnpm-lock.yaml` and `vitest.config.mjs`. Docs, router-chromium, zudo-react-browser and
wind-computed-style filter on the lockfile. Stage 4 must add the Oxfmt config file when it creates
one.

## CI must prove

- `health`: `test:workspace` with Vite+ on Linux x64 Node 22.23.3, including the compound zfb fixture
  compiles and the adapter packed checks, with 102 files / 1,622 cases. Watch for 5 s timeouts in the
  root `scripts` project (above). The same job must also pass `typecheck:workspace`, `format:check`
  and the island-size step with the new `pnpmLockSha256` and unchanged Linux totals.
- `wasm-md (default)`: md-wasm's `vp test run` after the wasm build: 14 files / 234 cases,
  `typecheck:consumer`, and the `vite-6.4.3` fixture's own frozen install (isolated from the new
  workspace override).
- `Docs Checks`: docs `vp test run` (5 / 25) and the strict docs build on the unchanged docs product
  stack.
- `pnpm audit (prod)` with the new dev graph, and the supervisor-timeline records keeping their
  identity on `main`.

## Regressions versus the baseline

- **The root 5 s timeout now binds synchronous suites** (section above). It is reported as a risk,
  not fixed by raising a gate.
- No suite, case or environment was lost or duplicated, and no skip was added. Mock and timer
  defaults are pinned to their earlier behavior.

## Not run here

Cargo builds and tests, the md-wasm wasm build and its 234-case run, the docs strict build, all
browser lanes and Windows were not run, per the 8 GiB host rule. Removing any compatibility setting
was not attempted.
