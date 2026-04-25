//! React adapter.
//!
//! Maps the [`Adapter`](super::Adapter) contract onto the React
//! ecosystem:
//!
//! - JSX import source: `"react"` (drives SWC's automatic JSX runtime,
//!   producing `import { jsx, jsxs } from "react/jsx-runtime"`).
//! - Render-to-string module: `"react-dom/server"`. We deliberately use
//!   the synchronous `renderToString` export rather than the streaming
//!   `renderToReadableStream` / `renderToPipeableStream` variants —
//!   zfb's output is static HTML produced ahead of time, so a sync
//!   string is simpler and avoids dragging streaming primitives into
//!   the build pipeline.
//! - Pre-render setup: installs `globalThis.__zfbRenderToString` so the
//!   orchestrator can call into the framework without branching on it.
//!
//! See `docs/architecture/adr-002-framework-adapters.md` for the
//! portable-component contract that constrains what users can write
//! against this adapter, and for the documented divergences between
//! Preact and React (aria casing, controlled inputs, hydrate vs
//! hydrateRoot, bundle size).

use crate::{RenderError, RenderHost};

use super::Adapter;

/// React framework adapter.
///
/// Stateless — a single instance is reused across all page renders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReactAdapter;

/// Synthetic module specifier under which the pre-render shim is
/// evaluated. Must not collide with any user-authored module path.
const REACT_SETUP_SPECIFIER: &str = "zfb:internal/adapters/react-setup.mjs";

/// Pre-render setup shim source. Imports the synchronous
/// `renderToString` entry from `react-dom/server` and exposes it at
/// `globalThis.__zfbRenderToString` so `render.rs` can call it
/// uniformly across adapters.
///
/// We import explicitly from `react-dom/server` (not `react-dom`) and
/// pick `renderToString` (not the streaming APIs) — see the module
/// docs for the rationale.
const REACT_SETUP_SOURCE: &str = r#"import { renderToString } from "react-dom/server";
globalThis.__zfbRenderToString = renderToString;
"#;

impl Adapter for ReactAdapter {
    fn name(&self) -> &'static str {
        "react"
    }

    fn jsx_import_source(&self) -> &'static str {
        "react"
    }

    fn render_to_string_module(&self) -> &'static str {
        "react-dom/server"
    }

    fn pre_render_setup(&self, host: &mut dyn RenderHost) -> Result<(), RenderError> {
        host.execute_module(REACT_SETUP_SPECIFIER, REACT_SETUP_SOURCE)
            .map(|_handle| ())
            .map_err(|e| {
                RenderError::Adapter(format!("react pre-render setup failed: {e}"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    impl RenderHost for CapturingHost {
        fn execute_module(
            &mut self,
            name: &str,
            source: &str,
        ) -> Result<ModuleHandle, RenderError> {
            self.calls
                .push((name.to_owned(), source.to_owned()));
            if let Some(msg) = &self.fail_with {
                return Err(RenderError::Adapter(msg.clone()));
            }
            Ok(ModuleHandle::new(self.calls.len() as u32, name))
        }

        fn call_default(
            &mut self,
            _handle: &ModuleHandle,
            _props: JsonValue,
        ) -> Result<String, RenderError> {
            Ok(String::new())
        }

        fn get_export(
            &mut self,
            _handle: &ModuleHandle,
            _name: &str,
        ) -> Result<JsonValue, RenderError> {
            Ok(JsonValue::Null)
        }
    }

    #[test]
    fn name_jsx_source_and_render_module_are_stable() {
        let a = ReactAdapter;
        assert_eq!(a.name(), "react");
        assert_eq!(a.jsx_import_source(), "react");
        assert_eq!(a.render_to_string_module(), "react-dom/server");
    }

    #[test]
    fn pre_render_setup_evaluates_shim_module() {
        let mut host = CapturingHost::default();
        ReactAdapter.pre_render_setup(&mut host).unwrap();
        assert_eq!(host.calls.len(), 1);
        let (specifier, source) = &host.calls[0];
        assert_eq!(specifier, REACT_SETUP_SPECIFIER);
        assert!(source.contains("react-dom/server"));
        assert!(source.contains("renderToString"));
        assert!(source.contains("__zfbRenderToString"));
    }

    #[test]
    fn pre_render_setup_does_not_use_streaming_apis() {
        // Guard rail against accidentally swapping in
        // renderToReadableStream / renderToPipeableStream. zfb produces
        // static HTML; streaming render adds complexity without a
        // matching benefit.
        assert!(!REACT_SETUP_SOURCE.contains("renderToReadableStream"));
        assert!(!REACT_SETUP_SOURCE.contains("renderToPipeableStream"));
    }

    #[test]
    fn pre_render_setup_wraps_host_errors_with_adapter_context() {
        let mut host = CapturingHost {
            fail_with: Some("oops".into()),
            ..Default::default()
        };
        let err = ReactAdapter.pre_render_setup(&mut host).unwrap_err();
        let msg = match err {
            RenderError::Adapter(m) => m,
            other => panic!("expected RenderError::Adapter, got {other:?}"),
        };
        assert!(msg.contains("react"));
        assert!(msg.contains("oops"));
    }
}
