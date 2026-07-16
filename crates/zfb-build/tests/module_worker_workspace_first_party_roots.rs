//! Issue #1664 — the module-worker / island preprocessing contract treats the
//! pnpm workspace (nearest ancestor with `pnpm-workspace.yaml`) as the
//! first-party boundary, so a sub-package host may depend on sibling
//! workspace source. Files beyond the workspace, under `node_modules`, or
//! reached through a symlink escaping the workspace stay rejected.
//!
//! Modeled on `module_worker_plugin_virtual_absolute_deps.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{
    discover_module_preprocessing_with_context, rewrite_module_worker_urls_with_context,
    ModuleWorkerBuildContext,
};

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
