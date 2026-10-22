//! Direct-V8 port of `src/embedded_v8/js_crypto_tests.rs` (#4393, S6a): the
//! #2018 Web Crypto acceptance matrix — `crypto.getRandomValues`,
//! `crypto.randomUUID` and SubtleCrypto — run against the default
//! `zfb_js_host::native::NativeHost`.
//!
//! Every dispatch case runs real bundle JS through the production
//! `dispatch_fetch` seam with a real [`DispatchMode`]. The polyfill is the
//! shared `zfb_js_host::shims::WEB_POLYFILLS_SRC`, so its expected messages
//! are byte-identical to the Deno-host originals. What changed is the host
//! bridge: the native host exposes its entropy callback as
//! `globalThis.__zfbHost.randomBytes` (a writable data property) instead of
//! `Deno.core.ops.op_zfb_random_bytes`, so the JS spy and the fail-closed
//! cases wrap or remove that property. The polyfill's diagnostic still names
//! `op_zfb_random_bytes`; that wording is the shared polyfill's, not Deno's.
//!
//! ## Deterministic only — never statistical
//!
//! Nothing here asserts exact random bytes or samples a distribution. What is
//! asserted is invariants (eligibility, quota, UUID version/variant bits,
//! known-answer digests) and wiring (the entropy callback is genuinely
//! invoked; `OS_ENTROPY_CALLS` is a process-wide lower bound).

#![cfg(all(feature = "direct-v8", not(feature = "embed_v8")))]

use std::sync::atomic::Ordering;
use std::time::Duration;

use zfb_js_host::crypto::OS_ENTROPY_CALLS;
use zfb_js_host::limits::MAX_RANDOM_BYTES_PER_CALL;
use zfb_js_host::shims::WEB_POLYFILLS_SRC;
use zfb_render::{BundleModuleLoader, DispatchMode, EmbeddedV8RenderHost, HttpRequestLike};

/// Bounds each probe dispatch so a hang reports instead of stalling the binary.
const BOUND: Duration = Duration::from_secs(30);

/// Run `script` as the body of an `async` IIFE inside a bundle's `fetch`
/// handler and return the string it produced (a throw becomes
/// `UNCAUGHT:<name>:<message>`). Same seam as the Deno original's `probe`.
async fn probe(script: &str, mode: DispatchMode) -> String {
    let bundle = format!(
        r#"
        export default {{
          async fetch(request) {{
            let out;
            try {{
              out = await (async () => {{ {script} }})();
            }} catch (e) {{
              out = "UNCAUGHT:" + (e && e.name) + ":" + (e && e.message);
            }}
            return new Response(String(out), {{ status: 200 }});
          }},
        }};
        "#
    );
    let mut host = EmbeddedV8RenderHost::with_loader(BundleModuleLoader::new()).expect("host boot");
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute the probe bundle");
    let mut request = HttpRequestLike::get("http://zfb.local/");
    request.mode = mode;
    let response = tokio::time::timeout(BOUND, host.dispatch_fetch(request))
        .await
        .expect("the probe dispatch settles within 30s")
        .expect("dispatch");
    assert_eq!(response.status, 200, "the probe handler always answers 200");
    response
        .body_utf8()
        .expect("probe body is UTF-8")
        .to_string()
}

/// `describe(e)` renders `<name>|<message>`; `expectReject` awaits a call.
const DESCRIBE: &str = r#"
  const describe = (e) => String(e && e.name) + "|" + String(e && e.message);
  const expectReject = async (fn) => {
    try {
      const r = await fn();
      return "RESOLVED|" + String(r && r.status);
    } catch (e) {
      return describe(e);
    }
  };
"#;

/// Every dispatch mode, so "entropy and hashing are NOT mode-gated" is
/// checked rather than assumed. The SSG denial is about *network*; a
/// build-time render getting weaker randomness than the request-time
/// render of the same component would be its own footgun.
const BOTH_MODES: [DispatchMode; 2] = [DispatchMode::BuildTime, DispatchMode::RequestTime];

/// JS helpers used by most scripts below: hex rendering for digest
/// output, and a synchronous throw-describer (`DESCRIBE`'s
/// `expectReject` covers the promise case).
const HELPERS: &str = r#"
  const hex = (buf) =>
    Array.from(new Uint8Array(buf)).map((b) => b.toString(16).padStart(2, "0")).join("");
  const expectThrow = (fn) => {
    try {
      const r = fn();
      return "NO-THROW|" + String(r);
    } catch (e) {
      return String(e && e.name) + "|" + String(e && e.message);
    }
  };
"#;

// ---------------------------------------------------------------
// `Math.random` is gone
// ---------------------------------------------------------------

/// The headline of #1751: the polyfill must contain **no**
/// `Math.random` at all.
///
/// A source-level assertion on purpose. A behavioural test cannot see
/// the difference — `Math.random` produces bytes that look exactly as
/// random as the CSPRNG's to every functional check, which is precisely
/// why the bug survived. Asserted across the whole file rather than the
/// crypto section, because a fallback re-introduced anywhere in it (a
/// `catch` that degrades, a "just for SSG" branch) is the same bug.
#[test]
fn the_polyfill_carries_no_math_random_anywhere() {
    // Comments MAY name it — the crypto section's header explains what
    // was removed and why, and that prose is worth keeping. Only
    // executable references are the bug, so a `//` ahead of the mention
    // on the same line is what distinguishes them.
    let offenders: Vec<&str> = WEB_POLYFILLS_SRC
        .lines()
        .filter(|line| match (line.find("Math.random"), line.find("//")) {
            (Some(_), None) => true,
            (Some(at), Some(comment)) => comment > at,
            (None, _) => false,
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "web_polyfills.js still CALLS Math.random — issue #1751 is that a session ID or CSRF \
         token minted during SSR must never come from a non-cryptographic source, and no \
         functional test can tell the two apart:\n{offenders:#?}"
    );
}

/// The JS side must read every numeric limit out of `__zfb.limits`
/// (issue #2016), never carry its own copy. A duplicated constant
/// drifts from `limits.rs` silently while every test still passes.
#[test]
fn the_polyfill_carries_no_hardcoded_copy_of_the_random_byte_quota() {
    for spelling in ["65536", "65_536"] {
        assert!(
            !WEB_POLYFILLS_SRC.contains(spelling),
            "web_polyfills.js hardcodes the random-bytes quota as {spelling}; it must read \
             __zfb.limits.maxRandomBytesPerCall so the Rust constant stays the one source of truth"
        );
    }
    assert!(
        WEB_POLYFILLS_SRC.contains("maxRandomBytesPerCall"),
        "web_polyfills.js must read the quota out of the Rust-injected limits object"
    );
}

// ---------------------------------------------------------------
// `getRandomValues` — typed-array eligibility
// ---------------------------------------------------------------

/// Exactly Cloudflare's documented eligible-view list, every entry
/// accepted, each returning **the same object** it was handed.
#[tokio::test]
async fn every_eligible_integer_view_is_accepted_and_the_same_object_comes_back() {
    let script = r#"
      const names = [
        "Int8Array", "Uint8Array", "Uint8ClampedArray", "Int16Array", "Uint16Array",
        "Int32Array", "Uint32Array", "BigInt64Array", "BigUint64Array",
      ];
      const out = [];
      for (const name of names) {
        const ctor = globalThis[name];
        if (typeof ctor !== "function") { out.push(name + ":MISSING-CTOR"); continue; }
        const view = new ctor(4);
        try {
          const returned = crypto.getRandomValues(view);
          out.push(name + ":" + (returned === view ? "same" : "DIFFERENT-OBJECT"));
        } catch (e) {
          out.push(name + ":THREW:" + e.name);
        }
      }
      return out.join(",");
    "#;
    assert_eq!(
        probe(script, DispatchMode::RequestTime).await,
        "Int8Array:same,Uint8Array:same,Uint8ClampedArray:same,Int16Array:same,\
         Uint16Array:same,Int32Array:same,Uint32Array:same,BigInt64Array:same,\
         BigUint64Array:same"
    );
}

/// Ineligible views and non-views throw `TypeMismatchError` with the
/// contract's message, naming the constructor that was rejected — a
/// bare "wrong type" would leave a caller guessing which argument.
#[tokio::test]
async fn ineligible_views_and_non_views_throw_type_mismatch_error() {
    let script = format!(
        r#"{HELPERS}
        const cases = [];
        cases.push(expectThrow(() => crypto.getRandomValues(new Float32Array(4))));
        cases.push(expectThrow(() => crypto.getRandomValues(new Float64Array(4))));
        cases.push(expectThrow(() => crypto.getRandomValues(new DataView(new ArrayBuffer(8)))));
        cases.push(expectThrow(() => crypto.getRandomValues([1, 2, 3])));
        cases.push(expectThrow(() => crypto.getRandomValues(new ArrayBuffer(8))));
        cases.push(expectThrow(() => crypto.getRandomValues(null)));
        cases.push(
          typeof Float16Array === "function"
            ? expectThrow(() => crypto.getRandomValues(new Float16Array(4)))
            : "TypeMismatchError|crypto.getRandomValues: Float16Array is not an integer-typed ArrayBufferView",
        );
        return cases.join("\n");
        "#
    );
    let expected = [
        "Float32Array",
        "Float64Array",
        "DataView",
        "Array",
        "ArrayBuffer",
        "null",
        "Float16Array",
    ]
    .map(|ctor| {
        format!("TypeMismatchError|crypto.getRandomValues: {ctor} is not an integer-typed ArrayBufferView")
    })
    .join("\n");
    assert_eq!(probe(&script, DispatchMode::RequestTime).await, expected);
}

// ---------------------------------------------------------------
// `getRandomValues` — the quota, measured in BYTES
// ---------------------------------------------------------------

/// The contract's quota boundary, exercised at the real ceiling.
///
/// `Uint32Array(16_384)` is 65,536 bytes (accepted) and
/// `Uint32Array(16_385)` is 65,540 bytes (rejected) — the pair that
/// makes the element-count reading visibly wrong at the boundary
/// itself. `Uint32Array(20_000)` is the issue's own example: 20,000
/// elements, 80,000 bytes.
#[tokio::test]
async fn the_quota_boundary_is_measured_on_byte_length_not_element_count() {
    let quota = MAX_RANDOM_BYTES_PER_CALL;
    let script = format!(
        r#"{HELPERS}
        const at = (ctor, n) => expectThrow(() => {{ crypto.getRandomValues(new ctor(n)); return "accepted"; }});
        return [
          "u8@max:" + at(Uint8Array, {quota}),
          "u8@max+1:" + at(Uint8Array, {quota_plus}),
          "u32@max:" + at(Uint32Array, {u32_at_max}),
          "u32@max+1:" + at(Uint32Array, {u32_over}),
          "u32@20000:" + at(Uint32Array, 20000),
        ].join("\n");
        "#,
        quota = quota,
        quota_plus = quota + 1,
        u32_at_max = quota / 4,
        u32_over = quota / 4 + 1,
    );
    let over = |bytes: usize| {
        format!("QuotaExceededError|crypto.getRandomValues: requested {bytes} bytes, quota is {quota} bytes")
    };
    assert_eq!(
        probe(&script, DispatchMode::RequestTime).await,
        [
            "u8@max:NO-THROW|accepted".to_string(),
            format!("u8@max+1:{}", over(quota + 1)),
            "u32@max:NO-THROW|accepted".to_string(),
            format!("u32@max+1:{}", over(quota + 4)),
            format!("u32@20000:{}", over(80_000)),
        ]
        .join("\n")
    );
}

/// **The falsifier for the `byteLength`-vs-element-count rule.**
///
/// The quota boundary test above cannot isolate the JS layer: the native
/// `randomBytes` callback enforces the identical ceiling on the identical
/// byte count, so an element-count check in JS would let the call through
/// to a native rejection carrying the same name and message.
///
/// The Deno original separated the layers by booting with
/// `maxRandomBytesPerCall` lowered to 8; the native host has no public seam
/// for that limit (only the fetch limits ride `FetchConfig`). This port
/// separates them with a spy on `__zfbHost.randomBytes` at the real quota
/// instead: `Uint32Array(quota / 4 + 1)` has fewer ELEMENTS than the quota
/// but more BYTES, so a byteLength check rejects it in JS with **zero**
/// callback calls, while an element-count check would call the callback once.
/// The accepted `Uint8Array(quota)` proves the spy does observe a call that
/// passes the JS check. That the JS reads the injected limit rather than a
/// literal is pinned by
/// [`the_polyfill_carries_no_hardcoded_copy_of_the_random_byte_quota`].
#[tokio::test]
async fn the_js_quota_check_measures_bytes_before_calling_the_host() {
    let quota = MAX_RANDOM_BYTES_PER_CALL;
    let script = format!(
        r#"{HELPERS}
        const ops = globalThis.__zfbHost;
        const real = ops.randomBytes;
        let calls = 0;
        ops.randomBytes = (buf) => {{ calls += 1; return real(buf); }};
        try {{
          const at = (ctor, n) => {{
            const before = calls;
            const outcome = expectThrow(() => {{ crypto.getRandomValues(new ctor(n)); return "accepted"; }});
            return outcome + "|calls=" + (calls - before);
          }};
          return [
            "u8@max:" + at(Uint8Array, {quota}),
            "u32@max+1:" + at(Uint32Array, {u32_over}),
          ].join("\n");
        }} finally {{
          ops.randomBytes = real;
        }}
        "#,
        u32_over = quota / 4 + 1,
    );
    assert_eq!(
        probe(&script, DispatchMode::RequestTime).await,
        format!(
            "u8@max:NO-THROW|accepted|calls=1\n\
             u32@max+1:QuotaExceededError|crypto.getRandomValues: requested {} bytes, quota is \
             {quota} bytes|calls=0",
            quota + 4
        )
    );
}

/// A zero-length view is a no-op that returns the view — asserted for
/// both a byte view and a wider element type, since "length 0" and
/// "byteLength 0" coincide there but the code path reads the latter.
#[tokio::test]
async fn a_zero_length_view_is_a_no_op_that_returns_the_view() {
    let script = r#"
      const u8 = new Uint8Array(0);
      const u32 = new Uint32Array(0);
      return [
        crypto.getRandomValues(u8) === u8,
        crypto.getRandomValues(u32) === u32,
      ].join(",");
    "#;
    assert_eq!(probe(script, DispatchMode::RequestTime).await, "true,true");
}

// ---------------------------------------------------------------
// `randomUUID` — version and variant invariants
// ---------------------------------------------------------------

/// Length, separator positions, version nibble, variant nibble, and
/// lowercase-hex alphabet. **Never exact bytes** — these are the
/// invariants RFC 4122 fixes, and they are what a consumer parses back.
///
/// Run over 64 UUIDs so a version/variant bug that only manifests for
/// some random inputs cannot slip through as luck. That is a bounded
/// invariant sweep, not a distribution test: no property of the
/// *spread* of values is asserted.
#[tokio::test]
async fn random_uuid_sets_the_version_and_variant_bits_and_is_well_formed() {
    let script = r#"
      const failures = [];
      const seen = new Set();
      for (let i = 0; i < 64; i++) {
        const s = crypto.randomUUID();
        if (s.length !== 36) failures.push("length:" + s.length);
        if (s[8] !== "-" || s[13] !== "-" || s[18] !== "-" || s[23] !== "-") {
          failures.push("separators:" + s);
        }
        if (s[14] !== "4") failures.push("version:" + s[14]);
        if (!["8", "9", "a", "b"].includes(s[19])) failures.push("variant:" + s[19]);
        if (!/^[0-9a-f-]{36}$/.test(s)) failures.push("alphabet:" + s);
        seen.add(s);
      }
      // Not a distribution claim: 64 collisions-free draws from a
      // working CSPRNG is certain, and a constant or no-op source is
      // what this catches.
      if (seen.size !== 64) failures.push("distinct:" + seen.size);
      return failures.length === 0 ? "ok" : failures.join(",");
    "#;
    assert_eq!(probe(script, DispatchMode::RequestTime).await, "ok");
}

// ---------------------------------------------------------------
// Wiring: the OS entropy host op is actually invoked
// ---------------------------------------------------------------

/// Both entropy APIs route through `__zfbHost.randomBytes`, with exact
/// call counts, and the OS CSPRNG behind it runs.
///
/// The JS spy carries the exactness (it wraps the op the polyfill
/// resolves at call time). [`OS_ENTROPY_CALLS`] is
/// process-wide, so with the test binary running in parallel it is
/// asserted as a **lower bound** — the delta cannot be smaller than
/// what this probe demanded unless the OS source was skipped.
#[tokio::test]
async fn both_entropy_apis_invoke_the_os_entropy_host_op() {
    const CALLS: u64 = 8;
    let script = format!(
        r#"
        const ops = globalThis.__zfbHost;
        const real = ops.randomBytes;
        let calls = 0;
        let bytes = 0;
        ops.randomBytes = (buf) => {{
          calls += 1;
          bytes += buf.byteLength;
          return real(buf);
        }};
        try {{
          for (let i = 0; i < {half}; i++) crypto.getRandomValues(new Uint8Array(32));
          for (let i = 0; i < {half}; i++) crypto.randomUUID();
          return `calls=${{calls}} bytes=${{bytes}}`;
        }} finally {{
          ops.randomBytes = real;
        }}
        "#,
        half = CALLS / 2,
    );

    let before = OS_ENTROPY_CALLS.load(Ordering::Relaxed);
    let observed = probe(&script, DispatchMode::RequestTime).await;
    let after = OS_ENTROPY_CALLS.load(Ordering::Relaxed);

    // 4 × 32 bytes for getRandomValues + 4 × 16 bytes for randomUUID.
    assert_eq!(observed, format!("calls={CALLS} bytes={}", 4 * 32 + 4 * 16));
    assert!(
        after - before >= CALLS,
        "the production OS entropy source ran fewer than {CALLS} times across a probe that \
         demanded exactly {CALLS} fills ({before} -> {after}); the op is not reaching the OS CSPRNG"
    );
}

/// Entropy is **not** mode-gated: the same call works at build time and
/// at request time. Mode-dependent randomness quality would mean a
/// prerendered page's nonce was weaker than the request-time render of
/// the identical component.
#[tokio::test]
async fn entropy_and_digest_are_available_in_both_dispatch_modes() {
    let script = r#"
      const view = crypto.getRandomValues(new Uint8Array(16));
      const uuid = crypto.randomUUID();
      const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode("abc"));
      return [
        "bytes=" + view.byteLength,
        "uuid=" + uuid.length + uuid[14],
        "digest=" + digest.byteLength,
      ].join(",");
    "#;
    for mode in BOTH_MODES {
        assert_eq!(
            probe(script, mode).await,
            "bytes=16,uuid=364,digest=32",
            "web crypto must behave identically in {mode:?}"
        );
    }
}

// ---------------------------------------------------------------
// Fail closed
// ---------------------------------------------------------------

/// **The security property this sub-issue exists for.**
///
/// With the host op unreachable, both entropy APIs must THROW and
/// produce no values. A regression that degrades to `Math.random` (or
/// to a zero fill, or to any other "at least it works" fallback) passes
/// every other test in this file: the bytes arrive, the UUID is
/// well-formed, nothing looks broken. Only this assertion catches it,
/// which is why the caller's buffer is checked byte-for-byte against
/// the sentinel it was pre-filled with.
#[tokio::test]
async fn with_the_host_op_unreachable_both_entropy_apis_throw_and_write_nothing() {
    let script = format!(
        r#"{HELPERS}
        const ops = globalThis.__zfbHost;
        const real = ops.randomBytes;
        delete ops.randomBytes;
        try {{
          const buf = new Uint8Array(32).fill(0x5a);
          const thrown = expectThrow(() => crypto.getRandomValues(buf));
          const untouched = buf.every((b) => b === 0x5a);
          const uuid = expectThrow(() => crypto.randomUUID());
          return [thrown, "untouched=" + untouched, uuid].join("\n");
        }} finally {{
          ops.randomBytes = real;
        }}
        "#
    );
    assert_eq!(
        probe(&script, DispatchMode::RequestTime).await,
        "Error|crypto.getRandomValues: OS entropy unavailable: op_zfb_random_bytes is not \
         registered in this runtime\n\
         untouched=true\n\
         Error|crypto.randomUUID: OS entropy unavailable: op_zfb_random_bytes is not registered \
         in this runtime"
    );
}

/// Removing the backend globals must preserve the same fail-closed diagnostic
/// as an absent operation, without a property-access TypeError from the adapter.
#[tokio::test]
async fn absent_backend_globals_preserve_the_entropy_unavailable_diagnostic() {
    // The native bridge global is `__zfbHost`; the Deno original cleared
    // `globalThis.Deno` / `Deno.core` instead.
    for absent in [
        "globalThis.__zfbHost = undefined",
        "globalThis.__zfbHost = {}",
    ] {
        let script = format!(
            r#"{HELPERS}
            const originalHost = globalThis.__zfbHost;
            try {{
              {absent};
              const bytes = new Uint8Array(1).fill(0x5a);
              const thrown = expectThrow(() => crypto.getRandomValues(bytes));
              return thrown + "\nuntouched=" + (bytes[0] === 0x5a);
            }} finally {{
              globalThis.__zfbHost = originalHost;
            }}
            "#
        );
        assert_eq!(
            probe(&script, DispatchMode::RequestTime).await,
            "Error|crypto.getRandomValues: OS entropy unavailable: op_zfb_random_bytes is not registered in this runtime\nuntouched=true"
        );
    }
}

/// A zero-length view is the ONE case that still succeeds with no
/// entropy source at all — zero bytes cannot be weak, and the contract
/// makes it a no-op that returns the view. Pinned separately so the
/// short-circuit cannot be widened into "small requests are fine"
/// without a test noticing.
#[tokio::test]
async fn a_zero_length_view_still_succeeds_with_the_host_op_unreachable() {
    let script = r#"
      const ops = globalThis.__zfbHost;
      const real = ops.randomBytes;
      delete ops.randomBytes;
      try {
        const empty = new Uint8Array(0);
        const same = crypto.getRandomValues(empty) === empty;
        let oneByteThrew = false;
        try { crypto.getRandomValues(new Uint8Array(1)); } catch (_) { oneByteThrew = true; }
        return "zero=" + same + ",one=" + oneByteThrew;
      } finally {
        ops.randomBytes = real;
      }
    "#;
    assert_eq!(
        probe(script, DispatchMode::RequestTime).await,
        "zero=true,one=true"
    );
}

// ---------------------------------------------------------------
// SubtleCrypto — the implemented half
// ---------------------------------------------------------------

/// `digest` returns a real `Promise<ArrayBuffer>` whose bytes match the
/// published vectors, for all four supported algorithms, through the
/// JS boundary. The `{ name }` object form and case-insensitive
/// matching are covered here too because they are what the JS layer
/// normalises before the op sees them.
#[tokio::test]
async fn digest_matches_the_published_vectors_through_the_js_boundary() {
    let script = format!(
        r#"{HELPERS}
        const data = new TextEncoder().encode("abc");
        const one = await crypto.subtle.digest("SHA-256", data);
        return [
          "isArrayBuffer=" + (one instanceof ArrayBuffer),
          "sha1=" + hex(await crypto.subtle.digest("SHA-1", data)),
          "sha256=" + hex(one),
          "sha384=" + hex(await crypto.subtle.digest("SHA-384", data)),
          "sha512=" + hex(await crypto.subtle.digest("SHA-512", data)),
          "object-form=" + hex(await crypto.subtle.digest({{ name: "sha-256" }}, data)),
          "arraybuffer-input=" + hex(await crypto.subtle.digest("SHA-256", data.buffer)),
          "empty=" + hex(await crypto.subtle.digest("SHA-256", new Uint8Array(0))),
        ].join("\n");
        "#
    );
    assert_eq!(
        probe(&script, DispatchMode::RequestTime).await,
        "isArrayBuffer=true\n\
         sha1=a9993e364706816aba3e25717850c26c9cd0d89d\n\
         sha256=ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n\
         sha384=cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7\n\
         sha512=ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f\n\
         object-form=ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n\
         arraybuffer-input=ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n\
         empty=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

/// `timingSafeEqual` is synchronous, returns a boolean, and throws a
/// `TypeError` on a length mismatch (workerd's own behaviour). Equal
/// contents across two *different* view types with the same bytes must
/// compare true — a `byteLength`-based comparison, not an identity one.
#[tokio::test]
async fn timing_safe_equal_compares_bytes_and_rejects_a_length_mismatch() {
    let script = format!(
        r#"{HELPERS}
        const enc = new TextEncoder();
        const a = enc.encode("secret");
        const b = enc.encode("secret");
        const c = enc.encode("secreT");
        return [
          "equal=" + crypto.subtle.timingSafeEqual(a, b),
          "different=" + crypto.subtle.timingSafeEqual(a, c),
          "buffer-form=" + crypto.subtle.timingSafeEqual(a.buffer, b.buffer),
          "empty=" + crypto.subtle.timingSafeEqual(new Uint8Array(0), new Uint8Array(0)),
          "mismatch=" + expectThrow(() => crypto.subtle.timingSafeEqual(a, enc.encode("secrets"))),
        ].join("\n");
        "#
    );
    assert_eq!(
        probe(&script, DispatchMode::RequestTime).await,
        "equal=true\n\
         different=false\n\
         buffer-form=true\n\
         empty=true\n\
         mismatch=TypeError|crypto.subtle.timingSafeEqual: buffers must be the same byteLength"
    );
}

/// `timingSafeEqual` must not take an early exit on the first differing
/// byte — an early return leaks the position of the first difference
/// through timing, which is the entire reason the function exists.
/// Read off the source, because *measuring* the timing would be exactly
/// the flaky, unfalsifiable test guardrail 7 forbids.
#[test]
fn timing_safe_equal_accumulates_over_the_whole_buffer_with_no_early_exit() {
    let body = WEB_POLYFILLS_SRC
        .split("timingSafeEqual(a, b) {")
        .nth(1)
        .expect("timingSafeEqual is defined in the polyfill")
        .split("\n    },")
        .next()
        .expect("the method body is delimited");
    assert!(
        body.contains("diff |="),
        "timingSafeEqual must accumulate differences over the whole buffer"
    );
    assert!(
        !body.contains("return false"),
        "timingSafeEqual must not return early on a differing byte — that leaks the position of \
         the first difference through timing:\n{body}"
    );
}

// ---------------------------------------------------------------
// SubtleCrypto — the fail-closed half (divergences D7 and D8)
// ---------------------------------------------------------------

/// Divergence **D7**: `MD5` is rejected, and the rejection names the
/// set this host *does* implement so the caller has somewhere to go.
#[tokio::test]
async fn md5_is_rejected_with_not_supported_error_and_names_the_implemented_set() {
    let script = format!(
        r#"{DESCRIBE}
        return await expectReject(() =>
          crypto.subtle.digest("MD5", new TextEncoder().encode("abc")));
        "#
    );
    assert_eq!(
        probe(&script, DispatchMode::RequestTime).await,
        "NotSupportedError|crypto.subtle.digest: unsupported algorithm \"MD5\". \
         This host implements SHA-1, SHA-256, SHA-384, SHA-512."
    );
}

/// **Every** unimplemented method is `typeof === "function"` *and*
/// rejects.
///
/// The `typeof` half is not decoration: silent absence is what makes
/// `typeof crypto.subtle.sign === "function"` feature detection take a
/// local fallback branch production never takes — the exact class of
/// divergence #1751 exists to remove. A method that is simply missing
/// would satisfy "does not work" while failing this test.
#[tokio::test]
async fn every_unimplemented_subtle_method_is_present_and_rejects() {
    let methods = [
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
    let script = format!(
        r#"{DESCRIBE}
        const methods = {methods:?};
        const out = [];
        for (const m of methods) {{
          const present = typeof crypto.subtle[m] === "function";
          const outcome = await expectReject(() => crypto.subtle[m]());
          out.push(m + "|present=" + present + "|" + outcome);
        }}
        out.push(
          "DigestStream|present=" + (typeof crypto.subtle.DigestStream === "function") +
          "|global=" + (typeof crypto.DigestStream === "function"),
        );
        return out.join("\n");
        "#
    );
    let expected_tail = |method: &str| {
        format!(
            "NotSupportedError|crypto.subtle.{method} is not implemented in the zfb embedded \
             runtime. Production Cloudflare Workers DOES implement this call — see \
             research/2013-request-time-capability-contract.md. This host implements digest \
             (SHA-1/256/384/512) and timingSafeEqual only."
        )
    };
    let mut expected: Vec<String> = methods
        .iter()
        .map(|m| format!("{m}|present=true|{}", expected_tail(m)))
        .collect();
    expected.push("DigestStream|present=true|global=true".to_string());
    assert_eq!(
        probe(&script, DispatchMode::RequestTime).await,
        expected.join("\n")
    );
}

/// `DigestStream` is present as a constructor and throws on `new`
/// (a constructor cannot return a rejected promise).
#[tokio::test]
async fn the_digest_stream_constructor_is_present_and_throws() {
    let script = format!(
        r#"{HELPERS}
        return [
          expectThrow(() => new crypto.subtle.DigestStream("SHA-256")),
          expectThrow(() => new crypto.DigestStream("SHA-256")),
        ].join("\n");
        "#
    );
    let expected = "NotSupportedError|crypto.subtle.DigestStream is not implemented in the zfb \
                    embedded runtime. Production Cloudflare Workers DOES implement this call — \
                    see research/2013-request-time-capability-contract.md. This host implements \
                    digest (SHA-1/256/384/512) and timingSafeEqual only.";
    assert_eq!(
        probe(&script, DispatchMode::RequestTime).await,
        format!("{expected}\n{expected}")
    );
}

/// **The diagnostic-parity acceptance check** (#1751's "align local
/// diagnostics with production behavior", and #2018's own requirement).
///
/// For the fail-closed set, local diagnostics *cannot* be made to match
/// production, because production **succeeds** — that is contract
/// divergence D8, and synthesising a workerd-shaped error for a call
/// workerd would not have errored on would be actively misleading.
/// Alignment therefore means the message must orient the developer
/// toward production rather than away from it, and this test pins the
/// four properties that do that:
///
/// 1. it names the exact method, so the diagnostic is actionable;
/// 2. it states that production Workers DOES implement the call, so a
///    developer does not conclude their code is wrong;
/// 3. it names the contract document, so the divergence is one grep
///    away rather than folklore;
/// 4. it does **not** describe itself as a build-time/SSG policy
///    denial. That wording belongs to exactly one code path (the
///    network denial), and borrowing it here would send a request-time
///    caller looking for a build-step fix that does not exist.
///
/// The MD5 leg is the D7 half of the same requirement: production
/// supports MD5, this host does not, and the message says which
/// algorithms it does support instead of failing anonymously.
#[tokio::test]
async fn an_unsupported_subtle_diagnostic_orients_the_developer_toward_production() {
    let script = format!(
        r#"{DESCRIBE}
        return [
          await expectReject(() => crypto.subtle.sign("HMAC", {{}}, new Uint8Array(1))),
          await expectReject(() => crypto.subtle.digest("MD5", new Uint8Array(1))),
        ].join("\n");
        "#
    );
    let out = probe(&script, DispatchMode::RequestTime).await;
    let (d8, d7) = out.split_once('\n').expect("two diagnostics");

    for (label, message) in [("D8 (sign)", d8), ("D7 (MD5)", d7)] {
        assert!(
            message.starts_with("NotSupportedError|"),
            "{label}: an unimplemented capability must fail as NotSupportedError, the name a \
             caller checking err.name against production would use: {message}"
        );
        for forbidden in [
            "SSG",
            "build-time render",
            "Move the data fetch",
            "network requests",
        ] {
            assert!(
                !message.contains(forbidden),
                "{label}: the diagnostic borrows the SSG network-denial wording ({forbidden:?}), \
                 which sends a request-time caller looking for a build-step fix: {message}"
            );
        }
    }

    assert!(
        d8.contains("crypto.subtle.sign")
            && d8.contains("Production Cloudflare Workers DOES implement this call")
            && d8.contains("research/2013-request-time-capability-contract.md"),
        "D8: the diagnostic must name the method, say production implements it, and name the \
         contract document: {d8}"
    );
    assert!(
        d7.contains("unsupported algorithm \"MD5\"")
            && d7.contains("This host implements SHA-1, SHA-256, SHA-384, SHA-512."),
        "D7: the diagnostic must name the rejected algorithm and the supported set: {d7}"
    );
}

/// `CryptoKey` stays undefined — nothing in the implemented set
/// produces or consumes one, so a reference to it is a plain
/// `ReferenceError` rather than a stub that pretends to be a key.
#[tokio::test]
async fn crypto_key_is_not_defined() {
    let script = r#"return String(typeof CryptoKey) + "," + String(typeof globalThis.CryptoKey);"#;
    assert_eq!(
        probe(script, DispatchMode::RequestTime).await,
        "undefined,undefined"
    );
}

/// The native entropy callback itself rejects an over-quota buffer with the
/// contract's byte-quota message and class — the bridge-boundary twin of the
/// JS check above (ported from `src/embedded_v8/crypto/tests.rs`'s
/// `the_op_rejects_an_over_quota_buffer_at_the_v8_boundary`, #4393 S6a).
/// Called directly so the polyfill's own quota check cannot answer first.
#[tokio::test]
async fn the_native_random_bytes_callback_rejects_an_over_quota_buffer() {
    let over = MAX_RANDOM_BYTES_PER_CALL + 1;
    let script = format!(
        r#"{HELPERS}
        return expectThrow(() => globalThis.__zfbHost.randomBytes(new Uint8Array({over})));
        "#
    );
    assert_eq!(
        probe(&script, DispatchMode::RequestTime).await,
        format!(
            "QuotaExceededError|crypto.getRandomValues: requested {over} bytes, quota is \
             {MAX_RANDOM_BYTES_PER_CALL} bytes"
        )
    );
}
