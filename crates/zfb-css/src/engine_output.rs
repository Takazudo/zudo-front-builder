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

/// Only stylesheets are watched by the later dev integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssInputDependencyKind {
    Stylesheet,
    Asset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssDiagnostic {
    pub severity: CssDiagnosticSeverity,
    pub code: String,
    pub message: String,
    pub origin: CssDiagnosticOrigin,
    pub candidate: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssDiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CssDiagnosticOrigin {
    pub path: Option<PathBuf>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

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
