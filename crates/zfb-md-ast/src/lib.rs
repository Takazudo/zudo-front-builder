//! Shared AST types, visitor traits, and visitor-contract context for the
//! zfb markdown/MDX pipeline.
//!
//! This crate is the dependency boundary that lets `zfb-md-extras` (and other
//! downstream plugin crates) implement visitors without depending on
//! `zfb-content`. It carries the visitor contract end-to-end: the AST type
//! (`HastNode`), the traits (`MdastVisitor` / `HastVisitor`), and the
//! orchestration context types those traits reference (`BuildContext`,
//! `DiagnosticsSink`, `HeadingRegistry`, `MarkdownDiagnostic`).
//!
//! Wave 1 scope expansion: this crate originally targeted just the AST + trait
//! defs (#565). Sub-issue #568 added `HastVisitor::visit_with_context` whose
//! signature references `BuildContext` and its dependent types — those moved
//! here too so the contract stays self-contained. `zfb-content` re-exports
//! everything for backwards-compatible consumer paths.

use std::path::PathBuf;

use markdown::mdast::Node as MdastNode;

pub mod diagnostics;
pub mod directives;
pub mod features_config;
pub mod hast_text;
pub mod heading_registry;

pub use directives::{
    AttrSchema, AttrType, AttrValidationResult, DirectiveDef, DirectiveDiagnostic, DirectiveKind,
    ValidatedAttrValue,
};
pub use features_config::{
    directives_enabled, feature_enabled, heading_marker_toc_enabled, CodeEnrichmentConfig,
    DirectiveFullSpec, DirectiveSpec, DirectiveSpecKind, FeatureOptions, FeatureToggle,
    GithubAutolinksConfig, HeadingMarkerTocFeature, ImageDimensionsConfig, LinkValidationConfig,
    MarkdownFeaturesConfig, TocConfig, TocExportConfig, TranscludeConfig, into_directive_def,
};
pub use hast_text::extract_text;

/// Lightweight HTML AST node.
///
/// Plugins (mdast and hast visitors) operate on this representation in
/// memory; the serializer turns it into an HTML string later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HastNode {
    /// Document root.
    Root {
        /// Top-level children.
        children: Vec<HastNode>,
    },
    /// HTML element.
    Element {
        /// Tag name (e.g. `"p"`, `"h1"`, `"a"`).
        tag: String,
        /// Attribute list as `(name, value)` pairs.
        ///
        /// Attribute order is preserved so the serializer produces stable
        /// output and so plugins can assert on ordering when useful.
        attrs: Vec<(String, String)>,
        /// Child nodes; empty for void elements.
        children: Vec<HastNode>,
        /// True for self-closing void elements (`img`, `br`, `hr`, etc.).
        void: bool,
    },
    /// Plain text content (escaped on serialization).
    Text(String),
    /// Raw HTML passthrough; the serializer emits this verbatim without
    /// escaping. Produced by the mdast→hast conversion for
    /// `markdown::mdast::Node::Html`, and by hast plugins that synthesize
    /// complete HTML fragments (e.g. syntect).
    ///
    /// On the JSX-emit path (`mdx_jsx_emit::mdx_to_jsx_module_with_pipeline`),
    /// `Raw` cannot be embedded verbatim — JSX does not understand
    /// arbitrary HTML such as `class="…"` or inline `<span style="…">`.
    /// The hast→JSX bridge wraps `Raw` content in a span with
    /// `dangerouslySetInnerHTML` so the rendered DOM still receives the
    /// original markup. See [`HastNode::JsxRaw`] for the JSX-shaped
    /// counterpart that IS safe to inline.
    Raw(String),
    /// JSX-shaped passthrough — MDX components (`<Note>…</Note>`),
    /// flow / text expressions (`{1 + 1}`), and synthesized JSX
    /// fragments. The serializer treats this identically to
    /// [`HastNode::Raw`] (verbatim, no escaping); the JSX-emit path
    /// embeds it verbatim into the output module so PascalCase
    /// component references and `{…}` expression containers survive
    /// untouched.
    ///
    /// Splitting JSX from HTML at the hast level lets the JSX bridge
    /// pick the right embedding strategy without parsing the payload.
    JsxRaw(String),
    /// HTML comment body (without the `<!--` / `-->` delimiters).
    Comment(String),
}

/// Per-document build context threaded into pipeline visitors when the
/// orchestrator needs wave-6 features (image dimensions, link validation,
/// transclusion).
///
/// **Backwards-compat:** the existing `Pipeline::run` method passes no
/// context. Wave-6 visitors that depend on the context are only invoked via
/// `Pipeline::run_with_context`. Callers that never pass a context pay zero
/// overhead.
pub struct BuildContext<'a> {
    /// Absolute path of the markdown source file being rendered, or `None`
    /// if not available (e.g. rendering an in-memory string in tests).
    pub source_path: Option<PathBuf>,
    /// Root directory of the project — used to resolve asset paths and
    /// build the registry key for the heading-ID lookup.
    pub project_root: PathBuf,
    /// The public / static-assets directory — used by the image-dimensions
    /// plugin (wave 6.1) to locate image files on disk.
    pub public_dir: PathBuf,
    /// Optional mutable reference to the build-scoped heading-ID registry.
    ///
    /// When `Some`, `HeadingLinksPlugin` writes an entry for each heading it
    /// processes. When `None`, no registry writes occur (zero cost for callers
    /// that do not perform link validation).
    pub heading_registry: Option<&'a mut crate::heading_registry::HeadingRegistry>,
    /// Optional mutable reference to a diagnostics sink.
    ///
    /// Plugins emit [`crate::diagnostics::MarkdownDiagnostic`] values here.
    /// When `None`, diagnostics are silently discarded (zero cost for callers
    /// that do not collect diagnostics).
    pub diagnostics: Option<&'a mut dyn crate::diagnostics::DiagnosticsSink>,
}

impl<'a> BuildContext<'a> {
    /// Convenience constructor for tests and simple orchestrators that only
    /// need paths (no registry or diagnostics wired in yet).
    #[must_use]
    pub fn for_paths(
        source_path: impl Into<PathBuf>,
        project_root: impl Into<PathBuf>,
        public_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            source_path: Some(source_path.into()),
            project_root: project_root.into(),
            public_dir: public_dir.into(),
            heading_registry: None,
            diagnostics: None,
        }
    }
}

/// Mdast visitor: mutates an mdast tree in place.
///
/// Implementors typically call [`MdastNode::children_mut`] to recurse, or
/// implement their own walk. The pipeline does NOT auto-recurse for
/// visitors; each visitor decides its own traversal strategy.
pub trait MdastVisitor {
    /// Visit (and possibly mutate) `node`.
    fn visit(&mut self, node: &mut MdastNode);

    /// Visit with optional build context (wave-6 seam).
    ///
    /// Plugins that need `BuildContext` (source path, project root) to
    /// perform file resolution or diagnostics override this method. The
    /// default delegates to [`Self::visit`] so all existing visitors are
    /// automatically backwards-compatible.
    ///
    /// Called by `Pipeline::apply_mdast_visitors_with_context` /
    /// `Pipeline::run_with_context` when context is available.
    fn visit_with_context(&mut self, node: &mut MdastNode, _ctx: &mut BuildContext<'_>) {
        self.visit(node);
    }
}

/// Hast visitor: mutates a hast tree in place.
///
/// Same recursion contract as [`MdastVisitor`].
pub trait HastVisitor {
    /// Visit (and possibly mutate) `node`.
    fn visit(&mut self, node: &mut HastNode);

    /// Visit with optional build context (wave-6 seam).
    ///
    /// Plugins that need `BuildContext` (heading-ID registry, diagnostics
    /// sink, source-path resolution) override this method. The default
    /// delegates to [`Self::visit`] so all existing visitors are
    /// automatically backwards-compatible — they receive context-free
    /// calls via `visit` and never see the context.
    ///
    /// Called by `Pipeline::run_with_context` / `Pipeline::apply_hast_visitors_with_context`.
    fn visit_with_context(&mut self, node: &mut HastNode, _ctx: &mut BuildContext<'_>) {
        self.visit(node);
    }

    /// Reset any per-document state accumulated during [`Self::visit`].
    ///
    /// Called by `Pipeline::reset_per_entry` between documents so
    /// cross-document state (e.g. duplicate-slug counters in
    /// `HeadingLinksPlugin`) cannot leak from one entry to the next.
    /// The default implementation is a no-op, which is correct for
    /// stateless visitors. Stateful visitors (currently only
    /// `HeadingLinksPlugin`) override this method.
    fn reset(&mut self) {}
}
