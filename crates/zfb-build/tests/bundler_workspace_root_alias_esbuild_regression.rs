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
            import "./generated/root-card.css";
            import { rootCompatibleDep } from "root-compatible-dep";
            export function RootCard() {
              return "ROOT_COMPONENT_MARKER:" + rootSource + ":" + rootCompatibleDep;
            }
        "#,
    );
    write(
        &workspace.join("components/generated/root-card.css"),
        "/* ROOT_CSS_MARKER: resolution-only; SSR uses the empty CSS loader. */\n",
    );
    write(
        &workspace.join("src/root-source.tsx"),
        r#"
            import rootData from "@/src/data/generated/root-data.json";
            import { rootLib } from "@/lib/root-lib";
            export const rootSource = "ROOT_SRC_MARKER:" + rootLib + ":" + rootData.rootData;
        "#,
    );
    write(
        &workspace.join("src/data/generated/root-data.json"),
        r#"{ "rootData": "ROOT_JSON_MARKER" }"#,
    );
    write(
        &workspace.join("src/data/generated/unreached.json"),
        r#"{ "rootData": "UNREACHED_MUST_NOT_STAGE" }"#,
    );
    write(
        &workspace.join(".gitignore"),
        "components/generated/\nsrc/data/generated/\n",
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
        "../../components/generated/root-card.css",
        "../../src/root-source.tsx",
        "../../src/data/generated/root-data.json",
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
    assert!(
        !session
            .shadow_root()
            .join("src/data/generated/unreached.json")
            .exists(),
        "an unreached sibling of an exact ignored leaf must stay absent"
    );
}

/// Issue #1905's combined consumer shape. The 17 inputs reported by the
/// blocked next.92 adoption are deliberately all reached in one real-esbuild
/// call: six package-owned generated route sources, nine public subpath
/// exports from a `workspace:*` sibling, and two generated JSON leaves
/// reached through the claimed workspace-root alias. CSS is an additional
/// (non-17) input because it has its own loader/output contract.
///
/// Keep the exact metafile spellings below: a green bundle alone cannot tell
/// a staged input apart from a live-tree fallback.
#[cfg(unix)]
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_workspace_root_alias_esbuild_regression -- --ignored"]
fn real_esbuild_combines_all_next_92_residual_stage_escape_classes() {
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
            import "./generated/root-card.css";
            export const rootCard = "ROOT_CSS_CONSUMER:" + rootSource;
        "#,
    );
    write(
        &workspace.join("components/generated/root-card.css"),
        "/* COMBINED_ROOT_CSS_MARKER: SSR's CSS loader is intentionally empty. */\n",
    );
    write(
        &workspace.join("src/root-source.ts"),
        r#"
            import first from "@/src/data/generated/first.json";
            import second from "@/src/data/generated/second.json";
            export const rootSource = "COMBINED_ROOT_JSON:" + first.value + ":" + second.value;
        "#,
    );
    write(
        &workspace.join("src/data/generated/first.json"),
        r#"{ "value": "FIRST_JSON_MARKER" }"#,
    );
    write(
        &workspace.join("src/data/generated/second.json"),
        r#"{ "value": "SECOND_JSON_MARKER" }"#,
    );
    write(
        &workspace.join(".gitignore"),
        "components/generated/\nsrc/data/generated/\n",
    );

    let route_files = [
        ("robots.txt.tsx", "robots"),
        ("404.tsx", "not-found"),
        ("_chrome.tsx", "chrome"),
        ("_context.ts", "context"),
        ("docs-slug.tsx", "docs-slug"),
        ("sitemap.xml.tsx", "sitemap"),
    ];
    for (file, marker) in route_files {
        write(
            &project.join(".zudo-doc/routes-src").join(file),
            &format!("export const routeMarker = \"COMBINED_ROUTE_{marker}\";\n"),
        );
    }
    write(&project.join(".gitignore"), ".zudo-doc/\n");

    let ui = workspace.join("sub-packages/ui-preact");
    let package_exports = [
        ("code", "code"),
        ("action-button", "action-button"),
        ("h4", "h4"),
        ("h5", "h5"),
        ("h6", "h6"),
        ("em", "em"),
        ("hr", "hr"),
        ("strong", "strong"),
        ("story-contract", "story-contract"),
    ];
    let exports = package_exports
        .iter()
        .map(|(name, _)| {
            let extension = if *name == "story-contract" {
                "ts"
            } else {
                "tsx"
            };
            format!(r#""./{name}":"./src/{name}/{name}.{extension}""#)
        })
        .collect::<Vec<_>>()
        .join(",");
    write(
        &ui.join("package.json"),
        &format!(r#"{{"name":"@acme/ui-preact","exports":{{{exports}}}}}"#),
    );
    for (name, marker) in package_exports {
        let extension = if name == "story-contract" {
            "ts"
        } else {
            "tsx"
        };
        write(
            &ui.join("src")
                .join(name)
                .join(format!("{name}.{extension}")),
            &format!("export const packageMarker = \"COMBINED_PACKAGE_{marker}\";\n"),
        );
    }
    fs::create_dir_all(node_modules.join("@acme")).expect("create scoped node_modules directory");
    std::os::unix::fs::symlink(&ui, node_modules.join("@acme/ui-preact"))
        .expect("link declared workspace package into hoisted install");
    write(
        &project.join("package.json"),
        r#"{"name":"host","dependencies":{"@acme/ui-preact":"workspace:*"}}"#,
    );

    let route_imports = route_files
        .iter()
        .enumerate()
        .map(|(index, (file, _))| {
            format!("import {{ routeMarker as route{index} }} from \"#route-{index}\"; // {file}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let package_imports = package_exports
        .iter()
        .enumerate()
        .map(|(index, (name, _))| {
            format!("import {{ packageMarker as package{index} }} from \"@acme/ui-preact/{name}\";")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let route_values = (0..route_files.len())
        .map(|index| format!("route{index}"))
        .collect::<Vec<_>>()
        .join(" + ");
    let package_values = (0..package_exports.len())
        .map(|index| format!("package{index}"))
        .collect::<Vec<_>>()
        .join(" + ");
    write(
        &project.join("pages/index.tsx"),
        &format!(
            "import {{ rootCard }} from \"@components/root-card\";\n{route_imports}\n{package_imports}\nexport default function Home() {{ return rootCard + {route_values} + {package_values}; }}\n"
        ),
    );

    let mut input = base_input(&project, esbuild, node_modules);
    for (index, (file, _)) in route_files.iter().enumerate() {
        input.tsconfig_paths.insert(
            format!("#route-{index}"),
            vec![project
                .join(".zudo-doc/routes-src")
                .join(file)
                .to_string_lossy()
                .into_owned()],
        );
    }
    let mut session = ShadowSession::new(&project).expect("shadow session");
    let output = bundle_with_session(input, Some(&mut session)).expect(
        "all next.92 residual stage-escape classes must resolve from one staged workspace consumer",
    );
    let body = bundle_text(&output.bundle_path);
    for marker in [
        "COMBINED_ROOT_JSON",
        "FIRST_JSON_MARKER",
        "SECOND_JSON_MARKER",
        "COMBINED_ROUTE_robots",
        "COMBINED_ROUTE_not-found",
        "COMBINED_ROUTE_chrome",
        "COMBINED_ROUTE_context",
        "COMBINED_ROUTE_docs-slug",
        "COMBINED_ROUTE_sitemap",
        "COMBINED_PACKAGE_code",
        "COMBINED_PACKAGE_action-button",
        "COMBINED_PACKAGE_h4",
        "COMBINED_PACKAGE_h5",
        "COMBINED_PACKAGE_h6",
        "COMBINED_PACKAGE_em",
        "COMBINED_PACKAGE_hr",
        "COMBINED_PACKAGE_strong",
        "COMBINED_PACKAGE_story-contract",
    ] {
        assert!(
            body.contains(marker),
            "bundle must contain {marker}; got: {body}"
        );
    }

    let shadow = fs::canonicalize(session.shadow_root())
        .expect("canonicalize work mirror")
        .join("sub-packages/host");
    let keys = metafile_input_keys(&shadow);
    let residual_keys = [
        ".zudo-doc/routes-src/robots.txt.tsx",
        ".zudo-doc/routes-src/404.tsx",
        ".zudo-doc/routes-src/_chrome.tsx",
        ".zudo-doc/routes-src/_context.ts",
        ".zudo-doc/routes-src/docs-slug.tsx",
        ".zudo-doc/routes-src/sitemap.xml.tsx",
        "node_modules/@acme/ui-preact/src/code/code.tsx",
        "node_modules/@acme/ui-preact/src/action-button/action-button.tsx",
        "node_modules/@acme/ui-preact/src/h4/h4.tsx",
        "node_modules/@acme/ui-preact/src/h5/h5.tsx",
        "node_modules/@acme/ui-preact/src/h6/h6.tsx",
        "node_modules/@acme/ui-preact/src/em/em.tsx",
        "node_modules/@acme/ui-preact/src/hr/hr.tsx",
        "node_modules/@acme/ui-preact/src/strong/strong.tsx",
        "node_modules/@acme/ui-preact/src/story-contract/story-contract.ts",
        "../../src/data/generated/first.json",
        "../../src/data/generated/second.json",
    ];
    assert_eq!(
        residual_keys.len(),
        17,
        "the adoption report has 17 residual inputs"
    );
    for expected in residual_keys {
        assert!(
            keys.iter().any(|key| key == expected),
            "metafile must contain exact staged residual spelling {expected}; got {keys:?}"
        );
    }
    assert!(
        keys.iter()
            .any(|key| key == "../../components/generated/root-card.css"),
        "the CSS proof must use its staged metafile input; got {keys:?}"
    );
    assert!(
        keys.iter().all(|key| !key.starts_with('/')),
        "no canonical live first-party input may appear in the metafile; got {keys:?}"
    );
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
