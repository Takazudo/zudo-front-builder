//! Shared AST types and visitor traits for the zfb markdown/MDX pipeline.
//!
//! This crate is a thin shared-types layer so `zfb-md-extras` (and other
//! downstream crates) can depend on these definitions without pulling in all
//! of `zfb-content`.

use markdown::mdast::Node as MdastNode;

/// Lightweight HTML AST node.
///
/// Plugins (mdast and hast visitors) operate on this representation in
/// memory; the serializer turns it into an HTML string later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HastNode {
    /// Document root.
    Root {
        /// Top-level children.
        children: Vec<HastNode>,
    },
    /// HTML element.
    Element {
        /// Tag name (e.g. `"p"`, `"h1"`, `"a"`).
        tag: String,
        /// Attribute list as `(name, value)` pairs.
        ///
        /// Attribute order is preserved so the serializer produces stable
        /// output and so plugins can assert on ordering when useful.
        attrs: Vec<(String, String)>,
        /// Child nodes; empty for void elements.
        children: Vec<HastNode>,
        /// True for self-closing void elements (`img`, `br`, `hr`, etc.).
        void: bool,
    },
    /// Plain text content (escaped on serialization).
    Text(String),
    /// Raw HTML passthrough; the serializer emits this verbatim without
    /// escaping. Produced by the mdast→hast conversion for
    /// `markdown::mdast::Node::Html`, and by hast plugins that synthesize
    /// complete HTML fragments (e.g. syntect).
    ///
    /// On the JSX-emit path (`mdx_jsx_emit::mdx_to_jsx_module_with_pipeline`),
    /// `Raw` cannot be embedded verbatim — JSX does not understand
    /// arbitrary HTML such as `class="…"` or inline `<span style="…">`.
    /// The hast→JSX bridge wraps `Raw` content in a span with
    /// `dangerouslySetInnerHTML` so the rendered DOM still receives the
    /// original markup. See [`HastNode::JsxRaw`] for the JSX-shaped
    /// counterpart that IS safe to inline.
    Raw(String),
    /// JSX-shaped passthrough — MDX components (`<Note>…</Note>`),
    /// flow / text expressions (`{1 + 1}`), and synthesized JSX
    /// fragments. The serializer treats this identically to
    /// [`HastNode::Raw`] (verbatim, no escaping); the JSX-emit path
    /// embeds it verbatim into the output module so PascalCase
    /// component references and `{…}` expression containers survive
    /// untouched.
    ///
    /// Splitting JSX from HTML at the hast level lets the JSX bridge
    /// pick the right embedding strategy without parsing the payload.
    JsxRaw(String),
    /// HTML comment body (without the `<!--` / `-->` delimiters).
    Comment(String),
}

/// Mdast visitor: mutates an mdast tree in place.
///
/// Implementors typically call [`MdastNode::children_mut`] to recurse, or
/// implement their own walk. The pipeline does NOT auto-recurse for
/// visitors; each visitor decides its own traversal strategy.
pub trait MdastVisitor {
    /// Visit (and possibly mutate) `node`.
    fn visit(&mut self, node: &mut MdastNode);
}

/// Hast visitor: mutates a hast tree in place.
///
/// Same recursion contract as [`MdastVisitor`].
pub trait HastVisitor {
    /// Visit (and possibly mutate) `node`.
    fn visit(&mut self, node: &mut HastNode);

    /// Reset any per-document state accumulated during [`Self::visit`].
    ///
    /// Called by `Pipeline::reset_per_entry` between documents so
    /// cross-document state (e.g. duplicate-slug counters in
    /// `HeadingLinksPlugin`) cannot leak from one entry to the next.
    /// The default implementation is a no-op, which is correct for
    /// stateless visitors. Stateful visitors (currently only
    /// `HeadingLinksPlugin`) override this method.
    fn reset(&mut self) {}
}
