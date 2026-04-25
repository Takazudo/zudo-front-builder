//! Scan a `pages/` directory and build the route table.
//!
//! Conventions follow Next.js / Astro:
//!
//! | source                              | route         |
//! |-------------------------------------|---------------|
//! | `pages/index.tsx`                   | `/`           |
//! | `pages/about.tsx`                   | `/about`      |
//! | `pages/blog/index.tsx`              | `/blog`       |
//! | `pages/blog/[slug].tsx`             | `/blog/:slug` |
//! | `pages/blog/page/[page].tsx`        | `/blog/page/:page` |
//! | `pages/docs/[...slug].tsx`          | `/docs/:slug*` |
//! | `pages/[lang]/[slug].tsx`           | `/:lang/:slug` |
//!
//! Files starting with `_` (e.g. `_app.tsx`, `_document.tsx`) and any file
//! whose extension is not `.tsx` are ignored.

use std::collections::HashMap;
use std::path::{Component, Path};

use walkdir::WalkDir;

use crate::error::RouterError;
use crate::route::{Route, RouteKind, Segment};

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

        // Only .tsx files participate.
        if path.extension().and_then(|e| e.to_str()) != Some("tsx") {
            continue;
        }

        // Skip files whose stem starts with '_' (framework internals).
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

        let route = parse_route(path, rel)?;
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
    let mut raw_segments: Vec<&str> = Vec::new();
    let components: Vec<&str> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();

    let last_idx = components.len().saturating_sub(1);
    for (i, comp) in components.iter().enumerate() {
        if i == last_idx {
            // Strip `.tsx` extension.
            let stem = comp.strip_suffix(".tsx").unwrap_or(comp);
            if stem == "index" {
                // Index file contributes no segment.
                continue;
            }
            raw_segments.push(stem);
        } else {
            raw_segments.push(comp);
        }
    }

    let mut segments: Vec<Segment> = Vec::with_capacity(raw_segments.len());
    let total = raw_segments.len();
    for (i, raw) in raw_segments.into_iter().enumerate() {
        let parsed = parse_segment(source, raw)?;
        if let Segment::Catchall(_) = &parsed {
            if i != total - 1 {
                return Err(RouterError::CatchallNotLast {
                    path: source.to_path_buf(),
                    segment: raw.to_string(),
                });
            }
        }
        segments.push(parsed);
    }

    let kind = classify(&segments);
    let specificity = score(&segments, source);

    Ok(Route {
        source_path: source.to_path_buf(),
        segments,
        kind,
        specificity,
    })
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
            Segment::Catchall(_) => return RouteKind::Catchall,
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
            Segment::Catchall(_) => CATCHALL_WEIGHT,
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
        Segment::Catchall(_) => 2,
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
    }

    #[test]
    fn parses_index_to_root() {
        let r = route_from("index.tsx");
        assert_eq!(r.template(), "/");
        assert!(r.segments.is_empty());
        assert_eq!(r.kind, RouteKind::Static);
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
        assert_eq!(r.template(), "/docs/:slug*");
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
}
