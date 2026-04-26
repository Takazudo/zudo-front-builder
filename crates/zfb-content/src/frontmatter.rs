//! Frontmatter parser.
//!
//! Parses YAML frontmatter delimited by `---` markers at the very start of a
//! string. Supports both LF and CRLF line endings.

use thiserror::Error;

/// Result of a successful frontmatter parse.
#[derive(Debug, Clone)]
pub struct Parsed {
    /// Parsed YAML value. `Null` if there is no frontmatter or it is empty.
    pub frontmatter: serde_yaml::Value,
    /// The body text following the closing `---` delimiter.
    pub body: String,
    /// Byte offset within the original input where `body` begins.
    pub body_offset: usize,
}

/// Errors produced by [`parse`].
#[derive(Debug, Error)]
pub enum FrontmatterError {
    #[error("frontmatter unterminated: missing closing `---`")]
    Unterminated,
    #[error("invalid YAML in frontmatter: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

/// Parse a string that may begin with YAML frontmatter.
///
/// Recognised forms:
/// - `---\n<yaml>\n---\n<body>`
/// - `---\r\n<yaml>\r\n---\r\n<body>`
///
/// If the input does not begin with `---` followed by a line terminator, the
/// entire input is treated as body and `frontmatter` is set to `Null`.
pub fn parse(input: &str) -> Result<Parsed, FrontmatterError> {
    // Detect opening delimiter. Must be the very first three bytes followed by
    // either `\n` or `\r\n`. A plain `---` at EOF or followed by other text on
    // the same line is not considered a frontmatter opener.
    let after_open = if let Some(rest) = input.strip_prefix("---\n") {
        rest
    } else if let Some(rest) = input.strip_prefix("---\r\n") {
        rest
    } else {
        return Ok(Parsed {
            frontmatter: serde_yaml::Value::Null,
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
    let frontmatter = if trimmed.is_empty() {
        serde_yaml::Value::Null
    } else {
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

    #[test]
    fn standard_frontmatter_with_body() {
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
    fn no_frontmatter() {
        let input = "# Just markdown\n\nNo frontmatter here.\n";
        let parsed = parse(input).expect("parse ok");
        assert!(matches!(parsed.frontmatter, serde_yaml::Value::Null));
        assert_eq!(parsed.body, input);
        assert_eq!(parsed.body_offset, 0);
    }

    #[test]
    fn empty_frontmatter() {
        let input = "---\n---\nbody starts here\n";
        let parsed = parse(input).expect("parse ok");
        assert!(matches!(parsed.frontmatter, serde_yaml::Value::Null));
        assert_eq!(parsed.body, "body starts here\n");
        assert_eq!(&input[parsed.body_offset..], parsed.body);
    }

    #[test]
    fn crlf_line_endings() {
        let input = "---\r\ntitle: CRLF\r\ncount: 3\r\n---\r\nbody line\r\n";
        let parsed = parse(input).expect("parse ok");
        assert_eq!(parsed.frontmatter["title"].as_str(), Some("CRLF"));
        assert_eq!(parsed.frontmatter["count"].as_i64(), Some(3));
        assert_eq!(parsed.body, "body line\r\n");
        assert_eq!(&input[parsed.body_offset..], parsed.body);
    }

    #[test]
    fn malformed_yaml_returns_error() {
        // Unbalanced bracket -> serde_yaml will reject.
        let input = "---\ntitle: [unterminated\n---\nbody\n";
        let err = parse(input).expect_err("should fail");
        assert!(
            matches!(err, FrontmatterError::Yaml(_)),
            "expected Yaml error, got {err:?}",
        );
    }

    #[test]
    fn unterminated_frontmatter_returns_error() {
        let input = "---\ntitle: Forever\nbody but no closing\n";
        let err = parse(input).expect_err("should fail");
        assert!(matches!(err, FrontmatterError::Unterminated));
    }

    #[test]
    fn nested_yaml_list_and_map() {
        let input = "---\n\
            tags:\n  - rust\n  - markdown\n  - frontmatter\n\
            author:\n  name: Alice\n  handle: alice42\n\
            ---\n# Title\n";
        let parsed = parse(input).expect("parse ok");

        let tags = parsed.frontmatter["tags"]
            .as_sequence()
            .expect("tags is a sequence");
        let tag_strs: Vec<&str> = tags.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(tag_strs, vec!["rust", "markdown", "frontmatter"]);

        assert_eq!(parsed.frontmatter["author"]["name"].as_str(), Some("Alice"));
        assert_eq!(
            parsed.frontmatter["author"]["handle"].as_str(),
            Some("alice42"),
        );
        assert_eq!(parsed.body, "# Title\n");
    }
}
