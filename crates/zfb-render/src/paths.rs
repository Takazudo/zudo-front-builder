//! `paths()` runtime: resolve dynamic-route URLs from a page module's
//! exported `paths()` function.
//!
//! For each dynamic route (e.g. `blog/[slug].tsx`, `docs/[...slug].tsx`,
//! `[lang]/[slug].tsx`), the page module is expected to export a `paths()`
//! function that returns an array of `{ params, props }` entries. This
//! module multiplies those entries against the route template into concrete
//! URLs (`ResolvedPath`s) and caches the result keyed by the route template
//! and the JSON shape of the export.
//!
//! # URL encoding (issue #1767)
//!
//! Every dynamic/catch-all value is run through one canonical segment codec
//! before it lands in a resolved URL: the RFC 3986 *unreserved* set
//! (`A-Z a-z 0-9 - . _ ~`) passes through bare, everything else — including
//! a literal `%`, which is always encoded so the codec is injective — is
//! percent-encoded. Catch-all values are encoded **per path component**,
//! never as the joined string (that would encode away the separating `/`).
//! `ResolvedPath.params` keeps the raw, un-encoded author value; only the
//! `url` field carries the encoded form. `.` and `..` components are
//! rejected outright rather than encoded, since a dot-segment changes what
//! the URL *means*, not just how a byte is spelled.
//!
//! **Portability caveat:** Windows reserved device names (`CON`, `PRN`,
//! `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`) and components with trailing
//! dots or spaces are accepted as-is — common-SSG stance, matching how most
//! static-site generators treat `paths()`/route-param values. If a project
//! needs Windows-safe output filenames it must sanitize those cases itself
//! before returning them from `paths()`.
//!
//! See issue #4 (Sub 5).

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::Value;
use thiserror::Error;

// Re-export the canonical Segment type from zfb-types. The previous local
// stub (marked "temporary") is now replaced by the shared definition.
pub use zfb_types::Segment;

/// A concrete resolved URL for a dynamic route, plus the params and props
/// that produced it.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPath {
    /// Concrete URL with leading slash, e.g. `/blog/hello-world`.
    pub url: String,
    /// Param map exactly as it will be passed to the page's `default()`
    /// (catch-all values are stored as the joined path string).
    pub params: HashMap<String, String>,
    /// Props blob extracted from the matching `paths()` entry. Defaults to
    /// `Value::Null` when the entry has no `props` key.
    pub props: Value,
}

/// Errors produced while resolving a `paths()` export.
///
/// Every variant carries `route` — the route template, which by convention is
/// the source TSX file's path relative to `pages/` (e.g. `blog/[slug].tsx`).
/// That string lets editors / build output point at the offending file.
#[derive(Debug, Error)]
pub enum PathsError {
    /// A required dynamic/catch-all param was missing from a `paths()`
    /// entry's `params` object.
    ///
    /// `provided` is the list of keys the entry's `params` object actually
    /// carried (sorted), so a typo like `wrongName` instead of `slug` shows
    /// up directly in the error: "params must include `slug`, got
    /// [`wrongName`]".
    #[error(
        "missing param `{name}` in paths() entry for route `{route}`: \
         params must include `{name}`, got [{provided_pretty}]",
        provided_pretty = pretty_keys(provided),
    )]
    MissingParam {
        name: String,
        route: String,
        provided: Vec<String>,
    },

    /// A `paths()` entry's `params` object contained a key that is not
    /// declared in the route template.
    ///
    /// `expected` is the list of param names declared in the route template
    /// (sorted). Surfacing it next to the offending name produces a clear
    /// "expected one of […], got `wrongName`" diagnostic.
    #[error(
        "extra param `{name}` not declared in route `{route}`: \
         expected one of [{expected_pretty}], got `{name}`",
        expected_pretty = pretty_keys(expected),
    )]
    ExtraParam {
        name: String,
        route: String,
        expected: Vec<String>,
    },

    /// A param value had the wrong JSON shape (e.g. number where a string
    /// was expected, empty string, etc.).
    #[error("invalid param type for `{name}` in route `{route}`: {reason}")]
    InvalidParamType {
        name: String,
        reason: String,
        route: String,
    },

    /// The `paths()` export itself was malformed (not an array, missing
    /// `params`, etc.).
    ///
    /// `route` points at the source file (route template) so the operator
    /// knows which page module's `paths()` returned the wrong shape; `field`
    /// names the bad field when known (e.g. `params`, `entry[2].params.slug`)
    /// so they can find it inside that file. `expected` describes what the
    /// resolver wanted to see.
    #[error(
        "invalid `paths()` export in `{route}`{field_note}: {reason} (expected {expected})",
        field_note = match field {
            Some(f) => format!(" at `{f}`"),
            None => String::new(),
        },
    )]
    InvalidPathsExport {
        route: String,
        field: Option<String>,
        reason: String,
        expected: String,
    },

    /// Two entries resolved to the same URL within a single `paths()`
    /// invocation. Surfaced so the build can fail loudly rather than
    /// silently overwrite an output file.
    #[error("ambiguous resolution for route `{route}`: {reason}")]
    AmbiguousResolution { route: String, reason: String },
}

/// In-memory cache of resolved paths, keyed by route template and the hash
/// of the `paths()` JSON return value. Created once per build and reused
/// across rebuilds — invalidate explicitly when a page module changes.
#[derive(Debug, Default)]
pub struct PathsCache {
    // route_template -> (paths_export hash -> resolved list)
    entries: HashMap<String, HashMap<u64, Vec<ResolvedPath>>>,
    hit_count: u64,
    miss_count: u64,
}

impl PathsCache {
    /// Construct an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop all cached resolutions for `route_template`. Useful when the
    /// underlying page module changes (any `paths_export` variant becomes
    /// stale).
    pub fn invalidate(&mut self, route_template: &str) {
        self.entries.remove(route_template);
    }

    /// Number of times `resolve_paths` returned a cached result. Exposed for
    /// tests and build-time diagnostics.
    pub fn hit_count(&self) -> u64 {
        self.hit_count
    }

    /// Number of times `resolve_paths` had to compute a fresh result.
    pub fn miss_count(&self) -> u64 {
        self.miss_count
    }
}

/// Resolve a single dynamic route's `paths()` export into concrete
/// `ResolvedPath`s.
///
/// - `route_template` — source path (e.g. `blog/[slug].tsx`) used as the
///   primary cache key. Should match exactly what the router uses.
/// - `route_segments` — parsed segments of the route. Determines required
///   param names and how the URL is reassembled.
/// - `paths_export` — JSON-shaped return of the page's `paths()` function:
///   `[{ params: { slug: "hello" }, props: {...} }, ...]`.
///
/// Returns an empty vector for an empty `paths_export` array (a route with
/// no dynamic instances is valid). All errors are deterministic — same
/// inputs produce same outputs (or same error).
pub fn resolve_paths(
    cache: &mut PathsCache,
    route_template: &str,
    route_segments: &[Segment],
    paths_export: &Value,
) -> Result<Vec<ResolvedPath>, PathsError> {
    let hash = hash_value(paths_export);

    if let Some(per_route) = cache.entries.get(route_template) {
        if let Some(cached) = per_route.get(&hash) {
            cache.hit_count += 1;
            return Ok(cached.clone());
        }
    }
    cache.miss_count += 1;

    let entries = paths_export
        .as_array()
        .ok_or_else(|| PathsError::InvalidPathsExport {
            route: route_template.to_string(),
            field: Some("paths()".to_string()),
            reason: format!("got {}", value_kind(paths_export)),
            expected: "an array of `{ params, props }` objects".to_string(),
        })?;

    // Required param names + their kind. Every declared param — including
    // an optional catchall — must appear as a key in each entry's `params`
    // object; "optional" refers to the URL segments it matches (zero is
    // allowed via an explicit empty array), not to the key itself.
    let required: Vec<(&str, ParamKind)> = route_segments
        .iter()
        .filter_map(|seg| match seg {
            Segment::Dynamic(name) => Some((name.as_str(), ParamKind::Dynamic)),
            Segment::Catchall(name) => Some((name.as_str(), ParamKind::Catchall)),
            Segment::OptionalCatchall(name) => Some((name.as_str(), ParamKind::OptionalCatchall)),
            Segment::Static(_) => None,
        })
        .collect();

    let mut result: Vec<ResolvedPath> = Vec::with_capacity(entries.len());
    let mut seen_urls: HashMap<String, usize> = HashMap::new();

    for (i, entry) in entries.iter().enumerate() {
        let entry_obj = entry
            .as_object()
            .ok_or_else(|| PathsError::InvalidPathsExport {
                route: route_template.to_string(),
                field: Some(format!("entry[{i}]")),
                reason: format!("got {}", value_kind(entry)),
                expected: "an object with `params` (and optional `props`)".to_string(),
            })?;

        let params_value =
            entry_obj
                .get("params")
                .ok_or_else(|| PathsError::InvalidPathsExport {
                    route: route_template.to_string(),
                    field: Some(format!("entry[{i}].params")),
                    reason: "missing".to_string(),
                    expected: "an object mapping each route param to a value".to_string(),
                })?;
        let params_obj =
            params_value
                .as_object()
                .ok_or_else(|| PathsError::InvalidPathsExport {
                    route: route_template.to_string(),
                    field: Some(format!("entry[{i}].params")),
                    reason: format!("got {}", value_kind(params_value)),
                    expected: "an object mapping each route param to a value".to_string(),
                })?;

        // Required-param check + value extraction.
        let mut params_map: HashMap<String, String> = HashMap::with_capacity(required.len());
        for (name, kind) in &required {
            let raw = params_obj
                .get(*name)
                .ok_or_else(|| PathsError::MissingParam {
                    name: (*name).to_string(),
                    route: route_template.to_string(),
                    provided: sorted_keys(params_obj.keys()),
                })?;
            let resolved = match kind {
                ParamKind::Dynamic => dynamic_string(raw, name, route_template)?,
                ParamKind::Catchall => catchall_string(raw, name, route_template, false)?,
                ParamKind::OptionalCatchall => catchall_string(raw, name, route_template, true)?,
            };
            params_map.insert((*name).to_string(), resolved);
        }

        // Extra-param check (any key in params_obj not declared by the route).
        for k in params_obj.keys() {
            if !required.iter().any(|(n, _)| *n == k) {
                return Err(PathsError::ExtraParam {
                    name: k.clone(),
                    route: route_template.to_string(),
                    expected: required.iter().map(|(n, _)| (*n).to_string()).collect(),
                });
            }
        }

        let url = build_url(route_segments, &params_map);

        // Detect duplicate URLs within this single resolution call.
        if let Some(prev_idx) = seen_urls.insert(url.clone(), i) {
            return Err(PathsError::AmbiguousResolution {
                route: route_template.to_string(),
                reason: format!("entries at indices {prev_idx} and {i} both resolve to `{url}`"),
            });
        }

        let props = entry_obj.get("props").cloned().unwrap_or(Value::Null);

        result.push(ResolvedPath {
            url,
            params: params_map,
            props,
        });
    }

    cache
        .entries
        .entry(route_template.to_string())
        .or_default()
        .insert(hash, result.clone());

    Ok(result)
}

/// Kind of a declared route param, derived from its [`Segment`] variant.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ParamKind {
    Dynamic,
    Catchall,
    OptionalCatchall,
}

/// Validate and extract a single-segment dynamic param value (must be a
/// non-empty string with no `/`).
fn dynamic_string(val: &Value, name: &str, route: &str) -> Result<String, PathsError> {
    let s = val.as_str().ok_or_else(|| PathsError::InvalidParamType {
        name: name.to_string(),
        reason: format!("dynamic param must be a string, got {}", value_kind(val)),
        route: route.to_string(),
    })?;
    if s.is_empty() {
        return Err(PathsError::InvalidParamType {
            name: name.to_string(),
            reason: "dynamic param must not be empty".to_string(),
            route: route.to_string(),
        });
    }
    if s.contains('/') {
        return Err(PathsError::InvalidParamType {
            name: name.to_string(),
            reason: "dynamic param must not contain `/` (use a catchall `[...name]` for path-shaped values)".to_string(),
            route: route.to_string(),
        });
    }
    if is_dot_segment(s) {
        return Err(PathsError::InvalidParamType {
            name: name.to_string(),
            reason: format!("dynamic param must not be a dot-segment (`.` or `..`), got `{s}`"),
            route: route.to_string(),
        });
    }
    Ok(s.to_string())
}

/// Validate and extract a catch-all param value. Accepts either a single
/// string (`"a/b/c"`) or an array of strings (`["a", "b", "c"]`); both are
/// normalized to a slash-joined string used as one URL segment-bag.
///
/// `optional` is `true` for an optional catchall (`[[...name]]`): the ONLY
/// extra shape it accepts is the explicit empty array `[]` (→ the bare
/// directory URL, normalized to the empty string here). `""`, `"/"`, and
/// `[""]` stay invalid for both forms — zero segments must be spelled
/// `[]`, never an empty string.
fn catchall_string(
    val: &Value,
    name: &str,
    route: &str,
    optional: bool,
) -> Result<String, PathsError> {
    match val {
        Value::String(s) => {
            let trimmed = s.trim_matches('/');
            if trimmed.is_empty() {
                let hint = if optional {
                    " (use an empty array `[]` for the zero-segment case)"
                } else {
                    ""
                };
                return Err(PathsError::InvalidParamType {
                    name: name.to_string(),
                    reason: format!("catchall param string must not be empty{hint}"),
                    route: route.to_string(),
                });
            }
            // Reject empty inner segments: "a//b".
            if trimmed.split('/').any(str::is_empty) {
                return Err(PathsError::InvalidParamType {
                    name: name.to_string(),
                    reason: "catchall param string must not contain empty segments".to_string(),
                    route: route.to_string(),
                });
            }
            if let Some(bad) = trimmed.split('/').find(|part| is_dot_segment(part)) {
                return Err(PathsError::InvalidParamType {
                    name: name.to_string(),
                    reason: format!(
                        "catchall param must not contain a dot-segment component \
                         (`.` or `..`), got `{bad}`"
                    ),
                    route: route.to_string(),
                });
            }
            Ok(trimmed.to_string())
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                if optional {
                    // Zero segments: the optional catchall serves the bare
                    // directory URL. Joined-string convention → empty.
                    return Ok(String::new());
                }
                return Err(PathsError::InvalidParamType {
                    name: name.to_string(),
                    reason: "catchall param array must not be empty".to_string(),
                    route: route.to_string(),
                });
            }
            let mut parts = Vec::with_capacity(arr.len());
            for (i, v) in arr.iter().enumerate() {
                let s = v.as_str().ok_or_else(|| PathsError::InvalidParamType {
                    name: name.to_string(),
                    reason: format!(
                        "catchall param array element {i} must be a string, got {}",
                        value_kind(v)
                    ),
                    route: route.to_string(),
                })?;
                if s.is_empty() {
                    return Err(PathsError::InvalidParamType {
                        name: name.to_string(),
                        reason: format!("catchall param array element {i} must not be empty"),
                        route: route.to_string(),
                    });
                }
                if s.contains('/') {
                    return Err(PathsError::InvalidParamType {
                        name: name.to_string(),
                        reason: format!(
                            "catchall param array element {i} must not contain `/` (split into separate elements)"
                        ),
                        route: route.to_string(),
                    });
                }
                if is_dot_segment(s) {
                    return Err(PathsError::InvalidParamType {
                        name: name.to_string(),
                        reason: format!(
                            "catchall param array element {i} must not be a dot-segment \
                             (`.` or `..`), got `{s}`"
                        ),
                        route: route.to_string(),
                    });
                }
                parts.push(s.to_string());
            }
            Ok(parts.join("/"))
        }
        _ => Err(PathsError::InvalidParamType {
            name: name.to_string(),
            reason: format!(
                "catchall param must be a string or array of strings, got {}",
                value_kind(val)
            ),
            route: route.to_string(),
        }),
    }
}

/// True for a path component that is exactly `.` or `..` — reserved so a
/// resolved URL can never spell a dot-segment (which would otherwise let a
/// `paths()` entry produce a self- or parent-referencing path component).
fn is_dot_segment(s: &str) -> bool {
    s == "." || s == ".."
}

/// The canonical segment codec (issue #1767): RFC 3986 *unreserved* set
/// (`A-Z a-z 0-9 - . _ ~`) is left bare; every other byte of the UTF-8 value
/// — including `%` itself — is percent-encoded. Encoding `%` unconditionally
/// makes the codec injective: a literal `%2F` in an author-supplied value
/// becomes `%252F` in the URL, never collapsing into a real `/`.
///
/// `NON_ALPHANUMERIC` already percent-encodes every ASCII byte outside
/// `[A-Za-z0-9]`, including `%`; removing the four unreserved punctuation
/// bytes from it yields exactly the RFC 3986 unreserved set. Bytes outside
/// the ASCII range (i.e. any non-ASCII UTF-8 byte) are always percent-encoded
/// by `utf8_percent_encode` regardless of the `AsciiSet`, so non-ASCII text
/// is covered without listing it explicitly.
const SEGMENT_UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encode a single path component (a value with no internal `/`)
/// under the canonical codec.
fn encode_segment_component(s: &str) -> String {
    utf8_percent_encode(s, SEGMENT_UNRESERVED).to_string()
}

/// Percent-encode a catch-all value **per component**, then rejoin with `/`.
/// The joined string is never encoded as a whole — that would percent-encode
/// the separating `/` itself and collapse every segment into one.
fn encode_catchall_value(joined: &str) -> String {
    if joined.is_empty() {
        return String::new();
    }
    joined
        .split('/')
        .map(encode_segment_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// Reassemble a URL from a route's segments and the resolved (raw,
/// author-supplied) params map, applying the canonical segment codec to
/// every dynamic/catch-all value. `params` itself stays raw — encoding only
/// ever applies to the URL form built here.
fn build_url(route_segments: &[Segment], params: &HashMap<String, String>) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(route_segments.len());
    for seg in route_segments {
        match seg {
            Segment::Static(s) => parts.push(s.clone()),
            Segment::Dynamic(name) => {
                if let Some(v) = params.get(name) {
                    parts.push(encode_segment_component(v));
                }
            }
            Segment::Catchall(name) => {
                if let Some(v) = params.get(name) {
                    parts.push(encode_catchall_value(v));
                }
            }
            Segment::OptionalCatchall(name) => {
                // Zero segments resolve to the empty string — skip the
                // segment entirely so `/docs` comes out bare, never
                // `/docs/` with a dangling separator.
                if let Some(v) = params.get(name) {
                    if !v.is_empty() {
                        parts.push(encode_catchall_value(v));
                    }
                }
            }
        }
    }
    let mut url = String::with_capacity(64);
    url.push('/');
    url.push_str(&parts.join("/"));
    url
}

/// Collect an iterator of `&String` keys into a sorted owned `Vec<String>`,
/// for stable error output.
fn sorted_keys<'a, I>(iter: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a String>,
{
    let mut v: Vec<String> = iter.into_iter().cloned().collect();
    v.sort();
    v
}

/// Render a list of param names as `` `a`, `b`, `c` `` for embedding in error
/// messages. An empty list renders as the empty string so the surrounding
/// `[...]` shows up as `[]`.
fn pretty_keys(keys: &[String]) -> String {
    let mut parts = Vec::with_capacity(keys.len());
    for k in keys {
        parts.push(format!("`{k}`"));
    }
    parts.join(", ")
}

fn value_kind(val: &Value) -> &'static str {
    match val {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Stable hash of a JSON value: object keys are sorted before hashing so
/// that two semantically-equal `paths()` exports collide in the cache.
fn hash_value(val: &Value) -> u64 {
    let mut canon = String::new();
    canon_into(val, &mut canon);
    let mut hasher = DefaultHasher::new();
    canon.hash(&mut hasher);
    hasher.finish()
}

fn canon_into(val: &Value, out: &mut String) {
    match val {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => push_canon_string(s, out),
        Value::Array(arr) => {
            out.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canon_into(v, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                push_canon_string(k, out);
                out.push(':');
                canon_into(&map[*k], out);
            }
            out.push('}');
        }
    }
}

fn push_canon_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn route_blog_slug() -> Vec<Segment> {
        vec![
            Segment::Static("blog".to_string()),
            Segment::Dynamic("slug".to_string()),
        ]
    }

    fn route_docs_catchall() -> Vec<Segment> {
        vec![
            Segment::Static("docs".to_string()),
            Segment::Catchall("slug".to_string()),
        ]
    }

    fn route_lang_slug() -> Vec<Segment> {
        vec![
            Segment::Dynamic("lang".to_string()),
            Segment::Dynamic("slug".to_string()),
        ]
    }

    #[test]
    fn single_dynamic_resolves_one_url() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([
            { "params": { "slug": "hello-world" }, "props": { "title": "Hello" } }
        ]);

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "/blog/hello-world");
        assert_eq!(out[0].params.get("slug").unwrap(), "hello-world");
        assert_eq!(out[0].props, json!({ "title": "Hello" }));
    }

    #[test]
    fn multiple_dynamic_entries_resolve_to_multiple_urls() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([
            { "params": { "slug": "a" } },
            { "params": { "slug": "b" } },
            { "params": { "slug": "c" } },
            { "params": { "slug": "d" } },
            { "params": { "slug": "e" } }
        ]);

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();

        assert_eq!(out.len(), 5);
        let urls: Vec<&str> = out.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["/blog/a", "/blog/b", "/blog/c", "/blog/d", "/blog/e"]
        );
        // props default to null when absent
        assert!(out.iter().all(|r| r.props.is_null()));
    }

    #[test]
    fn catchall_string_value_resolves_full_path() {
        let mut cache = PathsCache::new();
        let segs = route_docs_catchall();
        let export = json!([
            { "params": { "slug": "a/b/c" } }
        ]);

        let out = resolve_paths(&mut cache, "docs/[...slug].tsx", &segs, &export).unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "/docs/a/b/c");
        assert_eq!(out[0].params.get("slug").unwrap(), "a/b/c");
    }

    #[test]
    fn catchall_array_value_resolves_full_path() {
        let mut cache = PathsCache::new();
        let segs = route_docs_catchall();
        let export = json!([
            { "params": { "slug": ["a", "b"] } }
        ]);

        let out = resolve_paths(&mut cache, "docs/[...slug].tsx", &segs, &export).unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "/docs/a/b");
        assert_eq!(out[0].params.get("slug").unwrap(), "a/b");
    }

    #[test]
    fn catchall_string_with_leading_or_trailing_slashes_is_normalized() {
        let mut cache = PathsCache::new();
        let segs = route_docs_catchall();
        let export = json!([
            { "params": { "slug": "/a/b/" } }
        ]);

        let out = resolve_paths(&mut cache, "docs/[...slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/docs/a/b");
    }

    #[test]
    fn missing_param_is_an_error() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([
            { "params": { "wrongName": "x" } }
        ]);

        let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap_err();
        let msg = err.to_string();
        // The message must point at the source file path.
        assert!(
            msg.contains("blog/[slug].tsx"),
            "expected route file path in error, got: {msg}",
        );
        // The message must say what was expected and what was provided.
        assert!(
            msg.contains("`slug`") && msg.contains("`wrongName`"),
            "expected `expected/got` param diagnostic in error, got: {msg}",
        );
        match err {
            PathsError::MissingParam {
                name,
                route,
                provided,
            } => {
                assert_eq!(name, "slug");
                assert_eq!(route, "blog/[slug].tsx");
                assert_eq!(provided, vec!["wrongName".to_string()]);
            }
            other => panic!("expected MissingParam, got {other:?}"),
        }
    }

    #[test]
    fn missing_param_with_empty_params_lists_no_provided_keys() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([
            { "params": {} }
        ]);

        let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("blog/[slug].tsx"), "got: {msg}");
        assert!(msg.contains("got []"), "got: {msg}");
        match err {
            PathsError::MissingParam { provided, .. } => {
                assert!(
                    provided.is_empty(),
                    "expected empty provided, got {provided:?}"
                );
            }
            other => unreachable!("expected MissingParam, got {other:?}"),
        }
    }

    #[test]
    fn extra_param_not_in_route_is_an_error() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([
            { "params": { "slug": "hi", "unexpected": "x" } }
        ]);

        let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("blog/[slug].tsx"), "got: {msg}");
        assert!(
            msg.contains("`slug`") && msg.contains("`unexpected`"),
            "expected `expected one of … got …` diagnostic, got: {msg}",
        );
        match err {
            PathsError::ExtraParam {
                name,
                route,
                expected,
            } => {
                assert_eq!(name, "unexpected");
                assert_eq!(route, "blog/[slug].tsx");
                assert_eq!(expected, vec!["slug".to_string()]);
            }
            other => unreachable!("expected ExtraParam, got {other:?}"),
        }
    }

    #[test]
    fn dynamic_param_must_be_a_string() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([
            { "params": { "slug": 42 } }
        ]);

        let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::InvalidParamType { .. }));
    }

    #[test]
    fn dynamic_param_must_not_contain_slash() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([
            { "params": { "slug": "a/b" } }
        ]);

        let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::InvalidParamType { .. }));
    }

    #[test]
    fn cache_hit_returns_same_value_without_recomputation() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([
            { "params": { "slug": "x" } }
        ]);

        let first = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert_eq!(cache.hit_count(), 0);
        assert_eq!(cache.miss_count(), 1);

        let second = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert_eq!(cache.hit_count(), 1);
        assert_eq!(cache.miss_count(), 1);

        assert_eq!(first, second);
    }

    #[test]
    fn cache_keys_distinguish_different_paths_exports() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let a = json!([{ "params": { "slug": "a" } }]);
        let b = json!([{ "params": { "slug": "b" } }]);

        let _ = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &a).unwrap();
        let _ = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &b).unwrap();

        // Different exports → both miss, neither is a hit.
        assert_eq!(cache.miss_count(), 2);
        assert_eq!(cache.hit_count(), 0);

        // Re-running the original now hits.
        let _ = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &a).unwrap();
        assert_eq!(cache.hit_count(), 1);
    }

    #[test]
    fn invalidation_clears_all_variants_for_a_route() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let a = json!([{ "params": { "slug": "a" } }]);
        let b = json!([{ "params": { "slug": "b" } }]);

        let _ = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &a).unwrap();
        let _ = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &b).unwrap();
        let _ = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &a).unwrap();
        assert_eq!(cache.hit_count(), 1);

        cache.invalidate("blog/[slug].tsx");

        // Both variants are now misses again.
        let _ = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &a).unwrap();
        let _ = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &b).unwrap();
        assert_eq!(cache.miss_count(), 4);
    }

    #[test]
    fn empty_paths_array_yields_empty_result() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([]);

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn nested_dynamic_resolves_each_combo() {
        let mut cache = PathsCache::new();
        let segs = route_lang_slug();
        let export = json!([
            { "params": { "lang": "en", "slug": "hello" }, "props": { "i": 1 } },
            { "params": { "lang": "ja", "slug": "hello" }, "props": { "i": 2 } },
            { "params": { "lang": "en", "slug": "world" }, "props": { "i": 3 } },
            { "params": { "lang": "ja", "slug": "world" }, "props": { "i": 4 } }
        ]);

        let out = resolve_paths(&mut cache, "[lang]/[slug].tsx", &segs, &export).unwrap();

        let urls: Vec<&str> = out.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["/en/hello", "/ja/hello", "/en/world", "/ja/world"]
        );
        assert_eq!(out[0].props, json!({ "i": 1 }));
        assert_eq!(out[3].props, json!({ "i": 4 }));
        // Each row's params map carries both keys.
        for r in &out {
            assert!(r.params.contains_key("lang"));
            assert!(r.params.contains_key("slug"));
        }
    }

    #[test]
    fn props_default_to_null_when_absent() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": "x" } }]);
        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert!(out[0].props.is_null());
    }

    #[test]
    fn paths_export_must_be_an_array() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!({ "not": "an array" });
        let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::InvalidPathsExport { .. }));
    }

    #[test]
    fn entry_must_have_params_object() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "props": {} }]);
        let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::InvalidPathsExport { .. }));
    }

    #[test]
    fn duplicate_resolved_url_is_ambiguous() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([
            { "params": { "slug": "dup" } },
            { "params": { "slug": "dup" } }
        ]);
        let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap_err();
        match err {
            PathsError::AmbiguousResolution { route, .. } => {
                assert_eq!(route, "blog/[slug].tsx");
            }
            other => unreachable!("expected AmbiguousResolution, got {other:?}"),
        }
    }

    #[test]
    fn cache_keys_treat_object_key_order_as_equal() {
        // Object key order in JSON shouldn't break cache hits.
        let mut cache = PathsCache::new();
        let segs = route_lang_slug();
        let a = json!([{ "params": { "lang": "en", "slug": "x" } }]);
        let b = json!([{ "params": { "slug": "x", "lang": "en" } }]);

        let _ = resolve_paths(&mut cache, "[lang]/[slug].tsx", &segs, &a).unwrap();
        let _ = resolve_paths(&mut cache, "[lang]/[slug].tsx", &segs, &b).unwrap();
        assert_eq!(cache.hit_count(), 1);
    }

    // ---- optional catchall `[[...slug]]` (#812) -----------------------------

    fn route_docs_optional_catchall() -> Vec<Segment> {
        vec![
            Segment::Static("docs".to_string()),
            Segment::OptionalCatchall("slug".to_string()),
        ]
    }

    #[test]
    fn optional_catchall_empty_array_resolves_bare_url() {
        let mut cache = PathsCache::new();
        let segs = route_docs_optional_catchall();
        let export = json!([
            { "params": { "slug": [] }, "props": { "title": "Docs home" } }
        ]);

        let out = resolve_paths(&mut cache, "docs/[[...slug]].tsx", &segs, &export).unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].url, "/docs",
            "zero segments must map to the bare URL"
        );
        assert_eq!(out[0].params.get("slug").unwrap(), "");
        assert_eq!(out[0].props, json!({ "title": "Docs home" }));
    }

    #[test]
    fn optional_catchall_nested_paths_still_resolve() {
        let mut cache = PathsCache::new();
        let segs = route_docs_optional_catchall();
        let export = json!([
            { "params": { "slug": [] } },
            { "params": { "slug": ["a", "b"] } },
            { "params": { "slug": "c/d" } }
        ]);

        let out = resolve_paths(&mut cache, "docs/[[...slug]].tsx", &segs, &export).unwrap();

        let urls: Vec<&str> = out.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(urls, vec!["/docs", "/docs/a/b", "/docs/c/d"]);
    }

    #[test]
    fn top_level_optional_catchall_zero_segments_is_root() {
        let mut cache = PathsCache::new();
        let segs = vec![Segment::OptionalCatchall("rest".to_string())];
        let export = json!([{ "params": { "rest": [] } }]);

        let out = resolve_paths(&mut cache, "[[...rest]].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/");
    }

    #[test]
    fn optional_catchall_rejects_empty_string() {
        // `""` stays invalid — zero segments must be spelled `[]`.
        let mut cache = PathsCache::new();
        let segs = route_docs_optional_catchall();
        let export = json!([{ "params": { "slug": "" } }]);

        let err = resolve_paths(&mut cache, "docs/[[...slug]].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::InvalidParamType { .. }));
        assert!(
            err.to_string().contains("[]"),
            "error should hint at the `[]` spelling: {err}",
        );
    }

    #[test]
    fn optional_catchall_rejects_slash_string() {
        let mut cache = PathsCache::new();
        let segs = route_docs_optional_catchall();
        let export = json!([{ "params": { "slug": "/" } }]);

        let err = resolve_paths(&mut cache, "docs/[[...slug]].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::InvalidParamType { .. }));
    }

    #[test]
    fn optional_catchall_rejects_array_with_empty_element() {
        let mut cache = PathsCache::new();
        let segs = route_docs_optional_catchall();
        let export = json!([{ "params": { "slug": [""] } }]);

        let err = resolve_paths(&mut cache, "docs/[[...slug]].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::InvalidParamType { .. }));
    }

    #[test]
    fn optional_catchall_still_requires_the_param_key() {
        // The zero case needs an explicit `slug: []` — a missing key is
        // still a MissingParam error, not an implicit empty.
        let mut cache = PathsCache::new();
        let segs = route_docs_optional_catchall();
        let export = json!([{ "params": {} }]);

        let err = resolve_paths(&mut cache, "docs/[[...slug]].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::MissingParam { .. }));
    }

    #[test]
    fn optional_catchall_duplicate_bare_urls_are_ambiguous() {
        let mut cache = PathsCache::new();
        let segs = route_docs_optional_catchall();
        let export = json!([
            { "params": { "slug": [] } },
            { "params": { "slug": [] } }
        ]);

        let err = resolve_paths(&mut cache, "docs/[[...slug]].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::AmbiguousResolution { .. }));
    }

    #[test]
    fn required_catchall_still_rejects_empty_array() {
        // The strict `[...slug]` form keeps its guard: `[]` is invalid.
        let mut cache = PathsCache::new();
        let segs = route_docs_catchall();
        let export = json!([{ "params": { "slug": [] } }]);

        let err = resolve_paths(&mut cache, "docs/[...slug].tsx", &segs, &export).unwrap_err();
        match err {
            PathsError::InvalidParamType { reason, .. } => {
                assert!(
                    reason.contains("must not be empty"),
                    "unexpected reason: {reason}",
                );
            }
            other => unreachable!("expected InvalidParamType, got {other:?}"),
        }
    }

    #[test]
    fn required_catchall_still_rejects_empty_string() {
        let mut cache = PathsCache::new();
        let segs = route_docs_catchall();
        let export = json!([{ "params": { "slug": "" } }]);

        let err = resolve_paths(&mut cache, "docs/[...slug].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::InvalidParamType { .. }));
    }

    // ---- canonical segment codec (#1767) ------------------------------

    #[test]
    fn dynamic_param_percent_encodes_question_mark() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": "a?b" } }]);

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/blog/a%3Fb");
        // params stays raw, not encoded.
        assert_eq!(out[0].params.get("slug").unwrap(), "a?b");
    }

    #[test]
    fn dynamic_param_percent_encodes_hash() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": "a#b" } }]);

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/blog/a%23b");
    }

    #[test]
    fn dynamic_param_percent_encodes_literal_percent() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": "a%b" } }]);

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/blog/a%25b");
    }

    #[test]
    fn dynamic_param_percent_encodes_space() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": "a b" } }]);

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/blog/a%20b");
    }

    #[test]
    fn dynamic_param_percent_encodes_non_ascii() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": "\u{3042}" } }]); // "あ"

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/blog/%E3%81%82");
    }

    #[test]
    fn dynamic_param_literal_percent_2f_is_lossless_not_a_slash() {
        // A literal "%2F" in the author-supplied value must not decode into
        // a real path separator: the `%` is itself encoded, so the value
        // round-trips losslessly as `%252F`.
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": "a%2Fb" } }]);

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/blog/a%252Fb");
    }

    #[test]
    fn dynamic_param_percent_encodes_backslash() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": "a\\b" } }]);

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/blog/a%5Cb");
    }

    #[test]
    fn dynamic_param_percent_encodes_control_byte() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": "a\u{0007}b" } }]); // BEL

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/blog/a%07b");
    }

    #[test]
    fn dynamic_param_rejects_dot_segment() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": "." } }]);

        let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap_err();
        match err {
            PathsError::InvalidParamType { reason, .. } => {
                assert!(reason.contains("dot-segment"), "got: {reason}");
            }
            other => unreachable!("expected InvalidParamType, got {other:?}"),
        }
    }

    #[test]
    fn dynamic_param_rejects_dot_dot_segment() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": ".." } }]);

        let err = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::InvalidParamType { .. }));
    }

    #[test]
    fn catchall_string_encodes_per_component_not_joined() {
        let mut cache = PathsCache::new();
        let segs = route_docs_catchall();
        let export = json!([{ "params": { "slug": "a?b/c#d" } }]);

        let out = resolve_paths(&mut cache, "docs/[...slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/docs/a%3Fb/c%23d");
        // Raw params stays the un-encoded, slash-joined string.
        assert_eq!(out[0].params.get("slug").unwrap(), "a?b/c#d");
    }

    #[test]
    fn catchall_array_encodes_each_element() {
        let mut cache = PathsCache::new();
        let segs = route_docs_catchall();
        let export = json!([{ "params": { "slug": ["a b", "c%d"] } }]);

        let out = resolve_paths(&mut cache, "docs/[...slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/docs/a%20b/c%25d");
    }

    #[test]
    fn catchall_string_rejects_dot_segment_component() {
        let mut cache = PathsCache::new();
        let segs = route_docs_catchall();
        let export = json!([{ "params": { "slug": "a/../b" } }]);

        let err = resolve_paths(&mut cache, "docs/[...slug].tsx", &segs, &export).unwrap_err();
        match err {
            PathsError::InvalidParamType { reason, .. } => {
                assert!(reason.contains("dot-segment"), "got: {reason}");
            }
            other => unreachable!("expected InvalidParamType, got {other:?}"),
        }
    }

    #[test]
    fn catchall_array_rejects_dot_segment_element() {
        let mut cache = PathsCache::new();
        let segs = route_docs_catchall();
        let export = json!([{ "params": { "slug": ["a", "."] } }]);

        let err = resolve_paths(&mut cache, "docs/[...slug].tsx", &segs, &export).unwrap_err();
        assert!(matches!(err, PathsError::InvalidParamType { .. }));
    }

    #[test]
    fn optional_catchall_still_encodes_when_non_empty() {
        let mut cache = PathsCache::new();
        let segs = route_docs_optional_catchall();
        let export = json!([{ "params": { "slug": ["a b"] } }]);

        let out = resolve_paths(&mut cache, "docs/[[...slug]].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/docs/a%20b");
    }

    #[test]
    fn unreserved_characters_pass_through_unencoded() {
        let mut cache = PathsCache::new();
        let segs = route_blog_slug();
        let export = json!([{ "params": { "slug": "Hello-World_v1.2~3" } }]);

        let out = resolve_paths(&mut cache, "blog/[slug].tsx", &segs, &export).unwrap();
        assert_eq!(out[0].url, "/blog/Hello-World_v1.2~3");
    }
}
