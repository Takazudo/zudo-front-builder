# zfb v3 migration guide: Takazudo/gh-zudo-synth-components

Generated 2026-10-04 by an automated diagnosis of `main` @ `a65433f`. Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**Blocked. Effort S for this repo once its two presets ship; the real work is upstream.** `doc/` is byte-for-byte the same `create-zudo-doc@5.27.0` + `@takazudo/zudo-circuit-doc` host as Takazudo/zudo-osc-hole-field except for three files (`doc/package.json`, `doc/zfb.config.ts`, and the absence of `wrangler.jsonc`; `diff -rq` over `doc/` excluding `content/` and `public/`). It owns no components, islands, utility classes or CSS: `zfb wind audit` found **0 host-owned utility candidates** across `pages/`, `src/` and 162 MDX files. Everything zfb-facing comes from `@takazudo/zudo-doc` 5.27.0 (6.0.0 on zfb 3 in progress: zudolab/zudo-doc#4430 / PR #4477) and `@takazudo/zudo-circuit-doc` 0.1.0 exact (npm `latest` 0.2.0; peers `@takazudo/zfb ^2.20.2`, `@takazudo/zudo-doc ^5.27.0`, `preact ^10.29.1`; no v3 branch or issue in Takazudo/zudo-circuit-doc). A preset that still emits `framework`/`tailwind` fails config loading regardless of the host (https://zfb.takazudomodular.com/guides/migrating-to-v3/#before-you-start). What gates the start: zudo-doc 6.0.0, then a zudo-circuit-doc release targeting it. This repo additionally leans on `<PackageModelViewer>` (35 uses, three.js WebGL island) and a Playwright browser check in CI, so the island port quality in zudo-circuit-doc matters more here than in the instrument repo.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `doc/` (`zudo-modular-component-corpus-doc`; `doc/zfb.config.ts`) | `@takazudo/zfb` `2.21.0` exact, `-runtime` `2.21.0`, `-md-wasm` `2.21.0` (`doc/package.json:22-24`) | `@takazudo/zudo-doc` `5.27.0`, `@takazudo/zudo-doc-history-server` `5.27.0` (`:25-26`; history server is a scaffold leftover — `docHistory: false` at `doc/zfb.config.ts:20`); no zudo-sg, no zdtp | scaffold `doc/src/styles/global.css` only (`tailwindcss/preflight`, `tailwindcss/utilities`, 3 `@source`, empty `@theme {}`); no `tailwind.config.*`, no `@tailwindcss/*`, no `ZFB_TAILWIND*` | `preact ^10.29.1`, `preact-render-to-string ^6.6.6` (`:29-30`) for zudo-doc's engine; host has 1 pragma file (`doc/pages/docs/[[...slug]].tsx:1-2`), 0 hooks | none host-owned; `doc/pages/lib/_circuit-doc-islands.ts` imports `@takazudo/zudo-circuit-doc/islands` (2 Preact island roots) | dep only (zudo-doc HtmlPreview); no direct calls | **none**: no `wrangler.*`; `.github/workflows/check.yml` builds and uploads `doc/dist` as the `corpus-reading-site` artifact; `README.md:143`: "No website is deployed by this import" |
| `@takazudo/zudo-circuit-doc` (root `devDependencies` `0.1.0` exact; `doc/` dep `0.1.0` exact) | peer `^2.20.2` | peer `^5.27.0` | `styles.css` 525 lines authored BEM `zcd-*`, 0 directives, reads 22 zudo-doc theme custom properties (`--color-muted` x12, `--color-surface` x6, `--spacing-vsp-xs` x5, `--default-transition-duration` x4, …) | peer; 10 shipped modules import `preact`; hooks `useState` x5, `useRef` x5, `useEffect` x2 call sites (7/7/4 counting the import identifiers); `useModalDialog` from `@takazudo/zudo-doc/use-modal-dialog` | 3 `"use client"` modules: `FootprintPreviewIsland` (`useState` x4, `useRef` x2, `useEffect` x1), `PackageModelViewerIsland` (three.js 0.185.1; `useState` x1, `useRef` x3, `useEffect` x1), `PreviewEnlargeDialog` (`useModalDialog`) | none | n/a |
| repo root (`corpus/`, `scripts/build_corpus.py`, `scripts/build_browser.mjs`, `doc/public/assets/corpus-browser.html`) | none | none | none (the standalone viewer is bundled by `scripts/build_browser.mjs`, not zfb) | none | none | none | n/a |

Measured with: `find . -name 'zfb.config.*'`, the briefing greps, `diff -rq` against the sibling repo, `npm view` / `npm pack @takazudo/zudo-circuit-doc@0.2.0`, `gh api repos/Takazudo/zudo-circuit-doc/branches`.

## Sequencing and blockers

1. **zudo-doc 6.0.0** (zudolab/zudo-doc#4430, PR #4477; consumer guide #4473 planned). Blocked on zfb fixes #3569/#3570 that are on zfb `main` but **unreleased**; the epic's round-2 lock is exact 3.1.0 / peer `^3.1.0` (R2-DD3), so expect 6.0.0 to re-lock on the **next zfb release** — pin whatever its peer range names.
2. **zudo-circuit-doc v3 release** (public repo, `main` pushed 2026-10-02 for v0.2.0; branches `main` + `claude/quirky-clarke-ayd6am` at 0 ahead/95 behind; 0 open issues). Needs 2 islands + 7 UI components on zudo-react, peers `zfb ^3`/`zudo-doc ^6`, `preact` peer dropped, `styles.css` variables re-pointed at zudo-doc 6's emitted names. Separate M-size job; this guide lists what this host needs from it.
3. **This repo** (S): pins; re-copy the five vendored scaffold files from `create-zudo-doc@6`; re-apply the two ADR-015/ADR-016 glue lines; drop the dead history-server dep/script; rebuild; `pnpm check:site`, `pnpm corpus:check`, `scripts/test_browser.py`.
4. **Possible now**: file the zudo-circuit-doc tracking issue; note the plan in `ZUDO_DEPS_PINS.md`; nothing else — all pins are exact and CI installs with `--frozen-lockfile`, so nothing floats to 3.x by accident.

## Required changes

### 1. Dependencies, config, tsconfig, env

- `doc/package.json:22-24` — zfb family to the exact version zudo-doc 6.0.0 pins. One root `pnpm-lock.yaml` covers `doc/` (`pnpm-workspace.yaml`: `["doc"]`); no separate `doc/` lockfile. `doc/package.json:25` + root `package.json:23` — `@takazudo/zudo-doc` → `6.0.0`; `@takazudo/zudo-circuit-doc` → its first v3-compatible release (both exact, as today).
- `doc/package.json:26` + `:10,17,18` — `@takazudo/zudo-doc-history-server` and the `dev:history` / `dev:network` scripts are unused (`docHistory: false`). Drop them in the same PR, or keep the dep at `6.0.0` for lockstep; either way do not leave 5.27.0 behind.
- `doc/package.json:29-30` — delete `preact` and `preact-render-to-string` (only present for zfb's removed engine; https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands), unless zudo-circuit-doc's v3 release still declares a `preact` peer.

  ```jsonc
  "dependencies": {
    "@takazudo/zfb": "<pinned by zudo-doc 6.0.0>",
    "@takazudo/zfb-runtime": "<same>",
    "@takazudo/zfb-md-wasm": "<same>",
    "@takazudo/zudo-doc": "6.0.0",
    "@takazudo/zudo-circuit-doc": "<first v3-compatible release>",
    "zod": "^4.3.6",
    "diff": "^8.0.3"
    // removed: preact, preact-render-to-string, @takazudo/zudo-doc-history-server (docHistory is false)
  }
  ```

- `doc/zfb.config.ts` (77 lines) — no host-side `framework`/`tailwind` key exists (both are emitted by `zudoDoc()` in 5.x); the `delete site.markdown?.features?.linkValidation;` at `:75` is unrelated and stays. An absent `wind` key is the empty v1 config (reset `none`, no tokens; https://zfb.takazudomodular.com/zudo-wind/configuration/#the-wind-key), so tokens/reset must come from the preset. zudo-doc 6 plans the consumer override as `zudoDoc({ wind: {...} })`, package-owned tokens as `var(--…)`, `owned-v1` reset plus an authored preflight patch, no `spacingUnit`, `sm`/`lg`/`xl` breakpoints, `dark: false` and a `./wind.json` manifest (DD4 in #4430; confirm in #4473). This host has no utilities of its own, so it should need no override.
- `doc/tsconfig.json:8-10` — remove the `react`/`react/jsx-runtime`/`react-dom` → `preact/compat` aliases; `jsx: "react-jsx"` + `jsxImportSource: "@takazudo/zfb/zudo-react"` should arrive via `@takazudo/zudo-doc/tsconfig.base.json` (planned flip of a consumer-shipped file); add them locally only if 6.0.0 does not.

  ```json
  {
    "extends": "@takazudo/zudo-doc/tsconfig.base.json",
    "include": ["src", "pages", "zfb.config.ts"],
    "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["src/*"] } }
  }
  ```

- Env/CI: `grep -rn ZFB_TAILWIND .github package.json scripts` → 0 hits. Nothing to remove.
- `ZUDO_DEPS_PINS.md:19,43` — move both `pinned:` values together; the ADR-003 "fall back to 2.20.3" rule has no meaning on 3.x; replace with "fall back to the last green 2.x set".

### 2. CSS and utilities

- `doc/src/styles/global.css` (31 lines) — ZW009 errors on v3, including under `wind: false` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives): `:8` `@import "tailwindcss/preflight" layer(zd-preflight)`, `:9` `@import "tailwindcss/utilities"`, `:14` `@import "@takazudo/zudo-doc/safelist.css"` (a Tailwind `@source inline(...)` sheet in 5.x; zudo-doc 6 plans to retire it in favour of a `wind.json` candidate manifest declared by the preset), `:23-25` three `@source`, `:29-31` empty `@theme {}`. `:7` `@layer zd-preflight, zd-flow;` stays valid. `:20` `@import "@takazudo/zudo-circuit-doc/styles.css";` (ADR-015) stays, but its `var(--color-*|--spacing-*|--text-*|--default-transition-duration|--z-index-*)` references must match what zudo-doc 6 emits (`@theme` → `:root` custom properties planned; wind token variables are `--zw-*`, https://zfb.takazudomodular.com/zudo-wind/tokens/). `--default-transition-duration` is Tailwind's and will not exist — that fix lives in zudo-circuit-doc.
- Re-copy `global.css` from `create-zudo-doc@6` (procedure in `ZUDO_DEPS_PINS.md:21-23`) and re-add line 20 rather than hand-editing.

  ```css
  /* expected v3 shape — exact list = what create-zudo-doc@6 emits */
  @layer zd-preflight, zd-flow;
  @import "@takazudo/zudo-doc/theme.css";
  @import "@takazudo/zudo-doc/content.css";
  @import "@takazudo/zudo-doc/page-loading.css";
  @import "@takazudo/zudo-doc/features.css";
  /* ADR-015 */
  @import "@takazudo/zudo-circuit-doc/styles.css";
  ```

- Reset: today's page relies on Tailwind preflight in the `zd-preflight` layer. zudo-doc 6 plans `owned-v1` plus an authored preflight patch (DD4 in #4430); the measured preflight → `owned-v1` differences the patch must cover are `sub`/`sup`, `small`, `hr`, `::placeholder`, `::file-selector-button`, `[hidden]` and control backgrounds (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#differences-from-tailwind-preflight). Check the evidence tables, `<details>` blocks and the model-viewer `<dialog>` — a prose-only pixel diff misses them.
- Cascade: wind places utilities **after** authored unlayered CSS by default (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties); zudo-circuit-doc's `styles.css` is unlayered, so `(0,1,0)` `zcd-*` rules that beat a zudo-doc utility by order today will lose. `wind.utilities.placement: "before-authored"` is **next zfb release** (3.1.0 rejects the key as `wind.utilities: unknown field`, measured); zudo-doc keeps the default order and fixes flips locally (DD5 in #4430).
- Audit (zfb 3.1.0, temporary `wind: { spec: 1 }`, `zfb wind audit --project-root doc`, exit 0):

  ```text
  outcome: complete
  spec: 1 revision 3
  unrecognized classes: (none)
  conflicts: (none)
  dead classes: (none)
  dynamic constructions:
    - docs; at default/pages:docs/[[...slug]].tsx:2636
  diagnostics: 11 x auditInfo (ZW002 x2, ZW005 x8, ZW012 x1) — import specifiers in 4 files + the template literal
  ```

  162 MDX/MD files: 0 `class=`/`className=`. Content JSX: `<EvidenceAnchor>` x686, `<EvidenceFact>` x261, `<EvidenceTable>` x36, `<PackageModelViewer>` x35, `<EvidenceDetails>` x35, `<ComponentReferences>` x35, `<CategoryNav>` x1 — all package components. The host declares no tokens. The 11 `auditInfo` lines sit at import specifiers (`@takazudo/zudo-doc/...`, `virtual:zudo-doc-*`, `../lib/_circuit-doc-islands`) in the stub, `pages/index.tsx`, `pages/lib/_circuit-doc-islands.ts` and `src/chrome-bindings.tsx` — not in comments. (3.1.0 prints byte offsets; `file:line:col` audit locations and `--json`/`--severity`/`--plan` are **next zfb release**.)

### 3. Components and islands

- `doc/src/chrome-bindings.tsx` (11 lines): `defineChromeBindings({ mdxExtras: { ...circuitDocMdxExtras } })`, no JSX, no pragma — unchanged if both packages keep those exports.
- `doc/pages/docs/[[...slug]].tsx`: generator stub with `/** @jsxImportSource preact */` (`:1-2`) and `import type { JSX } from "preact"` (`:27`); plus the one host line `:37` `import "../lib/_circuit-doc-islands";` (ADR-016). On v3 a Preact pragma in a page fails the build after a `zfb warn:` with file:line:col (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Re-copy the 6.0.0 stub and re-add line 37.
- `doc/pages/index.tsx`: 1-line re-export of `@takazudo/zudo-doc/routes/index`.
- `doc/pages/lib/_circuit-doc-islands.ts`: side-effect import of `@takazudo/zudo-circuit-doc/islands`; v3's scanner follows installed packages and registers only reachable `<Island>` targets (https://zfb.takazudomodular.com/concepts/islands/#boundary-discovery-and-migration), so the seed keeps working once the package's islands are zudo-react components.
- `<PackageModelViewer>` is this corpus's signature feature (35 record pages, 22 shared WRL models + 3 STEP-derived meshes). The v3 island will be `PackageModelViewerIsland` on zudo-react: WebGL context creation belongs in `getScope().onActivate` (browser-only), teardown in its cleanup, state in `signal()`s; the enlarge `<dialog>` goes through `<Show>` (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/, https://zfb.takazudomodular.com/zudo-react/scopes-and-lifecycle/). `scripts/test_browser.py` records the actual renderer (WebGL vs SVG fallback) — keep that assertion as the acceptance test for the port.

### 4. md-wasm and other packages

- No host `compile()`/`renderHtml()` calls; `@takazudo/zfb-md-wasm` is zudo-doc's HtmlPreview dependency. The `jsxRuntime` removal (https://zfb.takazudomodular.com/guides/migrating-to-v3/#mdx-and-md-wasm) is zudo-doc's concern.
- `scripts/build_corpus.py` renders corpus-owned MDX from `corpus/catalog.json` / `corpus/guides.json`; `scripts/build_browser.mjs` bundles the standalone `corpus-browser.html` with its own locked runtime deps. Neither goes through zfb; unaffected.

### 5. Tests, CI, deploy

- `.github/workflows/check.yml:35-55` — `pnpm install --frozen-lockfile` → `pnpm circuit:check` + `pnpm check` (`zudo-circuit-doc check && zfb check`) → `pnpm build` (`corpus:generate && models && generate && zfb build`) → `pnpm check:site` → `pnpm corpus:check` → `git diff --exit-code -- corpus doc/src/content/docs circuit/generated circuit/publication doc/public`. The drift gate means a v3 generator that changes emitted MDX must be committed in the same PR (expected churn, not a regression).
- `:56-75` — Playwright 1.58.0 `scripts/test_browser.py --chromium` over the built site: this is the repo's Level-4 check for the islands; it stays the acceptance test.
- Add after `pnpm check`: `pnpm --dir doc exec zfb wind audit --fail-on error` (`--fail-on` exists in 3.1.0; https://zfb.takazudomodular.com/api/cli/#zfb-wind).

  ```yaml
      - name: Audit utility candidates
        run: pnpm --dir doc exec zfb wind audit --fail-on error
  ```

- No deploy exists; the `corpus-reading-site` artifact is the thing to diff.

## Step-by-step plan

Adapted from the 7-step checklist (https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist).

1. **Wait for the presets.** Track zudolab/zudo-doc#4477 and #4473; open a "Port to zfb 3 / zudo-doc 6" issue on Takazudo/zudo-circuit-doc carrying the island/hook census from the sibling guide (Takazudo--zudo-osc-hole-field.md §3). Nothing below runs before both ship.
2. **Baseline.** `git tag pre-zfb3` on green `main`; download the current `corpus-reading-site` artifact for diffing.
3. **Pins.** `doc/package.json` + root `package.json` as in §1; `pnpm install`; `pnpm --dir doc exec zfb --version`.
4. **Re-sync scaffold files.** `pnpm create zudo-doc@6 <scratch>`, diff `app/{pages/docs/[[...slug]].tsx,pages/index.tsx,tsconfig.json,src/styles/global.css,scripts/check-links.js}` against `doc/`, copy, re-apply the ADR-016 import (stub) and ADR-015 import (global.css), drop `paths` aliases; update `ZUDO_DEPS_PINS.md`.
5. **Config.** `pnpm check` — `zfb check` must load the config (a stale preset fails with the named `framework`/`tailwind` error).
6. **CSS + audit.** `pnpm build` (any ZW009 is listed with `file:line:column`), then `pnpm --dir doc exec zfb wind audit --fail-on error`.
7. **Verify.** `pnpm check:site && pnpm corpus:check`, `python3 scripts/test_browser.py --chromium <chrome>`, `git diff --exit-code` on the generated paths, then browser-diff `/docs/project/`, `/docs/parts/`, one record page with `<PackageModelViewer>` (open/close the enlarge dialog, focus return, WebGL vs SVG label), `/docs/claude/`.

## Verification checklist

- [ ] `pnpm --dir doc exec zfb --version` → 3.x + `embedded esbuild` only.
- [ ] `pnpm check` green; `pnpm build` green with no `ZW009` and no `zfb warn:` pragma lines.
- [ ] `pnpm --dir doc exec zfb wind audit --fail-on error` exit 0.
- [ ] `pnpm check:site`, `pnpm corpus:check`, `scripts/test_browser.py` green; `provenance/validation/browser-repro.json` reports the expected renderer.
- [ ] `git diff --exit-code -- corpus doc/src/content/docs circuit/generated circuit/publication doc/public` clean after a full `pnpm build`.
- [ ] Browser: model viewer renders and tears down on navigation (`dynamicPageTransition: true` is on), enlarge dialog, search, theme toggle; reset table items (`sub`/`sup`, `<details>`, `::placeholder`).
- [ ] `git grep -n preact doc/` → comments only; `ZUDO_DEPS_PINS.md` matches `doc/package.json`.

## Risks and open questions

- **Chain of two upstreams**: zudo-doc 6.0.0 needs the next zfb release; zudo-circuit-doc has not started. This host is last.
- **Token-name coupling** in `@takazudo/zudo-circuit-doc/styles.css` (22 zudo-doc custom properties incl. Tailwind's `--default-transition-duration`): silent visual loss, no diagnostic. Ask zudo-circuit-doc to pin against 6.0.0's emitted names.
- **three.js inside a zudo-react island**: lifecycle must move to `onActivate`/cleanup; `dynamicPageTransition: true` means islands are disposed and re-created across client-side navigations — watch for WebGL context leaks in the browser check.
- **`useModalDialog`** (zudo-doc 5.x Preact hook) is ported first as a shared zudo-react primitive in the epic (#4441 in #4430); the dialog port should target that 6.0.0 export.
- **Cascade flip** for unlayered `zcd-*` rules; `placement: "before-authored"` is next zfb release only.
- Not verified here: `zfb check` cannot run on the clone (preset imports need `node_modules`); the audit used a stand-in `wind: { spec: 1 }` config.

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ , https://zfb.takazudomodular.com/zudo-wind/configuration/ , https://zfb.takazudomodular.com/zudo-wind/tokens/ , https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/ , https://zfb.takazudomodular.com/zudo-react/scopes-and-lifecycle/ , https://zfb.takazudomodular.com/zudo-react/conditionals-and-lists/
- https://zfb.takazudomodular.com/concepts/islands/ , https://zfb.takazudomodular.com/api/cli/
- https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/ (v2.21.0–v2.22.1 notes carry no Breaking Changes section)
- zudolab/zudo-doc#4430, #4477, #4473; Takazudo/zudo-front-builder#3569, #3570
- https://github.com/Takazudo/zudo-circuit-doc (0.2.0; peers zfb ^2.20.2 / zudo-doc ^5.27.0 / preact ^10.29.1; no v3 branch)
- Sibling guide with the identical host glue: `Takazudo--zudo-osc-hole-field.md`
- This repo: `ZUDO_DEPS_PINS.md`, `doc/zfb.config.ts`, `doc/src/styles/global.css`, `doc/pages/docs/[[...slug]].tsx`, `.github/workflows/check.yml`, `scripts/test_browser.py`
