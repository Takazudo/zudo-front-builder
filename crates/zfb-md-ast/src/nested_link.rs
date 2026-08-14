//! Nested-link normalisation for GFM autolink literals (zfb#2388).
//!
//! GFM's autolink-literal extension is specified to skip content that is
//! already inside a link — cmark-gfm's extension walks the AST and never
//! descends into a `CMARK_NODE_LINK`. `markdown-rs` (the `markdown` crate)
//! does not: its autolink pass fires inside a link label too, so an
//! ordinary markdown link whose label contains a bare URL yields an `<a>`
//! nested in an `<a>`:
//!
//! ```text
//! [http://localhost:4321](http://localhost:4321)
//! → <a href="http://localhost:4321"><a href="http://localhost:4321">…</a></a>
//! ```
//!
//! Nested anchors are invalid HTML (`html-validate`:
//! `element-permitted-content`), and the defect became reachable on
//! previously-valid documents the moment zfb 2.5.0 flipped
//! `gfm.autolinkLiteral` on by default (a5be2fca) — a silent behavioural
//! regression, not a new authoring mistake.
//!
//! `markdown-rs` is a plain crates.io dependency whose tokeniser is not
//! configurable from outside, so — exactly like
//! [`CjkAutolinkBoundaryPlugin`](crate::cjk_autolink::CjkAutolinkBoundaryPlugin) does for the CJK
//! boundary defect — the fix is a post-parse mdast pass.
//!
//! # Why unwrapping the *inner* link is the correct normalisation
//!
//! The spec position is that the inner autolink should never have been
//! created at all. Replacing the inner `Link` with its own children
//! reproduces exactly that: the visible text survives, and the enclosing
//! link keeps the destination the author actually wrote.
//!
//! # Why unwrapping never discards author-written markup
//!
//! Only a node with the exact shape of an autolink literal is removed — see
//! [`is_autolink_literal`]. Under a markdown `Link` ancestor that guard is
//! belt-and-braces: CommonMark forbids links inside links and `markdown-rs`
//! enforces it for explicit syntax (`[a [b](c) d](e)` degrades the *outer*
//! link to literal text), so the inner node could only ever be an autolink.
//! Under an MDX `<a>` ancestor no such enforcement exists — markdown-rs
//! happily nests a real `[inner](/y)` there — and the guard is load-bearing:
//! without it that author's `/y` destination would vanish silently.
//!
//! # Shapes this has to cover
//!
//! The nesting is not always a direct child — the autolink fires anywhere
//! inside the label, including through intervening inline markup, so the
//! walk has to recurse while remembering it is inside a link:
//!
//! ```text
//! [**bold https://example.com**](https://x.com)
//! → <a …><strong>bold <a …>…</a></strong></a>
//! ```
//!
//! It also fires inside the *inner* link of an outer link that CommonMark
//! already degraded (`[a [b https://e.com c](d) e](f)`), and inside link
//! labels in table cells and list items — all reached by the same recursion.
//!
//! It fires inside author-written MDX JSX too — a JSX element's mdast
//! children are ordinary markdown-parsed nodes — so `<Note>[x](url)</Note>`
//! nests an anchor just like a bare paragraph, and an MDX `<a>` element
//! wrapping a bare URL nests one inside itself. Both are covered; see
//! [`is_no_recurse`] and [`is_link`] for why this pass descends into JSX
//! where the CJK passes stop.
//!
//! Constructs the autolink pass does not enter (inline code, raw HTML,
//! image alt text) never produce the nesting and are skipped.
//!
//! # Why this is applied at the parse sites, not registered as a visitor
//!
//! It runs immediately after `markdown::to_mdast`, ahead of the visitor
//! chain, for two reasons. It covers every pipeline regardless of how it
//! was constructed — the mdast visitors are only wired by the
//! `with_defaults*` constructors, so a bare `Pipeline::new` would
//! otherwise keep emitting nested anchors. And it guarantees that no
//! visitor ever observes the malformed tree:
//! [`CjkAutolinkBoundaryPlugin`](crate::cjk_autolink::CjkAutolinkBoundaryPlugin) and
//! `CjkFriendlyPlugin` both walk `Link` nodes, and
//! reasoning about them against a shape the GFM spec says cannot exist is
//! a trap worth removing outright.
//!
//!
use markdown::mdast::Node as MdastNode;

/// Unwrap every autolink-literal `Link` nested inside something that
/// renders as an `<a>`, replacing it with its own children.
///
/// Call directly after `markdown::to_mdast` and only when
/// `constructs.gfm_autolink_literal` is on: the construct is the sole
/// producer of the nesting this pass removes, so gating keeps every other
/// configuration's output byte-identical.
///
/// Note the gate is about *this* defect, not about nested anchors in
/// general. An MDX `<a>` element with a real markdown link inside it nests
/// with the construct off too — but that markup is entirely author-written,
/// nothing here created it, and the right response (drop the outer? warn?)
/// is a product decision rather than a regression fix. [`is_autolink_literal`]
/// would decline to touch it in any case.
pub fn unwrap_nested_links(node: &mut MdastNode) {
    rewrite(node, false);
}

/// Walk `node`, then flatten its link children once `inside_link` holds.
/// Stops at no-recurse boundaries (verbatim / author-owned content).
///
/// Recursion runs **bottom-up** — children are normalised before this
/// node's own children are flattened — because unwrapping an inner link can
/// be what makes its parent recognisable as an autolink literal. In a
/// stacked `Link > Link > Link`, the middle node holds a `Link` child
/// rather than a `Text` one, so [`is_autolink_literal`] rejects it until
/// the innermost unwrap has collapsed it to a single `Text`. Top-down
/// would leave the middle anchor standing.
fn rewrite(node: &mut MdastNode, inside_link: bool) {
    if is_no_recurse(node) {
        return;
    }
    // A footnote definition's body is RELOCATED at render into the
    // document-level `<section class="footnotes">`, so it never ends up
    // inside whatever link it was written under — there is no nesting to
    // fix, and unwrapping there is pure loss (the autolink in the body
    // simply disappears). Reset the flag rather than carrying it in;
    // a link *within* the body re-arms it normally on the way down.
    // `zfb_md_ast::mdx_jsx::rewrite_jsx_nested` resets its own ancestry flag
    // at this same node for this same reason.
    let inside_link = if matches!(node, MdastNode::FootnoteDefinition(_)) {
        false
    } else {
        inside_link || is_link(node)
    };
    if let Some(children) = node.children_mut() {
        for child in children {
            rewrite(child, inside_link);
        }
    }
    if inside_link {
        if let Some(children) = node.children_mut() {
            let flattened = flatten_links(std::mem::take(children));
            *children = flattened;
        }
    }
}

/// True for a node that renders as an `<a>` element, and so makes anything
/// beneath it "inside a link".
///
/// Covers the two mdast link nodes plus an MDX JSX element literally named
/// `a` — an author writing `<a href="/x">bare https://example.com</a>` in
/// MDX gets the autolink fired inside it exactly like a markdown label,
/// and the result is the same invalid nested anchor. cmark-gfm tracks raw
/// `<a>`/`</a>` for this reason. Only the lowercase intrinsic name counts:
/// a capitalised `<Note>` is a component whose rendered output zfb cannot
/// know, so it is not treated as an anchor.
///
/// This makes an MDX `<a>` a link *ancestor*; it does not make it something
/// the pass will remove. An MDX `<a>` written INSIDE a markdown link —
/// `[x <a href="y">z</a> w](https://q.com)` — stays nested, because only
/// autolink literals are ever unwrapped (see [`is_autolink_literal`]) and
/// that anchor is author-written. So the awareness here is narrower than
/// cmark-gfm's raw-`<a>` tracking: it prevents zfb from *creating* a nested
/// anchor, not from *emitting* one the author wrote by hand.
///
/// `Definition` is deliberately absent — it carries no children and emits
/// nothing. `FootnoteReference` also renders an anchor, but it is a leaf
/// the autolink pass cannot nest a link inside, so it is out of scope here.
fn is_link(node: &MdastNode) -> bool {
    match node {
        MdastNode::Link(_) | MdastNode::LinkReference(_) => true,
        MdastNode::MdxJsxFlowElement(e) => e.name.as_deref() == Some("a"),
        MdastNode::MdxJsxTextElement(e) => e.name.as_deref() == Some("a"),
        _ => false,
    }
}

/// Nodes whose subtree must NOT be touched: verbatim code and raw HTML
/// (whose `value` is an opaque string), and MDX expression bodies
/// (author-written JavaScript).
///
/// Deliberately NARROWER than the set
/// [`CjkAutolinkBoundaryPlugin`](crate::cjk_autolink::CjkAutolinkBoundaryPlugin) and
/// `CjkFriendlyPlugin` use: those also stop at
/// `MdxJsxFlowElement` / `MdxJsxTextElement`, and inheriting that here
/// would leave the bug live inside author-written JSX. A JSX element's
/// mdast *children* are ordinary markdown-parsed nodes — which is exactly
/// why the autolink pass fires in them — so `<Note>[x](url)</Note>` nests
/// an anchor just like a bare paragraph does. Only a JSX element's
/// *attributes* are author-owned, and those are not children, so
/// descending cannot disturb them. Link validation hit the identical JSX
/// blind spot and had to descend as well (zfb#2184 / zfb#2223).
///
/// Container directives are unaffected either way: `DirectiveRegistry`
/// expands `:::note` into an `MdxJsxFlowElement` during the visitor chain,
/// which runs after this pass, over already-normalised content.
///
/// The `Html` arm is dead on every zfb render path and kept only as a
/// guard: `Pipeline` always parses with `markdown::ParseOptions::mdx()`,
/// where `html_text` / `html_flow` are off, so a literal
/// `<a href="/x">bare https://example.com</a>` in a page is an
/// `MdxJsxTextElement` — the shape [`is_link`] handles — and inline
/// `MdastNode::Html` never materialises. The arm matters only if this pass
/// is ever reused on a CommonMark-dialect parse (`facade::parse_mdast` with
/// `ParseDialect::Markdown`), where such a tag DOES become raw `Html` and
/// the autolink inside it becomes a *sibling* `Link`, not a descendant —
/// structurally invisible here, though it still renders as a nested anchor.
///
fn is_no_recurse(node: &MdastNode) -> bool {
    matches!(
        node,
        MdastNode::Code(_)
            | MdastNode::InlineCode(_)
            | MdastNode::Html(_)
            | MdastNode::MdxFlowExpression(_)
            | MdastNode::MdxTextExpression(_)
    )
}

/// Replace each autolink-literal `Link` in `children` with its own
/// children. Recurses into the spliced-in run so a directly-stacked
/// `Link > Link > Link` collapses in one pass; every other child — notably
/// `Image` (legal inside a link), `LinkReference` (never an autolink), and
/// any author-written `[label](dest)` — passes through untouched.
///
/// Adjacent `Text` nodes are left unmerged: an unwrap can leave
/// `Text("see ") Text("https://e.com") Text(" now")` where one node would
/// do. The HTML serializer concatenates them, so only the JSX emitter shows
/// it, as three expressions instead of one — cosmetic, and merging would
/// mean inventing `position` spans for the merged node.
fn flatten_links(children: Vec<MdastNode>) -> Vec<MdastNode> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        match child {
            MdastNode::Link(l) if is_autolink_literal(&l) => out.extend(flatten_links(l.children)),
            other => out.push(other),
        }
    }
    out
}

/// True when `link` has the exact shape `markdown-rs` gives a GFM autolink
/// literal: no title, a single `Text` child, and a `url` that reconstructs
/// from that visible text.
///
/// This is what keeps the pass from destroying author-written markup. For a
/// markdown `Link` ancestor the check is belt-and-braces — CommonMark
/// guarantees the inner node can only be an autolink — but for an MDX `<a>`
/// ancestor there is no such guarantee, and without this guard
/// `<a href="/x">[inner](/y)</a>` would silently lose the author's `/y`
/// destination.
///
/// Three url spellings exist, all verified against real parses: the
/// `http(s)://` form carries its scheme in the visible text (`url ==
/// visible`); the `www.` form gets `http://` prepended; the email form gets
/// `mailto:` prepended. `ftp://` is NOT autolinked by `markdown-rs` at all,
/// so it needs no arm.
///
/// The visible text is checked against what the autolink tokeniser would
/// actually have accepted, not just against the url — mirroring Guard A in
/// [`CjkAutolinkBoundaryPlugin`](crate::cjk_autolink::CjkAutolinkBoundaryPlugin). Matching on the url
/// alone would misread ordinary author links whose destination happens to
/// echo their label: `[/foo](/foo)` satisfies `url == visible`, and
/// `[example.com](http://example.com)` satisfies the `http://` arm, yet
/// `markdown-rs` autolinks neither (a bare host needs a `www.` prefix or a
/// scheme). Inside an MDX `<a>` both would otherwise lose their
/// destination.
///
/// The one case this cannot separate is an author hand-writing
/// `[https://x.com](https://x.com)` — byte-identical to what the autolink
/// pass produces. Unresolvable in mdast, and the same ambiguity
/// [`CjkAutolinkBoundaryPlugin`](crate::cjk_autolink::CjkAutolinkBoundaryPlugin) documents; it is only
/// reachable once bare-URL autolinking is on, the surface that creates the
/// bug.
fn is_autolink_literal(link: &markdown::mdast::Link) -> bool {
    if link.title.is_some() {
        return false;
    }
    let [MdastNode::Text(t)] = link.children.as_slice() else {
        return false;
    };
    let visible = t.value.as_str();

    // GFM matches the scheme and the `www.` host prefix case-INSENSITIVELY,
    // and markdown-rs preserves the author's casing in both the url and the
    // visible text — `WWW.Example.com` autolinks with url
    // `http://WWW.Example.com`. Folding case here is load-bearing: comparing
    // the prefixes case-sensitively leaves every uppercase spelling nested,
    // i.e. reopens the bug for `HTTPS://Example.COM`.
    let lower = visible.to_ascii_lowercase();

    // `http(s)://…` — the scheme is part of the visible text.
    if link.url == visible && (lower.starts_with("http://") || lower.starts_with("https://")) {
        return true;
    }
    // `www.…` — `http://` prepended to a visible host that must start `www.`.
    // markdown-rs always prepends that scheme lowercase whatever the visible
    // casing, so only the visible side needs folding.
    if lower.starts_with("www.") && link.url.strip_prefix("http://") == Some(visible) {
        return true;
    }
    // `user@host` — `mailto:` prepended to a visible address. Reachable only
    // under an MDX `<a>` ancestor: markdown-rs does not fire the email form
    // inside a markdown link label.
    visible.contains('@') && link.url.strip_prefix("mailto:") == Some(visible)
}

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{
        Image, InlineCode, Link, LinkReference, Paragraph, ReferenceKind, Root, Strong, Text,
    };

    fn text(value: &str) -> MdastNode {
        MdastNode::Text(Text {
            value: value.to_string(),
            position: None,
        })
    }

    fn link(url: &str, children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Link(Link {
            url: url.to_string(),
            title: None,
            children,
            position: None,
        })
    }

    fn link_ref(identifier: &str, children: Vec<MdastNode>) -> MdastNode {
        MdastNode::LinkReference(LinkReference {
            identifier: identifier.to_string(),
            label: Some(identifier.to_string()),
            reference_kind: ReferenceKind::Full,
            children,
            position: None,
        })
    }

    fn para(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children,
            position: None,
        })
    }

    fn root(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Root(Root {
            children,
            position: None,
        })
    }

    fn jsx_text(name: &str, children: Vec<MdastNode>) -> MdastNode {
        MdastNode::MdxJsxTextElement(markdown::mdast::MdxJsxTextElement {
            name: Some(name.to_string()),
            attributes: vec![],
            children,
            position: None,
        })
    }

    fn strong(children: Vec<MdastNode>) -> MdastNode {
        MdastNode::Strong(Strong {
            children,
            position: None,
        })
    }

    /// The issue's headline repro: a label that is itself a bare URL.
    #[test]
    fn unwraps_a_directly_nested_autolink() {
        let mut tree = root(vec![para(vec![link(
            "http://localhost:4321",
            vec![link(
                "http://localhost:4321",
                vec![text("http://localhost:4321")],
            )],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![link(
            "http://localhost:4321",
            vec![text("http://localhost:4321")],
        )])]);
        assert_eq!(tree, expected);
    }

    /// A bare URL merely embedded in the label keeps its surrounding text.
    #[test]
    fn unwraps_an_embedded_autolink_and_keeps_siblings() {
        let mut tree = root(vec![para(vec![link(
            "https://example.com",
            vec![
                text("see "),
                link("https://example.com", vec![text("https://example.com")]),
                text(" now"),
            ],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![link(
            "https://example.com",
            vec![text("see "), text("https://example.com"), text(" now")],
        )])]);
        assert_eq!(tree, expected);
    }

    /// The nesting is not always a direct child — the autolink fires through
    /// intervening inline markup, so the walk must stay "inside a link".
    #[test]
    fn unwraps_through_intervening_inline_markup() {
        let mut tree = root(vec![para(vec![link(
            "https://x.com",
            vec![strong(vec![
                text("bold "),
                link("https://example.com", vec![text("https://example.com")]),
            ])],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![link(
            "https://x.com",
            vec![strong(vec![text("bold "), text("https://example.com")])],
        )])]);
        assert_eq!(tree, expected);
    }

    /// `Link > Link > Link` collapses in a single pass.
    #[test]
    fn collapses_directly_stacked_links() {
        let mut tree = root(vec![para(vec![link(
            "https://a.com",
            vec![link(
                "https://a.com",
                vec![link("https://a.com", vec![text("https://a.com")])],
            )],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![link(
            "https://a.com",
            vec![text("https://a.com")],
        )])]);
        assert_eq!(tree, expected);
    }

    /// A `LinkReference` renders an `<a>`, so it counts as a link ancestor
    /// and an autolink nested in its label is unwrapped. This is a real
    /// shape: `[see https://example.com now][ref]` parses exactly this way.
    #[test]
    fn unwraps_an_autolink_inside_a_link_reference_label() {
        let mut tree = root(vec![para(vec![link_ref(
            "ref",
            vec![link("https://y.com", vec![text("https://y.com")])],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![link_ref(
            "ref",
            vec![text("https://y.com")],
        )])]);
        assert_eq!(tree, expected);
    }

    /// A `LinkReference` is never something the autolink pass produced, so
    /// one sitting inside a link is author-written and must be preserved —
    /// dropping it would discard the reference and its destination.
    #[test]
    fn preserves_a_link_reference_nested_inside_a_link() {
        let mut tree = root(vec![para(vec![link(
            "https://x.com",
            vec![link_ref("ref", vec![text("label")])],
        )])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }

    /// The regression guard for the MDX `<a>` ancestor: markdown-rs does
    /// NOT enforce no-links-in-links across a JSX boundary, so a real
    /// author-written `[inner](/y)` can sit inside `<a href="/x">`. It must
    /// survive — unwrapping it would silently discard `/y`.
    #[test]
    fn preserves_an_author_written_link_inside_an_mdx_anchor() {
        let mut tree = root(vec![para(vec![jsx_text(
            "a",
            vec![link("/y", vec![text("inner")])],
        )])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }

    /// Author links whose destination merely echoes their label are NOT
    /// autolinks — markdown-rs autolinks neither a bare path nor a bare
    /// host without `www.` or a scheme. Matching on the url alone would
    /// destroy both.
    #[test]
    fn preserves_author_links_whose_url_echoes_their_label() {
        let mut tree = root(vec![para(vec![jsx_text(
            "a",
            vec![
                link("/foo", vec![text("/foo")]),
                link("http://example.com", vec![text("example.com")]),
                link("mailto:team", vec![text("team")]),
            ],
        )])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }

    /// A titled link is never an autolink literal, so it is preserved even
    /// when its url happens to equal its visible text.
    #[test]
    fn preserves_a_titled_link_inside_a_link() {
        let mut tree = root(vec![para(vec![jsx_text(
            "a",
            vec![MdastNode::Link(Link {
                url: "https://y.com".to_string(),
                title: Some("t".to_string()),
                children: vec![text("https://y.com")],
                position: None,
            })],
        )])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }

    /// The `www.` and email autolink spellings put a scheme in `url` that is
    /// absent from the visible text; both must still be recognised.
    #[test]
    fn recognises_the_www_and_email_autolink_spellings() {
        let mut tree = root(vec![para(vec![link(
            "https://x.com",
            vec![
                link("http://www.example.com", vec![text("www.example.com")]),
                text(" and "),
                link("mailto:a@b.com", vec![text("a@b.com")]),
            ],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![link(
            "https://x.com",
            vec![text("www.example.com"), text(" and "), text("a@b.com")],
        )])]);
        assert_eq!(tree, expected);
    }

    /// A top-level autolink is not nested — it must survive untouched.
    #[test]
    fn leaves_an_unnested_link_alone() {
        let mut tree = root(vec![para(vec![
            text("bare "),
            link("https://example.com", vec![text("https://example.com")]),
            text(" here"),
        ])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }

    /// An `Image` inside a link is valid HTML and must be preserved.
    #[test]
    fn preserves_an_image_inside_a_link() {
        let mut tree = root(vec![para(vec![link(
            "https://example.com",
            vec![MdastNode::Image(Image {
                url: "img.png".to_string(),
                alt: "alt".to_string(),
                title: None,
                position: None,
            })],
        )])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }

    /// Verbatim subtrees are boundaries: markdown-rs keeps their content in
    /// an opaque `value` string, so a URL parked in one is left untouched.
    #[test]
    fn does_not_descend_into_verbatim_subtrees() {
        let mut tree = root(vec![para(vec![link(
            "https://x.com",
            vec![MdastNode::InlineCode(InlineCode {
                value: "https://code.com".to_string(),
                position: None,
            })],
        )])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }

    /// A JSX element's children ARE markdown-parsed, so the autolink fires
    /// in them and the walk must descend — unlike the CJK passes, which
    /// stop at JSX. `<Note>` here stands for any component.
    #[test]
    fn descends_into_jsx_element_children() {
        let mut tree = root(vec![para(vec![jsx_text(
            "Note",
            vec![link(
                "https://example.com",
                vec![
                    text("see "),
                    link("https://example.com", vec![text("https://example.com")]),
                    text(" now"),
                ],
            )],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![jsx_text(
            "Note",
            vec![link(
                "https://example.com",
                vec![text("see "), text("https://example.com"), text(" now")],
            )],
        )])]);
        assert_eq!(tree, expected);
    }

    /// An MDX element literally named `a` IS an anchor, so a bare URL
    /// autolinked inside it nests and must be unwrapped.
    #[test]
    fn treats_a_lowercase_jsx_a_element_as_a_link_ancestor() {
        let mut tree = root(vec![para(vec![jsx_text(
            "a",
            vec![
                text("bare "),
                link("https://example.com", vec![text("https://example.com")]),
            ],
        )])]);

        unwrap_nested_links(&mut tree);

        let expected = root(vec![para(vec![jsx_text(
            "a",
            vec![text("bare "), text("https://example.com")],
        )])]);
        assert_eq!(tree, expected);
    }

    /// A capitalised component is NOT an anchor — zfb cannot know what it
    /// renders — so a link directly inside it stays a link.
    #[test]
    fn does_not_treat_a_component_as_a_link_ancestor() {
        let mut tree = root(vec![para(vec![jsx_text(
            "Anchor",
            vec![link(
                "https://example.com",
                vec![text("https://example.com")],
            )],
        )])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }

    /// Author-written JavaScript in an MDX expression is never touched.
    #[test]
    fn does_not_descend_into_mdx_expressions() {
        let mut tree = root(vec![para(vec![link(
            "https://x.com",
            vec![MdastNode::MdxTextExpression(
                markdown::mdast::MdxTextExpression {
                    value: "\"https://expr.com\"".to_string(),
                    position: None,
                    stops: vec![],
                },
            )],
        )])]);
        let expected = tree.clone();

        unwrap_nested_links(&mut tree);

        assert_eq!(tree, expected);
    }
}
