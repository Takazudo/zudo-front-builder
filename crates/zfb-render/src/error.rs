//! Crate-wide error type.

use thiserror::Error;

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, RenderError>;

/// All recoverable failure modes surfaced by `zfb-render`.
#[derive(Debug, Error)]
pub enum RenderError {
    /// SWC parse / transform / codegen failure.
    #[error("compile error in {file}: {message}")]
    Compile {
        /// Display name (path) of the source the compiler was working on.
        file: String,
        /// Human-readable explanation.
        message: String,
    },

    /// Module resolution failure (e.g., relative import not found).
    #[error("could not resolve `{specifier}` from `{importer}`")]
    Resolve {
        /// The bare/relative specifier we tried to resolve.
        specifier: String,
        /// The importer module's display name.
        importer: String,
    },

    /// JS runtime failure (loading, evaluating, or calling into a module).
    #[error("runtime error: {0}")]
    Runtime(String),

    /// The page's default export was missing or not callable.
    #[error("`default` export missing or not callable in `{0}`")]
    MissingDefaultExport(String),

    /// I/O failure (reading source files, etc.).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialisation failure.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Catch-all for context that doesn't fit a specific variant. Prefer a
    /// specific variant when possible.
    #[error("{0}")]
    Other(String),
}

impl RenderError {
    /// Convenience constructor for compile errors.
    pub fn compile(file: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Compile {
            file: file.into(),
            message: message.into(),
        }
    }

    /// Convenience constructor for resolve errors.
    pub fn resolve(specifier: impl Into<String>, importer: impl Into<String>) -> Self {
        Self::Resolve {
            specifier: specifier.into(),
            importer: importer.into(),
        }
    }
}
