//! Bytes-only emitter adapter for `ProductionAssetPipeline`.
//!
//! `ProductionAssetPipeline` (in `zfb-build`) is the single owner of
//! asset hashing and HTML rewrite for the prod build. To plug into it,
//! every asset-producing crate exposes an emitter that returns raw
//! bytes plus a *stable* URL — the pipeline computes the SHA-256
//! truncated to 8 hex chars, rewrites HTML, and atomically writes the
//! hashed file.
//!
//! This module is the CSS half of that contract:
//!
//! - [`CssEmitterOutput`] carries the bytes + the stable URL declared
//!   in `zfb_types::STABLE_CSS_URL`. The matching on-disk filename
//!   (`styles.css`) lives in `zfb_types::STABLE_CSS_FILENAME`.
//! - [`CssProductionEmitter`] wraps a closure that produces the bytes.
//!   `zfb-css` deliberately does not depend on `zfb-build`, so this
//!   crate cannot directly impl `zfb_build::pipeline::prod::AssetEmitter`.
//!   The orchestrator (S4) bridges by adapting [`CssProductionEmitter`]
//!   into an `EmittedAsset` (the trait is `Fn() -> Result<Option<EmittedAsset>>`,
//!   already implemented for any `Fn`).
//!
//! ## Why bytes-only and not the existing `CssPipeline::build()`?
//!
//! `CssPipeline::build()` writes the hashed file to disk *itself* — a
//! contract older callers (dev paths, ad-hoc consumers) still depend
//! on. Reusing it from the prod pipeline would emit the asset twice:
//! once at the wrong (emitter-computed) hash and once at the right
//! (pipeline-computed) hash. The bytes-only [`CssPipeline::build_emitter`]
//! path skips the asset write and returns the bytes for the prod
//! pipeline to ship.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::url_attribution::PackageUrlAsset;

/// Raw bytes + stable URL produced by [`crate::CssPipeline::build_emitter`].
///
/// The `bytes` are the same combined CSS string `CssPipeline::build()`
/// would write — the optional framework block (issue #1533,
/// [`crate::pipeline::CssPipelineConfig::framework_css`]), then Tailwind
/// utilities, then CSS Modules output, each joined by a single `\n`.
/// `stable_url` always equals
/// `zfb_types::STABLE_CSS_URL` (`/assets/styles.css`); the prod
/// pipeline matches this string against rendered HTML and rewrites it
/// to `/assets/styles-<hash>.css`.
#[derive(Debug, Clone)]
pub struct CssEmitterOutput {
    /// Combined CSS asset bytes. Empty bytes are technically valid (a
    /// project with no Tailwind utilities and no CSS Modules emits an
    /// empty `\n`-joined string), but in practice the engine output
    /// always carries at least Tailwind's preflight reset.
    pub bytes: Vec<u8>,

    /// The unhashed public URL the renderer embeds. Always the
    /// `STABLE_CSS_URL` constant — exposed here so the prod pipeline
    /// can match it against HTML without re-importing `zfb-types`
    /// transitively at the call site.
    pub stable_url: String,

    /// Companion assets emitted for package-attributed `url()` references
    /// the engine resolved and rewrote `bytes` to reference (issue #2316,
    /// decision c). Empty for a project with no such references — the
    /// common case. The caller (`run_css_emitter` in
    /// `crates/zfb/src/commands/build.rs`) threads these into the css
    /// `EmittedAsset.companions` slot, which the production pipeline
    /// writes verbatim beside the hashed CSS entry.
    pub companions: Vec<PackageUrlAsset>,
}

/// On-disk relative path under `dist_root` for the (pre-hash) CSS
/// asset. The prod pipeline splices the hash into the filename
/// (`styles.css` → `styles-<hash>.css`) before atomic-writing under
/// this directory. Constructed from `zfb_types` constants so a future
/// rename of the assets dir or stable filename only changes one place.
pub fn css_relative_path() -> PathBuf {
    PathBuf::from(zfb_types::DIST_ASSETS_DIR).join(zfb_types::STABLE_CSS_FILENAME)
}

/// Closure-backed adapter that produces a [`CssEmitterOutput`] on
/// demand.
///
/// Wraps any `Fn() -> Result<CssEmitterOutput>` so the orchestrator
/// (S4) can hand `ProductionAssetPipeline` a callable without
/// `zfb-css` taking on a `zfb-build` dependency. The orchestrator is
/// expected to wrap this in an `Fn() -> Result<Option<EmittedAsset>>`
/// that fills in `relative_path` from [`css_relative_path`] and
/// `stable_url` from the inner output.
///
/// `Arc<dyn Fn(...)>` rather than `Box<dyn Fn(...)>` so the
/// orchestrator can clone the emitter into multiple build contexts
/// (e.g. tests that re-run the pipeline) without rebuilding the
/// underlying [`crate::CssPipeline`].
#[derive(Clone)]
pub struct CssProductionEmitter {
    inner: Arc<dyn Fn() -> Result<CssEmitterOutput> + Send + Sync>,
}

impl CssProductionEmitter {
    /// Construct from a closure that produces the emitter output. The
    /// closure typically captures a `CssPipeline` by `Arc` and calls
    /// `pipeline.build_emitter()`.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn() -> Result<CssEmitterOutput> + Send + Sync + 'static,
    {
        Self { inner: Arc::new(f) }
    }

    /// Invoke the wrapped closure to produce the next emitter output.
    pub fn emit(&self) -> Result<CssEmitterOutput> {
        (self.inner)()
    }
}

impl std::fmt::Debug for CssProductionEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CssProductionEmitter")
            .field("inner", &"<closure>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_relative_path_matches_stable_url_suffix() {
        let p = css_relative_path();
        // The relative path's filename must equal the suffix of the
        // stable URL. Drift here would break HTML rewrite — the
        // pipeline matches on URL while the emitter writes by path.
        assert_eq!(
            p.file_name().and_then(|s| s.to_str()),
            Some(zfb_types::STABLE_CSS_FILENAME)
        );
        assert!(zfb_types::STABLE_CSS_URL.ends_with(zfb_types::STABLE_CSS_FILENAME));
    }

    #[test]
    fn production_emitter_invokes_inner_closure() {
        let emitter = CssProductionEmitter::new(|| {
            Ok(CssEmitterOutput {
                bytes: b".x{}".to_vec(),
                stable_url: zfb_types::STABLE_CSS_URL.to_string(),
                companions: Vec::new(),
            })
        });
        let out = emitter.emit().unwrap();
        assert_eq!(out.bytes, b".x{}");
        assert_eq!(out.stable_url, zfb_types::STABLE_CSS_URL);
    }
}
