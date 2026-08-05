//! Sourcemap-driven attribution of relative `url()` references in the
//! compiled Tailwind output, plus the hard-error floor for package-attributed
//! references (issue #2315, epic #2311).
//!
//! ## The problem this closes
//!
//! Tailwind inlines `@import`ed package stylesheets (e.g.
//! `@fontsource-variable/noto-sans/index.css`) into the compiled output
//! byte-for-byte, so a relative `url(./files/x.woff2)` inside one now resolves
//! against the built CSS location and 404s at runtime — while the build exits
//! 0. This module makes that state impossible: every relative `url()` in the
//! compiled output is attributed to its origin stylesheet via the compiler's
//! own sourcemap, and every reference attributed to a `node_modules`
//! stylesheet **fails the build loudly** (asset emission is a follow-up wave —
//! until it lands, the floor error below is the guaranteed behaviour).
//!
//! ## Why sourcemap-driven (not text matching, not a Rust re-resolver)
//!
//! Tailwind/Lightning CSS stays the *only* resolver: zfb reads the resolution
//! the compiler already performed, recorded in Source Map v3 (`--map` on the
//! CLI invocation, inline base64 data-URL comment). There is no parity
//! surface to chase and no decoy-matching hazard — the #2312 diagnosis ran
//! the adversarial cases (identical `url()` text in authored + imported CSS,
//! in two different imported files, literal duplicates) and position-based
//! lookup attributed every one distinctly. Text search provably cannot.
//!
//! ## Attribution rule (locked in #2313, decision b)
//!
//! For each **relative** reference the scanner reports (`./x`, `../x`, bare
//! `files/x` — `data:`, absolute `/`, any scheme, protocol-relative `//`,
//! fragment-only, query-only, and empty are untouched and never error):
//!
//! 1. span start → (generated line, UTF-16 column); sourcemap lookup of the
//!    mapping segment at-or-before that position **on that line** — a miss
//!    (no segment on the line at-or-before the column) fails closed with the
//!    attribution-anomaly error.
//! 2. resolve `sources[idx]` (the `sourcemap` crate applies `sourceRoot`),
//!    canonicalize.
//! 3. classify: a `node_modules` path component → package stylesheet → the
//!    hard-error floor; none → authored/project CSS → byte-for-byte
//!    passthrough (the documented `static-assets.mdx` contract, unchanged).
//!    A pnpm-workspace-linked local package canonicalizes outside
//!    `node_modules` and is therefore authored — the user's own code keeps
//!    the documented contract.
//! 4. package identity: walk up from the canonical source to the nearest
//!    `package.json`, read `name`/`version` (missing/unreadable/nameless →
//!    hard error naming the source path).
//!
//! **Trust boundary (accepted residual, in writing):** attribution trusts
//! the compiler's own mapping. A wrong mapping (compiler bug) would attribute
//! wrongly, but resolving against the wrong directory almost always finds no
//! file → hard error, not silent corruption. The accepted residual is the
//! double coincidence (wrong mapping AND a same-relative-path file existing
//! there). Valid only while zfb never passes `-m`/`--optimize` — Lightning
//! CSS's rule merging is coupled to those flags and destroys the
//! one-declaration-per-line mapping shape. If minification is ever added to
//! this pipeline, re-verify #2312 Q4d before trusting attribution.
//!
//! ## Containment (decision e)
//!
//! A package-attributed reference's target — canonicalized first, so a
//! symlink escaping the package resolves outside — must be a regular file
//! inside the package root (the attributed `package.json`'s directory).
//! A package stylesheet can never make zfb touch a path outside that
//! package's own directory.

use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Result};
use base64::Engine as _;
use sourcemap::SourceMap;

use crate::url_scanner::{scan_css_urls, CssUrlOccurrence};

/// The origin a relative `url()` reference was attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlOrigin {
    /// Authored/project CSS (the entry and project-local `@import`ed files,
    /// including Tailwind's own bundled internals): byte-for-byte
    /// passthrough, never an error.
    Authored {
        /// Canonical source path when it resolves; the sourcemap's own
        /// (lexically resolved) path otherwise.
        source: PathBuf,
    },
    /// A stylesheet inlined from `node_modules` — subject to the hard-error
    /// floor until package asset emission lands.
    Package(PackageOrigin),
}

/// Identity of the package a reference was attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageOrigin {
    /// Canonical path of the stylesheet inside the package.
    pub source: PathBuf,
    /// The directory containing the attributed `package.json`.
    pub package_root: PathBuf,
    /// `package.json` `name`.
    pub name: String,
    /// `package.json` `version` (`"unknown"` when absent).
    pub version: String,
}

/// One relative `url()` occurrence with its attributed origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributedUrl {
    /// The scanner occurrence (exact byte span + decoded value).
    pub occurrence: CssUrlOccurrence,
    /// 0-based generated line of the value span start.
    pub line: u32,
    /// Where the reference came from, per the compiler's sourcemap.
    pub origin: UrlOrigin,
}

/// Engine entry point: strip the trailing `sourceMappingURL` comment from the
/// raw Tailwind output, attribute every relative `url()` through the parsed
/// sourcemap, and enforce the package hard-error floor. Returns the stripped
/// CSS — byte-identical to the pre-`--map` output, since the comment is
/// trailing and stripping shifts no earlier offsets.
///
/// `base_dir` anchors relative sourcemap `sources` entries (the engine passes
/// its subprocess working directory; Tailwind emits absolute paths in
/// practice).
pub fn attribute_and_enforce_package_url_floor(raw_css: &str, base_dir: &Path) -> Result<String> {
    let (css, map_url) = split_sourcemap_comment(raw_css);
    let map = match map_url.and_then(inline_data_url_payload) {
        Some(payload) => Some(parse_inline_sourcemap(payload)?),
        None => None,
    };
    let attributed = attribute_relative_urls(css, map.as_ref(), base_dir)?;
    enforce_package_url_floor(css, &attributed)?;
    Ok(css.to_string())
}

/// The comment Tailwind's `--map` appends to the output file. Everything
/// before it is the exact CSS text a `--map`-less invocation would produce.
const SOURCEMAP_COMMENT_PREFIX: &str = "/*# sourceMappingURL=";

/// Split a trailing `/*# sourceMappingURL=... */` comment off `raw`, returning
/// the stripped CSS slice and the comment's URL when present. The comment is
/// only recognised in trailing position (nothing but whitespace after its
/// `*/`) — a lookalike in the middle of the text is someone's content, not
/// the compiler's annotation, and is left alone.
pub fn split_sourcemap_comment(raw: &str) -> (&str, Option<&str>) {
    let Some(start) = raw.rfind(SOURCEMAP_COMMENT_PREFIX) else {
        return (raw, None);
    };
    let after = &raw[start + SOURCEMAP_COMMENT_PREFIX.len()..];
    let Some(end) = after.find("*/") else {
        return (raw, None);
    };
    if !after[end + 2..].trim().is_empty() {
        return (raw, None);
    }
    (&raw[..start], Some(after[..end].trim()))
}

/// Extract the base64 payload of an inline `data:application/json;base64,...`
/// sourcemap URL. Any other URL shape (an external `.map` reference) yields
/// `None` — the caller then has no map, and attribution fails closed if a
/// relative reference needs one.
fn inline_data_url_payload(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("data:")?;
    let comma = rest.find(',')?;
    let meta = &rest[..comma];
    if !meta.ends_with(";base64") {
        return None;
    }
    Some(&rest[comma + 1..])
}

/// Decode and parse an inline base64 sourcemap payload.
fn parse_inline_sourcemap(payload: &str) -> Result<SourceMap> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|e| anyhow!("invalid base64 in tailwind sourceMappingURL data URL: {e}"))?;
    SourceMap::from_slice(&bytes).map_err(|e| anyhow!("invalid sourcemap in tailwind output: {e}"))
}

/// Attribute every **relative** `url()` occurrence in `css` to its origin
/// stylesheet via `map`. Fails closed (attribution-anomaly error) when a
/// relative reference has no mapping segment at-or-before its column on its
/// generated line — including when there is no map at all.
///
/// No deduplication happens at any point: N occurrences yield N entries, each
/// attributed by its own generated position (the #2312 no-dedup finding —
/// zfb's unminified `-i/-o` invocation preserves literal duplicates).
pub fn attribute_relative_urls(
    css: &str,
    map: Option<&SourceMap>,
    base_dir: &Path,
) -> Result<Vec<AttributedUrl>> {
    let mut out = Vec::new();
    for occurrence in scan_css_urls(css) {
        if !is_relative_reference(&occurrence.decoded) {
            continue;
        }
        let (line, col) = generated_position(css, occurrence.value_span.start);
        let source = map
            .and_then(|m| m.lookup_token(line, col))
            // `lookup_token` is a greatest-lower-bound over the whole map: a
            // hit on an EARLIER line means this line has no segment
            // at-or-before the column (zero-mapping line, or a position
            // before the line's first segment) — both fail closed.
            .filter(|token| token.get_dst_line() == line)
            .and_then(|token| token.get_source().map(str::to_owned))
            .ok_or_else(|| attribution_anomaly_error(css, &occurrence.value_span, line))?;
        let resolved = resolve_source_path(&source, base_dir);
        let canonical = std::fs::canonicalize(&resolved).unwrap_or(resolved);
        let origin = if has_node_modules_component(&canonical) {
            UrlOrigin::Package(package_identity(&canonical)?)
        } else {
            UrlOrigin::Authored { source: canonical }
        };
        out.push(AttributedUrl {
            occurrence,
            line,
            origin,
        });
    }
    Ok(out)
}

/// The hard-error floor (locked decision d): every package-attributed
/// relative reference fails the build. A target that is missing, not a
/// regular file, or escapes the package root gets the emission-error shape
/// naming the reason; a valid, contained target gets the floor error —
/// asset emission is not implemented yet, and exiting 0 with a reference
/// that will 404 is the exact reported harm.
fn enforce_package_url_floor(css: &str, attributed: &[AttributedUrl]) -> Result<()> {
    for entry in attributed {
        let UrlOrigin::Package(pkg) = &entry.origin else {
            continue;
        };
        let raw_reference = &css[entry.occurrence.value_span.clone()];
        let (path_part, _suffix) = split_query_fragment(&entry.occurrence.decoded);
        let source_dir = pkg.source.parent().unwrap_or(Path::new("."));
        let target = source_dir.join(path_part);
        // Containment: canonicalize FIRST (resolving symlinks), so a symlink
        // escaping the package resolves outside the root and is rejected.
        let reason = match std::fs::canonicalize(&target) {
            Err(_) => Some(format!("file not found at {}", target.display())),
            Ok(asset_canonical) => {
                let package_root = std::fs::canonicalize(&pkg.package_root)
                    .unwrap_or_else(|_| pkg.package_root.clone());
                if !asset_canonical.starts_with(&package_root) {
                    Some(format!(
                        "resolves outside the package directory: {}",
                        asset_canonical.display()
                    ))
                } else if !asset_canonical.is_file() {
                    Some(format!("not a regular file: {}", asset_canonical.display()))
                } else {
                    None
                }
            }
        };
        return Err(match reason {
            Some(reason) => cannot_emit_error(pkg, raw_reference, &reason),
            None => floor_error(pkg, raw_reference),
        });
    }
    Ok(())
}

/// Whether a decoded `url()` value is a relative reference in scope for
/// attribution. Everything else is untouched byte-for-byte and never errors.
fn is_relative_reference(decoded: &str) -> bool {
    !(decoded.is_empty()
        // fragment-only (`#blur`)
        || decoded.starts_with('#')
        // query-only — no path part to resolve
        || decoded.starts_with('?')
        // absolute (`/img/x.png`) and protocol-relative (`//host/x.png`)
        || decoded.starts_with('/')
        // any scheme: `data:`, `https:`, `blob:`, ...
        || has_url_scheme(decoded))
}

/// RFC 3986 scheme detection: `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`.
fn has_url_scheme(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.first().is_none_or(|b| !b.is_ascii_alphabetic()) {
        return false;
    }
    for &b in &bytes[1..] {
        match b {
            b':' => return true,
            b if b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.') => {}
            _ => return false,
        }
    }
    false
}

/// Byte offset → (0-based line, UTF-16 column) in the generated text —
/// sourcemap columns count UTF-16 code units per the Source Map spec.
fn generated_position(css: &str, byte_offset: usize) -> (u32, u32) {
    let before = &css[..byte_offset];
    let line = before.bytes().filter(|&b| b == b'\n').count() as u32;
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col: usize = css[line_start..byte_offset]
        .chars()
        .map(char::len_utf16)
        .sum();
    (line, col as u32)
}

/// Resolve a sourcemap `sources` entry to a filesystem path: strip a
/// `file://` scheme, anchor relative entries on `base_dir`. (Tailwind emits
/// absolute paths in practice; its bundled internals use the non-existent
/// `/$bunfs/...` prefix, which classifies as authored via the
/// canonicalize-fallback path.)
fn resolve_source_path(source: &str, base_dir: &Path) -> PathBuf {
    let source = source.strip_prefix("file://").unwrap_or(source);
    let path = Path::new(source);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

/// Whether any path component is exactly `node_modules`.
fn has_node_modules_component(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::Normal(name) if name == "node_modules"))
}

/// Walk up from the canonical source path to the nearest `package.json` and
/// read the package identity. Missing/unreadable/nameless manifest → hard
/// error naming the source path (locked decision b, step 4).
fn package_identity(source_canonical: &Path) -> Result<PackageOrigin> {
    let mut dir = source_canonical.parent();
    while let Some(d) = dir {
        let manifest_path = d.join("package.json");
        if manifest_path.is_file() {
            let text = std::fs::read_to_string(&manifest_path).map_err(|e| {
                anyhow!(
                    "unreadable package.json at {} for imported package stylesheet {}: {e}",
                    manifest_path.display(),
                    source_canonical.display()
                )
            })?;
            let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
                anyhow!(
                    "invalid package.json at {} for imported package stylesheet {}: {e}",
                    manifest_path.display(),
                    source_canonical.display()
                )
            })?;
            let name = json
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow!(
                        "package.json at {} has no name — cannot identify the package owning \
                         imported stylesheet {}",
                        manifest_path.display(),
                        source_canonical.display()
                    )
                })?;
            let version = json
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Ok(PackageOrigin {
                source: source_canonical.to_path_buf(),
                package_root: d.to_path_buf(),
                name: name.to_string(),
                version: version.to_string(),
            });
        }
        dir = d.parent();
    }
    Err(anyhow!(
        "no package.json found walking up from imported package stylesheet {}",
        source_canonical.display()
    ))
}

/// Split a decoded reference at the first `?` or `#`; the suffix is preserved
/// verbatim for the emission wave's rewriting.
fn split_query_fragment(decoded: &str) -> (&str, &str) {
    match decoded.find(['?', '#']) {
        Some(i) => decoded.split_at(i),
        None => (decoded, ""),
    }
}

/// The floor error (locked template): fires for every attributed
/// package-relative reference until package asset emission lands.
fn floor_error(pkg: &PackageOrigin, raw_reference: &str) -> anyhow::Error {
    anyhow!(
        "error: unsupported `url()` in an imported package stylesheet\n\
         \x20 package:    {name}@{version}\n\
         \x20 stylesheet: {source}\n\
         \x20 reference:  url({raw_reference})\n\
         \x20 This stylesheet was inlined by `@import`, so its relative `url()` resolves\n\
         \x20 against the built CSS location and would 404 at runtime. zfb does not yet\n\
         \x20 emit package stylesheet assets.\n\
         \x20 workaround: vendor the file into `public/` and reference it by absolute URL\n\
         \x20             (see concepts/static-assets)",
        name = pkg.name,
        version = pkg.version,
        source = pkg.source.display(),
    )
}

/// Emission-shape error (locked template) for a target that is missing, not
/// a regular file, or escapes the package root.
fn cannot_emit_error(pkg: &PackageOrigin, raw_reference: &str, reason: &str) -> anyhow::Error {
    anyhow!(
        "error: cannot emit `url()` asset from an imported package stylesheet\n\
         \x20 package:    {name}@{version}\n\
         \x20 stylesheet: {source}\n\
         \x20 reference:  url({raw_reference})\n\
         \x20 reason:     {reason}",
        name = pkg.name,
        version = pkg.version,
        source = pkg.source.display(),
    )
}

/// The attribution-anomaly error (locked template): a relative `url()` that
/// cannot be attributed — zero-mapping line, position before the line's first
/// segment, a segment with no source, or no sourcemap at all.
fn attribution_anomaly_error(css: &str, value_span: &Range<usize>, line: u32) -> anyhow::Error {
    anyhow!(
        "error: could not attribute a relative `url()` in the compiled stylesheet\n\
         \x20 reference:  url({raw_reference})\n\
         \x20 at:         compiled CSS line {display_line}\n\
         \x20 This is unexpected with zfb's pipeline (unminified Tailwind output).\n\
         \x20 Please report it: https://github.com/Takazudo/zudo-front-builder/issues",
        raw_reference = &css[value_span.clone()],
        display_line = line + 1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sourcemap::SourceMapBuilder;
    use std::fs;

    /// Build a sourcemap mapping `(dst_line, dst_col) -> source` segments.
    fn map_of(segments: &[(u32, u32, &str)]) -> SourceMap {
        let mut builder = SourceMapBuilder::new(None);
        for &(dst_line, dst_col, source) in segments {
            builder.add(dst_line, dst_col, 0, 0, Some(source), None, false);
        }
        builder.into_sourcemap()
    }

    /// A fixture package under `root/node_modules/` with a stylesheet and an
    /// asset file. Returns the stylesheet path.
    fn write_package(root: &Path, name: &str, version: &str, assets: &[(&str, &str)]) -> PathBuf {
        let pkg_root = root.join("node_modules").join(name);
        fs::create_dir_all(&pkg_root).unwrap();
        fs::write(
            pkg_root.join("package.json"),
            format!(r#"{{"name":"{name}","version":"{version}"}}"#),
        )
        .unwrap();
        for (rel, bytes) in assets {
            let p = pkg_root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, bytes).unwrap();
        }
        let stylesheet = pkg_root.join("index.css");
        fs::write(&stylesheet, "/* fixture package css */\n").unwrap();
        stylesheet
    }

    fn attribute(css: &str, map: &SourceMap, base: &Path) -> Vec<AttributedUrl> {
        attribute_relative_urls(css, Some(map), base).expect("attribution should succeed")
    }

    fn floor_err(css: &str, map: &SourceMap, base: &Path) -> String {
        let attributed = attribute(css, map, base);
        format!(
            "{}",
            enforce_package_url_floor(css, &attributed).expect_err("floor must fire")
        )
    }

    // ---- the floor fires (THE acceptance test of this wave) --------------

    #[test]
    fn package_relative_url_with_existing_asset_fails_with_the_locked_floor_error() {
        // The reported repro shape: the referenced .woff2 EXISTS — an
        // unresolvable-only policy would exit 0 here, which is exactly the
        // silent failure the floor eliminates.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(
            root,
            "@demo/fonts",
            "1.2.3",
            &[("files/a.woff2", "woff2-bytes")],
        );
        let css = "@font-face{src:url(./files/a.woff2) format('woff2')}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);

        let msg = floor_err(css, &map, root);
        assert!(
            msg.contains("unsupported `url()` in an imported package stylesheet"),
            "floor header missing:\n{msg}"
        );
        assert!(
            msg.contains("package:    @demo/fonts@1.2.3"),
            "package identity missing:\n{msg}"
        );
        let canonical = fs::canonicalize(&stylesheet).unwrap();
        assert!(
            msg.contains(&format!("stylesheet: {}", canonical.display())),
            "canonical stylesheet path missing:\n{msg}"
        );
        assert!(
            msg.contains("reference:  url(./files/a.woff2)"),
            "raw reference missing:\n{msg}"
        );
        assert!(
            msg.contains("vendor the file into `public/`"),
            "workaround missing:\n{msg}"
        );
    }

    #[test]
    fn quoted_package_reference_reports_the_raw_quoted_span() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet =
            write_package(root, "plain-pkg", "0.1.0", &[("img/logo.png", "png-bytes")]);
        let css = ".a{background:url(\"./img/logo.png\")}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);

        let msg = floor_err(css, &map, root);
        assert!(
            msg.contains("reference:  url(\"./img/logo.png\")"),
            "raw quoted reference missing:\n{msg}"
        );
    }

    #[test]
    fn query_and_fragment_suffix_still_resolves_the_path_part_and_fires_the_floor() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "@demo/fonts", "1.2.3", &[("files/a.woff2", "bytes")]);
        let css = "@font-face{src:url(./files/a.woff2?v=1#iefix)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);

        let msg = floor_err(css, &map, root);
        // The path part resolved (file exists, contained) — so this is the
        // floor error, not a file-not-found — and the reference echoes the
        // full raw value including the suffix.
        assert!(
            msg.contains("unsupported `url()` in an imported package stylesheet"),
            "expected the floor error:\n{msg}"
        );
        assert!(
            msg.contains("reference:  url(./files/a.woff2?v=1#iefix)"),
            "full raw reference (with suffix) missing:\n{msg}"
        );
    }

    // ---- authored passthrough (must NOT over-fire) -----------------------

    #[test]
    fn authored_relative_url_passes_through_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let authored = root.join("styles/global.css");
        fs::create_dir_all(authored.parent().unwrap()).unwrap();
        fs::write(&authored, ".a{background:url(./hero.png)}\n").unwrap();

        let css = ".a{background:url(./hero.png)}\n";
        let map = map_of(&[(0, 0, authored.to_str().unwrap())]);
        let attributed = attribute(css, &map, root);
        assert_eq!(attributed.len(), 1);
        assert!(
            matches!(attributed[0].origin, UrlOrigin::Authored { .. }),
            "project CSS must classify as authored"
        );
        enforce_package_url_floor(css, &attributed).expect("authored CSS never errors");
    }

    #[test]
    fn workspace_linked_package_canonicalizing_outside_node_modules_is_authored() {
        // pnpm-workspace shape: node_modules/@local/ui is a symlink to a
        // sibling packages/ui directory. The canonical source path has no
        // node_modules component → authored → passthrough (locked corollary).
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let real_pkg = root.join("packages/ui");
        fs::create_dir_all(&real_pkg).unwrap();
        fs::write(
            real_pkg.join("package.json"),
            r#"{"name":"@local/ui","version":"0.0.1"}"#,
        )
        .unwrap();
        fs::write(real_pkg.join("index.css"), ".u{background:url(./x.png)}\n").unwrap();
        let link_dir = root.join("node_modules/@local");
        fs::create_dir_all(&link_dir).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_pkg, link_dir.join("ui")).unwrap();
        #[cfg(not(unix))]
        return;

        let css = ".u{background:url(./x.png)}\n";
        let linked_sheet = root.join("node_modules/@local/ui/index.css");
        let map = map_of(&[(0, 0, linked_sheet.to_str().unwrap())]);
        let attributed = attribute(css, &map, root);
        assert!(
            matches!(attributed[0].origin, UrlOrigin::Authored { .. }),
            "workspace-linked package must classify as authored, got {:?}",
            attributed[0].origin
        );
        enforce_package_url_floor(css, &attributed).expect("workspace package never errors");
    }

    #[test]
    fn untouched_forms_need_no_map_and_never_error() {
        // data:, absolute /, protocol-relative //, any scheme, fragment-only,
        // query-only, empty — never attributed, so a build whose authored CSS
        // uses only these succeeds even with NO sourcemap at all.
        let css = ".a{a:url(data:image/png;base64,AAAA);b:url(/img/x.png);\
                   c:url(//host/x.png);d:url(https://e.com/x.png);e:url(#frag);\
                   f:url(?v=1);g:url()}\n";
        let attributed =
            attribute_relative_urls(css, None, Path::new("/nonexistent")).expect("no error");
        assert!(attributed.is_empty(), "nothing should be attributed");
        enforce_package_url_floor(css, &attributed).expect("nothing to enforce");
    }

    #[test]
    fn tailwind_internal_bunfs_sources_classify_as_authored() {
        // Tailwind's own bundled internals map to /$bunfs/... paths that do
        // not exist on disk: canonicalize falls back to the lexical path,
        // which has no node_modules component → authored → passthrough.
        let css = ".t{background:url(./from-internals.png)}\n";
        let map = map_of(&[(0, 0, "/$bunfs/root/preflight-9vzsy0yp.css")]);
        let attributed = attribute(css, &map, Path::new("/nonexistent"));
        assert!(matches!(attributed[0].origin, UrlOrigin::Authored { .. }));
        enforce_package_url_floor(css, &attributed).expect("internals never error");
    }

    // ---- fail-closed anomalies -------------------------------------------

    #[test]
    fn relative_url_with_no_sourcemap_fails_closed_with_the_anomaly_error() {
        let css = ".a{background:url(./x.png)}\n";
        let err = attribute_relative_urls(css, None, Path::new("/nonexistent"))
            .expect_err("must fail closed");
        let msg = format!("{err}");
        assert!(
            msg.contains("could not attribute a relative `url()`"),
            "anomaly header missing:\n{msg}"
        );
        assert!(msg.contains("reference:  url(./x.png)"), "{msg}");
        assert!(msg.contains("at:         compiled CSS line 1"), "{msg}");
        assert!(
            msg.contains("Please report it: https://github.com/Takazudo/zudo-front-builder/issues"),
            "{msg}"
        );
    }

    #[test]
    fn relative_url_on_an_unmapped_line_fails_closed() {
        // Line 0 is mapped; the url() sits on line 2, which has no segments.
        // The greatest-lower-bound lookup lands on line 0 → rejected.
        let css = ".mapped{color:red}\n.filler{color:blue}\n.a{background:url(./x.png)}\n";
        let map = map_of(&[(0, 0, "/src/authored.css")]);
        let err = attribute_relative_urls(css, Some(&map), Path::new("/"))
            .expect_err("zero-mapping line must fail closed");
        let msg = format!("{err}");
        assert!(msg.contains("could not attribute"), "{msg}");
        assert!(msg.contains("compiled CSS line 3"), "{msg}");
    }

    #[test]
    fn position_before_the_first_segment_on_a_multi_source_line_fails_closed() {
        // The url() at column ~14 sits BEFORE the line's first segment at
        // column 40 — the ambiguous shape locked to fail closed. Guard the
        // geometry so a css edit can't silently invalidate the case.
        let css = ".a{background:url(./x.png)}                 .b{color:red}\n";
        let url_col = css.find("./x.png").unwrap() as u32;
        assert!(url_col < 40, "url must sit before the first segment");
        let map = map_of(&[(0, 40, "/src/authored.css")]);
        let err = attribute_relative_urls(css, Some(&map), Path::new("/"))
            .expect_err("pre-segment position must fail closed");
        assert!(format!("{err}").contains("could not attribute"));
    }

    // ---- the four adversarial cases (#2312 Q4, deterministic here) -------

    #[test]
    fn adversarial_a_authored_and_package_with_identical_url_text_attribute_distinctly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "@demo/fonts", "1.2.3", &[("files/a.woff2", "bytes")]);
        let authored = root.join("styles/global.css");
        fs::create_dir_all(authored.parent().unwrap()).unwrap();
        fs::write(&authored, "/* authored */\n").unwrap();

        // Identical url() text on two generated lines, one from each origin.
        let css = "@font-face{src:url(./files/a.woff2)}\n.mine{background:url(./files/a.woff2)}\n";
        let map = map_of(&[
            (0, 0, stylesheet.to_str().unwrap()),
            (1, 0, authored.to_str().unwrap()),
        ]);
        let attributed = attribute(css, &map, root);
        assert_eq!(attributed.len(), 2);
        assert!(
            matches!(&attributed[0].origin, UrlOrigin::Package(p) if p.name == "@demo/fonts"),
            "line 1 must attribute to the package, got {:?}",
            attributed[0].origin
        );
        assert!(
            matches!(attributed[1].origin, UrlOrigin::Authored { .. }),
            "line 2 must attribute to the authored file, got {:?}",
            attributed[1].origin
        );
        // And the floor fires on the package one (position-keyed, not
        // text-keyed — identical text in authored CSS does not suppress it).
        let msg = format!(
            "{}",
            enforce_package_url_floor(css, &attributed).expect_err("floor must fire")
        );
        assert!(msg.contains("@demo/fonts@1.2.3"), "{msg}");
    }

    #[test]
    fn adversarial_b_two_packages_with_identical_url_text_attribute_to_their_own_package() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sheet_one = write_package(root, "pkg-one", "1.0.0", &[("files/shared.png", "one")]);
        let sheet_two = write_package(root, "pkg-two", "2.0.0", &[("files/shared.png", "two")]);

        let css =
            ".a{background:url(./files/shared.png)}\n.b{background:url(./files/shared.png)}\n";
        let map = map_of(&[
            (0, 0, sheet_one.to_str().unwrap()),
            (1, 0, sheet_two.to_str().unwrap()),
        ]);
        let attributed = attribute(css, &map, root);
        assert_eq!(attributed.len(), 2);
        let names: Vec<&str> = attributed
            .iter()
            .map(|a| match &a.origin {
                UrlOrigin::Package(p) => p.name.as_str(),
                other => panic!("expected package origin, got {other:?}"),
            })
            .collect();
        assert_eq!(
            names,
            vec!["pkg-one", "pkg-two"],
            "identical text must attribute by position to distinct packages"
        );
    }

    #[test]
    fn adversarial_c_missing_asset_at_the_attributed_origin_errors_loudly_never_matches_a_decoy() {
        // The full case (c) — "the true origin's FILE vanished while a decoy
        // stylesheet contains the same url() text" — is unreachable by
        // construction before this code runs: Tailwind itself hard-fails the
        // whole build when an @import target is missing (#2312 Q4c), so no
        // compiled output exists to attribute. What IS reachable is the
        // boundary this test pins: attribution never text-searches, so when
        // the attributed origin's directory lacks the referenced ASSET, the
        // result is a loud file-not-found naming the true origin — never a
        // silent fallback to a decoy that happens to contain matching text.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // The attributed package does NOT ship the asset...
        let stylesheet = write_package(root, "@demo/fonts", "1.2.3", &[]);
        // ...while a decoy package DOES have a file at the same relative path.
        let _decoy = write_package(root, "decoy-pkg", "9.9.9", &[("files/a.woff2", "decoy")]);

        let css = "@font-face{src:url(./files/a.woff2)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);
        let msg = floor_err(css, &map, root);
        assert!(
            msg.contains("cannot emit `url()` asset"),
            "missing asset must be the emission-shape error:\n{msg}"
        );
        assert!(msg.contains("file not found at"), "{msg}");
        assert!(
            msg.contains("@demo/fonts@1.2.3"),
            "error must name the TRUE attributed origin:\n{msg}"
        );
        assert!(
            !msg.contains("decoy-pkg"),
            "a decoy with matching text must never be consulted:\n{msg}"
        );
    }

    #[test]
    fn adversarial_d_literal_duplicates_attribute_as_distinct_occurrences_no_dedup() {
        // Pins the no-dedup assumption: zfb never passes -m/--optimize, so
        // Lightning CSS preserves literal duplicate declarations, and this
        // module must report one attribution PER occurrence (position-keyed).
        // If minification is ever added, re-verify #2312 Q4d.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "dup-pkg", "1.0.0", &[("files/x.png", "bytes")]);

        let css = ".a{background:url(./files/x.png)}\n.b{background:url(./files/x.png)}\n";
        let map = map_of(&[
            (0, 0, stylesheet.to_str().unwrap()),
            (1, 0, stylesheet.to_str().unwrap()),
        ]);
        let attributed = attribute(css, &map, root);
        assert_eq!(attributed.len(), 2, "one attribution per occurrence");
        assert_ne!(
            attributed[0].occurrence.value_span, attributed[1].occurrence.value_span,
            "distinct positions, not a deduplicated entry"
        );
        assert_eq!(attributed[0].line, 0);
        assert_eq!(attributed[1].line, 1);
    }

    // ---- containment (locked decision e) ---------------------------------

    #[test]
    fn parent_traversal_escaping_the_package_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "escape-pkg", "1.0.0", &[]);
        // A real file OUTSIDE the package the reference climbs to.
        fs::write(root.join("node_modules/outside.txt"), "outside").unwrap();

        let css = ".a{background:url(../outside.txt)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);
        let msg = floor_err(css, &map, root);
        assert!(
            msg.contains("resolves outside the package directory"),
            "containment violation must be named:\n{msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escaping_the_package_root_fails_containment_after_canonicalization() {
        // The reference stays lexically INSIDE the package (./assets/f.bin),
        // but that path is a symlink to a file outside the package root —
        // canonicalization resolves the symlink first, so containment fails.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "symlink-pkg", "1.0.0", &[]);
        let outside = root.join("secret.bin");
        fs::write(&outside, "secret").unwrap();
        let pkg_root = root.join("node_modules/symlink-pkg");
        fs::create_dir_all(pkg_root.join("assets")).unwrap();
        std::os::unix::fs::symlink(&outside, pkg_root.join("assets/f.bin")).unwrap();

        let css = ".a{background:url(./assets/f.bin)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);
        let msg = floor_err(css, &map, root);
        assert!(
            msg.contains("resolves outside the package directory"),
            "symlink escape must fail containment:\n{msg}"
        );
    }

    #[test]
    fn directory_target_is_rejected_as_not_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "dir-pkg", "1.0.0", &[("files/sub/keep", "x")]);

        let css = ".a{background:url(./files/sub)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);
        let msg = floor_err(css, &map, root);
        assert!(msg.contains("not a regular file"), "{msg}");
    }

    // ---- package identity errors -----------------------------------------

    #[test]
    fn nameless_package_json_is_a_hard_error_naming_the_source_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let pkg_root = root.join("node_modules/anon");
        fs::create_dir_all(&pkg_root).unwrap();
        fs::write(pkg_root.join("package.json"), r#"{"version":"1.0.0"}"#).unwrap();
        let stylesheet = pkg_root.join("index.css");
        fs::write(&stylesheet, "x\n").unwrap();

        let css = ".a{background:url(./x.png)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);
        let err =
            attribute_relative_urls(css, Some(&map), root).expect_err("nameless must hard-error");
        let msg = format!("{err}");
        assert!(msg.contains("has no name"), "{msg}");
        assert!(
            msg.contains("index.css"),
            "must name the source path:\n{msg}"
        );
    }

    #[test]
    fn missing_package_json_is_a_hard_error_naming_the_source_path() {
        // A node_modules-shaped source with NO package.json anywhere up the
        // tree. tempdirs live under paths with no package.json ancestors in
        // practice; if an ancestor manifest ever exists the walk would find
        // it — acceptable for this fixture.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let orphan_dir = root.join("node_modules/orphan");
        fs::create_dir_all(&orphan_dir).unwrap();
        let stylesheet = orphan_dir.join("index.css");
        fs::write(&stylesheet, "x\n").unwrap();

        let css = ".a{background:url(./x.png)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);
        let result = attribute_relative_urls(css, Some(&map), root);
        if let Err(err) = result {
            let msg = format!("{err}");
            assert!(msg.contains("no package.json found"), "{msg}");
            assert!(msg.contains("index.css"), "{msg}");
        } else {
            // An ancestor package.json outside the tempdir got picked up —
            // environment-dependent, but the walk itself behaved as specified.
        }
    }

    // ---- sourcemap comment strip + parse ---------------------------------

    #[test]
    fn split_strips_only_a_trailing_sourcemap_comment() {
        let css = ".a{color:red}\n";
        let raw = format!("{css}/*# sourceMappingURL=data:application/json;base64,e30= */\n");
        let (stripped, url) = split_sourcemap_comment(&raw);
        assert_eq!(stripped, css, "stripped CSS must be byte-identical");
        assert_eq!(url, Some("data:application/json;base64,e30="));

        // No comment → untouched.
        let (untouched, none) = split_sourcemap_comment(css);
        assert_eq!(untouched, css);
        assert_eq!(none, None);

        // A lookalike in the MIDDLE (content after the comment) is content,
        // not the compiler's trailing annotation.
        let tricky = ".a{content:'x'}\n/*# sourceMappingURL=data:application/json;base64,e30= */\n.b{color:blue}\n";
        let (kept, no_url) = split_sourcemap_comment(tricky);
        assert_eq!(kept, tricky);
        assert_eq!(no_url, None);
    }

    #[test]
    fn end_to_end_inline_map_round_trip_authored_ok_and_package_floors() {
        // Full engine-entry path: compose raw output with a real encoded
        // inline sourcemap, run attribute_and_enforce_package_url_floor.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "@demo/fonts", "1.2.3", &[("files/a.woff2", "bytes")]);
        let authored = root.join("styles/global.css");
        fs::create_dir_all(authored.parent().unwrap()).unwrap();
        fs::write(&authored, "/* authored */\n").unwrap();

        let encode = |map: &SourceMap| {
            let mut buf = Vec::new();
            map.to_writer(&mut buf).unwrap();
            base64::engine::general_purpose::STANDARD.encode(&buf)
        };

        // Authored-only: returns the stripped CSS unchanged.
        let css = ".mine{background:url(./hero.png)}\n";
        fs::write(root.join("styles/hero.png"), "png").unwrap();
        let map = map_of(&[(0, 0, authored.to_str().unwrap())]);
        let raw = format!(
            "{css}/*# sourceMappingURL=data:application/json;base64,{} */\n",
            encode(&map)
        );
        let out = attribute_and_enforce_package_url_floor(&raw, root).expect("authored passes");
        assert_eq!(out, css, "returned CSS must be the exact stripped bytes");

        // Package-attributed: the floor fires through the same entry point.
        let css = "@font-face{src:url(./files/a.woff2)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);
        let raw = format!(
            "{css}/*# sourceMappingURL=data:application/json;base64,{} */\n",
            encode(&map)
        );
        let err = attribute_and_enforce_package_url_floor(&raw, root).expect_err("floor fires");
        assert!(format!("{err}").contains("@demo/fonts@1.2.3"));
    }

    #[test]
    fn absolute_only_css_with_no_map_passes_through_the_entry_point() {
        // The acceptance criterion "a project with only absolute-URL authored
        // CSS builds unchanged" — even when no sourcemap comment is present.
        let css = ".a{background:url(/img/x.png)}.b{background:url(data:,x)}\n";
        let out = attribute_and_enforce_package_url_floor(css, Path::new("/nonexistent"))
            .expect("absolute-only CSS never needs a map");
        assert_eq!(out, css);
    }

    // ---- position mechanics ----------------------------------------------

    #[test]
    fn generated_position_counts_utf16_columns() {
        // '日' and '本' are 1 UTF-16 code unit each (3 UTF-8 bytes); '𝒳'
        // (U+1D4B3) is 2 UTF-16 code units (4 UTF-8 bytes).
        let css = "/*日本𝒳*/.a{background:url(./x.png)}\n";
        let offset = css.find("./x.png").unwrap();
        let (line, col) = generated_position(css, offset);
        assert_eq!(line, 0);
        let expected: usize = css[..offset].chars().map(char::len_utf16).sum();
        assert_eq!(col as usize, expected);
        assert!(
            (col as usize) < offset,
            "UTF-16 columns must be shorter than byte offsets here"
        );
    }

    #[test]
    fn multiline_position_is_line_relative() {
        let css = ".a{color:red}\n.b{color:blue}\n.c{background:url(./x.png)}\n";
        let offset = css.find("./x.png").unwrap();
        let (line, col) = generated_position(css, offset);
        assert_eq!(line, 2);
        let line_start = css.rfind(".c{").unwrap();
        assert_eq!(col as usize, offset - line_start);
    }

    #[test]
    fn scope_filter_matches_the_locked_untouched_list() {
        for untouched in [
            "",
            "#frag",
            "?v=1",
            "/abs/x.png",
            "//host/x.png",
            "data:image/png;base64,AAAA",
            "https://example.com/x.png",
            "blob:abc",
        ] {
            assert!(
                !is_relative_reference(untouched),
                "{untouched:?} must be untouched"
            );
        }
        for relative in ["./x.png", "../x.png", "files/x.woff2", "x.png"] {
            assert!(
                is_relative_reference(relative),
                "{relative:?} must be in scope"
            );
        }
    }
}
