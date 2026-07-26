//! OS-entropy host primitive for the embedded V8 host (issue #2017,
//! epic #2012 — #1751 part 1 of 2).
//!
//! This is the **Rust primitive only**. Nothing in
//! `js/web_polyfills.js` calls [`op_zfb_random_bytes`] yet; sub-issue
//! #2018 builds `crypto.getRandomValues` / `crypto.randomUUID` on top
//! of it. There is deliberately no JS surface here.
//!
//! ## Why the op is SYNCHRONOUS (and why that is not a guardrail-1
//! violation)
//!
//! Guardrail 1 of the epic — "the isolate thread must never park on a
//! socket" — targets **network** I/O, and is what forces
//! [`super::fetch::op_zfb_fetch`] to be async. It does not apply here:
//!
//! - `crypto.getRandomValues` is **synchronous by specification**. It
//!   returns the filled view, not a promise. Making the host op async
//!   would not make the JS API async; it would only make it
//!   unimplementable.
//! - `getrandom` reads the kernel CSPRNG (`getrandom(2)` on Linux,
//!   `getentropy` on macOS/BSD, `BCryptGenRandom` on Windows). That is
//!   a syscall against an in-kernel generator: no network, no disk, and
//!   no blocking once the pool is initialised at boot.
//!
//! **Do not "fix" this into an async op.** Doing so breaks every caller
//! and buys nothing. [`op_is_sync`] pins the property mechanically off
//! deno_core's own `OpDecl` so the intent cannot be lost to a refactor.
//!
//! ## Why entropy is available in BOTH dispatch modes
//!
//! Unlike `fetch`, this capability is **not** mode-gated. The
//! build-time (SSG) denial that `web_polyfills.js` enforces is about
//! *network access*, not about randomness — and randomness whose
//! quality depends on which pipeline rendered the page is its own
//! footgun: a build-time render would silently get weaker bytes than
//! the request-time render of the same component. Build-time renders
//! get the same OS CSPRNG. The op therefore takes no
//! [`super::DispatchMode`] and consults none.
//!
//! ## Fail closed, always
//!
//! If the OS entropy source errors, [`fill_random_bytes`] propagates
//! the error. There is no fallback source, no retry against a weaker
//! generator, and no `Math.random`. Silently degrading to predictable
//! randomness is precisely bug #1751: a session ID or CSRF token that
//! looks fine locally and is guessable. Every functional test still
//! passes under such a regression — the bytes arrive, the API works —
//! so the fail-closed behaviour has its own explicit test
//! (`a_failing_entropy_source_errors_and_never_falls_back`).

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use deno_core::{op2, OpState};
use deno_error::JsErrorBox;

use super::limits;

/// [`EntropySource::kind`] of the production source. Tests assert this
/// on the source the *production* extension installs, so "the op is
/// wired to the OS CSPRNG" is checked against the real registration
/// rather than against an injected test double.
pub const OS_CSPRNG_KIND: &str = "os-csprng";

/// Number of times [`OsEntropy::fill`] has been entered, process-wide.
///
/// Instrumentation that ships in release builds on purpose: it is the
/// only way a test can prove the *OS* source — not some userspace PRNG
/// that happens to be installed under the same trait — actually ran.
/// The cost is one relaxed atomic increment per call, against a
/// syscall.
pub static OS_ENTROPY_CALLS: AtomicU64 = AtomicU64::new(0);

/// Where random bytes come from.
///
/// A trait, not a direct `getrandom` call, for exactly one reason: the
/// **failure** path must be testable. The OS CSPRNG cannot be made to
/// fail on demand, and a fail-closed guarantee that is never exercised
/// is not a guarantee.
pub trait EntropySource {
    /// Stable identifier for what this source reads from. Never derived
    /// from a type name — a test asserting `OS_CSPRNG_KIND` must fail
    /// if the production registration is swapped for something else.
    fn kind(&self) -> &'static str;

    /// Fill `dst` completely, or fail. A partial fill is a failure:
    /// implementations must not return `Ok` having written fewer bytes
    /// than `dst.len()`.
    fn fill(&self, dst: &mut [u8]) -> std::result::Result<(), String>;
}

/// The production source: the operating system's CSPRNG, via
/// `getrandom`.
///
/// Deliberately **not** `rand`'s thread-local userspace PRNG and not
/// any seeded generator — those are reproducible from their seed, which
/// is the property #1751 is about eliminating.
pub struct OsEntropy;

impl EntropySource for OsEntropy {
    fn kind(&self) -> &'static str {
        OS_CSPRNG_KIND
    }

    fn fill(&self, dst: &mut [u8]) -> std::result::Result<(), String> {
        OS_ENTROPY_CALLS.fetch_add(1, Ordering::Relaxed);
        getrandom::fill(dst).map_err(|e| e.to_string())
    }
}

/// Failure modes of [`fill_random_bytes`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EntropyError {
    /// Over the per-call byte quota. The message is the contract's
    /// (#2013) `crypto.getRandomValues` wording verbatim, so #2018's
    /// JS-side pre-check and this Rust ceiling report the *same* text
    /// and cannot drift into two different quotas.
    #[error("crypto.getRandomValues: requested {requested} bytes, quota is {limit} bytes")]
    QuotaExceeded { requested: usize, limit: usize },
    /// The OS CSPRNG errored, or the source is missing from `OpState`
    /// because the runtime is shutting down or was built without the
    /// extension.
    ///
    /// The message carries **no** `crypto.<method>:` prefix on purpose:
    /// this one op backs both `getRandomValues` and `randomUUID`, and
    /// the contract gives the latter `crypto.randomUUID: OS entropy
    /// unavailable: <detail>`. #2018 prefixes per API; prefixing here
    /// would produce a doubled or wrong method name.
    #[error("OS entropy unavailable: {detail}")]
    Unavailable { detail: String },
}

impl EntropyError {
    /// The JS error class this maps to. The contract gives the quota
    /// failure `name = "QuotaExceededError"` so `err.name` checks
    /// behave as they would against a real `DOMException`
    /// (divergence D4); an unavailable source is a plain `Error`.
    ///
    /// **#2018 must register `QuotaExceededError` in JS.** deno_core
    /// rebuilds an op's error through its own `buildCustomError`, which
    /// can only construct classes in its `errorMap`. A class it does
    /// not know arrives in JS as a thrown **`undefined`** — no name, no
    /// message, no quota diagnostic — as measured while writing
    /// `the_op_rejects_an_over_quota_buffer_at_the_v8_boundary`.
    /// `js/web_polyfills.js`'s `registerHostErrorClasses` already does
    /// this for the transport's `TimeoutError`; the same line is needed
    /// for `QuotaExceededError` when the Web Crypto surface lands.
    pub fn js_error_class(&self) -> &'static str {
        match self {
            EntropyError::QuotaExceeded { .. } => "QuotaExceededError",
            EntropyError::Unavailable { .. } => "Error",
        }
    }

    fn into_js_error_box(self) -> JsErrorBox {
        JsErrorBox::new(self.js_error_class(), self.to_string())
    }
}

/// Fill `dst` from `source`, enforcing the per-call quota.
///
/// The testable core: [`op_zfb_random_bytes`] is a thin wrapper that
/// resolves the source out of `OpState` and calls this.
///
/// The quota is measured on **`dst.len()`, i.e. bytes**, never on an
/// element count. #2018 applies the identical ceiling to a typed
/// array's `byteLength`, so a `Uint32Array(16_385)` (65_540 bytes,
/// 16_385 elements) is rejected by both layers rather than slipping
/// past one of them.
///
/// A zero-length `dst` is a valid no-op that **succeeds unconditionally**
/// — the contract's `crypto.getRandomValues` row: "a zero-length view is
/// a no-op that returns the view `[spec]`". It short-circuits *before*
/// the source is consulted, so `getRandomValues(new Uint8Array(0))`
/// returns its view even on a host whose CSPRNG is unavailable.
///
/// That is not a hole in fail-closed: fail-closed exists to stop weak
/// bytes reaching a caller, and zero bytes cannot be weak. Every request
/// for one byte or more still fails closed.
pub fn fill_random_bytes(
    source: &dyn EntropySource,
    dst: &mut [u8],
) -> std::result::Result<(), EntropyError> {
    if dst.is_empty() {
        return Ok(());
    }
    if dst.len() > limits::MAX_RANDOM_BYTES_PER_CALL {
        return Err(EntropyError::QuotaExceeded {
            requested: dst.len(),
            limit: limits::MAX_RANDOM_BYTES_PER_CALL,
        });
    }
    // FAIL CLOSED. This `?` is the security property of the whole
    // sub-issue: there is no `unwrap_or_else` fallback here, and adding
    // one — a zero fill, a seeded PRNG, anything — would let a caller
    // receive predictable "random" bytes with no error and no symptom.
    source
        .fill(dst)
        .map_err(|detail| EntropyError::Unavailable { detail })
}

/// The entropy source parked in `OpState` at extension init.
pub struct HostEntropySource(pub Rc<dyn EntropySource>);

/// Fill a caller-provided buffer from the OS CSPRNG.
///
/// **Synchronous on purpose** — see this module's header. `#[op2(fast)]`
/// is the synchronous op form; the buffer is written in place through
/// V8's backing store, so no copy crosses the boundary.
#[op2(fast)]
pub fn op_zfb_random_bytes(
    state: &mut OpState,
    #[buffer] out: &mut [u8],
) -> std::result::Result<(), JsErrorBox> {
    // The zero-length no-op succeeds before the source is even looked
    // up, so it cannot fail on a host missing the extension either —
    // see [`fill_random_bytes`] for why this is not a fail-closed hole.
    if out.is_empty() {
        return Ok(());
    }
    let source = state
        .try_borrow::<HostEntropySource>()
        .ok_or_else(|| EntropyError::Unavailable {
            detail: "OS entropy source is not installed in this runtime".to_string(),
        })
        .map_err(EntropyError::into_js_error_box)?
        .0
        .clone();
    fill_random_bytes(source.as_ref(), out).map_err(EntropyError::into_js_error_box)
}

deno_core::extension!(
    zfb_crypto,
    ops = [op_zfb_random_bytes],
    state = |state| {
        state.put(HostEntropySource(Rc::new(OsEntropy)));
    },
);

/// Whether [`op_zfb_random_bytes`] is registered as a **synchronous**
/// op.
///
/// Read straight off deno_core's own `OpDecl`, so the "deliberately
/// synchronous" decision documented above cannot be quietly reversed:
/// an async op here would hand `crypto.getRandomValues` a promise it
/// cannot await.
pub fn op_is_sync() -> bool {
    // `#[op2]` rewrites the annotated fn into a `const fn() -> OpDecl`,
    // so calling it yields the declaration deno_core itself registers.
    !op_zfb_random_bytes().is_async
}

#[cfg(test)]
mod tests;
