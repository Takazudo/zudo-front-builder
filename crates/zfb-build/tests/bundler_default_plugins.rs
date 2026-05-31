//! Bundler-wiring integration test for the default-plugin chain
//! (zfb#127 / #128).
//!
//! Pins the contract that
//! [`zfb_build::bundler::bundle`] threads a fully-wired content pipeline
//! through `compile_mdx_to_jsx_module_cached` at every MDX pre-compile call
//! site. Without this wiring, none of the six default plugins
//! (admonitions, CJK-friendly emphasis, heading-links, code-title,
//! mermaid, syntect) fire on built `dist/` output —
//! which is exactly the bug zfb#126 surfaced.
//!
//! ## `markdown.features` branch (#586)
//!
//! Since #586 the bundler builds its pipeline via
//! [`Pipeline::with_defaults_and_full_config`], dispatching on
//! `BundlerInput::markdown_features`. This test leaves `markdown_features`
//! at `None` — the **legacy always-on branch**, byte-for-byte equivalent to
//! [`Pipeline::with_defaults()`] — so the three markers below MUST still
//! appear (a default `zfb.config.ts` with no `features` key is unchanged by
//! #586). The opt-in `Some(..)` branch (where mermaid/etc.
//! become toggle-controlled) is covered by
//! `bundler_threads_markdown_features_through_mdx_compile` below.
//!
//! [`Pipeline::with_defaults_and_full_config`]: zfb_content::pipeline::Pipeline::with_defaults_and_full_config
//! [`Pipeline::with_defaults()`]: zfb_content::pipeline::Pipeline::with_defaults
//!
//! ## Scope vs. the wider plugin matrix
//!
//! This is a **wiring** test, not a re-test of every plugin's output.
//! The full plugin coverage matrix lives in
//! `crates/zfb-content/tests/mdx_jsx_emit_hast.rs`. Here we only need
//! to prove the bundler now passes the pipeline through. Three
//! markers — one per plugin path the source issue's test plan called
//! out — are enough signal to catch a `pipeline=None` regression.
//!
//! ## Markers (from zfb#126's test plan, inverted for the opt-in default)
//!
//! Since #586 wired `markdown.features` and the three framework features
//! became opt-in, the no-features default INVERTS two of the three markers:
//!
//! - **Admonition** (`:::note … :::`): `admonitionsPreset` is OFF by default,
//!   so the directive is NOT transformed — the `:::note` markers survive
//!   verbatim and NO `<Note>` JSX component is emitted.
//! - **Mermaid** (` ```mermaid `): `mermaid` is OFF by default, so the bundle
//!   must NOT contain `data-mermaid`.
//! - **Syntect** (a non-mermaid fenced code block): syntect is a CORE plugin
//!   (not opt-in), so the bundle must STILL contain a `syntect-` class hook
//!   (see `crates/zfb-content/src/plugins/syntect_plugin.rs`).
//!
//! Because the bundler emits JSX text and esbuild compiles it to JS,
//! the assertions look at the final bundle body — JSX string-literal
//! attributes survive verbatim in the compiled output.
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
//! After asserting all three markers, the bundler is run a second time
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

/// Build a minimal user-project tree exercising all three marker plugin
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

    // A page MDX that exercises the three marker plugin paths in one
    // file:
    //
    // 1. `:::note\n\nbody\n\n:::`          → admonition (mdast plugin)
    //                                         (blank lines are required
    //                                          per the directive parser
    //                                          — see
    //                                          `crates/zfb-content/src/plugins/directives.rs`)
    // 2. ```mermaid graph TD; A-->B; ```   → mermaid (hast plugin)
    // 3. ```rust fn main() {} ```          → syntect (hast plugin)
    //
    // A block image `![alt](pic.png)` is also present but, with the
    // image_enlarge feature removed, it now renders as a plain
    // `<p><img>` — no marker, so it is not asserted on.
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

    // Under the post-epic opt-in default (no `markdown.features` key), the three
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
    // Compare with the inherently-random shadow tempdir collapsed out (see
    // `normalize_shadow_paths`): esbuild embeds the bundler's per-run
    // `zfb-bundler-<rand>` shadow path in module-boundary comments, the only
    // thing that legitimately differs between two independent runs (#631).
    assert_eq!(
        normalize_shadow_paths(&body),
        normalize_shadow_paths(&second_body),
        "bundler output should be byte-identical across runs (deterministic \
         Pipeline::with_defaults emit), ignoring the random zfb-bundler-<rand> \
         shadow tempdir esbuild embeds in module-boundary comments"
    );
}

/// Minimal fixture exercising the mermaid plugin path through BOTH the page
/// shadow walker (`materialise_shadow`) and the content-collection walker
/// (`materialise_collection`), for the `markdown.features`-driven test below.
/// Both walkers construct their pipeline via the same
/// `with_defaults_and_full_config` entry point, so covering both proves the
/// feature toggle threads through every bundler pipeline (the snapshot walker
/// shares the same constructor — see `content_bridge.rs`).
fn write_mermaid_fixture(root: &std::path::Path) {
    for d in ["pages", "content/posts", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function DefaultLayout({ children }) { return children; }",
    )
    .unwrap();
    let mermaid_doc = "```mermaid\ngraph TD;\n  A-->B;\n```\n";
    fs::write(
        root.join("pages/index.mdx"),
        format!("---\ntitle: Mermaid Feature\n---\n\n{mermaid_doc}"),
    )
    .unwrap();
    // Collection entry → exercises `materialise_collection` under the same
    // `markdown.features` value.
    fs::write(
        root.join("content/posts/intro.mdx"),
        format!("---\ntitle: Intro\n---\n\n{mermaid_doc}"),
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
    main_fields: Vec::new(),
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

/// Truncate a long string for assert messages.
fn snippet(s: &str) -> String {
    if s.len() <= 1200 {
        return s.to_string();
    }
    format!("{}…[{} bytes truncated]", &s[..1200], s.len() - 1200)
}

/// Collapse the bundler's per-run random shadow tempdir out of a bundle body
/// so a determinism check can compare the *emitted code* rather than the
/// inherently-random temp dir name.
///
/// The bundler materialises its shadow tree in a fresh tempdir named
/// `zfb-bundler-<rand>` (`tempfile::Builder::prefix("zfb-bundler-")` in
/// `bundler.rs`). esbuild prints each module's source path as a
/// module-boundary comment, e.g.
/// `// ../../../../tmp/<base>/zfb-bundler-<rand>/pages/index.mdx`. Two
/// independent `bundle()` runs over the same project therefore differ **only**
/// in that `<rand>` segment — the only divergence observed in #631 (the same
/// class of environment-sensitive flakiness as #621). Under parallel CPU load
/// esbuild resolves the comment's relative base up through the full temp path
/// (including `<rand>`), which broke the byte-equality assert in
/// `bundler_threads_default_plugins_through_mdx_compile` even though the actual
/// code is identical.
///
/// For every embedded path, this strips the whole prefix up to **and
/// including** `zfb-bundler-<rand>/`, leaving the stable in-shadow remainder
/// (`pages/index.mdx`). Collapsing the entire prefix — not just the `<rand>`
/// token — also folds the short in-shadow form (`// entry.mjs`, which esbuild
/// emits when the relative base stays inside the shadow) onto the same shape,
/// so the two runs match regardless of which form esbuild picks under load.
/// Distinct in-shadow paths (`pages/index` vs `content/posts/intro`) are
/// preserved, so this cannot mask a genuine emit divergence.
///
/// The `MARKER` + alphanumeric-suffix shape is coupled to esbuild's path-comment
/// format and `tempfile`'s `[A-Za-z0-9]` random suffix. If either changes, this
/// silently no-ops (no panic, no false green) — the determinism check just
/// degrades back to the original intermittent flakiness rather than masking a
/// real regression, which the always-run unit test below cannot catch since it
/// pins today's format.
fn normalize_shadow_paths(s: &str) -> String {
    const MARKER: &str = "zfb-bundler-";
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(rel) = rest.find(MARKER) {
        // Walk left from the marker to the start of the path token esbuild
        // printed (the run of non-whitespace, non-quote chars), so the comment
        // lead-in (`// `) and any surrounding syntax are preserved. ASCII
        // whitespace only: esbuild paths are ASCII, and matching multibyte
        // whitespace here could split a char boundary (`i + 1`) and panic.
        let token_start = rest[..rel]
            .rfind(|c: char| c.is_ascii_whitespace() || c == '"' || c == '\'')
            .map(|i| i + 1)
            .unwrap_or(0);
        out.push_str(&rest[..token_start]);
        // Drop the random suffix after the marker plus one trailing '/', so the
        // path collapses to its in-shadow remainder.
        let after_marker = rel + MARKER.len();
        let rand_len = rest[after_marker..]
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest[after_marker..].len());
        let mut cut = after_marker + rand_len;
        if rest[cut..].starts_with('/') {
            cut += 1;
        }
        rest = &rest[cut..];
    }
    out.push_str(rest);
    out
}

/// Deterministic guard for `normalize_shadow_paths` itself. Unlike the esbuild
/// determinism test above (which needs the esbuild binary and only diverges
/// under load), this runs everywhere and pins the exact behaviour the de-flake
/// relies on: that two bodies differing only in the random shadow segment —
/// the empirically-confirmed divergence in #631 — normalize to equal, while
/// genuinely different in-shadow paths stay distinct.
#[test]
fn normalize_shadow_paths_collapses_random_tempdir() {
    // Two runs, same code, different random shadow dir — the real #631 shape.
    let body_a =
        "// ../../../../tmp/claude-501/zfb-bundler-otl6Ig/entry.mjs\nexport const x = 1;\n";
    let body_b =
        "// ../../../../tmp/claude-501/zfb-bundler-U8zj2y/entry.mjs\nexport const x = 1;\n";
    assert_eq!(
        normalize_shadow_paths(body_a),
        normalize_shadow_paths(body_b),
        "bodies differing only in the random zfb-bundler-<rand> segment must normalize to equal"
    );

    // The full-path form and the short in-shadow form must collapse together,
    // so a load-induced switch between the two forms can't reintroduce flake.
    let full = "// ../../../../tmp/claude-501/zfb-bundler-otl6Ig/entry.mjs\n";
    let short = "// entry.mjs\n";
    assert_eq!(
        normalize_shadow_paths(full),
        normalize_shadow_paths(short),
        "the full embedded path must collapse to the same shape as the short in-shadow path"
    );
    assert_eq!(
        normalize_shadow_paths(short),
        short,
        "short form is already in-shadow; left untouched"
    );

    // Distinct in-shadow paths must remain distinct — the normalizer strips the
    // random dir, never the meaningful remainder.
    let pages = "// ../../tmp/zfb-bundler-AAAAAA/pages/index.mdx\n";
    let content = "// ../../tmp/zfb-bundler-AAAAAA/content/posts/intro.mdx\n";
    assert_ne!(
        normalize_shadow_paths(pages),
        normalize_shadow_paths(content),
        "different in-shadow source paths must not be collapsed into each other"
    );
    assert_eq!(normalize_shadow_paths(pages), "// pages/index.mdx\n");

    // Multiple occurrences on different lines are all rewritten; non-path text
    // is preserved verbatim.
    let multi =
        "// ../x/zfb-bundler-AAAAAA/entry.mjs\nkeep me\n// ../x/zfb-bundler-AAAAAA/pages/a.mdx\n";
    assert_eq!(
        normalize_shadow_paths(multi),
        "// entry.mjs\nkeep me\n// pages/a.mdx\n"
    );

    // A body with no shadow marker is returned unchanged.
    let plain = "export const y = 2;\n// a normal comment\n";
    assert_eq!(normalize_shadow_paths(plain), plain);

    // Multibyte content (incl. ideographic space U+3000) before the marker must
    // not panic: the left-walk matches ASCII whitespace only, so it lands on the
    // `// ` space — a valid char boundary — instead of splitting a UTF-8 char.
    let multibyte = "// 日本語\u{3000}../x/zfb-bundler-AAAAAA/entry.mjs\n";
    assert_eq!(normalize_shadow_paths(multibyte), "// entry.mjs\n");
}
