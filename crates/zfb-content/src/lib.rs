//! zfb-content: markdown/MDX pipeline, syntect, frontmatter, content collections.

pub mod collection;
pub mod content_bridge;
pub mod dep_manifest;
pub mod diagnostics;
pub mod directive_parser;
pub mod facade;
pub mod footnotes;
pub mod frontmatter;
pub mod heading_registry;
pub mod hi_roles;
pub mod mdx_jsx_emit;
pub(crate) mod path_norm;
pub mod pipeline;
pub mod pipeline_spec;
pub mod plugins;
pub mod render_metadata;
pub mod schema;
pub mod serializer;
pub mod syntect_highlight;
pub mod tsx_frontmatter;

pub use content_bridge::{
    build_snapshot, build_snapshot_with_config, build_snapshot_with_options,
    debug_snapshot_enabled, BridgeError, CollectionConfig, ContentSnapshot, EntrySnapshot,
    SnapshotOptions,
};

pub use pipeline_spec::{CodeHighlightMode, PipelineSpec, PipelineSpecError};

// Render-artifact metadata channel (issue #2423, epic #2421): region
// identity, the raw-source digest, and the compiler-allocated headings.
// Re-exported at the crate root so the build's artifact writer can name
// the whole contract without reaching into module paths.
pub use render_metadata::{
    region_id_addresses, region_id_without_hash, render_region_metadata, source_digest,
    RenderRegionMetadata, SOURCE_DIGEST_PREFIX,
};

// Document-level GFM footnote model (#2025, epic #2021). Shared by BOTH emit
// paths (`pipeline`'s hast bridge and `mdx_jsx_emit`'s JSX emitter) so the
// numbering / id-allocation policy cannot drift between them — see the
// `footnotes` module docs for every policy decision it encodes.
pub use footnotes::{
    FootnoteCursor, FootnoteEntry, FootnoteModel, FootnoteRef, IdAllocator,
    FOOTNOTE_BACKREF_MARKER, FOOTNOTE_CLOBBER_PREFIX, FOOTNOTE_LABEL_ID, FOOTNOTE_LABEL_STYLE,
    FOOTNOTE_LABEL_TEXT, FOOTNOTE_SECTION_CLASS,
};

pub use pipeline::{
    constructs_for_jsx_emit, constructs_for_pipeline, Pipeline, PipelineError,
    ResolvedGfmConstructs,
};
pub use plugins::{ExternalLinksConfig, ExternalLinksPlugin};
pub use syntect_highlight::{
    validate_class_highlight_classes, validate_class_highlight_options,
    ClassHighlightFallbackReason, ClassHighlightOutcome, ClassHighlightRenderError,
    ClassHighlightValidationError, DEFAULT_CLASS_HIGHLIGHT_PREFIX,
};

// Wasm-safe facade (zfb#1574): config-JSON -> Pipeline -> { jsx module |
// html }, with no filesystem coupling. See `facade` module docs for the
// full contract; re-exported here so downstream crates (e.g. the future
// `zfb-md-wasm`) can name every facade type/fn from the crate root.
pub use facade::{
    build_pipeline, build_pipeline_from_json, compile_mdx_jsx_from_config, parse_pipeline_options,
    render_html, render_html_from_config, render_mdx_jsx_module, FacadeError, GfmOptions,
    PipelineOptions,
};

pub use plugins::toc::TocConfig;

// Re-export the markdown-features config types so downstream crates that only
// depend on `zfb-content` (e.g. `zfb-build`, `zfb-render`) can name them when
// threading `markdown.features` into the feature-aware pipeline constructor,
// without taking a direct dependency on `zfb-md-ast` / `zfb-md-extras`.
pub use zfb_md_extras::{
    directives_enabled, heading_id_strategy, into_directive_def, DirectiveFullSpec, DirectiveSpec,
    DirectiveSpecKind, FeatureToggle, HeadingIdStrategy, HeadingIdsConfig, ImageDimensionsConfig,
    LinkValidationConfig, MarkdownFeaturesConfig, TranscludeConfig,
};

// Read-recorder surface (zfb#942): the recorder + outcome types live in
// `zfb-md-ast` (so `zfb-md-extras` feature plugins can report reads
// without depending on zfb-content); the manifest stored in the compile
// cache lives here. Re-exported together so cache-side consumers can
// name the whole contract from one crate.
pub use dep_manifest::DependencyManifest;
pub use zfb_md_ast::{ReadOutcome, ReadRecorder};

// Cross-file anchor side channels (#960 / #977): the per-entry types the
// compile cache stores/replays and `Pipeline::take_cross_file_link_candidates`
// / `take_file_headings` drain. Re-exported so the post-compile check in
// `zfb-build` can name them without a direct `zfb-md-ast` dependency.
// Key contract: both `CrossFileLinkCandidate::target_path` and
// `FileHeadings::source_path` are normalised with
// `zfb_types::normalize_path_lexical` — consumers building heading maps
// MUST apply the same helper, never a near-match.
pub use zfb_md_ast::{CrossFileLinkCandidate, FileHeadings};

pub use frontmatter::{extract_from_filename, FrontmatterError, UnifiedFrontmatter};
pub use mdx_jsx_emit::{
    compile_mdx_to_jsx_module, compile_mdx_to_jsx_module_cached,
    compile_mdx_to_jsx_module_cached_with_deps, mdx_to_jsx_module, mdx_to_jsx_module_with_pipeline,
    parse_mdx_specifier, CompiledMdx, HeadingEntry, MdxJsxOptions, MdxModuleCache,
    MdxModuleSpecifier, SpecifierError,
};
// `DefaultExportFirstParam` / `PlainFirstParam` / `RequestParamTier` and
// `ssr_request_param_tier` are the SSR handler-shape detector (#2352):
// one gate definition shared by `zfb dev`, `zfb build`, and `zfb check`
// so none of the three re-implements the rule.
pub use tsx_frontmatter::{
    extract as extract_tsx_frontmatter, filename_extension_candidate, ssr_request_param_tier,
    DefaultExportFirstParam, PlainFirstParam, RequestParamTier, TsxFrontmatter,
    TsxFrontmatterError,
};
