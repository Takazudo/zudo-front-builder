//! Resolve a CSS entry's `@import` graph to canonicalised real file paths
//! (D2 of bug #1284 / sub-issue #1288).
//!
//! ## Why this exists
//!
//! zfb shells out to the Tailwind v4 standalone CLI as an opaque `-i/-o`
//! transform ([`crate::engine`]). Tailwind resolves `@import` targets
//! invisibly and exposes **no machine-readable dependency manifest**, so zfb
//! never learns the real on-disk paths of the CSS files an entry transitively
//! pulls in. That breaks dev invalidation two ways:
//!
//! 1. editing a transitively-imported CSS file (`@import './tokens.css';`)
//!    does not refresh `/assets/styles.css`, because no dependency edge / watch
//!    target exists for it; and
//! 2. a **workspace** dep consumed via `@import '@scope/design-system'` resolves
//!    through a `node_modules` symlink — `notify` (started recursive) does not
//!    follow symlinks, and `node_modules` is excluded anyway, so the real file
//!    is never watched.
//!
//! This module parses the `@import` graph in Rust and returns each target's
//! **canonicalised real path** ([`std::fs::canonicalize`], which follows the
//! workspace symlink to the real file). Those real paths are the watch targets
//! the dev layer registers (D4) and the Style edges the graph records.
//!
//! ## Scope (per #1286 D2)
//!
//! - Resolves `@import "..."` and `@import url(...)` targets that resolve to a
//!   file on disk: **relative** specifiers (`./`, `../`, bare-relative like
//!   `tokens.css`) against the importing file's directory, and **bare package**
//!   specifiers (`@scope/pkg`, `pkg/sub.css`) against `node_modules` (walking up
//!   from the importing file, so a workspace/pnpm layout resolves).
//! - **Recurses** into resolved CSS files so a chain
//!   `entry → a.css → b.css` surfaces both `a.css` and `b.css`.
//! - **Skips** `@import "tailwindcss"` (and `tailwindcss/...`) and other
//!   virtual/builtin specifiers that do not resolve to a real on-disk file
//!   (e.g. `@import "tw-animate-css"` with no installed file is simply dropped).
//! - Returns canonicalised real paths, de-duplicated and sorted for stable
//!   downstream ordering. The entry itself is **not** included.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Specifiers that name Tailwind's own virtual entry — never an on-disk file
/// zfb should resolve or watch.
fn is_virtual_specifier(spec: &str) -> bool {
    spec == "tailwindcss" || spec.starts_with("tailwindcss/")
}

/// Resolve the transitive `@import` graph rooted at `entry`, returning the
/// canonicalised real paths of every imported CSS file that resolves to a file
/// on disk.
///
/// `entry` is the CSS entry file (e.g. the consumer's authored global
/// stylesheet). It is read from disk; a missing/unreadable entry yields an
/// empty set. The entry path itself is never included in the result.
///
/// `project_root` anchors `node_modules` lookups when the importing file lives
/// outside the project tree (a canonicalised symlink target); the per-file
/// upward walk from each importer is the primary mechanism and covers the
/// in-repo and pnpm/workspace cases.
pub fn resolve_css_imports(entry: &Path, project_root: &Path) -> Vec<PathBuf> {
    let mut seen_real: BTreeSet<PathBuf> = BTreeSet::new();
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();

    // Seed the walk with the entry's own real path so a self-referential
    // `@import` cycle terminates. The entry is excluded from `out`.
    let entry_real = std::fs::canonicalize(entry).unwrap_or_else(|_| entry.to_path_buf());
    seen_real.insert(entry_real.clone());

    let mut stack: Vec<PathBuf> = vec![entry_real];
    while let Some(importer_real) = stack.pop() {
        let text = match std::fs::read_to_string(&importer_real) {
            Ok(t) => t,
            Err(_) => continue,
        };
        for spec in extract_import_specifiers(&text) {
            if is_virtual_specifier(&spec) {
                continue;
            }
            let Some(resolved) = resolve_one(&importer_real, &spec, project_root) else {
                // Unresolvable (virtual/builtin or not installed) — skip.
                continue;
            };
            let real = match std::fs::canonicalize(&resolved) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if seen_real.insert(real.clone()) {
                out.insert(real.clone());
                stack.push(real);
            }
        }
    }

    out.into_iter().collect()
}

/// Resolve a single `@import` specifier against the importing file.
///
/// Relative specifiers (`./`, `../`, or a bare-relative filename like
/// `tokens.css`) resolve against the importer's directory. Package specifiers
/// (`@scope/pkg`, `pkg/sub.css`) resolve against the nearest `node_modules`
/// walking up from the importer; a directory target falls back to the package's
/// `package.json` `style`/`exports`/`main` CSS entry, then `index.css`.
fn resolve_one(importer_real: &Path, spec: &str, project_root: &Path) -> Option<PathBuf> {
    let importer_dir = importer_real.parent().unwrap_or(Path::new("."));

    if is_relative_specifier(spec) {
        let candidate = importer_dir.join(spec);
        return file_or_css_index(&candidate);
    }

    // Bare package specifier — walk up looking for node_modules.
    resolve_package_specifier(importer_dir, project_root, spec)
}

/// A relative specifier resolves against the importing file's directory.
/// Anything that is not an absolute path and not a bare package name (no
/// leading `@scope` / `pkg` form) is treated as relative — including a plain
/// `tokens.css` next to the importer.
fn is_relative_specifier(spec: &str) -> bool {
    spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/') || {
        // A bare token with a CSS-ish extension and no path separator that
        // does NOT look like a package (no `@scope`) is treated as relative
        // (e.g. `theme.css`). A token like `design-system` with no extension
        // is treated as a package specifier.
        !spec.starts_with('@') && spec.contains('.') && !spec.contains('/')
    }
}

/// Resolve a package specifier (`@scope/pkg`, `pkg`, `pkg/sub.css`) against the
/// nearest `node_modules` directory, walking up from `start_dir` and finally
/// trying `project_root/node_modules`.
fn resolve_package_specifier(start_dir: &Path, project_root: &Path, spec: &str) -> Option<PathBuf> {
    let (pkg_name, subpath) = split_package_specifier(spec)?;

    let mut search_dirs: Vec<PathBuf> = Vec::new();
    let mut dir = Some(start_dir);
    while let Some(d) = dir {
        search_dirs.push(d.join("node_modules"));
        dir = d.parent();
    }
    search_dirs.push(project_root.join("node_modules"));

    for nm in &search_dirs {
        let pkg_root = nm.join(&pkg_name);
        if !pkg_root.is_dir() {
            continue;
        }
        // Explicit subpath (`pkg/dist/tokens.css`) — resolve it directly.
        if let Some(sub) = &subpath {
            if let Some(hit) = file_or_css_index(&pkg_root.join(sub)) {
                return Some(hit);
            }
            continue;
        }
        // Bare package — consult package.json for a CSS entry, then index.css.
        if let Some(hit) = package_css_entry(&pkg_root) {
            return Some(hit);
        }
        if let Some(hit) = file_or_css_index(&pkg_root) {
            return Some(hit);
        }
    }
    None
}

/// Split a package specifier into `(package_name, optional_subpath)`.
/// `@scope/pkg/sub.css` → (`@scope/pkg`, `sub.css`); `pkg/sub` → (`pkg`,
/// `sub`); `pkg` → (`pkg`, None).
fn split_package_specifier(spec: &str) -> Option<(String, Option<String>)> {
    if spec.is_empty() {
        return None;
    }
    let parts: Vec<&str> = spec
        .splitn(if spec.starts_with('@') { 3 } else { 2 }, '/')
        .collect();
    if spec.starts_with('@') {
        match parts.as_slice() {
            [scope, pkg] => Some((format!("{scope}/{pkg}"), None)),
            [scope, pkg, sub] => Some((format!("{scope}/{pkg}"), Some((*sub).to_string()))),
            _ => None,
        }
    } else {
        match parts.as_slice() {
            [pkg] => Some(((*pkg).to_string(), None)),
            [pkg, sub] => Some(((*pkg).to_string(), Some((*sub).to_string()))),
            _ => None,
        }
    }
}

/// Read a package's `package.json` and return the CSS entry it advertises, in
/// precedence order: `style`, an `exports` map's `"."` → `style`/`default` CSS
/// value, then `main` when it points at a `.css` file. Returns the resolved
/// on-disk path (un-canonicalised; the caller canonicalises).
fn package_css_entry(pkg_root: &Path) -> Option<PathBuf> {
    let manifest = std::fs::read_to_string(pkg_root.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&manifest).ok()?;

    if let Some(style) = json.get("style").and_then(|v| v.as_str()) {
        if let Some(hit) = file_or_css_index(&pkg_root.join(style)) {
            return Some(hit);
        }
    }
    if let Some(entry) = exports_css_entry(json.get("exports")) {
        if let Some(hit) = file_or_css_index(&pkg_root.join(entry)) {
            return Some(hit);
        }
    }
    if let Some(main) = json.get("main").and_then(|v| v.as_str()) {
        if main.ends_with(".css") {
            if let Some(hit) = file_or_css_index(&pkg_root.join(main)) {
                return Some(hit);
            }
        }
    }
    None
}

/// Extract a CSS-looking target from an `exports` field's root (`"."`) entry.
/// Handles a bare string export, and a conditions object with `style` /
/// `default` keys.
fn exports_css_entry(exports: Option<&serde_json::Value>) -> Option<String> {
    let exports = exports?;
    let root = match exports {
        serde_json::Value::String(s) => return ends_with_css(s),
        // `exports."."` when present, else treat the object itself as the
        // condition map (some packages put conditions at the top level).
        serde_json::Value::Object(map) => map.get(".").unwrap_or(exports),
        _ => return None,
    };
    match root {
        serde_json::Value::String(s) => ends_with_css(s),
        serde_json::Value::Object(map) => map
            .get("style")
            .or_else(|| map.get("default"))
            .and_then(|v| v.as_str())
            .and_then(ends_with_css),
        _ => None,
    }
}

fn ends_with_css(s: &str) -> Option<String> {
    if s.ends_with(".css") {
        Some(s.to_string())
    } else {
        None
    }
}

/// Return `candidate` if it is an existing file; if it is a directory, try
/// `<candidate>/index.css`. Returns `None` when nothing on disk matches.
fn file_or_css_index(candidate: &Path) -> Option<PathBuf> {
    if candidate.is_file() {
        return Some(candidate.to_path_buf());
    }
    if candidate.is_dir() {
        let idx = candidate.join("index.css");
        if idx.is_file() {
            return Some(idx);
        }
    }
    None
}

/// Extract every `@import` target specifier from CSS text.
///
/// Recognises both `@import "target";` / `@import 'target';` and
/// `@import url("target");` / `@import url(target);`. Comment- and
/// string-aware enough for real stylesheets: block comments (`/* … */`) are
/// stripped before scanning, and a media-query / layer suffix after the target
/// (`@import "x.css" screen;`) is ignored because we stop at the target token.
fn extract_import_specifiers(css: &str) -> Vec<String> {
    let without_comments = strip_block_comments(css);
    let mut out: Vec<String> = Vec::new();
    let bytes = without_comments.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Find the next `@import`. Compare on raw bytes (case-insensitive) rather
        // than slicing the `&str` + allocating a lowercased suffix: the suffix-copy
        // form was O(n^2) on `@`-dense files, and slicing `without_comments[i..]`
        // can panic if a stray non-ASCII byte leaves `i` mid-codepoint.
        if bytes[i] == b'@' && ascii_ci_starts_with(bytes, i, b"@import") {
            let mut j = i + "@import".len();
            // Skip whitespace.
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            // Optional `url(`.
            if ascii_ci_starts_with(bytes, j, b"url(") {
                j += "url(".len();
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
            }
            if let Some((spec, next)) = read_target(&without_comments, j) {
                if !spec.is_empty() {
                    out.push(spec);
                }
                i = next;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Read an `@import` target starting at byte offset `start`. Handles a quoted
/// target (`"x"` / `'x'`) or a bare `url(x)` target. Returns the target string
/// and the offset just past it.
fn read_target(css: &str, start: usize) -> Option<(String, usize)> {
    let bytes = css.as_bytes();
    if start >= bytes.len() {
        return None;
    }
    let q = bytes[start];
    if q == b'"' || q == b'\'' {
        let mut k = start + 1;
        let s = k;
        while k < bytes.len() && bytes[k] != q {
            k += 1;
        }
        if k >= bytes.len() {
            return None;
        }
        return Some((css[s..k].to_string(), k + 1));
    }
    // Bare `url(target)` form (no quotes) — read until `)` or whitespace.
    let s = start;
    let mut k = start;
    while k < bytes.len() && bytes[k] != b')' && !bytes[k].is_ascii_whitespace() && bytes[k] != b';'
    {
        k += 1;
    }
    if k == s {
        return None;
    }
    Some((css[s..k].trim().to_string(), k))
}

/// ASCII case-insensitive `starts_with` at byte offset `at`, operating purely on
/// bytes so it never allocates and never slices on a char boundary.
fn ascii_ci_starts_with(bytes: &[u8], at: usize, needle: &[u8]) -> bool {
    bytes.len() >= at + needle.len() && bytes[at..at + needle.len()].eq_ignore_ascii_case(needle)
}

/// Strip `/* … */` block comments, replacing each with a single space so byte
/// adjacency that mattered (e.g. `@import/* x */"y"`) does not glue tokens.
///
/// Copies non-comment bytes verbatim into a byte buffer rather than
/// `out.push(byte as char)` — the latter reinterprets every UTF-8 continuation
/// byte (>=0x80) as a Latin-1 code point, corrupting non-ASCII CSS and shifting
/// the byte offsets the scanner relies on (a single non-ASCII byte near an
/// `@import` could then panic the resolver).
fn strip_block_comments(css: &str) -> String {
    let bytes = css.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            out.push(b' ');
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // Only ASCII comment delimiters were dropped; every other byte is copied
    // verbatim from a valid `&str`, so the result is still valid UTF-8.
    String::from_utf8(out).unwrap_or_else(|_| css.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn extracts_quoted_and_url_imports() {
        let css = r#"
            @import "tailwindcss";
            @import './tokens.css';
            @import url("theme.css");
            @import url(reset.css);
            @import "@scope/design-system";
            body { color: red; }
        "#;
        let specs = extract_import_specifiers(css);
        assert_eq!(
            specs,
            vec![
                "tailwindcss".to_string(),
                "./tokens.css".to_string(),
                "theme.css".to_string(),
                "reset.css".to_string(),
                "@scope/design-system".to_string(),
            ]
        );
    }

    #[test]
    fn non_ascii_css_does_not_panic_and_imports_still_extracted() {
        // A non-ASCII byte near an `@import` previously corrupted the byte
        // offsets (`byte as char`) and could panic the scanner mid-codepoint.
        let css = "/* コメント */\n@import \"./café.css\";\nbody::before{content:\"日本語\";}\n";
        let specs = extract_import_specifiers(css);
        assert_eq!(specs, vec!["./café.css".to_string()]);
    }

    #[test]
    fn skips_commented_out_imports() {
        let css = r#"
            /* @import "commented.css"; */
            @import "./real.css";
        "#;
        let specs = extract_import_specifiers(css);
        assert_eq!(specs, vec!["./real.css".to_string()]);
    }

    #[test]
    fn resolves_relative_imports_recursively_to_real_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("entry.css"),
            "@import \"tailwindcss\";\n@import './tokens.css';\n",
        )
        .unwrap();
        fs::write(root.join("tokens.css"), "@import './base.css';\n").unwrap();
        fs::write(root.join("base.css"), "body{}\n").unwrap();

        let resolved = resolve_css_imports(&root.join("entry.css"), root);
        let tokens_real = fs::canonicalize(root.join("tokens.css")).unwrap();
        let base_real = fs::canonicalize(root.join("base.css")).unwrap();
        assert!(resolved.contains(&tokens_real), "tokens.css resolved");
        assert!(
            resolved.contains(&base_real),
            "base.css resolved transitively"
        );
        // tailwindcss is virtual — never resolved, and the entry is excluded.
        assert_eq!(resolved.len(), 2);
    }

    #[test]
    fn resolves_workspace_package_through_symlink_to_real_path() {
        // Layout:
        //   proj/styles/styles.css  @import '@scope/design-system';
        //   real/design-system/{package.json(style: dist/tokens.css), dist/tokens.css}
        //   proj/node_modules/@scope/design-system -> ../../real/design-system (symlink)
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let proj = base.join("proj");
        let real_ds = base.join("real/design-system");
        fs::create_dir_all(proj.join("styles")).unwrap();
        fs::create_dir_all(proj.join("node_modules/@scope")).unwrap();
        fs::create_dir_all(real_ds.join("dist")).unwrap();
        fs::write(
            proj.join("styles/styles.css"),
            "@import \"tailwindcss\";\n@import '@scope/design-system';\n",
        )
        .unwrap();
        fs::write(
            real_ds.join("package.json"),
            r#"{"name":"@scope/design-system","style":"dist/tokens.css"}"#,
        )
        .unwrap();
        fs::write(real_ds.join("dist/tokens.css"), "body{}\n").unwrap();

        // Symlink the package into node_modules (the pnpm/workspace shape).
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_ds, proj.join("node_modules/@scope/design-system"))
            .unwrap();
        #[cfg(not(unix))]
        return; // symlink semantics differ; this test is unix-only

        let resolved = resolve_css_imports(&proj.join("styles/styles.css"), &proj);
        let tokens_real = fs::canonicalize(real_ds.join("dist/tokens.css")).unwrap();
        assert_eq!(
            resolved,
            vec![tokens_real],
            "workspace dep resolves through the node_modules symlink to its real path"
        );
    }
}
