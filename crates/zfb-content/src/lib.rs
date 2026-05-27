//! zfb-content: markdown/MDX pipeline, syntect, frontmatter, content collections.

pub mod collection;
pub mod content_bridge;
pub mod diagnostics;
pub mod frontmatter;
pub mod heading_registry;
pub mod mdx_jsx_emit;
pub mod pipeline;
pub mod plugins;
pub mod schema;
pub mod serializer;
pub mod syntect_highlight;
pub mod tsx_frontmatter;

pub use content_bridge::{
    build_snapshot, build_snapshot_with_config, debug_snapshot_enabled, BridgeError,
    CollectionConfig, ContentSnapshot, EntrySnapshot, SnapshotPipelineConfig,
};

pub use pipeline::{
    constructs_for_jsx_emit, constructs_for_pipeline, ResolvedGfmConstructs,
};
pub use plugins::{ExternalLinksConfig, ExternalLinksPlugin};

pub use plugins::toc::TocConfig;

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

