# Toolchain modernization: stage 2 TypeScript 6 evidence (#3556)

Compared with the stage 0 ledger ([`BASELINE.md`](./BASELINE.md), `baseline.json`) and stage 1
([`STAGE-1-PNPM.md`](./STAGE-1-PNPM.md)). Implementation base: `8ca7ebcd` (stage 1 merged into
`base/sweep-261001-toolchain-modernization`). Host: macOS 26.6.1 arm64, 8 GiB, Node v24.14.0,
pnpm 12.8.2, shared and loaded (load average 9.8 to 12.9 during the checks). Every command ran in
the foreground. No TypeScript 7 was adopted.

## Selected typescript@6.0.3, pinned exactly

`typescript@6.0.3` (published 2026-04-16) is the final 6.x patch, as stage 0 selected. Engines
`>=14.17`, `bin.tsc` = `./bin/tsc`, no `exports` map. It replaces `^5.9.0` in all five manifests
that declared a compiler: `packages/zfb`, `packages/zfb-runtime`, `packages/zfb-adapter-cloudflare`,
`crates/zfb-md-wasm/npm` and `docs`. The pin is exact so the checkpoint names one compiler; stage 5
replaces it with the TS7 pin.

The `docs` compiler moves too. It is a contributor tool (`zfb check` runs the project-local
`node_modules/.bin/tsc`), not one of the frozen docs product pins: `@takazudo/zfb*` 2.20.2 and
`@takazudo/zudo-doc*` 5.27.0 are unchanged.

`packages/zfb`, the adapter and md-wasm also gain `typescript-5.9` (`npm:typescript@5.9.3`) for
the consumer-floor probe below. The `create-zfb` template keeps `typescript: ^5.6.0`; it is a
public consumer declaration.

### Resolved graph

| Item | Value |
| --- | --- |
| Compiler copies in the lockfile | `typescript@6.0.3` (5 importers) and `typescript@5.9.3` (3 `typescript-5.9` aliases) |
| Other packages depending on `typescript` | none; no dependency or peer edge to it in the lockfile |
| `node_modules/.bin/tsc` | 6.0.3 in every importer, including the three with the alias (both manifests are named `typescript`; pnpm links the higher version) |
| `pnpm-lock.yaml` SHA-256 | stage 1 `c8b344b9…852b`; after the TS6 bump `9426d83d…fbcd`; after the alias `024082220a02df2a4c4deb5bd5455af776b78c0c0dd7cf4a5064d12c5388f6e2` |
| Lockfile churn | only the five `typescript` specifiers, the three alias entries and the package entries; a repeat install is "Already up to date" |

Both island-size contracts record the final lockfile SHA-256 (`toolchain.pnpmLockSha256`); the
README notes why the totals are unchanged.

## Compiler-API and internal-path audit found two launchers and no API users

| Site | Before | After |
| --- | --- | --- |
| `packages/zfb-adapter-cloudflare/scripts/check-packed-types.mjs` (Finding A) | `require.resolve("typescript/bin/tsc")` | `resolvePackageTsc(packageDir)` from `scripts/package-tsc.mjs` |
| `packages/zfb/scripts/zudo-react-packed.mjs` (stage 0's second site) | `join(packageDir, "node_modules/typescript/bin/tsc")` | same helper |
| `crates/zfb-md-wasm/npm/test/package-browser.test.ts` | `node_modules/.bin/tsc` | same helper, so it can also run the 5.9 probe |
| `crates/zfb-md-wasm/npm/scripts/build.mjs`, `tests/zudo-react-browser/build-fixture.mjs` | `node_modules/.bin/tsc`, `pnpm --filter @takazudo/zfb exec tsc` | unchanged: the package manager's bin shim is a supported launch |
| Rust `zfb check` | `<project>/node_modules/.bin/tsc`, then `PATH` | unchanged (product behavior) |

`git grep` finds no `from "typescript"`, `require("typescript")` or `typescript/lib/*` import in
scripts, tests or packages, and no installed package depends on `typescript`. No tool needs the
legacy compiler API, so no exception is recorded.

### `scripts/package-tsc.mjs`

`resolvePackageTsc(packageDir, specifier = "typescript")` walks `node_modules` upward from the
package directory, reads the compiler's `package.json`, checks that it is named `typescript`, and
returns `{ version, binPath, command, args }`. A bin with a `.js`/`.cjs`/`.mjs` extension or a
`node` shebang runs through `process.execPath`; anything else runs directly. It deliberately skips
Node's `NODE_PATH` and global-folder fallbacks: pnpm's bin shims export
`NODE_PATH=<root>/node_modules/.pnpm/node_modules`, and the first test run proved that
`createRequire` from an empty directory then found the hoisted 6.0.3 instead of failing.

Callers resolve the compiler from the package before entering the isolated consumer directory. The
adapter script resolves both compilers before it creates its temp directory, so a missing compiler
leaves nothing to clean up. Each launch keeps its `cwd` and `--project` path. A nonzero exit still
throws, and a signal is now named (`signal SIGKILL`, not `exit signal`). The `finally` cleanup is
unchanged.

Against the real published `typescript@7.0.2` package (extracted, not installed):
`require.resolve("typescript/bin/tsc")` fails with `ERR_PACKAGE_PATH_NOT_EXPORTED`, reproducing
Finding A, and `resolvePackageTsc` returns version 7.0.2 with `node <pkg>/bin/tsc`. TS7's `bin/tsc`
is `#!/usr/bin/env node` + `import "../lib/tsc.js"` in a `"type": "module"` package, so the node
launch applies. Running it needs the native optional dependency and belongs to stage 5.

`scripts/__tests__/package-tsc.test.mjs` (7 cases) covers a TS7-shaped package whose `exports`
hide `./bin/tsc`, a TS6-shaped package, a non-node bin, the alias, a non-`typescript` alias, the
no-compiler failure, and the real workspace pins.

## Deprecated compiler settings repaired without `ignoreDeprecations`

No config uses `ignoreDeprecations`. A scan of all 21 tracked tsconfig files for TS6 deprecations
(`baseUrl`, `moduleResolution` node10/classic, `outFile`, `downlevelIteration`, ES3/ES5 targets,
AMD/UMD/System modules, `esModuleInterop`/`allowSyntheticDefaultImports`/`alwaysStrict` false and
the removed 5.0 options) found only `baseUrl`.

| Site | TS6 result before | Change | Proof of unchanged behavior |
| --- | --- | --- | --- |
| `packages/zfb-adapter-cloudflare/tsconfig.consumer.json` | TS5101 | dropped `baseUrl`; the `paths` target was already `./src/index.ts` | `--traceResolution` still maps `@takazudo/zfb-adapter-cloudflare` to `./src/index.ts`; the program file list is identical under 5.9.3 and 6.0.3 |
| `docs/tsconfig.json` (Finding B) | TS5101, from the local **and** the inherited upstream `baseUrl` | inlined zudo-doc 5.27.0's base config without `baseUrl` | see below |
| `crates/zfb/templates/basic-blog/tsconfig.json` | TS5101 under TS6 | **not changed**: it ships to users, whose `^5.6.0` resolves 5.9.x | public-consumer change; left to stage 5's consumer-range work |
| six Rust fixture tsconfigs, the Rust-generated synthetic esbuild tsconfig | not compiler configs | not changed | ZFB's own alias contract |

### Docs: the upstream `baseUrl` is a real blocker, solved locally

`@takazudo/zudo-doc@5.27.0/tsconfig.base.json` sets `baseUrl: "."`. Removing only the local
`baseUrl` still fails TS5101, and the inherited value anchors the `paths` at the zudo-doc package
directory. Overriding with `"baseUrl": null` satisfies tsc (5.9.3 and 6.0.3 both accept it, and the
module-resolution trace matched the original), but esbuild 0.25.12 (ZFB's bundler) ignores the
`null` and keeps the inherited value: a probe importing `react` failed with
`Could not resolve "react"`. `crates/zfb-plugin-resolver` reads `baseUrl` with `as_str()` and
behaves the same way. So `null` would have silently changed the product build.

`docs/tsconfig.json` now copies the upstream `compilerOptions` and `files` with `baseUrl` removed,
adds `types: ["node"]`, and keeps the four `paths` relative to `docs/` (`@/*` → `./src/*`; `react`,
`react/jsx-runtime`, `react-dom` → `./node_modules/preact/...`). The dependency itself is untouched.
Evidence:

| Check | Result |
| --- | --- |
| esbuild 0.25.12 bundle of a probe importing all four aliases and JSX | resolved targets identical to the original config, and the output bundle byte-identical (`cmp`) |
| tsc module resolution (probe + every docs file), 5.9.3 original vs 6.0.3 new | identical apart from TS6 no longer probing `@typescript/lib-*` replacement packages |
| Program file list, 5.9.3 original vs 5.9.3 new | identical files; zudo-doc declarations are now reached through the `docs/node_modules` symlink path instead of the `.pnpm` real path |
| `pnpm --filter docs check` (`zfb check`, zfb 2.20.2) | `✓ checked 2 collections and tsc — no errors`, using tsc 6.0.3 |

`docs/CLAUDE.md` records why the file is a copy and that a zudo-doc bump must re-diff it. Reporting
the upstream `baseUrl` to zudo-doc is a separate follow-up; nothing here depends on it.

### Ambient types: TypeScript 6 stopped auto-loading `@types/*`

TS6 defaults `types` to `[]`. The three SDK fixture configs pulled `src/content.ts` and failed with
`Cannot find name 'process'`/`NodeJS` until `types: ["node"]` was made explicit in
`tsconfig.zudo-react-fixture.json` (inherited by the dev fixture) and
`tsconfig.zudo-react-sdk-fixture.json`. With it, each fixture's program has exactly the files
5.9.3 loaded (148 to 150 non-lib files, zero difference), and both compilers exit 0, so every
`@ts-expect-error` in the fixtures still fires. The packed zudo-react consumer now writes
`types: []`, which is what 5.9.3 resolved in an empty temp directory. The package build configs
already declared `types: ["node"]` (adapter consumer config too) and the docs config gained it.

Program file lists for every config, 5.9.3 vs 6.0.3, after the changes:

| Config | Result |
| --- | --- |
| zfb, zfb-runtime, adapter, adapter consumer | identical, both exit 0 |
| 3 SDK fixtures | identical, both exit 0 |
| md-wasm `tsconfig.json` | identical; both fail the same 4 TS2307 errors locally because the wasm artifacts are not built on this host |
| `tests/zudo-react-browser/fixtures` | identical; both fail standalone (needs the staged packed SDK) |
| md-wasm `packed-consumer` run in place | 5.9.3 loads `@types/node` from the package directory, 6.0.3 does not. In its real use (a tarball-only temp consumer with no `@types`) both resolve no ambient types, so CI is the proof |
| `create-zfb` template | TS2688 in both (no install); plus TS5101 under 6.0.3 (see the table above) |

## Emitted output is byte-identical

`dist/` was built from clean with 5.9.3 at the base and with 6.0.3 after the change, then compared
with `diff -r`:

| Package | Files | Difference |
| --- | ---: | --- |
| `@takazudo/zfb` | 114 | none |
| `@takazudo/zfb-runtime` | 42 | none |
| `@takazudo/zfb-adapter-cloudflare` (tsc + copied `worker-wrapper.mjs`, `emit-worker.mjs`) | 8 | none |
| md-wasm `tsc` emit (into a scratch dir; the full `scripts/build.mjs` needs wasm) | 45 | none |

Packed with pnpm 12.8.2 and compared with `baseline.json` → `packedTarballs`:

| Tarball | Files | Unpacked bytes (baseline) | Only difference | tgz SHA-256 |
| --- | ---: | --- | --- | --- |
| `takazudo-zfb-3.1.0.tgz` | 120 | 978,500 (978,455) | `package.json` devDependencies | `803ec3a17fdef741cf59500fb7e25bc56adfb9432e6251bd130989f813eabe5a` |
| `takazudo-zfb-runtime-3.1.0.tgz` | 46 | 446,277 (446,278) | `package.json` devDependencies | `d24b3badb7108b3c1460a66853467636d3c201a94732da9502199b3a31b3e8ea` |
| `takazudo-zfb-adapter-cloudflare-3.1.0.tgz` | 15 | 73,357 (73,312) | `package.json` devDependencies | `1bd640df733452ad74e277954ed92faad398e0e66921e01679879d5505910128` |
| `create-zfb-3.1.0.tgz` | 7 | 11,120 (11,120) | none; same SHA-256 as stage 1 | `ffd2c2273ad4a929c2b4c05b7374e46750b20ab53ddf04711c0d6e1dca779a3b` |

Because the SDK and runtime JS are unchanged, the island-size totals cannot move; only the lockfile
provenance hash changed.

## TS 5.9 consumers keep a non-regression probe

Before this stage the packed checks compiled with the workspace's 5.9.3, which doubled as the
consumer probe. They now compile twice: with the package's pinned compiler and with the
`typescript-5.9` alias.

| Check | Result |
| --- | --- |
| `pnpm --filter @takazudo/zfb-adapter-cloudflare test:packed-types` | `passed strict tsc 6.0.3`, then `passed strict tsc 5.9.3` |
| `node packages/zfb/scripts/zudo-react-packed.mjs check` | build, pack, stage, packed exports and DOM-free server import PASS; production and development types PASS under 6.0.3 and 5.9.3; esbuild probe skipped (no staged binary), as at the baseline |
| md-wasm "tarball-only isolated consumer" test | now runs both compilers; **not run locally** (needs the wasm build), CI's `wasm-md (default)` leg runs it |

## Workspace gates pass locally under TypeScript 6.0.3

| Gate | Result |
| --- | --- |
| `pnpm format:check` | pass, 11.1 s |
| `pnpm typecheck:workspace` | pass, 5.5 s (adapter + consumer, zfb, runtime) |
| `pnpm test:workspace` | pass, 43.3 s: 7 projects, 102 files, **1,622** cases (stage 1: 101 / 1,615, plus the new 7-case `package-tsc` suite). Root 24 / 513, zfb 49 / 720, runtime 17 / 300, adapter 5 / 40, create-zfb 2 / 24, docs 5 / 25 |
| Compound steps | the zfb fixtures (zudo-react, dev, sdk) and the adapter `typecheck:consumer` + `test:packed-types` (both compilers) ran inside `test:workspace` |
| `pnpm --filter docs check` | pass (see Docs above) |

Timings are single runs on a loaded host and are not a speed claim; stage 5 owns the alternating
measurement.

## CI path filters

`scripts/package-tsc.mjs` is now imported by the md-wasm test and by `zudo-react-packed.mjs`, so it
was added to health's `wasm_md` filter and to both `zudo-react-browser.yml` path lists. The adapter
check runs inside the unfiltered `health` job.

## CI must prove

- `health`: `test:workspace` and `typecheck:workspace` on Linux with tsc 6.0.3, including the
  adapter's two-compiler packed check; the island-size step with the new `pnpmLockSha256` and
  unchanged Linux totals (the emitted SDK/runtime JS is byte-identical, so a moved total would be a
  finding, not an expected rebaseline).
- `wasm-md (default)`: the md-wasm build with tsc 6.0.3, `typecheck:consumer`, and the tarball
  consumer compiled by 6.0.3 and 5.9.3.
- `Docs Checks` and `PR Preview`: the strict docs build on the inlined `docs/tsconfig.json`
  (aliases and JSX through zfb 2.20.2's esbuild), `zfb check` with 6.0.3, `check:html` and
  `check:islands`.
- `zudo-react-browser` (path filter: `pnpm-lock.yaml`, `packages/zfb/**`, the helper): the fixture
  compiled by `pnpm --filter @takazudo/zfb exec tsc` 6.0.3 against the staged packed SDK.
- `router-chromium`, `wind-computed-style`, Scaffold E2E and the local smokes (lockfile filter): the
  packed SDK/runtime built by 6.0.3.

## Regressions versus the baseline: none found

No check that passed at the baseline fails here. The only behavior changes are the intended ones:
TS6 deprecation and ambient-type defaults handled explicitly, and the packed checks running a
second compiler.

## Open items for later stages

- `create-zfb` template `baseUrl`: a scaffolded project fails TS5101 if its owner upgrades it to
  TypeScript 6. Changing the template is a public-consumer change for stage 5's consumer-range
  decision.
- zudo-doc's published `tsconfig.base.json` carries `baseUrl`; an upstream fix would let
  `docs/tsconfig.json` extend it again after a future zudo-doc bump.
- `typescript-5.9` must stay until stage 5 defines the supported consumer TypeScript range.

## Not run here

Cargo builds and tests, the md-wasm wasm build and its package/browser tests, the docs strict build,
browser lanes, and running a TS7 binary were not run, per the 8 GiB host rule. Windows has no lane.
