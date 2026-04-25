//! Preact framework adapter — STUB.
//!
//! Sub 4 fills this in. Expected responsibilities:
//! - Provide the `preact/jsx-runtime` module to the JS runtime's loader.
//! - Provide `preact-render-to-string`.
//! - Wire `Renderer` to invoke `renderToString(default(props))`.
//!
//! The minimal placeholder here keeps the crate compiling end-to-end while
//! the adapter is being built in a sibling worktree.

/// Identifier returned by `name()` — used by the framework selector.
pub const NAME: &str = "preact";

/// Adapter name. Sub 4 will likely replace this with a richer trait
/// implementation.
pub fn name() -> &'static str {
    NAME
}
