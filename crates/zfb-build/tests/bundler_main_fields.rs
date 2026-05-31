//! Integration test for `bundle.mainFields` / `bundle.external` (#676).
//!
//! ## What is being proven
//!
//! The page/SSR bundler runs esbuild with `--platform=neutral`, whose
//! main-fields list is EMPTY by default. A dependency resolved purely via
//! `package.json` `main`/`module` (no `exports` map) — the shape of
//! `path-to-regexp@6`, the transitive dep `msw` pulls in — is therefore
//! REJECTED:
//!
//! ```text
//! Could not resolve "<pkg>" … The "main" field here was ignored. Main
//! fields must be configured explicitly when using the "neutral" platform.
//! ```
//!
//! #676 adds two host knobs to fix this WITHOUT excluding the file:
//! - `bundle.mainFields` → `BundlerInput::main_fields` → esbuild
//!   `--main-fields=…`, so the CJS-main-only dep RESOLVES and is bundled.
//! - `bundle.external` → appended to `BundlerInput::external` → esbuild
//!   `--external:…`, so the dep is left unresolved (the other escape hatch).
//!
//! ## Negative control (load-bearing)
//!
//! Each test builds the SAME tree twice: once WITHOUT the knob (must FAIL —
//! proving the bad import really is reached and rejected under neutral) and
//! once WITH it (must PASS). A with-only assertion would pass even with no
//! implementation, so the fail-without half is what gives these tests teeth.
//!
//! ## Faithfulness, not literalness
//!
//! A hermetic Rust test cannot `npm install msw`, so the fixture hand-rolls a
//! `main` + `module` / no-`exports` package mirroring the `--platform=neutral`
//! CJS-rejection *mechanism* (the same approach as `bundler_exclude_glob.rs`
//! and `bundler_workspace_pkg_alias.rs`). The faithfulness is to the
//! resolution failure, not to the literal `msw`/`path-to-regexp`.

use std::fs;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Write a hand-rolled CJS-only package into `<root>/node_modules/<name>`.
///
/// The `package.json` carries `main` (CJS) + `module` (an ESM-ish sibling) and
/// deliberately NO `exports` map — the literal `path-to-regexp@6` shape. Under
/// `--platform=neutral` esbuild's main-fields list is empty, so this package
/// fails to resolve unless `--main-fields` is configured (or it is marked
/// external), reproducing the `msw` → `path-to-regexp@6` worker-bundle failure
/// without a real npm install.
fn write_cjs_only_package(root: &std::path::Path, name: &str) {
    let pkg = root.join("node_modules").join(name);
    fs::create_dir_all(pkg.join("dist")).unwrap();
    fs::create_dir_all(pkg.join("dist.es2015")).unwrap();
    fs::write(
        pkg.join("package.json"),
        format!(
            r#"{{ "name": "{name}", "version": "6.3.0", "main": "dist/index.js", "module": "dist.es2015/index.js" }}"#
        ),
    )
    .unwrap();
    // CJS body — top-level module.exports.
    fs::write(
        pkg.join("dist/index.js"),
        "function http() { return \"http-cjs\"; }\nmodule.exports = { http: http };\n",
    )
    .unwrap();
    // ESM sibling the `module` field points at.
    fs::write(
        pkg.join("dist.es2015/index.js"),
        "export function http() { return \"http-esm\"; }\n",
    )
    .unwrap();
}

/// Standard source dirs plus a page that STATICALLY imports `dep`, so the bad
/// import is reached directly from the synthetic entry (no glob needed).
fn scaffold_project_importing(root: &std::path::Path, dep: &str) {
    for d in ["pages", "components", "layouts", "content"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("pages/index.tsx"),
        format!(
            r#"
                import {{ http }} from "{dep}";
                export default function Home() {{ return http(); }}
            "#
        ),
    )
    .unwrap();
}

/// Preact + neutral worker bundle (the failing combination): node_modules
/// adjacent to the project so the hand-rolled package is resolvable, and
/// preact/runtime bare specifiers marked external so the synthetic `entry.mjs`
/// itself bundles. `main_fields` / extra `external` are the knobs under test.
fn make_input(
    root: &std::path::Path,
    esbuild: std::path::PathBuf,
    main_fields: Vec<String>,
    extra_external: Vec<String>,
) -> BundlerInput {
    main_fields: Vec::new(),
    let mut input = BundlerInput::for_project(
        root.to_path_buf(),
        Framework::Preact,
        BundleMode::Production,
        root.join("dist"),
        None,
    );
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input.external.extend(extra_external);
    input.main_fields = main_fields;
    input.esbuild_binary = Some(esbuild);
    input.node_modules_dir = Some(root.join("node_modules"));
    input
}

/// Core proof: a Preact/neutral bundle whose page imports a CJS-main-only dep
/// FAILS without `bundle.mainFields` and PASSES (with the dep actually
/// resolved + inlined) when `main_fields = ["main", "module"]`.
#[test]
fn main_fields_knob_resolves_cjs_main_only_dep_fails_without_passes_with() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_main_fields] no esbuild binary; skipping.");
        return;
    };

    // --- Negative control: NO main_fields → must FAIL. ---
    let tmp_fail = tempfile::tempdir().expect("tempdir");
    scaffold_project_importing(tmp_fail.path(), "badcjs");
    write_cjs_only_package(tmp_fail.path(), "badcjs");
    let fail = bundle(make_input(
        tmp_fail.path(),
        esbuild.clone(),
        Vec::new(),
        Vec::new(),
    ));
    assert!(
        fail.is_err(),
        "WITHOUT bundle.mainFields the Preact/neutral pass has an empty \
         main-fields list and must reject the CJS-main-only dep. A green build \
         here means the negative control is broken."
    );
    let msg = format!("{:?}", fail.unwrap_err());
    assert!(
        msg.contains("esbuild") || msg.to_lowercase().contains("resolve") || msg.contains("badcjs"),
        "failure should originate from esbuild's resolution of the CJS-only \
         package; got: {msg}"
    );

    // --- With main_fields = [main, module] → must PASS and bundle the dep. ---
    let tmp_pass = tempfile::tempdir().expect("tempdir");
    scaffold_project_importing(tmp_pass.path(), "badcjs");
    write_cjs_only_package(tmp_pass.path(), "badcjs");
    let out = bundle(make_input(
        tmp_pass.path(),
        esbuild,
        vec!["main".to_string(), "module".to_string()],
        Vec::new(),
    ))
    .expect("WITH bundle.mainFields the CJS-main-only dep must resolve → green build");
    assert!(out.bundle_path.exists(), "bundle.mjs should exist");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    // The dep was actually RESOLVED and inlined (not externalised): one of its
    // two entry bodies is present.
    assert!(
        body.contains("http-cjs") || body.contains("http-esm"),
        "the resolved CJS-main-only dep should be inlined into the bundle"
    );
}

/// The other #676 escape hatch: `bundle.external` lets the build go green by
/// leaving the CJS-main-only dep unresolved (a bare import in the bundle)
/// instead of resolving it.
#[test]
fn external_knob_lets_build_skip_cjs_main_only_dep() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_main_fields] no esbuild binary; skipping.");
        return;
    };

    // --- Negative control: NO external → must FAIL. ---
    let tmp_fail = tempfile::tempdir().expect("tempdir");
    scaffold_project_importing(tmp_fail.path(), "badcjs");
    write_cjs_only_package(tmp_fail.path(), "badcjs");
    let fail = bundle(make_input(
        tmp_fail.path(),
        esbuild.clone(),
        Vec::new(),
        Vec::new(),
    ));
    assert!(
        fail.is_err(),
        "negative control: must fail without any knob"
    );

    // --- With external = [badcjs] → must PASS, dep left unresolved. ---
    let tmp_pass = tempfile::tempdir().expect("tempdir");
    scaffold_project_importing(tmp_pass.path(), "badcjs");
    write_cjs_only_package(tmp_pass.path(), "badcjs");
    let out = bundle(make_input(
        tmp_pass.path(),
        esbuild,
        Vec::new(),
        vec!["badcjs".to_string()],
    ))
    .expect("WITH bundle.external the dep is left external → green build");
    assert!(out.bundle_path.exists(), "bundle.mjs should exist");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    // Externalised: the import is preserved as a bare specifier, not inlined.
    assert!(
        body.contains("badcjs"),
        "externalised dep should remain a bare import in the bundle"
    );
    assert!(
        !body.contains("http-cjs") && !body.contains("http-esm"),
        "externalised dep body must NOT be inlined into the bundle"
    );
}
