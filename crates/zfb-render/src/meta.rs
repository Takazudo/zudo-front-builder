//! Page meta export handling.
//!
//! Reads the `export const meta` value from a page module, parses it into
//! a typed [`PageMeta`], and resolves the layout module that should wrap
//! the page output.
//!
//! # Default-layout convention
//!
//! When a page does not specify `meta.layout`, the renderer falls back to
//! a project-wide default-layout convention. The caller (the orchestrator
//! in `render.rs`) builds the candidate list; this module just walks it
//! and picks the first existing file.
//!
//! The conventional candidate order is:
//!
//! - For `pages/blog/[slug].tsx`:
//!   1. `layouts/blog.tsx` — matches the immediate parent dir name
//!   2. `layouts/default.tsx` — project-wide fallback
//! - For `pages/about.tsx`:
//!   1. `layouts/default.tsx`
//!
//! When no candidate exists on disk, [`ResolvedMeta::layout_path`] is
//! `None` and the page output is rendered raw.
//!
//! # `meta.layout` resolution
//!
//! When `meta.layout` is set explicitly, it is interpreted in this order:
//!
//! - Values prefixed with `@/` (e.g. `@/layouts/blog`) are resolved
//!   relative to the project root (the directory that contains `pages/`).
//! - Values prefixed with `/` are treated as project-root absolute, NOT
//!   filesystem absolute. This keeps user-authored layout specs portable.
//! - Any other value (e.g. `../layouts/blog`, `./blog`) is resolved
//!   relative to the page file's parent directory.
//!
//! The file extension is optional; `.tsx` is auto-appended when the value
//! does not already end in a known JS/TS extension (`tsx`, `ts`, `jsx`,
//! `js`, `mjs`, `cjs`).
//!
//! # Signature note
//!
//! [`resolve_meta`] takes an extra `project_root` parameter beyond the
//! initial spec, so that `@/`-aliases and `/`-rooted specs can be resolved
//! cleanly without forcing the caller to pre-rewrite them.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// User-authored meta. Everything is optional — pages without a meta
/// export still work.
///
/// Unknown top-level fields are rejected at parse time
/// (`#[serde(deny_unknown_fields)]`) so that frontmatter typos surface
/// as errors instead of being silently dropped.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PageMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    /// Path or specifier (e.g. `"@/layouts/blog"` or `"../layouts/blog"`).
    pub layout: Option<String>,
}

/// A [`PageMeta`] paired with its resolved layout file path.
#[derive(Clone, Debug)]
pub struct ResolvedMeta {
    pub meta: PageMeta,
    /// Concrete layout module path resolved from `meta.layout` (or via
    /// the default-layout convention). `None` means "no layout, render
    /// the page output directly".
    pub layout_path: Option<PathBuf>,
    /// Output-file extension override extracted from
    /// `export const extension = "…"` on the page module (the TSX
    /// frontmatter extractor). Beats the filename convention; both fall
    /// through to the `html` default. See [`derive_output_extension`]
    /// for the precedence rule.
    pub extension: Option<String>,
    /// `Content-Type` override extracted from
    /// `export const contentType = "…"`. Build-time metadata only — the
    /// dev server (`zfb-server`) consults this when setting the
    /// `Content-Type` response header for the page; static-file hosts
    /// derive it from the file extension instead. See
    /// [`derive_content_type`] for the precedence rule.
    pub content_type: Option<String>,
}

/// Errors produced while parsing or resolving a page meta export.
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("invalid meta export shape: {0}")]
    InvalidShape(String),
    #[error("layout not found: {0}")]
    LayoutNotFound(String),
    #[error("layout escapes project root: {0}")]
    LayoutOutsideProjectRoot(String),
}

const KNOWN_EXTENSIONS: &[&str] = &["tsx", "ts", "jsx", "js", "mjs", "cjs"];

/// Parse the raw JS-side meta export (a [`serde_json::Value`]) into a
/// typed [`PageMeta`].
///
/// Returns `Ok(PageMeta::default())` when `meta_export` is absent or
/// `null`. Any non-object value (number, string, array, bool) is rejected
/// as [`MetaError::InvalidShape`], as is an object whose fields have the
/// wrong types (e.g. `layout = 42`).
pub fn parse_meta(meta_export: Option<&Value>) -> Result<PageMeta, MetaError> {
    match meta_export {
        None | Some(Value::Null) => Ok(PageMeta::default()),
        Some(value) => {
            if !value.is_object() {
                return Err(MetaError::InvalidShape(format!(
                    "expected object, got {}",
                    json_type_name(value)
                )));
            }
            serde_json::from_value::<PageMeta>(value.clone())
                .map_err(|e| MetaError::InvalidShape(e.to_string()))
        }
    }
}

/// Resolve `meta.layout` (or, if unset, the default-layout convention) to
/// a concrete file on disk.
///
/// - When `meta.layout` is `Some(spec)`, the spec is interpreted using
///   the rules described in the module docs and the resolved path is
///   required to exist (else [`MetaError::LayoutNotFound`]). The resolved
///   path must stay within `project_root` — `..`-traversal that escapes
///   the project (e.g. `"../../../etc/passwd"`) is rejected with
///   [`MetaError::LayoutOutsideProjectRoot`].
/// - When `meta.layout` is `None`, the first entry of
///   `default_layout_candidates` that exists on disk wins. Default
///   candidates are caller-supplied (typically built by the orchestrator
///   from the page path) and are NOT re-validated against `project_root`
///   — the caller is trusted to construct them correctly. Missing default
///   candidates do NOT produce [`MetaError::LayoutNotFound`]; the result
///   simply has `layout_path = None`.
/// - When neither yields a file, the result has `layout_path = None`.
///
/// `project_root` should be the directory that contains `pages/` and is
/// used to resolve `@/`-aliases and `/`-rooted specs as well as to bound
/// relative resolutions.
pub fn resolve_meta(
    meta: PageMeta,
    page_path: &Path,
    project_root: &Path,
    default_layout_candidates: &[PathBuf],
) -> Result<ResolvedMeta, MetaError> {
    resolve_meta_with_overrides(
        meta,
        page_path,
        project_root,
        default_layout_candidates,
        None,
        None,
    )
}

/// Same as [`resolve_meta`] but also threads through the
/// `extension` / `contentType` overrides extracted from the page's TSX
/// frontmatter (see `zfb_content::tsx_frontmatter::extract`).
///
/// The orchestrator uses this entry point because the layout
/// resolution and the output-extension / content-type resolution share
/// the same "is this page valid?" failure modes — keeping them on one
/// `ResolvedMeta` lets the call site fail-fast on either error without
/// constructing two parallel structures.
pub fn resolve_meta_with_overrides(
    meta: PageMeta,
    page_path: &Path,
    project_root: &Path,
    default_layout_candidates: &[PathBuf],
    extension: Option<String>,
    content_type: Option<String>,
) -> Result<ResolvedMeta, MetaError> {
    let layout_path = match &meta.layout {
        Some(spec) => {
            let resolved = resolve_layout_spec(spec, page_path, project_root);
            if !is_within(&resolved, project_root) {
                return Err(MetaError::LayoutOutsideProjectRoot(
                    resolved.display().to_string(),
                ));
            }
            if !resolved.exists() {
                return Err(MetaError::LayoutNotFound(resolved.display().to_string()));
            }
            Some(resolved)
        }
        None => default_layout_candidates
            .iter()
            .find(|c| c.exists())
            .cloned(),
    };
    Ok(ResolvedMeta {
        meta,
        layout_path,
        extension,
        content_type,
    })
}

/// Default output extension used when neither the frontmatter nor the
/// filename convention specifies one.
pub const DEFAULT_OUTPUT_EXTENSION: &str = "html";

/// Default `Content-Type` header used when no extension-specific entry
/// is known.
pub const DEFAULT_CONTENT_TYPE: &str = "text/html; charset=utf-8";

/// Apply the documented precedence to derive the final output
/// extension for a page:
///
/// 1. `frontmatter_extension` (the value of `export const extension`),
/// 2. else the filename-convention `route_extension` (e.g. the `xml`
///    in `sitemap.xml.tsx`),
/// 3. else `"html"` ([`DEFAULT_OUTPUT_EXTENSION`]).
///
/// This rule is also pinned in `zfb_router::route::Route::output_filename`
/// (the router crate doesn't depend on this one, so the rule lives in
/// both places). Keep the two in sync when changing it.
pub fn derive_output_extension(
    frontmatter_extension: Option<&str>,
    route_extension: Option<&str>,
) -> String {
    frontmatter_extension
        .or(route_extension)
        .unwrap_or(DEFAULT_OUTPUT_EXTENSION)
        .to_string()
}

/// Apply the documented precedence to derive the final
/// `Content-Type` for a page:
///
/// 1. `frontmatter_content_type` (the value of `export const contentType`),
/// 2. else the conventional default for `extension` (e.g. `xml` →
///    `application/xml`, `rss` → `application/rss+xml`,
///    `txt` → `text/plain; charset=utf-8`, `html` →
///    `text/html; charset=utf-8`),
/// 3. else [`DEFAULT_CONTENT_TYPE`] (`text/html; charset=utf-8`) — a
///    permissive fallback for unknown extensions; in practice the
///    page author should set `contentType` explicitly when picking
///    an exotic extension so the dev server doesn't lie.
///
/// The known-extension table is intentionally small and only covers
/// the cases we care about for static-site builds. Adding entries is
/// a non-breaking change.
pub fn derive_content_type(extension: &str, frontmatter_content_type: Option<&str>) -> String {
    if let Some(ct) = frontmatter_content_type {
        return ct.to_string();
    }
    // Mirror of zfb_server::routes::content_type_for_extension; keep both
    // in sync. Differs in the catch-all only: pages emitted
    // by the SSG renderer default to HTML when the extension is unknown,
    // because the build pipeline only writes route outputs the user
    // declared (a missing extension on a known page is HTML).
    match extension.to_ascii_lowercase().as_str() {
        // Documents
        "html" | "htm" => "text/html; charset=utf-8".to_string(),
        "xml" => "application/xml".to_string(),
        "rss" => "application/rss+xml".to_string(),
        "atom" => "application/atom+xml".to_string(),
        "json" | "map" => "application/json".to_string(),
        "webmanifest" => "application/manifest+json".to_string(),
        "txt" => "text/plain; charset=utf-8".to_string(),
        // Code / styles
        "css" => "text/css; charset=utf-8".to_string(),
        "js" | "mjs" | "cjs" => "application/javascript; charset=utf-8".to_string(),
        "wasm" => "application/wasm".to_string(),
        // Images
        "svg" => "image/svg+xml".to_string(),
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "webp" => "image/webp".to_string(),
        "avif" => "image/avif".to_string(),
        "gif" => "image/gif".to_string(),
        "ico" => "image/x-icon".to_string(),
        // Fonts
        "woff" => "font/woff".to_string(),
        "woff2" => "font/woff2".to_string(),
        "ttf" => "font/ttf".to_string(),
        "otf" => "font/otf".to_string(),
        "eot" => "application/vnd.ms-fontobject".to_string(),
        // Media
        "mp4" => "video/mp4".to_string(),
        "webm" => "video/webm".to_string(),
        "mp3" => "audio/mpeg".to_string(),
        "ogg" => "audio/ogg".to_string(),
        "pdf" => "application/pdf".to_string(),
        _ => DEFAULT_CONTENT_TYPE.to_string(),
    }
}

fn resolve_layout_spec(spec: &str, page_path: &Path, project_root: &Path) -> PathBuf {
    let with_ext = ensure_extension(spec);
    let raw = if let Some(rest) = with_ext.strip_prefix("@/") {
        project_root.join(rest)
    } else if let Some(rest) = with_ext.strip_prefix('/') {
        project_root.join(rest)
    } else {
        // Relative spec — resolve from the page file's directory. If the
        // page path is somehow rootless (e.g. just "[slug].tsx"), fall
        // back to project_root rather than the empty path so that the
        // result still lands inside the project tree.
        let dir = page_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(project_root);
        dir.join(&with_ext)
    };
    normalize(&raw)
}

/// Lexical containment check: is `candidate` equal to or below `root`?
/// Both paths are normalized first so `..` segments cannot fool the check.
///
/// An empty `root` is rejected explicitly: `Path::starts_with(empty)` is
/// always `true`, which would silently disable the traversal guard for
/// any caller that happened to pass an empty project root.
///
/// Lexical containment with a relative `root` is fragile (`a/b` is
/// "within" `a/b` but is also nominally within `c/d` after a chdir).
/// Production callers always pass an absolute project root, so
/// `debug_assert!` it to surface accidental misuse during development
/// without paying the cost in release builds.
fn is_within(candidate: &Path, root: &Path) -> bool {
    debug_assert!(
        root.as_os_str().is_empty() || root.is_absolute(),
        "is_within: project_root must be absolute (got {})",
        root.display()
    );
    let cand = normalize(candidate);
    let root = normalize(root);
    if root.as_os_str().is_empty() {
        return false;
    }
    cand.starts_with(&root)
}

fn ensure_extension(spec: &str) -> String {
    let lower = spec.to_ascii_lowercase();
    let already = KNOWN_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")));
    if already {
        spec.to_string()
    } else {
        format!("{spec}.tsx")
    }
}

/// Collapse `.` and `..` components without touching the filesystem, so
/// the result is stable for paths that don't yet exist.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Build a minimal project tree under `root`:
    ///
    /// ```text
    /// <root>/
    ///   pages/
    ///     about.tsx
    ///     blog/
    ///       [slug].tsx
    ///   layouts/   (created on demand by callers)
    /// ```
    ///
    /// Returns the temp dir handle (kept alive by the caller).
    fn project_skeleton() -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pages/blog")).unwrap();
        std::fs::write(root.join("pages/about.tsx"), "// about").unwrap();
        std::fs::write(root.join("pages/blog/[slug].tsx"), "// blog").unwrap();
        std::fs::create_dir_all(root.join("layouts")).unwrap();
        dir
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, "// layout").unwrap();
    }

    // ---- parse_meta -------------------------------------------------------

    #[test]
    fn parse_meta_missing_returns_default() {
        let parsed = parse_meta(None).unwrap();
        assert!(parsed.title.is_none());
        assert!(parsed.layout.is_none());
    }

    #[test]
    fn parse_meta_null_returns_default() {
        let v = Value::Null;
        let parsed = parse_meta(Some(&v)).unwrap();
        assert!(parsed.title.is_none());
        assert!(parsed.layout.is_none());
    }

    #[test]
    fn parse_meta_title_only() {
        let v = json!({ "title": "Hello" });
        let parsed = parse_meta(Some(&v)).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Hello"));
        assert!(parsed.layout.is_none());
    }

    #[test]
    fn parse_meta_unknown_fields_are_rejected() {
        // deny_unknown_fields: unknown keys must surface as an error
        // rather than being silently swallowed. This catches frontmatter
        // typos (e.g. `titel` instead of `title`) that would otherwise
        // produce a confusingly empty page title.
        let v = json!({
            "title": "Hello",
            "openGraph": { "image": "/og.png" },
            "draft": true,
        });
        let err = parse_meta(Some(&v)).unwrap_err();
        match err {
            MetaError::InvalidShape(_) => {}
            other => unreachable!("expected InvalidShape for unknown field, got {other:?}"),
        }
    }

    #[test]
    fn parse_meta_known_fields_accepted() {
        // All three known fields can round-trip together.
        let v = json!({
            "title": "Hello",
            "description": "A page",
            "layout": "@/layouts/blog",
        });
        let parsed = parse_meta(Some(&v)).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Hello"));
        assert_eq!(parsed.description.as_deref(), Some("A page"));
        assert_eq!(parsed.layout.as_deref(), Some("@/layouts/blog"));
    }

    #[test]
    fn parse_meta_invalid_layout_type_is_invalid_shape() {
        let v = json!({ "layout": 42 });
        let err = parse_meta(Some(&v)).unwrap_err();
        match err {
            MetaError::InvalidShape(_) => {}
            other => unreachable!("expected InvalidShape, got {other:?}"),
        }
    }

    #[test]
    fn parse_meta_non_object_top_level_is_invalid_shape() {
        let v = json!("just a string");
        let err = parse_meta(Some(&v)).unwrap_err();
        match err {
            MetaError::InvalidShape(msg) => assert!(msg.contains("string")),
            other => unreachable!("expected InvalidShape, got {other:?}"),
        }
    }

    // ---- resolve_meta: explicit layout ------------------------------------

    #[test]
    fn resolve_layout_relative_parent_dir() {
        let dir = project_skeleton();
        let root = dir.path();
        let layout = root.join("layouts/blog.tsx");
        touch(&layout);

        let page = root.join("pages/blog/[slug].tsx");
        let meta = PageMeta {
            layout: Some("../../layouts/blog".to_string()),
            ..Default::default()
        };
        let resolved = resolve_meta(meta, &page, root, &[]).unwrap();
        assert_eq!(resolved.layout_path.as_deref(), Some(layout.as_path()));
    }

    #[test]
    fn resolve_layout_at_alias_uses_project_root() {
        let dir = project_skeleton();
        let root = dir.path();
        let layout = root.join("layouts/blog.tsx");
        touch(&layout);

        let page = root.join("pages/blog/[slug].tsx");
        let meta = PageMeta {
            layout: Some("@/layouts/blog".to_string()),
            ..Default::default()
        };
        let resolved = resolve_meta(meta, &page, root, &[]).unwrap();
        assert_eq!(resolved.layout_path.as_deref(), Some(layout.as_path()));
    }

    #[test]
    fn resolve_layout_dot_relative() {
        let dir = project_skeleton();
        let root = dir.path();
        // Sibling layout next to the page file itself.
        let layout = root.join("pages/blog/blog.tsx");
        touch(&layout);

        let page = root.join("pages/blog/[slug].tsx");
        let meta = PageMeta {
            layout: Some("./blog".to_string()),
            ..Default::default()
        };
        let resolved = resolve_meta(meta, &page, root, &[]).unwrap();
        assert_eq!(resolved.layout_path.as_deref(), Some(layout.as_path()));
    }

    #[test]
    fn resolve_layout_root_absolute_treated_as_project_root() {
        let dir = project_skeleton();
        let root = dir.path();
        let layout = root.join("layouts/blog.tsx");
        touch(&layout);

        let page = root.join("pages/blog/[slug].tsx");
        let meta = PageMeta {
            layout: Some("/layouts/blog".to_string()),
            ..Default::default()
        };
        let resolved = resolve_meta(meta, &page, root, &[]).unwrap();
        assert_eq!(resolved.layout_path.as_deref(), Some(layout.as_path()));
    }

    #[test]
    fn resolve_layout_keeps_explicit_extension() {
        let dir = project_skeleton();
        let root = dir.path();
        let layout = root.join("layouts/blog.jsx");
        touch(&layout);

        let page = root.join("pages/blog/[slug].tsx");
        let meta = PageMeta {
            layout: Some("@/layouts/blog.jsx".to_string()),
            ..Default::default()
        };
        let resolved = resolve_meta(meta, &page, root, &[]).unwrap();
        assert_eq!(resolved.layout_path.as_deref(), Some(layout.as_path()));
    }

    #[test]
    fn resolve_layout_escaping_project_root_is_rejected() {
        let dir = project_skeleton();
        let root = dir.path();
        let page = root.join("pages/blog/[slug].tsx");
        // From pages/blog/, this lexically escapes the project root.
        let meta = PageMeta {
            layout: Some("../../../etc/passwd".to_string()),
            ..Default::default()
        };
        let err = resolve_meta(meta, &page, root, &[]).unwrap_err();
        match err {
            MetaError::LayoutOutsideProjectRoot(_) => {}
            other => unreachable!("expected LayoutOutsideProjectRoot, got {other:?}"),
        }
    }

    #[test]
    fn resolve_layout_at_alias_escaping_root_is_rejected() {
        let dir = project_skeleton();
        let root = dir.path();
        let page = root.join("pages/about.tsx");
        let meta = PageMeta {
            layout: Some("@/../sibling".to_string()),
            ..Default::default()
        };
        let err = resolve_meta(meta, &page, root, &[]).unwrap_err();
        match err {
            MetaError::LayoutOutsideProjectRoot(_) => {}
            other => unreachable!("expected LayoutOutsideProjectRoot, got {other:?}"),
        }
    }

    #[test]
    fn resolve_layout_missing_file_errors() {
        let dir = project_skeleton();
        let root = dir.path();
        let page = root.join("pages/blog/[slug].tsx");
        let meta = PageMeta {
            layout: Some("@/layouts/nope".to_string()),
            ..Default::default()
        };
        let err = resolve_meta(meta, &page, root, &[]).unwrap_err();
        match err {
            MetaError::LayoutNotFound(p) => assert!(p.contains("layouts/nope.tsx")),
            other => unreachable!("expected LayoutNotFound, got {other:?}"),
        }
    }

    // ---- resolve_meta: default-layout convention --------------------------

    #[test]
    fn default_layout_picks_parent_dir_match_first() {
        let dir = project_skeleton();
        let root = dir.path();
        let blog = root.join("layouts/blog.tsx");
        let default = root.join("layouts/default.tsx");
        touch(&blog);
        touch(&default);

        let page = root.join("pages/blog/[slug].tsx");
        let candidates = vec![blog.clone(), default];
        let resolved = resolve_meta(PageMeta::default(), &page, root, &candidates).unwrap();
        assert_eq!(resolved.layout_path.as_deref(), Some(blog.as_path()));
    }

    #[test]
    fn default_layout_falls_through_to_default_tsx() {
        let dir = project_skeleton();
        let root = dir.path();
        let blog = root.join("layouts/blog.tsx"); // not created
        let default = root.join("layouts/default.tsx");
        touch(&default);

        let page = root.join("pages/blog/[slug].tsx");
        let candidates = vec![blog, default.clone()];
        let resolved = resolve_meta(PageMeta::default(), &page, root, &candidates).unwrap();
        assert_eq!(resolved.layout_path.as_deref(), Some(default.as_path()));
    }

    #[test]
    fn default_layout_about_resolves_to_default() {
        let dir = project_skeleton();
        let root = dir.path();
        let default = root.join("layouts/default.tsx");
        touch(&default);

        let page = root.join("pages/about.tsx");
        let candidates = vec![default.clone()];
        let resolved = resolve_meta(PageMeta::default(), &page, root, &candidates).unwrap();
        assert_eq!(resolved.layout_path.as_deref(), Some(default.as_path()));
    }

    #[test]
    fn no_meta_no_candidates_yields_none() {
        let dir = project_skeleton();
        let root = dir.path();
        let page = root.join("pages/about.tsx");
        let resolved = resolve_meta(PageMeta::default(), &page, root, &[]).unwrap();
        assert!(resolved.layout_path.is_none());
    }

    #[test]
    fn no_meta_candidates_all_missing_yields_none() {
        let dir = project_skeleton();
        let root = dir.path();
        let page = root.join("pages/about.tsx");
        let candidates = vec![
            root.join("layouts/about.tsx"),
            root.join("layouts/default.tsx"),
        ];
        let resolved = resolve_meta(PageMeta::default(), &page, root, &candidates).unwrap();
        assert!(resolved.layout_path.is_none());
    }

    // ---- end-to-end through parse_meta + resolve_meta ---------------------

    // ---- output extension / content-type precedence -----------------

    #[test]
    fn derive_extension_frontmatter_beats_route() {
        // Frontmatter wins over filename convention.
        assert_eq!(derive_output_extension(Some("rss"), Some("xml")), "rss",);
    }

    #[test]
    fn derive_extension_falls_through_to_route() {
        // No frontmatter override → use the filename convention.
        assert_eq!(derive_output_extension(None, Some("xml")), "xml",);
    }

    #[test]
    fn derive_extension_default_html() {
        // Neither override nor convention → html default.
        assert_eq!(derive_output_extension(None, None), "html",);
    }

    #[test]
    fn derive_content_type_frontmatter_beats_default() {
        // Frontmatter override beats the extension default.
        assert_eq!(
            derive_content_type("xml", Some("application/rss+xml")),
            "application/rss+xml",
        );
    }

    #[test]
    fn derive_content_type_extension_defaults() {
        assert_eq!(
            derive_content_type("html", None),
            "text/html; charset=utf-8"
        );
        assert_eq!(derive_content_type("xml", None), "application/xml");
        assert_eq!(derive_content_type("rss", None), "application/rss+xml");
        assert_eq!(derive_content_type("json", None), "application/json");
        assert_eq!(
            derive_content_type("txt", None),
            "text/plain; charset=utf-8"
        );
    }

    #[test]
    fn derive_content_type_unknown_extension_falls_back() {
        // Unknown extensions get the permissive default; the page
        // author should set contentType explicitly in this case.
        let ct = derive_content_type("something-weird", None);
        assert_eq!(ct, DEFAULT_CONTENT_TYPE);
    }

    #[test]
    fn resolve_meta_with_overrides_threads_extension_and_content_type() {
        let dir = project_skeleton();
        let root = dir.path();
        let page = root.join("pages/about.tsx");

        let resolved = resolve_meta_with_overrides(
            PageMeta::default(),
            &page,
            root,
            &[],
            Some("rss".into()),
            Some("application/rss+xml".into()),
        )
        .unwrap();
        assert_eq!(resolved.extension.as_deref(), Some("rss"));
        assert_eq!(
            resolved.content_type.as_deref(),
            Some("application/rss+xml")
        );
    }

    #[test]
    fn resolve_meta_default_overrides_are_none() {
        let dir = project_skeleton();
        let root = dir.path();
        let page = root.join("pages/about.tsx");

        let resolved = resolve_meta(PageMeta::default(), &page, root, &[]).unwrap();
        assert!(resolved.extension.is_none());
        assert!(resolved.content_type.is_none());
    }

    #[test]
    fn parse_then_resolve_known_fields_round_trip() {
        // With deny_unknown_fields in place, only the three declared fields
        // are legal. This verifies the parse+resolve path for a valid meta
        // with all three fields set.
        let dir = project_skeleton();
        let root = dir.path();
        let layout = root.join("layouts/blog.tsx");
        touch(&layout);

        let v = json!({
            "title": "Post",
            "layout": "@/layouts/blog",
        });
        let meta = parse_meta(Some(&v)).unwrap();
        let page = root.join("pages/blog/[slug].tsx");
        let resolved = resolve_meta(meta, &page, root, &[]).unwrap();

        assert_eq!(resolved.meta.title.as_deref(), Some("Post"));
        assert_eq!(resolved.layout_path.as_deref(), Some(layout.as_path()));
    }

    #[test]
    fn parse_then_resolve_unknown_field_rejected() {
        // deny_unknown_fields: a frontmatter object with an unrecognised key
        // must fail at parse time so the page author sees an explicit error
        // rather than a mysteriously empty title / layout.
        let dir = project_skeleton();
        let root = dir.path();

        let v = json!({
            "title": "Post",
            "layout": "@/layouts/blog",
            "openGraph": { "image": "/og.png" },
        });
        let err = parse_meta(Some(&v)).unwrap_err();
        match err {
            MetaError::InvalidShape(_) => {}
            other => unreachable!("expected InvalidShape, got {other:?}"),
        }
        let _ = root; // keep alive
    }

    // ---- is_within --------------------------------------------------------

    #[test]
    fn is_within_rejects_empty_project_root() {
        // A bare `Path::starts_with(empty)` is always true; we must
        // refuse to consider an empty `root` as containing anything,
        // otherwise the traversal guard is silently disabled.
        let cand = PathBuf::from("/etc/passwd");
        let root = PathBuf::new();
        assert!(!is_within(&cand, &root));
    }

    #[test]
    fn is_within_basic_containment() {
        let root = PathBuf::from("/project");
        assert!(is_within(&PathBuf::from("/project/pages/index.tsx"), &root));
        assert!(!is_within(&PathBuf::from("/other/file.tsx"), &root));
    }

    // ---- parity: derive_content_type must agree with zfb-server's
    // content_type_for_extension for the extensions both tables cover.
    // The two tables differ only in catch-all behaviour (render defaults to
    // HTML; server defaults to application/octet-stream), so we test only
    // the rows present in derive_content_type.  When adding an extension to
    // either table, add it here too.  Mirror: zfb_server::routes::content_type_for_extension.
    #[test]
    fn derive_content_type_parity_with_server_table() {
        let cases: &[(&str, &str)] = &[
            ("html", "text/html; charset=utf-8"),
            ("htm", "text/html; charset=utf-8"),
            ("xml", "application/xml"),
            ("rss", "application/rss+xml"),
            ("atom", "application/atom+xml"),
            ("json", "application/json"),
            ("map", "application/json"),
            ("webmanifest", "application/manifest+json"),
            ("txt", "text/plain; charset=utf-8"),
            ("css", "text/css; charset=utf-8"),
            ("js", "application/javascript; charset=utf-8"),
            ("mjs", "application/javascript; charset=utf-8"),
            ("cjs", "application/javascript; charset=utf-8"),
            ("wasm", "application/wasm"),
            ("svg", "image/svg+xml"),
            ("png", "image/png"),
            ("jpg", "image/jpeg"),
            ("jpeg", "image/jpeg"),
            ("webp", "image/webp"),
            ("avif", "image/avif"),
            ("gif", "image/gif"),
            ("ico", "image/x-icon"),
            ("woff", "font/woff"),
            ("woff2", "font/woff2"),
            ("ttf", "font/ttf"),
            ("otf", "font/otf"),
            ("eot", "application/vnd.ms-fontobject"),
            ("mp4", "video/mp4"),
            ("webm", "video/webm"),
            ("mp3", "audio/mpeg"),
            ("ogg", "audio/ogg"),
            ("pdf", "application/pdf"),
        ];
        for (ext, expected) in cases {
            assert_eq!(
                derive_content_type(ext, None),
                *expected,
                "derive_content_type({ext:?}) should match server table"
            );
        }
    }
}
