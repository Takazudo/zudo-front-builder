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
use zfb_test_utils::locate_esbuild;

#[path = "../src/embedded_node_modules_cache.rs"]
#[allow(dead_code)]
mod embedded_node_modules_cache;

static EMBEDDED_VENDOR: include_dir::Dir<'static> = include_dir::include_dir!("$OUT_DIR/vendor");

const WORKER_RESULT_ENV: &str = "ZFB_FRAMEWORK_CACHE_WORKER_RESULT";
const WORKER_PROJECT_PARENT_ENV: &str = "ZFB_FRAMEWORK_CACHE_WORKER_PROJECT_PARENT";

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
    let project = match std::env::var_os(WORKER_PROJECT_PARENT_ENV) {
        Some(parent) => tempfile::Builder::new()
            .prefix("project-")
            .tempdir_in(parent)
            .expect("tempdir for worker project root"),
        None => tempfile::tempdir().expect("tempdir for project root"),
    };
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
    let nm_lease = embedded_node_modules_cache::acquire_embedded_node_modules_if_enabled(
        &root,
        &EMBEDDED_VENDOR,
        embedded_node_modules,
    )
    .expect("embedded node_modules lease must succeed");
    let nm_path = nm_lease.node_modules().to_path_buf();
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
        main_fields: Vec::new(),
        extra_loader_args: Vec::new(),
        project_root: root.clone(),
        authored_css_paths: Default::default(),
        pages_dir: PathBuf::from("pages"),
        injected_pages_root: None,
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: std::collections::BTreeMap::new(),
        public_env_vars: HashMap::new(),
        tsconfig_paths: BTreeMap::new(),
        external: vec![],
        outdir: root.join("dist"),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.clone()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: Some(nm_path.clone()),
        // Vendored mode: the simulated project has NO node_modules — the
        // bundler injected the embedded @takazudo extraction. esbuild
        // must stay anchored at `<shadow>/node_modules/<pkg>` so it
        // finds the injected vendor tree (and not walk up to a
        // node_modules-less project root). Mirrors the production
        // wiring in `crates/zfb/src/commands/build.rs` for the
        // embedded fallback branch. See
        // `BundlerInput::node_modules_preserve_symlinks` for the full
        // rationale (issues #443 / #450 / #434).
        node_modules_preserve_symlinks: true,
        content_collections: Vec::new(),
        pipeline_spec: zfb_content::PipelineSpec::default(),
        resolve_markdown_links: None,
        site: None,
        prefetch_disabled: false,
        emit_render_artifacts: false,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: HashMap::new(),
        mdx_components_file: None,
        bundle_exclude: Vec::new(),
        base_prefix: None,
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

    if let Some(result_path) = std::env::var_os(WORKER_RESULT_ENV) {
        let lease_kind = match &nm_lease {
            embedded_node_modules_cache::EmbeddedNodeModulesLease::Owned { .. } => "owned",
            embedded_node_modules_cache::EmbeddedNodeModulesLease::Borrowed { .. } => "borrowed",
        };
        let result_path = PathBuf::from(result_path);
        fs::write(result_path.with_extension("bundle"), body.as_bytes())
            .expect("write worker bundle bytes");
        fs::write(
            result_path.with_extension("lease"),
            format!("{lease_kind}\n{}\n", nm_path.display()),
        )
        .expect("write worker lease evidence");
    }

    drop(nm_lease); // explicit: owned tempdir or borrowed read lock ends here
}

#[test]
fn cache_flag_preserves_build_bytes_and_default_stderr_across_processes() {
    if locate_esbuild().is_none() {
        eprintln!("[framework_packages_no_pnpm] no esbuild binary available; skipping");
        return;
    }
    let owner = tempfile::tempdir().expect("cache wiring test root");
    let cache_parent = owner.path().join("cache-parent");
    let project_parent = owner.path().join("projects");
    fs::create_dir_all(&cache_parent).unwrap();
    fs::create_dir_all(&project_parent).unwrap();

    let off = run_framework_worker(
        owner.path().join("off"),
        &cache_parent,
        &project_parent,
        None,
    );
    let cold = run_framework_worker(
        owner.path().join("cold"),
        &cache_parent,
        &project_parent,
        Some("1"),
    );
    let warm = run_framework_worker(
        owner.path().join("warm"),
        &cache_parent,
        &project_parent,
        Some("true"),
    );

    assert!(off.status.success(), "flag-off worker failed: {off:?}");
    assert!(cold.status.success(), "cold cache worker failed: {cold:?}");
    assert!(warm.status.success(), "warm cache worker failed: {warm:?}");
    assert_eq!(off.stderr, cold.stderr, "flag-on changed default stderr");
    assert_eq!(off.stderr, warm.stderr, "warm reuse changed default stderr");
    assert_eq!(
        fs::read(owner.path().join("off.bundle")).unwrap(),
        fs::read(owner.path().join("cold.bundle")).unwrap(),
        "flag-on changed emitted bundle bytes"
    );
    assert_eq!(
        fs::read(owner.path().join("off.bundle")).unwrap(),
        fs::read(owner.path().join("warm.bundle")).unwrap(),
        "warm cache reuse changed emitted bundle bytes"
    );

    let off_lease = fs::read_to_string(owner.path().join("off.lease")).unwrap();
    let cold_lease = fs::read_to_string(owner.path().join("cold.lease")).unwrap();
    let warm_lease = fs::read_to_string(owner.path().join("warm.lease")).unwrap();
    assert!(off_lease.starts_with("owned\n"), "{off_lease}");
    assert!(cold_lease.starts_with("borrowed\n"), "{cold_lease}");
    assert!(warm_lease.starts_with("borrowed\n"), "{warm_lease}");
    assert_eq!(
        cold_lease.lines().nth(1),
        warm_lease.lines().nth(1),
        "separate flag-on processes did not reuse the same published tree"
    );
}

fn run_framework_worker(
    result_path: PathBuf,
    cache_parent: &std::path::Path,
    project_parent: &std::path::Path,
    flag: Option<&str>,
) -> std::process::Output {
    let mut command = Command::new(std::env::current_exe().expect("current test executable"));
    command
        .arg("--exact")
        .arg("embedded_extraction_resolves_framework_imports_with_no_consumer_node_modules")
        .arg("--nocapture")
        .env(WORKER_RESULT_ENV, result_path)
        .env(WORKER_PROJECT_PARENT_ENV, project_parent)
        .env("TMPDIR", cache_parent)
        .env("XDG_CACHE_HOME", cache_parent)
        .env_remove(embedded_node_modules_cache::ZFB_EMBEDDED_NODE_MODULES_CACHE);
    if let Some(value) = flag {
        command.env(
            embedded_node_modules_cache::ZFB_EMBEDDED_NODE_MODULES_CACHE,
            value,
        );
    }
    command.output().expect("run framework cache worker")
}
