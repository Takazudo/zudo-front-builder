//! Issue #1664 — the module-worker / island preprocessing contract treats the
//! pnpm workspace (nearest ancestor with `pnpm-workspace.yaml`) as the
//! first-party boundary, so a sub-package host may depend on sibling
//! workspace source. Files beyond the workspace, under `node_modules`, or
//! reached through a symlink escaping the workspace stay rejected.
//!
//! Modeled on `module_worker_plugin_virtual_absolute_deps.rs`.
//!
//! The bottom section (issue #1672) is a DIFFERENT, `bundle()`-level layer:
//! it pins the issue #1668 guard in `crates/zfb-build/src/bundler.rs` (~3140)
//! that fires when a first-party workspace-sibling file itself needs
//! `?raw`/glob/module-worker preprocessing the SSR bundler cannot yet stage
//! outside the project root. That guard previously had zero test coverage;
//! the upcoming SSR re-root refactor must not be able to silently drop it.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{
    bundle, discover_module_preprocessing_with_context, rewrite_module_worker_urls_with_context,
    BundleMode, BundlerInput, ModuleWorkerBuildContext,
};
use zfb_render::adapters::Framework;

fn write(path: &Path, source: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, source).unwrap();
}

fn source_with_worker() -> &'static str {
    "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });"
}

/// A pnpm workspace with a sub-package project and a sibling shared library:
/// returns `(workspace_root, project_root, importer, shared)`.
fn write_workspace_fixture(workspace: &Path) -> (PathBuf, PathBuf, PathBuf) {
    write(
        &workspace.join("pnpm-workspace.yaml"),
        "packages:\n  - '.'\n  - 'sub-packages/*'\n",
    );
    let project = workspace.join("sub-packages/host");
    let importer = project.join("src/app.ts");
    let worker = project.join("src/worker.ts");
    let shared = workspace.join("lib/shared/contract.ts");
    write(&importer, "placeholder");
    write(
        &worker,
        "import { value } from 'virtual:worker-data'; self.postMessage(value);",
    );
    write(&shared, "export const value = 'workspace-sibling';");
    (project, importer, shared)
}

fn virtual_module_importing(path: &Path) -> ModuleWorkerBuildContext {
    ModuleWorkerBuildContext::default().with_plugins(
        Vec::new(),
        vec![(
            "virtual:worker-data".into(),
            format!(
                "import {{ value }} from {}; export {{ value }};",
                serde_json::to_string(&path.to_string_lossy()).unwrap()
            ),
        )],
    )
}

#[test]
fn workspace_sibling_dependency_is_accepted_and_tracked() {
    let workspace = tempfile::tempdir().unwrap();
    let (project, importer, shared) = write_workspace_fixture(workspace.path());

    let context = virtual_module_importing(&shared);
    let rewrite = rewrite_module_worker_urls_with_context(
        source_with_worker(),
        &importer,
        &project,
        &context,
    )
    .expect("a sibling-workspace dependency must pass the widened first-party contract");

    assert!(
        rewrite
            .dependencies
            .iter()
            .any(|dependency| dependency.dependency.ends_with("lib/shared/contract.ts")),
        "the workspace-sibling file must join the tracked dependency closure: {:?}",
        rewrite.dependencies
    );
}

#[test]
fn dependency_beyond_the_workspace_root_stays_rejected() {
    let outer = tempfile::tempdir().unwrap();
    let workspace = outer.path().join("workspace");
    let (project, importer, _) = write_workspace_fixture(&workspace);
    let beyond = outer.path().join("elsewhere/secret.ts");
    write(&beyond, "export const value = 'beyond';");

    let context = virtual_module_importing(&beyond);
    let error = rewrite_module_worker_urls_with_context(
        source_with_worker(),
        &importer,
        &project,
        &context,
    )
    .expect_err("a file beyond the workspace root must still be rejected");
    let message = format!("{error:#}");
    assert!(
        message.contains("outside the project graph contract"),
        "{message}"
    );
}

#[test]
fn node_modules_under_the_workspace_is_not_project_source() {
    let workspace = tempfile::tempdir().unwrap();
    let (project, importer, _) = write_workspace_fixture(workspace.path());
    let dependency = workspace.path().join("node_modules/pkg/helper.ts");
    write(&dependency, "export const value = 'dependency';");

    let context = virtual_module_importing(&dependency);
    let rewrite = rewrite_module_worker_urls_with_context(
        source_with_worker(),
        &importer,
        &project,
        &context,
    )
    .expect("workspace-root node_modules imports stay package territory");
    assert!(
        !rewrite
            .dependencies
            .iter()
            .any(|edge| edge.dependency == dependency),
        "node_modules imports must not be classified as first-party dependencies"
    );
}

#[cfg(unix)]
#[test]
fn symlink_escape_beyond_the_workspace_stays_rejected() {
    use std::os::unix::fs::symlink;

    let outer = tempfile::tempdir().unwrap();
    let workspace = outer.path().join("workspace");
    let (project, importer, _) = write_workspace_fixture(&workspace);
    let outside = outer.path().join("outside/shared.ts");
    let escape = workspace.join("lib/escape.ts");
    write(&outside, "export const value = 'outside';");
    fs::create_dir_all(escape.parent().unwrap()).unwrap();
    symlink(&outside, &escape).unwrap();

    let context = virtual_module_importing(&escape);
    let error = rewrite_module_worker_urls_with_context(
        source_with_worker(),
        &importer,
        &project,
        &context,
    )
    .expect_err("a symlink escaping the workspace must still be rejected");
    let message = format!("{error:#}");
    assert!(
        message.contains("outside the project graph contract"),
        "{message}"
    );
}

/// The issue #1664 island shape: a preprocessing entry whose alias-resolved
/// dependency lives in a sibling workspace package (here spelled relative,
/// which resolves identically once tsconfig aliases have been applied).
#[test]
fn preprocessing_graph_walks_into_sibling_workspace_source() {
    let workspace = tempfile::tempdir().unwrap();
    let (project, _, shared) = write_workspace_fixture(workspace.path());
    let entry = project.join("src/island.ts");
    write(
        &entry,
        "import { value } from '../../../lib/shared/contract';\nexport const island = value;\n",
    );
    // A registered plugin alias marks the context as plugin-resolving, which
    // is the production precondition for the discovery walk.
    let context = ModuleWorkerBuildContext::default().with_plugins(
        vec![(
            "@shared/contract".into(),
            shared.to_string_lossy().into_owned(),
        )],
        Vec::new(),
    );

    let discovery = discover_module_preprocessing_with_context(&entry, &project, &context)
        .expect("an island graph reaching sibling-workspace source must be discoverable");
    assert!(
        discovery
            .files
            .iter()
            .any(|file| file.ends_with("lib/shared/contract.ts")),
        "the sibling-workspace dependency must be part of the discovered graph: {:?}",
        discovery.files
    );
}

/// Workspace-sibling worker ENTRIES stay unsupported (the #1500 naming
/// contract is project-scoped) but must fail with the explicit limitation
/// error, not the naming contract's generic one. Tracked at issue 1667.
#[test]
fn workspace_sibling_worker_entry_fails_with_named_limitation() {
    let workspace = tempfile::tempdir().unwrap();
    let (project, _, _) = write_workspace_fixture(workspace.path());
    let sibling_importer = workspace.path().join("lib/widgets/panel.ts");
    let sibling_worker = workspace.path().join("lib/widgets/worker.ts");
    write(&sibling_importer, "placeholder");
    write(&sibling_worker, "self.postMessage('sibling');");

    let context = ModuleWorkerBuildContext::default().with_plugins(
        vec![(
            "@widgets/panel".into(),
            sibling_importer.to_string_lossy().into_owned(),
        )],
        Vec::new(),
    );

    let error = rewrite_module_worker_urls_with_context(
        source_with_worker(),
        &sibling_importer,
        &project,
        &context,
    )
    .expect_err("a sibling-package worker entry must fail until issue 1667 lands");
    let message = format!("{error:#}");
    assert!(
        message.contains("sibling workspace package") && message.contains("issues/1667"),
        "expected the named workspace-worker limitation, got: {message}"
    );
}

/// The worker cache key must not embed the checkout location: two identical
/// workspaces materialised at different paths (each importing a sibling file
/// through an absolute virtual-module import) must produce identical worker
/// URLs (`?v=` hashes).
#[test]
fn workspace_sibling_virtual_import_hash_is_checkout_location_independent() {
    let rewrite_for = |workspace: &Path| {
        let (project, importer, shared) = write_workspace_fixture(workspace);
        let context = virtual_module_importing(&shared);
        rewrite_module_worker_urls_with_context(source_with_worker(), &importer, &project, &context)
            .expect("sibling-workspace dependency must resolve")
            .expanded_source
    };

    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let first_workspace = first.path().join("checkout-a/deep/nested");
    let second_workspace = second.path().join("b");
    std::fs::create_dir_all(&first_workspace).unwrap();
    std::fs::create_dir_all(&second_workspace).unwrap();

    assert_eq!(
        rewrite_for(&first_workspace),
        rewrite_for(&second_workspace),
        "worker cache keys must be stable across relocated workspaces"
    );
}

/// Single-package guard: without a workspace marker the boundary stays the
/// project root and the pre-#1664 rejection is byte-for-byte preserved.
#[test]
fn without_workspace_marker_outside_dependency_stays_rejected() {
    let outer = tempfile::tempdir().unwrap();
    let project = outer.path().join("project");
    let importer = project.join("src/app.ts");
    let worker = project.join("src/worker.ts");
    let outside = outer.path().join("outside/shared.ts");
    write(&importer, "placeholder");
    write(
        &worker,
        "import { value } from 'virtual:worker-data'; self.postMessage(value);",
    );
    write(&outside, "export const value = 'outside';");

    let context = virtual_module_importing(&outside);
    let error = rewrite_module_worker_urls_with_context(
        source_with_worker(),
        &importer,
        &project,
        &context,
    )
    .expect_err("without a workspace marker the project root stays the boundary");
    let message = format!("{error:#}");
    assert!(
        message.contains("outside the project graph contract"),
        "{message}"
    );
}

// --- Issue #1672: bundle()-level regression tests for the #1668 guard ---
//
// `discover_module_preprocessing_with_context` (exercised directly above)
// happily walks a `bundle()`-level exact-target staging graph into
// workspace-sibling source (issue #1664's widened first-party boundary).
// But `bundler.rs`'s materialisation pass cannot yet STAGE `?raw`/glob/
// module-worker preprocessing for a file living outside the project root —
// so it must fail loudly (~bundler.rs:3140) instead of letting an
// unprocessed specifier reach esbuild. These tests pin that guard at the
// full `bundle()` level, one per trigger predicate, plus a control case
// proving an ordinary sibling module still bundles via the pre-#1664
// real-tree escape (the `continue` at ~bundler.rs:3148).
//
// **Coming inversion:** once the SSR re-root (the sibling sub-issues of
// #1668 in this epic) lands, sibling `?raw`/glob/module-worker files become
// stageable too, and the 3 guard-message tests below flip to success
// assertions (mirroring the plain-module test's `bundle(input).expect(...)`
// shape and `bundler_exact_match_resolution.rs`'s
// `project_plugin_alias_is_preprocessed_inside_the_ssr_shadow`, which proves
// the same 3 preprocessing forms for a PROJECT-LOCAL plugin alias target).
// Each guard test below carries its own inversion-intent comment.
//
// The guard fires purely from Rust-side graph discovery and a direct file
// read — no esbuild subprocess runs before it. Every fixture here sets
// `mock_subprocess_output` (never a real `esbuild_binary`), and the guard
// still fires identically, confirming these tests need NO esbuild env-gate.

/// A pnpm workspace whose sub-package project is `bundle()`'s
/// `project_root`, with the full directory set `bundle()` requires
/// (`pages`/`content`/`components`/`layouts`) — a superset of
/// `write_workspace_fixture`'s narrower shape above, which only serves the
/// lower-level `discover_module_preprocessing_with_context` /
/// `rewrite_module_worker_urls_with_context` entry points. Returns
/// `(workspace_root, project_root)`.
fn write_bundle_workspace_project(workspace: &Path) -> (PathBuf, PathBuf) {
    write(
        &workspace.join("pnpm-workspace.yaml"),
        "packages:\n  - '.'\n  - 'sub-packages/*'\n",
    );
    let project = workspace.join("sub-packages/host");
    for dir in ["pages", "content", "components", "layouts", "src"] {
        fs::create_dir_all(project.join(dir)).unwrap();
    }
    write(
        &project.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    );
    write(
        &project.join("pages/index.tsx"),
        "export default function Page() { return null; }\n",
    );
    (workspace.to_path_buf(), project)
}

/// Every field `BundlerInput` needs, held to the minimum this suite
/// exercises. `mock_subprocess_output` is always set — the guard under test
/// fires (or doesn't) before `run_esbuild` would ever be reached, so no
/// fixture in this section needs a real esbuild binary.
fn make_bundle_input(
    project: &Path,
    outdir_name: &str,
    plugin_alias_entries: Vec<(String, String)>,
) -> BundlerInput {
    BundlerInput {
        main_fields: Vec::new(),
        extra_loader_args: Vec::new(),
        project_root: project.to_path_buf(),
        pages_dir: PathBuf::from("pages"),
        injected_pages_root: None,
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: BTreeMap::new(),
        public_env_vars: HashMap::new(),
        tsconfig_paths: BTreeMap::new(),
        external: vec![
            "preact".into(),
            "preact-render-to-string".into(),
            "@takazudo/zfb-runtime".into(),
        ],
        outdir: project.join(outdir_name),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: None,
        mock_subprocess_output: Some("export default {};".to_string()),
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: vec![],
        pipeline_spec: zfb_content::PipelineSpec::default(),
        resolve_markdown_links: None,
        site: None,
        prefetch_disabled: false,
        plugin_alias_entries,
        plugin_virtual_modules: vec![],
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: HashMap::new(),
        mdx_components_file: None,
        bundle_exclude: Vec::new(),
        base_prefix: None,
    }
}

/// Shared fixture builder for every #1672 guard test: a pnpm workspace +
/// sub-package project, an entry file INSIDE the project
/// (`src/<name>-entry.ts`) that imports a sibling-workspace file
/// (`<workspace>/lib/shared/<name>.ts`) by relative path, and a plugin
/// alias registered on that entry so `bundle()`'s exact-target staging walk
/// discovers the sibling transitively — mirroring
/// `bundler_exact_match_resolution.rs`'s
/// `project_plugin_alias_is_preprocessed_inside_the_ssr_shadow` fixture
/// shape. The caller still has to write the sibling file's own contents
/// (that's the part that varies per guard predicate). Returns
/// `(sibling_path, BundlerInput)`.
fn write_guard_fixture(workspace: &Path, name: &str) -> (PathBuf, BundlerInput) {
    let (workspace_root, project) = write_bundle_workspace_project(workspace);
    let entry = project.join(format!("src/{name}-entry.ts"));
    write(
        &entry,
        &format!(
            "import {{ value }} from '../../../lib/shared/{name}';\n\
             export const entry = value;\n"
        ),
    );
    let sibling = workspace_root.join(format!("lib/shared/{name}.ts"));
    let input = make_bundle_input(
        &project,
        &format!("dist-{name}"),
        vec![(
            format!("plugin:{name}-entry"),
            entry.to_string_lossy().into_owned(),
        )],
    );
    (sibling, input)
}

/// **Guard trigger 1/3 — sibling `?raw` importer.**
///
/// The sibling file itself imports another sibling file via `?raw`. The
/// widened #1664 graph walk happily discovers it, but the SSR bundler
/// cannot stage a `?raw` rewrite outside the project root yet, so `bundle()`
/// must fail with the named #1668 limitation instead of letting the
/// unresolved `?raw` specifier reach esbuild.
///
/// INVERSION (post SSR re-root): flips to `bundle(input).expect(...)` + an
/// assertion that `SIBLING_RAW_PAYLOAD` reached the bundle, the same shape
/// `project_plugin_alias_is_preprocessed_inside_the_ssr_shadow` already uses
/// for a project-local `?raw` importer.
#[test]
fn workspace_sibling_raw_importer_fails_with_1668_guard() {
    let workspace = tempfile::tempdir().unwrap();
    let (sibling, input) = write_guard_fixture(workspace.path(), "raw-importer");
    write(
        &sibling,
        "import payload from './raw-importer-payload.txt?raw';\n\
         export const value = payload;\n",
    );
    write(
        &workspace.path().join("lib/shared/raw-importer-payload.txt"),
        "SIBLING_RAW_PAYLOAD",
    );

    let error = bundle(input).expect_err(
        "a workspace-sibling `?raw` importer must fail with the #1668 guard, not bundle unprocessed",
    );
    let message = format!("{error:#}");
    assert!(
        message.contains("needs `?raw`/glob/module-worker"),
        "{message}"
    );
    assert!(message.contains("issues/1668"), "{message}");
    assert!(message.contains("raw-importer.ts"), "{message}");
}

/// **Guard trigger 2/3 — sibling `import.meta.glob` user.**
///
/// The sibling file contains a real `import.meta.glob(...)` call. The guard
/// check re-reads the file directly and parses it for the call form — no
/// matching glob target files are needed for the guard itself to fire, so
/// this fixture deliberately leaves `./glob-user-data/*` unpopulated.
///
/// INVERSION (post SSR re-root): flips to a success assertion once the
/// bundler can stage the sibling's glob expansion outside the project root.
#[test]
fn workspace_sibling_import_meta_glob_fails_with_1668_guard() {
    let workspace = tempfile::tempdir().unwrap();
    let (sibling, input) = write_guard_fixture(workspace.path(), "glob-user");
    write(
        &sibling,
        "export const value = 'ok';\n\
         export const modules = import.meta.glob('./glob-user-data/*.ts');\n",
    );

    let error = bundle(input).expect_err(
        "a workspace-sibling `import.meta.glob` user must fail with the #1668 guard, not bundle unprocessed",
    );
    let message = format!("{error:#}");
    assert!(
        message.contains("needs `?raw`/glob/module-worker"),
        "{message}"
    );
    assert!(message.contains("issues/1668"), "{message}");
    assert!(message.contains("glob-user.ts"), "{message}");
}

/// **Guard trigger 3/3 — sibling nested `new Worker(...)` construction.**
///
/// The sibling file constructs a module Worker via a relative URL that
/// resolves BACK INTO the project root (`../../sub-packages/host/src/...`).
/// That relative-URL shape is deliberate: a worker TARGET that itself stays
/// a workspace sibling hits the unrelated, earlier #1667 flat-naming-contract
/// limitation inside `resolve_worker_target` before this #1668 guard is ever
/// reached (see `workspace_sibling_worker_entry_fails_with_named_limitation`
/// above) — #1667 guards the worker's own companion-file naming, #1668
/// guards staging the sibling IMPORTER's other preprocessing needs, and this
/// test isolates the latter.
///
/// INVERSION (post SSR re-root): flips to a success assertion + an
/// assertion that the worker companion filename reached the bundle, the
/// same shape `project_plugin_alias_is_preprocessed_inside_the_ssr_shadow`
/// already uses for a project-local nested worker.
#[test]
fn workspace_sibling_nested_worker_construction_fails_with_1668_guard() {
    let workspace = tempfile::tempdir().unwrap();
    let (sibling, input) = write_guard_fixture(workspace.path(), "worker-user");
    write(
        &input.project_root.join("src/nested-worker.ts"),
        "self.postMessage('nested-worker-target');\n",
    );
    write(
        &sibling,
        "export const value = 'ok';\n\
         new Worker(new URL('../../sub-packages/host/src/nested-worker.ts', import.meta.url), { type: 'module' });\n",
    );

    let error = bundle(input).expect_err(
        "a workspace-sibling nested `new Worker(...)` construction must fail with the #1668 guard, not bundle unprocessed",
    );
    let message = format!("{error:#}");
    assert!(
        message.contains("needs `?raw`/glob/module-worker"),
        "{message}"
    );
    assert!(message.contains("issues/1668"), "{message}");
    assert!(message.contains("worker-user.ts"), "{message}");
}

/// **Control — a PLAIN sibling module still bundles.**
///
/// No `?raw`/glob/module-worker preprocessing need anywhere in the sibling
/// file, so the pre-#1664 real-tree escape (the `continue` at
/// ~bundler.rs:3148) still applies: the widened first-party graph walk
/// discovers the sibling, the guard's 3 predicates all come back false, and
/// `bundle()` returns `Ok` instead of the #1668 guard's `Err` — proving the
/// guard itself does not misfire for a plain sibling. Note the scope: this
/// fixture uses `mock_subprocess_output`, which skips the esbuild subprocess
/// entirely, so it cannot confirm that the sibling's authored content
/// actually threads through real esbuild resolution — only that the Rust-side
/// guard check ahead of esbuild takes its `continue` branch rather than
/// erroring. Real-tree content threading is exercised elsewhere (e.g.
/// `workspace_sibling_dependency_is_accepted_and_tracked` /
/// `preprocessing_graph_walks_into_sibling_workspace_source` above) and by
/// the real-esbuild fixtures in `bundler_exact_match_resolution.rs`. This is
/// NOT expected to change shape after the SSR re-root (unlike the 3 guard
/// tests above); it stays a success assertion.
#[test]
fn workspace_sibling_plain_module_bundles_via_real_tree_escape() {
    let workspace = tempfile::tempdir().unwrap();
    let (sibling, input) = write_guard_fixture(workspace.path(), "plain-sibling");
    write(&sibling, "export const value = 'PLAIN_SIBLING_VALUE';\n");

    let output = bundle(input).expect(
        "a plain workspace-sibling module (no ?raw/glob/module-worker preprocessing needs) \
         must still bundle via the pre-#1664 real-tree escape",
    );
    assert!(
        output.bundle_path.exists(),
        "expected a bundle to be written to {}",
        output.bundle_path.display()
    );
}
