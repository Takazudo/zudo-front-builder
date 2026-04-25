//! URL classification helper.
//!
//! Mirrors zudo-doc's `isExternal()` predicate used by the
//! markdown-link rewriting plugins. A URL is "external" if it has a
//! scheme (`http:`, `https:`, `mailto:`, `tel:`, …) or is
//! protocol-relative (`//host/path`). Anchor-only URLs (`#foo`) and
//! plain relative paths are NOT external.

/// True if `url` is external (scheme-bearing or protocol-relative).
///
/// Definitions:
/// - `http://...`, `https://...`, `mailto:...`, `tel:...`, `ftp:...` →
///   external.
/// - `//cdn.example.com/x` (protocol-relative) → external.
/// - `#anchor`, `./path`, `path`, `/abs/path` → NOT external.
#[must_use]
pub fn is_external(url: &str) -> bool {
    if url.is_empty() {
        return false;
    }
    if url.starts_with('#') {
        return false;
    }
    if url.starts_with("//") {
        return true;
    }
    // Look for a `:` that comes before any `/`, `?`, or `#`. That is the
    // shape of a scheme-bearing URL ("scheme:..."). The first char of
    // the scheme must be ASCII alphabetic (per RFC 3986).
    let bytes = url.as_bytes();
    let first = bytes[0];
    if !first.is_ascii_alphabetic() {
        return false;
    }
    for (i, &b) in bytes.iter().enumerate() {
        if b == b':' {
            // Found a scheme separator before any path/query/fragment
            // character → external.
            return i > 0;
        }
        if b == b'/' || b == b'?' || b == b'#' {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_and_https_are_external() {
        assert!(is_external("http://example.com"));
        assert!(is_external("https://example.com/path?x=1#frag"));
    }

    #[test]
    fn mailto_and_tel_are_external() {
        assert!(is_external("mailto:a@b.c"));
        assert!(is_external("tel:+1-555-0100"));
    }

    #[test]
    fn protocol_relative_is_external() {
        assert!(is_external("//cdn.example.com/lib.js"));
    }

    #[test]
    fn anchor_only_is_internal() {
        assert!(!is_external("#section"));
        assert!(!is_external("#"));
    }

    #[test]
    fn relative_paths_are_internal() {
        assert!(!is_external("foo.md"));
        assert!(!is_external("./foo.md"));
        assert!(!is_external("../bar/baz.mdx"));
        assert!(!is_external("/abs/path.md"));
    }

    #[test]
    fn empty_is_internal() {
        assert!(!is_external(""));
    }
}
