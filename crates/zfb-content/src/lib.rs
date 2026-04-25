//! zfb-content: markdown/MDX pipeline, syntect, frontmatter, content collections.

pub mod frontmatter;
pub mod pipeline;
pub mod plugins;
pub mod syntect_highlight;
pub mod serializer;
pub mod collection;

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
