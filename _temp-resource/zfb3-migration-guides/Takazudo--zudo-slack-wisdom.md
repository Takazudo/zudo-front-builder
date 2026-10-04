# zfb v3 migration guide: Takazudo/zudo-slack-wisdom

Generated 2026-10-04 by an automated diagnosis of `main` @ `3003754`. Counts come from the commands listed; re-run them locally before relying on them. The `zfb wind audit` census below was taken with the zfb 3.1.0 CLI and re-run with 3.2.0; zfb 3.2.0 shipped on npm `latest` at 15:30 UTC on 2026-10-04 while this guide was being written, so every item formerly labelled "next zfb release" now reads "zfb 3.2.0".

This guide is one of six produced from a single analysis of the near-identical `*-wisdom` zudo-doc sites (`zudo-css-wisdom`, `zudo-slack-wisdom`, `zudo-codemirror-wisdom`, `zudo-cloudflare-wisdom`, `zudo-tauri-wisdom`, `zudo-test-wisdom`). `zudo-css-wisdom` was inspected in full; the other five were diffed against it file by file. Paragraphs marked **Shared** are identical in all six guides; **This repo** blocks carry only the deltas. The sibling guides live next to this file as `Takazudo--zudo-*-wisdom.md`; the zudo-doc status review is `zudolab--zudo-doc.md`.

## Verdict

**Blocked** on `@takazudo/zudo-doc` 6.0.0 (this host touches zfb only through the zudo-doc preset). Effort **S** — a pure template host: no host components, no authored CSS rules, template `tsconfig.json`, zero content-side audit findings. The repo-specific surface is configuration only (`themePack: "observatory"`, `dynamicPageTransition: true`, `tocToggle: true`, Noto Sans JP `head` links) plus three scaffolded `.claude/skills/zudo-doc-*` skill files whose text describes Tailwind/Preact. What gates the start: zudo-doc 6.0.0 and its consumer guide (zudolab/zudo-doc#4473) plus the zfb release 6.0.0 pins (3.2.0 or later — zfb 3.2.0, released 2026-10-04, already carries the fixes zudo-doc 6 waited for, so zfb itself no longer gates this). There is no code-level `[prep-now]` work; the flip is a template re-adoption.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `/` (repo root, `zfb.config.ts`, 107 tracked files under `src/`) | `@takazudo/zfb` 2.22.0 + `zfb-adapter-cloudflare`, `zfb-md-wasm`, `zfb-runtime` 2.22.0 (exact, lockstep-gated) | `@takazudo/zudo-doc` ^5.28.1 via `zudoDoc()`; `zudo-doc-history-server` ^5.28.1; no zudo-sg; zdtp removed in #79 (2026-09-28) | only through zfb's embedded engine: `src/styles/global.css` lines 8–9 imports, 20–22 `@source`, 26 empty `@theme` (template, byte-identical to zudo-css-wisdom) | `preact ^10.29.1`, `preact-render-to-string ^6.6.6`; `@jsxImportSource preact` pragmas in the 2 route stubs only; 0 hooks | none host-authored (the one `client:` grep hit is prose in `worker-backend/index.mdx:12`) | 2.22.0 pinned for zudo-doc's peer; no host calls | Cloudflare Workers, `wrangler.toml` name `zudo-slack-wisdom`, domain `zudo-slack-wisdom.takazudomodular.com`, wrangler 4.85.0 |

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
2. **Upstream gate: zudo-doc 6.0.0.** Epic zudolab/zudo-doc#4430, root PR zudolab/zudo-doc#4477 (draft), consumer migration guide zudolab/zudo-doc#4473 (EN + JA `guides/migrating-to-zudo-doc-6.mdx`, open). The integration floor #4467 is blocked on zfb bugs Takazudo/zudo-front-builder#3569 (Wind extractor UTF-8 offsets) and #3570 (quoted island markers in highlighted code); both shipped in **zfb 3.2.0 (released 2026-10-04)**, so the zudo-doc side is unblocked and 6.0.0's zfb floor is 3.2.0 or later, not 3.1.0 (3.1.0 still carries the #3569 build panic). zudo-doc's `base/zfb3-migration` is pushed (`70e0875`; #4477 now carries 102 commits), with only the #4467 integration topic still local; 6.0.0 itself is not published (`@takazudo/zudo-doc` `latest` is 5.28.2). Do not start this host on 3.1.0.
3. **Do now (prep, no behaviour change on 2.22.0):** the items tagged `[prep-now]` in "Required changes". Everything else waits for 6.0.0 and its consumer guide.
4. **Hold the pins meanwhile.** npm `latest` for every `@takazudo/zfb*` package has resolved to 3.x since 2026-09-30. `scripts/check-pin-parity.mjs` keeps the four zfb packages equal but cannot stop a family-wide jump to 3.2.0, and `/dev-bump-zudo-deps` resolves a stable pin to the newest stable. Until zudo-doc 6.0.0 ships, bump only `@takazudo/zudo-doc` / `@takazudo/zudo-doc-history-server` 5.28.x patches and leave the zfb family at 2.22.x.
5. **After 6.0.0:** one migration PR per repo following "Step-by-step plan"; the six repos are independent and can be done in any order (zudo-css-wisdom is the largest; do it last or first as a learning exercise, not in the middle).

**This repo.** No v3 branch exists on `origin` (2 remote branches); last commit 2026-09-28 (`#79`, zdtp removal). No code-level prep is possible before 6.0.0; the optional prep is editorial (§5, the scaffolded skill texts).

## Required changes

### 1. Dependencies, config, tsconfig, env

**Shared.**

- `package.json` (what/why: https://zfb.takazudomodular.com/guides/migrating-to-v3/ "Components and islands"; zudolab/zudo-doc#4473 locked spec): bump `@takazudo/zudo-doc` and `@takazudo/zudo-doc-history-server` to `^6.0.0` first, then set the four `@takazudo/zfb*` packages to the exact version zudo-doc 6.0.0's `peerDependencies` name (read `node_modules/@takazudo/zudo-doc/package.json` after the preset bump; #4473 round 2 says `^3.1.0` floor "or the later aligned release pin recorded by #4467/#4475"). Expect that pin to be exact `3.2.0` or later: 3.2.0 is a minor, so a `^3.1.0` floor would still admit 3.1.0 and its build-panic bug (Takazudo/zudo-front-builder#3569) — do not settle below 3.2.0. Keep them exact — `scripts/check-pin-parity.mjs:49-54` rejects `^`/`~` on this group. Remove `preact` and `preact-render-to-string`: they are present only for zfb's old engine and zudo-doc 5.x's peer list; zdtp (the one thing that still needs a Preact peer in 6.0) is not used here. Keep `diff`, `katex`, `minisearch`, `zod` until 6.0.0's peer list says otherwise (5.28.2 peers them).
- `zfb.config.ts`: nothing to delete — the host never authored `framework`/`tailwind`. After 6.0.0, token overrides move to `zudoDoc({ wind })` or authored `:root` custom properties (#4473). Expect `zudoDoc()` to declare the package manifest (`wind.manifests["zudo-doc"]` → `@takazudo/zudo-doc/wind.json`, planning map `explore/pkg-build.md` §1.4) — a bare package specifier resolves through the producer's `exports` (https://zfb.takazudomodular.com/zudo-wind/configuration/ "Manifest path resolution"); the host does not declare it.
- `tsconfig.json`: the template extends `@takazudo/zudo-doc/tsconfig.base.json` (which in 5.28.x supplies `jsx: "react-jsx"`, `jsxImportSource: "preact"` and loads the package's `zfb-config-shim.d.ts` + `virtual-modules.d.ts`) and adds three `paths` aliases `react`, `react/jsx-runtime`, `react-dom` → `./node_modules/preact/…`. 6.0.0's base flips the JSX source to `@takazudo/zfb/zudo-react`; the host deletes the three aliases (#4473 "remove the react→preact paths"; https://zfb.takazudomodular.com/guides/migrating-to-v3/ "Components and islands").
- Env: no `ZFB_TAILWIND_BIN` / `ZFB_TAILWIND_OXIDE_WARMUP` anywhere (grep over `package.json`, `scripts/`, `.github/`): nothing to remove.
- wrangler: the 3.1.0 linux-x64 binary embeds expected wrangler `4.85.0` (checked with the exact regex `scripts/check-wrangler-pin.mjs:95-97` uses) and the v3.2.0 tag's `crates/zfb-toolchain-pins` still says `EXPECTED_WRANGLER_VERSION = "4.85.0"`, so the current `wrangler 4.85.0` devDependency stays valid and the extraction anchor still matches. Re-run `pnpm check:wrangler-pin` after the bump anyway — the pinned release may move.
- `ZUDO_DEPS_PINS.md`: after the flip, re-point `pinned:` at the create-zudo-doc 6.0.x tag and list which of the 11 tracked template files were re-adopted.

**This repo.**

- `tsconfig.json` is the stock template (13 lines): `extends: "@takazudo/zudo-doc/tsconfig.base.json"`, `include: ["src", "pages", "zfb.config.ts"]`, and the three aliases to delete at lines 8–10. No host `zfb-shim.d.ts` (the package base supplies one that re-exports the installed `@takazudo/zfb/config`, so config typing follows the engine automatically).
- Scripts/deps beyond zudo-css-wisdom: `dev` = `run-p dev:zfb dev:history` (`npm-run-all2 ^7.0.2`, `doc-history-server --port 4322`), `prepare: lefthook install`, `engines.node >=22`, `check:links … --strict-anchors`. None involve zfb directly.
- `.npmrc` carries only `trust-policy-exclude[]=undici-types@6.21.0` and `public-hoist-pattern[]=miniflare` (zudo-css-wisdom's adds `strictDepBuilds=true` and `ignoredBuilds`). `pnpm-workspace.yaml` sets `minimumReleaseAge: 0` instead of zudo-css-wisdom's `minimumReleaseAgeExclude` list (its comment: pnpm's exclude matcher cannot match this project's peer-nested lockfile keys), so a day-0 6.0.0 install is not held back by the release-age gate here either.

### 2. CSS and utilities

**Shared.**

- Every Tailwind construct in `src/styles/global.css` is a ZW009 build error under zfb 3, even under `wind: false` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ "Directives"): the two `@import "tailwindcss/…"` lines, every `@source`, and the `@theme` block. The plain `@layer zd-preflight, zd-flow;` statement itself is allowed ("Ordinary `@layer` statements … remain valid"), but whether 6.0.0 keeps those layer names is the template's decision.
- `@import "@takazudo/zudo-doc/safelist.css"` must go too: that file is one `@source inline("…")` directive (2,693 tokens today, zudo-doc planning map `explore/css-wind.md` §3), so importing it is itself ZW009. 6.0.0 replaces it with a strict `wind.json` candidate manifest declared by `zudoDoc()` — "automatic through `zudoDoc()`", #4473 — so the host imports nothing for it. Whether `theme.css` / `content.css` / `page-loading.css` / `features.css` stay as host-side `@import`s is decided by 6.0.0's template; take the template file rather than editing the 5.x one.
- Do not hand-port `global.css`. `scripts/check-template-drift.sh:31-37` already downloads the create-zudo-doc release matching the **installed** zudo-doc version, so after the preset bump `pnpm check:template-drift` prints `[DIFF] src/styles/global.css`; copy the 6.0 template from `node_modules/.cache/create-zudo-doc-templates/<version>/templates/base/src/styles/global.css`, then re-apply only the host's authored overrides (listed under **This repo**). Move any `@theme` token overrides to authored `:root { … }` custom properties or `zudoDoc({ wind })` (#4473 "override tokens via `zudoDoc({ wind })` or `:root`").
- Reset: v3 ships no implicit preflight; `zudoDoc()` chooses `wind.reset` for the site (planning recommends `owned-v1` plus an authored preflight-delta in `@layer base`). The host's job is the step-3 checklist of https://zfb.takazudomodular.com/guides/migrating-to-v3/: compare `html` and `code`/`pre` font stacks, `sub`/`sup`, `::placeholder`, `::file-selector-button`, `[hidden]`, lists, headings, borders and form controls (search input, language switcher) on representative pages in a browser — "a pixel diff of a page without those elements cannot catch their differences". HtmlPreview iframes carry the package's own preflight (`html-preview-wrapper/preflight.ts`) and are not affected by the page reset.
- Cascade: zudo-wind emits utilities **after** authored global CSS by default, so an authored unlayered rule of specificity (0,1,0) that sets the same property as a package utility on the same element flips its winner compared with Tailwind v4 (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ "Utility placement and ties"). `wind.utilities.placement: "before-authored"` restores Tailwind's order and shipped in **zfb 3.2.0 (released 2026-10-04)** (https://zfb.takazudomodular.com/zudo-wind/configuration/ "Fields", `utilities`). `group-*`/`peer-*` drop from (0,2,0) to (0,1,0) under either placement ("Relation variant specificity"; Takazudo/zudo-front-builder#3386). Exposure for each host is only its authored rules in `global.css`, listed under **This repo**.
- Audit result shared by all six (`zfb wind audit --project-root <repo>` with zfb 3.1.0 and a bare `wind: { spec: 1 }` config, exit 0, `outcome: complete`, `spec: 1 revision 3`): `unrecognized classes: (none)`, `conflicts: (none)`, `dead classes: (none)`; 15 `auditInfo` entries from the three route stubs — import specifiers like `@takazudo/zudo-doc/route-context` (ZW005 "invalid slash modifier"), `virtual:zudo-doc-route-context` (ZW002 "unknown or unconfigured variant"), the template literals `` `docs;${locale}` `` / `` `locale-docs;${locale}` `` (ZW012 dynamic construction), and the `zudolab/zudo-doc#2651` comment in `pages/index.tsx` (ZW005). All are lower-confidence string-literal origins; they never fail a build (https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/ "Four candidate outcomes"). Re-run with the zfb 3.2.0 CLI (same config, exit 0, `outcome: complete`, `spec: 1 revision 4`): still 0 unrecognized / 0 dead / 0 conflicts, and only **2** `auditInfo` entries remain — the two ZW012 `routeSig` template literals, now reported as `file:line:col` — because 3.2.0's extractor skips module specifiers (https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/). The audit scans only the host's own roots, so package-owned candidates are invisible until 6.0.0's manifest exists; `zfb check` could not run on the bare clone (preset import needs `node_modules`).
- On 3.1.0 `zfb wind audit` accepts only `--project-root` and `--fail-on error|warning` (`zfb wind audit --help`). `--json`, `--severity`, `--plan build`, `file:line:col` origins, `zfb wind manifest`, `wind.strict`/ZW014 and `wind.sources` shipped in **zfb 3.2.0 (released 2026-10-04)** (https://zfb.takazudomodular.com/api/cli/ "zfb wind", https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/ "Migration warnings and strict mode").

**This repo.**

- `src/styles/global.css` is byte-identical to zudo-css-wisdom's (the 5.28.1 template). ZW009 sources: line 8, line 9, lines 20–22 (`@source` ×3), line 26 (`@theme {}`, empty — delete). No authored rules → no placement/tie-flip exposure.
- `themePack: "observatory"` (`zfb.config.ts:8`): theme packs are package-owned CSS copied into the output by zudo-doc's `theme-packs` plugin; the planning map requires every `pack.css` to be wind-clean, so after the flip verify the pack still paints (colors, display/sans/mono faces) rather than assuming it.
- `dynamicPageTransition: true` (`zfb.config.ts:57`) activates `page-loading.css` (overlay, spinner, pending-link bootstrap) — one of the imports the 6.0 template decides about; verify page transitions after the flip.
- The Noto Sans JP webfont is loaded via `head.preconnect` + `head.stylesheets` (`zfb.config.ts:35-46`), not via CSS `@import`; the comment at lines 31–34 explaining the Tailwind bundling reason becomes historical — update the comment, keep the mechanism.
- Audit: 15 diagnostics total on 3.1.0, all from the three route stubs (none in `src/content/**`); the stubs' byte offsets differ from the siblings because this repo's `pages/docs/[[...slug]].tsx` carries an older comment block (`import type { JSX }` at line 29). On 3.2.0: 2 (ZW012 at `pages/[locale]/docs/[[...slug]].tsx:66:18` and `pages/docs/[[...slug]].tsx:55:16`).

### 3. Components and islands

**Shared.**

- The two dynamic route stubs are host-owned copies of package routes. Each must lose lines 1–2 (`/** @jsxRuntime automatic */`, `/** @jsxImportSource preact */`) and the `import type { JSX } from "preact"` line, and re-type `JSX.Element` with whatever 6.0.0's template exports. A leftover per-file pragma overrides `tsconfig.json`: with `preact` uninstalled the build fails with `Could not resolve "preact/jsx-runtime"`; `zfb build`/`zfb dev` first print a `zfb warn:` line with the file, line and column of each pragma (https://zfb.takazudomodular.com/guides/migrating-to-v3/ "Components and islands"). Audit the removal with `grep -rln "@jsxImportSource" pages src` (expected: 0).
- Take the 6.0.0 template versions of all three `pages/` files (`pnpm check:template-drift` will flag them) and re-apply the DocHistory patch the way 6.0's `create-zudo-doc` emits it (`docHistory: true` stays enabled); the imports `createRouteContext`, `createChrome`, `DocHistory`, `defineChromeBindings` and the two `virtual:zudo-doc-*` modules are package API whose 6.0 shapes #4473 covers ("chrome bindings and ejected components in zudo-react (`class`, `on:click`, signals)", "Child/Description/Component/JSX type migration").
- `pages/index.tsx` stays a one-line re-export; nothing to port.
- Islands, hydration, form models: none are host-authored; the package owns search, theme toggle, language switcher, sidebar, image enlarge, doc history. Verify them in a browser after the flip rather than porting anything.

**This repo.** Only the three template stubs; `pages/docs/[[...slug]].tsx` differs from the current template by 14 comment lines (an earlier explanation of the dev-mode 404 gap) and the locale stub by 4 — the 6.0 re-adoption replaces both anyway. MDX uses only package components (`<CategoryNav` ×9, `<Tabs` ×1, `<CodeGroup` ×1); nothing to port.

### 4. md-wasm and other packages

**Shared.**

- `@takazudo/zfb-md-wasm 2.22.0` is pinned only because zudo-doc peer-requires it and `scripts/check-pin-parity.mjs:37-47` keeps it in the exact-lockstep group. The v3 API delta is the removal of the `jsxRuntime` option on `compile()`/`renderHtml()` (https://zfb.takazudomodular.com/api/md-wasm/; https://zfb.takazudomodular.com/guides/migrating-to-v3/ "MDX and md-wasm"); `grep -rn jsxRuntime` over host code returns only the `/** @jsxRuntime automatic */` pragmas, which are unrelated. Bump in lockstep with the family.
- `@takazudo/zudo-doc-history-server` (`doc-history-generate` in CI, `doc-history-server` in the `dev` scripts of the template hosts) is a Node CLI with no zfb engine dependency; bump to the 6.0.x lockstep version only because the pin-parity gate groups it with zudo-doc.

**This repo.** Nothing beyond the shared bump; no script imports `@takazudo/zfb-md-wasm`.

### 5. Tests, CI, deploy

**Shared.**

- Workflows carry no Tailwind environment and no `tailwindcss` install step, so https://zfb.takazudomodular.com/guides/migrating-to-v3/ "CLI and environment" has nothing to remove here.
- `pr-checks.yml` `typecheck` → `pnpm check` (`zfb check`) and `build-site` → `pnpm build` are the gates that will fail first on 6.0.0 if a Preact pragma, a `react` path alias or a Tailwind directive survives; the failure texts are the ones quoted in the migration guide (`Could not resolve "preact/jsx-runtime"`, `ZW009 …`).
- `.assetsignore`: both workflows `printf '%s\n' '_worker.js' '_zfb_inner.mjs' > dist/.assetsignore` before deploy (`main-deploy.yml` "Prepare deploy directory", `pr-checks.yml` "Prepare dist for deployment"). The v3 adapter emits `dist/.assetsignore` itself, listing `_worker.js`, `_zfb_inner.mjs` and every emitted Wasm basename (https://zfb.takazudomodular.com/guides/ssr-and-cloudflare-bindings/). The `>` overwrite would drop the Wasm lines; after the first 6.0.0 build inspect `dist/.assetsignore` and either delete the step or change it to append (`>>`) only the names that are missing.
- Add the audit as a gate once on v3: a step `pnpm exec zfb wind audit --fail-on error` after `pnpm build` (migration checklist step 6). `--fail-on` exists on 3.1.0 (v3.1.0 changelog "Add failure thresholds to `zfb wind audit`"); `--json`/`--severity` for machine-readable output shipped in zfb 3.2.0 (released 2026-10-04).
- `check:template-drift` auto-follows the installed zudo-doc version, so on 6.0.0 it compares against create-zudo-doc 6.0.x; every entry in `.template-drift-allowlist` must be re-justified against the new template (the comments in that file reference the 5.22.0/5.27.0 baselines).
- `check:pin-parity` and `check:wrangler-pin` keep working unchanged (see §1). `check:html` (`html-validate dist/**/*.html`) stays the renderer-vocabulary gate (the `meta property` / `link as` / `svg xmlns` fixes of Takazudo/zudo-front-builder#3359 shipped in zfb 3.2.0, released 2026-10-04; 6.0.0 will be built against them).
- `scripts/run-b4push.sh` mirrors the same steps locally; keep it in sync when adding the audit step.

**This repo.**

- `pr-checks.yml` has fewer jobs than the siblings (template-drift, pin-parity, format, typecheck, build-site, html-validate, build-history, preview — no `check-category-meta`, no `check-nav-labels`); `typecheck` runs `check:wrangler-pin` then `pnpm check` (no `test:links` step in this repo's CI; `check:links` runs inside `build-site` at `:201`). The `.assetsignore` overwrite is at `main-deploy.yml:207` and `pr-checks.yml:343`.
- `.claude/skills/zudo-doc-design-system/SKILL.md`, `zudo-doc-translate/SKILL.md`, `zudo-doc-version-bump/SKILL.md` are the `claudeSkills` feature's scaffolded files (listed in `ZUDO_DEPS_PINS.md` and drift-checked: the feature loop at `scripts/check-template-drift.sh:137` is `for feature in i18n claudeSkills`). All 9 Tailwind/Preact mentions sit in `zudo-doc-design-system/SKILL.md` (the other two have none); after the preset bump `pnpm check:template-drift` lists them as `[DIFF]` — copy them from the cached 6.0 `templates/features/claudeSkills/files/`.
- `CLAUDE.md:3` and `:7-10` describe the stack as "Tailwind CSS v4 … Preact islands … compiled by zfb's embedded Tailwind engine" — rewrite after the flip (zudo-wind, zudo-react).

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

**This repo — prep items for step 1:** none required. **Flip-time additions:** after step 4, re-copy the three `.claude/skills/zudo-doc-*/SKILL.md` files from the 6.0 `claudeSkills` feature template (§5); after step 7, rewrite `CLAUDE.md:3,7-10`.

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

- [ ] The `observatory` theme pack paints (header/surface colors, display and mono faces) on `/` and a doc page in both color modes.
- [ ] Page transition overlay/spinner (`dynamicPageTransition: true`) appears and clears when navigating between docs; the TOC toggle (`tocToggle: true`) works.
- [ ] Noto Sans JP renders on `/ja/docs/getting-started/` (computed `font-family` on `body`).

## Risks and open questions

**Shared.**

- zudo-doc 6.0.0 has no release date; its `base/zfb3-migration` branch is pushed (`70e0875`, root PR zudolab/zudo-doc#4477 at 102 commits, still draft) with only the #4467 integration topic local, and the zfb bugs that blocked that integration shipped in zfb 3.2.0 on 2026-10-04 — so the release is unblocked but unpublished (see `zudolab--zudo-doc.md`). Any host work beyond `[prep-now]` before 6.0.0 ships will be redone.
- 6.0.0 must pin zfb 3.2.0 or later (the first release with the #3569/#3570 fixes); a `^3.1.0` floor would still admit 3.1.0. Installing 3.1.0 here would reproduce those bugs in highlighted code and non-ASCII content (all six sites have Japanese content).
- Utility placement flip and `group-*`/`peer-*` specificity drop (#3386) are package-owned concerns, but any authored host rule in `global.css` of specificity (0,1,0) is exposed; `placement: "before-authored"` is available since zfb 3.2.0 (released 2026-10-04).
- The zudo-react client runtime makes no bundle-size promise (https://zfb.takazudomodular.com/guides/migrating-to-v3/ "Components and islands", the paragraph under "Optional island props": "no promise to match Preact's bundle size … no fixed byte budget"; Takazudo/zudo-front-builder#3383 measured a trivial island at 48 KB / 16.6 KB gz vs 18 KB / 7.4 KB gz on 2.15.0, about 2.2×). The sites ship search/theme/sidebar islands from the package; expect a larger JS payload and re-check Lighthouse if it matters.
- Open: which `@takazudo/zudo-doc/*.css` imports the 6.0 template keeps (#4473 round 2 mentions "public CSS exports" while also voiding "physical CSS imports" as the migration path); whether `zd-flow` survives; the exact zfb version 6.0.0 pins (#4467/#4475). Resolve by reading the 6.0 template, not by guessing.
- Open: `.template-drift-allowlist` entries written against the 5.22–5.28 templates need re-justification; a stale entry hides real drift.

**This repo.** The only host-specific unknown is pack CSS under zudo-wind (package-owned, verify visually); the scaffolded skill texts are documentation, not a build risk.

## References

- zfb: https://zfb.takazudomodular.com/guides/migrating-to-v3/ · https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ · https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/ · https://zfb.takazudomodular.com/zudo-wind/configuration/ · https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/ · https://zfb.takazudomodular.com/zudo-wind/cascade-and-reset/ · https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/ · https://zfb.takazudomodular.com/zudo-react/components-and-jsx/ · https://zfb.takazudomodular.com/concepts/islands/ · https://zfb.takazudomodular.com/api/cli/ · https://zfb.takazudomodular.com/api/define-config/ · https://zfb.takazudomodular.com/api/md-wasm/ · https://zfb.takazudomodular.com/guides/ssr-and-cloudflare-bindings/ · https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ · https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/ · https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/
- zfb issues: Takazudo/zudo-front-builder#3569, #3570 (shipped in zfb 3.2.0, 2026-10-04), #3386 (placement/specificity), #3383 (bundle size), #3359 (renderer vocabulary).
- zudo-doc: epic zudolab/zudo-doc#4430 · root PR zudolab/zudo-doc#4477 · consumer migration guide zudolab/zudo-doc#4473 · integration floor zudolab/zudo-doc#4467 · planning resources `_temp-resource/4430-zfb3-migration/` on branch `base/zfb3-migration` (pushed at `70e0875`; `explore/css-wind.md`, `explore/pkg-build.md`, `upstream-issues.md`).
- Sibling guides in this directory: `Takazudo--zudo-css-wisdom.md`, `Takazudo--zudo-slack-wisdom.md`, `Takazudo--zudo-codemirror-wisdom.md`, `Takazudo--zudo-cloudflare-wisdom.md`, `Takazudo--zudo-tauri-wisdom.md`, `Takazudo--zudo-test-wisdom.md`; `zudolab--zudo-doc.md`.
- Commands used for the counts (run from each clone): `git status --short`; `grep -rnE '@import\s+"tailwindcss|@tailwind|@theme|@apply|@source|@utility|@variant|@custom-variant|@plugin|@config|theme\(' --include=*.css --include=*.ts --include=*.tsx --include=*.mjs .`; `grep -rnE "from ['\"]preact|preact/hooks|@jsxImportSource" --include=*.ts --include=*.tsx pages src scripts`; `grep -rnE 'useState|useEffect|useRef|useMemo|useCallback|useContext|createPortal|forwardRef|dangerouslySetInnerHTML|className=|onClick=|onInput=' pages src/components`; the audit recipe: back up `zfb.config.ts`, write `export default defineConfig({ wind: { spec: 1 } })`, `zfb wind audit --project-root .` with zfb 3.1.0 (re-run with 3.2.0), restore the config.
- Repo-specific: Takazudo/zudo-slack-wisdom#79 (zdtp removal, 2026-09-28); `ZUDO_DEPS_PINS.md` file list (includes the three `.claude/skills/zudo-doc-*` files); `scripts/check-template-drift.sh:137` (features loop = `i18n claudeSkills`).
