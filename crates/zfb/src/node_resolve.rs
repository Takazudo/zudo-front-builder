//! Node ESM bare-specifier resolver backed by `oxc_resolver`.
//!
//! Replaces the `createRequire(...).resolve(name)` / `import.meta.resolve`
//! logic in `crates/zfb/js/config-loader.mjs:53-78` with a pure-Rust
//! implementation that honours conditional exports, scoped packages, and
//! parent-directory walk — the full Node module-resolution algorithm.
//!
//! # Why `oxc_resolver`
//!
//! Real `@takazudo/zfb-*` packages use **conditional-export objects**
//! (`"exports": { ".": { "types": ..., "import": ..., "default": ... } }`).
//! A narrow hand-roll would silently pick the wrong branch (or fail).
//! `oxc_resolver` (from the Oxc project, used by Rspack) handles the full
//! Node ESM exports algorithm correctly — issue #416 / epic #414.
//!
//! The helper lives in `crates/zfb` (V8-free) and is consumed only by
//! `zfb::config`. It is NOT part of `crates/zfb-render`.

use std::path::Path;

use anyhow::{anyhow, bail, Result};
use oxc_resolver::{ResolveOptions, Resolver};
use url::Url;

/// Resolve a Node bare specifier from `project_root` to a `file://` URL.
///
/// The resolver is constructed with ESM-priority conditions
/// `["import", "node", "default"]` — the order Node uses for
/// `--input-type=module` — so conditional-export objects pick the `import`
/// branch first, matching the behaviour of `import.meta.resolve`.
///
/// # Errors
///
/// - Returns an error if `name` is empty.
/// - Returns an error if `name` looks like a relative path (`./`, `../`),
///   an absolute path (`/`, or an OS-absolute path per [`Path::is_absolute`]),
///   or a subpath-import key (`#`).  Callers are expected to pre-filter these
///   and handle them separately (see `config-loader.mjs:resolvePluginName`).
/// - Returns an error if the package cannot be found, with a message matching
///   the `config-loader.mjs` diagnostic shape so plugin authors see the same
///   text regardless of which evaluation path ran their config.
// Sub 3 (#417) wires this into `config.rs`; unused-until-then is expected.
#[allow(dead_code)]
pub fn resolve_node_bare_specifier(name: &str, project_root: &Path) -> Result<String> {
    // --- input validation -------------------------------------------------------

    if name.is_empty() {
        bail!(r#"plugin "name" must be a non-empty string (got "")"#);
    }

    // Reject relative paths — caller handles those separately.
    if name.starts_with("./") || name.starts_with("../") {
        bail!(
            "resolve_node_bare_specifier called with relative specifier {:?}; \
             caller must handle relative paths before invoking this helper",
            name
        );
    }

    // Reject absolute filesystem paths.
    if name.starts_with('/') || Path::new(name).is_absolute() {
        bail!(
            "resolve_node_bare_specifier called with absolute path {:?}; \
             caller must handle absolute paths before invoking this helper",
            name
        );
    }

    // Reject subpath-import keys (Node's `#imports` map — not a bare specifier).
    if name.starts_with('#') {
        bail!(
            "resolve_node_bare_specifier called with subpath import key {:?}; \
             caller must handle `#`-prefixed specifiers before invoking this helper",
            name
        );
    }

    // --- resolution -------------------------------------------------------------

    // ESM-priority condition order: `import` wins over `node`, which wins over
    // `default`.  This matches the Node `--input-type=module` resolution order
    // and is what `import.meta.resolve` uses, as opposed to `require.resolve`
    // which would use `["require", "node", "default"]`.
    let resolver = Resolver::new(ResolveOptions {
        condition_names: vec![
            "import".to_string(),
            "node".to_string(),
            "default".to_string(),
        ],
        ..ResolveOptions::default()
    });

    let resolution = resolver.resolve(project_root, name).map_err(|e| {
        anyhow!(
            "plugin {:?} could not be resolved as a Node bare specifier from {}: {}",
            name,
            project_root.display(),
            e
        )
    })?;

    // --- path → file:// URL conversion -----------------------------------------

    let abs_path = resolution.path();

    let file_url = Url::from_file_path(abs_path).map_err(|()| {
        anyhow!(
            "resolved path {:?} for plugin {:?} could not be converted to a file:// URL",
            abs_path,
            name
        )
    })?;

    Ok(file_url.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Returns the absolute path to a named sub-directory under
    /// `crates/zfb/tests/fixtures/node_resolve/`.
    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("node_resolve")
            .join(name)
    }

    // (a) `main` field present → resolves to the file named by `main`.
    #[test]
    fn a_main_field() {
        let project_root = fixture("main-field");
        let result = resolve_node_bare_specifier("pkg-main", &project_root)
            .expect("should resolve pkg-main");
        assert!(
            result.starts_with("file://"),
            "expected file:// URL, got: {result}"
        );
        assert!(
            result.ends_with("/index.js"),
            "expected to resolve to index.js, got: {result}"
        );
    }

    // (b) `exports: "./x.js"` string form → resolves.
    #[test]
    fn b_exports_string_form() {
        let project_root = fixture("exports-string");
        let result = resolve_node_bare_specifier("pkg-exports-str", &project_root)
            .expect("should resolve pkg-exports-str");
        assert!(result.starts_with("file://"), "expected file:// URL, got: {result}");
        assert!(
            result.ends_with("/x.js"),
            "expected to resolve to x.js, got: {result}"
        );
    }

    // (c) `exports: { ".": "./x.js" }` object form → resolves.
    #[test]
    fn c_exports_object_form() {
        let project_root = fixture("exports-object");
        let result = resolve_node_bare_specifier("pkg-exports-obj", &project_root)
            .expect("should resolve pkg-exports-obj");
        assert!(result.starts_with("file://"), "expected file:// URL, got: {result}");
        assert!(
            result.ends_with("/x.js"),
            "expected to resolve to x.js, got: {result}"
        );
    }

    // (d) Conditional-export object → resolves to the `import` branch.
    // This is the critical case that a hand-roll would have missed:
    // `"exports": { ".": { "import": "./dist/index.mjs", "default": "./dist/index.cjs" } }`
    // With ESM conditions ["import", "node", "default"], the resolver must
    // pick `index.mjs`, NOT `index.cjs`.
    #[test]
    fn d_conditional_exports_picks_import_branch() {
        let project_root = fixture("exports-conditional");
        let result = resolve_node_bare_specifier("pkg-cond", &project_root)
            .expect("should resolve pkg-cond");
        assert!(result.starts_with("file://"), "expected file:// URL, got: {result}");
        assert!(
            result.ends_with("/index.mjs"),
            "expected to resolve to index.mjs (import branch), got: {result}"
        );
        assert!(
            !result.ends_with("/index.cjs"),
            "must NOT resolve to index.cjs (default branch), got: {result}"
        );
    }

    // (e) Scoped package `@scope/pkg` resolves under `node_modules/@scope/pkg/`.
    #[test]
    fn e_scoped_package() {
        let project_root = fixture("scoped-package");
        let result = resolve_node_bare_specifier("@scope/pkg", &project_root)
            .expect("should resolve @scope/pkg");
        assert!(result.starts_with("file://"), "expected file:// URL, got: {result}");
        assert!(
            result.contains("@scope/pkg"),
            "expected path to contain @scope/pkg, got: {result}"
        );
        assert!(
            result.ends_with("/index.js"),
            "expected to resolve to index.js, got: {result}"
        );
    }

    // (f) Walks parent directories when `node_modules` is not in project root.
    // The fixture has `node_modules/pkg-parent/` in `parent/` but not in
    // `parent/child/`, so the resolver must walk up.
    #[test]
    fn f_walks_parent_directories() {
        // Resolve from the child directory — no node_modules there.
        let project_root = fixture("parent-walk").join("parent").join("child");
        let result = resolve_node_bare_specifier("pkg-parent", &project_root)
            .expect("should walk up and find pkg-parent");
        assert!(result.starts_with("file://"), "expected file:// URL, got: {result}");
        assert!(
            result.ends_with("/index.js"),
            "expected to resolve to index.js, got: {result}"
        );
    }

    // (g) Missing package → error message names the package.
    #[test]
    fn g_missing_package_names_it_in_error() {
        let project_root = fixture("missing-package");
        let err = resolve_node_bare_specifier("pkg-does-not-exist", &project_root)
            .expect_err("should fail for missing package");
        let msg = err.to_string();
        assert!(
            msg.contains("pkg-does-not-exist"),
            "error message should name the missing package, got: {msg}"
        );
        assert!(
            msg.contains("could not be resolved as a Node bare specifier"),
            "error message should match diagnostic shape, got: {msg}"
        );
    }

    // (h) Snapshot / real-plugin-shape test: mirrors a real `@takazudo/zfb-*`
    //     plugin's package.json (conditional exports with types/import/default).
    //     Must resolve to the `import` branch (`dist/index.mjs`).
    #[test]
    fn h_real_plugin_shape_snapshot() {
        let project_root = fixture("real-plugin-shape");
        let result =
            resolve_node_bare_specifier("@takazudo/zfb-plugin-example", &project_root)
                .expect("should resolve @takazudo/zfb-plugin-example");
        assert!(result.starts_with("file://"), "expected file:// URL, got: {result}");
        // Must resolve to the `import` branch, not `default` (CJS) or `types` (d.ts).
        assert!(
            result.ends_with("/dist/index.mjs"),
            "expected import branch (index.mjs), got: {result}"
        );
        assert!(
            !result.ends_with("/dist/index.cjs"),
            "must NOT pick the default (CJS) branch, got: {result}"
        );
        assert!(
            !result.ends_with("/dist/index.d.ts"),
            "must NOT pick the types branch, got: {result}"
        );
    }

    // --- input validation tests -------------------------------------------------

    #[test]
    fn rejects_empty_name() {
        let project_root = fixture("main-field");
        let err = resolve_node_bare_specifier("", &project_root)
            .expect_err("empty name should be rejected");
        assert!(err.to_string().contains("non-empty"), "got: {err}");
    }

    #[test]
    fn rejects_relative_path() {
        let project_root = fixture("main-field");
        let err = resolve_node_bare_specifier("./local", &project_root)
            .expect_err("relative path should be rejected");
        assert!(err.to_string().contains("relative"), "got: {err}");
    }

    #[test]
    fn rejects_absolute_path() {
        let project_root = fixture("main-field");
        let err = resolve_node_bare_specifier("/usr/local/lib/something", &project_root)
            .expect_err("absolute path should be rejected");
        assert!(err.to_string().contains("absolute"), "got: {err}");
    }

    #[test]
    fn rejects_subpath_import_key() {
        let project_root = fixture("main-field");
        let err = resolve_node_bare_specifier("#internal", &project_root)
            .expect_err("subpath import key should be rejected");
        assert!(err.to_string().contains("subpath import"), "got: {err}");
    }
}
