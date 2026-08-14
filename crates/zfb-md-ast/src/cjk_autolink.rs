//! GFM autolink-literal CJK boundary fixup (zfb#1105).
//!
//! GFM's extended-URL autolink **path** grammar terminates only on ASCII
//! whitespace / EOL (`path ::= 1*( … | byte - eof - eol - space_or_tab )`),
//! so a bare URL written flush against Japanese/Chinese/Korean text —
//! `詳細はhttps://example.com参照してください。` — has its trailing CJK run
//! swallowed into both the `href` and the visible link text. The domain
//! rule already terminates on Unicode whitespace; only the path rule is
//! affected (cmark-gfm spec issue #650).
//!
//! `markdown-rs` (the `markdown` crate) is a plain crates.io dependency —
//! its tokeniser is not configurable from outside — so, exactly like
//! `CjkFriendlyPlugin` does for
//! emphasis flanking, the cleanest fix is a post-parse mdast pass: find the
//! `Link` node markdown-rs already produced for an over-consuming autolink
//! literal and split it at the first CJK character, moving the CJK run out
//! into a following `Text` sibling.
//!
//! A GFM autolink literal materialises as
//! `Link { title: None, children: [Text(visible)], url }` where `url`
//! either equals `visible` (the `http://` / `https://` / `ftp://` forms,
//! whose scheme is part of the visible text) or is `visible` with `http://`
//! prepended (the `www.` form). Email autolinks (`mailto:` URLs) already
//! terminate before CJK in markdown-rs, so they never over-consume and are
//! naturally skipped here.
//!
//! We only ever touch a `Link` whose truncated `url` reconstructs to the
//! exact autolink markdown-rs would have built (Guard A — the kept URL must
//! carry a real `http(s)://`/`ftp://` scheme, or be a `www.` host with
//! `http://` prepended) and still has a real host (Guard B — ≥2 dot-separated
//! labels, ignoring an optional `:port`; rejects degenerate IDN cuts like
//! `https://www.日本語.com参照` that would otherwise yield a hostless/partial
//! `https://www`). Ordinary `[label](dest)` links — including schemeless ones
//! like `[example.com参照](example.com参照)` — fail these guards. The truncated
//! URL is re-trimmed with GFM's own autolink end rules (trailing `?!.,:*_~`
//! punctuation plus the unmatched-`)` paren-balance rule), so a cut that
//! exposes a stray `)` (`(https://example.com)参照`) drops it from the href.
//!
//! This pass runs in the mdast phase **before** [`CjkFriendlyPlugin`]: that
//! plugin re-tokenises emphasis markers inside `Text` nodes (including a
//! `Link`'s text child), which would split an over-consumed autolink's single
//! `Text` child apart and hide it from the boundary check. It is only
//! registered when `gfm.autolinkLiteral` is on (and `markdown.cjkFriendly` is
//! on), so a pipeline that never produces autolink literals never runs it.
//! The one ambiguity that cannot be resolved in mdast — an author
//! hand-writing `[https://x.com参照](https://x.com参照)`, byte-identical to an
//! autolink — is only reachable once bare-URL autolinking is enabled, the
//! same surface that produces the bug.
//!

use markdown::mdast::{Link, Node as MdastNode, Text};

use crate::cjk::is_cjk;
use crate::MdastVisitor;

/// Horizontal ellipsis. Not a CJK codepoint (so it stays out of the global
/// [`is_cjk`] table, which also governs emphasis flanking), but in CJK prose
/// it is a sentence-internal boundary — `…ADDAC127` in the real-world repro.
/// Treated as an autolink terminator only within this plugin.
const HORIZONTAL_ELLIPSIS: char = '\u{2026}';

/// True for a character that should terminate a GFM autolink path in CJK
/// text: any CJK character (incl. full-width punctuation like `）` U+FF09 and
/// `。` U+3002) plus the horizontal ellipsis.
fn is_autolink_boundary(c: char) -> bool {
    is_cjk(c) || c == HORIZONTAL_ELLIPSIS
}

/// GFM trailing-punctuation set: `?!.,:*_~` are never part of an autolink's
/// tail (though they may appear in its interior).
fn is_gfm_trailing_punct(c: char) -> bool {
    matches!(c, '?' | '!' | '.' | ',' | ':' | '*' | '_' | '~')
}

/// Re-apply GFM's autolink end-trimming to `url` after the CJK cut and return
/// the kept prefix. The cut can expose tail characters that GFM would not
/// have treated as part of the link: trailing `?!.,:*_~` punctuation, and a
/// trailing `)` that has no matching `(` inside the link (the paren-balance
/// rule that lets a bare URL sit inside parentheses — `(https://example.com)`).
/// markdown-rs applied this to the *whole* over-consumed run, which ended in
/// CJK; once we cut before the CJK, the rule must run again on the new tail.
/// Iterates because trimming a `)` can expose more punctuation and vice versa,
/// mirroring cmark-gfm's backward scan.
fn trim_gfm_url_end(url: &str) -> &str {
    let mut end = url.len();
    loop {
        let s = &url[..end];
        let after_punct = s.trim_end_matches(is_gfm_trailing_punct);
        if after_punct.len() != end {
            end = after_punct.len();
            continue;
        }
        // Trailing `)` is dropped only when the link holds more `)` than `(`.
        if s.ends_with(')') && s.matches(')').count() > s.matches('(').count() {
            end -= 1; // ')' is one byte
            continue;
        }
        break;
    }
    &url[..end]
}

/// URL schemes whose host follows a `scheme://host` shape. Used by
/// [`has_host`] to peel the scheme before validating the host. `mailto:` is
/// intentionally absent — email autolinks never reach the split.
const HOST_SCHEMES: [&str; 3] = ["https://", "http://", "ftp://"];

/// Visitor that splits over-consuming GFM autolink-literal `Link` nodes at
/// the first CJK character. Stateless.
#[derive(Debug, Default, Clone, Copy)]
pub struct CjkAutolinkBoundaryPlugin;

impl CjkAutolinkBoundaryPlugin {
    /// New visitor; stateless.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl MdastVisitor for CjkAutolinkBoundaryPlugin {
    fn visit(&mut self, node: &mut MdastNode) {
        rewrite(node);
    }
}

/// Walk `node`: split over-consuming autolink `Link`s in its children list,
/// then recurse. Stops at no-recurse boundaries (verbatim / author-owned
/// content) — mirrors `CjkFriendlyPlugin`.
fn rewrite(node: &mut MdastNode) {
    if is_no_recurse(node) {
        return;
    }
    if let Some(children) = node.children_mut() {
        let split = split_autolinks(std::mem::take(children));
        *children = split;
    }
    if let Some(children) = node.children_mut() {
        for child in children {
            rewrite(child);
        }
    }
}

/// Nodes whose subtree must NOT be touched: verbatim code, raw HTML, and
/// MDX JSX / expression bodies (author-controlled). Same set as
/// `CjkFriendlyPlugin`.
fn is_no_recurse(node: &MdastNode) -> bool {
    matches!(
        node,
        MdastNode::Code(_)
            | MdastNode::InlineCode(_)
            | MdastNode::Html(_)
            | MdastNode::MdxJsxFlowElement(_)
            | MdastNode::MdxJsxTextElement(_)
            | MdastNode::MdxFlowExpression(_)
            | MdastNode::MdxTextExpression(_)
    )
}

/// Replace each over-consuming autolink `Link` in `children` with a
/// truncated `Link` followed by a `Text` carrying the CJK remainder. Every
/// other child passes through untouched.
fn split_autolinks(children: Vec<MdastNode>) -> Vec<MdastNode> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        match child {
            MdastNode::Link(link) => match analyze(&link) {
                Some(plan) => {
                    out.push(MdastNode::Link(Link {
                        url: plan.new_url,
                        title: None,
                        children: vec![text_node(plan.before)],
                        position: None,
                    }));
                    out.push(text_node(plan.remainder));
                }
                None => out.push(MdastNode::Link(link)),
            },
            other => out.push(other),
        }
    }
    out
}

/// The result of deciding to split a `Link`.
struct SplitPlan {
    /// Truncated `href` (and visible text) of the kept autolink.
    new_url: String,
    /// Kept visible text — equals `new_url` minus any scheme prefix.
    before: String,
    /// CJK run (plus any re-trimmed trailing punctuation) moved out into a
    /// following `Text` sibling.
    remainder: String,
}

/// Decide whether `link` is an over-consuming GFM autolink literal and, if
/// so, where to cut it. Returns `None` to leave the `Link` untouched.
fn analyze(link: &Link) -> Option<SplitPlan> {
    // Autolink literals never carry a title and always have a single Text
    // child. An explicit `[label](dest "title")` or a label with inline
    // markup fails here.
    if link.title.is_some() {
        return None;
    }
    let visible = match link.children.as_slice() {
        [MdastNode::Text(t)] => t.value.as_str(),
        _ => return None,
    };

    // First CJK/ellipsis boundary. `k == 0` means the visible text *starts*
    // with CJK — that is an explicit link label, never a bare URL.
    let k = visible
        .char_indices()
        .find(|&(_, c)| is_autolink_boundary(c))
        .map(|(i, _)| i)?;
    if k == 0 {
        return None;
    }
    let after0 = &visible[k..];

    let before0 = &visible[..k];
    // Re-apply GFM autolink end-trimming (trailing punctuation + unmatched
    // `)`): the cut can expose tail characters GFM would not treat as part of
    // the link.
    let before = trim_gfm_url_end(before0);
    if before.is_empty() {
        return None;
    }
    let trimmed_tail = &before0[before.len()..];

    // Peel `(CJK run) + (moved tail)` off the URL's end with strip_suffix (no
    // length arithmetic). A genuine autolink's `url` is `scheme_prefix +
    // visible == scheme_prefix + before + trimmed_tail + after0`, so both
    // suffixes peel cleanly; an explicit `[これは詳細](url)` whose `url` does
    // not end with the label's CJK run fails the first strip and returns
    // `None`.
    let new_url = link
        .url
        .strip_suffix(after0)
        .and_then(|u| u.strip_suffix(trimmed_tail))?;

    // Guard A — reconstruction: the truncated URL must be exactly the autolink
    // markdown-rs would have built for `before`. markdown-rs only emits an
    // autolink-literal `Link` whose visible text either carries its own scheme
    // (`http://`, `https://`, `ftp://` — then `url == visible`) or is a `www.`
    // host with `http://` prepended. Requiring the scheme / `www.` prefix here
    // rejects ordinary explicit links such as `[example.com参照](example.com参照)`
    // or `[ex.com参照](http://ex.com参照)`, which are reachable because this
    // visitor also runs over explicit `Link` nodes.
    let is_reconstructable = (new_url == before && starts_with_url_scheme(before))
        || new_url
            .strip_prefix("http://")
            .is_some_and(|h| h == before && before.starts_with("www."));
    if !is_reconstructable {
        return None;
    }
    // Guard B — real host: reject cuts that leave a scheme without a proper
    // host (`https://日本語.com参照` → `https://`; `https://www.日本語.com参照`
    // → `https://www`). Such inputs stay as-is rather than autolinking a
    // partial host.
    if !has_host(new_url) {
        return None;
    }

    let remainder = &visible[before.len()..];
    Some(SplitPlan {
        new_url: new_url.to_string(),
        before: before.to_string(),
        remainder: remainder.to_string(),
    })
}

/// True when `url` begins with a literal autolink scheme (`http://`,
/// `https://`, `ftp://`) — i.e. the scheme is part of the visible text.
fn starts_with_url_scheme(url: &str) -> bool {
    HOST_SCHEMES.iter().any(|scheme| url.starts_with(scheme))
}

/// True when `url`'s host is a real domain: at least two non-empty
/// dot-separated labels (`example.com`, `www.example.com`) made of ASCII
/// host characters. The host is the run after any recognised scheme up to
/// the first `/`, `?`, or `#`, with an optional `:port` stripped (so
/// `https://example.com:8080/参照` validates on `example.com`). Rejects
/// scheme-only (`https://`) and single-label (`https://www`) truncations.
fn has_host(url: &str) -> bool {
    let rest = HOST_SCHEMES
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme))
        .unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // Strip a trailing `:port` (digits only) so the port colon doesn't break
    // label validation.
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    };
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Build a position-less `Text` node (matching `CjkFriendlyPlugin`, which
/// also drops positions on synthesised nodes).
fn text_node(value: String) -> MdastNode {
    MdastNode::Text(Text {
        value,
        position: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse with GFM autolink-literal ON, run the plugin, and render the
    /// first paragraph's children to a compact, assertable string:
    /// `Text` → its value verbatim; an autolink `Link` →
    /// `[A href=<url> text=<visible>]`.
    fn dump(input: &str) -> String {
        let opts = markdown::ParseOptions {
            constructs: markdown::Constructs {
                gfm_autolink_literal: true,
                ..markdown::Constructs::mdx()
            },
            ..markdown::ParseOptions::mdx()
        };
        let mut mdast = markdown::to_mdast(input, &opts).expect("markdown-rs parses fixture");
        CjkAutolinkBoundaryPlugin::new().visit(&mut mdast);

        let MdastNode::Root(root) = &mdast else {
            unreachable!("expected Root, got {mdast:?}")
        };
        let MdastNode::Paragraph(p) = &root.children[0] else {
            unreachable!("expected Paragraph, got {:?}", root.children[0])
        };
        let mut out = String::new();
        for child in &p.children {
            match child {
                MdastNode::Text(t) => out.push_str(&t.value),
                MdastNode::Link(l) => {
                    let text = match l.children.as_slice() {
                        [MdastNode::Text(t)] => t.value.as_str(),
                        _ => "<non-text>",
                    };
                    out.push_str(&format!("[A href={} text={}]", l.url, text));
                }
                other => out.push_str(&format!("{other:?}")),
            }
        }
        out
    }

    // --- The two issue #1105 repro cases. ---

    #[test]
    fn https_url_before_cjk_run_is_split() {
        assert_eq!(
            dump("詳細はhttps://example.com参照してください。"),
            "詳細は[A href=https://example.com text=https://example.com]参照してください。"
        );
    }

    #[test]
    fn fullwidth_paren_terminates_the_path() {
        // The real-world hit: `）` (U+FF09) is the first boundary, so the
        // whole `）が施されており…ADDAC127` run drops out of the link.
        assert_eq!(
            dump("（https://ebow.com/diy-mods）が施されており…ADDAC127"),
            "（[A href=https://ebow.com/diy-mods text=https://ebow.com/diy-mods]）が施されており…ADDAC127"
        );
    }

    // --- Scheme variants. ---

    #[test]
    fn www_form_keeps_prepended_http_scheme() {
        assert_eq!(
            dump("www.example.com参照"),
            "[A href=http://www.example.com text=www.example.com]参照"
        );
    }

    // --- Boundary / trimming rules. ---

    #[test]
    fn cjk_full_stop_terminates() {
        assert_eq!(
            dump("https://example.com。"),
            "[A href=https://example.com text=https://example.com]。"
        );
    }

    #[test]
    fn trailing_ascii_punct_is_re_trimmed_into_remainder() {
        // The interior `,` becomes trailing once the path is cut at `参`,
        // so GFM trimming moves it out of the href.
        assert_eq!(
            dump("https://example.com/path,参照"),
            "[A href=https://example.com/path text=https://example.com/path],参照"
        );
    }

    #[test]
    fn ellipsis_is_a_boundary() {
        assert_eq!(
            dump("https://example.com…参照"),
            "[A href=https://example.com text=https://example.com]…参照"
        );
    }

    #[test]
    fn multiple_autolinks_each_split() {
        assert_eq!(
            dump("https://a.com参照 https://b.com確認"),
            "[A href=https://a.com text=https://a.com]参照 [A href=https://b.com text=https://b.com]確認"
        );
    }

    // --- Cases that must be left UNCHANGED. ---

    #[test]
    fn ascii_terminated_autolink_unchanged() {
        assert_eq!(
            dump("see https://example.com for details"),
            "see [A href=https://example.com text=https://example.com] for details"
        );
    }

    #[test]
    fn no_cjk_anywhere_unchanged() {
        assert_eq!(
            dump("https://example.com/a/b/c"),
            "[A href=https://example.com/a/b/c text=https://example.com/a/b/c]"
        );
    }

    #[test]
    fn idn_cjk_in_host_left_unchanged() {
        // First CJK is inside the host (right after the scheme) — there is no
        // clean ASCII host to truncate to, so we leave it as markdown-rs
        // produced it rather than emit a hostless `https://`.
        assert_eq!(
            dump("https://日本語.com参照"),
            "[A href=https://日本語.com参照 text=https://日本語.com参照]"
        );
    }

    #[test]
    fn www_with_cjk_in_host_left_unchanged() {
        // Guard B: truncating before `日` would leave `https://www` (a single
        // label, no TLD) — rejected, link untouched.
        assert_eq!(
            dump("https://www.日本語.com参照"),
            "[A href=https://www.日本語.com参照 text=https://www.日本語.com参照]"
        );
    }

    #[test]
    fn explicit_link_with_cjk_label_unchanged() {
        // `[詳細](https://example.com)` — the label is CJK but the url is not
        // derived from it, so the strip_suffix reconstruction fails and the
        // explicit link is left alone. (The pathological collision
        // `[https://x参照](https://x参照)` is byte-identical to an autolink in
        // mdast and is intentionally NOT covered — see module docs.)
        assert_eq!(
            dump("[詳細](https://example.com)参照"),
            "[A href=https://example.com text=詳細]参照"
        );
    }

    #[test]
    fn schemeless_explicit_link_unchanged() {
        // `[example.com参照](example.com参照)` is an explicit link with no URL
        // scheme — Guard A requires a real scheme (or `www.`), so it is not
        // mistaken for an autolink and is left untouched.
        assert_eq!(
            dump("[example.com参照](example.com参照)"),
            "[A href=example.com参照 text=example.com参照]"
        );
    }

    // --- Re-applied GFM end-trimming after the cut. ---

    #[test]
    fn unbalanced_trailing_paren_is_rebalanced() {
        // An ASCII `(` wraps the URL; markdown-rs kept the `)` because the run
        // ended in CJK, not `)`. After the cut, the unmatched `)` is dropped
        // from the href and moved into the remainder.
        assert_eq!(
            dump("(https://example.com)参照"),
            "([A href=https://example.com text=https://example.com])参照"
        );
    }

    #[test]
    fn balanced_interior_paren_is_kept() {
        // A balanced `(bar)` inside the path stays in the href.
        assert_eq!(
            dump("https://example.com/foo(bar)参照"),
            "[A href=https://example.com/foo(bar) text=https://example.com/foo(bar)]参照"
        );
    }

    #[test]
    fn port_is_kept_in_host() {
        // `:8080` is a port, not a malformed label — the host still validates
        // and the cut lands after the port.
        assert_eq!(
            dump("https://example.com:8080参照"),
            "[A href=https://example.com:8080 text=https://example.com:8080]参照"
        );
    }
}
