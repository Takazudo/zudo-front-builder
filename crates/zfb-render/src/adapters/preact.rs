//! Preact adapter — the default zfb framework.
//!
//! Maps the [`Adapter`](super::Adapter) contract onto the Preact
//! ecosystem:
//!
//! - JSX import source: `"preact"` (drives SWC's automatic JSX runtime,
//!   producing `import { jsx, jsxs } from "preact/jsx-runtime"`).
//! - Render-to-string module: `"preact-render-to-string"`.
//! - Pre-render setup: installs `globalThis.__zfbRenderToString` so the
//!   orchestrator can call into the framework without branching on it.

use async_trait::async_trait;

use crate::{RenderError, RenderHost};

use super::Adapter;

/// Preact framework adapter.
///
/// Stateless — a single instance is reused across all page renders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreactAdapter;

/// Synthetic module specifier under which the pre-render shim is
/// evaluated. Must not collide with any user-authored module path.
const PREACT_SETUP_SPECIFIER: &str = "zfb:internal/adapters/preact-setup.mjs";

/// Pre-render setup shim source. Imports the synchronous
/// render-to-string entry from `preact-render-to-string` and exposes it
/// at `globalThis.__zfbRenderToString` so `render.rs` can call it
/// uniformly across adapters.
const PREACT_SETUP_SOURCE: &str = r#"import { renderToString } from "preact-render-to-string";
globalThis.__zfbRenderToString = renderToString;
"#;

/// Synthetic module specifier under which the islands bundler folds the
/// per-adapter hydration shim into the islands bundle entry.
const PREACT_HYDRATE_SHIM_SPECIFIER: &str = "zfb:internal/adapters/preact-hydrate.mjs";

/// Per-adapter hydration shim. Exports the uniform
/// `hydrateIsland(Component, props, element)` the framework-agnostic
/// hydration runtime in `zfb-islands` calls to drive Preact. Uses `h` +
/// `hydrate` from bare `preact` (not `preact/compat`) because zfb's
/// portable-component contract targets bare Preact.
const PREACT_HYDRATE_SHIM_SOURCE: &str = r#"import { h, hydrate } from "preact";
export function hydrateIsland(Component, props, element) {
  hydrate(h(Component, props), element);
}
"#;

#[async_trait(?Send)]
impl Adapter for PreactAdapter {
    fn name(&self) -> &'static str {
        "preact"
    }

    fn jsx_import_source(&self) -> &'static str {
        "preact"
    }

    fn render_to_string_module(&self) -> &'static str {
        "preact-render-to-string"
    }

    async fn pre_render_setup(&self, host: &mut dyn RenderHost) -> Result<(), RenderError> {
        host.execute_module(PREACT_SETUP_SPECIFIER, PREACT_SETUP_SOURCE)
            .await
            .map(|_handle| ())
            .map_err(|e| RenderError::Adapter(format!("preact pre-render setup failed: {e}")))
    }

    fn hydrate_shim_specifier(&self) -> &'static str {
        PREACT_HYDRATE_SHIM_SPECIFIER
    }

    fn hydrate_shim_source(&self) -> &'static str {
        PREACT_HYDRATE_SHIM_SOURCE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::render_host::ModuleHandle;
    use serde_json::Value as JsonValue;

    /// In-memory `RenderHost` that records every module evaluation it
    /// receives. Lets us assert what shim source was installed without
    /// pulling in a real JS engine.
    #[derive(Default)]
    struct CapturingHost {
        calls: Vec<(String, String)>,
        fail_with: Option<String>,
    }

    #[async_trait(?Send)]
    impl RenderHost for CapturingHost {
        async fn execute_module(
            &mut self,
            name: &str,
            source: &str,
        ) -> Result<ModuleHandle, RenderError> {
            self.calls.push((name.to_owned(), source.to_owned()));
            if let Some(msg) = &self.fail_with {
                return Err(RenderError::Adapter(msg.clone()));
            }
            Ok(ModuleHandle::new(self.calls.len() as u32, name))
        }

        async fn call_default(
            &mut self,
            _handle: &ModuleHandle,
            _props: JsonValue,
        ) -> Result<String, RenderError> {
            Ok(String::new())
        }

        async fn get_export(
            &mut self,
            _handle: &ModuleHandle,
            _name: &str,
        ) -> Result<JsonValue, RenderError> {
            Ok(JsonValue::Null)
        }
    }

    #[test]
    fn name_jsx_source_and_render_module_are_stable() {
        let a = PreactAdapter;
        assert_eq!(a.name(), "preact");
        assert_eq!(a.jsx_import_source(), "preact");
        assert_eq!(a.render_to_string_module(), "preact-render-to-string");
    }

    #[tokio::test]
    async fn pre_render_setup_evaluates_shim_module() {
        let mut host = CapturingHost::default();
        PreactAdapter.pre_render_setup(&mut host).await.unwrap();
        assert_eq!(host.calls.len(), 1);
        let (specifier, source) = &host.calls[0];
        assert_eq!(specifier, PREACT_SETUP_SPECIFIER);
        assert!(source.contains("preact-render-to-string"));
        assert!(source.contains("__zfbRenderToString"));
    }

    #[test]
    fn hydrate_shim_imports_bare_preact_and_exports_hydrate_island() {
        let a = PreactAdapter;
        let src = a.hydrate_shim_source();
        // Imports `h` and `hydrate` from bare `preact`. Importing from
        // `preact/compat` would silently pull in React-shim semantics
        // we explicitly do not want.
        assert!(src.contains(r#"from "preact""#));
        assert!(!src.contains("preact/compat"));
        assert!(src.contains("hydrateIsland"));
        // Specifier lives under zfb:internal/ so user code cannot collide.
        assert_eq!(a.hydrate_shim_specifier(), PREACT_HYDRATE_SHIM_SPECIFIER);
        assert!(a.hydrate_shim_specifier().starts_with("zfb:internal/"));
    }

    #[tokio::test]
    async fn pre_render_setup_wraps_host_errors_with_adapter_context() {
        let mut host = CapturingHost {
            fail_with: Some("oops".into()),
            ..Default::default()
        };
        let err = PreactAdapter.pre_render_setup(&mut host).await.unwrap_err();
        let msg = match err {
            RenderError::Adapter(m) => m,
            other => unreachable!("expected RenderError::Adapter, got {other:?}"),
        };
        assert!(msg.contains("preact"));
        assert!(msg.contains("oops"));
    }
}
