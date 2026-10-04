# zfb v3 migration guide: Takazudo/claude-settings

Generated 2026-10-04 by an automated diagnosis of `main` @ `939a657`. Counts come from the commands listed; re-run them locally before relying on them.

This repo is the **source of truth** for the `dev-basic-cloudflare-webapp` skill. Two other repos carry copies of the same skill and are covered by sibling guides: `Takazudo/claude-resources` (public mirror, byte-identical, refreshed by `/claude-resources-share`) and `Takazudo/cloudflare-app-bootstrap-skill` (standalone Japanese translation, hand-ported). Fix it here first, then propagate.

## Verdict

**Small, effort M, no external gate for the template itself.** `claude-settings` is not a zfb project: it has no `zfb.config.*`, no pages, components or CSS, so there is nothing to migrate at runtime and `zfb wind audit` / `zfb check` cannot run. What it ships is a **scaffold template that is broken since 2026-09-30**: `skills/dev-basic-cloudflare-webapp/assets/templates/package.json:40-42` pins `@takazudo/zfb`, `@takazudo/zfb-adapter-cloudflare` and `@takazudo/zfb-runtime` to the dist-tag `"latest"`, which npm now resolves to **3.2.0** for all three (`npm view … dist-tags` on 2026-10-04; 3.0.0 was published 2026-09-30, 3.1.0 on 2026-10-01 and 3.2.0 on 2026-10-04 15:30 UTC, so a pin written this morning as `3.1.0` is already one minor behind), while the same file still installs `preact ^10.29.1` + `preact-render-to-string ^6.6.6` (`:43-44`), `tsconfig.json:9` sets `"jsxImportSource": "preact"`, and the skill prose tells the agent to build "zfb + Preact + Tailwind v4" with `framework: "preact"`, `@import "tailwindcss"` and `@theme` (`SKILL.md:34`, `references/stack.md:20,22,54`). Every project scaffolded from the skill today therefore gets a v3 binary plus v2 instructions and fails on the first `zfb dev`/`zfb build` with the removed-key errors and ZW009 (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#configuration, #stylesheets). The fix is a template + prose update against zfb's own v3 scaffold (`crates/zfb/templates/basic-blog`, live at https://create-zfb.takazudomodular.com): ~3 config files, 1 new shim file, 5 reference docs, 1 `SKILL.md` table. Effort M only because the prose spans six files and the skill should start delegating the site layer to `pnpm create zfb@latest`. The **Docs-site shape row** (`SKILL.md:35`, zudo-doc) stays on zudo-doc 5.28.2 / zfb 2.22.1 until **zudo-doc 6.0.0** ships (zudolab/zudo-doc#4430, root PR #4477, consumer guide #4473) — that row needs an explicit "do not mix with zfb 3" caveat now and a rewrite later. An interim one-line fix (pin the three packages to `2.22.1` exact, the last 2.x) restores a working scaffold in minutes if the full update must wait.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `skills/dev-basic-cloudflare-webapp/assets/templates/` (17 files: `package.json`, `tsconfig.json`, `wrangler.toml`, `pnpm-workspace.yaml`, `npmrc`, `gitignore`, `prettierignore`, `.prettierrc.json`, `lefthook.yml`, `cloudflare-setup.md`, 3 workflows, 4 scripts) | `"latest"` ×3 (`package.json:40-42`) → resolves 3.x since 2026-09-30 (3.2.0 today); scripts `dev`/`build`/`preview` = `zfb dev`, `zfb build`, `zfb preview` (`:12-14`), `typecheck` = `zfb check` (`:17`); **no `zfb.config.*`** in the template | none as deps; `SKILL.md:35` and `stack.md:21` recommend zudo-doc for the docs shape; `stack.md:147` lists `@takazudo/zdtp`; `@takazudo/zudo-design-token-lint: latest` (= 2.1.0) in devDeps (`:51`) | no CSS file, no `tailwind.config.*`, no `@tailwindcss/*` dep; **`gitignore:14-15`** keeps a `zfb-tailwind-entry-*.css` entry (a v2 temp-file name; zero hits in zfb v3 source); 0 `ZFB_TAILWIND*` in scripts/workflows | `preact ^10.29.1`, `preact-render-to-string ^6.6.6` (`package.json:43-44`); `tsconfig.json:8-9` `"jsx": "react-jsx"`, `"jsxImportSource": "preact"`; 0 component files, 0 hooks, 0 `className`, 0 `dangerouslySetInnerHTML` (template has no TSX) | none (no pages/components shipped; `tsconfig.json:22-29` includes `pages/`, `components/`, `layouts/`, `lib/`, `src/`, `zfb.config.ts`) | none (`stack.md:145` lists `@takazudo/zfb-md-wasm` as a first-party option only) | `wrangler.toml` (Workers Static Assets, `main = "./dist/_worker.js"`, `nodejs_compat`, `[assets] directory = "./dist"`, `not_found_handling = "404-page"`, preview env), `deploy.yml` (`wrangler deploy --env=""` + `versions upload --preview-alias`), `ci.yml` (install → build → test → typecheck → lint → `wrangler deploy --dry-run`), `scripts/run-b4push.sh` (6 steps mirroring CI) — **all adapter-facing and unchanged by v3** |
| `skills/dev-basic-cloudflare-webapp/SKILL.md` + `references/{stack,tooling,testing,conventions,ci,cloudflare}.md` (prose) | `stack.md:20` "Preact SSR, islands"; `:46-48` `adapter: "@takazudo/zfb-adapter-cloudflare"` (still valid) | `stack.md:21,64-66,143,147` | `SKILL.md:34,38`; `stack.md:54,60-66,131,133,148`; `tooling.md:223-231` (design-token-lint "raw Tailwind numeric spacing") | `stack.md:22` `framework: "preact"`; `tooling.md:128-149` tsconfig baseline (`:138` `jsxImportSource: "preact"`), `:219-221` `eslint-plugin-react(-hooks)`; `testing.md:10` `@testing-library/preact`, `:49-67` Preact-compat vitest aliases; `conventions.md:25` "Preact/React components" | `SKILL.md:36` SSR-app row (`prerender = false` + CF adapter) — still valid | `stack.md:145` | `ci.md`, `cloudflare.md`, `cloudflare-setup.md`: 0 Preact/Tailwind mentions; `cloudflare.md:22,39,49,255,260` describe the adapter (`_worker.js`, `_zfb_inner.mjs`, `async_hooks`, `getCloudflareContext()` throwing under `zfb dev`) — still accurate for 3.x |
| Rest of the repo (`skills/dev-bump-zudo-deps`, `skills/dev-reduce-deps/references/verdicts.md`, `skills/cleanup-resources`, `skills/dev-clean-mac`, `.gitignore:103`) | operational mentions only (`zfb-shadow-session-*` sweep, `.zfb-build/` cleanup, `@takazudo/zfb` as a bump example) | `verdicts.md:140,180` are KEEP rows for `preact` / `preact-render-to-string` under zfb 2.x hosts (`:118` is the lesson note that justified them) | none | none | none | none | n/a — **not a zfb consumer; unaffected** |

Measured with: `find /home/user/claude-settings -name 'zfb.config.*' -not -path '*/node_modules/*'` → 0; `grep -rnE '@import\s+"tailwindcss|@tailwind|@theme|@apply|…' skills/dev-basic-cloudflare-webapp` → 3 hits, all prose in `stack.md:54,65,66`; the Preact grep → 4 hits (`stack.md:22`, `tooling.md:138`, `package.json:44`, `tsconfig.json:9`); hooks/`className`/`onClick`/`<Island` grep → 0; `grep -rnE 'ZFB_TAILWIND|framework:|tailwind:'` → `stack.md:22` only; `npm view @takazudo/zfb{,-runtime,-adapter-cloudflare} dist-tags` → `latest: 3.2.0` each (`npm view @takazudo/zfb time` → 3.2.0 published 2026-10-04T15:30Z); `npm view @takazudo/zfb versions` → last 2.x is `2.22.1`. `zfb wind audit` and `zfb check` **skipped**: no zfb project dir exists in this repo.

## Sequencing and blockers

1. **Now, S (stop the bleeding):** change `package.json:40-42` from `"latest"` to an exact lockstep version. Either `"3.2.0"` (then do steps 2-3 before anyone scaffolds) or `"2.22.1"` as a holding pin that matches the current Preact/Tailwind prose. A scaffold template must never float on `latest`: the SKILL's own rule (`SKILL.md:119` "Replace `latest` with resolved versions after the first install") runs *after* the install that already pulled the wrong major, and `pnpm-workspace.yaml:16` `minimumReleaseAge: 0` removes pnpm 11's 24 h buffer.
2. **Now, M (full v3 alignment, recommended):** the file edits in "Required changes" 1-3 and 5, plus the prose rewrite. Nothing here waits on an upstream release; zfb 3.2.0 and the adapter 3.2.0 are published and the adapter has no config-facing change anywhere in 3.x: 3.0.0 lists "No package-specific changes", 3.1.0 adds a context generic, 3.2.0 only strips a dangling `sourceMappingURL` comment from the emitted `_zfb_inner.mjs` (https://zfb.takazudomodular.com/docs/changelog/zfb-adapter-cloudflare/v3.0.0/, https://zfb.takazudomodular.com/docs/changelog/zfb-adapter-cloudflare/v3.1.0/, GitHub release v3.2.0).
3. **Now, decision:** stop maintaining a second copy of the zfb site layer. Let scaffold mode run `pnpm create zfb@latest <name>` (https://zfb.takazudomodular.com/docs/getting-started/your-first-site/) for `zfb.config.ts`, `tsconfig.json` JSX settings, `components/zfb-shim.d.ts`, `styles/global.css`, `mdx-components.tsx`, `layouts/`, `pages/`, then overlay the house templates (CI, deploy, wrangler, hooks, b4push, prettier, pnpm settings). `zfb new` pins the zfb packages to the CLI's own version (https://zfb.takazudomodular.com/docs/getting-started/project-structure/), which removes the `latest` problem at the root.
4. **After zudo-doc 6.0.0** (zudolab/zudo-doc#4430 / #4477 / #4473; its integration floor needed zfb fixes #3569/#3570, which **shipped in zfb 3.2.0 on 2026-10-04**; 6.0.0 itself is still unreleased — npm `latest` for `@takazudo/zudo-doc` is 5.28.2): rewrite `SKILL.md:35`, `stack.md:21,64-66,143`, `conventions.md:93-102` for zudo-doc 6 (wind.json manifest, `zudoDoc({ wind })` override, zdtp kept as an opaque Preact bundle). Until then those lines must say: a `doc/` zudo-doc site pins `@takazudo/zfb` **2.22.1** and `@takazudo/zudo-doc` 5.28.x and must live in its own workspace package so the root site can be on 3.x.
5. **Shipped in zfb 3.2.0 (2026-10-04), optional here:** `wind.strict`, `wind.utilities.placement: "before-authored"`, `wind.sources`, `zfb wind manifest`, `file:line:col` audit output and `zfb wind audit --json`. Nothing in this template depends on them; if the prose mentions them, say "3.2.0+" (the local docs checkout at 5db06df still carries `version: 3.1.0`, so treat its pages as describing 3.2.0).
6. **Propagate:** `/claude-resources-share -a` republishes `skills/` to `Takazudo/claude-resources` (one-direction rsync, see `skills/claude-resources-share/SKILL.md`); `Takazudo/cloudflare-app-bootstrap-skill` has no sync path and needs a manual Japanese port (its guide lists the JP line numbers).

## Required changes

### 1. Dependencies, config, tsconfig, env

- **`assets/templates/package.json:40-42`** — replace the three `"latest"` strings with `"3.2.0"` (lockstep; bump later with `/dev-bump-zudo-deps`, whose `resolve-bumps.mjs:6` already refuses to auto-pin a literal dist-tag). Interim alternative: `"2.22.1"`.
- **`package.json:43-44`** — delete `preact` and `preact-render-to-string`. They were present only for zfb's removed engine; the runtime ships as `@takazudo/zfb/zudo-react`, not a separate package (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#components-and-islands). Keep `@takazudo/zfb-runtime` (`:42`) — the client router/prefetch runtime is still a direct dependency in the v3 scaffold (`basic-blog/package.json:13-14`).
- **`package.json:51`** `@takazudo/zudo-design-token-lint` — keep. It regex-lints class spellings (`p-4`, `bg-gray-500` …) in TSX and those spellings are unchanged in zudo-wind; what changes is that v3 ships **no implicit palette or spacing scale**, so a raw palette class is ZW006 "missing token" even before the linter sees it (https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/#theme-and-tokens). Update its one-line description (`stack.md:148`) rather than the dependency.

  ```jsonc
  // assets/templates/package.json — dependencies after the change
  "dependencies": {
    "@takazudo/zfb": "3.2.0",
    "@takazudo/zfb-adapter-cloudflare": "3.2.0",
    "@takazudo/zfb-runtime": "3.2.0",
    "zod": "^4.3.6"
    // removed: "preact", "preact-render-to-string"
  }
  ```

- **`assets/templates/tsconfig.json:8-9`** — `"jsx": "react-jsx"` stays; `"jsxImportSource": "preact"` → `"@takazudo/zfb/zudo-react"` (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#components-and-islands). Keep `types: ["node", "@cloudflare/workers-types"]` (`:14`) and the `@/*` alias (`:18-20`); the v3 scaffold uses `~/*` → project root, pick one and say so in `tooling.md`. Add `"mdx-components.tsx"` and `"content/**/*"` to `include` (`:22-29`) when the project uses the markdown component map (`basic-blog/tsconfig.json:26-34`).

  ```json
  {
    "compilerOptions": {
      "jsx": "react-jsx",
      "jsxImportSource": "@takazudo/zfb/zudo-react"
    }
  }
  ```

- **New template file `assets/templates/components/zfb-shim.d.ts`** — copy verbatim from `crates/zfb/templates/basic-blog/components/zfb-shim.d.ts` (10 lines: `declare module "zfb/config" { export * from "@takazudo/zfb/config"; }`). `zfb.config.ts` imports `defineConfig` from the bare specifier `zfb/config`, which the config loader aliases at parse time; without the shim `zfb check` (`tsc --noEmit`) reports it unresolved (https://zfb.takazudomodular.com/docs/api/define-config/#typing-the-zfbconfig-import).
- **New template file `assets/templates/zfb.config.ts`** (or: take it from `pnpm create zfb@latest` and add the adapter line). Minimum for the "Content site" and "SSR app" shapes:

  ```ts
  import { defineConfig } from "zfb/config";

  export default defineConfig({
    adapter: "@takazudo/zfb-adapter-cloudflare", // emits dist/_worker.js — see stack.md "zfb SSR contract"
    wind: {
      spec: 1,
      reset: "owned-v1",
      tokens: {
        spacingUnit: "0.25rem",
        // Semantic tokens only (design-token-lint forbids raw palette classes):
        colors: { surface: "var(--color-surface)", fg: "var(--color-fg)", accent: "var(--color-accent)" },
      },
      breakpoints: { sm: { minWidthPx: 640 }, md: { minWidthPx: 768 }, lg: { minWidthPx: 1024 } },
      dark: { attribute: "data-theme", value: "dark" },
      authoredClasses: { prose: true },
    },
  });
  ```

  Every map starts empty in v3; add a token for every utility family the project uses (`fontSizes`, `radii`, `fontFamilies` …) — see `basic-blog/zfb.config.ts:14-113` for a complete map and https://zfb.takazudomodular.com/docs/zudo-wind/configuration/ for the schema. There is **no** `framework` key and **no** `tailwind` key; a leftover one fails config loading with a named error (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#configuration).
- **`assets/templates/gitignore:14-15`** — delete the comment and `zfb-tailwind-entry-*.css`. The name belongs to zfb ≤ 2.x's Tailwind runner (changelog v0.1.0-next.31, v2.3.1); `grep -rn tailwind-entry crates packages` on zfb `main` → 0. Keep `.zfb/`, `.zfb-build/`, `.wrangler/`. Same for `conventions.md:106-119` (its `.gitignore` block already omits the line — leave it).
- **Env / CI:** `grep -rnE 'ZFB_TAILWIND|ZFB_TAILWIND_OXIDE_WARMUP' assets/templates` → 0, nothing to remove (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#cli-and-environment).
- **Lockfile:** the template ships no `pnpm-lock.yaml` (`find assets/templates -name 'pnpm-lock.yaml'` → 0, and `/claude-resources-share` excludes lockfiles anyway), so nothing fixes the resolved version before the scaffold's first `pnpm install` — the exact `3.2.0` pin in `package.json` is the only guard against a future major.
- **`assets/templates/wrangler.toml`** — unchanged. `main = "./dist/_worker.js"`, `compatibility_flags = ["nodejs_compat"]`, `[assets]` and the preview env match the v3 adapter docs (https://zfb.takazudomodular.com/docs/guides/ssr-and-cloudflare-bindings/#configuring-the-cloudflare-adapter); adapter 3.0.0 lists "No package-specific changes", 3.1.0 only adds a context generic (https://zfb.takazudomodular.com/docs/changelog/zfb-adapter-cloudflare/v3.1.0/), and 3.2.0 only strips a dangling `sourceMappingURL` comment from `_zfb_inner.mjs` (GitHub release v3.2.0, 2026-10-04).

### 2. CSS and utilities

Prose only — the template ships no stylesheet. Add one (`assets/templates/styles/global.css`) or point scaffold mode at the generated `styles/global.css` from `pnpm create zfb@latest` (`crates/zfb/templates/basic-blog/styles/global.css`: authored `@layer base` custom properties + a `.prose` block, no Tailwind import; the `styles/global.css` resolution rule is https://zfb.takazudomodular.com/docs/concepts/styling/#global-css).

- `references/stack.md:54` `| Utility CSS | **Tailwind CSS v4** (`@import "tailwindcss"`, `@theme` blocks) |` → `| Utility CSS | **zudo-wind** (built into zfb 3; configured in the `wind` key of `zfb.config.ts`, values in `wind.tokens`; no CSS directives) |`. Under zfb 3 every Tailwind directive or import is a **ZW009 build error**, even with `wind: false` (https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/#directives). Tailwind v4 via `@tailwindcss/vite` remains correct for the **Interactive app** shape (`SKILL.md:38`, plain Vite + React) — that path does not go through zfb and is unaffected; say so in the same table.
- `stack.md:60-62` — keep the semantic-token rule; add: v3 supplies no palette, spacing unit, scales or breakpoints, so the semantic tokens *are* the `wind.tokens` map (`colors.surface: "var(--color-surface)"`), and the three-tier custom properties (`:56`) stay in authored CSS (https://zfb.takazudomodular.com/docs/zudo-wind/tokens/).
- `stack.md:64-66` "Under zudo-doc, Tailwind is compiled by zfb's embedded engine …" → true only for zudo-doc 5.x on zfb 2.22.1; rewrite after 6.0.0 ("zudo-doc 6 runs zudo-wind and ships a `wind.json` candidate manifest; apps declare their own tokens"). Until then prefix it with the version lock.
- `stack.md:131` `| styled-components / emotion | Tailwind v4 or CSS Modules |` → `zudo-wind utilities or CSS Modules (Tailwind v4 only inside a Vite SPA)`.
- `stack.md:133`, `:148`, `tooling.md:223-231` — "Tailwind token linter" / "raw Tailwind numeric spacing" → "utility class-name token linter (Tailwind-style spellings, applies to zudo-wind candidates)".
- Add to `tooling.md` or `stack.md` a short "what zudo-wind does not have" line so agents stop writing them: `ring-*`, `animate-*`, `scale-*`, arbitrary properties, `!important`, `aria-*`/`data-[…]` variants, named `group/peer`, `@apply`, `line-clamp-*`, `container`, `order-*`, `basis-*` (ZW014 warning; error under `wind.strict: true`, available since zfb 3.2.0) — https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/#what-is-not-supported-and-what-to-do-instead, https://zfb.takazudomodular.com/docs/zudo-wind/diagnostics-and-tools/#migration-warnings-and-strict-mode.
- Optional, for projects that want Tailwind-preflight parity: link the measured `@layer base` block in https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/#reset-differences-from-tailwind-preflight instead of pasting it into the template.

### 3. Components and islands

Prose only — the template ships no components.

- `SKILL.md:34` `| **Content site** | … | zfb + Preact + Tailwind v4 |` → `zfb 3 (zudo-react components + zudo-wind utilities)`.
- `stack.md:20` "Preact SSR, islands" → "zudo-react SSR, islands". `stack.md:22` the whole row → `| UI runtime under zfb | **zudo-react** (`@takazudo/zfb/zudo-react`, shipped inside `@takazudo/zfb`) | Setup-once components, `signal()`/`computed()`, `Show`/`For`, native `on:click`, HTML spellings (`class`, `for`). No Preact, no React, no compat aliases. A Preact/React widget is embedded as an opaque self-mounting bundle inside an island. |` (https://zfb.takazudomodular.com/docs/zudo-react/coming-from-preact-hooks/#hook-by-hook, https://zfb.takazudomodular.com/docs/concepts/islands/#embedding-a-third-party-widget).
- `stack.md:23` "Client-heavy app | Vite + React 19" — keep, and strengthen the reason: zudo-react has no hooks, context, portals, `forwardRef`, `lazy`/`Suspense`, class components or error boundaries (https://zfb.takazudomodular.com/docs/zudo-react/coming-from-preact-hooks/#what-does-not-exist), so an editor/dashboard still belongs in the Vite + React shape.
- `tooling.md:128` "A reasonable baseline for a zfb/Preact project" and `:138` → `jsxImportSource: "@takazudo/zfb/zudo-react"`; mention `components/zfb-shim.d.ts`.
- `tooling.md:219-221` "React/Preact projects add `eslint-plugin-react` + `eslint-plugin-react-hooks`" → scope it to the Vite + React shape. For zfb 3 projects do **not** add them: there are no hooks to lint and the React rules reject the zudo-react dialect (`class`, `for`, `on:click`, `style={{ "background-color": … }}`).
- `conventions.md:25` `components/   Preact/React components` → `components/   zudo-react components; islands are "use client" files wrapped in <Island>`.
- `SKILL.md:36` SSR-app row and `stack.md:32-48` "zfb SSR contract" — unchanged: `export const prerender = false`, `getCloudflareContext<Env>()`, `adapter: "@takazudo/zfb-adapter-cloudflare"` are all current (https://zfb.takazudomodular.com/docs/guides/ssr-and-cloudflare-bindings/).
- Add one island sentence to `stack.md`: `<Island>` takes one component child and JSON props, hydration fails closed per island, nested islands are rejected (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#components-and-islands).
- Add one pragma sentence to `tooling.md` next to the tsconfig block: never write a per-file `/** @jsxImportSource preact */` (or `react`) pragma — it overrides `tsconfig.json`, fails the build (`Could not resolve "preact/jsx-runtime"` or `ZR_CHILD`), and since zfb 3.2.0 `zfb build`/`zfb dev` print a `zfb warn:` line per offending file (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#components-and-islands). The template itself has 0 pragmas (`grep -rn '@jsxImportSource' skills/dev-basic-cloudflare-webapp` → only `tooling.md:138` and `tsconfig.json:9`, both the tsconfig key).

### 4. md-wasm and other packages

- `stack.md:145` `@takazudo/zfb-md-wasm` — add: on 3.x `compile()`/`renderHtml()` take no `jsxRuntime`; compiled MDX imports its Fragment from `@takazudo/zfb/zudo-react/jsx-runtime`, so a React/Preact (Vite SPA) consumer must use `renderHtml()` or stay on md-wasm 2.x (https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/#mdx-and-md-wasm, https://zfb.takazudomodular.com/docs/api/md-wasm/).
- `SKILL.md:35`, `stack.md:21,143` zudo-doc — add now: "zudo-doc 5.28.x requires zfb **2.22.1**; do not combine with `@takazudo/zfb` 3.x in the same package. zudo-doc 6.0.0 (zfb 3) is in progress — zudolab/zudo-doc#4430." Rewrite after 6.0.0 per the consumer guide (#4473).
- `stack.md:147` `@takazudo/zdtp` — add: a Preact widget; on zfb 3 it is embedded via the third-party-widget island pattern, not rendered by zudo-react (zdtp#1002 asks it to bundle Preact).
- `skills/dev-reduce-deps/references/verdicts.md:140,180` (KEEP rows; `:118` is the lesson note behind them) — the KEEP verdicts for `preact`/`preact-render-to-string` under zfb hosts hold only for zudo-doc 5.x / zfb 2.x; append a dated row saying they become REMOVE on a v3 host (do not edit the historical rows).
- `skills/dev-bump-zudo-deps/SKILL.md` — optional: add that a `@takazudo/zfb` 2.x → 3.x bump is a migration, not a plain bump, and link https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/ (the skill's impact step at `:87` would otherwise present it as "renamed options").

### 5. Tests, CI, deploy

- `references/testing.md:10` `| L2 component | vitest + @testing-library/preact (or /react) |` → `| L2 component | vitest + happy-dom + @takazudo/zfb/zudo-react/testing (createIslandTest; renderToString from @takazudo/zfb/zudo-react/server) |` (https://zfb.takazudomodular.com/docs/zudo-react/testing/#hydrate-and-mount-an-island). Add `happy-dom` to the template devDependencies if L2 tests are expected.
- `testing.md:49-67` "Preact-in-compat-mode aliases" — delete the block (no `react/jsx-runtime` alias exists to fail any more) and replace with the v3 runner config (https://zfb.takazudomodular.com/docs/zudo-react/testing/#configure-jsx-and-a-dom):

  ```ts
  // vitest.config.ts
  import { defineConfig } from "vitest/config";
  export default defineConfig({
    esbuild: { jsx: "automatic", jsxImportSource: "@takazudo/zfb/zudo-react" },
    test: { environment: "happy-dom" },
  });
  ```

  Keep `:69-72` (projects do not inherit the root config) — still true and now about the `esbuild.jsx*` keys instead of aliases.
- `assets/templates/.github/workflows/ci.yml:46-61` and `scripts/run-b4push.sh:34-39` — add one step after Build, identically in both (the SKILL's own parity rule): `pnpm exec zfb wind audit --fail-on error` (https://zfb.takazudomodular.com/docs/api/cli/#zfb-wind; migration checklist step 6). The command needs the project's `zfb.config.*`, so it only applies once the scaffold has one. `pnpm typecheck` (`zfb check`) already covers removed keys and collection schemas (https://zfb.takazudomodular.com/docs/api/cli/#zfb-check).
- `deploy.yml`, `actionlint.yml`, `scripts/smoke.mjs`, `wrangler.toml`, `cloudflare-setup.md`, `references/ci.md`, `references/cloudflare.md` — no change; none mention Preact or Tailwind and the adapter output (`dist/_worker.js`, `dist/.assetsignore`) is the same on 3.x.

## Step-by-step plan

1. **Pick the version policy** (5 min). Edit `assets/templates/package.json:40-42` to `"3.2.0"` (or `"2.22.1"` if steps 2-6 must wait). Replace `SKILL.md:119` "Replace `latest` with resolved versions after the first install" with "zfb packages are pinned exactly and in lockstep; bump with `/dev-bump-zudo-deps`".
   `cd $HOME/.claude && grep -n '"latest"' skills/dev-basic-cloudflare-webapp/assets/templates/package.json`
2. **Template files** (migration checklist steps 2-4 applied to a template): `package.json:43-44` delete Preact deps; `tsconfig.json:9` JSX import source; `gitignore:14-15` drop the Tailwind temp pattern; add `components/zfb-shim.d.ts`; add `zfb.config.ts` and `styles/global.css` (copy from `pnpm create zfb@latest` output or `crates/zfb/templates/basic-blog`, then add `adapter`). Update the Scaffolding table in `SKILL.md:117-132` with the new rows.
3. **Prose** — apply the `file:line` edits in Required changes 2-5: `SKILL.md:34,35`; `stack.md:20,21,22,54,60-66,131,133,143,145,147,148`; `tooling.md:128,138,219-221,223-231`; `testing.md:10,49-67`; `conventions.md:25`. Add the "not supported in zudo-wind" list and the zudo-doc version lock.
4. **Decide the scaffold flow** and write it into `SKILL.md` "Scaffolding": `pnpm create zfb@latest <name>` → overlay `assets/templates/` (CI, deploy, wrangler, hooks, b4push, pnpm/prettier config) → merge `package.json` scripts/devDeps → set `adapter` in `zfb.config.ts` → replace `__PROJECT_NAME__`/`__DOMAIN__`. Note the pnpm 11 `minimumReleaseAge` caveat from https://zfb.takazudomodular.com/docs/getting-started/your-first-site/.
5. **Dry-run the skill** in a scratch directory (this is the only executable verification; the template has no code of its own):
   ```sh
   cd "$(mktemp -d)" && pnpm create zfb@latest smoke-site && cd smoke-site
   # overlay the house templates, then:
   pnpm exec zfb --version          # "3.2.0" + embedded esbuild, no Tailwind line
   pnpm exec zfb check              # tsc + schema; fails on leftover framework/tailwind keys
   pnpm exec zfb wind audit --fail-on error
   pnpm exec zfb build && pnpm exec wrangler deploy --dry-run
   pnpm b4push
   ```
   Confirm `dist/_worker.js` and `dist/.assetsignore` exist and that `grep -rn "preact" node_modules/.pnpm | head` finds nothing pulled by the site.
6. **Commit and propagate**: `/co`, then `/claude-resources-share -a` (refreshes `Takazudo/claude-resources`); open an issue or PR on `Takazudo/cloudflare-app-bootstrap-skill` for the Japanese port (see its guide).
7. **After zudo-doc 6.0.0 ships**: rewrite the docs-site lines (`SKILL.md:35`, `stack.md:21,64-66,143`, `conventions.md:93-102`) from zudolab/zudo-doc#4473, and re-run step 5 with a `doc/` package added.

## Verification checklist

- [ ] `grep -rn '"latest"' skills/dev-basic-cloudflare-webapp/assets/templates/package.json` lists no `@takazudo/zfb*` line.
- [ ] `grep -rn 'preact' skills/dev-basic-cloudflare-webapp/assets/templates/` → 0.
- [ ] `grep -rnE 'tailwind|Tailwind' skills/dev-basic-cloudflare-webapp/` returns only the Vite + React (Interactive app) rows and the ZW009 warning text.
- [ ] `grep -rn 'jsxImportSource' skills/dev-basic-cloudflare-webapp/` → every hit is `@takazudo/zfb/zudo-react`.
- [ ] `grep -rn 'framework:' skills/dev-basic-cloudflare-webapp/` → 0.
- [ ] A fresh dry-run scaffold passes `zfb check`, `zfb wind audit --fail-on error`, `zfb build`, `wrangler deploy --dry-run`, `pnpm b4push`; `zfb --version` prints 3.2.0 and no Tailwind line.
- [ ] `diff -r skills/dev-basic-cloudflare-webapp $HOME/repos/p/claude-resources/skills/dev-basic-cloudflare-webapp` is empty after `/claude-resources-share`.
- [ ] The docs-site row states the zudo-doc 5.28.x ↔ zfb 2.22.1 lock until 6.0.0.

## Risks and open questions

- **Public blast radius.** `claude-resources` is installable as a plugin (`/plugin marketplace add takazudo/claude-resources`); every external user who scaffolds with this skill today gets the broken combination. Prioritise step 1 and the re-share over the prose polish.
- **Mixed-version workspaces.** A repo scaffolded with a 3.x root site plus a `doc/` zudo-doc 5.x package needs two zfb majors in one pnpm workspace. pnpm isolates them per package, but `pnpm-workspace.yaml:28` `linkWorkspacePackages: false` and separate lockfile hygiene must be stated; repos that already keep `doc/` as its own workspace package (e.g. zudo-slack-notify: `doc/` on zfb 2.22.1 beside an `app/` worker with no zfb) show the layout, but no repo in the org mixes two zfb majors yet — the step-5 dry run with a `doc/` package added is the first proof.
- **Token-lint semantics.** design-token-lint 2.1.0 still names Tailwind in its README; its rules work on zudo-wind spellings, but nobody has run it against a v3 project. Verify in the dry run that it does not flag `size-8`, `tracking-[0.08em]` or `dark:bg-sky-950/40` as violations.
- **Which tsconfig alias wins** (`@/*` → `src/*` in the house template vs `~/*` → root in basic-blog): the overlay step must choose one, or `zfb check` fails on the first import.
- **Japanese copy drift.** `cloudflare-app-bootstrap-skill` was last updated 2026-08-19 and already differs in non-zfb details (plugin skill names); a v3 rewrite here widens that gap unless the port is scheduled.
- Open: should the skill keep shipping its own `zfb.config.ts`/CSS at all, or defer entirely to `create-zfb` (whose output is CI-verified at https://create-zfb.takazudomodular.com)? Recommendation: defer, and keep only the adapter line and the Cloudflare overlay in this skill.

## References

- Migration guide: https://zfb.takazudomodular.com/docs/guides/migrating-to-v3/ (#configuration, #stylesheets, #components-and-islands, #mdx-and-md-wasm, #cli-and-environment, #checklist)
- Tailwind map: https://zfb.takazudomodular.com/docs/zudo-wind/coming-from-tailwind/ · config: https://zfb.takazudomodular.com/docs/zudo-wind/configuration/ · tokens: https://zfb.takazudomodular.com/docs/zudo-wind/tokens/ · diagnostics: https://zfb.takazudomodular.com/docs/zudo-wind/diagnostics-and-tools/
- Preact hooks map: https://zfb.takazudomodular.com/docs/zudo-react/coming-from-preact-hooks/ · testing: https://zfb.takazudomodular.com/docs/zudo-react/testing/ · islands / widget embedding: https://zfb.takazudomodular.com/docs/concepts/islands/#embedding-a-third-party-widget
- CLI: https://zfb.takazudomodular.com/docs/api/cli/ (#zfb-new, #zfb-check, #zfb-wind) · config: https://zfb.takazudomodular.com/docs/api/define-config/ · styling: https://zfb.takazudomodular.com/docs/concepts/styling/ · md-wasm: https://zfb.takazudomodular.com/docs/api/md-wasm/
- Cloudflare adapter: https://zfb.takazudomodular.com/docs/guides/ssr-and-cloudflare-bindings/ · changelogs: https://zfb.takazudomodular.com/docs/changelog/zfb/v3.0.0/, https://zfb.takazudomodular.com/docs/changelog/zfb/v3.1.0/, https://zfb.takazudomodular.com/docs/changelog/zfb-adapter-cloudflare/v3.0.0/
- v3 scaffold: `crates/zfb/templates/basic-blog` in Takazudo/zudo-front-builder; `pnpm create zfb@latest` (https://zfb.takazudomodular.com/docs/getting-started/your-first-site/); live output https://create-zfb.takazudomodular.com
- zudo-doc 6.0.0: zudolab/zudo-doc#4430 (epic), #4477 (root PR), #4473 (consumer migration guide); zdtp: Takazudo/zudo-design-token-panel#1002
- Sibling guides: `Takazudo--claude-resources.md`, `Takazudo--cloudflare-app-bootstrap-skill.md` (same template, propagation only)
