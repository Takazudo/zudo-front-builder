//! Canonical SHA algorithms and diagnostics.
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};
/// The digest algorithms this host implements, in the spelling
/// WebCrypto uses. **The single source of truth**: the rejection
/// message below is rendered from this slice rather than repeating the
/// names, so adding an algorithm cannot leave the diagnostic claiming
/// otherwise.
pub const SUPPORTED_DIGEST_ALGORITHMS: &[&str] = &["SHA-1", "SHA-256", "SHA-384", "SHA-512"];

/// Render [`SUPPORTED_DIGEST_ALGORITHMS`] for the rejection message.
pub fn supported_algorithms_list() -> String {
    SUPPORTED_DIGEST_ALGORITHMS.join(", ")
}

/// The one way [`digest_bytes`] can fail.
///
/// A hash over an in-memory buffer has no I/O, no allocation ceiling
/// worth policing and no cancellation surface, so "I do not implement
/// that algorithm" is the whole failure space.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DigestError {
    /// The contract's (#2013) `crypto.subtle.digest` wording verbatim.
    /// It is produced HERE rather than in JS so the algorithm list and
    /// the message that advertises it cannot drift apart.
    #[error(
        "crypto.subtle.digest: unsupported algorithm \"{requested}\". \
         This host implements {}.",
        supported_algorithms_list()
    )]
    UnsupportedAlgorithm { requested: String },
}

impl DigestError {
    /// The JS error class this maps to.
    ///
    /// **`js/web_polyfills.js` must register `NotSupportedError`** with
    /// `Deno.core.registerErrorClass`. deno_core rebuilds an op's error
    /// through its own `buildCustomError`, which can only construct
    /// classes in its `errorMap`; an unregistered class arrives in JS as
    /// a thrown **`undefined`** — no name, no message, no diagnostic at
    /// all. Wave 5 measured exactly that for `QuotaExceededError`.
    pub fn js_error_class(&self) -> &'static str {
        match self {
            DigestError::UnsupportedAlgorithm { .. } => "NotSupportedError",
        }
    }
}

/// Hash `data` with `algorithm`, or fail closed.
///
/// `algorithm` is matched **ASCII-case-insensitively and nothing
/// more** — that is exactly WebCrypto's "normalize an algorithm" step,
/// and callers write `"sha-256"` as often as `"SHA-256"`. Surrounding
/// whitespace is deliberately NOT trimmed: `" SHA-256 "` is not a
/// registered algorithm name, production Workers rejects it, and
/// accepting it here would manufacture the precise local/production
/// divergence this sub-issue exists to remove. The **original**
/// spelling is what the rejection quotes back, so a typo is
/// recognisable in the message rather than being echoed uppercased.
///
/// There is no default algorithm and no "closest match" fallback: an
/// unrecognised name is an error, never a silent substitution.
pub fn digest_bytes(algorithm: &str, data: &[u8]) -> std::result::Result<Vec<u8>, DigestError> {
    match algorithm.to_ascii_uppercase().as_str() {
        "SHA-1" => Ok(Sha1::digest(data).to_vec()),
        "SHA-256" => Ok(Sha256::digest(data).to_vec()),
        "SHA-384" => Ok(Sha384::digest(data).to_vec()),
        "SHA-512" => Ok(Sha512::digest(data).to_vec()),
        _ => Err(DigestError::UnsupportedAlgorithm {
            requested: algorithm.to_string(),
        }),
    }
}
