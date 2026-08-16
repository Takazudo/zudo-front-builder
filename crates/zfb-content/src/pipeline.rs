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

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use markdown::mdast::{AttributeContent, AttributeValue, Node as MdastNode};
use sha2::{Digest, Sha256};
use zfb_md_ast::diagnostics::{DiagnosticSeverity, MarkdownDiagnostic};
use zfb_md_ast::{CrossFileLinkCandidate, FileHeadings, HeadingIdStrategy, ReadRecorder};

use crate::dep_manifest::DependencyManifest;
use crate::footnotes::{
    FootnoteCursor, FootnoteEntry, FootnoteModel, FootnoteRef, FOOTNOTE_BACKREF_MARKER,
    FOOTNOTE_LABEL_ID, FOOTNOTE_LABEL_STYLE, FOOTNOTE_LABEL_TEXT, FOOTNOTE_SECTION_CLASS,
};
use crate::path_norm::normalize_path_lexically;
use crate::plugins::{
    BrokenLinkDiagnostic, CjkAutolinkBoundaryPlugin, CjkFriendlyPlugin, CodeTitlePlugin,
    ExternalLinksConfig, ExternalLinksPlugin, HardBreaksPlugin, HeadingLinksPlugin, MermaidPlugin,
    ResolveLinksPlugin, ResolveMarkdownLinksOptions, StripMdExtensionPlugin, SyntectPlugin,
    TocConfig, TocPlugin,
};
use crate::syntect_highlight::Highlighter;

// `ResolvedGfmConstructs` and the two `constructs_for_*` builders live in
// `zfb-md-ast` so `zfb-md-extras` can name the resolved flag set at its own
// parse sites (transclude re-parses every included file) without an upward
// dependency on this crate. Re-exported here so `zfb_content::pipeline::*`
// and `zfb::config`'s `pub use zfb_content::ResolvedGfmConstructs` are
// unaffected.
pub use zfb_md_ast::gfm_constructs::{
    constructs_for_jsx_emit, constructs_for_pipeline, constructs_for_target, ResolvedGfmConstructs,
};

/// Version prefix for the [`Pipeline::config_fingerprint`] descriptor.
/// Bump when the descriptor schema changes so entries written by an
/// older schema cannot collide with the new one (the compile cache is
/// in-memory only, so this only matters for mixed-version paranoia,
/// but it costs nothing).
///
/// Also bump on any change to the bundled syntect `SyntaxSet` (issue #1848):
/// the fingerprint does not otherwise encode the syntax set, so swapping in
/// a new grammar dump would silently leave stale compile-cache entries
/// pointing at HTML highlighted under the old grammar set.
const FINGERPRINT_VERSION: &str = "zfb-pipeline-fp-v2";

/// Canonical descriptor segment for a [`ResolvedGfmConstructs`] set.
fn gfm_fingerprint_segment(resolved: ResolvedGfmConstructs) -> String {
    // Drift guard (zfb#913): exhaustive destructure — NO `..` rest pattern.
    // Adding a construct flag to `ResolvedGfmConstructs` stops compiling
    // here until the new flag joins the descriptor below, instead of
    // silently aliasing compile-cache entries across configs that differ
    // only in that flag.
    let ResolvedGfmConstructs {
        strikethrough,
        table,
        autolink_literal,
        task_list_item,
        footnote_definition,
    } = resolved;
    format!(
        "gfm=strikethrough:{strikethrough},table:{table},autolink_literal:{autolink_literal},task_list_item:{task_list_item},footnote_definition:{footnote_definition}"
    )
}

/// Canonical descriptor segment for the `codeHighlight.themesDir` knob.
///
/// `None` (no dir configured) is a fixed token. `Some(dir)` hashes the
/// **bytes of every `.tmTheme` file** in the directory (same filter as
/// `Highlighter::load_themes_from_dir`), in sorted file-name order, so a
/// theme file edited between two pipeline constructions changes the
/// fingerprint instead of silently serving stale highlighted JSX from
/// the compile cache. Returns `None` (pipeline uncacheable) if the dir
/// or a theme file is unreadable — the constructor's own
/// `load_themes_from_dir` call has usually already failed loudly by
/// then, but a racing delete must degrade to "no caching", never to a
/// wrong fingerprint.
fn themes_dir_fingerprint_segment(themes_dir: Option<&Path>) -> Option<String> {
    let Some(dir) = themes_dir else {
        return Some("themes_dir=none".to_string());
    };
    let entries = std::fs::read_dir(dir).ok()?;
    let mut theme_files: Vec<std::path::PathBuf> = Vec::new();
    for entry in entries {
        let path = entry.ok()?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("tmTheme") {
            theme_files.push(path);
        }
    }
    theme_files.sort();
    let mut hasher = Sha256::new();
    for path in &theme_files {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            hasher.update(name.as_bytes());
        }
        hasher.update([0u8]);
        let bytes = std::fs::read(path).ok()?;
        hasher.update(&bytes);
        hasher.update([0u8]);
    }
    Some(format!(
        "themes_dir={}#{}",
        dir.display(),
        hex::encode(hasher.finalize())
    ))
}

/// Canonical descriptor segment for `markdown.features`.
///
/// `serde_json::to_value` alone is NOT order-stable here: the workspace
/// enables serde_json's `preserve_order` feature transitively, so `Map`
/// keeps insertion order and a `directives` HashMap's arbitrary
/// iteration order would leak into the descriptor. The explicit
/// recursive key sort makes two configs with equal contents produce
/// byte-identical JSON regardless of build features or hasher seed.
/// Returns `None` (pipeline uncacheable) on the never-expected
/// serialization failure.
fn features_fingerprint_segment(
    features: &zfb_md_extras::MarkdownFeaturesConfig,
) -> Option<String> {
    assert_features_fingerprint_covers_every_field(features);
    let mut v = serde_json::to_value(features).ok()?;
    sort_value_keys(&mut v);
    Some(format!("features={v}"))
}

/// Drift guard for the `markdown.features` part of the fingerprint
/// (zfb#913): exhaustively destructures [`MarkdownFeaturesConfig`] AND
/// every nested per-feature option struct/enum — **no `..` rest pattern
/// anywhere** — so adding a field to any of them is a compile error in
/// this function until the author decides how the fingerprint covers the
/// new knob:
///
/// - plain data fields are covered automatically by the canonical
///   features JSON ([`features_fingerprint_segment`] serializes the WHOLE
///   struct via serde) — bind the new field below and you are done. Do
///   NOT add `#[serde(skip)]` / `#[serde(skip_serializing_if)]` to these
///   structs: a skipped field silently vanishes from the descriptor (the
///   `canonical_features_json_covers_every_field` test in
///   `tests/pipeline_fingerprint.rs` pins the serialized key set as a
///   second line of defense);
/// - a feature whose plugin reads OTHER files at compile time must ALSO
///   record every read through the [`zfb_md_ast::ReadRecorder`]
///   (constructor clone wired in `register_features_config_derived`,
///   zfb#944) AND join the recorder-creation condition in
///   [`Pipeline::with_defaults_and_full_config`] — the compile cache
///   validates the recorded dependency manifest before serving a hit,
///   which is what lets such features stay cacheable.
///
/// All bindings are deliberately discarded; the function exists purely to
/// break the build on config drift (the optimizer removes it entirely).
///
/// [`MarkdownFeaturesConfig`]: zfb_md_extras::MarkdownFeaturesConfig
fn assert_features_fingerprint_covers_every_field(
    features: &zfb_md_extras::MarkdownFeaturesConfig,
) {
    use zfb_md_ast::{ReadingTimeFeature, ReadingTimeOptions};
    use zfb_md_extras::{
        CodeEnrichmentConfig, DirectiveFullSpec, DirectiveSpec, FeatureOptions, FeatureToggle,
        HeadingIdsConfig, HeadingMarkerTocFeature, ImageDimensionsConfig, LinkValidationConfig,
        MarkdownFeaturesConfig, TocExportConfig, TranscludeConfig,
    };

    let MarkdownFeaturesConfig {
        github_alerts,
        reading_time,
        code_enrichment,
        code_tabs,
        ruby,
        toc_export,
        image_dimensions,
        link_validation,
        transclude,
        directives,
        mermaid,
        heading_marker_toc,
        heading_ids,
    } = features;

    // Bool-or-empty-options toggles share the FeatureToggle shape.
    for toggle in [github_alerts, code_tabs, ruby, mermaid] {
        match toggle {
            Some(FeatureToggle::Options(FeatureOptions {}) | FeatureToggle::Bool(_)) | None => {}
        }
    }
    match reading_time {
        Some(
            ReadingTimeFeature::Options(ReadingTimeOptions { wpm: _ })
            | ReadingTimeFeature::Bool(_),
        )
        | None => {}
    }
    match code_enrichment {
        Some(CodeEnrichmentConfig {
            diff_markers: _,
            line_highlight: _,
            word_highlight: _,
        })
        | None => {}
    }
    match toc_export {
        Some(TocExportConfig { max_depth: _ }) | None => {}
    }
    match image_dimensions {
        Some(ImageDimensionsConfig { skip_remote: _ }) | None => {}
    }
    match link_validation {
        Some(LinkValidationConfig { fail_on_broken: _ }) | None => {}
    }
    match transclude {
        Some(TranscludeConfig { max_depth: _ }) | None => {}
    }
    if let Some(map) = directives {
        for spec in map.values() {
            match spec {
                DirectiveSpec::Full(DirectiveFullSpec {
                    component: _,
                    kind: _,
                    title_from_label: _,
                }) => {}
                DirectiveSpec::Short(_) => {}
            }
        }
    }
    match heading_marker_toc {
        Some(
            HeadingMarkerTocFeature::Config(TocConfig {
                heading: _,
                max_depth: _,
            })
            | HeadingMarkerTocFeature::Bool(_),
        )
        | None => {}
    }
    match heading_ids {
        Some(HeadingIdsConfig { strategy: _ }) | None => {}
    }
}

/// Recursively sort all object keys in `value` so its serialization is
/// deterministic even when serde_json's `preserve_order` feature is on
/// (a no-op when `Map` is already the sorted BTreeMap variant).
fn sort_value_keys(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> =
                std::mem::take(map).into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            for (_, v) in entries.iter_mut() {
                sort_value_keys(v);
            }
            *map = entries.into_iter().collect();
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                sort_value_keys(v);
            }
        }
        _ => {}
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
pub use zfb_md_ast::{BuildContext, HastNode, HastVisitor, MdastVisitor, SecondaryParseTarget};

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
    /// The project's `markdown.cjkFriendly` setting, as supplied to the
    /// constructor (zfb#1105).
    ///
    /// Set unconditionally by every constructor, BEFORE any visitor
    /// wiring runs — deliberately not inside the `if cjk_friendly` block
    /// that pushes [`CjkAutolinkBoundaryPlugin`], so it records the
    /// project's setting rather than "did we happen to take that
    /// branch". `register_features_config_derived` forwards it to the
    /// two secondary parse sites (transclude and
    /// `DirectiveRegistry::reparse_block`), which call
    /// `markdown::to_mdast` from *within* the visitor chain and so are
    /// never reached by the chain-index-0 plugin (zfb#2390). Each site
    /// ANDs it with `autolink_literal` itself.
    ///
    /// Not a fingerprint input of its own: `cjk_friendly` is already
    /// covered by the descriptor.
    cjk_friendly: bool,
    /// The project's `markdown.hardBreaks` setting, as supplied to the
    /// constructor (zfb#2398).
    ///
    /// Mirrors [`Pipeline::cjk_friendly`] exactly: set unconditionally by
    /// every full-config constructor, before any visitor wiring, so
    /// `register_features_config_derived` can forward the project's raw
    /// setting to the same two secondary parse sites (transclude and
    /// `DirectiveRegistry::reparse_block`), which re-enter
    /// `markdown::to_mdast` from inside the visitor chain and so are
    /// never reached by the chain-index-N `HardBreaksPlugin` copy
    /// `Pipeline` wires into its own chain (zfb#2398).
    ///
    /// Not a fingerprint input of its own: `hard_breaks` is already
    /// covered by the descriptor.
    hard_breaks: bool,
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
    /// after the directives step) so link rewriting sees finalized
    /// mdast link nodes.
    resolve_links: Option<ResolveLinksPlugin>,
    /// Optional JSX-nested link-candidate collector (zfb#2184), created
    /// in lockstep with `LinkValidationPlugin` when
    /// `markdown.features.linkValidation` is enabled — the two share one
    /// `Arc<Mutex<…>>` candidate buffer (see
    /// `register_features_config_derived`).
    ///
    /// Deliberately NOT in `mdast_visitors`: registered mdast visitors
    /// run BEFORE the `resolve_links` application, and the collector
    /// MUST run after it — `ResolveLinksPlugin::visit` descends into JSX
    /// children and rewrites nested `Link.url` in place, so collecting
    /// pre-rewrite spellings would validate `./page.md` forms that
    /// resolve_links turns into (skipped) URL-space hrefs: false
    /// positives. Applied by `run_with_context` /
    /// `apply_mdast_visitors_with_context` only — the context-free paths
    /// never validate, so they never collect.
    jsx_nested_link_collector: Option<zfb_md_extras::link_validation::JsxNestedLinkCollector>,
    /// Optional JSX-nested image-dimensions stub (zfb#2247), created in
    /// lockstep with the hast-phase `ImageDimensionsPlugin` when
    /// `markdown.features.imageDimensions` is enabled — the two share one
    /// dimensions-cache `Arc` (+ read_count + recorder clone; see
    /// `register_features_config_derived`), mirroring the
    /// `jsx_nested_link_collector` shared-buffer precedent above.
    ///
    /// Applied LAST in the mdast phase (after the `jsx_nested_link_collector`
    /// block), at both `apply_mdast_visitors_with_context` and
    /// `apply_mdast_visitors`. Placement after the collector is load-bearing:
    /// replacing a JSX-nested `Image` before collection would delete its
    /// `is_img` existence-check candidate (a #2225 validation-coverage
    /// regression). No post-`resolve_links` need of its own — this pass
    /// never touches `Link`/`Image` urls. This sub only wires the field and
    /// application slot; the visitor itself is a no-op stub — behavior lands
    /// in #2248.
    jsx_nested_image_dimensions: Option<zfb_md_extras::image_dimensions::JsxNestedImageDimensions>,
    /// Optional JSX-nested external-links stub (zfb#2247), constructed
    /// from the same `config` + `site` as the hast-phase
    /// `ExternalLinksPlugin` in [`Pipeline::add_external_links`].
    ///
    /// Applied LAST in the mdast phase, after `jsx_nested_image_dimensions`
    /// (order-insensitive by construction, pinned deterministically here),
    /// at both `apply_mdast_visitors_with_context` and
    /// `apply_mdast_visitors`. No post-`resolve_links` need: `ResolveLinksPlugin::visit`
    /// only ever matches `MdastNode::Link` (`resolve_links.rs`), so `Image`
    /// urls are never rewritten by it, and external-vs-internal
    /// classification of a `Link`'s URL is rewrite-invariant either way.
    /// This sub only wires the field and application slot; the visitor
    /// itself is a no-op stub — behavior lands in #2249.
    jsx_nested_external_links: Option<crate::plugins::external_links::JsxNestedExternalLinks>,
    /// Heading-ID strategy the wired `HeadingLinksPlugin` uses
    /// (`markdown.features.headingIds`, zfb#871). Read back by the
    /// JSX-emit path so `collect_headings` mirrors the same scheme.
    /// Default: [`HeadingIdStrategy::Flat`].
    heading_id_strategy: HeadingIdStrategy,
    /// Base descriptor of the config this pipeline was constructed from,
    /// set by the constructors (see [`Pipeline::config_fingerprint`]).
    ///
    /// `None` means "uncacheable": the pipeline's visitor chain can no
    /// longer be derived from config alone — a raw trait-object visitor
    /// was appended via [`Pipeline::add_mdast_visitor`] /
    /// [`Pipeline::add_hast_visitor`].
    config_fingerprint_base: Option<String>,
    /// Descriptor segments appended by the named config-driven mutators
    /// ([`Pipeline::add_toc`], [`Pipeline::add_strip_md_ext`],
    /// [`Pipeline::add_external_links`],
    /// [`Pipeline::set_heading_id_strategy`]). Sorted before hashing so
    /// the bundler's and the snapshot walker's differing *call* order
    /// (which produces the identical effective visitor chain — `add_toc`
    /// inserts at a fixed position) yields the same fingerprint.
    config_fingerprint_extras: Vec<String>,
    /// Optional read-recorder shared with filesystem-reading feature
    /// plugins (zfb#942). When set, `compile_mdx_to_jsx_module_cached`
    /// clears it before each compile and drains the recorded reads into
    /// the cache entry's [`DependencyManifest`] afterwards. See
    /// [`Pipeline::set_read_recorder`].
    read_recorder: Option<Arc<ReadRecorder>>,
    /// Project-level roots for the per-file `BuildContext` the JSX-emit
    /// path builds when compiling through
    /// `compile_mdx_to_jsx_module_cached` (zfb#944): `(project_root,
    /// public_dir)`. `None` (the default) keeps the context-free emit
    /// path — context-aware feature plugins (transclude,
    /// imageDimensions, linkValidation) stay no-ops there. See
    /// [`Pipeline::set_build_context_roots`].
    build_context_roots: Option<(PathBuf, PathBuf)>,
    /// Markdown diagnostics emitted by context-aware feature plugins
    /// through the per-file `BuildContext` sink during JSX-emit compiles
    /// (zfb#944) — e.g. linkValidation broken-link findings. Buffered
    /// here (mirroring the resolve-links `broken_links` side channel)
    /// so the compile cache can store the slice one compile appended
    /// and replay it on a hit; call sites drain via
    /// [`Pipeline::take_markdown_diagnostics`].
    markdown_diagnostics: Vec<MarkdownDiagnostic>,
    /// Cross-file fragment-link candidates recorded by
    /// `LinkValidationPlugin` through the per-file `BuildContext` during
    /// JSX-emit compiles (#960 / #977). Buffered exactly like
    /// [`Pipeline::markdown_diagnostics`] — observational side channel,
    /// never part of the config fingerprint or cache-key shape — so the
    /// compile cache can store the slice one compile appended and
    /// replay it on a hit; the post-compile cross-file check drains via
    /// [`Pipeline::take_cross_file_link_candidates`].
    cross_file_link_candidates: Vec<CrossFileLinkCandidate>,
    /// Per-file heading records surfaced from the JSX-emit path's
    /// canonical `collect_headings` walk during context-armed compiles
    /// (#960 / #977). Same buffering/store/replay discipline as the
    /// candidates channel above; drained via
    /// [`Pipeline::take_file_headings`].
    file_headings: Vec<FileHeadings>,
    /// Whether `markdown.features.linkValidation` is enabled
    /// (#960 / #977). Gates the per-file headings side channel in the
    /// JSX-emit path so configs without linkValidation record nothing.
    /// Derived purely from the `features` config (already in the
    /// fingerprint) — NEVER fingerprinted separately.
    link_validation_enabled: bool,
    /// Severity used when promoting trusted resolver fragment metadata into
    /// the existing build-wide cross-file candidate channel.
    resolved_link_fragment_severity: Option<DiagnosticSeverity>,
    /// Configuration-derived spec for reconstructing the code-block hast
    /// chain on demand, so fenced code nested inside an MDX JSX body /
    /// `:::` directive can be highlighted exactly like a top-level fence
    /// (#2207). Set only by the two default-chain constructor families
    /// ([`Pipeline::build_defaults`] and
    /// [`Pipeline::with_defaults_and_full_config_inner`]) — the ones
    /// that wire a [`SyntectPlugin`]. `None` (bare / manually-wired
    /// pipelines) means "no chain": the JSX-emit path keeps its
    /// byte-stable fallback emission for nested fences.
    ///
    /// A spec rather than a prebuilt `Vec<Box<dyn HastVisitor>>` because
    /// the live visitor chain is non-cloneable trait objects; the getter
    /// ([`Pipeline::nested_code_render_chain`]) reconstructs fresh
    /// instances per call from this stored config. Purely derived from
    /// already-fingerprinted config — never fingerprinted separately.
    nested_code_chain_spec: Option<NestedCodeChainSpec>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
enum HiEmit<'a> {
    Single(Option<&'a str>),
    Dual(&'a str, &'a str),
    Class(&'a str, &'a BTreeMap<String, String>),
}

/// Stored config for [`Pipeline::nested_code_render_chain`] (#2207): the
/// minimum needed to reconstruct the code-block hast chain — code-title →
/// mermaid → syntect → code-enrichment (the documented ordering contract
/// of the default constructors) — for one detached nested fence.
///
/// [`CodeTitlePlugin`] is unconditionally part of every chain (both
/// constructor families always wire it), so it carries no field here.
/// [`SyntectPlugin`] is stored as a ready instance (it is `Clone` — the
/// heavy `Highlighter` sits behind an `Arc`) so the getter reproduces the
/// exact single/dual/class emission mode of the live chain. Mermaid and
/// code-enrichment are stored as their enabling config: both plugin types
/// are stateless but not `Clone`, and reconstruction from config is the
/// same thing the constructors themselves do.
struct NestedCodeChainSpec {
    /// Whether the constructed chain wired a [`MermaidPlugin`] —
    /// always `true` for the legacy `with_defaults*` family, and
    /// `feature_enabled(features.mermaid)` for the full-config family.
    mermaid: bool,
    /// Mode-carrying clone of the exact `SyntectPlugin` the live hast
    /// chain runs (single theme / dual themes / class emission).
    syntect: SyntectPlugin,
    /// `markdown.features.codeEnrichment` when enabled — the post-syntect
    /// enrichment visitor's construction config. `None` for the legacy
    /// family (which never wires enrichment) and for configs without it.
    code_enrichment: Option<zfb_md_ast::CodeEnrichmentConfig>,
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
    /// `zfb_content::content_bridge::build_snapshot_with_config` calls this
    /// out explicitly.
    ///
    /// `gfm_label_start_footnote` is mirrored to the resolved
    /// `footnote_definition` flag because the two constructs are
    /// paired in markdown-rs: enabling the definition without the
    /// label-start means the parser sees `[^a]: body` but never the
    /// `[^a]` reference at the use site.
    #[must_use]
    pub fn with_resolved_gfm_constructs(resolved: ResolvedGfmConstructs) -> Self {
        let constructs = constructs_for_pipeline(resolved);
        Self {
            mdast_visitors: Vec::new(),
            hast_visitors: Vec::new(),
            parse_options: markdown::ParseOptions {
                constructs,
                ..markdown::ParseOptions::mdx()
            },
            gfm_constructs: resolved,
            cjk_friendly: false,
            hard_breaks: false,
            add_trailing_slash: true,
            resolve_links: None,
            jsx_nested_link_collector: None,
            jsx_nested_image_dimensions: None,
            jsx_nested_external_links: None,
            heading_id_strategy: HeadingIdStrategy::default(),
            config_fingerprint_base: Some(format!(
                "{FINGERPRINT_VERSION};bare;{}",
                gfm_fingerprint_segment(resolved)
            )),
            config_fingerprint_extras: Vec::new(),
            read_recorder: None,
            build_context_roots: None,
            markdown_diagnostics: Vec::new(),
            cross_file_link_candidates: Vec::new(),
            file_headings: Vec::new(),
            link_validation_enabled: false,
            resolved_link_fragment_severity: None,
            nested_code_chain_spec: None,
        }
    }

    /// Config-derived fingerprint of this pipeline, or `None` when the
    /// pipeline is **uncacheable**.
    ///
    /// The fingerprint is a SHA-256 hex digest over a canonical
    /// descriptor of every config knob that can change the JSX a given
    /// input emits: the constructor family (bare / legacy defaults /
    /// feature-aware full config), resolved GFM construct flags, the
    /// syntect highlight theme, `themesDir` (path **and** the bytes of
    /// every `.tmTheme` file in it), the CJK-friendly and hard-breaks
    /// toggles, the full `markdown.features` config (canonical JSON,
    /// map keys sorted), plus any named config-driven mutators applied
    /// after construction (`add_toc`, `add_strip_md_ext`,
    /// `add_external_links`, `add_resolve_links`,
    /// `set_heading_id_strategy`).
    ///
    /// [`compile_mdx_to_jsx_module_cached`] combines this fingerprint
    /// with `sha256(input)` (plus the per-call
    /// [`Pipeline::cache_key_context`], when present) to form its cache
    /// key, so two pipelines built from different configs can never
    /// alias one cache entry.
    ///
    /// `None` (uncacheable — the compile cache is bypassed) when:
    ///
    /// - a raw trait-object visitor was appended via
    ///   [`Pipeline::add_mdast_visitor`] / [`Pipeline::add_hast_visitor`]
    ///   (including the public `register_features` /
    ///   `register_post_syntect_features` helpers when called manually
    ///   after construction) — an arbitrary `Box<dyn …Visitor>` cannot
    ///   be fingerprinted reliably;
    /// - a `themesDir` was configured but became unreadable while
    ///   computing the fingerprint.
    ///
    /// Filesystem-reading feature plugins (`transclude`,
    /// `imageDimensions`, `linkValidation`) no longer bail (the pre-#944
    /// gate): their config is covered by the canonical features JSON,
    /// and their per-file reads are validated through the read-recorder
    /// dependency manifest ([`Pipeline::set_read_recorder`], zfb#942/#944)
    /// before any cache hit is honoured.
    ///
    /// [`compile_mdx_to_jsx_module_cached`]: crate::mdx_jsx_emit::compile_mdx_to_jsx_module_cached
    #[must_use]
    pub fn config_fingerprint(&self) -> Option<String> {
        let base = self.config_fingerprint_base.as_ref()?;
        let mut hasher = Sha256::new();
        hasher.update(base.as_bytes());
        let mut extras = self.config_fingerprint_extras.clone();
        extras.sort();
        for segment in &extras {
            // NUL separator: descriptor segments never contain NUL, so
            // concatenation ambiguity ("ab"+"c" vs "a"+"bc") is impossible.
            hasher.update([0u8]);
            hasher.update(segment.as_bytes());
        }
        Some(hex::encode(hasher.finalize()))
    }

    /// Replace the fingerprint base descriptor (constructors only).
    /// Clears any extras accumulated during internal wiring.
    fn set_config_fingerprint_base(&mut self, descriptor: Option<String>) {
        self.config_fingerprint_base = descriptor;
        self.config_fingerprint_extras.clear();
    }

    /// Mark the pipeline uncacheable (see [`Pipeline::config_fingerprint`]).
    fn invalidate_config_fingerprint(&mut self) {
        self.config_fingerprint_base = None;
        self.config_fingerprint_extras.clear();
    }

    /// Record a named config-driven mutation in the fingerprint. No-op
    /// when the pipeline is already uncacheable.
    fn extend_config_fingerprint(&mut self, segment: String) {
        if self.config_fingerprint_base.is_some() {
            self.config_fingerprint_extras.push(segment);
        }
    }

    /// Internal, non-invalidating visitor push.
    ///
    /// **Invalidation rule (zfb#913): internal pushes that derive purely
    /// from already-fingerprinted config MUST NOT invalidate the
    /// fingerprint; manual external pushes MUST.**
    ///
    /// Internal wiring — the constructors, the named config mutators
    /// ([`Pipeline::add_toc`], [`Pipeline::add_strip_md_ext`],
    /// [`Pipeline::add_external_links`]), and the `register_features*`
    /// internals — builds the visitor chain from config the fingerprint
    /// already (or finally) describes, so it pushes through these private
    /// helpers. Two obligations come with the shortcut:
    ///
    /// - a **constructor** that wires visitors this way MUST finish with
    ///   [`Pipeline::set_config_fingerprint_base`], passing a descriptor
    ///   covering every knob it consumed (or `None` for uncacheable
    ///   shapes) — leaving the interim bare-constructor descriptor in
    ///   place would alias the bare pipeline's compile-cache entries;
    /// - a **post-construction named mutator** MUST call
    ///   [`Pipeline::extend_config_fingerprint`] with a segment capturing
    ///   its full config.
    ///
    /// Everything reachable by consumers — [`Pipeline::add_mdast_visitor`],
    /// [`Pipeline::add_hast_visitor`], [`register_features`],
    /// [`register_post_syntect_features`] — MUST invalidate instead: an
    /// arbitrary external mutation cannot be derived from config, so the
    /// compile cache must never key it.
    fn push_config_derived_mdast_visitor(&mut self, v: Box<dyn MdastVisitor>) {
        self.mdast_visitors.push(v);
    }

    /// Hast twin of [`Pipeline::push_config_derived_mdast_visitor`] —
    /// same invalidation rule.
    fn push_config_derived_hast_visitor(&mut self, v: Box<dyn HastVisitor>) {
        self.hast_visitors.push(v);
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

    /// The heading-ID strategy this pipeline's `HeadingLinksPlugin` was
    /// wired with (`markdown.features.headingIds`, zfb#871).
    ///
    /// The JSX-emit detour reads this so `collect_headings` computes the
    /// same slugs the rendered `<hN id="…">` carries. Defaults to
    /// [`HeadingIdStrategy::Flat`] on every constructor except
    /// [`Pipeline::with_defaults_and_full_config`], which resolves it from
    /// `markdown.features`.
    #[must_use]
    pub fn heading_id_strategy(&self) -> HeadingIdStrategy {
        self.heading_id_strategy
    }

    /// Reconstruct the code-block hast chain for rendering ONE detached
    /// fenced code block, in the documented ordering contract of the
    /// default constructors: code-title → mermaid → syntect →
    /// code-enrichment (#2207).
    ///
    /// The JSX-emit path uses this so a fence nested inside an MDX JSX
    /// element body (`<Note>…</Note>`) or a `:::` directive — which the
    /// mdast→hast conversion flattens into an opaque
    /// [`HastNode::JsxRaw`] payload that the live hast visitors never
    /// traverse — is highlighted exactly like a top-level fence. The
    /// chain is handed out OWNED (fresh instances per call,
    /// reconstructed from the stored `NestedCodeChainSpec` config)
    /// so the caller can run it inside the emit walk without borrowing
    /// the pipeline — `mdx_to_jsx_module_inner` still needs
    /// `&mut Pipeline` afterwards for `apply_hast_visitors`.
    ///
    /// Every visitor in the chain is stateless per-document
    /// ([`CodeTitlePlugin`], [`MermaidPlugin`], [`SyntectPlugin`], the
    /// enrichment plugin), so fresh instances behave identically to the
    /// live chain's and need no `reset()` discipline.
    ///
    /// Returns `None` for pipelines whose construction wired no
    /// [`SyntectPlugin`] (bare [`Pipeline::new`] / [`Pipeline::with_mdx`]
    /// / manually-assembled chains): callers must keep their existing
    /// fallback emission byte-stable in that case.
    #[must_use]
    pub fn nested_code_render_chain(&self) -> Option<Vec<Box<dyn HastVisitor>>> {
        let spec = self.nested_code_chain_spec.as_ref()?;
        let mut chain: Vec<Box<dyn HastVisitor>> = Vec::new();
        chain.push(Box::new(CodeTitlePlugin::new()));
        if spec.mermaid {
            chain.push(Box::new(MermaidPlugin::new()));
        }
        chain.push(Box::new(spec.syntect.clone()));
        if let Some(cfg) = &spec.code_enrichment {
            chain.push(Box::new(
                zfb_md_extras::code_enrichment::CodeEnrichmentPlugin::new(cfg.clone()),
            ));
        }
        Some(chain)
    }

    /// Declare the heading-ID strategy of a manually wired
    /// [`HeadingLinksPlugin`] so the JSX-emit path (`collect_headings`)
    /// mirrors the rendered `<hN id="…">`.
    ///
    /// Only needed for custom pipelines that call
    /// [`add_hast_visitor`](Pipeline::add_hast_visitor) with
    /// `HeadingLinksPlugin::with_strategy(…)` themselves — without this,
    /// the `headings` export stays on the flat default while the rendered
    /// ids are hierarchical. [`Pipeline::with_defaults_and_full_config`]
    /// sets it automatically from `features.headingIds`.
    pub fn set_heading_id_strategy(&mut self, strategy: HeadingIdStrategy) -> &mut Self {
        self.heading_id_strategy = strategy;
        // The strategy changes the emitted `headings` export, so it is a
        // cache-key knob (see `config_fingerprint`). Inside
        // `with_defaults_and_full_config` this extra is superseded by the
        // final base descriptor, which captures the strategy via the
        // canonical features JSON.
        self.extend_config_fingerprint(format!("heading_id_strategy={strategy:?}"));
        self
    }

    /// Set the `add_trailing_slash` option. Affects subsequent
    /// `add_strip_md_ext()` calls. Defaults to `true`.
    pub fn set_add_trailing_slash(&mut self, value: bool) -> &mut Self {
        self.add_trailing_slash = value;
        self
    }

    /// Append a [`StripMdExtensionPlugin`] configured by the pipeline's
    /// current `add_trailing_slash` setting (defaults to `true`).
    ///
    /// Config-driven and deterministic per input, so this **extends**
    /// the config fingerprint instead of invalidating it (invalidation
    /// rule — see [`Pipeline::push_config_derived_hast_visitor`]).
    pub fn add_strip_md_ext(&mut self) -> &mut Self {
        let plugin = if self.add_trailing_slash {
            StripMdExtensionPlugin::with_trailing_slash()
        } else {
            StripMdExtensionPlugin::new()
        };
        self.push_config_derived_hast_visitor(Box::new(plugin));
        self.extend_config_fingerprint(format!(
            "strip_md_ext;trailing_slash={}",
            self.add_trailing_slash
        ));
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
    ///
    /// Config-driven and deterministic per input, so this **extends**
    /// the config fingerprint instead of invalidating it (invalidation
    /// rule — see [`Pipeline::push_config_derived_hast_visitor`]).
    pub fn add_external_links(
        &mut self,
        config: ExternalLinksConfig,
        site: Option<&str>,
    ) -> &mut Self {
        let segment = format!(
            "external_links;rel={:?};target={:?};site={:?}",
            config.rel, config.target, site
        );
        // JSX-nested external-links stub (zfb#2247): constructed from the
        // SAME config + site as the hast-phase plugin below, beside its
        // push — no fingerprint machinery needed, `segment` already covers
        // the config.
        self.jsx_nested_external_links = Some(
            crate::plugins::external_links::JsxNestedExternalLinks::new(config.clone(), site),
        );
        self.push_config_derived_hast_visitor(Box::new(ExternalLinksPlugin::new(config, site)));
        self.extend_config_fingerprint(segment);
        self
    }

    /// Wire a [`ResolveLinksPlugin`] into the pipeline's mdast phase.
    ///
    /// The plugin is applied before the generic mdast visitors so it
    /// runs on the raw mdast before the directives step transforms
    /// directives. The `source_dir` slot is empty until the caller
    /// calls [`Pipeline::set_resolve_links_source_dir`] per file.
    ///
    /// Call at most once per pipeline instance — a second call
    /// replaces the previous plugin (and its fingerprint segment).
    ///
    /// **Cacheable (zfb#939).** The plugin never reads the filesystem at
    /// compile time — it resolves against this prebuilt `source_map`
    /// plus the per-file `source_dir` — so wiring it **extends** the
    /// config fingerprint with a digest of the map instead of
    /// invalidating. The map is rebuilt from the content tree each dev
    /// tick, so a content add/remove/rename changes the digest and
    /// correctly invalidates every cached entry whose links could now
    /// resolve differently. The two remaining per-file dependencies are
    /// handled by [`compile_mdx_to_jsx_module_cached`]:
    ///
    /// - the per-file `source_dir` joins the cache key as a per-call
    ///   context segment ([`Pipeline::cache_key_context`]);
    /// - broken-link diagnostics are stored with the cached entry and
    ///   replayed into this plugin on a hit, so draining
    ///   [`Pipeline::take_broken_links`] after a hit observes exactly
    ///   what a fresh compile would have produced.
    ///
    /// Digest canonicalisation: entries are sorted and hashed as
    /// length-delimited records (no separator/`=` ambiguity), with the
    /// path keys normalised by the shared lexical helper
    /// (`path_norm::normalize_path_lexically`), whose canonical form
    /// mirrors `Path` equality — exactly the spellings the runtime
    /// `HashMap<PathBuf, _>` lookup merges digest identically, and the
    /// ones it distinguishes (`..`, leading `./`) stay distinct.
    ///
    /// [`compile_mdx_to_jsx_module_cached`]: crate::mdx_jsx_emit::compile_mdx_to_jsx_module_cached
    pub fn add_resolve_links(
        &mut self,
        source_map: std::collections::HashMap<std::path::PathBuf, String>,
    ) -> &mut Self {
        let mut records: Vec<(String, &str)> = source_map
            .iter()
            .map(|(path, url)| (normalize_path_lexically(path), url.as_str()))
            .collect();
        records.sort();
        let mut hasher = Sha256::new();
        for (path, url) in &records {
            hasher.update((path.len() as u64).to_le_bytes());
            hasher.update(path.as_bytes());
            hasher.update((url.len() as u64).to_le_bytes());
            hasher.update(url.as_bytes());
        }
        let digest = hex::encode(hasher.finalize());

        self.resolve_links = Some(ResolveLinksPlugin::new(ResolveMarkdownLinksOptions {
            source_map,
            source_dir: None,
        }));
        // A second call REPLACES the plugin (doc contract above), so any
        // previously-pushed segment must go too — a stale map digest
        // lingering in the extras would split the fingerprint.
        self.config_fingerprint_extras
            .retain(|s| !s.starts_with("resolve_links;"));
        self.extend_config_fingerprint(format!("resolve_links;source_map_sha256={digest}"));
        self
    }

    /// Update the per-file source directory used by the wired
    /// [`ResolveLinksPlugin`] to resolve relative link targets.
    ///
    /// Call once per MDX file, before `apply_mdast_visitors`, so
    /// `./other.mdx` links are resolved against the correct directory.
    /// No-op when [`add_resolve_links`](Pipeline::add_resolve_links)
    /// was not called.
    ///
    /// Disarms the URL-space fallback a previous
    /// [`Pipeline::set_resolve_links_source_file`] call may have armed —
    /// prefer that setter, which derives the directory AND the fallback
    /// state from one input (zfb#1030).
    pub fn set_resolve_links_source_dir(&mut self, dir: std::path::PathBuf) {
        if let Some(p) = self.resolve_links.as_mut() {
            p.set_source_dir(dir);
        }
    }

    /// Update the wired [`ResolveLinksPlugin`]'s per-file context from
    /// the source **file** path (zfb#1030).
    ///
    /// Like [`Pipeline::set_resolve_links_source_dir`] (the directory
    /// becomes the file's parent), but additionally arms the URL-space
    /// fallback for non-index files so dir-style hrefs written against
    /// the rendered URL (`../other-article/` from `section/article.mdx`)
    /// resolve when the file-space probe misses. Call once per MDX file,
    /// before `apply_mdast_visitors`. No-op when
    /// [`add_resolve_links`](Pipeline::add_resolve_links) was not called.
    pub fn set_resolve_links_source_file(&mut self, file: std::path::PathBuf) {
        if let Some(p) = self.resolve_links.as_mut() {
            p.set_source_file(file);
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
        // Config-driven and deterministic per input → extends the config
        // fingerprint (see `config_fingerprint`) rather than invalidating
        // (invalidation rule — see `push_config_derived_hast_visitor`).
        let segment = format!("toc;heading={:?};max_depth={}", cfg.heading, cfg.max_depth);
        // Insert at position 1 in the hast visitors list so TOC runs
        // immediately after HeadingLinksPlugin (index 0) and before all
        // subsequent hast visitors. This guarantees ids are already set.
        // Inserted at a fixed position rather than pushed, so this site
        // manipulates `hast_visitors` directly instead of going through
        // the push helper — same non-invalidating contract.
        //
        // If the list is empty (e.g. in a bare pipeline built without
        // with_defaults), append normally so the visitor still runs.
        let toc = Box::new(TocPlugin::new(cfg)) as Box<dyn HastVisitor>;
        if self.hast_visitors.is_empty() {
            self.hast_visitors.push(toc);
        } else {
            self.hast_visitors.insert(1, toc);
        }
        self.extend_config_fingerprint(segment);
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

    /// Per-call cache-key context for `compile_mdx_to_jsx_module_cached`
    /// (zfb#939).
    ///
    /// [`Pipeline::config_fingerprint`] covers construction-time config
    /// only; this surfaces the per-FILE pipeline state that also shapes
    /// the emitted JSX — the wired `ResolveLinksPlugin`'s `source_dir`
    /// plus its URL-space fallback base (both set between compiles via
    /// [`Pipeline::set_resolve_links_source_file`] /
    /// [`Pipeline::set_resolve_links_source_dir`]; they change how
    /// relative link targets resolve). `None` when no per-file state is
    /// in play, keeping the cache key byte-identical to the pre-#939
    /// two-part shape for every other pipeline.
    ///
    /// Both paths are normalised with the same lexical helper as the
    /// source-map digest in [`Pipeline::add_resolve_links`]: spellings
    /// the runtime lookup treats as one dir (`Path` equality) key
    /// identically, while dirs whose lookups can differ never collide.
    /// An unset value maps to a distinct `none` token — `source_dir =
    /// None` only performs absolute lookups, and an unarmed fallback
    /// (index file, or dir-only setter) resolves dir-style hrefs
    /// differently from any armed base (zfb#1030: `section/index.mdx`
    /// and `section/article.mdx` share a `source_dir` but must never
    /// alias a cache entry).
    pub(crate) fn cache_key_context(&self) -> Option<String> {
        self.resolve_links.as_ref().map(|p| {
            let dir = match p.source_dir() {
                Some(dir) => format!("some:{}", normalize_path_lexically(dir)),
                None => "none".to_string(),
            };
            let url_dir = match p.url_space_dir() {
                Some(dir) => format!("some:{}", normalize_path_lexically(dir)),
                None => "none".to_string(),
            };
            format!("resolve_links_source_dir={dir};url_dir={url_dir}")
        })
    }

    /// Number of broken-link diagnostics currently buffered (not yet
    /// drained). The compile cache snapshots this before a compile so it
    /// can slice off exactly the diagnostics that compile appended
    /// (zfb#939) — the buffer may still hold earlier files' diagnostics
    /// when the caller drains lazily (the snapshot walker never drains).
    pub(crate) fn broken_links_len(&self) -> usize {
        self.resolve_links
            .as_ref()
            .map_or(0, ResolveLinksPlugin::broken_links_len)
    }

    /// Clone the broken-link diagnostics buffered at index `from`
    /// onward, without draining (zfb#939 — see
    /// [`Pipeline::broken_links_len`]).
    pub(crate) fn broken_links_since(&self, from: usize) -> Vec<BrokenLinkDiagnostic> {
        self.resolve_links
            .as_ref()
            .map(|p| p.broken_links_since(from))
            .unwrap_or_default()
    }

    /// Cache-hit replay (zfb#939): re-inject diagnostics stored with a
    /// cached compile so call sites draining
    /// [`Pipeline::take_broken_links`] after a hit observe exactly what
    /// the fresh compile produced. No-op when the plugin is not wired —
    /// unreachable in practice, because entries carrying diagnostics are
    /// only ever keyed under a resolve-links fingerprint.
    pub(crate) fn replay_broken_links(&mut self, diags: Vec<BrokenLinkDiagnostic>) {
        if diags.is_empty() {
            return;
        }
        if let Some(p) = self.resolve_links.as_mut() {
            p.replay_broken_links(diags);
        }
    }

    /// Attach the read-recorder filesystem-reading feature plugins
    /// report their external reads through (zfb#942).
    ///
    /// The caller wires the SAME `Arc` into the plugins (they receive a
    /// clone at construction) and into the pipeline here, so the
    /// compile-cache choke point (`compile_mdx_to_jsx_module_cached`)
    /// can scope the recording per compile: it clears the recorder
    /// before each compile and drains the recorded reads into the
    /// cache entry's [`DependencyManifest`] afterwards. Because of that
    /// clear/drain cycle, a recorder instance must serve exactly ONE
    /// pipeline — sharing it across pipelines would interleave reads
    /// from unrelated compiles.
    ///
    /// **Fingerprint-neutral.** The recorder is observational: it never
    /// changes the JSX a given input emits, so attaching it does NOT
    /// invalidate [`Pipeline::config_fingerprint`]. (Since zfb#944 the
    /// recording plugins ARE fingerprintable — the old
    /// `filesystem_dependent_feature` gate in
    /// [`Pipeline::with_defaults_and_full_config`] is flipped, and that
    /// constructor wires one recorder per pipeline whenever a
    /// filesystem-reading feature is enabled.)
    ///
    /// **Cache-key effect.** Because recording plugins may resolve
    /// reads relative to the file being compiled, a recorder-armed
    /// pipeline makes `compile_mdx_to_jsx_module_cached` append the
    /// source file's parent directory to the cache key — identical
    /// bodies in different directories stop sharing an entry (their
    /// relative reads can differ), while same-directory bodies still
    /// dedupe. See the key-shape docs on that function.
    pub fn set_read_recorder(&mut self, recorder: Arc<ReadRecorder>) -> &mut Self {
        self.read_recorder = Some(recorder);
        self
    }

    /// The attached read-recorder, if any (see
    /// [`Pipeline::set_read_recorder`]).
    #[must_use]
    pub fn read_recorder(&self) -> Option<&Arc<ReadRecorder>> {
        self.read_recorder.as_ref()
    }

    /// Discard reads left in the recorder by an earlier compile (e.g.
    /// one that aborted on a parse error), so they cannot leak into
    /// the next entry's manifest. Called by
    /// `compile_mdx_to_jsx_module_cached` before every compile. No-op
    /// without a recorder.
    pub(crate) fn clear_recorded_reads(&self) {
        if let Some(r) = &self.read_recorder {
            r.clear();
        }
    }

    /// Drain the reads recorded since the last clear into the
    /// [`DependencyManifest`] stored with the cache entry, normalising
    /// each path via the shared `path_norm` helper. Empty manifest
    /// without a recorder (the shape of every plain pipeline).
    pub(crate) fn take_dependency_manifest(&self) -> DependencyManifest {
        self.read_recorder
            .as_ref()
            .map(|r| DependencyManifest::from_recorded_reads(r.take_reads()))
            .unwrap_or_default()
    }

    /// Arm per-file `BuildContext` threading on the JSX-emit path
    /// (zfb#944): `compile_mdx_to_jsx_module_cached` builds a
    /// `BuildContext { source_path: <the compiled file>, project_root,
    /// public_dir, .. }` for every compile and applies the visitor
    /// chains through their `*_with_context` variants, so the
    /// context-aware feature plugins (transclude, imageDimensions,
    /// linkValidation) actually fire — and record their reads — instead
    /// of no-opping. Without this call the emit path stays context-free
    /// and byte-identical to before.
    ///
    /// **Fingerprint-extending.** Both roots shape the emitted JSX
    /// (containment checks, `/`-absolute image resolution), so they join
    /// the config fingerprint as a normalised segment — two pipelines
    /// armed with different roots can never alias one compile-cache
    /// entry. A second call replaces the roots AND the segment.
    ///
    /// **Cache-key effect.** A roots-armed pipeline additionally keys
    /// every compile-cache entry by the full normalised source PATH (not
    /// just the recorder's parent-dir segment): context-aware plugins
    /// observe the source path itself (transclude seeds its cycle
    /// detection with it; linkValidation stamps it into diagnostic
    /// locations), so identical bodies in one directory may legitimately
    /// produce different output/diagnostics per file. See the key-shape
    /// docs on `compile_mdx_to_jsx_module_cached`.
    ///
    /// The per-compile context carries a compile-local `HeadingRegistry`
    /// seeded from `collect_headings` (same-file anchor validation, zfb#954).
    /// BUILD-scoped cross-file registry (validating `./other.md#frag` across
    /// files) remains deferred to #960 — structurally incompatible with
    /// cache-hit replay today (HeadingLinksPlugin never runs on a hit).
    ///
    /// Production pipelines (`PipelineSpec::build_pipeline` — bundler and
    /// snapshot walker; `zfb dev` is the bundler in Development mode) arm
    /// this when a filesystem-dependent feature is enabled in the config
    /// (transclude / imageDimensions / linkValidation).  Unarmed pipelines
    /// leave the plugins inert and the fingerprint byte-identical to the
    /// pre-arming shape (zfb#952).
    ///
    /// The `zfb-render ModuleLoader` (`crates/zfb-render/src/loader.rs`) is
    /// a library/embedder path that does NOT go through `PipelineSpec`; it
    /// stays unarmed by design — embedder callers do not have a
    /// `project_root` at hand.
    pub fn set_build_context_roots(
        &mut self,
        project_root: PathBuf,
        public_dir: PathBuf,
    ) -> &mut Self {
        let segment = format!(
            "build_context_roots;project_root={};public_dir={}",
            normalize_path_lexically(&project_root),
            normalize_path_lexically(&public_dir),
        );
        self.build_context_roots = Some((project_root, public_dir));
        // A second call REPLACES the roots, so the previous segment must
        // go too (mirrors `add_resolve_links`).
        self.config_fingerprint_extras
            .retain(|s| !s.starts_with("build_context_roots;"));
        self.extend_config_fingerprint(segment);
        self
    }

    /// The armed `(project_root, public_dir)` pair, if any (see
    /// [`Pipeline::set_build_context_roots`]).
    #[must_use]
    pub fn build_context_roots(&self) -> Option<(&Path, &Path)> {
        self.build_context_roots
            .as_ref()
            .map(|(root, public)| (root.as_path(), public.as_path()))
    }

    /// Drain the markdown diagnostics context-aware feature plugins
    /// emitted during JSX-emit compiles since the last drain (zfb#944) —
    /// e.g. linkValidation broken-link findings, transclude read
    /// errors. Mirrors [`Pipeline::take_broken_links`]; empty unless
    /// [`Pipeline::set_build_context_roots`] armed context threading.
    pub fn take_markdown_diagnostics(&mut self) -> Vec<MarkdownDiagnostic> {
        std::mem::take(&mut self.markdown_diagnostics)
    }

    /// Number of buffered (not yet drained) markdown diagnostics. The
    /// compile cache snapshots this before a compile so it can slice
    /// off exactly the diagnostics that compile appended (zfb#944,
    /// mirroring [`Pipeline::broken_links_len`]).
    pub(crate) fn markdown_diagnostics_len(&self) -> usize {
        self.markdown_diagnostics.len()
    }

    /// Clone the markdown diagnostics buffered at index `from` onward,
    /// without draining (zfb#944 — see
    /// [`Pipeline::markdown_diagnostics_len`]).
    pub(crate) fn markdown_diagnostics_since(&self, from: usize) -> Vec<MarkdownDiagnostic> {
        self.markdown_diagnostics
            .get(from..)
            .unwrap_or_default()
            .to_vec()
    }

    /// Append diagnostics collected by the JSX-emit path's per-file
    /// context sink (zfb#944). Internal — the emit path flushes its
    /// `CollectingSink` here after the visitor chains run.
    pub(crate) fn extend_markdown_diagnostics(&mut self, diags: Vec<MarkdownDiagnostic>) {
        self.markdown_diagnostics.extend(diags);
    }

    /// Cache-hit replay (zfb#944): re-inject markdown diagnostics stored
    /// with a cached compile so call sites draining
    /// [`Pipeline::take_markdown_diagnostics`] after a hit observe
    /// exactly what the fresh compile produced (the sibling of
    /// [`Pipeline::replay_broken_links`]).
    pub(crate) fn replay_markdown_diagnostics(&mut self, diags: Vec<MarkdownDiagnostic>) {
        self.extend_markdown_diagnostics(diags);
    }

    /// Drain the cross-file fragment-link candidates recorded during
    /// JSX-emit compiles since the last drain (#960 / #977) — the input
    /// of the post-compile cross-file anchor check. Mirrors
    /// [`Pipeline::take_markdown_diagnostics`]; empty unless
    /// [`Pipeline::set_build_context_roots`] armed context threading AND
    /// `linkValidation` is enabled.
    pub fn take_cross_file_link_candidates(&mut self) -> Vec<CrossFileLinkCandidate> {
        std::mem::take(&mut self.cross_file_link_candidates)
    }

    /// Number of buffered (not yet drained) cross-file link candidates.
    /// The compile cache snapshots this before a compile so it can slice
    /// off exactly the candidates that compile appended (mirroring
    /// [`Pipeline::markdown_diagnostics_len`]).
    pub(crate) fn cross_file_link_candidates_len(&self) -> usize {
        self.cross_file_link_candidates.len()
    }

    /// Clone the cross-file link candidates buffered at index `from`
    /// onward, without draining (see
    /// [`Pipeline::cross_file_link_candidates_len`]).
    pub(crate) fn cross_file_link_candidates_since(
        &self,
        from: usize,
    ) -> Vec<CrossFileLinkCandidate> {
        self.cross_file_link_candidates
            .get(from..)
            .unwrap_or_default()
            .to_vec()
    }

    /// Append candidates collected by the JSX-emit path's per-file
    /// context buffer (#977). Internal — the emit path flushes its
    /// per-compile vec here after the visitor chains run.
    pub(crate) fn extend_cross_file_link_candidates(
        &mut self,
        candidates: Vec<CrossFileLinkCandidate>,
    ) {
        self.cross_file_link_candidates.extend(candidates);
    }

    /// Cache-hit replay (#977): re-inject cross-file link candidates
    /// stored with a cached compile so call sites draining
    /// [`Pipeline::take_cross_file_link_candidates`] after a hit observe
    /// exactly what the fresh compile produced (the sibling of
    /// [`Pipeline::replay_markdown_diagnostics`]).
    pub(crate) fn replay_cross_file_link_candidates(
        &mut self,
        candidates: Vec<CrossFileLinkCandidate>,
    ) {
        self.extend_cross_file_link_candidates(candidates);
    }

    /// Drain the per-file heading records surfaced during JSX-emit
    /// compiles since the last drain (#960 / #977) — the lookup side of
    /// the post-compile cross-file anchor check. Mirrors
    /// [`Pipeline::take_markdown_diagnostics`]; empty unless
    /// [`Pipeline::set_build_context_roots`] armed context threading AND
    /// `linkValidation` is enabled.
    pub fn take_file_headings(&mut self) -> Vec<FileHeadings> {
        std::mem::take(&mut self.file_headings)
    }

    /// Whether `markdown.features.linkValidation` was enabled at
    /// construction (#977) — the JSX-emit path consults this to gate the
    /// per-file headings side channel.
    pub(crate) fn link_validation_enabled(&self) -> bool {
        self.link_validation_enabled
    }

    /// Number of buffered (not yet drained) per-file heading records
    /// (the slicing snapshot — see
    /// [`Pipeline::cross_file_link_candidates_len`]).
    pub(crate) fn file_headings_len(&self) -> usize {
        self.file_headings.len()
    }

    /// Clone the per-file heading records buffered at index `from`
    /// onward, without draining (see [`Pipeline::file_headings_len`]).
    pub(crate) fn file_headings_since(&self, from: usize) -> Vec<FileHeadings> {
        self.file_headings.get(from..).unwrap_or_default().to_vec()
    }

    /// Append per-file heading records surfaced by the JSX-emit path
    /// (#977). Internal — the emit path flushes one record per
    /// context-armed compile.
    pub(crate) fn extend_file_headings(&mut self, headings: Vec<FileHeadings>) {
        self.file_headings.extend(headings);
    }

    /// Cache-hit replay (#977): re-inject per-file heading records
    /// stored with a cached compile so call sites draining
    /// [`Pipeline::take_file_headings`] after a hit observe exactly what
    /// the fresh compile produced.
    pub(crate) fn replay_file_headings(&mut self, headings: Vec<FileHeadings>) {
        self.extend_file_headings(headings);
    }

    /// Test seam (zfb#942): push an mdast visitor WITHOUT invalidating
    /// the config fingerprint, so the synthetic-recorder cache tests in
    /// `mdx_jsx_emit` can make a cacheable pipeline record reads during
    /// compile — standing in for the config-derived feature plugins
    /// that will record for real in zfb#944. Production code must use
    /// [`Pipeline::add_mdast_visitor`] (invalidating) or the private
    /// config-derived helpers (see their invalidation-rule docs).
    #[cfg(test)]
    pub(crate) fn push_mdast_visitor_preserving_fingerprint_for_tests(
        &mut self,
        v: Box<dyn MdastVisitor>,
    ) {
        self.push_config_derived_mdast_visitor(v);
    }

    /// New pipeline preloaded with the project's default plugin chain.
    ///
    /// This is the entry point most orchestrator call sites want: it
    /// bundles the five custom hast plugins so headings get permalink
    /// anchors, titled code blocks get a `<div class="code-block-container">`
    /// wrapper plus syntect highlighting, mermaid blocks become
    /// `<div class="mermaid">` containers, and block-level paragraph
    /// images get wrapped in an enlargeable `<figure>` — all without
    /// manual plugin wiring at the call site. Core seeds NO directive
    /// vocabulary; `:::name` → `<Component>` mapping is opt-in via
    /// `features.directives` on [`Pipeline::with_defaults_and_full_config`].
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
    ///    (No directive registry is wired by `with_defaults`: core seeds
    ///    zero directive names. When a `features.directives` map is supplied
    ///    via `with_defaults_and_full_config`, a [`DirectiveRegistry`] runs
    ///    here and folds runs of paragraphs delimited by `:::name` … `:::`
    ///    into a single [`MdxJsxFlowElement`] before the mdast→hast
    ///    conversion.)
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
    /// 5. [`MermaidPlugin`] — replaces `<pre><code class="language-mermaid">`
    ///    blocks with `<div class="mermaid" data-mermaid>…</div>`.
    ///    Must run BEFORE [`SyntectPlugin`] so the latter can identify
    ///    and skip mermaid blocks rather than syntect-highlighting them.
    /// 6. [`SyntectPlugin`] — replaces remaining fenced code blocks
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
    /// `zfb_content::content_bridge::build_snapshot_with_config`.
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
        // Recorded BEFORE any visitor wiring, so the two secondary parse
        // sites see the project's setting rather than a branch outcome
        // (zfb#2390 — see the field doc).
        p.cjk_friendly = cjk_friendly;
        // mdast phase. Config-derived wiring — pushed through the
        // non-invalidating helpers (invalidation rule — see
        // `push_config_derived_mdast_visitor`).
        if cjk_friendly {
            // GFM autolink-literal CJK boundary fix (zfb#1105). Runs BEFORE
            // CjkFriendlyPlugin so it sees the single-Text-child autolink
            // shape — emphasis retokenisation would otherwise split markers
            // inside an over-consumed autolink's text (e.g.
            // `https://x.com**重要。**`) and hide it from the boundary pass.
            // Only fires when bare-URL autolinking is on (no autolink Link
            // nodes exist otherwise, and gating on it keeps explicit links
            // untouched). Both `cjk_friendly` and `autolink_literal` are
            // already in the config fingerprint, so this stays
            // non-invalidating.
            if resolved.autolink_literal {
                // Note this plugin is a visitor at chain index 0, so it never
                // sees a subtree parsed LATER in the chain by transclude or
                // the directive registry; `p.cjk_friendly` (set above, before
                // any wiring) is how those two secondary parse sites apply the
                // same fix to their own output (zfb#2390).
                p.push_config_derived_mdast_visitor(Box::new(CjkAutolinkBoundaryPlugin::new()));
            }
            p.push_config_derived_mdast_visitor(Box::new(CjkFriendlyPlugin::new()));
        }
        // No directive registry here: core seeds zero directive names. Callers
        // that want `:::name` → `<Component>` go through
        // `with_defaults_and_full_config` with a `features.directives` map.
        // hast phase — ordering rationale lives in the doc comment above.
        p.push_config_derived_hast_visitor(Box::new(HeadingLinksPlugin::new()));
        p.push_config_derived_hast_visitor(Box::new(CodeTitlePlugin::new()));
        p.push_config_derived_hast_visitor(Box::new(MermaidPlugin::new()));
        // No class-mode branch here: this legacy chain (`with_defaults` /
        // `with_theme*`) has no config-driven `codeHighlight.mode` input at
        // all — it is used by direct callers (tests, embedders), never by
        // `PipelineSpec::build_pipeline`. Class emission (zfb#1532) lives on
        // the full-config path via
        // `with_defaults_and_full_config_class` → the class-emission arm in
        // `with_defaults_and_full_config_inner`. If a legacy class-mode entry
        // point is ever needed, add a `with_theme_class_mode`-style
        // constructor rather than branching here.
        let syntect = if let Some(t) = theme {
            SyntectPlugin::new(highlighter).with_theme(t)
        } else {
            SyntectPlugin::new(highlighter)
        };
        // Nested-code render chain (#2207): store the config needed to
        // reconstruct the code-block chain for fences nested inside MDX
        // JSX bodies. The legacy chain always wires MermaidPlugin and
        // never wires code-enrichment.
        p.nested_code_chain_spec = Some(NestedCodeChainSpec {
            mermaid: true,
            syntect: syntect.clone(),
            code_enrichment: None,
        });
        p.push_config_derived_hast_visitor(Box::new(syntect));
        // Fingerprint the construction config LAST: the interim descriptor
        // from `with_resolved_gfm_constructs` only covers the bare
        // constructor; this final assignment replaces it with the
        // legacy-defaults descriptor covering every knob this chain
        // consumed (constructor obligation of the invalidation rule).
        // `defaults` vs `full` prefix keeps this chain (which
        // always wires the core MermaidPlugin) distinct from the
        // feature-aware `with_defaults_and_full_config` chain even when
        // the shared knobs coincide.
        let base = themes_dir_fingerprint_segment(themes_dir).map(|themes_seg| {
            format!(
                "{FINGERPRINT_VERSION};defaults;theme={theme:?};{gfm};{themes_seg};cjk={cjk_friendly}",
                gfm = gfm_fingerprint_segment(resolved),
            )
        });
        p.set_config_fingerprint_base(base);
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
    /// 3. `DirectiveRegistry` — directive transforms must fold `:::name` runs
    ///    before mdast→hast conversion. Activated by the generic `directives`
    ///    map (zero default names).
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
    pub fn with_defaults_and_features(features: &zfb_md_extras::MarkdownFeaturesConfig) -> Self {
        // Default theme, conservative GFM, no themes_dir, CJK on — the shape
        // Wave 2/3 hard-coded here. The chain itself now lives in
        // [`Pipeline::with_defaults_and_full_config`] so the bundler, snapshot
        // walker, and dev loader all share one feature-aware code path.
        Self::with_defaults_and_full_config(
            None,
            ResolvedGfmConstructs::CONSERVATIVE,
            None,
            true,
            false,
            Some(features),
        )
        .expect("with_defaults_and_features passes no themes_dir — cannot fail")
    }

    /// Feature-aware default constructor — the single entry point the bundler,
    /// snapshot walker, and dev loader use to honour `markdown.features` from
    /// `zfb.config.ts`.
    ///
    /// The pipeline is the always-on Core chain (CJK-friendly emphasis,
    /// heading-links, code-title, syntect) plus exactly the opt-in
    /// `zfb-md-extras` feature visitors whose `features.*` flags are enabled
    /// ([`register_features`] / [`register_post_syntect_features`]).
    ///
    /// `features = None` is treated as an **empty** feature set: the
    /// former-Core framework features (`mermaid`, `directives`,
    /// `heading_marker_toc`) are **off**. This is the
    /// post-epic opt-in default documented in the v0.1.0-next.12 changelog
    /// (#583): a default `zfb.config.ts` build omits them, and users opt in via
    /// `markdown.features.*`. The legacy [`Pipeline::with_defaults`] /
    /// `with_defaults_and_theme*` constructors retain the pre-epic Core hast
    /// chain (no directive vocabulary) for direct callers (tests, embedders).
    ///
    /// All three production pipelines MUST thread the SAME `features` value
    /// through this one constructor so the snapshot ↔ bundler `content_hash`
    /// stays byte-identical (see `crates/zfb-content/src/content_bridge.rs`).
    ///
    /// `theme`, `resolved`, `themes_dir`, `cjk_friendly`, and `hard_breaks`
    /// carry the same meaning as on
    /// [`Pipeline::with_defaults_and_theme_and_gfm_and_themes_dir`] (and
    /// `zfb::config::resolve_hard_breaks`). Returns `Err` only when
    /// `themes_dir` is `Some` and a `.tmTheme` file fails to load.
    pub fn with_defaults_and_full_config(
        theme: Option<&str>,
        resolved: ResolvedGfmConstructs,
        themes_dir: Option<&Path>,
        cjk_friendly: bool,
        hard_breaks: bool,
        features: Option<&zfb_md_extras::MarkdownFeaturesConfig>,
    ) -> Result<Self, crate::syntect_highlight::HighlightError> {
        Self::with_defaults_and_full_config_inner(
            HiEmit::Single(theme),
            resolved,
            themes_dir,
            cjk_friendly,
            hard_breaks,
            features,
        )
    }

    /// Like [`Pipeline::with_defaults_and_full_config`] but with an explicit
    /// dual-theme pair.
    ///
    /// Constructs the `SyntectPlugin` in dual-theme mode (CSS custom properties
    /// `--shiki-light`/`--shiki-dark` instead of inline `color:`).
    ///
    /// Both theme names must be SYNTECT names — NOT Shiki names like `"dracula"`.
    pub fn with_defaults_and_full_config_dual(
        resolved: ResolvedGfmConstructs,
        themes_dir: Option<&Path>,
        cjk_friendly: bool,
        hard_breaks: bool,
        features: Option<&zfb_md_extras::MarkdownFeaturesConfig>,
        theme_light: &str,
        theme_dark: &str,
    ) -> Result<Self, crate::syntect_highlight::HighlightError> {
        Self::with_defaults_and_full_config_inner(
            HiEmit::Dual(theme_light, theme_dark),
            resolved,
            themes_dir,
            cjk_friendly,
            hard_breaks,
            features,
        )
    }

    /// Like [`Pipeline::with_defaults_and_full_config`] but in class-emission
    /// mode (`codeHighlight.mode: "class"`, Highlight Tokens epic zfb#1528).
    ///
    /// Wires the `SyntectPlugin` in class-emission mode
    /// ([`SyntectPlugin::with_class_mode`]): every token becomes a
    /// `<span class="…">` role class (default `{class_prefix}{short_name}`, or
    /// the `role_classes` override) with NO inline colour, and the `<pre>` is
    /// classed `{class_prefix}root`. The `class_prefix` / `role_classes` pair
    /// is also encoded into the compile-cache fingerprint segment (see the
    /// class-emission arm inside [`Self::with_defaults_and_full_config_inner`])
    /// so a class-mode config can never alias an inline/dual entry and
    /// `classPrefix`/`roleClasses` edits correctly invalidate the cache.
    pub fn with_defaults_and_full_config_class(
        resolved: ResolvedGfmConstructs,
        themes_dir: Option<&Path>,
        cjk_friendly: bool,
        hard_breaks: bool,
        features: Option<&zfb_md_extras::MarkdownFeaturesConfig>,
        class_prefix: &str,
        role_classes: &BTreeMap<String, String>,
    ) -> Result<Self, crate::syntect_highlight::HighlightError> {
        Self::with_defaults_and_full_config_inner(
            HiEmit::Class(class_prefix, role_classes),
            resolved,
            themes_dir,
            cjk_friendly,
            hard_breaks,
            features,
        )
    }

    /// Internal shared builder for the single / dual / class full-config
    /// constructors.
    fn with_defaults_and_full_config_inner(
        hi_emit: HiEmit<'_>,
        resolved: ResolvedGfmConstructs,
        themes_dir: Option<&Path>,
        cjk_friendly: bool,
        hard_breaks: bool,
        features: Option<&zfb_md_extras::MarkdownFeaturesConfig>,
    ) -> Result<Self, crate::syntect_highlight::HighlightError> {
        // `markdown.features` absent → empty feature set (post-epic opt-in
        // default, #583 / #586): the former-Core framework features
        // (mermaid, directives, heading_marker_toc) are
        // OFF. The legacy `with_defaults*` constructors / `build_defaults`
        // retain the pre-epic Core hast chain (no directive vocabulary) for
        // direct callers, but the bundler/snapshot/dev pipelines route here.
        let empty;
        let features = match features {
            Some(f) => f,
            None => {
                empty = zfb_md_extras::MarkdownFeaturesConfig::default();
                &empty
            }
        };

        // Feature-aware chain. Mirrors `build_defaults` for the framework
        // plugins that are ALWAYS on (cjk, heading-links, code-title, syntect)
        // but routes the opt-in plugins (mermaid, directives, …)
        // through `register_features`.
        //
        // Visitor ordering contract (see doc comment on
        // `with_defaults_and_features`):
        //   mdast: CjkFriendlyPlugin → [features mdast] → [directives]
        //   hast:  HeadingLinksPlugin → CodeTitlePlugin → [features hast] →
        //          SyntectPlugin → [features post-syntect]
        let mut highlighter = Highlighter::new();
        if let Some(dir) = themes_dir {
            highlighter.load_themes_from_dir(dir)?;
        }
        // Build-start validation (dual #1067, single #1070): a misspelled or
        // unloaded theme name — the single `theme`, or either of the dual
        // `themeLight`/`themeDark` — must surface the documented `UnknownTheme`
        // error here rather than silently rendering unhighlighted blocks. Both
        // the single `highlight_lines` and the dual `highlight_lines_dual`
        // per-block calls swallow their `Err` inside `SyntectPlugin`, so the
        // only place a bad name can be rejected loudly is at construction,
        // after any `themesDir` themes are loaded. This enforces the
        // `CodeHighlightConfig` doc promise — "unknown theme names are rejected
        // with a clear error rather than silently falling back" — for BOTH
        // modes. (config.rs only validates name *presence* / dual-pair
        // completeness; theme-name existence depends on `themesDir`, which is
        // not known until here.)
        let required_themes: Vec<&str> = match hi_emit {
            HiEmit::Single(theme) => theme.into_iter().collect(),
            HiEmit::Dual(light, dark) => vec![light, dark],
            HiEmit::Class(_, _) => Vec::new(),
        };
        if !required_themes.is_empty() {
            let names = highlighter.theme_names();
            for name in required_themes {
                if !names.iter().any(|n| n == name) {
                    return Err(crate::syntect_highlight::HighlightError::UnknownTheme(
                        name.to_string(),
                    ));
                }
            }
        }
        let highlighter = Arc::new(highlighter);
        let mut p = Self::with_resolved_gfm_constructs(resolved);
        // Recorded BEFORE any visitor wiring, so the two secondary parse
        // sites see the project's setting rather than a branch outcome
        // (zfb#2390 — see the field doc).
        p.cjk_friendly = cjk_friendly;
        // Same rationale as `p.cjk_friendly` above, for `markdown.hardBreaks`
        // (zfb#2398 — see the field doc).
        p.hard_breaks = hard_breaks;
        // mdast phase — CjkFriendlyPlugin honours the cjk_friendly toggle.
        // Config-derived wiring throughout this constructor is pushed via
        // the non-invalidating helpers (invalidation rule — see
        // `push_config_derived_mdast_visitor`).
        if cjk_friendly {
            // GFM autolink-literal CJK boundary fix (zfb#1105) — runs BEFORE
            // CjkFriendlyPlugin; see the twin wiring in `build_defaults` for
            // the ordering / gating / fingerprint rationale.
            if resolved.autolink_literal {
                // Note this plugin is a visitor at chain index 0, so it never
                // sees a subtree parsed LATER in the chain by transclude or
                // the directive registry; `p.cjk_friendly` (set above, before
                // any wiring) is how those two secondary parse sites apply the
                // same fix to their own output (zfb#2390).
                p.push_config_derived_mdast_visitor(Box::new(CjkAutolinkBoundaryPlugin::new()));
            }
            p.push_config_derived_mdast_visitor(Box::new(CjkFriendlyPlugin::new()));
        }
        // HardBreaksPlugin runs AFTER CjkFriendlyPlugin so CJK emphasis
        // re-tokenisation sees intact Text nodes first (emphasis markers are
        // resolved before soft line breaks are split). Default is false.
        if hard_breaks {
            p.push_config_derived_mdast_visitor(Box::new(HardBreaksPlugin::new()));
        }
        // hast phase — HeadingLinksPlugin and CodeTitlePlugin are always on.
        // HeadingLinksPlugin honours `features.headingIds.strategy` (zfb#871);
        // the resolved strategy is also stored on the pipeline so the
        // JSX-emit path (`collect_headings`) mirrors the same scheme.
        let strategy = zfb_md_extras::heading_id_strategy(&features.heading_ids);
        p.set_heading_id_strategy(strategy);
        p.push_config_derived_hast_visitor(Box::new(HeadingLinksPlugin::with_strategy(strategy)));
        p.push_config_derived_hast_visitor(Box::new(CodeTitlePlugin::new()));
        // One read-recorder per pipeline (zfb#942/#944), created iff a
        // filesystem-reading feature plugin will be wired: the plugins
        // receive clones at construction (inside
        // `register_features_config_derived`) and the SAME `Arc` goes on
        // the pipeline below, so the compile-cache choke point can scope
        // the recorded reads per compile.
        let filesystem_dependent_feature = features.transclude.is_some()
            || features.image_dimensions.is_some()
            || features.link_validation.is_some();
        let read_recorder = filesystem_dependent_feature.then(|| Arc::new(ReadRecorder::new()));
        // Cross-file anchor side channels (#960 / #977): the per-file
        // headings channel only carries data when `linkValidation` is
        // enabled — it exists solely for the post-compile cross-file
        // anchor check, so configs without linkValidation record NOTHING
        // (the candidates channel is implicitly gated the same way:
        // only `LinkValidationPlugin` writes to it). The flag is a pure
        // function of `features` — already covered by the canonical
        // features JSON in the fingerprint below — so it cannot split
        // cache keys or desynchronise store/replay.
        p.link_validation_enabled = features.link_validation.is_some();
        // Single call-path from zfb-content into zfb-md-extras: adds the opt-in
        // visitors in the correct phase/position (before SyntectPlugin for
        // mermaid; after for post-syntect). The `_config_derived` variant
        // does not invalidate — the final base descriptor below covers the
        // whole `features` value.
        register_features_config_derived(&mut p, features, read_recorder.as_ref());
        // SyntectPlugin MUST be added AFTER register_features so pre-syntect
        // extras visitors (mermaid, …) run first.
        //
        let syntect = match hi_emit {
            HiEmit::Class(class_prefix, role_classes) => {
                // Class-emission mode (zfb#1532): every token becomes a
                // `<span class="…">` role class (default `{prefix}{short_name}`,
                // or the `roleClasses` override) with NO inline colour; the
                // `<pre>` is classed `{prefix}root`. config.rs validation forbids
                // setting `theme`/`themeLight`/`themeDark` alongside class mode,
                // so this variant never coexists with single/dual theme input.
                SyntectPlugin::new(highlighter).with_class_mode(class_prefix, role_classes.clone())
            }
            HiEmit::Dual(light, dark) => {
                SyntectPlugin::new(highlighter).with_dual_themes(light, dark)
            }
            HiEmit::Single(Some(theme)) => SyntectPlugin::new(highlighter).with_theme(theme),
            HiEmit::Single(None) => SyntectPlugin::new(highlighter),
        };
        // Nested-code render chain (#2207): store the config needed to
        // reconstruct the code-block chain for fences nested inside MDX
        // JSX bodies / directives. Mermaid and code-enrichment mirror the
        // exact feature gating used for the live chain above
        // (`register_features_config_derived` /
        // `register_post_syntect_features_config_derived`).
        p.nested_code_chain_spec = Some(NestedCodeChainSpec {
            mermaid: zfb_md_ast::feature_enabled(&features.mermaid),
            syntect: syntect.clone(),
            code_enrichment: features.code_enrichment.clone(),
        });
        p.push_config_derived_hast_visitor(Box::new(syntect));
        // Post-syntect extras visitors operate on the per-line
        // <span class="line"> structure SyntectPlugin emits.
        register_post_syntect_features_config_derived(&mut p, features);
        // Fingerprint the construction config LAST — the interim
        // descriptor from `with_resolved_gfm_constructs` only covers the
        // bare constructor; this final assignment replaces it with the
        // full-config descriptor covering every knob this chain consumed
        // (constructor obligation of the invalidation rule — see
        // `push_config_derived_mdast_visitor`).
        //
        // Plugins that read OTHER files at compile time (`transclude`,
        // `imageDimensions`, `linkValidation`) no longer bail out of the
        // fingerprint (the pre-#944 `filesystem_dependent_feature` gate):
        // their CONFIG is covered by the canonical features JSON below,
        // and their per-file reads are covered by the read-recorder wired
        // above — `compile_mdx_to_jsx_module_cached` stores the recorded
        // reads as a `DependencyManifest` with every entry and re-probes
        // them before honouring a hit, so a cached entry can no longer go
        // stale when a referenced file changes between dev ticks.
        //
        // The mode is encoded EXPLICITLY in the fingerprint segment
        // (`code_highlight=single(theme=…)` vs `code_highlight=dual(light=…,dark=…)`
        // vs `code_highlight=class(prefix=…,roles=…)`) so a single-theme
        // config can never alias a dual-theme or class-mode entry in the
        // compile cache, regardless of how the theme names compare (codex
        // #5). `role_classes` is a `BTreeMap` so its `Debug` output is
        // deterministically key-sorted — required for fingerprint
        // stability across two constructions of an equal map.
        // IMPORTANT: the single-mode segment must be BYTE-IDENTICAL to the
        // pre-dual form so existing warm caches are not invalidated on upgrade.
        let code_highlight_seg = match hi_emit {
            HiEmit::Class(prefix, role_classes) => {
                format!("code_highlight=class(prefix={prefix:?},roles={role_classes:?})")
            }
            HiEmit::Dual(light, dark) => {
                format!("code_highlight=dual(light={light:?},dark={dark:?})")
            }
            HiEmit::Single(theme) => {
                // Single mode: reproduce the exact pre-dual descriptor string so
                // fingerprints are unchanged for all existing single-theme configs.
                format!("theme={theme:?}")
            }
        };
        let base = match (
            themes_dir_fingerprint_segment(themes_dir),
            features_fingerprint_segment(features),
        ) {
            (Some(themes_seg), Some(features_seg)) => Some(format!(
                "{FINGERPRINT_VERSION};full;{code_highlight_seg};{gfm};{themes_seg};cjk={cjk_friendly};hard_breaks={hard_breaks};{features_seg}",
                gfm = gfm_fingerprint_segment(resolved),
            )),
            _ => None,
        };
        p.set_config_fingerprint_base(base);
        // Attach AFTER the base descriptor: `set_read_recorder` is
        // fingerprint-neutral, but the recorder must survive the
        // `set_config_fingerprint_base` extras reset regardless of order
        // — it lives in its own field. One recorder per pipeline.
        if let Some(recorder) = read_recorder {
            p.set_read_recorder(recorder);
        }
        Ok(p)
    }

    /// Append an mdast visitor; visitors run in insertion order.
    ///
    /// Appending an arbitrary trait-object visitor makes the pipeline
    /// **uncacheable** ([`Pipeline::config_fingerprint`] → `None`): a
    /// `Box<dyn MdastVisitor>` cannot be fingerprinted from outside, so
    /// the MDX compile cache could not tell two manually-wired
    /// pipelines apart.
    ///
    /// Invalidation rule (zfb#913): manual external pushes — this method,
    /// [`Pipeline::add_hast_visitor`], and the public [`register_features`]
    /// / [`register_post_syntect_features`] helpers — MUST invalidate;
    /// internal pushes that derive purely from already-fingerprinted
    /// config MUST NOT (they go through the private
    /// `push_config_derived_*` helpers — see there for the full rule).
    pub fn add_mdast_visitor(&mut self, v: Box<dyn MdastVisitor>) -> &mut Self {
        self.mdast_visitors.push(v);
        self.invalidate_config_fingerprint();
        self
    }

    /// Append a hast visitor; visitors run in insertion order.
    ///
    /// Same cacheability contract and invalidation rule as
    /// [`Pipeline::add_mdast_visitor`]: the pipeline becomes uncacheable.
    pub fn add_hast_visitor(&mut self, v: Box<dyn HastVisitor>) -> &mut Self {
        self.hast_visitors.push(v);
        self.invalidate_config_fingerprint();
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
    /// mdast visitors (i.e. after the directives step) so the source
    /// map lookup sees the final mdast link nodes.
    pub fn apply_mdast_visitors(&mut self, node: &mut MdastNode) {
        for v in &mut self.mdast_visitors {
            v.set_secondary_parse_target(SecondaryParseTarget::Jsx);
            v.visit(node);
        }
        // Apply ResolveLinksPlugin last in the mdast phase (after the
        // directives step) when wired. See field doc.
        if let Some(p) = self.resolve_links.as_mut() {
            p.visit(node);
            // Context-free compiles cannot contribute to the build-wide
            // fragment check. Drain so metadata from this document cannot
            // leak into a later context-armed compile on the same pipeline.
            let _ = p.take_resolved_fragment_links();
        }
        // JSX-nested mutation stubs (zfb#2247), applied LAST in the mdast
        // phase — after resolve_links. See the field docs on
        // `jsx_nested_image_dimensions` / `jsx_nested_external_links` for
        // the ordering rationale. Context-free path: both stubs receive
        // context-free `visit` calls.
        if let Some(v) = self.jsx_nested_image_dimensions.as_mut() {
            v.visit(node);
        }
        if let Some(v) = self.jsx_nested_external_links.as_mut() {
            v.visit(node);
        }
    }

    /// Run only the hast visitor chain against an externally-built
    /// hast tree.
    ///
    /// Mirror of [`Pipeline::apply_mdast_visitors`], added for #121 so
    /// the JSX emit path can detour through hast — `mdast → hast →
    /// hast visitors → JSX emit` — and pick up the project's hast-phase
    /// plugins (heading-links, code-title, mermaid,
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
    /// Stateless visitors (code-title, mermaid, syntect,
    /// strip-md-ext) provide the default no-op implementation of
    /// [`HastVisitor::reset`], so calling this unconditionally is safe.
    ///
    /// [`HeadingLinksPlugin`]: crate::plugins::HeadingLinksPlugin
    pub fn reset_per_entry(&mut self) {
        for v in &mut self.hast_visitors {
            v.reset();
        }
    }

    /// Strip autolink literals that markdown-rs created inside a link label
    /// (zfb#2388), which would otherwise render as nested `<a>` — invalid
    /// HTML.
    ///
    /// Applied here rather than as a registered mdast visitor so it covers
    /// every pipeline regardless of constructor (the visitor chain is only
    /// wired by the `with_defaults*` family) and is guaranteed to run before
    /// any visitor observes the malformed tree. Gated on the construct that
    /// produces the nesting, so a pipeline with autolink literals off keeps
    /// byte-identical output. See [`plugins::nested_link`] for the full
    /// rationale.
    ///
    /// [`plugins::nested_link`]: crate::plugins::nested_link
    fn normalize_nested_links(&self, mdast: &mut MdastNode) {
        if self.parse_options.constructs.gfm_autolink_literal {
            crate::plugins::unwrap_nested_links(mdast);
        }
    }

    /// Parse `input` to mdast, run mdast visitors, transform to hast, run
    /// hast visitors. Returns the resulting hast root.
    ///
    /// Deliberately does NOT apply `jsx_nested_image_dimensions` /
    /// `jsx_nested_external_links` (zfb#2247): this HTML path feeds
    /// `mdast_to_hast`, whose `reconstruct_jsx` fallback (below)
    /// lossily stringifies markdown nested in JSX (`[label](url)` →
    /// text `label`) — no nested `<a>`/`<img>` elements exist here to
    /// treat, and running the mutators would change this path's output
    /// (a synthesized JSX element gets structurally reconstructed
    /// instead of stringified, for treated nodes only). Applying them
    /// only in `apply_mdast_visitors*` (the MDX compile path, which
    /// keeps `MdxJsxTextElement` nodes structured all the way to
    /// `JsxEmitter`) keeps this path's output byte-identical.
    ///
    /// # Errors
    /// Returns [`PipelineError::Parse`] if markdown-rs rejects the input.
    pub fn run(&mut self, input: &str) -> Result<HastNode, PipelineError> {
        let mut mdast = markdown::to_mdast(input, &self.parse_options)
            .map_err(|m| PipelineError::Parse(m.to_string()))?;
        self.normalize_nested_links(&mut mdast);

        for v in &mut self.mdast_visitors {
            v.set_secondary_parse_target(SecondaryParseTarget::Html);
            v.visit(&mut mdast);
        }

        let mut hast = mdast_to_hast(&mdast);

        for v in &mut self.hast_visitors {
            v.visit(&mut hast);
        }

        Ok(hast)
    }

    /// Like [`Pipeline::run`] but threads a [`BuildContext`] through both
    /// the mdast and hast visitor chains.
    ///
    /// Mdast visitors that override [`MdastVisitor::visit_with_context`]
    /// (e.g. the wave-6 `TranscludePlugin`) receive the context so they can
    /// resolve source-relative file paths. Hast visitors that override
    /// [`HastVisitor::visit_with_context`] receive context for the registry
    /// and diagnostics sink.
    ///
    /// All other visitors fall back to the no-context `visit` call via the
    /// default trait implementations, so the output is **byte-identical** to
    /// [`Pipeline::run`] when no context-aware visitors are registered.
    ///
    /// Also deliberately does NOT apply `jsx_nested_image_dimensions` /
    /// `jsx_nested_external_links` (zfb#2247) — same reasoning as
    /// [`Pipeline::run`]'s doc comment (this is the other HTML-path
    /// entry point, sharing its `reconstruct_jsx` fallback).
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
        self.normalize_nested_links(&mut mdast);

        for v in &mut self.mdast_visitors {
            v.set_secondary_parse_target(SecondaryParseTarget::Html);
            v.visit_with_context(&mut mdast, ctx);
        }
        if let Some(p) = self.resolve_links.as_mut() {
            p.visit(&mut mdast);
        }
        self.promote_resolved_fragment_links(ctx);
        // JSX-nested link collection (zfb#2184) MUST stay after the
        // resolve_links application — see the field doc on
        // `jsx_nested_link_collector`.
        if let Some(c) = self.jsx_nested_link_collector.as_mut() {
            c.visit(&mut mdast);
        }

        let mut hast = mdast_to_hast(&mdast);

        for v in &mut self.hast_visitors {
            v.visit_with_context(&mut hast, ctx);
        }

        Ok(hast)
    }

    /// Run only the mdast visitor chain with build context against an
    /// externally-parsed mdast tree.
    ///
    /// Parallel to [`Pipeline::apply_mdast_visitors`] but threads the context
    /// through each visitor so wave-6 mdast plugins (e.g. `TranscludePlugin`)
    /// can access source path and project root for file resolution.
    pub fn apply_mdast_visitors_with_context(
        &mut self,
        node: &mut MdastNode,
        ctx: &mut BuildContext<'_>,
    ) {
        for v in &mut self.mdast_visitors {
            v.set_secondary_parse_target(SecondaryParseTarget::Jsx);
            v.visit_with_context(node, ctx);
        }
        if let Some(p) = self.resolve_links.as_mut() {
            p.visit(node);
        }
        self.promote_resolved_fragment_links(ctx);
        // JSX-nested link collection (zfb#2184) MUST stay after the
        // resolve_links application — see the field doc on
        // `jsx_nested_link_collector`. This hook is what covers the MDX
        // compile path (`mdx_jsx_emit` calls this method).
        if let Some(c) = self.jsx_nested_link_collector.as_mut() {
            c.visit(node);
        }
        // JSX-nested mutation stubs (zfb#2247), applied LAST in the mdast
        // phase — after the collector block above. See the field docs on
        // `jsx_nested_image_dimensions` / `jsx_nested_external_links` for
        // the ordering rationale (both need collector output preserved;
        // neither needs resolve_links). `jsx_nested_image_dimensions`
        // takes context (mirrors its hast sibling, which requires
        // `visit_with_context` to resolve paths); `jsx_nested_external_links`
        // works context-free (mirrors its hast sibling's availability).
        if let Some(v) = self.jsx_nested_image_dimensions.as_mut() {
            v.visit_with_context(node, ctx);
        }
        if let Some(v) = self.jsx_nested_external_links.as_mut() {
            v.visit(node);
        }
    }

    /// Move trusted source-map identities captured by `ResolveLinksPlugin`
    /// into the same cache-replayed channel used by ordinary relative
    /// cross-file fragment links. The rewritten URL is intentionally never
    /// parsed back into file space.
    fn promote_resolved_fragment_links(&mut self, ctx: &mut BuildContext<'_>) {
        let resolved = self
            .resolve_links
            .as_mut()
            .map(ResolveLinksPlugin::take_resolved_fragment_links)
            .unwrap_or_default();
        let Some(severity) = self.resolved_link_fragment_severity else {
            return;
        };
        let (Some(source_path), Some(out)) = (
            ctx.source_path.as_ref(),
            ctx.cross_file_links.as_deref_mut(),
        ) else {
            return;
        };
        let occurrence_base = out.len();
        out.extend(
            resolved
                .into_iter()
                .enumerate()
                .map(|(offset, link)| CrossFileLinkCandidate {
                    source_path: source_path.clone(),
                    target_path: zfb_types::normalize_path_lexical(&link.target_path),
                    fragment: link.fragment,
                    raw_href: link.raw_href,
                    occurrence_index: occurrence_base + offset,
                    severity,
                }),
        );
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
/// - mdast visitors: after `CjkFriendlyPlugin` and BEFORE the directives step.
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
///
/// # Cacheability
///
/// Calling this helper manually after construction makes the pipeline
/// **uncacheable** ([`Pipeline::config_fingerprint`] → `None`), even with an
/// empty feature set: a post-construction registration wires visitors the
/// construction-time descriptor knows nothing about (invalidation rule —
/// manual external pushes MUST invalidate; see
/// [`Pipeline::add_mdast_visitor`]). The feature-aware constructor routes
/// through the non-invalidating internal variant instead and covers the
/// whole `features` value in its final base descriptor.
pub fn register_features(p: &mut Pipeline, features: &zfb_md_extras::MarkdownFeaturesConfig) {
    // No read-recorder on this path: the manual registration makes the
    // pipeline uncacheable anyway, so there is no dependency manifest to
    // feed (the compile cache never stores entries for it).
    register_features_config_derived(p, features, None);
    // Keep the nested-code render chain (#2207) in lockstep with the
    // live hast chain: a manual post-construction registration can add
    // the mermaid step to a pipeline whose constructor-stored spec
    // predates it — without this sync, a top-level mermaid fence would
    // render the mermaid div while a JSX-nested one kept the raw
    // fallback. Constructors never route through this wrapper (they
    // call the `_config_derived` inner directly and compute the spec
    // themselves), so this is the single sync point for the manual path.
    if zfb_md_ast::feature_enabled(&features.mermaid) {
        if let Some(spec) = p.nested_code_chain_spec.as_mut() {
            spec.mermaid = true;
        }
    }
    p.invalidate_config_fingerprint();
}

/// Internal, non-invalidating implementation of [`register_features`] —
/// called from [`Pipeline::with_defaults_and_full_config`], whose final base
/// descriptor covers the whole `features` value (invalidation rule — see
/// `Pipeline::push_config_derived_mdast_visitor`).
///
/// `read_recorder` (zfb#944): when `Some`, the filesystem-reading feature
/// plugins (transclude, imageDimensions, linkValidation) receive a clone
/// at construction so every external read they perform is reported for
/// the compile cache's dependency manifest. The caller owns putting the
/// SAME `Arc` on the pipeline via [`Pipeline::set_read_recorder`].
fn register_features_config_derived(
    p: &mut Pipeline,
    features: &zfb_md_extras::MarkdownFeaturesConfig,
    read_recorder: Option<&Arc<ReadRecorder>>,
) {
    // Wave 3 (#570): conditionally wire the four opt-in framework features.
    //
    // Ordering contract (MUST match the doc comment on with_defaults_and_features):
    //   mdast phase:
    //     - directives — MUST run BEFORE the syntect / hast phase.
    //       Inserted here (after CjkFriendlyPlugin that was added in the caller).
    //   hast phase (all run BEFORE SyntectPlugin which is appended by the caller
    //   after register_features returns):
    //     - mermaid — MUST run BEFORE SyntectPlugin so syntect can skip mermaid
    //       blocks. The `data-mermaid` div shape is not a `<pre>`; syntect
    //       ignores it automatically once it has been replaced.
    //   heading_marker_toc is wired AFTER headings are slugified by
    //   HeadingLinksPlugin. Since HeadingLinksPlugin was added first in the
    //   caller's hast chain, any hast visitor appended here runs after it.

    use zfb_md_ast::{
        directives_enabled, feature_enabled, heading_marker_toc_enabled, reading_time_enabled,
    };

    // ── mdast phase ────────────────────────────────────────────────────────
    // transclude MUST run FIRST in the mdast phase — before code_tabs,
    // directives, and all other mdast visitors — so that included
    // content is spliced into the tree and then processed by subsequent
    // visitors normally. The TranscludePlugin implements
    // `MdastVisitor::visit_with_context` and requires a BuildContext
    // (source_path + project_root) to resolve file paths. When the pipeline
    // is driven via `run_with_context`, the context is automatically threaded.
    // Both secondary parse sites below (transclude here, the directive
    // registry further down) call `markdown::to_mdast` themselves from
    // inside the visitor chain. Before zfb#2390 both hardcoded a bare
    // `ParseOptions::mdx()`, whose `Constructs::mdx()` inherits
    // `Constructs::default()` — every `gfm_*` flag false — so the same
    // markdown rendered differently depending on whether it was written
    // inline or reached through one of these paths. They now inherit the
    // pipeline's own resolved set, plus the post-parse normalisations
    // that set makes mandatory (the pipeline applies those to its own
    // parse before the chain starts, which is exactly why a subtree
    // parsed later cannot rely on them).
    let secondary_parse_gfm = p.gfm_constructs;
    let secondary_parse_cjk_friendly = p.cjk_friendly;
    // `markdown.hardBreaks` (zfb#2398), threaded to the same two secondary
    // parse sites for the same reason as `secondary_parse_cjk_friendly`
    // above: `HardBreaksPlugin` is a visitor at a fixed chain index and
    // never sees a subtree parsed later, from inside the chain, by
    // transclude or the directive registry.
    let secondary_parse_hard_breaks = p.hard_breaks;

    if let Some(cfg) = &features.transclude {
        let mut plugin = zfb_md_extras::transclude::TranscludePlugin::new(cfg.clone())
            .with_gfm(secondary_parse_gfm, secondary_parse_cjk_friendly)
            .with_hard_breaks(secondary_parse_hard_breaks);
        if let Some(recorder) = read_recorder {
            plugin = plugin.with_recorder(Arc::clone(recorder));
        }
        p.push_config_derived_mdast_visitor(Box::new(plugin));
    }

    // code_tabs MUST run BEFORE the directives step and github_alerts so that
    // `:::code-group` opener paragraphs are consumed before the directive
    // registry or alert scanner inspects them. The CodeTabsPlugin looks for
    // the literal `:::code-group` opener and the closing `:::` separator so
    // it must see raw paragraph nodes, not already-rewritten JSX elements.
    if feature_enabled(&features.code_tabs) {
        p.push_config_derived_mdast_visitor(Box::new(
            zfb_md_extras::code_tabs::CodeTabsPlugin::new(),
        ));
    }

    // github_alerts MUST run BEFORE the directives step so both features can
    // coexist: alert blockquotes are rewritten to MdxJsxFlowElement first,
    // then the directives pass handles `:::directive` syntax separately.
    if feature_enabled(&features.github_alerts) {
        p.push_config_derived_mdast_visitor(Box::new(
            zfb_md_extras::github_alerts::GithubAlertsPlugin::new(),
        ));
    }

    if reading_time_enabled(&features.reading_time) {
        let wpm = features
            .reading_time
            .as_ref()
            .and_then(zfb_md_ast::ReadingTimeFeature::wpm)
            .unwrap_or(200);
        p.push_config_derived_mdast_visitor(Box::new(
            zfb_md_extras::reading_time::ReadingTimePlugin::with_wpm(wpm),
        ));
    }

    // ruby runs in the mdast phase so it can scan raw text before mdast→hast.
    // Order-independent relative to github_alerts and the directives step
    // (those operate on blockquote/directive shapes; ruby operates on Text).
    if feature_enabled(&features.ruby) {
        p.push_config_derived_mdast_visitor(Box::new(zfb_md_extras::ruby::RubyPlugin::new()));
    }

    // Build a DirectiveRegistry from the `directives` map only. Core seeds NO
    // default directive names — the `:::name` → `<Component>` vocabulary
    // (note/tip/… and everything else) is supplied entirely by the user's
    // config / docs recipes.
    if directives_enabled(&features.directives) {
        use crate::plugins::directives::DirectiveRegistry;
        use zfb_md_ast::into_directive_def;

        let mut registry = DirectiveRegistry::new()
            .with_gfm(secondary_parse_gfm, secondary_parse_cjk_friendly)
            .with_hard_breaks(secondary_parse_hard_breaks);
        if let Some(dir_map) = &features.directives {
            for (name, spec) in dir_map {
                registry.register(into_directive_def(name, spec));
            }
        }
        p.push_config_derived_mdast_visitor(registry.into_visitor());
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
        p.push_config_derived_hast_visitor(Box::new(
            zfb_md_extras::heading_marker_toc::TocPlugin::new(cfg),
        ));
    }
    if feature_enabled(&features.mermaid) {
        p.push_config_derived_hast_visitor(Box::new(zfb_md_extras::mermaid::MermaidPlugin::new()));
    }
    // Wave 5 (#578): toc_export — emit page TOC as MDX named export.
    // Gated on `is_some()` (the config type carries its own fields; no outer
    // `FeatureToggle` wrapper). Must run AFTER HeadingLinksPlugin (already in
    // the hast chain) so IDs are stable. Inserted before SyntectPlugin so the
    // export node lands at the front of the document root before code blocks
    // are transformed.
    if let Some(cfg) = &features.toc_export {
        p.push_config_derived_hast_visitor(Box::new(
            zfb_md_extras::toc_export::TocExportPlugin::new(cfg.clone()),
        ));
    }
    // Wave 6 (#579): image_dimensions — inject width/height on local <img> elements.
    // Gated on `is_some()` (Option<ImageDimensionsConfig>; no outer FeatureToggle).
    // Uses visit_with_context — pipeline must call run_with_context for this to fire.
    if let Some(cfg) = features.image_dimensions.clone() {
        // JSX-nested image-dimensions stub (zfb#2247): the hast plugin and
        // the mdast-phase stub SHARE one dimensions-cache Arc + read_count
        // (via `new_shared`, zfb-md-extras's passthrough constructor seam)
        // and the SAME recorder clone — mirrors the collector/validator
        // shared-buffer wiring below. An image referenced both top-level
        // and JSX-nested costs one disk read, not two, once #2248 fills
        // in the stub's behavior.
        let (mut plugin, shared) =
            zfb_md_extras::image_dimensions::ImageDimensionsPlugin::new_shared(cfg.clone());
        if let Some(recorder) = read_recorder {
            plugin = plugin.with_recorder(Arc::clone(recorder));
        }
        p.jsx_nested_image_dimensions = Some(
            zfb_md_extras::image_dimensions::JsxNestedImageDimensions::new(
                cfg,
                shared,
                read_recorder.map(Arc::clone),
            ),
        );
        p.push_config_derived_hast_visitor(Box::new(plugin));
    }

    // Wave 6 (#580): link_validation — validate internal links + anchor
    // fragments against the heading-ID registry. Runs VERY LATE in the hast
    // phase — after all heading-mutating visitors — so registry entries for the
    // current file are already populated by HeadingLinksPlugin. Gated on
    // `is_some()` (uses a rich options struct, not a FeatureToggle).
    if let Some(cfg) = &features.link_validation {
        p.resolved_link_fragment_severity = Some(if cfg.fail_on_broken.unwrap_or(false) {
            DiagnosticSeverity::Error
        } else {
            DiagnosticSeverity::Warning
        });
        let mut plugin = zfb_md_extras::link_validation::LinkValidationPlugin::new(cfg.clone());
        if let Some(recorder) = read_recorder {
            plugin = plugin.with_recorder(Arc::clone(recorder));
        }
        // JSX-nested link descent (zfb#2184): one collector + one
        // validator per pipeline share one candidate buffer, wired at
        // feature-registration time (mirrors the recorder wiring above).
        // Config-derived construction, covered by the constructor's base
        // descriptor over the whole `features` value — deliberately NOT
        // routed through `add_mdast_visitor`, which would invalidate
        // `config_fingerprint` (and would also run the collector too
        // early: before the resolve_links application — see the
        // `jsx_nested_link_collector` field doc).
        let nested_buffer: zfb_md_extras::link_validation::JsxNestedLinkBuffer =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        plugin = plugin.with_nested_link_buffer(Arc::clone(&nested_buffer));
        p.jsx_nested_link_collector = Some(
            zfb_md_extras::link_validation::JsxNestedLinkCollector::new(nested_buffer),
        );
        p.push_config_derived_hast_visitor(Box::new(plugin));
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
///
/// # Cacheability
///
/// Same contract as [`register_features`]: a manual post-construction call
/// makes the pipeline uncacheable, even with an empty feature set.
pub fn register_post_syntect_features(
    p: &mut Pipeline,
    features: &zfb_md_extras::MarkdownFeaturesConfig,
) {
    register_post_syntect_features_config_derived(p, features);
    // Keep the nested-code render chain (#2207) in lockstep with the
    // live hast chain — same rationale as the mermaid sync in
    // [`register_features`]: without it, a manually-registered
    // code-enrichment visitor would enrich top-level fences while
    // JSX-nested ones ran a stale reconstructed chain and dropped line
    // highlights / diff markers.
    if let Some(cfg) = &features.code_enrichment {
        if let Some(spec) = p.nested_code_chain_spec.as_mut() {
            spec.code_enrichment = Some(cfg.clone());
        }
    }
    p.invalidate_config_fingerprint();
}

/// Internal, non-invalidating implementation of
/// [`register_post_syntect_features`] — called from
/// [`Pipeline::with_defaults_and_full_config`], whose final base descriptor
/// covers the whole `features` value (invalidation rule — see
/// `Pipeline::push_config_derived_mdast_visitor`).
fn register_post_syntect_features_config_derived(
    p: &mut Pipeline,
    features: &zfb_md_extras::MarkdownFeaturesConfig,
) {
    // Wave 5 (#575), extended by #1657: code_enrichment — diff markers,
    // line highlighting, and visible-text word emphasis.
    if let Some(cfg) = &features.code_enrichment {
        p.push_config_derived_hast_visitor(Box::new(
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
    /// JSX-path strategy. The closure receives an mdast node plus the
    /// SAME [`FootnoteRenderCtx`] the surrounding `mdast_to_hast_with`
    /// walk is using, and returns the JSX-shaped string the bridge
    /// should embed verbatim.
    ///
    /// Threading `FootnoteRenderCtx` through here (issue #2027) is what
    /// lets a footnote reference nested inside an MDX JSX element body
    /// claim its occurrence from the SAME shared cursor the top-level
    /// walk uses, instead of `mdx_jsx_emit` building a second,
    /// independently-advancing model/cursor that could drift out of
    /// sync with the main walk for a document mixing top-level and
    /// JSX-nested references to one identifier.
    JsxPath(&'a dyn Fn(&MdastNode, &FootnoteRenderCtx<'_>) -> String),
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
///
/// Thin wrapper over [`mdast_to_hast_with_model`] that discards the
/// `FootnoteModel` it builds — use that sibling entry point directly
/// when a caller needs the model too (issue #2246, Registration B).
#[must_use]
pub fn mdast_to_hast_with(node: &MdastNode, strategy: &JsxEmitStrategy<'_>) -> HastNode {
    mdast_to_hast_with_model(node, strategy).0
}

/// Same as [`mdast_to_hast_with`], but additionally returns the
/// [`FootnoteModel`] built for `node`.
///
/// Added for issue #2246 (Registration B): `mdx_jsx_emit`'s hast-detour
/// arm needs the SAME `FootnoteModel` instance this conversion consumes
/// — never a second, independently re-derived one — to register every
/// footnote occurrence id (`model.entries()[*].references[*].id`) into
/// the per-compile `HeadingRegistry` before hast visitors run, so a
/// footnote reference marker rendered inside a JsxRaw string (invisible
/// to the hast-phase `HeadingLinksPlugin::visit_node` walk) still
/// resolves as a valid link target. `mdast_to_hast_with` stays the
/// plain-`HastNode` entry point every other caller uses.
///
/// Returning the model alongside the hast tree is safe because
/// [`FootnoteRenderCtx`] only *borrows* it (`FootnoteRenderCtx::new`
/// takes `&FootnoteModel`) — that borrow, and the local `fc` value
/// holding it, are both last used while `hast` is being computed below,
/// so by the time this function returns, moving `model` into the
/// result tuple borrows nothing that is still live.
#[must_use]
pub fn mdast_to_hast_with_model(
    node: &MdastNode,
    strategy: &JsxEmitStrategy<'_>,
) -> (HastNode, FootnoteModel) {
    // Footnotes are the one construct that cannot be rendered from an
    // independent per-node match arm in `mdast_to_hast_inner` below:
    // the rendered footnote section collects at the END of the
    // document, and repeated references to one definition each need a
    // distinct backreference target. `FootnoteModel` (issue #2025)
    // owns that document-level collection/numbering/id-allocation
    // policy; this function's job is only to build the model ONCE per
    // document, thread a cursor through the recursive walk so each
    // `FootnoteReference` node can claim its next occurrence in
    // document order, and append the rendered section as the Root's
    // last child. See `FootnoteRenderCtx` below for why a `RefCell` is
    // needed here despite the walk otherwise taking only shared
    // references.
    let model = FootnoteModel::collect(node);
    let fc = FootnoteRenderCtx::new(&model);
    let hast = match node {
        MdastNode::Root(r) => {
            let mut children: Vec<HastNode> = r
                .children
                .iter()
                .map(|c| mdast_to_hast_inner(c, strategy, &fc))
                .collect();
            if !model.is_empty() {
                children.push(render_footnote_section(&model, strategy, &fc));
            }
            // Walk-parity guard (epic #2021 review fix), the footnote
            // counterpart of `mdx_jsx_emit`'s nested-heading-slug
            // `debug_assert_eq!`. `FootnoteModel::collect` recurses
            // through EVERY `Node::children()`, whereas the emit walk
            // above drops whole subtrees through its catch-all arm
            // (`LinkReference` is the concrete one). A reference
            // stranded in a dropped subtree would still be numbered and
            // would still get a definition whose backreference anchor
            // points at a marker that was never emitted — so assert the
            // two walks covered the same set.
            //
            // Extended to the HTML path by issue #2396: previously this
            // was JSX-path-only, because `JsxEmitStrategy::HtmlPath`'s
            // `reconstruct_jsx` is a documented lossy fallback that
            // stringifies an MDX JSX element's children instead of
            // recursing — a footnote inside a JSX body used to go
            // unclaimed there, which would have fired this very assert
            // on valid input. `reconstruct_jsx` now gets DEDICATED
            // recursive handling for footnotes specifically
            // (`subtree_contains_footnote` + `jsx_body_stringify`),
            // so the invariant holds on both paths — verified by this
            // crate's full test suite (including a same-document,
            // six-container-variant sweep nested inside JSX) with the
            // gate temporarily removed, zero failures. `reconstruct_jsx`
            // remains lossy in general for every OTHER construct
            // (Image/Table/Code/Math nested in JSX, out of #2396's
            // scope — see its "Out of scope" section); this assert only
            // ever inspects footnote occurrence counts, so that general
            // lossiness cannot trip it.
            //
            // The one theoretical gap — a `FootnoteReference` stranded
            // inside a `LinkReference` (reference-style link) that
            // `mdast_to_hast_inner`'s own catch-all drops whole — is
            // NOT reachable through real parsing: markdown-rs's
            // `[^label]` footnote-reference tokenizer takes priority
            // over reference-link bracket matching, so
            // `[text[^n]][ref]` never parses into a `LinkReference`
            // containing a `FootnoteReference` (confirmed by inspecting
            // the produced mdast directly — the sibling test below,
            // `a_reference_in_a_dropped_subtree_trips_the_walk_parity_
            // guard`, has to hand-build that exact shape because
            // markdown-rs cannot produce it).
            //
            // The image-reference spelling `![text[^n]][ref]` is safe for
            // a DIFFERENT reason worth stating, since a reader checking
            // this invariant will reach for it next: markdown-rs folds
            // the whole image label into the `ImageReference`'s `alt`
            // string and emits no `FootnoteReference` node at all, so
            // both walks agree at zero rather than agreeing on one.
            {
                debug_assert_eq!(
                    fc.consumed_total(),
                    model.total_references(),
                    "the hast emit walk claimed {} footnote reference occurrences \
                     but FootnoteModel::collect recorded {} — a reference is \
                     reachable by the model's walk but dropped by an emitter arm",
                    fc.consumed_total(),
                    model.total_references(),
                );
            }
            HastNode::Root { children }
        }
        _ => mdast_to_hast_inner(node, strategy, &fc),
    };
    (hast, model)
}

/// Per-document footnote render state threaded through the recursive
/// [`mdast_to_hast_inner`] walk.
///
/// The walk is otherwise a plain shared-reference recursive descent
/// (no `&mut` anywhere), but claiming "the next occurrence of this
/// footnote identifier" from a [`FootnoteCursor`] needs `&mut self`.
/// Wrapping the cursor in a `RefCell` avoids widening every recursive
/// helper in this module to take `&mut` just to thread one piece of
/// state through — the interior mutability is confined to this one
/// struct.
///
/// Correctness depends on the hast walk visiting `FootnoteReference`
/// nodes in EXACTLY the order [`FootnoteModel::collect`] did when it
/// built the model: main-body nodes in document order (skipping
/// `FootnoteDefinition` subtrees — see the arm below), then each
/// rendered entry's own body, in the model's final entry order. Both
/// walks are plain depth-first traversals over the same
/// `Node::children()`, so they agree.
// `pub`, not `pub(crate)`: `JsxEmitStrategy::JsxPath` is a variant of a
// `pub` enum reachable through `pub mod pipeline`, so a lower-visibility
// type in its function-pointer signature would be a "private type in
// public interface" error. Nothing about `FootnoteRenderCtx` is
// sensitive — it only hands out the next pre-computed reference
// occurrence, mirroring how `HastNode`/`MdastNode` are already public
// types threaded through this same enum.
pub struct FootnoteRenderCtx<'a> {
    cursor: RefCell<FootnoteCursor<'a>>,
}

impl<'a> FootnoteRenderCtx<'a> {
    fn new(model: &'a FootnoteModel) -> Self {
        Self {
            cursor: RefCell::new(model.cursor()),
        }
    }

    /// Consume and return the next reference occurrence for `identifier`
    /// from the shared cursor — see [`FootnoteCursor::next_reference`].
    /// This is what lets `mdx_jsx_emit`'s separate JSX-child recursive
    /// renderer claim occurrences from the SAME cursor the main
    /// `mdast_to_hast_inner` walk uses, keeping numbering/ids correct
    /// for footnotes nested inside an MDX JSX element body.
    #[must_use]
    pub fn next_reference(&self, identifier: &str) -> Option<(&'a FootnoteEntry, &'a FootnoteRef)> {
        self.cursor.borrow_mut().next_reference(identifier)
    }

    /// How many occurrences the shared cursor has handed out — see
    /// [`FootnoteCursor::consumed_total`] and the walk-parity
    /// `debug_assert_eq!` in [`mdast_to_hast_with`].
    #[must_use]
    pub fn consumed_total(&self) -> usize {
        self.cursor.borrow().consumed_total()
    }
}

fn mdast_to_hast_inner(
    node: &MdastNode,
    strategy: &JsxEmitStrategy<'_>,
    fc: &FootnoteRenderCtx<'_>,
) -> HastNode {
    match node {
        MdastNode::Root(r) => HastNode::Root {
            children: convert_children_with(&r.children, strategy, fc),
        },
        MdastNode::Paragraph(p) => element(
            "p",
            vec![],
            convert_children_with(&p.children, strategy, fc),
        ),
        MdastNode::Heading(h) => {
            let depth = h.depth.clamp(1, 6);
            let tag = format!("h{depth}");
            element(
                &tag,
                vec![],
                convert_children_with(&h.children, strategy, fc),
            )
        }
        MdastNode::Text(t) => HastNode::Text(t.value.clone()),
        MdastNode::Emphasis(e) => element(
            "em",
            vec![],
            convert_children_with(&e.children, strategy, fc),
        ),
        MdastNode::Strong(s) => element(
            "strong",
            vec![],
            convert_children_with(&s.children, strategy, fc),
        ),
        MdastNode::Delete(d) => element(
            "del",
            vec![],
            convert_children_with(&d.children, strategy, fc),
        ),
        MdastNode::InlineCode(c) => element("code", vec![], vec![HastNode::Text(c.value.clone())]),
        MdastNode::Code(c) => code_block_hast(c),
        MdastNode::Link(l) => {
            let mut attrs = vec![("href".to_string(), l.url.clone())];
            if let Some(title) = &l.title {
                attrs.push(("title".to_string(), title.clone()));
            }
            element("a", attrs, convert_children_with(&l.children, strategy, fc))
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
            element(tag, attrs, convert_children_with(&l.children, strategy, fc))
        }
        MdastNode::ListItem(li) => {
            let mut children = convert_children_with(&li.children, strategy, fc);
            // GFM task-list checkbox (issue #2024, epic #2021): the
            // `markdown` crate's task-list tokenizer sets `checked` on
            // `ListItem` when `taskListItem` is enabled. Minimal
            // compatible rendering (Option B — see #2028): a disabled
            // `<input type="checkbox">` before the item's own content,
            // `checked` present only when the item is checked. Static
            // server-rendered output has no toggle handler, so the
            // checkbox is always `disabled`.
            if let Some(checked) = li.checked {
                prepend_task_list_checkbox(&mut children, checked);
            }
            element("li", vec![], children)
        }
        MdastNode::Blockquote(b) => element(
            "blockquote",
            vec![],
            convert_children_with(&b.children, strategy, fc),
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
        | MdastNode::MdxTextExpression(_) => HastNode::JsxRaw(emit_jsx_raw(node, strategy, fc)),
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
            attrs: vec![("class".to_string(), "language-math math-inline".to_string())],
            children: vec![HastNode::Text(m.value.clone())],
            void: false,
        },
        // GFM pipe-table → <table><thead>...</thead><tbody>...</tbody></table>
        // with per-column `style="text-align: ..."` on each th/td. Mirrors
        // emit_table_jsx in mdx_jsx_emit.rs for the no-pipeline path.
        MdastNode::Table(t) => {
            let align = &t.align;
            let style_attr = |col: usize| -> Option<(String, String)> {
                let kind = align
                    .get(col)
                    .copied()
                    .unwrap_or(markdown::mdast::AlignKind::None);
                let s = match kind {
                    markdown::mdast::AlignKind::Left => Some("left"),
                    markdown::mdast::AlignKind::Right => Some("right"),
                    markdown::mdast::AlignKind::Center => Some("center"),
                    markdown::mdast::AlignKind::None => None,
                };
                s.map(|v| ("style".to_string(), format!("text-align: {v}")))
            };

            let row_to_cells = |row: &MdastNode, tag: &str| -> Vec<HastNode> {
                let MdastNode::TableRow(tr) = row else {
                    return Vec::new();
                };
                tr.children
                    .iter()
                    .enumerate()
                    .filter_map(|(col, cell)| {
                        let MdastNode::TableCell(tc) = cell else {
                            return None;
                        };
                        let mut attrs: Vec<(String, String)> = Vec::new();
                        if let Some(s) = style_attr(col) {
                            attrs.push(s);
                        }
                        Some(element(
                            tag,
                            attrs,
                            convert_children_with(&tc.children, strategy, fc),
                        ))
                    })
                    .collect()
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
        // GFM footnotes (issue #2023/#2025/#2026, epic #2021). A
        // `FootnoteDefinition` never renders in place — its body only
        // appears once, in the collected section
        // `mdast_to_hast_with` appends at the end of the document — so
        // encountering one here (main body, or nested inside another
        // rendered entry's body, which structurally never happens but
        // is handled uniformly regardless) always renders nothing.
        // Duplicate definitions for the same identifier all hit this
        // arm too; `FootnoteModel` already decided which one's body is
        // the winning entry.
        MdastNode::FootnoteDefinition(_) => HastNode::Raw(String::new()),
        // A `FootnoteReference` claims its next occurrence from the
        // shared cursor — see `FootnoteRenderCtx`'s ordering guarantee.
        // `FootnoteModel::collect` only ever creates an entry for an
        // identifier that has a matching definition, and markdown-rs
        // itself never parses an unmatched `[^label]` into this node
        // (it stays literal text), so `next_reference` returning `None`
        // here is not reachable through the public parse API — the
        // fallback exists only so a hand-built mdast tree degrades to
        // nothing instead of panicking.
        MdastNode::FootnoteReference(r) => fc
            .cursor
            .borrow_mut()
            .next_reference(&r.identifier)
            .map(|(entry, footnote_ref)| footnote_reference_marker(entry, footnote_ref))
            .unwrap_or_else(|| HastNode::Raw(String::new())),
        // Unhandled: degrade to empty Raw so we never crash on
        // unsupported input. Reference-style link/image definitions
        // (`[label]: /url`) and their `[label]`/`![label]` references,
        // ESM, frontmatter, etc. fall here. They become passthrough
        // holes that Sub 4 plugins can later fill in.
        _ => HastNode::Raw(String::new()),
    }
}

/// Convert a slice of mdast children into a vec of hast children
/// using the given strategy for the JSX-shaped arms.
fn convert_children_with(
    children: &[MdastNode],
    strategy: &JsxEmitStrategy<'_>,
    fc: &FootnoteRenderCtx<'_>,
) -> Vec<HastNode> {
    children
        .iter()
        .map(|c| mdast_to_hast_inner(c, strategy, fc))
        .collect()
}

/// `<sup><a href="#{definition id}" id="{occurrence id}" data-footnote-ref
/// aria-describedby="footnote-label">{number}</a></sup>` — the reference
/// marker shape pinned by `footnotes` module docs' "Backreference
/// markup" / "Accessibility attributes" sections.
fn footnote_reference_marker(entry: &FootnoteEntry, footnote_ref: &FootnoteRef) -> HastNode {
    let marker = HastNode::Element {
        tag: "a".to_string(),
        attrs: vec![
            ("href".to_string(), entry.href()),
            ("id".to_string(), footnote_ref.id.clone()),
            ("data-footnote-ref".to_string(), String::new()),
            (
                "aria-describedby".to_string(),
                FOOTNOTE_LABEL_ID.to_string(),
            ),
        ],
        children: vec![HastNode::Text(footnote_ref.number.to_string())],
        void: false,
    };
    element("sup", vec![], vec![marker])
}

/// The collected `<section data-footnotes>` appended at document end —
/// empty when [`FootnoteModel::is_empty`], so callers must check that
/// first (mirrors `footnotes` module docs' rendered-shape sketch).
fn render_footnote_section(
    model: &FootnoteModel,
    strategy: &JsxEmitStrategy<'_>,
    fc: &FootnoteRenderCtx<'_>,
) -> HastNode {
    // `role="heading" aria-level="2"` on a `div`, not a real `<h2>`
    // element (issue #2026 review fix). `HeadingLinksPlugin` and
    // `TocPlugin` both key on the literal tag name (`heading_depth` /
    // `is_heading_tag` match `"h1"`..`"h6"` only) with no opt-out for a
    // synthetic heading, so an actual `<h2>` here would get its
    // `id="footnote-label"` silently overwritten with a content slug
    // and a permalink anchor appended — breaking every reference's
    // `aria-describedby="footnote-label"` and potentially leaking into
    // a page's TOC. The ARIA `role="heading"` pattern announces the
    // same landmark to assistive tech without being a tag those
    // visitors recognise.
    let heading = HastNode::Element {
        tag: "div".to_string(),
        attrs: vec![
            ("role".to_string(), "heading".to_string()),
            ("aria-level".to_string(), "2".to_string()),
            ("class".to_string(), "sr-only".to_string()),
            // Hiding is carried by the INLINE style, not by the class:
            // zfb ships no stylesheet defining `.sr-only`, and because
            // the class is emitted from Rust, Tailwind's content scan
            // never sees it and so never generates the utility either.
            // Without this the "visually hidden" landmark documented in
            // `footnotes`' module docs would render as plain visible
            // body text in essentially every project.
            ("style".to_string(), FOOTNOTE_LABEL_STYLE.to_string()),
            ("id".to_string(), FOOTNOTE_LABEL_ID.to_string()),
        ],
        children: vec![HastNode::Text(FOOTNOTE_LABEL_TEXT.to_string())],
        void: false,
    };
    let items: Vec<HastNode> = model
        .entries()
        .iter()
        .map(|entry| render_footnote_item(entry, strategy, fc))
        .collect();
    HastNode::Element {
        tag: "section".to_string(),
        attrs: vec![
            ("data-footnotes".to_string(), String::new()),
            ("class".to_string(), FOOTNOTE_SECTION_CLASS.to_string()),
        ],
        children: vec![heading, element("ol", vec![], items)],
        void: false,
    }
}

/// One `<li id="{definition id}">{body}{backref}…</li>` — the
/// definition's own children, converted through the SAME shared
/// `FootnoteRenderCtx` (so nested references inside a definition body
/// claim their occurrences from the same cursor), followed by one
/// backreference link per [`FootnoteEntry::references`] occurrence.
fn render_footnote_item(
    entry: &FootnoteEntry,
    strategy: &JsxEmitStrategy<'_>,
    fc: &FootnoteRenderCtx<'_>,
) -> HastNode {
    let mut children = convert_children_with(&entry.body, strategy, fc);
    for footnote_ref in &entry.references {
        children.push(HastNode::Element {
            tag: "a".to_string(),
            attrs: vec![
                ("href".to_string(), footnote_ref.backref_href()),
                ("data-footnote-backref".to_string(), String::new()),
                ("aria-label".to_string(), footnote_ref.backref_aria_label()),
            ],
            children: vec![HastNode::Text(FOOTNOTE_BACKREF_MARKER.to_string())],
            void: false,
        });
    }
    HastNode::Element {
        tag: "li".to_string(),
        attrs: vec![("id".to_string(), entry.id.clone())],
        children,
        void: false,
    }
}

/// Pick the right JSX-text producer for the supplied strategy and
/// invoke it. The HTML-path strategy uses the in-module
/// `reconstruct_jsx` (lossy fallback for non-text children, preserves
/// pre-#121 HTML snapshot output). The JSX-path strategy delegates to
/// the user-supplied closure (typically the recursive renderer in
/// `mdx_jsx_emit`).
fn emit_jsx_raw(
    node: &MdastNode,
    strategy: &JsxEmitStrategy<'_>,
    fc: &FootnoteRenderCtx<'_>,
) -> String {
    if let JsxEmitStrategy::JsxPath(emit) = strategy {
        return emit(node, fc);
    }
    // HTML-path strategy: preserve pre-#121 behaviour exactly, except for
    // footnotes (issue #2396) — `reconstruct_jsx` needs the same `fc` the
    // main walk uses so a `FootnoteReference`/`FootnoteDefinition` nested
    // in a JSX body claims from (and is counted by) the one shared cursor.
    match node {
        MdastNode::MdxJsxFlowElement(j) => {
            reconstruct_jsx(j.name.as_deref(), &j.attributes, &j.children, fc)
        }
        MdastNode::MdxJsxTextElement(j) => {
            reconstruct_jsx(j.name.as_deref(), &j.attributes, &j.children, fc)
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

/// Build the hast node for ONE fenced code block: `<pre><code
/// class="language-{lang}" data-lang="…" data-meta="…">TEXT</code></pre>`.
/// `lang` and `meta` are exposed as data-* attrs so the code-block
/// plugins (code-title, mermaid, syntect, code-enrichment) can inspect
/// them.
///
/// The SINGLE authority for the node shape both render paths feed the
/// code-block chain: [`mdast_to_hast_inner`]'s top-level `Code` arm AND
/// the JSX-emit path's nested-fence renderer
/// (`mdx_jsx_emit::nested_code_via_chain`, #2207). Sharing one
/// constructor is what guarantees a nested fence enters the chain as the
/// EXACT node a top-level fence would — keep any shape change here, in
/// one place.
pub(crate) fn code_block_hast(c: &markdown::mdast::Code) -> HastNode {
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

/// Build the (disabled) task-list checkbox hast node that precedes a
/// `ListItem`'s own children when `ListItem.checked` is `Some(_)`.
/// Mirrors `mdx_jsx_emit`'s JSX-emit counterpart of the same fix
/// (issue #2024): always `disabled` (static, server-rendered output,
/// never interactive), and carries a `checked` attribute only when the
/// item itself is checked. The serializer always writes `attr="value"`
/// (no bare-boolean HTML shorthand), so both attributes get an empty
/// string value here.
/// Place a task-list item's checkbox so it reads as a checkbox BESIDE its
/// label, the way GitHub renders one.
///
/// This pipeline never unwraps tight-list paragraphs, so a task item's
/// converted children are `[<p>label</p>, …]`. Inserting the checkbox as a
/// SIBLING before that `<p>` would put it on its own line above the label
/// (`<li><input/><p>label</p></li>`), which is what the epic originally
/// shipped. So the checkbox goes INSIDE the item's leading paragraph
/// instead, followed by a single space — no general tight-list unwrapping
/// (which would rewrite every list in every existing snapshot), just the
/// one placement this construct needs.
///
/// An item that does not start with a paragraph (a task item whose body
/// begins with a nested list or a code block) keeps the sibling-prefix
/// placement; there is no inline context to join in that case.
///
/// The JSX-emit counterparts in `mdx_jsx_emit` apply the same rule — see
/// `task_list_checkbox_jsx`.
fn prepend_task_list_checkbox(children: &mut Vec<HastNode>, checked: bool) {
    let checkbox = task_list_checkbox_hast(checked);
    let spacer = HastNode::Text(" ".to_string());
    match children.first_mut() {
        Some(HastNode::Element {
            tag,
            children: paragraph_children,
            ..
        }) if tag == "p" => {
            paragraph_children.insert(0, spacer);
            paragraph_children.insert(0, checkbox);
        }
        _ => {
            children.insert(0, spacer);
            children.insert(0, checkbox);
        }
    }
}

fn task_list_checkbox_hast(checked: bool) -> HastNode {
    let mut attrs = vec![
        ("type".to_string(), "checkbox".to_string()),
        ("disabled".to_string(), String::new()),
    ];
    if checked {
        attrs.push(("checked".to_string(), String::new()));
    }
    HastNode::Element {
        tag: "input".to_string(),
        attrs,
        children: vec![],
        void: true,
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
///
/// Footnotes are the first exception (issue #2396): `fc` is threaded
/// through so a `FootnoteReference`/`FootnoteDefinition` anywhere in the
/// children (including nested inside another JSX element) is caught by
/// the `subtree_contains_footnote` gate in the fallback arm below and
/// rendered through `jsx_body_stringify` instead of the plain
/// `other.to_string()`, which silently drops the reference marker and
/// inlines the definition body at the wrong spot.
///
/// `Break` is the second exception (issue #2401), gated for the same
/// structural reason: it is one of `to_string()`'s "voids", so a hard
/// break inside a JSX body vanishes ENTIRELY and fuses the words on
/// either side of it (`:::note\nfirst line\nsecond line\n:::` under
/// `markdown.hardBreaks` rendered `first linesecond line`). That is
/// deletion of author content, categorically different from
/// `Strong`/`Emphasis`, which merely drop their formatting while
/// retaining every character — which is why those stay on the
/// deliberately lossy catch-all and `Break` does not.
fn reconstruct_jsx(
    name: Option<&str>,
    attrs: &[AttributeContent],
    children: &[MdastNode],
    fc: &FootnoteRenderCtx<'_>,
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
                reconstruct_jsx(j.name.as_deref(), &j.attributes, &j.children, fc)
            }
            MdastNode::MdxJsxTextElement(j) => {
                reconstruct_jsx(j.name.as_deref(), &j.attributes, &j.children, fc)
            }
            // Fallback: stringify the markdown text content. This loses
            // formatting but keeps content visible; downstream plugins
            // generally avoid putting markdown inside JSX bodies anyway.
            // Gated on whether the subtree contains a footnote node
            // (issue #2396) or a `Break` (issue #2401) so every OTHER
            // subtree keeps taking the exact `other.to_string()` path —
            // byte-identity for input carrying neither is structural,
            // not dependent on a hand-copied mirror of markdown-rs's own
            // dispatch staying in sync across crate bumps. Both
            // exceptions share ONE mirror, so a subtree carrying both
            // renders both correctly by construction.
            other if subtree_contains_footnote(other) || subtree_contains_break(other) => {
                jsx_body_stringify(other, fc)
            }
            other => other.to_string(),
        })
        .collect();

    format!("<{tag}{space}{attrs_str}>{inner}</{tag}>")
}

/// True if `node` or anything reachable through its `Node::children()`
/// subtree is a `FootnoteReference` or `FootnoteDefinition`. Used only to
/// decide, per child, whether `reconstruct_jsx`'s fallback arm needs the
/// footnote-aware stringifier — see that arm's doc comment.
fn subtree_contains_footnote(node: &MdastNode) -> bool {
    if matches!(
        node,
        MdastNode::FootnoteReference(_) | MdastNode::FootnoteDefinition(_)
    ) {
        return true;
    }
    node.children()
        .is_some_and(|children| children.iter().any(subtree_contains_footnote))
}

/// True if `node` or anything reachable through its `Node::children()`
/// subtree is a `Break` (a hard line break). Used only to decide, per
/// child, whether `reconstruct_jsx`'s fallback arm needs the recursive
/// stringifier — see that arm's doc comment.
///
/// The gate has to be subtree-wide, not a direct-child match: the real
/// repro (issue #2401) reaches `reconstruct_jsx` as an
/// `MdxJsxFlowElement` whose children are `Paragraph`s built by
/// `directives::paragraph_from_lines`, with the `Break` nested INSIDE
/// one of them. A direct `MdastNode::Break(_)` arm on `reconstruct_jsx`
/// would never fire for it.
fn subtree_contains_break(node: &MdastNode) -> bool {
    if matches!(node, MdastNode::Break(_)) {
        return true;
    }
    node.children()
        .is_some_and(|children| children.iter().any(subtree_contains_break))
}

/// Recursive stringifier for a JSX-body subtree that
/// `subtree_contains_footnote` or `subtree_contains_break` flagged as
/// containing a node the plain `to_string()` renders destructively.
///
/// Mirrors markdown-rs's own `Node::to_string()` dispatch (recurse
/// containers via `Node::children()`, literals return their `value`,
/// voids return an empty string) so every other node in the subtree
/// renders exactly as `to_string()` would — `Node::children()`
/// returns `Some` for precisely the set of variants `to_string()` treats
/// as containers, so generic recursion through it reproduces that
/// dispatch without hand-copying its ~30 arms. Keeping ONE mirror for
/// both exceptions (rather than a second parallel stringifier per
/// exception) is what makes a subtree carrying a footnote AND a `Break`
/// correct by construction. Three variants are special-cased instead of
/// falling through to that mirror:
///
/// - `FootnoteReference` is one of `to_string()`'s "voids" (renders as
///   `""`), which is exactly the "reference vanishes" bug this issue
///   fixes. Here it claims its next occurrence from the shared cursor
///   (mirroring the top-level `MdastNode::FootnoteReference` arm in
///   `mdast_to_hast_inner`) and renders the same `<sup><a>` marker
///   through the serializer — consistent with how `Html` nodes already
///   splice raw markup verbatim into this lossy string.
/// - `FootnoteDefinition` is one of `to_string()`'s "containers", which
///   would inline its body at the definition point. It always
///   stringifies to empty here, matching the top-level
///   `MdastNode::FootnoteDefinition` arm in `mdast_to_hast_inner` — a
///   definition's body renders exactly once, in the footnote section
///   `mdast_to_hast_with` appends at document end.
/// - `Break` is another of `to_string()`'s "voids", so a hard break in a
///   JSX body used to be deleted outright, fusing the words on either
///   side of it (issue #2401). It renders as literal `<br />`: this
///   string becomes a raw JSX payload the serializer passes through
///   verbatim, and it cannot land inside an attribute value (attributes
///   are reconstructed separately by `render_attrs`). The sibling
///   transclusion assertion in `tests/gfm_secondary_parse_sites.rs`
///   already pins `<br` as a hard break's rendering under
///   `markdown.hardBreaks`, so this keeps JSX bodies consistent with
///   every other surface.
fn jsx_body_stringify(node: &MdastNode, fc: &FootnoteRenderCtx<'_>) -> String {
    match node {
        MdastNode::FootnoteReference(r) => fc
            .next_reference(&r.identifier)
            .map(|(entry, footnote_ref)| {
                crate::serializer::serialize(&footnote_reference_marker(entry, footnote_ref))
            })
            .unwrap_or_default(),
        MdastNode::FootnoteDefinition(_) => String::new(),
        MdastNode::Break(_) => "<br />".to_string(),
        other => match other.children() {
            Some(children) => children.iter().map(|c| jsx_body_stringify(c, fc)).collect(),
            None => other.to_string(),
        },
    }
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
        let mut p = Pipeline::with_resolved_gfm_constructs(ResolvedGfmConstructs::ALL_OFF);
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

    // 13. with_defaults seeds ZERO directive vocabulary — `:::note` is left
    // untransformed because core ships no built-in directive names. Directive
    // mapping is opt-in via `features.directives` on
    // `with_defaults_and_full_config`.
    #[test]
    fn with_defaults_seeds_no_directive_vocabulary() {
        let mut p = Pipeline::with_defaults();
        let h = p.run(":::note\n\nbody\n\n:::\n").expect("pipeline runs ok");
        let mut raws = Vec::new();
        collect_raw(&h, &mut raws);
        assert!(
            !raws.iter().any(|r| r.contains("<Note")),
            "with_defaults must NOT synthesize <Note> (zero core vocabulary); got raws={raws:?}",
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
            let h = p.run(":::note\n\nbody\n\n:::\n").expect("pipeline runs ok");
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
        let mut themed_pipeline = Pipeline::with_defaults_and_theme(Some("InspiredGitHub"));
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

    // 18. Per-call cache-key context (zfb#939): only resolve-links
    // pipelines carry one; an unset source_dir is distinct from every
    // set dir (the empty path included — None resolves only absolute
    // lookups); spelling differences of one dir do not split the key.
    #[test]
    fn cache_key_context_keys_the_resolve_links_source_dir() {
        let mut p = Pipeline::with_defaults();
        assert!(
            p.cache_key_context().is_none(),
            "no resolve-links plugin => no per-call context (key shape unchanged)"
        );

        p.add_resolve_links(std::collections::HashMap::new());
        let unset = p.cache_key_context().expect("wired => context");

        p.set_resolve_links_source_dir(std::path::PathBuf::from(""));
        let empty = p.cache_key_context().expect("wired => context");
        assert_ne!(unset, empty, "unset dir must never alias an empty dir");

        p.set_resolve_links_source_dir(std::path::PathBuf::from("/x/./y/"));
        let spelled = p.cache_key_context().expect("wired => context");
        p.set_resolve_links_source_dir(std::path::PathBuf::from("/x/y"));
        let canonical = p.cache_key_context().expect("wired => context");
        assert_eq!(
            spelled, canonical,
            "two spellings of one dir must share a cache key"
        );

        p.set_resolve_links_source_dir(std::path::PathBuf::from("/x/z"));
        assert_ne!(
            p.cache_key_context().expect("wired => context"),
            canonical,
            "different dirs must never share a cache key"
        );

        // `..` spellings are runtime-distinct (`Path` equality keeps
        // them, and so do the map lookups joined from this dir), so
        // they must key separately — merging them could serve a stale
        // hit.
        p.set_resolve_links_source_dir(std::path::PathBuf::from("/x/a/../y"));
        assert_ne!(
            p.cache_key_context().expect("wired => context"),
            canonical,
            "a `..` spelling can look up differently — it must key separately"
        );
    }

    // 18b. zfb#1030: the URL-space fallback base joins the per-call
    // context. `section/index.mdx` and `section/article.mdx` share a
    // source_dir but resolve dir-style hrefs differently — their cache
    // keys must never alias.
    #[test]
    fn cache_key_context_keys_the_url_space_fallback_base() {
        let mut p = Pipeline::with_defaults();
        p.add_resolve_links(std::collections::HashMap::new());

        p.set_resolve_links_source_file(std::path::PathBuf::from("/x/section/index.mdx"));
        let index = p.cache_key_context().expect("wired => context");

        p.set_resolve_links_source_file(std::path::PathBuf::from("/x/section/article.mdx"));
        let article = p.cache_key_context().expect("wired => context");
        assert_ne!(
            index, article,
            "index vs non-index file in the same dir must key separately"
        );

        p.set_resolve_links_source_file(std::path::PathBuf::from("/x/section/other.mdx"));
        assert_ne!(
            p.cache_key_context().expect("wired => context"),
            article,
            "two non-index files in the same dir must key separately"
        );

        // The dir-only setter disarms the fallback — its context must
        // match the index file's (same dir, no fallback base), proving
        // legacy callers key like the unarmed state rather than leaking
        // the previous file's base.
        p.set_resolve_links_source_dir(std::path::PathBuf::from("/x/section"));
        assert_eq!(
            p.cache_key_context().expect("wired => context"),
            index,
            "dir-only setter must key as the unarmed-fallback state"
        );
    }

    // -----------------------------------------------------------------
    // Footnote RED tests (issue #2023, epic #2021: GFM Footnotes And
    // Task Lists).
    //
    // `FootnoteDefinition`/`FootnoteReference` currently fall into the
    // catch-all at the bottom of `mdast_to_hast_inner` (`_ =>
    // HastNode::Raw(String::new())`), so both the reference marker and
    // the definition body vanish with no diagnostic. These tests pin
    // the DOCUMENT-LEVEL behaviour the fix (#2026) must produce —
    // reference/definition association, per-occurrence distinct
    // backreference targets, and document-end ordering — not just
    // "some text survives somewhere". They deliberately do NOT
    // hardcode the exact id/href STRING scheme (e.g.
    // `user-content-fn-1`): that escaping/allocation policy belongs to
    // the document-level model in #2025. Structural helpers only.

    /// Depth-first collection of every `HastNode::Element` in `node`
    /// (pre-order, i.e. document order), including `node` itself when
    /// it is an `Element`.
    fn collect_elements<'a>(node: &'a HastNode, out: &mut Vec<&'a HastNode>) {
        match node {
            HastNode::Element { children, .. } => {
                out.push(node);
                for c in children {
                    collect_elements(c, out);
                }
            }
            HastNode::Root { children } => {
                for c in children {
                    collect_elements(c, out);
                }
            }
            _ => {}
        }
    }

    /// Flatten all `Text`/`Raw`/`JsxRaw` content under `node`, ignoring
    /// markup structure — used to check that a definition's body text
    /// (or a dropped literal reference) landed somewhere in the
    /// output, and to compare document-order text positions.
    fn flatten_text(node: &HastNode) -> String {
        match node {
            HastNode::Text(s) | HastNode::Raw(s) | HastNode::JsxRaw(s) => s.clone(),
            HastNode::Element { children, .. } | HastNode::Root { children } => {
                children.iter().map(flatten_text).collect()
            }
            HastNode::Comment(_) => String::new(),
        }
    }

    /// Look up an attribute value on an `Element` node; `None` for any
    /// other node kind or a missing attribute.
    fn attr<'a>(node: &'a HastNode, key: &str) -> Option<&'a str> {
        match node {
            HastNode::Element { attrs, .. } => attrs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    /// Pipeline with every GFM construct on, including
    /// `footnote_definition` (off in the `CONSERVATIVE` default the
    /// rest of this module's tests use via the bare `run()` helper).
    fn run_with_footnotes(input: &str) -> HastNode {
        Pipeline::with_resolved_gfm_constructs(ResolvedGfmConstructs::ALL_ON)
            .run(input)
            .expect("parse ok")
    }

    /// The `<Note>` JsxRaw string an HTML-path render collapses a
    /// `<Note>` element into. Every footnote-nested-in-JSX test starts
    /// from this, whether the element came from literal JSX syntax or
    /// from a `:::note` directive.
    fn note_jsx(h: &HastNode) -> String {
        let mut raws = Vec::new();
        collect_raw(h, &mut raws);
        raws.into_iter()
            .find(|r| r.starts_with("<Note>"))
            .unwrap_or_else(|| panic!("expected a <Note> JsxRaw node: {h:#?}"))
    }

    /// The association triple every footnote-nested-in-JSX test asserts:
    /// the reference marker renders inside the JSX body, the definition
    /// body is NOT inlined there, and it renders exactly once in the
    /// appended footnote section. Returns the definition element so a
    /// caller can go on to assert about its subtree (e.g. the backref).
    fn assert_footnote_renders_once<'a>(
        h: &'a HastNode,
        jsx: &str,
        label: &str,
        body: &str,
    ) -> &'a HastNode {
        assert!(
            jsx.contains(&format!("<sup><a href=\"#user-content-fn-{label}\"")),
            "expected the footnote reference marker for [^{label}] inside the \
             JSX body: {jsx:?}"
        );
        assert!(
            !jsx.contains(body),
            "definition body must not be inlined inside the JSX body: {jsx:?}"
        );

        let mut elements = Vec::new();
        collect_elements(h, &mut elements);
        let definition_target = elements
            .iter()
            .find(|e| attr(e, "id") == Some(format!("user-content-fn-{label}").as_str()))
            .copied()
            .unwrap_or_else(|| panic!("expected the rendered definition element: {h:#?}"));
        assert!(
            flatten_text(definition_target).contains(body),
            "definition body must render exactly once, in the footnote \
             section: {definition_target:#?}"
        );
        definition_target
    }

    // 1. A single reference and its definition are ASSOCIATED: the
    // reference renders a visible marker element that links (by some
    // id) to the definition's rendered location, and the definition's
    // rendered location links back to the reference. Today both nodes
    // fall into the catch-all, so no such elements exist at all —
    // every `find`/`unwrap_or_else` below panics on the current code.
    #[test]
    fn footnote_reference_and_definition_are_associated() {
        let h = run_with_footnotes("Ref one[^a] end.\n\n[^a]: Definition body.\n");

        let mut elements = Vec::new();
        collect_elements(&h, &mut elements);

        // The reference renders as SOME element carrying the visible
        // footnote number "1" and a fragment link into the document.
        let marker = elements
            .iter()
            .find(|e| flatten_text(e) == "1" && attr(e, "href").is_some_and(|h| h.starts_with('#')))
            .copied()
            .unwrap_or_else(|| panic!("expected a footnote reference marker element in {h:#?}"));
        let marker_href = attr(marker, "href")
            .unwrap()
            .trim_start_matches('#')
            .to_string();

        // The definition body text must appear SOMEWHERE in the
        // document (not dropped)…
        assert!(
            flatten_text(&h).contains("Definition body."),
            "footnote definition body missing from output: {h:#?}"
        );

        // …and specifically at an element carrying the id the marker's
        // href points at (proves the reference resolves to ITS
        // definition, not merely that the text exists somewhere).
        let definition_target = elements
            .iter()
            .find(|e| attr(e, "id") == Some(marker_href.as_str()))
            .copied()
            .unwrap_or_else(|| {
                panic!(
                    "no element with id=\"{marker_href}\" — the reference's href \
                     must resolve to the rendered definition: {h:#?}"
                )
            });
        assert!(
            flatten_text(definition_target).contains("Definition body."),
            "the id the reference points at must contain the definition body, got: {definition_target:#?}"
        );

        // The definition must carry a backreference link pointing back
        // at the reference occurrence (some element within the
        // definition's own subtree whose href targets the marker's
        // own id — i.e. the two link to each other).
        let marker_id = attr(marker, "id")
            .unwrap_or_else(|| panic!("reference marker must carry its own id: {marker:#?}"));
        let mut def_subtree = Vec::new();
        collect_elements(definition_target, &mut def_subtree);
        let expected_backref = format!("#{marker_id}");
        assert!(
            def_subtree
                .iter()
                .any(|e| attr(e, "href") == Some(expected_backref.as_str())),
            "definition must contain a backreference link to {expected_backref}: {definition_target:#?}"
        );
    }

    // 2. Repeated references to ONE definition: each occurrence needs
    // its OWN backreference target (so the definition can link back to
    // each usage individually), even though both display the SAME
    // footnote number — this is called out explicitly in #2023's
    // acceptance criteria.
    #[test]
    fn repeated_references_get_distinct_backreference_targets() {
        let h = run_with_footnotes("Ref one[^a] and ref again[^a] end.\n\n[^a]: Shared def.\n");
        let mut elements = Vec::new();
        collect_elements(&h, &mut elements);

        let markers: Vec<&&HastNode> = elements
            .iter()
            .filter(|e| {
                flatten_text(e) == "1" && attr(e, "href").is_some_and(|h| h.starts_with('#'))
            })
            .collect();
        assert_eq!(
            markers.len(),
            2,
            "expected exactly two footnote-1 reference markers (same number, \
             two occurrences), got {}: {h:#?}",
            markers.len()
        );

        let ids: Vec<&str> = markers
            .iter()
            .map(|m| attr(m, "id").unwrap_or_else(|| panic!("marker missing id: {m:#?}")))
            .collect();
        assert_ne!(
            ids[0], ids[1],
            "each repeated reference occurrence must get its own distinct \
             backreference id, got the same id twice: {ids:?}"
        );

        // Every marker id must have a corresponding backreference link
        // SOMEWHERE in the document (proving the definition can return
        // to each specific occurrence, not just to one of them).
        for id in &ids {
            let target_href = format!("#{id}");
            assert!(
                elements
                    .iter()
                    .any(|e| attr(e, "href") == Some(target_href.as_str())),
                "no backreference link found pointing at {target_href}: {h:#?}"
            );
        }
    }

    // 3. Multiple definitions: numbering and document-end ORDER follow
    // REFERENCE order (the convention every GFM/remark-based renderer
    // follows), not source-definition order — this fixture deliberately
    // declares `[^b]` before `[^a]` while the body references `a`
    // first, so a definition-order implementation would fail this
    // differently (numbers/order swapped) than today's total drop.
    #[test]
    fn multiple_definitions_are_numbered_and_ordered_by_first_reference() {
        let h = run_with_footnotes(
            "First[^a] then second[^b] end.\n\n[^b]: Second body.\n\n[^a]: First body.\n",
        );
        let mut elements = Vec::new();
        collect_elements(&h, &mut elements);

        // Document-order DFS visits the `a` reference (appears first
        // in the body) before the `b` reference.
        let markers: Vec<&&HastNode> = elements
            .iter()
            .filter(|e| attr(e, "href").is_some_and(|href| href.starts_with('#')))
            .filter(|e| flatten_text(e) == "1" || flatten_text(e) == "2")
            .collect();
        assert_eq!(
            markers.len(),
            2,
            "expected two distinct footnote reference markers, got {}: {h:#?}",
            markers.len()
        );
        assert_eq!(
            flatten_text(markers[0]),
            "1",
            "the FIRST-referenced footnote (`a`) must be numbered 1: {h:#?}"
        );
        assert_eq!(
            flatten_text(markers[1]),
            "2",
            "the SECOND-referenced footnote (`b`) must be numbered 2: {h:#?}"
        );

        let full_text = flatten_text(&h);
        let a_pos = full_text
            .find("First body.")
            .unwrap_or_else(|| panic!("First body. missing from output: {full_text:?}"));
        let b_pos = full_text
            .find("Second body.")
            .unwrap_or_else(|| panic!("Second body. missing from output: {full_text:?}"));
        assert!(
            a_pos < b_pos,
            "footnote definitions must render in REFERENCE order (a before b), \
             got a@{a_pos} b@{b_pos}: {full_text:?}"
        );
    }

    // 4. Duplicate `[^a]` definitions. WHICH body wins (first vs last)
    // is an explicit POLICY CHOICE #2025 owns (see its issue body's
    // "Duplicate definitions" bullet) — this test does NOT prescribe
    // the tie-break. It only pins the one structural fact that must
    // hold under ANY reasonable policy: duplicates COLLAPSE to exactly
    // one rendered footnote, not two, and not neither.
    #[test]
    fn duplicate_definitions_collapse_to_exactly_one_entry() {
        let h = run_with_footnotes("Dup label[^a] end.\n\n[^a]: First.\n\n[^a]: Second (dup).\n");
        let full_text = flatten_text(&h);
        let has_first = full_text.contains("First.");
        let has_second = full_text.contains("Second (dup).");
        assert!(
            has_first ^ has_second,
            "exactly ONE of the duplicate definition bodies must survive \
             (the exact tie-break is #2025's policy call) — got \
             first={has_first} second={has_second}: {full_text:?}"
        );

        let mut elements = Vec::new();
        collect_elements(&h, &mut elements);
        let markers: Vec<&&HastNode> = elements
            .iter()
            .filter(|e| {
                flatten_text(e) == "1" && attr(e, "href").is_some_and(|href| href.starts_with('#'))
            })
            .collect();
        assert_eq!(
            markers.len(),
            1,
            "duplicate definitions must still yield exactly one reference \
             marker, got {}: {h:#?}",
            markers.len()
        );
    }

    // 5. A reference with NO matching definition is not even a
    // `FootnoteReference` mdast node: markdown-rs's footnote constructs
    // only recognise `[^label]` as a reference when a matching
    // `[^label]: …` definition exists elsewhere in the document;
    // otherwise the bracketed text parses as ordinary literal text (a
    // parser-level fact, confirmed by inspecting the mdast tree
    // directly — verified via a throwaway `zfb-content` example during
    // this issue's investigation). The catch-all bug this epic is
    // about therefore never runs for this case, so this is a passing
    // (NOT `#[ignore]`d) characterization pin, not a RED test — it
    // satisfies #2023's "do not leave [the missing-definition case]
    // untested" instruction by pinning the already-correct behaviour.
    #[test]
    fn unmatched_reference_stays_literal_text() {
        let h = run_with_footnotes("Dangling ref[^missing] end.\n");
        let children = root_children(&h);
        assert_eq!(
            children.len(),
            1,
            "no phantom footnote section should be appended: {h:#?}"
        );
        let (_, p_children, _) = assert_element(&children[0], "p");
        let text: String = p_children.iter().map(flatten_text).collect();
        assert!(
            text.contains("[^missing]"),
            "unmatched footnote reference must stay literal text, got: {text:?}"
        );
    }

    // ---- gfm: false must render byte-identically to before this fix ----
    //
    // The task-list wave (#2024) achieved this by construction: emission
    // is gated on a field only the GFM tokenizer populates, so the flag-
    // off path never sees it. Footnotes get the same property from
    // markdown-rs itself: with `footnote_definition` off,
    // `[^a]`/`[^a]: …` never parse into `FootnoteReference`/
    // `FootnoteDefinition` mdast nodes at all (they stay literal bracket
    // text — same parser fact `unmatched_reference_stays_literal_text`
    // above pins for the on-but-unmatched case), so `FootnoteModel::
    // collect` walks a tree with zero footnote nodes, produces an empty
    // model, and `mdast_to_hast_with`'s `if !model.is_empty()` guard
    // never appends a section. The new match arms in
    // `mdast_to_hast_inner` are therefore unreachable on this path,
    // proven here rather than assumed: no footnote section, no
    // `data-footnote-*` attribute anywhere, and the literal bracket
    // syntax survives verbatim — exactly pre-fix behaviour.
    #[test]
    fn gfm_false_leaves_footnote_syntax_as_literal_text_with_no_section_appended() {
        let h = run("Ref[^a] end.\n\n[^a]: Definition body.\n");
        let children = root_children(&h);
        // Two ordinary paragraphs (`footnote_definition` off means
        // `[^a]: Definition body.` is not even recognised as a
        // footnote-definition block — it stays a plain paragraph).
        // Neither is a `<section data-footnotes>`.
        assert_eq!(
            children.len(),
            2,
            "no footnote section may be appended when footnote_definition \
             is off: {h:#?}"
        );
        assert!(
            children.iter().all(|c| assert_element(c, "p").1.len() == 1),
            "both lines must stay ordinary paragraphs, no footnote \
             section: {h:#?}"
        );

        let mut elements = Vec::new();
        collect_elements(&h, &mut elements);
        assert!(
            elements.iter().all(|e| {
                attr(e, "data-footnote-ref").is_none()
                    && attr(e, "data-footnote-backref").is_none()
                    && attr(e, "data-footnotes").is_none()
            }),
            "no footnote-shaped attribute may appear anywhere when the \
             construct is off: {h:#?}"
        );

        let text = flatten_text(&h);
        assert!(
            text.contains("[^a]") && text.contains("[^a]: Definition body."),
            "footnote syntax must survive as literal text when \
             footnote_definition is off, got: {text:?}"
        );
    }

    // ---- the catch-all still swallows what it always has ----
    //
    // Taking footnotes OUT of `_ => HastNode::Raw(String::new())` must
    // not weaken it for the node kinds it legitimately still owns. A
    // reference-style link definition (`[label]: /url "title"`,
    // `MdastNode::Definition`) is one such kind: unlike footnotes,
    // nothing in this module has ever rendered it, and that has not
    // changed here — it still degrades to an empty `Raw` node, same as
    // before this fix.
    #[test]
    fn catch_all_still_swallows_reference_style_link_definitions() {
        let h = run("Para text.\n\n[label]: /elsewhere \"Title\"\n");

        let text = flatten_text(&h);
        assert!(
            text.contains("Para text."),
            "the surrounding paragraph must still render: {text:?}"
        );
        assert!(
            !text.contains("/elsewhere") && !text.contains("Title"),
            "the definition's url/title must not leak into rendered text \
             (still silently dropped by the catch-all), got: {text:?}"
        );

        let mut elements = Vec::new();
        collect_elements(&h, &mut elements);
        assert!(
            elements.iter().all(|e| attr(e, "href").is_none()),
            "a reference-style link definition must not render as any \
             element carrying an href: {h:#?}"
        );
    }

    // ---- the footnote label survives the real hast visitor chain ----
    //
    // `/codex-review` (issue #2026 self-review) caught this: the four
    // tests above all use `run_with_footnotes`, which builds a bare
    // `Pipeline::with_resolved_gfm_constructs` with NO hast visitors
    // registered — so none of them would have caught
    // `HeadingLinksPlugin` (always wired by `Pipeline::with_defaults*`,
    // which every real project uses) silently overwriting the footnote
    // label's `id="footnote-label"` with a content-derived slug and
    // appending a permalink anchor, since it treats any `<h2>`–`<h6>`
    // element as a document heading with no opt-out. That would break
    // every reference's `aria-describedby="footnote-label"` binding.
    // The fix renders the label as `<div role="heading"
    // aria-level="2">` — a real tag `HeadingLinksPlugin`/`TocPlugin`
    // never match — instead of an actual `<h2>`. This test runs the
    // FULL default plugin chain (`Pipeline::with_defaults_and_gfm`) to
    // prove the id survives untouched and no permalink anchor leaks in.
    #[test]
    fn footnote_label_id_survives_the_default_heading_links_plugin() {
        let mut p = Pipeline::with_defaults_and_gfm(ResolvedGfmConstructs::ALL_ON);
        let h = p
            .run("Ref one[^a] end.\n\n[^a]: Definition body.\n")
            .expect("parse ok");

        let mut elements = Vec::new();
        collect_elements(&h, &mut elements);

        let label = elements
            .iter()
            .find(|e| attr(e, "id") == Some(FOOTNOTE_LABEL_ID))
            .copied()
            .unwrap_or_else(|| {
                panic!(
                    "no element with id=\"{FOOTNOTE_LABEL_ID}\" survived the \
                     default hast visitor chain: {h:#?}"
                )
            });
        assert_eq!(
            flatten_text(label),
            FOOTNOTE_LABEL_TEXT,
            "HeadingLinksPlugin must not have rewritten the label's text: {label:#?}"
        );

        let marker = elements
            .iter()
            .find(|e| flatten_text(e) == "1" && attr(e, "href").is_some_and(|h| h.starts_with('#')))
            .copied()
            .unwrap_or_else(|| panic!("expected a footnote reference marker element in {h:#?}"));
        assert_eq!(
            attr(marker, "aria-describedby"),
            Some(FOOTNOTE_LABEL_ID),
            "the reference's aria-describedby must still resolve to the \
             unmodified label id: {marker:#?}"
        );
    }

    // ---- epic #2021 review fixes ----

    #[test]
    fn the_task_list_checkbox_opens_the_items_own_paragraph() {
        // This pipeline never unwraps tight-list paragraphs, so a task
        // item's children are `[<p>label</p>]`. A checkbox inserted as a
        // SIBLING before that `<p>` renders on its own line above the
        // label; it must open the paragraph instead so the checkbox sits
        // beside its text, the way GitHub renders one.
        let h = run_with_footnotes("- [ ] Todo\n- [x] Done\n");

        let mut elements = Vec::new();
        collect_elements(&h, &mut elements);
        let items: Vec<&&HastNode> = elements
            .iter()
            .filter(|e| matches!(e, HastNode::Element { tag, .. } if tag == "li"))
            .collect();
        assert_eq!(items.len(), 2, "expected two list items: {h:#?}");

        for item in items {
            let HastNode::Element { children, .. } = item else {
                unreachable!("filtered to elements above");
            };
            assert_eq!(
                children.len(),
                1,
                "a task item's only child must be its own paragraph — the \
                 checkbox belongs INSIDE it, not beside it: {item:#?}"
            );
            let HastNode::Element {
                tag,
                children: paragraph_children,
                ..
            } = &children[0]
            else {
                panic!("expected the item's paragraph element: {item:#?}");
            };
            assert_eq!(tag, "p", "expected a paragraph, got <{tag}>: {item:#?}");
            let checkbox = &paragraph_children[0];
            let HastNode::Element { tag: first_tag, .. } = checkbox else {
                panic!("expected the checkbox to open the paragraph: {item:#?}");
            };
            assert_eq!(first_tag, "input", "expected the checkbox: {item:#?}");
            assert_eq!(
                attr(checkbox, "type"),
                Some("checkbox"),
                "expected a checkbox input: {item:#?}"
            );
            assert!(
                matches!(&paragraph_children[1], HastNode::Text(t) if t == " "),
                "a single space must separate the checkbox from its label: {item:#?}"
            );
        }
    }

    #[test]
    fn the_footnote_label_is_hidden_by_an_inline_style_not_a_project_css_class() {
        // `sr-only` is a Tailwind utility and this class is emitted from
        // Rust, so Tailwind's content scan never sees the string and never
        // generates the utility; zfb ships no stylesheet defining it
        // either. The inline style is what makes the documented "visually
        // hidden" landmark actually true.
        let h = run_with_footnotes("Ref[^a].\n\n[^a]: Body.\n");
        let mut elements = Vec::new();
        collect_elements(&h, &mut elements);
        let label = elements
            .iter()
            .find(|e| attr(e, "id") == Some(FOOTNOTE_LABEL_ID))
            .unwrap_or_else(|| panic!("expected the footnote label: {h:#?}"));
        assert_eq!(
            attr(label, "style"),
            Some(FOOTNOTE_LABEL_STYLE),
            "the footnote label must carry the visually-hidden inline \
             style: {label:#?}"
        );
        assert_eq!(
            attr(label, "class"),
            Some("sr-only"),
            "the sr-only class stays as a styling hook alongside the \
             inline style: {label:#?}"
        );
    }

    #[test]
    fn the_model_and_the_emit_walk_cover_the_same_reference_set() {
        // The positive half of the walk-parity `debug_assert_eq!` in
        // `mdast_to_hast_with`: for a document mixing a top-level
        // reference, a repeated one, and one nested inside a definition
        // body, the model's total and the cursor's consumed count agree —
        // so the guard is meaningful rather than vacuously true.
        let src = "Top[^a] and again[^a], plus[^b].\n\n\
                   [^a]: A body citing[^c].\n\n[^b]: B body.\n\n[^c]: C body.\n";
        let options = markdown::ParseOptions {
            constructs: constructs_for_pipeline(ResolvedGfmConstructs::ALL_ON),
            ..markdown::ParseOptions::mdx()
        };
        let root = markdown::to_mdast(src, &options).expect("parse ok");
        let model = FootnoteModel::collect(&root);
        assert_eq!(
            model.total_references(),
            4,
            "expected 4 reference occurrences (a×2, b, c): {model:#?}"
        );

        let fc = FootnoteRenderCtx::new(&model);
        let strategy_fn = |_: &MdastNode, _: &FootnoteRenderCtx<'_>| -> String { String::new() };
        let strategy = JsxEmitStrategy::JsxPath(&strategy_fn);
        // Re-runs the same walk `mdast_to_hast_with` does (including the
        // footnote section, where the nested reference is claimed).
        let MdastNode::Root(r) = &root else {
            panic!("expected a Root node");
        };
        for child in &r.children {
            let _ = mdast_to_hast_inner(child, &strategy, &fc);
        }
        let _ = render_footnote_section(&model, &strategy, &fc);
        assert_eq!(
            fc.consumed_total(),
            model.total_references(),
            "the emit walk must claim every occurrence the model recorded"
        );
    }

    /// A reference stranded in a subtree the emitter drops through its
    /// catch-all trips the walk-parity guard. `LinkReference` is the
    /// concrete gap: `collect_reference_order` recurses into it, while
    /// `mdast_to_hast_inner` drops it whole.
    ///
    /// Debug-only: `debug_assert_eq!` compiles out in release, so this
    /// `should_panic` expectation only holds with debug assertions on.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "footnote reference occurrences")]
    fn a_reference_in_a_dropped_subtree_trips_the_walk_parity_guard() {
        use markdown::mdast;

        // Hand-built, because markdown-rs cannot currently produce this
        // shape — which is exactly why it needs a guard rather than a
        // rendering test.
        let stranded = MdastNode::LinkReference(mdast::LinkReference {
            children: vec![MdastNode::FootnoteReference(mdast::FootnoteReference {
                identifier: "a".to_string(),
                label: Some("a".to_string()),
                position: None,
            })],
            position: None,
            reference_kind: mdast::ReferenceKind::Full,
            identifier: "l".to_string(),
            label: Some("l".to_string()),
        });
        let definition = MdastNode::FootnoteDefinition(mdast::FootnoteDefinition {
            children: vec![MdastNode::Paragraph(mdast::Paragraph {
                children: vec![MdastNode::Text(mdast::Text {
                    value: "Body.".to_string(),
                    position: None,
                })],
                position: None,
            })],
            position: None,
            identifier: "a".to_string(),
            label: Some("a".to_string()),
        });
        let root = MdastNode::Root(mdast::Root {
            children: vec![
                MdastNode::Paragraph(mdast::Paragraph {
                    children: vec![stranded],
                    position: None,
                }),
                definition,
            ],
            position: None,
        });

        let strategy_fn = |_: &MdastNode, _: &FootnoteRenderCtx<'_>| -> String { String::new() };
        let _ = mdast_to_hast_with(&root, &JsxEmitStrategy::JsxPath(&strategy_fn));
    }

    // ── issue #2396: footnote-aware `reconstruct_jsx` on the HTML path ──
    //
    // Before this fix, `reconstruct_jsx`'s catch-all (`other =>
    // other.to_string()`) silently dropped a `FootnoteReference` nested
    // inside a JSX body (`to_string()` treats it as a "void", rendering
    // `""`) while the footnote SECTION still appended a
    // `data-footnote-backref` pointing at an anchor that was never
    // emitted, and inlined a nested `FootnoteDefinition`'s body as plain
    // text at the definition point AND rendered it again in the
    // section. `subtree_contains_footnote` + `jsx_body_stringify`
    // fix both defects for exactly the subtrees that contain a footnote,
    // leaving every other subtree on the byte-identical `to_string()`
    // path.

    /// Repro shape 1 (issue #2396's primary repro): a `note`-directive
    /// body reaching `MdxJsxFlowElement` via `DirectiveRegistry::
    /// reparse_block`, which produces a `FootnoteReference` nested
    /// INSIDE a `Paragraph` — not as a direct JSX child. `reparse_block`
    /// is only reachable when the collapsed single-Paragraph shape
    /// (`single_text_collapsed`) sees the whole `:::note … :::` run as
    /// ONE merged `Text` child, which requires the TOP-LEVEL parse to
    /// never split it — so the pipeline's own top-level constructs run
    /// with `footnote_definition` OFF (leaving `[^n]` / `[^n]: …` as
    /// inert literal text there, since a real `FootnoteDefinition`
    /// always interrupts the merged paragraph) while the registry's own
    /// `with_gfm` turns `footnote_definition` ON for the re-parse of the
    /// extracted body text. This is a deliberately constructed test
    /// pipeline (not the production `secondary_parse_gfm` wiring, which
    /// always mirrors the pipeline's own constructs) built to exercise
    /// `reparse_block` directly and deterministically.
    #[test]
    fn directive_body_footnote_reference_and_definition_are_associated_on_html_path() {
        use crate::plugins::directives::DirectiveRegistry;

        let mut p = Pipeline::with_resolved_gfm_constructs(ResolvedGfmConstructs::ALL_OFF);
        let mut registry = DirectiveRegistry::new().with_gfm(ResolvedGfmConstructs::ALL_ON, false);
        registry.register(zfb_md_ast::DirectiveDef {
            name: "note".to_string(),
            kind: zfb_md_ast::DirectiveKind::Container,
            component_name: "Note".to_string(),
            title_from_label: false,
            attrs: Vec::new(),
        });
        p.add_mdast_visitor(registry.into_visitor());
        let src = ":::note\nSome text.[^n]\n[^n]: the note body.\n:::\n";
        let h = p.run(src).expect("parse ok");

        let jsx = note_jsx(&h);

        // The reference marker exists inside the JSX body with an href
        // pointing at a real `#`-anchor (not dropped by the catch-all),
        // and the definition renders exactly once in the appended
        // section rather than inline.
        let definition_target = assert_footnote_renders_once(&h, &jsx, "n", "the note body.");
        assert!(
            jsx.contains("data-footnote-ref"),
            "reference marker must carry data-footnote-ref: {jsx:?}"
        );
        let marker_id = "user-content-fnref-n";
        assert!(
            jsx.contains(&format!("id=\"{marker_id}\"")),
            "reference marker must carry its own occurrence id: {jsx:?}"
        );

        // The backref round-trips: the definition's own subtree carries
        // a backreference link pointing at the marker's id.
        let mut def_subtree = Vec::new();
        collect_elements(definition_target, &mut def_subtree);
        let expected_backref = format!("#{marker_id}");
        assert!(
            def_subtree
                .iter()
                .any(|e| attr(e, "href") == Some(expected_backref.as_str())),
            "definition must contain a backreference link to {expected_backref}: {definition_target:#?}"
        );
    }

    /// Repro shape 2: literal `<Note>` JSX syntax (blank-line-separated
    /// body) produces `FootnoteReference`/`FootnoteDefinition` as DIRECT
    /// children of the `MdxJsxFlowElement` — no `reparse_block` involved
    /// at all, proving the fix is not shape-specific.
    #[test]
    fn literal_jsx_footnote_reference_and_definition_are_associated_on_html_path() {
        let h = run_with_footnotes("<Note>\n\nSome text.[^n]\n\n[^n]: the note body.\n\n</Note>\n");

        let jsx = note_jsx(&h);
        assert_footnote_renders_once(&h, &jsx, "n", "the note body.");
    }

    /// Container sweep: a footnote reference nested one level deeper
    /// inside every container variant that can appear in a JSX body
    /// (Paragraph, Table cell, Blockquote, ListItem, Emphasis, and a
    /// nested JSX element) still renders its marker instead of being
    /// dropped. This is also the positive-coverage gate for extending
    /// the walk-parity `debug_assert_eq!` to the HTML path — see the
    /// `debug_assert_eq!` gate in `mdast_to_hast_with` for the decision
    /// this test enables.
    #[test]
    fn container_sweep_footnote_reference_renders_in_every_variant_nested_in_jsx() {
        let src = "<Note>\n\n\
                   Paragraph text.[^p]\n\n\
                   | Col |\n| --- |\n| Cell.[^t] |\n\n\
                   > Quote.[^b]\n\n\
                   - Item.[^l]\n\n\
                   *Emph.[^e]*\n\n\
                   <Inner>\n\nNested.[^j]\n\n</Inner>\n\n\
                   </Note>\n\n\
                   [^p]: P body.\n\n[^t]: T body.\n\n[^b]: B body.\n\n\
                   [^l]: L body.\n\n[^e]: E body.\n\n[^j]: J body.\n";
        let h = run_with_footnotes(src);

        let jsx = note_jsx(&h);

        for label in ["p", "t", "b", "l", "e", "j"] {
            assert!(
                jsx.contains(&format!("<sup><a href=\"#user-content-fn-{label}\"")),
                "expected a footnote marker for [^{label}] nested inside JSX: {jsx:?}"
            );
        }

        // The model and the emit walk agree: every container variant's
        // reference was claimed, none stranded (the same invariant the
        // `debug_assert_eq!` below checks, verified positively here
        // against a REAL parse rather than a hand-built tree). All 6
        // markers live inside the single JsxRaw string (`collect_elements`
        // does not descend into it), so count occurrences directly.
        assert_eq!(
            jsx.matches("data-footnote-ref").count(),
            6,
            "expected exactly 6 reference markers, one per container variant: {jsx:?}"
        );
    }

    /// Scope-guard negative control (acceptance criterion 4): a
    /// reference-style link definition nested in JSX, with NO footnote
    /// anywhere in the subtree, is still silently swallowed by the
    /// catch-all — `subtree_contains_footnote` is `false` so the arm
    /// takes the untouched `other.to_string()` path, true by
    /// construction. Mirrors `catch_all_still_swallows_reference_style_
    /// link_definitions` above, scoped to inside JSX.
    #[test]
    fn catch_all_still_swallows_reference_style_link_definitions_nested_in_jsx() {
        let h = run("<Note>\n\nPara text.\n\n[label]: /elsewhere \"Title\"\n\n</Note>\n");

        let jsx = note_jsx(&h);
        assert!(
            jsx.contains("Para text."),
            "the surrounding paragraph must still render: {jsx:?}"
        );
        assert!(
            !jsx.contains("/elsewhere") && !jsx.contains("Title"),
            "the definition's url/title must not leak into rendered text \
             (still silently dropped by the catch-all), got: {jsx:?}"
        );
    }

    fn directive_body_footnote_and_cjk_pipeline() -> (Pipeline, String) {
        let mut directives = std::collections::HashMap::new();
        directives.insert(
            "note".to_string(),
            zfb_md_ast::DirectiveSpec::Short("Note".to_string()),
        );
        let features = zfb_md_ast::MarkdownFeaturesConfig {
            directives: Some(directives),
            ..Default::default()
        };
        let p = Pipeline::with_defaults_and_full_config(
            None,
            ResolvedGfmConstructs::ALL_ON,
            None,
            true,  // cjk_friendly
            false, // hard_breaks — orthogonal to this test
            Some(&features),
        )
        .expect("pipeline builds");
        // The CJK fixture is `zfb_md_ast::cjk_friendly`'s own canonical
        // repro rather than a fourth hand-copy of it (zfb#2402).
        let repro = zfb_md_ast::cjk_friendly::FLANKED_EMPHASIS_REPRO;
        let src = format!(":::note\n\n{repro}[^n]\n\n[^n]: the note body.\n\n:::\n");
        (p, src)
    }

    /// Cross-fix interaction (zfb#2399): a directive body carrying BOTH a
    /// footnote reference/definition (zfb#2396) and CJK-flanked emphasis
    /// (zfb#2398), checked at the mdast level. Deliberately BLANK-LINE-
    /// SEPARATED `:::note` syntax (not the collapsed, blank-line-less
    /// form) — the main top-level parse produces an ordinary `Paragraph`
    /// for the directive's body BEFORE `DirectiveRegistry` ever wraps it
    /// into an `MdxJsxFlowElement`, so `CjkFriendlyPlugin` (chain index
    /// 0, which deliberately does NOT descend into JSX bodies — see its
    /// module doc, "JSX bodies are author-controlled") corrects the
    /// emphasis while it is still a plain Paragraph child, exactly like
    /// `transcluded_cjk_emphasis_flanking_is_corrected` in
    /// `gfm_secondary_parse_sites.rs` does for the transclude site. This
    /// also sidesteps the COLLAPSED-directive-body pre-emption zfb#2401
    /// documents for the (unrelated) `reparse_block` call site — this
    /// test never reaches `reparse_block` at all.
    ///
    /// Checked at the mdast level (not the rendered HTML string)
    /// deliberately: `reconstruct_jsx`'s fallback arm — `footnote_aware_
    /// stringify` when the subtree contains a footnote, `other.to_
    /// string()` otherwise — has ALWAYS discarded Strong/Emphasis
    /// wrapper syntax for a JSX-body child, footnote or no footnote (see
    /// #2396's "Out of scope" section: "everything except footnotes must
    /// stay exactly as lossy as today"). Asserting `<strong>` in the
    /// serialized JsxRaw string would measure that pre-existing,
    /// unrelated lossiness, not whether the two fixes compose — so this
    /// replays `Pipeline::run`'s own mdast-phase half (parse +
    /// `mdast_visitors` chain, no hast conversion) and inspects the tree
    /// directly.
    #[test]
    fn directive_body_footnote_and_cjk_emphasis_compose_at_the_mdast_level() {
        let (p, src) = directive_body_footnote_and_cjk_pipeline();
        let mut p = p;
        let mut mdast = markdown::to_mdast(&src, &p.parse_options).expect("parse ok");
        for v in &mut p.mdast_visitors {
            v.set_secondary_parse_target(SecondaryParseTarget::Html);
            v.visit(&mut mdast);
        }

        fn find_note(node: &MdastNode) -> Option<&markdown::mdast::MdxJsxFlowElement> {
            if let MdastNode::MdxJsxFlowElement(j) = node {
                if j.name.as_deref() == Some("Note") {
                    return Some(j);
                }
            }
            node.children()?.iter().find_map(find_note)
        }
        let note = find_note(&mdast)
            .unwrap_or_else(|| panic!("expected a <Note> MdxJsxFlowElement: {mdast:#?}"));

        fn has_corrected_strong(nodes: &[MdastNode]) -> bool {
            nodes.iter().any(|n| match n {
                MdastNode::Strong(s) => s
                    .children
                    .iter()
                    .any(|c| matches!(c, MdastNode::Text(t) if t.value == "重要。")),
                _ => n
                    .children()
                    .is_some_and(|children| has_corrected_strong(children)),
            })
        }
        assert!(
            has_corrected_strong(&note.children),
            "CJK emphasis must be corrected into a real Strong node inside \
             the directive body, alongside a footnote: {:#?}",
            note.children
        );

        fn has_footnote_ref(nodes: &[MdastNode]) -> bool {
            nodes.iter().any(|n| match n {
                MdastNode::FootnoteReference(r) if r.identifier == "n" => true,
                _ => n
                    .children()
                    .is_some_and(|children| has_footnote_ref(children)),
            })
        }
        assert!(
            has_footnote_ref(&note.children),
            "expected a FootnoteReference(\"n\") inside the directive body \
             alongside the corrected CJK emphasis: {:#?}",
            note.children
        );
    }

    /// The HTML-rendering half of the same cross-fix interaction: the
    /// footnote marker actually renders and the definition renders
    /// exactly once, and the walk-parity `debug_assert_eq!` in
    /// `mdast_to_hast_with` does not fire for a subtree that also
    /// carries CJK-corrected emphasis — this test running to completion
    /// without panicking (a debug build) IS that half of the acceptance
    /// criterion.
    #[test]
    fn directive_body_footnote_and_cjk_emphasis_render_on_the_html_path() {
        let (mut p, src) = directive_body_footnote_and_cjk_pipeline();
        let h = p.run(&src).expect("parse ok");

        // The whole point of THIS test is that the association triple
        // still holds for a JSX body that also carries CJK-corrected
        // emphasis — same assertions, different subject.
        let jsx = note_jsx(&h);
        assert_footnote_renders_once(&h, &jsx, "n", "the note body.");
    }

    // ── zfb#2247: shared mdast-phase wiring for the JSX-nested mutation
    // stubs (#2248 ImageDimensions, #2249 ExternalLinks). Infrastructure
    // only — both stubs are no-ops; these tests pin registration wiring,
    // not behavior.

    #[test]
    fn image_dimensions_config_populates_jsx_nested_image_dimensions_field() {
        let features = zfb_md_extras::MarkdownFeaturesConfig {
            image_dimensions: Some(zfb_md_ast::ImageDimensionsConfig { skip_remote: None }),
            ..Default::default()
        };
        let p = Pipeline::with_defaults_and_features(&features);
        assert!(
            p.jsx_nested_image_dimensions.is_some(),
            "imageDimensions config must populate jsx_nested_image_dimensions"
        );
    }

    #[test]
    fn absent_image_dimensions_config_leaves_jsx_nested_image_dimensions_none() {
        let features = zfb_md_extras::MarkdownFeaturesConfig::default();
        let p = Pipeline::with_defaults_and_features(&features);
        assert!(
            p.jsx_nested_image_dimensions.is_none(),
            "absent imageDimensions config must leave jsx_nested_image_dimensions None"
        );
    }

    #[test]
    fn add_external_links_populates_jsx_nested_external_links_field() {
        let mut p = Pipeline::new();
        assert!(
            p.jsx_nested_external_links.is_none(),
            "field must start None before add_external_links is called"
        );
        p.add_external_links(ExternalLinksConfig::default(), Some("https://example.com"));
        assert!(
            p.jsx_nested_external_links.is_some(),
            "add_external_links must populate jsx_nested_external_links"
        );
    }

    #[test]
    fn absent_external_links_call_leaves_jsx_nested_external_links_none() {
        let p = Pipeline::new();
        assert!(p.jsx_nested_external_links.is_none());
    }

    #[test]
    fn jsx_nested_registration_keeps_config_fingerprint_some() {
        let features = zfb_md_extras::MarkdownFeaturesConfig {
            image_dimensions: Some(zfb_md_ast::ImageDimensionsConfig { skip_remote: None }),
            ..Default::default()
        };
        let mut p = Pipeline::with_defaults_and_features(&features);
        assert!(
            p.jsx_nested_image_dimensions.is_some(),
            "sanity: the field this test's fingerprint claim depends on"
        );
        assert!(
            p.config_fingerprint().is_some(),
            "config_fingerprint must stay Some after imageDimensions registration"
        );

        p.add_external_links(ExternalLinksConfig::default(), Some("https://example.com"));
        assert!(
            p.jsx_nested_external_links.is_some(),
            "sanity: the field this test's fingerprint claim depends on"
        );
        assert!(
            p.config_fingerprint().is_some(),
            "config_fingerprint must stay Some after add_external_links registration"
        );
    }
}
