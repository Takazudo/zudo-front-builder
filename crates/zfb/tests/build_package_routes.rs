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
//! the fixture/node_modules wiring of `client_router_autoinclude_build.rs`
//! and `build_cleans_outdir.rs`: `node_modules/` is symlinked to the
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

// ---------------------------------------------------------------------------
// 2. Empty/absent pages/ + a "/" package route → dist/index.html.
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

    // `"/"` is allowed in BUILD mode (dev-only rejection scoped in #1193).
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
