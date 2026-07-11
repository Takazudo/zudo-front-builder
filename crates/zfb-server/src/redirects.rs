//! `_redirects` engine — the Cloudflare Workers Static Assets subset
//! (issue #1543 / epic #1541 Preview Parity).
//!
//! `zfb` follows the documented `_redirects` file format so a project's
//! redirect/rewrite rules behave the same in `zfb dev`, `zfb preview`,
//! and the deployed Cloudflare Worker. Spec reference:
//! <https://developers.cloudflare.com/workers/static-assets/redirects/>.
//!
//! ## Grammar (one rule per non-comment, non-blank line)
//!
//! ```text
//! /source /target [status]
//! ```
//!
//! - Lines starting with `#` and blank lines are skipped.
//! - `source` may contain a single splat (`*`) segment, which greedily
//!   captures the matched path portion (including any `/`) for use in
//!   `target` as `:splat`. Only one splat per rule is allowed.
//! - `source` may contain `:name` placeholder segments
//!   (`:[A-Za-z]\w*`), each capturing exactly one path segment for use
//!   in `target` as `:name`.
//! - `status` is optional and defaults to **302**. Allowed values are
//!   301/302/303/307/308 (redirect) and 200 (rewrite/proxy — serve
//!   `target` without changing the client-visible URL).
//! - A malformed line (wrong token count, unparsable/disallowed status,
//!   a source not starting with `/`, more than one splat, an invalid
//!   placeholder name, or a `200` rule whose target is an external
//!   URL — proxying only supports relative targets) is logged via
//!   `tracing::warn!` and skipped. [`Redirects::parse`] never fails —
//!   a broken `_redirects` file must never take the server down.
//!
//! ## Matching contract
//!
//! - **First-match-wins**: rules are tried in file order; the first
//!   rule whose `source` matches the request path wins.
//! - **Single rule application per request — no chaining.** A `200`
//!   rewrite resolves its target once; [`Redirects::match_request`]
//!   does not recursively re-match the rewritten path against the rule
//!   set, and callers must not do so either (this mirrors Cloudflare's
//!   documented behaviour: "Only the first redirect in your file will
//!   apply").
//! - **Method gating**: only `GET`/`HEAD` requests are evaluated.
//!   zfb-specific decision (issue #1543) — a deployed Worker with
//!   static assets only ever probes the asset layer (and therefore the
//!   `_redirects` rules) for GET/HEAD, so other methods bypass
//!   evaluation entirely rather than matching an asset-shaped rule by
//!   accident.
//! - **Trailing slash is significant.** Matching is exact per path
//!   segment (including the empty trailing segment a trailing `/`
//!   produces), so `/old` and `/old/` are distinct sources unless the
//!   rule uses a splat.
//! - **Query string** on the incoming request is preserved on
//!   redirects (appended to `target` after substitution); rewrites
//!   ignore it since the client-visible URL never changes.

use std::sync::Arc;

/// One parsed `_redirects` rule.
#[derive(Debug, Clone)]
struct Rule {
    source: Vec<SourceSegment>,
    /// Raw target string with `:name` / `:splat` tokens, substituted at
    /// match time (a rule with no matching request never needs this
    /// work done).
    target: String,
    status: u16,
    is_rewrite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceSegment {
    Literal(String),
    Placeholder(String),
    Splat,
}

/// Outcome of a successful [`Redirects::match_request`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectOutcome {
    /// A 301/302/303/307/308 redirect. `location` is the fully
    /// substituted target with the request's query string appended
    /// (Cloudflare spec: query string is preserved on redirects).
    Redirect { status: u16, location: String },
    /// A `200` rewrite: serve the asset at `target` instead of the
    /// requested path, without changing the client-visible URL or
    /// status. Resolved once — see the module docs' "no chaining"
    /// contract.
    Rewrite { target: String },
}

/// A parsed, ready-to-match `_redirects` file.
///
/// Cheap to clone (the rule list is behind an [`Arc`]) so later
/// dev/preview integration can hand a shared handle to request
/// handlers without re-parsing per request.
#[derive(Debug, Clone, Default)]
pub struct Redirects {
    rules: Arc<Vec<Rule>>,
}

impl Redirects {
    /// Parse `_redirects` file contents. Never fails — malformed lines
    /// are logged and skipped; see the module docs for exactly what
    /// counts as malformed.
    pub fn parse(input: &str) -> Self {
        let mut rules = Vec::new();
        for (idx, raw_line) in input.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match parse_rule(line) {
                Some(rule) => rules.push(rule),
                None => {
                    tracing::warn!(
                        line = idx + 1,
                        content = raw_line,
                        "_redirects: skipping malformed line"
                    );
                }
            }
        }
        Self {
            rules: Arc::new(rules),
        }
    }

    /// `true` when no rules were parsed (missing file, empty file, or
    /// every line was malformed/comment/blank).
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Number of successfully parsed rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Match `path` (leading `/`, no query string) plus an optional raw
    /// `query` (no leading `?`) against the rule set for an incoming
    /// `method`. Returns `None` when no rule matches, or immediately
    /// when `method` is not `GET`/`HEAD` (see the module docs' method
    /// gating rule).
    pub fn match_request(
        &self,
        path: &str,
        query: Option<&str>,
        method: &str,
    ) -> Option<RedirectOutcome> {
        if !method.eq_ignore_ascii_case("GET") && !method.eq_ignore_ascii_case("HEAD") {
            return None;
        }
        let url_segments = split_path(path);
        self.rules
            .iter()
            .find_map(|rule| rule.try_match(&url_segments, query))
    }
}

/// Captures produced by a successful [`match_segments`] call: named
/// placeholder values plus an optional splat capture (`None` when the
/// rule's source has no splat segment).
struct Captures {
    placeholders: Vec<(String, String)>,
    splat: Option<String>,
}

impl Rule {
    /// Attempt to match `url_segments` against this rule's source.
    /// Returns the finished [`RedirectOutcome`] (with substitution and
    /// query-string handling already applied) on a hit.
    fn try_match(&self, url_segments: &[&str], query: Option<&str>) -> Option<RedirectOutcome> {
        let captures = match_segments(&self.source, url_segments)?;
        let substituted = substitute_target(
            &self.target,
            &captures.placeholders,
            captures.splat.as_deref(),
        );
        Some(if self.is_rewrite {
            RedirectOutcome::Rewrite {
                target: substituted,
            }
        } else {
            let location = match query.filter(|q| !q.is_empty()) {
                Some(q) => append_query(&substituted, q),
                None => substituted,
            };
            RedirectOutcome::Redirect {
                status: self.status,
                location,
            }
        })
    }
}

/// Split a request path into segments the same way [`parse_source`]
/// splits a rule's source, so a trailing slash produces a trailing
/// empty segment on both sides (this is what makes `/old` vs `/old/`
/// distinct — see the module docs).
fn split_path(path: &str) -> Vec<&str> {
    path.strip_prefix('/').unwrap_or(path).split('/').collect()
}

/// Match `source` segments against `url_segments`. `source` contains at
/// most one [`SourceSegment::Splat`] (enforced at parse time); when
/// present it greedily captures every URL segment between the fixed
/// prefix and suffix (so a splat may appear anywhere, not only at the
/// end, even though the common case is trailing). Returns the captured
/// placeholders and the splat capture (`None` when the rule has no
/// splat) on a match.
fn match_segments(source: &[SourceSegment], url_segments: &[&str]) -> Option<Captures> {
    let mut placeholders = Vec::new();
    if let Some(splat_idx) = source.iter().position(|s| *s == SourceSegment::Splat) {
        let prefix = &source[..splat_idx];
        let suffix = &source[splat_idx + 1..];
        // `+ 1`: the splat itself must consume at least one segment
        // (possibly empty), because the pattern's own `/` on either
        // side of `*` has to line up with a real `/` in the request.
        // Without this, a bare `/blog` (no trailing slash at all)
        // would wrongly match `/blog/*` via a zero-segment capture.
        if url_segments.len() < prefix.len() + suffix.len() + 1 {
            return None;
        }
        let suffix_start = url_segments.len() - suffix.len();
        match_fixed_segments(prefix, &url_segments[..prefix.len()], &mut placeholders)?;
        match_fixed_segments(suffix, &url_segments[suffix_start..], &mut placeholders)?;
        let splat = url_segments[prefix.len()..suffix_start].join("/");
        Some(Captures {
            placeholders,
            splat: Some(splat),
        })
    } else {
        if source.len() != url_segments.len() {
            return None;
        }
        match_fixed_segments(source, url_segments, &mut placeholders)?;
        Some(Captures {
            placeholders,
            splat: None,
        })
    }
}

/// Match a splat-free run of source segments 1:1 against the same
/// number of URL segments, appending any placeholder captures to
/// `out`. Caller guarantees equal lengths.
fn match_fixed_segments(
    source: &[SourceSegment],
    url_segments: &[&str],
    out: &mut Vec<(String, String)>,
) -> Option<()> {
    for (seg, url_seg) in source.iter().zip(url_segments.iter()) {
        match seg {
            SourceSegment::Literal(lit) => {
                if lit != url_seg {
                    return None;
                }
            }
            SourceSegment::Placeholder(name) => out.push((name.clone(), (*url_seg).to_string())),
            SourceSegment::Splat => unreachable!("splat handled by the caller"),
        }
    }
    Some(())
}

/// Replace `:name` tokens in `target` with their captured values.
/// `:splat` is only substituted when the rule actually had a splat
/// segment; an unknown or unmatched token is left as literal text
/// (e.g. a target that legitimately contains a bare `:` with no
/// corresponding capture).
fn substitute_target(
    target: &str,
    placeholders: &[(String, String)],
    splat: Option<&str>,
) -> String {
    let chars: Vec<char> = target.chars().collect();
    let mut out = String::with_capacity(target.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ':' {
            let start = i + 1;
            let mut j = start;
            while j < chars.len() && (chars[j] == '_' || chars[j].is_ascii_alphanumeric()) {
                j += 1;
            }
            if j > start {
                let name: String = chars[start..j].iter().collect();
                let replacement = if name == "splat" {
                    splat
                } else {
                    placeholders
                        .iter()
                        .find(|(n, _)| *n == name)
                        .map(|(_, v)| v.as_str())
                };
                match replacement {
                    Some(v) => out.push_str(v),
                    None => {
                        out.push(':');
                        out.push_str(&name);
                    }
                }
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Append the request's (already-substituted) query string to a
/// redirect target, inserting before any fragment. Merges with `&`
/// when the target already carries its own query string.
fn append_query(location: &str, query: &str) -> String {
    let (before_fragment, fragment) = match location.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (location, None),
    };
    let sep = if before_fragment.contains('?') {
        '&'
    } else {
        '?'
    };
    let mut out = format!("{before_fragment}{sep}{query}");
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(f);
    }
    out
}

/// Parse one non-blank, non-comment `_redirects` line into a [`Rule`].
/// `None` means the line is malformed and must be skipped (with a
/// warning logged by the caller).
fn parse_rule(line: &str) -> Option<Rule> {
    let mut tokens = line.split_whitespace();
    let source_str = tokens.next()?;
    let target_str = tokens.next()?;
    let status_tok = tokens.next();
    if tokens.next().is_some() {
        // Cloudflare spec: "Only one redirect can be defined per line
        // and must follow this format, otherwise it will be ignored."
        return None;
    }
    let status: u16 = match status_tok {
        Some(tok) => {
            let parsed: u16 = tok.parse().ok()?;
            if !matches!(parsed, 200 | 301 | 302 | 303 | 307 | 308) {
                return None;
            }
            parsed
        }
        None => 302,
    };
    if !source_str.starts_with('/') {
        return None;
    }
    let is_rewrite = status == 200;
    if is_rewrite && is_external_target(target_str) {
        // Cloudflare spec: "Proxying will only support relative URLs
        // on your site. You cannot proxy external domains."
        return None;
    }
    let source = parse_source(source_str)?;
    Some(Rule {
        source,
        target: target_str.to_string(),
        status,
        is_rewrite,
    })
}

/// Split a rule's `source` into matchable segments. `None` when the
/// source contains more than one splat or an invalid placeholder name.
fn parse_source(source: &str) -> Option<Vec<SourceSegment>> {
    let trimmed = source.strip_prefix('/').unwrap_or(source);
    let mut segments = Vec::new();
    let mut splat_seen = false;
    for seg in trimmed.split('/') {
        if seg == "*" {
            if splat_seen {
                // Cloudflare spec: "You may only include a single
                // splat in the URL."
                return None;
            }
            splat_seen = true;
            segments.push(SourceSegment::Splat);
        } else if let Some(name) = seg.strip_prefix(':') {
            if !is_valid_placeholder_name(name) {
                return None;
            }
            segments.push(SourceSegment::Placeholder(name.to_string()));
        } else {
            segments.push(SourceSegment::Literal(seg.to_string()));
        }
    }
    Some(segments)
}

/// Cloudflare spec: placeholder names must match `:[A-Za-z]\w*`.
fn is_valid_placeholder_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `true` when `target` starts with a URL scheme (`scheme:`), i.e. is
/// an absolute/external URL rather than a same-site relative path.
/// A relative target always starts with `/`, which never matches a
/// scheme (schemes must start with an ASCII letter), so this cannot
/// misclassify placeholder-bearing relative targets like `/:lang/about`.
fn is_external_target(target: &str) -> bool {
    let mut chars = target.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    for c in chars {
        if c == ':' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> (&str, Option<&str>, &str) {
        (path, None, "GET")
    }

    // --- exact-source match --------------------------------------------

    #[test]
    fn exact_source_matches_only_that_path() {
        let r = Redirects::parse("/about /about-us 301\n");
        assert_eq!(
            r.match_request("/about", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/about-us".to_string(),
            })
        );
        assert_eq!(r.match_request("/about-us", None, "GET"), None);
        assert_eq!(r.match_request("/about/extra", None, "GET"), None);
    }

    // --- trailing-slash handling ----------------------------------------

    #[test]
    fn trailing_slash_is_a_distinct_source() {
        let r = Redirects::parse("/old /new 301\n/old/ /new-slash 301\n");
        let (p, q, m) = get("/old");
        assert_eq!(
            r.match_request(p, q, m),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/new".to_string(),
            })
        );
        assert_eq!(
            r.match_request("/old/", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/new-slash".to_string(),
            })
        );
    }

    // --- splat capture / substitution ------------------------------------

    #[test]
    fn splat_captures_full_remaining_path_and_substitutes() {
        let r = Redirects::parse("/blog/* /news/:splat 301\n");
        assert_eq!(
            r.match_request("/blog/2024/07/hello", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/news/2024/07/hello".to_string(),
            })
        );
        // The `/blog` prefix itself, with nothing after it, still needs
        // a (possibly empty) segment to satisfy the splat.
        assert_eq!(
            r.match_request("/blog/", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/news/".to_string(),
            })
        );
        // A bare `/blog` with no trailing slash at all has no `/` for
        // the splat to line up with — must NOT match.
        assert_eq!(r.match_request("/blog", None, "GET"), None);
        assert_eq!(r.match_request("/other/path", None, "GET"), None);
    }

    #[test]
    fn bare_splat_is_a_catch_all_including_root() {
        // Common SPA-fallback shape: `/* /index.html 200`.
        let r = Redirects::parse("/* /index.html 200\n");
        for path in ["/", "/foo", "/foo/bar/baz"] {
            assert_eq!(
                r.match_request(path, None, "GET"),
                Some(RedirectOutcome::Rewrite {
                    target: "/index.html".to_string(),
                }),
                "path {path} must be caught by the bare splat",
            );
        }
    }

    // --- placeholder capture / substitution -------------------------------

    #[test]
    fn placeholder_captures_one_segment_and_substitutes() {
        let r = Redirects::parse("/blog/:slug /articles/:slug 301\n");
        assert_eq!(
            r.match_request("/blog/hello-world", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/articles/hello-world".to_string(),
            })
        );
        // A placeholder matches exactly one segment — extra segments
        // (or missing ones) don't match.
        assert_eq!(r.match_request("/blog/hello/world", None, "GET"), None);
        assert_eq!(r.match_request("/blog", None, "GET"), None);
    }

    #[test]
    fn multiple_placeholders_all_substitute() {
        let r = Redirects::parse("/:lang/about /about?lang=:lang 302\n");
        assert_eq!(
            r.match_request("/fr/about", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 302,
                location: "/about?lang=fr".to_string(),
            })
        );
    }

    // --- status parsing incl. default 302 ---------------------------------

    #[test]
    fn status_defaults_to_302_when_omitted() {
        let r = Redirects::parse("/a /b\n");
        assert_eq!(
            r.match_request("/a", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 302,
                location: "/b".to_string(),
            })
        );
    }

    #[test]
    fn all_documented_statuses_parse() {
        for status in [301u16, 302, 303, 307, 308] {
            let r = Redirects::parse(&format!("/a /b {status}\n"));
            assert_eq!(
                r.match_request("/a", None, "GET"),
                Some(RedirectOutcome::Redirect {
                    status,
                    location: "/b".to_string(),
                }),
                "status {status} must parse and round-trip",
            );
        }
    }

    // --- 200-rewrite marker -------------------------------------------------

    #[test]
    fn status_200_produces_a_rewrite_outcome() {
        let r = Redirects::parse("/api/* /api-handler 200\n");
        assert_eq!(
            r.match_request("/api/anything", None, "GET"),
            Some(RedirectOutcome::Rewrite {
                target: "/api-handler".to_string(),
            })
        );
    }

    // --- external targets ---------------------------------------------------

    #[test]
    fn external_target_is_allowed_for_redirects() {
        let r = Redirects::parse("/old https://example.com/new 301\n");
        assert_eq!(
            r.match_request("/old", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "https://example.com/new".to_string(),
            })
        );
    }

    #[test]
    fn external_target_is_rejected_for_200_rewrites() {
        let r = Redirects::parse("/old https://example.com/new 200\n");
        assert!(r.is_empty(), "external 200-rewrite rule must be skipped");
        assert_eq!(r.match_request("/old", None, "GET"), None);
    }

    // --- query-string preservation on redirects ----------------------------

    #[test]
    fn query_string_is_preserved_on_redirect() {
        let r = Redirects::parse("/old /new 301\n");
        assert_eq!(
            r.match_request("/old", Some("a=1&b=2"), "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/new?a=1&b=2".to_string(),
            })
        );
    }

    #[test]
    fn query_string_merges_with_targets_own_query() {
        let r = Redirects::parse("/old /new?x=1 301\n");
        assert_eq!(
            r.match_request("/old", Some("a=1"), "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/new?x=1&a=1".to_string(),
            })
        );
    }

    #[test]
    fn query_string_is_not_appended_for_rewrites() {
        let r = Redirects::parse("/old /new 200\n");
        assert_eq!(
            r.match_request("/old", Some("a=1"), "GET"),
            Some(RedirectOutcome::Rewrite {
                target: "/new".to_string(),
            })
        );
    }

    // --- malformed-line tolerance --------------------------------------------

    #[test]
    fn malformed_lines_are_skipped_not_fatal() {
        let input = "\
# a comment

/only-one-token
/bad-status /target 999
no-leading-slash /target 301
/double-splat/*/* /target 301
/bad-placeholder/:1abc /target 301
/proxy-external https://example.com 200
/good /target 301
";
        let r = Redirects::parse(input);
        assert_eq!(r.len(), 1, "only the single well-formed rule survives");
        assert_eq!(
            r.match_request("/good", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/target".to_string(),
            })
        );
    }

    #[test]
    fn fully_garbage_input_never_panics_and_yields_empty() {
        let r = Redirects::parse("not a redirects file\nat all\n\n\n# just comments\n");
        assert!(r.is_empty());
        assert_eq!(r.match_request("/anything", None, "GET"), None);
    }

    // --- first-match-wins -----------------------------------------------------

    #[test]
    fn first_matching_rule_wins() {
        let r = Redirects::parse("/a /first 301\n/a /second 302\n");
        assert_eq!(
            r.match_request("/a", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/first".to_string(),
            })
        );
    }

    #[test]
    fn rewrite_does_not_chain_into_a_later_matching_rule() {
        // Rule A rewrites /a -> /b; rule B would redirect /b if it were
        // ever looked up again. match_request must resolve /a via rule
        // A only, in one pass — never auto-chaining into rule B.
        let r = Redirects::parse("/a /b 200\n/b /c 301\n");
        assert_eq!(
            r.match_request("/a", None, "GET"),
            Some(RedirectOutcome::Rewrite {
                target: "/b".to_string(),
            })
        );
    }

    // --- method gating ----------------------------------------------------

    #[test]
    fn only_get_and_head_are_evaluated() {
        let r = Redirects::parse("/a /b 301\n");
        assert_eq!(
            r.match_request("/a", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/b".to_string(),
            })
        );
        assert_eq!(
            r.match_request("/a", None, "HEAD"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/b".to_string(),
            })
        );
        assert_eq!(r.match_request("/a", None, "POST"), None);
        assert_eq!(r.match_request("/a", None, "PUT"), None);
        assert_eq!(r.match_request("/a", None, "DELETE"), None);
    }

    // --- misc ---------------------------------------------------------------

    #[test]
    fn empty_input_yields_empty_rule_set() {
        let r = Redirects::parse("");
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn root_source_matches_root_path_only() {
        let r = Redirects::parse("/ /home 301\n");
        assert_eq!(
            r.match_request("/", None, "GET"),
            Some(RedirectOutcome::Redirect {
                status: 301,
                location: "/home".to_string(),
            })
        );
        assert_eq!(r.match_request("/foo", None, "GET"), None);
    }
}
