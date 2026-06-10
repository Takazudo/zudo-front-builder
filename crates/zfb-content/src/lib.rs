//! zfb-content: markdown/MDX pipeline, syntect, frontmatter, content collections.

pub mod collection;
pub mod content_bridge;
pub mod dep_manifest;
pub mod diagnostics;
pub mod frontmatter;
pub mod heading_registry;
pub mod mdx_jsx_emit;
pub(crate) mod path_norm;
pub mod pipeline;
pub mod pipeline_spec;
pub mod plugins;
pub mod schema;
pub mod serializer;
pub mod syntect_highlight;
pub mod tsx_frontmatter;

pub use content_bridge::{
    build_snapshot, build_snapshot_with_config, debug_snapshot_enabled, BridgeError,
    CollectionConfig, ContentSnapshot, EntrySnapshot,
};
pub use pipeline_spec::{PipelineSpec, PipelineSpecError};

pub use pipeline::{constructs_for_jsx_emit, constructs_for_pipeline, ResolvedGfmConstructs};
pub use plugins::{ExternalLinksConfig, ExternalLinksPlugin};

pub use plugins::toc::TocConfig;

// Re-export the markdown-features config types so downstream crates that only
// depend on `zfb-content` (e.g. `zfb-build`, `zfb-render`) can name them when
// threading `markdown.features` into the feature-aware pipeline constructor,
// without taking a direct dependency on `zfb-md-ast` / `zfb-md-extras`.
pub use zfb_md_extras::{
    directives_enabled, heading_id_strategy, into_directive_def, DirectiveFullSpec, DirectiveSpec,
    DirectiveSpecKind, FeatureToggle, HeadingIdStrategy, HeadingIdsConfig, MarkdownFeaturesConfig,
};

// Read-recorder surface (zfb#942): the recorder + outcome types live in
// `zfb-md-ast` (so `zfb-md-extras` feature plugins can report reads
// without depending on zfb-content); the manifest stored in the compile
// cache lives here. Re-exported together so cache-side consumers can
// name the whole contract from one crate.
pub use dep_manifest::DependencyManifest;
pub use zfb_md_ast::{ReadOutcome, ReadRecorder};

pub use frontmatter::{FrontmatterError, UnifiedFrontmatter};
pub use mdx_jsx_emit::{
    compile_mdx_to_jsx_module, compile_mdx_to_jsx_module_cached, mdx_to_jsx_module,
    mdx_to_jsx_module_with_pipeline, parse_mdx_specifier, CompiledMdx, MdxJsxOptions,
    MdxModuleCache, MdxModuleSpecifier, SpecifierError,
};
pub use tsx_frontmatter::{
    extract as extract_tsx_frontmatter, filename_extension_candidate, TsxFrontmatter,
    TsxFrontmatterError,
};
