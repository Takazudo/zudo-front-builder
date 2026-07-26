# Request-time capability contract for the embedded V8 host (#2013)

Status: locked for implementation by issues #2014, #2015, #2016, #2017, #2018, #2019, and #2020
(epic #2012, superseding #1750 and #1751).

This note is the durable decision record for what `fetch` and Web Crypto do inside
`crates/zfb-render/src/embedded_v8/` once the host can tell **build-time SSG** from **request-time
SSR**. It is a contract, not a survey: every cell below is a decision an implementer can code
against without a follow-up question. Where a number was chosen rather than inherited from
production, the row says so and states what would change it.

## Why this exists

`crates/zfb/src/ssr_adapter.rs` dispatches `prerender = false` requests into the **same**
`EmbeddedV8RenderHost` instance that the dev tick uses to prerender SSG pages
(`RendererState::embedded_v8_host_mut`). The polyfill therefore cannot distinguish the two, and the
deliberate build-time network denial in `web_polyfills.js` (~:556) leaks into request-time SSR —
that is bug #1750. The same shared-host fact is why mode must be **per dispatch**, not per host
(see [Mode distinction](#mode-distinction)).

## Parity baseline

Production for zfb is **Cloudflare Workers / workerd**, reached through
`packages/zfb-adapter-cloudflare`. Every row below is anchored to one of:

- **workerd-documented** — Cloudflare's published runtime behaviour or platform limits.
- **spec** — WHATWG Fetch or W3C WebCrypto, which workerd states it implements.
- **zfb decision** — chosen here because production does not settle it, or because the embedded
  host is deliberately tighter. Every such row carries its rationale and its "what would change
  this".

Sources consulted: [Workers Web Crypto](https://developers.cloudflare.com/workers/runtime-apis/web-crypto/),
[Workers limits](https://developers.cloudflare.com/workers/platform/limits/).

## Numeric constants — one source of truth

All limits live in **Rust** as `pub const`s in a new
`crates/zfb-render/src/embedded_v8/limits.rs`, re-exported from `zfb_render`. The host injects them
into `globalThis.__zfb.limits` at boot (alongside the existing `__zfb` bridge in
`embedded_v8/js/globals_shim.js`), and the JS polyfill reads them from there rather than hardcoding
a second copy. A test asserts the JS-visible values equal the Rust constants, so the two can never
drift.

| Constant                        |           Value | Applies to                                |
| ------------------------------- | --------------: | ----------------------------------------- |
| `ALLOWED_FETCH_SCHEMES`         | `["http","https"]` | outbound `fetch` URL scheme allowlist  |
| `MAX_REDIRECTS`                 |            `20` | `redirect: "follow"` chain length         |
| `DEFAULT_FETCH_TIMEOUT_MS`      |        `30_000` | per-`fetch` wall clock                    |
| `MAX_REQUEST_BODY_BYTES`        | `104_857_600`   | outbound request body (100 MB)            |
| `MAX_RESPONSE_BODY_BYTES`       | `104_857_600`   | buffered response body (100 MB)           |
| `MAX_SUBREQUESTS_PER_DISPATCH`  |            `50` | `fetch` calls per `dispatch_fetch`        |
| `MAX_RANDOM_BYTES_PER_CALL`     |        `65_536` | `crypto.getRandomValues` byte quota       |

`DEFAULT_FETCH_TIMEOUT_MS` is the only one overridable at runtime, via the
`ZFB_SSR_FETCH_TIMEOUT_MS` environment variable (parsed once at host boot; a non-numeric or `0`
value is ignored with a warning and the default stands).

## The contract table

Legend for the source anchor in **Supported behavior**: `[wd]` workerd-documented, `[spec]`
WHATWG/W3C, `[zfb]` decided here.

### fetch

| Feature | Supported behavior | Limits | Error type / message | SSG behavior | SSR behavior | Cancellation / resource limits |
| --- | --- | --- | --- | --- | --- | --- |
| **URL scheme** | `http:` and `https:` only `[wd]`. Loopback and private addresses ARE reachable `[zfb]` (production Workers cannot reach them; local dev talking to a local API is the point of the feature, and guardrail 3 requires loopback test servers). | `ALLOWED_FETCH_SCHEMES` | `TypeError` — `Fetch API cannot load: <url>` (workerd/Chromium message shape; verified against real `wrangler dev` in #2020) | Rejects with the existing SSG message before scheme checking. | Scheme checked in **Rust**, before any socket is opened. | n/a |
| **Method** | Every standard method passes through verbatim, uppercased `[spec]`. `GET`/`HEAD` with a non-null body is a `TypeError` `[spec]`. | none | `TypeError` — `Request with GET/HEAD method cannot have body.` | denied | supported | n/a |
| **Request headers** | Caller headers pass through verbatim except hop-by-hop names the transport owns: `host`, `connection`, `transfer-encoding`, `content-length`, `upgrade`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`, `te`, `trailer` — these are **dropped and recomputed** by the host `[zfb]`. No `accept-encoding` is sent unless the caller sets one, and automatic decompression is **disabled**, so response bytes are surfaced verbatim `[zfb]` (workerd negotiates compression transparently; identity-only keeps the local host byte-deterministic — divergence row below). | none on count or size beyond the transport's own | dropped headers are silent, not an error (matches `Headers`' forbidden-header behaviour) | denied | supported | n/a |
| **Request body** | `string`, `ArrayBuffer`, `ArrayBufferView`, `URLSearchParams`, `null` — the existing `_Body` shapes in `web_polyfills.js`. `ReadableStream` bodies are **not** supported `[zfb]`; no stream type exists in this host. | `MAX_REQUEST_BODY_BYTES` (100 MB) | `TypeError` — `fetch(<url>): request body exceeds the <N>-byte limit` / `TypeError: fetch(<url>): ReadableStream request bodies are not supported by the zfb embedded runtime` | denied | supported | Body is materialised in memory before the op is called; the size check happens in **JS before** the op and again in **Rust** (defence in depth). |
| **Redirects** | `redirect: "follow"` (default), `"manual"`, `"error"` `[spec]`. `follow` chases up to `MAX_REDIRECTS`; `manual` returns the 3xx response unchanged with `redirected = false`; `error` makes any 3xx a network error. Per `[spec]`, a 301/302 response to `POST` is re-issued as `GET` with the body dropped; 307/308 preserve method and body. Cross-origin redirects strip `authorization`, `cookie`, and `proxy-authorization` `[spec]`. | `MAX_REDIRECTS = 20` `[spec]` — the Fetch standard redirect count; Cloudflare documents no divergence | `TypeError` — `fetch(<url>): too many redirects (limit 20)`; for `redirect:"error"`, `TypeError: fetch(<url>): redirect not allowed (redirect mode is "error")` | denied | supported | Each hop counts against `MAX_SUBREQUESTS_PER_DISPATCH`, matching Cloudflare, where every hop in a redirect chain is a subrequest `[wd]`. |
| **Response status / headers** | `status`, `statusText`, and the **complete** header list are surfaced. Headers cross the boundary as an ordered `Array<[name, value]>`, never a map, so repeated names (notably `set-cookie`) survive — the same shape `dispatch_fetch` already uses for the outbound direction. `response.url` is the **final** URL after redirects; `redirected` is `true` when at least one hop was followed; `type` is `"default"`. | none | n/a | denied | supported | n/a |
| **Response body** | Fully buffered bytes. `text()`, `arrayBuffer()`, `json()` work. `response.body` is `null` — `ReadableStream` does not exist in this host `[zfb]`. `blob()` and `formData()` continue to throw, but with a request-time-specific message. | `MAX_RESPONSE_BODY_BYTES` (100 MB) | `TypeError` — `fetch(<url>): response body exceeds the <N>-byte limit` / `Error: response.blob() is not implemented in the zfb embedded runtime` | denied | supported | The Rust op counts bytes **as they stream in** and aborts the connection the moment the cap is crossed; a declared `content-length` above the cap is rejected before the body is read at all. This is the resource-exhaustion guard required by guardrail 6. |
| **Abort** | `init.signal` and `Request.signal` are honoured. `AbortController` / `AbortSignal` / `AbortSignal.timeout()` / `AbortSignal.abort()` must be **added** to `web_polyfills.js` — they do not exist today `[zfb]`. An already-aborted signal rejects synchronously without opening a socket. `signal.reason` is used as the rejection value when set, else an `AbortError` `[spec]`. | n/a | `Error` with `name = "AbortError"` — `The operation was aborted.` (a real `DOMException` is unavailable — see divergences) | denied | supported | Aborting **drops the Rust future**, which cancels the in-flight `reqwest` request and closes the socket. Cancellation mid-body must be covered by a #2015 test. |
| **Timeout** | Every `fetch` carries a wall-clock deadline of `DEFAULT_FETCH_TIMEOUT_MS`, enforced in **Rust** `[zfb]`. Production Workers has **no** per-subrequest time limit while the client stays connected `[wd]` — this is a deliberate divergence (see below). | 30 s, override via `ZFB_SSR_FETCH_TIMEOUT_MS` | `Error` with `name = "TimeoutError"` — `fetch(<url>): timed out after <N>ms (zfb embedded-runtime request-time limit; production Cloudflare Workers has no per-subrequest timeout)` | denied | supported | Timeout fires the same cancellation path as abort: the future is dropped, the socket closes. A caller-supplied `signal` and the deadline race; whichever fires first wins. |
| **Subrequest count** | Counted per `dispatch_fetch`, reset at the start of every dispatch. | `MAX_SUBREQUESTS_PER_DISPATCH = 50` `[zfb]`, anchored on Cloudflare's smallest documented per-invocation subrequest limit (Workers Free = 50) `[wd]` so anything that passes locally fits every plan | `TypeError` — `fetch(<url>): exceeded the 50-subrequest limit for a single request` | denied (counter never increments) | supported | The counter lives in Rust, keyed to the active dispatch, so a `Promise.all` fan-out cannot evade it. Guardrail 6's resource-exhaustion case. |
| **Transport failure** | Any DNS, TCP, or TLS failure surfaces as a network error `[spec]`. | n/a | `TypeError` — `fetch(<url>): <cause>` where `<cause>` is the transport's own message | denied | supported | n/a |
| **Host-op failure** | If the op itself cannot run (channel closed, runtime shutting down), the promise rejects. It never resolves to a synthetic empty `Response` `[zfb]` — a silent empty body would be indistinguishable from a real 200 with no content. | n/a | `TypeError` — `fetch(<url>): embedded host transport unavailable: <detail>` | denied | supported | Guardrail 6's host-op-failure case. |
| **SSG denial (policy row)** | Build-time render makes **no** outbound requests. This is deliberate policy, not a missing feature, and survives this epic intact (guardrail 4). | n/a | `Error` — the **existing** message, byte-identical: `fetch() called from SSG runtime (url=<url>). The embedded V8 host does not support outgoing network requests during build-time render. Move the data fetch to a build step or a runtime-only branch.` | **This row IS the SSG behaviour.** | Never reached — request-time takes the other branch. | The rejection happens in JS, before any op call, so no host resource is touched. |

### Web Crypto

| Feature | Supported behavior | Limits | Error type / message | SSG behavior | SSR behavior | Cancellation / resource limits |
| --- | --- | --- | --- | --- | --- | --- |
| **`crypto.getRandomValues(view)`** | Fills `view` from the **OS CSPRNG** via a host op and returns the same `view` `[spec]`. Eligible views, exactly Cloudflare's list `[wd]`: `Int8Array`, `Uint8Array`, `Uint8ClampedArray`, `Int16Array`, `Uint16Array`, `Int32Array`, `Uint32Array`, `BigInt64Array`, `BigUint64Array`. Ineligible: `Float16Array`, `Float32Array`, `Float64Array`, `DataView`, and anything that is not an `ArrayBufferView`. A zero-length view is a no-op that returns the view `[spec]`. | `MAX_RANDOM_BYTES_PER_CALL = 65_536`, measured on **`byteLength`**, not element count `[spec]` | ineligible view → `Error` with `name = "TypeMismatchError"`, message `crypto.getRandomValues: <ctor> is not an integer-typed ArrayBufferView`. Over quota → `Error` with `name = "QuotaExceededError"`, message `crypto.getRandomValues: requested <N> bytes, quota is 65536 bytes`. | **Same as SSR.** `Math.random` is removed on both paths — the SSG policy is about *network*, not entropy, and mode-dependent randomness quality is a footgun. | Same as SSG. | Entropy op is **synchronous** — see the note on guardrail 1 below. Host-op failure throws; it never degrades to `Math.random` (fail closed, the whole point of #1751). |
| **`crypto.randomUUID()`** | RFC 4122 version 4 UUID built from 16 CSPRNG bytes `[wd]`. Version and variant are set explicitly: `bytes[6] = (bytes[6] & 0x0f) \| 0x40`, `bytes[8] = (bytes[8] & 0x3f) \| 0x80`. Output is lowercase hex in `8-4-4-4-12` form, so `s[14] === "4"` and `s[19] ∈ {"8","9","a","b"}`, `s.length === 36`. | n/a | host-op failure → `Error` — `crypto.randomUUID: OS entropy unavailable: <detail>` | Same as SSR (CSPRNG-backed). | Same as SSG. | Consumes 16 bytes per call, well under the quota. |
| **`crypto.subtle.digest(alg, data)`** | Implemented for `SHA-1`, `SHA-256`, `SHA-384`, `SHA-512` `[wd]`, computed by a Rust host op (`sha1` / `sha2`, already present transitively in `Cargo.lock`) rather than hand-rolled JS. `alg` accepts the string form or `{ name }`, case-insensitive `[spec]`. Returns a `Promise<ArrayBuffer>`. **`MD5` is deliberately NOT implemented** — workerd supports it as a legacy extension `[wd]`; this host fails it closed (divergence row below). | input is already in memory; no extra cap | unknown or unsupported algorithm → rejects with `Error`, `name = "NotSupportedError"`, message `crypto.subtle.digest: unsupported algorithm "<alg>". This host implements SHA-1, SHA-256, SHA-384, SHA-512.` | Same as SSR — digest is pure computation with no network or policy dimension, so it is enabled on both paths (this **replaces** today's unconditional rejection at ~:671). | Same as SSG. | CPU-bound and bounded by the caller's own input; no cancellation surface. |
| **`crypto.subtle.timingSafeEqual(a, b)`** | Implemented `[wd]` (a workerd extension, not WebCrypto). Constant-time comparison of two `ArrayBuffer`/`ArrayBufferView`s of equal `byteLength`. Unequal lengths throw `[wd]`. Synchronous, returns `boolean`. | n/a | `TypeError` — `crypto.subtle.timingSafeEqual: buffers must be the same byteLength` | Same as SSR. | Same as SSG. | n/a |
| **Every other SubtleCrypto method** | `encrypt`, `decrypt`, `sign`, `verify`, `generateKey`, `deriveKey`, `deriveBits`, `importKey`, `exportKey`, `wrapKey`, `unwrapKey` are **present as callable methods that fail closed**, plus a `DigestStream` constructor that throws. They are deliberately NOT absent: silent absence makes feature detection (`typeof crypto.subtle.sign === "function"`) take a fallback branch locally that production would never take, which is exactly the class of divergence #1751 is about. | n/a | rejects (throws, for `DigestStream`) with `Error`, `name = "NotSupportedError"`, message `crypto.subtle.<method> is not implemented in the zfb embedded runtime. Production Cloudflare Workers DOES implement this call — see research/2013-request-time-capability-contract.md. This host implements digest (SHA-1/256/384/512) and timingSafeEqual only.` | Same as SSR. | Same as SSG. | n/a |
| **`CryptoKey`** | Not defined. No method in the implemented set produces or consumes one. | n/a | referencing it is a plain `ReferenceError` | Same as SSR. | Same as SSG. | n/a |

### Mode distinction

| Feature | Supported behavior | Limits | Error type / message | SSG behavior | SSR behavior | Cancellation / resource limits |
| --- | --- | --- | --- | --- | --- | --- |
| **Dispatch mode** | A **per-dispatch** flag, not a per-host one. `crates/zfb/src/ssr_adapter.rs` drives request-time SSR through the *same* `EmbeddedV8RenderHost` that the dev tick uses to prerender SSG pages, so a host-level flag would either deny legitimate SSR or open build-time network access. New `DispatchMode { BuildTime, RequestTime }` on `HttpRequestLike` (`crates/zfb-render/src/embedded_v8/dispatch.rs`), defaulting to `BuildTime`; threaded through `dispatch_fetch` into `__zfb.dispatch(url, method, headers, body, mode)` in `embedded_v8/js/globals_shim.js`, which sets `__zfb.mode` for the duration of the dispatch and **restores it in a `finally`** so a throwing dispatch cannot leak request-time capability into the next build-time render. `web_polyfills.js` reads `__zfb.mode`. | one flag, two values | n/a | `BuildTime` | `RequestTime` | Reset is unconditional (`finally`), so mode cannot outlive its dispatch. |
| **Default is fail-safe** | `BuildTime` is the default at every layer: the `Default` impl, `HttpRequestLike::get`, and the JS reader when `__zfb.mode` is absent. Any call site not explicitly updated keeps the existing SSG denial. | n/a | n/a | default | must be opted into | n/a |
| **Who sets `RequestTime`** | Exactly one production call site: the dev SSR path, `EmbeddedV8Host::dispatch_fetch_full` as reached from `crates/zfb/src/ssr_adapter.rs`. Everything else — `zfb-build`'s SSG page renderer, `crates/zfb/src/render_pipeline.rs` prerendering, `paths()` evaluation, `ThreadedConfigEvaluator` — stays `BuildTime`. Production Workers routes never go through this host at all; workerd runs the bundle directly. | n/a | n/a | all other call sites | one call site | n/a |

## Deliberate divergences from production workerd

Recorded here so #2020 (workerd parity) checks them rather than "fixing" them, and so #2018's
"align local diagnostics with production" requirement has an explicit answer where alignment is
impossible.

| # | Divergence | Why | What would change it |
| --- | --- | --- | --- |
| D1 | 30 s per-`fetch` timeout; workerd has none while the client stays connected. | SSR dispatches serialise on one mutex and one V8 thread (`ssr_adapter.rs` "Concurrency"). One hung `fetch` wedges the entire dev server, not just one request. A bounded default fails loudly instead of hanging. | Nothing — but `ZFB_SSR_FETCH_TIMEOUT_MS` lets a user with a genuinely slow upstream raise it. |
| D2 | 100 MB caps on request and response bodies; workerd enforces no outbound cap and no response cap. | The host buffers whole bodies in memory (no `ReadableStream`). An unbounded response is an OOM of the developer's machine. 100 MB is Cloudflare's *smallest* documented inbound body limit, so nothing that passes locally is too large for any plan. | Streaming bodies landing in this host, which would make the cap unnecessary. |
| D3 | `Response.body` is `null`; no `ReadableStream`, no streaming request bodies. | The host has never had a stream type; `dispatch_fetch` already materialises every body as bytes. Adding streams is a separate, larger piece of work. | A route that genuinely needs incremental streaming in dev. |
| D4 | Errors are `Error` instances with a spec-correct `.name` (`AbortError`, `TimeoutError`, `TypeMismatchError`, `QuotaExceededError`, `NotSupportedError`), not real `DOMException`s. | `DOMException` is not defined in this host and `deno_core` is not wired for it. Code that checks `err.name` — the common idiom — behaves identically; code that checks `instanceof DOMException` does not. | Installing a `DOMException` polyfill, a cheap follow-up if anything trips on it. |
| D5 | Loopback and private addresses are reachable; production Workers cannot reach them. | Deliberate: dev talking to a local API is a feature, and guardrail 3 mandates loopback test servers. | Nothing. This one is intended and permanent. |
| D6 | No `accept-encoding` sent, automatic decompression disabled; workerd negotiates compression transparently. | Byte-verbatim responses make tests deterministic and keep `content-length` honest against the response-size cap. | A real upstream that compresses unconditionally regardless of `accept-encoding`. |
| D7 | `crypto.subtle.digest` does not support `MD5`; workerd does, as a documented legacy extension. | MD5 is cryptographically broken and nothing in zfb's own runtime needs it. Failing closed with a message that names the divergence is better than quietly enabling it. | A real consumer that must interoperate with an MD5-based legacy system. |
| D8 | Key-bearing SubtleCrypto (`encrypt`/`decrypt`/`sign`/`verify`/`generateKey`/`deriveKey`/`deriveBits`/`importKey`/`exportKey`/`wrapKey`/`unwrapKey`) and `DigestStream` fail closed; workerd implements the full matrix (RSA, ECDSA, Ed25519, X25519, AES-\*, HKDF, PBKDF2, HMAC). **Local diagnostics cannot match production here — production succeeds.** The mitigation is that the failure message says so explicitly and names this document. | The full matrix is a large Rust crypto surface (key import/export formats, JWK, curve handling) — an epic of its own, not one wave. `digest` is what #1751 names, is keyless, and covers ETag / content-hash / request-id, the overwhelming majority of SSR usage. | Real demand. The fail-closed message is the signal that will surface it. |
| D9 | 50 subrequests per dispatch; Cloudflare Free is 50 but Paid is far higher and now configurable. | Pinning the floor means a bundle that works locally works on every plan. | A consumer on a Paid plan legitimately exceeding 50; the constant is one edit. |

## Note on guardrail 1 (never block the isolate thread)

Guardrail 1 targets **network** I/O and is binding for `fetch`: `op_zfb_fetch` must be
`#[op2(async)]`, returning a future that `deno_core`'s event loop polls, using the workspace's
existing `reqwest 0.12` with `rustls-tls` (already in `Cargo.lock` — no new TLS stack, and none of
the `deno_fetch` hyper/tower surface the `zfb-render` `Cargo.toml` rejects) in **non-blocking**
mode on the host's current-thread tokio runtime.

The two crypto ops are deliberately **synchronous**, and this is not a violation:

- `op_zfb_random_bytes` — `crypto.getRandomValues` is synchronous by specification and cannot be
  made async without breaking every caller. `getrandom` reads the kernel CSPRNG (`getrandom(2)` /
  `BCryptGenRandom`), which performs no network or disk I/O and does not block after boot.
- `op_zfb_digest` — CPU-bound over a buffer the caller already holds in memory. It is wrapped in an
  already-resolved promise on the JS side so `crypto.subtle.digest` still returns a `Promise`, as
  the spec requires.

## Testing posture this contract implies

- **Loopback only.** Every fetch test drives a deterministic local server. No public internet
  (guardrail 3).
- **Crypto is tested by invariant plus wiring, never by distribution.** Assert eligibility rules,
  the quota boundary, UUID version/variant bits, and *that the OS entropy op was invoked* — the
  latter via an injectable entropy source or a call counter. Do **not** write statistical shape
  tests; they are flaky and cannot demonstrate cryptographic security (guardrail 7, which overrides
  #1751's own phrasing).
- **The SSG denial gets its own direct regression test**, asserting the byte-identical existing
  message (guardrail 4).
- **Resource exhaustion and host-op failure are explicit cases** (guardrail 6): oversized response,
  subrequest-count overflow, transport unavailable, entropy op failure.
- **Fetch and crypto are implemented serially, never in parallel** — they share
  `web_polyfills.js`, the op registration in `embedded_v8::build_extensions` (today `vec![]`), and
  `Cargo.lock` (guardrail 8).
- `cargo check --no-default-features -p zfb --tests` after every wave — the `embed_v8` cfg boundary
  is not covered by `pnpm b4push`.
