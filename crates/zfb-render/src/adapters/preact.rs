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
//!
//! See `docs/architecture/adr-002-framework-adapters.md` for the
//! portable-component contract that constrains what users can write
//! against this adapter.

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

    fn pre_render_setup(&self, host: &mut dyn RenderHost) -> Result<(), RenderError> {
        host.execute_module(PREACT_SETUP_SPECIFIER, PREACT_SETUP_SOURCE)
            .map(|_handle| ())
            .map_err(|e| {
                RenderError::Adapter(format!("preact pre-render setup failed: {e}"))
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
        let a = PreactAdapter;
        assert_eq!(a.name(), "preact");
        assert_eq!(a.jsx_import_source(), "preact");
        assert_eq!(a.render_to_string_module(), "preact-render-to-string");
    }

    #[test]
    fn pre_render_setup_evaluates_shim_module() {
        let mut host = CapturingHost::default();
        PreactAdapter.pre_render_setup(&mut host).unwrap();
        assert_eq!(host.calls.len(), 1);
        let (specifier, source) = &host.calls[0];
        assert_eq!(specifier, PREACT_SETUP_SPECIFIER);
        assert!(source.contains("preact-render-to-string"));
        assert!(source.contains("__zfbRenderToString"));
    }

    #[test]
    fn pre_render_setup_wraps_host_errors_with_adapter_context() {
        let mut host = CapturingHost {
            fail_with: Some("oops".into()),
            ..Default::default()
        };
        let err = PreactAdapter.pre_render_setup(&mut host).unwrap_err();
        let msg = match err {
            RenderError::Adapter(m) => m,
            other => panic!("expected RenderError::Adapter, got {other:?}"),
        };
        assert!(msg.contains("preact"));
        assert!(msg.contains("oops"));
    }
}
