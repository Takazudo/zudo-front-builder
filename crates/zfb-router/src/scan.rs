//! Scan a `pages/` directory and build the route table.
//!
//! Conventions follow Next.js / Astro:
//!
//! | source                              | route              | notes                          |
//! |-------------------------------------|--------------------|--------------------------------|
//! | `pages/index.tsx`                   | `/`                |                                |
//! | `pages/about.tsx`                   | `/about`           |                                |
//! | `pages/about.md`                    | `/about`           | SSG-only; MDX pipeline         |
//! | `pages/about.html`                  | `/about`           | SSG-only; static-asset copy    |
//! | `pages/blog/index.tsx`              | `/blog`            |                                |
//! | `pages/blog/[slug].tsx`             | `/blog/:slug`      |                                |
//! | `pages/blog/page/[page].tsx`        | `/blog/page/:page` |                                |
//! | `pages/docs/[...slug].tsx`          | `/docs/:slug{.+}`  |                                |
//! | `pages/docs/[[...slug]].tsx`        | `/docs/:slug{.+}?` | optional catchall: also `/docs`|
//! | `pages/[lang]/[slug].tsx`           | `/:lang/:slug`     |                                |
//!
//! Files starting with `_` (e.g. `_app.tsx`, `_document.tsx`) are ignored.
//! Accepted page extensions: `.tsx`, `.md`, `.html`. Files with any other
//! extension are skipped with a `tracing::warn!` so authors notice accidental
//! mis-placements.
//!
//! ## `.md` page contract (v1)
//!
//! `.md` pages are compiled through the MDX pipeline and wrapped in a
//! minimal HTML shell. Recognised frontmatter keys: `title` (string,
//! used as `<title>`; falls back to the URL slug) and `lang` (string,
//! used as `<html lang="…">`; defaults to `"en"`). All other frontmatter
//! keys are silently ignored. There is no layout system for `.md` pages
//! in v1 — `layout:` frontmatter has no effect; wrap the content in a
//! `.tsx` page if a shared layout is needed. SSG-only: `.md` pages are
//! not supported in SSR mode.
//!
//! ## `.html` page contract (v1)
//!
//! `.html` pages are copied verbatim to `dist/` without any JS rendering
//! or post-processing. The file must be a complete HTML document (starting
//! with `<!doctype` or `<html>`); bare HTML fragments are out of scope for
//! v1. Because the file is treated as a static asset, base-path rewriting,
//! link normalisation, and sitemap inclusion do not apply. Use `.md` or
//! `.tsx` if any of those transformations are needed. SSG-only: `.html`
//! pages are not supported in SSR mode.

use std::collections::HashMap;
use std::path::{Component, Path};

use walkdir::WalkDir;

use crate::error::RouterError;
use crate::route::{Route, RouteKind, Segment};

/// Extensions accepted as page source files.
///
/// `mdx` is included for parity with existing zfb projects that author MDX
/// pages directly in `pages/`. The bundler routes `.mdx` and `.md` through
/// the same MDX-compile pipeline, but the router must accept both shapes so
/// `pages/about.mdx` continues to produce `/about` (zfb#404 regression fix
/// — earlier diff briefly dropped `.mdx` while extending the allowlist).
const ACCEPTED_PAGE_EXTENSIONS: &[&str] = &["tsx", "mdx", "md", "html"];

/// Maximum specificity points awarded per route segment. The exact value is an
/// implementation detail; only relative ordering matters.
const STATIC_WEIGHT: u32 = 100;
const DYNAMIC_WEIGHT: u32 = 10;
const CATCHALL_WEIGHT: u32 = 1;
/// Bonus added so that `index.tsx` outranks a sibling `[slug].tsx` when both
/// produce the same number of segments.
const INDEX_BONUS: u32 = 1;

/// Walk `pages_dir` and produce the sorted list of routes.
pub fn scan_pages(pages_dir: &Path) -> Result<Vec<Route>, RouterError> {
    if !pages_dir.is_dir() {
        return Err(RouterError::PagesDirMissing(pages_dir.to_path_buf()));
    }

    let mut routes: Vec<Route> = Vec::new();

    for entry in WalkDir::new(pages_dir).follow_links(false).sort_by_file_name() {
        let entry = entry.map_err(|e| RouterError::Io {
            path: e
                .path()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| pages_dir.to_path_buf()),
            source: e.into(),
        })?;

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        let ext = path.extension().and_then(|e| e.to_str());

        // Skip files whose stem starts with '_' (framework internals) before
        // the extension check so we don't warn about private helper files.
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        if stem.starts_with('_') {
            continue;
        }

        // Also skip routes that traverse a directory whose name starts with
        // '_' — these are conventionally private (e.g. `pages/_components/`).
        let rel = path.strip_prefix(pages_dir).map_err(|_| RouterError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other("entry path was outside pages_dir"),
        })?;

        if rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => s.to_str(),
                _ => None,
            })
            .any(|name| name.starts_with('_'))
        {
            continue;
        }

        // Accept .tsx, .mdx, .md, .html as page sources. Warn on any other
        // extension so authors notice accidental mis-placements in pages/.
        match ext {
            Some(e) if ACCEPTED_PAGE_EXTENSIONS.contains(&e) => {}
            _ => {
                tracing::warn!(
                    path = %rel.display(),
                    "pages/ file has an unrecognised extension and will be skipped; \
                     accepted extensions are: tsx, mdx, md, html"
                );
                continue;
            }
        }

        let route = parse_route(path, rel)?;

        // Dynamic / catchall .md and .html routes cannot work in v1:
        // there is no `paths()` story for either extension (the build-time
        // expansion would need a top-level export from the source, which a
        // pure .md or .html file cannot carry). Skip them with a loud
        // warning so authors notice rather than shipping a green build
        // that silently produces no pages.
        if matches!(ext, Some("md") | Some("html"))
            && !matches!(route.kind, RouteKind::Static)
        {
            tracing::warn!(
                path = %rel.display(),
                kind = ?route.kind,
                "dynamic .md / .html page routes are not supported in v1 (no paths() story); \
                 this file will be skipped — author it as .tsx if dynamic paths are needed"
            );
            continue;
        }

        routes.push(route);
    }

    detect_ambiguity(&routes)?;
    sort_routes(&mut routes);
    Ok(routes)
}

/// Parse a single source file into a [`Route`].
fn parse_route(source: &Path, rel: &Path) -> Result<Route, RouterError> {
    // Build raw segment list. Drop the file extension; treat `index` as the
    // empty segment.
    let mut raw_segments: Vec<String> = Vec::new();
    let components: Vec<&str> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();

    // Output extension hint from the filename convention. Filled in
    // when the last component matches `<stem>.<ext>.tsx` (or `.ts`).
    // `index.tsx` and `about.tsx` carry no hint; `sitemap.xml.tsx`
    // carries `Some("xml")`.
    let mut output_extension: Option<String> = None;

    let last_idx = components.len().saturating_sub(1);
    for (i, comp) in components.iter().enumerate() {
        if i == last_idx {
            // Strip the source extension (`.tsx` or `.ts`). Everything
            // that survives is part of the URL segment — including any
            // intermediate dots like `sitemap.xml`.
            let stem = strip_source_extension(comp);

            // Filename convention: the LAST `.`-separated segment of
            // the surviving stem (if any) is the output-file
            // extension. Multi-dot stems like `api.v2.json` keep the
            // earlier dots as part of the URL and yield only `json`.
            //
            // We exclude `index` (no extension) and stems with a
            // single token, which look like normal pages.
            //
            // Dynamic / catchall segments (`[name]`, `[...name]`) are
            // **not** subject to extension parsing. Their dots are
            // syntactic — `[...slug]` is the catchall sigil, not the
            // multi-dot stem `[..` + extension `slug]`. Without this
            // guard `[...slug].tsx` was mis-parsed as
            // `output_extension = Some("slug]")`, which switched the
            // route into the non-HTML write path and produced bare
            // files at every catchall URL (no `index.html`),
            // collapsing the directory layout downstream renderers
            // expect.
            //
            // `.md` and `.html` page sources always default to
            // `output_extension = None` (directory-index HTML layout).
            // Frontmatter extension overrides are not supported for
            // these source types (out of scope for v1).
            let is_tsx_or_ts = comp.ends_with(".tsx") || comp.ends_with(".ts");
            let stem_is_param = stem.starts_with('[');
            if is_tsx_or_ts && !stem_is_param {
                if let Some((before_dot, after_dot)) = stem.rsplit_once('.') {
                    if !before_dot.is_empty() && !after_dot.is_empty() {
                        output_extension = Some(after_dot.to_string());
                    }
                }
            }

            // Index special-case applies to the "stem-before-extension".
            // `index.tsx` → no segment. `index.xml.tsx` → URL is the
            // parent path, file is `index.xml` (handled by the
            // route's output_filename helper later).
            let index_marker = match stem.split_once('.') {
                Some((head, _)) => head == "index",
                None => stem == "index",
            };
            if index_marker {
                continue;
            }
            raw_segments.push(stem.to_string());
        } else {
            raw_segments.push((*comp).to_string());
        }
    }

    let mut segments: Vec<Segment> = Vec::with_capacity(raw_segments.len());
    let total = raw_segments.len();
    for (i, raw) in raw_segments.iter().enumerate() {
        let parsed = parse_segment(source, raw)?;
        if matches!(
            &parsed,
            Segment::Catchall(_) | Segment::OptionalCatchall(_)
        ) && i != total - 1
        {
            return Err(RouterError::CatchallNotLast {
                path: source.to_path_buf(),
                segment: raw.clone(),
            });
        }
        segments.push(parsed);
    }

    let kind = classify(&segments);
    let specificity = score(&segments, source);

    // `.html` source files bypass JS render entirely — the build pipeline
    // copies the body verbatim to dist/ without involving V8.
    let static_html = source
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e == "html")
        .unwrap_or(false);

    Ok(Route {
        source_path: source.to_path_buf(),
        segments,
        kind,
        specificity,
        output_extension,
        static_html,
    })
}

/// Strip the source-language extension from a filename component.
/// Recognised source extensions: `.tsx`, `.ts`, `.mdx`, `.md`, `.html`.
/// Returns the input unchanged when the component does not end with
/// one of those extensions.
fn strip_source_extension(component: &str) -> &str {
    if let Some(stem) = component.strip_suffix(".mdx") {
        return stem;
    }
    if let Some(stem) = component.strip_suffix(".tsx") {
        return stem;
    }
    if let Some(stem) = component.strip_suffix(".ts") {
        return stem;
    }
    if let Some(stem) = component.strip_suffix(".md") {
        return stem;
    }
    if let Some(stem) = component.strip_suffix(".html") {
        return stem;
    }
    component
}

/// Parse a single path component into a [`Segment`].
fn parse_segment(source: &Path, raw: &str) -> Result<Segment, RouterError> {
    if raw.is_empty() {
        return Err(RouterError::InvalidSegment {
            path: source.to_path_buf(),
            segment: raw.to_string(),
            message: "empty path component".into(),
        });
    }

    let starts = raw.starts_with('[');
    let ends = raw.ends_with(']');

    if starts != ends {
        return Err(RouterError::InvalidSegment {
            path: source.to_path_buf(),
            segment: raw.to_string(),
            message: "unbalanced brackets".into(),
        });
    }

    if !starts {
        return Ok(Segment::Static(raw.to_string()));
    }

    // Optional catchall: `[[...name]]` (Next.js-style). Matched before the
    // single-bracket forms so the doubled brackets are not mis-parsed as a
    // parameter literally named `[...name]`.
    if raw.starts_with("[[") && raw.ends_with("]]") {
        let inner = &raw[2..raw.len() - 2];
        let Some(name) = inner.strip_prefix("...") else {
            return Err(RouterError::InvalidSegment {
                path: source.to_path_buf(),
                segment: raw.to_string(),
                message: "double brackets are only valid as an optional catchall `[[...name]]`"
                    .into(),
            });
        };
        if name.is_empty() {
            return Err(RouterError::InvalidSegment {
                path: source.to_path_buf(),
                segment: raw.to_string(),
                message: "empty optional catchall parameter name".into(),
            });
        }
        validate_param_name(source, raw, name)?;
        return Ok(Segment::OptionalCatchall(name.to_string()));
    }

    // Strip outer brackets.
    let inner = &raw[1..raw.len() - 1];
    if inner.is_empty() {
        return Err(RouterError::InvalidSegment {
            path: source.to_path_buf(),
            segment: raw.to_string(),
            message: "empty parameter name".into(),
        });
    }

    if let Some(name) = inner.strip_prefix("...") {
        if name.is_empty() {
            return Err(RouterError::InvalidSegment {
                path: source.to_path_buf(),
                segment: raw.to_string(),
                message: "empty catchall parameter name".into(),
            });
        }
        validate_param_name(source, raw, name)?;
        return Ok(Segment::Catchall(name.to_string()));
    }

    validate_param_name(source, raw, inner)?;
    Ok(Segment::Dynamic(inner.to_string()))
}

fn validate_param_name(source: &Path, raw: &str, name: &str) -> Result<(), RouterError> {
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let starts_ok = name
        .chars()
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false);
    if !ok || !starts_ok {
        return Err(RouterError::InvalidSegment {
            path: source.to_path_buf(),
            segment: raw.to_string(),
            message: format!("invalid parameter name {name:?}"),
        });
    }
    Ok(())
}

fn classify(segments: &[Segment]) -> RouteKind {
    let mut has_dynamic = false;
    for seg in segments {
        match seg {
            // Optional catchalls share `RouteKind::Catchall`: the kind only
            // drives sorting/dispatch, and a required + optional catchall
            // can never coexist at the same prefix (scan-time conflict), so
            // no finer ordering between them is ever needed.
            Segment::Catchall(_) | Segment::OptionalCatchall(_) => return RouteKind::Catchall,
            Segment::Dynamic(_) => has_dynamic = true,
            Segment::Static(_) => {}
        }
    }
    if has_dynamic {
        RouteKind::Dynamic
    } else {
        RouteKind::Static
    }
}

fn score(segments: &[Segment], source: &Path) -> u32 {
    let mut total: u32 = 0;
    for seg in segments {
        total = total.saturating_add(match seg {
            Segment::Static(_) => STATIC_WEIGHT,
            Segment::Dynamic(_) => DYNAMIC_WEIGHT,
            Segment::Catchall(_) | Segment::OptionalCatchall(_) => CATCHALL_WEIGHT,
        });
    }
    if source
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s == "index")
        .unwrap_or(false)
    {
        total = total.saturating_add(INDEX_BONUS);
    }
    total
}

fn detect_ambiguity(routes: &[Route]) -> Result<(), RouterError> {
    let mut seen: HashMap<String, &Path> = HashMap::new();
    for route in routes {
        let template = route.template();
        if let Some(prev) = seen.insert(template.clone(), &route.source_path) {
            return Err(RouterError::AmbiguousRoute {
                template,
                first: prev.to_path_buf(),
                second: route.source_path.clone(),
            });
        }
    }
    detect_optional_catchall_conflicts(routes)
}

/// Render the template of a segment-slice prefix (`/docs` for
/// `[Static("docs")]`, `/` for the empty slice). Mirrors
/// [`Route::template`] but over an arbitrary prefix.
fn prefix_template(segments: &[Segment]) -> String {
    if segments.is_empty() {
        return "/".to_string();
    }
    let mut out = String::new();
    for seg in segments {
        out.push('/');
        out.push_str(&seg.template());
    }
    out
}

/// Detect conflicts specific to optional catchall routes. A full-set pass
/// (not folded into the incremental template map) so file-visit order can
/// never hide a conflict — `[` sorts before `i`, so `[[...slug]].tsx` is
/// walked before a sibling `index.tsx`.
///
/// For each `[[...name]]` route with prefix `P` (the URL it serves for
/// zero segments):
///
/// 1. Any route whose full template equals `P` conflicts — both produce
///    the bare URL (e.g. `pages/docs/index.tsx` / `pages/docs.tsx` vs
///    `pages/docs/[[...slug]].tsx`, all serving `/docs`).
/// 2. Any other catchall (required or optional) at the same prefix
///    conflicts regardless of param name — they overlap on every
///    non-empty path.
///
/// Deeper / more specific routes under the prefix (`docs/about.tsx`,
/// `docs/sub/[id].tsx`) are NOT conflicts — they coexist via the
/// specificity sort exactly as they do with a required `[...slug]`.
fn detect_optional_catchall_conflicts(routes: &[Route]) -> Result<(), RouterError> {
    for (i, route) in routes.iter().enumerate() {
        if !matches!(route.segments.last(), Some(Segment::OptionalCatchall(_))) {
            continue;
        }
        let prefix = prefix_template(&route.segments[..route.segments.len() - 1]);
        for (j, other) in routes.iter().enumerate() {
            if i == j {
                continue;
            }
            if other.template() == prefix {
                return Err(RouterError::OptionalCatchallConflict {
                    first: other.source_path.clone(),
                    second: route.source_path.clone(),
                    reason: format!(
                        "both serve the bare URL `{prefix}` (an optional catchall \
                         `[[...name]]` matches zero segments)"
                    ),
                });
            }
            if matches!(
                other.segments.last(),
                Some(Segment::Catchall(_) | Segment::OptionalCatchall(_))
            ) && prefix_template(&other.segments[..other.segments.len() - 1]) == prefix
            {
                return Err(RouterError::OptionalCatchallConflict {
                    first: other.source_path.clone(),
                    second: route.source_path.clone(),
                    reason: format!(
                        "two catchall routes at the same prefix `{prefix}` overlap on \
                         every non-empty path; keep exactly one of them"
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Sort routes by:
/// 1. Kind (static < dynamic < catchall) — most specific first.
/// 2. Number of segments, longer first.
/// 3. `index.tsx` before non-index siblings (via specificity bonus).
/// 4. Per-segment kind from left to right (static beats dynamic beats catchall).
/// 5. Source path, lexicographically, for total ordering.
fn sort_routes(routes: &mut [Route]) {
    routes.sort_by(|a, b| {
        a.kind
            .order_key()
            .cmp(&b.kind.order_key())
            .then_with(|| b.segments.len().cmp(&a.segments.len()))
            .then_with(|| b.specificity.cmp(&a.specificity))
            .then_with(|| {
                for (sa, sb) in a.segments.iter().zip(b.segments.iter()) {
                    let ord = segment_rank(sa).cmp(&segment_rank(sb));
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                }
                std::cmp::Ordering::Equal
            })
            .then_with(|| a.source_path.cmp(&b.source_path))
    });
}

fn segment_rank(seg: &Segment) -> u8 {
    match seg {
        Segment::Static(_) => 0,
        Segment::Dynamic(_) => 1,
        // Required and optional catchalls share a rank: they can never
        // coexist at the same prefix (scan-time conflict), so a finer
        // ordering between them is unreachable.
        Segment::Catchall(_) | Segment::OptionalCatchall(_) => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn route_from(path: &str) -> Route {
        let p = PathBuf::from(path);
        let rel = PathBuf::from(path);
        parse_route(&p, &rel).expect("parse")
    }

    #[test]
    fn parses_static_route() {
        let r = route_from("about.tsx");
        assert_eq!(r.template(), "/about");
        assert_eq!(r.kind, RouteKind::Static);
        assert_eq!(r.output_extension, None);
    }

    #[test]
    fn parses_index_to_root() {
        let r = route_from("index.tsx");
        assert_eq!(r.template(), "/");
        assert!(r.segments.is_empty());
        assert_eq!(r.kind, RouteKind::Static);
        assert_eq!(r.output_extension, None);
    }

    #[test]
    fn parses_nested_index() {
        let r = route_from("blog/index.tsx");
        assert_eq!(r.template(), "/blog");
    }

    #[test]
    fn parses_dynamic_segment() {
        let r = route_from("blog/[slug].tsx");
        assert_eq!(r.template(), "/blog/:slug");
        assert_eq!(r.kind, RouteKind::Dynamic);
    }

    #[test]
    fn parses_catchall_segment() {
        let r = route_from("docs/[...slug].tsx");
        assert_eq!(r.template(), "/docs/:slug{.+}");
        assert_eq!(r.kind, RouteKind::Catchall);
    }

    #[test]
    fn rejects_catchall_in_middle() {
        let p = PathBuf::from("docs/[...slug]/edit.tsx");
        let err = parse_route(&p, &p).unwrap_err();
        assert!(matches!(err, RouterError::CatchallNotLast { .. }));
    }

    #[test]
    fn rejects_empty_param() {
        let p = PathBuf::from("blog/[].tsx");
        let err = parse_route(&p, &p).unwrap_err();
        assert!(matches!(err, RouterError::InvalidSegment { .. }));
    }

    // ---- optional catchall `[[...name]]` (#812) -----------------------------

    #[test]
    fn parses_optional_catchall_segment() {
        let r = route_from("docs/[[...slug]].tsx");
        assert_eq!(r.template(), "/docs/:slug{.+}?");
        assert_eq!(r.kind, RouteKind::Catchall);
        assert_eq!(
            r.segments,
            vec![
                Segment::Static("docs".into()),
                Segment::OptionalCatchall("slug".into()),
            ],
        );
    }

    #[test]
    fn parses_top_level_optional_catchall() {
        let r = route_from("[[...rest]].tsx");
        assert_eq!(r.template(), "/:rest{.+}?");
        assert_eq!(r.kind, RouteKind::Catchall);
        assert_eq!(r.segments, vec![Segment::OptionalCatchall("rest".into())]);
    }

    #[test]
    fn rejects_optional_catchall_in_middle() {
        let p = PathBuf::from("docs/[[...slug]]/edit.tsx");
        let err = parse_route(&p, &p).unwrap_err();
        assert!(matches!(err, RouterError::CatchallNotLast { .. }));
    }

    #[test]
    fn rejects_double_bracket_without_catchall_dots() {
        // `[[slug]]` is not a valid form — optional params exist only as
        // the optional catchall `[[...slug]]`.
        let p = PathBuf::from("docs/[[slug]].tsx");
        let err = parse_route(&p, &p).unwrap_err();
        assert!(matches!(err, RouterError::InvalidSegment { .. }));
    }

    #[test]
    fn rejects_empty_optional_catchall_name() {
        let p = PathBuf::from("docs/[[...]].tsx");
        let err = parse_route(&p, &p).unwrap_err();
        assert!(matches!(err, RouterError::InvalidSegment { .. }));
    }

    #[test]
    fn rejects_invalid_optional_catchall_name() {
        let p = PathBuf::from("docs/[[...1slug]].tsx");
        let err = parse_route(&p, &p).unwrap_err();
        assert!(matches!(err, RouterError::InvalidSegment { .. }));
    }

    fn scan_tree(files: &[&str]) -> Result<Vec<Route>, RouterError> {
        use std::fs;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        for rel in files {
            let abs = tmp.path().join(rel);
            fs::create_dir_all(abs.parent().unwrap()).unwrap();
            fs::write(&abs, "export default function P() { return null; }\n").unwrap();
        }
        // TempDir must outlive the scan; routes carry owned PathBufs so
        // returning them after drop is fine.
        scan_pages(tmp.path())
    }

    #[test]
    fn optional_catchall_conflicts_with_sibling_index() {
        // Both produce the bare `/docs` URL. Note `[` sorts before `i` in
        // the directory walk, so this also pins the full-set conflict pass
        // (a forward-only check would visit `[[...slug]].tsx` first and
        // miss `index.tsx`).
        let err = scan_tree(&["docs/[[...slug]].tsx", "docs/index.tsx"]).unwrap_err();
        assert!(
            matches!(err, RouterError::OptionalCatchallConflict { .. }),
            "expected OptionalCatchallConflict, got {err:?}",
        );
        assert!(err.to_string().contains("/docs"), "got: {err}");
    }

    #[test]
    fn optional_catchall_conflicts_with_file_route_at_prefix() {
        // `pages/docs.tsx` also produces `/docs`.
        let err = scan_tree(&["docs.tsx", "docs/[[...slug]].tsx"]).unwrap_err();
        assert!(matches!(err, RouterError::OptionalCatchallConflict { .. }));
    }

    #[test]
    fn optional_catchall_conflicts_with_required_catchall_any_name() {
        // Overlap on every non-empty path, regardless of param name.
        let err = scan_tree(&["docs/[...a].tsx", "docs/[[...b]].tsx"]).unwrap_err();
        assert!(matches!(err, RouterError::OptionalCatchallConflict { .. }));
    }

    #[test]
    fn two_optional_catchalls_at_same_prefix_conflict() {
        let err = scan_tree(&["docs/[[...a]].tsx", "docs/[[...b]].tsx"]).unwrap_err();
        assert!(matches!(err, RouterError::OptionalCatchallConflict { .. }));
    }

    #[test]
    fn root_optional_catchall_conflicts_with_root_index() {
        let err = scan_tree(&["[[...rest]].tsx", "index.tsx"]).unwrap_err();
        assert!(matches!(err, RouterError::OptionalCatchallConflict { .. }));
    }

    #[test]
    fn optional_catchall_coexists_with_deeper_routes() {
        // More-specific routes under the prefix are NOT conflicts — they
        // coexist via the specificity sort, exactly like with a required
        // catchall.
        let routes = scan_tree(&[
            "docs/[[...slug]].tsx",
            "docs/about.tsx",
            "docs/sub/[id].tsx",
        ])
        .expect("deeper routes under an optional catchall must coexist");
        assert_eq!(routes.len(), 3);
        // The optional catchall must sort last (loosest matcher).
        assert_eq!(
            routes.last().unwrap().template(),
            "/docs/:slug{.+}?",
            "optional catchall should sort last: {:?}",
            routes.iter().map(Route::template).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn required_catchall_unaffected_by_optional_support() {
        // The strict form keeps its behaviour: a required catchall at one
        // prefix and an optional at another coexist.
        let routes = scan_tree(&["docs/[...slug].tsx", "manual/[[...slug]].tsx"])
            .expect("catchalls at different prefixes must coexist");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert!(templates.contains(&"/docs/:slug{.+}".to_string()));
        assert!(templates.contains(&"/manual/:slug{.+}?".to_string()));
    }

    // ---- non-HTML page convention (Sub 49) -------------------------------

    #[test]
    fn extension_convention_xml() {
        // `pages/sitemap.xml.tsx` → URL `/sitemap.xml`, output extension xml.
        let r = route_from("sitemap.xml.tsx");
        assert_eq!(r.template(), "/sitemap.xml");
        assert_eq!(r.output_extension.as_deref(), Some("xml"));
        assert_eq!(r.kind, RouteKind::Static);
        assert_eq!(
            r.output_filename(None),
            PathBuf::from("sitemap.xml"),
        );
    }

    #[test]
    fn extension_convention_multi_dot() {
        // Only the LAST dot before `.tsx` counts; earlier dots are
        // part of the URL.
        let r = route_from("api.v2.json.tsx");
        assert_eq!(r.template(), "/api.v2.json");
        assert_eq!(r.output_extension.as_deref(), Some("json"));
        assert_eq!(
            r.output_filename(None),
            PathBuf::from("api.v2.json"),
        );
    }

    #[test]
    fn frontmatter_extension_override_replaces_filename_extension() {
        // `pages/sitemap.xml.tsx` with frontmatter `extension: "rss"`
        // should write to `sitemap.rss`.
        let r = route_from("sitemap.xml.tsx");
        assert_eq!(
            r.output_filename(Some("rss")),
            PathBuf::from("sitemap.rss"),
        );
    }

    #[test]
    fn html_default_uses_directory_index() {
        // No filename extension and no frontmatter override → standard
        // `<path>/index.html` layout.
        let about = route_from("about.tsx");
        assert_eq!(about.output_filename(None), PathBuf::from("about/index.html"));

        let root = route_from("index.tsx");
        assert_eq!(root.output_filename(None), PathBuf::from("index.html"));

        let nested = route_from("blog/index.tsx");
        assert_eq!(
            nested.output_filename(None),
            PathBuf::from("blog/index.html"),
        );
    }

    // ---- output_filename: extension preservation -------------------------

    #[test]
    fn output_filename_index_tsx_emits_index_html() {
        let r = route_from("index.tsx");
        assert_eq!(r.output_filename(None), PathBuf::from("index.html"));
    }

    #[test]
    fn output_filename_index_xml_tsx_top_level() {
        // Top-level `index.xml.tsx` → `index.xml` (the parser drops
        // the `index` segment, so `output_filename` re-attaches it).
        let r = route_from("index.xml.tsx");
        assert_eq!(r.output_extension.as_deref(), Some("xml"));
        assert_eq!(r.output_filename(None), PathBuf::from("index.xml"));
    }

    #[test]
    fn output_filename_feed_xml_tsx() {
        let r = route_from("feed.xml.tsx");
        assert_eq!(r.output_extension.as_deref(), Some("xml"));
        assert_eq!(r.output_filename(None), PathBuf::from("feed.xml"));
    }

    #[test]
    fn output_filename_sitemap_xml_tsx() {
        let r = route_from("sitemap.xml.tsx");
        assert_eq!(r.output_extension.as_deref(), Some("xml"));
        assert_eq!(r.output_filename(None), PathBuf::from("sitemap.xml"));
    }

    #[test]
    fn output_filename_nested_index_xml_preserves_extension() {
        // Regression: `blog/index.xml.tsx` previously wrote to a file
        // literally named `blog`. It should write to `blog/index.xml`.
        let r = route_from("blog/index.xml.tsx");
        assert_eq!(r.output_extension.as_deref(), Some("xml"));
        assert_eq!(
            r.output_filename(None),
            PathBuf::from("blog/index.xml"),
        );
    }

    // ---- error-page convention (Sub 107) ------------------------------------

    #[test]
    fn top_level_404_emits_at_root() {
        // `pages/404.tsx` → `404.html` (not `404/index.html`).
        let r = route_from("404.tsx");
        assert_eq!(r.template(), "/404");
        assert_eq!(r.kind, RouteKind::Static);
        assert_eq!(r.output_extension, None);
        assert_eq!(r.output_filename(None), PathBuf::from("404.html"));
    }

    #[test]
    fn top_level_500_emits_at_root() {
        // `pages/500.tsx` → `500.html` (not `500/index.html`).
        let r = route_from("500.tsx");
        assert_eq!(r.template(), "/500");
        assert_eq!(r.kind, RouteKind::Static);
        assert_eq!(r.output_extension, None);
        assert_eq!(r.output_filename(None), PathBuf::from("500.html"));
    }

    #[test]
    fn nested_404_still_uses_directory_index() {
        // `pages/foo/404.tsx` is a regular page: `foo/404/index.html`.
        let r = route_from("foo/404.tsx");
        assert_eq!(r.template(), "/foo/404");
        assert_eq!(
            r.output_filename(None),
            PathBuf::from("foo/404/index.html"),
        );
    }

    #[test]
    fn about_still_uses_directory_index() {
        // Non-error pages are unaffected.
        let r = route_from("about.tsx");
        assert_eq!(
            r.output_filename(None),
            PathBuf::from("about/index.html"),
        );
    }

    // ---- .md page sources (Sub 406) ----------------------------------------

    #[test]
    fn md_static_about() {
        let r = route_from("about.md");
        assert_eq!(r.template(), "/about");
        assert_eq!(r.kind, RouteKind::Static);
        assert_eq!(r.output_extension, None);
        assert_eq!(r.output_filename(None), PathBuf::from("about/index.html"));
    }

    #[test]
    fn md_index_root() {
        let r = route_from("index.md");
        assert_eq!(r.template(), "/");
        assert!(r.segments.is_empty());
        assert_eq!(r.output_extension, None);
    }

    #[test]
    fn md_nested_path() {
        let r = route_from("blog/post.md");
        assert_eq!(r.template(), "/blog/post");
        assert_eq!(r.kind, RouteKind::Static);
        assert_eq!(r.output_extension, None);
    }

    // ---- .mdx page sources (zfb#404) ---------------------------------------

    #[test]
    fn mdx_static_about() {
        // Regression: `.mdx` was not stripped, leaving `/about.mdx`.
        let r = route_from("about.mdx");
        assert_eq!(r.template(), "/about");
        assert_eq!(r.kind, RouteKind::Static);
        assert_eq!(r.output_extension, None);
        assert_eq!(r.output_filename(None), PathBuf::from("about/index.html"));
    }

    #[test]
    fn mdx_index_root() {
        let r = route_from("index.mdx");
        assert_eq!(r.template(), "/");
        assert!(r.segments.is_empty());
        assert_eq!(r.output_extension, None);
    }

    #[test]
    fn mdx_nested_path() {
        let r = route_from("blog/post.mdx");
        assert_eq!(r.template(), "/blog/post");
        assert_eq!(r.kind, RouteKind::Static);
        assert_eq!(r.output_extension, None);
    }

    // ---- .html page sources (Sub 406) --------------------------------------

    #[test]
    fn html_static_contact() {
        let r = route_from("contact.html");
        assert_eq!(r.template(), "/contact");
        assert_eq!(r.kind, RouteKind::Static);
        assert_eq!(r.output_extension, None);
        assert_eq!(r.output_filename(None), PathBuf::from("contact/index.html"));
    }

    #[test]
    fn html_index_root() {
        let r = route_from("index.html");
        assert_eq!(r.template(), "/");
        assert!(r.segments.is_empty());
        assert_eq!(r.output_extension, None);
    }

    // ---- .html static_html flag (Sub 409) ----------------------------------

    #[test]
    fn html_source_has_static_html_true() {
        // `.html` pages bypass JS render — the flag must be set.
        let r = route_from("contact.html");
        assert!(r.static_html, "contact.html must have static_html=true");
    }

    #[test]
    fn tsx_source_has_static_html_false() {
        // `.tsx` pages go through JS render — flag must be false.
        let r = route_from("about.tsx");
        assert!(!r.static_html, "about.tsx must have static_html=false");
    }

    #[test]
    fn md_source_has_static_html_false() {
        // `.md` pages go through MDX compilation — flag must be false.
        let r = route_from("post.md");
        assert!(!r.static_html, "post.md must have static_html=false");
    }

    #[test]
    fn html_index_root_has_static_html_true() {
        // The index `.html` page must also carry the flag.
        let r = route_from("index.html");
        assert!(r.static_html, "index.html must have static_html=true");
    }

    // ---- underscore-stem skipping for new extensions -----------------------

    #[test]
    fn md_underscore_stem_is_skipped_by_scan() {
        // `_private.md` must be skipped by scan_pages.
        // We test this via scan_pages with a temp dir.
        use std::fs;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let pages = tmp.path();
        fs::write(pages.join("_private.md"), "# private").unwrap();
        let routes = scan_pages(pages).expect("scan");
        assert!(routes.is_empty(), "underscore .md file should be skipped");
    }

    // ---- unknown-extension warning (Sub 406) --------------------------------

    #[test]
    #[tracing_test::traced_test]
    fn unknown_extension_emits_warning() {
        use std::fs;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let pages = tmp.path();
        fs::write(pages.join("notes.txt"), "hello").unwrap();
        let routes = scan_pages(pages).expect("scan");
        assert!(routes.is_empty(), "unknown extension file should be skipped");
        assert!(
            logs_contain("unrecognised extension"),
            "expected a warning about the unrecognised extension"
        );
    }
}
