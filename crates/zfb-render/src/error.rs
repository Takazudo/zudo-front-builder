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
    ///
    /// `importer` points at the file that contains the failing import so the
    /// error is actionable in editors (file path + optional line/col). `tried`
    /// lists the on-disk candidates probed so the operator can see whether a
    /// typo or missing extension is the cause.
    #[error(
        "could not resolve `{specifier}` imported from `{importer}`{loc}: no such module (tried: {tried_pretty})\n\
         help: check the import path or extension; the resolver probes .tsx, .ts, .jsx, .js, and `index.<ext>` for directories",
        loc = match (line, col) {
            (Some(l), Some(c)) => format!(":{l}:{c}"),
            (Some(l), None) => format!(":{l}"),
            _ => String::new(),
        },
        tried_pretty = if tried.is_empty() { "<none>".to_string() } else { tried.join(", ") },
    )]
    Resolve {
        /// The bare/relative specifier we tried to resolve.
        specifier: String,
        /// File-system path of the module that contains the failing import.
        importer: String,
        /// 1-based line number of the import statement when known.
        line: Option<u32>,
        /// 1-based column number of the import statement when known.
        col: Option<u32>,
        /// Filesystem candidates probed by the resolver. Helps the operator
        /// diagnose typos and missing extensions.
        tried: Vec<String>,
    },

    /// JS runtime failure (loading, evaluating, or calling into a module).
    #[error("runtime error: {0}")]
    Runtime(String),

    /// The page's default export was missing or not callable.
    #[error("`default` export missing or not callable in `{0}`")]
    MissingDefaultExport(String),

    /// Framework adapter setup or rendering failure.
    #[error("adapter error: {0}")]
    Adapter(String),

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

    /// Convenience constructor for resolve errors with no probe list and no
    /// import-statement source location. Prefer
    /// [`RenderError::resolve_with`] when those details are available.
    pub fn resolve(specifier: impl Into<String>, importer: impl Into<String>) -> Self {
        Self::Resolve {
            specifier: specifier.into(),
            importer: importer.into(),
            line: None,
            col: None,
            tried: Vec::new(),
        }
    }

    /// Convenience constructor for resolve errors that record the candidate
    /// paths probed by the resolver and (optionally) the line/column of the
    /// `import` statement in the importer file.
    pub fn resolve_with(
        specifier: impl Into<String>,
        importer: impl Into<String>,
        line: Option<u32>,
        col: Option<u32>,
        tried: Vec<String>,
    ) -> Self {
        Self::Resolve {
            specifier: specifier.into(),
            importer: importer.into(),
            line,
            col,
            tried,
        }
    }
}
