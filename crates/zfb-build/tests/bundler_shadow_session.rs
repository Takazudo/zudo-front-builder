//! Persistent dev shadow-tree session tests (issue #993).
//!
//! The lever's load-bearing safety property is **byte-determinism**: a
//! `bundle_with_session` call on a persistent session must produce
//! bundle bytes identical to a fresh `bundle()` over the same tree, for
//! every mutation class a dev loop can produce (the #940 skip key hashes
//! these bytes, so any divergence would corrupt skip decisions). The
//! equivalence test drives the same project through content edits,
//! file adds/deletes, page adds/deletes, and a glob-set change, checking
//! session-vs-fresh byte equality at every step.
//!
//! The remaining tests pin the session bookkeeping itself: stale-file
//! pruning (#727 hazard family), dirty-reset on error (false-invalidate,
//! never false-reuse — the #940 spirit), and diagnostics replay on
//! unchanged ticks (the "no computation skipping" property).
//!
//! ## Esbuild gating
//!
//! The byte-equivalence and dirty-reset tests need a real esbuild binary
//! and self-skip when none is available (`ZFB_ESBUILD_BIN` → workspace
//! slot → pnpm store → PATH), matching the other bundler integration
//! tests. The prune and diagnostics-replay tests run everywhere via
//! `mock_subprocess_output`.

use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{
    bundle, bundle_with_session, BundleMode, BundlerInput, ContentCollectionSpec, OnBrokenLinks,
    ResolveMarkdownLinksRoute, ResolveMarkdownLinksSpec, ShadowSession,
};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Write the base fixture project: pages (a css-module consumer, an
/// `import.meta.glob` consumer, an `.md` page), a content collection,
/// components (css module + glob-matched widgets), and a layout stub.
fn write_fixture(root: &Path) {
    for d in ["pages", "content/posts", "components/widgets", "layouts"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"import styles from "../components/card.module.css";
export default function Index() {
  return <div class={styles.card}>home</div>;
}
"#,
    )
    .unwrap();
    // The glob consumer lives in components/ because the expander only
    // supports same-or-sub-directory patterns (no `../`).
    fs::write(
        root.join("components/gallery.tsx"),
        r#"const widgets = import.meta.glob("./widgets/*.ts", { eager: true });
export function Gallery() {
  return <pre>{Object.keys(widgets).join(",")}</pre>;
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("pages/glob.tsx"),
        r#"import { Gallery } from "../components/gallery";
export default function Glob() {
  return <Gallery />;
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("pages/about.md"),
        "---\ntitle: About\n---\n\n# About\n\nplain md page body.\n",
    )
    .unwrap();
    fs::write(
        root.join("components/card.module.css"),
        ".card { color: red; }\n",
    )
    .unwrap();
    fs::write(
        root.join("components/widgets/w1.ts"),
        "export const name = \"w1\";\n",
    )
    .unwrap();
    fs::write(
        root.join("content/posts/first.mdx"),
        "# First post\n\nSome *markdown* body.\n",
    )
    .unwrap();
}

/// BundlerInput for the fixture. `outdir_name` keeps the session and
/// fresh outputs separate so they never clobber each other.
fn make_input(root: &Path, esbuild: Option<&Path>, outdir_name: &str) -> BundlerInput {
    let mut css_names = std::collections::HashMap::new();
    css_names.insert("card".to_string(), "card_scoped1".to_string());
    let mut css_maps = std::collections::HashMap::new();
    css_maps.insert(root.join("components/card.module.css"), css_names);

    BundlerInput {
        external: vec![
            "preact".into(),
            "preact-render-to-string".into(),
            "@takazudo/zfb-runtime".into(),
        ],
        esbuild_binary: esbuild.map(|p| p.to_path_buf()),
        content_collections: vec![ContentCollectionSpec::new(
            "posts",
            PathBuf::from("content/posts"),
        )],
        css_module_class_maps: css_maps,
        ..BundlerInput::for_project(
            root.to_path_buf(),
            Framework::Preact,
            BundleMode::Development,
            root.join(outdir_name),
            None,
        )
    }
}

/// Spec test 1 (#993): for every dev mutation class, the session bundle's
/// bytes must equal a fresh `bundle()`'s bytes over the same tree.
#[test]
fn shadow_session_bundle_bytes_match_fresh_bundle() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[shadow_session_bundle_bytes_match_fresh_bundle] no esbuild binary; \
             set ZFB_ESBUILD_BIN, stage the slot, or install esbuild on PATH. Skipping."
        );
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_fixture(root);

    let mut session = ShadowSession::new().unwrap();

    let mut assert_equal_bytes = |step: &str| {
        let session_out = bundle_with_session(
            make_input(root, Some(&esbuild), "dist-session"),
            Some(&mut session),
        )
        .unwrap_or_else(|e| panic!("session bundle failed at step `{step}`: {e:#}"));
        let fresh_out = bundle(make_input(root, Some(&esbuild), "dist-fresh"))
            .unwrap_or_else(|e| panic!("fresh bundle failed at step `{step}`: {e:#}"));
        let session_bytes = fs::read(&session_out.bundle_path).unwrap();
        let fresh_bytes = fs::read(&fresh_out.bundle_path).unwrap();
        assert_eq!(
            session_bytes, fresh_bytes,
            "session vs fresh bundle bytes diverged at step `{step}`"
        );
        session_bytes
    };

    // Baseline.
    assert_equal_bytes("baseline");

    // Content body edit.
    fs::write(
        root.join("content/posts/first.mdx"),
        "# First post\n\nSome *markdown* body.\n\nEdited paragraph.\n",
    )
    .unwrap();
    assert_equal_bytes("content body edit");

    // Content file added.
    fs::write(
        root.join("content/posts/second.mdx"),
        "# Second post\n\nAnother body.\n",
    )
    .unwrap();
    assert_equal_bytes("content file added");

    // Content file deleted.
    fs::remove_file(root.join("content/posts/second.mdx")).unwrap();
    assert_equal_bytes("content file deleted");

    // Page .tsx added.
    fs::write(
        root.join("pages/extra.tsx"),
        "export default function Extra() { return <p>extra</p>; }\n",
    )
    .unwrap();
    assert_equal_bytes("page tsx added");

    // Page .tsx deleted.
    fs::remove_file(root.join("pages/extra.tsx")).unwrap();
    assert_equal_bytes("page tsx deleted");

    // Glob-matched file added — must change pages/glob.tsx's expanded
    // import list even though glob.tsx itself is untouched (the expansion
    // is recomputed from the live tree every call; #993's "no computation
    // skipping" property).
    fs::write(
        root.join("components/widgets/w2.ts"),
        "export const name = \"w2\";\n",
    )
    .unwrap();
    let bytes = assert_equal_bytes("glob-matched file added");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("w2"),
        "bundle must include the newly glob-matched widget module"
    );

    // No-op tick — nothing changed since the previous step.
    assert_equal_bytes("no-op tick");
}

/// Spec test 2 (#993): a deleted source's compiled shadow artifacts (incl.
/// an `.md` page's `_zfb_md_body_*` sibling) must be pruned from the
/// persistent shadow before esbuild would run.
#[test]
fn shadow_session_prunes_deleted_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_fixture(root);

    let mut session = ShadowSession::new().unwrap();
    let input = BundlerInput {
        mock_subprocess_output: Some("export default {};".to_string()),
        ..make_input(root, None, "dist-session")
    };
    bundle_with_session(input, Some(&mut session)).unwrap();

    let shadow = session.shadow_root().to_path_buf();
    let compiled_mdx = shadow.join("content/posts/first.mdx");
    let md_shell = shadow.join("pages/about.md");
    let md_body = shadow.join("pages/_zfb_md_body_about.jsx");
    assert!(
        compiled_mdx.exists(),
        "compiled collection mdx materialised"
    );
    assert!(md_shell.exists(), "md page shell materialised");
    assert!(md_body.exists(), "md page body sibling materialised");

    fs::remove_file(root.join("content/posts/first.mdx")).unwrap();
    fs::remove_file(root.join("pages/about.md")).unwrap();

    let input = BundlerInput {
        mock_subprocess_output: Some("export default {};".to_string()),
        ..make_input(root, None, "dist-session")
    };
    bundle_with_session(input, Some(&mut session)).unwrap();

    assert!(
        !compiled_mdx.exists(),
        "deleted collection source's compiled shadow file must be pruned"
    );
    assert!(
        !md_shell.exists(),
        "deleted md page's shell module must be pruned"
    );
    assert!(
        !md_body.exists(),
        "deleted md page's _zfb_md_body_* sibling must be pruned"
    );
    // Surviving sources stay materialised.
    assert!(
        shadow.join("pages/index.tsx").exists(),
        "untouched page must survive the prune"
    );
}

/// Spec test 3 (#993): a failed call leaves the session dirty; the next
/// call must rebuild the shadow from scratch and succeed with bytes
/// identical to a fresh bundle.
#[test]
fn shadow_session_dirty_resets_after_error() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[shadow_session_dirty_resets_after_error] no esbuild binary; \
             set ZFB_ESBUILD_BIN, stage the slot, or install esbuild on PATH. Skipping."
        );
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_fixture(root);

    let broken_links_input = |outdir: &str| BundlerInput {
        resolve_markdown_links: Some(ResolveMarkdownLinksSpec {
            routes: vec![ResolveMarkdownLinksRoute {
                docs_dir: PathBuf::from("content/posts"),
                route_prefix: "/posts/".to_string(),
            }],
            on_broken_links: OnBrokenLinks::Error,
        }),
        ..make_input(root, Some(&esbuild), outdir)
    };

    let mut session = ShadowSession::new().unwrap();

    // Induce a bundle error AFTER the materialise walks populated the
    // shadow (the broken-links gate runs post-walk), so the session is
    // left with a half-trusted tree.
    fs::write(
        root.join("content/posts/first.mdx"),
        "# First post\n\n[bad](./missing.mdx)\n",
    )
    .unwrap();
    let err = bundle_with_session(broken_links_input("dist-session"), Some(&mut session))
        .expect_err("broken link with on_broken_links=Error must fail the bundle");
    assert!(
        format!("{err:#}").contains("missing.mdx"),
        "error must report the broken link: {err:#}"
    );

    // Plant a foreign file in the persistent shadow. The next call starts
    // from a dirty session, so it must wipe the tree — observable as this
    // file vanishing.
    let foreign = session
        .shadow_root()
        .join("zz-foreign-from-failed-tick.txt");
    fs::write(&foreign, "leftover").unwrap();

    // Fix the link; the next call must full-rebuild and succeed.
    fs::write(
        root.join("content/posts/first.mdx"),
        "# First post\n\nfixed body.\n",
    )
    .unwrap();
    let session_out = bundle_with_session(broken_links_input("dist-session"), Some(&mut session))
        .expect("call after a failed tick must rebuild from scratch and succeed");
    assert!(
        !foreign.exists(),
        "dirty session must wipe the shadow before reusing it"
    );

    let fresh_out = bundle(broken_links_input("dist-fresh")).expect("fresh bundle");
    assert_eq!(
        fs::read(&session_out.bundle_path).unwrap(),
        fs::read(&fresh_out.bundle_path).unwrap(),
        "post-recovery session bundle must be byte-identical to a fresh bundle"
    );
}

/// Spec test 4 (#993): diagnostics must be replayed on EVERY call,
/// including a call where no input changed — the zfb#905 compile cache
/// stores and replays them, and the session must never skip the
/// computation that drains them (the "no computation skipping" property).
#[test]
fn shadow_session_replays_diagnostics_on_unchanged_tick() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_fixture(root);
    fs::write(
        root.join("content/posts/first.mdx"),
        "# First post\n\n[bad](./missing.mdx)\n",
    )
    .unwrap();

    let broken_links_input = || BundlerInput {
        mock_subprocess_output: Some("export default {};".to_string()),
        resolve_markdown_links: Some(ResolveMarkdownLinksSpec {
            routes: vec![ResolveMarkdownLinksRoute {
                docs_dir: PathBuf::from("content/posts"),
                route_prefix: "/posts/".to_string(),
            }],
            on_broken_links: OnBrokenLinks::Error,
        }),
        ..make_input(root, None, "dist-session")
    };

    let mut session = ShadowSession::new().unwrap();

    let err1 = bundle_with_session(broken_links_input(), Some(&mut session))
        .expect_err("first call must surface the broken link");
    assert!(format!("{err1:#}").contains("missing.mdx"));

    // Nothing changed — the second call compiles via the zfb#905 cache
    // hit, which must REPLAY the stored broken-link diagnostic rather
    // than silently succeed.
    let err2 = bundle_with_session(broken_links_input(), Some(&mut session))
        .expect_err("unchanged tick must replay the broken-link diagnostic");
    assert!(
        format!("{err2:#}").contains("missing.mdx"),
        "replayed diagnostics must match the original: {err2:#}"
    );
}
