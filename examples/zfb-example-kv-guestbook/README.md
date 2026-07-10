# zfb-example-kv-guestbook

A compact zfb + Cloudflare Workers KV guestbook recipe. The homepage is a
server-rendered `prerender = false` route with a plain HTML form, and the same
KV helpers power JSON API endpoints.

## Local run

```bash
pnpm install
pnpm dev
pnpm build
pnpm preview
```

`pnpm dev` is useful for normal zfb authoring, but Cloudflare bindings are not
available there. Binding-backed routes return a controlled `503` instead of
crashing. Use `pnpm build` followed by `pnpm preview` for the local Worker and
KV simulation.

Wrangler stores local KV state under `.wrangler/`. Delete that directory when
you want a fresh local namespace.

## Provision Cloudflare resources

Create a KV namespace:

```bash
pnpm exec wrangler kv namespace create zfb-example-kv-guestbook
```

Paste the printed namespace ID into `wrangler.toml`:

```toml
[[kv_namespaces]]
binding = "GUESTBOOK"
id = "REPLACE_WITH_KV_NAMESPACE_ID"
```

Set the admin delete token as a secret:

```bash
pnpm exec wrangler secret put ADMIN_TOKEN
```

For local `pnpm preview`, put a local token in `.dev.vars`:

```dotenv
ADMIN_TOKEN=local-dev-token
```

Deploy after building:

```bash
pnpm build
pnpm exec wrangler deploy
```

## Endpoints

- `GET /` renders the guestbook and the no-JS form.
- `POST /` handles the form, queues the KV write, and redirects back to `/`.
- `GET /api/entries` returns the current bounded entry window as JSON.
- `POST /api/entries` accepts JSON, form, or text input with a `message`.
- `DELETE /api/entries/<entry-key>` deletes an entry when the request includes
  `Authorization: Bearer <ADMIN_TOKEN>`.

Example JSON write:

```bash
curl -X POST http://localhost:8787/api/entries \
  -H "content-type: application/json" \
  --data '{"message":"hello from curl"}'
```

Example admin delete:

```bash
ENTRY_KEY='entry:2026-07-10T00:00:00.000Z:replace'
ENCODED_KEY=$(node -e 'process.stdout.write(encodeURIComponent(process.argv[1]))' "$ENTRY_KEY")
curl -X DELETE "http://localhost:8787/api/entries/$ENCODED_KEY" \
  -H "Authorization: Bearer $ADMIN_TOKEN"
```

## KV behavior

Writes use keys shaped as `entry:<ISO timestamp>:<random hex>`, so the key name
contains the creation time and remains sortable. Each entry uses
`expirationTtl`, currently 30 days, so the namespace does not grow forever.

`POST /api/entries` and the homepage form pass `KV.put(...)` to
`ctx.waitUntil()` and return before the write settles. That keeps the response
fast, but KV is eventually consistent, so a redirect or immediate
`GET /api/entries` may not show the new entry yet.

The read path calls `KV.list({ prefix, limit })` first, then reads only a capped
number of keys with per-key `KV.get(...)` calls. The cap keeps the recipe under
Workers subrequest budgets and avoids opening an unbounded number of KV
connections for a single request.

If the `GUESTBOOK` binding is missing, routes return a clear `503` JSON or text
response. If `ADMIN_TOKEN` is missing, the delete endpoint returns a clear
`503`; missing or wrong bearer tokens return `401`.
