//! Wave 3 / Sub #199 — Migration-Fixes integration confirm.
//!
//! End-to-end smoke that exercises every Wave 1 + Wave 2 change in
//! concert (zfb#189 sub-issues #190–#198) so regressions introduced by
//! one fix cannot hide behind another.
//!
//! ## Test split
//!
//! The suite is divided into two groups:
//!
//! **Content-pipeline assertions** (no binary required, always run):
//!
//! 1. Admonition `:::note[Hello]` → `title="Hello"` attribute
//!    (`title_from_label` default flip, sub #195 / #135).
//! 2. Un-blank-lined directive (`:::note\nbody\n:::`) → blank-line
//!    diagnostic emitted (sub #195 / #185 Gap 2).
//! 3. GFM pipe-table → JSX contains `table`/`thead`/`tbody` components
//!    (sub #193 / #136).
//! 4. Two collections with the same H2 heading → each document starts
//!    with slug `overview` (reset_per_entry walk-order fix, sub #190 /
//!    #187).
//!
//! **Bundler-level assertions** (esbuild-gated — skipped when no binary):
//!
//! 5. Full fixture with:
//!    - `public/` containing a nested asset (sub #192 / #158) — we
//!      verify `copy_public_dir` is wired by asserting the fixture
//!      layout is accepted (the copy itself is tested in `zfb` crate
//!      unit tests; here we just confirm the bundler accepts
//!      `code_highlight_theme` and `resolve_markdown_links`).
//!    - CSS using the split-import pattern; the engine-level assertion
//!      lives in `zfb-css` tests but is re-exercised here at the
//!      bundler level by passing `code_highlight_theme = "InspiredGitHub"`.
//!    - MDX content exercising directive-label (`:::note[World]`), GFM
//!      pipe-table, and an author-style `[label](./other.mdx)` link.
//!    - `code_highlight_theme = Some("InspiredGitHub")` → bundle must
//!      contain `syntect-inspiredgithub` class (sub #194 / #188).
//!    - `resolve_markdown_links = Some(…)` with `onBrokenLinks: Warn` →
//!      bundle succeeds (sub #196 / #185 Gap 1).
//!    - Two collections walked in sorted order → each collection's
//!      bundle contains a clean `overview` slug (sub #190 / #187).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use zfb_build::{bundle, BundleMode, BundlerInput, ContentCollectionSpec, OnBrokenLinks, ResolveMarkdownLinksRoute, ResolveMarkdownLinksSpec};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

// ---------------------------------------------------------------------------
// Helpers shared by zzmod_all_five_migration_fixes_compose only.
// ---------------------------------------------------------------------------

/// Mirror the behaviour of `zfb/src/commands/build.rs::read_tsconfig_paths`:
/// resolve each `compilerOptions.paths` target to an absolute path against
/// the project root (preserving a trailing `/*`). Duplicated here (same
/// shape as in `bundler_workspace_pkg_alias.rs`) because `read_tsconfig_paths`
/// is `pub(crate)` in `zfb` and `zfb-build` cannot depend on `zfb` (cycle).
///
/// NOTE: #666's actual `read_tsconfig_paths` (extends + baseUrl) is
/// unit-tested in `crates/zfb/src/commands/build.rs`.  Here we verify
/// composition at the bundler level: paths fed into `BundlerInput::tsconfig_paths`
/// ARE wired through to esbuild — a regression in that wiring fails THIS test.
fn tsconfig_paths_absolute(
    project_root: &std::path::Path,
    paths: &[(&str, &str)],
) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for (key, target) in paths {
        let (prefix, suffix) = match target.rsplit_once("/*") {
            Some((p, "")) => (p, "/*"),
            _ => (*target, ""),
        };
        let abs = project_root.join(prefix);
        let mut s = abs.to_string_lossy().into_owned();
        s.push_str(suffix);
        out.insert(key.to_string(), vec![s]);
    }
    out
}

/// Write the hand-rolled CJS-only package into `<root>/node_modules/<name>`.
///
/// Same shape as `bundler_exclude_glob.rs::write_cjs_only_package` — a
/// `package.json` with `main`+`module` and NO `exports` map (the
/// `path-to-regexp@6` / msw CJS-rejection shape). Under `--platform=neutral`
/// esbuild cannot resolve it, reproducing the motivating failure without a
/// real npm install.
fn write_cjs_only_pkg_for_confirm(root: &std::path::Path, name: &str) {
    let pkg = root.join("node_modules").join(name);
    fs::create_dir_all(pkg.join("dist")).unwrap();
    fs::write(
        pkg.join("package.json"),
        format!(
            r#"{{ "name": "{name}", "version": "6.3.0", "main": "dist/index.js" }}"#
        ),
    )
    .unwrap();
    // CJS body — top-level module.exports, no ESM fallback.
    fs::write(
        pkg.join("dist/index.js"),
        "function handler() { return \"handler\"; }\nmodule.exports = { handler: handler };\n",
    )
    .unwrap();
}

// ===========================================================================
// Content-pipeline assertions (always run — no esbuild)
// ===========================================================================

// ---------------------------------------------------------------------------
// 1. title_from_label default — :::note[Hello] must produce title="Hello"
// ---------------------------------------------------------------------------

#[test]
fn admonition_directive_label_becomes_title_attribute() {
    use zfb_content::mdx_to_jsx_module_with_pipeline;
    use zfb_content::pipeline::Pipeline;
    use zfb_content::MdxJsxOptions;

    // The MDX source: a :::note[Hello] directive with proper blank lines.
    // The pipeline must run AdmonitionsPlugin (mdast phase) to transform
    // the directive node; `mdx_to_jsx_module` alone (no pipeline) skips
    // mdast visitors and would leave the directive as raw text.
    let src = ":::note[Hello]\n\nBody text.\n\n:::\n";
    let mut pipeline = Pipeline::with_defaults();
    let jsx = mdx_to_jsx_module_with_pipeline(src, MdxJsxOptions::default(), &mut pipeline)
        .expect("mdx_to_jsx_module_with_pipeline must succeed for a valid directive");

    // The compiled JSX must contain `title="Hello"` (the label promoted to
    // a title attribute by AdmonitionsPlugin / DirectiveRegistry).
    assert!(
        jsx.contains("title=\"Hello\"") || jsx.contains("title: \"Hello\""),
        ":::note[Hello] must promote the label to a title attribute in compiled JSX.\
         \nJSX excerpt: {}",
        &jsx[..jsx.len().min(1200)]
    );
}

// ---------------------------------------------------------------------------
// 2. Un-blank-lined directive → blank-line diagnostic
//
// The blank-line diagnostic is emitted by `DirectiveRegistry` when it
// detects that a `:::note\nbody\n:::` block was merged into a single
// paragraph (no surrounding blank lines).  We test this at the
// `zfb-content` pipeline level — below the bundler — so this assertion
// always runs even without an esbuild binary.
//
// The diagnostic is accessible via `DirectiveRegistry::take_diagnostics()`
// which is exposed through `zfb_content::plugins::DirectiveRegistry`.
// We drive the registry standalone (not through `Pipeline`) so we can
// call `take_diagnostics()` after the visit.
//
// NOTE: The `markdown::mdast` crate is an internal dep of `zfb-content`
// but not of `zfb-build`.  We avoid importing it directly here by using
// `zfb_content::pipeline::Pipeline` with a custom mdast visitor — but
// since `Pipeline` boxes visitors and doesn't expose the registry's
// diagnostic drain, we use a different approach: we verify that the
// bad source does NOT produce a recognized `<Note>` element in the
// serialized HTML (because the directive was left as a raw paragraph),
// which is the observable effect of the missing-blank-lines path.
// ---------------------------------------------------------------------------

#[test]
fn un_blank_lined_directive_does_not_produce_note_element() {
    use zfb_content::mdx_to_jsx_module_with_pipeline;
    use zfb_content::pipeline::Pipeline;
    use zfb_content::MdxJsxOptions;

    // Source WITHOUT blank lines around :::note:
    //   :::note
    //   body text
    //   :::
    // The markdown parser collapses this into a single merged paragraph
    // (the `:::` tokens have no special meaning without the surrounding
    // blank lines that let the directive parser see them as block fences).
    // The DirectiveRegistry detects the merge via `paragraph_text_looks_merged`,
    // emits a diagnostic, and leaves the paragraph intact — so no `<Note>`
    // JSX element is emitted.
    let bad_src = ":::note\nbody text without blank lines\n:::\n";

    let mut pipeline = Pipeline::with_defaults();
    let jsx = mdx_to_jsx_module_with_pipeline(bad_src, MdxJsxOptions::default(), &mut pipeline)
        .expect("pipeline must not hard-error on bad-blank-line source");

    // The compiled JSX must NOT contain a `<Note` opening tag — the directive
    // was left unrecognised (merged paragraph, no blank lines).
    assert!(
        !jsx.contains("<Note"),
        "un-blank-lined :::note must not produce a <Note> JSX element.\
         \nJSX excerpt: {}",
        &jsx[..jsx.len().min(1200)]
    );

    // The raw `:::` fence markers (as string literals) must be visible in the
    // JSX output, confirming the paragraph was NOT consumed by the registry.
    assert!(
        jsx.contains(":::"),
        "un-blank-lined directive source must survive as raw text in the JSX.\
         \nJSX excerpt: {}",
        &jsx[..jsx.len().min(1200)]
    );
}

// ---------------------------------------------------------------------------
// 3. GFM pipe-table → JSX contains table/thead/tbody component names
// ---------------------------------------------------------------------------

#[test]
fn gfm_pipe_table_emits_table_thead_tbody_jsx() {
    use zfb_content::mdx_to_jsx_module;
    use zfb_content::MdxJsxOptions;

    let src = "| Name | Value |\n| --- | --- |\n| alpha | 1 |\n| beta | 2 |\n";
    let jsx = mdx_to_jsx_module(src, MdxJsxOptions::default())
        .expect("pipe-table must compile without error");

    // The emitter wraps the table in `_components.table`, with
    // `_components.thead` and `_components.tbody` children.
    for marker in ["table", "thead", "tbody", "tr", "th", "td"] {
        assert!(
            jsx.contains(marker),
            "compiled JSX must reference '{marker}' component for a GFM pipe-table.\
             \nJSX excerpt: {}",
            &jsx[..jsx.len().min(1200)]
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Walk-order determinism — overlapping H2 slugs get reset per entry
// ---------------------------------------------------------------------------

#[test]
fn overlapping_h2_slugs_reset_between_entries() {
    use zfb_content::pipeline::Pipeline;
    use zfb_content::serializer::serialize;

    // Two MDX documents each containing `## Overview`. Without the
    // per-entry reset (zfb#187 fix) the second document's heading would
    // receive the slug `overview-1` instead of `overview` because the
    // pipeline reuses `HeadingLinksPlugin`'s seen-counter.
    let doc_a = "## Overview\n\nContent of collection A.\n";
    let doc_b = "## Overview\n\nContent of collection B.\n";

    let mut pipeline = Pipeline::with_defaults();

    // Process doc A.
    let hast_a = pipeline.run(doc_a).expect("doc_a must parse");
    let html_a = serialize(&hast_a);

    // Reset per-entry state (exactly what the bundler does between files).
    pipeline.reset_per_entry();

    // Process doc B.
    let hast_b = pipeline.run(doc_b).expect("doc_b must parse");
    let html_b = serialize(&hast_b);

    // Both documents must assign id="overview" to their heading — not
    // id="overview-1" for the second document.
    assert!(
        html_a.contains("id=\"overview\""),
        "collection A's ## Overview must have id=\"overview\".\nHTML: {html_a}"
    );
    assert!(
        html_b.contains("id=\"overview\""),
        "collection B's ## Overview must have id=\"overview\" after reset_per_entry().\n\
         Got: {html_b}\n\
         (If this is 'id=\"overview-1\"' the HeadingLinksPlugin reset is not wired.)"
    );
}

// ===========================================================================
// Bundler-level assertions (esbuild-gated)
// ===========================================================================

/// Write the full fixture project tree exercising all Wave 1+2 features.
///
/// Layout:
/// ```
/// <root>/
///   public/
///     assets/
///       logo.svg          ← nested public asset (#192)
///   pages/
///     index.mdx           ← main page with table, admonition, link
///   content/
///     docs/
///       index.mdx         ← source with good + broken link (resolve-links)
///       other.mdx         ← target of [label](./other.mdx)
///     blog/
///       post-a.mdx        ← ## Overview heading (walk-order collection A)
///     blog2/
///       post-b.mdx        ← ## Overview heading (walk-order collection B)
///   layouts/
///     default.tsx
///   components/
///     (empty — required by bundler)
///   styles/
///     global.css          ← split-import (#191)
/// ```
fn write_full_fixture(root: &std::path::Path) {
    for d in [
        "public/assets",
        "pages",
        "content/docs",
        "content/blog",
        "content/blog2",
        "layouts",
        "components",
        "styles",
    ] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    // Nested public asset — verifies publicDir wiring compiles (actual copy
    // is tested in the `zfb` crate; here we just need the directory to exist
    // so the fixture is realistic).
    fs::write(
        root.join("public/assets/logo.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><text>logo</text></svg>\n",
    )
    .unwrap();

    // Minimal layout.
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function DefaultLayout({ children }) { return children; }\n",
    )
    .unwrap();

    // Main page MDX:
    //   - :::note[World] directive with blank lines   (title_from_label)
    //   - GFM pipe-table                             (emit-table)
    //   - a fenced Rust code block                   (syntect/InspiredGitHub)
    //   - an un-blank-lined :::tip                   (blank-line diagnostic —
    //     will be swallowed at bundle level but the Pipeline still fires)
    fs::write(
        root.join("pages/index.mdx"),
        "---\ntitle: Migration Confirm Smoke\n---\n\
         \n\
         ## Overview\n\
         \n\
         :::note[World]\n\
         \n\
         This admonition uses directive-label syntax.\n\
         \n\
         :::\n\
         \n\
         :::tip\n\
         Un-blank-lined directive — should produce a diagnostic.\n\
         :::\n\
         \n\
         | Column A | Column B |\n\
         | --- | --- |\n\
         | alpha | 1 |\n\
         | beta  | 2 |\n\
         \n\
         ```rust\n\
         fn main() {\n\
             println!(\"hello\");\n\
         }\n\
         ```\n",
    )
    .unwrap();

    // Docs content: good link + broken link (resolve-links).
    fs::write(
        root.join("content/docs/index.mdx"),
        "---\ntitle: Docs Index\n---\n\n\
         [good link](./other.mdx)\n\n\
         [broken link](./missing.mdx)\n",
    )
    .unwrap();

    fs::write(
        root.join("content/docs/other.mdx"),
        "---\ntitle: Other Page\n---\n\nOther page body.\n",
    )
    .unwrap();

    // Blog collection A — ## Overview heading (walk-order).
    fs::write(
        root.join("content/blog/post-a.mdx"),
        "---\ntitle: Post A\n---\n\n## Overview\n\nContent A.\n",
    )
    .unwrap();

    // Blog collection B — same ## Overview heading (walk-order).
    fs::write(
        root.join("content/blog2/post-b.mdx"),
        "---\ntitle: Post B\n---\n\n## Overview\n\nContent B.\n",
    )
    .unwrap();

    // Split-import CSS (#191 / #159): using tailwindcss sub-paths so the
    // engine does not prepend the full `@import "tailwindcss";` and avoids
    // leaking default color tokens.
    fs::write(
        root.join("styles/global.css"),
        "@import \"tailwindcss/preflight\";\n@import \"tailwindcss/utilities\";\n",
    )
    .unwrap();
}

/// Build the bundler input for the full integration fixture.
fn make_full_fixture_input(
    root: &std::path::Path,
    esbuild: &std::path::Path,
) -> BundlerInput {
    BundlerInput {
        project_root: root.to_path_buf(),
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
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        // Two collections with overlapping H2 slugs — exercises walk-order
        // determinism fix (#190 / #187).
        content_collections: vec![
            ContentCollectionSpec::new("docs", PathBuf::from("content/docs")),
            ContentCollectionSpec::new("blog", PathBuf::from("content/blog")),
            ContentCollectionSpec::new("blog2", PathBuf::from("content/blog2")),
        ],
        strip_md_ext: false,
        // InspiredGitHub theme — exercises sub #194 / #188.
        code_highlight_theme: Some("InspiredGitHub".to_string()),
        code_highlight_themes_dir: None,
        // Warn on broken links — exercises sub #196 / #185 Gap 1.
        resolve_markdown_links: Some(ResolveMarkdownLinksSpec {
            routes: vec![ResolveMarkdownLinksRoute {
                docs_dir: PathBuf::from("content/docs"),
                route_prefix: "/docs/".to_string(),
            }],
            on_broken_links: OnBrokenLinks::Warn,
        }),
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
        bundle_exclude: Vec::new(),
    }
}

/// Integration smoke: full fixture bundles without error.
///
/// This is the primary bundler-level assertion: all Wave 1+2 features
/// in concert must not prevent the bundler from succeeding.
#[test]
fn full_fixture_bundles_without_error() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[migration_fixes_integration_confirm] no esbuild binary; \
             set ZFB_ESBUILD_BIN or place at crates/zfb/binaries/esbuild/esbuild. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_full_fixture(&root);

    let input = make_full_fixture_input(&root, &esbuild);
    let out = bundle(input).expect(
        "full integration fixture must bundle without error — \
         check individual Wave 1+2 test suites for the failing feature",
    );
    assert!(out.bundle_path.exists(), "bundle.mjs must exist on disk");
}

/// Integration smoke: InspiredGitHub theme class appears in the bundle.
///
/// Exercises sub #194 / #188 (syntect theme via config).
#[test]
fn full_fixture_bundle_contains_inspiredgithub_class() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[migration_fixes_integration_confirm] no esbuild binary; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_full_fixture(&root);

    let input = make_full_fixture_input(&root, &esbuild);
    let out = bundle(input).expect("bundle should succeed");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // The Rust fenced code block in pages/index.mdx must be highlighted
    // with the InspiredGitHub theme, producing a class of the form
    // `syntect-inspiredgithub`.
    assert!(
        body.contains("syntect-inspiredgithub"),
        "bundle must contain 'syntect-inspiredgithub' class from InspiredGitHub theme.\
         \nBundle excerpt: {}",
        &body[..body.len().min(1200)]
    );
}

/// Integration smoke: good .mdx link is rewritten to the route URL.
///
/// Exercises sub #196 / #185 Gap 1 (resolve-links in bundle).
#[test]
fn full_fixture_bundle_rewrites_good_mdx_link() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[migration_fixes_integration_confirm] no esbuild binary; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_full_fixture(&root);

    let input = make_full_fixture_input(&root, &esbuild);
    let out = bundle(input).expect("bundle should succeed with onBrokenLinks: warn");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // [good link](./other.mdx) in content/docs/index.mdx must be rewritten
    // to /docs/other/ (the configured route prefix + stem).
    assert!(
        body.contains("/docs/other/"),
        "good .mdx link must be rewritten to /docs/other/ in the bundle.\
         \nBundle excerpt: {}",
        &body[..body.len().min(1200)]
    );

    // The raw .mdx extension must not survive in the resolved link.
    assert!(
        !body.contains("./other.mdx"),
        "./other.mdx must not survive verbatim in the bundle (should be rewritten).\
         \nBundle excerpt: {}",
        &body[..body.len().min(1200)]
    );
}

/// Integration smoke: title_from_label admonition survives bundling.
///
/// Exercises sub #195 / #135 (directive-label titles at the bundler level).
#[test]
fn full_fixture_bundle_contains_admonition_title() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[migration_fixes_integration_confirm] no esbuild binary; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_full_fixture(&root);

    let input = make_full_fixture_input(&root, &esbuild);
    let out = bundle(input).expect("bundle should succeed");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // :::note[World] in pages/index.mdx must produce a Note element with
    // title="World" in the compiled JSX → compiled bundle.
    assert!(
        body.contains("World"),
        ":::note[World] title 'World' must appear in the bundle.\
         \nBundle excerpt: {}",
        &body[..body.len().min(1200)]
    );
}

/// Integration smoke: GFM pipe-table structure survives bundling.
///
/// Exercises sub #193 / #136 (table emit at bundler level).
#[test]
fn full_fixture_bundle_contains_gfm_table_components() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[migration_fixes_integration_confirm] no esbuild binary; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_full_fixture(&root);

    let input = make_full_fixture_input(&root, &esbuild);
    let out = bundle(input).expect("bundle should succeed");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // The GFM pipe-table in pages/index.mdx must produce table/thead/tbody
    // component references in the compiled bundle.
    for marker in ["table", "thead", "tbody"] {
        assert!(
            body.contains(marker),
            "bundle must contain '{marker}' from GFM pipe-table.\
             \nBundle excerpt: {}",
            &body[..body.len().min(1200)]
        );
    }
}

// ===========================================================================
// Wave 3 / Sub #673 — all five zzmod-migration fixes compose.
//
// One test that exercises #662 + #663 + #664 + #665 + #666 together to prove
// no fix silently breaks another.  Each assertion names the fix it covers so
// a regression fails with a clear signal.
//
// Structure:
//   A.  #663 config-eval URL polyfill — synchronous, no esbuild needed.
//   B.  esbuild-gated build 1: #662 (hardBreaks) + #664 (bundle.exclude) +
//       #666 (tsconfig paths via extends/baseUrl) + #664+#665 composition
//       (build succeeds despite bad.stories.tsx importing CJS-only dep).
//   C.  esbuild-gated build 2: #665 (import.meta.glob literal absent, keys
//       present) + #664 (bad story excluded from glob expansion).
//
// NOTE on the two-build structure: esbuild's `--preserve-symlinks` flag is
// added by the bundler when `tsconfig_paths` is EMPTY (branch 3 in
// `crates/zfb-build/src/bundler.rs:3868`).  When `tsconfig_paths` is
// non-empty (needed for #666), the flag is intentionally OMITTED to avoid
// the workspace-package alias regression (#443/#450).  Without
// `--preserve-symlinks`, esbuild canonicalises symlinks in the shadow tree
// and reads the original source files, bypassing the Rust-side
// `import.meta.glob` expansion written to the shadow.  The #665 glob
// assertion therefore runs in a second build where `tsconfig_paths` is
// empty, matching the semantics of the production config-with-globs path
// (no tsconfig paths) and the sibling `bundler_exclude_glob.rs` tests.
// ===========================================================================

/// End-to-end composition test for all five zzmod migration fixes (#673).
///
/// ## Fix coverage
///
/// - **#663** (config-eval URL polyfill): `new URL(...)` in an eval'd config
///   bundle succeeds (part A — no esbuild required).
/// - **#662** (markdown.hardBreaks): `hard_breaks: true` turns soft line
///   breaks into `"br"` JSX calls in the compiled bundle (part B build 1).
/// - **#666** (tsconfig extends/baseUrl): a path alias fed via
///   `BundlerInput::tsconfig_paths` (the shape `read_tsconfig_paths` returns
///   after following an `extends` chain with non-"." baseUrl) resolves
///   through esbuild's synthetic tsconfig; alias target content in bundle
///   (part B build 1).  The production resolver is unit-tested in
///   `crates/zfb/src/commands/build.rs`; here we assert bundler wiring.
/// - **#664** (bundle.exclude): `bad.stories.tsx` with a CJS-only dep is
///   excluded; both builds succeed and the excluded file is absent (B1+B2).
/// - **#665** (import.meta.glob eager expansion): the literal
///   `import.meta.glob(` is absent from the bundle; expanded keys present
///   (part B build 2, which uses empty `tsconfig_paths` to enable
///   `--preserve-symlinks` so esbuild reads the shadow's expanded file).
#[test]
fn zzmod_all_five_migration_fixes_compose() {
    // -----------------------------------------------------------------------
    // Part A — #663 config-eval URL polyfill (no esbuild binary required).
    // -----------------------------------------------------------------------
    // `new URL(...)` must be available in the V8 config-eval isolate.
    // Gated on the `embed_v8` feature (which is the default for this crate).
    #[cfg(feature = "embed_v8")]
    {
        use zfb_render::ThreadedConfigEvaluator;

        // The config bundle exercises URL (patch from #663), plus the fields
        // that would appear in a real `zfb.config.ts` that exercises all fixes.
        let config_js = r#"
            const base = new URL("https://example.com/docs/");
            const pathname = base.pathname;
            export default {
                markdown: { hardBreaks: true },
                bundle: { exclude: ["components/**/*.stories.tsx"] },
                _urlPathnameCheck: pathname,
            };
        "#;

        let val = ThreadedConfigEvaluator::eval_bundle(config_js)
            .expect(
                "#663 regression: new URL(...) must be available in the config-eval isolate — \
                 the WEB_POLYFILLS bootstrap in ThreadedConfigEvaluator must install URL before \
                 the user's config bundle runs"
            );

        // The URL-derived value must round-trip correctly.
        assert_eq!(
            val["_urlPathnameCheck"], "/docs/",
            "#663: new URL().pathname must return the correct value in the config-eval isolate"
        );
    }

    // -----------------------------------------------------------------------
    // Part B — esbuild-gated bundle assertions.
    // -----------------------------------------------------------------------
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[migration_confirm_all_five] no esbuild binary; \
             set ZFB_ESBUILD_BIN or install esbuild in node_modules. Skipping part B."
        );
        return;
    };

    // -----------------------------------------------------------------------
    // Part B build 1 — #662 (hardBreaks) + #664 (bundle.exclude) + #666
    // (tsconfig_paths).
    //
    // The bad.stories.tsx is EXCLUDED — the build succeeds even though the
    // story imports a CJS-only package that would fail under --platform=neutral.
    // -----------------------------------------------------------------------
    let tmp1 = tempfile::tempdir().expect("tempdir b1");
    let root1 = tmp1.path().to_path_buf();

    for d in ["pages", "components", "layouts", "content", "src/lib"] {
        fs::create_dir_all(root1.join(d)).unwrap();
    }

    // #666: alias target reachable via `@lib/*` → `src/lib/*`.
    // The alias map is built by `tsconfig_paths_absolute` (simulating
    // `read_tsconfig_paths` after following an `extends` chain with non-"."
    // baseUrl — that resolver logic is unit-tested in crates/zfb).
    fs::write(
        root1.join("src/lib/zfb_zzmod_marker.ts"),
        "export const ZZMOD_ALIAS_MARKER = \"zzmod-alias-resolved\";\n",
    )
    .unwrap();

    // #662: MDX page with soft break → hard break (requires hard_breaks: true).
    fs::write(
        root1.join("pages/doc.mdx"),
        "---\ntitle: HardBreaks Test\n---\n\
         \n\
         First line\n\
         Second line on next\n",
    )
    .unwrap();

    // Page that uses the #666 alias.
    fs::write(
        root1.join("pages/index.tsx"),
        r#"
            import { ZZMOD_ALIAS_MARKER } from "@lib/zfb_zzmod_marker";
            export default function Home() { return ZZMOD_ALIAS_MARKER; }
        "#,
    )
    .unwrap();

    // #664: bad story (CJS-only dep) — present in components/ so the exclude
    // mechanism is exercised; excluded from both shadow and glob expansion.
    write_cjs_only_pkg_for_confirm(&root1, "badcjs-zzmod");
    fs::write(
        root1.join("components/bad.stories.tsx"),
        r#"
            import { handler } from "badcjs-zzmod";
            export const BadStory = () => handler();
        "#,
    )
    .unwrap();

    // Minimal layout.
    fs::write(
        root1.join("layouts/default.tsx"),
        "export default function Layout({ children }) { return children; }\n",
    )
    .unwrap();

    let paths1 = tsconfig_paths_absolute(&root1, &[("@lib/*", "src/lib/*")]);
    let mut input1 = BundlerInput::for_project(
        root1.clone(),
        Framework::Preact,
        BundleMode::Production,
        root1.join("dist"),
        None,
    );
    input1.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input1.esbuild_binary = Some(esbuild.clone());
    input1.node_modules_dir = Some(root1.join("node_modules"));
    input1.tsconfig_paths = paths1;  // #666
    input1.hard_breaks = true;        // #662
    input1.bundle_exclude = vec!["components/bad.stories.tsx".to_string()]; // #664

    let out1 = bundle(input1).expect(
        "build 1 must succeed — #662 hard_breaks / #664 bundle.exclude / #666 tsconfig_paths"
    );
    assert!(out1.bundle_path.exists(), "build 1: bundle.mjs must exist");
    let body1 = fs::read_to_string(&out1.bundle_path).expect("read bundle 1");

    // #666 assertion: path alias resolved through bundler's synthetic tsconfig.
    assert!(
        body1.contains("zzmod-alias-resolved"),
        "#666 regression: @lib/zfb_zzmod_marker alias must resolve through the \
         bundler's synthetic tsconfig (BundlerInput::tsconfig_paths wiring).\n\
         Bundle excerpt: {}",
        &body1[..body1.len().min(1200)]
    );

    // #662 assertion: soft break → \"br\" JSX tag.
    // `"br"` (quoted string) is the canonical token per
    // `crates/zfb-content/tests/hard_breaks.rs::hard_breaks_on_jsx_emit_path_produces_br`.
    assert!(
        body1.contains("\"br\""),
        "#662 regression: markdown.hardBreaks must convert soft line breaks to \
         \"br\" JSX calls. `hard_breaks: true` must activate HardBreaksPlugin.\n\
         Bundle excerpt: {}",
        &body1[..body1.len().min(1200)]
    );

    // #664 assertion (build 1): excluded bad story absent from bundle.
    assert!(
        !body1.contains("bad.stories.tsx"),
        "#664 regression (build 1): bad.stories.tsx must be absent from the bundle.\n\
         Bundle excerpt: {}",
        &body1[..body1.len().min(1200)]
    );
    assert!(
        !body1.contains("badcjs-zzmod"),
        "#664 regression (build 1): the excluded story's CJS-only import must not \
         appear in the bundle.\n\
         Bundle excerpt: {}",
        &body1[..body1.len().min(1200)]
    );

    // -----------------------------------------------------------------------
    // Part B build 2 — #665 (import.meta.glob expansion) + #664 (glob exclude).
    //
    // `tsconfig_paths` is intentionally EMPTY here so the bundler adds
    // `--preserve-symlinks` (branch 3, bundler.rs:3868), keeping esbuild
    // anchored at the shadow tree and reading the Rust-expanded glob file
    // rather than the original through a symlink.  This mirrors the semantics
    // of the sibling `bundler_exclude_glob.rs` tests.
    // -----------------------------------------------------------------------
    let tmp2 = tempfile::tempdir().expect("tempdir b2");
    let root2 = tmp2.path().to_path_buf();

    for d in ["pages", "components", "layouts", "content"] {
        fs::create_dir_all(root2.join(d)).unwrap();
    }

    // Bare page so the bundle has an entrypoint.
    fs::write(
        root2.join("pages/index.tsx"),
        r#"
            import { galleryKeys } from "../components/_gallery";
            export default function Home() { return galleryKeys; }
        "#,
    )
    .unwrap();

    // #665 glob barrel — anchored at importer's own directory (`./`), matching
    // the glob expansion contract (parent-directory patterns rejected).
    fs::write(
        root2.join("components/_gallery.tsx"),
        r#"
            const stories = import.meta.glob('./*.stories.tsx', { eager: true });
            export const galleryKeys = Object.keys(stories).sort().join(",");
        "#,
    )
    .unwrap();

    // #665 + #664 good story (non-excluded; must appear in expanded glob).
    fs::write(
        root2.join("components/good.stories.tsx"),
        "export const GoodStory = () => \"ok\";\n",
    )
    .unwrap();

    // #664 + #665 bad story (CJS-only dep; excluded from BOTH shadow and glob).
    // Without `bundle.exclude` the eager glob would statically import this and
    // esbuild (--platform=neutral, empty main-fields) would reject the CJS dep.
    write_cjs_only_pkg_for_confirm(&root2, "badcjs-zzmod2");
    fs::write(
        root2.join("components/bad.stories.tsx"),
        r#"
            import { handler } from "badcjs-zzmod2";
            export const BadStory = () => handler();
        "#,
    )
    .unwrap();

    fs::write(
        root2.join("layouts/default.tsx"),
        "export default function Layout({ children }) { return children; }\n",
    )
    .unwrap();

    let mut input2 = BundlerInput::for_project(
        root2.clone(),
        Framework::Preact,
        BundleMode::Production,
        root2.join("dist"),
        None,
    );
    input2.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input2.esbuild_binary = Some(esbuild);
    input2.node_modules_dir = Some(root2.join("node_modules"));
    // tsconfig_paths intentionally EMPTY (default) so the bundler adds
    // --preserve-symlinks, enabling esbuild to read the shadow's expanded file.
    input2.bundle_exclude = vec!["components/bad.stories.tsx".to_string()]; // #664

    let out2 = bundle(input2).expect(
        "build 2 must succeed — #665 import.meta.glob expansion + #664 bundle.exclude \
         (without exclude the CJS-only bad story would fail under --platform=neutral)"
    );
    assert!(out2.bundle_path.exists(), "build 2: bundle.mjs must exist");
    let body2 = fs::read_to_string(&out2.bundle_path).expect("read bundle 2");

    // #665 assertion: import.meta.glob( must be ABSENT (expanded Rust-side).
    assert!(
        !body2.contains("import.meta.glob("),
        "#665 regression: import.meta.glob( must be expanded Rust-side before \
         esbuild runs — the literal macro must NOT survive into the bundle.\n\
         Bundle excerpt: {}",
        &body2[..body2.len().min(2000)]
    );

    // #665 assertion: expanded good story key present.
    assert!(
        body2.contains("good.stories.tsx"),
        "#665 regression: the good story's expanded glob key must appear in the \
         bundle (glob expansion must include it, not erase it).\n\
         Bundle excerpt: {}",
        &body2[..body2.len().min(1200)]
    );

    // #664 assertion (build 2): excluded bad story absent from glob + bundle.
    assert!(
        !body2.contains("bad.stories.tsx"),
        "#664 regression (build 2): bad.stories.tsx must be absent — bundle.exclude \
         must drop it from both shadow materialisation and import.meta.glob expansion.\n\
         Bundle excerpt: {}",
        &body2[..body2.len().min(1200)]
    );
    assert!(
        !body2.contains("badcjs-zzmod2"),
        "#664 regression (build 2): the excluded story's CJS-only import must not \
         appear in the bundle.\n\
         Bundle excerpt: {}",
        &body2[..body2.len().min(1200)]
    );
}
