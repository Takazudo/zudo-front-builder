# zfb v3 migration guide: Takazudo/zudo-design-token-panel

Generated 2026-10-04 by an automated diagnosis of `main` @ `6e22465` (v0.8.5, 2026-09-27); the audit used the zfb 3.1.0 CLI. zfb 3.2.0 shipped on 2026-10-04 15:30 UTC while this guide was being written, so the pins and version labels below were updated to it (https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/) and the `playground/` + `doc/` audits were re-run with the 3.2.0 CLI (both results are reported). Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**blocked (doc/) with a ready sub-part (playground/), effort M.** This monorepo has two zfb project dirs and one published package. `doc/` is a zudo-doc host (`@takazudo/zudo-doc ^5.27.0`, zfb 2.20.3, Preact, Tailwind through the preset) whose only zfb surface is preset-owned, so it cannot move until zudo-doc 6.0.0 ships (epic zudolab/zudo-doc#4430, root PR zudolab/zudo-doc#4477, consumer guide zudolab/zudo-doc#4473). `playground/` is a first-party zfb 2.20.3 site that uses **zero utilities** (audit: 0 dead classes) and one hooks island, so it can go to zfb 3.2.0 (released 2026-10-04) today with `wind: false`; its one real design problem is `pages/dashboard.tsx`, which renders the Preact `TokenDashboard` component through zfb SSR and has no zudo-react equivalent. `packages/zdtp` is a Vite-built Preact widget, not a zfb project; zfb 3 does not break it, but **zdtp#1002** (bundle Preact, drop the `@tailwindcss/browser` runtime dependency, Preact-free `.d.ts`, document the CSS import) is the task that makes every v3 host, including zudo-doc 6, cleaner, and it is doable now. `examples/minimal` is a plain Vite app and is unaffected. No migration branch exists (`main` + `base/panel-ui-tweaks`, `base/sweep-260925*`, `claude/*`, `open-issues-sweep/*`); `grep -rn 'zudo-react\|zudo-wind\|wind:' --exclude-dir=node_modules .` returns nothing.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `doc/` (zudo-doc host; 44+44 changelog, 5 getting-started, 12 recipes, 12 reference pages, EN + `docs-ja`) | `@takazudo/zfb`, `zfb-runtime`, `zfb-adapter-cloudflare`, `zfb-md-wasm` **2.20.3** (`doc/package.json:28-31`); `zfb.config.ts` = `defineConfig(zudoDoc({...}))` with `adapter: "@takazudo/zfb-adapter-cloudflare"` (l.110) | `@takazudo/zudo-doc ^5.27.0`, `zudo-doc-history-server ^5.27.0`; `@takazudo/zdtp 0.8.2` from npm (deliberately not `workspace:*`, `pnpm-workspace.yaml:18` `linkWorkspacePackages: false`); `designTokenPanel: true` (l.41) | preset-owned: `src/styles/global.css:9-10` `@import "tailwindcss/preflight" layer(zd-preflight)` + `"tailwindcss/utilities"`, `:21-23` three `@source`, `:27` empty `@theme` | `preact ^10.29.1`, `preact-render-to-string ^6.6.6`; `tsconfig.json:7-9` maps `react*` → `preact/compat`; route stubs `pages/docs/[[...slug]].tsx:1-2` and `pages/[locale]/docs/[[...slug]].tsx:1-2` carry `/** @jsxRuntime automatic */ /** @jsxImportSource preact */` | all preset-owned (header trigger `design-token-panel`, l.107) | dependency only (zudo-doc uses it) | Cloudflare Workers `zdtp.zudolab.dev`, `main = ./dist/_worker.js`, `nodejs_compat` (`doc/wrangler.toml`); `.github/workflows/main-deploy.yml`, `pr-checks.yml`, `preview-deploy.yml` (all gated on `vars.CF_WORKERS_DEPLOY`) |
| `playground/` (first-party zfb site: 3 pages + `prose/[slug]`, 2 components, 3 plugins) | `@takazudo/zfb`, `zfb-runtime` **2.20.3** (`playground/package.json:27-28`); `zfb.config.ts`: `framework: 'preact'`, `base: '/'`, 1 collection, `markdown.gfm`, 3 plugins | `@takazudo/zdtp: workspace:*` (aliased to `../packages/zdtp/dist` by `plugins/workspace-zdtp-alias.mjs`) | none (no directive, no dep; `styles/global.css` 391 lines of authored `--zfb-*` CSS) | `preact ^10.29.1` (as zfb framework **and** zdtp peer), `preact-render-to-string ^6.6.6`; hooks only in `components/playground-controls.tsx` (`useState` ×2, `useEffect` ×1) | 1: `components/app-shell.tsx:36` `<Island when="load" ssrFallback={<span aria-hidden="true" />}>` → `PlaygroundControls` | no | assets-only Worker `zdtp-playground.zudolab.dev` (`playground/wrangler.toml`); `playground-deploy.yml`, `playground-pr-preview.yml` |
| `packages/zdtp` (published `@takazudo/zdtp` 0.8.5) | none — Vite lib build (`vite.config.ts`), `esbuild.jsxImportSource: 'preact'` | is zdtp | runtime dep `@tailwindcss/browser 4.3.2` + `tailwind-merge 3.6.0`, both only in `src/dom-tweaker/lazy/` | `peerDependencies.preact ^10.29.1`; dist imports `preact`, `preact/hooks`, `preact/compat`, `preact/jsx-runtime` (zdtp#1002) | n/a | no | npm (`release.yml`) |
| `examples/minimal` (Vite + TS) | none | `@takazudo/zdtp: workspace:*` | none | `preact` as zdtp peer only | n/a | no | `zdtp-minimal.zudolab.dev` — **unaffected by zfb 3** |

Audit (3.1.0 CLI, then re-run with the 3.2.0 CLI on 2026-10-04; `wind: { spec: 1 }` swapped in temporarily; configs restored, `git status --short` clean):

- `doc/`: 3.1.0 — 0 unrecognized, 0 dead, **0 errors**, 15 `auditInfo` (ZW002/ZW005/ZW012 from strings such as `` `docs;${locale}` `` in the two vendored route stubs). 3.2.0 — 0 unrecognized, 0 dead, **0 errors**, 2 `auditInfo` (ZW012 ×2, the `docs;`/`locale-docs;` template literals; the import-specifier ZW002/ZW005 noise is gone because 3.2.0's extractor skips module specifiers). The host has no utilities of its own; everything comes from the preset.
- `playground/`: 3.1.0 — 43 unrecognized (all authored `zfb-*` / `dashboard-page` names), **0 dead**, **22 ZW001 errors — every one a BEM `__` name** (`zfb-nav__manifest`, `zfb-nav__link`, `zfb-nav__note` in `components/app-shell.tsx`; `dashboard-page__header/brand/provenance/main/intro/eyebrow/title/theme-toggle/nav/embed/embed-copy/compact/footer` in `pages/dashboard.tsx`; `zfb-palette__row`, `zfb-palette__label`, `zfb-swatch__chip` in `pages/index.tsx`). On 3.1.0 those fail a build at a class position. 3.2.0 (spec 1 revision 4) — 42 distinct unrecognized names at 68 class positions, the 19 BEM `__` names (24 positions) now listed among them as ordinary authored classes, **0 dead**, **0 errors**, 24 `auditInfo` (12 ZW001 on prose text and punctuation such as `·`, `#fed7aa`, `日本語の文章`; 7 ZW005 on URL paths; 5 ZW012 dynamic constructions): the fix "underscore class names are ordinary" (zfb #3365) **shipped in zfb 3.2.0 (released 2026-10-04)**. `wind: false` is still the right setting because the playground uses no utilities — it disables candidate scanning (https://zfb.takazudomodular.com/api/define-config/).

Greps (`--exclude-dir=node_modules`): Tailwind directives 7, all in `doc/src/styles/global.css`; Preact imports outside `packages/zdtp`: 2 route-stub pragmas + `import type { JSX } from "preact"` (doc), `preact/hooks` + `ComponentChildren` (playground); `dangerouslySetInnerHTML` 1 (`playground/components/app-shell.tsx:28`); `onClick` 2 (`playground-controls.tsx:97,100`); `jsxRuntime` 1 hit at `scripts/fixtures/dashboard-consumer/build.mjs:19` — that is esbuild's own option, not md-wasm, unaffected.

Commands used for the counts above (run from the clone root with a zfb 3.1.0 binary, then again with 3.2.0; `zfb check` cannot run on a bare clone because the preset imports need `node_modules`; the config swap is temporary — restore it and confirm `git status --short` is empty):

```sh
for D in doc playground; do cp $D/zfb.config.ts /tmp/zfb.config.$D.bak; printf 'import { defineConfig } from "zfb/config";\nexport default defineConfig({ wind: { spec: 1 } });\n' > $D/zfb.config.ts; zfb wind audit --project-root $D > /tmp/audit-$D.txt; cp /tmp/zfb.config.$D.bak $D/zfb.config.ts; done
grep -rnE '@import\s+"tailwindcss|@tailwind|@theme|@apply|@source|@utility|@variant|@custom-variant|@plugin|@config|theme\(' --include=*.css --include=*.ts --include=*.tsx --include=*.mjs --exclude-dir=node_modules --exclude-dir=dist . | grep -v '^./packages/zdtp/'
grep -rnE "from ['\"]preact|preact/hooks|@jsxImportSource|dangerouslySetInnerHTML|onClick=|onKeyDown=|<Island|\"use client\"" --include=*.ts --include=*.tsx --exclude-dir=node_modules --exclude-dir=dist . | grep -v '^./packages/zdtp/'
```

## Sequencing and blockers

1. **Now — `packages/zdtp`: implement zdtp#1002.** Bundle Preact (or make it a regular `dependency`), lazy-load or make optional `@tailwindcss/browser`/`tailwind-merge` (only `src/dom-tweaker/lazy/tailwind-runtime.ts:108,137` and `edit-session.ts:1` use them), ship `.d.ts` that do not import Preact types, and document for v3 hosts that no CSS import is required (PORTABLE-CONTRACT §7 self-injects) or that `@takazudo/zdtp/dist/zdtp.css` works. The #1002 probe already showed zdtp 0.8.5 mounting under zfb 3.0.0 as a lazily imported 504,228-byte chunk with 0 console errors, so this is packaging hygiene, not a rewrite. It unblocks zudo-doc 6's decision DD3 ("opaque lazily imported Preact bundle") and every host guide that points here.
2. **Now — `playground/` to zfb 3.2.0** (§Required changes). Independent of zudo-doc. Keep `preact` installed as zdtp's peer until step 1 ships.
3. **Now — docs content**: `doc/src/content/docs/getting-started/frameworks-comparison.mdx:137-193` documents `framework: 'preact'`, "zfb uses Preact islands", the `@theme` block and `@takazudo/zfb/config`; `examples.mdx:59-77` describes both example repos; the JA mirrors under `docs-ja/` match. `doc/CLAUDE.md` allows prose to lead the pinned release, so these can be rewritten to the v3 pattern (lazy `import()` in `onActivate`, `wind.tokens`) as soon as the two example repos migrate (see their guides).
4. **After zudo-doc 6.0.0 ships — `doc/`**: upgrade the preset first (a preset still supplying `framework`/`tailwind` fails config loading, https://zfb.takazudomodular.com/guides/migrating-to-v3/#before-you-start), then regenerate the host files listed in §Required changes 1/2 from the create-zudo-doc 6 templates via `/dev-bump-zudo-deps` + `ZUDO_DEPS_PINS.md`. Nothing in `doc/` can be built on zfb 3 before that.
5. **zfb 3.2.0 (released 2026-10-04)** ships #3365 BEM underscores, `zfb wind audit --json` / `explain --stdin`, `wind.strict` + ZW014, `zfb wind manifest`, `wind.utilities.placement`, zudo-wind spec revision 4 and the #3569/#3570 fixes zudo-doc 6.0.0 was blocked on (https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/). Nothing in this repo waits on zfb any more; pin exact `3.2.0` (peer floor `^3.2.0` — `^3.1.0` would still admit the 3.1.0 build panic, #3569).

## Required changes

### 1. Dependencies, config, tsconfig, env

**playground/**
- `playground/package.json:27-28`: `@takazudo/zfb`, `@takazudo/zfb-runtime` → `3.2.0` (exact; not 3.1.0, which has the #3569 build panic). The repo has one root `pnpm-lock.yaml` (no `doc/` or `playground/` lockfile), so the bump re-resolves there. Keep `preact` (zdtp peer, see zdtp#1002) but move it next to `@takazudo/zdtp` with a comment; drop `preact-render-to-string` from `dependencies` unless the dashboard pre-render in §3 adopts it (then `devDependencies`).
- `playground/zfb.config.ts:4`: delete `framework: 'preact'`; add `wind: false` (no utilities anywhere; on 3.1.0 it also sidestepped the 22 ZW001 BEM errors, which 3.2.0 fixes). `base`, `collections`, `markdown.gfm`, `plugins` are unchanged keys (https://zfb.takazudomodular.com/api/define-config/). Plugin hooks used — `setup`/`addAlias`, `preBuild`, `postBuild` with `routes`, `devMiddleware` — all still exist (https://zfb.takazudomodular.com/concepts/plugins/).

```ts
import { defineConfig } from '@takazudo/zfb/config';
export default defineConfig({
  base: '/',
  wind: false,
  collections: [{ name: 'prose', path: 'content/prose' }],
  markdown: { gfm: true },
  plugins: [
    { name: './plugins/workspace-zdtp-alias.mjs' },
    { name: './plugins/dashboard-styles.mjs' },
    { name: './plugins/dev-apply-proxy.mjs' },
  ],
});
```
- `playground/tsconfig.json:8`: `"jsxImportSource": "preact"` → `"@takazudo/zfb/zudo-react"` (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands).
- Env/CI: no `ZFB_TAILWIND_*` anywhere (`grep -rn ZFB_TAILWIND .github scripts` → 0).

**doc/** (after zudo-doc 6.0.0)
- `doc/package.json:28-40`: zfb packages → the release zudo-doc 6 pins; `@takazudo/zudo-doc` / `zudo-doc-history-server` → `^6.0.0`; keep `preact` only if zdtp#1002 has not shipped (6.0 keeps it as an install-time dep for zdtp per #1002); bump `@takazudo/zdtp` (currently 0.8.2 while the package is 0.8.5).
- `doc/zfb.config.ts`: the host passes no `framework`/`tailwind`; whatever `zudoDoc({ wind })` override 6.0 exposes replaces the empty `@theme` slot. Follow zudolab/zudo-doc#4473 when it exists.
- `doc/tsconfig.json:8-10`: drop the `react`/`react-dom` → `preact/compat` `paths` (only needed by the Preact engine); take `jsx`/`jsxImportSource` from the 6.0 `tsconfig.base.json`.
- `doc/pages/docs/[[...slug]].tsx:1-2`, `doc/pages/[locale]/docs/[[...slug]].tsx:1-2`: remove both pragmas and `import type { JSX } from "preact"`. With Preact still installed for zdtp the failure mode is a `ZR_CHILD` diagnostic, not a resolve error (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Regenerate both stubs from create-zudo-doc 6 (`ZUDO_DEPS_PINS.md` lists them).

### 2. CSS and utilities

- `playground/styles/global.css` (391 lines, authored only) and `public/dashboard-page.css` need no change under `wind: false`; the leftover-directive check still runs and finds nothing (0 directives).
- `doc/src/styles/global.css:8-10,21-23,27`: every line except the zudo-doc `@import`s is a **ZW009 error** in v3, even under `wind: false` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives). Replace the whole file with the create-zudo-doc 6 template when it exists. Line 8 `@import "@takazudo/zdtp/styles.css"` is the spelling zdtp#1002 reports failing on 3.0.0; 3.1.0's changelog lists "Resolve authored CSS package subpaths through package `exports`" (https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/) — verify on the pinned release, fall back to `@takazudo/zdtp/dist/zdtp.css` or omit it (self-injected).

### 3. Components and islands

**`playground/components/app-shell.tsx`**
- l.2 `import type { ComponentChildren } from 'preact'` → `import type { Child } from '@takazudo/zfb/zudo-react'` (https://zfb.takazudomodular.com/zudo-react/api-reference/#child).
- l.24 `<meta charSet="utf-8" />` → `charset` (React spellings fail validation, https://zfb.takazudomodular.com/zudo-react/components-and-jsx/#use-html-and-svg-names).
- l.27-31 `<script dangerouslySetInnerHTML={{ __html: ... }} />` → `<script rawHtml={THEME_BOOT} />` with the string in a `const` (trusted, static; https://zfb.takazudomodular.com/zudo-react/components-and-jsx/).
- l.36-38: `<Island when="load" ssrFallback={<span aria-hidden="true" />}>` keeps working (`when`, `ssrFallback` are v3 props, https://zfb.takazudomodular.com/api/island/). Drop the `as unknown as IslandProps['children']` cast: in 3.1.0 `IslandProps.children` and `ssrFallback` are typed `VNode`, a loose union (`string | number | boolean | null | undefined | bigint | VNodeArray | VNodeObject | object`, `@takazudo/zfb/dist/island.d.ts` + `jsx-types.d.ts`); 3.2.0 narrows them to the owned types `Description` (children) and `Child` (ssrFallback) from `@takazudo/zfb/dist/zudo-react/description.d.ts`, which a JSX element satisfies directly — either way a JSX child needs no cast.

**`playground/components/playground-controls.tsx`** (the one island; 4 hook calls, 2 `onClick`)
- `useState` ×2 → `signal<Mode>('light')`, `signal<ManifestName>('playground')`; `useEffect(..., [])` → `getScope().onActivate(() => { ...; return () => window.removeEventListener(...) })` (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/#hook-by-hook).
- l.4 `import * as zdtp from '@takazudo/zdtp'` is a **static** import of the Preact widget inside a `"use client"` module. Replace it with `const mod = await import('@takazudo/zdtp')` started **inside** `onActivate`, exactly as https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget prescribes ("start `import()` inside the callback, never at module top level where SSR could execute it"). `configurePanel()` returns the `PanelInstanceHandle` (`open/close/toggle/destroy`) this file already uses, so the body is unchanged.
- l.97,100 `onClick` → `on:click`; `{mode === 'dark' ? ... : ...}` → `computed()` label.

**`playground/pages/dashboard.tsx` — design decision required.** `<TokenDashboard>` from `@takazudo/zdtp/dashboard` is a Preact component rendered by zfb SSR into a static page. zudo-react cannot render Preact VNodes ("Do not pass the widget's VNodes to zudo-react", islands doc above). Options, in order of fidelity to the page's own promise ("no client JavaScript", `plugins/dashboard-styles.mjs:19-38` enforces it): (a) pre-render each `<TokenDashboard ...>` to an HTML string with `preact-render-to-string` in a `preBuild` plugin or a build-time module and emit it through `<div rawHtml={html} />` (the recipe `doc/src/content/docs/recipes/static-token-dashboard.mdx:67` already tells non-Preact hosts to do this); (b) mount it client-side as an opaque widget island (breaks the static-only guard); (c) move the page to `examples/minimal` (Vite). Recommend (a). `pages/index.tsx` and `pages/prose/[slug].tsx` need only the `charset` spelling; `style={`--swatch-color: ...`}` CSS strings stay valid; `@takazudo/zfb/content` still exports `getCollection`, `getEntry`, `defaultComponents`.

**`plugins/dashboard-styles.mjs:27-33`** assumes zfb 2.13 injected the islands loader on island-less pages; v3 injects no islands script on a page without islands (https://zfb.takazudomodular.com/concepts/islands/#consequences-of-the-shared-bundle-model), so the `<script>` regex becomes a no-op and the `<link ... /assets/styles*.css>` regex must be re-verified against v3 output. The final `/<script\b/` guard still protects the contract.

**doc/**: no host-owned components (`src/chrome-bindings.tsx` absent). Nothing to port.

### 4. md-wasm and other packages

- `@takazudo/zfb-md-wasm` is only a dependency of `doc/` for zudo-doc's HtmlPreview; no `compile(`/`renderHtml(` call in this repo. Nothing to do beyond the version bump.
- `packages/zdtp` (zdtp#1002): bundle Preact or declare it as a `dependency` (`vite.config.ts` `rollupOptions.external` lists `preact`, `preact/compat`, `preact/hooks`, `preact/jsx-runtime` — remove them from `external` or add `preact` to `dependencies`); make `@tailwindcss/browser` + `tailwind-merge` optional/lazy (already dynamically imported at `src/dom-tweaker/lazy/tailwind-runtime.ts:108`, so moving them to `optionalDependencies` or `peerDependenciesMeta.optional` is the packaging change; `src/dom-tweaker/lazy/__tests__/tailwind-runtime.lazy-boundary.test.ts:11` already guards the boundary); strip Preact types from the emitted `dist/**/*.d.ts` files (dist/ is not committed — count them after `pnpm --filter @takazudo/zdtp build`) or document them; add a v3-host section to `PORTABLE-CONTRACT.md` §7 and `README.md`. Keep `./dashboard` export Preact-only and documented as "render with preact-render-to-string" for zudo-react hosts.

### 5. Tests, CI, deploy

- `.github/workflows/ci.yml:84-85` `pnpm --filter @takazudo/zdtp test:e2e` boots the **playground zfb dev server** (`packages/zdtp/playwright.e2e.config.ts:16` `pnpm --filter playground run dev:zfb`), so the package's walking-skeleton E2E is the first thing that proves the v3 playground; keep it green before merging the playground change.
- `ci.yml:67-68` `pnpm --filter playground build:deploy`, `scripts/run-b4push.sh:56-57`, `playground-deploy.yml:46-50`, `playground-pr-preview.yml`: unchanged commands; `zfb build` on 3.1.0 must stay green (no `wind` audit gate needed under `wind: false`).
- `ci.yml:109-153` `consumer-smoke` → `scripts/verify-packed-remount-consumer.sh:159-160` hard-asserts `@takazudo/zfb@2.20.3` and `@takazudo/zudo-doc@5.27.0`, and `:167` asserts the panel and zudo-doc share one Preact; `scripts/doc-with-local-panel.sh:109-124` has the same Preact-sharing check. After zudo-doc 6 + zdtp#1002 the versions change and the "shared Preact" invariant inverts (the panel bundles its own); rewrite both assertions then.
- `packages/zdtp/scripts/verify-packed-remount-browser.mjs:191-224` listens to `zfb:before-swap` / `zfb:after-swap` / `zfb:page-load` from the zfb client router — those names are the client-router contract (https://zfb.takazudomodular.com/concepts/client-side-routing/); re-run against the v3 doc once it exists.
- `.github/workflows/consumer-drift.yml` installs the latest zudo-doc for the manifest-contract check; it will start pulling 6.0.0 the day it ships — expect that run to go red first and treat it as the signal to start step 4.
- Deploy configs (`doc/wrangler.toml`, `playground/wrangler.toml`) are unchanged by v3; re-run `scripts/check-deploy-paths.sh` after the first v3 build.

## Step-by-step plan

1. `packages/zdtp`: implement zdtp#1002 (bundle/declare Preact, optional Tailwind runtime, Preact-free types, v3 CSS note); `pnpm --filter @takazudo/zdtp build && pnpm test && pnpm test:pack`; release 0.9.0. Nothing else in this plan depends on it, but zudo-doc 6 and both example repos want it.
2. Branch `topic/playground-zfb3`. `playground/package.json` → zfb 3.2.0; `pnpm install` (root lockfile). Run `pnpm --filter playground exec zfb --version` (expect `zfb 3.2.0 / embedded esbuild`, no Tailwind line).
3. Config: delete `framework`, add `wind: false`; `zfb check` from `playground/` must report no removed-key error (https://zfb.takazudomodular.com/guides/migrating-to-v3/#configuration).
4. tsconfig `jsxImportSource`; port `app-shell.tsx` and `playground-controls.tsx` as in §3; `pnpm --filter playground typecheck`.
5. Decide and implement the dashboard pre-render (§3, option a); update `plugins/dashboard-styles.mjs` regexes to v3 output.
6. `pnpm --filter playground build:deploy && pnpm --filter playground preview`; open `/`, `/dashboard/`, `/prose/en/`, `/prose/ja-sample/`; open the panel, toggle theme, switch `?manifest=zudo-doc`.
7. `pnpm --filter @takazudo/zdtp test:e2e` (walking skeleton against the v3 playground) and `pnpm b4push`.
8. Rewrite `frameworks-comparison.mdx` §zfb / §zfb + Tailwind and `examples.mdx` (EN + JA) once the example repos have their v3 shape.
9. When zudo-doc 6.0.0 ships: `/dev-bump-zudo-deps`, regenerate `doc/pages/**` stubs and `doc/src/styles/global.css` from create-zudo-doc 6, remove the pragmas and tsconfig `paths`, update the two consumer scripts' assertions, `pnpm build && pnpm --filter doc check:html && check:links`, then `pnpm --dir doc exec wrangler deploy --dry-run`.

## Verification checklist

- [ ] `zfb --version` in `playground/` prints 3.2.0 and only an embedded esbuild line.
- [ ] `zfb check` passes in `playground/` with no `framework`/`tailwind` error.
- [ ] `grep -rln '@jsxImportSource\|preact/hooks' playground doc/pages` → 0 (after step 9 for `doc/`).
- [ ] `zfb build` emits no `zfb warn:` pragma lines and no `ZR_` diagnostic.
- [ ] `/dashboard/` HTML contains `<zdtp-dashboard>` markup and **no** `<script>` (existing postBuild guard).
- [ ] Panel opens on `/`, `/prose/en/`; `window.zfb.toggleDesignPanel` is a function; theme toggle re-configures the zudo-doc manifest.
- [ ] `pnpm --filter @takazudo/zdtp test:e2e`, `test:pack`, VRT green; `consumer-smoke` rewritten and green after step 9.
- [ ] `doc/`: `check:html` and `check:links` green; `wrangler deploy --dry-run` green; `/docs/getting-started/frameworks-comparison/` content matches the migrated examples.

## Risks and open questions

- The static dashboard page has no zudo-react rendering path; option (a) depends on `rawHtml` accepting the full `TokenDashboard` markup (block content inside `<div>` is valid; hydration is not involved on a page with no islands).
- `ssrFallback={<span aria-hidden="true" />}` keeps skip-SSR semantics in v3, and so would a `null` fallback: 3.1.0 selects skip-SSR whenever the prop is not `undefined` (`packages/zfb/src/island-boundary.ts:55`, `skipSsr: fallback !== undefined`; https://zfb.takazudomodular.com/api/island/#skip-server-rendering).
- Until zdtp#1002 ships, `preact` stays installed in `playground/` purely as zdtp's peer; a leftover `@jsxImportSource preact` anywhere then fails with `ZR_CHILD` rather than a resolve error — grep before every build.
- The 22 BEM `__` names are harmless under `wind: false`; if the playground ever turns `wind` on (to demo utilities), zfb 3.2.0 already treats them as ordinary authored classes (#3365; re-run audit: 0 errors).
- `doc/` timing is entirely zudo-doc's: its `base/zfb3-migration` branch is pushed @ `70e0875` (PR zudolab/zudo-doc#4477, 102 commits; only the #4467 integration topic is still local), and the integration floor's zfb blockers #3569/#3570 shipped in zfb 3.2.0 on 2026-10-04 — so the zudo-doc side is unblocked, but 6.0.0 is not published; plan the doc move for after it lands.
- `consumer-drift.yml` will go red on the first zudo-doc 6.0.0 publish; decide beforehand whether to pin or to treat it as the trigger.

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/
- https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget
- https://zfb.takazudomodular.com/api/island/
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/
- https://zfb.takazudomodular.com/zudo-react/components-and-jsx/
- https://zfb.takazudomodular.com/api/define-config/ · https://zfb.takazudomodular.com/concepts/plugins/
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ · https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ · https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/ · https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/ (released 2026-10-04)
- Takazudo/zudo-design-token-panel#1002 (zfb v3 hosts: bundle Preact, CSS import) · zudolab/zudo-doc#4430, #4477, #4473 · Takazudo/zudo-front-builder#3365 (BEM underscores, shipped in 3.2.0)
- Sibling guides: `Takazudo--zudo-design-token-panel-example-zfb.md`, `Takazudo--zudo-design-token-panel-example-zfb-tailwind.md`, `zudolab--zudo-doc.md`
