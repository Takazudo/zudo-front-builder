//! Data returned by one CSS engine invocation.

use std::path::PathBuf;

use crate::PackageUrlAsset;

/// All output from a single engine call. No companion state is retained by the engine.
#[derive(Debug, Clone)]
pub struct CssEngineOutput {
    pub css: String,
    pub companions: Vec<PackageUrlAsset>,
    pub input_dependencies: Vec<CssInputDependency>,
    pub diagnostics: Vec<CssDiagnostic>,
    pub provenance: Option<CssProvenance>,
    pub engine: CssEngineId,
}

impl CssEngineOutput {
    pub fn new(css: impl Into<String>, engine: CssEngineId) -> Self {
        Self {
            css: css.into(),
            companions: Vec::new(),
            input_dependencies: Vec::new(),
            diagnostics: Vec::new(),
            provenance: None,
            engine,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssInputDependency {
    pub path: PathBuf,
    pub kind: CssInputDependencyKind,
}

/// Classifies authored CSS inputs for later dev invalidation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssInputDependencyKind {
    Stylesheet,
    PackageManifest,
    Asset,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CssDiagnostic {
    pub severity: CssDiagnosticSeverity,
    pub code: String,
    pub message: String,
    pub origin: CssDiagnosticOrigin,
    pub candidate: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CssDiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct CssDiagnosticOrigin {
    pub path: Option<PathBuf>,
    /// One-based line in the original source.
    pub line: Option<usize>,
    /// One-based byte column in that line.
    pub column: Option<usize>,
    /// Byte offset from the start of the source, kept apart from line and column.
    pub byte_offset: Option<usize>,
    /// Where a candidate without a source position came from, such as
    /// `manifest widgets[2]` or `wind.safelist.app[0]`.
    pub label: Option<String>,
}

impl CssDiagnosticOrigin {
    /// `path:line:column`, `path`, or the label, whichever is known.
    pub fn location(&self) -> Option<String> {
        let place = match (&self.path, self.line, self.column) {
            (Some(path), Some(line), Some(column)) => {
                Some(format!("{}:{line}:{column}", path.display()))
            }
            (Some(path), _, _) => Some(path.display().to_string()),
            _ => None,
        };
        match (place, &self.label) {
            (Some(place), Some(label)) => Some(format!("{place} ({label})")),
            (Some(place), None) => Some(place),
            (None, label) => label.clone(),
        }
    }
}

impl CssDiagnostic {
    /// `location: CODE candidate: message`, omitting the parts that are unknown.
    pub fn render(&self) -> String {
        let mut line = String::new();
        if let Some(location) = self.origin.location() {
            line.push_str(&location);
            line.push_str(": ");
        }
        line.push_str(&self.code);
        if let Some(candidate) = &self.candidate {
            line.push(' ');
            line.push_str(candidate);
        }
        line.push_str(": ");
        line.push_str(&self.message);
        line
    }
}

/// Drop repeats of the same code, candidate, source and span, keeping the
/// first occurrence's order; distinct occurrences all survive.
pub fn dedup_diagnostics(diagnostics: Vec<CssDiagnostic>) -> Vec<CssDiagnostic> {
    let mut seen = std::collections::HashSet::new();
    diagnostics
        .into_iter()
        .filter(|diagnostic| {
            seen.insert((
                diagnostic.code.clone(),
                diagnostic.candidate.clone(),
                diagnostic.origin.clone(),
            ))
        })
        .collect()
}

/// Error-severity wind diagnostics that stopped a compile, kept structured so
/// a caller can render or serialize them instead of parsing a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindDiagnosticsError {
    pub diagnostics: Vec<CssDiagnostic>,
}

impl std::fmt::Display for WindDiagnosticsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.diagnostics.len();
        write!(
            formatter,
            "wind CSS failed with {count} error{}:",
            if count == 1 { "" } else { "s" }
        )?;
        for diagnostic in &self.diagnostics {
            write!(formatter, "\n  {}", diagnostic.render())?;
        }
        Ok(())
    }
}

impl std::error::Error for WindDiagnosticsError {}

/// Identity of generated CSS rules; v1 has no generated source map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssProvenance {
    pub source_id: String,
    pub kind: CssProvenanceKind,
    pub spec_version: u32,
    pub spec_revision: u32,
    pub map: Option<String>,
    /// Canonical authored stylesheet inputs when generated and authored CSS are combined.
    pub authored_stylesheets: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssProvenanceKind {
    Generated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssEngineId {
    pub name: String,
    pub version: Option<String>,
}

impl CssEngineId {
    pub fn new(name: impl Into<String>, version: Option<String>) -> Self {
        Self {
            name: name.into(),
            version,
        }
    }
}
