//! Auto-detect and inject `width`/`height` attributes on `<img>` elements.
//!
//! Rust port of [`rehype-img-size`](https://www.npmjs.com/package/rehype-img-size).
//!
//! For each `<img>` whose `src` resolves to a local file, this plugin reads the
//! image header (without decoding the full payload) and injects `width` and
//! `height` attributes if they are not already present.
//!
//! # Source resolution
//!
//! | `src` form | Resolution |
//! |---|---|
//! | `http://…` / `https://…` / `data:…` | skipped silently (remote/data URLs) |
//! | `/img/foo.png` (absolute path) | resolved against `ctx.public_dir` |
//! | `./foo.png` / `foo.png` (relative) | resolved against the markdown source file's directory, or `ctx.project_root` as fallback |
//!
//! # Caching
//!
//! A per-plugin `Arc<Mutex<HashMap>>` caches `(absolute_path) → (mtime, width, height)`.
//! A second reference to the same file within the same build hits the cache
//! rather than re-reading the file header.
//!
//! # Diagnostics
//!
//! When a referenced file cannot be found or cannot be probed (unsupported
//! format, truncated file, etc.), a `MarkdownDiagnostic::Warning` is emitted
//! via `ctx.diagnostics` and the `<img>` element is left unchanged.
//!
//! # Configuration
//!
//! Wire via `features.imageDimensions: {}` in `zfb.config.ts`. Requires
//! `Pipeline::run_with_context` or `Pipeline::apply_hast_visitors_with_context`
//! — the plugin is a no-op when called via the context-free `visit` path.
//!
//! **Ordering:** must run BEFORE `SyntectPlugin` (which does not touch `<img>`).
//! Wired in `register_features` in `zfb-content::pipeline`.
//!
//! Ported in Wave 6 (#579).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use zfb_md_ast::diagnostics::MarkdownDiagnostic;
use zfb_md_ast::{BuildContext, HastNode, HastVisitor, ImageDimensionsConfig};

use zfb_types::normalize_path_lexical;

/// Cached entry: `(mtime, width, height)`.
type CacheEntry = (SystemTime, u32, u32);

/// Hast visitor that injects `width`/`height` attributes on local `<img>` elements.
///
/// Uses [`imagesize`] for header-only parsing — no full image decode occurs.
/// The cache is `Arc<Mutex<...>>` so the same cache can be shared across
/// multiple documents processed by the same pipeline instance (though today
/// each document gets a fresh plugin — the Arc is future-proofing).
pub struct ImageDimensionsPlugin {
    config: ImageDimensionsConfig,
    cache: Arc<Mutex<HashMap<PathBuf, CacheEntry>>>,
    /// Tracks how many actual disk reads were performed (not cache hits).
    /// Exposed via [`read_count`](Self::read_count) for test instrumentation.
    read_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl ImageDimensionsPlugin {
    /// Create a new plugin instance with the given configuration.
    #[must_use]
    pub fn new(config: ImageDimensionsConfig) -> Self {
        Self {
            config,
            cache: Arc::new(Mutex::new(HashMap::new())),
            read_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// How many times the plugin read image metadata from disk (not cache).
    ///
    /// Used in tests to verify cache-hit behaviour.
    #[must_use]
    pub fn read_count(&self) -> usize {
        self.read_count.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl HastVisitor for ImageDimensionsPlugin {
    /// Context-free visit: no-op (needs `BuildContext` to resolve paths).
    fn visit(&mut self, _node: &mut HastNode) {
        // Image-dimensions requires ctx.public_dir and ctx.source_path.
        // When called without context (e.g. Pipeline::run), skip silently.
    }

    /// Context-aware visit: resolves and injects dimensions.
    fn visit_with_context(&mut self, node: &mut HastNode, ctx: &mut BuildContext<'_>) {
        walk_node(node, ctx, &self.config, &self.cache, &self.read_count);
    }
}

/// Determine whether `skip_remote` is effectively `true`.
///
/// The spec requires remote images (http/https) to be skipped by default.
/// `skip_remote: None` (absent) is treated as `true`.
fn should_skip_remote(config: &ImageDimensionsConfig) -> bool {
    config.skip_remote.unwrap_or(true)
}

/// Recursively walk `node`, injecting dimensions on matching `<img>` elements.
fn walk_node(
    node: &mut HastNode,
    ctx: &mut BuildContext<'_>,
    config: &ImageDimensionsConfig,
    cache: &Arc<Mutex<HashMap<PathBuf, CacheEntry>>>,
    read_count: &Arc<std::sync::atomic::AtomicUsize>,
) {
    match node {
        HastNode::Root { children } | HastNode::Element { children, .. } => {
            for child in children.iter_mut() {
                try_inject_dimensions(child, ctx, config, cache, read_count);
                walk_node(child, ctx, config, cache, read_count);
            }
        }
        _ => {}
    }
}

/// If `node` is an `<img>` element with a local `src`, inject `width`/`height`
/// unless already present.
fn try_inject_dimensions(
    node: &mut HastNode,
    ctx: &mut BuildContext<'_>,
    config: &ImageDimensionsConfig,
    cache: &Arc<Mutex<HashMap<PathBuf, CacheEntry>>>,
    read_count: &Arc<std::sync::atomic::AtomicUsize>,
) {
    let HastNode::Element { tag, attrs, .. } = node else {
        return;
    };
    if tag != "img" {
        return;
    }

    // Skip if width OR height already present.
    if attrs.iter().any(|(k, _)| k == "width" || k == "height") {
        return;
    }

    // Find the `src` attribute.
    let src = match attrs.iter().find(|(k, _)| k == "src") {
        Some((_, v)) => v.clone(),
        None => return,
    };

    // Skip remote URLs when `skipRemote` is true (the default).
    if should_skip_remote(config)
        && (src.starts_with("http://") || src.starts_with("https://") || src.starts_with("//"))
    {
        return;
    }

    // Always skip data: URLs regardless of skipRemote (they are never on-disk files).
    if src.starts_with("data:") {
        return;
    }

    // Resolve the `src` to an absolute filesystem path.
    let abs_path = resolve_src(&src, ctx);
    let abs_path = match abs_path {
        Some(p) => p,
        None => {
            emit_warning(
                ctx,
                format!(
                    "imageDimensions: could not resolve src '{src}' — no source path available"
                ),
            );
            return;
        }
    };

    // Containment guard: reject paths that escape the expected root directory.
    // Absolute `/…` srcs are resolved into `public_dir`; relative srcs into
    // `project_root`. Lexically normalize to collapse any `..` components
    // injected via crafted src values (e.g. `../../etc/hosts`).
    let is_absolute_src = src.starts_with('/');
    let expected_root = if is_absolute_src {
        &ctx.public_dir
    } else {
        &ctx.project_root
    };
    let normalized = normalize_path_lexical(&abs_path);
    if !normalized.starts_with(expected_root) {
        emit_warning(
            ctx,
            format!(
                "imageDimensions: src '{src}' resolves outside the expected root '{}' — skipping",
                expected_root.display()
            ),
        );
        return;
    }

    // Probe dimensions (from cache or disk).
    match probe_dimensions(&abs_path, cache, read_count) {
        Ok((width, height)) => {
            attrs.push(("width".to_string(), width.to_string()));
            attrs.push(("height".to_string(), height.to_string()));
        }
        Err(err_msg) => {
            emit_warning(ctx, format!("imageDimensions: {err_msg}"));
        }
    }
}

/// Resolve `src` to an absolute [`PathBuf`].
///
/// - Absolute paths (starting with `/`) → joined to `ctx.public_dir`.
/// - Relative paths → joined to the markdown source file's parent directory,
///   falling back to `ctx.project_root` when `ctx.source_path` is absent.
fn resolve_src(src: &str, ctx: &BuildContext<'_>) -> Option<PathBuf> {
    if let Some(stripped) = src.strip_prefix('/') {
        // Public-directory asset: `/img/foo.png` → `<public_dir>/img/foo.png`.
        Some(ctx.public_dir.join(stripped))
    } else {
        // Relative to the markdown file's directory.
        let base = ctx
            .source_path
            .as_deref()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| ctx.project_root.clone());
        Some(base.join(src))
    }
}

/// Probe `path` for image dimensions.
///
/// Returns `(width, height)` on success, or an error string on failure.
///
/// Cache entries include the file's `mtime` so that edits during the dev loop
/// invalidate stale entries without requiring a plugin restart.
fn probe_dimensions(
    path: &PathBuf,
    cache: &Arc<Mutex<HashMap<PathBuf, CacheEntry>>>,
    read_count: &Arc<std::sync::atomic::AtomicUsize>,
) -> Result<(u32, u32), String> {
    // Read current mtime (cheap syscall).
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map_err(|e| format!("cannot stat '{}': {e}", path.display()))?;

    // Check cache.
    {
        let guard = cache.lock().unwrap();
        if let Some(&(cached_mtime, w, h)) = guard.get(path) {
            if cached_mtime == mtime {
                return Ok((w, h));
            }
        }
    }

    // Cache miss — read the file header.
    read_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let size = imagesize::size(path)
        .map_err(|e| format!("cannot probe dimensions of '{}': {e}", path.display()))?;

    // `imagesize` reports `usize` dimensions straight from the (untrusted)
    // file header — reject out-of-range values instead of truncating to
    // silently wrong width/height attributes.
    let (Ok(w), Ok(h)) = (u32::try_from(size.width), u32::try_from(size.height)) else {
        return Err(format!(
            "image dimensions of '{}' out of range: {}x{}",
            path.display(),
            size.width,
            size.height
        ));
    };

    // Store in cache.
    {
        let mut guard = cache.lock().unwrap();
        guard.insert(path.clone(), (mtime, w, h));
    }

    Ok((w, h))
}

/// Emit a warning via `ctx.diagnostics` (no-op when `diagnostics` is `None`).
fn emit_warning(ctx: &mut BuildContext<'_>, message: String) {
    if let Some(sink) = ctx.diagnostics.as_mut() {
        sink.emit(MarkdownDiagnostic::warning(message));
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn img(attrs: &[(&str, &str)]) -> HastNode {
        HastNode::Element {
            tag: "img".to_string(),
            attrs: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            children: vec![],
            void: true,
        }
    }

    fn get_attr<'a>(node: &'a HastNode, name: &str) -> Option<&'a str> {
        let HastNode::Element { attrs, .. } = node else {
            return None;
        };
        attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn skip_remote_http() {
        assert!(
            "http://example.com/foo.png".starts_with("http://")
                || "https://example.com/foo.png".starts_with("https://")
        );
    }

    #[test]
    fn skip_remote_https() {
        assert!("https://example.com/foo.png".starts_with("https://"));
    }

    #[test]
    fn skip_data_url() {
        assert!("data:image/png;base64,abc".starts_with("data:"));
    }

    #[test]
    fn attrs_with_existing_width_not_modified() {
        // An img that already has `width` must be left alone.
        let node = img(&[("src", "./foo.png"), ("width", "100")]);
        let has_width = matches!(&node, HastNode::Element { attrs, .. } if attrs.iter().any(|(k,_)| k == "width"));
        assert!(has_width);
    }

    #[test]
    fn resolve_src_absolute_uses_public_dir() {
        let ctx = BuildContext::for_paths("/project/src/page.mdx", "/project", "/project/public");
        let resolved = resolve_src("/img/foo.png", &ctx);
        assert_eq!(resolved, Some(PathBuf::from("/project/public/img/foo.png")));
    }

    #[test]
    fn resolve_src_relative_uses_source_dir() {
        let ctx = BuildContext::for_paths("/project/src/page.mdx", "/project", "/project/public");
        let resolved = resolve_src("./assets/foo.png", &ctx);
        assert_eq!(resolved, Some(PathBuf::from("/project/src/assets/foo.png")));
    }

    #[test]
    fn resolve_src_relative_no_leading_dot() {
        let ctx = BuildContext::for_paths("/project/src/page.mdx", "/project", "/project/public");
        let resolved = resolve_src("assets/foo.png", &ctx);
        assert_eq!(resolved, Some(PathBuf::from("/project/src/assets/foo.png")));
    }

    #[test]
    fn resolve_src_fallback_to_project_root_when_no_source_path() {
        let ctx = BuildContext {
            source_path: None,
            project_root: PathBuf::from("/project"),
            public_dir: PathBuf::from("/project/public"),
            heading_registry: None,
            diagnostics: None,
        };
        let resolved = resolve_src("./foo.png", &ctx);
        assert_eq!(resolved, Some(PathBuf::from("/project/foo.png")));
    }

    #[test]
    fn should_skip_remote_defaults_true() {
        let cfg = ImageDimensionsConfig { skip_remote: None };
        assert!(should_skip_remote(&cfg));
    }

    #[test]
    fn should_skip_remote_explicit_true() {
        let cfg = ImageDimensionsConfig {
            skip_remote: Some(true),
        };
        assert!(should_skip_remote(&cfg));
    }

    #[test]
    fn should_skip_remote_explicit_false() {
        let cfg = ImageDimensionsConfig {
            skip_remote: Some(false),
        };
        assert!(!should_skip_remote(&cfg));
    }

    #[test]
    fn visit_is_noop_without_context() {
        // The context-free `visit` must not panic and must not change the tree.
        let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
        let original = HastNode::Root {
            children: vec![img(&[("src", "./foo.png")])],
        };
        let mut node = original.clone();
        plugin.visit(&mut node);
        // visit() is a no-op when no context is available.
        assert_eq!(node, original, "visit() must leave tree unchanged");
    }

    /// Verify that `get_attr` helper works on our `img` helper.
    #[test]
    fn get_attr_finds_existing() {
        let node = img(&[("src", "foo.png"), ("alt", "desc")]);
        assert_eq!(get_attr(&node, "src"), Some("foo.png"));
        assert_eq!(get_attr(&node, "alt"), Some("desc"));
        assert_eq!(get_attr(&node, "width"), None);
    }

    // ── Fix #703: containment guard tests ────────────────────────────────────

    /// Helper: run `try_inject_dimensions` for a given src and return collected warnings.
    fn collect_warnings_for_src(
        src: &str,
        project_root: &str,
        public_dir: &str,
        source_path: &str,
    ) -> Vec<String> {
        use zfb_md_ast::diagnostics::CollectingSink;

        let mut node = img(&[("src", src)]);
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let read_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let config = ImageDimensionsConfig::default();

        let mut sink = CollectingSink::new();
        let mut ctx = zfb_md_ast::BuildContext {
            source_path: Some(PathBuf::from(source_path)),
            project_root: PathBuf::from(project_root),
            public_dir: PathBuf::from(public_dir),
            heading_registry: None,
            diagnostics: Some(&mut sink),
        };
        try_inject_dimensions(&mut node, &mut ctx, &config, &cache, &read_count);
        sink.take()
            .into_iter()
            .filter_map(|d| match d {
                zfb_md_ast::diagnostics::MarkdownDiagnostic::Generic { message, .. } => {
                    Some(message)
                }
                _ => None,
            })
            .collect()
    }

    /// A relative src that escapes the project root via `../../` must be rejected
    /// with a containment warning and must NOT trigger a disk probe.
    #[test]
    fn relative_path_traversal_rejected_with_warning() {
        // `../../etc/hosts` resolves outside project_root → containment warning.
        let warnings = collect_warnings_for_src(
            "../../etc/hosts",
            "/project",
            "/project/public",
            "/project/src/page.mdx",
        );
        assert!(
            !warnings.is_empty(),
            "expected a containment warning for path-traversal src"
        );
        assert!(
            warnings[0].contains("resolves outside the expected root"),
            "warning should mention containment: {:?}",
            warnings[0]
        );
    }

    /// An absolute src that escapes `public_dir` via `/../../` must be rejected.
    #[test]
    fn absolute_path_traversal_rejected_with_warning() {
        // `/../../etc/hosts` → join public_dir `/project/public` →
        // `/project/public/../../etc/hosts` → normalizes to `/etc/hosts`
        // → not under `/project/public` → containment warning.
        let warnings = collect_warnings_for_src(
            "/../../etc/hosts",
            "/project",
            "/project/public",
            "/project/src/page.mdx",
        );
        assert!(
            !warnings.is_empty(),
            "expected a containment warning for absolute path-traversal src"
        );
        assert!(
            warnings[0].contains("resolves outside the expected root"),
            "warning should mention containment: {:?}",
            warnings[0]
        );
    }
}
