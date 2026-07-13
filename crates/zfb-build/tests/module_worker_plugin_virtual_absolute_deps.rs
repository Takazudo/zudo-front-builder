use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{rewrite_module_worker_urls_with_context, ModuleWorkerBuildContext};

fn write(path: &Path, source: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, source).unwrap();
}

fn source_with_worker() -> &'static str {
    "new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });"
}

fn write_worker_fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let importer = root.join("src/app.ts");
    let worker = root.join("src/worker.ts");
    let helper = root.join("src/virtual-helper.ts");
    write(&importer, "placeholder");
    write(
        &worker,
        "import { value } from 'virtual:worker-data'; self.postMessage(value);",
    );
    write(&helper, "export const value = 'helper';");
    (importer, worker, helper)
}

#[test]
fn virtual_module_absolute_project_import_matches_relative_worker_cache_key() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    let (importer, _, helper) = write_worker_fixture(root);

    let absolute_context = ModuleWorkerBuildContext::default().with_plugins(
        Vec::new(),
        vec![(
            "virtual:worker-data".into(),
            format!(
                "import {{ value }} from {}; export {{ value }};",
                serde_json::to_string(&helper.to_string_lossy()).unwrap()
            ),
        )],
    );
    let relative_context = ModuleWorkerBuildContext::default().with_plugins(
        Vec::new(),
        vec![(
            "virtual:worker-data".into(),
            "import { value } from './src/virtual-helper'; export { value };".into(),
        )],
    );

    let absolute = rewrite_module_worker_urls_with_context(
        source_with_worker(),
        &importer,
        root,
        &absolute_context,
    )
    .expect("absolute in-project import from plugin virtual module must resolve");
    let relative = rewrite_module_worker_urls_with_context(
        source_with_worker(),
        &importer,
        root,
        &relative_context,
    )
    .expect("relative in-project import from plugin virtual module must resolve");

    assert_eq!(
        absolute.expanded_source, relative.expanded_source,
        "absolute and relative virtual imports of the same project file must produce the same worker cache key"
    );
    assert!(absolute
        .dependencies
        .iter()
        .any(|dependency| dependency.dependency == helper));
}

#[test]
fn virtual_module_absolute_outside_project_import_keeps_contract_error() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().join("project");
    let outside = workspace.path().join("outside/shared.ts");
    let (importer, _, _) = write_worker_fixture(&root);
    write(&outside, "export const value = 'outside';");

    let context = ModuleWorkerBuildContext::default().with_plugins(
        Vec::new(),
        vec![(
            "virtual:worker-data".into(),
            format!(
                "import {{ value }} from {}; export {{ value }};",
                serde_json::to_string(&outside.to_string_lossy()).unwrap()
            ),
        )],
    );

    let error =
        rewrite_module_worker_urls_with_context(source_with_worker(), &importer, &root, &context)
            .expect_err("absolute imports outside the project root must still be rejected");
    let message = format!("{error:#}");
    assert!(
        message.contains("absolute module-worker dependency"),
        "{message}"
    );
    assert!(
        message.contains("outside the project graph contract"),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
fn virtual_module_absolute_symlink_escape_keeps_contract_error() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().join("project");
    let outside = workspace.path().join("outside/shared.ts");
    let escape = root.join("src/escape.ts");
    let (importer, _, _) = write_worker_fixture(&root);
    write(&outside, "export const value = 'outside';");
    symlink(&outside, &escape).unwrap();

    let context = ModuleWorkerBuildContext::default().with_plugins(
        Vec::new(),
        vec![(
            "virtual:worker-data".into(),
            format!(
                "import {{ value }} from {}; export {{ value }};",
                serde_json::to_string(&escape.to_string_lossy()).unwrap()
            ),
        )],
    );

    let error =
        rewrite_module_worker_urls_with_context(source_with_worker(), &importer, &root, &context)
            .expect_err("absolute imports that symlink outside the project root must be rejected");
    let message = format!("{error:#}");
    assert!(
        message.contains("absolute module-worker dependency"),
        "{message}"
    );
    assert!(
        message.contains("outside the project graph contract"),
        "{message}"
    );
}

#[test]
fn virtual_module_absolute_node_modules_import_is_not_project_source() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    let (importer, _, _) = write_worker_fixture(root);
    let dependency = root.join("node_modules/pkg/helper.ts");
    write(&dependency, "export const value = 'dependency';");

    let context = ModuleWorkerBuildContext::default().with_plugins(
        Vec::new(),
        vec![(
            "virtual:worker-data".into(),
            format!(
                "import {{ value }} from {}; export {{ value }};",
                serde_json::to_string(&dependency.to_string_lossy()).unwrap()
            ),
        )],
    );

    let rewrite =
        rewrite_module_worker_urls_with_context(source_with_worker(), &importer, root, &context)
            .expect("absolute node_modules imports should be left to package tooling, not treated as project files");

    assert!(
        !rewrite
            .dependencies
            .iter()
            .any(|dependency_edge| dependency_edge.dependency == dependency),
        "node_modules absolute imports must not be classified as project source dependencies"
    );
}
