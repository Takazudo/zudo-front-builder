//! Sub #209 — Framework-packages no-pnpm integration test.
//!
//! Verifies that a consumer with NO `node_modules/` directory and NO `pnpm
//! install` can still bundle a page that imports `preact`,
//! `preact-render-to-string`, and `hono` — because the embedded extraction
//! produced by [`zfb::render_pipeline::embedded_node_modules`] supplies all
//! three.
//!
//! ## Why this lives in `crates/zfb/tests/`
//!
//! [`embedded_node_modules`] is a function on the `zfb` crate (it owns the
//! `include_dir!` snapshot of `$OUT_DIR/vendor`). The bundler test crates
//! (`crates/zfb-build/tests/`) do not depend on `zfb`, so they cannot reach
//! the embedded snapshot. Driving the test from here lets us exercise the
//! exact same code path that `zfb build` and `zfb dev` use at runtime when no
//! consumer-side `node_modules` is present.
//!
//! ## What the assertion proves
//!
//! 1. The build script (`crates/zfb/build.rs::embed_framework_packages`)
//!    successfully copied the three framework packages into the embedded
//!    vendor tree.
//! 2. The runtime extraction (`embedded_node_modules`) lays the packages out
//!    as proper `node_modules/<pkg>/package.json` siblings esbuild can
//!    resolve.
//! 3. esbuild's bundler can resolve and bundle imports of `preact`,
//!    `preact-render-to-string`, and `hono` against the extracted tree —
//!    nothing is marked external, and there is no consumer `node_modules/`
//!    on disk.
//!
//! ## Skipping
//!
//! The test gates on the same esbuild discovery path as the rest of the
//! bundler tests. When no esbuild binary is available it prints a skip note
//! and returns early.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use zfb::render_pipeline::embedded_node_modules;
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

#[test]
fn embedded_extraction_resolves_framework_imports_with_no_consumer_node_modules() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[framework_packages_no_pnpm] no esbuild binary available; \
             set ZFB_ESBUILD_BIN, place the binary at \
             crates/zfb/binaries/esbuild/esbuild, or install esbuild on PATH \
             to enable this test. Skipping."
        );
        return;
    };

    // Step 1 — synthesize a minimal consumer project on disk with NO
    // node_modules/ tree at all. The page imports `preact`,
    // `preact-render-to-string`, and `hono` directly so esbuild MUST resolve
    // them somewhere — and the only "somewhere" available is the embedded
    // extraction we wire in below.
    let project = tempfile::tempdir().expect("tempdir for project root");
    let root = project.path().to_path_buf();
    for d in ["pages", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function DefaultLayout({ children }) { return children; }\n",
    )
    .unwrap();

    fs::write(
        root.join("pages/index.tsx"),
        // Touch every package the embedded extraction provides so the test
        // fails if any one of them goes missing from the vendor tree.
        // - `preact` (top-level): the `h` JSX runtime entry.
        // - `preact-render-to-string` (top-level): the `renderToString` SSR
        //   entry — the page reaches into it directly so esbuild has to
        //   resolve `preact-render-to-string`'s `dist/` against the embedded
        //   extraction.
        // - `hono`: the consumer rarely imports hono directly, but pulling
        //   it in here forces esbuild to resolve it from the same extraction
        //   tree (it is a transitive dep of `@takazudo/zfb-runtime` in
        //   the real consumer flow; importing it directly removes any
        //   indirection from the test).
        r#"
            import { h } from "preact";
            import { renderToString } from "preact-render-to-string";
            import { Hono } from "hono";

            const app = new Hono();
            app.get("/", (c) => c.text("hello"));

            export default function Home() {
              const tree = h("div", null, "hello world");
              return renderToString(tree) + " / app=" + (typeof app);
            }
        "#,
    )
    .unwrap();

    // Step 2 — extract the embedded vendor tree into a fresh tempdir and
    // confirm every framework package's `package.json` is present at the
    // expected path. (The `embedded_node_modules` smoke test in the unit
    // tests covers this too; we re-assert here so a failure points the
    // operator at this file's exact import set rather than at a generic
    // unit-test layout assertion.)
    let (nm_handle, nm_path) = embedded_node_modules().expect("embedded_node_modules must succeed");
    for pkg in ["preact", "preact-render-to-string", "hono"] {
        let pkg_json = nm_path.join(pkg).join("package.json");
        assert!(
            pkg_json.exists(),
            "embedded extraction is missing {pkg}/package.json at {} — \
             check crates/zfb/build.rs::embed_framework_packages and the \
             *_VERSION constants",
            pkg_json.display()
        );
    }

    // Step 3 — run the bundler with `external: vec![]` (NOTHING marked
    // external — every framework import must resolve from the extraction)
    // and `node_modules_dir = Some(nm_path)` pointing at the extraction.
    let input = BundlerInput {
        project_root: root.clone(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: HashMap::new(),
        tsconfig_paths: BTreeMap::new(),
        external: vec![],
        outdir: root.join("dist"),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.clone()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: Some(nm_path.clone()),
        node_modules_preserve_symlinks: false,
        content_collections: Vec::new(),
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
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
    };

    let out = bundle(input).expect(
        "bundle must succeed against the embedded framework-packages extraction \
         with no consumer-side node_modules — \
         if it fails, check crates/zfb/build.rs::embed_framework_packages and \
         the embedded extraction in render_pipeline.rs",
    );

    // The bundle file must exist and must contain at least a hint of every
    // framework module's runtime code, proving that esbuild really did
    // pull each module in from the embedded extraction (rather than, say,
    // tree-shaking everything away). We check for a couple of distinctive
    // identifiers from each module instead of insisting on exact strings —
    // the published bundles minify-rename a lot of internals, but the
    // exported entry-point names survive.
    assert!(out.bundle_path.exists(), "bundle output must exist");
    let body = fs::read_to_string(&out.bundle_path).expect("bundle output must be readable utf-8");

    assert!(
        body.contains("renderToString") || body.contains("render_to_string"),
        "bundle should contain preact-render-to-string entry; \
         excerpt: {}",
        &body[..body.len().min(800)]
    );
    assert!(
        body.contains("Hono"),
        "bundle should contain hono's `Hono` class; \
         excerpt: {}",
        &body[..body.len().min(800)]
    );

    drop(nm_handle); // explicit: tempdir is removed here
}
