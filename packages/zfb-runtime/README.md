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
`react-dom/server` slot in.

## Public API

```ts
import { createPageRouter } from "@takazudo/zfb-runtime";
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
  ClientRouter,               // <ClientRouter /> Preact component — mounts view-transition intercepts
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

#### Enabling SPA soft-navigation — the bootstrap island

Mounting `<ClientRouter />` in your layout `<head>` emits the meta tags and
styles the soft-navigation router reads, **but it is not enough on its own** to
turn navigation interception on. You also need a tiny client-side *bootstrap
island*. This step is easy to miss, so it is spelled out here.

**Why.** The click/form interception is registered by an `init()` call that runs
as a top-of-module side effect in the client-router module, guarded so it never
runs during SSR:

```ts
// inside @takazudo/zfb-runtime — client-router.ts
if (typeof document !== "undefined") {
  init();
}
```

When your **only** import of the client-router happens in a layout that renders
server-side — `import { ClientRouter } from "@takazudo/zfb-runtime"` — that
import is evaluated where `typeof document === "undefined"`, so:

1. `init()` is correctly skipped during SSR, **and**
2. because nothing reachable from a `"use client"` graph imports the
   client-router, the module never enters the **client** bundle either — so the
   guard never gets a chance to fire in the browser.

The result: the `<ClientRouter />` meta tags are present, but no navigation is
ever intercepted. Every link falls through to a full page load — no view
transition, and persisted islands are destroyed.

**The fix** is a minimal `"use client"` bootstrap island whose only job is a
side-effect import of the `/client-router` subpath. zfb's island scanner walks
`page → bootstrap island → @takazudo/zfb-runtime/client-router`, which pulls the
client-router subgraph into the browser bundle, where the
`typeof document !== "undefined"` guard finally fires:

```tsx
// src/components/client-router-bootstrap.tsx
"use client";

// Side-effect import. Running this island's bundle in the browser triggers the
// `if (typeof document !== "undefined") { init(); }` guard at the top of the
// client-router module, registering the SPA click/form intercepts. init() is
// idempotent (guarded by an `initialized` flag), so reaching it again is a
// no-op.
import "@takazudo/zfb-runtime/client-router";

// Renders nothing — the component exists only so the scanner has a page-reachable
// "use client" module to bundle the side-effect import into.
export default function ClientRouterBootstrap() {
  return null;
}

// Stable marker name so the SSR marker, the scanner record, and the hydration
// manifest agree under production minification.
ClientRouterBootstrap.displayName = "ClientRouterBootstrap";
```

Mount it once near the end of `<body>`, wrapped in zfb's `<Island>` so it
hydrates as early as possible:

```tsx
import { Island } from "@takazudo/zfb";
import ClientRouterBootstrap from "@/components/client-router-bootstrap";

// when="load" registers the intercepts as soon as the islands runtime starts —
// before the first navigation.
<Island when="load">
  <ClientRouterBootstrap />
</Island>;
```

Notes:

- Keep `<ClientRouter />` in `<head>` **and** add the bootstrap island — they
  are complementary. The head component emits the configuration meta tags; the
  bootstrap island delivers the runtime to the browser.
- The bootstrap pulls in only the client-router subgraph (router + swap +
  events + types) plus the island-manager dependency; the server-only runtime
  modules stay out of the client bundle.
- The bootstrap island must be reachable through a normal `import` chain from a
  page module — that is how the scanner discovers it. Mounting `<ClientRouter />`
  alone does not make the runtime reachable, because `<ClientRouter />` renders
  into `<head>` and is not itself a `"use client"` island.

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
2. Constructs an internal Hono app and registers `app.get(page.route, …)`
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
import { createPageRouter } from "@takazudo/zfb-runtime";
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
runtime targets the Workers `fetch` model, which Node implements
natively). The framework adapter is stubbed so tests do not pull in
preact-render-to-string. Determinism is asserted by rendering twice
from independently-constructed routers and comparing byte-equal.

The embedded V8 host is **not** booted from this package's tests — that
integration belongs to the Rust-side build host. The end-to-end
acceptance criterion ("Worker bundle returns correct HTML for each
route") is exercised by the host crate's test suite, not here.

## Why a peer dependency on `zfb`

`createPageRouter` calls `setContentSnapshot` from `zfb/content`. The
two modules share module-level state, so they must resolve to the same
instance — pinning `zfb` as a peer dep makes that explicit and lets pnpm
hoist a single shared copy. Workspace-internal usage today resolves via
`workspace:*`; an external publish would change to a SemVer range.
