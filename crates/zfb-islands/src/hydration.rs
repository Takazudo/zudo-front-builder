//! Hydration HTML emit.
//!
//! Given a page's server-rendered HTML and a set of *islands* that were
//! actually used by that page, rewrite each island's outer markup so it
//! carries the metadata the client-side hydration runtime walks.
//!
//! Output shape per island:
//!
//! ```html
//! <div data-zfb-island="ComponentName"
//!      data-props="<json-encoded props, html-escaped>"
//!      data-when="visible|idle|load">
//!   …server-output…
//! </div>
//! ```
//!
//! ## Locating islands in the rendered HTML
//!
//! Locating an island deterministically inside an opaque HTML blob is the
//! tricky bit. There are two reasonable approaches:
//!
//! 1. **Marker-based.** The renderer wraps each island's server output with
//!    a sentinel pair (here: HTML comments shaped like
//!    `<!--zfb-island:KEY-->…<!--/zfb-island:KEY-->`). Sub 3 owns the
//!    rewriter; the renderer in `zfb-render` and the `<Island>` wrapper in
//!    Sub 4 are expected to emit markers as they integrate.
//!
//! 2. **AST-based.** Parse the rendered HTML and locate islands structurally.
//!    This is more robust against hand-authored markup but adds an HTML parse
//!    on every render and is overkill while the renderer is the only thing
//!    producing islands.
//!
//! We pick **option 1** for now and keep the rewriter narrow enough that we
//! can swap in an AST-based locator later behind the same public surface
//! ([`rewrite_islands`]) without touching callers. The marker shape is
//! intentionally a plain string so the renderer can emit it without dragging
//! in this crate as a dependency.
//!
//! ### Follow-up: marker emission in the renderer
//!
//! The renderer in `zfb-render` does not yet emit these markers. That work
//! belongs with the `<Island>` wrapper (Sub 4) and is tracked there. Until
//! the renderer emits markers, the rewriter is exercised only by tests in
//! this crate.

use serde::Serialize;
use thiserror::Error;

/// One island actually used by a rendered page.
///
/// Field semantics:
/// - [`component_name`](Self::component_name): stable identifier the runtime
///   uses to pick the right component out of the islands bundle. Must match
///   the export name produced by Sub 1's scanner.
/// - [`props_json`](Self::props_json): the serialised props passed to the
///   component at server-render time. Must already be valid JSON; the
///   rewriter does not validate it (the renderer is expected to obtain it
///   via `serde_json::to_string` on the same `JsonValue` it handed to the
///   component).
/// - [`marker_key`](Self::marker_key): the unique key the renderer used in
///   the surrounding `<!--zfb-island:KEY-->` / `<!--/zfb-island:KEY-->`
///   sentinel pair. Each key is expected to appear at most once per page.
/// - [`when`](Self::when): optional `data-when` hydration hint. `None`
///   means no attribute is emitted; the runtime treats the absent case as
///   `"load"` (immediate hydration).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IslandDescriptor {
    /// Component export name as produced by the islands AST scanner.
    pub component_name: String,
    /// Pre-serialised props JSON. Must be valid JSON; rewriter does not
    /// validate.
    pub props_json: String,
    /// The unique key used in the renderer's `<!--zfb-island:KEY-->` /
    /// `<!--/zfb-island:KEY-->` sentinel pair around this island's server
    /// output.
    pub marker_key: String,
    /// Optional `data-when` hint (`"visible"` / `"idle"` / `"load"`).
    /// Owned by Sub 4's `<Island>` wrapper. `None` ⇒ omit the attribute.
    pub when: Option<String>,
}

impl IslandDescriptor {
    /// Build a descriptor with no `data-when` hint (immediate hydration).
    pub fn new(
        component_name: impl Into<String>,
        props_json: impl Into<String>,
        marker_key: impl Into<String>,
    ) -> Self {
        Self {
            component_name: component_name.into(),
            props_json: props_json.into(),
            marker_key: marker_key.into(),
            when: None,
        }
    }

    /// Set a `data-when` hint. Caller is responsible for using one of the
    /// runtime-recognised values (`"visible"`, `"idle"`, `"load"`).
    pub fn with_when(mut self, when: impl Into<String>) -> Self {
        self.when = Some(when.into());
        self
    }
}

/// Errors the rewriter surfaces when the input HTML and the descriptors are
/// inconsistent. Callers should propagate these as build-time render errors;
/// each variant points at the descriptor whose marker is at fault.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IslandRewriteError {
    /// The opening `<!--zfb-island:KEY-->` sentinel was not found in the
    /// rendered HTML. Either the renderer skipped emitting it for this
    /// island or the key is wrong.
    #[error("opening marker not found for island `{component}` (key `{key}`)")]
    OpenMarkerMissing {
        /// Component name from the descriptor.
        component: String,
        /// Marker key from the descriptor.
        key: String,
    },
    /// The closing `<!--/zfb-island:KEY-->` sentinel was missing or not
    /// after the opening marker. Indicates a renderer bug.
    #[error("closing marker not found for island `{component}` (key `{key}`)")]
    CloseMarkerMissing {
        /// Component name from the descriptor.
        component: String,
        /// Marker key from the descriptor.
        key: String,
    },
    /// Two descriptors used the same `marker_key`. Each island must have a
    /// distinct key per page; this is otherwise a silent footgun (the
    /// rewriter would only match the first occurrence).
    #[error("duplicate marker key `{key}`")]
    DuplicateKey {
        /// The duplicated marker key.
        key: String,
    },
}

/// Rewrite each island's marker-bracketed server output into the
/// `<div data-zfb-island="…" data-props="…">…</div>` wrapper.
///
/// This is the swappable seam for the marker-based vs. AST-based approach
/// described in the module docs. The signature is deliberately kept narrow
/// (HTML in, HTML out, errors out) so the implementation can be replaced
/// without touching call sites.
///
/// # Errors
///
/// - [`IslandRewriteError::DuplicateKey`] if two descriptors share a
///   `marker_key`.
/// - [`IslandRewriteError::OpenMarkerMissing`] /
///   [`IslandRewriteError::CloseMarkerMissing`] if the renderer's sentinel
///   pair for a descriptor is not present in the input HTML.
pub fn rewrite_islands(
    html: &str,
    islands: &[IslandDescriptor],
) -> Result<String, IslandRewriteError> {
    // Reject duplicate keys up front. Duplicates would otherwise rewrite
    // only the first occurrence and silently corrupt the page.
    {
        let mut seen: Vec<&str> = Vec::with_capacity(islands.len());
        for d in islands {
            if seen.contains(&d.marker_key.as_str()) {
                return Err(IslandRewriteError::DuplicateKey {
                    key: d.marker_key.clone(),
                });
            }
            seen.push(&d.marker_key);
        }
    }

    let mut out = String::from(html);
    for d in islands {
        let open = format!("<!--zfb-island:{}-->", d.marker_key);
        let close = format!("<!--/zfb-island:{}-->", d.marker_key);

        let open_idx = out
            .find(&open)
            .ok_or_else(|| IslandRewriteError::OpenMarkerMissing {
                component: d.component_name.clone(),
                key: d.marker_key.clone(),
            })?;
        let after_open = open_idx + open.len();
        let close_rel = out[after_open..].find(&close).ok_or_else(|| {
            IslandRewriteError::CloseMarkerMissing {
                component: d.component_name.clone(),
                key: d.marker_key.clone(),
            }
        })?;
        let close_idx = after_open + close_rel;
        let inner = &out[after_open..close_idx];

        let replacement = render_wrapper(d, inner);

        let end_idx = close_idx + close.len();
        let mut rebuilt = String::with_capacity(out.len() + replacement.len());
        rebuilt.push_str(&out[..open_idx]);
        rebuilt.push_str(&replacement);
        rebuilt.push_str(&out[end_idx..]);
        out = rebuilt;
    }
    Ok(out)
}

/// Build the `<div data-zfb-island="…" …>…</div>` wrapper for a single
/// island. Internal helper; tests cover the attribute-escaping rules.
fn render_wrapper(d: &IslandDescriptor, inner: &str) -> String {
    let mut s = String::with_capacity(inner.len() + 96);
    s.push_str("<div data-zfb-island=\"");
    s.push_str(&escape_attr(&d.component_name));
    s.push_str("\" data-props=\"");
    s.push_str(&escape_attr(&d.props_json));
    s.push('"');
    if let Some(when) = &d.when {
        s.push_str(" data-when=\"");
        s.push_str(&escape_attr(when));
        s.push('"');
    }
    s.push('>');
    s.push_str(inner);
    s.push_str("</div>");
    s
}

/// HTML-escape a value for use inside an attribute value. We escape the
/// five characters that can break out of an attribute or change its
/// meaning (`&`, `<`, `>`, `"`, `'`) and leave everything else verbatim.
/// Single-quote escaping is defence-in-depth: today the rewriter only
/// emits double-quoted attributes, but consumers / downstream rewriters
/// may not, and `&#39;` is cheap.
fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Errors the attribute-skeleton bridge ([`rewrite_islands_in_attr_skeleton`])
/// surfaces when the rendered HTML and the descriptors do not match up.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IslandSkeletonRewriteError {
    /// The number of empty `data-zfb-island=""` skeletons in the rendered
    /// HTML does not equal the number of descriptors. Indicates an
    /// orchestration bug in the renderer — or, more rarely, user content
    /// that contains the literal string ` data-zfb-island=""` (substring
    /// matching is fragile here; tracked for replacement under
    /// https://github.com/Takazudo/zfb2/issues/65).
    #[error(
        "island skeleton/descriptor count mismatch: rendered HTML contains {skeletons} \
         empty data-zfb-island=\"\" skeletons but the renderer produced {descriptors} \
         IslandDescriptor(s). \
         Causes: (1) bug in the renderer's descriptor-collection step, OR \
         (2) user content (e.g. Markdown body text, code fences) that contains the \
         literal string ` data-zfb-island=\"\"` and is being miscounted as a skeleton. \
         The substring-based matching is fragile and is being replaced with \
         anchor-based locators — see issue #65."
    )]
    CountMismatch {
        /// Number of `data-zfb-island=""` skeletons found in the input HTML.
        skeletons: usize,
        /// Number of descriptors passed in.
        descriptors: usize,
    },
}

/// Bridge rewriter for HTML emitted by Sub 4's `<Island when="…">` JSX
/// wrapper, which produces `<div data-zfb-island="" data-when="…">…</div>`
/// skeletons at server-render time.
///
/// Pairs each empty-`data-zfb-island` skeleton with one descriptor in
/// document order: the Nth skeleton receives the Nth descriptor. The
/// renderer is responsible for ordering descriptors to match the order in
/// which `<Island>` instances are encountered when walking the rendered
/// page (depth-first, left-to-right — the natural JSX traversal order).
///
/// Each skeleton's `data-zfb-island=""` is replaced with
/// `data-zfb-island="ComponentName"`, and `data-props="…json…"` is
/// inserted alongside. The skeleton's existing `data-when` attribute, set
/// by the wrapper, is left untouched. If the descriptor's `when` field is
/// `Some(_)`, it is **not** used here — Sub 4's wrapper is the source of
/// truth for `data-when` whenever a skeleton is present.
///
/// This function is the "bridge B" agreed by topic-hydration-emit (Sub 3)
/// and topic-island-wrapper (Sub 4): the marker-comment path in
/// [`rewrite_islands`] stays untouched for renderer code paths that emit
/// sentinel comments around server output, and this skeleton path handles
/// the JSX wrapper case directly.
///
/// # Errors
///
/// - [`IslandSkeletonRewriteError::CountMismatch`] if the skeleton count
///   in the input HTML does not equal the descriptor count.
pub fn rewrite_islands_in_attr_skeleton(
    html: &str,
    islands: &[IslandDescriptor],
) -> Result<String, IslandSkeletonRewriteError> {
    // Find every empty data-zfb-island="" attribute occurrence. Match on
    // the literal attribute pair (with a leading space so we don't match
    // a non-data-zfb-island prefix).
    //
    // Substring-based matching is fragile — if user content (e.g. a
    // Markdown body) contains the literal string ` data-zfb-island=""`
    // it will be counted as a skeleton, and we either error out
    // (count mismatch, this branch) or mis-pair descriptors. The
    // anchor-based replacement is tracked under
    // https://github.com/Takazudo/zfb2/issues/65.
    const NEEDLE: &str = " data-zfb-island=\"\"";

    let mut positions: Vec<usize> = Vec::new();
    {
        let mut search_from = 0;
        while let Some(rel) = html[search_from..].find(NEEDLE) {
            let abs = search_from + rel;
            positions.push(abs);
            search_from = abs + NEEDLE.len();
        }
    }

    if positions.len() != islands.len() {
        return Err(IslandSkeletonRewriteError::CountMismatch {
            skeletons: positions.len(),
            descriptors: islands.len(),
        });
    }

    // Walk in reverse order so earlier positions remain valid indices into
    // the buffer as we splice in longer replacement strings.
    let mut out = String::from(html);
    for (i, &pos) in positions.iter().enumerate().rev() {
        let d = &islands[i];
        let mut replacement = String::with_capacity(NEEDLE.len() + d.props_json.len() + 64);
        replacement.push_str(" data-zfb-island=\"");
        replacement.push_str(&escape_attr(&d.component_name));
        replacement.push_str("\" data-props=\"");
        replacement.push_str(&escape_attr(&d.props_json));
        replacement.push('"');

        let end = pos + NEEDLE.len();
        let mut rebuilt = String::with_capacity(out.len() + replacement.len() - NEEDLE.len());
        rebuilt.push_str(&out[..pos]);
        rebuilt.push_str(&replacement);
        rebuilt.push_str(&out[end..]);
        out = rebuilt;
    }
    Ok(out)
}

/// Build a single-attribute `<script type="module" src="…"></script>` tag
/// for the per-island hydration runtime bundle.
///
/// Used together with [`inject_runtime_script_into_head`]: the page
/// router's HTML pass calls this to materialise the tag for the
/// runtime URL emitted by
/// [`crate::EsbuildSubprocessBundler::bundle_per_island`], then asks
/// the helper to splice it into the rendered page's `<head>`.
///
/// The `src` is HTML-attribute-escaped.
pub fn islands_runtime_script_tag(runtime_url: &str) -> String {
    format!(
        "<script type=\"module\" src=\"{src}\"></script>",
        src = escape_attr(runtime_url),
    )
}

/// Inject the islands-runtime `<script type="module">` tag into
/// `html`'s `<head>`.
///
/// Returns `Ok(html_with_script_injected)` on success. The tag is
/// inserted **immediately before** the closing `</head>` so it ships
/// with the page's other module scripts. If `</head>` cannot be
/// located (e.g. a fragment renderer that emits headerless HTML), the
/// tag is prepended to the input and the resulting markup is still
/// valid as a fragment — but the caller should treat that path as a
/// renderer bug and probably propagate it.
///
/// `runtime_url` is the public URL of the islands runtime bundle, e.g.
/// `/islands/islands-runtime-abc12345.js`. Multiple calls are
/// idempotent in the trivial sense — calling twice will inject twice;
/// the page router is expected to only call this once per render.
pub fn inject_runtime_script_into_head(html: &str, runtime_url: &str) -> String {
    let tag = islands_runtime_script_tag(runtime_url);
    if let Some(close_idx) = html.find("</head>") {
        let mut out = String::with_capacity(html.len() + tag.len());
        out.push_str(&html[..close_idx]);
        out.push_str(&tag);
        out.push_str(&html[close_idx..]);
        return out;
    }
    // Fragment / headerless markup: prepend.
    let mut out = String::with_capacity(html.len() + tag.len());
    out.push_str(&tag);
    out.push_str(html);
    out
}

/// Build the `<script type="module" …>` tag the renderer drops into the
/// page's `<head>` (or end-of-`<body>`) so the hydration runtime can find
/// the islands bundle.
///
/// Two attributes are set:
/// - `src` — the runtime script URL (the small ~3 KB hydration runtime).
/// - `data-zfb-bundle` — the islands bundle URL (`dist/assets/islands-{hash}.js`),
///   which the runtime reads off its own `<script>` element via
///   `document.currentScript` (or by querying the attribute).
///
/// Both URLs are HTML-attribute-escaped.
pub fn hydration_script_tag(runtime_url: &str, bundle_url: &str) -> String {
    format!(
        "<script type=\"module\" src=\"{runtime}\" data-zfb-bundle=\"{bundle}\"></script>",
        runtime = escape_attr(runtime_url),
        bundle = escape_attr(bundle_url),
    )
}

/// Build a marker-pair the renderer can emit around a server-rendered
/// island's HTML. Exposed so renderer code can construct the same shape
/// the rewriter consumes without re-deriving it.
pub fn island_marker_pair(key: &str) -> (String, String) {
    (
        format!("<!--zfb-island:{key}-->"),
        format!("<!--/zfb-island:{key}-->"),
    )
}

/// Wrap `inner` in the marker pair for `key`. Test helper / convenience.
#[doc(hidden)]
pub fn wrap_with_markers(key: &str, inner: &str) -> String {
    let (open, close) = island_marker_pair(key);
    let mut s = String::with_capacity(open.len() + inner.len() + close.len());
    s.push_str(&open);
    s.push_str(inner);
    s.push_str(&close);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_with(inner: &str) -> String {
        format!("<html><body><h1>Page</h1>{inner}<footer>fin</footer></body></html>")
    }

    #[test]
    fn rewrites_single_island_into_data_attribute_wrapper() {
        let inner = wrap_with_markers("counter#0", "<button>3</button>");
        let html = page_with(&inner);
        let d = IslandDescriptor::new("Counter", r#"{"start":3}"#, "counter#0");
        let out = rewrite_islands(&html, &[d]).unwrap();
        assert!(
            out.contains(
                r#"<div data-zfb-island="Counter" data-props="{&quot;start&quot;:3}"><button>3</button></div>"#
            ),
            "actual: {out}"
        );
        // Markers are gone from the output.
        assert!(!out.contains("<!--zfb-island:"));
        assert!(!out.contains("<!--/zfb-island:"));
    }

    #[test]
    fn emits_data_when_attribute_only_when_present() {
        let inner = wrap_with_markers("k", "<x/>");
        let html = page_with(&inner);
        let d = IslandDescriptor::new("Foo", "{}", "k").with_when("visible");
        let out = rewrite_islands(&html, &[d]).unwrap();
        assert!(out.contains(r#"data-when="visible""#));

        let inner = wrap_with_markers("k", "<x/>");
        let html = page_with(&inner);
        let d = IslandDescriptor::new("Foo", "{}", "k");
        let out = rewrite_islands(&html, &[d]).unwrap();
        assert!(!out.contains("data-when="));
    }

    #[test]
    fn handles_multiple_islands_in_order() {
        let mut html = String::from("<html><body>");
        html.push_str(&wrap_with_markers("a", "<i>1</i>"));
        html.push_str("<hr>");
        html.push_str(&wrap_with_markers("b", "<i>2</i>"));
        html.push_str("</body></html>");

        let descriptors = vec![
            IslandDescriptor::new("A", "{}", "a"),
            IslandDescriptor::new("B", r#"{"x":1}"#, "b"),
        ];
        let out = rewrite_islands(&html, &descriptors).unwrap();
        let pos_a = out.find(r#"data-zfb-island="A""#).unwrap();
        let pos_b = out.find(r#"data-zfb-island="B""#).unwrap();
        assert!(pos_a < pos_b);
        assert!(out.contains("<i>1</i>"));
        assert!(out.contains("<i>2</i>"));
    }

    #[test]
    fn errors_when_open_marker_is_missing() {
        let html = page_with("<span>plain</span>");
        let d = IslandDescriptor::new("Foo", "{}", "missing");
        let err = rewrite_islands(&html, &[d]).unwrap_err();
        assert!(matches!(err, IslandRewriteError::OpenMarkerMissing { .. }));
    }

    #[test]
    fn errors_when_close_marker_is_missing() {
        let html = "<html><body><!--zfb-island:k--><span>x</span></body></html>";
        let d = IslandDescriptor::new("Foo", "{}", "k");
        let err = rewrite_islands(html, &[d]).unwrap_err();
        assert!(matches!(err, IslandRewriteError::CloseMarkerMissing { .. }));
    }

    #[test]
    fn errors_on_duplicate_marker_keys() {
        let html = page_with(&wrap_with_markers("k", "<x/>"));
        let descriptors = vec![
            IslandDescriptor::new("A", "{}", "k"),
            IslandDescriptor::new("B", "{}", "k"),
        ];
        let err = rewrite_islands(&html, &descriptors).unwrap_err();
        assert!(matches!(err, IslandRewriteError::DuplicateKey { .. }));
    }

    #[test]
    fn escapes_attributes_correctly() {
        // Component name and props with `<`, `&`, `"`, `>` all need
        // attribute escaping. Using realistic JSON props makes the test
        // closer to production.
        let inner = wrap_with_markers("k", "<i>x</i>");
        let html = page_with(&inner);
        let props = r#"{"text":"<a & \"b\">"}"#;
        let d = IslandDescriptor::new("My&Comp", props, "k");
        let out = rewrite_islands(&html, &[d]).unwrap();
        assert!(out.contains(r#"data-zfb-island="My&amp;Comp""#));
        assert!(out.contains("&quot;"));
        assert!(out.contains("&lt;"));
        assert!(out.contains("&gt;"));
        // The literal raw `"` from props_json must NOT appear inside the
        // attribute (it would break out of the attribute).
        let attr_start = out.find("data-props=\"").unwrap() + "data-props=\"".len();
        let attr_end = out[attr_start..].find('"').unwrap() + attr_start;
        let attr_value = &out[attr_start..attr_end];
        assert!(!attr_value.contains('"'));
    }

    #[test]
    fn hydration_script_tag_escapes_urls() {
        let tag = hydration_script_tag(
            "/runtime?v=1&t=2",
            "/dist/assets/islands-deadbeef.js?\"oops",
        );
        assert!(tag.starts_with("<script type=\"module\""));
        assert!(tag.contains(r#"src="/runtime?v=1&amp;t=2""#));
        // Bundle URL `"` must be escaped.
        assert!(tag.contains("&quot;oops"));
        assert!(tag.contains("data-zfb-bundle="));
    }

    #[test]
    fn islands_runtime_script_tag_minimal_shape() {
        let tag = islands_runtime_script_tag("/islands/islands-runtime-abc12345.js");
        assert_eq!(
            tag,
            "<script type=\"module\" src=\"/islands/islands-runtime-abc12345.js\"></script>"
        );
    }

    #[test]
    fn islands_runtime_script_tag_escapes_url() {
        let tag = islands_runtime_script_tag("/runtime?v=1&t=2");
        assert!(tag.contains(r#"src="/runtime?v=1&amp;t=2""#));
    }

    #[test]
    fn inject_runtime_script_inserts_before_close_head() {
        let html =
            "<!doctype html><html><head><title>X</title></head><body><p>hi</p></body></html>";
        let out = inject_runtime_script_into_head(html, "/islands/islands-runtime-abc.js");
        let head_close = out.find("</head>").unwrap();
        let script_at = out
            .find("<script type=\"module\" src=\"/islands/islands-runtime-abc.js\">")
            .expect("script tag injected");
        assert!(script_at < head_close, "script must appear before </head>");
        // Original head contents preserved.
        assert!(out.contains("<title>X</title>"));
        assert!(out.contains("<p>hi</p>"));
    }

    #[test]
    fn inject_runtime_script_falls_back_to_prepend_when_head_missing() {
        let html = "<p>fragment</p>";
        let out = inject_runtime_script_into_head(html, "/r.js");
        assert!(out.starts_with("<script type=\"module\" src=\"/r.js\">"));
        assert!(out.ends_with("<p>fragment</p>"));
    }

    #[test]
    fn island_marker_pair_shape_is_stable() {
        let (o, c) = island_marker_pair("counter#3");
        assert_eq!(o, "<!--zfb-island:counter#3-->");
        assert_eq!(c, "<!--/zfb-island:counter#3-->");
    }

    #[test]
    fn no_descriptors_is_a_no_op() {
        let html = page_with("<span>plain</span>");
        let out = rewrite_islands(&html, &[]).unwrap();
        assert_eq!(out, html);
    }

    // ---- Bridge: rewrite_islands_in_attr_skeleton (Sub 4 wrapper output) ----

    #[test]
    fn skeleton_bridge_fills_empty_data_zfb_island() {
        // Single skeleton emitted by Sub 4's <Island when="visible">.
        let html = r#"<html><body><div data-zfb-island="" data-when="visible"><button>3</button></div></body></html>"#;
        let d = IslandDescriptor::new("Counter", r#"{"start":3}"#, "k");
        let out = rewrite_islands_in_attr_skeleton(html, &[d]).unwrap();
        assert!(out.contains(r#"data-zfb-island="Counter""#));
        assert!(out.contains(r#"data-props="{&quot;start&quot;:3}""#));
        // data-when from the wrapper is preserved.
        assert!(out.contains(r#"data-when="visible""#));
        // Inner content is left alone.
        assert!(out.contains("<button>3</button>"));
    }

    #[test]
    fn skeleton_bridge_pairs_in_document_order() {
        // Two skeletons; descriptors must match positional order.
        let html = r#"<html><body><div data-zfb-island="" data-when="visible"><i>a</i></div><span>between</span><div data-zfb-island="" data-when="idle"><i>b</i></div></body></html>"#;
        let descriptors = vec![
            IslandDescriptor::new("A", "{}", "k1"),
            IslandDescriptor::new("B", "{}", "k2"),
        ];
        let out = rewrite_islands_in_attr_skeleton(html, &descriptors).unwrap();
        let pos_a = out.find(r#"data-zfb-island="A""#).unwrap();
        let pos_b = out.find(r#"data-zfb-island="B""#).unwrap();
        assert!(pos_a < pos_b);
        // Both data-when values from the wrapper preserved.
        assert!(out.contains(r#"data-when="visible""#));
        assert!(out.contains(r#"data-when="idle""#));
    }

    #[test]
    fn skeleton_bridge_count_mismatch_is_an_error() {
        let html = r#"<div data-zfb-island="" data-when="load"><i>x</i></div>"#;
        let descriptors = vec![
            IslandDescriptor::new("A", "{}", "k1"),
            IslandDescriptor::new("B", "{}", "k2"),
        ];
        let err = rewrite_islands_in_attr_skeleton(html, &descriptors).unwrap_err();
        assert_eq!(
            err,
            IslandSkeletonRewriteError::CountMismatch {
                skeletons: 1,
                descriptors: 2,
            }
        );
    }

    #[test]
    fn skeleton_bridge_no_skeletons_no_descriptors_is_ok() {
        let html = "<html><body><span>plain</span></body></html>";
        let out = rewrite_islands_in_attr_skeleton(html, &[]).unwrap();
        assert_eq!(out, html);
    }
}
