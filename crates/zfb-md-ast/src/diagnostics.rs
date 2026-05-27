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

/// Severity of a markdown diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticSeverity {
    /// Informational note — does not block the build.
    Info,
    /// Warning — build succeeds but the issue should be investigated.
    Warning,
    /// Error — indicates a problem that should be treated as a build failure.
    Error,
}

/// Source location within a markdown file (1-based line/column).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    /// Path of the source file that produced the diagnostic.
    pub path: Option<PathBuf>,
    /// 1-based line number, if known.
    pub line: Option<u32>,
    /// 1-based column number, if known.
    pub col: Option<u32>,
}

impl SourceLocation {
    /// Location with path only (no line/column information).
    #[must_use]
    pub fn from_path(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            line: None,
            col: None,
        }
    }
}

/// A single diagnostic produced by a markdown pipeline plugin.
///
/// Wave-6 plugins emit these through a [`DiagnosticsSink`]; the orchestrator
/// collects them and can report errors, emit warnings, or suppress info
/// messages depending on build configuration.
#[derive(Debug, Clone, PartialEq)]
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
        /// Severity level.
        severity: DiagnosticSeverity,
        /// Human-readable message describing the issue.
        message: String,
        /// Location in the source file, if available.
        location: Option<SourceLocation>,
    },
}

impl MarkdownDiagnostic {
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
            severity: DiagnosticSeverity::Warning,
            message: message.into(),
            location: None,
        }
    }

    /// Convenience constructor for a generic error.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        MarkdownDiagnostic::Generic {
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
    fn severity_ordering() {
        assert!(DiagnosticSeverity::Info < DiagnosticSeverity::Warning);
        assert!(DiagnosticSeverity::Warning < DiagnosticSeverity::Error);
    }
}
