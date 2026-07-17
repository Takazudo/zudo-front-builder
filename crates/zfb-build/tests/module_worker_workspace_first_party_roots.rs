//! Issue #1664 — the module-worker / island preprocessing contract treats the
//! pnpm workspace (nearest ancestor with `pnpm-workspace.yaml`) as the
//! first-party boundary, so a sub-package host may depend on sibling
//! workspace source. Files beyond the workspace, under `node_modules`, or
//! reached through a symlink escaping the workspace stay rejected.
//!
//! Modeled on `module_worker_plugin_virtual_absolute_deps.rs`.
//!
//! The bottom section is a DIFFERENT, `bundle()`-level layer: issue #1668
//! part 2 STAGES every discovered workspace-sibling first-party file into the
//! re-rooted SSR shadow, so its `?raw`/glob/nested-worker rewrites land there.
//! These tests are the INVERSION of the former #1672 guard-pin tests (the
//! guard in `crates/zfb-build/src/bundler.rs` that rejected such siblings is
//! now deleted): they assert the sibling is staged + rewritten, obeys
//! `copy_mode`, layers node_modules by precedence, and still contains
//! symlink escapes.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{
    bundle, bundle_with_session, discover_module_preprocessing_with_context,
    rewrite_module_worker_urls_with_context, BundleMode, BundlerInput, ModuleWorkerBuildContext,
    ShadowSession,
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

/// Workspace-sibling worker ENTRIES are now supported (issue #1677 threaded
/// the `worker--ws-` naming contract added by #1673 into
/// `resolve_worker_target`'s naming call and deleted the former #1667 guard):
/// a worker source resolving to a sibling workspace package mints a scoped,
/// workspace-relative companion name instead of failing.
#[test]
fn workspace_sibling_worker_entry_succeeds_with_scoped_companion_name() {
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

    let rewrite = rewrite_module_worker_urls_with_context(
        source_with_worker(),
        &sibling_importer,
        &project,
        &context,
    )
    .expect("a sibling-package worker entry must now succeed (issue #1677)");

    let first_party_root = zfb_types::first_party_root_for(&project);
    let expected_filename =
        zfb_types::module_worker_filename_scoped(&project, &first_party_root, &sibling_worker)
            .expect("a workspace-sibling worker source has a scoped companion filename");
    assert!(
        expected_filename.starts_with("worker--ws-"),
        "expected the scoped `-ws-` prefix: {expected_filename}"
    );
    assert!(
        rewrite.expanded_source.contains(&expected_filename),
        "the rewritten URL must reference the scoped companion filename {expected_filename}: {}",
        rewrite.expanded_source
    );
    assert!(
        rewrite
            .worker_edges
            .iter()
            .any(|edge| edge.source_path == sibling_worker),
        "the sibling worker target must be tracked as a worker edge: {:?}",
        rewrite.worker_edges
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

// --- Issue #1668 part 2: bundle()-level SIBLING STAGING (the inverted #1672 guard) ---
//
// `discover_module_preprocessing_with_context` (exercised directly above)
// walks a `bundle()`-level exact-target staging graph into workspace-sibling
// source (issue #1664's widened first-party boundary). Issue #1668 part 1
// re-rooted the SSR shadow at the workspace boundary; part 2 (this change)
// STAGES every discovered workspace-sibling first-party file uniformly
// through the same materialise pass as a project file — at its
// workspace-relative slot in the WORK mirror — so its `?raw`/glob/nested-
// worker rewrites land in the shadow. The pre-#1668 guard (`bundler.rs`,
// which rejected such siblings) is DELETED.
//
// These tests are the INVERSION of the #1672 guard-pin tests: the 3 tests
// that used to assert the guard error now assert the sibling was staged +
// rewritten in the shadow copy, one per preprocessing form. They mirror
// `bundler_exact_match_resolution.rs`'s
// `project_plugin_alias_is_preprocessed_inside_the_ssr_shadow`, which proves
// the same 3 forms for a PROJECT-LOCAL plugin alias target under real
// esbuild; the L4 real-esbuild sibling equivalent runs centrally in the
// epic's confirm sub-issue (#1674's cross-pipeline lane).
//
// Staging is a pure Rust materialise pass — no esbuild subprocess runs to
// build the shadow tree. Every fixture here sets `mock_subprocess_output`
// (never a real `esbuild_binary`) and drives `bundle_with_session` so the
// persistent shadow survives the call for inspection, confirming these tests
// need NO esbuild env-gate.

/// Root of the WORK mirror (the persistent shadow tree) after a mock bundle.
/// On macOS the bundler canonicalises the shadow root (`/var` → `/private/var`),
/// so canonicalise here too before joining workspace-relative staged paths.
fn work_mirror_root(session: &ShadowSession) -> PathBuf {
    fs::canonicalize(session.shadow_root()).expect("canonicalize persistent shadow root")
}

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

/// **Preprocessing form 1/3 — sibling `?raw` importer (inverted #1672 guard).**
///
/// The sibling file itself imports another sibling file via `?raw`. The
/// widened #1664 graph walk discovers it; #1668 part 2 stages it into the WORK
/// mirror with the `?raw` rewrite applied. We inspect the staged copy: the
/// `?raw` specifier is gone and the payload was inlined into a generated
/// module beside it. (Mirrors the project-local
/// `project_plugin_alias_is_preprocessed_inside_the_ssr_shadow`, which proves
/// the same under real esbuild.)
#[test]
fn workspace_sibling_raw_importer_is_staged_and_rewritten() {
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

    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session))
        .expect("a workspace-sibling `?raw` importer must now stage into the SSR shadow");

    let shared_dir = work_mirror_root(&session).join("lib/shared");
    let staged = shared_dir.join("raw-importer.ts");
    let staged_body = fs::read_to_string(&staged)
        .unwrap_or_else(|e| panic!("sibling must be staged at {}: {e}", staged.display()));
    assert!(
        !staged_body.contains("?raw"),
        "the `?raw` specifier must be rewritten away in the staged copy: {staged_body}"
    );
    // The raw payload is inlined into a generated module written beside the
    // staged importer.
    let generated_has_payload = fs::read_dir(&shared_dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            fs::read_to_string(entry.path())
                .map(|body| body.contains("SIBLING_RAW_PAYLOAD"))
                .unwrap_or(false)
        });
    assert!(
        generated_has_payload,
        "the inlined `?raw` payload must appear in a generated module under {}",
        shared_dir.display()
    );
}

/// **Preprocessing form 2/3 — sibling `import.meta.glob` user (inverted #1672 guard).**
///
/// The sibling file contains a real `import.meta.glob(...)` call over two
/// sibling target files. #1668 part 2 stages the sibling into the WORK mirror
/// with the glob expanded to a static barrel; the staged copy no longer
/// contains `import.meta.glob` and references both discovered targets.
#[test]
fn workspace_sibling_import_meta_glob_is_staged_and_expanded() {
    let workspace = tempfile::tempdir().unwrap();
    let (sibling, input) = write_guard_fixture(workspace.path(), "glob-user");
    write(
        &sibling,
        "export const value = 'ok';\n\
         export const modules = import.meta.glob('./glob-user-data/*.ts', { eager: true });\n",
    );
    // Two real glob targets so the expansion is a non-empty barrel.
    write(
        &workspace.path().join("lib/shared/glob-user-data/a.ts"),
        "export const a = 1;\n",
    );
    write(
        &workspace.path().join("lib/shared/glob-user-data/b.ts"),
        "export const b = 2;\n",
    );

    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session))
        .expect("a workspace-sibling `import.meta.glob` user must now stage into the SSR shadow");

    let staged = work_mirror_root(&session).join("lib/shared/glob-user.ts");
    let staged_body = fs::read_to_string(&staged)
        .unwrap_or_else(|e| panic!("sibling must be staged at {}: {e}", staged.display()));
    assert!(
        !staged_body.contains("import.meta.glob"),
        "`import.meta.glob` must be expanded away in the staged copy: {staged_body}"
    );
    assert!(
        staged_body.contains("glob-user-data/a.ts") && staged_body.contains("glob-user-data/b.ts"),
        "the expanded barrel must reference both glob targets: {staged_body}"
    );
}

/// **Preprocessing form 3/3 — sibling nested `new Worker(...)` (inverted #1672 guard).**
///
/// The sibling file constructs a module Worker via a relative URL that
/// resolves BACK INTO the project root (`../../sub-packages/host/src/...`).
/// This isolates the sibling IMPORTER's own staging from the worker TARGET's
/// naming: the target here is project-local, so it keeps the unscoped
/// companion contract. See `workspace_sibling_worker_target_is_staged_with_scoped_companion_name`
/// below for the case where the worker TARGET is itself a workspace sibling
/// (issue #1677 — the former #1667 flat-naming-contract limitation that used
/// to reject that case is deleted). #1668 part 2 stages the sibling with its
/// `new Worker(...)` URL rewritten to the project-local companion filename +
/// content-hash query.
#[test]
fn workspace_sibling_nested_worker_construction_is_staged_and_rewritten() {
    let workspace = tempfile::tempdir().unwrap();
    let (sibling, input) = write_guard_fixture(workspace.path(), "worker-user");
    let project = input.project_root.clone();
    write(
        &project.join("src/nested-worker.ts"),
        "self.postMessage('nested-worker-target');\n",
    );
    write(
        &sibling,
        "export const value = 'ok';\n\
         new Worker(new URL('../../sub-packages/host/src/nested-worker.ts', import.meta.url), { type: 'module' });\n",
    );
    let worker_target = project.join("src/nested-worker.ts");
    let worker_filename = zfb_types::module_worker_filename(&project, &worker_target)
        .expect("a project-local worker target has a companion filename");

    let mut session = ShadowSession::new(&project).unwrap();
    bundle_with_session(input, Some(&mut session))
        .expect("a workspace-sibling nested `new Worker(...)` must now stage into the SSR shadow");

    let staged = work_mirror_root(&session).join("lib/shared/worker-user.ts");
    let staged_body = fs::read_to_string(&staged)
        .unwrap_or_else(|e| panic!("sibling must be staged at {}: {e}", staged.display()));
    assert!(
        staged_body.contains(&worker_filename),
        "the nested worker URL must be rewritten to its companion filename \
         {worker_filename}: {staged_body}"
    );
    assert!(
        staged_body.contains("?v="),
        "the rewritten worker URL must carry the content-hash query: {staged_body}"
    );
    assert!(
        !staged_body.contains("nested-worker.ts"),
        "the original worker source path must be gone from the rewritten URL: {staged_body}"
    );
}

/// **Issue #1677 — a worker TARGET that is itself a workspace sibling.**
///
/// The sibling importer's `new Worker(...)` URL resolves to ANOTHER workspace
/// sibling file (not back into the project). This is exactly the case the
/// former #1667 guard in `resolve_worker_target` used to reject; #1677
/// deletes that guard and threads the `worker--ws-` scoped naming contract
/// (issue #1673) through the SSR shadow staging pipeline instead.
#[test]
fn workspace_sibling_worker_target_is_staged_with_scoped_companion_name() {
    let workspace = tempfile::tempdir().unwrap();
    let (sibling, input) = write_guard_fixture(workspace.path(), "ws-worker-user");
    let project = input.project_root.clone();
    let worker_target = workspace.path().join("lib/shared/ws-worker-target.ts");
    write(&worker_target, "self.postMessage('ws-worker-target');\n");
    write(
        &sibling,
        "export const value = 'ok';\n\
         new Worker(new URL('./ws-worker-target.ts', import.meta.url), { type: 'module' });\n",
    );
    let first_party_root = zfb_types::first_party_root_for(&project);
    let worker_filename =
        zfb_types::module_worker_filename_scoped(&project, &first_party_root, &worker_target)
            .expect("a workspace-sibling worker target has a scoped companion filename");
    assert!(
        worker_filename.starts_with("worker--ws-"),
        "expected the scoped `-ws-` prefix: {worker_filename}"
    );

    let mut session = ShadowSession::new(&project).unwrap();
    bundle_with_session(input, Some(&mut session)).expect(
        "a sibling importer whose worker TARGET is also a workspace sibling must now stage \
         into the SSR shadow (issue #1677)",
    );

    let staged = work_mirror_root(&session).join("lib/shared/ws-worker-user.ts");
    let staged_body = fs::read_to_string(&staged)
        .unwrap_or_else(|e| panic!("sibling must be staged at {}: {e}", staged.display()));
    assert!(
        staged_body.contains(&worker_filename),
        "the nested worker URL must be rewritten to its scoped companion filename \
         {worker_filename}: {staged_body}"
    );
    assert!(
        staged_body.contains("?v="),
        "the rewritten worker URL must carry the content-hash query: {staged_body}"
    );
    assert!(
        !staged_body.contains("ws-worker-target.ts"),
        "the original worker source path must be gone from the rewritten URL: {staged_body}"
    );
}

/// **Uniform staging — a PLAIN sibling module is staged too (was the control).**
///
/// The #1668 part 2 DECISION: every discovered workspace-sibling first-party
/// file is staged, not just the preprocessing-hungry ones. A plain module (no
/// `?raw`/glob/worker needs) that formerly took the real-tree escape (the old
/// `continue`) is now materialised into the WORK mirror at its
/// workspace-relative slot, carrying its exact authored bytes. This replaces
/// the old control that asserted the real-tree escape.
#[test]
fn workspace_sibling_plain_module_is_staged_uniformly() {
    let workspace = tempfile::tempdir().unwrap();
    let (sibling, input) = write_guard_fixture(workspace.path(), "plain-sibling");
    write(&sibling, "export const value = 'PLAIN_SIBLING_VALUE';\n");

    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session))
        .expect("a plain workspace-sibling module must now stage uniformly into the SSR shadow");

    let staged = work_mirror_root(&session).join("lib/shared/plain-sibling.ts");
    let staged_body = fs::read_to_string(&staged)
        .unwrap_or_else(|e| panic!("plain sibling must be staged at {}: {e}", staged.display()));
    assert_eq!(
        staged_body, "export const value = 'PLAIN_SIBLING_VALUE';\n",
        "a plain sibling is staged with its exact authored bytes (no rewrite)"
    );
}

/// **Sibling `copy_mode` — a staged sibling is a REAL copy under branch 4.**
///
/// When esbuild will run WITHOUT `--preserve-symlinks` (branch 4:
/// `node_modules_dir` set + non-empty `tsconfig_paths`), the bundler sets
/// `copy_mode` so every in-shadow source is a real file — otherwise esbuild
/// canonicalises a symlinked shadow file back to the real tree and any sibling
/// in-shadow transform becomes invisible (the #443/#450 bug class). A staged
/// workspace sibling must obey `copy_mode` exactly like a project file.
#[test]
fn workspace_sibling_obeys_copy_mode() {
    let workspace = tempfile::tempdir().unwrap();
    let (sibling, mut input) = write_guard_fixture(workspace.path(), "copymode-sibling");
    write(&sibling, "export const value = 'COPY_MODE_SIBLING';\n");
    // Trigger branch 4 (copy_mode): a node_modules root + a non-empty tsconfig
    // paths map. The `@/*` wildcard is not a concrete staging target, so it
    // only flips the copy_mode predicate.
    fs::create_dir_all(workspace.path().join("node_modules")).unwrap();
    input.node_modules_dir = Some(workspace.path().join("node_modules"));
    input.tsconfig_paths = BTreeMap::from([(
        "@/*".to_string(),
        vec![input
            .project_root
            .join("src/*")
            .to_string_lossy()
            .into_owned()],
    )]);

    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("copy_mode sibling bundle must succeed");

    let staged = work_mirror_root(&session).join("lib/shared/copymode-sibling.ts");
    let meta = fs::symlink_metadata(&staged)
        .unwrap_or_else(|e| panic!("sibling must be staged at {}: {e}", staged.display()));
    assert!(
        !meta.file_type().is_symlink(),
        "under copy_mode a staged sibling must be a REAL copy, not a symlink"
    );
    assert_eq!(
        fs::read_to_string(&staged).unwrap(),
        "export const value = 'COPY_MODE_SIBLING';\n"
    );
}

/// **Sibling symlink-escape containment.**
///
/// A sibling that constructs a `new Worker` whose target file is a symlink
/// pointing OUTSIDE the workspace must be CONTAINED: the first-party boundary
/// check canonicalises the target and rejects the escape, so `bundle()` fails
/// rather than smuggling out-of-workspace content through the new
/// sibling-staging path. (The lower-level discovery guard is pinned by
/// `symlink_escape_beyond_the_workspace_stays_rejected` above; this pins it
/// through the full `bundle()`.)
#[cfg(unix)]
#[test]
fn workspace_sibling_symlink_escape_is_contained() {
    use std::os::unix::fs::symlink;

    let outer = tempfile::tempdir().unwrap();
    let workspace = outer.path().join("workspace");
    let (sibling, input) = write_guard_fixture(&workspace, "escape-worker-user");
    let outside_worker = outer.path().join("outside/escaped-worker.ts");
    write(&outside_worker, "self.postMessage('ESCAPED_WORKER');\n");
    let escape_link = workspace.join("lib/shared/escape-worker.ts");
    fs::create_dir_all(escape_link.parent().unwrap()).unwrap();
    symlink(&outside_worker, &escape_link).unwrap();
    write(
        &sibling,
        "export const value = 'ok';\n\
         new Worker(new URL('./escape-worker.ts', import.meta.url), { type: 'module' });\n",
    );

    let error = bundle(input).expect_err(
        "a sibling Worker target that symlinks outside the workspace must be contained",
    );
    let message = format!("{error:#}");
    assert!(
        !message.is_empty(),
        "containment must surface as a build error"
    );
    assert!(
        !message.contains("ESCAPED_WORKER"),
        "the escaped worker's contents must never reach the build: {message}"
    );
}

/// **Workspace-vs-project node_modules precedence — nearest install wins.**
///
/// #1668 part 1 gives the re-rooted shadow two install roots: the project's
/// own `<shadow>/node_modules` (nearest) and the workspace-hoisted
/// `<work>/node_modules` (outer). esbuild walks up from a project-mirror file
/// and reaches the project install first, so a package present at BOTH levels
/// resolves to the project copy. This pins the LAYOUT that guarantees it: the
/// two links point at the two distinct installs.
#[test]
fn workspace_and_project_node_modules_are_layered_by_precedence() {
    let workspace = tempfile::tempdir().unwrap();
    let ws_root = workspace.path();
    write(
        &ws_root.join("pnpm-workspace.yaml"),
        "packages:\n  - '.'\n  - 'sub-packages/*'\n",
    );
    let project = ws_root.join("sub-packages/host");
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
    // The SAME package name installed at BOTH levels, with distinct contents.
    write(
        &ws_root.join("node_modules/dup-pkg/marker.js"),
        "WORKSPACE_INSTALL",
    );
    write(
        &project.join("node_modules/dup-pkg/marker.js"),
        "PROJECT_INSTALL",
    );

    let mut input = make_bundle_input(&project, "dist-precedence", vec![]);
    input.node_modules_dir = Some(project.join("node_modules"));

    let mut session = ShadowSession::new(&project).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("precedence layout bundle must succeed");

    let work = work_mirror_root(&session);
    // The project mirror's OWN node_modules link → the PROJECT install (nearest).
    assert_eq!(
        fs::read_to_string(work.join("sub-packages/host/node_modules/dup-pkg/marker.js")).unwrap(),
        "PROJECT_INSTALL",
        "the project-mirror node_modules must resolve the nearest (project) install"
    );
    // The workspace-hoisted link → the WORKSPACE install (outer fallback).
    assert_eq!(
        fs::read_to_string(work.join("node_modules/dup-pkg/marker.js")).unwrap(),
        "WORKSPACE_INSTALL",
        "the workspace-hoisted node_modules must resolve the outer install"
    );
}
