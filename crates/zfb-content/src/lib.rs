//! zfb-content: markdown/MDX pipeline, syntect, frontmatter, content collections.

pub mod collection;
pub mod frontmatter;
pub mod mdx_jsx_emit;
pub mod pipeline;
pub mod plugins;
pub mod serializer;
pub mod syntect_highlight;
pub mod tsx_frontmatter;

pub use frontmatter::{
    extract as extract_frontmatter, FrontmatterError, UnifiedFrontmatter,
};
pub use mdx_jsx_emit::{
    compile_mdx_to_jsx_module, compile_mdx_to_jsx_module_cached, mdx_to_jsx_module,
    parse_mdx_specifier, CompiledMdx, MdxJsxOptions, MdxModuleCache, MdxModuleSpecifier,
    SpecifierError,
};
pub use tsx_frontmatter::{
    extract as extract_tsx_frontmatter, filename_extension_candidate, TsxFrontmatter,
    TsxFrontmatterError,
};

/// Crate-wide error type. Concrete variants are added by feature modules.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("frontmatter error: {0}")]
    Frontmatter(String),
    #[error("pipeline error: {0}")]
    Pipeline(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("collection error: {0}")]
    Collection(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
