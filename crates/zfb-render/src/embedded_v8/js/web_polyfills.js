// Minimal Web Platform API polyfills for the embedded V8 host.
//
// The SSG path never makes outgoing network requests, so the build-time
// `fetch` is a loud denial rather than a transport. The REQUEST-TIME
// path does make them, through the Rust op `op_zfb_fetch` (epic #2012,
// contract: research/2013-request-time-capability-contract.md) — see
// the `fetch` section below for which side enforces what.
//
// Hono and similar workerd-style routers only need:
//
// - `Request` / `Response` / `Headers` constructible from the shapes
//   they document.
// - `URL` / `URLSearchParams` (deno_core re-exports the WHATWG URL
//   parser via the `url` Rust crate, but does not expose it as a
//   global; we wrap it here).
// - `TextEncoder` / `TextDecoder` for body encoding.
// - `atob` / `btoa` for base64.
// - `structuredClone` for object cloning (workerd ships it; some
//   user libraries assume its presence).
// - `crypto` — `getRandomValues` / `randomUUID` backed by the OS
//   CSPRNG (never `Math.random`, bug #1751), `subtle.digest` for
//   SHA-1/256/384/512, `subtle.timingSafeEqual`, and the rest of
//   SubtleCrypto present-and-failing-closed. See the `crypto` section
//   below.
// - `MessageChannel` / `MessagePort` — React 19's react-dom server
//   bundle constructs a `MessageChannel` at module-load time
//   (unguarded) for its Fizz `scheduleWork` scheduler. React 18 did
//   not; this is the next.16 gap. Promise/microtask-backed.
// - `setTimeout` / `clearTimeout` — React 19's `handleErrorInNextTick`
//   re-throws SSR errors via `setTimeout`; without it the error is
//   masked by `ReferenceError: setTimeout is not defined`.
//   queueMicrotask-backed, no real delay (see the impl docblock).
// - `AbortController` / `AbortSignal` — added by issue #2016 for the
//   request-time `fetch` branch; neither existed here before.
//
// All polyfills live in the global object so module-shape detection
// (`typeof Request === "function"`) reports as you'd expect on
// workerd.
//
// What we deliberately DO NOT polyfill:
//
// - Outgoing `fetch(url)` during BUILD-TIME render — that `fetch`
//   rejects so user code accidentally making a network call during SSG
//   fails loudly instead of silently hanging. Request-time SSR is a
//   separate branch and does reach the network.
// - `WebSocket`, `EventSource`, `Worker` — none are sensible in a
//   build-time renderer.
// - `ReadableStream` — no stream type exists here at all (divergence
//   D3): `response.body` is `null`, and a `ReadableStream` request body
//   is rejected rather than coerced.
//
// If a real zfb project bundle surfaces a missing API,
// fix it here, not at the call site.

(function installWebPolyfills(globalThis) {
  // ---- Headers ---------------------------------------------------
  //
  // RFC 7230 §3.2: header field names are case-insensitive. The
  // WHATWG `Headers` spec lowercases on store + iteration. We
  // mirror that.
  class Headers {
    constructor(init) {
      // Raw ordered list of `[lowercaseName, value]` pairs. Every
      // `append()` (including ones the constructor drives) pushes a new
      // pair rather than comma-joining immediately — this is what lets
      // duplicate `set-cookie` values (whose own `Expires` attribute may
      // itself contain a comma) survive untouched. Ordinary headers are
      // combined lazily by `get()`/iteration instead — see
      // `_combinedEntries()`, which mirrors the WHATWG Fetch "sort and
      // combine" algorithm.
      this._pairs = [];
      if (init == null) return;
      if (init instanceof Headers) {
        // Clone: copy the raw pairs so duplicate set-cookie entries in
        // `init` survive into the clone too.
        this._pairs = init._pairs.map((pair) => [pair[0], pair[1]]);
        return;
      }
      if (Array.isArray(init)) {
        for (const entry of init) {
          if (!Array.isArray(entry) || entry.length !== 2) {
            throw new TypeError("Invalid Headers init entry");
          }
          this.append(entry[0], entry[1]);
        }
        return;
      }
      if (typeof init === "object") {
        for (const k of Object.keys(init)) {
          this.append(k, init[k]);
        }
        return;
      }
      throw new TypeError("Invalid Headers init");
    }
    append(name, value) {
      this._pairs.push([String(name).toLowerCase(), String(value)]);
    }
    delete(name) {
      const k = String(name).toLowerCase();
      this._pairs = this._pairs.filter(([n]) => n !== k);
    }
    get(name) {
      const k = String(name).toLowerCase();
      const values = this._pairs.filter(([n]) => n === k).map(([, v]) => v);
      if (values.length === 0) return null;
      // Per the Fetch `Headers.get()` algorithm this comma-joins
      // unconditionally, `set-cookie` included — even though an
      // `Expires` attribute's own comma can make that join ambiguous.
      // That's a known, spec-sanctioned wart: `getSetCookie()` is the
      // lossless API for callers that need every value uncombined.
      return values.join(", ");
    }
    // WHATWG addition (not in the original Fetch `Headers.get` steps):
    // returns every `set-cookie` value uncombined, in append order. The
    // only spec-correct way to read multiple Set-Cookie values back out.
    getSetCookie() {
      return this._pairs.filter(([n]) => n === "set-cookie").map(([, v]) => v);
    }
    has(name) {
      const k = String(name).toLowerCase();
      return this._pairs.some(([n]) => n === k);
    }
    set(name, value) {
      const k = String(name).toLowerCase();
      this._pairs = this._pairs.filter(([n]) => n !== k);
      this._pairs.push([k, String(value)]);
    }
    forEach(cb, thisArg) {
      for (const [k, v] of this._liveCombinedEntries()) {
        cb.call(thisArg, v, k, this);
      }
    }
    *keys() {
      for (const [k] of this._liveCombinedEntries()) yield k;
    }
    *values() {
      for (const [, v] of this._liveCombinedEntries()) yield v;
    }
    *entries() {
      for (const pair of this._liveCombinedEntries()) yield pair;
    }
    [Symbol.iterator]() {
      return this.entries();
    }
    // LIVE map iteration, per the WHATWG "iterate a map" semantics
    // (https://infra.spec.whatwg.org/#map-iterate) that Fetch's `Headers`
    // iterator inherits: iteration is index-based over the *current*
    // combined view, re-derived at every step rather than snapshotted up
    // front. So mutations made mid-traversal are observed — deleting a
    // not-yet-visited header removes it from later steps (it is NOT
    // yielded), and appending a header makes it visible to a later step
    // (it IS yielded). Native `Headers` (and the previous Map-backed
    // polyfill) behave this way; a snapshot would yield stale results.
    // The non-mutating order is identical to the snapshot order because
    // `_combinedEntries()` is deterministic, so existing iteration tests
    // stay green.
    *_liveCombinedEntries() {
      let i = 0;
      for (;;) {
        const combined = this._combinedEntries();
        if (i >= combined.length) return;
        yield combined[i];
        i++;
      }
    }
    // WHATWG Fetch "sort and combine" algorithm
    // (https://fetch.spec.whatwg.org/#concept-header-list-sort-and-combine):
    // header names are visited in sorted order; `set-cookie` values are
    // yielded uncombined (one entry per value, Expires-comma-safe) while
    // every other name's values are comma-joined into a single entry.
    // Backs get/entries/keys/values/forEach/Symbol.iterator (through
    // `_liveCombinedEntries`) so all iteration surfaces agree.
    _combinedEntries() {
      const names = Array.from(new Set(this._pairs.map(([n]) => n))).sort();
      const out = [];
      for (const name of names) {
        if (name === "set-cookie") {
          for (const [n, v] of this._pairs) {
            if (n === name) out.push([n, v]);
          }
        } else {
          out.push([name, this.get(name)]);
        }
      }
      return out;
    }
  }

  // ---- URLSearchParams ------------------------------------------
  //
  // We accept string / array / object init shapes (matching the
  // WHATWG spec). The actual parsing is done by splitting on '&'
  // / '=' — no encoding subtleties are required for the SSG path
  // (router params are typically already encoded by the framework),
  // except for the `application/x-www-form-urlencoded` `+`-means-space
  // rule, which callers rely on (e.g. HTML forms encode spaces as `+`).
  //
  // decode: `+` must become a space BEFORE decodeURIComponent runs,
  // since decodeURIComponent leaves a literal `+` untouched.
  function decodeFormUrlComponent(piece) {
    return decodeURIComponent(piece.replace(/\+/g, " "));
  }
  // encode: encodeURIComponent renders a space as `%20`; form-urlencoded
  // serialization renders it as `+` instead.
  function encodeFormUrlComponent(str) {
    return encodeURIComponent(str).replace(/%20/g, "+");
  }
  class URLSearchParams {
    constructor(init) {
      this._pairs = [];
      if (init == null || init === "") return;
      if (typeof init === "string") {
        const trimmed = init.startsWith("?") ? init.slice(1) : init;
        if (trimmed === "") return;
        for (const piece of trimmed.split("&")) {
          if (piece === "") continue;
          const eq = piece.indexOf("=");
          if (eq < 0) {
            this._pairs.push([decodeFormUrlComponent(piece), ""]);
          } else {
            const k = decodeFormUrlComponent(piece.slice(0, eq));
            const v = decodeFormUrlComponent(piece.slice(eq + 1));
            this._pairs.push([k, v]);
          }
        }
        return;
      }
      if (Array.isArray(init)) {
        for (const [k, v] of init) {
          this._pairs.push([String(k), String(v)]);
        }
        return;
      }
      for (const k of Object.keys(init)) {
        this._pairs.push([k, String(init[k])]);
      }
    }
    append(k, v) {
      this._pairs.push([String(k), String(v)]);
    }
    delete(k) {
      const key = String(k);
      this._pairs = this._pairs.filter(([n]) => n !== key);
    }
    get(k) {
      const key = String(k);
      for (const [n, v] of this._pairs) {
        if (n === key) return v;
      }
      return null;
    }
    getAll(k) {
      const key = String(k);
      return this._pairs.filter(([n]) => n === key).map(([, v]) => v);
    }
    has(k) {
      const key = String(k);
      return this._pairs.some(([n]) => n === key);
    }
    set(k, v) {
      this.delete(k);
      this.append(k, v);
    }
    sort() {
      this._pairs.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
    }
    *keys() {
      for (const [k] of this._pairs) yield k;
    }
    *values() {
      for (const [, v] of this._pairs) yield v;
    }
    *entries() {
      for (const p of this._pairs) yield [p[0], p[1]];
    }
    [Symbol.iterator]() {
      return this.entries();
    }
    toString() {
      return this._pairs
        .map(([k, v]) => encodeFormUrlComponent(k) + "=" + encodeFormUrlComponent(v))
        .join("&");
    }
  }

  // ---- URL -------------------------------------------------------
  //
  // We don't fully reimplement WHATWG URL parsing in pure JS — that
  // would be 1000+ lines. Instead we use a minimal parser that
  // covers the cases workerd-style routers see:
  //   "http://example.com/path?q=1#frag"
  //   "https://example.com/blog/[slug]"
  //   relative resolution against a base URL is rare on the SSG
  //   path; we support the simple `/path` + base case.
  //
  // The tradeoff: the polyfill's parser is *less strict* than the
  // WHATWG spec — it does not normalise paths, does not encode
  // userinfo, and accepts inputs that would round-trip differently
  // through Chrome. The SSG renderer always feeds it
  // already-validated URLs from the route table, so the relaxed
  // parser is safe in practice.
  class URL {
    constructor(input, base) {
      let resolved;
      if (base != null && !/^[a-z][a-z0-9+\-.]*:/i.test(input)) {
        // Relative URL — resolve against base.
        const baseUrl = base instanceof URL ? base : new URL(base);
        if (input.startsWith("//")) {
          resolved = baseUrl.protocol + input;
        } else if (input.startsWith("/")) {
          resolved = baseUrl.protocol + "//" + baseUrl.host + input;
        } else if (input.startsWith("?") || input.startsWith("#")) {
          resolved = baseUrl.protocol + "//" + baseUrl.host + baseUrl.pathname + input;
        } else {
          // Drop last path segment of base.
          const lastSlash = baseUrl.pathname.lastIndexOf("/");
          const dir = lastSlash >= 0 ? baseUrl.pathname.slice(0, lastSlash + 1) : "/";
          resolved = baseUrl.protocol + "//" + baseUrl.host + dir + input;
        }
      } else {
        resolved = String(input);
      }
      // Parse: scheme://host:port/path?query#fragment
      const schemeMatch = /^([a-z][a-z0-9+\-.]*:)(\/\/)?/i.exec(resolved);
      if (!schemeMatch) {
        throw new TypeError("Invalid URL: " + resolved);
      }
      this.protocol = schemeMatch[1].toLowerCase();
      let rest = resolved.slice(schemeMatch[0].length);
      const hashIdx = rest.indexOf("#");
      let hash = "";
      if (hashIdx >= 0) {
        hash = rest.slice(hashIdx);
        rest = rest.slice(0, hashIdx);
      }
      const queryIdx = rest.indexOf("?");
      let search = "";
      if (queryIdx >= 0) {
        search = rest.slice(queryIdx);
        rest = rest.slice(0, queryIdx);
      }
      let host = "";
      let pathname = rest;
      if (schemeMatch[2] === "//") {
        const slashIdx = rest.indexOf("/");
        if (slashIdx < 0) {
          host = rest;
          pathname = "/";
        } else {
          host = rest.slice(0, slashIdx);
          pathname = rest.slice(slashIdx);
          if (pathname === "") pathname = "/";
        }
      }
      this.host = host;
      const colonIdx = host.indexOf(":");
      if (colonIdx < 0) {
        this.hostname = host;
        this.port = "";
      } else {
        this.hostname = host.slice(0, colonIdx);
        this.port = host.slice(colonIdx + 1);
      }
      this.pathname = pathname || "/";
      this.search = search;
      this.hash = hash;
      this.searchParams = new URLSearchParams(search);
    }
    get origin() {
      return this.protocol + "//" + this.host;
    }
    get href() {
      return this.toString();
    }
    set href(_) {
      throw new Error("URL.href setter not implemented");
    }
    toString() {
      return (
        this.protocol +
        (this.host ? "//" + this.host : "") +
        this.pathname +
        this.search +
        this.hash
      );
    }
    toJSON() {
      return this.toString();
    }
  }

  // ---- TextEncoder / TextDecoder --------------------------------
  //
  // Pure-JS UTF-8 encode/decode. Suffices for the SSG path's HTML
  // serialisation; we do not implement decoding of non-UTF-8
  // labels (Hono / Preact don't ask for that).
  class TextEncoder {
    constructor() {
      this.encoding = "utf-8";
    }
    encode(input) {
      const str = input == null ? "" : String(input);
      const out = [];
      for (let i = 0; i < str.length; i++) {
        let c = str.charCodeAt(i);
        if (c >= 0xd800 && c <= 0xdbff && i + 1 < str.length) {
          const c2 = str.charCodeAt(i + 1);
          if (c2 >= 0xdc00 && c2 <= 0xdfff) {
            c = 0x10000 + ((c - 0xd800) << 10) + (c2 - 0xdc00);
            i++;
          }
        }
        if (c < 0x80) {
          out.push(c);
        } else if (c < 0x800) {
          out.push(0xc0 | (c >> 6));
          out.push(0x80 | (c & 0x3f));
        } else if (c < 0x10000) {
          out.push(0xe0 | (c >> 12));
          out.push(0x80 | ((c >> 6) & 0x3f));
          out.push(0x80 | (c & 0x3f));
        } else {
          out.push(0xf0 | (c >> 18));
          out.push(0x80 | ((c >> 12) & 0x3f));
          out.push(0x80 | ((c >> 6) & 0x3f));
          out.push(0x80 | (c & 0x3f));
        }
      }
      return new Uint8Array(out);
    }
  }

  class TextDecoder {
    constructor(label, options) {
      this.encoding = (label || "utf-8").toLowerCase();
      this.fatal = !!(options && options.fatal);
      this.ignoreBOM = !!(options && options.ignoreBOM);
    }
    decode(input) {
      if (input == null) return "";
      const bytes = input instanceof Uint8Array ? input : new Uint8Array(input.buffer || input);
      let out = "";
      let i = 0;
      while (i < bytes.length) {
        const b = bytes[i];
        if (b < 0x80) {
          out += String.fromCharCode(b);
          i++;
        } else if (b < 0xc0) {
          // continuation byte at start — invalid; emit replacement.
          out += "�";
          i++;
        } else if (b < 0xe0) {
          if (i + 1 >= bytes.length) {
            out += "�";
            break;
          }
          const c = ((b & 0x1f) << 6) | (bytes[i + 1] & 0x3f);
          out += String.fromCharCode(c);
          i += 2;
        } else if (b < 0xf0) {
          if (i + 2 >= bytes.length) {
            out += "�";
            break;
          }
          const c = ((b & 0x0f) << 12) | ((bytes[i + 1] & 0x3f) << 6) | (bytes[i + 2] & 0x3f);
          out += String.fromCharCode(c);
          i += 3;
        } else {
          if (i + 3 >= bytes.length) {
            out += "�";
            break;
          }
          let c =
            ((b & 0x07) << 18) |
            ((bytes[i + 1] & 0x3f) << 12) |
            ((bytes[i + 2] & 0x3f) << 6) |
            (bytes[i + 3] & 0x3f);
          c -= 0x10000;
          out += String.fromCharCode(0xd800 + (c >> 10), 0xdc00 + (c & 0x3ff));
          i += 4;
        }
      }
      return out;
    }
  }

  // ---- Body helpers ---------------------------------------------
  //
  // We treat the body as `Uint8Array` internally. Both Request and
  // Response need to expose `text()`, `json()`, `arrayBuffer()`.

  // WHATWG Fetch "extract a body" step 5 (the BodyInit → default
  // Content-Type table): a `string` body defaults to
  // `text/plain;charset=UTF-8`, a `URLSearchParams` body defaults to
  // `application/x-www-form-urlencoded;charset=UTF-8`. Typed arrays /
  // `ArrayBuffer` get NO automatic type per spec. Returns `null` when
  // no default applies (caller must supply an explicit header).
  function bodyInitContentType(body) {
    if (typeof body === "string") return "text/plain;charset=UTF-8";
    if (body instanceof URLSearchParams) {
      return "application/x-www-form-urlencoded;charset=UTF-8";
    }
    return null;
  }

  function bodyToUint8Array(input) {
    if (input == null) return new Uint8Array(0);
    if (input instanceof Uint8Array) return input;
    if (input instanceof ArrayBuffer) return new Uint8Array(input);
    if (typeof input === "string") {
      return new TextEncoder().encode(input);
    }
    if (input && typeof input.byteLength === "number" && input.buffer instanceof ArrayBuffer) {
      return new Uint8Array(input.buffer, input.byteOffset || 0, input.byteLength);
    }
    // Anything else — coerce to string and encode.
    return new TextEncoder().encode(String(input));
  }

  class _Body {
    constructor(body) {
      this._bodyBytes = bodyToUint8Array(body);
      // Whether a payload was supplied AT ALL, which `_bodyBytes` alone
      // cannot say — `null` and a zero-length body both encode to an
      // empty `Uint8Array`, and the request-time `fetch` has to tell
      // them apart twice over: `GET`/`HEAD` with a *present* body is a
      // `TypeError`, and the transport's `hasBody` flag decides whether
      // a request is framed with a body at all.
      this._bodyPresent = body != null;
      // The BodyInit-derived default Content-Type, captured here rather
      // than recomputed at send time. By the time `fetch` sees a
      // `Request` the body is already `Uint8Array` bytes, and bytes
      // carry no default type — so recomputing there would silently
      // drop the header for `fetch(new Request(url, { body: "x" }))`
      // while keeping it for the equivalent url-plus-init call.
      this._bodyDefaultContentType = bodyInitContentType(body);
      // A `ReadableStream`-shaped body, remembered because the
      // conversion above has already coerced it beyond recognition.
      // Construction still succeeds (that is the pre-existing
      // behaviour); it is the request-time `fetch` that must refuse it,
      // and it can only do so if the shape was recorded here — a stream
      // handed to `new Request(...)` first would otherwise reach the
      // wire as "[object Object]".
      this._bodyIsStream = isReadableStreamLike(body);
      // Names this half of the interface in the not-implemented
      // messages below (`response.blob()`, `request.formData()`).
      // Subclasses overwrite it.
      this._bodyLabel = "body";
      this.bodyUsed = false;
    }
    async arrayBuffer() {
      if (this.bodyUsed) {
        throw new TypeError("Body already consumed");
      }
      this.bodyUsed = true;
      return this._bodyBytes.buffer.slice(
        this._bodyBytes.byteOffset,
        this._bodyBytes.byteOffset + this._bodyBytes.byteLength,
      );
    }
    async text() {
      if (this.bodyUsed) {
        throw new TypeError("Body already consumed");
      }
      this.bodyUsed = true;
      return new TextDecoder().decode(this._bodyBytes);
    }
    async json() {
      const t = await this.text();
      return JSON.parse(t);
    }
    // `blob()` / `formData()` are unimplemented on BOTH paths, so their
    // message must name neither. The old wording ("… in the SSG
    // runtime") told a request-time caller their problem was a
    // build-time policy — exactly the misdiagnosis epic #2012 exists to
    // remove. The one message that may still say "SSG runtime" is the
    // build-time `fetch()` denial below, which genuinely IS that policy.
    async blob() {
      throw new Error(this._bodyLabel + ".blob() is not implemented in the zfb embedded runtime");
    }
    async formData() {
      throw new Error(
        this._bodyLabel + ".formData() is not implemented in the zfb embedded runtime",
      );
    }
  }

  // ---- Request --------------------------------------------------
  class Request extends _Body {
    constructor(input, init) {
      let url;
      let baseInit = {};
      if (input instanceof Request) {
        url = input.url;
        // A faithful copy, not just url/method/headers/body: `fetch`
        // must honour a `signal` and a `redirect` mode carried by a
        // `Request` the caller built earlier (contract row "Abort":
        // *both* `init.signal` and `Request.signal` are honoured), and
        // `new Request(req)` is how that request reaches it.
        baseInit = {
          method: input.method,
          headers: input.headers,
          // `_bodyBytes` is an empty `Uint8Array` for a body-less
          // request, which is NOT the same as a zero-length body — pass
          // `null` through so the copy keeps the distinction.
          body: input._bodyPresent ? input._bodyBytes : null,
          signal: input.signal,
          redirect: input.redirect,
          cache: input.cache,
          credentials: input.credentials,
          mode: input.mode,
          referrer: input.referrer,
          integrity: input.integrity,
        };
      } else {
        url = String(input);
      }
      const finalInit = Object.assign({}, baseInit, init || {});
      super(finalInit.body);
      this._bodyLabel = "request";
      // When the body came from a source `Request` rather than from
      // `init`, it arrived as bytes — so the two properties derived
      // from the ORIGINAL BodyInit have to be carried across by hand or
      // they are lost in the copy.
      if (input instanceof Request && !(init && "body" in init)) {
        this._bodyDefaultContentType = input._bodyDefaultContentType;
        this._bodyIsStream = input._bodyIsStream;
      }
      this.url = url;
      this.method = (finalInit.method || "GET").toUpperCase();
      this.headers =
        finalInit.headers instanceof Headers ? finalInit.headers : new Headers(finalInit.headers);
      this.signal = finalInit.signal || null;
      this.cache = finalInit.cache || "default";
      this.credentials = finalInit.credentials || "same-origin";
      this.mode = finalInit.mode || "cors";
      this.redirect = finalInit.redirect || "follow";
      this.referrer = finalInit.referrer || "";
      this.integrity = finalInit.integrity || "";
    }
    clone() {
      return new Request(this.url, {
        method: this.method,
        headers: this.headers,
        body: this._bodyPresent ? this._bodyBytes : null,
        signal: this.signal,
        redirect: this.redirect,
      });
    }
  }

  // ---- Response -------------------------------------------------
  class Response extends _Body {
    constructor(body, init) {
      super(body);
      this._bodyLabel = "response";
      const i = init || {};
      this.status = i.status == null ? 200 : Number(i.status);
      this.statusText = i.statusText || statusText(this.status);
      this.ok = this.status >= 200 && this.status < 300;
      // Always clone `init.headers` into a fresh `Headers` instance
      // (the `Headers` constructor already copies pairs out of a
      // `Headers` init) rather than aliasing the caller's object —
      // otherwise the BodyInit default `set()` below would mutate a
      // `Headers` instance the caller still holds a reference to.
      this.headers = new Headers(i.headers);
      if (!this.headers.has("content-type")) {
        // Captured by `_Body` from the original BodyInit — same table,
        // one place.
        const defaultType = this._bodyDefaultContentType;
        if (defaultType) {
          this.headers.set("content-type", defaultType);
        }
      }
      this.type = "default";
      this.url = i.url || "";
      this.redirected = false;
      // ALWAYS `null` — this host has no `ReadableStream` (contract
      // divergence D3), and every body is materialised as bytes before
      // a `Response` exists. Present as an own property rather than
      // absent so `response.body === null` reads as it does in
      // production instead of `undefined`; `text()` / `arrayBuffer()` /
      // `json()` are the ways to reach the bytes.
      this.body = null;
    }
    clone() {
      const copy = new Response(this._bodyPresent ? this._bodyBytes : null, {
        status: this.status,
        statusText: this.statusText,
        headers: this.headers,
        url: this.url,
      });
      // Not `init` members, so they are carried across by hand — a
      // clone of a redirected response is still redirected.
      copy.redirected = this.redirected;
      copy.type = this.type;
      return copy;
    }
    static error() {
      const r = new Response(null, { status: 0 });
      r.type = "error";
      return r;
    }
    static redirect(url, status) {
      const s = status == null ? 302 : status;
      return new Response(null, {
        status: s,
        headers: { location: String(url) },
      });
    }
    static json(data, init) {
      const body = JSON.stringify(data);
      const i = init || {};
      // Install the JSON content-type BEFORE construction: the
      // constructor now defaults a string body to
      // `text/plain;charset=UTF-8` when no header is present, which
      // would otherwise beat a post-hoc `set()` here since `has()`
      // would already be true. An explicit caller header still wins.
      const headers = new Headers(i.headers);
      if (!headers.has("content-type")) {
        headers.set("content-type", "application/json");
      }
      return new Response(body, Object.assign({}, i, { headers }));
    }
  }

  function statusText(code) {
    const map = {
      200: "OK",
      201: "Created",
      204: "No Content",
      301: "Moved Permanently",
      302: "Found",
      304: "Not Modified",
      400: "Bad Request",
      401: "Unauthorized",
      403: "Forbidden",
      404: "Not Found",
      500: "Internal Server Error",
      502: "Bad Gateway",
      503: "Service Unavailable",
    };
    return map[code] || "";
  }

  // ---- dispatch mode --------------------------------------------
  //
  // Reads the per-dispatch signal `globals_shim.js` publishes as
  // `__zfb.mode` for the duration of a dispatch (issue #2014).
  //
  // Fail-safe by construction: ONLY the exact string "request-time"
  // selects the request-time branch. Absent (`__zfb` not installed yet,
  // module evaluation, a caller that never passed a mode), null, or any
  // unrecognised value all read as build-time — the denying default.
  function dispatchMode() {
    const bridge = globalThis.__zfb;
    const mode = bridge ? bridge.mode : undefined;
    return mode === "request-time" ? "request-time" : "build-time";
  }

  // ---- AbortController / AbortSignal ----------------------------
  //
  // Neither existed in this host before issue #2016; the request-time
  // `fetch` branch below needs them (contract row "Abort"). Deliberately
  // minimal — enough of the `EventTarget` surface for the `abort` event,
  // plus the two static constructors the Fetch standard defines.
  //
  // Divergence D4: the abort reason is an `Error` carrying a
  // spec-correct `.name` (`AbortError` / `TimeoutError`), NOT a real
  // `DOMException` — none exists in this host, and inventing a partial
  // one is a rejected design. Code that checks `err.name` (the common
  // idiom) behaves exactly as in production; code that checks
  // `instanceof DOMException` does not.
  function abortError() {
    const e = new Error("The operation was aborted.");
    e.name = "AbortError";
    return e;
  }
  function timeoutError() {
    const e = new Error("The operation was aborted due to timeout.");
    e.name = "TimeoutError";
    return e;
  }

  class AbortSignal {
    constructor() {
      this.aborted = false;
      this.reason = undefined;
      this.onabort = null;
      this._listeners = [];
      // Set only by `AbortSignal.timeout(ms)`. The host has NO event
      // loop timers — `setTimeout` below is microtask-backed and
      // ignores its delay — so a JS-side timer could not honour this
      // signal; firing one would abort every fetch instantly, which is
      // worse than not having it. Instead `fetch` forwards the value to
      // the Rust transport, which owns every wall-clock deadline and
      // can only NARROW its own with it. Consequence, stated plainly:
      // the deadline is real and closes the socket, but this signal
      // object itself never flips to `aborted` when it elapses.
      this._timeoutMs = null;
    }
    addEventListener(type, listener) {
      if (type === "abort" && listener != null) {
        this._listeners.push(listener);
      }
    }
    removeEventListener(type, listener) {
      if (type === "abort") {
        this._listeners = this._listeners.filter((l) => l !== listener);
      }
    }
    throwIfAborted() {
      if (this.aborted) {
        throw this.reason;
      }
    }
    // Internal: the only way `aborted` becomes true. Not part of the
    // public interface — `AbortController.abort()` is.
    _abort(reason) {
      if (this.aborted) return;
      this.aborted = true;
      this.reason = reason;
      const evt = { type: "abort", target: this };
      if (typeof this.onabort === "function") {
        this.onabort(evt);
      }
      for (const listener of this._listeners.slice()) {
        if (typeof listener === "function") {
          listener(evt);
        } else if (listener && typeof listener.handleEvent === "function") {
          listener.handleEvent(evt);
        }
      }
    }
    static abort(reason) {
      const signal = new AbortSignal();
      signal._abort(reason === undefined ? abortError() : reason);
      return signal;
    }
    static timeout(milliseconds) {
      const signal = new AbortSignal();
      const ms = Number(milliseconds);
      if (!Number.isFinite(ms) || ms < 0) {
        throw new TypeError(
          "AbortSignal.timeout: milliseconds must be a non-negative finite number",
        );
      }
      if (ms === 0) {
        // A zero deadline has already elapsed, so this one CAN be
        // honoured here — and an already-aborted signal never opens a
        // socket.
        signal._abort(timeoutError());
        return signal;
      }
      signal._timeoutMs = ms;
      return signal;
    }
  }

  class AbortController {
    constructor() {
      this.signal = new AbortSignal();
    }
    abort(reason) {
      this.signal._abort(reason === undefined ? abortError() : reason);
    }
  }

  // ---- fetch ----------------------------------------------------
  //
  // Build-time render never makes outgoing requests. If a bundle calls
  // `fetch(url)` we reject with a message that names the offending URL
  // so the operator can find the call site. That rejection is deliberate
  // POLICY (guardrail 4 of epic #2012) and its wording is asserted
  // byte-for-byte by `js_fetch_tests.rs` — do not reword it.
  //
  // Request-time SSR takes a DISTINCT branch, which since issue #2016
  // performs a real outbound request through the Rust transport op
  // (`op_zfb_fetch`, issue #2015).
  //
  // ## The one rule this whole surface exists for
  //
  // Everything that is still unsupported at request time fails with a
  // REQUEST-TIME-SPECIFIC diagnostic. The "fetch() called from SSG
  // runtime" wording appears on exactly one path — a genuine build-time
  // render — because reporting a request-time failure as an SSG policy
  // denial is precisely the defect epic #2012 is fixing.
  //
  // ## What is enforced where
  //
  // The scheme allowlist, the redirect rules and hop limit, the response
  // cap, the wall-clock deadline, cancellation, and the per-dispatch
  // subrequest budget are ALL enforced in Rust, where bundle code cannot
  // reach them. This layer does not re-implement any of them. The one
  // deliberate exception is the request-body cap, checked here as well
  // as in Rust — defence in depth, per the contract.
  function fetch(input, init) {
    const url = input instanceof Request ? input.url : String(input);
    if (dispatchMode() === "request-time") {
      // `requestTimeFetch` is `async`, so everything it throws —
      // including the pre-dispatch rejections that must never open a
      // socket — surfaces as a rejected promise, as `fetch` requires.
      return requestTimeFetch(input, init, url);
    }
    return Promise.reject(
      new Error(
        "fetch() called from SSG runtime (url=" +
          url +
          "). The embedded V8 host does not support outgoing network requests during build-time render. Move the data fetch to a build step or a runtime-only branch.",
      ),
    );
  }

  // deno_core rebuilds an op's Rust-side error in JS through its own
  // `buildCustomError` (`00_infra.js`), which can only construct classes
  // present in its `errorMap` — the six ECMAScript builtins. A
  // `JsErrorBox` carrying ANY other class is rebuilt as
  // `TypeError: invalid_argument`, losing BOTH the name and the
  // message. The transport's deadline uses `TimeoutError` (contract row
  // "Timeout", divergence D4), so without this registration the entire
  // timeout diagnostic — deadline, the note that production Workers has
  // no per-subrequest limit — is unreachable from JS, and a timed-out
  // fetch is indistinguishable from a malformed argument.
  //
  // Registered once at install time; `registerErrorBuilder` throws on a
  // duplicate class, and a host that predates the API simply keeps the
  // lossy behaviour rather than failing to boot.
  function registerHostErrorClasses() {
    const core = globalThis.Deno && globalThis.Deno.core;
    if (!core || typeof core.registerErrorClass !== "function") return;
    // Every non-builtin error class a Rust op can raise MUST be listed
    // here. deno_core rebuilds an op's error through its own
    // `buildCustomError`, which can only construct classes in its
    // `errorMap`; an unregistered class arrives in JS as a thrown
    // `undefined` — no name, no message, no diagnostic at all.
    //
    // - `TimeoutError`: the fetch transport's deadline (#2015/#2016).
    // - `QuotaExceededError`: `op_zfb_random_bytes`'s byte quota (#2017).
    // - `NotSupportedError`: `op_zfb_digest`'s unsupported algorithm (#2018).
    const classes = ["TimeoutError", "QuotaExceededError", "NotSupportedError"];
    for (const name of classes) {
      const cls = class extends Error {
        constructor(message) {
          super(message);
          this.name = name;
        }
      };
      try {
        core.registerErrorClass(name, cls);
      } catch (_) {
        // Already registered on this runtime — nothing to do.
      }
    }
  }

  // The Rust-injected limit constants (`globals_shim.js`, issue #2016).
  // Read lazily rather than captured at install time: the polyfills are
  // executed BEFORE the host shim, so `__zfb` does not exist yet here.
  function hostLimits(url) {
    const bridge = globalThis.__zfb;
    const limits = bridge ? bridge.limits : undefined;
    if (!limits) {
      throw new TypeError(
        "fetch(" +
          url +
          "): embedded host transport unavailable: the host did not publish __zfb.limits",
      );
    }
    return limits;
  }

  // A `ReadableStream` body. The type does not exist in this host
  // (divergence D3), so a duck-typed check is the only one available —
  // and it must run BEFORE `Request` coerces the value, since
  // `bodyToUint8Array` would otherwise silently `String()` the stream
  // into a body reading "[object ReadableStream]".
  function isReadableStreamLike(value) {
    if (value == null || typeof value !== "object") return false;
    if (
      typeof globalThis.ReadableStream === "function" &&
      value instanceof globalThis.ReadableStream
    ) {
      return true;
    }
    return typeof value.getReader === "function";
  }

  // Settle as soon as EITHER the transport finishes or `signal` aborts.
  //
  // Known limitation, stated rather than hidden: the op's future is
  // owned by the Rust event loop and cannot be dropped from JS, so an
  // abort arriving MID-FLIGHT settles the caller's promise immediately
  // while the transport runs to its own conclusion and its result is
  // discarded — the socket closes on the Rust deadline, not on the
  // abort. The common case (a signal that is already aborted when
  // `fetch` is called) never opens a socket at all, and the wall-clock
  // deadline does drop the future.
  function raceWithAbort(promise, signal) {
    return new Promise((resolve, reject) => {
      let settled = false;
      const onAbort = () => {
        if (settled) return;
        settled = true;
        reject(signal.reason !== undefined ? signal.reason : abortError());
      };
      signal.addEventListener("abort", onAbort);
      promise.then(
        (value) => {
          if (settled) return;
          settled = true;
          signal.removeEventListener("abort", onAbort);
          resolve(value);
        },
        (error) => {
          if (settled) return;
          settled = true;
          signal.removeEventListener("abort", onAbort);
          reject(error);
        },
      );
    });
  }

  async function requestTimeFetch(input, init, url) {
    const i = init || {};
    const limits = hostLimits(url);

    // One `Request` is the single source of truth for method, headers,
    // body, redirect mode and signal, whether the caller passed a URL
    // string plus `init` or a `Request` they built earlier.
    const request = new Request(input, i);
    const signal = request.signal;

    // Checked off the RECORDED shape (`_Body` sets `_bodyIsStream`)
    // rather than off `i.body`, so a stream that arrived inside an
    // already-constructed `Request` is refused too. Reading `i.body`
    // alone would miss it: `new Request(url, { body: stream })` has by
    // then coerced the stream to "[object Object]", which would reach
    // the wire as a real payload instead of this diagnostic.
    if (request._bodyIsStream) {
      throw new TypeError(
        "fetch(" +
          url +
          "): ReadableStream request bodies are not supported by the zfb embedded runtime",
      );
    }

    // An already-aborted signal rejects WITHOUT opening a socket —
    // asserted by `an_already_aborted_signal_rejects_without_opening_a_socket`
    // against a loopback server's request count, not just by the error.
    if (signal && signal.aborted) {
      throw signal.reason !== undefined ? signal.reason : abortError();
    }

    const method = request.method;
    const hasBody = request._bodyPresent;
    if (hasBody && (method === "GET" || method === "HEAD")) {
      throw new TypeError("Request with GET/HEAD method cannot have body.");
    }

    // Per the Fetch standard, dispatching DISTURBS the request's body:
    // a caller-supplied `Request` may be fetched once, and one whose
    // body was already read is a `TypeError` rather than a silent
    // resend. `new Request(input, init)` above copies the bytes, so
    // without these two lines a body-bearing `Request` could be fetched
    // in a loop — each pass a real network side effect — and a
    // half-consumed one could still be sent.
    const source = input instanceof Request ? input : request;
    if (hasBody) {
      if (source.bodyUsed) {
        throw new TypeError("Body already consumed");
      }
      source.bodyUsed = true;
    }

    const bodyBytes = hasBody ? request._bodyBytes : new Uint8Array(0);
    // Defence in depth: Rust rejects this too (and is the enforcement
    // that matters, since bundle code cannot reach it), but catching it
    // here keeps a 100 MB payload from crossing the op boundary at all.
    // The message is byte-identical to the transport's own.
    if (bodyBytes.byteLength > limits.maxRequestBodyBytes) {
      throw new TypeError(
        "fetch(" +
          url +
          "): request body exceeds the " +
          limits.maxRequestBodyBytes +
          "-byte limit",
      );
    }

    // The RAW ordered pairs, not `entries()`: the latter applies the
    // Fetch "sort and combine" view, which comma-joins duplicates. The
    // outbound direction must carry repeated request headers verbatim,
    // exactly as the response direction does.
    const headers = request.headers._pairs.map((pair) => [pair[0], pair[1]]);
    if (hasBody && !request.headers.has("content-type")) {
      // WHATWG Fetch "extract a body" step 5 — a `string` or
      // `URLSearchParams` body carries a default Content-Type. Read off
      // the `Request`, which recorded it from the ORIGINAL BodyInit:
      // `i.body` is undefined when the caller passed a `Request`, and
      // by then the body is bytes, which carry no default at all.
      const defaultType = request._bodyDefaultContentType;
      if (defaultType) {
        headers.push(["content-type", defaultType]);
      }
    }

    const ops = globalThis.Deno && globalThis.Deno.core ? globalThis.Deno.core.ops : undefined;
    const op = ops ? ops.op_zfb_fetch : undefined;
    if (typeof op !== "function") {
      // Never a synthetic empty `Response`: a silent empty body is
      // indistinguishable from a real 200 with no content.
      throw new TypeError(
        "fetch(" +
          url +
          "): embedded host transport unavailable: op_zfb_fetch is not registered in this runtime",
      );
    }

    const spec = {
      url: request.url,
      method,
      headers,
      // Anything the caller did not spell as a Fetch redirect mode
      // falls back to the standard's default rather than reaching Rust
      // as an unparseable value.
      redirect:
        request.redirect === "manual" || request.redirect === "error" ? request.redirect : "follow",
      hasBody,
    };
    const signalTimeoutMs = signal ? signal._timeoutMs : null;
    if (signalTimeoutMs != null) {
      // Clamped to a whole number in `u32` range: the op decodes this
      // field as a `u32` (a `u64` would require a BigInt on this side),
      // and since the value can only NARROW the host's own 30s deadline,
      // clamping the top end costs nothing.
      spec.timeoutMs = Math.min(Math.floor(signalTimeoutMs), 4294967295);
    }

    let outcome;
    try {
      const dispatched = op(spec, bodyBytes);
      outcome = signal ? await raceWithAbort(dispatched, signal) : await dispatched;
    } catch (e) {
      // A deadline the CALLER asked for is an abort, not a host limit,
      // so it reports itself the way the standard says rather than
      // quoting the host's own timeout diagnostic.
      //
      // Only when the caller's deadline is the one that actually fired,
      // though: the transport applies `min(host, caller)`, so a signal
      // LONGER than the host ceiling means the host's deadline won.
      // Relabelling that one would discard the URL, the effective
      // deadline and the production-divergence note, and tell the
      // developer their own 60s timeout fired at 30s.
      const callerDeadlineWon = signalTimeoutMs != null && signalTimeoutMs <= limits.fetchTimeoutMs;
      if (callerDeadlineWon && e && e.name === "TimeoutError") {
        throw timeoutError();
      }
      throw e;
    }

    const response = new Response(outcome.body, {
      status: outcome.status,
      statusText: outcome.statusText,
      // An ordered `[name, value]` array, never an object: repeated
      // `set-cookie` values must survive the boundary, and an object
      // would collapse them to the last one.
      headers: outcome.headers,
      // The FINAL url, after any redirect the transport followed.
      url: outcome.url,
    });
    response.redirected = outcome.redirected === true;
    return response;
  }

  // ---- atob / btoa ----------------------------------------------
  //
  // Standard base64. Only ASCII inputs to btoa, only ASCII outputs
  // from atob — matches the WHATWG spec.
  const B64_CHARS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  function btoa(input) {
    const str = String(input);
    let out = "";
    let i = 0;
    while (i < str.length) {
      const c1 = str.charCodeAt(i++);
      const c2 = i < str.length ? str.charCodeAt(i++) : NaN;
      const c3 = i < str.length ? str.charCodeAt(i++) : NaN;
      if (c1 > 0xff || (i > 1 && c2 > 0xff) || (i > 2 && c3 > 0xff)) {
        throw new Error("btoa: input contains characters outside Latin-1");
      }
      const b1 = c1 >> 2;
      const b2 = ((c1 & 0x3) << 4) | (isNaN(c2) ? 0 : c2 >> 4);
      const b3 = isNaN(c2) ? 64 : ((c2 & 0xf) << 2) | (isNaN(c3) ? 0 : c3 >> 6);
      const b4 = isNaN(c3) ? 64 : c3 & 0x3f;
      out +=
        B64_CHARS[b1] +
        B64_CHARS[b2] +
        (b3 === 64 ? "=" : B64_CHARS[b3]) +
        (b4 === 64 ? "=" : B64_CHARS[b4]);
    }
    return out;
  }
  function atob(input) {
    const str = String(input).replace(/[^A-Za-z0-9+/=]/g, "");
    let out = "";
    let i = 0;
    while (i < str.length) {
      const b1 = B64_CHARS.indexOf(str.charAt(i++));
      const b2 = B64_CHARS.indexOf(str.charAt(i++));
      const b3 = B64_CHARS.indexOf(str.charAt(i++));
      const b4 = B64_CHARS.indexOf(str.charAt(i++));
      const c1 = (b1 << 2) | (b2 >> 4);
      const c2 = ((b2 & 0xf) << 4) | (b3 >> 2);
      const c3 = ((b3 & 0x3) << 6) | b4;
      out += String.fromCharCode(c1);
      if (str.charAt(i - 2) !== "=") out += String.fromCharCode(c2);
      if (str.charAt(i - 1) !== "=") out += String.fromCharCode(c3);
    }
    return out;
  }

  // ---- structuredClone ------------------------------------------
  //
  // Shallow JSON-safe deep clone. Real `structuredClone` handles
  // Map / Set / Date / typed arrays and circular references; this
  // version covers JSON-shaped data plus typed arrays. If user
  // bundles surface a regression, expand here.
  function structuredClone(input) {
    if (input === null || typeof input !== "object") return input;
    if (input instanceof Uint8Array) {
      return new Uint8Array(input);
    }
    if (input instanceof ArrayBuffer) {
      return input.slice(0);
    }
    if (input instanceof Date) {
      return new Date(input.getTime());
    }
    if (Array.isArray(input)) {
      return input.map((v) => structuredClone(v));
    }
    const out = {};
    for (const k of Object.keys(input)) {
      out[k] = structuredClone(input[k]);
    }
    return out;
  }

  // ---- crypto ---------------------------------------------------
  //
  // Backed by the OS CSPRNG through `op_zfb_random_bytes` (issue #2017)
  // and by `op_zfb_digest` for SHA hashing (issue #2018). Both ops are
  // synchronous and available in BOTH dispatch modes: unlike `fetch`,
  // neither entropy nor hashing is mode-gated — the SSG denial is about
  // *network access*, and randomness whose quality depended on which
  // pipeline rendered the page would be its own footgun.
  //
  // Until #2018 this section derived `randomUUID` and `getRandomValues`
  // from `Math.random`. That is bug #1751: a session ID, CSRF token or
  // nonce minted during local SSR was PREDICTABLE while the identical
  // code on production Workers was not — and nothing about it looked
  // broken, because the bytes still arrived and every functional test
  // still passed. There is deliberately **no fallback path** anywhere
  // below: if the host op is missing or the kernel CSPRNG errors, every
  // entropy-consuming call THROWS. Weak bytes must never reach a
  // caller.
  //
  // The unimplemented SubtleCrypto surface is PRESENT and throwing
  // rather than absent, for the same reason: `typeof crypto.subtle.sign
  // === "function"` is how libraries feature-detect, and silent absence
  // would make them take a local fallback branch production never
  // takes. See `research/2013-request-time-capability-contract.md`
  // divergences D7 (no MD5) and D8 (key-bearing methods).

  // Exactly Cloudflare's documented eligible-view list for
  // `crypto.getRandomValues`. Floats and `DataView` are excluded by the
  // WebCrypto spec itself (integer-typed views only).
  const RANDOM_VALUES_ELIGIBLE_VIEW_NAMES = [
    "Int8Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "Int16Array",
    "Uint16Array",
    "Int32Array",
    "Uint32Array",
    "BigInt64Array",
    "BigUint64Array",
  ];

  // The SubtleCrypto methods production Workers implements and this
  // host does not (contract divergence D8). Present as callable
  // methods; every one rejects.
  const SUBTLE_UNIMPLEMENTED_METHODS = [
    "encrypt",
    "decrypt",
    "sign",
    "verify",
    "generateKey",
    "deriveKey",
    "deriveBits",
    "importKey",
    "exportKey",
    "wrapKey",
    "unwrapKey",
  ];

  // `Error` with a spec-correct `.name`. No real `DOMException` exists
  // in this host (contract divergence D4), so `err.name` checks — the
  // common idiom — behave as they would against one, while
  // `instanceof DOMException` does not.
  function namedError(name, message) {
    const err = new Error(message);
    err.name = name;
    return err;
  }

  function describeCtor(value) {
    if (value === null || value === undefined) return String(value);
    const ctor = value.constructor;
    if (ctor && typeof ctor.name === "string" && ctor.name) return ctor.name;
    return typeof value;
  }

  function isEligibleRandomValuesView(view) {
    if (!ArrayBuffer.isView(view)) return false;
    for (const name of RANDOM_VALUES_ELIGIBLE_VIEW_NAMES) {
      const ctor = globalThis[name];
      if (typeof ctor === "function" && view instanceof ctor) return true;
    }
    return false;
  }

  // The per-call byte quota, read out of the Rust-injected
  // `__zfb.limits` (issue #2016). A second hardcoded copy of the number
  // in JS is a REJECTED design — it drifts from `limits.rs` silently
  // while every test still passes.
  //
  // `null` means the host published no limits at all, which happens
  // only outside a booted host (these polyfills execute before the
  // shim). That is not a hole: the identical ceiling is enforced in
  // Rust, on the same byte count, where bundle code cannot reach it.
  function randomBytesQuota() {
    const bridge = globalThis.__zfb;
    const limits = bridge ? bridge.limits : undefined;
    const quota = limits ? limits.maxRandomBytesPerCall : undefined;
    return typeof quota === "number" ? quota : null;
  }

  function hostOp(name) {
    const ops = globalThis.Deno && globalThis.Deno.core ? globalThis.Deno.core.ops : undefined;
    const op = ops ? ops[name] : undefined;
    return typeof op === "function" ? op : null;
  }

  // Fill `u8` from the OS CSPRNG.
  //
  // FAIL CLOSED: every failure throws. There is no fallback source, no
  // zero fill, and no retry against a weaker generator — that is the
  // whole of bug #1751. `apiName` is the calling API, because the one
  // op backs both `getRandomValues` and `randomUUID` and the contract
  // gives each its own message prefix.
  function fillFromHostEntropy(apiName, u8) {
    const op = hostOp("op_zfb_random_bytes");
    if (!op) {
      throw new Error(
        apiName + ": OS entropy unavailable: op_zfb_random_bytes is not registered in this runtime",
      );
    }
    try {
      op(u8);
    } catch (e) {
      // The Rust quota rejection already carries its own
      // `crypto.getRandomValues:` prefix and `QuotaExceededError` name;
      // re-prefixing it would produce a doubled method name.
      if (e && e.name === "QuotaExceededError") throw e;
      throw new Error(apiName + ": " + (e && e.message ? e.message : String(e)));
    }
  }

  // A `Uint8Array` view over whatever bytes `value` holds, without
  // copying. Accepts the WebCrypto `BufferSource` union.
  function bufferSourceBytes(apiName, value) {
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    if (ArrayBuffer.isView(value)) {
      return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    throw new TypeError(
      apiName + ": expected an ArrayBuffer or ArrayBufferView, got " + describeCtor(value),
    );
  }

  function subtleNotSupported(method) {
    // Deliberately states that production SUCCEEDS here and names the
    // contract document. Local diagnostics CANNOT match production for
    // these calls (divergence D8) — production returns a key or a
    // signature — so the honest alignment is to say so, rather than
    // synthesise a workerd-shaped error workerd would never have
    // raised.
    return namedError(
      "NotSupportedError",
      "crypto.subtle." +
        method +
        " is not implemented in the zfb embedded runtime. Production Cloudflare Workers DOES " +
        "implement this call — see research/2013-request-time-capability-contract.md. This host " +
        "implements digest (SHA-1/256/384/512) and timingSafeEqual only.",
    );
  }

  // workerd exposes `DigestStream` as a constructor. It is present here
  // and throws on construction rather than being absent, for the same
  // feature-detection reason as the methods above.
  function DigestStream() {
    throw subtleNotSupported("DigestStream");
  }

  const subtle = {
    // Returns `Promise<ArrayBuffer>` as WebCrypto requires, even though
    // the underlying op is synchronous (hashing is CPU-bound over a
    // buffer already in memory — there is no socket to await).
    // Algorithm validation lives in Rust so the supported set and the
    // message advertising it cannot drift.
    digest(algorithm, data) {
      try {
        const name =
          typeof algorithm === "string" ? algorithm : String(algorithm && algorithm.name);
        const bytes = bufferSourceBytes("crypto.subtle.digest", data);
        const op = hostOp("op_zfb_digest");
        if (!op) {
          throw new Error(
            "crypto.subtle.digest: embedded host transport unavailable: op_zfb_digest is not " +
              "registered in this runtime",
          );
        }
        const out = op(name, bytes);
        // Detach a standalone `ArrayBuffer`: the op's result may be a
        // view over a larger backing store, and handing back
        // `out.buffer` would leak whatever else lives in it.
        return Promise.resolve(out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength));
      } catch (e) {
        return Promise.reject(e);
      }
    },
    // A workerd extension, not WebCrypto: synchronous, returns a
    // boolean. Constant-time in the length of the inputs — no early
    // exit, and the whole buffer is always read, so the comparison's
    // duration carries no information about WHERE the first difference
    // is. The length check is deliberately NOT constant-time: byte
    // lengths are not secret, and the spec throws on a mismatch.
    timingSafeEqual(a, b) {
      const left = bufferSourceBytes("crypto.subtle.timingSafeEqual", a);
      const right = bufferSourceBytes("crypto.subtle.timingSafeEqual", b);
      if (left.byteLength !== right.byteLength) {
        throw new TypeError("crypto.subtle.timingSafeEqual: buffers must be the same byteLength");
      }
      let diff = 0;
      for (let i = 0; i < left.length; i++) {
        diff |= left[i] ^ right[i];
      }
      return diff === 0;
    },
    DigestStream,
  };

  for (const method of SUBTLE_UNIMPLEMENTED_METHODS) {
    // Rejected promises, not throws: every one of these is async in
    // WebCrypto, so a caller's `.catch()` must be what sees the
    // failure.
    subtle[method] = () => Promise.reject(subtleNotSupported(method));
  }

  const crypto = {
    getRandomValues(view) {
      if (!isEligibleRandomValuesView(view)) {
        throw namedError(
          "TypeMismatchError",
          "crypto.getRandomValues: " +
            describeCtor(view) +
            " is not an integer-typed ArrayBufferView",
        );
      }
      const quota = randomBytesQuota();
      // Measured on `byteLength`, NEVER on element count: a
      // `Uint32Array(20000)` is 20,000 elements but 80,000 bytes, and
      // an element-count reading would wave it through.
      if (quota !== null && view.byteLength > quota) {
        throw namedError(
          "QuotaExceededError",
          "crypto.getRandomValues: requested " +
            view.byteLength +
            " bytes, quota is " +
            quota +
            " bytes",
        );
      }
      // A zero-length view is a valid no-op that returns the view, and
      // must succeed even on a host whose CSPRNG is unavailable — zero
      // bytes cannot be weak.
      if (view.byteLength === 0) return view;
      // A byte view over the SAME backing store, so the op writes
      // through to `view` whatever its element type is (the op takes a
      // `&mut [u8]`).
      fillFromHostEntropy(
        "crypto.getRandomValues",
        new Uint8Array(view.buffer, view.byteOffset, view.byteLength),
      );
      return view;
    },
    randomUUID() {
      const bytes = new Uint8Array(16);
      fillFromHostEntropy("crypto.randomUUID", bytes);
      // RFC 4122 §4.4: version 4 in the high nibble of octet 6, variant
      // 10xx in the top two bits of octet 8. Set on the BYTES rather
      // than spelled into the output string, so the invariants hold for
      // anyone who parses the UUID back to bytes.
      bytes[6] = (bytes[6] & 0x0f) | 0x40;
      bytes[8] = (bytes[8] & 0x3f) | 0x80;
      let hex = "";
      for (let i = 0; i < bytes.length; i++) {
        hex += bytes[i].toString(16).padStart(2, "0");
      }
      return (
        hex.slice(0, 8) +
        "-" +
        hex.slice(8, 12) +
        "-" +
        hex.slice(12, 16) +
        "-" +
        hex.slice(16, 20) +
        "-" +
        hex.slice(20, 32)
      );
    },
    subtle,
    // workerd's own location for the constructor. Exposed here too so
    // `typeof crypto.DigestStream === "function"` answers the same
    // locally as in production — the absence, not the throw, is what
    // would send feature detection down a branch production never
    // takes.
    DigestStream,
  };
  // `CryptoKey` is deliberately left undefined: nothing in the
  // implemented set produces or consumes one, so referencing it is a
  // plain `ReferenceError`.

  // ---- setTimeout / clearTimeout --------------------------------
  //
  // The embedded V8 host has NO event loop timers (deno_core's
  // timer ops are not wired in). React 19's react-dom server bundle
  // references `setTimeout` in `handleErrorInNextTick` — the path that
  // re-throws an SSR error on a fresh tick. Without `setTimeout`, a
  // component that throws during render would surface
  // `ReferenceError: setTimeout is not defined` and MASK the real
  // error. We back it with `queueMicrotask` (a pure-V8/host builtin —
  // confirmed present), which the host's microtask checkpoint drains
  // synchronously after the current turn.
  //
  // There are NO real delay semantics: the `delay` argument is
  // ignored and the callback runs on the next microtask, not after
  // `delay` ms. The SSG render path never depends on timer ordering
  // (it completes synchronously); the only caller is React's
  // error-retick. `clearTimeout` cannot cancel an already-scheduled
  // microtask, so it is a documented no-op — nothing on the SSG path
  // relies on cancellation.
  let __zfbTimerId = 1;
  function setTimeout(callback, _delay, ...args) {
    const id = __zfbTimerId++;
    if (typeof callback === "function") {
      queueMicrotask(function () {
        callback.apply(undefined, args);
      });
    }
    return id;
  }
  function clearTimeout(_id) {
    // No-op: a microtask-backed timer cannot be cancelled. Nothing on
    // the SSG path requires cancellation (see setTimeout docblock).
  }

  // ---- MessageChannel / MessagePort -----------------------------
  //
  // React 19's `react-dom-server.browser.production.js` constructs a
  // `MessageChannel` at MODULE-LOAD time (unguarded:
  // `var channel = new MessageChannel()`), using `channel.port1.onmessage`
  // + `channel.port2.postMessage(null)` to schedule async work (the
  // Fizz streaming `scheduleWork` path). React 18 did not — this is
  // the next.16 / React 19 gap. The host has no DOM MessageChannel, so
  // the module-load `new MessageChannel()` throws
  // `ReferenceError: MessageChannel is not defined` before any render.
  //
  // Minimal Promise/microtask-backed implementation: a `postMessage`
  // on one port schedules the *paired* port's `onmessage` (and any
  // `addEventListener("message")` listeners) on a microtask via
  // `queueMicrotask`. `Promise` and `queueMicrotask` are host builtins
  // and the host drives the microtask checkpoint, so this is enough to
  // satisfy React's scheduler whether or not it actually flushes during
  // a synchronous `renderToString`. The legacy `renderToString` path
  // (the one the SSG renderer uses) renders in one sync pass and never
  // posts on this channel — the channel only needs to be *constructible*
  // at module load; it is never flushed during build-time SSR. The
  // Promise-backed delivery below is there for completeness in case a
  // streaming path ever exercises it.
  class MessagePort {
    constructor() {
      this.onmessage = null;
      this._listeners = [];
      this._other = null;
    }
    postMessage(data) {
      const target = this._other;
      if (target == null) return;
      const evt = { data: data, target: target };
      queueMicrotask(function () {
        if (typeof target.onmessage === "function") {
          target.onmessage(evt);
        }
        for (const listener of target._listeners.slice()) {
          if (typeof listener === "function") {
            listener(evt);
          } else if (listener && typeof listener.handleEvent === "function") {
            listener.handleEvent(evt);
          }
        }
      });
    }
    addEventListener(type, listener) {
      if (type === "message" && listener != null) {
        this._listeners.push(listener);
      }
    }
    removeEventListener(type, listener) {
      if (type === "message") {
        this._listeners = this._listeners.filter((l) => l !== listener);
      }
    }
    // `start()` / `close()` are part of the MessagePort interface but
    // React never calls them (it assigns `onmessage` directly, which
    // implicitly starts the port). No-ops keep the surface complete.
    start() {}
    close() {}
  }

  class MessageChannel {
    constructor() {
      this.port1 = new MessagePort();
      this.port2 = new MessagePort();
      this.port1._other = this.port2;
      this.port2._other = this.port1;
    }
  }

  // ---- Install --------------------------------------------------
  // Order matters: TextEncoder/TextDecoder are used by Request body
  // construction, so install them first.
  globalThis.TextEncoder = TextEncoder;
  globalThis.TextDecoder = TextDecoder;
  globalThis.URL = URL;
  globalThis.URLSearchParams = URLSearchParams;
  globalThis.Headers = Headers;
  globalThis.Request = Request;
  globalThis.Response = Response;
  globalThis.fetch = fetch;
  registerHostErrorClasses();
  globalThis.AbortController = AbortController;
  globalThis.AbortSignal = AbortSignal;
  globalThis.atob = atob;
  globalThis.btoa = btoa;
  globalThis.structuredClone = structuredClone;
  globalThis.crypto = crypto;
  // Timer + scheduling shims for the React 19 server bundle. Installed
  // only if absent so a host that later wires real timer ops wins.
  if (typeof globalThis.setTimeout !== "function") {
    globalThis.setTimeout = setTimeout;
    globalThis.clearTimeout = clearTimeout;
  }
  if (typeof globalThis.MessageChannel !== "function") {
    globalThis.MessageChannel = MessageChannel;
    globalThis.MessagePort = MessagePort;
  }
})(globalThis);
