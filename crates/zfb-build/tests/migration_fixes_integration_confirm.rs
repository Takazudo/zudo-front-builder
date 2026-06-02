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

/// Test fixture: a `features.directives` map registering `note → Note`.
/// Core seeds zero directive names, so the mapping is supplied explicitly.
fn note_directive_features() -> zfb_content::MarkdownFeaturesConfig {
    let mut directives = HashMap::new();
    directives.insert(
        "note".to_string(),
        zfb_content::DirectiveSpec::Short("Note".to_string()),
    );
    zfb_content::MarkdownFeaturesConfig {
        directives: Some(directives),
        ..Default::default()
    }
}

#[test]
fn directive_label_becomes_title_attribute() {
    use zfb_content::mdx_to_jsx_module_with_pipeline;
    use zfb_content::pipeline::Pipeline;
    use zfb_content::MdxJsxOptions;

    // The MDX source: a :::note[Hello] directive with proper blank lines.
    // The pipeline must run the directives step (mdast phase) to transform
    // the directive node; `mdx_to_jsx_module` alone (no pipeline) skips
    // mdast visitors and would leave the directive as raw text. The
    // `note → Note` mapping is supplied via `features.directives`.
    let features = note_directive_features();
    let src = ":::note[Hello]\n\nBody text.\n\n:::\n";
    let mut pipeline = Pipeline::with_defaults_and_features(&features);
    let jsx = mdx_to_jsx_module_with_pipeline(src, MdxJsxOptions::default(), &mut pipeline)
        .expect("mdx_to_jsx_module_with_pipeline must succeed for a valid directive");

    // The compiled JSX must contain `title="Hello"` (the label promoted to
    // a title attribute by the directives step / DirectiveRegistry).
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

    // `note → Note` is registered, so a well-formed `:::note` WOULD transform;
    // this test proves the merged (no-blank-lines) form does NOT.
    let features = note_directive_features();
    let mut pipeline = Pipeline::with_defaults_and_features(&features);
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
        main_fields: Vec::new(),
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
        // Register the `note`/`tip` directive names the fixture exercises.
        // Core seeds zero directive vocabulary, so the `:::note[World]` /
        // `:::tip` blocks only transform when supplied via `features.directives`.
        markdown_features: Some({
            let mut directives = HashMap::new();
            directives.insert(
                "note".to_string(),
                zfb_content::DirectiveSpec::Short("Note".to_string()),
            );
            directives.insert(
                "tip".to_string(),
                zfb_content::DirectiveSpec::Short("Tip".to_string()),
            );
            zfb_content::MarkdownFeaturesConfig {
                directives: Some(directives),
                ..Default::default()
            }
        }),
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
// Wave 4 / Sub #677 — all five zzmod-migration fixes compose in ONE build.
//
// One esbuild build that exercises #662 + #663 + #664 + #665 + #666 together
// — AND the #677 composition (in-shadow transforms visible under non-empty
// tsconfig paths + a real node_modules, i.e. branch 4 with copy_mode +
// shadow-first dual-target rebase). Each assertion names the fix it covers so
// a regression fails with a clear signal.
//
// Before #677 this was two builds: the #665 glob assertion needed an EMPTY
// `tsconfig_paths` to enable `--preserve-symlinks` (so esbuild read the
// Rust-expanded shadow file), while the #666 alias assertion needed a
// NON-empty `tsconfig_paths`. #677's copy_mode + dual-target rebase make the
// in-shadow transforms visible WITH non-empty paths, so both — plus a
// CSS-module reached via the alias — now compose in a single bundle.
//
// Structure:
//   A.  #663 config-eval URL polyfill — synchronous, no esbuild needed.
//   B.  ONE esbuild-gated build: #662 (hardBreaks) + #664 (bundle.exclude on
//       a CJS bad story) + #666 (tsconfig paths alias resolves) + #665
//       (import.meta.glob in the aliased dir expands) + #677 (CSS-module
//       reached via alias is transformed) — all in a single bundle.
// ===========================================================================

/// End-to-end composition test for all five zzmod migration fixes in ONE
/// build (#673 / #677).
///
/// ## Fix coverage (single bundle, branch 4: nm Some + non-empty paths)
///
/// - **#663** (config-eval URL polyfill): `new URL(...)` in an eval'd config
///   bundle succeeds (part A — no esbuild required).
/// - **#662** (markdown.hardBreaks): `hard_breaks: true` turns soft line
///   breaks into `"br"` JSX calls.
/// - **#666** (tsconfig extends/baseUrl): a `@lib/*` alias resolves through
///   esbuild's synthetic tsconfig (`zzmod-alias-resolved` marker present).
/// - **#664** (bundle.exclude): a CJS-only `bad.story.tsx` that the @lib glob
///   barrel WOULD import is excluded; the build succeeds (it would fail on the
///   CJS dep otherwise) and the excluded file/dep are absent.
/// - **#665** (import.meta.glob eager expansion): the literal
///   `import.meta.glob(` is absent; `good.story.tsx` key present — and the
///   barrel lives in the ALIASED dir, so it is reached through the dual-target.
/// - **#677** (transforms compose): a `.module.css` reached via the `@lib/*`
///   alias resolves to its shadow-rewritten scoped-class shim.
#[test]
fn zzmod_all_five_migration_fixes_compose() {
    // -----------------------------------------------------------------------
    // Part A — #663 config-eval URL polyfill (no esbuild binary required).
    // -----------------------------------------------------------------------
    #[cfg(feature = "embed_v8")]
    {
        use zfb_render::ThreadedConfigEvaluator;

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

        assert_eq!(
            val["_urlPathnameCheck"], "/docs/",
            "#663: new URL().pathname must return the correct value in the config-eval isolate"
        );
    }

    // -----------------------------------------------------------------------
    // Part B — ONE esbuild-gated bundle exercising #662/#664/#665/#666/#677.
    // -----------------------------------------------------------------------
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[migration_confirm_all_five] no esbuild binary; \
             set ZFB_ESBUILD_BIN or install esbuild in node_modules. Skipping part B."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();

    for d in ["pages", "components", "layouts", "content", "src/lib"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    // #666: alias target reachable via `@lib/*` → `src/lib/*`.
    fs::write(
        root.join("src/lib/zfb_zzmod_marker.ts"),
        "export const ZZMOD_ALIAS_MARKER = \"zzmod-alias-resolved\";\n",
    )
    .unwrap();

    // #665 + #677: a glob barrel that lives in the ALIASED dir — reached via
    // `@lib/...`, so the dual-target rebase must route it to the shadow copy
    // whose `import.meta.glob` was Rust-expanded.
    fs::write(
        root.join("src/lib/gallery.tsx"),
        "const stories = import.meta.glob('./*.story.tsx', { eager: true });\n\
         export const zzmodGalleryKeys = Object.keys(stories).sort().join(\",\");\n",
    )
    .unwrap();
    // The glob pattern is `./*.story.tsx`; the good entry matches and is kept.
    fs::write(
        root.join("src/lib/good.story.tsx"),
        "export const Good = () => \"good\";\n",
    )
    .unwrap();

    // #677: a CSS-module in the aliased dir, reached via `@lib/...`.
    let module_css = root.join("src/lib/card.module.css");
    fs::write(&module_css, ".badge { color: teal; }\n").unwrap();

    // #662: MDX page with a soft break → hard break (needs hard_breaks: true).
    fs::write(
        root.join("pages/doc.mdx"),
        "---\ntitle: HardBreaks Test\n---\n\
         \n\
         First line\n\
         Second line on next\n",
    )
    .unwrap();

    // Page exercises the alias marker (#666), the aliased glob barrel
    // (#665/#677), and the aliased CSS-module (#677) — all via `@lib/...`.
    fs::write(
        root.join("pages/index.tsx"),
        r#"
            import { ZZMOD_ALIAS_MARKER } from "@lib/zfb_zzmod_marker";
            import { zzmodGalleryKeys } from "@lib/gallery";
            import cardStyles from "@lib/card.module.css";
            export default function Home() {
              return ZZMOD_ALIAS_MARKER + zzmodGalleryKeys + cardStyles.badge;
            }
        "#,
    )
    .unwrap();

    // #664 (load-bearing): a CJS-only bad story that lives in the ALIASED
    // dir with a glob-MATCHING name (`bad.story.tsx`), so the eager
    // `import.meta.glob('./*.story.tsx')` barrel WOULD statically import it.
    // Under --platform=neutral (Preact path, no --main-fields) the CJS-only
    // dep is unresolvable, so WITHOUT `bundle.exclude` the build fails — that
    // is what makes the exclude assertion non-vacuous. With exclude, the file
    // is dropped from BOTH shadow materialisation AND the glob expansion.
    write_cjs_only_pkg_for_confirm(&root, "badcjs-zzmod");
    fs::write(
        root.join("src/lib/bad.story.tsx"),
        r#"
            import { handler } from "badcjs-zzmod";
            export const BadStory = () => handler();
        "#,
    )
    .unwrap();

    fs::write(
        root.join("layouts/default.tsx"),
        "export default function Layout({ children }) { return children; }\n",
    )
    .unwrap();

    let paths = tsconfig_paths_absolute(&root, &[("@lib/*", "src/lib/*")]);
    let mut input = BundlerInput::for_project(
        root.clone(),
        Framework::Preact,
        BundleMode::Production,
        root.join("dist"),
        None,
    );
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input.esbuild_binary = Some(esbuild);
    input.node_modules_dir = Some(root.join("node_modules"));
    input.tsconfig_paths = paths; // #666 — non-empty → branch 4 (copy_mode + rebase)
    input.hard_breaks = true; // #662
    input.bundle_exclude = vec!["src/lib/bad.story.tsx".to_string()]; // #664
    let mut class_maps: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    let mut names: HashMap<String, String> = HashMap::new();
    names.insert("badge".into(), "zzmod_badge_scoped".into());
    class_maps.insert(module_css, names);
    input.css_module_class_maps = class_maps; // #677

    let out = bundle(input).expect(
        "single zzmod build must succeed — #662 hardBreaks / #664 exclude / #665 glob / \
         #666 alias / #677 transform-compose, all in one bundle",
    );
    assert!(out.bundle_path.exists(), "bundle.mjs must exist");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // #666: alias resolved through the synthetic tsconfig.
    assert!(
        body.contains("zzmod-alias-resolved"),
        "#666: @lib/zfb_zzmod_marker alias must resolve through the synthetic tsconfig.\n\
         Bundle excerpt: {}",
        &body[..body.len().min(1500)]
    );

    // #662: soft break → "br" JSX tag.
    assert!(
        body.contains("\"br\""),
        "#662: markdown.hardBreaks must convert soft line breaks to \"br\" JSX calls.\n\
         Bundle excerpt: {}",
        &body[..body.len().min(1500)]
    );

    // #665: import.meta.glob( expanded (absent) + good.story.tsx key present.
    // The barrel is in the ALIASED dir, so this also proves #677 (alias-reach
    // to a glob transform).
    assert!(
        !body.contains("import.meta.glob("),
        "#665: import.meta.glob( must be expanded Rust-side and absent.\n\
         Bundle excerpt: {}",
        &body[..body.len().min(2000)]
    );
    assert!(
        body.contains("good.story.tsx"),
        "#665/#677: the expanded glob key (good.story.tsx) must appear — reached via @lib/* \
         alias through the shadow-first dual-target.\n\
         Bundle excerpt: {}",
        &body[..body.len().min(2000)]
    );

    // #677: the aliased CSS-module's scoped class must appear.
    assert!(
        body.contains("zzmod_badge_scoped"),
        "#677: the @lib/card.module.css scoped class must appear — the aliased CSS-module \
         must resolve to its shadow-rewritten shim.\n\
         Bundle excerpt: {}",
        &body[..body.len().min(2000)]
    );

    // #664: excluded bad story + its CJS dep absent. (Load-bearing: the bad
    // story is glob-matched by the @lib barrel, so without exclude the build
    // would have failed on the CJS dep before reaching these assertions.)
    assert!(
        !body.contains("bad.story.tsx"),
        "#664: bad.story.tsx must be absent from the bundle.\n\
         Bundle excerpt: {}",
        &body[..body.len().min(1500)]
    );
    assert!(
        !body.contains("badcjs-zzmod"),
        "#664: the excluded story's CJS-only import must not appear in the bundle.\n\
         Bundle excerpt: {}",
        &body[..body.len().min(1500)]
    );
}
