//! zfb-render: SWC TSX compile, JS runtime host, framework adapters.
//!
//! NOTE (Sub 4 stub): This `lib.rs` is a minimal stub authored by Sub 4
//! (adapters layer) so that the adapter modules under `src/adapters/`
//! compile in isolation. Sub 3 (topic-sub03-render) owns the authoritative
//! version of this file — at merge time the manager will reconcile this
//! stub with Sub 3's `RenderHost` trait, `swc_pipeline`, `loader`, and
//! `render` modules.
//!
//! The shapes defined here (`RenderHost`, `RenderError`) are placeholders
//! whose surface is intentionally narrow so adapter code does not bake in
//! assumptions that conflict with Sub 3's design. The merge-time owner
//! should replace these with Sub 3's definitions; the adapter trait in
//! [`adapters`] only depends on the items re-exported from this module.

pub mod adapters;

/// Placeholder error type.
///
/// Sub 3 owns the authoritative definition. Sub 4 only emits adapter setup
/// errors via [`RenderError::Adapter`]; the merge-time owner can preserve
/// or migrate that variant as appropriate.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("adapter error: {0}")]
    Adapter(String),
}

/// Placeholder render-host trait.
///
/// Sub 3 owns the authoritative definition. Sub 4's [`adapters::Adapter`]
/// trait only depends on the [`RenderHost::eval_module`] method signature
/// shown here — it executes a JS source snippet inside the embedded
/// runtime under a synthetic module specifier so adapter setup shims
/// (e.g. installing `globalThis.__zfbRenderToString`) can run before the
/// first page render.
pub trait RenderHost {
    /// Evaluate `source` as an ES module under `specifier`. Used by
    /// adapters to install render-to-string shims into `globalThis`
    /// before any page is rendered.
    fn eval_module(&mut self, specifier: &str, source: &str) -> Result<(), RenderError>;
}
