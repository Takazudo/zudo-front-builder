//! Canonical page-extension contract shared by `zfb-router` and `zfb-build`.
//!
//! ## Why this lives in `zfb-types`
//!
//! The router (`zfb-router`'s `scan_pages`) and the bundler
//! (`zfb-build`'s `derive_route` / `materialise_shadow`) each independently
//! decided which `pages/` file extensions are page sources, and the two
//! lists drifted apart (issue #1742 / epic #1990): the bundler treated
//! `.tsx`/`.ts`/`.jsx`/`.js`/`.mdx`/`.md`/`.html` as page-capable while the
//! router only accepted `.tsx`/`.mdx`/`.md`/`.html`, so `pages/index.ts` was
//! bundle-capable yet never routed. Publishing the contract once here — the
//! same pattern already used for [`crate::is_client_script_file`], which
//! `zfb-router` cites as "the single source of truth" — means both layers
//! read the same literal and cannot re-diverge silently.
//!
//! ## Two subsets, not one flat list
//!
//! The layers legitimately mean different things by "page source":
//!
//! - [`ROUTABLE_PAGE_EXTENSIONS`] — every extension `scan_pages` will accept
//!   as a route: script pages plus the non-script `.mdx`/`.md`/`.html`
//!   content pages.
//! - [`SCRIPT_PAGE_EXTENSIONS`] — the subset that requires script bundling
//!   (esbuild): `.tsx`/`.ts`/`.jsx`/`.js`. `.mdx`/`.md`/`.html` pages are
//!   compiled/copied through their own dedicated pipelines instead, so they
//!   are deliberately excluded from this narrower set.
//!
//! Each consumer takes the subset it actually means; forcing every consumer
//! onto one universal list would be worse than the duplication it replaces.

/// Extensions `scan_pages` accepts as routable page sources.
///
/// `mdx` is included for parity with existing zfb projects that author MDX
/// pages directly in `pages/`; `zfb-router` accepts both `.mdx` and `.md`
/// so `pages/about.mdx` continues to produce `/about` (zfb#404 regression
/// fix — an earlier diff briefly dropped `.mdx` while extending this
/// allowlist).
pub const ROUTABLE_PAGE_EXTENSIONS: &[&str] = &["tsx", "ts", "jsx", "js", "mdx", "md", "html"];

/// Subset of [`ROUTABLE_PAGE_EXTENSIONS`] whose page sources require script
/// bundling (esbuild) rather than the dedicated MDX/Markdown/static-HTML
/// pipelines.
pub const SCRIPT_PAGE_EXTENSIONS: &[&str] = &["tsx", "ts", "jsx", "js"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_page_extensions_is_a_subset_of_routable_page_extensions() {
        for ext in SCRIPT_PAGE_EXTENSIONS {
            assert!(
                ROUTABLE_PAGE_EXTENSIONS.contains(ext),
                "{ext} is in SCRIPT_PAGE_EXTENSIONS but not ROUTABLE_PAGE_EXTENSIONS"
            );
        }
    }

    #[test]
    fn routable_page_extensions_contains_the_seven_documented_extensions() {
        for ext in ["tsx", "ts", "jsx", "js", "mdx", "md", "html"] {
            assert!(
                ROUTABLE_PAGE_EXTENSIONS.contains(&ext),
                "missing expected extension: {ext}"
            );
        }
        assert_eq!(ROUTABLE_PAGE_EXTENSIONS.len(), 7);
    }

    #[test]
    fn script_page_extensions_excludes_non_script_content_extensions() {
        for ext in ["mdx", "md", "html"] {
            assert!(
                !SCRIPT_PAGE_EXTENSIONS.contains(&ext),
                "{ext} should not require script bundling"
            );
        }
        assert_eq!(SCRIPT_PAGE_EXTENSIONS.len(), 4);
    }
}
