//! Content query Rust contract for the Rust → JS bridge.
//!
//! This module defines the serializable shape that flows from the Rust
//! build into the JS runtime so TSX page modules can call
//! `getCollection("docs")` and `getEntry("docs", "slug")` synchronously
//! during SSR. The actual JS-side wiring (embedding the snapshot into
//! `globalThis.__zfb.content` via the embedded V8 host)
//! lands in a follow-up epic — this module delivers ONLY the Rust contract
//! and the TypeScript surface.
//!
//! # Determinism
//!
//! [`build_snapshot`] is deterministic: collections are keyed in a
//! [`BTreeMap`] (sorted ascending by collection name) and each
//! collection's `Vec<EntrySnapshot>` is sorted ascending by slug. Two
//! calls with the same `[CollectionConfig]` input always produce
//! byte-identical `serde_json` output, which the build cache relies on.
//!
//! # Why a parallel walker
//!
//! [`crate::collection::walk_collection`] is generic over `T: Validate`,
//! which is exactly what build-time consumers want when they *have* a
//! schema. The bridge does not — it ships the raw frontmatter as
//! `serde_json::Value` so the JS side can read whatever fields a TSX
//! page asks for. To reuse the existing walker without duplicating the
//! filesystem-traversal logic, we wrap [`serde_json::Value`] in a
//! transparent `UntypedFrontmatter` newtype that implements
//! [`garde::Validate`] as a no-op, then convert each `Entry<UntypedFrontmatter>`
//! into an `EntrySnapshot`. This keeps `walk_collection` as the single
//! source of truth for slug derivation, rel_path computation,
//! frontmatter parsing, and `module_specifier` generation.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::collection::{
    walk_collection_with_cache_and_filter, CollectionError, CollectionFilter, Entry,
};
use crate::pipeline::Pipeline;

/// A single content entry, in the shape the JS bridge sees.
///
/// All paths are normalized to forward-slash strings so the JSON
/// serialization is platform-independent. `frontmatter` is the parsed
/// frontmatter as JSON (YAML for `.md`/`.mdx`, the `export const
/// frontmatter` literal for `.tsx`). `body` is the markdown body for
/// `.md`/`.mdx` and the empty string for `.tsx` (TSX has no separate
/// markdown body).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntrySnapshot {
    /// Slug from the file stem (no extension).
    pub slug: String,
    /// Parsed frontmatter as JSON. `Null` when the source had none.
    pub frontmatter: JsonValue,
    /// Markdown body. Empty string for TSX entries.
    pub body: String,
    /// Stable specifier addressing the compiled module
    /// (`mdx://collection/slug#hash` or `tsx://collection/slug#hash`).
    pub module_specifier: String,
    /// Path relative to the collection root, normalized to use `/`
    /// separators so JSON output is stable across OSes.
    pub rel_path: String,
}

/// A point-in-time snapshot of every configured collection.
///
/// Collections are stored in a [`BTreeMap`] so iteration order is
/// deterministic (sorted ascending by collection name). Each
/// collection's `Vec<EntrySnapshot>` is sorted ascending by slug.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentSnapshot {
    /// Sorted map of collection name → entries.
    pub collections: BTreeMap<String, Vec<EntrySnapshot>>,
}

/// Configuration for one content collection in the snapshot.
///
/// `name` is the public-facing collection name JS code uses with
/// `getCollection(name)`. `root` is the directory the walker scans —
/// any `.md`, `.mdx`, or `.tsx` file under it becomes an
/// [`EntrySnapshot`]. A non-existent `root` is treated as an empty
/// collection (matches [`walk_collection`] today; documented on
/// [`build_snapshot`]).
#[derive(Debug, Clone, Default)]
pub struct CollectionConfig {
    /// Public collection name (the key in [`ContentSnapshot::collections`]).
    pub name: String,
    /// Directory containing the collection's source files.
    pub root: PathBuf,
    /// Optional include globs (Astro-style). When `Some` and non-empty,
    /// the walker keeps only entries whose relative path matches at
    /// least one pattern. See [`crate::collection::CollectionFilter`]
    /// for the full semantics.
    pub include: Option<Vec<String>>,
    /// Optional exclude globs. Applied AFTER `include`. See
    /// [`crate::collection::CollectionFilter`].
    pub exclude: Option<Vec<String>>,
    /// Optional suffix to strip from each kept entry's slug + module
    /// specifier. MUST match the bundler's
    /// `ContentCollectionSpec::id_strip_suffix` exactly so the snapshot
    /// specifier and the bundler's bridge-map key stay byte-identical.
    pub id_strip_suffix: Option<String>,
}

impl CollectionConfig {
    /// Convenience constructor (no filters). Use field-init shorthand
    /// (`CollectionConfig { name, root, include: Some(..), .. }`) or
    /// the with_* builders for filtered collections.
    pub fn new(name: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            root: root.into(),
            include: None,
            exclude: None,
            id_strip_suffix: None,
        }
    }

    /// Builder: replace `include` globs.
    #[must_use]
    pub fn with_include(mut self, patterns: Option<Vec<String>>) -> Self {
        self.include = patterns;
        self
    }

    /// Builder: replace `exclude` globs.
    #[must_use]
    pub fn with_exclude(mut self, patterns: Option<Vec<String>>) -> Self {
        self.exclude = patterns;
        self
    }

    /// Builder: replace `id_strip_suffix`.
    #[must_use]
    pub fn with_id_strip_suffix(mut self, suffix: Option<String>) -> Self {
        self.id_strip_suffix = suffix;
        self
    }
}

/// Errors produced by [`build_snapshot`].
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// Underlying collection walker failed (IO, frontmatter parse, MDX
    /// compile, schema). The variant is per-collection so the caller
    /// can tell which collection blew up.
    #[error("collection `{collection}` failed to walk: {source}")]
    Walk {
        /// Name of the failing collection.
        collection: String,
        /// Source error from the walker.
        #[source]
        source: CollectionError,
    },
    /// Pipeline configuration failed (e.g. loading a `themesDir` before the
    /// walk starts).
    #[error("{0}")]
    PipelineConfig(String),
}

/// Pipeline-shape configuration for [`build_snapshot_with_config`].
///
/// The fields here mirror the knobs the bundler passes to
/// `Pipeline::with_defaults_and_theme` + `add_strip_md_ext` +
/// `add_resolve_links` in `crates/zfb-build/src/bundler.rs`. The
/// snapshot walker MUST construct its pipelines with the same shape so
/// the JSX `content_hash` it bakes into each `EntrySnapshot::module_specifier`
/// matches the bundler's bridge-map key byte-for-byte. Any divergence
/// here makes `bridge.get(specifier)` miss and dumps the page into a
/// `<pre data-zfb-content-fallback>` block.
///
/// `Default` reproduces the pre-config-API behaviour (no theme override,
/// no strip-md-ext, no resolve-links) so existing call sites that don't
/// need any of these knobs can keep using [`build_snapshot`].
#[derive(Debug, Clone)]
pub struct SnapshotPipelineConfig {
    /// Optional syntect theme name. Forwarded to
    /// [`Pipeline::with_defaults_and_theme`]. `None` keeps the built-in
    /// default theme.
    pub code_highlight_theme: Option<String>,
    /// Optional absolute path to a directory of `.tmTheme` files.
    /// Forwarded to
    /// [`Pipeline::with_defaults_and_theme_and_gfm_and_themes_dir`].
    /// `None` keeps syntect's bundled themes only.
    ///
    /// MUST match `BundlerInput::code_highlight_themes_dir` so the
    /// snapshot ↔ bundler `content_hash` values stay byte-identical.
    pub code_highlight_themes_dir: Option<std::path::PathBuf>,
    /// When true, append [`Pipeline::add_strip_md_ext`] to every
    /// per-collection pipeline. Match the bundler's `strip_md_ext`
    /// flag exactly.
    pub strip_md_ext: bool,
    /// When `Some`, wire [`Pipeline::add_resolve_links`] with this
    /// `(absolute path → URL)` map and call
    /// [`Pipeline::set_resolve_links_source_dir`] per file so internal
    /// `[link](./other.mdx)` references resolve to URLs. `None` skips
    /// the plugin entirely.
    pub resolve_source_map:
        Option<std::collections::HashMap<std::path::PathBuf, String>>,
    /// Resolved GFM construct flags (output of
    /// `zfb::config::resolve_gfm_constructs` / `MarkdownConfig::resolve_constructs`).
    /// MUST match what the bundler threads into its own
    /// `Pipeline::with_defaults_and_theme_and_gfm_and_cjk` call. Divergence
    /// here is the snapshot ↔ bundler hash divergence land mine
    /// documented above — flipping `gfm_strikethrough` on one side and
    /// off on the other shifts every snapshot's JSX `content_hash`,
    /// and every `<Content />` lookup falls back to
    /// `<pre data-zfb-content-fallback>`.
    ///
    /// `Default` is [`ResolvedGfmConstructs::CONSERVATIVE`] so existing
    /// tests + fixtures that don't construct this struct manually keep
    /// the same effective behaviour as before this field landed.
    pub gfm_constructs: super::pipeline::ResolvedGfmConstructs,

    /// When `Some`, wire [`TocPlugin`] into the hast phase immediately after
    /// `HeadingLinksPlugin`. MUST match the bundler's `markdown.toc` setting
    /// exactly — divergence shifts the JSX `content_hash` and every
    /// `<Content />` lookup falls back to `<pre data-zfb-content-fallback>`.
    ///
    /// `Default` is `None` (visitor not wired) for byte-for-byte parity with
    /// the pre-TOC build.
    pub toc: Option<super::plugins::toc::TocConfig>,
    /// When `Some`, append [`Pipeline::add_external_links`] with the given
    /// config and optional site origin so external `<a>` elements are
    /// annotated with `target` / `rel`. `None` (the default) skips the
    /// plugin entirely — byte-for-byte identical to the pre-feature build.
    ///
    /// Mirrors `markdown.externalLinks` in `zfb.config.ts`.
    pub external_links: Option<(crate::plugins::ExternalLinksConfig, Option<String>)>,
    /// Whether to include [`CjkFriendlyPlugin`] in the mdast phase.
    /// MUST match the bundler's value (output of
    /// `zfb::config::resolve_cjk_friendly(config.markdown.as_ref())`).
    ///
    /// `Default` is `true` (plugin on) so existing call sites and tests
    /// that don't set this field keep the same effective behaviour as
    /// before this field landed.
    ///
    /// [`CjkFriendlyPlugin`]: super::plugins::CjkFriendlyPlugin
    pub cjk_friendly: bool,

    /// `markdown.features` from `zfb.config.ts`. MUST match the bundler's
    /// `BundlerInput::markdown_features` exactly — the feature-aware pipeline
    /// changes which visitors fire, so any divergence shifts the JSX
    /// `content_hash` and every `<Content />` lookup falls back to
    /// `<pre data-zfb-content-fallback>`.
    ///
    /// `Default` is `None` — no feature surface configured — which is treated
    /// as an empty feature set: the three former-Core framework features
    /// (mermaid, admonitions-preset, heading-marker TOC) are
    /// OFF (the post-epic opt-in default, #583 / #586).
    pub features: Option<zfb_md_extras::MarkdownFeaturesConfig>,
}

impl Default for SnapshotPipelineConfig {
    fn default() -> Self {
        Self {
            code_highlight_theme: None,
            code_highlight_themes_dir: None,
            strip_md_ext: false,
            resolve_source_map: None,
            gfm_constructs: super::pipeline::ResolvedGfmConstructs::default(),
            cjk_friendly: true,
            toc: None,
            external_links: None,
            features: None,
        }
    }
}

impl SnapshotPipelineConfig {
    /// Construct a pipeline shaped by this config. Used by
    /// [`build_snapshot_with_config`] once per collection.
    fn build_pipeline(&self) -> Result<Pipeline, BridgeError> {
        // Single feature-aware entry point — MUST match the bundler's dispatch
        // (see `crates/zfb-build/src/bundler.rs`) so `content_hash` stays
        // byte-identical. `features = None` is an empty feature set: the three
        // former-Core framework features are off (the post-epic opt-in default).
        let mut p = Pipeline::with_defaults_and_full_config(
            self.code_highlight_theme.as_deref(),
            self.gfm_constructs,
            self.code_highlight_themes_dir.as_deref(),
            self.cjk_friendly,
            self.features.as_ref(),
        )
        .map_err(|e| BridgeError::PipelineConfig(format!("codeHighlight.themesDir: {e}")))?;
        if self.strip_md_ext {
            p.add_strip_md_ext();
        }
        if let Some(map) = self.resolve_source_map.as_ref() {
            p.add_resolve_links(map.clone());
        }
        if let Some(toc_cfg) = self.toc.clone() {
            p.add_toc(toc_cfg);
        }
        if let Some((cfg, site)) = self.external_links.as_ref() {
            p.add_external_links(cfg.clone(), site.as_deref());
        }
        Ok(p)
    }
}

/// Build a deterministic [`ContentSnapshot`] from the configured
/// collections, using the **default** pipeline shape (no theme override,
/// no strip-md-ext, no resolve-links).
///
/// **Most callers should prefer [`build_snapshot_with_config`]**, which
/// accepts a [`SnapshotPipelineConfig`] mirroring the bundler's
/// pipeline knobs. This zero-arg form exists for unit tests and
/// fixture-based call sites that don't enable any of the optional
/// pipeline plugins; it produces snapshot hashes that only match the
/// bundler when the bundler is also using the default pipeline shape.
///
/// See [`build_snapshot_with_config`] for full semantics.
///
/// # Errors
///
/// Returns [`BridgeError::Walk`] if any underlying walker call fails.
pub fn build_snapshot(collections: &[CollectionConfig]) -> Result<ContentSnapshot, BridgeError> {
    build_snapshot_with_config(collections, &SnapshotPipelineConfig::default())
}

/// Build a deterministic [`ContentSnapshot`] from the configured
/// collections, using a pipeline whose shape matches the bundler's.
///
/// Walks each collection in `collections` via [`walk_collection`],
/// driving the walker through a fresh-per-collection [`Pipeline`] built
/// from `pipeline_config` so the resulting JSX (and the
/// [`module_specifier`](EntrySnapshot::module_specifier) hash baked
/// from it) is **byte-identical** to what the bundler emits in
/// `crates/zfb-build/src/bundler.rs`. Snapshot specifiers MUST agree
/// with the bundler's bridge-map keys byte-for-byte; otherwise every
/// `globalThis.__zfb.content.get(specifier)` lookup misses and the
/// page renders the raw-markdown `<pre data-zfb-content-fallback>`
/// fallback.
///
/// The pipeline shape covered by [`SnapshotPipelineConfig`] must mirror
/// the bundler's three knobs (theme, strip-md-ext, resolve-links). When
/// any of them is enabled in the bundler but disabled here, the JSX
/// content hash diverges and the bridge lookup misses (zfb#188).
///
/// Per-collection instantiation matters because some default plugins
/// (notably `HeadingLinksPlugin`) carry per-document state (the
/// slug-dedupe `seen` map). Reusing one pipeline across collections
/// would mean a heading like "Intro" in collection B would slugify to
/// `intro-1` if collection A already used `intro`, leaking unrelated
/// collections' headings into B's compiled JSX (and into the hash that
/// drives `module_specifier`). The bundler's
/// `materialise_*` helpers also instantiate one pipeline per
/// collection; this walker matches that shape.
///
/// Each entry is converted into an [`EntrySnapshot`] and the resulting
/// `Vec` is inserted into a [`BTreeMap`] keyed by collection name.
///
/// Sort order:
/// - top-level: collection name ascending (free, courtesy of `BTreeMap`).
/// - per-collection: slug ascending (explicitly sorted; the walker's
///   own ordering is by `rel_path` which can differ from slug when
///   files live in nested directories).
///
/// # Missing collection roots
///
/// A `CollectionConfig` whose `root` does not exist on disk is treated
/// as an empty collection (zero entries). This matches the behavior of
/// [`walk_collection`] / `collect_collection_files`, which return
/// `Ok(())` for absent directories. Tests cover this contract so
/// regressions are loud.
///
/// # Errors
///
/// Returns [`BridgeError::Walk`] if any underlying walker call fails
/// (typically a malformed frontmatter, schema validation failure, or
/// MDX compile error inside one of the configured roots).
pub fn build_snapshot_with_config(
    collections: &[CollectionConfig],
    pipeline_config: &SnapshotPipelineConfig,
) -> Result<ContentSnapshot, BridgeError> {
    let mut out: BTreeMap<String, Vec<EntrySnapshot>> = BTreeMap::new();

    for cfg in collections {
        let mut pipeline = pipeline_config.build_pipeline()?;

        // The bridge ships frontmatter as raw JSON, so we walk with the
        // no-op `UntypedFrontmatter` schema. See module-level docs for
        // why the parallel walker is implemented as a `T` substitution
        // rather than a duplicate FS traversal.
        //
        // `walk_collection_with_cache_and_filter` resets the pipeline's
        // per-entry state before each file (zfb#188-followup), so any
        // stateful plugin in the chain — `HeadingLinksPlugin`'s slug
        // counter included — starts fresh per document, matching the
        // bundler's `materialise_*` walk loop.
        //
        // The configured include / exclude / idStripSuffix MUST match
        // the bundler's `ContentCollectionSpec` exactly — otherwise
        // the snapshot's `module_specifier` and the bundler's bridge
        // map key disagree and every `bridge.get(spec)` lookup misses.
        let filter = CollectionFilter::new(
            cfg.include.as_deref(),
            cfg.exclude.as_deref(),
            cfg.id_strip_suffix.as_deref(),
        )
        .map_err(|source| BridgeError::Walk {
            collection: cfg.name.clone(),
            source,
        })?;
        let entries: Vec<Entry<UntypedFrontmatter>> =
            walk_collection_with_cache_and_filter(&cfg.root, None, Some(&mut pipeline), &filter)
                .map_err(|source| BridgeError::Walk {
                    collection: cfg.name.clone(),
                    source,
                })?;

        let mut snapshots: Vec<EntrySnapshot> = entries
            .into_iter()
            .map(|e| EntrySnapshot {
                slug: e.slug,
                frontmatter: e.data.0,
                body: e.body,
                module_specifier: e.module_specifier,
                rel_path: rel_path_to_string(&e.rel_path),
            })
            .collect();

        // Sort by slug ascending. `walk_collection` already sorts by
        // rel_path, but slugs and rel_paths can disagree (nested dirs,
        // different file extensions on the same slug, etc.) so we
        // re-sort explicitly to match the documented bridge contract.
        snapshots.sort_by(|a, b| a.slug.cmp(&b.slug));

        out.insert(cfg.name.clone(), snapshots);
    }

    let snapshot = ContentSnapshot { collections: out };
    log_snapshot_size_if_requested(&snapshot);
    Ok(snapshot)
}

/// If `ZFB_DEBUG_SNAPSHOT` is set to a truthy value (`1` or `true`,
/// case-insensitive), print a one-line summary of the serialized
/// snapshot size to stderr. Anything else (unset, empty, `0`, `false`,
/// or unrecognized) is a no-op.
///
/// Used to monitor V8 RAM pressure: zfb embeds the full content
/// snapshot in the embedded V8 host at build time, so the byte size
/// reported here is roughly what gets held in V8 during render. See
/// the "Limits" section of the project README.
///
/// Failure is silent on purpose — debug telemetry must never panic the
/// build. If `serde_json::to_string` ever returns `Err` (it cannot for
/// our shape, which contains no NaN/non-string keys), we just skip the
/// log line.
fn log_snapshot_size_if_requested(snap: &ContentSnapshot) {
    if !debug_snapshot_enabled() {
        return;
    }
    let entries: usize = snap.collections.values().map(|v| v.len()).sum();
    let bytes = match serde_json::to_string(snap) {
        Ok(s) => s.len(),
        Err(_) => return,
    };
    // Round up so a 1-byte snapshot still reports as 1 KB rather than 0.
    let kb = bytes.div_ceil(1024);
    eprintln!("content snapshot: {entries} entries / {kb} KB");
}

/// Read `ZFB_DEBUG_SNAPSHOT` and decide whether to emit the debug line.
/// Truthy values: `1`, `true` (case-insensitive). Everything else —
/// including unset, empty string, and unrecognized values like `yes` —
/// is treated as off so a stray export does not change build output.
///
/// Public so the CLI can avoid building a snapshot purely for telemetry
/// when the flag is off.
pub fn debug_snapshot_enabled() -> bool {
    std::env::var("ZFB_DEBUG_SNAPSHOT")
        .ok()
        .as_deref()
        .map(parse_debug_truthy)
        .unwrap_or(false)
}

/// Pure parser for the `ZFB_DEBUG_SNAPSHOT` value. Split out from
/// [`debug_snapshot_enabled`] so the truthiness rule is testable
/// without mutating process-global env state.
fn parse_debug_truthy(raw: &str) -> bool {
    let t = raw.trim();
    t.eq_ignore_ascii_case("1") || t.eq_ignore_ascii_case("true")
}

// -----------------------------------------------------------------------------
// internals
// -----------------------------------------------------------------------------

/// Newtype wrapper around [`serde_json::Value`] that satisfies the
/// `T: DeserializeOwned + garde::Validate<Context = ()>` bound on
/// [`walk_collection`] without imposing any schema. `#[serde(transparent)]`
/// makes deserialization equivalent to deserializing the inner Value.
#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
struct UntypedFrontmatter(JsonValue);

impl garde::Validate for UntypedFrontmatter {
    type Context = ();

    fn validate_into(
        &self,
        _ctx: &Self::Context,
        _parent: &mut dyn FnMut() -> garde::Path,
        _report: &mut garde::Report,
    ) {
        // Bridge accepts any frontmatter shape — schema validation is a
        // build-time concern handled elsewhere (e.g. typed
        // walk_collection callers).
    }
}

/// Render a `Path` (relative) as a forward-slash string. `walk_collection`
/// builds rel_paths via `Path::strip_prefix`, which on Windows would emit
/// backslashes. Forcing `/` here keeps the JSON snapshot identical across
/// platforms — important for the determinism / cache-key contract.
fn rel_path_to_string(p: &std::path::Path) -> String {
    p.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

// -----------------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Self-cleaning temp dir helper (mirrors the one in `collection.rs`).
    struct TmpDir {
        path: PathBuf,
    }

    impl TmpDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!(
                "zfb-content-bridge-{label}-{nanos}-{n}-{pid}",
                pid = std::process::id()
            ));
            fs::create_dir_all(&dir).expect("create tmp dir");
            Self { path: dir }
        }
        fn write(&self, rel: &str, contents: &str) -> PathBuf {
            let p = self.path.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&p, contents).expect("write file");
            p
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn md(title: &str) -> String {
        format!("---\ntitle: \"{title}\"\n---\nbody for {title}\n")
    }

    fn tsx(title: &str) -> String {
        format!(
            "export const frontmatter = {{ title: '{title}' }};\n\
             export default function Page() {{ return null; }}\n"
        )
    }

    fn hash_canonical(snap: &ContentSnapshot) -> String {
        // serde_json on a `BTreeMap` emits keys in sorted order, and
        // each entry vec is already sorted by slug. The serializer is
        // therefore canonical for our purposes: identical input →
        // identical bytes.
        let s = serde_json::to_string(snap).expect("snapshot serializes");
        let mut h = Sha256::new();
        h.update(s.as_bytes());
        hex::encode(h.finalize())
    }

    #[test]
    fn build_snapshot_is_deterministic_under_repeated_calls() {
        let tmp = TmpDir::new("determinism");
        // Two collections, each with multiple entries. Mixed kinds.
        tmp.write("blog/alpha.md", &md("Alpha"));
        tmp.write("blog/zulu.mdx", &md("Zulu"));
        tmp.write("blog/mike.tsx", &tsx("Mike"));
        tmp.write("docs/intro.md", &md("Intro"));
        tmp.write("docs/advanced.mdx", &md("Advanced"));

        let cfgs = vec![
            CollectionConfig::new("blog", tmp.path.join("blog")),
            CollectionConfig::new("docs", tmp.path.join("docs")),
        ];

        let s1 = build_snapshot(&cfgs).expect("snapshot 1");
        let s2 = build_snapshot(&cfgs).expect("snapshot 2");

        assert_eq!(
            hash_canonical(&s1),
            hash_canonical(&s2),
            "two snapshot calls over the same fixture must hash identically",
        );
    }

    // #586 — `markdown.features` must thread into the snapshot pipeline. The
    // compiled JSX `content_hash` is baked into each entry's
    // `module_specifier`, so enabling a feature that changes the rendered
    // output (mermaid) must change that hash. This is the snapshot-side
    // counterpart to `bundler_default_plugins`'s features test in `zfb-build`;
    // both feed the SAME constructor, so matching dispatch keeps the snapshot ↔
    // bundler `content_hash` byte-identical.
    #[test]
    fn snapshot_threads_markdown_features_into_content_hash() {
        let tmp = TmpDir::new("md-features");
        // A mermaid fence is only transformed when `features.mermaid` is on.
        tmp.write(
            "docs/diagram.mdx",
            "---\ntitle: \"Diagram\"\n---\n\n```mermaid\ngraph TD; A-->B;\n```\n",
        );
        let collection = || CollectionConfig::new("docs", tmp.path.join("docs"));

        // Default (no features) → mermaid OFF.
        let off = build_snapshot_with_config(&[collection()], &SnapshotPipelineConfig::default())
            .expect("snapshot (features off)");
        // `features.mermaid: true` → mermaid ON.
        let on_cfg = SnapshotPipelineConfig {
            features: Some(zfb_md_extras::MarkdownFeaturesConfig {
                mermaid: Some(zfb_md_extras::FeatureToggle::Bool(true)),
                ..Default::default()
            }),
            ..Default::default()
        };
        let on = build_snapshot_with_config(&[collection()], &on_cfg)
            .expect("snapshot (features on)");

        let spec_off = &off.collections.get("docs").expect("docs off")[0].module_specifier;
        let spec_on = &on.collections.get("docs").expect("docs on")[0].module_specifier;
        assert_ne!(
            spec_off, spec_on,
            "enabling features.mermaid must change the snapshot's content_hash \
             (proves markdown.features threads into the snapshot pipeline)",
        );
    }

    #[test]
    fn build_snapshot_is_deterministic_under_reversed_config_order() {
        let tmp = TmpDir::new("determinism-reorder");
        tmp.write("a/one.md", &md("One"));
        tmp.write("b/two.md", &md("Two"));

        let forward = vec![
            CollectionConfig::new("a", tmp.path.join("a")),
            CollectionConfig::new("b", tmp.path.join("b")),
        ];
        let reversed = vec![
            CollectionConfig::new("b", tmp.path.join("b")),
            CollectionConfig::new("a", tmp.path.join("a")),
        ];

        // BTreeMap sorts keys → the resulting snapshots are equivalent
        // regardless of the order configs were supplied in.
        assert_eq!(
            hash_canonical(&build_snapshot(&forward).unwrap()),
            hash_canonical(&build_snapshot(&reversed).unwrap()),
            "config order must not affect snapshot bytes",
        );
    }

    #[test]
    fn entries_within_collection_are_sorted_by_slug_ascending() {
        let tmp = TmpDir::new("sort");
        // Names chosen so that `rel_path` ordering and slug ordering
        // agree here (single-level dir), but the explicit slug sort
        // would still fire if the walker behaved differently.
        tmp.write("docs/zebra.md", &md("Z"));
        tmp.write("docs/apple.md", &md("A"));
        tmp.write("docs/mango.md", &md("M"));

        let snap = build_snapshot(&[CollectionConfig::new("docs", tmp.path.join("docs"))])
            .expect("snapshot ok");
        let docs = snap.collections.get("docs").expect("docs present");
        let slugs: Vec<&str> = docs.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(slugs, vec!["apple", "mango", "zebra"]);
    }

    #[test]
    fn missing_collection_root_yields_empty_vec_not_error() {
        // Documented contract: aligning with `walk_collection`'s lenient
        // treatment of a non-existent dir. This test pins the choice.
        let tmp = TmpDir::new("missing");
        let cfg = CollectionConfig::new("ghost", tmp.path.join("does-not-exist"));
        let snap = build_snapshot(&[cfg]).expect("missing root must not error");
        let ghost = snap.collections.get("ghost").expect("ghost key present");
        assert!(ghost.is_empty(), "missing root → empty collection");
    }

    #[test]
    fn entry_snapshot_carries_frontmatter_body_and_specifier() {
        let tmp = TmpDir::new("shape");
        tmp.write("blog/post.md", &md("Hello"));
        tmp.write("blog/page.tsx", &tsx("Tsx"));

        let snap = build_snapshot(&[CollectionConfig::new("blog", tmp.path.join("blog"))])
            .expect("snapshot ok");
        let blog = snap.collections.get("blog").expect("blog present");
        assert_eq!(blog.len(), 2);

        let page = blog.iter().find(|e| e.slug == "page").expect("page entry");
        let post = blog.iter().find(|e| e.slug == "post").expect("post entry");

        // Frontmatter survives the round-trip as JSON.
        assert_eq!(post.frontmatter["title"].as_str(), Some("Hello"));
        assert_eq!(page.frontmatter["title"].as_str(), Some("Tsx"));

        // Markdown bodies are preserved; TSX bodies are empty.
        assert!(post.body.contains("body for Hello"));
        assert!(page.body.is_empty(), "tsx entries have empty body");

        // Module specifiers reflect the file kind.
        assert!(
            post.module_specifier.starts_with("mdx://"),
            "got {}",
            post.module_specifier,
        );
        assert!(
            page.module_specifier.starts_with("tsx://"),
            "got {}",
            page.module_specifier,
        );

        // rel_path is forward-slash normalised even on Windows.
        assert_eq!(post.rel_path, "post.md");
        assert_eq!(page.rel_path, "page.tsx");
    }

    #[test]
    fn snapshot_serde_roundtrip_preserves_shape() {
        let tmp = TmpDir::new("roundtrip");
        tmp.write("blog/x.md", &md("X"));

        let snap = build_snapshot(&[CollectionConfig::new("blog", tmp.path.join("blog"))])
            .expect("snapshot ok");

        let json = serde_json::to_string(&snap).expect("serialize");
        let back: ContentSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(hash_canonical(&snap), hash_canonical(&back));
    }

    #[test]
    fn malformed_collection_surfaces_bridge_error_with_collection_name() {
        let tmp = TmpDir::new("error");
        tmp.write("blog/bad.md", "---\ntitle: [unclosed\n---\nbody\n");
        let cfg = CollectionConfig::new("blog", tmp.path.join("blog"));
        let err = build_snapshot(&[cfg]).expect_err("malformed YAML must fail");
        match err {
            BridgeError::Walk { collection, .. } => assert_eq!(collection, "blog"),
            other => panic!("expected BridgeError::Walk, got: {other:?}"),
        }
    }

    #[test]
    fn debug_truthy_parser_accepts_only_documented_truthy_values() {
        // Documented truthy: `1`, `true` (case-insensitive, trim whitespace).
        for raw in ["1", " 1 ", "true", "TRUE", "True", "  true\n"] {
            assert!(parse_debug_truthy(raw), "{raw:?} should be truthy");
        }
        // Everything else is off — guards against accidental enabling
        // when a script sets `ZFB_DEBUG_SNAPSHOT=yes` or similar.
        for raw in ["", "0", "false", "no", "yes", "on", "off", "TRUE!"] {
            assert!(!parse_debug_truthy(raw), "{raw:?} should be falsy");
        }
    }

    #[test]
    fn snapshot_specifier_matches_bridge_hash_for_pipeline_transformed_entry() {
        // Regression guard for zfb #132: `build_snapshot` must drive
        // `walk_collection` through the same default `Pipeline` the
        // bundler uses, so the `module_specifier` baked into the
        // snapshot agrees byte-for-byte with the bridge map key the
        // bundler emits. Without the fix, `walk_collection` ran with
        // `pipeline = None` (pre-pipeline JSX, pre-pipeline hash) while
        // the bundler ran with `Some(&mut Pipeline::with_defaults())`
        // (post-pipeline JSX, post-pipeline hash). Any page whose body
        // the pipeline actually transformed — admonitions, mermaid,
        // syntect-highlighted fences, heading-links,
        // CJK-friendly emphasis — produced two divergent specifiers,
        // and `globalThis.__zfb.content.get(snapshot.module_specifier)`
        // missed at render time → silent fallback render.
        //
        // We reproduce that hazard with the cheapest signal we have:
        // a `:::note` admonition. `AdmonitionsPlugin` rewrites the
        // mdast subtree into an `<MdxJsxFlowElement name="Note">`, so
        // the post-pipeline JSX (and therefore its `content_hash`)
        // differs from the pre-pipeline JSX. Asserting that the
        // snapshot's hash component matches the hash an independent
        // `compile_mdx_to_jsx_module_cached(... Some(&mut Pipeline::with_defaults()))`
        // call produces is the explicit "snapshot hash matches bridge
        // hash" guarantee.
        use crate::mdx_jsx_emit::{compile_mdx_to_jsx_module_cached, parse_mdx_specifier};

        let tmp = TmpDir::new("snapshot-pipeline-hash");
        // The `:::note` directive is the canonical pipeline-transformable
        // signal — without the default pipeline it survives as raw text;
        // with it, it becomes an `<MdxJsxFlowElement name="Note">`.
        let mdx = "---\ntitle: \"Admon\"\n---\n\n:::note\nhi\n:::\n";
        let path = tmp.write("docs/admon.mdx", mdx);

        let snap = build_snapshot(&[CollectionConfig::new("docs", tmp.path.join("docs"))])
            .expect("build_snapshot must succeed");
        let docs = snap
            .collections
            .get("docs")
            .expect("docs collection present");
        let entry = docs
            .iter()
            .find(|e| e.slug == "admon")
            .expect("admon entry");

        // Independently compile the same body through the same default
        // pipeline the bundler uses, then compare hash components.
        let body = "\n:::note\nhi\n:::\n"; // post-frontmatter body, mirrors `walk_collection`'s split.
        let mut pipeline = Pipeline::with_defaults();
        let compiled = compile_mdx_to_jsx_module_cached(body, &path, None, Some(&mut pipeline))
            .expect("independent compile must succeed");

        let snap_spec = parse_mdx_specifier(&entry.module_specifier)
            .expect("snapshot specifier parses as `mdx://...#hash`");
        let bridge_spec = parse_mdx_specifier(&compiled.specifier)
            .expect("bundler-style specifier parses as `mdx://...#hash`");

        assert_eq!(
            snap_spec.content_hash, bridge_spec.content_hash,
            "snapshot module_specifier hash ({snap}) must equal the bundler's bridge key hash ({bridge}); a divergence here is the zfb #132 regression — the snapshot path is walking with `pipeline = None` while the bundler walks with `Some(&mut Pipeline::with_defaults())`, so transformed pages render the raw-markdown fallback",
            snap = snap_spec.content_hash,
            bridge = bridge_spec.content_hash,
        );
        // Sanity: collection + slug also agree.
        assert_eq!(snap_spec.collection, bridge_spec.collection);
        assert_eq!(snap_spec.slug, bridge_spec.slug);
    }

    #[test]
    fn pipeline_state_does_not_leak_across_collections() {
        // Companion guard for the per-collection pipeline shape:
        // `HeadingLinksPlugin` carries a `seen: HashMap<String, usize>`
        // slug-dedupe counter on the pipeline instance. If
        // `build_snapshot` reused one `Pipeline::with_defaults()` across
        // every collection in the for-loop, a heading like `## Intro`
        // in collection B would emit `id="intro-1"` after collection A
        // already consumed `intro` — and the snapshot bytes for B would
        // depend on which other collections were configured alongside
        // it. The bundler dodges this by hoisting one pipeline per
        // `materialise_*` call (one per collection), and `build_snapshot`
        // mirrors that shape for the same reason.
        //
        // We assert the contract by building the snapshot for two
        // single-collection configs in isolation, then for the same two
        // collections together, and proving the per-collection slice of
        // the combined snapshot matches the isolated build byte-for-byte.
        let tmp = TmpDir::new("collection-isolation");
        // Both files have the same heading text — without per-collection
        // pipelines the second one walked would slug to `intro-1`.
        let mdx_with_h2 = "---\ntitle: \"X\"\n---\n\n## Intro\n\nbody\n";
        tmp.write("a/page.mdx", mdx_with_h2);
        tmp.write("b/page.mdx", mdx_with_h2);

        let cfg_a = CollectionConfig::new("a", tmp.path.join("a"));
        let cfg_b = CollectionConfig::new("b", tmp.path.join("b"));

        let solo_a = build_snapshot(&[cfg_a.clone()]).expect("solo a");
        let solo_b = build_snapshot(&[cfg_b.clone()]).expect("solo b");
        let combined = build_snapshot(&[cfg_a, cfg_b]).expect("combined");

        let combined_a = combined.collections.get("a").expect("combined a");
        let combined_b = combined.collections.get("b").expect("combined b");
        let solo_a_entries = solo_a.collections.get("a").expect("solo a entries");
        let solo_b_entries = solo_b.collections.get("b").expect("solo b entries");

        assert_eq!(
            combined_a[0].module_specifier, solo_a_entries[0].module_specifier,
            "collection a's specifier must not depend on whether collection b is also configured (heading-link state leak)",
        );
        assert_eq!(
            combined_b[0].module_specifier, solo_b_entries[0].module_specifier,
            "collection b's specifier must not depend on whether collection a is also configured (heading-link state leak)",
        );
    }

    #[test]
    fn nested_directories_use_slash_separator_in_rel_path() {
        let tmp = TmpDir::new("nested");
        tmp.write("docs/guides/intro.md", &md("Intro"));

        let snap = build_snapshot(&[CollectionConfig::new("docs", tmp.path.join("docs"))])
            .expect("snapshot ok");
        let docs = snap.collections.get("docs").expect("docs present");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].rel_path, "guides/intro.md");
    }

    /// Sub-issue #61 land-mine guard: GFM resolved-constructs threaded
    /// through `SnapshotPipelineConfig` must produce the same JSX
    /// `content_hash` as an independent bundler-shaped compile that
    /// uses the same constructs. Diverging here is exactly the
    /// `<pre data-zfb-content-fallback>` failure mode the docstring at
    /// lines 118-153 warns about.
    ///
    /// We exercise BOTH endpoints of the new config surface:
    /// `gfm: false` (every GFM construct off — strikethrough should
    /// pass through as raw `~~` text) and `gfm: true` (every GFM
    /// construct on — strikethrough emits `<del>` and divides the
    /// `content_hash` from the all-off path).
    #[test]
    fn snapshot_specifier_matches_bridge_hash_under_explicit_gfm_choice() {
        use crate::mdx_jsx_emit::{compile_mdx_to_jsx_module_cached, parse_mdx_specifier};
        use crate::pipeline::ResolvedGfmConstructs;

        let mdx = "---\ntitle: \"Strike\"\n---\n\nplain ~~gone~~ here\n";
        let body = "\nplain ~~gone~~ here\n";

        for (label, resolved) in [
            ("ALL_OFF", ResolvedGfmConstructs::ALL_OFF),
            ("ALL_ON", ResolvedGfmConstructs::ALL_ON),
        ] {
            let tmp = TmpDir::new(&format!("snapshot-gfm-parity-{}", label.to_lowercase()));
            let path = tmp.write("docs/strike.mdx", mdx);

            let cfg = CollectionConfig::new("docs", tmp.path.join("docs"));
            let snap = build_snapshot_with_config(
                &[cfg],
                &SnapshotPipelineConfig {
                    gfm_constructs: resolved,
                    ..Default::default()
                },
            )
            .expect("snapshot must succeed");
            let entry = snap
                .collections
                .get("docs")
                .expect("docs collection")
                .iter()
                .find(|e| e.slug == "strike")
                .expect("strike entry");

            // Independent bundler-shaped compile with the same
            // constructs. Hash must agree byte-for-byte.
            let mut pipeline =
                crate::pipeline::Pipeline::with_defaults_and_theme_and_gfm(None, resolved);
            let compiled =
                compile_mdx_to_jsx_module_cached(body, &path, None, Some(&mut pipeline))
                    .expect("bundler-style compile must succeed");

            let snap_spec = parse_mdx_specifier(&entry.module_specifier)
                .expect("snapshot specifier parses");
            let bridge_spec = parse_mdx_specifier(&compiled.specifier)
                .expect("bundler specifier parses");

            assert_eq!(
                snap_spec.content_hash, bridge_spec.content_hash,
                "snapshot ↔ bundler hash divergence under gfm={label}: \
                 snapshot={snap}, bundler={bridge} — this is the \
                 sub-#61 / zfb#132 hazard (see content_bridge.rs:118-153)",
                snap = snap_spec.content_hash,
                bridge = bridge_spec.content_hash,
            );
            // Sanity — neither output should carry the fallback marker
            // (the `<pre data-zfb-content-fallback>` shape is emitted
            // by the bundler entry-module shim, not the compiled JSX
            // itself; the proxy here is "compiled JSX agrees on both
            // sides", which is the input the shim sees).
            assert!(
                !compiled.jsx_source.contains("zfb-content-fallback"),
                "unexpected fallback marker in compiled JSX under gfm={label}"
            );
        }
    }

    /// Smoke check that the two extreme GFM choices produce *different*
    /// `content_hash` values for the same input — otherwise the parity
    /// assertion above could trivially pass when the resolve path is
    /// broken (everything would collapse to a single hash).
    #[test]
    fn snapshot_specifier_differs_between_gfm_on_and_off() {
        use crate::pipeline::ResolvedGfmConstructs;

        let mdx = "---\ntitle: \"Strike\"\n---\n\nplain ~~gone~~ here\n";

        let tmp = TmpDir::new("snapshot-gfm-distinct");
        tmp.write("docs/strike.mdx", mdx);
        let cfg = || CollectionConfig::new("docs", tmp.path.join("docs"));

        let off = build_snapshot_with_config(
            &[cfg()],
            &SnapshotPipelineConfig {
                gfm_constructs: ResolvedGfmConstructs::ALL_OFF,
                ..Default::default()
            },
        )
        .expect("snapshot off");
        let on = build_snapshot_with_config(
            &[cfg()],
            &SnapshotPipelineConfig {
                gfm_constructs: ResolvedGfmConstructs::ALL_ON,
                ..Default::default()
            },
        )
        .expect("snapshot on");

        let off_spec = off.collections["docs"][0].module_specifier.as_str();
        let on_spec = on.collections["docs"][0].module_specifier.as_str();
        assert_ne!(
            off_spec, on_spec,
            "snapshot specifier MUST differ between gfm=on and gfm=off — \
             otherwise the resolve_constructs path is not actually \
             threading through to the parser"
        );
    }
}
