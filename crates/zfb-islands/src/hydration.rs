//! Hydration HTML emit.
//!
//! Given a page's server-rendered HTML (as an [`HtmlTree`] parse handle)
//! and a set of *islands* that were actually used by that page, rewrite
//! each island's outer markup so it carries the metadata the client-side
//! hydration runtime walks.
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
//! ### Marker-based ([`rewrite_islands`])
//!
//! The renderer wraps each island's server output with a sentinel pair:
//! `<!--zfb-island:KEY-->…<!--/zfb-island:KEY-->`. The rewriter locates
//! these via document-level comment handlers in `lol_html`, which fire
//! only on tokens the HTML5 tokenizer parses as actual comments — so a
//! literal marker byte sequence sitting inside a `<script>`/`<style>`
//! raw-text body (or a `<textarea>`/`<title>` RCDATA body) is ignored.
//! The handler *replaces* each matched comment with a unique random
//! sentinel string before the inner-HTML splice, so the splice can only
//! target the structurally-confirmed comment and never a raw-text
//! look-alike.
//!
//! ### Attribute-skeleton bridge ([`rewrite_islands_in_attr_skeleton`])
//!
//! Sub 4's `<Island>` JSX wrapper emits
//! `<div data-zfb-island="" data-when="…">…</div>` skeletons.  The
//! previous implementation scanned for the literal string
//! ` data-zfb-island=""` which was fragile: user content in a Markdown
//! body or a code fence containing the same byte sequence would be
//! miscounted.  The anchor-based replacement uses `lol_html`'s CSS
//! selector `div[data-zfb-island=""]`, which only matches actual
//! `<div>` *elements* carrying an empty `data-zfb-island` attribute —
//! text content is never involved.
//!
//! ### Head injection ([`inject_runtime_script_into_head`])
//!
//! Uses the CSS selector `head` (via `lol_html`) to locate the closing
//! `</head>` tag and insert the `<script>` tag before it. This is
//! immune to a literal `</head>` appearing inside a `<pre>` or `<code>`
//! block that fooled the previous string-find approach.
//!
//! ## Follow-up: marker emission in the renderer
//!
//! The renderer in `zfb-render` does not yet emit these markers. That
//! work belongs with the `<Island>` wrapper (Sub 4) and is tracked
//! there. Until the renderer emits markers, the rewriter is exercised
//! only by tests in this crate.

use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use lol_html::html_content::{ContentType, Element};
use lol_html::{doc_comments, ElementContentHandlers, RewriteStrSettings, Selector};
use serde::Serialize;
use serde_json::Value as JsonValue;
use thiserror::Error;

use crate::html_tree::HtmlTree;
use zfb_types::escape_html;

// ---------------------------------------------------------------------------
// WhenHint
// ---------------------------------------------------------------------------

/// Typed hydration-timing hint for the `data-when` attribute.
///
/// Each variant maps to the string value the client-side hydration runtime
/// recognises:
///
/// | Variant | `data-when` value |
/// |---------|-------------------|
/// | `Visible` | `"visible"` |
/// | `Idle` | `"idle"` |
/// | `Load` | `"load"` |
/// | `Media(query)` | `"media"` (+ separate `data-media` attribute) |
///
/// Using an enum instead of a bare `String` means the rewriter and its
/// callers cannot accidentally pass an unrecognised hint — unknown values
/// fail at construction time (or static analysis) rather than silently
/// round-tripping into the HTML.
///
/// The `Media(query)` variant emits `data-when="media"` plus a separate
/// `data-media="<query>"` attribute (NOT the packed `data-when="media(<q>)"`
/// shape). The client runtime reads `data-media` via `getAttribute("data-media")`
/// and passes it to `window.matchMedia`. The two-attribute shape was chosen
/// so each attribute is orthogonal and attribute escaping is handled
/// uniformly by `zfb_types::escape_html`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WhenHint {
    /// Hydrate when the island's root element enters the viewport
    /// (`IntersectionObserver`).
    Visible,
    /// Hydrate during the browser's next idle period
    /// (`requestIdleCallback`).
    Idle,
    /// Hydrate immediately on page load (default when no `data-when` is
    /// emitted). Providing this variant explicitly allows callers to be
    /// unambiguous about intent without relying on the absent-attribute
    /// default.
    Load,
    /// Hydrate when the given CSS media query first matches (`matchMedia`).
    /// The query string is emitted as a separate `data-media` attribute;
    /// `data-when` is set to `"media"`.
    Media(String),
}

impl WhenHint {
    /// Return the `data-when` attribute string this hint maps to.
    ///
    /// ```
    /// use zfb_islands::hydration::WhenHint;
    /// assert_eq!(WhenHint::Visible.as_str(), "visible");
    /// assert_eq!(WhenHint::Idle.as_str(), "idle");
    /// assert_eq!(WhenHint::Media("(max-width: 768px)".to_string()).as_str(), "media");
    /// ```
    pub fn as_str(&self) -> std::borrow::Cow<'_, str> {
        match self {
            WhenHint::Visible => std::borrow::Cow::Borrowed("visible"),
            WhenHint::Idle => std::borrow::Cow::Borrowed("idle"),
            WhenHint::Load => std::borrow::Cow::Borrowed("load"),
            WhenHint::Media(_) => std::borrow::Cow::Borrowed("media"),
        }
    }
}

// ---------------------------------------------------------------------------
// IslandDescriptor
// ---------------------------------------------------------------------------

/// One island actually used by a rendered page.
///
/// Field semantics:
/// - [`component_name`](Self::component_name): stable identifier the runtime
///   uses to pick the right component out of the islands bundle. Must match
///   the export name produced by Sub 1's scanner.
/// - [`props`](Self::props): the props passed to the component at
///   server-render time as a `serde_json::Value`. The rewriter serialises
///   this value on demand; there is no pre-serialisation round-trip.
/// - [`marker_key`](Self::marker_key): the unique key the renderer used in
///   the surrounding `<!--zfb-island:KEY-->` / `<!--/zfb-island:KEY-->`
///   sentinel pair. Each key is expected to appear at most once per page.
/// - [`when`](Self::when): optional typed hydration hint ([`WhenHint`]).
///   `None` means no `data-when` attribute is emitted; the runtime treats
///   the absent case as `"load"` (immediate hydration).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IslandDescriptor {
    /// Component export name as produced by the islands AST scanner.
    pub component_name: String,
    /// Props for the component as a structured JSON value. The rewriter
    /// serialises this to a JSON string when emitting `data-props`.
    pub props: JsonValue,
    /// The unique key used in the renderer's `<!--zfb-island:KEY-->` /
    /// `<!--/zfb-island:KEY-->` sentinel pair around this island's server
    /// output.
    pub marker_key: String,
    /// Optional typed `data-when` hint. `None` ⇒ omit the attribute.
    pub when: Option<WhenHint>,
}

impl IslandDescriptor {
    /// Build a descriptor with no `data-when` hint (immediate hydration).
    pub fn new(
        component_name: impl Into<String>,
        props: impl Into<JsonValue>,
        marker_key: impl Into<String>,
    ) -> Self {
        Self {
            component_name: component_name.into(),
            props: props.into(),
            marker_key: marker_key.into(),
            when: None,
        }
    }

    /// Set a typed `data-when` hint.
    pub fn with_when(mut self, when: WhenHint) -> Self {
        self.when = Some(when);
        self
    }
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

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

/// Errors the attribute-skeleton bridge ([`rewrite_islands_in_attr_skeleton`])
/// surfaces when the rendered HTML and the descriptors do not match up.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IslandSkeletonRewriteError {
    /// The number of `<div data-zfb-island="">` skeleton elements located by
    /// the HTML parser does not equal the number of descriptors. Indicates an
    /// orchestration bug in the renderer.
    ///
    /// Unlike the previous substring-scan implementation this error is NOT
    /// triggered by user content (e.g. Markdown body text or code fences)
    /// that happens to contain the literal string `data-zfb-island=""` —
    /// the anchor-based locator uses `lol_html`'s CSS selector
    /// `div[data-zfb-island=""]` which only matches actual DOM elements.
    #[error(
        "island skeleton/descriptor count mismatch: rendered HTML contains {skeletons} \
         empty data-zfb-island=\"\" skeletons but the renderer produced {descriptors} \
         IslandDescriptor(s). \
         Causes: bug in the renderer's descriptor-collection step."
    )]
    CountMismatch {
        /// Number of `<div data-zfb-island="">` elements found via the parser.
        skeletons: usize,
        /// Number of descriptors passed in.
        descriptors: usize,
    },
}

// ---------------------------------------------------------------------------
// Public API — anchor-based signatures
// ---------------------------------------------------------------------------

/// Rewrite each island's marker-bracketed server output into the
/// `<div data-zfb-island="…" data-props="…">…</div>` wrapper.
///
/// The `tree` handle is mutated in place. The caller should call
/// [`HtmlTree::parse`] once before the render pipeline and pass the same
/// handle to all subsequent helpers.
///
/// **Dormant emitter note:** This is not the live SSR path. The live SSR island
/// emitter is `packages/zfb/src/island.ts`.
///
/// ## Implementation note — single-pass strategy
///
/// Island markers are HTML comments (`<!--zfb-island:KEY-->`). The old
/// implementation did one lol_html pass **per island** followed by one full
/// string rebuild per island — O(N) HTML re-parses for N islands.
///
/// The new implementation does ONE lol_html Phase-1 pass that replaces ALL
/// marker comments in a single streaming scan, then N sequential in-place
/// string splices (Phase 2) with no further HTML parsing.  For a page with
/// ten islands this is a 10× reduction in lol_html parse work.
///
/// `lol_html`'s comment handler fires only on tokens it parsed as real HTML
/// comments, so a literal marker byte sequence inside a `<script>`/`<style>`
/// raw-text body is left untouched, and the sentinel strings those handlers
/// emit are hex-only (`[A-Za-z0-9_]`) so they can never be mistaken for the
/// original markers by Phase 2's byte-find.
///
/// # Errors
///
/// - [`IslandRewriteError::DuplicateKey`] if two descriptors share a
///   `marker_key`.
/// - [`IslandRewriteError::OpenMarkerMissing`] /
///   [`IslandRewriteError::CloseMarkerMissing`] if the renderer's sentinel
///   pair for a descriptor is not present in the input HTML.
pub fn rewrite_islands(
    tree: &mut HtmlTree,
    islands: &[IslandDescriptor],
) -> Result<(), IslandRewriteError> {
    if islands.is_empty() {
        return Ok(());
    }

    // Reject duplicate keys up front.
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

    // Build per-island sentinel pairs before the lol_html pass.  All
    // sentinels are derived from the current html content, so they are all
    // collision-free with respect to the original HTML and with each other
    // (make_sentinels already checks for collisions; since the sentinels are
    // only ever inserted by lol_html's comment handler — not by each other —
    // cross-sentinel collisions cannot occur in practice for the hex-only
    // alphabet).
    let sentinels: Vec<(String, String)> = islands
        .iter()
        .map(|d| make_sentinels(&tree.html, &d.marker_key))
        .collect();

    // Phase 1: ONE lol_html pass that replaces ALL marker comments with their
    // sentinel strings.  Seen-flags are collected per island.
    type SeenPair = (Rc<Cell<bool>>, Rc<Cell<bool>>);
    let seen_flags: Vec<SeenPair> = islands
        .iter()
        .map(|_| (Rc::new(Cell::new(false)), Rc::new(Cell::new(false))))
        .collect();

    {
        // Build the per-island comment-text → (sentinel, seen_flag) map that
        // the single comment handler walks.  We use a Vec of tuples because N
        // is always small (typically < 50 islands per page).
        let mut handlers: Vec<(String, String, Rc<Cell<bool>>)> =
            Vec::with_capacity(islands.len() * 2);
        for (i, d) in islands.iter().enumerate() {
            let (open_s, close_s) = &sentinels[i];
            let (open_seen, close_seen) = &seen_flags[i];
            handlers.push((
                format!("zfb-island:{}", d.marker_key),
                open_s.clone(),
                Rc::clone(open_seen),
            ));
            handlers.push((
                format!("/zfb-island:{}", d.marker_key),
                close_s.clone(),
                Rc::clone(close_seen),
            ));
        }

        let settings: RewriteStrSettings<'_, '_> = RewriteStrSettings {
            document_content_handlers: vec![doc_comments!(move |c| {
                let text = c.text();
                for (comment_text, sentinel, seen) in &handlers {
                    if text == comment_text.as_str() {
                        seen.set(true);
                        c.replace(sentinel, ContentType::Text);
                        break;
                    }
                }
                Ok(())
            })],
            ..RewriteStrSettings::new()
        };

        tree.rewrite(settings)
            .expect("lol_html rewriting for marker replacement should not fail");
    }

    // Validate that all markers fired.
    for (i, d) in islands.iter().enumerate() {
        let (open_seen, close_seen) = &seen_flags[i];
        if !open_seen.get() {
            return Err(IslandRewriteError::OpenMarkerMissing {
                component: d.component_name.clone(),
                key: d.marker_key.clone(),
            });
        }
        if !close_seen.get() {
            return Err(IslandRewriteError::CloseMarkerMissing {
                component: d.component_name.clone(),
                key: d.marker_key.clone(),
            });
        }
    }

    // Phase 2: N sequential string splices — no further HTML parsing.
    // Each iteration byte-finds its own open/close sentinel (guaranteed
    // unique in the post-Phase-1 HTML) and splices in the wrapper.
    for (i, d) in islands.iter().enumerate() {
        let (open_sentinel, close_sentinel) = &sentinels[i];
        let html = &tree.html;
        let open_idx = html.find(open_sentinel.as_str()).ok_or_else(|| {
            IslandRewriteError::OpenMarkerMissing {
                component: d.component_name.clone(),
                key: d.marker_key.clone(),
            }
        })?;
        let after_open = open_idx + open_sentinel.len();
        let close_rel = html[after_open..]
            .find(close_sentinel.as_str())
            .ok_or_else(|| IslandRewriteError::CloseMarkerMissing {
                component: d.component_name.clone(),
                key: d.marker_key.clone(),
            })?;
        let close_idx = after_open + close_rel;
        let inner_html = &html[after_open..close_idx];
        let replacement = render_wrapper(d, inner_html);
        let end_idx = close_idx + close_sentinel.len();
        let mut rebuilt = String::with_capacity(html.len() + replacement.len());
        rebuilt.push_str(&html[..open_idx]);
        rebuilt.push_str(&replacement);
        rebuilt.push_str(&html[end_idx..]);
        tree.html = rebuilt;
    }

    Ok(())
}

/// Generate a collision-free `(open, close)` sentinel pair for one island.
///
/// Each sentinel is `ZFB_OPEN_{key_digest}_{nonce}` /
/// `ZFB_CLOSE_{key_digest}_{nonce}` where `key_digest` is a 16-hex-char
/// FNV-1a hash of the marker key and `nonce` is an 8-hex-char value
/// derived from `html` length, the key, and a seed that is incremented
/// until neither sentinel already appears verbatim in `html`.
///
/// The key is embedded as a **hex digest** rather than verbatim so the
/// sentinel is always `[A-Za-z0-9_]`: a raw key containing `& < > " '`
/// would be HTML-escaped by `Comment::replace(.., ContentType::Text)` in
/// Phase 1, after which the Phase-2 byte-find for the un-escaped sentinel
/// would silently miss (a spurious `OpenMarkerMissing`). Hex-only digest +
/// nonce also guarantee the sentinels never contain `-->`, which would
/// otherwise break `lol_html`'s comment-text validation.
fn make_sentinels(html: &str, key: &str) -> (String, String) {
    // Deterministic by design: same (html, key) → same sentinels on every
    // run. Reproducible output, and no `rand` dependency.
    let key_digest = format!("{:016x}", fnv1a(key.as_bytes()));
    let mut seed: u64 = 0;
    loop {
        // Cheap FNV-1a-style hash of (html.len(), key, seed).
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in (html.len() as u64)
            .to_le_bytes()
            .iter()
            .chain(key.as_bytes())
            .chain(seed.to_le_bytes().iter())
        {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let nonce = format!("{:08x}", hash as u32);
        let open = format!("ZFB_OPEN_{key_digest}_{nonce}");
        let close = format!("ZFB_CLOSE_{key_digest}_{nonce}");
        if !html.contains(&open) && !html.contains(&close) {
            return (open, close);
        }
        seed += 1;
    }
}

/// FNV-1a hash of `bytes`. Used to turn an arbitrary marker key into an
/// escape-proof hex token for sentinel construction.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Bridge rewriter for HTML emitted by Sub 4's `<Island when="…">` JSX
/// wrapper.
///
/// Uses `lol_html`'s CSS selector `div[data-zfb-island=""]` to locate
/// skeleton elements rather than scanning for the literal byte sequence
/// ` data-zfb-island=""`. This means text content inside `<pre>`, `<code>`,
/// or Markdown-rendered code blocks that happens to contain the string is
/// not miscounted.
///
/// Each skeleton's `data-zfb-island=""` is replaced with
/// `data-zfb-island="ComponentName"` and `data-props="…json…"` is
/// inserted. The existing `data-when` attribute (set by the wrapper) is
/// preserved.
///
/// **Dormant emitter note:** This is not the live SSR path. The live SSR island
/// emitter is `packages/zfb/src/island.ts`.
///
/// # Errors
///
/// - [`IslandSkeletonRewriteError::CountMismatch`] if the parser finds a
///   different number of skeleton elements than the length of `islands`.
pub fn rewrite_islands_in_attr_skeleton(
    tree: &mut HtmlTree,
    islands: &[IslandDescriptor],
) -> Result<(), IslandSkeletonRewriteError> {
    // Count skeletons via lol_html to detect mismatches before the rewrite.
    let skeleton_count = count_island_skeletons(&tree.html);
    if skeleton_count != islands.len() {
        return Err(IslandSkeletonRewriteError::CountMismatch {
            skeletons: skeleton_count,
            descriptors: islands.len(),
        });
    }

    if islands.is_empty() {
        return Ok(());
    }

    // Perform the rewrite: match skeletons in document order, pairing each
    // with the corresponding descriptor.
    //
    // lol_html's element handler fires in document order. We use an
    // Rc<Cell<usize>> index to advance through the descriptors.
    let idx = Rc::new(Cell::new(0usize));
    let descriptors: Vec<IslandDescriptor> = islands.to_vec();

    let selector: Selector = "div[data-zfb-island=\"\"]"
        .parse()
        .expect("static selector is valid");

    {
        let idx_c = Rc::clone(&idx);
        let settings: RewriteStrSettings<'_, '_> = RewriteStrSettings {
            element_content_handlers: vec![(
                Cow::Borrowed(&selector),
                ElementContentHandlers::default().element(move |el: &mut Element<'_, '_, _>| {
                    let i = idx_c.get();
                    let d = &descriptors[i];
                    idx_c.set(i + 1);

                    let props_json = serde_json::to_string(&d.props)
                        .expect("serde_json::Value always serialises to valid JSON");
                    el.set_attribute("data-zfb-island", &escape_html(&d.component_name))?;
                    el.set_attribute("data-props", &escape_html(&props_json))?;

                    Ok(())
                }),
            )],
            ..RewriteStrSettings::new()
        };

        tree.rewrite(settings)
            .expect("lol_html rewriting of well-formed skeleton HTML should not fail");
    }

    Ok(())
}

/// Count `<div data-zfb-island="">` skeleton elements using lol_html.
///
/// This is an internal helper used for the count-mismatch check before
/// the rewrite pass.
fn count_island_skeletons(html: &str) -> usize {
    let count = Rc::new(Cell::new(0usize));
    let selector: Selector = "div[data-zfb-island=\"\"]"
        .parse()
        .expect("static selector is valid");
    let count_c = Rc::clone(&count);
    let settings: RewriteStrSettings<'_, '_> = RewriteStrSettings {
        element_content_handlers: vec![(
            Cow::Borrowed(&selector),
            ElementContentHandlers::default().element(move |_el: &mut Element<'_, '_, _>| {
                count_c.set(count_c.get() + 1);
                Ok(())
            }),
        )],
        ..RewriteStrSettings::new()
    };
    let _ = lol_html::rewrite_str(html, settings);
    count.get()
}

// ---------------------------------------------------------------------------
// inject_runtime_script_into_head
// ---------------------------------------------------------------------------

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
        src = escape_html(runtime_url),
    )
}

/// Outcome of [`inject_runtime_script_into_head`]: where the runtime
/// `<script>` tag ended up.
///
/// Callers should treat [`PrependFallback`](Self::PrependFallback) as a
/// renderer bug — a well-formed page is expected to carry a `<head>`, and
/// the prepend path is only a best-effort fallback for headerless
/// fragments.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadInjection {
    /// The tag was appended inside the document's first `<head>` element,
    /// immediately before `</head>`.
    Head,
    /// No `<head>` element was found, so the tag was prepended to the
    /// document.
    PrependFallback,
}

/// Inject the islands-runtime `<script type="module">` tag into the
/// `<head>` element of `tree`.
///
/// Locates `<head>` via `lol_html`'s CSS selector rather than by string
/// search for `</head>`, which prevents false matches inside `<pre>` or
/// `<code>` content.
///
/// The tag is inserted **immediately before** `</head>` (appended to the
/// `<head>` element's inner content). Only the document's **first**
/// `<head>` is mutated; a malformed document with multiple `<head>`
/// elements would otherwise double-load the runtime. If no `<head>`
/// element is found the tag is prepended to the document and
/// [`HeadInjection::PrependFallback`] is returned — the caller should
/// treat that as a renderer bug.
///
/// `runtime_url` is HTML-attribute-escaped before being embedded.
pub fn inject_runtime_script_into_head(tree: &mut HtmlTree, runtime_url: &str) -> HeadInjection {
    let tag = islands_runtime_script_tag(runtime_url);

    let head_found = Rc::new(Cell::new(false));
    let head_found_c = Rc::clone(&head_found);
    let tag_for_handler = tag.clone();

    let selector: Selector = "head".parse().expect("static selector 'head' is valid");

    let settings: RewriteStrSettings<'_, '_> = RewriteStrSettings {
        element_content_handlers: vec![(
            Cow::Borrowed(&selector),
            ElementContentHandlers::default().element(move |el: &mut Element<'_, '_, _>| {
                // Guard against a second <head> (malformed input): only the
                // first one receives the runtime tag.
                if head_found_c.get() {
                    return Ok(());
                }
                head_found_c.set(true);
                el.append(&tag_for_handler, ContentType::Html);
                Ok(())
            }),
        )],
        ..RewriteStrSettings::new()
    };

    tree.rewrite(settings)
        .expect("lol_html rewriting for head injection should not fail");

    if head_found.get() {
        HeadInjection::Head
    } else {
        // Fragment / headerless: prepend the tag.
        let mut out = String::with_capacity(tag.len() + tree.html.len());
        out.push_str(&tag);
        out.push_str(&tree.html);
        tree.html = out;
        HeadInjection::PrependFallback
    }
}

// ---------------------------------------------------------------------------
// Auxiliary helpers
// ---------------------------------------------------------------------------

/// Build the `<div data-zfb-island="…" …>…</div>` wrapper for a single
/// island. Internal helper; tests cover the attribute-escaping rules.
///
/// **Dormant emitter note:** This is not the live SSR path. The live SSR island
/// emitter is `packages/zfb/src/island.ts`.
///
/// The `props` field is serialised to a JSON string here; this is the
/// single point where the `serde_json::Value` is converted to a string
/// that ends up in the `data-props` attribute.
fn render_wrapper(d: &IslandDescriptor, inner: &str) -> String {
    // Serialise props. `serde_json::to_string` on a `Value` is infallible
    // for any valid `Value` (all variants produce valid JSON).
    let props_json =
        serde_json::to_string(&d.props).expect("serde_json::Value always serialises to valid JSON");

    let mut s = String::with_capacity(inner.len() + props_json.len() + 96);
    s.push_str("<div data-zfb-island=\"");
    s.push_str(&escape_html(&d.component_name));
    s.push_str("\" data-props=\"");
    s.push_str(&escape_html(&props_json));
    s.push('"');
    if let Some(when) = &d.when {
        s.push_str(" data-when=\"");
        s.push_str(&escape_html(&when.as_str()));
        s.push('"');
        if let WhenHint::Media(query) = when {
            s.push_str(" data-media=\"");
            s.push_str(&escape_html(query));
            s.push('"');
        }
    }
    s.push('>');
    s.push_str(inner);
    s.push_str("</div>");
    s
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
        runtime = escape_html(runtime_url),
        bundle = escape_html(bundle_url),
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn page_with(inner: &str) -> HtmlTree {
        HtmlTree::parse(format!(
            "<html><body><h1>Page</h1>{inner}<footer>fin</footer></body></html>"
        ))
    }

    #[test]
    fn rewrites_single_island_into_data_attribute_wrapper() {
        let inner = wrap_with_markers("counter#0", "<button>3</button>");
        let mut tree = page_with(&inner);
        let d = IslandDescriptor::new("Counter", json!({"start": 3}), "counter#0");
        rewrite_islands(&mut tree, &[d]).unwrap();
        let out = tree.serialize();
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
        let mut tree = page_with(&inner);
        let d = IslandDescriptor::new("Foo", json!({}), "k").with_when(WhenHint::Visible);
        rewrite_islands(&mut tree, &[d]).unwrap();
        assert!(tree.serialize().contains(r#"data-when="visible""#));

        let inner = wrap_with_markers("k", "<x/>");
        let mut tree = page_with(&inner);
        let d = IslandDescriptor::new("Foo", json!({}), "k");
        rewrite_islands(&mut tree, &[d]).unwrap();
        assert!(!tree.serialize().contains("data-when="));
    }

    #[test]
    fn when_hint_variants_produce_correct_strings() {
        assert_eq!(WhenHint::Visible.as_str(), "visible");
        assert_eq!(WhenHint::Idle.as_str(), "idle");
        assert_eq!(WhenHint::Load.as_str(), "load");
        assert_eq!(
            WhenHint::Media("(max-width: 768px)".to_string()).as_str(),
            "media"
        );
    }

    #[test]
    fn when_hint_idle_is_emitted_in_html() {
        let inner = wrap_with_markers("k", "<x/>");
        let mut tree = page_with(&inner);
        let d = IslandDescriptor::new("Foo", json!({}), "k").with_when(WhenHint::Idle);
        rewrite_islands(&mut tree, &[d]).unwrap();
        assert!(tree.serialize().contains(r#"data-when="idle""#));
    }

    #[test]
    fn when_hint_media_emits_both_data_when_and_data_media_attributes() {
        let inner = wrap_with_markers("k", "<x/>");
        let mut tree = page_with(&inner);
        let d = IslandDescriptor::new("Foo", json!({}), "k")
            .with_when(WhenHint::Media("(max-width: 768px)".to_string()));
        rewrite_islands(&mut tree, &[d]).unwrap();
        let out = tree.serialize();
        assert!(
            out.contains(r#"data-when="media""#),
            "missing data-when=media: {out}"
        );
        assert!(
            out.contains(r#"data-media="(max-width: 768px)""#),
            "missing data-media attribute: {out}"
        );
    }

    #[test]
    fn when_hint_media_escapes_query_in_attribute() {
        let inner = wrap_with_markers("k", "<x/>");
        let mut tree = page_with(&inner);
        // Query contains double-quote which must be HTML-escaped.
        let d = IslandDescriptor::new("Foo", json!({}), "k").with_when(WhenHint::Media(
            r#"screen and (min-width: "640px")"#.to_string(),
        ));
        rewrite_islands(&mut tree, &[d]).unwrap();
        let out = tree.serialize();
        // The double-quote inside the value must be escaped as &quot;
        assert!(
            out.contains("data-media="),
            "data-media attribute missing: {out}"
        );
        assert!(
            !out.contains(r#"data-media="screen and (min-width: ""#),
            "unescaped double-quote in attribute: {out}"
        );
        assert!(
            out.contains("&quot;"),
            "expected HTML-escaped &quot; in output: {out}"
        );
    }

    #[test]
    fn handles_multiple_islands_in_order() {
        let mut html = String::from("<html><body>");
        html.push_str(&wrap_with_markers("a", "<i>1</i>"));
        html.push_str("<hr>");
        html.push_str(&wrap_with_markers("b", "<i>2</i>"));
        html.push_str("</body></html>");

        let mut tree = HtmlTree::parse(html);
        let descriptors = vec![
            IslandDescriptor::new("A", json!({}), "a"),
            IslandDescriptor::new("B", json!({"x": 1}), "b"),
        ];
        rewrite_islands(&mut tree, &descriptors).unwrap();
        let out = tree.serialize();
        let pos_a = out.find(r#"data-zfb-island="A""#).unwrap();
        let pos_b = out.find(r#"data-zfb-island="B""#).unwrap();
        assert!(pos_a < pos_b);
        assert!(out.contains("<i>1</i>"));
        assert!(out.contains("<i>2</i>"));
    }

    #[test]
    fn rewrite_single_island_ignores_marker_in_script_body() {
        // A <script> body containing the literal marker strings appears
        // BEFORE the real island markers. lol_html does not parse <!-- -->
        // inside raw-text elements as comments, so Phase 1 only replaces the
        // real markers and Phase 2 splices the correct span — the script
        // body is left untouched.
        let mut html = String::from("<html><body>");
        html.push_str(r#"<script>window.x = "<!--zfb-island:k--><!--/zfb-island:k-->";</script>"#);
        html.push_str(&wrap_with_markers("k", "<button>real</button>"));
        html.push_str("</body></html>");

        let mut tree = HtmlTree::parse(html);
        let d = IslandDescriptor::new("Counter", json!({"v": 1}), "k");
        rewrite_islands(&mut tree, &[d]).unwrap();
        let out = tree.serialize();

        // The real island was wrapped at its true location.
        assert!(
            out.contains(r#"<div data-zfb-island="Counter" data-props="{&quot;v&quot;:1}"><button>real</button></div>"#),
            "actual: {out}"
        );
        // The script body literal is preserved verbatim — not spliced.
        assert!(
            out.contains(
                r#"<script>window.x = "<!--zfb-island:k--><!--/zfb-island:k-->";</script>"#
            ),
            "script body must be untouched: {out}"
        );
    }

    #[test]
    fn marker_key_with_html_special_chars_round_trips() {
        // A marker key containing the HTML-special characters & < > " '
        // (but no `-->`). The sentinels are built from a hex digest of the
        // key, so Phase-1's ContentType::Text escaping cannot mangle them
        // and Phase-2's byte-find still locates the span. Before the digest
        // fix this silently produced OpenMarkerMissing.
        let key = "k&<>\"'#0";
        let inner = wrap_with_markers(key, "<button>real</button>");
        let mut tree = page_with(&inner);
        let d = IslandDescriptor::new("Counter", json!({"v": 1}), key);
        rewrite_islands(&mut tree, &[d]).unwrap();
        let out = tree.serialize();
        assert!(
            out.contains(r#"<div data-zfb-island="Counter""#),
            "island must be wrapped: {out}"
        );
        assert!(out.contains("<button>real</button>"));
        // Markers are gone.
        assert!(!out.contains("<!--zfb-island:"));
    }

    #[test]
    fn errors_when_open_marker_is_missing() {
        let mut tree = page_with("<span>plain</span>");
        let d = IslandDescriptor::new("Foo", json!({}), "missing");
        let err = rewrite_islands(&mut tree, &[d]).unwrap_err();
        assert!(matches!(err, IslandRewriteError::OpenMarkerMissing { .. }));
    }

    #[test]
    fn errors_when_close_marker_is_missing() {
        let mut tree =
            HtmlTree::parse("<html><body><!--zfb-island:k--><span>x</span></body></html>");
        let d = IslandDescriptor::new("Foo", json!({}), "k");
        let err = rewrite_islands(&mut tree, &[d]).unwrap_err();
        assert!(matches!(err, IslandRewriteError::CloseMarkerMissing { .. }));
    }

    #[test]
    fn errors_on_duplicate_marker_keys() {
        let mut tree = page_with(&wrap_with_markers("k", "<x/>"));
        let descriptors = vec![
            IslandDescriptor::new("A", json!({}), "k"),
            IslandDescriptor::new("B", json!({}), "k"),
        ];
        let err = rewrite_islands(&mut tree, &descriptors).unwrap_err();
        assert!(matches!(err, IslandRewriteError::DuplicateKey { .. }));
    }

    #[test]
    fn escapes_attributes_correctly() {
        let inner = wrap_with_markers("k", "<i>x</i>");
        let mut tree = page_with(&inner);
        // The JSON value has a string field containing HTML-special characters.
        let d = IslandDescriptor::new("My&Comp", json!({"text": "<a & \"b\">"}), "k");
        rewrite_islands(&mut tree, &[d]).unwrap();
        let out = tree.serialize();
        assert!(out.contains(r#"data-zfb-island="My&amp;Comp""#));
        assert!(out.contains("&quot;"));
        assert!(out.contains("&lt;"));
        assert!(out.contains("&gt;"));
        let attr_start = out.find("data-props=\"").unwrap() + "data-props=\"".len();
        let attr_end = out[attr_start..].find('"').unwrap() + attr_start;
        let attr_value = &out[attr_start..attr_end];
        assert!(!attr_value.contains('"'));
    }

    #[test]
    fn hydration_script_tag_escapes_urls() {
        // Inputs deliberately include adversarial characters and a
        // hashed URL fragment to verify attribute-escaping; the
        // bundler itself no longer mints hashed URLs (those come from
        // `ProductionAssetPipeline`), but this helper must still
        // round-trip whatever URL its caller passes.
        let tag = hydration_script_tag(
            "/runtime?v=1&t=2",
            "/dist/assets/islands-deadbeef.js?\"oops",
        );
        assert!(tag.starts_with("<script type=\"module\""));
        assert!(tag.contains(r#"src="/runtime?v=1&amp;t=2""#));
        assert!(tag.contains("&quot;oops"));
        assert!(tag.contains("data-zfb-bundle="));
    }

    #[test]
    fn islands_runtime_script_tag_minimal_shape() {
        // Stable URL per the S0 contract; production hashing is the
        // `ProductionAssetPipeline`'s job.
        let tag = islands_runtime_script_tag("/islands/islands-runtime.js");
        assert_eq!(
            tag,
            "<script type=\"module\" src=\"/islands/islands-runtime.js\"></script>"
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
        let mut tree = HtmlTree::parse(html);
        // Stable URL per the S0 contract — `ProductionAssetPipeline`
        // handles any hashing pass downstream.
        let outcome = inject_runtime_script_into_head(&mut tree, "/islands/islands-runtime.js");
        assert_eq!(outcome, HeadInjection::Head);
        let out = tree.serialize();
        let head_close = out.find("</head>").unwrap();
        let script_at = out
            .find("<script type=\"module\" src=\"/islands/islands-runtime.js\">")
            .expect("script tag injected");
        assert!(script_at < head_close, "script must appear before </head>");
        assert!(out.contains("<title>X</title>"));
        assert!(out.contains("<p>hi</p>"));
    }

    #[test]
    fn inject_runtime_script_falls_back_to_prepend_when_head_missing() {
        let mut tree = HtmlTree::parse("<p>fragment</p>");
        let outcome = inject_runtime_script_into_head(&mut tree, "/r.js");
        assert_eq!(outcome, HeadInjection::PrependFallback);
        let out = tree.serialize();
        assert!(out.starts_with("<script type=\"module\" src=\"/r.js\">"));
        assert!(out.ends_with("<p>fragment</p>"));
    }

    #[test]
    fn inject_runtime_script_only_first_head_mutated() {
        // A malformed document with two <head> start tags (as a buggy
        // renderer or hand-authored fragment might produce) must receive the
        // runtime script exactly once — only the first <head> is mutated.
        let html = "<!doctype html><html><head><title>A</title></head><head><title>B</title></head><body><p>hi</p></body></html>";
        let mut tree = HtmlTree::parse(html);
        let outcome = inject_runtime_script_into_head(&mut tree, "/islands/islands-runtime.js");
        assert_eq!(outcome, HeadInjection::Head);
        let out = tree.serialize();
        let count = out
            .matches("<script type=\"module\" src=\"/islands/islands-runtime.js\">")
            .count();
        assert_eq!(count, 1, "runtime script must appear exactly once: {out}");
    }

    #[test]
    fn island_marker_pair_shape_is_stable() {
        let (o, c) = island_marker_pair("counter#3");
        assert_eq!(o, "<!--zfb-island:counter#3-->");
        assert_eq!(c, "<!--/zfb-island:counter#3-->");
    }

    #[test]
    fn no_descriptors_is_a_no_op() {
        let html_str =
            "<html><body><h1>Page</h1><span>plain</span><footer>fin</footer></body></html>"
                .to_string();
        let mut tree = HtmlTree::parse(html_str.clone());
        rewrite_islands(&mut tree, &[]).unwrap();
        assert_eq!(tree.serialize(), html_str);
    }

    // ---- Bridge: rewrite_islands_in_attr_skeleton (Sub 4 wrapper output) ----

    #[test]
    fn skeleton_bridge_fills_empty_data_zfb_island() {
        let html = r#"<html><body><div data-zfb-island="" data-when="visible"><button>3</button></div></body></html>"#;
        let mut tree = HtmlTree::parse(html);
        let d = IslandDescriptor::new("Counter", json!({"start": 3}), "k");
        rewrite_islands_in_attr_skeleton(&mut tree, &[d]).unwrap();
        let out = tree.serialize();
        assert!(out.contains(r#"data-zfb-island="Counter""#));
        assert!(out.contains(r#"data-props="{&quot;start&quot;:3}""#));
        assert!(out.contains(r#"data-when="visible""#));
        assert!(out.contains("<button>3</button>"));
    }

    #[test]
    fn skeleton_bridge_pairs_in_document_order() {
        let html = r#"<html><body><div data-zfb-island="" data-when="visible"><i>a</i></div><span>between</span><div data-zfb-island="" data-when="idle"><i>b</i></div></body></html>"#;
        let mut tree = HtmlTree::parse(html);
        let descriptors = vec![
            IslandDescriptor::new("A", json!({}), "k1"),
            IslandDescriptor::new("B", json!({}), "k2"),
        ];
        rewrite_islands_in_attr_skeleton(&mut tree, &descriptors).unwrap();
        let out = tree.serialize();
        let pos_a = out.find(r#"data-zfb-island="A""#).unwrap();
        let pos_b = out.find(r#"data-zfb-island="B""#).unwrap();
        assert!(pos_a < pos_b);
        assert!(out.contains(r#"data-when="visible""#));
        assert!(out.contains(r#"data-when="idle""#));
    }

    #[test]
    fn skeleton_bridge_count_mismatch_is_an_error() {
        let html = r#"<div data-zfb-island="" data-when="load"><i>x</i></div>"#;
        let mut tree = HtmlTree::parse(html);
        let descriptors = vec![
            IslandDescriptor::new("A", json!({}), "k1"),
            IslandDescriptor::new("B", json!({}), "k2"),
        ];
        let err = rewrite_islands_in_attr_skeleton(&mut tree, &descriptors).unwrap_err();
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
        let mut tree = HtmlTree::parse(html);
        rewrite_islands_in_attr_skeleton(&mut tree, &[]).unwrap();
        assert_eq!(tree.serialize(), html);
    }

    // ---- Anchor-based locator: text content does not trigger false positives ----

    #[test]
    fn skeleton_bridge_ignores_data_zfb_island_in_text_content() {
        // A <pre> block containing the literal attribute string must NOT
        // be counted as a skeleton — the parser distinguishes elements from
        // text content.
        let html = r#"<html><body>
<pre><code>data-zfb-island=""</code></pre>
<div data-zfb-island="" data-when="load"><i>real island</i></div>
</body></html>"#;
        let mut tree = HtmlTree::parse(html);
        let d = IslandDescriptor::new("Real", json!({}), "k");
        // Should succeed: parser found exactly one skeleton element.
        rewrite_islands_in_attr_skeleton(&mut tree, &[d]).unwrap();
        let out = tree.serialize();
        assert!(out.contains(r#"data-zfb-island="Real""#));
        // The <pre> text is not modified.
        assert!(out.contains(r#"<pre><code>data-zfb-island=""</code></pre>"#));
    }

    // ---- Islands ↔ render-output marker contract (item #1146-2) ──────────────
    //
    // These tests simulate what a "use client" page render produces and then
    // assert the rewriter produces the correct wrapper shape.
    //
    // ## Why no real V8 render path
    //
    // The renderer in `zfb-render` does NOT yet emit
    // `<!--zfb-island:KEY-->` markers (see the module-level doc, line
    // 52-57). When the renderer is updated (Sub 4), a Level-4 E2E test
    // should drive the full pipeline with a real V8 bundle. Until then
    // this Level-3 test exercises the same marker contract using hand-crafted
    // HTML that exactly matches `island_marker_pair`'s output — the
    // single public source of truth for the marker shape.
    //
    // This test is CI-runnable (`cargo test --workspace`) with no V8 or
    // esbuild dependency.

    /// Simulate a minimal "use client" page render: a page that server-renders
    /// two counter islands inside the expected marker brackets. Assert that
    /// `rewrite_islands` produces the canonical wrapper shape for each.
    #[test]
    fn use_client_page_marker_to_wrapper_shape() {
        // Canonical markers from the public helper — identical to what the
        // renderer is expected to emit once Sub 4 is done.
        let (open_a, close_a) = island_marker_pair("counter#0");
        let (open_b, close_b) = island_marker_pair("counter#1");

        // Simulate server-rendered HTML for a "use client" page with two Counter
        // islands. The renderer wraps each island's SSR output in the marker pair.
        let html = format!(
            r#"<!doctype html><html><head><title>Demo</title></head><body>
<h1>My Page</h1>
{open_a}<button>Count: 0</button>{close_a}
<p>Some text between islands.</p>
{open_b}<button>Count: 5</button>{close_b}
</body></html>"#
        );

        let mut tree = HtmlTree::parse(html);
        let islands = vec![
            IslandDescriptor::new("Counter", json!({"start": 0}), "counter#0"),
            IslandDescriptor::new("Counter", json!({"start": 5}), "counter#1")
                .with_when(WhenHint::Visible),
        ];

        rewrite_islands(&mut tree, &islands).unwrap();
        let out = tree.serialize();

        // ── wrapper shape for island A (no data-when) ────────────────────
        assert!(
            out.contains(r#"data-zfb-island="Counter""#),
            "wrapper attribute present: {out}"
        );
        assert!(
            out.contains(r#"data-props="{&quot;start&quot;:0}""#),
            "props for counter#0: {out}"
        );
        assert!(
            out.contains("<button>Count: 0</button>"),
            "inner HTML preserved for counter#0: {out}"
        );

        // ── wrapper shape for island B (data-when=visible) ───────────────
        assert!(
            out.contains(r#"data-props="{&quot;start&quot;:5}""#),
            "props for counter#1: {out}"
        );
        assert!(
            out.contains(r#"data-when="visible""#),
            "data-when emitted for counter#1: {out}"
        );
        assert!(
            out.contains("<button>Count: 5</button>"),
            "inner HTML preserved for counter#1: {out}"
        );

        // ── markers are gone ─────────────────────────────────────────────
        assert!(
            !out.contains("<!--zfb-island:"),
            "open markers must be removed: {out}"
        );
        assert!(
            !out.contains("<!--/zfb-island:"),
            "close markers must be removed: {out}"
        );

        // ── document structure preserved ─────────────────────────────────
        assert!(out.contains("<h1>My Page</h1>"), "static content intact");
        assert!(
            out.contains("<p>Some text between islands.</p>"),
            "between-island content intact"
        );
    }

    /// Verify `island_marker_pair` shape is stable — both the open/close
    /// forms and the fact that `<!--` / `-->` delimiters are present.
    /// This is the source-of-truth contract test referenced in item #1146-2.
    #[test]
    fn island_marker_pair_contract_shape() {
        let (open, close) = island_marker_pair("MyComp#3");
        // Open marker must start with `<!--zfb-island:` and end with `-->`.
        assert!(
            open.starts_with("<!--zfb-island:"),
            "open marker prefix: {open}"
        );
        assert!(open.ends_with("-->"), "open marker suffix: {open}");
        // Close marker must start with `<!--/zfb-island:` and end with `-->`.
        assert!(
            close.starts_with("<!--/zfb-island:"),
            "close marker prefix: {close}"
        );
        assert!(close.ends_with("-->"), "close marker suffix: {close}");
        // Both must embed the key.
        assert!(open.contains("MyComp#3"), "open contains key: {open}");
        assert!(close.contains("MyComp#3"), "close contains key: {close}");
        // Open and close must differ (and thus be distinguishable by the parser).
        assert_ne!(open, close, "open and close must be distinct");
    }

    /// Verify that the rewriter correctly handles a "use client" page where
    /// the same component appears twice under different marker keys — the
    /// common case when a single component type is used in multiple locations.
    #[test]
    fn use_client_same_component_two_instances() {
        let (o1, c1) = island_marker_pair("btn#0");
        let (o2, c2) = island_marker_pair("btn#1");
        let html =
            format!("<html><body>{o1}<span>A</span>{c1}<hr>{o2}<span>B</span>{c2}</body></html>");
        let mut tree = HtmlTree::parse(html);
        let islands = vec![
            IslandDescriptor::new("Button", json!({"label": "A"}), "btn#0"),
            IslandDescriptor::new("Button", json!({"label": "B"}), "btn#1"),
        ];
        rewrite_islands(&mut tree, &islands).unwrap();
        let out = tree.serialize();

        // Both instances must be wrapped.
        let count = out.matches(r#"data-zfb-island="Button""#).count();
        assert_eq!(count, 2, "both Button instances wrapped: {out}");
        // Inner HTML preserved for each.
        assert!(
            out.contains("<span>A</span>"),
            "first instance inner: {out}"
        );
        assert!(
            out.contains("<span>B</span>"),
            "second instance inner: {out}"
        );
        // No dangling markers.
        assert!(!out.contains("<!--zfb-island:"), "markers removed: {out}");
    }
}
