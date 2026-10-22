//! Canonical entropy and digest primitives. Engine adapters own error conversion.
use crate::limits;
use std::sync::atomic::{AtomicU64, Ordering};
pub mod digest;
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
        map_os_entropy_result(getrandom::fill(dst))
    }
}

/// Map a `getrandom` result onto [`EntropySource::fill`]'s contract.
///
/// Extracted from [`OsEntropy::fill`] so the PRODUCTION error path is
/// itself testable. The trait above exists so the failure path can be
/// driven through a double — but every fail-closed test then runs
/// against that double, and none of them touch this mapping. Verified
/// during the epic review: rewriting `OsEntropy::fill` to swallow the
/// error and return `Ok(())` failed **no** test, which is precisely the
/// #1751 silent-degradation bug reintroduced in the one place no test
/// was looking. `map_os_entropy_result` is that missing seam.
pub fn map_os_entropy_result(
    result: std::result::Result<(), getrandom::Error>,
) -> std::result::Result<(), String> {
    result.map_err(|e| e.to_string())
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
}

/// Fill `dst` from `source`, enforcing the per-call quota.
///
/// The testable core: `op_zfb_random_bytes` is a thin wrapper that
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

#[cfg(test)]
mod tests;
