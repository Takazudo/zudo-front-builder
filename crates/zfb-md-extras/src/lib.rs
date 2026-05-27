//! `zfb-md-extras` — remark/rehype plugin ports (GFM, footnotes, etc.).
//!
//! This crate skeleton was bootstrapped by Wave 1 (#567). The actual
//! markdown pipeline and visitor traits land in Wave 2 (#569).

/// Test harness module — `run_fixture` and helpers for fixture-based
/// snapshot tests. Gated behind `cfg(any(test, feature = "test-utils"))` so
/// it is never compiled into production builds.
///
/// Wave 2+ feature crates enable this via:
///   `zfb-md-extras = { path = "...", features = ["test-utils"] }`
#[cfg(any(test, feature = "test-utils"))]
pub mod test_harness;
