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

#[cfg(test)]
mod tests {
    //! Known-answer and boundary tests moved from
    //! `zfb-render/src/embedded_v8/crypto/digest.rs` for #4393 (S6a); the two
    //! Deno `op_zfb_digest` registration cases stay with the Deno host.
    use super::*;

    /// Hex, lowercase — the form every published test vector is written
    /// in.
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Published NIST / RFC vectors for `"abc"`. Known-answer tests, not
    /// round-trips: a round-trip against our own implementation would
    /// pass just as happily if the implementation were wrong.
    #[test]
    fn the_four_supported_algorithms_match_their_published_vectors() {
        assert_eq!(
            hex(&digest_bytes("SHA-1", b"abc").unwrap()),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&digest_bytes("SHA-256", b"abc").unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&digest_bytes("SHA-384", b"abc").unwrap()),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
             8086072ba1e7cc2358baeca134c825a7"
        );
        assert_eq!(
            hex(&digest_bytes("SHA-512", b"abc").unwrap()),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
    }

    /// The empty input is the case a length-handling bug most often
    /// survives — the padding block is the whole message.
    #[test]
    fn the_empty_input_hashes_to_its_published_vector() {
        assert_eq!(
            hex(&digest_bytes("SHA-256", b"").unwrap()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// WebCrypto's "normalize an algorithm" step is ASCII-case-folding
    /// and nothing else.
    #[test]
    fn the_algorithm_name_is_matched_case_insensitively_and_not_trimmed() {
        let canonical = digest_bytes("SHA-256", b"abc").unwrap();
        assert_eq!(digest_bytes("sha-256", b"abc").unwrap(), canonical);
        assert_eq!(digest_bytes("Sha-256", b"abc").unwrap(), canonical);
        // Whitespace-padded names are NOT registered algorithm names.
        // Production Workers rejects them; trimming here would let a
        // bundle that only works locally ship — the exact divergence
        // class #1751 is about.
        for padded in [" SHA-256", "SHA-256 ", "  SHA-256  ", "\tSHA-256"] {
            let err = digest_bytes(padded, b"abc")
                .expect_err("a whitespace-padded algorithm name is not registered");
            assert_eq!(
                err,
                DigestError::UnsupportedAlgorithm {
                    requested: padded.to_string(),
                }
            );
        }
    }

    /// Divergence **D7**: workerd implements MD5 as a legacy extension;
    /// this host fails it closed. The message is asserted whole — a
    /// reworded rejection that no longer names the supported set would
    /// leave a developer with nowhere to go.
    #[test]
    fn md5_is_rejected_and_the_message_names_the_supported_set() {
        let err = digest_bytes("MD5", b"abc").expect_err("MD5 is divergence D7");
        assert_eq!(
            err.to_string(),
            "crypto.subtle.digest: unsupported algorithm \"MD5\". \
             This host implements SHA-1, SHA-256, SHA-384, SHA-512."
        );
        assert_eq!(err.js_error_class(), "NotSupportedError");
    }

    /// The rejection quotes the caller's ORIGINAL spelling, so a typo is
    /// recognisable rather than echoed back uppercased.
    #[test]
    fn the_rejection_quotes_the_callers_own_spelling() {
        let err = digest_bytes("sha-257", b"").expect_err("not an algorithm");
        assert!(
            err.to_string()
                .contains("unsupported algorithm \"sha-257\""),
            "the message must quote the caller's spelling: {err}"
        );
    }

    /// The advertised list is rendered from the implemented set, so the
    /// two cannot drift.
    #[test]
    fn every_advertised_algorithm_is_actually_implemented() {
        for algorithm in SUPPORTED_DIGEST_ALGORITHMS {
            assert!(
                digest_bytes(algorithm, b"abc").is_ok(),
                "{algorithm} is advertised in the rejection message but not implemented"
            );
        }
        let advertised = supported_algorithms_list();
        for algorithm in SUPPORTED_DIGEST_ALGORITHMS {
            assert!(advertised.contains(algorithm));
        }
    }
}
