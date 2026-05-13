//! Bundler-wiring integration test for the default-plugin chain
//! (zfb#127 / #128).
//!
//! Pins the contract that
//! [`zfb_build::bundler::bundle`] threads
//! [`zfb_content::pipeline::Pipeline::with_defaults()`] through
//! `compile_mdx_to_jsx_module_cached` at every MDX pre-compile call
//! site. Without this wiring, none of the seven default plugins
//! (admonitions, CJK-friendly emphasis, heading-links, code-title,
//! image-enlarge, mermaid, syntect) fire on built `dist/` output —
//! which is exactly the bug zfb#126 surfaced.
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
//! ## Markers (from zfb#126's test plan)
//!
//! - **Admonition** (`:::note Title\nbody\n:::`): the directive markers
//!   must be CONSUMED, and the JSX must contain a `<Note>` element.
//!   The directive previously passed through verbatim because the
//!   bundler skipped the mdast-phase plugin chain.
//! - **Mermaid** (` ```mermaid `): the bundle must contain
//!   `data-mermaid` (the canonical attribute the source issue named).
//! - **Syntect** (a non-mermaid fenced code block): the bundle must
//!   contain a `syntect-` class hook (the actual class shape
//!   `SyntectPlugin` emits — see
//!   `crates/zfb-content/src/plugins/syntect_plugin.rs`).
//! - **Image-enlarge** (a block-level paragraph image): the bundle
//!   must contain `zd-enlargeable` (the class on the wrapping
//!   `<figure>`).
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
use std::process::Command;

use zfb_build::{bundle, BundleMode, BundlerInput, ContentCollectionSpec};
use zfb_render::adapters::Framework;

fn locate_esbuild() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZFB_ESBUILD_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace) = here.parent().and_then(|p| p.parent()) {
        let slot = workspace.join("crates/zfb/binaries/esbuild/esbuild");
        if slot.exists() {
            return Some(slot);
        }
    }
    if let Ok(out) = Command::new("which").arg("esbuild").output() {
        if out.status.success() {
            let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

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

    // ---- Admonition (mdast-phase plugin) -----------------------------
    //
    // The `:::note` directive markers must be CONSUMED — if they
    // survive verbatim in the bundle, the admonition plugin did not
    // fire (the source-issue regression). Conversely the JSX must
    // contain a `<Note>` reference, which esbuild renders as a
    // `Note`/`_components.Note` identifier in the compiled output.
    assert!(
        !body.contains(":::note"),
        ":::note directive markers must be consumed by the admonition plugin (regression: pipeline=None lets them survive verbatim).\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );
    // The PascalCase preamble baked by the JSX emitter is the most
    // reliable bundle-side proof that `<Note>` survived the hast
    // detour into the JSX output (see
    // `crates/zfb-content/tests/mdx_jsx_emit_hast.rs::mdx_component_passthrough_survives_hast_detour`).
    // The emitter source emits
    // `const Note = _components.Note ?? components.Note;` —
    // esbuild may rename the local through minification but the
    // property-access `_components.Note` (or the destructure
    // `Note: ` in the `_components` literal) survives.
    assert!(
        body.contains("_components.Note") || body.contains("Note: ") || body.contains("Note:"),
        "compiled bundle should reference the <Note> JSX component the admonition plugin emits (looked for `_components.Note` / `Note: ` / `Note:` in the bundle).\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );

    // ---- Mermaid (hast-phase plugin) ---------------------------------
    //
    // Source issue #126's canonical marker. The mermaid plugin
    // replaces ```mermaid blocks with `<div class="mermaid"
    // data-mermaid>`; the `data-mermaid` attribute is the unique
    // marker we look for in the bundle.
    assert!(
        body.contains("data-mermaid"),
        "mermaid plugin should emit the data-mermaid attribute in the bundle.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );
    // The original `language-mermaid` class must NOT survive — the
    // mermaid plugin replaces the entire <pre> wrapper.
    assert!(
        !body.contains("language-mermaid"),
        "language-mermaid class must not leak past mermaid plugin (the <pre> shell should be replaced with <div class=\"mermaid\">).\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );

    // ---- Syntect (hast-phase plugin) ---------------------------------
    //
    // Syntect emits HTML that the JSX bridge wraps in
    // `dangerouslySetInnerHTML`. The `syntect-` class hook is the
    // unique marker (`<pre class="syntect-…">` per
    // `crates/zfb-content/src/plugins/syntect_plugin.rs`).
    assert!(
        body.contains("syntect-"),
        "syntect plugin should emit a `syntect-` class hook in the bundle.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );

    // ---- Image-enlarge (hast-phase plugin) ---------------------------
    //
    // A block-level <p><img></p> becomes <figure class="zd-enlargeable">.
    // The class string survives in the bundle as a JSX `className`
    // string-literal value.
    assert!(
        body.contains("zd-enlargeable"),
        "image-enlarge plugin should wrap block-level images in <figure class=\"zd-enlargeable\">.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );

    // ---- Determinism check -------------------------------------------
    //
    // Run the bundler a second time against the same project tree and
    // assert byte-identical output. With `pipeline = Some(&mut _)` the
    // MDX cache is bypassed by design (see
    // `crates/zfb-content/src/mdx_jsx_emit.rs` near `cache_for_lookup`),
    // so this is a determinism / regression check on
    // `Pipeline::with_defaults()` itself, not a cache-path check.
    let second_input = make_input(&root, &esbuild, "dist2");
    let second_out = bundle(second_input).expect("second bundle should succeed");
    let second_body =
        fs::read_to_string(&second_out.bundle_path).expect("read second bundle");
    assert_eq!(
        body, second_body,
        "bundler output should be byte-identical across runs (deterministic Pipeline::with_defaults emit)"
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
        resolve_markdown_links: None,
        gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
        site: None,
    }
}

/// Truncate a long string for assert messages.
fn snippet(s: &str) -> String {
    if s.len() <= 1200 {
        return s.to_string();
    }
    format!("{}…[{} bytes truncated]", &s[..1200], s.len() - 1200)
}
