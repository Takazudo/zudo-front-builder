//! Integration tests for `bundle.exclude` (#664, superseded by epic #667;
//! tracked as #672).
//!
//! ## What is being proven
//!
//! Once #665's `import.meta.glob('components/**/*.stories.tsx', { eager: true })`
//! transform lands (it is in this branch's base), an eager glob over a
//! `components/` tree expands to a STATIC import of every matched story. If a
//! story imports a CJS-only package whose `package.json` resolves only via
//! `main`/`module` (no `exports` map) — the literal shape of
//! `path-to-regexp@6`, the transitive dep that `msw` pulls in — esbuild,
//! invoked by the worker bundler with `--platform=neutral`, REJECTS it:
//!
//! ```text
//! Could not resolve "<pkg>" … The "main" field here was ignored. Main
//! fields must be configured explicitly when using the "neutral" platform.
//! ```
//!
//! (Empirically confirmed against esbuild 0.27.7 while authoring this test;
//! `--main-fields` is only set for `Framework::React`, so a Preact/neutral
//! bundle has an empty main-fields list and cannot resolve such a package.)
//!
//! `bundle.exclude` is the control that keeps the migration build green: it
//! drops the offending file from BOTH the shadow tree and the glob expansion.
//!
//! ## Faithfulness, not literalness
//!
//! A hermetic Rust test cannot `npm install msw`, so the fixture is a
//! HAND-ROLLED package mirroring the `--platform=neutral` CJS-rejection
//! *mechanism* (a `main` + `module` package.json with no `exports` map), the
//! way `bundler_workspace_pkg_alias.rs` hand-rolls its fixture packages. The
//! faithfulness is to the resolution failure, not to the literal `msw`.
//!
//! ## Negative control (the load-bearing assertion)
//!
//! `bundle_exclude_glob_composition_fails_without_exclude_passes_with` builds
//! the SAME tree twice: once WITHOUT `bundle.exclude` (must fail — proving the
//! bad import really is reached and rejected) and once WITH it (must pass —
//! proving the exclude is what fixes it). A test that only asserts the
//! with-exclude build is green would pass even with no implementation, so the
//! fail-without case is what gives this test its teeth.

use std::fs;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Write a hand-rolled CJS-only package into `<root>/node_modules/<name>`.
///
/// The `package.json` carries `main` (CJS) + `module` (an ESM-ish sibling)
/// and deliberately NO `exports` map — the literal `path-to-regexp@6` shape.
/// Under `--platform=neutral` esbuild's main-fields list is empty, so this
/// package fails to resolve, reproducing the `msw`→`path-to-regexp@6` worker
/// bundle failure faithfully without a real npm install.
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
    // CJS body — top-level module.exports, no ESM fallback.
    fs::write(
        pkg.join("dist/index.js"),
        "function http() { return \"http\"; }\nmodule.exports = { http: http };\n",
    )
    .unwrap();
    // The `module` field points here, but neutral's empty main-fields list
    // never consults it — esbuild errors before reading either entry.
    fs::write(
        pkg.join("dist.es2015/index.js"),
        "export function http() { return \"http\"; }\n",
    )
    .unwrap();
}

/// Shared `BundlerInput` for these fixtures: Preact + neutral worker bundle
/// (the failing combination), node_modules adjacent to the project so the
/// hand-rolled package is resolvable, runtime/preact bare specifiers marked
/// external so the synthetic `entry.mjs` itself bundles.
fn make_input(
    root: &std::path::Path,
    esbuild: std::path::PathBuf,
    bundle_exclude: Vec<String>,
) -> BundlerInput {
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
    input.esbuild_binary = Some(esbuild);
    input.node_modules_dir = Some(root.join("node_modules"));
    input.bundle_exclude = bundle_exclude;
    input
}

/// Create the standard source dirs and a bare page that does NOT import the
/// story (so the page's own import graph never reaches the bad package).
fn scaffold_project(root: &std::path::Path) {
    for d in ["pages", "components", "layouts", "content"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("pages/index.tsx"),
        r#"
            export default function Home() {
              return "home";
            }
        "#,
    )
    .unwrap();
}

/// Acceptance criterion 1: a `components/X.stories.tsx` importing the CJS-only
/// package, NOT imported by any page, listed in `bundle.exclude` → build
/// succeeds.
///
/// NOTE: a bare page never statically reaches the story, so esbuild's
/// single-entry tree-shaker would keep this green even without the fix. This
/// test guards the no-glob path and that an excluded file is absent from the
/// bundle; the *load-bearing* proof is the negative control below.
#[test]
fn bundle_exclude_drops_unreferenced_story_with_bad_cjs_dep() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exclude_glob] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    scaffold_project(root);
    write_cjs_only_package(root, "badcjs");
    fs::write(
        root.join("components/Button.stories.tsx"),
        r#"
            import { http } from "badcjs";
            export const Bad = () => http();
        "#,
    )
    .unwrap();

    let input = make_input(root, esbuild, vec!["components/*.stories.tsx".to_string()]);
    let out = bundle(input).expect(
        "build must succeed: the excluded story is never materialised, so its \
         CJS-only import cannot reach esbuild",
    );
    assert!(out.bundle_path.exists(), "bundle.mjs should exist");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        !body.contains("http"),
        "the excluded story's body must not appear in the bundle"
    );
}

/// Acceptance criterion 2 (CORE composition guard, with negative control):
/// an eager `import.meta.glob('./components/**/*.stories.tsx', { eager: true })`
/// makes every story a STATIC import. With the bad story present:
///
/// - WITHOUT `bundle.exclude` → esbuild reaches `badcjs` and the build FAILS
///   (proves the bad import is genuinely reached + rejected under neutral).
/// - WITH `bundle.exclude` → the glob expansion skips the bad story AND the
///   shadow copy is skipped, so the build is GREEN.
///
/// The fail-without half is the load-bearing assertion: it is the only thing
/// proving that `bundle.exclude` is what fixes the build, not tree-shaking.
#[test]
fn bundle_exclude_glob_composition_fails_without_exclude_passes_with() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_exclude_glob] no esbuild binary; skipping.");
        return;
    };

    // Build the identical tree in a fresh tempdir for each half so the
    // shadow materialisation cannot leak between runs.
    let build_tree = |dir: &std::path::Path| {
        scaffold_project(dir);
        write_cjs_only_package(dir, "badcjs");
        // A "good" story that the glob also matches — must always bundle.
        fs::write(
            dir.join("components/Good.stories.tsx"),
            "export const Good = () => \"ok\";\n",
        )
        .unwrap();
        // The "bad" story importing the CJS-only package.
        fs::write(
            dir.join("components/Bad.stories.tsx"),
            r#"
                import { http } from "badcjs";
                export const Bad = () => http();
            "#,
        )
        .unwrap();
        // A glob barrel INSIDE components/ that eagerly globs its sibling
        // stories. #665's expansion anchors the glob at the importer's own
        // directory and rejects `../`-rooted patterns, so the barrel must
        // live alongside the stories and glob `./*.stories.tsx`. Each match
        // becomes a STATIC import in the expanded source, so the bad story
        // IS reached through the import graph.
        fs::write(
            dir.join("components/_gallery.tsx"),
            r#"
                const stories = import.meta.glob('./*.stories.tsx', { eager: true });
                export const galleryKeys = Object.keys(stories);
            "#,
        )
        .unwrap();
        // A page that imports the barrel so the glob (and thus the bad story)
        // reaches esbuild from the single synthetic entry.mjs.
        fs::write(
            dir.join("pages/gallery.tsx"),
            r#"
                import { galleryKeys } from "../components/_gallery";
                export default function Gallery() {
                  return galleryKeys.join(",");
                }
            "#,
        )
        .unwrap();
    };

    // --- Negative control: NO bundle.exclude → must FAIL. ---
    let tmp_fail = tempfile::tempdir().expect("tempdir");
    build_tree(tmp_fail.path());
    let fail_input = make_input(tmp_fail.path(), esbuild.clone(), Vec::new());
    let fail_result = bundle(fail_input);
    assert!(
        fail_result.is_err(),
        "WITHOUT bundle.exclude the eager glob statically imports Bad.stories.tsx \
         → badcjs (CJS-only, no exports) → esbuild must reject it under \
         --platform=neutral. A green build here means the bad import is NOT \
         being reached and the negative control is broken."
    );
    let msg = format!("{:?}", fail_result.unwrap_err());
    assert!(
        msg.contains("esbuild") || msg.to_lowercase().contains("resolve") || msg.contains("badcjs"),
        "failure should originate from esbuild's resolution of the CJS-only \
         package; got: {msg}"
    );

    // --- With bundle.exclude → must PASS. ---
    let tmp_pass = tempfile::tempdir().expect("tempdir");
    build_tree(tmp_pass.path());
    let pass_input = make_input(
        tmp_pass.path(),
        esbuild,
        vec!["components/Bad.stories.tsx".to_string()],
    );
    let out = bundle(pass_input).expect(
        "WITH bundle.exclude the eager glob must skip Bad.stories.tsx so the \
         CJS-only import never reaches esbuild → build is green",
    );
    assert!(out.bundle_path.exists(), "bundle.mjs should exist");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    // The good story is still pulled in via the glob.
    assert!(
        body.contains("Good.stories.tsx") || body.contains("ok"),
        "the non-excluded Good story must still be expanded into the glob and bundled"
    );
    // The excluded story's specifier must not appear as an expanded import.
    assert!(
        !body.contains("Bad.stories.tsx"),
        "the excluded Bad story must not appear as an expanded glob import"
    );
}
