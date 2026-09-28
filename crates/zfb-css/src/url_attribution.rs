//! Resolve package stylesheet URLs against the declaring CSS file and emit companions.
//! Relative project URLs retain their authored identity.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};

use crate::url_scanner::{scan_css_urls, CssUrlOccurrence, UrlQuote};

/// The origin a relative `url()` reference was attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlOrigin {
    /// Authored/project CSS passes through unchanged.
    Authored {
        /// Canonical path of the declaring stylesheet.
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
    /// Declaring stylesheet of this reference.
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

/// Resolve package assets while bundling a declaring stylesheet.
#[derive(Default)]
pub(crate) struct PackageUrlResolver {
    pub(crate) companions: Vec<PackageUrlAsset>,
    pub(crate) asset_paths: BTreeSet<PathBuf>,
    filename_by_canonical: HashMap<PathBuf, String>,
}

/// Rewrite URLs before handing a declaring stylesheet to the bundler. The
/// input is already import-ordered, so scanner spans refer to these bytes.
pub(crate) fn rewrite_package_urls_for_source(
    css: &str,
    source_canonical: &Path,
    resolver: &mut PackageUrlResolver,
) -> Result<String> {
    if !has_node_modules_component(source_canonical) {
        return Ok(css.to_string());
    }
    let occurrences: Vec<_> = scan_css_urls(css)
        .into_iter()
        .filter(|occurrence| is_relative_reference(&occurrence.decoded))
        .collect();
    if occurrences.is_empty() {
        return Ok(css.to_string());
    }
    let origin = UrlOrigin::Package(package_identity(source_canonical)?);
    let attributed: Vec<_> = occurrences
        .into_iter()
        .map(|occurrence| AttributedUrl {
            occurrence,
            origin: origin.clone(),
        })
        .collect();
    rewrite_with_origins(css, &attributed, resolver)
}

fn rewrite_with_origins(
    css: &str,
    attributed: &[AttributedUrl],
    resolver: &mut PackageUrlResolver,
) -> Result<String> {
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

    let mut resolved: Vec<Resolved> = Vec::new();

    for entry in attributed {
        let UrlOrigin::Package(pkg) = &entry.origin else {
            continue;
        };
        let raw_reference = &css[entry.occurrence.value_span.clone()];
        let (_, suffix) = split_query_fragment(&entry.occurrence.decoded);
        let filename = resolver.resolve(pkg, raw_reference, &entry.occurrence.decoded)?;
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

    Ok(spliced)
}

impl PackageUrlResolver {
    fn resolve(
        &mut self,
        pkg: &PackageOrigin,
        raw_reference: &str,
        decoded: &str,
    ) -> Result<String> {
        let (path_part, _) = split_query_fragment(decoded);
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

        let filename = match self.filename_by_canonical.get(&asset_canonical) {
            Some(existing) => existing.clone(),
            None => {
                let bytes = std::fs::read(&asset_canonical).map_err(|e| {
                    cannot_emit_error(pkg, raw_reference, &format!("unreadable: {e}"))
                })?;
                let filename = package_url_companion_filename(&asset_canonical, &bytes);
                match self.companions.iter().find(|c| c.filename == filename) {
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
                    None => self.companions.push(PackageUrlAsset {
                        filename: filename.clone(),
                        bytes,
                    }),
                }
                self.filename_by_canonical
                    .insert(asset_canonical.clone(), filename.clone());
                filename
            }
        };
        self.asset_paths.insert(asset_canonical);
        Ok(filename)
    }
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
///
/// A newline or other control/non-printable character (e.g. a decoded `\a`)
/// CANNOT use that literal-character escape strategy (codex review finding,
/// #2327): `\` + a literal newline is a *line continuation* inside a quoted
/// string (the character is silently dropped, CSS Syntax §4.3.7), and is not
/// a valid escape at all in an unquoted url-token (the value becomes a
/// bad-url-token and the whole reference is dropped on re-parse, CSS Syntax
/// §4.3.6). These characters are instead re-serialized as a CSS hex escape
/// (`\<hex> `), which round-trips correctly in both quoting contexts. The
/// terminator space is always appended, even when the following character is
/// not itself a hex digit — a redundant terminator is valid CSS and never
/// changes the decoded value, and always appending it is simpler than
/// conditioning on the next character while staying correct when that next
/// character IS a hex digit (which would otherwise be swallowed into the
/// escape).
fn escape_suffix_for_quote(suffix: &str, quote: UrlQuote) -> String {
    let mut out = String::with_capacity(suffix.len());
    for c in suffix.chars() {
        if c.is_control() {
            out.push('\\');
            out.push_str(&format!("{:x}", c as u32));
            out.push(' ');
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_url_rewrite_keeps_asset_and_source_identity() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("node_modules/demo");
        std::fs::create_dir_all(package.join("files")).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(package.join("files/font.woff2"), b"font bytes").unwrap();
        let css = "@font-face { src: url(./files/font.woff2) }";
        let mut resolver = PackageUrlResolver::default();
        let output =
            rewrite_package_urls_for_source(css, &package.join("index.css"), &mut resolver)
                .unwrap();
        assert!(output.contains("url(./font-"), "{output}");
        assert_eq!(resolver.companions.len(), 1);
        assert_eq!(resolver.companions[0].bytes, b"font bytes");
    }
}
