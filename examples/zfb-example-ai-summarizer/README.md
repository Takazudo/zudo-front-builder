# zfb-example-ai-summarizer

A compact zfb app with a Preact island UI and a `pages/api/summarize.tsx`
Worker route backed by a Cloudflare Workers AI binding.

## Local Development

Install dependencies from the repository root:

```sh
pnpm install
```

Fast UI loop:

```sh
pnpm --filter zfb-example-ai-summarizer dev
```

`zfb dev` does not provide Cloudflare Worker bindings. This example keeps the
API route usable anyway: when the `AI` binding is missing, the route returns a
deterministic local fallback summary. That is the primary zero-account local
check.

Production build:

```sh
pnpm --filter zfb-example-ai-summarizer build
```

Preview:

```sh
pnpm --filter zfb-example-ai-summarizer preview
```

Because this project configures the Cloudflare adapter, `zfb preview` hands off
to `wrangler dev` after the build.

## Cloudflare Workers AI

Workers AI does not need placeholder IDs in `wrangler.toml`. The binding is:

```toml
[ai]
binding = "AI"
```

For real model responses, authenticate Wrangler with a Cloudflare account that
can use Workers AI:

```sh
pnpm --filter zfb-example-ai-summarizer exec wrangler login
```

Then run the binding-realistic loop:

```sh
pnpm --filter zfb-example-ai-summarizer dev:cf
```

`dev:cf` runs `pnpm build`, starts `wrangler dev --port 8788`, and watches the
source files with `chokidar-cli`, rebuilding when pages, components, `lib`, or
styles change.

Endpoint check after `pnpm build` and `wrangler dev`:

```sh
curl -X POST http://localhost:8788/api/summarize \
  -H "content-type: application/json" \
  -d '{"text":"zfb renders static pages by default and uses prerender = false for request-time routes."}'
```

If Wrangler is not authenticated or Workers AI is unavailable, the endpoint
still returns JSON with `"fallback": true`.

## Deploy

```sh
pnpm --filter zfb-example-ai-summarizer build
pnpm --filter zfb-example-ai-summarizer exec wrangler deploy
```
