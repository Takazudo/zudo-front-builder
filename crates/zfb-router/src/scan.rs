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
//! Files starting with `_` (e.g. `_app.tsx`, `_document.tsx`), and any page
//! nested under a directory starting with `_` (e.g. `_components/foo.tsx`),
//! are ignored — see [`zfb_types::path_has_private_prefix_component`], the
//! single source of truth shared with the bundler's `derive_route`.
//! Accepted page extensions: `.tsx`, `.ts`, `.jsx`, `.js`, `.mdx`, `.md`,
//! `.html` (see [`zfb_types::ROUTABLE_PAGE_EXTENSIONS`], the single source of
//! truth shared with the bundler). Files with any other extension are
//! skipped with a `tracing::warn!` so authors notice accidental
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

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

use walkdir::WalkDir;

use crate::error::RouterError;
use crate::route::{Route, RouteKind, Segment};

/// Extensions accepted as page source files.
///
/// Re-exported from [`zfb_types::ROUTABLE_PAGE_EXTENSIONS`] — the single
/// source of truth shared with the bundler (`zfb-build`'s `derive_route`),
/// so the two layers cannot silently drift apart again (issue #1742 / epic
/// #1990). `mdx` is included for parity with existing zfb projects that
/// author MDX pages directly in `pages/`. The bundler routes `.mdx` and
/// `.md` through the same MDX-compile pipeline, but the router must accept
/// both shapes so `pages/about.mdx` continues to produce `/about` (zfb#404
/// regression fix — earlier diff briefly dropped `.mdx` while extending the
/// allowlist).
const ACCEPTED_PAGE_EXTENSIONS: &[&str] = zfb_types::ROUTABLE_PAGE_EXTENSIONS;

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
    let (mut routes, _skipped_dynamic) = scan_pages_inner(pages_dir)?;
    detect_ambiguity(&routes)?;
    sort_routes(&mut routes);
    Ok(routes)
}

/// Shape keys of every page-shaped file under `pages/`, **including** the dynamic
/// `.md`/`.html` routes that [`scan_pages`] deliberately skips (those are still
/// routes the user authored, so a package-owned route must not silently shadow
/// them — this powers the user-wins pre-scan in `zfb`'s package-route resolver,
/// #1201). Non-page files (`_`-prefixed, `*.client.*`, unrecognised extension)
/// are excluded, exactly as [`scan_pages`] excludes them.
///
/// Buildable-route ambiguity is still surfaced — the buildable set runs through
/// [`detect_ambiguity`]. The skipped dynamic `.md`/`.html` shapes are unioned in
/// as **side data only**: they are NOT fed to ambiguity detection, so a
/// `docs/[id].tsx` + `docs/[slug].md` pair does not become a false hard error
/// (matching `scan_pages`, which skips the `.md` and succeeds).
pub fn user_page_shape_keys(pages_dir: &Path) -> Result<HashSet<String>, RouterError> {
    let (routes, skipped_dynamic) = scan_pages_inner(pages_dir)?;
    // Run ambiguity detection on the BUILDABLE routes only — preserves the
    // existing `scan_pages` error behaviour — then union the skipped-dynamic
    // shapes in afterwards so they cannot manufacture a new ambiguity error.
    detect_ambiguity(&routes)?;
    let mut keys: HashSet<String> = routes.iter().map(|r| shape_key(&r.segments)).collect();
    keys.extend(skipped_dynamic);
    Ok(keys)
}

/// Shared directory walk for [`scan_pages`] and [`user_page_shape_keys`].
///
/// Returns `(routes, skipped_dynamic_md_html_shape_keys)`: the buildable routes,
/// plus the shape keys of the dynamic `.md`/`.html` files skipped at the v1 gate
/// (recorded here so callers can apply user-wins precedence without
/// re-implementing the walk and drifting from the filename rules). Runs neither
/// [`detect_ambiguity`] nor [`sort_routes`] — each caller decides.
fn scan_pages_inner(pages_dir: &Path) -> Result<(Vec<Route>, Vec<String>), RouterError> {
    if !pages_dir.is_dir() {
        return Err(RouterError::PagesDirMissing(pages_dir.to_path_buf()));
    }

    let mut routes: Vec<Route> = Vec::new();
    let mut skipped_dynamic_md_html_shape_keys: Vec<String> = Vec::new();

    for entry in WalkDir::new(pages_dir)
        .follow_links(false)
        .sort_by_file_name()
    {
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

        // Skip files whose stem cannot be decoded as UTF-8 before running
        // the shared privacy check below — this is the router's own
        // long-standing posture (unlike `derive_route`, which falls through
        // to a lossy conversion instead; see
        // `zfb_types::path_has_private_prefix_component`'s doc for why the
        // two layers are allowed to diverge here).
        if path.file_stem().and_then(|s| s.to_str()).is_none() {
            continue;
        }

        let rel = path.strip_prefix(pages_dir).map_err(|_| RouterError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other("entry path was outside pages_dir"),
        })?;

        // Skip files whose stem starts with '_' (framework internals), or
        // that traverse a directory whose name starts with '_' — these are
        // conventionally private (e.g. `pages/_components/`, `_app.tsx`).
        // Shared with `zfb-build`'s `derive_route` so the two layers cannot
        // silently drift apart again (issue #2123 / #2148).
        if zfb_types::path_has_private_prefix_component(rel) {
            continue;
        }

        // Skip `*.client.{ts,tsx,js,jsx}` client-script entries. A file like
        // `pages/search-widget.client.tsx` is bundled by the client-script
        // pipeline (`zfb-islands`), not rendered as a page — it has no page
        // default export. Without this skip the `.tsx`/`.jsx` extension would
        // make `scan_pages` accept it as a `/search-widget.client` route, so
        // the build/render would fail or ship an unintended route. The
        // filename contract is the single source of truth in `zfb-types`,
        // shared with `zfb-islands`'s discovery.
        if zfb_types::is_client_script_file(path) {
            continue;
        }

        // Skip conventional sidecars that carry a routable extension but are
        // never pages: TypeScript declaration files (`env.d.ts`) and colocated
        // tests (`index.test.ts`, `about.spec.tsx`). Widening the allowlist to
        // `.ts`/`.js`/`.jsx` (epic #1990) newly swept these in — an
        // `index.test.ts` beside `index.tsx` would otherwise turn a green build
        // into `RouterError::AmbiguousRoute`. Deliberately SILENT (debug, not
        // the `warn!` below): these extensions ARE recognised, the files are
        // just not pages, so the "unrecognised extension" warning would be
        // actively misleading. The filename contract is the single source of
        // truth in `zfb-types`, beside `is_client_script_file`.
        if zfb_types::is_page_sidecar_file(path) {
            tracing::debug!(
                path = %rel.display(),
                "skipping conventional non-page sidecar under pages/ \
                 (*.d.ts / *.test.* / *.spec.*)"
            );
            continue;
        }

        // Accept .tsx, .ts, .jsx, .js, .mdx, .md, .html as page sources. Warn
        // on any other extension so authors notice accidental mis-placements
        // in pages/.
        match ext {
            Some(e) if ACCEPTED_PAGE_EXTENSIONS.contains(&e) => {}
            _ => {
                tracing::warn!(
                    path = %rel.display(),
                    "pages/ file has an unrecognised extension and will be skipped; \
                     accepted extensions are: tsx, ts, jsx, js, mdx, md, html"
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
        if matches!(ext, Some("md") | Some("html")) && !matches!(route.kind, RouteKind::Static) {
            tracing::warn!(
                path = %rel.display(),
                kind = ?route.kind,
                "dynamic .md / .html page routes are not supported in v1 (no paths() story); \
                 this file will be skipped — author it as .tsx if dynamic paths are needed"
            );
            // The file is skipped as a buildable route, but the user DID author a
            // page at this route shape — record its shape key so the user-wins
            // pre-scan (#1201) drops a same-shape package route instead of letting
            // it silently shadow this page. Side data only — never fed to
            // `detect_ambiguity` (see `user_page_shape_keys`).
            skipped_dynamic_md_html_shape_keys.push(shape_key(&route.segments));
            // An optional catchall (`[[...rest]]`) ALSO owns its zero-segment
            // prefix URL — `pages/docs/[[...rest]].md` serves `/docs` too. Record
            // the prefix shape as well, mirroring `detect_optional_catchall_conflicts`,
            // so a package route at the bare URL can't slip past the exact-key
            // pre-scan and silently shadow it (codex review).
            if matches!(route.segments.last(), Some(Segment::OptionalCatchall(_))) {
                let prefix = &route.segments[..route.segments.len() - 1];
                skipped_dynamic_md_html_shape_keys.push(shape_key(prefix));
            }
            continue;
        }

        routes.push(route);
    }

    Ok((routes, skipped_dynamic_md_html_shape_keys))
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
            //
            // Applies to every script page source (`.tsx`/`.ts`/`.jsx`/
            // `.js` — `zfb_types::SCRIPT_PAGE_EXTENSIONS`), not just
            // `.tsx`/`.ts`: a `.js`/`.jsx` page must get the same
            // `sitemap.xml.js` → `output_extension = Some("xml")`
            // convention as its `.tsx`/`.ts` equivalent now that both are
            // routable (epic #1990).
            let is_script_page = zfb_types::SCRIPT_PAGE_EXTENSIONS
                .iter()
                .any(|ext| comp.ends_with(&format!(".{ext}")));
            let stem_is_param = stem.starts_with('[');
            if is_script_page && !stem_is_param {
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
        if matches!(&parsed, Segment::Catchall(_) | Segment::OptionalCatchall(_)) && i != total - 1
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
///
/// The recognised set is exactly [`ACCEPTED_PAGE_EXTENSIONS`]
/// (`zfb_types::ROUTABLE_PAGE_EXTENSIONS`) — this answers "can this file be
/// routed", so it must be the ROUTABLE subset, not the narrower script one:
/// `about.md` and `about.html` need their extensions stripped just as much as
/// `about.tsx` does. Consuming the shared constant means an eighth routable
/// extension cannot be accepted at the gate above yet left un-stripped here,
/// which would leak the extension into the URL segment (`/about.ts`).
///
/// Order is irrelevant: no accepted extension is a suffix of another once the
/// leading `.` is included (`"a.tsx".strip_suffix(".ts")` is `None`).
///
/// Returns the input unchanged when the component does not end with one of
/// those extensions.
fn strip_source_extension(component: &str) -> &str {
    for ext in ACCEPTED_PAGE_EXTENSIONS {
        if let Some(stem) = component.strip_suffix(&format!(".{ext}")) {
            return stem;
        }
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
    // Optional-catchall conflicts run first: those pairs (an `[[...name]]`
    // route vs a same-shaped sibling or another catchall at its prefix) get
    // the more specific `OptionalCatchallConflict` error, including the
    // cross-length cases the full-shape check below cannot see (a `[[...]]`
    // also matches zero segments).
    detect_optional_catchall_conflicts(routes)?;
    detect_shape_duplicates(routes)
}

/// Reject two DIFFERENT source files whose routes share a segment-kind shape
/// (static segments by literal, dynamic / catchall / optional-catchall by
/// kind only). Such routes match exactly the same set of URLs regardless of
/// how their params are named, so one would silently shadow the other via an
/// arbitrary tiebreak.
///
/// This generalises the older byte-identical-template check: a duplicate
/// template has an identical shape too, but so does `docs/[a].tsx` vs
/// `docs/[b].tsx` (`/docs/:*`) and `docs/[...a].tsx` vs `docs/[...b].tsx`
/// (`/docs/:...`), which the template check missed (zfb#816). When the two
/// templates are byte-identical we keep the clearer `AmbiguousRoute` error;
/// when only the shape matches we raise `AmbiguousShape`.
///
/// Differing static segments keep legitimate siblings legal:
/// `/[lang]/[slug]` (`/:*/:*`) vs `/blog/[slug]` (`/blog/:*`) differ at index
/// 0, and `/docs/[a]/x` (`/docs/:*/x`) vs `/docs/[b]/y` (`/docs/:*/y`) differ
/// in the static tail.
fn detect_shape_duplicates(routes: &[Route]) -> Result<(), RouterError> {
    let mut seen: HashMap<String, &Route> = HashMap::new();
    for route in routes {
        let key = shape_key(&route.segments);
        if let Some(prev) = seen.insert(key.clone(), route) {
            let prev_template = prev.template();
            let template = route.template();
            if prev_template == template {
                return Err(RouterError::AmbiguousRoute {
                    template,
                    first: prev.source_path.clone(),
                    second: route.source_path.clone(),
                });
            }
            return Err(RouterError::AmbiguousShape {
                shape: key,
                first: prev.source_path.clone(),
                second: route.source_path.clone(),
            });
        }
    }
    Ok(())
}

/// Render a segment slice as a param-name-insensitive shape key: static
/// segments keep their literal, dynamic segments collapse to `:*`, and
/// catchall segments to `:...`. Two routes (or prefixes) with equal shape
/// keys match exactly the same set of URLs regardless of how their params
/// are named — `/[id]` and `/[lang]` both render as `/:*`.
pub fn shape_key(segments: &[Segment]) -> String {
    if segments.is_empty() {
        return "/".to_string();
    }
    let mut out = String::new();
    for seg in segments {
        out.push('/');
        match seg {
            Segment::Static(s) => out.push_str(s),
            Segment::Dynamic(_) => out.push_str(":*"),
            Segment::Catchall(_) | Segment::OptionalCatchall(_) => out.push_str(":..."),
        }
    }
    out
}

/// Compute the param-name-insensitive [`shape_key`] of a route from a
/// `pages_dir`-relative source path (e.g. `blog/[slug].tsx`,
/// `index.tsx`).
///
/// This parses the path through the SAME grammar [`scan_pages`] uses, so
/// the resulting key is directly comparable to the keys of scanned
/// routes. It exists so the build's package-owned-routes materialiser
/// (#1193) can implement user-`pages/`-wins precedence by a pre-scan
/// drop: `detect_ambiguity` is origin-blind and shape-keyed, so a
/// user-vs-package collision must be resolved BEFORE the merged scan or
/// it hard-errors (`[id]` ≡ `[slug]`).
///
/// `rel` must be relative; `_app.tsx`-style private files are not
/// special-cased here (the caller derives `rel` from a route pattern, so
/// it never names a private file).
pub fn route_shape_key_for_pages_rel(rel: &Path) -> Result<String, RouterError> {
    // `source` is used only for error context inside `parse_route`; the
    // rel path is what drives segment parsing.
    let route = parse_route(rel, rel)?;
    Ok(shape_key(&route.segments))
}

/// Render the template of a segment-slice prefix (`/docs` for
/// `[Static("docs")]`, `/` for the empty slice). Mirrors
/// [`Route::template`] but over an arbitrary prefix. Used for error
/// messages only — conflict comparisons use [`shape_key`].
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
/// For each `[[...name]]` route with prefix `P` (the URL set it serves
/// for zero segments):
///
/// 1. Any route whose full shape equals `P`'s shape conflicts — both
///    produce the bare URL (e.g. `pages/docs/index.tsx` / `pages/docs.tsx`
///    vs `pages/docs/[[...slug]].tsx`, all serving `/docs`). The shape
///    comparison is param-name-insensitive so `pages/[id].tsx` also
///    conflicts with `pages/[lang]/[[...slug]].tsx` — both match `/en`.
/// 2. Any other catchall (required or optional) at a same-shaped prefix
///    conflicts regardless of param names — they overlap on every
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
        let prefix_segs = &route.segments[..route.segments.len() - 1];
        let prefix_shape = shape_key(prefix_segs);
        let prefix = prefix_template(prefix_segs);
        for (j, other) in routes.iter().enumerate() {
            if i == j {
                continue;
            }
            if shape_key(&other.segments) == prefix_shape {
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
            ) && shape_key(&other.segments[..other.segments.len() - 1]) == prefix_shape
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
/// 1. Per-segment rank vector, compared lexicographically left to right
///    (static `0` < dynamic `1` < catchall / optional-catchall `2`); a
///    shorter prefix-matching vector sorts first — most specific first.
/// 2. `index.tsx` before non-index siblings (via specificity bonus).
/// 3. Source path, lexicographically, for total ordering.
///
/// The rank-vector key is kept byte-for-byte in sync with
/// `zfb_build::bundler::route_sort_key` (the JS/Hono runtime's
/// registration order). Mirroring it here is load-bearing: dev SSR
/// dispatch (`DevRenderSession::ssr_patterns` → `SsrRouteSet::find_match`,
/// first-match) preserves this scan order, so a coarse per-route kind
/// sort (the previous design) would let a top-level dynamic SSR route
/// (`/[id]`, `/[lang]/[slug]`) steal `/docs` / `/docs/a` from a
/// static-prefixed optional catchall (`/docs/[[...slug]]`) in dev while
/// the bundled Hono runtime correctly chose the catchall — a dev/prod
/// routing divergence for the optional-catchall route type.
fn sort_routes(routes: &mut [Route]) {
    routes.sort_by(|a, b| {
        route_rank_vector(&a.segments)
            .cmp(&route_rank_vector(&b.segments))
            .then_with(|| b.specificity.cmp(&a.specificity))
            .then_with(|| a.source_path.cmp(&b.source_path))
    });
}

/// Per-segment specificity rank vector for a route. Mirrors
/// `zfb_build::bundler::route_sort_key` so dev and prod agree on which
/// of two overlapping routes is more specific (see [`sort_routes`]).
fn route_rank_vector(segments: &[Segment]) -> Vec<u8> {
    segments.iter().map(segment_rank).collect()
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

    /// Like [`scan_tree`] but returns the [`user_page_shape_keys`] census.
    fn census_tree(files: &[&str]) -> Result<HashSet<String>, RouterError> {
        use std::fs;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        for rel in files {
            let abs = tmp.path().join(rel);
            fs::create_dir_all(abs.parent().unwrap()).unwrap();
            fs::write(&abs, "export default function P() { return null; }\n").unwrap();
        }
        user_page_shape_keys(tmp.path())
    }

    #[test]
    fn census_includes_dynamic_md_html_that_scan_pages_skips() {
        // scan_pages skips a dynamic `.md`/`.html` route (v1-unsupported) → no route.
        assert!(scan_tree(&["docs/[slug].md"]).unwrap().is_empty());
        assert!(scan_tree(&["docs/[slug].html"]).unwrap().is_empty());

        // ...but the census MUST record their shape keys so a same-shape package
        // route can be dropped by the user-wins pre-scan (#1201). Compute the
        // expected key via the public grammar helper rather than hardcoding it.
        let md_key = route_shape_key_for_pages_rel(Path::new("docs/[slug].md")).unwrap();
        let keys = census_tree(&["docs/[slug].md"]).unwrap();
        assert!(
            keys.contains(&md_key),
            "census must include the skipped dynamic .md shape; got {keys:?}"
        );

        let html_key = route_shape_key_for_pages_rel(Path::new("docs/[slug].html")).unwrap();
        let keys = census_tree(&["docs/[slug].html"]).unwrap();
        assert!(
            keys.contains(&html_key),
            "census must include the skipped dynamic .html shape; got {keys:?}"
        );
    }

    #[test]
    fn census_excludes_non_page_files() {
        // `_`-prefixed, `*.client.*`, and unrecognised-extension files are not
        // page routes — they must be excluded, exactly as scan_pages excludes them.
        let keys = census_tree(&[
            "_private/helper.tsx",
            "_app.tsx",
            "widget.client.tsx",
            "notes.txt",
        ])
        .unwrap();
        assert!(
            keys.is_empty(),
            "non-page files must be excluded; got {keys:?}"
        );
    }

    /// A page nested two levels under a `_`-prefixed directory must be
    /// skipped — the privacy check walks every ancestor, not just the
    /// immediate parent — while an ordinary (non-`_`) sibling directory at
    /// the same depth still routes. Shared with `zfb-build`'s
    /// `derive_route` via `zfb_types::path_has_private_prefix_component`
    /// (issue #2123 / #2148).
    #[test]
    fn scan_pages_skips_nested_private_directory_with_positive_control() {
        let routes = scan_tree(&["_components/nested/api.tsx", "lib/nested/api.tsx"]).unwrap();
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert!(
            !templates.iter().any(|t| t == "/components/nested/api"),
            "page under a nested `_`-prefixed directory must not route; got {templates:?}"
        );
        assert!(
            templates.iter().any(|t| t == "/lib/nested/api"),
            "ordinary sibling directory at the same depth must still route; got {templates:?}"
        );
    }

    #[test]
    fn census_does_not_invent_ambiguity_for_tsx_plus_dynamic_md_sibling() {
        // `docs/[id].tsx` (buildable) + `docs/[slug].md` (skipped dynamic) share
        // the shape `/docs/:*`. scan_pages skips the `.md` and succeeds; the census
        // must likewise NOT hard-error (skipped shapes are side data, never fed to
        // detect_ambiguity) and must still contain the shape.
        let key = route_shape_key_for_pages_rel(Path::new("docs/[slug].md")).unwrap();
        let keys = census_tree(&["docs/[id].tsx", "docs/[slug].md"]).unwrap();
        assert!(
            keys.contains(&key),
            "census should contain the /docs/:* shape; got {keys:?}"
        );
    }

    #[test]
    fn census_still_errors_on_buildable_ambiguity() {
        // Two BUILDABLE dynamic siblings at the same shape are a genuine ambiguity —
        // detect_ambiguity must still fire (buildable-route behaviour unchanged).
        let err = census_tree(&["docs/[id].tsx", "docs/[slug].tsx"]).unwrap_err();
        assert!(
            matches!(err, RouterError::AmbiguousShape { .. }),
            "expected AmbiguousShape, got {err:?}"
        );
    }

    #[test]
    fn census_records_dynamic_catchall_md_shape() {
        // A catchall `.md` page contributes its catchall shape to the census,
        // while scan_pages still skips it.
        let key = route_shape_key_for_pages_rel(Path::new("docs/[...rest].md")).unwrap();
        let keys = census_tree(&["docs/[...rest].md"]).unwrap();
        assert!(
            keys.contains(&key),
            "census must include the catchall .md shape; got {keys:?}"
        );
        assert!(scan_tree(&["docs/[...rest].md"]).unwrap().is_empty());
    }

    #[test]
    fn census_records_optional_catchall_md_zero_segment_prefix() {
        // An optional-catchall `.md` page (`docs/[[...rest]].md`) owns BOTH its
        // catchall shape AND its zero-segment prefix URL (`/docs`). Both must be
        // in the census or a package `/docs` route would silently shadow the bare
        // URL the user's optional catchall serves (codex review).
        let keys = census_tree(&["docs/[[...rest]].md"]).unwrap();
        let catchall_key = route_shape_key_for_pages_rel(Path::new("docs/[[...rest]].md")).unwrap();
        let bare_key = route_shape_key_for_pages_rel(Path::new("docs/index.tsx")).unwrap();
        assert!(
            keys.contains(&catchall_key),
            "census must include the optional-catchall shape; got {keys:?}"
        );
        assert!(
            keys.contains(&bare_key),
            "census must include the zero-segment `/docs` prefix shape; got {keys:?}"
        );
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
    fn dynamic_prefix_bare_conflict_is_param_name_insensitive() {
        // `pages/[id].tsx` (`/:id`) and `pages/[lang]/[[...slug]].tsx`
        // (zero-segment prefix `/:lang`) both match `/en` — the differing
        // param names must not hide the conflict.
        let err = scan_tree(&["[id].tsx", "[lang]/[[...slug]].tsx"]).unwrap_err();
        assert!(
            matches!(err, RouterError::OptionalCatchallConflict { .. }),
            "expected OptionalCatchallConflict, got {err:?}",
        );
    }

    #[test]
    fn dynamic_prefix_catchall_overlap_is_param_name_insensitive() {
        // `pages/[a]/[...x].tsx` and `pages/[b]/[[...y]].tsx` overlap on
        // every non-empty nested path regardless of param names.
        let err = scan_tree(&["[a]/[...x].tsx", "[b]/[[...y]].tsx"]).unwrap_err();
        assert!(matches!(err, RouterError::OptionalCatchallConflict { .. }));
    }

    #[test]
    fn static_prefix_dynamic_sibling_is_not_a_bare_conflict() {
        // `pages/[id].tsx` and `pages/docs/[[...slug]].tsx`: `/docs` is
        // matched by both at runtime, but the static-prefixed catchall is
        // strictly more specific there — this is ordinary overlap resolved
        // by the specificity sort, not a conflict.
        let routes = scan_tree(&["[id].tsx", "docs/[[...slug]].tsx"])
            .expect("static-prefix optional catchall must coexist with /[id]");
        assert_eq!(routes.len(), 2);
    }

    /// Index of `template` in a scanned + sorted route list, or panic.
    fn template_index(routes: &[Route], template: &str) -> usize {
        routes
            .iter()
            .position(|r| r.template() == template)
            .unwrap_or_else(|| {
                panic!(
                    "missing route {template}; got {:?}",
                    routes.iter().map(Route::template).collect::<Vec<_>>(),
                )
            })
    }

    // ---- per-segment specificity sort (dev/prod parity, #814) --------------
    //
    // These pin the scan order that dev SSR first-match dispatch
    // (`SsrRouteSet::find_match`) preserves, against the JS/Hono runtime's
    // `route_sort_key` registration order in zfb-build's bundler.

    #[test]
    fn optional_catchall_sorts_before_top_level_dynamic_sibling() {
        // Divergence case 1: URL `/docs` is matched by both
        // `pages/docs/[[...slug]].tsx` (zero-segment optional catchall) and
        // `pages/[id].tsx`. The static-prefixed catchall is strictly more
        // specific at `/docs`, so it must sort FIRST — otherwise dev SSR
        // first-match would send `/docs` to the less-specific `/[id]`.
        let routes = scan_tree(&["[id].tsx", "docs/[[...slug]].tsx"]).expect("scan");
        assert!(
            template_index(&routes, "/docs/:slug{.+}?") < template_index(&routes, "/:id"),
            "optional catchall /docs/[[...slug]] must sort before /[id]: {:?}",
            routes.iter().map(Route::template).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn optional_catchall_sorts_before_top_level_dynamic_pair() {
        // Divergence case 2: URL `/docs/a` is matched by both
        // `pages/docs/[[...slug]].tsx` and `pages/[lang]/[slug].tsx`. The
        // static-prefixed catchall (rank `[0, 2]`) is more specific than the
        // fully-dynamic pair (rank `[1, 1]`) — first differing segment is
        // static-vs-dynamic at index 0 — so it must sort FIRST.
        let routes = scan_tree(&["[lang]/[slug].tsx", "docs/[[...slug]].tsx"]).expect("scan");
        assert!(
            template_index(&routes, "/docs/:slug{.+}?") < template_index(&routes, "/:lang/:slug"),
            "optional catchall /docs/[[...slug]] must sort before /[lang]/[slug]: {:?}",
            routes.iter().map(Route::template).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn required_catchall_orderings_unchanged() {
        // Existing required-catchall / dynamic invariants stay green under
        // the per-segment sort:
        //   - a plain dynamic sibling sorts before the catchall at the same
        //     static depth (`/docs/[id]` before `/docs/[...slug]`);
        //   - a static-prefixed catchall sorts before a fully-dynamic pair
        //     (`/docs/[...slug]` before `/[lang]/[slug]`);
        //   - a deeper dynamic descendant sorts before the shallow catchall
        //     (`/docs/v/[page]` before `/docs/[...slug]`).
        let routes = scan_tree(&[
            "docs/[id].tsx",
            "docs/[...slug].tsx",
            "docs/v/[page].tsx",
            "[lang]/[slug].tsx",
        ])
        .expect("scan");
        let cat = template_index(&routes, "/docs/:slug{.+}");
        assert!(
            template_index(&routes, "/docs/:id") < cat,
            "/docs/:id before /docs/:slug{{.+}}: {:?}",
            routes.iter().map(Route::template).collect::<Vec<_>>(),
        );
        assert!(
            cat < template_index(&routes, "/:lang/:slug"),
            "/docs/:slug{{.+}} before /:lang/:slug: {:?}",
            routes.iter().map(Route::template).collect::<Vec<_>>(),
        );
        assert!(
            template_index(&routes, "/docs/v/:page") < cat,
            "/docs/v/:page before /docs/:slug{{.+}}: {:?}",
            routes.iter().map(Route::template).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn index_sibling_outranks_top_level_dynamic() {
        // The index-bonus path still resolves a same-shape tie: `/foo`
        // (from `pages/foo/index.tsx`) and `/:foo` (from `pages/[foo].tsx`)
        // are both single-segment; the static one must sort first.
        let routes = scan_tree(&["foo/index.tsx", "[foo].tsx"]).expect("scan");
        assert!(
            template_index(&routes, "/foo") < template_index(&routes, "/:foo"),
            "static /foo must sort before dynamic /:foo: {:?}",
            routes.iter().map(Route::template).collect::<Vec<_>>(),
        );
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

    // ---- param-name-insensitive shape conflicts (#816) ---------------------

    #[test]
    fn required_catchall_siblings_with_different_param_names_conflict() {
        // `pages/docs/[...a].tsx` (`/docs/:a{.+}`) and
        // `pages/docs/[...b].tsx` (`/docs/:b{.+}`) overlap on every URL
        // either can serve. Same shape, different param names → conflict.
        let err = scan_tree(&["docs/[...a].tsx", "docs/[...b].tsx"]).unwrap_err();
        assert!(
            matches!(err, RouterError::AmbiguousShape { .. }),
            "expected AmbiguousShape, got {err:?}",
        );
        assert!(err.to_string().contains("/docs/:..."), "got: {err}");
    }

    #[test]
    fn dynamic_siblings_with_different_param_names_conflict() {
        // `pages/docs/[a].tsx` vs `pages/docs/[b].tsx`: full overlap for
        // single-segment dynamics.
        let err = scan_tree(&["docs/[a].tsx", "docs/[b].tsx"]).unwrap_err();
        assert!(
            matches!(err, RouterError::AmbiguousShape { .. }),
            "expected AmbiguousShape, got {err:?}",
        );
        assert!(err.to_string().contains("/docs/:*"), "got: {err}");
    }

    #[test]
    fn top_level_dynamic_siblings_with_different_param_names_conflict() {
        // `pages/[a].tsx` vs `pages/[b].tsx` (`/:a` vs `/:b`).
        let err = scan_tree(&["[a].tsx", "[b].tsx"]).unwrap_err();
        assert!(matches!(err, RouterError::AmbiguousShape { .. }));
    }

    #[test]
    fn nested_dynamic_siblings_with_different_param_names_conflict() {
        // Mixed static + dynamic shape: `/docs/:*/edit` shared by both.
        let err = scan_tree(&["docs/[a]/edit.tsx", "docs/[b]/edit.tsx"]).unwrap_err();
        assert!(matches!(err, RouterError::AmbiguousShape { .. }));
    }

    #[test]
    fn byte_identical_templates_still_report_ambiguous_route() {
        // The clearer `AmbiguousRoute` error is preserved when the two
        // templates are byte-identical (same param name too).
        let err = scan_tree(&["blog.tsx", "blog/index.tsx"]).unwrap_err();
        match err {
            RouterError::AmbiguousRoute { template, .. } => {
                assert_eq!(template, "/blog");
            }
            other => unreachable!("expected AmbiguousRoute, got {other:?}"),
        }
    }

    #[test]
    fn different_static_prefix_same_shape_is_legal() {
        // `/[lang]/[slug]` (`/:*/:*`) vs `/blog/[slug]` (`/blog/:*`) differ
        // at index 0 — different static prefix, both legal.
        let routes = scan_tree(&["[lang]/[slug].tsx", "blog/[slug].tsx"])
            .expect("different static prefixes must coexist");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn same_dynamic_prefix_different_static_tail_is_legal() {
        // `/docs/[a]/x` (`/docs/:*/x`) vs `/docs/[b]/y` (`/docs/:*/y`):
        // same dynamic prefix shape but different static tail → distinct
        // URL sets, both legal.
        let routes = scan_tree(&["docs/[a]/x.tsx", "docs/[b]/y.tsx"])
            .expect("different static tails must coexist");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn dynamic_and_catchall_at_same_prefix_are_legal() {
        // `/docs/[id]` (`/docs/:*`) and `/docs/[...slug]` (`/docs/:...`) have
        // DIFFERENT shapes (dynamic vs catchall) — they coexist via the
        // specificity sort, not a conflict.
        let routes = scan_tree(&["docs/[id].tsx", "docs/[...slug].tsx"])
            .expect("dynamic and catchall at the same prefix must coexist");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn required_catchall_optional_catchall_conflict_kept() {
        // Optional-catchall-involving pairs still get the more specific
        // OptionalCatchallConflict error even though they share a shape key
        // (the optional-catchall pass runs first).
        let err = scan_tree(&["docs/[...a].tsx", "docs/[[...b]].tsx"]).unwrap_err();
        assert!(
            matches!(err, RouterError::OptionalCatchallConflict { .. }),
            "expected OptionalCatchallConflict, got {err:?}",
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
        assert_eq!(r.output_filename(None), PathBuf::from("sitemap.xml"),);
    }

    #[test]
    fn extension_convention_multi_dot() {
        // Only the LAST dot before `.tsx` counts; earlier dots are
        // part of the URL.
        let r = route_from("api.v2.json.tsx");
        assert_eq!(r.template(), "/api.v2.json");
        assert_eq!(r.output_extension.as_deref(), Some("json"));
        assert_eq!(r.output_filename(None), PathBuf::from("api.v2.json"),);
    }

    #[test]
    fn frontmatter_extension_override_replaces_filename_extension() {
        // `pages/sitemap.xml.tsx` with frontmatter `extension: "rss"`
        // should write to `sitemap.rss`.
        let r = route_from("sitemap.xml.tsx");
        assert_eq!(r.output_filename(Some("rss")), PathBuf::from("sitemap.rss"),);
    }

    #[test]
    fn html_default_uses_directory_index() {
        // No filename extension and no frontmatter override → standard
        // `<path>/index.html` layout.
        let about = route_from("about.tsx");
        assert_eq!(
            about.output_filename(None),
            PathBuf::from("about/index.html")
        );

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
    fn output_filename_sitemap_xml_js_matches_tsx_convention() {
        // codex review (page-ext-centralize self-review): `.js`/`.jsx` page
        // sources must follow the same `sitemap.xml.<ext>` ->
        // `output_extension = Some("xml")` convention as `.tsx`/`.ts` now
        // that both are routable (epic #1990) — a widened router that only
        // widened the routing gate but not this convention would silently
        // regress JS/JSX sitemap-style pages to `sitemap.xml/index.html`.
        for src in ["sitemap.xml.js", "sitemap.xml.jsx"] {
            let r = route_from(src);
            assert_eq!(r.output_extension.as_deref(), Some("xml"), "{src}");
            assert_eq!(
                r.output_filename(None),
                PathBuf::from("sitemap.xml"),
                "{src}"
            );
        }
    }

    #[test]
    fn output_filename_nested_index_xml_preserves_extension() {
        // Regression: `blog/index.xml.tsx` previously wrote to a file
        // literally named `blog`. It should write to `blog/index.xml`.
        let r = route_from("blog/index.xml.tsx");
        assert_eq!(r.output_extension.as_deref(), Some("xml"));
        assert_eq!(r.output_filename(None), PathBuf::from("blog/index.xml"),);
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
        assert_eq!(r.output_filename(None), PathBuf::from("foo/404/index.html"),);
    }

    #[test]
    fn about_still_uses_directory_index() {
        // Non-error pages are unaffected.
        let r = route_from("about.tsx");
        assert_eq!(r.output_filename(None), PathBuf::from("about/index.html"),);
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

    // ---- client-script files are not routes (#971 P1) ----------------------

    #[test]
    fn client_script_file_produces_no_route() {
        // `pages/search-widget.client.tsx` is a client-script entry, not a
        // page — it must NOT be scanned into a `/search-widget.client` route.
        // Its sibling regular page is unaffected.
        let routes = scan_tree(&["search-widget.client.tsx", "index.tsx"]).expect("scan");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert!(
            templates.contains(&"/".to_string()),
            "regular index page must still route: {templates:?}",
        );
        assert!(
            !templates.iter().any(|t| t.contains("client")),
            "client-script file must not produce a route: {templates:?}",
        );
        assert_eq!(routes.len(), 1, "exactly the index route: {templates:?}");
    }

    #[test]
    fn client_script_all_extensions_produce_no_route() {
        // Every `.client.<ext>` shape is skipped; the regular `.tsx`/`.jsx`
        // siblings still route.
        let routes = scan_tree(&[
            "a.client.ts",
            "b.client.tsx",
            "c.client.js",
            "d.client.jsx",
            "real.tsx",
        ])
        .expect("scan");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert_eq!(
            templates,
            vec!["/real".to_string()],
            "only the non-client page routes: {templates:?}",
        );
    }

    #[test]
    fn regular_page_with_dotted_stem_still_routes() {
        // A page whose stem merely contains a dot (but not the `.client.`
        // infix) is unaffected — `pages/clientele.tsx` → `/clientele`.
        let routes = scan_tree(&["clientele.tsx"]).expect("scan");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert_eq!(templates, vec!["/clientele".to_string()]);
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
        assert!(
            routes.is_empty(),
            "unknown extension file should be skipped"
        );
        assert!(
            logs_contain("unrecognised extension"),
            "expected a warning about the unrecognised extension"
        );
    }

    // ---- Page Extension Contract characterization (epic #1990, #1991) ------
    //
    // Pinned the router-vs-bundler extension divergence in #1991: the
    // bundler treated `.tsx`/`.ts`/`.jsx`/`.js`/`.mdx` as page-capable script
    // sources (plus `.md`/`.html` as non-script page sources — see
    // `crates/zfb-build/src/bundler.rs`'s `derive_route`), but `scan_pages`
    // only accepted `tsx`/`mdx`/`md`/`html` — so `pages/index.ts`, `.js`,
    // `.jsx` were bundle-capable yet never routed. #1992 (Wave 2) widened
    // `ACCEPTED_PAGE_EXTENSIONS` to the shared `zfb_types::
    // ROUTABLE_PAGE_EXTENSIONS` constant, so the trio below is now green.
    // These are BEHAVIORAL tests (observed routing outcome), deliberately
    // not an equality check against the bundler's own extension list — that
    // would be tautological now that both layers share one constant.

    #[test]
    fn tsx_page_is_routed_today() {
        let routes = scan_tree(&["index.tsx"]).expect("scan");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert_eq!(templates, vec!["/".to_string()]);
    }

    #[test]
    fn mdx_page_is_routed_today() {
        // zfb#404 regression guard, exercised through the FULL scan_pages
        // gate (ACCEPTED_PAGE_EXTENSIONS), not just parse_route in isolation
        // — see mdx_static_about/mdx_index_root/mdx_nested_path above, which
        // test parse_route directly and never exercise the extension gate.
        let routes = scan_tree(&["a.mdx"]).expect("scan");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert_eq!(templates, vec!["/a".to_string()]);
    }

    #[test]
    fn md_page_is_routed_today() {
        let routes = scan_tree(&["b.md"]).expect("scan");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert_eq!(templates, vec!["/b".to_string()]);
    }

    #[test]
    fn html_page_is_routed_today() {
        let routes = scan_tree(&["c.html"]).expect("scan");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert_eq!(templates, vec!["/c".to_string()]);
    }

    #[test]
    fn ts_page_is_routed_after_epic_1990() {
        let routes = scan_tree(&["d.ts"]).expect("scan");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert_eq!(templates, vec!["/d".to_string()]);
    }

    #[test]
    fn js_page_is_routed_after_epic_1990() {
        let routes = scan_tree(&["e.js"]).expect("scan");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert_eq!(templates, vec!["/e".to_string()]);
    }

    #[test]
    fn jsx_page_is_routed_after_epic_1990() {
        let routes = scan_tree(&["f.jsx"]).expect("scan");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert_eq!(templates, vec!["/f".to_string()]);
    }

    #[test]
    #[tracing_test::traced_test]
    fn txt_page_is_skipped_and_warned_negative_control() {
        // Negative control (table row `pages/g.txt`): an extension with no
        // stake in this epic must remain skipped + warned both before and
        // after the widening — this must NEVER flip green.
        let routes = scan_tree(&["g.txt"]).expect("scan");
        assert!(routes.is_empty(), ".txt page must be skipped: {routes:?}");
        assert!(
            logs_contain("unrecognised extension"),
            "expected a warning about the unrecognised .txt extension"
        );
    }

    #[test]
    #[tracing_test::traced_test]
    fn dynamic_md_route_stays_skipped_and_warned_across_the_epic() {
        // Pins the existing dynamic/catchall carve-out (see the
        // `matches!(ext, Some("md") | Some("html")) && !matches!(route.kind, ...)`
        // gate above): dynamic `.md` routes have no `paths()` story and are
        // skipped even though `.md` itself is an accepted extension. The
        // gate is keyed on `ext` + `route.kind`, not on the accepted-
        // extensions allowlist, so it must survive Wave 2's widening
        // untouched.
        let routes = scan_tree(&["docs/[slug].md"]).expect("scan");
        assert!(
            routes.is_empty(),
            "dynamic .md page must stay skipped: {routes:?}"
        );
        assert!(
            logs_contain("dynamic .md / .html page routes are not supported"),
            "expected the dynamic .md/.html v1 warning"
        );
    }

    #[test]
    #[tracing_test::traced_test]
    fn dynamic_html_route_stays_skipped_and_warned_across_the_epic() {
        let routes = scan_tree(&["docs/[slug].html"]).expect("scan");
        assert!(
            routes.is_empty(),
            "dynamic .html page must stay skipped: {routes:?}"
        );
        assert!(
            logs_contain("dynamic .md / .html page routes are not supported"),
            "expected the dynamic .md/.html v1 warning"
        );
    }

    #[test]
    fn dynamic_ts_route_is_routed_like_tsx() {
        // `.ts`/`.js`/`.jsx` CAN carry a top-level `paths()` export (unlike a
        // pure `.md`/`.html` file), so the dynamic/catchall carve-out above
        // must NOT extend to them — they stay eligible for dynamic and
        // catchall routes exactly like `.tsx` (epic #1990's explicit
        // decision: do not over-apply the `.md`/`.html` restriction).
        let routes = scan_tree(&["blog/[slug].ts"]).expect("scan");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert_eq!(templates, vec!["/blog/:slug".to_string()]);
        assert_eq!(routes[0].kind, RouteKind::Dynamic);
    }

    #[test]
    fn ts_page_behaves_exactly_like_tsx_page() {
        // `pages/[slug].ts` must behave exactly like `pages/[slug].tsx`
        // (epic #1990 acceptance criterion): same template, same route
        // kind, for both a static and a dynamic shape.
        let tsx_static = scan_tree(&["about.tsx"]).expect("scan");
        let ts_static = scan_tree(&["about.ts"]).expect("scan");
        assert_eq!(
            tsx_static.iter().map(Route::template).collect::<Vec<_>>(),
            ts_static.iter().map(Route::template).collect::<Vec<_>>(),
        );
        assert_eq!(tsx_static[0].kind, ts_static[0].kind);

        let tsx_dynamic = scan_tree(&["blog/[slug].tsx"]).expect("scan");
        let ts_dynamic = scan_tree(&["blog/[slug].ts"]).expect("scan");
        assert_eq!(
            tsx_dynamic.iter().map(Route::template).collect::<Vec<_>>(),
            ts_dynamic.iter().map(Route::template).collect::<Vec<_>>(),
        );
        assert_eq!(tsx_dynamic[0].kind, ts_dynamic[0].kind);
    }

    // -----------------------------------------------------------------
    // Sidecar skips (epic #1990 review fix) — widening the allowlist to
    // `.ts`/`.js`/`.jsx` newly swept conventional non-page sidecars into
    // `pages/`. See `zfb_types::is_page_sidecar_file`.
    // -----------------------------------------------------------------

    #[test]
    fn colocated_test_beside_a_page_does_not_cause_ambiguous_route() {
        // The build-breaking regression: `pages/index.test.ts` beside
        // `pages/index.tsx` both strip to the `index` marker, so before the
        // skip they collided as `/` and hard-failed the whole build.
        let routes = scan_tree(&["index.tsx", "index.test.ts"]).expect("scan must not error");
        let templates: Vec<String> = routes.iter().map(Route::template).collect();
        assert_eq!(templates, vec!["/".to_string()]);
    }

    #[test]
    fn colocated_tests_and_specs_are_skipped_across_script_extensions() {
        for ext in zfb_types::SCRIPT_PAGE_EXTENSIONS {
            for infix in ["test", "spec"] {
                let file = format!("widget.{infix}.{ext}");
                let routes = scan_tree(&[&file]).expect("scan");
                assert!(
                    routes.is_empty(),
                    "{file} must not produce a route, got: {:?}",
                    routes.iter().map(Route::template).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn declaration_files_do_not_route() {
        // Would otherwise route as `/env.d` with `output_extension =
        // Some("d")` via the widened script-page sidecar convention.
        let routes = scan_tree(&["env.d.ts"]).expect("scan");
        assert!(
            routes.is_empty(),
            "env.d.ts must not route, got: {:?}",
            routes.iter().map(Route::template).collect::<Vec<_>>()
        );
    }

    #[test]
    fn genuine_plain_ts_page_still_routes() {
        // Guard against over-exclusion: only the two universal sidecar
        // conventions are skipped, not `.ts` pages in general.
        let routes = scan_tree(&["plain.ts", "index.tsx", "test.md", "d.ts"]).expect("scan");
        let mut templates: Vec<String> = routes.iter().map(Route::template).collect();
        templates.sort();
        assert_eq!(
            templates,
            vec![
                "/".to_string(),
                "/d".to_string(),
                "/plain".to_string(),
                "/test".to_string(),
            ]
        );
    }

    #[test]
    #[tracing_test::traced_test]
    fn sidecar_skip_does_not_emit_the_unrecognised_extension_warning() {
        // These extensions ARE recognised — the files are just not pages.
        // Emitting the "unrecognised extension" warning would be actively
        // misleading, and the epic's own e2e asserts that warning never
        // fires for a valid fixture.
        let routes = scan_tree(&["index.tsx", "index.test.ts", "env.d.ts"]).expect("scan");
        assert_eq!(routes.len(), 1);
        assert!(
            !logs_contain("unrecognised extension"),
            "sidecar skips must be silent, not warn about an unrecognised extension"
        );
    }
}
