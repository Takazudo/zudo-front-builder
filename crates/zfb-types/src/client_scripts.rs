//! Canonical filename contract for `*.client.{ts,tsx,js,jsx}` client-script
//! entries.
//!
//! ## Why this lives in `zfb-types`
//!
//! Two unrelated crates need to agree on what a "client-script file" is:
//!
//! - `zfb-islands` discovers `.client.*` files and bundles them.
//! - `zfb-router` scans `pages/` for routes and must **skip** `.client.*`
//!   files, or a `pages/search-widget.client.tsx` would be both bundled as a
//!   client script AND accepted as a `/search-widget.client` route (it has no
//!   page default export, so the build/render then fails or ships an
//!   unintended route).
//!
//! `zfb-router` cannot depend on `zfb-islands` (a heavier crate pulling in
//! swc / esbuild), so the filename contract — the `.client.` infix, the
//! accepted extensions, and the predicate / entry-name helpers — lives here in
//! the zero-zfb-dep leaf crate both can already depend on. `zfb-islands`
//! re-consumes these so there is a single source of truth for the string
//! literals (no duplicated `".client."` across crates).

use std::path::Path;

/// File extensions recognised as `.client.*` entries.
pub const CLIENT_SCRIPT_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx"];

/// The `.client.` infix that marks a file as a client-script entry.
pub const CLIENT_SCRIPT_INFIX: &str = ".client.";

/// Returns `true` when `path` looks like a `*.client.{ts,tsx,js,jsx}` file.
///
/// The check is purely filename-based — no filesystem access. A bare
/// `.client.ts` (no stem before the infix) is **not** a valid entry.
pub fn is_client_script_file(path: &Path) -> bool {
    let file_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    for ext in CLIENT_SCRIPT_EXTENSIONS {
        let suffix = format!("{CLIENT_SCRIPT_INFIX}{ext}");
        if file_name.ends_with(&suffix) && file_name.len() > suffix.len() {
            return true;
        }
    }
    false
}

/// Derive the entry name from a `*.client.<ext>` file path.
///
/// Returns `Some("search-widget")` for `path/search-widget.client.ts`, or
/// `None` if the file does not match the convention.
pub fn client_script_entry_name(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    for ext in CLIENT_SCRIPT_EXTENSIONS {
        let suffix = format!("{CLIENT_SCRIPT_INFIX}{ext}");
        if file_name.ends_with(&suffix) {
            let stem = &file_name[..file_name.len() - suffix.len()];
            if !stem.is_empty() {
                return Some(stem.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn is_client_script_file_matches_all_extensions() {
        for ext in ["ts", "tsx", "js", "jsx"] {
            let path = PathBuf::from(format!("components/search-widget.client.{ext}"));
            assert!(
                is_client_script_file(&path),
                "expected match for .client.{ext}"
            );
        }
    }

    #[test]
    fn is_client_script_file_rejects_non_client_files() {
        let non_client = [
            "components/button.tsx",
            "components/button.ts",
            "pages/index.tsx",
            "src/utils.ts",
            "analytics.js",
        ];
        for p in &non_client {
            assert!(
                !is_client_script_file(Path::new(p)),
                "should not match: {p}"
            );
        }
    }

    #[test]
    fn is_client_script_file_rejects_bare_client_prefix() {
        assert!(!is_client_script_file(Path::new(".client.ts")));
        assert!(!is_client_script_file(Path::new("components/.client.ts")));
    }

    #[test]
    fn client_script_entry_name_strips_client_suffix() {
        assert_eq!(
            client_script_entry_name(Path::new("components/search-widget.client.ts")),
            Some("search-widget".into())
        );
        assert_eq!(
            client_script_entry_name(Path::new("pages/analytics.client.tsx")),
            Some("analytics".into())
        );
        assert_eq!(
            client_script_entry_name(Path::new("src/my-lib.client.js")),
            Some("my-lib".into())
        );
        assert_eq!(
            client_script_entry_name(Path::new("src/fancy.client.jsx")),
            Some("fancy".into())
        );
    }

    #[test]
    fn client_script_entry_name_returns_none_for_non_client_files() {
        assert_eq!(
            client_script_entry_name(Path::new("components/button.tsx")),
            None
        );
        assert_eq!(client_script_entry_name(Path::new("index.ts")), None);
    }
}
