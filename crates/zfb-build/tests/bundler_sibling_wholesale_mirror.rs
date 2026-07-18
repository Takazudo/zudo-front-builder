//! Issue #1692 — wholesale sibling mirror + project-root loose-file staging.
//!
//! The #1668 part 2 layer stages workspace-sibling first-party files
//! FILE-BY-FILE (only the preprocessing-discovery-graph members). That misses
//! everything esbuild's own resolver reaches that Rust discovery cannot predict
//! — `import.meta.glob` targets, plain wildcard-alias targets, sibling files
//! not on the import graph — plus the project-root loose-file variant that
//! broke a real site (`"@/*": ["./*"]` → repo-root JSON + non-empty
//! `bundle.exclude` → `Could not resolve`).
//!
//! This suite pins the fix: each claimed sibling region is WHOLESALE-mirrored
//! (raw tree copy, honoring `.gitignore` + the infra skip-list, root-level
//! files included) into its workspace-relative slot in the WORK mirror, and
//! project-root loose files reached via a wildcard root alias are staged under
//! exclusions (reserved generated names skipped). esbuild stays the sole
//! resolver — Rust only stages trees (see `l-lessons-client-bundling`).
//!
//! Every fixture drives `bundle_with_session` with `mock_subprocess_output`
//! (never a real esbuild binary): the staging is a pure Rust materialise pass,
//! so these are Level-3 staging tests that need NO esbuild env-gate. The
//! persistent `ShadowSession` survives the call so the staged tree can be
//! inspected.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{bundle_with_session, BundleMode, BundlerInput, ShadowSession};
use zfb_render::adapters::Framework;

fn write(path: &Path, source: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, source).unwrap();
}

/// Root of the WORK mirror (the persistent shadow tree) after a mock bundle.
/// On macOS the bundler canonicalises the shadow root (`/var` → `/private/var`),
/// so canonicalise here too before joining workspace-relative staged paths.
fn work_mirror_root(session: &ShadowSession) -> PathBuf {
    fs::canonicalize(session.shadow_root()).expect("canonicalize persistent shadow root")
}

/// A pnpm workspace whose sub-package project is `bundle()`'s `project_root`,
/// with the directory set `bundle()` requires. Returns `(workspace_root,
/// project_root)`.
fn write_bundle_workspace_project(workspace: &Path) -> (PathBuf, PathBuf) {
    write(
        &workspace.join("pnpm-workspace.yaml"),
        "packages:\n  - '.'\n  - 'sub-packages/*'\n  - 'packages/*'\n",
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

/// A standalone (non-workspace) project with the required directory set.
fn write_standalone_project(root: &Path) -> PathBuf {
    for dir in ["pages", "content", "components", "layouts", "src"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    write(
        &root.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    );
    write(
        &root.join("pages/index.tsx"),
        "export default function Page() { return null; }\n",
    );
    root.to_path_buf()
}

fn make_bundle_input(
    project: &Path,
    outdir_name: &str,
    plugin_alias_entries: Vec<(String, String)>,
    tsconfig_paths: BTreeMap<String, Vec<String>>,
    bundle_exclude: Vec<String>,
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
        tsconfig_paths,
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
        bundle_exclude,
        base_prefix: None,
    }
}

/// Write a project entry that imports `relative_import` and register a plugin
/// alias on it, so `bundle()`'s exact-target staging walk discovers the graph
/// transitively (mirrors the #1672-guard fixtures).
fn project_entry_importing(project: &Path, name: &str, relative_import: &str) -> (String, String) {
    let entry = project.join(format!("src/{name}-entry.ts"));
    write(
        &entry,
        &format!("import {{ value }} from '{relative_import}';\nexport const entry = value;\n"),
    );
    (
        format!("plugin:{name}-entry"),
        entry.to_string_lossy().into_owned(),
    )
}

// ---------------------------------------------------------------------------
// Wholesale sibling mirror
// ---------------------------------------------------------------------------

/// A package-layout sibling (`packages/ui/` WITH its own `package.json`) is
/// mirrored WHOLESALE: the discovered entry file, a NON-discovered sibling
/// file, the root-level `package.json`, and a nested file are all physically
/// present in the work mirror.
#[test]
fn package_layout_sibling_with_package_json_is_wholesale_mirrored() {
    let workspace = tempfile::tempdir().unwrap();
    let (ws_root, project) = write_bundle_workspace_project(workspace.path());
    let alias = project_entry_importing(
        &project,
        "ui",
        "../../../packages/ui/index", // discovered import into the sibling package
    );
    write(
        &ws_root.join("packages/ui/package.json"),
        "{\n  \"name\": \"ui\"\n}\n",
    );
    write(
        &ws_root.join("packages/ui/index.ts"),
        "export const value = 'UI_INDEX';\n",
    );
    // NOT imported anywhere — only wholesale mirroring reaches it.
    write(
        &ws_root.join("packages/ui/unreferenced.ts"),
        "export const extra = 'UI_UNREFERENCED';\n",
    );
    write(
        &ws_root.join("packages/ui/deep/nested/util.ts"),
        "export const util = 'UI_NESTED';\n",
    );

    let input = make_bundle_input(&project, "dist-ui", vec![alias], BTreeMap::new(), vec![]);
    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("package-layout sibling bundle succeeds");

    let ui = work_mirror_root(&session).join("packages/ui");
    assert!(ui.join("index.ts").is_file(), "discovered entry staged");
    assert!(
        ui.join("unreferenced.ts").is_file(),
        "a NON-discovered sibling file must be wholesale-mirrored"
    );
    assert!(
        ui.join("package.json").is_file(),
        "the mirror root's root-level package.json must be staged"
    );
    assert!(
        ui.join("deep/nested/util.ts").is_file(),
        "a nested sibling file must be wholesale-mirrored"
    );
}

/// A bare-directory sibling (`lib/shared/`, NO `package.json`) is mirrored
/// wholesale using the claim's own directory as the mirror root.
#[test]
fn bare_dir_sibling_without_package_json_is_wholesale_mirrored() {
    let workspace = tempfile::tempdir().unwrap();
    let (ws_root, project) = write_bundle_workspace_project(workspace.path());
    let alias = project_entry_importing(&project, "shared", "../../../lib/shared/contract");
    write(
        &ws_root.join("lib/shared/contract.ts"),
        "export const value = 'CONTRACT';\n",
    );
    write(
        &ws_root.join("lib/shared/unreferenced.ts"),
        "export const extra = 'SHARED_UNREFERENCED';\n",
    );

    let input = make_bundle_input(
        &project,
        "dist-shared",
        vec![alias],
        BTreeMap::new(),
        vec![],
    );
    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("bare-dir sibling bundle succeeds");

    let shared = work_mirror_root(&session).join("lib/shared");
    assert!(
        shared.join("contract.ts").is_file(),
        "discovered file staged"
    );
    assert!(
        shared.join("unreferenced.ts").is_file(),
        "a NON-discovered bare-dir sibling file must be wholesale-mirrored"
    );
}

/// Skip-list directories and `.gitignore`d files are NOT mirrored, and nothing
/// is ever mirrored through a `node_modules` component.
#[test]
fn skip_list_dirs_and_gitignored_files_are_not_mirrored() {
    let workspace = tempfile::tempdir().unwrap();
    let (ws_root, project) = write_bundle_workspace_project(workspace.path());
    let alias = project_entry_importing(&project, "skip", "../../../lib/shared/contract");
    let shared_src = ws_root.join("lib/shared");
    write(
        &shared_src.join("contract.ts"),
        "export const value = 'C';\n",
    );
    // Infra dirs (skip-list) — never mirrored.
    write(
        &shared_src.join("node_modules/pkg/vendored.ts"),
        "export const v = 1;\n",
    );
    write(&shared_src.join("dist/out.js"), "export const o = 1;\n");
    write(&shared_src.join("target/debug/artifact.txt"), "junk\n");
    // A .gitignore'd file — never mirrored (honored even without a .git dir).
    write(&shared_src.join(".gitignore"), "secret.ts\n");
    write(
        &shared_src.join("secret.ts"),
        "export const secret = 'LEAK';\n",
    );

    let input = make_bundle_input(&project, "dist-skip", vec![alias], BTreeMap::new(), vec![]);
    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("skip-list sibling bundle succeeds");

    let shared = work_mirror_root(&session).join("lib/shared");
    assert!(
        shared.join("contract.ts").is_file(),
        "discovered file staged"
    );
    assert!(
        !shared.join("node_modules").exists(),
        "a node_modules component must never be mirrored"
    );
    assert!(
        !shared.join("dist").exists(),
        "dist/ (skip-list) must not be mirrored"
    );
    assert!(
        !shared.join("target").exists(),
        "target/ (skip-list) must not be mirrored"
    );
    assert!(
        !shared.join("secret.ts").exists(),
        "a .gitignore'd file must not be mirrored"
    );
}

/// A wildcard-alias-only claim (`@shared/* -> <ws>/lib/shared/*`) with NO
/// discovered sibling file still mirrors its target directory wholesale.
#[test]
fn wildcard_alias_only_claim_mirrors_target_dir() {
    let workspace = tempfile::tempdir().unwrap();
    let (ws_root, project) = write_bundle_workspace_project(workspace.path());
    // NO project entry imports the sibling — the ONLY claim is the wildcard
    // alias target directory.
    write(
        &ws_root.join("lib/shared/helper.ts"),
        "export const help = 'HELP';\n",
    );
    let tsconfig_paths = BTreeMap::from([(
        "@shared/*".to_string(),
        vec![ws_root.join("lib/shared/*").to_string_lossy().into_owned()],
    )]);

    let input = make_bundle_input(&project, "dist-wildcard", vec![], tsconfig_paths, vec![]);
    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("wildcard-alias-only bundle succeeds");

    let shared = work_mirror_root(&session).join("lib/shared");
    assert!(
        shared.join("helper.ts").is_file(),
        "a wildcard-alias-only claim must still wholesale-mirror its target dir"
    );
}

// ---------------------------------------------------------------------------
// Project-root loose-file staging
// ---------------------------------------------------------------------------

/// A project-root loose JSON reached via `"@/*": ["./*"]` is staged into the
/// shadow root when `bundle.exclude` is active (shadow-only tsconfig, no
/// live-tree fallback), and a reserved generated name is NOT overwritten.
#[test]
fn project_root_loose_json_via_wildcard_alias_is_staged_under_exclusions() {
    let root = tempfile::tempdir().unwrap();
    let project = write_standalone_project(root.path());
    write(&project.join("data.json"), "{ \"loose\": true }\n");
    // A user file colliding with a reserved GENERATED name — must be skipped so
    // the generated file wins.
    write(&project.join("tsconfig.json"), "USER_TSCONFIG_SENTINEL\n");
    let tsconfig_paths = BTreeMap::from([(
        "@/*".to_string(),
        vec![project.join("*").to_string_lossy().into_owned()],
    )]);

    let input = make_bundle_input(
        &project,
        "dist-root",
        vec![],
        tsconfig_paths,
        vec!["**/*.secret".to_string()], // any non-empty exclude arms the shadow-only regime
    );
    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("root loose-file bundle succeeds");

    let shadow = work_mirror_root(&session);
    assert!(
        shadow.join("data.json").is_file(),
        "a project-root loose JSON reached via a wildcard root alias must be staged"
    );
    let staged_tsconfig = fs::read_to_string(shadow.join("tsconfig.json")).unwrap();
    assert!(
        !staged_tsconfig.contains("USER_TSCONFIG_SENTINEL"),
        "a reserved generated name (tsconfig.json) must not be overwritten by the user file"
    );
    assert!(
        staged_tsconfig.contains("compilerOptions"),
        "the generated synthetic tsconfig.json must win: {staged_tsconfig}"
    );
}

/// A root `?raw` importer whose payload is ANOTHER root `.ts` file must keep
/// the payload RAW even when the payload sorts/materialises before its
/// importer: the loose-file pass preflights every root source file before
/// materialising any, so the terminal raw-target edge is registered first.
/// (`a-payload.ts` sorts before `z-importer.ts`, so without the preflight pass
/// the payload would be materialised as source — its `import.meta.glob`
/// expanded away — before the importer's edge marked it a raw target.)
#[test]
fn root_loose_raw_payload_is_preflighted_before_materialize() {
    let root = tempfile::tempdir().unwrap();
    let project = write_standalone_project(root.path());
    write(
        &project.join("a-payload.ts"),
        "export const modules = import.meta.glob('./a-data/*.ts', { eager: true });\n\
         export const SENTINEL = 'RAW_PAYLOAD_BODY';\n",
    );
    write(&project.join("a-data/one.ts"), "export const one = 1;\n");
    write(
        &project.join("z-importer.ts"),
        "import payload from './a-payload.ts?raw';\nexport const value = payload;\n",
    );
    let tsconfig_paths = BTreeMap::from([(
        "@/*".to_string(),
        vec![project.join("*").to_string_lossy().into_owned()],
    )]);

    let input = make_bundle_input(
        &project,
        "dist-rawroot",
        vec![],
        tsconfig_paths,
        vec!["**/*.secret".to_string()],
    );
    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("root raw-payload bundle succeeds");

    let shadow = work_mirror_root(&session);
    let staged_payload = fs::read_to_string(shadow.join("a-payload.ts"))
        .expect("the raw payload must be staged at the shadow root");
    assert!(
        staged_payload.contains("import.meta.glob"),
        "the raw payload must be kept verbatim (not macro-expanded as source): {staged_payload}"
    );
    let staged_importer =
        fs::read_to_string(shadow.join("z-importer.ts")).expect("the root importer must be staged");
    assert!(
        !staged_importer.contains("?raw"),
        "the importer's `?raw` specifier must be rewritten away: {staged_importer}"
    );
}

/// Byte-identical: with an EMPTY `bundle.exclude` the dual-target tsconfig
/// still resolves `@/<name>` against the real project root, so the root
/// loose-file pass must NOT stage anything — preserving the pre-#1692 output.
#[test]
fn empty_exclude_does_not_stage_project_root_loose_files() {
    let root = tempfile::tempdir().unwrap();
    let project = write_standalone_project(root.path());
    write(&project.join("data.json"), "{ \"loose\": true }\n");
    let tsconfig_paths = BTreeMap::from([(
        "@/*".to_string(),
        vec![project.join("*").to_string_lossy().into_owned()],
    )]);

    let input = make_bundle_input(&project, "dist-empty", vec![], tsconfig_paths, vec![]);
    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("empty-exclude bundle succeeds");

    assert!(
        !work_mirror_root(&session).join("data.json").exists(),
        "empty-exclude builds must NOT stage project-root loose files (byte-identical)"
    );
}

/// Byte-identical: a standalone (non-workspace) build claims no sibling region,
/// so nothing outside the project mirror is staged even when a would-be sibling
/// directory sits next to it.
#[test]
fn non_workspace_build_stages_no_sibling_region() {
    let outer = tempfile::tempdir().unwrap();
    let project = write_standalone_project(&outer.path().join("project"));
    // A sibling-looking directory OUTSIDE the standalone project. Without a
    // pnpm workspace the first-party boundary stays the project root, so this
    // is never claimed.
    write(
        &outer.path().join("lib/shared/contract.ts"),
        "export const value = 'OUTSIDE';\n",
    );

    let input = make_bundle_input(&project, "dist-standalone", vec![], BTreeMap::new(), vec![]);
    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("standalone bundle succeeds");

    // Without a workspace, `work == shadow` (the project mirror is the root),
    // so a sibling slot like `lib/shared` never appears in the work mirror.
    assert!(
        !work_mirror_root(&session).join("lib/shared").exists(),
        "a non-workspace build must stage no sibling region (byte-identical)"
    );
}
