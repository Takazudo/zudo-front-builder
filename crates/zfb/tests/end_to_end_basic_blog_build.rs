//! End-to-end regression test for issue #1354 (Test B) / #1361 — a full
//! `zfb build` against the in-repo `basic-blog` template, asserting the
//! emitted post pages, paginated indexes, and tag pages.
//!
//! ## What this tests
//!
//! `crates/zfb/templates/basic-blog/` is the dogfood example shipped with
//! `zfb`: 5 posts (4 `.md` + `hello-zfb.mdx`) in the `blog` collection,
//! `pages/blog/[slug].tsx` (per-post route), `pages/blog/page/[page].tsx`
//! (`paginate()` with `pageSize: 3` over 5 posts → pages 1 and 2),
//! `pages/tags/[tag].tsx` (one route per unique tag — 6 tags across the
//! fixture posts: intro, tooling, perf, framework, ssr, deploy), and
//! `layouts/default.tsx` which renders `<main class="site-main">{children}</main>`.
//! Tailwind is ENABLED in the template's `zfb.config.json`, so this build
//! also exercises the tailwindcss-v4 subprocess slot staged by
//! `crates/zfb/build.rs`.
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

/// The 5 post slugs in `templates/basic-blog/content/blog/` (filename stem,
/// per `zfb_content::collection::derive_slug_for_file`).
const POST_SLUGS: [&str; 5] = [
    "deploying-static-sites",
    "dev-loop-feel",
    "hello-zfb",
    "ssr-and-islands",
    "why-rust-cli",
];

/// Paginated index pages: `pageSize: 3` over 5 posts → 2 pages.
const PAGINATED_PAGES: [u32; 2] = [1, 2];

/// The 6 unique tags across the 5 fixture posts' frontmatter.
const TAGS: [&str; 6] = ["intro", "tooling", "perf", "framework", "ssr", "deploy"];

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

/// Runs a full `zfb build` on a fresh copy of `templates/basic-blog/` and
/// asserts the 5 post pages, 2 paginated index pages, and 6 tag pages are
/// all emitted with a non-empty `<main>`.
///
/// Level 4 (real `zfb build` process e2e), tier T1 — serialized via the
/// `e2e-heavy` nextest test-group alongside the other build-command
/// binaries (`.config/nextest.toml`).
#[test]
fn end_to_end_basic_blog_build() {
    let tmp = tempfile::tempdir().expect("create tempdir for basic-blog copy");
    let root = tmp.path();
    copy_dir(&template_dir(), root).expect("copy templates/basic-blog into tempdir");

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

    for slug in POST_SLUGS {
        let path = dist.join("blog").join(slug).join("index.html");
        assert_page_has_nonempty_main(&path, &format!("post page /blog/{slug}"));
    }

    for page in PAGINATED_PAGES {
        let path = dist
            .join("blog")
            .join("page")
            .join(page.to_string())
            .join("index.html");
        assert_page_has_nonempty_main(&path, &format!("paginated index /blog/page/{page}"));
    }

    for tag in TAGS {
        let path = dist.join("tags").join(tag).join("index.html");
        assert_page_has_nonempty_main(&path, &format!("tag page /tags/{tag}"));
    }
}
