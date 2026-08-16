//! Generic MDX directive registry.
//!
//! A runtime-extensible mapping from directive name (e.g. `card`, `badge`,
//! `note`) to a JSX component name (e.g. `Card`, `Badge`, `Note`). Core seeds
//! ZERO directive names — the entire `:::name` → `<Component>` vocabulary is
//! supplied by the user's `features.directives` config / docs recipes.
//!
//! ## Directive shapes
//!
//! Three CommonMark-Directives-flavoured shapes are recognised:
//!
//! - **Container** — `:::name[label]{attrs}` … `:::`. Block-level. Body
//!   between fences becomes the JSX element's children.
//! - **Leaf** — `::name[label]{attrs}` (single line, no fenced body).
//!   Block-level. The bracketed `[label]` (if any) becomes the only
//!   text child.
//! - **Text** — `:name[label]{attrs}` inline within a paragraph.
//!
//! `[label]` and `{attrs}` are both optional. Attribute parsing also
//! falls back to the legacy unbraced form (e.g.
//! `:::details title="Click me"`) so the existing admonition fixtures
//! keep producing bit-for-bit identical JSX output.
//!
//! ## Attribute escaping (v1)
//!
//! All directive attribute values emit as JSX **string-literal**
//! attributes (`title="foo"`). Raw-expression attributes
//! (`title={someVar}`) are NOT supported in v1 — every value is a
//! [`AttributeValue::Literal`] under the hood, and the downstream JSX
//! emitter (`crate::mdx_jsx_emit`) escapes `"`, `&`, `<`, `>`, and
//! line terminators when rendering. Hyphenated keys (e.g. `data-foo`)
//! round-trip verbatim as-is.
//!
//! ## Warning sink (v1)
//!
//! Unknown directive names — and, since zfb#2206, genuinely-unclosed
//! container openers (an opener with no closing `:::` before the next
//! directive opener / end of siblings) — produce a
//! [`DirectiveDiagnostic`] with optional line/column info, NOT a parse
//! error. The source paragraph is left intact. Diagnostics accumulate on
//! the registry instance and the orchestrator (e.g. the pipeline runner)
//! is responsible for draining and printing them. Use
//! [`DirectiveRegistry::take_diagnostics`] after a pipeline run.
//!
//! ## No built-in defaults
//!
//! [`DirectiveRegistry::new`] starts empty. Callers register exactly the
//! directives they want via [`DirectiveRegistry::register`]; there is no
//! preset of `note`/`tip`/… in core.

use std::collections::HashMap;

use markdown::mdast::{
    AttributeContent, AttributeValue, MdxJsxAttribute, MdxJsxFlowElement, MdxJsxTextElement,
    Node as MdastNode, Text,
};

use crate::pipeline::{
    constructs_for_target, BuildContext, MdastVisitor, ResolvedGfmConstructs, SecondaryParseTarget,
};
use zfb_md_ast::diagnostics::{DiagnosticSeverity, MarkdownDiagnostic, SourceLocation};
use zfb_md_ast::{SecondaryParseNormalization, SecondaryParsePlacement};

// Directive definition types moved to `zfb-md-ast` so `zfb-md-extras`
// (which cannot depend on `zfb-content`) can produce `Vec<DirectiveDef>`
// for preset functions. Re-export them here so all existing
// `zfb_content::plugins::directives::*` import paths keep compiling.
pub use zfb_md_ast::{
    AttrSchema, AttrType, AttrValidationResult, DirectiveDef, DirectiveDiagnostic, DirectiveKind,
    ValidatedAttrValue,
};

/// Runtime registry mapping directive names to JSX components.
///
/// Implements [`MdastVisitor`]: when used as a pipeline visitor it
/// rewrites recognised directive paragraphs into [`MdxJsxFlowElement`]
/// (or [`MdxJsxTextElement`] for inline text directives) nodes, and
/// records [`DirectiveDiagnostic`]s for unknown directive names it
/// encounters.
#[derive(Debug, Clone)]
pub struct DirectiveRegistry {
    defs: HashMap<String, DirectiveDef>,
    diagnostics: Vec<DirectiveDiagnostic>,
    /// True if any registered directive has `kind == Text`. Cached so
    /// the inline scanner can be skipped in the common case.
    has_text_dirs: bool,
    /// GFM constructs [`reparse_block`] parses with (zfb#2390).
    ///
    /// Defaults to [`ResolvedGfmConstructs::ALL_OFF`], reproducing the
    /// bare `ParseOptions::mdx()` that site used before #2390 — so a
    /// registry built by [`DirectiveRegistry::new`] alone stays
    /// byte-identical. `zfb-content`'s feature wiring calls
    /// [`DirectiveRegistry::with_gfm`] with the pipeline's own resolved
    /// set.
    ///
    /// This governs the two `reparse_block` callers: a collapsed
    /// (blank-line-less) directive body, and — via `flush_prose` —
    /// ordinary page prose that merely sits between two collapsed
    /// directive runs.
    ///
    /// Note this rarely changes end-to-end pipeline output, and that is
    /// by design rather than by accident: `reparse_block` is reachable
    /// only through `single_text_collapsed`, and now that the main parse
    /// shares these same constructs, content rich enough to render
    /// differently is generally already tokenised into multiple inline
    /// children by that main parse and routed to
    /// `transform_block_container` instead. Threading the constructs
    /// here removes a latent inconsistency and keeps the parse sites in
    /// lockstep; it is not the surface #2390's changelog entry warns
    /// about (that is transclude). The math constructs on top of this
    /// set follow [`DirectiveRegistry::target`].
    gfm: ResolvedGfmConstructs,
    /// The project's `markdown.cjkFriendly` setting (zfb#1105), ANDed
    /// with `gfm.autolink_literal` at the single point that consumes it
    /// so the inconsistent combination is unrepresentable — see the
    /// matching field on `zfb_md_extras::transclude::TranscludePlugin`.
    cjk_friendly: bool,
    /// The project's `markdown.hardBreaks` setting (zfb#2398), passed raw
    /// — unlike `cjk_friendly` it is never ANDed with `gfm.autolink_literal`
    /// (the two features are unrelated). [`reparse_block`]'s callers each
    /// declare their own downstream [`SecondaryParsePlacement`], which
    /// decides whether `HardBreaksPlugin` is safe to apply at the current
    /// [`DirectiveRegistry::target`] — see [`DirectiveRegistry::flush_prose`]
    /// and [`DirectiveRegistry::build_collapsed_body`].
    hard_breaks: bool,
    /// Which emit path the current pipeline run is feeding (zfb#2397).
    ///
    /// Overwritten unconditionally by
    /// [`MdastVisitor::set_secondary_parse_target`] at the top of every
    /// `Pipeline` mdast dispatch loop; never read as carry-over from a
    /// previous run. [`reparse_block`] turns it into the math half of
    /// its construct set — off for `Html` (byte-identical to before), on
    /// for `Jsx`, matching what each path's own top-level parse uses.
    ///
    /// As with the `gfm` field above, this rarely changes end-to-end
    /// output, and for the same reason: on the JSX path the main parse
    /// already has math on, so it tokenises the math into `InlineMath` /
    /// `Math` nodes, and the collapsed run's line view carries them as
    /// PRESERVED nodes — those ranges are assembled losslessly rather than
    /// re-parsed, so [`reparse_block`] is never reached for them
    /// (zfb#2413). Threading it keeps the two parse sites in lockstep; the
    /// surface #2397 actually fixes is transclude.
    ///
    /// Defaults to `Html`, so a registry driven outside a `Pipeline`
    /// loop keeps exactly the pre-#2397 construct set. Also combined with
    /// each call site's [`SecondaryParsePlacement`] (zfb#2398) to decide
    /// whether `HardBreaksPlugin` may run there — see `hard_breaks` above.
    target: SecondaryParseTarget,
}

/// The knobs [`reparse_block`] needs, bundled into one argument so this
/// secondary parse site can grow parser settings without growing a
/// positional parameter list.
///
/// `zfb_md_extras::transclude::ExpandEnv` — the twin at the other
/// secondary parse site — carries these same four knobs, but as loose
/// fields alongside its recursion state (`project_root`, `recorder`,
/// `max_depth`), so it is not a knobs bundle in the way this struct is.
/// What the two sites genuinely share is the normalisation those knobs
/// drive: [`SecondaryParseNormalization`].
#[derive(Debug, Clone, Copy)]
struct SecondaryParseCtx {
    gfm: ResolvedGfmConstructs,
    cjk_friendly: bool,
    /// The registry's `hard_breaks` setting — [`reparse_block`] ANDs this
    /// with the caller-supplied [`SecondaryParsePlacement`] rather than
    /// applying it unconditionally (zfb#2398).
    hard_breaks: bool,
    target: SecondaryParseTarget,
}

impl Default for DirectiveRegistry {
    /// Empty registry that re-parses with every GFM construct OFF.
    ///
    /// Hand-written rather than derived: a derived `Default` would give
    /// `ResolvedGfmConstructs::default()`, which is `CONSERVATIVE` (three
    /// constructs ON) — silently changing what a bare registry does.
    fn default() -> Self {
        Self {
            defs: HashMap::new(),
            diagnostics: Vec::new(),
            has_text_dirs: false,
            gfm: ResolvedGfmConstructs::ALL_OFF,
            cjk_friendly: false,
            hard_breaks: false,
            target: SecondaryParseTarget::Html,
        }
    }
}

impl DirectiveRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Re-parse collapsed directive bodies and inter-run prose with
    /// `gfm`, applying the post-parse normalisations those constructs
    /// make mandatory (zfb#2390).
    ///
    /// `cjk_friendly` is the project's `markdown.cjkFriendly` setting,
    /// passed raw — it is ANDed with `gfm.autolink_literal` here rather
    /// than by the caller.
    #[must_use]
    pub fn with_gfm(mut self, gfm: ResolvedGfmConstructs, cjk_friendly: bool) -> Self {
        self.gfm = gfm;
        self.cjk_friendly = cjk_friendly;
        self
    }

    /// Apply `HardBreaksPlugin` at [`reparse_block`]'s call sites when
    /// `hard_breaks` is `true` (zfb#2398), matching the project's
    /// `markdown.hardBreaks` setting. Passed raw, un-ANDed with anything
    /// here — each call site declares its own downstream
    /// [`SecondaryParsePlacement`], which decides whether the pass is safe
    /// there (see [`Self::flush_prose`] / [`Self::build_collapsed_body`]).
    #[must_use]
    pub fn with_hard_breaks(mut self, hard_breaks: bool) -> Self {
        self.hard_breaks = hard_breaks;
        self
    }

    /// Register a directive. Later registrations under the same name
    /// overwrite earlier ones (registry is the user's tool — they
    /// decide precedence).
    pub fn register(&mut self, def: DirectiveDef) {
        if def.kind == DirectiveKind::Text {
            self.has_text_dirs = true;
        }
        self.defs.insert(def.name.clone(), def);
    }

    /// Read-only view of accumulated diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> &[DirectiveDiagnostic] {
        &self.diagnostics
    }

    /// Drain accumulated diagnostics. Useful when the registry is
    /// reused across multiple pipeline runs and the orchestrator wants
    /// to print and clear the buffer between files.
    pub fn take_diagnostics(&mut self) -> Vec<DirectiveDiagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// Wrap this registry in a `Box<dyn MdastVisitor>` for insertion
    /// into a [`crate::pipeline::Pipeline`]. The returned box owns the
    /// registry; for streaming diagnostics across pipeline runs, keep
    /// a separate registry instance and call [`MdastVisitor::visit`]
    /// directly.
    #[must_use]
    pub fn into_visitor(self) -> Box<dyn MdastVisitor> {
        Box::new(self)
    }

    /// Get a registered directive by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&DirectiveDef> {
        self.defs.get(name)
    }

    fn secondary_parse_ctx(&self) -> SecondaryParseCtx {
        SecondaryParseCtx {
            gfm: self.gfm,
            cjk_friendly: self.cjk_friendly,
            hard_breaks: self.hard_breaks,
            target: self.target,
        }
    }
}

impl MdastVisitor for DirectiveRegistry {
    fn visit(&mut self, node: &mut MdastNode) {
        if let Some(children) = node.children_mut() {
            self.transform_children(children);
            for c in children.iter_mut() {
                self.visit(c);
            }
        }
    }

    fn set_secondary_parse_target(&mut self, target: SecondaryParseTarget) {
        self.target = target;
    }

    /// Context-armed pipelines (real builds) surface directive
    /// diagnostics through the shared markdown-diagnostics channel: after
    /// the walk, the registry drains its buffer into the [`BuildContext`]
    /// diagnostics sink as Warning-severity [`MarkdownDiagnostic`]s, so
    /// unknown-directive and unclosed-container findings (zfb#2206) reach
    /// the build output via `Pipeline::take_markdown_diagnostics` instead
    /// of dying inside the boxed visitor. Warning severity keeps the
    /// graceful-fallback contract — the build prints the findings but
    /// never aborts on them.
    ///
    /// Context-free runs (`Pipeline::run`, direct `visit` calls) keep
    /// accumulating on the registry for manual draining via
    /// [`DirectiveRegistry::take_diagnostics`] — unchanged behaviour.
    fn visit_with_context(&mut self, node: &mut MdastNode, ctx: &mut BuildContext<'_>) {
        self.visit(node);
        if self.diagnostics.is_empty() {
            return;
        }
        let Some(sink) = ctx.diagnostics.as_deref_mut() else {
            return;
        };
        for d in std::mem::take(&mut self.diagnostics) {
            sink.emit(MarkdownDiagnostic::Generic {
                severity: DiagnosticSeverity::Warning,
                message: d.message,
                location: Some(SourceLocation {
                    path: ctx.source_path.clone(),
                    line: d.line.and_then(|l| u32::try_from(l).ok()),
                    col: d.column.and_then(|c| u32::try_from(c).ok()),
                }),
            });
        }
    }
}

// -- transformation -----------------------------------------------------

impl DirectiveRegistry {
    /// Rewrite `:::kind / :::` and `::name[…]` runs in `children` in
    /// place. Inline `:name[…]` rewrites are handled in the per-text
    /// scanner.
    fn transform_children(&mut self, children: &mut Vec<MdastNode>) {
        // First pass: container + leaf at block level.
        let mut i = 0;
        while i < children.len() {
            // Collapsed-run entry (issues #1090 + #1094 + #2413). See
            // `recognise_collapsed_run`. It must run BEFORE the block-level
            // container check so a fully-collapsed run keeps its dedicated
            // line-level path. A `None` means no fence run was recognised
            // (e.g. the first line looked like an opener but had no closer)
            // — fall through to the block-level handlers below.
            if let Some(replacement) = self.recognise_collapsed_run(&children[i]) {
                i = self.splice_collapsed_replacement(children, i, replacement);
                continue;
            }

            // Try container open.
            if let Some((parsed, open_colons)) = self.parse_block_container_open(&children[i]) {
                if let Some(def) = self.defs.get(&parsed.name).cloned() {
                    if def.kind == DirectiveKind::Container {
                        if let Some(next_i) =
                            self.transform_block_container(children, i, &def, &parsed, open_colons)
                        {
                            i = next_i;
                            continue;
                        }
                        // Genuinely unclosed (diagnostic recorded inside):
                        // leave the paragraph alone and fall through.
                    }
                } else {
                    // Looks like a container, but the name is not
                    // registered. Surface a warning and leave the AST
                    // alone — old behaviour.
                    self.warn_unknown(&parsed.name, &children[i]);
                }
            }

            // Try leaf at block level. A leaf occupies a single
            // paragraph whose first text run is exactly
            // `::name[label]{attrs}` (no fenced body).
            if let Some(parsed) = self.parse_block_open(&children[i], 2) {
                if let Some(def) = self.defs.get(&parsed.name).cloned() {
                    if def.kind == DirectiveKind::Leaf {
                        let position = paragraph_position(&children[i]);
                        let (line, column) = paragraph_line_col(&children[i]);
                        let validated_opt = self.run_validation(&def, &parsed, line, column);
                        let jsx = build_leaf_jsx(&def, &parsed, position, validated_opt.as_ref());
                        children[i] = jsx;
                        i += 1;
                        continue;
                    }
                } else {
                    self.warn_unknown(&parsed.name, &children[i]);
                }
            }

            // zfb#2211: nothing above engaged this node — but a paragraph
            // whose first line is NOT an opener can still glue a `:::name`
            // opener on a later line. Record the unclosed diagnostic for a
            // genuinely-unclosed buried opener; the node stays untouched.
            self.warn_unclosed_buried_opener(children, i);

            // Inline text directive scan inside this paragraph.
            if self.has_text_dirs {
                self.transform_inline_in(&mut children[i]);
            }

            i += 1;
        }
    }

    /// Try the line-level re-segmenter on the paragraph at `node`.
    ///
    /// When the fences are written with NO blank lines, `markdown::to_mdast`
    /// merges the opener, body, inner fences, and closers into ONE
    /// multi-line Paragraph. The block-level scan in `transform_children`
    /// (which inspects whole paragraph nodes and exactly-3-colon openers)
    /// cannot see sibling/nested directives buried as mid-paragraph LINES,
    /// nor a >3-colon outer opener, so any collapsed paragraph whose first
    /// line is a `:::+name` opener (>=3 colons) is routed through the
    /// re-segmenter, which emits one JSX flow element per top-level
    /// directive run and recurses for nested ones.
    ///
    /// Two source shapes reach it, and they differ only in how the LINES
    /// are recovered (zfb#2413):
    ///
    /// - the pristine collapsed paragraph — ONE multi-line `Text` child
    ///   ([`single_text_collapsed`]), split on `\n`;
    /// - the SPLIT shape — the same authored source after the pipeline's
    ///   own top-level `HardBreaksPlugin` / `CjkFriendlyPlugin` (both
    ///   wired before `DirectiveRegistry`) retokenised that `Text` child
    ///   into `Text`/`Break`/`Strong`/… , recovered with
    ///   [`split_inline_lines`]. Before zfb#2413 this shape declined here
    ///   and fell through to [`DirectiveRegistry::transform_block_container`],
    ///   which has no colon stack and no >3-colon opener rule — so under
    ///   `markdown.hardBreaks` a NESTED collapsed directive leaked a
    ///   literal `:::tip` plus a stray `:::` paragraph into the rendered
    ///   output on both emit paths, and a `:::::note` outer fence went
    ///   fully literal.
    fn recognise_collapsed_run(&mut self, node: &MdastNode) -> Option<Vec<MdastNode>> {
        // Cheap pre-filter before any line lifting or node cloning — the
        // same discipline `warn_unclosed_buried_opener` applies. This runs
        // for EVERY child of EVERY `transform_children` call, and only a
        // paragraph whose FIRST line is an opener fence can be a collapsed
        // run; `is_block_opener_shaped` answers that off the leading `Text`
        // child alone. It is exactly the gate the line view re-applies
        // below (a leading non-`Text` node, or a first line that is not
        // `:::+name`, fails both), so no paragraph changes hands — plain
        // prose paragraphs just stop being split and deep-cloned first.
        if !is_block_opener_shaped(node) {
            return None;
        }
        // `single_text_collapsed` defaults a position-less paragraph to
        // (1, 1) while the block-level `warn_unknown` reports `None` for
        // one; each shape keeps the diagnostic position rule it had.
        let (lines, first_pos) = match single_text_collapsed(node) {
            Some((text_value, line_no, base_col)) => {
                (lift_plain_lines(&text_value), Some((line_no, base_col)))
            }
            None => (
                paragraph_inline_lines(node)?,
                paragraph_position(node).map(|p| (p.start.line, p.start.column)),
            ),
        };
        inline_line_opener_colons(lines.first()?)?;
        self.transform_collapsed_run(
            &lines,
            first_pos,
            // This paragraph IS a top-level sibling in `children`, and the
            // replacement is spliced into its slot — so is any prose
            // flushed from it.
            SecondaryParsePlacement::TopLevelSibling,
        )
    }

    /// Splice a recognised collapsed run's `replacement` into `children[i]`
    /// and return the scan position to continue from.
    ///
    /// [`DirectiveRegistry::transform_collapsed_run`] owns the paragraph: it
    /// has transformed every recognised run and warned every unknown exactly
    /// once, so the caller must NOT fall through to the block-level warn
    /// paths (no double-warn).
    fn splice_collapsed_replacement(
        &mut self,
        children: &mut Vec<MdastNode>,
        i: usize,
        replacement: Vec<MdastNode>,
    ) -> usize {
        let n = replacement.len();
        children.splice(i..=i, replacement);
        // One exception (zfb#2206): when the run's TRAILING prose starts
        // with a REGISTERED container opener (an unclosed tail from the
        // collapsed run's in-paragraph view), give the block-level handler
        // one direct shot at it — it may close across the FOLLOWING sibling
        // paragraphs, and otherwise records the unclosed diagnostic.
        // Unknown names are never re-warned here (the registered-name gate
        // excludes them), and the handler never re-enters the collapsed
        // path, so no double-warn and no re-entry loop is possible.
        if n == 0 {
            return i;
        }
        let tail = i + n - 1;
        // NON-tail segments can leak a literal opener too (zfb#2212): an
        // unclosed opener glued ABOVE a valid run precedes its transformed
        // JSX in the replacement, where the tail check never looks. Record
        // the unclosed diagnostic for those — diagnostic only, never a
        // transform.
        self.warn_unclosed_leaked_segments(children, i, tail);
        // The zfb#2211 buried shape can arrive as a spliced literal segment
        // too (e.g. trailing prose gluing an unclosed opener behind a
        // non-opener first line) — the loop skips the replacement range, so
        // scan it here. Opener-HEADED segments are skipped by the scan's
        // head gate: the leaked-segment pass above and the tail handler
        // below own those, so no segment can warn twice.
        for seg in i..=tail {
            self.warn_unclosed_buried_opener(children, seg);
        }
        if let Some((tail_parsed, tail_colons)) = self.parse_block_container_open(&children[tail]) {
            if let Some(tail_def) = self.defs.get(&tail_parsed.name).cloned() {
                if tail_def.kind == DirectiveKind::Container {
                    if let Some(next_i) = self.transform_block_container(
                        children,
                        tail,
                        &tail_def,
                        &tail_parsed,
                        tail_colons,
                    ) {
                        return next_i;
                    }
                    // Genuinely unclosed: diagnostic recorded; the tail
                    // stays literal.
                }
            }
        }
        i + n
    }

    /// Transform the container directive whose opener paragraph sits at
    /// `children[i]`, handling GLUED opener/closer lines (zfb#2206).
    ///
    /// markdown-rs glues `:::` fence lines into adjacent paragraphs when
    /// blank lines are missing, so the opener paragraph may carry the first
    /// body block after its opener line, and the closing `:::` may be the
    /// last line of the final body paragraph. Shapes handled, in order:
    ///
    /// (a) the closing `:::` line lives INSIDE the opener paragraph itself
    ///     (fully-collapsed multi-inline-child form — e.g. a backticked
    ///     bracket title with no blank lines; the single-`Text` collapsed
    ///     form never reaches here, the collapsed-run entry owns it);
    /// (b) the closer lives in a LATER sibling — a standalone `:::`
    ///     paragraph, or glued as a line of a body paragraph. The
    ///     following siblings are flattened into ONE block-level fence-line
    ///     view and matched by [`inline_lines_matching_closer`], the same
    ///     colon stack the collapsed-run path uses (zfb#2416), so a NESTED
    ///     container written with blank lines around its fences is matched
    ///     transactionally: the outer fence claims a closer only once the
    ///     nested range is proven balanced. An unbalanced range yields no
    ///     match at all, which is what keeps a broken opener from stealing
    ///     a later directive's closer (the pre-zfb#2206 cascade the old
    ///     bounded "stop at the next opener-shaped sibling" scan guarded).
    ///     Non-paragraph siblings (code fences, lists, headings, …)
    ///     contribute no fence lines and pass through as body content.
    ///
    /// `open_colons` is the opener's own colon count, from
    /// [`Self::parse_block_container_open`] — a `:::::note` outer fence
    /// matches on 5, not on a hardcoded 3.
    ///
    /// Body assembly preserves phrasing: the opener paragraph's glued
    /// remainder lines and the closer paragraph's preceding lines become
    /// body paragraphs with their inline nodes intact (only the opener
    /// LINE itself is ever flattened, for parsing). Prose glued AFTER the
    /// closer line is re-emitted as a following sibling and left at the
    /// caller's scan position, so a directive run glued after a closer is
    /// revisited rather than dropped.
    ///
    /// Returns `Some(next_index)` when the directive transformed. Returns
    /// `None` when the opener is genuinely unclosed — an "unclosed
    /// container directive" diagnostic is recorded and the paragraph is
    /// left untouched (graceful literal fallback).
    fn transform_block_container(
        &mut self,
        children: &mut Vec<MdastNode>,
        i: usize,
        def: &DirectiveDef,
        parsed: &ParsedDirective,
        open_colons: usize,
    ) -> Option<usize> {
        let (line, column) = paragraph_line_col(&children[i]);
        let opener_lines = paragraph_inline_lines(&children[i]);

        // (a) closer inside the opener paragraph itself.
        if let Some(lines) = &opener_lines {
            if let Some(k) = inline_lines_matching_closer(lines, 1, open_colons) {
                let validated_opt = self.run_validation(def, parsed, line, column);
                let inner: Vec<MdastNode> =
                    paragraph_from_lines(&lines[1..k]).into_iter().collect();
                let after = paragraph_from_lines(&lines[k + 1..]);
                let jsx = build_flow_jsx(def, parsed, inner, validated_opt.as_ref());
                children[i] = jsx;
                if let Some(after) = after {
                    children.insert(i + 1, after);
                }
                return Some(i + 1);
            }
        }

        // (b) cross-sibling colon-stack scan for the closer (zfb#2416).
        //
        // Flatten the following siblings into one block-level view of FENCE
        // LINES and hand it to the ONE matcher, `inline_lines_matching_closer`
        // — no second stack implementation. Which lines become fence events is
        // load-bearing:
        //
        //   * ALL lines of a paragraph whose HEAD is opener-shaped — the
        //     OPENER paragraph itself included: that paragraph is a real
        //     fence run (possibly a collapsed one), so its inner
        //     opener/closer lines are genuine nesting events.
        //   * Only CLOSER lines of any other paragraph: a `:::` glued to the
        //     end of a body paragraph really does close something.
        //
        // An opener-shaped line BURIED behind a non-opener first line stays a
        // NON-event. zfb#2211 pins those as literal (they never transform), so
        // counting them here would flip a spaced container whose body glues an
        // unclosed buried opener (`prose` / `:::tip` / `never closed`) from
        // transforming today to unclosed — a regression, not a fix.
        let mut fence_lines: Vec<InlineLine> = Vec::new();
        // `None` marks a line lifted from the OPENER paragraph itself; a
        // `Some((sibling, line))` origin is a following sibling's line.
        let mut fence_origins: Vec<Option<(usize, usize)>> = Vec::new();
        // The opener paragraph's OWN glued remainder lines open the scan.
        // Its head is an opener by definition, so — exactly like an
        // opener-headed sibling — every one of its lines is a fence event:
        // a `:::tip` glued directly under the opener nests just as it would
        // from a separate paragraph. Shape (a) has already proved no closer
        // matches inside this paragraph, so a match can never land in this
        // prefix; it only carries the nesting state into the sibling scan.
        // Without it, `:::note` / `:::tip` glued together and closed by two
        // later `:::` siblings closed the OUTER fence on the FIRST closer
        // and leaked a literal `:::tip` plus a stray `:::`.
        if opener_lines.as_ref().is_some_and(|lines| lines.len() > 1) {
            let mut own =
                paragraph_inline_lines(&children[i]).expect("children[i] is the opener paragraph");
            for own_line in own.drain(1..) {
                fence_lines.push(own_line);
                fence_origins.push(None);
            }
        }
        for (j, sibling) in children.iter().enumerate().skip(i + 1) {
            let head_opener = is_block_opener_shaped(sibling);
            if !head_opener && !paragraph_may_carry_fence_line(sibling) {
                // Cheap pre-filter: a closer line is a single `Text` of
                // colons, so a paragraph with no `:::` anywhere in its text
                // contributes nothing — skip the split + node cloning.
                continue;
            }
            let Some(lines) = paragraph_inline_lines(sibling) else {
                continue;
            };
            for (k, sibling_line) in lines.into_iter().enumerate() {
                if head_opener || inline_line_closer_colons(&sibling_line).is_some() {
                    fence_lines.push(sibling_line);
                    fence_origins.push(Some((j, k)));
                }
            }
        }
        let Some(Some((j, k))) = inline_lines_matching_closer(&fence_lines, 0, open_colons)
            .map(|hit| fence_origins[hit])
        else {
            self.record_unclosed(&parsed.name, line, column);
            return None;
        };
        let closer_lines = paragraph_inline_lines(&children[j])
            .expect("the matched fence line came from a paragraph sibling");

        let validated_opt = self.run_validation(def, parsed, line, column);
        let mut body_nodes: Vec<MdastNode> = children.drain(i..=j).collect();
        // Drop the opener paragraph (its glued remainder lines re-enter
        // below) and the closer paragraph (replaced by its line splits).
        body_nodes.remove(0);
        body_nodes.pop();
        let mut inner: Vec<MdastNode> = Vec::new();
        if let Some(lines) = &opener_lines {
            inner.extend(paragraph_from_lines(&lines[1..]));
        }
        inner.extend(body_nodes);
        inner.extend(paragraph_from_lines(&closer_lines[..k]));
        let jsx = build_flow_jsx(def, parsed, inner, validated_opt.as_ref());
        children.insert(i, jsx);
        if let Some(after) = paragraph_from_lines(&closer_lines[k + 1..]) {
            children.insert(i + 1, after);
        }
        Some(i + 1)
    }

    /// Transform a fully-collapsed directive paragraph by RE-SEGMENTING it at
    /// the line level, over the [`InlineLine`] view of its inline children.
    ///
    /// ## Why a separate line-level pass exists
    ///
    /// When `:::` fences are written with NO blank lines between them (the
    /// common real-world shape), `markdown::to_mdast` collapses the opener,
    /// body, inner fences, and closers into ONE multi-line `Paragraph`. The
    /// inner / sibling `:::` markers are mid-paragraph LINES — NOT separate
    /// block paragraphs — so the block-level `transform_children` scan (which
    /// inspects whole paragraph nodes) never sees them. The #1090 fix only
    /// rewrote the OUTER container and left any sibling/nested directive as
    /// literal text. This method re-discovers the directive runs line by line.
    ///
    /// The line view is the single shape both callers speak (zfb#2413): a
    /// pristine collapsed paragraph's one multi-line `Text` child lifts
    /// through [`lift_plain_lines`], while a paragraph already retokenised by
    /// the top-level `HardBreaksPlugin` / `CjkFriendlyPlugin` arrives through
    /// [`split_inline_lines`] with its `Break`/`Strong`/… nodes intact. See
    /// [`DirectiveRegistry::recognise_collapsed_run`].
    ///
    /// ## Fence-matching rule
    ///
    /// Openers and closers are matched by COLON COUNT, mirroring the
    /// CommonMark-Directives convention that a nested container uses MORE
    /// colons on its outer fence. A closer line of `k` colons closes the
    /// most-recently-opened (innermost) fence whose opener colon-count is
    /// `<= k`. This makes:
    ///   - `:::note … ::: :::tip … :::` parse as two SIBLINGS, and
    ///   - `:::::outer … :::inner … ::: :::::` parse as inner-then-outer
    ///     (innermost-first), with the body of an outer run recursively
    ///     re-segmented for its nested directives.
    ///
    /// ## Unbalanced / malformed handling
    ///
    /// If an opener never finds a matching closer, the opener line and the
    /// remaining lines after the last successfully-matched run are emitted as
    /// literal text (a Paragraph) — no panic, graceful fallback. Lines between
    /// recognised runs that are plain prose are likewise re-parsed as markdown
    /// and emitted verbatim.
    ///
    /// Returns `Some(replacement_nodes)` when at least one directive fence run
    /// was recognised (transformed to JSX, or an unknown/unmatched run left as
    /// re-parsed prose). In that case this method OWNS the paragraph and warns
    /// every unknown directive exactly once, so the caller must splice the
    /// result and NOT fall through to other warn paths. Returns `None` only
    /// when no fence run was recognised at all (e.g. the first line looked
    /// like an opener but had no closer and no other run matched), letting the
    /// caller leave the paragraph alone.
    ///
    /// Unknown directives are warned HERE (not by the caller's re-walk):
    /// once a run is recognised, every unmatched/unknown line is folded into a
    /// re-parsed prose Paragraph that the outer `visit` re-walk does NOT
    /// re-block-scan (it only walks inline children), so this is the single
    /// authoritative warn site for collapsed directives.
    ///
    /// `first_pos` carries the `(line, column)` source position of the
    /// paragraph's opener for diagnostics, or `None` when unknown (recursive
    /// body re-segmentation has no reliable per-line position).
    ///
    /// `placement` is where this call's returned nodes will land, which
    /// the two callers answer differently and this method cannot work out
    /// for itself: `transform_children` splices the result in as top-level
    /// siblings, while [`DirectiveRegistry::build_collapsed_body`] hands
    /// it to `build_flow_jsx` as the CHILDREN of an `MdxJsxFlowElement`.
    /// It is forwarded to [`DirectiveRegistry::flush_prose`] for the prose
    /// between recognised runs, which lands in whichever of those two
    /// slots its caller occupies (zfb#2402).
    fn transform_collapsed_run(
        &mut self,
        lines: &[InlineLine],
        first_pos: Option<(usize, usize)>,
        placement: SecondaryParsePlacement,
    ) -> Option<Vec<MdastNode>> {
        let mut out: Vec<MdastNode> = Vec::new();
        let mut recognised_any = false;
        let mut i = 0usize;
        // Track plain (non-directive) lines so we can flush them as a prose
        // block in source order between recognised runs.
        let mut prose_start = 0usize;

        while i < lines.len() {
            if let Some(open_colons) = inline_line_opener_colons(&lines[i]) {
                // Find the matching closer line by colon count.
                if let Some(close_idx) = inline_lines_matching_closer(lines, i + 1, open_colons) {
                    // `inline_line_opener_colons` already counted the leading
                    // colons, so slice them off and parse the rest directly
                    // rather than re-scanning the same colons through
                    // `parse_directive_line` (zfb#1099: one colon-count site).
                    if let Some(parsed) = parse_inline_line_body(&lines[i], open_colons) {
                        if let Some(def) = self.defs.get(&parsed.name).cloned() {
                            if def.kind == DirectiveKind::Container {
                                // Flush any pending prose before this run.
                                self.flush_prose(lines, prose_start, i, placement, &mut out);
                                let (line_no, col_no) = pos_for(i, first_pos);
                                let validated_opt =
                                    self.run_validation(&def, &parsed, line_no, col_no);
                                // Body = lines between opener and its closer.
                                let inner = self.build_collapsed_body(&lines[i + 1..close_idx]);
                                let jsx =
                                    build_flow_jsx(&def, &parsed, inner, validated_opt.as_ref());
                                out.push(jsx);
                                recognised_any = true;
                                i = close_idx + 1;
                                prose_start = i;
                                continue;
                            }
                            // Registered but not a container (wrong kind): keep
                            // the ENTIRE opener..=closer run literal — skip past
                            // the closer so a KNOWN directive nested inside the
                            // wrong-kind wrapper is not re-scanned and leaked out
                            // transformed mid-prose (zfb#1094 review C1). These
                            // lines stay in the pending prose run and are flushed
                            // verbatim (or the whole paragraph is left alone when
                            // nothing else in it is recognised).
                            i = close_idx + 1;
                            continue;
                        } else {
                            // Unknown directive name. The opener..=closer lines
                            // stay as literal prose; warn exactly once. (The
                            // re-parsed prose is never re-block-scanned, so this
                            // is the only place the warning can come from.)
                            let (line_no, col_no) = pos_for(i, first_pos);
                            self.diagnostics.push(DirectiveDiagnostic {
                                message: format!("unknown directive `{}`", parsed.name),
                                line: line_no,
                                column: col_no,
                            });
                            recognised_any = true;
                            // Skip past the closer: the whole unknown run stays
                            // literal prose. Without this, a KNOWN directive
                            // nested inside the unknown wrapper would be
                            // re-scanned and transformed mid-prose, splitting the
                            // literal `:::::name`/`:::::` around it (zfb#1094
                            // review C1).
                            i = close_idx + 1;
                            continue;
                        }
                    }
                }
                // No matching closer: leave this line as prose; advance one.
            }
            i += 1;
        }

        if !recognised_any {
            return None;
        }
        // Flush trailing prose after the last recognised run.
        self.flush_prose(lines, prose_start, lines.len(), placement, &mut out);
        Some(out)
    }

    /// Emit the lines `lines[start..end]` as body/prose block nodes and
    /// append them to `out`. Empty / whitespace-only ranges produce nothing.
    /// Used for prose that sits between collapsed directive runs (or
    /// unmatched/unknown fence lines), including literal `:::` markers that
    /// did not form a recognised directive.
    ///
    /// The assembly policy is what keeps every pinned rendering
    /// byte-identical across both source shapes (zfb#2413) — see
    /// [`plain_lines_text`]:
    ///
    /// - a range of PLAIN `Text` lines with no hard-break provenance
    ///   re-parses through [`reparse_block`], as the collapsed path has
    ///   always done, so the GFM-materialisation and diagnostics behaviour
    ///   is unchanged;
    /// - a range carrying any PRESERVED node (a `Break`, `Strong`,
    ///   `InlineCode`, math, …) is assembled losslessly with
    ///   [`paragraph_from_lines`] instead. Re-parsing it would flatten the
    ///   preserved nodes AND re-fire the [`SecondaryParsePlacement`] gate on
    ///   `Break`s the top-level `HardBreaksPlugin` already created; carrying
    ///   them through untouched is what leaves that gate's policy alone.
    ///
    /// `placement` comes from the caller
    /// ([`DirectiveRegistry::transform_collapsed_run`]) because this
    /// method's output lands wherever its caller's does, and that differs
    /// per call chain (zfb#2402): prose between two TOP-LEVEL collapsed
    /// runs splices in as an ordinary sibling, where a `Break` renders as
    /// a real `<br>` on both paths; prose between two runs NESTED in a
    /// collapsed container's body becomes a child of the outer
    /// `MdxJsxFlowElement`. The gate is the same one
    /// [`DirectiveRegistry::build_collapsed_body`] applies — see
    /// [`SecondaryParsePlacement`].
    fn flush_prose(
        &self,
        lines: &[InlineLine],
        start: usize,
        end: usize,
        placement: SecondaryParsePlacement,
        out: &mut Vec<MdastNode>,
    ) {
        if start >= end {
            return;
        }
        let range = &lines[start..end];
        match plain_lines_text(range) {
            Some(text) => {
                if text.trim().is_empty() {
                    return;
                }
                out.extend(reparse_block(&text, self.secondary_parse_ctx(), placement));
            }
            None => out.extend(paragraph_from_lines(range)),
        }
    }

    /// Build the JSX children of a collapsed container from its body lines.
    /// Recursively re-segments the body so nested collapsed directives
    /// transform too, then emits any remaining prose under
    /// [`DirectiveRegistry::flush_prose`]'s assembly policy.
    fn build_collapsed_body(&mut self, body_lines: &[InlineLine]) -> Vec<MdastNode> {
        if body_lines.is_empty() {
            return Vec::new();
        }
        // Everything this method produces — whether the recursion's JSX
        // runs and inter-run prose, or the plain-markdown fallback below —
        // becomes the CHILDREN of the `MdxJsxFlowElement` the caller
        // builds with `build_flow_jsx`. When the gate was written that
        // meant the HTML path's lossy `reconstruct_jsx` catch-all
        // stringified a `Break` to the EMPTY STRING, so keeping
        // `HardBreaksPlugin` off here avoided DELETING newlines from
        // directive bodies; zfb#2401 has since given that renderer a real
        // `Break` arm (`<br />`), so what the gate does today is
        // behaviour-preserving, not deletion avoidance — see
        // `SecondaryParsePlacement`'s doc comment in `zfb-md-ast`. The JSX
        // path has always had a real emit arm for `Break`, and is the
        // target where this is a genuine parity fix (zfb#2398, extended to
        // the recursion's own prose by zfb#2402).
        let placement = SecondaryParsePlacement::InsideJsxElement;
        // Recurse: a nested `:::tip … :::` inside the body is itself a
        // collapsed run. `None` means "no directive in the body" — emit the
        // body as plain blocks.
        if let Some(nodes) = self.transform_collapsed_run(body_lines, None, placement) {
            return nodes;
        }
        let mut out = Vec::new();
        self.flush_prose(body_lines, 0, body_lines.len(), placement, &mut out);
        out
    }

    /// Walk inline children of `node` (if any) and rewrite recognised
    /// `:name[label]{attrs}` text runs into [`MdxJsxTextElement`]
    /// nodes. No-op for nodes that are not paragraphs/headings/etc.
    fn transform_inline_in(&mut self, node: &mut MdastNode) {
        let Some(inline) = node.children_mut() else {
            return;
        };
        let mut out: Vec<MdastNode> = Vec::with_capacity(inline.len());
        for child in inline.drain(..) {
            if let MdastNode::Text(t) = &child {
                let scanned = self.scan_text_for_text_directives(&t.value);
                if scanned.len() == 1 {
                    if let MdastNode::Text(ref new_text) = scanned[0] {
                        if new_text.value == t.value {
                            // No replacement happened — keep original.
                            out.push(child);
                            continue;
                        }
                    }
                }
                out.extend(scanned);
            } else {
                out.push(child);
            }
        }
        *inline = out;
    }

    fn scan_text_for_text_directives(&mut self, source: &str) -> Vec<MdastNode> {
        let mut out: Vec<MdastNode> = Vec::new();
        let bytes = source.as_bytes();
        let mut i = 0;
        let mut last_emit = 0;
        while i < bytes.len() {
            if bytes[i] == b':'
                && (i == 0 || !is_name_char(bytes[i - 1]))
                && (i + 1 < bytes.len() && is_name_start(bytes[i + 1]))
            {
                // Ensure we are NOT at a `::` (leaf/container) opener.
                if i + 1 < bytes.len() && bytes[i + 1] == b':' {
                    i += 1;
                    continue;
                }
                // Try to parse `:name[label]{attrs}` starting at i.
                if let Some((parsed, end)) = parse_text_directive(source, i) {
                    if let Some(def) = self.defs.get(&parsed.name).cloned() {
                        if def.kind == DirectiveKind::Text {
                            // Flush prefix.
                            if last_emit < i {
                                out.push(MdastNode::Text(Text {
                                    value: source[last_emit..i].to_string(),
                                    position: None,
                                }));
                            }
                            // Inline text directives have no source position
                            // from the text scanner — pass None for both.
                            let validated_opt = self.run_validation(&def, &parsed, None, None);
                            out.push(build_text_jsx(&def, &parsed, validated_opt.as_ref()));
                            i = end;
                            last_emit = end;
                            continue;
                        }
                        // Registered but wrong kind: leave alone.
                    } else {
                        // Unknown text directive: warn, leave intact.
                        self.diagnostics.push(DirectiveDiagnostic {
                            message: format!("unknown directive `:{}`", parsed.name),
                            line: None,
                            column: None,
                        });
                    }
                }
            }
            i += 1;
        }
        if last_emit == 0 {
            // No replacements; return original text wrapped.
            return vec![MdastNode::Text(Text {
                value: source.to_string(),
                position: None,
            })];
        }
        if last_emit < bytes.len() {
            out.push(MdastNode::Text(Text {
                value: source[last_emit..].to_string(),
                position: None,
            }));
        }
        out
    }

    /// Try to parse the first LINE of `node` (a Paragraph) as a directive
    /// opener with exactly `colons` leading colons.
    ///
    /// The opener head (`:::name`) must start in a leading `Text` child —
    /// a paragraph beginning with a code span is never an opener. When the
    /// opener line spans SEVERAL inline children (a bracketed title
    /// carrying inline markup, e.g. `` :::warning[`code` in title] ``,
    /// splits into Text + InlineCode + Text — zfb#2206), the line is
    /// flattened to plain text for parsing, which normalizes the label to
    /// its plain text content.
    fn parse_block_open(&self, node: &MdastNode, colons: usize) -> Option<ParsedDirective> {
        let MdastNode::Paragraph(p) = node else {
            return None;
        };
        let MdastNode::Text(t) = p.children.first()? else {
            return None;
        };
        // Require the FIRST line to be the directive — anything after a
        // newline is body text.
        let first_ln = first_line(&t.value).trim_end();
        if count_leading_colons(first_ln) != colons {
            return None;
        }
        // Single inline child, or the opener line ends inside the leading
        // Text: the whole opener line is `first_ln` — parse it directly.
        if p.children.len() == 1 || t.value.contains('\n') {
            return parse_directive_body(&first_ln[colons..]);
        }
        // Multi-inline-child opener line: flatten it to plain text.
        let lines = split_inline_lines(&p.children);
        let flat = flatten_line_plain(lines.first()?)?;
        let flat = flat.trim_end();
        if count_leading_colons(flat) != colons {
            return None;
        }
        parse_directive_body(&flat[colons..])
    }

    /// Try to parse `node` (a Paragraph) as a CONTAINER opener of ANY
    /// colon count the convention allows, returning the parse and that
    /// count. The count comes from [`container_opener_colons`] (`>= 3`
    /// followed by a name start), so a `:::::note` outer fence is
    /// recognised at block level too (zfb#2416) — the caller then matches
    /// its closer against that same count.
    ///
    /// This exists as its own entry point rather than as a loosening of
    /// [`Self::parse_block_open`]: the LEAF call site parses with exactly
    /// `2` colons, and a globally lenient count there would make a
    /// container opener parse as a leaf. Every other gate (leading `Text`
    /// child, first line only, multi-inline-child flattening) is
    /// `parse_block_open`'s, unchanged — the count is the only difference.
    fn parse_block_container_open(&self, node: &MdastNode) -> Option<(ParsedDirective, usize)> {
        let MdastNode::Paragraph(p) = node else {
            return None;
        };
        let MdastNode::Text(t) = p.children.first()? else {
            return None;
        };
        let colons = container_opener_colons(first_line(&t.value).trim_end())?;
        self.parse_block_open(node, colons)
            .map(|parsed| (parsed, colons))
    }

    fn warn_unknown(&mut self, name: &str, node: &MdastNode) {
        let (line, column) = paragraph_line_col(node);
        self.diagnostics.push(DirectiveDiagnostic {
            message: format!("unknown directive `{name}`"),
            line,
            column,
        });
    }

    /// Record the "unclosed container directive" Warning for `name`. The
    /// single message site, shared by the block-level scan's
    /// genuinely-unclosed branch and the collapsed-run splice's
    /// leaked-segment pass (zfb#2212).
    fn record_unclosed(&mut self, name: &str, line: Option<usize>, column: Option<usize>) {
        self.diagnostics.push(DirectiveDiagnostic {
            message: format!(
                "unclosed container directive `:::{name}` — missing closing `:::` fence"
            ),
            line,
            column,
        });
    }

    /// Diagnostic-only pass over a collapsed run's spliced replacement
    /// segments in `children[from..to]` (zfb#2212): record the
    /// unclosed-container Warning for every literal segment that BEGINS
    /// with a REGISTERED container opener. Such a segment is an unclosed
    /// leak by construction — a registered container opener with a
    /// matching closer inside the run always transforms, so an opener
    /// heading a literal segment found no closer under the fence-matching
    /// rule. Callers exclude the TAIL segment (it gets the block-level
    /// handler's shot at closing across FOLLOWING siblings instead);
    /// non-tail segments are bounded by the rest of their own run, so
    /// their non-closure is final and no transform is ever attempted —
    /// the literal leak IS the graceful-fallback output for this
    /// malformed shape. Unknown names never fire (the collapsed run
    /// already warned them once) and wrong-kind names are not containers.
    fn warn_unclosed_leaked_segments(&mut self, children: &[MdastNode], from: usize, to: usize) {
        for node in children.iter().take(to).skip(from) {
            let Some((parsed, _)) = self.parse_block_container_open(node) else {
                continue;
            };
            if self.defs.get(&parsed.name).map(|d| d.kind) == Some(DirectiveKind::Container) {
                let (line, column) = paragraph_line_col(node);
                self.record_unclosed(&parsed.name, line, column);
            }
        }
    }

    /// Diagnostic-only scan for a container opener BURIED behind a
    /// non-opener first line (zfb#2211). markdown-rs glues `:::` fence
    /// lines into the preceding prose paragraph when blank lines are
    /// missing, so a paragraph like `prose\n:::warning\nnever closed`
    /// reaches no head-level path at all — pre-2211 the opener leaked
    /// literally with NO "unclosed container directive" Warning, while
    /// the same opener at the paragraph HEAD did warn.
    ///
    /// Records [`Self::record_unclosed`] for the FIRST genuinely-unclosed
    /// buried opener and leaves the AST completely untouched (the literal
    /// leak IS the pinned transform behaviour for this malformed shape).
    /// "Genuinely unclosed" mirrors the head-opener paths on both levels:
    ///
    /// - in-paragraph: no matching closer line under the collapsed scan's
    ///   colon-stack rule ([`inline_lines_matching_closer`]); a CLOSED buried
    ///   run is skipped past — it still leaks, but it is not unclosed;
    /// - siblings: no bare closer line in a FOLLOWING sibling paragraph
    ///   before the next opener-shaped one
    ///   ([`later_siblings_have_closer`], mirroring
    ///   [`Self::transform_block_container`]'s shape-(b) bounds) — a
    ///   later directive's closer never suppresses, but a stray `:::` the
    ///   author plausibly meant as the closer does.
    ///
    /// Exactly-one-diagnostic guards: paragraphs whose FIRST line is
    /// opener-shaped are skipped wholesale — the block-level,
    /// collapsed-run, and leaked-segment paths own those, which keeps the
    /// warn sites disjoint. Only REGISTERED container names in the exact
    /// 3-colon form warn (the same gates as every other `record_unclosed`
    /// caller); a code-span `` `:::name` `` line never counts (the
    /// InlineLine classifiers reject non-`Text` leading nodes). After the
    /// first unclosed opener-shaped line the scan stops: the rest of the
    /// paragraph is that opener's would-be body, mirroring how the head
    /// paths warn only the outermost unclosed opener.
    fn warn_unclosed_buried_opener(&mut self, children: &[MdastNode], i: usize) {
        let MdastNode::Paragraph(p) = &children[i] else {
            return;
        };
        // Cheap pre-filter before any node cloning: a buried fence line
        // starts either right after a newline inside a top-level `Text`,
        // or at a `Text` that opens a line after a hard `Break`. Plain
        // prose paragraphs (the overwhelmingly common case) bail here.
        let may_have_buried_fence = p.children.iter().enumerate().any(|(idx, c)| match c {
            MdastNode::Text(t) => {
                t.value.contains("\n:::") || (idx > 0 && t.value.starts_with(":::"))
            }
            _ => false,
        });
        if !may_have_buried_fence || is_block_opener_shaped(&children[i]) {
            return;
        }
        let lines = split_inline_lines(&p.children);
        let (para_line, column) = paragraph_line_col(&children[i]);
        let mut k = 1;
        while k < lines.len() {
            let Some(open_colons) = inline_line_opener_colons(&lines[k]) else {
                k += 1;
                continue;
            };
            if let Some(close_idx) = inline_lines_matching_closer(&lines, k + 1, open_colons) {
                // A balanced buried run: not unclosed. (It does not
                // transform either — pinned literal fallback.) Skip past
                // its closer and keep scanning.
                k = close_idx + 1;
                continue;
            }
            if open_colons == 3 {
                if let Some(parsed) = parse_inline_line_opener(&lines[k], 3) {
                    if self.defs.get(&parsed.name).map(|d| d.kind) == Some(DirectiveKind::Container)
                        && !later_siblings_have_closer(children, i + 1)
                    {
                        // Each InlineLine is one source line (soft `\n`
                        // and hard-break splits both consume exactly
                        // one), so the buried fence sits `k` lines below
                        // the paragraph start.
                        self.record_unclosed(&parsed.name, para_line.map(|l| l + k), column);
                    }
                }
            }
            return;
        }
    }

    /// Run `validate_attrs` on `def` against the raw attrs in `parsed`.
    /// Appends any resulting diagnostics (annotated with position) to
    /// `self.diagnostics`. Returns `Some(validated_map)` on success
    /// (or when the schema is empty), `None` on validation error.
    fn run_validation(
        &mut self,
        def: &DirectiveDef,
        parsed: &ParsedDirective,
        line: Option<usize>,
        column: Option<usize>,
    ) -> Option<HashMap<String, ValidatedAttrValue>> {
        let (result, warnings) = def.validate_attrs(&parsed.attrs);

        // Append warnings (unknown attrs) — always, regardless of Ok/Err.
        for mut w in warnings {
            w.line = line;
            w.column = column;
            self.diagnostics.push(w);
        }

        match result {
            Ok(validated) => {
                // Schema is empty or all attrs passed — return validated map.
                Some(validated)
            }
            Err(errors) => {
                // Hard errors: append with position and fall back to raw attrs.
                for mut e in errors {
                    e.line = line;
                    e.column = column;
                    self.diagnostics.push(e);
                }
                None
            }
        }
    }
}

// -- shared parsing -----------------------------------------------------

/// Result of parsing a single directive opener.
#[derive(Debug, Clone)]
pub(crate) struct ParsedDirective {
    pub name: String,
    pub label: Option<String>,
    pub attrs: Vec<(String, String)>,
}

/// Parse the part of a directive opener AFTER its leading colons — the name,
/// optional `[label]`, and optional `{attrs}` (or legacy unbraced attrs).
/// Returns `None` when `rest` does not start with a name-start char.
///
/// Every caller counts the leading colons once (via
/// [`count_leading_colons`] / [`container_opener_colons`]) and slices them
/// off before calling here, keeping a single colon-count site per scan
/// (zfb#1099).
fn parse_directive_body(rest: &str) -> Option<ParsedDirective> {
    // The "rest" must start with a name char (so `:::` alone, or `::: `
    // is rejected).
    let rest_bytes = rest.as_bytes();
    if rest_bytes.is_empty() || !is_name_start(rest_bytes[0]) {
        return None;
    }
    let mut i = 0;
    while i < rest_bytes.len() && is_name_char(rest_bytes[i]) {
        i += 1;
    }
    let name = rest[..i].to_string();
    let mut after = &rest[i..];

    // Optional [label].
    let mut label: Option<String> = None;
    if let Some(stripped) = after.strip_prefix('[') {
        // An unterminated bracket is a malformed opener, not a lax
        // no-label opener: reject so the source stays literal instead of
        // transforming with the title silently dropped (zfb#2206). The
        // oracle parser (`crate::directive_parser`) rejects the same shape
        // by requiring the label to close before the line end.
        let end = stripped.find(']')?;
        label = Some(stripped[..end].to_string());
        after = &stripped[end + 1..];
    }

    let trimmed = after.trim_start();
    let attrs = if let Some(braced) = trimmed.strip_prefix('{') {
        // Find the matching close brace. Attribute values may contain
        // `}` only inside quotes; do a quote-aware scan.
        let close = find_matching_close_brace(braced).unwrap_or(braced.len());
        parse_braced_attrs(&braced[..close])
    } else {
        parse_unbraced_attrs(trimmed)
    };

    Some(ParsedDirective { name, label, attrs })
}

/// Try to parse `:name[label]{attrs}` starting at byte offset `start`.
/// Returns the parse plus the byte offset of the first byte AFTER the
/// directive on success. The opening `:` at `start` is required.
fn parse_text_directive(source: &str, start: usize) -> Option<(ParsedDirective, usize)> {
    let bytes = source.as_bytes();
    if bytes.get(start) != Some(&b':') {
        return None;
    }
    let mut i = start + 1;
    if i >= bytes.len() || !is_name_start(bytes[i]) {
        return None;
    }
    let name_start = i;
    while i < bytes.len() && is_name_char(bytes[i]) {
        i += 1;
    }
    let name = source[name_start..i].to_string();

    // [label]
    let mut label: Option<String> = None;
    if i < bytes.len() && bytes[i] == b'[' {
        let label_start = i + 1;
        let mut j = label_start;
        while j < bytes.len() && bytes[j] != b']' {
            j += 1;
        }
        if j >= bytes.len() {
            return None;
        }
        label = Some(source[label_start..j].to_string());
        i = j + 1;
    }

    // {attrs}
    let mut attrs: Vec<(String, String)> = Vec::new();
    if i < bytes.len() && bytes[i] == b'{' {
        let attrs_start = i + 1;
        let close = find_matching_close_brace(&source[attrs_start..])?;
        attrs = parse_braced_attrs(&source[attrs_start..attrs_start + close]);
        i = attrs_start + close + 1;
    }

    // For text directives we require AT LEAST one of label or attrs
    // (otherwise `:foo` text in normal prose would gobble identifiers).
    if label.is_none() && attrs.is_empty() {
        return None;
    }

    Some((ParsedDirective { name, label, attrs }, i))
}

/// Find the index of the matching `}` in `s`, treating `"…"` runs as
/// quoted regions where `}` does not terminate the brace.
fn find_matching_close_brace(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'}' => return Some(i),
            b'"' => {
                // Skip past the closing quote.
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parse a `{key=value key2="value 2"}` attribute body (without the
/// surrounding braces). Returns `(key, value)` pairs in source order.
/// Tolerant: malformed pairs are silently dropped.
pub(crate) fn parse_braced_attrs(s: &str) -> Vec<(String, String)> {
    parse_unbraced_attrs(s)
}

/// Parse a space-separated `key=value key2="value 2"` attribute body.
/// Same tolerance as [`parse_braced_attrs`].
pub(crate) fn parse_unbraced_attrs(s: &str) -> Vec<(String, String)> {
    let mut attrs: Vec<(String, String)> = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Read key: [A-Za-z0-9_-]+
        let key_start = i;
        while i < bytes.len() && is_attr_key_char(bytes[i]) {
            i += 1;
        }
        if i == key_start {
            // Couldn't parse a key — bail out.
            break;
        }
        let key = s[key_start..i].to_string();
        // Skip whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        // `=` required for v1 (no boolean attributes yet).
        if i >= bytes.len() || bytes[i] != b'=' {
            // Treat as boolean: empty-string value.
            attrs.push((key, String::new()));
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'"' {
            // Quoted value.
            i += 1;
            let mut val = String::new();
            // Char-aware copy: slice `s` at byte index `i` so multibyte UTF-8
            // (CJK, accented Latin, emoji) stays intact. `bytes[i] as char`
            // would map each byte 0x80-0xFF to a lone code point and corrupt
            // non-ASCII attribute values.
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    // Support \" and \\ inside quoted attribute values.
                    let c = s[i + 1..].chars().next().unwrap();
                    val.push(c);
                    i += 1 + c.len_utf8();
                    continue;
                }
                let c = s[i..].chars().next().unwrap();
                val.push(c);
                i += c.len_utf8();
            }
            if i < bytes.len() {
                i += 1; // consume closing quote
            }
            attrs.push((key, val));
        } else {
            // Bare token.
            let val_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            attrs.push((key, s[val_start..i].to_string()));
        }
    }
    attrs
}

fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}
fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}
fn is_attr_key_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b':'
}

fn first_line(s: &str) -> &str {
    match s.find('\n') {
        Some(i) => &s[..i],
        None => s,
    }
}

// -- inline-line machinery (zfb#2206) -----------------------------------
//
// markdown-rs glues `:::` fence lines into adjacent paragraphs, and inline
// markup (a backticked bracket title, emphasis in the body) splits a
// paragraph into several inline children. The block-level container scan
// therefore needs a LINE view of a paragraph's inline children: split at
// top-level newline boundaries, find the opener/closer lines, and rejoin
// the remaining lines as body content WITHOUT flattening its phrasing.

/// One line of a paragraph's inline children, plus the separator that
/// ended it: `Some(Break)` for a CommonMark hard break (restored verbatim
/// on rejoin so `<br>` semantics survive the round trip), `None` for a
/// soft newline from a `Text` split or the paragraph end.
#[derive(Debug, Default)]
struct InlineLine {
    nodes: Vec<MdastNode>,
    hard_break: Option<MdastNode>,
}

/// Split a paragraph's inline children into LINES at top-level newline
/// boundaries: newlines inside top-level `Text` values, and `Break`
/// nodes (recorded as the ending line's [`InlineLine::hard_break`]).
/// Every other inline node is atomic and stays on its current line —
/// including nodes with a newline NESTED inside (e.g. emphasis spanning
/// a soft break): the nested newline travels inside the node, which is
/// exactly right for body content. Only opener-line FLATTENING must
/// reject such nodes, and [`flatten_line_plain`] does that itself.
/// Split `Text` segments carry no position; unsplit nodes keep theirs.
fn split_inline_lines(children: &[MdastNode]) -> Vec<InlineLine> {
    fn push_text_segment(line: &mut InlineLine, segment: &str) {
        let segment = strip_trailing_cr(segment);
        if segment.is_empty() {
            return;
        }
        line.nodes.push(MdastNode::Text(Text {
            value: segment.to_string(),
            position: None,
        }));
    }

    let mut lines: Vec<InlineLine> = vec![InlineLine::default()];
    for child in children {
        match child {
            MdastNode::Text(t) if t.value.contains('\n') => {
                let mut parts = t.value.split('\n');
                if let Some(first) = parts.next() {
                    push_text_segment(lines.last_mut().expect("seeded"), first);
                }
                for part in parts {
                    lines.push(InlineLine::default());
                    push_text_segment(lines.last_mut().expect("just pushed"), part);
                }
            }
            MdastNode::Break(_) => {
                lines.last_mut().expect("seeded").hard_break = Some(child.clone());
                lines.push(InlineLine::default());
            }
            other => {
                lines.last_mut().expect("seeded").nodes.push(other.clone());
            }
        }
    }
    lines
}

/// Lift a raw multi-line `Text` value into the [`InlineLine`] view: one
/// single-`Text` line per `\n`-separated source line, with no hard-break
/// provenance. This is the pristine collapsed paragraph's entry into the
/// shared re-segmenter (zfb#2413) — a blank line lifts to a line with no
/// nodes at all, exactly as [`split_inline_lines`] produces for one.
fn lift_plain_lines(value: &str) -> Vec<InlineLine> {
    value
        .split('\n')
        .map(|line| InlineLine {
            nodes: if line.is_empty() {
                Vec::new()
            } else {
                vec![MdastNode::Text(Text {
                    value: line.to_string(),
                    position: None,
                })]
            },
            hard_break: None,
        })
        .collect()
}

/// The plain source text of `lines`, or `None` when the range carries any
/// PRESERVED node — a phrasing node (`Strong`, `InlineCode`, math, …) or a
/// hard break the top-level `HardBreaksPlugin` already created. This is the
/// discriminator for [`DirectiveRegistry::flush_prose`]'s assembly policy:
/// `Some` means the range is losslessly re-parseable, `None` means it must
/// be assembled node-for-node instead.
///
/// A hard break on the range's LAST line is a separator to whatever follows
/// the range, not content inside it — [`join_inline_lines`] drops it too, so
/// it does not make the range unparseable.
fn plain_lines_text(lines: &[InlineLine]) -> Option<String> {
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        if idx + 1 < lines.len() && line.hard_break.is_some() {
            return None;
        }
        for node in &line.nodes {
            match node {
                MdastNode::Text(t) => out.push_str(&t.value),
                _ => return None,
            }
        }
    }
    Some(out)
}

/// Does any text content anywhere inside `node` contain a newline?
fn node_text_contains_newline(node: &MdastNode) -> bool {
    match node {
        MdastNode::Text(t) => t.value.contains('\n'),
        MdastNode::InlineCode(c) => c.value.contains('\n'),
        other => other
            .children()
            .is_some_and(|cs| cs.iter().any(node_text_contains_newline)),
    }
}

/// The inline-line view of a Paragraph node, or `None` for other node
/// kinds (see [`split_inline_lines`]).
fn paragraph_inline_lines(node: &MdastNode) -> Option<Vec<InlineLine>> {
    let MdastNode::Paragraph(p) = node else {
        return None;
    };
    Some(split_inline_lines(&p.children))
}

/// Flatten one inline LINE to plain text for directive-opener parsing.
/// `Text` contributes its value verbatim, `InlineCode` its value (the
/// parser already stripped the backticks), and emphasis-like wrappers
/// their nested text — which is exactly the "title normalized to plain
/// text" contract for bracketed labels carrying inline markup (zfb#2206).
/// Returns `None` for any other inline node kind, and for a node carrying
/// a NESTED newline (the visual line ends inside it, so the flattened
/// text would swallow content beyond the opener line): the line is not
/// reliably flattenable, and callers leave the paragraph alone.
fn flatten_line_plain(line: &InlineLine) -> Option<String> {
    fn rec(node: &MdastNode, out: &mut String) -> bool {
        match node {
            MdastNode::Text(t) => {
                out.push_str(&t.value);
                true
            }
            MdastNode::InlineCode(c) => {
                out.push_str(&c.value);
                true
            }
            MdastNode::Emphasis(_) | MdastNode::Strong(_) | MdastNode::Delete(_) => node
                .children()
                .is_none_or(|cs| cs.iter().all(|c| rec(c, out))),
            _ => false,
        }
    }
    let mut out = String::new();
    for node in &line.nodes {
        if node_text_contains_newline(node) || !rec(node, &mut out) {
            return None;
        }
    }
    Some(out)
}

/// Rejoin split inline lines into one inline run, restoring each line's
/// separator — the recorded `Break` node for a hard break, a `\n` for a
/// soft newline — and merging adjacent `Text` nodes so plain multi-line
/// prose stays a single `Text` (the shape the parser itself produces).
/// Merged `Text` nodes drop their (now stale) positions.
fn join_inline_lines(lines: &[InlineLine]) -> Vec<MdastNode> {
    let mut out: Vec<MdastNode> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            match lines[idx - 1].hard_break.as_ref() {
                Some(br) => out.push(br.clone()),
                None => match out.last_mut() {
                    Some(MdastNode::Text(prev)) => {
                        prev.value.push('\n');
                        prev.position = None;
                    }
                    _ => out.push(MdastNode::Text(Text {
                        value: "\n".to_string(),
                        position: None,
                    })),
                },
            }
        }
        for node in &line.nodes {
            match (out.last_mut(), node) {
                (Some(MdastNode::Text(prev)), MdastNode::Text(t)) => {
                    prev.value.push_str(&t.value);
                    prev.position = None;
                }
                _ => out.push(node.clone()),
            }
        }
    }
    out
}

/// Wrap rejoined lines in a body Paragraph. Empty / whitespace-only
/// content produces no node.
fn paragraph_from_lines(lines: &[InlineLine]) -> Option<MdastNode> {
    if lines.is_empty() {
        return None;
    }
    let children = join_inline_lines(lines);
    let only_ws = children
        .iter()
        .all(|c| matches!(c, MdastNode::Text(t) if t.value.trim().is_empty()));
    if children.is_empty() || only_ws {
        return None;
    }
    Some(MdastNode::Paragraph(markdown::mdast::Paragraph {
        children,
        position: None,
    }))
}

/// Is this paragraph SHAPED like a container-directive opener (first line
/// of the leading `Text` child: >= 3 colons immediately followed by a
/// name)? Answered off the leading `Text` child alone, so it is the cheap
/// pre-filter every fence path opens with, whether or not the name is
/// registered. Two callers depend on it: the collapsed-run entry gate, and
/// `transform_block_container`'s shape-(b) scan, where an opener-shaped
/// HEAD promotes ALL of that paragraph's lines to fence events (a
/// non-opener-headed paragraph contributes only its bare closer lines —
/// zfb#2211's buried openers stay non-events).
fn is_block_opener_shaped(node: &MdastNode) -> bool {
    let MdastNode::Paragraph(p) = node else {
        return false;
    };
    let Some(MdastNode::Text(t)) = p.children.first() else {
        return false;
    };
    container_opener_colons(first_line(&t.value).trim_end()).is_some()
}

/// Could this node contribute a bare CLOSE fence line to the shape-(b)
/// scan? A closer line is a single `Text` node of colons only, so a
/// paragraph carrying no `:::` in any of its text at all cannot hold one.
/// A cheap textual pre-filter — the same discipline
/// [`DirectiveRegistry::warn_unclosed_buried_opener`] applies — so the
/// scan does not split and clone every prose paragraph after an opener.
fn paragraph_may_carry_fence_line(node: &MdastNode) -> bool {
    let MdastNode::Paragraph(p) = node else {
        return false;
    };
    p.children
        .iter()
        .any(|c| matches!(c, MdastNode::Text(t) if t.value.contains(":::")))
}

/// The leading colon count of `line` when it is SHAPED like a container
/// opener fence — the InlineLine spelling of the per-line
/// [`container_opener_colons`] check. The
/// fence must start in a leading `Text` node: a code span rendering as
/// `:::name` is an `InlineCode` child and never counts (phrasing, not a
/// fence), exactly like [`is_block_opener_shaped`] at the paragraph head.
/// Top-level `Text` segments in an [`InlineLine`] never contain `\n`
/// ([`split_inline_lines`] splits on them), so the value IS the line
/// prefix.
fn inline_line_opener_colons(line: &InlineLine) -> Option<usize> {
    match line.nodes.first() {
        Some(MdastNode::Text(t)) => container_opener_colons(&t.value),
        _ => None,
    }
}

/// The colon count of `line` when it is a bare container CLOSE fence: a
/// single `Text` node whose trimmed value is an all-colon run of three or
/// more ([`container_close_colons`]). A code span rendering as `:::` is an
/// `InlineCode` child and never matches. Exposed with the count so
/// [`inline_lines_matching_closer`] can drive the colon-stack matching rule
/// with it — for the collapsed-run transform (zfb#2413), the block-level
/// container scan (zfb#2416), and the buried-opener diagnostic scan
/// (zfb#2211) alike.
fn inline_line_closer_colons(line: &InlineLine) -> Option<usize> {
    match &line.nodes[..] {
        [MdastNode::Text(t)] => container_close_colons(&t.value),
        _ => None,
    }
}

/// The ONE fence-matching stack (zfb#1099's rule, one implementation):
/// find the index in `lines` (searching from `from`) of the closer that
/// matches an opener with `open_colons` colons, honouring NESTED openers of
/// the same family. A close fence of `k` colons closes the innermost
/// still-open fence whose opener colon-count is `<= k`. We start one fence
/// deep (the opener the caller already consumed) and return the index of
/// the closer that brings the depth back to zero. Returns `None` for an
/// unbalanced run (no matching closer) — the caller leaves it as literal
/// text.
///
/// Both source shapes reach it through the same [`InlineLine`] view (a raw
/// `&str` line lifts via [`lift_plain_lines`]), so the rule is written once.
/// Drives both the collapsed-run transform and the buried-opener diagnostic
/// scan (zfb#2211).
fn inline_lines_matching_closer(
    lines: &[InlineLine],
    from: usize,
    open_colons: usize,
) -> Option<usize> {
    // Stack of open fence colon-counts; seed with the opener we're matching.
    let mut stack: Vec<usize> = vec![open_colons];
    for (j, line) in lines.iter().enumerate().skip(from) {
        if let Some(close_k) = inline_line_closer_colons(line) {
            let &top = stack.last().expect("stack seeded with the opener");
            if top > close_k {
                // Closer too small for the innermost opener: it cannot close
                // it — unbalanced, stop scanning (malformed).
                return None;
            }
            stack.pop();
            if stack.is_empty() {
                // Closed back down to our opener — this is the match.
                return Some(j);
            }
        } else if let Some(inner_colons) = inline_line_opener_colons(line) {
            // A nested opener pushes onto the stack.
            stack.push(inner_colons);
        }
    }
    None
}

/// Parse ONE InlineLine as a directive opener with exactly `colons`
/// leading colons — the line-level mirror of
/// [`DirectiveRegistry::parse_block_open`] for fence lines buried BEHIND
/// the paragraph head (zfb#2211). Same gates: the fence must start in a
/// leading `Text` node, and a multi-node line (e.g. a bracketed title
/// carrying inline markup) is flattened via [`flatten_line_plain`] with
/// the colon count re-checked on the flat text.
fn parse_inline_line_opener(line: &InlineLine, colons: usize) -> Option<ParsedDirective> {
    let MdastNode::Text(t) = line.nodes.first()? else {
        return None;
    };
    if count_leading_colons(t.value.trim_end()) != colons {
        return None;
    }
    parse_inline_line_body(line, colons)
}

/// Parse the directive body of `line` past a leading colon run of `colons`
/// that the caller has ALREADY counted (via [`inline_line_opener_colons`]),
/// so the same colons are never re-scanned (zfb#1099: one colon-count site).
/// A single-`Text` line parses its remainder directly; a multi-node line
/// (e.g. a bracketed title carrying inline markup) is flattened via
/// [`flatten_line_plain`] with the colon count re-checked on the flat text.
fn parse_inline_line_body(line: &InlineLine, colons: usize) -> Option<ParsedDirective> {
    let MdastNode::Text(t) = line.nodes.first()? else {
        return None;
    };
    if line.nodes.len() == 1 {
        let lead = strip_trailing_cr(&t.value);
        return parse_directive_body(lead.get(colons..)?);
    }
    let flat = flatten_line_plain(line)?;
    let flat = flat.trim_end();
    if count_leading_colons(flat) != colons {
        return None;
    }
    parse_directive_body(&flat[colons..])
}

/// Does any FOLLOWING sibling paragraph offer a bare closer line to the
/// buried opener, before the next opener takes over? The buried-opener
/// scan's sibling half (zfb#2211). Non-paragraph siblings pass through
/// (they would be body content), as in
/// [`DirectiveRegistry::transform_block_container`]'s shape (b) — but the
/// "stop at the next opener" bound is applied PER LINE, not only to
/// sibling paragraph HEADS (codex review of zfb#2211): within each
/// sibling the FIRST fence-shaped line wins. A bare closer preceded by
/// no opener plausibly closes OUR opener (suppress); an opener-shaped
/// line first — head or buried, code-span-safe via the InlineLine
/// classifiers — means any later closer belongs to THAT opener, never
/// ours, so the scan stops there entirely.
///
/// Deliberately NOT widened to the colon stack when shape (b) adopted it
/// (zfb#2416): this is a diagnostics-only suppression heuristic for a
/// shape that never transforms, and its first-fence-wins bound is what
/// keeps a buried opener from being silenced by a later directive's
/// closer.
fn later_siblings_have_closer(children: &[MdastNode], from: usize) -> bool {
    for sibling in &children[from..] {
        if !matches!(sibling, MdastNode::Paragraph(_)) {
            continue;
        }
        let Some(lines) = paragraph_inline_lines(sibling) else {
            continue;
        };
        for line in &lines {
            if inline_line_closer_colons(line).is_some() {
                return true;
            }
            if inline_line_opener_colons(line).is_some() {
                return false;
            }
        }
    }
    false
}

/// Strip a trailing `\r` (CRLF input) off a single re-segmented line.
fn strip_trailing_cr(line: &str) -> &str {
    line.trim_end_matches('\r')
}

/// Count the leading `:` run at the start of `line`. The single
/// colon-count source of truth, shared by [`container_opener_colons`] and
/// [`parse_directive_line`] so the leading-fence scan is not re-implemented
/// per call site (zfb#1099).
fn count_leading_colons(line: &str) -> usize {
    line.as_bytes().iter().take_while(|&&b| b == b':').count()
}

/// If `line` is a container DIRECTIVE OPENER (`:::+name…`, i.e. ≥3 leading
/// colons immediately followed by a name-start char), return its leading
/// colon count. A bare `:::` (close fence) returns `None` because the byte
/// after the colons is not a name start.
fn container_opener_colons(line: &str) -> Option<usize> {
    let trimmed = line.trim_end();
    let bytes = trimmed.as_bytes();
    let n = count_leading_colons(trimmed);
    if n < 3 {
        return None;
    }
    if n < bytes.len() && is_name_start(bytes[n]) {
        Some(n)
    } else {
        None
    }
}

/// If `line` is a bare container CLOSE fence (only colons, ≥3, no name),
/// return its colon count, else `None`. Reached from both levels through
/// [`inline_line_closer_colons`], so a lenient `:::::` closer behaves
/// identically in the collapsed and blank-line-separated forms.
fn container_close_colons(line: &str) -> Option<usize> {
    let t = line.trim();
    if t.len() >= 3 && t.bytes().all(|b| b == b':') {
        Some(t.len())
    } else {
        None
    }
}

/// Re-parse a body/prose text fragment with the real markdown parser and
/// return its top-level block nodes. Used to materialise collapsed-directive
/// bodies and inter-run prose as proper mdast (Paragraph/list/etc.), matching
/// the shape the blank-line-separated form produces.
///
/// Constructs come from the host project's resolved `markdown.gfm`
/// (zfb#2390) — a bare `ParseOptions::mdx()` inherits
/// `Constructs::default()`, where every `gfm_*` flag is false, so a table
/// or task list written inside a collapsed directive body used to render
/// as literal text while the identical markup at top level rendered
/// normally.
///
/// The math constructs on top of that follow the emit path (zfb#2397),
/// matching what the top-level parse of each path uses: math off for
/// HTML, on for JSX. See [`SecondaryParseCtx::target`].
///
/// The pipeline normalises its own top-level parse ahead of the visitor
/// chain; this site parses from *within* that chain, so it owns the same
/// normalisation for what it produces. The passes, their order, and their
/// gating live in [`SecondaryParseNormalization`], shared with
/// `zfb_md_extras::transclude::normalize_included_subtree`, the twin at
/// the other secondary parse site (zfb#2402).
///
/// `placement` (zfb#2398) is the caller's answer to a question this
/// function cannot see: where the nodes it returns will land. Only
/// `HardBreaksPlugin` cares — see [`SecondaryParsePlacement`] — and
/// `reparse_block` never inspects `ctx.target` for that purpose itself.
/// See `DirectiveRegistry::flush_prose` /
/// `DirectiveRegistry::build_collapsed_body` for what each caller passes
/// and why.
fn reparse_block(
    text: &str,
    ctx: SecondaryParseCtx,
    placement: SecondaryParsePlacement,
) -> Vec<MdastNode> {
    let SecondaryParseCtx {
        gfm,
        cjk_friendly,
        hard_breaks,
        target,
    } = ctx;
    let opts = markdown::ParseOptions {
        constructs: constructs_for_target(gfm, target),
        ..markdown::ParseOptions::mdx()
    };
    let fallback = || {
        vec![MdastNode::Paragraph(markdown::mdast::Paragraph {
            children: vec![MdastNode::Text(Text {
                value: text.to_string(),
                position: None,
            })],
            position: None,
        })]
    };
    let Ok(mut root) = markdown::to_mdast(text, &opts) else {
        return fallback();
    };
    SecondaryParseNormalization::new(gfm, cjk_friendly, hard_breaks, placement, target)
        .apply(&mut root);
    match root {
        MdastNode::Root(root) => root.children,
        _ => fallback(),
    }
}

/// When `node` is a Paragraph with exactly one multi-line `Text` child (the
/// collapsed shape `markdown::to_mdast` produces for blank-line-less fences),
/// return `(text_value, start_line, start_column)` — the Text's value plus the
/// paragraph's start position (defaulting to 1:1 when absent) for diagnostics.
/// Returns `None` for the multi-inline-child (SPLIT) case, which since
/// zfb#2413 reaches the same re-segmenter through [`split_inline_lines`] —
/// see [`DirectiveRegistry::recognise_collapsed_run`]. This function now
/// only picks which line-recovery route that entry takes, and which
/// diagnostic position rule applies; it no longer decides whether the
/// collapsed path runs at all.
fn single_text_collapsed(node: &MdastNode) -> Option<(String, usize, usize)> {
    let MdastNode::Paragraph(p) = node else {
        return None;
    };
    if p.children.len() != 1 {
        return None;
    }
    let MdastNode::Text(t) = &p.children[0] else {
        return None;
    };
    if !t.value.contains('\n') {
        return None;
    }
    let (line, col) = p
        .position
        .as_ref()
        .map_or((1, 1), |pos| (pos.start.line, pos.start.column));
    Some((t.value.clone(), line, col))
}

fn paragraph_position(node: &MdastNode) -> Option<markdown::unist::Position> {
    if let MdastNode::Paragraph(p) = node {
        return p.position.clone();
    }
    None
}

fn paragraph_line_col(node: &MdastNode) -> (Option<usize>, Option<usize>) {
    if let Some(pos) = paragraph_position(node) {
        return (Some(pos.start.line), Some(pos.start.column));
    }
    (None, None)
}

/// Source position for the `i`-th opener line of a collapsed run. Only the
/// very first line (`i == 0`) carries the paragraph's known `first_pos`; later
/// sibling/nested openers are mid-text lines with no reliable position, so they
/// get `(None, None)`. Extracted so `transform_collapsed_run`'s known-container
/// and unknown-name branches share one position rule (zfb#1099).
fn pos_for(i: usize, first_pos: Option<(usize, usize)>) -> (Option<usize>, Option<usize>) {
    match (i, first_pos) {
        (0, Some((l, c))) => (Some(l), Some(c)),
        _ => (None, None),
    }
}

// -- JSX construction ---------------------------------------------------

/// Build a flow JSX element for a Container directive.
fn build_flow_jsx(
    def: &DirectiveDef,
    parsed: &ParsedDirective,
    children: Vec<MdastNode>,
    validated: Option<&HashMap<String, ValidatedAttrValue>>,
) -> MdastNode {
    let attributes = build_attributes(def, parsed, validated);
    MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
        children,
        position: None,
        name: Some(def.component_name.clone()),
        attributes,
    })
}

/// Build a flow JSX element for a Leaf directive. The label (if any
/// and `title_from_label` is false) becomes a single Text child.
fn build_leaf_jsx(
    def: &DirectiveDef,
    parsed: &ParsedDirective,
    position: Option<markdown::unist::Position>,
    validated: Option<&HashMap<String, ValidatedAttrValue>>,
) -> MdastNode {
    let attributes = build_attributes(def, parsed, validated);
    let children: Vec<MdastNode> = if !def.title_from_label {
        if let Some(label) = &parsed.label {
            vec![MdastNode::Text(Text {
                value: label.clone(),
                position: None,
            })]
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    MdastNode::MdxJsxFlowElement(MdxJsxFlowElement {
        children,
        position,
        name: Some(def.component_name.clone()),
        attributes,
    })
}

/// Build a text JSX element for an inline Text directive.
fn build_text_jsx(
    def: &DirectiveDef,
    parsed: &ParsedDirective,
    validated: Option<&HashMap<String, ValidatedAttrValue>>,
) -> MdastNode {
    let attributes = build_attributes(def, parsed, validated);
    let children: Vec<MdastNode> = if !def.title_from_label {
        if let Some(label) = &parsed.label {
            vec![MdastNode::Text(Text {
                value: label.clone(),
                position: None,
            })]
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    MdastNode::MdxJsxTextElement(MdxJsxTextElement {
        children,
        position: None,
        name: Some(def.component_name.clone()),
        attributes,
    })
}

/// Convert parsed attrs (+ optional title-from-label promotion) into
/// mdast `AttributeContent::Property` entries.
///
/// When `validated` is `Some`, the validated map is used as the source of
/// truth for schema-declared attrs (with defaults applied). Unknown attrs
/// (not in the schema) still pass through from `parsed.attrs` verbatim,
/// as the unknown-attr policy is warning-only. When `validated` is `None`,
/// `parsed.attrs` is used directly (fallback on validation error or empty
/// schema).
fn build_attributes(
    def: &DirectiveDef,
    parsed: &ParsedDirective,
    validated: Option<&HashMap<String, ValidatedAttrValue>>,
) -> Vec<AttributeContent> {
    let mut out: Vec<AttributeContent> = Vec::new();
    if def.title_from_label {
        if let Some(label) = &parsed.label {
            out.push(AttributeContent::Property(MdxJsxAttribute {
                name: "title".to_string(),
                value: Some(AttributeValue::Literal(label.clone())),
            }));
        }
    }

    match validated {
        None => {
            // No schema or validation failed — emit raw parsed attrs.
            for (k, v) in &parsed.attrs {
                out.push(AttributeContent::Property(MdxJsxAttribute {
                    name: k.clone(),
                    value: Some(AttributeValue::Literal(v.clone())),
                }));
            }
        }
        Some(val_map) => {
            // Schema-declared attrs: emit from validated map (preserves
            // defaults and type-normalised values like "true"/"false").
            let schema_names: std::collections::HashSet<&str> =
                def.attrs.iter().map(|s| s.name.as_str()).collect();
            for schema in &def.attrs {
                if let Some(val) = val_map.get(&schema.name) {
                    out.push(AttributeContent::Property(MdxJsxAttribute {
                        name: schema.name.clone(),
                        value: Some(AttributeValue::Literal(val.as_str().to_string())),
                    }));
                }
            }
            // Unknown attrs pass through verbatim (warning-only policy).
            for (k, v) in &parsed.attrs {
                if !schema_names.contains(k.as_str()) {
                    out.push(AttributeContent::Property(MdxJsxAttribute {
                        name: k.clone(),
                        value: Some(AttributeValue::Literal(v.clone())),
                    }));
                }
            }
        }
    }

    out
}

// -- helpers used by tests ----------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{Code, Heading, Paragraph, Root};

    /// Build a paragraph with one Text child. Used by the registry's
    /// own tests to construct fixtures.
    fn text_para(value: &str) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children: vec![MdastNode::Text(Text {
                value: value.to_string(),
                position: None,
            })],
            position: None,
        })
    }

    /// Test fixture: a registry pre-loaded with the seven admonition
    /// container names (with `title_from_label = true`, matching how a docs
    /// recipe would register them via `features.directives`). Core no longer
    /// ships a built-in preset, so the vocabulary is declared explicitly here
    /// for tests that exercise `:::note` / `:::details` style directives.
    fn registry_with_admonitions() -> DirectiveRegistry {
        let mut r = DirectiveRegistry::new();
        for (name, component) in [
            ("note", "Note"),
            ("tip", "Tip"),
            ("warning", "Warning"),
            ("danger", "Danger"),
            ("info", "Info"),
            ("details", "Details"),
            ("caution", "Caution"),
        ] {
            let mut def = DirectiveDef::container(name, component);
            def.title_from_label = true;
            r.register(def);
        }
        r
    }

    fn run_with_registry(reg: &mut DirectiveRegistry, children: Vec<MdastNode>) -> Vec<MdastNode> {
        let mut root = MdastNode::Root(Root {
            children,
            position: None,
        });
        reg.visit(&mut root);
        let MdastNode::Root(Root { children, .. }) = root else {
            unreachable!()
        };
        children
    }

    fn flow(node: &MdastNode) -> &MdxJsxFlowElement {
        match node {
            MdastNode::MdxJsxFlowElement(j) => j,
            other => unreachable!("expected MdxJsxFlowElement, got {other:?}"),
        }
    }

    fn text(node: &MdastNode) -> &MdxJsxTextElement {
        match node {
            MdastNode::MdxJsxTextElement(j) => j,
            other => unreachable!("expected MdxJsxTextElement, got {other:?}"),
        }
    }

    fn attr(j: &MdxJsxFlowElement, name: &str) -> Option<String> {
        for a in &j.attributes {
            if let AttributeContent::Property(p) = a {
                if p.name == name {
                    if let Some(AttributeValue::Literal(v)) = &p.value {
                        return Some(v.clone());
                    }
                }
            }
        }
        None
    }

    fn text_attr(j: &MdxJsxTextElement, name: &str) -> Option<String> {
        for a in &j.attributes {
            if let AttributeContent::Property(p) = a {
                if p.name == name {
                    if let Some(AttributeValue::Literal(v)) = &p.value {
                        return Some(v.clone());
                    }
                }
            }
        }
        None
    }

    // ---- real-parser container tests (Sub #1090) ----

    /// Parse `input` with the real markdown parser, run a registry that
    /// transforms it, and return the resulting top-level nodes. This
    /// exercises the SAME `markdown::to_mdast` → DirectiveRegistry path a
    /// real build uses — NOT hand-built mdast — so it reproduces the
    /// collapsed-paragraph shape that hand-built fixtures never hit.
    fn run_real_parser(reg: &mut DirectiveRegistry, input: &str) -> Vec<MdastNode> {
        let mut root = markdown::to_mdast(input, &markdown::ParseOptions::mdx())
            .expect("markdown-rs should parse the sample");
        reg.visit(&mut root);
        let MdastNode::Root(Root { children, .. }) = root else {
            unreachable!("to_mdast always yields a Root")
        };
        children
    }

    #[test]
    fn real_parser_transforms_container_directives_without_blank_lines() {
        // Issue #1090 / #1085 repro: the reporter's exact markdown with NO
        // blank lines around the fences. `markdown::to_mdast` collapses each
        // `:::name … :::` block into a single multi-line Paragraph; before
        // the fix the registry left them as literal `<p>:::note…</p>` text.
        //
        // This is the guard test: it MUST fail before the fix and pass
        // after. It covers both the bracket title form (`:::tip[Title]`) and
        // the space-after-name title form (`:::tip title="…"`) — the issue
        // calls out both. (The documented title semantics are bracket-label
        // → `title`; the space form carries the title as an explicit
        // `title="…"` attribute, exactly like the legacy `:::details
        // title="Click me"` form. There is no bare-word space-to-title
        // promotion in the engine, so this test does not assert one.)
        let mut r = registry_with_admonitions();
        let input = "\
:::note
plain note body
:::

:::tip[Bracket Title]
tip body
:::

:::tip title=\"Space Title\"
another tip
:::
";
        let out = run_real_parser(&mut r, input);
        // Three directive blocks → three JSX flow elements (no literal text).
        assert_eq!(
            out.len(),
            3,
            "expected 3 transformed directives, got {out:#?}"
        );

        // 1. :::note → <Note> with the plain body.
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        let MdastNode::Paragraph(body) = &note.children[0] else {
            unreachable!(
                "note body should be a Paragraph, got {:?}",
                note.children[0]
            );
        };
        let MdastNode::Text(t) = &body.children[0] else {
            unreachable!("note body text expected");
        };
        assert_eq!(t.value, "plain note body");

        // 2. :::tip[Bracket Title] → <Tip title="Bracket Title">.
        let tip_bracket = flow(&out[1]);
        assert_eq!(tip_bracket.name.as_deref(), Some("Tip"));
        assert_eq!(
            attr(tip_bracket, "title").as_deref(),
            Some("Bracket Title"),
            "bracket label promoted to title attr"
        );

        // 3. :::tip title="Space Title" → <Tip title="Space Title"> (space
        //    form carries the title as an explicit attribute).
        let tip_space = flow(&out[2]);
        assert_eq!(tip_space.name.as_deref(), Some("Tip"));
        assert_eq!(
            attr(tip_space, "title").as_deref(),
            Some("Space Title"),
            "space-form title attribute preserved"
        );

        // No diagnostics — every block is recognised and transformed.
        assert!(
            r.take_diagnostics().is_empty(),
            "no diagnostics expected for well-formed collapsed directives"
        );
    }

    #[test]
    fn real_parser_collapsed_container_bare_space_form_transforms() {
        // A bare space form (`:::tip heads up`) still TRANSFORMS to <Tip>
        // (no longer literal text) — the bug was that it stayed as a
        // `<p>:::tip…</p>` paragraph. The trailing words parse as boolean
        // attributes (engine semantics; not a title), but the key acceptance
        // is that the directive is recognised at all.
        let mut r = registry_with_admonitions();
        let out = run_real_parser(&mut r, ":::tip heads up\nbody\n:::\n");
        assert_eq!(out.len(), 1, "got {out:#?}");
        let tip = flow(&out[0]);
        assert_eq!(tip.name.as_deref(), Some("Tip"));
    }

    #[test]
    fn real_parser_blank_line_separated_form_still_transforms() {
        // The blank-line-separated form parses to SEPARATE paragraphs (the
        // shape the hand-built fixtures simulate). It must keep working
        // through the real parser too.
        let mut r = registry_with_admonitions();
        let input = "\
:::note

separated body

:::
";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1, "got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert!(r.take_diagnostics().is_empty());
    }

    #[test]
    fn real_parser_unknown_collapsed_container_left_alone_and_warned() {
        // A collapsed `:::unknown` block whose name is NOT registered must
        // stay as literal text and earn an unknown-directive warning — the
        // pre-existing unknown-directive behaviour is preserved.
        let mut r = registry_with_admonitions();
        let input = ":::nope\nbody\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1);
        assert!(
            matches!(out[0], MdastNode::Paragraph(_)),
            "unknown directive preserved as paragraph"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "unknown-directive warning expected");
        assert!(diags[0].message.contains("nope"));
    }

    // ---- recursive collapsed sibling/nested transform (Sub #1094) ----

    #[test]
    fn real_parser_collapsed_sibling_directives_both_transform() {
        // Issue #1094: two SIBLING containers collapsed with NO blank line
        // between them land in ONE multi-line Paragraph. Before the recursive
        // fix only the OUTER (first) directive transformed and the second was
        // left as literal text. Both must now become JSX flow elements.
        let mut r = registry_with_admonitions();
        let input = ":::note\nA\n:::\n:::tip\nB\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 2, "both siblings transform, got {out:#?}");

        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        let MdastNode::Paragraph(nb) = &note.children[0] else {
            unreachable!("note body Paragraph, got {:?}", note.children[0]);
        };
        let MdastNode::Text(nt) = &nb.children[0] else {
            unreachable!("note body Text");
        };
        assert_eq!(nt.value, "A");

        let tip = flow(&out[1]);
        assert_eq!(tip.name.as_deref(), Some("Tip"));
        let MdastNode::Paragraph(tb) = &tip.children[0] else {
            unreachable!("tip body Paragraph, got {:?}", tip.children[0]);
        };
        let MdastNode::Text(tt) = &tb.children[0] else {
            unreachable!("tip body Text");
        };
        assert_eq!(tt.value, "B");

        assert!(
            r.take_diagnostics().is_empty(),
            "well-formed siblings produce no diagnostics"
        );
    }

    #[test]
    fn real_parser_collapsed_nested_directive_transforms() {
        // A NESTED collapsed directive: the outer fence uses MORE colons
        // (`:::::`) per the CommonMark-Directives nesting convention, the
        // inner uses `:::`. The inner `:::tip` must transform into <Tip>
        // (no longer literal text) and live inside the outer <Note>.
        let mut r = registry_with_admonitions();
        let input = ":::::note\nbefore\n:::tip\ninner\n:::\nafter\n:::::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1, "one outer directive, got {out:#?}");

        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));

        // The outer body must contain a transformed <Tip> JSX element among
        // its children — proof the inner directive was not left as literal
        // text.
        let has_tip = note.children.iter().any(
            |c| matches!(c, MdastNode::MdxJsxFlowElement(j) if j.name.as_deref() == Some("Tip")),
        );
        assert!(
            has_tip,
            "nested <Tip> should be transformed inside <Note>, got {:#?}",
            note.children
        );

        assert!(
            r.take_diagnostics().is_empty(),
            "well-formed nested directives produce no diagnostics"
        );
    }

    #[test]
    fn real_parser_collapsed_unbalanced_run_leaves_remainder_literal() {
        // MALFORMED: a well-formed first directive followed by a second
        // opener with NO closer. The first transforms; the unbalanced
        // remainder is left as literal text — no panic, graceful fallback.
        let mut r = registry_with_admonitions();
        let input = ":::note\nA\n:::\n:::tip\nB\n";
        let out = run_real_parser(&mut r, input);

        // First directive transformed.
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));

        // The unbalanced `:::tip\nB` remainder survives as literal text
        // somewhere in the output (not transformed into a <Tip>).
        let literal: String = out
            .iter()
            .filter_map(|n| match n {
                MdastNode::Paragraph(p) => Some(
                    p.children
                        .iter()
                        .filter_map(|c| match c {
                            MdastNode::Text(t) => Some(t.value.clone()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect();
        assert!(
            literal.contains(":::tip"),
            "unbalanced opener left literal, got {out:#?}"
        );
        let has_tip = out.iter().any(
            |c| matches!(c, MdastNode::MdxJsxFlowElement(j) if j.name.as_deref() == Some("Tip")),
        );
        assert!(!has_tip, "unbalanced tip must NOT transform");
        // Since zfb#2206 the unclosed trailing opener additionally records
        // an unclosed-container diagnostic (the literal fallback stays).
        let diags = r.take_diagnostics();
        assert_eq!(
            diags.len(),
            1,
            "unclosed diagnostic expected, got {diags:#?}"
        );
        assert!(diags[0].message.contains("unclosed") && diags[0].message.contains("tip"));
    }

    #[test]
    fn real_parser_collapsed_inner_unknown_warns_once_and_left_alone() {
        // A KNOWN outer container with an UNKNOWN inner directive: the inner
        // is left as literal text and earns EXACTLY ONE unknown-directive
        // warning (no duplicate from the recursive pass + the outer re-walk).
        let mut r = registry_with_admonitions();
        let input = ":::::note\nbefore\n:::bogus\nx\n:::\nafter\n:::::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1);
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));

        // No <Bogus>/<bogus> transform happened.
        let has_jsx_inner = note.children.iter().any(
            |c| matches!(c, MdastNode::MdxJsxFlowElement(j) if j.name.as_deref() != Some("Tip")),
        );
        assert!(!has_jsx_inner, "unknown inner must stay literal");

        let diags = r.take_diagnostics();
        assert_eq!(
            diags.len(),
            1,
            "exactly one unknown-directive warning, got {diags:#?}"
        );
        assert!(diags[0].message.contains("bogus"));
    }

    #[test]
    fn real_parser_collapsed_unknown_with_extra_colons_warns_once() {
        // A top-level UNKNOWN collapsed container with MORE than 3 colons
        // (`:::::bogus … :::::`) must still warn exactly once and be left as
        // literal text — the exactly-3-colon block scan alone would miss it.
        let mut r = registry_with_admonitions();
        let out = run_real_parser(&mut r, ":::::bogus\nx\n:::::\n");
        let has_jsx = out
            .iter()
            .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_)));
        assert!(
            !has_jsx,
            "unknown directive must not transform, got {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "exactly one warning, got {diags:#?}");
        assert!(diags[0].message.contains("bogus"));
    }

    #[test]
    fn real_parser_collapsed_known_then_unknown_sibling_warns_once() {
        // A KNOWN sibling followed by an UNKNOWN sibling in one collapsed run:
        // the known transforms, the unknown is left literal and warns exactly
        // once (no duplicate, no lost warning).
        let mut r = registry_with_admonitions();
        let out = run_real_parser(&mut r, ":::note\nA\n:::\n:::bogus\nB\n:::\n");
        let note_count = out
            .iter()
            .filter(
                |c| matches!(c, MdastNode::MdxJsxFlowElement(j) if j.name.as_deref() == Some("Note")),
            )
            .count();
        assert_eq!(note_count, 1, "known sibling transforms, got {out:#?}");
        let has_other_jsx = out.iter().any(
            |c| matches!(c, MdastNode::MdxJsxFlowElement(j) if j.name.as_deref() != Some("Note")),
        );
        assert!(!has_other_jsx, "unknown sibling must not transform");
        let diags = r.take_diagnostics();
        assert_eq!(
            diags.len(),
            1,
            "exactly one unknown-directive warning, got {diags:#?}"
        );
        assert!(diags[0].message.contains("bogus"));
    }

    #[test]
    fn real_parser_collapsed_unknown_outer_keeps_known_inner_literal() {
        // zfb#1094 review C1: an UNKNOWN outer fence wrapping a KNOWN inner
        // directive must keep the WHOLE run literal — the inner `:::note` must
        // NOT leak out transformed mid-prose (splitting the literal
        // `:::::bogus`/`:::::` around a real <Note>). Before the fix the unknown
        // branch fell through into the body and re-scanned the inner opener.
        let mut r = registry_with_admonitions();
        let out = run_real_parser(&mut r, ":::::bogus\n:::note\nA\n:::\n:::::\n");
        let has_jsx = out
            .iter()
            .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_)));
        assert!(
            !has_jsx,
            "inner known directive must stay literal inside an unknown wrapper, got {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(
            diags.len(),
            1,
            "exactly one unknown warning, got {diags:#?}"
        );
        assert!(diags[0].message.contains("bogus"));
    }

    #[test]
    fn real_parser_collapsed_closer_with_more_colons_still_closes() {
        // zfb#1094 review C2 (pin behavior): a closer with MORE colons than its
        // opener still closes it — the colon-count rule is "a closer of k
        // colons closes the innermost open fence whose opener colon-count
        // <= k". This leniency is what makes nested fences (outer uses more
        // colons) work, so `:::note\nA\n:::::` transforms to <Note> rather than
        // being left literal. Documented + pinned so it is intended, not
        // incidental.
        let mut r = registry_with_admonitions();
        let out = run_real_parser(&mut r, ":::note\nA\n:::::\n");
        assert_eq!(out.len(), 1, "got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert!(
            r.take_diagnostics().is_empty(),
            "lenient closer is accepted without diagnostics"
        );
    }

    // ---- multi-block bodies, glued fences, backtick titles (zfb#2206) ----

    /// Collect the plain-text content of each top-level Paragraph child of a
    /// JSX flow element (InlineCode contributes its value; other inline
    /// markup contributes its nested Text content).
    fn body_paragraph_texts(j: &MdxJsxFlowElement) -> Vec<String> {
        fn inline_text(nodes: &[MdastNode]) -> String {
            let mut s = String::new();
            for n in nodes {
                match n {
                    MdastNode::Text(t) => s.push_str(&t.value),
                    MdastNode::InlineCode(c) => s.push_str(&c.value),
                    other => {
                        if let Some(children) = other.children() {
                            s.push_str(&inline_text(children));
                        }
                    }
                }
            }
            s
        }
        j.children
            .iter()
            .filter_map(|c| match c {
                MdastNode::Paragraph(p) => Some(inline_text(&p.children)),
                _ => None,
            })
            .collect()
    }

    /// Recursively collect every Text value in the tree so tests can assert
    /// no literal `:::` fence leaked anywhere in the output.
    fn collect_text_values(nodes: &[MdastNode], out: &mut Vec<String>) {
        for n in nodes {
            if let MdastNode::Text(t) = n {
                out.push(t.value.clone());
            }
            if let Some(children) = n.children() {
                collect_text_values(children, out);
            }
        }
    }

    fn assert_no_literal_fence(nodes: &[MdastNode]) {
        let mut texts = Vec::new();
        collect_text_values(nodes, &mut texts);
        assert!(
            !texts.iter().any(|t| t.contains(":::")),
            "no literal ::: may leak into the output, got texts: {texts:#?}"
        );
    }

    #[test]
    fn real_parser_blank_line_inside_body_transforms() {
        // zfb#2206 repro form A: a blank line inside the body glues the
        // opener to the first body block and the closer to the last one.
        // Both body paragraphs must survive inside ONE <Warning>.
        let mut r = registry_with_admonitions();
        let input = ":::warning\nalpha one\n\nalpha two\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1, "one directive expected, got {out:#?}");
        let warning = flow(&out[0]);
        assert_eq!(warning.name.as_deref(), Some("Warning"));
        assert_eq!(
            body_paragraph_texts(warning),
            vec!["alpha one".to_string(), "alpha two".to_string()],
            "both body blocks must survive, got {:#?}",
            warning.children
        );
        assert_no_literal_fence(&out);
        assert!(
            r.take_diagnostics().is_empty(),
            "well-formed: no diagnostics"
        );
    }

    #[test]
    fn real_parser_glued_opener_padded_closer_keeps_first_body_block() {
        // The opener is glued to the first body line but the closer is
        // padded. Pre-#2206 this TRANSFORMED but silently dropped the glued
        // "body one" line together with the opener paragraph.
        let mut r = registry_with_admonitions();
        let input = ":::warning\nbody one\n\nbody two\n\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1, "one directive expected, got {out:#?}");
        let warning = flow(&out[0]);
        assert_eq!(warning.name.as_deref(), Some("Warning"));
        assert_eq!(
            body_paragraph_texts(warning),
            vec!["body one".to_string(), "body two".to_string()],
            "the glued first body line must not be dropped, got {:#?}",
            warning.children
        );
        assert!(r.take_diagnostics().is_empty());
    }

    #[test]
    fn real_parser_padded_opener_glued_closer_transforms() {
        // The opener is padded but the closer is glued to the last body
        // block. Pre-#2206 the glued closer was never recognised, so the
        // whole run leaked literal (or stole a later directive's closer).
        let mut r = registry_with_admonitions();
        let input = ":::warning\n\nbody one\n\nbody two\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1, "one directive expected, got {out:#?}");
        let warning = flow(&out[0]);
        assert_eq!(warning.name.as_deref(), Some("Warning"));
        assert_eq!(
            body_paragraph_texts(warning),
            vec!["body one".to_string(), "body two".to_string()],
            "both body blocks must survive, got {:#?}",
            warning.children
        );
        assert_no_literal_fence(&out);
        assert!(r.take_diagnostics().is_empty());
    }

    #[test]
    fn real_parser_backtick_bracket_title_normalizes_to_plain_text() {
        // zfb#2206 repro form B: inline code in the bracketed title splits
        // the opener line across several inline children. The directive must
        // transform, with the title normalized to plain text in the `title`
        // attribute and NO title fragment leaking into the body.
        let mut r = registry_with_admonitions();
        let input = ":::warning[`Evidence:` in the title]\nbravo body\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1, "one directive expected, got {out:#?}");
        let warning = flow(&out[0]);
        assert_eq!(warning.name.as_deref(), Some("Warning"));
        assert_eq!(
            attr(warning, "title").as_deref(),
            Some("Evidence: in the title"),
            "backticked bracket title must normalize to plain text"
        );
        assert_eq!(
            body_paragraph_texts(warning),
            vec!["bravo body".to_string()],
            "no title fragment may leak into the body, got {:#?}",
            warning.children
        );
        assert_no_literal_fence(&out);
        assert!(r.take_diagnostics().is_empty());
    }

    #[test]
    fn real_parser_cascade_broken_form_does_not_steal_later_closer() {
        // zfb#2206 root cause 3: pre-fix, the glued-closer form A opener
        // stole the standalone `:::` of the NEXT directive, swallowing
        // everything between (cascade). Both directives must transform
        // independently with their own bodies.
        let mut r = registry_with_admonitions();
        let input = "\
:::warning
alpha one

alpha two
:::

:::note

padded body

:::
";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 2, "two directives expected, got {out:#?}");
        let warning = flow(&out[0]);
        assert_eq!(warning.name.as_deref(), Some("Warning"));
        assert_eq!(
            body_paragraph_texts(warning),
            vec!["alpha one".to_string(), "alpha two".to_string()]
        );
        let note = flow(&out[1]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert_eq!(body_paragraph_texts(note), vec!["padded body".to_string()]);
        assert_no_literal_fence(&out);
        assert!(r.take_diagnostics().is_empty());
    }

    #[test]
    fn real_parser_unclosed_opener_warns_and_stays_literal() {
        // A genuinely-unclosed container opener must emit a build
        // diagnostic (zfb#2206 acceptance) instead of leaking silently.
        // The source stays literal — graceful fallback, no transform.
        let mut r = registry_with_admonitions();
        let input = ":::warning\nnever closed\n\nplain trailing paragraph\n";
        let out = run_real_parser(&mut r, input);
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "unclosed opener must not transform, got {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("warning"),
            "diagnostic must name the unclosed directive, got {:?}",
            diags[0].message
        );
        assert_eq!(diags[0].line, Some(1), "diagnostic carries the opener line");
    }

    #[test]
    fn real_parser_unclosed_opener_leaves_later_padded_directive_intact() {
        // Cascade half 2: a genuinely-unclosed opener followed by a
        // well-formed padded directive. Pre-fix the unclosed opener stole
        // the later directive's closer; now the later directive must render
        // and the unclosed one stays literal with a diagnostic.
        let mut r = registry_with_admonitions();
        let input = "\
:::warning
never closed

:::note

padded body

:::
";
        let out = run_real_parser(&mut r, input);
        let notes: Vec<&MdxJsxFlowElement> = out
            .iter()
            .filter_map(|c| match c {
                MdastNode::MdxJsxFlowElement(j) => Some(j),
                _ => None,
            })
            .collect();
        assert_eq!(
            notes.len(),
            1,
            "exactly the later <Note> transforms, got {out:#?}"
        );
        assert_eq!(notes[0].name.as_deref(), Some("Note"));
        assert_eq!(
            body_paragraph_texts(notes[0]),
            vec!["padded body".to_string()]
        );
        // The unclosed opener survives as literal text.
        let mut texts = Vec::new();
        collect_text_values(&out, &mut texts);
        assert!(
            texts.iter().any(|t| t.contains(":::warning")),
            "unclosed opener stays literal, got {texts:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(diags[0].message.contains("unclosed"));
    }

    #[test]
    fn real_parser_unclosed_opener_glued_before_directive_warns_and_stays_literal() {
        // zfb#2212: an unclosed opener glued DIRECTLY (no blank lines)
        // above a valid collapsed run leaks as a literal prose segment
        // BEFORE the transformed run. The pre-fix splice check inspected
        // only the FINAL replacement node for an opener shape, so this
        // shape lost its unclosed diagnostic entirely — a silent literal
        // leak. Transformation semantics are pinned unchanged: the
        // trailing `:::note` still transforms and the `:::warning`
        // opener stays a literal leak.
        let mut r = registry_with_admonitions();
        let input = ":::warning\nnever closed\n:::note\nbody\n:::\n";
        let out = run_real_parser(&mut r, input);

        let jsx_names: Vec<&str> = out
            .iter()
            .filter_map(|c| match c {
                MdastNode::MdxJsxFlowElement(j) => j.name.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(
            jsx_names,
            vec!["Note"],
            "only the trailing note transforms, got {out:#?}"
        );
        let note = flow(
            out.iter()
                .find(|c| matches!(c, MdastNode::MdxJsxFlowElement(_)))
                .expect("note JSX present"),
        );
        assert_eq!(body_paragraph_texts(note), vec!["body".to_string()]);

        let mut texts = Vec::new();
        collect_text_values(&out, &mut texts);
        assert!(
            texts.iter().any(|t| t.contains(":::warning")),
            "unclosed opener stays literal, got {texts:#?}"
        );

        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("warning"),
            "diagnostic names the leaked opener, got {:?}",
            diags[0].message
        );
    }

    #[test]
    fn real_parser_unclosed_opener_glued_between_directives_warns_once() {
        // zfb#2212 companion shape: the leaked opener sits BETWEEN two
        // valid collapsed runs — neither the first nor the final
        // replacement node. Both runs transform; the middle `:::warning`
        // leaks literally with exactly ONE unclosed diagnostic (the
        // whole-segment scan must not double-report, and the JSX
        // segments must not confuse it).
        let mut r = registry_with_admonitions();
        let input = ":::note\nalpha\n:::\n:::warning\nnever closed\n:::tip\nbravo\n:::\n";
        let out = run_real_parser(&mut r, input);

        let jsx_names: Vec<&str> = out
            .iter()
            .filter_map(|c| match c {
                MdastNode::MdxJsxFlowElement(j) => j.name.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(
            jsx_names,
            vec!["Note", "Tip"],
            "both well-formed runs transform, got {out:#?}"
        );

        let mut texts = Vec::new();
        collect_text_values(&out, &mut texts);
        assert!(
            texts.iter().any(|t| t.contains(":::warning")),
            "unclosed opener stays literal, got {texts:#?}"
        );

        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("warning"),
            "diagnostic names the leaked opener, got {:?}",
            diags[0].message
        );
    }

    #[test]
    fn real_parser_buried_unclosed_opener_behind_prose_warns_and_stays_literal() {
        // zfb#2211: an unclosed opener buried BEHIND a non-opener first
        // line (markdown-rs glues the fence line into the prose paragraph
        // when blank lines are missing). No head-level path engages such a
        // paragraph, so pre-fix the `:::warning` leaked literally with NO
        // diagnostic. Diagnostic-only contract: the paragraph still leaks
        // literally, exactly as before.
        let mut r = registry_with_admonitions();
        let input = "some prose\n:::warning\nnever closed\n\nplain trailing paragraph\n";
        let out = run_real_parser(&mut r, input);
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "buried unclosed opener must not transform, got {out:#?}"
        );
        let mut texts = Vec::new();
        collect_text_values(&out, &mut texts);
        assert!(
            texts.iter().any(|t| t.contains(":::warning")),
            "buried opener stays literal, got {texts:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("warning"),
            "diagnostic names the buried opener, got {:?}",
            diags[0].message
        );
        assert_eq!(
            diags[0].line,
            Some(2),
            "diagnostic points at the buried fence line, not the paragraph head"
        );
    }

    #[test]
    fn real_parser_buried_unclosed_opener_in_multi_inline_paragraph_warns() {
        // Same buried shape, but the first line carries inline markup so
        // the paragraph splits into several inline children — the
        // detection must work on the InlineLine view, not just the
        // single-Text collapsed form.
        let mut r = registry_with_admonitions();
        let input = "prose with *emphasis* here\n:::warning\nnever closed\n";
        let out = run_real_parser(&mut r, input);
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "buried unclosed opener must not transform, got {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("warning"),
            "diagnostic names the buried opener, got {:?}",
            diags[0].message
        );
        assert_eq!(diags[0].line, Some(2), "buried fence line attached");
    }

    #[test]
    fn real_parser_buried_unclosed_opener_after_hard_break_warns() {
        // A hard break (two trailing spaces) before the buried opener puts
        // the fence text at the START of a later `Text` child instead of
        // behind a `\n` inside one — the pre-filter's second arm. Without
        // it this variant would silently skip the scan.
        let mut r = registry_with_admonitions();
        let input = "some prose  \n:::warning\nnever closed\n";
        let out = run_real_parser(&mut r, input);
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "buried unclosed opener must not transform, got {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("warning"),
            "diagnostic names the buried opener, got {:?}",
            diags[0].message
        );
        assert_eq!(diags[0].line, Some(2), "buried fence line attached");
    }

    #[test]
    fn real_parser_buried_unclosed_opener_before_later_directive_warns_and_leaves_it_intact() {
        // The buried unclosed opener is followed by a WELL-FORMED
        // directive. The later directive's own closer must not suppress
        // the buried diagnostic (the sibling closer scan stops at the next
        // opener-shaped paragraph, mirroring the head-opener bound), and
        // the later directive must still transform untouched.
        let mut r = registry_with_admonitions();
        let input = "some prose\n:::warning\nnever closed\n\n:::note\nbody\n:::\n";
        let out = run_real_parser(&mut r, input);
        let jsx_names: Vec<&str> = out
            .iter()
            .filter_map(|c| match c {
                MdastNode::MdxJsxFlowElement(j) => j.name.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(
            jsx_names,
            vec!["Note"],
            "only the later note transforms, got {out:#?}"
        );
        let mut texts = Vec::new();
        collect_text_values(&out, &mut texts);
        assert!(
            texts.iter().any(|t| t.contains(":::warning")),
            "buried opener stays literal, got {texts:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("warning"),
            "diagnostic names the buried opener, got {:?}",
            diags[0].message
        );
    }

    #[test]
    fn real_parser_head_and_buried_unclosed_openers_warn_once_each() {
        // A buried unclosed opener in a prose paragraph AND a head-level
        // unclosed opener in the next paragraph: exactly ONE diagnostic
        // per unclosed opener — the buried scan must not re-report the
        // head shape the block-level path already records.
        let mut r = registry_with_admonitions();
        let input =
            "intro\n:::warning\nnever closed\n\n:::tip\nalso never closed\n\ntrailing prose\n";
        let out = run_real_parser(&mut r, input);
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "nothing transforms, got {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(
            diags.len(),
            2,
            "one diagnostic per unclosed opener, got {diags:#?}"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("unclosed") && d.message.contains("warning")),
            "the buried opener warns, got {diags:#?}"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("unclosed") && d.message.contains("tip")),
            "the head opener still warns, got {diags:#?}"
        );
    }

    #[test]
    fn real_parser_buried_unclosed_opener_in_collapsed_run_tail_warns() {
        // zfb#2211 companion: the buried shape can also arrive as a
        // collapsed run's TRAILING prose segment — a recognised run
        // followed by prose gluing an unclosed opener behind a non-opener
        // first line. The recognised run transforms; the tail's buried
        // opener records exactly one unclosed diagnostic and keeps
        // leaking literally.
        let mut r = registry_with_admonitions();
        let input = ":::note\nalpha\n:::\nprose\n:::warning\nnever closed\n";
        let out = run_real_parser(&mut r, input);
        let jsx_names: Vec<&str> = out
            .iter()
            .filter_map(|c| match c {
                MdastNode::MdxJsxFlowElement(j) => j.name.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(
            jsx_names,
            vec!["Note"],
            "the recognised run still transforms, got {out:#?}"
        );
        let mut texts = Vec::new();
        collect_text_values(&out, &mut texts);
        assert!(
            texts.iter().any(|t| t.contains(":::warning")),
            "buried opener stays literal, got {texts:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("warning"),
            "diagnostic names the buried opener, got {:?}",
            diags[0].message
        );
    }

    #[test]
    fn real_parser_buried_opener_with_glued_closer_stays_literal_and_silent() {
        // NEGATIVE (zfb#2211): a buried opener whose closer lives in the
        // SAME glued paragraph is NOT unclosed — no diagnostic. Today's
        // transform behavior for this malformed shape is pinned
        // unchanged: nothing transforms and both fence lines leak
        // literally.
        let mut r = registry_with_admonitions();
        let input = "some prose\n:::warning\nbody\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "buried-but-closed shape does not transform today (pinned), got {out:#?}"
        );
        let mut texts = Vec::new();
        collect_text_values(&out, &mut texts);
        assert!(
            texts.iter().any(|t| t.contains(":::warning")),
            "the buried run stays literal, got {texts:#?}"
        );
        let diags = r.take_diagnostics();
        assert!(
            diags.is_empty(),
            "a closed buried opener must not warn, got {diags:#?}"
        );
    }

    #[test]
    fn real_parser_buried_opener_with_sibling_closer_stays_silent() {
        // NEGATIVE (zfb#2211): the buried opener's closer sits in a LATER
        // sibling paragraph. The sibling scan mirrors the head-opener
        // shape-(b) bounds, so the bare closer suppresses the diagnostic;
        // a directive AFTER that closer still transforms untouched.
        let mut r = registry_with_admonitions();
        let input = "some prose\n:::warning\nbody\n\n:::\n\n:::note\nreal\n:::\n";
        let out = run_real_parser(&mut r, input);
        let jsx_names: Vec<&str> = out
            .iter()
            .filter_map(|c| match c {
                MdastNode::MdxJsxFlowElement(j) => j.name.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(
            jsx_names,
            vec!["Note"],
            "the later note still transforms, got {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert!(
            diags.is_empty(),
            "a sibling closer suppresses the buried diagnostic, got {diags:#?}"
        );
    }

    #[test]
    fn real_parser_buried_opener_not_suppressed_by_sibling_buried_run_closer() {
        // Codex review of zfb#2211: a FOLLOWING sibling paragraph whose
        // own BURIED directive run supplies the closer must not suppress
        // the earlier buried opener's diagnostic — that closer belongs to
        // the sibling's opener (`text` / `:::note` / `body` / `:::`), not
        // ours. Pre-fix the sibling scan only stopped at opener-shaped
        // paragraph HEADS, so the bare first-closer scan grabbed the note's
        // closer and silenced the warning. The sibling's own CLOSED
        // buried run stays silent (per the glued-closer negative pin), so
        // exactly ONE diagnostic fires — for `:::warning`.
        let mut r = registry_with_admonitions();
        let input = "prose\n:::warning\nnever closed\n\ntext\n:::note\nbody\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "nothing transforms (both shapes are buried), got {out:#?}"
        );
        let mut texts = Vec::new();
        collect_text_values(&out, &mut texts);
        assert!(
            texts.iter().any(|t| t.contains(":::warning"))
                && texts.iter().any(|t| t.contains(":::note")),
            "both buried runs stay literal, got {texts:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed")
                && diags[0].message.contains("warning")
                && !diags[0].message.contains("note"),
            "the diagnostic names the genuinely-unclosed opener, got {:?}",
            diags[0].message
        );
    }

    #[test]
    fn real_parser_consecutive_buried_unclosed_openers_warn_once_each() {
        // Companion pin for the fix above: when the sibling's buried
        // opener is itself UNCLOSED, stopping our sibling scan at it must
        // not eat the sibling's OWN diagnostic — each paragraph warns for
        // its own buried opener, exactly once each.
        let mut r = registry_with_admonitions();
        let input = "prose\n:::warning\nnever closed\n\ntext\n:::note\nalso never closed\n";
        let out = run_real_parser(&mut r, input);
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "nothing transforms, got {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(
            diags.len(),
            2,
            "one diagnostic per buried unclosed opener, got {diags:#?}"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("unclosed") && d.message.contains("warning")),
            "the first paragraph's opener warns, got {diags:#?}"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("unclosed") && d.message.contains("note")),
            "the sibling's own opener warns too, got {diags:#?}"
        );
    }

    #[test]
    fn real_parser_backtick_fence_line_is_not_a_buried_opener() {
        // NEGATIVE (zfb#2211): a code span rendering as `:::warning` is
        // PHRASING, not a fence line — the buried-opener scan must not
        // mistake it (the InlineLine's leading node is InlineCode, never
        // a fence-bearing Text).
        let mut r = registry_with_admonitions();
        let out = run_real_parser(&mut r, "some prose\n`:::warning`\nnever closed\n");
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "nothing transforms, got {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert!(
            diags.is_empty(),
            "a backtick code span must not warn as an opener, got {diags:#?}"
        );

        // Discriminating case: a code-span `:::note` line ABOVE a real
        // buried unclosed `:::warning`. The one diagnostic must name the
        // real fence (warning), never the phrasing (note) — proving the
        // code-span line is skipped rather than accidentally pre-filtered.
        let mut r = registry_with_admonitions();
        let out = run_real_parser(&mut r, "prose\n`:::note`\nx\n:::warning\nnever closed\n");
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "nothing transforms, got {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("warning") && !diags[0].message.contains("note"),
            "the diagnostic names the real fence, not the code span, got {:?}",
            diags[0].message
        );
    }

    #[test]
    fn real_parser_buried_unregistered_or_off_form_openers_stay_silent() {
        // NEGATIVE (zfb#2211): only REGISTERED container names in the
        // exact 3-colon form warn (mirroring the head gates). A buried
        // unknown name and a buried 4-colon opener both stay silent — and
        // keep leaking literally.
        let mut r = registry_with_admonitions();
        let out = run_real_parser(&mut r, "prose\n:::bogus\nnever closed\n");
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "nothing transforms, got {out:#?}"
        );
        assert!(
            r.take_diagnostics().is_empty(),
            "unknown buried name must not warn"
        );

        let mut r = registry_with_admonitions();
        let out = run_real_parser(&mut r, "prose\n::::warning\nnever closed\n");
        assert!(
            !out.iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "nothing transforms, got {out:#?}"
        );
        assert!(
            r.take_diagnostics().is_empty(),
            "a 4-colon buried opener is outside the exact-3-colon gate"
        );
    }

    #[test]
    fn real_parser_closer_with_trailing_prose_keeps_prose_after_directive() {
        // Prose glued AFTER the closing `:::` on the same paragraph must be
        // re-emitted after the directive, not silently dropped (pre-#2206
        // the whole closer paragraph vanished with its trailing lines).
        let mut r = registry_with_admonitions();
        let input = ":::note\nbody\n\n:::\nmore text\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 2, "directive + trailing prose, got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert_eq!(body_paragraph_texts(note), vec!["body".to_string()]);
        let MdastNode::Paragraph(p) = &out[1] else {
            unreachable!("trailing prose paragraph expected, got {:?}", out[1]);
        };
        let MdastNode::Text(t) = &p.children[0] else {
            unreachable!("trailing prose text expected");
        };
        assert_eq!(t.value, "more text");
        assert!(r.take_diagnostics().is_empty());
    }

    #[test]
    fn real_parser_emphasis_in_collapsed_body_preserves_phrasing() {
        // Parity pin for the multi-inline-child collapsed shape: emphasis in
        // the body splits the paragraph into several inline children. The
        // body must keep its phrasing nodes (NOT flattened to plain text).
        let mut r = registry_with_admonitions();
        let input = ":::note\nbody *em* text\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1, "got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        let MdastNode::Paragraph(body) = &note.children[0] else {
            unreachable!("body paragraph expected, got {:?}", note.children[0]);
        };
        assert!(
            body.children
                .iter()
                .any(|c| matches!(c, MdastNode::Emphasis(_))),
            "emphasis phrasing must survive in the body, got {:#?}",
            body.children
        );
        assert_eq!(body_paragraph_texts(note), vec!["body em text".to_string()]);
        assert!(r.take_diagnostics().is_empty());
    }

    #[test]
    fn real_parser_backtick_title_followed_by_padded_directive_no_steal() {
        // Form B (backtick title, collapsed) followed by a padded sibling:
        // the collapsed run must close inside its own paragraph and never
        // reach for the padded sibling's closer.
        let mut r = registry_with_admonitions();
        let input = "\
:::warning[`Evidence:` in the title]
bravo body
:::

:::note

padded body

:::
";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 2, "two directives expected, got {out:#?}");
        let warning = flow(&out[0]);
        assert_eq!(warning.name.as_deref(), Some("Warning"));
        assert_eq!(
            attr(warning, "title").as_deref(),
            Some("Evidence: in the title")
        );
        assert_eq!(
            body_paragraph_texts(warning),
            vec!["bravo body".to_string()]
        );
        let note = flow(&out[1]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert_eq!(body_paragraph_texts(note), vec!["padded body".to_string()]);
        assert_no_literal_fence(&out);
        assert!(r.take_diagnostics().is_empty());
    }

    #[test]
    fn real_parser_hard_break_in_collapsed_body_survives() {
        // Codex review (zfb#2206): a CommonMark hard break (two trailing
        // spaces) in a collapsed body produces a Break node. The line
        // machinery must restore it verbatim on rejoin — not degrade it to
        // a soft newline (the pre-#2206 clone path preserved it).
        let mut r = registry_with_admonitions();
        let input = ":::note\nfoo  \nbar\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1, "got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        let MdastNode::Paragraph(body) = &note.children[0] else {
            unreachable!("body paragraph expected, got {:?}", note.children[0]);
        };
        assert!(
            body.children
                .iter()
                .any(|c| matches!(c, MdastNode::Break(_))),
            "the hard Break node must survive in the body, got {:#?}",
            body.children
        );
        assert!(r.take_diagnostics().is_empty());
    }

    #[test]
    fn real_parser_emphasis_spanning_soft_break_in_body_transforms() {
        // Codex review (zfb#2206): inline markup spanning a soft break
        // (`*foo\nbar*`) nests a newline inside the emphasis node. The
        // directive must still transform with the phrasing intact — the
        // nested newline only disqualifies OPENER-line flattening, never
        // the body (the pre-#2206 clone path transformed this shape).
        let mut r = registry_with_admonitions();
        let input = ":::note\n*foo\nbar*\n:::\n";
        let out = run_real_parser(&mut r, input);
        assert_eq!(out.len(), 1, "got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        let MdastNode::Paragraph(body) = &note.children[0] else {
            unreachable!("body paragraph expected, got {:?}", note.children[0]);
        };
        assert!(
            body.children
                .iter()
                .any(|c| matches!(c, MdastNode::Emphasis(_))),
            "emphasis spanning the soft break must survive, got {:#?}",
            body.children
        );
        assert_no_literal_fence(&out);
        assert!(
            r.take_diagnostics().is_empty(),
            "no false unclosed diagnostic for a well-formed body"
        );
    }

    #[test]
    fn oracle_parser_and_registry_agree_on_2206_repro_forms() {
        // Divergence guard (zfb#2206): the oracle-pinned source-level parser
        // (`crate::directive_parser`) and the production registry must both
        // recognise the three repro forms — one container each.
        use crate::directive_parser::{parse_directive_mdast, DirectiveMdastNode};
        use crate::facade::{GfmOptions, ParseDialect, ParseMdastOptions};

        fn count_containers(node: &DirectiveMdastNode) -> usize {
            let own = usize::from(node.kind == "containerDirective");
            own + node
                .children
                .as_ref()
                .map_or(0, |c| c.iter().map(count_containers).sum())
        }

        let forms = [
            ":::warning\nalpha one\n\nalpha two\n:::\n",
            ":::warning[`Evidence:` in the title]\nbravo body\n:::\n",
            ":::warning[plain title here]\ncharlie body\n:::\n",
        ];
        for src in forms {
            let opts = ParseMdastOptions {
                dialect: ParseDialect::Markdown,
                gfm: GfmOptions::default(),
                frontmatter: false,
            };
            let root = parse_directive_mdast(opts, src).expect("oracle parses the form");
            assert_eq!(
                count_containers(&root),
                1,
                "oracle must claim one containerDirective for {src:?}"
            );

            let mut r = registry_with_admonitions();
            let out = run_real_parser(&mut r, src);
            let jsx = out
                .iter()
                .filter(|c| matches!(c, MdastNode::MdxJsxFlowElement(_)))
                .count();
            assert_eq!(jsx, 1, "registry must transform {src:?}, got {out:#?}");
            assert!(
                r.take_diagnostics().is_empty(),
                "no diagnostics for {src:?}"
            );
        }
    }

    // ---- container tests ----

    #[test]
    fn container_with_attributes() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("card", "Card"));
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::card{title=\"x\" variant=\"outline\"}"),
                text_para("body"),
                text_para(":::"),
            ],
        );
        assert_eq!(out.len(), 1);
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Card"));
        assert_eq!(attr(j, "title").as_deref(), Some("x"));
        assert_eq!(attr(j, "variant").as_deref(), Some("outline"));
        assert_eq!(j.children.len(), 1, "body paragraph preserved");
    }

    #[test]
    fn container_with_unbraced_legacy_attributes() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("details", "Details"));
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::details title=\"Click me\""),
                text_para("hidden"),
                text_para(":::"),
            ],
        );
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Details"));
        assert_eq!(attr(j, "title").as_deref(), Some("Click me"));
    }

    #[test]
    fn attribute_value_with_double_quotes_round_trips_via_literal() {
        // The parser sees `title="he said \"hi\""` and unescapes to
        // `he said "hi"`. The downstream JSX emitter is responsible for
        // re-escaping `"` → `&quot;` when serialising.
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("card", "Card"));
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::card{title=\"he said \\\"hi\\\"\"}"),
                text_para("body"),
                text_para(":::"),
            ],
        );
        let j = flow(&out[0]);
        assert_eq!(attr(j, "title").as_deref(), Some("he said \"hi\""));
    }

    #[test]
    fn hyphenated_attribute_keys_round_trip_verbatim() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("card", "Card"));
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::card{data-foo=\"1\" aria-label=\"ok\"}"),
                text_para("body"),
                text_para(":::"),
            ],
        );
        let j = flow(&out[0]);
        assert_eq!(attr(j, "data-foo").as_deref(), Some("1"));
        assert_eq!(attr(j, "aria-label").as_deref(), Some("ok"));
    }

    // ---- leaf tests ----

    #[test]
    fn leaf_with_label_and_attrs() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::leaf("badge", "Badge"));
        let out = run_with_registry(&mut r, vec![text_para("::badge[Label]{variant=success}")]);
        assert_eq!(out.len(), 1);
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Badge"));
        assert_eq!(attr(j, "variant").as_deref(), Some("success"));
        // Label becomes the only Text child.
        assert_eq!(j.children.len(), 1);
        if let MdastNode::Text(t) = &j.children[0] {
            assert_eq!(t.value, "Label");
        } else {
            unreachable!("expected Text child, got {:?}", j.children[0]);
        }
    }

    // ---- text directive tests ----

    #[test]
    fn text_directive_inline_in_paragraph() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::text("kbd", "Kbd"));
        let out = run_with_registry(
            &mut r,
            vec![MdastNode::Paragraph(Paragraph {
                children: vec![MdastNode::Text(Text {
                    value: "Press :kbd[Ctrl+S] to save".to_string(),
                    position: None,
                })],
                position: None,
            })],
        );
        let MdastNode::Paragraph(Paragraph { children, .. }) = &out[0] else {
            unreachable!("expected paragraph, got {:?}", out[0]);
        };
        // children: [Text("Press "), MdxJsxTextElement(<Kbd>), Text(" to save")]
        assert_eq!(children.len(), 3);
        if let MdastNode::Text(t) = &children[0] {
            assert_eq!(t.value, "Press ");
        } else {
            unreachable!("expected leading Text");
        }
        let j = text(&children[1]);
        assert_eq!(j.name.as_deref(), Some("Kbd"));
        assert_eq!(j.children.len(), 1);
        if let MdastNode::Text(t) = &children[2] {
            assert_eq!(t.value, " to save");
        } else {
            unreachable!("expected trailing Text");
        }
    }

    #[test]
    fn text_directive_passes_attributes() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::text("link", "Link"));
        let out = run_with_registry(
            &mut r,
            vec![MdastNode::Paragraph(Paragraph {
                children: vec![MdastNode::Text(Text {
                    value: "see :link[here]{href=\"/x\" data-id=\"7\"}.".to_string(),
                    position: None,
                })],
                position: None,
            })],
        );
        let MdastNode::Paragraph(Paragraph { children, .. }) = &out[0] else {
            unreachable!("expected MdastNode::Paragraph")
        };
        let j = text(&children[1]);
        assert_eq!(j.name.as_deref(), Some("Link"));
        assert_eq!(text_attr(j, "href").as_deref(), Some("/x"));
        assert_eq!(text_attr(j, "data-id").as_deref(), Some("7"));
    }

    // ---- diagnostics ----

    #[test]
    fn unknown_container_emits_warning_and_keeps_source() {
        let mut r = DirectiveRegistry::new();
        // No registrations.
        let out = run_with_registry(
            &mut r,
            vec![text_para(":::nope"), text_para("body"), text_para(":::")],
        );
        // Source preserved.
        assert_eq!(out.len(), 3);
        // Warning recorded.
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("nope"),
            "diag should mention name, got {:?}",
            diags[0].message
        );
    }

    #[test]
    fn unknown_text_directive_emits_warning_and_keeps_source() {
        let mut r = DirectiveRegistry::new();
        // Need at least one Text-kind directive to engage the inline
        // scanner.
        r.register(DirectiveDef::text("kbd", "Kbd"));
        let out = run_with_registry(
            &mut r,
            vec![MdastNode::Paragraph(Paragraph {
                children: vec![MdastNode::Text(Text {
                    value: "see :unknownx[bar] please".to_string(),
                    position: None,
                })],
                position: None,
            })],
        );
        let MdastNode::Paragraph(Paragraph { children, .. }) = &out[0] else {
            unreachable!("expected MdastNode::Paragraph")
        };
        // No JSX node — text preserved verbatim. Either as a single
        // unchanged Text node, or split but still semantically the
        // same string.
        let collected: String = children
            .iter()
            .filter_map(|c| match c {
                MdastNode::Text(t) => Some(t.value.clone()),
                _ => None,
            })
            .collect();
        assert!(
            collected.contains(":unknownx[bar]"),
            "text preserved, got: {collected:?}"
        );
        assert!(!r.take_diagnostics().is_empty(), "warning recorded");
    }

    #[test]
    fn context_armed_visit_flushes_diagnostics_to_sink() {
        // zfb#2206: with a BuildContext diagnostics sink armed (real
        // builds — the JSX-emit path drains the sink into the pipeline's
        // markdown-diagnostics counters), directive diagnostics flow out
        // as Warning-severity MarkdownDiagnostics with the source path and
        // position attached, and the registry buffer is drained.
        use zfb_md_ast::diagnostics::CollectingSink;

        let mut r = registry_with_admonitions();
        let input = ":::warning\nnever closed\n\n:::bogus\n\nx\n\n:::\n";
        let mut root = markdown::to_mdast(input, &markdown::ParseOptions::mdx())
            .expect("markdown-rs should parse the sample");

        let mut sink = CollectingSink::new();
        let mut ctx = BuildContext::for_paths("/proj/content/a.mdx", "/proj", "/proj/public");
        ctx.diagnostics = Some(&mut sink);
        r.visit_with_context(&mut root, &mut ctx);
        drop(ctx);

        let flushed = sink.take();
        assert_eq!(
            flushed.len(),
            2,
            "unclosed + unknown must reach the sink, got {flushed:#?}"
        );
        for d in &flushed {
            let MarkdownDiagnostic::Generic {
                severity,
                message,
                location,
            } = d
            else {
                unreachable!("directive diagnostics are Generic, got {d:?}");
            };
            assert_eq!(*severity, DiagnosticSeverity::Warning);
            assert!(
                message.contains("unclosed") || message.contains("unknown directive"),
                "unexpected message {message:?}"
            );
            let loc = location.as_ref().expect("location attached");
            assert_eq!(
                loc.path.as_deref(),
                Some(std::path::Path::new("/proj/content/a.mdx"))
            );
        }
        // The unclosed-opener diagnostic carries the opener's position.
        assert!(
            flushed.iter().any(|d| matches!(
                d,
                MarkdownDiagnostic::Generic {
                    message,
                    location: Some(loc),
                    ..
                } if message.contains("unclosed") && loc.line == Some(1)
            )),
            "unclosed diagnostic keeps its line, got {flushed:#?}"
        );
        // Drained: nothing left on the registry buffer.
        assert!(
            r.take_diagnostics().is_empty(),
            "registry buffer must be drained into the sink"
        );
    }

    #[test]
    fn context_without_sink_keeps_diagnostics_on_registry() {
        // No diagnostics sink armed: the registry keeps accumulating for
        // manual draining (the pre-#2206 contract, unchanged).
        let mut r = registry_with_admonitions();
        let input = ":::warning\nnever closed\n";
        let mut root = markdown::to_mdast(input, &markdown::ParseOptions::mdx())
            .expect("markdown-rs should parse the sample");
        let mut ctx = BuildContext::for_paths("/proj/content/a.mdx", "/proj", "/proj/public");
        r.visit_with_context(&mut root, &mut ctx);
        drop(ctx);
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "diagnostic stays on the registry");
        assert!(diags[0].message.contains("unclosed"));
    }

    // ---- bit-for-bit admonition behaviour ----

    #[test]
    fn admonition_defaults_match_legacy_output() {
        let mut r = registry_with_admonitions();
        for (key, tag) in [
            ("note", "Note"),
            ("tip", "Tip"),
            ("info", "Info"),
            ("warning", "Warning"),
            ("danger", "Danger"),
            ("details", "Details"),
        ] {
            let mut reg = r.clone();
            let out = run_with_registry(
                &mut reg,
                vec![
                    text_para(&format!(":::{key}")),
                    text_para("body"),
                    text_para(":::"),
                ],
            );
            assert_eq!(out.len(), 1);
            let j = flow(&out[0]);
            assert_eq!(j.name.as_deref(), Some(tag), "kind {key}");
        }
        let _ = r.take_diagnostics();
    }

    #[test]
    fn admonition_details_with_legacy_title_attr() {
        let mut r = registry_with_admonitions();
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::details title=\"Click me\""),
                text_para("hidden"),
                text_para(":::"),
            ],
        );
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Details"));
        assert_eq!(attr(j, "title").as_deref(), Some("Click me"));
    }

    #[test]
    fn missing_close_left_alone() {
        let mut r = registry_with_admonitions();
        let out = run_with_registry(&mut r, vec![text_para(":::note"), text_para("body")]);
        assert_eq!(out.len(), 2);
        // First is still a paragraph.
        assert!(matches!(out[0], MdastNode::Paragraph(_)));
        // Since zfb#2206 the genuinely-unclosed opener additionally records
        // an unclosed-container diagnostic (the source still stays literal).
        let diags = r.take_diagnostics();
        assert_eq!(
            diags.len(),
            1,
            "unclosed diagnostic expected, got {diags:#?}"
        );
        assert!(diags[0].message.contains("unclosed"));
    }

    #[test]
    fn nested_admonition_inside_blockquote() {
        let mut r = registry_with_admonitions();
        // Build a blockquote that contains the admonition paragraphs.
        let bq = MdastNode::Blockquote(markdown::mdast::Blockquote {
            children: vec![text_para(":::note"), text_para("inner"), text_para(":::")],
            position: None,
        });
        let out = run_with_registry(&mut r, vec![bq]);
        let MdastNode::Blockquote(bq) = &out[0] else {
            unreachable!("expected blockquote")
        };
        assert_eq!(bq.children.len(), 1, "admonition collapsed inside bq");
        let j = flow(&bq.children[0]);
        assert_eq!(j.name.as_deref(), Some("Note"));
    }

    #[test]
    fn directive_in_heading_is_left_alone() {
        // Ensure `transform_inline_in` doesn't crash on heading nodes
        // and only fires for registered text directives.
        let mut r = registry_with_admonitions();
        let h = MdastNode::Heading(Heading {
            depth: 2,
            children: vec![MdastNode::Text(Text {
                value: "Hello world".to_string(),
                position: None,
            })],
            position: None,
        });
        let out = run_with_registry(&mut r, vec![h]);
        assert!(matches!(out[0], MdastNode::Heading(_)));
    }

    // ---- unbraced parser edge cases ----

    #[test]
    fn parse_unbraced_attrs_handles_quoted_and_bare() {
        let attrs = parse_unbraced_attrs("a=1 b=\"two words\" c=3");
        assert_eq!(
            attrs,
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "two words".to_string()),
                ("c".to_string(), "3".to_string()),
            ]
        );
    }

    #[test]
    fn parse_unbraced_attrs_preserves_non_ascii_quoted_values() {
        // Regression: quoted values were copied byte-by-byte via `as char`,
        // corrupting multibyte UTF-8. CJK, accented Latin, and emoji must
        // round-trip intact, and \" / \\ escapes must still work.
        let attrs = parse_unbraced_attrs("t=\"日本語\" u=\"café\" e=\"🎉\" q=\"a\\\"b\"");
        assert_eq!(
            attrs,
            vec![
                ("t".to_string(), "日本語".to_string()),
                ("u".to_string(), "café".to_string()),
                ("e".to_string(), "🎉".to_string()),
                ("q".to_string(), "a\"b".to_string()),
            ]
        );
    }

    #[test]
    fn parse_unbraced_attrs_treats_bare_words_as_boolean_attrs() {
        // Non-key=value tokens become boolean attributes with empty
        // values. The parser never panics, even on weird input.
        let attrs = parse_unbraced_attrs("just words");
        assert_eq!(
            attrs,
            vec![
                ("just".to_string(), String::new()),
                ("words".to_string(), String::new()),
            ]
        );
    }

    // ---- title_from_label ----

    #[test]
    fn title_from_label_promotes_label_to_title_attr() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef {
            name: "callout".to_string(),
            kind: DirectiveKind::Container,
            component_name: "Callout".to_string(),
            title_from_label: true,
            attrs: Vec::new(),
        });
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::callout[Heads up]"),
                text_para("body"),
                text_para(":::"),
            ],
        );
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Callout"));
        assert_eq!(attr(j, "title").as_deref(), Some("Heads up"));
    }

    #[test]
    fn note_with_custom_label_produces_title_attr() {
        // Sub #135: :::note[Custom Title] should emit title="Custom Title".
        let mut r = registry_with_admonitions();
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::note[Custom Title]"),
                text_para("body"),
                text_para(":::"),
            ],
        );
        assert_eq!(out.len(), 1);
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Note"));
        assert_eq!(
            attr(j, "title").as_deref(),
            Some("Custom Title"),
            "bracketed label promoted to title attribute"
        );
    }

    // ---- merged (collapsed) container transform (Sub #1090) ----

    #[test]
    fn merged_container_transforms_to_jsx() {
        // CHANGED for #1090 (was `merged_container_emits_blank_line_diagnostic`).
        //
        // Previously a merged `:::note\nbody\n:::` (written WITHOUT blank
        // lines around the fences — the shape `markdown::to_mdast` actually
        // produces for real input) was left untransformed and only earned a
        // "missing blank lines" diagnostic. That diverged from `githubAlerts`,
        // which transforms the same no-blank-line shape end-to-end, and meant
        // every container directive in a real build rendered as literal text.
        //
        // After the fix the collapsed single-Paragraph form TRANSFORMS, so
        // this test asserts the new behaviour (transform, no diagnostic)
        // instead of the old diagnostic. The assertion is updated, not
        // deleted: the merged form is now handled, not flagged.
        let mut r = registry_with_admonitions();
        // Simulate the merged paragraph: first Text child has the full
        // un-blank-lined content as a single multi-line value.
        let merged_para = MdastNode::Paragraph(Paragraph {
            children: vec![MdastNode::Text(Text {
                value: ":::note\nbody text\n:::".to_string(),
                position: None,
            })],
            position: None,
        });
        let out = run_with_registry(&mut r, vec![merged_para]);
        // The collapsed paragraph is now rewritten to a <Note> flow element.
        assert_eq!(out.len(), 1);
        let j = flow(&out[0]);
        assert_eq!(j.name.as_deref(), Some("Note"));
        // The body becomes a single paragraph child.
        assert_eq!(j.children.len(), 1, "body wrapped in one paragraph");
        let MdastNode::Paragraph(body) = &j.children[0] else {
            unreachable!("expected body Paragraph, got {:?}", j.children[0]);
        };
        let MdastNode::Text(t) = &body.children[0] else {
            unreachable!("expected body Text, got {:?}", body.children[0]);
        };
        assert_eq!(t.value, "body text");
        // No diagnostic — the form is handled, not flagged.
        assert!(
            r.take_diagnostics().is_empty(),
            "merged container now transforms; no diagnostic expected"
        );
    }

    #[test]
    fn fenced_code_block_does_not_trip_blank_line_diagnostic() {
        // A MdastNode::Code (fenced code block) whose content contains
        // `:::` should not trigger the diagnostic — only Paragraph nodes
        // are inspected.
        let mut r = registry_with_admonitions();
        let code_block = MdastNode::Code(Code {
            value: ":::note\nsome code\n:::".to_string(),
            lang: Some("markdown".to_string()),
            meta: None,
            position: None,
        });
        let out = run_with_registry(&mut r, vec![code_block]);
        assert_eq!(out.len(), 1, "code block preserved");
        assert!(matches!(out[0], MdastNode::Code(_)));
        let diags = r.take_diagnostics();
        assert!(
            diags.is_empty(),
            "no diagnostic for fenced code block, got {diags:?}"
        );
    }

    #[test]
    fn single_line_lone_opener_emits_unclosed_diagnostic() {
        // UPDATED for zfb#2206 (was `single_line_unrecognised_container_no_
        // blank_line_diagnostic`, which pinned "no diagnostic for a lone
        // opener" back when the only candidate was the long-removed
        // blank-line diagnostic). #2206's acceptance mandates the opposite:
        // a genuinely-unclosed container opener must EMIT a build
        // diagnostic instead of leaking silently. The source paragraph is
        // still preserved — graceful literal fallback, no transform.
        let mut r = registry_with_admonitions();
        // Just the opener with no close sibling.
        let out = run_with_registry(&mut r, vec![text_para(":::note")]);
        let diags = r.take_diagnostics();
        assert_eq!(
            diags.len(),
            1,
            "unclosed-container diagnostic expected, got {diags:#?}"
        );
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("note"),
            "diagnostic names the unclosed directive, got {:?}",
            diags[0].message
        );
        // Source paragraph preserved.
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], MdastNode::Paragraph(_)));
    }

    // ---- directives-only (generic directives feature, zero defaults) tests ----

    // A registry built with only a user-supplied name registers ONLY that
    // name — core seeds no `note`/`tip`/… vocabulary.
    #[test]
    fn directives_only_no_defaults_registered() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("spoiler", "Spoiler"));

        // `:::spoiler` is resolved.
        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::spoiler"),
                text_para("hidden"),
                text_para(":::"),
            ],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(flow(&out[0]).name.as_deref(), Some("Spoiler"));

        // `:::note` is NOT registered — left untransformed (zero defaults).
        let out2 = run_with_registry(
            &mut r,
            vec![text_para(":::note"), text_para("body"), text_para(":::")],
        );
        // Should still be 3 paragraphs (no transformation).
        assert_eq!(out2.len(), 3, ":::note must stay as-is when not registered");
    }

    // Registering the same name twice is last-wins: the later component name
    // overrides the earlier one. This is the semantic the `features.directives`
    // map relies on when a user redefines a name.
    #[test]
    fn register_same_name_is_last_wins() {
        let mut r = DirectiveRegistry::new();
        r.register(DirectiveDef::container("caution", "Caution"));
        // Redefine `caution` with a different component name.
        r.register(DirectiveDef::container("caution", "MyCaution"));

        let out = run_with_registry(
            &mut r,
            vec![
                text_para(":::caution"),
                text_para("be careful"),
                text_para(":::"),
            ],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(
            flow(&out[0]).name.as_deref(),
            Some("MyCaution"),
            "later registration must override the earlier one for the same name"
        );
    }

    // ── GFM constructs at the `reparse_block` parse site (zfb#2390) ─────
    //
    // `reparse_block` is reached only through the COLLAPSED shape — a
    // directive run written without blank lines, which `markdown::to_mdast`
    // hands back as one Paragraph with a single multi-line `Text` child
    // (`single_text_collapsed`). The blank-line-separated form is parsed by
    // the main parse and never reaches this code at all.
    //
    // These tests build that exact shape directly rather than going through
    // a Pipeline, for two reasons. It is the only way to guarantee the
    // re-parse is what produced a node: a body containing inline markup
    // makes the main parse emit MULTIPLE inline children, which routes to
    // `transform_block_container` instead and would silently test the wrong
    // path. And a block construct such as a table, written inside a
    // collapsed fence in real source, is consumed by the MAIN parse — the
    // table absorbs the `:::` closer as a row, so the directive is never
    // recognised (pre-existing, unrelated to #2390, and unaffected by it).

    /// Collect every mdast node in `nodes`, depth-first, that satisfies
    /// `pred` — the re-parsed body is nested inside the emitted
    /// `MdxJsxFlowElement`, so assertions cannot look at the top level only.
    fn any_node(nodes: &[MdastNode], pred: &dyn Fn(&MdastNode) -> bool) -> bool {
        nodes.iter().any(|n| {
            pred(n)
                || n.children()
                    .is_some_and(|children| any_node(children, pred))
        })
    }

    fn has_table(nodes: &[MdastNode]) -> bool {
        any_node(nodes, &|n| matches!(n, MdastNode::Table(_)))
    }

    fn has_strikethrough(nodes: &[MdastNode]) -> bool {
        any_node(nodes, &|n| matches!(n, MdastNode::Delete(_)))
    }

    fn has_task_list_item(nodes: &[MdastNode]) -> bool {
        any_node(
            nodes,
            &|n| matches!(n, MdastNode::ListItem(li) if li.checked.is_some()),
        )
    }

    fn has_footnote_definition(nodes: &[MdastNode]) -> bool {
        any_node(nodes, &|n| matches!(n, MdastNode::FootnoteDefinition(_)))
    }

    fn has_link_to(nodes: &[MdastNode], url: &str) -> bool {
        any_node(nodes, &|n| matches!(n, MdastNode::Link(l) if l.url == url))
    }

    fn has_break(nodes: &[MdastNode]) -> bool {
        any_node(nodes, &|n| matches!(n, MdastNode::Break(_)))
    }

    fn has_strong_wrapping(nodes: &[MdastNode], text: &str) -> bool {
        any_node(
            nodes,
            &|n| matches!(n, MdastNode::Strong(s) if s.children.iter().any(|c| matches!(c, MdastNode::Text(t) if t.value == text))),
        )
    }

    /// A collapsed `:::note … :::` run carrying every GFM construct, in the
    /// single-multi-line-`Text` shape the main parse produces for it.
    const COLLAPSED_GFM_BODY: &str = ":::note\n\
        | a | b |\n\
        | - | - |\n\
        | 1 | 2 |\n\
        ~~struck~~\n\
        - [x] done\n\
        See https://example.com/x for more.\n\
        Ref.[^n]\n\
        [^n]: the note body.\n\
        :::";

    #[test]
    fn collapsed_directive_body_reparses_with_the_registrys_gfm_constructs() {
        let mut r = registry_with_admonitions().with_gfm(ResolvedGfmConstructs::ALL_ON, false);
        let out = run_with_registry(&mut r, vec![text_para(COLLAPSED_GFM_BODY)]);

        assert!(has_table(&out), "GFM table did not parse: {out:#?}");
        assert!(
            has_strikethrough(&out),
            "GFM strikethrough did not parse: {out:#?}"
        );
        assert!(
            has_task_list_item(&out),
            "GFM task list item did not parse: {out:#?}"
        );
        assert!(
            has_link_to(&out, "https://example.com/x"),
            "GFM autolink literal did not parse: {out:#?}"
        );
        assert!(
            has_footnote_definition(&out),
            "GFM footnote definition did not parse: {out:#?}"
        );
    }

    #[test]
    fn collapsed_directive_body_keeps_every_gfm_construct_off_by_default() {
        // `registry_with_admonitions` uses `DirectiveRegistry::new`, i.e. the
        // ALL_OFF default — this pins that a registry built without
        // `with_gfm` behaves exactly as it did before #2390.
        let mut r = registry_with_admonitions();
        let out = run_with_registry(&mut r, vec![text_para(COLLAPSED_GFM_BODY)]);

        assert!(!has_table(&out), "table must stay literal: {out:#?}");
        assert!(
            !has_strikethrough(&out),
            "strikethrough must stay literal: {out:#?}"
        );
        assert!(
            !has_task_list_item(&out),
            "task list item must stay literal: {out:#?}"
        );
        assert!(
            !has_link_to(&out, "https://example.com/x"),
            "autolink literal must stay literal: {out:#?}"
        );
        assert!(
            !has_footnote_definition(&out),
            "footnote definition must stay literal: {out:#?}"
        );
        // Positive control: the body WAS emitted, so the assertions above
        // cannot be passing merely because nothing was produced.
        assert_eq!(out.len(), 1, "the directive still transforms: {out:#?}");
    }

    /// `flush_prose` re-parses ordinary page prose that merely sits BETWEEN
    /// two collapsed directive runs — a wider blast radius than "directive
    /// bodies", which is why the #2390 changelog entry names it separately.
    #[test]
    fn inter_run_prose_reparses_with_the_registrys_gfm_constructs() {
        let mut r = registry_with_admonitions().with_gfm(ResolvedGfmConstructs::ALL_ON, false);
        let out = run_with_registry(
            &mut r,
            vec![text_para(
                ":::note\nfirst.\n:::\n\
                 ~~struck~~\n\
                 See https://example.com/x for more.\n\
                 :::note\nsecond.\n:::",
            )],
        );

        assert!(
            has_strikethrough(&out),
            "inter-run prose: strikethrough did not parse: {out:#?}"
        );
        assert!(
            has_link_to(&out, "https://example.com/x"),
            "inter-run prose: autolink literal did not parse: {out:#?}"
        );
    }

    /// zfb#2388 at this parse site. The pipeline unwraps nested autolinks
    /// right after its OWN top-level parse, ahead of the visitor chain — a
    /// subtree re-parsed from inside that chain is out of its reach, so
    /// `reparse_block` has to normalise its own output or ship an `<a>`
    /// nested in an `<a>`.
    #[test]
    fn collapsed_directive_body_unwraps_nested_autolinks() {
        let mut r = registry_with_admonitions().with_gfm(ResolvedGfmConstructs::ALL_ON, false);
        let out = run_with_registry(
            &mut r,
            vec![text_para(
                ":::note\nsee it\n[http://localhost:4321](http://localhost:4321)\n:::",
            )],
        );

        // Positive control first: the author's link must exist at all,
        // otherwise "no nesting" would pass on an empty tree.
        assert!(
            has_link_to(&out, "http://localhost:4321"),
            "the author's link must survive: {out:#?}"
        );
        assert!(
            !any_node(&out, &|n| {
                matches!(n, MdastNode::Link(l)
                    if any_node(&l.children, &|c| matches!(c, MdastNode::Link(_))))
            }),
            "autolink literal left nested inside a link label: {out:#?}"
        );
    }

    /// zfb#1105 at this parse site, gated the same way the pipeline gates
    /// its own `CjkAutolinkBoundaryPlugin` wiring.
    #[test]
    fn collapsed_directive_body_stops_autolinks_at_a_cjk_boundary() {
        let mut r = registry_with_admonitions().with_gfm(ResolvedGfmConstructs::ALL_ON, true);
        let out = run_with_registry(
            &mut r,
            vec![text_para(
                ":::note\n案内\n詳しくは https://example.com/xを参照。\n:::",
            )],
        );

        assert!(
            has_link_to(&out, "https://example.com/x"),
            "the URL must autolink at exactly the ASCII boundary: {out:#?}"
        );
        assert!(
            !any_node(&out, &|n| {
                matches!(n, MdastNode::Link(l) if l.url.contains('を'))
            }),
            "trailing CJK was swallowed into the href: {out:#?}"
        );
    }

    // ── CJK-friendly emphasis + hard breaks at this parse site (zfb#2398) ──

    /// A CJK-flanked strong marker `CjkFriendlyPlugin` corrects — see
    /// `zfb_md_ast::cjk_friendly`'s module docs for why plain CommonMark
    /// leaves this as literal `*` text. Shared with that module's other
    /// test sites rather than re-spelled here (zfb#2402).
    const CJK_EMPHASIS_BODY: &str = zfb_md_ast::cjk_friendly::FLANKED_EMPHASIS_REPRO;

    /// zfb#2398 at this parse site: `CjkFriendlyPlugin` applies to a
    /// collapsed directive body on the default (Html) target — decoupled
    /// from `target`, unlike `HardBreaksPlugin` below.
    #[test]
    fn collapsed_directive_body_corrects_cjk_emphasis_flanking() {
        let mut r = registry_with_admonitions().with_gfm(ResolvedGfmConstructs::ALL_ON, true);
        let out = run_with_registry(
            &mut r,
            vec![text_para(&format!(":::note\n{CJK_EMPHASIS_BODY}\n:::"))],
        );

        assert!(
            has_strong_wrapping(&out, "重要。"),
            "CJK emphasis flanking must be corrected in a collapsed directive body: {out:#?}"
        );
    }

    /// Same fixture on the Jsx target: `CjkFriendlyPlugin` is not gated on
    /// `target`, so it must correct there too.
    #[test]
    fn collapsed_directive_body_corrects_cjk_emphasis_flanking_on_the_jsx_target() {
        let mut r = registry_with_admonitions().with_gfm(ResolvedGfmConstructs::ALL_ON, true);
        r.set_secondary_parse_target(SecondaryParseTarget::Jsx);
        let out = run_with_registry(
            &mut r,
            vec![text_para(&format!(":::note\n{CJK_EMPHASIS_BODY}\n:::"))],
        );

        assert!(
            has_strong_wrapping(&out, "重要。"),
            "CJK emphasis flanking must be corrected on the Jsx target too: {out:#?}"
        );
    }

    /// `cjkFriendly: false` leaves the collapsed body literal — no
    /// regression from the #2398 wiring.
    #[test]
    fn collapsed_directive_body_keeps_cjk_emphasis_literal_when_off() {
        let mut r = registry_with_admonitions().with_gfm(ResolvedGfmConstructs::ALL_ON, false);
        let out = run_with_registry(
            &mut r,
            vec![text_para(&format!(":::note\n{CJK_EMPHASIS_BODY}\n:::"))],
        );

        assert!(
            !has_strong_wrapping(&out, "重要。"),
            "CJK emphasis must stay literal when cjkFriendly is off: {out:#?}"
        );
    }

    /// zfb#2398, the Jsx-only gate: `HardBreaksPlugin` applies at this call
    /// site only when the pipeline run is feeding the JSX-emit path — a
    /// collapsed directive body re-parses INTO the JSX children of the
    /// `MdxJsxFlowElement` the directive builds, and only the JSX emitter
    /// has a real arm for `Break`.
    #[test]
    fn collapsed_directive_body_inserts_break_on_the_jsx_target() {
        let mut r = registry_with_admonitions()
            .with_gfm(ResolvedGfmConstructs::ALL_ON, false)
            .with_hard_breaks(true);
        r.set_secondary_parse_target(SecondaryParseTarget::Jsx);
        let out = run_with_registry(
            &mut r,
            vec![text_para(":::note\nfirst line\nsecond line\n:::")],
        );

        assert!(
            has_break(&out),
            "HardBreaksPlugin must apply on the Jsx target: {out:#?}"
        );
    }

    /// The Html-target counterpart of the test above: `HardBreaksPlugin`
    /// must NOT run. On the HTML path this body lands inside an
    /// `MdxJsxFlowElement` that `reconstruct_jsx` renders through a lossy
    /// catch-all which stringifies `Break` to an EMPTY STRING — applying
    /// the plugin here would delete newlines from directive bodies rather
    /// than turn them into `<br>` (a regression, not parity).
    ///
    /// This mdast-level test (driving the registry directly) is the
    /// meaningful, revert-sensitive proof of the gate — an HTML-level
    /// end-to-end assertion through the full `Pipeline` cannot exercise
    /// this call site at all when `hardBreaks` is on: the pipeline's own
    /// top-level `HardBreaksPlugin` (wired earlier in the mdast chain,
    /// pre-dating zfb#2398) already splits a collapsed directive's single
    /// multi-line `Text` child before `DirectiveRegistry` ever runs,
    /// which routes the directive through `transform_block_container`
    /// instead and never reaches `reparse_block`. See the explanatory
    /// comment in `gfm_secondary_parse_sites.rs` where the HTML-level
    /// test would otherwise have gone, for the full argument.
    #[test]
    fn collapsed_directive_body_does_not_insert_break_on_the_html_target() {
        let mut r = registry_with_admonitions()
            .with_gfm(ResolvedGfmConstructs::ALL_ON, false)
            .with_hard_breaks(true);
        // Default target is Html — no explicit set needed.
        let out = run_with_registry(
            &mut r,
            vec![text_para(":::note\nfirst line\nsecond line\n:::")],
        );

        assert!(
            !has_break(&out),
            "HardBreaksPlugin must NOT apply on the Html target (Jsx-only gate): {out:#?}"
        );
    }

    /// `hardBreaks: false` inserts no `Break` on either target — no
    /// regression from the #2398 wiring.
    #[test]
    fn collapsed_directive_body_inserts_no_break_when_hard_breaks_is_off() {
        for target in [SecondaryParseTarget::Html, SecondaryParseTarget::Jsx] {
            let mut r = registry_with_admonitions().with_gfm(ResolvedGfmConstructs::ALL_ON, false);
            r.set_secondary_parse_target(target);
            let out = run_with_registry(
                &mut r,
                vec![text_para(":::note\nfirst line\nsecond line\n:::")],
            );

            assert!(
                !has_break(&out),
                "hardBreaks off must insert no Break on {target:?}: {out:#?}"
            );
        }
    }

    /// `flush_prose` applies `HardBreaksPlugin` on BOTH targets — inter-run
    /// prose splices in as top-level siblings, so `Break` renders as a real
    /// `<br>` on the HTML path too (unlike the directive-body site above).
    #[test]
    fn inter_run_prose_inserts_break_on_both_targets() {
        for target in [SecondaryParseTarget::Html, SecondaryParseTarget::Jsx] {
            let mut r = registry_with_admonitions()
                .with_gfm(ResolvedGfmConstructs::ALL_ON, false)
                .with_hard_breaks(true);
            r.set_secondary_parse_target(target);
            let out = run_with_registry(
                &mut r,
                vec![text_para(
                    ":::note\nfirst.\n:::\n\
                     prose line one\n\
                     prose line two\n\
                     :::note\nsecond.\n:::",
                )],
            );

            assert!(
                has_break(&out),
                "inter-run prose must insert Break on {target:?}: {out:#?}"
            );
        }
    }

    /// A collapsed `:::note` whose body holds prose ABOVE a nested
    /// `:::tip` run — the shape that drives `build_collapsed_body` →
    /// `transform_collapsed_run` → `flush_prose`, where the flushed prose
    /// becomes a CHILD of the outer `<Note>` rather than a top-level
    /// sibling.
    const NESTED_RUN_PROSE_BODY: &str = ":::note\n\
         prose line one\n\
         prose line two\n\
         :::tip\n\
         inner.\n\
         :::\n\
         :::";

    /// zfb#2402: `flush_prose` reached through the recursion is subject to
    /// the SAME Jsx-only gate as the directive body around it. The
    /// original #2398 wiring hardcoded "top-level sibling" at that call
    /// site, which is true only for the outermost
    /// `transform_collapsed_run` — the recursive one hands its output to
    /// `build_flow_jsx` as `MdxJsxFlowElement` children — which, when the
    /// gate was written, meant the HTML path's `reconstruct_jsx`
    /// stringified the `Break` to the empty string and DELETED the
    /// author's newline. zfb#2401 has since fixed that renderer (such a
    /// `Break` now renders `<br />`), so what this test pins today is the
    /// gate's *behaviour-preserving* half, not deletion avoidance — see
    /// `SecondaryParsePlacement`'s doc comment in `zfb-md-ast`.
    ///
    /// Driven at the mdast level (`DirectiveRegistry` directly) rather
    /// than through the full `Pipeline`, and still necessarily so after
    /// zfb#2413: with `markdown.hardBreaks` on, the pipeline's own
    /// top-level `HardBreaksPlugin` splits the collapsed paragraph before
    /// `DirectiveRegistry` runs, and the line-level re-segmenter now
    /// carries those `Break` nodes through UNPARSED (they are preserved
    /// provenance), so an inter-run prose range with a newline in it never
    /// reaches `flush_prose`'s own `HardBreaksPlugin` application
    /// end-to-end. Feeding a single-`Text` paragraph here is what puts the
    /// gate under test.
    #[test]
    fn nested_inter_run_prose_does_not_insert_break_on_the_html_target() {
        let mut r = registry_with_admonitions()
            .with_gfm(ResolvedGfmConstructs::ALL_ON, false)
            .with_hard_breaks(true);
        // Default target is Html — no explicit set needed.
        let out = run_with_registry(&mut r, vec![text_para(NESTED_RUN_PROSE_BODY)]);

        // Positive control: the nested run really was recognised, so the
        // prose above it really did travel through the recursion's
        // `flush_prose` (and not the outer, top-level one).
        assert_eq!(out.len(), 1, "one outer <Note> element: {out:#?}");
        assert!(
            any_node(&out, &|n| {
                matches!(n, MdastNode::MdxJsxFlowElement(j) if j.name.as_deref() == Some("Tip"))
            }),
            "the nested :::tip must transform, proving the recursion ran: {out:#?}"
        );
        assert!(
            !has_break(&out),
            "HardBreaksPlugin must NOT apply to prose flushed INSIDE a \
             collapsed directive body on the Html target — the placement \
             gate keeps the author's literal newline there (zfb#2402): \
             {out:#?}"
        );
    }

    /// The Jsx-target counterpart: the same nested prose DOES get its
    /// `Break`, because the JSX emitter has a real arm for it. Without
    /// this half, the test above would also pass with `HardBreaksPlugin`
    /// removed from `flush_prose` altogether.
    #[test]
    fn nested_inter_run_prose_inserts_break_on_the_jsx_target() {
        let mut r = registry_with_admonitions()
            .with_gfm(ResolvedGfmConstructs::ALL_ON, false)
            .with_hard_breaks(true);
        r.set_secondary_parse_target(SecondaryParseTarget::Jsx);
        let out = run_with_registry(&mut r, vec![text_para(NESTED_RUN_PROSE_BODY)]);

        assert!(
            has_break(&out),
            "HardBreaksPlugin must still apply to nested prose on the Jsx \
             target: {out:#?}"
        );
    }

    // ── cross-fix interaction: footnote × hard-breaks (zfb#2399) ────────
    //
    // A collapsed directive body is, by construction, a multi-line
    // paragraph with an embedded `\n` in its single `Text` child — and
    // when `markdown.hardBreaks` is on, the pipeline's OWN top-level
    // `HardBreaksPlugin` (wired earlier in the mdast chain than
    // `DirectiveRegistry`) already splits that shape before
    // `DirectiveRegistry` ever runs. zfb#2413 made the re-segmenter
    // TOLERANT of that split rather than reordering the chain, so the
    // split shape is now recognised — but a body range carrying `Break`
    // provenance is assembled losslessly instead of re-parsed, which is
    // precisely what leaves the `SecondaryParsePlacement` gate untouched.
    // `reparse_block`'s hard-breaks half therefore stays unreachable for
    // this shape through the full `Pipeline`, and the meaningful,
    // revert-sensitive proof that the footnote-aware stringifier
    // (zfb#2396) and the Jsx-only hard-breaks gate (zfb#2398) compose is
    // still here, driving `DirectiveRegistry` directly — the same
    // isolation technique `collapsed_directive_body_inserts_break_on_
    // the_jsx_target` / `..._does_not_insert_break_on_the_html_target`
    // use above.

    /// Html target: `HardBreaksPlugin` must NOT insert a `Break` (the
    /// Jsx-only gate), and a footnote reparsed in the SAME body must
    /// still associate correctly — proving the gate's placement decision
    /// and the footnote-aware reparse don't interfere with each other.
    #[test]
    fn collapsed_directive_body_footnote_and_hard_breaks_compose_on_the_html_target() {
        let mut r = registry_with_admonitions()
            .with_gfm(ResolvedGfmConstructs::ALL_ON, false)
            .with_hard_breaks(true);
        // Default target is Html.
        let out = run_with_registry(
            &mut r,
            vec![text_para(
                ":::note\nfirst line\nsecond line.[^n]\n[^n]: the note body.\n:::",
            )],
        );

        assert!(
            has_footnote_definition(&out),
            "footnote definition must still parse alongside hardBreaks: {out:#?}"
        );
        assert!(
            !has_break(&out),
            "HardBreaksPlugin must not insert Break on the Html target even \
             with a footnote reparsed in the same body (Jsx-only gate, \
             zfb#2398): {out:#?}"
        );
    }

    /// Jsx target counterpart: `HardBreaksPlugin` DOES apply there, and
    /// must not be disturbed by the footnote also being reparsed in the
    /// same body.
    #[test]
    fn collapsed_directive_body_footnote_and_hard_breaks_compose_on_the_jsx_target() {
        let mut r = registry_with_admonitions()
            .with_gfm(ResolvedGfmConstructs::ALL_ON, false)
            .with_hard_breaks(true);
        r.set_secondary_parse_target(SecondaryParseTarget::Jsx);
        let out = run_with_registry(
            &mut r,
            vec![text_para(
                ":::note\nfirst line\nsecond line.[^n]\n[^n]: the note body.\n:::",
            )],
        );

        assert!(
            has_footnote_definition(&out),
            "footnote definition must still parse alongside hardBreaks: {out:#?}"
        );
        assert!(
            has_break(&out),
            "HardBreaksPlugin must insert Break on the Jsx target alongside \
             a footnote reparsed in the same body: {out:#?}"
        );
    }

    // ── zfb#2413: the SPLIT shape reaches the same re-segmenter ─────────
    //
    // These drive `DirectiveRegistry` directly with a paragraph already in
    // the shape the pipeline's top-level `HardBreaksPlugin` /
    // `CjkFriendlyPlugin` produce — `Text`/`Break`/`Text` rather than one
    // multi-line `Text` — so they pin the recognition tolerance itself,
    // independently of which plugin chain happens to produce that shape.
    // The end-to-end rendering pins live in
    // `tests/gfm_secondary_parse_sites.rs`.

    /// A paragraph whose `lines` are separated by real `Break` nodes: the
    /// shape `HardBreaksPlugin` leaves behind.
    fn hard_broken_para(lines: &[&str]) -> MdastNode {
        let mut children: Vec<MdastNode> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                children.push(MdastNode::Break(markdown::mdast::Break { position: None }));
            }
            children.push(MdastNode::Text(Text {
                value: (*line).to_string(),
                position: None,
            }));
        }
        MdastNode::Paragraph(Paragraph {
            children,
            position: None,
        })
    }

    #[test]
    fn split_shape_nested_collapsed_run_transforms_both_levels() {
        let mut r = registry_with_admonitions().with_hard_breaks(true);
        let out = run_with_registry(
            &mut r,
            vec![hard_broken_para(&[
                ":::note",
                "prose above",
                ":::tip",
                "inner body",
                ":::",
                ":::",
            ])],
        );

        assert_eq!(out.len(), 1, "one outer <Note> element: {out:#?}");
        assert!(
            any_node(&out, &|n| {
                matches!(n, MdastNode::MdxJsxFlowElement(j) if j.name.as_deref() == Some("Tip"))
            }),
            "the nested :::tip must transform in the split shape: {out:#?}"
        );
        assert!(
            !any_node(
                &out,
                &|n| matches!(n, MdastNode::Text(t) if t.value.contains(":::"))
            ),
            "no literal fence may survive: {out:#?}"
        );
    }

    /// The colon-stack rule reaches the split shape too — the block-level
    /// opener parse required EXACTLY three colons, so this fixture went
    /// fully literal before zfb#2413.
    #[test]
    fn split_shape_deep_outer_fence_nests_innermost_first() {
        let mut r = registry_with_admonitions().with_hard_breaks(true);
        let out = run_with_registry(
            &mut r,
            vec![hard_broken_para(&[
                ":::::note",
                "prose above",
                ":::tip",
                "inner body",
                ":::",
                ":::::",
            ])],
        );

        assert_eq!(out.len(), 1, "one outer <Note> element: {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert!(
            note.children.iter().any(|c| {
                matches!(c, MdastNode::MdxJsxFlowElement(j) if j.name.as_deref() == Some("Tip"))
            }),
            "the <Tip> must be a CHILD of the <Note>, not a sibling: {out:#?}"
        );
    }

    /// The assembly policy, pinned at the node level: a body range carrying
    /// `Break` provenance is rebuilt from the existing nodes, never
    /// re-parsed — which is what keeps `reparse_block`'s
    /// `SecondaryParsePlacement` gate out of this path entirely (zfb#2413,
    /// axis 4 unchanged).
    #[test]
    fn split_shape_body_keeps_its_break_nodes_on_the_html_target() {
        let mut r = registry_with_admonitions().with_hard_breaks(true);
        // Default target is Html, where the placement gate would STRIP a
        // Break had the body been re-parsed.
        let out = run_with_registry(
            &mut r,
            vec![hard_broken_para(&[
                ":::note",
                "first line",
                "second line",
                ":::",
            ])],
        );

        assert_eq!(out.len(), 1, "one <Note> element: {out:#?}");
        assert!(
            has_break(&out),
            "the author's Break must survive into the directive body: {out:#?}"
        );
    }

    /// An unknown name in the split shape warns exactly once. Before
    /// zfb#2413 the block-level `warn_unknown` owned this shape; now the
    /// line-level site does, and the two must not both fire.
    #[test]
    fn split_shape_unknown_name_warns_exactly_once_and_stays_literal() {
        let mut r = registry_with_admonitions().with_hard_breaks(true);
        let out = run_with_registry(
            &mut r,
            vec![hard_broken_para(&[":::nosuchname", "body line", ":::"])],
        );

        let diags = r.take_diagnostics();
        assert_eq!(
            diags.len(),
            1,
            "exactly one unknown-directive warning: {diags:#?}"
        );
        assert_eq!(diags[0].message, "unknown directive `nosuchname`");
        assert!(
            !any_node(&out, &|n| matches!(n, MdastNode::MdxJsxFlowElement(_))),
            "an unknown name must stay literal: {out:#?}"
        );
    }

    /// A genuinely unclosed opener in the split shape keeps the block-level
    /// unclosed diagnostic and the literal fallback: the re-segmenter
    /// declines (no closer) and hands the paragraph back untouched.
    #[test]
    fn split_shape_unclosed_opener_still_records_the_unclosed_diagnostic() {
        let mut r = registry_with_admonitions().with_hard_breaks(true);
        let out = run_with_registry(&mut r, vec![hard_broken_para(&[":::note", "body line"])]);

        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "exactly one warning: {diags:#?}");
        assert!(
            diags[0].message.starts_with("unclosed container directive"),
            "unexpected diagnostic: {diags:#?}"
        );
        assert!(
            !any_node(&out, &|n| matches!(n, MdastNode::MdxJsxFlowElement(_))),
            "an unclosed opener must stay literal: {out:#?}"
        );
    }

    // ── zfb#2416: blank-line-separated ("spaced") nested containers ──
    //
    // The collapsed form has nested for a while (zfb#1099 / zfb#2413's
    // colon stack), but the SPACED form's outer fence never transformed:
    // `transform_block_container`'s sibling scan stopped at the first
    // opener-shaped sibling — which, for a spaced nested run, IS the inner
    // directive — and the outer opener was recorded unclosed. The scan now
    // feeds a flattened block-level fence-line view to that same colon
    // stack. The tests below pin the shapes the fix enables and, just as
    // importantly, the shapes it must NOT change; the anti-cascade pins
    // (`real_parser_unclosed_opener_leaves_later_padded_directive_intact`,
    // `real_parser_cascade_broken_form_does_not_steal_later_closer`) and
    // the `real_parser_buried_*` family stay untouched above.

    /// The reporter's fixture (zfb#2415): a spaced outer fence whose body
    /// carries a GLUED inner run. Both fences must transform, nested, with
    /// no diagnostic — the outer used to leak as literal `<p>:::note</p>` /
    /// `<p>:::</p>` paragraphs around a lone `<Tip>`.
    #[test]
    fn real_parser_spaced_outer_transforms_around_a_glued_inner_run() {
        let mut r = registry_with_admonitions();
        let input = ":::note\n\nprose above\n\n:::tip\ninner body\n:::\n\n:::\n";
        let out = run_real_parser(&mut r, input);

        assert_eq!(out.len(), 1, "one outer <Note>, got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert_eq!(body_paragraph_texts(note), vec!["prose above".to_string()]);
        let tip = flow(
            note.children
                .iter()
                .find(|c| matches!(c, MdastNode::MdxJsxFlowElement(_)))
                .unwrap_or_else(|| panic!("the <Tip> must be a CHILD of the <Note>: {out:#?}")),
        );
        assert_eq!(tip.name.as_deref(), Some("Tip"));
        assert_eq!(body_paragraph_texts(tip), vec!["inner body".to_string()]);
        assert_no_literal_fence(&out);
        assert!(r.take_diagnostics().is_empty());
    }

    /// The same nesting with the INNER fences spaced too — every fence on
    /// its own paragraph. The fence-line view has to reach across four
    /// sibling paragraphs to match the outer closer.
    #[test]
    fn real_parser_fully_spaced_nested_containers_transform() {
        let mut r = registry_with_admonitions();
        let input = ":::note\n\nprose above\n\n:::tip\n\ninner body\n\n:::\n\n:::\n";
        let out = run_real_parser(&mut r, input);

        assert_eq!(out.len(), 1, "one outer <Note>, got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert!(
            note.children.iter().any(|c| {
                matches!(c, MdastNode::MdxJsxFlowElement(j) if j.name.as_deref() == Some("Tip"))
            }),
            "the <Tip> must be a CHILD of the <Note>: {out:#?}"
        );
        assert_no_literal_fence(&out);
        assert!(r.take_diagnostics().is_empty());
    }

    /// A `:::::note` outer fence — the documented spelling for "this
    /// container wraps another". The block level used to parse openers with
    /// a hardcoded 3, so a 5-colon spaced opener was invisible: no
    /// transform and not even an unclosed diagnostic.
    #[test]
    fn real_parser_five_colon_spaced_outer_transforms() {
        let mut r = registry_with_admonitions();
        let input = ":::::note\n\nprose above\n\n:::tip\ninner body\n:::\n\n:::::\n";
        let out = run_real_parser(&mut r, input);

        assert_eq!(out.len(), 1, "one outer <Note>, got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert!(
            note.children.iter().any(|c| {
                matches!(c, MdastNode::MdxJsxFlowElement(j) if j.name.as_deref() == Some("Tip"))
            }),
            "the <Tip> must be a CHILD of the <Note>: {out:#?}"
        );
        assert_no_literal_fence(&out);
        assert!(r.take_diagnostics().is_empty());
    }

    /// Three levels, 7 / 5 / 3 colons, nesting innermost-first — the
    /// colon-stack rule the collapsed path already applied, now reached
    /// through the block level. The visitor recurses into a transformed
    /// body, so the middle and inner fences are matched on later passes.
    #[test]
    fn real_parser_three_spaced_levels_nest_innermost_first() {
        let mut r = registry_with_admonitions();
        let input = ":::::::note\n\nouter prose\n\n:::::tip\n\nmid prose\n\n\
                     :::warning\ninnermost\n:::\n\n:::::\n\n:::::::\n";
        let out = run_real_parser(&mut r, input);

        assert_eq!(out.len(), 1, "one outer <Note>, got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        let tip = flow(
            note.children
                .iter()
                .find(|c| matches!(c, MdastNode::MdxJsxFlowElement(_)))
                .unwrap_or_else(|| panic!("<Tip> must be a child of <Note>: {out:#?}")),
        );
        assert_eq!(tip.name.as_deref(), Some("Tip"));
        let warning = flow(
            tip.children
                .iter()
                .find(|c| matches!(c, MdastNode::MdxJsxFlowElement(_)))
                .unwrap_or_else(|| panic!("<Warning> must be a child of <Tip>: {out:#?}")),
        );
        assert_eq!(warning.name.as_deref(), Some("Warning"));
        assert_eq!(body_paragraph_texts(warning), vec!["innermost".to_string()]);
        assert_no_literal_fence(&out);
        assert!(r.take_diagnostics().is_empty());
    }

    /// Anti-cascade control, spaced flavour: an unclosed INNER opener with
    /// exactly one trailing closer. The inner claims that closer (it is the
    /// innermost still-open fence) and the outer is left genuinely
    /// unclosed — one diagnostic, naming the OUTER, and no cascade.
    #[test]
    fn real_parser_spaced_unclosed_inner_claims_the_closer_and_outer_warns() {
        let mut r = registry_with_admonitions();
        let input = ":::note\n\nprose\n\n:::tip\nnever closed inner\n\n:::\n";
        let out = run_real_parser(&mut r, input);

        let jsx_names: Vec<&str> = out
            .iter()
            .filter_map(|c| match c {
                MdastNode::MdxJsxFlowElement(j) => j.name.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(
            jsx_names,
            vec!["Tip"],
            "only the inner directive transforms, got {out:#?}"
        );
        let mut texts = Vec::new();
        collect_text_values(&out, &mut texts);
        assert!(
            texts.iter().any(|t| t.contains(":::note")),
            "the unclosed outer opener stays literal, got {texts:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("note"),
            "the diagnostic names the unclosed OUTER, got {:?}",
            diags[0].message
        );
    }

    /// Mismatched colon counts: a 5-colon outer whose only closers are
    /// 3-colon. The stack refuses to close a 5-colon opener with a 3-colon
    /// fence, so the outer stays literal — and now says so. Before this
    /// change the 5-colon opener was not even parsed at block level, so the
    /// leak was SILENT; the diagnostic is additive.
    #[test]
    fn real_parser_spaced_mismatched_closer_counts_stay_literal_and_warn() {
        let mut r = registry_with_admonitions();
        let input = ":::::note\n\nprose\n\n:::tip\ninner\n:::\n\n:::\n";
        let out = run_real_parser(&mut r, input);

        let jsx_names: Vec<&str> = out
            .iter()
            .filter_map(|c| match c {
                MdastNode::MdxJsxFlowElement(j) => j.name.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(
            jsx_names,
            vec!["Tip"],
            "the 5-colon outer must not close on a 3-colon fence, got {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("note"),
            "the diagnostic names the unclosed outer, got {:?}",
            diags[0].message
        );
    }

    /// An UNKNOWN inner name inside a spaced outer: the outer transforms,
    /// the balanced inner run stays literal inside its body, and exactly
    /// one `unknown directive` warning fires. The inner fence lines still
    /// count as stack events (they are shaped like fences whatever the
    /// registry thinks of the name), which is what keeps the outer's
    /// closer correctly attributed.
    #[test]
    fn real_parser_spaced_outer_transforms_around_an_unknown_inner_name() {
        let mut r = registry_with_admonitions();
        let input = ":::note\n\nprose\n\n:::bogus\ninner\n:::\n\n:::\n";
        let out = run_real_parser(&mut r, input);

        assert_eq!(out.len(), 1, "one outer <Note>, got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert!(
            !note
                .children
                .iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "the unknown inner run must stay literal: {out:#?}"
        );
        let mut texts = Vec::new();
        collect_text_values(&out, &mut texts);
        assert!(
            texts.iter().any(|t| t.contains(":::bogus")),
            "the unknown fence stays literal, got {texts:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one diagnostic, got {diags:#?}");
        assert_eq!(diags[0].message, "unknown directive `bogus`");
    }

    /// A WRONG-KIND inner name — a registered LEAF spelled as a container
    /// fence. The outer transforms, the inner stays literal, and NO
    /// diagnostic fires (the name is registered, so `warn_unknown` never
    /// applies; the kind mismatch is silent, as it is everywhere else).
    #[test]
    fn real_parser_spaced_outer_transforms_around_a_wrong_kind_inner_name() {
        let mut r = registry_with_admonitions();
        r.register(DirectiveDef::leaf("video", "Video"));
        let input = ":::note\n\nprose\n\n:::video\ninner\n:::\n\n:::\n";
        let out = run_real_parser(&mut r, input);

        assert_eq!(out.len(), 1, "one outer <Note>, got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert!(
            !note
                .children
                .iter()
                .any(|c| matches!(c, MdastNode::MdxJsxFlowElement(_))),
            "the wrong-kind inner run must stay literal: {out:#?}"
        );
        assert!(r.take_diagnostics().is_empty());
    }

    /// Prose glued AFTER the outer closer is re-emitted as a following
    /// sibling, not swallowed into the body — the shape-(b) assembly
    /// contract, unchanged by the stack, now reachable through a nested
    /// spaced run.
    #[test]
    fn real_parser_spaced_nesting_re_emits_prose_glued_after_the_outer_closer() {
        let mut r = registry_with_admonitions();
        let input =
            ":::note\n\nprose above\n\n:::tip\ninner body\n:::\n\n:::\ntrailing prose glued\n";
        let out = run_real_parser(&mut r, input);

        assert_eq!(out.len(), 2, "<Note> plus the trailing prose, got {out:#?}");
        assert_eq!(flow(&out[0]).name.as_deref(), Some("Note"));
        let mut trailing = Vec::new();
        collect_text_values(&out[1..], &mut trailing);
        assert_eq!(trailing, vec!["trailing prose glued".to_string()]);
        assert_no_literal_fence(&out);
        assert!(r.take_diagnostics().is_empty());
    }

    /// The buried-opener rule, stated as a test: an opener-shaped line
    /// BEHIND a non-opener first line is not a fence event. Without that
    /// rule the buried `:::tip` would push onto the stack, the lone `:::`
    /// would close it instead of the outer, and this spaced `:::note` —
    /// which transforms today — would regress to unclosed.
    ///
    /// Pinned byte-for-byte against the pre-zfb#2416 behaviour, diagnostic
    /// included: the outer transforms, the buried run stays literal inside
    /// its body, and the zfb#2211 buried-opener scan warns about `:::tip`
    /// exactly once, at the source line it sits on.
    #[test]
    fn real_parser_spaced_outer_transforms_over_a_buried_unclosed_inner_opener() {
        let mut r = registry_with_admonitions();
        let input = ":::note\n\nprose\n:::tip\nnever closed\n\n:::\n";
        let out = run_real_parser(&mut r, input);

        assert_eq!(out.len(), 1, "one <Note>, got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        let mut texts = Vec::new();
        collect_text_values(&out, &mut texts);
        assert!(
            texts.iter().any(|t| t.contains(":::tip")),
            "the buried opener stays literal INSIDE the body, got {texts:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(
            diags.len(),
            1,
            "one buried-opener diagnostic, got {diags:#?}"
        );
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("tip"),
            "the diagnostic names the buried opener, got {:?}",
            diags[0].message
        );
        assert_eq!(diags[0].line, Some(4), "the buried fence's own source line");
    }

    /// A structured body survives the spaced nesting path: a fenced code
    /// block between the outer fences reaches the JSX body as a `Code`
    /// node, not flattened prose (the shape `mdx_jsx_emit_hast.rs`'s
    /// syntect pins depend on).
    #[test]
    fn real_parser_spaced_outer_passes_a_fenced_code_body_through() {
        let mut r = registry_with_admonitions();
        let input = ":::note\n\n```ts\nconst x = 1\n```\n\n:::\n";
        let out = run_real_parser(&mut r, input);

        assert_eq!(out.len(), 1, "one <Note>, got {out:#?}");
        let note = flow(&out[0]);
        assert!(
            note.children
                .iter()
                .any(|c| matches!(c, MdastNode::Code(_))),
            "the fenced code block must reach the body as a Code node: {out:#?}"
        );
        assert!(r.take_diagnostics().is_empty());
    }

    /// The MIXED form: the inner opener is GLUED to the outer opener (one
    /// paragraph), while both closers are spaced siblings. The shape-(b)
    /// scan has to carry the opener paragraph's OWN nesting state into the
    /// sibling scan — otherwise the outer closes on the FIRST sibling
    /// `:::` and the output leaks a literal `:::tip` plus a stray `:::`.
    #[test]
    fn real_parser_inner_opener_glued_to_the_outer_opener_still_nests() {
        let mut r = registry_with_admonitions();
        let input = ":::note\n:::tip\ninner\n\n:::\n\n:::\n";
        let out = run_real_parser(&mut r, input);

        assert_eq!(out.len(), 1, "one outer <Note>, got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        let tip = flow(
            note.children
                .iter()
                .find(|c| matches!(c, MdastNode::MdxJsxFlowElement(_)))
                .unwrap_or_else(|| panic!("the <Tip> must be a CHILD of the <Note>: {out:#?}")),
        );
        assert_eq!(tip.name.as_deref(), Some("Tip"));
        assert_eq!(body_paragraph_texts(tip), vec!["inner".to_string()]);
        assert_no_literal_fence(&out);
        assert!(r.take_diagnostics().is_empty());
    }

    /// The UNBALANCED mixed form: an unclosed `:::tip` buried in the outer
    /// opener's own paragraph, with a single sibling closer. Carrying the
    /// opener paragraph's nesting state into the sibling scan means that
    /// lone `:::` is consumed by the INNER fence, so the outer never
    /// balances and the whole run stays literal under one `note` unclosed
    /// diagnostic.
    ///
    /// Pinned because it is a deliberate flip: before the opener
    /// paragraph's own fence lines fed the scan, the outer transformed and
    /// the buried run leaked literal inside its body. Note this does NOT
    /// converge with the collapsed twin `:::note\nprose\n:::tip\nnever
    /// closed\n:::`, which leaves the outer literal but still emits a
    /// `<Tip>` for the inner — measured, not assumed. The spaced form is
    /// the more conservative of the two: an unbalanced run transforms
    /// nothing, which is what keeps it away from the anti-cascade shapes.
    #[test]
    fn real_parser_unclosed_inner_glued_to_the_outer_opener_leaves_all_literal() {
        let mut r = registry_with_admonitions();
        let input = ":::note\nprose\n:::tip\nnever closed\n\n:::\n";
        let out = run_real_parser(&mut r, input);

        assert!(
            !out.iter()
                .any(|n| matches!(n, MdastNode::MdxJsxFlowElement(_))),
            "nothing transforms — the outer never balances: {out:#?}"
        );
        let diags = r.take_diagnostics();
        assert_eq!(diags.len(), 1, "one unclosed diagnostic, got {diags:#?}");
        assert!(
            diags[0].message.contains("unclosed") && diags[0].message.contains("note"),
            "the diagnostic names the OUTER opener, got {:?}",
            diags[0].message
        );
    }

    /// The same mixed form with the inner BODY spaced away from its opener,
    /// so the outer opener paragraph is exactly `:::note` + `:::tip` — two
    /// fence lines and nothing else.
    #[test]
    fn real_parser_two_glued_openers_close_across_spaced_siblings() {
        let mut r = registry_with_admonitions();
        let input = ":::note\n:::tip\n\ninner\n\n:::\n\n:::\n";
        let out = run_real_parser(&mut r, input);

        assert_eq!(out.len(), 1, "one outer <Note>, got {out:#?}");
        let note = flow(&out[0]);
        assert_eq!(note.name.as_deref(), Some("Note"));
        assert!(
            note.children.iter().any(|c| {
                matches!(c, MdastNode::MdxJsxFlowElement(j) if j.name.as_deref() == Some("Tip"))
            }),
            "the <Tip> must be a CHILD of the <Note>: {out:#?}"
        );
        assert_no_literal_fence(&out);
        assert!(r.take_diagnostics().is_empty());
    }
}
