//! Issue #999 — scanner support for `"use client"` islands shipped in the
//! compiled dist of a *regular npm package* (consumed from `node_modules`,
//! not a pnpm-workspace symlink).
//!
//! Before #999 the scanner only descended into pnpm-workspace packages
//! (`node_modules/<pkg>` symlinked to source OUTSIDE `node_modules`).
//! Regular installed packages were skipped wholesale, so a downstream
//! `create-zudo-doc` project consuming `@takazudo/zudo-doc` from npm never
//! registered the package's dist-shipped islands (`Toc`, `MobileToc`,
//! `Sidebar`, a directly-imported `ThemeToggle`, …) and they shipped dead.
//!
//! These tests pin the new resolution behaviour against real on-disk
//! fixtures (the feature is filesystem-specific — `node_modules` layout,
//! pnpm symlinks, `package.json` `exports`), exercising the public API
//! (`scan_islands` + `FsResolver`) exactly as the build pipeline does.
//!
//! Traversal policy under test (agreed in plan review):
//! 1. Enter a regular npm package only via a bare-specifier import from
//!    PROJECT SOURCE (importer outside `node_modules`).
//! 2. `package.json` `exports` resolution is first-class: exact subpath
//!    entries, top-level `"."`, conditional objects (ESM-preferring).
//! 3. Once inside the package, RELATIVE imports are followed (so barrels
//!    without `"use client"` are traversed through to the real modules).
//! 4. Bare imports made from INSIDE a package are never followed — the
//!    framework dependency graph (`preact`, `@takazudo/zfb-runtime`, …) is
//!    not crawled.

use std::fs;
use std::path::{Path, PathBuf};

use zfb_islands::{
    is_same_package_duplicate, scan_islands, scan_islands_with_meta, FsResolver, Manifest,
};

/// Write `body` to `path`, creating parent directories first.
fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, body).expect("write fixture file");
}

/// Scan a single page and return the sorted component names.
fn scan_component_names(page: &Path) -> Vec<String> {
    let resolver = FsResolver::new();
    let islands = scan_islands(&[page.to_path_buf()], &resolver).expect("scan");
    islands.iter().map(|i| i.component_name.clone()).collect()
}

/// A regular npm package laid out as a flat `node_modules/<pkg>` directory
/// (npm/yarn-classic layout — a real dir, NOT a workspace symlink) whose
/// dist module carries `"use client"` must register its islands when a
/// page imports it.
#[test]
fn flat_regular_npm_package_use_client_module_is_registered() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // node_modules/@acme/widgets — a real directory (regular package).
    let pkg = root.join("node_modules/@acme/widgets");
    write(
        &pkg.join("package.json"),
        r#"{ "name": "@acme/widgets", "type": "module",
            "exports": { "./widget": { "types": "./dist/widget.d.ts", "default": "./dist/widget.js" } } }"#,
    );
    write(
        &pkg.join("dist/widget.js"),
        r#""use client";
        import { signal } from "preact/signals";
        export function Widget() { return null; }
        Widget.displayName = "Widget";
        "#,
    );

    // Project source imports the dist subpath directly.
    let page = root.join("pages/home.tsx");
    write(
        &page,
        r#"import { Widget } from "@acme/widgets/widget";
        export default function Home() { return null; }
        "#,
    );

    assert_eq!(scan_component_names(&page), vec!["Widget".to_string()]);
}

/// pnpm consumer layout: `node_modules/@acme/widgets` is a SYMLINK into
/// `node_modules/.pnpm/.../node_modules/@acme/widgets`. Its canonical path
/// still contains a `node_modules/` segment, so it is a *regular* package
/// (not a workspace package), yet a page importing it must still register
/// its dist island.
#[cfg(unix)]
#[test]
fn pnpm_symlinked_regular_package_use_client_module_is_registered() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // The real package lives in the pnpm content store.
    let store = root.join("node_modules/.pnpm/@acme+widgets@1.0.0/node_modules/@acme/widgets");
    write(
        &store.join("package.json"),
        r#"{ "name": "@acme/widgets", "type": "module",
            "exports": { "./widget": { "default": "./dist/widget.js" } } }"#,
    );
    write(
        &store.join("dist/widget.js"),
        r#""use client";
        export function Widget() { return null; }
        "#,
    );

    // node_modules/@acme/widgets -> the store path (pnpm's symlink shape).
    let link = root.join("node_modules/@acme/widgets");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&store, &link).expect("symlink pnpm package");

    let page = root.join("pages/home.tsx");
    write(
        &page,
        r#"import { Widget } from "@acme/widgets/widget";
        export default function Home() { return null; }
        "#,
    );

    assert_eq!(scan_component_names(&page), vec!["Widget".to_string()]);
}

/// Issue #1703, Guard (a): a bare-specifier import that resolves into a
/// REGULAR (non-workspace) npm package — laid out flat, the shape a plain
/// npm/yarn install produces — must never be misreported as a
/// workspace-package edge, even though (per this file's own tests above)
/// the scanner DOES follow and read it. Only a bare import that resolves
/// through a genuine pnpm-workspace symlink (canonical target OUTSIDE
/// `node_modules`) trips Guard (a); framework/third-party deps must never
/// trip it.
#[test]
fn regular_npm_package_import_from_island_is_not_a_workspace_package_edge() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // A flat (non-workspace) regular npm package — a real directory, not a
    // pnpm symlink.
    let pkg = root.join("node_modules/@acme/utils");
    write(
        &pkg.join("package.json"),
        r#"{ "name": "@acme/utils", "type": "module", "main": "index.js" }"#,
    );
    write(&pkg.join("index.js"), "export const helper = 1;\n");

    // Project-source island imports the regular package directly (not the
    // page — the import must happen from an island-reachable module for
    // Guard (a)'s detection to even have a chance to fire).
    let island = root.join("components/gallery.tsx");
    write(
        &island,
        r#""use client";
        import { helper } from "@acme/utils";
        export function Gallery() { return helper; }
        "#,
    );
    let page = root.join("pages/home.tsx");
    write(
        &page,
        r#"import { Gallery } from "../components/gallery";
        export default function Home() { return null; }
        "#,
    );

    let resolver = FsResolver::new();
    let (islands, meta) = scan_islands_with_meta(&[page], &resolver).expect("scan");
    assert_eq!(
        islands
            .iter()
            .map(|i| i.component_name.clone())
            .collect::<Vec<_>>(),
        vec!["Gallery".to_string()],
        "the regular package import IS followed (issue #999)"
    );
    assert!(
        meta.workspace_package_edges_from_islands.is_empty(),
        "a regular (non-workspace) npm package import from an island must never be flagged \
         as a workspace-package edge: {:?}",
        meta.workspace_package_edges_from_islands
    );
}

/// The barrel case (the decisive shape from the real `@takazudo/zudo-doc`
/// dist): a page imports the package's top-level entry (`exports["."]`),
/// which is a barrel `index.js` WITHOUT `"use client"` that re-exports
/// relative modules — one of which (`./toc.js`) IS a `"use client"`
/// module. The scanner must traverse the barrel's relative imports and
/// register the island reached through it.
#[test]
fn barrel_without_use_client_is_traversed_to_relative_use_client_module() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let pkg = root.join("node_modules/@acme/docs");
    write(
        &pkg.join("package.json"),
        r#"{ "name": "@acme/docs", "type": "module",
            "exports": { ".": { "types": "./dist/index.d.ts", "default": "./dist/index.js" } } }"#,
    );
    // Top-level barrel: no directive, only relative re-exports.
    write(
        &pkg.join("dist/index.js"),
        r#"import { Toc } from "./toc/toc.js";
        export { Toc };
        export { helper } from "./util.js";
        "#,
    );
    // Relative leaf WITH the directive — the real island.
    write(
        &pkg.join("dist/toc/toc.js"),
        r#""use client";
        import { useState } from "preact/hooks";
        export function Toc() { return null; }
        Toc.displayName = "Toc";
        "#,
    );
    // A non-island relative module reached through the barrel — must be
    // followed but contributes nothing (no directive).
    write(
        &pkg.join("dist/util.js"),
        r#"export function helper() { return 1; }
        "#,
    );

    let page = root.join("pages/home.tsx");
    write(
        &page,
        r#"import { Toc } from "@acme/docs";
        export default function Home() { return null; }
        "#,
    );

    assert_eq!(scan_component_names(&page), vec!["Toc".to_string()]);
}

/// The exact issue #999 `ThemeToggle` shape: a dist module with
/// `"use client"` as its first line, a plain named export
/// (`export { ThemeToggle };`, NOT the `as default` alias form), and a
/// `displayName` assignment — reached via a `package.json` `exports`
/// subpath (`"./theme-toggle"`) with the real `{ types, default }`
/// conditional shape. A page importing `@scope/pkg/theme-toggle` directly
/// must register `ThemeToggle`.
#[test]
fn issue_999_theme_toggle_shape_via_subpath_export_is_registered() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let pkg = root.join("node_modules/@takazudo/zudo-doc");
    write(
        &pkg.join("package.json"),
        r#"{ "name": "@takazudo/zudo-doc", "type": "module",
            "exports": {
                "./theme-toggle": {
                    "types": "./dist/theme-toggle/index.d.ts",
                    "default": "./dist/theme-toggle/index.js"
                }
            } }"#,
    );
    write(
        &pkg.join("dist/theme-toggle/index.js"),
        r#""use client";
        import { jsx } from "preact/jsx-runtime";
        import { useState } from "preact/hooks";
        import { applyColorScheme } from "./color-scheme-sync.js";
        function ThemeToggle() { return null; }
        ThemeToggle.displayName = "ThemeToggle";
        export { ThemeToggle };
        "#,
    );
    // Relative dependency of the dist module — followed, no island.
    write(
        &pkg.join("dist/theme-toggle/color-scheme-sync.js"),
        r#"export function applyColorScheme() {}
        "#,
    );

    // Mirrors the real `pages/lib/_header-with-defaults.tsx` import.
    let page = root.join("pages/lib/_header-with-defaults.tsx");
    write(
        &page,
        r#"import { ThemeToggle } from "@takazudo/zudo-doc/theme-toggle";
        export default function Header() { return null; }
        "#,
    );

    let resolver = FsResolver::new();
    let islands = scan_islands(std::slice::from_ref(&page), &resolver).expect("scan");
    let island = islands
        .iter()
        .find(|i| i.component_name == "ThemeToggle")
        .expect("ThemeToggle island registered");
    // The named export keys the registry under the same marker the SSR
    // side derives from `displayName`.
    assert_eq!(island.marker_name, "ThemeToggle");
}

/// A bare import made from INSIDE a package's dist must NOT be followed:
/// the package's own dist references its peer dependencies
/// (`preact`-like) via bare specifiers, and the scanner must never crawl
/// into them — even when such a peer (mischievously) carries
/// `"use client"`. Only the directly-imported package's own island
/// surfaces.
#[test]
fn bare_import_from_inside_a_package_is_not_followed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // The package the page imports — has an island AND a bare import of a
    // peer dependency.
    let pkg = root.join("node_modules/@acme/widgets");
    write(
        &pkg.join("package.json"),
        r#"{ "name": "@acme/widgets", "type": "module",
            "exports": { ".": { "default": "./dist/index.js" } } }"#,
    );
    write(
        &pkg.join("dist/index.js"),
        r#""use client";
        import { render } from "fake-preact";
        export function Widget() { return null; }
        "#,
    );

    // A peer "framework" package with a sneaky island — reachable only via
    // the bare `import "fake-preact"` made from inside @acme/widgets/dist.
    let peer = root.join("node_modules/fake-preact");
    write(
        &peer.join("package.json"),
        r#"{ "name": "fake-preact", "type": "module", "exports": { ".": { "default": "./index.js" } } }"#,
    );
    write(
        &peer.join("index.js"),
        r#""use client";
        export function PeerSneaksIn() { return null; }
        "#,
    );

    let page = root.join("pages/home.tsx");
    write(
        &page,
        r#"import { Widget } from "@acme/widgets";
        export default function Home() { return null; }
        "#,
    );

    // Only Widget — the peer's island must never be crawled into.
    assert_eq!(scan_component_names(&page), vec!["Widget".to_string()]);
}

/// ESM-only contract (traversal policy point 5): the scanner does not
/// implement CJS `module.exports` analysis. A regular package that exposes
/// only a `require` condition (no `import`/`module`/`default`) has no ESM
/// entry to resolve, so nothing is scanned and no island is emitted — even
/// when the would-be CJS file contains a `"use client"` directive. This
/// keeps CJS-only dependencies silently inert rather than mis-registered.
#[test]
fn require_only_cjs_package_yields_no_island() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let pkg = root.join("node_modules/@acme/legacy");
    write(
        &pkg.join("package.json"),
        r#"{ "name": "@acme/legacy",
            "exports": { ".": { "require": "./dist/index.cjs" } } }"#,
    );
    // A CJS file — even if it carried a directive, there is no `import`
    // condition to route the scanner here.
    write(
        &pkg.join("dist/index.cjs"),
        r#""use client";
        function Legacy() { return null; }
        module.exports = { Legacy };
        "#,
    );

    let page = root.join("pages/home.tsx");
    write(
        &page,
        r#"import { Legacy } from "@acme/legacy";
        export default function Home() { return null; }
        "#,
    );

    assert!(
        scan_component_names(&page).is_empty(),
        "a require-only CJS package must contribute no islands"
    );
}

/// Hardening for the #999 require-only CJS hole: a regular npm package whose
/// `package.json` declares `exports` with ONLY `require` conditions (no
/// usable ESM target) is `exports`-gated. Even when a STRAY top-level
/// `index.js` carrying `"use client"` sits at the package root — the shape
/// the conventional `src/index`/`index` probe would otherwise pick up — the
/// package stays inert: Node treats `exports` as an encapsulation boundary,
/// so that top-level file is unreachable and must never be scanned.
#[test]
fn require_only_cjs_package_with_stray_esm_index_stays_inert() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let pkg = root.join("node_modules/@acme/legacy");
    write(
        &pkg.join("package.json"),
        r#"{ "name": "@acme/legacy",
            "exports": { ".": { "require": "./dist/index.cjs" } } }"#,
    );
    write(
        &pkg.join("dist/index.cjs"),
        r#""use client";
        function Legacy() { return null; }
        module.exports = { Legacy };
        "#,
    );
    // The stray top-level ESM `index.js` the `exports` gate forbids. Without
    // the gate, the conventional `index` probe would resolve and scan it,
    // registering `StrayIsland` — exactly the hole this test pins shut.
    write(
        &pkg.join("index.js"),
        r#""use client";
        export function StrayIsland() { return null; }
        "#,
    );

    let page = root.join("pages/home.tsx");
    write(
        &page,
        r#"import { Legacy } from "@acme/legacy";
        export default function Home() { return null; }
        "#,
    );

    assert!(
        scan_component_names(&page).is_empty(),
        "a require-only CJS package must stay inert despite a stray top-level index.js"
    );
}

/// When a local project-source component and a package-provided component
/// share a marker name (e.g. both named `ThemeToggle`), the scanner emits
/// two distinct islands (keyed by `(source_path, name)`), and the manifest
/// records a collision the build pass can warn on. This is the #999
/// scenario that makes duplicate marker names likely once `node_modules`
/// is scanned.
#[test]
fn duplicate_marker_name_across_sources_is_recorded_as_a_collision() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // Package-provided ThemeToggle.
    let pkg = root.join("node_modules/@takazudo/zudo-doc");
    write(
        &pkg.join("package.json"),
        r#"{ "name": "@takazudo/zudo-doc", "type": "module",
            "exports": { "./theme-toggle": { "default": "./dist/theme-toggle/index.js" } } }"#,
    );
    write(
        &pkg.join("dist/theme-toggle/index.js"),
        r#""use client";
        export function ThemeToggle() { return null; }
        ThemeToggle.displayName = "ThemeToggle";
        "#,
    );

    // Local project-source ThemeToggle with the same name.
    let local = root.join("src/components/theme-toggle.tsx");
    write(
        &local,
        r#""use client";
        export function ThemeToggle() { return null; }
        "#,
    );

    let page = root.join("pages/home.tsx");
    write(
        &page,
        r#"import { ThemeToggle as Pkg } from "@takazudo/zudo-doc/theme-toggle";
        import { ThemeToggle as Local } from "../src/components/theme-toggle";
        export default function Home() { return null; }
        "#,
    );

    let resolver = FsResolver::new();
    let islands = scan_islands(std::slice::from_ref(&page), &resolver).expect("scan");
    // Two distinct source files, same marker name.
    let theme_islands: Vec<&PathBuf> = islands
        .iter()
        .filter(|i| i.marker_name == "ThemeToggle")
        .map(|i| &i.source_path)
        .collect();
    assert_eq!(
        theme_islands.len(),
        2,
        "expected two ThemeToggle islands from distinct sources: {islands:?}"
    );

    let manifest = Manifest::from_islands(&islands);
    let collisions = manifest.collisions();
    assert_eq!(
        collisions.len(),
        1,
        "expected exactly one marker-name collision: {collisions:?}"
    );
    assert_eq!(collisions[0].name, "ThemeToggle");
    assert_ne!(collisions[0].kept_path, collisions[0].dropped_path);
    // #2441 must not swallow this one: the two components are genuinely
    // different, and "rename one" is advice the author can act on.
    assert!(
        !is_same_package_duplicate(&collisions[0]),
        "a local component colliding with a package component is actionable: {:?}",
        collisions[0]
    );
}

/// Issue #2441 — a package that ships BOTH its compiled `dist/` output and
/// its sources can have one component reach the scanner twice, through two
/// entry graphs, and the two hits are the same component.
///
/// This is zudo-doc's `packageOwnedRoutes` shape, reproduced end to end
/// through the real scanner: a page re-exports the package's compiled route
/// barrel (pulling in `dist/routes/_widget.js`) while a second entry is a
/// verbatim copy of the package's own `routes-src/_widget.tsx`, staged
/// outside `node_modules` so the package's virtual-module imports resolve.
/// The manifest records a collision because the two source paths differ —
/// but nothing in it is actionable, so the build must not warn.
#[test]
fn package_source_and_compiled_duplicate_is_classified_as_a_same_package_duplicate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let pkg = root.join("node_modules/@takazudo/zudo-doc");
    write(
        &pkg.join("package.json"),
        r#"{ "name": "@takazudo/zudo-doc", "type": "module",
            "exports": { "./routes/index": { "default": "./dist/routes/index.js" } } }"#,
    );
    // The package's SHIPPED SOURCE — what the routes plugin stages verbatim.
    let shipped_source = r#""use client";
export function Widget() { return null; }
Widget.displayName = "Widget";
"#;
    write(&pkg.join("routes-src/_widget.tsx"), shipped_source);
    // The COMPILED counterpart of the same module, reached from the barrel.
    write(
        &pkg.join("dist/routes/_widget.js"),
        r#""use client";
export function Widget() {
  return null;
}
Widget.displayName = "Widget";
"#,
    );
    write(
        &pkg.join("dist/routes/index.js"),
        r#"export { Widget } from "./_widget.js";
export default function Index() { return null; }
"#,
    );

    // Entry 1 — a page re-exporting the package's compiled route barrel.
    let page = root.join("pages/index.tsx");
    write(
        &page,
        r#"export { default } from "@takazudo/zudo-doc/routes/index";
"#,
    );
    // Entry 2 — the injected route, pointing at the STAGED copy of the
    // package's own source (a verbatim `cpSync`, byte-for-byte identical).
    let staged = root.join(".zudo-doc/routes-src/_widget.tsx");
    write(&staged, shipped_source);
    let injected = root.join(".zudo-doc/routes-src/route.tsx");
    write(
        &injected,
        r#"import { Widget } from "./_widget";
export default function Route() { return null; }
"#,
    );

    let resolver = FsResolver::new();
    let islands = scan_islands(&[page.clone(), injected.clone()], &resolver).expect("scan");
    let widget_paths: Vec<&PathBuf> = islands
        .iter()
        .filter(|i| i.marker_name == "Widget")
        .map(|i| &i.source_path)
        .collect();
    assert_eq!(
        widget_paths.len(),
        2,
        "expected the same component from both graphs: {islands:?}"
    );

    let manifest = Manifest::from_islands(&islands);
    let collisions = manifest.collisions();
    assert_eq!(
        collisions.len(),
        1,
        "expected exactly one marker-name collision: {collisions:?}"
    );
    assert_eq!(collisions[0].name, "Widget");
    assert!(
        is_same_package_duplicate(&collisions[0]),
        "the staged copy and the package's compiled module are the same component: {:?}",
        collisions[0]
    );
}
