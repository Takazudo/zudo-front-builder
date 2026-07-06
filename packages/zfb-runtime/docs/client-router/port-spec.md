# W1B — Per-module port spec (Astro ClientRouter → zfb-runtime) + island-lifecycle contract

**Wave:** 1 (Strategy B port, epic zudolab/zudo-doc#1510, sub-issue #1512)
**Author agent:** W1B
**Date:** 2026-05-07
**Status:** Settled. W3A–W3D children should implement directly from this spec without further design judgment.

---

## Guiding principle (re-stated, normative)

**Replicate, do not redesign.** Per the May-6 backside-migration retro in `l-lessons-zfb-migration-parity` ("default contract is replicate, do not redesign"), the zfb client-router mirrors Astro's structure file-by-file, function-by-function. Naming changes (`astro:` → `zfb:`, `data-astro-*` → `data-zfb-*`) are mechanical, not creative. **Any deviation from Astro's structure must have a NAMED TECHNICAL CAUSE recorded in this spec.** "Looks cleaner" or "more idiomatic for Preact" are NOT acceptable causes.

W3A–W3D children inherit this rule; they self-check by grepping their own diff for un-mapped Astro symbols.

---

## 0. Source files audited

Read in full. Line counts confirmed:

| File | Lines |
|------|-------|
| `packages/astro/components/ClientRouter.astro` | 155 |
| `packages/astro/src/transitions/router.ts` | 745 |
| `packages/astro/src/transitions/swap-functions.ts` | 271 |
| `packages/astro/src/transitions/events.ts` | 204 |
| `packages/astro/src/transitions/cssesc.ts` | 103 |
| `packages/astro/src/transitions/index.ts` | 78 (public API surface — animations only, NOT router) |
| `packages/astro/src/transitions/types.ts` | 10 |
| `packages/astro/src/transitions/vite-plugin-transitions.ts` | 66 |

zfb-side context audited:

- `packages/zfb-runtime/src/index.ts` (current public exports)
- `packages/zfb-runtime/src/view-transitions.ts` (existing typed no-op, kept for shim compat)
- `packages/zfb-runtime/src/router.ts` (server-side Hono page router — UNRELATED to this work; named "router" but it's the SSR dispatcher, not the client router)
- `packages/zfb/src/island.ts` (Island JSX wrapper, marker attrs `data-zfb-island`, `data-zfb-island-skip-ssr`, `data-props`)
- `packages/zfb/src/runtime.ts` (`mountIslands`, `scheduleHydrate`, `mounted`/`pending` WeakSet guards)

---

## 1. Module structure (decision #1)

**Decision:** create `packages/zfb-runtime/src/client-router/` subdirectory holding the per-module ports, plus a top-level `<ClientRouter />` Preact component file at `packages/zfb-runtime/src/client-router.tsx` (or `client-router/index.tsx`; see naming note below).

```
packages/zfb-runtime/src/
├── client-router/
│   ├── cssesc.ts          ← port of transitions/cssesc.ts
│   ├── events.ts          ← port of transitions/events.ts
│   ├── swap-functions.ts  ← port of transitions/swap-functions.ts
│   ├── router.ts          ← port of transitions/router.ts
│   ├── types.ts           ← port of transitions/types.ts
│   └── index.ts           ← barrel re-exports (public API surface, decision #2)
├── client-router.tsx      ← <ClientRouter /> Preact component (replaces ClientRouter.astro)
├── view-transitions.ts    ← KEEP (typed no-op for back-compat; do NOT delete)
└── index.ts               ← top-level package barrel; re-exports from client-router/
```

**Naming note (DEVIATION from Astro, with cause):**
Astro's source lives under `src/transitions/` because Astro uses "transitions" as the integration name. zfb's existing `view-transitions.ts` is already a typed no-op with that name. We name the new directory `client-router/` (not `transitions/`) because:

- **Named technical cause:** the existing `view-transitions.ts` symbol is the public *deprecated* compatibility shim. Reusing the name "transitions" for the new live router would either (a) shadow the existing symbol and break any consumer still using `<ViewTransitions />`, or (b) require renaming the deprecated file, which is a forbidden API break. Naming the new directory `client-router/` keeps both layers exportable side-by-side. Functionally it also matches Astro's runtime-internal nomenclature: Astro calls the runtime `ClientRouter` even though the source folder is `transitions/`.

**Top-level file naming:** `client-router.tsx` for the Preact component (singular .tsx file, JSX-bearing) sits beside the `client-router/` directory. If TypeScript's `moduleResolution` policy makes the dir-and-file pairing fragile, we may instead put the component at `client-router/component.tsx` and re-export from `client-router/index.ts`. W3A picks one — either is acceptable, no further design judgment needed; they are equivalent under "replicate, do not redesign" because Astro itself has the component file (`ClientRouter.astro`) outside the runtime sources. Default: `client-router.tsx` at top level.

> **Post-spec note (2026-06):** The shipped file is `client-router.ts` (plain
> `.ts`, not `.tsx`). JSX syntax is avoided; head nodes are minted by calling
> `jsx` from `react/jsx-runtime` directly so the file needs no `tsconfig` JSX
> pragma change and the engine's alias-rewrite (`react/jsx-runtime` →
> `preact/jsx-runtime` in Preact mode) handles framework selection.
> Additionally, the spec's "final decision for W3A" describing a separate
> `client-router/init.ts` module was not the shipped approach — instead
> `init()` lives in `client-router/router.ts` and is called as a side effect
> on first import of `client-router.ts` (guarded by the `initialized` flag in
> `router.ts`). There is no `client-router/init.ts` file.

---

## 2. Public API surface (decision #2)

The `@takazudo/zfb-runtime` top-level `index.ts` adds (alongside existing exports):

```ts
// Components
export { ClientRouter, type ClientRouterProps } from "./client-router.tsx";

// Imperative API (mirrors astro:transitions/client)
export { navigate, supportsViewTransitions, transitionEnabledOnThisPage } from "./client-router/router.ts";

// Lifecycle event classes + constants (mirrors astro:transitions/client events)
export {
  TRANSITION_BEFORE_PREPARATION,
  TRANSITION_AFTER_PREPARATION,
  TRANSITION_BEFORE_SWAP,
  TRANSITION_AFTER_SWAP,
  TRANSITION_PAGE_LOAD,
  TransitionBeforePreparationEvent,
  TransitionBeforeSwapEvent,
  isTransitionBeforePreparationEvent,
  isTransitionBeforeSwapEvent,
} from "./client-router/events.ts";

// Swap functions (advanced consumers can replace individual swap steps)
export { swapFunctions, swap } from "./client-router/swap-functions.ts";

// Types
export type { Direction, Fallback, NavigationTypeString, Options } from "./client-router/types.ts";
```

> **Post-spec note (2026-06):** The shipped `index.ts` exports a sixth event
> constant not listed in this spec: `TRANSITION_NAVIGATION_ABORTED =
> "zfb:navigation-aborted"` (see `client-router/events.ts:10` and
> `index.ts:54`). The constant is exported alongside the five constants above.
> Additionally, the spec code block above references `"./client-router.tsx"`;
> the shipped file is `"./client-router.js"` (compiled from `client-router.ts`).

The existing `ViewTransitions` typed no-op stays exported as a deprecated alias. Note: it is NOT replaced by `ClientRouter`; the names are separate and the deprecation comment on `ViewTransitions` already explains the migration path (CSS `@view-transition` at-rule), so consumers can keep it mounted while they switch.

`<ClientRouter />` props (mirrors Astro's `Props`):

```ts
export interface ClientRouterProps {
  fallback?: "none" | "animate" | "swap"; // default "animate"
}
```

The component renders:

1. A scoped `<style is:global>` equivalent — emit a global `<style>` tag with the `.zfb-route-announcer` class (renamed from `.astro-route-announcer`). The style is global because the announcer `<div>` is appended to `document.body` at runtime, not under any Preact-controlled subtree.
2. `<meta name="zfb-view-transitions-enabled" content="true" />`
3. `<meta name="zfb-view-transitions-fallback" content={fallback} />`
4. A `<script>` with the click + submit interceptors, identical structure to Astro's inline `<script>`.

**Naming note:** Astro's `<style is:global>` is an Astro-template directive. In Preact, just emit a plain `<style>` element — the CSS rule is a class selector that targets a runtime-appended `<div>`, so a non-scoped `<style>` is the literal port (no scoped-class transform happens in either case).

---

## 3. Lifecycle event names (decision #3)

Mirror Astro's `astro:*` namespace one-for-one with `zfb:`:

| Astro | zfb (this port) |
|-------|-----------------|
| `astro:before-preparation` | `zfb:before-preparation` |
| `astro:after-preparation` | `zfb:after-preparation` |
| `astro:before-swap` | `zfb:before-swap` |
| `astro:after-swap` | `zfb:after-swap` |
| `astro:page-load` | `zfb:page-load` |

Constants in `client-router/events.ts`:

```ts
export const TRANSITION_BEFORE_PREPARATION = "zfb:before-preparation";
export const TRANSITION_AFTER_PREPARATION = "zfb:after-preparation";
export const TRANSITION_BEFORE_SWAP = "zfb:before-swap";
export const TRANSITION_AFTER_SWAP = "zfb:after-swap";
export const TRANSITION_PAGE_LOAD = "zfb:page-load";
```

**Why mirror namespace, not condense:** Astro user code in the wild listens on `document.addEventListener("astro:page-load", ...)`. Anyone porting from Astro to zfb does a one-time string replace `astro:` → `zfb:`. Any other naming (`zfb:router:page-load`, `view-transition:page-load`, etc.) breaks that mechanical migration with no benefit.

**Justification one-liners:**

- `before-preparation` / `after-preparation`: hooks around the fetch + DOMParser step. Verbatim from Astro.
- `before-swap` / `after-swap`: hooks around the actual `swap()` call. Verbatim from Astro.
- `page-load`: fired after scripts re-run + new page is announced. Verbatim from Astro.

---

## 4. Persist directive name (decision #4)

**Decision:** `data-zfb-transition-persist="<id>"` — direct mirror of Astro's `data-astro-transition-persist`.

**Collision grep result (mandatory check):**

```
$ rg -n "data-zfb-transition-persist|zfb:transition" \
    <consumer-checkout> \
    <zfb-checkout> 2>/dev/null
(empty)
```

**No collisions in either codebase.** Name is safe to commit.

Companion attribute (mirrors Astro's `data-astro-transition-persist-props`):

- `data-zfb-transition-persist-props="false"` — when present, the swap function copies new `props` into the persisted island marker so it can re-render with refreshed data. Default behavior (attribute absent or `"false"`) IS to copy. Astro's `shouldCopyProps` returns `true` when the value is `null` (absent) or `"false"`, both meaning "copy". Replicate verbatim.

---

## 5. Fallback attribute names (decision #5)

| Astro | zfb |
|-------|-----|
| `data-astro-transition` (set on `<html>` to direction) | `data-zfb-transition` |
| `data-astro-transition-fallback` (set on `<html>` to "old"/"new") | `data-zfb-transition-fallback` |
| `data-astro-rerun` | `data-zfb-rerun` |
| `data-astro-exec` | `data-zfb-exec` |
| `data-astro-history` (on `<a>` for replace-mode) | `data-zfb-history` |
| `data-astro-reload` (decision #6) | `data-zfb-reload` |
| `astro-route-announcer` (CSS class) | `zfb-route-announcer` |
| `astro-view-transitions-enabled` (meta name) | `zfb-view-transitions-enabled` |
| `astro-view-transitions-fallback` (meta name) | `zfb-view-transitions-fallback` |
| `astro-island` (custom-element local-name) | **(see decision #12 — zfb has no equivalent custom element; the persist branch that special-cases `astro-island` re-routes to `data-zfb-island` markers)** |

`NON_OVERRIDABLE_ASTRO_ATTRS` becomes `NON_OVERRIDABLE_ZFB_ATTRS = ['data-zfb-transition', 'data-zfb-transition-fallback']`.

**zfb addition (deviation #11):** `swapRootAttributes` also preserves any `<html>` attribute named in a `<meta name="zfb-preserve-html-attrs" content="…">` tag, which the new `<ClientRouter preserveHtmlAttrs={[…]} />` prop emits. This is the public, declarative way for a consumer to keep a *runtime* `<html>` attribute (set from a persisted island, e.g. `data-theme` / `data-sidebar-hidden` driven from `localStorage`) across swaps — without it, the incoming SSR document's defaults wipe it on every navigation (zudolab/zudo-doc#2200 → Takazudo/zudo-front-builder#1103). The preserve-set is `NON_OVERRIDABLE_ZFB_ATTRS ∪ the meta names`, so with no meta the behavior is byte-identical to Astro's. The before-swap escape hatch — mutating `event.newDocument.documentElement` inside a `zfb:before-swap` listener — remains available for computed/dynamic cases.

**Consumer contract:** `swapRootAttributes` runs before `swapHeadElements`, so it reads the *current* (outgoing) page's `<meta name="zfb-preserve-html-attrs">`. The prop must therefore carry the **same value on every page** that participates in SPA navigation — a page that omits an entry drops that attribute when navigating away from it (the same site-wide-static assumption as the `zfb-prefetch-disabled` meta, but easier to violate since this is a per-mount array prop). Names are matched case-insensitively: `consumerPreservedAttrs()` lowercases the meta content because the DOM exposes `<html>` attribute names lowercased.

`PERSIST_ATTR`, `DIRECTION_ATTR`, `OLD_NEW_ATTR` constants in `swap-functions.ts` and `router.ts` rename mechanically.

`VITE_ID = 'data-vite-dev-id'` — KEEP verbatim. This is Vite's own attribute, not Astro's, and zfb's dev pipeline goes through Vite too. (Note: in zfb the equivalent dev-pass-through case may not occur because zfb doesn't have a Vue dev pipeline; see decisions #8 and #9.)

---

## 6. Reload opt-out (decision #6)

`data-zfb-reload` — direct mirror of `data-astro-reload`. Used in `ClientRouter.astro`'s `isReloadEl()` check; ports verbatim into the new `<ClientRouter />` script body. Same JS pattern: `el.dataset.zfbReload !== undefined`.

---

## 7. Prefetch integration (decision #7) — DEFERRED

**Decision:** Out of scope for v1. Follow-up issue created.

**Tracker issue:** zudolab/zudo-doc#1527 — `[VT Strategy B][Followup] Port Astro prefetch module to zfb-runtime`
URL: https://github.com/zudolab/zudo-doc/issues/1527

The new `<ClientRouter />` script does NOT include the Astro `init({ prefetchAll: true })` call. The Vite plugin's `__PREFETCH_DISABLED__` substitution is also out of scope (decision #9 expands).

If user wants prefetch in v1: raise during W2A confirm gate, W3C absorbs.

> **Post-spec note (2026-06):** Prefetch shipped post-spec via issue #276.
> `packages/zfb-runtime/src/client-router/prefetch.ts` was added, exporting
> `prefetch` and `init` (re-exported as `prefetchInit` from `index.ts`).
> `<ClientRouter prefetchAll>` triggers `prefetchInit({ prefetchAll: true })`
> via a module-level guard in `client-router.ts`. The `./client-router` subpath
> also exposes the prefetch surface. The `__PREFETCH_DISABLED__` Vite gate
> was not needed — the guard is a runtime boolean check.

---

## 8. Vue-scoped style ID handling (decision #8)

Astro's `vueScopedStyleId(el: HTMLStyleElement)` reads `el.dataset.viteDevId`, parses it as a URL, and checks for `?vue&type=style&scoped` query params. If matching, returns the dev ID (used to dedupe Vue scoped styles across navigations in DEV). The `knownVueScopedStyles` Map preserves transformed styles whose textContent has been mutated by Vue's `:deep()` rewriting.

**Decision:** **PORT VERBATIM as a no-op equivalent.** Reasoning:

- zfb does not ship Vue. The query-param check (`searchParams.get('vue') !== null`) will never match, so `vueScopedStyleId` will always return `''`.
- BUT — keeping the function (renamed to a more accurate `viteDevScopedStyleId` if desired, or kept as `vueScopedStyleId` for line-for-line replication) costs nothing at runtime (one URL parse on a small attribute), and the `knownVueScopedStyles` Map dedup logic is still correct: if zfb later adds a similar dev-time scoped-style transform (Tailwind v4 `@layer`, scoped CSS modules, etc.), the slot is already in place.
- **Named technical cause for keeping the dead-looking branch:** insurance against a future zfb dev plugin that uses `data-vite-dev-id` for transformed styles. The cost of removing and re-adding later is greater than the cost of a single URL-parse-that-returns-empty per `<style>` element on swap.

**Recommendation:** keep the function name `vueScopedStyleId` (replicate-don't-redesign rule). If a reviewer pushes back on the "Vue" prefix in zfb code, rename to `viteDevScopedStyleId` — but do this in a follow-up cosmetic PR, NOT in W3 implementation.

The `import.meta.env.DEV` gate is preserved verbatim. zfb's bundler exposes `import.meta.env.DEV` (Vite-style) the same way Astro does — confirmed via existing zfb-runtime code that uses `process.env["NODE_ENV"]` and `import.meta` patterns interchangeably.

---

## 9. Vite plugin requirement (decision #9)

Astro's `vite-plugin-transitions.ts`:

1. Resolves the virtual module IDs `astro:transitions` and `astro:transitions/client` (used by `import { navigate } from 'astro:transitions/client'` in user code).
2. Treeshakes the prefetch import: when `settings.config.prefetch === false`, replaces the literal string `__PREFETCH_DISABLED__` with `true` so the gated `init(...)` call in `ClientRouter.astro` is dropped.

**Decision:** zfb does NOT need the virtual-module resolver. zfb is a static-import / package-export world: consumers import directly from `@takazudo/zfb-runtime` as a regular package. There is no `zfb:transitions/client` virtual-module convention.

The prefetch treeshake hook is also unnecessary in v1 because **prefetch is deferred (decision #7)** — there is no `init({ prefetchAll: true })` call in the v1 `<ClientRouter />` script, so there is nothing to gate.

**Out of scope for v1: vite-plugin-transitions equivalent.** When prefetch lands (issue #1527), a small zfb plugin in `@takazudo/zfb` will be added at that time to gate the prefetch import.

**Named technical cause:** zfb's package surface is direct-import; Astro uses virtual modules because `astro:` is reserved by the Astro integration system. There is nothing for the plugin to do in zfb v1.

---

## 10. Style preload — `preloadStyleLinks()` (decision #10)

**Decision:** PORT VERBATIM into `client-router/router.ts`.

`preloadStyleLinks(newDocument)` walks `head link[rel=stylesheet]` in the new document, and for each href that is NOT already present in the current page (matched by `data-zfb-transition-persist` ID OR by `href` literal), emits a `<link rel=preload as=style href=...>` into the current head and returns a Promise that resolves on `load` or `error`.

The router awaits all these preloads inside `defaultLoader` so the swap doesn't FOUC. This is core to the "no flash on swap" behavior — skipping it visibly degrades the experience.

The function is small (~25 lines), has no Astro-specific dependencies, and the persist-ID branch already accepts the renamed `data-zfb-transition-persist` attribute.

**Replicate verbatim.**

---

## 11. STOP gate trigger phrasing (decision #11)

> If during W3A–W3D implementation a child agent encounters a **substantial upstream runtime change requiring maintainer signoff** — e.g. Astro's router introduces a new lifecycle event class, a new persist-element protocol, or a fundamentally different swap algorithm in a version we'd be tracking — **STOP**. Do not silently re-design. File a comment on the parent epic (#1510) describing:
>
> 1. The Astro symbol that has materially changed shape (cite line ranges in the upstream Astro source).
> 2. The minimal port that would still mechanically replicate the change.
> 3. Why "replicate-don't-redesign" still applies vs. an exception.
>
> Wait for maintainer ack before proceeding.

**Phrasing rationale:** the trigger is "**substantial upstream runtime change requiring maintainer signoff**" (large surface area + browser-behavior subtlety + maintainer review burden). It is NOT "material risk in product terms" — the feature is opt-in (consumers must explicitly mount `<ClientRouter />`), so a regression there does not break consumers who haven't opted in. The bar is reviewer attention budget, not user-facing impact.

---

## 12. Island-lifecycle contract (decision #12) — CRITICAL

This is the load-bearing decision. W3D implements against this; W6B writes tests against this.

### 12.1 Background — current zfb island bootstrap

(Read from `packages/zfb/src/runtime.ts` lines 240–414.)

zfb's `mountIslands(manifest)` is called once at script load. It:

1. `document.querySelectorAll("[data-zfb-island]")` — for SSR-hydrated islands, calls `scheduleMount(..., "hydrate")`.
2. `document.querySelectorAll("[data-zfb-island-skip-ssr]")` — for SSR-skip islands (Astro `client:only`-equivalent), calls `scheduleMount(..., "render")`.

`scheduleMount` is guarded by two module-level WeakSets: `mounted` (already mounted) and `pending` (dynamic-import in flight). Each guard is keyed by `Element`. If an element passes both guards, the manifest is consulted and either an inline `mount()` is called (shared-bundle path) or a dynamic `import(url)` chain runs (per-island bundle path).

`scheduleHydrate(target, when, fire)` defers the actual mount based on `data-when` (`"load"`, `"idle"`, `"visible"`).

**Idempotency status:** `mountIslands` IS already idempotent at the *element* level: a given DOM element is mounted at most once. **But it is NOT idempotent at the document level after a body swap** — see 12.3.

### 12.2 Re-hydration after `swapBodyElement`

**Contract:** after `swap-functions.ts:swap(doc)` finishes (which invokes `swapBodyElement`), the client-router router code MUST trigger an island re-bootstrap walk on the new body before dispatching `zfb:after-swap`.

**Implementation requirement (W3D):**

1. Add a new public entry point in `@takazudo/zfb`'s runtime export: `mountNewIslands()` (or similar) that walks ONLY the current document body for `[data-zfb-island]` and `[data-zfb-island-skip-ssr]` markers and runs `scheduleMount` against them. It re-uses the same module-level `mounted` and `pending` WeakSets.
   - **Why a new function vs. just calling `mountIslands(manifest)` again:** the existing `mountIslands` requires the manifest to be passed by the caller. The router should not need to know the manifest. Solution: store the manifest on first call (as a module-level captured `lastManifest` variable inside `mountIslands`) and expose `mountNewIslands()` that re-uses the captured value. **Named technical cause:** the router lives in `@takazudo/zfb-runtime`, the islands runtime lives in `@takazudo/zfb`; passing the manifest through the swap event would require either widening the event API or threading manifest into router options. The captured-manifest pattern keeps the boundary clean and matches the existing module-level singleton design (`mounted`, `pending`, `importImpl`).

2. Call `mountNewIslands()` from inside `client-router/router.ts`'s `runScripts` step or immediately after `triggerEvent('zfb:after-swap')`. The exact ordering: run AFTER `swap()` mutates the DOM, AFTER `runScripts()` re-runs new inline scripts (so any new `mountIslands` registration the new page might have done at script-evaluation time has executed), and BEFORE `triggerEvent('zfb:page-load')`. **Pin point:** call `mountNewIslands()` between `runScripts()` and `onPageLoad()` in router's `currentTransition.viewTransition?.updateCallbackDone.finally(...)` block.

3. The dispatch is fire-and-forget; mount scheduling happens asynchronously (idle / visible) after this point and is not awaited.

### 12.3 Persisted islands — recommendation by zone

Per `swapBodyElement` (lines 87–137), elements carrying `data-zfb-transition-persist` are physically moved from the old body into the new body via DOM `moveBefore` (Chrome 133+) or `appendChild` + `replaceWith`. They are NOT detached and re-rendered.

For islands specifically (Astro's branch around line 124–132 detects `astro-island` by `localName`), Astro copies the new `props` attribute onto the persisted element when the new HTML had different props, then sets a `ssr` attribute to trigger re-render.

**zfb has no `astro-island` custom element.** zfb's Island wrapper renders a plain `<div data-zfb-island="ComponentName" data-when="..." data-props="...">`. So the persist-merge branch keys differently:

```
// Replace Astro's:
//   if (newTarget.localName === 'astro-island' && shouldCopyProps(...) && !isSameProps(...)) {
//     el.setAttribute('ssr', '');
//     el.setAttribute('props', newTarget.getAttribute('props')!);
//   }
// With zfb equivalent:
//   if (newTarget.matches('[data-zfb-island]') && shouldCopyProps(...) && !isSameProps(...)) {
//     el.setAttribute('data-props', newTarget.getAttribute('data-props')!);
//     // Trigger re-render — see 12.3 zone-by-zone below for what "trigger" means.
//   }
```

**Named technical cause for the deviation:** Astro's `<astro-island>` is a custom element; it observes the `props` attribute mutation via its `connectedCallback` / `MutationObserver` and re-runs `hydrate` when `ssr` is re-set. zfb's `<div data-zfb-island>` is NOT a custom element — it's a marker div. There is no auto re-render trigger; we must explicitly invoke the mount path again.

#### 12.3.1 Zones

- **Sidebar / header / footer / nav (chrome zones):** persisted via `data-zfb-transition-persist="<stable-id>"`. Recommendation **(a) skipped entirely** — DOM continuity preserved, component instance preserved, internal Preact state (e.g. expanded/collapsed nav state, focused item, scroll position inside scrollable nav) preserved. NO props refresh. The new body's matching marker is discarded by `swapBodyElement`'s `newTarget.remove()` step.
  - Boundary signal: site-chrome islands. Examples in zudo-doc: `sidebar-tree.tsx`, `theme-toggle.tsx`, `mobile-toc.tsx`. These never need fresh props per page; their state IS the user's reading session.

- **Content-area islands (`toc.tsx`, `doc-history.tsx`, `find-bar.tsx`, etc.):** Recommendation **(b) re-rendered with new props from the new HTML** — the user navigated to a new doc, so the TOC headings, history dropdown, etc. should reflect the new page. Implementation: do NOT carry `data-zfb-transition-persist` on these. Let `swapBodyElement` discard them with the old body, and let `mountNewIslands()` mount the fresh marker from the new body.
  - Boundary signal: anything whose props are derived from the current page's frontmatter or content.

- **Hybrid case — content-area islands that DO want state preservation across navs (rare):** carry `data-zfb-transition-persist` AND `data-zfb-transition-persist-props` (default — copy props). The persist-merge branch (above) updates `data-props` on the surviving element. W3D MUST then call `scheduleMount` against the persisted element with the new props so the component re-renders. Concretely: after the persist-merge branch updates `data-props`, push the affected element into a "needs-remount" queue, then in `mountNewIslands()` clear the `mounted` WeakSet entry for those queued elements before walking. **Caveat:** this is a niche path; if no zudo-doc island opts in for v1, W3D MAY mark this branch as `// TODO post-v1` with a unit test asserting it throws or no-ops cleanly. **Recommended for v1: leave the niche path unimplemented.** Document the gap in the v1 release notes.

  > **Post-spec note (2026-07, issue #1389):** SHIPPED — the hybrid path is now
  > implemented, not left unimplemented. The "needs-remount queue" is realized
  > **through the DOM**, not an in-memory array: `swapBodyElement`'s persist-merge
  > branch marks the surviving element with a `data-zfb-island-remount` attribute
  > (and refreshes `data-props`), and `@takazudo/zfb`'s `mountNewIslands()` consumes
  > that flag in `clearMountedForRemount()` — it fires the stale instance's unmount
  > thunk, drops the `mounted` entry, strips the flag, and lets the following
  > `scheduleMount` re-mount with the new props. A cross-package in-memory queue is
  > impossible because the `mounted` map is module-private to `runtime.ts` (a
  > different package from the swap functions); the flagged DOM node IS the queue.
  > The original Astro port set a bare `ssr` attribute here that nothing consumed
  > (zfb islands are marker divs, not `<astro-island>` custom elements) — #1389
  > replaces it with the namespaced `data-zfb-island-remount` flag and wires the
  > consumer. Two v1 limitations remain for this niche path (tracked as a
  > follow-up issue): a persisted island still mid-import at swap time, or one with
  > deferred `data-when`, does not remount seamlessly with fresh props.

#### 12.3.2 Boundary table (W3D pins this)

| Island | Persist? | Persist-props? | Behavior |
|--------|----------|----------------|----------|
| sidebar-tree | yes | no (state survives) | (a) DOM kept, no remount |
| sidebar-toggle | yes | no | (a) |
| theme-toggle | yes | no | (a) |
| mobile-toc | yes | no | (a) |
| ai-chat-modal | yes | no | (a) |
| toc | no | n/a | (b) discarded; remount fresh |
| doc-history | no | n/a | (b) |
| find-bar | yes (search state survives navs) | no | (a) |
| design-token-tweak panel | yes | no | (a) |

W3D applies this table by writing or updating `data-zfb-transition-persist="<id>"` attributes on the relevant island markers in zudo-doc's layout components. Stable ID convention: kebab-case island-component name (`sidebar-tree`, `theme-toggle`, etc.).

### 12.4 Idempotency of global island bootstrap

**Current state:** `mountIslands` IS idempotent per-element (WeakSet). It is NOT idempotent across full document re-walks because:

1. The WeakSets are module-level, so a re-call of `mountIslands(manifest)` with the same manifest re-walks the document but skips already-mounted elements. **Safe.**
2. **But:** if `mountIslands` is called from a per-page inline script (rather than once at the global runtime entry), each navigation that swaps in a new inline `mountIslands(...)` call from the new page's HTML would re-walk and re-mount any new markers — also safe per-element, but the manifest reference might differ.

**Conclusion for W3D:** confirm that `mountIslands` is called exactly ONCE at global runtime entry (the runtime bundle, `islands-runtime-<hash>.js`), NOT per-page-inline. Then introduce `mountNewIslands()` (no manifest arg) that captures the same manifest and is callable from the router. **If the current zfb pipeline embeds `mountIslands(...)` per page inline, fix it as part of W3D**: refactor to a single global call + module-level captured manifest.

### 12.5 Edge case — deferred-hydration islands navigated away before firing

An island with `data-when="idle"` or `data-when="visible"` schedules its hydration via `requestIdleCallback` or `IntersectionObserver`. If the user navigates away before the gate fires:

**Current state:** the `oneShot` gate's `cancel` function is returned by `scheduleHydrate`. Currently nothing in zfb keeps a reference to those cancel handles, so no cancel is called. The deferred fire still runs (rIC / observer), but on a swapped-out element that is no longer in the document.

**Failure modes if not handled:**

- **rIC fire on detached element:** `fire()` runs `mounted.add(element)` and calls `fn(props, element, mode)`. The component `hydrate(<X props />, element)` is called against a detached DOM node — Preact creates a new tree under the orphan, but it is invisible (the element is gone from the document tree). Effect: a small memory leak (orphan tree retained as long as something keeps the element alive — typically nothing, so GC eventually reclaims). Functionally harmless but wasteful.
- **IntersectionObserver fire on detached element:** observers don't fire for detached elements (entries report `isIntersecting: false`), so this case is naturally inert. No leak.
- **rIC for already-removed element:** same as the rIC fire case above.

**Decision (cleanup contract for W3D):**

1. Track the cancel handles. Inside `scheduleMount`, when `scheduleHydrate(element, when, fire)` returns its cancel function, store `(element, cancel)` in a module-level `Map<Element, () => void>` named `pendingCancels`.
2. On `zfb:before-swap` (or alternatively, in `swapBodyElement` itself for tighter coupling), iterate over the OLD body's `[data-zfb-island][data-when]:not([data-when="load"])` elements, look them up in `pendingCancels`, and call cancel(). Then clear those entries.
3. After the swap, the new body's deferred islands are scheduled fresh by `mountNewIslands()`.

**Named technical cause for cancel contract:** without it, deferred-fires can run against orphan elements and either leak memory (rIC) or warn loudly in dev (Preact's `hydrate` against detached DOM in dev mode). W6B should test this case explicitly.

### 12.6 Summary contract for W3D + W6B

Pinned in this order:

1. `mountIslands(manifest)` is called ONCE at runtime entry. Manifest is captured at module level.
2. `mountNewIslands()` (new export from `@takazudo/zfb`) re-walks the current body and mounts new markers using the captured manifest, no double-mount thanks to WeakSet.
3. The router calls `mountNewIslands()` after `swap()` + `runScripts()`, before `triggerEvent('zfb:page-load')`.
4. Persisted island markers (`data-zfb-transition-persist`) follow the boundary table in 12.3.2.
5. Persist-props branch is V1-out-of-scope; keep the marker check stub but no-op.
6. Deferred-hydration cancellation: track cancel handles in `pendingCancels: Map<Element, () => void>`; cancel for old body elements on `zfb:before-swap`.

> **Post-spec note (2026-07, issue #1389):** Items 4 and 5 are superseded by the
> shipped persist implementation. The pre-swap `unmountIslands()` walk now takes
> the incoming body as a second argument and **skips** any old-body island whose
> persist id matches an incoming marker — without this, the walk unmounted every
> island (persisted ones included) and emptied the container before
> `swapBodyElement` lifted it, so `data-zfb-transition-persist` preserved nothing
> (the case-(a) "component instance preserved" promise was never actually met).
> Item 5's "no-op stub" is now the fully-wired hybrid remount path — see the
> §12.3.1 post-spec note above.

W6B writes tests for: (1) mount-after-swap, (2) double-mount-prevention, (3) persisted-skip path, (4) deferred-cancel-on-swap.

---

## 13. Per-symbol Astro→zfb mapping

### 13.1 `cssesc.ts`

**Status:** PORT VERBATIM. No naming changes required (no `astro:` symbols inside).

Single export: `export default function cssesc(string, options)`. Used by Astro for safely escaping persist IDs into CSS selectors. zfb uses it the same way.

Lines: 103. Self-contained. No external deps.

### 13.2 `types.ts`

| Astro export | zfb export | Notes |
|--------------|------------|-------|
| `Fallback = 'none' \| 'animate' \| 'swap'` | same | verbatim |
| `Direction = 'forward' \| 'back'` | same | verbatim |
| `NavigationTypeString = 'push' \| 'replace' \| 'traverse'` | same | verbatim |
| `Options = { history?, info?, state?, formData?, sourceElement? }` | same | verbatim |

10 lines, ports as-is.

### 13.3 `events.ts`

| Astro symbol | zfb symbol | Mapping |
|--------------|------------|---------|
| `TRANSITION_BEFORE_PREPARATION = 'astro:before-preparation'` | `'zfb:before-preparation'` | rename string literal |
| `TRANSITION_AFTER_PREPARATION` | `'zfb:after-preparation'` | rename |
| `TRANSITION_BEFORE_SWAP` | `'zfb:before-swap'` | rename |
| `TRANSITION_AFTER_SWAP` | `'zfb:after-swap'` | rename |
| `TRANSITION_PAGE_LOAD` | `'zfb:page-load'` | rename |
| `triggerEvent(name)` | same | unchanged signature |
| `onPageLoad()` | same | unchanged |
| `class BeforeEvent` | same | rename internal event-type constructor argument string from `'astro:before-preparation'` to `'zfb:before-preparation'` (line 91) and `'astro:before-swap'` to `'zfb:before-swap'` (line 124) |
| `class TransitionBeforePreparationEvent` | same | unchanged shape; constructor change above |
| `class TransitionBeforeSwapEvent` | same | unchanged shape; constructor change above |
| `isTransitionBeforePreparationEvent` | same | uses renamed constant |
| `isTransitionBeforeSwapEvent` | same | uses renamed constant |
| `doPreparation` | same | dispatches renamed event |
| `updateScrollPosition` | same | unchanged |
| `doSwap` | same | unchanged |

**Deprecation comments:** Astro marks the constants as `@deprecated This will be removed in Astro 7`. zfb does NOT carry the deprecation note — these are NEW exports for zfb consumers, with no v0 → v1 deprecation path. Drop the `/** @deprecated */` JSDoc comments. **Named technical cause:** the deprecation is Astro-internal lifecycle; replicating it would mislead zfb consumers into thinking the API is unstable from day one.

> **Post-spec note (2026-06):** A sixth constant was added in the shipped
> implementation that is absent from this spec's mapping table:
> `TRANSITION_NAVIGATION_ABORTED = "zfb:navigation-aborted"` (see
> `packages/zfb-runtime/src/client-router/events.ts:10` and `index.ts:54`).
> It is exported from both `events.ts` and the top-level `index.ts`.

### 13.4 `swap-functions.ts`

| Astro symbol | zfb symbol | Mapping |
|--------------|------------|---------|
| `PERSIST_ATTR = 'data-astro-transition-persist'` | `'data-zfb-transition-persist'` | rename |
| `NON_OVERRIDABLE_ASTRO_ATTRS = [...]` | `NON_OVERRIDABLE_ZFB_ATTRS` | rename const + entries |
| `knownVueScopedStyles: Map<string, HTMLStyleElement>` | same | unchanged (decision #8 keeps as insurance slot) |
| `scriptsAlreadyRan: Set<string>` | same | unchanged |
| `detectScriptExecuted` | same | unchanged |
| `deselectScripts` | same | rename `data-astro-rerun` check to `data-zfb-rerun`; rename `dataset.astroExec` to `dataset.zfbExec` |
| `swapRootAttributes` | same | uses renamed `NON_OVERRIDABLE_ZFB_ATTRS` |
| `swapHeadElements` | same | uses `vueScopedStyleId` (kept verbatim per decision #8); persisted-head detection rebases on `data-zfb-transition-persist` |
| `swapBodyElement` | same | core logic verbatim; the `localName === 'astro-island'` branch (line 126) becomes `newTarget.matches('[data-zfb-island]')` per decision #12.3 |
| `attachShadowRoots` | same | verbatim — not Astro-specific |
| `saveFocus` / `restoreFocus` | same | uses `data-zfb-transition-persist` |
| `vueScopedStyleId` | same | kept verbatim (decision #8) |
| `persistedHeadElement` | same | uses renamed PERSIST_ATTR; the DEV-only Vue branch keeps; the inline-style preservation (lines 231–238) and font-preload branch (240–243) port verbatim |
| `shouldCopyProps` | same | reads `el.dataset.zfbTransitionPersistProps` |
| `isSameProps` | same | NOTE: zfb's persisted islands carry `data-props`, not `props`. So `oldEl.getAttribute('data-props') === newEl.getAttribute('data-props')` |
| `swapFunctions` | same | re-export bag |
| `swap` | same | top-level orchestrator |

**Deviation:** `isSameProps` reads `data-props` instead of `props`. **Named technical cause:** zfb's Island wrapper writes the SSR-serialized props to `data-props` (verified in `packages/zfb/src/island.ts:82`), not `props`. Astro's `<astro-island props=...>` is custom-element-driven; zfb's `<div data-zfb-island ... data-props=...>` is marker-driven. The attribute name is a different attribute, not just a renamed one.

### 13.5 `router.ts`

This is the largest port (~745 lines). All renames are mechanical:

- `data-astro-*` attribute reads → `data-zfb-*`
- Event-type literals `astro:*` → `zfb:*` (same as events.ts above)
- Meta-tag names `astro-view-transitions-*` → `zfb-view-transitions-*`
- CSS class `.astro-route-announcer` → `.zfb-route-announcer`
- `dataset.astroExec` → `dataset.zfbExec`
- `dataset.astroHistory` → `dataset.zfbHistory`
- `dataset.astroRerun` → `dataset.zfbRerun`

**Internal-fetch-headers shim (line 1):**

```ts
import { internalFetchHeaders } from 'virtual:astro:adapter-config/client';
```

This is Astro's adapter-injected internal-fetch headers (used for things like CF Pages worker auth). zfb has no equivalent virtual module.

**Decision:** drop the import; replace with `const internalFetchHeaders: Record<string, string> = {};`. The `Object.entries(internalFetchHeaders)` loop in `fetchHTML` iterates an empty object, so the headers logic is a no-op in v1.

**Named technical cause:** zfb adapters (`@takazudo/zfb-adapter-cloudflare`) do not currently expose a per-fetch internal-headers contract. If/when they do, a future zfb plugin can inject the equivalent. Empty object preserves call shape and keeps the future hook obvious.

**`prepareForClientOnlyComponents` (lines 692–745):**

This is the iframe-based pre-hydration trick for Astro's `client:only` components. The function only runs in DEV mode (`import.meta.env.DEV`). It:

1. Detects `astro-island[client='only']` in the new document.
2. Mounts the new page in a hidden iframe to coerce its `client:only` styles to be injected.
3. Cherry-picks `<style data-vite-dev-id="...">` elements from the iframe into the new document.

**Decision:** **DROP from v1 (skip — out of scope for v1).** Reasoning:

- It is gated by `import.meta.env.DEV`, so it's a dev-mode-only convenience. Not a correctness requirement.
- zfb's SSR-skip islands (`data-zfb-island-skip-ssr`) emit no DOM at SSR time; the styles they need are bundled into the islands runtime, not dev-injected per-route by Vite. So the underlying motivation (Vue/Vite per-component CSS injection only happens after hydration) does not apply.
- Adding it requires walking new-doc markers using `data-zfb-island-skip-ssr` and the iframe trick — significant code surface for a dev-mode-only payoff.

If a regression surfaces in DEV mode where `client:only`-equivalent islands lose styles across navigations, port at that point. Document in v1 release notes as a known limitation.

**Named technical cause:** zfb does not have the Vite-per-component-CSS-injection-on-hydrate behavior that motivates the iframe trick.

**Other notable router pieces:**

| Astro line | Behavior | zfb port |
|------------|----------|----------|
| `transitionEnabledOnThisPage()` | checks for `meta[name=astro-view-transitions-enabled]` | rename meta name |
| `getFallback()` | reads `meta[name=astro-view-transitions-fallback]` | rename meta name |
| `originalLocation` global | unchanged | verbatim |
| `currentHistoryIndex` global | unchanged | verbatim |
| `samePage`, `parser`, `runScripts`, `moveToLocation`, `preloadStyleLinks`, `updateDOM`, `transition`, `navigate`, `onPopState`, `onScrollEnd` | unchanged | verbatim with attribute/event renames |
| Top-level `if (inBrowser)` init block (lines 642–688) | unchanged | verbatim |
| Final `for (const script of document.getElementsByTagName('script'))` block (lines 684–688) | unchanged | verbatim — but `dataset.astroExec` becomes `dataset.zfbExec` |

### 13.6 `ClientRouter.astro` → `client-router.tsx`

Astro source is a `.astro` SFC with frontmatter, `<style is:global>`, two `<meta>` tags, and an inline `<script>` block.

zfb port emits the same DOM structure as a Preact functional component:

```tsx
export interface ClientRouterProps {
  fallback?: 'none' | 'animate' | 'swap';
}

const announcerCss = `
.zfb-route-announcer {
  position: absolute;
  left: 0;
  top: 0;
  clip: rect(0 0 0 0);
  clip-path: inset(50%);
  overflow: hidden;
  white-space: nowrap;
  width: 1px;
  height: 1px;
}
`;

const inlineScript = `/* IIFE-wrapped click+submit interceptors, mirroring Astro's <script> body */`;

export function ClientRouter({ fallback = 'animate' }: ClientRouterProps) {
  return (
    <>
      <style dangerouslySetInnerHTML={{ __html: announcerCss }} />
      <meta name="zfb-view-transitions-enabled" content="true" />
      <meta name="zfb-view-transitions-fallback" content={fallback} />
      <script dangerouslySetInnerHTML={{ __html: inlineScript }} />
    </>
  );
}
```

The inline script's body is a mechanical port of `ClientRouter.astro`'s `<script>` content (lines 27–155 of the Astro file), with these changes:

- Imports change from `import { ... } from 'astro:transitions/client'` to runtime-resolved access. **DEVIATION:** Astro's `<script>` block is processed by Vite which resolves the virtual module. zfb's inline `<script dangerouslySetInnerHTML>` is NOT processed by a bundler. **Named technical cause:** the Astro SFC's `<script>` is a build-time-bundled module; in Preact JSX we are emitting a raw `<script>` tag. Solution: write the inline script body assuming the runtime API is available on `window.__zfb_router` (or via direct package import resolved by the consumer's bundler when they emit the page, not by us). This is the only meaningful structural deviation in the port — see implementation note below.
- Drop the prefetch import + `init({ prefetchAll: true })` call (decision #7).
- Drop the `__PREFETCH_DISABLED__` Vite plugin gating (decision #9).
- `dataset.astroReload` → `dataset.zfbReload` (decision #6).
- `dataset.astroHistory` → `dataset.zfbHistory`.

**Implementation note for W3 children:** the cleanest port is to NOT inline the script. Instead, ship the `<ClientRouter />` component as a wrapper that emits the meta tags + the global stylesheet, and have a sibling top-level `client-router/init.ts` module that runs the click+submit interceptors. The init module is imported by the consumer's page-level entry script (e.g. zudo-doc imports `@takazudo/zfb-runtime/client-router/init` from its `src/main.ts` or `pages/_layout.tsx`). This sidesteps the inline-script bundling problem.

**Final decision for W3A:** emit the meta tags + style from `<ClientRouter />`, and put the click/submit interceptors in `client-router/init.ts` (auto-runs on import via top-of-module IIFE, side-effecting). The `<ClientRouter />` component MAY also render a `<script type="module" src="..." />` tag pointing at the runtime entry, OR rely on the consumer to import `init.ts` themselves. Default for zudo-doc integration: explicit import in the layout.

**Named technical cause for this structural deviation:** Astro processes the SFC `<script>` block through Vite's bundler at build time. Preact JSX inside zfb does not have an equivalent bundler-passed inline-script transform; emitting raw `<script dangerouslySetInnerHTML>` would bypass module resolution entirely. Splitting the JS interceptors into a separately-imported `init.ts` is the literal mechanical equivalent in a non-SFC framework. Astro's authors would do the same if they were not writing inside an Astro component.

> **Post-spec note (2026-06):** The shipped implementation differs from the
> spec's "Final decision for W3A" in two ways: (1) The file is `client-router.ts`
> (`.ts`, not `.tsx`) — JSX syntax is not used; `jsx` from `react/jsx-runtime`
> is called directly so no tsconfig JSX pragma is needed and the engine's
> `react/jsx-runtime` → `preact/jsx-runtime` alias handles framework selection.
> (2) There is no `client-router/init.ts` — the click/submit intercepts live in
> `client-router/router.ts` and are activated by calling `init()` there.
> `client-router.ts` calls `init()` as a side effect on first import (guarded by
> the `initialized` flag in `router.ts`). No inline `<script>` tag is emitted.

### 13.7 `vite-plugin-transitions.ts` — SKIP

Out of scope for v1 (decision #9). No port.

### 13.8 `transitions/index.ts` (Astro animations only)

Astro's `transitions/index.ts` is the **public API surface for `astro:transitions`** in user space — it exports `slide()` and `fade()` animation factories used in CSS `view-transition-name` configurations. **This is not the router; this is animation helpers.**

**Decision: SKIP — out of scope for v1.**

**Named technical cause:** zfb's view-transitions CSS approach (per existing `view-transitions.ts` docstring) uses the native CSS `@view-transition { navigation: auto; }` at-rule + per-element `view-transition-name` declarations. Animation helpers are CSS authoring sugar, not router functionality. They can ship as a separate convenience export later without affecting the router contract.

If user wants `slide()`/`fade()` helpers in v1: trivial port (~80 lines), can be added at any time.

---

## 14. Out-of-scope-for-v1 appendix

| Item | Decision section | Tracker |
|------|------------------|---------|
| Prefetch module (`astro/virtual-modules/prefetch.js`) | #7 | zudolab/zudo-doc#1527 |
| Vite plugin equivalent (virtual modules + prefetch gate) | #9 | (rolled into #1527) |
| `prepareForClientOnlyComponents` iframe trick (DEV-mode CSS hoist) | #13.5 | inline TODO |
| Animation helpers (`slide()`, `fade()`) | #13.8 | inline TODO — trivial follow-up |
| Persisted-island props-refresh path | #12.3.1 hybrid case | inline TODO; W3D leaves stub |
| `/** @deprecated */` JSDoc on event constants | #13.3 | (intentional drop) |
| `internalFetchHeaders` adapter integration | #13.5 | inline TODO; consumed when adapter exposes it |

> **Post-spec note (2026-06):** The first two rows (prefetch module, Vite plugin
> prefetch gate) shipped post-spec via issue #276 and are no longer deferred.
> See the §7 post-spec note above for details.

---

## 15. Deviations from pure replication — summary

For audit. Each deviation has a NAMED TECHNICAL CAUSE per the guiding principle.

| # | Deviation | Named technical cause |
|---|-----------|----------------------|
| 1 | Module dir named `client-router/`, not `transitions/` | Existing `view-transitions.ts` typed-no-op shim would shadow; renaming it is a forbidden API break. |
| 2 | `<ClientRouter />` script body lives in a separate `init.ts` module instead of inline `<script>` block | Astro SFC `<script>` is bundler-processed; Preact JSX has no equivalent transform — `init.ts` is the literal mechanical equivalent. |
| 3 | `isSameProps` reads `data-props` instead of `props` | zfb islands are marker-divs (`data-props`), not custom elements (`props`). Different attribute, not a rename. |
| 4 | `localName === 'astro-island'` branch becomes `matches('[data-zfb-island]')` | Same root cause as #3. |
| 5 | `internalFetchHeaders` import dropped, replaced with `{}` | zfb adapters don't expose this contract. Empty object preserves call shape. |
| 6 | `prepareForClientOnlyComponents` skipped | DEV-mode-only Vue/Vite-CSS-injection workaround; zfb islands inject CSS via the bundle, not per-component-on-hydrate. |
| 7 | Drop `/** @deprecated */` JSDoc on `TRANSITION_*` event constants | Astro-internal lifecycle marker; misleading for new zfb consumers. |
| 8 | Drop prefetch + `__PREFETCH_DISABLED__` gating | Decision #7 — out of scope for v1. |
| 9 | New `mountNewIslands()` export from `@takazudo/zfb` (no manifest arg, captures from first `mountIslands` call) | Router lives in zfb-runtime, islands manifest lives in zfb. Captured-manifest pattern avoids threading manifest through the router. |
| 10 | `pendingCancels: Map<Element, () => void>` for deferred-hydration cancel-on-swap | Without it, deferred-fires (rIC / observer) run against orphan elements — memory leak + Preact dev warnings. |
| 11 | `swapRootAttributes` preserves a consumer-configurable attribute set — `NON_OVERRIDABLE_ZFB_ATTRS` ∪ the names in `<meta name="zfb-preserve-html-attrs">` (emitted by the new `<ClientRouter preserveHtmlAttrs>` prop) | Astro has no consumer-extensible preserve-list. Runtime `<html>` attributes a consumer sets from a persisted island (`data-theme`, `data-sidebar-hidden` from `localStorage`) were wiped on every swap — Takazudo/zudo-front-builder#1103. Additive + opt-in: with no meta the set is exactly `NON_OVERRIDABLE_ZFB_ATTRS`, byte-identical to Astro's behavior. |

All other Astro symbols port verbatim with mechanical `astro:` → `zfb:` and `data-astro-*` → `data-zfb-*` renames.

---

## 16. Implementation handoff

W3A — `<ClientRouter />` component + `init.ts` + `cssesc.ts` + `types.ts` + `events.ts` (the foundation files).
W3B — `swap-functions.ts` + per-symbol attribute renames + persisted-island branch.
W3C — `router.ts` (the core 745-line port; orchestrates W3A + W3B).
W3D — Island lifecycle integration: `mountNewIslands`, `pendingCancels`, persist-zone wiring on zudo-doc layouts (per 12.3.2 boundary table). Idempotency audit of current `mountIslands` call sites.

W3D depends on W3A–W3C completion of router infrastructure. W3A–W3C can run in parallel.

W6B writes lifecycle tests against contract sections 12.5 + 12.6.

---

## 17. References

- Astro source: `withastro/astro` — `packages/astro/src/transitions/` and `packages/astro/components/ClientRouter.astro`
- zfb-runtime: `packages/zfb-runtime/src/` (this directory)
- zfb islands runtime: `packages/zfb/src/runtime.ts`
- zfb Island wrapper: `packages/zfb/src/island.ts`
- Downstream tracking epic: zudolab/zudo-doc#1510
- Sub-issue: zudolab/zudo-doc#1512
- Prefetch follow-up: zudolab/zudo-doc#1527

---

*End of W1B port spec. Children should consider every decision settled. STOP gate triggers per section 11.*
