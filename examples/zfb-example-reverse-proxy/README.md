# zfb-example-reverse-proxy

Catch-all SSR reverse proxy example for zfb on Cloudflare Workers Static
Assets. Requests under `/proxy/...` are forwarded to `PROXY_ORIGIN`, with
method, path remainder, query string, and request body streamed to the
upstream origin.

## When to use this pattern

Use this when a zfb site needs to expose a small, trusted HTTP surface below
its own domain, such as a documentation mirror, a same-company API facade, or
an origin that must share the site's deployment boundary.

Do not use this as an open proxy. Keep the target origin fixed in
`wrangler.toml`, validate any user-controlled paths in real applications, and
avoid proxying personalized or private responses unless you also disable shared
edge caching.

## Upstream origin

`wrangler.toml` sets:

```toml
[vars]
PROXY_ORIGIN = "https://httpbingo.org"
```

`httpbingo.org` is a deterministic, httpbin-compatible origin with endpoints
that exercise the behaviors this example needs:

- `/anything/...` echoes the forwarded method, URL, query, and headers.
- `/redirect-to?url=/anything/redirect-target&status_code=302` emits a
  same-origin `Location` header for redirect rewriting.
- `/cookies/set?zfb_proxy_cookie=demo` emits `Set-Cookie`.
- `/response-headers?...` emits CSP, HSTS, and ordinary headers in one
  response.

## Route shape

The proxy route is `pages/proxy/[...path].tsx` and exports the required literal:

```tsx
export const prerender = false;
```

The catch-all route receives the path remainder after `/proxy/`. The helper
derives the upstream URL from the original request URL so percent-encoded path
segments and the query string are preserved exactly when forwarding.

## Header policy

`lib/proxy.ts` strips hop-by-hop headers on both sides:

- `connection`
- `keep-alive`
- `te`
- `trailer`
- `transfer-encoding`
- `upgrade`
- `proxy-*`

The response also strips `Set-Cookie`, `Content-Security-Policy`,
`Content-Security-Policy-Report-Only`, and `Strict-Transport-Security`.
That prevents the upstream from setting cookies for the proxy host or applying
security policies that were authored for a different origin. The trade-off is
that upstream sessions and upstream browser security policy are intentionally
not preserved by this example.

Same-origin upstream `Location` redirect targets are rewritten back under
`/proxy/`, so a redirect to `https://httpbingo.org/anything/target` becomes
`/proxy/anything/target`.

The proxy fetch uses:

```ts
fetch(upstreamRequest, { cf: { cacheEverything: true } });
```

On Cloudflare, this asks the edge cache to treat GET and HEAD upstream
responses as cacheable content beyond the default cached file types, while
still respecting origin cache headers. The cache is shared at the edge, so do
not use this setting for user-specific responses without adding a stricter
cache policy.

## Local run

From the repository root:

```sh
pnpm install
pnpm --filter zfb-example-reverse-proxy dev
pnpm --filter zfb-example-reverse-proxy build
pnpm --filter zfb-example-reverse-proxy preview
```

`pnpm dev` is useful for the static index page. Because the proxy reads
Cloudflare `env.PROXY_ORIGIN`, use `pnpm preview` or direct Wrangler dev after
building when checking the SSR proxy path.

## Manual Wrangler checks

Build once, then run Wrangler from this package:

```sh
pnpm --filter zfb-example-reverse-proxy build
pnpm --dir examples/zfb-example-reverse-proxy exec wrangler dev --port 8788
```

In another shell:

```sh
curl -i "http://127.0.0.1:8788/proxy/anything/reverse-proxy?via=zfb"
curl -i "http://127.0.0.1:8788/proxy/redirect-to?url=/anything/redirect-target&status_code=302"
curl -i "http://127.0.0.1:8788/proxy/cookies/set?zfb_proxy_cookie=demo"
curl -i "http://127.0.0.1:8788/proxy/response-headers?Content-Security-Policy=default-src%20%27self%27&Strict-Transport-Security=max-age%3D31536000&X-Demo=kept"
```

Expected checks:

- `/anything/...` returns upstream JSON without buffering the body.
- `/redirect-to...` returns `Location: /proxy/anything/redirect-target`.
- `/cookies/set...` does not return `Set-Cookie`.
- `/response-headers...` keeps `X-Demo: kept` and strips CSP and HSTS.

## Deploy

No extra Cloudflare resources are required. After `pnpm build`, deploy with:

```sh
pnpm --dir examples/zfb-example-reverse-proxy exec wrangler deploy
```
