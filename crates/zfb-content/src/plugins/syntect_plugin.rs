//! Replace fenced code blocks with syntect-highlighted structured HAST.
//!
//! Wraps [`crate::syntect_highlight::Highlighter`] as a hast plugin.
//! For each `<pre><code data-lang="…">…</code></pre>` element (the
//! `data-lang` is set by Sub 3's mdast→hast conversion), this plugin:
//!
//! 1. Skips `mermaid` blocks — those are handled by
//!    [`crate::plugins::MermaidPlugin`].
//! 2. Calls `highlighter.highlight_lines(code, Some(lang), None)` (single
//!    mode) or `highlighter.highlight_lines_dual(...)` (dual mode).
//! 3. Replaces the entire `<pre>` [`HastNode`] with a structured HAST
//!    tree of the form:
//!    ```text
//!    Element<pre class="syntect-{slug}">          (single mode)
//!    Element<pre class="syntect-dual" style="…">  (dual mode)
//!      Element<code>
//!        Element<span class="line">  Raw(line_1_html)  </span>
//!        Element<span class="line">  Raw(line_2_html)  </span>
//!        …
//!    ```
//!    Each `<span class="line">` is an independent [`HastNode::Element`]
//!    that downstream visitors (e.g. wave-5 code-enrichment) can mutate
//!    to add diff markers or line-highlight classes.
//!
//! The per-line content (colored token spans) is kept as
//! [`HastNode::Raw`] inside each `<span class="line">` so the
//! serialized HTML output is byte-identical to what a flat concatenation
//! of the same token spans would produce — the only new bytes are the
//! `<span class="line">` / `</span>` wrappers.
//!
//! The plugin holds the highlighter behind an [`Arc`] so it can be
//! cheaply cloned across multi-document pipelines.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::pipeline::{HastNode, HastVisitor};
use crate::syntect_highlight::Highlighter;

/// Internal mode selector for [`SyntectPlugin`].
///
/// `Single` uses the existing single-theme path (inline `color:`).
/// `Dual` calls `highlight_lines_dual` and emits `--shiki-light` /
/// `--shiki-dark` custom properties instead of inline color.
#[derive(Clone)]
enum SyntectMode {
    /// Single theme — forwards to `highlight_lines` with the optional
    /// theme name override. `None` falls back to the highlighter default.
    Single(Option<String>),
    /// Dual light + dark theme — forwards to `highlight_lines_dual` and
    /// emits `<pre class="syntect-dual" style="--shiki-light-bg:…;--shiki-dark-bg:…">`.
    ///
    /// Theme names are SYNTECT names (e.g. `"base16-ocean.light"`,
    /// `"base16-ocean.dark"`), NOT Shiki names like `"dracula"`.
    Dual {
        /// Light-mode syntect theme name (e.g. `"base16-ocean.light"`).
        light: String,
        /// Dark-mode syntect theme name (e.g. `"base16-ocean.dark"`).
        dark: String,
    },
    /// Class-emission mode — forwards to `highlight_lines_classes` and emits
    /// per-token `<span class="…">` role classes (from the #1529 taxonomy)
    /// instead of any inline colour. The `<pre>` element is classed
    /// `{prefix}root` (e.g. `hi-root`). This is `codeHighlight.mode: "class"`
    /// (Highlight Tokens epic, zfb#1528).
    Classes {
        /// Class-name prefix for role classes (e.g. `"hi-"`); the default
        /// class for a role is `{prefix}{short_name}`.
        prefix: String,
        /// Per-role class overrides keyed by role short name (e.g. `"kw"`).
        role_classes: BTreeMap<String, String>,
    },
}

/// Visitor that swaps fenced code blocks for syntect HTML.
#[derive(Clone)]
pub struct SyntectPlugin {
    highlighter: Arc<Highlighter>,
    mode: SyntectMode,
}

impl SyntectPlugin {
    /// Construct with a shared highlighter in single-theme mode.
    ///
    /// The theme defaults to the highlighter's configured default
    /// (`base16-ocean.dark`). Use [`SyntectPlugin::with_theme`] to
    /// override it, or [`SyntectPlugin::with_dual_themes`] for dual mode.
    #[must_use]
    pub fn new(highlighter: Arc<Highlighter>) -> Self {
        Self {
            highlighter,
            mode: SyntectMode::Single(None),
        }
    }

    /// Override the theme passed to the highlighter (defaults to the
    /// highlighter's configured default).
    ///
    /// The name must be a SYNTECT built-in or user-loaded theme name
    /// (e.g. `"InspiredGitHub"`, `"Solarized (dark)"`), NOT a Shiki
    /// name like `"dracula"`.
    #[must_use]
    pub fn with_theme(mut self, theme: impl Into<String>) -> Self {
        self.mode = SyntectMode::Single(Some(theme.into()));
        self
    }

    /// Switch to dual light + dark theme mode.
    ///
    /// In dual mode every fenced code block is highlighted twice — once
    /// with `theme_light` and once with `theme_dark` — and the per-token
    /// colors are emitted as CSS custom properties (`--shiki-light` /
    /// `--shiki-dark`) instead of inline `color:`. The `<pre>` element
    /// gets `class="syntect-dual"` and carries `--shiki-light-bg` /
    /// `--shiki-dark-bg` as inline `style` variables (omitted when the
    /// theme has no background). The consumer resolves the active color
    /// with a `light-dark()` CSS rule.
    ///
    /// Both names must be SYNTECT built-in or user-loaded theme names
    /// (e.g. `"base16-ocean.light"`, `"base16-ocean.dark"`), NOT Shiki
    /// names like `"dracula"`.
    #[must_use]
    pub fn with_dual_themes(
        mut self,
        theme_light: impl Into<String>,
        theme_dark: impl Into<String>,
    ) -> Self {
        self.mode = SyntectMode::Dual {
            light: theme_light.into(),
            dark: theme_dark.into(),
        };
        self
    }

    /// Switch to class-emission mode (`codeHighlight.mode: "class"`).
    ///
    /// In class mode every fenced code block is highlighted via
    /// [`Highlighter::highlight_lines_classes`]: each token becomes a
    /// `<span class="…">` carrying a semantic role class (default
    /// `{prefix}{short_name}`, e.g. `hi-kw`, or the `role_classes` override
    /// for that role) instead of any inline `color:` or `--shiki-*` property.
    /// The `<pre>` element is classed `{prefix}root` (default `hi-root`).
    #[must_use]
    pub fn with_class_mode(
        mut self,
        prefix: impl Into<String>,
        role_classes: BTreeMap<String, String>,
    ) -> Self {
        self.mode = SyntectMode::Classes {
            prefix: prefix.into(),
            role_classes,
        };
        self
    }
}

impl HastVisitor for SyntectPlugin {
    fn visit(&mut self, node: &mut HastNode) {
        match node {
            HastNode::Root { children } | HastNode::Element { children, .. } => {
                rewrite_children(children, &self.highlighter, &self.mode);
                for c in children {
                    self.visit(c);
                }
            }
            _ => {}
        }
    }
}

fn rewrite_children(children: &mut [HastNode], highlighter: &Highlighter, mode: &SyntectMode) {
    for child in children.iter_mut() {
        if let Some((lang, meta, code)) = lang_and_code(child) {
            if lang == "mermaid" {
                continue;
            }
            match mode {
                SyntectMode::Single(theme) => {
                    rewrite_single(child, highlighter, theme.as_deref(), &lang, meta, &code);
                }
                SyntectMode::Dual { light, dark } => {
                    rewrite_dual(child, highlighter, light, dark, &lang, meta, &code);
                }
                SyntectMode::Classes {
                    prefix,
                    role_classes,
                } => {
                    rewrite_classes(child, highlighter, prefix, role_classes, &lang, meta, &code);
                }
            }
        }
    }
}

/// Rewrite `child` using the single-theme path. Mirrors the pre-dual logic.
fn rewrite_single(
    child: &mut HastNode,
    highlighter: &Highlighter,
    theme: Option<&str>,
    lang: &str,
    meta: Option<String>,
    code: &str,
) {
    if let Ok(result) = highlighter.highlight_lines(code, Some(lang), theme) {
        // Build structured HAST: <pre class="syntect-{slug}"><code>
        //   <span class="line">Raw(line_html)</span>…
        // </code></pre>
        // The per-line Raw keeps token-span HTML verbatim; the
        // <span class="line"> Element wrapper exposes each line to
        // downstream visitors (wave-5 code-enrichment).
        let line_spans = build_line_spans(result.lines);
        // Preserve `data-meta` on the new <code> element so downstream
        // hast visitors (e.g. wave-5 CodeEnrichmentPlugin) can read the
        // fence info-string (e.g. `{1,3-5}` for line highlighting).
        let code_el = build_code_el(line_spans, meta);
        *child = HastNode::Element {
            tag: "pre".to_string(),
            attrs: vec![(
                "class".to_string(),
                format!("syntect-{}", result.theme_slug),
            )],
            children: vec![code_el],
            void: false,
        };
    }
}

/// Rewrite `child` using the dual light/dark theme path.
///
/// Calls `highlight_lines_dual` and emits:
/// ```html
/// <pre class="syntect-dual" style="--shiki-light-bg:#…;--shiki-dark-bg:#…">
///   <code>
///     <span class="line">Raw(dual_line_html)</span>…
///   </code>
/// </pre>
/// ```
///
/// Per-token spans carry `--shiki-light:#…;--shiki-dark:#…` custom
/// properties (NO inline `color:`). The `--shiki-*-bg` vars are emitted
/// only when the corresponding theme provides a background color; if
/// BOTH themes lack a background the `style` attribute is omitted
/// entirely.
fn rewrite_dual(
    child: &mut HastNode,
    highlighter: &Highlighter,
    theme_light: &str,
    theme_dark: &str,
    lang: &str,
    meta: Option<String>,
    code: &str,
) {
    if let Ok(result) = highlighter.highlight_lines_dual(code, Some(lang), theme_light, theme_dark)
    {
        let line_spans = build_line_spans(result.lines);
        let code_el = build_code_el(line_spans, meta);

        // Build the style attribute carrying the --shiki-*-bg variables.
        // Omit a given *-bg var when its Option is None (colour-only theme);
        // omit the style attr entirely when both are None.
        let mut style_parts: Vec<String> = Vec::new();
        if let Some(light_bg) = &result.light_bg {
            style_parts.push(format!("--shiki-light-bg:{light_bg}"));
        }
        if let Some(dark_bg) = &result.dark_bg {
            style_parts.push(format!("--shiki-dark-bg:{dark_bg}"));
        }

        let mut attrs: Vec<(String, String)> =
            vec![("class".to_string(), "syntect-dual".to_string())];
        if !style_parts.is_empty() {
            attrs.push(("style".to_string(), style_parts.join(";")));
        }

        *child = HastNode::Element {
            tag: "pre".to_string(),
            attrs,
            children: vec![code_el],
            void: false,
        };
    }
}

/// Rewrite `child` using the class-emission path.
///
/// Calls [`Highlighter::highlight_lines_classes`] and emits:
/// ```html
/// <pre class="{prefix}root">
///   <code>
///     <span class="line">Raw(class_line_html)</span>…
///   </code>
/// </pre>
/// ```
///
/// Per-token spans carry role classes (`{prefix}{short_name}` or the
/// `role_classes` override) with NO inline colour. The `<pre>` element is
/// classed `{prefix}root` (default `hi-root`). The `<pre> → <code>[data-meta]
/// → <span class="line"> → Raw` shape is byte-identical to the single/dual
/// paths — `build_line_spans` / `build_code_el` are reused verbatim.
fn rewrite_classes(
    child: &mut HastNode,
    highlighter: &Highlighter,
    prefix: &str,
    role_classes: &BTreeMap<String, String>,
    lang: &str,
    meta: Option<String>,
    code: &str,
) {
    if let Ok(result) = highlighter.highlight_lines_classes(code, Some(lang), prefix, role_classes)
    {
        let line_spans = build_line_spans(result.lines);
        let code_el = build_code_el(line_spans, meta);
        *child = HastNode::Element {
            tag: "pre".to_string(),
            attrs: vec![("class".to_string(), format!("{prefix}root"))],
            children: vec![code_el],
            void: false,
        };
    }
}

/// Build a `<code>` element wrapping the given line spans.
///
/// Preserves `data-meta` when the fence carried a meta string so
/// downstream hast visitors (e.g. wave-5 `CodeEnrichmentPlugin`) can
/// read the fence info-string (e.g. `{1,3-5}` for line highlighting).
fn build_code_el(line_spans: Vec<HastNode>, meta: Option<String>) -> HastNode {
    let mut code_attrs: Vec<(String, String)> = Vec::new();
    if let Some(m) = meta {
        code_attrs.push(("data-meta".to_string(), m));
    }
    HastNode::Element {
        tag: "code".to_string(),
        attrs: code_attrs,
        children: line_spans,
        void: false,
    }
}

/// Wrap each per-line HTML fragment in `<span class="line">Raw(…)</span>`.
fn build_line_spans(lines: Vec<String>) -> Vec<HastNode> {
    lines
        .into_iter()
        .map(|line_html| HastNode::Element {
            tag: "span".to_string(),
            attrs: vec![("class".to_string(), "line".to_string())],
            children: vec![HastNode::Raw(line_html)],
            void: false,
        })
        .collect()
}

/// If `node` is `<pre><code data-lang="…">TEXT</code></pre>`, return
/// `(lang, meta, code_text)` where `meta` is the optional `data-meta` value.
fn lang_and_code(node: &HastNode) -> Option<(String, Option<String>, String)> {
    let HastNode::Element { tag, children, .. } = node else {
        return None;
    };
    if tag != "pre" {
        return None;
    }
    let HastNode::Element {
        tag: ctag,
        attrs,
        children: code_children,
        ..
    } = children.first()?
    else {
        return None;
    };
    if ctag != "code" {
        return None;
    }
    let lang = attrs
        .iter()
        .find(|(k, _)| k == "data-lang")
        .map(|(_, v)| v.clone())?;
    let meta = attrs
        .iter()
        .find(|(k, _)| k == "data-meta")
        .map(|(_, v)| v.clone());
    let mut code = String::new();
    for c in code_children {
        if let HastNode::Text(t) = c {
            code.push_str(t);
        }
    }
    Some((lang, meta, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre_code(lang: Option<&str>, body: &str) -> HastNode {
        let mut attrs: Vec<(String, String)> = Vec::new();
        if let Some(l) = lang {
            attrs.push(("data-lang".to_string(), l.to_string()));
        }
        HastNode::Element {
            tag: "pre".to_string(),
            attrs: vec![],
            children: vec![HastNode::Element {
                tag: "code".to_string(),
                attrs,
                children: vec![HastNode::Text(body.to_string())],
                void: false,
            }],
            void: false,
        }
    }

    /// Walk a structured syntect HAST node and assert the expected shape:
    /// `Element<pre class="syntect-…">` → `Element<code>` → N
    /// `Element<span class="line">` each containing `Raw(…)`.
    ///
    /// Returns the serialized HTML so callers can make content assertions.
    fn assert_syntect_structure(node: &HastNode) -> String {
        use crate::serializer::serialize;

        let HastNode::Element {
            tag: pre_tag,
            attrs: pre_attrs,
            children: pre_children,
            ..
        } = node
        else {
            panic!("expected Element<pre>, got {node:?}");
        };
        assert_eq!(pre_tag, "pre", "outer tag must be pre");
        assert!(
            pre_attrs
                .iter()
                .any(|(k, v)| k == "class" && v.starts_with("syntect-")),
            "pre must have class=\"syntect-…\": {pre_attrs:?}"
        );
        let code_el = pre_children.first().expect("pre must have a child <code>");
        let HastNode::Element {
            tag: code_tag,
            children: code_children,
            ..
        } = code_el
        else {
            panic!("expected Element<code> inside pre, got {code_el:?}");
        };
        assert_eq!(code_tag, "code", "inner tag must be code");
        for (i, span) in code_children.iter().enumerate() {
            let HastNode::Element {
                tag: span_tag,
                attrs: span_attrs,
                children: span_children,
                ..
            } = span
            else {
                panic!("line {i}: expected Element<span>, got {span:?}");
            };
            assert_eq!(span_tag, "span", "line {i}: span tag must be span");
            assert!(
                span_attrs.iter().any(|(k, v)| k == "class" && v == "line"),
                "line {i}: span must have class=\"line\": {span_attrs:?}"
            );
            assert_eq!(
                span_children.len(),
                1,
                "line {i}: span must have exactly one child (Raw)"
            );
            assert!(
                matches!(span_children[0], HastNode::Raw(_)),
                "line {i}: span child must be Raw, got {:?}",
                span_children[0]
            );
        }
        serialize(node)
    }

    #[test]
    fn highlights_rust_block() {
        let h = Arc::new(Highlighter::new());
        let mut plugin = SyntectPlugin::new(h);
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("rust"), "fn main() {}\n")],
        };
        plugin.visit(&mut tree);
        let HastNode::Root { children } = tree else {
            unreachable!("expected HastNode::Root")
        };
        // The plugin now emits a structured Element tree, not a Raw blob.
        let html = assert_syntect_structure(&children[0]);
        assert!(html.contains("<pre class=\"syntect-"), "got: {html}");
        assert!(html.contains("</code></pre>"));
    }

    #[test]
    fn skips_mermaid_block() {
        let h = Arc::new(Highlighter::new());
        let mut plugin = SyntectPlugin::new(h);
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("mermaid"), "graph TD;")],
        };
        let original = tree.clone();
        plugin.visit(&mut tree);
        assert_eq!(tree, original, "mermaid blocks must be untouched");
    }

    #[test]
    fn skips_block_without_data_lang() {
        let h = Arc::new(Highlighter::new());
        let mut plugin = SyntectPlugin::new(h);
        let mut tree = HastNode::Root {
            children: vec![pre_code(None, "anything")],
        };
        let original = tree.clone();
        plugin.visit(&mut tree);
        assert_eq!(tree, original);
    }

    #[test]
    fn unknown_language_falls_back_to_pre_code() {
        let h = Arc::new(Highlighter::new());
        let mut plugin = SyntectPlugin::new(h);
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("klingon"), "hello")],
        };
        plugin.visit(&mut tree);
        let HastNode::Root { children } = tree else {
            unreachable!("expected HastNode::Root")
        };
        // Even on the fallback path the plugin emits structured HAST.
        let html = assert_syntect_structure(&children[0]);
        // Unknown lang now produces a themed wrapper instead of bare <pre><code>
        assert!(
            html.starts_with("<pre class=\"syntect-"),
            "expected themed wrapper for unknown lang: {html}"
        );
        assert!(html.contains("hello"), "code content missing: {html}");
    }

    // ── Dual-theme mode (with_dual_themes) ────────────────────────────────

    /// Acceptance: dual mode emits `<pre class="syntect-dual">` with
    /// `--shiki-light-bg`/`--shiki-dark-bg` in the `style` attribute, and
    /// per-token spans carry `--shiki-light`/`--shiki-dark` with NO inline
    /// `color:`.
    #[test]
    fn dual_mode_emits_syntect_dual_class_and_bg_vars() {
        let h = Arc::new(Highlighter::new());
        let mut plugin =
            SyntectPlugin::new(h).with_dual_themes("base16-ocean.light", "base16-ocean.dark");
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("rust"), "fn main() {}\n")],
        };
        plugin.visit(&mut tree);

        let HastNode::Root { children } = &tree else {
            unreachable!("expected Root")
        };
        let HastNode::Element {
            tag,
            attrs,
            children: pre_children,
            ..
        } = &children[0]
        else {
            panic!("expected Element<pre>, got {:?}", children[0])
        };
        assert_eq!(tag, "pre");
        // class must be "syntect-dual"
        assert!(
            attrs
                .iter()
                .any(|(k, v)| k == "class" && v == "syntect-dual"),
            "pre must have class=\"syntect-dual\": {attrs:?}"
        );
        // style must carry both --shiki-light-bg and --shiki-dark-bg
        let style = attrs
            .iter()
            .find(|(k, _)| k == "style")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        assert!(
            style.contains("--shiki-light-bg:"),
            "style must contain --shiki-light-bg: {style}"
        );
        assert!(
            style.contains("--shiki-dark-bg:"),
            "style must contain --shiki-dark-bg: {style}"
        );

        // Serialize the whole tree and check token properties.
        use crate::serializer::serialize;
        let html = serialize(&children[0]);
        assert!(
            html.contains("--shiki-light:#"),
            "must contain --shiki-light: {html}"
        );
        assert!(
            html.contains("--shiki-dark:#"),
            "must contain --shiki-dark: {html}"
        );
        // The whole point: no inline `color:` in dual mode.
        assert!(
            !html.contains("color:#"),
            "dual mode must NOT emit inline color: {html}"
        );

        // <code> must still be present, with <span class="line"> children.
        let code_el = pre_children.first().expect("pre must have <code>");
        let HastNode::Element {
            tag: code_tag,
            children: code_children,
            ..
        } = code_el
        else {
            panic!("expected Element<code>");
        };
        assert_eq!(code_tag, "code");
        assert!(
            !code_children.is_empty(),
            "code must have line span children"
        );
    }

    /// Byte-identity: single mode must still emit inline `color:` and
    /// NO `--shiki-*` vars (the pre-dual output is unchanged).
    #[test]
    fn single_mode_still_emits_inline_color_not_shiki_vars() {
        let h = Arc::new(Highlighter::new());
        let mut plugin = SyntectPlugin::new(h);
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("rust"), "fn main() {}\n")],
        };
        plugin.visit(&mut tree);

        let HastNode::Root { children } = &tree else {
            unreachable!("expected Root")
        };
        use crate::serializer::serialize;
        let html = serialize(&children[0]);
        assert!(
            html.contains("color:#"),
            "single mode must still emit inline color: {html}"
        );
        assert!(
            !html.contains("--shiki-"),
            "single mode must NOT emit --shiki- vars: {html}"
        );
    }

    /// Dual mode mermaid blocks are skipped (same as single mode).
    #[test]
    fn dual_mode_skips_mermaid_block() {
        let h = Arc::new(Highlighter::new());
        let mut plugin =
            SyntectPlugin::new(h).with_dual_themes("base16-ocean.light", "base16-ocean.dark");
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("mermaid"), "graph TD;")],
        };
        let original = tree.clone();
        plugin.visit(&mut tree);
        assert_eq!(tree, original, "dual mode must also skip mermaid blocks");
    }

    /// New acceptance criterion (issue #571): a 3-line `<pre>` produces
    /// 3 `<span class="line">` Element nodes inside `<code>` that a
    /// downstream visitor could mutate independently.
    #[test]
    fn three_line_block_produces_three_span_line_elements() {
        let h = Arc::new(Highlighter::new());
        let mut plugin = SyntectPlugin::new(h);
        let code = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("rust"), code)],
        };
        plugin.visit(&mut tree);

        let HastNode::Root { children } = &tree else {
            unreachable!("expected Root")
        };
        let HastNode::Element {
            tag,
            children: pre_children,
            ..
        } = &children[0]
        else {
            panic!("expected Element<pre>, got {:?}", children[0])
        };
        assert_eq!(tag, "pre");
        let HastNode::Element {
            tag: code_tag,
            children: code_children,
            ..
        } = pre_children.first().expect("pre must have <code>")
        else {
            panic!("expected Element<code>")
        };
        assert_eq!(code_tag, "code");
        // 3 source lines → 3 <span class="line"> elements.
        assert_eq!(
            code_children.len(),
            3,
            "expected 3 line spans, got {}: {code_children:?}",
            code_children.len()
        );
        for (i, span) in code_children.iter().enumerate() {
            let HastNode::Element {
                tag,
                attrs,
                children: span_children,
                ..
            } = span
            else {
                panic!("line {i}: expected Element<span>, got {span:?}");
            };
            assert_eq!(tag, "span", "line {i}: wrong tag");
            assert!(
                attrs.iter().any(|(k, v)| k == "class" && v == "line"),
                "line {i}: missing class=\"line\": {attrs:?}"
            );
            // Each span must have exactly one Raw child (the token HTML).
            assert_eq!(span_children.len(), 1, "line {i}: expected 1 child");
            assert!(
                matches!(span_children[0], HastNode::Raw(_)),
                "line {i}: child must be Raw, got {:?}",
                span_children[0]
            );
        }
    }

    // ── Class-emission mode (with_class_mode) ─────────────────────────────

    /// Build a class-mode plugin over a fresh highlighter with the given
    /// prefix and empty role overrides.
    fn class_plugin(prefix: &str) -> SyntectPlugin {
        SyntectPlugin::new(Arc::new(Highlighter::new())).with_class_mode(prefix, BTreeMap::new())
    }

    /// Structural check for class mode, mirroring [`assert_syntect_structure`]
    /// but expecting `class="{prefix}root"` on the `<pre>` instead of a
    /// `syntect-…` slug. Returns the serialized HTML.
    fn assert_class_structure(node: &HastNode, prefix: &str) -> String {
        use crate::serializer::serialize;

        let HastNode::Element {
            tag: pre_tag,
            attrs: pre_attrs,
            children: pre_children,
            ..
        } = node
        else {
            panic!("expected Element<pre>, got {node:?}");
        };
        assert_eq!(pre_tag, "pre", "outer tag must be pre");
        let expected_root = format!("{prefix}root");
        assert!(
            pre_attrs
                .iter()
                .any(|(k, v)| k == "class" && *v == expected_root),
            "pre must have class=\"{expected_root}\": {pre_attrs:?}"
        );
        let code_el = pre_children.first().expect("pre must have a child <code>");
        let HastNode::Element {
            tag: code_tag,
            children: code_children,
            ..
        } = code_el
        else {
            panic!("expected Element<code> inside pre, got {code_el:?}");
        };
        assert_eq!(code_tag, "code", "inner tag must be code");
        for (i, span) in code_children.iter().enumerate() {
            let HastNode::Element {
                tag: span_tag,
                attrs: span_attrs,
                children: span_children,
                ..
            } = span
            else {
                panic!("line {i}: expected Element<span>, got {span:?}");
            };
            assert_eq!(span_tag, "span", "line {i}: span tag must be span");
            assert!(
                span_attrs.iter().any(|(k, v)| k == "class" && v == "line"),
                "line {i}: span must have class=\"line\": {span_attrs:?}"
            );
            assert_eq!(
                span_children.len(),
                1,
                "line {i}: span must have exactly one child (Raw)"
            );
            assert!(
                matches!(span_children[0], HastNode::Raw(_)),
                "line {i}: span child must be Raw, got {:?}",
                span_children[0]
            );
        }
        serialize(node)
    }

    /// Class mode emits `<pre class="hi-root">` with role-classed token spans
    /// and NO inline colour form, preserving the structured HAST shape.
    #[test]
    fn class_mode_emits_prefix_root_and_role_classes() {
        let mut plugin = class_plugin("hi-");
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("rust"), "fn main() {}\n")],
        };
        plugin.visit(&mut tree);
        let HastNode::Root { children } = &tree else {
            unreachable!("expected Root")
        };
        let html = assert_class_structure(&children[0], "hi-");
        assert!(
            html.contains("class=\"hi-root\""),
            "pre must carry hi-root: {html}"
        );
        assert!(
            html.contains("class=\"hi-kw\""),
            "must carry a role class: {html}"
        );
        assert!(
            !html.contains("color:#"),
            "class mode must NOT emit inline color: {html}"
        );
        assert!(
            !html.contains("--shiki-"),
            "class mode must NOT emit shiki vars: {html}"
        );
    }

    /// The class prefix threads through to BOTH the `<pre>` root class and the
    /// per-token role classes.
    #[test]
    fn class_mode_honours_custom_prefix() {
        let mut plugin = class_plugin("tok-");
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("rust"), "fn main() {}\n")],
        };
        plugin.visit(&mut tree);
        let HastNode::Root { children } = &tree else {
            unreachable!("expected Root")
        };
        let html = assert_class_structure(&children[0], "tok-");
        assert!(
            html.contains("class=\"tok-root\""),
            "custom prefix on pre: {html}"
        );
        assert!(
            html.contains("class=\"tok-kw\""),
            "custom prefix on tokens: {html}"
        );
    }

    /// A configured `roleClasses` override reaches the emitted span verbatim.
    #[test]
    fn class_mode_plugin_applies_role_class_override() {
        let mut roles = BTreeMap::new();
        // Keyed by the FULL role name (`"keyword"`), matching user config.
        roles.insert("keyword".to_string(), "text-violet-600".to_string());
        let mut plugin =
            SyntectPlugin::new(Arc::new(Highlighter::new())).with_class_mode("hi-", roles);
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("rust"), "fn main() {}\n")],
        };
        plugin.visit(&mut tree);
        use crate::serializer::serialize;
        let HastNode::Root { children } = &tree else {
            unreachable!("expected Root")
        };
        let html = serialize(&children[0]);
        assert!(
            html.contains("class=\"text-violet-600\""),
            "override applied: {html}"
        );
        assert!(
            !html.contains("class=\"hi-kw\""),
            "overridden role must not use the default class: {html}"
        );
    }

    /// A 3-line block produces 3 `<span class="line">` Element nodes (same
    /// contract as single/dual mode).
    #[test]
    fn class_mode_three_line_block_produces_three_span_lines() {
        let mut plugin = class_plugin("hi-");
        let code = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("rust"), code)],
        };
        plugin.visit(&mut tree);
        let HastNode::Root { children } = &tree else {
            unreachable!("expected Root")
        };
        let HastNode::Element {
            children: pre_children,
            ..
        } = &children[0]
        else {
            panic!("expected Element<pre>, got {:?}", children[0])
        };
        let HastNode::Element {
            tag: code_tag,
            children: code_children,
            ..
        } = pre_children.first().expect("pre must have <code>")
        else {
            panic!("expected Element<code>")
        };
        assert_eq!(code_tag, "code");
        assert_eq!(
            code_children.len(),
            3,
            "3 source lines → 3 line spans, got {}",
            code_children.len()
        );
        assert_class_structure(&children[0], "hi-");
    }

    /// Class mode skips mermaid blocks (same as single/dual mode).
    #[test]
    fn class_mode_skips_mermaid_block() {
        let mut plugin = class_plugin("hi-");
        let mut tree = HastNode::Root {
            children: vec![pre_code(Some("mermaid"), "graph TD;")],
        };
        let original = tree.clone();
        plugin.visit(&mut tree);
        assert_eq!(tree, original, "class mode must skip mermaid blocks");
    }

    /// Class mode skips blocks with no `data-lang` (same as single/dual mode).
    #[test]
    fn class_mode_skips_block_without_data_lang() {
        let mut plugin = class_plugin("hi-");
        let mut tree = HastNode::Root {
            children: vec![pre_code(None, "anything")],
        };
        let original = tree.clone();
        plugin.visit(&mut tree);
        assert_eq!(
            tree, original,
            "class mode must skip blocks without data-lang"
        );
    }
}
