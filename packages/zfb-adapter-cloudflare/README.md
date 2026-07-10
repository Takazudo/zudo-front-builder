# @takazudo/zfb-adapter-cloudflare

> Rust-built static-site engine for Astro and Next.js users — millisecond rebuilds, single binary.

The Cloudflare adapter for [zfb][zfb-site], targeting **Workers Static
Assets** (also deployable to Cloudflare Pages advanced mode). It wraps the
`@takazudo/zfb-runtime` page router into a Worker entry (`_worker.js`),
threading `(env, ctx)` through to user code via `AsyncLocalStorage`.

This package is the Cloudflare half of the SSR adapter contract. Other
targets (Node, Netlify, …) will land as sibling `@takazudo/zfb-adapter-*`
packages with the same shape.

Full documentation: <https://takazudomodular.com/pj/zudo-front-builder/>.
Source: <https://github.com/Takazudo/zudo-front-builder>.

[zfb-site]: https://takazudomodular.com/pj/zudo-front-builder/

## Install

```sh
npm install --save-dev @takazudo/zfb-adapter-cloudflare
# or: pnpm add -D @takazudo/zfb-adapter-cloudflare
```

## Usage

In `zfb.config.json`:

```json
{
  "framework": "preact",
  "adapter": "@takazudo/zfb-adapter-cloudflare"
}
```

Then in any page that needs Cloudflare bindings — secrets, KV, or a
D1 database:

```ts
// pages/api/products.tsx
import { getCloudflareContext } from "@takazudo/zfb-adapter-cloudflare";

export const prerender = false; // opt out of build-time SSG

interface Env {
  ANTHROPIC_API_KEY: string;
  DB: D1Database; // a `wrangler.toml` D1 binding named "DB"
}

export default async function Products() {
  const { env, ctx } = getCloudflareContext<Env>();
  ctx.waitUntil(reportToAnalytics());
  // A D1 binding is just-another-object on `env` — query it directly.
  const { results } = await env.DB.prepare("SELECT * FROM products").all();
  return new Response(JSON.stringify(results), {
    headers: { "content-type": "application/json" },
  });
}
```

See the [SSR and Cloudflare Bindings guide][ssr-guide] for the full D1
lifecycle (`wrangler d1 create`, migrations, preview-vs-prod).

[ssr-guide]: https://takazudomodular.com/pj/zudo-front-builder/docs/guides/ssr-and-cloudflare-bindings/

`zfb build` will:

1. Render every SSG page (`prerender !== false`) into static HTML under
   `dist/`.
2. Hand the SSR bundle to this adapter, which writes `dist/_worker.js`
   (the wrapper), `dist/_zfb_inner.mjs` (the bundle), and
   `dist/.assetsignore` (see below) — ready to deploy as a Worker with
   Static Assets, or via Cloudflare Pages advanced mode.

## `wrangler.toml`

```toml
name = "my-site"
main = "./dist/_worker.js"
compatibility_date = "2024-09-23"
compatibility_flags = ["nodejs_compat"]

[assets]
directory = "./dist"
binding = "ASSETS" # optional — lets the Worker probe assets itself
not_found_handling = "404-page"
```

- `compatibility_flags = ["nodejs_compat"]` is **required**. The wrapper
  imports `node:async_hooks` to thread `(env, ctx)` into your route
  handlers; without this flag the Worker fails at runtime with
  `No such module "node:async_hooks"`.
- `not_found_handling = "404-page"` is recommended. With it, an
  unmatched path makes the asset layer serve your styled `dist/404.html`
  (built from `pages/404.tsx`) with a `404` status. The `_worker.js`
  wrapper still probes the inner Worker for genuinely dynamic
  `prerender = false` routes, but its **404 precedence** is:
  - inner returns a non-404 (a real dynamic route) → the inner response
    wins;
  - inner also 404s with only the framework default body (Hono's default
    `text/plain` "404 Not Found", or a bare 404 with no content-type) →
    the **styled `404.html` wins**, so visitors see your designed 404 page
    instead of plain text;
  - inner 404s with any other content-type → that **deliberate response is
    preserved** and the styled page does not stomp it: a `text/html` 404 is
    a rendered custom not-found page (e.g. a `prerender = false` route
    SSRing its own 404), and an `application/json` 404 is an intentional
    API error.

  A bare `text/plain` API 404 is indistinguishable from the framework
  default and yields to the styled page — use `application/json` for API
  errors you want preserved.

  Under `not_found_handling = "none"` the asset 404 has no styled body, so
  the inner Worker's plain 404 is always shown. Avoid
  `single-page-application`: it returns `index.html` for every unresolved
  path _before_ the Worker ever sees the request, which breaks dynamic
  routes like `pages/api/*.tsx`.

- `[assets] binding = "ASSETS"` is optional but recommended — it lets
  the `_worker.js` wrapper probe `env.ASSETS.fetch()` directly for
  GET/HEAD requests, matching the SSG-vs-SSR precedence described
  below.

## `wrangler dev` / `wrangler deploy`

```sh
zfb build
wrangler dev     # local Worker + assets simulation
wrangler deploy  # ship it
```

## `.assetsignore`

The adapter emits `dist/.assetsignore` alongside `_worker.js` and
`_zfb_inner.mjs`:

```
_worker.js
_zfb_inner.mjs
```

This excludes the wrapper and the inner SSR bundle from the asset
upload, so they are reachable only through the Worker's own module
graph — never served as public static files. Without it, a request for
`/_worker.js` or `/_zfb_inner.mjs` would serve your server code as a
plain-text download.

**Precedence:** zfb copies your project's `public/` directory into
`dist/` _after_ running this adapter. If your `public/` directory
contains its own `.assetsignore`, it overrides the one this adapter
emits — the adapter's excludes are silently dropped. Only add a
`public/.assetsignore` if you have additional paths to exclude and
include the two lines above yourself.

## Cloudflare Pages compatibility

The same `dist/` output is still deployable to Cloudflare Pages
advanced mode (a `_worker.js` at the root of the Pages output directory
is Pages' equivalent convention). The `_worker.js` wrapper dispatches
requests identically on both platforms; the only platform-visible
difference is that trailing-slash asset redirects come back as `307`
on Workers Static Assets versus `308` on Pages.

## Why two bundle files instead of one

The wrapper imports the inner bundle by relative path
(`./_zfb_inner.mjs`) instead of inlining it, so the adapter package
itself does not need to ship an esbuild binary. Workerd's Module
loader resolves relative ESM imports inside the `_worker.js` directory
layout.

## Why AsyncLocalStorage on a globalThis registry

Cloudflare Workers can dispatch multiple requests concurrently in the
same isolate, so reading `env` from `globalThis` would race across
requests. AsyncLocalStorage gives each request its own scope.

We register the storage instance on `globalThis` under a stable key so
the wrapper at `_worker.js` and the user pages bundled together share
the same instance even when this module ends up duplicated in the
final module graph (the wrapper and the user bundle are separate ESM
graphs by design).

## Acceptance test

`src/__tests__/cli.test.ts` imports the produced `_worker.js`
directly into vitest, builds a synthetic `Request + env + ctx`, and
asserts that user code reading `env.ANTHROPIC_API_KEY` sees the value
the wrapper passed in. This stands in for a full `wrangler dev` smoke
test, which would require port binding and is out of scope for the
worktree-side test loop.
