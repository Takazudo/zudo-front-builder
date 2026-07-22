//! Issue #1885 (epic #1883) — real-esbuild regression coverage for the
//! workspace-root alias claim introduced by #1884.
//!
//! The mock staging tests prove the claim graph is materialised. This binary
//! exercises the consumer topology with real esbuild and its real metafile:
//! a nested workspace host aliases into the explicitly claimed root package,
//! a sibling package, and the pre-existing dot-path allowlist. It also keeps
//! two deliberately-red contracts separate: a parent-relative root-tree
//! crossing remains prohibited, while an unstaged first-party input must be
//! rejected by guard (b)'s stage-escape audit.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{bundle_with_session, BundleMode, BundlerInput, ShadowSession};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent directory");
    }
    fs::write(path, contents).expect("write fixture file");
}

/// Returns `(workspace_root, project_root)`. The root package and both
/// sub-packages are explicitly claimed, matching the supported #1884 shape.
fn write_workspace(root: &Path) -> (PathBuf, PathBuf) {
    write(
        &root.join("pnpm-workspace.yaml"),
        "packages:\n  - '.'\n  - 'sub-packages/*'\n",
    );
    write(
        &root.join("package.json"),
        r#"{ "name": "workspace-root", "private": true }"#,
    );
    let project = root.join("sub-packages/host");
    write(
        &project.join("package.json"),
        r#"{ "name": "host", "private": true }"#,
    );
    for dir in ["pages", "content", "components", "layouts"] {
        fs::create_dir_all(project.join(dir)).expect("create required project directory");
    }
    (root.to_path_buf(), project)
}

/// A real, workspace-hoisted dependency root. The positive fixture imports
/// this package from root-package source so resolution is rooted compatibly
/// with a normal pnpm workspace, rather than relying on the host's absent
/// local `node_modules` directory.
fn write_hoisted_node_modules(workspace: &Path) -> PathBuf {
    let node_modules = workspace.join("node_modules");
    write(
        &node_modules.join("root-compatible-dep/package.json"),
        r#"{ "name": "root-compatible-dep", "main": "index.js" }"#,
    );
    write(
        &node_modules.join("root-compatible-dep/index.js"),
        r#"export const rootCompatibleDep = "HOISTED_NODE_MODULES_MARKER";"#,
    );
    node_modules
}

fn base_input(project: &Path, esbuild: PathBuf, node_modules: PathBuf) -> BundlerInput {
    let mut input = BundlerInput::for_project(
        project.to_path_buf(),
        Framework::Preact,
        BundleMode::Production,
        project.join("dist"),
        None,
    );
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input.esbuild_binary = Some(esbuild);
    input.node_modules_dir = Some(node_modules);
    input.tsconfig_paths = BTreeMap::from([
        (
            "@/*".to_string(),
            vec![project.join("../..").to_string_lossy().into_owned() + "/*"],
        ),
        (
            "@components/*".to_string(),
            vec![project
                .join("../../components/*")
                .to_string_lossy()
                .into_owned()],
        ),
        (
            "@shared/*".to_string(),
            vec![project.join("../shared/*").to_string_lossy().into_owned()],
        ),
        (
            "#generated-route".to_string(),
            vec![project
                .join(".zudo-doc/routes-src/generated-route.tsx")
                .to_string_lossy()
                .into_owned()],
        ),
        (
            "#doc-history-meta".to_string(),
            vec![project
                .join(".zfb/doc-history-meta.json")
                .to_string_lossy()
                .into_owned()],
        ),
    ]);
    input
}

fn metafile_input_keys(shadow: &Path) -> Vec<String> {
    let bytes = fs::read(shadow.join(".zfb-metafile.json")).expect("read real esbuild metafile");
    let metafile: serde_json::Value =
        serde_json::from_slice(&bytes).expect("parse esbuild metafile");
    metafile["inputs"]
        .as_object()
        .expect("metafile inputs object")
        .keys()
        .cloned()
        .collect()
}

fn bundle_text(path: &Path) -> String {
    fs::read_to_string(path).expect("read emitted bundle")
}

#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_workspace_root_alias_esbuild_regression -- --ignored"]
fn real_esbuild_stages_workspace_root_alias_graph_with_sibling_and_dot_paths() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_workspace_root_alias_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let (workspace, project) = write_workspace(temp.path());
    let node_modules = write_hoisted_node_modules(&workspace);

    write(
        &workspace.join("components/root-card.tsx"),
        r#"
            import { rootSource } from "@/src/root-source";
            import "./root-card.css";
            import { rootCompatibleDep } from "root-compatible-dep";
            export function RootCard() {
              return "ROOT_COMPONENT_MARKER:" + rootSource + ":" + rootCompatibleDep;
            }
        "#,
    );
    write(
        &workspace.join("components/root-card.css"),
        "/* ROOT_CSS_MARKER: resolution-only; SSR uses the empty CSS loader. */\n",
    );
    write(
        &workspace.join("src/root-source.tsx"),
        r#"
            import rootData from "@/src/root-data.json";
            import { rootLib } from "@/lib/root-lib";
            export const rootSource = "ROOT_SRC_MARKER:" + rootLib + ":" + rootData.rootData;
        "#,
    );
    write(
        &workspace.join("src/root-data.json"),
        r#"{ "rootData": "ROOT_JSON_MARKER" }"#,
    );
    write(
        &workspace.join("lib/root-lib.ts"),
        r#"export const rootLib = "ROOT_LIB_MARKER";"#,
    );
    write(
        &workspace.join("sub-packages/shared/package.json"),
        r#"{ "name": "shared", "private": true }"#,
    );
    write(
        &workspace.join("sub-packages/shared/Badge.tsx"),
        r#"export const Badge = () => "SIBLING_PACKAGE_MARKER";"#,
    );
    write(
        &project.join(".zudo-doc/routes-src/generated-route.tsx"),
        r##"
            import docHistory from "#doc-history-meta";
            export const generatedRoute = "DOT_ROUTE_MARKER:" + docHistory.history[0];
        "##,
    );
    write(
        &project.join(".zfb/doc-history-meta.json"),
        r#"{ "history": ["DOT_JSON_MARKER"] }"#,
    );
    write(&project.join(".gitignore"), ".zudo-doc/\n.zfb/\n");
    write(
        &project.join("pages/index.tsx"),
        r##"
            import { RootCard } from "@components/root-card";
            import { Badge } from "@shared/Badge";
            import { generatedRoute } from "#generated-route";
            export default function Home() {
              return RootCard() + Badge() + generatedRoute;
            }
        "##,
    );

    let mut session = ShadowSession::new(&project).expect("shadow session");
    let output = bundle_with_session(
        base_input(&project, esbuild, node_modules),
        Some(&mut session),
    )
    .expect("#1884 must stage concretely reached workspace-root aliases for real esbuild");

    let body = bundle_text(&output.bundle_path);
    for marker in [
        "ROOT_COMPONENT_MARKER",
        "ROOT_SRC_MARKER",
        "ROOT_JSON_MARKER",
        "ROOT_LIB_MARKER",
        "SIBLING_PACKAGE_MARKER",
        "DOT_ROUTE_MARKER",
        "DOT_JSON_MARKER",
        "HOISTED_NODE_MODULES_MARKER",
    ] {
        assert!(
            body.contains(marker),
            "bundle must contain {marker}; got: {body}"
        );
    }

    // Esbuild's cwd is the nested project mirror. These exact, workspace-
    // relative spellings prove every first-party source class was resolved
    // from the stage; a live-tree fallback would instead be absolute or climb
    // out of the work mirror and fail the stage-escape audit.
    let shadow = fs::canonicalize(session.shadow_root())
        .expect("canonicalize work mirror")
        .join("sub-packages/host");
    let keys = metafile_input_keys(&shadow);
    for staged_key in [
        "pages/index.tsx",
        "../../components/root-card.tsx",
        "../../components/root-card.css",
        "../../src/root-source.tsx",
        "../../src/root-data.json",
        "../../lib/root-lib.ts",
        "../shared/Badge.tsx",
        ".zudo-doc/routes-src/generated-route.tsx",
        ".zfb/doc-history-meta.json",
    ] {
        assert!(
            keys.iter().any(|key| key == staged_key),
            "metafile must contain exact staged spelling {staged_key}; got {keys:?}"
        );
    }
}

#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_workspace_root_alias_esbuild_regression -- --ignored"]
fn parent_relative_root_tree_crossing_stays_rejected_even_when_alias_target_is_legal() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_workspace_root_alias_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let (workspace, project) = write_workspace(temp.path());
    let node_modules = write_hoisted_node_modules(&workspace);
    write(
        &workspace.join("components/illegal-relative.tsx"),
        r#"
            import { rootSource } from "../src/root-source";
            export const illegalRelative = rootSource;
        "#,
    );
    write(
        &workspace.join("src/root-source.ts"),
        r#"export const rootSource = "LEGAL_ALIAS_TARGET_MARKER";"#,
    );
    write(
        &project.join("pages/index.tsx"),
        r#"
            import { illegalRelative } from "@components/illegal-relative";
            // The same root src target is legal and concretely claimed through
            // the workspace-root alias; only the component's relative spelling
            // must remain rejected.
            import { rootSource as legalAlias } from "@/src/root-source";
            export default function Home() { return illegalRelative + legalAlias; }
        "#,
    );

    let error = bundle_with_session(
        base_input(&project, esbuild, node_modules),
        Some(&mut ShadowSession::new(&project).expect("shadow session")),
    )
    .expect_err("parent-relative root-package tree crossings must remain prohibited");
    let message = format!("{error:#}");
    assert!(
        message.contains("parent-escaping relative value import"),
        "{message}"
    );
    assert!(message.contains("../src/root-source"), "{message}");
    assert!(
        message.contains("use a tsconfig alias spelling"),
        "{message}"
    );
}

#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_workspace_root_alias_esbuild_regression -- --ignored"]
fn real_metafile_guard_b_rejects_genuinely_unstaged_workspace_input() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_workspace_root_alias_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let (workspace, project) = write_workspace(temp.path());
    let node_modules = write_hoisted_node_modules(&workspace);
    // Hidden root-package input: its explicit alias is legal TypeScript, but
    // the claim policy must not stage hidden trees. Esbuild can only reach the
    // live fallback, so guard (b) must reject its real metafile record.
    write(
        &workspace.join(".unstaged/escape.ts"),
        r#"export const unstagedEscape = "UNSTAGED_GUARD_B_MARKER";"#,
    );
    write(
        &project.join("pages/index.tsx"),
        r#"
            import { unstagedEscape } from "@unstaged";
            export default function Home() { return unstagedEscape; }
        "#,
    );

    let mut input = base_input(&project, esbuild, node_modules);
    input.tsconfig_paths.insert(
        "@unstaged".to_string(),
        vec![workspace
            .join(".unstaged/escape.ts")
            .to_string_lossy()
            .into_owned()],
    );
    let error = bundle_with_session(
        input,
        Some(&mut ShadowSession::new(&project).expect("shadow session")),
    )
    .expect_err("guard (b) must reject a first-party source with no staged spelling");
    let message = format!("{error:#}");
    assert!(message.contains("stage-escape audit"), "{message}");
    assert!(message.contains("no staged spelling"), "{message}");
    assert!(message.contains(".unstaged/escape.ts"), "{message}");
}
