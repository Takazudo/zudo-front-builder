//! Canonical render-region sentinel marker contract (epic #2421 / #2432).
//!
//! ## Why this lives in `zfb-types`
//!
//! The sentinel `<template data-zfb-render-region>` pair is produced by
//! THREE independent producers — `zfb-build`'s `render_md_page_shell` JSX
//! generator, `@takazudo/zfb`'s `content.ts` SSR bridge (TypeScript, so it
//! cannot import Rust constants at all), and (indirectly, as the byte
//! contract every producer must match) `zfb`'s `render_artifact.rs`
//! `parse_marker` matcher/stripper — and consumed by the render-artifact
//! export pass. A byte drift in any one producer either strips a live
//! `<template>` element into shipped `dist/` HTML (parser rejects a marker
//! it should have recognised) or corrupts the extracted artifact (parser
//! recognises something it should not). `zfb-types` is the designated
//! zero-zfb-dep leaf crate both `zfb` and `zfb-build` already depend on
//! (see `asset_urls.rs`, `page_extensions.rs` for the same pattern), so the
//! contract lives here once instead of drifting between two crates.
//!
//! ## Canonical spelling
//!
//! ```text
//! <template data-zfb-render-region="start" data-zfb-region-id="ID"></template>
//! ```
//!
//! Double quotes only; `data-zfb-render-region` first; exactly one space
//! before `data-zfb-region-id`; no other attributes; no content; paired
//! closing tag (a self-closing spelling is rejected by the matcher). The
//! id is taken verbatim — HTML-escaped entities (`&` -> `&amp;`) are
//! accepted by design: a renderer escapes attribute values before this
//! pass ever runs, and the matcher does not attempt to un-escape the id
//! back to the original specifier. See
//! `crates/zfb-types/tests/fixtures/render-region-marker-parity.json` for
//! the pinned plain-id and escaped-id cases, and
//! `crates/zfb/src/commands/render_artifact.rs`'s `parse_marker` doc
//! comment for the full rationale.
//!
//! ## What is NOT covered here
//!
//! `render_md_page_shell`'s JSX-form sentinel (self-closing, with an
//! expression-valued id: `data-zfb-region-id={__zfbRegionId}`) cannot
//! reuse [`render_region_marker`] — the canonical string is a paired,
//! literal-id HTML fragment, not JSX source. That producer instead builds
//! its two sentinel lines from [`RENDER_REGION_ATTR`] /
//! [`REGION_ID_ATTR`] directly via `format!`, so at least the attribute
//! names cannot drift; its JSX->HTML rendering is covered only
//! transitively, by the same renderers the TS suites exercise plus its
//! own byte-pinned source tests (see the epic's "Coverage honesty" note).

/// Attribute marking a sentinel's edge (`"start"` or `"end"`). Reserved
/// zfb attribute namespace — see the module doc for the full contract.
pub const RENDER_REGION_ATTR: &str = "data-zfb-render-region";

/// Attribute carrying the region id (verbatim, not un-escaped).
pub const REGION_ID_ATTR: &str = "data-zfb-region-id";

/// Which edge of a render region a sentinel marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderRegionEdge {
    Start,
    End,
}

// ---------------------------------------------------------------------------
// Matcher byte-parts
// ---------------------------------------------------------------------------
//
// Mirrors `crates/zfb/src/commands/render_artifact.rs`'s `parse_marker`
// byte-level scan. Kept as literal byte-string constants (not built from
// `RENDER_REGION_ATTR`/`REGION_ID_ATTR` via `format!`, which is not
// available in `const` position) so `parse_marker` can `strip_prefix`
// against them directly with no runtime allocation — the same reason the
// pre-#2435 local constants in `render_artifact.rs` were literals. Kept in
// sync with the two attribute-name constants above by hand; the fixture
// parity test below is what actually proves they agree byte-for-byte with
// [`render_region_marker`]'s output.

/// Everything up to and including the render-region attribute's opening
/// quote.
pub const MARKER_HEAD: &[u8] = b"<template data-zfb-render-region=\"";
/// The `start` edge value plus its closing quote.
pub const MARKER_KIND_START: &[u8] = b"start\"";
/// The `end` edge value plus its closing quote.
pub const MARKER_KIND_END: &[u8] = b"end\"";
/// The space-separated region-id attribute name plus its opening quote.
pub const MARKER_ID_ATTR: &[u8] = b" data-zfb-region-id=\"";
/// Everything from the region id's closing quote to the end of the
/// element — an EMPTY `<template>`, which is what makes the marker inert
/// in the DOM and its removal byte-exact.
pub const MARKER_TAIL: &[u8] = b"\"></template>";

/// Canonical serializer for a render-region sentinel: assembles the exact
/// bytes every producer must match, byte-for-byte, out of the 5 matcher
/// byte-parts above — so this function and `parse_marker` can never
/// independently drift, only agree or disagree with the shared fixture.
/// `id` is inserted verbatim: callers that need HTML-attribute escaping
/// (an id containing `&` or `"`) must escape it themselves before
/// calling, matching what `parse_marker` expects to read back.
pub fn render_region_marker(edge: RenderRegionEdge, id: &str) -> String {
    let kind = match edge {
        RenderRegionEdge::Start => MARKER_KIND_START,
        RenderRegionEdge::End => MARKER_KIND_END,
    };
    let mut bytes = Vec::with_capacity(
        MARKER_HEAD.len() + kind.len() + MARKER_ID_ATTR.len() + id.len() + MARKER_TAIL.len(),
    );
    bytes.extend_from_slice(MARKER_HEAD);
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(MARKER_ID_ATTR);
    bytes.extend_from_slice(id.as_bytes());
    bytes.extend_from_slice(MARKER_TAIL);
    String::from_utf8(bytes)
        .expect("marker scaffolding is ASCII and `id` was passed in as a UTF-8 &str")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_region_marker_matches_the_pinned_shape() {
        assert_eq!(
            render_region_marker(RenderRegionEdge::Start, "mdx://blog/hello"),
            "<template data-zfb-render-region=\"start\" data-zfb-region-id=\"mdx://blog/hello\"></template>"
        );
        assert_eq!(
            render_region_marker(RenderRegionEdge::End, "mdx://blog/hello"),
            "<template data-zfb-render-region=\"end\" data-zfb-region-id=\"mdx://blog/hello\"></template>"
        );
    }
}
