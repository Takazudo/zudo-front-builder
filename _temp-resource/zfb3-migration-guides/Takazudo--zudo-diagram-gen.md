# zfb v3 migration guide: Takazudo/zudo-diagram-gen

Generated 2026-10-04 by an automated diagnosis of `main` @ `2d73f90` (2026-09-27, "Merge pull request #57"). Counts come from the commands listed; re-run them locally before relying on them. Reviewed the same day against zfb **3.2.0**, which npm published as `latest` at 15:30 UTC on 2026-10-04 (the audit was first run on 3.1.0 and repeated on 3.2.0). The clone was left untouched (`git status --short` empty after the audit's temporary config swap).

## Verdict

**Blocked (root site) / ready (package + initializer). Effort M.** The repo has exactly one zfb project dir, the root, and it is a zudo-doc 5.27.0 host: `zfb.config.ts:4-5` is `defineConfig(zudoDoc({...}))`, and zudo-doc 5.x's preset itself supplies `framework: "preact"` and `tailwind: { enabled: true }` (zudo-doc `packages/zudo-doc/src/config.ts:911-913` at 5.28.2), so the root cannot load on zfb 3 until zudo-doc 6.0.0 ships (epic zudolab/zudo-doc#4430, root PR zudolab/zudo-doc#4477 — still a draft on 2026-10-04 17:49 UTC; consumer guide zudolab/zudo-doc#4473; npm `latest` is still 5.28.2). zfb itself no longer gates that release: the two zfb bugs zudo-doc 6 was waiting for (Takazudo/zudo-front-builder#3569, #3570) shipped in zfb 3.2.0 on 2026-10-04. The host-owned surface that must change at cutover is small: an 11-line entry sheet, a 48-line Preact-hooks island, one `import type { JSX } from 'preact'`, eight `className=` lines in a generator script, `tsconfig.json`, and seven dependency lines in `package.json` (four bumps, three removals). The other half is not blocked: `packages/diagram-gen` never imports zfb as a library (it resolves the `zfb` binary from the session host, `src/runner.mjs:62-69`) and only emits a four-line Preact page template (`src/render.mjs:24`); together with `packages/create-zudo-diagram-gen`'s pins (`src/index.mjs:161-166`) it can move to zfb 3.2.0 (current npm `latest`) now, independent of zudo-doc, and CI already exercises exactly that path (`.github/workflows/ci.yml:43-58`). M rather than S because the two halves must land in a fixed order, the engine peer flips a major while the root stays on 2.x for a while, and the real-browser proof (`scripts/check-site-browser.mjs`) depends on zudo-doc 6 internals. No migration branch, PR or issue exists yet (REST: 9 open PRs, all dependabot; 0 open issues; remote heads are `main` + `dependabot/*`).

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `/` (root zudo-doc site; `zfb.config.ts`) | `@takazudo/zfb`, `zfb-md-wasm`, `zfb-runtime` **2.21.1** exact (`package.json:32-34`) | `@takazudo/zudo-doc` **5.27.0** (`package.json:36`); no zudo-sg; zdtp disabled (`designTokenPanel: false`, `zfb.config.ts:19`) | `tailwindcss` 4.2.0 (`package.json:42`); 4 directives in `src/styles/global.css` (L2, L3, L10, L11); no `tailwind.config.*`; no `ZFB_TAILWIND*` anywhere | `preact` 10.29.2 exact + `preact-render-to-string` 6.6.6 (`package.json:40-41`); `tsconfig.json:7` `jsxImportSource: "preact"`; 3 hook calls in one file; 1 type import | 1 island: `DiagramWorkbench`, `Island({ when: 'visible', ... })` via zudo-doc `mdxExtras` (`src/chrome-bindings.tsx:7-8`) | dependency only (zudo-doc peer); 0 `compile(`/`renderHtml(`/`jsxRuntime` in repo code | Cloudflare Worker, assets-only + redirect (`wrangler.jsonc`, `worker/index.mjs`); `deploy.yml` runs after green CI on `main`; live at zudo-diagram-gen.zudolab.dev |
| `packages/diagram-gen` (`@takazudo/zudo-diagram-gen` 0.1.0, unpublished; **no `zfb.config`**) | peer `^2.21.1` (`package.json:43`) used only to locate `bin/zfb.mjs` in the *consumer* host (`src/runner.mjs:62-65`) | none | none: `client/app.css` is 2,538 lines of authored CSS, 113 `.dg-*` selectors, 0 directives | peer `^10.29.1` (`package.json:44`); generated route template carries `/** @jsxImportSource preact */`, `charSet`, `dangerouslySetInnerHTML` (`src/render.mjs:24`) | none; `client/mount.mjs` is dependency-free vanilla JS (1,198 lines) | none | not published; packed by `pnpm pack:local` and installed in CI |
| `packages/create-zudo-diagram-gen` (0.1.0; writes a session host at run time) | pins `@takazudo/zfb` + `@takazudo/zfb-runtime` **2.21.1** into generated `package.json` (`src/index.mjs:161-162`) | none | none: generated `zfb.config.ts` is `defineConfig({})` (`src/index.mjs:169`) | pins `hono` 4.13.9 + `preact` 10.29.2 + `preact-render-to-string` 6.6.6 (`src/index.mjs:163-165`); asserted in `test/initializer.test.mjs:36-41` | none | none | local only; CI "Build an installed consumer" (`ci.yml:43-58`) + preview + Chromium |

Audit facts (zfb 3.1.0, root dir, temporary `wind: { spec: 1 }` config): `outcome: complete`; unrecognized classes 0; dead classes 0; conflicts 0; 1 dynamic construction; 14 `auditInfo` diagnostics. All 14 are import specifiers or the `` `docs;${locale}` `` route-signature string, not utilities: 11 ZW005 "invalid slash modifier" at `'@takazudo/zudo-doc/...'`, `'preact/hooks'`, `'../../src/chrome-bindings'`, `'@takazudo/zudo-diagram-gen/client/...'`; 2 ZW002 at `'virtual:zudo-doc-route-context'` / `'virtual:zudo-doc-chrome-bindings'`; 1 ZW012 at `pages/docs/[[...slug]].tsx` (`routeSig`). The tracked host source contains **zero** utility candidates. Note: 3.1.0 prints byte offsets (`[[...slug]].tsx:952` in a 33-line file). Re-run on **3.2.0** (same temporary config): `spec: 1 revision 4`, `outcome: complete`, 0 unrecognized / 0 dead / 0 conflicts and exactly **one** diagnostic — `ZW012 auditInfo at default/pages:docs/[[...slug]].tsx:19:56` (the `routeSig` template string). 3.2.0 skips module specifiers during extraction and prints `file:line:col`, so the 13 import-specifier findings disappear (#3370, https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/). `zfb check` was not run (preset import needs `node_modules`).

The only utilities the host owns live in `scripts/build-showcase.mjs:37-59`, which writes gitignored MDX (`src/content/docs/examples/index.mdx`): 28 unique classes. `zfb wind explain` under the empty config: 12 resolve (`block`, `grid`, `grid-cols-1`, `inline-flex`, `items-start`, `shrink-0`, `min-w-0`, `h-auto`, `w-full`, `max-w-full`, `hover:underline`, `focus-visible:underline`); 14 are "recognized invalid" for missing tokens (`mt-vsp-sm|2xs|lg`, `gap-x-hsp-xs|lg`, `gap-y-vsp-lg`, `w-hsp-lg` → spacing tokens; `text-fg`, `text-muted`, `hover:text-accent`, `focus-visible:text-accent` → colors; `text-small`, `text-caption` → fontSizes; `font-medium` → fontWeights); 2 are ZW002 for unconfigured breakpoints (`sm:grid-cols-2`, `lg:grid-cols-3`). Every one of those tokens is a zudo-doc theme token, so they are zudo-doc 6's to declare, not this repo's.

## Sequencing and blockers

1. **Now, independent of zudo-doc: move `packages/diagram-gen` + `packages/create-zudo-diagram-gen` to zfb 3.2.0 together** (section 4). The generated session host has no zudo-doc, no Tailwind and no islands; its only JSX is the template in `src/render.mjs:24`. CI's packed-consumer step (`ci.yml:43-58`, then preview `ci.yml:59-75` and Chromium `ci.yml:76-81`) is the existing proof. Do not flip one without the other: a zudo-react template under a 2.21.1 pin, or a 3.x pin under the Preact template, both break the consumer build.
2. **Now, safe on 2.21.1 and v3-ready: host prep** (section 2 and 3): `className=` → `class=` in `scripts/build-showcase.mjs` (Preact accepts `class`; keep the `<svg xmlns=…>` as is — see section 2); drop the `: JSX.Element` annotation in `pages/docs/[[...slug]].tsx:28`; close or let dependabot PRs #60 (preact 10.29.8) and #61 (preact-render-to-string 6.7.0) be superseded; open the tracking issue and decide the branch (recommend `base/zfb3-migration` with `topic/*` children, the repo's existing `base/**` CI trigger already covers it: `ci.yml:6`, `security.yml:6`).
3. **Blocked until zudo-doc 6.0.0 ships: the root cutover** (sections 1–3, 5). Upstream: zudolab/zudo-doc#4430 (epic), #4477 (root PR, draft), #4473 (5.x → 6.0 consumer guide). The zfb fixes zudo-doc 6 was waiting for (Takazudo/zudo-front-builder#3569 UTF-8 extractor offsets, #3570 quoted island markers in highlighted code) shipped in **zfb 3.2.0 on 2026-10-04** (https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/ "Bug Fixes"), so zfb no longer gates zudo-doc 6; the root will pin whatever 6.0.0 declares as its `@takazudo/zfb` peer, which can be no lower than 3.2.0. 2.21.1 → 2.22.x in between carries no breaking notes; 2.22.1 switched dev live-reload to WebSockets and made dev boot lazy (https://zfb.takazudomodular.com/changelog/zfb/v2.22.1/), which only matters if a dev reverse proxy is in front of `pnpm dev`.
4. What to expect from zudo-doc 6 (from its planning resources on the `base/zfb3-migration` branch of zudolab/zudo-doc — `_temp-resource/4430-zfb3-migration/explore/pkg-build.md` and `css-wind.md`; they are not on `main`; **planned, not released**): the preset supplies `wind` (tokens translated from `theme.css`'s `@theme` blocks, a `manifests: { "zudo-doc": { path: "@takazudo/zudo-doc/wind.json" } }` entry, reset `owned-v1` plus an authored preflight delta); `./safelist.css` is expected to disappear from the exports map (a `@source inline(...)` file is ZW009 under v3); `theme.css` stays importable; the `zd-preflight` layer is removed from the host template (`css-wind.md`, item W3); `zudoDoc({ wind })` is the host override slot if this site ever needs its own tokens; `chromeBindingsModule` stays a `zudoDoc()` setting, but the notes never mention `defineChromeBindings` / `mdxExtras`, so treat their 6.0 signatures as unverified. Re-read #4473 when it exists before editing `src/styles/global.css`.

## Required changes

### 1. Dependencies, config, tsconfig, env

- **`package.json:32-34, 36, 40-42`** (at cutover): bump `@takazudo/zfb`, `@takazudo/zfb-md-wasm`, `@takazudo/zfb-runtime` to the zfb version zudo-doc 6.0.0 requires (exact pins, per `AGENTS.md:22`); bump `@takazudo/zudo-doc` to `6.0.0`; remove `tailwindcss`; remove `preact-render-to-string` unconditionally — it is a zfb 2.x Preact-SSR host requirement, not a zudo-doc peer (5.28.2's `peerDependencies`: `@takazudo/zfb`, `@takazudo/zfb-runtime`, `preact ^10.29.1`, `zod`, plus *optional* `@takazudo/zdtp`, `@takazudo/zfb-md-wasm`, `@takazudo/zudo-doc-history-server`, `diff`, `katex`; the 5.27.0 copy is in `pnpm-lock.yaml:1044-1064`); remove `preact` unless zudo-doc 6 still lists it as a peer (the briefing says 6.0 keeps zdtp as an opaque Preact bundle, but `designTokenPanel: false` here and zdtp is an optional peer, so pnpm installs nothing for it). `hono` (`package.json:38`) is not a zudo-doc peer either: it exists for zfb 2.x's server entry (`packages/create-zudo-diagram-gen/README.md:52`), and `@takazudo/zfb-runtime` 3.1.0 and 3.2.0 both declare `hono ^4.12.25` as their own dependency (read from the published `package.json`), so the root pin is probably removable at cutover — verify as in section 4. Keep `diff`, `katex`, `zod`, `@takazudo/zfb-md-wasm` only while zudo-doc 6 declares them as peers. Why: https://zfb.takazudomodular.com/guides/migrating-to-v3/ ("Components and islands": remove `preact`/`react` deps present only for zfb's old engines).
- **`zfb.config.ts`**: no `framework`/`tailwind` keys to delete; the removed keys come from the preset, which is why the preset upgrade is step 1 of the official checklist. Add a `wind` override only if zudo-doc 6's guide asks for host tokens: `zudoDoc({ ..., wind: { tokens: { ... } } })` (shape per https://zfb.takazudomodular.com/zudo-wind/configuration/; preset and host `wind` objects merge recursively, host wins).
- **`tsconfig.json:7`**: `"jsxImportSource": "preact"` → `"@takazudo/zfb/zudo-react"` (keep `"jsx": "react-jsx"`, `tsconfig.json:6`). The `paths` entry for `zfb` (`tsconfig.json:15`) can stay; the repo imports the scoped `@takazudo/zfb/config`, so no `zfb/config` shim is needed (https://zfb.takazudomodular.com/api/define-config/#typing-the-zfbconfig-import).
- **`pnpm-workspace.yaml:3-5`**: `onlyBuiltDependencies` keeps `@takazudo/zfb` (platform binary) — unchanged.
- **Env**: nothing to remove; `grep -rn ZFB_TAILWIND .github scripts package.json` is empty.

### 2. CSS and utilities

- **`src/styles/global.css`** (11 lines) — the whole file is rewritten at cutover; lines 2, 3, 10 and 11 are ZW009 errors under v3; line 5 trips ZW009 inside the sheet it imports (5.28.2's `dist/safelist.css` is one `@source inline("…")` directive, `packages/zudo-doc/scripts/gen-safelist.mjs:5`, and the scanner checks every imported stylesheet); line 1 is an ordinary `@layer` statement and stays valid CSS (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#directives, https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/#leftover-directives-and-css-command):

  | Line | Today | v3 |
  | --- | --- | --- |
  | 1 | `@layer zd-preflight, zd-flow;` | valid CSS under v3 as-is; follow zudo-doc 6's template (the `zd-preflight` layer goes away with the vendored preflight, `css-wind.md` W3) |
  | 2 | `@import 'tailwindcss/preflight' layer(zd-preflight);` | delete; reset comes from the preset's `wind.reset` |
  | 3 | `@import 'tailwindcss/utilities';` | delete |
  | 5 | `@import '@takazudo/zudo-doc/safelist.css';` | delete; the 5.x file is a single `@source inline(…)` directive (ZW009 once imported) and zudo-doc 6 plans to retire the export in favour of a `wind.json` manifest |
  | 4, 6–8 | `theme.css`, `content.css`, `page-loading.css`, `features.css` imports | keep unless #4473 says otherwise (package `exports` subpath imports resolve on 3.1.0: https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/) |
  | 9 | `@import '@takazudo/zudo-diagram-gen/client/app.css';` | keep (authored CSS; also imported by the island, see 3 — keep one of the two) |
  | 10–11 | `@source '../content/**/*.{mdx,md}'; @source '../../pages/**/*.tsx';` | delete; zfb's source plan owns the scan roots — this repo's 3.1.0 audit scanned `pages/` and `src/` (owner labels `default/pages`, `default/src`) (https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/) |

- **`scripts/build-showcase.mjs:37-59`** — the only host-owned utilities. Do now: `className=` → `class=` on all 8 occurrences (React spellings fail validation on zudo-react: https://zfb.takazudomodular.com/zudo-react/components-and-jsx/; Preact accepts `class`, so this is safe on 2.21.1). Keep the inline `<svg xmlns="http://www.w3.org/2000/svg" viewBox fill aria-hidden>` at L41 unchanged: `xmlns`, `viewBox` and `fill` are in zudo-react's SVG attribute vocabulary since **3.2.0** (changelog v3.2.0 "Expand the finite HTML and SVG markup vocabulary", #3359; https://zfb.takazudomodular.com/zudo-react/components-and-jsx/ lists the finite SVG subset), and ARIA attributes are accepted everywhere. Only 3.1.0 rejects `xmlns` by name, and the root never targets 3.1.0 (it pins what zudo-doc 6 requires, ≥ 3.2.0). The 28 classes themselves need no change: the 16 token-bearing ones (`*-vsp-*`, `*-hsp-*`, `text-fg|muted|accent`, `text-small|caption`, `font-medium`, `sm:`/`lg:`) resolve once zudo-doc 6's preset supplies its tokens and breakpoints; if any stays ZW006/ZW002 after the upgrade, add the missing token in a `zudoDoc({ wind: { tokens } })` override rather than renaming classes. Add `wind.authoredClasses` only for a name CSS owns that collides with a utility root — none found.
- **`packages/diagram-gen/client/app.css`** needs nothing: 0 directives, all selectors scoped under `.dg-app`, own font stacks (L33-35), own `:where()` margin/heading resets (L104-109) and `box-sizing: border-box` on controls (L127). It is already proven under *no* reset by the standalone `export-html` path, so the preset's reset choice does not affect the workbench. Utility placement flip (`after-authored`, https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties) is irrelevant here: no utility class is applied to any `.dg-*` element.

### 3. Components and islands

- **`src/components/diagram-workbench.tsx`** (48 lines; `useRef` L11, `useState` L12, `useEffect` L14-37; conditional `&&` rendering L41-44). Rewrite as a setup-once zudo-react island; the vanilla `mountDiagramApp` call is exactly the "Embedding a third-party widget" shape (https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget): an empty `ref` host, work started in `onActivate`, synchronous cleanup returned, abort check after each `await`.

  ```tsx
  'use client';

  import { computed, getScope, Show, signal, type Ref } from '@takazudo/zfb/zudo-react';
  import { mountDiagramApp } from '@takazudo/zudo-diagram-gen/client/mount';
  import '@takazudo/zudo-diagram-gen/client/app.css';
  import type { GalleryData } from '@takazudo/zudo-diagram-gen';

  type Status = 'loading' | 'ready' | 'error';

  export default function DiagramWorkbench({ dataUrl }: { dataUrl: string }) {
    const root: Ref<HTMLDivElement> = { current: null };
    const status = signal<Status>('loading');
    const scope = getScope();
    const loading = computed(() => status.value === 'loading');
    const failed = computed(() => status.value === 'error');

    scope.onActivate(() => {
      const abortSignal = scope.abortSignal;
      let dispose: (() => void) | undefined;
      void (async () => {
        try {
          const response = await fetch(dataUrl, { signal: abortSignal });
          if (!response.ok) throw new Error(`HTTP ${response.status}`);
          const data = (await response.json()) as GalleryData;
          if (abortSignal.aborted || !root.current) return;
          dispose = mountDiagramApp(root.current, data, { embedded: true });
          status.value = 'ready';
        } catch {
          if (!abortSignal.aborted) status.value = 'error';
        }
      })();
      return () => { dispose?.(); };
    });

    return (
      <div>
        <Show when={loading}>{() => <p role="status">Loading diagram workbench…</p>}</Show>
        <Show when={failed}>{() => <p role="alert">The diagram workbench could not load. Please reload the page.</p>}</Show>
        <div ref={root} />
      </div>
    );
  }
  ```

  Mapping, per https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/: `useRef` → plain `Ref` object (assigned before activation); `useState` → `signal`; `useEffect([], cleanup)` → `getScope().onActivate(() => cleanup)` (must stay synchronous — start the async work inside, never `onActivate(async ...)`, https://zfb.takazudomodular.com/zudo-react/scopes-and-lifecycle/); `controller.abort()` → `scope.abortSignal`; `{cond && <p/>}` → `<Show when={computed}>` owned regions (https://zfb.takazudomodular.com/zudo-react/conditionals-and-lists/). The `[dataUrl]` dependency is gone on purpose: `dataUrl` is a static MDX attribute and island props are setup-time data (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/#island-props). `mountDiagramApp` only ever mutates its own childless host `<div>`, which satisfies the widget rule "do not let it mutate a region containing zudo-react children". The CSS side-import on L5 duplicates `global.css:9`; keep one.
- **`src/chrome-bindings.tsx:1, 7-8`**: `import { Island } from '@takazudo/zfb'` and the direct call `Island({ when: 'visible', children: <DiagramWorkbench dataUrl={dataUrl} /> })` stay valid: `Island` is still exported from `@takazudo/zfb`, `IslandProps.when` still accepts `"visible"`, and the scanner recognizes direct calls with a `children` attribute (https://zfb.takazudomodular.com/concepts/islands/#boundary-discovery-and-migration, https://zfb.takazudomodular.com/api/island/). Props are one JSON string — fine. `defineChromeBindings`/`mdxExtras` is zudo-doc API; re-check its 6.0 signature.
- **`pages/docs/[[...slug]].tsx:2, 28`**: `import type { JSX } from 'preact'` must go. Do now: drop the `: JSX.Element` return annotation and the import (TypeScript infers the return type of `renderDocPage`). If an explicit type is wanted later, zudo-react's public element type is `Description` / `Child` from `@takazudo/zfb/zudo-react` (https://zfb.takazudomodular.com/zudo-react/api-reference/). Lines 1 and 3-11 (`virtual:zudo-doc-*`, `createRouteContext`, `createChrome`) are zudo-doc API — follow #4473.
- **`pages/index.tsx`**: a one-line re-export of `@takazudo/zudo-doc/routes/index` — no change beyond the preset.
- **MDX**: 23 committed pages use only `<CategoryNav>` ×7 (zudo-doc's) and `<DiagramWorkbench>` ×1 (`src/content/docs/workbench/index.mdx`); 0 `className`, 0 imports. Nothing to do.
- No forms, controlled inputs, `select multiple`, portals, context, `forwardRef`, `lazy`/`Suspense`, `dangerouslySetInnerHTML` or React-style handlers exist in host source (grep over `pages/`, `src/`: 0 hits each). The workbench's `<input type="range">`, `type="search"`, `type="file"`, `<select>` and `<textarea>` live in vanilla `mount.mjs` and are outside zudo-react's model contract.

### 4. md-wasm and other packages

- **`@takazudo/zfb-md-wasm`** is a zudo-doc peer only; the repo has 0 `compile(`/`renderHtml(` calls and 0 `jsxRuntime` — nothing to do beyond the pin bump (https://zfb.takazudomodular.com/api/md-wasm/).
- **`packages/diagram-gen/src/render.mjs:24`** (`createPageSource`) — the one v3-breaking line in the engine. Today it emits `/** @jsxRuntime automatic */`, `/** @jsxImportSource preact */`, `<meta charSet="utf-8"/>` and `<body style={{margin:0}} dangerouslySetInnerHTML={{__html: body}}/>`. Under zfb 3 the pragma fails the build (`Could not resolve "preact/jsx-runtime"`, or `ZR_CHILD` if Preact is installed; since 3.2.0 a `zfb warn:` line first names the pragma's file:line:col), `charSet` fails validation (use `charset`), and `dangerouslySetInnerHTML` is replaced by `rawHtml` (https://zfb.takazudomodular.com/guides/migrating-to-v3/#components-and-islands, https://zfb.takazudomodular.com/zudo-react/components-and-jsx/). Replacement template body:

  ```js
  return `// Generated by zudo-diagram-gen. Edit session files, not this route.\nconst body = ${JSON.stringify(html)};\nexport const frontmatter = {title: ${JSON.stringify(data.session.title)}};\nexport default function DiagramGalleryPage() {\n  return <html lang="en"><head><meta charset="utf-8"/><meta name="viewport" content="width=device-width,initial-scale=1"/><meta name="color-scheme" content="light dark"/><title>{frontmatter.title} · zudo-diagram-gen</title></head><body style={{margin:0}} rawHtml={body}/></html>;\n}\n`;
  ```

  Keep the first line unchanged: `src/runner.mjs:9` uses `// Generated by zudo-diagram-gen.` as the ownership marker and `test/render.test.mjs:29` asserts it. `rawHtml` is opaque, so the embedded `<style>`/`<script>` text inside `body` (already `</style`/`</script`-escaped at `render.mjs:13`) is emitted verbatim; `style={{margin:0}}` is valid (numeric zero). **Verified 2026-10-04:** this exact template body, written as `pages/index.tsx` in a scratch host containing only `package.json` (`@takazudo/zfb` + `@takazudo/zfb-runtime` 3.2.0), `zfb.config.ts` = `defineConfig({})` and **no `tsconfig.json`**, built on zfb 3.2.0 (`✓ 1 pages built`) and emitted `<body style="margin:0;">` followed by the raw body verbatim. No doc states the page default when no tsconfig exists (https://zfb.takazudomodular.com/concepts/plugins/ documents the no-tsconfig fallback only for plugin entries), so keep the packed-consumer CI step as the standing proof rather than relying on the doc.
- **`packages/diagram-gen/package.json:42-45`**: `peerDependencies` → `{ "@takazudo/zfb": "^3.2.0" }` (the template itself only needs 3.0 features — `rawHtml`, `charset` — but match the range to the pin the initializer writes); delete the `preact` peer. The peer is a CLI contract (which `bin/zfb.mjs` dialect the generated route targets), resolved from the session host by `src/runner.mjs:62-65`. Expect a pnpm unmet-peer **warning** inside this workspace while the root still has `@takazudo/zfb` 2.21.1 — it is a warning, not a failure, and clears at the root cutover.
- **`packages/diagram-gen/README.md:22-23`**: "prepares a Preact route" → "prepares a zudo-react route". `IMPLEMENTATION-CONTRACT.md:10, 45`, `AGENTS.md:24`, `docs/CODEX-HANDOFF.md:37`, `src/content/docs/reference/architecture.mdx:28` say "zfb page entrypoints use Preact TSX" — update in the same PR (changelog entry required by `AGENTS.md:53`).
- **`packages/create-zudo-diagram-gen/src/index.mjs:161-166`**: pin `@takazudo/zfb` and `@takazudo/zfb-runtime` to `3.2.0` (npm `latest`); remove `preact` and `preact-render-to-string`. `hono`: zfb 3's `@takazudo/zfb-runtime` declares `hono` as its own dependency and the v3 `basic-blog` scaffold lists only `@takazudo/zfb` + `@takazudo/zfb-runtime`, so the explicit `hono` pin is probably removable — verify with a generated host (`pnpm install && pnpm dev && pnpm build`) before deleting it and the sentence at `README.md:52`. The generated `zfb.config.ts` (`defineConfig({})`, L169) stays valid: an absent `wind` key is the empty enabled config with reset `none`, and the generated page carries all its CSS inside `rawHtml`. Update `test/initializer.test.mjs:37-41`, and add a `!source.includes('@jsxImportSource')` assertion to `test/render.test.mjs` (today it asserts only the ownership marker, L29, and `!source.includes('node:fs')`, L30).
- `client/mount.mjs`, `client/app.js` (classic bundle regenerated by `client/build-standalone.mjs` — a plain string rewrite of `mount.mjs`, no esbuild), `client/app.css`, `src/model.mjs`, `src/commands.mjs`, `schemas/`, `tones/`: unaffected (vanilla JS/CSS/JSON; happy-dom suite `test/ui.test.mjs` exercises `mount.mjs` directly).
- No sub-package uses Vite, Next or Astro (`grep -n '"vite"\|"next"\|"astro"' package.json packages/*/package.json` → 0 hits), so there is no Tailwind-through-Vite surface to leave alone; the only lockfile in the repo is the root `pnpm-lock.yaml` (no `docs/` package), and a generated session host gets its own lockfile at run time, never committed here.

### 5. Tests, CI, deploy

- **`.github/workflows/ci.yml:28-35`**: `pnpm install --frozen-lockfile` → regenerate `pnpm-lock.yaml` in the migration PR. `pnpm check` is `node scripts/build-showcase.mjs && tsc --noEmit` (`package.json:17`) and will type-check the island against `@takazudo/zfb/zudo-react` once `tsconfig.json:7` changes. Add `pnpm exec zfb wind audit --fail-on error` after `pnpm build` (https://zfb.takazudomodular.com/api/cli/#zfb-wind-audit; checklist step 6) — this is a v3-only step, add it at cutover.
- **`ci.yml:43-58` (installed consumer)** is the proof for the package half: it packs both archives, runs `create-zudo-diagram-gen`, then `pnpm install`, `pnpm check`, `pnpm build` and two `export-html` runs in the generated host. After the package flip this host installs zfb 3.1.0 and builds the zudo-react template — no workflow edit needed. `ci.yml:59-75` previews the consumer build; `ci.yml:76-81` runs `check:browser` (export-html, unaffected) and `check:site-browser` (root, blocked until cutover).
- **`scripts/check-site-browser.mjs`**: `expectHostTheme` (L48-66) requires `document.documentElement.dataset.theme === mode` and `.dg-app[data-ui-theme] === mode`, reading zudo-doc's `[data-zd-theme-menu] button[aria-haspopup="menu"]` on failure; L114-121 asserts 10 `.dg-card` after hydration and the footer copyright text. These depend on zudo-doc 6's theme attribute and menu markup — re-verify, not rewrite, once 6.0.0 is in.
- **`scripts/prepare-cloudflare-assets.mjs:3-11`** asserts `dist/404.html` exists and contains `Page not found.` (zudo-doc's 404 route). Re-verify the string under zudo-doc 6.
- **`deploy.yml`** / **`wrangler.jsonc`** / **`worker/index.mjs`**: assets-only Worker with `run_worker_first` redirects; no SSR adapter; nothing zfb-specific beyond `pnpm build`. Unchanged. `security.yml` (`pnpm audit --prod --audit-level high`) picks up the new lockfile automatically.
- **Dependabot**: PRs #60 (preact 10.29.8) and #61 (preact-render-to-string 6.7.0) become moot; #59 (typescript 6.0.3) and #62 (`@types/node` 26) are unrelated — merge or close them before the base branch to keep the lockfile diff small.
- `test/cloudflare-worker.test.mjs`, `test/build-showcase.test.mjs`, `packages/diagram-gen/test/model.test.mjs`: no zfb coupling.

## Step-by-step plan

Phase A — now (independent of zudo-doc), one PR `topic/engine-zfb3` into `base/zfb3-migration` (or straight to `main`, since it changes nothing the root site renders):

1. `packages/diagram-gen/src/render.mjs:24`: apply the template from section 4 (drop both pragmas, `charSet` → `charset`, `dangerouslySetInnerHTML` → `rawHtml`).
2. `packages/diagram-gen/package.json`: `peerDependencies: { "@takazudo/zfb": "^3.2.0" }`; remove `preact`. Update `README.md:22-23`.
3. `packages/create-zudo-diagram-gen/src/index.mjs:161-166`: `@takazudo/zfb` and `@takazudo/zfb-runtime` → `3.2.0`; remove `preact`, `preact-render-to-string`; decide `hono` after step 5. Update `test/initializer.test.mjs:37-41` and `README.md:52`.
4. `pnpm install` (root lockfile changes only for the workspace peer), `pnpm test` (vitest: `test/render.test.mjs`, `packages/*/test`), `pnpm lint`, `pnpm format:check`.
5. Prove the consumer path locally, mirroring `ci.yml:43-58`:
   ```sh
   pnpm pack:local
   consumer="$(mktemp -d)/diagram-consumer"
   npm exec --yes --package ./artifacts/create-zudo-diagram-gen-0.1.0.tgz -- create-zudo-diagram-gen "$consumer" --name 'v3 consumer' --engine-package "$PWD/artifacts/takazudo-zudo-diagram-gen-0.1.0.tgz" --yes
   cd "$consumer" && pnpm install && pnpm exec zfb --version && pnpm check && pnpm build && pnpm preview --port 4334
   cp -R /path/to/repo/examples/project-text/rounds/r01/r01-c01 rounds/r01/ && pnpm build
   ```
   `zfb --version` must print `zfb 3.2.0` and an `embedded esbuild: 0.25.12` line only. Open the preview and confirm the gallery mounts (`.dg-app` present, 1 candidate after the copy).
6. Also do the two safe host edits here or in a sibling PR: `scripts/build-showcase.mjs` `className=` → `class=` (8 lines; leave the `<svg xmlns=…>` at L41 alone); `pages/docs/[[...slug]].tsx` drop `import type { JSX } from 'preact'` and the `: JSX.Element` annotation. Run `pnpm check && pnpm build && pnpm check:site-browser` on 2.21.1 to show no regression.
7. Document the state: changelog entry under `src/content/docs/changelog/`, and update the "Preact TSX" sentences listed in section 4.

Phase B — after zudo-doc 6.0.0 ships, `topic/host-zfb3` into the same base (official checklist, https://zfb.takazudomodular.com/guides/migrating-to-v3/#checklist, adapted):

8. Read zudolab/zudo-doc#4473. `package.json`: `@takazudo/zudo-doc` → `6.0.0`; `@takazudo/zfb`, `zfb-md-wasm`, `zfb-runtime` → the exact version 6.0.0's peer range names (≥ 3.2.0, since that is the first release carrying #3569/#3570); remove `tailwindcss`, `preact`, `preact-render-to-string` (unless still peers). `pnpm install`; `pnpm exec zfb --version`.
9. `zfb.config.ts`: no key changes expected; run `pnpm exec zfb check` — it now loads the preset and reports any leftover removed key with the named migration error.
10. `src/styles/global.css`: rewrite per section 2 against the 6.0 template; run `pnpm exec zfb css --input src/styles/global.css --output /tmp/dg.css` and fix every ZW009 it lists (one run lists them all).
11. `tsconfig.json:7` → `@takazudo/zfb/zudo-react`; replace `src/components/diagram-workbench.tsx` with the section 3 component; `grep -rln "@jsxImportSource" pages src` must be empty; `pnpm check`.
12. `pnpm build`; then `pnpm exec zfb wind audit --fail-on error`. Expect the 16 token-bearing showcase classes to resolve through the preset; add a `zudoDoc({ wind: { tokens } })` override only for a residual ZW006/ZW002.
13. `node scripts/check-built-links.mjs`; `pnpm check:site-browser` (hydration: 10 `.dg-card`; theme sync); `pnpm check:browser`; `pnpm check:examples`; `pnpm test`.
14. Reset/preflight pass on `/docs/workbench/` and one prose page: compare headings, lists, `code`/`pre` font stack, `::placeholder` of the search box, the `type="range"` slider, `sub`/`sup`, `[hidden]` against the 5.x build (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#differences-from-tailwind-preflight). The workbench's own controls are styled by `app.css`; the page chrome is zudo-doc's.
15. `pnpm exec wrangler deploy --dry-run`; `pnpm exec wrangler deploy --config wrangler.preview.jsonc` for a `workers.dev` smoke (`docs/cloudflare-setup.md` step 4); `pnpm smoke:cloudflare https://<preview-host>/`; delete the preview Worker.
16. Merge `base/zfb3-migration` → `main`; `deploy.yml` deploys; `pnpm smoke:cloudflare https://zudo-diagram-gen.zudolab.dev/` and `--redirects`; record in `docs/VERIFICATION.md` as the repo convention requires.

## Verification checklist

- [ ] `pnpm exec zfb --version` prints a 3.x version and only an `embedded esbuild` line (root and generated consumer).
- [ ] `grep -rnE '@import\s+.tailwindcss|@tailwind|@theme|@apply|@source|@utility|@variant|@custom-variant|@plugin|@config|theme\(' --include=*.css --include=*.ts --include=*.tsx --include=*.mjs --exclude-dir=node_modules .` → 0 hits (today: 4, all in `src/styles/global.css`).
- [ ] `grep -rnE "from ['\"]preact|preact/hooks|@jsxImportSource|dangerouslySetInnerHTML|className=|charSet" --include=*.ts --include=*.tsx --include=*.mjs --exclude-dir=node_modules pages src scripts packages` → 0 hits (today 11: `diagram-workbench.tsx:3`, `[[...slug]].tsx:2`, `render.mjs:24`, `build-showcase.mjs:37-41,44,45,59`). That pattern does not catch the dependency pins — also run `grep -rn preact package.json packages/*/package.json packages/create-zudo-diagram-gen/src/index.mjs` → 0 hits (today: `package.json:40-41`, `packages/diagram-gen/package.json:44`, `index.mjs:164-165`).
- [ ] `pnpm check` (tsc) green with `jsxImportSource: "@takazudo/zfb/zudo-react"`; `pnpm exec zfb check` green.
- [ ] `pnpm exec zfb wind audit --fail-on error` exit 0; audit shows 0 unrecognized / 0 dead classes (today 0/0).
- [ ] `pnpm build` emits the same route set as 5.x (VERIFICATION.md records 33 pages at Doc Combine) and `dist/404.html` contains `Page not found.`.
- [ ] `pnpm check:site-browser`: `/docs/workbench/` hydrates (`.dg-card` count 10), theme toggle keeps `html[data-theme]` and `.dg-app[data-ui-theme]` in sync, no `pageerror`/console errors, no horizontal overflow at 1280/390.
- [ ] Packed consumer (`ci.yml:43-58` flow) installs zfb 3.x, `pnpm check && pnpm build && pnpm preview` pass, `export-html` output unchanged in behaviour (`pnpm check:browser`).
- [ ] `pnpm test`: all vitest suites green, including the updated `initializer.test.mjs` pins and `render.test.mjs` ownership marker.
- [ ] `wrangler deploy --dry-run` accepts the Worker; preview Worker smoke passes; production smoke + redirects pass after merge.
- [ ] Not tested by anything here (state it in the PR): visual parity of zudo-doc chrome under its new reset — that is zudo-doc 6's surface, not this repo's.

## Risks and open questions

- **zudo-doc 6.0.0 API drift.** `defineChromeBindings`/`mdxExtras` (`src/chrome-bindings.tsx:2,5`), `createRouteContext`/`createChrome` (`pages/docs/[[...slug]].tsx:4-5`), the `virtual:zudo-doc-*` modules, `zudoDoc()` settings keys and the shipped CSS exports are all zudo-doc's to change; this guide can only say which host lines touch them. Re-read #4473 before Phase B.
- **Peer major skew inside the workspace** after Phase A: `packages/diagram-gen` declares `@takazudo/zfb ^3.2.0` while the root installs 2.21.1. pnpm warns; nothing in the root imports the package's zfb-facing code (`createPageSource`) at build time, so the root site keeps building. Confirm the warning is tolerable in CI logs or add a short-lived note in the PR.
- **Root pin target.** The root will pin whatever zudo-doc 6.0.0 declares; 3.2.0 is the first release containing #3569/#3570 (zudo-doc's integration blockers), so it cannot be lower. Until 6.0.0 exists, Phase B cannot even be dry-run; the Phase A consumer flow is the only v3 build this repo can exercise today.
- **Hydration fails closed.** A server/client mismatch leaves the workbench island inert instead of client-rendering (https://zfb.takazudomodular.com/zudo-react/hydration/). The `Show` regions and the childless host `div` are deterministic, but any zudo-doc 6 wrapper markup around `mdxExtras` output must be identical on both sides — `check:site-browser` L114-121 is the detector.
- **Island bundle size** is not promised to match Preact's (https://zfb.takazudomodular.com/concepts/islands/#measured-shared-bundle-baseline); the site has one island, so measure `dist/assets/islands-*.js` before/after and note it.
- **`hono` in the generated host** (`create-zudo-diagram-gen/src/index.mjs:163`): removable only if a fresh generated host runs `pnpm dev`/`build`/`preview` without it on zfb 3.2.0 — verify, do not assume (the scratch build above only proved `zfb build` with `hono` present as zfb-runtime's own dependency).
- **Exact-pin policy vs peer ranges**: `AGENTS.md:22` requires exact versions; zudo-doc 6's peer range will be a caret. Keep exact pins at the root and let the range be satisfied.
- **Open question**: should `packages/diagram-gen` stop declaring a zfb peer at all and instead document "run in a zfb 3 host"? It never imports zfb; the peer only encodes the generated route's dialect. Keeping `^3.2.0` is the conservative choice and is what this guide assumes.
- **Open question**: publication. Both packages are unpublished (`README.md:9`); if 0.2.0 is the first published version, make it the v3-only one so no 2.x-compatible release has to be maintained.

## References

- Official migration guide: https://zfb.takazudomodular.com/guides/migrating-to-v3/
- Tailwind → zudo-wind map: https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/
- Preact hooks → zudo-react map: https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/
- Wind configuration (tokens, manifests, authoredClasses, placement): https://zfb.takazudomodular.com/zudo-wind/configuration/
- Wind diagnostics (ZW001–ZW014, strict mode, audit output): https://zfb.takazudomodular.com/zudo-wind/diagnostics-and-tools/
- Sources and candidates (scan roots, class positions): https://zfb.takazudomodular.com/zudo-wind/sources-and-candidates/
- Components and JSX (attribute spellings, `rawHtml`, SVG vocabulary): https://zfb.takazudomodular.com/zudo-react/components-and-jsx/
- Scopes and lifecycle (`onActivate`, `abortSignal`, cleanup): https://zfb.takazudomodular.com/zudo-react/scopes-and-lifecycle/
- Conditionals and lists (`Show`, `For`): https://zfb.takazudomodular.com/zudo-react/conditionals-and-lists/
- Hydration contract: https://zfb.takazudomodular.com/zudo-react/hydration/
- API reference (`Description`, `Child`, `Ref`): https://zfb.takazudomodular.com/zudo-react/api-reference/
- Islands (direct `Island({...})` calls, third-party widget pattern, bundle baseline): https://zfb.takazudomodular.com/concepts/islands/ and https://zfb.takazudomodular.com/api/island/
- CLI (`zfb css`, `zfb wind explain|audit`, `zfb check`; `zfb wind manifest`, `zfb wind audit --json|--severity|--plan build`, `--config <file>` and `file:line:col` diagnostics are **3.2.0** features — 3.1.0's `zfb wind --help` lists only `explain` and `audit`): https://zfb.takazudomodular.com/api/cli/
- `defineConfig` / removed keys / `wind` shape: https://zfb.takazudomodular.com/api/define-config/
- md-wasm v3 options (no `jsxRuntime`): https://zfb.takazudomodular.com/api/md-wasm/
- Changelogs: https://zfb.takazudomodular.com/changelog/zfb/v2.22.0/ , https://zfb.takazudomodular.com/changelog/zfb/v2.22.1/ , https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/ , https://zfb.takazudomodular.com/changelog/zfb/v3.2.0/ (released 2026-10-04: SVG/HTML vocabulary incl. `svg xmlns`, `file:line:col` Wind diagnostics, `zfb wind manifest`, `wind.sources`, `wind.strict`/ZW014, `wind.utilities.placement`, the #3569/#3570 fixes)
- Upstream: zudolab/zudo-doc#4430 (6.0 epic), zudolab/zudo-doc#4477 (root PR), zudolab/zudo-doc#4473 (consumer guide); Takazudo/zudo-front-builder#3569, #3570 (UTF-8 offsets / quoted island markers), #3359 (`svg xmlns` vocabulary), #3370 (`file:line:col` audit output) — all closed and shipped in zfb 3.2.0 on 2026-10-04
- Repo facts: `docs/cloudflare-setup.md` (deploy procedure), `docs/VERIFICATION.md` (33 pages / 408 assets at Doc Combine), `AGENTS.md:22` (exact pins), `AGENTS.md:53` (changelog entry per user-visible change)
