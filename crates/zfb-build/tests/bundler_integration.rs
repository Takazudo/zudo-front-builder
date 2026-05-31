//! End-to-end integration test for `zfb_build::bundler::bundle`.
//!
//! Builds a minimal user project on disk that exercises every feature
//! the bundler is responsible for in Wave 1 / T3:
//!
//! - TS path aliases from a synthetic `tsconfig.json`
//!   (`@/components/foo` → `./components/foo`).
//! - `import.meta.env.PROD` substitution (the bundle must contain a
//!   literal `true` / `false`, not the original expression).
//! - MDX import resolved through
//!   `zfb_content::compile_mdx_to_jsx_module_cached` (hand-off
//!   verified by feeding a `.mdx` file containing markdown that, if
//!   left unprocessed, would not be valid JSX text).
//! - A `"use client"` island compiling cleanly into the same ESM
//!   bundle (the SSR bundle does not separate islands; the islands
//!   client bundle is `zfb-islands`' job).
//!
//! Assertions are deliberately syntactic — the test does **not** spin
//! up the embedded V8 host, does **not** load the bundle into V8, and does **not**
//! perform any port-binding work. It only verifies the resulting ESM
//! file is well-formed (parseable as a JS module by a separate
//! esbuild-syntax-check pass) and that the build-time substitutions
//! landed.
//!
//! ## Esbuild binary discovery
//!
//! The test resolves the esbuild binary in the same order
//! [`zfb_build::bundler::bundle`] does, plus a `which esbuild` PATH
//! fallback. If no binary is available, the test prints a skip note and
//! returns early — the assertion pipeline is gated on having a working
//! subprocess. This matches the gating pattern used by
//! `crates/zfb-islands/tests/integration.rs`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

#[test]
fn end_to_end_bundles_aliases_mdx_islands_and_define() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_integration] no esbuild binary available; \
             set ZFB_ESBUILD_BIN, place the binary at \
             crates/zfb/binaries/esbuild/esbuild, or install esbuild on PATH \
             to enable this test. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    for d in ["pages", "content/posts", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    // Layout — referenced via a relative import (sanity check that
    // shadow tree preserves directory layout for relative imports).
    fs::write(
        root.join("layouts/default.tsx"),
        r#"
            export default function DefaultLayout({ children }) {
              return children;
            }
        "#,
    )
    .unwrap();

    // Component reachable through the `@/components/*` alias.
    fs::write(
        root.join("components/foo.tsx"),
        r#"
            export function Foo({ name }) {
              return "Hello " + name;
            }
        "#,
    )
    .unwrap();

    // "use client" island compiled as part of the SSR bundle (the
    // separate client-side islands bundle is zfb-islands' job; the SSR
    // bundle just needs to handle the directive without choking).
    fs::write(
        root.join("components/island.tsx"),
        r#"
            "use client";
            export function Counter() {
              return "I am an island";
            }
        "#,
    )
    .unwrap();

    // MDX content — markdown body that would NOT be valid JSX text if
    // it survived raw into esbuild (the body has bare `# heading`
    // syntax). Round-tripping through compile_mdx_to_jsx_module_cached
    // is what makes this build at all.
    fs::write(
        root.join("content/posts/hello.mdx"),
        "---\ntitle: Hi\n---\n\n# Hello from MDX\n\nA paragraph body.\n",
    )
    .unwrap();

    // Page that ties the four threads together. The MDX import path
    // uses a relative path (the bundler's `--loader:.mdx=jsx` is what
    // makes esbuild parse the rewritten body as JSX; we keep the .mdx
    // extension so user import sites don't have to know the bundler
    // pre-compiled it).
    fs::write(
        root.join("pages/index.tsx"),
        r#"
            import { Foo } from "@/components/foo";
            import { Counter } from "@/components/island";
            import HelloPost from "../content/posts/hello.mdx";
            import DefaultLayout from "../layouts/default";

            export default function Home() {
              const env = import.meta.env.PROD ? "prod-build" : "dev-build";
              return [
                env,
                Foo({ name: "world" }),
                Counter(),
                typeof HelloPost,
                DefaultLayout({ children: "x" }),
              ].join(" ");
            }
        "#,
    )
    .unwrap();

    // Path alias — the user normally writes this in their tsconfig.
    // We emit it through BundlerInput::tsconfig_paths verbatim.
    let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
    paths.insert("@/components/*".into(), vec!["./components/*".into()]);

    let input = BundlerInput {
        project_root: root.clone(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: HashMap::new(),
        tsconfig_paths: paths,
        // Mark every bare specifier the synthetic entry.mjs imports
        // as external so this test doesn't need a node_modules tree
        // adjacent to the (in-tempdir) shadow root. The bundler's
        // entry.mjs now also imports `createPageRouter` from
        // `@takazudo/zfb-runtime` and `renderToString` from the
        // framework's render module — see
        // `bundler::write_entry_module`.
        external: vec![
            "preact".into(),
            "preact-render-to-string".into(),
            "@takazudo/zfb-runtime".into(),
        ],
        outdir: root.join("dist"),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.clone()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: Vec::new(),
        strip_md_ext: false,
        code_highlight_theme: None,
        code_highlight_themes_dir: None,
        resolve_markdown_links: None,
        gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
        site: None,
        prefetch_disabled: false,
        toc: None,
            external_links: None,
            cjk_friendly: true,
            hard_breaks: false,
            markdown_features: None,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
    };

    let out = bundle(input).expect("end-to-end bundle should succeed");

    // 1. Bundle file exists and has content.
    assert!(out.bundle_path.exists(), "bundle.mjs should exist");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(!body.is_empty(), "bundle should be non-empty");

    // 2. Sourcemap was emitted alongside.
    assert!(
        out.sourcemap_path.exists(),
        "sourcemap should be linked next to the bundle"
    );

    // 3. Manifest reports the single route.
    assert_eq!(out.manifest.framework, "preact");
    assert_eq!(out.manifest.routes.len(), 1, "manifest routes: {:?}", out.manifest.routes);
    assert_eq!(out.manifest.routes[0].route, "/");
    assert_eq!(
        out.manifest.routes[0].source_path,
        PathBuf::from("pages/index.tsx")
    );

    // 4. import.meta.env.PROD was substituted by the boolean literal.
    //    The original expression `import.meta.env.PROD` MUST NOT survive
    //    in the bundle.
    assert!(
        !body.contains("import.meta.env.PROD"),
        "import.meta.env.PROD should have been substituted, but it survives in the bundle.\n--- bundle ---\n{}",
        snippet(&body)
    );
    assert!(
        body.contains("\"prod-build\"") || body.contains("'prod-build'"),
        "PROD ternary's true branch should be reachable in the bundle"
    );

    // 5. The bundle should re-export `routes` and `hydrateIsland`.
    assert!(
        body.contains("routes") && body.contains("hydrateIsland"),
        "expected `routes` and `hydrateIsland` exports"
    );

    // 6. Well-formed: re-parse the bundle through esbuild itself in
    //    "no-bundle" mode. This is the cheapest hermetic JS parse we
    //    have available — esbuild rejects non-parseable input with a
    //    non-zero exit.
    let parse = Command::new(&esbuild)
        .arg(&out.bundle_path)
        .arg("--bundle=false")
        .arg("--log-level=warning")
        .output()
        .expect("re-parse via esbuild");
    assert!(
        parse.status.success(),
        "bundle is not parseable by esbuild: stderr={}",
        String::from_utf8_lossy(&parse.stderr)
    );

    // 7. The MDX body was actually compiled — the JSX output of
    //    `compile_mdx_to_jsx_module_cached` always declares `MDXContent`
    //    or `_createMdxContent`. Either appearing in the bundle proves
    //    the .mdx import was routed through the emitter.
    assert!(
        body.contains("MDXContent") || body.contains("_createMdxContent"),
        "expected MDX emitter output (MDXContent / _createMdxContent) in bundle"
    );

    // 8. The hydration shim was folded in (Preact's shim exports the
    //    hydrateIsland symbol; the import pattern from "preact" stays
    //    bare because we marked preact external).
    assert!(
        body.contains("hydrateIsland"),
        "hydrate shim's hydrateIsland export should be present"
    );
    assert!(
        body.contains("from \"preact\"") || body.contains("from'preact'"),
        "preact import should remain external in the bundle"
    );
}

/// Truncate a long string for assert messages.
fn snippet(s: &str) -> String {
    if s.len() <= 800 {
        return s.to_string();
    }
    format!("{}…[{} bytes truncated]", &s[..800], s.len() - 800)
}

/// Vite `import.meta.glob('…', { eager: true })` is expanded Rust-side
/// inside the shadow tree BEFORE esbuild runs (#665 / #670). esbuild leaves
/// the macro untransformed, so if expansion didn't happen the literal
/// `import.meta.glob(` would survive into the bundle and blow up at SSR.
///
/// This mirrors the `import.meta.env.PROD` substitution test: build a fixture
/// that globs a sibling directory, bundle it, and assert (a) the literal macro
/// text is ABSENT from the emitted bundle and (b) the expanded relative keys
/// are present. Gated on `locate_esbuild()`; self-skips when no binary.
#[test]
fn import_meta_glob_eager_is_expanded_before_esbuild() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[import_meta_glob_eager_is_expanded_before_esbuild] no esbuild binary; \
             set ZFB_ESBUILD_BIN, stage the slot, or install esbuild on PATH. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    for d in ["pages", "components/widgets"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    // Globbed siblings live under components/widgets/ (NOT pages/) so
    // derive_route does not turn them into surprise routes.
    fs::write(
        root.join("components/widgets/alpha.tsx"),
        "export const alpha = 'A';\n",
    )
    .unwrap();
    fs::write(
        root.join("components/widgets/beta.tsx"),
        "export const beta = 'B';\n",
    )
    .unwrap();

    // A registry module under components/ that uses the eager glob macro.
    // It is reached from the page via the relative import below.
    fs::write(
        root.join("components/registry.tsx"),
        r#"
            const widgets = import.meta.glob('./widgets/*.tsx', { eager: true });
            export const widgetKeys = Object.keys(widgets).sort().join(",");
        "#,
    )
    .unwrap();

    // Page imports the registry so the whole graph is reachable from the
    // single esbuild entrypoint.
    fs::write(
        root.join("pages/index.tsx"),
        r#"
            import { widgetKeys } from "../components/registry";
            export default function Home() {
              return widgetKeys;
            }
        "#,
    )
    .unwrap();

    let input = BundlerInput {
        project_root: root.clone(),
        pages_dir: PathBuf::from("pages"),
        content_dir: PathBuf::from("content"),
        components_dir: PathBuf::from("components"),
        layouts_dir: PathBuf::from("layouts"),
        framework: Framework::Preact,
        define_vars: HashMap::new(),
        tsconfig_paths: BTreeMap::new(),
        external: vec![
            "preact".into(),
            "preact-render-to-string".into(),
            "@takazudo/zfb-runtime".into(),
        ],
        outdir: root.join("dist"),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.clone()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: Vec::new(),
        strip_md_ext: false,
        code_highlight_theme: None,
        code_highlight_themes_dir: None,
        resolve_markdown_links: None,
        gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
        site: None,
        prefetch_disabled: false,
        toc: None,
        external_links: None,
        cjk_friendly: true,
        hard_breaks: false,
        markdown_features: None,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
        mdx_components_file: None,
    };

    let out = bundle(input).expect("import.meta.glob end-to-end bundle should succeed");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // 1. The Vite-only macro must NOT survive into the emitted bundle.
    assert!(
        !body.contains("import.meta.glob("),
        "import.meta.glob( should have been expanded Rust-side, but it survives:\n--- bundle ---\n{}",
        snippet(&body)
    );

    // 2. The expanded relative keys are present (esbuild keeps the string
    //    literals from the generated object literal).
    assert!(
        body.contains("./widgets/alpha.tsx") && body.contains("./widgets/beta.tsx"),
        "expanded glob keys should be present in the bundle:\n--- bundle ---\n{}",
        snippet(&body)
    );

    // 3. Sanity: the bundle re-parses cleanly through esbuild.
    let parse = Command::new(&esbuild)
        .arg(&out.bundle_path)
        .arg("--bundle=false")
        .arg("--log-level=warning")
        .output()
        .expect("re-parse via esbuild");
    assert!(
        parse.status.success(),
        "expanded bundle is not parseable by esbuild: stderr={}",
        String::from_utf8_lossy(&parse.stderr)
    );
}
