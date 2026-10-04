# zfb v3 migration guide: zudolab/zudo-panel-designer

Generated 2026-10-04 by an automated diagnosis of `main` @ `85f6ef0` (merge of #239, 2026-07-27). Counts come from the commands listed; re-run them locally before relying on them. The clone is shallow (depth 1), so `doc/` history was read through the GitHub REST API (`gh api repos/zudolab/zudo-panel-designer/commits?path=doc`), not `git log`. The `zfb wind audit` census below was taken with the zfb 3.1.0 CLI and re-run with 3.2.0; zfb 3.2.0 shipped on npm `latest` at 15:30 UTC on 2026-10-04 while this guide was being written, so every item formerly labelled "next zfb release" now reads "zfb 3.2.0".

## Verdict

**Blocked, effort S.** The only zfb consumer is `doc/`, a `create-zudo-doc` 4.x scaffold (commit `c20fce6` "scaffold zudo-doc v4 documentation sub-project", 2026-07-17; 42 commits touched `doc/`, last `4b19ce1` 2026-07-25) pinning `@takazudo/zfb 0.1.0-next.89` (86th of the 141 `@takazudo/zfb` versions on npm; 55 behind 3.2.0) and `@takazudo/zudo-doc ^4.1.0` (locked 4.1.0; 57th of 129 published versions, 72 behind 5.28.2). Every host file except `zfb.config.ts` and the MDX content is byte-identical to today's `create-zudo-doc` 5.28.2 templates (`global.css`, `tsconfig.json`, `pages/index.tsx`) or differs only in comment text (both route stubs). There are **zero** host components, islands, authored CSS rules or utility classes (`zfb wind audit`: 0 unrecognized, 0 dead, 0 conflicts). The app itself, `packages/app`, is a Vite 7 + React 19 SPA that uses Tailwind 4 through `@tailwindcss/vite` and never goes through zfb — **unaffected by zfb 3**. The doc migration is a **re-scaffold** with `create-zudo-doc` 6 plus a 1:1 port of the 14 `zudoDoc()` fields and a copy of 72 MDX files (EN + JA); the real work is link/anchor verification across the two locales (266 relative `.mdx` links, 121 with anchors). What gates the start: zudo-doc 6.0.0 (zudolab/zudo-doc#4430, root PR #4477), whose zfb blockers (Takazudo/zudo-front-builder#3569/#3570) shipped in zfb 3.2.0 on 2026-10-04 — zfb no longer gates it, but 6.0.0 itself is unpublished (`@takazudo/zudo-doc` `latest` is 5.28.2).

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `doc/` (standalone pnpm root; `doc/pnpm-lock.yaml`; not in root `pnpm-workspace.yaml`) | `@takazudo/zfb`, `-runtime`, `-adapter-cloudflare`, `-md-wasm` **0.1.0-next.89** exact (`doc/package.json:16-19`); `adapter: "@takazudo/zfb-adapter-cloudflare"` set in `zfb.config.ts:11` → `dist/_worker.js` SSR worker | `@takazudo/zudo-doc ^4.1.0` → lock 4.1.0 (`:20`); `@takazudo/zdtp 0.4.9` (`:32`, unused: no `designTokenPanel`) | `tailwindcss ^4.2.0` + `@tailwindcss/vite ^4.2.0` (`:36-37`, lock 4.3.2); `src/styles/global.css` l.8–9 imports, l.20–22 `@source` ×3, l.26 `@theme {}`, l.14 `safelist.css` | `preact ^10.29.1` (lock 10.29.7), `preact-render-to-string ^6.6.6`, `@types/react ^19.2.0`; `tsconfig.json:8-10` react→`preact/compat` paths; pragma `:2` + `import type { JSX } from "preact"` in `pages/docs/[[...slug]].tsx:29` and `pages/[locale]/docs/[[...slug]].tsx:27` | none host-side | dependency only (zudo-doc peer); no host call | Cloudflare Worker `doc-zudo-panel-designer`: `doc/wrangler.toml` (`main = "./dist/_worker.js"`, `nodejs_compat`, `[assets] binding = "ASSETS"`, custom domain `doc-zudo-panel-designer.takazudomodular.com`, `[env.preview]`/`[env.production]`); `.github/workflows/production-deploy.yml` job `deploy-doc` (node 22, `pnpm install --frozen-lockfile`, `pnpm build`, writes `dist/.assetsignore`, `wrangler deploy --env production`, smoke `/`, `/docs/overview/`, `/ja/docs/overview/`); `pr-checks.yml` job `preview` builds doc + `wrangler versions upload --env preview --preview-alias pr-N` |
| `packages/app` (`@zpd/app`, Vite + React 19 + `@tailwindcss/vite ^4.3.2`, `src/index.css:1 @import 'tailwindcss'`) | — | — | yes, via Vite only | React 19 (not Preact) | — | — | root `wrangler.toml` assets-only SPA → `zudo-panel-designer.takazudomodular.com`; **unaffected by zfb 3** |
| `packages/core`, `packages/patterns` | — | — | — | — | — | — | libraries; **unaffected** |

Audit-free facts: no `tailwind.config.*`, no `ZFB_TAILWIND*` in tracked files, no `"use client"`, no `<Island`, no `className=`, no hooks in `doc/` code (the only hook/`dangerouslySetInnerHTML` hits are prose in MDX describing the React app). `doc/src/` holds only `content/docs` (36 MDX), `content/docs-ja` (36 MDX) and `styles/global.css`; no `src/components/`, no `public/`, no `scripts/`. `doc/pnpm-workspace.yaml:14-25` carries an 11-entry `minimumReleaseAgeExclude` list of exact `@takazudo/*@0.1.0-next.89` / `zudo-doc@4.0.0|4.1.0` versions.

Content census: 72 MDX files, 5,567 lines, EN/JA mirrored 36/36. Package MDX globals used (no imports): `<CategoryNav>` ×12, `<Note>` ×8, `<Info>` ×4, `<Warning>` ×4, `<Tip>` ×2 (JSX admonitions — still registered in zudo-doc 5.28.2 `packages/zudo-doc/src/mdx-components/index.ts:457-460`), directives `:::note` ×20, `:::tip` ×6, `:::warning` ×4. Links: 58 in-page `](#…)`, 266 relative `.mdx` links of which **121 carry anchors**; JA pages use Japanese anchors (`#ダウンロードトリガー`). Frontmatter keys: `title`, `description`, `sidebar_position` only. No inline HTML, no `class=`/`style=` in MDX. The repo's own `.claude/skills/l-lessons-doc-site-authoring/SKILL.md` records the authoring traps (build is the gate, `broken link:` grep, hierarchical slugs, EN/JA anchor behaviour differs) — reuse it as the acceptance method for the migration PR.

### `zfb wind audit` (zfb 3.1.0, re-run with 3.2.0; `wind: { spec: 1 }` temporary config, `--project-root doc`)

```
outcome: complete / spec: 1 revision 3
unrecognized classes: (none)   conflicts: (none)   dead classes: (none)
dynamic constructions: 2  — `docs;${locale}` (pages/docs/[[...slug]].tsx@2527), `locale-docs;${locale}` (pages/[locale]/docs/[[...slug]].tsx@2714)
diagnostics: 11, all auditInfo, none strict:
  ZW002 ×4  "virtual:zudo-doc-route-context" / "virtual:zudo-doc-chrome-bindings" import specifiers (colon read as variant)
  ZW005 ×5  "@takazudo/zudo-doc/route-context" / ".../chrome" / ".../routes/index" (slash read as modifier)
  ZW012 ×2  the two routeSig template literals
```

Re-run with zfb 3.2.0 (same temporary config, exit 0):

```
outcome: complete / spec: 1 revision 4
unrecognized classes: (none)   conflicts: (none)   dead classes: (none)
dynamic constructions: 2  — `docs;${locale}` (pages/docs/[[...slug]].tsx:50:16), `locale-docs;${locale}` (pages/[locale]/docs/[[...slug]].tsx:61:18)
diagnostics: 2, both ZW012 auditInfo (the two routeSig template literals); the 9 ZW002/ZW005 import-specifier findings are gone — 3.2.0 skips module specifiers during extraction and prints `file:line:col`
```

Reading: no host utility candidates exist, so there are **no tokens to declare**; every finding is a string-literal import specifier at a lower-confidence origin, which never fails a build (https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/). The default audit scans the standalone plan only (`pages components layouts content src`), never `node_modules/@takazudo/zudo-doc` (https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/). 3.1.0 prints byte offsets; `file:line:col`, `--json`, `--severity` and `--plan build` are absent from 3.1.0's `--help` and shipped in **zfb 3.2.0 (released 2026-10-04)**. `zfb check` was not run (preset import needs `node_modules`).

## Sequencing and blockers

1. **zudo-doc 6.0.0 is the gate.** All zfb usage runs through the `zudoDoc()` preset; a preset that still emits `framework`/`tailwind` fails v3 config loading even after the host removes its own keys (https://zfb.takazudomodular.com/guides/migrating-to-v3/#before-you-start). 6.0.0 is in flight (epic zudolab/zudo-doc#4430, draft PR #4477 — `base/zfb3-migration` pushed at `70e0875`, 102 commits; only the #4467 integration topic is still local) and no longer blocked on zfb: #3569 and #3570 shipped in zfb 3.2.0 on 2026-10-04 (https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/). The remaining gate is zudo-doc's own release. See `zudolab--zudo-doc.md` §"What downstream zudo-doc hosts should expect from 6.0.0".
2. **create-zudo-doc 6 is the tool.** 5.x generator releases are frozen (DD8, `check-scaffold-pin-freshness` red on zudo-doc `main`).
3. **Nothing in this repo blocks the host.** No custom components/CSS/islands, no md-wasm calls, no `ZFB_TAILWIND` env; node 22 + `pnpm@11.5.2` already satisfy zfb 3 (`node >=22`, `pnpm >=10`).
4. **i18n is the only feature that needs care.** `locales: { ja: { label: "JA", dir: "src/content/docs-ja" } }` and the `[locale]` route stub are both current in 5.28.2 (`guides/i18n.mdx`, `templates/features/i18n/`); the risk is anchor/link parity, not the engine.
5. **Do now (independent of upstream):** nothing is required. Optional step 0: bump `doc/` to zudo-doc 5.28.2 + zfb 2.22.1. Breaking notes between 4.1.0 and 5.28.2 that could touch this host: 5.0.0 moves the zfb peer to `^2.1.0` (lockstep bump), removes `githubAutolinks` (not used), flips `tsconfig.base.json` to `react-jsx`/`jsxImportSource: preact` and drops `@types/react` from the scaffold (harmless). Hierarchical heading IDs (4.0.0) are already in effect on 4.1.0, so the 121 anchored links are not newly at risk from the 5.x jump. Value of step 0: 5.x fixes meanwhile; cost: lockfile churn + a deploy. Skip if 6.0.0 is near.

## Required changes

### 1. Dependencies, config, tsconfig, env

- **`doc/package.json`** (`:16-19`) — move all four `0.1.0-next.89` pins to the zfb release zudo-doc 6.0.0 declares as peer (**3.2.0 or later, not 3.1.0** — 3.2.0, released 2026-10-04, is the first release carrying #3569/#3570; 3.2.0 is a minor, so a `^3.1.0` floor would still admit the 3.1.0 build panic). Keep `@takazudo/zfb-adapter-cloudflare` (the site is SSR-on-a-Worker by config; adapter 3.x exists, lockstep). `:20` `@takazudo/zudo-doc` → `^6.0.0`. Delete `:22-23` `preact`, `preact-render-to-string` and `:40` `@types/react` (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Delete `:36-37` `@tailwindcss/vite`, `tailwindcss`. Delete `:32` `@takazudo/zdtp 0.4.9` unless `designTokenPanel` is enabled (then `preact` stays as zdtp's peer — zdtp stays a Preact widget, zudo-doc decision DD3). `wrangler ^4.110.0` stays.
- **`doc/pnpm-workspace.yaml`** — delete the 11 stale `minimumReleaseAgeExclude` entries (`:14-25`; they name `0.1.0-next.89` and `zudo-doc@4.0.0/4.1.0`); keep `minimumReleaseAge: 0` and `allowBuilds` (esbuild, sharp, workerd).
- **`doc/zfb.config.ts`** — keep `defineConfig(zudoDoc({...}))`; it has no `framework`/`tailwind` key (both are hard errors in v3, https://zfb.takazudomodular.com/api/define-config/). All 14 fields (`port`, `adapter`, `siteName`, `siteUrl`, `locales`, `githubUrl`, `llmsTxt`, `sidebarResizer`, `sidebarToggle`, `imageEnlarge`, `dynamicPageTransition`, `footer{links,copyright}`, `headerNav` ×6, `headerRightItems` ×4 incl. `language-switcher`) exist in zudo-doc 5.28.2 (`packages/zudo-doc/src/config.ts`, `settings.ts:49`); re-verify against the 6.0 consumer guide (zudolab/zudo-doc#4473). `port: 15210` keeps the repo's 152xx convention.
- **`doc/tsconfig.json`** — keep `extends: "@takazudo/zudo-doc/tsconfig.base.json"`; delete the three `paths` aliases `react`, `react/jsx-runtime`, `react-dom` (`:8-10`); keep `"@/*"`.
- **Env/CI** — no `ZFB_TAILWIND_BIN`/`ZFB_TAILWIND_OXIDE_WARMUP` anywhere; nothing to remove (https://zfb.takazudomodular.com/guides/migrating-to-v3/#cli-and-environment). Root `eslint-plugin-react-hooks`/`react-refresh` belong to `packages/app` and stay.

### 2. CSS and utilities

`doc/src/styles/global.css` (28 lines, identical to the 5.28.2 template):

| Line | Today | v3 |
| --- | --- | --- |
| 7 | `@layer zd-preflight, zd-flow;` | follow the 6.0 template (layer prelude is wind-owned; https://zfb.takazudomodular.com/zudo-wind/cascade-and-reset/) |
| 8 | `@import "tailwindcss/preflight" layer(zd-preflight);` | **delete** (ZW009); the preset selects `wind.reset` (`owned-v1` + authored preflight patch per the zudo-doc plan) |
| 9 | `@import "tailwindcss/utilities";` | **delete** (ZW009) |
| 13, 15–17 | `@import "@takazudo/zudo-doc/{theme,content,page-loading,features}.css"` | keep (package `exports` subpaths resolve since 3.1.0) |
| 14 | `@import "@takazudo/zudo-doc/safelist.css";` | **delete** — export removed in 6.0; `zudoDoc()` wires the package `wind.json` manifest |
| 20–22 | `@source` ×3 | **delete** (ZW009) |
| 26 | `@theme { }` | **delete** (ZW009); overrides go to `zudoDoc({ wind: { tokens } })` or `:root { --… }` |

No host utilities exist, so no `wind.tokens`/`authoredClasses`/`safelist`/ZW014 work. If authored CSS is ever added, remember utilities now follow authored CSS by default (equal-specificity ties flip); `wind.utilities.placement: "before-authored"` restores Tailwind's order and shipped in **zfb 3.2.0 (released 2026-10-04)** (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties).

### 3. Components and islands

- `doc/pages/docs/[[...slug]].tsx:2` and `doc/pages/[locale]/docs/[[...slug]].tsx:2`: delete `/** @jsxImportSource preact */` (a per-file pragma overrides `tsconfig.json`; without Preact installed the build fails with `Could not resolve "preact/jsx-runtime"`, after a `zfb warn:` naming file:line:col — https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Replace `import type { JSX } from "preact"` (`:29` / `:27`) with the type the 6.0 stub uses (#4473 lists `Child`/`Description`/`Component`/`JSX` replacements). Both stubs already match the 5.28.2 template body (incl. the `virtual:zudo-doc-chrome-bindings` import), so taking the 6.0 template files wholesale is the cheapest correct edit.
- `doc/pages/index.tsx`: unchanged.
- No host components, hooks, forms or `rawHtml` sites exist; the hooks map (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/) does not apply here. The React hooks mentioned in `doc/src/content/**/editor/*.mdx` are prose about `packages/app` and compile as text.
- MDX JSX admonitions (`<Note>`, `<Info>`, `<Tip>`, `<Warning>`) are rendered by package components; after 6.0 they compile against the zudo-react JSX runtime via md-wasm with no content change expected (https://zfb.takazudomodular.com/guides/migrating-to-v3/#mdx-and-md-wasm). Verify one EN and one JA page containing each.

### 4. md-wasm and other packages

- `@takazudo/zfb-md-wasm` is listed (`:19`) only as zudo-doc's peer; no host file calls `compile()`/`renderHtml()`/`jsxRuntime` (`grep -rnE 'jsxRuntime|compile\(|renderHtml\(' doc` → only the `/** @jsxRuntime automatic */` pragmas). Bump in lockstep; nothing else.
- `packages/app`: Tailwind 4 via `@tailwindcss/vite` and React 19 via `@vitejs/plugin-react` — **not a zfb project**; keep as is (the briefing's rule: Tailwind in a standalone Vite sub-package is unaffected by zfb 3).
- `@takazudo/zdtp 0.4.9`: dead dependency; drop (see §1).

### 5. Tests, CI, deploy

- `.github/workflows/pr-checks.yml` job `preview` (l.107–240) already installs and builds `doc/` on every PR and uploads a preview — this is the migration PR's gate. Add `cd doc && pnpm check` and `npx zfb wind audit --fail-on error` steps after `Build doc` (https://zfb.takazudomodular.com/api/cli/#zfb-wind). Keep the `broken link:` grep from the lessons skill as a step, since the build exits 0 on broken links.
- `.github/workflows/production-deploy.yml` job `deploy-doc` (l.83–154): unchanged. The `Write dist/.assetsignore` step (`_worker.js`, `_zfb_inner.mjs`) is redundant — the Cloudflare adapter has emitted `.assetsignore` itself since zfb 0.1.0-next.74 and still does on 3.x (https://zfb.takazudomodular.com/guides/ssr-and-cloudflare-bindings/) — but harmless; keep or drop. Smoke URLs stay valid (`/`, `/docs/overview/`, `/ja/docs/overview/`).
- `doc/wrangler.toml`: unchanged (`main = "./dist/_worker.js"` + `nodejs_compat` remain the adapter contract; zfb 3.2.0 (released 2026-10-04) also removes the dangling `bundle-runtime.mjs.map` reference from the emitted Worker (#3480) and lowers the Linux binary's glibc requirement from 2.35 to 2.34 (#3584; releases up to 3.1.0 need 2.35, which `ubuntu-latest` already satisfies — only Amazon Linux 2023 / RHEL 9-class hosts were affected).
- `doc/.gitignore`: keep `.zfb*`, `.zudo-doc/`, `.wrangler/`; v3 still writes `.zfb-build/` and `.zfb/graph.bin` (https://zfb.takazudomodular.com/api/cli/#scratch-dirs).
- No dependabot config exists; consider adding one for `/doc` after 6.0.0 so zudo-doc patches arrive.

## Step-by-step plan

Adapted from the official 7-step checklist (https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist). Steps 1–2 are possible today; 3–9 wait for `@takazudo/zudo-doc@6.0.0` + `create-zudo-doc@6`.

1. **(Now) Harden the doc gate in `pr-checks.yml`:** add `pnpm check` and a `grep -i 'broken link'` step after `Build doc`; merge independently.
2. **(Now) Watch the gates:** `npm view @takazudo/zudo-doc dist-tags --json`, `npm view create-zudo-doc dist-tags --json`, `npm view @takazudo/zfb dist-tags --json`; zudolab/zudo-doc#4430 / #4477 / #4473; https://zfb.takazudomodular.com/changelog/zfb/.
3. **Re-scaffold into a sibling dir:** from the repo root `pnpm create zudo-doc doc-next --yes --pm pnpm --lang en --additional-langs ja --search` (i18n contract: https://zudo-doc.takazudomodular.com/docs/guides/i18n/; check `create-zudo-doc --help` for the 6.0 flag names covering sidebarResizer, sidebarToggle, llmsTxt, imageEnlarge, dynamicPageTransition, footer). Expect: zfb pins = 6.0.0's peer floor, no Tailwind, no Preact, `jsxImportSource: "@takazudo/zfb/zudo-react"`, both route stubs present.
4. **Port the config:** copy the 14 `zudoDoc()` fields from `doc/zfb.config.ts:5-80` (keep `port: 15210`, `adapter`, `locales.ja.dir: "src/content/docs-ja"`, the 6 `headerNav` entries, the 4 `headerRightItems` incl. `language-switcher`, the `footer.copyright`). `pnpm -C doc-next check`.
5. **Move content and config files:** `cp -R doc/src/content/docs doc/src/content/docs-ja doc-next/src/content/`; copy `doc/wrangler.toml`, `doc/.npmrc`, the `.gitignore` extras (`.zfb*`, `.zudo-doc/`), and `doc/CLAUDE.md` (then replace its "Tailwind CSS v4 / Preact" tech-stack lines with the 6.0 generator's text). Rewrite `doc/pnpm-workspace.yaml` without the stale exclude list.
6. **Build, then verify links the way the lessons skill says:** `cd doc-next && pnpm install && pnpm build 2>&1 | tee build.log; grep -i 'broken link' build.log; grep -c ZW0 build.log`. Then `grep -oE '"depth":[23],"slug":"[^"]*"' dist/**/index.html` and check the 121 anchored cross-links in both locales (EN emits `broken link:` for a wrong leaf slug, JA historically did not — check `href=#…` in built HTML for JA).
7. **Browser pass** on `/docs/overview/`, `/ja/docs/overview/`, `/docs/editor/add-actions-and-dialogs/` (tables, `<Note>`/`<Warning>`), `/docs/document-model/document-state-and-layers/` (long code, `<Warning title>`), `/ja/docs/export/round-trip/` (blockquote dialog mockups), the language switcher, search, theme toggle, sidebar resizer/toggle.
8. **Swap and gate:** `git mv doc doc-old && git mv doc-next doc`, delete `doc-old` in the same PR; `pr-checks.yml` preview builds it and publishes `pr-N` aliases for app + doc; run `npx zfb wind audit --fail-on error` in `doc/` (expect 0 strict findings).
9. **Deploy:** merge → `production-deploy.yml` `deploy-doc` → smoke 200 on the three URLs; confirm `wrangler deploy` picked up the adapter-emitted `.assetsignore` (no public `/_worker.js`).

In-place alternative to 3–8: bump pins, apply §1–§3 edits by hand, and manually fold in generator changes since 4.1.0 (`public/` favicon set from 5.0.0, `scripts/check-links.js` from 5.8.0). Equivalent effort for this host; the re-scaffold leaves less drift.

## Verification checklist

- [ ] `cd doc && pnpm build` exits 0 **and** `grep -i 'broken link'` of its output is empty (both locales).
- [ ] No `ZW009` in the build log; `npx zfb wind audit --fail-on error` passes; `npx zfb --version` prints 3.x + `embedded esbuild` only.
- [ ] `pnpm check` passes with `preact`/`@types/react` removed and the `paths` aliases gone; `grep -rn '@jsxImportSource' doc/pages doc/src` → 0.
- [ ] `dist/_worker.js`, `dist/_zfb_inner.mjs`, `dist/.assetsignore` present; `wrangler deploy --dry-run --env production` from `doc/` succeeds.
- [ ] The 58 in-page anchors and 121 anchored cross-links resolve in built HTML; JA Japanese anchors unchanged (other `docs-ja` pages link to them).
- [ ] Reset diff on representative pages per https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#differences-from-tailwind-preflight: headings, lists, tables, `code`/`pre` font stack, `sub`/`sup`, `::placeholder` in search, `[hidden]`.
- [ ] `<Note>`/`<Info>`/`<Tip>`/`<Warning>` and `:::note|tip|warning` render in EN and JA; `<CategoryNav>` cards link correctly on the 12 index pages.
- [ ] Package islands (search, theme toggle, language switcher, sidebar resizer/toggle, image enlarge, page transitions) hydrate without console errors; client-side navigation between `/docs/...` and `/ja/docs/...` works.
- [ ] `pr-checks.yml` preview and `production-deploy.yml` smoke green; `git status --short` clean; `doc/pnpm-lock.yaml` has no `preact`, `tailwindcss`, `@takazudo/zdtp`.

## Risks and open questions

- **Upstream timing is the whole risk.** Nothing can go green before `@takazudo/zudo-doc@6.0.0`; its zfb floor (3.2.0) is already released, so the only remaining gate is zudo-doc's own release.
- **Link/anchor regressions across 72 pages** are the realistic failure mode, not the engine. The repo's own lessons skill documents that EN and JA differ in how a wrong anchor surfaces; budget the verification, not the port.
- **Cascade flip and bundle growth** (utilities after authored CSS; ~2.2× island bundle per zudo-doc #3383) are package-level behaviour changes you inherit; no host CSS exists to break today.
- **`zfb check` not run** on the clone; config typing gaps surface only at step 4.
- **Open questions:** (a) will the 6.0 generator keep `language-switcher` as a `headerRightItems` component id and the `locales.<code>.dir` shape (both exist in 5.28.2; confirm in #4473)? (b) does `port` remain a `zudoDoc()` passthrough? (c) `pr-checks.yml` installs doc deps *after* the app deploy — keep that order so a doc failure cannot block the app preview.

## References

- Official guide and checklist: https://zfb.takazudomodular.com/guides/migrating-to-v3/
- Tailwind → zudo-wind map, reset table, placement/ties: https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/
- Preact hooks → zudo-react (not needed here): https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/
- Config shape, removed keys, `adapter`: https://zfb.takazudomodular.com/api/define-config/
- `zfb wind audit|explain|manifest`, scratch dirs: https://zfb.takazudomodular.com/api/cli/
- Diagnostics ZW001–ZW014: https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- Source plan: https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/
- Cloudflare adapter output (`_worker.js`, `_zfb_inner.mjs`, `.assetsignore`): https://zfb.takazudomodular.com/guides/ssr-and-cloudflare-bindings/
- zfb release notes: https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/
- zudo-doc i18n contract: https://zudo-doc.takazudomodular.com/docs/guides/i18n/
- zudo-doc 6.0.0 status: zudolab/zudo-doc#4430 (epic), #4477 (root PR), #4473 (consumer migration guide), and `zudolab--zudo-doc.md` in this directory
- zfb fixes zudo-doc waits on: Takazudo/zudo-front-builder#3569, #3570, and also #3480, #3584 — all shipped in zfb 3.2.0 (2026-10-04)
- Repo facts: `doc/package.json`, `doc/zfb.config.ts`, `doc/pnpm-workspace.yaml`, `doc/src/styles/global.css`, `doc/pages/**/[[...slug]].tsx`, `doc/wrangler.toml`, `.github/workflows/{pr-checks,production-deploy}.yml`, `README.md` §Deployment, `.claude/skills/l-lessons-doc-site-authoring/SKILL.md`, `packages/app/vite.config.ts`
- Commands used: `zfb wind audit --project-root doc` (zfb 3.1.0, re-run with 3.2.0; temp `wind:{spec:1}` config, restored), the briefing's Tailwind/Preact/hooks/md-wasm greps, `diff` against `zudo-doc/packages/create-zudo-doc/templates/{base,features/i18n}/**`, `gh api repos/zudolab/zudo-panel-designer/{commits?path=doc,issues,pulls}`

### Re-run this census

```sh
# from the repo root; ZFB = a zfb 3.x binary (npx -p @takazudo/zfb@latest zfb, or node_modules/.bin/zfb)
cp doc/zfb.config.ts /tmp/zfb.config.bak
printf 'import { defineConfig } from "zfb/config";\nexport default defineConfig({ wind: { spec: 1 } });\n' > doc/zfb.config.ts
$ZFB wind audit --project-root doc; cp /tmp/zfb.config.bak doc/zfb.config.ts; git status --short   # must be empty
grep -rnE '@import\s+"tailwindcss|@tailwind|@theme|@apply|@source|@utility|@variant|@custom-variant|@plugin|@config|theme\(' --include=*.css --include=*.ts --include=*.tsx doc --exclude-dir=node_modules
grep -rnE "from ['\"]preact|preact/hooks|@preact/signals|preact-render-to-string|@jsxImportSource" --include=*.ts --include=*.tsx --include=*.mdx doc --exclude-dir=node_modules
grep -rnE 'useState|useEffect|useRef|useMemo|useCallback|createContext|createPortal|forwardRef|dangerouslySetInnerHTML|className=|<Island|"use client"' --include=*.ts --include=*.tsx doc --exclude-dir=node_modules
grep -rhoE '<(CategoryNav|Note|Tip|Info|Warning|CodeGroup|Tabs|TabItem|MathBlock)\b' doc/src/content --include=*.mdx | sort | uniq -c
grep -rhoE '\]\([^)]*\.mdx?#[^)]*\)' doc/src/content --include=*.mdx | wc -l    # 121 anchored cross-links today
diff doc/src/styles/global.css <zudo-doc clone>/packages/create-zudo-doc/templates/base/src/styles/global.css   # IDENTICAL today
```
