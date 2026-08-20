//! Public TSX frontmatter surface when the optional compiler is disabled.
//!
//! The carrier and lint types stay available so downstream APIs remain
//! type-stable. Only static extraction is unavailable because it requires
//! SWC; callers receive an explicit capability error instead of a parse
//! failure or a silently incomplete result.

use serde_json::Value as JsonValue;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsxFrontmatter {
    pub frontmatter: JsonValue,
    pub extension: Option<String>,
    pub content_type: Option<String>,
    pub prerender: bool,
    pub default_export_param: DefaultExportFirstParam,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultExportFirstParam {
    Absent,
    Destructured,
    Plain(PlainFirstParam),
    Opaque,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainFirstParam {
    pub name: String,
    pub annotation_is_request: bool,
    pub body_uses_request_members: bool,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestParamTier {
    Strong,
    Heuristic,
}

const HEURISTIC_PARAM_NAMES: [&str; 2] = ["request", "req"];

impl DefaultExportFirstParam {
    #[must_use]
    pub fn request_param_tier(&self) -> Option<RequestParamTier> {
        let Self::Plain(plain) = self else {
            return None;
        };
        if plain.annotation_is_request
            || (plain.body_uses_request_members
                && HEURISTIC_PARAM_NAMES.contains(&plain.name.as_str()))
        {
            return Some(RequestParamTier::Strong);
        }
        HEURISTIC_PARAM_NAMES
            .contains(&plain.name.as_str())
            .then_some(RequestParamTier::Heuristic)
    }
}

#[must_use]
pub fn ssr_request_param_tier(
    prerender: bool,
    param: &DefaultExportFirstParam,
) -> Option<RequestParamTier> {
    (!prerender).then(|| param.request_param_tier()).flatten()
}

#[derive(Debug, Error)]
pub enum TsxFrontmatterError {
    #[error("{file}: TSX frontmatter extraction requires the `zfb-content/compiler` capability")]
    CompilerUnavailable { file: String },
    #[error("{file}: parse error: {message}")]
    Parse { file: String, message: String },
    #[error("{file}: missing required `export const frontmatter`")]
    MissingFrontmatter {
        file: String,
        prerender: bool,
        default_export_param: DefaultExportFirstParam,
    },
    #[error("{file}:{line}:{col}: duplicate top-level `export const {name}`")]
    DuplicateExport {
        file: String,
        name: String,
        line: usize,
        col: usize,
    },
    #[error(
        "{file}:{line}:{col}: non-literal value not allowed in `export const {export}` ({reason})"
    )]
    ComputedValue {
        file: String,
        export: String,
        reason: String,
        line: usize,
        col: usize,
    },
    #[error("{file}:{line}:{col}: `export const {export}` {reason}")]
    WrongShape {
        file: String,
        export: String,
        reason: String,
        line: usize,
        col: usize,
    },
}

pub fn extract(_source: &str, file_name: &str) -> Result<TsxFrontmatter, TsxFrontmatterError> {
    Err(TsxFrontmatterError::CompilerUnavailable {
        file: file_name.to_string(),
    })
}

#[must_use]
pub fn filename_extension_candidate(file_name: &str) -> Option<&str> {
    let base = file_name
        .rsplit_once('/')
        .map_or(file_name, |(_, rest)| rest);
    let stem = base.strip_suffix(".tsx")?;
    if stem.is_empty() {
        return None;
    }
    let candidate = &stem[stem.rfind('.')? + 1..];
    (!candidate.is_empty()).then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_reports_the_stable_capability_error() {
        let error = extract("export const frontmatter = {};", "page.tsx")
            .expect_err("compiler-off TSX extraction must be rejected");
        assert!(matches!(
            error,
            TsxFrontmatterError::CompilerUnavailable { ref file } if file == "page.tsx"
        ));
        assert_eq!(
            error.to_string(),
            "page.tsx: TSX frontmatter extraction requires the `zfb-content/compiler` capability"
        );
    }
}
