//! Node ESM bare-specifier resolver backed by `oxc_resolver`.
//!
//! Replaces the `createRequire(...).resolve(name)` / `import.meta.resolve`
//! logic in `js/config-loader.mjs:53-78` with a pure-Rust implementation that
//! honours conditional exports, scoped packages, and parent-directory walk —
//! the full Node module-resolution algorithm.
//!
//! # Why `oxc_resolver`
//!
//! Real `@takazudo/zfb-*` packages use **conditional-export objects**
//! (`"exports": { ".": { "types": ..., "import": ..., "default": ... } }`).
//! A narrow hand-roll would silently pick the wrong branch (or fail).
//! `oxc_resolver` (from the Oxc project, used by Rspack) handles the full
//! Node ESM exports algorithm correctly — issue #416 / epic #414.
//!
//! The helper lives in `crates/zfb-config-loader` (V8-free) and is consumed
//! by this crate's TS-config loader as well as `zfb::config`'s JSON-path
//! plugin resolver (via the crate's re-export). It is NOT part of
//! `crates/zfb-render`.

use std::path::{Path, PathBuf};

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

/// Resolve a package's root directory from `project_root`.
///
/// Resolves `pkg` (a bare package name, e.g. `"@takazudo/zfb-preset-example"`)
/// via `oxc_resolver` from `project_root`, then reads the package root off the
/// resolution:
///
/// - **Primary path:** uses `oxc_resolver::Resolution::package_json().directory()`
///   — robust against nested `package.json` files inside the package.
/// - **Fallback path:** if the resolution carries no `package_json()` (unusual;
///   can happen when resolving a bare file outside a package), walks up from the
///   resolved file to the nearest ancestor directory containing a `package.json`.
///
/// Hard-errors if the package is not installed (no `node_modules` entry found).
///
/// # Known edge (documented, not fixed here)
///
/// A preset whose `exports` map exposes neither `"."` nor `"./package.json"` —
/// for example only `"@scope/preset/zfb"` — cannot be resolved from its bare
/// name alone. In that case this function will still succeed for `"."` (it
/// resolves the default entry), but callers that need to resolve a non-root
/// subpath will receive a different package dir or an error. The correct fix
/// (T2/T4) is to anchor resolution at the project root and use a clearer error
/// message when the preset is not found.
pub fn resolve_package_dir(pkg: &str, project_root: &Path) -> Result<PathBuf> {
    if pkg.is_empty() {
        bail!("resolve_package_dir: package name must be non-empty");
    }

    let resolver = Resolver::new(ResolveOptions {
        condition_names: vec![
            "import".to_string(),
            "node".to_string(),
            "default".to_string(),
        ],
        ..ResolveOptions::default()
    });

    let resolution = resolver.resolve(project_root, pkg).map_err(|e| {
        anyhow!(
            "package {:?} could not be resolved from {}: {} \
             (is it installed in node_modules?)",
            pkg,
            project_root.display(),
            e
        )
    })?;

    // Primary path: use the package_json attached to the resolution.
    // oxc_resolver populates this for all normal node_modules resolutions.
    if let Some(pkg_json) = resolution.package_json() {
        return Ok(pkg_json.directory().to_path_buf());
    }

    // Fallback path: walk up from the resolved file to the nearest directory
    // containing a package.json. This handles the rare case where the
    // resolution carries no package_json (e.g. a bare file resolved outside
    // of any package boundary).
    let resolved_file = resolution.path();
    let mut dir = resolved_file.parent().ok_or_else(|| {
        anyhow!(
            "resolve_package_dir: resolved path {:?} has no parent directory",
            resolved_file
        )
    })?;
    loop {
        if dir.join("package.json").exists() {
            return Ok(dir.to_path_buf());
        }
        dir = dir.parent().ok_or_else(|| {
            anyhow!(
                "resolve_package_dir: no package.json found while walking up from {:?}",
                resolved_file
            )
        })?;
    }
}

/// Resolve a plugin `name` from two anchors: preferred first, then fallback.
///
/// Tries both path-relative/absolute resolution ([`crate::loader::resolve_plugin_path_to_file_url`])
/// and bare-specifier resolution ([`resolve_node_bare_specifier`]) at the
/// **preferred** anchor first, then the **fallback** anchor. Returns the first
/// successful `file://` URL.
///
/// This helper is anchor-agnostic — it does not assume which anchor is the
/// preset dir and which is the project root. T4 passes `(preset_dir,
/// project_root)` as `(preferred, fallback)` to implement the provenance
/// rule: a plugin that lives inside the preset's own `node_modules` takes
/// priority over one installed at the project level.
///
/// Returns a combined error naming both anchors when neither resolves.
pub fn resolve_plugin_from_anchors(
    name: &str,
    preferred: &Path,
    fallback: &Path,
) -> Result<String> {
    use crate::loader::resolve_plugin_path_to_file_url;

    // --- preferred anchor -------------------------------------------------------

    // Try relative/absolute resolution at preferred anchor.
    match resolve_plugin_path_to_file_url(name, preferred) {
        Ok(Some(url)) => return Ok(url),
        Ok(None) => {} // bare specifier — fall through to bare resolution
        Err(_) => {}   // file not found at preferred anchor — try fallback
    }

    // Try bare-specifier resolution at preferred anchor.
    if !name.starts_with("./")
        && !name.starts_with("../")
        && !name.starts_with('/')
        && !name.starts_with('#')
        && !name.is_empty()
    {
        if let Ok(url) = resolve_node_bare_specifier(name, preferred) {
            return Ok(url);
        }
    }

    // --- fallback anchor --------------------------------------------------------

    // Try relative/absolute resolution at fallback anchor.
    match resolve_plugin_path_to_file_url(name, fallback) {
        Ok(Some(url)) => return Ok(url),
        Ok(None) => {} // bare specifier — fall through to bare resolution
        Err(_) => {}   // file not found at fallback anchor either
    }

    // Try bare-specifier resolution at fallback anchor.
    if !name.starts_with("./")
        && !name.starts_with("../")
        && !name.starts_with('/')
        && !name.starts_with('#')
        && !name.is_empty()
    {
        if let Ok(url) = resolve_node_bare_specifier(name, fallback) {
            return Ok(url);
        }
    }

    // --- combined error ---------------------------------------------------------
    bail!(
        "plugin {:?} could not be resolved from preferred anchor {} \
         or fallback anchor {}",
        name,
        preferred.display(),
        fallback.display()
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Fixture helpers
    // -----------------------------------------------------------------------

    /// Root of the committed fixture sources:
    /// `crates/zfb/tests/fixtures/node_resolve/<case>/src/`
    fn fixture_src(case: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("node_resolve")
            .join(case)
            .join("src")
    }

    /// Recursively copy every file from `src` into `dst`, preserving the
    /// relative path structure.
    fn copy_dir_all(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).expect("create dst dir");
        for entry in std::fs::read_dir(src).expect("read src dir") {
            let entry = entry.expect("read dir entry");
            let ty = entry.file_type().expect("file type");
            if ty.is_dir() {
                copy_dir_all(&entry.path(), &dst.join(entry.file_name()));
            } else {
                std::fs::copy(entry.path(), dst.join(entry.file_name())).expect("copy file");
            }
        }
    }

    /// Build a temporary `node_modules/` tree for the named fixture case.
    ///
    /// Copies every top-level entry from `<case>/src/` into
    /// `<tmpdir>/node_modules/<entry>/` and returns the `TempDir` handle
    /// (which must stay alive for the duration of the test).
    /// The returned `TempDir` itself is the `project_root` to pass to
    /// `resolve_node_bare_specifier`.
    fn build_fixture(case: &str) -> TempDir {
        let src_root = fixture_src(case);
        let tmp = TempDir::new().expect("create TempDir");
        let node_modules = tmp.path().join("node_modules");

        for entry in std::fs::read_dir(&src_root)
            .unwrap_or_else(|e| panic!("read fixture src for {case}: {e}"))
        {
            let entry = entry.expect("read dir entry");
            let pkg_name = entry.file_name();
            let dst = node_modules.join(&pkg_name);
            copy_dir_all(&entry.path(), &dst);
        }

        tmp
    }

    /// Build the parent-walk fixture.
    ///
    /// Layout created in `<tmpdir>`:
    /// ```
    /// parent/
    ///   node_modules/pkg-parent/   ← package lives here
    ///   child/                     ← resolve FROM here (no node_modules)
    /// ```
    ///
    /// Returns `(TempDir, project_root)` where `project_root` is
    /// `<tmpdir>/parent/child`.  The caller must keep `TempDir` alive.
    fn build_parent_walk_fixture() -> (TempDir, PathBuf) {
        let src_root = fixture_src("parent-walk");
        let tmp = TempDir::new().expect("create TempDir");
        let node_modules = tmp.path().join("parent").join("node_modules");

        for entry in std::fs::read_dir(&src_root).expect("read parent-walk fixture src") {
            let entry = entry.expect("read dir entry");
            let dst = node_modules.join(entry.file_name());
            copy_dir_all(&entry.path(), &dst);
        }

        let child = tmp.path().join("parent").join("child");
        std::fs::create_dir_all(&child).expect("create child dir");

        (tmp, child)
    }

    /// Build the node-modules-nested fixture.
    ///
    /// Layout created in `<tmpdir>`:
    /// ```text
    /// node_modules/
    ///   @takazudo/
    ///     zfb-preset-example/          ← preset with ./package.json export
    ///       package.json
    ///       search.js                  ← relative plugin inside preset dir
    ///       dist/{index.mjs,.cjs,.d.ts}
    ///       node_modules/
    ///         zfb-plugin-nested/       ← bare plugin nested inside preset
    ///           package.json
    ///           dist/{index.mjs,.cjs,.d.ts}
    ///     zfb-preset-no-pkg-export/    ← preset WITHOUT ./package.json export
    ///       package.json
    ///       dist/{index.mjs,.cjs,.d.ts}
    /// ```
    ///
    /// Returns `(TempDir, project_root, preset_dir)` where:
    /// - `project_root` is the tmpdir root (has `node_modules/`)
    /// - `preset_dir` is the resolved root of `@takazudo/zfb-preset-example`
    ///   inside the temp tree
    ///
    /// The caller must keep `TempDir` alive.
    fn build_node_modules_nested_fixture() -> (TempDir, PathBuf, PathBuf) {
        let src_root = fixture_src("node-modules-nested");
        let tmp = TempDir::new().expect("create TempDir");
        let node_modules = tmp.path().join("node_modules");

        // Copy the entire src/ tree (preserving scoped dirs and nested
        // node_modules) into <tmpdir>/node_modules/
        copy_dir_all(&src_root, &node_modules);

        let preset_dir = node_modules
            .join("@takazudo")
            .join("zfb-preset-example");
        let project_root = tmp.path().to_path_buf();

        (tmp, project_root, preset_dir)
    }

    // -----------------------------------------------------------------------
    // New acceptance tests for resolve_package_dir + resolve_plugin_from_anchors
    // -----------------------------------------------------------------------

    // (i) resolve_package_dir — package whose exports expose "./package.json".
    //     Primary path: package_json().directory() should return the package root.
    #[test]
    fn i_resolve_package_dir_with_pkg_json_export() {
        let (_tmp, project_root, preset_dir) = build_node_modules_nested_fixture();
        let result = resolve_package_dir("@takazudo/zfb-preset-example", &project_root)
            .expect("should resolve preset package dir");
        // The returned dir should contain package.json and match the preset dir.
        assert!(
            result.join("package.json").exists(),
            "returned dir must contain package.json, got: {}",
            result.display()
        );
        assert_eq!(
            result.canonicalize().unwrap(),
            preset_dir.canonicalize().unwrap(),
            "returned dir should be the preset package root"
        );
    }

    // (i-b) resolve_package_dir — package WITHOUT a "./package.json" export.
    //       The primary path (package_json().directory()) still works because
    //       oxc_resolver always populates package_json for node_modules
    //       resolutions. This test confirms we get the correct package root
    //       regardless of whether the exports map includes "./package.json".
    #[test]
    fn i_b_resolve_package_dir_without_pkg_json_export() {
        let (_tmp, project_root, _preset_dir) = build_node_modules_nested_fixture();
        let result = resolve_package_dir("@takazudo/zfb-preset-no-pkg-export", &project_root)
            .expect("should resolve preset-no-pkg-export package dir");
        // The returned dir should contain package.json.
        assert!(
            result.join("package.json").exists(),
            "returned dir must contain package.json, got: {}",
            result.display()
        );
        let expected = project_root
            .join("node_modules")
            .join("@takazudo")
            .join("zfb-preset-no-pkg-export");
        assert_eq!(
            result.canonicalize().unwrap(),
            expected.canonicalize().unwrap(),
            "returned dir should be the preset-no-pkg-export package root"
        );
    }

    // (i-c) resolve_package_dir — missing package errors with a diagnostic message.
    #[test]
    fn i_c_resolve_package_dir_missing_package_errors() {
        let tmp = TempDir::new().expect("create TempDir");
        let err = resolve_package_dir("@takazudo/zfb-preset-does-not-exist", tmp.path())
            .expect_err("missing package should fail");
        let msg = err.to_string();
        assert!(
            msg.contains("zfb-preset-does-not-exist"),
            "error should name the missing package, got: {msg}"
        );
        assert!(
            msg.contains("installed in node_modules"),
            "error should mention node_modules, got: {msg}"
        );
    }

    // (ii) Two-anchor helper — bare plugin (`zfb-plugin-nested`) present ONLY
    //      inside the preset dir's node_modules resolves from the preferred
    //      (preset) anchor.
    #[test]
    fn ii_resolve_plugin_from_anchors_bare_from_preset_dir() {
        let (_tmp, project_root, preset_dir) = build_node_modules_nested_fixture();

        // zfb-plugin-nested is nested inside the preset's node_modules —
        // not installed at the project root.
        let result = resolve_plugin_from_anchors("zfb-plugin-nested", &preset_dir, &project_root)
            .expect("should resolve nested bare plugin from preset dir");
        assert!(
            result.starts_with("file://"),
            "expected file:// URL, got: {result}"
        );
        assert!(
            result.contains("zfb-plugin-nested"),
            "URL should reference zfb-plugin-nested, got: {result}"
        );
    }

    // (iii) Two-anchor helper — relative plugin (`./search.js`) present ONLY
    //       inside the preset dir resolves against the preset anchor, NOT the
    //       project root.
    //
    //       Acceptance criterion (d): `./search.js` exists only inside the
    //       preset dir — it is NOT found when the preferred anchor is the
    //       project root (the file isn't there).
    #[test]
    fn iii_relative_plugin_resolves_from_preset_dir_only() {
        let (_tmp, project_root, preset_dir) = build_node_modules_nested_fixture();

        // Preferred = preset dir: should find search.js inside the preset.
        let result =
            resolve_plugin_from_anchors("./search.js", &preset_dir, &project_root)
                .expect("should resolve ./search.js from preset dir");
        assert!(
            result.starts_with("file://"),
            "expected file:// URL, got: {result}"
        );
        assert!(
            result.ends_with("/search.js"),
            "URL should end with /search.js, got: {result}"
        );
        // The resolved file must live inside the preset dir, not the project root.
        assert!(
            result.contains("zfb-preset-example"),
            "URL must point into the preset dir, got: {result}"
        );

        // Preferred = project root (search.js NOT present there): should fail.
        let err =
            resolve_plugin_from_anchors("./search.js", &project_root, &project_root)
                .expect_err("./search.js absent from project root should fail");
        let msg = err.to_string();
        assert!(
            msg.contains("search.js") || msg.contains("could not be resolved"),
            "error should mention search.js or resolution failure, got: {msg}"
        );
    }

    // (iv) Two-anchor helper — plugin absent from BOTH anchors names both in error.
    #[test]
    fn iv_resolve_plugin_from_anchors_names_both_anchors_in_error() {
        let tmp = TempDir::new().expect("create TempDir");
        let preferred = tmp.path().join("preferred");
        let fallback = tmp.path().join("fallback");
        std::fs::create_dir_all(&preferred).unwrap();
        std::fs::create_dir_all(&fallback).unwrap();

        let err =
            resolve_plugin_from_anchors("@absent/zfb-plugin-ghost", &preferred, &fallback)
                .expect_err("absent plugin should fail");
        let msg = err.to_string();
        assert!(
            msg.contains("preferred") || msg.contains(preferred.to_str().unwrap()),
            "error should name preferred anchor, got: {msg}"
        );
        assert!(
            msg.contains("fallback") || msg.contains(fallback.to_str().unwrap()),
            "error should name fallback anchor, got: {msg}"
        );
    }

    // (v) Two-anchor helper — bare plugin present at fallback (project root)
    //     but NOT preferred (preset dir) resolves via fallback.
    #[test]
    fn v_resolve_plugin_from_anchors_falls_back_to_project_root() {
        let (_tmp, project_root, preset_dir) = build_node_modules_nested_fixture();

        // @takazudo/zfb-preset-example is installed at project root but not
        // inside zfb-preset-example's own node_modules — use it as the test
        // plugin for the fallback path.
        let result = resolve_plugin_from_anchors(
            "@takazudo/zfb-preset-no-pkg-export",
            &preset_dir,
            &project_root,
        )
        .expect("should fall back to project root for project-level package");
        assert!(
            result.starts_with("file://"),
            "expected file:// URL, got: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // Acceptance tests (a–h)
    // -----------------------------------------------------------------------

    // (a) `main` field present → resolves to the file named by `main`.
    #[test]
    fn a_main_field() {
        let tmp = build_fixture("main-field");
        let result =
            resolve_node_bare_specifier("pkg-main", tmp.path()).expect("should resolve pkg-main");
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
        let tmp = build_fixture("exports-string");
        let result = resolve_node_bare_specifier("pkg-exports-str", tmp.path())
            .expect("should resolve pkg-exports-str");
        assert!(
            result.starts_with("file://"),
            "expected file:// URL, got: {result}"
        );
        assert!(
            result.ends_with("/x.js"),
            "expected to resolve to x.js, got: {result}"
        );
    }

    // (c) `exports: { ".": "./x.js" }` object form → resolves.
    #[test]
    fn c_exports_object_form() {
        let tmp = build_fixture("exports-object");
        let result = resolve_node_bare_specifier("pkg-exports-obj", tmp.path())
            .expect("should resolve pkg-exports-obj");
        assert!(
            result.starts_with("file://"),
            "expected file:// URL, got: {result}"
        );
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
        let tmp = build_fixture("exports-conditional");
        let result =
            resolve_node_bare_specifier("pkg-cond", tmp.path()).expect("should resolve pkg-cond");
        assert!(
            result.starts_with("file://"),
            "expected file:// URL, got: {result}"
        );
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
        let tmp = build_fixture("scoped-package");
        let result = resolve_node_bare_specifier("@scope/pkg", tmp.path())
            .expect("should resolve @scope/pkg");
        assert!(
            result.starts_with("file://"),
            "expected file:// URL, got: {result}"
        );
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
    // Package lives in `<tmp>/parent/node_modules/` but resolution starts from
    // `<tmp>/parent/child/` (no node_modules there), so the resolver must walk up.
    #[test]
    fn f_walks_parent_directories() {
        let (_tmp, child) = build_parent_walk_fixture();
        let result = resolve_node_bare_specifier("pkg-parent", &child)
            .expect("should walk up and find pkg-parent");
        assert!(
            result.starts_with("file://"),
            "expected file:// URL, got: {result}"
        );
        assert!(
            result.ends_with("/index.js"),
            "expected to resolve to index.js, got: {result}"
        );
    }

    // (g) Missing package → error message names the package.
    #[test]
    fn g_missing_package_names_it_in_error() {
        // An empty TempDir has no node_modules — any package will fail to resolve.
        let tmp = TempDir::new().expect("create TempDir");
        let err = resolve_node_bare_specifier("pkg-does-not-exist", tmp.path())
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
        let tmp = build_fixture("real-plugin-shape");
        let result = resolve_node_bare_specifier("@takazudo/zfb-plugin-example", tmp.path())
            .expect("should resolve @takazudo/zfb-plugin-example");
        assert!(
            result.starts_with("file://"),
            "expected file:// URL, got: {result}"
        );
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

    // -----------------------------------------------------------------------
    // Input validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_empty_name() {
        let tmp = TempDir::new().expect("create TempDir");
        let err =
            resolve_node_bare_specifier("", tmp.path()).expect_err("empty name should be rejected");
        assert!(err.to_string().contains("non-empty"), "got: {err}");
    }

    #[test]
    fn rejects_relative_path() {
        let tmp = TempDir::new().expect("create TempDir");
        let err = resolve_node_bare_specifier("./local", tmp.path())
            .expect_err("relative path should be rejected");
        assert!(err.to_string().contains("relative"), "got: {err}");
    }

    #[test]
    fn rejects_absolute_path() {
        let tmp = TempDir::new().expect("create TempDir");
        let err = resolve_node_bare_specifier("/usr/local/lib/something", tmp.path())
            .expect_err("absolute path should be rejected");
        assert!(err.to_string().contains("absolute"), "got: {err}");
    }

    #[test]
    fn rejects_subpath_import_key() {
        let tmp = TempDir::new().expect("create TempDir");
        let err = resolve_node_bare_specifier("#internal", tmp.path())
            .expect_err("subpath import key should be rejected");
        assert!(err.to_string().contains("subpath import"), "got: {err}");
    }
}
