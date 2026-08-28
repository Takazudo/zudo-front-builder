//! Level-3 build-output tests for package-owned routes (#1193, epic #1191).
//!
//! A preset plugin's `setup()` calls `injectRoute(pattern, entrypoint, opts?)`
//! during a **build**. As of #1193 the build ACCEPTS those routes (the
//! dev-only guard was lifted), materialises a synthesized module into a
//! per-build overlay pages root, and prerenders the route through the normal
//! scan → bundle → render pipeline.
//!
//! These tests drive the real `zfb` binary against on-disk fixtures — the
//! exact path a downstream consumer (zudo-doc's preset) hits. They mirror
//! the fixture/node_modules wiring of `client_router_autoinclude_build.rs`:
//! `node_modules/` is symlinked to the
//! binary-embedded `@takazudo` tree, and a local `.mjs` preset is referenced
//! via `{"plugins":[{"name":"./preset.mjs"}]}`.
//!
//! Unix-only because the embedded-tree symlink wiring is the established Unix
//! pattern in the sibling build tests.
//!
//! ## What is proven here (vs the `package_routes` unit tests)
//!
//! The unit tests (`crates/zfb/src/commands/package_routes.rs`) prove the
//! materialiser logic in isolation (pattern→path, overlay write, pre-scan
//! drop). These end-to-end tests prove the full build path: the lifted
//! build-mode guard, the overlay scan+bundle, the prerendered HTML, the
//! import-path fix for nested routes, `output: static` SSR rejection, and
//! the islands overlay seed.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use zfb_test_utils::{locate_esbuild, zfb_binary};

/// `true` when `node` is on PATH (the plugin host needs it).
fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Symlink `node_modules/` to the extracted embedded `@takazudo` tree so
/// `@takazudo/zfb-runtime` + JSX runtime resolve. Returns the TempDir handle
/// that must outlive the build.
fn link_embedded_node_modules(root: &Path) -> tempfile::TempDir {
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, root.join("node_modules"))
        .expect("symlink node_modules");
    nm_handle
}

/// Copy the embedded runtime packages into a real workspace-level
/// `node_modules` so both a consumer app and its linked sibling package can
/// resolve the same live dependency graph in the regression fixture.
fn materialize_embedded_node_modules(root: &Path) {
    let (_nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    copy_dir(&embedded_nm_path, &root.join("node_modules"));
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create_dir_all");
    for entry in fs::read_dir(src).expect("read_dir").flatten() {
        let ty = entry.file_type().expect("file_type");
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &dst_path);
        } else {
            fs::copy(entry.path(), &dst_path).expect("copy file");
        }
    }
}

/// Run `zfb build` in `root` with the supplied esbuild binary.
fn run_zfb_build(root: &Path, esbuild: &Path) -> std::process::Output {
    Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn `zfb build`")
}

/// `true` when the non-zero build is a known-skip (no embedded V8 / no esbuild),
/// matching the skip pattern in the sibling build tests.
fn is_known_skip(combined: &str) -> bool {
    combined.contains("embed_v8") || combined.contains("no esbuild")
}

/// Recursively collect files under `dir` with the given extension.
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

/// A minimal page component module body. `marker` is a unique string baked
/// into the rendered HTML so a test can assert the page actually rendered.
fn page_module(marker: &str) -> String {
    format!(
        r#"export default function Page() {{
  return (
    <html lang="en">
      <head><title>{marker}</title></head>
      <body><p>{marker}</p></body>
    </html>
  );
}}
"#
    )
}

/// A skip-or-build helper: returns `Some(dist)` on a successful build, or
/// `None` (after printing why) when the environment can't run the build.
fn build_or_skip(root: &Path, esbuild: &Path, test: &str) -> Option<PathBuf> {
    let output = run_zfb_build(root, esbuild);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    if !output.status.success() && is_known_skip(&combined) {
        eprintln!("[{test}] known-skip indicator; skipping.\nstdout: {stdout}\nstderr: {stderr}");
        return None;
    }
    assert!(
        output.status.success(),
        "[{test}] expected `zfb build` to succeed; status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );
    Some(root.join("dist"))
}

// ---------------------------------------------------------------------------
// 1. Static package route prerenders; user pages/ untouched.
// ---------------------------------------------------------------------------

/// A preset injecting a static `/preset-page` route prerenders
/// `dist/preset-page/index.html` with the page marker, while the project's
/// own `pages/index.tsx` still renders `dist/index.html` unchanged.
#[test]
fn static_package_route_prerenders_alongside_user_pages() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[static_package_route] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[static_package_route] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    // The package's page component lives outside pages/ (its own dir).
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(
        root.join("pkg/preset-page.tsx"),
        page_module("PRESET_PAGE_MARKER"),
    )
    .unwrap();

    // The preset injects the static route during setup.
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "routes-preset",
  setup({ injectRoute }) {
    injectRoute("/preset-page", "./pkg/preset-page.tsx");
  },
};
"#,
    )
    .unwrap();

    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();

    // The project's own home page.
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        page_module("USER_HOME_MARKER"),
    )
    .unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "static_package_route") else {
        return;
    };

    // Package route prerendered.
    let preset_html = dist.join("preset-page/index.html");
    assert!(
        preset_html.is_file(),
        "expected dist/preset-page/index.html; dist html: {:#?}",
        collect_files(&dist, "html")
    );
    let preset_body = fs::read_to_string(&preset_html).unwrap();
    assert!(
        preset_body.contains("PRESET_PAGE_MARKER"),
        "package route HTML must contain the page marker; got: {preset_body}"
    );

    // User home page still rendered.
    let home_html = dist.join("index.html");
    assert!(
        home_html.is_file(),
        "expected dist/index.html for the user's pages/index.tsx"
    );
    let home_body = fs::read_to_string(&home_html).unwrap();
    assert!(
        home_body.contains("USER_HOME_MARKER"),
        "user home HTML must contain its marker; got: {home_body}"
    );
}

/// Regression guard (codex P2): a USER page that imports a sibling
/// component via a project-relative path must still resolve when a package
/// route is also present. The overlay must not relocate user pages such
/// that `../components/Box` no longer reaches the real project file.
#[test]
fn user_page_relative_import_resolves_with_package_route_present() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[user_rel_import] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[user_rel_import] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    // A real project component the user page imports relatively.
    fs::create_dir_all(root.join("components")).unwrap();
    fs::write(
        root.join("components/box.tsx"),
        "export function Box() { return <div>BOX_FROM_COMPONENT</div>; }\n",
    )
    .unwrap();
    // User home page imports the sibling component via `../components/box`.
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"import { Box } from "../components/box";
export default function Page() {
  return (
    <html lang="en">
      <head><title>home</title></head>
      <body><Box /></body>
    </html>
  );
}
"#,
    )
    .unwrap();

    // An unrelated package route, present so the overlay is materialised.
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/preset.tsx"), page_module("PRESET")).unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "rel-user-preset",
  setup({ injectRoute }) {
    injectRoute("/preset-page", "./pkg/preset.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "user_rel_import") else {
        return;
    };

    let home = dist.join("index.html");
    assert!(home.is_file(), "expected dist/index.html");
    let body = fs::read_to_string(&home).unwrap();
    assert!(
        body.contains("BOX_FROM_COMPONENT"),
        "the user page's relative `../components/box` import must resolve even with a \
         package route present (overlay must not strand user-page relative imports); got: {body}"
    );
}

/// A user page copied into the package-route overlay keeps project dependency
/// resolution when a non-empty `bundle.exclude` removes the live shadow
/// `node_modules` link. The page's bare import must seed the staged dependency
/// view even though the copied source file physically lives outside the project
/// root (#1645 release follow-up).
#[test]
fn user_page_bare_import_is_staged_from_package_route_overlay() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[overlay_bare_import] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[overlay_bare_import] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"import { slugify } from "@takazudo/zfb/slugify";
export default function Page() {
  const marker = slugify("OVERLAY BARE IMPORT STAGED");
  return <html lang="en"><body><p>{marker}</p></body></html>;
}
"#,
    )
    .unwrap();

    // An unrelated package route forces the user page into the external
    // per-build overlay.
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/preset.tsx"), page_module("PRESET")).unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "overlay-bare-import-preset",
  setup({ injectRoute }) {
    injectRoute("/preset-page", "./pkg/preset.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "plugins": [{ "name": "./preset.mjs" }],
  "bundle": { "exclude": ["does-not-exist/**"] }
}
"#,
    )
    .unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "overlay_bare_import") else {
        return;
    };
    let home = dist.join("index.html");
    assert!(home.is_file(), "expected dist/index.html");
    let body = fs::read_to_string(&home).unwrap();
    assert!(
        body.contains("overlay-bare-import-staged"),
        "the overlay user page's project dependency must resolve from the staged view; got: {body}"
    );
}

/// Workspace-linked package routes must execute against the same staged Preact
/// singleton as the generated renderer when `bundle.exclude` disables the live
/// shadow `node_modules` link. The direct route covers issue #1650. The second
/// route reaches the same hook component through a virtual module that
/// absolutely re-exports an in-project host binding, covering issue #1652.
#[test]
fn workspace_package_routes_and_virtual_host_hooks_share_staged_preact_identity() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[workspace_route_hooks] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[workspace_route_hooks] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace = tmp.path();
    materialize_embedded_node_modules(workspace);

    let root = workspace.join("apps/site");
    fs::create_dir_all(root.join("pages")).unwrap();
    std::os::unix::fs::symlink(workspace.join("node_modules"), root.join("node_modules"))
        .expect("link app node_modules to workspace dependencies");

    let package_root = workspace.join("packages/route-package");
    fs::create_dir_all(package_root.join("src")).unwrap();
    fs::write(
        package_root.join("package.json"),
        r#"{
  "name": "@fixture/route-package",
  "version": "0.0.0",
  "type": "module",
  "exports": {
    "./sidebar-toggle": "./src/sidebar-toggle.tsx"
  }
}
"#,
    )
    .unwrap();
    fs::write(
        package_root.join("src/sidebar-toggle.tsx"),
        r#"import { useState } from "preact/hooks";
export function SidebarToggle() {
  const [label] = useState("WORKSPACE_ROUTE_HOOK_SINGLETON");
  return <p>{label}</p>;
}
"#,
    )
    .unwrap();
    let entrypoint = package_root.join("src/page.tsx");
    fs::write(
        &entrypoint,
        r#"import { SidebarToggle } from "@fixture/route-package/sidebar-toggle";
export default function Page() {
  return <html lang="en"><body><SidebarToggle /></body></html>;
}
"#,
    )
    .unwrap();
    let virtual_entrypoint = package_root.join("src/virtual-page.tsx");
    fs::write(
        &virtual_entrypoint,
        r#"import { bindings } from "virtual:host-bindings";
export default function Page() {
  const SidebarToggle = bindings.SidebarToggle;
  return <html lang="en"><body><SidebarToggle /></body></html>;
}
"#,
    )
    .unwrap();

    let package_link = workspace.join("node_modules/@fixture/route-package");
    fs::create_dir_all(package_link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&package_root, &package_link)
        .expect("link workspace route package into node_modules");

    fs::create_dir_all(root.join("src")).unwrap();
    let host_bindings = root.join("src/host-bindings.tsx");
    fs::write(
        &host_bindings,
        r#"import { SidebarToggle } from "@fixture/route-package/sidebar-toggle";
export const bindings = { SidebarToggle };
"#,
    )
    .unwrap();

    let entrypoint_json = serde_json::to_string(&entrypoint.to_string_lossy()).unwrap();
    let virtual_entrypoint_json =
        serde_json::to_string(&virtual_entrypoint.to_string_lossy()).unwrap();
    let virtual_module_source = format!(
        "export {{ bindings }} from {};\n",
        serde_json::to_string(&host_bindings.to_string_lossy()).unwrap()
    );
    let virtual_module_source_json = serde_json::to_string(&virtual_module_source).unwrap();
    fs::write(
        root.join("preset.mjs"),
        format!(
            r#"const virtualModuleSource = {virtual_module_source_json};
export default {{
  name: "workspace-route-hooks",
  setup({{ injectRoute, addVirtualModule }}) {{
    addVirtualModule("virtual:host-bindings", () => virtualModuleSource);
    injectRoute("/package-hooks", {entrypoint_json});
    injectRoute("/virtual-host-hooks", {virtual_entrypoint_json});
  }},
}};
"#,
        ),
    )
    .unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        page_module("WORKSPACE_ROUTE_HOME"),
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "plugins": [{ "name": "./preset.mjs" }],
  "bundle": { "exclude": ["e2e/fixtures/**", "_temp-resource/**"] }
}
"#,
    )
    .unwrap();

    let Some(dist) = build_or_skip(&root, &esbuild, "workspace_route_hooks") else {
        return;
    };
    let body = fs::read_to_string(dist.join("package-hooks/index.html")).unwrap();
    assert!(
        body.contains("WORKSPACE_ROUTE_HOOK_SINGLETON"),
        "the linked package route hook must render through the shared staged Preact identity; got: {body}"
    );
    let body = fs::read_to_string(dist.join("virtual-host-hooks/index.html")).unwrap();
    assert!(
        body.contains("WORKSPACE_ROUTE_HOOK_SINGLETON"),
        "the virtual absolute host binding must render through the shared staged Preact identity; got: {body}"
    );
}

// ---------------------------------------------------------------------------
// 2. Root package routes and user-page precedence.
// ---------------------------------------------------------------------------

/// A project with NO `pages/` dir at all builds when a preset owns the root
/// `/` route, which prerenders to `dist/index.html`.
#[test]
fn empty_pages_with_root_package_route_builds() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[empty_pages_root] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[empty_pages_root] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/home.tsx"), page_module("PKG_ROOT_MARKER")).unwrap();

    // Root injection is accepted in both build and dev; with no user index it
    // materializes as the package-owned root page.
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "root-preset",
  setup({ injectRoute }) {
    injectRoute("/", "./pkg/home.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();
    // NOTE: intentionally NO pages/ dir.

    let Some(dist) = build_or_skip(root, &esbuild, "empty_pages_root") else {
        return;
    };

    let index_html = dist.join("index.html");
    assert!(
        index_html.is_file(),
        "a `/` package route with no user pages/ must prerender dist/index.html; dist html: {:#?}",
        collect_files(&dist, "html")
    );
    let body = fs::read_to_string(&index_html).unwrap();
    assert!(
        body.contains("PKG_ROOT_MARKER"),
        "root package route HTML must contain the marker; got: {body}"
    );
}

/// A user `pages/index.tsx` wins over an injected `/` in the build route
/// selection, so the package root cannot replace the project's home page.
#[test]
fn user_index_wins_over_root_package_route_in_build() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[user_index_root_precedence] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[user_index_root_precedence] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        page_module("USER_ROOT_MARKER"),
    )
    .unwrap();
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/home.tsx"), page_module("PKG_ROOT_MARKER")).unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "root-precedence-preset",
  setup({ injectRoute }) {
    injectRoute("/", "./pkg/home.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "user_index_root_precedence") else {
        return;
    };

    let index_html = dist.join("index.html");
    assert!(index_html.is_file(), "expected user dist/index.html");
    let body = fs::read_to_string(&index_html).unwrap();
    assert!(
        body.contains("USER_ROOT_MARKER"),
        "user pages/index.tsx must win over injected `/`; got: {body}"
    );
    assert!(
        !body.contains("PKG_ROOT_MARKER"),
        "the injected root must not replace the user page; got: {body}"
    );
}

// ---------------------------------------------------------------------------
// 3. Nested package route imports the correct module (import-path regression).
// ---------------------------------------------------------------------------

/// A nested package route `/a/b/c` must prerender at `dist/a/b/c/index.html`
/// with its marker — the regression guard for the `route_path_under_pages`
/// import-path fix. Before the fix, a nested overlay route collapsed to a
/// bare-filename import (`./pages/c.tsx`), producing a broken/missing page.
#[test]
fn nested_package_route_imports_correct_module() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[nested_package_route] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[nested_package_route] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/deep.tsx"), page_module("NESTED_ABC_MARKER")).unwrap();

    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "nested-preset",
  setup({ injectRoute }) {
    injectRoute("/a/b/c", "./pkg/deep.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME")).unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "nested_package_route") else {
        return;
    };

    let nested_html = dist.join("a/b/c/index.html");
    assert!(
        nested_html.is_file(),
        "nested package route /a/b/c must prerender dist/a/b/c/index.html (import-path \
         regression guard); dist html: {:#?}",
        collect_files(&dist, "html")
    );
    let body = fs::read_to_string(&nested_html).unwrap();
    assert!(
        body.contains("NESTED_ABC_MARKER"),
        "nested package route HTML must contain its marker — proves the correct module \
         was imported, not a collapsed bare filename; got: {body}"
    );
}

// ---------------------------------------------------------------------------
// 4. Package route with a relative import bundles & renders.
// ---------------------------------------------------------------------------

/// A package route whose entrypoint imports a sibling module must bundle the
/// sibling and render — proves esbuild resolves the entrypoint's own relative
/// imports correctly through the overlay.
#[test]
fn package_route_with_relative_import_bundles() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[relative_import] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[relative_import] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pkg")).unwrap();
    // Sibling module the entrypoint imports.
    fs::write(
        root.join("pkg/greeting.ts"),
        "export const GREETING = \"RELATIVE_IMPORT_MARKER\";\n",
    )
    .unwrap();
    fs::write(
        root.join("pkg/with-import.tsx"),
        r#"import { GREETING } from "./greeting";
export default function Page() {
  return (
    <html lang="en">
      <head><title>rel</title></head>
      <body><p>{GREETING}</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "rel-preset",
  setup({ injectRoute }) {
    injectRoute("/with-import", "./pkg/with-import.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME")).unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "relative_import") else {
        return;
    };

    let html = dist.join("with-import/index.html");
    assert!(html.is_file(), "expected dist/with-import/index.html");
    let body = fs::read_to_string(&html).unwrap();
    assert!(
        body.contains("RELATIVE_IMPORT_MARKER"),
        "package route's relative import must bundle & render; got: {body}"
    );
}

// ---------------------------------------------------------------------------
// 5. output: static rejects an SSR-shaped (prerender=false) package route.
// ---------------------------------------------------------------------------

/// With `output: "static"`, a package route declared `{ prerender: false }`
/// (SSR-shaped) must FAIL the build — proving the inlined `prerender = false`
/// is actually SEEN by the prerender-map / `resolve_v8_mode` gate, not
/// silently defaulted to SSG.
#[test]
fn output_static_rejects_ssr_shaped_package_route() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[static_rejects_ssr] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[static_rejects_ssr] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/ssr.tsx"), page_module("SSR_MARKER")).unwrap();

    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "ssr-preset",
  setup({ injectRoute }) {
    injectRoute("/ssr-page", "./pkg/ssr.tsx", { prerender: false });
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "output": "static", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME")).unwrap();

    let output = run_zfb_build(root, &esbuild);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    if !output.status.success() && is_known_skip(&combined) {
        eprintln!("[static_rejects_ssr] known-skip indicator; skipping.\n{combined}");
        return;
    }

    assert!(
        !output.status.success(),
        "`output: static` must REJECT a prerender=false package route; build succeeded.\n\
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    // The load-bearing proof: the `prerender = false` inlined into the
    // overlay module was SEEN (not silently defaulted to SSG), so the route
    // was classified SSR and rejected. With no adapter + `output: static`,
    // the SSR-detection gate fires (whichever of the two SSR gates is first
    // — `ensure_no_ssr_without_adapter` or the `resolve_v8_mode` static gate
    // — both require the SSR classification, which only happens because the
    // inlined `prerender = false` was extracted). The rejection therefore
    // names the package route and `prerender = false`.
    assert!(
        combined.contains("/ssr-page") && combined.contains("prerender = false"),
        "rejection must prove the inlined `prerender = false` was seen (SSR classification \
         of the package route `/ssr-page`), not a generic build failure; got:\n{combined}"
    );
}

// ---------------------------------------------------------------------------
// 6. User pages/ wins a collision (package route dropped pre-scan).
// ---------------------------------------------------------------------------

/// A package route colliding with a user `pages/` route — including a SHAPE
/// duplicate (`[id]` vs `[slug]`) — is dropped pre-scan (user wins). The build
/// must succeed (no `AmbiguousShape` hard error) and serve the user's page.
#[test]
fn user_pages_wins_collision_including_shape_dup() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[user_wins] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[user_wins] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    // User owns /blog/[id]. Literal-returning `export function paths()` so
    // the build can statically expand it (no runtime V8 paths() call):
    // `id` "x" → /blog/x.
    fs::create_dir_all(root.join("pages/blog")).unwrap();
    fs::write(root.join("pages/blog/[id].tsx"), {
        r#"export function paths() {
  return [{ params: { id: "x" } }];
}
export default function Page() {
  return (
    <html lang="en">
      <head><title>USER_BLOG</title></head>
      <body><p>USER_BLOG_MARKER</p></body>
    </html>
  );
}
"#
    })
    .unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME")).unwrap();

    // Package tries /blog/[slug] — SAME shape (`/blog/:*`) → must be dropped.
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/blog-slug.tsx"), {
        r#"export function paths() {
  return [{ params: { slug: "y" } }];
}
export default function Page() {
  return (
    <html lang="en">
      <head><title>PKG_BLOG</title></head>
      <body><p>PKG_BLOG_MARKER</p></body>
    </html>
  );
}
"#
    })
    .unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "collide-preset",
  setup({ injectRoute }) {
    injectRoute("/blog/[slug]", "./pkg/blog-slug.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "user_wins") else {
        return;
    };

    // The user's /blog/x must be the one that rendered (package dropped).
    let blog_html = dist.join("blog/x/index.html");
    assert!(
        blog_html.is_file(),
        "user's /blog/[id] (→ /blog/x) must prerender; dist html: {:#?}",
        collect_files(&dist, "html")
    );
    let body = fs::read_to_string(&blog_html).unwrap();
    assert!(
        body.contains("USER_BLOG_MARKER"),
        "the USER page must win the shape collision; got: {body}"
    );
    // The package's slug ("y") must NOT have rendered.
    assert!(
        !dist.join("blog/y/index.html").is_file(),
        "the package route must have been dropped pre-scan (no /blog/y)"
    );
}

// ---------------------------------------------------------------------------
// 7. Island: a "use client" package route emits the island asset.
// ---------------------------------------------------------------------------

/// A package route reachable to a `"use client"` component must emit the
/// islands asset — proves the islands scanner is seeded from the overlay
/// pages root (#1193), not the hardcoded `project_root/pages`.
#[test]
fn package_route_with_use_client_emits_island_asset() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[island] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[island] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pkg")).unwrap();
    // A "use client" island component. Kept SSR-render-safe (no hooks at
    // render time): the assertion here is that the island ASSET is emitted
    // (scanner found `"use client"` reachable from a package route), which
    // is independent of any client-side interactivity. A `data-island`
    // marker keeps the SSR output deterministic.
    fs::write(
        root.join("pkg/counter.tsx"),
        r#""use client";
export function Counter() {
  return <button type="button" data-island="counter">click</button>;
}
"#,
    )
    .unwrap();
    // The package page renders the island.
    fs::write(
        root.join("pkg/island-page.tsx"),
        r#"import { Counter } from "./counter";
export default function Page() {
  return (
    <html lang="en">
      <head><title>island</title></head>
      <body><Counter /></body>
    </html>
  );
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "island-preset",
  setup({ injectRoute }) {
    injectRoute("/island-page", "./pkg/island-page.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME")).unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "island") else {
        return;
    };

    let js_assets = collect_files(&dist.join("assets"), "js");
    let has_island = js_assets.iter().any(|p| {
        p.file_name()
            .and_then(|s| s.to_str())
            .map(|n| n.starts_with("islands-") && n.ends_with(".js"))
            .unwrap_or(false)
    });
    assert!(
        has_island,
        "a `\"use client\"` component reachable from a package route must emit \
         dist/assets/islands-<hash>.js — proves the islands scanner is seeded from the \
         overlay pages root. js assets: {js_assets:#?}"
    );
}

// ---------------------------------------------------------------------------
// 7b. Island regression (codex P1): a USER page's island reached via an
//     outside-`pages/` import must still ship when a package route is present.
// ---------------------------------------------------------------------------

/// Regression guard (codex P1): a USER `pages/` page whose `"use client"`
/// island lives OUTSIDE `pages/` (e.g. `../components/Widget`) must still be
/// discovered — and ship in the islands bundle — when ANY package route is
/// present.
///
/// Before the fix the islands scanner was seeded by walking the build pages
/// root, which is the OVERLAY temp dir when a package route exists. The
/// overlay copies only `pages/` (plus generated package modules); it has no
/// `components/`. The copied `pages/index.tsx` lives at `<temp>/pages/`, so
/// its `../components/Widget` import resolves to `<temp>/components/Widget`
/// → missing → the user's island is silently dropped from the production
/// bundle whenever a package route is present (no build error).
///
/// The fix seeds user pages from the REAL `project_root/pages` (so their
/// imports resolve against the real `components/`) while still discovering
/// package-route islands via each materialized route's real entrypoint. We
/// assert the islands asset is emitted AND contains the user `Widget`
/// island's marker name (the scanner-derived `data-zfb-island` value the
/// runtime registry keys on).
#[test]
fn user_page_island_outside_pages_ships_with_package_route_present() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[user_island] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[user_island] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    // A "use client" island component OUTSIDE pages/ — the case the overlay
    // strands. SSR-render-safe (no hooks at render time); the assertion is
    // that the island ships, independent of client interactivity.
    fs::create_dir_all(root.join("components")).unwrap();
    fs::write(
        root.join("components/widget.tsx"),
        r#""use client";
export function Widget() {
  return <button type="button" data-island="widget">UserWidget</button>;
}
"#,
    )
    .unwrap();
    // User home page imports the island via the outside-pages/ relative path.
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"import { Widget } from "../components/widget";
export default function Page() {
  return (
    <html lang="en">
      <head><title>home</title></head>
      <body><Widget /></body>
    </html>
  );
}
"#,
    )
    .unwrap();

    // An unrelated package route, present so the overlay is materialised.
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/preset.tsx"), page_module("PRESET")).unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "user-island-preset",
  setup({ injectRoute }) {
    injectRoute("/preset-page", "./pkg/preset.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "user_island") else {
        return;
    };

    let js_assets = collect_files(&dist.join("assets"), "js");
    let island_assets: Vec<PathBuf> = js_assets
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.starts_with("islands-") && n.ends_with(".js"))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    assert!(
        !island_assets.is_empty(),
        "a user page's `\"use client\"` island reached via `../components/widget` must emit \
         dist/assets/islands-<hash>.js even with a package route present. js assets: {js_assets:#?}"
    );
    // The user `Widget` island must actually be registered in the bundle —
    // proving it was discovered, not just that SOME island shipped. The
    // scanner-derived marker name (the component name) is emitted into the
    // bundle's `__zfb_register(...)` call.
    let any_has_widget = island_assets.iter().any(|p| {
        fs::read_to_string(p)
            .map(|s| s.contains("Widget"))
            .unwrap_or(false)
    });
    assert!(
        any_has_widget,
        "the user page's `Widget` island (imported from `../components/widget`, OUTSIDE pages/) \
         must be present in the islands bundle when a package route is present — the overlay seed \
         must not strand user-page islands. island assets: {island_assets:#?}"
    );
}

// ---------------------------------------------------------------------------
// 8. Parity: a build with NO package routes is unaffected (overlay bypassed).
// ---------------------------------------------------------------------------

/// A project with NO package routes builds exactly as before — the overlay
/// machinery is bypassed (`build_pages_root == project_root/pages`). This is
/// the byte-identical-parity guard: the same fixture built with no plugins
/// produces the expected dist output.
#[test]
fn no_package_routes_build_is_unaffected() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[parity] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[parity] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    // No plugins at all → no injected routes → overlay fully bypassed.
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact" }
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("PARITY_HOME")).unwrap();
    fs::write(root.join("pages/about.tsx"), page_module("PARITY_ABOUT")).unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "parity") else {
        return;
    };

    let home = dist.join("index.html");
    let about = dist.join("about/index.html");
    assert!(home.is_file(), "expected dist/index.html");
    assert!(about.is_file(), "expected dist/about/index.html");
    assert!(fs::read_to_string(&home).unwrap().contains("PARITY_HOME"));
    assert!(fs::read_to_string(&about).unwrap().contains("PARITY_ABOUT"));
}

// ===========================================================================
// Z1b (#1194): DYNAMIC package routes — paths() enumeration.
//
// A dynamic package route (`[param]` / `[...catchall]`) must enumerate one
// prerendered HTML per concrete path. Covers both the LITERAL paths() path
// (static extraction, no V8 round-trip) and the RUNTIME getCollection()
// path (deferred to the V8 `__paths__` worker), plus catchall route_key
// round-trip and the missing-paths() hard-error parity.
// ===========================================================================

// ---------------------------------------------------------------------------
// 9. Dynamic literal paths() — enumerates statically (no V8).
// ---------------------------------------------------------------------------

/// A dynamic package route `/blog/[slug]` whose entrypoint exports a
/// LITERAL-returning `paths()` enumerates one `dist/blog/<slug>/index.html`
/// per entry — statically, with no V8 round-trip (the overlay re-classifies
/// the inlined literal `paths()` as `Literal`). Each page carries a
/// per-`slug` marker proving params reached the render.
#[test]
fn dynamic_package_route_literal_paths_enumerates() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dyn_literal] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[dyn_literal] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pkg")).unwrap();
    // Literal paths() + a default page that renders the slug param so each
    // enumerated path gets a distinct marker.
    fs::write(
        root.join("pkg/blog.tsx"),
        r#"export function paths() {
  return [
    { params: { slug: "alpha" }, props: { title: "alpha" } },
    { params: { slug: "beta" }, props: { title: "beta" } },
  ];
}
export default function Page({ title }) {
  return (
    <html lang="en">
      <head><title>{title}</title></head>
      <body><p>BLOG_MARKER_{title}</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "dyn-literal-preset",
  setup({ injectRoute }) {
    injectRoute("/blog/[slug]", "./pkg/blog.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME")).unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "dyn_literal") else {
        return;
    };

    for slug in ["alpha", "beta"] {
        let html = dist.join(format!("blog/{slug}/index.html"));
        assert!(
            html.is_file(),
            "literal paths() must enumerate dist/blog/{slug}/index.html; dist html: {:#?}",
            collect_files(&dist, "html")
        );
        let body = fs::read_to_string(&html).unwrap();
        assert!(
            body.contains(&format!("BLOG_MARKER_{slug}")),
            "enumerated page for `{slug}` must carry its per-slug marker; got: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// 10. Dynamic runtime getCollection() paths() — V8 __paths__ enumeration.
// ---------------------------------------------------------------------------

/// A dynamic package route `/docs/[slug]` whose entrypoint calls
/// `getCollection("docs")` in `paths()` is NON-literal → deferred to the V8
/// `/__paths__/<route-key>` worker. The fixture is self-contained: a real
/// `docs` collection (config + `content/docs/*.md`). The build must enumerate
/// ONE `dist/docs/<slug>/index.html` per collection entry, each with the
/// right per-entry marker. This is "the crux of correctness" — it exercises
/// the embedded V8 worker running the bundled module's real `paths()`.
#[test]
fn dynamic_package_route_runtime_getcollection_paths_enumerates() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dyn_runtime] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[dyn_runtime] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    // A real content collection backing the runtime paths().
    fs::create_dir_all(root.join("content/docs")).unwrap();
    fs::write(
        root.join("content/docs/intro.md"),
        "---\ntitle: Intro Doc\n---\n\nintro body.\n",
    )
    .unwrap();
    fs::write(
        root.join("content/docs/setup.md"),
        "---\ntitle: Setup Doc\n---\n\nsetup body.\n",
    )
    .unwrap();

    // The package's dynamic entrypoint: runtime paths() via getCollection.
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(
        root.join("pkg/doc.tsx"),
        r#"type Doc = { slug: string; data: { title: string } };
export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const docs = (await getCollection("docs")) as Doc[];
  return docs.map((d) => ({ params: { slug: d.slug }, props: { slug: d.slug } }));
}
export default function Page({ slug }: { slug: string }) {
  return (
    <html lang="en">
      <head><title>{slug}</title></head>
      <body><p>DOC_MARKER_{slug}</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "dyn-runtime-preset",
  setup({ injectRoute }) {
    injectRoute("/docs/[slug]", "./pkg/doc.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "collections": [{ "name": "docs", "path": "content/docs" }],
  "plugins": [{ "name": "./preset.mjs" }]
}
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME")).unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "dyn_runtime") else {
        return;
    };

    // One HTML per collection entry (slug derived from the .md filename).
    for slug in ["intro", "setup"] {
        let html = dist.join(format!("docs/{slug}/index.html"));
        assert!(
            html.is_file(),
            "runtime getCollection() paths() must enumerate dist/docs/{slug}/index.html via the \
             V8 __paths__ worker; dist html: {:#?}",
            collect_files(&dist, "html")
        );
        let body = fs::read_to_string(&html).unwrap();
        assert!(
            body.contains(&format!("DOC_MARKER_{slug}")),
            "enumerated runtime page for `{slug}` must carry its per-entry marker; got: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// 10a. Compiled-JS paths() export clauses — literal + V8-deferred paths.
// ---------------------------------------------------------------------------

/// tsup/esbuild-style JavaScript route entries commonly keep `paths` as a
/// local function and end with `export { Page as default, paths }`. Both a
/// literal function (the static fast path) and a content-backed function (the
/// deferred V8 path) must build from that compiled `.js` form, without a
/// parallel routes-source copy.
#[test]
fn compiled_js_package_routes_export_clause_enumerate_literal_and_runtime_paths() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[compiled_js_export_clause] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[compiled_js_export_clause] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("content/docs")).unwrap();
    fs::write(
        root.join("content/docs/intro.md"),
        "---\ntitle: Intro\n---\n\nintro body.\n",
    )
    .unwrap();
    fs::write(
        root.join("content/docs/setup.md"),
        "---\ntitle: Setup\n---\n\nsetup body.\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(
        root.join("pkg/compiled-literal.js"),
        r#"import { jsx as _jsx, jsxs as _jsxs } from "preact/jsx-runtime";
function paths() {
  return [
    { params: { slug: "alpha" }, props: { title: "alpha" } },
    { params: { slug: "beta" }, props: { title: "beta" } },
  ];
}
function CompiledLiteralPage({ title }) {
  return _jsxs("html", {
    children: [
      _jsx("head", { children: _jsx("title", { children: title }) }),
      _jsx("body", { children: _jsx("p", { children: "COMPILED_LITERAL_" + title }) }),
    ],
  });
}
export { CompiledLiteralPage as default, paths };
"#,
    )
    .unwrap();
    fs::write(
        root.join("pkg/compiled-runtime.js"),
        r#"import { jsx as _jsx, jsxs as _jsxs } from "preact/jsx-runtime";
async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const docs = await getCollection("docs");
  return docs.map((doc) => ({
    params: { slug: doc.slug },
    props: { slug: doc.slug },
  }));
}
function CompiledRuntimePage({ slug }) {
  return _jsxs("html", {
    children: [
      _jsx("head", { children: _jsx("title", { children: slug }) }),
      _jsx("body", { children: _jsx("p", { children: "COMPILED_RUNTIME_" + slug }) }),
    ],
  });
}
export { CompiledRuntimePage as default, paths };
"#,
    )
    .unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "compiled-js-export-clause-preset",
  setup({ injectRoute }) {
    injectRoute("/compiled-literal/[slug]", "./pkg/compiled-literal.js");
    injectRoute("/compiled-runtime/[slug]", "./pkg/compiled-runtime.js");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "collections": [{ "name": "docs", "path": "content/docs" }],
  "plugins": [{ "name": "./preset.mjs" }]
}
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME")).unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "compiled_js_export_clause") else {
        return;
    };

    for slug in ["alpha", "beta"] {
        let html = dist.join(format!("compiled-literal/{slug}/index.html"));
        assert!(
            html.is_file(),
            "compiled literal paths() must emit {html:?}; dist html: {:#?}",
            collect_files(&dist, "html")
        );
        assert!(
            fs::read_to_string(&html)
                .unwrap()
                .contains(&format!("COMPILED_LITERAL_{slug}")),
            "compiled literal page for `{slug}` must render its marker"
        );
    }
    for slug in ["intro", "setup"] {
        let html = dist.join(format!("compiled-runtime/{slug}/index.html"));
        assert!(
            html.is_file(),
            "compiled runtime paths() must emit {html:?}; dist html: {:#?}",
            collect_files(&dist, "html")
        );
        assert!(
            fs::read_to_string(&html)
                .unwrap()
                .contains(&format!("COMPILED_RUNTIME_{slug}")),
            "compiled runtime page for `{slug}` must render its marker"
        );
    }
}

// ---------------------------------------------------------------------------
// 11. Catchall [...slug] package route — enumerates + route_key round-trips.
// ---------------------------------------------------------------------------

/// A catchall package route `/docs/[...slug]` enumerates multi-segment URLs.
/// The literal paths() returns both single- and multi-segment values; each
/// must prerender at the joined path. This also exercises the route_key
/// encode/round-trip for a catchall template through the universe → manifest
/// join (a catchall template `/docs/[...slug]` survives as the route_key).
#[test]
fn catchall_package_route_enumerates_multisegment() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[catchall] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[catchall] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pkg")).unwrap();
    // Catchall paths(): a one-segment and a two-segment value.
    fs::write(
        root.join("pkg/catchall.tsx"),
        r#"export function paths() {
  return [
    { params: { slug: ["guide"] }, props: { label: "guide" } },
    { params: { slug: ["guide", "deep"] }, props: { label: "guide-deep" } },
  ];
}
export default function Page({ label }) {
  return (
    <html lang="en">
      <head><title>{label}</title></head>
      <body><p>CATCHALL_MARKER_{label}</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "catchall-preset",
  setup({ injectRoute }) {
    injectRoute("/docs/[...slug]", "./pkg/catchall.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME")).unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "catchall") else {
        return;
    };

    let single = dist.join("docs/guide/index.html");
    let multi = dist.join("docs/guide/deep/index.html");
    assert!(
        single.is_file(),
        "catchall must enumerate the single-segment dist/docs/guide/index.html; dist html: {:#?}",
        collect_files(&dist, "html")
    );
    assert!(
        multi.is_file(),
        "catchall must enumerate the multi-segment dist/docs/guide/deep/index.html; dist html: {:#?}",
        collect_files(&dist, "html")
    );
    assert!(fs::read_to_string(&single)
        .unwrap()
        .contains("CATCHALL_MARKER_guide"));
    assert!(fs::read_to_string(&multi)
        .unwrap()
        .contains("CATCHALL_MARKER_guide-deep"));
}

// ---------------------------------------------------------------------------
// 12. Missing paths() on a dynamic package route → hard build error (parity).
// ---------------------------------------------------------------------------

/// A dynamic package route whose entrypoint has NO `paths()` export must FAIL
/// the build with a clear error — the same hard-error invariant a `pages/`
/// dynamic route gets. The overlay inlines no `paths`, so the pipeline's
/// extractor returns `Missing` → the canonical `render_pipeline` hard error
/// fires. (A silent zero-page build would 404 at serve time.)
#[test]
fn dynamic_package_route_missing_paths_hard_errors() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dyn_missing] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[dyn_missing] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pkg")).unwrap();
    // Dynamic route pattern but NO paths() export.
    fs::write(root.join("pkg/no-paths.tsx"), page_module("NO_PATHS")).unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "dyn-missing-preset",
  setup({ injectRoute }) {
    injectRoute("/blog/[slug]", "./pkg/no-paths.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME")).unwrap();

    let output = run_zfb_build(root, &esbuild);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    if !output.status.success() && is_known_skip(&combined) {
        eprintln!("[dyn_missing] known-skip indicator; skipping.\n{combined}");
        return;
    }

    assert!(
        !output.status.success(),
        "a dynamic package route with no paths() must FAIL the build; build succeeded.\n\
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        combined.contains("paths") && combined.to_lowercase().contains("dynamic"),
        "the rejection must be the missing-paths() hard error (parity with pages/); got:\n{combined}"
    );
}

// ---------------------------------------------------------------------------
// fix-A [5]: Tailwind utility classes used ONLY in a package-route page must
// survive into the emitted stylesheet.
// ---------------------------------------------------------------------------

/// A package-route page uses a Tailwind utility class (`bg-blue-500`) that
/// appears in NO user page. Its entrypoint lives outside the conventional
/// project content roots (`pkg/`), so Tailwind's `@source` scan would miss it
/// unless the materialized entrypoint dir is threaded into the content globs.
/// Assert the class survives into `dist/assets/styles-*.css`.
#[test]
fn package_route_page_tailwind_class_survives_in_stylesheet() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[pkg_tw_class] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[pkg_tw_class] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    // The package page uses a Tailwind utility class that no user page uses.
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(
        root.join("pkg/styled.tsx"),
        r#"export default function Page() {
  return (
    <html lang="en">
      <head><title>styled</title></head>
      <body><p className="bg-blue-500">PKG_STYLED_MARKER</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "styled-preset",
  setup({ injectRoute }) {
    injectRoute("/styled", "./pkg/styled.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();

    // A user page WITHOUT the `bg-blue-500` class, so the class can only enter
    // the stylesheet via the package page's content glob.
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("USER_HOME")).unwrap();

    // An authored global stylesheet importing Tailwind so the utility scan runs.
    fs::create_dir_all(root.join("styles")).unwrap();
    fs::write(root.join("styles/global.css"), "@import \"tailwindcss\";\n").unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "pkg_tw_class") else {
        return;
    };

    let css_files = collect_files(&dist.join("assets"), "css");
    assert!(
        !css_files.is_empty(),
        "expected an emitted dist/assets/styles-*.css; dist css: {:#?}",
        collect_files(&dist, "css")
    );
    let any_has_class = css_files.iter().any(|p| {
        fs::read_to_string(p)
            .map(|c| c.contains("bg-blue-500"))
            .unwrap_or(false)
    });
    assert!(
        any_has_class,
        "the package-route page's Tailwind class `bg-blue-500` must be scanned into \
         the emitted stylesheet (package entrypoint dir threaded into @source globs); \
         css files: {css_files:#?}"
    );
}

// ---------------------------------------------------------------------------
// fix-A [2]: a dangling symlink under pages/ must not fail the build once a
// package route forces the overlay copy.
// ---------------------------------------------------------------------------

/// A project with a dangling/broken symlink under `pages/` builds successfully
/// when a preset registers a package route. The overlay copy mirrors the
/// scanner's `follow_links(false)` policy, so the broken symlink is skipped
/// (not stat-errored) — parity with the no-package-route build.
#[test]
fn dangling_symlink_under_pages_does_not_break_build_with_package_route() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[dangling_symlink] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[dangling_symlink] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME_MARKER")).unwrap();
    // A dangling symlink whose target does not exist.
    std::os::unix::fs::symlink(
        root.join("pages/does-not-exist.tsx"),
        root.join("pages/broken.tsx"),
    )
    .unwrap();

    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/preset.tsx"), page_module("PRESET_MARKER")).unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "dangling-preset",
  setup({ injectRoute }) {
    injectRoute("/preset-page", "./pkg/preset.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();

    let Some(dist) = build_or_skip(root, &esbuild, "dangling_symlink") else {
        return;
    };

    // Build succeeded; both the user home and the package route rendered.
    assert!(
        dist.join("index.html").is_file(),
        "user home must render despite the dangling symlink; dist html: {:#?}",
        collect_files(&dist, "html")
    );
    assert!(
        dist.join("preset-page/index.html").is_file(),
        "package route must render; dist html: {:#?}",
        collect_files(&dist, "html")
    );
}

// ---------------------------------------------------------------------------
// fix-A [3][7]: a `.client`-suffixed package route is rejected loudly, and a
// user's real `pages/*.client.tsx` is never clobbered.
// ---------------------------------------------------------------------------

/// A preset injecting `/widget.client` derives `widget.client.tsx`, which the
/// scanner skips as a client-script entry (the route would silently produce no
/// page). The build must FAIL with a clear error, and the user's real
/// `pages/widget.client.tsx` must be left untouched.
#[test]
fn client_suffixed_package_route_rejected_and_user_client_script_safe() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[client_suffix] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[client_suffix] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);

    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), page_module("HOME")).unwrap();
    // A real user client script the package route must NOT clobber.
    let user_client = root.join("pages/widget.client.tsx");
    let user_body = "export default function Widget() { return null; } // USER_CLIENT\n";
    fs::write(&user_client, user_body).unwrap();

    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/widget.tsx"), page_module("PKG_WIDGET")).unwrap();
    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "client-suffix-preset",
  setup({ injectRoute }) {
    injectRoute("/widget.client", "./pkg/widget.tsx");
  },
};
"#,
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();

    let output = run_zfb_build(root, &esbuild);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    if !output.status.success() && is_known_skip(&combined) {
        eprintln!("[client_suffix] known-skip indicator; skipping.\n{combined}");
        return;
    }

    assert!(
        !output.status.success(),
        "a `.client`-suffixed package route must FAIL the build; it succeeded.\n\
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        combined.contains("client-script") || combined.contains(".client"),
        "the rejection must name the client-script contract; got:\n{combined}"
    );
    // The user's real client script is untouched (zfb never writes to pages/).
    assert_eq!(
        fs::read_to_string(&user_client).unwrap(),
        user_body,
        "user's real pages/widget.client.tsx must not be clobbered"
    );
}
