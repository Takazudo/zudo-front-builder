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
    ///
    /// The opt-in `StripMdExtensionPlugin` is OFF — use
    /// [`Renderer::with_strip_md_ext`] when the project enables
    /// `stripMdExt` so dev rendering matches built dist.
    pub fn new(host: H, jsx_runtime: JsxRuntime) -> Self {
        Self::with_strip_md_ext(host, jsx_runtime, false)
    }

    /// Build a renderer, optionally enabling the
    /// [`StripMdExtensionPlugin`] in the loader's MDX content pipeline.
    ///
    /// Mirrors the bundler's `pipeline_spec.strip_md_ext` knob (zfb#127 /
    /// #129); the build CLI threads `Config::strip_md_ext` here so
    /// `zfb dev` and `zfb build` produce identical href shapes.
    ///
    /// GFM constructs default to the conservative set
    /// (`ResolvedGfmConstructs::CONSERVATIVE`); use
    /// [`Renderer::with_strip_md_ext_and_gfm`] when the project's
    /// `zfb.config.ts` configures `markdown.gfm`.
    ///
    /// [`StripMdExtensionPlugin`]: zfb_content::plugins::StripMdExtensionPlugin
    pub fn with_strip_md_ext(host: H, jsx_runtime: JsxRuntime, strip_md_ext: bool) -> Self {
        Self::with_strip_md_ext_and_gfm(
            host,
            jsx_runtime,
            strip_md_ext,
            zfb_content::ResolvedGfmConstructs::default(),
        )
    }

    /// Constructor with `strip_md_ext`, resolved GFM construct set, and
    /// CJK-friendly toggle. Forwards all three to the loader so dev
    /// rendering and the bundler agree on every parser knob.
    pub fn with_strip_md_ext_and_gfm(
        host: H,
        jsx_runtime: JsxRuntime,
        strip_md_ext: bool,
        gfm_constructs: zfb_content::ResolvedGfmConstructs,
    ) -> Self {
        Self::with_strip_md_ext_and_gfm_and_cjk(
            host,
            jsx_runtime,
            strip_md_ext,
            gfm_constructs,
            true,
        )
    }

    /// Constructor: forwards `strip_md_ext`, GFM constructs, and
    /// CJK-friendly toggle to the loader. `markdown.features` is left at
    /// `None` (an empty feature set → the three former-Core framework features
    /// are OFF, the opt-in default); use
    /// [`Renderer::with_strip_md_ext_and_gfm_and_cjk_and_features`] to honour
    /// the feature surface.
    pub fn with_strip_md_ext_and_gfm_and_cjk(
        host: H,
        jsx_runtime: JsxRuntime,
        strip_md_ext: bool,
        gfm_constructs: zfb_content::ResolvedGfmConstructs,
        cjk_friendly: bool,
    ) -> Self {
        Self::with_strip_md_ext_and_gfm_and_cjk_and_features(
            host,
            jsx_runtime,
            strip_md_ext,
            gfm_constructs,
            cjk_friendly,
            false,
            None,
        )
    }

    /// Most-explicit constructor: forwards `strip_md_ext`, GFM constructs,
    /// CJK-friendly toggle, `hard_breaks`, and `markdown.features` to the
    /// loader so dev rendering agrees with the bundler on every knob —
    /// including which opt-in feature plugins fire. `features = None` is an
    /// empty feature set: the three former-Core framework features are off
    /// (the opt-in default).
    ///
    /// Note: the `zfb dev` CLI renders through the V8 `RendererState` in
    /// `zfb-build`, threading features via `BundlerInput` — not through this
    /// `Renderer`. These constructors are for library / embedder callers.
    pub fn with_strip_md_ext_and_gfm_and_cjk_and_features(
        host: H,
        jsx_runtime: JsxRuntime,
        strip_md_ext: bool,
        gfm_constructs: zfb_content::ResolvedGfmConstructs,
        cjk_friendly: bool,
        hard_breaks: bool,
        features: Option<&zfb_content::MarkdownFeaturesConfig>,
    ) -> Self {
        Self {
            loader: ModuleLoader::with_strip_md_ext_and_gfm_and_cjk_and_features(
                jsx_runtime,
                strip_md_ext,
                gfm_constructs,
                cjk_friendly,
                hard_breaks,
                features,
            ),
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
