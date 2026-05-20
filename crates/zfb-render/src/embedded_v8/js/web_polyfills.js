// Minimal Web Platform API polyfills for the embedded V8 host.
//
// The SSG path never makes outgoing network requests, so we don't
// need a real `fetch`. Hono and similar workerd-style routers only
// need:
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
// - `crypto` with `randomUUID` and `subtle.digest` (Hono's request-id
//   middleware probes them; we provide enough for the probe to not
//   throw, but keep the surface small).
//
// All polyfills live in the global object so module-shape detection
// (`typeof Request === "function"`) reports as you'd expect on
// workerd.
//
// What we deliberately DO NOT polyfill:
//
// - Outgoing `fetch(url)` to a network — we install a `fetch` that
//   throws so user code that accidentally tries to make a network
//   call during SSG fails loudly instead of silently hanging.
// - `WebSocket`, `EventSource`, `Worker` — none are sensible in a
//   build-time renderer.
// - `ReadableStream` beyond what `Response.arrayBuffer()` needs.
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
      this._map = new Map();
      if (init == null) return;
      if (init instanceof Headers) {
        for (const [k, v] of init._map) this._map.set(k, v);
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
      const k = String(name).toLowerCase();
      const v = String(value);
      const prev = this._map.get(k);
      this._map.set(k, prev == null ? v : prev + ", " + v);
    }
    delete(name) {
      this._map.delete(String(name).toLowerCase());
    }
    get(name) {
      const v = this._map.get(String(name).toLowerCase());
      return v == null ? null : v;
    }
    has(name) {
      return this._map.has(String(name).toLowerCase());
    }
    set(name, value) {
      this._map.set(String(name).toLowerCase(), String(value));
    }
    forEach(cb, thisArg) {
      for (const [k, v] of this._map) {
        cb.call(thisArg, v, k, this);
      }
    }
    *keys() {
      for (const k of this._map.keys()) yield k;
    }
    *values() {
      for (const v of this._map.values()) yield v;
    }
    *entries() {
      for (const [k, v] of this._map) yield [k, v];
    }
    [Symbol.iterator]() {
      return this.entries();
    }
  }

  // ---- URLSearchParams ------------------------------------------
  //
  // We accept string / array / object init shapes (matching the
  // WHATWG spec). The actual parsing is done by splitting on '&'
  // / '=' — no encoding subtleties are required for the SSG path
  // (router params are typically already encoded by the framework).
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
            this._pairs.push([decodeURIComponent(piece), ""]);
          } else {
            const k = decodeURIComponent(piece.slice(0, eq));
            const v = decodeURIComponent(piece.slice(eq + 1));
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
        .map(([k, v]) => encodeURIComponent(k) + "=" + encodeURIComponent(v))
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
    async blob() {
      throw new Error("Blob is not implemented in the SSG runtime");
    }
    async formData() {
      throw new Error("FormData is not implemented in the SSG runtime");
    }
  }

  // ---- Request --------------------------------------------------
  class Request extends _Body {
    constructor(input, init) {
      let url;
      let baseInit = {};
      if (input instanceof Request) {
        url = input.url;
        baseInit = {
          method: input.method,
          headers: input.headers,
          body: input._bodyBytes,
        };
      } else {
        url = String(input);
      }
      const finalInit = Object.assign({}, baseInit, init || {});
      super(finalInit.body);
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
        body: this._bodyBytes,
      });
    }
  }

  // ---- Response -------------------------------------------------
  class Response extends _Body {
    constructor(body, init) {
      super(body);
      const i = init || {};
      this.status = i.status == null ? 200 : Number(i.status);
      this.statusText = i.statusText || statusText(this.status);
      this.ok = this.status >= 200 && this.status < 300;
      this.headers = i.headers instanceof Headers ? i.headers : new Headers(i.headers);
      this.type = "default";
      this.url = i.url || "";
      this.redirected = false;
    }
    clone() {
      return new Response(this._bodyBytes, {
        status: this.status,
        statusText: this.statusText,
        headers: this.headers,
      });
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
      const r = new Response(body, init);
      if (!r.headers.has("content-type")) {
        r.headers.set("content-type", "application/json");
      }
      return r;
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

  // ---- fetch ----------------------------------------------------
  //
  // SSG never makes outgoing requests. If a bundle calls `fetch(url)`
  // we throw with a message that names the offending URL so the
  // operator can find the call site.
  function fetch(input, _init) {
    const url = input instanceof Request ? input.url : String(input);
    return Promise.reject(
      new Error(
        "fetch() called from SSG runtime (url=" +
          url +
          "). The embedded V8 host does not support outgoing network requests during build-time render. Move the data fetch to a build step or a runtime-only branch.",
      ),
    );
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
  // `crypto.randomUUID()` is the only thing zfb-runtime / Hono
  // request-id middleware reaches for. We provide a deterministic-
  // looking v4 UUID using `Math.random()` (good enough for SSG —
  // the IDs are not cryptographically meaningful at build time,
  // and using a real CSPRNG would require a Rust op).
  const crypto = {
    randomUUID() {
      const hex = "0123456789abcdef";
      let s = "";
      for (let i = 0; i < 36; i++) {
        if (i === 8 || i === 13 || i === 18 || i === 23) {
          s += "-";
        } else if (i === 14) {
          s += "4";
        } else if (i === 19) {
          s += hex[(Math.random() * 4) | 8];
        } else {
          s += hex[(Math.random() * 16) | 0];
        }
      }
      return s;
    },
    getRandomValues(typedArray) {
      for (let i = 0; i < typedArray.length; i++) {
        typedArray[i] = (Math.random() * 256) | 0;
      }
      return typedArray;
    },
    subtle: {
      // Stubs that throw — Hono's request-id middleware probes
      // these but doesn't call them on the SSG path. If a real
      // bundle calls `subtle.digest(...)`, fix it here with a
      // pure-JS sha256 implementation (~80 lines).
      digest() {
        return Promise.reject(
          new Error("crypto.subtle.digest is not implemented in the SSG runtime"),
        );
      },
    },
  };

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
  globalThis.atob = atob;
  globalThis.btoa = btoa;
  globalThis.structuredClone = structuredClone;
  globalThis.crypto = crypto;
})(globalThis);
