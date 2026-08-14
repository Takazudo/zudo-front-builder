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

pub mod cjk;
pub mod cjk_autolink;
pub mod diagnostics;
pub mod directives;
pub mod features_config;
pub mod gfm_constructs;
pub mod hast_text;
pub mod heading_registry;
pub mod mdx_jsx;
pub mod nested_link;
pub mod read_recorder;

pub use cjk_autolink::CjkAutolinkBoundaryPlugin;
pub use gfm_constructs::{constructs_for_jsx_emit, constructs_for_pipeline, ResolvedGfmConstructs};
pub use nested_link::unwrap_nested_links;

pub use directives::{
    AttrSchema, AttrType, AttrValidationResult, DirectiveDef, DirectiveDiagnostic, DirectiveKind,
    ValidatedAttrValue,
};
pub use features_config::{
    directives_enabled, feature_enabled, heading_id_strategy, heading_marker_toc_enabled,
    into_directive_def, reading_time_enabled, CodeEnrichmentConfig, DirectiveFullSpec,
    DirectiveSpec, DirectiveSpecKind, FeatureOptions, FeatureToggle, HeadingIdStrategy,
    HeadingIdsConfig, HeadingMarkerTocFeature, ImageDimensionsConfig, LinkValidationConfig,
    MarkdownFeaturesConfig, ReadingTimeFeature, ReadingTimeOptions, TocConfig, TocExportConfig,
    TranscludeConfig,
};
pub use hast_text::extract_text;
pub use read_recorder::{sha256_hex, ReadOutcome, ReadRecorder};

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

/// A cross-file fragment link (`./other.md#frag`) the per-compile
/// validator could NOT verify locally and therefore defers to the
/// post-compile cross-file check (#960 / #977).
///
/// `LinkValidationPlugin` records one candidate at its existence-only
/// degrade branch: the link already passed containment + existence, but
/// the per-compile-local [`heading_registry::HeadingRegistry`] has no
/// entry for the target file, so the fragment verdict needs the
/// build-wide heading map that only exists after every file compiled.
/// Candidates are a pure function of the compile input (plus
/// dep-manifest-covered reads), so the compile cache can store and
/// replay them exactly like markdown diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CrossFileLinkCandidate {
    /// Absolute path of the source file containing the link — diagnostic
    /// location for the post-compile check (NOT a lookup key).
    pub source_path: PathBuf,
    /// Resolved absolute path of the link target, normalised with
    /// `zfb_types::normalize_path_lexical` — the shared helper the
    /// post-compile consumer (#980) MUST also apply to its heading-map
    /// keys so candidate↔headings lookups can never split on path
    /// spelling.
    pub target_path: PathBuf,
    /// The fragment after `#`, exactly as written (non-empty, not
    /// percent-encoded — those links never degrade).
    pub fragment: String,
    /// The original href as authored, for diagnostic messages.
    pub raw_href: String,
    /// Zero-based occurrence within this source compile's deferred-candidate
    /// stream. Together with the source path, this distinguishes repeated
    /// authored hrefs while allowing the bundler to collapse an identical
    /// candidate replayed by multiple materialisation passes.
    pub occurrence_index: usize,
    /// Severity the recording plugin would have emitted
    /// (`failOnBroken` ⇒ `Error`, else `Warning`).
    pub severity: crate::diagnostics::DiagnosticSeverity,
}

/// The headings of one compiled file, surfaced as a compile side channel
/// for the post-compile cross-file anchor check (#960 / #977).
///
/// Recorded once per context-armed compile from the same canonical
/// `collect_headings` walk that seeds the per-compile registry
/// (transclusion-aware and JSX-nested-aware). An entry with an EMPTY
/// `headings` vec is meaningful: the file compiled and has no
/// anchor-addressable headings — distinct from "file never compiled".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHeadings {
    /// Absolute path of the compiled source file, normalised with
    /// `zfb_types::normalize_path_lexical` (the same shared helper used
    /// for [`CrossFileLinkCandidate::target_path`] — consumers keying a
    /// heading map MUST apply that helper, never a near-match).
    pub source_path: PathBuf,
    /// Anchor-addressable headings in document order — the exact entry
    /// set the per-compile registry is seeded with (empty slugs are
    /// excluded there too).
    pub headings: Vec<crate::heading_registry::HeadingEntry>,
    /// Explicit non-heading anchor ids rendered by the file (`id` and legacy
    /// `a[name]` targets), carried alongside headings for the build-wide
    /// fragment check.
    pub anchor_ids: Vec<String>,
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
    /// Optional per-compile buffer for cross-file fragment-link
    /// candidates (#960 / #977). `LinkValidationPlugin` pushes a
    /// [`CrossFileLinkCandidate`] here whenever a `./other.md#frag`
    /// verdict degrades to existence-only; the orchestrator flushes the
    /// buffer into the pipeline's side channel so the compile cache can
    /// store/replay it. When `None`, candidates are silently discarded
    /// (zero cost for callers that do not run the post-compile check).
    pub cross_file_links: Option<&'a mut Vec<CrossFileLinkCandidate>>,
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
            cross_file_links: None,
        }
    }
}

/// Which emit path the pipeline run currently under way is feeding.
///
/// Only meaningful to visitors that re-enter `markdown::to_mdast` from
/// *inside* the mdast visitor chain (`TranscludePlugin`,
/// `DirectiveRegistry::reparse_block`). Those secondary parse sites pick
/// their own `markdown::Constructs`, and the correct set differs per
/// path: the HTML serializer renders `Math` / `InlineMath` into real
/// `<pre><code class="language-math …">` elements, so math must stay off
/// there to keep output byte-identical, while the JSX emitter has
/// dedicated arms for those nodes and needs math ON — without it LaTeX
/// leaks out as bare `{…}` expression containers that esbuild rejects,
/// falling the whole page back to `<pre data-zfb-content-fallback>`.
///
/// Delivered per-run via [`MdastVisitor::set_secondary_parse_target`]
/// rather than at plugin construction: one plugin instance serves both
/// paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryParseTarget {
    /// `Pipeline::run` / `Pipeline::run_with_context` — the HTML
    /// serializer and collection walker.
    Html,
    /// `Pipeline::apply_mdast_visitors` /
    /// `Pipeline::apply_mdast_visitors_with_context` — the MDX → JSX
    /// emit path.
    Jsx,
}

/// Mdast visitor: mutates an mdast tree in place.
///
/// Implementors typically call [`MdastNode::children_mut`] to recurse, or
/// implement their own walk. The pipeline does NOT auto-recurse for
/// visitors; each visitor decides its own traversal strategy.
pub trait MdastVisitor {
    /// Visit (and possibly mutate) `node`.
    fn visit(&mut self, node: &mut MdastNode);

    /// Announce which emit path the imminent visit is feeding.
    ///
    /// Called by every `Pipeline` mdast dispatch loop on every visitor
    /// immediately before visiting, with a target hardcoded per loop —
    /// never caller-supplied. Visitors that re-parse markdown override
    /// this to store the value and consult it when choosing constructs;
    /// see [`SecondaryParseTarget`]. The default is a no-op, so every
    /// other implementor is unaffected.
    ///
    /// Implementors must overwrite their stored target unconditionally
    /// on each call — a pipeline instance is reused across documents and
    /// across both paths, so a stale value from a previous run is the
    /// failure mode to avoid.
    fn set_secondary_parse_target(&mut self, _target: SecondaryParseTarget) {}

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
