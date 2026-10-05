//! Generic markdown diagnostics sink for wave-6 visitors.
//!
//! Provides a `MarkdownDiagnostic` enum that carries a severity level,
//! optional source location, and a message. Wave-6 plugins (link
//! validation, image dimensions, transclusion) emit diagnostics through
//! a `DiagnosticsSink` trait so the orchestrator can route them to the
//! appropriate output without coupling the pipeline to a specific
//! reporting backend.
//!
//! The existing `BrokenLinkDiagnostic` (from `resolve_links`) is
//! represented as the `BrokenLink` variant so all link-related
//! diagnostics share one drain point.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use zfb_types::build_diagnostics::BuildDiagnostic;
pub use zfb_types::build_diagnostics::{codes, DiagnosticSeverity};

/// Source location within a markdown file (1-based line/column).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    /// Path of the source file that produced the diagnostic.
    pub path: Option<PathBuf>,
    /// 1-based line number, if known.
    pub line: Option<u32>,
    /// 1-based column number, if known.
    pub col: Option<u32>,
    /// Explicit one-based UTF-8 byte column; legacy `col` is never converted.
    #[serde(
        default,
        rename = "byteColumn",
        skip_serializing_if = "Option::is_none"
    )]
    pub byte_column: Option<u32>,
}

impl SourceLocation {
    /// Location with path only (no line/column information).
    #[must_use]
    pub fn from_path(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            line: None,
            col: None,
            byte_column: None,
        }
    }
}

/// A single diagnostic produced by a markdown pipeline plugin.
///
/// Wave-6 plugins emit these through a [`DiagnosticsSink`]; the orchestrator
/// collects them and can report errors, emit warnings, or suppress info
/// messages depending on build configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MarkdownDiagnostic {
    /// A `.md`/`.mdx` link target that could not be resolved via the source
    /// map. Migrated from the standalone `BrokenLinkDiagnostic` type so all
    /// link diagnostics share one drain point.
    BrokenLink {
        /// Severity — typically `Warning` so the build continues.
        severity: DiagnosticSeverity,
        /// The original (unresolved) link URL, as written by the author.
        url: String,
        /// Location of the link in the source file, if available.
        location: Option<SourceLocation>,
    },
    /// A generic diagnostic emitted by any pipeline plugin.
    ///
    /// Used by smoke tests and future wave-6 plugins (image dimensions,
    /// transclusion) before dedicated variants are warranted.
    Generic {
        /// Producer-assigned identity; omitted by older plugins.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        /// Severity level.
        severity: DiagnosticSeverity,
        /// Human-readable message describing the issue.
        message: String,
        /// Location in the source file, if available.
        location: Option<SourceLocation>,
    },
}

impl MarkdownDiagnostic {
    /// Stable identity, independent of human-readable prose.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::BrokenLink { .. } => codes::BROKEN_LINK,
            Self::Generic { code, .. } => code
                .as_deref()
                .filter(|code| !code.trim().is_empty())
                .unwrap_or(codes::GENERIC_MARKDOWN),
        }
    }

    /// Set a generic diagnostic's identity at its emitting origin.
    #[must_use]
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        if let Self::Generic { code: value, .. } = &mut self {
            *value = Some(code.into());
        }
        self
    }

    /// Adapt without guessing source units. Legacy `col` remains available to
    /// legacy formatters but is not a UTF-8 byte coordinate.
    #[must_use]
    pub fn to_build_diagnostic(&self) -> BuildDiagnostic {
        let (message, location) = match self {
            Self::BrokenLink { url, location, .. } => (format!("broken link: {url}"), location),
            Self::Generic {
                message, location, ..
            } => (message.clone(), location),
        };
        let mut diagnostic = BuildDiagnostic::new(self.code(), self.severity(), message);
        if let Some(location) = location {
            diagnostic.file = location
                .path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned());
            diagnostic.line = location.line;
            diagnostic.byte_column = location.byte_column;
        }
        diagnostic
    }

    /// The severity level of this diagnostic.
    #[must_use]
    pub fn severity(&self) -> DiagnosticSeverity {
        match self {
            MarkdownDiagnostic::BrokenLink { severity, .. } => *severity,
            MarkdownDiagnostic::Generic { severity, .. } => *severity,
        }
    }

    /// Convenience constructor for a generic warning.
    #[must_use]
    pub fn warning(message: impl Into<String>) -> Self {
        MarkdownDiagnostic::Generic {
            code: None,
            severity: DiagnosticSeverity::Warning,
            message: message.into(),
            location: None,
        }
    }

    /// Convenience constructor for a generic error.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        MarkdownDiagnostic::Generic {
            code: None,
            severity: DiagnosticSeverity::Error,
            message: message.into(),
            location: None,
        }
    }
}

/// Receiver for diagnostics emitted by markdown pipeline plugins.
///
/// Implementations are typically provided by the orchestrator (e.g. a
/// `Vec<MarkdownDiagnostic>` collector for tests, or a logger for production
/// builds). The pipeline itself does not collect diagnostics; each plugin
/// obtains a `&mut dyn DiagnosticsSink` from the `BuildContext` and pushes
/// to it directly.
pub trait DiagnosticsSink {
    /// Receive a single diagnostic.
    fn emit(&mut self, diagnostic: MarkdownDiagnostic);
}

/// A simple `Vec`-backed sink — useful for testing and for orchestrators
/// that want to batch-process diagnostics after the pipeline run.
#[derive(Debug, Default)]
pub struct CollectingSink {
    diagnostics: Vec<MarkdownDiagnostic>,
}

impl CollectingSink {
    /// Create a new empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Drain all accumulated diagnostics.
    pub fn take(&mut self) -> Vec<MarkdownDiagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// Borrow the accumulated diagnostics without draining.
    #[must_use]
    pub fn diagnostics(&self) -> &[MarkdownDiagnostic] {
        &self.diagnostics
    }
}

impl DiagnosticsSink for CollectingSink {
    fn emit(&mut self, diagnostic: MarkdownDiagnostic) {
        self.diagnostics.push(diagnostic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collecting_sink_receives_emitted_diagnostics() {
        let mut sink = CollectingSink::new();
        sink.emit(MarkdownDiagnostic::warning("test warning"));
        sink.emit(MarkdownDiagnostic::error("test error"));
        let diags = sink.take();
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].severity(), DiagnosticSeverity::Warning);
        assert_eq!(diags[1].severity(), DiagnosticSeverity::Error);
    }

    #[test]
    fn broken_link_variant_severity() {
        let d = MarkdownDiagnostic::BrokenLink {
            severity: DiagnosticSeverity::Warning,
            url: "missing.md".to_string(),
            location: None,
        };
        assert_eq!(d.severity(), DiagnosticSeverity::Warning);
    }

    #[test]
    fn legacy_generic_defaults_without_guessing_code_or_column_units() {
        let d: MarkdownDiagnostic = serde_json::from_value(serde_json::json!({
            "kind":"generic", "severity":"warning", "message":"imageDimensions: anything",
            "location":{"path":"page.mdx","line":2,"col":3}
        }))
        .unwrap();
        let record = d.to_build_diagnostic();
        assert_eq!(record.code, codes::GENERIC_MARKDOWN);
        assert_eq!(record.file.as_deref(), Some("page.mdx"));
        assert_eq!(record.line, Some(2));
        assert_eq!(record.byte_column, None);
        let explicit: MarkdownDiagnostic = serde_json::from_value(serde_json::json!({
            "kind":"generic", "severity":"info", "message":"changed prose", "code":"plugin/class",
            "location":{"path":"page.mdx","line":2,"col":3,"byteColumn":7}
        }))
        .unwrap();
        assert_eq!(explicit.code(), "plugin/class");
        assert_eq!(explicit.to_build_diagnostic().byte_column, Some(7));
        assert_eq!(
            serde_json::from_value::<MarkdownDiagnostic>(serde_json::to_value(&explicit).unwrap())
                .unwrap(),
            explicit
        );
    }

    #[test]
    fn broken_link_identity_and_generic_fallback_do_not_follow_prose() {
        let d = MarkdownDiagnostic::BrokenLink {
            severity: DiagnosticSeverity::Error,
            url: "#missing".into(),
            location: None,
        };
        assert_eq!(d.to_build_diagnostic().code, codes::BROKEN_LINK);
        assert_eq!(d.to_build_diagnostic().severity, DiagnosticSeverity::Error);
        assert_eq!(
            MarkdownDiagnostic::warning("arbitrary").code(),
            codes::GENERIC_MARKDOWN
        );
        assert_eq!(
            MarkdownDiagnostic::warning("changed prose")
                .with_code(codes::IMAGE_DIMENSIONS)
                .code(),
            codes::IMAGE_DIMENSIONS
        );
    }

    #[test]
    fn severity_ordering() {
        assert!(DiagnosticSeverity::Info < DiagnosticSeverity::Warning);
        assert!(DiagnosticSeverity::Warning < DiagnosticSeverity::Error);
    }
}
