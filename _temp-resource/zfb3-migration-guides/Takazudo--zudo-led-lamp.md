# zfb v3 migration guide: Takazudo/zudo-led-lamp

Generated 2026-10-04 by an automated diagnosis of `main` @ `1fc399e`; the wind audit used the zfb 3.1.0 CLI. zfb 3.2.0 was released on 2026-10-04 while this diagnosis was being written, so the audit was re-run with the 3.2.0 CLI and both results are reported below. Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**Blocked** on `@takazudo/zudo-doc` 6.0.0 (zudolab/zudo-doc#4430, root PR #4477). The only zfb project dir is `doc/`, a zudo-doc 5.27 host: every page route, the chrome, Tailwind entry and Preact JSX runtime come from the preset, so nothing can move to zfb 3 before the preset does. The repo-owned surface that must be ported is small but real: 3 Preact islands (331 lines, 12 hook calls plus zudo-doc's `useModalDialog`), 7 SSR-only MDX components (227 lines), 1 chrome-bindings module, 1 scaffold-derived route stub, 535 lines of authored CSS and a CI gate script that parses `zfb warn:` output. **Effort M** (one focused PR of roughly 2 days once zudo-doc 6.0.0 exists; about half a day of prep is possible now). One thing gates the start: zudo-doc 6.0.0 on npm. The zfb side is settled: **zfb 3.2.0 (released 2026-10-04)** makes the repo's BEM class names (`zld-model-viewer__caption`, 19 distinct) ordinary class text (Takazudo/zudo-front-builder#3365) — they were ZW001 build errors on 3.1.0; the 3.2.0 re-audit reports 0 ZW001 — and pin exact `3.2.0`, not `^3.1.0`. The repo is live (last commit 2026-10-04) and deploys from CI; do not retire.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `doc/` (`doc/package.json`, `doc/zfb.config.ts`) | `@takazudo/zfb` 2.20.2, `-runtime` 2.20.2, `-adapter-cloudflare` 2.20.2, `-md-wasm` 2.20.2 | `@takazudo/zudo-doc` ^5.27.0, `-history-server` ^5.27.0, `@takazudo/zdtp` 0.8.2 (unused by host code; see #125) | `tailwindcss` ^4.2.0 + `@tailwindcss/vite` ^4.2.0 (devDeps); 2 Tailwind imports, 3 `@source`, 1 `@theme` in `doc/src/styles/global.css` | `preact` ^10.29.1, `preact-render-to-string` ^6.6.6, `@types/react` ^19.2.0; 12 files with `/** @jsxImportSource preact */`; hooks: `useState` 5, `useRef` 5, `useEffect` 2 | 2 `<Island when="visible">` wrappers (`doc/component-docs/ui/footprint-preview.tsx:15`, `package-model-viewer.tsx:12`) around 2 `"use client"` roots + 1 shared `"use client"` dialog | dependency only (`doc/package.json:47`), 0 source imports (zudo-doc's lazy HtmlPreview) | Cloudflare Workers via adapter (`dist/_worker.js`, `doc/wrangler.toml`, `nodejs_compat`), wrangler 4.114.0; `.github/workflows/main-deploy.yml` + `pr-checks.yml` (preview alias) |
| `doc/enclosure-viewer/` | none (plain esbuild bundle of three.js into `doc/public/assets/enclosure/`) | – | none | none | none | – | copied as static assets |
| repo root (`boards/`, `footprints/`, `enclosure/`, `scripts/`) | none | – | – | – | – | – | KiCad/Python; `component-spec-skills.yml` is Python-only |

Audit run (`zfb wind audit --project-root doc` on zfb 3.1.0 with a swapped minimal config): `outcome: complete`, 4 "unrecognized classes" (all authored `zld-*` names), 1 dead class (`z-modal` → ZW006), 3 dynamic constructions (2 are non-class template literals; 1 is the class template `zld-preview-dialog--${variant}` at `doc/src/component-preview/preview-enlarge-dialog.tsx:47`), **12 error-severity diagnostics**: 11 × ZW001 at BEM `__` class positions in the three island files and 1 × ZW006 for `z-modal`. Re-run on zfb 3.2.0 (`spec: 1 revision 4`): 15 "unrecognized classes" (the 4 above plus the 11 `zld-*__*` BEM positions, now ordinary), the same 1 dead class and 3 dynamic constructions, **1 error-severity diagnostic** (ZW006 `z-modal`), 8 auditInfo. The 95 MDX content files contain **0** `class=` attributes, so the site has no utility candidates of its own; the host CSS is 100 % authored. `zfb check` was not run (preset import needs `node_modules`).

## Sequencing and blockers

1. **zudo-doc 6.0.0** (zudolab/zudo-doc#4430 / PR #4477; consumer migration guide planned as zudolab/zudo-doc#4473). Until it ships, `zudoDoc()` still emits the removed `framework`/`tailwind` keys and config loading fails on zfb 3 regardless of what this repo does (https://zfb.takazudomodular.com/guides/migrating-to-v3/#before-you-start). Watch for: the `./safelist.css` export going away (it is a `@source inline(...)` sheet, ZW009 under wind), a `wind.json` candidate manifest, `tsconfig.base.json` flipping `jsxImportSource` to `@takazudo/zfb/zudo-react`, regenerated `pages/docs/[[...slug]].tsx` and `global.css` scaffold templates, and the replacement for the `useModalDialog` hook: the locked spec in zudolab/zudo-doc#4473 renames it `modalDialog(scope, options)` with exported option/result interfaces, makes `ENLARGE_DIALOG_STYLE` a string, and deletes the compatibility aliases (port #4441, merged into the maintainer's base branch, which is now pushed as `base/zfb3-migration` @ `70e0875` — PR #4477, 102 commits — but not published; see `zudolab--zudo-doc.md`). Treat the name as planned, not published.
2. **zfb 3.2.0 (released 2026-10-04)** — already available; it ships (a) underscore class names as ordinary text (#3365) — this repo has 19 distinct `zld-*__*` names at 21 class positions, re-audited on 3.2.0 with 0 ZW001; (b) the Cloudflare worker dangling source-map fix (#3480) that affects `dist/_worker.js`; (c) `zfb wind audit --plan build` / `--json` and `file:line:col` diagnostics (#3370) which make the audit usable on `component-docs/`. It also carries #3569/#3570, so zudo-doc 6.0.0's integration floor is unblocked (`base/zfb3-migration` @ `70e0875` is pushed, PR #4477; 6.0.0 itself is still unpublished).
3. **Now (safe on 2.x)**: the prep list in "Step-by-step plan" steps 0a–0h — dependency hygiene, HTML attribute spellings Preact already accepts, authored `z-index` instead of the `z-modal` utility, the zdtp decision (#125), the `check-zfb-link-warnings.sh` prefix hardening (#127), doc prose.
4. **After zudo-doc 6.0.0 ships**: one migration PR following the official 7-step checklist, with the scaffold refresh procedure already written in `doc/SCAFFOLD.md` ("Refresh procedure") and `ZUDO_DEPS_PINS.md` re-pointed at the create-zudo-doc 6 release.

## Required changes

### 1. Dependencies, config, tsconfig, env

- `doc/package.json:44-75` — bump `@takazudo/zfb`, `-runtime`, `-adapter-cloudflare`, `-md-wasm` to exact `3.2.0` (released 2026-10-04), or the later lockstep release zudo-doc 6.0.0 pins — never `^3.1.0`, which would still admit the 3.1.0 build with the UTF-8 offset panic (#3569); bump `@takazudo/zudo-doc` / `-history-server` to `^6.0.0`. Remove `tailwindcss`, `@tailwindcss/vite` (l.67, l.73), `@types/react` (l.70), `preact` (l.59), `preact-render-to-string` (l.60) — v3 ships the runtime inside `@takazudo/zfb/zudo-react` (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Keep `preact` only if zdtp stays and still declares it as a peer (see §3). Keep `three`, `esbuild`, `wrangler`.
- `doc/zfb.config.ts:4-140` — no `framework`/`tailwind` keys of its own; nothing to delete here. After zudo-doc 6.0.0, add what the preset asks for (expected: a `wind` block or `zudoDoc({ wind })` override; verify against the zudo-doc 6 guide). Keep `adapter: "@takazudo/zfb-adapter-cloudflare"` (l.138).
- `doc/tsconfig.json:6-11` — drop the `react`, `react/jsx-runtime`, `react-dom` → `preact/compat` path aliases. `jsx: "react-jsx"` and `jsxImportSource: "@takazudo/zfb/zudo-react"` are expected to come from zudo-doc 6's `tsconfig.base.json`; if the shipped base does not set them, set them here (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands).
- Env: `grep -rn ZFB_TAILWIND .` → 0 hits; nothing to remove. `doc/.gitignore` last line `/src/styles/zfb-tailwind-entry-*.css` is a 2.x leftover; delete.
- `ZUDO_DEPS_PINS.md` and `doc/SCAFFOLD.md` currently pin `create-zudo-doc@5.27.0` (commit `50cbd5c6`); the refresh procedure in `SCAFFOLD.md` is the right process, re-targeted at the 6.0.0 scaffold.

### 2. CSS and utilities

`doc/src/styles/global.css` (535 lines):

- l.8 `@import "tailwindcss/preflight" layer(zd-preflight);` and l.9 `@import "tailwindcss/utilities";` → remove (ZW009; https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives).
- l.14 `@import "@takazudo/zudo-doc/safelist.css";` → remove when zudo-doc 6 drops the export (it is a Tailwind `@source inline(...)` file and would be a ZW009 error through the import chain).
- l.20-22 three `@source` lines → remove (wind's source plan owns scan roots; https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/).
- l.26-28 empty `@theme {}` → remove; token overrides move to `wind.tokens` in `zfb.config.ts` or stay authored custom properties (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#theme-and-tokens).
- l.7 `@layer zd-preflight, zd-flow;` — ordinary `@layer` is still valid; keep or drop per the zudo-doc 6 CSS chain (its plan removes the `zd-preflight` layer in favour of `reset: "owned-v1"` plus a preflight-delta block).
- l.30-535 authored `.zld-*` rules: unchanged in form. They consume 22 distinct zudo-doc theme custom properties (`grep -oE 'var\(--[a-z0-9-]+' doc/src/styles/global.css | sort -u`): `--spacing-hsp-xs|sm|lg`, `--spacing-vsp-2xs|xs|sm|lg`, `--color-muted|surface|bg|fg|accent|overlay`, `--text-caption`, `--z-index-local-1|2`, `--spacing-image-overlay-inset`, `--color-image-overlay-fg|bg`, `--spacing-icon-sm|lg`, `--default-transition-duration`. All 22 are declared in zudo-doc 5.28.2's `packages/zudo-doc/src/theme.css` (so is `--z-index-modal`, l.284); `--default-transition-duration` is zudo-doc's own declaration there (l.304), not a Tailwind default, so it survives the Tailwind removal. zudo-doc 6 plans to keep these as authored `:root` properties, but every name must be re-verified against the 6.0.0 `theme.css`.
- Utility placement flip: wind emits utilities **after** authored CSS by default; Tailwind emitted them before. This host has no own utilities and zudo-doc's own classes sit in the package, so the only tie to check is `z-modal` on the dialog (below). If zudo-doc 6 recommends `wind.utilities.placement: "before-authored"` (zfb 3.2.0), follow it (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties).
- `z-modal` (`doc/src/component-preview/preview-enlarge-dialog.tsx:47`) is the one utility class the host emits; it was generated from zudo-doc's `--z-index-modal` theme token. Audit on 3.1.0 and 3.2.0: `ZW006 ... unknown value or token modal` (the one error left on 3.2.0). Replace with authored CSS now (safe on 2.x):

```css
/* global.css, next to .zld-preview-dialog */
.zld-preview-dialog { z-index: var(--z-index-modal, 100); }
```

  and delete `z-modal` from the class template. Alternative: `wind.authoredClasses: { "z-modal": true }` (probe-confirmed to turn it into an ordinary class) once zudo-doc 6 documents how it ships `z-modal`.
- BEM names: 19 distinct `zld-*__*` names, 21 class positions in TSX (`grep -ohE 'zld-[a-z-]+__[a-z-]+' doc/src/**/*.tsx doc/component-docs/ui/*.tsx`), 49 selector occurrences in `global.css`. On **zfb 3.1.0** each class position is a ZW001 **error** (`zfb wind explain -- zld-model-viewer__caption` → `ZW001 invalid named utility characters`); on **zfb 3.2.0 (released 2026-10-04)** underscore names are ordinary (https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/#diagnostic-codes) — re-measured: `explain` answers `outcome: ordinary class` and the audit lists all 11 class positions under `unrecognized classes`, 0 ZW001. Measured workaround on 3.1.0: reserving a name in `wind.authoredClasses` removed its error from the audit (12 → 10 errors) but `zfb wind explain` still printed ZW001 for it — confirm with a real `zfb build` before relying on it. Recommended: do not rename; pin 3.2.0.
- `doc/component-docs/ui/*.tsx` sits outside wind's conventional roots (the 3.1.0 audit reported no diagnostics for `component-references.tsx` although it carries 10 BEM class positions). Harmless today (no utilities there), but any utility added there would silently emit no CSS. Either move the UI components under `doc/src/` or confirm coverage with `zfb wind audit --plan build` (zfb 3.2.0; https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/#the-source-plan).

### 3. Components and islands

Files and what changes (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/, https://zfb.takazudomodular.com/zudo-react/components-and-jsx/#use-html-and-svg-names):

| File | Lines | Preact-specific today | v3 change |
| --- | --- | --- | --- |
| `doc/src/component-preview/footprint-preview-island.tsx` | 84 | `useRef`×2 (l.16-17), `useState`×4 (l.18-21), `useEffect` (l.24-32), `className`×4, `onClick`×2, `onLoad`, `onError`×2, `{isDialogOpen && ...}` (l.76) | `signal()` for the 4 states; `Ref` objects `{ current: null }`; the load listener moves to `getScope().onActivate` (reads `imageRef.current`, returns cleanup); `class`, `on:click`, `on:load`, `on:error`; the dialog child region becomes `<Show when={isDialogOpen}>{() => ...}</Show>` |
| `doc/src/component-preview/preview-enlarge-dialog.tsx` | 95 | `useModalDialog` from `@takazudo/zudo-doc/use-modal-dialog` (l.10,30-37), `ENLARGE_DIALOG_STYLE` from `/island-types`, `AFTER_NAVIGATE_EVENT` from `/transitions`, `JSX.MouseEventHandler` cast (l.42), `className`×4, `onClick`×2, `onKeyDown`, `strokeWidth`, `z-modal` | Depends on zudo-doc 6's `modalDialog(scope, options)` (planned per zudolab/zudo-doc#4473; takes the scope from `getScope()` and `isOpen` as a readonly signal); `isOpen`/`onClose` props become a signal + callback; focus-trap `onKeyDown` → `on:keydown` with native `KeyboardEvent`; `stroke-width`; `style={ENLARGE_DIALOG_STYLE}` keeps working because the 6.0 constant becomes a string and zudo-react's `style` accepts a CSS string (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/#styles-and-events) |
| `doc/src/component-model-viewer/package-model-viewer-island.tsx` | 152 | `useRef`×3, `useState`, `useEffect` (l.57-90, dynamic `import("./viewer-runtime.ts")` with AbortController), `<>` fragment (l.22), `tabIndex`×1, `onPointerDown`, `onClick`×2, `onKeyDown`, `className`×7, `{isDialogOpen && ...}` (l.38), `instance === "inline" && ...` ×2 (static prop; plain JS is fine) | The effect is already the "third-party widget" shape: move it verbatim into `getScope().onActivate` (start `import()` inside the callback, abort on cleanup; https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget); `tabindex`; `on:pointerdown` etc.; dialog region via `Show`; `displayName` assignments (l.84, l.152) have no meaning and can go |
| `doc/component-docs/ui/{evidence-anchor,evidence-details,evidence-fact,evidence-table}.tsx` | 27+53+13+57 | Preact pragma + `import type { ComponentChildren, JSX } from "preact"`; already use `class=`; `tabIndex={0}` (evidence-table.tsx:53) | Remove pragmas and Preact type imports (use `Child`/`Description` from `@takazudo/zfb/zudo-react`; https://zfb.takazudomodular.com/zudo-react/api-reference/); `tabindex` |
| `doc/component-docs/ui/component-references.tsx` | 42 | `className`×10 | `class` |
| `doc/component-docs/ui/{footprint-preview,package-model-viewer}.tsx` | 19+16 | `import { Island } from "@takazudo/zfb"` + `<Island when="visible">` with one component child | Unchanged API (https://zfb.takazudomodular.com/api/island/#hydration-strategies); props are already JSON strings (`descriptor`, `assetUrl`, `footprintName`) |
| `doc/src/chrome-bindings.tsx` | 41 | pragma only | Drop pragma; keep `defineChromeBindings({ mdxExtras })` if zudo-doc 6 keeps the API |
| `doc/pages/docs/[[...slug]].tsx` | 76 | scaffold stub with pragma, `import type { JSX } from "preact"`, static island imports + `void` refs (l.38-45) | Regenerate from the create-zudo-doc 6 base template, then re-apply the two static island imports if zfb 3's scanner still needs them (ZUDO_DEPS_PINS.md note; open issue #126 tracks the resync) |

Pragmas: the same 12 files also carry `/** @jsxRuntime automatic */` (`grep -rl '@jsxRuntime automatic' doc/src doc/pages doc/component-docs` → 12). The migration guide only flags `@jsxImportSource` (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands); remove both lines together so `jsx`/`jsxImportSource` come from tsconfig alone.

Repo-wide counts (`grep -ohE '\b(className|onClick|onKeyDown|onError|onLoad|onPointerDown|strokeWidth|tabIndex)=' doc/src/**/*.tsx doc/component-docs/ui/*.tsx | sort | uniq -c`): `className=` 25, `onClick=` 6, `strokeWidth=` 3, `tabIndex=` 2, `onKeyDown=` 2, `onError=` 2, `onPointerDown=` 1, `onLoad=` 1. `dangerouslySetInnerHTML`: 0. Controlled inputs / `<select multiple>` / file-range-number-date inputs: 0. Portals, context, `useId`, `forwardRef`, `lazy`/`Suspense`: 0. Nested `<Island>`: 0 (the dialog is a child component, not a second island).

The 3 islands and `viewer-runtime.ts` (232 lines, three.js only) have identical copies in Takazudo/zudo-circuit-doc `packages/circuit-doc/src/islands/` (prefix `zcd-`); port once and copy, or switch this repo to consume `@takazudo/zudo-circuit-doc` once its v3 release exists.

zdtp: `@takazudo/zdtp` 0.8.2 is a dependency but no host file imports it; zudo-doc mounts `DesignTokenPanelBootstrap` unstyled on every page (issue #125). On zfb 3 zdtp stays an opaque Preact bundle (zudo-doc decision DD3; Takazudo/zudo-design-token-panel#1002). Decide now: `designTokenPanel: false` in `zfb.config.ts` (removes the `preact` requirement entirely) or keep it and import its CSS as zudo-doc 6 documents.

### 4. md-wasm and other packages

- `@takazudo/zfb-md-wasm` is only a dependency (`doc/package.json:47`); `grep -rn "zfb-md-wasm\|jsxRuntime\|compile(\|renderHtml(" doc --include=*.ts --include=*.tsx --include=*.mjs` → 0 source hits. Bump in lockstep; nothing else (https://zfb.takazudomodular.com/guides/migrating-to-v3/#mdx-and-md-wasm).
- `doc/component-docs/**` (generator, 50+ TS files, `node --experimental-strip-types`) emits MDX with only `<EvidenceAnchor id>`, `<EvidenceDetails label>`, `<EvidenceFact>`, `<EvidenceTable label>`, `<ComponentReferences descriptor>`, `<PackageModelViewer descriptor>`, `<CategoryNav category>` (`ALLOWED_COMPONENT_ATTRIBUTES`, `doc/component-docs/core/mdx.ts:41-49`; occurrences across the 39 generated pages: 820/35/391/38/35/0/1). No class attributes are emitted, so the generated tree is unaffected by wind; the MDX pipeline change is only the jsx-runtime import inside compiled MDX, handled by zfb.
- `doc/enclosure-viewer/` is a standalone esbuild bundle (`build.mjs`) into `public/assets/enclosure/`; not zfb, not Tailwind, unaffected.

### 5. Tests, CI, deploy

- `doc/component-docs/tests/evidence-fact.test.ts:5-6` imports `h` from `preact` and `renderToString` from `preact-render-to-string` to SSR-test `EvidenceFact` (esbuild-bundled at l.8-14); it runs as `pnpm test:components` (`main-deploy.yml:108-110`, `pr-checks.yml:129` "Component generator tests"). Port it to `renderToString` from `@takazudo/zfb/zudo-react/server` and build the tree with the zudo-react JSX runtime (a `.tsx` test file or `jsx()` from `@takazudo/zfb/zudo-react/jsx-runtime`) — https://zfb.takazudomodular.com/zudo-react/server-rendering/#render-a-page and https://zfb.takazudomodular.com/zudo-react/testing/. Removing `preact-render-to-string` from `doc/package.json` (§1) breaks this test until it is ported; the `^<div class="zld-evidence-fact">` assertion (l.29) should survive.
- `doc/component-docs/scripts/check-zfb-link-warnings.sh` (pr-checks.yml step "Hand-authored links resolve") fails on any `zfb warn:` line whose shape it does not recognise, by design. zfb 3 adds new warning shapes (the per-file `@jsxImportSource` pragma warning, ZW014 migration warnings); expect this gate to go red on the first v3 build until the known-false classes are reviewed. Fix the anchored-prefix fragility first (issue #127) so a changed prefix cannot make it silently green.
- `doc/component-docs/scripts/model-viewer-browser-smoke.mjs` (both workflows, "Package model viewer browser smoke") drives headless Chrome over CDP and queries `[data-zfb-island="PackageModelViewerIsland"]` (l.217-218) plus the `data-*` state attributes the island sets. v3 keeps `data-zfb-island` and adds `data-zfb-protocol="zudo-react/1"` (https://zfb.takazudomodular.com/concepts/islands/#how-islands-are-loaded); the state attributes are set by `viewer-state.ts`, framework-free. Expect this to keep passing; re-run after the port.
- `.github/workflows/main-deploy.yml:183-189` and `pr-checks.yml:275-281` write `dist/.assetsignore` with `_worker.js` and `_zfb_inner.mjs`; confirm the adapter's v3 output names before the first deploy (the adapter emits the file itself).
- `doc/wrangler.toml` needs no change for v3 (`main = "./dist/_worker.js"`, `nodejs_compat`). Wrangler stays read from `doc/package.json` devDependencies.
- `pnpm check` (`zfb check`) is the type gate in both workflows; add `zfb wind audit --fail-on error` after the migration as the checklist suggests (https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist).
- Node 22 in both workflows; zfb 3.2.0 lowers the Linux glibc floor to 2.34 (#3584) — irrelevant on `ubuntu-latest`.

## Step-by-step plan

0. Prep now, on 2.x (one small PR):
   - 0a. `doc/package.json`: remove `@types/react` (nothing imports `react`); keep everything else until zudo-doc 6.
   - 0b. HTML spellings Preact already accepts: `className`→`class` (25), `tabIndex`→`tabindex` (2), `strokeWidth`→`stroke-width` (3) in the files listed in §3; run `pnpm --dir doc check` and `pnpm --dir doc test:components`.
   - 0c. Replace `z-modal` with the authored `z-index` rule (§2) and drop the `displayName` lines.
   - 0d. Decide #125: add `designTokenPanel: false` to `doc/zfb.config.ts` unless the panel is wanted.
   - 0e. Harden `check-zfb-link-warnings.sh` per #127 (match `zfb warn: ` anywhere on the line, or add a liveness assertion).
   - 0f. Update prose: `doc/CLAUDE.md:9-10` ("Tailwind CSS v4", "Preact"), `doc/src/content/docs/getting-started/introduction.mdx:14`; `claude-md/doc.mdx:19-20` regenerates from `CLAUDE.md`.
   - 0g. Delete the `/src/styles/zfb-tailwind-entry-*.css` line from `doc/.gitignore`.
   - 0h. Record in `ZUDO_DEPS_PINS.md` that the next scaffold refresh targets create-zudo-doc 6 and list the theme custom properties `global.css` depends on (§2) so the 6.0 diff can be checked mechanically.
1. When zudo-doc 6.0.0 is on npm (zfb 3.2.0 already is): in `doc/`, `pnpm add @takazudo/zudo-doc@^6 @takazudo/zudo-doc-history-server@^6 @takazudo/zfb@3.2.0 @takazudo/zfb-runtime@3.2.0 @takazudo/zfb-adapter-cloudflare@3.2.0 @takazudo/zfb-md-wasm@3.2.0` (exact; or the later lockstep version 6.0.0 pins); `pnpm remove tailwindcss @tailwindcss/vite preact preact-render-to-string` (keep `preact` only for zdtp); `npx zfb --version` must print the v3 version and embedded esbuild only (checklist step 1).
2. Run `npx create-zudo-doc@6 probe --yes --no-install --no-git` outside the repo (the `SCAFFOLD.md` refresh procedure) and diff `pages/docs/[[...slug]].tsx`, `pages/index.tsx`, `tsconfig.json`, `src/styles/global.css`, `scripts/check-links.js` against `doc/`; re-apply the island imports and the authored CSS block (l.30-535); update `ZUDO_DEPS_PINS.md` pins.
3. `doc/zfb.config.ts`: add whatever the zudo-doc 6 guide requires for wind; `npx zfb check` (checklist step 2).
4. `doc/src/styles/global.css`: apply §2; `npx zfb css --input src/styles/global.css --output /tmp/out.css` must pass without ZW009 (checklist step 3).
5. `doc/tsconfig.json`: remove the preact path aliases; remove the 12 `@jsxImportSource` + 12 `@jsxRuntime` pragmas (`grep -rln "@jsxImportSource\|@jsxRuntime" doc/src doc/pages doc/component-docs`); port the islands and components per §3 and `component-docs/tests/evidence-fact.test.ts` per §5; `npx zfb check && pnpm test:components` (checklist step 4).
6. `npx zfb wind audit --fail-on error` → 0 errors; `npx zfb wind explain -- zld-model-viewer__caption` → ordinary class (zfb 3.2.0; confirmed by the 3.2.0 re-run) (checklist step 6).
7. `pnpm build` (runs `generate:enclosure-viewer`, `generate:models`, `generate:components`, then `zfb build`), then `pnpm check:images && pnpm check:anchors && pnpm check:built-component-references && pnpm test:model-viewer:browser && pnpm check:components && pnpm scan:artifacts` — the exact `main-deploy.yml` sequence; then `bash component-docs/scripts/check-zfb-link-warnings.sh <build log>` and triage any new warning shapes (checklist step 7).
8. `npx wrangler@4.114.0 deploy --dry-run` from `doc/`; open a PR so `pr-checks.yml` publishes a `pr-<N>` preview; compare the record page `/docs/components/records/al8860mp-13/` (footprint dialog, 3D viewer, keyboard focus trap) light/dark at 390 and 1280 px against production.

## Verification checklist

- [ ] `npx zfb --version` prints a 3.x version and only the embedded esbuild line.
- [ ] `npx zfb check` passes with `jsxImportSource: "@takazudo/zfb/zudo-react"` and no `preact` path aliases.
- [ ] `npx zfb wind audit --fail-on error` exits 0; the 19 `zld-*__*` names are reported as ordinary (or reserved).
- [ ] `npx zfb css --input src/styles/global.css --output /tmp/out.css` emits no ZW009 and `/tmp/out.css` contains the `.zld-*` rules.
- [ ] `grep -rn "@jsxImportSource\|from \"preact" doc/src doc/pages doc/component-docs` → 0, including `component-docs/tests/evidence-fact.test.ts` (unless zdtp keeps `preact` as a peer, which must not be imported by host JSX).
- [ ] `pnpm test:components` passes with the ported `evidence-fact.test.ts`.
- [ ] `pnpm build` log: no `zfb warn:` pragma line, no ZW014; `check-zfb-link-warnings.sh` passes.
- [ ] `pnpm test:model-viewer:browser` passes: viewer hydrates (`data-viewer-state="ready"`), dialog opens/closes, SPA back/forward re-mounts, no-JS state, WebGL failure fallback.
- [ ] Footprint preview: image loads, enlarge button appears only after hydration, dialog focus trap and return focus work (keyboard only).
- [ ] Reset check on a record page and the catalog: headings, `<details>` summary, tables, `<code>` font stack, `sub`/`sup`, `::placeholder` in the search box (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#differences-from-tailwind-preflight).
- [ ] `dist/.assetsignore` names match the adapter output; `npx wrangler deploy --dry-run` succeeds; PR preview routes return 200 (`pr-checks.yml` smoke).
- [ ] `pnpm check:components` and the "Generated output is committed" diff gate still pass (generator output must be byte-identical).

## Risks and open questions

- **BEM `__` on 3.1.0**: 11 build-blocking ZW001 errors unless zfb 3.2.0 (released 2026-10-04) is used or every name is reserved in `wind.authoredClasses` (reservation verified only in the audit, not in `explain`). Do not rename the 19 classes just for this; the fix shipped in 3.2.0.
- **`useModalDialog` replacement**: the dialog's behaviour (showModal/close sync, backdrop close, close on `AFTER_NAVIGATE_EVENT`, focus restore) depends on what zudo-doc 6 exports; the planned export is `modalDialog(scope, options)` (zudolab/zudo-doc#4473), unreleased. If the shipped signature differs from the planned signal-based shape, the two islands' `isDialogOpen` state must be a `signal` passed down rather than a boolean prop.
- **Theme token names**: 22 distinct zudo-doc custom properties are consumed by `global.css`; any rename in 6.0 silently breaks spacing/colour on the generated pages. Diff `node_modules/@takazudo/zudo-doc/dist/theme.css` 5.27 vs 6.0 before building.
- **`component-docs/` outside scan roots**: fine today; becomes a trap if utilities are ever added there. Resolve with `--plan build` (zfb 3.2.0), or relocate.
- **Link-warning gate**: new zfb 3 warning shapes will fail `check-zfb-link-warnings.sh` by design; budget time to classify them rather than loosening the gate.
- **Open issue #126** says the route stub is pinned to `v4.4.6`, while `ZUDO_DEPS_PINS.md` (2026-09-24) says `zudo-doc-v5.27.0`; reconcile and close #126 during the 6.0 scaffold refresh.
- **zdtp**: keeping the panel keeps `preact` in the tree (as zdtp's peer) and adds the "embedding a third-party widget" constraints; dropping it is the simpler path for this site.
- **Generated-page byte contract**: the generator must not learn about class names; all styling stays in the components and CSS (same rule as zudo-circuit-doc ADR-015).

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/ (7-step checklist; preset-first rule)
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ (directives, reset differences, placement/tie flip)
- https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/ (ZW001/ZW006/ZW009/ZW014, `authoredClasses`, `wind.strict`)
- https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/ (scan roots, `--plan build`, package roots)
- https://zfb.takazudomodular.com/zudo-wind/configuration/ (`wind` key, `authoredClasses`, `manifests`, `sources.packageRoots`)
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/ (hook → signal/scope map, `Show`, refs, raw HTML)
- https://zfb.takazudomodular.com/zudo-react/components-and-jsx/ (HTML/SVG spellings, `on:` listeners, styles, refs)
- https://zfb.takazudomodular.com/zudo-react/scopes-and-lifecycle/ (`onActivate`, `effect`, abort signal)
- https://zfb.takazudomodular.com/zudo-react/conditionals-and-lists/ (`Show`/`For`)
- https://zfb.takazudomodular.com/concepts/islands/ (island contract, `data-zfb-island`, embedding a third-party widget)
- https://zfb.takazudomodular.com/api/island/ (`when="visible"`, props boundary)
- https://zfb.takazudomodular.com/api/cli/ (`zfb wind explain|audit`, `zfb css`, `zfb check`)
- https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/ and https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/ (released 2026-10-04: #3365, #3480, #3370, #3569/#3570, glibc 2.34 floor)
- zudolab/zudo-doc#4430 (6.0.0 epic), #4477 (root PR), #4473 (consumer migration guide); see `zudolab--zudo-doc.md` in this directory for the preset's status
- Takazudo/zudo-front-builder#3365 (underscore class names), #3480 (adapter source maps), #3569/#3570 (zudo-doc blockers), #3370 (`--json`/`file:line:col`) — all shipped in zfb 3.2.0
- Takazudo/zudo-design-token-panel#1002 (zdtp bundling decision); zudo-led-lamp issues #69, #125, #126, #127
- Sibling guide: `Takazudo--zudo-circuit-doc.md` (same islands, packaged)
