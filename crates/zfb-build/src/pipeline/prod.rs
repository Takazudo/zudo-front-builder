//! [`ProductionAssetPipeline`] — the one-shot full-build pipeline `zfb
//! build` runs.
//!
//! Differences from [`super::dev::DevAssetPipeline`]:
//!
//! - **Content-hashed asset filenames.** Every CSS / islands asset emitted
//!   this tick is content-addressed: the SHA-256 of the bytes (truncated
//!   to 8 hex chars) is folded into the filename — e.g. `styles.css`
//!   → `styles-<hash>.css`. Two builds with identical bytes produce
//!   identical filenames; any byte change cascades into a new URL. The
//!   8-char width matches the existing convention used by
//!   [`zfb_css::link_href`] and `zfb_islands::bundle_link_href` so all
//!   three crates produce comparable filenames for the same byte
//!   stream.
//!
//! - **HTML rewrite for content-addressed URLs.** Every `RenderedPage`
//!   has its body scanned for the *stable URL* declared on the
//!   matching [`AssetEmitter`] (e.g. `/assets/styles.css`); each match
//!   is rewritten to the **hashed** URL (`/assets/styles-<hash>.css`)
//!   before the page is atomically written to dist. Pages can therefore
//!   be authored against a stable URL contract and still get
//!   forever-cacheable hashed URLs at deploy time.
//!
//! - **No SSE / no reload signaling.** Production builds emit a static
//!   `dist/` tree only; the pipeline never produces reload events. The
//!   `BuildOutcome::pages_written` field is still populated for tooling
//!   that wants a per-tick summary, but no consumer is expected to act
//!   on it as a reload trigger.
//!
//! - **No incremental byte cache.** Each `apply` writes every emitted
//!   asset and every rendered page from scratch. Production runs once,
//!   then exits — caching is the bin crate's responsibility (e.g.
//!   wiping `dist/` before invoking).
//!
//! - **Minification is the emitter's responsibility.** The bin crate is
//!   expected to construct each [`AssetEmitter`] with `minify=true`
//!   (CSS via `lightningcss`'s minifier, JS via esbuild's
//!   `--minify`). The pipeline is opaque to the *content* of the bytes
//!   — it only hashes and ships them. Tests that simulate the
//!   minification toggle do so by feeding the emitter pre-minified
//!   bytes.
//!
//! ## Why not consume `BuildContext::run_css` / `run_islands`?
//!
//! Those callbacks are typed `Result<bool>` — a black-box "did the
//! asset change?" signal. The production pipeline needs the bytes
//! themselves so it can hash them and emit a content-addressed URL. So
//! production swaps to a richer [`AssetEmitter`] contract and ignores
//! the dev-style runners entirely. The fields stay on `BuildContext`
//! to keep the dev path's wiring untouched.
//!
//! ## Selection
//!
//! Selection is by trait dispatch at the bin crate. `zfb dev`
//! constructs [`super::dev::DevAssetPipeline`]; `zfb build` constructs
//! [`ProductionAssetPipeline`]. The orchestrator is generic over the
//! [`super::AssetPipeline`] trait so neither command sprouts an
//! `if mode == Production` conditional.

use std::path::PathBuf;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use zfb_graph::PageId;

use crate::atomic::{atomic_write, validate_output_path};
use crate::pipeline::{AssetPipeline, BuildContext, BuildOutcome};
use crate::plan::{PageSelection, RebuildPlan};

/// Asset categories the production pipeline knows how to ship.
///
/// Used as the key in [`BuildOutcome::hashed_asset_urls`]. Adding a new
/// variant is the right way to wire a new asset (e.g. a manifest.json
/// or a sitemap-derived RSS feed) into the production pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetKind {
    /// The global stylesheet emitted by the CSS pipeline (Tailwind +
    /// CSS Modules). Mirrors `zfb_css::CssPipelineOutput`.
    Css,
    /// The islands client bundle emitted by the islands pipeline.
    /// Mirrors `zfb_islands::BundleOutput`.
    Islands,
}

/// One verbatim companion file shipped alongside an [`EmittedAsset`] entry.
///
/// Companions are written to the **same directory** as the hashed entry
/// under their `filename` verbatim — no content-hashing, no renaming.
/// The canonical use case is esbuild code-split chunks: the entry's
/// relative `import("./islands-chunk-<hash>.js")` bakes the chunk
/// filename in, so renaming the chunk would break the import.
///
/// The pipeline validates that `filename` is a safe flat basename before
/// writing (no path separators, no `..`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionFile {
    /// Flat basename for the companion (e.g. `islands-chunk-WOEGGERP.js`).
    /// Must not contain a path separator or `..`.
    pub filename: String,

    /// Raw bytes to write verbatim — never rewritten or hashed.
    pub bytes: Vec<u8>,
}

/// One asset the production pipeline is asked to ship.
///
/// The pipeline:
///
/// 1. Computes `sha256(bytes)[..8]`.
/// 2. Writes the bytes to
///    `<dist_root>/<filename_with_hash>` (where `filename_with_hash`
///    inserts the hash before the extension — `styles.css` →
///    `styles-<hash>.css`).
/// 3. Records the hashed URL in [`BuildOutcome::hashed_asset_urls`].
/// 4. Rewrites every occurrence of `stable_url` in the rendered HTML
///    to the new hashed URL.
/// 5. Writes each [`CompanionFile`] verbatim to the same directory.
///
/// `stable_url` and `relative_path` are kept as separate fields so a
/// caller can mount assets at a CDN URL while still emitting them under
/// `dist/assets/...` on disk.
#[derive(Debug, Clone)]
pub struct EmittedAsset {
    /// Asset bytes — the emitter is expected to have already minified
    /// them in production mode (CSS via `lightningcss::PrinterOptions
    /// { minify: true, .. }`, JS via esbuild `--minify`). The pipeline
    /// is opaque to the bytes.
    pub bytes: Vec<u8>,

    /// Output filename relative to `dist_root`. Must include the file
    /// extension. The hash is inserted before the extension. Example:
    /// `assets/styles.css` → `<dist_root>/assets/styles-<hash>.css`.
    pub relative_path: PathBuf,

    /// The unhashed public URL the rendered HTML uses by default
    /// (e.g. `/assets/styles.css`). The pipeline replaces every match
    /// of this string in HTML bodies with the hashed equivalent.
    ///
    /// `None` means "skip HTML rewriting": the asset still gets a
    /// hashed filename, but no rendered HTML references it by this
    /// stable URL. Useful for assets that are loaded by other assets
    /// rather than by the rendered HTML — the rewrites for those
    /// happen elsewhere.
    pub stable_url: Option<String>,

    /// Verbatim companion files to write beside the hashed entry.
    ///
    /// Each companion is written to the **same directory** as the entry
    /// under its own `filename` with no hashing or renaming. Empty for
    /// assets that have no companions (CSS, zero-chunk islands bundles).
    ///
    /// See [`CompanionFile`] for the flat-basename contract and the
    /// canonical code-split-chunks use case.
    pub companions: Vec<CompanionFile>,
}

/// Pluggable producer of an [`EmittedAsset`].
///
/// One-shot: called at most once per [`ProductionAssetPipeline::apply`]
/// invocation, when the corresponding `plan.rerun_*` flag is set. A
/// returned `None` is interpreted as "no asset to emit this tick"
/// (e.g. the project has no CSS), and the pipeline silently skips the
/// hashing + write + HTML rewrite for this asset slot.
pub trait AssetEmitter: Send + Sync {
    /// Produce the asset for this build cycle.
    fn emit(&self) -> Result<Option<EmittedAsset>>;
}

impl<F> AssetEmitter for F
where
    F: Fn() -> Result<Option<EmittedAsset>> + Send + Sync + 'static,
{
    fn emit(&self) -> Result<Option<EmittedAsset>> {
        (self)()
    }
}

/// Bundle of emitters [`ProductionAssetPipeline`] consults each tick.
///
/// Both emitters are optional: a project with no CSS or no islands
/// simply leaves the corresponding slot `None` and the pipeline skips
/// it.
#[derive(Default)]
pub struct ProductionEmitters {
    /// Emitter for the global CSS asset. Called when
    /// `plan.rerun_css == true`.
    pub css: Option<Box<dyn AssetEmitter>>,
    /// Emitter for the islands client bundle. Called when
    /// `plan.rerun_islands == true`.
    pub islands: Option<Box<dyn AssetEmitter>>,
}

impl std::fmt::Debug for ProductionEmitters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProductionEmitters")
            .field("css", &self.css.as_ref().map(|_| "<emitter>"))
            .field("islands", &self.islands.as_ref().map(|_| "<emitter>"))
            .finish()
    }
}

/// One-shot, content-addressed asset pipeline used by `zfb build`.
///
/// See the module-level docs for the full contract and the dev-vs-prod
/// trade-offs.
pub struct ProductionAssetPipeline {
    emitters: ProductionEmitters,
}

impl ProductionAssetPipeline {
    /// Construct a production pipeline with the given emitters.
    pub fn new(emitters: ProductionEmitters) -> Self {
        Self { emitters }
    }

    /// Construct a production pipeline with no asset emitters (useful
    /// for tests that only exercise the page-render path).
    pub fn empty() -> Self {
        Self::new(ProductionEmitters::default())
    }

    /// Borrow the emitter bundle.
    pub fn emitters(&self) -> &ProductionEmitters {
        &self.emitters
    }
}

impl std::fmt::Debug for ProductionAssetPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProductionAssetPipeline")
            .field("emitters", &self.emitters)
            .finish()
    }
}

impl AssetPipeline for ProductionAssetPipeline {
    fn apply(&self, plan: &RebuildPlan, ctx: &BuildContext) -> Result<BuildOutcome> {
        let mut outcome = BuildOutcome::default();

        // 0. Resolve the page list. Production never sees `All` after
        //    the orchestrator's resolution step, but mirror the dev
        //    pipeline's defensive check so a bypass surfaces clearly.
        let pages: Vec<PageId> = match &plan.pages {
            PageSelection::All => {
                return Err(anyhow::anyhow!(
                    "ProductionAssetPipeline: PageSelection::All must be resolved to a concrete \
                     page list by the orchestrator before reaching the pipeline"
                ));
            }
            PageSelection::Specific(s) => s.iter().cloned().collect(),
        };

        // 1. Render pages — but defer writing until after assets are
        //    emitted, so we can rewrite stable URLs to hashed URLs in
        //    one pass over each page body.
        let rendered = if pages.is_empty() {
            Vec::new()
        } else {
            // Production never narrows (issue #958): every selected
            // page renders in full.
            (ctx.render_pages)(&pages, None)?
        };
        outcome.pages_rendered = rendered.len();

        // 2. Run the asset emitters and collect (stable_url → hashed_url)
        //    rewrites.
        let mut rewrites: Vec<(String, String)> = Vec::new();

        if plan.rerun_css {
            outcome.css_rerun = true;
            if let Some(em) = self.emitters.css.as_ref() {
                if let Some(asset) = em.emit().context("production CSS emitter failed")? {
                    let hashed_url = ship_asset(ctx, &asset, AssetKind::Css, &mut outcome)?;
                    if let Some(stable) = asset.stable_url {
                        rewrites.push((stable, hashed_url));
                    }
                    outcome.css_changed = true;
                }
            }
        }

        if plan.rerun_islands {
            outcome.islands_rerun = true;
            if let Some(em) = self.emitters.islands.as_ref() {
                if let Some(asset) = em.emit().context("production islands emitter failed")? {
                    let hashed_url = ship_asset(ctx, &asset, AssetKind::Islands, &mut outcome)?;
                    if let Some(stable) = asset.stable_url {
                        rewrites.push((stable, hashed_url));
                    }
                    outcome.islands_changed = true;
                }
            }
        }

        // 3. Write each page, rewriting stable URLs to hashed URLs as
        //    we go. The rewrite is boundary-anchored substring
        //    replacement: a match is only rewritten when the byte
        //    immediately after the match is a URL delimiter (quote,
        //    whitespace, `<`, `>`, `?`, `#`, end-of-string). That
        //    prevents a stable URL like `/styles.css` from rewriting
        //    inside `/styles.css.map` (sourcemap reference) or any
        //    longer URL that happens to share a prefix.
        //
        //    Sort rewrites by `from` length descending so longer keys
        //    are applied before any shorter prefixes — defence in
        //    depth even with the boundary check, since two registered
        //    stable URLs could still nest.
        rewrites.sort_by_key(|r| std::cmp::Reverse(r.0.len()));
        for r in rendered {
            let dest = validate_output_path(&ctx.dist_root, r.output_path.as_path())
                .with_context(|| format!("while building page {:?}", r.page))?;
            let body = if rewrites.is_empty() {
                r.html
            } else {
                let mut buf = r.html;
                for (from, to) in &rewrites {
                    if buf.contains(from.as_str()) {
                        buf = boundary_replace(&buf, from, to);
                    }
                }
                buf
            };
            atomic_write(&dest, body.as_bytes())?;
            outcome.pages_written.push(r.page);
        }

        Ok(outcome)
    }
}

/// Replace each occurrence of `from` in `haystack` with `to`, but only
/// when the byte immediately after the match is a URL delimiter
/// (quote, whitespace, `<`, `>`, `?`, `#`, `\\`, `(`, `)`) or
/// start/end-of-string. A match adjacent to an alphanumeric, `.`, `_`,
/// or `-` is preserved.
///
/// Both leading and trailing bytes are checked. The trailing check
/// protects sourcemap references and other URLs that contain a
/// registered stable URL as a prefix: e.g. with `from = "/styles.css"`,
/// `to = "/styles-abc.css"`, the haystack
///   `<link href="/styles.css"> ... <a href="/styles.css.map">`
/// rewrites only the first occurrence. The leading check guards against
/// suffix collisions where `from` is itself a suffix of a longer URL,
/// e.g. `/foo.css` must NOT rewrite inside `/myfoo.css` — the `o`
/// preceding `/foo.css` is a non-delimiter byte.
///
/// Pure substring matching against UTF-8: the boundary bytes are read
/// directly from the byte slice, which is safe because any non-ASCII
/// continuation byte (0x80-0xBF) is treated as a non-delimiter and
/// therefore does NOT trigger a rewrite — the worst case is a missed
/// rewrite, never an incorrect one.
fn boundary_replace(haystack: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return haystack.to_string();
    }
    let bytes = haystack.as_bytes();
    let from_bytes = from.as_bytes();
    // Walk byte-by-byte looking for `from`, but build the output via
    // `str` slicing so multi-byte UTF-8 sequences in the surrounding
    // content are preserved verbatim. Both `last_copied` and `i` are
    // char boundaries by construction: `last_copied` only advances
    // past either ASCII bytes (incremented one at a time, so each byte
    // is its own char) or the full ASCII `from` match.
    let mut out = String::with_capacity(haystack.len());
    let mut i = 0usize;
    let mut last_copied = 0usize;
    while i < bytes.len() {
        if i + from_bytes.len() <= bytes.len() && &bytes[i..i + from_bytes.len()] == from_bytes {
            let before = if i == 0 { None } else { bytes.get(i - 1).copied() };
            let after = bytes.get(i + from_bytes.len()).copied();
            let is_leading_boundary = is_url_boundary_byte(before);
            let is_trailing_boundary = is_url_boundary_byte(after);
            if is_leading_boundary && is_trailing_boundary {
                out.push_str(&haystack[last_copied..i]);
                out.push_str(to);
                i += from_bytes.len();
                last_copied = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&haystack[last_copied..]);
    out
}

/// A byte is a URL boundary if it's missing (start/end of string) or one
/// of the typical surrounders for an HTML/CSS URL token: quotes,
/// whitespace, `<`, `>`, `?`, `#`, `\\`, `(`, `)`. Non-ASCII bytes
/// (including UTF-8 continuation bytes) are intentionally classified as
/// non-boundary so a multi-byte character adjacent to `from` is never
/// mistaken for a delimiter.
fn is_url_boundary_byte(b: Option<u8>) -> bool {
    match b {
        None => true,
        Some(b) => matches!(
            b,
            b'"' | b'\''
                | b'\\'
                | b'\n'
                | b'\r'
                | b'\t'
                | b' '
                | b'<'
                | b'>'
                | b'?'
                | b'#'
                | b'('
                | b')'
                // `=` is a legitimate URL boundary in query strings:
                // `<a href="/download?asset=/assets/styles.css">` should
                // still rewrite the asset URL, even though `=` is not a
                // surrounding-quote-style delimiter.
                | b'='
                // `,` and `;` show up inside `srcset="...x.png 1x, ..."`
                // attributes between candidate URLs.
                | b','
                | b';'
        ),
    }
}

/// Hash, write, and record the URL for a single emitted asset.
///
/// Writes the entry under a content-hashed filename, then writes each
/// [`CompanionFile`] verbatim in the same directory with no renaming.
///
/// Returns the hashed public URL the pipeline will rewrite into HTML.
fn ship_asset(
    ctx: &BuildContext,
    asset: &EmittedAsset,
    kind: AssetKind,
    outcome: &mut BuildOutcome,
) -> Result<String> {
    let hash = sha256_8(&asset.bytes);
    let hashed_relative = insert_hash_before_extension(&asset.relative_path, &hash);
    // The relative path comes from the asset emitter — validate before
    // joining so a malformed (e.g. absolute, traversal-laden, or
    // symlink-escaping) `relative_path` cannot land outside dist_root.
    let dest = validate_output_path(&ctx.dist_root, &hashed_relative).with_context(|| {
        format!(
            "production: refused to write hashed asset relative path {}",
            hashed_relative.display()
        )
    })?;

    atomic_write(&dest, &asset.bytes).with_context(|| {
        format!(
            "production: failed to write hashed asset {}",
            dest.display()
        )
    })?;

    // Write each companion verbatim to the same directory as the entry.
    // Companions must be FLAT basenames (no path separators / `..` / empty)
    // so they land beside the entry without escaping the asset directory —
    // a separator means esbuild's chunk contract was violated upstream, so
    // we reject loudly rather than ship it. The hashed entry's relative
    // directory is then re-joined and run through the same symlink-aware
    // `validate_output_path` the entry used, so a planted symlink inside
    // dist cannot redirect the write outside dist_root.
    if !asset.companions.is_empty() {
        let entry_rel_dir = hashed_relative.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        for companion in &asset.companions {
            if companion.filename.is_empty()
                || companion.filename.contains('/')
                || companion.filename.contains('\\')
                || companion.filename.contains("..")
            {
                return Err(anyhow::anyhow!(
                    "production: companion filename {:?} must be a non-empty flat basename \
                     (no path separator or `..`)",
                    companion.filename
                ));
            }
            let companion_rel = entry_rel_dir.join(&companion.filename);
            let companion_dest =
                validate_output_path(&ctx.dist_root, &companion_rel).with_context(|| {
                    format!(
                        "production: refused to write companion relative path {}",
                        companion_rel.display()
                    )
                })?;
            atomic_write(&companion_dest, &companion.bytes).with_context(|| {
                format!(
                    "production: failed to write companion file {}",
                    companion_dest.display()
                )
            })?;
        }
    }

    let hashed_url = if let Some(ref stable) = asset.stable_url {
        rewrite_url(stable, &asset.relative_path, &hashed_relative)
    } else {
        // No stable URL declared — synthesise one from the relative
        // path so callers logging `hashed_asset_urls` still see something
        // actionable. Leading `/` for parity with the typical declared
        // form `/assets/styles.css`.
        format!("/{}", path_to_url(&hashed_relative))
    };

    outcome.hashed_asset_urls.push((kind, hashed_url.clone()));
    Ok(hashed_url)
}

/// Insert `hash` before the extension of `path`. `assets/styles.css`
/// + `deadbeef` → `assets/styles-deadbeef.css`. Paths without an
///   extension get `-<hash>` appended to the file stem.
fn insert_hash_before_extension(path: &std::path::Path, hash: &str) -> PathBuf {
    let mut out = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned());
    let filename = match ext {
        Some(e) if !e.is_empty() => format!("{stem}-{hash}.{e}"),
        _ => format!("{stem}-{hash}"),
    };
    out.push(filename);
    out
}

/// Derive the hashed URL by replacing the unhashed filename portion of
/// `stable_url` with the hashed filename.
///
/// We do not assume `stable_url` matches `relative_path` exactly — the
/// caller may serve assets under a different URL prefix than the on-disk
/// layout (e.g. `/cdn/` mounted onto `dist/assets/`). The unhashed
/// filename is always present in `stable_url` by definition (the emitter
/// declared it), so we replace just that suffix.
fn rewrite_url(stable_url: &str, relative: &std::path::Path, hashed_relative: &std::path::Path) -> String {
    let unhashed_filename = relative
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let hashed_filename = hashed_relative
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if unhashed_filename.is_empty() || !stable_url.ends_with(&unhashed_filename) {
        // Defensive: stable_url didn't end with the declared filename.
        // Leave the URL untouched rather than guessing — the emitter
        // is wrong and HTML rewriting won't fire.
        return stable_url.to_string();
    }
    let prefix = &stable_url[..stable_url.len() - unhashed_filename.len()];
    format!("{prefix}{hashed_filename}")
}

/// SHA-256 of `bytes`, truncated to 8 hex characters. Mirrors the
/// `zfb_css::hash_8` / `zfb_islands::hash_8` convention.
fn sha256_8(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let full = hex::encode(digest);
    full[..8].to_string()
}

/// Convert a relative path to a forward-slash URL fragment, suitable
/// for the synthesised hashed URL when no `stable_url` was declared.
fn path_to_url(path: &std::path::Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{BuildContext, RelDistPath, RenderedPage};
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn boundary_replace_rewrites_at_quote_boundaries() {
        let html = r#"<link href="/styles.css">"#;
        let out = boundary_replace(html, "/styles.css", "/styles-abc.css");
        assert_eq!(out, r#"<link href="/styles-abc.css">"#);
    }

    #[test]
    fn boundary_replace_does_not_rewrite_inside_longer_url() {
        // Round 2 regression: `/styles.css` must NOT rewrite inside
        // `/styles.css.map`. The `.` after the match is not a
        // delimiter, so the boundary check rejects it.
        let html = r#"<a href="/styles.css.map">"#;
        let out = boundary_replace(html, "/styles.css", "/styles-abc.css");
        assert_eq!(out, r#"<a href="/styles.css.map">"#);
    }

    #[test]
    fn boundary_replace_mixed_match_and_no_match() {
        let html = concat!(
            r#"<link rel="stylesheet" href="/styles.css">"#,
            r#"<a href="/styles.css.map">map</a>"#,
        );
        let out = boundary_replace(html, "/styles.css", "/styles-abc.css");
        let expected = concat!(
            r#"<link rel="stylesheet" href="/styles-abc.css">"#,
            r#"<a href="/styles.css.map">map</a>"#,
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn boundary_replace_handles_end_of_string() {
        let out = boundary_replace("/styles.css", "/styles.css", "/styles-abc.css");
        assert_eq!(out, "/styles-abc.css");
    }

    #[test]
    fn boundary_replace_preserves_multibyte_surroundings() {
        // The output construction must keep multi-byte UTF-8 intact.
        let html = "前<link href=\"/styles.css\">後";
        let out = boundary_replace(html, "/styles.css", "/styles-abc.css");
        assert_eq!(out, "前<link href=\"/styles-abc.css\">後");
    }

    #[test]
    fn boundary_replace_does_not_rewrite_with_leading_non_delimiter() {
        // Round 3 regression: `/foo.css` must NOT rewrite inside
        // `/myfoo.css`. The `o` preceding the match is a non-delimiter,
        // so the leading boundary check rejects it. (The trailing
        // check alone is insufficient — `/myfoo.css` ends with a quote
        // delimiter and would otherwise pass.)
        let html = r#"<link href="/myfoo.css">"#;
        let out = boundary_replace(html, "/foo.css", "/foo-abc.css");
        assert_eq!(out, r#"<link href="/myfoo.css">"#);
    }

    #[test]
    fn boundary_replace_rewrites_at_start_of_string() {
        // No leading byte → boundary. Match should fire.
        let out = boundary_replace("/styles.css\"", "/styles.css", "/styles-abc.css");
        assert_eq!(out, "/styles-abc.css\"");
    }

    #[test]
    fn boundary_replace_rewrites_after_query_param_equals() {
        // `=` is a valid URL boundary in query strings:
        // `?asset=/assets/styles.css` should rewrite the asset URL.
        let html = r#"<a href="/download?asset=/assets/styles.css">dl</a>"#;
        let out = boundary_replace(html, "/assets/styles.css", "/assets/styles-abc.css");
        assert_eq!(
            out,
            r#"<a href="/download?asset=/assets/styles-abc.css">dl</a>"#
        );
    }

    #[test]
    fn boundary_replace_rewrites_inside_srcset() {
        // `srcset` separates candidates with `,`. Both candidates should rewrite.
        let html = r#"<img srcset="/img/a.png 1x,/img/b.png 2x">"#;
        let out_a = boundary_replace(html, "/img/a.png", "/img/a-1.png");
        let out_b = boundary_replace(&out_a, "/img/b.png", "/img/b-2.png");
        assert_eq!(
            out_b,
            r#"<img srcset="/img/a-1.png 1x,/img/b-2.png 2x">"#
        );
    }

    #[test]
    fn boundary_replace_rewrites_inside_url_function() {
        // CSS `url(/foo.css)` — both `(` and `)` count as delimiters.
        let css = "background:url(/foo.css);";
        let out = boundary_replace(css, "/foo.css", "/foo-abc.css");
        assert_eq!(out, "background:url(/foo-abc.css);");
    }


    fn pid(s: &str) -> PageId {
        PageId::new(PathBuf::from(s))
    }

    fn render_one(html: impl Into<String>, output: &str) -> RenderedPage {
        RenderedPage {
            page: pid(&format!("/p{output}")),
            output_path: RelDistPath::new(output.trim_start_matches('/')).unwrap(),
            html: html.into(),
            content_type: None,
        }
    }

    fn ctx_with_pages(dist_root: PathBuf, pages: Vec<RenderedPage>) -> BuildContext {
        BuildContext {
            dist_root,
            render_pages: Arc::new(move |_pages: &[PageId], _: Option<&crate::ContentNarrowing>| {
                Ok(pages.clone())
            }),
            run_css: None,
            run_islands: None,
            reload_renderer: None,
        }
    }

    fn plan_full(pages: Vec<&str>) -> RebuildPlan {
        let mut sel = BTreeSet::new();
        for p in pages {
            sel.insert(pid(p));
        }
        RebuildPlan {
            pages: PageSelection::Specific(sel),
            rerun_css: true,
            rerun_islands: true,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        }
    }

    /// Production builds emit hashed asset filenames, write the bytes
    /// to disk, and rewrite the stable URL in rendered HTML to the
    /// hashed URL. This is the core acceptance criterion for sub-task
    /// 8 — every asset reference in the rendered HTML must use the
    /// hashed filename.
    #[test]
    fn prod_pipeline_hashes_css_and_rewrites_html() {
        let dir = tempdir().unwrap();
        let css_bytes = b".btn{color:red}".to_vec();
        let css_emitter = move || {
            Ok(Some(EmittedAsset {
                bytes: css_bytes.clone(),
                relative_path: PathBuf::from("assets/styles.css"),
                stable_url: Some("/assets/styles.css".into()),
                companions: Vec::new(),
            }))
        };
        let pipeline = ProductionAssetPipeline::new(ProductionEmitters {
            css: Some(Box::new(css_emitter)),
            islands: None,
        });
        let pages = vec![render_one(
            "<html><head><link rel=\"stylesheet\" href=\"/assets/styles.css\"></head><body/></html>",
            "/index.html",
        )];
        let ctx = ctx_with_pages(dir.path().to_path_buf(), pages);
        let plan = plan_full(vec!["//index.html"]);
        let outcome = pipeline.apply(&plan, &ctx).unwrap();

        // Exactly one CSS asset url was emitted, with hash baked in.
        assert_eq!(outcome.hashed_asset_urls.len(), 1);
        let (kind, url) = &outcome.hashed_asset_urls[0];
        assert_eq!(*kind, AssetKind::Css);
        assert!(
            url.starts_with("/assets/styles-") && url.ends_with(".css") && url.len() == "/assets/styles-12345678.css".len(),
            "expected /assets/styles-<8hex>.css; got {url}",
        );

        // The hashed CSS file is on disk; the stable filename is not.
        let entries: Vec<String> = std::fs::read_dir(dir.path().join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries.len(), 1, "expected exactly one css asset on disk");
        assert!(
            entries[0].starts_with("styles-") && entries[0].ends_with(".css"),
            "got {entries:?}",
        );

        // HTML body had its `/assets/styles.css` reference rewritten.
        let html = std::fs::read_to_string(dir.path().join("index.html")).unwrap();
        assert!(
            !html.contains("/assets/styles.css\""),
            "stable URL leaked into HTML: {html}",
        );
        assert!(html.contains(url), "hashed URL not present in HTML: {html}");
    }

    /// Identical input bytes produce the same hash so the URL is
    /// stable across builds. A byte change cascades into a new URL.
    #[test]
    fn prod_pipeline_hash_is_byte_stable_and_change_sensitive() {
        let dir = tempdir().unwrap();

        let bytes_a = b".x{color:red}".to_vec();
        let url_a = {
            let css_a = bytes_a.clone();
            let pipeline = ProductionAssetPipeline::new(ProductionEmitters {
                css: Some(Box::new(move || {
                    Ok(Some(EmittedAsset {
                        bytes: css_a.clone(),
                        relative_path: PathBuf::from("assets/styles.css"),
                        stable_url: Some("/assets/styles.css".into()),
                        companions: Vec::new(),
                    }))
                })),
                islands: None,
            });
            let ctx = ctx_with_pages(dir.path().to_path_buf(), vec![]);
            let plan = plan_full(vec![]);
            let outcome = pipeline.apply(&plan, &ctx).unwrap();
            outcome.hashed_asset_urls[0].1.clone()
        };

        // Identical bytes → identical hashed URL.
        let url_a2 = {
            let css_a = bytes_a.clone();
            let pipeline = ProductionAssetPipeline::new(ProductionEmitters {
                css: Some(Box::new(move || {
                    Ok(Some(EmittedAsset {
                        bytes: css_a.clone(),
                        relative_path: PathBuf::from("assets/styles.css"),
                        stable_url: Some("/assets/styles.css".into()),
                        companions: Vec::new(),
                    }))
                })),
                islands: None,
            });
            let ctx = ctx_with_pages(dir.path().to_path_buf(), vec![]);
            let plan = plan_full(vec![]);
            let outcome = pipeline.apply(&plan, &ctx).unwrap();
            outcome.hashed_asset_urls[0].1.clone()
        };
        assert_eq!(url_a, url_a2, "byte-identical assets must produce the same URL");

        // Differing bytes → differing URL.
        let url_b = {
            let css_b = b".x{color:blue}".to_vec();
            let pipeline = ProductionAssetPipeline::new(ProductionEmitters {
                css: Some(Box::new(move || {
                    Ok(Some(EmittedAsset {
                        bytes: css_b.clone(),
                        relative_path: PathBuf::from("assets/styles.css"),
                        stable_url: Some("/assets/styles.css".into()),
                        companions: Vec::new(),
                    }))
                })),
                islands: None,
            });
            let ctx = ctx_with_pages(dir.path().to_path_buf(), vec![]);
            let plan = plan_full(vec![]);
            let outcome = pipeline.apply(&plan, &ctx).unwrap();
            outcome.hashed_asset_urls[0].1.clone()
        };
        assert_ne!(url_a, url_b, "differing bytes must produce a differing URL");
    }

    /// A second emitter (islands) is shipped through the same path —
    /// hashed filename, separate URL entry, separate HTML rewrite.
    #[test]
    fn prod_pipeline_hashes_both_css_and_islands() {
        let dir = tempdir().unwrap();
        let css_emitter = || {
            Ok(Some(EmittedAsset {
                bytes: b"/* css */".to_vec(),
                relative_path: PathBuf::from("assets/styles.css"),
                stable_url: Some("/assets/styles.css".into()),
                companions: Vec::new(),
            }))
        };
        let islands_emitter = || {
            Ok(Some(EmittedAsset {
                bytes: b"// islands".to_vec(),
                relative_path: PathBuf::from("assets/islands.js"),
                stable_url: Some("/assets/islands.js".into()),
                companions: Vec::new(),
            }))
        };
        let pipeline = ProductionAssetPipeline::new(ProductionEmitters {
            css: Some(Box::new(css_emitter)),
            islands: Some(Box::new(islands_emitter)),
        });

        let html = "\
            <html><head>\
            <link rel=\"stylesheet\" href=\"/assets/styles.css\">\
            <script type=\"module\" src=\"/assets/islands.js\"></script>\
            </head><body/></html>";
        let pages = vec![render_one(html, "/index.html")];
        let ctx = ctx_with_pages(dir.path().to_path_buf(), pages);
        let plan = plan_full(vec!["//index.html"]);
        let outcome = pipeline.apply(&plan, &ctx).unwrap();

        assert_eq!(outcome.hashed_asset_urls.len(), 2);
        let kinds: Vec<AssetKind> = outcome
            .hashed_asset_urls
            .iter()
            .map(|(k, _)| *k)
            .collect();
        assert!(kinds.contains(&AssetKind::Css));
        assert!(kinds.contains(&AssetKind::Islands));

        let dist_html = std::fs::read_to_string(dir.path().join("index.html")).unwrap();
        assert!(
            !dist_html.contains("/assets/styles.css\"") && !dist_html.contains("/assets/islands.js\""),
            "stable URLs leaked: {dist_html}",
        );
        for (_, url) in &outcome.hashed_asset_urls {
            assert!(dist_html.contains(url), "hashed url {url} missing from HTML: {dist_html}");
        }

        // Both hashed files exist on disk with no stable copies left
        // behind.
        let assets: Vec<String> = std::fs::read_dir(dir.path().join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(assets.len(), 2);
        assert!(assets.iter().any(|n| n.starts_with("styles-") && n.ends_with(".css")));
        assert!(assets.iter().any(|n| n.starts_with("islands-") && n.ends_with(".js")));
    }

    /// Companion files are written verbatim beside the hashed entry —
    /// no content hashing, no rename, no HTML rewrite touching their
    /// bytes (#808).
    #[test]
    fn prod_pipeline_ships_companions_verbatim() {
        let dir = tempdir().unwrap();
        let chunk_bytes = b"import(\"./other.js\");export const x=1;".to_vec();
        let chunk_for_emitter = chunk_bytes.clone();
        let islands_emitter = move || {
            Ok(Some(EmittedAsset {
                bytes: b"// entry\nimport(\"./islands-chunk-AAAA1111.js\");".to_vec(),
                relative_path: PathBuf::from("assets/islands.js"),
                stable_url: Some("/assets/islands.js".into()),
                companions: vec![CompanionFile {
                    filename: "islands-chunk-AAAA1111.js".to_string(),
                    bytes: chunk_for_emitter.clone(),
                }],
            }))
        };
        let pipeline = ProductionAssetPipeline::new(ProductionEmitters {
            css: None,
            islands: Some(Box::new(islands_emitter)),
        });
        let ctx = ctx_with_pages(dir.path().to_path_buf(), vec![]);
        let plan = plan_full(vec![]);
        let outcome = pipeline.apply(&plan, &ctx).unwrap();

        // Only the entry is reported as a hashed asset URL; the chunk is not.
        assert_eq!(outcome.hashed_asset_urls.len(), 1);

        let assets: Vec<String> = std::fs::read_dir(dir.path().join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        // Hashed entry + verbatim chunk.
        assert_eq!(assets.len(), 2, "expected hashed entry + chunk; got {assets:?}");
        assert!(
            assets.iter().any(|n| n.starts_with("islands-") && n.ends_with(".js") && !n.contains("chunk")),
            "hashed entry missing: {assets:?}",
        );
        assert!(
            assets.iter().any(|n| n == "islands-chunk-AAAA1111.js"),
            "verbatim chunk missing: {assets:?}",
        );
        // Chunk bytes are byte-identical to the input — never rewritten.
        let on_disk = std::fs::read(dir.path().join("assets").join("islands-chunk-AAAA1111.js")).unwrap();
        assert_eq!(on_disk, chunk_bytes);
    }

    /// A companion with a non-flat filename (path separator / `..` /
    /// empty) is rejected loudly rather than written — esbuild's chunk
    /// contract guarantees flat basenames, so anything else is a bug
    /// upstream we refuse to ship.
    #[test]
    fn prod_pipeline_rejects_non_flat_companion_filenames() {
        for bad in ["../escape.js", "sub/dir.js", "a\\b.js", ""] {
            let dir = tempdir().unwrap();
            let bad = bad.to_string();
            let islands_emitter = move || {
                Ok(Some(EmittedAsset {
                    bytes: b"// entry".to_vec(),
                    relative_path: PathBuf::from("assets/islands.js"),
                    stable_url: Some("/assets/islands.js".into()),
                    companions: vec![CompanionFile {
                        filename: bad.clone(),
                        bytes: b"x".to_vec(),
                    }],
                }))
            };
            let pipeline = ProductionAssetPipeline::new(ProductionEmitters {
                css: None,
                islands: Some(Box::new(islands_emitter)),
            });
            let ctx = ctx_with_pages(dir.path().to_path_buf(), vec![]);
            let plan = plan_full(vec![]);
            assert!(
                pipeline.apply(&plan, &ctx).is_err(),
                "non-flat companion filename must be rejected",
            );
        }
    }

    /// Production never relies on the dev-style bool runners. Even
    /// when `plan.rerun_css` is set and `BuildContext::run_css` is
    /// `Some`, the runner must NOT be invoked — production reads bytes
    /// through its own [`AssetEmitter`] instead.
    #[test]
    fn prod_pipeline_ignores_dev_bool_runners() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempdir().unwrap();
        let bool_runner_calls = Arc::new(AtomicUsize::new(0));
        let calls_cb = bool_runner_calls.clone();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_, _| Ok(vec![])),
            run_css: Some(Arc::new(move || {
                calls_cb.fetch_add(1, Ordering::SeqCst);
                Ok(true)
            })),
            run_islands: None,
            reload_renderer: None,
        };
        let pipeline = ProductionAssetPipeline::empty();
        let plan = plan_full(vec![]);
        let _ = pipeline.apply(&plan, &ctx).unwrap();
        assert_eq!(
            bool_runner_calls.load(Ordering::SeqCst),
            0,
            "ProductionAssetPipeline must not call dev-style run_css runner"
        );
    }

    /// `plan.rerun_css` without a CSS emitter is a quiet no-op: the
    /// outcome reports `css_rerun = true` (the plan asked for it) but
    /// `css_changed = false` (no asset was emitted).
    #[test]
    fn prod_pipeline_skips_emit_when_no_emitter_registered() {
        let dir = tempdir().unwrap();
        let pipeline = ProductionAssetPipeline::empty();
        let ctx = ctx_with_pages(dir.path().to_path_buf(), vec![]);
        let plan = plan_full(vec![]);
        let outcome = pipeline.apply(&plan, &ctx).unwrap();
        assert!(outcome.css_rerun);
        assert!(!outcome.css_changed);
        assert!(outcome.islands_rerun);
        assert!(!outcome.islands_changed);
        assert!(outcome.hashed_asset_urls.is_empty());
    }

    /// Production refuses `PageSelection::All` — same defensive
    /// contract as [`super::dev::DevAssetPipeline`].
    #[test]
    fn prod_pipeline_rejects_unresolved_all() {
        let dir = tempdir().unwrap();
        let pipeline = ProductionAssetPipeline::empty();
        let ctx = BuildContext {
            dist_root: dir.path().to_path_buf(),
            render_pages: Arc::new(|_, _| Ok(vec![])),
            run_css: None,
            run_islands: None,
            reload_renderer: None,
        };
        let plan = RebuildPlan {
            pages: PageSelection::All,
            rerun_css: false,
            rerun_islands: false,
            renderer_fresh: false,
            ssr_reload_needed: false,
            prune_paths: vec![],
            triggers: vec![],
            content_narrowing: None,
        };
        assert!(pipeline.apply(&plan, &ctx).is_err());
    }

    #[test]
    fn insert_hash_before_extension_handles_common_cases() {
        assert_eq!(
            insert_hash_before_extension(std::path::Path::new("assets/styles.css"), "deadbeef"),
            PathBuf::from("assets/styles-deadbeef.css"),
        );
        assert_eq!(
            insert_hash_before_extension(std::path::Path::new("islands.js"), "abcdef12"),
            PathBuf::from("islands-abcdef12.js"),
        );
        // No extension: append `-<hash>` to the stem.
        assert_eq!(
            insert_hash_before_extension(std::path::Path::new("assets/manifest"), "01020304"),
            PathBuf::from("assets/manifest-01020304"),
        );
    }

    #[test]
    fn rewrite_url_swaps_only_the_filename_portion() {
        // Stable URL co-located with the on-disk layout.
        let from = "/assets/styles.css";
        let rel = std::path::Path::new("assets/styles.css");
        let hashed = std::path::Path::new("assets/styles-deadbeef.css");
        assert_eq!(rewrite_url(from, rel, hashed), "/assets/styles-deadbeef.css");

        // Stable URL on a CDN prefix decoupled from the on-disk path.
        let cdn = "https://cdn.example.test/v1/styles.css";
        assert_eq!(
            rewrite_url(cdn, rel, hashed),
            "https://cdn.example.test/v1/styles-deadbeef.css"
        );
    }

    #[test]
    fn sha256_8_is_byte_stable() {
        let a = sha256_8(b"hello");
        let b = sha256_8(b"hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
        assert_ne!(a, sha256_8(b"hellp"));
    }
}
