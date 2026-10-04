# zfb v3 migration guide: Takazudo/zudo-slack-notify

Generated 2026-10-04 by an automated diagnosis of `main` @ `cbc6c5b`. Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**Blocked on zudo-doc 6.0.0 only. Effort S.** `doc/` is an unmodified `create-zudo-doc` scaffold (zudo-doc `^5.28.2`, zfb `2.22.1` exact): one `zfb.config.ts`, the two generator stubs, the generator `global.css`, 16 MDX files, no `chrome-bindings`, no `src/components/`, no host CSS, no islands. `zfb wind audit` found **0 host-owned utility candidates**, and the greps found 0 hooks, 0 `className`, 0 Tailwind directives outside the generated `global.css`. `app/` is a plain Cloudflare Worker + Node CLI (TypeScript, wrangler, vitest) with no zfb, Preact or Tailwind dependency — unaffected by zfb 3. The only gate is `@takazudo/zudo-doc` 6.0.0 (zudolab/zudo-doc#4430, root PR #4477, consumer guide #4473): until the preset stops emitting `framework`/`tailwind`, config loading fails on zfb 3 regardless of host changes (https://zfb.takazudomodular.com/guides/migrating-to-v3/#before-you-start). Once it ships, this repo is a pin bump plus a re-copy of the scaffold files, verified by the existing `build-doc` job and `deploy-doc` smoke.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `doc/` (`zudo-slack-notify-doc`; `doc/zfb.config.ts`, 64 lines, `defineConfig(zudoDoc({...}))`) | `@takazudo/zfb` `2.22.1` exact, `-runtime` `2.22.1`, `-md-wasm` `2.22.1` (`doc/package.json:22-24`) | `@takazudo/zudo-doc` `^5.28.2`, `@takazudo/zudo-doc-history-server` `^5.28.2` (`:25,30`); no zudo-sg, no zdtp (`designTokenPanel` not configured) | generated `doc/src/styles/global.css` only (`:8` `tailwindcss/preflight`, `:9` `tailwindcss/utilities`, `:14` `@takazudo/zudo-doc/safelist.css`, `:20-22` 3 `@source`, `:26-28` empty `@theme {}`); no `tailwind.config.*`, no `@tailwindcss/*` dep, no `ZFB_TAILWIND*` in scripts/CI | `preact ^10.29.1`, `preact-render-to-string ^6.6.6` (`:27-28`) for zudo-doc's engine only; host code: 1 pragma file (`doc/pages/docs/[[...slug]].tsx:1-2`, `:27` `import type { JSX } from "preact"`), 0 hooks | none host-owned (zudo-doc's own islands only) | dep only (zudo-doc HtmlPreview); no direct calls | Cloudflare Workers static assets, `doc/wrangler.toml` (`./dist`, custom domain `zudo-slack-notify.zudolab.dev`); `.github/workflows/deploy-doc.yml` on `main` push (path-filtered `doc/**`), gated on Cloudflare secrets, smoke via `scripts/smoke.sh doc` |
| `app/` (`zudo-slack-notify-app`, Worker API + CLI) | none | none | none | none | none | none | `app/wrangler.toml`, `deploy-app.yml`; **not a zfb project — unaffected** |
| repo root (`scripts/`, `skills/notify-slack/`, lefthook, prettier, `@takazudo/mdx-formatter ^1.3.0-next.4`) | none | none | none | none | none | none | n/a |

Measured with: `find . -name 'zfb.config.*'`, the briefing greps (`grep -rn "zfb\|preact\|tailwind" app` → 0 files), `zfb wind audit --project-root doc` (zfb 3.1.0, temporary `wind: { spec: 1 }` config).

## Sequencing and blockers

1. **zudo-doc 6.0.0** — zudolab/zudo-doc#4430 (epic), #4477 (root PR, draft), #4473 (consumer migration guide, planned). Its integration floor was blocked on zfb bugs Takazudo/zudo-front-builder#3569/#3570; both are fixed on zfb `main` (2026-10-04) but **unreleased**; the epic's round-2 lock is exact 3.1.0 / peer `^3.1.0` (R2-DD3), so expect 6.0.0 to re-lock on the **next zfb release**. Pin whatever 6.0.0's peer range names.
2. **This repo** (S): bump pins, re-copy the four generated files (`doc/pages/docs/[[...slug]].tsx`, `doc/pages/index.tsx`, `doc/tsconfig.json`, `doc/src/styles/global.css`) from `create-zudo-doc@6`, drop `preact`/`preact-render-to-string`, rewrite `doc/CLAUDE.md` tech-stack lines, rebuild, let `deploy-doc.yml` ship it.
3. **Possible now**: nothing load-bearing. The `^5.28.2` carets cannot float into 6.x, zfb is pinned exactly, and CI uses `pnpm install --frozen-lockfile` (`.github/actions/setup/action.yml:24-26`), so the site cannot break by accident. Do **not** bump `@takazudo/zfb` to 3.x ahead of the preset. Optionally pre-write the `doc/CLAUDE.md` edit (below) in a draft PR.

## Required changes

### 1. Dependencies, config, tsconfig, env

- `doc/package.json:22-24` — zfb family to the exact version zudo-doc 6.0.0 pins (`zfb --version` on v3 prints only the release and `embedded esbuild`; https://zfb.takazudomodular.com/guides/migrating-to-v3/#cli-and-environment). One root `pnpm-lock.yaml` covers `app/` and `doc/` (`pnpm-workspace.yaml`); there is no separate `doc/` lockfile.
- `doc/package.json:25,30` — `@takazudo/zudo-doc` and `@takazudo/zudo-doc-history-server` → `^6.0.0` (lockstep release; keep the caret style the file already uses).
- `doc/package.json:27-28` — delete `preact` and `preact-render-to-string`: present only for zfb's removed engine (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Nothing in `doc/` imports them except the generator stub's `import type { JSX } from "preact"`, which the 6.0.0 stub replaces.
- `doc/package.json:35` — `wrangler` `4.143.0` is unrelated to zfb; leave it.

  ```jsonc
  // doc/package.json after the bump
  "dependencies": {
    "@takazudo/zfb": "<zfb version pinned by zudo-doc 6.0.0>",
    "@takazudo/zfb-runtime": "<same>",
    "@takazudo/zfb-md-wasm": "<same>",
    "@takazudo/zudo-doc": "^6.0.0",
    "@takazudo/zudo-doc-history-server": "^6.0.0",
    "zod": "^4.3.6",
    "diff": "^8.0.3"
    // removed: preact, preact-render-to-string
  }
  ```

- `doc/zfb.config.ts` — host-side there is **no** `framework` or `tailwind` key (lines 5-63 are `zudoDoc()` options: `siteName`, `siteUrl`, `githubUrl`, `llmsTxt`, `sidebarResizer`, `sidebarToggle`, `tocToggle`, `imageEnlarge`, `dynamicPageTransition`, `docHistory`, `assetViewer`, `footer`, `headerNav`, `headerRightItems`). In 5.x both removed keys are emitted by `zudoDoc()` itself, so the host has nothing to delete. An absent `wind` key means the empty v1 config (reset `none`, no tokens — https://zfb.takazudomodular.com/zudo-wind/configuration/#the-wind-key); the preset must supply tokens and reset. zudo-doc 6 plans the consumer override as `zudoDoc({ wind: {...} })`, with package-owned tokens, `owned-v1` reset plus an authored preflight patch, `sm`/`lg`/`xl` breakpoints and a `./wind.json` manifest (DD4 in zudolab/zudo-doc#4430; confirm in #4473); this site has no custom utilities, so it should not need one.
- `doc/tsconfig.json:8-10` — remove the `react` / `react/jsx-runtime` / `react-dom` → `./node_modules/preact/compat/` aliases. `jsx: "react-jsx"` and `jsxImportSource: "@takazudo/zfb/zudo-react"` are expected from `@takazudo/zudo-doc/tsconfig.base.json` (a consumer-shipped file 6.0.0 plans to flip); add them in `compilerOptions` only if 6.0.0 does not.

  ```json
  {
    "extends": "@takazudo/zudo-doc/tsconfig.base.json",
    "include": ["src", "pages", "zfb.config.ts"],
    "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["src/*"] } }
  }
  ```

- Env/CI: `grep -rn ZFB_TAILWIND .github scripts package.json` → 0 hits. `pnpm-workspace.yaml:13-16` `allowBuilds: esbuild, lefthook, workerd` is unrelated to zfb 3 (zfb ships platform binaries through `optionalDependencies`); leave it unless `pnpm install` reports a new blocked build script after the bump.

### 2. CSS and utilities

- `doc/src/styles/global.css` (28 lines) — every line below is a **ZW009 error** on v3, including under `wind: false` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives): `:8` `@import "tailwindcss/preflight" layer(zd-preflight);`, `:9` `@import "tailwindcss/utilities";`, `:14` `@import "@takazudo/zudo-doc/safelist.css";` (a Tailwind `@source inline(...)` sheet in 5.x; zudo-doc 6 plans to retire the export in favour of a `wind.json` candidate manifest the preset declares — the host declares nothing), `:20-22` the three `@source` globs, `:26-28` the empty `@theme {}` slot. `:7` `@layer zd-preflight, zd-flow;` and `:13,15-17` (`theme.css`, `content.css`, `page-loading.css`, `features.css`) stay valid if 6.0.0 keeps those exports. **Re-copy the file from `create-zudo-doc@6`; do not hand-edit.** This host adds nothing to it today, so there is nothing to merge back.
- Tokens: none host-owned. The audit's `dead classes: (none)` means the host has no utility to declare tokens for; all utilities live inside `@takazudo/zudo-doc` and will be covered by its manifest/tokens.
- Reset: today the page relies on Tailwind preflight in the `zd-preflight` layer. zudo-doc 6 plans `owned-v1` plus an authored preflight patch (DD4 in #4430); the measured preflight → `owned-v1` differences the patch must cover are `sub`/`sup`, `small`, `hr`, `::placeholder`, `::file-selector-button`, `[hidden]`, control backgrounds (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#differences-from-tailwind-preflight). This site's content uses `<Tabs>`/`<TabItem>` x3/8 and `<CategoryNav>` x3 plus code blocks; check tabs, the search input placeholder and lists after the bump.
- Audit (`zfb wind audit --project-root doc`, zfb 3.1.0, exit 0):

  ```text
  outcome: complete
  spec: 1 revision 3
  unrecognized classes: (none)
  conflicts: (none)
  dead classes: (none)
  dynamic constructions:
    - docs; at default/pages:docs/[[...slug]].tsx:2598
  diagnostics: 8 x auditInfo (ZW002 x2, ZW005 x5, ZW012 x1) — import specifiers in pages/docs/[[...slug]].tsx and pages/index.tsx + the template literal
  ```

  16 MDX files: 0 `class=`/`className=`. The `auditInfo` lines sit at import specifiers (`@takazudo/zudo-doc/...`, `virtual:zudo-doc-*`), not in comments. 3.1.0 prints byte offsets; `file:line:col` locations, `--json`, `--severity` and `--plan` for the audit are **next zfb release** (https://zfb.takazudomodular.com/api/cli/#zfb-wind).

### 3. Components and islands

- No host components, no `chrome-bindings.tsx`, no `src/components/`. `doc/src/` holds only `content/` and `styles/`.
- `doc/pages/docs/[[...slug]].tsx` (67 lines) — generator stub (locked manifest #2653) with `/** @jsxRuntime automatic */` + `/** @jsxImportSource preact */` at `:1-2` and `import type { JSX } from "preact"` at `:27`. On v3 a per-file Preact pragma in a page fails the build (`Could not resolve "preact/jsx-runtime"` once preact is removed) after a `zfb warn:` naming file:line:col (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Replace the whole file with the 6.0.0 stub; it has no host edits.
- `doc/pages/index.tsx` — 1-line re-export of `@takazudo/zudo-doc/routes/index`; keep if the export survives.
- Islands, forms, `rawHtml`, `<Island>` transport: all inside `@takazudo/zudo-doc`; nothing for the host to port (https://zfb.takazudomodular.com/concepts/islands/).

### 4. md-wasm and other packages

- No host `compile()`/`renderHtml()` calls; `@takazudo/zfb-md-wasm` is zudo-doc's HtmlPreview dependency, so the `jsxRuntime` removal (https://zfb.takazudomodular.com/guides/migrating-to-v3/#mdx-and-md-wasm) is zudo-doc's concern.
- `app/` (Worker, CLI, workerd tests) and `scripts/` (ops, smoke) are not zfb projects; `@takazudo/mdx-formatter` at the root formats Markdown and has no zfb dependency. Unaffected.

### 5. Tests, CI, deploy

- `.github/workflows/ci.yml:67-76` `build-doc`: `pnpm --filter zudo-slack-notify-doc build` + `check:links`. `deploy-doc.yml:48-78`: same build, `wrangler deploy` from `doc/`, then `bash scripts/smoke.sh doc` against `DOC_BASE_URL`. No command changes; the lockfile is the switch. `scripts/run-b4push.sh` Step 7 (`pnpm build`) and Step 8 (`check:links`) cover it locally.
- Add a utility gate once on v3 (`--fail-on` exists in 3.1.0):

  ```yaml
  # ci.yml, build-doc job, after "Build doc"
      - name: Audit utility candidates
        run: pnpm --filter zudo-slack-notify-doc exec zfb wind audit --fail-on error
  ```

- `doc/scripts/check-links.js --strict-broken` runs over source and `dist/`; it stays the link gate.
- `doc/CLAUDE.md:3,9-10,12` still describe "Tailwind CSS v4", "zfb's embedded Tailwind engine", "Preact for interactive islands", "default `@theme` design tokens". Rewrite to zudo-wind / zudo-react wording after the bump (or copy the 6.0.0 scaffold's CLAUDE.md text).

  Replacement wording for `doc/CLAUDE.md:3,9-10,12` (drop-in; adjust once the 6.0.0 scaffold publishes its own text):

  ```markdown
  Documentation site built with zudo-doc 6 — a zfb 3 documentation framework with MDX,
  zudo-wind utilities and zudo-react islands. Layout, chrome, islands, tokens and routes ship
  from `@takazudo/zudo-doc`; this project owns `zfb.config.ts`, `src/content/` and `src/styles/global.css`.
  - **zudo-wind** — utilities compiled by zfb's built-in engine from the preset's tokens (no Tailwind)
  - **zudo-react** — the owned component runtime used by the package's islands (no Preact)
  ```

- Root `pnpm typecheck` (`pnpm -r --if-present typecheck`) does not include `doc/` (it has `check`, not `typecheck`); `zfb check` runs only via `pnpm --filter zudo-slack-notify-doc check`. Consider adding that to `ci.yml` `build-doc` so a config-loading error surfaces before the build step.

## Step-by-step plan

Adapted from the 7-step checklist (https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist).

1. **Wait for zudo-doc 6.0.0** and read its consumer migration guide (#4473). Nothing below runs before it ships.
2. **Baseline.** `git tag pre-zfb3` on green `main`; keep a `doc/dist` build (`pnpm --filter zudo-slack-notify-doc build`) and screenshots of `/`, `/docs/getting-started/`, one `<Tabs>` page, search open.
3. **Pins.** Edit `doc/package.json` as in §1; `pnpm install`; `pnpm --filter zudo-slack-notify-doc exec zfb --version`.
4. **Re-copy scaffold files.** `pnpm create zudo-doc@6 <scratch>` with the same features as `doc/zfb.config.ts:9-16` (`llmsTxt`, `sidebarResizer`, `sidebarToggle`, `tocToggle`, `imageEnlarge`, `dynamicPageTransition`, `docHistory`, `assetViewer`; search is a `headerRightItems` component), then copy its `app/pages/docs/[[...slug]].tsx`, `app/pages/index.tsx`, `app/tsconfig.json`, `app/src/styles/global.css` over `doc/`; copy its `CLAUDE.md` wording into `doc/CLAUDE.md`.
5. **Config.** `pnpm --filter zudo-slack-notify-doc check` — `zfb check` must load the config (a stale preset fails with the named `framework`/`tailwind` migration error).
6. **CSS + audit.** `pnpm --filter zudo-slack-notify-doc build`; any leftover directive is listed as ZW009 with `file:line:column`. Then `pnpm --filter zudo-slack-notify-doc exec zfb wind audit --fail-on error`.
7. **Verify + ship.** `pnpm b4push`, browser-diff the pages from step 2 (reset table items), open a PR (CI `build-doc`), merge → `deploy-doc.yml` deploys and smokes.

## Verification checklist

- [ ] `pnpm --filter zudo-slack-notify-doc exec zfb --version` → 3.x and `embedded esbuild` only.
- [ ] `pnpm --filter zudo-slack-notify-doc check` green (config loads; no removed-key error).
- [ ] `pnpm --filter zudo-slack-notify-doc build` green; no `ZW009`; no `zfb warn:` pragma lines.
- [ ] `pnpm --filter zudo-slack-notify-doc exec zfb wind audit --fail-on error` exit 0.
- [ ] `pnpm --filter zudo-slack-notify-doc check:links` green; `pnpm b4push` green.
- [ ] Browser: home, `/docs/getting-started/` (`<CategoryNav>`), an API page with `<Tabs>`, search (placeholder colour/opacity per the reset table), theme toggle, doc-history button, image enlarge; compare with the `pre-zfb3` screenshots.
- [ ] `git grep -n preact doc/` → nothing (or comments only).
- [ ] After merge: `deploy-doc.yml` green including the `smoke` job; `doc/CLAUDE.md` no longer mentions Tailwind/Preact.

## Risks and open questions

- **Single upstream dependency.** Everything waits on zudo-doc 6.0.0, which itself waits on the next zfb release (#3569/#3570 fixed on `main` only). No host-side work can be verified before that.
- **Reset differences** are the only visual risk: this site has no custom CSS to protect, but preflight → `owned-v1` (plus whatever the planned authored preflight patch restores, DD4 in #4430) changes form controls, `sub`/`sup`, `[hidden]` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#reset). Check the search box and tabs.
- **Caret ranges** (`^5.28.2`) are safe today (exclude 6.x) but mean a future `^6.0.0` will float within 6.x; keep `pnpm-lock.yaml` committed (it is) and CI frozen (it is).
- Not verified here: `zfb check` cannot run on the clone (preset imports need `node_modules`); the audit used a stand-in `wind: { spec: 1 }` config, so package-owned classes were not evaluated.

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/ (checklist; config keys; pragmas; CLI/env)
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ (directives → ZW009; reset differences)
- https://zfb.takazudomodular.com/zudo-wind/configuration/ , https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- https://zfb.takazudomodular.com/concepts/islands/ , https://zfb.takazudomodular.com/api/cli/ (`zfb wind audit`; `--json`/`--severity`/`--plan` are next zfb release)
- https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/
- zudolab/zudo-doc#4430 (epic), #4477 (root PR), #4473 (consumer migration guide, planned); Takazudo/zudo-front-builder#3569, #3570 (fixed on main, unreleased)
- This repo: `doc/zfb.config.ts`, `doc/package.json`, `doc/tsconfig.json`, `doc/src/styles/global.css`, `doc/pages/docs/[[...slug]].tsx`, `doc/CLAUDE.md`, `.github/workflows/ci.yml`, `.github/workflows/deploy-doc.yml`, `scripts/run-b4push.sh`, `scripts/smoke.sh`
