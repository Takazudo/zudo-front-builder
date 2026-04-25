//! React framework adapter — STUB.
//!
//! Sub 4 fills this in. Expected responsibilities:
//! - Provide the `react/jsx-runtime` module to the JS runtime's loader.
//! - Provide `react-dom/server` (`renderToString`).
//! - Wire `Renderer` to invoke `renderToString(default(props))`.

/// Identifier returned by `name()`.
pub const NAME: &str = "react";

/// Adapter name. Sub 4 will likely replace this with a richer trait
/// implementation.
pub fn name() -> &'static str {
    NAME
}
