# zfb v3 migration guide: Takazudo/takazudomodular-manuals

Generated 2026-10-04 by an automated diagnosis of `main` @ `3324165`. Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**Ready** (root app) / **blocked** (`doc/`). Effort **L**. The root is a direct zfb 2.20.2 + Cloudflare-adapter host with no preset dependency, so its migration can start today on zfb 3.1.0: it owns a 742-line Tailwind v4 CSS-first design system (`@theme` with ~250 token lines, 9 `@utility`, 15 `@apply`), 23 Preact component/helper files (4,561 lines incl. tests) using 180 hook calls, a mega-island whose SSR body is passed as island **children** and re-injected with `dangerouslySetInnerHTML` (both rejected by zfb 3), 136 `className=` props, 2 controlled form controls and 5 Preact-rendering Vitest suites. `zfb wind audit` on 3.1.0 reports 30 class-position errors (29 missing tokens, 1 unconfigured `lg:` breakpoint) plus 223 token-missing literals inside `ctl()` template strings. The `doc/` site is a `create-zudo-doc@5.27.0` single-package consumer and must wait for zudo-doc 6.0.0 (zudolab/zudo-doc#4430 / #4477); its host-owned surface is small (settings, z-index tokens, 10 content files). Start gate for the root: none. Start gate for `doc/`: zudo-doc 6.0.0.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `/` (`takazudomodular-manuals`, manual viewer; `zfb.config.ts`) | `@takazudo/zfb` 2.20.2, `zfb-runtime` 2.20.2, `zfb-adapter-cloudflare` 2.20.2 | none | `@import 'tailwindcss'` in `styles/global.css:16`; `@theme` (124–378), 9 `@utility` (383–439), 15 `@apply` (397–489); leftover Next.js-era `tailwind.config.cjs` + `postcss.config.cjs`; devDeps `tailwindcss` ^4.2.2, `@tailwindcss/postcss` ^4.2.2 | `preact` ^10.22.0, `preact-render-to-string` ^6.5.0, `@preact/preset-vite`, `@testing-library/preact`; `tsconfig.json` `jsxImportSource: "preact"` + `react*` → `preact/compat` paths; `zfb.config.ts:11` `framework: 'preact'`; 20 files import `preact`/`preact/hooks`/`preact/compat` | 3 `<Island>` sites: `pages/[manualId]/page/[pageNum].tsx:116` (`when="load"`, mega-island `ManualApp` **with children**), `pages/[manualId]/index.tsx:148,151` (`when="idle"`, `LandingLangIsland`, `LandingSearchIsland`); `'use client'` in 5 files | not used | Cloudflare Workers via adapter: `wrangler.toml` `main = "./dist/_worker.js"`, `nodejs_compat`, assets `./dist`, `manuals.takazudomodular.com`; `scripts/zfb-finalize-build.js` copies PDFs + writes `.assetsignore`; `main-deploy.yml`, `main-e2e.yml`, `pr-quality-checks.yml`; `wrangler` 4.85.0 |
| `doc/` (`zmanuals-doc`; `doc/zfb.config.ts`) | `@takazudo/zfb` 2.20.2, `zfb-md-wasm` 2.20.2, `zfb-runtime` 2.20.2, `zfb-adapter-cloudflare` 2.20.2 (adapter dep present, config emits static output) | `@takazudo/zudo-doc` ^5.27.0, `zudo-doc-history-server` ^5.27.0, `create-zudo-doc` ^5.27.0 (dev); `@takazudo/zdtp` 0.8.2 declared but `designTokenPanel: false` (`src/config/settings.ts:82`) | scaffold `src/styles/global.css` (416 lines): 2 Tailwind imports (12–13), 5 `@source` (64–68), 2 `@theme` blocks (90–257, 273–287 generated z-index block); devDeps `@tailwindcss/vite`, `tailwindcss` ^4.2.0 (unused by zfb) | `preact` ^10.29.1, `preact-render-to-string` ^6.6.6, `@types/react` ^19.2.0; `tsconfig.json` `jsx: "preserve"` + `react*` → `preact/compat` paths; 26 template-derived files import `preact` | zudo-doc islands via template copies (`pages/lib/_body-end-islands.tsx`, `src/components/image-enlarge.tsx`, `doc-history.tsx` with `'use client'`) | pinned, not called directly | static Cloudflare Workers assets: `doc/wrangler.toml` (no `main`), `doc-manuals.takazudomodular.com`; `doc-deploy.yml`, `doc-pr-checks.yml` |
| `scripts/md-formatter/` (workspace member) | — | — | — | — | — | — | internal tool, unaffected |

### Root census (commands: `grep -rnE`, `zfb wind audit`, `zfb wind explain` on 3.1.0)

- Hooks (`components/`, `pages/`, `layouts/`, non-test): `useCallback` 44, `useRef` 42, `useEffect` 41, `useState` 35, `useMemo` 9, `useLayoutEffect` 7, `forwardRef` + `useImperativeHandle` 2 (`components/zfb/scroll-viewer.tsx:1,72,233`). Heaviest files: `manual-app.tsx` (29 hook calls), `page-viewer.tsx` (29), `scroll-viewer.tsx` (25), `search-dialog.tsx` (28), `use-intersection-pages.ts` (14), `thumbs-modal.tsx` (12).
- Dialect: `className=` 136; React-style listeners `onClick=` 18, `onInput=` 1, `onChange=` 1, `onKeyDown=` 1, `onMouseMove/Enter/Leave=` 3, `onLoad=` 1, `onError=` 1 (the other `on*=` hits are component callback props); `dangerouslySetInnerHTML` at `components/zfb/prose-content.tsx:37`, `components/zfb/manual-app.tsx:301`, `layouts/default.tsx:114` (`<script>`); `charSet` (`layouts/default.tsx:103`), `crossOrigin` (111).
- Forms: `components/zfb/search-dialog.tsx:393–397` `<input value={query} onInput=…>` (controlled text); `components/zfb/page-navigation.tsx:93–106` `<select value={currentPage} onChange=…>` with `.map()`-generated numeric `<option value={page}>`; `<dialog ref>` + `showModal()/close()` in `search-dialog.tsx:218–226,380`; a `role="dialog"` div with a manual focus trap in `thumbs-modal.tsx:96–145`.
- Island transport: `pages/[manualId]/page/[pageNum].tsx:116–129` wraps `<ManualApp …><ViewerShell …/></ManualApp>` in `<Island when="load">`; `manual-app.tsx:59–65,251–302` reads `document.querySelector('[data-manual-body]').innerHTML` during the first client render and re-injects it via `dangerouslySetInnerHTML` so Preact's hydrate skips the diff.
- Audit (`zfb` 3.1.0, temporary `wind: { spec: 1 }`): exit 0, 30 **error**-severity diagnostics: ZW006 ×29 at class positions in `page-viewer.tsx` (11), `viewer-shell.tsx` (8), `scroll-viewer.tsx` (6), `sidebar-thumbs.tsx` (2), `thumbs-modal.tsx` (2), `prose-content.tsx` (1) for `text-sm`, `text-lg`, `text-zd-gray6`, `text-zd-gray`, `text-zd-red`, `font-bold`, `font-futura`, `mb-vgap-xs`, `ml-hgap-sm`, `p-hgap-md`, `bg-white`, `bg-zd-black`, `bg-zd-gray2`, `hover:bg-white/10`; ZW002 ×1 `lg:-mx-hgap-2xl` (`pages/[manualId]/index.tsx:130`). `auditInfo`: ZW006 ×223, ZW005 ×160, ZW001 ×103, ZW012 ×46, ZW002 ×30, ZW004 ×15, ZW013 ×1, ZW003 ×1. Unrecognized (ordinary) classes: `page-image-loader`, `zoom-lens`, `zoom-panel`, `zd-prose`. Conflict: `duration-300` vs `transition-opacity` (`page-viewer.tsx:313`).
- Why most findings are `auditInfo`: every class list is built with `ctl(\`…\`)` (93 call sites across 18 files; `components/zfb/ctl.ts` is a local whitespace-collapsing re-implementation). A template literal passed to a function is a lower-confidence literal, not a class position, so on zfb 3 a misspelt or token-less utility inside `ctl()` silently emits nothing instead of failing the build (https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/#class-positions-and-confidence).
- `zfb wind explain` (3.1.0, with the project's tokens declared): resolved: `-translate-x-1/2`, `list-disc`, `list-inside`, `italic`, `outline-2`, `outline-offset-4`, `no-underline`, `shadow-lg`, `h-screen`, `pt-[60px]`, `max-w-[80%]`, `text-[11px]`, `bg-zd-red/90`, `text-zd-white/70`, `hover:bg-zd-white/10`, `border-3`, `rounded-xs`, `z-50`, `z-[101]`, `duration-300`, `transition-opacity`, `cursor-not-allowed`, `pointer-events-none`, `inset-0`, `shrink-0`, `min-w-0`, `flex-1`, `font-futura`, `text-6xl`, `sm:text-2xl`, `lg:-mx-hgap-2xl`, `active:bg-zd-gray5`, `focus:outline-none`, `focus-within:opacity-100`, `disabled:opacity-50`, `group-hover:opacity-100`. **Rejected (ZW004)**: `transform` (`page-viewer.tsx:22`, `tooltip-styles.ts`), `ring-1`/`focus-visible:ring-1`/`ring-zd-white` (`language-toggle.tsx:34`), `scale-[1.03]`, `[mask-image:url(...)]` + 3 sibling arbitrary properties (`layouts/default.tsx:79–82`). **Rejected (ZW005)**: `shadow-zd-white/5` (`layouts/default.tsx:47`, shadow colour with opacity), `h-[calc(100vh-60px)]` (`sidebar-thumbs.tsx:12`; spaces must be `_`: `h-[calc(100vh_-_60px)]`). **Silently ordinary on 3.1.0, ZW014 warning on the next release**: `container` (22 uses), `after:content-['']` (4 `after:` uses). **Next-release utilities, ZW006 on 3.1.0**: `leading-none` (declare `lineHeights.none` as a token instead), `underline-offset-4` (only inside `@apply` at `global.css:456`, so it disappears with the `@apply` rewrite).
- Variants used: `hover:` 16, `sm:` 10, `focus:` 8, `md:` 5, `lg:` 5, `after:` 4, `focus-within:` 3, `disabled:` 3, `active:` 3, `group-hover:` 2 (11 bare `group` markers), `focus-visible:` 2, `placeholder:` 1. No `dark:` (dark theme is enforced), no `aria-*`/`data-[…]` variants, no named `group/…`.
- Tests: Vitest (`vitest.config.ts`: jsdom, `@preact/preset-vite`, `react*` aliases) with 5 Preact-rendering suites (`components/__tests__/page-viewer.test.tsx` 10 tests, `components/zfb/__tests__/language-toggle.test.tsx` 7, `scroll-viewer.test.tsx` 1, `search-dialog.test.tsx` 9, `search-trigger.test.tsx` 7) and 2 pure suites (`lang.test.ts` 12, `zoom.test.ts` 7); `lib/*.test.ts` (14) and `scripts/*.test.ts` (117) are engine-free. Playwright: 54 tests in 5 specs against the served `dist/` (`e2e/oxi-one-mk2-viewer.spec.ts` 28, `viewer-ui` 15, `scroll-mode` 8, `language-toggle` 2, `all-pages` 1).

### `doc/` census

- Audit (3.1.0): 31 error-severity, all in template-derived files (`pages/index.tsx` 29, `src/components/tree-nav-shared.tsx` 2): ZW006 ×27 (`text-fg`, `text-muted`, `hover:text-accent`, `gap-hsp-md`, `mb-vsp-*`, `border-muted`, …), ZW002 ×4 (`lg:` unconfigured). 0 findings in project-owned code. `.template-drift-allowlist` lists 82 scaffold files that differ from the template only by Prettier style; `.zudo-doc.json` = `{ "packageVersion": "5.27.0", "ejected": {} }`.
- Project-owned: `src/config/settings.ts` (115 lines), `src/config/z-index-tokens.ts` (feeds the generated `@theme` block at `global.css:269–288` via `pnpm gen:z-index`), `scripts/{check-links.js,check-pin-parity.mjs,check-wrangler-pin.mjs,check-template-drift.sh,setup-doc-skill.sh,run-b4push.sh}`, 10 content files (`<CategoryNav` ×4, 0 `class=`), 7 public files, `wrangler.toml`, `.htmlvalidate.json`, `setup-preset.json` (features: search, imageEnlarge, claudeResources, sidebarResizer, sidebarToggle, docHistory, bodyFootUtil).

## Sequencing and blockers

1. **Root app: no blocker.** zfb 3.1.0 (npm `latest`) has everything the root needs. Start on a branch (`zfb3/root`) now; the 2.x `main` keeps deploying meanwhile.
2. **Root, next zfb release (optional but useful):** ZW014 warnings for `container`/`content-*` and `wind.strict` (#3365), `file:line:col` + `--json` audit output (#3370), `wind.utilities.placement: "before-authored"` to keep Tailwind's tie order (#3386), `leading-none`/`underline-offset-*` utilities, and the adapter fix for dangling Worker source-map references (#3480). None blocks the migration; `container` must be replaced by authored CSS either way.
3. **`doc/`: blocked on zudo-doc 6.0.0** (zudolab/zudo-doc#4430, root PR #4477, consumer guide #4473), which itself needs the next zfb release (fixes #3569/#3570 are merged, unreleased). The two zfb project dirs can migrate independently: they have separate `package.json`s, lockfile entries and workflows (`doc-*.yml` are path-filtered to `doc/**`). Keep `doc/` on 2.20.2 + zudo-doc 5.27.0 while the root moves; `check:pin-parity` only compares pins inside `doc/package.json`.
4. **`doc/` later:** re-scaffold with `create-zudo-doc@6` using `doc/setup-preset.json`, carry `settings.ts`, `z-index-tokens.ts`, content, public and scripts; rewrite `doc/src/content/docs/design-system/design-system.md` (320 lines, "the authoritative reference for the custom Tailwind CSS v4 configuration") after the root's token table exists.

## Required changes

### 1. Dependencies, config, tsconfig, env

- `zfb.config.ts`: delete `framework: 'preact'` (line 11) and replace `tailwind: { enabled: true }` (line 15) with a `wind` block (section 2). Keep `base`, `outDir`, `publicDir`, `site`, `port`, `adapter`. Why: https://zfb.takazudomodular.com/guides/migrating-to-v3/#configuration.
- `package.json` (root): bump `@takazudo/zfb`, `@takazudo/zfb-runtime`, `@takazudo/zfb-adapter-cloudflare` to 3.1.0 together; remove `preact`, `preact-render-to-string`, `@preact/preset-vite`, `@testing-library/preact`, `tailwindcss`, `@tailwindcss/postcss`, `eslint-plugin-react-hooks` (its rules have no meaning without hooks) and `@netlify/classnames-template-literals` (already unused: `components/zfb/ctl.ts` re-implements it). Keep `minisearch`, `wrangler` 4.85.0 (zfb's adapter preview checks a minimum wrangler, https://zfb.takazudomodular.com/api/cli/#zfb-preview). Drop the `"preact"` keyword. Why: https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands.
- Delete `tailwind.config.cjs` and `postcss.config.cjs` (Next.js-era; `content` paths point at `./app/**` which no longer exists; zfb never read them).
- `tsconfig.json`: `"jsxImportSource": "preact"` (line 18) → `"@takazudo/zfb/zudo-react"`; delete the four `react*` path aliases (lines 25–28). Add `components/zfb-shim.d.ts` with `declare module "zfb/config" { export * from "@takazudo/zfb/config"; }` only if `zfb.config.ts` switches to the bare `zfb/config` import; today it imports `@takazudo/zfb/config` directly. Why: https://zfb.takazudomodular.com/api/define-config/#typing-the-zfbconfig-import.
- `pnpm-workspace.yaml`: `publicHoistPattern` entries `react`, `react-dom`, `next*`, `@next/*` are legacy; prune after the migration builds (the comment already marks them safe to prune once verified). `shamefullyHoist: true` can stay.
- `eslint.config.js`: drop `eslint-plugin-react-hooks`; review `eslint-plugin-react` rules that assume `className`/`htmlFor` (e.g. `react/no-unknown-property`) because zudo-react requires `class`/`for` (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/#use-html-and-svg-names).
- No `ZFB_TAILWIND_BIN`/`ZFB_TAILWIND_OXIDE_WARMUP` in scripts or workflows (grep: 0). `build-zfb/action.yml` sets `NODE_ENV=production` and `NODE_OPTIONS=--max-old-space-size=4096`; both stay.
- Docs: `CLAUDE.md` (11 Tailwind/Preact mentions), `README.md` (6), `.claude/CLAUDE.md` (3), `scripts/CLAUDE.md` (1), `doc/CLAUDE.md` (3) describe "Preact islands" and "Tailwind CSS v4 / Zudo Design System"; rewrite the stack sections and the "Styling" rule ("NEVER use inline styles — Always use Tailwind CSS classes").

### 2. CSS and utilities

Everything in `styles/global.css` that is a Tailwind directive is a ZW009 build error on zfb 3, even under `wind: false`, and the audit names each site (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives):

- Line 16 `@import 'tailwindcss'` → delete. Line 21 `@import './prose.css'` stays (plain CSS import; `prose.css` has 0 directives and 70 `.zd-prose` rules).
- `@theme` block (124–378) → `wind.tokens` in `zfb.config.ts`. The `--*: initial` resets (128–145) need no equivalent: v3 has no implicit palette, spacing or scale (https://zfb.takazudomodular.com/zudo-wind/tokens/#no-implicit-theme). Mapping, keeping the `--zd-*` Tier-1 custom properties in authored CSS and pointing tokens at them:

```ts
import { defineConfig } from '@takazudo/zfb/config';
import { ZFB_BASE } from './lib/base-path.js';

export default defineConfig({
  base: ZFB_BASE,
  outDir: 'dist',
  publicDir: 'public',
  site: 'https://manuals.takazudomodular.com',
  port: 3300,
  adapter: '@takazudo/zfb-adapter-cloudflare',
  wind: {
    spec: 1,
    reset: 'owned-v1',
    // Tailwind v4's default; keep it so transition-* timing does not change.
    defaultTransitionTimingFunction: 'cubic-bezier(0.4, 0, 0.2, 1)',
    tokens: {
      // No spacingUnit: the old `--spacing-*: initial` reset already disabled numeric spacing,
      // so only the named hgap/vgap scale exists. If the audit reports "missing spacing unit",
      // a numeric candidate (p-4 …) was dead in 2.x too; fix the markup, not the config.
      colors: {
        'zd-black': 'var(--zd-color-black)', 'zd-white': 'var(--zd-color-white)',
        'zd-gray': 'var(--zd-color-gray)', 'zd-gray2': 'var(--zd-color-gray2)',
        'zd-gray3': 'var(--zd-color-gray3)', 'zd-gray4': 'var(--zd-color-gray4)',
        'zd-gray5': 'var(--zd-color-gray5)', 'zd-gray6': 'var(--zd-color-gray6)',
        'zd-gray7': 'var(--zd-color-gray7)', 'zd-overlay': 'var(--zd-color-overlay)',
        'zd-link': 'var(--zd-color-link)', 'zd-active': 'var(--zd-color-active)',
        'zd-outline': 'var(--zd-color-outline)', 'zd-strong': 'var(--zd-color-strong)',
        'zd-sold': 'var(--zd-color-sold)', 'zd-notify': 'var(--zd-color-notify)',
        'zd-error': 'var(--zd-color-error)', debug: 'var(--zd-color-debug)',
        'zd-price': 'var(--zd-color-price)', 'zd-mercari-corporate': 'var(--zd-color-mercari-corporate)',
        black: '#000', white: '#fff', transparent: 'transparent',
      },
      spacing: {
        '1px': 'var(--zd-spacing-1px)',
        'hgap-2xs': 'var(--zd-spacing-hgap-2xs)', 'hgap-xs': 'var(--zd-spacing-hgap-xs)',
        'hgap-sm': 'var(--zd-spacing-hgap-sm)', 'hgap-md': 'var(--zd-spacing-hgap-md)',
        'hgap-md-x2': 'var(--zd-spacing-hgap-md-x2)', 'hgap-lg': 'var(--zd-spacing-hgap-lg)',
        'hgap-lg-x2': 'var(--zd-spacing-hgap-lg-x2)', 'hgap-xl': 'var(--zd-spacing-hgap-xl)',
        'hgap-2xl': 'var(--zd-spacing-hgap-2xl)',
        'vgap-2xs': 'var(--zd-spacing-vgap-2xs)', 'vgap-xs': 'var(--zd-spacing-vgap-xs)',
        'vgap-sm': 'var(--zd-spacing-vgap-sm)', 'vgap-md': 'var(--zd-spacing-vgap-md)',
        'vgap-lg': 'var(--zd-spacing-vgap-lg)', 'vgap-xl': 'var(--zd-spacing-vgap-xl)',
        'vgap-2xl': 'var(--zd-spacing-vgap-2xl)',
      },
      fontSizes: {
        xs: { size: 'var(--zd-font-xs-size)', lineHeight: 'var(--zd-font-xs-lineHeight)' },
        sm: { size: 'var(--zd-font-sm-size)', lineHeight: 'var(--zd-font-sm-lineHeight)' },
        base: { size: 'var(--zd-font-base-size)', lineHeight: 'var(--zd-font-base-lineHeight)' },
        lg: { size: 'var(--zd-font-lg-size)', lineHeight: 'var(--zd-font-lg-lineHeight)' },
        xl: { size: 'var(--zd-font-xl-size)', lineHeight: 'var(--zd-font-xl-lineHeight)' },
        '2xl': { size: 'var(--zd-font-2xl-size)', lineHeight: 'var(--zd-font-2xl-lineHeight)' },
        '3xl': { size: 'var(--zd-font-3xl-size)', lineHeight: 'var(--zd-font-3xl-lineHeight)' },
        '4xl': { size: 'var(--zd-font-4xl-size)', lineHeight: 'var(--zd-font-4xl-lineHeight)' },
        '5xl': { size: 'var(--zd-font-5xl-size)', lineHeight: 'var(--zd-font-5xl-lineHeight)' },
        // pages/404.tsx uses text-6xl, which the old @theme never defined (dead in 2.x). Add or fix.
      },
      fontFamilies: { noto: '…', futura: '…', sans: '…', mono: '…' },  // copy the 4 stacks from global.css:249–258
      fontWeights: { thin: '100', extralight: '200', light: '300', normal: '400', medium: '500', semibold: '600', bold: '700', extrabold: '800', black: '900' },
      lineHeights: { none: 'var(--zd-lineHeight-none)', tight: 'var(--zd-lineHeight-tight)', snug: 'var(--zd-lineHeight-snug)', normal: 'var(--zd-lineHeight-normal)', relaxed: 'var(--zd-lineHeight-relaxed)', loose: 'var(--zd-lineHeight-loose)' },
      radii: { xs: '0.125rem', sm: '0.25rem', md: '0.375rem', lg: '0.5rem' },
      shadows: { sm: '0 1px 2px 0 rgb(0 0 0 / 0.05)', default: '0 1px 3px 0 rgb(0 0 0 / 0.1), 0 1px 2px -1px rgb(0 0 0 / 0.1)', md: '…', lg: '0 10px 15px -3px rgb(0 0 0 / 0.1), 0 4px 6px -4px rgb(0 0 0 / 0.1)', xl: '…', '2xl': '…', inner: 'inset 0 2px 4px 0 rgb(0 0 0 / 0.05)', none: '0 0 #0000' },
    },
    breakpoints: { sm: { minWidthPx: 580 }, md: { minWidthPx: 740 }, lg: { minWidthPx: 980 }, xl: { minWidthPx: 1280 }, '2xl': { minWidthPx: 1630 }, '3xl': { minWidthPx: 1800 } },
    authoredClasses: { 'text-shadow-md': true, 'text-shadow-none': true, 'zd-prose': true },
  },
});
```

  Notes: token values must parse as the consuming property, `var()` references are allowed except for `spacingUnit`, `default` is reserved for `radii`/`shadows`, and `zIndices` keys that are purely numeric are rejected ("numeric-only token names are reserved for numeric utility values"), so `z-10`/`z-40`/`z-50`/`z-[101]` need no token (https://zfb.takazudomodular.com/zudo-wind/tokens/#validating-values). `border-3` resolved without a `borderWidths` map (no such map exists; numeric widths are constants). Sizes such as `--width-full`, `--max-width-xs…7xl` and the fraction entries (361–377) are not needed: `w-full`, `h-screen`, `max-w-[80%]`, `top-1/2`, `-translate-x-1/2` resolved as language constants/arbitrary values; add `sizes` only for named widths the audit still reports (none found).
- `@utility` blocks (383–439) and every `@apply` (397–420, 456–489): no replacement directive exists; write the declarations as ordinary classes in authored CSS (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives). `zd-gradient-black-to-transparent`, `zd-gradient-white-to-transparent`, `zd-hash`, `zd-invert-color-link`, `zd-invert-color-link--focus`, `zd-invert-color-link-inline`, `clearfix` have no utility root and stay ordinary; `text-shadow-md`/`text-shadow-none` start with the `text` root and are ZW006 unless reserved via `authoredClasses` (explain output: "reserve its complete name with wind.authoredClasses"). Expand the `@apply` chains inside `@layer base` (`html`, `body`, `input, textarea`, `a`, `a:focus`, `a:active`) into plain declarations using `var(--zd-*)`; `underline-offset-4` (456) becomes `text-underline-offset: 4px` (the utility itself is **next release**).
- Reset: `@import 'tailwindcss'` shipped preflight, so choose `reset: 'owned-v1'` and paste the measured preflight-compat block (font stacks on `html`/`code`, `sub`/`sup`, `::placeholder`, `::file-selector-button`, `[hidden]`, control `appearance`/`border-radius: 0`/transparent background) from https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#differences-from-tailwind-preflight into `@layer base`. The search dialog `<input>`, the page `<select>` and all `<button>`s are the controls to eyeball. The existing `@layer base { *,::after,::before,::backdrop,::file-selector-button { border-color: currentcolor } }` (446–453) stays.
- Cascade order: Tailwind v4 emitted utilities **before** this file's authored rules, so e.g. `.zd-prose`, `.zoom-lens.is-active`, `.page-image-loader`, `[data-search-dialog]::backdrop` won equal-specificity ties against utilities on the same element. zudo-wind's default `after-authored` flips those ties (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties). On 3.1.0 resolve each tie by specificity or by moving the authored rule into a layer; `wind.utilities.placement: "before-authored"` restores Tailwind's order but is **next release** only. `group-hover:*` selectors drop from (0,2,0) to (0,1,0) (2 sites; check `.group` tooltips in `tooltip-styles.ts` and `thumbs-modal.tsx`).
- Unsupported constructs to rewrite in authored CSS or CSS Modules (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#what-is-not-supported-and-what-to-do-instead): `transform` (bare; `-translate-x-1/2` alone is enough, `page-viewer.tsx:22`, `tooltip-styles.ts`), `ring-1 ring-zd-white` / `focus-visible:ring-1` (`language-toggle.tsx:34` → `outline`/`box-shadow` in CSS), `scale-[1.03]` (one site), `[mask-image:url('/img/takazudo-logo.svg')] [mask-size:contain] [mask-repeat:no-repeat] [mask-position:center]` (`layouts/default.tsx:79–82` → a `.zd-logo-mask` class; `url()` is also rejected in arbitrary values), `shadow-zd-white/5` (`layouts/default.tsx:47`), `h-[calc(100vh-60px)]` → `h-[calc(100vh_-_60px)]` (`sidebar-thumbs.tsx:12`), `container` ×22 → an authored `.container` rule (on 3.1.0 it is silently ordinary and emits nothing; next release warns ZW014), `after:content-['']` ×4 → authored `::after { content: '' }` (same ZW014 class, `content-*`).
- Animations (`@keyframes` ×4 and the `.page-fade-in`, `.drawer-*`, `.fade-in-animation` classes at 506–576) are already authored CSS; keep. `.scrollbar-hide`, `.zd-toc-pointer`, zoom lens/panel rules: keep.
- `ctl()` strings: replace `ctl(\`…\`)` with plain string constants (`const pageStyles = 'min-h-screen pt-[60px] …'`) referenced directly from `class={pageStyles}`. A same-module `const` referenced from a class expression is traced as a class position, so missing tokens and typos fail the build like a direct `class` (https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/#class-positions-and-confidence). Template literals with interpolation (`thumbs-modal.tsx:199`, `sidebar-thumbs.tsx:63,67,81`, `page-navigation.tsx:83,121`, `page-viewer.tsx:313`) are fine as long as each branch is a complete literal (they are). The local `cx()` in `language-toggle.tsx:53` is not one of the recognised helpers (`clsx`, `cn`, `cx`, `classNames`, `classnames` are recognised; `cx` is, so this one qualifies).
- `doc/src/styles/global.css` (ZW009 at 12, 13, 64–68, 90, 273): scaffold-owned; comes back from `create-zudo-doc@6`. The project-owned part is the generated z-index `@theme` block (`pnpm gen:z-index` from `src/config/z-index-tokens.ts`); in v3 that becomes either `wind.tokens.zIndices` (string values, via whatever host `wind` override zudo-doc 6 exposes) or plain `:root { --z-index-… }` custom properties consumed by authored CSS. Decide when the 6.0.0 consumer guide is out.

### 3. Components and islands

Conversion patterns: https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/. Dialect rules: https://zfb.takazudomodular.com/zudo-react/components-and-jsx/. Island boundary: https://zfb.takazudomodular.com/api/island/#the-island-boundary.

- **Mega-island redesign (largest item).** `pages/[manualId]/page/[pageNum].tsx:116–129` passes `<ViewerShell>` as `ManualApp`'s children; in v3 "the child component cannot receive nonempty `props.children`, because children do not cross the island boundary" and hydration fails closed on DOM mismatches, so the capture-and-reinject trick in `manual-app.tsx:18–26,59–65,251–302` cannot survive. Pass the SSR page data as JSON props instead (they are already serialised in `paths()` props: `currentPageNum`, `currentPageTitle`, `currentPageImage`, `currentPageHasContent`, `currentPageContentHtml`, `totalPages`, `availableLangs`, `appManifest`), render `ViewerShell` **inside** `ManualApp` deterministically during setup, and swap to the fetched viewer with `<Show when={dataReady} fallback={() => <ViewerShell …/>}>`. `contentHtml` then travels in `data-props` as well as the SSR body (duplicate bytes per page); if that is too heavy, hydrate only the chrome and keep the shell static. `rawHtml={page.contentHtml}` replaces `dangerouslySetInnerHTML` (`prose-content.tsx:37`); it is mutually exclusive with children and opaque to hydration.
- Hooks → signals/scopes, per file: `useState` → `signal()`; `useMemo`/derived `className` → `computed()`; `useCallback` → plain closures (setup runs once); `useEffect([])`/`useLayoutEffect` → `getScope().onActivate()` (runs after DOM commit, browser only; there is no pre-paint layout phase, so re-check the two `useLayoutEffect` scroll-snap sites in `scroll-viewer.tsx:215` and `page-viewer.tsx:198,227` for flicker); `useEffect([deps])` → `scope.effect()`; `useRef` for DOM → `{ current: null }` `Ref` objects assigned before activation; `forwardRef` + `useImperativeHandle` (`scroll-viewer.tsx:72,233`) → pass a handle `Ref` object or a `scrollTarget` signal as an ordinary prop (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/#what-does-not-exist). Custom hooks `useLang`, `useZoom`, `useIntersectionPages` become setup-time helper functions taking the scope.
- Conditional/keyed regions: `{sidebarOpen && <SidebarThumbs/>}`, `{thumbsModalOpen && <ThumbsModal/>}`, `{fetchFailed && …}` (`manual-app.tsx:322–348`) → `<Show when={signal}>{() => …}</Show>`; thumbnail/option lists built with `.map(… key=…)` from fetched data → `<For each={pages} by={p => p.pageNum}>`; static lists rendered once (`pages/index.tsx:56`, option list in `page-navigation.tsx:101`) may stay as arrays, but `<option>` children must be intrinsic elements with static scalar text and unique string values (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/#table-and-select-children).
- Forms: `search-dialog.tsx:393–397` `<input value={query} onInput>` → `<input modelValue={query} />` with `query = signal('')` (search inputs are supported text models); `page-navigation.tsx:93–106` `<select value={currentPage} onChange>` → `modelValue={pageSelection}` where `pageSelection` is a **string** signal and each `<option value={String(page)}>`; derive the number in an effect (https://zfb.takazudomodular.com/zudo-react/forms/#bind-a-writable-signal). Reactive `value=` is rejected by `zfb check` and at render.
- Listeners: `onClick` → `on:click`, `onInput` → (model), `onChange` → (model), `onKeyDown` → `on:keydown`, `onMouseMove/Enter/Leave` → `on:mousemove`/`on:mouseenter`/`on:mouseleave`, `onLoad`/`onError` on `<img>` → `on:load`/`on:error`. `window`/`document` listeners (`popstate` in `manual-app.tsx:231–246`, keyboard in `keyboard-navigation.tsx`, focus trap in `thumbs-modal.tsx:96–133`) go into `onActivate` with their cleanup returned synchronously. Fetches should take `scope.abortSignal` and check it before writing signals.
- Attribute spellings: 136 `className=` → `class=`; `layouts/default.tsx:103` `charSet` → `charset`, `:111` `crossOrigin` → `crossorigin`; `aria-pressed={boolean}`/`aria-disabled` are fine (booleans serialise); `data-testid` stays (the 54 Playwright tests depend on them).
- `<script dangerouslySetInnerHTML={{ __html: LANG_BOOTSTRAP_SCRIPT }} />` (`layouts/default.tsx:114`) → `<script rawHtml={LANG_BOOTSTRAP_SCRIPT} />`; text children on `<script>`/`<style>` are rejected (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/#trusted-raw-html).
- `<dialog ref={dialogRef}>` with `showModal()`/`close()` (`search-dialog.tsx`) is the recommended replacement for portals and needs no change beyond refs/listeners; `thumbs-modal.tsx`'s `role="dialog"` div + manual focus trap can also move to a native `<dialog>`.
- Island roots stay `LandingLangIsland`, `LandingSearchIsland` (`'use client'`, default exports, JSON props) and `ManualApp`; `appManifest.searchIndexVersion?: string` being `undefined` is omitted from props since 3.1.0 (#3376). `use-lang.ts`/`use-zoom.ts` carry `'use client'` but export no island targets; only reachable `<Island>` targets are registered (https://zfb.takazudomodular.com/concepts/islands/#boundary-discovery-and-migration).
- `components/zfb/search-highlight.ts:1` `import { h } from 'preact'` → `import { h } from '@takazudo/zfb/zudo-react'`; `ComponentChildren`/`JSX` type imports (7 files) → `Child`/`Description` from `@takazudo/zfb/zudo-react`.

### 4. md-wasm and other packages

- No `@takazudo/zfb-md-wasm` usage at the root; `doc/` pins it as part of the zudo-doc lockstep group only. Nothing to change for `jsxRuntime`. Why: https://zfb.takazudomodular.com/guides/migrating-to-v3/#mdx-and-md-wasm.
- `lib/zfb-registry.generated.ts` imports 52 manifests + `pages-ja.json` (~4 MB) into the SSR bundle to stay under the embedded V8 ~10 MB threshold documented in `lib/zfb-registry.ts`; the zudo-react SSR bundle is a different size from Preact's, so re-measure the bundle after the first `zfb build` before adding manuals.
- `@takazudo/zfb-adapter-cloudflare` 3.1.0: `dist/_worker.js` + `_zfb_inner.mjs` layout and the `.assetsignore` convention in `scripts/zfb-finalize-build.js:` and `main-deploy.yml` are unchanged by the migration guide; the dangling source-map reference fix (#3480) is **next release**.

### 5. Tests, CI, deploy

- Vitest: replace `vitest.config.ts`'s `preact()` plugin and `react*` aliases with `esbuild: { jsx: 'automatic', jsxImportSource: '@takazudo/zfb/zudo-react' }` and `environment: 'happy-dom'`; rewrite the 5 Preact-rendering suites (34 tests) on `createIslandTest`/`withIslandTestContext` from `@takazudo/zfb/zudo-react/testing` (https://zfb.takazudomodular.com/zudo-react/testing/). `lang.test.ts`, `zoom.test.ts`, `lib/*.test.ts`, `scripts/*.test.ts` need no change.
- Playwright (54 tests, black-box against `dist/` on :8030): keep as the behavioural oracle for the mega-island rewrite; `data-testid`s must survive. `main-e2e.yml` runs it after `build-zfb`.
- `scripts/test-all-pages-fast.js` smoke (b4push step 6) and `scripts/b4push.sh` are engine-free; add `pnpm exec zfb wind audit --fail-on error` as a b4push step and to `pr-quality-checks.yml` next to `pnpm check` (https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist step 6).
- `.github/actions/build-zfb/action.yml` verifies `dist/assets`, `dist/_headers`, `dist/_redirects`; unchanged. `main-deploy.yml` writes `dist/.assetsignore` with `_worker.js` + `_zfb_inner.mjs` and runs `npx wrangler@<devDeps version> deploy`; unchanged. Node 22 on `ubuntu-latest` is fine.
- `doc-deploy.yml` / `doc-pr-checks.yml` (`pnpm run doc:build`, wrangler upload) stay on the 2.x `doc/` until zudo-doc 6.0.0.
- `doc/scripts/check-pin-parity.mjs` requires the 4 zfb pins and the 3 zudo-doc pins to agree inside `doc/package.json`; `check-template-drift.sh` diffs against `node_modules/create-zudo-doc/templates/*`, so its allowlist is re-derived at 6.0.0.

## Step-by-step plan

Root first (checklist from https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist), `doc/` after zudo-doc 6.0.0.

1. **Branch `zfb3/root`.** Record the baseline: `pnpm build && node scripts/zfb-finalize-build.js`, keep `dist/` as `dist-2x/` for diffing; `pnpm test` (Playwright) green on 2.20.2.
2. **Pins:** `pnpm add @takazudo/zfb@3.1.0 @takazudo/zfb-runtime@3.1.0 @takazudo/zfb-adapter-cloudflare@3.1.0`; `pnpm remove preact preact-render-to-string @preact/preset-vite @testing-library/preact tailwindcss @tailwindcss/postcss eslint-plugin-react-hooks @netlify/classnames-template-literals`; `pnpm add -D happy-dom`; `rm tailwind.config.cjs postcss.config.cjs`; `pnpm exec zfb --version`.
3. **Config:** edit `zfb.config.ts` as in section 2 (delete `framework`, `tailwind` → `wind`); `pnpm exec zfb check` and read the removed-key messages until none remain.
4. **Stylesheet:** delete `styles/global.css:16`, convert `@theme` → tokens (done in step 3), rewrite the 9 `@utility` + 15 `@apply` as authored classes, add the preflight-compat `@layer base` block, author `.container`, `.zd-logo-mask`, ring/transform replacements. `pnpm exec zfb css --input styles/global.css --output /tmp/zmanuals.css` must report no ZW009; `pnpm exec zfb wind audit --fail-on error`.
5. **Components:** `tsconfig.json` jsx source; `grep -rln "from 'preact" components pages layouts lib` → 0; per-file conversion in this order (leaf → root): `ctl.ts` (drop; use string consts), `lang.ts`, `zoom.ts`, `routing.ts`, `search-highlight.ts`, `prose-content.tsx` (rawHtml), `viewer-shell.tsx`, `arrow-link.tsx`, `tooltip-styles.ts`, `viewer-layout-styles.ts`, `language-toggle.tsx`, `search-trigger.tsx`, `search-dialog.tsx` (modelValue), `page-navigation.tsx` (select model), `keyboard-navigation.tsx`, `sidebar-thumbs.tsx`, `thumbs-modal.tsx`, `page-viewer.tsx`, `scroll-viewer.tsx` (handle prop), `header-utility-bar.tsx`, `use-lang.ts`/`use-zoom.ts`/`use-intersection-pages.ts` (scope helpers), `landing-*-island.tsx`, `manual-app.tsx` (props instead of children, `Show`), `layouts/default.tsx` (`charset`, `crossorigin`, `rawHtml`, logo class), `pages/*` (`class=`, drop children from `<Island>`). `pnpm exec zfb check` after each group.
6. **md-wasm:** nothing.
7. **Dynamic classes + gate:** confirm every interpolated class branch is a complete literal (`sidebar-thumbs.tsx:67,81`, `thumbs-modal.tsx:199`, `page-navigation.tsx:83,121`, `page-viewer.tsx:313`); run `pnpm exec zfb wind explain -- '-translate-x-1/2'`, `… 'hover:bg-zd-white/10'`, `… 'lg:-mx-hgap-2xl'`, `… 'text-shadow-md'`; add `zfb wind audit --fail-on error` to `scripts/b4push.sh` and `pr-quality-checks.yml`.
8. **Build and compare:** `pnpm build` (search index + `zfb build` + finalize), `pnpm serve`, open `/`, `/oxi-one-mk2`, `/oxi-one-mk2/page/1`, `/404`; compare against `dist-2x/` screenshots at 390/740/980/1280 widths, in page and scroll modes, with the thumbs modal, search dialog (type with a Japanese IME), language toggle and zoom lens. Run `pnpm test:unit` (rewritten) and `pnpm test` (54 Playwright tests). Update `CLAUDE.md`/`README.md`, push, merge, watch `main-deploy.yml` + `main-e2e.yml`.
9. **`doc/` (after zudo-doc 6.0.0):** `pnpm dlx create-zudo-doc@6` with `doc/setup-preset.json` into scratch; replace the 82 allowlisted template files; keep `settings.ts`, `z-index-tokens.ts`, content, public, scripts; move the z-index block out of `@theme`; bump `doc/package.json` pins (4 zfb + 3 zudo-doc) and run `pnpm -C doc check:pin-parity`, `check:template-drift`, `b4push`; rewrite `design-system/design-system.md` from the root's `wind.tokens`.

## Verification checklist

- [ ] `pnpm exec zfb --version` prints 3.1.0 (or newer) and `embedded esbuild`.
- [ ] `pnpm exec zfb check` passes; no `framework`/`tailwind` removed-key message; no `zfb warn:` about `@jsxImportSource` pragmas.
- [ ] `pnpm exec zfb css --input styles/global.css --output /tmp/zmanuals.css` emits CSS with no ZW009 and the file contains `--zw-color-zd-white`, `--zw-spacing-hgap-sm`, `--zw-font-size-base`.
- [ ] `pnpm exec zfb wind audit --fail-on error` exits 0; `unrecognized classes` lists only intentionally authored names (`zd-prose`, `page-image-loader`, `zoom-lens`, `zoom-panel`, `container`, `scrollbar-hide`, `zd-invert-color-link*`, `clearfix`, `zd-hash`, `zd-gradient-*`).
- [ ] `grep -rnE "className=|onClick=|dangerouslySetInnerHTML|from 'preact" components pages layouts lib` → 0.
- [ ] `pnpm test:unit` passes with the zudo-react testing harness; `pnpm test` (Playwright) passes all 54 tests including `oxi-one-mk2-viewer.spec.ts` navigation, scroll mode, thumbs modal and search.
- [ ] Hydration: no `ZR_HYDRATION_MISMATCH`/`ZR_PROPS`/`ZR_IDENTITY` diagnostics in the browser console on `/oxi-one-mk2/page/1` and on a landing page; `data-zfb-island-mounted` present on all three islands.
- [ ] Search dialog: typing with an IME, Cmd/Ctrl+K open, Escape close, result click navigates; `<select>` page jump works with the string model.
- [ ] Visual: header logo mask, `shadow-lg` header shadow, zoom lens, sidebar thumbnails height (`calc(100vh - 60px)`), `.container` widths, `container`-dependent layouts, prose typography (`.zd-prose` 70 rules), focus rings replaced for `ring-*`.
- [ ] `dist/_worker.js` + `dist/_zfb_inner.mjs` exist; `dist/.assetsignore` written; `npx wrangler deploy --dry-run` succeeds.
- [ ] SSR bundle size re-measured against the ~10 MB V8 note in `lib/zfb-registry.ts`.

## Risks and open questions

- **Island children** is an architectural change, not a mechanical port: the SSR-first-paint guarantee today comes from passing `ViewerShell` as children and from Preact tolerating a DOM/VDOM divergence. zudo-react fails closed on mismatches, so the shell must be rendered by `ManualApp` from props. Expect the island's `data-props` to grow by `contentHtml` per page unless the shell is kept outside the island.
- Client bundle size: zudo-react makes no size promise; zudo-doc's dogfood measured the islands bundle at roughly 2.2× the Preact-era size (zfb#3383). The viewer loads one island per page; measure before and after.
- `ctl()` literals never fail the build; until they are turned into traced `const` strings the audit (not the build) is the only typo detector. Prefer the rewrite over `wind.safelist`.
- `useLayoutEffect` has no pre-paint equivalent (activation runs after DOM commit); the scroll-position restore in `scroll-viewer.tsx` may flash one frame.
- Tie flips (`after-authored` default) can change the look of elements that carry both authored classes and utilities (`.zd-prose` + `text-*`, `.zoom-lens.is-active` + `opacity-*`). The placement switch is **next release** only.
- `container` and `after:content-['']` produce **no CSS and no error** on 3.1.0; only the next release's ZW014 warns. Grep for them explicitly (22 + 4 sites) rather than trusting a green build.
- `doc/` timeline is zudo-doc's (39/51 topics local-only, blocked on unreleased zfb fixes); keep the two zfb pins decoupled in the meantime.
- Open: whether `eslint-plugin-react`'s JSX rules can be configured for the `class`/`on:click` dialect or should be dropped; whether the zudo-doc 6 host `wind` override will accept `zIndices` for the `gen:z-index` flow.

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/
- https://zfb.takazudomodular.com/zudo-wind/configuration/, https://zfb.takazudomodular.com/zudo-wind/tokens/, https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/, https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- https://zfb.takazudomodular.com/zudo-react/components-and-jsx/, https://zfb.takazudomodular.com/zudo-react/forms/, https://zfb.takazudomodular.com/zudo-react/scopes-and-lifecycle/, https://zfb.takazudomodular.com/zudo-react/hydration/, https://zfb.takazudomodular.com/zudo-react/testing/
- https://zfb.takazudomodular.com/concepts/islands/, https://zfb.takazudomodular.com/api/island/, https://zfb.takazudomodular.com/api/cli/, https://zfb.takazudomodular.com/api/define-config/
- https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/, https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/
- zudo-doc 6.0.0: zudolab/zudo-doc#4430, #4477, #4473. zfb follow-ups: #3365 (ZW014/`wind.strict`), #3370 (`--json` audit), #3376 (undefined island props, shipped 3.1.0), #3380/#3383 (bundle size), #3386 (placement), #3480 (adapter source maps)
- Repo files: `zfb.config.ts`, `tsconfig.json`, `styles/global.css`, `styles/prose.css`, `layouts/default.tsx`, `components/zfb/*.tsx`, `pages/[manualId]/page/[pageNum].tsx`, `pages/[manualId]/index.tsx`, `vitest.config.ts`, `playwright.config.ts`, `scripts/zfb-finalize-build.js`, `.github/actions/build-zfb/action.yml`, `.github/workflows/{main-deploy,main-e2e,pr-quality-checks,doc-deploy,doc-pr-checks}.yml`, `wrangler.toml`, `doc/zfb.config.ts`, `doc/src/config/settings.ts`, `doc/src/styles/global.css`, `doc/.template-drift-allowlist`, `doc/scripts/check-pin-parity.mjs`
