//! Level-3 build-output tests for the dev server's ADDITIVE injected-route
//! pages root (epic #1228, S2 #1230 — the B1 multi-root mechanism).
//!
//! `zfb dev` stages package-owned **injected** routes (synthesized by
//! `package_routes.rs`) into a session-lifetime temp dir holding ONLY those
//! modules — it does NOT copy the user's real `pages/`. That dir is threaded
//! into the bundler via [`BundlerInput::injected_pages_root`], which is
//! ADDITIVE: the bundler walks BOTH the real `pages_dir` and the injected
//! root into the same shadow `pages/` tree. These tests pin the two halves of
//! the S2 deliverable at the bundler level:
//!
//! 1. The injected entrypoint's module lands in the dev bundle ALONGSIDE the
//!    user pages (user pages are NOT dropped — HMR watch path intact).
//! 2. A `virtual:`-importing injected entrypoint resolves at bundle time (the
//!    injected module's `virtual:` specifier is provided via
//!    `plugin_virtual_modules`, exactly as the dev command threads it).
//!
//! These need a real esbuild binary (the mock-subprocess path bypasses
//! `run_esbuild` entirely, so it cannot exercise esbuild's resolution
//! pipeline). Esbuild gating mirrors the sibling
//! `bundler_exact_match_resolution.rs`: skip with a printed note when no
//! binary is present.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// A minimal user project: one TSX page under `pages/` carrying a unique
/// marker so we can prove the user page survives into the bundle even when an
/// additive injected root is present.
fn write_user_project(root: &Path) {
    for d in ["pages", "content", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        "export default function Home() { return <h1>USER_HOME_MARKER</h1>; }\n",
    )
    .unwrap();
}

/// Stage an injected-only pages root the way `resolve_dev_pages_root` does:
/// a temp dir whose `pages/` subdir holds ONLY the synthesized injected
/// modules (no user-page copy). Returns the TempDir (kept alive by the
/// caller) and the `pages` root the bundler should additively walk.
fn stage_injected_root(modules: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let pages = dir.path().join("pages");
    fs::create_dir_all(&pages).unwrap();
    for (rel, src) in modules {
        let dest = pages.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&dest, src).unwrap();
    }
    (dir, pages)
}

fn make_input(
    root: &Path,
    esbuild: &Path,
    outdir_name: &str,
    injected_pages_root: Option<PathBuf>,
    plugin_virtual_modules: Vec<(String, String)>,
) -> BundlerInput {
    BundlerInput {
        main_fields: Vec::new(),
        extra_loader_args: Vec::new(),
        project_root: root.to_path_buf(),
        authored_css_paths: Default::default(),
        pages_dir: PathBuf::from("pages"),
        injected_pages_root,
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: std::collections::BTreeMap::new(),
        public_env_vars: HashMap::new(),
        tsconfig_paths: BTreeMap::new(),
        external: vec![
            "preact".into(),
            "preact-render-to-string".into(),
            "@takazudo/zfb-runtime".into(),
        ],
        outdir: root.join(outdir_name),
        // Development mode — the dev bundle is what S2 targets.
        mode: BundleMode::Development,
        minify: false,
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: vec![],
        pipeline_spec: zfb_content::PipelineSpec::default(),
        resolve_markdown_links: None,
        site: None,
        prefetch_disabled: false,
        emit_render_artifacts: false,
        plugin_alias_entries: vec![],
        plugin_virtual_modules,
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: HashMap::new(),
        mdx_components_file: None,
        bundle_exclude: Vec::new(),
        base_prefix: None,
    }
}

/// **Core S2 deliverable.** An injected static route whose entrypoint imports
/// a `virtual:` module is bundled ADDITIVELY alongside the user's pages: the
/// bundle must contain the injected module's marker AND the resolved
/// `virtual:` module's marker AND the user page's marker (user pages are not
/// dropped by the additive root).
#[test]
fn injected_root_adds_module_and_resolves_virtual_without_dropping_user_pages() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_injected_pages_root] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_user_project(root);

    // A synthesized injected entrypoint (the shape `synthesize_static_overlay_module`
    // produces for a route whose package page imports a virtual module): it
    // re-exports a default component that reads from `virtual:preset-meta`.
    let injected_src = "\
import { presetTitle } from \"virtual:preset-meta\";\n\
export default function PresetAbout() {\n\
  return <h1>INJECTED_ABOUT_MARKER: {presetTitle}</h1>;\n\
}\n";
    let (_inj_guard, injected_root) = stage_injected_root(&[("preset-about.tsx", injected_src)]);

    let plugin_vms = vec![(
        "virtual:preset-meta".to_string(),
        "export const presetTitle = \"VIRTUAL_PRESET_META_MARKER\";\n".to_string(),
    )];

    let input = make_input(
        root,
        &esbuild,
        "dist-injected",
        Some(injected_root),
        plugin_vms,
    );
    let out = bundle(input).expect(
        "dev bundle with an additive injected_pages_root (static route importing a \
         virtual: module) must bundle successfully",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // (1) Injected entrypoint's module is in the dev bundle.
    assert!(
        body.contains("INJECTED_ABOUT_MARKER"),
        "the injected entrypoint's module must be present in the dev bundle"
    );
    // (2) The `virtual:` specifier resolved at bundle time — the loader source
    //     is inlined into the bundle.
    assert!(
        body.contains("VIRTUAL_PRESET_META_MARKER"),
        "the injected entrypoint's `virtual:` import must resolve at bundle time \
         (the virtual module source must be inlined into the bundle)"
    );
    // (3) The user's own page is NOT dropped by the additive root — HMR watch
    //     path intact (the user pages still reach the bundle).
    assert!(
        body.contains("USER_HOME_MARKER"),
        "the user's pages/ content must still reach the dev bundle when an \
         additive injected_pages_root is present"
    );
}

/// **Parity guard.** With `injected_pages_root = None` (no injected routes),
/// the bundle is exactly the user pages — the additive walk is skipped
/// entirely. A `None` root must never pull in anything from a staging dir and
/// must leave the user-only bundle unchanged in shape (still contains the user
/// page; contains no injected marker).
#[test]
fn no_injected_root_is_user_pages_only() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_injected_pages_root] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_user_project(root);

    let input = make_input(root, &esbuild, "dist-parity", None, vec![]);
    let out = bundle(input).expect("user-only dev bundle must succeed");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    assert!(
        body.contains("USER_HOME_MARKER"),
        "the user's page must be in the bundle on the parity (no-injected) path"
    );
    assert!(
        !body.contains("INJECTED_ABOUT_MARKER"),
        "no injected module may appear when injected_pages_root is None"
    );
}
