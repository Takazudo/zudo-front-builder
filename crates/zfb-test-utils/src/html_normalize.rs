//! HTML normalization helper for snapshot-based integration tests.
//!
//! # Design
//!
//! We use **html5ever** (browser-grade HTML5 parser) plus **markup5ever_rcdom**
//! (mutable reference-counted DOM). This pairing gives us:
//!
//! - Canonical parsing: `<br>`, `<br/>`, `<br />` all become the same DOM node.
//! - Canonical serialization via `html5ever::serialize`: void elements are
//!   emitted as `<br>` (no `/>`); text in `<script>`/`<style>` is written
//!   verbatim; text in other elements uses minimal HTML5 escaping.
//! - Attribute sorting: we walk the DOM and sort each element's attribute list
//!   lexicographically before serializing, producing a stable order.
//!
//! # Entity encoding canonical form
//!
//! The html5ever serializer emits exactly: `&amp;`, `&lt;`, `&gt;` (in text),
//! `&quot;` and `&amp;` (in attribute values), and `&nbsp;`. No other named
//! entities are emitted. Input entities like `&#x27;`, `&apos;`, `&#39;` are
//! decoded by the parser to their Unicode characters and re-encoded with the
//! same minimal set. This is the **canonical form** for this project.
//!
//! # Literal-text preservation
//!
//! Inside `<pre>`, `<code>`, `<textarea>`, `<script>`, and `<style>` the
//! serializer writes text verbatim (no whitespace squashing). This is handled
//! by the html5ever serializer for `<script>` / `<style>`, and by our
//! whitespace-collapse pass which skips those contexts for the rest.
//!
//! # Whitespace collapse
//!
//! Inter-element whitespace (text nodes that are pure whitespace between block
//! elements) is collapsed to a single newline. Whitespace *inside* text nodes
//! that contain non-whitespace characters is preserved. Whitespace inside
//! raw-text contexts (`<pre>`, `<code>`, `<textarea>`, `<script>`, `<style>`)
//! is never touched.
//!
//! # Boolean attributes
//!
//! HTML5 boolean attributes (`disabled`, `checked`, `readonly`, `required`,
//! `multiple`, `autofocus`, `selected`, `open`, `hidden`, `async`, `defer`,
//! `autoplay`, `controls`, `loop`, `muted`, `default`, `formnovalidate`,
//! `novalidate`, `ismap`, `reversed`, `scoped`, `seamless`, `allowfullscreen`,
//! `typemustmatch`) are canonicalized to the empty-string form (`disabled=""`).
//! The html5ever parser already does this — it stores the attribute with an
//! empty-string value regardless of whether the source had `disabled`,
//! `disabled=""`, or `disabled="disabled"`.

use html5ever::driver::ParseOpts;
use html5ever::parse_fragment;
use html5ever::serialize::{SerializeOpts, TraversalScope};
use html5ever::tendril::TendrilSink;
use html5ever::{namespace_url, ns, LocalName, QualName};
use markup5ever_rcdom::{NodeData, RcDom, SerializableHandle};

/// HTML5 boolean attributes whose value should be canonicalized to the
/// empty-string form (`disabled=""`) rather than the redundant
/// `disabled="disabled"` form. html5ever already normalizes the bare
/// `disabled` form to `""` during parsing; this list handles the
/// `disabled="disabled"` form that the parser preserves as-is.
///
/// Source: https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#boolean-attributes
const BOOLEAN_ATTRS: &[&str] = &[
    "allowfullscreen",
    "async",
    "autofocus",
    "autoplay",
    "checked",
    "controls",
    "default",
    "defer",
    "disabled",
    "formnovalidate",
    "hidden",
    "ismap",
    "loop",
    "multiple",
    "muted",
    "nomodule",
    "novalidate",
    "open",
    "readonly",
    "required",
    "reversed",
    "selected",
    "typemustmatch",
];

/// Normalize an HTML fragment for snapshot comparison.
///
/// Idempotent: calling `normalize_html(normalize_html(s))` returns the same
/// result as `normalize_html(s)`.
///
/// See the module-level doc for the full specification of what is and isn't
/// changed.
///
/// # Verbatim opt-out
///
/// This function always normalizes. The per-fixture verbatim opt-out is
/// handled by the caller (`run_fixture`) which reads `normalize.txt` from the
/// fixture directory.
pub fn normalize_html(html: &str) -> String {
    // parse_fragment with a <body> context so that all nodes — including
    // <script> and <style> — are treated as body content and not hoisted
    // to <head>. parse_document would move <script>/<style> into <head>.
    let context_name = QualName::new(None, ns!(html), LocalName::from("body"));
    let dom = parse_fragment(
        RcDom::default(),
        ParseOpts::default(),
        context_name,
        vec![],
    )
    .one(html.to_string());

    // Walk the DOM: sort attributes, normalize boolean attr values, and
    // collapse inter-element whitespace.
    sort_attrs_and_collapse_whitespace(&dom.document);

    // html5ever's fragment parse result has the structure:
    //   Document → html → body → [the actual fragment nodes]
    // We extract the body element and serialize its children directly.
    let body = find_body(&dom.document)
        .unwrap_or_else(|| dom.document.clone());

    let mut output = Vec::new();
    let handle: SerializableHandle = body.clone().into();
    // scripting_enabled: true — when false the HTML5 parser treats <script>
    // content as markup (spec §8.2.6.5). We always want <script>/<style>
    // text preserved verbatim, which requires parsing them as raw text.
    html5ever::serialize(
        &mut output,
        &handle,
        SerializeOpts {
            traversal_scope: TraversalScope::ChildrenOnly(None),
            scripting_enabled: true,
            create_missing_parent: false,
        },
    )
    .expect("html5ever serialization is infallible for well-formed DOM");

    let serialized = String::from_utf8(output).expect("html5ever always emits valid UTF-8");
    serialized.trim().to_string()
}

/// Find the `<body>` element in a parsed DOM tree by walking the document
/// node's subtree depth-first.
fn find_body(node: &markup5ever_rcdom::Handle) -> Option<markup5ever_rcdom::Handle> {
    for child in node.children.borrow().iter() {
        if let NodeData::Element { ref name, .. } = child.data {
            if name.local.as_ref() == "body" {
                return Some(child.clone());
            }
        }
        if let Some(found) = find_body(child) {
            return Some(found);
        }
    }
    None
}

/// Recursively walk a DOM node, sorting each element's attribute list
/// lexicographically by local name, and collapsing inter-element whitespace
/// (pure-whitespace text nodes become a single newline).
///
/// Raw-text contexts (`<pre>`, `<code>`, `<textarea>`, `<script>`, `<style>`)
/// are preserved verbatim — their text-node children are not touched.
fn sort_attrs_and_collapse_whitespace(node: &markup5ever_rcdom::Handle) {
    // Determine if this node is a raw-text context (no whitespace collapse
    // inside it).
    let is_raw = match &node.data {
        NodeData::Element { name, .. } => {
            matches!(
                name.local.as_ref(),
                "pre" | "code" | "textarea" | "script" | "style"
            )
        }
        _ => false,
    };

    // Sort attributes on this element node and normalize boolean attr values.
    if let NodeData::Element { attrs, .. } = &node.data {
        let mut attrs_mut = attrs.borrow_mut();
        // Normalize boolean attributes: any attr whose value equals its own name
        // (e.g. `disabled="disabled"`) is canonicalized to the empty-string form
        // (`disabled=""`). html5ever already normalizes bare `disabled` → `""`,
        // but `disabled="disabled"` is preserved as-is by the parser.
        for attr in attrs_mut.iter_mut() {
            let local = attr.name.local.as_ref();
            if BOOLEAN_ATTRS.contains(&local) && attr.value.as_ref() == local {
                attr.value = "".into();
            }
        }
        attrs_mut.sort_by(|a, b| a.name.local.cmp(&b.name.local));
    }

    // Process children.
    let children = node.children.borrow();
    for child in children.iter() {
        if !is_raw {
            // Collapse pure-whitespace text nodes to a single newline.
            if let NodeData::Text { contents } = &child.data {
                let text = contents.borrow();
                if text.trim().is_empty() && !text.is_empty() {
                    drop(text);
                    *contents.borrow_mut() = "\n".into();
                    continue;
                }
            }
        }
        sort_attrs_and_collapse_whitespace(child);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    // ── Attribute order canonicalization ──────────────────────────────────

    #[test]
    fn test_attr_order_canonical() {
        // Attributes are sorted lexicographically by local name.
        // Input has rel before href; output must have href before rel.
        let input = r#"<a rel="noreferrer noopener" href="x">y</a>"#;
        let out = normalize_html(input);
        // href comes before rel lexicographically
        let href_pos = out.find("href=").expect("href attr present");
        let rel_pos = out.find("rel=").expect("rel attr present");
        assert!(href_pos < rel_pos, "href must come before rel; got: {out}");
    }

    #[test]
    fn test_attr_order_negative_out_of_order() {
        // Negative path: verify the raw HTML has rel before href (the wrong order),
        // and normalize_html fixes it.
        let input = r#"<a rel="noreferrer noopener" href="x">y</a>"#;
        let href_pos_raw = input.find("href=").unwrap();
        let rel_pos_raw = input.find("rel=").unwrap();
        assert!(
            rel_pos_raw < href_pos_raw,
            "precondition: rel before href in raw input"
        );
        // After normalization the order is reversed.
        let out = normalize_html(input);
        let href_pos = out.find("href=").unwrap();
        let rel_pos = out.find("rel=").unwrap();
        assert!(href_pos < rel_pos, "normalize_html must sort attrs; got: {out}");
    }

    // ── Entity encoding ───────────────────────────────────────────────────

    #[test]
    fn test_entity_encoding_canonical() {
        // &amp; in input stays &amp;; < becomes &lt;; > becomes &gt; in text.
        let input = "<p>a &amp; b &lt;c&gt; d</p>";
        let out = normalize_html(input);
        assert!(out.contains("a &amp; b"), "ampersand preserved; got: {out}");
        assert!(out.contains("&lt;c&gt;"), "lt/gt preserved; got: {out}");
    }

    #[test]
    fn test_entity_encoding_apos_normalizes() {
        // &#x27; and &apos; are decoded to ' by the parser and re-encoded
        // minimally. In body text, apostrophe does NOT need to be escaped,
        // so html5ever serializes it as a literal '.
        let input = "<p>it&#x27;s fine</p>";
        let out = normalize_html(input);
        // Decoded to literal apostrophe (html5ever minimal encoding in body text)
        assert!(out.contains("it's fine"), "apostrophe decoded; got: {out}");
    }

    #[test]
    fn test_entity_encoding_negative_non_canonical_form() {
        // Prove that &#x27; and &apos; are NOT preserved as-is — they get decoded.
        let with_apos = normalize_html("<p>a &apos; b</p>");
        let with_x27 = normalize_html("<p>a &#x27; b</p>");
        let with_literal = normalize_html("<p>a ' b</p>");
        assert_eq!(with_apos, with_literal, "apos → literal apostrophe");
        assert_eq!(with_x27, with_literal, "&#x27; → literal apostrophe");
    }

    // ── Boolean attributes ────────────────────────────────────────────────

    #[test]
    fn test_boolean_attr_bare_form() {
        // html5ever parses <input disabled> and stores the attribute value as "".
        // The serializer emits disabled="".
        let input = "<input disabled>";
        let out = normalize_html(input);
        assert!(
            out.contains(r#"disabled=""#),
            "boolean attr must have empty-string value; got: {out}"
        );
    }

    #[test]
    fn test_boolean_attr_disabled_disabled_form() {
        // <input disabled="disabled"> normalizes to same form as bare <input disabled>.
        let a = normalize_html("<input disabled>");
        let b = normalize_html(r#"<input disabled="">"#);
        let c = normalize_html(r#"<input disabled="disabled">"#);
        assert_eq!(a, b, "bare vs empty-string disabled must normalize equal");
        assert_eq!(a, c, "disabled=disabled vs bare must normalize equal");
    }

    #[test]
    fn test_boolean_attr_negative_original_forms_differ() {
        // Negative path: the three raw forms are textually different.
        let bare = "<input disabled>";
        let empty = r#"<input disabled="">"#;
        let redundant = r#"<input disabled="disabled">"#;
        assert_ne!(bare, empty, "precondition: raw forms differ");
        assert_ne!(bare, redundant, "precondition: raw forms differ");
    }

    // ── Self-closing / void elements ──────────────────────────────────────

    #[test]
    fn test_void_element_canonical_form() {
        // html5ever serializes all void elements as <br> (no />) per HTML5 spec.
        // All three input forms must produce the same output.
        let a = normalize_html("<p>a<br>b</p>");
        let b = normalize_html("<p>a<br/>b</p>");
        let c = normalize_html("<p>a<br />b</p>");
        assert_eq!(a, b, "br vs br/ must normalize equal");
        assert_eq!(a, c, "br vs br-space-/ must normalize equal");
        // Output must not contain "/>" — HTML5 serializer never emits that.
        assert!(!a.contains("/>"), "void elements must not have />; got: {a}");
    }

    #[test]
    fn test_void_element_negative_xhtml_form_present_in_input() {
        // Negative: input really does contain the XHTML self-close form.
        let raw = "<p>a<br />b</p>";
        assert!(raw.contains("/>"), "precondition: raw input has />");
        let out = normalize_html(raw);
        assert!(!out.contains("/>"), "output must not have />; got: {out}");
    }

    // ── Whitespace between elements ───────────────────────────────────────

    #[test]
    fn test_inter_element_whitespace_collapsed() {
        // Pure-whitespace text nodes between elements become a single newline.
        let input = "<div>  \n\t  <p>hello</p>  \n  </div>";
        let out = normalize_html(input);
        // No run of multiple whitespace-only characters should remain.
        assert!(
            !out.contains("  \n\t"),
            "inter-element whitespace must be collapsed; got: {out}"
        );
    }

    #[test]
    fn test_mixed_text_whitespace_preserved() {
        // Text nodes with actual content are not modified.
        let input = "<p>hello   world</p>";
        let out = normalize_html(input);
        assert!(
            out.contains("hello   world"),
            "inline text whitespace must be preserved; got: {out}"
        );
    }

    #[test]
    fn test_whitespace_negative_significant_content_not_collapsed() {
        // Negative: prove that a text node with content is preserved.
        let input = "<p>  spaces  </p>";
        let out = normalize_html(input);
        assert!(
            out.contains("  spaces  "),
            "significant whitespace must survive; got: {out}"
        );
    }

    // ── Literal text preservation ─────────────────────────────────────────

    #[test]
    fn test_pre_literal_preserved() {
        // Whitespace inside <pre> must not be squashed.
        let input = "<pre>  line one\n    line two\n</pre>";
        let out = normalize_html(input);
        assert!(
            out.contains("  line one\n    line two"),
            "pre content must be verbatim; got: {out}"
        );
    }

    #[test]
    fn test_code_literal_preserved() {
        let input = "<code>  a   b  </code>";
        let out = normalize_html(input);
        assert!(
            out.contains("  a   b  "),
            "code content must be verbatim; got: {out}"
        );
    }

    #[test]
    fn test_script_literal_preserved() {
        let input = "<script>var x =   1;</script>";
        let out = normalize_html(input);
        assert!(
            out.contains("var x =   1;"),
            "script content must be verbatim; got: {out}"
        );
    }

    #[test]
    fn test_style_literal_preserved() {
        let input = "<style>body {   color: red; }</style>";
        let out = normalize_html(input);
        assert!(
            out.contains("body {   color: red; }"),
            "style content must be verbatim; got: {out}"
        );
    }

    #[test]
    fn test_textarea_literal_preserved() {
        let input = "<textarea>  hello   world  </textarea>";
        let out = normalize_html(input);
        assert!(
            out.contains("  hello   world  "),
            "textarea content must be verbatim; got: {out}"
        );
    }

    #[test]
    fn test_literal_negative_outside_raw_context_is_collapsed() {
        // Negative: outside raw-text contexts, pure-whitespace nodes ARE collapsed.
        let input = "<div>   </div>";
        let out = normalize_html(input);
        // The whitespace-only text node should be reduced to \n, not "   ".
        assert!(
            !out.contains("   "),
            "pure-whitespace outside raw context must collapse; got: {out}"
        );
    }

    // ── Debug ─────────────────────────────────────────────────────────────

    // ── Idempotency ───────────────────────────────────────────────────────

    #[test]
    fn test_normalize_is_idempotent() {
        let input = r#"<a rel="noreferrer noopener" href="x">y &amp; z</a>"#;
        let once = normalize_html(input);
        let twice = normalize_html(&once);
        assert_eq!(once, twice, "normalize_html must be idempotent");
    }
}
