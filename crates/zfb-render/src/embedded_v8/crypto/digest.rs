//! SHA digest host primitive behind `crypto.subtle.digest` (issue
//! #2018, epic #2012 — #1751 part 2 of 2).
//!
//! ## Why this is a Rust op and not hand-rolled JS
//!
//! The pre-#2018 polyfill's own comment proposed "a pure-JS sha256
//! implementation (~80 lines)" as the fix. That is the wrong trade:
//! `digest` is what an application's ETag, content hash, cache key and
//! request signature are built from, and a subtly wrong hand-written
//! SHA would emit *plausible-looking* digests that silently disagree
//! with production Workers — the same shape of local/production
//! divergence #1751 exists to remove. `sha1` / `sha2` are the
//! implementations eight sibling crates in this workspace already use.
//!
//! ## Why the op is SYNCHRONOUS
//!
//! Same reasoning as [`super::op_zfb_random_bytes`]: guardrail 1 of the
//! epic targets **network** I/O. Hashing is CPU-bound over a buffer the
//! caller already holds in memory — there is no socket to park on. The
//! JS side wraps the synchronous result in an already-resolved promise
//! so `crypto.subtle.digest` still returns the `Promise<ArrayBuffer>`
//! the WebCrypto spec requires. [`digest_op_is_sync`] pins the property
//! mechanically off deno_core's own `OpDecl`.
//!
//! ## What is deliberately NOT here
//!
//! `MD5` — production workerd supports it as a documented legacy
//! extension, this host fails it closed through the unsupported-
//! algorithm path. That is contract divergence **D7**
//! (`research/2013-request-time-capability-contract.md`), not an
//! oversight, and the rejection message names the supported set so the
//! divergence is visible at the call site rather than in a doc.
//!
//! Every key-bearing SubtleCrypto operation (`sign`, `encrypt`,
//! `importKey`, …) is divergence **D8** and has no Rust surface at all
//! — those fail closed in `js/web_polyfills.js` with a message that
//! states production DOES implement them.

use deno_core::op2;
use deno_error::JsErrorBox;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};

/// The digest algorithms this host implements, in the spelling
/// WebCrypto uses. **The single source of truth**: the rejection
/// message below is rendered from this slice rather than repeating the
/// names, so adding an algorithm cannot leave the diagnostic claiming
/// otherwise.
pub const SUPPORTED_DIGEST_ALGORITHMS: &[&str] = &["SHA-1", "SHA-256", "SHA-384", "SHA-512"];

/// Render [`SUPPORTED_DIGEST_ALGORITHMS`] for the rejection message.
fn supported_algorithms_list() -> String {
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

    fn into_js_error_box(self) -> JsErrorBox {
        JsErrorBox::new(self.js_error_class(), self.to_string())
    }
}

/// Hash `data` with `algorithm`, or fail closed.
///
/// `algorithm` is matched case-insensitively after trimming, because
/// WebCrypto normalises the algorithm name that way and callers write
/// `"sha-256"` as often as `"SHA-256"`. The **original** spelling is
/// what the rejection quotes back, so a typo is recognisable in the
/// message rather than being echoed uppercased.
///
/// There is no default algorithm and no "closest match" fallback: an
/// unrecognised name is an error, never a silent substitution.
pub fn digest_bytes(algorithm: &str, data: &[u8]) -> std::result::Result<Vec<u8>, DigestError> {
    match algorithm.trim().to_ascii_uppercase().as_str() {
        "SHA-1" => Ok(Sha1::digest(data).to_vec()),
        "SHA-256" => Ok(Sha256::digest(data).to_vec()),
        "SHA-384" => Ok(Sha384::digest(data).to_vec()),
        "SHA-512" => Ok(Sha512::digest(data).to_vec()),
        _ => Err(DigestError::UnsupportedAlgorithm {
            requested: algorithm.to_string(),
        }),
    }
}

/// Hash a caller-provided buffer.
///
/// **Synchronous on purpose** — see this module's header.
#[op2]
#[buffer]
pub fn op_zfb_digest(
    #[string] algorithm: String,
    #[buffer] data: &[u8],
) -> std::result::Result<Vec<u8>, JsErrorBox> {
    digest_bytes(&algorithm, data).map_err(DigestError::into_js_error_box)
}

/// Whether [`op_zfb_digest`] is registered as a **synchronous** op.
///
/// Read straight off deno_core's own `OpDecl` so the deliberate choice
/// documented above cannot be quietly reversed: an async op here would
/// hand the JS side a promise where it expects bytes, and the
/// already-resolved-promise wrapper would silently start resolving to a
/// promise-of-a-promise.
pub fn digest_op_is_sync() -> bool {
    !op_zfb_digest().is_async
}

#[cfg(test)]
mod tests {
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

    /// WebCrypto normalises the algorithm name case-insensitively.
    #[test]
    fn the_algorithm_name_is_matched_case_insensitively_after_trimming() {
        let canonical = digest_bytes("SHA-256", b"abc").unwrap();
        assert_eq!(digest_bytes("sha-256", b"abc").unwrap(), canonical);
        assert_eq!(digest_bytes("Sha-256", b"abc").unwrap(), canonical);
        assert_eq!(digest_bytes("  SHA-256  ", b"abc").unwrap(), canonical);
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

    #[test]
    fn the_digest_op_is_registered_as_a_synchronous_op() {
        assert!(digest_op_is_sync());
    }

    /// Driven through a real V8 isolate built from the production
    /// `build_extensions()`, so "the op is registered and reachable
    /// under the name the polyfill calls" is checked against the real
    /// registration rather than against this module's own function.
    #[test]
    fn the_op_is_reachable_from_a_real_isolate_and_rejects_md5_with_its_class() {
        let mut runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: crate::embedded_v8::build_extensions(),
            ..Default::default()
        });
        let result = runtime
            .execute_script(
                "zfb:digest_op_probe",
                r#"
                class NotSupportedError extends Error {
                  constructor(message) {
                    super(message);
                    this.name = "NotSupportedError";
                  }
                }
                Deno.core.registerErrorClass("NotSupportedError", NotSupportedError);
                (() => {
                  // No `TextEncoder` here: this is a bare deno_core
                  // runtime, without `web_polyfills.js`.
                  const abc = Uint8Array.from([0x61, 0x62, 0x63]);
                  const out = Deno.core.ops.op_zfb_digest("SHA-256", abc);
                  const hex = Array.from(out).map((b) => b.toString(16).padStart(2, "0")).join("");
                  let rejected;
                  try {
                    Deno.core.ops.op_zfb_digest("MD5", abc);
                    rejected = "NO-THROW";
                  } catch (e) {
                    rejected = `${e.name}|${e.message}`;
                  }
                  return hex + "\n" + rejected;
                })()
                "#,
            )
            .expect("the probe must evaluate");
        deno_core::scope!(scope, &mut runtime);
        let value = deno_core::v8::Local::new(scope, result);
        assert_eq!(
            value.to_rust_string_lossy(scope),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n\
             NotSupportedError|crypto.subtle.digest: unsupported algorithm \"MD5\". \
             This host implements SHA-1, SHA-256, SHA-384, SHA-512."
        );
    }
}
