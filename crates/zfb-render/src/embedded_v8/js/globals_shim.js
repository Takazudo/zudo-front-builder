// Host-bridge globals for the embedded V8 host.
//
// Installs `globalThis.__zfb` with two helpers the Rust side calls
// via `execute_script`:
//
// - `__zfb.setBundle(defaultExport)` — called once after the bundle
//   evaluates; stashes the bundle's `default` export so subsequent
//   `dispatch` calls can reach `default.fetch`.
//
// - `__zfb.dispatch(urlStr, method, headersObj, bodyU8)` — builds a
//   `Request`, awaits `default.fetch(request)`, awaits the response
//   body as a `Uint8Array`, and returns a Promise resolving to
//   `{ status, headers, body }`. The Rust side drives the event
//   loop until the Promise resolves, then unpacks the fields via v8
//   property reads.
//
// Both helpers are pure JS; the host does not register any Rust ops
// for them. That keeps the deno_core op-version coupling out of this
// crate's surface.

const __zfb_state = {
  bundle: null,
};

globalThis.__zfb = {
  setBundle(defaultExport) {
    __zfb_state.bundle = defaultExport;
  },
  async dispatch(urlStr, method, headersObj, bodyU8) {
    if (!__zfb_state.bundle || typeof __zfb_state.bundle.fetch !== "function") {
      throw new Error("embedded V8 host: bundle has no callable `default.fetch`");
    }
    const init = { method };
    if (headersObj) {
      init.headers = headersObj;
    }
    if (bodyU8 && bodyU8.byteLength > 0) {
      init.body = bodyU8;
    }
    const req = new Request(urlStr, init);
    const resp = await __zfb_state.bundle.fetch(req);
    // Materialise the body up front so the host doesn't have to
    // poke at ReadableStream.
    const buf = await resp.arrayBuffer();
    const headers = {};
    for (const [k, v] of resp.headers.entries()) {
      // Headers iteration already lowercases keys per spec, but
      // be defensive.
      headers[k.toLowerCase()] = v;
    }
    return {
      status: resp.status,
      headers,
      body: new Uint8Array(buf),
    };
  },
};
