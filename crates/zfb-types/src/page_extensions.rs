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

/// Infixes that mark a colocated test file: `foo.test.ts`, `foo.spec.tsx`.
const TEST_SIDECAR_INFIXES: &[&str] = &[".test.", ".spec."];

/// The TypeScript ambient-declaration suffix, e.g. `env.d.ts`.
const DECLARATION_SUFFIX: &str = ".d.ts";

/// Returns `true` when `path` is a conventional **sidecar** that happens to
/// carry a routable page extension but is never itself a page.
///
/// Widening [`ROUTABLE_PAGE_EXTENSIONS`] to `.ts`/`.js`/`.jsx` (epic #1990)
/// newly made two universally-understood sidecar shapes look like page
/// sources to `scan_pages`:
///
/// - **TypeScript declaration files** (`env.d.ts`) — would route as `/env.d`
///   with `output_extension = Some("d")`, because the widened script-page
///   sidecar convention reads the last dot-segment of the stem as an output
///   extension.
/// - **Colocated tests** (`index.test.ts` beside `index.tsx`) — both strip to
///   different stems, but `index.test.ts` still becomes a `/index.test` route,
///   and a `foo.test.tsx` beside `foo.tsx` collides outright, turning a
///   previously-green build into a `RouterError::AmbiguousRoute` failure.
///
/// Neither shape is a page under any convention in any ecosystem, so skipping
/// them cannot surprise an author — unlike an opinion about bare helper
/// modules (`pages/helpers.ts`), which stays deliberately unhandled.
///
/// The check is purely filename-based — no filesystem access — mirroring
/// [`crate::is_client_script_file`], the sibling filename contract this module
/// is modelled on. Only [`SCRIPT_PAGE_EXTENSIONS`] participate: `about.test.md`
/// is a plausible content page name, not a test.
pub fn is_page_sidecar_file(path: &std::path::Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    // `env.d.ts` — but not a bare `.d.ts` with no stem.
    if file_name.ends_with(DECLARATION_SUFFIX) && file_name.len() > DECLARATION_SUFFIX.len() {
        return true;
    }

    for ext in SCRIPT_PAGE_EXTENSIONS {
        for infix in TEST_SIDECAR_INFIXES {
            let suffix = format!("{infix}{ext}");
            if file_name.ends_with(&suffix) && file_name.len() > suffix.len() {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

    #[test]
    fn sidecar_matches_declaration_files() {
        for name in ["env.d.ts", "pages/env.d.ts", "globals.d.ts"] {
            assert!(is_page_sidecar_file(Path::new(name)), "expected: {name}");
        }
    }

    #[test]
    fn sidecar_matches_colocated_tests_across_script_extensions() {
        for ext in SCRIPT_PAGE_EXTENSIONS {
            for infix in ["test", "spec"] {
                let name = format!("pages/index.{infix}.{ext}");
                assert!(is_page_sidecar_file(Path::new(&name)), "expected: {name}");
            }
        }
    }

    #[test]
    fn sidecar_rejects_genuine_pages() {
        let pages = [
            "pages/index.tsx",
            "pages/plain.ts",
            "pages/about.js",
            "pages/contact.jsx",
            "pages/sitemap.xml.ts",
            "pages/about.md",
            // Plausible CONTENT page names — the sidecar conventions are
            // script-only, so these must still route.
            "pages/test.md",
            "pages/api.spec.md",
            "pages/d.ts",
        ];
        for p in pages {
            assert!(!is_page_sidecar_file(Path::new(p)), "should not match: {p}");
        }
    }

    #[test]
    fn sidecar_rejects_stemless_names() {
        assert!(!is_page_sidecar_file(Path::new(".d.ts")));
        assert!(!is_page_sidecar_file(Path::new("pages/.test.ts")));
    }
}
