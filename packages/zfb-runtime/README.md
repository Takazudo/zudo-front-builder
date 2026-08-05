# @takazudo/zfb-runtime

> Rust-built static-site engine for Astro and Next.js users — millisecond rebuilds, single binary.

The JS-side runtime for [zfb][zfb-site]'s SSG-first build pipeline. It
exposes `createPageRouter`, a Hono-backed page router whose returned
fetch handler is shape-compatible with the Cloudflare Workers
`(request) => Promise<Response>` model. The Rust build host drives this
router at build time to emit static HTML; the same handler also serves
SSR requests at the edge.

Full documentation: <https://takazudomodular.com/pj/zudo-front-builder/>.
Source: <https://github.com/Takazudo/zudo-front-builder>.

[zfb-site]: https://takazudomodular.com/pj/zudo-front-builder/

## Install

```sh
npm install @takazudo/zfb-runtime @takazudo/zfb
# or: pnpm add @takazudo/zfb-runtime @takazudo/zfb
```

`@takazudo/zfb` is a peer dependency — `createPageRouter` shares
module-level state (`ContentSnapshot`) with `@takazudo/zfb/content`, so
both packages must resolve to the same instance.

## What this package does

zfb's render pipeline goes:

```
user pages/ + content/ + layouts/ + components/
  → esbuild bundle                     // single ESM file, Worker entry
  → embedded V8 host                   // same WinterCG surface as CF Workers
  → @takazudo/zfb-runtime              // <-- this package
  → (request) => Promise<Response>     // Worker fetch handler
```

This package supplies the page-router factory the Worker entry calls. It
is built on [Hono][hono] but does not leak Hono types through the public
surface — consumers import from `src/index.ts`, which re-exports the
page-router types, the client-router and prefetch API, lifecycle event
constants, and plugin types, none of which require Hono.

[hono]: https://hono.dev/

The runtime is **JSX-runtime-agnostic**. It never imports preact or
react. The caller passes a `FrameworkAdapter` that pins `renderToString`
to the chosen JSX runtime; both `preact-render-to-string` and
`react-dom/server` slot in. `preact/compat` is a supported target, and
the `react` peer dependency is **optional**
(`peerDependenciesMeta.react.optional`) — a preact-only consumer does
not need `react` installed and does not need `auto-install-peers=true`.

## Public API

`createPageRouter` is **server-only** — it builds a Hono app, so it lives at
the `@takazudo/zfb-runtime/server` subpath. Keeping it out of the client-safe
`.` barrel means an island importing the bare `@takazudo/zfb-runtime` never
drags `hono` into a `--platform=browser` bundle (issue #1298). Its *types*
remain importable from the root barrel (they carry no runtime).

```ts
import { createPageRouter } from "@takazudo/zfb-runtime/server";
import type {
  CreatePageRouterOptions,
  PageDefinition,
  PageModule,
  PageHeading,
  PageRouter,
  FrameworkAdapter,
  ContentSnapshot,
  EntrySnapshot,
} from "@takazudo/zfb-runtime";
```

### Client-router / view-transitions / prefetch

```ts
import {
  ClientRouter,               // framework-agnostic <head> helper — enables view-transition intercepts
  navigate,                   // imperative navigation
  supportsViewTransitions,    // browser capability check
  transitionEnabledOnThisPage, // reads zfb-view-transitions-enabled meta
  prefetch,                   // prefetch a URL on demand
  prefetchInit,               // bootstrap prefetch strategy (e.g. { prefetchAll: true })
  TRANSITION_BEFORE_PREPARATION,
  TRANSITION_AFTER_PREPARATION,
  TRANSITION_BEFORE_SWAP,
  TRANSITION_AFTER_SWAP,
  TRANSITION_PAGE_LOAD,
  TRANSITION_NAVIGATION_ABORTED,
  swapFunctions,              // swap step overrides for advanced consumers
  swap,
} from "@takazudo/zfb-runtime";
```

The `./snapshot` and `./client-router` subpath exports are also available for
consumers that only need part of the surface:

```ts
import type { ContentSnapshot } from "@takazudo/zfb-runtime/snapshot";
import { ClientRouter } from "@takazudo/zfb-runtime/client-router";
```

#### Enabling SPA soft-navigation (the runtime ships automatically)

Mounting `<ClientRouter />` in your layout `<head>` is normally **all you need**.
The build detects the usage and automatically ships the client-router runtime to
the browser — you do **not** have to add a manual `"use client"` bootstrap
island.

```tsx
import { ClientRouter } from "@takazudo/zfb-runtime";

export default function Layout({ children }) {
  return (
    <html>
      <head>
        <ClientRouter fallback="animate" />
      </head>
      <body>{children}</body>
    </html>
  );
}
```

`ClientRouterProps`:

- `fallback?: "none" | "animate" | "swap"` controls the fallback strategy
  when native View Transitions are unavailable. The default is `"animate"`.
- `prefetchAll?: boolean` bootstraps same-origin link prefetching with the
  default hover strategy. Treat this as a site-wide router policy and mount
  the same value on every page that participates in SPA navigation.
- `preserveHtmlAttrs?: string[]` emits
  `<meta name="zfb-preserve-html-attrs">` so the swap step preserves
  runtime-owned `<html>` attributes such as `data-theme`. Mount the same
  list on every soft-navigable page: the router reads the outgoing page's
  meta before each swap, so a page that omits a name can drop that
  attribute when navigating away.
- `traverseRefetch?: boolean` emits
  `<meta name="zfb-traverse-refetch" content="true">` to opt out of the
  same-URL Back/Forward fast path for per-request SSR pages whose content
  can change at the same URL. Mount the same value on every page in that
  SPA navigation set so traversal behavior is deterministic.

**How the runtime reaches the browser.** `<ClientRouter />` itself only renders
SSR `<head>` tags. The click/form interception is registered by an `init()` call
that runs as a side effect when `@takazudo/zfb-runtime/client-router` is imported
in the browser, guarded so it never runs during SSR:

```ts
// inside @takazudo/zfb-runtime — client-router.ts
if (typeof document !== "undefined") {
  init();
}
```

To get that side-effect import into the client bundle, zfb's island scanner
detects when a page transitively reaches `<ClientRouter />` and injects
`import "@takazudo/zfb-runtime/client-router"` into the islands asset
(`assets/islands.js`) — **even when the project has no `"use client"` islands of
its own**. The asset is then loaded on the page, the import runs, and `init()`
fires on the client with zero boilerplate. (Auto-include added in zfb #289 /
#307; the runtime is byte-for-byte absent from projects that never reach
`<ClientRouter />`.)

**What counts as a `ClientRouter` usage the scanner detects:**

- a named `ClientRouter` import from the `@takazudo/zfb-runtime` barrel —
  `import { ClientRouter } from "@takazudo/zfb-runtime"` (renamed forms like
  `{ ClientRouter as CR }` match on the imported name), or
- any import or re-export of the `@takazudo/zfb-runtime/client-router` subpath
  (e.g. when you only need `navigate` / `prefetch`), or
- a named `ClientRouter` imported from a *local* barrel that re-exports the
  runtime via `export * from "@takazudo/zfb-runtime"`.

The importing module has to be reachable from a page in `pages/` — that is the
import graph the scanner walks.

**When you need a manual side-effect import.** Detection keys off the *import* of
`ClientRouter`, and a couple of shapes are deliberately **not** treated as a
trigger — firing on them would ship the runtime to projects that only reference
`ClientRouter` as a type or never call it:

- a namespace import — `import * as rt from "@takazudo/zfb-runtime"` used as
  `rt.ClientRouter`, and
- a type-only import — `import type { ClientRouter }` (or `{ type ClientRouter }`).

If soft-navigation is not working because your reference takes one of these forms
(or the mounting module is otherwise not reachable from a page), force the
runtime in with an explicit side-effect import from a page-reachable
`"use client"` island. The island renders nothing; running its bundle in the
browser fires the same `typeof document !== "undefined"` guard (`init()` is
idempotent, so reaching it again is a no-op):

```tsx
// src/components/client-router-bootstrap.tsx
"use client";
import "@takazudo/zfb-runtime/client-router";

export default function ClientRouterBootstrap() {
  return null;
}

// Stable marker name so the SSR marker, the scanner record, and the hydration
// manifest agree under production minification.
ClientRouterBootstrap.displayName = "ClientRouterBootstrap";
```

```tsx
import { Island } from "@takazudo/zfb";
import ClientRouterBootstrap from "@/components/client-router-bootstrap";

// Mount once near the end of <body>; when="load" registers the intercepts as
// soon as the islands runtime starts.
<Island when="load">
  <ClientRouterBootstrap />
</Island>;
```

This is the escape hatch, not the default — prefer a plain
`import { ClientRouter } from "@takazudo/zfb-runtime"`, which the scanner detects
on its own. See the [Client-Side Routing concept
guide](https://takazudomodular.com/pj/zudo-front-builder/docs/concepts/client-side-routing/)
for the full API.

#### `navigate()` needs `<ClientRouter />` mounted on the current page

The root barrel exports `navigate` and `syncHistoryEntry`, but **not** `init`
— importing either of those two on their own is not enough to get soft
navigation. `navigate()` checks for the `<meta
name="zfb-view-transitions-enabled">` tag that `<ClientRouter />` renders into
`<head>` before it will do a soft swap; with that tag absent it silently falls
back to a full `location.href` load. Mount `<ClientRouter />` in the layout
`<head>` of every page that should be soft-navigable — it both renders that
meta tag and calls `init()` (click/form interception) as a side effect. `init`
itself is exported from the `@takazudo/zfb-runtime/client-router` subpath, not
the root barrel, for the rare case where you want to call it directly instead
of mounting the component.

#### Persisting elements and island state across navigations (`data-zfb-transition-persist`)

Add `data-zfb-transition-persist="<id>"` to an element (matched by the same
`id` on both the outgoing and incoming page) to keep it alive across a soft
navigation instead of letting it be discarded and re-created — the router
lifts it out of the old body and reattaches it into the new one. This works
on plain elements (`<video>`, `<canvas>`) and on island wrapper elements
(`[data-zfb-island]`) alike; for an island, the live component instance (and
its internal state) survives too, not just the DOM node.

```tsx
<div data-zfb-island="SidebarTree" data-zfb-transition-persist="sidebar-tree" data-props={props}>
  <SidebarTree {...props} />
</div>
```

By default, a persisted island still gets its `data-props` refreshed to match
the incoming page on every swap; if the refreshed props differ from what the
live instance currently holds, the island is unmounted and remounted fresh
with the new props (so it can't get stuck showing stale data), otherwise
nothing happens and the instance's state carries over untouched. Set
`data-zfb-transition-persist-props` to any value other than `"false"`
(conventionally `"true"`) to opt OUT of that props refresh and keep the
island's current props/state exactly as they are, regardless of what the
incoming page's props would have been — the attribute's absence, or the
literal string `"false"`, is what makes props refresh (mirrors Astro's
`data-astro-transition-persist-props`). See the [Client-Side Routing concept
guide](https://takazudomodular.com/pj/zudo-front-builder/docs/concepts/client-side-routing/)
for the full walkthrough, including when to reach for the opt-out.

### `createPageRouter(options) → PageRouter`

Build a fetch-handler that serves the supplied pages. The returned
function is shape-compatible with a Worker `default.fetch`.

```ts
const router = createPageRouter({
  pages,            // PageDefinition[]
  contentSnapshot,  // ContentSnapshot embedded by the bundler
  framework,        // FrameworkAdapter
});

export default { fetch: router };
```

**Side effects.**

1. Calls `setContentSnapshot(contentSnapshot)` on the `zfb/content`
   module so any user page importing `getCollection(name)` resolves
   from memory rather than the Node `fs` API. Workers have no `fs`,
   so this branch is the production path. Idempotent; subsequent
   calls overwrite (matches the dev-mode live-reload contract).
2. Constructs an internal Hono app and registers `app.all(page.route, …)`
   for every entry in `pages`. The handler imports the page module,
   calls `framework.renderToString(module.default({}))`, and returns
   the string in a `Response`.

### `PageDefinition`

```ts
interface PageDefinition {
  readonly route: string;                            // Hono path pattern
  readonly module: () => Promise<PageModule>;        // thunk for code-split friendliness
}
```

### `PageModule`

The shape every page module must export:

```ts
interface PageModule {
  readonly default: (props: Record<string, unknown>) => unknown;
  readonly prerender?: boolean;          // literal `false` excludes from SSG
  readonly contentType?: string;         // overrides Content-Type (e.g. "application/xml")
  readonly headings?: readonly PageHeading[]; // MDX-emitted TOC data
  readonly paths?: () => unknown[] | Promise<unknown[]>; // enumerates concrete URLs for dynamic routes (SSG)
  readonly getStaticProps?: () => Promise<{ props: Record<string, unknown> }>; // fetches props at build/render time; result spread into default()
}

interface PageHeading {
  readonly depth: number;
  readonly slug: string;
  readonly text: string;
}
```

Default `Content-Type` is `text/html; charset=utf-8`.

### `FrameworkAdapter`

```ts
interface FrameworkAdapter {
  renderToString: (vnode: unknown) => string;
}
```

### `ContentSnapshot` / `EntrySnapshot`

Direct TypeScript mirror of the Rust contract in
`crates/zfb-content/src/content_bridge.rs`. Field names are snake_case
(`module_specifier`, `rel_path`) to match the JSON serialization.

```ts
interface EntrySnapshot {
  readonly slug: string;
  readonly frontmatter: unknown;        // null when source had none; getCollection normalises to {}
  readonly body: string;                // empty for .tsx entries
  readonly module_specifier: string;
  readonly rel_path: string;
}

interface ContentSnapshot {
  readonly collections: Readonly<Record<string, readonly EntrySnapshot[]>>;
}
```

The Rust side guarantees deterministic order (collections sorted by
name, entries sorted by slug). The TS side does **not** re-sort — it
preserves the order the bundle delivers, so the determinism story is
"identical Rust input → identical bundle bytes → identical render
output" without an extra sort step.

## Bundle shape consumed by the embedded V8 host

zfb's embedded V8 host loads a single ESM Worker bundle produced by the
esbuild step. The bundle's entry point must look like this:

```ts
// dist/worker.mjs (shape — generated by the bundler, not committed)
import { createPageRouter } from "@takazudo/zfb-runtime/server";
import * as preactRender from "preact-render-to-string";

import HomePage from "./pages/index.tsx";
import BlogPost from "./pages/blog/[slug].tsx";
// ... user content + layouts + components, bundled flat

const router = createPageRouter({
  pages: [
    { route: "/",            module: () => Promise.resolve({ default: HomePage }) },
    { route: "/blog/:slug",  module: () => Promise.resolve({ default: BlogPost }) },
    // ... one entry per route, expanded from `paths()` static evaluation
  ],
  contentSnapshot: {
    // Embedded JSON literal — the Rust bundler injects the snapshot via
    // an `import.meta.env`-style replacement or a top-level inline.
    collections: { /* ... */ },
  },
  framework: {
    renderToString: (vnode) => preactRender.renderToString(vnode as unknown as preactRender.ComponentChild),
  },
});

export default { fetch: router };
```

The host then drives the Worker by sending `GET` requests for each
enumerated route and writing the response body to `dist/{route}/index.html`.

### Contract this package commits to

- `createPageRouter` is the single export the build host wires to.
- The returned function is **always** `(request: Request) => Promise<Response>`,
  even if the underlying Hono path returns synchronously.
- `Content-Type` defaults to `text/html; charset=utf-8`. Page modules
  with a `contentType` field override it.
- Errors in page evaluation surface as 500 responses with a diagnostic
  text body; the host's source-map plumbing projects those back to the
  user's TSX line.
- The `ContentSnapshot` registration is idempotent and observable via
  `getContentSnapshot()` re-exported from `zfb/content`. Dev-mode hosts
  can call `setContentSnapshot(undefined)` to clear between rebuilds if
  needed (today the runtime overwrites on each `createPageRouter` call,
  which is the documented happy path).

## Local development

```sh
pnpm --filter @takazudo/zfb-runtime test
pnpm --filter @takazudo/zfb-runtime typecheck
```

Tests run in `vitest` under Node's `node` environment (no jsdom — the
server-side router targets the Workers `fetch` model, which Node
implements natively). Client-router DOM suites opt into
`@vitest-environment happy-dom` per test file. The framework adapter is
stubbed so tests do not pull in preact-render-to-string. Determinism is
asserted by rendering twice from independently-constructed routers and
comparing byte-equal.

The embedded V8 host is **not** booted from this package's tests — that
integration belongs to the Rust-side build host. The end-to-end
acceptance criterion ("Worker bundle returns correct HTML for each
route") is exercised by the host crate's test suite, not here.

## Why a peer dependency on `zfb`

`createPageRouter` calls `setContentSnapshot` from `zfb/content`. The
two modules share module-level state, so they must resolve to the same
instance — pinning `zfb` as a peer dep makes that explicit and lets pnpm
hoist a single shared copy. Install matching published versions of
`@takazudo/zfb-runtime` and `@takazudo/zfb` so that shared state remains
single-instanced.

Unlike `@takazudo/zfb`, the peer dependency on `react` is **optional**
here too — preact/compat is a fully supported target, so a preact-only
project does not need `react` installed and does not need
`auto-install-peers=true` in its pnpm config.
