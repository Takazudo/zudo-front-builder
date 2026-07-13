use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use zfb_build::bundler::shadow_parent_dir;
use zfb_build::{bundle, BundleMode, BundlerInput, ContentCollectionSpec, ShadowSession};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

const CHILD_KIND: &str = "ZFB_SHADOW_TMPDIR_ISOLATION_CHILD";
const CHILD_ROOT: &str = "ZFB_SHADOW_TMPDIR_ISOLATION_ROOT";
const CHILD_CACHE: &str = "ZFB_SHADOW_TMPDIR_ISOLATION_CACHE";
const CHILD_ESBUILD: &str = "ZFB_SHADOW_TMPDIR_ISOLATION_ESBUILD";
const COMPLETE_FILE: &str = ".shadow-tmpdir-child-complete";
const SHADOW_PREFIXES: &[&str] = &[
    "zfb-shadow-session-",
    "zfb-bundler-",
    "zfb-exact-node-modules-",
];

#[test]
fn ephemeral_bundler_tmpdir_is_not_project_local_when_tmpdir_is_project_local() {
    if in_child("ephemeral") {
        let esbuild = child_esbuild();
        let root = child_root();
        scaffold_project(&root);
        fs::create_dir_all(root.join("node_modules/tmpdir-live-dependency")).unwrap();
        fs::write(
            root.join("node_modules/tmpdir-live-dependency/package.json"),
            r#"{"type":"module"}"#,
        )
        .unwrap();
        fs::write(
            root.join("node_modules/tmpdir-live-dependency/secret.js"),
            "export default 'PROJECT_LOCAL_TMPDIR_SECRET_MUST_NOT_LEAK';\n",
        )
        .unwrap();
        fs::write(
            root.join("components/Included.stories.tsx"),
            "export const Included = () => 'included';\n",
        )
        .unwrap();
        fs::write(
            root.join("components/Excluded.stories.tsx"),
            "import secret from 'tmpdir-live-dependency/secret.js';\n\
             export const Excluded = () => secret;\n",
        )
        .unwrap();
        fs::write(
            root.join("components/_gallery.tsx"),
            "const stories = import.meta.glob('./*.stories.tsx', { eager: true });\n\
             export const galleryKeys = Object.keys(stories);\n",
        )
        .unwrap();
        fs::write(
            root.join("pages/index.tsx"),
            "import { galleryKeys } from '../components/_gallery';\n\
             export default function Page() { return galleryKeys.join(','); }\n",
        )
        .unwrap();

        let mut input = make_input(&root, &esbuild, "dist-ephemeral");
        input.node_modules_dir = Some(root.join("node_modules"));
        input.bundle_exclude = vec!["components/Excluded.stories.tsx".to_string()];
        let output = bundle(input).expect("bundle should succeed with project-local TMPDIR");
        let body = fs::read_to_string(output.bundle_path).expect("read emitted bundle");
        assert!(
            !body.contains("PROJECT_LOCAL_TMPDIR_SECRET_MUST_NOT_LEAK"),
            "excluded dependency marker must not be bundled"
        );
        assert_shadow_parent_outside_project(&root);
        assert_no_shadow_dirs_under_project_tmp(&root);
        mark_child_complete(&root);
        return;
    }

    run_esbuild_child("ephemeral");
}

#[test]
fn shadow_session_tmpdir_is_not_project_local_when_tmpdir_is_project_local() {
    if in_child("session") {
        let root = child_root();
        fs::create_dir_all(&root).unwrap();
        let session = ShadowSession::new(&root).expect("allocate shadow session");
        assert_path_outside_project(&root, session.shadow_root(), "shadow session root");
        assert_shadow_parent_outside_project(&root);
        assert_no_shadow_dirs_under_project_tmp(&root);
        mark_child_complete(&root);
        return;
    }

    run_child("session", None);
}

#[test]
fn exact_node_modules_tmpdir_is_not_project_local_when_tmpdir_is_project_local() {
    if in_child("exact") {
        let esbuild = child_esbuild();
        let root = child_root();
        scaffold_project(&root);
        let package = root.join("node_modules/tmpdir-exact-dependency");
        fs::create_dir_all(package.join("dist")).unwrap();
        fs::write(
            package.join("package.json"),
            r#"{"type":"module","exports":"./dist/entry.js"}"#,
        )
        .unwrap();
        fs::write(
            package.join("dist/entry.js"),
            "export default 'PROJECT_LOCAL_TMPDIR_EXACT_DEPENDENCY';\n",
        )
        .unwrap();
        fs::write(
            root.join("pages/index.tsx"),
            "import value from 'plugin:tmpdir-exact';\n\
             export default function Page() { return value; }\n",
        )
        .unwrap();

        let mut input = make_input(&root, &esbuild, "dist-exact");
        input.node_modules_dir = Some(root.join("node_modules"));
        input.plugin_alias_entries = vec![(
            "plugin:tmpdir-exact".to_string(),
            package.join("dist/entry.js").to_string_lossy().into_owned(),
        )];
        let output = bundle(input).expect("exact-node-modules bundle should succeed");
        let body = fs::read_to_string(&output.bundle_path).expect("read emitted bundle");
        assert!(
            body.contains("PROJECT_LOCAL_TMPDIR_EXACT_DEPENDENCY"),
            "fixture must actually bundle the staged node_modules target"
        );

        let sourcemap = fs::read_to_string(&output.sourcemap_path).expect("read sourcemap");
        let exact_root = extract_zfb_exact_root_from_sourcemap(&output.sourcemap_path, &sourcemap)
            .expect("sourcemap should expose the exact-node-modules staging root");
        assert_path_outside_project(&root, &exact_root, "exact-node-modules root");
        assert_shadow_parent_outside_project(&root);
        assert_no_shadow_dirs_under_project_tmp(&root);
        mark_child_complete(&root);
        return;
    }

    run_esbuild_child("exact");
}

fn run_esbuild_child(kind: &str) {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_shadow_tmpdir_isolation] no esbuild binary available; skipping.");
        return;
    };
    run_child(
        kind,
        Some(fs::canonicalize(esbuild).expect("canonical esbuild")),
    );
}

fn run_child(kind: &str, esbuild: Option<PathBuf>) {
    let project = tempfile::tempdir().expect("project root");
    let project_tmp = project.path().join(".tmp");
    fs::create_dir_all(&project_tmp).expect("project-local tmp");
    let outside_cache = tempfile::tempdir().expect("outside cache");

    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--exact")
        .arg(current_test_name(kind))
        .arg("--nocapture")
        .env(CHILD_KIND, kind)
        .env(CHILD_ROOT, project.path())
        .env(CHILD_CACHE, outside_cache.path())
        .env("TMPDIR", &project_tmp)
        .env("TEMP", &project_tmp)
        .env("TMP", &project_tmp)
        .env("XDG_CACHE_HOME", outside_cache.path());
    if let Some(esbuild) = esbuild {
        command.env(CHILD_ESBUILD, &esbuild);
        command.env("ZFB_ESBUILD_BIN", esbuild);
    }

    let output = command.output().expect("spawn isolated TMPDIR child");
    assert!(
        output.status.success(),
        "child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        project.path().join(COMPLETE_FILE).is_file(),
        "child process exited without running assertions"
    );
    assert_no_shadow_dirs_under_project_tmp(project.path());
}

fn current_test_name(kind: &str) -> &'static str {
    match kind {
        "ephemeral" => "ephemeral_bundler_tmpdir_is_not_project_local_when_tmpdir_is_project_local",
        "session" => "shadow_session_tmpdir_is_not_project_local_when_tmpdir_is_project_local",
        "exact" => "exact_node_modules_tmpdir_is_not_project_local_when_tmpdir_is_project_local",
        _ => unreachable!("unknown child kind"),
    }
}

fn in_child(kind: &str) -> bool {
    std::env::var(CHILD_KIND).as_deref() == Ok(kind)
}

fn child_root() -> PathBuf {
    PathBuf::from(std::env::var_os(CHILD_ROOT).expect("child root"))
}

fn child_esbuild() -> PathBuf {
    let Some(esbuild) = locate_esbuild() else {
        panic!("child process lost the pinned esbuild binary");
    };
    let expected = PathBuf::from(std::env::var_os(CHILD_ESBUILD).expect("child esbuild"));
    assert_eq!(
        fs::canonicalize(&esbuild).expect("canonical located esbuild"),
        fs::canonicalize(&expected).expect("canonical expected esbuild"),
        "child should honor the pinned ZFB_ESBUILD_BIN"
    );
    expected
}

fn mark_child_complete(root: &Path) {
    fs::write(root.join(COMPLETE_FILE), "ok").unwrap();
}

fn assert_shadow_parent_outside_project(root: &Path) {
    let parent = shadow_parent_dir(root).expect("shadow parent");
    assert_path_outside_project(root, &parent, "shadow parent");

    let cache = PathBuf::from(std::env::var_os(CHILD_CACHE).expect("child cache"));
    let expected = fs::canonicalize(cache.join("zfb")).expect("canonical XDG_CACHE_HOME/zfb");
    assert_eq!(
        parent, expected,
        "project-local TMPDIR must be rejected in favor of the outside cache parent"
    );
}

fn assert_path_outside_project(root: &Path, path: &Path, label: &str) {
    let project = fs::canonicalize(root).expect("canonical project");
    let path = fs::canonicalize(path).unwrap_or_else(|_| normalize_path_lexical(path));
    assert!(
        !path.starts_with(&project),
        "{label} must be outside project root: {} is inside {}",
        path.display(),
        project.display()
    );
}

fn assert_no_shadow_dirs_under_project_tmp(root: &Path) {
    let project_tmp = root.join(".tmp");
    if !project_tmp.exists() {
        return;
    }
    let leaked: Vec<_> = fs::read_dir(&project_tmp)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| {
                    SHADOW_PREFIXES
                        .iter()
                        .any(|prefix| name.starts_with(prefix))
                })
                .unwrap_or(false)
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "project-local .tmp must not contain shadow tempdirs: {leaked:?}"
    );
}

fn extract_zfb_exact_root_from_sourcemap(
    sourcemap_path: &Path,
    sourcemap: &str,
) -> Option<PathBuf> {
    let map: serde_json::Value = serde_json::from_str(sourcemap).ok()?;
    let sources = map.get("sources")?.as_array()?;
    let sourcemap_dir = sourcemap_path.parent()?;
    sources
        .iter()
        .filter_map(|source| source.as_str())
        .find_map(|source| {
            let marker = "zfb-exact-node-modules-";
            let index = source.find(marker)?;
            let relative_tail = &source[index..];
            let root_len = relative_tail
                .find("/node_modules/")
                .or_else(|| relative_tail.find("\\node_modules\\"))?;
            let root = PathBuf::from(&source[..index + root_len]);
            let absolute_root = if root.is_absolute() {
                root
            } else {
                sourcemap_dir.join(root)
            };
            Some(normalize_path_lexical(&absolute_root))
        })
}

fn normalize_path_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn scaffold_project(root: &Path) {
    for dir in ["pages", "content", "components", "layouts"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function Layout({ children }) { return children; }\n",
    )
    .unwrap();
}

fn make_input(root: &Path, esbuild: &Path, outdir_name: &str) -> BundlerInput {
    BundlerInput {
        main_fields: Vec::new(),
        extra_loader_args: Vec::new(),
        project_root: root.to_path_buf(),
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
        outdir: root.join(outdir_name),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: Vec::<ContentCollectionSpec>::new(),
        pipeline_spec: zfb_content::PipelineSpec::default(),
        resolve_markdown_links: None,
        site: None,
        prefetch_disabled: false,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: HashMap::new(),
        mdx_components_file: None,
        bundle_exclude: Vec::new(),
        base_prefix: None,
    }
}
