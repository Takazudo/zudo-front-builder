//! Bundler-wiring integration test for the default-plugin chain
//! (zfb#127 / #128).
//!
//! Pins the contract that
//! [`zfb_build::bundler::bundle`] threads a fully-wired content pipeline
//! through `compile_mdx_to_jsx_module_cached` at every MDX pre-compile call
//! site. Without this wiring, none of the seven default plugins
//! (admonitions, CJK-friendly emphasis, heading-links, code-title,
//! image-enlarge, mermaid, syntect) fire on built `dist/` output —
//! which is exactly the bug zfb#126 surfaced.
//!
//! ## `markdown.features` branch (#586)
//!
//! Since #586 the bundler builds its pipeline via
//! [`Pipeline::with_defaults_and_full_config`], dispatching on
//! `BundlerInput::markdown_features`. This test leaves `markdown_features`
//! at `None` — the **legacy always-on branch**, byte-for-byte equivalent to
//! [`Pipeline::with_defaults()`] — so the four markers below MUST still
//! appear (a default `zfb.config.ts` with no `features` key is unchanged by
//! #586). The opt-in `Some(..)` branch (where mermaid/image-enlarge/etc.
//! become toggle-controlled) is covered by
//! `bundler_threads_markdown_features_through_mdx_compile` below.
//!
//! [`Pipeline::with_defaults_and_full_config`]: zfb_content::pipeline::Pipeline::with_defaults_and_full_config
//! [`Pipeline::with_defaults()`]: zfb_content::pipeline::Pipeline::with_defaults
//!
//! ## Scope vs. the wider plugin matrix
//!
//! This is a **wiring** test, not a re-test of every plugin's output.
//! The full seven-plugin coverage matrix lives in
//! `crates/zfb-content/tests/mdx_jsx_emit_hast.rs`. Here we only need
//! to prove the bundler now passes the pipeline through. Four
//! markers — one per plugin path the source issue's test plan called
//! out — are enough signal to catch a `pipeline=None` regression.
//!
//! ## Markers (from zfb#126's test plan, inverted for the opt-in default)
//!
//! Since #586 wired `markdown.features` and the four framework features
//! became opt-in, the no-features default INVERTS three of the four markers:
//!
//! - **Admonition** (`:::note … :::`): `admonitionsPreset` is OFF by default,
//!   so the directive is NOT transformed — the `:::note` markers survive
//!   verbatim and NO `<Note>` JSX component is emitted.
//! - **Mermaid** (` ```mermaid `): `mermaid` is OFF by default, so the bundle
//!   must NOT contain `data-mermaid`.
//! - **Image-enlarge** (a block-level paragraph image): `imageEnlarge` is OFF
//!   by default, so the bundle must NOT contain `zd-enlargeable`.
//! - **Syntect** (a non-mermaid fenced code block): syntect is a CORE plugin
//!   (not opt-in), so the bundle must STILL contain a `syntect-` class hook
//!   (see `crates/zfb-content/src/plugins/syntect_plugin.rs`).
//!
//! Because the bundler emits JSX text and esbuild compiles it to JS,
//! the assertions look at the final bundle body — JSX string-literal
//! attributes survive verbatim in the compiled output (e.g.
//! `class="zd-enlargeable"` becomes `className:"zd-enlargeable"` or a
//! similar string-bearing form, but the substring `zd-enlargeable`
//! survives).
//!
//! ## Esbuild gating
//!
//! Same precedence as `bundler_integration.rs`:
//! `ZFB_ESBUILD_BIN` env var → `crates/zfb/binaries/esbuild/esbuild`
//! slot → `which esbuild` PATH fallback. If no binary is available the
//! test prints a skip note and returns early.
//!
//! ## Determinism
//!
//! After asserting all four markers, the bundler is run a second time
//! against the same project tree and the two bundle bodies are
//! compared byte-for-byte. This is a regression check on
//! `Pipeline::with_defaults()` determinism, NOT a cache-path check
//! (with `pipeline = Some(&mut _)`, the cache is bypassed by design —
//! see `crates/zfb-content/src/mdx_jsx_emit.rs`).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use zfb_build::{bundle, BundleMode, BundlerInput, ContentCollectionSpec};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Build a minimal user-project tree exercising all four marker plugin
/// paths. Returns the project root.
fn write_fixture_project(root: &std::path::Path) {
    for d in ["pages", "content/posts", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    // A minimal layout — purely a relative-import sanity element.
    fs::write(
        root.join("layouts/default.tsx"),
        r#"
            export default function DefaultLayout({ children }) {
              return children;
            }
        "#,
    )
    .unwrap();

    // A page MDX that exercises the four marker plugin paths in one
    // file:
    //
    // 1. `:::note\n\nbody\n\n:::`          → admonition (mdast plugin)
    //                                         (blank lines are required
    //                                          per the directive parser
    //                                          — see
    //                                          `crates/zfb-content/src/plugins/directives.rs`)
    // 2. ```mermaid graph TD; A-->B; ```   → mermaid (hast plugin)
    // 3. ```rust fn main() {} ```          → syntect (hast plugin)
    // 4. `![alt](pic.png)` (block image)   → image-enlarge (hast plugin)
    //
    // Frontmatter is stripped by the bundler before the body reaches
    // `compile_mdx_to_jsx_module_cached`, so no special handling
    // beyond the existing `strip_yaml_frontmatter` is needed.
    fs::write(
        root.join("pages/index.mdx"),
        "---\n\
         title: Default Plugins Smoke\n\
         ---\n\
         \n\
         :::note\n\
         \n\
         admonition body\n\
         \n\
         :::\n\
         \n\
         ```mermaid\n\
         graph TD;\n\
           A-->B;\n\
         ```\n\
         \n\
         ```rust\n\
         fn main() {}\n\
         ```\n\
         \n\
         ![alt text](pic.png)\n",
    )
    .unwrap();

    // A second MDX file under a content collection so the
    // collection-shadow walker (the second `compile_mdx_to_jsx_module_cached`
    // call site at `bundler.rs:949`) is also exercised. The two
    // walkers each hoist their own `Pipeline::with_defaults()`; this
    // file proves the second one fires too.
    fs::write(
        root.join("content/posts/intro.mdx"),
        "---\ntitle: Intro\n---\n\n:::tip\n\nuse it well\n\n:::\n",
    )
    .unwrap();
}

#[test]
fn bundler_threads_default_plugins_through_mdx_compile() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_default_plugins] no esbuild binary available; \
             set ZFB_ESBUILD_BIN, place the binary at \
             crates/zfb/binaries/esbuild/esbuild, or install esbuild on PATH \
             to enable this test. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root);

    let input = make_input(&root, &esbuild, "dist");
    let out = bundle(input).expect("bundle should succeed");

    assert!(out.bundle_path.exists(), "bundle.mjs should exist");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(!body.is_empty(), "bundle should be non-empty");

    // Under the post-epic opt-in default (no `markdown.features` key), the four
    // former-Core framework features are OFF. Only the always-on Core plugins
    // (heading-links, code-title, CJK-friendly, syntect) fire. The opt-in
    // (`Some(..)`) path is covered by
    // `bundler_threads_markdown_features_through_mdx_compile` below.

    // ---- Admonition (admonitionsPreset) — OFF by default -------------
    //
    // Without the admonitions directive registry the `:::note` container is
    // not transformed: markdown-rs emits the lines as plain paragraphs, so the
    // marker survives verbatim and NO `<Note>` JSX component is emitted.
    assert!(
        body.contains(":::note"),
        "without `features.admonitionsPreset` the :::note directive is left untransformed and survives verbatim.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );
    assert!(
        !body.contains("_components.Note"),
        "no <Note> JSX component should be emitted when `features.admonitionsPreset` is off.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );

    // ---- Mermaid (mermaid) — OFF by default --------------------------
    //
    // The mermaid wrapper (`<div class="mermaid" data-mermaid>`) is only
    // emitted when `features.mermaid` is on. By default the ```mermaid fence
    // falls through to syntect like any other code block.
    assert!(
        !body.contains("data-mermaid"),
        "the mermaid wrapper must NOT be emitted when `features.mermaid` is off.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );

    // ---- Syntect (Core) — ALWAYS on ----------------------------------
    //
    // Syntect is a Core plugin (not part of the opt-in feature surface), so
    // the ```rust block is still highlighted. The `syntect-` class hook is
    // the unique marker (`<pre class="syntect-…">` per
    // `crates/zfb-content/src/plugins/syntect_plugin.rs`).
    assert!(
        body.contains("syntect-"),
        "syntect is a Core plugin and must still emit a `syntect-` class hook by default.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );

    // ---- Image-enlarge (imageEnlarge) — OFF by default ---------------
    //
    // Block-level images are only wrapped in `<figure class="zd-enlargeable">`
    // when `features.imageEnlarge` is on. By default the image stays a plain
    // `<img>`.
    assert!(
        !body.contains("zd-enlargeable"),
        "block images must NOT be wrapped in <figure class=\"zd-enlargeable\"> when `features.imageEnlarge` is off.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );

    // ---- Determinism check -------------------------------------------
    //
    // Run the bundler a second time against the same project tree and
    // assert byte-identical output. With `pipeline = Some(&mut _)` the
    // MDX cache is bypassed by design (see
    // `crates/zfb-content/src/mdx_jsx_emit.rs` near `cache_for_lookup`),
    // so this is a determinism / regression check on the feature-aware
    // default emit itself, not a cache-path check.
    let second_input = make_input(&root, &esbuild, "dist2");
    let second_out = bundle(second_input).expect("second bundle should succeed");
    let second_body =
        fs::read_to_string(&second_out.bundle_path).expect("read second bundle");
    assert_eq!(
        body, second_body,
        "bundler output should be byte-identical across runs (deterministic Pipeline::with_defaults emit)"
    );
}

/// Minimal fixture exercising only the mermaid plugin path, for the
/// `markdown.features`-driven test below. Keeping it to a single mermaid
/// fence isolates the feature toggle from the other plugins' output.
fn write_mermaid_fixture(root: &std::path::Path) {
    for d in ["pages", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function DefaultLayout({ children }) { return children; }",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.mdx"),
        "---\ntitle: Mermaid Feature\n---\n\n```mermaid\ngraph TD;\n  A-->B;\n```\n",
    )
    .unwrap();
}

/// #586 — the opt-in `Some(..)` branch of the feature-aware pipeline.
///
/// Setting `markdown.features.mermaid` flips whether the mermaid wrapper is
/// emitted, proving `BundlerInput::markdown_features` is actually threaded
/// into the MDX pre-compile pipeline (the bug the issue describes was that it
/// was parsed but dropped on the floor).
#[test]
fn bundler_threads_markdown_features_through_mdx_compile() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_default_plugins] no esbuild binary available; skipping \
             markdown.features wiring test."
        );
        return;
    };

    // features.mermaid: true → the mermaid wrapper is emitted.
    let tmp_on = tempfile::tempdir().expect("tempdir");
    let root_on = tmp_on.path().to_path_buf();
    write_mermaid_fixture(&root_on);
    let mut input_on = make_input(&root_on, &esbuild, "dist");
    input_on.content_collections = Vec::new();
    input_on.markdown_features = Some(zfb_content::MarkdownFeaturesConfig {
        mermaid: Some(zfb_content::FeatureToggle::Bool(true)),
        ..Default::default()
    });
    let out_on = bundle(input_on).expect("bundle (mermaid on) should succeed");
    let body_on = fs::read_to_string(&out_on.bundle_path).expect("read bundle");
    assert!(
        body_on.contains("data-mermaid"),
        "features.mermaid:true must emit the mermaid wrapper (data-mermaid).\n--- bundle excerpt ---\n{}",
        snippet(&body_on)
    );

    // features.mermaid: false → the wrapper is NOT emitted (the fence falls
    // through to syntect like any other code block).
    let tmp_off = tempfile::tempdir().expect("tempdir");
    let root_off = tmp_off.path().to_path_buf();
    write_mermaid_fixture(&root_off);
    let mut input_off = make_input(&root_off, &esbuild, "dist");
    input_off.content_collections = Vec::new();
    input_off.markdown_features = Some(zfb_content::MarkdownFeaturesConfig {
        mermaid: Some(zfb_content::FeatureToggle::Bool(false)),
        ..Default::default()
    });
    let out_off = bundle(input_off).expect("bundle (mermaid off) should succeed");
    let body_off = fs::read_to_string(&out_off.bundle_path).expect("read bundle");
    assert!(
        !body_off.contains("data-mermaid"),
        "features.mermaid:false must NOT emit the mermaid wrapper.\n--- bundle excerpt ---\n{}",
        snippet(&body_off)
    );
}

fn make_input(
    root: &std::path::Path,
    esbuild: &std::path::Path,
    outdir_name: &str,
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
        // Mark every bare specifier the synthetic entry.mjs imports
        // as external — we don't need a node_modules tree for this
        // wiring test.
        external: vec![
            "preact".into(),
            "preact-render-to-string".into(),
            "@takazudo/zfb-runtime".into(),
        ],
        outdir: root.join(outdir_name),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        // Wire the `posts` collection so the second pre-compile call
        // site (the collection-shadow walker at `bundler.rs:949`)
        // also runs. Without a registered collection, the `content/`
        // tree is materialised into the page shadow but not iterated
        // through `materialise_collection`.
        content_collections: vec![ContentCollectionSpec::new(
            "posts",
            PathBuf::from("content/posts"),
        )],
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
        markdown_features: None,
        plugin_alias_entries: Vec::new(),
        plugin_virtual_modules: Vec::new(),
        worker_only_routes: None,
        bundle_basename: None,
        css_module_class_maps: std::collections::HashMap::new(),
    }
}

/// Truncate a long string for assert messages.
fn snippet(s: &str) -> String {
    if s.len() <= 1200 {
        return s.to_string();
    }
    format!("{}…[{} bytes truncated]", &s[..1200], s.len() - 1200)
}
