//! Hashing, asset emission, and the top-level [`CssPipeline`].
//!
//! Stage 3 of the CSS pipeline:
//!
//! 1. Engine output (Tailwind utilities) and CSS Modules output are
//!    concatenated, in that order, separated by `\n`.
//! 2. The combined bytes are hashed with SHA-256, and the first 8 hex
//!    characters become the asset filename suffix (`styles-{hash}.css`).
//! 3. The file is written under
//!    `{output_root}/assets/styles-{hash}.css`. Default `output_root` is
//!    `dist/`.
//!
//! [`link_href`] derives the URL for the renderer to inject as
//! `<link href="...">`. We do **not** inject HTML here — that's the
//! renderer's responsibility (Epic 3 / Epic 7 wiring).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::emitter::CssEmitterOutput;
use crate::engine::CssEngine;
use crate::modules::{CssModulesConfig, CssModulesProcessor};
use crate::scanner::{scan_css_module_imports, ModuleImportScan, SourceModuleUsage};

/// Configuration for [`CssPipeline::new`].
#[derive(Debug, Clone)]
pub struct CssPipelineConfig {
    /// Source files to scan for utility classes (passed to the engine).
    pub sources: Vec<PathBuf>,

    /// CSS Modules files to compile. Order is significant: it determines
    /// the order of concatenation in the global asset and therefore the
    /// hash.
    pub css_modules: Vec<PathBuf>,

    /// Where to write the global asset. The pipeline emits
    /// `{output_root}/assets/styles-{hash}.css` under this directory.
    /// Default: `dist/`.
    pub output_root: PathBuf,

    /// Public base URL prefix for the asset, e.g. `"/"` (default) or
    /// `"https://cdn.example.com/"`. Used by [`link_href`].
    pub base_url: String,

    /// CSS Modules processing config.
    pub modules_config: CssModulesConfig,

    /// When true, the pipeline scans every entry in
    /// [`Self::sources`] for `import * from "*.module.css"` statements
    /// and adds the discovered files to the CSS Modules input set.
    /// Discovered files are appended *after* the explicit
    /// [`Self::css_modules`] list so callers can pin a leading order
    /// that wins the hash.
    ///
    /// Default: `true`. Set to `false` when the caller has its own
    /// (e.g. SWC-based) module graph and wants
    /// [`Self::css_modules`] treated as the complete set.
    pub auto_discover_modules: bool,

    /// Where to write the per-module JSON class-name maps (the JS-side
    /// rewrite contract — see [`crate::lib`] docs). Each `.module.css`
    /// file ends up at
    /// `{class_map_dir}/<rel-from-output_root>.classes.json` if the
    /// module path is descendant of `output_root`'s parent, or at
    /// `{class_map_dir}/<sha8(path)>.classes.json` otherwise. Default:
    /// `{output_root}/css-modules`.
    ///
    /// When `None`, no JSON is written; the in-memory `class_maps` is
    /// still returned in [`CssPipelineOutput`].
    pub class_map_dir: Option<PathBuf>,

    /// Framework-shipped CSS spliced in ahead of the Tailwind utility
    /// output (issue #1533; today this carries zfb's default
    /// `--zfb-hi-*` token stylesheet for class-mode syntax highlighting,
    /// see [`crate::default_hi_css`]). `None` (default) omits the block
    /// entirely, so [`combine`]/[`hash_8`] stay byte-for-byte identical
    /// to their pre-#1533 output. Folded into the asset hash — the
    /// [`hash_8`] inputs on the `build()` path, and the combined bytes
    /// (via [`combine`]) on the prod bytes-only path — so toggling this
    /// block changes the hashed asset URL. This deliberately does NOT
    /// join the content-pipeline fingerprint (that's a separate,
    /// config-owned concern).
    pub framework_css: Option<String>,
}

impl Default for CssPipelineConfig {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            css_modules: Vec::new(),
            output_root: PathBuf::from("dist"),
            base_url: "/".to_string(),
            modules_config: CssModulesConfig::default(),
            auto_discover_modules: true,
            class_map_dir: None,
            framework_css: None,
        }
    }
}

/// Result of running [`CssPipeline::build`].
#[derive(Debug, Clone)]
pub struct CssPipelineOutput {
    /// 8-character hex hash (first 8 chars of SHA-256 of the combined CSS).
    pub hash: String,

    /// The full combined CSS string.
    pub css: String,

    /// Absolute or output-root-relative path the asset was written to.
    /// Convention: `{output_root}/assets/styles-{hash}.css`.
    pub asset_path: PathBuf,

    /// Per-file CSS Modules class-name maps. See
    /// [`crate::modules::CssModulesOutput::class_maps`].
    pub class_maps: HashMap<PathBuf, HashMap<String, String>>,

    /// Per-source-file CSS Modules usage. Empty when
    /// [`CssPipelineConfig::auto_discover_modules`] is false. The
    /// dev-asset-graph crate (`zfb-graph`'s SSE topic) consumes this
    /// to answer "which pages depend on which CSS modules" without
    /// needing to re-scan the sources itself.
    pub per_source_modules: Vec<SourceModuleUsage>,

    /// JSON class-map files emitted to disk, in the order they were
    /// written. Empty when
    /// [`CssPipelineConfig::class_map_dir`] is `None`. Each entry maps
    /// the original `.module.css` path to the `.classes.json` file
    /// containing `{ "originalClass": "scopedClass", ... }`.
    pub class_map_files: HashMap<PathBuf, PathBuf>,
}

/// Top-level CSS pipeline.
///
/// Generic over the engine type so callers can swap [`crate::TailwindSubprocessEngine`]
/// for [`crate::NativeRustEngine`] (or a test double) without touching the
/// pipeline code.
pub struct CssPipeline<E: CssEngine> {
    engine: E,
    modules: CssModulesProcessor,
    config: CssPipelineConfig,
}

impl<E: CssEngine> CssPipeline<E> {
    /// Construct a new pipeline.
    pub fn new(engine: E, config: CssPipelineConfig) -> Self {
        let modules = CssModulesProcessor::new(config.modules_config.clone());
        Self {
            engine,
            modules,
            config,
        }
    }

    /// Run all stages: engine → CSS Modules → hash → write.
    pub fn build(&self) -> Result<CssPipelineOutput> {
        // Stage 0: optionally discover CSS Modules from the source
        // files. Discovered files are appended after the explicit list,
        // de-duplicated.
        let (module_files, per_source_modules) = self.collect_modules()?;

        let tailwind = self
            .engine
            .produce_utility_css(&self.config.sources)
            .context("CSS engine stage failed")?;

        let modules = self
            .modules
            .process(&module_files)
            .context("CSS Modules stage failed")?;

        let framework = self.config.framework_css.as_deref();
        let combined = combine(framework, &tailwind, &modules.css);
        let hash = hash_8(framework, &tailwind, &modules.css);
        let asset_path = self
            .config
            .output_root
            .join("assets")
            .join(format!("styles-{hash}.css"));

        atomic_write(&asset_path, combined.as_bytes())
            .with_context(|| format!("failed to write {}", asset_path.display()))?;

        // Emit JSON class-name maps if requested.
        let class_map_files = if let Some(dir) = &self.config.class_map_dir {
            write_class_map_files(
                dir,
                &modules.class_maps,
                self.config.modules_config.project_root.as_deref(),
            )?
        } else {
            HashMap::new()
        };

        Ok(CssPipelineOutput {
            hash,
            css: combined,
            asset_path,
            class_maps: modules.class_maps,
            per_source_modules,
            class_map_files,
        })
    }

    /// Bytes-only entry point used by `ProductionAssetPipeline`.
    ///
    /// Runs every stage `build()` runs *except* the disk write of the
    /// hashed `styles-<hash>.css` file: the prod pipeline owns asset
    /// hashing and writes the file once after URL rewrite, so any
    /// emit-side write here would leave a stale duplicate next to the
    /// hashed copy. The CSS Modules class-map JSONs are still emitted
    /// when [`CssPipelineConfig::class_map_dir`] is set — those files
    /// are not asset-graph nodes and the bundler reads them at the
    /// next stage.
    ///
    /// The returned [`CssEmitterOutput`] carries the combined CSS
    /// bytes plus the stable URL constant from
    /// [`zfb_types::STABLE_CSS_URL`]; the `ProductionAssetPipeline`
    /// matches HTML occurrences of that string and rewrites them to
    /// the hashed URL before writing pages.
    pub fn build_emitter(&self) -> Result<CssEmitterOutput> {
        let (module_files, _per_source_modules) = self.collect_modules()?;

        let tailwind = self
            .engine
            .produce_utility_css(&self.config.sources)
            .context("CSS engine stage failed")?;

        let modules = self
            .modules
            .process(&module_files)
            .context("CSS Modules stage failed")?;

        let combined = combine(
            self.config.framework_css.as_deref(),
            &tailwind,
            &modules.css,
        );

        // Emit JSON class-name maps if requested. Done here too so the
        // bundler stage can resolve `*.module.css` imports against the
        // map files, exactly as `build()` does — dropping this would
        // break the bytes-only path for any project that uses CSS
        // Modules.
        if let Some(dir) = &self.config.class_map_dir {
            write_class_map_files(
                dir,
                &modules.class_maps,
                self.config.modules_config.project_root.as_deref(),
            )?;
        }

        Ok(CssEmitterOutput {
            bytes: combined.into_bytes(),
            stable_url: zfb_types::STABLE_CSS_URL.to_string(),
        })
    }

    fn collect_modules(&self) -> Result<(Vec<PathBuf>, Vec<SourceModuleUsage>)> {
        let mut files: Vec<PathBuf> = self.config.css_modules.clone();
        let mut seen: std::collections::HashSet<PathBuf> = files.iter().cloned().collect();

        let per_source = if self.config.auto_discover_modules && !self.config.sources.is_empty() {
            let scan: ModuleImportScan = scan_css_module_imports(&self.config.sources)
                .context("CSS Modules import scan failed")?;
            for m in &scan.modules {
                // Auto-discover only resolved paths that exist on
                // disk; bare specifiers (e.g. `@org/pkg/...`) are
                // recorded in per_source but not fed to the
                // processor, since lightningcss needs a real file.
                if seen.insert(m.clone()) && m.exists() {
                    files.push(m.clone());
                }
            }
            scan.per_source
        } else {
            Vec::new()
        };

        Ok((files, per_source))
    }

    /// Borrow the underlying config.
    pub fn config(&self) -> &CssPipelineConfig {
        &self.config
    }

    /// Borrow the underlying engine. Useful for inspection in tests
    /// (e.g. fetching [`crate::TailwindSubprocessEngine::last_entry_css`]).
    pub fn engine_ref(&self) -> &E {
        &self.engine
    }
}

/// Emit the JSON class-name map file for each `.module.css` file under
/// `dir`. Returns the on-disk path for each map.
///
/// File layout:
/// `{dir}/<sha8(module_path)>__<basename>.classes.json`. The hash prefix
/// guarantees uniqueness across the whole module set even when two
/// modules share a basename, while keeping the basename visible for
/// debugging.
///
/// `project_root` is used to relativize the path before hashing so the
/// JSON filename prefix is stable across machines/checkout paths — the
/// same normalisation lightningcss uses for the scoped `[hash]` prefix
/// (see issue #825 and [`crate::modules::hash_filename`]). The
/// `class_maps` keys stay absolute; only the hash *input* is normalised.
fn write_class_map_files(
    dir: &Path,
    class_maps: &HashMap<PathBuf, HashMap<String, String>>,
    project_root: Option<&Path>,
) -> Result<HashMap<PathBuf, PathBuf>> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create class-map dir {}", dir.display()))?;
    let mut out: HashMap<PathBuf, PathBuf> = HashMap::new();
    for (module_path, names) in class_maps {
        let mut hasher = Sha256::new();
        hasher.update(crate::modules::hash_filename(module_path, project_root).as_bytes());
        let h = hex::encode(hasher.finalize());
        let prefix: &str = &h[..8];
        let basename = module_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "anon".to_string());
        let json_path = dir.join(format!("{prefix}__{basename}.classes.json"));

        let mut sorted: Vec<(&String, &String)> = names.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        let json = serde_json::to_string_pretty(
            &sorted
                .into_iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<std::collections::BTreeMap<_, _>>(),
        )
        .context("failed to serialise CSS Modules class-name map")?;
        atomic_write(&json_path, json.as_bytes())
            .with_context(|| format!("failed to write {}", json_path.display()))?;
        out.insert(module_path.clone(), json_path);
    }
    Ok(out)
}

/// Combine framework CSS (optional) + engine output + CSS Modules output
/// exactly as the hashing stage will see it. Exposed as a free function so
/// tests and the hashing helper agree on the canonical form.
///
/// `framework` is [`CssPipelineConfig::framework_css`] (issue #1533); when
/// `Some`, it is spliced in ahead of the Tailwind + modules body — but
/// **after** any leading `@charset` / `@layer name,…;` order statements the
/// Tailwind half emits (see [`splice_framework_after_layer_prefix`] for why a
/// naive index-0 prepend would corrupt import hoisting). When `None` the
/// output is byte-for-byte identical to the pre-#1533 two-piece form
/// (`tailwind + "\n" + modules`).
///
/// After concatenation, external `@import` at-rules are hoisted to the top
/// of the stylesheet via [`hoist_external_imports`] — see that function for
/// why. The hash (`hash_8`) is computed from the raw `(framework, tailwind,
/// modules)` pieces, not from this combined form, so hoisting does not
/// perturb asset hashing: it is a deterministic pure function of the same
/// inputs.
pub(crate) fn combine(framework: Option<&str>, tailwind: &str, modules: &str) -> String {
    let mut body = String::with_capacity(tailwind.len() + modules.len() + 1);
    body.push_str(tailwind);
    body.push('\n');
    body.push_str(modules);

    let combined = match framework {
        None => body,
        Some(framework) => splice_framework_after_layer_prefix(&body, framework),
    };
    hoist_external_imports(&combined)
}

/// Insert the framework block into `body` **after** any leading `@charset` /
/// `@layer name,…;` order-statement prefix, rather than at absolute
/// position 0.
///
/// **Why not a naive prepend (issue #1533):** the framework block is a
/// *populated* `@layer zfb-hi { … }` — [`classify_node`] classes it as
/// [`NodeKind::Other`] (an insertion ceiling), not a leading order
/// statement. Prepending it at index 0 would push Tailwind v4's own leading
/// `@layer theme, base, components, utilities;` order preamble out of the
/// leading-prefix region [`hoist_external_imports`] scans, so a trailing
/// external `@import` (e.g. an authored webfont) would then hoist *above*
/// that layer-order statement and silently reorder the cascade layers.
/// Splicing after the leading order-statement prefix keeps that preamble
/// first, so imports hoist below it exactly as they did pre-#1533.
///
/// When `body` has no leading `@charset` / `@layer …;` prefix — the common
/// case (Tailwind disabled, or the engine emits no order statement) — the
/// framework block lands at position 0, byte-identical to a naive prepend.
fn splice_framework_after_layer_prefix(body: &str, framework: &str) -> String {
    let nodes = split_top_level(body);
    let mut cut = 0usize;
    for &(s, e) in &nodes {
        match classify_node(&body[s..e]) {
            NodeKind::Charset | NodeKind::LayerStatement => cut = e,
            _ => break,
        }
    }
    let mut out = String::with_capacity(body.len() + framework.len() + 2);
    out.push_str(&body[..cut]);
    // Separate from the preceding order statement's `;` when we spliced after
    // a non-empty prefix; at cut == 0 this stays a bare prepend (no leading
    // newline) so the no-order-statement case matches the old byte layout.
    if cut > 0 {
        out.push('\n');
    }
    out.push_str(framework);
    out.push('\n');
    out.push_str(&body[cut..]);
    out
}

/// Move every top-level `@import` at-rule to the top of the stylesheet so it
/// stays spec-valid regardless of authored position.
///
/// **Why this exists (external CSS-spec contract, issue #1280):** the CSS
/// spec (Cascade & Inheritance) requires `@import` rules to precede all other
/// at-rules and style rules in a stylesheet — the only things allowed before
/// them are a leading `@charset` and empty `@layer name, …;` layer-ordering
/// statements. **An `@import` that follows any style rule is invalid and
/// silently dropped by every browser.** zfb's Tailwind v4 pipeline inlines
/// the local `@import "tailwindcss/…"` statements into real style rules *in
/// place*, so a consumer's external `@import url(<webfont>)` authored *below*
/// those lines ends up after thousands of emitted rules and is dropped — the
/// webfont never loads, with every build/lint gate still green because the
/// string is present but inert. Hoisting here makes the position-sensitivity
/// trap disappear.
///
/// The pass:
/// - operates on **top-level** statements only (brace depth 0); a nested
///   `@import` is already invalid and is left untouched;
/// - is comment-, string-, and `url(<…>)`-aware (a `;` inside `( … )` — e.g.
///   `url(data:text/css;base64,…)` — does not terminate the statement);
/// - re-inserts the collected imports, in their original relative order,
///   after any leading `@charset` and leading `@layer name,…;` statements and
///   before all other content;
/// - is a **fixed point on its own output** — running it twice equals running
///   it once, and a stylesheet whose imports are already correctly positioned
///   is returned unchanged.
pub(crate) fn hoist_external_imports(css: &str) -> String {
    let nodes = split_top_level(css);
    let kinds: Vec<NodeKind> = nodes
        .iter()
        .map(|&(s, e)| classify_node(&css[s..e]))
        .collect();

    let import_idxs: Vec<usize> = (0..nodes.len())
        .filter(|&i| kinds[i] == NodeKind::Import)
        .collect();
    if import_idxs.is_empty() {
        return css.to_string();
    }

    // Leading run of `@charset` / `@layer name,…;` statements the imports
    // must sit *after* (per the spec). The run stops at the first node that
    // is an import or anything else.
    let mut prefix_end = 0;
    while prefix_end < nodes.len()
        && matches!(
            kinds[prefix_end],
            NodeKind::Charset | NodeKind::LayerStatement
        )
    {
        prefix_end += 1;
    }

    let mut out = String::with_capacity(css.len());
    // 1. The leading @charset / @layer-statement prefix, verbatim.
    for &(s, e) in &nodes[..prefix_end] {
        out.push_str(&css[s..e]);
    }
    // 2. Every import, in original order.
    for &i in &import_idxs {
        let (s, e) = nodes[i];
        out.push_str(&css[s..e]);
    }
    // 3. Everything else after the prefix, in original order, imports removed.
    for i in prefix_end..nodes.len() {
        if kinds[i] != NodeKind::Import {
            let (s, e) = nodes[i];
            out.push_str(&css[s..e]);
        }
    }
    out
}

/// Classification of a top-level stylesheet node for import hoisting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    /// `@charset "…";` — must stay first; imports go after it.
    Charset,
    /// `@layer name, …;` statement form (no block) — allowed before `@import`.
    LayerStatement,
    /// `@import …;` — the at-rule being hoisted.
    Import,
    /// Any style rule, populated `@layer { … }`, `@media { … }`, declaration,
    /// or other content — the imports must end up above all of these.
    Other,
}

/// Split a stylesheet into contiguous top-level node spans `[start, end)`.
///
/// The spans cover the entire string with no gaps (concatenating them in
/// order reproduces the input), so reordering nodes only permutes the exact
/// source bytes — preserving content and idempotency. Each node carries the
/// whitespace/comments that lead up to its statement; the final span is any
/// trailing remainder after the last terminator.
///
/// The scan tracks string literals, block (`/* */`) and line (`//`) comments,
/// brace depth, and paren depth, so statement terminators are only recognised
/// outside those contexts. Paren tracking is what makes a `;` inside
/// `url(data:…;…)` non-terminating without a dedicated `url(` token scanner
/// (unquoted `url()` cannot contain unescaped parentheses per the CSS spec).
fn split_top_level(css: &str) -> Vec<(usize, usize)> {
    let b = css.as_bytes();
    let n = b.len();
    let mut nodes: Vec<(usize, usize)> = Vec::new();
    let mut seg_start = 0usize;
    let mut i = 0usize;
    let mut brace: i32 = 0;
    let mut paren: i32 = 0;

    while i < n {
        let c = b[i];
        match c {
            b'"' | b'\'' | b'`' => {
                // String literal: skip to the closing quote, honoring `\`
                // escapes, and bail on a raw newline (matches the rest of the
                // crate's tolerant scanners).
                let quote = c;
                i += 1;
                while i < n {
                    let d = b[i];
                    if d == b'\\' && i + 1 < n {
                        i += 2;
                        continue;
                    }
                    if d == quote || d == b'\n' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(n);
            }
            b'/' if i + 1 < n && b[i + 1] == b'/' => {
                i += 2;
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' => {
                paren += 1;
                i += 1;
            }
            b')' => {
                if paren > 0 {
                    paren -= 1;
                }
                i += 1;
            }
            b'{' => {
                brace += 1;
                i += 1;
            }
            b'}' => {
                if brace > 0 {
                    brace -= 1;
                }
                i += 1;
                if brace == 0 {
                    nodes.push((seg_start, i));
                    seg_start = i;
                }
            }
            b';' => {
                i += 1;
                if brace == 0 && paren == 0 {
                    nodes.push((seg_start, i));
                    seg_start = i;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    if seg_start < n {
        nodes.push((seg_start, n));
    }
    nodes
}

/// Classify a node by its first significant token (skipping leading
/// whitespace, a UTF-8 BOM, and comments). At-rule names are ASCII
/// case-insensitive, so the comparison is too.
fn classify_node(node: &str) -> NodeKind {
    let rest = significant_rest(node);
    if starts_with_ascii_ci(rest, "@charset") {
        NodeKind::Charset
    } else if starts_with_ascii_ci(rest, "@import") {
        NodeKind::Import
    } else if starts_with_ascii_ci(rest, "@layer") && rest.trim_end().ends_with(';') {
        // `@layer a, b;` (no block) is a layer-ordering statement, allowed
        // before `@import`. A populated `@layer name { … }` ends in `}` and is
        // treated as Other (an insertion ceiling). Classifying by the
        // terminating delimiter (the node ends exactly at its `;`/`}`) is
        // comment-proof — a raw scan for `{` would misread `@layer a /* { */;`.
        NodeKind::LayerStatement
    } else {
        NodeKind::Other
    }
}

/// Return the slice of `node` starting at its first significant byte,
/// skipping leading ASCII whitespace, a leading UTF-8 BOM, and leading
/// block/line comments.
fn significant_rest(node: &str) -> &str {
    let b = node.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    // Optional leading UTF-8 BOM (EF BB BF).
    if n >= 3 && b[0] == 0xEF && b[1] == 0xBB && b[2] == 0xBF {
        i = 3;
    }
    loop {
        while i < n && (b[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        if i + 1 < n && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(n);
            continue;
        }
        if i + 1 < n && b[i] == b'/' && b[i + 1] == b'/' {
            i += 2;
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        break;
    }
    &node[i.min(n)..]
}

/// ASCII-case-insensitive `starts_with` (at-rule keywords are ASCII).
fn starts_with_ascii_ci(haystack: &str, needle: &str) -> bool {
    let h = haystack.as_bytes();
    let nd = needle.as_bytes();
    h.len() >= nd.len() && h[..nd.len()].eq_ignore_ascii_case(nd)
}

/// Compute the 8-char hex hash for the given (framework, tailwind, modules)
/// pieces.
///
/// The hash is the first 8 characters of `sha256(tailwind + "\n" +
/// modules)`, with `framework + "\n"` prepended to the hasher input when
/// `Some`. Using a fixed separator means appending a class to one piece is
/// distinguishable from prepending it to another.
///
/// This is the hash the **disk-writing** [`CssPipeline::build`] path uses to
/// name its `styles-<hash>.css`. The bytes-only production path
/// ([`CssPipeline::build_emitter`] → `zfb-build`'s `ProductionAssetPipeline`)
/// does **not** call this — it content-hashes the combined asset bytes
/// directly (`sha256_8(&bytes)` in `crates/zfb-build/src/pipeline/prod.rs`).
/// Folding `framework` into *both* this hash and the combined bytes (via
/// [`combine`]) is what makes toggling [`CssPipelineConfig::framework_css`]
/// change the emitted filename on either path, so a stale cached copy is
/// never reused (issue #1533).
pub fn hash_8(framework: Option<&str>, tailwind: &str, modules: &str) -> String {
    let mut hasher = Sha256::new();
    if let Some(framework) = framework {
        hasher.update(framework.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(tailwind.as_bytes());
    hasher.update(b"\n");
    hasher.update(modules.as_bytes());
    let digest = hasher.finalize();
    let full = hex::encode(digest);
    full[..8].to_string()
}

/// Build the public URL for the global CSS asset.
///
/// `base_url` is concatenated with the asset path's filename portion under
/// an `assets/` segment. `base_url` is normalised to end with `/`.
///
/// Examples:
/// ```
/// use std::path::PathBuf;
/// use zfb_css::link_href;
///
/// let p = PathBuf::from("dist/assets/styles-abc12345.css");
/// assert_eq!(link_href("/", &p), "/assets/styles-abc12345.css");
/// assert_eq!(
///     link_href("https://cdn.example.com", &p),
///     "https://cdn.example.com/assets/styles-abc12345.css",
/// );
/// ```
pub fn link_href(base_url: &str, asset_path: &Path) -> String {
    let filename = asset_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let trimmed = base_url.trim_end_matches('/');
    format!("{trimmed}/assets/{filename}")
}

/// Atomic write helper local to this crate. Mirrors the
/// `write-temp-then-rename` recipe used by `zfb-build::atomic` so the
/// CSS pipeline never leaves a half-written `.css` (or class-map JSON)
/// behind.
fn atomic_write(dest: &Path, bytes: &[u8]) -> Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);

    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
    }

    let pid = std::process::id();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = dest
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("zfb-css"));
    name.push(format!(".tmp-{pid}-{seq}"));
    let mut temp_path = dest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    if temp_path.as_os_str().is_empty() {
        temp_path = PathBuf::from(".");
    }
    temp_path.push(name);

    {
        let mut f = std::fs::File::create(&temp_path)
            .with_context(|| format!("failed to create temp file {}", temp_path.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("failed to write temp file {}", temp_path.display()))?;
        f.sync_all()
            .with_context(|| format!("failed to fsync temp file {}", temp_path.display()))?;
    }

    // Rust's `std::fs::rename` replaces an existing destination on
    // both POSIX and Windows (Windows: implemented via `MoveFileExW`
    // with `MOVEFILE_REPLACE_EXISTING`). We rely on it directly.
    // See https://doc.rust-lang.org/std/fs/fn.rename.html.
    if let Err(e) = std::fs::rename(&temp_path, dest) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e).with_context(|| {
            format!(
                "failed to rename {} -> {}",
                temp_path.display(),
                dest.display()
            )
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- hoist_external_imports (issue #1280) ------------------------------

    /// The byte offset of the first occurrence of `needle` in `s`.
    fn offset_of(s: &str, needle: &str) -> usize {
        s.find(needle)
            .unwrap_or_else(|| panic!("expected to find {needle:?} in:\n{s}"))
    }

    #[test]
    fn hoist_moves_font_import_above_style_rules() {
        // The #1280 repro: a font @import authored *after* style rules.
        let css = "@layer a, b;\n.x { color: red }\n.y { color: blue }\n\
                   @import url(\"https://fonts.googleapis.com/css2?family=Noto\");\n";
        let out = hoist_external_imports(css);
        // After hoist, the @import precedes the first style rule...
        assert!(
            offset_of(&out, "@import") < offset_of(&out, ".x"),
            "import must precede the first style rule:\n{out}"
        );
        // ...and follows the @layer ordering statement.
        assert!(
            offset_of(&out, "@layer a, b;") < offset_of(&out, "@import"),
            "import must follow the leading @layer statement:\n{out}"
        );
    }

    #[test]
    fn hoist_preserves_relative_order_of_multiple_imports() {
        let css = ".x {}\n@import url(\"a.css\");\n@import url(\"b.css\");\n";
        let out = hoist_external_imports(css);
        assert!(offset_of(&out, "a.css") < offset_of(&out, "b.css"));
        assert!(offset_of(&out, "@import") < offset_of(&out, ".x"));
    }

    #[test]
    fn hoist_keeps_charset_first() {
        let css = "@charset \"utf-8\";\n.x {}\n@import url(\"a.css\");\n";
        let out = hoist_external_imports(css);
        assert!(
            out.starts_with("@charset \"utf-8\";"),
            "charset must stay first:\n{out}"
        );
        assert!(offset_of(&out, "@charset") < offset_of(&out, "@import"));
        assert!(offset_of(&out, "@import") < offset_of(&out, ".x"));
    }

    #[test]
    fn hoist_ignores_import_inside_block_comment() {
        let css = ".x {}\n/* @import url(\"commented.css\"); */\n.y {}\n";
        let out = hoist_external_imports(css);
        // Nothing real to hoist → unchanged.
        assert_eq!(out, css);
    }

    #[test]
    fn hoist_ignores_import_inside_line_comment() {
        let css = ".x {}\n// @import url(\"commented.css\");\n.y {}\n";
        let out = hoist_external_imports(css);
        assert_eq!(out, css);
    }

    #[test]
    fn hoist_ignores_import_like_text_in_a_string() {
        // A declaration value that merely contains "@import" text must not
        // be detected as a statement-level import.
        let css = ".x { --u: \"@import url(evil)\"; }\n.y {}\n";
        let out = hoist_external_imports(css);
        assert_eq!(out, css);
    }

    #[test]
    fn hoist_does_not_truncate_data_url_with_semicolon() {
        // The `;` inside url(data:...;base64,...) is NOT the statement
        // terminator — paren tracking must carry past it.
        let css = ".x {}\n@import url(data:text/css;base64,AAA);\n.y {}\n";
        let out = hoist_external_imports(css);
        assert!(
            out.contains("@import url(data:text/css;base64,AAA);"),
            "data-url import must survive intact:\n{out}"
        );
        assert!(offset_of(&out, "@import") < offset_of(&out, ".x"));
    }

    #[test]
    fn hoist_preserves_conditional_import_verbatim() {
        let import =
            "@import url(x.css) layer(base) supports(display:grid) screen and (min-width:400px);";
        let css = format!(".x {{}}\n{import}\n");
        let out = hoist_external_imports(&css);
        assert!(
            out.contains(import),
            "conditional import must be preserved verbatim:\n{out}"
        );
        assert!(offset_of(&out, "@import") < offset_of(&out, ".x"));
    }

    #[test]
    fn hoist_places_import_above_namespace_but_below_charset() {
        let css = "@charset \"utf-8\";\n@namespace svg url(http://www.w3.org/2000/svg);\n\
                   .x {}\n@import url(\"a.css\");\n";
        let out = hoist_external_imports(css);
        assert!(out.starts_with("@charset"), "charset stays first:\n{out}");
        assert!(
            offset_of(&out, "@import") < offset_of(&out, "@namespace"),
            "imports must precede @namespace:\n{out}"
        );
    }

    #[test]
    fn hoist_keeps_import_below_layer_statement_bearing_a_comment() {
        // A leading @layer ordering statement whose node carries a comment
        // containing `{` must still classify as a layer statement (comment-
        // aware), so the import lands *after* it, not above it.
        let css = "@layer a /* sets { scope */;\n.x {}\n@import url(\"z.css\");\n";
        let out = hoist_external_imports(css);
        assert!(
            offset_of(&out, "@layer") < offset_of(&out, "@import"),
            "import must stay below the @layer statement even with an inline comment:\n{out}"
        );
        assert!(offset_of(&out, "@import") < offset_of(&out, ".x"));
    }

    #[test]
    fn hoist_treats_populated_layer_block_as_a_ceiling() {
        // A populated `@layer name { … }` is content the import must sit above.
        let css = "@layer base { .b { color: red } }\n@import url(\"z.css\");\n";
        let out = hoist_external_imports(css);
        assert!(
            offset_of(&out, "@import") < offset_of(&out, "@layer base"),
            "import must be hoisted above a populated @layer block:\n{out}"
        );
    }

    #[test]
    fn hoist_is_idempotent_and_a_fixed_point() {
        let css =
            "@layer a, b;\n.x { color: red }\n@import url(\"a.css\");\n@import url(\"b.css\");\n";
        let once = hoist_external_imports(css);
        let twice = hoist_external_imports(&once);
        assert_eq!(
            once, twice,
            "the pass must be a fixed point on its own output"
        );
    }

    #[test]
    fn hoist_leaves_already_correct_stylesheet_unchanged() {
        let css = "@charset \"utf-8\";\n@import url(\"a.css\");\n.x { color: red }\n";
        let out = hoist_external_imports(css);
        assert_eq!(
            out, css,
            "an already-correct stylesheet is returned unchanged"
        );
    }

    #[test]
    fn hoist_noop_when_no_external_imports() {
        let css = ".a { color: red }\n.b { color: blue }\n";
        assert_eq!(hoist_external_imports(css), css);
    }

    #[test]
    fn hoist_ignores_nested_import_inside_a_block() {
        // A non-top-level @import (inside a block) is already invalid; leave
        // it where it is rather than yanking it out of the block.
        let css = "@media screen {\n  @import url(\"nested.css\");\n}\n.x {}\n";
        let out = hoist_external_imports(css);
        assert_eq!(out, css);
    }

    #[test]
    fn combine_hoists_external_import_in_the_tailwind_half() {
        // End-to-end through the canonical combine() form: a font import that
        // trails the (inlined) Tailwind rules lands above them.
        let tailwind = "@layer a, b;\n.tw { color: red }\n\
                        @import url(\"https://fonts.googleapis.com/css2?family=Noto\");";
        let combined = combine(None, tailwind, "");
        assert!(
            offset_of(&combined, "@import") < offset_of(&combined, ".tw"),
            "combine() must hoist the trailing font import:\n{combined}"
        );
    }

    #[test]
    fn hash_is_stable_for_identical_inputs() {
        let a = hash_8(None, ".a{color:red}", ".b{color:blue}");
        let b = hash_8(None, ".a{color:red}", ".b{color:blue}");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn hash_changes_when_a_class_changes() {
        let before = hash_8(None, ".a{color:red}", ".b{color:blue}");
        let after = hash_8(None, ".a{color:green}", ".b{color:blue}");
        assert_ne!(
            before, after,
            "changing a class in the tailwind half must change the hash"
        );

        let after2 = hash_8(None, ".a{color:red}", ".b{color:teal}");
        assert_ne!(
            before, after2,
            "changing a class in the modules half must change the hash"
        );
    }

    #[test]
    fn hash_uses_separator_to_avoid_boundary_collisions() {
        // Without a separator, ("ab", "cd") and ("a", "bcd") would hash the
        // same. With our "\n" separator they must differ.
        let a = hash_8(None, "ab", "cd");
        let b = hash_8(None, "a", "bcd");
        assert_ne!(a, b);
    }

    // --- framework_css injection (issue #1533) ---

    #[test]
    fn combine_prepends_framework_css_when_present() {
        let combined = combine(
            Some(".fw{color:gold}"),
            ".tw{color:red}",
            ".mod{color:blue}",
        );
        assert_eq!(
            combined,
            ".fw{color:gold}\n.tw{color:red}\n.mod{color:blue}"
        );
    }

    #[test]
    fn combine_omits_framework_block_when_none() {
        // None must be byte-for-byte identical to the pre-#1533 two-piece
        // form — no stray separator when there is no framework CSS.
        let with_none = combine(None, ".tw{color:red}", ".mod{color:blue}");
        assert_eq!(with_none, ".tw{color:red}\n.mod{color:blue}");
    }

    #[test]
    fn hash_8_changes_when_framework_css_toggles() {
        let without = hash_8(None, ".tw{color:red}", ".mod{color:blue}");
        let with = hash_8(
            Some(".fw{color:gold}"),
            ".tw{color:red}",
            ".mod{color:blue}",
        );
        assert_ne!(
            without, with,
            "toggling framework_css must change the hashed asset URL \
             so a stale cached copy is never reused"
        );
    }

    #[test]
    fn hash_8_stable_for_identical_framework_css_inputs() {
        let a = hash_8(
            Some(".fw{color:gold}"),
            ".tw{color:red}",
            ".mod{color:blue}",
        );
        let b = hash_8(
            Some(".fw{color:gold}"),
            ".tw{color:red}",
            ".mod{color:blue}",
        );
        assert_eq!(a, b);
    }

    #[test]
    fn combine_keeps_import_below_leading_layer_statement_with_framework_block() {
        // Regression (issue #1533): the framework block is a *populated*
        // `@layer …{ … }` (an import-hoist ceiling). A naive index-0 prepend
        // pushes Tailwind's leading `@layer …;` order statement out of the
        // hoist prefix region, so a trailing external @import would hoist
        // ABOVE the order statement and reorder the cascade layers. The
        // splice must keep the order statement first.
        let framework = "@layer zfb-hi { .hi-kw { color: var(--zfb-hi-kw) } }";
        let tailwind = "@layer theme, base, components, utilities;\n\
                        .tw { color: red }\n\
                        @import url(\"https://fonts.googleapis.com/css2?family=Noto\");";
        let combined = combine(Some(framework), tailwind, "");

        // The @layer order statement stays first — the import hoists BELOW it,
        // not above it.
        assert!(
            offset_of(&combined, "@layer theme,") < offset_of(&combined, "@import"),
            "the leading @layer order statement must stay above the hoisted import:\n{combined}"
        );
        // The import is still hoisted above the style rule — its whole purpose.
        assert!(
            offset_of(&combined, "@import") < offset_of(&combined, ".tw"),
            "the font import must still hoist above style rules:\n{combined}"
        );
        // And the framework block is present, spliced after the order statement.
        assert!(
            offset_of(&combined, "@layer theme,") < offset_of(&combined, "@layer zfb-hi"),
            "the framework @layer block must sit after the order statement:\n{combined}"
        );
    }

    #[test]
    fn link_href_normalises_trailing_slash() {
        let p = PathBuf::from("dist/assets/styles-deadbeef.css");
        assert_eq!(link_href("/", &p), "/assets/styles-deadbeef.css");
        assert_eq!(link_href("", &p), "/assets/styles-deadbeef.css");
        assert_eq!(
            link_href("https://cdn.example.com/", &p),
            "https://cdn.example.com/assets/styles-deadbeef.css"
        );
        assert_eq!(
            link_href("https://cdn.example.com", &p),
            "https://cdn.example.com/assets/styles-deadbeef.css"
        );
    }

    /// Round 2 regression guard. CSS pipeline reuses stable filenames
    /// across rebuilds — a second write to the same destination must
    /// succeed and replace the prior bytes. `std::fs::rename` does the
    /// right thing on both POSIX and Windows; verifying it here keeps
    /// repeat builds covered on every platform CI runs on.
    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("styles-abcd1234.css");
        atomic_write(&dest, b".a{color:red}").unwrap();
        atomic_write(&dest, b".a{color:blue}").unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b".a{color:blue}");
    }

    #[test]
    fn atomic_write_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("nested/deep/styles.css");
        atomic_write(&dest, b"body{}").unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"body{}");
    }

    #[test]
    fn atomic_write_leaves_no_temp_files_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.css");
        atomic_write(&dest, b"hi").unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["a.css".to_string()]);
    }
}
