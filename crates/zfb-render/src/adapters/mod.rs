//! Framework adapters.
//!
//! zfb supports two JSX frameworks: Preact (default) and React. The
//! choice is made once at config-load time via the `framework` field in
//! `zfb.config.ts` and is centralized through SWC's `transform-react`
//! configuration — *not* per-file pragmas. See
//! `docs/architecture/adr-002-framework-adapters.md` for the contract a
//! portable component must follow and for the gotchas that arise from
//! this two-track design.
//!
//! An [`Adapter`] is responsible for three things and three things only:
//!
//! 1. Telling the SWC pipeline which JSX import source to inject
//!    (see [`Adapter::jsx_import_source`]).
//! 2. Telling the JS runtime which module exposes the synchronous
//!    render-to-string entry point
//!    (see [`Adapter::render_to_string_module`]).
//! 3. Installing a tiny pre-render shim into `globalThis` so that
//!    `render.rs` can call `__zfbRenderToString(vnode)` without caring
//!    which framework is active
//!    (see [`Adapter::pre_render_setup`]).
//!
//! Anything beyond that — hook semantics, signal interop, hydration
//! strategy — is intentionally out of scope. ADR-002 documents why.

use crate::{RenderError, RenderHost};

pub mod preact;
pub mod react;

pub use preact::PreactAdapter;
pub use react::ReactAdapter;

/// Which framework to render with. Selected once at config-load time.
///
/// Serde accepts the canonical lowercase form (`"preact"` / `"react"`)
/// and the matching aliases so `zfb.config.ts` can spell the value
/// either way without surprise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Framework {
    #[default]
    #[serde(alias = "preact")]
    Preact,
    #[serde(alias = "react")]
    React,
}

/// The portable adapter contract.
///
/// All four methods are pure (`name`, `jsx_import_source`,
/// `render_to_string_module`) or side-effecting only on the host
/// (`pre_render_setup`). Adapters MUST NOT carry per-render state — a
/// single adapter instance is reused across every page render.
pub trait Adapter {
    /// Human-readable adapter name. Stable, lowercase, no whitespace.
    /// Used in error messages and in build logs.
    fn name(&self) -> &'static str;

    /// JSX import source to feed into SWC's `transform-react`
    /// `importSource` option. Drives both the JSX factory module and
    /// the automatic-runtime `jsx`/`jsxs` imports.
    fn jsx_import_source(&self) -> &'static str;

    /// Module specifier the JS runtime should resolve to obtain the
    /// synchronous render-to-string entry. The runtime resolver maps
    /// this specifier to an actual module load.
    fn render_to_string_module(&self) -> &'static str;

    /// Run once, before the first page render, on the embedded JS
    /// runtime. Installs `globalThis.__zfbRenderToString = ...` so the
    /// orchestrator in `render.rs` can call into the framework
    /// uniformly.
    fn pre_render_setup(&self, host: &mut dyn RenderHost) -> Result<(), RenderError>;
}

/// Construct the boxed adapter for a given [`Framework`].
///
/// This is the single dispatch point used by the rest of the crate;
/// callers should never instantiate adapters directly.
pub fn make_adapter(framework: Framework) -> Box<dyn Adapter> {
    match framework {
        Framework::Preact => Box::new(PreactAdapter),
        Framework::React => Box::new(ReactAdapter),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_default_is_preact() {
        assert_eq!(Framework::default(), Framework::Preact);
    }

    #[test]
    fn framework_deserializes_lowercase() {
        let f: Framework = serde_json::from_str("\"preact\"").unwrap();
        assert_eq!(f, Framework::Preact);
        let f: Framework = serde_json::from_str("\"react\"").unwrap();
        assert_eq!(f, Framework::React);
    }

    #[test]
    fn make_adapter_returns_correct_name() {
        assert_eq!(make_adapter(Framework::Preact).name(), "preact");
        assert_eq!(make_adapter(Framework::React).name(), "react");
    }

    #[test]
    fn jsx_import_sources_match_framework() {
        assert_eq!(
            make_adapter(Framework::Preact).jsx_import_source(),
            "preact"
        );
        assert_eq!(make_adapter(Framework::React).jsx_import_source(), "react");
    }

    #[test]
    fn render_to_string_modules_match_framework() {
        assert_eq!(
            make_adapter(Framework::Preact).render_to_string_module(),
            "preact-render-to-string"
        );
        assert_eq!(
            make_adapter(Framework::React).render_to_string_module(),
            "react-dom/server"
        );
    }
}
