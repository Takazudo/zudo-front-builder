# zfb v3 migration guide: Takazudo/case-estimate

Generated 2026-10-04 by an automated diagnosis of `main` @ `49fe131`; the `zfb wind audit` census used the zfb 3.1.0 CLI. zfb 3.2.0 was released (npm `latest`, https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/) while this diagnosis was being written, so every pin and label below targets 3.2.0. Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**ready, effort XL.** This is a live product site (https://panels.takazudomodular.com/, Netlify, deploys from `main`) that finished a Next.js → zfb migration on 2026-05-29 (PR #93) and has had no commits since. It pins `@takazudo/zfb 0.1.0-next.16` (released 2026-05-29) with **React 19 + Tailwind 4 + Headless UI**, no zudo-doc/zudo-sg preset, so nothing upstream gates the start — zfb 3.2.0 (npm `latest`, released 2026-10-04) can be adopted today. The cost is the whole component layer: 55 `'use client'` files, 51 `useState` / 51 `useEffect` / 36 `useCallback` / 23 `useRef` / 9 `useMemo` call sites, one React context, one `createPortal`, one class error boundary, Headless UI `Dialog`/`Listbox`/`Transition`, 474 `className`, and a 93-line Tailwind `@theme` + 13 `@apply` rules. Two structural facts must be fixed before any 2.x or 3.x binary even builds it: (1) the zfb project lives in `zfb-app/` but imports `../components`, `../hooks`, `../utils`, `../data`, `../types` through `tsconfig` paths — relative imports above the project root hard-fail since zfb 0.1.0-next.90 (2026-07-21); (2) the shared components are only scanned for utilities through `@source "../components"`, which is a ZW009 error in v3. Both disappear if the zfb project moves to the repo root, which the config comment (`zfb-app/zfb.config.ts:7-9`) already planned once Next was retired (it was, in 93d9f19). If the site is not going to be touched otherwise, the alternative is to keep the pinned next.16 binary frozen; it still builds, but it is a dead prerelease line (the `next` dist-tag no longer resolves to it) and gets no fixes.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `zfb-app/` (zfb project; `pages/` 9 routes + `_mdx-components.tsx`, `layouts/default.tsx`, 6 components, 6 MDX in `content/`) | `@takazudo/zfb` + `zfb-runtime` **0.1.0-next.16**; `framework: 'react'`, `tailwind: { enabled: true }`, `site`, `outDir: 'dist'`, `publicDir: 'public'` (symlink → `../public`), 1 collection `content` | none | yes: `styles/global.css` `@import 'tailwindcss'` (l.1), `@source "../components"` (l.21), `@theme` (l.164-256), 13 `@apply` (l.274-383), `@layer utilities/base` | no — **React 19.2** (`react`, `react-dom`, `@types/react*`), `tsconfig` `jsxImportSource: "react"` | 11 `<Island>` sites: `layouts/default.tsx:101,120`, `pages/m.tsx:65` (`ssrFallback`), `gallery.tsx:55,66` (`ssrFallback={null}`), `modules.tsx:35`, `_mdx-components.tsx:57-93` (5 wrappers) | no | Netlify via `.github/workflows/main-deploy.yml` (`pnpm run build:with-docs` → `dist/`), PR previews via `pr-checks.yml` |
| repo root `components/` (81 tsx, incl. `article/` 18, `modal/` 8, `icons/` 11), `hooks/` (10), `utils/`, `data/`, `types/` | consumed by `zfb-app` through `@/*` → `../*` paths (`zfb-app/tsconfig.json:20-30`) | none | utilities live here (816 candidates needing tokens, see audit) | React 19: 48 `'use client'` components + 2 hooks; `@headlessui/react` (2 files), `@heroicons/react` (3 files), `react-dom` `createPortal` (1) | component bodies of every island above | no | n/a |
| `doc/` (Docusaurus 3.9, React 19) | none | none | none | React (Docusaurus's own) | n/a | no | built into `dist/doc/` by `build:with-docs` — **unaffected by zfb 3** |
| `sub-packages/md-formatter` | none | none | none | no | n/a | no | tooling only — unaffected |

Live-or-abandoned evidence: `pnpm run build` is wired into both workflows and the Netlify site is the public configurator; the last 15 commits (2026-05-29) are the zfb cut-over; open issues are #94 (stale e2e specs, agent-found), #50/#51 (product content). Nothing suggests retirement.

## Sequencing and blockers

1. **No upstream blocker.** No preset package is involved; `@takazudo/zfb@latest` is 3.2.0 (published 2026-10-04 15:30 UTC; https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/). The audit below was run with the 3.1.0 binary earlier the same day — 3.2.0 adds `file:line:col` diagnostics, ZW014 warnings and `--json`, so a re-run will look different but report the same inventory.
2. **Relocate the zfb project to the repo root first** (can be done now, on the pinned next.16 binary, as its own PR). Reason: https://zfb.takazudomodular.com/changelog/zfb/v0.1.0-next.90/ — "A relative import (`./` or `../`) that resolves above your project root … now hard-fails with `Could not resolve`". Every page/layout in `zfb-app/` imports `@/components/*` = `../components/*`. The config comment at `zfb-app/zfb.config.ts:4-9` says the only reason for the subdirectory was Next's `pages/` collision, and Next was deleted in commit 93d9f19.
3. **Absorb the 2.x breaking notes** that lie between next.16 and 2.22.1 (none of them touch this app's config keys, but check): next.25 (`markdown.features.admonitionsPreset` removed — not used here), next.38 (`linkValidation.allowExternal` removed — not used), next.90/next.91 (above-root imports — **applies**), 2.0.0 (`githubAutolinks` removed — not used). Pages: https://zfb.takazudomodular.com/changelog/zfb/v0.1.0-next.25/ … /v2.0.0/.
4. **Then the v3 cut-over proper** (config → CSS/tokens → components → tests/CI), following https://zfb.takazudomodular.com/guides/migrating-to-v3/. There is no React compatibility layer: "The runtime does not promise React or Preact compatibility, a compatibility wrapper, or mixed ownership" (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/).
5. **Nothing here waits on an unreleased zfb.** The two conveniences this guide leans on — `wind.utilities.placement: "before-authored"` and `zfb wind audit --json` / `zfb wind explain --stdin --json` — shipped in 3.2.0 (https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/), as did `wind.strict`/ZW014 and `wind.sources`.

## Required changes

### 1. Dependencies, config, tsconfig, env

- `zfb-app/package.json:12-21`: bump `@takazudo/zfb` and `@takazudo/zfb-runtime` to `3.2.0`; remove `react`, `react-dom`, `@types/react`, `@types/react-dom`. One lockfile covers the whole workspace (`pnpm-workspace.yaml` lists `.`, `doc`, `sub-packages/*`, `zfb-app`; `doc/` has no lockfile of its own) — regenerate the root `pnpm-lock.yaml` once after the removals. Root `package.json:46-80`: remove `react`, `react-dom`, `@types/react*`, `@headlessui/react`, `@heroicons/react`, `@mdx-js/loader`, `@mdx-js/react` (only `doc/` needs MDX-for-React and it has its own `package.json`), `@testing-library/react`, `eslint-plugin-react`, `eslint-plugin-react-hooks`, `eslint-plugin-react-refresh` (their rules misfire on `class=`/`on:click`). Keep `blurhash` (framework-free), `@testing-library/jest-dom`, `jsdom`, `vitest`, `playwright`.
- `pnpm-workspace.yaml:8`: drop `'@tailwindcss/oxide'` from `onlyBuiltDependencies` (nothing pulls Tailwind any more).
- `zfb-app/zfb.config.ts:15,19-21`: delete `framework: 'react'`, replace `tailwind: { enabled: true }` with a `wind` block (§2). Both leftover keys are hard config errors in v3 (https://zfb.takazudomodular.com/guides/migrating-to-v3/#configuration). `outDir`, `publicDir`, `site`, `collections` are unchanged keys (https://zfb.takazudomodular.com/api/define-config/).
- `zfb-app/tsconfig.json:15-16`: `"jsx": "react-jsx"` stays, `"jsxImportSource"` → `"@takazudo/zfb/zudo-react"`. Root `tsconfig.json:14` has `"jsx": "preserve"` and excludes `zfb-app/**` — after the relocation there is one tsconfig; give it the same two settings. The `~/*` and `@/*` aliases collapse to one.
- Add a `zfb-shim.d.ts` only if you keep the bare `zfb/config` import; the docs' shape is a one-line re-export, not a hand copy (https://zfb.takazudomodular.com/api/define-config/#typing-the-zfbconfig-import). The current scoped import `from '@takazudo/zfb/config'` (`zfb-app/zfb.config.ts:1`) resolves against the published `./config` export and needs no shim.
- Env: no `ZFB_TAILWIND_BIN` / `ZFB_TAILWIND_OXIDE_WARMUP` anywhere (`grep -rn ZFB_TAILWIND .github scripts package.json` → 0). Nothing to remove.

### 2. CSS and utilities

Audit facts (commands in §Step-by-step). Run 1, project root `zfb-app/` (what the recipe scans): 0 unrecognized, 6 dead (`pt-vgap-md`, `pb-vgap-lg`, `-mt-vgap-md`, `py-8`, `lg:w-[400px]`, `xl:w-[600px]`), 269 info diagnostics — 126 ZW001 + 81 ZW005 + 58 ZW002 that are **string-literal noise** from blurhash hashes and CDN URLs in `zfb-app/components/panel-acrylic-list.tsx` (45), `panel-printed-list.tsx` (69) and `content/case-models.mdx` (61). That run cannot see the real utility inventory because `components/` is outside the project root. Run 2, a supplementary audit from the **repo root** (temporary `zfb.config.ts`, removed afterwards): **45 unrecognized, 816 dead (702 ZW006 missing tokens + 114 ZW002 unconfigured variants), 242 unique dead candidates, 25 ZW013 conflicts, 33 dynamic constructions, 1,391 diagnostics incl. 38 ZW004 "recognized utility family is unsupported", 2 ZW004 arbitrary-selector variants, 1 ZW004 arbitrary property.** Heaviest files: `components/background-color-picker.tsx` (54 dead), `controls-sidebar.tsx` (48), `panel-selector.tsx` (41), `article/article-image-dialog.tsx` (31), `article/article-grid-image-list.tsx` (30), `color-picker.tsx` (28), `modal/color-selector-modal.tsx` (27), `error-boundary.tsx` (27), `modal/order-info-modal.tsx` (26), `top-nav-grid.tsx` (25).

**a. Directives to delete** (all ZW009 in v3, even under `wind: false`; https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives): `zfb-app/styles/global.css:1` `@import 'tailwindcss'`; `:21` `@source "../components"`; `:164-256` `@theme`; 13 `@apply` lines at `:274,277,278,282,283,287,288,348,352,358,376,380,383`. Keep everything else in the file as authored CSS (the `@font-face` block, `:root` `--zd-*` variables, `.loader`, keyframes, `@layer base/utilities` rule bodies with the `@apply`s expanded by hand).

**b. Tokens to declare** — the `@theme` block maps one-to-one onto `wind.tokens` (families per https://zfb.takazudomodular.com/zudo-wind/tokens/):

```ts
// zfb.config.ts (after relocation to the repo root)
import { defineConfig } from '@takazudo/zfb/config';

export default defineConfig({
  outDir: 'dist',
  publicDir: 'public',
  site: 'https://panels.takazudomodular.com',
  collections: [{ name: 'content', path: 'content' }],
  wind: {
    spec: 1,
    reset: 'owned-v1',                     // nearest to the preflight the site was built on; see §Risks
    defaultTransitionTimingFunction: 'cubic-bezier(0.4, 0, 0.2, 1)',
    tokens: {
      spacingUnit: '0.25rem',              // p-2, px-4, mb-4, w-5, h-5, top-2, right-4 …
      spacing: {
        '1px': '1px',
        'hgap-2xs': 'var(--zd-spacing-hgap-2xs)', 'hgap-xs': 'var(--zd-spacing-hgap-xs)',
        'hgap-sm': 'var(--zd-spacing-hgap-sm)',   'hgap-md': 'var(--zd-spacing-hgap-md)',
        'hgap-md-x2': 'var(--zd-spacing-hgap-md-x2)', 'hgap-lg': 'var(--zd-spacing-hgap-lg)',
        'hgap-lg-x2': 'var(--zd-spacing-hgap-lg-x2)', 'hgap-xl': 'var(--zd-spacing-hgap-xl)',
        'hgap-2xl': 'var(--zd-spacing-hgap-2xl)',
        'vgap-2xs': 'var(--zd-spacing-vgap-2xs)', 'vgap-xs': 'var(--zd-spacing-vgap-xs)',
        'vgap-sm': 'var(--zd-spacing-vgap-sm)',   'vgap-md': 'var(--zd-spacing-vgap-md)',
        'vgap-lg': 'var(--zd-spacing-vgap-lg)',   'vgap-xl': 'var(--zd-spacing-vgap-xl)',
        'vgap-2xl': 'var(--zd-spacing-vgap-2xl)',
      },
      colors: {
        'zd-black': 'var(--zd-color-black)', 'zd-white': 'var(--zd-color-white)',
        'zd-link': 'var(--zd-color-link)',   'zd-active': 'var(--zd-color-active)',
        'zd-gray': 'var(--zd-color-gray)',   'zd-gray2': 'var(--zd-color-gray2)',
        'zd-strong': 'var(--zd-color-strong)', 'zd-notify': 'var(--zd-color-notify)',
        'zd-error': 'var(--zd-color-error)', 'zd-price': 'var(--zd-color-price)',
        debug: '#ff0000',
        // Tailwind default palette entries the components still use (no implicit palette in v3):
        white: '#ffffff', black: '#000000', 'gray-300': '#d1d5db', 'orange-600': '#ea580c',
      },
      fontFamilies: {
        noto: "'Noto Sans', ui-sans-serif, system-ui, sans-serif",
        futura: "'Futura', 'Jost', 'Century Gothic', 'Noto Sans', sans-serif",
        sans: "'Helvetica', sans-serif",
        mono: "'Menlo', 'Monaco', 'Consolas', 'Liberation Mono', 'Courier New', monospace",
      },
      fontSizes: { xs: 'var(--zd-font-xs-size)', sm: 'var(--zd-font-sm-size)', base: 'var(--zd-font-base-size)',
        lg: 'var(--zd-font-lg-size)', xl: 'var(--zd-font-xl-size)', '2xl': 'var(--zd-font-2xl-size)',
        '3xl': 'var(--zd-font-3xl-size)', '4xl': 'var(--zd-font-4xl-size)', '5xl': 'var(--zd-font-5xl-size)' },
        // pair each with its --zd-font-*-lineHeight using the fontSizes paired-leading form (tokens doc)
      fontWeights: { light: '300', normal: '400', medium: '500', semibold: '600', bold: '700' },
      lineHeights: { none: 'var(--zd-lineHeight-none)', tight: 'var(--zd-lineHeight-tight)', snug: 'var(--zd-lineHeight-snug)',
        normal: 'var(--zd-lineHeight-normal)', relaxed: 'var(--zd-lineHeight-relaxed)', loose: 'var(--zd-lineHeight-loose)' },
      radii: { default: '0.25rem', xs: '0.125rem', sm: '0.125rem', md: '0.375rem', lg: '0.5rem', full: '9999px' },
      shadows: { xl: '0 20px 25px -5px rgb(0 0 0 / 0.1), 0 8px 10px -6px rgb(0 0 0 / 0.1)' },
    },
    breakpoints: { sm: { minWidthPx: 580 }, md: { minWidthPx: 740 }, lg: { minWidthPx: 980 },
      xl: { minWidthPx: 1280 }, '2xl': { minWidthPx: 1630 }, '3xl': { minWidthPx: 1800 } },
    authoredClasses: { 'text-shadow-md': true, 'text-shadow-lg': true, 'text-shadow-none': true },
  },
});
```

Top dead candidates that this config resolves: `text-zd-white` 45, `text-sm` 33, `border-zd-gray` 29, `text-zd-gray` 26, `px-hgap-sm` 19, `text-white` 18, `rounded` 18, `border-zd-white` 18, `rounded-lg` 16, `font-bold` 16, `py-vgap-sm` 12, `font-medium` 12, `bg-zd-black` 12, `w-5`/`h-5` 8 each, `space-y-vgap-xs` 8, `shadow-xl` 5, `bg-zd-black/70` 5 (opacity modifier is supported, W10). Unconfigured variants: `lg:` 55, `md:` 42, `xl:` 9, `2xl:` 5, `sm:` 3 → the `breakpoints` block above. Font-size tokens with paired leading replace the `--text-*--line-height` pairs; the `@theme` `--border-width-*` scale has no token family — bare `border-2`/`border-4` are numeric in v1, so check `zfb wind explain border-3` before assuming `border-3`/`border-5`/`border-10` exist. `text-shadow-md` starts with the `text` root, so the authored `.text-shadow-*` classes (`global.css:261-269`) must be reserved via `authoredClasses` or they are validated as colour utilities (https://zfb.takazudomodular.com/guides/migrating-to-v3/#stylesheets).

**c. Candidates v1 does not implement → authored CSS** (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#what-is-not-supported-and-what-to-do-instead): `backdrop-blur` 6 / `backdrop-blur-xl` 3 / `backdrop-blur-md` 1 (modals, header), `invert` 2 + `brightness-0` 2 (icons), `appearance-none` 2 (range sliders — a ZW014 warning since 3.2.0), `container` 1 (`model-section-body.tsx`), `float-right` 2 (`article/img-float-right.tsx:21` as `lg:float-right`, `article/grid-images-and-notes.tsx:46`), `clear-both` 1 (`article/h2.tsx:17`), `flow-root` 1 (`article/h3.tsx:15`), `break-keep` 1 (`mobile-menu-drawer.tsx:55`), `will-change-[transform]` 1 (`article/h2.tsx:21`), gradient stops `from-zd-black/70` + `to-zd-black` (`zd-gradient-*` territory), `prose`/`prose-gray`/`dark:prose-invert` (`model-section-body.tsx:29`; no typography plugin is installed — these were already dead; delete), 38 ZW004 "recognized utility family is unsupported" — `ring-*` (`ring-2` 8, `ring-zd-link` 4, `ring-zd-link/20` 3, `ring-white` 3, `ring-offset-*` 5, `ring-3` 2), `scale-*` (`scale-100` 4, `scale-95` 3, `scale-75`, `scale-50`) and `transform` 12 — concentrated in `components/model-gallery.tsx` (12), `gallery-thumbnail-grid.tsx` (6), `mobile-menu-toggle.tsx` (4) and two each in `preset-selector.tsx`, `panel-selector.tsx`, `model-selector.tsx`, `case-selector.tsx`, `modal/base-modal.tsx`, `modal/case-selector-modal.tsx`, `modal/preset-selector-modal.tsx` (confirm each with `zfb wind explain <candidate>`; focus rings become authored `outline`/`box-shadow` rules, `scale-*`/`transform` become authored `scale`/`transform` declarations), 2 arbitrary-selector variants `[&_li]:pt-vgap-xs` at `components/article/ol.tsx:12` and `ul.tsx:12` (write `.article-ol > li { padding-top: var(--zw-spacing-vgap-xs) }`), 1 arbitrary property. The 25 ZW013 conflicts (`duration-300` + `transition-opacity`, `border-b` + `border-dashed`) are informational — v1's `transition-*` sets its own 150 ms duration, so keep the explicit `duration-*`.

**d. Placement tie flip.** Tailwind 4 emitted utilities before the authored `@layer base` rules in this file; zudo-wind emits them after authored global CSS by default, so equal-specificity ties (`a { … }` in `@layer base` vs `text-zd-white` on a link) now go to the utility. Authored rules inside `@layer base` lose to every utility under both settings (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties). If a page diff shows regressions, `wind.utilities.placement: "before-authored"` restores Tailwind's order (available since 3.2.0; https://zfb.takazudomodular.com/zudo-wind/configuration/#fields).

**e. `[data-zfb-island-skip-ssr] { height: 100% }`** (`global.css:36`): the marker attribute still exists in v3 (https://zfb.takazudomodular.com/api/island/), so the rule can stay; re-verify on the built `/m/` page.

### 3. Components and islands

Census (repo root `components/`, `hooks/`, `zfb-app/`): 98 tsx (81 in `components/`, 17 in `zfb-app/`) + 10 hooks, ~8,800 LOC of ts/tsx; 55 `'use client'` files; hook call sites (`grep -rnoP '\buseX(<[^()]*>)?\('`): `useState` 51, `useEffect` 51, `useCallback` 36, `useRef` 23, `useMemo` 9 (the bare-identifier counts incl. imports are 81/84/47/37/15), `useSyncExternalStore` 3 (`hooks/use-is-standalone.ts:3,41`), `useId` 2 (`components/pattern-fill.tsx:14`), `createContext`/`useContext` (`components/navigation-context.tsx:3,18,151`), `ReactDOM.createPortal` (`components/mobile-menu-drawer.tsx:4,107`), class component `ErrorBoundary` (`components/error-boundary.tsx:16-27`, reads `process.env.NODE_ENV` at `:30,57`); Headless UI `Dialog`/`DialogPanel` (`mobile-menu-drawer.tsx:5`), `Listbox*`/`Transition` (`panel-selector.tsx:4-10`); `@heroicons/react` (`base-image-dialog.tsx:11`, `article/article-image-dialog.tsx:11`, `panel-selector.tsx:11`); no `dangerouslySetInnerHTML` anywhere (`components/icons/takazudo-logo.tsx:5` only mentions it in a stale comment — the SVG paths are inline JSX); `className` 474, `onClick` 48, `onChange` 4, `onLoad` 8, `onError` 8, `tabIndex` 1, `autoFocus` 2, SVG `strokeWidth=` 7 / `strokeLinecap=` 7, 13 camelCase keys inside 23 `style={{…}}` objects (`backgroundColor` 5, `opacity` 5, `paddingBottom`, `minWidth`, `imageRendering`), `key=` 28 list renders, `type="range"` ×2 (`background-color-picker.tsx:127,157`).

Conversion map (all from https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/ unless noted):

- `useState` → `signal()`; `useMemo` → `computed()`; `useCallback` → plain closures (setup runs once); `useRef` for DOM → `{ current: null }` `Ref` objects; `useEffect([])` → `getScope().onActivate`, `useEffect([deps])` → `getScope().effect`. `hooks/*.ts` become ordinary setup helpers taking `(scope, signals)` ("Custom hooks become functions").
- `useSyncExternalStore` in `use-is-standalone.ts` (`matchMedia('(display-mode: standalone)')`) → the "subscription with cleanup" pattern: `signal(false)` + `onActivate` + `matchMedia` listener. Server output stays deterministic.
- `useId` in `pattern-fill.tsx` → derive the SVG pattern id from props (`caseType`/panel id) or a module counter at setup; there is no `useId`.
- `navigation-context.tsx` (`createContext`) + `zfb-app/components/site-header.tsx:49` / `site-footer.tsx:17` (`NavigationProvider`) → no context in zudo-react ("What does not exist"). Pass the navigation signals (`isNavigating`, `currentPath`) as props from `SiteHeader`/`SiteFooter` down to `NavigationLink`, `MobileMenuDrawer`, `PageLoadingIndicator`. They are already separate islands, so there is no cross-island sharing to replace.
- `mobile-menu-drawer.tsx` (`createPortal` + Headless UI `Dialog`) and `base-modal.tsx`/`modal/*` (5 modals) → native `<dialog>` with `showModal()` from `onActivate`, focus trap via `use-focus-trap.ts` logic; `panel-selector.tsx` `Listbox` → native `<select>` with `modelValue`, or a `<details>`/button list driven by a signal. Headless UI and Heroicons are React-only; the 4 Heroicon imports become inline SVG (the project already has 11 inline icon components under `components/icons/`).
- `error-boundary.tsx` → delete; `pages/m.tsx:29-33` already dropped it for the configurator ("Error boundaries … the runtime does not provide a component boundary"). `process.env.NODE_ENV` is not a zfb define.
- `icons/takazudo-logo.tsx` needs no `rawHtml` (and could not use it — "Raw HTML is unsupported in SVG", https://zfb.takazudomodular.com/zudo-react/components-and-jsx/): flatten its nested `<svg>`-in-`<svg>` and the `{' '}` text children into one `<svg>` with inline `<path>` elements, and drop the `React.FC`/`React.SVGProps` types.
- `className` → `class` (474 sites; `sed`-able); `onClick`/`onChange`/`onLoad`/`onError` → `on:click`/`on:input`/`on:load`/`on:error` with native events; `tabIndex`/`autoFocus`/`strokeWidth`/`strokeLinecap` → HTML/SVG spellings (`tabindex`, `autofocus`, `stroke-width`, `stroke-linecap`); `style={{ backgroundColor }}` → `style={{ 'background-color': … }}` with units on every length (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/).
- `items.map(… key=…)` (28) → `<For each={signal} by={…}>`; `{cond && <X/>}` → `<Show when={…}>` (https://zfb.takazudomodular.com/zudo-react/conditionals-and-lists/).
- Range inputs (`background-color-picker.tsx:127,157`): range models are unsupported (https://zfb.takazudomodular.com/zudo-react/forms/) — keep them uncontrolled with `defaultValue` + `on:input` reading `event.currentTarget.value`.
- Islands: `<Island>` keeps `when`, `ssrFallback` (https://zfb.takazudomodular.com/api/island/#skip-server-rendering). Each island must have exactly one component child with JSON props; the five `_mdx-components.tsx` wrappers already comply. `pages/m.tsx:65` mounts the whole configurator as one skip-SSR island — that stays the right shape (its tree reads `window`/`localStorage` in effects). Hydration adopts server markup or fails closed per island (https://zfb.takazudomodular.com/concepts/islands/); the `GalleryDialogHost` `history.pushState` patch (`zfb-app/components/gallery-dialog-host.tsx:38-51`) is plain DOM code and ports unchanged into `onActivate`.
- `getStaticProps` (9 pages) and `getCollection`/`entry.Content` (6 pages) are unchanged APIs (https://zfb.takazudomodular.com/api/meta-export/, https://zfb.takazudomodular.com/api/get-collection/). Spread `defaultComponents` first in `_mdx-components.tsx` (https://zfb.takazudomodular.com/concepts/mdx-components/).

### 4. md-wasm and other packages

Not used (`grep -rn zfb-md-wasm . → 0`). `sub-packages/md-formatter` is a remark CLI, `doc/` is Docusaurus — neither touches zfb.

### 5. Tests, CI, deploy

- Vitest: `hooks/use-local-storage-color.test.ts:2` uses `renderHook` from `@testing-library/react`; rewrite against `createIslandTest` from `@takazudo/zfb/zudo-react/testing` or test the pure helpers (`getStoredValue`) directly (https://zfb.takazudomodular.com/zudo-react/testing/). `test/setup.ts` (`jest-dom`) and `vitest.config.ts` (jsdom) can stay; add the vitest JSX config from the testing doc.
- Playwright (`tests/*.spec.ts`, 8 files wait on `[data-zfb-island-skip-ssr="Configurator"]`/`"GalleryDialogHost"`): the marker survives in v3, but the skip-SSR mount now replaces the fallback only after setup succeeds, so keep the `… svg` readiness waits. Issue #94 already quarantines 4 stale specs.
- `.github/workflows/pr-checks.yml:40-59` and `main-deploy.yml:30-41`: `pnpm run typecheck` → one tsconfig at the root with the zudo-react `jsxImportSource`; `pnpm run lint` → drop the three React ESLint plugins (`eslint.config.js:5-7,40-45,54`); `pnpm run build` → `zfb build` (still `cp -r zfb-app/dist dist` in root `package.json:8` until the relocation removes the copy). Add `zfb wind audit --fail-on error` as a step (migration checklist item 6). Netlify `publish-dir: ./dist` is unchanged; `build:with-docs` keeps appending `doc/build` as `dist/doc/`.
- `scripts/b4push.sh` / `b4push-quick.sh`: same three commands; add the audit.

## Step-by-step plan

1. **Baseline PR (today, next.16 binary).** Move `zfb-app/{pages,layouts,components,content,styles,zfb.config.ts}` to the repo root (merge `zfb-app/components/*` into `components/`), delete the `public` symlink, collapse `@/*`/`~/*` to one alias, drop the `cp -r zfb-app/dist dist` from `package.json:8`, remove `doc` from root `tsconfig` excludes only if you want it typechecked. Verify: `pnpm build && pnpm test:smoke`. Commit — this is the "working 2.x-shaped baseline" the guide asks for.
2. **Install v3.** `pnpm --filter zfb-app remove react react-dom @types/react @types/react-dom` (or at root after step 1), `pnpm add -D @takazudo/zfb@3.2.0 @takazudo/zfb-runtime@3.2.0`, `pnpm remove @headlessui/react @heroicons/react @mdx-js/loader @mdx-js/react @testing-library/react eslint-plugin-react eslint-plugin-react-hooks eslint-plugin-react-refresh`. `pnpm exec zfb --version` must print `zfb 3.2.0` and an `embedded esbuild` line only.
3. **Config.** Replace `zfb.config.ts` with the §2b block; set `jsxImportSource` in `tsconfig.json`. Run `pnpm exec zfb check` — expect only component type errors from here on.
4. **Stylesheet.** Delete the 16 directive sites in `styles/global.css`; expand the 13 `@apply` bodies by hand; paste the measured preflight-compat block from https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#reset-differences-from-tailwind-preflight into `@layer base` (the site relies on `html { font-family }`, `button { appearance }`, `::-webkit-scrollbar`, `[id] { scroll-margin-top }`). Run `pnpm exec zfb css --input styles/global.css --output /tmp/case.css` and `pnpm exec zfb wind audit --fail-on error`; iterate until the 816 dead candidates are either tokenised or moved to authored CSS.
5. **Components, inside-out.** Order by blast radius: `components/icons/*` (11 pure SVG files: spellings only) → `article/*` (17; mostly `className`) → `navigation-link`/`navigation-context`/`app-header`/`footer`/`mobile-menu-drawer` (header & footer islands) → `gallery-*`/`blurhash*`/`base-image-dialog` → `modal/*` + `panel-selector` (Headless UI replacements) → `configurator` + `controls-sidebar` + `visualization-panel` + `case-visualizer` (623 lines, 19 hook calls in `configurator.tsx`) → `hooks/*`. `grep -rln "@jsxImportSource" components pages layouts` must be empty (it is today).
6. **Tests.** Rewrite `use-local-storage-color.test.ts`; run `pnpm test:run`; run `pnpm exec playwright test tests/e2e.spec.ts` and the 16-case `test:builder` suite locally (CLAUDE.md says it is not in CI).
7. **CI and deploy.** Update `eslint.config.js`, both workflows (add the audit step), `scripts/b4push*.sh`; open a PR to get a Netlify preview; compare `/`, `/m/?c=…`, `/gallery/?id=…`, `/panel/`, `/price/` against production in a browser (checklist item 7).

## Verification checklist

- [ ] `pnpm exec zfb --version` → `zfb 3.2.0`, no Tailwind line.
- [ ] `pnpm exec zfb check` clean; `grep -rn "from 'react'" components hooks pages layouts` → 0; `grep -rn className components pages layouts` → 0.
- [ ] `pnpm exec zfb wind audit --fail-on error` exits 0; the unrecognized list contains only authored names with no utility root — today's run shows `zd-invert-color-link` (11), `loader` (4), `slider` (2), `zd-button-gradient` (2) — and none of the 14 Tailwind-only names from §2c (`backdrop-blur*`, `invert`, `brightness-0`, `appearance-none`, `container`, `float-right`, `clear-both`, `flow-root`, `break-keep`, `will-change-[transform]`, `prose*`, `from-`/`to-zd-black`).
- [ ] `zfb build` emits `dist/m/index.html` with `data-zfb-island-skip-ssr="Configurator"`, `dist/gallery/index.html`, `dist/_zfb/*` CSS without any `@theme`/`@apply` leftovers.
- [ ] Browser diff vs https://panels.takazudomodular.com/: header nav active state, mobile drawer, `/m/` configurator (case switch, panel colour URL round-trip `?c=&p=`), gallery dialog deep link, `/panel/` lazy image grids, fonts (Noto Sans weights 300/400/500/700, Futura buttons), focus outlines (`outline-orange-600`), scrollbar styling.
- [ ] `pnpm test:run` (vitest) and `pnpm test:smoke` green; `test:builder` green locally.
- [ ] Netlify preview from `pr-checks.yml` renders `/doc/` (Docusaurus untouched).

## Risks and open questions

- **Size.** This is a second framework rewrite of a ~8.8k-LOC React app four months after the first; XL is honest. If the product is static for now, freezing on next.16 is cheaper short-term but leaves the site on an unmaintained prerelease binary with the above-root-import footgun already fixed upstream.
- **Reset choice.** `owned-v1` removes list markers and heading sizes; the article components (`components/article/ul.tsx`, `ol.tsx`, `h1-h3.tsx`) style these explicitly, but MDX bodies in `content/*.mdx` may rely on UA defaults. Check `sub`/`sup`, `::placeholder`, `[hidden]` per the migration guide; the measured compat block covers them.
- **Default palette leakage.** `text-white`, `bg-white`, `bg-black`, `text-gray-300`, `hover:bg-white/20`, `outline-orange-600`, `shadow-xl`, `rounded-md/sm/xs` come from Tailwind's default theme; each needs an explicit token or the class silently emits nothing (ZW006 is an error at strict origins, so the build will tell you).
- **Relation-variant specificity.** `group-hover:*` selectors drop from (0,2,0) to (0,1,0); the header/gallery hover states could lose to authored `.zd-invert-color-link:hover` rules. Tracked upstream in Takazudo/zudo-front-builder#3386 (still open; 3.2.0 shipped the placement switch but not a specificity switch); mitigations in https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#relation-variant-specificity.
- **Bundle size.** zudo-react makes no bundle-size promise relative to React (migration guide); the `/m/` island ships the entire configurator — measure `dist/_zfb/*.js` before/after.
- **Open question:** does anything still need `site`-based canonical URLs (`globalThis.__zfb.site`)? The layout emits no canonical `<link>`; the key is harmless but unused.
- **Open question:** `doc/` Docusaurus and the root ESLint/Prettier scripts exclude `zfb-app/**`; after relocation the excludes need rewriting or Prettier will start formatting `content/*.mdx` (currently skipped by `format:md` only).

## References

- Migration guide: https://zfb.takazudomodular.com/guides/migrating-to-v3/
- Tailwind map, reset diff, placement, relation specificity: https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/
- Hooks → signals/scopes: https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/
- Wind config fields, `sources.packageRoots`: https://zfb.takazudomodular.com/zudo-wind/configuration/ · tokens: https://zfb.takazudomodular.com/zudo-wind/tokens/ · diagnostics ZW001–ZW014: https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/ · scan roots: https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/
- Components/JSX spellings: https://zfb.takazudomodular.com/zudo-react/components-and-jsx/ · forms: https://zfb.takazudomodular.com/zudo-react/forms/ · lists/conditionals: https://zfb.takazudomodular.com/zudo-react/conditionals-and-lists/ · testing: https://zfb.takazudomodular.com/zudo-react/testing/
- Islands and `<Island>`: https://zfb.takazudomodular.com/concepts/islands/ · https://zfb.takazudomodular.com/api/island/
- Page module exports (`getStaticProps`, `paths`): https://zfb.takazudomodular.com/api/meta-export/ · collections: https://zfb.takazudomodular.com/api/get-collection/ · MDX component map: https://zfb.takazudomodular.com/concepts/mdx-components/
- Config: https://zfb.takazudomodular.com/api/define-config/ · CLI (`zfb css`, `zfb wind explain|audit`, `zfb check`, `zfb preview`): https://zfb.takazudomodular.com/api/cli/
- Breaking notes between next.16 and 2.22.1: https://zfb.takazudomodular.com/changelog/zfb/v0.1.0-next.25/ · /v0.1.0-next.38/ · /v0.1.0-next.90/ · /v0.1.0-next.91/ · /v2.0.0/ · v3: /v3.0.0/ · /v3.1.0/ · https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/ (2026-10-04: `wind.sources`, `wind.strict`/ZW014, `wind.utilities.placement`, `--json`, `file:line:col` diagnostics, six new utilities)
- Audit commands used (zfb 3.1.0 binary, no `pnpm install`; its `file:NNN` positions are byte offsets, not lines — 3.2.0 prints `file:line:col`):
  `cp zfb-app/zfb.config.ts /tmp/bak && printf 'import { defineConfig } from "zfb/config";\nexport default defineConfig({ wind: { spec: 1 } });\n' > zfb-app/zfb.config.ts && zfb wind audit --project-root zfb-app; cp /tmp/bak zfb-app/zfb.config.ts` (run 1) and the same with a temporary `zfb.config.ts` at the repo root (run 2, file removed afterwards; `git status --short` clean).
