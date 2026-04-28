//! Render orchestrator: compile → load → execute → render-to-string.
//!
//! The orchestrator is intentionally thin: it owns a `ModuleLoader` and a
//! `RenderHost`, and stitches them together. Smart bits (path resolution,
//! `paths()` evaluation, `meta` extraction) live in their own modules.

use serde_json::Value as JsonValue;

use crate::error::Result;
use crate::loader::ModuleLoader;
use crate::render_host::{ModuleHandle, RenderHost};
use crate::swc_pipeline::JsxRuntime;

/// Per-render input.
#[derive(Debug, Clone)]
pub struct RenderRequest {
    /// Display name / path of the page (used for error messages and as the
    /// JS runtime's module specifier).
    pub specifier: String,
    /// TSX source for the page.
    pub source: String,
    /// Props to pass to the page's `default` export.
    pub props: JsonValue,
}

impl RenderRequest {
    /// Build a request with empty props.
    pub fn new(specifier: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            specifier: specifier.into(),
            source: source.into(),
            props: JsonValue::Null,
        }
    }

    /// Attach props.
    pub fn with_props(mut self, props: JsonValue) -> Self {
        self.props = props;
        self
    }
}

/// Render orchestrator parameterised over the JS runtime host.
pub struct Renderer<H: RenderHost> {
    loader: ModuleLoader,
    host: H,
}

impl<H: RenderHost> Renderer<H> {
    /// Build a renderer with the given host and JSX runtime preference.
    pub fn new(host: H, jsx_runtime: JsxRuntime) -> Self {
        Self {
            loader: ModuleLoader::new(jsx_runtime),
            host,
        }
    }

    /// Render `req` to an HTML string.
    pub async fn render(&mut self, req: &RenderRequest) -> Result<String> {
        let handle = self.compile_and_load(req).await?;
        self.host.call_default(&handle, req.props.clone()).await
    }

    /// Compile the request's source and hand it to the host. Exposed so
    /// `paths()` / `meta` can reuse the same module handle.
    pub async fn compile_and_load(&mut self, req: &RenderRequest) -> Result<ModuleHandle> {
        let compiled = self
            .loader
            .load_source(&req.specifier, &req.source)?
            .clone();
        self.host
            .execute_module(&compiled.specifier, &compiled.code)
            .await
    }

    /// Borrow the underlying loader (mostly for tests / introspection).
    pub fn loader(&self) -> &ModuleLoader {
        &self.loader
    }

    /// Borrow the host (for advanced callers; prefer `render`).
    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }
}
