//! Shared build diagnostic contract. See `research/3728-build-diagnostics-contract.md`.
//!
//! Codes are assigned by producers, never inferred from message text. This
//! module has no output or collection side effects; build/dev own their sinks.

use serde::{Deserialize, Serialize};

/// Stable zfb-owned diagnostic classes. Never reuse a retired code.
pub mod codes {
    pub const BROKEN_LINK: &str = "ZB001";
    pub const IMAGE_DIMENSIONS: &str = "ZB002";
    pub const DROPPED_CSS: &str = "ZB003";
    pub const DANGLING_SYMLINK: &str = "ZB004";
    pub const FOREIGN_JSX_PRAGMA: &str = "ZB005";
    pub const PLUGIN_HOST_OUTPUT: &str = "ZB006";
    pub const UNSUPPORTED_COLLECTION_FILE: &str = "ZB007";
    pub const MIRROR_PREPROCESSING: &str = "ZB008";
    pub const GENERIC_MARKDOWN: &str = "ZB009";
    pub const PLUGIN_LOG: &str = "ZB010";
    pub const TRANSCLUSION: &str = "ZB011";
    pub const DIRECTIVE: &str = "ZB012";
    pub const INVALID_MDX_SPREAD: &str = "ZB013";
    // Reserved for #3730's origin-assigned Astro component-attribute warning.
    pub const ASTRO_CLIENT_ATTRIBUTE: &str = "ZB014";
    // Origin assignments reserved by the fresh-base inventory for #3729.
    pub const ISLAND_SCAN: &str = "ZB015";
    pub const NO_ISLAND_TARGETS: &str = "ZB016";
    pub const EMBEDDED_TOOL_FALLBACK: &str = "ZB017";
    pub const PRERENDER_EXTRACTION: &str = "ZB018";
    pub const SSR_ROUTE_CONTRACT: &str = "ZB019";
    pub const EMPTY_ROUTES: &str = "ZB020";
    pub const ADAPTER_STDERR: &str = "ZB021";
    pub const INVALID_ASSET_OUTPUT: &str = "ZB022";
    pub const DEFERRED_DYNAMIC_ROUTE: &str = "ZB023";
    pub const CONTENT_SNAPSHOT: &str = "ZB024";
    pub const RENDER_ARTIFACT: &str = "ZB025";
    pub const SNAPSHOT_PROBE: &str = "ZB026";
    pub const PUBLIC_COPY: &str = "ZB027";
    pub const DEV_ISLAND_REBUNDLE: &str = "ZB028";
    pub const DEV_CLIENT_SCRIPT_CLEANUP: &str = "ZB029";
    pub const DEV_CLIENT_SCRIPT_COLLISION: &str = "ZB030";
    pub const MARKDOWN_FRONTMATTER_FALLBACK: &str = "ZB031";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

/// One occurrence. All positions refer to authored source, never generated code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildDiagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
    /// Opaque origin identity (e.g. a Wind source ID or `plugin:<name>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Source path as supplied by the producer; absence is not the project root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// One-based source line, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// One-based UTF-8 byte column. Never substitute a character/UTF-16 column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_column: Option<u32>,
}

impl BuildDiagnostic {
    pub fn new(
        code: impl Into<String>,
        severity: DiagnosticSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            severity,
            message: message.into(),
            source_id: None,
            file: None,
            line: None,
            byte_column: None,
        }
    }

    /// Shared plain-text representation, including the historical warning prefix.
    /// Status/UI messages use their existing separate formatters.
    pub fn render(&self) -> String {
        let level = match self.severity {
            DiagnosticSeverity::Info => "info",
            DiagnosticSeverity::Warning => "warn",
            DiagnosticSeverity::Error => "error",
        };
        let mut result = format!("zfb {level}: {} ", self.code);
        if let Some(place) = self.file.as_ref().or(self.source_id.as_ref()) {
            result.push_str(place);
            if let Some(line) = self.line {
                result.push_str(&format!(":{line}"));
                if let Some(column) = self.byte_column {
                    result.push_str(&format!(":{column}"));
                }
            }
            result.push_str(": ");
        }
        result.push_str(&self.message);
        result
    }
}

pub const BUILD_DIAGNOSTICS_SCHEMA_VERSION: u32 = 1;

/// A failed or interrupted build can only report diagnostics observed so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildDiagnosticStatus {
    /// All build phases and diagnostic producers finished successfully.
    Success,
    /// Build returned an error. The diagnostics list may be partial.
    Failed,
    /// Collection did not finish (for example, a recoverable interruption).
    Incomplete,
}

/// Version belongs to the report envelope, not each repeated record.
/// No default status: the command must explicitly describe its outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildDiagnosticsReport {
    pub schema_version: u32,
    pub command: String,
    pub status: BuildDiagnosticStatus,
    pub diagnostics: Vec<BuildDiagnostic>,
}

impl BuildDiagnosticsReport {
    pub fn new(status: BuildDiagnosticStatus, diagnostics: Vec<BuildDiagnostic>) -> Self {
        Self {
            schema_version: BUILD_DIAGNOSTICS_SCHEMA_VERSION,
            command: "build".into(),
            status,
            diagnostics,
        }
    }
}

/// Optional additions to the existing plugin log envelope. Old plugins omit
/// all fields. `column` from old or third-party payloads is deliberately not
/// aliased to `byteColumn`: its units are unknown.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDiagnosticMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_column: Option<u32>,
}

impl PluginDiagnosticMetadata {
    pub fn diagnostic(
        &self,
        severity: DiagnosticSeverity,
        message: impl Into<String>,
    ) -> BuildDiagnostic {
        let code = self
            .code
            .as_deref()
            .filter(|code| !code.trim().is_empty())
            .unwrap_or(codes::PLUGIN_LOG);
        BuildDiagnostic {
            code: code.into(),
            severity,
            message: message.into(),
            source_id: self.source_id.clone(),
            file: self.file.clone(),
            line: self.line,
            byte_column: self.byte_column,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn version_one_empty_and_failed_reports_are_explicit() {
        for (status, label) in [
            (BuildDiagnosticStatus::Success, "success"),
            (BuildDiagnosticStatus::Failed, "failed"),
            (BuildDiagnosticStatus::Incomplete, "incomplete"),
        ] {
            assert_eq!(
                serde_json::to_value(BuildDiagnosticsReport::new(status, vec![])).unwrap(),
                json!({"schemaVersion":1,"command":"build","status":label,"diagnostics":[]})
            );
        }
        assert!(serde_json::from_value::<BuildDiagnosticsReport>(
            json!({"schemaVersion":1,"command":"build","diagnostics":[]})
        )
        .is_err());
    }

    #[test]
    fn record_serialization_preserves_occurrences_and_optional_source() {
        let mut d = BuildDiagnostic::new(
            codes::BROKEN_LINK,
            DiagnosticSeverity::Warning,
            "broken link: #missing",
        );
        assert_eq!(
            serde_json::to_value(&d).unwrap(),
            json!({"code":"ZB001","severity":"warning","message":"broken link: #missing"})
        );
        d.file = Some("pages/日本語.mdx".into());
        d.source_id = Some("app:pages/日本語.mdx".into());
        d.line = Some(2);
        d.byte_column = Some(7);
        let report = BuildDiagnosticsReport::new(BuildDiagnosticStatus::Failed, vec![d.clone(), d]);
        let serialized = serde_json::to_value(&report).unwrap();
        assert_eq!(serialized["diagnostics"].as_array().unwrap().len(), 2);
        assert_eq!(serialized["diagnostics"][0]["byteColumn"], 7);
        assert_eq!(
            serde_json::from_value::<BuildDiagnosticsReport>(serialized).unwrap(),
            report
        );
    }

    #[test]
    fn plugin_additions_are_optional_and_legacy_column_is_not_bytes() {
        let old: PluginDiagnosticMetadata = serde_json::from_value(json!({"column":9})).unwrap();
        let d = old.diagnostic(DiagnosticSeverity::Info, "plain CSS import was dropped");
        assert_eq!(d.code, codes::PLUGIN_LOG); // no message-based classification
        assert_eq!(d.byte_column, None);
        for code in [json!(null), json!(""), json!("  ")] {
            let meta: PluginDiagnosticMetadata =
                serde_json::from_value(json!({"code":code})).unwrap();
            assert_eq!(
                meta.diagnostic(DiagnosticSeverity::Warning, "m").code,
                codes::PLUGIN_LOG
            );
        }
        let new: PluginDiagnosticMetadata = serde_json::from_value(json!({"code":"my-plugin/asset","file":"日本.ts","line":3,"column":2,"byteColumn":4,"sourceId":"custom"})).unwrap();
        let d = new.diagnostic(DiagnosticSeverity::Warning, "missing asset");
        assert_eq!(
            d.render(),
            "zfb warn: my-plugin/asset 日本.ts:3:4: missing asset"
        );
        assert_eq!(d.source_id.as_deref(), Some("custom"));
    }
}
