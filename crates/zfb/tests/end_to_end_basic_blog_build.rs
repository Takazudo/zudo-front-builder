//! End-to-end regression test for issue #1354 (Test B) / #1361 — a full
//! `zfb build` against the in-repo `basic-blog` template, asserting the
//! emitted post/static pages and a representative sample of the enabled
//! markdown features' output markup.
//!
//! ## What this tests
//!
//! `crates/zfb/templates/basic-blog/` is the dogfood example shipped with
//! `zfb`: 3 posts (2 `.md` + `hello-zfb.mdx`) in the `blog` collection,
//! `pages/blog/[slug].tsx` (per-post route), plus the static routes
//! `pages/index.tsx` (home — lists every post), `pages/about.tsx`, and
//! `pages/404.tsx` (emits a flat `dist/404.html`, not `dist/404/index.html`
//! — see that file's own header comment). A full build emits 6 pages.
//! Tailwind is ENABLED in the template's `zfb.config.ts`, so this build also
//! exercises the tailwindcss-v4 subprocess slot staged by
//! `crates/zfb/build.rs`.
//!
//! `content/blog/markdown-showcase.md` renders one example of every
//! markdown feature the template's `zfb.config.ts` `markdown` block turns
//! on (task lists, footnotes, GitHub-style alerts, code enrichment, and the
//! heading-marker TOC — see that post and `zfb.config.ts` for the mapping),
//! so this test reads the built showcase page and asserts each feature's
//! emitted markup is present.
//!
//! This test previously lived as a dormant, `#[ignore]`d unit-test stub at
//! `crates/zfb/src/commands/build.rs` (see git history) — a lib unit test
//! can never see `CARGO_BIN_EXE_zfb` / `zfb_binary!()` (only integration
//! tests get that env var; see `zfb-test-utils/src/lib.rs`), so it could
//! never actually spawn the binary. This file replaces it with a real
//! integration test.
//!
//! ## Fixture handling
//!
//! The template is copied into a fresh `tempfile::tempdir` per run (copy
//! precedent: `content_snapshot_no_deferred.rs`) so parallel/repeat test
//! runs don't stomp on a shared `dist/`, and so the test never writes into
//! the checked-in `templates/basic-blog/` tree.
//!
//! ## No pnpm install required
//!
//! `zfb build` needs no project-level `node_modules`: when none is present,
//! the embedded `@takazudo/zfb`, `@takazudo/zfb-runtime`, `preact`, and
//! `preact-render-to-string` packages (staged into the binary by
//! `crates/zfb/build.rs`) are extracted on demand
//! (`render_pipeline::embedded_node_modules`), and the embedded esbuild /
//! tailwindcss-v4 helper binaries are extracted the same way
//! (`render_pipeline::embedded_binary`). See `build.rs:1344-1379` for the
//! fallback wiring. This mirrors `content_snapshot_no_deferred.rs`, which
//! runs `zfb build` with no extra env vars for the same reason.
//!
//! ## Skip behaviour
//!
//! Guards against a missing embedded V8 / esbuild / tailwindcss-v4 slot
//! (e.g. a build without the `embed_v8` feature, or a stripped CI image)
//! via the same known-skip indicators used across the sibling build-command
//! tests (`content_snapshot_no_deferred.rs`, `build_package_routes.rs`,
//! `dev_dep_invalidation_1284_e2e.rs`): `"embed_v8"`, `"no esbuild"`,
//! `"no tailwind"`, or the tailwindcss-v4 binary-not-found message emitted
//! by `zfb-css/src/engine.rs` (`"tailwindcss"` + `"not found"`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use zfb_test_utils::zfb_binary;

/// The 3 post slugs in `templates/basic-blog/content/blog/` (filename stem,
/// per `zfb_content::collection::derive_slug_for_file`).
const POST_SLUGS: [&str; 3] = ["hello-zfb", "markdown-showcase", "styling-with-tailwind"];

/// Substrings expected in the built `markdown-showcase` page, one per
/// enabled markdown feature it demonstrates. Paired with a label naming the
/// `zfb.config.ts` `markdown` key (or "core behaviour" for features that
/// need no config) each substring proves, so a failure names the feature
/// rather than just the missing bytes.
const SHOWCASE_FEATURE_MARKUP: [(&str, &str); 13] = [
    ("<table>", "GFM tables (markdown.gfm.table, default on)"),
    (
        "<del>",
        "GFM strikethrough (markdown.gfm.strikethrough, default on)",
    ),
    (
        // The URL-as-link-text form only an autolink produces — the post's
        // explicit `[GFM constructs docs](https://zfb...)` link shares the
        // href but not the text, so a plain-href needle would not prove
        // autolinking happened.
        "<a href=\"https://zfb.takazudomodular.com\">https://zfb.takazudomodular.com</a>",
        "GFM autolink literal (markdown.gfm.autolinkLiteral, default on)",
    ),
    (
        "type=\"checkbox\" disabled",
        "GFM task list checkbox (markdown.gfm.taskListItem)",
    ),
    (
        "data-footnotes",
        "GFM footnotes section (markdown.gfm.footnoteDefinition)",
    ),
    (
        "data-footnote-ref",
        "GFM footnote reference marker (markdown.gfm.footnoteDefinition)",
    ),
    (
        "data-component=\"note\"",
        "githubAlerts NOTE component (markdown.features.githubAlerts)",
    ),
    (
        "data-component=\"warning\"",
        "githubAlerts WARNING component (markdown.features.githubAlerts)",
    ),
    (
        "code-block-title",
        "codeEnrichment title bar (markdown.features.codeEnrichment)",
    ),
    (
        "data-line-highlight=\"true\"",
        "codeEnrichment line highlight (markdown.features.codeEnrichment)",
    ),
    (
        "data-line-diff=\"added\"",
        "codeEnrichment diff marker, added line (markdown.features.codeEnrichment)",
    ),
    (
        "data-line-diff=\"removed\"",
        "codeEnrichment diff marker, removed line (markdown.features.codeEnrichment)",
    ),
    (
        "highlighted-word",
        "codeEnrichment word highlight (markdown.features.codeEnrichment)",
    ),
];

fn template_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("templates")
        .join("basic-blog")
}

/// `true` when the non-zero build is a known-skip (no embedded V8 / no
/// esbuild / no tailwindcss-v4 binary), matching the skip pattern used
/// across the sibling build-command tests.
fn is_known_skip(combined: &str) -> bool {
    combined.contains("embed_v8")
        || combined.contains("no esbuild")
        || combined.contains("no tailwind")
        || (combined.contains("tailwindcss") && combined.contains("not found"))
}

/// Recursive directory copy (files only; creates target subdirs as needed).
/// Skips `node_modules`/`dist`/`.zfb-build` — none exist in the checked-in
/// template, but this keeps the helper safe if that ever changes.
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some("node_modules" | "dist" | ".zfb-build")) {
            continue;
        }
        let ty = entry.file_type()?;
        let dst_path = dst.join(&name);
        if ty.is_dir() {
            copy_dir(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

/// Asserts `path` exists and contains a non-empty `<main` element.
fn assert_page_has_nonempty_main(path: &Path, label: &str) {
    assert!(
        path.exists(),
        "{label}: expected emitted page at {}",
        path.display()
    );
    let html = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{label}: read {}: {e}", path.display()));
    let main_start = html.find("<main").unwrap_or_else(|| {
        panic!(
            "{label}: expected a <main element in {}\n--- html ---\n{html}",
            path.display()
        )
    });
    // Find the end of the opening `<main ...>` tag, then the very next
    // char after it — a non-empty body means the immediate next chunk
    // isn't the matching `</main>` close.
    let after_open = &html[main_start..];
    let tag_close = after_open.find('>').unwrap_or_else(|| {
        panic!(
            "{label}: malformed <main tag (no closing `>`) in {}",
            path.display()
        )
    });
    let body = &after_open[tag_close + 1..];
    let trimmed = body.trim_start();
    assert!(
        !trimmed.is_empty() && !trimmed.starts_with("</main>"),
        "{label}: <main> element must be non-empty in {}\n--- html ---\n{html}",
        path.display()
    );
}

/// Reads the built `markdown-showcase` page and asserts every
/// [`SHOWCASE_FEATURE_MARKUP`] substring is present, plus that the
/// `headingMarkerToc` feature actually generated a list right after the
/// `TOC` heading (not just that the heading itself has an `id`).
fn assert_showcase_feature_markup(dist: &Path) {
    let path = dist
        .join("blog")
        .join("markdown-showcase")
        .join("index.html");
    let html = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    for (needle, feature) in SHOWCASE_FEATURE_MARKUP {
        assert!(
            html.contains(needle),
            "markdown-showcase: expected {feature} markup `{needle}` in {}\n--- html ---\n{html}",
            path.display()
        );
    }

    // The heading-marker TOC (markdown.features.headingMarkerToc) inserts a
    // `<ul>` of links right after the `## TOC` heading (`id="toc"`) — assert
    // the list is close enough after the heading to be that generated list,
    // not some unrelated later list on the page.
    let toc_heading = html.find("id=\"toc\"").unwrap_or_else(|| {
        panic!(
            "markdown-showcase: expected the TOC heading (headingMarkerToc, id=\"toc\") in {}\n\
             --- html ---\n{html}",
            path.display()
        )
    });
    let after_heading = &html[toc_heading..];
    let list_offset = after_heading.find("<ul>").unwrap_or_else(|| {
        panic!(
            "markdown-showcase: expected a generated <ul> right after the TOC heading \
             (headingMarkerToc) in {}\n--- html ---\n{html}",
            path.display()
        )
    });
    assert!(
        list_offset < 200,
        "markdown-showcase: the <ul> found after id=\"toc\" is {list_offset} bytes away — too \
         far to be the headingMarkerToc-generated list in {}\n--- html ---\n{html}",
        path.display()
    );
}

/// Runs a full `zfb build` on a fresh copy of `templates/basic-blog/` and
/// asserts the home/about/404 static pages and the 3 post pages are all
/// emitted with a non-empty `<main>`, and that the markdown-showcase post
/// carries the emitted markup for every enabled markdown feature it
/// demonstrates.
///
/// Level 4 (real `zfb build` process e2e), tier T1 — serialized via the
/// `e2e-heavy` nextest test-group alongside the other build-command
/// binaries (`.config/nextest.toml`).
#[test]
fn end_to_end_basic_blog_build() {
    let tmp = tempfile::tempdir().expect("create tempdir for basic-blog copy");
    let root = tmp.path();
    copy_dir(&template_dir(), root).expect("copy templates/basic-blog into tempdir");

    // Inject the zfb#1729 regression fixture: an MDX post that wraps the
    // `<Note>` island (from `components/callout.tsx`, resolved through the
    // project's `mdx-components.tsx` map) around a code demo whose
    // highlighted template used to leak a bare `{\d}` expression. Pre-fix
    // that tripped the bundler's downstream-parser gate and degraded the
    // WHOLE page to the `<pre data-zfb-content-fallback>` shape; the
    // emitter now recovers the shape as a string literal so the page
    // renders normally.
    fs::write(
        root.join("content")
            .join("blog")
            .join("token-leak-demo.mdx"),
        include_str!("fixtures/token_leak_demo.mdx"),
    )
    .expect("write token-leak-demo fixture post");

    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .output()
        .expect("spawn zfb binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    if !output.status.success() {
        if is_known_skip(&combined) {
            eprintln!(
                "[end_to_end_basic_blog_build] zfb build exited non-zero with \
                 a known-skip indicator (V8/esbuild/tailwind unavailable); \
                 skipping test.\nstdout: {stdout}\nstderr: {stderr}"
            );
            return;
        }
        panic!(
            "zfb build failed unexpectedly for templates/basic-blog.\n\
             status: {:?}\nstdout: {stdout}\nstderr: {stderr}",
            output.status,
        );
    }

    let dist = root.join("dist");

    assert_page_has_nonempty_main(&dist.join("index.html"), "home page /");
    assert_page_has_nonempty_main(&dist.join("about").join("index.html"), "about page /about");
    // pages/404.tsx emits a flat dist/404.html, not dist/404/index.html —
    // see that file's own header comment and `commands/preview.rs`, which
    // serves it as the project's 404 body from that exact path.
    assert_page_has_nonempty_main(&dist.join("404.html"), "404 page");

    for slug in POST_SLUGS {
        let path = dist.join("blog").join(slug).join("index.html");
        assert_page_has_nonempty_main(&path, &format!("post page /blog/{slug}"));
    }

    assert_showcase_feature_markup(&dist);

    // zfb#1729 build-level proof: the token-leak-demo post's island +
    // code demo must render as real content, NOT degrade to the
    // whole-page fallback.
    let leak_page = dist.join("blog").join("token-leak-demo").join("index.html");
    assert_page_has_nonempty_main(&leak_page, "post page /blog/token-leak-demo");
    let leak_html = fs::read_to_string(&leak_page)
        .unwrap_or_else(|e| panic!("read {}: {e}", leak_page.display()));
    assert!(
        !leak_html.contains("data-zfb-content-fallback"),
        "zfb#1729: token-leak-demo must NOT render the <pre data-zfb-content-fallback> \
         shape — the emitter recovered the bare `{{\\d}}` expression, so the content \
         bridge must have compiled cleanly.\n--- html ---\n{leak_html}"
    );
    assert!(
        !leak_html.contains("[zfb fallback render]"),
        "zfb#1729: token-leak-demo must NOT carry the raw-body fallback marker.\n\
         --- html ---\n{leak_html}"
    );
    // The `<Note>` island actually rendered (proves real MDX evaluation,
    // not the raw-body fallback which would omit the component entirely).
    // `<Note>` resolves to `components/callout.tsx`'s `Callout` component
    // with `variant="note"`, which stamps this hook.
    assert!(
        leak_html.contains("data-component=\"note\""),
        "zfb#1729: the <Note> island (components/callout.tsx) must render in \
         token-leak-demo (its `data-component=\"note\"` hook must be present), proving \
         the page did not fall back.\n--- html ---\n{leak_html}"
    );
    // The recovered code bytes survive to the rendered HTML.
    assert!(
        leak_html.contains("\\d"),
        "zfb#1729: the recovered `\\d` code bytes must survive to the rendered HTML.\n\
         --- html ---\n{leak_html}"
    );
}
