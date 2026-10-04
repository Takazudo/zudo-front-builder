# zfb v3 migration guide: Takazudo/zudo-cloudflare-wisdom

Generated 2026-10-04 by an automated diagnosis of `main` @ `956e1b2`. Counts come from the commands listed; re-run them locally before relying on them.

This guide is one of six produced from a single analysis of the near-identical `*-wisdom` zudo-doc sites (`zudo-css-wisdom`, `zudo-slack-wisdom`, `zudo-codemirror-wisdom`, `zudo-cloudflare-wisdom`, `zudo-tauri-wisdom`, `zudo-test-wisdom`). `zudo-css-wisdom` was inspected in full; the other five were diffed against it file by file. Paragraphs marked **Shared** are identical in all six guides; **This repo** blocks carry only the deltas. The sibling guides live next to this file as `Takazudo--zudo-*-wisdom.md`; the zudo-doc status review is `zudolab--zudo-doc.md`.

## Verdict

**Blocked** on `@takazudo/zudo-doc` 6.0.0 (this host touches zfb only through the zudo-doc preset). Effort **S** — a template host with no host components, a byte-identical template `global.css`, template `tsconfig.json` and zero content-side audit findings. The one repo-specific zfb touchpoint is `scripts/check-code-parity.mjs`, a CI gate that calls `parseToAst` from `@takazudo/zfb-md-wasm`; that API survives in 3.x (only `jsxRuntime` was removed), so it needs a version bump and a re-run, nothing more. What gates the start: zudo-doc 6.0.0 and its consumer guide (zudolab/zudo-doc#4473) plus the zfb release 6.0.0 pins. No code-level `[prep-now]` work.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `/` (repo root, `zfb.config.ts`, 116 tracked files under `src/`) | `@takazudo/zfb` 2.22.0 + `zfb-adapter-cloudflare`, `zfb-md-wasm`, `zfb-runtime` 2.22.0 (exact, lockstep-gated) | `@takazudo/zudo-doc` ^5.28.1 via `zudoDoc()`; `zudo-doc-history-server` ^5.28.1; no zudo-sg; zdtp removed in #220 (2026-09-28) | only through zfb's embedded engine: `src/styles/global.css` lines 8–9 imports, 20–22 `@source`, 26 empty `@theme` (template, byte-identical to zudo-css-wisdom) | `preact ^10.29.1`, `preact-render-to-string ^6.6.6`; `@jsxImportSource preact` pragmas in the 2 route stubs only; 0 hooks | none host-authored | 2.22.0; **host script** `scripts/check-code-parity.mjs` imports `parseToAst` (CI gate) | Cloudflare Workers, `wrangler.toml` name `zudo-cloudflare-wisdom`, domain `zudo-cloudflare-wisdom.takazudomodular.com`, wrangler 4.85.0; `CUTOVER.md`, `public/_redirects` |

**Shared.** All six sites have exactly one zfb project dir (the repo root, `zfb.config.ts`) and the same host shape, scaffolded by `create-zudo-doc` 5.28.1 (`ZUDO_DEPS_PINS.md`, pinned `7151a5f5…`, updated 2026-09-28) and drift-gated by `scripts/check-template-drift.sh`:

- `zfb.config.ts` = `defineConfig(zudoDoc({ … }))` from `@takazudo/zudo-doc/config`. The host authors **no** `framework` or `tailwind` key; both are emitted by `zudoDoc()` itself (zudo-doc `packages/zudo-doc/src/config.ts:911` `framework: "preact"`, `:913` `tailwind: { enabled: true }`).
- `package.json`: `@takazudo/zfb`, `@takazudo/zfb-adapter-cloudflare`, `@takazudo/zfb-md-wasm`, `@takazudo/zfb-runtime` all exact `2.22.0` (lockstep enforced by `scripts/check-pin-parity.mjs`, which rejects range operators on this group); `@takazudo/zudo-doc` + `@takazudo/zudo-doc-history-server` `^5.28.1`; `preact ^10.29.1`, `preact-render-to-string ^6.6.6`; devDependency `wrangler 4.85.0` (exact; `scripts/check-wrangler-pin.mjs` compares it with the version embedded in the zfb binary).
- Lockfile and sub-packages: one root `pnpm-lock.yaml` (no `doc/` or sub-package lockfile); `pnpm-workspace.yaml` declares no `packages:`, so there is no sub-package and nothing consumes Tailwind through Vite/Next/Astro — nothing to mark unaffected. Both workflows install with `pnpm install --frozen-lockfile`, so the `pnpm add`/`pnpm remove` steps below must commit the regenerated lockfile.
- `pages/`: three template route stubs — `pages/index.tsx` (one-line re-export of `@takazudo/zudo-doc/routes/index`), `pages/docs/[[...slug]].tsx`, `pages/[locale]/docs/[[...slug]].tsx`. The two dynamic stubs open with `/** @jsxRuntime automatic */` and `/** @jsxImportSource preact */` (lines 1–2) and `import type { JSX } from "preact"` (line 27; line 29 in zudo-slack-wisdom), and carry the DocHistory chrome-binding patch (allowlisted in `.template-drift-allowlist`).
- `src/styles/global.css`: the template entry sheet — `@layer zd-preflight, zd-flow;`, `@import "tailwindcss/preflight" layer(zd-preflight)`, `@import "tailwindcss/utilities"`, five `@import "@takazudo/zudo-doc/{theme,safelist,content,page-loading,features}.css"`, `@source` globs for `src/content`, `src/components`, `pages`, and a `@theme {}` override slot. No `tailwindcss` dependency, no `tailwind.config.*`, no `@tailwindcss/*` package, no `ZFB_TAILWIND*` in scripts or CI (grep, 0 hits in every repo).
- Deploy: Cloudflare Workers static assets through `adapter: "@takazudo/zfb-adapter-cloudflare"`; `wrangler.toml` has `main = "./dist/_worker.js"`, `compatibility_flags = ["nodejs_compat"]`, `[assets] directory = "./dist"`, a `[[routes]]` custom domain `<name>.takazudomodular.com`. `.github/workflows/pr-checks.yml` (typecheck, build, html-validate, doc-history, preview upload) and `main-deploy.yml` (build, html-validate, doc-history, `wrangler deploy`, IFTTT).
- zdtp (`@takazudo/zdtp`) was removed from all six on 2026-09-28 (`chore/remove-unused-zdtp-20260928` / `chore/bump-zudo-deps-20260928-*` merges); `designTokenPanel` is not enabled anywhere. No `@takazudo/zudo-sg`.
- Host-authored islands: none (`<Island`, `"use client"`: 0 hits outside MDX prose in every repo). Preact hooks in host code: 0. `className=`/`onClick=`/`onInput=` in host `.tsx`: 0.

## Sequencing and blockers

**Shared.**

1. **Blocked now.** This host reaches zfb only through the `@takazudo/zudo-doc` preset, and zudo-doc 5.28.x is Preact + Tailwind: `zudoDoc()` emits the removed `framework` and `tailwind` keys, the package's routes/islands are Preact, and its shipped `tsconfig.base.json` sets `jsxImportSource: "preact"`. Under zfb 3 a preset that still supplies a removed key fails config loading even after the app removes its own copy, so the preset must be upgraded first (https://zfb.takazudomodular.com/guides/migrating-to-v3/ "Before you start" and "Configuration"). There is nothing a host can change in `zfb.config.ts` today to get past that.
2. **Upstream gate: zudo-doc 6.0.0.** Epic zudolab/zudo-doc#4430, root PR zudolab/zudo-doc#4477 (draft), consumer migration guide zudolab/zudo-doc#4473 (EN + JA `guides/migrating-to-zudo-doc-6.mdx`, open). The integration floor #4467 is blocked on zfb bugs Takazudo/zudo-front-builder#3569 (Wind extractor UTF-8 offsets) and #3570 (quoted island markers in highlighted code); both are fixed on zfb `main` but unreleased, so 6.0.0 will pin the **next zfb release after 3.1.0**, not 3.1.0. Do not start this host on 3.1.0.
3. **Do now (prep, no behaviour change on 2.22.0):** the items tagged `[prep-now]` in "Required changes". Everything else waits for 6.0.0 and its consumer guide.
4. **Hold the pins meanwhile.** npm `latest` for every `@takazudo/zfb*` package has resolved to 3.x since 2026-09-30. `scripts/check-pin-parity.mjs` keeps the four zfb packages equal but cannot stop a family-wide jump to 3.1.0, and `/dev-bump-zudo-deps` resolves a stable pin to the newest stable. Until zudo-doc 6.0.0 ships, bump only `@takazudo/zudo-doc` / `@takazudo/zudo-doc-history-server` 5.28.x patches and leave the zfb family at 2.22.x.
5. **After 6.0.0:** one migration PR per repo following "Step-by-step plan"; the six repos are independent and can be done in any order (zudo-css-wisdom is the largest; do it last or first as a learning exercise, not in the middle).

**This repo.** No v3 branch exists on `origin` (2 remote branches); last commit 2026-09-28 (`#220`, zdtp removal). No code-level prep before 6.0.0; the flip is a template re-adoption plus re-running the code-parity gate on md-wasm 3.x.

## Required changes

### 1. Dependencies, config, tsconfig, env

**Shared.**

- `package.json` (what/why: https://zfb.takazudomodular.com/guides/migrating-to-v3/ "Components and islands"; zudolab/zudo-doc#4473 locked spec): bump `@takazudo/zudo-doc` and `@takazudo/zudo-doc-history-server` to `^6.0.0` first, then set the four `@takazudo/zfb*` packages to the exact version zudo-doc 6.0.0's `peerDependencies` name (read `node_modules/@takazudo/zudo-doc/package.json` after the preset bump; #4473 round 2 says `^3.1.0` floor "or the later aligned release pin recorded by #4467/#4475"). Keep them exact — `scripts/check-pin-parity.mjs:49-54` rejects `^`/`~` on this group. Remove `preact` and `preact-render-to-string`: they are present only for zfb's old engine and zudo-doc 5.x's peer list; zdtp (the one thing that still needs a Preact peer in 6.0) is not used here. Keep `diff`, `katex`, `minisearch`, `zod` until 6.0.0's peer list says otherwise (5.28.2 peers them).
- `zfb.config.ts`: nothing to delete — the host never authored `framework`/`tailwind`. After 6.0.0, token overrides move to `zudoDoc({ wind })` or authored `:root` custom properties (#4473). Expect `zudoDoc()` to declare the package manifest (`wind.manifests["zudo-doc"]` → `@takazudo/zudo-doc/wind.json`, planning map `explore/pkg-build.md` §1.4) — a bare package specifier resolves through the producer's `exports` (https://zfb.takazudomodular.com/zudo-wind/configuration/ "Manifest path resolution"); the host does not declare it.
- `tsconfig.json`: the template extends `@takazudo/zudo-doc/tsconfig.base.json` (which in 5.28.x supplies `jsx: "react-jsx"`, `jsxImportSource: "preact"` and loads the package's `zfb-config-shim.d.ts` + `virtual-modules.d.ts`) and adds three `paths` aliases `react`, `react/jsx-runtime`, `react-dom` → `./node_modules/preact/…`. 6.0.0's base flips the JSX source to `@takazudo/zfb/zudo-react`; the host deletes the three aliases (#4473 "remove the react→preact paths"; https://zfb.takazudomodular.com/guides/migrating-to-v3/ "Components and islands").
- Env: no `ZFB_TAILWIND_BIN` / `ZFB_TAILWIND_OXIDE_WARMUP` anywhere (grep over `package.json`, `scripts/`, `.github/`): nothing to remove.
- wrangler: the 3.1.0 linux-x64 binary embeds expected wrangler `4.85.0` (checked with the exact regex `scripts/check-wrangler-pin.mjs:95-97` uses; zfb `main` `crates/zfb-toolchain-pins` also says 4.85.0), so the current `wrangler 4.85.0` devDependency stays valid and the extraction anchor still matches. Re-run `pnpm check:wrangler-pin` after the bump anyway — the pinned release may move.
- `ZUDO_DEPS_PINS.md`: after the flip, re-point `pinned:` at the create-zudo-doc 6.0.x tag and list which of the 11 tracked template files were re-adopted.

**This repo.**

- `tsconfig.json` is the stock template (13 lines): `extends: "@takazudo/zudo-doc/tsconfig.base.json"`, `include: ["src", "pages", "zfb.config.ts"]`, three aliases to delete at lines 8–10. No host `zfb-shim.d.ts`.
- Dependency deltas vs zudo-css-wisdom: no `minisearch`, no `pagefind`; `dev` = `run-parallel dev:zfb dev:history` (`doc-history-server --port 4322`) — no devDependency provides a `run-parallel` binary (`npm-run-all2` is absent here; the siblings use its `run-p`), a zfb-agnostic defect to fix separately; `engines.node >=22`; `.npmrc` has only `trust-policy-exclude[]=undici-types@6.21.0` and `public-hoist-pattern[]=miniflare` (no `strictDepBuilds`); `pnpm-workspace.yaml` identical to zudo-css-wisdom's (so `minimumReleaseAgeExclude` already covers `@takazudo/*`).
- `scripts/check-code-parity.mjs` resolves `@takazudo/zfb-md-wasm` from the root `dependencies`; the exact-pin rule keeps it on the same version as the engine.

### 2. CSS and utilities

**Shared.**

- Every Tailwind construct in `src/styles/global.css` is a ZW009 build error under zfb 3, even under `wind: false` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ "Directives"): the two `@import "tailwindcss/…"` lines, every `@source`, and the `@theme` block. The plain `@layer zd-preflight, zd-flow;` statement itself is allowed ("Ordinary `@layer` statements … remain valid"), but whether 6.0.0 keeps those layer names is the template's decision.
- `@import "@takazudo/zudo-doc/safelist.css"` must go too: that file is one `@source inline("…")` directive (2,693 tokens today, zudo-doc planning map `explore/css-wind.md` §3), so importing it is itself ZW009. 6.0.0 replaces it with a strict `wind.json` candidate manifest declared by `zudoDoc()` — "automatic through `zudoDoc()`", #4473 — so the host imports nothing for it. Whether `theme.css` / `content.css` / `page-loading.css` / `features.css` stay as host-side `@import`s is decided by 6.0.0's template; take the template file rather than editing the 5.x one.
- Do not hand-port `global.css`. `scripts/check-template-drift.sh:31-37` already downloads the create-zudo-doc release matching the **installed** zudo-doc version, so after the preset bump `pnpm check:template-drift` prints `[DIFF] src/styles/global.css`; copy the 6.0 template from `node_modules/.cache/create-zudo-doc-templates/<version>/templates/base/src/styles/global.css`, then re-apply only the host's authored overrides (listed under **This repo**). Move any `@theme` token overrides to authored `:root { … }` custom properties or `zudoDoc({ wind })` (#4473 "override tokens via `zudoDoc({ wind })` or `:root`").
- Reset: v3 ships no implicit preflight; `zudoDoc()` chooses `wind.reset` for the site (planning recommends `owned-v1` plus an authored preflight-delta in `@layer base`). The host's job is the step-3 checklist of https://zfb.takazudomodular.com/guides/migrating-to-v3/: compare `html` and `code`/`pre` font stacks, `sub`/`sup`, `::placeholder`, `::file-selector-button`, `[hidden]`, lists, headings, borders and form controls (search input, language switcher) on representative pages in a browser — "a pixel diff of a page without those elements cannot catch their differences". HtmlPreview iframes carry the package's own preflight (`html-preview-wrapper/preflight.ts`) and are not affected by the page reset.
- Cascade: zudo-wind emits utilities **after** authored global CSS by default, so an authored unlayered rule of specificity (0,1,0) that sets the same property as a package utility on the same element flips its winner compared with Tailwind v4 (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ "Utility placement and ties"). `wind.utilities.placement: "before-authored"` restores Tailwind's order but is on zfb `main` only — **next zfb release** (https://zfb.takazudomodular.com/zudo-wind/configuration/ "Fields", `utilities`). `group-*`/`peer-*` drop from (0,2,0) to (0,1,0) under either placement ("Relation variant specificity"; Takazudo/zudo-front-builder#3386). Exposure for each host is only its authored rules in `global.css`, listed under **This repo**.
- Audit result shared by all six (`zfb wind audit --project-root <repo>` with zfb 3.1.0 and a bare `wind: { spec: 1 }` config, exit 0, `outcome: complete`, `spec: 1 revision 3`): `unrecognized classes: (none)`, `conflicts: (none)`, `dead classes: (none)`; 15 `auditInfo` entries from the three route stubs — import specifiers like `@takazudo/zudo-doc/route-context` (ZW005 "invalid slash modifier"), `virtual:zudo-doc-route-context` (ZW002 "unknown or unconfigured variant"), the template literals `` `docs;${locale}` `` / `` `locale-docs;${locale}` `` (ZW012 dynamic construction), and the `zudolab/zudo-doc#2651` comment in `pages/index.tsx` (ZW005). All are lower-confidence string-literal origins; they never fail a build (https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/ "Four candidate outcomes"). The audit scans only the host's own roots, so package-owned candidates are invisible until 6.0.0's manifest exists; `zfb check` could not run on the bare clone (preset import needs `node_modules`).
- On 3.1.0 `zfb wind audit` accepts only `--project-root` and `--fail-on error|warning` (`zfb wind audit --help`). `--json`, `--severity`, `--plan build`, `file:line:col` origins, `zfb wind manifest`, `wind.strict`/ZW014 and `wind.sources` are documented on `main` and ship in the **next zfb release** (https://zfb.takazudomodular.com/api/cli/ "zfb wind", https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/ "Migration warnings and strict mode").

**This repo.**

- `src/styles/global.css` is byte-identical to zudo-css-wisdom's (the 5.28.1 template). ZW009 sources: line 8, line 9, lines 20–22 (`@source` ×3), line 26 (`@theme {}`, empty — delete). No authored rules → no placement/tie-flip exposure.
- `themePack: "matcha"` (`zfb.config.ts:42`): package-owned pack CSS; verify it paints after the flip (display/sans/mono faces, surfaces, palette) in both color modes rather than assuming it.
- Audit: 15 diagnostics total, all from the three route stubs; 0 in `src/content/**`. No `class=`/`className=` in MDX at all.

### 3. Components and islands

**Shared.**

- The two dynamic route stubs are host-owned copies of package routes. Each must lose lines 1–2 (`/** @jsxRuntime automatic */`, `/** @jsxImportSource preact */`) and the `import type { JSX } from "preact"` line, and re-type `JSX.Element` with whatever 6.0.0's template exports. A leftover per-file pragma overrides `tsconfig.json`: with `preact` uninstalled the build fails with `Could not resolve "preact/jsx-runtime"`; `zfb build`/`zfb dev` first print a `zfb warn:` line with the file, line and column of each pragma (https://zfb.takazudomodular.com/guides/migrating-to-v3/ "Components and islands"). Audit the removal with `grep -rln "@jsxImportSource" pages src` (expected: 0).
- Take the 6.0.0 template versions of all three `pages/` files (`pnpm check:template-drift` will flag them) and re-apply the DocHistory patch the way 6.0's `create-zudo-doc` emits it (`docHistory: true` stays enabled); the imports `createRouteContext`, `createChrome`, `DocHistory`, `defineChromeBindings` and the two `virtual:zudo-doc-*` modules are package API whose 6.0 shapes #4473 covers ("chrome bindings and ejected components in zudo-react (`class`, `on:click`, signals)", "Child/Description/Component/JSX type migration").
- `pages/index.tsx` stays a one-line re-export; nothing to port.
- Islands, hydration, form models: none are host-authored; the package owns search, theme toggle, language switcher, sidebar, image enlarge, doc history. Verify them in a browser after the flip rather than porting anything.

**This repo.** Only the three template stubs (byte-identical to zudo-css-wisdom's). MDX uses only package components (`<Note` ×4, `<Tip` ×2, `<CategoryNav` ×1); nothing to port.

### 4. md-wasm and other packages

**Shared.**

- `@takazudo/zfb-md-wasm 2.22.0` is pinned only because zudo-doc peer-requires it and `scripts/check-pin-parity.mjs:37-47` keeps it in the exact-lockstep group. The v3 API delta is the removal of the `jsxRuntime` option on `compile()`/`renderHtml()` (https://zfb.takazudomodular.com/api/md-wasm/; https://zfb.takazudomodular.com/guides/migrating-to-v3/ "MDX and md-wasm"); `grep -rn jsxRuntime` over host code returns only the `/** @jsxRuntime automatic */` pragmas, which are unrelated. Bump in lockstep with the family.
- `@takazudo/zudo-doc-history-server` (`doc-history-generate` in CI, `doc-history-server` in the `dev` scripts of the template hosts) is a Node CLI with no zfb engine dependency; bump to the 6.0.x lockstep version only because the pin-parity gate groups it with zudo-doc.

**This repo.**

- `scripts/check-code-parity.mjs:7` `import { parseToAst } from "@takazudo/zfb-md-wasm";` and `:191` `await parseToAst(source, { … })` extract fenced code blocks from every EN/JA page pair and diff them (`createTwoFilesPatch` from `diff`). It runs in `pr-checks.yml:157-161` (`pnpm test:code-parity`, `pnpm check:code-parity`) and in `scripts/run-b4push.sh`.
- `parseToAst(source, options?)` is still exported by md-wasm 3.x (https://zfb.takazudomodular.com/api/md-wasm/ "`parseToAst(source, options?)`"; the v3 delta is the removal of `jsxRuntime` on `compile()`/`renderHtml()` only). The options object at line 191 must stay within `ParseToAstOptions` ("Visitor/serializer options are not accepted"). Action: bump in lockstep, then run `pnpm test:code-parity && pnpm check:code-parity`; if the raw mdast shape changed, `toMdastRoot` (the strict adapter documented on the same page) is the supported way to normalise it.

### 5. Tests, CI, deploy

**Shared.**

- Workflows carry no Tailwind environment and no `tailwindcss` install step, so https://zfb.takazudomodular.com/guides/migrating-to-v3/ "CLI and environment" has nothing to remove here.
- `pr-checks.yml` `typecheck` → `pnpm check` (`zfb check`) and `build-site` → `pnpm build` are the gates that will fail first on 6.0.0 if a Preact pragma, a `react` path alias or a Tailwind directive survives; the failure texts are the ones quoted in the migration guide (`Could not resolve "preact/jsx-runtime"`, `ZW009 …`).
- `.assetsignore`: both workflows `printf '%s\n' '_worker.js' '_zfb_inner.mjs' > dist/.assetsignore` before deploy (`main-deploy.yml` "Prepare deploy directory", `pr-checks.yml` "Prepare dist for deployment"). The v3 adapter emits `dist/.assetsignore` itself, listing `_worker.js`, `_zfb_inner.mjs` and every emitted Wasm basename (https://zfb.takazudomodular.com/guides/ssr-and-cloudflare-bindings/). The `>` overwrite would drop the Wasm lines; after the first 6.0.0 build inspect `dist/.assetsignore` and either delete the step or change it to append (`>>`) only the names that are missing.
- Add the audit as a gate once on v3: a step `pnpm exec zfb wind audit --fail-on error` after `pnpm build` (migration checklist step 6). `--fail-on` exists on 3.1.0 (v3.1.0 changelog "Add failure thresholds to `zfb wind audit`"); `--json`/`--severity` for machine-readable output are **next zfb release**.
- `check:template-drift` auto-follows the installed zudo-doc version, so on 6.0.0 it compares against create-zudo-doc 6.0.x; every entry in `.template-drift-allowlist` must be re-justified against the new template (the comments in that file reference the 5.22.0/5.27.0 baselines).
- `check:pin-parity` and `check:wrangler-pin` keep working unchanged (see §1). `check:html` (`html-validate dist/**/*.html`) stays the renderer-vocabulary gate (the `meta property` / `link as` / `svg xmlns` fixes of Takazudo/zudo-front-builder#3359 are on `main`, **next zfb release**; 6.0.0 will have been built against them).
- `scripts/run-b4push.sh` mirrors the same steps locally; keep it in sync when adding the audit step.

**This repo.**

- `pr-checks.yml` adds `check-category-meta` and, inside `typecheck`, the two code-parity steps (`:157-161`) before `test:links`, `check:wrangler-pin` and `pnpm check`; `check:links` uses `--strict-anchors`. The `.assetsignore` overwrite is at `main-deploy.yml:199` and `pr-checks.yml:339`.
- `CUTOVER.md` records the Pages→Workers cutover (done); `public/_redirects` carries legacy redirects. Neither changes for v3.
- `CLAUDE.md:3` and `README.md` mention "Tailwind CSS v4" (1 line each); rewrite after the flip.

## Step-by-step plan

**Shared** (the official 7-step checklist from https://zfb.takazudomodular.com/guides/migrating-to-v3/ "Checklist", adapted; steps marked `[prep-now]` can be done on 2.22.0 today).

1. `[prep-now]` Branch `chore/zfb3-prep`: apply the **This repo** prep items below; `pnpm b4push`; merge. Nothing here changes the built site.
2. Wait for `@takazudo/zudo-doc` 6.0.0, read `guides/migrating-to-zudo-doc-6` (zudolab/zudo-doc#4473) and the zfb release notes it pins. Confirm `main` is a green 2.22.0 baseline (`pnpm b4push`).
3. Branch `chore/zfb3-migration`. Upgrade the preset first, then the engine family to the exact version 6.0.0 peers:
   ```sh
   pnpm add @takazudo/zudo-doc@^6.0.0 @takazudo/zudo-doc-history-server@^6.0.0
   node -p "require('@takazudo/zudo-doc/package.json').peerDependencies"   # read the zfb floor X
   pnpm add --save-exact @takazudo/zfb@X @takazudo/zfb-adapter-cloudflare@X @takazudo/zfb-md-wasm@X @takazudo/zfb-runtime@X
   pnpm remove preact preact-render-to-string
   pnpm exec zfb --version          # prints the zfb version and "embedded esbuild" only; no Tailwind line
   pnpm check:pin-parity && pnpm check:wrangler-pin
   ```
4. Re-adopt the scaffold: `pnpm check:template-drift` lists `[DIFF]`/`[MISSING]` files; copy each from `node_modules/.cache/create-zudo-doc-templates/<6.0.x>/templates/base/` (and `templates/features/i18n/files/`), then re-apply the allowlisted host customizations (DocHistory patch in the two dynamic stubs, favicons, repo-specific scripts). Delete the three `react*` aliases from `tsconfig.json` `paths`. `grep -rln "@jsxImportSource" pages src` must print nothing.
5. `src/styles/global.css`: take the template, re-apply the authored overrides listed under **This repo** (move former `@theme` values to `:root` or `zudoDoc({ wind })`), then `pnpm check && pnpm build`. Fix every `ZW009` the build names (one run lists them all with `file:line:column`).
6. `pnpm exec zfb wind audit --fail-on error`; `pnpm check:html`; `pnpm check:links`; `pnpm test:links`; `pnpm check:template-drift`; inspect `dist/.assetsignore` and adjust the two workflow steps (§5).
7. `pnpm dev`, then walk the "Verification checklist" in a browser (EN and JA). Update `CLAUDE.md`, `README.md`, `.claude/**` wording ("Tailwind CSS v4", "Preact islands") and `ZUDO_DEPS_PINS.md`.
8. Open the PR; `pr-checks.yml` posts the Workers preview URL; verify it; merge; watch `main-deploy.yml` and the IFTTT notification.

**This repo — prep items for step 1:** none required. **Flip-time additions:** in step 6 run `pnpm test:code-parity && pnpm check:code-parity` against md-wasm 3.x (§4) before pushing.

## Verification checklist

**Shared.**

- [ ] `pnpm exec zfb --version` shows the pinned 3.x version and `embedded esbuild` only.
- [ ] `pnpm check:pin-parity` OK (four zfb packages exact and equal; zudo-doc pair equal). `pnpm check:wrangler-pin` OK.
- [ ] `pnpm check` (zfb check) and `pnpm build` green; no `zfb warn:` pragma line in the build log; `grep -rln "@jsxImportSource" pages src` empty; `grep -nE '@import "tailwindcss|@source|@theme|@apply' src/styles/global.css` empty.
- [ ] `pnpm exec zfb wind audit --fail-on error` exit 0; the report has no strict-origin (`class`-position) diagnostics.
- [ ] `pnpm check:html`, `pnpm check:links`, `pnpm test:links`, `pnpm check:template-drift`, `pnpm format:md:check` green; `pnpm b4push` green.
- [ ] `dist/_worker.js`, `dist/_zfb_inner.mjs`, `dist/.assetsignore` present; `.assetsignore` lists both plus any `*.wasm`.
- [ ] Browser (`pnpm dev`, then the PR preview URL), EN and JA: home (`home.wide`), one doc page per header-nav category, `/ja/docs/…` translated and fallback pages, 404, `/sitemap.xml`, `/robots.txt`, `/llms.txt`; header nav labels; search opens and returns results; theme toggle persists; language switcher; sidebar resizer/toggle; image enlarge; the doc-history button hydrates on both dynamic routes and loads `/doc-history/*` JSON.
- [ ] Reset checklist on a doc page containing lists, headings, inline `code`, a `pre` block, `sub`/`sup` and the search `input`: compare computed `font-family` on `html` and `code`, list markers, heading sizes, `::placeholder` colour, control backgrounds with the 5.28 production site (Level 5: computed styles + screenshots, not unit tests).
- [ ] `wrangler versions upload --preview-alias` and `wrangler deploy` succeed with wrangler 4.85.0 (or the re-pinned version).

**This repo.**

- [ ] `pnpm test:code-parity` and `pnpm check:code-parity` green on the bumped `@takazudo/zfb-md-wasm`.
- [ ] The `matcha` theme pack paints on `/` and `/docs/workers/` in both color modes; Noto Sans JP is not used here (no `head` fonts), so only the pack faces matter.

## Risks and open questions

**Shared.**

- zudo-doc 6.0.0 has no release date; by 2026-10-02 39 of 51 topics were merged only into the maintainer's local base branch and the remote `base/zfb3-migration` carries only planning resources (see `zudolab--zudo-doc.md`). Any host work beyond `[prep-now]` before 6.0.0 ships will be redone.
- 6.0.0 must re-pin from zfb 3.1.0 to the next release (#3569/#3570 fixes). Installing 3.1.0 here would reproduce those bugs in highlighted code and non-ASCII content (all six sites have Japanese content).
- Utility placement flip and `group-*`/`peer-*` specificity drop (#3386) are package-owned concerns, but any authored host rule in `global.css` of specificity (0,1,0) is exposed; `placement: "before-authored"` is **next zfb release**.
- The zudo-react client runtime makes no bundle-size promise (https://zfb.takazudomodular.com/guides/migrating-to-v3/ "Components and islands", the paragraph under "Optional island props": "no promise to match Preact's bundle size … no fixed byte budget"; Takazudo/zudo-front-builder#3383 measured a trivial island at 48 KB / 16.6 KB gz vs 18 KB / 7.4 KB gz on 2.15.0, about 2.2×). The sites ship search/theme/sidebar islands from the package; expect a larger JS payload and re-check Lighthouse if it matters.
- Open: which `@takazudo/zudo-doc/*.css` imports the 6.0 template keeps (#4473 round 2 mentions "public CSS exports" while also voiding "physical CSS imports" as the migration path); whether `zd-flow` survives; the exact zfb version 6.0.0 pins (#4467/#4475). Resolve by reading the 6.0 template, not by guessing.
- Open: `.template-drift-allowlist` entries written against the 5.22–5.28 templates need re-justification; a stale entry hides real drift.

**This repo.** `parseToAst`'s raw tree is the forward-compatible tier; a node-shape change in 3.x could alter which fenced blocks the parity script sees. The script's own regression test (`scripts/check-code-parity.test.mjs`) is the detector — run it first.

## References

- zfb: https://zfb.takazudomodular.com/guides/migrating-to-v3/ · https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ · https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/ · https://zfb.takazudomodular.com/zudo-wind/configuration/ · https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/ · https://zfb.takazudomodular.com/zudo-wind/cascade-and-reset/ · https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/ · https://zfb.takazudomodular.com/zudo-react/components-and-jsx/ · https://zfb.takazudomodular.com/concepts/islands/ · https://zfb.takazudomodular.com/api/cli/ · https://zfb.takazudomodular.com/api/define-config/ · https://zfb.takazudomodular.com/api/md-wasm/ · https://zfb.takazudomodular.com/guides/ssr-and-cloudflare-bindings/ · https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ · https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/
- zfb issues: Takazudo/zudo-front-builder#3569, #3570 (fixed on `main`, unreleased), #3386 (placement/specificity), #3383 (bundle size), #3359 (renderer vocabulary).
- zudo-doc: epic zudolab/zudo-doc#4430 · root PR zudolab/zudo-doc#4477 · consumer migration guide zudolab/zudo-doc#4473 · integration floor zudolab/zudo-doc#4467 · planning resources `_temp-resource/4430-zfb3-migration/` on branch `base/zfb3-migration` (`explore/css-wind.md`, `explore/pkg-build.md`, `upstream-issues.md`).
- Sibling guides in this directory: `Takazudo--zudo-css-wisdom.md`, `Takazudo--zudo-slack-wisdom.md`, `Takazudo--zudo-codemirror-wisdom.md`, `Takazudo--zudo-cloudflare-wisdom.md`, `Takazudo--zudo-tauri-wisdom.md`, `Takazudo--zudo-test-wisdom.md`; `zudolab--zudo-doc.md`.
- Commands used for the counts (run from each clone): `git status --short`; `grep -rnE '@import\s+"tailwindcss|@tailwind|@theme|@apply|@source|@utility|@variant|@custom-variant|@plugin|@config|theme\(' --include=*.css --include=*.ts --include=*.tsx --include=*.mjs .`; `grep -rnE "from ['\"]preact|preact/hooks|@jsxImportSource" --include=*.ts --include=*.tsx pages src scripts`; `grep -rnE 'useState|useEffect|useRef|useMemo|useCallback|useContext|createPortal|forwardRef|dangerouslySetInnerHTML|className=|onClick=|onInput=' pages src/components`; the audit recipe: back up `zfb.config.ts`, write `export default defineConfig({ wind: { spec: 1 } })`, `zfb wind audit --project-root .` with zfb 3.1.0, restore the config.
- Repo-specific: Takazudo/zudo-cloudflare-wisdom#220 (zdtp removal); `scripts/check-code-parity.mjs:7,191`; `scripts/check-code-parity.test.mjs`; `.github/workflows/pr-checks.yml:157-161`.
