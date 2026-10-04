# zfb v3 migration guide: Takazudo/zudo-history-stash

Generated 2026-10-04 by an automated diagnosis of `main` @ `2be213a`. Counts come from the commands listed; re-run them locally before relying on them. The wind audit used the zfb **3.1.0** CLI (npm `latest` when it ran); zfb **3.2.0** was released at 2026-10-04 15:30 UTC while this diagnosis was being written, and the release references below reflect it (https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/).

## Verdict

**Blocked (upstream), effort M.** Only `doc/` is a zfb project; it is a create-zudo-doc 5.13.1 scaffold pinned to zfb 2.13.1 with an i18n (ja) lane, the Cloudflare adapter, a custom zfb plugin, and an 18-line `chrome-bindings.tsx` (two trivial MDX components). Host-side engine code is tiny: `zfb wind audit` finds zero host utility candidates and the hook census is zero. What makes this an M rather than an S is the **gate machinery that pins everything exactly**: `check:pin-parity` (zfb family and zudo-doc family in lockstep, `.zudo-doc.json.packageVersion` equal), `check:template-drift` (regenerates the scaffold with the installed `create-zudo-doc` and diffs), `check:wrangler-pin` (installed wrangler must equal the version embedded in the zfb binary), `ZUDO_DEPS_PINS.md`, and 13 b4push steps mirrored by `doc-checks.yml`/`doc-deploy.yml`/`doc-preview.yml`. Every one of them has to be re-baselined on 6.0.0. Nothing can start until `@takazudo/zudo-doc` 6.0.0 ships (5.x `zudoDoc()` emits `framework`/`tailwind`, which fail config loading on zfb 3, https://zfb.takazudomodular.com/guides/migrating-to-v3/#before-you-start). The React 19 + Tailwind code in `packages/ui` and `workers/viewer` is built by tsup/Vite, not zfb, and is unaffected.

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `doc/` (`zudo-history-stash-doc`) | `@takazudo/zfb` 2.13.1, `zfb-runtime` 2.13.1, `zfb-md-wasm` 2.13.1 (deps), `zfb-adapter-cloudflare` 2.13.1 (devDep), all exact | `@takazudo/zudo-doc` 5.13.1, `zudo-doc-history-server` 5.13.1, `create-zudo-doc` 5.13.1 (devDep); `@takazudo/zdtp` 0.4.12 **declared but unused** (`designTokenPanel` not set) | no `tailwindcss` dep; `global.css:8-9` Tailwind imports, `:20-22` 3 `@source`, `:26-28` empty `@theme` | `preact ^10.29.1`, `preact-render-to-string ^6.6.6`; host JSX: `src/chrome-bindings.tsx` (`ComponentChildren` type), `pages/docs/[[...slug]].tsx`, `pages/[locale]/docs/[[...slug]].tsx` (both with `@jsxImportSource preact` pragma) | none host-owned; package islands (docHistory, imageEnlarge, dynamicPageTransition, search, language-switcher) | transitively only | Cloudflare Workers via `adapter: "@takazudo/zfb-adapter-cloudflare"`, `doc/wrangler.toml` (`main = ./dist/_worker.js`, `nodejs_compat`, custom domain `zudo-history-stash.zudolab.dev`), `wrangler` 4.125.0 exact in root and `doc/` |
| `packages/ui` (`@takazudo/zudo-history-stash-ui`) | - | - | Tailwind v4 **via tsup `scripts/build-css.mjs`**, tokens example imports `tailwindcss/preflight`/`utilities` | React 19 peer (not Preact) | - | - | npm package; **unaffected by zfb 3** |
| `workers/viewer` | - | - | `@tailwindcss/vite ^4.2.0` + `tailwindcss ^4.2.0` in `vite.config.ts` | React 19 + react-router | - | - | `wrangler deploy`; **unaffected by zfb 3** |
| `workers/stash`, `workers/example-rpc-consumer`, `packages/core`, `packages/client` | - | - | - | - | - | - | not zfb projects |

Measured facts (clone root):

- `find . -name 'zfb.config.*' -not -path '*/node_modules*'` -> `doc/zfb.config.ts` only (120 lines).
- `doc/zfb.config.ts`: `zudoDoc({ base: DOC_BASE_PATH, trailingSlash: false, themePack: "drift", locales: { ja }, metaTags, llmsTxt, cjkFriendly, sidebar*, tocToggle, imageEnlarge, dynamicPageTransition, docHistory: true, versions: [], changelogs: CHANGELOGS (3 lanes -> packages/*/CHANGELOG.md), claudeResources: { claudeDir: "../.claude", projectRoot: ".", scanRoot: ".." }, defaultLocaleOnlyPrefixes, footer, headerNav x6, headerRightItems x4, adapter: "@takazudo/zfb-adapter-cloudflare" })`, then `config.plugins = [...(config.plugins ?? []), { name: "./plugins/normalize-generated-skill-links.mjs", options }]` (lines 106-118).
- Host source outside content: `src/chrome-bindings.tsx` (18 lines), `src/data/site-paths.ts`, `src/data/versions.ts` (imports six `package.json`/`openapi.json` files), `src/styles/global.css` (28 lines, byte-equal in shape to the create-zudo-doc base template), `plugins/normalize-generated-skill-links.mjs` (+ test), 23 `scripts/*.mjs` tooling files (checkers and their `*.test.mjs`) plus `scripts/check-links.js`, `examples-check/*.ts(x)` (React type-check fixtures for `packages/ui`, compiled with `examples-check/tsconfig.json` which extends the **repo-root** tsconfig and `types: ["react","react-dom"]`).
- Content: 55 EN + 55 JA pages. MDX components used: `<VersionValue name=.../>` (7 per locale, `reference/versions.mdx`), `<OpenApiLink>`, `<CategoryNav>` (16 files), `<CategoryTreeNav>`. `className=` outside code fences: 0 (React snippets in `reference/ui-package.mdx`, `guides/embedding-the-ui.mdx` are fenced code).
- Hook/dialect grep over `doc/src doc/pages doc/plugins doc/examples-check` (`useState|...|dangerouslySetInnerHTML|className=|onClick=|onInput=|<Island|"use client"`) -> 0 real hits (only the word `client` in type names).
- `grep -rn ZFB_TAILWIND .` -> 0.
- `zfb wind audit --project-root doc` (zfb 3.1.0, spec revision 3, temporary `{ wind: { spec: 1 } }`, restored; zfb 3.2.0 became npm `latest` later the same day, its revision-4 catalog and `ZW014` warnings cannot add findings to a host with zero candidates): `unrecognized classes: (none)`, `dead classes: (none)`, `dynamic constructions` = 2 (`docs;`/`locale-docs;` route-signature template literals in the two stubs), `tokens adjacent to interpolation` = 1 (`/` in `src/data/site-paths.ts` line 5, the `${normalizedBase}${path}` join; the audit prints byte offset `:70`, the file is 8 lines), 28 `auditInfo` diagnostics (ZW002/ZW005/ZW012) all in comment text or URL strings of `pages/*.tsx`, `src/chrome-bindings.tsx`, `src/data/*.ts`. No errors.
- `ZUDO_DEPS_PINS.md`: create-zudo-doc pinned at `7ca73f19...` (v5.13.1), updated 2026-08-29; notes zdtp stays 0.4.12 "until a create-zudo-doc release re-pins it" (create-zudo-doc 5.18.2 made zdtp conditional on `designTokenPanel`, so it can simply be dropped).
- `doc/.zudo-doc.json`: `{ "packageVersion": "5.13.1", "ejected": {} }` (checked by `check-pin-parity.mjs:73`).

## Sequencing and blockers

1. **Upstream gate:** `@takazudo/zudo-doc` 6.0.0 (zudolab/zudo-doc#4430 epic, #4477 root PR, #4473 consumer guide). The zfb side is done: zfb 3.2.0 (npm `latest`, published 2026-10-04 15:30 UTC, https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/) ships the fixes the integration floor waited for (Takazudo/zudo-front-builder#3569/#3570), and `@takazudo/zfb-adapter-cloudflare` 3.2.0 ships the dangling source-map fix (#3480, https://zfb.takazudomodular.com/changelog/zfb-adapter-cloudflare/v3.2.0/). Expect zudo-doc 6.0.0 to pin 3.2.0 or later.
2. **Now (recommended): step the 2.x/5.x pins to the last 2.x line** - `@takazudo/zfb*` 2.13.1 -> 2.22.1 and the zudo-doc family 5.13.1 -> 5.28.2 (`create-zudo-doc` too, `.zudo-doc.json` too). zfb's 2.x changelogs carry no `breaking` note after v2.0.0 (`grep -ci breaking docs/src/content/docs/changelog/zfb/v2.*.mdx`), and zudo-doc's CHANGELOG has no `Breaking Changes` heading between 5.8.0 and 5.28.2. This shortens the 6.0 diff (template drift, stubs) to the engine change alone. Two things to watch in that hop: create-zudo-doc 5.19.0 made the first positional a **destination path**, which is how `scripts/check-template-drift.mjs:17` (`["doc", "--preset", presetPath, "--no-install", "--no-git"]`) already uses it; and `check:wrangler-pin` will demand the wrangler version the 2.22.1 binary expects.
3. **Now:** drop `@takazudo/zdtp` from `doc/package.json:57` (unused; `designTokenPanel` is not enabled; generator stopped emitting it unconditionally in 5.18.2) and record it in `ZUDO_DEPS_PINS.md`.
4. **After 6.0.0:** regenerate the scaffold with `create-zudo-doc@6` through the existing template-drift flow, port `chrome-bindings.tsx` (section 3), re-baseline the pin gates, rebuild, redeploy.

## Required changes

### 1. Dependencies, config, tsconfig, env

- **`doc/package.json`**: zfb family (`@takazudo/zfb`, `zfb-runtime`, `zfb-md-wasm`, `zfb-adapter-cloudflare`) to the exact version zudo-doc 6.0.0 pins (exact `3.2.0` or later — a `^3.1.0`-style floor would still admit the 3.1.0 build-panic bug #3569, and this host pins exactly anyway); `@takazudo/zudo-doc`, `zudo-doc-history-server`, `create-zudo-doc` to 6.x exact (pin-parity requires exact versions, `tooling-utils.mjs:88`); remove `preact`, `preact-render-to-string`, `@takazudo/zdtp`. `wrangler` 4.125.0 must equal the version the new zfb binary expects (`check-wrangler-pin.mjs`) - also bump the root `package.json` copy, the check requires both. The lockfile is the root `pnpm-lock.yaml` (pnpm workspace; `doc/` has none of its own and a nested workspace is forbidden per `.template-drift-allowlist`), so the doc bump re-resolves inside the monorepo lockfile.
- **`doc/.zudo-doc.json`**: `packageVersion` -> the installed zudo-doc version (pin-parity).
- **`doc/zfb.config.ts`**: no `framework`/`tailwind`/`wind` key is authored by the host; nothing to delete. `adapter`, `locales`, `changelogs`, `versions`, `claudeResources`, `defaultLocaleOnlyPrefixes` are zudo-doc settings and survive as 6.0 documents them. Keep the `config.plugins` append (zfb's `plugins` array is additive and remains in v3, https://zfb.takazudomodular.com/api/define-config/). If host tokens are ever needed, the planned 6.0 `wind` override merges user-wins (https://zfb.takazudomodular.com/zudo-wind/configuration/#strict-validation-and-merging); today the audit shows none needed.
- **`doc/tsconfig.json`**: delete the `react`, `react/jsx-runtime`, `react-dom` paths (lines 8-10; line 7 is the `@/*` alias, keep it). `jsx`/`jsxImportSource` come from the 6.x `@takazudo/zudo-doc/tsconfig.base.json`; add `"jsx": "react-jsx", "jsxImportSource": "@takazudo/zfb/zudo-react"` locally if the shipped base does not (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands).
- **`doc/examples-check/tsconfig.json`** extends the repo-root tsconfig and types React: leave it alone; those files document `packages/ui` (React) and are neither rendered by zfb nor inside its scanned roots (`pages components layouts content src`).
- **Env**: no `ZFB_TAILWIND*` anywhere.
- **`ZUDO_DEPS_PINS.md`**: new create-zudo-doc commit/tag, note the engine change.

### 2. CSS and utilities

`doc/src/styles/global.css` is the unmodified 28-line template: `@layer zd-preflight, zd-flow;` (7), `@import "tailwindcss/preflight" layer(zd-preflight);` (8), `@import "tailwindcss/utilities";` (9), five `@import "@takazudo/zudo-doc/*.css"` (13-17), three `@source` (20-22), empty `@theme {}` (26-28). Lines 8, 9, 20-22, 26-28 are `ZW009` errors in v3, including under `wind: false` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives). Replace the file with the 6.x template output; there is no authored rule to carry over.

Reset and tokens are package decisions (planned `owned-v1` + preflight delta inside zudo-doc's `theme.css`). Verify the five reset-sensitive items on `/docs/reference/http-api/stashes` (tables, code), `/docs/guides/embedding-the-ui` (long code blocks) and the JA counterparts: headings, list markers, `code`/`pre` font stack, `sub`/`sup`, `::placeholder` in the search field, `[hidden]` (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#differences-from-tailwind-preflight).

Audit summary for this host: nothing to declare. The two `dynamic constructions` are the template's `routeSig: \`docs;${locale}\`` strings (not class lists); the 28 `auditInfo` lines are issue numbers and URLs inside comments (`#2651`, `https://...`) that the extractor reads as `/`-modified candidates. `auditInfo` never trips `--fail-on` (https://zfb.takazudomodular.com/api/cli/#zfb-wind).

### 3. Components and islands

**`doc/src/chrome-bindings.tsx`** (18 lines) is the only host component file:

```tsx
import { defineChromeBindings } from "@takazudo/zudo-doc/chrome-bindings";
import type { ComponentChildren } from "preact";           // line 2
function VersionValue({ name }: { name: VersionName }) { return <code>{projectVersions[name]}</code>; }
function OpenApiLink({ children }: { children: ComponentChildren }) { return <a href={OPENAPI_HREF}>{children}</a>; }
export const chromeBindings = defineChromeBindings({ mdxExtras: { OpenApiLink, VersionValue } });
```

Port: replace the `preact` type import with `import type { Child } from "@takazudo/zfb/zudo-react";` and type `children` as `Child` (`type Child = Scalar | Description | ReadonlySignal<Scalar> | readonly Child[]`, https://zfb.takazudomodular.com/zudo-react/api-reference/). Both components are static, prop-driven, hook-free and use `<code>`/`<a href>` - all inside the supported vocabulary - so no other change is needed (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/). `defineChromeBindings` and `mdxExtras` are zudo-doc APIs; confirm their 6.x signatures in zudolab/zudo-doc#4473.

**`pages/index.tsx`, `pages/docs/[[...slug]].tsx`, `pages/[locale]/docs/[[...slug]].tsx`**: generator-owned stubs (already allowlisted as "Prettier formatted" in `.template-drift-allowlist`). Take the 6.x versions through `check:template-drift`; do not hand-remove the pragmas. A leftover `/** @jsxImportSource preact */` fails the build once `preact` is uninstalled (`Could not resolve "preact/jsx-runtime"`); zfb prints a `zfb warn:` with the file, line and column of each pragma before that (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands).

**`plugins/normalize-generated-skill-links.mjs`**: a `definePlugin` from `@takazudo/zfb/plugins` that rewrites links in generated `claude-skills` MDX with `node:fs`; it touches neither CSS nor JSX. Keep; re-run its test (`doc/plugins/normalize-generated-skill-links.test.mjs`) on the new pin.

**MDX**: `<VersionValue>`, `<OpenApiLink>`, `<CategoryNav>`, `<CategoryTreeNav>` are component tags with string props - fine. Two content rules zudo-doc's 6.0 planning measured as renderer errors on zfb 3.0/3.1 - ordered lists that start at a number other than 1 (`ol[start]`, Takazudo/zudo-front-builder#3360) and inline raw `<svg xmlns=...>` (#3359) - are accepted since zfb 3.2.0 (`ol` takes a numeric `start`, the namespace begins at `svg`; https://zfb.takazudomodular.com/zudo-react/components-and-jsx/). Only if the adopted pin is older than 3.2.0 run `grep -rEn '^([2-9]|[1-9][0-9])\. ' doc/src/content` and `grep -rn '<svg' doc/src/content` to find candidates.

### 4. md-wasm and other packages

- `@takazudo/zfb-md-wasm` is not called by repo code; it is listed because the scaffold lists it. Follow the 6.x peer list (https://zfb.takazudomodular.com/guides/migrating-to-v3/#mdx-and-md-wasm).
- `packages/ui`'s Tailwind (`scripts/build-css.mjs`, `styles/tokens.example.css` with `@import "tailwindcss/preflight" layer(base)`) and `workers/viewer`'s `@tailwindcss/vite` are **outside zfb** and keep working on zfb 3; do not migrate them as part of this.
- The `design-token-lint` dogfood (`lint:tokens`) runs on `packages/ui` and `workers/viewer` only; unaffected.

### 5. Tests, CI, deploy

- `doc/scripts/run-b4push.sh` (13 steps: build:libs, format:md, template-drift, pin-parity, wrangler-pin, contract, versions, examples, locale-parity, `zfb check`, build + locale routes, html, links) is invoked by `doc-checks.yml` (PR), `doc-deploy.yml` (push `main`) and `doc-preview.yml` (PR preview). Add `pnpm exec zfb wind audit --fail-on error` after `zfb check` (official checklist step 6) and extend `scripts/check-b4push-ci-parity.mjs` accordingly.
- `check-template-drift.mjs` regenerates with `create-zudo-doc doc --preset setup-preset.json --no-install --no-git` and compares against `.template-drift-allowlist` (13 entries plus a header comment). On 6.x expect new diffs in `global.css`, `tsconfig.json` and both stubs until you adopt the regenerated files; keep the allowlist reasons honest.
- `check-wrangler-pin.mjs` asserts exact equality with the wrangler version embedded in the zfb binary. In v3, `zfb preview` treats its baseline as a **minimum** (older aborts, equal or newer proceeds, https://zfb.takazudomodular.com/api/cli/#zfb-preview); the exact-equality guard stays valid but becomes stricter than zfb itself. Decide whether to keep exactness.
- `doc-deploy.yml:66-67` runs `pnpm exec wrangler deploy --dry-run --outdir .wrangler/dry-run` before deploying `dist/_worker.js`; re-check the adapter's emitted file names and `compatibility_date = "2026-08-25"` against the 3.x adapter docs after the first build. The Cloudflare source-map fix (#3480) shipped in `@takazudo/zfb-adapter-cloudflare` 3.2.0 (it strips the trailing external `sourceMappingURL` comment from `_zfb_inner.mjs`); pin 3.2.0 or later.
- `doc-history-generate` (doc-deploy.yml:60, doc-preview.yml:94) comes from `@takazudo/zudo-doc-history-server`; bump in lockstep.
- Root `ci.yml` builds libraries, viewer and runs Playwright for the viewer; it never installs zfb's doc and needs no change.

## Step-by-step plan

1. **Now** - 2.x/5.x catch-up: in `doc/package.json` set the four `@takazudo/zfb*` to `2.22.1`, the three zudo-doc family packages to `5.28.2`, `.zudo-doc.json.packageVersion` to `5.28.2`, drop `@takazudo/zdtp`; `pnpm install`; `pnpm --filter zudo-history-stash-doc check:wrangler-pin` and move `wrangler` (root + doc) to whatever it reports; `pnpm b4push:doc`; update `ZUDO_DEPS_PINS.md`; re-baseline `.template-drift-allowlist` if `check:template-drift` reports template changes. Commit on a branch, let `doc-checks.yml`/`doc-preview.yml` run.
2. **Wait for `@takazudo/zudo-doc@6.0.0`**; read zudolab/zudo-doc#4473 and note the zfb pin it requires.
3. Bump: zfb family to that pin (exact, including `zfb-adapter-cloudflare`), zudo-doc family + `create-zudo-doc` to 6.x, `.zudo-doc.json`, remove `preact`/`preact-render-to-string`; `pnpm install`; `pnpm exec zfb --version` (3.x and `embedded esbuild`, no Tailwind line).
4. `pnpm --filter zudo-history-stash-doc check:template-drift`; adopt the regenerated `src/styles/global.css`, `tsconfig.json`, `pages/index.tsx`, both doc stubs; re-apply only the allowlisted adaptations.
5. Port `src/chrome-bindings.tsx` (section 3). `pnpm --filter zudo-history-stash-doc check` and fix type errors.
6. `pnpm --filter zudo-history-stash-doc exec zfb wind audit --fail-on error` (expect empty), `pnpm b4push:doc` (build, html-validate, links, locale parity, versions wiring).
7. `cd doc && pnpm exec wrangler deploy --dry-run --outdir .wrangler/dry-run`; then `pnpm dev` and compare EN + JA pages, the language switcher, doc-history button, search, and the generated Claude skill pages (the plugin's output) in a browser.
8. Merge; `doc-deploy.yml` deploys; confirm `https://zudo-history-stash.zudolab.dev/` and `/ja/docs/getting-started`.

## Verification checklist

- [ ] `grep -rnE '@import\s+"tailwindcss|@source|@theme' doc/src` -> 0.
- [ ] `grep -rn 'preact\|@jsxImportSource' doc/package.json doc/tsconfig.json doc/pages doc/src` -> 0.
- [ ] `check:pin-parity`, `check:wrangler-pin`, `check:template-drift` pass with an allowlist that still explains every entry.
- [ ] `zfb check` and `zfb wind audit --fail-on error` pass; `pnpm b4push:doc` green.
- [ ] `doc/plugins/normalize-generated-skill-links.test.mjs` passes; generated `claude-skills` pages link to GitHub as before.
- [ ] `examples-check` type-check still passes (`pnpm --filter zudo-history-stash-doc check:examples`).
- [ ] Browser: EN and JA page chrome, theme toggle, search, language switcher, doc-history, `reference/versions` values render (`VersionValue`), OpenAPI link resolves to `/openapi.json`.
- [ ] `wrangler deploy --dry-run` succeeds; production deploy green; `check:locale-routes` passes on `dist`.

## Risks and open questions

- **Preset ordering**: never install `@takazudo/zfb@3` with zudo-doc 5.x; config loading fails with the `framework`/`tailwind` migration errors.
- **Exact-pin gates multiply the work**: four pin scripts and the drift allowlist all move together; do the 2.x catch-up first so the 6.0 change set is the engine only.
- **Adapter output names**: `wrangler.toml` points at `dist/_worker.js`; verify the 3.x adapter still emits that path before trusting `--dry-run`.
- **Plugin host**: the custom plugin runs inside zfb's plugin host; zudo-doc's planning notes list an open upstream report about plugin-host init timeouts (Takazudo/zudo-front-builder#3127). Watch the first `zfb build` time on CI.
- **Content rules**: restarted ordered lists and raw inline SVG were renderer errors on zfb 3.0/3.1 and are accepted on 3.2.0; only a build on the adopted pin proves the content.
- **Not run here**: `zfb check`, template drift, any build or deploy. Measured: audit, greps, file reads.

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ and https://zfb.takazudomodular.com/zudo-wind/configuration/
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/, https://zfb.takazudomodular.com/zudo-react/components-and-jsx/, https://zfb.takazudomodular.com/zudo-react/api-reference/
- https://zfb.takazudomodular.com/api/cli/ (`zfb wind`, `zfb preview` wrangler baseline), https://zfb.takazudomodular.com/api/define-config/
- https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/, https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/, https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/ (npm `latest` since 2026-10-04), https://zfb.takazudomodular.com/changelog/zfb-adapter-cloudflare/v3.2.0/
- zudolab/zudo-doc#4430, #4477, #4473; Takazudo/zudo-front-builder#3480, #3569, #3570
- Repo files: `doc/zfb.config.ts`, `doc/package.json`, `doc/.zudo-doc.json`, `doc/tsconfig.json`, `doc/src/styles/global.css`, `doc/src/chrome-bindings.tsx`, `doc/pages/**`, `doc/plugins/normalize-generated-skill-links.mjs`, `doc/scripts/{check-pin-parity,check-wrangler-pin,check-template-drift}.mjs`, `doc/scripts/run-b4push.sh`, `doc/.template-drift-allowlist`, `ZUDO_DEPS_PINS.md`, `.github/workflows/doc-{checks,deploy,preview}.yml`

## Appendix: commands behind the counts

```sh
find . -name 'zfb.config.*' -not -path '*/node_modules*'                        # doc/zfb.config.ts
find doc/src/content/docs -name '*.md*' | wc -l; find doc/src/content/docs-ja -name '*.md*' | wc -l   # 55 / 55
grep -rnE '@import\s+"tailwindcss|@tailwind|@theme|@apply|@source|@utility|@variant|@custom-variant|@plugin|@config|theme\(' \
  --include=*.css --include=*.ts --include=*.tsx --include=*.mjs doc           # 7 lines, all global.css
grep -rnE "from ['\"]preact|preact/hooks|@preact/signals|preact-render-to-string|@jsxImportSource" \
  --include=*.ts --include=*.tsx --include=*.js --include=*.jsx --include=*.mdx doc   # 5 lines: chrome-bindings + 2 stubs
grep -rnE 'useState|useEffect|useRef|useMemo|useCallback|useContext|createContext|dangerouslySetInnerHTML|className=|onClick=|onInput=|<Island|"use client"' \
  --include=*.ts --include=*.tsx doc/src doc/pages doc/plugins doc/examples-check | grep -v src/content   # 0 real hits
grep -rn ZFB_TAILWIND . --exclude-dir=node_modules                                # 0
# audit (config swapped to `{ wind: { spec: 1 } }`, restored; `git status --short` empty)
zfb wind audit --project-root doc
```

## Appendix: host-owned files and their 6.0-era shape (expected, verify against the shipped template)

| File | Today (5.13.1 scaffold) | Expected after `create-zudo-doc@6` | Who owns the change |
| --- | --- | --- | --- |
| `doc/src/styles/global.css` (28 lines) | `@layer` + 2 Tailwind imports + 5 package imports + 3 `@source` + empty `@theme` | package imports only; override slot as `:root {}` custom properties or `wind.tokens`; no Tailwind imports, `@source`, `@theme` | template (adopt via `check:template-drift`) |
| `doc/tsconfig.json` | extends `@takazudo/zudo-doc/tsconfig.base.json` + 3 `react*` -> `preact` paths | same extends; no `react*` paths; base supplies `jsxImportSource: "@takazudo/zfb/zudo-react"` | template |
| `doc/pages/docs/[[...slug]].tsx`, `doc/pages/[locale]/docs/[[...slug]].tsx` | `@jsxImportSource preact` pragma, `import type { JSX } from "preact"`, `virtual:zudo-doc-*` imports | no pragma, zudo-react types, same route-context seams | template (already allowlisted as formatted copies) |
| `doc/src/chrome-bindings.tsx` (18 lines) | `ComponentChildren` from `preact` | `Child` from `@takazudo/zfb/zudo-react` | **host** (only hand-port in the repo) |
| `doc/package.json` | zfb 2.13.1 x4, zudo-doc 5.13.1 x3, preact pair, zdtp 0.4.12, wrangler 4.125.0 | zfb 3.x x4 (lockstep), zudo-doc 6.x x3, no preact pair, no zdtp, wrangler = zfb's expected version | host + pin scripts |
| `doc/.zudo-doc.json`, `ZUDO_DEPS_PINS.md`, `doc/.template-drift-allowlist` | 5.13.1 provenance | 6.x provenance; allowlist entries re-justified | host gates |
