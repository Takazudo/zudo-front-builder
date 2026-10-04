# zfb v3 migration guide: Takazudo/mdx-formatter

Generated 2026-10-04 by an automated diagnosis of `main` @ `72c8ade` (v1.3.0, 2026-09-25); the wind audit used the zfb 3.1.0 CLI. zfb 3.2.0 was released on 2026-10-04 while this diagnosis was being written, so the audit was re-run with the 3.2.0 CLI and both results are reported below. Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**Blocked on zudo-doc 6.0.0, with real host-owned work that can start now. Effort M.** The npm package itself (`@takazudo/mdx-formatter`: TS + Rust napi + wasm) has no zfb dependency; only `doc/` is a zfb project (zudo-doc `^5.27.0`, zfb `2.20.2` exact — the oldest pin in this group). Unlike a bare scaffold, `doc/` owns one Preact island, **`doc/src/components/formatter-playground.tsx` (403 lines, `useState` x6, `useCallback` x2, 30 `className=`, 5 `onInput`, 2 `onClick`, 1 `onChange`, 2 `type="number"` inputs, 2 textareas)**, wired through `doc/src/chrome-bindings.tsx` as `<FormatterPlayground />` for `doc/src/content/docs/playground/index.mdx:10`. `zfb wind audit` reports **81 dead-class occurrences / 32 distinct candidates on 3.1.0 and 89 / 35 on 3.2.0** (3.2.0 traces the same-module `TEXTAREA_CLASS` const into its two `className=` positions): on 3.2.0, 85 ZW006 "missing token" (31 distinct) and 4 ZW002 (`sm:`/`lg:`/`xl:` breakpoints), plus 9 ZW005 (`border-muted/30` x7, `placeholder:text-muted/50` x2) that are an artifact of the tokenless stand-in config — with a `muted` colour token configured both resolve through the W10 colour-opacity modifier (measured with `zfb wind explain` on 3.1.0), so nothing needs rewriting. zudo-doc 6's preset tokens should cover the named tokens and the `sm`/`lg`/`xl` breakpoints (DD4 in zudolab/zudo-doc#4430) but **not** the numeric spacing `py-0.5` x4 / `w-14` x2 / `ml-5` (DD4: no `spacingUnit`) — the one host-side token decision. The island port to zudo-react can be prototyped today against zfb 3.2.0 (released 2026-10-04); the switch itself waits for zudo-doc 6.0.0 (zudolab/zudo-doc#4430 / PR #4477), because the preset still emits `framework`/`tailwind` (https://zfb.takazudomodular.com/guides/migrating-to-v3/#before-you-start). `@takazudo/zdtp` 0.8.2 is declared but unused (`designTokenPanel` is not configured; no `zdtp/styles.css` import).

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `doc/` (`my-docs`; `doc/zfb.config.ts` 149 lines, `defineConfig({ ...zudoDoc({...}) })`, locales `ja`, archived version `0x`, `claudeResources`, `chromeBindingsModule`) | `@takazudo/zfb` `2.20.2` exact, `-runtime` `2.20.2`, `-md-wasm` `2.20.2` (`doc/package.json:25-27`) | `@takazudo/zudo-doc` `^5.27.0`, `@takazudo/zudo-doc-history-server` `^5.27.0` (`:28,35`); `@takazudo/zdtp` `0.8.2` (`:34`) declared, **not used** (no `designTokenPanel`, no `@takazudo/zdtp/styles.css` import); no zudo-sg | generated `doc/src/styles/global.css` (`:8-9` tailwindcss imports, `:14` `safelist.css`, `:20-22` `@source`, `:26-28` empty `@theme {}`) + 32 distinct utility candidates in the playground (`gap-vsp-*`, `p-hsp-*`, `text-caption`, `bg-code-bg`, `rounded-lg`, `sm:grid-cols-2`, …); no `tailwind.config.*`, no `@tailwindcss/*`, no `ZFB_TAILWIND*` | `preact ^10.29.1`, `preact-render-to-string ^6.6.6` (`:30-31`); host code: `formatter-playground.tsx:3-4` (`preact` types, `preact/hooks`), pragmas in `chrome-bindings.tsx:1-2`, `pages/docs/[[...slug]].tsx:1-2`, `pages/[locale]/docs/[[...slug]].tsx:1-2`; `import type { JSX } from "preact"` x3 | 1 host island: `FormatterPlayground` (`'use client'`, `displayName` pinned) wrapped by `Island({ when: "load", children: <FormatterPlayground /> })` in `chrome-bindings.tsx:8-13`; registered as MDX extra | dep only (zudo-doc HtmlPreview); the playground loads the repo's own `mdx_formatter_wasm` from `public/wasm/`, not md-wasm | Cloudflare Worker `mdx-formatter`: root `wrangler.jsonc` (`main: ./worker/index.js` legacy-redirect worker, `assets.directory: ./doc/dist`, custom domain `mdx-formatter.takazudomodular.com` + `takazudomodular.com/pj/mdx-formatter*`); `main-deploy.yml` (push `main`), `pr-preview.yml` (`wrangler versions upload`), `doc/legacy-pages/_redirects` via `wrangler pages deploy` |
| repo root (`@takazudo/mdx-formatter` 1.3.0: `src/`, `crates/`, `wasm/`, `npm/*`, `benchmark/`) | none (`dependencies`: chalk, commander, glob, ignore, js-yaml) | none | none | none in product code; `benchmark/fixtures/{large,medium}.mdx` contain React hook samples **as formatter fixtures only** (grep false positives) | none | none | npm via `release.yml` — unaffected |

Measured with: `find . -name 'zfb.config.*'`, the briefing greps, `zfb wind audit --project-root doc` and `zfb wind explain --project-root doc -- <candidate>` (zfb 3.1.0, audit re-run with zfb 3.2.0; temporary `wind: { spec: 1 }` config, restored; `git status --short` clean).

## Sequencing and blockers

1. **zudo-doc 6.0.0** — zudolab/zudo-doc#4430, PR #4477, consumer guide #4473 (planned). Its blockers Takazudo/zudo-front-builder#3569/#3570 ship in **zfb 3.2.0 (released 2026-10-04)**, so 6.0.0 can pin 3.2.0; zudo-doc's side is unblocked (`base/zfb3-migration` @ `70e0875` is pushed — PR #4477 carries 102 commits — and only the #4467 integration topic remains local), but 6.0.0 itself is still unpublished. The host switch (steps 4-7 below) waits for it.
2. **Now, independent of zudo-doc** (the M-size part): port `formatter-playground.tsx` to zudo-react in a scratch zfb 3.2.0 project (`pnpm create zfb@latest`, copy the component, `pnpm build`, hydrate it in a browser with the WASM files from `pnpm build:wasm:doc`); decide the three numeric-spacing utilities (`py-0.5`, `w-14`, `ml-5`: host `spacingUnit` through the `zudoDoc({ wind })` override, named `hsp-*`/`vsp-*` spacing, or authored CSS); decide the zdtp dep. Keep the result on a branch (`topic/zfb3-playground`) until 6.0.0 lands.
3. **2.20.2 → 3.x crosses v2.21.0, v2.21.1, v2.22.0, v2.22.1.** None of those changelog pages carries a Breaking Changes section (`grep -i breaking docs/src/content/docs/changelog/zfb/v2.2{1,2}.*.mdx` → 0), so the only breaks to absorb are v3's. zudo-doc `^5.27.0` will already have pulled 5.28.x; nothing to pre-bump.
4. **After 6.0.0**: pins, scaffold re-copy (3 stubs + tsconfig + global.css), merge the playground branch, rebuild, deploy through the existing workflows.

## Required changes

### 1. Dependencies, config, tsconfig, env

- `doc/package.json:25-27` — zfb family to exact `3.2.0` (released 2026-10-04), or the later exact version zudo-doc 6.0.0 pins — never a `^3.1.0` range, which would still admit the 3.1.0 build with the UTF-8 offset panic (#3569); `:28,35` zudo-doc + history-server → `^6.0.0`. One root `pnpm-lock.yaml` covers `doc/` (workspace member via `pnpm-workspace.yaml`); there is no separate `doc/` lockfile.
- `doc/package.json:30-31` — delete `preact`, `preact-render-to-string` (present only for zfb's removed engine; https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). The playground is rendered by zudo-react after the port, so no widget peer remains.
- `doc/package.json:34` — `@takazudo/zdtp` `0.8.2`: unused today. Either delete it, or, if the design-token panel is wanted, wait for zudo-doc 6's zdtp decision (zudo-doc keeps zdtp as an opaque, lazily imported Preact bundle; zdtp#1002 asks it to bundle Preact and document a CSS import that works on zfb 3). Do not import `@takazudo/zdtp/styles.css` into `global.css` on v3 until that is settled.
- `doc/package.json:33` `katex`, `:40` `npm-run-all2` (`run-p`), `:41` `wrangler 4.120.0` — unrelated; leave.

  ```jsonc
  "dependencies": {
    "@takazudo/zfb": "3.2.0",
    "@takazudo/zfb-runtime": "3.2.0",
    "@takazudo/zfb-md-wasm": "3.2.0",
    // exact; or the later lockstep version zudo-doc 6.0.0 pins
    "@takazudo/zudo-doc": "^6.0.0",
    "@takazudo/zudo-doc-history-server": "^6.0.0",
    "zod": "^4.3.6",
    "katex": "^0.16.38",
    "diff": "^8.0.3"
    // removed: preact, preact-render-to-string; @takazudo/zdtp unless the panel is adopted
  }
  ```

- `doc/zfb.config.ts` — no host-side `framework`/`tailwind` key (both are emitted by `zudoDoc()` in 5.x). The spread form `defineConfig({ ...zudoDoc({...}) })` (`:5-6`) works the same as `defineConfig(zudoDoc(...))`. `:3` imports `DOC_BASE` from `./src/components/formatter-playground-config` — a plain `export const`, fine for the esbuild-neutral config graph. If the playground keeps any token the preset does not supply (today: `spacingUnit` for `py-0.5`/`w-14`/`ml-5`), add it through zudo-doc 6's planned consumer override `zudoDoc({ wind: {...} })` (DD4 in zudolab/zudo-doc#4430; confirm the shape in #4473) — never a top-level `wind` beside the spread, which would be merged with the preset's by zfb's user-wins rules (https://zfb.takazudomodular.com/zudo-wind/configuration/#strict-validation-and-merging).
- `doc/tsconfig.json:8-10` — remove the `react`/`react/jsx-runtime`/`react-dom` → `preact/compat` aliases; `jsx: "react-jsx"` + `jsxImportSource: "@takazudo/zfb/zudo-react"` should arrive from `@takazudo/zudo-doc/tsconfig.base.json` (6.0.0 plans to flip this consumer-shipped file); add locally only if it does not.
- Env/CI: `grep -rn ZFB_TAILWIND .github scripts package.json doc/package.json` → 0 hits. `.github/actions/build-zfb/action.yml` installs Rust + wasm-pack and runs `pnpm build:wasm:doc` before `pnpm check`/`pnpm build` in `doc/` — unchanged.

### 2. CSS and utilities

- `doc/src/styles/global.css` (28 lines) — ZW009 errors on v3 even under `wind: false` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives): `:8` `@import "tailwindcss/preflight" layer(zd-preflight)`, `:9` `@import "tailwindcss/utilities"`, `:14` `@import "@takazudo/zudo-doc/safelist.css"` (zudo-doc 6 plans to retire it for a `wind.json` manifest declared by the preset), `:20-22` three `@source`, `:26-28` empty `@theme {}`. Re-copy from `create-zudo-doc@6`; the host adds nothing to this file today.
- **Audit result** (`zfb wind audit --project-root doc`, zfb 3.1.0 and zfb 3.2.0, exit 0 on both; all dead-class occurrences — 81 on 3.1.0, 89 on 3.2.0 — are in `src/components/formatter-playground.tsx`; content contributes 0 — the 30 `class=`/`className=` strings in 8 MDX files are inside fenced examples):

  | Code | Count (strict origin) | Candidates | Resolution on v3 |
  | --- | --- | --- | --- |
  | ZW006 missing token | 77 occurrences / 28 distinct on 3.1.0; 85 / 31 on 3.2.0 (the `TEXTAREA_CLASS` candidates below become strict: +`text-code-fg`, `leading-relaxed`, `focus:border-accent` and 5 more hits of names already listed) | `text-caption` x14, `text-muted` x7, `text-fg` x5, `rounded` x4, `py-0.5` x4, `px-hsp-xs` x4, `font-semibold` x4, `bg-code-bg` x4, `gap-vsp-2xs` x3, `gap-hsp-xs` x3, `w-14` x2, `rounded-lg` x2, `p-hsp-md` x2, `gap-vsp-xs` x2, `gap-vsp-sm` x2, `gap-hsp-md` x2, `focus-visible:outline-accent` x2, `text-danger`, `text-bg`, `py-hsp-xs`, `px-hsp-xl`, `ml-5`, `hover:text-fg`, `hover:bg-accent-hover`, `font-mono`, `bg-surface`, `bg-accent`, `accent-accent` | zudo-doc theme names: `vsp-*`/`hsp-*` → `tokens.spacing`, `caption` → `fontSizes`, `fg`/`muted`/`surface`/`code-bg`/`accent`/`accent-hover`/`danger`/`bg` → `colors` (also `accent-accent`, the accent-color utility), `semibold` → `fontWeights`, `mono` → `fontFamilies`, bare `rounded` → `radii.default`, `rounded-lg` → `radii.lg` (https://zfb.takazudomodular.com/zudo-wind/tokens/). DD4 in zudolab/zudo-doc#4430 plans package-owned tokens as `var(--…)` for exactly these families, so expect the preset to cover them. **Exception:** `py-0.5` x4, `w-14` x2, `ml-5` x1 need `spacingUnit`, which DD4 says the preset will **not** declare — set it in the host override (`zudoDoc({ wind: { tokens: { spacingUnit: "0.25rem" } } })`), or rewrite them to named `hsp-*`/`vsp-*` spacing or authored CSS. |
  | ZW005 slash modifier | 8 errors + 1 auditInfo on 3.1.0; 9 errors on 3.2.0 (the `TEXTAREA_CLASS` hit becomes strict) | `border-muted/30` x6 at class position (lines 189, 209, 241, 265, 321, 344) + 1 inside `TEXTAREA_CLASS` (line 52, auditInfo), `placeholder:text-muted/50` x2 (lines 321, 344) | **No rewrite needed — an artifact of the tokenless stand-in config.** Without a `muted` colour token, `border-muted` and `text-muted` fall into non-colour grammar entries (border width, font size) that carry no colour-opacity modifier, so 3.1.0 and 3.2.0 both report ZW005 (3.2.0 words it `slash modifier is not supported`). With `tokens.colors.muted` configured, `zfb wind explain -- border-muted/30` resolves to `border-*-color: color-mix(in oklab, var(--zw-color-muted) 30%, transparent)` and `text-muted/50` to `color: color-mix(… 50% …)` (measured on 3.1.0) — the W10 colour-opacity modifier (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#variants-and-grammar). Keep the spellings; they ride on the `muted` colour token. |
  | ZW002 variant | 4 errors | `sm:grid-cols-2`, `sm:gap-hsp-md`, `xl:grid-cols-3`, `lg:grid-cols-2` (lines 210, 353) | Breakpoints are off until configured with `minWidthPx` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#variants-and-grammar); DD4 in #4430 names exactly `sm`/`lg`/`xl` for the preset. |
  | `TEXTAREA_CLASS` const (lines 51-52) | 3.1.0: 9 auditInfo (ZW006 x8, ZW005 x1) — **not in the dead-class list**; 3.2.0: the same 9 as strict-origin errors, included in the counts above | `min-h-[32rem]`, `w-full`, `resize-y`, `rounded-lg`, `border`, `border-muted/30`, `bg-code-bg`, `p-hsp-md`, `font-mono`, `text-caption`, `leading-relaxed`, `text-code-fg`, `focus:border-accent`, `focus:outline-none` | 3.1.0 does not trace a same-module `const` into its `className={TEXTAREA_CLASS}` positions, so these were audited as low-confidence literals; zfb 3.2.0 (released 2026-10-04) traces a same-module `const` into a class expression (https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/) and checks them like a direct class — the re-run confirms it (dead classes 81 → 89). Three are unique to this const and need tokens all the same (verified with `zfb wind explain`): `text-code-fg` → `colors.code-fg`, `leading-relaxed` → `lineHeights.relaxed`, `focus:border-accent` → `colors.accent`. |
  | resolved | — | `rotate-90`, `min-h-[32rem]`, `focus:outline-none`, `select-none`, `break-all`, `break-words`, `transition-transform`, `transition-colors`, `shrink-0`, `resize-y`, `cursor-pointer`, `grid-cols-1`, `flex-1`, `min-w-0`, `w-full`, `items-center`, `opacity-40`, `disabled:cursor-not-allowed`, `disabled:opacity-50`, `focus-visible:outline-2`, `focus-visible:outline-offset-2` | Verified with `zfb wind explain`; no change. |
  | auditInfo noise | 3.1.0: 44 lines (ZW005 x22, ZW006 x8, ZW001 x7, ZW012 x4, ZW002 x2, ZW004 x1); 3.2.0: 15 lines (ZW001 x7, ZW012 x4, ZW005 x3, ZW004 x1 — 3.2.0 skips module specifiers, URLs and non-class attribute values) | import specifiers (`@takazudo/zudo-doc/*`, `virtual:zudo-doc-*`, `preact/hooks`, `../../src/chrome-bindings`) in the two stubs, `pages/index.tsx` and `chrome-bindings.tsx`; `SAMPLE_INPUT` text (lines 7-34); the `type="text"`, `placeholder="Comp1, Comp2"` and `.split(',')` literals; the two `${DOC_BASE}wasm/...` URL templates (lines 78, 81); the `TEXTAREA_CLASS` const above | Low-confidence literals; never errors. |

  Host token override, only if the three numeric-spacing utilities are kept (shape per DD4 in zudolab/zudo-doc#4430; confirm in #4473):

  ```ts
  // doc/zfb.config.ts — inside the zudoDoc({...}) options, not beside the spread
  wind: { tokens: { spacingUnit: "0.25rem" } },
  ```

  Every variant the playground uses is admitted on v1: `placeholder:` (pseudo-element), `hover:`, `focus:`, `focus-visible:`, `disabled:` (state), `sm:`/`lg:`/`xl:` once configured (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#variants-and-grammar).
- Cascade: wind puts utilities **after** authored unlayered CSS by default (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties); this host has no authored rules competing with the playground utilities, so no flip risk. `wind.utilities.placement` shipped in zfb 3.2.0 (released 2026-10-04) if it is ever needed.
- Reset: zudo-doc 6 plans `owned-v1` plus an authored preflight patch (DD4 in #4430). The measured preflight → `owned-v1` differences are `::placeholder` (both textareas and two text inputs use `placeholder=`), `button`/`input`/`textarea` backgrounds, `sub`/`sup`, `small`, `hr`, `::file-selector-button` and `[hidden]` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#reset-differences-from-tailwind-preflight); whatever the patch does not restore shows up here first. The playground is this site's only form UI — check it explicitly.
- 3.1.0 prints byte offsets in the audit; `file:line:col` audit locations, `--json`, `--severity`, `--plan`, `--config`, and `wind.strict`/ZW014 shipped in zfb 3.2.0 (released 2026-10-04; https://zfb.takazudomodular.com/api/cli/#zfb-wind, https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/#migration-warnings-and-strict-mode, https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/). Measured: 3.1.0 rejects `wind.strict` and `wind.utilities` as unknown fields and accepts only `spec`, `reset`, `tokens`, `breakpoints`, `dark`, `defaultTransitionTimingFunction`, `safelist`, `authoredClasses`, `manifests`; `wind.sources` likewise arrived in 3.2.0. The 3.2.0 audit reports `spec: 1 revision 4` (3.1.0: revision 3) after its six new utility groups.

### 3. Components and islands

- `doc/src/chrome-bindings.tsx` (19 lines): delete the pragmas `:1-2`; keep `import { Island } from "@takazudo/zfb"` and the direct-call wrapper `Island({ when: "load", children: <FormatterPlayground /> })` — v3's scanner recognizes direct calls and fixed-target forwarding wrappers, registers the target (`FormatterPlayground`, default export of a `"use client"` module), not the wrapper (https://zfb.takazudomodular.com/concepts/islands/#boundary-discovery-and-migration); `when: "load"` is still the default strategy (https://zfb.takazudomodular.com/api/island/). The `displayName` pin at `formatter-playground.tsx:403` is allowed only because it equals the function name; delete it to avoid confusion.
- `doc/pages/docs/[[...slug]].tsx` (`:1-2` pragmas, `:33` `import type { JSX } from "preact"`) and `doc/pages/[locale]/docs/[[...slug]].tsx` (`:1-2`, `:29`; `:38` imports `chromeBindings` from `../../../src/chrome-bindings` for scanner reachability — keep that line when re-copying): replace with the 6.0.0 stubs; a Preact pragma in a page fails the v3 build after a `zfb warn:` (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). `doc/pages/index.tsx` is a 1-line re-export.
- **`doc/src/components/formatter-playground.tsx` → zudo-react** (the M-size item). Construct-by-construct (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/, https://zfb.takazudomodular.com/zudo-react/forms/, https://zfb.takazudomodular.com/zudo-react/components-and-jsx/):

  | Line(s) | Today | v3 |
  | --- | --- | --- |
  | 1, 3-4 | `'use client'`; `import type { ComponentChildren, JSX } from 'preact'`; `useCallback, useState` from `preact/hooks` | keep `"use client"`; `import { computed, getScope, Show, signal, type Child } from "@takazudo/zfb/zudo-react"`; return type is a description, not `JSX.Element` |
  | 158-163 | 6 `useState` (`input`, `output`, `isFormatting`, `error`, `settingsOpen`, `settings` object) | `signal()` each; split `settings` into one writable signal per field (8 `enabled` booleans + `indentSize`, `propsThreshold`, `containerComponents`, `blockComponents`) so checkboxes can take `modelChecked` |
  | 164-169 | `useCallback(updateSetting)` | plain assignments to the per-field signals; no memoization needed |
  | 171-184 | `useCallback(handleFormat)` async | plain `async` function; guard writes with `getScope().abortSignal.aborted` after the `await loadWasm()` |
  | 135-136, 143, 187, 189, 195, 209-210, 241, 265, 301, 321, 344, 353-355, 366-367, 382, 387, 392 | `className="…"` (static) | `class="…"` |
  | 148, 200 | template `className={`… ${cond ? 'x' : ''}`}` | `class={computed(() => …)}` |
  | 137-139 | `<input type="checkbox" checked={enabled} onChange={onToggle}>` | `<input type="checkbox" modelChecked={enabledSignal} />` (writable signal passed as a prop; reactive `checked` is rejected) |
  | 146-147 | `<fieldset disabled={!enabled}>` | `disabled={computed(() => !enabled.value)}` |
  | 192, 384 | `onClick={…}` | `on:click={…}` |
  | 193 | `aria-expanded={settingsOpen}` | `aria-expanded={settingsOpen}` (signal binding, as `aria-pressed={isDark}` in the docs) |
  | 208, 391 | `{settingsOpen && (…)}`, `{error && (…)}` | `<Show when={settingsOpen}>{() => …}</Show>`, `<Show when={computed(() => error.value !== null)}>{() => <span role="alert" class="…">{error}</span>}</Show>` |
  | 145 | `{children && (…)}` | static (children are known at setup) — a plain conditional is fine |
  | 231-242, 255-266 | `<input type="number" value={n} onInput={…}>` | **no numeric model in v1** (https://zfb.takazudomodular.com/zudo-react/forms/#models-must-be-writable): uncontrolled `defaultValue={String(indentSize.value)}` + `on:input={(e) => { indentSize.value = clamp(Number(e.currentTarget.value)); }}`; `min`/`max` stay |
  | 313-322, 336-345 | text `<input value={s} onInput={…} placeholder>` | `<input modelValue={containerComponents} placeholder="Comp1, Comp2" class="…" />` |
  | 358-364 | `<textarea value={input} onInput={…} spellcheck={false}>` | `<textarea id="pg-input" modelValue={input} spellcheck={false} class={TEXTAREA_CLASS} />` |
  | 370-377 | `<textarea value={output} readOnly placeholder>` | `<textarea id="pg-output" modelValue={output} readonly placeholder="…" />` (reactive `value` is rejected; `readOnly` → HTML spelling `readonly`) — or render `{output}` into a `<pre>` |
  | 355, 367 | `htmlFor="pg-input"` | `for="pg-input"` |
  | 386, 389 | `disabled={isFormatting \|\| !input.trim()}`, `{isFormatting ? 'Formatting...' : 'Format'}` | `disabled={computed(() => isFormatting.value \|\| input.value.trim() === "")}`, `{computed(() => isFormatting.value ? "Formatting..." : "Format")}` |
  | 123-155 `SettingRow` | props `enabled: boolean`, `onToggle`, `children?: ComponentChildren` | props `enabled: Signal<boolean>`, `children?: Child`; no `onToggle` (the model writes the signal) |
  | 51-52 `TEXTAREA_CLASS` const | string reaching `className={TEXTAREA_CLASS}` twice | unchanged spellings (see the §2 `TEXTAREA_CLASS` row); on 3.1.0 its candidates are audited as low-confidence literals; zfb 3.2.0 traces the same-module `const` into `class={…}` and checks it like a direct class |

  Skeleton of the state block:

  ```tsx
  "use client";
  import { computed, getScope, Show, signal } from "@takazudo/zfb/zudo-react";

  export default function FormatterPlayground() {
    const scope = getScope();
    const input = signal(SAMPLE_INPUT);
    const output = signal("");
    const isFormatting = signal(false);
    const error = signal<string | null>(null);
    const settingsOpen = signal(false);
    const indentSize = signal(2);
    const formatMultiLineJsx = signal(true);
    // …one signal per setting…
    const canFormat = computed(() => !isFormatting.value && input.value.trim() !== "");

    async function handleFormat() {
      isFormatting.value = true; error.value = null;
      try {
        const wasm = await loadWasm();
        if (scope.abortSignal.aborted) return;
        output.value = wasm.format(input.value, JSON.stringify(buildFormatterSettings(/* read signals */)));
      } catch (e) {
        if (!scope.abortSignal.aborted) error.value = e instanceof Error ? e.message : "An unexpected error occurred";
      } finally {
        isFormatting.value = false;
      }
    }
    // …JSX with class=, on:click=, modelValue=, modelChecked=, <Show>…
  }
  ```

  The WASM loader (`loadWasm`, lines 72-90, dynamic `import()` of `${DOC_BASE}wasm/mdx_formatter_wasm.js`) is framework-free and stays. Hydration adopts the server markup and the DOM value wins for the two textareas and two text inputs (https://zfb.takazudomodular.com/zudo-react/hydration/), so typing before activation is preserved.

### 4. md-wasm and other packages

- No `compile()`/`renderHtml()` calls anywhere in the repo; `@takazudo/zfb-md-wasm` is zudo-doc's HtmlPreview dependency. The `jsxRuntime` removal (https://zfb.takazudomodular.com/guides/migrating-to-v3/#mdx-and-md-wasm) is zudo-doc's concern.
- The product package (`src/`, `crates/mdx-formatter-napi`, `crates/mdx-formatter-wasm`, `npm/*`) does not depend on zfb; its CI (`ci.yml`, Node 18/20/22 + Rust) and `release.yml` are untouched. `benchmark/fixtures/*.mdx` keep their React samples — they are inputs to the formatter, not rendered.

### 5. Tests, CI, deploy

- `.github/actions/build-zfb/action.yml`: pnpm → Node → Rust → wasm-pack → `pnpm install --frozen-lockfile` → `pnpm build:wasm:doc` → root `pnpm build` → `doc/`: `pnpm check` + `pnpm build` → `dist/` HTML count. No command changes. Add after `Check zfb documentation site`:

  ```yaml
      - name: Audit utility candidates
        run: pnpm exec zfb wind audit --fail-on error
        shell: bash
        working-directory: ${{ inputs.working-directory }}
  ```

- `main-deploy.yml` / `pr-preview.yml` read the wrangler version from `doc/package.json` and deploy `doc/dist` + `doc/legacy-pages`; unchanged. The PR preview is the place to eyeball the ported playground before merge.
- `scripts/run-b4push.sh` steps 7-9 (`build:wasm:doc` → doc checks → doc build) cover it locally; `lefthook.yml` `prettier-doc` formats `doc/src/**/*.{ts,tsx,js,jsx,css}` — the ported component goes through it.
- `doc/CLAUDE.md:3,7,9-10,12` says "zudo-doc 5.27.0", "zfb 2.20.2", "Tailwind CSS v4", "Preact"; rewrite after the bump.

## Step-by-step plan

Adapted from the 7-step checklist (https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist).

1. **Now — prototype the island.** `pnpm create zfb@latest pg-proto` (zfb 3.2.0 scaffold), copy `doc/src/components/formatter-playground{,-config}.ts(x)` + the WASM files from `pnpm build:wasm:doc` into its `public/wasm/`, add placeholder tokens (`spacingUnit: "0.25rem"`, the `colors` incl. `code-fg`, `spacing`, `fontSizes` — a struct per entry, `fontSizes: { caption: { size: "0.75rem" } }`; a bare string is rejected with `expected struct WindFontSize` (measured on 3.1.0) — `fontWeights.semibold`, `fontFamilies.mono`, `lineHeights.relaxed`, `radii.default`/`radii.lg` from the §2 tables, `breakpoints: { sm: { minWidthPx: 640 }, lg: { minWidthPx: 1024 }, xl: { minWidthPx: 1280 } }`) in its `zfb.config.ts`, port per §3, `pnpm build`, open it, format the sample, toggle every setting. Run `zfb wind audit --fail-on error` there. Commit the component to `topic/zfb3-playground` in this repo (do not merge yet — it will not build on zudo-doc 5.x).
2. **Now — decide zdtp.** Remove `@takazudo/zdtp` from `doc/package.json:34` (unused) or open an issue to adopt the panel after zudo-doc 6.
3. **Wait for zudo-doc 6.0.0**; read #4473. Steps 4-7 run after it ships.
4. **Baseline + pins.** `git tag pre-zfb3`; keep `doc/dist`; edit `doc/package.json` per §1; `pnpm install`; `pnpm --dir doc exec zfb --version`.
5. **Re-copy scaffold files.** `pnpm create zudo-doc@6 <scratch>` with the same features (`claudeResources`, `docHistory`, `versions`, `locales ja`, `sidebarResizer`, `sidebarToggle`, `tocToggle`, `imageEnlarge`, `dynamicPageTransition`, `llmsTxt`, `cjkFriendly`); copy `app/pages/docs/[[...slug]].tsx`, `app/pages/[locale]/docs/[[...slug]].tsx` (re-add the `../../../src/chrome-bindings` import if the template imports the virtual module instead), `app/pages/index.tsx`, `app/tsconfig.json`, `app/src/styles/global.css`; merge `topic/zfb3-playground`; delete the pragmas in `chrome-bindings.tsx`; move any host token into the `zudoDoc()` wind override.
6. **Check, build, audit.** `cd doc && pnpm check && pnpm build && pnpm exec zfb wind audit --fail-on error`; fix remaining ZW006 (declare the token, or `spacingUnit` for `py-0.5`/`w-14`/`ml-5`) / ZW002 (breakpoint) items with `pnpm exec zfb wind explain -- <candidate>`; the former ZW005 slash-modifier hits resolve by themselves once `muted` is a configured colour.
7. **Verify + ship.** `pnpm b4push`; open the PR (`pr-preview.yml` URL): `/docs/playground/` format round-trip, settings toggles, number inputs clamp, `/ja/docs/overview/`, `/docs/0x/…` archived version, version switcher, search; merge → `main-deploy.yml`.

## Verification checklist

- [ ] Prototype: the ported `FormatterPlayground` builds on zfb 3.2.0, hydrates, formats `SAMPLE_INPUT`, every checkbox/number/text control round-trips; `zfb wind audit --fail-on error` exit 0 in the prototype.
- [ ] `pnpm --dir doc exec zfb --version` → 3.x + `embedded esbuild` only.
- [ ] `cd doc && pnpm check` green (config loads; no removed-key error); `pnpm build` green with no `ZW009`, no `zfb warn:` pragma lines, no `ZR_*` render diagnostics.
- [ ] `pnpm exec zfb wind audit --fail-on error` exit 0 in `doc/` (0 dead classes: preset tokens present, `spacingUnit` decided, `border-muted/30` and `placeholder:text-muted/50` resolving as colour-opacity utilities).
- [ ] `git grep -n "preact\|className=\|onClick=\|onInput=\|htmlFor=" doc/src doc/pages` → nothing.
- [ ] Browser (PR preview): playground in light/dark, `::placeholder` on both textareas, disabled-state opacity, focus rings (`focus-visible:outline-*`), `sm`/`lg`/`xl` grid breakpoints at 375/1024/1280px; JA page, archived `0x` page, version/language switchers, search, doc-history.
- [ ] `pnpm b4push` green; `main-deploy.yml` green incl. legacy redirects; `doc/CLAUDE.md` updated.

## Risks and open questions

- **Token names are zudo-doc's.** All 28 ZW006 candidates (plus `text-code-fg`, `leading-relaxed`, `focus:border-accent` from `TEXTAREA_CLASS`) are 5.x theme names (`vsp-*`, `hsp-*`, `caption`, `fg`, `muted`, `surface`, `code-bg`, `code-fg`, `accent`, `accent-hover`, `danger`, `bg`, `relaxed`). If zudo-doc 6 renames or drops any, the playground loses styling only at build time as ZW006 errors — good — but the fix then needs the `zudoDoc({ wind })` override (DD4 in #4430), whose final shape #4473 has yet to publish.
- **Numeric spacing has no preset home.** DD4 in #4430 declares no `spacingUnit`, so `py-0.5` x4, `w-14` x2 and `ml-5` stay ZW006 after the bump until the host sets `spacingUnit` in the `zudoDoc({ wind })` override or rewrites them (named `hsp-*`/`vsp-*` spacing or authored CSS). The 8 ZW005 slash-modifier hits are not a risk: they resolve once `muted` is a configured colour (measured on 3.1.0).
- **Numeric inputs have no model**; the uncontrolled `defaultValue` + `on:input` pattern means the displayed number is not corrected when the clamp changes the stored value — set `e.currentTarget.value` explicitly in the handler if the old behaviour matters.
- **Spread config** (`defineConfig({ ...zudoDoc() })`): adding a sibling `wind` key would be merged by zfb, not by zudo-doc; use the preset's override instead.
- **Oldest pin in the group** (zfb 2.20.2): no Breaking Changes sections in v2.21.0–v2.22.1, but zudo-doc `^5.27.0` resolves to 5.28.x already; run `pnpm install` once before the migration to make sure the lockfile is at the latest 5.x.
- Not verified here: `zfb check` cannot run on the clone (preset imports need `node_modules`); the audit used a stand-in `wind: { spec: 1 }` config (no tokens), so ZW006 counts are "tokens to declare", not failures.

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/ (checklist; `framework`/`tailwind`; pragmas; `rawHtml`; md-wasm; CLI/env)
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ (directives → ZW009; tokens pattern; W10 colour-opacity modifier and admitted variants; reset table; placement)
- https://zfb.takazudomodular.com/zudo-wind/configuration/ , https://zfb.takazudomodular.com/zudo-wind/tokens/ , https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/ , https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/ (same-module `const` tracing: zfb 3.2.0)
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/ , https://zfb.takazudomodular.com/zudo-react/forms/ , https://zfb.takazudomodular.com/zudo-react/components-and-jsx/ , https://zfb.takazudomodular.com/zudo-react/conditionals-and-lists/ , https://zfb.takazudomodular.com/zudo-react/scopes-and-lifecycle/ , https://zfb.takazudomodular.com/zudo-react/hydration/ , https://zfb.takazudomodular.com/zudo-react/api-reference/
- https://zfb.takazudomodular.com/concepts/islands/ (boundary discovery; direct-call `Island({ children })`; third-party widget pattern for zdtp), https://zfb.takazudomodular.com/api/island/ (`when`)
- https://zfb.takazudomodular.com/api/cli/ (`zfb wind audit --fail-on` in 3.1.0; `--json`/`--severity`/`--plan`/`--config`, `file:line:col` audit output and `zfb wind manifest` since zfb 3.2.0)
- https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/ (released 2026-10-04: #3365, #3367, #3370, #3569/#3570, `wind.utilities.placement`, same-module `const` tracing)
- zudolab/zudo-doc#4430 (epic), #4477 (root PR), #4473 (consumer migration guide, planned); Takazudo/zudo-front-builder#3569, #3570 (fixed in zfb 3.2.0); Takazudo/zudo-design-token-panel#1002 (zdtp bundling)
- This repo: `doc/zfb.config.ts`, `doc/package.json`, `doc/tsconfig.json`, `doc/src/styles/global.css`, `doc/src/chrome-bindings.tsx`, `doc/src/components/formatter-playground.tsx`, `doc/pages/docs/[[...slug]].tsx`, `doc/pages/[locale]/docs/[[...slug]].tsx`, `.github/actions/build-zfb/action.yml`, `.github/workflows/{main-deploy,pr-preview}.yml`, `scripts/run-b4push.sh`, `doc/CLAUDE.md`
