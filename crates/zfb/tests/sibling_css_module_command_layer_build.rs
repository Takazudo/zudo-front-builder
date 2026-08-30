//! Issue #1698 — `crates/zfb` command-layer regression test for sibling
//! `.module.css` (issue #1696: CSS discovery + emission wiring through
//! `zfb_build::SiblingMirrorPlan`).
//!
//! `crates/zfb-build/tests/bundler_sibling_mirror_esbuild_regression.rs`'s
//! case (e) proves the BUNDLER'S sibling `.module.css` rewrite reaches
//! esbuild correctly, but it pre-supplies `BundlerInput::css_module_class_maps`
//! directly (the shortcut documented on
//! `crates/zfb-build/tests/bundler_css_modules.rs`) — it cannot exercise
//! `discover_css_source_files` / `compute_css_module_class_maps`, which only
//! exist at THIS crate's command layer (`crates/zfb/src/commands/build.rs`).
//! This test drives the real `zfb build` binary end-to-end so a sibling
//! `.module.css`'s class map is DISCOVERED and HASHED by the real
//! command-layer scan — not hand-supplied — proving #1696's
//! `discover_css_source_files` (walking every claimed `SiblingMirrorPlan`
//! mirror root) and `compute_css_module_class_maps` / `run_css_emitter`
//! (gating a resolved sibling module on `plan.claims_path` and hashing via
//! the workspace-aware `CssModulesConfig`) are actually wired into `zfb
//! build`, mirroring `css_modules_components_build.rs`'s project-level
//! pattern for issue #553 but for a WORKSPACE SIBLING reached through a
//! wildcard tsconfig alias under an UNRELATED non-empty `bundle.exclude`
//! (the exact combination issue #1685 broke).
//!
//! ## Regression criterion
//!
//! Verified to FAIL when cherry-picked onto the epic's parent
//! (`base/sweep-260718`, pre-epic — no `SiblingMirrorPlan`, no sibling CSS
//! discovery) and PASS on the epic branch; see the PR/issue description for
//! both run transcripts.
//!
//! Unix-only (`#![cfg(unix)]`): the fixture symlinks the workspace-hoisted
//! `node_modules` to the embedded-vendor tree (`std::os::unix::fs::symlink`),
//! matching `css_modules_components_build.rs`'s
//! `corp_shape_with_real_node_modules_and_no_tsconfig_paths_builds` gate —
//! Windows symlinks need admin/developer mode this test doesn't assume.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use zfb_test_utils::{locate_esbuild, zfb_binary};

fn collect_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|s| s.to_str()) == Some(ext) {
                out.push(p);
            }
        }
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let cut = (0..=n).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    format!("{}…[truncated]", &s[..cut])
}

/// A pnpm workspace whose `sub-packages/host` member imports a sibling
/// component (`<ws_root>/lib/shared/Widget.tsx`) through a wildcard
/// tsconfig alias (`@shared/*`). The sibling component imports its own
/// `Widget.module.css` — the class map for it can ONLY be produced by the
/// real command-layer scan discovering the sibling through the claimed
/// `SiblingMirrorPlan` mirror root, since nothing under `project_root`
/// itself references `Widget.module.css`.
///
/// Returns `(project_root, node_modules_tempdir_handle)` — the handle keeps
/// the workspace-hoisted `node_modules` (symlinked from the SAME
/// embedded-vendor tree the `zfb` binary itself would extract, per
/// `css_modules_components_build.rs`'s `corp_shape_with_real_node_modules_...`
/// variant) alive for the test's duration. The SIBLING's own automatic-JSX
/// import (`preact/jsx-runtime`) resolves via `<work>/node_modules`, which
/// the bundler symlinks to `<ws_root>/node_modules` regardless of
/// `bundle.exclude` (issue #1693) — so the workspace root needs a real
/// `node_modules/preact`, not just the project's own embedded fallback.
fn write_fixture(ws_root: &Path) -> (PathBuf, tempfile::TempDir) {
    fs::write(
        ws_root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'sub-packages/*'\n",
    )
    .unwrap();
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, ws_root.join("node_modules"))
        .expect("symlink workspace node_modules");

    let project = ws_root.join("sub-packages/host");
    fs::create_dir_all(project.join("pages")).unwrap();

    // No `tailwind` key -> CSS enabled by default (matches
    // `css_modules_components_build.rs`'s corp-shape fixture). An UNRELATED
    // non-empty `bundle.exclude` arms the shadow-only / no-live-fallback
    // regime issue #1685 broke under.
    fs::write(
        project.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "bundle": { "exclude": ["components/never-matches/**"] }
}
"#,
    )
    .unwrap();

    fs::write(
        project.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@shared/*": ["../../lib/shared/*"] }
  }
}
"#,
    )
    .unwrap();

    fs::write(
        project.join("pages/index.tsx"),
        r#"import Widget from "@shared/Widget";

export default function HomePage() {
  return (
    <main>
      <Widget />
    </main>
  );
}
"#,
    )
    .unwrap();

    let sibling = ws_root.join("lib/shared");
    fs::create_dir_all(&sibling).unwrap();
    fs::write(
        sibling.join("Widget.module.css"),
        ".box { color: #101010; }\n.label { color: #202020; }\n",
    )
    .unwrap();
    fs::write(
        sibling.join("Widget.tsx"),
        r#"import styles from "./Widget.module.css";

export default function Widget() {
  return (
    <section class={styles.box}>
      <span class={styles.label}>widget</span>
    </section>
  );
}
"#,
    )
    .unwrap();

    (project, nm_handle)
}

/// Issue #1696/#1698: a workspace-sibling `.module.css` reached through a
/// claimed tsconfig alias gets a REAL, command-layer-computed class map
/// (not a manually-supplied one) that reaches both the emitted HTML and the
/// emitted stylesheet, under an UNRELATED non-empty `bundle.exclude`.
#[test]
fn sibling_module_css_class_map_is_discovered_and_emitted_by_real_zfb_build() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[sibling_css_module_command_layer_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, _nm_handle) = write_fixture(tmp.path());

    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&project)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected `zfb build` to succeed under an UNRELATED non-empty \
         bundle.exclude; got status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );

    let dist = project.join("dist");
    let html_paths = collect_files(&dist, "html");
    assert!(
        !html_paths.is_empty(),
        "no HTML files emitted under dist/; expected at least dist/index.html"
    );
    let html_blob = html_paths
        .iter()
        .map(|p| fs::read_to_string(p).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    for local in ["box", "label"] {
        let needle = format!("_{local}");
        assert!(
            html_blob.contains(&needle),
            "expected hashed class containing `{needle}` in emitted HTML — a \
             raw `{local}` class (or a missing one) would mean the sibling's \
             class map was never discovered by the command layer.\n--- html ---\n{}",
            truncate(&html_blob, 1200)
        );
    }

    let assets_dir = dist.join("assets");
    let css_paths = collect_files(&assets_dir, "css");
    let styles_css = css_paths
        .iter()
        .find(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.starts_with("styles-") && n.ends_with(".css"))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            panic!("expected dist/assets/styles-<hash>.css to be emitted; got: {css_paths:#?}")
        });
    let css_body = fs::read_to_string(styles_css).unwrap();
    for local in ["box", "label"] {
        let needle = format!("_{local}");
        assert!(
            css_body.contains(&needle),
            "expected scoped selector containing `{needle}` in {}; the \
             sibling's scoped CSS must reach the emitted stylesheet, not just \
             the class-map shim.\n--- css ---\n{}",
            styles_css.display(),
            truncate(&css_body, 1200),
        );
    }
}

/// Write the #1883 workspace-root alias fixture. The nested host can reach
/// concrete root-package source only because the workspace claims `.`; the
/// broad alias must not turn that claim into a wholesale root-tree mirror.
///
/// The workspace-root `node_modules` symlink deliberately gives the root
/// source the same hoisted runtime-dependency topology as a pnpm workspace.
/// Returning its tempdir handle keeps the embedded dependency tree alive for
/// the spawned command.
fn write_workspace_root_alias_fixture(ws_root: &Path) -> (PathBuf, tempfile::TempDir) {
    fs::write(
        ws_root.join("pnpm-workspace.yaml"),
        "packages:\n  - '.'\n  - 'sub-packages/*'\n",
    )
    .unwrap();
    fs::write(
        ws_root.join("package.json"),
        r#"{ "name": "workspace-root", "private": true }"#,
    )
    .unwrap();
    fs::write(
        ws_root.join(".gitignore"),
        "components/generated/\ncomponents/data/generated/\n",
    )
    .unwrap();
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, ws_root.join("node_modules"))
        .expect("symlink workspace node_modules");

    let project = ws_root.join("sub-packages/host");
    fs::create_dir_all(project.join("pages")).unwrap();
    fs::write(
        project.join("package.json"),
        r#"{ "name": "host", "private": true }"#,
    )
    .unwrap();
    fs::write(
        project.join("zfb.config.json"),
        r#"{
  "framework": "preact"
}
"#,
    )
    .unwrap();
    fs::write(
        project.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@/*": ["../../*"],
      "@components/*": ["../../components/*"],
      "@shared/*": ["../shared/*"]
    }
  }
}
"#,
    )
    .unwrap();

    let components = ws_root.join("components");
    fs::create_dir_all(&components).unwrap();
    fs::create_dir_all(components.join("generated")).unwrap();
    fs::create_dir_all(components.join("data/generated")).unwrap();
    fs::write(
        components.join("root-card.tsx"),
        r#"import { rootSource } from "@/components/root-source";
import styles from "./generated/root-card.module.css";

export function RootCard() {
  return <section class={styles.rootAliasStyle}>ROOT_COMPONENT_MARKER:{rootSource}</section>;
}
"#,
    )
    .unwrap();
    fs::write(
        components.join("generated/root-card.module.css"),
        ".rootAliasStyle { color: #123abc; } /* ROOT_CSS_MARKER */\n",
    )
    .unwrap();

    fs::write(
        components.join("root-source.ts"),
        r#"import rootData from "@/components/data/generated/root-data.json";

export const rootSource = `ROOT_SRC_MARKER:ROOT_LIB_MARKER:${rootData.rootData}`;
"#,
    )
    .unwrap();
    fs::write(
        components.join("data/generated/root-data.json"),
        r#"{ "rootData": "ROOT_JSON_MARKER" }"#,
    )
    .unwrap();
    fs::write(
        components.join("data/generated/unreached.json"),
        r#"{ "rootData": "UNREACHED_MUST_NOT_STAGE" }"#,
    )
    .unwrap();

    let sibling = ws_root.join("sub-packages/shared");
    fs::create_dir_all(&sibling).unwrap();
    fs::write(
        sibling.join("package.json"),
        r#"{ "name": "shared", "private": true }"#,
    )
    .unwrap();
    fs::write(
        sibling.join("Badge.tsx"),
        r#"export function Badge() {
  return <aside>SIBLING_PACKAGE_MARKER</aside>;
}
"#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"import { rootSource } from "@/components/root-source";
import { RootCard } from "@components/root-card";
import { Badge } from "@shared/Badge";

export default function Home() {
  return <main>ROOT_PAGE_MARKER:{rootSource}<RootCard /><Badge /></main>;
}
"#,
    )
    .unwrap();

    (project, nm_handle)
}

fn write_broad_root_alias_only_fixture(ws_root: &Path) -> (PathBuf, tempfile::TempDir) {
    fs::write(
        ws_root.join("pnpm-workspace.yaml"),
        "packages:\n  - '.'\n  - 'styleguide'\n",
    )
    .unwrap();
    fs::write(
        ws_root.join("package.json"),
        r#"{ "name": "workspace-root", "private": true }"#,
    )
    .unwrap();
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, ws_root.join("node_modules"))
        .expect("symlink workspace node_modules");

    let project = ws_root.join("styleguide");
    fs::create_dir_all(project.join("pages")).unwrap();
    fs::write(
        project.join("package.json"),
        r#"{ "name": "styleguide", "private": true }"#,
    )
    .unwrap();
    fs::write(
        project.join("zfb.config.json"),
        r#"{ "framework": "preact" }"#,
    )
    .unwrap();
    fs::write(
        project.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@/*": ["../*"] }
  }
}
"#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"import { RootCard } from "@/root-card";

export default function Home() {
  return <main><RootCard /></main>;
}
"#,
    )
    .unwrap();
    fs::write(
        ws_root.join("root-card.tsx"),
        r#"import styles from "./root-card.module.css";

export function RootCard() {
  return <section class={styles.rootCard}>ROOT_CARD_MARKER</section>;
}
"#,
    )
    .unwrap();
    fs::write(
        ws_root.join("root-card.module.css"),
        ".rootCard { color: #456def; }\n",
    )
    .unwrap();

    (project, nm_handle)
}

#[test]
fn broad_root_alias_only_discovers_root_package_css_module_class_map() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[sibling_css_module_command_layer_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, _nm_handle) = write_broad_root_alias_only_fixture(tmp.path());
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&project)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build`");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected `zfb build` to succeed; got status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );

    let html_blob = collect_files(&project.join("dist"), "html")
        .iter()
        .map(|path| fs::read_to_string(path).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        html_blob.contains("_rootCard"),
        "expected the root-package CSS Module's scoped class in emitted HTML.\n--- html ---\n{}",
        truncate(&html_blob, 2_000),
    );
    assert!(
        !html_blob.contains("class=\"undefined\""),
        "root-package CSS Module must not render an undefined class.\n--- html ---\n{}",
        truncate(&html_blob, 2_000),
    );
}

/// Issue #1886 (epic #1883): the real command path must support a nested host
/// reaching concrete root-package TS/TSX/CSS/JSON source through a broad root
/// alias, alongside a narrower root component alias and a sibling package
/// alias. This is deliberately in the existing serialized build-command
/// binary: it spawns `zfb build`, boots V8, and verifies fresh `dist/` bytes
/// rather than the lower-level bundler's prepared output.
#[test]
fn workspace_root_alias_graph_builds_through_real_zfb_command() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[sibling_css_module_command_layer_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, _nm_handle) = write_workspace_root_alias_fixture(tmp.path());
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&project)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected `zfb build` to succeed for the claimed workspace-root alias \
         graph; got status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );

    let dist = project.join("dist");
    let html_paths = collect_files(&dist, "html");
    assert!(
        !html_paths.is_empty(),
        "no fresh HTML files emitted under dist/; expected at least dist/index.html"
    );
    let html_blob = html_paths
        .iter()
        .map(|path| fs::read_to_string(path).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    for marker in [
        "ROOT_COMPONENT_MARKER",
        "ROOT_SRC_MARKER",
        "ROOT_LIB_MARKER",
        "ROOT_JSON_MARKER",
        "SIBLING_PACKAGE_MARKER",
    ] {
        assert!(
            html_blob.contains(marker),
            "fresh emitted HTML must contain {marker}; this proves the root and \
             sibling source graph participated in the spawned command.\n--- html ---\n{}",
            truncate(&html_blob, 2_000),
        );
    }
    assert!(
        html_blob.contains("_rootAliasStyle"),
        "fresh emitted HTML must contain the root component's scoped CSS Module class; \
         the raw CSS marker alone does not prove the class map participated.\n--- html ---\n{}",
        truncate(&html_blob, 2_000),
    );

    let css_blob = collect_files(&dist.join("assets"), "css")
        .iter()
        .map(|path| fs::read_to_string(path).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        css_blob.contains("123abc"),
        "fresh emitted CSS must contain the root package stylesheet marker; \
         root TSX participation alone would not prove root CSS was staged.\n--- css ---\n{}",
        truncate(&css_blob, 2_000),
    );
}

/// A pnpm workspace whose `sub-packages/vhost` member reaches a sibling
/// component (`<ws_root>/lib/vshared/Panel.tsx`) ONLY through a registered
/// plugin virtual module (`virtual:panel`, via `addVirtualModule` in
/// `preset.mjs`) — deliberately NO tsconfig/plugin alias targets
/// `lib/vshared` at all. Issue #1775: the sibling's `Panel.module.css`
/// class map can only be produced by the real command-layer scan
/// discovering the sibling through the virtual-module preprocessing graph
/// (claim source c), since nothing under `project_root` references
/// `Panel.module.css` directly and no alias claims the sibling's
/// directory.
fn write_virtual_only_fixture(ws_root: &Path) -> (PathBuf, tempfile::TempDir) {
    fs::write(
        ws_root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'sub-packages/*'\n",
    )
    .unwrap();
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, ws_root.join("node_modules"))
        .expect("symlink workspace node_modules");

    let project = ws_root.join("sub-packages/vhost");
    fs::create_dir_all(project.join("pages")).unwrap();

    // No `tailwind` key -> CSS enabled by default. No tsconfig.json at all —
    // the sibling below is reached ONLY via the registered virtual module.
    fs::write(
        project.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "plugins": [{ "name": "./preset.mjs" }]
}
"#,
    )
    .unwrap();

    fs::write(
        project.join("preset.mjs"),
        r#"import path from "node:path";

export default {
  name: "virtual-panel-preset",
  setup({ projectRoot, addVirtualModule }) {
    const sibling = path.join(projectRoot, "../../lib/vshared/Panel.tsx");
    addVirtualModule(
      "virtual:panel",
      () => `export { default } from ${JSON.stringify(sibling)};\n`,
    );
  },
};
"#,
    )
    .unwrap();

    fs::write(
        project.join("pages/index.tsx"),
        r#"import Panel from "virtual:panel";

export default function HomePage() {
  return (
    <main>
      <Panel />
    </main>
  );
}
"#,
    )
    .unwrap();

    let sibling = ws_root.join("lib/vshared");
    fs::create_dir_all(&sibling).unwrap();
    fs::write(
        sibling.join("Panel.module.css"),
        ".box { color: #303030; }\n.label { color: #404040; }\n",
    )
    .unwrap();
    fs::write(
        sibling.join("Panel.tsx"),
        r#"import styles from "./Panel.module.css";

export default function Panel() {
  return (
    <section class={styles.box}>
      <span class={styles.label}>panel</span>
    </section>
  );
}
"#,
    )
    .unwrap();

    (project, nm_handle)
}

/// Issue #1775: a workspace-sibling `.module.css` reached ONLY through a
/// registered plugin virtual module (no direct tsconfig/plugin alias) gets
/// a REAL, command-layer-computed class map that reaches both the emitted
/// HTML and the emitted stylesheet — proving `discover_css_source_files` /
/// `compute_css_module_class_maps` thread the virtual-module claim sources
/// (a+c), not just the alias claim source (b) the sibling test above
/// already covers.
#[test]
fn sibling_module_css_reached_only_via_virtual_module_is_discovered_and_emitted() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[sibling_css_module_command_layer_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, _nm_handle) = write_virtual_only_fixture(tmp.path());

    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&project)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected `zfb build` to succeed for a sibling reached only via a \
         registered virtual module; got status={:?}\n--- stdout ---\n{stdout}\n\
         --- stderr ---\n{stderr}",
        output.status,
    );

    let dist = project.join("dist");
    let html_paths = collect_files(&dist, "html");
    assert!(
        !html_paths.is_empty(),
        "no HTML files emitted under dist/; expected at least dist/index.html"
    );
    let html_blob = html_paths
        .iter()
        .map(|p| fs::read_to_string(p).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !html_blob.contains("class=\"box\""),
        "the sibling's class must be SCOPED, not the raw `box` name — a raw \
         class here means the virtual-module sibling's class map was never \
         discovered and the bundler fell back to the `export default {{}}` \
         shim.\n--- html ---\n{}",
        truncate(&html_blob, 1200)
    );
    for local in ["box", "label"] {
        let needle = format!("_{local}");
        assert!(
            html_blob.contains(&needle),
            "expected hashed class containing `{needle}` in emitted HTML — a \
             raw `{local}` class (or a missing one) would mean the \
             virtual-module sibling's class map was never discovered by the \
             command layer.\n--- html ---\n{}",
            truncate(&html_blob, 1200)
        );
    }

    let assets_dir = dist.join("assets");
    let css_paths = collect_files(&assets_dir, "css");
    let styles_css = css_paths
        .iter()
        .find(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.starts_with("styles-") && n.ends_with(".css"))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            panic!("expected dist/assets/styles-<hash>.css to be emitted; got: {css_paths:#?}")
        });
    let css_body = fs::read_to_string(styles_css).unwrap();
    for local in ["box", "label"] {
        let needle = format!("_{local}");
        assert!(
            css_body.contains(&needle),
            "expected scoped selector containing `{needle}` in {}; the \
             virtual-module sibling's scoped CSS must reach the emitted \
             stylesheet, not just the class-map shim.\n--- css ---\n{}",
            styles_css.display(),
            truncate(&css_body, 1200),
        );
    }
}

/// A pnpm workspace whose `sub-packages/dhost` member reaches a sibling
/// `.module.css` (`<ws_root>/lib/vdirect/Direct.module.css`) ONLY through a
/// registered plugin virtual module whose source imports the CSS **directly**
/// — deliberately NO intermediate `.tsx` component under `lib/vdirect` that
/// imports it, and no tsconfig/plugin alias. Issue #1775 follow-up: the
/// sibling's class map can only be produced when the command-layer scan folds
/// the directly-imported `.module.css` in from the virtual-module discovery
/// graph (`discovered_direct_css_modules`); before the fix,
/// `discover_css_source_files` returned only JS/TS/MD sources and the
/// in-memory virtual source was never scanned, so the module dropped out of
/// both the class map and CSS emission while the bundler rewrote the staged
/// file to `export default {}` — a green build with missing styles.
fn write_direct_virtual_css_fixture(ws_root: &Path) -> (PathBuf, tempfile::TempDir) {
    fs::write(
        ws_root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'sub-packages/*'\n",
    )
    .unwrap();
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, ws_root.join("node_modules"))
        .expect("symlink workspace node_modules");

    let project = ws_root.join("sub-packages/dhost");
    fs::create_dir_all(project.join("pages")).unwrap();

    fs::write(
        project.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "plugins": [{ "name": "./preset.mjs" }]
}
"#,
    )
    .unwrap();

    // The virtual module's source imports the sibling `.module.css` DIRECTLY
    // and re-exports its class map — no intermediate component.
    fs::write(
        project.join("preset.mjs"),
        r#"import path from "node:path";

export default {
  name: "virtual-direct-css-preset",
  setup({ projectRoot, addVirtualModule }) {
    const siblingCss = path.join(projectRoot, "../../lib/vdirect/Direct.module.css");
    addVirtualModule(
      "virtual:styles",
      () => `import styles from ${JSON.stringify(siblingCss)};\nexport default styles;\n`,
    );
  },
};
"#,
    )
    .unwrap();

    fs::write(
        project.join("pages/index.tsx"),
        r#"import styles from "virtual:styles";

export default function HomePage() {
  return (
    <main>
      <section class={styles.box}>
        <span class={styles.label}>direct</span>
      </section>
    </main>
  );
}
"#,
    )
    .unwrap();

    // ONLY the `.module.css` under the sibling dir — no `.tsx` importing it,
    // so the mirror-root scan-source walk cannot reach it; only the direct
    // virtual→CSS discovery path can.
    let sibling = ws_root.join("lib/vdirect");
    fs::create_dir_all(&sibling).unwrap();
    fs::write(
        sibling.join("Direct.module.css"),
        ".box { color: #505050; }\n.label { color: #606060; }\n",
    )
    .unwrap();

    (project, nm_handle)
}

/// Issue #1775 follow-up: a workspace-sibling `.module.css` imported DIRECTLY
/// by a registered virtual module (no intermediate component, no alias) gets
/// a REAL, command-layer-computed class map that reaches both the emitted
/// HTML and the emitted stylesheet — proving `discovered_direct_css_modules`
/// folds the directly-imported CSS into `compute_css_module_class_maps` /
/// `run_css_emitter`, closing the silent-missing-styles gap the indirect
/// (component-mediated) virtual-module test above did not cover.
#[test]
fn sibling_module_css_imported_directly_by_virtual_module_is_discovered_and_emitted() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[sibling_css_module_command_layer_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, _nm_handle) = write_direct_virtual_css_fixture(tmp.path());

    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&project)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected `zfb build` to succeed for a sibling `.module.css` imported \
         directly by a registered virtual module; got status={:?}\n\
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );

    let dist = project.join("dist");
    let html_paths = collect_files(&dist, "html");
    assert!(
        !html_paths.is_empty(),
        "no HTML files emitted under dist/; expected at least dist/index.html"
    );
    let html_blob = html_paths
        .iter()
        .map(|p| fs::read_to_string(p).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !html_blob.contains("class=\"box\""),
        "the sibling's class must be SCOPED, not the raw `box` name — a raw \
         class here means the direct virtual→CSS class map was never \
         discovered and the bundler fell back to the `export default {{}}` \
         shim.\n--- html ---\n{}",
        truncate(&html_blob, 1200)
    );
    for local in ["box", "label"] {
        let needle = format!("_{local}");
        assert!(
            html_blob.contains(&needle),
            "expected hashed class containing `{needle}` in emitted HTML — a \
             raw `{local}` class (or a missing one) would mean the direct \
             virtual→CSS class map was never discovered by the command \
             layer.\n--- html ---\n{}",
            truncate(&html_blob, 1200)
        );
    }

    let assets_dir = dist.join("assets");
    let css_paths = collect_files(&assets_dir, "css");
    let styles_css = css_paths
        .iter()
        .find(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.starts_with("styles-") && n.ends_with(".css"))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            panic!("expected dist/assets/styles-<hash>.css to be emitted; got: {css_paths:#?}")
        });
    let css_body = fs::read_to_string(styles_css).unwrap();
    for local in ["box", "label"] {
        let needle = format!("_{local}");
        assert!(
            css_body.contains(&needle),
            "expected scoped selector containing `{needle}` in {}; the direct \
             virtual→CSS module's scoped rules must reach the emitted \
             stylesheet, not just the class-map shim.\n--- css ---\n{}",
            styles_css.display(),
            truncate(&css_body, 1200),
        );
    }
}

/// A pnpm workspace whose `sub-packages/uhost` member reaches a sibling
/// component (`<ws_root>/lib/ushared/Badge.tsx`) through a wildcard
/// tsconfig alias, exactly like `write_fixture` above — but the sibling's
/// only reference to a Tailwind utility class is an arbitrary-value class
/// (`bg-[#1a2b3c]`) that appears NOWHERE under `project_root` itself.
/// Issue #1776: proving this utility's compiled CSS actually ships requires
/// the sibling's `SiblingMirrorPlan` mirror root to be fed into Tailwind's
/// own `@source` content-glob scan (`assemble_css_content_globs` in
/// `crates/zfb/src/commands/build.rs`), not just the CSS Modules discovery
/// walk `write_fixture`'s test already covers.
fn write_utility_only_sibling_fixture(ws_root: &Path) -> (PathBuf, tempfile::TempDir) {
    fs::write(
        ws_root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'sub-packages/*'\n",
    )
    .unwrap();
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, ws_root.join("node_modules"))
        .expect("symlink workspace node_modules");

    let project = ws_root.join("sub-packages/uhost");
    fs::create_dir_all(project.join("pages")).unwrap();

    // A real project's `.gitignore` excludes zfb's own build artifacts
    // (`.zfb-build/`, `dist/`). Tailwind v4's default automatic content
    // detection (active here since the synthesised entry never adds
    // `source(none)`) respects `.gitignore` — WITHOUT this file it would
    // find the sibling-only `bg-[#1a2b3c]` utility through the compiled
    // `.zfb-build/bundle.mjs` (esbuild inlines the JSX class string
    // literally), masking whether the `@source` mirror-root wiring under
    // test actually did anything. This file removes that confound so the
    // assertion below is a real proof, not a false positive.
    fs::write(
        project.join(".gitignore"),
        ".zfb-build/\ndist/\nnode_modules/\n",
    )
    .unwrap();

    // No `tailwind` key -> CSS (and hence the Tailwind utility scan) is
    // enabled by default.
    fs::write(
        project.join("zfb.config.json"),
        r#"{
  "framework": "preact"
}
"#,
    )
    .unwrap();

    fs::write(
        project.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@ushared/*": ["../../lib/ushared/*"] }
  }
}
"#,
    )
    .unwrap();

    fs::write(
        project.join("pages/index.tsx"),
        r#"import Badge from "@ushared/Badge";

export default function HomePage() {
  return (
    <main>
      <Badge />
    </main>
  );
}
"#,
    )
    .unwrap();

    let sibling = ws_root.join("lib/ushared");
    fs::create_dir_all(&sibling).unwrap();
    // The `bg-[#1a2b3c]` arbitrary-value utility is used ONLY here — no
    // file under `project_root` references it, so it can only reach the
    // emitted stylesheet if Tailwind's `@source` scan walks this sibling
    // mirror root.
    fs::write(
        sibling.join("Badge.tsx"),
        r#"export default function Badge() {
  return <span class="bg-[#1a2b3c]">badge</span>;
}
"#,
    )
    .unwrap();

    (project, nm_handle)
}

/// Issue #1776: a Tailwind utility class used ONLY inside a sibling file
/// reached through a claimed tsconfig alias (no direct project-root
/// reference at all) is scanned and emitted in the real `zfb build`
/// stylesheet — proving `SiblingMirrorPlan` mirror roots feed
/// `TailwindSubprocessConfig::content_globs` / the engine's `@source`
/// directives, not just the CSS Modules source walk.
#[test]
#[ignore = "env-gate: tailwindcss v4 binary — cargo test -p zfb --test \
            sibling_css_module_command_layer_build -- --ignored \
            (ZFB_TAILWIND_BIN or the staged crates/zfb/binaries/tailwindcss-v4 \
            slot; also needs ZFB_ESBUILD_BIN or an esbuild on PATH)"]
fn sibling_only_utility_class_reaches_tailwind_source_scan_and_is_emitted() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[sibling_css_module_command_layer_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, _nm_handle) = write_utility_only_sibling_fixture(tmp.path());

    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&project)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected `zfb build` to succeed for a sibling-only Tailwind utility \
         class; got status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );

    let dist = project.join("dist");
    let assets_dir = dist.join("assets");
    let css_paths = collect_files(&assets_dir, "css");
    let styles_css = css_paths
        .iter()
        .find(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.starts_with("styles-") && n.ends_with(".css"))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            panic!("expected dist/assets/styles-<hash>.css to be emitted; got: {css_paths:#?}")
        });
    let css_body = fs::read_to_string(styles_css).unwrap();
    assert!(
        css_body.contains("1a2b3c"),
        "expected the sibling-only `bg-[#1a2b3c]` utility's compiled bytes in \
         {}; a missing hex value means the sibling mirror root never reached \
         Tailwind's `@source` content-glob scan.\n--- css ---\n{}",
        styles_css.display(),
        truncate(&css_body, 1200),
    );
}

/// Issue #1803 (epic #1799 gap b): extends `write_utility_only_sibling_fixture`
/// with an UNGITIGNORED `dist/`-style generated directory inside the SAME
/// sibling mirror root (`lib/ushared/dist/`), carrying its own
/// arbitrary-value utility class (`bg-[#9f8e7d]`) that appears NOWHERE else
/// in the fixture. Real generated-output directories are typically
/// build-tool-emitted, not authored source — `discover_css_source_files`
/// already skips `CSS_SIBLING_MIRROR_SKIP_DIRS` infra dirs (including
/// `dist`) when it wholesale-walks a mirror root for CSS Modules discovery,
/// but (pre-#1803) Tailwind's own `@source` content-glob scan had no
/// equivalent exclusion and would see the mirror root wholesale, leaking
/// this class into the emitted stylesheet.
///
/// The sibling itself carries no `.gitignore` — only the PROJECT's
/// `.gitignore` (written by `write_utility_only_sibling_fixture`, covering
/// `.zfb-build/`/`dist/`/`node_modules/` INSIDE THE PROJECT) exists, and
/// that file governs the project dir, not the workspace-sibling mirror
/// root under `ws_root/lib/`. So Tailwind's `.gitignore`-respecting
/// automatic content detection does not save this test from the leak on
/// its own — verified empirically (a throwaway `tailwindcss` run against
/// an ungitignored sibling `dist/` leaks its class; the same run with a
/// `.gitignore` covering that `dist/` does not) — the exclusion must come
/// from `negative_source_globs`.
fn write_utility_only_sibling_fixture_with_generated_dir_leak(
    ws_root: &Path,
) -> (PathBuf, tempfile::TempDir) {
    let (project, nm_handle) = write_utility_only_sibling_fixture(ws_root);

    let generated_dir = ws_root.join("lib/ushared/dist");
    fs::create_dir_all(&generated_dir).unwrap();
    // A class string that appears NOWHERE else in the fixture — its
    // presence in the emitted stylesheet would mean the generated `dist/`
    // subtree leaked into Tailwind's content scan.
    fs::write(
        generated_dir.join("Generated.tsx"),
        r#"export default function Generated() {
  return <span class="bg-[#9f8e7d]">generated</span>;
}
"#,
    )
    .unwrap();

    (project, nm_handle)
}

/// Issue #1803 (epic #1799 gap b): a generated-looking `dist/` subtree
/// INSIDE a sibling mirror root must NOT leak its class strings into
/// Tailwind's content scan, while the sibling's legitimate utility class
/// (outside that subtree, already proven reachable by
/// `sibling_only_utility_class_reaches_tailwind_source_scan_and_is_emitted`
/// above) MUST still reach the emitted stylesheet — proving selective
/// exclusion rather than a change that broke sibling scanning wholesale
/// (which would also make the leak class absent, but for the wrong
/// reason).
///
/// ## Regression criterion
///
/// Verified to FAIL (the leak class `9f8e7d` present in the emitted
/// stylesheet) with the `negative_source_globs` wiring in
/// `assemble_css_content_globs` / `build_default_css_payload`
/// (`crates/zfb/src/commands/build.rs`) reverted, and PASS with it in
/// place — the #1776 cherry-pick-out precedent; see the PR/issue
/// description for both run transcripts.
#[test]
#[ignore = "env-gate: tailwindcss v4 binary — cargo test -p zfb --test \
            sibling_css_module_command_layer_build -- --ignored \
            (ZFB_TAILWIND_BIN or the staged crates/zfb/binaries/tailwindcss-v4 \
            slot; also needs ZFB_ESBUILD_BIN or an esbuild on PATH)"]
fn sibling_generated_dir_utility_class_is_excluded_from_tailwind_source_scan() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[sibling_css_module_command_layer_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, _nm_handle) =
        write_utility_only_sibling_fixture_with_generated_dir_leak(tmp.path());

    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&project)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected `zfb build` to succeed for the sibling generated-dir leak \
         fixture; got status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );

    let dist = project.join("dist");
    let assets_dir = dist.join("assets");
    let css_paths = collect_files(&assets_dir, "css");
    let styles_css = css_paths
        .iter()
        .find(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.starts_with("styles-") && n.ends_with(".css"))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            panic!("expected dist/assets/styles-<hash>.css to be emitted; got: {css_paths:#?}")
        });
    let css_body = fs::read_to_string(styles_css).unwrap();

    assert!(
        !css_body.contains("9f8e7d"),
        "expected the sibling generated-dir (`lib/ushared/dist/`) utility class \
         to be EXCLUDED from {}; its presence means the mirror root's `dist/` \
         subtree leaked into Tailwind's `@source` content-glob scan.\n--- css ---\n{}",
        styles_css.display(),
        truncate(&css_body, 1200),
    );
    assert!(
        css_body.contains("1a2b3c"),
        "expected the sibling's legitimate `bg-[#1a2b3c]` utility (outside the \
         generated dir) to REMAIN present in {} — its absence would mean \
         sibling scanning broke wholesale rather than the generated subtree \
         being selectively excluded.\n--- css ---\n{}",
        styles_css.display(),
        truncate(&css_body, 1200),
    );
}
