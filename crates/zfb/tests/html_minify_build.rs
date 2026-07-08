//! Integration coverage for optional production HTML minification.
//!
//! This drives the real `zfb build` binary against a small fixture so the
//! config/CLI decision, production post-processing order, passthrough skips,
//! non-HTML skips, and `postBuild` observation all run through the same path a
//! consumer project uses.

use std::fs;
use std::path::Path;
use std::process::Command;

use zfb_test_utils::{locate_esbuild, zfb_binary};

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn is_known_skip(combined: &str) -> bool {
    combined.contains("embed_v8")
        || combined.contains("no esbuild")
        || combined.contains("no tailwind")
        || (combined.contains("tailwindcss") && combined.contains("not found"))
}

fn run_zfb_build(root: &Path, esbuild: &Path, extra_args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(zfb_binary!());
    cmd.arg("build")
        .args(extra_args)
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild);
    cmd.output().expect("spawn `zfb build`")
}

fn assert_build_success_or_skip(output: std::process::Output, label: &str) -> Option<()> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    if !output.status.success() && is_known_skip(&combined) {
        eprintln!(
            "[html_minify_build::{label}] known-skip indicator; skipping.\nstdout: {stdout}\nstderr: {stderr}"
        );
        return None;
    }
    assert!(
        output.status.success(),
        "[html_minify_build::{label}] expected `zfb build` to succeed; status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );
    Some(())
}

fn write_fixture(root: &Path) {
    fs::write(
        root.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "base": "/site/",
  "minifyHtml": true,
  "plugins": [{ "name": "./postbuild-observer.mjs" }]
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("postbuild-observer.mjs"),
        r#"import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export default {
  name: "postbuild-observer",
  postBuild(ctx) {
    const html = readFileSync(join(ctx.outDir, "index.html"), "utf8");
    writeFileSync(
      join(ctx.outDir, "postbuild-observed.txt"),
      [
        html.includes("\n  <section") ? "pretty" : "minified",
        html.includes("<!-- keep comment -->") ? "comment-preserved" : "comment-missing",
        html.includes("/site/about") ? "base-link-present" : "base-link-missing",
      ].join("\n"),
    );
  },
};
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"export default function Page() {
  const html = `
  <section class="hero">
    <!-- keep comment -->
    <p>  Hello    world  </p>
    <a href="/about" data-zfb-island="Counter">About</a>
  </section>
`;
  return (
    <html lang="en">
      <head><title>HTML minify fixture</title></head>
      <body dangerouslySetInnerHTML={{ __html: html }} />
    </html>
  );
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("pages/raw.html"),
        r#"---
title: Raw
---
<html>
  <body>
    <!-- raw keep -->
    <a href="/plain">Plain</a>
  </body>
</html>
"#,
    )
    .unwrap();

    fs::write(
        root.join("pages/feed.xml.tsx"),
        r#"export const contentType = "application/xml";

export default function Feed(): string {
  return `<?xml version="1.0" encoding="UTF-8"?>
<feed>
  <!-- xml keep -->
  <title>  Feed   Title  </title>
</feed>
`;
}
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("styles")).unwrap();
    fs::write(root.join("styles/global.css"), "body { color: red; }\n").unwrap();
}

#[test]
fn production_html_minification_real_build_path() {
    if !node_available() {
        eprintln!("[html_minify_build] node not on PATH; skipping postBuild fixture test.");
        return;
    }
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[html_minify_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_fixture(root);

    let output = run_zfb_build(root, &esbuild, &[]);
    if assert_build_success_or_skip(output, "enabled").is_none() {
        return;
    }

    let dist = root.join("dist");
    let index = fs::read_to_string(dist.join("index.html")).expect("read minified index");
    assert!(
        !index.contains("\n  <section"),
        "enabled minifyHtml should compact rendered HTML:\n{index}",
    );
    assert!(
        index.contains("<!-- keep comment -->"),
        "conservative preset must preserve comments:\n{index}",
    );
    assert!(
        index.contains("/site/about"),
        "link-base rewrite should run before minification:\n{index}",
    );
    assert!(
        index.contains("data-zfb-island=Counter") || index.contains(r#"data-zfb-island="Counter""#),
        "island marker attribute should survive minification:\n{index}",
    );
    assert!(
        index.contains("/site/assets/styles-") && !index.contains("/site/assets/styles.css"),
        "asset URL rewrite should happen before minification:\n{index}",
    );

    let observed =
        fs::read_to_string(dist.join("postbuild-observed.txt")).expect("postBuild observer file");
    assert!(
        observed.contains("minified")
            && observed.contains("comment-preserved")
            && observed.contains("base-link-present"),
        "postBuild should observe final minified HTML; got:\n{observed}",
    );

    let raw = fs::read_to_string(dist.join("raw/index.html")).expect("read raw passthrough");
    assert!(
        raw.contains("\n  <body>") && raw.contains(r#"href="/plain""#),
        "static .html passthrough should remain verbatim:\n{raw}",
    );

    let feed = fs::read_to_string(dist.join("feed.xml")).expect("read feed.xml");
    assert!(
        feed.contains("\n  <!-- xml keep -->") && feed.contains("  Feed   Title  "),
        "non-HTML SSG output should not be HTML-minified:\n{feed}",
    );

    let output = run_zfb_build(root, &esbuild, &["--no-minify-html"]);
    assert_build_success_or_skip(output, "cli-disable")
        .expect("second build should only skip when first build skipped");
    let disabled = fs::read_to_string(dist.join("index.html")).expect("read disabled index");
    assert!(
        disabled.contains("\n  <section"),
        "`--no-minify-html` should override config minifyHtml:true:\n{disabled}",
    );
}
