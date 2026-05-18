//! Wave 2 regression tests for plugin alias / virtual-module
//! exact-match resolution in the main bundler (#269).
//!
//! The contract being pinned:
//!
//! 1. `addAlias("@/foo", "/abs/foo.tsx")` resolves `@/foo` exactly —
//!    `@/foo/bar` is NOT silently rewritten to `<target>/bar` the way
//!    esbuild's prefix-with-slash `--alias` flag would have done.
//! 2. `addVirtualModule("virtual:foo", ...)` behaves the same way —
//!    `virtual:foo/bar` is unresolved.
//! 3. A project that sets `BundlerInput::tsconfig_paths` AND uses
//!    plugin aliases has BOTH honored — plugin entries are merged on
//!    top, user-supplied entries win on key collision.
//!
//! All three tests need a real esbuild binary: the unit-test path
//! (`mock_subprocess_output`) bypasses `run_esbuild` entirely, so it
//! cannot exercise esbuild's path-mapping pipeline. Esbuild gating
//! follows the precedence used by sibling integration tests
//! (`bundler_strip_md_ext.rs`): `ZFB_ESBUILD_BIN` env var →
//! `crates/zfb/binaries/esbuild/esbuild` slot → `which esbuild` PATH
//! fallback. Skips with a printed note when no binary is present.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;

fn locate_esbuild() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZFB_ESBUILD_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace) = here.parent().and_then(|p| p.parent()) {
        let slot = workspace.join("crates/zfb/binaries/esbuild/esbuild");
        if slot.exists() {
            return Some(slot);
        }
    }
    if let Ok(out) = Command::new("which").arg("esbuild").output() {
        if out.status.success() {
            let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Lay down a minimal user-project tree: one TSX page that imports a
/// single specifier the caller picks. `import_specifier` is what the
/// page's `import` statement asks for; `import_target_path` is the
/// real file the alias resolves to. The two are decoupled so the
/// caller can register `@/foo` and then make the page import either
/// `@/foo` (exact match — should succeed) or `@/foo/bar` (prefix —
/// should fail).
fn write_fixture_project(
    root: &std::path::Path,
    import_specifier: &str,
    aliased_target_filename: &str,
) {
    for d in ["pages", "content", "components", "layouts", "src"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    )
    .unwrap();
    fs::write(
        root.join(format!("src/{aliased_target_filename}")),
        "export default function Foo() { return null; }\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        format!(
            "import Foo from {spec};\n\
             export default function Page() {{ return <Foo />; }}\n",
            spec = serde_json::to_string(import_specifier).unwrap()
        ),
    )
    .unwrap();
}

fn make_input(
    root: &std::path::Path,
    esbuild: &std::path::Path,
    outdir_name: &str,
    tsconfig_paths: BTreeMap<String, Vec<String>>,
    plugin_alias_entries: Vec<(String, String)>,
    plugin_virtual_modules: Vec<(String, String)>,
) -> BundlerInput {
    BundlerInput {
        project_root: root.to_path_buf(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: HashMap::new(),
        tsconfig_paths,
        external: vec![
            "preact".into(),
            "preact-render-to-string".into(),
            "@takazudo/zfb-runtime".into(),
        ],
        outdir: root.join(outdir_name),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: vec![],
        strip_md_ext: false,
        code_highlight_theme: None,
        code_highlight_themes_dir: None,
        resolve_markdown_links: None,
        gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
        site: None,
        prefetch_disabled: false,
        toc: None,
        external_links: None,
        cjk_friendly: true,
        plugin_alias_entries,
        plugin_virtual_modules,
        worker_only_routes: None,
        bundle_basename: None,
    }
}

/// **Exact-match regression test (alias).**
///
/// Register `@/foo` → `<root>/src/foo.tsx`. The page imports
/// `@/foo/bar` (NOT the registered specifier). esbuild must NOT
/// rewrite this to `<root>/src/foo.tsx/bar` the way `--alias` would
/// have — bundling must fail with an unresolved-import error
/// referencing the bare `@/foo/bar` string.
#[test]
fn plugin_alias_does_not_match_prefix_with_slash() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_exact_match_resolution] no esbuild binary available; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "@/foo/bar", "foo.tsx");

    let plugin_aliases = vec![(
        "@/foo".to_string(),
        root.join("src/foo.tsx").to_string_lossy().into_owned(),
    )];
    let input = make_input(
        &root,
        &esbuild,
        "dist-alias",
        BTreeMap::new(),
        plugin_aliases,
        vec![],
    );
    let err = bundle(input).expect_err(
        "Wave 2 contract: addAlias(\"@/foo\", ...) must NOT match `@/foo/bar`; \
         bundling must fail with an unresolved-import error.",
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("@/foo/bar"),
        "esbuild error should mention the unresolved specifier `@/foo/bar`. \
         Got: {msg}"
    );
}

/// **Exact-match regression test — happy path (alias).**
///
/// Same setup as above, but the page imports `@/foo` (the registered
/// specifier). Bundling must succeed and the bundled output must
/// reference the aliased file's exported identifier `Foo`.
#[test]
fn plugin_alias_matches_exact_specifier() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_exact_match_resolution] no esbuild binary available; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "@/foo", "foo.tsx");

    let plugin_aliases = vec![(
        "@/foo".to_string(),
        root.join("src/foo.tsx").to_string_lossy().into_owned(),
    )];
    let input = make_input(
        &root,
        &esbuild,
        "dist-alias-ok",
        BTreeMap::new(),
        plugin_aliases,
        vec![],
    );
    let out = bundle(input).expect(
        "exact-match alias `@/foo` must resolve when the import string is the \
         registered specifier",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    // The aliased target's exported `Foo` function survives into the
    // bundle (modulo esbuild's identifier rename — `function Foo` /
    // `Foo()` / `Foo.displayName` style references are all stable
    // because we don't minify).
    assert!(
        body.contains("Foo"),
        "exact-match alias bundle should contain the aliased component's \
         identifier `Foo`."
    );
}

/// **Virtual-module exact-match regression test.**
///
/// Register `virtual:foo` with a source string. The page imports
/// `virtual:foo/bar`. Bundling must fail; esbuild must NOT silently
/// rewrite this to `<temp-mjs>/bar`.
#[test]
fn plugin_virtual_module_does_not_match_prefix_with_slash() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_exact_match_resolution] no esbuild binary available; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    // The page imports `virtual:foo/bar` — wrong by one slash.
    write_fixture_project(&root, "virtual:foo/bar", "foo.tsx");

    let plugin_vms = vec![(
        "virtual:foo".to_string(),
        "export default function Foo() { return null; }\n".to_string(),
    )];
    let input = make_input(
        &root,
        &esbuild,
        "dist-vm",
        BTreeMap::new(),
        vec![],
        plugin_vms,
    );
    let err = bundle(input).expect_err(
        "Wave 2 contract: addVirtualModule(\"virtual:foo\", ...) must NOT match \
         `virtual:foo/bar`; bundling must fail with an unresolved-import error.",
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("virtual:foo/bar"),
        "esbuild error should mention the unresolved specifier `virtual:foo/bar`. \
         Got: {msg}"
    );
}

/// **Virtual-module exact-match — happy path.**
///
/// Same registration; the page imports `virtual:foo` (the registered
/// specifier). Bundling must succeed.
#[test]
fn plugin_virtual_module_matches_exact_specifier() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_exact_match_resolution] no esbuild binary available; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root, "virtual:foo", "foo.tsx");

    let plugin_vms = vec![(
        "virtual:foo".to_string(),
        "export default function Foo() { return null; }\n".to_string(),
    )];
    let input = make_input(
        &root,
        &esbuild,
        "dist-vm-ok",
        BTreeMap::new(),
        vec![],
        plugin_vms,
    );
    let out = bundle(input).expect(
        "exact-match virtual module `virtual:foo` must resolve when the import \
         string is the registered specifier",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("Foo"),
        "exact-match virtual-module bundle should contain the loader's \
         exported identifier `Foo`."
    );
}

/// **Pre-existing `tsconfig_paths` preservation test.**
///
/// A project that sets `BundlerInput::tsconfig_paths` AND uses plugin
/// aliases must have BOTH honored. The user explicitly maps
/// `@/components/*` in their tsconfig; the plugin separately registers
/// `@/data`. Both must resolve. The user's entry must not be erased
/// or shadowed by the plugin merge (collision policy: user wins).
#[test]
fn user_tsconfig_paths_coexist_with_plugin_aliases() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_exact_match_resolution] no esbuild binary available; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    // Build a project tree with TWO aliased targets — one resolved
    // via the user's tsconfig paths entry, one via a plugin alias.
    for d in ["pages", "content", "components", "layouts", "src", "src/components"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    )
    .unwrap();
    // User-tsconfig-mapped target: `@/components/*` → `src/components/*`.
    fs::write(
        root.join("src/components/widget.tsx"),
        "export default function Widget() { return null; }\n",
    )
    .unwrap();
    // Plugin-alias target: `@/data` → `<root>/src/data.ts` (absolute).
    fs::write(
        root.join("src/data.ts"),
        "export default { ok: true };\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        "import Widget from \"@/components/widget\";\n\
         import Data from \"@/data\";\n\
         export default function Page() {\n\
             const _ = [Widget, Data];\n\
             return <Widget />;\n\
         }\n",
    )
    .unwrap();

    let mut user_paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Wildcard entry — user's tsconfig style.
    user_paths.insert(
        "@/components/*".to_string(),
        vec!["src/components/*".to_string()],
    );

    let plugin_aliases = vec![(
        "@/data".to_string(),
        root.join("src/data.ts").to_string_lossy().into_owned(),
    )];

    let input = make_input(
        &root,
        &esbuild,
        "dist-coexist",
        user_paths,
        plugin_aliases,
        vec![],
    );
    let out = bundle(input).expect(
        "User-supplied tsconfig_paths AND plugin aliases must coexist — both \
         resolve in the same bundle.",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    // Both targets' identifiers (`Widget`, `Data`) survive into the
    // bundle — esbuild does not minify here.
    assert!(
        body.contains("Widget"),
        "user tsconfig_paths entry (`@/components/*`) must resolve and the \
         target's `Widget` symbol must reach the bundle."
    );
    assert!(
        body.contains("ok: true") || body.contains("ok:true"),
        "plugin alias (`@/data`) must resolve and its source content must \
         reach the bundle."
    );
}

/// **Collision policy test: user wins.**
///
/// User maps `@/x` → `src/user-x.tsx` via `tsconfig_paths`; a plugin
/// also tries to register `@/x` → `<root>/src/plugin-x.tsx`. The
/// merged tsconfig must keep the user's entry. Bundling resolves
/// `@/x` to the user's file, not the plugin's.
#[test]
fn user_tsconfig_paths_win_over_plugin_alias_on_collision() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_exact_match_resolution] no esbuild binary available; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    for d in ["pages", "content", "components", "layouts", "src"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    )
    .unwrap();
    // The two candidate targets carry distinct identifier names so
    // we can tell which one esbuild actually pulled into the bundle.
    fs::write(
        root.join("src/user-x.tsx"),
        "export default function UserVersionMarker() { return null; }\n",
    )
    .unwrap();
    fs::write(
        root.join("src/plugin-x.tsx"),
        "export default function PluginVersionMarker() { return null; }\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        "import X from \"@/x\";\n\
         export default function Page() { return <X />; }\n",
    )
    .unwrap();

    let mut user_paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
    user_paths.insert(
        "@/x".to_string(),
        // tsconfig paths values are joined against baseUrl (the shadow
        // root, which mirrors `<root>`), so the relative `src/user-x.tsx`
        // here resolves to the user's file under the project tree.
        vec!["src/user-x.tsx".to_string()],
    );
    let plugin_aliases = vec![(
        "@/x".to_string(),
        root.join("src/plugin-x.tsx").to_string_lossy().into_owned(),
    )];

    let input = make_input(
        &root,
        &esbuild,
        "dist-collide",
        user_paths,
        plugin_aliases,
        vec![],
    );
    let out = bundle(input).expect("bundling must succeed with collision");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("UserVersionMarker"),
        "collision policy: user-supplied tsconfig_paths entry must win — \
         the user's marker symbol `UserVersionMarker` must reach the bundle."
    );
    assert!(
        !body.contains("PluginVersionMarker"),
        "collision policy: plugin entry must NOT shadow the user's entry — \
         the plugin's marker symbol `PluginVersionMarker` should not reach \
         the bundle."
    );
}
