//! Per-candidate `RenderHost` implementations. Each is gated behind a feature
//! flag so the heavy V8-backed crates only compile when explicitly requested.

#[cfg(feature = "quickjs")]
pub mod quickjs;

#[cfg(feature = "deno")]
pub mod deno_core;

#[cfg(feature = "ssr-rs")]
pub mod ssr_rs;
