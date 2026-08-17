//! Region identity, raw-source digest, and headings — the metadata
//! channel the render-artifact writer joins against (issue #2423, epic
//! #2421).
//!
//! # What the writer needs
//!
//! For every compiled markdown module the build's artifact writer must be
//! able to resolve `{ headings, source_digest }` from a **region id** —
//! the module specifier, `mdx://<collection>/<slug>` (optionally carrying
//! the `#<hash8>` fragment). Two properties make that non-trivial:
//!
//! 1. **Cache hits.** The MDX compile cache
//!    ([`crate::mdx_jsx_emit::MdxModuleCache`]) short-circuits the whole
//!    emitter, so nothing derived from the mdast walk is recomputed. The
//!    headings therefore ride on [`CompiledMdx::headings`], which the
//!    cache stores and replays — a hit carries exactly what a fresh
//!    compile would have produced.
//! 2. **The digest is not a compile-seam quantity.** The compiler only
//!    ever receives the frontmatter-STRIPPED body, while the contract
//!    digests the raw on-disk bytes (frontmatter included). It is
//!    therefore computed wherever the file is read — the collection
//!    walker (`crate::collection::parse_entry`) for collection entries,
//!    and [`render_region_metadata`] for direct `pages/*.md` /
//!    `pages/*.mdx` sources, which are never collection members.
//!
//! # Digest semantics (pinned by the epic)
//!
//! `source_digest` is `"sha256:" + 64 lowercase hex` over the entry's raw
//! source bytes **exactly as read** — frontmatter included, no BOM strip,
//! no CRLF normalisation. It identifies the SOURCE, not the rendered
//! output: a transcluded dependency can change the rendered fragment
//! without changing this digest, so it is not an ETag for the artifact.
//!
//! Region identity is the module specifier, never the digest — two
//! byte-identical files legitimately share a digest.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::frontmatter::{self, strip_yaml_frontmatter};
use crate::mdx_jsx_emit::{
    compile_mdx_to_jsx_module_cached, CompiledMdx, HeadingEntry, MdxModuleCache,
};
use crate::pipeline::{Pipeline, PipelineError};

/// Algorithm prefix on every [`RenderRegionMetadata::source_digest`].
/// Serialized form is `{SOURCE_DIGEST_PREFIX}{64 lowercase hex}`.
pub const SOURCE_DIGEST_PREFIX: &str = "sha256:";

/// Digest raw source bytes into the pinned `"sha256:<64-hex>"` form.
///
/// Takes bytes rather than `&str` so the contract ("the raw on-disk bytes
/// exactly as read") is expressible at the type level: callers hand over
/// what they read, and no normalisation can sneak in on the way.
#[must_use]
pub fn source_digest(raw: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw);
    format!("{SOURCE_DIGEST_PREFIX}{}", hex::encode(hasher.finalize()))
}

/// The per-region payload the artifact writer joins against.
///
/// Field order is the render-artifact contract's order (`headings` then
/// `sourceDigest`); serde emits fields in declaration order, so a writer
/// can serialize this struct directly. Field NAMES are snake_case to
/// match the surrounding snapshot serialization
/// ([`crate::content_bridge::EntrySnapshot`]); the artifact file's
/// camelCase spelling is the writer's mapping to apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderRegionMetadata {
    /// Compiler-allocated headings in document order, lockstep with the
    /// module's `export const headings` and with the rendered `<hN id>`
    /// attributes (same `SlugAllocator`). Author component overrides
    /// (a custom `h2`, say) can still make the rendered DOM diverge from
    /// this list — the caveat the epic documents.
    pub headings: Vec<HeadingEntry>,
    /// `"sha256:<64-hex>"` over the raw on-disk source bytes. See the
    /// module docs for why this is not an ETag for rendered output.
    pub source_digest: String,
}

impl RenderRegionMetadata {
    /// Pair a compile's headings with a digest of the raw source that
    /// produced it.
    #[must_use]
    pub fn new(headings: Vec<HeadingEntry>, source_digest: String) -> Self {
        Self {
            headings,
            source_digest,
        }
    }
}

/// Strip the `#<hash8>` fragment from a module specifier, yielding the
/// hash-less region id `mdx://<collection>/<slug>`.
///
/// Both spellings are live in-tree: the Rust snapshot's
/// `module_specifier` carries the hash, while `buildModuleSpecifier` in
/// `packages/zfb/src/content.ts` mints the hash-less form. The content
/// bridge registers both keys for exactly this reason
/// (`crates/zfb-build/src/bundler.rs`), so resolvers here accept both too.
#[must_use]
pub fn region_id_without_hash(specifier: &str) -> &str {
    match specifier.split_once('#') {
        Some((head, _)) => head,
        None => specifier,
    }
}

/// True when `region_id` addresses `specifier` — either byte-identical or
/// as its hash-less form.
///
/// Deliberately NOT symmetric on partial matches: a hash-BEARING
/// `region_id` must match the stored hash exactly, so a stale region id
/// from an earlier build can never resolve against current metadata.
#[must_use]
pub fn region_id_addresses(specifier: &str, region_id: &str) -> bool {
    specifier == region_id
        || (!region_id.contains('#') && region_id_without_hash(specifier) == region_id)
}

/// Read a markdown source and return `(module_specifier, metadata)` for
/// it — the direct-page counterpart to the collection walker's
/// per-entry metadata.
///
/// Direct `pages/*.md` / `pages/*.mdx` routes are markdown-backed but are
/// not collection members, so they never appear in a
/// [`crate::content_bridge::ContentSnapshot`]. This is how the writer
/// covers them.
///
/// # Cost
///
/// The compile is served from `cache` in a real build: the bundler has
/// already compiled the same body through an identically-configured
/// pipeline, and `MdxModuleCache::process_global()` is shared across both.
/// The extra work is therefore one file read plus one map lookup.
///
/// **The caller must supply the same `pipeline` shape the bundler used**
/// (build it from the same [`crate::PipelineSpec`]) — the compile-cache
/// key covers the pipeline's config fingerprint, so a different shape
/// costs a full recompile rather than a hit. Body stripping is dispatched
/// on the extension to mirror the bundler's two page passes: `.mdx` goes
/// through [`strip_yaml_frontmatter`], `.md` through
/// [`frontmatter::extract`] with the same lenient fallback the bundler
/// applies when the YAML is malformed.
///
/// # Errors
///
/// Returns [`PipelineError::Parse`] when the emitter rejects the body.
/// Read failures are the caller's to handle — pass the bytes you read.
pub fn render_region_metadata(
    path: &Path,
    raw: &str,
    pipeline: Option<&mut Pipeline>,
    cache: Option<&MdxModuleCache>,
) -> Result<(String, RenderRegionMetadata), PipelineError> {
    let body = markdown_page_body(path, raw);
    let compiled = compile_mdx_to_jsx_module_cached(&body, path, cache, pipeline)?;
    Ok(from_compiled(&compiled, raw.as_bytes()))
}

/// Pair an already-performed compile with the raw bytes that produced it.
///
/// Split out from [`render_region_metadata`] so call sites that already
/// hold both halves (the collection walker, a bundler pass) join them
/// through one definition instead of re-deriving the shape.
#[must_use]
pub fn from_compiled(compiled: &CompiledMdx, raw: &[u8]) -> (String, RenderRegionMetadata) {
    (
        compiled.specifier.clone(),
        RenderRegionMetadata::new(compiled.headings.clone(), source_digest(raw)),
    )
}

/// Frontmatter-strip a direct page source the way the bundler's page
/// passes do, dispatching on extension.
///
/// The two dialects genuinely differ (see [`strip_yaml_frontmatter`]), and
/// the difference lands in the compile-cache key — mirroring the bundler
/// per-extension is what keeps this a cache HIT.
fn markdown_page_body(path: &Path, raw: &str) -> String {
    let is_mdx = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mdx"));
    if is_mdx {
        return strip_yaml_frontmatter(raw).to_string();
    }
    match frontmatter::extract(path, raw) {
        Ok(uf) => uf.body.unwrap_or_default(),
        // Malformed YAML: the bundler warns and feeds the lenient strip
        // rather than failing the page, so mirror that here too.
        Err(_) => strip_yaml_frontmatter(raw).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::Pipeline;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

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
                "zfb-content-render-meta-{label}-{nanos}-{n}-{pid}",
                pid = std::process::id()
            ));
            fs::create_dir_all(&dir).expect("create tmp dir");
            Self { path: dir }
        }
        fn write(&self, rel: &str, contents: &[u8]) -> PathBuf {
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

    /// The digest contract in one assertion: whatever bytes the caller
    /// read are the bytes hashed. A fixture carrying frontmatter, a BOM,
    /// and CRLF terminators — the three things a "helpful" normalisation
    /// would touch — must hash to the same value an external `sha256sum`
    /// of the file would produce.
    #[test]
    fn source_digest_is_byte_exact_over_raw_bytes_including_bom_and_crlf() {
        let raw: &[u8] = b"\xEF\xBB\xBF---\r\ntitle: \"Digest\"\r\n---\r\n\r\n## Alpha\r\n";
        let got = source_digest(raw);

        // Independent reference: hash the identical bytes directly.
        let mut h = Sha256::new();
        h.update(raw);
        let want = format!("sha256:{}", hex::encode(h.finalize()));
        assert_eq!(got, want);

        assert!(got.starts_with(SOURCE_DIGEST_PREFIX), "prefix: {got}");
        assert_eq!(got.len(), SOURCE_DIGEST_PREFIX.len() + 64, "width: {got}");
        assert!(
            got[SOURCE_DIGEST_PREFIX.len()..]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "lowercase hex: {got}"
        );

        // The frontmatter-stripped body would hash differently — proof
        // the digest is over the WHOLE source, not the compiled body.
        let body_only = source_digest(b"\r\n## Alpha\r\n");
        assert_ne!(got, body_only);

        // A CRLF→LF normalisation would also change the answer, so the
        // equality above cannot be passing by coincidence.
        let lf_variant = source_digest(b"\xEF\xBB\xBF---\ntitle: \"Digest\"\n---\n\n## Alpha\n");
        assert_ne!(got, lf_variant);
    }

    #[test]
    fn direct_page_metadata_carries_headings_and_raw_digest() {
        let tmp = TmpDir::new("direct-page");
        let raw = "---\ntitle: \"About\"\n---\n\n## Alpha\n\n### Beta\n";
        let path = tmp.write("pages/about.md", raw.as_bytes());

        let mut pipeline = Pipeline::with_defaults();
        let (specifier, meta) = render_region_metadata(&path, raw, Some(&mut pipeline), None)
            .expect("direct page metadata");

        assert_eq!(
            specifier.split('#').next(),
            Some("mdx://pages/about"),
            "region id is the module specifier: {specifier}"
        );
        assert_eq!(meta.source_digest, source_digest(raw.as_bytes()));
        let slugs: Vec<&str> = meta.headings.iter().map(|h| h.slug.as_str()).collect();
        assert_eq!(slugs, vec!["alpha", "beta"]);
        assert_eq!(meta.headings[0].depth, 2);
        assert_eq!(meta.headings[1].depth, 3);
    }

    /// The `.md` / `.mdx` strip dialects differ on a body that starts
    /// with a blank line, and the bundler uses a different one per
    /// extension. Pin that this helper dispatches the same way — a
    /// regression here is a silent compile-cache MISS, not a test failure
    /// anywhere else.
    #[test]
    fn page_body_stripping_matches_the_bundler_dialect_per_extension() {
        let raw = "---\ntitle: \"X\"\n---\n\n## Alpha\n";
        assert_eq!(
            markdown_page_body(Path::new("pages/x.mdx"), raw),
            strip_yaml_frontmatter(raw),
            "`.mdx` pages use the lenient greedy strip",
        );
        assert_eq!(
            markdown_page_body(Path::new("pages/x.md"), raw),
            "\n## Alpha\n",
            "`.md` pages keep `extract`'s single-terminator body",
        );
        assert_ne!(
            markdown_page_body(Path::new("pages/x.mdx"), raw),
            markdown_page_body(Path::new("pages/x.md"), raw),
            "the two dialects must actually differ here, or this test proves nothing",
        );
    }

    #[test]
    fn malformed_frontmatter_page_falls_back_to_the_lenient_strip() {
        let raw = "---\ntitle: [unterminated\n---\n\n## Alpha\n";
        // `extract` rejects the YAML; the body still compiles.
        assert!(frontmatter::extract(Path::new("pages/bad.md"), raw).is_err());
        assert_eq!(
            markdown_page_body(Path::new("pages/bad.md"), raw),
            strip_yaml_frontmatter(raw),
        );
    }

    #[test]
    fn region_id_matching_accepts_both_specifier_spellings() {
        let spec = "mdx://docs/intro#0123abcd";
        assert!(region_id_addresses(spec, spec));
        assert!(region_id_addresses(spec, "mdx://docs/intro"));
        assert!(
            !region_id_addresses(spec, "mdx://docs/intro#ffffffff"),
            "a stale hash-bearing id must NOT resolve against current metadata",
        );
        assert!(!region_id_addresses(spec, "mdx://docs/other"));
        assert_eq!(region_id_without_hash(spec), "mdx://docs/intro");
        assert_eq!(
            region_id_without_hash("mdx://docs/intro"),
            "mdx://docs/intro"
        );
    }
}
