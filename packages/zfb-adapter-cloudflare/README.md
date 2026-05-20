# @takazudo/zfb-adapter-cloudflare

> Rust-built static-site engine for Astro and Next.js users — millisecond rebuilds, single binary.

The Cloudflare Pages adapter for [zfb][zfb-site]. Wraps the
`@takazudo/zfb-runtime` page router into a Cloudflare Pages advanced-mode
`_worker.js` entry, threading `(env, ctx)` through to user code via
`AsyncLocalStorage`.

This package is the Cloudflare half of the SSR adapter contract. Other
targets (Node, Netlify, …) will land as sibling `@takazudo/zfb-adapter-*`
packages with the same shape.

Full documentation: <https://takazudomodular.com/pj/zudo-front-builder/>.

[zfb-site]: https://takazudomodular.com/pj/zudo-front-builder/
[zfb-repo]: https://github.com/Takazudo/zudo-front-builder

## Install

```sh
npm install --save-dev @takazudo/zfb-adapter-cloudflare
# or: pnpm add -D @takazudo/zfb-adapter-cloudflare
```

## Usage

In `zfb.config.json`:

```jsonc
{
  "framework": "preact",
  "adapter": "@takazudo/zfb-adapter-cloudflare"
}
```

Then in any page that needs Cloudflare bindings:

```ts
// pages/api/whoami.tsx
import { getCloudflareContext } from "@takazudo/zfb-adapter-cloudflare";

export const prerender = false; // opt out of build-time SSG

interface Env {
  ANTHROPIC_API_KEY: string;
}

export default async function WhoAmI() {
  const { env, ctx } = getCloudflareContext<Env>();
  ctx.waitUntil(reportToAnalytics());
  return new Response(env.ANTHROPIC_API_KEY ? "ok" : "missing key");
}
```

`zfb build` will:

1. Render every SSG page (`prerender !== false`) into static HTML under
   `dist/`.
2. Hand the SSR bundle to this adapter, which writes
   `dist/_worker.js` (the wrapper) and `dist/_zfb_inner.mjs` (the
   bundle) ready to be deployed via Cloudflare Pages advanced mode.

## CLI

The package ships a `zfb-adapter-cloudflare` bin invoked by
`zfb-build`:

```
zfb-adapter-cloudflare bundle <input.mjs> --outdir dist/
```

`<input.mjs>` is the ESM bundle `zfb-build`'s bundler emits. The
output is a single Workers-shaped entry plus a sidecar copy of the
inner bundle.

## Why two files

The wrapper imports the inner bundle by relative path
(`./_zfb_inner.mjs`) instead of inlining it, so the adapter package
itself does not need to ship an esbuild binary. Workerd's Module
loader resolves relative ESM imports inside an advanced-mode
`_worker.js` directory layout.

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
