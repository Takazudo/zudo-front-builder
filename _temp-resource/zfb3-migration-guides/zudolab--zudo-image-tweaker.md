# zfb v3 migration guide: zudolab/zudo-image-tweaker

Generated 2026-10-04 by an automated diagnosis of `main` @ `2db9ad6` (merge of #107, 2026-09-21 UTC); the audit used the zfb 3.1.0 CLI. zfb 3.2.0 shipped on 2026-10-04 15:30 UTC while this guide was being written, so the version labels and pins below were updated to it (https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/); the audit result (0 host-owned candidates) is not affected by 3.2.0 and was not re-run. Counts come from the commands listed; re-run them locally before relying on them. The clone used here is shallow (depth 1), so `doc/` history was read through the GitHub REST API (`gh api repos/zudolab/zudo-image-tweaker/commits?path=doc`), not `git log`.

## Verdict

**Blocked, effort S.** The only zfb consumer in this repo is `doc/`, a `create-zudo-doc@3.3.0` scaffold (commit `a846163`, 2026-07-17) that has not been touched since 2026-07-17 (13 commits ever, last `17a5eb5`). It pins `@takazudo/zfb 0.1.0-next.78` (75th of the 141 `@takazudo/zfb` versions on npm; 66 behind 3.2.0) and `@takazudo/zudo-doc ^3.3.0` (locked 3.3.0; 55th of 129 published versions, 74 behind 5.28.2). Every host-authored file except `zfb.config.ts` and the MDX content is byte-identical to today's `create-zudo-doc` 5.28.2 template (`global.css`, `tsconfig.json`, `pages/index.tsx`) or differs only in comments and one import (`pages/docs/[[...slug]].tsx`). There are **zero** custom components, islands, authored CSS rules, or utility classes on the host side (`zfb wind audit`: 0 unrecognized, 0 dead, 0 conflicts). The migration is therefore not a port but a **re-scaffold**: run `create-zudo-doc` 6 when it ships, copy `src/content/`, port the 12 `zudoDoc()` fields, keep `wrangler.toml`/CI. What gates the start: zudo-doc 6.0.0 (zudolab/zudo-doc#4430, root PR #4477) alone — its zfb blockers Takazudo/zudo-front-builder#3569/#3570 shipped in **zfb 3.2.0 (released 2026-10-04)**, so zudo-doc is unblocked on zfb (its `base/zfb3-migration` branch is pushed @ `70e0875`; the #4467 integration topic is still local) but 6.0.0 is not published. The published package `packages/zit` does not depend on zfb at all.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `doc/` (standalone pnpm root, `doc/pnpm-workspace.yaml` `packages: []`, own `doc/pnpm-lock.yaml`; **not** in the root workspace) | `@takazudo/zfb`, `-runtime`, `-adapter-cloudflare` **0.1.0-next.78** exact (`doc/package.json:23-25`; the adapter is installed but unused — no `adapter` key, pure static build) | `@takazudo/zudo-doc ^3.3.0` → lock 3.3.0 (`doc/package.json:26`); `@takazudo/zdtp 0.4.6` (`:38`, unused: `designTokenPanel` not set, no `zdtp/styles.css` import) | `tailwindcss ^4.2.0` + `@tailwindcss/vite ^4.2.0` (`:42-43`, lock 4.3.2); `src/styles/global.css` l.8–9 imports, l.20–22 `@source` ×3, l.26 `@theme {}`; l.14 `@takazudo/zudo-doc/safelist.css` | `preact ^10.29.1` (lock 10.29.7), `preact-render-to-string ^6.6.6`, `@types/react ^19.2.0`; `tsconfig.json:8-10` `react`/`react/jsx-runtime`/`react-dom` → `preact/compat`; `pages/docs/[[...slug]].tsx:2` pragma + `:25` `import type { JSX } from "preact"` | none host-side (all islands ship from the package) | not a dependency (`@takazudo/zfb-md-wasm` absent from `doc/package.json`) | Cloudflare Workers Static Assets, assets-only: `doc/wrangler.toml` (no `main`, `not_found_handling = "404-page"`, custom domain `zudo-image-tweaker.takazudomodular.com`); `.github/workflows/main-deploy.yml` (push to `main`, path-filtered `doc/**`, `CLAUDE.md`, `.claude/**`; node 22; `pnpm -C doc install --frozen-lockfile && pnpm -C doc build`; `npx wrangler@$WRANGLER_VERSION deploy` with the version read from `doc/package.json` (4.111.0 today); HTTPS smoke) |
| `packages/zit` (`@takazudo/zudo-image-tweaker`, published) | — | — | — | — | — | — | npm via `release.yml`; **unaffected** |
| `doc/examples-check/` | — (plain `tsc -p examples-check/tsconfig.json` against `../../packages/zit/dist/*.d.ts`) | — | — | — | — | — | run by `ci.yml` job `doc-examples` (l.132–170: root install, build `packages/zit`, `pnpm -C doc install --frozen-lockfile`, `pnpm -C doc check:examples`); **unaffected**, must survive the re-scaffold |

Audit-free facts: no `tailwind.config.*`, no `ZFB_TAILWIND*`/`OXIDE_WARMUP` in tracked files, no `"use client"`, no `<Island`, no `className=`/`onClick=`, no `dangerouslySetInnerHTML`, no hooks (`grep -rnE 'useState|useEffect|…' doc --exclude-dir=node_modules` → 0). `doc/src/` contains only `content/` and `styles/global.css`; there is no `src/components/`, no `public/`, no `scripts/check-links.js` (added to the template in create-zudo-doc 5.8.0; `public/` favicons in 5.0.0).

Content census (`find doc/src/content -name '*.mdx' | wc -l`): 27 MDX files, 2,523 lines; 21 hand-authored + 6 `generated: true` pages under `claude-md/`, `claude/`, `claude-skills/` written by the `claudeResources` integration and committed. MDX components used (package globals, no imports): `<CategoryNav>` ×5, `<CategoryTreeNav>` ×1, `<CodeGroup>`/`<Tabs>`/`<TabItem>`/`<MathBlock>`/`<SiteTreeNavDemo>` ×1 each; directives `:::warning` ×3, `:::danger` ×2, `:::info` ×1, `:::code` ×1. Links: 10 in-page `](#…)`, 24 relative `.mdx` links (1 with an anchor). Frontmatter keys: `title`, `description`, `sidebar_position`, `sidebar_label`, `category_no_page`, `generated`. No inline HTML, no `class=`/`style=` in MDX.

### `zfb wind audit` (zfb 3.1.0, `wind: { spec: 1 }` temporary config, `--project-root doc`)

```
outcome: complete / spec: 1 revision 3      # the 3.2.0 CLI prints `revision 4` (six new utility groups)
unrecognized classes: (none)   conflicts: (none)   dead classes: (none)
dynamic constructions: 1  — `docs;${locale}` at pages/docs/[[...slug]].tsx (byte 2186: the routeSig template literal)
diagnostics (all auditInfo, none strict):
  ZW002 pages/docs/[[...slug]].tsx@1527  "virtual:zudo-doc-route-context"   (import specifier read as variant:utility)
  ZW005 pages/docs/[[...slug]].tsx@1627  "@takazudo/zudo-doc/route-context" (slash read as modifier)
  ZW005 pages/docs/[[...slug]].tsx@1692  "@takazudo/zudo-doc/chrome"
  ZW012 pages/docs/[[...slug]].tsx@2186  dynamic construction
  ZW005 pages/index.tsx@408             "@takazudo/zudo-doc/routes/index"
```

Reading: there are no host utility candidates at all, so there are **no tokens to declare** on the host side; every finding is a string-literal import specifier at a lower-confidence origin, which never fails a build (https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/). The audit scans only the standalone plan (`pages components layouts content src`), not `node_modules/@takazudo/zudo-doc`, so the package's own utilities are invisible here by design (https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/). 3.1.0 prints byte offsets; `file:line:col` and `--json` shipped in zfb 3.2.0 (#3370, released 2026-10-04). `zfb check` was not run (preset import needs `node_modules`).

## Sequencing and blockers

1. **zudo-doc 6.0.0 is the gate.** The host's zfb usage is 100 % through the `zudoDoc()` preset, and a preset that still emits `framework`/`tailwind` fails v3 config loading even after the host removes its own keys (https://zfb.takazudomodular.com/guides/migrating-to-v3/#before-you-start). zudo-doc 5.28.2 is Preact + Tailwind; 6.0.0 is in flight (epic zudolab/zudo-doc#4430, draft PR #4477 — `base/zfb3-migration` is pushed @ `70e0875` with 102 commits; only the #4467 integration topic is still local) and its zfb blockers are cleared: #3569 Wind extractor UTF-8 offsets and #3570 quoted island markers shipped in zfb 3.2.0 (released 2026-10-04). What remains is zudo-doc's own release. See `zudolab--zudo-doc.md` §"What downstream zudo-doc hosts should expect from 6.0.0".
2. **create-zudo-doc 6 is the tool.** DD8 in the zudo-doc plan froze 5.x (`check-scaffold-pin-freshness` is red on `main`), so the next generator release is 6.0.0. Until then nothing on the zfb/zudo-doc side can be changed here.
3. **Nothing in this repo blocks the host.** No custom components, no authored CSS, no islands, no md-wasm calls, no `ZFB_TAILWIND` env, Node 22 already (zfb 3 needs `node >=22`, `pnpm >=10`; the repo pins `pnpm@11.5.2`).
4. **Do now (independent of upstream):** add `pnpm -C doc build` to `ci.yml` (today its `doc-examples` job already installs `doc/` but runs only `check:examples`, never `zfb build`, so a broken doc site is first noticed by `main-deploy.yml` after merge); decide whether the 6 committed `generated: true` pages stay committed or move to build output; note the `.claude/skills/l-make-release/SKILL.md:97` rule that releases never touch `doc/` (still true after migration).
5. **Optional step 0 (now):** bump `doc/` to zudo-doc 5.28.2 + zfb 2.22.1 first. Between 3.3.0 and 5.28.2 the only host-visible breaking notes are 4.0.0 "heading IDs are hierarchical-only" (this site has 10 in-page anchors and 1 anchored cross-link to re-check) and 5.0.0's `tsconfig.base.json` flip to `react-jsx` + dropping `@types/react` from the scaffold (harmless here). Value: splits content-level breakage from the engine change and gets 5.x fixes meanwhile; cost: one more lockfile round and deploy. Skip it if 6.0.0 is close.

## Required changes

### 1. Dependencies, config, tsconfig, env

- **`doc/package.json`** (`:23-25`) — replace the three `0.1.0-next.78` pins with the zfb release that zudo-doc 6.0.0 declares as peer — expected exact `3.2.0` (released 2026-10-04), peer floor `^3.2.0`; **not 3.1.0** and not `^3.1.0`, which would still admit the 3.1.0 build panic (#3569); drop `@takazudo/zfb-adapter-cloudflare` entirely (no `adapter` key, pure static build) or keep it only if you later opt into SSR. `:26` `@takazudo/zudo-doc` → `^6.0.0`. Delete `:28-29` `preact`, `preact-render-to-string` and `:46` `@types/react` (present only for zfb's old engine; https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Delete `:42-43` `@tailwindcss/vite`, `tailwindcss`. Delete `:38` `@takazudo/zdtp 0.4.6` unless `designTokenPanel` is enabled (then `preact` stays as zdtp's peer — zdtp remains a Preact widget by zudo-doc decision DD3). Keep `shiki`, `mermaid`, `katex`, `diff`, `minisearch`, `gray-matter`, `remark-*`, `zod`, `pagefind`, `typescript`, `wrangler` as the 6.0 scaffold dictates.
- **`doc/zfb.config.ts`** — keep `defineConfig(zudoDoc({...}))`; no `framework`/`tailwind` key exists here today (good: both are hard config errors in v3, https://zfb.takazudomodular.com/api/define-config/). All 12 fields used (`siteName`, `siteDescription`, `siteUrl`, `base`, `githubUrl`, `llmsTxt`, `imageEnlarge`, `dynamicPageTransition`, `claudeResources{claudeDir,projectRoot,scanRoot}`, `defaultLocaleOnlyPrefixes`, `headerNav`, `headerRightItems`) exist in zudo-doc 5.28.2 `packages/zudo-doc/src/config.ts`; re-verify against the 6.0 consumer guide (zudolab/zudo-doc#4473) when it exists. Token overrides, if ever wanted, move from the `@theme {}` slot to the planned `zudoDoc({ wind })` passthrough or authored `:root` custom properties.
- **`doc/tsconfig.json`** — keep `extends: "@takazudo/zudo-doc/tsconfig.base.json"` (6.0 ships `jsxImportSource: "@takazudo/zfb/zudo-react"` there); delete the three `paths` aliases `react`, `react/jsx-runtime`, `react-dom` (`:8-10`). Keep `"@/*"`.
- **`doc/pnpm-workspace.yaml`** — `allowBuilds` (esbuild, sharp, workerd) still applies; nothing Tailwind-specific. `doc/.npmrc` `trust-policy-exclude[]=undici-types@6.21.0` is unrelated.
- **Env/CI** — no `ZFB_TAILWIND_BIN`/`ZFB_TAILWIND_OXIDE_WARMUP` anywhere (`grep -rn ZFB_TAILWIND .` → 0); nothing to remove (https://zfb.takazudomodular.com/guides/migrating-to-v3/#cli-and-environment).

### 2. CSS and utilities

`doc/src/styles/global.css` (28 lines, identical to the 5.28.2 template) — delete the ZW009 sites and the removed export:

| Line | Today | v3 |
| --- | --- | --- |
| 7 | `@layer zd-preflight, zd-flow;` | keep only if the 6.0 template keeps it (layer prelude is wind-owned; see https://zfb.takazudomodular.com/zudo-wind/cascade-and-reset/) |
| 8 | `@import "tailwindcss/preflight" layer(zd-preflight);` | **delete** (ZW009); reset comes from the preset's `wind.reset` (`owned-v1` + authored preflight patch per the zudo-doc plan) |
| 9 | `@import "tailwindcss/utilities";` | **delete** (ZW009) |
| 13, 15–17 | `@import "@takazudo/zudo-doc/{theme,content,page-loading,features}.css"` | keep (package `exports` subpaths resolve since zfb 3.1.0, https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/) |
| 14 | `@import "@takazudo/zudo-doc/safelist.css";` | **delete** — the export is removed in 6.0; the package ships a `wind.json` manifest wired by `zudoDoc()` instead |
| 20–22 | `@source` ×3 | **delete** (ZW009; the source plan owns scan roots) |
| 26 | `@theme { }` | **delete** (ZW009); overrides go to `zudoDoc({ wind: { tokens } })` or `:root { --… }` |

Expected result: 4–5 `@import` lines. There is no host utility usage to migrate (audit above), so no `wind.tokens`, `authoredClasses`, `safelist` or ZW014 work. Cascade note for later content/CSS work: utilities now sit **after** authored CSS by default, so equal-specificity ties flip versus Tailwind; `wind.utilities.placement: "before-authored"` restores the old order and shipped in zfb 3.2.0 (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties).

### 3. Components and islands

- `doc/pages/docs/[[...slug]].tsx`: delete `:2` `/** @jsxImportSource preact */` (a per-file pragma overrides `tsconfig.json`; with Preact uninstalled the build fails with `Could not resolve "preact/jsx-runtime"`, and `zfb build`/`zfb dev` print a `zfb warn:` with file:line:col first — https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Replace `:25` `import type { JSX } from "preact"` with the zudo-react type the 6.0 route stub uses (#4473 lists `Child`/`Description`/`Component`/`JSX` replacements). This file is also **one generator revision behind**: it calls `createChrome(routeCtx)` without the `virtual:zudo-doc-chrome-bindings` import that the 5.28.2 (and 6.0) stub passes as the second argument. Re-scaffolding replaces the file wholesale, which is why re-scaffold beats hand-editing here.
- `doc/pages/index.tsx`: unchanged (1-line re-export of `@takazudo/zudo-doc/routes/index`).
- No host components, hooks, forms, islands or `rawHtml` sites exist; the Preact-hooks map (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/) does not apply to this repo.

### 4. md-wasm and other packages

- `@takazudo/zfb-md-wasm` is not a dependency and no file calls `compile()`/`renderHtml()`/`jsxRuntime` (`grep -rnE 'jsxRuntime|compile\(|renderHtml\(' doc` → only the `/** @jsxRuntime automatic */` pragma). Nothing to do.
- `packages/zit` is sharp-based Node code with no zfb import; `doc/examples-check/` typechecks against its `dist/*.d.ts` and is independent of zfb. Preserve `examples-check/` and the `check:examples` script when re-scaffolding.
- `@takazudo/zdtp 0.4.6` is a dead dependency today; drop it (see §1).

### 5. Tests, CI, deploy

- `.github/workflows/ci.yml`: never runs `zfb build` for `doc/`. Its `doc-examples` job (l.132–170) already does `pnpm -C doc install --frozen-lockfile` (l.163–164) before `check:examples`; append `pnpm -C doc build` (and `pnpm -C doc check`) there so the migration PR is gated before `main-deploy.yml` runs. The build is the gate, not `check` — `zfb check` never renders MDX.
- `.github/workflows/main-deploy.yml`: keep as-is. Pure static output means no `_worker.js`, no `.assetsignore` step needed (the `wrangler.toml` header comment already documents the worker-script variant if an adapter is ever added; the v3 adapter still emits `dist/_worker.js` + `_zfb_inner.mjs` + `.assetsignore`, https://zfb.takazudomodular.com/guides/ssr-and-cloudflare-bindings/). Bump `wrangler` in `doc/package.json` only as part of the scaffold's own pin.
- `scripts/run-b4push.sh` and `doc/package.json` `b4push` (`pnpm check && pnpm build`): unchanged; add `zfb wind audit --fail-on error` as a doc-side step once on v3 (https://zfb.takazudomodular.com/api/cli/#zfb-wind). The audit's `--plan build`, `--severity` and `--json` flags are absent from 3.1.0's `--help`; they shipped in zfb 3.2.0 (released 2026-10-04).
- `.github/dependabot.yml` covers GitHub Actions only; keep (the root lockfile is frozen by policy, and `doc/` has no npm ecosystem entry — consider adding `directory: "/doc"` after 6.0.0 so zudo-doc patch bumps arrive).
- `doc/.gitignore`: keep `.zfb`, `.zfb-build/`, `.zudo-doc/`, `.wrangler/`; v3 still writes `.zfb-build/` and `.zfb/graph.bin` (https://zfb.takazudomodular.com/api/cli/#scratch-dirs).

## Step-by-step plan

Adapted from the official 7-step checklist (https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist). Steps 1–2 can run today; 3–9 wait for `@takazudo/zudo-doc@6.0.0` + `create-zudo-doc@6`.

1. **Gate the doc build in CI (now).** Add the `doc/` build step to `ci.yml`; merge independently.
2. **Watch the gates (now).** `npm view @takazudo/zudo-doc dist-tags --json`, `npm view create-zudo-doc dist-tags --json`, `npm view @takazudo/zfb dist-tags --json`; zudolab/zudo-doc#4430 / #4477 / #4473; https://zfb.takazudomodular.com/changelog/zfb/.
3. **Re-scaffold into a sibling dir** (when 6.0.0 ships): from the repo root `pnpm create zudo-doc doc-next --yes --pm pnpm --search` (add the flags matching today's features: search, llmsTxt, claudeResources, imageEnlarge, dynamicPageTransition; check `create-zudo-doc --help` for the 6.0 flag names). Expect zfb pins = 6.0.0's peer floor (3.2.0 or later), no Tailwind, no Preact, `jsxImportSource: "@takazudo/zfb/zudo-react"`.
4. **Port the config.** Copy the 12 `zudoDoc()` fields from `doc/zfb.config.ts:5-62` into `doc-next/zfb.config.ts` (keep `claudeResources.claudeDir: "../.claude"`, `projectRoot: "."`, `scanRoot: ".."` and the 4 `defaultLocaleOnlyPrefixes`). Run `pnpm -C doc-next check` to catch renamed fields.
5. **Move content and non-zfb assets.** `cp -R doc/src/content doc-next/src/content`; copy `doc/examples-check/` and the `check:examples`, `b4push`, `setup:doc-skill*` scripts into `doc-next/package.json` (the 6.0 scaffold regenerates `scripts/setup-doc-skill.sh`; today's copy is 781 diff lines behind the 5.28.2 template). Copy `doc/wrangler.toml`, `doc/.npmrc`, `doc/.gitignore` extras (`.claude/skills/doc-wisdom/**`, `.codex/skills/doc-wisdom/**`), `doc/CLAUDE.md` (then rewrite its "Tailwind CSS v4 / Preact islands" tech-stack lines — the 6.0 generator writes a new one).
6. **Build and diff.** `cd doc-next && pnpm install && pnpm build 2>&1 | tee build.log; grep -i 'broken link' build.log; grep -c 'ZW0' build.log`. Compare `dist/` route lists against the live site (`find dist -name index.html | sort`), then open representative pages: `/`, `/docs/getting-started/`, `/docs/reference/variants/` (code fences + `<Tabs>`), `/docs/reference/budget/` (`<MathBlock>`), `/docs/claude-skills/l-make-release/` (generated page, long code blocks — the #3570 class of bug), `/docs/claude/` (`<SiteTreeNavDemo>`).
7. **Swap directories.** `git mv doc doc-old && git mv doc-next doc` (or replace in place), delete `doc-old` in the same PR; confirm `main-deploy.yml`'s path filter still matches.
8. **Audit gate.** `cd doc && npx zfb wind audit --fail-on error` → expect 0 strict diagnostics (host has none); `pnpm check`.
9. **Deploy.** Merge → `main-deploy.yml` → smoke `https://zudo-image-tweaker.takazudomodular.com/` 200. Spot-check search (Pagefind), theme toggle, the Claude section nav, and `llms.txt`.

In-place alternative to steps 3–7 (if a minimal diff is preferred): bump pins, apply §1–§3 edits by hand, and manually fold in the generator changes since 3.3.0 (chrome-bindings import in the route stub, `public/` favicon set from 5.0.0, `scripts/check-links.js` from 5.8.0). Same effort, more chance of drift.

## Verification checklist

- [ ] `pnpm -C doc build` exits 0 **and** `grep -i 'broken link'` of its output is empty (the build exits 0 on broken links).
- [ ] No `ZW009` in the build log; `npx zfb wind audit --fail-on error` passes; `zfb --version` prints a 3.x version and `embedded esbuild` only.
- [ ] `pnpm -C doc check` passes with `preact`/`@types/react` removed and the `paths` aliases gone.
- [ ] `grep -rn '@jsxImportSource' doc/pages doc/src` → 0.
- [ ] `pnpm -C doc check:examples` still passes after `pnpm build` at the root (examples-check untouched).
- [ ] Visual pass per the migration guide's reset checklist on `/docs/reference/exif/` (tables), `/docs/reference/variants/` (code, `sub`/`sup` if any), `/docs/getting-started/installation/` (`<CodeGroup>`): headings, lists, borders, `code`/`pre` font stack, `::placeholder` in the search box, `[hidden]` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#differences-from-tailwind-preflight).
- [ ] Anchors: the 10 in-page `](#…)` links and the 1 anchored cross-link resolve in built HTML (`grep -oE '"depth":[23],"slug":"[^"]*"' dist/**/index.html`).
- [ ] The 6 `generated: true` pages regenerate identically (or decide to stop committing them).
- [ ] Hydrated islands from the package (search, theme toggle, sidebar) work in a browser with no console errors; `main-deploy.yml` smoke returns 200.
- [ ] `git -C doc status --short` clean; `doc/pnpm-lock.yaml` contains no `preact`, `tailwindcss`, `@takazudo/zdtp`.

## Risks and open questions

- **Upstream timing is the whole risk.** Nothing here can be made green before `@takazudo/zudo-doc@6.0.0`; its zfb gate cleared with 3.2.0 on 2026-10-04, so what remains is zudo-doc's own release. If 5.x security patches are needed meanwhile, step 0 (bump to 5.28.2) is the only lever — DD8 means no further 5.x generator releases.
- **Generated Claude pages.** `claudeResources` scans `../.claude` and `..`; the 6 generated pages are committed under `src/content/docs/claude-*/`. If 6.0 changes the generator output (e.g. hierarchical IDs, frontmatter), the committed copies drift; decide at step 5 whether to regenerate or keep committing.
- **Cascade flip.** Not a risk today (no host CSS), but any future authored rule in `global.css` now loses equal-specificity ties to utilities unless `placement: "before-authored"` (zfb 3.2.0+) is set.
- **Island bundle size** grows (~2.2× per zudo-doc's measurement #3383) — affects first-load of search/theme-toggle; acceptable for a docs site, but check Lighthouse if it matters.
- **`zfb check` could not be run** on the clone (preset import requires `node_modules`); typing gaps in `zfb.config.ts` will only surface at step 4.
- **Open question:** does the 6.0 generator still accept `defaultLocaleOnlyPrefixes` for a single-locale site, and does `claudeResources.scanRoot: ".."` still work with the v3 source plan (scan roots are project-relative)? Verify at step 4 against zudolab/zudo-doc#4473.

## References

- Official guide and checklist: https://zfb.takazudomodular.com/guides/migrating-to-v3/
- Tailwind → zudo-wind map, reset table, placement/ties: https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/
- Preact hooks → zudo-react (not needed here, for completeness): https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/
- Config shape, removed keys: https://zfb.takazudomodular.com/api/define-config/
- `zfb wind audit|explain|manifest`, scratch dirs: https://zfb.takazudomodular.com/api/cli/
- Diagnostics ZW001–ZW014, strict vs audit-only origins: https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- Source plan (why the audit does not see the package): https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/
- Cloudflare adapter output (`_worker.js`, `.assetsignore`): https://zfb.takazudomodular.com/guides/ssr-and-cloudflare-bindings/
- zfb release notes: https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/ (released 2026-10-04)
- zudo-doc 6.0.0 status: zudolab/zudo-doc#4430 (epic), #4477 (root PR), #4473 (consumer migration guide), and `zudolab--zudo-doc.md` in this directory
- zfb fixes zudo-doc waited on: Takazudo/zudo-front-builder#3569, #3570 (shipped in zfb 3.2.0)
- Repo facts: `doc/package.json`, `doc/zfb.config.ts`, `doc/src/styles/global.css`, `doc/pages/docs/[[...slug]].tsx`, `doc/wrangler.toml`, `.github/workflows/{ci,main-deploy}.yml`, `CLAUDE.md`, `.claude/skills/l-make-release/SKILL.md`
- Commands used: `zfb wind audit --project-root doc` (zfb 3.1.0, temp `wind:{spec:1}` config, restored), the Tailwind/Preact/hooks/md-wasm greps from the shared briefing, `diff` against `zudo-doc/packages/create-zudo-doc/templates/base/**`, `gh api repos/zudolab/zudo-image-tweaker/commits?path=doc`

### Re-run this census

```sh
# from the repo root; ZFB = a zfb 3.x binary (npx -p @takazudo/zfb@latest zfb, or node_modules/.bin/zfb)
cp doc/zfb.config.ts /tmp/zfb.config.bak
printf 'import { defineConfig } from "zfb/config";\nexport default defineConfig({ wind: { spec: 1 } });\n' > doc/zfb.config.ts
$ZFB wind audit --project-root doc; cp /tmp/zfb.config.bak doc/zfb.config.ts; git status --short   # must be empty
grep -rnE '@import\s+"tailwindcss|@tailwind|@theme|@apply|@source|@utility|@variant|@custom-variant|@plugin|@config|theme\(' --include=*.css --include=*.ts --include=*.tsx doc --exclude-dir=node_modules
grep -rnE "from ['\"]preact|preact/hooks|@preact/signals|preact-render-to-string|@jsxImportSource" --include=*.ts --include=*.tsx --include=*.mdx doc --exclude-dir=node_modules
grep -rnE 'useState|useEffect|useRef|useMemo|useCallback|createContext|createPortal|forwardRef|dangerouslySetInnerHTML|className=|<Island|"use client"' --include=*.ts --include=*.tsx doc --exclude-dir=node_modules
grep -rhoE '<(CategoryNav|CategoryTreeNav|CodeGroup|Tabs|TabItem|MathBlock|SiteTreeNavDemo|Note|Tip|Info|Warning)\b' doc/src/content --include=*.mdx | sort | uniq -c
grep -rhoE '^:::[a-z]+' doc/src/content --include=*.mdx | sort | uniq -c
diff doc/src/styles/global.css <zudo-doc clone>/packages/create-zudo-doc/templates/base/src/styles/global.css   # IDENTICAL today
```
