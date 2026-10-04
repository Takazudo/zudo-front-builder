# zfb v3 migration guide: Takazudo/zudo-design-token-panel-example-zfb

Generated 2026-10-04 by an automated diagnosis of `main` @ `5425d3a` (2026-09-07). Counts come from the commands listed; re-run them locally before relying on them.

## Verdict

**ready, effort M.** A single-dir zfb 2.15.1 demo site (6 prerendered routes, 12 components, 1 MDX entry) that embeds the `@takazudo/zdtp` Preact widget. It uses **no Tailwind and no utilities at all** (0 directives, 0 dead classes in the audit), so the styling side is one line: `wind: false`. The work is the component side: 3 islands (`PanelMount`, `ModalInner`, `TabsInner`), 6 hooks-using functions (`useEffect` ×4, `useState` ×3, `useRef` ×2, `useId` ×1), 5 `onClick` + 1 `onKeyDown`, 1 `dangerouslySetInnerHTML`, 1 callback ref, and 27 inline `style` objects with camelCase keys. Nothing upstream gates the start (no preset); `@takazudo/zfb@latest` is 3.1.0. Follow the zdtp decision — the panel stays an opaque Preact bundle mounted lazily from a zudo-react island, exactly the "Embedding a third-party widget" pattern — and keep `preact` installed only as zdtp's peer until zdtp#1002 ships. The next zfb release is not required (it would only matter if `wind` were turned on, because 36 BEM `__` class names are ZW001 errors on 3.1.0).

## Current state

| Package / dir | zfb | zudo-doc / zudo-sg / zdtp | Tailwind | Preact | Islands | md-wasm | Deploy |
| --- | --- | --- | --- | --- | --- | --- | --- |
| repo root (zfb project: `pages/` 6 routes, `components/` 12 files, `content/prose/index.mdx`, `plugins/dev-apply-proxy.mjs`, `styles/global.css` 1,193 lines / 81 `--zfb-*` declarations) | `@takazudo/zfb`, `@takazudo/zfb-runtime` **2.15.1** (`package.json:28-29`); `zfb.config.ts:46-60`: `framework: "preact"`, `base: "/"`, 1 collection, `markdown: { gfm: true }`, 1 plugin (`devMiddleware`); `defineConfig` from `@takazudo/zfb/config` | `@takazudo/zdtp 0.5.1` (package is at 0.8.5; independent of zfb) | none — `grep` for `@import "tailwindcss|@theme|@apply|@source|…` → 0; no `tailwind.config.*`, no `@tailwindcss/*` dep | `preact ^10.29.1`, `preact-render-to-string ^6.5.0`; `tsconfig.json:9` `jsxImportSource: "preact"`; `preact/hooks` in 4 files | 3 `<Island when="visible" ssrFallback={null}>`: `components/app-shell.tsx:89` (PanelMount), `components/widgets/modal.tsx:130` (ModalInner), `components/widgets/tabs.tsx:134` (TabsInner) | no | assets-only Worker `zdtp-zfb.zudolab.dev` (`wrangler.toml`, no `main`); `.github/workflows/deploy.yml` build → deploy (push main) / preview (PR, `wrangler versions upload`, routing-matrix assertion) |

Audit (3.1.0 CLI, temporary `wind: { spec: 1 }`, config restored, `git status --short` clean): 155 unrecognized (all authored `zfb-*` names), 0 conflicts, **0 dead classes**, 36 error-severity diagnostics — all **ZW001 "invalid named utility characters" on BEM double-underscore classes**: `zfb-topbar__label`, `zfb-app-shell__sidenav/__main`, `zfb-avatar-row__item`, `zfb-table__actions/__action` ×2, `zfb-media-card__image/__image-inner/__title/__desc/__cta`, `zfb-profile-card__avatar/__info/__name/__role/__action`, `zfb-stat-card__value/__label`, `zfb-sidenav__list`, `zfb-accordion__summary/__content`, `zfb-modal__trigger` ×2 `/__panel-inner/__header/__close`, `zfb-tabs__list/__tab/__indicator/__panel` ×3, `zfb-tag__close`, `zfb-tooltip__trigger/__bubble`. Plus `auditInfo` only: 54 ZW001 / 54 ZW005 / 7 ZW002 from string literals (SVG `xmlns="http://…"`, `'zfb:before-swap'`, route paths). Under `wind: false` candidate scanning is off and none of this is evaluated.

Greps: `preact/hooks` imports at `components/panel-mount.tsx:76`, `components/widgets/modal.tsx:26-27`, `components/widgets/tabs.tsx:18`, `pages/index.tsx:19`; `dangerouslySetInnerHTML` at `components/app-shell.tsx:67`; `onClick` at `pages/index.tsx:38`, `modal.tsx:82,96,117`, `tabs.tsx:63`; `onKeyDown` at `tabs.tsx:53`; callback ref `tabs.tsx:65`; `tabIndex` `tabs.tsx:62`; `charSet` `app-shell.tsx:42`; camelCase style objects at `accordion.tsx:44,48`, `pages/components/data.tsx:25,30,39,50,61,75,82,89`, `forms.tsx:26,31,139,163`, `status.tsx:24,74,85,96,133,171,183,217`, `widgets.tsx:34,46,49,61,64,74,77`. Forms (`pages/components/forms.tsx`) are all **uncontrolled** (no `value=`/`onInput`), including `type="range"`, `checkbox`, `radio`, `<select>`, `<textarea>` — fine in v3 (uncontrolled by default, https://zfb.takazudomodular.com/zudo-react/forms/).

Commands used for the counts above (run from the clone root with a zfb 3.1.0 binary; the config swap is temporary — restore it and confirm `git status --short` is empty):

```sh
cp zfb.config.ts /tmp/zfb.config.bak && printf 'import { defineConfig } from "zfb/config";\nexport default defineConfig({ wind: { spec: 1 } });\n' > zfb.config.ts
zfb wind audit --project-root . > /tmp/audit.txt; cp /tmp/zfb.config.bak zfb.config.ts
grep -rnE '@import\s+"tailwindcss|@tailwind|@theme|@apply|@source|@utility|@variant|@custom-variant|@plugin|@config|theme\(' --include=*.css --include=*.ts --include=*.tsx --include=*.mjs --exclude-dir=node_modules --exclude-dir=dist .
grep -rnE "from ['\"]preact|preact/hooks|@jsxImportSource|dangerouslySetInnerHTML|onClick=|onKeyDown=|<Island|\"use client\"" --include=*.ts --include=*.tsx --exclude-dir=node_modules --exclude-dir=dist .
```

## Sequencing and blockers

1. **No upstream blocker.** No preset; zdtp is consumed as an opaque bundle (zudo-doc decision DD3; zdtp#1002 tracks bundling Preact into it).
2. **zdtp#1002 is optional, not a gate.** Until it ships, keep `preact ^10.29.1` as a dependency for zdtp's peer; drop `preact-render-to-string` (zfb no longer needs it).
3. **Independent of v3:** bumping `@takazudo/zdtp` 0.5.1 → 0.8.5 (several behavior changes since 0.5.1, e.g. header actions collapse behind a kebab at ≤1135px panel width — `tests/e2e/highlight.spec.ts:88-91` already anticipates renames). Do it in a separate PR so e2e diffs stay attributable.
4. **Next zfb release** items that are nice-to-have here: `zfb wind audit --json`, ZW014/`wind.strict` — irrelevant under `wind: false`.

## Required changes

### 1. Dependencies, config, tsconfig, env

- `package.json:27-32`: `@takazudo/zfb`, `@takazudo/zfb-runtime` → `3.1.0`; remove `preact-render-to-string`; keep `preact` (annotate: zdtp peer). `predev` (`:12`) can drop `zfb-tailwind-entry-*.css` from its `rm -rf` list (no longer produced).
- `zfb.config.ts:46-60`: delete `framework: "preact"` (hard error in v3); add `wind: false` — "To keep authored CSS and CSS Modules without generated utilities, use `wind: false`" (https://zfb.takazudomodular.com/guides/migrating-to-v3/#configuration). `base`, `collections`, `markdown.gfm`, `plugins` are unchanged keys (https://zfb.takazudomodular.com/api/define-config/).

```ts
import { defineConfig } from "@takazudo/zfb/config";
export default defineConfig({
  base: "/",
  wind: false,
  collections: [{ name: "prose", path: "content/prose" }],
  markdown: { gfm: true },
  plugins: [{ name: "./plugins/dev-apply-proxy.mjs" }],
});
```
- `tsconfig.json:8-9`: keep `"jsx": "react-jsx"`, set `"jsxImportSource": "@takazudo/zfb/zudo-react"`.
- `.npmrc`: `public-hoist-pattern[]=hono` was added for the 2.x dev renderer; `@takazudo/zfb-runtime` 3.1.0 still depends on `hono` and the v3 `basic-blog` scaffold ships no `.npmrc`. Keep the line, try `zfb dev` without it once, and delete it only if the dev server renders.
- Env/CI: `grep -rn ZFB_TAILWIND .github scripts package.json` → 0. Nothing to remove.

### 2. CSS and utilities

- `styles/global.css`: no Tailwind constructs, so it is valid authored CSS as-is. Two things to check: (a) `@import '@takazudo/zdtp/styles.css'` sits at **line 147**, after `:root` blocks — zudo-wind hoists external imports in its emitted order (https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/#utility-placement-and-ties) but a mid-file `@import` is not standard CSS; move it to line 1. (b) That `exports`-subpath spelling failed on zfb 3.0.0 (zdtp#1002); 3.1.0 lists "Resolve authored CSS package subpaths through package `exports`" (https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/). If it still fails, use `@import '@takazudo/zdtp/dist/zdtp.css'` or delete the import — the panel self-injects its stylesheet (zdtp PORTABLE-CONTRACT §7).
- Reset: the file already owns its own box-sizing/body/control resets (`styles/global.css:27-62`); `wind: false` emits no reset, so visual output is unchanged.
- The 36 BEM `__` names are only a problem if `wind` is ever enabled; then rename to single hyphens or wait for the **next zfb release** (zfb #3365 makes underscores ordinary).

### 3. Components and islands

**`components/panel-mount.tsx` → zudo-react host for the opaque widget** (pattern: https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget). The file already does the right thing structurally — static import of the zero-dependency `@takazudo/zdtp/constants` (l.78-81) and a lazy `import('@takazudo/zdtp')` (l.205) — so the port is mechanical:

```tsx
"use client";
import { getScope } from "@takazudo/zfb/zudo-react";
// ...unchanged helpers: getAdapterState, hasActiveFlagSignal, hasPersistedOverrides, loadPanelModule, installConsoleApi, mountPanel
export default function PanelMount() {
  getScope().onActivate(() => { mountPanel(); });   // was useEffect(..., [])
  return null;
}
```
Keep the lazy import inside `loadPanelModule` (it runs from `mountPanel`, i.e. after activation — never at module top level). Verify that a component returning `null` is accepted by the renderer (the docs' example returns a host `<section>`); if not, return an empty element such as `<span hidden />`. The console API, eager-load gate and `window.__zudoDesignTokenPanelAdapter` map are framework-free and stay.

**`components/app-shell.tsx`**
- l.35 `children: preact.ComponentChildren` → `import type { Child } from "@takazudo/zfb/zudo-react"`.
- l.42 `charSet` → `charset`.
- l.66-71 `<script dangerouslySetInnerHTML={{ __html: "..." }} />` → `<script rawHtml={PANEL_BUTTON_BRIDGE} />` (trusted static string; https://zfb.takazudomodular.com/zudo-react/components-and-jsx/).
- l.89-91: `<Island when="visible" ssrFallback={null}>` — `when` and `ssrFallback` are v3 props (https://zfb.takazudomodular.com/api/island/#skip-server-rendering). Remove the `as unknown as IslandProps['children']` casts here and in `modal.tsx:131`, `tabs.tsx:135`; `IslandProps.children` is typed as a `Description`. `ssrFallback={null}` still selects skip-SSR mode in 3.1.0 — the boundary skips SSR whenever the prop is not `undefined` (`packages/zfb/src/island-boundary.ts:55`, `skipSsr: fallback !== undefined`; https://zfb.takazudomodular.com/api/island/#skip-server-rendering) — so the `null` idiom stays.

**`components/widgets/modal.tsx`** (`useState`, `useRef`, 3 `useEffect`, 3 `onClick`)
- `const isOpen = signal(false)`; `const dialogRef: Ref<HTMLDialogElement> = { current: null }` (plain object, assigned before activation).
- Effect 1 (l.38-52, `showModal()`/`close()` + `main.inert`) → `getScope().effect(() => { const dialog = dialogRef.current; if (!dialog) return; if (isOpen.value) {...} else {...} })` — reruns on `isOpen`.
- Effect 2 (l.54-63, Escape listener while open) → `scope.effect` with the cleanup returned from the callback; effect 3 (l.66-75, `cancel` listener once) → `scope.onActivate` with cleanup.
- `onClick={() => setIsOpen(true)}` → `on:click={() => { isOpen.value = true; }}` (https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/#patterns).
- `<>…</>` fragment at l.78 stays.

**`components/widgets/tabs.tsx`** (`useState`, `useId`, `useRef` + callback refs, `onClick`, `onKeyDown`, `tabIndex`, reactive `style`)
- `activeIndex` → `signal(0)`; per-tab derived values via `computed()` (`aria-selected`, `tabindex`, `class`, `hidden` — ARIA/boolean attributes accept reactive scalars; `hidden` accepts booleans only).
- `useId()` has **no replacement**; the page mounts one `TabsInner`, so a module constant (`const baseId = "zfb-tabs"`) is enough.
- l.36,65 `useRef<(HTMLButtonElement|null)[]>` + `ref={(el) => …}`: callback refs are rejected ("Refs are simple objects … There are no callback refs"); build `const tabRefs = TABS.map((): Ref<HTMLButtonElement> => ({ current: null }))` and pass `ref={tabRefs[i]}`.
- l.62 `tabIndex` → `tabindex`; l.53 `onKeyDown` → `on:keydown` (native `KeyboardEvent`); l.63 `onClick` → `on:click`.
- l.81-85 indicator `style={{ width, transform, transition }}` depends on `activeIndex`: reactive values **inside** a style object are rejected, so make the whole prop reactive: `style={computed(() => ({ width: `${indicatorPct}%`, transform: `translateX(${activeIndex.value * 100}%)`, transition: "transform 0.25s var(--zfb-easing-tab-open)" }))}`.

**`pages/index.tsx:32-46` `EasingDemoCard`** uses `useState` but is **not** inside an island and has no `"use client"`, so on 2.x it was SSR-only and its click never worked in the browser. In v3: either make it static (drop the state; the CSS `is-active` toggle then needs a tiny `rawHtml` script or an island) or export it from a `"use client"` module and wrap it in `<Island>`. Recommend the island; convert `onClick` → `on:click`, `aria-pressed={active}` → the signal.

**Inline style objects (27 sites)**: object keys must be CSS-spelled, units explicit (https://zfb.takazudomodular.com/zudo-react/components-and-jsx/): `marginTop` → `"margin-top"`, `marginBottom` → `"margin-bottom"`, `maxWidth` → `"max-width"`, `flexWrap` → `"flex-wrap"`, `fontSize` → `"font-size"`, `lineHeight` → `"line-height"`, `userSelect` → `"user-select"`; `gap`, `display`, `color` are already valid. CSS **string** styles (`avatar-row.tsx:27`, `media-card.tsx:21`, `pages/index.tsx:113`) are accepted as-is.

**Static components** (`sidenav.tsx`, `accordion.tsx`, `data/*.tsx`, `pages/components/*.tsx`, `pages/prose.tsx`): `class` is already the HTML spelling; `key` on mapped children is carried by descriptions; `aria-hidden="true"`, `role`, `hidden` are fine. `pages/prose.tsx:36-42` imports `getCollection`, `defaultComponents`, `CollectionEntry`, `ContentElement`, `ContentProps` from `@takazudo/zfb/content` — all still exported in 3.1.0. Its `globalThis.__zfb.content` bridge fallback (l.64-66) is an internal; test whether `getCollection('prose')` now returns the entry and delete the fallback if so.

### 4. md-wasm and other packages

- No `@takazudo/zfb-md-wasm` dependency, no `compile(`/`renderHtml(`. Nothing to do.
- `@takazudo/zdtp`: types are imported from `@takazudo/zdtp/astro` (`panel-mount.tsx:77`, `config/panel-config.ts:30`); the main entry types also export `PanelConfig`. Both keep working; after zdtp#1002 ships, drop `preact` from `dependencies`.

### 5. Tests, CI, deploy

- `.github/workflows/deploy.yml:85-89`: `pnpm typecheck` (= `zfb check`) and `pnpm build` (= `zfb build`) are the gate; both commands exist unchanged in v3 (https://zfb.takazudomodular.com/api/cli/). Add nothing for wind (`wind: false`). The preview job's routing-matrix assertion (`:343-348`, six `<title>` strings, `/prose` → 307 `/prose/`) depends only on `base: "/"` and directory-index output; keep it as the post-migration smoke.
- `tests/e2e/*.spec.ts` (4 files) run against `zfb preview` + the `zdtp-server` sidecar via `scripts/launch.mjs`; they assert panel DOM (`.tokenpanel-shell`, `.tokenpanel-highlight-*`) and `window.zfb.toggleDesignPanel`, not framework internals. They stay valid provided the `PanelMount` island activates; `highlight.spec.ts:49-52` comment ("PanelMount's useEffect") becomes `onActivate`.
- `playwright.config.ts:11-19` explains why the harness uses preview, not dev; unchanged.
- `README.md` ("mounted as a Preact island", "PanelMount … only runs a useEffect", the `__H` hooks rationale for `ssrFallback`) and `package.json:9` description must be rewritten: the v3 reason for skip-SSR is "the widget is a self-mounting Preact bundle; its module must never run on the server". `PROBE-REPORT.md` is history; add a dated v3 section rather than editing.

## Step-by-step plan

1. `git switch -c topic/zfb3`; bump `package.json` (zfb 3.1.0, drop `preact-render-to-string`); `pnpm install`; `pnpm exec zfb --version` → `zfb 3.1.0`.
2. `zfb.config.ts`: remove `framework`, add `wind: false`; `pnpm typecheck` must not report a removed-key error.
3. `tsconfig.json` `jsxImportSource`; `grep -rln "@jsxImportSource" pages components` → 0.
4. Port `panel-mount.tsx`, `app-shell.tsx` (rawHtml, charset, casts), `modal.tsx`, `tabs.tsx`, `pages/index.tsx` (`EasingDemoCard` → island), then the 27 style objects; `pnpm typecheck` after each file.
5. Move `@import '@takazudo/zdtp/styles.css'` to the top of `styles/global.css`; `pnpm build`. If the import fails to resolve, switch to `dist/zdtp.css` or remove it.
6. `pnpm preview` (or `pnpm dev` + sidecar): on `/`, click "Open Design Token Panel", confirm `.tokenpanel-shell`; on `/components/widgets/` open the modal, arrow through the tabs; on `/components/forms/` type into every control; `/prose/` renders the MDX.
7. `pnpm test:e2e` (builds, serves, starts the sidecar; apply round-trip rewrites `styles/global.css` and restores it).
8. Update `README.md`, `package.json` description, `playwright.config.ts`/spec comments; push; let `deploy.yml` preview assert the routing matrix.
9. Separately: `@takazudo/zdtp` 0.5.1 → 0.8.5 and, once zdtp#1002 ships, drop `preact`.

## Verification checklist

- [ ] `pnpm exec zfb --version` → 3.1.0; `pnpm typecheck` clean; `pnpm build` prints no `zfb warn:` pragma line.
- [ ] `grep -rn "preact" components pages tsconfig.json` → 0 (dependency only).
- [ ] Built `dist/index.html` has one `<script type="module" src="/assets/islands-*.js">` and three island wrappers (`data-zfb-island-skip-ssr` ×3).
- [ ] Panel opens on all six routes (`tests/e2e/highlight.spec.ts` matrix) and `window.zfb.toggleDesignPanel` is a function.
- [ ] Modal: open, Escape, native cancel; Tabs: click, ArrowLeft/Right/Home/End, focus moves; indicator slides.
- [ ] `pnpm test:e2e` green locally; `deploy.yml` build + preview green; routing matrix and `/prose` 307 hold.
- [ ] No console errors on `/` with the panel open (the #1002 probe baseline is 0).

## Risks and open questions

- `return null` from `PanelMount` is a Preact-era idiom; the v3 docs show element-returning hosts. Confirm with one `zfb build`; the fix is a one-line `<span hidden />`. (`ssrFallback={null}` is confirmed to keep skip-SSR, see §3.)
- `useId` removal: safe only while a single `TabsInner` exists per page (true today, `pages/components/widgets.tsx` mounts one).
- `EasingDemoCard` never hydrated on 2.x; making it an island changes behavior (it starts working). Decide whether the demo wants that.
- The `globalThis.__zfb.content` bridge fallback in `pages/prose.tsx` is undocumented; if v3 removed it, the page must rely on `getCollection` (verify the snapshot is now populated for static-only projects).
- `.npmrc` hono hoisting may be unnecessary on v3; removing it blind risks the silent "renderer disabled" dev failure documented in the file.
- Preact stays in `node_modules` for zdtp until zdtp#1002; any stray Preact pragma then fails as `ZR_CHILD` at render, not at install.

## References

- https://zfb.takazudomodular.com/guides/migrating-to-v3/
- https://zfb.takazudomodular.com/concepts/islands/#embedding-a-third-party-widget · https://zfb.takazudomodular.com/api/island/
- https://zfb.takazudomodular.com/zudo-react/coming-from-preact-hooks/ · https://zfb.takazudomodular.com/zudo-react/components-and-jsx/ · https://zfb.takazudomodular.com/zudo-react/forms/ · https://zfb.takazudomodular.com/zudo-react/scopes-and-lifecycle/ · https://zfb.takazudomodular.com/zudo-react/reactivity/
- https://zfb.takazudomodular.com/api/define-config/ · https://zfb.takazudomodular.com/concepts/plugins/ · https://zfb.takazudomodular.com/api/cli/
- https://zfb.takazudomodular.com/zudo-wind/coming-from-tailwind/ (why BEM `__` and `@import` placement matter even with `wind: false`)
- https://zfb.takazudomodular.com/changelog/zfb/v3.0.0/ · https://zfb.takazudomodular.com/changelog/zfb/v3.1.0/
- Takazudo/zudo-design-token-panel#1002 · Takazudo/zudo-front-builder#3365 (next release) · sibling guide `Takazudo--zudo-design-token-panel.md` (zdtp package + playground), `Takazudo--zudo-design-token-panel-example-zfb-tailwind.md`
