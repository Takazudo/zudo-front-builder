# zfb v3 migration guide: Takazudo/zudo-case

Generated 2026-10-04 by an automated diagnosis of `main` @ `6a13d80`. Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**Blocked** on zudo-doc 6.0.0. Effort **S**: this repo is a pure zudo-doc 5.27.0 host whose only host-owned rendering code is one 19-line `h()`-based component file, a 6-line chrome-bindings module and a 28-line scaffold stylesheet; everything else that zfb 3 breaks (`framework`/`tailwind` keys, Tailwind imports, Preact route stubs, `tsconfig.base.json`) is emitted by the `zudoDoc()` preset or copied from the `create-zudo-doc@5.27.0` scaffold. The start gate is the zudo-doc 6.0.0 release (epic zudolab/zudo-doc#4430, root PR zudolab/zudo-doc#4477, consumer migration guide zudolab/zudo-doc#4473); a preset that still supplies `framework`/`tailwind` fails config loading even after the host removes its own copy, so the host cannot move first. The `zfb wind audit` run on 3.1.0 found **0 class-position errors** in host code. What can be done now: port `src/components/preview-links.jsx` to the zudo-react `h()` dialect, retire the per-run config mirror that works around the now-closed zfb#3318, and bump the scaffold-version constants in `scripts/setup.mjs` / `tests/setup.test.mjs` when 6.0.0 ships.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `/` (`zudo-case-doc`, root zfb project, `zfb.config.ts`) | `@takazudo/zfb` 2.20.2, `zfb-runtime` 2.20.2, `zfb-md-wasm` 2.20.2 (exact pins) | `@takazudo/zudo-doc` ^5.27.0, `@takazudo/zudo-doc-history-server` 5.27.0 (dev); no zudo-sg; no zdtp dep (`designTokenPanel` not set) | only via the scaffold `src/styles/global.css` (2 imports, 3 `@source`, 1 `@theme`); no `tailwind.config.*`, no `@tailwindcss/*` dep, no `ZFB_TAILWIND*` in scripts/CI | `preact` ^10.29.1, `preact-render-to-string` ^6.6.6; 1 host file imports `h` from `preact`; 1 scaffold route stub carries `/** @jsxImportSource preact */` | 0 host islands (`<Island`, `"use client"`: 0 hits); zudo-doc chrome islands come from the preset | not called directly (`compile(`/`renderHtml(`/`jsxRuntime`: 0 hits) | Cloudflare Workers Static Assets, no zfb adapter: `wrangler.docs.jsonc` (assets `./dist`, `zudo-case.zudolab.dev`) + `wrangler.preview.jsonc` (`main: ./src/worker.js`, assets `./dist-preview`, `zudo-case-preview.zudolab.dev`); `wrangler` 4.138.0; GitHub Actions `deploy.yml` on push to `main` |

Host-owned files that touch zfb/Preact/Tailwind (everything else under `src/content/docs/` is 41 MDX files with 0 `class=`/`style=` attributes):

- `zfb.config.ts` (48 lines): `defineConfig(zudoDoc({...}))`, `chromeBindingsModule: "./src/chrome-bindings.js"`, `docHistory: false`, `assetViewer: true`, `headerNav` ×9. No `framework`/`tailwind` keys of its own (the preset emits them).
- `src/chrome-bindings.js` (6 lines): `defineChromeBindings({ mdxExtras: { PreviewFrame, PreviewLink, PreviewSource } })`.
- `src/components/preview-links.jsx` (19 lines): `import { h } from 'preact'` (line 1); three `h()`-built components; iframe `style: { border: '1px solid #6b7280', borderRadius: '8px' }` (line 13, camelCase key), `loading: 'lazy'`, `height` passed as a number. Used by content: `<PreviewLink` ×48, `<PreviewFrame` ×2, `<PreviewSource` ×1 (`<CategoryNav` ×8 is zudo-doc's).
- `src/styles/global.css` (28 lines, scaffold copy): `@import "tailwindcss/preflight" layer(zd-preflight)` (8), `@import "tailwindcss/utilities"` (9), `@source` (20–22), `@theme {}` (26).
- `pages/index.tsx` (scaffold, 1-line re-export of `@takazudo/zudo-doc/routes/index`) and `pages/docs/[[...slug]].tsx` (scaffold, 62 lines): `/** @jsxRuntime automatic */` + `/** @jsxImportSource preact */` (lines 1–2), `import type { JSX } from "preact"` (27).
- `tsconfig.json` (scaffold): extends `@takazudo/zudo-doc/tsconfig.base.json`, maps `react`/`react/jsx-runtime`/`react-dom` to `preact/compat` (lines 7–9).
- `scripts/run-site.mjs` (94 lines) + `scripts/lib/preview-origin.mjs` (36 lines): every `dev`/`build`/`check`/`preview` runs zfb from a mirrored copy under `.cache/site-runs/run-*` with a generated `zfb.config.ts` that spreads the authored config and overrides `outDir` and `bundle.define.__ZUDO_CASE_PREVIEW_ORIGIN__` (preview-origin.mjs:24–35). Comment at run-site.mjs:75: workaround for zfb#3318.
- `src/worker.js`, `src/worker-handler.js`, `scripts/prepare-worker-assets.mjs`: plain JS Cloudflare Worker that streams two >25 MiB assets in chunks; reads only `dist/previews` and `dist/downloads` (prepare-worker-assets.mjs:48, 104). Not affected by zfb 3.
- `scripts/setup.mjs`: `defaultCliVersion = "5.27.0"` (line 9), `create-zudo-doc@${cliVersion}` (201, 250, 267); `tests/setup.test.mjs` asserts `5.27.0` (lines 45, 51, 60, 68, 70). `.handoff/setup-record.json` records the scaffold provenance (create-zudo-doc 5.27.0, commit 50cbd5c, copied files: `.npmrc`, `pages/docs/[[...slug]].tsx`, `pages/index.tsx`, `pnpm-workspace.yaml`, 4 favicons, `scripts/check-links.js`, `src/styles/global.css`, `tsconfig.json`).

Audit run (`zfb` 3.1.0, temporary `wind: { spec: 1 }` config, authored config restored, `git status --short` clean): `outcome: complete`, exit 0, **unrecognized classes: none, dead classes: none, conflicts: none**, 22 `auditInfo` diagnostics only: ZW005 ×12 (slash modifiers in URL/path string literals in `src/worker.js`, `src/worker-handler.js`, `src/chrome-bindings.js`, `pages/*`), ZW012 ×5 (dynamic string joins in `worker-handler.js` and the `docs;${locale}` route signature), ZW002 ×2, ZW001 ×3 (Japanese text literals in `preview-links.jsx`). None is in a `class` position; nothing needs a token or an `authoredClasses` entry today. The standalone audit plan does not see zudo-doc's package-owned routes or candidates (https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/), so this is a host-code census only. `zfb check` was not run (preset imports need `node_modules`).

File-by-file change list (host-owned files only; `src/content/docs/**`, `public/**`, `engineering/**`, `project/**` are untouched):

| File | Lines | Change | When |
| --- | --- | --- | --- |
| `zfb.config.ts` | 48 | none expected (the preset emits the engine keys); re-check every `zudoDoc()` option name against the 6.0.0 consumer guide | after 6.0.0 |
| `src/chrome-bindings.js` | 6 | none unless `defineChromeBindings`/`mdxExtras` change | after 6.0.0 |
| `src/components/preview-links.jsx` | 19 | `h` from `@takazudo/zfb/zudo-react`; `"border-radius"` style key | now (builds only after 6.0.0) |
| `src/styles/global.css` | 28 | replace with the `create-zudo-doc@6` template (6 ZW009 sites: lines 8, 9, 20, 21, 22, 26) | after 6.0.0 |
| `pages/docs/[[...slug]].tsx` | 62 | replace with the 6.0.0 scaffold stub (pragmas 1–2, `preact` type import 27) | after 6.0.0 |
| `pages/index.tsx` | 6 | none | — |
| `tsconfig.json` | 12 | drop the three `react*` → `preact/compat` paths; base comes from zudo-doc 6 | after 6.0.0 |
| `package.json` | 60 | 3 zfb pins + 2 zudo-doc pins; remove `preact`, `preact-render-to-string` | after 6.0.0 |
| `scripts/run-site.mjs`, `scripts/lib/preview-origin.mjs` | 94 + 36 | replace the per-run mirror with `--scratch-dir` + `--define`, or keep as is | now (verify on the installed binary) |
| `scripts/setup.mjs:9,229`, `tests/setup.test.mjs:45,51,60,68,70`, `README.md` | — | `5.27.0` → the create-zudo-doc 6 version | after 6.0.0 |
| `.github/workflows/check.yml`, `deploy.yml` | 77, 103 | add `zfb wind audit --fail-on error` after `pnpm check` | after 6.0.0 |
| `src/worker.js`, `src/worker-handler.js`, `scripts/prepare-worker-assets.mjs`, `wrangler.docs.jsonc`, `wrangler.preview.jsonc` | — | none | — |

## Sequencing and blockers

1. **zudo-doc 6.0.0 on zfb 3 must ship first** (epic zudolab/zudo-doc#4430; root PR zudolab/zudo-doc#4477 is still a draft and the remote branch carries only planning resources). Its own integration floor is blocked on zfb fixes (#3569, #3570) that are merged on zfb `main` but not released, so 6.0.0 needs the **next zfb release after 3.1.0**. Read the consumer migration guide zudolab/zudo-doc#4473 when it exists; it will state the new `zudoDoc()` shape (planned: drops `framework`/`tailwind`, emits `wind` with its tokens, reset and a `@takazudo/zudo-doc/wind.json` candidate manifest, with an optional host `wind` override) and what `create-zudo-doc@6` scaffolds.
2. **Now (no upstream dependency):** port `src/components/preview-links.jsx` to zudo-react `h()` with CSS-spelled style keys; replace the `.cache/site-runs` mirror with `--scratch-dir` + `--define` (both shipped in 3.0.0 per https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ and https://zfb.takazudomodular.com/api/cli/#per-invocation-defines; zfb#3318 was closed as completed on 2026-09-29). Keep the port on a branch; it cannot build until step 1.
3. **When 6.0.0 ships:** re-scaffold the copied files with `create-zudo-doc@6` (same `setup.preset.json`), bump the four `@takazudo/*` pins in lockstep, delete `preact`/`preact-render-to-string`, run the 7-step checklist below.
4. **Next zfb release only:** `zfb wind audit --json`, `file:line:col` diagnostics (#3370) and `wind.strict` (#3365/ZW014) make the CI gate in step 6 of the checklist cleaner, but nothing in this repo needs them to migrate.

## Required changes

### 1. Dependencies, config, tsconfig, env

- `package.json` (root): bump `@takazudo/zfb`, `@takazudo/zfb-md-wasm`, `@takazudo/zfb-runtime` to the zfb line zudo-doc 6.0.0 requires (3.1.0 or the next release; keep them identical), `@takazudo/zudo-doc` and `@takazudo/zudo-doc-history-server` to 6.0.0, and remove `preact` and `preact-render-to-string` unless the 6.0.0 guide keeps `preact` as an install-time peer for zdtp (this repo does not enable `designTokenPanel`, so expect to drop it). Why: https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands.
- `zfb.config.ts`: no host-side `framework`/`tailwind` key exists; nothing to delete. If 6.0.0 exposes a host `wind` override, this project has no host tokens to declare (0 host utilities in the audit), so leave it absent. Why: https://zfb.takazudomodular.com/zudo-wind/configuration/.
- `tsconfig.json`: expect the 6.0.0 scaffold/`tsconfig.base.json` to set `"jsx": "react-jsx"` and `"jsxImportSource": "@takazudo/zfb/zudo-react"`; delete the three `react*` → `preact/compat` path aliases (lines 7–9) if the new base does not. Why: https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands.
- `scripts/run-site.mjs` + `scripts/lib/preview-origin.mjs`: replace the mirror + generated config with direct invocations, e.g. `zfb build --define __ZUDO_CASE_PREVIEW_ORIGIN__='"https://zudo-case-preview.zudolab.dev"'` and `zfb dev --scratch-dir .zfb-build/dev --define __ZUDO_CASE_PREVIEW_ORIGIN__='""'`. `--define` is accepted by `zfb dev`, `zfb build` and `zfb check` (not `preview`) and wins per key over `bundle.define`; `--scratch-dir` isolates concurrent runs. Keep `previewRunConfig`'s `outDir` canonicalisation only if a concurrent `build` must still write to the checkout's `dist/` (it does: `deploy:docs` reads `./dist`). Why: https://zfb.takazudomodular.com/api/cli/#per-invocation-defines and https://zfb.takazudomodular.com/api/cli/#scratch-dirs. Verify against the live 3.x binary; #3318 was closed without a linked PR.
- `scripts/setup.mjs:9` `defaultCliVersion = "5.27.0"` → the create-zudo-doc 6 version; update the fixture strings in `tests/setup.test.mjs:45,51,60,68,70` and `README.md` ("公式`create-zudo-doc@5.27.0`").
  If the mirror goes, the `package.json` scripts become plain zfb invocations (the define value is a raw esbuild expression, so a string needs its own inner quotes; `""` reproduces `previewRunConfig`'s dev value and `preview` takes no define):

```json
"dev": "zfb dev --scratch-dir .zfb-build/dev --define __ZUDO_CASE_PREVIEW_ORIGIN__='\"\"'",
"build": "zfb build --define __ZUDO_CASE_PREVIEW_ORIGIN__='\"https://zudo-case-preview.zudolab.dev\"'",
"check": "zfb check --define __ZUDO_CASE_PREVIEW_ORIGIN__='\"\"'",
"preview": "zfb preview"
```

  `ZUDO_CASE_PREVIEW_ORIGIN=http://localhost:8787 pnpm dev` (README) then becomes `pnpm dev -- --define __ZUDO_CASE_PREVIEW_ORIGIN__='"http://localhost:8787"'` (the last occurrence of a repeated key wins), or a 10-line wrapper that validates the origin with `resolvePreviewOrigin()` and appends the flag. `tests/preview-links.test.mjs:47–63` ("per-run config preserves definitions") goes with the mirror.
- No `ZFB_TAILWIND_BIN` / `ZFB_TAILWIND_OXIDE_WARMUP` anywhere (grep: 0 hits); nothing to remove. Why: https://zfb.takazudomodular.com/guides/migrating-to-v3/#cli-and-environment.

### 2. CSS and utilities

- `src/styles/global.css` lines 8, 9, 20, 21, 22, 26 are ZW009 errors in zfb 3 even under `wind: false` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives). This file is a scaffold copy with an empty `@theme {}` override slot; take the `create-zudo-doc@6` template instead of editing it. If the 6.0.0 scaffold keeps a token-override slot it will be a `:root { --… }` block or a config `wind.tokens` entry; this repo overrides nothing.
- No host-authored utilities exist (41 MDX files have 0 `class=`), so there are no tokens to declare, no `authoredClasses` and no reset choice to make at host level; the reset (`none` | `minimal-v1` | `owned-v1`) will be the preset's decision. Why: https://zfb.takazudomodular.com/zudo-wind/configuration/#fields.
- `preview-links.jsx:13` iframe `style` object: zudo-react rejects camelCase keys, so `borderRadius: '8px'` → `"border-radius": "8px"`; `border` stays. Why: https://zfb.takazudomodular.com/zudo-react/components-and-jsx/#styles-and-events.

### 3. Components and islands

- `src/components/preview-links.jsx`: change line 1 to `import { h } from "@takazudo/zfb/zudo-react"`; keep the `h(tag, props, ...children)` shape (public `h` is supported; variadic children replace `props.children`). Attributes: `href`, `src`, `title`, `width`, `height`, `loading` follow HTML spelling already; `height: 720` is a number, which is accepted for scalar attributes. These components are static (rendered through zudo-doc's MDX pipeline, not islands), so no signals are needed. Why: https://zfb.takazudomodular.com/zudo-react/components-and-jsx/#jsx-creates-a-description. Verify `loading` on `<iframe>` builds on the target zfb: the renderer uses a finite attribute vocabulary and names unsupported attributes at build time (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/#use-html-and-svg-names); the broader vocabulary fixes (#3359) are in the next release.
  The ported file, line for line (only the import and one style key change):

```js
import { h } from "@takazudo/zfb/zudo-react";
import { previewUrl, previewSourceLabel, PRODUCTION_PREVIEW_ORIGIN } from "../../scripts/lib/preview-origin.mjs";

const origin = typeof __ZUDO_CASE_PREVIEW_ORIGIN__ === "undefined"
  ? PRODUCTION_PREVIEW_ORIGIN : __ZUDO_CASE_PREVIEW_ORIGIN__;

export function PreviewLink({ path, children }) {
  return h("a", { href: previewUrl(path, origin) }, children);
}

export function PreviewFrame({ path, title, height = 720 }) {
  return h("div", null,
    h("iframe", { src: previewUrl(path, origin), title, width: "100%", height, loading: "lazy",
      style: { border: "1px solid #6b7280", "border-radius": "8px" } }),
    h("p", null, `表示元: ${previewSourceLabel(origin)}`));
}

export function PreviewSource() {
  return h("p", null, `表示元: ${previewSourceLabel(origin)}`);
}
```

- `src/chrome-bindings.js`: unchanged in shape (`defineChromeBindings({ mdxExtras })`); whether `mdxExtras` keeps its name is a zudo-doc 6.0.0 contract item (zudolab/zudo-doc#4473).
- `pages/docs/[[...slug]].tsx`: delete lines 1–2 (`@jsxRuntime`/`@jsxImportSource preact` pragmas; a leftover pragma fails the build with `Could not resolve "preact/jsx-runtime"` or `ZR_CHILD`) and the `import type { JSX } from "preact"` on line 27. Prefer replacing the whole stub with the `create-zudo-doc@6` version; its comment block references `virtual:zudo-doc-*` modules whose 6.0.0 shape is zudo-doc's to define. Why: https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands.
- `pages/index.tsx`: 1-line re-export, keep.
- No host islands, forms, `dangerouslySetInnerHTML`, `className` or React-style handlers in host code (grep: `className=`/`onClick=`/`dangerouslySetInnerHTML` 3 hits total, all inside the scaffold route stub comments or `preview-links.jsx` string text).

### 4. md-wasm and other packages

- `@takazudo/zfb-md-wasm` is a lockstep pin only; no host code calls `compile()`/`renderHtml()` (0 hits), so the `jsxRuntime` removal does not touch this repo. Why: https://zfb.takazudomodular.com/guides/migrating-to-v3/#mdx-and-md-wasm.
- `src/worker.js` / `src/worker-handler.js`: framework-free Worker code; unchanged. `wrangler.preview.jsonc` `run_worker_first` routes stay.

### 5. Tests, CI, deploy

- `.github/workflows/check.yml` and `deploy.yml`: no engine-specific env; they run `pnpm check` (→ `zfb check`), `pnpm build`, `pnpm check:built-preview-links`, `pnpm prepare:worker-assets`. Add `node_modules/.bin/zfb wind audit --fail-on error` after `pnpm check` once on 3.x (https://zfb.takazudomodular.com/api/cli/#zfb-wind). Node 24 + pnpm 10.30.3 are fine; zfb 3's Linux glibc floor fix (#3584) is **next release** only if the runner image trips it (ubuntu-latest did not for the 3.1.0 audit here).
  In both workflows the addition is one step after `- run: pnpm check` (the authored `zfb.config.ts` loads the preset from `node_modules`, which CI has installed by then):

```yaml
      - run: pnpm check
      - run: pnpm exec zfb wind audit --fail-on error
```

- `tests/preview-links.test.mjs` imports only `scripts/lib/*` helpers (no Preact) and keeps working; `tests/setup.test.mjs` needs the version-string bump above.
- `scripts/check-built-preview-links.mjs` and `scripts/verify-deployed-assets.mjs` read built HTML/bytes and are engine-agnostic.
- Deploy topology (two Workers, chunked large assets, `deploy:preview` then `deploy:docs`) does not change. The zfb Cloudflare adapter is not used here (static output), so adapter fixes (#3480, next release) are irrelevant.

## Step-by-step plan

Adapted from the 7-step checklist in https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist.

1. **Wait for zudo-doc 6.0.0**; meanwhile branch `zfb3/host-prep` and land the engine-neutral prep: port `preview-links.jsx` (step 4 below) behind the same file, replace the `.cache/site-runs` mirror with `--scratch-dir`/`--define` (verify on the current 2.20.2 binary first: `pnpm exec zfb build --help | grep -E 'define|scratch'`; if 2.20.2 lacks them, keep the mirror until the pin bump).
2. **Upgrade the preset, then install matching v3 packages** in one commit: `pnpm add @takazudo/zudo-doc@6 @takazudo/zfb@<v3> @takazudo/zfb-runtime@<v3> @takazudo/zfb-md-wasm@<v3>`, `pnpm add -D @takazudo/zudo-doc-history-server@6`, `pnpm remove preact preact-render-to-string`; run `pnpm exec zfb --version` (expect the release version + `embedded esbuild`, no Tailwind line).
3. **Re-scaffold the copied files**: run `pnpm dlx create-zudo-doc@6` into a scratch dir with this repo's `setup.preset.json`, then copy over `pages/docs/[[...slug]].tsx`, `pages/index.tsx`, `src/styles/global.css`, `tsconfig.json` (and `.npmrc`/`pnpm-workspace.yaml` if they changed). Keep `zfb.config.ts`, `src/content/docs`, `public`, `src/chrome-bindings.js`, `src/components`. Run `pnpm check` (`zfb check`) and read any removed-key message; none should come from this repo's config.
4. **Remove Tailwind imports and directives**: confirmed by step 3's new `global.css`. Run `pnpm exec zfb css --input src/styles/global.css --output /tmp/zudo-case.css` and `pnpm exec zfb wind audit --fail-on error`; both must pass with no ZW009.
5. **Components**: `src/components/preview-links.jsx` → `import { h } from "@takazudo/zfb/zudo-react"`, `"border-radius": "8px"`; `grep -rln "@jsxImportSource" src pages` must return nothing after step 3. `pnpm check`.
6. **md-wasm**: nothing to do (no direct calls).
7. **Build and compare**: `pnpm build` (through `scripts/run-site.mjs` or the direct command from step 1), then `pnpm check:built-preview-links`, `pnpm prepare:worker-assets`, `node --test tests/worker-assets.test.mjs tests/worker-assets-deployment.test.mjs`; open `/docs/overview/start/`, `/docs/resources/current-preview/` (uses `<PreviewFrame>`), `/files/` (asset viewer) and the theme toggle + search chrome at desktop 1440×900 and narrow 390×844 as the README's browser-validation record did on 2026-09-24. Pixel-compare a page with lists, headings, tables and the iframe border.
8. Update `README.md`/`AGENTS.md` version strings and `scripts/setup.mjs`/`tests/setup.test.mjs` (5.27.0 → 6.x), push, let `check.yml` run, merge, let `deploy.yml` deploy both Workers and run its full-GET SHA-256 verification.

## Verification checklist

- [ ] `pnpm exec zfb --version` prints a 3.x version and `embedded esbuild` only.
- [ ] `pnpm check` passes with no `framework`/`tailwind` removed-key error and no `@jsxImportSource` warning line.
- [ ] `pnpm exec zfb wind audit --fail-on error` exits 0; the standalone report shows `unrecognized classes: (none)` for host code as it does today.
- [ ] `grep -rnE '@import\s+"tailwindcss|@source|@theme' src/styles` returns nothing.
- [ ] `grep -rln "from ['\"]preact\|@jsxImportSource" src pages` returns nothing.
- [ ] `pnpm build` emits 60 pages (the README's current count) and `pnpm check:built-preview-links` passes.
- [ ] `<PreviewFrame>` renders the iframe with the 1px `#6b7280` border and 8px radius; `<PreviewLink>` hrefs point at `https://zudo-case-preview.zudolab.dev/...` in the production build and at `/previews/...` under `pnpm dev`.
- [ ] Theme toggle, search, image enlarge and the `/files/` asset viewer hydrate (these are zudo-doc 6 islands; failure here is a preset bug to report upstream, not host work).
- [ ] `node --test tests/*.test.mjs` passes after the version-string updates.
- [ ] Deploy workflow's HEAD probes and full-GET SHA-256 verification pass on both hosts.

## Risks and open questions

- The whole timeline is zudo-doc's: 39/51 topics exist only in the maintainer's local branch and the integration floor waits on an unreleased zfb. Do not estimate a date from this guide.
- `chromeBindingsModule` / `defineChromeBindings({ mdxExtras })` is the only host→preset API this repo uses; if 6.0.0 renames or re-types it, `src/chrome-bindings.js` and the three preview components follow the consumer guide (zudolab/zudo-doc#4473).
- `scripts/run-site.mjs` exists because `bundle.define` had to differ per run and concurrent runs collided (#3318). zfb 3.0.0's `--scratch-dir` + `--define` are the designed replacement, but #3318 closed without a linked PR; verify that a `dev` with its own `--scratch-dir` keeps its define while a concurrent `build` with a different define runs, before deleting the mirror. `zfb preview` rejects `--define`, which is fine (preview serves `dist/`).
- `preview-links.jsx` passes `loading: "lazy"` and a numeric `height` to `<iframe>`; the finite attribute vocabulary is widened in the **next zfb release** (#3359). If the target release rejects an attribute, the build names it and the file:line (https://zfb.takazudomodular.com/zudo-react/server-rendering/#read-a-render-failure).
- Utility placement flipped from Tailwind's order (utilities first) to `after-authored` by default; zudo-doc's own stylesheets are affected, not this host's (0 host utilities). `wind.utilities.placement: "before-authored"` is **next release** only.
- The audit's standalone plan cannot see zudo-doc-owned candidates; a full inventory of what the preset renders only comes from `zfb build`/`zfb dev` on 6.0.0.

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/
- https://zfb.takazudomodular.com/zudo-react/components-and-jsx/
- https://zfb.takazudomodular.com/zudo-wind/configuration/
- https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/
- https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- https://zfb.takazudomodular.com/api/cli/
- https://zfb.takazudomodular.com/api/define-config/
- https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ and https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/
- zudo-doc 6.0.0: zudolab/zudo-doc#4430 (epic), zudolab/zudo-doc#4477 (root PR), zudolab/zudo-doc#4473 (consumer migration guide)
- Takazudo/zudo-front-builder#3318 (closed 2026-09-29; the `scripts/run-site.mjs` workaround), #3359 (renderer vocabulary, next release), #3370 (`--json` audit, next release)
- Repo files: `zfb.config.ts`, `src/chrome-bindings.js`, `src/components/preview-links.jsx`, `src/styles/global.css`, `pages/docs/[[...slug]].tsx`, `scripts/run-site.mjs`, `scripts/lib/preview-origin.mjs`, `scripts/setup.mjs`, `.handoff/setup-record.json`, `.github/workflows/check.yml`, `.github/workflows/deploy.yml`, `wrangler.docs.jsonc`, `wrangler.preview.jsonc`
