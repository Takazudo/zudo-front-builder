# Diagnosis — the styled 404 page is never substituted (epic #2035, sub-issue #2036)

_Decision record for epic #2035 (supersedes #1971). Wave 2 (#2037) implements against the locked
fix spec in section 6; wave 3 (#2038) writes the Level-3 regression test shaped in section 7.
**No production code changed in this sub-issue.** Every number below was measured, not inferred —
against a real `wrangler dev` 4.85.0 (real workerd, local mode) and against the live production
site._

## TL;DR

- **Verdict: (b).** The asset router **does** return the styled 404 page to the Worker — status 404,
  `content-type: text/html; charset=utf-8`, a full 2790-byte body. The Worker's detection predicate
  `assetHasStyled404Body` (`packages/zfb-adapter-cloudflare/src/worker-wrapper.mjs:93-106`) rejects
  it because of its **second conjunct**, `contentLength > 0`: the Workers Static Assets binding
  **never sends a `content-length` header at all**.
- It is not "sometimes absent". The assets service streams every response — a plain 200 asset hit
  is `Transfer-Encoding: chunked` in local workerd, and HTTP/2 framed with no `content-length` in
  production. `Number(null || "0")` is `0`, so `contentLength > 0` is **dead-false for every real
  ASSETS response**, and the styled-404 branch introduced by issue #1322 can never be taken in
  production or preview.
- **Production and preview share this one root cause.** The deployed docs run
  `@takazudo/zfb-adapter-cloudflare@0.1.0-next.96`, whose `src/worker-wrapper.mjs` is **byte-identical**
  to this branch's (verified by `diff`), and the failure signature matches exactly
  (404 / `text/plain; charset=UTF-8` / 13 bytes on both).
- The narrowing recorded at `cli.test.ts:681` is **not** the bug. It governs the *inner* side, and the
  inner here returns `text/plain` — `innerIsFrameworkDefault404` was observed returning `true`. That
  test must not be touched.
- **Why every #1322 test is green while the feature is broken:** the mock helper
  `styledAsset404Response` (`cli.test.ts:106-116`) sets `content-length` by hand, under a comment
  asserting `(verified: workerd/CF serve 404.html with Content-Length)`. **That claim is false.** The
  mock encodes the exact assumption the predicate got wrong, so the tests could only ever confirm it.

## 1. Method

A minimal fixture driven by real `wrangler dev` 4.85.0 in local mode (real workerd — no deploy, no
account, no network), mirroring `docs/wrangler.toml`'s `[assets]` block verbatim:

```toml
[assets]
directory = "./dist"
binding = "ASSETS"
not_found_handling = "404-page"
run_worker_first = false
```

`dist/` held a hand-written 2790-byte styled `404.html`, an `index.html`, the `.assetsignore` the
adapter emits, a `_zfb_inner.mjs` standing in for the zfb inner bundle (Hono's default not-found:
404 + `text/plain; charset=UTF-8` + `"404 Not Found"`), and `_worker.js` written **verbatim** from
this branch's `WORKER_WRAPPER_SOURCE` export.

The docs site itself was deliberately **not** built: `docs/package.json` pins the published
`@takazudo/zfb` `0.1.0-next.96`, so building it would have exercised the published binary rather than
this branch. Instead, parity was established directly — `diff` on the two `worker-wrapper.mjs` files
(section 4) — which is stronger evidence than a build, since it compares the exact artifact.

## 2. The symptom reproduces offline, verbatim

Unmodified wrapper, real workerd:

```
GET /                     status=200 type=text/html; charset=utf-8  size=45
GET /nope-does-not-exist  status=404 type=text/plain; charset=UTF-8 size=13   <- the bug
GET /404.html             status=307                                          <- canonicalisation
body: 404 Not Found
```

Identical to the production signature the epic reports. `/404.html` 307-redirects exactly as
described, confirming the page is reachable as an asset.

## 3. What the asset router actually returned (the decisive evidence)

The same wrapper, instrumented with `console.log` immediately after
`const assetResponse = await env.ASSETS.fetch(request)` and again after the inner call. Observed on
`GET /nope-does-not-exist` with `not_found_handling = "404-page"`:

```
PROBE assetResponse.status = 404
PROBE assetResponse.headers = [["cache-control","public, max-age=0, must-revalidate"],
                               ["cf-cache-status","HIT"],
                               ["content-type","text/html; charset=utf-8"],
                               ["etag","\"56829349183dffaaddcd5855ace71ee2\""]]
PROBE content-type   = "text/html; charset=utf-8"
PROBE content-length = null
PROBE assetHasStyled404Body = false          <- the predicate rejects it
PROBE actual body byteLength = 2790          <- the styled page WAS there, in full
PROBE body head = "<!doctype html><html lang=\"en\"><head>..."

PROBE styledAsset404 !== null      = false
PROBE inner.status = 404, inner content-type = "text/plain; charset=UTF-8"
PROBE innerIsFrameworkDefault404   = true
PROBE branch = RETURN INNER
```

Read directly off those lines:

- Side (a) is **exonerated**: the asset router returned the complete styled body (2790 bytes) with
  the correct `text/html` content-type.
- Side (b) is **the failure**: `content-length` is `null`, so `Number(null || "0") === 0`, so
  `contentLength > 0` is false, so `assetHasStyled404Body` is false and `styledAsset404` is `null`.
- The inner side is behaving exactly as designed — `innerIsFrameworkDefault404` returned `true`, so
  the branch **would** have fired had the asset side been classified correctly.

The content-type conjunct passes. **Only the content-length conjunct fails.**

## 4. Production and preview share this root cause

Three independent measurements, taken together:

**(i) The deployed code is the same code.**

```
diff packages/zfb-adapter-cloudflare/src/worker-wrapper.mjs \
     node_modules/.pnpm/@takazudo+zfb-adapter-cloudflare@0.1.0-next.96/.../src/worker-wrapper.mjs
→ IDENTICAL
```

The `dist/` copy the adapter actually ships carries the same
`Number(response.headers.get("content-length") || "0")` line at `:104`. Production runs this exact
predicate.

**(ii) The failure signature is identical.** Live, re-run today:

```
$ curl -s -o /dev/null -w "status=%{http_code} type=%{content_type} size=%{size_download}\n" \
      https://zfb.takazudomodular.com/docs/nope-does-not-exist
status=404 type=text/plain; charset=UTF-8 size=13
```

Byte-for-byte the local signature. And the styled page is demonstrably present in production —
`/404.html` 307s to `/404`, which serves `200 text/html` at **94109 bytes**.

**(iii) Cloudflare's production asset service also omits `content-length`.** A real production asset
hit:

```
$ curl -s -D - -o /dev/null https://zfb.takazudomodular.com/
HTTP/2 200
content-type: text/html
cf-cache-status: HIT
cache-control: public, max-age=0, must-revalidate
...                                    <- no content-length
```

So the header's absence is a property of Cloudflare's asset serving, not a miniflare artifact.

**Stated limit (per #2036's instruction not to assert equivalence loosely):** (iii) observes the
*edge* response, not the in-Worker `env.ASSETS.fetch()` response — reading the latter in production
would require deploying an instrumented Worker, which this sub-issue does not do. What is proven is
that production runs an identical predicate, produces an identical failure signature, holds the
styled page as a real asset, and serves assets without `content-length`. That is a shared root cause
on the strength of the evidence available without a deploy. It is also moot for the fix: the
corrected predicate (section 6) no longer consults `content-length` at all, so it is correct whether
or not production's binding happens to set it.

## 5. Controls — the content-length conjunct is not load-bearing, and dropping it is safe

**Control A — `not_found_handling = "none"`, real workerd.** The historical behaviour the second
conjunct was believed to protect:

```
PROBE assetResponse.status = 404
PROBE assetResponse.headers = []      <- no headers at all
PROBE content-type   = null
PROBE actual body byteLength = 0
PROBE branch = RETURN INNER
```

The `"none"` response carries **no content-type whatsoever**. `content-type: text/html` alone
therefore separates `"404-page"` from `"none"` cleanly; the content-length conjunct contributes
nothing to that discrimination.

**Control B — HEAD, `404-page`.** Same headers, same missing `content-length`, empty body. The
predicate must stay header-only (it already is) so HEAD and GET classify identically; dropping the
conjunct preserves that symmetry.

**Control C — the corrected predicate, driven end-to-end through real workerd.** With the second
conjunct removed and nothing else changed:

| Config | Request | Result |
|---|---|---|
| `not_found_handling = "404-page"` | `GET /nope-x` | `404` `text/html; charset=utf-8` **2790 bytes**, body is the styled page |
| `not_found_handling = "404-page"` | `HEAD /nope-x` | `404` `text/html; charset=utf-8` |
| `not_found_handling = "none"` | `GET /nope-x` | `404` `text/plain; charset=UTF-8` 13 bytes, `404 Not Found` |
| `not_found_handling = "none"` | `HEAD /nope-x` | `404` `text/plain; charset=UTF-8` |

The bug is fixed and the `"none"` fallback is preserved, on the real runtime.

## 6. Locked fix spec for #2037

**File:** `packages/zfb-adapter-cloudflare/src/worker-wrapper.mjs` — the only copy of this predicate
in the repository (verified by grep across `packages/` and `crates/`; the `content-length` hits in
`crates/zfb-render/` and `crates/zfb-server/` are unrelated).

**Change:** in `assetHasStyled404Body` (currently lines 93-106), delete the `contentLength` local and
the `&& contentLength > 0` conjunct, leaving the content-type test as the sole discriminator:

```js
const contentType = (response.headers.get("content-type") || "").toLowerCase();
return contentType.includes("text/html");
```

**Also update the doc comment.** Lines 96-100 currently assert the platform sends "a real
`Content-Length` (it is a static file)" — that is the false premise. Replace it with the measured
fact: the Workers Static Assets binding streams asset responses and sends **no** `content-length`,
so content-type is the discriminator; `not_found_handling = "none"` returns a 404 with no headers at
all, which is what keeps the inner winning there. The same false claim appears in the wrapper's
top-of-file header comment around line 51 ("detected as an HTML document with a real
Content-Length") — fix that wording too.

**Also fix the test mock, and say so in the PR.** `cli.test.ts:106-116`'s `styledAsset404Response`
sets `content-length` under the comment `(verified: workerd/CF serve 404.html with Content-Length)`.
That comment is false and is why the suite never caught this. Drop the `content-length` header from
the mock and correct the comment, so the mock models what workerd actually sends. This is not
"editing a test to make a fix pass" — it is removing a mock's incorrect premise, and every existing
assertion stays as-is except the one at `:739` that reads `content-length` back off the HEAD
response, which must go with it.

**What must NOT change:**

- `innerIsFrameworkDefault404` — untouched. The inner side is correct and was observed returning
  `true` for the real Hono default.
- The narrowing at `cli.test.ts:681` (an inner `text/html` 404 wins over the styled asset 404) —
  untouched. It is not implicated; #2037's acceptance criterion about it is satisfied by leaving it
  alone.
- The precedence order overall: an inner 404 declaring `application/json` or `text/html` still wins.
  A non-404 inner response still wins.
- The `not_found_handling = "none"` behaviour — the inner must still win there (Control A proves
  content-type alone preserves this).
- `styledAsset404` must keep being returned **verbatim and unread** — the one-shot body must not be
  consumed by the predicate. The predicate stays header-only.

## 7. Recommended shape for #2038's Level-3 regression test

The reason this bug shipped is that the only coverage mocked `env.ASSETS` with a hand-written
`content-length`. The regression test must not be able to repeat that mistake:

1. **Assert the predicate against a header set that carries no `content-length`.** Drive the
   *emitted* `_worker.js` (the existing dynamic-import pattern in `cli.test.ts`) with an `env.ASSETS`
   mock returning `new Response(styledHtml, { status: 404, headers: { "content-type":
   "text/html; charset=utf-8" } })` — **no `content-length`, not even implicitly**. Assert the styled
   body wins. This one test would have caught the bug and cannot pass under the old predicate.
2. **Keep the negative control in the same file:** a `"none"`-shaped asset 404 modelled as
   `new Response(null, { status: 404 })` with **no headers at all** (that is what workerd actually
   sends — see Control A) must still yield the inner 404.
3. **Cover HEAD with the same header-only, content-length-free shape** — asset response with a null
   body and only a content-type; the styled asset's status/headers must still win.
4. Optionally record the measured header set from section 3 as a comment beside the mock, so the next
   reader can see the mock is grounded in an observation rather than an assumption.

A real `wrangler dev` lane would be stronger still, but it is a T3/T4 concern — see the
"wrangler/workerd adapter heavy lane" row in `CLAUDE.md`'s T3 cutover manifest. Level 3 on the
emitted worker is the right tier for the PR gate, provided the mock is corrected as above.
