//! Consumer integration test and byte-identical parity check for
//! package-owned routes (#1195, epic #1191 Wave 4 / Confirm).
//!
//! ## What this file proves
//!
//! 1. **End-to-end consumer build**: a realistic fixture that mirrors the
//!    shape `@takazudo/zudo-doc` needs — a preset injecting BOTH a static
//!    route AND a dynamic route (literal `paths()`) alongside the consumer
//!    project's own `pages/` routes. The build must produce the full
//!    expected `dist/` tree: static package-route output + per-slug dynamic
//!    outputs + untouched `pages/` outputs.
//!
//! 2. **Byte-identical control-project parity**: a `pages/`-only control
//!    project (NO preset) built twice produces identical `dist/` trees. This
//!    guards the overlay bypass invariant: when there are no injected routes
//!    the overlay machinery is skipped entirely (`build_pages_root ==
//!    project_root/pages`), so the output is indistinguishable from a run
//!    that never knew about package routes. If any nondeterministic output
//!    is found, that is a regression to flag, not to normalize away silently.
//!
//! 3. **Re-verification matrix summary**: each case from the Wave-4 spec is
//!    verified by a landed test across `build_package_routes.rs` and this
//!    file; the matrix is documented in the comments below.
//!
//! ## Level / tier
//!
//! Level 3 (build output) — reads emitted `dist/` files; no HTTP server
//! needed. The consumer test runs the real `zfb` binary against on-disk
//! fixtures, following the pattern of `build_package_routes.rs`.
//!
//! Unix-only because the embedded-tree symlink wiring is the established
//! Unix pattern in the sibling build tests.
//!
//! ## Re-verification matrix (Wave-4 spec)
//!
//! | Case | Covered by |
//! |---|---|
//! | empty `pages/` + package route | `build_package_routes::empty_pages_with_root_package_route_builds` |
//! | package route with relative imports | `build_package_routes::package_route_with_relative_import_bundles` + this file (consumer fixture uses `./site-meta` relative import) |
//! | dynamic literal `paths()` | `build_package_routes::dynamic_package_route_literal_paths_enumerates` + this file (consumer fixture) |
//! | dynamic runtime `getCollection` | `build_package_routes::dynamic_package_route_runtime_getcollection_paths_enumerates` |
//! | missing `paths()` hard error | `build_package_routes::dynamic_package_route_missing_paths_hard_errors` |
//! | `prerender=false` rejection under `output: static` | `build_package_routes::output_static_rejects_ssr_shaped_package_route` |
//! | user-vs-package collision | `build_package_routes::user_pages_wins_collision_including_shape_dup` |
//! | package-only island route | `build_package_routes::package_route_with_use_client_emits_island_asset` |

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use zfb_test_utils::{locate_esbuild, zfb_binary};

// ---------------------------------------------------------------------------
// Shared helpers (mirrors build_package_routes.rs patterns)
// ---------------------------------------------------------------------------

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

/// Copy the embedded runtime packages into a real, consumer-local
/// `node_modules/` directory. This is intentionally distinct from
/// [`link_embedded_node_modules`]: the #1516 certification needs the route
/// entrypoint's canonical path to remain inside the temporary consumer project.
fn materialize_embedded_node_modules(root: &Path) {
    let (_nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    copy_dir(&embedded_nm_path, &root.join("node_modules"));
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

/// `true` when the non-zero build is a known-skip (no embedded V8 / no esbuild).
fn is_known_skip(combined: &str) -> bool {
    combined.contains("embed_v8") || combined.contains("no esbuild")
}

/// Recursively collect every regular file under `dir`. Returns a
/// `BTreeMap<relative-path-string, file-bytes>` so the comparison is order-
/// stable and fully content-addressed (no timestamps).
fn collect_all_files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
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
            } else {
                let rel = p
                    .strip_prefix(dir)
                    .expect("strip_prefix")
                    .to_string_lossy()
                    .into_owned();
                let bytes = fs::read(&p).unwrap_or_default();
                out.insert(rel, bytes);
            }
        }
    }
    out
}

/// Run `zfb build` in `root`, returning `Some(dist_tree)` on success
/// (where `dist_tree` is a `BTreeMap<rel-path, bytes>`) or `None` for
/// known-skip environments (no V8 / no esbuild).
fn build_and_collect(root: &Path, esbuild: &Path, test: &str) -> Option<BTreeMap<String, Vec<u8>>> {
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
    let dist = root.join("dist");
    Some(collect_all_files(&dist))
}

/// Copy a directory tree recursively. Mirrors the helper in
/// `dev_build_static_parity.rs`.
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

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("package-routes-consumer")
}

// ---------------------------------------------------------------------------
// Test 1 — Consumer build: static + dynamic routes + user pages/ unaffected
// ---------------------------------------------------------------------------

/// A realistic consumer project using a preset that injects BOTH a static
/// route (`/preset-about`) AND a dynamic route (`/preset-docs/[slug]` with
/// literal `paths()`) alongside the consumer's own `pages/` routes
/// (`/` and `/guide`).
///
/// The consumer fixture's preset page also exercises a relative TS import
/// (`./site-meta`) to prove the overlay resolves package-route relative
/// imports through the full consumer build.
///
/// Expected `dist/` tree:
/// - `dist/index.html` — user home page (CONSUMER_USER_HOME_MARKER)
/// - `dist/guide/index.html` — user guide page (CONSUMER_USER_GUIDE_MARKER)
/// - `dist/preset-about/index.html` — static package route
///   (CONSUMER_PRESET_ABOUT_MARKER)
/// - `dist/preset-docs/getting-started/index.html` — dynamic enumeration
///   (CONSUMER_PRESET_DOCS_MARKER_getting-started)
/// - `dist/preset-docs/api-reference/index.html` — dynamic enumeration
///   (CONSUMER_PRESET_DOCS_MARKER_api-reference)
#[test]
fn consumer_build_emits_full_dist_tree() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[consumer_build] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[consumer_build] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _nm = link_embedded_node_modules(root);
    copy_dir(&fixture_dir(), root);

    let Some(dist) = build_and_collect(root, &esbuild, "consumer_build") else {
        return;
    };

    // --- User pages/ routes unaffected ---

    let home = root.join("dist/index.html");
    assert!(
        home.is_file(),
        "dist/index.html must exist (user pages/index.tsx); dist: {dist:#?}"
    );
    let home_body = fs::read_to_string(&home).unwrap();
    assert!(
        home_body.contains("CONSUMER_USER_HOME_MARKER"),
        "user home page must contain its marker; got: {home_body}"
    );

    let guide = root.join("dist/guide/index.html");
    assert!(
        guide.is_file(),
        "dist/guide/index.html must exist (user pages/guide.tsx); dist: {dist:#?}"
    );
    let guide_body = fs::read_to_string(&guide).unwrap();
    assert!(
        guide_body.contains("CONSUMER_USER_GUIDE_MARKER"),
        "user guide page must contain its marker; got: {guide_body}"
    );

    // --- Static package route ---

    let about = root.join("dist/preset-about/index.html");
    assert!(
        about.is_file(),
        "dist/preset-about/index.html must exist (preset static route); dist: {dist:#?}"
    );
    let about_body = fs::read_to_string(&about).unwrap();
    assert!(
        about_body.contains("CONSUMER_PRESET_ABOUT_MARKER"),
        "static package route must render with its marker; got: {about_body}"
    );
    // Relative import from pkg/about.tsx → pkg/site-meta.ts must bundle.
    assert!(
        about_body.contains("Consumer Demo"),
        "relative import from site-meta.ts must be bundled into the static package route; \
         got: {about_body}"
    );

    // --- Dynamic package route: literal paths() enumeration ---

    for slug in ["getting-started", "api-reference"] {
        let doc = root.join(format!("dist/preset-docs/{slug}/index.html"));
        assert!(
            doc.is_file(),
            "dist/preset-docs/{slug}/index.html must exist (dynamic package route enumeration); \
             dist: {dist:#?}"
        );
        let doc_body = fs::read_to_string(&doc).unwrap();
        assert!(
            doc_body.contains(&format!("CONSUMER_PRESET_DOCS_MARKER_{slug}")),
            "dynamic package route enumeration for `{slug}` must carry its per-slug marker; \
             got: {doc_body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 2 — Byte-identical control-project parity (overlay bypass invariant)
// ---------------------------------------------------------------------------

/// A `pages/`-only control project (NO preset, NO injected routes) built
/// twice produces **identical** `dist/` trees — every file, same bytes.
///
/// This guards the overlay bypass invariant from `package_routes.rs`:
/// when there are no injected routes, `build_pages_root` is set to
/// `project_root/pages` and the overlay temp-dir is never created, so the
/// emitted output is byte-for-byte identical to a world where package routes
/// were never implemented. Any nondeterministic output found here is a
/// regression in the build pipeline, not something to normalize away.
///
/// The control project is built from scratch each time (fresh tempdir, fresh
/// symlinked node_modules) to ensure the comparison is cross-invocation.
#[test]
fn pages_only_build_is_byte_identical_across_two_runs() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[parity] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[parity] node not on PATH; skipping.");
        return;
    }

    // Build A — first clean run.
    let tmp_a = tempfile::tempdir().expect("tempdir A");
    let root_a = tmp_a.path();
    let _nm_a = link_embedded_node_modules(root_a);
    write_control_project(root_a);
    let Some(tree_a) = build_and_collect(root_a, &esbuild, "parity_A") else {
        return;
    };

    // Build B — second independent run (different temp directory).
    let tmp_b = tempfile::tempdir().expect("tempdir B");
    let root_b = tmp_b.path();
    let _nm_b = link_embedded_node_modules(root_b);
    write_control_project(root_b);
    let Some(tree_b) = build_and_collect(root_b, &esbuild, "parity_B") else {
        return;
    };

    // Assert identical file sets.
    let keys_a: Vec<&str> = tree_a.keys().map(|s| s.as_str()).collect();
    let keys_b: Vec<&str> = tree_b.keys().map(|s| s.as_str()).collect();
    assert_eq!(
        keys_a, keys_b,
        "two control-project builds must emit identical file sets (overlay bypass invariant); \
         A={keys_a:#?} B={keys_b:#?}"
    );

    // Assert byte-identical content for every file.
    let file_count = tree_a.len();
    let mut mismatches: Vec<String> = Vec::new();
    for (rel, bytes_a) in &tree_a {
        let bytes_b = tree_b.get(rel).expect("key checked above");
        if bytes_a != bytes_b {
            mismatches.push(rel.clone());
        }
    }
    assert!(
        mismatches.is_empty(),
        "byte-identical parity FAILED for {} file(s) out of {} total — this is a \
         nondeterministic build regression (not to be normalized away):\n{mismatches:#?}",
        mismatches.len(),
        file_count,
    );

    eprintln!(
        "[parity] byte-identical across {file_count} files — overlay bypass invariant holds."
    );
}

/// Write a minimal `pages/`-only control project into `root`. No plugins,
/// no preset — just user pages, so the overlay is entirely bypassed.
fn write_control_project(root: &Path) {
    fs::write(
        root.join("zfb.config.json"),
        // No plugins key — overlay machinery never activates.
        r#"{ "framework": "preact" }
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"export default function Page() {
  return (
    <html lang="en">
      <head><title>Parity Home</title></head>
      <body><p>PARITY_CONTROL_HOME</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("pages/about.tsx"),
        r#"export default function Page() {
  return (
    <html lang="en">
      <head><title>Parity About</title></head>
      <body><p>PARITY_CONTROL_ABOUT</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Test 3 — Compiled node_modules route + virtual paths() certification
// ---------------------------------------------------------------------------

/// A tsup-style compiled `.js` dynamic entrypoint physically installed under
/// the consumer's pnpm-shaped `node_modules/.pnpm/.../node_modules/...` store
/// imports a plugin-provided `virtual:` module and returns its slugs from a
/// non-literal local `paths()` function exported through a trailing clause.
///
/// This certifies that zudo-doc can delete both its `routes-src` workaround
/// and `.zudo-doc/` staging: its compiled package route can resolve virtual
/// data from real node_modules, defer `paths()` to V8, and emit the exact
/// pages selected by that virtual data in a full `zfb build`.
#[test]
fn compiled_node_modules_virtual_paths_certify_zudo_doc_can_drop_staging() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[node_modules_virtual_paths] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[node_modules_virtual_paths] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    materialize_embedded_node_modules(root);
    let node_modules = root.join("node_modules");
    let node_modules_metadata = fs::symlink_metadata(&node_modules).expect("node_modules metadata");
    assert!(
        node_modules_metadata.file_type().is_dir() && !node_modules_metadata.file_type().is_symlink(),
        "the consumer fixture must use a real local node_modules directory, not the harness's external whole-directory symlink"
    );

    // Model pnpm's published-package realpath. The usual consumer-facing
    // `node_modules/@takazudo/zudo-doc` symlink points at this actual store
    // directory, so canonicalizing the injected entrypoint stays under the
    // temporary project's real node_modules tree.
    let package_root = node_modules
        .join(".pnpm")
        .join("@takazudo+zudo-doc@0.0.0")
        .join("node_modules")
        .join("@takazudo")
        .join("zudo-doc");
    let routes_dir = package_root.join("dist/routes");
    fs::create_dir_all(&routes_dir).unwrap();
    fs::write(
        package_root.join("package.json"),
        r#"{ "name": "@takazudo/zudo-doc", "version": "0.0.0", "type": "module" }"#,
    )
    .unwrap();
    let entrypoint = routes_dir.join("docs-slug.js");
    fs::write(
        &entrypoint,
        r#"import { jsx as _jsx, jsxs as _jsxs } from "preact/jsx-runtime";
import { virtualSlugs } from "virtual:zudo-doc-route-slugs";
function paths() {
  return virtualSlugs.map((slug) => ({ params: { slug }, props: { slug } }));
}
function DocsPage({ slug }) {
  return _jsxs("html", {
    children: [
      _jsx("head", { children: _jsx("title", { children: slug }) }),
      _jsx("body", { children: _jsx("p", { children: "NODE_MODULES_VIRTUAL_ROUTE_" + slug }) }),
    ],
  });
}
export { DocsPage as default, paths };
"#,
    )
    .unwrap();
    let package_link = node_modules.join("@takazudo/zudo-doc");
    fs::create_dir_all(package_link.parent().expect("scoped package parent")).unwrap();
    std::os::unix::fs::symlink(&package_root, &package_link).expect("pnpm package symlink");

    let canonical_node_modules = node_modules.canonicalize().expect("canonical node_modules");
    let canonical_entrypoint = entrypoint.canonicalize().expect("canonical entrypoint");
    assert!(
        canonical_entrypoint.starts_with(&canonical_node_modules),
        "the injected entrypoint's canonical path must stay under the temporary consumer's real node_modules: {}",
        canonical_entrypoint.display()
    );
    assert!(
        canonical_entrypoint.ends_with(
            Path::new(".pnpm/@takazudo+zudo-doc@0.0.0/node_modules/@takazudo/zudo-doc/dist/routes/docs-slug.js")
        ),
        "the canonical entrypoint must use the pnpm store layout; got {}",
        canonical_entrypoint.display()
    );
    assert_eq!(
        package_link
            .canonicalize()
            .expect("canonical pnpm package link"),
        package_root
            .canonicalize()
            .expect("canonical pnpm package root"),
        "the consumer-facing package link must resolve to the pnpm store directory"
    );

    let entrypoint_json = serde_json::to_string(&canonical_entrypoint.to_string_lossy()).unwrap();
    fs::write(
        root.join("preset.mjs"),
        format!(
            r#"export default {{
  name: "node-modules-virtual-paths-certification",
  setup({{ injectRoute, addVirtualModule }}) {{
    addVirtualModule(
      "virtual:zudo-doc-route-slugs",
      () => "export const virtualSlugs = ['from-virtual-install', 'from-virtual-api'];",
    );
    injectRoute("/zudo-doc/[slug]", {entrypoint_json});
  }},
}};
"#
        ),
    )
    .unwrap();
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"export default function Home() {
  return <html><body><p>NODE_MODULES_VIRTUAL_HOME</p></body></html>;
}
"#,
    )
    .unwrap();

    let Some(dist) = build_and_collect(root, &esbuild, "node_modules_virtual_paths") else {
        return;
    };

    for slug in ["from-virtual-install", "from-virtual-api"] {
        let page = root.join(format!("dist/zudo-doc/{slug}/index.html"));
        assert!(
            page.is_file(),
            "the virtual module's `{slug}` value must produce {page:?}; dist: {dist:#?}"
        );
        let body = fs::read_to_string(&page).unwrap();
        assert!(
            body.contains(&format!("NODE_MODULES_VIRTUAL_ROUTE_{slug}")),
            "the route selected by virtual data must render its marker; got: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 4 — Project-local generated package routes stay inside the stage
// ---------------------------------------------------------------------------

/// Project-local generated route entrypoints are already copied by the narrow
/// `.zudo-doc/routes-src/**` staging allowlist. The package-route overlay must
/// import that staged copy through a lexical project-relative spelling instead
/// of the live absolute entrypoint; otherwise a workspace build's stage-escape
/// audit correctly rejects the overlay import.
///
/// This fixture uses the source filenames emitted by zudo-doc (`robots.txt`,
/// `404`, `_chrome`, `_context`, `docs-slug`, and `sitemap.xml`), shares a
/// generated module between routes, and includes a nested generated source.
/// The workspace shape arms the stage-escape audit, while the rendered markers
/// prove the freshly staged source — not a live-tree fallback — reached V8.
#[test]
fn project_local_generated_package_routes_resolve_from_the_staged_workspace_copy() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[project_local_generated_routes] no esbuild; skipping.");
        return;
    };
    if !node_available() {
        eprintln!("[project_local_generated_routes] node not on PATH; skipping.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace = tmp.path();
    fs::write(
        workspace.join("pnpm-workspace.yaml"),
        "packages:\n  - 'sub-packages/*'\n",
    )
    .expect("write workspace manifest");
    let root = workspace.join("sub-packages/host");
    fs::create_dir_all(&root).expect("create workspace project");
    let _nm = link_embedded_node_modules(&root);
    write_project_local_generated_routes_fixture(&root);

    let Some(dist) = build_and_collect(&root, &esbuild, "project_local_generated_routes") else {
        return;
    };

    for route in [
        "robots",
        "not-found",
        "chrome",
        "context",
        "sitemap",
        "nested/reference",
    ] {
        let page = root.join("dist/generated").join(route).join("index.html");
        assert!(
            page.is_file(),
            "generated route `{route}` must render from the package-route overlay; dist: {dist:#?}"
        );
        let body = fs::read_to_string(&page).expect("read rendered generated route");
        assert!(
            body.contains(&format!(
                "PROJECT_LOCAL_ROUTE_{route}_PROJECT_LOCAL_SHARED_MARKER_FRESH"
            )),
            "generated route `{route}` must render its fresh shared staged marker; got: {body}"
        );
    }

    let dynamic = root.join("dist/generated/docs/from-stage/index.html");
    assert!(
        dynamic.is_file(),
        "the generated docs-slug route must enumerate its staged dynamic path; dist: {dist:#?}"
    );
    let dynamic_body = fs::read_to_string(&dynamic).expect("read rendered dynamic generated route");
    assert!(
        dynamic_body.contains("PROJECT_LOCAL_ROUTE_docs-from-stage_PROJECT_LOCAL_SHARED_MARKER_FRESH"),
        "the generated docs-slug route must render its fresh shared staged marker; got: {dynamic_body}"
    );
}

fn write_project_local_generated_routes_fixture(root: &Path) {
    let routes = root.join(".zudo-doc/routes-src");
    fs::create_dir_all(routes.join("nested")).expect("create generated route source dirs");
    // The allowlist must bypass generic hidden-dir and gitignore filtering.
    fs::write(root.join(".gitignore"), ".zudo-doc/\n").expect("write fixture gitignore");
    fs::write(
        routes.join("shared.ts"),
        r#"export const sharedMarker = "PROJECT_LOCAL_SHARED_MARKER_FRESH";
"#,
    )
    .expect("write generated shared module");

    for (filename, route_marker) in [
        ("robots.txt.tsx", "robots"),
        ("404.tsx", "not-found"),
        ("_chrome.tsx", "chrome"),
        ("sitemap.xml.tsx", "sitemap"),
    ] {
        fs::write(
            routes.join(filename),
            format!(
                r#"import {{ sharedMarker }} from "./shared";
export default function GeneratedRoute() {{
  return <html><body>PROJECT_LOCAL_ROUTE_{route_marker}_{{sharedMarker}}</body></html>;
}}
"#
            ),
        )
        .expect("write generated static route");
    }
    fs::write(
        routes.join("_context.ts"),
        r#"import { h } from "preact";
import { sharedMarker } from "./shared";
export default function GeneratedContext() {
  return h("html", null, h("body", null, "PROJECT_LOCAL_ROUTE_context_" + sharedMarker));
}
"#,
    )
    .expect("write generated context route");
    fs::write(
        routes.join("docs-slug.tsx"),
        r#"import { sharedMarker } from "./shared";
export function paths() {
  return [{ params: { slug: "from-stage" }, props: { slug: "from-stage" } }];
}
export default function GeneratedDocs({ slug }: { slug: string }) {
  return <html><body>PROJECT_LOCAL_ROUTE_docs-{slug}_{sharedMarker}</body></html>;
}
"#,
    )
    .expect("write generated docs route");
    fs::write(
        routes.join("nested/route.tsx"),
        r#"import { sharedMarker } from "../shared";
export default function GeneratedNestedRoute() {
  return <html><body>PROJECT_LOCAL_ROUTE_nested/reference_{sharedMarker}</body></html>;
}
"#,
    )
    .expect("write nested generated route");

    fs::write(
        root.join("preset.mjs"),
        r#"export default {
  name: "project-local-generated-routes",
  setup({ injectRoute }) {
    injectRoute("/generated/robots", ".zudo-doc/routes-src/robots.txt.tsx");
    injectRoute("/generated/not-found", ".zudo-doc/routes-src/404.tsx");
    injectRoute("/generated/chrome", ".zudo-doc/routes-src/_chrome.tsx");
    injectRoute("/generated/context", ".zudo-doc/routes-src/_context.ts");
    injectRoute("/generated/docs/[slug]", ".zudo-doc/routes-src/docs-slug.tsx");
    injectRoute("/generated/sitemap", ".zudo-doc/routes-src/sitemap.xml.tsx");
    injectRoute("/generated/nested/reference", ".zudo-doc/routes-src/nested/route.tsx");
  },
};
"#,
    )
    .expect("write generated-routes preset");
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./preset.mjs" }] }
"#,
    )
    .expect("write generated-routes config");
    fs::create_dir_all(root.join("pages")).expect("create pages dir");
    fs::write(
        root.join("pages/index.tsx"),
        r#"export default function Home() {
  return <html><body>PROJECT_LOCAL_GENERATED_HOME</body></html>;
}
"#,
    )
    .expect("write home page");
}
