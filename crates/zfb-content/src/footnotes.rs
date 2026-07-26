//! Document-level GFM footnote model (issue #2025, epic #2021).
//!
//! Footnotes are the one GFM construct that **cannot** be rendered from an
//! independent per-node match arm. A `[^a]` reference and its `[^a]: …`
//! definition can sit arbitrarily far apart; the definitions render as one
//! collected section at the **end of the document**; the numbering follows
//! **first-reference order**, not source order; and each of N repeated
//! references to one definition needs its **own** backreference target so the
//! definition can link back to that specific occurrence.
//!
//! All of that is document-level state: collection, ordering, numbering and ID
//! allocation. This module owns it, once, so the two emitters
//! ([`crate::pipeline`]'s hast bridge and [`crate::mdx_jsx_emit`]'s JSX
//! emitter) consume the same decisions instead of each re-deriving them and
//! drifting. **This module renders nothing** — it produces the plan; the
//! emitters produce the markup.
//!
//! # Policy decisions (all eight bullets of issue #2025, answered)
//!
//! ## 1. Collection and ordering
//!
//! [`FootnoteModel::collect`] makes two passes over the mdast tree in document
//! order:
//!
//! - definitions are indexed by their **normalized mdast identifier**
//!   (`FootnoteDefinition::identifier`, which markdown-rs has already
//!   case-folded and whitespace-collapsed), and
//! - references are walked in document order; the **first** reference to an
//!   identifier is what creates the rendered entry.
//!
//! Definition *source* order is therefore irrelevant to output order. Entries
//! are stored in [`FootnoteModel::entries`] in first-reference order, which is
//! the order the rendered footnote section must use.
//!
//! A definition body may itself reference another footnote. Those nested
//! references are ordered by **where the containing definition renders** (in
//! the footer), not by where it was written — so the reference walk skips
//! definition subtrees on its first pass and revisits them afterwards, entry
//! by entry, in footer order. Two consequences worth stating: a nested
//! reference can never be numbered ahead of a main-body reference that renders
//! before it, and a definition that never renders (unreferenced, or a
//! discarded duplicate) contributes nothing at all — its body is never walked,
//! so it cannot mint a phantom entry or a backreference to an element that
//! does not exist.
//!
//! The walks recurse through [`markdown::mdast::Node::children`], so footnotes
//! nested inside an MDX JSX element body (`<Note>…[^a]…</Note>`) are collected
//! exactly like top-level ones.
//!
//! ## 2. Numbering
//!
//! 1-based, assigned in first-reference order: [`FootnoteEntry::number`] is
//! `index + 1`. Every reference to an entry displays that same number,
//! however many occurrences there are.
//!
//! A definition that is **never referenced** gets no number and no entry — it
//! is not rendered at all. (Numbering is defined by reference order, so an
//! unreferenced definition has no place in the sequence; this also matches
//! remark-rehype.)
//!
//! ## 3. ID allocation and escaping
//!
//! The scheme is GitHub's, including the `user-content-` clobber prefix
//! ([`FOOTNOTE_CLOBBER_PREFIX`]) that keeps footnote ids out of the way of
//! `document.getElementById`-style DOM clobbering by author content:
//!
//! | thing | id | href pointing at it |
//! |---|---|---|
//! | rendered definition (`<li>`) | `user-content-fn-{slug}` | `#user-content-fn-{slug}` |
//! | reference occurrence (`<a>`) | `user-content-fnref-{slug}` | `#user-content-fnref-{slug}` |
//!
//! `{slug}` is the label put through [`crate::plugins::heading_links::slugify`]
//! — the same github-slugger-equivalent function that produces heading
//! anchors. That is deliberate: footnote labels are author-controlled and may
//! contain anything (`[^Note (1)]`, `[^注 1]`, `[^a/b]`, quotes, angle
//! brackets), and this repo has already been bitten by ad-hoc anchor slugging
//! with full-width Japanese characters. Reusing `slugify` means:
//!
//! - all ASCII punctuation that is unsafe in an id or a URL fragment (`"`,
//!   `'`, `<`, `>`, `&`, `/`, `#`, `?`, `%`, backtick, braces, …) and every
//!   control character is **stripped**;
//! - runs of whitespace collapse to a single `-`;
//! - ASCII letters are lowercased;
//! - everything else — CJK, kana, hangul, accented Latin, emoji — passes
//!   through unchanged, matching how heading anchors already behave in this
//!   codebase, and staying valid in both an HTML5 `id` and a URL fragment.
//!
//! A label that slugifies to the **empty string** (e.g. `[^!!!]`) would
//! produce a bare `user-content-fn-` id, so it falls back to the entry's
//! 1-based number instead: `user-content-fn-1`, `user-content-fn-2`, ….
//!
//! ## 4. Collision handling
//!
//! Slugging is lossy, so two distinct labels can collapse onto one slug
//! (`[^a.b]` and `[^a/b]` both slug to `a-b`). Note that a pure difference of
//! ASCII case (`[^A]` vs `[^a]`) never reaches here — markdown-rs case-folds
//! the identifier upstream, so those are already one footnote.
//!
//! Every footnote id in the document — definition ids **and** per-occurrence
//! reference ids alike — is allocated from **one shared allocator**
//! ([`IdAllocator`]) that appends `-1`, `-2`, … to repeats, mirroring
//! github-slugger's repeat-numbering (and hence
//! [`crate::plugins::heading_links::next_slug`]'s shape).
//!
//! Two properties of that allocator are load-bearing, and neither is free:
//!
//! - **One namespace for both families.** Without it, the 2nd occurrence of
//!   label `a` and the 1st occurrence of a label that slugged to `a-1` would
//!   both want `fnref-a-1`.
//! - **Dedup on the *produced* id, not on the base.** A pure per-base counter
//!   (which is what `next_slug` is) does not actually guarantee uniqueness:
//!   `fnref-a` allocated twice yields `fnref-a-1`, which a *separate* base
//!   `fnref-a-1` will then happily hand out again. [`IdAllocator`] therefore
//!   keeps the set of ids it has already emitted and skips forward until it
//!   finds a free one.
//!
//! The ordinal in a reference id is therefore an **allocation counter, not an
//! occurrence index**: the first occurrence is `fnref-{slug}`, the second
//! `fnref-{slug}-1`, and so on. Use [`FootnoteRef::occurrence`] when you need
//! the human-facing "back to reference 1-2" ordinal.
//!
//! Not in scope: a footnote id colliding with an author-written heading id
//! that literally starts with `user-content-fn`. The clobber prefix exists to
//! make that vanishingly unlikely, and heading ids are allocated by a separate
//! per-document allocator.
//!
//! ## 5. Per-occurrence reference IDs
//!
//! [`FootnoteEntry::references`] holds one [`FootnoteRef`] per occurrence, in
//! document order, each with its own unique [`FootnoteRef::id`]. The rendered
//! definition emits one backreference link per entry in that vector.
//!
//! ## 6. Backreference markup
//!
//! The model does not emit markup, but it does pin the shape both emitters
//! must produce, so they cannot drift. See [`FOOTNOTE_SECTION_CLASS`],
//! [`FOOTNOTE_LABEL_ID`], [`FOOTNOTE_BACKREF_MARKER`],
//! [`FootnoteRef::backref_aria_label`] and the module-level shape sketch:
//!
//! ```text
//! reference:  <sup><a href="#user-content-fn-a" id="user-content-fnref-a"
//!                     data-footnote-ref="" aria-describedby="footnote-label">1</a></sup>
//!
//! section:    <section data-footnotes="" class="footnotes">
//!               <div role="heading" aria-level="2" class="sr-only"
//!                    style="…visually-hidden…" id="footnote-label">Footnotes</div>
//!               <ol>
//!                 <li id="user-content-fn-a"> …definition body…
//!                   <a href="#user-content-fnref-a" data-footnote-backref=""
//!                      aria-label="Back to reference 1">↩</a>
//!                 </li>
//!               </ol>
//!             </section>
//! ```
//!
//! `data-footnote-ref` / `data-footnote-backref` / `data-footnotes` are
//! spelled as **empty-valued attributes** (`attr=""`), not bare booleans,
//! on BOTH emit paths. That is what keeps a footnote nested inside a JSX
//! component's children serializing identically to one at the document's
//! top level — see [`crate::mdx_jsx_emit`]'s
//! `jsx_footnote_reference_marker`.
//!
//! The heading is a `div` carrying `role="heading" aria-level="2"` rather
//! than a real `<h2>` (see [`crate::pipeline`]'s `render_footnote_section`
//! for why), and it is hidden by the inline [`FOOTNOTE_LABEL_STYLE`]
//! rather than by the `sr-only` class alone.
//!
//! ## 7. Missing definition
//!
//! **Nothing to do — and deliberately so.** markdown-rs only tokenizes
//! `[^label]` into a `FootnoteReference` node when a matching definition
//! exists somewhere in the document; otherwise the brackets stay ordinary
//! literal text. So an unmatched reference never reaches this model, and the
//! model must not invent a "fix" for it. (Wave 1 pinned this as a passing,
//! non-ignored characterization test rather than a red one — see
//! `pipeline::tests::unmatched_reference_stays_literal_text`.)
//!
//! For robustness against hand-built mdast, [`FootnoteModel::collect`] simply
//! skips a `FootnoteReference` whose identifier has no definition: no entry,
//! no number, no id. [`FootnoteModel::reference`] then returns `None` and the
//! emitter renders nothing for that node.
//!
//! ## 8. Duplicate definitions
//!
//! **First definition wins**, later ones are discarded — matching CommonMark's
//! rule for duplicate link reference definitions, so an author who knows one
//! rule already knows the other. The duplicates collapse into exactly **one**
//! entry with one number, one id, and one rendered body.
//!
//! No diagnostic is emitted for either the duplicate or the unreferenced case.
//! Both are silent-but-correct in GitHub's own renderer, and this epic exists
//! to stop footnotes being *silently dropped*; a warning on legal (if
//! redundant) markup would be noise. If a future wave wants one, it belongs in
//! the diagnostics channel, not here.
//!
//! ## 9. Accessibility attributes
//!
//! - reference link: `data-footnote-ref` and
//!   `aria-describedby="footnote-label"`, pointing at the visually-hidden
//!   section heading so a screen reader announces "Footnotes" when the link
//!   gets focus;
//! - backreference link: `data-footnote-backref` and an
//!   `aria-label="Back to reference {n}"` (or `{n}-{k}` for the k-th
//!   occurrence) — [`FootnoteRef::backref_aria_label`] — because the visible
//!   `↩` glyph is meaningless when announced;
//! - the section carries `data-footnotes`, and its visually-hidden
//!   `role="heading"` landmark (`id="footnote-label"`) is the
//!   `aria-describedby` target. It is hidden by [`FOOTNOTE_LABEL_STYLE`],
//!   an inline style, so the hiding never depends on the consuming
//!   project happening to define a `.sr-only` rule.

use std::collections::{HashMap, HashSet};

use markdown::mdast::Node as MdastNode;

use crate::plugins::heading_links::slugify;

/// DOM-clobbering guard prefix on every footnote id, matching GitHub and
/// remark-rehype's `clobberPrefix` default.
pub const FOOTNOTE_CLOBBER_PREFIX: &str = "user-content-";

/// `class` on the collected footnote `<section>`.
pub const FOOTNOTE_SECTION_CLASS: &str = "footnotes";

/// `id` of the visually-hidden section heading that reference links point at
/// via `aria-describedby`.
pub const FOOTNOTE_LABEL_ID: &str = "footnote-label";

/// Visible text of the visually-hidden section heading.
pub const FOOTNOTE_LABEL_TEXT: &str = "Footnotes";

/// Inline `style` that visually hides the section heading while leaving it
/// in the accessibility tree.
///
/// This is deliberately an INLINE style rather than a rule keyed off the
/// `sr-only` class the heading also carries. `sr-only` is a Tailwind
/// utility, and zfb emits this class from Rust — Tailwind's content scan
/// never sees the string, so the utility is never generated, and zfb ships
/// no base stylesheet that could define it either (`zfb-css`'s
/// `assets/zfb-hi.css` is the syntax-highlighting token sheet and is
/// opt-in). Without the inline style the heading renders as ordinary
/// visible body text in essentially every project, contradicting the
/// documented "visually hidden landmark" contract. The class stays for
/// projects that DO define it and for styling hooks.
///
/// The declarations are the standard visually-hidden recipe (clip to a
/// 1×1 box, no wrapping, no border); `clip` is kept alongside `clip-path`
/// for older engines.
pub const FOOTNOTE_LABEL_STYLE: &str = "position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);clip-path:inset(50%);white-space:nowrap;border:0";

/// Visible glyph of a backreference link.
pub const FOOTNOTE_BACKREF_MARKER: &str = "\u{21a9}";

/// Per-document allocator handing out unique footnote ids.
///
/// `allocate("fn-a")` returns `fn-a`, then `fn-a-1`, `fn-a-2`, … — but
/// crucially it dedups on the **produced** id, not on the base, so a later
/// `allocate("fn-a-1")` cannot re-emit an id an earlier `allocate("fn-a")`
/// already handed out. See the "Collision handling" module doc.
///
/// Definition ids and reference ids share one instance, which is what keeps
/// the two families from colliding with each other.
#[derive(Clone, Debug, Default)]
pub struct IdAllocator {
    taken: HashSet<String>,
    counters: HashMap<String, usize>,
}

impl IdAllocator {
    /// A fresh allocator with no ids taken.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a unique id for `base`.
    pub fn allocate(&mut self, base: &str) -> String {
        let mut n = self.counters.get(base).copied().unwrap_or(0);
        loop {
            let candidate = if n == 0 {
                base.to_string()
            } else {
                format!("{base}-{n}")
            };
            n += 1;
            if self.taken.insert(candidate.clone()) {
                self.counters.insert(base.to_string(), n);
                return candidate;
            }
        }
    }
}

/// One reference occurrence of a footnote, in document order.
///
/// Every occurrence gets its own [`id`](Self::id) so the rendered definition
/// can link back to *this* usage specifically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FootnoteRef {
    /// Unique element id for this occurrence, e.g. `user-content-fnref-a`.
    pub id: String,
    /// The footnote's number — the same for every occurrence of one entry.
    pub number: usize,
    /// 1-based ordinal of this occurrence among the references to its entry.
    pub occurrence: usize,
}

impl FootnoteRef {
    /// `href` the definition's backreference link uses to return here.
    #[must_use]
    pub fn backref_href(&self) -> String {
        format!("#{}", self.id)
    }

    /// `aria-label` for the backreference link: `Back to reference 1` for a
    /// single-occurrence footnote, `Back to reference 1-2` for the 2nd
    /// occurrence of footnote 1 (GitHub's own wording).
    #[must_use]
    pub fn backref_aria_label(&self) -> String {
        if self.occurrence <= 1 {
            format!("Back to reference {}", self.number)
        } else {
            format!("Back to reference {}-{}", self.number, self.occurrence)
        }
    }
}

/// One rendered footnote: a definition body plus every reference to it.
#[derive(Clone, Debug)]
pub struct FootnoteEntry {
    /// Normalized mdast identifier this entry was keyed on.
    pub identifier: String,
    /// 1-based number, assigned in first-reference order.
    pub number: usize,
    /// Element id of the rendered definition, e.g. `user-content-fn-a`.
    pub id: String,
    /// The winning definition's children (first definition wins).
    pub body: Vec<MdastNode>,
    /// One entry per reference occurrence, in document order.
    pub references: Vec<FootnoteRef>,
}

impl FootnoteEntry {
    /// `href` a reference link uses to jump to this definition.
    #[must_use]
    pub fn href(&self) -> String {
        format!("#{}", self.id)
    }
}

/// The document's whole footnote plan: which footnotes are rendered, in what
/// order, under what ids.
///
/// See the [module docs](self) for every policy decision this encodes.
#[derive(Clone, Debug, Default)]
pub struct FootnoteModel {
    entries: Vec<FootnoteEntry>,
    /// identifier -> index into `entries`.
    index: HashMap<String, usize>,
}

impl FootnoteModel {
    /// Build the model for one document.
    ///
    /// Pass 1 indexes definitions by identifier (first wins). Pass 2 walks the
    /// **main body** — every reference *outside* a definition subtree, in
    /// document order — creating an entry the first time an identifier is
    /// referenced and appending a per-occurrence [`FootnoteRef`] every time.
    /// Pass 3 then walks the bodies of the entries created so far, in
    /// **rendered footer order**, picking up nested references; entries it
    /// creates join the end of the list and are themselves walked in turn.
    ///
    /// Pass 3 is what makes a reference nested inside a definition body number
    /// by where it *renders* rather than where it was *written*, and is why an
    /// unreferenced (or duplicate-discarded) definition's body can never
    /// contribute a phantom entry: that body is never walked at all.
    #[must_use]
    pub fn collect(root: &MdastNode) -> Self {
        let mut definition_bodies: HashMap<&str, &Vec<MdastNode>> = HashMap::new();
        collect_definitions(root, &mut definition_bodies);

        let mut main_body_order: Vec<String> = Vec::new();
        collect_reference_order(root, &mut main_body_order);

        let mut model = Self::default();
        // One shared allocator namespace for definition ids AND reference
        // ids — see the "Collision handling" module doc.
        let mut allocator = IdAllocator::new();

        for identifier in main_body_order {
            model.record_reference(&identifier, &definition_bodies, &mut allocator);
        }

        // Rendered-footer order: each entry's body may reference further
        // footnotes, which render after it. `entries` grows as we go, so the
        // index-based loop naturally covers newcomers too — and terminates,
        // since each entry's body is walked exactly once.
        let mut i = 0;
        while i < model.entries.len() {
            let mut nested: Vec<String> = Vec::new();
            for child in &model.entries[i].body {
                collect_reference_order(child, &mut nested);
            }
            for identifier in nested {
                model.record_reference(&identifier, &definition_bodies, &mut allocator);
            }
            i += 1;
        }

        model
    }

    /// Register one reference occurrence of `identifier`, creating its entry
    /// (and allocating its definition id) on first sight.
    ///
    /// A reference with no matching definition is skipped entirely —
    /// markdown-rs never produces one, and inventing an entry for it would
    /// emit a marker linking at an id nothing renders.
    fn record_reference(
        &mut self,
        identifier: &str,
        definition_bodies: &HashMap<&str, &Vec<MdastNode>>,
        allocator: &mut IdAllocator,
    ) {
        let Some(body) = definition_bodies.get(identifier) else {
            return;
        };
        let entry_index = match self.index.get(identifier) {
            Some(i) => *i,
            None => {
                let number = self.entries.len() + 1;
                let base = slug_base(identifier, number);
                let id = format!(
                    "{FOOTNOTE_CLOBBER_PREFIX}{}",
                    allocator.allocate(&format!("fn-{base}"))
                );
                self.entries.push(FootnoteEntry {
                    identifier: identifier.to_string(),
                    number,
                    id,
                    body: (*body).clone(),
                    references: Vec::new(),
                });
                let i = self.entries.len() - 1;
                self.index.insert(identifier.to_string(), i);
                i
            }
        };
        let entry = &mut self.entries[entry_index];
        let base = slug_base(&entry.identifier, entry.number);
        let id = format!(
            "{FOOTNOTE_CLOBBER_PREFIX}{}",
            allocator.allocate(&format!("fnref-{base}"))
        );
        let occurrence = entry.references.len() + 1;
        entry.references.push(FootnoteRef {
            id,
            number: entry.number,
            occurrence,
        });
    }

    /// Rendered footnotes, in first-reference order. Empty when the document
    /// has none — emitters must then append no section at all.
    #[must_use]
    pub fn entries(&self) -> &[FootnoteEntry] {
        &self.entries
    }

    /// `true` when the document renders no footnote section.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total reference occurrences across every entry — i.e. exactly how
    /// many `FootnoteReference` nodes [`collect`](Self::collect) reached.
    ///
    /// Emitters compare this against [`FootnoteCursor::consumed_total`] to
    /// prove their own walk visited the same set: the model recurses
    /// through every `Node::children()`, while an emitter's match
    /// statement can drop a whole subtree through its catch-all (a
    /// `LinkReference` body, say). A reference stranded in a dropped
    /// subtree would still be numbered and would still emit a definition
    /// whose backreference anchor points at nothing.
    #[must_use]
    pub fn total_references(&self) -> usize {
        self.entries.iter().map(|e| e.references.len()).sum()
    }

    /// The entry for `identifier`, or `None` when it is not rendered (no
    /// definition, or a definition nothing references).
    #[must_use]
    pub fn entry(&self, identifier: &str) -> Option<&FootnoteEntry> {
        self.index.get(identifier).map(|i| &self.entries[*i])
    }

    /// The `occurrence`-th (0-based) reference to `identifier`.
    ///
    /// Emitters walking the tree keep their own per-identifier counter — see
    /// [`FootnoteCursor`], which does that bookkeeping — because the two emit
    /// paths visit the tree independently and neither should have to
    /// re-derive occurrence numbering.
    #[must_use]
    pub fn reference(&self, identifier: &str, occurrence: usize) -> Option<&FootnoteRef> {
        self.entry(identifier)?.references.get(occurrence)
    }

    /// A fresh cursor over this model.
    #[must_use]
    pub fn cursor(&self) -> FootnoteCursor<'_> {
        FootnoteCursor {
            model: self,
            consumed: HashMap::new(),
        }
    }
}

/// Per-emitter walk state: hands out the next [`FootnoteRef`] each time a
/// `FootnoteReference` node for a given identifier is reached.
///
/// Keyed per identifier rather than by a single global counter, so it stays
/// correct regardless of the order an emitter happens to visit references in.
#[derive(Debug)]
pub struct FootnoteCursor<'a> {
    model: &'a FootnoteModel,
    consumed: HashMap<String, usize>,
}

impl<'a> FootnoteCursor<'a> {
    /// The model this cursor walks.
    #[must_use]
    pub fn model(&self) -> &'a FootnoteModel {
        self.model
    }

    /// How many occurrences this cursor has handed out so far, across
    /// every identifier. Compare against [`FootnoteModel::total_references`]
    /// once a document has been fully emitted — see that method's docs.
    #[must_use]
    pub fn consumed_total(&self) -> usize {
        self.consumed.values().sum()
    }

    /// Consume and return the next reference occurrence for `identifier`,
    /// together with its entry.
    ///
    /// `None` when the identifier has no rendered entry (unmatched reference)
    /// or when more references are visited than were collected — in both
    /// cases the emitter renders nothing for that node.
    pub fn next_reference(
        &mut self,
        identifier: &str,
    ) -> Option<(&'a FootnoteEntry, &'a FootnoteRef)> {
        let entry = self.model.entry(identifier)?;
        let slot = self.consumed.entry(identifier.to_string()).or_insert(0);
        let r = entry.references.get(*slot)?;
        *slot += 1;
        Some((entry, r))
    }
}

/// Slug base for a label: the github-slugger slug, falling back to the
/// footnote's number when the label slugifies to nothing.
fn slug_base(identifier: &str, number: usize) -> String {
    let slug = slugify(identifier);
    if slug.is_empty() {
        number.to_string()
    } else {
        slug
    }
}

fn collect_definitions<'a>(node: &'a MdastNode, out: &mut HashMap<&'a str, &'a Vec<MdastNode>>) {
    if let MdastNode::FootnoteDefinition(def) = node {
        // First definition wins; `or_insert` discards later duplicates.
        out.entry(def.identifier.as_str()).or_insert(&def.children);
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_definitions(child, out);
        }
    }
}

/// Collect reference identifiers in document order, **not descending into
/// footnote definition subtrees**.
///
/// A definition's body renders in the footer, not where it was written, so its
/// own nested references must be ordered by the definition's rendered position
/// — `FootnoteModel::collect`'s pass 3 does that separately. Walking them here
/// would number a nested reference by its source position and, worse, mint
/// entries out of definitions that never render at all.
fn collect_reference_order(node: &MdastNode, out: &mut Vec<String>) {
    if matches!(node, MdastNode::FootnoteDefinition(_)) {
        return;
    }
    if let MdastNode::FootnoteReference(r) = node {
        out.push(r.identifier.clone());
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_reference_order(child, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{constructs_for_pipeline, ResolvedGfmConstructs};

    /// Parse with every GFM construct on (footnotes are off in the
    /// `CONSERVATIVE` default) and MDX enabled, matching what
    /// `Pipeline::with_resolved_gfm_constructs(ALL_ON)` feeds
    /// `markdown::to_mdast`.
    fn parse(src: &str) -> MdastNode {
        let options = markdown::ParseOptions {
            constructs: constructs_for_pipeline(ResolvedGfmConstructs::ALL_ON),
            ..markdown::ParseOptions::mdx()
        };
        markdown::to_mdast(src, &options).expect("parse ok")
    }

    fn model(src: &str) -> FootnoteModel {
        FootnoteModel::collect(&parse(src))
    }

    fn body_text(entry: &FootnoteEntry) -> String {
        entry.body.iter().map(MdastNode::to_string).collect()
    }

    #[test]
    fn empty_document_has_no_entries() {
        let m = model("Just a paragraph.\n");
        assert!(m.is_empty());
        assert!(m.entries().is_empty());
    }

    #[test]
    fn single_reference_and_definition_are_associated() {
        let m = model("Ref one[^a] end.\n\n[^a]: Definition body.\n");
        assert_eq!(m.entries().len(), 1);
        let e = &m.entries()[0];
        assert_eq!(e.identifier, "a");
        assert_eq!(e.number, 1);
        assert_eq!(e.id, "user-content-fn-a");
        assert_eq!(body_text(e), "Definition body.");
        assert_eq!(e.references.len(), 1);
        assert_eq!(e.references[0].id, "user-content-fnref-a");
        assert_eq!(e.references[0].occurrence, 1);
        assert_eq!(e.references[0].backref_href(), "#user-content-fnref-a");
        assert_eq!(e.href(), "#user-content-fn-a");
    }

    #[test]
    fn repeated_references_get_distinct_ids_but_share_a_number() {
        let m = model("Ref one[^a] and ref again[^a] end.\n\n[^a]: Shared def.\n");
        assert_eq!(m.entries().len(), 1);
        let e = &m.entries()[0];
        assert_eq!(e.references.len(), 2);
        assert_ne!(e.references[0].id, e.references[1].id);
        assert_eq!(e.references[0].number, e.references[1].number);
        assert_eq!(e.references[0].occurrence, 1);
        assert_eq!(e.references[1].occurrence, 2);
        assert_eq!(e.references[1].id, "user-content-fnref-a-1");
    }

    #[test]
    fn numbering_and_order_follow_first_reference_not_definition_order() {
        let m =
            model("First[^a] then second[^b] end.\n\n[^b]: Second body.\n\n[^a]: First body.\n");
        assert_eq!(m.entries().len(), 2);
        assert_eq!(m.entries()[0].identifier, "a");
        assert_eq!(m.entries()[0].number, 1);
        assert_eq!(body_text(&m.entries()[0]), "First body.");
        assert_eq!(m.entries()[1].identifier, "b");
        assert_eq!(m.entries()[1].number, 2);
        assert_eq!(body_text(&m.entries()[1]), "Second body.");
    }

    #[test]
    fn number_follows_first_reference_even_when_referenced_again_later() {
        let m = model("A[^a] B[^b] A again[^a].\n\n[^a]: One.\n\n[^b]: Two.\n");
        assert_eq!(m.entries()[0].number, 1);
        assert_eq!(m.entries()[0].references.len(), 2);
        assert_eq!(m.entries()[1].number, 2);
    }

    #[test]
    fn duplicate_definitions_collapse_to_one_entry_and_the_first_wins() {
        let m = model("Dup label[^a] end.\n\n[^a]: First.\n\n[^a]: Second (dup).\n");
        assert_eq!(m.entries().len(), 1);
        let text = body_text(&m.entries()[0]);
        assert!(
            text.contains("First."),
            "first definition must win: {text:?}"
        );
        assert!(
            !text.contains("Second (dup)."),
            "later duplicate must be discarded: {text:?}"
        );
    }

    #[test]
    fn unreferenced_definition_is_not_rendered() {
        // `[^b]` is defined but never referenced, so it has no number and no
        // entry; `[^a]` still numbers 1.
        let m = model("Only a[^a].\n\n[^a]: A body.\n\n[^b]: B body.\n");
        assert_eq!(m.entries().len(), 1);
        assert_eq!(m.entries()[0].identifier, "a");
        assert!(m.entry("b").is_none());
    }

    #[test]
    fn reference_without_definition_yields_no_entry() {
        // markdown-rs does not even produce a FootnoteReference node here —
        // the brackets stay literal text — so the model sees nothing. Pinned
        // so a future parser change cannot silently start inventing entries.
        let m = model("Dangling ref[^missing] end.\n");
        assert!(m.is_empty());
        assert!(m.entry("missing").is_none());
    }

    // ---- references nested inside definition bodies ----

    #[test]
    fn reference_inside_a_definition_body_is_numbered_by_footer_position() {
        // `[^a]`'s body references `[^b]`, and `[^a]`'s definition is written
        // BEFORE the main-body reference to `[^c]`. `b` renders after `a` in
        // the footer, so it must number after `c`, which is referenced in the
        // main body — source position must not win.
        let m = model(
            "Main[^a] then[^c] end.\n\n\
             [^a]: A body[^b].\n\n[^b]: B body.\n\n[^c]: C body.\n",
        );
        let numbering: Vec<(&str, usize)> = m
            .entries()
            .iter()
            .map(|e| (e.identifier.as_str(), e.number))
            .collect();
        assert_eq!(numbering, vec![("a", 1), ("c", 2), ("b", 3)]);
        assert_eq!(m.entry("b").unwrap().references.len(), 1);
        assert_unique_ids(&m);
    }

    #[test]
    fn unreferenced_definition_body_contributes_no_phantom_entry() {
        // `[^ghost]` is never referenced, so it never renders — and neither
        // does the `[^hidden]` its body references. Walking its body anyway
        // would mint an entry whose backreference points at a marker that is
        // never emitted.
        let m = model(
            "Only a[^a].\n\n\
             [^a]: A body.\n\n[^ghost]: Ghost[^hidden].\n\n[^hidden]: Hidden body.\n",
        );
        assert_eq!(m.entries().len(), 1);
        assert_eq!(m.entries()[0].identifier, "a");
        assert!(m.entry("ghost").is_none());
        assert!(m.entry("hidden").is_none());
    }

    #[test]
    fn discarded_duplicate_definition_body_contributes_no_phantom_entry() {
        // The losing duplicate's body references `[^ghost]`. Only the winning
        // body renders, so `ghost` must not appear.
        let m = model("Dup[^a].\n\n[^a]: First.\n\n[^a]: Second[^ghost].\n\n[^ghost]: G.\n");
        assert_eq!(m.entries().len(), 1);
        assert!(m.entry("ghost").is_none());
    }

    #[test]
    fn self_referencing_definition_body_terminates_with_one_extra_occurrence() {
        let m = model("Main[^a].\n\n[^a]: A body[^a].\n");
        assert_eq!(m.entries().len(), 1);
        assert_eq!(m.entries()[0].references.len(), 2);
        assert_unique_ids(&m);
    }

    #[test]
    fn mutually_referencing_definition_bodies_terminate() {
        let m = model("Main[^a].\n\n[^a]: A body[^b].\n\n[^b]: B body[^a].\n");
        assert_eq!(m.entries().len(), 2);
        assert_eq!(m.entries()[0].identifier, "a");
        assert_eq!(m.entries()[1].identifier, "b");
        assert_eq!(m.entry("a").unwrap().references.len(), 2);
        assert_eq!(m.entry("b").unwrap().references.len(), 1);
        assert_unique_ids(&m);
    }

    #[test]
    fn footnotes_nested_inside_a_jsx_element_body_are_collected() {
        let m = model("<Note>\n\nRef[^a] end.\n\n[^a]: Body text.\n\n</Note>\n");
        assert_eq!(m.entries().len(), 1);
        assert_eq!(m.entries()[0].number, 1);
        assert_eq!(m.entries()[0].references.len(), 1);
    }

    // ---- escaping ----
    //
    // NOTE on fixtures: markdown-rs's footnote-label tokenizer rejects a
    // label containing whitespace (`[^a b]` never becomes a reference at
    // all), and `[^a<b]` is a hard MDX parse error because `<` opens a JSX
    // tag. So the whitespace and markup-character cases are exercised
    // directly against `slug_base` below rather than through a document that
    // could never contain them.

    #[test]
    fn label_punctuation_is_slugified_out_of_the_id() {
        let m = model("Ref[^Note.1] end.\n\n[^Note.1]: Body.\n");
        let e = &m.entries()[0];
        assert_eq!(e.id, "user-content-fn-note-1");
        assert_eq!(e.references[0].id, "user-content-fnref-note-1");
    }

    #[test]
    fn label_quote_and_ampersand_cannot_escape_an_attribute() {
        // markdown-rs happily produces identifiers containing `"` and `&`
        // (verified: `[^a"b]` yields identifier `a"b`), which would break out
        // of an `id="…"` attribute if written through raw.
        for label in ["a\"b", "a&b", "a'b", "x%y"] {
            let src = format!("Ref[^{label}] end.\n\n[^{label}]: Body.\n");
            let m = model(&src);
            let e = m
                .entries()
                .first()
                .unwrap_or_else(|| panic!("label {label:?} did not parse as a footnote"));
            for id in std::iter::once(&e.id).chain(e.references.iter().map(|r| &r.id)) {
                assert!(
                    !id.contains(['"', '<', '>', '&', '\'', '%']),
                    "id for label {label:?} must not carry attribute- or \
                     URL-unsafe characters, got {id:?}"
                );
            }
        }
    }

    #[test]
    fn slug_base_collapses_whitespace_and_strips_markup_characters() {
        assert_eq!(slug_base("a  \t b", 1), "a-b");
        assert_eq!(slug_base("a\"><script>", 1), "a-script");
        assert_eq!(slug_base("Mixed CASE", 1), "mixed-case");
    }

    #[test]
    fn japanese_label_survives_slugification() {
        // The repo has been bitten by full-width characters in anchor slugs
        // before: kana/kanji/full-width digits are not in github-slugger's
        // ASCII strip set and must pass through unchanged.
        let m = model("参照[^脚注１] 終わり。\n\n[^脚注１]: 本文。\n");
        let e = &m.entries()[0];
        assert_eq!(e.id, "user-content-fn-脚注１");
        assert_eq!(e.references[0].id, "user-content-fnref-脚注１");
        assert!(!e.id.contains(' '), "no raw spaces in an id: {:?}", e.id);
    }

    #[test]
    fn full_width_japanese_punctuation_passes_through() {
        // `。` / `、` are punctuation but NOT in the ASCII strip set, so they
        // survive — the exact full-width pitfall this repo has hit before.
        assert_eq!(slug_base("脚注。、", 1), "脚注。、");
    }

    #[test]
    fn label_slugifying_to_nothing_falls_back_to_the_number() {
        let m = model("Ref[^!!!] end.\n\n[^!!!]: Body.\n");
        let e = &m.entries()[0];
        assert_eq!(e.id, "user-content-fn-1");
        assert_eq!(e.references[0].id, "user-content-fnref-1");
    }

    #[test]
    fn two_labels_slugging_to_nothing_both_fall_back_and_stay_distinct() {
        let m = model("A[^!!!] B[^???] end.\n\n[^!!!]: One.\n\n[^???]: Two.\n");
        assert_eq!(m.entries().len(), 2);
        assert_eq!(m.entries()[0].id, "user-content-fn-1");
        assert_eq!(m.entries()[1].id, "user-content-fn-2");
    }

    // ---- collisions ----

    #[test]
    fn two_labels_slugging_to_the_same_base_get_distinct_ids() {
        let m = model("A[^a.b] B[^a/b] end.\n\n[^a.b]: First.\n\n[^a/b]: Second.\n");
        assert_eq!(m.entries().len(), 2);
        assert_eq!(m.entries()[0].id, "user-content-fn-a-b");
        assert_eq!(m.entries()[1].id, "user-content-fn-a-b-1");
    }

    #[test]
    fn definition_and_reference_id_families_share_one_namespace() {
        // `[^a]` referenced twice wants `fnref-a` then `fnref-a-1`; the label
        // `[^a-1]` wants `fnref-a-1` too. One shared allocator that dedups on
        // the PRODUCED id is what keeps all of them unique — a per-base
        // counter alone emits `fnref-a-1` twice here.
        let m = model("A[^a] A[^a] C[^a-1] end.\n\n[^a]: One.\n\n[^a-1]: Two.\n");
        assert_unique_ids(&m);
    }

    #[test]
    fn many_labels_slugging_to_one_base_all_stay_unique() {
        let m = model(
            "A[^x.y] B[^x/y] C[^x?y] D[^x-y] end.\n\n\
             [^x.y]: 1.\n\n[^x/y]: 2.\n\n[^x?y]: 3.\n\n[^x-y]: 4.\n",
        );
        assert_eq!(m.entries().len(), 4);
        assert_unique_ids(&m);
    }

    #[test]
    fn repeated_references_to_colliding_labels_stay_unique() {
        let m = model(
            "A[^a] A[^a] A[^a] B[^a-1] B[^a-1] C[^a-2] end.\n\n\
             [^a]: One.\n\n[^a-1]: Two.\n\n[^a-2]: Three.\n",
        );
        assert_eq!(m.entries().len(), 3);
        assert_unique_ids(&m);
    }

    /// Every definition id and every per-occurrence reference id in the
    /// document must be pairwise distinct.
    fn assert_unique_ids(m: &FootnoteModel) {
        let mut ids: Vec<&str> = Vec::new();
        for e in m.entries() {
            ids.push(&e.id);
            for r in &e.references {
                ids.push(&r.id);
            }
        }
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ids.len(),
            "every footnote id in a document must be unique, got {ids:?}"
        );
    }

    #[test]
    fn allocator_dedups_on_the_produced_id_not_the_base() {
        let mut a = IdAllocator::new();
        assert_eq!(a.allocate("fnref-a"), "fnref-a");
        assert_eq!(a.allocate("fnref-a"), "fnref-a-1");
        // A different base whose first candidate is already taken must skip
        // forward rather than re-emit it.
        assert_eq!(a.allocate("fnref-a-1"), "fnref-a-1-1");
        assert_eq!(a.allocate("fnref-a"), "fnref-a-2");
    }

    // ---- backreference / a11y shape ----

    #[test]
    fn backref_aria_label_names_the_occurrence() {
        let m = model("A[^a] again[^a].\n\n[^a]: Body.\n");
        let refs = &m.entries()[0].references;
        assert_eq!(refs[0].backref_aria_label(), "Back to reference 1");
        assert_eq!(refs[1].backref_aria_label(), "Back to reference 1-2");
    }

    // ---- cursor ----

    #[test]
    fn cursor_hands_out_each_occurrence_once_in_order() {
        let m = model("A[^a] B[^b] A[^a].\n\n[^a]: One.\n\n[^b]: Two.\n");
        let mut c = m.cursor();
        let (e1, r1) = c.next_reference("a").expect("first a");
        assert_eq!((e1.number, r1.occurrence), (1, 1));
        let (e2, r2) = c.next_reference("b").expect("first b");
        assert_eq!((e2.number, r2.occurrence), (2, 1));
        let (e3, r3) = c.next_reference("a").expect("second a");
        assert_eq!((e3.number, r3.occurrence), (1, 2));
        assert_ne!(r1.id, r3.id);
        assert!(
            c.next_reference("a").is_none(),
            "no third occurrence was collected"
        );
        assert!(c.next_reference("nope").is_none());
    }
}
