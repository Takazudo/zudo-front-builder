//! Canned CSS engine for tests of engine-independent pipeline behavior.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};

use crate::{CssEngine, CssEngineId, CssEngineOutput};

#[derive(Debug, Clone)]
pub struct StubCssEngine {
    output: Result<CssEngineOutput, String>,
    calls: Arc<Mutex<Vec<Vec<PathBuf>>>>,
}

impl StubCssEngine {
    pub fn new(css: impl Into<String>) -> Self {
        Self::with_output(CssEngineOutput::new(css, CssEngineId::new("stub", None)))
    }

    pub fn with_output(output: CssEngineOutput) -> Self {
        Self {
            output: Ok(output),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_error(message: impl Into<String>) -> Self {
        Self {
            output: Err(message.into()),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn calls(&self) -> Vec<Vec<PathBuf>> {
        self.calls
            .lock()
            .expect("stub engine call log poisoned")
            .clone()
    }
}

impl CssEngine for StubCssEngine {
    fn produce_utility_css(&self, sources: &[PathBuf]) -> Result<CssEngineOutput> {
        self.calls
            .lock()
            .map_err(|_| anyhow!("stub engine call log poisoned"))?
            .push(sources.to_vec());
        self.output.clone().map_err(|message| anyhow!(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CssDiagnostic, CssDiagnosticOrigin, CssDiagnosticSeverity, CssInputDependency,
        CssInputDependencyKind, PackageUrlAsset,
    };

    #[test]
    fn returns_canned_result_and_records_sources() {
        let mut output = CssEngineOutput::new(".a{}", CssEngineId::new("test", Some("1".into())));
        output.companions.push(PackageUrlAsset {
            filename: "asset-a.svg".into(),
            bytes: b"<svg/>".to_vec(),
        });
        output.input_dependencies.push(CssInputDependency {
            path: "a.css".into(),
            kind: CssInputDependencyKind::Stylesheet,
        });
        output.diagnostics.push(CssDiagnostic {
            severity: CssDiagnosticSeverity::Warning,
            code: "test".into(),
            message: "warning".into(),
            origin: CssDiagnosticOrigin::default(),
            candidate: None,
        });
        let engine = StubCssEngine::with_output(output);
        let sources = vec![PathBuf::from("page.tsx")];
        let result = engine.produce_utility_css(&sources).unwrap();
        assert_eq!(result.css, ".a{}");
        assert_eq!(result.companions.len(), 1);
        assert_eq!(result.input_dependencies.len(), 1);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(engine.calls(), vec![sources]);
    }

    #[test]
    fn returns_canned_error_and_records_sources() {
        let engine = StubCssEngine::with_error("expected failure");
        let sources = vec![PathBuf::from("page.tsx")];
        let error = engine.produce_utility_css(&sources).unwrap_err();
        assert_eq!(error.to_string(), "expected failure");
        assert_eq!(engine.calls(), vec![sources]);
    }
}
