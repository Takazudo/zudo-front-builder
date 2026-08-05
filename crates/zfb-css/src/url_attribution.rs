//! Sourcemap-driven attribution of relative `url()` references in the
//! compiled Tailwind output, plus emission of the referenced assets as CSS
//! companions and the hard-error floor for the cases that still cannot be
//! emitted (issue #2315 attribution + #2316 emission, epic #2311).
//!
//! ## The problem this closes
//!
//! Tailwind inlines `@import`ed package stylesheets (e.g.
//! `@fontsource-variable/noto-sans/index.css`) into the compiled output
//! byte-for-byte, so a relative `url(./files/x.woff2)` inside one now resolves
//! against the built CSS location and 404s at runtime — while the build exits
//! 0. This module makes that state impossible: every relative `url()` in the
//! compiled output is attributed to its origin stylesheet via the compiler's
//! own sourcemap. A reference attributed to a `node_modules` stylesheet whose
//! target resolves to a real, contained, regular file is **emitted as a CSS
//! companion** and the reference is rewritten to point at it (decision c/f,
//! #2313) — the earlier "attributed but unsupported" floor (#2315) is
//! replaced for this handled case. A reference whose target is missing, not a
//! regular file, unreadable, or escapes the package directory **still fails
//! the build loudly** with the permanent emission-error template.
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
//!
//! ## Emission (decision c) and URL rewriting form (decision f)
//!
//! For every package-attributed reference whose target resolves to a
//! contained regular file: the asset's bytes are hashed upstream
//! (`sha256_8`, the existing [`crate::pipeline::hash_8`] /
//! `zfb_islands::hash_8` convention) into a flat companion filename
//! `{stem}-{hash8}.{ext}` (`stem` is the asset's own basename, sanitized to
//! `[A-Za-z0-9._-]` — never the decoded reference path, so identical bytes
//! always hash to the same companion regardless of how many different
//! relative paths reach them). The `url()` value span is spliced to
//! `./{filename}` plus the original `?`/`#` suffix verbatim, preserving the
//! original quoting style. Companions land beside the hashed CSS entry, so
//! `./` resolves correctly under any base/CDN URL prefix.
//!
//! Dedup is keyed on the asset's **canonical path**: the same file
//! referenced N times (even across different packages, if byte-identical)
//! produces exactly one companion; two different files that happen to share
//! a basename hash to different filenames and never collide. A companion
//! filename colliding with different bytes is a hard error (practically
//! unreachable in sha256-8 space).
//!
//! **Atomicity:** every referenced asset's bytes are read, and every
//! companion filename resolved, before any `url()` is spliced — the first
//! unresolvable reference fails the whole call, so [`attribute_and_emit_package_urls`]
//! never returns a partially-rewritten stylesheet.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Result};
use base64::Engine as _;
use sha2::{Digest, Sha256};
use sourcemap::SourceMap;

use crate::url_scanner::{scan_css_urls, CssUrlOccurrence, UrlQuote};

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

/// One companion asset emitted for a resolved package-attributed `url()`
/// reference (decision c, #2313). `filename` is the flat, sanitized
/// `{stem}-{hash8}.{ext}` basename the CSS was rewritten to reference —
/// safe to hand straight to [`crate::emitter::CssEmitterOutput::companions`]
/// and, downstream, `zfb_build::pipeline::prod::CompanionFile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageUrlAsset {
    /// Flat basename, e.g. `a-1a2b3c4d.woff2`. Never contains a path
    /// separator or `..`.
    pub filename: String,
    /// The asset's raw bytes, read once and reused for every reference
    /// that dedups to this same companion.
    pub bytes: Vec<u8>,
}

/// Engine entry point: strip the trailing `sourceMappingURL` comment from the
/// raw Tailwind output, attribute every relative `url()` through the parsed
/// sourcemap, then emit a companion asset for every package-attributed
/// reference that resolves cleanly and rewrite its `url()` value to point at
/// the companion. A reference that cannot be resolved (missing, not a
/// regular file, unreadable, or outside the package directory) still fails
/// the build with the permanent emission-error template — the only change
/// from the prior wave's floor is that a *resolvable* reference is now
/// handled instead of unconditionally erroring.
///
/// Returns the rewritten CSS (byte-identical to the pre-`--map` output
/// outside of package `url()` value spans — the comment is trailing, so
/// stripping shifts no earlier offsets, and only package-attributed value
/// spans are ever spliced) plus the companions to ship beside it.
///
/// `base_dir` anchors relative sourcemap `sources` entries (the engine passes
/// its subprocess working directory; Tailwind emits absolute paths in
/// practice).
pub fn attribute_and_emit_package_urls(
    raw_css: &str,
    base_dir: &Path,
) -> Result<(String, Vec<PackageUrlAsset>)> {
    let (css, map_url) = split_sourcemap_comment(raw_css);
    let map = match map_url.and_then(inline_data_url_payload) {
        Some(payload) => Some(parse_inline_sourcemap(payload)?),
        None => None,
    };
    let attributed = attribute_relative_urls(css, map.as_ref(), base_dir)?;
    rewrite_and_emit_package_urls(css, &attributed)
}

/// The comment Tailwind's `--map` appends to the output file. Everything
/// before it (minus the CLI's one separator newline — see
/// [`split_sourcemap_comment`]) is the exact CSS text a `--map`-less
/// invocation would produce.
const SOURCEMAP_COMMENT_PREFIX: &str = "/*# sourceMappingURL=";

/// Split a trailing `/*# sourceMappingURL=... */` comment off `raw`, returning
/// the stripped CSS slice and the comment's URL when present. The comment is
/// only recognised in trailing position (nothing but whitespace after its
/// `*/`) — a lookalike in the middle of the text is someone's content, not
/// the compiler's annotation, and is left alone.
///
/// External-tool contract (pinned Tailwind v4 CLI, verified empirically):
/// `--map` writes `{css}\n{comment}` — one separator newline between the
/// `--map`-less output and the comment. That separator is the comment's, not
/// the stylesheet's, so it is stripped with it; keeping it would change the
/// shipped bytes (and every downstream content hash) on every real build.
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
    let css = &raw[..start];
    let css = css.strip_suffix('\n').unwrap_or(css);
    (css, Some(after[..end].trim()))
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

/// Resolve, read, and hash every package-attributed reference in
/// `attributed`, then splice each into `css` as a rewritten companion
/// reference (decision c/f, #2313). A reference that is missing, not a
/// regular file, unreadable, or escapes the package root fails the whole
/// call with the permanent emission-error template naming the reason —
/// nothing is spliced and no companion list is returned, so a caller can
/// never publish a partially-rewritten stylesheet (the atomicity
/// requirement, #2316).
///
/// Two passes: first resolve every occurrence (fail fast on the first
/// unresolvable one), then splice back-to-front so earlier byte spans stay
/// valid. Authored-origin occurrences are never touched.
fn rewrite_and_emit_package_urls(
    css: &str,
    attributed: &[AttributedUrl],
) -> Result<(String, Vec<PackageUrlAsset>)> {
    /// One resolved package reference, ready to splice. `filename` and
    /// `suffix` are kept apart (rather than pre-joined) so the splice step
    /// can escape `suffix` for the occurrence's own quote style — the
    /// filename never needs escaping (it is built from `[A-Za-z0-9._-]`
    /// only), but `suffix` came from a CSS-escape-decoded query/fragment
    /// and may itself contain a quote or backslash character that would
    /// otherwise break out of a requoted replacement (see
    /// [`escape_suffix_for_quote`]).
    struct Resolved<'a> {
        entry: &'a AttributedUrl,
        filename: String,
        suffix: &'a str,
    }

    let mut companions: Vec<PackageUrlAsset> = Vec::new();
    // Canonical asset path -> already-resolved companion filename. Dedup
    // rule (decision c): the same file referenced N times emits once.
    let mut filename_by_canonical: HashMap<PathBuf, String> = HashMap::new();
    let mut resolved: Vec<Resolved> = Vec::new();

    for entry in attributed {
        let UrlOrigin::Package(pkg) = &entry.origin else {
            continue;
        };
        let raw_reference = &css[entry.occurrence.value_span.clone()];
        let (path_part, suffix) = split_query_fragment(&entry.occurrence.decoded);
        let source_dir = pkg.source.parent().unwrap_or(Path::new("."));
        let target = source_dir.join(path_part);

        // Containment: canonicalize FIRST (resolving symlinks), so a symlink
        // escaping the package resolves outside the root and is rejected.
        let asset_canonical = std::fs::canonicalize(&target).map_err(|_| {
            cannot_emit_error(
                pkg,
                raw_reference,
                &format!("file not found at {}", target.display()),
            )
        })?;
        let package_root =
            std::fs::canonicalize(&pkg.package_root).unwrap_or_else(|_| pkg.package_root.clone());
        if !asset_canonical.starts_with(&package_root) {
            return Err(cannot_emit_error(
                pkg,
                raw_reference,
                &format!(
                    "resolves outside the package directory: {}",
                    asset_canonical.display()
                ),
            ));
        }
        if !asset_canonical.is_file() {
            return Err(cannot_emit_error(
                pkg,
                raw_reference,
                &format!("not a regular file: {}", asset_canonical.display()),
            ));
        }

        let filename = match filename_by_canonical.get(&asset_canonical) {
            Some(existing) => existing.clone(),
            None => {
                let bytes = std::fs::read(&asset_canonical).map_err(|e| {
                    cannot_emit_error(pkg, raw_reference, &format!("unreadable: {e}"))
                })?;
                let filename = package_url_companion_filename(&asset_canonical, &bytes);
                match companions.iter().find(|c| c.filename == filename) {
                    // Byte-identical companion already registered (possibly
                    // from a different canonical path, e.g. two packages
                    // shipping the same font) — reuse it, no duplicate.
                    Some(existing) if existing.bytes == bytes => {}
                    // A companion filename collision with DIFFERENT bytes is
                    // practically unreachable in sha256-8 space, but must
                    // never silently overwrite (decision c collision rule).
                    Some(_) => {
                        return Err(anyhow!(
                            "error: companion filename collision with different bytes\n\
                             \x20 filename: {filename}\n\
                             \x20 package:    {name}@{version}\n\
                             \x20 stylesheet: {source}\n\
                             \x20 asset:      {asset}",
                            name = pkg.name,
                            version = pkg.version,
                            source = pkg.source.display(),
                            asset = asset_canonical.display(),
                        ));
                    }
                    None => companions.push(PackageUrlAsset {
                        filename: filename.clone(),
                        bytes,
                    }),
                }
                filename_by_canonical.insert(asset_canonical.clone(), filename.clone());
                filename
            }
        };

        resolved.push(Resolved {
            entry,
            filename,
            suffix,
        });
    }

    let mut spliced = css.to_string();
    for r in resolved.iter().rev() {
        let quote = r.entry.occurrence.quote;
        let escaped_suffix = escape_suffix_for_quote(r.suffix, quote);
        let replacement = format!("./{}{escaped_suffix}", r.filename);
        let value = match quote {
            UrlQuote::None => replacement,
            UrlQuote::Single => format!("'{replacement}'"),
            UrlQuote::Double => format!("\"{replacement}\""),
        };
        spliced.replace_range(r.entry.occurrence.value_span.clone(), &value);
    }

    Ok((spliced, companions))
}

/// CSS-escape any byte in `suffix` (the `?`/`#` tail split off the
/// CSS-escape-decoded `url()` value) that would be unsafe to splice
/// verbatim into `quote`'s replacement context.
///
/// `suffix` comes from [`split_query_fragment`] applied to
/// [`CssUrlOccurrence::decoded`] — CSS escapes (e.g. `\22` for `"`) have
/// already been resolved to their literal characters at that point. A
/// literal quote character matching the target quote style (or, for an
/// unquoted replacement, a literal quote/paren/backslash/whitespace
/// character) would otherwise terminate the spliced token early or turn a
/// well-formed url-token into a bad one. `\` + literal-char is a valid CSS
/// escape for any of these (CSS Syntax §4.3.7) — none of them is a hex
/// digit, so the escape can never be misread as the start of a hex escape.
fn escape_suffix_for_quote(suffix: &str, quote: UrlQuote) -> String {
    let mut out = String::with_capacity(suffix.len());
    for c in suffix.chars() {
        let needs_escape = match quote {
            UrlQuote::Double => matches!(c, '"' | '\\'),
            UrlQuote::Single => matches!(c, '\'' | '\\'),
            UrlQuote::None => matches!(c, '"' | '\'' | '(' | ')' | '\\') || c.is_whitespace(),
        };
        if needs_escape {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// SHA-256 of `bytes`, truncated to 8 lowercase hex characters. Mirrors the
/// `zfb_css::pipeline::hash_8` / `zfb_islands::hash_8` convention (decision
/// c, #2313: "the existing `hash_8` convention").
fn sha256_hash8(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    hex::encode(digest)[..8].to_string()
}

/// Sanitize one filename component (stem or extension) to the
/// flat-basename-safe alphabet `[A-Za-z0-9._-]`, replacing every other byte
/// with `_`. Applied to the asset's own basename, never to the decoded
/// reference path, so path separators and `..` in a crafted reference can
/// never reach a companion filename (containment already rejected them
/// earlier, but this keeps the filename builder independently safe).
fn sanitize_flat_basename_component(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Build the flat companion filename `{stem}-{hash8}.{ext}` for a resolved
/// package asset (decision c/f, #2313). `stem`/`ext` come from the asset's
/// own canonical basename — never the decoded `url()` reference — so
/// identical bytes reached via different relative paths always hash to the
/// same companion.
fn package_url_companion_filename(asset_canonical: &Path, bytes: &[u8]) -> String {
    let basename = asset_canonical
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "asset".to_string());
    let (stem, ext) = match basename.rfind('.') {
        Some(idx) if idx > 0 => (&basename[..idx], Some(&basename[idx + 1..])),
        _ => (basename.as_str(), None),
    };
    let stem = sanitize_flat_basename_component(stem);
    let hash8 = sha256_hash8(bytes);
    match ext {
        Some(ext) if !ext.is_empty() => {
            format!("{stem}-{hash8}.{}", sanitize_flat_basename_component(ext))
        }
        _ => format!("{stem}-{hash8}"),
    }
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
///
/// The walk never crosses a `node_modules` boundary: a stylesheet whose
/// package ships no `package.json` must hard-error, not walk up past
/// `node_modules` and adopt the application's own root manifest — that would
/// attribute the package to the app and widen the containment root to the
/// whole project.
fn package_identity(source_canonical: &Path) -> Result<PackageOrigin> {
    let mut dir = source_canonical.parent();
    while let Some(d) = dir {
        if d.file_name().is_some_and(|n| n == "node_modules") {
            break;
        }
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

/// Emission-shape error (locked template, decision d permanent form): fires
/// for a package-attributed reference that is missing, not a regular file,
/// unreadable, or escapes the package root. This is now the ONLY error a
/// resolvable-vs-unresolvable package reference can produce — a resolvable
/// target is emitted and rewritten instead of erroring (the prior wave's
/// "unsupported `url()`" floor is replaced for that case).
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

    /// Run the emission function end to end (attribute + rewrite/emit),
    /// expecting success.
    fn emit(css: &str, map: &SourceMap, base: &Path) -> (String, Vec<PackageUrlAsset>) {
        let attributed = attribute(css, map, base);
        rewrite_and_emit_package_urls(css, &attributed).expect("emission should succeed")
    }

    /// Same, expecting a hard error; returns its Display text.
    fn emit_err(css: &str, map: &SourceMap, base: &Path) -> String {
        let attributed = attribute(css, map, base);
        format!(
            "{}",
            rewrite_and_emit_package_urls(css, &attributed).expect_err("emission must fail")
        )
    }

    // ---- resolvable references are emitted and rewritten, not floored ----
    // (THE acceptance test of this wave: #2316 replaces the wave-4 floor for
    // handled cases.)

    #[test]
    fn package_relative_url_with_existing_asset_is_emitted_and_rewritten() {
        // The reported repro shape: the referenced .woff2 EXISTS. Wave 4
        // hard-errored here (the guaranteed floor); wave 5 now emits a
        // companion and rewrites the reference instead.
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

        let (rewritten, companions) = emit(css, &map, root);
        assert_eq!(companions.len(), 1, "exactly one companion: {companions:?}");
        let companion = &companions[0];
        assert_eq!(companion.bytes, b"woff2-bytes");
        assert!(
            companion.filename.starts_with("a-") && companion.filename.ends_with(".woff2"),
            "unexpected companion filename: {}",
            companion.filename
        );
        let expected = format!(
            "@font-face{{src:url(./{}) format('woff2')}}\n",
            companion.filename
        );
        assert_eq!(rewritten, expected, "url() must rewrite to the companion");
    }

    #[test]
    fn quoted_package_reference_preserves_its_quote_style_and_rewrites_inside_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet =
            write_package(root, "plain-pkg", "0.1.0", &[("img/logo.png", "png-bytes")]);
        let css = ".a{background:url(\"./img/logo.png\")}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);

        let (rewritten, companions) = emit(css, &map, root);
        assert_eq!(companions.len(), 1);
        let expected = format!(".a{{background:url(\"./{}\")}}\n", companions[0].filename);
        assert_eq!(rewritten, expected, "double quotes must be preserved");
    }

    #[test]
    fn single_quoted_package_reference_preserves_its_quote_style() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet =
            write_package(root, "plain-pkg", "0.1.0", &[("img/logo.png", "png-bytes")]);
        let css = ".a{background:url('./img/logo.png')}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);

        let (rewritten, companions) = emit(css, &map, root);
        assert_eq!(companions.len(), 1);
        let expected = format!(".a{{background:url('./{}')}}\n", companions[0].filename);
        assert_eq!(rewritten, expected, "single quotes must be preserved");
    }

    #[test]
    fn query_and_fragment_suffix_is_preserved_verbatim_after_the_rewritten_basename() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "@demo/fonts", "1.2.3", &[("files/a.woff2", "bytes")]);
        let css = "@font-face{src:url(./files/a.woff2?v=1#iefix)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);

        let (rewritten, companions) = emit(css, &map, root);
        assert_eq!(companions.len(), 1);
        let expected = format!(
            "@font-face{{src:url(./{}?v=1#iefix)}}\n",
            companions[0].filename
        );
        assert_eq!(
            rewritten, expected,
            "the ?v=1#iefix suffix must survive verbatim after the rewritten basename"
        );
    }

    #[test]
    fn a_css_escaped_quote_in_the_suffix_is_re_escaped_not_spliced_raw() {
        // codex review finding: `\22` decodes to a literal `"`. Splicing it
        // unescaped into a double-quoted replacement would terminate the
        // string early and produce invalid CSS.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "@demo/fonts", "1.2.3", &[("files/a.woff2", "bytes")]);
        let css = "@font-face{src:url(\"./files/a.woff2?x=\\22\")}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);

        let (rewritten, companions) = emit(css, &map, root);
        assert_eq!(companions.len(), 1);
        let expected = format!(
            "@font-face{{src:url(\"./{}?x=\\\"\")}}\n",
            companions[0].filename
        );
        assert_eq!(
            rewritten, expected,
            "the decoded quote must be re-escaped for the double-quoted context:\n{rewritten}"
        );
        // And the result must re-parse as exactly one url() whose decoded
        // suffix round-trips to the original literal quote character.
        let reparsed = scan_css_urls(&rewritten);
        assert_eq!(reparsed.len(), 1);
        assert!(reparsed[0].decoded.ends_with("?x=\""));
    }

    #[test]
    fn a_backslash_in_the_suffix_is_escaped_in_an_unquoted_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "@demo/fonts", "1.2.3", &[("files/a.woff2", "bytes")]);
        // `\5c` decodes to a literal backslash.
        let css = "@font-face{src:url(./files/a.woff2?x=\\5c)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);

        let (rewritten, companions) = emit(css, &map, root);
        assert_eq!(companions.len(), 1);
        let reparsed = scan_css_urls(&rewritten);
        assert_eq!(reparsed.len(), 1);
        assert!(
            reparsed[0].decoded.ends_with("?x=\\"),
            "decoded suffix must round-trip to a single literal backslash: {:?}",
            reparsed[0].decoded
        );
    }

    #[test]
    fn unicode_range_and_every_other_declaration_stay_byte_identical_only_url_value_changes() {
        // The reporter's stated acceptance bar (#2316): subsetting keeps
        // working because unicode-range is untouched, and nothing else in
        // the declaration list shifts.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "@demo/fonts", "1.2.3", &[("files/a.woff2", "bytes")]);
        let css = "@font-face{font-family:A;src:url(./files/a.woff2) format('woff2');\
                   unicode-range:U+0000-00FF,U+0131,U+0152-0153}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);

        let (rewritten, companions) = emit(css, &map, root);
        assert_eq!(companions.len(), 1);
        let expected = format!(
            "@font-face{{font-family:A;src:url(./{}) format('woff2');\
             unicode-range:U+0000-00FF,U+0131,U+0152-0153}}\n",
            companions[0].filename
        );
        assert_eq!(rewritten, expected);
        assert!(
            rewritten.contains("unicode-range:U+0000-00FF,U+0131,U+0152-0153"),
            "unicode-range must stay byte-identical:\n{rewritten}"
        );
    }

    #[test]
    fn non_font_asset_an_image_is_emitted_too_general_url_support_not_fonts_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(
            root,
            "@demo/icons",
            "2.0.0",
            &[("img/logo.png", "png-bytes")],
        );
        let css = ".a{background-image:url(./img/logo.png)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);

        let (rewritten, companions) = emit(css, &map, root);
        assert_eq!(companions.len(), 1);
        assert!(companions[0].filename.ends_with(".png"));
        assert_eq!(companions[0].bytes, b"png-bytes");
        assert!(rewritten.contains(&format!("url(./{})", companions[0].filename)));
    }

    #[test]
    fn duplicate_references_to_one_file_emit_a_single_companion() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(
            root,
            "@demo/fonts",
            "1.2.3",
            &[("files/a.woff2", "shared-bytes")],
        );
        let css = "@font-face{src:url(./files/a.woff2)}\n\
                   .also-uses-it{background:url(./files/a.woff2)}\n";
        let map = map_of(&[
            (0, 0, stylesheet.to_str().unwrap()),
            (1, 0, stylesheet.to_str().unwrap()),
        ]);

        let (rewritten, companions) = emit(css, &map, root);
        assert_eq!(
            companions.len(),
            1,
            "one file referenced twice must emit exactly one companion: {companions:?}"
        );
        let filename = &companions[0].filename;
        assert_eq!(
            rewritten.matches(&format!("url(./{filename})")).count(),
            2,
            "both occurrences must rewrite to the same companion:\n{rewritten}"
        );
    }

    #[test]
    fn two_files_sharing_a_basename_get_distinct_companions_no_collision() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let pkg_a = write_package(root, "pkg-a", "1.0.0", &[("assets/x.png", "AAAA")]);
        let pkg_b = write_package(root, "pkg-b", "2.0.0", &[("assets/x.png", "BBBB")]);
        let css = ".a{background:url(./assets/x.png)}\n.b{background:url(./assets/x.png)}\n";
        let map = map_of(&[
            (0, 0, pkg_a.to_str().unwrap()),
            (1, 0, pkg_b.to_str().unwrap()),
        ]);

        let (rewritten, companions) = emit(css, &map, root);
        assert_eq!(
            companions.len(),
            2,
            "distinct bytes must not collide: {companions:?}"
        );
        assert_ne!(companions[0].filename, companions[1].filename);
        assert!(rewritten.contains(&format!("url(./{})", companions[0].filename)));
        assert!(rewritten.contains(&format!("url(./{})", companions[1].filename)));
    }

    #[test]
    fn byte_identical_files_from_different_packages_dedup_to_one_companion() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let pkg_a = write_package(root, "pkg-a", "1.0.0", &[("assets/x.png", "SAME-BYTES")]);
        let pkg_b = write_package(root, "pkg-b", "2.0.0", &[("assets/x.png", "SAME-BYTES")]);
        let css = ".a{background:url(./assets/x.png)}\n.b{background:url(./assets/x.png)}\n";
        let map = map_of(&[
            (0, 0, pkg_a.to_str().unwrap()),
            (1, 0, pkg_b.to_str().unwrap()),
        ]);

        let (_, companions) = emit(css, &map, root);
        assert_eq!(
            companions.len(),
            1,
            "byte-identical files from different packages must dedup: {companions:?}"
        );
    }

    #[test]
    fn unresolvable_reference_still_hard_errors_and_no_companion_is_emitted() {
        // Failure atomicity: a resolvable reference alongside an
        // unresolvable one must not leave a half-emitted companion set —
        // the whole call fails.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stylesheet = write_package(root, "@demo/fonts", "1.2.3", &[("files/a.woff2", "bytes")]);
        let css = "@font-face{src:url(./files/a.woff2)}\n\
                   .missing{background:url(./files/does-not-exist.png)}\n";
        let map = map_of(&[
            (0, 0, stylesheet.to_str().unwrap()),
            (1, 0, stylesheet.to_str().unwrap()),
        ]);

        let msg = emit_err(css, &map, root);
        assert!(
            msg.contains("cannot emit `url()` asset"),
            "expected the emission-error shape:\n{msg}"
        );
        assert!(msg.contains("file not found at"), "{msg}");
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
        let (rewritten, companions) =
            rewrite_and_emit_package_urls(css, &attributed).expect("authored CSS never errors");
        assert_eq!(rewritten, css, "authored CSS stays byte-for-byte unchanged");
        assert!(companions.is_empty());
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
        let (rewritten, companions) = rewrite_and_emit_package_urls(css, &attributed)
            .expect("workspace package never errors");
        assert_eq!(rewritten, css);
        assert!(companions.is_empty());
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
        let (rewritten, companions) =
            rewrite_and_emit_package_urls(css, &attributed).expect("nothing to enforce");
        assert_eq!(rewritten, css);
        assert!(companions.is_empty());
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
        let (rewritten, companions) =
            rewrite_and_emit_package_urls(css, &attributed).expect("internals never error");
        assert_eq!(rewritten, css);
        assert!(companions.is_empty());
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
        // And the package one gets emitted+rewritten (position-keyed, not
        // text-keyed — identical text in authored CSS does not suppress it),
        // while the authored line stays untouched.
        let (rewritten, companions) =
            rewrite_and_emit_package_urls(css, &attributed).expect("package asset resolves");
        assert_eq!(companions.len(), 1);
        assert!(companions[0].filename.contains("a-"));
        assert!(
            rewritten.contains(".mine{background:url(./files/a.woff2)}"),
            "the authored occurrence must stay untouched:\n{rewritten}"
        );
        assert!(
            !rewritten.contains("@font-face{src:url(./files/a.woff2)}"),
            "the package occurrence must be rewritten:\n{rewritten}"
        );
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
        let msg = emit_err(css, &map, root);
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
        let msg = emit_err(css, &map, root);
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
        let msg = emit_err(css, &map, root);
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
        let msg = emit_err(css, &map, root);
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
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let orphan_dir = root.join("node_modules/orphan");
        fs::create_dir_all(&orphan_dir).unwrap();
        let stylesheet = orphan_dir.join("index.css");
        fs::write(&stylesheet, "x\n").unwrap();

        let css = ".a{background:url(./x.png)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);
        let err = attribute_relative_urls(css, Some(&map), root)
            .expect_err("manifest-less package must hard-error");
        let msg = format!("{err}");
        assert!(msg.contains("no package.json found"), "{msg}");
        assert!(msg.contains("index.css"), "{msg}");
    }

    #[test]
    fn manifest_walk_never_crosses_node_modules_to_adopt_the_app_manifest() {
        // A manifest-less package must NOT walk up past node_modules and
        // attribute itself to the application's root package.json — that
        // would misname the package AND widen the containment root to the
        // whole project.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("package.json"),
            r#"{"name":"my-app","version":"0.0.1"}"#,
        )
        .unwrap();
        let orphan_dir = root.join("node_modules/orphan");
        fs::create_dir_all(&orphan_dir).unwrap();
        let stylesheet = orphan_dir.join("index.css");
        fs::write(&stylesheet, "x\n").unwrap();

        let css = ".a{background:url(./x.png)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);
        let err = attribute_relative_urls(css, Some(&map), root)
            .expect_err("must hard-error, never adopt the app manifest");
        let msg = format!("{err}");
        assert!(msg.contains("no package.json found"), "{msg}");
        assert!(
            !msg.contains("my-app"),
            "the app's own manifest must never be adopted:\n{msg}"
        );
    }

    // ---- sourcemap comment strip + parse ---------------------------------

    #[test]
    fn split_strips_only_a_trailing_sourcemap_comment() {
        // The real CLI shape: `{css}\n{comment}` — the separator newline is
        // the comment's and must be stripped with it, so the result is
        // byte-identical to a `--map`-less run.
        let css = ".a{color:red}\n";
        let raw = format!("{css}\n/*# sourceMappingURL=data:application/json;base64,e30= */\n");
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
    fn end_to_end_inline_map_round_trip_authored_ok_and_package_emits() {
        // Full engine-entry path: compose raw output with a real encoded
        // inline sourcemap, run attribute_and_emit_package_urls.
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

        // Authored-only: returns the stripped CSS unchanged, no companions.
        let css = ".mine{background:url(./hero.png)}\n";
        fs::write(root.join("styles/hero.png"), "png").unwrap();
        let map = map_of(&[(0, 0, authored.to_str().unwrap())]);
        let raw = format!(
            "{css}\n/*# sourceMappingURL=data:application/json;base64,{} */\n",
            encode(&map)
        );
        let (out, companions) =
            attribute_and_emit_package_urls(&raw, root).expect("authored passes");
        assert_eq!(out, css, "returned CSS must be the exact stripped bytes");
        assert!(companions.is_empty());

        // Package-attributed: emitted + rewritten through the same entry point.
        let css = "@font-face{src:url(./files/a.woff2)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);
        let raw = format!(
            "{css}\n/*# sourceMappingURL=data:application/json;base64,{} */\n",
            encode(&map)
        );
        let (out, companions) =
            attribute_and_emit_package_urls(&raw, root).expect("package asset resolves");
        assert_eq!(companions.len(), 1);
        assert_eq!(companions[0].bytes, b"bytes");
        assert!(out.contains(&format!("url(./{})", companions[0].filename)));

        // And the unresolvable-target error still surfaces through the same
        // entry point (decision d, unchanged for unhandled cases).
        let css = "@font-face{src:url(./files/does-not-exist.woff2)}\n";
        let map = map_of(&[(0, 0, stylesheet.to_str().unwrap())]);
        let raw = format!(
            "{css}\n/*# sourceMappingURL=data:application/json;base64,{} */\n",
            encode(&map)
        );
        let err = attribute_and_emit_package_urls(&raw, root).expect_err("still hard-errors");
        assert!(format!("{err}").contains("@demo/fonts@1.2.3"));
    }

    #[test]
    fn absolute_only_css_with_no_map_passes_through_the_entry_point() {
        // The acceptance criterion "a project with only absolute-URL authored
        // CSS builds unchanged" — even when no sourcemap comment is present.
        let css = ".a{background:url(/img/x.png)}.b{background:url(data:,x)}\n";
        let (out, companions) = attribute_and_emit_package_urls(css, Path::new("/nonexistent"))
            .expect("absolute-only CSS never needs a map");
        assert_eq!(out, css);
        assert!(companions.is_empty());
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
