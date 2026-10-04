# Toolchain modernization: stage 5 TypeScript 7 evidence (#3559)

Compared with stage 2 ([`STAGE-2-TS6.md`](./STAGE-2-TS6.md)) and the stage 0 ledger
([`BASELINE.md`](./BASELINE.md)). Implementation base: `00b8c4fd` (stage 4 merged into
`base/sweep-261001-toolchain-modernization`). Host: macOS 26.6.1 arm64, 8 GiB, 6 CPUs,
Node v24.14.0, pnpm 12.8.2, shared and loaded (1-minute load 3.5 to 7.9 during the checks). Every
command ran in the foreground. The TS6 reference outputs were built on this base before any change.

## Selected typescript@7.0.2, pinned exactly

Registry recheck on 2026-10-04: `latest` is still **7.0.2** (published 2026-07-08), the only stable
7.x. Everything newer is `7.1.0-dev.*` on `next`, and `rc` is the older 7.0.1-rc, so the epic's
stable-not-nightly rule selects 7.0.2.

| Item | 7.0.2 metadata |
| --- | --- |
| Engines | `node >=16.20.0` (CI 22.23.3 and local 24.14.0 both satisfy it) |
| Package | `"type": "module"`; `bin.tsc` = `bin/tsc` (`#!/usr/bin/env node`, `import "../lib/tsc.js"`) |
| `exports` | `.` (`lib/version.cjs`), `./package.json`, `./unstable/*` only. No `./bin/tsc`, no `lib/tsserver.js`, no `typescript.js` compiler API |
| Native binary | 20 `@typescript/typescript-<os>-<cpu>@7.0.2` optional dependencies; pnpm installs one (`darwin-arm64` here, `linux-x64` on CI) |
| Launch | `lib/tsc.js` calls `process.execve` on the native `tsc` when available (Node > 22.15, so both CI and local), and otherwise falls back to `execFileSync` and propagates the exit status |

It replaces `6.0.3` in the same five manifests stage 2 pinned: `packages/zfb`,
`packages/zfb-runtime`, `packages/zfb-adapter-cloudflare`, `crates/zfb-md-wasm/npm` and `docs`.
The `docs` compiler is a contributor tool (`zfb check` runs `docs/node_modules/.bin/tsc`), not a
frozen product pin; `@takazudo/zfb*` 2.20.2 and `@takazudo/zudo-doc*` 5.27.0 are unchanged.

`packages/zfb`, the adapter and md-wasm gain `typescript-6.0` (`npm:typescript@6.0.3`) beside the
existing `typescript-5.9` alias, so the packed-consumer checks compile with all three compilers.

### Every command resolves 7.0.2

| Importer | `node_modules/.bin/tsc` | `pnpm -C <dir> exec tsc` | Aliases |
| --- | --- | --- | --- |
| `packages/zfb` | 7.0.2 | 7.0.2 | 6.0.3, 5.9.3 |
| `packages/zfb-runtime` | 7.0.2 | 7.0.2 | none |
| `packages/zfb-adapter-cloudflare` | 7.0.2 | 7.0.2 | 6.0.3, 5.9.3 |
| `crates/zfb-md-wasm/npm` | 7.0.2 | 7.0.2 | 6.0.3, 5.9.3 |
| `docs` | 7.0.2 (`zfb check` logs `running …/docs/node_modules/.bin/tsc`) | 7.0.2 | none |
| root, `packages/create-zfb` | no `tsc` bin, no `node_modules/typescript` | n/a | n/a |

The compiler call sites are unchanged: package scripts (`tsc`, `tsc -p …`), md-wasm's
`scripts/build.mjs` (`node_modules/.bin/tsc`), `tests/zudo-react-browser/build-fixture.mjs`
(`pnpm --filter @takazudo/zfb exec tsc`), the three packed checks (`scripts/package-tsc.mjs`), and
Rust `zfb check`. The Stage 2 launcher fix is now exercised by the real TS7 package:
`resolvePackageTsc` returns 7.0.2 with `node <pkg>/bin/tsc`, and every packed check below ran it.

### The one remaining 6.0.3 edge is not a compiler consumer

`vite-plus` 1.0.0 (root and `create-zfb`, neither declares a compiler) has an **optional** peer
`typescript: ^5 || ^6 || ^7`. pnpm's `autoInstallPeers` resolves it to 6.0.3 there, exactly as on
the base (the root importer line is unchanged); importers that declare 7.0.2 resolve it to 7.0.2.
The only code in `vite-plus`/`@voidzero-dev/vite-plus-core` that imports `typescript` is the bundled
tsdown (`vp pack`), which this repository does not use: package emit stays on `tsc`, and no Vitest
config enables `typecheck`. No `tsc` bin or hoisted `typescript` is exposed at either importer. So
there is no TS6 exception to declare; forcing this optional peer to 7.0.2 would only give tsdown a
compiler without the API it expects.

### Lockfile

| Item | Value |
| --- | --- |
| Compiler copies | `typescript@7.0.2` (5 importers), `typescript@6.0.3` (3 `typescript-6.0` aliases + the vite-plus optional peer above), `typescript@5.9.3` (3 `typescript-5.9` aliases) |
| Added packages | `typescript@7.0.2` and its 20 platform packages (lockfile metadata; one installed per host) |
| `pnpm-lock.yaml` SHA-256 | stage 4 `781439c5…45a5` → `af4dc9f5a8bd02096ffaea0998067f3898469a78930af35e69a99e5085c956b5` |
| Install | `pnpm install` with stdin closed: "+19 -8", done in 10.6 s. `pnpm peers check` shows only the stage 3 `html-validate`/`vitest` optional-peer note |

Both island-size contracts record the new `toolchain.pnpmLockSha256`; the README explains why the
totals cannot move (next section).

## Emitted JavaScript is byte-identical; maps and one declaration differ harmlessly

`dist/` was built from clean with 6.0.3 at the base and with 7.0.2 after the change (package
`build` scripts, so the adapter's two `cp` helper copies are included), then compared with
`diff -r`. md-wasm's `tsc` emit went to a scratch `outDir` (its full build needs the wasm); both
compilers report the same four TS2307 errors for the unbuilt wasm glue and still emit.

| Package | Files | `.js`/`.mjs` | `.d.ts` | `.js.map` |
| --- | ---: | --- | --- | --- |
| `@takazudo/zfb` | 114 / 114 | identical | identical | 9 differ |
| `@takazudo/zfb-runtime` | 42 / 42 | identical | 1 differs | 4 differ |
| `@takazudo/zfb-adapter-cloudflare` (tsc + copied `worker-wrapper.mjs`, `emit-worker.mjs`) | 8 / 8 | identical | identical | identical |
| md-wasm `tsc` emit | 45 / 45 | identical | identical | 10 differ |

**Source maps.** Only the `mappings` field differs; `version`, `file`, `sourceRoot`, `sources` and
`names` are equal. Decoded, no generated line maps to a different set of source lines (1,920 / 2,836 /
971 generated lines checked in runtime / zfb / md-wasm). The difference is a few column segments:
6.0.3 also emitted a segment at statement ends (between `)` and `;`), which 7.0.2 omits, and 7.0.2
adds one at a parameter default. Segments: zfb 20,395 → 20,382, runtime 10,454 → 10,454, md-wasm
7,373 → 7,342. Debugger stepping granularity only.

**Declaration.** `dist/client-router/swap-functions.d.ts` line 21, inside the `swapFunctions`
object type:

```diff
-    saveFocus: () => (() => void);
+    saveFocus: typeof saveFocus;
```

`saveFocus` is the `export declare const saveFocus: () => (() => void)` in the same file, so the
type is identical. A probe compiled against the 7.0.2 declaration asserts exact type equality
(`swapFunctions.saveFocus` vs `() => () => void` and vs `typeof saveFocus`) and passes under 7.0.2,
6.0.3 and 5.9.3; its negative control (`() => number`) fails as expected. The module is internal:
none of the runtime's four public entries (`.`, `./server`, `./snapshot`, `./client-router`)
reaches it, as shown by an isolated consumer of the packed runtime below.

**Automatic JSX emit.** The SDK and runtime `src` have no JSX, so the comparison above does not
cover the JSX transform. `tests/zudo-react-browser/build-fixture.mjs` (the packed-SDK fixture CI
compiles) was run with 7.0.2 and its tsconfig re-run with 6.0.3: of 19 compiled files 15 are
identical and 4 differ only in the order of the named `jsx-runtime` import specifiers
(7.0.2 sorts them: `import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from
"@takazudo/zfb/zudo-react/jsx-runtime"`). The runtime module, the bindings and every call are
unchanged. A `react-jsxdev` probe through a tsconfig shows the same and only that difference
(`jsx-dev-runtime`, `jsxDEV`; the `_jsxFileName` is equal). The repository's own `react-jsxdev`
fixture is type-only (`noEmit`). No test asserts on tsc's JSX import text (the matching strings in
the Rust and md-wasm tests are SWC/MDX output or fixture input).

### Packed tarballs keep their inventories

Packed with pnpm 12.8.2 and compared with the stage-5-base (6.0.3) tarballs:

| Tarball | Files | Unpacked bytes (6.0.3) | Differences | tgz SHA-256 |
| --- | ---: | --- | --- | --- |
| `takazudo-zfb-3.1.0.tgz` | 120 / 120 | 978,492 (978,504) | `package.json` devDependencies; 9 `.js.map` | `d80ac871622cd105a902f8e068cae7dfa112336b7379d05fc611f3b29e6b99a4` |
| `takazudo-zfb-runtime-3.1.0.tgz` | 46 / 46 | 446,281 (446,281) | `package.json` devDependencies; 4 `.js.map`; `swap-functions.d.ts` | `49a318a45d0e14aa6b4003afac502f3039f5da44ee05a3a349d54a0a7749cbf3` |
| `takazudo-zfb-adapter-cloudflare-3.1.0.tgz` | 15 / 15 | 73,407 (73,361) | `package.json` devDependencies | `6b6386ab05ca104365c3ac8e4eb2d23ac7762f18a41f7362713b100948ad8518` |
| `create-zfb-3.1.0.tgz` | 7 / 7 | 11,124 (11,124) | none; SHA-256 unchanged | `2678397220cd55d7d0a6bd7ee124e23a5ca4754938c62fca2362e4742244bc61` |

The packed `package.json` change is `typescript` `6.0.3` → `7.0.2` plus, for the SDK and adapter, the
`typescript-6.0` alias, all in `devDependencies`. `exports`, `publishConfig`, `files`,
`dependencies` and `peerDependencies` are unchanged, and none of them names `typescript`, so no
consumer is told to install TS7.

The island-size totals cannot move: the SDK and runtime JavaScript are byte-identical and a
production `zfb build` emits no source maps (`module_worker.rs` sets `sourcemap: !production`).

## Published types still work for TS 7.0, 6.0 and 5.9 consumers

The baseline records no published consumer range beyond the `create-zfb` template's
`typescript ^5.6.0` and the requirement to keep TS 5.9 working. This stage keeps that range and does
not raise it: TS 5.9 (the template's resolution) is the floor, TS 6.0 is now probed too, and TS 7.0
is what contributors build with.

| Check | Result |
| --- | --- |
| `test:packed-types` (adapter, inside `test:workspace`) | `passed strict tsc 7.0.2`, `6.0.3`, `5.9.3` |
| `node packages/zfb/scripts/zudo-react-packed.mjs check` | build, pack, stage, packed exports, DOM-free server import PASS; production and development types PASS under 7.0.2, 6.0.3 and 5.9.3; esbuild probe skipped (no staged binary), as at the baseline |
| Ad hoc isolated consumer of the packed runtime + SDK (all 4 runtime entries, `skipLibCheck: false`, `types: []`) | exit 0 under 7.0.2, 6.0.3 and 5.9.3; 13 runtime declaration files in the program |
| md-wasm tarball-only consumer test (`package-browser.test.ts`) | now runs all three compilers; **not run locally** (needs the wasm build); CI's `wasm-md (default)` runs it |

## Every config loads the same program and diagnostics under 7.0.2

`tsc -p <config> --noEmit --listFiles` with 6.0.3 and 7.0.2 (lib files excluded, `.pnpm` paths
normalized):

| Config | Program files | Diagnostics |
| --- | --- | --- |
| zfb, zfb-runtime, adapter, adapter consumer | identical (142 / 163 / 106 / 106) | none |
| 3 SDK fixtures (zudo-react, dev, sdk) | identical (149 / 150 / 148) | none, so every `@ts-expect-error` still fires |
| md-wasm `tsconfig.json`, `test/tsconfig.consumer.json` | identical (162 / 143) | the same 4 / 5 TS2307 (wasm and `dist` not built locally) |
| `docs/tsconfig.json` | identical after `realpath` (387); 7.0.2 lists zudo-doc declarations by their `.pnpm` real path, 6.0.3 partly by the `docs/node_modules` symlink path | none |

**Exit code.** 7.0.2 exits **1** when it reports errors; 6.0.3 and 5.9.3 exit **2** ("errors,
outputs generated"). Every caller treats any nonzero status as failure: `zfb check` uses
`!status.success()`, the `package-tsc.mjs` callers throw on any nonzero status (a `spawnSync` status check or `execFileSync`), and package scripts chain
with `&&`. No code compares the value with 2.

### Real-tsc Rust fixtures (simulated without cargo)

Two `check_command.rs` tests put `packages/zfb/node_modules/.bin` (now 7.0.2) on `PATH` and run
`zfb check` with a real compiler. Running the same `tsc --noEmit` directly:

| Test | Expectation | 7.0.2 / 6.0.3 / 5.9.3 |
| --- | --- | --- |
| `check_passes_with_sdk_wasm_import_types` (`check-wasm-import`, `baseUrl` already dropped on this base) | exit 0 | exit 0 / 0 / 0 |
| `raw_text_children_are_rejected_by_zfb_check` (`raw-html-invalid-check`, compiled against the 7.0.2-built packed SDK instead of the binary's embedded copy) | nonzero, a diagnostic naming `pages/index.tsx` and `children` | exit 1 / 2 / 2, the same TS2747 at `pages/index.tsx(6,57)` |

The five remaining fixture tsconfigs with `baseUrl` (`bundle-repro`,
`client-bundling-cross-pipeline` and its `reroot-host`, `collection-seeds-3133/…/apps/site`,
`dev-sibling-watch/sub/host`) are used only by build/dev tests of ZFB's own alias resolution; none
reaches `zfb check` without `--skip-tsc` or any other real compiler. They keep `baseUrl`, per the
epic's "do not delete Rust/esbuild alias configuration" boundary.

## The create-zfb template keeps `baseUrl` and `typescript ^5.6.0`

`crates/zfb/templates/basic-blog/tsconfig.json` sets `baseUrl: "."` with `paths: { "~/*": ["./*"] }`,
and its `package.json` declares `typescript: ^5.6.0`.

| Compiler | Result on the template config |
| --- | --- |
| 5.9.3 (what `^5.6.0` resolves today) | no option diagnostic |
| 6.0.3 | TS5101 (`baseUrl` deprecated; `"ignoreDeprecations": "6.0"` silences it) |
| 7.0.2 | TS5102 (`baseUrl` removed) |

Decision: **unchanged.** The epic keeps public consumer TypeScript requirements unless a reviewed
public change is made, and the scaffold's users resolve 5.9.x, where the config is valid. Removing
`baseUrl` is also not compiler-only: `zfb-plugin-resolver` (`raw_alias.rs`, `base_url_candidate`)
reads it to resolve bare specifiers against the project root, so a scaffolded project would change
ZFB alias behavior, which needs Rust tests this host does not run. A scaffold owner who moves to
TypeScript 6 or 7 must drop `baseUrl` themselves; changing the template (and possibly its
`typescript` range) is a separate public-consumer decision for a follow-up.

## Workspace gates pass under TypeScript 7.0.2

| Gate | Result |
| --- | --- |
| `pnpm typecheck:workspace` | pass, 3.8 s wall (adapter + consumer, zfb, runtime) |
| `pnpm test:workspace` | pass, 38.2 s: 7 projects, **103 files / 1,645 cases**, the same as stage 4. Root 25 / 536, zfb 49 / 720, runtime 17 / 300, adapter 5 / 40, create-zfb 2 / 24, docs 5 / 25. No #3630 timeout flake this run |
| Compound steps inside it | zfb: `vp test run`, then the three fixture `tsc -p` compiles; adapter: `typecheck:consumer`, `vp test run`, `test:packed-types` (three compilers) |
| `scripts/__tests__/package-tsc.test.mjs` | the workspace-pin case now also resolves `typescript-6.0` and requires three distinct compilers |
| `pnpm format:check` | pass (Oxfmt 856 files; mdx-formatter all files) |
| `pnpm --filter docs check` | `✓ checked 2 collections and tsc — no errors`, tsc 7.0.2 |
| `node tests/zudo-react-browser/build-fixture.mjs` | pass: packed SDK staged, fixture compiled by `pnpm --filter @takazudo/zfb exec tsc` (7.0.2), every scenario page generated. The Playwright half was not run |

## Same-host measurement: about 3.4x to 5.1x faster, with less memory

`bench.mjs` (scratch) launched each compiler's `bin/tsc` directly with `node`, alternating the
order every pair (6.0.3 first, then 7.0.2 first), 8 pairs per task with the first pair discarded as
the cold-ish warm-up, so medians are over 7 warm runs. Both compilers were installed side by side,
so nothing was reinstalled between runs. Load average 3.8 / 4.8 / 7.9 (min / median / max).

| Command scope | 6.0.3 median | 7.0.2 median | Speedup |
| --- | ---: | ---: | ---: |
| typecheck: zfb, runtime, adapter, adapter consumer (`--noEmit`) | 3.04 s | 0.72 s | 4.2x |
| the three SDK fixture configs | 4.06 s | 0.89 s | 4.6x |
| emit build: zfb, runtime, adapter (`tsc` into a scratch `outDir`) | 3.26 s | 0.64 s | 5.1x |
| docs program (`--noEmit`) | 1.60 s | 0.47 s | 3.4x |

Peak RSS (`/usr/bin/time -l`; the TS7 launcher `execve`s into the native binary, so the number is
the compiler's own): SDK fixture 343 to 357 MB (6.0.3) vs 183 to 186 MB (7.0.2); docs 363 MB vs
234 MB. These are this host's numbers for these scopes, not a claim about the Rust `zfb build`.

`pnpm -r` keeps its default workspace concurrency, and no `--checkers`, `--builders` or
`--singleThreaded` flag was added: TS7's defaults use less memory than TS6 here, so nothing
justifies tuning.

## Contributor documentation and editors

- `CONTRIBUTING.md` names 7.0.2 as the pinned compiler, states which commands run it, lists the
  `typescript-6.0` and `typescript-5.9` probe aliases, and adds an **Editors** note: no editor
  settings are committed; TS7 ships no `lib/tsserver.js`, so VS Code's "Use Workspace Version"
  cannot select it; an editor's bundled TypeScript still works; the authoritative result is the
  package's `typecheck` script; an editor that accepts a custom language server can use
  `<package>/node_modules/.bin/tsc --lsp --stdio` (verified: the shim reaches the native binary's
  JSON-RPC server).
- `DEPENDENCIES.md` keeps both aliases with their reason.
- `research/v3-island-size/README.md` records the lockfile change and why totals are unchanged.

## CI must prove

- `health`: `typecheck:workspace` and `test:workspace` on Linux x64 with the
  `@typescript/typescript-linux-x64` native binary (Node 22.23.3, so the `execve` path), including
  the adapter's three-compiler packed check; the island-size step with the new `pnpmLockSha256` and
  unchanged Linux totals (a moved total would be a finding, not an expected rebaseline).
- `wasm-md (default)`: md-wasm `scripts/build.mjs` with tsc 7.0.2, `typecheck:consumer`, and the
  tarball consumer under 7.0.2, 6.0.3 and 5.9.3.
- `Docs gate` / `Build docs site` and `PR Preview`: the strict docs build, `zfb check` with 7.0.2,
  `check:html`, `check:islands`.
- `zudo-react-browser` (lockfile filter): the fixture compiled by tsc 7.0.2 against the staged
  packed SDK, then the Playwright suite.
- `router-chromium`, `wind-computed-style`, Scaffold E2E and the local smokes: the packed SDK/runtime
  built by 7.0.2.
- The Rust `check_command.rs` real-tsc tests (inside `cargo nextest`) with tsc 7.0.2 on `PATH`.

## Regressions versus stage 4: none found

No check that passed before fails now. Intended or explained differences: the compiler pin and
probe aliases, source-map column segments, one equivalent internal declaration, sorted
`jsx-runtime` import specifiers in JSX emit, and the 1-versus-2 error exit code.

#3480 (dangling `bundle-runtime.mjs.map` reference in the emitted Worker) is closed; the adapter's
tsc emit and its two copied `.mjs` helpers are byte-identical under 7.0.2, so this stage does not
touch that path.

## Not run here

Cargo builds and tests (the two real-tsc Rust tests were simulated with the compilers directly),
the md-wasm wasm build and its package/browser tests, the docs strict build, every Playwright lane,
and Windows (no lane). The md-wasm and docs builds use the same tsc call sites whose emit and
programs are compared above.
