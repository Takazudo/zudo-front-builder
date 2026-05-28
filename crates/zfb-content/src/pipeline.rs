//! mdast → hast pipeline + visitor framework.
//!
//! This module implements the markdown/MDX content pipeline used by zfb:
//!
//! 1. parse input → [`markdown::mdast::Node`] (mdast tree) using
//!    [`markdown::to_mdast`] with MDX-aware [`markdown::ParseOptions`] by
//!    default.
//! 2. run user-supplied [`MdastVisitor`]s over the mdast tree (mutation).
//! 3. transform the mutated mdast into [`HastNode`] via
//!    [`mdast_to_hast`] — a lightweight HTML AST defined in this module.
//! 4. run user-supplied [`HastVisitor`]s over the hast tree (mutation).
//!
//! Hast-to-HTML serialization is intentionally NOT implemented here; that
//! is the responsibility of the `serializer` module (Sub 6).
//!
//! markdown-rs (the `markdown` crate, v1.0) does not expose hast directly;
//! it parses to mdast and renders to HTML internally. To give downstream
//! plugins (Sub 4) a stable per-element hook point, we define our own
//! minimal hast representation here. This mirrors the
//! `remark` (mdast) → `rehype` (hast) split in the unified ecosystem.

use std::path::Path;
use std::sync::Arc;

use markdown::mdast::{AttributeContent, AttributeValue, Node as MdastNode};

use crate::plugins::{
    AdmonitionsPlugin, BrokenLinkDiagnostic, CjkFriendlyPlugin, CodeTitlePlugin,
    ExternalLinksConfig, ExternalLinksPlugin, HeadingLinksPlugin, ImageEnlargePlugin,
    MermaidPlugin, ResolveLinksPlugin, ResolveMarkdownLinksOptions, StripMdExtensionPlugin,
    SyntectPlugin, TocConfig, TocPlugin,
};
use crate::syntect_highlight::Highlighter;

/// Resolved per-construct GFM flags.
///
/// Output of `zfb::config::MarkdownConfig::resolve_constructs` (and the
/// matching `resolve_gfm_constructs` free function). Threaded into
/// every site that builds [`markdown::ParseOptions`] so the snapshot
/// walker, bundler, and dev loader stay in lockstep on the parser
/// constructs — divergence here is the
/// `content_bridge.rs:118-153` land mine (snapshot ↔ bundler
/// `content_hash` divergence → `<pre data-zfb-content-fallback>`).
///
/// Defined here in `zfb-content` (the lowest crate that actually
/// touches `markdown::Constructs`) so consumers can wire it into the
/// pipeline without an upward dependency on `zfb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedGfmConstructs {
    /// GFM strikethrough (`~~text~~`).
    pub strikethrough: bool,
    /// GFM pipe-style tables.
    pub table: bool,
    /// GFM autolink literal (bare URLs).
    pub autolink_literal: bool,
    /// GFM task list items (`- [x]` / `- [ ]`).
    pub task_list_item: bool,
    /// GFM footnote definitions (`[^ref]: …`).
    pub footnote_definition: bool,
}

impl ResolvedGfmConstructs {
    /// Conservative default — strikethrough + table on, every other
    /// GFM construct off. See
    /// `zfb::config::ResolvedGfmConstructs::CONSERVATIVE` for the full
    /// rationale; both constants must stay in sync.
    pub const CONSERVATIVE: Self = Self {
        strikethrough: true,
        table: true,
        autolink_literal: false,
        task_list_item: false,
        footnote_definition: false,
    };

    /// Every GFM construct ON.
    pub const ALL_ON: Self = Self {
        strikethrough: true,
        table: true,
        autolink_literal: true,
        task_list_item: true,
        footnote_definition: true,
    };

    /// Every GFM construct OFF.
    pub const ALL_OFF: Self = Self {
        strikethrough: false,
        table: false,
        autolink_literal: false,
        task_list_item: false,
        footnote_definition: false,
    };
}

impl Default for ResolvedGfmConstructs {
    /// `Default` is the conservative default — the only `Default` that
    /// makes sense without further context.
    fn default() -> Self {
        Self::CONSERVATIVE
    }
}

/// Build `markdown::Constructs` for the HTML-serializer / collection
/// walker pipeline (`Pipeline::run`) from a resolved GFM flag set.
///
/// Math constructs are deliberately left at their `Constructs::mdx()`
/// default values here. The HTML serializer path treats math nodes as
/// passthrough; enabling them here would not change the serializer
/// output. The JSX-emit path enables `math_flow` / `math_text`
/// separately, where the JSX emitter has dedicated arms for them.
#[must_use]
pub fn constructs_for_pipeline(
    resolved: ResolvedGfmConstructs,
) -> markdown::Constructs {
    markdown::Constructs {
        gfm_strikethrough: resolved.strikethrough,
        gfm_table: resolved.table,
        gfm_autolink_literal: resolved.autolink_literal,
        gfm_task_list_item: resolved.task_list_item,
        gfm_footnote_definition: resolved.footnote_definition,
        // `gfm_label_start_footnote` is the inline-side of footnotes
        // (`[^ref]` reference markers); markdown-rs treats it as a
        // pair with `gfm_footnote_definition`, so we mirror the flag.
        gfm_label_start_footnote: resolved.footnote_definition,
        ..markdown::Constructs::mdx()
    }
}

/// Same as [`constructs_for_pipeline`] but additionally turns on
/// `math_flow` + `math_text`. Used at the JSX emit site so `$$…$$`
/// and `$…$` parse into dedicated `Math` / `InlineMath` mdast nodes
/// (zfb#93).
#[must_use]
pub fn constructs_for_jsx_emit(
    resolved: ResolvedGfmConstructs,
) -> markdown::Constructs {
    markdown::Constructs {
        math_flow: true,
        math_text: true,
        ..constructs_for_pipeline(resolved)
    }
}


// HastNode, MdastVisitor, HastVisitor, and BuildContext live in
// `zfb-md-ast` so downstream plugin crates (zfb-md-extras) can depend on
// the visitor contract without depending on zfb-content. The
// `diagnostics` and `heading_registry` sub-modules also moved there
// because BuildContext references them via the visitor contract.
//
// Re-exported here under their historical paths so existing consumers of
// `zfb_content::pipeline::{HastNode, HastVisitor, ...}` continue to
// resolve.
pub use zfb_md_ast::{BuildContext, HastNode, HastVisitor, MdastVisitor};

/// Pipeline error type.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// markdown-rs failed to parse the input.
    #[error("markdown parse error: {0}")]
    Parse(String),
}

/// Pipeline configuration: the chain of mdast + hast visitors and the
/// markdown-rs parse options used to produce the initial mdast tree.
pub struct Pipeline {
    mdast_visitors: Vec<Box<dyn MdastVisitor>>,
    hast_visitors: Vec<Box<dyn HastVisitor>>,
    parse_options: markdown::ParseOptions,
    /// Resolved GFM constructs the pipeline was constructed with.
    /// Stored separately from `parse_options.constructs` so the JSX
    /// emit path can read back the same flags and build its own
    /// `ParseOptions` (which also enables math constructs) without
    /// having to round-trip through the markdown-rs `Constructs`
    /// struct field-by-field. The two MUST stay in sync; the
    /// constructors enforce this by building both from one
    /// `ResolvedGfmConstructs` input.
    gfm_constructs: ResolvedGfmConstructs,
    /// When the `StripMdExtensionPlugin` is wired into the pipeline,
    /// this flag controls whether the plugin appends `/` to internal
    /// hrefs after stripping `.md`/`.mdx` (and to any extensionless
    /// relative href that lacks one). Defaults to `true` to match the
    /// JS engine and converge URL shape with `ResolveLinksPlugin`.
    add_trailing_slash: bool,
    /// Optional `ResolveLinksPlugin` wired by the orchestrator when the
    /// consumer enabled `resolveMarkdownLinks` in `zfb.config.ts`.
    ///
    /// Stored as a named field (not a generic boxed visitor) so the
    /// orchestrator can call `set_resolve_links_source_dir` per-file
    /// and `take_broken_links` to drain diagnostics. Applied in
    /// `apply_mdast_visitors` AFTER all generic mdast visitors (i.e.
    /// after `AdmonitionsPlugin`) so link rewriting sees finalized
    /// mdast link nodes.
    resolve_links: Option<ResolveLinksPlugin>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    /// New pipeline with MDX-aware parsing (the project default).
    ///
    /// Equivalent to [`Pipeline::with_mdx`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_mdx()
    }

    /// New pipeline using MDX-aware [`markdown::ParseOptions`].
    ///
    /// GFM construct flags follow the conservative default
    /// (`gfm_strikethrough` + `gfm_table` on, every other GFM construct
    /// off) — matching `ResolvedGfmConstructs::CONSERVATIVE` in
    /// `crates/zfb/src/config.rs`. Math constructs (`math_flow`,
    /// `math_text`) stay OFF here because [`Pipeline::run`] consumes
    /// HTML output and the HTML serializer treats math nodes as
    /// passthrough; the JSX-emit path enables them separately at
    /// `mdx_jsx_emit::mdx_to_jsx_module_inner`. Callers wiring this
    /// pipeline from a project config should prefer
    /// [`Pipeline::with_resolved_gfm_constructs`] so the user's
    /// `markdown.gfm` setting flows through.
    #[must_use]
    pub fn with_mdx() -> Self {
        Self::with_resolved_gfm_constructs(ResolvedGfmConstructs::CONSERVATIVE)
    }

    /// New pipeline whose [`markdown::ParseOptions::constructs`] is
    /// built from a project-supplied [`ResolvedGfmConstructs`] (see
    /// `zfb::config::resolve_gfm_constructs`).
    ///
    /// The bundler + dev loader + snapshot bridge all funnel through
    /// this entry point so every site that materialises MDX content
    /// agrees on the same parser constructs — which is what keeps the
    /// snapshot ↔ bundler JSX `content_hash` byte-identical. The
    /// land-mine comment at
    /// `crates/zfb-content/src/content_bridge.rs:118-153` calls this
    /// out explicitly.
    ///
    /// `gfm_label_start_footnote` is mirrored to the resolved
    /// `footnote_definition` flag because the two constructs are
    /// paired in markdown-rs: enabling the definition without the
    /// label-start means the parser sees `[^a]: body` but never the
    /// `[^a]` reference at the use site.
    #[must_use]
    pub fn with_resolved_gfm_constructs(
        resolved: ResolvedGfmConstructs,
    ) -> Self {
        let constructs = constructs_for_pipeline(resolved);
        Self {
            mdast_visitors: Vec::new(),
            hast_visitors: Vec::new(),
            parse_options: markdown::ParseOptions {
                constructs,
                ..markdown::ParseOptions::mdx()
            },
            gfm_constructs: resolved,
            add_trailing_slash: true,
            resolve_links: None,
        }
    }

    /// Borrow the resolved GFM construct set this pipeline was built
    /// with.
    ///
    /// The JSX-emit detour reads this to build its own
    /// `ParseOptions` (which also enables math constructs) so the two
    /// parse paths agree on every non-math construct. Snapshot ↔
    /// bundler hash parity depends on this agreement.
    #[must_use]
    pub fn gfm_constructs(&self) -> ResolvedGfmConstructs {
        self.gfm_constructs
    }

    /// Set the `add_trailing_slash` option. Affects subsequent
    /// `add_strip_md_ext()` calls. Defaults to `true`.
    pub fn set_add_trailing_slash(&mut self, value: bool) -> &mut Self {
        self.add_trailing_slash = value;
        self
    }

    /// Append a [`StripMdExtensionPlugin`] configured by the pipeline's
    /// current `add_trailing_slash` setting (defaults to `true`).
    pub fn add_strip_md_ext(&mut self) -> &mut Self {
        let plugin = if self.add_trailing_slash {
            StripMdExtensionPlugin::with_trailing_slash()
        } else {
            StripMdExtensionPlugin::new()
        };
        self.add_hast_visitor(Box::new(plugin));
        self
    }

    /// Append an [`ExternalLinksPlugin`] to the pipeline's hast phase.
    ///
    /// External links (absolute HTTP/HTTPS hrefs whose origin differs from
    /// `site`, or any absolute HTTP/HTTPS href when `site` is absent) will
    /// receive `target` and `rel` attributes as specified by `config`.
    ///
    /// Not in [`Pipeline::with_defaults`] because the feature is opt-in via
    /// `markdown.externalLinks` in `zfb.config.ts`. Absent config flag →
    /// visitor not registered, output identical to today.
    pub fn add_external_links(
        &mut self,
        config: ExternalLinksConfig,
        site: Option<&str>,
    ) -> &mut Self {
        self.add_hast_visitor(Box::new(ExternalLinksPlugin::new(config, site)));
        self
    }

    /// Wire a [`ResolveLinksPlugin`] into the pipeline's mdast phase.
    ///
    /// The plugin is applied before the generic mdast visitors so it
    /// runs on the raw mdast before `AdmonitionsPlugin` transforms
    /// directives. The `source_dir` slot is empty until the caller
    /// calls [`Pipeline::set_resolve_links_source_dir`] per file.
    ///
    /// Call at most once per pipeline instance — a second call
    /// replaces the previous plugin.
    pub fn add_resolve_links(
        &mut self,
        source_map: std::collections::HashMap<std::path::PathBuf, String>,
    ) -> &mut Self {
        self.resolve_links = Some(ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map,
            source_dir: None,
        }));
        self
    }

    /// Update the per-file source directory used by the wired
    /// [`ResolveLinksPlugin`] to resolve relative link targets.
    ///
    /// Call once per MDX file, before `apply_mdast_visitors`, so
    /// `./other.mdx` links are resolved against the correct directory.
    /// No-op when [`add_resolve_links`](Pipeline::add_resolve_links)
    /// was not called.
    pub fn set_resolve_links_source_dir(&mut self, dir: std::path::PathBuf) {
        if let Some(p) = self.resolve_links.as_mut() {
            p.set_source_dir(dir);
        }
    }

    /// Append a [`TocPlugin`] to the hast phase.
    ///
    /// The TOC visitor runs **after** [`HeadingLinksPlugin`] (which is always
    /// first in the hast chain) so it can read the final deduplicated `id`
    /// attributes that plugin placed on each `<h2>`–`<h6>`. Callers should
    /// invoke this after [`Pipeline::with_defaults_and_theme_and_gfm`] — the
    /// insertion order is preserved, so TOC ends up scheduled after
    /// heading-links but before code-title and syntect.
    ///
    /// Not in `with_defaults()` because the feature is opt-in: absence of
    /// `markdown.toc` in `zfb.config.ts` must leave the build byte-for-byte
    /// identical.
    pub fn add_toc(&mut self, cfg: TocConfig) -> &mut Self {
        // Insert at position 1 in the hast visitors list so TOC runs
        // immediately after HeadingLinksPlugin (index 0) and before all
        // subsequent hast visitors. This guarantees ids are already set.
        //
        // If the list is empty (e.g. in a bare pipeline built without
        // with_defaults), append normally so the visitor still runs.
        let toc = Box::new(TocPlugin::new(cfg)) as Box<dyn HastVisitor>;
        if self.hast_visitors.is_empty() {
            self.hast_visitors.push(toc);
        } else {
            self.hast_visitors.insert(1, toc);
        }
        self
    }

    /// Drain broken-link diagnostics from the wired [`ResolveLinksPlugin`].
    ///
    /// Returns diagnostics accumulated since the last call (or since the
    /// plugin was wired). Returns an empty vec when no plugin is wired.
    /// Mirrors `DirectiveRegistry::take_diagnostics`.
    pub fn take_broken_links(&mut self) -> Vec<BrokenLinkDiagnostic> {
        self.resolve_links
            .as_mut()
            .map(|p| p.take_broken_links())
            .unwrap_or_default()
    }

    /// New pipeline preloaded with the project's default plugin chain.
    ///
    /// This is the entry point most orchestrator call sites want: it
    /// bundles the directive registry (via [`AdmonitionsPlugin`]) plus
    /// the five custom hast plugins so a `:::note` block compiles to
    /// `<Note>…</Note>`, headings get permalink anchors, titled code
    /// blocks get a `<div class="code-block-container">` wrapper plus
    /// syntect highlighting, mermaid blocks become
    /// `<div class="mermaid">` containers, and block-level paragraph
    /// images get wrapped in an enlargeable `<figure>` — all without
    /// manual plugin wiring at the call site.
    ///
    /// Callers that need a different mix should construct a pipeline
    /// via [`Pipeline::with_mdx`] (or [`Pipeline::new`]) and append
    /// only the visitors they want.
    ///
    /// # Visitor order
    ///
    /// The pipeline runs in two distinct phases — mdast (markdown AST,
    /// pre-HTML) then hast (HTML AST, post-conversion). Each plugin is
    /// registered in the phase that best matches the rewrite it does:
    ///
    /// **mdast phase** (run first, against the parsed markdown tree):
    ///
    /// 1. [`CjkFriendlyPlugin`] — re-tokenises emphasis/strong markers
    ///    around CJK characters that base CommonMark flanking rules
    ///    rejected. Runs before any visitor that depends on emphasis
    ///    being already tokenised.
    /// 2. [`AdmonitionsPlugin`] — directive-style transforms run on
    ///    mdast because [`DirectiveRegistry`] folds runs of paragraphs
    ///    delimited by `:::name` … `:::` into a single
    ///    [`MdxJsxFlowElement`]. That collapsing has to happen before
    ///    the mdast→hast conversion, or each `:::` line would already
    ///    be its own `<p>` element and the collapse would have to walk
    ///    arbitrary HTML structure to recover the run.
    ///
    /// **hast phase** (run after mdast→hast conversion, in this order):
    ///
    /// 3. [`HeadingLinksPlugin`] — adds `id` + permalink anchor to
    ///    `<h2>`–`<h6>`. Runs first in the hast phase so subsequent
    ///    plugins that might rewrite headings (none today, but the
    ///    door is open) see the slugified ids.
    /// 4. [`CodeTitlePlugin`] — wraps `<pre>` with a titled `data-meta`
    ///    in `<div class="code-block-container">` +
    ///    `<div class="code-block-title">`. Must run BEFORE
    ///    [`SyntectPlugin`] because syntect replaces the `<pre>` element
    ///    with structured HAST (`<pre><code><span class="line">…</span>
    ///    </code></pre>`); once that happens, the original `data-meta`
    ///    attribute on the input `<code>` is no longer reachable.
    /// 5. [`ImageEnlargePlugin`] — wraps any `<p>` whose only
    ///    non-whitespace child is `<img>` in
    ///    `<figure class="zd-enlargeable">` + an enlarge `<button>`.
    ///    Order-independent relative to syntect/mermaid (it only
    ///    touches `<p>`/`<img>` shapes).
    /// 6. [`MermaidPlugin`] — replaces `<pre><code class="language-mermaid">`
    ///    blocks with `<div class="mermaid" data-mermaid>…</div>`.
    ///    Must run BEFORE [`SyntectPlugin`] so the latter can identify
    ///    and skip mermaid blocks rather than syntect-highlighting them.
    /// 7. [`SyntectPlugin`] — replaces remaining fenced code blocks
    ///    with per-line structured HAST. Runs last among CORE hast
    ///    visitors so the title-wrapper and mermaid-skip decision are
    ///    already baked in. Extras-side enrichment visitors (registered
    ///    via `register_features`) run AFTER syntect on the per-line
    ///    `<span class="line">` structure they expose.
    ///
    /// `ResolveLinksPlugin` and `StripMdExtensionPlugin` are NOT in
    /// the defaults: the former needs a project-specific path-to-URL
    /// `source_map` so the orchestrator constructs it explicitly, and
    /// the latter is opt-in for sites whose authors hand-write
    /// `[link](other.md)` style references.
    ///
    /// [`DirectiveRegistry`]: crate::plugins::DirectiveRegistry
    /// [`MdxJsxFlowElement`]: markdown::mdast::MdxJsxFlowElement
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::with_defaults_and_theme(None)
    }

    /// New pipeline preloaded with the default plugin chain plus an
    /// explicit GFM construct set.
    ///
    /// Forwards to [`Pipeline::with_defaults_and_theme_and_gfm`] with
    /// the default theme. Use when the project's `zfb.config.ts`
    /// configures `markdown.gfm` but not `codeHighlight.theme`.
    #[must_use]
    pub fn with_defaults_and_gfm(resolved: ResolvedGfmConstructs) -> Self {
        Self::with_defaults_and_theme_and_gfm(None, resolved)
    }

    /// New pipeline preloaded with the default plugin chain, optionally
    /// overriding the syntect highlight theme.
    ///
    /// When `theme` is `Some`, the [`SyntectPlugin`] is constructed via
    /// [`SyntectPlugin::with_theme`] so every fenced code block in this
    /// pipeline uses the named built-in syntect theme instead of the
    /// `Highlighter` default (`base16-ocean.dark`).
    ///
    /// Theme names are syntect's built-in set (e.g. `"InspiredGitHub"`,
    /// `"Solarized (light)"`). Shiki names like `"dracula"` are **not**
    /// part of the bundled set and will produce an `unknown theme` error
    /// at render time. See
    /// [`zfb_content::syntect_highlight::Highlighter::theme_names`] for
    /// the full list.
    ///
    /// `None` falls back to [`Pipeline::with_defaults`] behaviour
    /// (theme = `base16-ocean.dark`).
    #[must_use]
    pub fn with_defaults_and_theme(theme: Option<&str>) -> Self {
        Self::with_defaults_and_theme_and_gfm(theme, ResolvedGfmConstructs::CONSERVATIVE)
    }

    /// Default constructor + theme + resolved GFM construct set.
    /// CJK-friendly emphasis is always on (the conservative default).
    ///
    /// Use [`Pipeline::with_defaults_and_theme_and_gfm_and_cjk`] when
    /// the user has set `markdown.cjkFriendly: false` to disable the
    /// plugin. All other call sites use this form.
    ///
    /// The bundler, snapshot walker, and dev loader all funnel through
    /// this entry point so every site that materialises MDX content
    /// agrees on the same parser constructs — the snapshot ↔ bundler
    /// hash parity requirement called out in
    /// `crates/zfb-content/src/content_bridge.rs:118-153`.
    #[must_use]
    pub fn with_defaults_and_theme_and_gfm(
        theme: Option<&str>,
        resolved: ResolvedGfmConstructs,
    ) -> Self {
        Self::with_defaults_and_theme_and_gfm_and_cjk(theme, resolved, true)
    }

    /// Most-explicit constructor: default plugin chain + theme + GFM +
    /// CJK-friendly toggle.
    ///
    /// When `cjk_friendly` is `true` (the default for all other
    /// `with_defaults*` constructors), [`CjkFriendlyPlugin`] is
    /// prepended to the mdast phase so emphasis/strong markers adjacent
    /// to CJK characters are re-tokenised correctly. Set `false` to omit
    /// the plugin — useful when the author opts out via
    /// `markdown.cjkFriendly: false` in `zfb.config.ts`.
    ///
    /// All other callers should use [`Pipeline::with_defaults_and_theme_and_gfm`]
    /// (which hard-codes `cjk_friendly: true`) unless they need to
    /// honour the user-supplied `markdown.cjkFriendly` flag.
    #[must_use]
    pub fn with_defaults_and_theme_and_gfm_and_cjk(
        theme: Option<&str>,
        resolved: ResolvedGfmConstructs,
        cjk_friendly: bool,
    ) -> Self {
        // Infallible path: no themes_dir.  Any error from a
        // `themes_dir`-bearing call site is caught by the fallible variant.
        Self::build_defaults(theme, resolved, None, cjk_friendly)
            .expect("no themes_dir — cannot fail")
    }

    /// Like [`with_defaults_and_theme_and_gfm`] but also loads extra
    /// `.tmTheme` files from `themes_dir` before constructing the
    /// `SyntectPlugin`.
    ///
    /// Returns `Err` if the directory is missing, unreadable, or any
    /// `.tmTheme` file inside it fails to parse.  The error message
    /// includes the failing file's path so users get a clear diagnostic
    /// at build start.
    ///
    /// Call sites that don't use `themes_dir` stay on the infallible
    /// `with_defaults_and_theme_and_gfm` path.
    pub fn with_defaults_and_theme_and_gfm_and_themes_dir(
        theme: Option<&str>,
        resolved: ResolvedGfmConstructs,
        themes_dir: &Path,
        cjk_friendly: bool,
    ) -> Result<Self, crate::syntect_highlight::HighlightError> {
        Self::build_defaults(theme, resolved, Some(themes_dir), cjk_friendly)
    }

    /// Shared builder used by the infallible and fallible public constructors.
    fn build_defaults(
        theme: Option<&str>,
        resolved: ResolvedGfmConstructs,
        themes_dir: Option<&Path>,
        cjk_friendly: bool,
    ) -> Result<Self, crate::syntect_highlight::HighlightError> {
        let mut highlighter = Highlighter::new();
        if let Some(dir) = themes_dir {
            highlighter.load_themes_from_dir(dir)?;
        }
        let highlighter = Arc::new(highlighter);
        let mut p = Self::with_resolved_gfm_constructs(resolved);
        // mdast phase.
        if cjk_friendly {
            p.add_mdast_visitor(Box::new(CjkFriendlyPlugin::new()));
        }
        p.add_mdast_visitor(Box::new(AdmonitionsPlugin::new()));
        // hast phase — ordering rationale lives in the doc comment above.
        p.add_hast_visitor(Box::new(HeadingLinksPlugin::new()));
        p.add_hast_visitor(Box::new(CodeTitlePlugin::new()));
        p.add_hast_visitor(Box::new(ImageEnlargePlugin::new()));
        p.add_hast_visitor(Box::new(MermaidPlugin::new()));
        let syntect = if let Some(t) = theme {
            SyntectPlugin::new(highlighter).with_theme(t)
        } else {
            SyntectPlugin::new(highlighter)
        };
        p.add_hast_visitor(Box::new(syntect));
        Ok(p)
    }

    /// Sibling constructor: default plugin chain driven by a
    /// [`zfb_md_extras::MarkdownFeaturesConfig`] feature set.
    ///
    /// # Purpose
    ///
    /// This is the single entry point for feature-flag-driven pipeline
    /// construction. Callers pass a `MarkdownFeaturesConfig` (from
    /// `zfb-md-extras`) and this constructor wires the appropriate visitors
    /// into the pipeline. The existing `with_defaults*` constructors are
    /// **not touched** — this is a pure sibling, and all current call sites
    /// remain byte-for-byte unchanged.
    ///
    /// # Wave 2 behaviour (stub)
    ///
    /// In Wave 2, the full default plugin chain is always wired (identical to
    /// `with_defaults_and_theme_and_gfm_and_cjk`), and `register_features`
    /// is called for future extensibility but is a no-op. Wave 3.1 (#570)
    /// will move zfb-content's own framework-feature visitors
    /// (`ImageEnlargePlugin`, `MermaidPlugin`, `AdmonitionsPlugin`) into
    /// `zfb-md-extras` and make them conditional on the corresponding
    /// `features.*` flags.
    ///
    /// Because the default chain is always wired in Wave 2, output is
    /// **byte-identical** to `with_defaults()` for an empty `features` set,
    /// and the image-enlarge wrapper is always present regardless of the
    /// `features.image_enlarge` flag. Wave 3.1 introduces the conditionality.
    ///
    /// # Visitor ordering contract
    ///
    /// The contract below is documented HERE (Wave 2); Wave 4-6 implement it.
    /// Feature modules in `zfb-md-extras` that add their own visitors MUST
    /// respect this order:
    ///
    /// **mdast phase** (in order):
    /// 1. `CjkFriendlyPlugin` — must run before any visitor that depends on
    ///    emphasis/strong being correctly tokenised around CJK characters.
    /// 2. *(extras mdast visitors — added by Wave 4-6 per-feature rules)*
    /// 3. `AdmonitionsPlugin` — directive transforms must fold `:::name` runs
    ///    before mdast→hast conversion.
    ///
    /// **hast phase** (in order):
    /// 4. `HeadingLinksPlugin` — MUST be first in hast so subsequent plugins
    ///    see the final slugified ids.
    /// 5. `CodeTitlePlugin` — MUST run BEFORE `SyntectPlugin` (syntect
    ///    replaces the input `<pre>` element with structured HAST; once
    ///    that happens, the original `data-meta` attribute is no longer
    ///    reachable).
    /// 6. `MermaidPlugin` — MUST run BEFORE `SyntectPlugin` so syntect can
    ///    identify and skip mermaid blocks.
    /// 7. `SyntectPlugin` — runs last among CORE hast visitors in the
    ///    code-block chain; emits `<pre><code><span class="line">…</span>
    ///    </code></pre>` structured HAST so each line is a mutable Element.
    /// 8. *(extras hast visitors registered via `register_features` run
    ///    AFTER `SyntectPlugin` on the per-line `<span class="line">`
    ///    structure — e.g. wave-5 diff markers and line-highlighting)*
    #[must_use]
    pub fn with_defaults_and_features(
        features: &zfb_md_extras::MarkdownFeaturesConfig,
    ) -> Self {
        // Wave 3 (#570): build the framework chain WITHOUT the four opt-in
        // plugins (mermaid, image_enlarge, admonitions_preset,
        // heading_marker_toc). `register_features` adds them back conditionally
        // based on the `features.*` flags.
        //
        // Visitor ordering contract (see doc comment on this method):
        //   mdast: CjkFriendlyPlugin → [features mdast] → AdmonitionsPlugin
        //   hast:  HeadingLinksPlugin → CodeTitlePlugin → [Mermaid] →
        //          SyntectPlugin → [features hast post-syntect]
        let highlighter = Arc::new(Highlighter::new());
        let mut p = Self::with_resolved_gfm_constructs(ResolvedGfmConstructs::CONSERVATIVE);
        // mdast phase — CjkFriendlyPlugin always on (matches with_defaults).
        p.add_mdast_visitor(Box::new(CjkFriendlyPlugin::new()));
        // [features mdast visitors inserted by register_features here]
        // hast phase — HeadingLinksPlugin and CodeTitlePlugin are always on.
        p.add_hast_visitor(Box::new(HeadingLinksPlugin::new()));
        p.add_hast_visitor(Box::new(CodeTitlePlugin::new()));
        // `register_features` is the single call-path from zfb-content into
        // zfb-md-extras. It adds the opt-in visitors in the correct phase and
        // position (before SyntectPlugin for mermaid; after for post-syntect).
        register_features(&mut p, features);
        // SyntectPlugin MUST be added AFTER register_features so that all
        // pre-syntect extras visitors (mermaid, image_enlarge, etc.) run first.
        p.add_hast_visitor(Box::new(SyntectPlugin::new(highlighter)));
        // Post-syntect extras visitors: these operate on the per-line
        // <span class="line"> structure that SyntectPlugin emits.
        register_post_syntect_features(&mut p, features);
        p
    }

    /// Append an mdast visitor; visitors run in insertion order.
    pub fn add_mdast_visitor(&mut self, v: Box<dyn MdastVisitor>) -> &mut Self {
        self.mdast_visitors.push(v);
        self
    }

    /// Append a hast visitor; visitors run in insertion order.
    pub fn add_hast_visitor(&mut self, v: Box<dyn HastVisitor>) -> &mut Self {
        self.hast_visitors.push(v);
        self
    }

    /// Run only the mdast visitor chain against an externally-parsed
    /// mdast tree.
    ///
    /// Sub 46 (#46) added this seam so the JSX emit path
    /// (`mdx_jsx_emit::compile_mdx_to_jsx_module_cached`) can apply the
    /// pipeline's mdast visitors without going through full
    /// [`Pipeline::run`] (which would also build a hast tree the JSX
    /// emitter does not consume). Hast visitors stay untouched here —
    /// they are applied by [`Pipeline::run`] only.
    ///
    /// The optional `resolve_links` plugin (wired via
    /// [`Pipeline::add_resolve_links`]) is applied AFTER the generic
    /// mdast visitors (i.e. after `AdmonitionsPlugin`) so the source
    /// map lookup sees the final mdast link nodes.
    pub fn apply_mdast_visitors(&mut self, node: &mut MdastNode) {
        for v in &mut self.mdast_visitors {
            v.visit(node);
        }
        // Apply ResolveLinksPlugin last in the mdast phase (after
        // AdmonitionsPlugin) when wired. See field doc.
        if let Some(p) = self.resolve_links.as_mut() {
            p.visit(node);
        }
    }

    /// Run only the hast visitor chain against an externally-built
    /// hast tree.
    ///
    /// Mirror of [`Pipeline::apply_mdast_visitors`], added for #121 so
    /// the JSX emit path can detour through hast — `mdast → hast →
    /// hast visitors → JSX emit` — and pick up the project's hast-phase
    /// plugins (heading-links, code-title, image-enlarge, mermaid,
    /// syntect, optional strip-md-ext) on MDX content. The HTML
    /// serializer path keeps using [`Pipeline::run`] unchanged.
    pub fn apply_hast_visitors(&mut self, node: &mut HastNode) {
        for v in &mut self.hast_visitors {
            v.visit(node);
        }
    }

    /// Reset per-document state in every hast visitor.
    ///
    /// Call this **before processing each new document** when a single
    /// `Pipeline` instance is reused across multiple entries (e.g. the
    /// bundler's walk loop). Without this, stateful visitors such as
    /// [`HeadingLinksPlugin`] accumulate slug counters across files —
    /// the same heading text can resolve to `basic-usage` in one
    /// document and `basic-usage-7` in another, producing a different
    /// `content_hash` and breaking the `mdx://<collection>/<slug>#<hash>`
    /// bridge lookup (zfb#187).
    ///
    /// Stateless visitors (code-title, image-enlarge, mermaid, syntect,
    /// strip-md-ext) provide the default no-op implementation of
    /// [`HastVisitor::reset`], so calling this unconditionally is safe.
    ///
    /// [`HeadingLinksPlugin`]: crate::plugins::HeadingLinksPlugin
    pub fn reset_per_entry(&mut self) {
        for v in &mut self.hast_visitors {
            v.reset();
        }
    }

    /// Parse `input` to mdast, run mdast visitors, transform to hast, run
    /// hast visitors. Returns the resulting hast root.
    ///
    /// # Errors
    /// Returns [`PipelineError::Parse`] if markdown-rs rejects the input.
    pub fn run(&mut self, input: &str) -> Result<HastNode, PipelineError> {
        let mut mdast = markdown::to_mdast(input, &self.parse_options)
            .map_err(|m| PipelineError::Parse(m.to_string()))?;

        for v in &mut self.mdast_visitors {
            v.visit(&mut mdast);
        }

        let mut hast = mdast_to_hast(&mdast);

        for v in &mut self.hast_visitors {
            v.visit(&mut hast);
        }

        Ok(hast)
    }

    /// Like [`Pipeline::run`] but threads a [`BuildContext`] through the
    /// hast visitor chain.
    ///
    /// Visitors that opt in to wave-6 features (heading-ID registry,
    /// diagnostics sink) override [`HastVisitor::visit_with_context`] and
    /// read from `ctx`. All other visitors receive the same call as in
    /// [`run`] — the default `visit_with_context` implementation delegates
    /// to `visit` — so the output is **byte-identical** to `run` when no
    /// context-aware visitors are registered.
    ///
    /// # Errors
    /// Returns [`PipelineError::Parse`] if markdown-rs rejects the input.
    pub fn run_with_context(
        &mut self,
        input: &str,
        ctx: &mut BuildContext<'_>,
    ) -> Result<HastNode, PipelineError> {
        let mut mdast = markdown::to_mdast(input, &self.parse_options)
            .map_err(|m| PipelineError::Parse(m.to_string()))?;

        for v in &mut self.mdast_visitors {
            v.visit(&mut mdast);
        }
        if let Some(p) = self.resolve_links.as_mut() {
            p.visit(&mut mdast);
        }

        let mut hast = mdast_to_hast(&mdast);

        for v in &mut self.hast_visitors {
            v.visit_with_context(&mut hast, ctx);
        }

        Ok(hast)
    }

    /// Run only the hast visitor chain with build context against an
    /// externally-built hast tree.
    ///
    /// Parallel to [`Pipeline::apply_hast_visitors`] but threads the context
    /// through each visitor so wave-6 plugins can access the registry and
    /// diagnostics sink.
    pub fn apply_hast_visitors_with_context(
        &mut self,
        node: &mut HastNode,
        ctx: &mut BuildContext<'_>,
    ) {
        for v in &mut self.hast_visitors {
            v.visit_with_context(node, ctx);
        }
    }
}

/// Register feature visitors from `zfb-md-extras` into the pipeline.
///
/// This is the **single entry point** from `zfb-content` into `zfb-md-extras`.
/// No other call path should cross the crate boundary. Called exclusively from
/// [`Pipeline::with_defaults_and_features`].
///
/// # Wave 2 stub
///
/// In Wave 2 no `zfb-md-extras` feature modules export visitors yet — the
/// feature stub modules (`github_alerts`, `reading_time`, etc.) are empty.
/// This function is a no-op. Wave 4-6 will fill in each feature module and
/// call into them here, conditionally on the `features.*` flags.
///
/// # Ordering contract
///
/// When Wave 4-6 add visitors, they MUST be inserted at the correct phase:
/// - mdast visitors: after `CjkFriendlyPlugin` and BEFORE `AdmonitionsPlugin`.
/// - hast visitors: AFTER `SyntectPlugin` for visitors that depend on
///   syntect's per-line structure; BEFORE `SyntectPlugin` for anything that
///   rewrites `<pre>`/`<code>` shapes.
///
/// The caller (`with_defaults_and_features`) already wires the framework
/// visitors in the correct order before calling this function, so any visitors
/// appended here run after the framework chain by default. If a feature
/// visitor must be inserted before `SyntectPlugin`, use
/// `Pipeline::add_mdast_visitor` / `Pipeline::add_hast_visitor` with explicit
/// ordering (document it in the feature module's sub-issue).
pub fn register_features(
    p: &mut Pipeline,
    features: &zfb_md_extras::MarkdownFeaturesConfig,
) {
    // Wave 3 (#570): conditionally wire the four opt-in framework features.
    //
    // Ordering contract (MUST match the doc comment on with_defaults_and_features):
    //   mdast phase:
    //     - admonitions_preset — MUST run BEFORE the syntect / hast phase.
    //       Inserted here (after CjkFriendlyPlugin that was added in the caller).
    //   hast phase (all run BEFORE SyntectPlugin which is appended by the caller
    //   after register_features returns):
    //     - image_enlarge — runs before syntect; no ordering constraint vs.
    //       heading_links (heading_links was already added by the caller).
    //     - mermaid — MUST run BEFORE SyntectPlugin so syntect can skip mermaid
    //       blocks. The `data-mermaid` div shape is not a `<pre>`; syntect
    //       ignores it automatically once it has been replaced.
    //   heading_marker_toc is wired AFTER headings are slugified by
    //   HeadingLinksPlugin. Since HeadingLinksPlugin was added first in the
    //   caller's hast chain, any hast visitor appended here runs after it.

    use zfb_md_ast::{feature_enabled, heading_marker_toc_enabled};

    // ── mdast phase ────────────────────────────────────────────────────────
    // code_tabs MUST run BEFORE admonitions_preset and github_alerts so that
    // `:::code-group` opener paragraphs are consumed before the directive
    // registry or alert scanner inspects them. The CodeTabsPlugin looks for
    // the literal `:::code-group` opener and the closing `:::` separator so
    // it must see raw paragraph nodes, not already-rewritten JSX elements.
    if feature_enabled(&features.code_tabs) {
        p.add_mdast_visitor(Box::new(
            zfb_md_extras::code_tabs::CodeTabsPlugin::new(),
        ));
    }

    // github_alerts MUST run BEFORE admonitions_preset so both features can
    // coexist: alert blockquotes are rewritten to MdxJsxFlowElement first,
    // then the admonitions pass handles `:::directive` syntax separately.
    if feature_enabled(&features.github_alerts) {
        p.add_mdast_visitor(Box::new(
            zfb_md_extras::github_alerts::GithubAlertsPlugin::new(),
        ));
    }

    if feature_enabled(&features.reading_time) {
        p.add_mdast_visitor(Box::new(
            zfb_md_extras::reading_time::ReadingTimePlugin::new(),
        ));
    }

    // ruby runs in the mdast phase so it can scan raw text before mdast→hast.
    // Order-independent relative to github_alerts and admonitions_preset
    // (those operate on blockquote/directive shapes; ruby operates on Text).
    if feature_enabled(&features.ruby) {
        p.add_mdast_visitor(Box::new(zfb_md_extras::ruby::RubyPlugin::new()));
    }

    if feature_enabled(&features.admonitions_preset) {
        use crate::plugins::directives::DirectiveRegistry;
        use zfb_md_extras::admonitions_preset::default_admonition_directives;
        let mut registry = DirectiveRegistry::new();
        for def in default_admonition_directives() {
            registry.register(def);
        }
        p.add_mdast_visitor(registry.into_visitor());
    }

    // ── hast phase (all before SyntectPlugin) ──────────────────────────────
    // The gating helpers `feature_enabled` and `heading_marker_toc_enabled`
    // treat `Some(FeatureToggle::Bool(false))` as disabled — `is_some()`
    // alone would silently wire the plugin even when the user explicitly
    // turned it off.
    if heading_marker_toc_enabled(&features.heading_marker_toc) {
        let cfg = features
            .heading_marker_toc
            .as_ref()
            .map(zfb_md_ast::HeadingMarkerTocFeature::to_config)
            .unwrap_or_default();
        p.add_hast_visitor(Box::new(
            zfb_md_extras::heading_marker_toc::TocPlugin::new(cfg),
        ));
    }
    if feature_enabled(&features.image_enlarge) {
        p.add_hast_visitor(Box::new(
            zfb_md_extras::image_enlarge::ImageEnlargePlugin::new(),
        ));
    }
    if feature_enabled(&features.mermaid) {
        p.add_hast_visitor(Box::new(
            zfb_md_extras::mermaid::MermaidPlugin::new(),
        ));
    }
    // Wave 5 (#574): GitHub-style autolinks — #NNN, user/repo#NNN, SHA.
    // Uses Option<GithubAutolinksConfig> (not FeatureToggle), so gated with
    // is_some() + required `repo` field extraction.
    if let Some(cfg) = features.github_autolinks.as_ref() {
        if let Some(repo) = cfg.repo.as_ref() {
            p.add_hast_visitor(Box::new(
                zfb_md_extras::github_autolinks::GithubAutolinksPlugin::new(repo.clone()),
            ));
        }
    }

    // Wave 5 (#578): toc_export — emit page TOC as MDX named export.
    // Gated on `is_some()` (the config type carries its own fields; no outer
    // `FeatureToggle` wrapper). Must run AFTER HeadingLinksPlugin (already in
    // the hast chain) so IDs are stable. Inserted before SyntectPlugin so the
    // export node lands at the front of the document root before code blocks
    // are transformed.
    if let Some(cfg) = &features.toc_export {
        p.add_hast_visitor(Box::new(
            zfb_md_extras::toc_export::TocExportPlugin::new(cfg.clone()),
        ));
    }

    // Wave 6 (#580): link_validation — validate internal links + anchor
    // fragments against the heading-ID registry. Runs VERY LATE in the hast
    // phase — after all heading-mutating visitors — so registry entries for the
    // current file are already populated by HeadingLinksPlugin. Gated on
    // `is_some()` (uses a rich options struct, not a FeatureToggle).
    if let Some(cfg) = &features.link_validation {
        p.add_hast_visitor(Box::new(
            zfb_md_extras::link_validation::LinkValidationPlugin::new(cfg.clone()),
        ));
    }
}

/// Register post-syntect feature visitors from `zfb-md-extras` into the pipeline.
///
/// Called **after** [`SyntectPlugin`] is added in
/// [`Pipeline::with_defaults_and_features`]. Visitors registered here operate
/// on the per-line `<span class="line">` structure that SyntectPlugin emits —
/// they see already-highlighted structured HAST.
///
/// # Wave 5 (#575)
///
/// `code_enrichment` is the first visitor registered here. It adds
/// `data-line-diff` and `data-line-highlight` attributes to matching
/// `<span class="line">` elements.
///
/// # Ordering contract
///
/// All visitors registered here run AFTER SyntectPlugin.
/// They MUST NOT rewrite the `<pre><code>` structure itself — only mutate
/// existing `<span class="line">` children.
pub fn register_post_syntect_features(
    p: &mut Pipeline,
    features: &zfb_md_extras::MarkdownFeaturesConfig,
) {
    // Wave 5 (#575): code_enrichment — diff markers + line highlighting.
    if let Some(cfg) = &features.code_enrichment {
        p.add_hast_visitor(Box::new(
            zfb_md_extras::code_enrichment::CodeEnrichmentPlugin::new(cfg.clone()),
        ));
    }
    // Wave 5-6: additional post-syntect feature visitor wiring lands here.
}

/// Strategy for emitting the JSX-shaped Raw payload of `MdxJsxFlow*`,
/// `MdxJsxText*`, `MdxFlow/TextExpression`, and `Math`/`InlineMath`
/// nodes during [`mdast_to_hast_with`].
///
/// The default strategy ([`JsxEmitStrategy::HtmlPath`]) preserves the
/// pre-#121 HTML serializer behaviour: `reconstruct_jsx` falls back
/// to `Node::to_string()` for non-text children (lossy in markdown
/// formatting, but stable for the project's HTML snapshots).
///
/// The JSX-aware strategy ([`JsxEmitStrategy::JsxPath`]) is used by
/// `mdx_jsx_emit::mdx_to_jsx_module_with_pipeline` to produce
/// recursively-rendered JSX so markdown formatting INSIDE an MDX JSX
/// body (`<Note>**bold**</Note>`) survives as `<strong>bold</strong>`.
/// Users supply this strategy via the closure on the variant.
pub enum JsxEmitStrategy<'a> {
    /// HTML-path preserving strategy. Same as the pre-#121 behaviour.
    HtmlPath,
    /// JSX-path strategy. The closure receives an mdast node and
    /// returns the JSX-shaped string the bridge should embed
    /// verbatim.
    JsxPath(&'a dyn Fn(&MdastNode) -> String),
}

/// Convert an mdast node into a hast node.
///
/// Convenience wrapper over [`mdast_to_hast_with`] using
/// [`JsxEmitStrategy::HtmlPath`] — i.e. the pre-#121 HTML-path
/// behaviour. Existing callers (the HTML serializer path,
/// `Pipeline::run`) keep their snapshot output unchanged.
///
/// See the module docs for the full coverage list. Unhandled node
/// types degrade to [`HastNode::Raw("".into())`] so the pipeline
/// never panics on novel input — Sub 4 / Sub 6 can extend handling
/// later.
#[must_use]
pub fn mdast_to_hast(node: &MdastNode) -> HastNode {
    mdast_to_hast_with(node, &JsxEmitStrategy::HtmlPath)
}

/// Convert an mdast node into a hast node, using the supplied strategy
/// for emitting the Raw / JsxRaw payload of MDX JSX, MDX expression,
/// and remark-math nodes.
///
/// Added for #121 so the JSX-emit detour can swap in a recursive
/// renderer for those arms without changing the HTML serializer
/// output.
#[must_use]
pub fn mdast_to_hast_with(node: &MdastNode, strategy: &JsxEmitStrategy<'_>) -> HastNode {
    match node {
        MdastNode::Root(r) => HastNode::Root {
            children: r
                .children
                .iter()
                .map(|c| mdast_to_hast_with(c, strategy))
                .collect(),
        },
        _ => mdast_to_hast_inner(node, strategy),
    }
}

fn mdast_to_hast_inner(node: &MdastNode, strategy: &JsxEmitStrategy<'_>) -> HastNode {
    match node {
        MdastNode::Root(r) => HastNode::Root {
            children: convert_children_with(&r.children, strategy),
        },
        MdastNode::Paragraph(p) => {
            element("p", vec![], convert_children_with(&p.children, strategy))
        }
        MdastNode::Heading(h) => {
            let depth = h.depth.clamp(1, 6);
            let tag = format!("h{depth}");
            element(&tag, vec![], convert_children_with(&h.children, strategy))
        }
        MdastNode::Text(t) => HastNode::Text(t.value.clone()),
        MdastNode::Emphasis(e) => {
            element("em", vec![], convert_children_with(&e.children, strategy))
        }
        MdastNode::Strong(s) => {
            element("strong", vec![], convert_children_with(&s.children, strategy))
        }
        MdastNode::Delete(d) => {
            element("del", vec![], convert_children_with(&d.children, strategy))
        }
        MdastNode::InlineCode(c) => element("code", vec![], vec![HastNode::Text(c.value.clone())]),
        MdastNode::Code(c) => {
            // Fenced code block. Wrap raw text in <pre><code>; expose
            // `lang` and `meta` as data-* attrs so Sub 4 plugins (e.g.
            // rehypeCodeTitle) and Sub 5 (syntect) can inspect them.
            let mut code_attrs: Vec<(String, String)> = Vec::new();
            if let Some(lang) = &c.lang {
                code_attrs.push(("class".to_string(), format!("language-{lang}")));
                code_attrs.push(("data-lang".to_string(), lang.clone()));
            }
            if let Some(meta) = &c.meta {
                code_attrs.push(("data-meta".to_string(), meta.clone()));
            }
            let code_el = HastNode::Element {
                tag: "code".to_string(),
                attrs: code_attrs,
                children: vec![HastNode::Text(c.value.clone())],
                void: false,
            };
            element("pre", vec![], vec![code_el])
        }
        MdastNode::Link(l) => {
            let mut attrs = vec![("href".to_string(), l.url.clone())];
            if let Some(title) = &l.title {
                attrs.push(("title".to_string(), title.clone()));
            }
            element("a", attrs, convert_children_with(&l.children, strategy))
        }
        MdastNode::Image(i) => {
            let mut attrs = vec![
                ("src".to_string(), i.url.clone()),
                ("alt".to_string(), i.alt.clone()),
            ];
            if let Some(title) = &i.title {
                attrs.push(("title".to_string(), title.clone()));
            }
            HastNode::Element {
                tag: "img".to_string(),
                attrs,
                children: vec![],
                void: true,
            }
        }
        MdastNode::List(l) => {
            let tag = if l.ordered { "ol" } else { "ul" };
            let mut attrs: Vec<(String, String)> = Vec::new();
            if l.ordered {
                if let Some(start) = l.start {
                    if start != 1 {
                        attrs.push(("start".to_string(), start.to_string()));
                    }
                }
            }
            element(tag, attrs, convert_children_with(&l.children, strategy))
        }
        MdastNode::ListItem(li) => {
            element("li", vec![], convert_children_with(&li.children, strategy))
        }
        MdastNode::Blockquote(b) => element(
            "blockquote",
            vec![],
            convert_children_with(&b.children, strategy),
        ),
        MdastNode::ThematicBreak(_) => HastNode::Element {
            tag: "hr".to_string(),
            attrs: vec![],
            children: vec![],
            void: true,
        },
        MdastNode::Break(_) => HastNode::Element {
            tag: "br".to_string(),
            attrs: vec![],
            children: vec![],
            void: true,
        },
        MdastNode::Html(h) => HastNode::Raw(h.value.clone()),
        // MDX JSX / expression nodes carry JSX-shaped source. Mark them
        // as `JsxRaw` so the JSX-emit bridge can inline them verbatim
        // into the output module — wrapping these in
        // `dangerouslySetInnerHTML` (the path `Raw` takes) would break
        // PascalCase component references and `{…}` expression
        // containers. The HTML serializer treats `JsxRaw` and `Raw`
        // identically (verbatim passthrough), so this distinction is
        // invisible on the HTML path. Strategy-aware: the JSX path
        // uses a recursive renderer so markdown formatting INSIDE the
        // JSX body survives.
        MdastNode::MdxJsxFlowElement(_)
        | MdastNode::MdxJsxTextElement(_)
        | MdastNode::MdxFlowExpression(_)
        | MdastNode::MdxTextExpression(_) => HastNode::JsxRaw(emit_jsx_raw(node, strategy)),
        // remark-math `$$...$$` block. Mirror the shape markdown-rs's
        // HTML serializer (`on_enter_raw_flow`) produces and what
        // `mdx_jsx_emit::JsxEmitter` emits on the no-pipeline path:
        // `<pre><code class="language-math math-display">…</code></pre>`.
        // Routing through `<pre>`/`<code>` keeps the JSX bridge able
        // to override both via `_components`, and matching the no-
        // pipeline path means the hast detour does not regress
        // pre-Sub-46 math handling. See zfb#93 / zfb#121.
        MdastNode::Math(m) => element(
            "pre",
            vec![],
            vec![HastNode::Element {
                tag: "code".to_string(),
                attrs: vec![(
                    "class".to_string(),
                    "language-math math-display".to_string(),
                )],
                children: vec![HastNode::Text(m.value.clone())],
                void: false,
            }],
        ),
        // remark-math `$...$` inline. Same shape as inline code with
        // an added `language-math math-inline` class.
        MdastNode::InlineMath(m) => HastNode::Element {
            tag: "code".to_string(),
            attrs: vec![(
                "class".to_string(),
                "language-math math-inline".to_string(),
            )],
            children: vec![HastNode::Text(m.value.clone())],
            void: false,
        },
        // GFM pipe-table → <table><thead>...</thead><tbody>...</tbody></table>
        // with per-column `style="text-align: ..."` on each th/td. Mirrors
        // emit_table_jsx in mdx_jsx_emit.rs for the no-pipeline path.
        MdastNode::Table(t) => {
            let align = &t.align;
            let style_attr = |col: usize| -> Option<(String, String)> {
                let kind = align.get(col).copied().unwrap_or(markdown::mdast::AlignKind::None);
                let s = match kind {
                    markdown::mdast::AlignKind::Left => Some("left"),
                    markdown::mdast::AlignKind::Right => Some("right"),
                    markdown::mdast::AlignKind::Center => Some("center"),
                    markdown::mdast::AlignKind::None => None,
                };
                s.map(|v| ("style".to_string(), format!("text-align: {v}")))
            };

            let row_to_cells = |row: &MdastNode, tag: &str| -> Vec<HastNode> {
                let MdastNode::TableRow(tr) = row else { return Vec::new(); };
                tr.children.iter().enumerate().filter_map(|(col, cell)| {
                    let MdastNode::TableCell(tc) = cell else { return None; };
                    let mut attrs: Vec<(String, String)> = Vec::new();
                    if let Some(s) = style_attr(col) { attrs.push(s); }
                    Some(element(tag, attrs, convert_children_with(&tc.children, strategy)))
                }).collect()
            };

            let mut thead_children: Vec<HastNode> = Vec::new();
            let mut tbody_children: Vec<HastNode> = Vec::new();

            if let Some((first, rest)) = t.children.split_first() {
                thead_children.push(element("tr", vec![], row_to_cells(first, "th")));
                for row in rest {
                    tbody_children.push(element("tr", vec![], row_to_cells(row, "td")));
                }
            }

            let mut table_children: Vec<HastNode> = Vec::new();
            if !thead_children.is_empty() {
                table_children.push(element("thead", vec![], thead_children));
            }
            if !tbody_children.is_empty() {
                table_children.push(element("tbody", vec![], tbody_children));
            }
            element("table", vec![], table_children)
        }
        // TableRow / TableCell are consumed by the Table arm above; if
        // they appear standalone (malformed input) emit nothing rather
        // than panicking.
        MdastNode::TableRow(_) | MdastNode::TableCell(_) => HastNode::Raw(String::new()),
        // Unhandled: degrade to empty Raw so we never crash on
        // unsupported input. Footnotes, definitions, reference
        // links/images, ESM, frontmatter, etc. fall here. They become
        // passthrough holes that Sub 4 plugins can later fill in.
        _ => HastNode::Raw(String::new()),
    }
}

/// Convert a slice of mdast children into a vec of hast children
/// using the given strategy for the JSX-shaped arms.
fn convert_children_with(
    children: &[MdastNode],
    strategy: &JsxEmitStrategy<'_>,
) -> Vec<HastNode> {
    children
        .iter()
        .map(|c| mdast_to_hast_inner(c, strategy))
        .collect()
}

/// Pick the right JSX-text producer for the supplied strategy and
/// invoke it. The HTML-path strategy uses the in-module
/// `reconstruct_jsx` (lossy fallback for non-text children, preserves
/// pre-#121 HTML snapshot output). The JSX-path strategy delegates to
/// the user-supplied closure (typically the recursive renderer in
/// `mdx_jsx_emit`).
fn emit_jsx_raw(node: &MdastNode, strategy: &JsxEmitStrategy<'_>) -> String {
    if let JsxEmitStrategy::JsxPath(emit) = strategy {
        return emit(node);
    }
    // HTML-path strategy: preserve pre-#121 behaviour exactly.
    match node {
        MdastNode::MdxJsxFlowElement(j) => {
            reconstruct_jsx(j.name.as_deref(), &j.attributes, &j.children)
        }
        MdastNode::MdxJsxTextElement(j) => {
            reconstruct_jsx(j.name.as_deref(), &j.attributes, &j.children)
        }
        MdastNode::MdxFlowExpression(e) => format!("{{{}}}", e.value),
        MdastNode::MdxTextExpression(e) => format!("{{{}}}", e.value),
        // Defensive: `mdast_to_hast_inner` only routes JSX-shaped arms
        // through this helper, so any other variant is unreachable
        // unless future arms are added.
        _ => String::new(),
    }
}

/// Build a non-void element.
fn element(tag: &str, attrs: Vec<(String, String)>, children: Vec<HastNode>) -> HastNode {
    HastNode::Element {
        tag: tag.to_string(),
        attrs,
        children,
        void: false,
    }
}

/// Best-effort textual reconstruction of an MDX JSX element.
///
/// We do NOT try to round-trip every MDX construct losslessly here; the
/// goal is to produce a plausible source-level snippet so the serializer
/// can pass it through verbatim. Sub 4 plugins that synthesize JSX
/// elements (e.g. `<Note>`) typically build the [`HastNode::Raw`]
/// payload themselves and bypass this path.
///
/// **HTML-path-only behaviour.** This helper feeds the HTML serializer
/// path (`Pipeline::run`); on the JSX-emit path (#121) the dedicated
/// `mdx_jsx_emit::reconstruct_jsx_recursive` is used instead so
/// markdown formatting inside MDX JSX bodies (`<Note>**bold**</Note>`)
/// survives as proper JSX elements. Updating this fallback to recurse
/// would change long-standing HTML snapshot output (admonition bodies
/// would gain `<p>` wrappers), which the issue brief explicitly
/// forbids ("Pipeline::run behaviour unchanged").
fn reconstruct_jsx(
    name: Option<&str>,
    attrs: &[AttributeContent],
    children: &[MdastNode],
) -> String {
    let tag = name.unwrap_or("");
    let attrs_str = render_attrs(attrs);
    let space = if attrs_str.is_empty() { "" } else { " " };

    if children.is_empty() {
        // Self-closing.
        return format!("<{tag}{space}{attrs_str} />");
    }

    let inner: String = children
        .iter()
        .map(|c| match c {
            MdastNode::Text(t) => t.value.clone(),
            MdastNode::Html(h) => h.value.clone(),
            MdastNode::MdxFlowExpression(e) => format!("{{{}}}", e.value),
            MdastNode::MdxTextExpression(e) => format!("{{{}}}", e.value),
            MdastNode::MdxJsxFlowElement(j) => {
                reconstruct_jsx(j.name.as_deref(), &j.attributes, &j.children)
            }
            MdastNode::MdxJsxTextElement(j) => {
                reconstruct_jsx(j.name.as_deref(), &j.attributes, &j.children)
            }
            // Fallback: stringify the markdown text content. This loses
            // formatting but keeps content visible; downstream plugins
            // generally avoid putting markdown inside JSX bodies anyway.
            other => other.to_string(),
        })
        .collect();

    format!("<{tag}{space}{attrs_str}>{inner}</{tag}>")
}

/// Render an MDX attribute list back to JSX-ish source text.
fn render_attrs(attrs: &[AttributeContent]) -> String {
    attrs
        .iter()
        .map(|a| match a {
            AttributeContent::Property(p) => match &p.value {
                None => p.name.clone(),
                Some(AttributeValue::Literal(s)) => format!("{}=\"{}\"", p.name, s),
                Some(AttributeValue::Expression(e)) => {
                    format!("{}={{{}}}", p.name, e.value)
                }
            },
            AttributeContent::Expression(e) => format!("{{...{}}}", e.value),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{Node as MdastNode, Text};

    fn run(input: &str) -> HastNode {
        Pipeline::new().run(input).expect("parse ok")
    }

    fn root_children(node: &HastNode) -> &[HastNode] {
        match node {
            HastNode::Root { children } => children,
            _ => unreachable!("expected Root, got {node:?}"),
        }
    }

    fn first_child(node: &HastNode) -> &HastNode {
        &root_children(node)[0]
    }

    fn assert_element<'a>(
        node: &'a HastNode,
        expected_tag: &str,
    ) -> (&'a [(String, String)], &'a [HastNode], bool) {
        match node {
            HastNode::Element {
                tag,
                attrs,
                children,
                void,
            } => {
                assert_eq!(tag, expected_tag, "tag mismatch in {node:?}");
                (attrs.as_slice(), children.as_slice(), *void)
            }
            _ => unreachable!("expected Element<{expected_tag}>, got {node:?}"),
        }
    }

    // 1. Empty input → empty Root.
    #[test]
    fn empty_input_yields_empty_root() {
        let h = run("");
        assert_eq!(h, HastNode::Root { children: vec![] });
    }

    // 2. Plain paragraph.
    #[test]
    fn plain_paragraph() {
        let h = run("hello world");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        assert_eq!(p_children, &[HastNode::Text("hello world".into())]);
    }

    // 3. Heading levels 1-6.
    #[test]
    fn heading_levels_1_through_6() {
        for depth in 1..=6 {
            let hashes = "#".repeat(depth);
            let input = format!("{hashes} title {depth}");
            let h = run(&input);
            let expected_tag = format!("h{depth}");
            let (_, children, _) = assert_element(first_child(&h), &expected_tag);
            assert_eq!(
                children,
                &[HastNode::Text(format!("title {depth}"))],
                "depth {depth}"
            );
        }
    }

    // 4. Bold/italic/strikethrough/inline-code.
    //
    // Strikethrough is GFM; with the conservative default
    // (`ResolvedGfmConstructs::CONSERVATIVE`) it is now ON by default,
    // so `~~x~~` parses into a `Delete` mdast node and the converter
    // maps it to `<del>`. We exercise both paths: the synthetic
    // `Delete` arm of `mdast_to_hast` AND the parse-driven path
    // through `Pipeline::run`.
    #[test]
    fn inline_formatting() {
        // *em* and **strong** and `code` work under MDX parse options.
        let h = run("*a* **b** `c`");
        let (_, p_children, _) = assert_element(first_child(&h), "p");

        let (_, em_children, _) = assert_element(&p_children[0], "em");
        assert_eq!(em_children, &[HastNode::Text("a".into())]);

        // p_children[1] is a Text(" ") between inline elements.
        let (_, strong_children, _) = assert_element(&p_children[2], "strong");
        assert_eq!(strong_children, &[HastNode::Text("b".into())]);

        let (_, code_children, _) = assert_element(&p_children[4], "code");
        assert_eq!(code_children, &[HastNode::Text("c".into())]);

        // Strikethrough — synthetic Delete node: the mdast→hast arm is
        // covered regardless of the parser construct flag.
        let del = MdastNode::Delete(markdown::mdast::Delete {
            children: vec![MdastNode::Text(Text {
                value: "gone".into(),
                position: None,
            })],
            position: None,
        });
        let hast = mdast_to_hast(&del);
        let (_, del_children, _) = assert_element(&hast, "del");
        assert_eq!(del_children, &[HastNode::Text("gone".into())]);
    }

    // 4b. Conservative-default parser turns `~~text~~` into a Delete
    // node. This is the missing-half of `inline_formatting` (the
    // pre-config-API test only exercised the synthetic-node path).
    // Sub-issue #61 of zudo-design-token-panel #60.
    #[test]
    fn strikethrough_parses_with_conservative_default() {
        let h = run("a ~~b~~ c");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        // The `<p>` body is: Text("a ") · <del>b</del> · Text(" c").
        // Find the `<del>` element regardless of how markdown-rs
        // splits the surrounding text — what matters is that one
        // exists.
        let del = p_children
            .iter()
            .find(|child| matches!(child, HastNode::Element { tag, .. } if tag == "del"))
            .expect("expected a <del> element in the parsed paragraph");
        let (_, del_children, _) = assert_element(del, "del");
        assert_eq!(del_children, &[HastNode::Text("b".into())]);
    }

    // 4c. With strikethrough explicitly OFF (`gfm: false` shape), the
    // parser leaves `~~text~~` as bare text — no `Delete` node.
    #[test]
    fn strikethrough_disabled_emits_no_delete_node() {
        let mut p = Pipeline::with_resolved_gfm_constructs(
            ResolvedGfmConstructs::ALL_OFF,
        );
        let h = p.run("a ~~b~~ c").expect("parse ok");
        // Walk the tree and assert there is no `<del>` anywhere — the
        // raw `~~` characters live inside a Text node instead.
        fn has_del(node: &HastNode) -> bool {
            match node {
                HastNode::Element { tag, children, .. } => {
                    if tag == "del" {
                        return true;
                    }
                    children.iter().any(has_del)
                }
                HastNode::Root { children } => children.iter().any(has_del),
                _ => false,
            }
        }
        assert!(!has_del(&h), "no <del> expected when strikethrough is off");
    }

    // 5. Fenced code block preserves lang and meta as attrs.
    #[test]
    fn fenced_code_preserves_lang_and_meta() {
        // markdown-rs accepts arbitrary text after the lang token as `meta`.
        let h = run("```rust title=\"main.rs\"\nfn main() {}\n```\n");
        let (_, pre_children, _) = assert_element(first_child(&h), "pre");
        let (code_attrs, code_children, _) = assert_element(&pre_children[0], "code");

        let attr_map: std::collections::HashMap<&str, &str> = code_attrs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(attr_map.get("class"), Some(&"language-rust"));
        assert_eq!(attr_map.get("data-lang"), Some(&"rust"));
        assert_eq!(attr_map.get("data-meta"), Some(&"title=\"main.rs\""));

        assert_eq!(code_children, &[HastNode::Text("fn main() {}".into())]);
    }

    // 6. Link with and without title.
    #[test]
    fn link_with_and_without_title() {
        let h = run("[a](https://example.com)");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        let (a_attrs, a_children, _) = assert_element(&p_children[0], "a");
        assert_eq!(
            a_attrs,
            &[("href".to_string(), "https://example.com".to_string())]
        );
        assert_eq!(a_children, &[HastNode::Text("a".into())]);

        let h = run("[a](https://example.com \"hi\")");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        let (a_attrs, _, _) = assert_element(&p_children[0], "a");
        assert_eq!(
            a_attrs,
            &[
                ("href".to_string(), "https://example.com".to_string()),
                ("title".to_string(), "hi".to_string()),
            ]
        );
    }

    // 7. Image (void).
    #[test]
    fn image_is_void_element() {
        let h = run("![alt text](pic.png)");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        let (img_attrs, img_children, void) = assert_element(&p_children[0], "img");
        assert!(void, "img must be a void element");
        assert!(img_children.is_empty());
        assert_eq!(
            img_attrs,
            &[
                ("src".to_string(), "pic.png".to_string()),
                ("alt".to_string(), "alt text".to_string()),
            ]
        );

        let h = run("![alt](pic.png \"caption\")");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        let (img_attrs, _, _) = assert_element(&p_children[0], "img");
        assert!(img_attrs.contains(&("title".to_string(), "caption".to_string())));
    }

    // 8. Ordered + unordered lists.
    #[test]
    fn ordered_and_unordered_lists() {
        let h = run("- a\n- b\n");
        let (_, ul_children, _) = assert_element(first_child(&h), "ul");
        assert_eq!(ul_children.len(), 2);
        let (_, li0, _) = assert_element(&ul_children[0], "li");
        // The list item wraps a paragraph.
        let (_, p0, _) = assert_element(&li0[0], "p");
        assert_eq!(p0, &[HastNode::Text("a".into())]);

        let h = run("1. one\n2. two\n");
        let (_, ol_children, _) = assert_element(first_child(&h), "ol");
        assert_eq!(ol_children.len(), 2);
    }

    // 9. Nested blockquote.
    #[test]
    fn nested_blockquote() {
        let h = run("> outer\n>\n> > inner\n");
        let (_, bq_children, _) = assert_element(first_child(&h), "blockquote");
        // outer has a paragraph then an inner blockquote.
        let mut found_inner = false;
        for c in bq_children {
            if let HastNode::Element { tag, .. } = c {
                if tag == "blockquote" {
                    found_inner = true;
                }
            }
        }
        assert!(
            found_inner,
            "expected nested <blockquote>, got {bq_children:?}"
        );
    }

    // 10. MDX JSX element passes through as JsxRaw.
    //
    // Walk the hast tree and collect every [`HastNode::JsxRaw`] /
    // [`HastNode::Raw`] payload — markdown-rs may parse JSX as either a
    // flow element (top-level) or a text element (inside a paragraph)
    // depending on surrounding whitespace. Either way the converter
    // must produce JsxRaw with the original-ish source so the
    // serializer passes it through.
    fn collect_raw(node: &HastNode, out: &mut Vec<String>) {
        match node {
            HastNode::Raw(s) | HastNode::JsxRaw(s) => out.push(s.clone()),
            HastNode::Root { children } => {
                for c in children {
                    collect_raw(c, out);
                }
            }
            HastNode::Element { children, .. } => {
                for c in children {
                    collect_raw(c, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn mdx_jsx_passes_through_as_raw() {
        let h = run("<Note>hello</Note>\n");
        let mut raws = Vec::new();
        collect_raw(&h, &mut raws);
        assert!(
            raws.iter()
                .any(|r| r.contains("<Note") && r.contains("hello") && r.contains("</Note>")),
            "expected a Raw containing the <Note>…</Note> source, got raws={raws:?} from {h:?}"
        );

        // Self-closing.
        let h = run("<Hr />\n");
        let mut raws = Vec::new();
        collect_raw(&h, &mut raws);
        assert!(
            raws.iter().any(|r| r.contains("<Hr") && r.contains("/>")),
            "expected a self-closing <Hr /> Raw, got raws={raws:?}"
        );
    }

    // 11. mdast visitor mutation runs.
    struct UppercaseText;
    impl MdastVisitor for UppercaseText {
        fn visit(&mut self, node: &mut MdastNode) {
            if let MdastNode::Text(t) = node {
                t.value = t.value.to_uppercase();
            }
            if let Some(children) = node.children_mut() {
                for c in children {
                    self.visit(c);
                }
            }
        }
    }

    #[test]
    fn mdast_visitor_mutation_runs() {
        let mut p = Pipeline::new();
        p.add_mdast_visitor(Box::new(UppercaseText));
        let h = p.run("hello world").expect("parse ok");
        let (_, p_children, _) = assert_element(first_child(&h), "p");
        assert_eq!(p_children, &[HastNode::Text("HELLO WORLD".into())]);
    }

    // 12. hast visitor mutation runs.
    struct AddTouchedClass;
    impl HastVisitor for AddTouchedClass {
        fn visit(&mut self, node: &mut HastNode) {
            if let HastNode::Element {
                attrs, children, ..
            } = node
            {
                attrs.push(("class".to_string(), "touched".to_string()));
                for c in children {
                    self.visit(c);
                }
            } else if let HastNode::Root { children } = node {
                for c in children {
                    self.visit(c);
                }
            }
        }
    }

    // 13. with_defaults wires the directive registry — `:::note`
    // becomes `<Note>…</Note>` without manual plugin wiring.
    #[test]
    fn with_defaults_wires_directive_registry() {
        let mut p = Pipeline::with_defaults();
        let h = p
            .run(":::note\n\nbody\n\n:::\n")
            .expect("pipeline runs ok");
        let mut raws = Vec::new();
        collect_raw(&h, &mut raws);
        assert!(
            raws.iter()
                .any(|r| r.contains("<Note") && r.contains("</Note>")),
            "expected a <Note>…</Note> Raw block, got raws={raws:?}",
        );
    }

    // 14. Pipeline::new() / with_mdx() stay plugin-free so callers can
    // opt out of the defaults.
    #[test]
    fn new_and_with_mdx_have_no_plugins() {
        // `:::note` should NOT collapse into a <Note> element when the
        // caller picks the no-plugins constructor — the paragraph runs
        // through to hast as plain `<p>:::note</p>` etc.
        for mut p in [Pipeline::new(), Pipeline::with_mdx()] {
            let h = p
                .run(":::note\n\nbody\n\n:::\n")
                .expect("pipeline runs ok");
            let mut raws = Vec::new();
            collect_raw(&h, &mut raws);
            assert!(
                !raws.iter().any(|r| r.contains("<Note")),
                "no-plugins pipeline must not synthesize <Note>; got raws={raws:?}",
            );
        }
    }

    // 15. The `add_trailing_slash` option is honoured both ways when
    // `StripMdExtensionPlugin` is wired via `add_strip_md_ext()`.
    #[test]
    fn add_trailing_slash_option_honoured_both_ways() {
        // Default (true) — JS-aligned shape with the slash.
        let mut p = Pipeline::with_mdx();
        p.add_strip_md_ext();
        let h = p.run("[x](./guide.md)").expect("ok");
        let html = crate::serializer::serialize(&h);
        assert!(
            html.contains("href=\"./guide/\""),
            "default add_trailing_slash=true should produce ./guide/, got: {html}",
        );

        // Off — legacy shape without the slash.
        let mut p = Pipeline::with_mdx();
        p.set_add_trailing_slash(false);
        p.add_strip_md_ext();
        let h = p.run("[x](./guide.md)").expect("ok");
        let html = crate::serializer::serialize(&h);
        assert!(
            html.contains("href=\"./guide\""),
            "add_trailing_slash=false should keep legacy shape, got: {html}",
        );
        assert!(
            !html.contains("href=\"./guide/\""),
            "add_trailing_slash=false must NOT add the trailing slash, got: {html}",
        );
    }

    #[test]
    fn hast_visitor_mutation_runs() {
        let mut p = Pipeline::new();
        p.add_hast_visitor(Box::new(AddTouchedClass));
        let h = p.run("# heading\n\nbody").expect("parse ok");
        let children = root_children(&h);
        for c in children {
            if let HastNode::Element { attrs, .. } = c {
                assert!(
                    attrs.contains(&("class".to_string(), "touched".to_string())),
                    "expected class=touched on {c:?}"
                );
            } else {
                unreachable!("expected element, got {c:?}");
            }
        }
    }

    // 16. with_defaults_and_theme — non-default theme produces different
    //     syntect class slug compared to the default pipeline.
    //
    // The default pipeline uses `base16-ocean.dark`, which yields
    // `class="syntect-base16-ocean-dark"` on the `<pre>` wrapper.
    // `InspiredGitHub` yields `class="syntect-inspiredgithub"`.
    // Asserting that both class slugs differ is sufficient to prove that
    // the theme is being threaded through end-to-end: the same Rust code
    // path, with two different built-in theme names, emits distinct HTML.
    #[test]
    fn with_defaults_and_theme_changes_highlight_class_slug() {
        let mdx = "```rust\nfn main() {}\n```\n";

        // Default pipeline — base16-ocean.dark.
        let mut default_pipeline = Pipeline::with_defaults();
        let default_hast = default_pipeline.run(mdx).expect("default pipeline ok");
        let default_html = crate::serializer::serialize(&default_hast);

        // Non-default theme pipeline — InspiredGitHub.
        let mut themed_pipeline =
            Pipeline::with_defaults_and_theme(Some("InspiredGitHub"));
        let themed_hast = themed_pipeline.run(mdx).expect("themed pipeline ok");
        let themed_html = crate::serializer::serialize(&themed_hast);

        // Both must emit highlighted output (not the plain fallback).
        assert!(
            default_html.contains("syntect-"),
            "default pipeline must emit syntect-highlighted HTML, got: {default_html}"
        );
        assert!(
            themed_html.contains("syntect-"),
            "themed pipeline must emit syntect-highlighted HTML, got: {themed_html}"
        );

        // The class slugs must differ — proving the theme was applied.
        assert!(
            default_html.contains("syntect-base16-ocean-dark"),
            "default pipeline must produce base16-ocean-dark slug, got: {default_html}"
        );
        assert!(
            themed_html.contains("syntect-inspiredgithub"),
            "InspiredGitHub pipeline must produce inspiredgithub slug, got: {themed_html}"
        );
        assert_ne!(
            default_html, themed_html,
            "different themes must produce different highlighted HTML"
        );
    }

    // 17. with_defaults() and with_defaults_and_theme(None) are identical.
    #[test]
    fn with_defaults_and_theme_none_matches_with_defaults() {
        let mdx = "```rust\nlet x = 1;\n```\n";
        let mut p1 = Pipeline::with_defaults();
        let h1 = p1.run(mdx).expect("ok");
        let html1 = crate::serializer::serialize(&h1);

        let mut p2 = Pipeline::with_defaults_and_theme(None);
        let h2 = p2.run(mdx).expect("ok");
        let html2 = crate::serializer::serialize(&h2);

        assert_eq!(
            html1, html2,
            "with_defaults() and with_defaults_and_theme(None) must produce identical output"
        );
    }
}
