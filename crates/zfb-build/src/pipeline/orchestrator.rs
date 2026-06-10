//! Production-build orchestration glue (S4).
//!
//! Wires the orphaned [`ProductionAssetPipeline`] into the [`zfb build`]
//! flow. The renderer (`render_all`) writes HTML files to disk with
//! **stable** asset URLs in `<head>` (CSS link, islands script). Then
//! this module:
//!
//! 1. Constructs [`ProductionEmitters`] from the supplied
//!    [`ProdAssetEmitterInputs`] (raw bytes the CSS / islands crates
//!    already produced).
//! 2. Hands the on-disk HTML files back to
//!    [`ProductionAssetPipeline::apply`] via a synthetic
//!    [`BuildContext::render_pages`] that **reads** each page from disk
//!    rather than re-rendering it.
//! 3. The pipeline then hashes + writes hashed asset files under
//!    `dist/assets/`, rewrites every match of the stable URL in HTML
//!    bodies to the hashed URL, and atomic-writes the page back to
//!    disk.
//!
//! ## Why a "render-from-disk" callback rather than a parallel apply?
//!
//! `ProductionAssetPipeline::apply` already owns the well-tested
//! ordering (emit → hash → URL rewrite → atomic write) and the
//! `boundary_replace` URL-rewrite contract. Recreating that here would
//! duplicate logic without value. Calling `apply` with a callback that
//! reads HTML from disk is the smallest possible glue that satisfies
//! the build flow's "render-first, emit-and-rewrite-after" ordering
//! constraint.
//!
//! ## Why are emitter outputs *raw bytes* rather than borrowed handles?
//!
//! `zfb-build` deliberately does not depend on `zfb-css` or
//! `zfb-islands`'s engine internals — it consumes the bytes-only
//! adapter outputs from S2 and S3. Forwarding raw `Vec<u8>` plus the
//! stable URL keeps `zfb-build` opaque to how the bytes were produced
//! (Tailwind subprocess, native engine, esbuild, …). The bin crate
//! (`zfb`) constructs the engines and hands the bytes here.
//!
//! ## Empty-island case
//!
//! `ProdAssetEmitterInputs::islands` is `Option<…>` — `None` means
//! "this project has no `"use client"` components, do not emit any
//! islands asset and do not splice any `<script>` tag." The renderer
//! is expected to leave `prod_head_assets.island_module_urls` empty in
//! that case so HTML never references a non-existent islands bundle.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use zfb_graph::PageId;

use crate::pipeline::prod::{
    AssetEmitter, CompanionFile, EmittedAsset, ProductionAssetPipeline, ProductionEmitters,
};
use crate::pipeline::{AssetPipeline, BuildContext, BuildOutcome, RelDistPath, RenderedPage};
use crate::plan::{PageSelection, RebuildPlan};

/// One emitter's already-produced bytes + stable-URL contract.
///
/// Constructed by the bin crate after running [`zfb_css::CssPipeline::build_emitter`]
/// (CSS) or [`zfb_islands::build_production_islands_asset`] (islands).
#[derive(Debug, Clone)]
pub struct AssetEmitterPayload {
    /// Asset bytes — already minified per the producing pipeline's
    /// `minify=true` flag in production.
    pub bytes: Vec<u8>,
    /// Output path relative to `dist_root`, e.g. `assets/styles.css`.
    /// The pipeline splices the hash before the extension.
    pub relative_path: PathBuf,
    /// Stable public URL the renderer embedded in the page head, e.g.
    /// `/assets/styles.css`. The pipeline rewrites every boundary-
    /// anchored match in HTML to the hashed equivalent.
    pub stable_url: String,
    /// Verbatim companion files to write beside the hashed entry (e.g.
    /// esbuild code-split chunks for the islands bundle). Written to the
    /// same directory as the entry with no hashing or renaming.
    /// Empty for assets without companions (CSS, zero-chunk islands).
    pub companions: Vec<CompanionFile>,
}

/// Inputs to [`apply_prod_asset_pipeline`].
///
/// Both optional single-asset slots (`css`, `islands`) and the
/// `client_scripts` Vec are independently optional / independently empty —
/// dropping a slot to `None` / leaving the Vec empty silently skips emission
/// for that asset kind. The pipeline still runs (HTML is round-tripped through
/// the rewrite step) but no hashed file lands on disk for the missing slot.
///
/// A project with no CSS, no islands, and no client-script entries sees
/// byte-identical output to a pre-client-script build.
#[derive(Debug, Default)]
pub struct ProdAssetEmitterInputs {
    /// Bytes-only payload for the CSS asset. `None` ⇒ skip CSS emit.
    pub css: Option<AssetEmitterPayload>,
    /// Bytes-only payload for the islands asset. `None` ⇒ skip islands
    /// emit (e.g. project has no `"use client"` components).
    pub islands: Option<AssetEmitterPayload>,
    /// Zero or more bytes-only payloads for per-entry client-script bundles.
    /// Empty ⇒ no client scripts to emit (project has no
    /// `*.client.{ts,tsx,js,jsx}` entries). Each element produces one
    /// `dist/assets/client/<name>-<hash>.js` and one HTML rewrite entry.
    pub client_scripts: Vec<AssetEmitterPayload>,
}

/// One HTML file the renderer wrote during the prod build.
///
/// `output_path` is the [`RelDistPath`] under `dist_root` where the
/// renderer wrote the page — same value the renderer's
/// `RouteUniverseEntry::output_path` carried, validated. The pipeline
/// reads the bytes off disk via this path, runs the URL rewrite, and
/// atomic-writes the result back to the same path.
#[derive(Debug, Clone)]
pub struct ProdRenderedFile {
    /// Synthesized page id — used only as a correlation key in
    /// [`BuildOutcome::pages_written`]. The orchestrator does not
    /// thread real `PageId`s through `render_all` / `RouteUniverseEntry`,
    /// so we manufacture one from the output path.
    pub page: PageId,
    /// Validated relative path under `dist_root`.
    pub output_path: RelDistPath,
}

/// Drive [`ProductionAssetPipeline`] over the already-rendered HTML
/// files.
///
/// Pre-conditions:
///
/// - Every `pages[i].output_path` resolves to an existing file under
///   `dist_dir` (the renderer wrote it). Missing files surface as a
///   clean error rather than a silent skip.
/// - The HTML referenced by each file already contains the stable
///   asset URLs in `<head>` (the renderer received `prod_head_assets`
///   with the stable URL constants from `zfb_types::asset_urls`).
///
/// Post-conditions:
///
/// - `dist_dir/<asset.relative_path with hash>` exists for every
///   non-`None` emitter slot.
/// - Each HTML page on disk has its stable URLs rewritten to the
///   hashed forms. No unhashed URL leaks.
/// - The returned [`BuildOutcome`] reports the hashed asset URLs (one
///   entry per emitted asset).
pub fn apply_prod_asset_pipeline(
    dist_dir: &Path,
    pages: Vec<ProdRenderedFile>,
    inputs: ProdAssetEmitterInputs,
) -> Result<BuildOutcome> {
    let emitters = build_emitters(inputs);
    let pipeline = ProductionAssetPipeline::new(emitters);

    // Materialize a (page_id → on-disk html) map keyed by `PageId` so
    // the synthetic `render_pages` callback can hand the bytes back to
    // `apply` in input order (apply does not care about the order, but
    // determinism helps `BuildOutcome::pages_written`).
    let dist_root = dist_dir.to_path_buf();
    let pages_for_callback = pages.clone();
    let render_pages: crate::pipeline::PageRenderer =
        Arc::new(move |_requested: &[PageId], _narrowing| {
            // The plan's page set MUST already cover every page in
            // `pages_for_callback` (we constructed the plan that way), so
            // we ignore `_requested` and return the full list. The plan
            // ordering doesn't matter; `apply` just iterates the result
            // verbatim. The narrowing hint is ignored — production renders
            // every page in full (issue #958).
            let mut out = Vec::with_capacity(pages_for_callback.len());
            for entry in &pages_for_callback {
                let on_disk = dist_root.join(entry.output_path.as_path());
                let html = std::fs::read_to_string(&on_disk).with_context(|| {
                    format!(
                        "prod orchestrator: failed to read rendered page from disk at {}",
                        on_disk.display()
                    )
                })?;
                out.push(RenderedPage {
                    page: entry.page.clone(),
                    output_path: entry.output_path.clone(),
                    html,
                    content_type: None,
                });
            }
            Ok(out)
        });

    let ctx = BuildContext {
        dist_root: dist_dir.to_path_buf(),
        render_pages,
        run_css: None,
        run_islands: None,
        reload_renderer: None,
    };

    // Build a plan that opts into both emit slots and lists every page
    // by its synthetic id. Even when a slot's emitter is `None` the
    // pipeline silently skips it (see `ProductionEmitters::default`),
    // so over-eager `rerun_css = true` / `rerun_islands = true` is
    // safe.
    let mut selection = std::collections::BTreeSet::new();
    for p in &pages {
        selection.insert(p.page.clone());
    }
    if selection.len() != pages.len() {
        return Err(anyhow!(
            "prod orchestrator: duplicate page ids in input ({} pages, {} unique ids)",
            pages.len(),
            selection.len()
        ));
    }
    let plan = RebuildPlan {
        pages: PageSelection::Specific(selection),
        rerun_css: true,
        rerun_islands: true,
        // One-shot production build: the render session is constructed
        // against the bundle that was just built — nothing to reload.
        renderer_fresh: true,
        ssr_reload_needed: false,
        prune_paths: vec![],
        triggers: vec![],
        content_narrowing: None,
    };

    pipeline.apply(&plan, &ctx)
}

fn build_emitters(inputs: ProdAssetEmitterInputs) -> ProductionEmitters {
    ProductionEmitters {
        css: inputs.css.map(payload_to_emitter),
        islands: inputs.islands.map(payload_to_emitter),
        client_scripts: inputs
            .client_scripts
            .into_iter()
            .map(payload_to_emitter)
            .collect(),
    }
}

/// Wrap an [`AssetEmitterPayload`] in the `Fn() -> Result<Option<EmittedAsset>>`
/// shape that the [`AssetEmitter`] blanket impl expects. The bytes are
/// captured by value into the closure, which is fine because each
/// emitter is invoked at most once per `apply` call.
fn payload_to_emitter(payload: AssetEmitterPayload) -> Box<dyn AssetEmitter> {
    Box::new(move || {
        Ok(Some(EmittedAsset {
            bytes: payload.bytes.clone(),
            relative_path: payload.relative_path.clone(),
            stable_url: Some(payload.stable_url.clone()),
            companions: payload.companions.clone(),
        }))
    })
}

/// Synthesize a [`PageId`] from a `RelDistPath`. Used by callers that
/// have an output path but no real page id (the renderer's
/// `RouteUniverseEntry` shape doesn't thread `PageId` through).
///
/// The id is opaque — the orchestrator only uses it as a hashable key
/// in `PageSelection::Specific` and surfaces it back via
/// `BuildOutcome::pages_written` for tooling that wants per-tick
/// reporting.
pub fn synthesize_page_id_from_output(output_path: &RelDistPath) -> PageId {
    PageId::new(output_path.as_path().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Write a stub HTML file under `dist_dir/<rel>` and return its
    /// `(PageId, RelDistPath)` shape.
    fn stage_html(dist_dir: &Path, rel: &str, body: &str) -> ProdRenderedFile {
        let abs = dist_dir.join(rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&abs, body).unwrap();
        let rel_path = RelDistPath::new(rel).unwrap();
        ProdRenderedFile {
            page: synthesize_page_id_from_output(&rel_path),
            output_path: rel_path,
        }
    }

    #[test]
    fn happy_path_writes_hashed_css_and_rewrites_html() {
        let dir = tempdir().unwrap();
        let dist = dir.path();

        // The renderer would have written this — stable URL in head.
        let html_with_stable = "<!doctype html><html><head>\
            <link rel=\"stylesheet\" href=\"/assets/styles.css\">\
            </head><body>hi</body></html>";
        let pages = vec![stage_html(dist, "index.html", html_with_stable)];

        let css_bytes = b".btn{color:red}".to_vec();
        let inputs = ProdAssetEmitterInputs {
            css: Some(AssetEmitterPayload {
                bytes: css_bytes,
                relative_path: PathBuf::from("assets/styles.css"),
                stable_url: "/assets/styles.css".to_string(),
                companions: Vec::new(),
            }),
            islands: None,
            ..Default::default()
        };

        let outcome = apply_prod_asset_pipeline(dist, pages, inputs).unwrap();

        // Acceptance criterion (a): hashed CSS lands on disk under
        // dist/assets/styles-<8hex>.css with non-zero bytes.
        let entries: Vec<String> = fs::read_dir(dist.join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries.len(), 1, "expected exactly one css asset on disk");
        let name = &entries[0];
        assert!(
            name.starts_with("styles-")
                && name.ends_with(".css")
                && name.len() == "styles-12345678.css".len(),
            "expected styles-<8hex>.css; got {name}"
        );
        let bytes = fs::read(dist.join("assets").join(name)).unwrap();
        assert!(!bytes.is_empty(), "hashed css must contain bytes");

        // Acceptance criterion (b): HTML contains the hashed URL.
        let html = fs::read_to_string(dist.join("index.html")).unwrap();
        let hashed_url = format!("/assets/{name}");
        assert!(
            html.contains(&hashed_url),
            "hashed URL {hashed_url} missing from HTML: {html}"
        );

        // Acceptance criterion (c): HTML does NOT contain the unhashed
        // URL anymore (boundary-anchored rewrite stripped it).
        assert!(
            !html.contains("/assets/styles.css\""),
            "stable URL leaked into HTML: {html}"
        );

        // BuildOutcome reports the hashed url.
        assert_eq!(outcome.hashed_asset_urls.len(), 1);
        assert_eq!(outcome.hashed_asset_urls[0].1, hashed_url);
    }

    #[test]
    fn no_islands_input_skips_islands_asset_emission() {
        // Acceptance: when the project has no islands, no
        // dist/assets/islands-*.js is written and HTML is left
        // untouched on the islands axis.
        let dir = tempdir().unwrap();
        let dist = dir.path();
        let html = "<!doctype html><html><head><title>x</title></head><body/></html>";
        let pages = vec![stage_html(dist, "index.html", html)];

        let inputs = ProdAssetEmitterInputs {
            css: Some(AssetEmitterPayload {
                bytes: b"/*tiny*/".to_vec(),
                relative_path: PathBuf::from("assets/styles.css"),
                stable_url: "/assets/styles.css".to_string(),
                companions: Vec::new(),
            }),
            islands: None,
            ..Default::default()
        };

        let outcome = apply_prod_asset_pipeline(dist, pages, inputs).unwrap();

        let entries: Vec<String> = fs::read_dir(dist.join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        // Only the css asset, no islands.
        assert_eq!(entries.len(), 1);
        assert!(entries[0].starts_with("styles-"));
        // Outcome reports exactly one hashed asset URL.
        assert_eq!(outcome.hashed_asset_urls.len(), 1);
    }

    #[test]
    fn both_css_and_islands_rewrite_independently() {
        let dir = tempdir().unwrap();
        let dist = dir.path();
        let html = "<!doctype html><html><head>\
            <link rel=\"stylesheet\" href=\"/assets/styles.css\">\
            <script type=\"module\" src=\"/assets/islands.js\"></script>\
            </head><body/></html>";
        let pages = vec![stage_html(dist, "page/index.html", html)];

        let inputs = ProdAssetEmitterInputs {
            css: Some(AssetEmitterPayload {
                bytes: b".x{color:red}".to_vec(),
                relative_path: PathBuf::from("assets/styles.css"),
                stable_url: "/assets/styles.css".to_string(),
                companions: Vec::new(),
            }),
            islands: Some(AssetEmitterPayload {
                bytes: b"// hydrate".to_vec(),
                relative_path: PathBuf::from("assets/islands.js"),
                stable_url: "/assets/islands.js".to_string(),
                companions: Vec::new(),
            }),
            ..Default::default()
        };

        let outcome = apply_prod_asset_pipeline(dist, pages, inputs).unwrap();
        assert_eq!(outcome.hashed_asset_urls.len(), 2);

        let final_html = fs::read_to_string(dist.join("page/index.html")).unwrap();
        assert!(!final_html.contains("/assets/styles.css\""));
        assert!(!final_html.contains("/assets/islands.js\""));
        for (_, url) in &outcome.hashed_asset_urls {
            assert!(
                final_html.contains(url),
                "hashed url {url} missing from HTML: {final_html}"
            );
        }

        let entries: std::collections::BTreeSet<String> = fs::read_dir(dist.join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|n| n.starts_with("styles-")));
        assert!(entries.iter().any(|n| n.starts_with("islands-")));
    }

    #[test]
    fn missing_html_file_surfaces_clean_error() {
        let dir = tempdir().unwrap();
        let dist = dir.path();
        // Reference a file we never wrote.
        let rel = RelDistPath::new("ghost.html").unwrap();
        let pages = vec![ProdRenderedFile {
            page: synthesize_page_id_from_output(&rel),
            output_path: rel,
        }];
        let inputs = ProdAssetEmitterInputs {
            css: Some(AssetEmitterPayload {
                bytes: b"x".to_vec(),
                relative_path: PathBuf::from("assets/styles.css"),
                stable_url: "/assets/styles.css".to_string(),
                companions: Vec::new(),
            }),
            islands: None,
            ..Default::default()
        };
        let err = apply_prod_asset_pipeline(dist, pages, inputs).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to read rendered page from disk"),
            "{msg}"
        );
    }

    #[test]
    fn empty_pages_still_emits_assets() {
        // Even with no pages on disk, the pipeline should still ship
        // the hashed assets — the rewrite step just becomes a no-op.
        let dir = tempdir().unwrap();
        let dist = dir.path();
        let inputs = ProdAssetEmitterInputs {
            css: Some(AssetEmitterPayload {
                bytes: b".y{}".to_vec(),
                relative_path: PathBuf::from("assets/styles.css"),
                stable_url: "/assets/styles.css".to_string(),
                companions: Vec::new(),
            }),
            islands: None,
            ..Default::default()
        };
        let outcome = apply_prod_asset_pipeline(dist, Vec::new(), inputs).unwrap();
        assert_eq!(outcome.hashed_asset_urls.len(), 1);
        let entries: Vec<String> = fs::read_dir(dist.join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].starts_with("styles-"));
    }
}
