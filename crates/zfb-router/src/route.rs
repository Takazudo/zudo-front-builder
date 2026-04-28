//! Core route data model.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// Re-export the canonical Segment type from zfb-types so external callers
// import from zfb_router::Segment as before without a breaking path change.
pub use zfb_types::Segment;

/// The kind of a route, used for sorting (static beats dynamic beats catchall).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RouteKind {
    Static,
    Dynamic,
    Catchall,
}

impl RouteKind {
    /// Lower numbers sort first (i.e. higher priority).
    pub(crate) fn order_key(self) -> u8 {
        match self {
            RouteKind::Static => 0,
            RouteKind::Dynamic => 1,
            RouteKind::Catchall => 2,
        }
    }
}

/// A single resolved route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    /// The source `.tsx` file this route was derived from.
    pub source_path: PathBuf,
    /// Parsed segments (excluding the leading `/`). An empty vec corresponds to
    /// the index route `/`.
    pub segments: Vec<Segment>,
    /// Coarse kind used for sorting and downstream dispatch.
    pub kind: RouteKind,
    /// Specificity score: higher is more specific. See [`crate::scan`] for the
    /// exact formula. Stable across versions only in relative terms.
    pub specificity: u32,
    /// Output extension derived from the filename convention. The rule:
    /// after stripping the source `.tsx` (or `.ts`) extension, the last
    /// `.`-separated segment of the resulting stem is the output
    /// extension (e.g. `sitemap.xml.tsx` → `Some("xml")`,
    /// `api.v2.json.tsx` → `Some("json")`, `about.tsx` → `None`).
    /// `None` means "no extension hint — default to `html`".
    ///
    /// **Precedence** (also documented in ADR-003 and in
    /// [`crate::route::Route::output_filename`]): frontmatter `extension`
    /// override > filename convention (this field) > the `html` default.
    pub output_extension: Option<String>,
}

/// Default output extension when neither frontmatter nor the filename
/// convention specifies one.
pub const DEFAULT_OUTPUT_EXTENSION: &str = "html";

impl Route {
    /// Render the route as a `/`-separated template, e.g. `/blog/:slug` or
    /// `/docs/:slug*`. The empty (index) route renders as `/`.
    pub fn template(&self) -> String {
        if self.segments.is_empty() {
            return "/".to_string();
        }
        let mut out = String::new();
        for seg in &self.segments {
            out.push('/');
            out.push_str(&seg.template());
        }
        out
    }

    /// Compute the relative output filename under `dist/` for this route,
    /// honouring the precedence:
    ///
    /// 1. `frontmatter_extension_override` if `Some`,
    /// 2. else the filename-convention extension on this route,
    /// 3. else [`DEFAULT_OUTPUT_EXTENSION`] (`"html"`).
    ///
    /// HTML routes render to a directory-style `…/index.html` layout
    /// (e.g. `/about` → `about/index.html`, `/` → `index.html`) so that
    /// static hosts serve them at clean URLs. Non-HTML routes render to
    /// the bare URL path (e.g. `/sitemap.xml` → `sitemap.xml`).
    ///
    /// Routes whose template still contains dynamic parameters
    /// (`Dynamic` / `Catchall`) cannot produce a concrete filename
    /// without a parameter binding; for those the caller must build
    /// the path itself. This helper falls back to the template literal
    /// in that case so it never panics, but the result will contain
    /// `:name` placeholders and is intended only for diagnostics.
    pub fn output_filename(&self, frontmatter_extension_override: Option<&str>) -> PathBuf {
        let ext = frontmatter_extension_override
            .or(self.output_extension.as_deref())
            .unwrap_or(DEFAULT_OUTPUT_EXTENSION);

        // Build the URL-style path from segments. Dynamic / catchall
        // segments fall back to their template form (`:name`,
        // `:name*`); concrete builds wouldn't reach here.
        let mut url_parts: Vec<String> = Vec::with_capacity(self.segments.len());
        for seg in &self.segments {
            url_parts.push(match seg {
                Segment::Static(s) => s.clone(),
                Segment::Dynamic(_) | Segment::Catchall(_) => seg.template(),
            });
        }

        // Was the source file an `index.<...>.tsx`? In that case the
        // parser dropped the `index` segment from the URL, but the
        // output file should still live at `<dir>/index.<ext>` (the
        // directory-index layout) for both HTML and non-HTML routes.
        let source_is_index = source_filename_is_index(&self.source_path);

        if ext == DEFAULT_OUTPUT_EXTENSION {
            // HTML pages: `/about` → `about/index.html`, `/` → `index.html`.
            let mut p = PathBuf::new();
            for part in &url_parts {
                p.push(part);
            }
            p.push("index.html");
            p
        } else {
            // Non-HTML pages: the URL path IS the file. The last
            // segment already carries the extension because the
            // filename rule preserves it as part of the URL segment
            // (e.g. `sitemap.xml.tsx` → segment `sitemap.xml`).
            //
            // Exception: when the source was an `index.<ext>.tsx`,
            // the parser stripped the `index` token from the URL, so
            // we re-attach `index.<ext>` here as a directory index
            // so the file actually carries its extension.
            //
            // If the override flipped the extension (e.g.
            // frontmatter says `rss` but the filename was
            // `sitemap.xml.tsx`), we splice the new extension in.
            let mut parts = url_parts;
            if source_is_index {
                // `pages/blog/index.xml.tsx` → `blog/index.xml`,
                // top-level `index.xml.tsx` → `index.xml`.
                let mut p = PathBuf::new();
                for part in &parts {
                    p.push(part);
                }
                p.push(format!("index.{ext}"));
                p
            } else if let Some(last) = parts.last_mut() {
                let conv = self.output_extension.as_deref();
                if conv != Some(ext) {
                    // Override — replace the trailing `.<conv>` with
                    // `.<ext>` if present, else append `.<ext>`.
                    let stem = match conv {
                        Some(conv_ext) => last
                            .strip_suffix(&format!(".{conv_ext}"))
                            .unwrap_or(last)
                            .to_string(),
                        None => last.clone(),
                    };
                    *last = format!("{stem}.{ext}");
                }
                let mut p = PathBuf::new();
                for part in &parts {
                    p.push(part);
                }
                p
            } else {
                // Index page emitting non-HTML — e.g. `/` rendering
                // an `rss` override. Emit `index.<ext>`.
                PathBuf::from(format!("index.{ext}"))
            }
        }
    }
}

/// Return `true` when the source filename (after stripping `.tsx`/`.ts`)
/// begins with `index` followed by either nothing or a dot. This is the
/// signal that the parser dropped an `index` URL segment for this route.
fn source_filename_is_index(source: &std::path::Path) -> bool {
    let Some(name) = source.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let stem = name
        .strip_suffix(".tsx")
        .or_else(|| name.strip_suffix(".ts"))
        .unwrap_or(name);
    match stem.split_once('.') {
        Some((head, _)) => head == "index",
        None => stem == "index",
    }
}
