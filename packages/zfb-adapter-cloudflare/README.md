# @takazudo/zfb-adapter-cloudflare

> Rust-built static-site engine for Astro and Next.js users — millisecond rebuilds, single binary.

The Cloudflare adapter for [zfb][zfb-site], verified on **Workers Static
Assets**. It wraps the `@takazudo/zfb-runtime` page router into a Worker entry
(`_worker.js`), threading `(env, ctx)` through to user code via
`AsyncLocalStorage`. Cloudflare Pages advanced mode is unverified.

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
   (the wrapper), `dist/_zfb_inner.mjs` (the bundle), copied
   `<name>-<hash>.wasm` modules (e.g. `index_bg-a1b2c3d4.wasm` — this is
   esbuild's own `--asset-names=[name]-[hash]` convention, not a
   zfb-specific scheme), and `dist/.assetsignore` (see below) — ready
   for Workers Static Assets.

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
`_zfb_inner.mjs`. When zfb passes one or more `--asset` Wasm modules, it
also adds every copied basename:

```
_worker.js
_zfb_inner.mjs
index_bg-a1b2c3d4.wasm
```

This excludes the wrapper, inner SSR bundle, and compiled Wasm modules from
the asset upload, so they are reachable only through the Worker's own module
graph — never served as public static files. Without it, a request for
`/_worker.js` or `/_zfb_inner.mjs` would serve your server code as a
plain-text download.

**Merge behavior:** zfb copies your project's `public/` directory into
`dist/` _after_ running this adapter. If `public/.assetsignore` adds entries,
zfb merges them with the generated entries; it does not drop the wrapper,
inner-bundle, or Wasm exclusions.

## CLI asset contract

zfb calls the package CLI with the SSR bundle plus one repeatable `--asset`
path for every bundle-relative Wasm module:

```sh
zfb-adapter-cloudflare bundle ./bundle.mjs --outdir ./dist \
  --asset index_bg-a1b2c3d4.wasm
```

Each asset path must be relative to the input bundle directory. The CLI copies
it into `outdir` under its basename and records that basename in
`.assetsignore`; paths that escape the input directory or collide with emitted
output fail the build.

## Cloudflare Pages advanced mode

The root-level `_worker.js` follows the Cloudflare Pages advanced-mode
convention, but this adapter is only verified on Workers Static Assets.
Cloudflare Pages advanced mode remains unverified and should not be treated as
a supported deployment target until it has a dedicated smoke test.

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
