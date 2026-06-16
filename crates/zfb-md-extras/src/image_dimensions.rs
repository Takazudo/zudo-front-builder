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
//! # SVG support
//!
//! `imagesize` is raster-only and cannot decode vector SVGs. Rather than warn
//! on every `.svg` (issue #1083), this plugin reads an SVG's intrinsic
//! dimensions straight from its markup: an explicit `width`+`height` pair (in
//! user units or `px`) wins, otherwise the `viewBox`'s width/height provides
//! the aspect ratio. An SVG with no determinable dimensions is skipped
//! silently — no `width`/`height` and no warning.
//!
//! # Diagnostics
//!
//! When a referenced file cannot be found or a *raster* image cannot be probed
//! (unsupported format, truncated file, etc.), a `MarkdownDiagnostic::Warning`
//! is emitted via `ctx.diagnostics` and the `<img>` element is left unchanged.
//! Undimensionable SVGs do NOT warn (see *SVG support* above).
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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use zfb_md_ast::diagnostics::MarkdownDiagnostic;
use zfb_md_ast::{BuildContext, HastNode, HastVisitor, ImageDimensionsConfig, ReadRecorder};

use zfb_types::normalize_path_lexical;

/// Cached entry: `(mtime, width, height)`.
type CacheEntry = (SystemTime, u32, u32);

/// Hast visitor that injects `width`/`height` attributes on local `<img>` elements.
///
/// Raster formats use [`imagesize`] for header-only parsing — no full image
/// decode occurs. SVGs (which `imagesize` cannot decode) are parsed from their
/// markup via [`roxmltree`] for their intrinsic `width`/`height`/`viewBox`.
/// The cache is `Arc<Mutex<...>>` so the same cache can be shared across
/// multiple documents processed by the same pipeline instance (though today
/// each document gets a fresh plugin — the Arc is future-proofing).
pub struct ImageDimensionsPlugin {
    config: ImageDimensionsConfig,
    cache: Arc<Mutex<HashMap<PathBuf, CacheEntry>>>,
    /// Tracks how many actual disk reads were performed (not cache hits).
    /// Exposed via [`read_count`](Self::read_count) for test instrumentation.
    read_count: Arc<std::sync::atomic::AtomicUsize>,
    /// Optional read-recorder (zfb#944): when present, every image file
    /// whose dimensions shape the output — including missing/unprobeable
    /// files — is reported so the MDX compile cache can validate a
    /// dependency manifest. `None` keeps the pre-#944 behaviour exactly.
    recorder: Option<Arc<ReadRecorder>>,
}

impl ImageDimensionsPlugin {
    /// Create a new plugin instance with the given configuration.
    #[must_use]
    pub fn new(config: ImageDimensionsConfig) -> Self {
        Self {
            config,
            cache: Arc::new(Mutex::new(HashMap::new())),
            read_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            recorder: None,
        }
    }

    /// Attach the read-recorder this plugin reports image reads through
    /// (zfb#944). The SAME `Arc` must also be set on the pipeline via
    /// `Pipeline::set_read_recorder` so the compile-cache choke point can
    /// scope the recording per compile.
    #[must_use]
    pub fn with_recorder(mut self, recorder: Arc<ReadRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
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
        walk_node(
            node,
            ctx,
            &self.config,
            &self.cache,
            &self.read_count,
            self.recorder.as_deref(),
        );
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
    recorder: Option<&ReadRecorder>,
) {
    match node {
        HastNode::Root { children } | HastNode::Element { children, .. } => {
            for child in children.iter_mut() {
                try_inject_dimensions(child, ctx, config, cache, read_count, recorder);
                walk_node(child, ctx, config, cache, read_count, recorder);
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
    recorder: Option<&ReadRecorder>,
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

    // Record the dependency read (zfb#944) BEFORE probing, regardless of
    // the mtime cache below: the emitted width/height (or the warning
    // diagnostic) depends on this file's on-disk state every compile.
    // `record_file` hashes the COMPLETE file — the header sniff below
    // consumes only a prefix, but the recorded hash must cover the whole
    // file per the read-recorder contract. A missing file records
    // `Missing` (so creating it later invalidates a cached entry); an
    // unreadable one records `Error` (never cached).
    if let Some(r) = recorder {
        let _ = r.record_file(&abs_path);
    }

    // Probe dimensions (from cache or disk).
    match probe_dimensions(&abs_path, cache, read_count) {
        Ok(Some((width, height))) => {
            attrs.push(("width".to_string(), width.to_string()));
            attrs.push(("height".to_string(), height.to_string()));
        }
        // SVG with no determinable intrinsic dimensions: leave the `<img>`
        // unchanged and emit nothing — warning per-SVG was pure noise (#1083).
        Ok(None) => {}
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
/// Returns:
/// - `Ok(Some((width, height)))` — dimensions resolved (raster header or SVG
///   markup) and ready to inject.
/// - `Ok(None)` — the file is an SVG with no determinable intrinsic dimensions;
///   the caller skips it silently (no warning — #1083).
/// - `Err(msg)` — a genuine failure (missing/unreadable file, or a raster image
///   that cannot be probed); the caller surfaces it as a warning.
///
/// Cache entries include the file's `mtime` so that edits during the dev loop
/// invalidate stale entries without requiring a plugin restart. The `Ok(None)`
/// SVG case is not cached (the cache stores concrete `u32` dimensions only).
fn probe_dimensions(
    path: &PathBuf,
    cache: &Arc<Mutex<HashMap<PathBuf, CacheEntry>>>,
    read_count: &Arc<std::sync::atomic::AtomicUsize>,
) -> Result<Option<(u32, u32)>, String> {
    // Read current mtime (cheap syscall). A missing/unreadable file fails here
    // — including a missing `.svg`, which still surfaces as a broken-reference
    // warning rather than being silently skipped.
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map_err(|e| format!("cannot stat '{}': {e}", path.display()))?;

    // Check cache.
    {
        let guard = cache.lock().unwrap();
        if let Some(&(cached_mtime, w, h)) = guard.get(path) {
            if cached_mtime == mtime {
                return Ok(Some((w, h)));
            }
        }
    }

    // Cache miss — read the file.
    read_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let (w, h) = if is_svg(path) {
        // Vector image: `imagesize` cannot decode it, so read the SVG's own
        // intrinsic dimensions from its markup. A `.svg` that exists but
        // carries no usable dimensions returns `Ok(None)` (silent skip); a
        // read error (e.g. the file vanished between stat and read) warns.
        let bytes =
            std::fs::read(path).map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
        // SVG is UTF-8 in practice; a non-UTF-8 file (e.g. a rare UTF-16
        // export) decodes lossily, fails to parse, and degrades to a silent
        // skip — never a crash or a wrong dimension.
        let content = String::from_utf8_lossy(&bytes);
        match parse_svg_dimensions(&content) {
            Some(dims) => dims,
            None => return Ok(None),
        }
    } else {
        // Raster image: header-only probe.
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
        (w, h)
    };

    // Store in cache.
    {
        let mut guard = cache.lock().unwrap();
        guard.insert(path.clone(), (mtime, w, h));
    }

    Ok(Some((w, h)))
}

/// Returns `true` when `path` has a `.svg` extension (case-insensitive).
fn is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

/// Parse intrinsic pixel dimensions from an SVG document.
///
/// Priority mirrors the `image-size` npm package used by `rehype-img-size`
/// (the library this plugin ports): an explicit `width`+`height` pair in user
/// units (unitless) or `px` wins; otherwise the `viewBox`'s width/height
/// provides the intrinsic aspect ratio. Other length units (`%`, `mm`, `cm`,
/// `pt`, …) on `width`/`height` are ignored in favour of the `viewBox`, which
/// CAD/diagram exporters (KiCad, Inkscape) always emit — so the aspect ratio is
/// preserved even when the physical size is expressed in millimetres.
///
/// Returns `None` (→ silent skip) when the markup is not parseable XML, the
/// root element is not `<svg>`, or no usable dimensions can be determined.
fn parse_svg_dimensions(content: &str) -> Option<(u32, u32)> {
    // `allow_dtd` lets the parser skip past a `<!DOCTYPE svg …>` prolog that
    // older Inkscape/editor exports emit; without it such files would fail to
    // parse and lose their dimensions.
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    let doc = roxmltree::Document::parse_with_options(content, opts).ok()?;
    let root = doc.root_element();
    // Compare the local name so a namespaced root (`<svg:svg>`) also matches.
    if root.tag_name().name() != "svg" {
        return None;
    }

    // Read the three attributes by local name (namespace-agnostic).
    let mut width_attr = None;
    let mut height_attr = None;
    let mut viewbox_attr = None;
    for a in root.attributes() {
        match a.name() {
            "width" => width_attr = Some(a.value()),
            "height" => height_attr = Some(a.value()),
            "viewBox" => viewbox_attr = Some(a.value()),
            _ => {}
        }
    }

    let width = width_attr.and_then(parse_svg_length_px);
    let height = height_attr.and_then(parse_svg_length_px);
    if let (Some(w), Some(h)) = (width, height) {
        return Some((w, h));
    }

    viewbox_attr.and_then(parse_viewbox_dimensions)
}

/// Parse an SVG/CSS length as integer pixels, accepting only unitless values
/// (SVG user units == px) and an explicit `px` suffix (case-insensitive — CSS
/// units are case-insensitive). Percentages and physical units (`mm`, `cm`,
/// `pt`, `in`, `em`, …) return `None` so the caller falls back to the `viewBox`.
fn parse_svg_length_px(raw: &str) -> Option<u32> {
    let s = raw.trim();
    let num = if s.len() >= 2 && s.as_bytes()[s.len() - 2..].eq_ignore_ascii_case(b"px") {
        &s[..s.len() - 2]
    } else {
        s
    };
    round_dimension(num.trim().parse::<f64>().ok()?)
}

/// Parse `viewBox="min-x min-y width height"` and return `(width, height)`
/// rounded to pixels. Values may be separated by whitespace and/or commas
/// (`"0 0 100 50"` and `"0,0,100,50"` are both legal). Returns `None` unless
/// exactly four finite numbers are present and width/height are usable.
fn parse_viewbox_dimensions(raw: &str) -> Option<(u32, u32)> {
    let nums: Vec<f64> = raw
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if nums.len() != 4 {
        return None;
    }
    Some((round_dimension(nums[2])?, round_dimension(nums[3])?))
}

/// Round a floating-point dimension to the nearest pixel, rejecting values that
/// are not finite, below 1px, or beyond `u32` range. Mirrors the spirit of the
/// raster path's `u32::try_from` out-of-range guard.
fn round_dimension(v: f64) -> Option<u32> {
    if !v.is_finite() || v < 1.0 || v > f64::from(u32::MAX) {
        return None;
    }
    Some(v.round() as u32)
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
            cross_file_links: None,
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

    // ── SVG dimension parsing (#1083) ─────────────────────────────────────────

    #[test]
    fn is_svg_matches_extension_case_insensitively() {
        assert!(is_svg(Path::new("/a/b/diagram.svg")));
        assert!(is_svg(Path::new("/a/b/DIAGRAM.SVG")));
        assert!(is_svg(Path::new("photo.Svg")));
        assert!(!is_svg(Path::new("photo.png")));
        assert!(!is_svg(Path::new("noext")));
    }

    #[test]
    fn svg_explicit_width_height_px() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="80"></svg>"#;
        assert_eq!(parse_svg_dimensions(svg), Some((120, 80)));
    }

    #[test]
    fn svg_explicit_width_height_with_px_unit() {
        let svg = r#"<svg width="120px" height="80px"></svg>"#;
        assert_eq!(parse_svg_dimensions(svg), Some((120, 80)));
    }

    #[test]
    fn svg_viewbox_only() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100"></svg>"#;
        assert_eq!(parse_svg_dimensions(svg), Some((200, 100)));
    }

    #[test]
    fn svg_viewbox_comma_separated() {
        let svg = r#"<svg viewBox="0,0,200,100"></svg>"#;
        assert_eq!(parse_svg_dimensions(svg), Some((200, 100)));
    }

    /// KiCad/Inkscape style: physical `mm` width/height alongside a user-unit
    /// `viewBox` → fall back to the `viewBox` (preserves aspect ratio).
    #[test]
    fn svg_non_px_units_fall_back_to_viewbox() {
        let svg = r#"<svg width="210mm" height="297mm" viewBox="0 0 210 297"></svg>"#;
        assert_eq!(parse_svg_dimensions(svg), Some((210, 297)));
    }

    #[test]
    fn svg_percent_width_falls_back_to_viewbox() {
        let svg = r#"<svg width="100%" height="100%" viewBox="0 0 64 48"></svg>"#;
        assert_eq!(parse_svg_dimensions(svg), Some((64, 48)));
    }

    /// No `width`/`height` and no `viewBox` → undimensionable → `None`
    /// (the caller skips silently; this is the core of #1083).
    #[test]
    fn svg_no_dimensions_returns_none() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>"#;
        assert_eq!(parse_svg_dimensions(svg), None);
    }

    /// XML declaration + comment (itself containing a `<svg>` token) + an
    /// external-DTD DOCTYPE before the root element must all be skipped.
    #[test]
    fn svg_with_xml_decl_comment_and_doctype_before_root() {
        let svg = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!-- Generator: exporter; mentions <svg> here -->\n",
            "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" ",
            "\"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">\n",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"300\" height=\"150\"></svg>\n",
        );
        assert_eq!(parse_svg_dimensions(svg), Some((300, 150)));
    }

    #[test]
    fn svg_namespaced_root_element() {
        let svg =
            r#"<svg:svg xmlns:svg="http://www.w3.org/2000/svg" viewBox="0 0 10 5"></svg:svg>"#;
        assert_eq!(parse_svg_dimensions(svg), Some((10, 5)));
    }

    #[test]
    fn svg_single_quoted_attrs() {
        let svg = "<svg width='40' height='20'></svg>";
        assert_eq!(parse_svg_dimensions(svg), Some((40, 20)));
    }

    #[test]
    fn svg_malformed_or_non_svg_returns_none() {
        assert_eq!(parse_svg_dimensions("<svg width=\"10\""), None); // unclosed
        assert_eq!(parse_svg_dimensions("not xml at all"), None);
        assert_eq!(parse_svg_dimensions(""), None);
        assert_eq!(parse_svg_dimensions(r#"<html><body/></html>"#), None);
    }

    #[test]
    fn svg_rounds_fractional_viewbox() {
        let svg = r#"<svg viewBox="0 0 100.4 50.6"></svg>"#;
        assert_eq!(parse_svg_dimensions(svg), Some((100, 51)));
    }

    #[test]
    fn svg_degenerate_zero_viewbox_returns_none() {
        let svg = r#"<svg viewBox="0 0 0 0"></svg>"#;
        assert_eq!(parse_svg_dimensions(svg), None);
    }

    #[test]
    fn parse_svg_length_px_unit_handling() {
        assert_eq!(parse_svg_length_px("100"), Some(100));
        assert_eq!(parse_svg_length_px("100px"), Some(100));
        // CSS/SVG units are case-insensitive.
        assert_eq!(parse_svg_length_px("100PX"), Some(100));
        assert_eq!(parse_svg_length_px("100Px"), Some(100));
        assert_eq!(parse_svg_length_px("  100.5 "), Some(101));
        assert_eq!(parse_svg_length_px("100%"), None);
        assert_eq!(parse_svg_length_px("10mm"), None);
        assert_eq!(parse_svg_length_px("0"), None);
        assert_eq!(parse_svg_length_px(""), None);
        assert_eq!(parse_svg_length_px("abc"), None);
    }

    #[test]
    fn round_dimension_guards() {
        assert_eq!(round_dimension(1.0), Some(1));
        assert_eq!(round_dimension(99.5), Some(100));
        assert_eq!(round_dimension(0.4), None);
        assert_eq!(round_dimension(0.0), None);
        assert_eq!(round_dimension(-5.0), None);
        assert_eq!(round_dimension(f64::NAN), None);
        assert_eq!(round_dimension(f64::INFINITY), None);
        assert_eq!(round_dimension(f64::from(u32::MAX) * 2.0), None);
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
            cross_file_links: None,
        };
        try_inject_dimensions(&mut node, &mut ctx, &config, &cache, &read_count, None);
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

    // ── Read recording (zfb#944) ──────────────────────────────────────────

    /// Run the plugin (with a recorder) over `<img src=…>` rooted at a
    /// tempdir and return the recorded reads.
    fn record_reads_for_src(
        src: &str,
        dir: &std::path::Path,
        source_path: &std::path::Path,
    ) -> std::collections::BTreeMap<PathBuf, zfb_md_ast::ReadOutcome> {
        let recorder = Arc::new(ReadRecorder::new());
        let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default())
            .with_recorder(Arc::clone(&recorder));
        let mut node = HastNode::Root {
            children: vec![img(&[("src", src)])],
        };
        let mut ctx = BuildContext {
            source_path: Some(source_path.to_path_buf()),
            project_root: dir.to_path_buf(),
            public_dir: dir.join("public"),
            heading_registry: None,
            diagnostics: None,
            cross_file_links: None,
        };
        plugin.visit_with_context(&mut node, &mut ctx);
        recorder.take_reads()
    }

    #[test]
    fn probed_image_records_full_content_hash() {
        let dir = tempdir::TempDir::new("imgdim_rec_ok").unwrap();
        // Not a real image — the probe fails with a warning, but the READ
        // is still recorded with the full-content hash: the warning
        // outcome depends on these bytes too.
        let bytes = b"not really a png";
        std::fs::write(dir.path().join("pic.png"), bytes).unwrap();
        let source = dir.path().join("page.mdx");

        let reads = record_reads_for_src("./pic.png", dir.path(), &source);
        assert_eq!(
            reads.get(&dir.path().join("pic.png")),
            Some(&zfb_md_ast::ReadOutcome::of_bytes(bytes)),
            "the image read must record the FULL file hash: {reads:?}"
        );
    }

    #[test]
    fn missing_image_records_missing_outcome() {
        let dir = tempdir::TempDir::new("imgdim_rec_missing").unwrap();
        let source = dir.path().join("page.mdx");
        let reads = record_reads_for_src("./absent.png", dir.path(), &source);
        assert_eq!(
            reads.get(&dir.path().join("absent.png")),
            Some(&zfb_md_ast::ReadOutcome::Missing),
            "a missing image must record Missing so creating it later \
             invalidates a cached entry: {reads:?}"
        );
    }

    #[test]
    fn remote_and_data_srcs_record_nothing() {
        let dir = tempdir::TempDir::new("imgdim_rec_remote").unwrap();
        let source = dir.path().join("page.mdx");
        for src in [
            "https://example.com/x.png",
            "http://example.com/x.png",
            "data:image/png;base64,abc",
        ] {
            let reads = record_reads_for_src(src, dir.path(), &source);
            assert!(
                reads.is_empty(),
                "non-filesystem src {src:?} must not record reads: {reads:?}"
            );
        }
    }

    #[test]
    fn mtime_cache_hit_still_records_the_read() {
        // The per-plugin mtime cache may skip the header sniff, but the
        // compile-cache manifest still needs the dependency recorded on
        // every compile — the OUTPUT depends on the file each time.
        let dir = tempdir::TempDir::new("imgdim_rec_cachehit").unwrap();
        let bytes = b"bytes";
        std::fs::write(dir.path().join("pic.png"), bytes).unwrap();
        let source = dir.path().join("page.mdx");

        let recorder = Arc::new(ReadRecorder::new());
        let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default())
            .with_recorder(Arc::clone(&recorder));
        let mut ctx_holder = |recorder: &Arc<ReadRecorder>| {
            let mut node = HastNode::Root {
                children: vec![img(&[("src", "./pic.png")])],
            };
            let mut ctx = BuildContext {
                source_path: Some(source.clone()),
                project_root: dir.path().to_path_buf(),
                public_dir: dir.path().join("public"),
                heading_registry: None,
                diagnostics: None,
                cross_file_links: None,
            };
            plugin.visit_with_context(&mut node, &mut ctx);
            recorder.take_reads()
        };

        let first = ctx_holder(&recorder);
        assert!(first.contains_key(&dir.path().join("pic.png")));
        // Second visit on the SAME plugin (mtime cache warm) — the drain
        // above emptied the recorder, so a fresh record must appear.
        let second = ctx_holder(&recorder);
        assert!(
            second.contains_key(&dir.path().join("pic.png")),
            "a warm mtime cache must not skip read recording: {second:?}"
        );
    }

    // ── SVG end-to-end through the plugin (#1083) ─────────────────────────────
    //
    // The `tests/image_dimensions.rs` integration suite is gated behind the
    // `test-utils` feature, which `cargo test --workspace` (the PR gate) does
    // NOT enable — so those tests do not run in CI. These lib-level tests run
    // on every `cargo test`, so the full plugin path for SVGs is guarded by
    // the gate, not just the pure `parse_svg_dimensions` helper.

    /// Run the plugin over a single `<img src=…>` against `dir` and return the
    /// resulting `<img>` node plus any diagnostics emitted.
    fn run_plugin_collecting(
        src: &str,
        dir: &std::path::Path,
    ) -> (HastNode, Vec<MarkdownDiagnostic>) {
        use zfb_md_ast::diagnostics::CollectingSink;

        let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
        let mut tree = HastNode::Root {
            children: vec![img(&[("src", src)])],
        };
        let mut sink = CollectingSink::new();
        let mut ctx = BuildContext {
            source_path: Some(dir.join("page.mdx")),
            project_root: dir.to_path_buf(),
            public_dir: dir.to_path_buf(),
            heading_registry: None,
            diagnostics: Some(&mut sink),
            cross_file_links: None,
        };
        plugin.visit_with_context(&mut tree, &mut ctx);
        let HastNode::Root { children } = tree else {
            panic!("expected root")
        };
        (children.into_iter().next().unwrap(), sink.take())
    }

    #[test]
    fn plugin_injects_svg_viewbox_dimensions() {
        let dir = tempdir::TempDir::new("imgdim_svg_vb").unwrap();
        std::fs::write(
            dir.path().join("diagram.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100"></svg>"#,
        )
        .unwrap();
        let (img, diags) = run_plugin_collecting("diagram.svg", dir.path());
        assert_eq!(get_attr(&img, "width"), Some("200"));
        assert_eq!(get_attr(&img, "height"), Some("100"));
        assert!(diags.is_empty(), "a valid SVG must not warn: {diags:?}");
    }

    #[test]
    fn plugin_skips_undimensionable_svg_without_warning() {
        // The core of #1083: an SVG with no width/height/viewBox is left
        // unchanged and emits NO diagnostic (was: one warning per SVG).
        let dir = tempdir::TempDir::new("imgdim_svg_none").unwrap();
        std::fs::write(
            dir.path().join("plain.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>"#,
        )
        .unwrap();
        let (img, diags) = run_plugin_collecting("plain.svg", dir.path());
        assert_eq!(get_attr(&img, "width"), None);
        assert_eq!(get_attr(&img, "height"), None);
        assert!(
            diags.is_empty(),
            "an undimensionable SVG must emit no warning (#1083): {diags:?}"
        );
    }

    #[test]
    fn plugin_still_warns_on_raster_decode_failure() {
        use zfb_md_ast::diagnostics::DiagnosticSeverity;

        // Regression guard: a non-SVG file imagesize cannot decode must STILL
        // warn — the #1083 silence is scoped to SVGs only.
        let dir = tempdir::TempDir::new("imgdim_raster_bad").unwrap();
        std::fs::write(dir.path().join("broken.png"), b"not a real png").unwrap();
        let (img, diags) = run_plugin_collecting("broken.png", dir.path());
        assert_eq!(get_attr(&img, "width"), None);
        assert_eq!(diags.len(), 1, "raster decode failure must still warn");
        assert_eq!(diags[0].severity(), DiagnosticSeverity::Warning);
    }

    #[test]
    fn plugin_warns_on_missing_svg() {
        use zfb_md_ast::diagnostics::DiagnosticSeverity;

        // A missing `.svg` is a broken reference, not an undimensionable SVG —
        // the `fs::metadata` stat fails before the SVG branch, so it still
        // warns (it is NOT swallowed by the silent-skip path).
        let dir = tempdir::TempDir::new("imgdim_svg_missing").unwrap();
        let (img, diags) = run_plugin_collecting("absent.svg", dir.path());
        assert_eq!(get_attr(&img, "width"), None);
        assert_eq!(diags.len(), 1, "a missing SVG must still warn");
        assert_eq!(diags[0].severity(), DiagnosticSeverity::Warning);
    }

    #[test]
    fn plugin_caches_svg_on_second_reference() {
        // The SVG path participates in the mtime cache + read_count
        // instrumentation, exactly like the raster path.
        let dir = tempdir::TempDir::new("imgdim_svg_cache").unwrap();
        std::fs::write(
            dir.path().join("d.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 10"></svg>"#,
        )
        .unwrap();

        let mut plugin = ImageDimensionsPlugin::new(ImageDimensionsConfig::default());
        let mut tree = HastNode::Root {
            children: vec![img(&[("src", "d.svg")]), img(&[("src", "d.svg")])],
        };
        let mut ctx = BuildContext {
            source_path: Some(dir.path().join("page.mdx")),
            project_root: dir.path().to_path_buf(),
            public_dir: dir.path().to_path_buf(),
            heading_registry: None,
            diagnostics: None,
            cross_file_links: None,
        };
        plugin.visit_with_context(&mut tree, &mut ctx);

        assert_eq!(
            plugin.read_count(),
            1,
            "second SVG reference must hit the cache"
        );
        let HastNode::Root { children } = &tree else {
            panic!()
        };
        for child in children {
            assert_eq!(get_attr(child, "width"), Some("20"));
            assert_eq!(get_attr(child, "height"), Some("10"));
        }
    }
}
