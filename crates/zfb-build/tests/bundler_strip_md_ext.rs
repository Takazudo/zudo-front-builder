//! Bundler-wiring integration test for the opt-in `stripMdExt` flag
//! (zfb#127 / #129).
//!
//! Pins the contract that
//! [`zfb_build::bundler::BundlerInput::strip_md_ext`] is honoured at
//! both MDX pre-compile call sites the Sub 1 hoist exposed
//! (`materialise_shadow` for pages, `materialise_collection` for
//! collections). When the flag is `true`, `[link](other.md)` style
//! hrefs in MDX content must be rewritten to `other/` (extension
//! stripped, trailing slash appended) by
//! [`zfb_content::pipeline::Pipeline::add_strip_md_ext`]. When the
//! flag is `false` (the default), hrefs must remain untouched —
//! byte-for-byte identical to the Sub 1 outcome — so existing
//! projects keep their current shape.
//!
//! ## Esbuild gating
//!
//! Same precedence as `bundler_integration.rs` /
//! `bundler_default_plugins.rs`: `ZFB_ESBUILD_BIN` env var →
//! `crates/zfb/binaries/esbuild/esbuild` slot → `which esbuild` PATH
//! fallback. If no binary is available the test prints a skip note
//! and returns early.

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

/// Build a minimal user-project tree exercising both the page walker
/// AND the collection walker so we cover both `add_strip_md_ext`
/// call sites.
fn write_fixture_project(root: &std::path::Path) {
    for d in ["pages", "content/posts", "components", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    fs::write(
        root.join("layouts/default.tsx"),
        r#"
            export default function DefaultLayout({ children }) {
              return children;
            }
        "#,
    )
    .unwrap();

    // Page MDX with two markdown links — one to a sibling `.md` page
    // and one to a sibling `.mdx` page. With `stripMdExt: true` both
    // hrefs become extension-stripped, trailing-slash-appended forms
    // (`./other/` and `./guide/`). With it off they survive verbatim.
    fs::write(
        root.join("pages/index.mdx"),
        "---\n\
         title: Strip MdExt Smoke\n\
         ---\n\
         \n\
         [other](./other.md) and [guide](./guide.mdx)\n",
    )
    .unwrap();

    // Collection MDX exercises the second call site
    // (`materialise_collection`). Same shape.
    fs::write(
        root.join("content/posts/intro.mdx"),
        "---\ntitle: Intro\n---\n\nsee [next post](./next.md) for more.\n",
    )
    .unwrap();
}

fn make_input(
    root: &std::path::Path,
    esbuild: &std::path::Path,
    outdir_name: &str,
    strip_md_ext: bool,
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
        outdir: root.join(outdir_name),
        mode: BundleMode::Production,
        minify: false,
        esbuild_binary: Some(esbuild.to_path_buf()),
        mock_subprocess_output: None,
        content_snapshot_json: None,
        node_modules_dir: None,
        node_modules_preserve_symlinks: false,
        content_collections: vec![ContentCollectionSpec::new(
            "posts",
            PathBuf::from("content/posts"),
        )],
        strip_md_ext,
        code_highlight_theme: None,
        code_highlight_themes_dir: None,
        resolve_markdown_links: None,
        gfm_constructs: zfb_content::ResolvedGfmConstructs::default(),
        site: None,
        toc: None,
    }
}

#[test]
fn strip_md_ext_on_rewrites_internal_md_hrefs_to_trailing_slash() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_strip_md_ext] no esbuild binary available; \
             set ZFB_ESBUILD_BIN, place the binary at \
             crates/zfb/binaries/esbuild/esbuild, or install esbuild on PATH \
             to enable this test. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root);

    let input = make_input(&root, &esbuild, "dist-on", true);
    let out = bundle(input).expect("bundle should succeed with strip_md_ext=true");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(!body.is_empty(), "bundle should be non-empty");

    // ---- Page-walker call site -------------------------------------
    //
    // The two markdown links in `pages/index.mdx` should reach the
    // bundle as `./other/` and `./guide/`. The compiled bundle emits
    // them as JSX `href` string-literal attributes — esbuild may
    // rename surrounding identifiers under minification, but the
    // rewritten URL string itself survives byte-for-byte.
    assert!(
        body.contains("./other/"),
        "stripMdExt=true must rewrite `./other.md` to `./other/` \
         in the page bundle.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );
    assert!(
        body.contains("./guide/"),
        "stripMdExt=true must rewrite `./guide.mdx` to `./guide/` \
         in the page bundle.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );
    // The `.md` / `.mdx` suffixes on internal hrefs must NOT survive
    // — that is the regression `add_strip_md_ext` exists to prevent.
    assert!(
        !body.contains("./other.md"),
        "stripMdExt=true must consume the `./other.md` href; the \
         literal extension survived.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );
    assert!(
        !body.contains("./guide.mdx"),
        "stripMdExt=true must consume the `./guide.mdx` href; the \
         literal extension survived.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );

    // ---- Collection-walker call site -------------------------------
    //
    // The `content/posts/intro.mdx` body has `[next post](./next.md)`
    // — the collection walker has its own hoisted pipeline in
    // `materialise_collection`, so the flag must be threaded there
    // too. With it on, the bundled collection module references
    // `./next/` (extension stripped, trailing slash appended).
    assert!(
        body.contains("./next/"),
        "stripMdExt=true must rewrite `./next.md` to `./next/` in \
         the collection bundle (materialise_collection call site).\n\
         --- bundle excerpt ---\n{}",
        snippet(&body)
    );
    assert!(
        !body.contains("./next.md"),
        "stripMdExt=true must consume the `./next.md` href in the \
         collection module.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );
}

#[test]
fn strip_md_ext_off_preserves_md_hrefs() {
    // The default (flag absent or false) MUST be byte-for-byte
    // identical to Sub 1's outcome — `[link](./other.md)` survives
    // verbatim into the bundle. This pins the "opt-in by default"
    // contract from the issue.
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_strip_md_ext] no esbuild binary available; skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    write_fixture_project(&root);

    let input = make_input(&root, &esbuild, "dist-off", false);
    let out = bundle(input).expect("bundle should succeed with strip_md_ext=false");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // Page walker — original `.md` / `.mdx` hrefs survive untouched.
    assert!(
        body.contains("./other.md"),
        "stripMdExt=false must preserve `./other.md` verbatim \
         (default = opt-in = no rewrite).\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );
    assert!(
        body.contains("./guide.mdx"),
        "stripMdExt=false must preserve `./guide.mdx` verbatim.\n\
         --- bundle excerpt ---\n{}",
        snippet(&body)
    );
    // Collection walker — same.
    assert!(
        body.contains("./next.md"),
        "stripMdExt=false must preserve `./next.md` verbatim in the \
         collection module.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );
    // And the trailing-slash forms must NOT appear by accident — if
    // they did, the default would be silently changing existing
    // projects.
    assert!(
        !body.contains("./other/"),
        "stripMdExt=false must NOT introduce `./other/` (the rewritten \
         shape) — the default must be byte-for-byte identical to \
         Sub 1's outcome.\n--- bundle excerpt ---\n{}",
        snippet(&body)
    );
}

/// Truncate a long string for assert messages.
fn snippet(s: &str) -> String {
    if s.len() <= 1200 {
        return s.to_string();
    }
    format!("{}…[{} bytes truncated]", &s[..1200], s.len() - 1200)
}
