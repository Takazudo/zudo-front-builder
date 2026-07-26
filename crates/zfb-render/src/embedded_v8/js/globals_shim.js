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
// - `__zfb.limits` — the request-time numeric limits, injected from
//   `embedded_v8/limits.rs` at host boot (issue #2016). Getter-only.
//   `web_polyfills.js`'s request-time `fetch` reads its request-body
//   cap from here rather than hardcoding a copy of the Rust constant.
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

// ## Why the body is wrapped in an IIFE
//
// Everything below is closure-private. A bare top-level `const` in a
// classic script lands in the GLOBAL LEXICAL environment, where the
// evaluated bundle can read it by name — which would expose
// `__ZFB_MODE_NONCE` (see below) and let bundle code forge a
// request-time dispatch. Nothing outside this file references these
// names, so the wrap costs nothing.
(function installHostGlobals() {
  // Per-host mode nonce (issue #2014). The Rust side substitutes a fresh
  // value for this placeholder at host boot; `dispatch` honours its `mode`
  // argument ONLY when the caller presents the matching nonce.
  //
  // Without it, `globalThis.__zfb.dispatch` — necessarily reachable from
  // the evaluated bundle — would let a build-time handler re-enter with
  // `"request-time"` and hand its nested handler the request-time branch.
  // A forged call does not fail: it INHERITS the enclosing dispatch's
  // mode, so it can never widen capability, only run at the mode it was
  // already entitled to.
  //
  // This is a guard against selecting the mode, not a sandbox: the bundle
  // is first-party code zfb itself compiled, and no JS-visible bridge can
  // be made proof against a hostile module in the same realm.
  const __ZFB_MODE_NONCE = "__ZFB_MODE_NONCE_PLACEHOLDER__";

  // Request-time numeric limits (issue #2016). The Rust side
  // substitutes a JSON object literal rendered by
  // `embedded_v8::limits::limits_js_literal()` for this placeholder at
  // host boot, so `web_polyfills.js` can read the caps out of Rust
  // instead of carrying a second, drift-prone copy of them — a
  // hardcoded duplicate in JS is a rejected design (contract:
  // "Numeric constants — one source of truth").
  //
  // FROZEN (epic #2012 review fix 5). An unfrozen object let bundle
  // code RAISE `maxRequestBodyBytes`, after which the JS pre-check
  // waved an oversized payload straight into the op — precisely the
  // allocation that check exists to prevent. The array inside it is
  // frozen too, since freezing the container leaves its members
  // mutable. Tests that need a different cap boot the host with one
  // (`EmbeddedV8RenderHost::with_limits_override`) rather than editing
  // it from JS.
  const __ZFB_LIMITS = Object.freeze(__ZFB_LIMITS_PLACEHOLDER__);
  if (Array.isArray(__ZFB_LIMITS.allowedFetchSchemes)) {
    Object.freeze(__ZFB_LIMITS.allowedFetchSchemes);
  }

  // Constant-time string comparison for the nonce check below.
  //
  // `===` on strings short-circuits at the first differing character,
  // which leaks a prefix oracle. The nonce is 256 CSPRNG bits so the
  // search is hopeless either way, but a timing-safe compare costs one
  // loop and removes the question.
  function __zfb_constantTimeEquals(a, b) {
    if (typeof a !== "string" || typeof b !== "string") return false;
    // Length is not secret — it is a fixed property of the format.
    if (a.length !== b.length) return false;
    let diff = 0;
    for (let i = 0; i < a.length; i++) {
      diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
    }
    return diff === 0;
  }

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

  // Host-authorised dispatches that have STARTED and not yet settled,
  // innermost last (epic #2012 review fix 3).
  //
  // The old code kept a single `previousMode` per call and restored it
  // in a `finally`, which mis-tracked the mode in two ways:
  //
  //  - an UNAUTHORISED nested call (all bundle code can make) captured
  //    the enclosing dispatch's mode and restored it in ITS `finally`,
  //    which — for a floating promise — runs after the enclosing
  //    dispatch has already ended, republishing "request-time" outside
  //    any dispatch. Unauthorised calls now do not touch the mode at
  //    all: they neither push a frame nor restore one.
  //  - anything a request-time handler started but did not await saw
  //    the mode fall back to `undefined` the moment its dispatch
  //    settled, so a `fetch` from that continuation reported itself as
  //    an SSG policy denial AT REQUEST TIME — the exact misdiagnosis
  //    this epic exists to remove. When the last frame pops, the mode
  //    therefore stays at that dispatch's own value rather than
  //    reverting; the next dispatch overwrites it unconditionally, and
  //    the host resets it to build-time before evaluating any module,
  //    so nothing is widened by leaving it standing.
  const __zfb_dispatch_frames = [];

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
    // Read-only view of the Rust-side request-time limit constants
    // (issue #2016). Getter-only for the same reason `mode` is.
    get limits() {
      return __ZFB_LIMITS;
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
    async dispatch(urlStr, method, headersObj, bodyU8, mode, nonce) {
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
      // Only the host (which knows the nonce) may CHOOSE the mode. A
      // call from bundle code neither selects nor restores it — it
      // simply runs at whatever mode the dispatch it is nested inside
      // already established, so it can never widen capability, and it
      // can never republish a stale one either (see
      // `__zfb_dispatch_frames`).
      const hostAuthorised = __zfb_constantTimeEquals(nonce, __ZFB_MODE_NONCE);
      let frame = null;
      if (hostAuthorised) {
        frame = { mode: mode === undefined || mode === null ? undefined : String(mode) };
        __zfb_dispatch_frames.push(frame);
        __zfb_mode_state.current = frame.mode;
      }
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
        if (frame) {
          const at = __zfb_dispatch_frames.indexOf(frame);
          if (at !== -1) {
            __zfb_dispatch_frames.splice(at, 1);
          }
          // The innermost dispatch still running owns the mode. When
          // none is left, THIS dispatch's mode stands — see
          // `__zfb_dispatch_frames` for why that is not a leak.
          __zfb_mode_state.current = __zfb_dispatch_frames.length
            ? __zfb_dispatch_frames[__zfb_dispatch_frames.length - 1].mode
            : frame.mode;
        }
      }
    },
    // Drop the ambient mode back to the denying default. The host calls
    // this before evaluating a module, because module top-level code is
    // bundle code running outside any dispatch and must not inherit the
    // capability of the render that happened to run before it.
    //
    // Nonce-guarded for symmetry with `dispatch`, and a no-op while any
    // dispatch is still on the stack — a reset there would be a
    // de-escalation the running dispatch never asked for.
    resetMode(nonce) {
      if (!__zfb_constantTimeEquals(nonce, __ZFB_MODE_NONCE)) return false;
      if (__zfb_dispatch_frames.length !== 0) return false;
      __zfb_mode_state.current = undefined;
      return true;
    },
  };

  // Hand the polyfill layer a DIRECT reader for the mode and the frozen
  // limits (epic #2012 review fix 1b).
  //
  // `globalThis.__zfb` is a writable property — the emitted bundle
  // prelude itself does `globalThis.__zfb = globalThis.__zfb ?? {}`, so
  // it cannot be locked down — and the polyfill used to re-read it on
  // every call. Replacing the object was therefore enough to make a
  // build-time render take the request-time branch. The polyfill now
  // captures these closures once, through a single-use channel
  // `web_polyfills.js` deletes as it consumes it, so no later
  // assignment to `globalThis.__zfb` can reach them.
  //
  // This is defence in depth for the DIAGNOSTIC, not the policy: the
  // policy is enforced in `op_zfb_fetch` (`embedded_v8/fetch.rs`),
  // where no JS value reaches at all.
  if (typeof globalThis.__zfbInstallHostBridge === "function") {
    globalThis.__zfbInstallHostBridge({
      mode: () => __zfb_mode_state.current,
      limits: () => __ZFB_LIMITS,
    });
  }
})();
