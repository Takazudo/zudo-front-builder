# zfb v3 migration guide: Takazudo/zudo-design-token-lint

Generated 2026-10-04 by an automated diagnosis of `main` @ `b697003`. Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**Blocked (upstream), effort L.** The npm package at the repo root is pure TypeScript and untouched by zfb 3. The blast radius is entirely `doc/`: a zudo-doc **5.8.0** host on zfb **2.7.1** (the oldest pins in this group), with the one genuinely interactive host island in the group - `src/components/playground.tsx` (232 lines, `preact/compat` `useState` x4 / `useRef` x2 / `useEffect` x1, two controlled `<textarea value onInput>`, a keyed `.map()` list, 22 `className=`, `onClick`, `htmlFor`) - wired through `src/chrome-bindings.tsx` as `Island({ when: "load", ssrFallback, children })`, plus `designTokenPanel: true` (zdtp 0.4.12, Preact), i18n (ja), a versioned `2.0` snapshot, the Cloudflare adapter, ten dead component shims, and a 140-line `global.css` that imports `@takazudo/zdtp/styles.css`. `zfb wind audit` reports 29 distinct host utility candidates (56 occurrences) that are all zudo-doc theme tokens (`hsp-*`/`vsp-*` spacing, `caption`, `muted`/`accent`/`danger`/`success`/`surface`/`code-bg` colors, `lg` breakpoint), four errors that exist only because the bare audit has no tokens (`lg:grid-cols-2` ZW002 unconfigured breakpoint; `border-muted/30` x2 and `border-muted/20` ZW005, a color-opacity modifier that cannot be validated without the `muted` color), and only audit-info noise from `src/lib/lint-browser.ts` regex strings. Start is gated on `@takazudo/zudo-doc` 6.0.0 (5.x `zudoDoc()` emits `framework`/`tailwind`, which fail config loading on zfb 3, https://zfb.takazudomodular.com/guides/migrating-to-v3/#before-you-start); the zfb side is already released (3.2.0, 2026-10-04). Prep that pays off now: delete the dead shims and unused Tailwind/React devDependencies, fix the stale `.zudo-doc.json`, and step the pins to 2.22.1 / 5.28.2 so the 6.0 change set is the engine alone.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `doc/` (`@takazudo/zudo-design-token-lint-doc`, workspace member) | `@takazudo/zfb` 2.7.1, `zfb-runtime` 2.7.1, `zfb-md-wasm` 2.7.1, `zfb-adapter-cloudflare` 2.7.1 (exact) | `@takazudo/zudo-doc ^5.8.0` (lock 5.8.0), `zudo-doc-history-server ^5.8.0`, `create-zudo-doc ^5.8.0` (devDep); `@takazudo/zdtp` 0.4.12 with `designTokenPanel: true` | **unused devDeps** `@tailwindcss/vite ^4.2.0`, `tailwindcss ^4.2.0` (no vite config in `doc/`); `global.css:12-13` Tailwind imports, `:84-88` 5 `@source`, no `@theme` block | `preact ^10.29.1` (lock 10.29.1), `preact-render-to-string`, **`@types/react ^19.2.0`** (devDep the scaffold deliberately omits); pragma in `chrome-bindings.tsx` + 2 stubs; `preact/compat` hooks in `playground.tsx` | 1 host island (`Playground`, `"use client"`, skip-SSR via `ssrFallback`) + package islands (docHistory, imageEnlarge, dynamicPageTransition, search, version-switcher, language-switcher, design-token-panel) | transitively only | Cloudflare Workers: `adapter: "@takazudo/zfb-adapter-cloudflare"`, `doc/wrangler.toml` (`main = ./dist/_worker.js`, `nodejs_compat`, custom domain `zudo-design-token-lint.takazudomodular.com`), `wrangler` 4.85.0 exact; `.github/workflows/doc-deploy.yml` + `doc-preview.yml` |
| repo root (`@takazudo/zudo-design-token-lint` 2.1.0) | - | - | lint target only (`.design-token-lint.json` points `patterns` at `doc/src/**`, `doc/pages/**`) | - | - | - | npm publish (`publish.yml`); **not a zfb project** |

Measured facts (clone root unless noted):

- `find . -name 'zfb.config.*' -not -path '*/node_modules*'` -> `doc/zfb.config.ts` (175 lines).
- `doc/zfb.config.ts`: `zudoDoc({ themePack: 'bauhaus', logo, siteUrl, locales: { ja }, entryDocSlug: 'overview/getting-started', metaTags, llmsTxt, cjkFriendly, chromeBindingsModule: './src/chrome-bindings.tsx', designTokenPanel: true, sidebar*, imageEnlarge, dynamicPageTransition, docHistory: true, bodyFootUtilArea, versions: [{ slug: '2.0', docsDir: 'src/content/docs-v2.0', locales: { ja }, banner: 'unmaintained' }], claudeResources: { claudeDir: '../.claude', projectRoot: '.', scanRoot: '..' }, defaultLocaleOnlyPrefixes, footer, headerNav x6, headerRightItems x5, adapter: '@takazudo/zfb-adapter-cloudflare' })`. No host-authored `framework`/`tailwind` key.
- `doc/.zudo-doc.json` = `{ "packageVersion": "2.0.0", "ejected": {} }` - **stale** (installed zudo-doc is 5.8.0); `doc/scripts/check-pin-parity.mjs` does not read this file, so nothing caught it.
- `doc/src/components/`: 11 files, 1 live (`playground.tsx`, imported by `src/chrome-bindings.tsx:39`) and **10 dead** (`grep -rn "@/components\|/components/" doc/src doc/pages --include=*.ts --include=*.tsx` -> that one import plus 8 comment-only mentions: 6 inside the shims themselves, 2 in `src/lib/*.ts`): `ai-chat-modal.tsx`, `preset-generator.tsx`, `client-router-bootstrap.tsx` (each `import type { JSX } from "preact"`), and 7 one-line re-exports of `@takazudo/zudo-doc/*` subpaths (`desktop-sidebar-toggle`, `doc-history`, `image-enlarge`, `sidebar-toggle`, `sidebar-tree`, `content/code-group`, `content/content-admonition`).
- Hook census (`doc/src doc/pages`, excluding content): `useState` x4, `useRef` x2, `useEffect` x1 (`playground.tsx:14,123-128,136`); `className=` 22 in `playground.tsx`, 2 in `chrome-bindings.tsx:57,60`; `onInput=` x2 (`:159,172`), `onClick=` x1 (`:185`), `htmlFor` x2 (`:153,166`), `key=` x1 (`:212`), `dangerouslySetInnerHTML` 0, `createPortal|useId|forwardRef|Suspense` 0.
- Content: 23 EN + 23 JA current pages, 22 + 22 in the `docs-v2.0` snapshots, `<Playground />` (EN) and `<Playground lang="ja" />`, `<Note>`/`<Warning>` admonitions. Every `className=` in MDX is inside a code fence or inline code (`reference/api/index.mdx:281-285,341`, `changelog/v1.1.0-next.2.mdx`); awk fence check found no raw JSX `className`.
- `src/lib/lint-browser.ts` (1176 lines) and `src/lib/playground-samples.ts` (53) are plain TS; the audit's 44 `ZW012` "dynamic construction" and 31 `ZW001` lines there are regex/string literals, all `auditInfo`.
- `grep -rn ZFB_TAILWIND .` -> 0. `pnpm-workspace.yaml` `minimumReleaseAgeExclude` still lists `@takazudo/zfb*@1.1.1` and `@takazudo/zdtp@0.4.9` (stale).
- Audit (`zfb wind audit --project-root doc`, zfb 3.1.0, spec revision 3, temporary `{ wind: { spec: 1 } }`, restored, tree clean; zfb 3.2.0 became npm `latest` later the same day - its revision-4 catalog adds none of the names used here, and its `ZW014` warnings target Tailwind-only names such as `ring-*`/`animate-*`, which the Playground does not use): `unrecognized classes` = 1 (`zd-playground-placeholder`, ordinary); `dead classes` = 56 entries / 29 unique candidates (`sed 's/ \[ZW.*//' | sort -u`), all `ZW006` missing-token except `lg:grid-cols-2` (`ZW002`) and `border-muted/30` (`chrome-bindings.tsx`, `playground.tsx:201`) + `border-muted/20` (`playground.tsx:213`) (`ZW005` "slash modifier is not supported": without a `muted` color token the audit cannot validate the opacity suffix; with `tokens.colors.muted` configured, `zfb wind explain -- border-muted/30` resolves to `v1.border.color`, slash modifier 30, so these need no rewrite once the token exists); diagnostics: ZW006 x124, ZW005 x41 (3 errors, 38 auditInfo), ZW012 x48, ZW001 x31, ZW002 x4 (1 error), ZW004 x3 (`!important` and arbitrary-property strings inside `lint-browser.ts`/`playground-samples.ts`, audit-only); by file: `playground.tsx` 83, `lint-browser.ts` 72, `chrome-bindings.tsx` 12, two stubs 7 each.

## Sequencing and blockers

1. **Upstream gate:** `@takazudo/zudo-doc` 6.0.0 (zudolab/zudo-doc#4430 epic, #4477 root PR, #4473 consumer guide). The zfb side is done: zfb 3.2.0 (npm `latest`, published 2026-10-04 15:30 UTC, https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/) ships the fixes the integration floor waited for (Takazudo/zudo-front-builder#3569/#3570), plus `wind.utilities.placement`, `wind.sources.exclude`, the pragma warning and the wider HTML/SVG vocabulary used below; `@takazudo/zfb-adapter-cloudflare` 3.2.0 ships the Cloudflare source-map fix (#3480). Expect 6.0.0 to pin 3.2.0 or later.
2. **zdtp stays Preact.** zudo-doc 6.0 keeps `@takazudo/zdtp` as an opaque, lazily imported Preact bundle (planning decision DD3; Takazudo/zudo-design-token-panel#1002 asks zdtp to bundle Preact and document a CSS import that works on zfb 3). Consequence for this host: `preact` remains installed as zdtp's peer, so a leftover `@jsxImportSource preact` pragma no longer fails at resolve time but **fails static rendering with `ZR_CHILD`** - the migration guide calls out exactly this configuration (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Remove every pragma.
3. **Now:** cleanup that is engine-neutral - delete the 10 dead shims, remove `@tailwindcss/vite`, `tailwindcss`, `@types/react`, fix `.zudo-doc.json`, refresh `pnpm-workspace.yaml` excludes, and step the pins (2.7.1 -> 2.22.1, 5.8.0 -> 5.28.2, zdtp 0.4.12 -> 0.8.5 which zudo-doc 5.28's peer range `^0.5.2 || ^0.6.0 || ^0.7.0 || ^0.8.0` admits). zfb's 2.x changelogs have no `breaking` note after v2.0.0; zudo-doc's CHANGELOG has `Breaking Changes` headings only at major versions (5.0.0 and earlier), none between 5.8.0 and 5.28.2; `check:wrangler-pin` will tell you the wrangler the 2.22.1 binary expects.
4. **Now (optional, forward-compatible):** rewrite `Playground` without hooks only after 6.0.0; on 5.x it must stay Preact. What you can prepare is the data shape: give each `LintResult` a stable key (the port needs it for `For`).
5. **After 6.0.0:** regenerate the scaffold files, port `chrome-bindings.tsx` + `playground.tsx`, re-baseline the four gate scripts, build, deploy.

## Required changes

### 1. Dependencies, config, tsconfig, env

- **`doc/package.json`**
  - zfb family x4 -> the exact version zudo-doc 6.0.0 pins (`check-pin-parity.mjs` checks `@takazudo/zfb`, `zfb-runtime`, `zfb-adapter-cloudflare`; **add `@takazudo/zfb-md-wasm`** to its `ZFB_PACKAGES`, it is currently missing from the lockstep check).
  - `@takazudo/zudo-doc`, `zudo-doc-history-server`, `create-zudo-doc` -> 6.x (the script strips `^`/`~` and compares bases).
  - remove `preact`, `preact-render-to-string` **only if** zdtp's 6.0-era peer no longer needs them (today zdtp peers `preact ^10.29.1`); otherwise keep `preact` and drop only `preact-render-to-string`.
  - remove `@tailwindcss/vite`, `tailwindcss` (nothing in `doc/` runs Vite; zfb 2.x bundled its own Tailwind), `@types/react` (the scaffold omits it on purpose: `jsx: react-jsx` + `jsxImportSource` provides JSX types).
  - review the pre-rescaffold leftovers `shiki`, `@shikijs/transformers`, `clsx`, `gray-matter`, `mermaid`, `remark-cjk-friendly`, `remark-directive`, `minisearch`, `pagefind`, `sharp`, `tsx` against the 6.x scaffold's `package.json`; `grep -rn 'mermaid\|katex' doc/src/content` decides `mermaid`/`katex`.
  - `wrangler` -> the version the new zfb binary expects (`check-wrangler-pin.mjs`); in v3 `zfb preview` treats that version as a **minimum** (older aborts, equal or newer proceeds, https://zfb.takazudomodular.com/api/cli/#zfb-preview), so the exact-equality guard is stricter than zfb - keep or relax deliberately.
- **`doc/.zudo-doc.json`**: `packageVersion` -> installed zudo-doc version (now, and again at 6.x).
- **Lockfile**: the root `pnpm-lock.yaml` (pnpm workspace, `pnpm-workspace.yaml` lists only `doc`); `doc/` has no lockfile of its own, so every doc bump re-resolves there and `doc-preview.yml`/`doc-deploy.yml` install it with `--frozen-lockfile`.
- **`doc/zfb.config.ts`**: no removed keys authored by the host. Expect 6.0 to carry a package-owned `wind` fragment (tokens, reset, a candidate manifest exported by the package) with a user-wins override (https://zfb.takazudomodular.com/zudo-wind/configuration/#strict-validation-and-merging). The audit (section 2) shows that every host candidate is a zudo-doc token, so the override should be empty unless 6.0 drops a token the Playground uses.
- **`doc/tsconfig.json`**: delete the `react`, `react/jsx-runtime`, `react-dom` paths (lines 8-10); keep `@/*` (line 7). Add `"jsx": "react-jsx", "jsxImportSource": "@takazudo/zfb/zudo-react"` only if the 6.x base does not.
- **Env/CI vars**: none (`ZFB_TAILWIND*` absent).
- **`pnpm-workspace.yaml`**: refresh `minimumReleaseAgeExclude` from the `@1.1.1`/`zdtp@0.4.9` list to the new pins, or drop the block if `minimumReleaseAge` is not enforced here.

### 2. CSS and utilities

`doc/src/styles/global.css` (140 lines), by block:

| Lines | Content | v3 disposition |
| --- | --- | --- |
| 11-13 | `@layer zd-preflight, zd-flow;` + Tailwind preflight/utilities imports | imports are **ZW009**; take the 6.x template's head (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives) |
| 20 | `@import "@takazudo/zdtp/styles.css";` | keep the intent (panel is on). zfb 3.1.0 lists "Resolve authored CSS package subpaths through package `exports`" (https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/); zudo-doc's planning measured this exact import failing on 3.0.0. Verify on the adopted pin; fallback is the file path `@takazudo/zdtp/dist/zdtp.css`. 6.0's generator may inject the line itself - do not duplicate. |
| 54-69 | five `@import "@takazudo/zudo-doc/*.css"` | package-owned; take the 6.x list (`safelist.css` is a Tailwind `@source inline()` file slated for removal) |
| 84-88 | five `@source` globs (incl. `src/hooks/**`, `src/utils/**`, which do not exist) | **ZW009**; delete. zfb's source plan scans `pages components layouts content src` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives) |
| 102-123 | `:root { --font-futura: ...; --zdc-doc-title-font; --zdc-doc-title-weight }` | **keep**: authored custom properties (inert while `themePack: 'bauhaus'`, per the in-file comment) |
| 137-140 | `[data-header-logo] { font-family; font-weight: var(--font-weight-normal) }` | **keep, but review the tie**: it wins today because authored CSS comes after Tailwind utilities. v3 places utilities **after** authored CSS by default, and `.font-bold` (0,1,0) ties `[data-header-logo]` (0,1,0), so the package's `font-bold` would win. Options: `wind.utilities.placement: "before-authored"` (zfb 3.2.0+), raise specificity (`header [data-header-logo]`), or `!important` in authored CSS (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties). Currently inert under bauhaus. |

**Tokens the host's own markup needs** (from the audit's 29 unique dead candidates plus the `TEXTAREA_CLASS` constant at `playground.tsx:18-19`, which is checked as a class once it reaches `class={...}`):

| Family (https://zfb.takazudomodular.com/zudo-wind/tokens/) | Names used |
| --- | --- |
| `spacing` (as `p-`, `px-`, `py-`, `pb-`, `ml-`, `gap-`) | `hsp-xs`, `hsp-sm`, `hsp-md`, `hsp-lg`, `vsp-2xs`, `vsp-xs`, `vsp-sm`; numeric `outline-2`, `outline-offset-2`, `py-px` |
| `colors` (as `text-`, `bg-`, `border-`, `outline-`, with `/10`, `/20`, `/30`, `/50` opacity - the W10 color-opacity modifier, valid on `border-*` once the color token exists, https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#variants-and-grammar) | `muted`, `accent`, `danger`, `success`, `surface`, `code-bg`, `code-fg` |
| `fontSizes` / `fontWeights` / `fontFamilies` / `lineHeights` | `caption`; `normal`, `medium`, `semibold`; `mono`; `relaxed` |
| `radii` | `default` (bare `rounded`), `lg` |
| `wind.breakpoints` (not a token family: `{ lg: { minWidthPx: ... } }`, https://zfb.takazudomodular.com/zudo-wind/configuration/#fields) / variants | `lg` (`lg:grid-cols-2` is the audit's ZW002 **error**); `hover:`, `focus:`, `focus-visible:`, `last:` (https://zfb.takazudomodular.com/zudo-wind/variants/) |
| arbitrary value | `min-h-[10rem]` (supported, https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#variants-and-grammar) |

All of these are zudo-doc theme names; expect them from the 6.0 `wind` fragment. Only if a name is missing after the bump do you add it under `wind.tokens` (and declare its `--...` value in authored CSS), or under `wind.breakpoints` for `lg`.

**Reset**: `owned-v1` + a preflight delta is the planned package choice. The Playground renders two `<textarea>`, a `<button>` and a `<ul class="list-none">`; check control background/border-radius, `::placeholder`, and list markers on `/docs/playground` after the port (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#differences-from-tailwind-preflight).

**Audit noise**: `src/lib/lint-browser.ts` contributes 72 `auditInfo` lines (regex sources). `auditInfo` never trips `--fail-on`. On zfb 3.2.0+, `wind.sources.exclude: ["src/lib/**"]` can drop it from the plan (https://zfb.takazudomodular.com/zudo-wind/configuration/#source-exclusions-and-package-roots).

### 3. Components and islands

**`src/chrome-bindings.tsx`** (77 lines). Remove lines 1-2 (pragmas) and the `VNode` import; keep the `displayName` pin (the server name must equal `type.displayName ?? type.name`, https://zfb.takazudomodular.com/api/island/); replace the call form with the wrapper and `class`:

```tsx
import { Island } from "@takazudo/zfb";
import { defineChromeBindings } from "@takazudo/zudo-doc/chrome-bindings";
import Playground, { type PlaygroundLang } from "./components/playground";

(Playground as { displayName?: string }).displayName = "Playground";

export function PlaygroundIsland({ lang }: { lang?: PlaygroundLang } = {}) {
  return (
    <Island
      when="load"
      ssrFallback={
        <div class="zd-playground-placeholder rounded-lg border border-muted/30 bg-surface/50 p-hsp-md" data-playground-placeholder>
          <p class="text-caption text-muted">{lang === "ja" ? "インタラクティブな Playground を読み込み中…" : "Loading the interactive playground…"}</p>
        </div>
      }
    >
      <Playground lang={lang} />
    </Island>
  );
}

export const chromeBindings = defineChromeBindings({ mdxExtras: { Playground: PlaygroundIsland } });
```

`ssrFallback` existed because `preact/compat` hooks crashed in SSR (`chrome-bindings.tsx:15-24`). A zudo-react `Playground` has no hooks and reads no browser API at setup, so you may instead server-render it (drop `ssrFallback`); on hydration the browser's textarea values win (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Only `lang` (a string) crosses the island boundary - already JSON-safe (https://zfb.takazudomodular.com/api/island/).

**`src/components/playground.tsx`** (232 lines). Hook-by-hook (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/):

| Today | Port |
| --- | --- |
| `useState(DEFAULT_CODE)` etc. x4 (`:123-126`) | `const code = signal(DEFAULT_CODE)`, `configStr`, `results`, `configError` signals |
| `<textarea value={code} onInput={(e) => setCode(...)}>` x2 (`:156-163`, `:169-176`) | `<textarea id="pg-code" modelValue={code} class={TEXTAREA_CLASS} rows={7} />` - textarea is a supported `modelValue` control (https://zfb.takazudomodular.com/zudo-react/forms/) |
| `useRef(debounce)` + `useRef(isInitialMount)` + `useEffect([code, configStr])` (`:127-146`) | `getScope().effect(() => { void code.value; void configStr.value; const t = setTimeout(applyLint, 300); return () => clearTimeout(t); })` - the effect tracks both reads, its cleanup runs before each rerun; the initial-mount skip is unnecessary because the first run only re-lints the defaults (https://zfb.takazudomodular.com/zudo-react/scopes-and-lifecycle/) |
| `onClick={() => applyLint(code, configStr)}` (`:185`) | `on:click={applyLint}` reading `code.value`/`configStr.value` inside |
| `{configError && <p>...}` / nested ternary (`:177-181`, `:204-227`) | `Show when={hasError}` with `const hasError = computed(() => configError.value !== null)`, `fallback={() => ...}`; text bindings pass the signal (`{configError}`) for live updates (https://zfb.takazudomodular.com/zudo-react/conditionals-and-lists/) |
| `results.map((r, i) => <li key={...}>)` (`:210-225`) | `<For each={results} by={(r) => r.key}>{(item) => ...}</For>` - add a stable `key` when building `results` in `runLintSync`; `item` is a readonly signal, derive fields with `computed` |
| `className=` x22, `htmlFor` x2 | `class=`, `for=` (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/) |
| `spellcheck={false}` (`:162`, `:175`) | `spellcheck` is an enumerated HTML attribute and zudo-react accepts a string or a boolean for it ("enumerated values such as `spellcheck` accept strings or booleans", https://zfb.takazudomodular.com/zudo-react/components-and-jsx/). Keep `spellcheck={false}` and confirm the built HTML carries `spellcheck="false"`; fall back to the string `"false"` if it does not |
| `import { useState, useRef, useEffect } from 'preact/compat'` (`:14`) and the alias comment (`:3-13`) | `import { computed, For, getScope, Show, signal } from "@takazudo/zfb/zudo-react"`; keep `"use client"` (`:1`) |

`INITIAL_LINT` (`:119`) computed at module load, `runLintSync`, `parseLintConfig` and the `@/lib/lint-browser` import stay as they are.

**Ten dead shims** (`src/components/*.tsx` except `playground.tsx`): delete now. Three would fail type-checking without `@types`/`preact` JSX types; the seven re-exports target 5.x subpaths that 6.0 may rename. Nothing imports them.

**Route stubs** `pages/docs/[[...slug]].tsx`, `pages/[locale]/docs/[[...slug]].tsx`, `pages/index.tsx`: generator-owned, already allowlisted for one deliberate divergence (static `../../src/chrome-bindings` import instead of `virtual:zudo-doc-chrome-bindings`, needed for island scanner reachability per the in-file comments). Take the 6.x stubs and re-apply only that import substitution if 6.0 still needs it (ask in zudolab/zudo-doc#4473).

**MDX**: `<Playground />`, `<Playground lang="ja" />`, `<Note>`, `<Warning>` only; no raw `className`. Restarted ordered lists (`ol[start]`) and inline `<svg xmlns>` were renderer errors on zfb 3.0/3.1 (zudo-doc's planning probes, Takazudo/zudo-front-builder#3359/#3360) and are accepted since 3.2.0 (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/); sweep only if the adopted pin is older: `grep -rEn '^([2-9]|[1-9][0-9])\. ' doc/src/content`, `grep -rn '<svg' doc/src/content`.

**Design token panel**: `designTokenPanel: true` stays a zudo-doc setting; zdtp mounts as an opaque Preact widget inside a zudo-react island owned by zudo-doc (pattern: https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget). The host does nothing beyond the CSS import (section 2) and the pin.

### 4. md-wasm and other packages

- `@takazudo/zfb-md-wasm` 2.7.1 is unused by repo code (`grep -rn 'zfb-md-wasm\|jsxRuntime' doc --include=*.ts --include=*.tsx --include=*.mjs` -> `package.json` only); bump in lockstep and add it to `check-pin-parity.mjs`.
- The root linter is unaffected, but its **dogfood config** `.design-token-lint.json` scans `doc/src/**/*.{tsx,jsx,astro}`: after the port the markup uses `class="..."`, which the extractor already supports alongside `className` (`doc/src/content/docs/reference/api/index.mdx:281`), so coverage of `playground.tsx` continues. `TEXTAREA_CLASS` is a string constant and is not linted today either.
- `src/lib/lint-browser.ts` mirrors the linter for the browser; no engine code, no change.

### 5. Tests, CI, deploy

- `doc/scripts/run-b4push.sh` (8 steps: format:md, template-drift, pin-parity, wrangler-pin, `zfb check`, `zfb build`, html-validate, links) is mirrored job-for-job by `doc-preview.yml` (TESTING.md:97-113). Add `pnpm exec zfb wind audit --fail-on error` after `zfb check` in both (official checklist step 6).
- `doc/scripts/check-template-drift.sh` compares against `node_modules/create-zudo-doc/templates/{base,features}` for the features `i18n llmsTxt docHistory bodyFootUtil claudeResources imageEnlarge sidebarToggle docTags tagGovernance` (lines 72-88); `designTokenPanel`/`versioning` are deliberately not compared. Re-baseline `.template-drift-allowlist` (currently: `scripts/setup-doc-skill.sh`, both stubs, `global.css`, 4 favicons, `scripts/check-links.js`) on 6.x.
- `doc-deploy.yml`: `pnpm check` -> `pnpm build` -> `pnpm check:links` -> html-validate -> `doc-history-generate` -> writes `dist/.assetsignore` with `_worker.js` and `_zfb_inner.mjs` -> `wrangler deploy`. Verify the 3.x adapter's emitted file names before trusting `.assetsignore`; the Cloudflare source-map fix (#3480) shipped in `@takazudo/zfb-adapter-cloudflare` 3.2.0, pin that or later.
- `doc-preview.yml:334-339` deploys with `npx wrangler@<devDependencies.wrangler> versions upload --preview-alias pr-N`; keep that version equal to the pin the wrangler-pin check demands.
- Root `ci.yml` tests the linter on Node 20/22/24 and never touches `doc/`.

## Step-by-step plan

1. **Now - cleanup on 2.x:** `git rm doc/src/components/{ai-chat-modal,preset-generator,client-router-bootstrap,desktop-sidebar-toggle,doc-history,image-enlarge,sidebar-toggle,sidebar-tree}.tsx doc/src/components/content/*.tsx`; `pnpm --filter @takazudo/zudo-design-token-lint-doc remove @tailwindcss/vite tailwindcss @types/react`; set `doc/.zudo-doc.json.packageVersion` to `5.8.0`; `pnpm build:doc`.
2. **Now - 2.x/5.x catch-up:** pins to zfb `2.22.1` x4, zudo-doc family `5.28.2`, `create-zudo-doc 5.28.2`, zdtp `0.8.5`; `pnpm install`; `pnpm --filter ...-doc check:wrangler-pin` and move `wrangler` accordingly; `.zudo-doc.json` -> `5.28.2`; refresh `pnpm-workspace.yaml` excludes; `pnpm --filter ...-doc b4push`; re-baseline `.template-drift-allowlist`. Open a PR so `doc-preview.yml` builds it.
3. **Now - data prep:** add a stable `key` to `LintResult` items in `src/lib/lint-browser.ts`/`runLintSync` (needed by `For`).
4. **Wait for `@takazudo/zudo-doc@6.0.0`**; read zudolab/zudo-doc#4473; note the zfb pin and whether `preact` must stay for zdtp.
5. Bump zfb family + zudo-doc family + `create-zudo-doc` to the 6.0 pair; drop `preact-render-to-string` (and `preact` if allowed); `pnpm install`; `pnpm exec zfb --version`.
6. `pnpm --filter ...-doc check:template-drift`; adopt the 6.x `global.css` (then re-append lines 102-140), `tsconfig.json`, stubs (re-apply the static chrome-bindings import if still required).
7. Port `src/chrome-bindings.tsx` and `src/components/playground.tsx` (section 3). `pnpm --filter ...-doc check`.
8. `pnpm --filter ...-doc exec zfb wind audit --fail-on error` - expect zero `dead classes` once the 6.0 fragment supplies the tokens; any remaining ZW006 names go into a host `wind.tokens` override. Then `pnpm --filter ...-doc b4push`.
9. Browser pass with `pnpm dev:doc`: `/docs/playground` (type in both textareas, debounce, Lint button, violation list), `/ja/docs/playground`, design-token-panel open/close, version switcher to `/v/2.0/`, dark/light, header logo weight (`[data-header-logo]` tie), search.
10. Merge; `doc-deploy.yml` deploys; confirm `https://zudo-design-token-lint.takazudomodular.com/docs/playground`.

## Verification checklist

- [ ] `grep -rnE '@import\s+"tailwindcss|@source|@theme' doc/src` -> 0; `@import "@takazudo/zdtp/..."` resolves in `zfb build`.
- [ ] `grep -rn '@jsxImportSource\|from .preact\|preact/compat' doc/src doc/pages` -> 0.
- [ ] `grep -rn 'className=\|htmlFor=\|onClick=\|onInput=' doc/src/components doc/src/chrome-bindings.tsx doc/pages` -> 0.
- [ ] `check:pin-parity` (with md-wasm added), `check:wrangler-pin`, `check:template-drift` pass; `.zudo-doc.json` matches the installed zudo-doc.
- [ ] `zfb check`, `zfb wind audit --fail-on error`, `zfb build`, html-validate, links pass; `pnpm -w run test` still green for the linter.
- [ ] Playground: initial results equal `INITIAL_LINT`, edits re-lint after 300 ms, Lint button works, config error path renders, JA labels on `/ja/docs/playground`; built HTML has `spellcheck="false"` on both textareas.
- [ ] Design token panel mounts (zdtp Preact bundle) with no console errors beside zudo-react islands.
- [ ] `/v/2.0/` snapshot pages render; `wrangler deploy --dry-run` succeeds; `.assetsignore` names match the emitted worker files.

## Risks and open questions

- **Preset ordering**: zfb 3 with zudo-doc 5.x fails at config load; never bump zfb alone.
- **Pragma + installed Preact = `ZR_CHILD`**: because zdtp keeps `preact` installed, a missed pragma fails at render, not at resolve. Grep before every build.
- **zdtp CSS import path** on the adopted zfb pin (exports subpath vs `dist/zdtp.css`); and whether 6.0's generator injects the line itself.
- **Tie flip on `[data-header-logo]`** when bauhaus is switched off; `placement: "before-authored"` needs zfb 3.2.0 or later.
- **Oldest pins in the group**: 2.7.1 -> 3.x and 5.8.0 -> 6.0 cross many releases; the catch-up step is the mitigation, and `check:wrangler-pin` will move `wrangler` twice.
- **Form semantics**: `modelValue` makes the DOM value win on hydration; if you keep `ssrFallback` there is no hydration mismatch risk, if you drop it verify the first paint.
- **Not run here**: `zfb check`, template drift, builds, deploys, browser checks. Measured: audit (zfb 3.1.0), greps, file reads.

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/, https://zfb.takazudomodular.com/zudo-wind/configuration/, https://zfb.takazudomodular.com/zudo-wind/tokens/, https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/, https://zfb.takazudomodular.com/zudo-react/forms/, https://zfb.takazudomodular.com/zudo-react/conditionals-and-lists/, https://zfb.takazudomodular.com/zudo-react/scopes-and-lifecycle/, https://zfb.takazudomodular.com/zudo-react/components-and-jsx/, https://zfb.takazudomodular.com/zudo-react/api-reference/
- https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget, https://zfb.takazudomodular.com/api/island/
- https://zfb.takazudomodular.com/api/cli/ (`zfb wind`, `zfb preview`), https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/, https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/ (npm `latest` since 2026-10-04), https://zfb.takazudomodular.com/changelog/zfb-adapter-cloudflare/v3.2.0/, https://zfb.takazudomodular.com/zudo-wind/variants/
- zudolab/zudo-doc#4430, #4477, #4473; Takazudo/zudo-design-token-panel#1002; Takazudo/zudo-front-builder#3480, #3569, #3570
- Repo files: `doc/zfb.config.ts`, `doc/package.json`, `doc/.zudo-doc.json`, `doc/tsconfig.json`, `doc/src/styles/global.css`, `doc/src/chrome-bindings.tsx`, `doc/src/components/playground.tsx`, `doc/src/lib/lint-browser.ts`, `doc/pages/**`, `doc/scripts/{check-pin-parity.mjs,check-wrangler-pin.mjs,check-template-drift.sh,run-b4push.sh}`, `doc/.template-drift-allowlist`, `.design-token-lint.json`, `pnpm-workspace.yaml`, `.github/workflows/doc-{deploy,preview}.yml`

## Appendix: commands behind the counts

```sh
find . -name 'zfb.config.*' -not -path '*/node_modules*'                               # doc/zfb.config.ts
grep -rn "@/components\|/components/" doc/src doc/pages --include=*.ts --include=*.tsx  # 1 import + 2 comments
grep -rnoE 'useState|useEffect|useRef|useMemo|useCallback|dangerouslySetInnerHTML|className=|onClick=|onInput=|<Island|"use client"' \
  --include=*.ts --include=*.tsx doc/src doc/pages | grep -v src/content | sort | uniq -c
grep -rnE '@import\s+"tailwindcss|@tailwind|@theme|@apply|@source|@utility|@variant|@custom-variant|@plugin|@config|theme\(' \
  --include=*.css --include=*.ts --include=*.tsx --include=*.mjs doc
grep -rnE "from ['\"]preact|preact/hooks|@preact/signals|preact-render-to-string|@jsxImportSource" \
  --include=*.ts --include=*.tsx --include=*.js --include=*.jsx --include=*.mdx doc
for f in $(grep -rl 'className=' doc/src/content --include=*.mdx); do \
  awk -v F="$f" 'BEGIN{fence=0} /^```/{fence=!fence; next} !fence && /className=/{c++} END{if(c>0) print F": "c}' "$f"; done   # only inline-code lines
# audit (config swapped to `{ wind: { spec: 1 } }`, restored; `git status --short` empty)
zfb wind audit --project-root doc
grep -oE 'ZW0[0-9]{2}' audit.txt | sort | uniq -c              # ZW006 124, ZW012 48, ZW005 41, ZW001 31, ZW002 4, ZW004 3
```
