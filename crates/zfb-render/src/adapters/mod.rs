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
//! An [`Adapter`] is responsible for four things and four things only:
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
//! 4. Exposing a tiny client-side **hydration shim** that the islands
//!    bundler folds into the islands bundle's entry. The shim exports a
//!    single `hydrateIsland(Component, props, element)` function so the
//!    framework-agnostic hydration runtime (`zfb-islands` JS, Sub 3) can
//!    hydrate any island without branching on the framework
//!    (see [`Adapter::hydrate_shim_specifier`] and
//!    [`Adapter::hydrate_shim_source`]).
//!
//! ## Hydration: per-adapter shim, not per-call JS expression
//!
//! For Sub 3 we considered two designs:
//!
//! - **`hydrate_call()` returning a JS expression string** that the
//!   runtime would template into a per-page generated module.
//! - **A per-adapter shim module** that the islands bundler includes as
//!   the islands-bundle entry; the hydration runtime imports a uniform
//!   `hydrateIsland(Component, props, element)`.
//!
//! We pick the shim. It keeps the Rust ↔ JS boundary expressed purely
//! as static strings (no JS-expression concatenation in Rust, no `eval`
//! in the runtime), it keeps tree-shaking honest because the shim is a
//! real module the bundler sees, and it leaves the hydration runtime
//! adapter-agnostic — the same JS file ships whether the project picked
//! Preact or React.
//!
//! Anything beyond these four hooks — hook semantics, signal interop,
//! event delegation strategy — is intentionally out of scope. ADR-002
//! documents why.

use async_trait::async_trait;

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
/// Methods are pure (`name`, `jsx_import_source`,
/// `render_to_string_module`, `hydrate_shim_specifier`,
/// `hydrate_shim_source`) or side-effecting only on the host
/// (`pre_render_setup`). Adapters MUST NOT carry per-render state — a
/// single adapter instance is reused across every page render.
///
/// `pre_render_setup` is `async` so it can drive the now-async
/// [`RenderHost::execute_module`] without blocking.
#[async_trait(?Send)]
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
    async fn pre_render_setup(&self, host: &mut dyn RenderHost) -> Result<(), RenderError>;

    /// Synthetic module specifier the islands bundler uses to write the
    /// hydration shim into the bundle. Conventionally lives under the
    /// `zfb:internal/adapters/` namespace so it cannot collide with a
    /// user-authored module. The bundler is free to substitute its own
    /// path; this value exists primarily to give the bundler a stable
    /// default and to give the shim source a recognisable display name.
    fn hydrate_shim_specifier(&self) -> &'static str;

    /// JS module source the islands bundler folds into the islands
    /// bundle as the framework-specific hydration entry.
    ///
    /// Contract: the module MUST export a function named `hydrateIsland`
    /// with signature
    /// `hydrateIsland(Component, props, element)` and MUST hydrate
    /// `Component` against `element` using `props`. The hydration
    /// runtime in `zfb-islands` calls this function for every
    /// `[data-zfb-island]` element in the DOM, so the function MUST be
    /// safe to call repeatedly with different elements.
    fn hydrate_shim_source(&self) -> &'static str;
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

    #[test]
    fn hydrate_shim_sources_export_hydrate_island() {
        // Both adapters must expose a `hydrateIsland` export. The
        // hydration runtime imports this name, not the framework's
        // native API, so a typo here would silently break every page.
        for adapter in [
            make_adapter(Framework::Preact),
            make_adapter(Framework::React),
        ] {
            let src = adapter.hydrate_shim_source();
            assert!(
                src.contains("hydrateIsland"),
                "{} shim does not export hydrateIsland: {src}",
                adapter.name()
            );
        }
    }

    #[test]
    fn hydrate_shim_specifiers_are_internal_namespace() {
        // Specifiers must live under zfb:internal/ so they can never
        // collide with a user-authored module path.
        for adapter in [
            make_adapter(Framework::Preact),
            make_adapter(Framework::React),
        ] {
            assert!(
                adapter
                    .hydrate_shim_specifier()
                    .starts_with("zfb:internal/"),
                "{} specifier escaped zfb:internal/",
                adapter.name()
            );
        }
    }
}
