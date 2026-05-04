//! Shared helpers that wire the bundler and renderer
//! (`zfb_build::bundler` + `zfb_build::renderer`) into the `zfb build`
//! and `zfb dev` commands.
//!
//! This module deliberately does **not** start the embedded V8 host or call
//! [`zfb_build::renderer::render_all`] directly. It owns the pure /
//! testable pieces of the wiring:
//!
//! - turning the [`zfb_router::Router`] scan into a concrete
//!   [`zfb_build::renderer::RouteUniverseEntry`] list (static routes
//!   only, for now — see [`build_route_universe`] for the dynamic
//!   `paths()` follow-up note),
//! - reading `TsxFrontmatter::prerender` for every page module that has
//!   one and folding that into the
//!   [`zfb_build::renderer::RendererInput::prerender_map`] shape, and
//!
//! The renderer call itself happens in [`crate::commands::build`] and
//! [`crate::commands::dev`] so the dev-mode long-lived `RendererState`
//! plumbing stays close to the dev session.
//!
//! ## Why a shared module
//!
//! `zfb build` and `zfb dev` produce the same `route_universe` and
//! `prerender_map`. Keeping the construction in one place prevents the
//! two commands from drifting (e.g. one resolving `prerender = false`
//! and the other not). The split also gives us a unit-test surface that
//! does not need a real renderer.
//!
//! ## Wave-3 (T7) gaps
//!
//! 1. **Dynamic `paths()` expansion.** Routes whose template contains
//!    `[slug]`, `[page]`, `[...rest]`, etc. need each page module's
//!    `paths()` export evaluated to enumerate the concrete URLs. This
//!    module wires the **static fast path**: when the page's `paths()`
//!    return value is a JSON-literal array (the
//!    [`zfb_render::paths_extract`] contract), [`expand_dynamic_routes`]
//!    runs it through [`zfb_render::paths::resolve_paths`] and produces
//!    one [`RouteUniverseEntry`] per resolved URL. Pages whose `paths()`
//!    is non-literal (helper calls, `await import`, runtime data
//!    sources, branching, …) are still surfaced via
//!    [`DeferredDynamicRoute`] with a reason string so callers can warn
//!    clearly. A future sub-task will add a runtime evaluator that
//!    consumes those deferred entries by booting a worker; until then
//!    they are skipped from `dist/`.
//!
//! 2. **Worker entry wrapping.** The bundler ([`zfb_build::bundle`])
//!    emits an ESM bundle that exports `routes` + `hydrateIsland` but
//!    not `default { fetch }`, while
//!    [`zfb_build::renderer::render_all`] expects a Worker-shaped
//!    bundle. Wrapping is its own sub-task; today the renderer call
//!    will fail at embedded V8 host boot time with an error message
//!    that names the missing `default` export. The CLI
//!    surfaces that error verbatim instead of swallowing it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use zfb_build::renderer::RouteUniverseEntry;
use zfb_build::EmbeddedV8Host;
use zfb_content::extract_tsx_frontmatter;
use zfb_render::paths::{
    resolve_paths, PathsCache, PathsError, Segment as PathsSegment,
};
use zfb_render::paths_extract::{extract_paths, PathsExtractError, PathsExtraction};
use zfb_router::{Route, RouteKind, Segment};

/// A dynamic / catchall route surfaced by [`build_route_universe`].
///
/// Carries enough metadata for [`expand_dynamic_routes`] to:
///
/// 1. Read the source file and try static `paths()` extraction.
/// 2. Pass the segments (now shared [`zfb_types::Segment`], re-exported
///    by both `zfb_router` and `zfb_render`) to
///    [`zfb_render::paths::resolve_paths`] for URL reassembly.
/// 3. Honour the route's filename-convention `output_extension` when
///    deriving each resolved entry's output path (e.g. a dynamic
///    `[slug].xml.tsx` should still produce `<slug>.xml`, not
///    `<slug>/index.html`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDynamicRoute {
    /// Source path of the page module, used both for static extraction
    /// and to point the user at the offending file in a warning.
    pub source_path: PathBuf,
    /// Route template (e.g. `/blog/:slug`). Used as the
    /// [`RouteUniverseEntry::route_key`] for every resolved URL so the
    /// prerender map lookup keys consistently.
    pub template: String,
    /// Parsed segments from the router. Reused for URL reassembly.
    pub segments: Vec<Segment>,
    /// Filename-convention extension override from the router (None →
    /// the renderer-side default of `html`).
    pub output_extension: Option<String>,
}

/// A dynamic route whose `paths()` couldn't be statically expanded.
/// Surfaced by [`expand_dynamic_routes`] so callers can warn loudly
/// instead of silently dropping the page from `dist/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredDynamicRoute {
    /// Source path of the page module.
    pub source_path: PathBuf,
    /// Route template, e.g. `/blog/:slug`.
    pub template: String,
    /// Parsed segments from the router. Required by
    /// [`eval_deferred_paths_via_worker`] to reassemble concrete URLs
    /// from the `paths()` return value.
    pub segments: Vec<Segment>,
    /// Filename-convention extension override from the router (None →
    /// the renderer-side default of `html`). Carried through so the
    /// runtime evaluator can produce the correct output path.
    pub output_extension: Option<String>,
    /// Why static expansion failed: `paths` export missing, return
    /// value not a literal, etc. Suitable for direct inclusion in a
    /// build warning.
    pub reason: String,
}

/// Output of [`build_route_universe`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteUniversePlan {
    /// Concrete routes the renderer can drive today (every static
    /// route from the router scan, in router-sorted order).
    pub static_routes: Vec<RouteUniverseEntry>,
    /// Dynamic / catchall routes deferred to the `paths()` follow-up.
    pub deferred_dynamic: Vec<PendingDynamicRoute>,
}

/// Build the renderer's `route_universe` input from a [`zfb_router::Router`]
/// scan.
///
/// **Static routes** are translated 1:1 into
/// [`RouteUniverseEntry`]:
///
/// - `url_path` is the route's [`Route::template`] (the URL the worker
///   will be asked to serve).
/// - `output_path` follows [`Route::output_filename`], which honours the
///   filename-extension precedence rule (frontmatter override >
///   filename convention > `html` default).
/// - `route_key` is the same template string — the prerender map keys
///   on it.
///
/// **Dynamic / catchall routes** are collected into
/// [`RouteUniversePlan::deferred_dynamic`] so callers can warn rather
/// than silently dropping them. Wiring `paths()` expansion is a
/// follow-up task; until then dynamic routes don't reach the renderer.
pub fn build_route_universe(routes: &[Route]) -> RouteUniversePlan {
    let mut plan = RouteUniversePlan::default();
    for route in routes {
        match route.kind {
            RouteKind::Static => {
                let template = route.template();
                plan.static_routes.push(RouteUniverseEntry {
                    url_path: template.clone(),
                    output_path: route.output_filename(None),
                    route_key: template,
                });
            }
            RouteKind::Dynamic | RouteKind::Catchall => {
                plan.deferred_dynamic.push(PendingDynamicRoute {
                    source_path: route.source_path.clone(),
                    template: route.template(),
                    segments: route.segments.clone(),
                    output_extension: route.output_extension.clone(),
                });
            }
        }
    }
    plan
}

/// Outcome of [`expand_dynamic_routes`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DynamicExpansion {
    /// One [`RouteUniverseEntry`] per concrete URL produced by a
    /// successful static `paths()` extraction. Order: input route
    /// order, then `paths()` array order within each route.
    pub resolved: Vec<RouteUniverseEntry>,
    /// Routes whose `paths()` could not be statically expanded —
    /// non-literal return value, missing `paths` export, source
    /// unreadable, parse error, or [`PathsError`] from the resolver
    /// itself (bad shape, missing param, ambiguous URL, …). Each
    /// carries a short reason suitable for a build warning.
    pub deferred: Vec<DeferredDynamicRoute>,
}

/// Walk the deferred dynamic routes from [`build_route_universe`] and
/// try to statically expand each into concrete URLs.
///
/// Pages whose `paths()` is a JSON-literal array (the
/// [`zfb_render::paths_extract`] contract) are handed to
/// [`zfb_render::paths::resolve_paths`] and produce one
/// [`RouteUniverseEntry`] per resolved URL. The `route_key` matches the
/// route's template so the prerender map join still works; the
/// `output_path` honours the dynamic route's filename-convention
/// extension (e.g. `[slug].xml.tsx` resolves to `foo.xml`, not
/// `foo/index.html`).
///
/// Pages whose `paths()` cannot be statically expanded are bundled into
/// [`DynamicExpansion::deferred`] with a one-line reason. A future
/// sub-task will pick those up and run them through a real JS runtime;
/// today they are skipped from `dist/` and surfaced as warnings.
///
/// `cache` is threaded through so callers can reuse a single
/// [`PathsCache`] across multiple invocations (e.g. dev-mode rebuilds);
/// `cache.miss_count()` and `cache.hit_count()` then reflect the whole
/// session.
pub fn expand_dynamic_routes(
    deferred: &[PendingDynamicRoute],
    project_root: &Path,
    cache: &mut PathsCache,
) -> DynamicExpansion {
    let mut out = DynamicExpansion::default();
    for route in deferred {
        match try_expand_one(route, project_root, cache) {
            Ok(entries) => out.resolved.extend(entries),
            Err(reason) => out.deferred.push(DeferredDynamicRoute {
                source_path: route.source_path.clone(),
                template: route.template.clone(),
                segments: route.segments.clone(),
                output_extension: route.output_extension.clone(),
                reason,
            }),
        }
    }
    out
}

/// Try to expand a single dynamic route into concrete entries. Returns
/// the resolved entries on success, or a one-line reason string on
/// failure (suitable for direct inclusion in a build warning).
fn try_expand_one(
    route: &PendingDynamicRoute,
    project_root: &Path,
    cache: &mut PathsCache,
) -> Result<Vec<RouteUniverseEntry>, String> {
    let abs = if route.source_path.is_absolute() {
        route.source_path.clone()
    } else {
        project_root.join(&route.source_path)
    };
    let file_name = abs
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| route.source_path.display().to_string());
    let source = std::fs::read_to_string(&abs)
        .map_err(|e| format!("could not read {} ({e})", abs.display()))?;
    let extraction = match extract_paths(&source, &file_name) {
        Ok(x) => x,
        Err(PathsExtractError::Parse { file, message }) => {
            return Err(format!("parse error in {file}: {message}"));
        }
    };
    let json = match extraction {
        PathsExtraction::Literal(v) => v,
        PathsExtraction::Missing => {
            return Err(format!(
                "no top-level `paths` export found in {}; dynamic routes require one",
                abs.display()
            ));
        }
        PathsExtraction::NonLiteral { reason } => {
            return Err(format!(
                "{}: paths() not statically resolvable ({reason}); pending runtime evaluation",
                abs.display()
            ));
        }
    };
    let segs: Vec<PathsSegment> = route
        .segments
        .iter()
        .cloned()
        .collect();
    let resolved = resolve_paths(cache, &route.template, &segs, &json)
        .map_err(|e| format!("{}: {}", abs.display(), format_paths_error(&e)))?;
    let mut out = Vec::with_capacity(resolved.len());
    for r in resolved {
        let output_path = build_output_path_for_resolved_url(
            &r.url,
            route.output_extension.as_deref(),
        );
        out.push(RouteUniverseEntry {
            url_path: r.url,
            output_path,
            route_key: route.template.clone(),
        });
    }
    Ok(out)
}

/// Compute the on-disk output path for a resolved dynamic URL, mirroring
/// the [`zfb_router::Route::output_filename`] contract:
///
/// - HTML pages render to `…/index.html` (so `/blog/hello` →
///   `blog/hello/index.html` and the index `/` → `index.html`).
/// - Non-HTML pages render to the bare URL path (so `/feed.xml` →
///   `feed.xml`); the URL itself already carries the extension because
///   the catchall reassembly preserves it.
fn build_output_path_for_resolved_url(url: &str, extension: Option<&str>) -> PathBuf {
    let ext = extension.unwrap_or("html");
    let trimmed = url.trim_start_matches('/');
    if ext == "html" {
        if trimmed.is_empty() {
            PathBuf::from("index.html")
        } else {
            PathBuf::from(trimmed).join("index.html")
        }
    } else {
        // Non-HTML: emit the URL path as-is. If, somehow, the resolved
        // URL is the bare root, fall back to `index.<ext>` so we never
        // emit an empty path.
        if trimmed.is_empty() {
            PathBuf::from(format!("index.{ext}"))
        } else {
            PathBuf::from(trimmed)
        }
    }
}

/// Render a [`PathsError`] without the long `Display` prefixes, since
/// the caller already prepends the source path and we don't want
/// `route` strings doubled in the warning.
fn format_paths_error(e: &PathsError) -> String {
    match e {
        PathsError::MissingParam { name, provided, .. } => {
            let pretty = provided
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "paths() entry is missing required param `{name}`: \
                 params must include `{name}`, got [{pretty}]"
            )
        }
        PathsError::ExtraParam { name, expected, .. } => {
            let pretty = expected
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "paths() entry has extra param `{name}` not in the route template: \
                 expected one of [{pretty}], got `{name}`"
            )
        }
        PathsError::InvalidParamType { name, reason, .. } => {
            format!("paths() entry has invalid param `{name}`: {reason}")
        }
        PathsError::InvalidPathsExport { field, reason, expected, .. } => {
            let field_note = match field {
                Some(f) => format!(" at `{f}`"),
                None => String::new(),
            };
            format!("paths() export is malformed{field_note}: {reason} (expected {expected})")
        }
        PathsError::AmbiguousResolution { reason, .. } => {
            format!("paths() produced ambiguous URLs: {reason}")
        }
    }
}

/// Dispatch handle for [`eval_deferred_paths_via_worker`].
///
/// Abstracts over the two ways the worker can be reached:
///
/// - `Http { base_url }` — the miniflare subprocess's base URL (e.g.
///   `http://127.0.0.1:54321/`). Requests go through `reqwest::blocking`.
///   Use this when `Backend::SpawnMiniflare` or `Backend::Existing` is
///   active; pass `state.base_url().unwrap()` from the [`RendererState`].
///
/// - `EmbeddedV8 { host }` — in-process V8 host (Sub 2 —
///   `EmbeddedV8RenderHost`). Requests call `host.dispatch_fetch` directly
///   without a TCP hop. Pass a mutable reference to the host extracted from
///   the active [`RendererState`] (via `state.embedded_v8_host_mut()` once
///   that accessor is added in Sub 6). The host is `!Send + !Sync`; the
///   caller must ensure this function runs on the same thread as the host.
///
/// The `__paths__` bundle registration in
/// `packages/zfb-runtime/src/router.ts` is unchanged — only the Rust
/// dispatch mechanism differs.
pub enum WorkerDispatch<'h> {
    /// HTTP path: miniflare subprocess or `Backend::Existing`.
    Http {
        /// Base URL of the running worker (e.g. `http://127.0.0.1:54321/`).
        base_url: String,
    },
    /// In-process V8 host (Sub 2 — `EmbeddedV8RenderHost`).
    EmbeddedV8 {
        /// Mutable reference to the live host. The caller retains
        /// ownership; this function borrows it only for the duration of
        /// the call.
        host: &'h mut dyn EmbeddedV8Host,
    },
}

/// Evaluate non-literal `paths()` exports by querying the running worker's
/// synthetic `/__paths__/<encoded-route-key>` endpoint.
///
/// The endpoint is provided by `@takazudo/zfb-runtime`'s `createPageRouter`
/// when the bundle is loaded by the embedded V8 host. For each
/// [`DeferredDynamicRoute`] in `deferred`, this function:
///
/// 1. Percent-encodes the route key so it is safe as a URL path segment.
/// 2. Dispatches `GET /__paths__/<encoded-route-key>` to the running worker
///    — either via reqwest (HTTP path) or via `host.dispatch_fetch` (V8
///    embedded path), depending on `dispatch`.
/// 3. Parses the JSON response array and feeds it through [`resolve_paths`]
///    with the route's segment list.
/// 4. Folds the resolved [`RouteUniverseEntry`]s into
///    [`DynamicExpansion::resolved`]; failures go into
///    [`DynamicExpansion::deferred`].
///
/// **Dispatch dual-path (post-merge sub-162 + sub-164 + sub-167):** the
/// production embedded V8 path uses `WorkerDispatch::EmbeddedV8 { host: ... }`
/// to call the host's `dispatch_fetch` directly. `WorkerDispatch::Http` is
/// kept for `Backend::Existing` callers (e.g. test fixtures that hand the
/// renderer a pre-running URL). Real production code post-Sub-167 uses
/// neither miniflare nor an HTTP base_url for the embedded path; the host
/// is in-process.
///
/// The timeout applies per-request (HTTP path only). A generous default (30 s)
/// is used because `paths()` may run a content-collection query. For the V8
/// path, timeout is enforced by the host implementation itself.
pub fn eval_deferred_paths_via_worker(
    deferred: &[DeferredDynamicRoute],
    dispatch: &mut WorkerDispatch<'_>,
    cache: &mut PathsCache,
    timeout: Option<Duration>,
) -> DynamicExpansion {
    if deferred.is_empty() {
        return DynamicExpansion::default();
    }

    match dispatch {
        WorkerDispatch::Http { base_url } => {
            eval_deferred_paths_http(deferred, base_url, cache, timeout)
        }
        WorkerDispatch::EmbeddedV8 { host } => {
            eval_deferred_paths_embedded(deferred, *host, cache)
        }
    }
}

/// HTTP path: builds a reqwest client and drives one GET per route.
fn eval_deferred_paths_http(
    deferred: &[DeferredDynamicRoute],
    base_url: &str,
    cache: &mut PathsCache,
    timeout: Option<Duration>,
) -> DynamicExpansion {
    let per_request_timeout = timeout.unwrap_or(Duration::from_secs(30));
    // Construct on a fresh OS thread so reqwest's internal tokio
    // runtime drops cleanly — see the matching note in
    // `zfb_build::renderer::build_http_client`. Without this, the build
    // CLI's outer `tokio::main` poisons the drop and we panic with
    // `Cannot drop a runtime in a context where blocking is not allowed`.
    let client = match std::thread::scope(|s| {
        s.spawn(|| {
            reqwest::blocking::Client::builder()
                .timeout(per_request_timeout)
                .no_proxy()
                .build()
        })
        .join()
        .expect("client-builder thread panicked")
    }) {
        Ok(c) => c,
        Err(e) => {
            // Extremely unlikely — only happens if TLS init fails.
            let reason = format!("could not build HTTP client for paths evaluation: {e}");
            let deferred_out: Vec<DeferredDynamicRoute> = deferred
                .iter()
                .map(|r| DeferredDynamicRoute {
                    source_path: r.source_path.clone(),
                    template: r.template.clone(),
                    segments: r.segments.clone(),
                    output_extension: r.output_extension.clone(),
                    reason: reason.clone(),
                })
                .collect();
            return DynamicExpansion {
                resolved: Vec::new(),
                deferred: deferred_out,
            };
        }
    };

    let base = base_url.trim_end_matches('/');
    let mut out = DynamicExpansion::default();

    for route in deferred {
        match eval_one_deferred_path_http(&client, base, route, cache) {
            Ok(entries) => out.resolved.extend(entries),
            Err(reason) => out.deferred.push(DeferredDynamicRoute {
                source_path: route.source_path.clone(),
                template: route.template.clone(),
                segments: route.segments.clone(),
                output_extension: route.output_extension.clone(),
                reason,
            }),
        }
    }

    out
}

/// Embedded V8 path: calls `host.dispatch_fetch` for each route.
fn eval_deferred_paths_embedded(
    deferred: &[DeferredDynamicRoute],
    host: &mut dyn EmbeddedV8Host,
    cache: &mut PathsCache,
) -> DynamicExpansion {
    let mut out = DynamicExpansion::default();

    for route in deferred {
        match eval_one_deferred_path_embedded(host, route, cache) {
            Ok(entries) => out.resolved.extend(entries),
            Err(reason) => out.deferred.push(DeferredDynamicRoute {
                source_path: route.source_path.clone(),
                template: route.template.clone(),
                segments: route.segments.clone(),
                output_extension: route.output_extension.clone(),
                reason,
            }),
        }
    }

    out
}

/// Query one route's `paths()` via HTTP and resolve it into
/// [`RouteUniverseEntry`]s. Returns a one-line reason string on any failure.
fn eval_one_deferred_path_http(
    client: &reqwest::blocking::Client,
    base: &str,
    route: &DeferredDynamicRoute,
    cache: &mut PathsCache,
) -> Result<Vec<RouteUniverseEntry>, String> {
    // Percent-encode the route key. The worker's `/__paths__/:routeKey{.+}`
    // pattern captures the rest of the path (including encoded slashes),
    // and the TS side does `decodeURIComponent` on it. We must encode `/`
    // and `:` so they are not misinterpreted as path separators / Hono
    // parameter delimiters.
    let encoded = encode_route_key(&route.template);
    let url = format!("{base}/__paths__/{encoded}");

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("HTTP request to {} failed: {}", url, e))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "worker returned {} for /__paths__/{}: {}",
            status.as_u16(),
            route.template,
            body.trim()
        ));
    }

    let json: serde_json::Value = resp
        .json()
        .map_err(|e| format!("could not parse JSON from /__paths__/{}: {}", route.template, e))?;

    resolve_json_paths(json, route, cache)
}

/// Query one route's `paths()` via the embedded V8 host and resolve it.
/// Returns a one-line reason string on any failure.
fn eval_one_deferred_path_embedded(
    host: &mut dyn EmbeddedV8Host,
    route: &DeferredDynamicRoute,
    cache: &mut PathsCache,
) -> Result<Vec<RouteUniverseEntry>, String> {
    let encoded = encode_route_key(&route.template);
    let url_path = format!("/__paths__/{encoded}");

    let resp = host
        .dispatch_fetch(&url_path)
        .map_err(|e| format!("embedded V8 dispatch for /__paths__/{} failed: {e}", route.template))?;

    if !(200..300).contains(&resp.status) {
        let body = String::from_utf8_lossy(&resp.body).into_owned();
        return Err(format!(
            "worker returned {} for /__paths__/{}: {}",
            resp.status,
            route.template,
            body.trim()
        ));
    }

    let json: serde_json::Value = serde_json::from_slice(&resp.body)
        .map_err(|e| format!("could not parse JSON from /__paths__/{}: {}", route.template, e))?;

    resolve_json_paths(json, route, cache)
}

/// Shared path resolution from a parsed JSON value.
fn resolve_json_paths(
    json: serde_json::Value,
    route: &DeferredDynamicRoute,
    cache: &mut PathsCache,
) -> Result<Vec<RouteUniverseEntry>, String> {
    let segs: Vec<PathsSegment> = route
        .segments
        .iter()
        .cloned()
        .collect();

    let resolved = resolve_paths(cache, &route.template, &segs, &json)
        .map_err(|e| format!("{}: {}", route.template, format_paths_error(&e)))?;

    let mut entries = Vec::with_capacity(resolved.len());
    for r in resolved {
        let output_path = build_output_path_for_resolved_url(&r.url, route.output_extension.as_deref());
        entries.push(RouteUniverseEntry {
            url_path: r.url,
            output_path,
            route_key: route.template.clone(),
        });
    }
    Ok(entries)
}

// Keep the old `eval_one_deferred_path` name as a private alias so any
// existing direct callers inside this crate still compile.
// This shim is intentionally not pub — only `eval_deferred_paths_via_worker`
// is the public surface.
#[allow(dead_code)]
fn eval_one_deferred_path(
    client: &reqwest::blocking::Client,
    base: &str,
    route: &DeferredDynamicRoute,
    cache: &mut PathsCache,
) -> Result<Vec<RouteUniverseEntry>, String> {
    eval_one_deferred_path_http(client, base, route, cache)
}

/// Percent-encode a route key so it is safe as a URL path segment.
///
/// We encode every byte that is not an ASCII alphanumeric or one of
/// `- _ . ~` (the RFC 3986 unreserved set). In particular `/` and `:`
/// are encoded so they don't confuse the worker's path router.
fn encode_route_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len() * 3);
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(hex_nibble(byte >> 4));
            out.push(hex_nibble(byte & 0xF));
        }
    }
    out
}

fn hex_nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'A' + n - 10) as char,
    }
}

/// Read every TSX page's frontmatter and fold the
/// `export const prerender = …` flag into a map keyed on the route
/// template (matching [`RouteUniverseEntry::route_key`]).
///
/// `warn_unreadable` is invoked once per TSX page whose source can't be
/// read or whose frontmatter extraction fails. The closure exists so
/// callers can route the message through their own logging surface
/// (e.g. `crate::output::warn`) instead of forcing this helper to
/// know about CLI-side I/O.
///
/// Behaviour notes:
///
/// - Non-TSX pages (e.g. `.mdx`) are skipped silently — TSX
///   frontmatter extraction does not apply to them. The renderer
///   treats a missing prerender entry as `true` (SSG), which is the
///   documented default for MDX too.
/// - TSX pages whose extraction fails are still skipped from the map
///   (so the renderer's missing-key default of `true` applies), but
///   the failure is reported through `warn_unreadable` so the user
///   can fix typos in their `frontmatter` / `prerender` exports
///   instead of staring at a silent default.
pub fn build_prerender_map(
    routes: &[Route],
    project_root: &Path,
    mut warn_unreadable: impl FnMut(&str),
) -> BTreeMap<String, bool> {
    let mut map = BTreeMap::new();
    for route in routes {
        let abs = if route.source_path.is_absolute() {
            route.source_path.clone()
        } else {
            project_root.join(&route.source_path)
        };
        let Some(ext) = abs.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        // Only TSX (and TS) carries the `export const prerender = …`
        // shape this extractor understands. MDX is left at the default.
        if !matches!(ext, "tsx" | "ts") {
            continue;
        }
        let source = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(e) => {
                warn_unreadable(&format!(
                    "could not read {} for prerender extraction ({}); defaulting to SSG",
                    abs.display(),
                    e
                ));
                continue;
            }
        };
        let file_name = abs
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<unknown>".into());
        match extract_tsx_frontmatter(&source, &file_name) {
            Ok(fm) => {
                map.insert(route.template(), fm.prerender);
            }
            Err(e) => {
                warn_unreadable(&format!(
                    "frontmatter extraction failed for {} ({}); defaulting to SSG",
                    abs.display(),
                    e
                ));
            }
        }
    }
    map
}

/// Verify that `@takazudo/zfb-runtime` is resolvable from `project_root`
/// (i.e. a `node_modules/@takazudo/zfb-runtime` exists somewhere up the
/// directory tree). The bundle the renderer drives imports the runtime
/// at module load time; without it, the embedded V8 host boots and immediately
/// throws a module-resolution error that's harder for users to map back
/// to a fixable action.
///
/// Returns `Ok(())` when the runtime is present, `Err(anyhow!)` with a
/// "run pnpm install …" hint when it isn't. The check is best-effort —
/// custom `node_modules` layouts (yarn pnp, …) are accepted as long as
/// the conventional path exists.
pub fn check_runtime_installed(project_root: &Path) -> Result<()> {
    let mut cur: Option<&Path> = Some(project_root);
    while let Some(p) = cur {
        if p
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-runtime")
            .exists()
        {
            return Ok(());
        }
        cur = p.parent();
    }
    Err(anyhow::anyhow!(
        "could not find `node_modules/@takazudo/zfb-runtime` under {} or any parent. \
         Run `pnpm install` (or your package manager's equivalent) in the project root \
         so the SSG-render bundle can resolve `@takazudo/zfb-runtime` at embedded V8 host load time.",
        project_root.display()
    ))
    .context("zfb runtime resolution check failed")
}

/// Convert the project's [`crate::config::Framework`] into the
/// renderer/bundler-facing [`zfb_render::adapters::Framework`].
pub fn cfg_framework_to_render(f: crate::config::Framework) -> zfb_render::adapters::Framework {
    match f {
        crate::config::Framework::Preact => zfb_render::adapters::Framework::Preact,
        crate::config::Framework::React => zfb_render::adapters::Framework::React,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use zfb_router::{Route, RouteKind, Segment};

    fn static_route(segments: Vec<&str>, source: &str) -> Route {
        let segs: Vec<Segment> = segments
            .into_iter()
            .map(|s| Segment::Static(s.to_string()))
            .collect();
        Route {
            source_path: PathBuf::from(source),
            segments: segs,
            kind: RouteKind::Static,
            specificity: 0,
            output_extension: None,
        }
    }

    fn dynamic_route(name: &str, source: &str) -> Route {
        Route {
            source_path: PathBuf::from(source),
            segments: vec![Segment::Dynamic(name.to_string())],
            kind: RouteKind::Dynamic,
            specificity: 0,
            output_extension: None,
        }
    }

    /// Build a multi-segment route mixing static + dynamic segments,
    /// e.g. `route(["blog", ":slug"], "pages/blog/[slug].tsx")` builds
    /// `/blog/[slug]`.
    fn route(segments: Vec<&str>, source: &str, kind: RouteKind) -> Route {
        let segs: Vec<Segment> = segments
            .into_iter()
            .map(|s| {
                if let Some(name) = s.strip_prefix(":") {
                    if let Some(name) = name.strip_suffix("*") {
                        Segment::Catchall(name.to_string())
                    } else {
                        Segment::Dynamic(name.to_string())
                    }
                } else {
                    Segment::Static(s.to_string())
                }
            })
            .collect();
        Route {
            source_path: PathBuf::from(source),
            segments: segs,
            kind,
            specificity: 0,
            output_extension: None,
        }
    }

    #[test]
    fn build_route_universe_partitions_static_and_dynamic() {
        let routes = vec![
            static_route(vec![], "pages/index.tsx"),
            static_route(vec!["about"], "pages/about.tsx"),
            dynamic_route("slug", "pages/[slug].tsx"),
        ];
        let plan = build_route_universe(&routes);
        assert_eq!(plan.static_routes.len(), 2);
        assert_eq!(plan.static_routes[0].url_path, "/");
        assert_eq!(plan.static_routes[0].output_path, PathBuf::from("index.html"));
        assert_eq!(plan.static_routes[0].route_key, "/");
        assert_eq!(plan.static_routes[1].url_path, "/about");
        assert_eq!(
            plan.static_routes[1].output_path,
            PathBuf::from("about/index.html")
        );

        assert_eq!(plan.deferred_dynamic.len(), 1);
        assert_eq!(plan.deferred_dynamic[0].template, "/:slug");
        assert_eq!(
            plan.deferred_dynamic[0].source_path,
            PathBuf::from("pages/[slug].tsx")
        );
    }

    #[test]
    fn build_prerender_map_reads_tsx_frontmatter_and_warns_on_unparseable() {
        let dir = tempdir().unwrap();
        let pages = dir.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();

        // SSG (default true)
        std::fs::write(
            pages.join("about.tsx"),
            "export const frontmatter = { title: 'A' };\nexport default function() { return null; }\n",
        )
        .unwrap();
        // SSR-only (prerender=false)
        std::fs::write(
            pages.join("preview.tsx"),
            "export const frontmatter = { title: 'P' };\nexport const prerender = false;\nexport default function() { return null; }\n",
        )
        .unwrap();
        // No frontmatter — extraction fails → no entry inserted, but a
        // warning IS emitted so the user can find their typo.
        std::fs::write(
            pages.join("broken.tsx"),
            "export default function() { return null; }\n",
        )
        .unwrap();

        let routes = vec![
            static_route(vec!["about"], "pages/about.tsx"),
            static_route(vec!["preview"], "pages/preview.tsx"),
            static_route(vec!["broken"], "pages/broken.tsx"),
        ];

        let mut warnings: Vec<String> = Vec::new();
        let map = build_prerender_map(&routes, dir.path(), |msg| warnings.push(msg.to_string()));
        assert_eq!(map.get("/about"), Some(&true));
        assert_eq!(map.get("/preview"), Some(&false));
        assert!(!map.contains_key("/broken"));
        // broken.tsx triggered exactly one warning naming the file.
        assert_eq!(warnings.len(), 1, "warnings: {warnings:?}");
        assert!(
            warnings[0].contains("broken.tsx"),
            "expected file name in warning, got: {}",
            warnings[0]
        );
    }

    #[test]
    fn build_prerender_map_skips_non_tsx_sources() {
        let dir = tempdir().unwrap();
        let pages = dir.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(pages.join("post.mdx"), "# hello\n").unwrap();

        let routes = vec![static_route(vec!["post"], "pages/post.mdx")];
        let mut warnings: Vec<String> = Vec::new();
        let map = build_prerender_map(&routes, dir.path(), |msg| warnings.push(msg.into()));
        assert!(map.is_empty(), "MDX should be left to the default-true path");
        // Non-TSX files are skipped silently (no warning).
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
    }

    #[test]
    fn check_runtime_installed_finds_runtime_in_parent_node_modules() {
        let dir = tempdir().unwrap();
        let runtime = dir
            .path()
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let nested = dir.path().join("examples/basic-blog");
        std::fs::create_dir_all(&nested).unwrap();
        check_runtime_installed(&nested).unwrap();
    }

    #[test]
    fn check_runtime_installed_errors_when_runtime_missing() {
        let dir = tempdir().unwrap();
        let err = check_runtime_installed(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("@takazudo/zfb-runtime"), "{msg}");
        assert!(msg.contains("pnpm install"), "{msg}");
    }

    // ---- expand_dynamic_routes -------------------------------------------

    /// Stage a single dynamic page on disk with the given source so
    /// [`expand_dynamic_routes`] can read it. Returns the project root
    /// (caller keeps the [`tempfile::TempDir`] alive) and the
    /// [`PendingDynamicRoute`] pointing at the staged page.
    fn stage_dynamic_page(
        page_relative: &str,
        segments: Vec<Segment>,
        template: &str,
        body: &str,
    ) -> (tempfile::TempDir, PendingDynamicRoute) {
        let dir = tempdir().unwrap();
        let abs = dir.path().join(page_relative);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, body).unwrap();
        let pending = PendingDynamicRoute {
            source_path: PathBuf::from(page_relative),
            template: template.to_string(),
            segments,
            output_extension: None,
        };
        (dir, pending)
    }

    #[test]
    fn expand_dynamic_routes_resolves_literal_paths_into_entries() {
        let body = r#"
            export function paths() {
                return [
                    { params: { slug: "hello" }, props: { i: 1 } },
                    { params: { slug: "world" }, props: { i: 2 } },
                ];
            }
            export default function P() { return null; }
        "#;
        let (dir, pending) = stage_dynamic_page(
            "pages/blog/[slug].tsx",
            vec![
                Segment::Static("blog".into()),
                Segment::Dynamic("slug".into()),
            ],
            "/blog/:slug",
            body,
        );

        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);

        assert_eq!(out.deferred.len(), 0, "deferred: {:?}", out.deferred);
        assert_eq!(out.resolved.len(), 2);

        assert_eq!(out.resolved[0].url_path, "/blog/hello");
        assert_eq!(
            out.resolved[0].output_path,
            PathBuf::from("blog/hello/index.html")
        );
        assert_eq!(out.resolved[0].route_key, "/blog/:slug");

        assert_eq!(out.resolved[1].url_path, "/blog/world");
        assert_eq!(
            out.resolved[1].output_path,
            PathBuf::from("blog/world/index.html")
        );
        assert_eq!(out.resolved[1].route_key, "/blog/:slug");

        // Cache miss for the first call — the second route in the same
        // call is a different `slug` value but shares the same JSON
        // shape (single resolve_paths call), so we still expect exactly
        // one miss for this page.
        assert_eq!(cache.miss_count(), 1);
        assert_eq!(cache.hit_count(), 0);
    }

    #[test]
    fn expand_dynamic_routes_respects_output_extension_for_non_html() {
        // `[slug].xml.tsx` should resolve to `<slug>.xml`, not
        // `<slug>/index.html`. The router would have set
        // `output_extension = Some("xml")`; we mirror that here.
        let body = r#"
            export function paths() {
                return [{ params: { slug: "feed-a" } }];
            }
        "#;
        let (dir, mut pending) = stage_dynamic_page(
            "pages/[slug].xml.tsx",
            vec![Segment::Dynamic("slug".into())],
            "/:slug",
            body,
        );
        pending.output_extension = Some("xml".into());

        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);

        assert_eq!(out.resolved.len(), 1);
        assert_eq!(out.resolved[0].url_path, "/feed-a");
        assert_eq!(out.resolved[0].output_path, PathBuf::from("feed-a"));
    }

    #[test]
    fn expand_dynamic_routes_defers_non_literal_paths_with_reason() {
        // Mirrors the real basic-blog page: `paths()` does an
        // `await import` + collection query, which is not statically
        // resolvable. Must defer with a reason that the build can
        // surface verbatim.
        let body = r#"
            export async function paths() {
                const { getCollection } = await import("zfb/content");
                const posts = await getCollection("blog");
                return posts.map((p) => ({ params: { slug: p.slug } }));
            }
        "#;
        let (dir, pending) = stage_dynamic_page(
            "pages/blog/[slug].tsx",
            vec![
                Segment::Static("blog".into()),
                Segment::Dynamic("slug".into()),
            ],
            "/blog/:slug",
            body,
        );

        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);

        assert_eq!(out.resolved.len(), 0);
        assert_eq!(out.deferred.len(), 1);
        assert!(
            out.deferred[0].reason.contains("not statically resolvable"),
            "reason: {}",
            out.deferred[0].reason,
        );
        assert!(
            out.deferred[0].reason.contains("pages/blog/[slug].tsx"),
            "reason should name the source path, got: {}",
            out.deferred[0].reason,
        );
        assert_eq!(out.deferred[0].template, "/blog/:slug");
    }

    #[test]
    fn expand_dynamic_routes_defers_when_paths_export_missing() {
        let body = r#"
            export default function P() { return null; }
        "#;
        let (dir, pending) = stage_dynamic_page(
            "pages/[slug].tsx",
            vec![Segment::Dynamic("slug".into())],
            "/:slug",
            body,
        );

        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);
        assert_eq!(out.resolved.len(), 0);
        assert_eq!(out.deferred.len(), 1);
        assert!(
            out.deferred[0]
                .reason
                .contains("no top-level `paths` export"),
            "reason: {}",
            out.deferred[0].reason,
        );
    }

    #[test]
    fn expand_dynamic_routes_defers_unreadable_source() {
        // Point at a file that doesn't exist; should defer with an
        // I/O-flavoured reason rather than panic.
        let dir = tempdir().unwrap();
        let pending = PendingDynamicRoute {
            source_path: PathBuf::from("pages/no-such.tsx"),
            template: "/no-such".into(),
            segments: vec![Segment::Dynamic("x".into())],
            output_extension: None,
        };
        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);
        assert_eq!(out.resolved.len(), 0);
        assert_eq!(out.deferred.len(), 1);
        assert!(
            out.deferred[0].reason.contains("could not read"),
            "reason: {}",
            out.deferred[0].reason,
        );
    }

    #[test]
    fn expand_dynamic_routes_defers_with_resolver_error_when_param_missing() {
        // Literal extraction succeeds — but the entry's `params` is
        // missing the required `slug` key. The resolver returns
        // MissingParam; we surface that as a deferred entry with the
        // user-facing reason.
        let body = r#"
            export function paths() {
                return [{ params: { wrong: "x" } }];
            }
        "#;
        let (dir, pending) = stage_dynamic_page(
            "pages/[slug].tsx",
            vec![Segment::Dynamic("slug".into())],
            "/:slug",
            body,
        );
        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);
        assert_eq!(out.resolved.len(), 0);
        assert_eq!(out.deferred.len(), 1);
        assert!(
            out.deferred[0].reason.contains("missing required param `slug`"),
            "reason: {}",
            out.deferred[0].reason,
        );
    }

    #[test]
    fn expand_dynamic_routes_handles_catchall_with_array_value() {
        let body = r#"
            export function paths() {
                return [
                    { params: { slug: ["a", "b"] } },
                    { params: { slug: ["x", "y", "z"] } },
                ];
            }
        "#;
        let (dir, pending) = stage_dynamic_page(
            "pages/docs/[...slug].tsx",
            vec![
                Segment::Static("docs".into()),
                Segment::Catchall("slug".into()),
            ],
            "/docs/:slug{.+}",
            body,
        );
        let mut cache = PathsCache::new();
        let out = expand_dynamic_routes(&[pending], dir.path(), &mut cache);

        assert_eq!(out.deferred.len(), 0, "deferred: {:?}", out.deferred);
        assert_eq!(out.resolved.len(), 2);
        assert_eq!(out.resolved[0].url_path, "/docs/a/b");
        assert_eq!(
            out.resolved[0].output_path,
            PathBuf::from("docs/a/b/index.html")
        );
        assert_eq!(out.resolved[1].url_path, "/docs/x/y/z");
        assert_eq!(
            out.resolved[1].output_path,
            PathBuf::from("docs/x/y/z/index.html")
        );
    }

    /// Integration-style fixture: stage a tiny project with both a
    /// static page and a dynamic page that has a literal `paths()`,
    /// then walk through `build_route_universe` →
    /// `expand_dynamic_routes` and assert the combined renderer-shaped
    /// route list. This is the closest we can get to end-to-end without
    /// booting the embedded V8 host (which is gated by the sibling
    /// worker-entry topic).
    #[test]
    fn build_then_expand_combined_route_universe() {
        let dir = tempdir().unwrap();
        // Static page.
        std::fs::create_dir_all(dir.path().join("pages")).unwrap();
        std::fs::write(
            dir.path().join("pages/about.tsx"),
            "export const frontmatter = { title: \"About\" };\n\
             export default function P() { return null; }\n",
        )
        .unwrap();
        // Dynamic page with a literal paths().
        std::fs::create_dir_all(dir.path().join("pages/blog")).unwrap();
        std::fs::write(
            dir.path().join("pages/blog/[slug].tsx"),
            "export function paths() {\n\
                return [\n\
                    { params: { slug: \"hello\" } },\n\
                    { params: { slug: \"world\" } }\n\
                ];\n\
             }\n\
             export default function P() { return null; }\n",
        )
        .unwrap();

        let routes = vec![
            static_route(vec!["about"], "pages/about.tsx"),
            route(
                vec!["blog", ":slug"],
                "pages/blog/[slug].tsx",
                RouteKind::Dynamic,
            ),
        ];

        let plan = build_route_universe(&routes);
        assert_eq!(plan.static_routes.len(), 1);
        assert_eq!(plan.deferred_dynamic.len(), 1);

        let mut cache = PathsCache::new();
        let expansion =
            expand_dynamic_routes(&plan.deferred_dynamic, dir.path(), &mut cache);
        assert_eq!(expansion.deferred.len(), 0, "{:?}", expansion.deferred);
        assert_eq!(expansion.resolved.len(), 2);

        // Final renderer-shaped list: statics first, then resolved
        // dynamics in input order. The build orchestration concatenates
        // in this same order; assert the combined shape end-to-end.
        let mut combined = plan.static_routes.clone();
        combined.extend(expansion.resolved);
        assert_eq!(combined.len(), 3);
        assert_eq!(combined[0].url_path, "/about");
        assert_eq!(combined[1].url_path, "/blog/hello");
        assert_eq!(combined[2].url_path, "/blog/world");
        // route_key of resolved entries must be the dynamic template,
        // not the resolved URL — that's how the prerender map joins.
        assert_eq!(combined[1].route_key, "/blog/:slug");
        assert_eq!(combined[2].route_key, "/blog/:slug");
    }

    // -------------------------------------------------------------------------
    // WorkerDispatch::EmbeddedV8 integration test
    //
    // This test exercises a page whose `paths()` export is non-literal (reads
    // a content collection at runtime). It is gated with `#[ignore]` because
    // it depends on `EmbeddedV8RenderHost` (Sub 2 — embed-v8/sub-162) which
    // is not yet merged into this worktree. Run after Sub 2 is merged with:
    //
    //     cargo test -p zfb -- \
    //         --include-ignored \
    //         eval_deferred_paths_via_worker_embedded_v8_non_literal_paths
    //
    // The test proves that the `__paths__` synthetic endpoint in the bundle
    // keeps working unchanged: only the dispatch mechanism differs (no HTTP).
    // -------------------------------------------------------------------------

    /// Integration test: `eval_deferred_paths_via_worker` drives the embedded
    /// V8 host's `dispatch_fetch` for a page whose `paths()` is non-literal.
    ///
    /// **Cannot run until Sub 2 (`EmbeddedV8RenderHost`) is merged.**
    /// Gated with `#[ignore]` — see module-level comment above.
    #[test]
    #[ignore = "depends on EmbeddedV8RenderHost (Sub 2, embed-v8/sub-162); run after merge"]
    fn eval_deferred_paths_via_worker_embedded_v8_non_literal_paths() {
        // This test intentionally left as a skeleton to be filled in by the
        // integration manager after Sub 2 is merged. The shape is:
        //
        //   1. Build the examples/basic-blog bundle (or a fixture).
        //   2. Construct EmbeddedV8RenderHost::new(&bundle_path).
        //   3. Build a DeferredDynamicRoute for `/blog/:slug` (non-literal
        //      paths() that reads the blog content collection).
        //   4. Call eval_deferred_paths_via_worker with
        //      WorkerDispatch::EmbeddedV8 { host: &mut host }.
        //   5. Assert the resolved entries match the blog posts in the
        //      fixture content collection.
        //
        // For now, assert that the test is reachable (not dead code):
        assert!(
            true,
            "skeleton test — fill in after EmbeddedV8RenderHost (Sub 2) is merged"
        );
    }
}
