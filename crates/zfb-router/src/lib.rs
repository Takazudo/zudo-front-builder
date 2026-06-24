//! zfb-router: file-system based route discovery for the zfb framework.
//!
//! Walks a `pages/` directory and produces a sorted list of [`Route`]s
//! following Next.js / Astro conventions:
//!
//! - `pages/about.tsx` → `/about`
//! - `pages/blog/[slug].tsx` → `/blog/:slug`
//! - `pages/docs/[...slug].tsx` → `/docs/:slug{.+}`
//! - `pages/[lang]/[slug].tsx` → `/:lang/:slug`
//! - `pages/index.tsx` → `/`
//! - `pages/blog/index.tsx` → `/blog`
//!
//! Files starting with `_` (e.g. `_app.tsx`, `_document.tsx`) and any non-`.tsx`
//! files are ignored. Two source files that resolve to the same route template
//! produce a [`RouterError::AmbiguousRoute`].

pub mod error;
pub mod route;
pub mod scan;

pub use error::RouterError;
pub use route::{Route, RouteKind, Segment};
pub use scan::{route_shape_key_for_pages_rel, scan_pages, shape_key};

use std::path::{Path, PathBuf};

/// Convenience wrapper that owns both the input directory and the resolved
/// route table. Most call sites can just use [`scan_pages`] directly; this
/// type exists so downstream crates can keep both pieces together.
#[derive(Debug, Clone)]
pub struct Router {
    pages_dir: PathBuf,
    routes: Vec<Route>,
}

impl Router {
    /// Scan `pages_dir` and build the router. Equivalent to
    /// [`scan_pages`] plus a small struct wrapper.
    pub fn scan(pages_dir: &Path) -> std::result::Result<Self, RouterError> {
        let routes = scan_pages(pages_dir)?;
        Ok(Self {
            pages_dir: pages_dir.to_path_buf(),
            routes,
        })
    }

    /// The pages directory this router was built from.
    pub fn pages_dir(&self) -> &Path {
        &self.pages_dir
    }

    /// Sorted route table — most specific first.
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }
}

/// Convenience `Result` alias that uses [`RouterError`].
pub type Result<T> = std::result::Result<T, RouterError>;
