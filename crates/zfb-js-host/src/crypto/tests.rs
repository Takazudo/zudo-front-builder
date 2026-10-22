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
//! - No test here drives `crypto.getRandomValues`; the JS surface and the
//!   native `__zfbHost.randomBytes` boundary are covered by
//!   `zfb-render/tests/direct_v8_web_crypto.rs` and
//!   `native/services_tests.rs`.
//!
//! Moved here from `zfb-render/src/embedded_v8/crypto/tests.rs` for #4393
//! (S6a); the Deno `OpState`/`OpDecl` cases stay with the Deno host.

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
fn the_production_os_entropy_mapping_propagates_failure_and_never_swallows_it() {
    // Every OTHER fail-closed test in this file drives an injected
    // double, because the OS CSPRNG cannot be made to fail on demand.
    // That leaves `OsEntropy::fill`'s own error mapping — the code that
    // actually runs in production — covered by nothing. Verified during
    // the epic review: rewriting it to `let _ = getrandom::fill(dst);
    // Ok(())` (the #1751 silent-degradation bug, exactly) failed NONE of
    // the 13 crypto tests. This asserts the production mapping directly.
    let failed = map_os_entropy_result(Err(getrandom::Error::UNEXPECTED));
    assert!(
        failed.is_err(),
        "the production OS-entropy mapping must propagate a getrandom failure, \
         never swallow it into Ok — silent degradation to predictable randomness \
         is the bug this sub-issue exists to eliminate"
    );
    assert!(
        map_os_entropy_result(Ok(())).is_ok(),
        "a successful OS read must stay successful"
    );
}

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

/// The zero-length request succeeds **even against a broken CSPRNG**,
/// and never consults it — the contract's "a zero-length view is a
/// no-op that returns the view `[spec]`". #2018's
/// `getRandomValues(new Uint8Array(0))` must therefore return its view
/// on a host with no working entropy at all.
///
/// This is not a fail-closed hole: fail-closed exists to stop weak
/// bytes reaching a caller, and zero bytes cannot be weak. The
/// one-byte case immediately below is the boundary that proves the
/// short-circuit is exactly one element wide.
#[test]
fn a_zero_length_request_is_a_no_op_that_never_consults_the_source() {
    let source = FailingEntropy::new();
    let mut buf: [u8; 0] = [];
    assert_eq!(fill_random_bytes(&source, &mut buf), Ok(()));
    assert_eq!(
        source.calls.get(),
        0,
        "a zero-length no-op must not reach the entropy source at all"
    );
}

/// One byte — the smallest request that carries entropy — still fails
/// closed. Without this, widening the zero-length short-circuit into
/// "small requests are fine" would go unnoticed.
#[test]
fn a_one_byte_request_still_fails_closed() {
    let source = FailingEntropy::new();
    let mut buf = [0x5Au8; 1];
    assert!(fill_random_bytes(&source, &mut buf).is_err());
    assert_eq!(source.calls.get(), 1);
    assert_eq!(buf, [0x5A], "no fallback may write the byte");
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
    // No `last_len` assertion here: the zero-length case short-circuits
    // before the source, which its own test above pins.
    assert_eq!(source.calls.get(), 0);

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
