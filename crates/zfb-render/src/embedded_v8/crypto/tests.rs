//! Acceptance matrix for the OS-entropy host primitive (issue #2017).
//!
//! **Deterministic only — never statistical.** Epic guardrail 7 (which
//! deliberately overrides #1751's own "statistical-shape tests"
//! wording): distribution tests are flaky and cannot demonstrate
//! cryptographic security. Nothing here asserts exact bytes either.
//!
//! What is asserted instead is the *source* and the *invariants*:
//!
//! - the op is wired to the OS CSPRNG (proved on the production
//!   extension's own registration, and by the OS source's call counter
//!   moving when the op runs),
//! - the failure path errors and never falls back,
//! - the three quota boundaries, measured in bytes.
//!
//! Blind spots, stated plainly:
//!
//! - Nothing here proves `getrandom` itself reads the kernel CSPRNG;
//!   that is the crate's contract, and re-testing it would mean
//!   re-implementing it.
//! - There is no JS surface in this sub-issue, so no test drives
//!   `crypto.getRandomValues`. #2018 owns that, including the
//!   `byteLength`-vs-element-count rejection at the JS boundary.

use std::cell::Cell;

use super::*;

/// A source that always fails, and — importantly — writes **nothing**.
/// If the production code ever grew a fallback, the buffer handed to it
/// would come back filled while this source reports failure, which is
/// exactly what `a_failing_entropy_source_errors_and_never_falls_back`
/// detects.
struct FailingEntropy {
    calls: Cell<u64>,
}

impl FailingEntropy {
    fn new() -> Self {
        Self {
            calls: Cell::new(0),
        }
    }
}

impl EntropySource for FailingEntropy {
    fn kind(&self) -> &'static str {
        "test-failing"
    }

    fn fill(&self, _dst: &mut [u8]) -> std::result::Result<(), String> {
        self.calls.set(self.calls.get() + 1);
        Err("simulated OS entropy failure".to_string())
    }
}

/// Records every requested length, so a test can assert the core
/// reached the source at all (and with the length it was given) without
/// looking at bytes.
struct RecordingEntropy {
    calls: Cell<u64>,
    last_len: Cell<usize>,
}

impl RecordingEntropy {
    fn new() -> Self {
        Self {
            calls: Cell::new(0),
            last_len: Cell::new(usize::MAX),
        }
    }
}

impl EntropySource for RecordingEntropy {
    fn kind(&self) -> &'static str {
        "test-recording"
    }

    fn fill(&self, dst: &mut [u8]) -> std::result::Result<(), String> {
        self.calls.set(self.calls.get() + 1);
        self.last_len.set(dst.len());
        // Write a non-zero marker so "the buffer was written" is
        // distinguishable from "the buffer was left alone"; this is
        // wiring, not a distribution claim.
        dst.fill(0xAB);
        Ok(())
    }
}

// ---------------------------------------------------------------
// Fail closed
// ---------------------------------------------------------------

/// **The security property this whole sub-issue exists for.**
///
/// A fallback to weak randomness passes every functional test: the
/// bytes still arrive, the API still works, nothing looks broken. Only
/// this assertion catches it. The buffer is pre-filled with a sentinel
/// so a fallback that zero-fills, or one that writes a seeded PRNG's
/// output, is caught the same way — the error must come back and the
/// buffer must be left as the caller had it.
#[test]
fn a_failing_entropy_source_errors_and_never_falls_back() {
    let source = FailingEntropy::new();
    const SENTINEL: u8 = 0x5A;
    let mut buf = [SENTINEL; 32];

    let result = fill_random_bytes(&source, &mut buf);

    assert_eq!(
        result,
        Err(EntropyError::Unavailable {
            detail: "simulated OS entropy failure".to_string(),
        }),
        "a failing OS entropy source must propagate as an error"
    );
    assert_eq!(source.calls.get(), 1, "the source must have been consulted");
    assert!(
        buf.iter().all(|&b| b == SENTINEL),
        "fail-closed violated: the buffer was written despite the source failing \
         (some fallback produced bytes)"
    );
    // Stated separately from the sentinel check because a zero fill is
    // the single most likely accidental fallback, and `[0u8; 32]` would
    // be the *most* dangerous "random" value to hand a caller.
    assert!(
        !buf.iter().all(|&b| b == 0),
        "fail-closed violated: the buffer came back zero-filled"
    );
}

/// The zero-length no-op is not a hole in fail-closed. A caller asking
/// for 0 bytes from a broken CSPRNG still learns the CSPRNG is broken,
/// because the length check never short-circuits the source call.
#[test]
fn a_failing_source_errors_even_for_a_zero_length_request() {
    let source = FailingEntropy::new();
    let mut buf: [u8; 0] = [];
    assert!(fill_random_bytes(&source, &mut buf).is_err());
    assert_eq!(source.calls.get(), 1);
}

/// The unavailable error is a plain `Error` and carries no
/// `crypto.<method>:` prefix — #2018 adds the right one per API
/// (`crypto.randomUUID: OS entropy unavailable: …` in the contract).
#[test]
fn the_unavailable_error_is_unprefixed_and_maps_to_a_plain_error_class() {
    let err = EntropyError::Unavailable {
        detail: "boom".to_string(),
    };
    assert_eq!(err.to_string(), "OS entropy unavailable: boom");
    assert_eq!(err.js_error_class(), "Error");
}

// ---------------------------------------------------------------
// Boundaries — measured in BYTES
// ---------------------------------------------------------------

/// `0`, `1`, and exactly `MAX_RANDOM_BYTES_PER_CALL` are all accepted;
/// one byte over is rejected. The rejection is checked as a whole
/// `EntropyError` value, so a regression that keeps rejecting but at a
/// different quota fails here too.
#[test]
fn zero_one_and_the_maximum_are_accepted_and_one_over_is_rejected() {
    let source = RecordingEntropy::new();

    let mut empty: [u8; 0] = [];
    assert_eq!(fill_random_bytes(&source, &mut empty), Ok(()));
    assert_eq!(source.last_len.get(), 0);

    let mut one = [0u8; 1];
    assert_eq!(fill_random_bytes(&source, &mut one), Ok(()));
    assert_eq!(source.last_len.get(), 1);

    let mut at_max = vec![0u8; limits::MAX_RANDOM_BYTES_PER_CALL];
    assert_eq!(fill_random_bytes(&source, &mut at_max), Ok(()));
    assert_eq!(source.last_len.get(), limits::MAX_RANDOM_BYTES_PER_CALL);

    let mut over = vec![0u8; limits::MAX_RANDOM_BYTES_PER_CALL + 1];
    assert_eq!(
        fill_random_bytes(&source, &mut over),
        Err(EntropyError::QuotaExceeded {
            requested: limits::MAX_RANDOM_BYTES_PER_CALL + 1,
            limit: limits::MAX_RANDOM_BYTES_PER_CALL,
        })
    );
}

/// An over-quota request must be rejected **before** the source is
/// touched — a 4 GB request should not become a 4 GB read from the
/// kernel that is then thrown away.
#[test]
fn an_over_quota_request_never_reaches_the_entropy_source() {
    let source = RecordingEntropy::new();
    let mut over = vec![0u8; limits::MAX_RANDOM_BYTES_PER_CALL + 1];
    assert!(fill_random_bytes(&source, &mut over).is_err());
    assert_eq!(
        source.calls.get(),
        0,
        "the quota check must run before the source is consulted"
    );
}

/// The ceiling is applied to the buffer's **byte length**, and its
/// message quotes bytes — this is what stops it drifting from #2018's
/// `byteLength` check at the JS boundary. A `Uint32Array` of 16_385
/// elements is 65_540 bytes: under any element-count reading of the
/// quota it would pass, and it must not.
#[test]
fn the_ceiling_is_enforced_on_byte_length_not_element_count() {
    let source = RecordingEntropy::new();
    const U32_ELEMENTS: usize = 16_385;
    let byte_len = U32_ELEMENTS * std::mem::size_of::<u32>();
    // The premise of the case: over quota by bytes, under it by
    // elements. `const` so a future edit to either number that breaks
    // the premise is a compile error, not a silently vacuous test.
    const _: () = assert!(U32_ELEMENTS * 4 > limits::MAX_RANDOM_BYTES_PER_CALL);
    const _: () = assert!(U32_ELEMENTS < limits::MAX_RANDOM_BYTES_PER_CALL);

    let mut buf = vec![0u8; byte_len];
    let err = fill_random_bytes(&source, &mut buf).expect_err("65_540 bytes is over quota");
    assert_eq!(
        err.to_string(),
        format!(
            "crypto.getRandomValues: requested {byte_len} bytes, quota is {} bytes",
            limits::MAX_RANDOM_BYTES_PER_CALL
        ),
        "the message must quote BYTES, in the contract's exact wording, so the Rust \
         ceiling and #2018's byteLength check cannot drift apart"
    );
    assert_eq!(err.js_error_class(), "QuotaExceededError");
}

// ---------------------------------------------------------------
// The source really is the OS CSPRNG
// ---------------------------------------------------------------

/// The **production** extension installs the OS CSPRNG — asserted
/// against the real `zfb_crypto` registration's own state, not against
/// a hand-built source. A swap to a userspace or seeded generator
/// changes `kind()` and fails here.
#[test]
fn the_production_extension_installs_the_os_csprng() {
    let runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
        extensions: crate::embedded_v8::build_extensions(),
        ..Default::default()
    });
    let state = runtime.op_state();
    let state = state.borrow();
    let installed = state
        .try_borrow::<HostEntropySource>()
        .expect("the production extension parks an entropy source in OpState");
    assert_eq!(installed.0.kind(), OS_CSPRNG_KIND);
}

/// The op, driven through a real V8 isolate built from the production
/// `build_extensions()`, reaches [`OsEntropy::fill`] — the OS entropy
/// host op is *invoked*, which is the guardrail's own phrasing. Proved
/// by the OS source's call counter moving across the call, so a
/// regression that resolved some other source out of `OpState` would
/// fail even though the buffer still came back filled.
#[test]
fn driving_the_op_through_a_real_isolate_invokes_the_os_entropy_source() {
    let mut runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
        extensions: crate::embedded_v8::build_extensions(),
        ..Default::default()
    });

    let before = OS_ENTROPY_CALLS.load(Ordering::Relaxed);
    let result = runtime
        .execute_script(
            "zfb:crypto_op_probe",
            // No JS surface is added by this sub-issue: the op is
            // reached through deno_core's own op table, exactly as
            // #2018's polyfill will.
            r#"
            (() => {
              const buf = new Uint8Array(32);
              Deno.core.ops.op_zfb_random_bytes(buf);
              return buf.some((b) => b !== 0) ? "written" : "untouched";
            })()
            "#,
        )
        .expect("the op call must not throw");
    let after = OS_ENTROPY_CALLS.load(Ordering::Relaxed);

    assert!(
        after > before,
        "the OS entropy source was never invoked (counter did not move), so the op is \
         not wired to the OS CSPRNG"
    );

    deno_core::scope!(scope, &mut runtime);
    let value = deno_core::v8::Local::new(scope, result);
    assert_eq!(value.to_rust_string_lossy(scope), "written");
}

/// The op rejects an over-quota buffer at the V8 boundary too, and the
/// throw carries the contract's byte-quota message. Pinned through a
/// real isolate because that is the path #2018 will take.
#[test]
fn the_op_rejects_an_over_quota_buffer_at_the_v8_boundary() {
    let mut runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
        extensions: crate::embedded_v8::build_extensions(),
        ..Default::default()
    });
    let over = limits::MAX_RANDOM_BYTES_PER_CALL + 1;
    let result = runtime.execute_script(
        "zfb:crypto_quota_probe",
        format!(
            r#"
            // deno_core rebuilds an op's Rust-side error in JS through
            // its own `buildCustomError`, which can only construct the
            // classes in its `errorMap` — the six ECMAScript builtins
            // plus whatever was registered. WITHOUT this registration
            // the `QuotaExceededError` box arrives as a thrown
            // `undefined`: no name, no message, no quota diagnostic at
            // all. `web_polyfills.js` already does exactly this for the
            // transport's `TimeoutError`; #2018 must add the same line
            // for `QuotaExceededError` when it builds the JS surface.
            // This probe registers it locally because #2017 adds no JS
            // surface of its own.
            class QuotaExceededError extends Error {{
              constructor(message) {{
                super(message);
                this.name = "QuotaExceededError";
              }}
            }}
            Deno.core.registerErrorClass("QuotaExceededError", QuotaExceededError);
            (() => {{
              try {{
                Deno.core.ops.op_zfb_random_bytes(new Uint8Array({over}));
                return "NO-THROW";
              }} catch (e) {{
                return `${{e.name}}|${{e.message}}`;
              }}
            }})()
            "#
        ),
    );
    let result = result.expect("the probe itself must not fail to evaluate");
    deno_core::scope!(scope, &mut runtime);
    let value = deno_core::v8::Local::new(scope, result);
    assert_eq!(
        value.to_rust_string_lossy(scope),
        format!(
            "QuotaExceededError|crypto.getRandomValues: requested {over} bytes, quota is {} bytes",
            limits::MAX_RANDOM_BYTES_PER_CALL
        )
    );
}

/// The op **is** synchronous, read off deno_core's own `OpDecl`. The
/// module header explains why that is deliberate and not a guardrail-1
/// violation; this is the mechanical half, so a future "fix" to an
/// async op fails a test instead of silently breaking every caller of
/// the synchronous-by-spec `crypto.getRandomValues`.
#[test]
fn the_entropy_op_is_registered_as_a_synchronous_op() {
    assert!(op_is_sync());
}

/// A cheap sanity check that the OS source actually writes the buffer —
/// **not** a distribution test. Two 32-byte reads from a working CSPRNG
/// colliding is a ~2^-256 event; a source that silently no-ops, or one
/// that returns a constant, is what this catches.
#[test]
fn two_successive_os_reads_do_not_return_the_identical_buffer() {
    let source = OsEntropy;
    let mut first = [0u8; 32];
    let mut second = [0u8; 32];
    fill_random_bytes(&source, &mut first).expect("the OS CSPRNG is available in test");
    fill_random_bytes(&source, &mut second).expect("the OS CSPRNG is available in test");
    assert_ne!(first, second);
}

// The op registered WITHOUT its state initialiser, so the
// `OpState`-miss branch — the runtime-shutting-down / extension-half-
// installed case — can actually be driven rather than described.
deno_core::extension!(zfb_crypto_stateless_for_test, ops = [op_zfb_random_bytes],);

/// A missing source in `OpState` is a host-op failure, not a fallback:
/// the op throws and the caller's buffer is left untouched. Without
/// this the shutdown path could quietly return an all-zero buffer,
/// which a caller cannot distinguish from 32 unlucky random bytes.
#[test]
fn an_uninstalled_entropy_source_fails_closed() {
    let mut runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
        extensions: vec![zfb_crypto_stateless_for_test::init()],
        ..Default::default()
    });
    let result = runtime
        .execute_script(
            "zfb:crypto_absent_probe",
            r#"
            (() => {
              const buf = new Uint8Array(32).fill(0x5A);
              try {
                Deno.core.ops.op_zfb_random_bytes(buf);
                return "NO-THROW";
              } catch (e) {
                const untouched = buf.every((b) => b === 0x5A);
                return `${e.name}|${e.message}|${untouched ? "untouched" : "WRITTEN"}`;
              }
            })()
            "#,
        )
        .expect("the probe must evaluate");
    deno_core::scope!(scope, &mut runtime);
    let value = deno_core::v8::Local::new(scope, result);
    assert_eq!(
        value.to_rust_string_lossy(scope),
        "Error|OS entropy unavailable: OS entropy source is not installed in this runtime|untouched"
    );
}
