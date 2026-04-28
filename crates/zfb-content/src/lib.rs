//! zfb-content: markdown/MDX pipeline, syntect, frontmatter, content collections.

pub mod collection;
pub mod content_bridge;
pub mod frontmatter;
pub mod mdx_jsx_emit;
pub mod pipeline;
pub mod plugins;
pub mod schema;
pub mod serializer;
pub mod syntect_highlight;
pub mod tsx_frontmatter;

pub use content_bridge::{
    build_snapshot, debug_snapshot_enabled, BridgeError, CollectionConfig, ContentSnapshot,
    EntrySnapshot,
};

pub use frontmatter::{FrontmatterError, UnifiedFrontmatter};
pub use mdx_jsx_emit::{
    compile_mdx_to_jsx_module, compile_mdx_to_jsx_module_cached, mdx_to_jsx_module,
    parse_mdx_specifier, CompiledMdx, MdxJsxOptions, MdxModuleCache, MdxModuleSpecifier,
    SpecifierError,
};
pub use tsx_frontmatter::{
    extract as extract_tsx_frontmatter, filename_extension_candidate, TsxFrontmatter,
    TsxFrontmatterError,
};

