//! Issue #1840 — first-party dot-path staging allowlist.
//!
//! zudo-doc 4.x's `packageOwnedRoutes` mechanism generates REAL first-party
//! route source into `<project>/.zudo-doc/routes-src/*.tsx`, and docHistory
//! writes `<project>/.zfb/doc-history-meta.json`. Both are hidden AND
//! conventionally gitignored, so every generic staging walker skips them — no
//! staged spelling ever exists, esbuild resolves the LIVE files via the
//! dual-target tsconfig fallback, and the (correct) stage-escape audit
//! hard-fails with case 4. The fix stages the narrow allowlisted surface
//! (`.zudo-doc/routes-src/**` + top-level `.zfb/*.json`) so the staged
//! spelling exists and the audit's in-stage rule allows it naturally.
//!
//! Every fixture drives `bundle_with_session` with `mock_subprocess_output`
//! (never a real esbuild binary): the staging is a pure Rust materialise pass,
//! so these are Level-3 staging tests that need NO esbuild env-gate. The
//! stage-escape audit itself CANNOT run under mocks (no metafile), so the
//! real-esbuild audit-level proof belongs to a later sub-issue — these tests
//! pin the staged spellings on disk.

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

/// A standalone (non-workspace) project with the required directory set.
fn write_standalone_project(root: &Path) -> PathBuf {
    for dir in ["pages", "content", "components", "layouts"] {
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

/// The zudo-doc-shaped dot-path fixture: allowlisted generated route source +
/// meta JSON, plus neighbours that must NEVER reach the stage.
fn write_dot_path_fixture(project: &Path) {
    write(
        &project.join(".zudo-doc/routes-src/generated-route.tsx"),
        "export default function GeneratedRoute() { return null; }\n",
    );
    write(
        &project.join(".zudo-doc/routes-src/nested/helper.ts"),
        "export const helper = 'HELPER';\n",
    );
    // Outside the allowlisted subdir — a cache the narrow surface must skip.
    write(&project.join(".zudo-doc/cache/private.txt"), "private\n");
    write(
        &project.join(".zfb/doc-history-meta.json"),
        "{\n  \"history\": []\n}\n",
    );
    // Non-JSON sibling — never staged.
    write(&project.join(".zfb/cache.bin"), "binary\n");
    // An unrelated hidden dir — stays pruned like always.
    write(&project.join(".cache/tool.ts"), "export const t = 1;\n");
    // The allowlist is deliberately gitignore-blind: conventionally these
    // paths ARE gitignored, which is exactly why the generic walkers miss
    // them.
    write(&project.join(".gitignore"), ".zudo-doc/\n.zfb/\n.cache/\n");
}

fn make_bundle_input(project: &Path, outdir_name: &str) -> BundlerInput {
    BundlerInput {
        main_fields: Vec::new(),
        extra_loader_args: Vec::new(),
        project_root: project.to_path_buf(),
        authored_css_paths: Default::default(),
        pages_dir: PathBuf::from("pages"),
        injected_pages_root: None,
        injected_route_entrypoints: Vec::new(),
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
        emit_render_artifacts: false,
        plugin_alias_entries: vec![],
        plugin_virtual_modules: vec![],
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: HashMap::new(),
        mdx_components_file: None,
        bundle_exclude: vec![],
        base_prefix: None,
    }
}

/// Assert the allowlisted staged spellings exist under `project_mirror` and
/// the non-allowlisted neighbours do not.
fn assert_dot_path_staging(project_mirror: &Path) {
    assert!(
        project_mirror
            .join(".zudo-doc/routes-src/generated-route.tsx")
            .is_file(),
        "generated route source must gain a staged spelling"
    );
    assert!(
        project_mirror
            .join(".zudo-doc/routes-src/nested/helper.ts")
            .is_file(),
        "nested files under the allowlisted dir must be staged"
    );
    assert!(
        project_mirror.join(".zfb/doc-history-meta.json").is_file(),
        "top-level .zfb/*.json meta file must gain a staged spelling"
    );
    assert!(
        !project_mirror.join(".zudo-doc/cache").exists(),
        ".zudo-doc content OUTSIDE routes-src must never reach the stage"
    );
    assert!(
        !project_mirror.join(".zfb/cache.bin").exists(),
        "non-JSON .zfb files must never reach the stage"
    );
    assert!(
        !project_mirror.join(".cache").exists(),
        "unrelated hidden dirs must stay pruned"
    );
}

/// Standalone project: the allowlisted dot paths are staged even though they
/// are hidden AND gitignored; everything else hidden stays pruned.
#[test]
fn dot_allowlist_paths_are_staged_even_when_gitignored() {
    let tmp = tempfile::tempdir().unwrap();
    let project = write_standalone_project(tmp.path());
    write_dot_path_fixture(&project);

    let input = make_bundle_input(&project, "dist-dot");
    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("dot-path staging bundle succeeds");

    assert_dot_path_staging(&work_mirror_root(&session));
}

/// pnpm-workspace project: the staged spellings land at the project's
/// workspace-relative slot in the WORK mirror (the same shape the live tree
/// has, so esbuild's in-stage resolution matches).
#[test]
fn dot_allowlist_paths_stage_at_workspace_relative_slot() {
    let workspace = tempfile::tempdir().unwrap();
    write(
        &workspace.path().join("pnpm-workspace.yaml"),
        "packages:\n  - '.'\n  - 'sub-packages/*'\n",
    );
    let project = write_standalone_project(&workspace.path().join("sub-packages/host"));
    write_dot_path_fixture(&project);

    let input = make_bundle_input(&project, "dist-dot-ws");
    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("workspace dot-path bundle succeeds");

    assert_dot_path_staging(&work_mirror_root(&session).join("sub-packages/host"));
}

/// Issue #3004 fixture: an injected route's project-local entrypoint in a
/// hidden, NON-allowlisted dir, importing a relative helper, plus neighbours
/// that are never reached from the entrypoint.
fn write_injected_entrypoint_fixture(project: &Path) -> PathBuf {
    let entrypoint = project.join(".example-package/routes-src/route.tsx");
    write(
        &entrypoint,
        "import { helper } from './helper.tsx';\nexport default function Route() { return helper; }\n",
    );
    write(
        &project.join(".example-package/routes-src/helper.tsx"),
        "export const helper = 'HELPER';\n",
    );
    write(
        &project.join(".example-package/routes-src/unreferenced.tsx"),
        "export const unreferenced = 1;\n",
    );
    write(
        &project.join(".example-package/cache/private.txt"),
        "private\n",
    );
    entrypoint
}

fn bundle_with_entrypoints(
    project: &Path,
    outdir_name: &str,
    entrypoints: Vec<PathBuf>,
) -> ShadowSession {
    let input = BundlerInput {
        injected_route_entrypoints: entrypoints,
        ..make_bundle_input(project, outdir_name)
    };
    let mut session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut session)).expect("injected-entrypoint bundle succeeds");
    session
}

/// Every regular file under `root`, as sorted root-relative paths.
fn staged_file_listing(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                walk(&path, root, out);
            } else if file_type.is_file() {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// (a) + (b): the entrypoint and its relative-import closure gain their
/// project-relative staged spellings; unreached neighbours do not.
#[test]
fn injected_entrypoint_and_its_relative_closure_are_staged() {
    let tmp = tempfile::tempdir().unwrap();
    let project = write_standalone_project(tmp.path());
    write_dot_path_fixture(&project);
    let entrypoint = write_injected_entrypoint_fixture(&project);

    let session = bundle_with_entrypoints(&project, "dist-inj", vec![entrypoint]);
    let mirror = work_mirror_root(&session);

    assert!(mirror
        .join(".example-package/routes-src/route.tsx")
        .is_file());
    assert!(mirror
        .join(".example-package/routes-src/helper.tsx")
        .is_file());
    assert!(
        !mirror
            .join(".example-package/routes-src/unreferenced.tsx")
            .exists(),
        "a sibling the entrypoint never imports must not be staged"
    );
    assert!(!mirror.join(".example-package/cache").exists());
    assert_dot_path_staging(&mirror);
}

/// (c): an empty entrypoint list leaves the staged set exactly as a bundle
/// that never knew the field — the hidden dir stays unstaged.
#[test]
fn empty_injected_entrypoints_leave_staging_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let project = write_standalone_project(tmp.path());
    write_dot_path_fixture(&project);
    write_injected_entrypoint_fixture(&project);

    let session = bundle_with_entrypoints(&project, "dist-inj-empty", Vec::new());
    let mirror = work_mirror_root(&session);

    assert!(!mirror.join(".example-package").exists());
    assert_dot_path_staging(&mirror);
}

/// (d): an entrypoint whose canonical path leaves `project_root` through a
/// symlink is skipped at insertion — the build succeeds without it instead of
/// hard-erroring in the closure walk.
#[cfg(unix)]
#[test]
fn symlinked_entrypoint_escaping_project_root_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let project = write_standalone_project(&tmp.path().join("project"));
    let outside = tmp.path().join("outside/route.tsx");
    write(
        &outside,
        "export default function Route() { return null; }\n",
    );
    let entrypoint = project.join(".example-package/routes-src/route.tsx");
    fs::create_dir_all(entrypoint.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &entrypoint).unwrap();

    let session = bundle_with_entrypoints(&project, "dist-inj-link", vec![entrypoint]);
    let mirror = work_mirror_root(&session);

    assert!(!mirror
        .join(".example-package/routes-src/route.tsx")
        .exists());
}

/// (e): exact-file staging consults no gitignore — a gitignored entrypoint
/// dir still stages.
#[test]
fn gitignored_injected_entrypoint_still_stages() {
    let tmp = tempfile::tempdir().unwrap();
    let project = write_standalone_project(tmp.path());
    let entrypoint = write_injected_entrypoint_fixture(&project);
    write(&project.join(".gitignore"), ".example-package/\n");

    let session = bundle_with_entrypoints(&project, "dist-inj-ignored", vec![entrypoint]);
    let mirror = work_mirror_root(&session);

    assert!(mirror
        .join(".example-package/routes-src/route.tsx")
        .is_file());
    assert!(mirror
        .join(".example-package/routes-src/helper.tsx")
        .is_file());
}

/// (f): a project-root-level (non-hidden) entrypoint stages with its closure,
/// and seeds the root-entry dependency view exactly as the same file does
/// when registered as an existing exact target (a plugin alias).
#[test]
fn root_level_injected_entrypoint_matches_exact_target_staging() {
    fn write_root_level_fixture(project: &Path) -> PathBuf {
        let entrypoint = project.join("injected-entry.tsx");
        write(
            &entrypoint,
            "import { dep } from './lib/dep.ts';\nexport default function Route() { return dep; }\n",
        );
        write(&project.join("lib/dep.ts"), "export const dep = 'DEP';\n");
        write(&project.join("lib/unused.ts"), "export const unused = 1;\n");
        entrypoint
    }

    let via_entrypoint = tempfile::tempdir().unwrap();
    let project = write_standalone_project(via_entrypoint.path());
    let entrypoint = write_root_level_fixture(&project);
    let session = bundle_with_entrypoints(&project, "dist-root", vec![entrypoint]);
    let entrypoint_mirror = work_mirror_root(&session);
    assert!(entrypoint_mirror.join("injected-entry.tsx").is_file());
    assert!(entrypoint_mirror.join("lib/dep.ts").is_file());

    let via_alias = tempfile::tempdir().unwrap();
    let project = write_standalone_project(via_alias.path());
    let entrypoint = write_root_level_fixture(&project);
    let input = BundlerInput {
        plugin_alias_entries: vec![(
            "#injected-entry".to_string(),
            entrypoint.to_string_lossy().into_owned(),
        )],
        ..make_bundle_input(&project, "dist-root")
    };
    let mut alias_session = ShadowSession::new(&input.project_root).unwrap();
    bundle_with_session(input, Some(&mut alias_session)).expect("alias bundle succeeds");

    assert_eq!(
        staged_file_listing(&entrypoint_mirror),
        staged_file_listing(&work_mirror_root(&alias_session)),
        "a root-level entrypoint must stage exactly what an exact alias target does"
    );
}

/// Session prune bookkeeping: deleting the live dot dirs prunes the staged
/// copies on the next call of the SAME session — the allowlist reuses the
/// ShadowWriter visited/prune machinery instead of hand-copying.
#[test]
fn deleting_live_dot_dirs_prunes_staged_copies_next_session() {
    let tmp = tempfile::tempdir().unwrap();
    let project = write_standalone_project(tmp.path());
    write_dot_path_fixture(&project);

    let mut session = ShadowSession::new(&project).unwrap();
    bundle_with_session(
        make_bundle_input(&project, "dist-dot-prune"),
        Some(&mut session),
    )
    .expect("first bundle succeeds");
    let mirror = work_mirror_root(&session);
    assert_dot_path_staging(&mirror);

    fs::remove_dir_all(project.join(".zudo-doc")).unwrap();
    fs::remove_dir_all(project.join(".zfb")).unwrap();
    bundle_with_session(
        make_bundle_input(&project, "dist-dot-prune"),
        Some(&mut session),
    )
    .expect("second bundle succeeds");

    assert!(
        !mirror
            .join(".zudo-doc/routes-src/generated-route.tsx")
            .exists(),
        "deleted live route source must be pruned from the stage"
    );
    assert!(
        !mirror.join(".zfb/doc-history-meta.json").exists(),
        "deleted live meta file must be pruned from the stage"
    );
}
