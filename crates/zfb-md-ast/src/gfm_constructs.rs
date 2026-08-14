//! Resolved GFM construct flags and the `markdown::Constructs` builders
//! every parse site funnels through.
//!
//! Lives here — the shared AST/contract crate — rather than in
//! `zfb-content`, because `zfb-md-extras` parses markdown too (the
//! transclude plugin re-parses each included file) and must be able to
//! name the resolved flag set without an upward dependency on
//! `zfb-content`. `zfb-content` re-exports everything in this module, so
//! consumer paths such as `zfb_content::ResolvedGfmConstructs` and
//! `zfb::config::ResolvedGfmConstructs` are unaffected.

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
/// Threading this 5-bool type rather than a bare `markdown::Constructs`
/// is deliberate: the `gfm_footnote_definition` ↔
/// `gfm_label_start_footnote` pairing is encoded once, in
/// [`constructs_for_pipeline`]. A caller handing a raw `Constructs`
/// around can set the definition flag without its label-start partner
/// and get footnote definitions that parse with references that never
/// resolve — silently, with no compile error.
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
    /// Conservative default — strikethrough, table, and autolink
    /// literal on; task-list-item and footnote-definition off. See
    /// `zfb::config::ResolvedGfmConstructs::CONSERVATIVE` for the full
    /// rationale; both constants must stay in sync.
    pub const CONSERVATIVE: Self = Self {
        strikethrough: true,
        table: true,
        autolink_literal: true,
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
pub fn constructs_for_pipeline(resolved: ResolvedGfmConstructs) -> markdown::Constructs {
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
pub fn constructs_for_jsx_emit(resolved: ResolvedGfmConstructs) -> markdown::Constructs {
    markdown::Constructs {
        math_flow: true,
        math_text: true,
        ..constructs_for_pipeline(resolved)
    }
}
