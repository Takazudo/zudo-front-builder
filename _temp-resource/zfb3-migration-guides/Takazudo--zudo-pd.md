# zfb v3 migration guide: Takazudo/zudo-pd

Generated 2026-10-04 by an automated diagnosis of `main` @ `65031f9`; reviewed the same day against the clone, the zfb docs, npm and the GitHub REST API. Counts come from the commands listed; re-run them locally before relying on them.

> **Release note (2026-10-04, review):** zfb **3.2.0** was published to npm `latest` on 2026-10-04 (git tag `v3.2.0`, https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/). Everything marked **3.2.0** below was "next zfb release" when the diagnosis ran; it is released now. The `zfb wind audit` census was run on the 3.1.0 CLI. zudo-doc 6.0.0 is still unreleased (npm `latest` 5.28.2). `@takazudo/zudo-circuit-doc` **0.2.0** is on npm (2026-10-02) but still peers on `preact ^10.29.1`, `@takazudo/zfb ^2.20.2` and `@takazudo/zudo-doc ^5.27.0` — a 2.x release, not a zfb 3 one.

## Verdict

**Blocked**, twice. Effort **M**. The only zfb project is `doc/`, a `create-zudo-circuit-doc@0.1.0` host layered on the `create-zudo-doc@5.27.0` scaffold (zfb 2.21.0, zudo-doc 5.27.0, Cloudflare adapter). Host-owned rendering code is tiny (a 23-line `chrome-bindings.tsx`, a 4-line island seed, a 62-line stylesheet, two scaffold route stubs), and `zfb wind audit` on 3.1.0 found only 2 class-position errors, both preset tokens (`text-fg`, `hover:text-accent`). What makes it M rather than S: (1) zudo-doc 6.0.0 has not shipped (zudolab/zudo-doc#4430 / #4477); (2) `@takazudo/zudo-circuit-doc` ships Preact components (`lib/ui/component-references.js` imports `preact/jsx-runtime`) and Preact islands (`@takazudo/zudo-circuit-doc/islands`: footprint preview, package model viewer) that zudo-react cannot render; its newest release 0.2.0 (2026-10-02) still peers on Preact and zfb ^2.20.2, and none of its 30 most recently updated issues mentions zfb 3 or zudo-react; (3) both packages are consumed through pnpm patches pinned to exact versions (`patches/@takazudo__zudo-circuit-doc@0.1.0.patch`, 102 KB; `patches/@takazudo__zudo-doc@5.27.0.patch`) whose removal is already tracked (#210, #211) — the circuit-doc one can probably retire now, because 0.2.0 came out of the "Sweep 261002" super-epic (Takazudo/zudo-circuit-doc#111, PR #150 merged 2026-10-02) that bundles the upstream fixes #102–#109 the patch bridges; (4) `circuit/tests/*.mjs` SSR the package's Preact internals with `preact-render-to-string` and stubbed `preact/hooks`, and `circuit/scripts/check-zfb-link-warnings.sh` fails CI on any `zfb warn:` line shape it does not know. Start gate: a zudo-doc 6.0.0 **and** a zudo-circuit-doc release built for zfb 3; nothing in this repo can build on 3.x before both exist.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `doc/` (`zudo-pd`, the docs site; `doc/zfb.config.ts`) | `@takazudo/zfb` 2.21.0, `zfb-runtime` 2.21.0, `zfb-adapter-cloudflare` 2.21.0, `zfb-md-wasm` 2.21.0 (exact) | `@takazudo/zudo-doc` 5.27.0 (patched), `zudo-doc-history-server` 5.27.0, `@takazudo/zudo-circuit-doc` ^0.1.0 (patched; a zudo-doc-dependent preset/runtime, not zudo-sg), `@takazudo/zdtp` 0.7.0 declared but `designTokenPanel` is not set anywhere in `doc/zfb.config.ts` (grep: 0) | scaffold `src/styles/global.css`: `@import "tailwindcss/preflight"` (8), `@import "tailwindcss/utilities"` (9), 3 `@source` (22–24); devDeps `@tailwindcss/vite`, `tailwindcss` ^4.2.0 (unused by zfb) | `preact` ^10.29.1, `preact-render-to-string` ^6.6.6, `@types/react` ^19.2.0; pragmas `/** @jsxImportSource preact */` in `src/chrome-bindings.tsx:2` and `pages/docs/[[...slug]].tsx:2`; `import type { JSX } from "preact"` (`[[...slug]].tsx:6`) | 0 host islands; zudo-doc chrome islands + zudo-circuit-doc islands seeded by `pages/lib/_circuit-doc-islands.ts` (`import "@takazudo/zudo-circuit-doc/islands"`) | not called by host code | Cloudflare Workers via adapter: `doc/wrangler.toml` `main = "./dist/_worker.js"`, `nodejs_compat`, `[[env.production.routes]] pd.takazudomodular.com`; `.github/workflows/main-deploy.yml` (`npx wrangler@4 deploy --env production`), `pr-checks.yml` (preview alias), `component-spec-skills.yml` |
| `/` (`zudo-pd-workspace`, root) | `@takazudo/zfb` 2.21.0 as a **devDependency** only (for `circuit/tests`; `heading-parity.test.mjs:12` loads `@takazudo/zfb-md-wasm/render` from `doc/node_modules`) | `@takazudo/zudo-doc` 5.27.0 (devDependency, for the patch and tests), `@takazudo/zudo-circuit-doc` ^0.1.0 | none | `preact`, `preact-render-to-string` devDependencies used by `circuit/tests/compatibility.test.mjs:19`, `model-layout.test.mjs:9`, `footprint-dialog.test.mjs:13` to bundle and SSR package components with esbuild | — | via tests only | no site; `pnpm build` delegates to `doc/` through `circuit/scripts/build-doc.sh` |

Host-owned files that touch the engines (content is 134 files: 70 hand-authored + 64 generated under `doc/src/content/docs/components/`, 0 `class=` attributes):

- `doc/zfb.config.ts` (67 lines): `defineConfig(zudoDoc({ themePack: "sumi", docHistory: true, chromeBindingsModule: "./src/chrome-bindings.tsx", adapter: "@takazudo/zfb-adapter-cloudflare", assetViewerExclude: ["component-previews/**"], strictContentBridge: true, … }))`. No `framework`/`tailwind` keys of its own.
- `doc/src/chrome-bindings.tsx` (23 lines): pragmas (1–2); `homeExtras: () => <a href="https://x.com/Takazudo" class="text-fg underline hover:text-accent" …>` (12–21; the `class` attribute is line 15, already HTML spelling); `mdxExtras: { ...circuitDocMdxExtras }` from `@takazudo/zudo-circuit-doc/mdx-extras` (5, 22).
- `doc/pages/docs/[[...slug]].tsx` (53 lines, scaffold-shaped): pragmas (1–2), `JSX` type from `preact` (6), explicit `DocHistory` binding (import 13, `defineChromeBindings({ DocHistory })` 27), `import "../lib/_circuit-doc-islands.ts"` (20). `doc/pages/index.tsx`: 1-line re-export of `@takazudo/zudo-doc/routes/index`.
- `doc/src/styles/global.css` (62 lines): Tailwind imports (8–9), zudo-doc package CSS imports (13–17), `@import "@takazudo/zudo-circuit-doc/styles.css"` (18, a package `exports` subpath), `@source` ×3 (22–24), authored GFM task-list workaround (59–62, zudolab/zudo-doc#3388).
- `doc/tsconfig.json`: extends `@takazudo/zudo-doc/tsconfig.base.json`, includes `src`, `pages`, `scripts`.
- `pnpm-workspace.yaml`: `patchedDependencies` for `@takazudo/zudo-circuit-doc@0.1.0` and `@takazudo/zudo-doc@5.27.0`; `minimumReleaseAgeExclude: hono@4.13.10`.
- `ZUDO_DEPS_PINS.md`: zfb family 2.21.0, zudo-doc 5.27.0, circuit-doc 0.1.0 + source commit 5d0e2b6, the two patches with their upstream trackers (#103–#108 on Takazudo/zudo-circuit-doc, zudolab/zudo-doc#4428) and removal trackers (#210, #211).
- MDX components in hand-authored content: `<Note>` 93, `<Warning>` 42, `<Details>` 12, `<CategoryNav>` 11, `<Info>` 9, `<Danger>` 8, `<Tip>` 7, `<Caution>` 3 (zudo-doc), `<LCSC>` 3, `<EASYEDA>` 1 (circuit-doc extras). Generated pages use `<EvidenceAnchor>` 1332, `<EvidenceFact>` 622, `<EvidenceTable>` 66, `<EvidenceDetails>` 60, `<ComponentReferences>` 60 (circuit-doc; regenerated by `zudo-circuit-doc generate`, never hand-edited).

Audit run (`zfb` 3.1.0, `--project-root doc`, temporary `wind: { spec: 1 }`, config restored, `git status --short` clean): `outcome: complete`, exit 0 (`--fail-on error` would fail), **2 error-severity**: `ZW006 … src/chrome-bindings.tsx: unknown value or token fg` (`text-fg`) and `… token accent` (`hover:text-accent`); dead classes: those two; unrecognized: none; 15 `auditInfo` (ZW005 ×10 slash modifiers in URL strings, ZW002 ×3, ZW012 ×1 — the `docs;${locale}` route signature, ZW001 ×1); re-run during review with identical results. Both tokens belong to zudo-doc's theme and will be supplied by the 6.0.0 preset; nothing host-side needs an `authoredClasses` entry. The standalone audit plan does not see zudo-doc's or zudo-circuit-doc's package-owned routes, islands or CSS (https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/#the-source-plan). `zfb check` was not run (preset imports need `node_modules`).

What the `@takazudo/zudo-circuit-doc@0.1.0` patch shows about the package (read from `patches/@takazudo__zudo-circuit-doc@0.1.0.patch`, 38 shipped files touched): `lib/ui/component-references.js` is compiled Preact JSX (`import { jsx as _jsx, jsxs as _jsxs } from "preact/jsx-runtime"` + `/** @jsxImportSource preact */`, patch lines 1218–1220) using `className` props and BEM class names (`zcd-component-references__document-label` …); `styles.css` is authored CSS (patched for a 375 px layout fix); islands `FootprintPreview`/`PackageModelViewer` (WRL models, `three` 0.185.1 + `@types/three` are declared in `doc/package.json` for them; host code never imports `three`). A zfb 3 host cannot render these: components rendered by zudo-react must not ship Preact JSX runtime imports (https://zfb.takazudomodular.com/guides/migrating-to-v3/#packages-that-ship-classes-or-components). The published 0.2.0 (`npm view @takazudo/zudo-circuit-doc@0.2.0`) keeps the same surface — exports `.`, `./ui`, `./config`, `./islands`, `./mdx-extras`, `./styles.css`, `./descriptors`, no `./wind.json` — and the same Preact/zfb-2 peers, so it is a drop-in for the 2.x host, not a v3 step.

File-by-file change list (everything else — KiCad sources, `boards/`, `footprints/`, `scripts/schgen`, `.claude/skills`, generated `components/` pages — is untouched by the engine change):

| File | Lines | Change | Gate |
| --- | --- | --- | --- |
| `doc/zfb.config.ts` | 67 | none expected; re-check option names (`themePack`, `assetViewerExclude`, `chromeBindingsModule`, `adapter`) against the 6.0.0 consumer guide | zudo-doc 6.0.0 |
| `doc/src/chrome-bindings.tsx` | 23 | delete pragmas (1–2); `mdxExtras` only valid once circuit-doc's extras are zudo-react | both releases |
| `doc/pages/docs/[[...slug]].tsx` | 53 | delete pragmas (1–2) + `preact` type (6), or take the scaffold stub and re-apply the `DocHistory` binding and the island seed import | zudo-doc 6.0.0 |
| `doc/pages/index.tsx` | 7 | none | — |
| `doc/pages/lib/_circuit-doc-islands.ts` | 4 | none; the islands it registers must be zudo-react | circuit-doc |
| `doc/src/styles/global.css` | 62 | scaffold part replaced (5 ZW009 sites: 8, 9, 22, 23, 24); keep line 18 and 59–62 | zudo-doc 6.0.0 |
| `doc/tsconfig.json` | 9 | none (base from zudo-doc 6) | zudo-doc 6.0.0 |
| `doc/package.json` | 55 | 4 zfb pins, 2 zudo-doc pins, circuit-doc; remove `preact`, `preact-render-to-string`, `@types/react`, `@tailwindcss/vite`, `tailwindcss`; decide `@takazudo/zdtp` | both releases |
| `package.json` (root) | 41 | pins; drop `preact*` after the test-renderer swap | both releases |
| `pnpm-lock.yaml` (one root lockfile for `.` + `doc`) | — | regenerates with the bumps; the `patchedDependencies` hashes leave it | both releases |
| `pnpm-workspace.yaml` | 13 | drop both `patchedDependencies` (the circuit-doc one possibly now, see Sequencing 3) | both releases |
| `patches/@takazudo__zudo-circuit-doc@0.1.0.patch`, `patches/@takazudo__zudo-doc@5.27.0.patch`, `patches/README.md` | 102 KB + 4 KB | retire or re-derive (#210, #211) | both releases |
| `circuit/tests/{compatibility,model-layout,footprint-dialog}.test.mjs` | — | SSR renderer → `@takazudo/zfb/zudo-react/server`, drop `react → preact/compat` aliases | circuit-doc |
| `circuit/tests/heading-parity.test.mjs` | — | verify `renderHtml()` options; re-baseline `heading-built-baseline.json` if the zudo-doc patch is gone | zudo-doc 6.0.0 |
| `circuit/scripts/check-zfb-link-warnings.sh` | 172 | extend the known-false `zfb warn:` shapes deliberately | first 3.x build log |
| `.github/workflows/{main-deploy,pr-checks,component-spec-skills}.yml`, `circuit/scripts/run-b4push.sh` | — | add `zfb wind audit --fail-on error` | after cutover |
| `ZUDO_DEPS_PINS.md`, `doc/SCAFFOLD.md`, `doc/CLAUDE.md`, `patches/README.md` | — | rewrite pin rows and the "Tailwind and Preact" wording | after cutover |

## Sequencing and blockers

1. **zudo-doc 6.0.0 on zfb 3** (epic zudolab/zudo-doc#4430 open; draft root PR #4477; consumer guide #4473 open). Its zfb-side blockers #3569/#3570 shipped in **zfb 3.2.0 (2026-10-04)**, so only zudo-doc's own release is outstanding. Planned shape per zudolab/zudo-doc `base/zfb3-migration:_temp-resource/4430-zfb3-migration/explore/pkg-build.md` (verify against the release): `zudoDoc()` drops `framework`/`tailwind`, emits `wind` with its tokens, reset and `manifests: { "zudo-doc": { path: "@takazudo/zudo-doc/wind.json" } }`, offers an optional host `wind` override, flips `tsconfig.base.json` to `jsxImportSource: "@takazudo/zfb/zudo-react"`, marks `./safelist.css` as a removal candidate, re-scaffolds `pages/` and `global.css`, and keeps zdtp as an opaque lazily-loaded Preact bundle.
2. **A zudo-circuit-doc release for zfb 3** (Takazudo/zudo-circuit-doc; read via the GitHub REST API during review: tags `v0.1.0`, `v0.2.0`, branches `main` + one `claude/*`; no issue among the 30 most recently updated mentions zfb 3 or zudo-react): port `lib/ui/*` and `lib/islands/*` to `@takazudo/zfb/zudo-react` JSX (`class`, `on:*`, signals; `<iframe>`/`three` canvases as childless shells or third-party mounts), republish `styles.css` wind-clean, and publish a candidate manifest with `zfb wind manifest --producer zudo-circuit-doc` (3.2.0, https://zfb.takazudomodular.com/api/cli/#zfb-wind-manifest) if its markup uses utilities. Its BEM classes contain `__`, which was ZW001 on 3.1.0 and is ordinary since 3.2.0 (#3365); a class in shipped HTML is not scanned by wind, but any `zcd-*__*` name that reaches a host `class` attribute would be. Its `mdx-extras` must be zudo-react components too.
   What that release has to provide, per https://zfb.takazudomodular.com/guides/migrating-to-v3/#packages-that-ship-classes-or-components and https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/#publishing-candidates-from-a-package:
   - every component zudo-react renders (`lib/ui/*`, `lib/islands/*`, `/mdx-extras`) imports from `@takazudo/zfb/zudo-react` with the same JSX import source; no `preact/jsx-runtime` or `react/jsx-runtime` imports in those files;
   - if its markup uses utilities, a strict candidate manifest generated with `zfb wind manifest --producer zudo-circuit-doc --output dist/wind.json --config <its wind config>` and exported as `./wind.json`, plus the token list the host must configure (or a preset that ships them); the host declares it under `wind.manifests["zudo-circuit-doc"]`;
   - `styles.css` stays authored CSS with no Tailwind directives (the host imports it, so ZW009 applies to it too);
   - the WRL viewer and footprint dialog either ported to zudo-react (`three` driven from `getScope().onActivate()`, `Ref` objects for the canvas) or kept as self-mounting widgets inside an empty host element (https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget); `<iframe>` inside an island is **next release** (#3361);
   - one runtime copy: the package must resolve `@takazudo/zfb/zudo-react` to the host's copy (peer dependency on `@takazudo/zfb`), otherwise signals shared across islands break (zfb#3331 context).
3. **Both patches retire or move**: `patches/@takazudo__zudo-doc@5.27.0.patch` (heading extraction, upstream zudolab/zudo-doc#4428, tracker #211) must be re-evaluated against 6.0.0's `dist/extract-headings`; `patches/@takazudo__zudo-circuit-doc@0.1.0.patch` (#210) cannot apply to a different version. The `pnpm-workspace.yaml` `patchedDependencies` keys pin exact versions, so the bump itself drops them.
4. **Now (no upstream dependency):** nothing in `doc/` can build on 3.x, but the engine-neutral prep below (CI warning gate, test harness decoupling, dependency cleanup) can land on `main`.
5. **Next zfb release items that matter here:** `#3364`-class authored CSS `@import` of package `exports` subpaths — `doc/src/styles/global.css:18` imports `@takazudo/zudo-circuit-doc/styles.css`; the 3.1.0 changelog lists "Resolve authored CSS package subpaths through package `exports`" (https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/), so verify it on the target binary rather than assuming; `#3365` underscore class names (circuit-doc BEM); `#3370` `file:line:col` diagnostics; `#3480` Cloudflare adapter dangling source-map references (this site deploys `dist/_worker.js`).

## Required changes

### 1. Dependencies, config, tsconfig, env

- `doc/package.json`: bump `@takazudo/zfb`, `zfb-runtime`, `zfb-adapter-cloudflare`, `zfb-md-wasm` to the zfb line zudo-doc 6.0.0 requires (keep the four identical; `ZUDO_DEPS_PINS.md` already mandates "align family together"), `@takazudo/zudo-doc` + `zudo-doc-history-server` to 6.0.0, `@takazudo/zudo-circuit-doc` to its zfb-3 release; remove `preact`, `preact-render-to-string`, `@types/react`, `@tailwindcss/vite`, `tailwindcss` unless the 6.0.0 guide keeps `preact` as zdtp's install-time peer. Decide `@takazudo/zdtp` 0.7.0: no `designTokenPanel` setting exists in `doc/zfb.config.ts`, so it looks removable today (verify no `pack.css`/theme-pack path needs it). Why: https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands.
- Root `package.json`: `@takazudo/zfb`, `@takazudo/zudo-doc`, `preact`, `preact-render-to-string` exist for `circuit/tests`; after the package SSR tests are rewritten (section 5) drop `preact*` and keep only what `circuit/tests/heading-parity.test.mjs` and `compatibility-types.ts` import.
- `pnpm-workspace.yaml`: replace or delete both `patchedDependencies` entries; keep `allowBuilds` (`esbuild`, `lefthook`, `sharp`, `workerd`).
  Target shape of `pnpm-workspace.yaml` (the exact-version `patchedDependencies` keys stop matching at the bump, so a stale entry would fail `pnpm install`):

```yaml
packages: [ "doc" ]
allowBuilds:
  esbuild: true
  lefthook: true
  sharp: true
  workerd: true
# patchedDependencies removed with the zfb 3 cutover:
#   @takazudo/zudo-circuit-doc@0.1.0 (Takazudo/zudo-pd#210), @takazudo/zudo-doc@5.27.0 (#211)
```

- `doc/zfb.config.ts`: no `framework`/`tailwind` to delete. If a host `wind` override exists in 6.0.0 it is not needed: the audit shows 0 host-owned utility tokens. `adapter`, `themePack: "sumi"`, `assetViewerExclude`, `strictContentBridge` and `headerNav` stay unless the consumer guide renames them. Why: https://zfb.takazudomodular.com/zudo-wind/configuration/.
- `doc/tsconfig.json`: expect `@takazudo/zudo-doc/tsconfig.base.json` 6.0.0 to carry `"jsx": "react-jsx"`, `"jsxImportSource": "@takazudo/zfb/zudo-react"`; this file adds only `include`.
- `.env.example` has Netlify leftovers only; no `ZFB_TAILWIND*` anywhere (grep: 0). Why: https://zfb.takazudomodular.com/guides/migrating-to-v3/#cli-and-environment.
- `ZUDO_DEPS_PINS.md` and `doc/SCAFFOLD.md`: rewrite the pin rows (zfb family, zudo-doc, circuit-doc, both patches) when the bump lands; `doc/CLAUDE.md:3` ("zfb, MDX, Tailwind and Preact") and `circuit/README.md`/`WORKFLOW.md` wording.

### 2. CSS and utilities

- `doc/src/styles/global.css` lines 8, 9, 22, 23, 24 are ZW009 errors on zfb 3, even under `wind: false` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives). Take the `create-zudo-doc@6` (or `create-zudo-circuit-doc` next) template version of this file and re-add only the two host-authored parts: the `@import "@takazudo/zudo-circuit-doc/styles.css"` line and the task-list workaround (`.zd-content li:has(> p > input[type="checkbox"][disabled]) { list-style-type: none }`, lines 59–62), which is plain CSS and stays valid.
  Expected shape of the host-authored remainder (the import list is whatever the 6.0.0 scaffold emits; zudo-doc's planning notes mark `safelist.css` — a Tailwind `@source inline()` file — as a likely removal because wind rejects it with ZW009):

```css
/* lines emitted by create-zudo-doc@6 go here: @layer order + @takazudo/zudo-doc/*.css imports */
@import "@takazudo/zudo-circuit-doc/styles.css"; /* package `exports` subpath, authored CSS */

/* host-authored, unchanged: GFM task-list marker reset (zudolab/zudo-doc#3388) */
.zd-content li:has(> p > input[type="checkbox"][disabled]),
.zd-content li:has(> input[type="checkbox"][disabled]) {
  list-style-type: none;
}
```

- Host tokens: none to declare. `text-fg`/`hover:text-accent` in `chrome-bindings.tsx:14` resolve when the zudo-doc 6 preset supplies `colors.fg`/`colors.accent` (ZW006 today only because the temporary audit config had no preset). If 6.0.0 renames its palette, update that one `class` string.
- Reset and cascade are the preset's call (`wind.reset`, utility placement). The host adds no unlayered rules that compete with utilities (the task-list rule targets `li` inside `.zd-content`), so the `after-authored` tie flip (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties) has no host-side impact; zudo-circuit-doc's `styles.css` (BEM selectors, no utilities seen in the patch) is unaffected unless it mixes utilities into its markup.
- `doc/public/circuits/*.svg` (schemdraw) and `doc/public/assets/component-previews/**` (69 files) are static; no CSS change.

### 3. Components and islands

- `doc/src/chrome-bindings.tsx`: delete lines 1–2 (`@jsxRuntime automatic`, `@jsxImportSource preact`). zfb 3 prints a `zfb warn:` line per such pragma and then fails (`Could not resolve "preact/jsx-runtime"`, or `ZR_CHILD` while Preact is still installed as zdtp's peer) — https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands. The `homeExtras` anchor already uses `class`, `href`, `target`, `rel`; no listener; no change beyond the pragma. `mdxExtras: { ...circuitDocMdxExtras }` is only valid once the circuit-doc extras are zudo-react components.
- `doc/pages/docs/[[...slug]].tsx`: delete the pragmas (1–2) and `import type { JSX } from "preact"` (6) — or replace the whole stub with the 6.0.0 scaffold version and re-apply the two host edits: the explicit `DocHistory` binding (`defineChromeBindings({ DocHistory })`, lines 13–27) and `import "../lib/_circuit-doc-islands.ts"` (20). Whether 6.0.0 still needs the explicit DocHistory merge is a consumer-guide item (zudolab/zudo-doc#4473).
- `doc/pages/lib/_circuit-doc-islands.ts`: keep as the side-effect seed; the islands it registers must be zudo-react (`"use client"`, one component per `<Island>`, JSON props, fail-closed hydration — https://zfb.takazudomodular.com/concepts/islands/). The footprint dialog and WRL model viewer are the package's to port; if the viewer keeps a foreign renderer, the supported pattern is a self-mounting widget inside a zudo-react island (https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget); `<iframe>` inside islands is **next release** (#3361).
- No host forms, `dangerouslySetInnerHTML`, `className` or React-style handlers (grep: 0 outside `circuit/tests`).

### 4. md-wasm and other packages

- `circuit/tests/heading-parity.test.mjs:12` imports `renderHtml` from `@takazudo/zfb-md-wasm/render` to compare the installed zudo-doc heading extractor against zfb's native Markdown pipeline. `renderHtml()` survives in 3.x but no longer accepts `jsxRuntime` (not passed here; verify the options object) — https://zfb.takazudomodular.com/guides/migrating-to-v3/#mdx-and-md-wasm. Its baseline `circuit/tests/heading-built-baseline.json` (101 IDs) was captured with the patched 5.27.0 extractor; re-baseline when the zudo-doc patch is dropped.
- `@takazudo/zudo-circuit-doc` is both a Python/Node generator (unaffected: `zudo-circuit-doc generate|validate|models|footprints|scan`, `circuit.config.ts`, `circuit/publication/*`) and a zfb preset/runtime (affected: `/islands`, `/mdx-extras`, `/styles.css`, `lib/ui/*`). Only the second half blocks.
- `@takazudo/zfb-adapter-cloudflare`: `dist/_worker.js` + `_zfb_inner.mjs` and the `.assetsignore` written by `main-deploy.yml:111` / `pr-checks.yml` stay; the source-map fix (#3480) is **next release**.

### 5. Tests, CI, deploy

- `circuit/scripts/check-zfb-link-warnings.sh` (gate in `check:site`, run by `main-deploy.yml:82` and `pr-checks.yml`): it suppresses exactly three `zfb warn:` shapes and **fails on any other**. zfb 3 adds at least the per-file `@jsxImportSource` pragma warning (gone once pragmas are removed) and wind diagnostics printed as `source:line:column: CODE candidate: message` (https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/#reading-diagnostic-output). Run a 3.x build log through the script before the cutover PR and decide per shape whether to allowlist (ZW014 warnings for circuit-doc markup) or fix. This is the most likely red check on day one.
- `circuit/tests/compatibility.test.mjs:19`, `model-layout.test.mjs:9`, `footprint-dialog.test.mjs:13` bundle `lib/ui/component-references.js` and `lib/islands/footprint-preview-island.js` with esbuild aliases `react → preact/compat` and render them with `preact-render-to-string`. After the package moves to zudo-react these become `renderToString` from `@takazudo/zfb/zudo-react/server` (and `createIslandTest` for the island) — https://zfb.takazudomodular.com/zudo-react/testing/. `circuit/tests/runtime-semantic-baseline.json`, `runtime-anchor-baseline.json`, `compatibility-baseline.json` hash evidence identities, not markup, and should survive.
  Put the engine behind one helper now so the swap is a single edit later (today the three suites each inline `export {h} from "preact"; export {default as render} from "preact-render-to-string"` into an esbuild stdin bundle):

```js
// circuit/tests/render-helper.mjs — the only file that names the SSR engine.
// 2.x today: preact-render-to-string + esbuild alias react -> preact/compat.
// 3.x: the owned server renderer (https://zfb.takazudomodular.com/zudo-react/testing/).
import { renderToString } from "@takazudo/zfb/zudo-react/server";
import { h } from "@takazudo/zfb/zudo-react";
export { h, renderToString as render };
```

- `circuit/scripts/model-viewer-browser-smoke.mjs` / `pnpm test:model-viewer:browser` and the 12-representative browser contract (`doc/SCAFFOLD.md`) are black-box; keep them as the parity oracle.
- Workflows (`component-spec-skills.yml`, `main-deploy.yml`, `pr-checks.yml`): Node 22, pnpm 11.5.2, Python 3.12, SHA-pinned actions; no engine env. Add `pnpm --dir doc exec zfb wind audit --fail-on error` after `pnpm check` once on 3.x (https://zfb.takazudomodular.com/api/cli/#zfb-wind). `npx wrangler@4 deploy --env production` is unaffected.
- `lefthook.yml` pre-push runs `pnpm b4push` on `main` (circuit checks, tests, `pnpm check`, `pnpm build`, `check:site`, `circuit:check-generated`); `pnpm check` includes `zfb check` through `circuit/scripts/doc-command.sh`.
- `doc/.htmlvalidate.json` (`element-permitted-content: error`) over `dist/**/*.html` remains a useful L3 gate for the zudo-react markup.

## Step-by-step plan

Adapted from https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist; steps 1–2 are prep, 3+ wait for both upstream releases.

1. **Prep (now):** open a tracking issue "zfb 3 cutover" linking #210, #211, zudolab/zudo-doc#4430/#4473 and the zudo-circuit-doc port; capture a 2.21.0 build log (`pnpm build` with `ZUDO_DOC_BUILD_LOG`) and `dist/` for later diffing; run `node_modules/.bin/zfb wind audit --project-root doc` on a 3.x binary against a scratch copy with `wind: { spec: 1 }` to re-confirm the 2-error census after content changes.
2. **Prep (now):** make `circuit/tests/compatibility.test.mjs`, `model-layout.test.mjs`, `footprint-dialog.test.mjs` import their renderer through one helper so the Preact → zudo-react swap is a single edit; remove `@takazudo/zdtp` from `doc/package.json` if confirmed unused.
3. **Upgrade the presets first, then install matching v3 packages**: when both releases exist, in one commit edit `pnpm-workspace.yaml` (drop `patchedDependencies`), `doc/package.json` (zudo-doc 6.0.0, history-server 6.0.0, circuit-doc vNext, four zfb pins to the required 3.x), root `package.json` (same zudo-doc/circuit-doc/zfb), `pnpm install --frozen-lockfile=false`, `pnpm --dir doc exec zfb --version`.
4. **Config:** `pnpm --dir doc exec zfb check`; the preset no longer emits `framework`/`tailwind`, and `doc/zfb.config.ts` has none, so the only expected failures are TypeScript errors in the two pragma files. Fix them (step 6) before continuing.
5. **Stylesheet:** replace `doc/src/styles/global.css` with the new scaffold file + the circuit-doc import + the task-list rule; `pnpm --dir doc exec zfb css --input src/styles/global.css --output /tmp/zudo-pd.css` → no ZW009; `pnpm --dir doc exec zfb wind audit --fail-on error` → 0 errors (`text-fg`, `hover:text-accent` now resolve through the preset).
6. **Components:** delete the pragmas and the `preact` type import in `doc/src/chrome-bindings.tsx` and `doc/pages/docs/[[...slug]].tsx` (or take the scaffold stub and re-apply DocHistory + island seed); `grep -rln "@jsxImportSource\|from \"preact\"" doc/src doc/pages` → 0; `pnpm --dir doc exec zfb check`.
7. **md-wasm:** re-run `pnpm test:circuit` (`heading-parity.test.mjs`); re-baseline `heading-built-baseline.json` only if the zudo-doc patch's fixes shipped upstream (zudolab/zudo-doc#4428).
8. **Gate:** feed a 3.x build log to `bash circuit/scripts/check-zfb-link-warnings.sh <log>`; extend its known-false classes deliberately for any new informational shape; add `zfb wind audit --fail-on error` to `pr-checks.yml`, `main-deploy.yml` and `circuit/scripts/run-b4push.sh`.
9. **Build and compare:** `pnpm build` → `pnpm check:site` (built references, scan, strict links, fragment links, warning gate) → `pnpm circuit:check-generated` (64 pages byte-stable) → `pnpm test:compatibility` → `pnpm test:model-viewer:browser`; open `/docs/components/records/stusb4500qtr/`, `/docs/components/catalog/`, `/docs/architecture/`, the home hero (`@Takazudo` link), footprint dialog, WRL viewer, theme toggle, search, doc history at 1440/375 in light/dark. Update `ZUDO_DEPS_PINS.md`, `doc/SCAFFOLD.md`, `patches/README.md` (delete if empty), `doc/CLAUDE.md`; merge via the PR preview; watch `main-deploy.yml`.

## Verification checklist

- [ ] `pnpm --dir doc exec zfb --version` prints the 3.x release and `embedded esbuild` only.
- [ ] `pnpm-workspace.yaml` has no `patchedDependencies`; `patches/` is empty or documents a new, version-matched bridge with an upstream tracker.
- [ ] `pnpm check` (incl. `zfb check`) passes; no `zfb warn:` pragma lines in the build log; `check-zfb-link-warnings.sh` exits 0 with its allowlist unchanged or deliberately extended.
- [ ] `pnpm --dir doc exec zfb wind audit --fail-on error` exits 0; `text-fg` and `hover:text-accent` resolve (no ZW006).
- [ ] `grep -rnE '@import\s+"tailwindcss|@source|@theme' doc/src/styles` → 0; `grep -rln "@jsxImportSource\|from \"preact\"" doc/src doc/pages` → 0.
- [ ] `pnpm circuit:check-generated` reports the 64 generated pages unchanged (generator output does not depend on the renderer).
- [ ] `pnpm test:compatibility` + `pnpm test:circuit` pass with the renderer helper on `@takazudo/zfb/zudo-react/server`.
- [ ] Browser: footprint preview dialog opens/closes, WRL model viewer renders a package, `<EvidenceAnchor>` ids resolve (`#rec-…`), `<Details>`/`<Note>` admonitions render, home-hero `@Takazudo` link present, no `ZR_*` diagnostics in the console, `data-zfb-island-mounted` on every island.
- [ ] `html-validate "dist/**/*.html"` (`check:html`) passes `element-permitted-content`.
- [ ] `dist/_worker.js`, `dist/_zfb_inner.mjs` present; `.assetsignore` written; preview alias deploy from `pr-checks.yml` serves `/docs/components/records/c1623/`.

## Risks and open questions

- **Two upstream majors, one of them unscoped here.** zudo-doc 6.0.0 is tracked (39/51 topics merged locally, release blocked on the next zfb). zudo-circuit-doc has no known zfb-3 branch or issue in this diagnosis's scope; its port (Preact JSX in `lib/ui`, Preact islands with `three`, `styles.css`, `mdx-extras`) is the critical path and should get its own issue on Takazudo/zudo-circuit-doc.
- **Patch coupling.** Both pnpm patches encode behaviour the project's tests depend on (`compatibility.test.mjs` asserts the patched 0.1.0 runtime; `heading-parity` asserts the patched extractor). A version bump silently drops the patches; the tests then decide whether upstream absorbed the fixes (#103–#108, zudolab/zudo-doc#4428) or new bridges are needed.
- **Warning gate brittleness.** `check-zfb-link-warnings.sh` turns any unknown `zfb warn:` shape red by design. zfb 3's wind diagnostics and the ZW014 vocabulary (**next release**) will add shapes; decide the allowlist consciously rather than suppressing `zfb warn:` wholesale (the gate exists to catch hand-authored broken links among ~2,500 false generated-anchor warnings).
- **zdtp.** `@takazudo/zdtp` 0.7.0 is declared in `doc/package.json` with no `designTokenPanel` setting; if the panel is ever enabled on zudo-doc 6, zdtp stays a Preact bundle and `preact` returns as a peer (zudo-doc decision DD3; zdtp#1002).
- **Underscore class names.** circuit-doc's `zcd-…__…` BEM names are ZW001 on 3.1.0 if they ever reach a host `class` attribute (the audit scans host sources, not package HTML); fixed on the **next zfb release** (#3365).
- **Cloudflare adapter.** The site deploys `dist/_worker.js`; the dangling source-map fix (#3480) is **next release**. If the first 3.x deploy logs source-map warnings in wrangler, that is the cause.
- Open: does `heading-parity.test.mjs` pass any option object to `renderHtml()` that 3.x rejects (`jsxRuntime` is gone)? Does the 6.0.0 scaffold still require the explicit `DocHistory` chrome binding this host adds?

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/
- https://zfb.takazudomodular.com/zudo-wind/configuration/, https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/, https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- https://zfb.takazudomodular.com/zudo-react/components-and-jsx/, https://zfb.takazudomodular.com/zudo-react/testing/
- https://zfb.takazudomodular.com/concepts/islands/ (incl. "Embedding a third-party widget"), https://zfb.takazudomodular.com/api/island/, https://zfb.takazudomodular.com/api/cli/
- https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/, https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/
- zudo-doc 6.0.0: zudolab/zudo-doc#4430 (epic), #4477 (root PR), #4473 (consumer guide), #4428 (heading extraction, patched here), #3388 (task-list CSS workaround)
- zfb follow-ups: #3361 (iframe in islands), #3364 (package CSS subpaths), #3365 (underscore names / ZW014), #3370 (diagnostics), #3480 (adapter source maps), #3569/#3570 (zudo-doc integration blockers, fixed on main)
- Project trackers: Takazudo/zudo-pd#210 (circuit-doc bridge removal), #211 (zudo-doc patch removal); Takazudo/zudo-circuit-doc#103–#110; Takazudo/zudo-design-token-panel#1002
- Repo files: `doc/zfb.config.ts`, `doc/src/chrome-bindings.tsx`, `doc/pages/docs/[[...slug]].tsx`, `doc/pages/lib/_circuit-doc-islands.ts`, `doc/src/styles/global.css`, `doc/tsconfig.json`, `doc/package.json`, `package.json`, `pnpm-workspace.yaml`, `patches/*`, `ZUDO_DEPS_PINS.md`, `doc/SCAFFOLD.md`, `circuit.config.ts`, `circuit/scripts/check-zfb-link-warnings.sh`, `circuit/scripts/run-b4push.sh`, `circuit/tests/{compatibility,model-layout,footprint-dialog,heading-parity}.test.mjs`, `.github/workflows/{component-spec-skills,main-deploy,pr-checks}.yml`, `doc/wrangler.toml`, `lefthook.yml`
