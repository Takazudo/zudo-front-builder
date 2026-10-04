# zfb v3 migration guide: Takazudo/zudo-osc-hole-field

Generated 2026-10-04 by an automated diagnosis of `main` @ `858df0f`. Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**Blocked. Effort S for this repo once its two presets ship; the real work is upstream.** `doc/` is a `create-zudo-doc@5.27.0` scaffold with the `@takazudo/zudo-circuit-doc` host glue layered on top. It owns no components, no islands, no utility classes and no CSS beyond the generated `global.css` chain: the `zfb wind audit` run below found **0 host-owned utility candidates** across `pages/`, `src/` and 146 MDX files. Every zfb-facing byte comes from two presets that are both still 2.x: `@takazudo/zudo-doc` 5.27.0 (6.0.0 on zfb 3 is in progress, zudolab/zudo-doc#4430 / PR #4477) and `@takazudo/zudo-circuit-doc` 0.2.0 (npm `latest`, published 2026-10-02, peers `@takazudo/zfb ^2.20.2`, `@takazudo/zudo-doc ^5.27.0`, `preact ^10.29.1`; no v3 branch or issue exists in Takazudo/zudo-circuit-doc as of today). Nothing in this repo can be switched to zfb 3 before **both** publish v3-compatible releases; a preset that still emits `framework`/`tailwind` fails config loading even if the host is clean (https://zfb.takazudomodular.com/guides/migrating-to-v3/#before-you-start). What gates the start: zudo-doc 6.0.0 first, then a zudo-circuit-doc release that targets it.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `doc/` (`zudo-osc-hole-field-doc`, the only zfb project; `doc/zfb.config.ts`) | `@takazudo/zfb` `2.21.0` exact, `-runtime` `2.21.0`, `-md-wasm` `2.21.0` (`doc/package.json:22-24`) | `@takazudo/zudo-doc` `5.27.0`, `@takazudo/zudo-doc-history-server` `5.27.0` (`:25-26`); no zudo-sg, no zdtp | only via the scaffold `doc/src/styles/global.css` (`tailwindcss/preflight`, `tailwindcss/utilities`, 3 `@source`, empty `@theme {}`); no `tailwind.config.*`, no `@tailwindcss/*` dep, no `ZFB_TAILWIND*` in scripts/CI | `preact ^10.29.1`, `preact-render-to-string ^6.6.6` (`:29-30`) kept only for zudo-doc's engine; host code has 1 pragma file (`doc/pages/docs/[[...slug]].tsx:1-2`) and 0 hooks | none host-owned; `doc/pages/lib/_circuit-doc-islands.ts` side-effect-imports `@takazudo/zudo-circuit-doc/islands` (2 Preact island roots in the package) | dep only (zudo-doc's HtmlPreview path); no direct `compile()`/`renderHtml()` calls | Cloudflare Workers static assets, `doc/wrangler.jsonc` (`./dist`, custom domain `zudo-osc-hole-field.zudolab.dev`); **not deployed by CI** (`.github/workflows/check.yml` builds and checks only; `CLAUDE.md` rule 6 forbids agents to deploy) |
| `@takazudo/zudo-circuit-doc` (root `devDependencies` `^0.1.0`, `doc/` dep `^0.1.0`; npm `latest` 0.2.0) | peer `^2.20.2` | peer `^5.27.0` | none in the package: `styles.css` (525 lines) is authored BEM (`zcd-*`), 0 Tailwind directives, but reads 22 zudo-doc theme custom properties (`--color-muted` x12, `--color-surface` x6, `--spacing-vsp-xs` x5, `--default-transition-duration` x4, `--text-caption` x3, …) | peer `^10.29.1`; 10 shipped modules import `preact` (`lib/ui/*` 7, `lib/islands/*` 3); hooks: `useState` x5, `useRef` x5, `useEffect` x2 call sites (7/7/4 counting the `preact/hooks` import identifiers); `useModalDialog` from `@takazudo/zudo-doc/use-modal-dialog`; `className` x26 | 3 `"use client"` modules: `FootprintPreviewIsland`, `PackageModelViewerIsland` (three.js 0.185.1), `PreviewEnlargeDialog` | none | n/a (library) |
| repo root (`package.json`, `circuit/`, `scripts/`, `design/`, KiCad/Python) | none | none | none | none | none | none | n/a |

Measured with: `find . -name 'zfb.config.*'`, the greps from the briefing, `npm view @takazudo/zudo-circuit-doc@0.2.0`, `npm pack @takazudo/zudo-circuit-doc@0.2.0` + `grep` over the tarball, `gh api repos/Takazudo/zudo-circuit-doc/branches`.

## Sequencing and blockers

1. **zudo-doc 6.0.0** (zudolab/zudo-doc#4430, root PR #4477, consumer migration guide planned as #4473). Its integration floor was blocked on zfb bugs Takazudo/zudo-front-builder#3569/#3570, fixed on zfb `main` 2026-10-04 but **unreleased** (ships in the release after 3.1.0). The epic's round-2 lock is exact `3.1.0` / peer `^3.1.0` (R2-DD3 in #4430), but its integration floor (#4467) needs those fixes, so expect 6.0.0 to re-lock on the next zfb release — pin whatever 6.0.0's peer range names, not `3.1.0` blindly.
2. **zudo-circuit-doc v3 release** (Takazudo/zudo-circuit-doc, public). Not started: branches are `main` and `claude/quirky-clarke-ayd6am` (0 ahead / 95 behind), 0 open issues, last push 2026-10-02 (v0.2.0). It must port 2 islands + 7 UI components to zudo-react, drop the `preact` peer, re-target its `styles.css` custom-property names to whatever zudo-doc 6 emits, and raise its peers to `zfb ^3` / `zudo-doc ^6`. That is a separate job in that repo — its own guide (`Takazudo--zudo-circuit-doc.md`, same directory) rates it **L** because the runtime library, `create-zudo-circuit-doc`, two example hosts, the SSR test harness and a coordinated 0.3.0 release move together; this guide only names what this host needs from it (see Risks).
3. **This repo** (S): bump pins, re-copy the five vendored scaffold files from `create-zudo-doc@6`, re-apply the two ADR-015/ADR-016 glue lines, rewrite `doc/CLAUDE.md`-equivalent text, rebuild, run `pnpm check:site`.
4. **Possible now** (no upstream needed): a tracking issue in Takazudo/zudo-circuit-doc, the `ZUDO_DEPS_PINS.md` note below, and verifying the current pins do not float (`doc/package.json` pins zfb exactly; root `^0.1.0` of zudo-circuit-doc excludes 0.2.0 under 0.x caret rules; `pnpm install --frozen-lockfile` in CI). Do **not** bump `@takazudo/zfb` to 3.x ahead of the presets.

## Required changes

### 1. Dependencies, config, tsconfig, env

- `doc/package.json:22-24` — move `@takazudo/zfb`, `@takazudo/zfb-runtime`, `@takazudo/zfb-md-wasm` to the exact version zudo-doc 6.0.0 pins (not before). One root `pnpm-lock.yaml` covers `doc/` (`pnpm-workspace.yaml` lists `["doc"]`); there is no separate `doc/` lockfile. `zfb --version` on v3 prints only the release and `embedded esbuild` lines (https://zfb.takazudomodular.com/guides/migrating-to-v3/#cli-and-environment).
- `doc/package.json:25-27` — `@takazudo/zudo-doc` and `@takazudo/zudo-doc-history-server` to `6.0.0` (lockstep, per zudo-doc's release plan); `@takazudo/zudo-circuit-doc` (also root `package.json:21`) to its first v3-compatible release.
- `doc/package.json:29-30` — delete `preact` and `preact-render-to-string`: they exist only for zfb's removed engine (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Keep them only if zudo-circuit-doc's v3 release still declares a `preact` peer (it should not; its islands are rendered by zudo-react, not an opaque widget).

  Expected `doc/package.json` shape after the bump (versions are placeholders until 6.0.0 and the zudo-circuit-doc v3 release publish):

  ```jsonc
  "dependencies": {
    "@takazudo/zfb": "<zfb version pinned by zudo-doc 6.0.0>",
    "@takazudo/zfb-runtime": "<same>",
    "@takazudo/zfb-md-wasm": "<same>",
    "@takazudo/zudo-doc": "6.0.0",
    "@takazudo/zudo-doc-history-server": "6.0.0",
    "@takazudo/zudo-circuit-doc": "<first v3-compatible release>",
    "zod": "^4.3.6",
    "diff": "^8.0.3"
    // preact, preact-render-to-string: removed
  }
  ```

- `doc/zfb.config.ts` — host-side there is **no** `framework` or `tailwind` key to delete (lines 8-68 are `zudoDoc({...})` options; both removed keys are emitted by `zudoDoc()` itself in 5.x). The `delete site.markdown?.features?.linkValidation;` mutation at line 78 is unrelated to v3 and can stay. zudo-doc 6 plans a consumer override as `zudoDoc({ wind: {...} })`, package-owned tokens as `var(--…)` pointing at authored custom properties, `owned-v1` reset plus an authored preflight patch, no `spacingUnit`, `sm`/`lg`/`xl` breakpoints, `dark: false` and a `./wind.json` manifest (DD4 in #4430; confirm the shapes in #4473). This host has no utilities of its own, so it should need no override. An absent `wind` key means empty v1 config, reset `none`, no tokens (https://zfb.takazudomodular.com/zudo-wind/configuration/#the-wind-key) — the preset must supply them.
- `doc/tsconfig.json:8-10` — remove the `react` / `react/jsx-runtime` / `react-dom` → `preact/compat` path aliases. `jsx: "react-jsx"` + `jsxImportSource: "@takazudo/zfb/zudo-react"` come from `@takazudo/zudo-doc/tsconfig.base.json` once 6.0.0 flips it (planned, shipped-to-consumers file); if 6.0.0 does not, add both under `compilerOptions` per https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands.

  Target `doc/tsconfig.json` (the `paths` block goes away; `jsx`/`jsxImportSource` only if the 6.0.0 base does not set them):

  ```json
  {
    "extends": "@takazudo/zudo-doc/tsconfig.base.json",
    "include": ["src", "pages", "zfb.config.ts"],
    "compilerOptions": {
      "baseUrl": ".",
      "paths": { "@/*": ["src/*"] }
    }
  }
  ```

- Env: no `ZFB_TAILWIND_BIN` / `ZFB_TAILWIND_OXIDE_WARMUP` anywhere in `.github/workflows/check.yml`, `package.json`, `scripts/` (grep returned 0 hits). Nothing to do.
- `ZUDO_DEPS_PINS.md:19,43` — `pinned:` for create-zudo-doc and the zfb family must move together; the ADR-003 fallback ("pin to 2.20.3 if 2.21.0 fails") becomes meaningless on 3.x — replace it with a fallback to the last green 2.x set.

### 2. CSS and utilities

- `doc/src/styles/global.css` — every directive below is a **ZW009 error** on v3, even under `wind: false` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives):
  - `:8` `@import "tailwindcss/preflight" layer(zd-preflight);`
  - `:9` `@import "tailwindcss/utilities";`
  - `:14` `@import "@takazudo/zudo-doc/safelist.css";` — 5.x ships this as a Tailwind `@source inline(...)` sheet; zudo-doc 6 plans to retire the export and replace it with a `wind.json` candidate manifest declared by the preset (planned; see #4430). Delete the line when 6.0.0's template drops it.
  - `:23-25` the three `@source` lines; `:29-31` the empty `@theme {}` slot.
  - `:7` `@layer zd-preflight, zd-flow;` is an ordinary layer statement and stays valid; `:13,15-17` (`theme.css`, `content.css`, `page-loading.css`, `features.css`) stay as plain CSS imports if 6.0.0 keeps those exports.
  - `:20` `@import "@takazudo/zudo-circuit-doc/styles.css";` (ADR-015) stays — but its 22 `var(--color-*|--spacing-*|--text-*|--default-transition-duration|--z-index-*)` references must match zudo-doc 6's emitted custom properties. zudo-doc 6 plans to turn `theme.css`'s `@theme` blocks into `:root` custom properties; wind's own token variables are emitted as `--zw-<family>-<name>` (https://zfb.takazudomodular.com/zudo-wind/tokens/). `--default-transition-duration` is a Tailwind theme variable and will not exist; that fix belongs in zudo-circuit-doc.
- Do **not** hand-edit `global.css`; re-copy it from `create-zudo-doc@6` (the `sync:` procedure already in `ZUDO_DEPS_PINS.md:21-23`) and re-add line 20.

  Shape of the v3 entry sheet this host should end up with (exact import list = whatever `create-zudo-doc@6` emits; only line 20's ADR-015 import is host-owned):

  ```css
  /* generated by create-zudo-doc@6 — no tailwindcss imports, no @source, no @theme */
  @layer zd-preflight, zd-flow;
  @import "@takazudo/zudo-doc/theme.css";
  @import "@takazudo/zudo-doc/content.css";
  @import "@takazudo/zudo-doc/page-loading.css";
  @import "@takazudo/zudo-doc/features.css";

  /* ADR-015: the package's unlayered styles, imported after the zudo-doc CSS. */
  @import "@takazudo/zudo-circuit-doc/styles.css";
  ```

- Reset: the 5.x page relies on Tailwind preflight (`zd-preflight` layer). zudo-doc 6 plans `owned-v1` plus an authored preflight patch (DD4 in #4430); the measured preflight → `owned-v1` differences the patch must cover are `sub`/`sup`, `small`, `hr`, `::placeholder`, `::file-selector-button`, `[hidden]` and control backgrounds (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#reset-differences-from-tailwind-preflight). The generated component pages contain evidence tables, `<details>` blocks and `<dialog>` previews — check those, not just a prose page.
- Utility placement flips ties: wind emits utilities **after** authored unlayered CSS by default (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties). zudo-circuit-doc's `styles.css` is unlayered and imported after zudo-doc's CSS; a `(0,1,0)` `zcd-*` rule that today beats a zudo-doc utility by order will lose. `wind.utilities.placement: "before-authored"` restores the old order but is **next zfb release** only (3.1.0 rejects the key with `wind.utilities: unknown field`, measured — 3.1.0 accepts only `spec`, `reset`, `tokens`, `breakpoints`, `dark`, `defaultTransitionTimingFunction`, `safelist`, `authoredClasses`, `manifests`); zudo-doc accepted the flipped order for its own CSS (DD5 in #4430) and has not decided whether the 6.0 preset ships `before-authored` for consumers once the release lands (open question in `zudolab--zudo-doc.md`), so zudo-circuit-doc should fix its own ties locally rather than rely on the key.
- Audit result (zfb 3.1.0, temporary `wind: { spec: 1 }` config, `zfb wind audit --project-root doc`): `unrecognized classes: (none)`, `conflicts: (none)`, `dead classes: (none)`, 1 dynamic construction (`docs;${locale}` template literal, `pages/docs/[[...slug]].tsx:2636` byte offset) and 12 `auditInfo` lines (ZW002 x2, ZW005 x9, ZW012 x1; stub x8, `pages/index.tsx` x1, `pages/lib/_circuit-doc-islands.ts` x1, `src/chrome-bindings.tsx` x2), all at **import specifiers** (`@takazudo/zudo-doc/...`, `virtual:zudo-doc-*`, `../lib/_circuit-doc-islands`) in the stub, `pages/index.tsx`, `pages/lib/_circuit-doc-islands.ts` and `src/chrome-bindings.tsx`, plus that one template literal — low-confidence literals, never errors. 146 MDX files contain 0 `class=`/`className=`. Content JSX is `<EvidenceAnchor>` x1238, `<EvidenceFact>` x528, `<EvidenceTable>` x60, `<EvidenceDetails>` x58, `<ComponentReferences>` x58, `<CategoryNav>` x6 — all package components. The host declares no tokens of its own.


  Raw audit header as printed by zfb 3.1.0 (byte offsets, not line:col; `file:line:col` audit locations are **next zfb release**):

  ```text
  outcome: complete
  spec: 1 revision 3
  unrecognized classes: (none)
  conflicts: (none)
  dead classes: (none)
  dynamic constructions:
    - docs; at default/pages:docs/[[...slug]].tsx:2636
  diagnostics: 12 x auditInfo (ZW002 x2, ZW005 x9, ZW012 x1) — import specifiers in 4 files + the template literal
  ```

### 3. Components and islands

- `doc/src/chrome-bindings.tsx` (11 lines) spreads `circuitDocMdxExtras` into `defineChromeBindings({ mdxExtras })`; no JSX, no pragma. Unchanged, provided zudo-doc 6 keeps `@takazudo/zudo-doc/chrome-bindings` and zudo-circuit-doc keeps `/mdx-extras`.
- `doc/pages/docs/[[...slug]].tsx` — generator stub (locked manifest #2653) with `/** @jsxImportSource preact */` at `:1-2` and `import type { JSX } from "preact"` at `:27`. On v3 a per-file Preact pragma fails the build (`Could not resolve "preact/jsx-runtime"`, or `ZR_CHILD` if preact is still installed); `zfb build`/`zfb dev` warn with file:line:col first (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands). Re-copy the 6.0.0 stub and re-add the one ADR-016 line (`:37` `import "../lib/_circuit-doc-islands";`).
- `doc/pages/index.tsx` — 1-line re-export of `@takazudo/zudo-doc/routes/index`; nothing to do if the export survives.
- `doc/pages/lib/_circuit-doc-islands.ts` — side-effect import; v3's scanner follows installed packages and `"use client"` roots (https://zfb.takazudomodular.com/concepts/islands/#boundary-discovery-and-migration), so the seed keeps working once zudo-circuit-doc's islands are zudo-react components.
- What zudo-circuit-doc must deliver (for the maintainer's upstream issue, measured from the 0.2.0 tarball): `FootprintPreviewIsland` (`useRef` x2, `useState` x4, `useEffect` x1, `onClick`/`onLoad`/`onError`, `className`), `PackageModelViewerIsland` (three.js viewer; `useRef` x3, `useState` x1, `useEffect` x1), `PreviewEnlargeDialog` (`useModalDialog` from `@takazudo/zudo-doc/use-modal-dialog`, `onKeyDown` focus trap, `<dialog>`), and 7 `lib/ui/*` server components. Conversion map: `useState` → `signal()`, `useEffect([])` → `getScope().onActivate`, `useRef` → `{ current: null }` `Ref`, `isDialogOpen && …` → `<Show>`, `className` → `class`, `onClick` → `on:click`, `strokeWidth` → `stroke-width` (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/). The native `<dialog>` approach already matches v3's "no portals" guidance.

### 4. md-wasm and other packages

- No host code calls `compile()`/`renderHtml()`; `@takazudo/zfb-md-wasm` is a dep only because zudo-doc's HtmlPreview island lazy-loads it. The `jsxRuntime` option removal (https://zfb.takazudomodular.com/guides/migrating-to-v3/#mdx-and-md-wasm) is zudo-doc's concern.
- `@takazudo/zudo-circuit-doc` uses `mdast-util-*` to emit MDX text; it does not consume md-wasm. Unaffected.
- The Python/KiCad/`scripts/` toolchain and `circuit.config.ts` are not zfb.

### 5. Tests, CI, deploy

- `.github/workflows/check.yml:42-55` — `pnpm install --frozen-lockfile` → `pnpm circuit:check` → `pnpm check` (`zudo-circuit-doc check && zfb check`) → `pnpm build` (`zudo-circuit-doc models && generate && zfb build`) → `pnpm check:site`. No command changes; the lockfile is the switch. Add `pnpm --dir doc exec zfb wind audit --fail-on error` after `pnpm check` once on v3 (`--fail-on` exists in 3.1.0; `--json`/`--severity`/`--plan` are **next zfb release**; https://zfb.takazudomodular.com/api/cli/#zfb-wind).

  Suggested CI addition (`.github/workflows/check.yml`, after `Check documentation`):

  ```yaml
        - name: Audit utility candidates
          run: pnpm --dir doc exec zfb wind audit --fail-on error
  ```

- `scripts/checks/check-site-host.mjs:9-40` parses `doc/zfb.config.ts` with the TypeScript AST looking for `zudoDoc({ siteUrl })` — keep `siteUrl` a string literal inside the `zudoDoc()` call when editing config.
- `pnpm check:site` → `zudo-circuit-doc check-built`, `scan`, and `check:links --strict-anchors --strict-broken --allowlist=…` run over built HTML; they will catch structural changes in the regenerated pages.
- Deploy is manual (`wrangler.jsonc` present, no deploy job, agents forbidden to deploy). Compare `dist/` HTML before/after on representative pages: `/docs/components/` (catalog), one record page with `<EvidenceTable>` and `<ComponentReferences>` previews, `/docs/claude/`.

## Step-by-step plan

Adapted from the official 7-step checklist (https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist).

1. **Wait for the presets.** Watch zudolab/zudo-doc#4477 and the 6.0.0 consumer guide (#4473); open an issue in Takazudo/zudo-circuit-doc titled "Port to zfb 3 / zudo-doc 6" with the island/hook census above. Nothing below runs before both ship.
2. **Baseline.** `git tag pre-zfb3` on a green `main`; keep `pnpm build && pnpm check:site` output (`doc/dist` HTML of the pages listed above) for the diff.
3. **Pins.** Edit `doc/package.json` (zfb family → 6.0.0's pinned zfb; zudo-doc + history-server → 6.0.0; zudo-circuit-doc → its v3 release; drop `preact`, `preact-render-to-string`), root `package.json:21`, then `pnpm install` (lockfile) and `pnpm --dir doc exec zfb --version`.
4. **Re-sync the scaffold files.** `pnpm create zudo-doc@6 <scratch>` and diff its `app/{pages/docs/[[...slug]].tsx,pages/index.tsx,tsconfig.json,src/styles/global.css,scripts/check-links.js}` against `doc/` (the `ZUDO_DEPS_PINS.md` procedure); copy, then re-apply `import "../lib/_circuit-doc-islands";` (stub) and `@import "@takazudo/zudo-circuit-doc/styles.css";` (global.css). Remove the `paths` aliases from `doc/tsconfig.json` if the new template still has them. Update `ZUDO_DEPS_PINS.md` `pinned:`/`updated:` lines.
5. **Config.** `pnpm --dir doc check` (`zfb check`) must load the config: a leftover `framework`/`tailwind` from a stale preset fails here with the named migration error.
6. **CSS.** `pnpm --dir doc build`; any remaining `@import "tailwindcss…"`/`@source`/`@theme` is listed as ZW009 with `file:line:column`. Then `pnpm --dir doc exec zfb wind audit --fail-on error`.
7. **Verify + ship.** `pnpm build && pnpm check:site`, browser-diff the pages in §5, commit `pnpm-lock.yaml` + pins + synced files in one PR; deploy manually as before.

## Verification checklist

- [ ] `pnpm --dir doc exec zfb --version` prints a 3.x version and `embedded esbuild` only.
- [ ] `pnpm check` green (`zfb check` loads the config; no `framework`/`tailwind` error).
- [ ] `pnpm build` green; `doc/dist` exists; no `ZW009` in the log; no `zfb warn:` about `@jsxImportSource` pragmas.
- [ ] `pnpm --dir doc exec zfb wind audit --fail-on error` exit 0.
- [ ] `pnpm check:site` green (built-reference scan, publication scope, strict anchors/links).
- [ ] Browser: catalog page, one record page (evidence table + footprint/model preview dialog open/close, focus return), `/docs/claude/`, theme toggle, search — compare against the `pre-zfb3` build; check lists, headings, `<details>` markers, `::placeholder` in search per the reset table.
- [ ] `git grep -n "preact" doc/` returns only comments (or nothing).
- [ ] `ZUDO_DEPS_PINS.md` `pinned:` lines match `doc/package.json`.

## Risks and open questions

- **Two upstreams, two timelines.** zudo-doc 6.0.0 cannot ship before the next zfb release (its blockers #3569/#3570 are fixed only on `main`), and zudo-circuit-doc has no v3 work yet. This host is last in that chain.
- **Token-name coupling.** `@takazudo/zudo-circuit-doc/styles.css` depends on 22 zudo-doc theme custom properties by name, including Tailwind's `--default-transition-duration`. If zudo-doc 6 renames them (`--zw-*` for wind tokens, `:root` properties for theme-only values), the previews/dialogs lose spacing and colours silently — no diagnostic covers authored `var()` references. Ask zudo-circuit-doc to pin against 6.0.0's emitted names.
- **Cascade flip.** Unlayered `zcd-*` rules vs utilities: zudo-doc accepted v3's utilities-after-authored cascade and fixes flips locally (DD5 in #4430; the upstream gap is Takazudo/zudo-front-builder#3386); only `placement: "before-authored"` (next zfb release) restores the old order.
- **`useModalDialog`** is a zudo-doc 5.x Preact hook consumed by zudo-circuit-doc. The epic ports it first as a shared zudo-react primitive (#4441 in #4430: "useModalDialog, island-types, Icons, tree-nav-shared and hydration-pending"; the zudo-doc guide records the ported form as `modalDialog(scope, options)`), so the dialog port should target that 6.0.0 export; the hook form disappears.
- **Generated pages are regenerated, not edited** (`CLAUDE.md`). If zudo-circuit-doc's v3 generator changes emitted MDX, `doc/src/content/docs/components/**` churns in the same PR; that is expected, not a regression.
- Unverified here: `zfb check` could not run on the clone (preset imports need `node_modules`), and the audit used a stand-in `wind: { spec: 1 }` config, so token-level results for package-owned classes will differ once the preset's manifest exists.

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/ (checklist, config, pragmas, md-wasm)
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ (directives → ZW009, reset differences, placement/tie flip)
- https://zfb.takazudomodular.com/zudo-wind/configuration/ , https://zfb.takazudomodular.com/zudo-wind/tokens/ , https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/ (for the zudo-circuit-doc port)
- https://zfb.takazudomodular.com/concepts/islands/ (boundary discovery; third-party widget pattern)
- https://zfb.takazudomodular.com/api/cli/ (`zfb wind audit|explain`; `--json`/`--severity`/`--plan` and `zfb wind manifest` are next zfb release)
- https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/ (no Breaking Changes sections exist in v2.21.0–v2.22.1, so 2.21.0 → 3.x has only the v3 breaks)
- zudolab/zudo-doc#4430 (epic), #4477 (root PR), #4473 (consumer migration guide, planned); Takazudo/zudo-front-builder#3569, #3570 (fixed on main, unreleased)
- https://github.com/Takazudo/zudo-circuit-doc (public; 0.2.0; peers zfb ^2.20.2 / zudo-doc ^5.27.0 / preact ^10.29.1; no v3 branch); its migration guide: `Takazudo--zudo-circuit-doc.md` (same directory; effort L)
- Sibling host with the identical `doc/` glue: `Takazudo--gh-zudo-synth-components.md`
- This repo: `ZUDO_DEPS_PINS.md`, `SCAFFOLD.md`, `doc/zfb.config.ts`, `doc/src/styles/global.css`, `doc/pages/docs/[[...slug]].tsx`, `.github/workflows/check.yml`, `scripts/checks/check-site-host.mjs`
