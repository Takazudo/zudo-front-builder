//! The post-parse normalisation a *secondary* parse site owes its own
//! output, plus the placement rule that decides how much of it is safe.
//!
//! A secondary parse site is one that re-enters `markdown::to_mdast` from
//! *inside* the mdast visitor chain: `zfb_md_extras::transclude` (each
//! included file) and `zfb_content`'s collapsed-directive re-parse. The
//! pipeline normalises its OWN top-level parse ahead of the visitor
//! chain, so nothing downstream will ever normalise what these sites
//! produce — every `to_mdast` call site owns its own output.
//!
//! Both sites ran the identical four-pass sequence, in the identical
//! order, with the identical gating, in two crates (zfb#2402). It had
//! drifted-by-addition three times in three consecutive issues (#1105,
//! #2390, #2398), each time needing a matching edit in both. Lives here
//! for the same reason [`crate::constructs_for_target`] does: a rule
//! duplicated per call site is a rule that can drift, silently, the next
//! time a pass is threaded through.

use markdown::mdast::Node as MdastNode;

use crate::{
    unwrap_nested_links, CjkAutolinkBoundaryPlugin, CjkFriendlyPlugin, HardBreaksPlugin,
    MdastVisitor, ResolvedGfmConstructs, SecondaryParseTarget,
};

/// Where a secondary parse site's freshly parsed nodes land in the host
/// tree — the fact [`SecondaryParseNormalization`] needs that the parse
/// site itself cannot see.
///
/// Only [`HardBreaksPlugin`] cares. The gate's ORIGINAL justification
/// (zfb#2402) was that on the HTML path an `MdxJsxFlowElement` is
/// rendered by `mdast_to_hast`'s lossy `reconstruct_jsx` catch-all,
/// which stringified a `Break` to the EMPTY STRING — so inserting one
/// inside a JSX body DELETED the author's newline instead of turning it
/// into `<br>`. **That renderer defect is fixed** (zfb#2401:
/// `reconstruct_jsx` now routes a `Break`-carrying subtree through
/// `jsx_body_stringify`, which renders `<br />`), so the gate no longer
/// prevents content deletion.
///
/// It is kept deliberately, as a behaviour-preserving choice rather than
/// a correctness one: lifting it would start emitting `<br>` where a
/// JSX-nested include previously rendered the author's literal newline,
/// which is an intentional-output change out of scope for zfb#2401. The
/// resulting asymmetry is real and pinned by tests — a JSX-nested
/// include renders no `<br>` on the HTML path while an equivalent
/// top-level include does. Revisit alongside the open chain-ordering
/// question documented at the foot of `zfb-content`'s
/// `tests/gfm_secondary_parse_sites.rs`.
///
/// Blockquote and list-item containers render `Break` correctly on both
/// paths, so the gate is JSX-specific, not "anything nested".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryParsePlacement {
    /// The nodes splice in as ordinary top-level siblings (or inside a
    /// container both emit paths render structurally, such as a
    /// blockquote or list item).
    TopLevelSibling,
    /// The nodes land among the children of an `MdxJsxFlowElement` —
    /// either one the directive registry is building around them, or an
    /// author-written literal `<Note>…</Note>` the include walk descended
    /// into.
    InsideJsxElement,
}

impl SecondaryParsePlacement {
    /// Whether [`HardBreaksPlugin`] may run for output at this placement
    /// on `target` (zfb#2398, zfb#2402).
    ///
    /// The JSX emitter has a real arm for `Break`. The HTML path now
    /// renders one too (zfb#2401), so this is a behaviour-preserving
    /// gate rather than a deletion guard — see the enum's doc comment.
    #[must_use]
    pub fn allows_hard_breaks(self, target: SecondaryParseTarget) -> bool {
        match self {
            Self::TopLevelSibling => true,
            Self::InsideJsxElement => target == SecondaryParseTarget::Jsx,
        }
    }
}

/// The four post-parse passes a secondary parse site applies to a
/// freshly parsed subtree, with every gate already resolved.
///
/// Build one with [`SecondaryParseNormalization::new`] — that constructor
/// is the single place the placement rule is applied, so neither call
/// site can decide it differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecondaryParseNormalization {
    autolink_literal: bool,
    cjk_friendly: bool,
    hard_breaks: bool,
}

impl SecondaryParseNormalization {
    /// Resolve the project's settings and the output's placement into the
    /// passes that will actually run.
    ///
    /// `cjk_friendly` and `hard_breaks` are the project's raw
    /// `markdown.cjkFriendly` / `markdown.hardBreaks` settings; the
    /// autolink-literal gate is read off `gfm`, and `hard_breaks` is
    /// ANDed with [`SecondaryParsePlacement::allows_hard_breaks`] here.
    #[must_use]
    pub fn new(
        gfm: ResolvedGfmConstructs,
        cjk_friendly: bool,
        hard_breaks: bool,
        placement: SecondaryParsePlacement,
        target: SecondaryParseTarget,
    ) -> Self {
        Self {
            autolink_literal: gfm.autolink_literal,
            cjk_friendly,
            hard_breaks: hard_breaks && placement.allows_hard_breaks(target),
        }
    }

    /// Run the enabled passes over `node`, in the order the pipeline runs
    /// them on its own top-level parse.
    ///
    /// - [`unwrap_nested_links`] (zfb#2388) removes autolink literals
    ///   markdown-rs fires inside a link label, which would otherwise emit
    ///   an `<a>` nested in an `<a>` — invalid HTML that fails
    ///   `html-validate`'s `element-permitted-content`.
    /// - [`CjkAutolinkBoundaryPlugin`] (zfb#1105) stops a bare URL flush
    ///   against CJK text from swallowing the trailing CJK run into the
    ///   `href`.
    ///
    /// Both are gated on `autolink_literal`, the construct that produces
    /// the defects they remove, so a subtree parsed without it stays
    /// byte-identical.
    ///
    /// - [`CjkFriendlyPlugin`] and [`HardBreaksPlugin`] (zfb#2398) are
    ///   NOT part of that gate — they are independent normalisations,
    ///   gated only on their own project settings, and additionally (for
    ///   hard breaks) on placement.
    pub fn apply(&self, node: &mut MdastNode) {
        if self.autolink_literal {
            unwrap_nested_links(node);
            if self.cjk_friendly {
                CjkAutolinkBoundaryPlugin::new().visit(node);
            }
        }
        if self.cjk_friendly {
            CjkFriendlyPlugin::new().visit(node);
        }
        if self.hard_breaks {
            HardBreaksPlugin::new().visit(node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_placement_allows_hard_breaks_on_both_targets() {
        for target in [SecondaryParseTarget::Html, SecondaryParseTarget::Jsx] {
            assert!(SecondaryParsePlacement::TopLevelSibling.allows_hard_breaks(target));
        }
    }

    #[test]
    fn jsx_nested_placement_allows_hard_breaks_only_on_the_jsx_target() {
        assert!(!SecondaryParsePlacement::InsideJsxElement
            .allows_hard_breaks(SecondaryParseTarget::Html));
        assert!(
            SecondaryParsePlacement::InsideJsxElement.allows_hard_breaks(SecondaryParseTarget::Jsx)
        );
    }

    #[test]
    fn constructor_ands_the_raw_setting_with_the_placement_rule() {
        let gfm = ResolvedGfmConstructs::ALL_ON;
        let nested_html = SecondaryParseNormalization::new(
            gfm,
            false,
            true,
            SecondaryParsePlacement::InsideJsxElement,
            SecondaryParseTarget::Html,
        );
        assert!(!nested_html.hard_breaks);

        let nested_jsx = SecondaryParseNormalization::new(
            gfm,
            false,
            true,
            SecondaryParsePlacement::InsideJsxElement,
            SecondaryParseTarget::Jsx,
        );
        assert!(nested_jsx.hard_breaks);

        // The project setting still wins when it is off.
        let off = SecondaryParseNormalization::new(
            gfm,
            false,
            false,
            SecondaryParsePlacement::TopLevelSibling,
            SecondaryParseTarget::Jsx,
        );
        assert!(!off.hard_breaks);
    }
}
