//! GitHub-style autolinks: `#123`, `user/repo#456`, and commit-SHA references.
//!
//! Rust port of the [`remark-github`](https://www.npmjs.com/package/remark-github)
//! plugin. Rewrites bare issue/PR references and commit-SHA hashes found in
//! plain-text hast nodes into `<a href="…">` elements pointing at the
//! configured GitHub repository.
//!
//! # Recognised patterns (in matching order)
//!
//! | Pattern | Example | URL shape |
//! |---------|---------|-----------|
//! | Cross-repo issue/PR | `user/repo#456` | `https://github.com/user/repo/issues/456` |
//! | Bare issue/PR | `#123` | `https://github.com/{repo}/issues/123` |
//! | Commit SHA | 7–40 hex chars | `https://github.com/{repo}/commit/{sha}` |
//!
//! Matching is done in that order (longest-first) so `user/repo#7` is never
//! partially eaten by the bare-`#NNN` arm.
//!
//! # Exclusions
//!
//! Text content inside `<code>`, `<pre>`, or `<a>` elements is never rewritten.
//! This preserves code spans, fenced code blocks, and existing hyperlinks.
//!
//! # Config
//!
//! Wire via `features.githubAutolinks: { repo: "owner/repo" }` in
//! `zfb.config.ts`. The `repo` field is required — if absent, the feature is
//! silently disabled (see `register_features` in `zfb-content::pipeline`).
//!
//! # Wave 5 (#574)
//!
//! Initial port. Operates as a hast visitor so it runs after mdast→hast
//! conversion and sees the final element tree. This matches the hast phase
//! described in the sub-issue task brief.

use zfb_md_ast::{HastNode, HastVisitor};

// ── Pattern matching helpers ─────────────────────────────────────────────────

/// Returns true if `c` is a lowercase hex digit (`[0-9a-f]`).
fn is_hex_lower(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, 'a'..='f')
}

/// Returns true if `c` is a word character (`[A-Za-z0-9_-]`).
///
/// Used as a boundary check: a match that is immediately preceded or followed
/// by a word character is not an isolated reference.
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Returns true if `c` is valid in a GitHub owner/repo slug (`[A-Za-z0-9_.-]`).
fn is_slug_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// A single segment of text, either plain or an autolink.
#[derive(Debug, PartialEq, Eq)]
enum Segment {
    /// Verbatim text with no rewrite.
    Plain(String),
    /// `owner/repo#NNN` — cross-repo issue/PR reference.
    CrossRepoIssue {
        owner: String,
        repo: String,
        number: u64,
        /// Original text (e.g. `user/repo#123`) for link display.
        raw: String,
    },
    /// `#NNN` — bare issue/PR reference against the configured repo.
    BareIssue { number: u64, raw: String },
    /// 7–40 lowercase hex char SHA, with word boundaries on both sides.
    CommitSha { sha: String },
}

/// Parse `text` into a sequence of [`Segment`]s.
///
/// Segments are produced left-to-right. Each character is consumed at most
/// once. The function never panics on arbitrary input.
fn parse_segments(text: &str) -> Vec<Segment> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut segments: Vec<Segment> = Vec::new();
    let mut plain_start = 0usize;
    let mut i = 0usize;

    // Helper: flush accumulated plain text up to (but not including) `end`.
    macro_rules! flush_plain {
        ($end:expr) => {
            if plain_start < $end {
                let s: String = chars[plain_start..$end].iter().collect();
                segments.push(Segment::Plain(s));
            }
        };
    }

    while i < n {
        let c = chars[i];

        // ── Cross-repo reference: `owner/repo#NNN` ──────────────────────────
        // A slug char at position i, preceded by a non-word-char (or SOT),
        // followed eventually by `/slug#digits`. We only proceed when we can
        // scan a full `owner/repo#NNN` from here.
        if is_slug_char(c) && !c.is_ascii_digit() {
            // Check left boundary: must be start-of-text or a non-word char.
            let left_ok = i == 0 || !is_word_char(chars[i - 1]);
            if left_ok {
                // Scan owner slug.
                let owner_start = i;
                let mut j = i;
                while j < n && is_slug_char(chars[j]) {
                    j += 1;
                }
                // After owner slug there must be a `/`.
                if j < n && chars[j] == '/' && j > owner_start {
                    let repo_start = j + 1;
                    let mut k = repo_start;
                    while k < n && is_slug_char(chars[k]) {
                        k += 1;
                    }
                    // After repo slug there must be `#`.
                    if k < n && chars[k] == '#' && k > repo_start {
                        let num_start = k + 1;
                        let mut m = num_start;
                        while m < n && chars[m].is_ascii_digit() {
                            m += 1;
                        }
                        // Must have at least one digit, and right boundary
                        // must be end-of-text or non-word char.
                        if m > num_start && (m == n || !is_word_char(chars[m])) {
                            let owner: String = chars[owner_start..j].iter().collect();
                            let repo_str: String = chars[repo_start..k].iter().collect();
                            let num_str: String = chars[num_start..m].iter().collect();
                            if let Ok(number) = num_str.parse::<u64>() {
                                flush_plain!(i);
                                let raw: String = chars[i..m].iter().collect();
                                segments.push(Segment::CrossRepoIssue {
                                    owner,
                                    repo: repo_str,
                                    number,
                                    raw,
                                });
                                plain_start = m;
                                i = m;
                                continue;
                            }
                        }
                    }
                }
            }
        }

        // ── Bare issue reference: `#NNN` ─────────────────────────────────────
        // `#` preceded by a non-word, non-`/` char (or SOT), followed by digits.
        if c == '#' {
            let left_ok = i == 0
                || (!is_word_char(chars[i - 1]) && chars[i - 1] != '/');
            if left_ok {
                let num_start = i + 1;
                let mut j = num_start;
                while j < n && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j > num_start && (j == n || !is_word_char(chars[j])) {
                    let num_str: String = chars[num_start..j].iter().collect();
                    if let Ok(number) = num_str.parse::<u64>() {
                        flush_plain!(i);
                        let raw: String = chars[i..j].iter().collect();
                        segments.push(Segment::BareIssue { number, raw });
                        plain_start = j;
                        i = j;
                        continue;
                    }
                }
            }
        }

        // ── Commit SHA: 7–40 lowercase hex chars with word boundaries ─────────
        if is_hex_lower(c) {
            let left_ok = i == 0 || !is_word_char(chars[i - 1]);
            if left_ok {
                let sha_start = i;
                let mut j = i;
                while j < n && is_hex_lower(chars[j]) {
                    j += 1;
                }
                let len = j - sha_start;
                let right_ok = j == n || !is_word_char(chars[j]);
                if len >= 7 && len <= 40 && right_ok {
                    // Additionally: must not also be a decimal digit run
                    // (all digits = likely a plain number, not a SHA).
                    let all_digits = chars[sha_start..j].iter().all(|ch| ch.is_ascii_digit());
                    if !all_digits {
                        flush_plain!(i);
                        let sha: String = chars[sha_start..j].iter().collect();
                        segments.push(Segment::CommitSha { sha });
                        plain_start = j;
                        i = j;
                        continue;
                    }
                }
            }
        }

        i += 1;
    }

    // Flush any remaining plain text.
    flush_plain!(n);

    segments
}

/// Convert a slice of [`Segment`]s into a vec of [`HastNode`]s, using
/// `repo` as the default GitHub repository (`owner/repo`).
fn segments_to_hast(segments: Vec<Segment>, repo: &str) -> Vec<HastNode> {
    segments
        .into_iter()
        .map(|seg| match seg {
            Segment::Plain(s) => HastNode::Text(s),
            Segment::CrossRepoIssue {
                owner,
                repo: repo_name,
                number,
                raw,
            } => {
                let href = format!(
                    "https://github.com/{owner}/{repo_name}/issues/{number}"
                );
                HastNode::Element {
                    tag: "a".to_string(),
                    attrs: vec![("href".to_string(), href)],
                    children: vec![HastNode::Text(raw)],
                    void: false,
                }
            }
            Segment::BareIssue { number, raw } => {
                let href = format!("https://github.com/{repo}/issues/{number}");
                HastNode::Element {
                    tag: "a".to_string(),
                    attrs: vec![("href".to_string(), href)],
                    children: vec![HastNode::Text(raw)],
                    void: false,
                }
            }
            Segment::CommitSha { sha } => {
                let href = format!("https://github.com/{repo}/commit/{sha}");
                HastNode::Element {
                    tag: "a".to_string(),
                    attrs: vec![("href".to_string(), href)],
                    children: vec![HastNode::Text(sha.clone())],
                    void: false,
                }
            }
        })
        .collect()
}

/// Rewrite text nodes inside `node` (recursively) into autolink `<a>` elements.
///
/// Recursion is skipped for `<code>`, `<pre>`, and `<a>` elements:
/// - `<code>` — inline code spans.
/// - `<pre>` — fenced code blocks (which contain `<code>` as a child).
/// - `<a>` — avoids double-wrapping existing hyperlinks.
fn rewrite_node(node: &mut HastNode, repo: &str) {
    match node {
        HastNode::Root { children } => {
            let new_children = rewrite_children(std::mem::take(children), repo);
            *children = new_children;
        }
        HastNode::Element { tag, children, .. } => {
            // Skip code spans, code blocks, and existing links.
            if matches!(tag.as_str(), "code" | "pre" | "a") {
                return;
            }
            let new_children = rewrite_children(std::mem::take(children), repo);
            *children = new_children;
        }
        // Text nodes are handled by the parent's rewrite_children call.
        // Leaf nodes with no children need no action.
        HastNode::Text(_)
        | HastNode::Raw(_)
        | HastNode::JsxRaw(_)
        | HastNode::Comment(_) => {}
    }
}

/// Process a children vec: replace any `Text` node that contains autolink
/// patterns with the corresponding link elements; recurse into elements.
fn rewrite_children(children: Vec<HastNode>, repo: &str) -> Vec<HastNode> {
    let mut result: Vec<HastNode> = Vec::with_capacity(children.len());
    for child in children {
        match child {
            HastNode::Text(text) => {
                let segments = parse_segments(&text);
                // Fast path: single plain segment — no rewrite needed.
                let only_plain = segments.len() == 1
                    && matches!(segments.first(), Some(Segment::Plain(_)));
                if only_plain {
                    result.push(HastNode::Text(text));
                    continue;
                }
                result.extend(segments_to_hast(segments, repo));
            }
            mut other => {
                rewrite_node(&mut other, repo);
                result.push(other);
            }
        }
    }
    result
}

// ── Visitor ──────────────────────────────────────────────────────────────────

/// Hast visitor that rewrites GitHub-style autolinks in plain text nodes.
///
/// Configured with a GitHub `repo` string (`owner/repo`) that is used to
/// build the URLs for bare `#NNN` issue references and commit SHA links.
/// Cross-repo `user/repo#NNN` references always use the repo name embedded
/// in the reference, not the configured repo.
///
/// Implements [`HastVisitor`] — plug into the pipeline's hast phase via
/// `register_features` in `zfb-content::pipeline`.
pub struct GithubAutolinksPlugin {
    repo: String,
}

impl GithubAutolinksPlugin {
    /// Create a new plugin instance for the given GitHub `repo` (`owner/repo`).
    #[must_use]
    pub fn new(repo: impl Into<String>) -> Self {
        Self { repo: repo.into() }
    }
}

impl HastVisitor for GithubAutolinksPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        rewrite_node(node, &self.repo);
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_segments ────────────────────────────────────────────────────

    #[test]
    fn plain_text_is_a_single_plain_segment() {
        let segs = parse_segments("hello world");
        assert_eq!(segs, vec![Segment::Plain("hello world".to_string())]);
    }

    #[test]
    fn bare_issue_ref_parsed() {
        let segs = parse_segments("see #123 for details");
        assert_eq!(
            segs,
            vec![
                Segment::Plain("see ".to_string()),
                Segment::BareIssue {
                    number: 123,
                    raw: "#123".to_string()
                },
                Segment::Plain(" for details".to_string()),
            ]
        );
    }

    #[test]
    fn cross_repo_ref_parsed() {
        let segs = parse_segments("user/repo#456");
        assert_eq!(
            segs,
            vec![Segment::CrossRepoIssue {
                owner: "user".to_string(),
                repo: "repo".to_string(),
                number: 456,
                raw: "user/repo#456".to_string()
            }]
        );
    }

    #[test]
    fn sha_parsed() {
        let segs = parse_segments("commit abc1234");
        assert_eq!(
            segs,
            vec![
                Segment::Plain("commit ".to_string()),
                Segment::CommitSha {
                    sha: "abc1234".to_string()
                },
            ]
        );
    }

    #[test]
    fn sha_40_chars_parsed() {
        let sha = "a".repeat(40);
        let input = format!("see {sha} now");
        let segs = parse_segments(&input);
        assert!(
            segs.iter()
                .any(|s| matches!(s, Segment::CommitSha { sha: sh } if sh == &"a".repeat(40))),
            "40-char hex SHA should be parsed: {segs:?}"
        );
    }

    #[test]
    fn sha_41_chars_not_parsed() {
        // 41 chars: too long to be a SHA.
        let sha = "a".repeat(41);
        let input = format!("see {sha} now");
        let segs = parse_segments(&input);
        assert!(
            !segs.iter().any(|s| matches!(s, Segment::CommitSha { .. })),
            "41-char hex string must NOT be parsed as SHA: {segs:?}"
        );
    }

    #[test]
    fn sha_6_chars_not_parsed() {
        // 6 chars: too short.
        let segs = parse_segments("see abc123 now");
        assert!(
            !segs.iter().any(|s| matches!(s, Segment::CommitSha { .. })),
            "6-char hex string must NOT be parsed as SHA"
        );
    }

    #[test]
    fn all_digits_sha_not_parsed() {
        // A run like `1234567` is digits-only and should not become a SHA.
        let segs = parse_segments("ref 1234567 done");
        assert!(
            !segs.iter().any(|s| matches!(s, Segment::CommitSha { .. })),
            "all-digit run must not become a SHA"
        );
    }

    #[test]
    fn bare_issue_at_start_of_text() {
        let segs = parse_segments("#1 is important");
        assert!(
            segs.iter()
                .any(|s| matches!(s, Segment::BareIssue { number: 1, .. })),
            "bare issue at start must parse"
        );
    }

    #[test]
    fn bare_issue_at_end_of_text() {
        let segs = parse_segments("see #99");
        assert!(
            segs.iter()
                .any(|s| matches!(s, Segment::BareIssue { number: 99, .. })),
            "bare issue at end must parse"
        );
    }

    #[test]
    fn cross_repo_precedes_bare_issue() {
        // `foo/bar#7` must become a CrossRepoIssue, not a bare issue for `#7`.
        let segs = parse_segments("foo/bar#7");
        assert_eq!(
            segs,
            vec![Segment::CrossRepoIssue {
                owner: "foo".to_string(),
                repo: "bar".to_string(),
                number: 7,
                raw: "foo/bar#7".to_string()
            }]
        );
    }

    #[test]
    fn hash_not_preceded_by_slash_is_bare_issue() {
        // When `#` is preceded by a space (not a `/`), it must be a bare issue.
        let segs = parse_segments("see #5");
        assert!(segs.iter().any(|s| matches!(s, Segment::BareIssue { number: 5, .. })));
    }

    // ── segments_to_hast ──────────────────────────────────────────────────

    #[test]
    fn bare_issue_becomes_anchor_with_default_repo() {
        let segs = vec![Segment::BareIssue {
            number: 123,
            raw: "#123".to_string(),
        }];
        let nodes = segments_to_hast(segs, "owner/myrepo");
        assert_eq!(nodes.len(), 1);
        let HastNode::Element { tag, attrs, children, .. } = &nodes[0] else {
            panic!("expected Element");
        };
        assert_eq!(tag, "a");
        assert!(attrs.iter().any(|(k, v)| k == "href"
            && v == "https://github.com/owner/myrepo/issues/123"));
        assert_eq!(children, &vec![HastNode::Text("#123".to_string())]);
    }

    #[test]
    fn cross_repo_issue_uses_inline_repo() {
        let segs = vec![Segment::CrossRepoIssue {
            owner: "user".to_string(),
            repo: "other".to_string(),
            number: 7,
            raw: "user/other#7".to_string(),
        }];
        let nodes = segments_to_hast(segs, "owner/myrepo");
        let HastNode::Element { attrs, .. } = &nodes[0] else { panic!() };
        let href = attrs.iter().find(|(k, _)| k == "href").unwrap().1.clone();
        assert_eq!(href, "https://github.com/user/other/issues/7");
    }

    #[test]
    fn sha_becomes_commit_anchor() {
        let segs = vec![Segment::CommitSha {
            sha: "abc1234".to_string(),
        }];
        let nodes = segments_to_hast(segs, "owner/myrepo");
        let HastNode::Element { attrs, .. } = &nodes[0] else { panic!() };
        let href = attrs.iter().find(|(k, _)| k == "href").unwrap().1.clone();
        assert_eq!(href, "https://github.com/owner/myrepo/commit/abc1234");
    }

    // ── rewrite_node: exclusion zones ─────────────────────────────────────

    #[test]
    fn code_element_not_rewritten() {
        let mut node = HastNode::Element {
            tag: "code".to_string(),
            attrs: vec![],
            children: vec![HastNode::Text("#123".to_string())],
            void: false,
        };
        rewrite_node(&mut node, "owner/myrepo");
        // Children must be unchanged.
        let HastNode::Element { children, .. } = &node else { panic!() };
        assert_eq!(children, &vec![HastNode::Text("#123".to_string())]);
    }

    #[test]
    fn pre_element_not_rewritten() {
        let mut node = HastNode::Element {
            tag: "pre".to_string(),
            attrs: vec![],
            children: vec![HastNode::Element {
                tag: "code".to_string(),
                attrs: vec![],
                children: vec![HastNode::Text("abc1234567def".to_string())],
                void: false,
            }],
            void: false,
        };
        rewrite_node(&mut node, "owner/myrepo");
        let HastNode::Element { children, .. } = &node else { panic!() };
        let HastNode::Element { children: code_children, .. } = &children[0] else { panic!() };
        assert_eq!(
            code_children,
            &vec![HastNode::Text("abc1234567def".to_string())]
        );
    }

    #[test]
    fn existing_anchor_not_double_linked() {
        let mut node = HastNode::Element {
            tag: "a".to_string(),
            attrs: vec![("href".to_string(), "https://example.com".to_string())],
            children: vec![HastNode::Text("#123".to_string())],
            void: false,
        };
        rewrite_node(&mut node, "owner/myrepo");
        let HastNode::Element { children, .. } = &node else { panic!() };
        // Must remain a plain text child — not wrapped in another <a>.
        assert_eq!(children, &vec![HastNode::Text("#123".to_string())]);
    }

    // ── GithubAutolinksPlugin visitor ──────────────────────────────────────

    #[test]
    fn visitor_rewrites_text_in_paragraph() {
        let mut root = HastNode::Root {
            children: vec![HastNode::Element {
                tag: "p".to_string(),
                attrs: vec![],
                children: vec![HastNode::Text("see #42".to_string())],
                void: false,
            }],
        };
        GithubAutolinksPlugin::new("owner/repo").visit(&mut root);
        let HastNode::Root { children } = &root else { panic!() };
        let HastNode::Element { children: p_children, .. } = &children[0] else { panic!() };
        // Should contain plain "see " + anchor for #42.
        assert_eq!(p_children.len(), 2);
        assert!(matches!(&p_children[0], HastNode::Text(t) if t == "see "));
        assert!(matches!(&p_children[1], HastNode::Element { tag, .. } if tag == "a"));
    }
}
