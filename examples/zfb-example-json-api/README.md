# zfb-example-json-api

## SSR Contract

This example is a compact SSR/API starter for zfb on Cloudflare Workers Static Assets.

- API routes must opt out of build-time rendering with the literal export `export const prerender = false;`. zfb detects that exact AST shape at build time.
- `getCloudflareContext<Env>()` returns `{ env, request, ctx }` inside the emitted Worker request. It throws outside a Worker request scope. The generic `Env` is still useful even when this example has no bindings.
- `zfb dev` routes `prerender = false` code through the SSR path, but it does not provision Worker bindings. This example's API handlers read `request` from the Cloudflare adapter context, so endpoint checks belong in `pnpm build` plus `pnpm preview` or `pnpm exec wrangler dev`.
- `not_found_handling = "404-page"` keeps unresolved asset paths on the styled static 404 path while preserving deliberate API 404 responses when they use `application/json`. A bare `text/plain` 404 can look like the framework default and yield to the styled page.
- No Cloudflare resources need provisioning. The `wrangler.toml` only declares the static assets binding used by the adapter wrapper.

## What It Shows

- `/api/items` filters the demo data with `q` and paginates with `page` and `per`.
- `/api/search` builds a module-scope MiniSearch index lazily and returns `indexBuiltAt` plus `indexBuildCount`, so repeated warm-isolate requests can observe index reuse.
- `/` is a static page with one Preact island that fetches both JSON endpoints.

## Run Locally

```sh
pnpm install
pnpm dev
pnpm build
pnpm preview
```

Use `pnpm dev` for the static shell and component iteration. Use `pnpm preview` after `pnpm build` for Worker-shaped API behavior.

## Endpoint Checks

After `pnpm build`, start a local Worker with `pnpm preview` or `pnpm exec wrangler dev`, then run:

```sh
curl 'http://localhost:8787/api/items?q=review&page=1&per=5'
curl 'http://localhost:8787/api/search?q=onboarding'
curl -i -X OPTIONS 'http://localhost:8787/api/items'
curl -i -X POST 'http://localhost:8787/api/items'
```

Run the search request twice against the same local Worker process. `indexBuiltAt` should stay stable while the isolate stays warm.

## Deploy

```sh
pnpm build
pnpm exec wrangler deploy
```

There are no D1, KV, R2, secret, or queue bindings to create.
