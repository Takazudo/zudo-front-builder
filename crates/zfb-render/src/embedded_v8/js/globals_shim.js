// Host-bridge globals for the embedded V8 host.
//
// Installs `globalThis.__zfb` with three helpers the Rust side calls
// via `execute_script`:
//
// - `__zfb.setBundle(defaultExport)` — called once after the bundle
//   evaluates; stashes the bundle's `default` export so subsequent
//   `dispatch` calls can reach `default.fetch`.
//
// - `__zfb.dispatch(urlStr, method, headersObj, bodyU8, mode)` — builds
//   a `Request`, awaits `default.fetch(request)`, awaits the response
//   body as a `Uint8Array`, and returns a Promise resolving to
//   `{ status, headers, body }`. The Rust side drives the event
//   loop until the Promise resolves, then unpacks the fields via v8
//   property reads.
//
//   `mode` is the per-dispatch build-time/request-time signal (issue
//   #2014): `"build-time"` or `"request-time"`. `dispatch` publishes it
//   as `__zfb.mode` for the duration of the dispatch and restores the
//   previous value in a `finally`, so a THROWING request-time dispatch
//   cannot leak request-time capability into the next build-time
//   render. `web_polyfills.js` reads `__zfb.mode`; when it is absent
//   (module evaluation, or any caller that never passed one) the
//   readers fall back to build-time, which is the denying default.
//
// - `__zfb.drainConsoleLogs()` — returns the worker console output
//   buffered by the console capture below (joined with `\n`, each
//   line prefixed with its level) and clears the buffer. Called by
//   the Rust side AFTER a render completes/fails — never
//   mid-dispatch — so failed renders can surface what the worker
//   printed (issue #700).
//
// All helpers are pure JS; the host does not register any Rust ops
// for them. That keeps the deno_core op-version coupling out of this
// crate's surface.

const __zfb_state = {
  bundle: null,
};

// Per-dispatch mode (issue #2014). `undefined` outside any dispatch —
// module evaluation, boot scripts — which the polyfill readers treat as
// build-time, the denying default.
//
// Kept in a module-scope cell rather than as a plain data property on
// `__zfb` so `__zfb.mode` can be exposed as a getter with no setter:
// bundle code cannot elevate itself to request-time by assigning to it.
const __zfb_mode_state = {
  current: undefined,
};

// Console capture (issue #700).
//
// The bare deno_core runtime installs a minimal `console` whose
// levelled methods print straight to the zfb process's stdout via
// `op_print`. That live passthrough is preserved (each patched method
// forwards to the original binding), but every line is ALSO buffered
// here so the Rust side can drain it and attach the worker's console
// output to render-failure diagnostics — without the buffer, a failed
// render surfaces only "Internal Server Error" with no context.
//
// Caps bound memory for noisy workers across a full render pass: once
// either cap is hit, further lines are dropped and a single
// truncation marker is recorded. Draining resets the caps.
const __zfb_console_state = {
  lines: [],
  totalChars: 0,
  truncated: false,
};
const __ZFB_CONSOLE_MAX_LINES = 1000;
const __ZFB_CONSOLE_MAX_TOTAL_CHARS = 262144; // 256 KiB of UTF-16 units
const __ZFB_CONSOLE_MAX_LINE_CHARS = 8192;

function __zfb_consoleStringifyArg(arg) {
  if (typeof arg === "string") {
    return arg;
  }
  if (
    arg === null ||
    arg === undefined ||
    typeof arg === "number" ||
    typeof arg === "boolean" ||
    typeof arg === "bigint" ||
    typeof arg === "symbol" ||
    typeof arg === "function"
  ) {
    return String(arg);
  }
  if (arg instanceof Error) {
    return arg.stack || String(arg);
  }
  try {
    const json = JSON.stringify(arg);
    if (json !== undefined) {
      return json;
    }
  } catch (_) {
    // Circular structure etc. — fall through to String().
  }
  try {
    return String(arg);
  } catch (_) {
    return "[unstringifiable value]";
  }
}

function __zfb_captureConsoleLine(level, args) {
  const st = __zfb_console_state;
  if (
    st.lines.length >= __ZFB_CONSOLE_MAX_LINES ||
    st.totalChars >= __ZFB_CONSOLE_MAX_TOTAL_CHARS
  ) {
    if (!st.truncated) {
      st.truncated = true;
      st.lines.push("[zfb] (console capture truncated: buffer cap reached)");
    }
    return;
  }
  let line;
  try {
    line = `[${level}] ` + args.map(__zfb_consoleStringifyArg).join(" ");
  } catch (_) {
    line = `[${level}] [unstringifiable arguments]`;
  }
  if (line.length > __ZFB_CONSOLE_MAX_LINE_CHARS) {
    line = line.slice(0, __ZFB_CONSOLE_MAX_LINE_CHARS) + " …(line truncated)";
  }
  st.lines.push(line);
  st.totalChars += line.length;
}

// Patch the five levelled console methods in place (rather than
// replacing the console object) so any extra members the runtime
// installed — e.g. inspector-only APIs copied from V8's console —
// survive untouched. Originals are snapshotted BEFORE patching so a
// missing level's fallback (`console.info` does not exist on
// deno_core's CoreConsole) forwards to the ORIGINAL `log`, not the
// patched one (which would double-capture).
(() => {
  if (!globalThis.console || typeof globalThis.console !== "object") {
    globalThis.console = {};
  }
  const c = globalThis.console;
  const levels = ["log", "info", "warn", "error", "debug"];
  const originals = {};
  for (const level of levels) {
    originals[level] = typeof c[level] === "function" ? c[level].bind(c) : null;
  }
  for (const level of levels) {
    const forward = originals[level] || originals.log;
    c[level] = (...args) => {
      __zfb_captureConsoleLine(level, args);
      if (forward) {
        try {
          forward(...args);
        } catch (_) {
          // Passthrough must never throw into worker code.
        }
      }
    };
  }
})();

globalThis.__zfb = {
  // ssrDebug: present only in the embedded build/dev V8 host, never in the
  // production Cloudflare Workers runtime — gates verbose SSR error output.
  ssrDebug: true,
  // Read-only view of the active dispatch's mode: "build-time",
  // "request-time", or undefined outside any dispatch. Getter-only on
  // purpose — see `__zfb_mode_state`.
  get mode() {
    return __zfb_mode_state.current;
  },
  setBundle(defaultExport) {
    __zfb_state.bundle = defaultExport;
  },
  drainConsoleLogs() {
    const st = __zfb_console_state;
    if (st.lines.length === 0) {
      return "";
    }
    const out = st.lines.join("\n");
    st.lines = [];
    st.totalChars = 0;
    st.truncated = false;
    return out;
  },
  async dispatch(urlStr, method, headersObj, bodyU8, mode) {
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
    // Publish the per-dispatch mode, remembering whatever was there
    // before (normally `undefined`) so the `finally` below restores it
    // EXACTLY — including on the throwing path. An unrestored
    // "request-time" would silently grant request-time capability to
    // every subsequent build-time render on this shared host.
    const previousMode = __zfb_mode_state.current;
    __zfb_mode_state.current = mode === undefined || mode === null ? undefined : String(mode);
    try {
      const req = new Request(urlStr, init);
      const resp = await __zfb_state.bundle.fetch(req);
      // Materialise the body up front so the host doesn't have to
      // poke at ReadableStream.
      const buf = await resp.arrayBuffer();
      // Ordered `[name, value]` pairs, not a `Record` — `resp.headers.entries()`
      // already applies the Fetch "sort and combine" view (duplicate
      // `set-cookie` values kept as separate entries, everything else
      // comma-joined), so a single-valued object here would silently
      // collapse repeated headers back down to one.
      const headers = [];
      for (const [k, v] of resp.headers.entries()) {
        // Headers iteration already lowercases keys per spec, but
        // be defensive.
        headers.push([k.toLowerCase(), v]);
      }
      return {
        status: resp.status,
        headers,
        body: new Uint8Array(buf),
      };
    } finally {
      __zfb_mode_state.current = previousMode;
    }
  },
};
