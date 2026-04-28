//! Unified frontmatter extraction.
//!
//! One entry point — [`extract`] — dispatches by file extension:
//!
//! - `.md`, `.mdx` → parse YAML frontmatter delimited by `---` markers,
//!   convert the result to [`serde_json::Value`].
//! - `.tsx` → statically extract `export const frontmatter` (plus the
//!   optional `extension` / `contentType` siblings) via
//!   [`crate::tsx_frontmatter::extract`].
//!
//! The public surface is JSON-only — `serde_yaml::Value` is an
//! implementation detail of the YAML branch and never leaks across the
//! crate boundary. Schema validation downstream (e.g. `garde`) operates
//! on the normalized JSON.

use std::path::Path;

use serde_json::Value as JsonValue;
use thiserror::Error;

use crate::tsx_frontmatter::{self, TsxFrontmatter, TsxFrontmatterError};

/// Result of the YAML-only [`parse`] helper. Internal — the public
/// surface is [`UnifiedFrontmatter`].
#[derive(Debug, Clone)]
pub(crate) struct Parsed {
    /// Parsed frontmatter as JSON. `Null` if there is no frontmatter or
    /// it is empty. We deserialize directly into JSON so the public
    /// surface speaks one value type.
    pub frontmatter: JsonValue,
    /// The body text following the closing `---` delimiter.
    pub body: String,
    /// Byte offset within the original input where `body` begins.
    pub body_offset: usize,
}

/// Unified frontmatter result returned by [`extract`].
///
/// The shape is the same regardless of source kind so a content
/// collection can mix `.md`, `.mdx`, and `.tsx` entries without
/// branching on extension. Body fields are absent for `.tsx` because a
/// TSX page module has no separate markdown body.
#[derive(Debug, Clone)]
pub struct UnifiedFrontmatter {
    /// Frontmatter as JSON. `Null` if the source had no frontmatter.
    pub value: JsonValue,
    /// Body text after the closing `---` delimiter (markdown / MDX
    /// only). `None` for `.tsx` sources.
    pub body: Option<String>,
    /// Byte offset within the original source where `body` begins.
    /// `None` whenever `body` is `None`.
    pub body_offset: Option<usize>,
    /// `export const extension = "…"` literal value, when present.
    /// Only populated for `.tsx` sources.
    pub extension: Option<String>,
    /// `export const contentType = "…"` literal value, when present.
    /// Only populated for `.tsx` sources.
    pub content_type: Option<String>,
}

/// Errors produced by [`extract`] (and the internal YAML helper).
#[derive(Debug, Error)]
pub enum FrontmatterError {
    #[error("frontmatter unterminated: missing closing `---`")]
    Unterminated,
    #[error("invalid YAML in frontmatter: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("TSX frontmatter error: {0}")]
    Tsx(#[from] TsxFrontmatterError),
    #[error("unsupported file extension `.{0}` for frontmatter extraction (expected md, mdx, or tsx)")]
    UnsupportedExtension(String),
    #[error("missing file extension; cannot dispatch frontmatter extraction")]
    MissingExtension,
}

/// Extract frontmatter from `source`, dispatching on `path`'s extension.
///
/// Returns a [`UnifiedFrontmatter`] whose `value` field is always a
/// `serde_json::Value`. The two body fields and the two TSX-only string
/// fields are populated according to source kind:
///
/// | extension       | `value`            | `body`         | `extension` / `content_type` |
/// |-----------------|--------------------|----------------|------------------------------|
/// | `md`, `mdx`     | YAML→JSON          | `Some(body)`   | always `None`                |
/// | `tsx`           | TSX literal → JSON | `None`         | from sibling exports         |
/// | other / missing | error              | error          | error                        |
pub fn extract(path: &Path, source: &str) -> Result<UnifiedFrontmatter, FrontmatterError> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .ok_or(FrontmatterError::MissingExtension)?;

    match ext {
        "md" | "mdx" => {
            let parsed = parse(source)?;
            Ok(UnifiedFrontmatter {
                value: parsed.frontmatter,
                body: Some(parsed.body),
                body_offset: Some(parsed.body_offset),
                extension: None,
                content_type: None,
            })
        }
        "tsx" => {
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("<unknown>.tsx");
            let TsxFrontmatter {
                frontmatter,
                extension,
                content_type,
                // `prerender` is consumed downstream by the build
                // orchestrator (T6); the unified frontmatter carrier
                // does not need it (and remains shape-compatible with
                // markdown / MDX paths that have no such concept).
                prerender: _,
            } = tsx_frontmatter::extract(source, file_name)?;
            Ok(UnifiedFrontmatter {
                value: frontmatter,
                body: None,
                body_offset: None,
                extension,
                content_type,
            })
        }
        other => Err(FrontmatterError::UnsupportedExtension(other.to_string())),
    }
}

/// YAML-only frontmatter parser. Internal helper — external code
/// should call [`extract`] instead.
///
/// Recognised forms:
/// - `---\n<yaml>\n---\n<body>`
/// - `---\r\n<yaml>\r\n---\r\n<body>`
///
/// If the input does not begin with `---` followed by a line terminator,
/// the entire input is treated as body and `frontmatter` is set to
/// `Null`. The YAML is deserialized straight into `serde_json::Value`
/// so the resulting struct is already in the public dialect.
pub(crate) fn parse(input: &str) -> Result<Parsed, FrontmatterError> {
    // Detect opening delimiter. Must be the very first three bytes followed by
    // either `\n` or `\r\n`. A plain `---` at EOF or followed by other text on
    // the same line is not considered a frontmatter opener.
    let after_open = if let Some(rest) = input.strip_prefix("---\n") {
        rest
    } else if let Some(rest) = input.strip_prefix("---\r\n") {
        rest
    } else {
        return Ok(Parsed {
            frontmatter: JsonValue::Null,
            body: input.to_string(),
            body_offset: 0,
        });
    };

    let open_len = input.len() - after_open.len();

    // Find the closing delimiter. It must be a line consisting solely of `---`
    // followed by a line terminator OR end-of-input.
    let (yaml_str, body_offset) = match find_closing(after_open, open_len) {
        Some(found) => found,
        None => return Err(FrontmatterError::Unterminated),
    };

    let trimmed = yaml_str.trim();
    let frontmatter: JsonValue = if trimmed.is_empty() {
        JsonValue::Null
    } else {
        // serde_yaml can deserialize directly into serde_json::Value.
        // YAML scalar/seq/map shapes map cleanly onto JSON; non-string
        // keys (which JSON cannot represent) surface as a YAML error.
        serde_yaml::from_str(yaml_str).map_err(FrontmatterError::Yaml)?
    };

    let body = input[body_offset..].to_string();

    Ok(Parsed {
        frontmatter,
        body,
        body_offset,
    })
}

/// Search `after_open` for a line consisting only of `---`. Returns the YAML
/// slice (between the delimiters, excluding the closing newline of the YAML
/// block) and the body offset relative to the original input.
fn find_closing(after_open: &str, open_len: usize) -> Option<(&str, usize)> {
    let bytes = after_open.as_bytes();
    let mut line_start = 0usize;

    while line_start <= bytes.len() {
        // Find end of current line.
        let rel_end = bytes[line_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| line_start + p);

        let line_end = rel_end.unwrap_or(bytes.len());

        // Slice the line without trailing \n; also strip a single trailing \r.
        let mut line_slice_end = line_end;
        if line_slice_end > line_start && bytes[line_slice_end - 1] == b'\r' {
            line_slice_end -= 1;
        }
        let line = &after_open[line_start..line_slice_end];

        if line == "---" {
            // YAML content is everything before this line. Strip the trailing
            // line terminator from the YAML slice if present.
            let mut yaml_end = line_start;
            if yaml_end >= 1 && bytes[yaml_end - 1] == b'\n' {
                yaml_end -= 1;
                if yaml_end >= 1 && bytes[yaml_end - 1] == b'\r' {
                    yaml_end -= 1;
                }
            }
            let yaml_str = &after_open[..yaml_end];

            // Body begins after the closing `---` line and its terminator.
            let body_rel = if let Some(end) = rel_end {
                end + 1 // skip past the \n
            } else {
                bytes.len() // closing delimiter at EOF, no body
            };
            let body_offset = open_len + body_rel;
            return Some((yaml_str, body_offset));
        }

        match rel_end {
            Some(end) => line_start = end + 1,
            None => break,
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ---------- internal `parse` (YAML-only) ----------

    #[test]
    fn parse_standard_frontmatter_with_body() {
        let input = "---\ntitle: Hello\ndraft: false\n---\n# Body\n\nSome text.\n";
        let parsed = parse(input).expect("parse ok");
        assert_eq!(
            parsed.frontmatter["title"].as_str(),
            Some("Hello"),
            "title should parse",
        );
        assert_eq!(parsed.frontmatter["draft"].as_bool(), Some(false));
        assert_eq!(parsed.body, "# Body\n\nSome text.\n");
        assert_eq!(&input[parsed.body_offset..], parsed.body);
    }

    #[test]
    fn parse_no_frontmatter() {
        let input = "# Just markdown\n\nNo frontmatter here.\n";
        let parsed = parse(input).expect("parse ok");
        assert!(parsed.frontmatter.is_null());
        assert_eq!(parsed.body, input);
        assert_eq!(parsed.body_offset, 0);
    }

    #[test]
    fn parse_empty_frontmatter() {
        let input = "---\n---\nbody starts here\n";
        let parsed = parse(input).expect("parse ok");
        assert!(parsed.frontmatter.is_null());
        assert_eq!(parsed.body, "body starts here\n");
        assert_eq!(&input[parsed.body_offset..], parsed.body);
    }

    #[test]
    fn parse_crlf_line_endings() {
        let input = "---\r\ntitle: CRLF\r\ncount: 3\r\n---\r\nbody line\r\n";
        let parsed = parse(input).expect("parse ok");
        assert_eq!(parsed.frontmatter["title"].as_str(), Some("CRLF"));
        assert_eq!(parsed.frontmatter["count"].as_i64(), Some(3));
        assert_eq!(parsed.body, "body line\r\n");
        assert_eq!(&input[parsed.body_offset..], parsed.body);
    }

    #[test]
    fn parse_malformed_yaml_returns_error() {
        // Unbalanced bracket -> serde_yaml will reject.
        let input = "---\ntitle: [unterminated\n---\nbody\n";
        let err = parse(input).expect_err("should fail");
        assert!(
            matches!(err, FrontmatterError::Yaml(_)),
            "expected Yaml error, got {err:?}",
        );
    }

    #[test]
    fn parse_unterminated_frontmatter_returns_error() {
        let input = "---\ntitle: Forever\nbody but no closing\n";
        let err = parse(input).expect_err("should fail");
        assert!(matches!(err, FrontmatterError::Unterminated));
    }

    #[test]
    fn parse_nested_yaml_list_and_map() {
        let input = "---\n\
            tags:\n  - rust\n  - markdown\n  - frontmatter\n\
            author:\n  name: Alice\n  handle: alice42\n\
            ---\n# Title\n";
        let parsed = parse(input).expect("parse ok");

        let tags = parsed.frontmatter["tags"]
            .as_array()
            .expect("tags is an array");
        let tag_strs: Vec<&str> = tags.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(tag_strs, vec!["rust", "markdown", "frontmatter"]);

        assert_eq!(parsed.frontmatter["author"]["name"].as_str(), Some("Alice"));
        assert_eq!(
            parsed.frontmatter["author"]["handle"].as_str(),
            Some("alice42"),
        );
        assert_eq!(parsed.body, "# Title\n");
    }

    // ---------- public `extract` dispatcher ----------

    #[test]
    fn extract_md_returns_yaml_as_json() {
        let path = PathBuf::from("post.md");
        let src = "---\ntitle: Hello\ncount: 3\n---\nbody\n";
        let uf = extract(&path, src).expect("extract ok");
        assert_eq!(uf.value["title"], JsonValue::String("Hello".to_string()));
        assert_eq!(uf.value["count"], JsonValue::Number(3.into()));
        assert_eq!(uf.body.as_deref(), Some("body\n"));
        assert_eq!(uf.body_offset, Some(src.len() - "body\n".len()));
        assert!(uf.extension.is_none());
        assert!(uf.content_type.is_none());
    }

    #[test]
    fn extract_mdx_routes_through_yaml_branch() {
        let path = PathBuf::from("post.mdx");
        let src = "---\ntitle: MDX\n---\n<MyComp />\n";
        let uf = extract(&path, src).expect("extract ok");
        assert_eq!(uf.value["title"].as_str(), Some("MDX"));
        assert_eq!(uf.body.as_deref(), Some("<MyComp />\n"));
    }

    #[test]
    fn extract_tsx_pulls_export_const_frontmatter() {
        let path = PathBuf::from("page.tsx");
        let src = "export const frontmatter = { title: 'Tsx', count: 7 };\n\
                   export const extension = 'html';\n\
                   export default function Page() { return null; }\n";
        let uf = extract(&path, src).expect("extract ok");
        assert_eq!(uf.value["title"].as_str(), Some("Tsx"));
        assert_eq!(uf.value["count"].as_i64(), Some(7));
        assert!(uf.body.is_none(), "tsx has no body");
        assert!(uf.body_offset.is_none());
        assert_eq!(uf.extension.as_deref(), Some("html"));
        assert!(uf.content_type.is_none());
    }

    #[test]
    fn extract_tsx_propagates_tsx_error() {
        let path = PathBuf::from("page.tsx");
        // No frontmatter export → TsxFrontmatterError::MissingFrontmatter.
        let src = "export default function Page() { return null; }\n";
        let err = extract(&path, src).expect_err("should fail");
        assert!(
            matches!(err, FrontmatterError::Tsx(_)),
            "expected Tsx error, got {err:?}",
        );
    }

    #[test]
    fn extract_unsupported_extension_errors() {
        let path = PathBuf::from("notes.markdown");
        let err = extract(&path, "---\ntitle: x\n---\n").expect_err("should fail");
        match err {
            FrontmatterError::UnsupportedExtension(ext) => assert_eq!(ext, "markdown"),
            other => unreachable!("expected UnsupportedExtension, got {other:?}"),
        }
    }

    #[test]
    fn extract_missing_extension_errors() {
        let path = PathBuf::from("README");
        let err = extract(&path, "anything").expect_err("should fail");
        assert!(matches!(err, FrontmatterError::MissingExtension));
    }
}
