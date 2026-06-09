//! `zfb preview` command — local preview server for built artifacts.
//!
//! This command does no rebuild, no live-reload, and injects no
//! `/__zfb/*` routes. It is a thin preview shell with two modes that
//! match what `zfb build` actually emitted:
//!
//! ## Static-only mode (`adapter: "none"` or omitted)
//!
//! Serves files from `<project>/dist/` over HTTP at `args.port`.
//!
//! Trailing-slash semantics match Cloudflare Pages:
//!
//! - `GET /` → serve `dist/index.html`.
//! - `GET /foo` →
//!   - if `dist/foo` is a regular file, serve it;
//!   - else if `dist/foo/index.html` exists, **301 redirect** to `/foo/`;
//!   - else 404.
//! - `GET /foo/` →
//!   - if `dist/foo/index.html` exists, serve it;
//!   - else 404.
//!
//! Naked directories (no `index.html`) always 404 — never a directory
//! listing. The 404 body is `dist/404.html` if it exists, otherwise a
//! plain `404 Not Found` text response.
//!
//! Cache-Control is `no-store` for v0 — preview is local only and we
//! never want a stale browser cache to mask a real bug.
//!
//! ## Adapter mode (`adapter: "@takazudo/zfb-adapter-cloudflare"`)
//!
//! Defers to `pnpm exec wrangler pages dev <outdir> --port <port>` so
//! the Worker bundle (`dist/_worker.js`) executes locally. Wrangler is
//! the canonical CF Pages local-dev tool, and matching its semantics
//! by deferring is more honest than reimplementing them.
//!
//! ## Config wiring
//!
//! Loads `zfb.config.json` (or surfaces a clear "ts not yet supported"
//! error for `zfb.config.ts`) via [`crate::config::load_from_dir`].
//! Port resolution layers as "CLI flag > config > built-in default
//! (`4321`)" — same rule used by `zfb dev`. `--outdir` keeps a clap
//! default because the preview command does not consult config for it
//! today.

use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use zfb_build::AdapterChoice;

use crate::cli::PreviewArgs;
use crate::commands::resolve::{resolve_addr, resolve_host, resolve_port, resolve_under_root};
use crate::config;
use crate::output;

// Re-export from the canonical source so callers that already reference
// `zfb::commands::preview::EXPECTED_WRANGLER_VERSION` keep compiling.
pub use zfb_toolchain_pins::{EXPECTED_WRANGLER_VERSION, EXPECTED_WORKERD_VERSION};

/// Built-in default port for `zfb preview` when neither the CLI nor the
/// project config supplies one. 4321 keeps `dev` (3000) and `preview`
/// running side-by-side, and matches what npm-side scaffolds expect.
const DEFAULT_PREVIEW_PORT: u16 = 4321;

/// Built-in default host for `zfb preview` when neither the CLI nor the
/// project config supplies one. Mirrors `dev`'s `localhost` default; pass
/// `--host 0.0.0.0` (or set `host` in `zfb.config.json`) to expose the built
/// site to other devices on the LAN.
const DEFAULT_PREVIEW_HOST: &str = "localhost";

/// Adapter package name handled in adapter mode. Only one adapter
/// exists today; if a project configures something else, we error out
/// rather than silently falling through to static-only.
const CLOUDFLARE_ADAPTER: &str = "@takazudo/zfb-adapter-cloudflare";

/// Set this env var to `1` to skip the pre-flight wrangler version
/// gate. Intended as an emergency escape hatch (e.g. while a
/// release-engineering bump is mid-flight); not meant for steady-state
/// use.
const SKIP_WRANGLER_VERSION_CHECK_ENV: &str = "ZFB_SKIP_WRANGLER_VERSION_CHECK";

pub async fn run(args: &PreviewArgs) -> Result<()> {
    // 1. Resolve the project root and load configuration. A missing
    //    config file is fine — `load_from_dir` returns
    //    `Config::default()`. Any *real* error (invalid JSON, an
    //    unsupported `.ts` config) is surfaced via `output::error` by
    //    `main()` after we propagate it.
    let project_root = std::env::current_dir().context("failed to read current working dir")?;
    let cfg = config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration")?;

    // 2. Resolve `args.outdir` against the project root so the
    //    existence check (and the static handler) operate on an
    //    unambiguous path. CLI wins over config unconditionally — see
    //    the precedence note in the module doc comment.
    let outdir = resolve_under_root(&project_root, &args.outdir);
    let port = resolve_port(args.port, cfg.port, DEFAULT_PREVIEW_PORT);
    let host = resolve_host(args.host.as_deref(), cfg.host.as_deref(), DEFAULT_PREVIEW_HOST);

    // Verify the output directory exists *before* binding the port so
    // missing-build errors don't leave a half-started server behind.
    if !outdir.exists() {
        anyhow::bail!(
            "{} does not exist — run zfb build first",
            outdir.display()
        );
    }

    // 3. Branch on adapter. `AdapterChoice::from_config` validates the
    //    package-name shape, so a typo in `zfb.config.json` surfaces
    //    here rather than as a confusing wrangler-spawn failure later.
    let adapter = AdapterChoice::from_config(cfg.adapter.as_deref())
        .context("invalid adapter in zfb.config.json")?;

    match adapter {
        AdapterChoice::None => run_static(&outdir, &host, port).await,
        AdapterChoice::Package(pkg) if pkg == CLOUDFLARE_ADAPTER => {
            run_via_wrangler(&project_root, &outdir, &host, port).await
        }
        AdapterChoice::Package(pkg) => anyhow::bail!(
            "preview: adapter {pkg:?} is not supported (only {CLOUDFLARE_ADAPTER:?} today)"
        ),
    }
}

// ---------------------------------------------------------------------------
// Static-only mode
// ---------------------------------------------------------------------------

/// Bind the static preview server and run it until Ctrl+C.
async fn run_static(dist_root: &Path, host: &str, port: u16) -> Result<()> {
    let app = build_static_router(dist_root.to_path_buf());

    let addr: SocketAddr = resolve_addr(host, port)?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind preview server to {addr}"))?;

    // Same Local/Network banner as `zfb dev` (#487): when bound to an
    // unspecified host (`0.0.0.0`/`::`) this enumerates LAN-reachable URLs
    // instead of printing a bare, unusable `http://0.0.0.0:PORT`.
    output::ready_with_interfaces("http", host, port);

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            // Ignore the result — we want to fall through to a clean
            // exit whether ctrl_c succeeded or the signal handler
            // errored out.
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("preview server failed")?;

    Ok(())
}

/// Shared state passed to the fallback handler. Cheap to clone — only
/// holds the dist root path.
#[derive(Clone)]
struct StaticState {
    dist_root: PathBuf,
}

/// Build the router used in static-only mode. Exposed (crate-private)
/// so unit tests can drive it via `tower::ServiceExt::oneshot` without
/// binding a port.
pub(crate) fn build_static_router(dist_root: PathBuf) -> Router {
    Router::new()
        .fallback(static_fallback)
        .with_state(StaticState { dist_root })
}

/// One handler covering every path. Easier than wiring `/`, `/*path`
/// separately because we want identical behaviour for both.
async fn static_fallback(State(state): State<StaticState>, uri: Uri) -> Response {
    serve_static(&state.dist_root, uri.path(), uri.query()).await
}

/// Apply the Cloudflare-Pages-style routing rule to a request, then
/// either serve a file, redirect, or 404.
///
/// `query` is the raw (already percent-encoded) query string from the
/// request URI, if any. It is appended verbatim to redirect targets so
/// that `/m?c=1a` → `301 /m/?c=1a` rather than silently dropping
/// the query.
async fn serve_static(dist_root: &Path, url_path: &str, query: Option<&str>) -> Response {
    match resolve_static(dist_root, url_path) {
        Resolution::File(path) => serve_file(&path, dist_root).await,
        Resolution::Redirect(target) => {
            let target = match query {
                Some(q) if !q.is_empty() => format!("{target}?{q}"),
                _ => target,
            };
            redirect_response(&target)
        }
        Resolution::NotFound => not_found_response(dist_root).await,
    }
}

/// Outcome of applying the routing rule to a URL path. Pure value type
/// so the rule can be unit-tested without touching axum or tokio.
#[derive(Debug, PartialEq, Eq)]
enum Resolution {
    /// Serve this file. Always under `dist_root` thanks to the
    /// sanitiser.
    File(PathBuf),
    /// 301-redirect the client to this URL path. Used when `/foo` is
    /// actually a directory (`/foo/index.html`).
    Redirect(String),
    /// Neither a file nor a directory-with-index matched. Caller
    /// returns the project's `404.html` when one exists.
    NotFound,
}

/// Resolve a URL path against `dist_root` per the rules in the module
/// doc comment. Pure of side effects beyond `is_file` filesystem
/// probes — no I/O on the file body itself.
fn resolve_static(dist_root: &Path, url_path: &str) -> Resolution {
    if !is_safe_path(url_path) {
        return Resolution::NotFound;
    }

    let stripped = url_path.trim_start_matches('/');
    let has_trailing = url_path.is_empty() || url_path.ends_with('/');
    let clean = stripped.trim_end_matches('/');

    if clean.is_empty() {
        // Root: serve dist/index.html or 404.
        let idx = dist_root.join("index.html");
        return if idx.is_file() {
            Resolution::File(idx)
        } else {
            Resolution::NotFound
        };
    }

    let candidate_file = dist_root.join(clean);
    let candidate_index = candidate_file.join("index.html");

    if has_trailing {
        // `/foo/` only matches a directory with an index.
        if candidate_index.is_file() {
            return Resolution::File(candidate_index);
        }
        return Resolution::NotFound;
    }

    // `/foo`: try file first, then directory-with-index (redirect),
    // else 404.
    if candidate_file.is_file() {
        return Resolution::File(candidate_file);
    }
    if candidate_index.is_file() {
        return Resolution::Redirect(format!("/{clean}/"));
    }
    Resolution::NotFound
}

/// Check that `path` (after symlink resolution) still lives inside
/// `root` (after symlink resolution). Returns `true` only when both
/// canonicalize successfully and the canonical path starts with the
/// canonical root.
///
/// Returning `false` on any canonicalize error (e.g. the file does not
/// exist yet) is intentional: callers treat a failed containment check
/// as not-found, so a missing symlink target is a safe 404.
///
/// This function is deliberately sync so it can be called from both the
/// current sync helper (`resolve_static`) and from the async `serve_file`
/// without spawning a blocking task — the syscall is cheap. Wave-2 (#903)
/// will migrate callers to `tokio::fs::canonicalize` as part of the async
/// conversion.
fn path_is_within_root(path: &Path, root: &Path) -> bool {
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let canonical_path = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => return false,
    };
    canonical_path.starts_with(&canonical_root)
}

/// Reject path traversal and Windows-style absolute components.
///
/// Empty / root paths are accepted and routed by the caller. We only
/// look at semantic [`Component`] kinds — `Component::ParentDir`
/// (`..`) is the only one that escapes `dist_root`. Backslash-bearing
/// segments are rejected too: on Windows they would be interpreted as
/// path separators and could escape; on Unix they're nonsensical.
fn is_safe_path(url_path: &str) -> bool {
    let stripped = url_path.trim_start_matches('/');
    if stripped.is_empty() {
        return true;
    }
    if stripped.contains('\0') {
        return false;
    }
    let p = Path::new(stripped);
    for comp in p.components() {
        match comp {
            Component::Normal(part) => {
                if let Some(s) = part.to_str() {
                    if s.contains('\\') {
                        return false;
                    }
                }
            }
            Component::CurDir => {}
            // Any of these means the URL tried to escape the project
            // root. ParentDir is the obvious one; the rest only ever
            // arise on Windows-shaped inputs and are equally unwanted.
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => return false,
        }
    }
    true
}

/// Read `path` from disk and turn it into an `OK` response with a
/// derived `Content-Type`. On read failure we fall through to the 404
/// path so a vanished file behaves like a missing one.
///
/// Before reading, we canonicalize `path` and verify it still lives
/// inside `dist_root` — a symlink planted inside dist that points
/// outside the root would otherwise be followed silently. Canonicalize
/// errors (broken symlink, missing file) are treated as not-found.
async fn serve_file(path: &Path, dist_root: &Path) -> Response {
    if !path_is_within_root(path, dist_root) {
        return not_found_response(dist_root).await;
    }
    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(_) => return not_found_response(dist_root).await,
    };
    let ct = content_type_for_path(path);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ct)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Build a 301 response pointing at `target`. The Location header is
/// validated; if the value is somehow not header-safe (it should be —
/// we only ever construct it from sanitised paths) we fall back to
/// `/`.
fn redirect_response(target: &str) -> Response {
    let location = HeaderValue::try_from(target).unwrap_or_else(|_| HeaderValue::from_static("/"));
    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header(header::LOCATION, location)
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Serve `dist/404.html` if present, else a plain text 404 body.
async fn not_found_response(dist_root: &Path) -> Response {
    let candidate = dist_root.join("404.html");
    if candidate.is_file() {
        if let Ok(bytes) = tokio::fs::read(&candidate).await {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    }
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from("404 Not Found"))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Map a file path to a `Content-Type` value by delegating to the canonical
/// [`zfb_server::content_type_for_extension`] helper. That helper handles the
/// same pragmatic extension set (HTML, CSS, JS, JSON, SVG, images, fonts,
/// wasm, …) with `mime_guess` as a catch-all fallback; keeping preview on the
/// same code path avoids the two tables drifting over time.
fn content_type_for_path(path: &Path) -> String {
    // Extract the extension without allocating when there is none.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    zfb_server::content_type_for_extension(ext)
}

// ---------------------------------------------------------------------------
// Adapter mode (Cloudflare via wrangler)
// ---------------------------------------------------------------------------

/// Spawn `pnpm exec wrangler pages dev …` and wait for it. We do not
/// pipe output — wrangler prints its own ready banner and we want the
/// user to see it directly. Returns non-zero when wrangler exits non-
/// zero so the parent shell sees the failure.
///
/// Before handing off, runs a pre-flight `pnpm exec wrangler --version`
/// gate against [`EXPECTED_WRANGLER_VERSION`]. A mismatch aborts with
/// an actionable error pointing at the version-pin procedure. The gate
/// can be bypassed by setting the
/// [`SKIP_WRANGLER_VERSION_CHECK_ENV`] env var to `1`.
async fn run_via_wrangler(project_root: &Path, outdir: &Path, host: &str, port: u16) -> Result<()> {
    ensure_wrangler_version(project_root).await?;

    output::info(format!(
        "preview: adapter mode — handing off to wrangler pages dev (host {host}, port {port})"
    ));

    let mut cmd = build_wrangler_command(project_root, outdir, host, port);
    let mut child = cmd
        .spawn()
        .context("failed to spawn wrangler — make sure it is installed in this project (pnpm add -D wrangler)")?;

    let status = child
        .wait()
        .await
        .context("failed to await wrangler subprocess")?;
    if !status.success() {
        anyhow::bail!("wrangler pages dev exited with status {status}");
    }
    Ok(())
}

/// Run `pnpm exec wrangler --version` and abort if the reported
/// version does not match [`EXPECTED_WRANGLER_VERSION`]. Skipped when
/// [`SKIP_WRANGLER_VERSION_CHECK_ENV`] is set to a truthy value.
async fn ensure_wrangler_version(project_root: &Path) -> Result<()> {
    if env_truthy(SKIP_WRANGLER_VERSION_CHECK_ENV, |name| {
        std::env::var(name).ok()
    }) {
        return Ok(());
    }

    let output = tokio::process::Command::new("pnpm")
        .arg("exec")
        .arg("wrangler")
        .arg("--version")
        .current_dir(project_root)
        .output()
        .await
        .context(
            "failed to spawn `pnpm exec wrangler --version` for the wrangler version gate \
             — make sure pnpm and wrangler are installed in this project",
        )?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "`pnpm exec wrangler --version` exited with status {}: {}",
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    match parse_wrangler_version(&stdout) {
        Some(reported) if reported == EXPECTED_WRANGLER_VERSION => Ok(()),
        Some(reported) => Err(anyhow::anyhow!(
            "wrangler version mismatch: expected `{expected}` (pinned in zfb), \
             got `{reported}`. \
             Update both the `wrangler` entry in package.json and \
             EXPECTED_WRANGLER_VERSION in crates/zfb-toolchain-pins/src/lib.rs in \
             lock-step (see the External tool version pins section in CONTRIBUTING.md), then run \
             `pnpm install`. To bypass this gate temporarily, set \
             {env}=1 (not recommended for steady-state use).",
            expected = EXPECTED_WRANGLER_VERSION,
            env = SKIP_WRANGLER_VERSION_CHECK_ENV,
        )),
        None => Err(anyhow::anyhow!(
            "could not parse a wrangler version from `pnpm exec wrangler --version` \
             output: {raw:?}. Expected something containing `{expected}`. \
             Set {env}=1 to bypass this gate if the output format has changed.",
            raw = stdout.trim(),
            expected = EXPECTED_WRANGLER_VERSION,
            env = SKIP_WRANGLER_VERSION_CHECK_ENV,
        )),
    }
}

/// Extract a semver-shaped version string from `wrangler --version`'s
/// banner. The current banner is roughly ` ⛅️ wrangler 4.85.0` — we
/// look for a version-shaped token *immediately following* a literal
/// `wrangler` token, then fall back to the first version-shaped token
/// in the output. The "wrangler-prefix" preference is what makes the
/// parser robust against banners that mention other version-shaped
/// strings (e.g. a copyright date or "[pre-release of 4.x.y]" remark)
/// before the real wrangler version. A leading `v` (e.g. `v4.85.0`)
/// is stripped, since other CLIs in the ecosystem emit that prefix
/// even though wrangler does not today — keeping the parser tolerant
/// future-proofs against an upstream banner reshuffle. Returns `None`
/// if no token matches, which the caller turns into a "could not
/// parse" error.
fn parse_wrangler_version(stdout: &str) -> Option<String> {
    let tokens: Vec<&str> = stdout.split_whitespace().collect();

    // First pass: prefer a version-shaped token that immediately
    // follows a literal `wrangler` token. This anchors the match to
    // the banner's actual wrangler-version field.
    for window in tokens.windows(2) {
        if let [prev, candidate] = window {
            if prev
                .trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .eq_ignore_ascii_case("wrangler")
            {
                if let Some(v) = version_shape(candidate) {
                    return Some(v);
                }
            }
        }
    }

    // Fallback: any version-shaped token. Tolerated for safety on
    // unforeseen banner reshuffles.
    for raw_token in &tokens {
        if let Some(v) = version_shape(raw_token) {
            return Some(v);
        }
    }
    None
}

/// Return `Some(version_string)` if `raw_token`'s body matches
/// `MAJOR.MINOR.PATCH...`. Strips a leading `v` and surrounding
/// non-alphanumerics. Returns the body without the `v` prefix.
fn version_shape(raw_token: &str) -> Option<String> {
    let token = raw_token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let body = token.strip_prefix('v').unwrap_or(token);
    let mut parts = body.splitn(3, '.');
    let (Some(maj), Some(min), Some(patch)) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    if maj.chars().all(|c| c.is_ascii_digit())
        && min.chars().all(|c| c.is_ascii_digit())
        && !maj.is_empty()
        && !min.is_empty()
        && !patch.is_empty()
        && patch
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    {
        Some(body.to_string())
    } else {
        None
    }
}

/// Treat `1`, `true`, `yes` (case-insensitive) as truthy. Anything else
/// — including unset / missing — is falsy. The lookup is delegated to
/// `getter` so tests can drive the function without touching process
/// environment (which is `unsafe` under Rust 2024).
fn env_truthy<F>(name: &str, getter: F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    match getter(name) {
        Some(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
        None => false,
    }
}

/// Build the `tokio::process::Command` we'd spawn for wrangler. Pulled
/// out so unit tests can introspect program name and args without
/// actually spawning a subprocess.
fn build_wrangler_command(
    project_root: &Path,
    outdir: &Path,
    host: &str,
    port: u16,
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("pnpm");
    cmd.arg("exec")
        .arg("wrangler")
        .arg("pages")
        .arg("dev")
        .arg(outdir)
        .arg("--port")
        .arg(port.to_string());
    // Only thread `--ip` when the user asked for a non-default host. wrangler
    // binds loopback by default; passing `--ip 0.0.0.0` exposes it to the LAN,
    // matching static-mode `--host`. Omitting it for the default keeps the
    // wrangler invocation unchanged for the common case.
    if host != DEFAULT_PREVIEW_HOST {
        cmd.arg("--ip").arg(host);
    }
    cmd.current_dir(project_root);
    cmd
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use std::fs;
    use tempfile::TempDir;
    use tower::ServiceExt;

    // ---- path safety ---------------------------------------------------

    #[test]
    fn is_safe_path_accepts_simple_paths() {
        assert!(is_safe_path("/"));
        assert!(is_safe_path(""));
        assert!(is_safe_path("/foo"));
        assert!(is_safe_path("/foo/"));
        assert!(is_safe_path("/foo/bar/baz.html"));
        assert!(is_safe_path("/assets/app.js"));
    }

    #[test]
    fn is_safe_path_rejects_parent_traversal() {
        assert!(!is_safe_path("/../etc/passwd"));
        assert!(!is_safe_path("/foo/../../etc"));
        assert!(!is_safe_path("/.."));
    }

    #[test]
    fn is_safe_path_rejects_nul_and_backslash() {
        assert!(!is_safe_path("/foo\0bar"));
        assert!(!is_safe_path("/foo\\bar"));
    }

    // ---- routing rule (resolve_static) --------------------------------

    /// Build a fixture dist tree:
    ///
    /// ```text
    /// dist/
    ///   index.html
    ///   404.html
    ///   about/
    ///     index.html
    ///   blog/
    ///     post.html
    ///   assets/
    ///     app.js
    /// ```
    fn fixture_dist() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(root.join("index.html"), "<h1>home</h1>").unwrap();
        fs::write(root.join("404.html"), "<h1>missing</h1>").unwrap();
        fs::create_dir_all(root.join("about")).unwrap();
        fs::write(root.join("about").join("index.html"), "<h1>about</h1>").unwrap();
        fs::create_dir_all(root.join("blog")).unwrap();
        fs::write(root.join("blog").join("post.html"), "<h1>post</h1>").unwrap();
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(root.join("assets").join("app.js"), "console.log('x');").unwrap();
        dir
    }

    #[test]
    fn resolve_root_serves_index_html() {
        let dist = fixture_dist();
        let res = resolve_static(dist.path(), "/");
        assert_eq!(res, Resolution::File(dist.path().join("index.html")));
    }

    #[test]
    fn resolve_directory_with_index_redirects_when_no_trailing_slash() {
        let dist = fixture_dist();
        let res = resolve_static(dist.path(), "/about");
        assert_eq!(res, Resolution::Redirect("/about/".to_string()));
    }

    #[test]
    fn resolve_directory_with_index_serves_index_when_trailing_slash() {
        let dist = fixture_dist();
        let res = resolve_static(dist.path(), "/about/");
        assert_eq!(
            res,
            Resolution::File(dist.path().join("about").join("index.html"))
        );
    }

    #[test]
    fn resolve_regular_file_served_directly() {
        let dist = fixture_dist();
        let res = resolve_static(dist.path(), "/blog/post.html");
        assert_eq!(
            res,
            Resolution::File(dist.path().join("blog").join("post.html"))
        );
    }

    #[test]
    fn resolve_asset_file_served_directly() {
        let dist = fixture_dist();
        let res = resolve_static(dist.path(), "/assets/app.js");
        assert_eq!(
            res,
            Resolution::File(dist.path().join("assets").join("app.js"))
        );
    }

    #[test]
    fn resolve_naked_directory_without_index_404s() {
        let dist = fixture_dist();
        // `blog/` has post.html but no index.html — must 404, NOT
        // produce a directory listing.
        let res = resolve_static(dist.path(), "/blog/");
        assert_eq!(res, Resolution::NotFound);
        let res2 = resolve_static(dist.path(), "/blog");
        assert_eq!(res2, Resolution::NotFound);
    }

    #[test]
    fn resolve_missing_route_404s() {
        let dist = fixture_dist();
        assert_eq!(resolve_static(dist.path(), "/nope"), Resolution::NotFound);
        assert_eq!(resolve_static(dist.path(), "/nope/"), Resolution::NotFound);
        assert_eq!(
            resolve_static(dist.path(), "/foo/bar/baz"),
            Resolution::NotFound
        );
    }

    #[test]
    fn resolve_traversal_attempts_404() {
        let dist = fixture_dist();
        assert_eq!(
            resolve_static(dist.path(), "/../etc/passwd"),
            Resolution::NotFound
        );
        assert_eq!(
            resolve_static(dist.path(), "/about/../../escape"),
            Resolution::NotFound
        );
    }

    #[test]
    fn resolve_root_404_when_index_missing() {
        // Empty dist → root becomes 404. The HTTP-side 404.html
        // fallback is exercised in the handler test below.
        let dir = TempDir::new().unwrap();
        assert_eq!(resolve_static(dir.path(), "/"), Resolution::NotFound);
    }

    // ---- content type --------------------------------------------------

    #[test]
    fn content_type_for_known_extensions() {
        assert_eq!(
            content_type_for_path(Path::new("foo.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type_for_path(Path::new("foo.css")),
            "text/css; charset=utf-8"
        );
        assert_eq!(
            content_type_for_path(Path::new("foo.js")),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type_for_path(Path::new("foo.svg")),
            "image/svg+xml"
        );
        assert_eq!(content_type_for_path(Path::new("foo.png")), "image/png");
        assert_eq!(
            content_type_for_path(Path::new("foo.wasm")),
            "application/wasm"
        );
    }

    #[test]
    fn content_type_for_unknown_extension_falls_back_to_octet_stream() {
        assert_eq!(
            content_type_for_path(Path::new("foo.bin")),
            "application/octet-stream"
        );
        assert_eq!(
            content_type_for_path(Path::new("noext")),
            "application/octet-stream"
        );
    }

    // ---- end-to-end handler tests via tower::oneshot ------------------
    //
    // These do NOT bind a port. We stand the router up in-process and
    // drive it with `tower::ServiceExt::oneshot`, which calls the
    // handler directly over a `Service<Request>` boundary — same as
    // `zfb-server`'s route tests.

    async fn body_string(resp: Response) -> String {
        let bytes = to_bytes(resp.into_body(), 1_000_000).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    async fn body_bytes(resp: Response) -> Vec<u8> {
        let bytes = to_bytes(resp.into_body(), 1_000_000).await.unwrap();
        bytes.to_vec()
    }

    #[tokio::test]
    async fn handler_serves_root_index() {
        let dist = fixture_dist();
        let router = build_static_router(dist.path().to_path_buf());

        let resp = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "text/html; charset=utf-8");
        assert!(body_string(resp).await.contains("home"));
    }

    #[tokio::test]
    async fn handler_redirects_directory_without_trailing_slash() {
        let dist = fixture_dist();
        let router = build_static_router(dist.path().to_path_buf());

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/about")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(location, "/about/");
    }

    #[tokio::test]
    async fn handler_redirects_directory_with_query_string() {
        // GET /about?c=1a (no trailing slash, dist/about/index.html exists)
        // must redirect to /about/?c=1a — query string must be preserved
        // exactly (verbatim, no re-encoding).
        let dist = fixture_dist();
        let router = build_static_router(dist.path().to_path_buf());

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/about?c=1a")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            location, "/about/?c=1a",
            "redirect must carry the query string verbatim (got {location:?})"
        );
    }

    #[tokio::test]
    async fn handler_redirects_directory_no_bare_question_mark_when_no_query() {
        // GET /about (no query) must redirect to /about/ — NOT /about/? (bare ?).
        let dist = fixture_dist();
        let router = build_static_router(dist.path().to_path_buf());

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/about")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            location, "/about/",
            "no query string means no bare ? in Location (got {location:?})"
        );
    }

    #[tokio::test]
    async fn handler_serves_directory_index_with_trailing_slash() {
        let dist = fixture_dist();
        let router = build_static_router(dist.path().to_path_buf());

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/about/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_string(resp).await.contains("about"));
    }

    #[tokio::test]
    async fn handler_serves_regular_file() {
        let dist = fixture_dist();
        let router = build_static_router(dist.path().to_path_buf());

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/assets/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "application/javascript; charset=utf-8");
        let body = body_string(resp).await;
        assert!(body.contains("console.log"));
    }

    #[tokio::test]
    async fn handler_404_falls_back_to_project_404_html() {
        let dist = fixture_dist();
        let router = build_static_router(dist.path().to_path_buf());

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "text/html; charset=utf-8");
        assert!(body_string(resp).await.contains("missing"));
    }

    #[tokio::test]
    async fn handler_404_uses_plain_text_when_no_404_html() {
        // A dist tree without the project's own 404.html should fall
        // back to the plain-text body so users still see a clear
        // signal in the browser.
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("index.html"), "<h1>hi</h1>").unwrap();
        let router = build_static_router(dir.path().to_path_buf());

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "text/plain; charset=utf-8");
        assert!(body_string(resp).await.contains("404"));
    }

    #[tokio::test]
    async fn handler_naked_directory_404s_no_listing() {
        let dist = fixture_dist();
        let router = build_static_router(dist.path().to_path_buf());

        // /blog/ has no index.html — must be a 404, NEVER a directory
        // listing.
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/blog/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_string(resp).await;
        // The 404.html fixture body should appear; no list of files.
        assert!(!body.contains("post.html"));
    }

    #[tokio::test]
    async fn handler_traversal_attempts_do_not_serve_real_files() {
        let dist = fixture_dist();
        let router = build_static_router(dist.path().to_path_buf());

        // axum/hyper normalises some traversal attempts before they
        // reach the handler; either way the response must not be a
        // 200 with content from outside dist_root.
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/../etc/passwd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn handler_serves_binary_file_bytes() {
        // Sanity: we read bytes (not a UTF-8 string) so binary assets
        // round-trip cleanly. Use a small fake PNG header.
        let dir = TempDir::new().unwrap();
        let bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0xff];
        fs::write(dir.path().join("pixel.png"), &bytes).unwrap();
        let router = build_static_router(dir.path().to_path_buf());

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/pixel.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "image/png");
        assert_eq!(body_bytes(resp).await, bytes);
    }

    // ---- adapter-mode wiring (no actual spawn) -----------------------

    #[test]
    fn wrangler_command_uses_pnpm_exec_with_outdir_and_port() {
        let project_root = Path::new("/tmp/proj");
        let outdir = Path::new("/tmp/proj/dist");
        // Default host: no `--ip` is threaded, so the invocation is unchanged.
        let cmd = build_wrangler_command(project_root, outdir, DEFAULT_PREVIEW_HOST, 8788);

        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program(), "pnpm");

        let args: Vec<&std::ffi::OsStr> = std_cmd.get_args().collect();
        // Expect: exec wrangler pages dev <outdir> --port <port>
        assert_eq!(args[0], "exec");
        assert_eq!(args[1], "wrangler");
        assert_eq!(args[2], "pages");
        assert_eq!(args[3], "dev");
        assert_eq!(args[4], outdir.as_os_str());
        assert_eq!(args[5], "--port");
        assert_eq!(args[6], "8788");
        assert!(
            !args.iter().any(|a| *a == "--ip"),
            "default host must not thread --ip"
        );

        assert_eq!(std_cmd.get_current_dir(), Some(project_root));
    }

    #[test]
    fn wrangler_command_threads_user_supplied_port() {
        // `--port` must reflect whatever resolve_port produced — so
        // overriding from the CLI propagates all the way through.
        let cmd =
            build_wrangler_command(Path::new("/x"), Path::new("/x/dist"), DEFAULT_PREVIEW_HOST, 9000);
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let port_idx = args
            .iter()
            .position(|a| a == "--port")
            .expect("--port must be present");
        assert_eq!(args[port_idx + 1], "9000");
    }

    #[test]
    fn wrangler_command_threads_ip_for_non_default_host() {
        // A non-default host (e.g. 0.0.0.0) must thread `--ip <host>` so the
        // adapter preview is reachable on the LAN, matching static-mode --host.
        let cmd = build_wrangler_command(Path::new("/x"), Path::new("/x/dist"), "0.0.0.0", 9000);
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let ip_idx = args
            .iter()
            .position(|a| a == "--ip")
            .expect("--ip must be present for a non-default host");
        assert_eq!(args[ip_idx + 1], "0.0.0.0");
    }

    // ---- wrangler version-gate parser --------------------------------

    #[test]
    fn parse_wrangler_version_extracts_pinned_format() {
        // The exact banner shape `wrangler --version` prints today.
        let stdout = " ⛅️ wrangler 4.85.0\n";
        assert_eq!(
            parse_wrangler_version(stdout).as_deref(),
            Some("4.85.0"),
            "must extract `4.85.0` from the canonical banner",
        );
    }

    #[test]
    fn parse_wrangler_version_handles_bare_version_line() {
        // Some CI environments / shims may print just the bare version.
        assert_eq!(
            parse_wrangler_version("4.85.0\n").as_deref(),
            Some("4.85.0"),
        );
    }

    #[test]
    fn parse_wrangler_version_strips_leading_v_prefix() {
        // Wrangler doesn't emit a leading `v` today, but other CLIs in
        // the ecosystem do — strip it so the equality check downstream
        // continues to match `4.85.0` even if upstream changes its banner.
        assert_eq!(
            parse_wrangler_version("⛅ wrangler v4.85.0").as_deref(),
            Some("4.85.0"),
        );
    }

    #[test]
    fn parse_wrangler_version_handles_prerelease_suffix() {
        // We accept any token whose head matches `MAJOR.MINOR.PATCH…`,
        // which lets prereleases round-trip through the parser. The
        // version-equality check downstream is what enforces strict
        // pinning — the parser's job is only extraction.
        assert_eq!(
            parse_wrangler_version("⛅ wrangler 5.0.0-rc.1").as_deref(),
            Some("5.0.0-rc.1"),
        );
    }

    #[test]
    fn parse_wrangler_version_returns_none_on_unrecognised_output() {
        assert_eq!(parse_wrangler_version("hello world\n"), None);
        assert_eq!(parse_wrangler_version(""), None);
        // Two-segment "version" (e.g. `4.85`) is not a valid semver
        // shape — we require all three of MAJOR.MINOR.PATCH.
        assert_eq!(parse_wrangler_version("wrangler 4.85"), None);
    }

    #[test]
    fn parse_wrangler_version_prefers_token_after_wrangler_literal() {
        // Defensive: if a banner mentions a version-shaped string
        // before `wrangler 4.85.0` (a copyright date, a prerelease
        // remark, etc.), the version after the literal `wrangler`
        // token must win — not the first version-shaped token.
        let stdout = "Copyright 2024.10.0 Cloudflare. ⛅️ wrangler 4.85.0";
        assert_eq!(
            parse_wrangler_version(stdout).as_deref(),
            Some("4.85.0"),
            "must anchor on the token immediately after `wrangler`",
        );
    }

    #[test]
    fn parse_wrangler_version_falls_back_when_no_wrangler_literal() {
        // No `wrangler` token in the banner — fall back to the first
        // version-shaped token. Keeps bare-version shims working.
        assert_eq!(
            parse_wrangler_version("4.85.0\n").as_deref(),
            Some("4.85.0"),
        );
    }

    #[test]
    fn env_truthy_recognises_common_truthy_values() {
        let key = "ZFB_TEST_ENV_TRUTHY_KEY";
        for (value, expected) in [
            ("1", true),
            ("true", true),
            ("TRUE", true),
            ("yes", true),
            ("0", false),
            ("false", false),
            ("", false),
            ("nope", false),
        ] {
            // Drive `env_truthy` via an injected getter rather than
            // mutating the real process environment. `set_var` is
            // `unsafe` under Rust 2024 because it races other threads
            // reading the env table.
            let v = value.to_string();
            assert_eq!(
                env_truthy(key, |_| Some(v.clone())),
                expected,
                "value = {value:?}",
            );
        }
        // Unset = no value returned by the getter.
        assert!(
            !env_truthy(key, |_| None),
            "unset env var must be falsy",
        );
    }

    // -------------------------------------------------------------------
    // Issue #899 — symlink-containment: symlinks pointing outside the
    // preview dist root must be blocked; legitimate in-root symlinks
    // must keep working.
    // -------------------------------------------------------------------

    /// A symlink inside `dist/` pointing OUTSIDE it must produce a 404
    /// response from the preview server — not serve the target file.
    #[cfg(unix)]
    #[tokio::test]
    async fn preview_rejects_out_of_root_symlink_in_dist() {
        use std::os::unix::fs::symlink;

        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), b"secret").unwrap();

        let dist = TempDir::new().unwrap();
        symlink(
            outside.path().join("secret.txt"),
            dist.path().join("escape.txt"),
        )
        .unwrap();

        let router = build_static_router(dist.path().to_path_buf());

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/escape.txt")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(
            resp.status(),
            StatusCode::OK,
            "out-of-root symlink in preview dist must not be served (got {:?})",
            resp.status()
        );
    }

    /// A symlink inside `dist/` that points to another file WITHIN `dist/`
    /// must be served — legitimate in-root symlinks must keep working.
    #[cfg(unix)]
    #[tokio::test]
    async fn preview_serves_in_root_symlink_in_dist() {
        use std::os::unix::fs::symlink;

        let dist = TempDir::new().unwrap();
        fs::write(dist.path().join("real.html"), b"<h1>real</h1>").unwrap();
        // Symlink inside dist pointing at another file inside dist.
        symlink(dist.path().join("real.html"), dist.path().join("alias.html")).unwrap();

        let router = build_static_router(dist.path().to_path_buf());

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/alias.html")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "in-root symlink in preview dist must be served"
        );
        let body = body_string(resp).await;
        assert!(body.contains("real"), "served body must match the symlink target");
    }
}
