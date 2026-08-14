//! Transclusion of other markdown files via `:::include{file="./path.md"}`.
//!
//! # Syntax
//!
//! ```text
//! :::include{file="./snippet.md"}
//! ```
//!
//! Inline a file's entire parsed mdast at the include site. The `file`
//! attribute is resolved relative to the **current source file** (from
//! `BuildContext.source_path`).
//!
//! ## Line-range option
//!
//! ```text
//! :::include{file="./foo.rs" lines="10-30"}
//! ```
//!
//! Slice only lines 10–30 (1-indexed, inclusive) of the file before parsing.
//! Works for both `.md`/`.mdx` content and arbitrary text files paired with
//! `code=true`.
//!
//! ## Code-fence wrap option
//!
//! ```text
//! :::include{file="./foo.rs" code=true lang="rust"}
//! ```
//!
//! Instead of parsing the included content as Markdown, wrap it verbatim in a
//! fenced `Code` node (`~~~lang … ~~~`). Useful for embedding source files
//! as highlighted code blocks. `lang` defaults to `""` when absent.
//!
//! # Parse shape
//!
//! The MDX parser renders `:::include{file="./x.md"}` as a `Paragraph` with
//! exactly two inline children:
//!
//! 1. `Text { value: ":::include" }`
//! 2. `MdxTextExpression { value: "file=\"./x.md\" …" }`
//!
//! This module matches that two-child pattern and replaces the parent
//! `Paragraph` with the spliced nodes from the included file.
//!
//! # Security
//!
//! - Absolute `file` paths are rejected with an error diagnostic.
//! - Paths that escape `project_root` (after canonicalization) are rejected.
//! - Symlink chains that point outside `project_root` are rejected.
//!
//! # Cycle detection
//!
//! The visitor maintains a set of currently-being-transcluded absolute paths.
//! If A→B→A is detected, an Error-severity diagnostic is emitted and the
//! second inclusion is skipped (the `:::include` node is left in the tree as
//! an empty paragraph rather than causing infinite recursion).
//!
//! # Config
//!
//! Wire via `features.transclude: {}` or `features.transclude: { maxDepth: N }`
//! in `zfb.config.ts`. Phase: **mdast** (runs early so included content is
//! processed by subsequent mdast visitors).
//!
//! # Read recording (zfb#944)
//!
//! When constructed with a [`ReadRecorder`]
//! ([`TranscludePlugin::with_recorder`]), EVERY attempted include read —
//! successful reads, missing files, and read errors — is reported to the
//! recorder so the MDX compile cache can store a dependency manifest and
//! invalidate cached entries when an included file changes (or a
//! previously-missing include is created). Without a recorder the plugin
//! behaves exactly as before.
//!
//! # Wave 6 (#581)

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use markdown::mdast::Node as MdastNode;
use zfb_md_ast::{
    constructs_for_pipeline, unwrap_nested_links, BuildContext, CjkAutolinkBoundaryPlugin,
    MdastVisitor, ReadOutcome, ReadRecorder, ResolvedGfmConstructs,
};

use crate::TranscludeConfig;

// ── Public API ──────────────────────────────────────────────────────────────

/// Mdast visitor that replaces `:::include{file="…"}` directive paragraphs
/// with the parsed mdast nodes from the referenced file.
///
/// Requires a `BuildContext` with `source_path` (or falls back gracefully
/// with a diagnostic when absent). Override `visit_with_context` for the
/// context-aware path; the base `visit` is a no-op (cannot resolve without
/// paths).
///
/// Wire via `features.transclude: {}` in `zfb.config.ts`.
#[derive(Debug)]
pub struct TranscludePlugin {
    config: TranscludeConfig,
    /// Optional read-recorder (zfb#944): when present, every attempted
    /// include read — including failures — is reported so the MDX
    /// compile cache can validate a dependency manifest before serving
    /// a cached entry. `None` keeps the pre-#944 behaviour exactly.
    recorder: Option<Arc<ReadRecorder>>,
    /// GFM constructs an included file is parsed with (zfb#2390).
    ///
    /// Defaults to [`ResolvedGfmConstructs::ALL_OFF`], reproducing the
    /// bare `ParseOptions::mdx()` this site used before #2390 — so a
    /// plugin built by [`TranscludePlugin::new`] alone stays
    /// byte-identical. `zfb-content`'s feature wiring calls
    /// [`TranscludePlugin::with_gfm`] with the pipeline's own resolved
    /// set, which is what makes a transcluded file honour the project's
    /// `markdown.gfm` config instead of silently parsing with every GFM
    /// construct off.
    ///
    /// This is the HTML/collection-walker construct set
    /// ([`constructs_for_pipeline`]) — math stays off, as it was before
    /// #2390. The same plugin instance serves both `Pipeline::run` and
    /// the JSX-emit path, so it cannot be path-aware, and enabling math
    /// here would change HTML-path output (that path renders `Math` into
    /// a real `<pre><code class="language-math …">` element, it does not
    /// pass it through). Consequence, unchanged by #2390 and NOT
    /// cosmetic: on the JSX-emit path LaTeX in an included file leaks
    /// out as bare `{…}` expression containers that esbuild rejects,
    /// falling the whole page back to `<pre data-zfb-content-fallback>`.
    /// Tracked separately; see the #2390 changelog entry.
    gfm: ResolvedGfmConstructs,
    /// The project's `markdown.cjkFriendly` setting (zfb#1105).
    ///
    /// Stored raw rather than pre-ANDed with `gfm.autolink_literal`:
    /// the two travel together into [`TranscludePlugin::with_gfm`], and
    /// combining them here — at the single point that consumes them —
    /// makes the inconsistent state (boundary fix demanded while
    /// autolinking is off) unrepresentable rather than merely
    /// documented.
    ///
    /// It has to travel at all because `CjkAutolinkBoundaryPlugin` is a
    /// visitor at mdast-chain index 0 while transcluded nodes are
    /// spliced in later, so the included subtree is never reached by
    /// the pipeline's own copy of the pass.
    cjk_friendly: bool,
}

impl TranscludePlugin {
    /// Create a new plugin from the given config.
    ///
    /// Parses included files with every GFM construct OFF. Use
    /// [`TranscludePlugin::with_gfm`] to inherit the project's resolved
    /// `markdown.gfm` configuration.
    #[must_use]
    pub fn new(config: TranscludeConfig) -> Self {
        Self {
            config,
            recorder: None,
            gfm: ResolvedGfmConstructs::ALL_OFF,
            cjk_friendly: false,
        }
    }

    /// Parse included files with `gfm`, applying the post-parse
    /// normalisations those constructs make mandatory (zfb#2390).
    ///
    /// `cjk_friendly` is the project's `markdown.cjkFriendly` setting,
    /// passed raw — this is the one place it is ANDed with
    /// `gfm.autolink_literal`, mirroring the gate the pipeline uses when
    /// it wires `CjkAutolinkBoundaryPlugin` into its own chain.
    #[must_use]
    pub fn with_gfm(mut self, gfm: ResolvedGfmConstructs, cjk_friendly: bool) -> Self {
        self.gfm = gfm;
        self.cjk_friendly = cjk_friendly;
        self
    }

    /// Attach the read-recorder this plugin reports include reads
    /// through (zfb#944). The SAME `Arc` must also be set on the
    /// pipeline via `Pipeline::set_read_recorder` so the compile-cache
    /// choke point can scope the recording per compile.
    #[must_use]
    pub fn with_recorder(mut self, recorder: Arc<ReadRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }
}

impl MdastVisitor for TranscludePlugin {
    /// No-op when called without context — cannot resolve file paths.
    fn visit(&mut self, _node: &mut MdastNode) {}

    /// Context-aware path: resolves include directives using `ctx.source_path`
    /// and `ctx.project_root`.
    fn visit_with_context(&mut self, node: &mut MdastNode, ctx: &mut BuildContext<'_>) {
        let Some(source_path) = ctx.source_path.clone() else {
            // No source path — cannot resolve includes. Emit a warning if a
            // sink is wired, then return.
            if let Some(sink) = ctx.diagnostics.as_mut() {
                sink.emit(zfb_md_ast::diagnostics::MarkdownDiagnostic::warning(
                    "transclude: no source_path in BuildContext; includes cannot be resolved",
                ));
            }
            return;
        };

        let source_dir = match source_path.parent() {
            Some(d) => d.to_path_buf(),
            None => PathBuf::from("."),
        };
        let project_root = ctx.project_root.clone();

        let mut visited: HashSet<PathBuf> = HashSet::new();
        // Mark the current file as "in the stack" to detect direct cycles.
        if let Ok(canonical_self) = source_path.canonicalize() {
            visited.insert(canonical_self);
        }

        let env = ExpandEnv {
            project_root: &project_root,
            max_depth: self.config.max_depth,
            recorder: self.recorder.as_deref(),
            gfm: self.gfm,
            cjk_friendly: self.cjk_friendly,
        };
        expand_includes_in_node(node, &source_dir, &env, &mut visited, 0, ctx);
    }
}

// ── Recursive expansion ─────────────────────────────────────────────────────

/// Knobs that stay constant across the whole recursive include walk of
/// one visit (per-level state — `source_dir`, `visited`, `depth` —
/// travels as separate arguments).
struct ExpandEnv<'a> {
    project_root: &'a Path,
    max_depth: u8,
    /// Read-recorder for the compile-cache dependency manifest
    /// (zfb#944); `None` when the plugin was built without one.
    recorder: Option<&'a ReadRecorder>,
    /// GFM constructs each included file is parsed with, and the
    /// project's `cjkFriendly` setting (zfb#2390) — see the matching
    /// fields on [`TranscludePlugin`].
    gfm: ResolvedGfmConstructs,
    cjk_friendly: bool,
}

/// Apply the post-parse normalisations that [`ExpandEnv::gfm`] makes
/// mandatory to a freshly parsed included subtree (zfb#2390).
///
/// The pipeline runs both of these immediately after its own top-level
/// `markdown::to_mdast`, ahead of the visitor chain. This site parses
/// AFTER that point, so nothing else will ever normalise what it
/// produces — every `to_mdast` call site owns its own output.
///
/// - `unwrap_nested_links` (zfb#2388) removes autolink literals that
///   markdown-rs fires inside a link label, which would otherwise emit an
///   `<a>` nested in an `<a>` — invalid HTML that fails `html-validate`'s
///   `element-permitted-content`.
/// - [`CjkAutolinkBoundaryPlugin`] (zfb#1105) stops a bare URL flush
///   against CJK text from swallowing the trailing CJK run into the
///   `href`.
///
/// Both are gated on `autolink_literal`, the construct that produces the
/// defects they remove, so an included file parsed without it keeps
/// byte-identical output.
fn normalize_included_subtree(node: &mut MdastNode, env: &ExpandEnv<'_>) {
    if !env.gfm.autolink_literal {
        return;
    }
    unwrap_nested_links(node);
    if env.cjk_friendly {
        CjkAutolinkBoundaryPlugin::new().visit(node);
    }
}

/// Recursively scan `node`'s children for `:::include` paragraphs and
/// replace each one with the parsed nodes from the referenced file.
///
/// Dispatches to [`expand_includes_in_children`] on the appropriate child
/// vec for each container node type. Non-container nodes are skipped.
fn expand_includes_in_node(
    node: &mut MdastNode,
    source_dir: &Path,
    env: &ExpandEnv<'_>,
    visited: &mut HashSet<PathBuf>,
    depth: u8,
    ctx: &mut BuildContext<'_>,
) {
    let children = match node {
        MdastNode::Root(r) => &mut r.children,
        MdastNode::ListItem(li) => &mut li.children,
        MdastNode::Blockquote(bq) => &mut bq.children,
        MdastNode::MdxJsxFlowElement(j) => &mut j.children,
        _ => return,
    };
    expand_includes_in_children(children, source_dir, env, visited, depth, ctx);
}

/// Scan a `Vec<MdastNode>` (the children of a container) for `:::include`
/// paragraphs, replacing each with the spliced nodes from the referenced file.
///
/// Also recurses into nested container nodes so includes nested inside
/// blockquotes, list items, etc. are found.
///
/// `depth` is the current nesting level; `env.max_depth` is the configured
/// bound.
fn expand_includes_in_children(
    children: &mut Vec<MdastNode>,
    source_dir: &Path,
    env: &ExpandEnv<'_>,
    visited: &mut HashSet<PathBuf>,
    depth: u8,
    ctx: &mut BuildContext<'_>,
) {
    let mut i = 0;
    while i < children.len() {
        match extract_include_attrs(&children[i]) {
            IncludeMatch::Ok(attrs) => {
                // Found an `:::include` paragraph at `i`.
                let replacement = resolve_and_expand(&attrs, source_dir, env, visited, depth, ctx);

                // Remove the directive paragraph.
                children.remove(i);

                // Insert the replacement nodes at `i`.
                let insert_count = replacement.len();
                for (offset, new_node) in replacement.into_iter().enumerate() {
                    children.insert(i + offset, new_node);
                }
                i += insert_count;
            }
            IncludeMatch::Malformed => {
                // Shaped like `:::include{...}` (the `:::include` marker text
                // plus an attrs expression) but the attrs failed to parse —
                // e.g. a misspelled `fil="./x.md"` instead of `file="./x.md"`.
                // Previously this fell through as "not an include directive"
                // and was left untouched: the literal `:::include{...}` text
                // leaked into rendered HTML, or blew up as an opaque JS
                // syntax error on the JSX-emit path (the MdxTextExpression's
                // raw attr string is not valid JS on its own). Emit a clear
                // diagnostic and drop the node instead (#1392).
                emit_error(
                    ctx,
                    "transclude: missing required file attribute".to_string(),
                );
                children.remove(i);
            }
            IncludeMatch::NotInclude => {
                // Not an include directive — recurse into container children.
                expand_includes_in_node(&mut children[i], source_dir, env, visited, depth, ctx);
                i += 1;
            }
        }
    }
}

/// Resolve the file and return the replacement nodes.
///
/// On error (path escape, cycle, read failure, depth exceeded), emits a
/// diagnostic and returns an empty vec (the directive paragraph is removed,
/// leaving nothing in its place).
///
/// Read recording (zfb#944): every branch that observes on-disk state
/// reports it through `env.recorder` — the successful read, the
/// missing/unreadable include (so creating it later invalidates the
/// cached entry), and the existing-but-rejected include (so deleting it
/// later invalidates the rejection diagnostic). Branches that never
/// touch the filesystem (depth bound, absolute-path reject, cycle
/// detection on an already-visited — and therefore already-recorded —
/// path) record nothing.
fn resolve_and_expand(
    attrs: &IncludeAttrs,
    source_dir: &Path,
    env: &ExpandEnv<'_>,
    visited: &mut HashSet<PathBuf>,
    depth: u8,
    ctx: &mut BuildContext<'_>,
) -> Vec<MdastNode> {
    let max_depth = env.max_depth;
    // ── Depth check ────────────────────────────────────────────────────────
    if depth >= max_depth {
        emit_error(
            ctx,
            format!(
                "transclude: max depth ({max_depth}) exceeded while including \"{}\"; chain is too deep",
                attrs.file
            ),
        );
        return Vec::new();
    }

    // ── Reject absolute paths ───────────────────────────────────────────────
    let raw_path = Path::new(&attrs.file);
    if raw_path.is_absolute() {
        emit_error(
            ctx,
            format!(
                "transclude: absolute file paths are not allowed; got \"{}\"",
                attrs.file
            ),
        );
        return Vec::new();
    }

    // ── Resolve relative to source dir ──────────────────────────────────────
    let resolved = source_dir.join(raw_path);

    // ── Canonicalize + project-root check ─────────────────────────────────
    let canonical = match resolved.canonicalize() {
        Ok(c) => c,
        Err(e) => {
            // Record the failed observation (zfb#944): NotFound → Missing,
            // which is what makes a cached entry invalidate the moment a
            // previously-missing include is created. Recorded under the
            // resolved (pre-canonicalization) spelling — the file does not
            // exist, so no canonical spelling is available, and re-probing
            // the resolved path observes the same state.
            if let Some(r) = env.recorder {
                r.record_outcome(&resolved, ReadOutcome::of_io_error(&e));
            }
            emit_error(
                ctx,
                format!("transclude: cannot read \"{}\": {e}", resolved.display()),
            );
            return Vec::new();
        }
    };

    let canonical_root = match env.project_root.canonicalize() {
        Ok(r) => r,
        Err(e) => {
            emit_error(
                ctx,
                format!(
                    "transclude: cannot canonicalize project root \"{}\": {e}",
                    env.project_root.display()
                ),
            );
            return Vec::new();
        }
    };
    if !canonical.starts_with(&canonical_root) {
        // The escape diagnostic depends on the target's on-disk state
        // (deleting it changes the outcome to "cannot read"), so the
        // observation is recorded even though the content is never
        // spliced (zfb#944).
        if let Some(r) = env.recorder {
            let _ = r.record_file(&canonical);
        }
        emit_error(
            ctx,
            format!(
                "transclude: \"{}\" escapes project root \"{}\"",
                canonical.display(),
                canonical_root.display()
            ),
        );
        return Vec::new();
    }

    // ── Cycle detection ────────────────────────────────────────────────────
    if visited.contains(&canonical) {
        emit_error(
            ctx,
            format!(
                "transclude: cycle detected — \"{}\" is already in the include stack",
                canonical.display()
            ),
        );
        return Vec::new();
    }

    // ── Read file ─────────────────────────────────────────────────────────
    let raw_content = match std::fs::read_to_string(&canonical) {
        Ok(c) => {
            // The caller holds the COMPLETE contents (read_to_string
            // preserves the exact bytes of valid-UTF-8 files), so record
            // the hash without a second filesystem read (zfb#944).
            if let Some(r) = env.recorder {
                r.record_bytes(&canonical, c.as_bytes());
            }
            c
        }
        Err(e) => {
            // NotFound → Missing (re-validatable); anything else — including
            // invalid UTF-8 — → Error (terminal: never cached). zfb#944.
            if let Some(r) = env.recorder {
                r.record_outcome(&canonical, ReadOutcome::of_io_error(&e));
            }
            emit_error(
                ctx,
                format!("transclude: cannot read \"{}\": {e}", canonical.display()),
            );
            return Vec::new();
        }
    };

    // ── Apply line-range slice ────────────────────────────────────────────
    let content = if let Some(ref lines_spec) = attrs.lines {
        match parse_line_range(lines_spec) {
            Some((start, end)) => slice_lines(&raw_content, start, end),
            None => {
                emit_error(
                    ctx,
                    format!(
                        "transclude: invalid lines attribute \"{lines_spec}\"; expected \"N-M\" (e.g. \"10-30\")"
                    ),
                );
                return Vec::new();
            }
        }
    } else {
        raw_content.clone()
    };

    // ── code=true wrap ───────────────────────────────────────────────────
    if attrs.code {
        let lang = attrs.lang.clone().unwrap_or_default();
        let code_node = MdastNode::Code(markdown::mdast::Code {
            lang: if lang.is_empty() { None } else { Some(lang) },
            meta: None,
            value: content,
            position: None,
        });
        return vec![code_node];
    }

    // ── Parse as mdast ────────────────────────────────────────────────────
    // Constructs come from the host project's resolved `markdown.gfm`
    // (zfb#2390) — a bare `ParseOptions::mdx()` here inherits
    // `Constructs::default()`, where every `gfm_*` flag is false, so the
    // same markdown used to render differently inline vs transcluded.
    let opts = markdown::ParseOptions {
        constructs: constructs_for_pipeline(env.gfm),
        ..markdown::ParseOptions::mdx()
    };
    let mut included_mdast = match markdown::to_mdast(&content, &opts) {
        Ok(tree) => tree,
        Err(e) => {
            emit_error(
                ctx,
                format!(
                    "transclude: failed to parse \"{}\": {e}",
                    canonical.display()
                ),
            );
            return Vec::new();
        }
    };

    normalize_included_subtree(&mut included_mdast, env);

    let MdastNode::Root(root) = included_mdast else {
        return Vec::new();
    };

    let mut nodes = root.children;

    // ── Recurse into the included file's nodes ──────────────────────────
    // Use `expand_includes_in_children` (not `expand_includes_in_node`) so
    // top-level `:::include` directives in the included file are found
    // directly in the root's children vec rather than being missed by the
    // container-dispatch in `expand_includes_in_node` (which skips Paragraph
    // nodes — the `:::include` pattern appears as a top-level Paragraph).
    let included_source_dir = canonical
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    visited.insert(canonical.clone());

    expand_includes_in_children(
        &mut nodes,
        &included_source_dir,
        env,
        visited,
        depth + 1,
        ctx,
    );

    visited.remove(&canonical);

    nodes
}

// ── Attribute extraction ────────────────────────────────────────────────────

/// Parsed attributes from a `:::include{...}` directive.
#[derive(Debug, Clone)]
struct IncludeAttrs {
    /// Required: relative path to the file to include.
    file: String,
    /// Optional line range `"N-M"` (1-indexed, inclusive).
    lines: Option<String>,
    /// When true, wrap the file's content in a code block rather than parsing.
    code: bool,
    /// Language identifier for the code block (only relevant when `code: true`).
    lang: Option<String>,
}

/// Outcome of matching a node against the `:::include{...}` shape.
///
/// Distinguishes "not this directive at all" (the caller should keep
/// recursing into the node as an ordinary container) from "this IS an
/// `:::include` paragraph but its attrs failed to parse" (the caller should
/// emit a diagnostic and drop the node rather than leaving the raw
/// `:::include{...}` text/expression in the tree — see #1392).
#[derive(Debug)]
enum IncludeMatch {
    /// Not shaped like `:::include` — recurse as normal.
    NotInclude,
    /// Shaped like `:::include{...}` but the attrs expression failed to
    /// parse (e.g. missing/misspelled `file`).
    Malformed,
    /// Successfully parsed.
    Ok(IncludeAttrs),
}

/// Check if `node` is an `:::include` paragraph and extract its attributes.
///
/// Returns [`IncludeMatch::Ok`] iff the node is a `Paragraph` with exactly
/// two children — `Text(":::include")` + `MdxTextExpression(attrs_str)` —
/// AND the attrs string parses with a required `file` attribute.
/// [`IncludeMatch::Malformed`] when the paragraph shape matches but the
/// attrs don't (see #1392); [`IncludeMatch::NotInclude`] otherwise.
fn extract_include_attrs(node: &MdastNode) -> IncludeMatch {
    let MdastNode::Paragraph(p) = node else {
        return IncludeMatch::NotInclude;
    };
    if p.children.len() != 2 {
        return IncludeMatch::NotInclude;
    }
    let MdastNode::Text(t) = &p.children[0] else {
        return IncludeMatch::NotInclude;
    };
    if first_line(&t.value).trim() != ":::include" {
        return IncludeMatch::NotInclude;
    }
    let MdastNode::MdxTextExpression(expr) = &p.children[1] else {
        return IncludeMatch::NotInclude;
    };

    match parse_attrs_from_expression(&expr.value) {
        Some(attrs) => IncludeMatch::Ok(attrs),
        None => IncludeMatch::Malformed,
    }
}

/// Parse key="value" / key=value / key=true/false attributes from an
/// `MdxTextExpression` value string.
///
/// The MDX parser stores the raw attr string without the surrounding `{}`,
/// e.g. `file="./snippet.md" lines="10-30" code=true lang="rust"`.
fn parse_attrs_from_expression(expr_value: &str) -> Option<IncludeAttrs> {
    let attrs = parse_attr_string(expr_value);

    let file = attrs.get("file").cloned()?;
    if file.is_empty() {
        return None;
    }

    let lines = attrs.get("lines").cloned();
    let code = attrs.get("code").map(|v| v == "true").unwrap_or(false);
    let lang = attrs.get("lang").cloned().filter(|l| !l.is_empty());

    Some(IncludeAttrs {
        file,
        lines,
        code,
        lang,
    })
}

/// Parse a key="value" / key=value / key=true / key=false string into a map.
///
/// Handles:
/// - Quoted values: `key="val"` or `key='val'`
/// - Unquoted values: `key=value` (no whitespace)
/// - Boolean shorthands: `key=true`, `key=false`
/// - Keys without values (treated as boolean true): `key`
fn parse_attr_string(s: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut rest = s.trim();

    while !rest.is_empty() {
        // Skip leading whitespace
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }

        // Read key
        let key_end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let key = rest[..key_end].to_string();
        rest = &rest[key_end..];

        if key.is_empty() {
            // Skip malformed token
            rest = &rest[1.min(rest.len())..];
            continue;
        }

        if rest.starts_with('=') {
            rest = &rest[1..]; // consume '='
                               // Read value
            if rest.starts_with('"') {
                // Quoted with "
                rest = &rest[1..];
                let end = rest.find('"').unwrap_or(rest.len());
                let value = rest[..end].to_string();
                rest = if end < rest.len() {
                    &rest[end + 1..]
                } else {
                    &rest[end..]
                };
                map.insert(key, value);
            } else if rest.starts_with('\'') {
                // Quoted with '
                rest = &rest[1..];
                let end = rest.find('\'').unwrap_or(rest.len());
                let value = rest[..end].to_string();
                rest = if end < rest.len() {
                    &rest[end + 1..]
                } else {
                    &rest[end..]
                };
                map.insert(key, value);
            } else {
                // Unquoted value (stops at whitespace)
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                let value = rest[..end].to_string();
                rest = &rest[end..];
                map.insert(key, value);
            }
        } else {
            // No '=' — boolean shorthand (key without value = true)
            map.insert(key, "true".to_string());
        }
    }

    map
}

// ── Line-range helpers ──────────────────────────────────────────────────────

/// Parse `"N-M"` into `(start, end)` (1-indexed, inclusive).
/// Returns `None` if the format is invalid.
fn parse_line_range(spec: &str) -> Option<(usize, usize)> {
    let (start_str, end_str) = spec.split_once('-')?;
    let start: usize = start_str.trim().parse().ok()?;
    let end: usize = end_str.trim().parse().ok()?;
    if start == 0 || end == 0 || start > end {
        return None;
    }
    Some((start, end))
}

/// Slice `content` to lines `start..=end` (1-indexed, inclusive).
///
/// Lines outside the range are silently dropped. If `end` exceeds the
/// actual line count, only available lines are returned.
fn slice_lines(content: &str, start: usize, end: usize) -> String {
    content
        .lines()
        .enumerate()
        .filter(|(i, _)| {
            let line_num = i + 1; // 1-indexed
            line_num >= start && line_num <= end
        })
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Diagnostic helper ───────────────────────────────────────────────────────

fn emit_error(ctx: &mut BuildContext<'_>, message: String) {
    if let Some(sink) = ctx.diagnostics.as_mut() {
        sink.emit(zfb_md_ast::diagnostics::MarkdownDiagnostic::error(message));
    }
}

// ── Node predicate helpers ─────────────────────────────────────────────────

fn first_line(s: &str) -> &str {
    match s.find('\n') {
        Some(i) => &s[..i],
        None => s,
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use markdown::mdast::{Paragraph, Text};

    // ── parse_attr_string ──────────────────────────────────────────────────

    #[test]
    fn parse_attrs_quoted_double() {
        let m = parse_attr_string(r#"file="./foo.md""#);
        assert_eq!(m.get("file").map(String::as_str), Some("./foo.md"));
    }

    #[test]
    fn parse_attrs_multiple_attrs() {
        let m = parse_attr_string(r#"file="./foo.rs" code=true lang="rust""#);
        assert_eq!(m.get("file").map(String::as_str), Some("./foo.rs"));
        assert_eq!(m.get("code").map(String::as_str), Some("true"));
        assert_eq!(m.get("lang").map(String::as_str), Some("rust"));
    }

    #[test]
    fn parse_attrs_lines_attr() {
        let m = parse_attr_string(r#"file="./foo.md" lines="10-30""#);
        assert_eq!(m.get("lines").map(String::as_str), Some("10-30"));
    }

    #[test]
    fn parse_attrs_code_without_lang() {
        let m = parse_attr_string(r#"file="./foo.txt" code=true"#);
        assert_eq!(m.get("code").map(String::as_str), Some("true"));
        assert!(!m.contains_key("lang"));
    }

    // ── parse_line_range ──────────────────────────────────────────────────

    #[test]
    fn line_range_valid() {
        assert_eq!(parse_line_range("10-30"), Some((10, 30)));
    }

    #[test]
    fn line_range_single_line() {
        assert_eq!(parse_line_range("5-5"), Some((5, 5)));
    }

    #[test]
    fn line_range_invalid_reversed() {
        assert!(parse_line_range("30-10").is_none());
    }

    #[test]
    fn line_range_invalid_format() {
        assert!(parse_line_range("abc").is_none());
        assert!(parse_line_range("").is_none());
    }

    #[test]
    fn line_range_zero_start_rejected() {
        assert!(parse_line_range("0-5").is_none());
    }

    // ── slice_lines ───────────────────────────────────────────────────────

    #[test]
    fn slice_lines_basic() {
        let content = "line1\nline2\nline3\nline4\nline5";
        assert_eq!(slice_lines(content, 2, 4), "line2\nline3\nline4");
    }

    #[test]
    fn slice_lines_single() {
        let content = "a\nb\nc";
        assert_eq!(slice_lines(content, 2, 2), "b");
    }

    #[test]
    fn slice_lines_beyond_end() {
        let content = "a\nb";
        // end=10 exceeds line count — only available lines returned
        assert_eq!(slice_lines(content, 1, 10), "a\nb");
    }

    #[test]
    fn slice_lines_from_start() {
        let content = "a\nb\nc\nd";
        assert_eq!(slice_lines(content, 1, 2), "a\nb");
    }

    // ── extract_include_attrs ─────────────────────────────────────────────

    fn make_include_paragraph(expr_value: &str) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children: vec![
                MdastNode::Text(Text {
                    value: ":::include".to_string(),
                    position: None,
                }),
                MdastNode::MdxTextExpression(markdown::mdast::MdxTextExpression {
                    value: expr_value.to_string(),
                    position: None,
                    stops: Vec::new(),
                }),
            ],
            position: None,
        })
    }

    /// Unwrap an [`IncludeMatch::Ok`], panicking with the actual variant
    /// otherwise — keeps the extraction tests below readable.
    fn expect_ok(m: IncludeMatch) -> IncludeAttrs {
        match m {
            IncludeMatch::Ok(attrs) => attrs,
            other => panic!("expected IncludeMatch::Ok, got: {other:?}"),
        }
    }

    #[test]
    fn extract_basic_include() {
        let node = make_include_paragraph(r#"file="./snippet.md""#);
        let attrs = expect_ok(extract_include_attrs(&node));
        assert_eq!(attrs.file, "./snippet.md");
        assert!(!attrs.code);
        assert!(attrs.lines.is_none());
        assert!(attrs.lang.is_none());
    }

    #[test]
    fn extract_include_with_lines() {
        let node = make_include_paragraph(r#"file="./foo.md" lines="10-30""#);
        let attrs = expect_ok(extract_include_attrs(&node));
        assert_eq!(attrs.file, "./foo.md");
        assert_eq!(attrs.lines.as_deref(), Some("10-30"));
    }

    #[test]
    fn extract_include_with_code_and_lang() {
        let node = make_include_paragraph(r#"file="./foo.rs" code=true lang="rust""#);
        let attrs = expect_ok(extract_include_attrs(&node));
        assert_eq!(attrs.file, "./foo.rs");
        assert!(attrs.code);
        assert_eq!(attrs.lang.as_deref(), Some("rust"));
    }

    // #1392: `:::include{fil="./x.md"}` (misspelled `file` attr) is shaped
    // exactly like a real include directive — the paragraph has the
    // `:::include` marker text plus an MdxTextExpression — but the attrs
    // parse to no `file` key. This must be distinguished from "not an
    // include directive at all" so the caller can emit a diagnostic instead
    // of silently leaving broken syntax in the tree.
    #[test]
    fn extract_missing_file_attr_is_malformed() {
        let node = make_include_paragraph(r#"fil="./x.md""#);
        match extract_include_attrs(&node) {
            IncludeMatch::Malformed => {}
            other => panic!("expected IncludeMatch::Malformed, got: {other:?}"),
        }
    }

    #[test]
    fn non_include_paragraph_returns_none() {
        let node = MdastNode::Paragraph(Paragraph {
            children: vec![MdastNode::Text(Text {
                value: "just a paragraph".to_string(),
                position: None,
            })],
            position: None,
        });
        assert!(matches!(
            extract_include_attrs(&node),
            IncludeMatch::NotInclude
        ));
    }

    #[test]
    fn non_include_directive_returns_none() {
        // `:::note` is not an include directive
        let node = make_include_paragraph_with_text(":::note", r#"file="./foo.md""#);
        assert!(matches!(
            extract_include_attrs(&node),
            IncludeMatch::NotInclude
        ));
    }

    fn make_include_paragraph_with_text(text: &str, expr_value: &str) -> MdastNode {
        MdastNode::Paragraph(Paragraph {
            children: vec![
                MdastNode::Text(Text {
                    value: text.to_string(),
                    position: None,
                }),
                MdastNode::MdxTextExpression(markdown::mdast::MdxTextExpression {
                    value: expr_value.to_string(),
                    position: None,
                    stops: Vec::new(),
                }),
            ],
            position: None,
        })
    }

    // ── Integration: file resolution (using tempdir) ──────────────────────

    fn run_transclude(
        input_md: &str,
        source_path: PathBuf,
        project_root: PathBuf,
        config: TranscludeConfig,
    ) -> (MdastNode, Vec<zfb_md_ast::diagnostics::MarkdownDiagnostic>) {
        let opts = markdown::ParseOptions::mdx();
        let mut tree = markdown::to_mdast(input_md, &opts).expect("parse ok");
        let mut sink = zfb_md_ast::diagnostics::CollectingSink::new();
        let mut ctx = BuildContext {
            source_path: Some(source_path),
            project_root,
            public_dir: PathBuf::from("/tmp"),
            heading_registry: None,
            diagnostics: Some(&mut sink),
            cross_file_links: None,
        };
        let mut plugin = TranscludePlugin::new(config);
        plugin.visit_with_context(&mut tree, &mut ctx);
        let diags = sink.take();
        (tree, diags)
    }

    #[test]
    fn simple_include_inlines_content() {
        let dir = tempfile::Builder::new()
            .prefix("transclude_test")
            .tempdir()
            .unwrap();
        let snippet_path = dir.path().join("snippet.md");
        std::fs::write(&snippet_path, "# Hello\n\nWorld.\n").unwrap();

        let input = r#":::include{file="./snippet.md"}"#;
        let source = dir.path().join("input.md");
        std::fs::write(&source, input).unwrap();

        let (tree, diags) = run_transclude(
            input,
            source,
            dir.path().to_path_buf(),
            TranscludeConfig::default(),
        );
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");

        let MdastNode::Root(root) = &tree else {
            panic!("expected Root")
        };
        // snippet.md has h1 + paragraph → 2 nodes spliced in
        assert_eq!(
            root.children.len(),
            2,
            "expected 2 spliced nodes, got: {root:?}"
        );
        assert!(matches!(root.children[0], MdastNode::Heading(_)));
        assert!(matches!(root.children[1], MdastNode::Paragraph(_)));
    }

    // #1392: `:::include{fil="./x.md"}` (misspelled `file`) used to fall
    // through as "not an include directive" — the literal `:::include{...}`
    // text/expression stayed in the tree, which either leaked onto the HTML
    // page or blew up as an opaque JS syntax error on the JSX-emit path.
    // Now it must emit a clear diagnostic and drop the node.
    #[test]
    fn malformed_attrs_emit_diagnostic_and_drop_node() {
        let dir = tempfile::Builder::new()
            .prefix("transclude_test_malformed")
            .tempdir()
            .unwrap();

        let input = r#":::include{fil="./x.md"}"#;
        let source = dir.path().join("input.md");
        std::fs::write(&source, input).unwrap();

        let (tree, diags) = run_transclude(
            input,
            source,
            dir.path().to_path_buf(),
            TranscludeConfig::default(),
        );

        assert_eq!(diags.len(), 1, "expected exactly one diagnostic: {diags:?}");
        match &diags[0] {
            zfb_md_ast::diagnostics::MarkdownDiagnostic::Generic {
                severity, message, ..
            } => {
                assert_eq!(
                    *severity,
                    zfb_md_ast::diagnostics::DiagnosticSeverity::Error
                );
                assert!(
                    message.contains("missing required file attribute"),
                    "got: {message}"
                );
            }
            other => panic!("expected a Generic diagnostic, got: {other:?}"),
        }

        // The malformed directive paragraph must be dropped, not left in
        // the tree as literal `:::include{...}` text.
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root")
        };
        assert!(
            root.children.is_empty(),
            "malformed include must be dropped, got: {root:?}"
        );
    }

    #[test]
    fn include_with_code_true_wraps_in_code_block() {
        let dir = tempfile::Builder::new()
            .prefix("transclude_code")
            .tempdir()
            .unwrap();
        let snippet_path = dir.path().join("foo.rs");
        std::fs::write(&snippet_path, "fn main() {}\n").unwrap();

        let input = r#":::include{file="./foo.rs" code=true lang="rust"}"#;
        let source = dir.path().join("input.md");
        std::fs::write(&source, input).unwrap();

        let (tree, diags) = run_transclude(
            input,
            source,
            dir.path().to_path_buf(),
            TranscludeConfig::default(),
        );
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root")
        };
        assert_eq!(root.children.len(), 1);
        let MdastNode::Code(code) = &root.children[0] else {
            panic!("expected Code, got: {:?}", root.children[0])
        };
        assert_eq!(code.lang.as_deref(), Some("rust"));
        assert!(code.value.contains("fn main()"));
    }

    #[test]
    fn include_with_lines_slices_correctly() {
        let dir = tempfile::Builder::new()
            .prefix("transclude_lines")
            .tempdir()
            .unwrap();
        let snippet_path = dir.path().join("nums.md");
        std::fs::write(&snippet_path, "line1\nline2\nline3\nline4\nline5\n").unwrap();

        let input = r#":::include{file="./nums.md" lines="2-4" code=true}"#;
        let source = dir.path().join("input.md");
        std::fs::write(&source, input).unwrap();

        let (tree, diags) = run_transclude(
            input,
            source,
            dir.path().to_path_buf(),
            TranscludeConfig::default(),
        );
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root")
        };
        let MdastNode::Code(code) = &root.children[0] else {
            panic!("expected Code")
        };
        assert_eq!(code.value, "line2\nline3\nline4");
    }

    #[test]
    fn cycle_detection_emits_error() {
        // A→A direct self-reference.
        let dir = tempfile::Builder::new()
            .prefix("transclude_cycle")
            .tempdir()
            .unwrap();
        let input = r#":::include{file="./cycle.md"}"#;
        // cycle.md includes itself
        let cycle_content = r#":::include{file="./cycle.md"}"#;
        std::fs::write(dir.path().join("cycle.md"), cycle_content).unwrap();

        let source = dir.path().join("cycle.md");
        let (_, diags) = run_transclude(
            input,
            source.clone(),
            dir.path().to_path_buf(),
            TranscludeConfig::default(),
        );
        let has_cycle_error = diags.iter().any(|d| {
            matches!(d, zfb_md_ast::diagnostics::MarkdownDiagnostic::Generic {
                severity: zfb_md_ast::diagnostics::DiagnosticSeverity::Error,
                message,
                ..
            } if message.contains("cycle"))
        });
        assert!(
            has_cycle_error,
            "expected cycle error diagnostic: {diags:?}"
        );
    }

    #[test]
    fn max_depth_exceeded_emits_error() {
        // With maxDepth=1 a chain A→B→C is rejected at B→C level.
        let dir = tempfile::Builder::new()
            .prefix("transclude_depth")
            .tempdir()
            .unwrap();
        // c.md is a leaf (no includes)
        std::fs::write(dir.path().join("c.md"), "leaf\n").unwrap();
        // b.md includes c.md (depth 1 — allowed)
        std::fs::write(dir.path().join("b.md"), r#":::include{file="./c.md"}"#).unwrap();
        // a.md includes b.md (depth 0 — allowed), which includes c.md (depth 1 — allowed)
        let input = r#":::include{file="./b.md"}"#;
        let source = dir.path().join("a.md");
        std::fs::write(&source, input).unwrap();

        // maxDepth=1: b.md can include nothing (depth 1 >= maxDepth 1)
        let config = TranscludeConfig { max_depth: 1 };
        let (_, diags) = run_transclude(input, source, dir.path().to_path_buf(), config);
        let has_depth_error = diags.iter().any(|d| {
            matches!(d, zfb_md_ast::diagnostics::MarkdownDiagnostic::Generic {
                severity: zfb_md_ast::diagnostics::DiagnosticSeverity::Error,
                message,
                ..
            } if message.contains("max depth"))
        });
        assert!(has_depth_error, "expected depth error: {diags:?}");
    }

    #[test]
    fn absolute_path_rejected() {
        let dir = tempfile::Builder::new()
            .prefix("transclude_abs")
            .tempdir()
            .unwrap();
        let input = r#":::include{file="/etc/passwd"}"#;
        let source = dir.path().join("input.md");
        std::fs::write(&source, input).unwrap();

        let (_, diags) = run_transclude(
            input,
            source,
            dir.path().to_path_buf(),
            TranscludeConfig::default(),
        );
        let has_err = diags.iter().any(|d| {
            matches!(d, zfb_md_ast::diagnostics::MarkdownDiagnostic::Generic {
                severity: zfb_md_ast::diagnostics::DiagnosticSeverity::Error,
                message,
                ..
            } if message.contains("absolute"))
        });
        assert!(has_err, "expected absolute path error: {diags:?}");
    }

    #[test]
    fn path_outside_project_root_rejected() {
        let outer = tempfile::Builder::new()
            .prefix("transclude_outer")
            .tempdir()
            .unwrap();
        let inner = tempfile::Builder::new()
            .prefix("transclude_inner")
            .tempdir()
            .unwrap();
        // outer/secret.md exists but project_root is inner/
        std::fs::write(outer.path().join("secret.md"), "secret\n").unwrap();

        let input = format!(
            r#":::include{{file="{}"}}"#,
            outer.path().join("secret.md").display()
        );
        let source = inner.path().join("input.md");
        std::fs::write(&source, &input).unwrap();

        // project_root = inner dir, but file resolves to outer dir (absolute → rejected first)
        let (_, diags) = run_transclude(
            &input,
            source,
            inner.path().to_path_buf(),
            TranscludeConfig::default(),
        );
        let has_err = diags.iter().any(|d| {
            matches!(
                d,
                zfb_md_ast::diagnostics::MarkdownDiagnostic::Generic {
                    severity: zfb_md_ast::diagnostics::DiagnosticSeverity::Error,
                    ..
                }
            )
        });
        assert!(has_err, "expected path error: {diags:?}");
    }

    #[test]
    fn two_level_chain_inlines_both() {
        // A → B → (nothing), verify both levels resolve.
        let dir = tempfile::Builder::new()
            .prefix("transclude_chain")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("b.md"), "From B.\n").unwrap();
        std::fs::write(dir.path().join("a.md"), r#":::include{file="./b.md"}"#).unwrap();

        let input = r#":::include{file="./a.md"}"#;
        let source = dir.path().join("root.md");
        std::fs::write(&source, input).unwrap();

        let (tree, diags) = run_transclude(
            input,
            source,
            dir.path().to_path_buf(),
            TranscludeConfig::default(),
        );
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        let MdastNode::Root(root) = &tree else {
            panic!("expected Root")
        };
        // a.md inlines b.md which is "From B." → a paragraph
        assert!(!root.children.is_empty(), "expected inlined content");
    }

    // ── Read recording (zfb#944) ──────────────────────────────────────────

    /// Like [`run_transclude`] but with a recorder attached; returns the
    /// recorded reads alongside the tree and diagnostics.
    fn run_transclude_recorded(
        input_md: &str,
        source_path: PathBuf,
        project_root: PathBuf,
    ) -> (
        MdastNode,
        Vec<zfb_md_ast::diagnostics::MarkdownDiagnostic>,
        std::collections::BTreeMap<PathBuf, ReadOutcome>,
    ) {
        let recorder = Arc::new(ReadRecorder::new());
        let opts = markdown::ParseOptions::mdx();
        let mut tree = markdown::to_mdast(input_md, &opts).expect("parse ok");
        let mut sink = zfb_md_ast::diagnostics::CollectingSink::new();
        let mut ctx = BuildContext {
            source_path: Some(source_path),
            project_root,
            public_dir: PathBuf::from("/tmp"),
            heading_registry: None,
            diagnostics: Some(&mut sink),
            cross_file_links: None,
        };
        let mut plugin =
            TranscludePlugin::new(TranscludeConfig::default()).with_recorder(Arc::clone(&recorder));
        plugin.visit_with_context(&mut tree, &mut ctx);
        (tree, sink.take(), recorder.take_reads())
    }

    #[test]
    fn successful_include_records_full_content_hash() {
        let dir = tempfile::Builder::new()
            .prefix("transclude_rec_ok")
            .tempdir()
            .unwrap();
        let snippet = dir.path().join("snippet.md");
        std::fs::write(&snippet, "shared text\n").unwrap();
        let source = dir.path().join("input.md");
        let input = r#":::include{file="./snippet.md"}"#;
        std::fs::write(&source, input).unwrap();

        let (_, diags, reads) = run_transclude_recorded(input, source, dir.path().to_path_buf());
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        let canonical = snippet.canonicalize().expect("canonicalize snippet");
        assert_eq!(
            reads.get(&canonical),
            Some(&ReadOutcome::of_bytes(b"shared text\n")),
            "the include read must record the FULL file contents hash: {reads:?}"
        );
    }

    #[test]
    fn missing_include_records_missing_outcome() {
        let dir = tempfile::Builder::new()
            .prefix("transclude_rec_missing")
            .tempdir()
            .unwrap();
        let source = dir.path().join("input.md");
        let input = r#":::include{file="./not-yet.md"}"#;
        std::fs::write(&source, input).unwrap();

        let (_, diags, reads) = run_transclude_recorded(input, source, dir.path().to_path_buf());
        assert!(
            !diags.is_empty(),
            "a missing include must emit a diagnostic"
        );
        let resolved = dir.path().join("not-yet.md");
        assert_eq!(
            reads.get(&resolved),
            Some(&ReadOutcome::Missing),
            "the FAILED read must be recorded as Missing so creating the \
             file later invalidates a cached entry: {reads:?}"
        );
    }

    #[test]
    fn nested_chain_records_every_level() {
        let dir = tempfile::Builder::new()
            .prefix("transclude_rec_chain")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("c.md"), "leaf\n").unwrap();
        std::fs::write(dir.path().join("b.md"), r#":::include{file="./c.md"}"#).unwrap();
        let source = dir.path().join("a.md");
        let input = r#":::include{file="./b.md"}"#;
        std::fs::write(&source, input).unwrap();

        let (_, diags, reads) = run_transclude_recorded(input, source, dir.path().to_path_buf());
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        let b = dir.path().join("b.md").canonicalize().unwrap();
        let c = dir.path().join("c.md").canonicalize().unwrap();
        assert!(
            reads.contains_key(&b),
            "level-1 include must record: {reads:?}"
        );
        assert!(
            reads.contains_key(&c),
            "level-2 include must record: {reads:?}"
        );
    }

    #[test]
    fn no_recorder_means_no_recording_and_same_output() {
        // Plugins without a recorder behave exactly as before — pinned by
        // the unchanged legacy tests above; this just confirms the default
        // constructor carries no recorder.
        let plugin = TranscludePlugin::new(TranscludeConfig::default());
        assert!(plugin.recorder.is_none());
    }
}
