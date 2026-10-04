# zfb v3 migration guide: Takazudo/zudo-design-token-panel-example-zfb-tailwind

Generated 2026-10-04 by an automated diagnosis of `main` @ `69993b6` (2026-09-07), audited with the zfb 3.1.0 CLI. zfb 3.2.0 shipped during the diagnosis (2026-10-04 15:30 UTC; https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/), so the audit was re-run with the 3.2.0 CLI (both counts below) and every "next zfb release" label was resolved against its notes. Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**ready, effort M.** The Tailwind twin of `zudo-design-token-panel-example-zfb`: same 6 routes, same 3 islands and hooks inventory, same lazily-mounted `@takazudo/zdtp` Preact widget, plus a zfb-runtime `<ClientRouter>` and one `@theme` block that maps `--zfbtw-*` tokens into Tailwind namespaces. The audit found **87 distinct utility candidates / 607 ZW006 "missing token" hits** with the 3.1.0 CLI (89 / 630 with the 3.2.0 CLI, which also reads ternary and template class positions), and a scratch `wind.tokens` config written from the `@theme` block (§2) resolves every one of them on both CLIs via `zfb wind explain` — only four candidates need source edits (`flex-shrink-0` → `shrink-0`, `left-1/2` → `left-[50%]`, `cursor-help` and `decoration-dotted` → authored CSS) and one arbitrary-selector variant (`[.relative:hover_&]:opacity-100`) plus five runtime-built classes (`bg-${color}` …) must become authored CSS / a literal map. Two `<style>{...}</style>` blocks must become `rawHtml`. No preset is involved, so nothing upstream gates the start; target zfb 3.2.0 (released 2026-10-04, npm `latest`; exact pin, peer floor `^3.2.0`), which adds a built-in `leading-none`, `wind.strict`, `--json` audits and `explain --stdin`.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| repo root (zfb project: `pages/` 6 routes, `components/` 12 files, `content/prose/index.mdx`, `plugins/dev-apply-proxy.mjs`, `styles/global.css` 534 lines) | `@takazudo/zfb`, `@takazudo/zfb-runtime` **2.15.1** (`package.json:25-26`); `zfb.config.ts:37-50`: `framework: "preact"`, `base: "/"`, **`tailwind: { enabled: true }`**, 1 collection, `markdown.gfm` object form, 1 plugin | `@takazudo/zdtp 0.5.1` (package is 0.8.5) | yes, through zfb's embedded engine: `styles/global.css:144` `@import "tailwindcss"`, `:163-233` `@theme` (9 colors, 6 `--spacing-size-*`, 7 `--spacing-vsp-*`, 5 `--spacing-hsp-*`, 7 `--text-scale-*`, 6 semantic `--text-*`, 3 `--leading-*`, `--radius-md`, `--font-mono`); `@layer base` `:237-276`, `@layer components .zfbtw-prose` `:293-534`; **no** `@apply`/`@source`/`@utility`; no `tailwindcss` npm dep, no `tailwind.config.*`; `scripts/assert-theme-chain.mjs` asserts the built CSS chain in CI | `preact ^10.29.1`, `preact-render-to-string ^6.5.0`; `tsconfig.json:8` `jsxImportSource: "preact"`; hooks in 4 files, `import type { JSX } from "preact"` in `client-router-bootstrap.tsx:29` | 4: `app-shell.tsx:122` PanelMount (`when="visible" ssrFallback={null}`), `app-shell.tsx:113` ClientRouterBootstrap (`when="load" ssrFallback={null}`), `widgets/modal.tsx:172`, `widgets/tabs.tsx:138`; `<ClientRouter fallback="animate">` at `app-shell.tsx:62`; `data-zfb-transition-persist` on header/aside | no | assets-only Worker `zdtp-zfb-tailwind.zudolab.dev`; `.github/workflows/deploy.yml` build (`pnpm audit`, `zfb check`, `pnpm test:unit`, `zfb build`, **`assert:theme-chain`**, wrangler dry-run) → deploy / preview with routing-matrix assertion |

Audit (3.1.0 CLI, temporary `wind: { spec: 1 }`, config restored, clone clean): 5 unrecognized (`widgets-accordion`, `widgets-modal`, `decoration-dotted`, `zfbtw-easing-card-label`, `zfbtw-prose`), 0 conflicts, **607 dead-class hits = 87 distinct candidates, all ZW006 missing token** (the report repeats each hit under `diagnostics`, so a raw `grep -c ZW006` reads 1,239 incl. 25 `auditInfo`) (top: `text-fg` 66, `text-muted` 37, `text-helper` 37, `rounded-md` 35, `text-body` 33, `font-mono` 25, `px-hsp-md` 23, `border-muted` 23, `py-vsp-md` 21, `gap-vsp-xs` 19, `font-semibold` 18), 25 ZW012 dynamic constructions (5 real: `bg-`, `border-`, `text-` at `pages/components/status.tsx:75-85` and the `filled-`/`outlined-` key prefixes), 607 error-severity total. **Re-run with the 3.2.0 CLI (spec revision 4, same recipe):** 7 unrecognized (`widgets-accordion`, `widgets-modal`, `zfbtw-easing-card` ×2, `is-active`, `zfbtw-easing-card-label`, `zfbtw-prose` — the ternary at `pages/index.tsx:53` is now read as a class position; `decoration-dotted` left this list because `decoration-<color>` is a 3.2.0 utility root, so it is now a ZW006 error at `status.tsx:185:26`), 0 conflicts, **630 dead-class hits = 89 distinct, all ZW006** (new: `decoration-dotted`, `focus:border-accent`, `hover:bg-bg`; gone: `leading-none`, now built in; top: `text-fg` 68, `text-muted` 38, `rounded-md` 38, `text-helper` 37, `text-body` 36), 24 ZW012 (the same 5 real ones), 630 error-severity total; `auditInfo` noise drops to ZW001 42 / ZW005 15 / ZW002 5 / ZW004 5 / ZW006 2 because 3.2.0 skips module specifiers, URLs, non-class attribute values and inline style text, and every location is `file:line:col`. ZW001 (54) / ZW005 (54) / ZW002 (7) / ZW004 (6) are `auditInfo` from string literals (console messages, SVG `xmlns`, `zfb:before-swap`, transition strings). Unlike the plain twin, **no BEM `__` names** — the authored classes are single-hyphen.

Greps: Tailwind constructs 22 hits (2 live: `global.css:144,163`; the rest are comments, `assert-theme-chain.mjs`, and `tests/e2e/token-tweak-style.spec.ts` prose); `preact/hooks` at `panel-mount.tsx:74`, `modal.tsx:31-32`, `tabs.tsx:20`, `pages/index.tsx:35`; `dangerouslySetInnerHTML` `app-shell.tsx:85`; `onClick` ×5, `onKeyDown` ×1, callback ref `tabs.tsx:69`, `tabIndex` `tabs.tsx:64`, `charSet` `app-shell.tsx`; `<style>{`…`}</style>` at `accordion.tsx:47-60`, `modal.tsx:89-116`; camelCase style keys `forms.tsx:186,214,242` (`accentColor`); CSS-string styles (valid) at `avatar-row.tsx:25`, `media-card.tsx:21`, `status.tsx:211`, `pages/index.tsx:146`.

Commands used for the counts above (run from the clone root with the zfb 3.1.0 binary, then repeated with 3.2.0; the config swap is temporary — restore it and confirm `git status --short` is empty):

```sh
cp zfb.config.ts /tmp/zfb.config.bak && printf 'import { defineConfig } from "zfb/config";\nexport default defineConfig({ wind: { spec: 1 } });\n' > zfb.config.ts
zfb wind audit --project-root . > /tmp/audit.txt; cp /tmp/zfb.config.bak zfb.config.ts
grep -rnE '@import\s+"tailwindcss|@tailwind|@theme|@apply|@source|@utility|@variant|@custom-variant|@plugin|@config|theme\(' --include=*.css --include=*.ts --include=*.tsx --include=*.mjs --exclude-dir=node_modules --exclude-dir=dist .
grep -rnE "from ['\"]preact|preact/hooks|@jsxImportSource|dangerouslySetInnerHTML|onClick=|onKeyDown=|<Island|\"use client\"" --include=*.ts --include=*.tsx --exclude-dir=node_modules --exclude-dir=dist .
```

## Sequencing and blockers

1. **No upstream blocker**; `@takazudo/zfb@latest` is 3.2.0 (released 2026-10-04) — pin it exactly, peer floor `^3.2.0` (3.2.0 is a minor, so `^3.1.0` would still admit the 3.1.0 build-panic bug Takazudo/zudo-front-builder#3569). zdtp stays an opaque Preact bundle (zudo-doc DD3; zdtp#1002 would remove the `preact` peer later).
2. Do the plain example first (`Takazudo--zudo-design-token-panel-example-zfb.md`): the component port is identical and this repo adds only the utility/token layer and the client router.
3. **zfb 3.2.0** (released 2026-10-04; the label used below — the first draft called these "next zfb release"): built-in `leading-none` (spec revision 4; the `lineHeights.none` token becomes optional), `wind.utilities.placement: "before-authored"` if the Tailwind tie order must be preserved, `zfb wind audit --json`/`--severity`, `explain --stdin`/`--json`, `wind.strict`/ZW014 — https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/.
4. Independent: `@takazudo/zdtp` 0.5.1 → 0.8.5 in its own PR.

## Required changes

### 1. Dependencies, config, tsconfig, env

- `package.json:24-28`: zfb packages → `3.2.0` (exact; single root `pnpm-lock.yaml`); remove `preact-render-to-string`; keep `preact` as zdtp's peer. `:4` description and `predev` `zfb-tailwind-entry-*.css` cleanup are stale.
- `zfb.config.ts:38,40`: delete `framework: "preact"` and `tailwind: { enabled: true }` (both hard errors, https://zfb.takazudomodular.com/guides/migrating-to-v3/#configuration); add the `wind` block below. It was loaded without validation errors by the 3.1.0 and 3.2.0 CLIs and every candidate in the audit resolved against it (3.1.0: `zfb wind explain --project-root <dir> -- <candidate>`, one call per candidate; 3.2.0 adds `--stdin`, one candidate per line, and `--json`).

```ts
import { defineConfig } from "@takazudo/zfb/config";
export default defineConfig({
  base: "/",
  collections: [{ name: "prose", path: "content/prose" }],
  markdown: { gfm: { strikethrough: true, table: true } },
  plugins: [{ name: "./plugins/dev-apply-proxy.mjs" }],
  wind: {
    spec: 1,
    reset: "owned-v1",                       // see §2 reset
    defaultTransitionTimingFunction: "cubic-bezier(0.4, 0, 0.2, 1)", // Tailwind 4 default
    tokens: {
      spacingUnit: "0.25rem",                // w-16 / h-16 swatches
      colors: { primary: "var(--zfbtw-color-primary)", accent: "var(--zfbtw-color-accent)",
        surface: "var(--zfbtw-color-surface)", muted: "var(--zfbtw-color-muted)",
        success: "var(--zfbtw-color-success)", warning: "var(--zfbtw-color-warning)",
        danger: "var(--zfbtw-color-danger)", bg: "var(--zfbtw-bg)", fg: "var(--zfbtw-fg)" },
      spacing: { "vsp-2xs": "var(--zfbtw-vsp-2xs)", "vsp-xs": "var(--zfbtw-vsp-xs)", "vsp-sm": "var(--zfbtw-vsp-sm)",
        "vsp-md": "var(--zfbtw-vsp-md)", "vsp-lg": "var(--zfbtw-vsp-lg)", "vsp-xl": "var(--zfbtw-vsp-xl)", "vsp-2xl": "var(--zfbtw-vsp-2xl)",
        "hsp-xs": "var(--zfbtw-hsp-xs)", "hsp-sm": "var(--zfbtw-hsp-sm)", "hsp-md": "var(--zfbtw-hsp-md)",
        "hsp-lg": "var(--zfbtw-hsp-lg)", "hsp-xl": "var(--zfbtw-hsp-xl)" },
      sizes: { "size-sidenav-w": "var(--zfbtw-size-sidenav-w)", "size-header-h": "var(--zfbtw-size-header-h)",
        "size-avatar-sm": "var(--zfbtw-size-avatar-sm)", "size-avatar-md": "var(--zfbtw-size-avatar-md)",
        "size-icon-sm": "var(--zfbtw-size-icon-sm)", "size-icon-md": "var(--zfbtw-size-icon-md)" },
      fontSizes: { "page-title": { size: "var(--zfbtw-text-page-title)" }, "section-title": { size: "var(--zfbtw-text-section-title)" },
        "subsection-title": { size: "var(--zfbtw-text-subsection-title)" }, body: { size: "var(--zfbtw-text-body)" },
        helper: { size: "var(--zfbtw-text-helper)" }, annotation: { size: "var(--zfbtw-text-annotation)" },
        "scale-xs": { size: "var(--zfbtw-scale-xs)" }, "scale-sm": { size: "var(--zfbtw-scale-sm)" }, "scale-base": { size: "var(--zfbtw-scale-base)" },
        "scale-md": { size: "var(--zfbtw-scale-md)" }, "scale-lg": { size: "var(--zfbtw-scale-lg)" }, "scale-xl": { size: "var(--zfbtw-scale-xl)" }, "scale-2xl": { size: "var(--zfbtw-scale-2xl)" } },
      fontFamilies: { mono: "var(--zfbtw-font-mono)" },
      fontWeights: { semibold: "600", bold: "700" },
      lineHeights: { tight: "var(--zfbtw-leading-tight)", snug: "var(--zfbtw-leading-snug)", relaxed: "var(--zfbtw-leading-relaxed)", none: "1" }, // `none` needed on 3.1.0 only; 3.2.0 ships leading-none built in
      radii: { md: "var(--zfbtw-radius)" },
    },
  },
});
```
Token families and `--zw-*` emission: https://zfb.takazudomodular.com/zudo-wind/tokens/ (`colors`/`fontSizes` names must be disjoint for `text-*` — they are; `spacing`/`sizes` disjoint — they are). `var()` references are accepted for every family except `spacingUnit` and are reported as category-unverified by `explain`.
- `tsconfig.json:7-8`: keep `"jsx": "react-jsx"`, set `"jsxImportSource": "@takazudo/zfb/zudo-react"`.
- `.npmrc` `public-hoist-pattern[]=hono`: same note as the plain twin — keep until `zfb dev` is verified without it.
- Env/CI: no `ZFB_TAILWIND_*` (`grep -rn ZFB_TAILWIND .github scripts package.json` → 0).

### 2. CSS and utilities

- **Delete** `styles/global.css:144` `@import "tailwindcss"` and `:163-233` `@theme { … }` — both ZW009 (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives). Keep `:33-136` `:root { --zfbtw-* }` (the panel rewrites these; `scaffold.routing.json` maps `zfbtw` → this file), `:237-276` `@layer base`, `:293-534` `@layer components` — "Keep the existing base and component layers; wind's prelude establishes their order".
- `:141` `@import '@takazudo/zdtp/styles.css'`: move to line 1 (standard `@import` placement); the `exports`-subpath form failed on 3.0.0 (zdtp#1002) and is listed as resolved in https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/ — if the build still rejects it, use `@takazudo/zdtp/dist/zdtp.css` or drop it (self-injected).
- **Reset**: the project relied on Tailwind's preflight plus its own `@layer base`. `owned-v1` is closest (border-box, zero margins, list markers removed — `.zfbtw-prose :where(ul/ol)` already re-adds `disc`/`decimal`, headings lose UA sizes — every heading here carries a `text-*` utility or a `.zfbtw-prose` rule). Add the measured preflight-parity block from https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#differences-from-tailwind-preflight to `@layer base` if `::placeholder`, `button` appearance or `[hidden]` must match the 2.x pixels; `tabs.tsx` relies on the `hidden` attribute with no display utility, so the UA rule suffices.
- **Candidates needing source edits** (from `zfb wind explain` on 3.1.0): `flex-shrink-0` ×2 (`components/data/profile-card.tsx:21,28`) → `shrink-0` (resolved); `left-1/2` (`pages/components/status.tsx:197` tooltip) → `left-[50%]` (`explain` reports ZW005 "slash modifier is not supported": W11 fractions apply only to sizing/translate roots, https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#variants-and-grammar); `cursor-help` (ZW006 unknown value `help`, `status.tsx:185`) and `decoration-dotted` (an ordinary class on 3.1.0; on **zfb 3.2.0** `decoration-<color>` is a utility root, so it becomes ZW006 "unknown value or token dotted" at `status.tsx:185:26` — either way no `decoration-<style>` utility exists) → authored CSS (`cursor: help; text-decoration: underline dotted`); `[.relative:hover_&]:opacity-100` (`status.tsx` tooltip bubble) → authored rule such as `.zfbtw-tooltip:hover .zfbtw-tooltip-bubble { opacity: 1 }` (arbitrary-selector variants are ZW004; single-hyphen or BEM `__` names are both fine on **zfb 3.2.0**, where underscores are ordinary — on 3.1.0 `__` was a ZW001 error). `leading-none` (`status.tsx` Tag close button) resolves through the `lineHeights.none` token above on 3.1.0 and is a built-in utility on **zfb 3.2.0** (`explain` → `v1.leading`; a declared token still wins).
- **Runtime-built classes** `pages/components/status.tsx:75-85`: `` `bg-${color}` ``, `` `border-${color}` ``, `` `text-${color}` `` never become utilities ("A prefix joined to a runtime value does not become a utility"). Replace with a finite literal map keyed by the four `BadgeColor`s, e.g. `const FILLED = { accent: "bg-accent text-bg …", success: "bg-success text-bg …", … }` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#what-is-not-supported-and-what-to-do-instead). `forms.tsx:33` `inputBase` const is fine (a const string reaching `class` is checked like a literal).
- **Placement/ties**: the `@layer components .zfbtw-prose` rules lose to every utility under either placement; the showcase block in `pages/prose.tsx:84-91` is deliberately outside `.zfbtw-prose`, so no tie flips. `wind.utilities.placement: "before-authored"` (**zfb 3.2.0**) is not needed.
- Verify: `zfb wind audit --fail-on error` (since 3.1.0) should report 0 errors once the tokens are in place and the four source edits are done — the 3.2.0 CLI run against the scratch token config reports exactly those four (`flex-shrink-0` ×2, `decoration-dotted`, `cursor-help`); `--json`/`--severity` exist from 3.2.0.

### 3. Components and islands

Identical to the plain twin for `panel-mount.tsx` (lazy `import('@takazudo/zdtp')` inside `onActivate`, https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget), `modal.tsx`, `tabs.tsx`, `pages/index.tsx` `EasingDemoCard`, the `IslandProps['children']` casts (droppable: `children`/`ssrFallback` are typed `VNode`, a loose union incl. `null` and `object`, in 3.1.0) and `ssrFallback={null}` check — see that guide's §3. Cast sites here: `app-shell.tsx:114,123`, `modal.tsx:173`, `tabs.tsx:139`. Deltas here:

- **`components/app-shell.tsx:62`** `{ClientRouter({ fallback: 'animate' }) as unknown as preact.JSX.Element}` → `<ClientRouter fallback="animate" />` (https://zfb.takazudomodular.com/concepts/client-side-routing/#mounting-clientrouter). `@takazudo/zfb-runtime` 3.1.0 still exports `.` and `./client-router`.
- **`components/client-router-bootstrap.tsx`**: the v3 doc says mounting `<ClientRouter />` already makes the island scanner ship `@takazudo/zfb-runtime/client-router` ("no `"use client"` boilerplate needed"), so this island and its `Island when="load"` wrapper at `app-shell.tsx:113-115` can likely be deleted; if kept, drop `import type { JSX } from "preact"` and the `displayName` assignment (it cannot rename a target). Verify soft navigation + `data-zfb-transition-persist` on `<header>`/`<aside>` after the change.
- **`<style>{`…`}</style>` text children are rejected** (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/): `widgets/accordion.tsx:47-60` and `widgets/modal.tsx:89-116` → either `<style rawHtml={ACCORDION_CSS} />` with a static string, or move the rules into `styles/global.css @layer components` as the plain twin already does (preferred — one source of truth, and the panel-tweakable `--zfbtw-easing-*` vars still resolve).
- **Style objects**: `forms.tsx:186,214,242` `style={{ accentColor: … }}` → `"accent-color"`; the tabs indicator `style={{ width, transform, transition }}` becomes one `computed()` object as in the plain guide.
- `charSet` → `charset` (`app-shell.tsx`); `dangerouslySetInnerHTML` (`:85`) → `rawHtml`.

### 4. md-wasm and other packages

- No md-wasm usage. `@takazudo/zdtp` as in the plain twin (types from `@takazudo/zdtp/astro`; `preact` peer until zdtp#1002).

### 5. Tests, CI, deploy

- **`scripts/assert-theme-chain.mjs:50-82`** encodes the Tailwind chain (`.gap-vsp-lg { gap: var(--spacing-vsp-lg) }` + `--spacing-vsp-lg: var(--zfbtw-vsp-lg)`). Under zudo-wind the emitted variable namespace is `--zw-` (https://zfb.takazudomodular.com/zudo-wind/tokens/#emitted-variables): rows become `utility: "gap-vsp-lg", themeVar: "--zw-spacing-vsp-lg"`, `px-hsp-md` → `--zw-spacing-hsp-md`, `text-scale-xs` → `--zw-font-size-scale-xs`, `h-size-header-h` → `--zw-size-size-header-h` (the `sizes` map prefix plus the token name; rename the tokens to `header-h` etc. only if you also rename the classes in markup). The token layer is `@layer zw-tokens`. Keep the script — it is exactly the "utility layer came out empty" guard the v3 switch needs — and rename the workflow step `deploy.yml:109-110` "Assert Tailwind @theme chain".
- `deploy.yml:97-98`: after `pnpm build`, add `pnpm exec zfb wind audit --fail-on error` (https://zfb.takazudomodular.com/api/cli/#zfb-wind) so a missing token fails CI the way the chain assert does.
- `tests/e2e/token-tweak-style.spec.ts`: assertions read computed `fontSize`/`gap`/`paddingLeft` in px — they stay valid because `px-hsp-md` resolves `var(--zw-spacing-hsp-md) → var(--zfbtw-hsp-md)`; only the comments (l.12-26, 143-231) describe `--spacing-*`. `tests/plugin/dev-apply-proxy.test.mjs` (`pnpm test:unit`) is framework-free.
- Routing-matrix preview assertion (`deploy.yml:331-420`) and `wrangler.toml` are unchanged by v3; they are the post-migration smoke.
- `README.md`, `package.json` description, `zfb.config.ts` header comment ("@tailwindcss/vite … `@theme`") must describe `wind.tokens` instead; the zdtp docs page `frameworks-comparison.mdx` §"zfb + Tailwind v4" quotes this repo and should be rewritten once this lands (tracked in the `zudo-design-token-panel` guide).

## Step-by-step plan

1. Land the plain twin first, then branch here (`topic/zfb3`); bump to zfb 3.2.0 (exact), drop `preact-render-to-string`, `pnpm install`, `pnpm exec zfb --version`.
2. `zfb.config.ts`: remove `framework`/`tailwind`, paste the `wind` block (§1); `pnpm typecheck` (= `zfb check`) must pass the removed-key check.
3. `styles/global.css`: delete l.144 and l.163-235, hoist the zdtp import, add tooltip/cursor rules; `pnpm exec zfb wind audit --fail-on error` → expect only the four candidates in §2 until the source edits in step 4.
4. Source edits: `shrink-0`, `left-[50%]`, tooltip classes, Badge literal map, `<style>` → global.css/`rawHtml`, `accent-color` keys; re-run the audit → 0 errors.
5. Port islands and `app-shell.tsx` as in the plain guide; replace `ClientRouter(...)` with JSX; try deleting `client-router-bootstrap.tsx`; `pnpm typecheck`.
6. `pnpm build && node scripts/assert-theme-chain.mjs --self-test` (after rewriting its rows) and `pnpm run assert:theme-chain`.
7. `pnpm preview`: check `/`, `/prose/`, `/components/{forms,status,widgets,data}/`; open the panel; tweak `--zfbtw-vsp-lg` and watch `gap-vsp-lg` elements; soft-navigate between routes (view transition, persisted header/aside).
8. `pnpm test:unit && pnpm test:e2e`; update docs/comments; push; let `deploy.yml` preview assert the routing matrix.
9. Later: `@takazudo/zdtp` 0.5.1 → 0.8.5; drop `preact` after zdtp#1002; on zfb 3.2.0 you may remove `lineHeights.none` (built-in `leading-none`) and consider `wind.strict: true`.

## Verification checklist

- [ ] `zfb --version` 3.2.0; `zfb check` clean; `zfb wind audit --fail-on error` exits 0.
- [ ] `grep -nE '@import "tailwindcss|@theme|<style>\{' styles components pages` → 0.
- [ ] Built CSS contains `@layer zw-tokens` with `--zw-spacing-hsp-md: var(--zfbtw-hsp-md)` and `.px-hsp-md` using it (`assert-theme-chain.mjs` rewritten rows pass; its `--self-test` still fails when it should).
- [ ] `pnpm test:e2e`: token-tweak spacing/font-size/palette specs green; panel opens on all six routes.
- [ ] Soft navigation works with `<ClientRouter fallback="animate" />` alone (bootstrap island removed) and header/aside persist.
- [ ] Badges render all four colors filled and outlined (literal map complete); tooltip shows on hover; Tag close button has `line-height: 1`.
- [ ] Visual check against the 2.x deploy at `::placeholder`, `<select>`/`<input type=range>` appearance, `<hr>`, list markers in `/prose/` (reset differences).

## Risks and open questions

- Reset parity: `owned-v1` ≠ preflight; the measured differences (`::placeholder`, `::file-selector-button`, `small`, `sub/sup`, `[hidden]`) are documented and the parity block is opt-in — decide per element after a visual diff of `/components/forms/`.
- `--zw-size-size-header-h` naming is ugly but churn-free; renaming tokens means touching ~10 class sites and the chain script.
- Whether `client-router-bootstrap.tsx` is still needed is a documented behavior ("the build notices the import … no `"use client"` boilerplate") that this repo contradicted on 2.15.1 for a race-condition reason (`client-router-bootstrap.tsx:18-21`); test the first-click race before deleting.
- `[.relative:hover_&]` tooltip and `decoration-dotted` move to authored CSS, so the panel's easing tweak still applies through the inline `transition` string (`status.tsx:211`) — unchanged.
- `return null` from `PanelMount` and `ClientRouterBootstrap` is a documented "empty value" return (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/#child-values); the open question is only whether a DOM-less island hydrates cleanly — one browser run settles it. `ssrFallback={null}` is confirmed to keep skip-SSR in 3.1.0 (`packages/zfb/src/island-boundary.ts:55`, `skipSsr: fallback !== undefined`).
- Audit `--json`, `explain --stdin`, ZW014/`wind.strict`, built-in `leading-none`: all shipped in **zfb 3.2.0**; none blocked anyway. The 3.2.0 scanner reads more class positions (ternaries, `clsx`-style helpers), so expect slightly larger candidate counts than the 3.1.0 numbers in §Current state.

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/ · https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/
- https://zfb.takazudomodular.com/zudo-wind/configuration/ · https://zfb.takazudomodular.com/zudo-wind/tokens/ · https://zfb.takazudomodular.com/zudo-wind/variants/ · https://zfb.takazudomodular.com/zudo-wind/cascade-and-reset/ · https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget · https://zfb.takazudomodular.com/api/island/ · https://zfb.takazudomodular.com/concepts/client-side-routing/
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/ · https://zfb.takazudomodular.com/zudo-react/components-and-jsx/ · https://zfb.takazudomodular.com/zudo-react/forms/
- https://zfb.takazudomodular.com/api/define-config/ · https://zfb.takazudomodular.com/api/cli/#zfb-wind · https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/ · https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/ (npm `latest` since 2026-10-04)
- Takazudo/zudo-design-token-panel#1002 · Takazudo/zudo-front-builder#3365 (BEM `__`, shipped in 3.2.0) · sibling guides `Takazudo--zudo-design-token-panel-example-zfb.md`, `Takazudo--zudo-design-token-panel.md`
