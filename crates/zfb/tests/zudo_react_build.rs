//! Real owned-runtime SSR build, registered in the unlocked heavy lane.

use std::fs;
use std::path::Path;
use std::process::Command;
use zfb_test_utils::{locate_esbuild, zfb_binary};

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn ssr_basic_builds_through_owned_runtime() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[zudo_react_build] no esbuild binary available; skipping");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zudo-react-build/ssr-basic");
    copy_dir(&fixture, temp.path());
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(temp.path())
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn zfb build");
    assert!(
        output.status.success(),
        "zfb build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let html =
        fs::read_to_string(temp.path().join("dist/index.html")).expect("read emitted home page");
    assert!(html.contains("<h1>Owned runtime SSR</h1>"), "{html}");
    assert_eq!(
        html.matches("<!doctype html>").count() + html.matches("<!DOCTYPE html>").count(),
        1,
        "{html}"
    );
    assert!(html.contains("<title>Owned SSR fixture</title>"), "{html}");
    assert!(html.contains("/assets/"), "head assets missing: {html}");
    assert!(
        !html.contains("data-zfb-island"),
        "ordinary page must not carry hydration marker: {html}"
    );
}

#[test]
fn markdown_page_and_mdx_collection_render_through_owned_runtime() {
    let esbuild = locate_esbuild().expect("real MD/MDX build requires esbuild");
    let temp = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zudo-react-build/md-mdx");
    copy_dir(&fixture, temp.path());
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(temp.path())
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn zfb build");
    assert!(
        output.status.success(),
        "zfb build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let markdown =
        fs::read_to_string(temp.path().join("dist/about/index.html")).expect("read Markdown page");
    assert!(markdown.contains("Markdown page heading"), "{markdown}");
    assert!(markdown.contains("<meta charset=\"utf-8\""), "{markdown}");
    assert!(
        markdown.contains("<span style=\"color:"),
        "highlight markup missing or escaped: {markdown}"
    );
    assert!(
        !markdown.contains("data-zfb-content-fallback"),
        "{markdown}"
    );

    let mdx = fs::read_to_string(temp.path().join("dist/posts/entry/index.html"))
        .expect("read MDX collection page");
    assert!(mdx.contains("Collection heading"), "{mdx}");
    assert!(mdx.contains("class=\"authored-jsx\""), "{mdx}");
    assert!(mdx.contains("highlighted"), "{mdx}");
    assert!(
        mdx.contains("<span style=\"color:"),
        "highlight markup missing or escaped: {mdx}"
    );
    assert!(!mdx.contains("data-zfb-content-fallback"), "{mdx}");
}

#[test]
fn owned_island_build_emits_matching_wrappers_and_bundle() {
    let esbuild = locate_esbuild().expect("owned island build requires esbuild");
    let temp = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zudo-react-build/island");
    copy_dir(&fixture, temp.path());
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(temp.path())
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn zfb build");
    assert!(
        output.status.success(),
        "zfb build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let html = fs::read_to_string(temp.path().join("dist/index.html")).unwrap();
    let outside = html.split("<div data-zfb-island=").next().unwrap();
    assert!(outside.contains("<strong>node-free</strong>"), "{html}");
    assert!(!outside.contains("<!--zr:1:"), "{html}");
    assert!(html.contains("data-zfb-island=\"Counter\""), "{html}");
    assert!(
        html.contains("data-zfb-island-skip-ssr=\"SkipCounter\""),
        "{html}"
    );
    assert!(html.contains("data-zfb-transport=\"json/1\""), "{html}");
    assert!(
        html.contains("data-zfb-protocol=\"zudo-react/1\""),
        "{html}"
    );
    assert!(html.contains("data-zfb-build=\""), "{html}");
    let build = html
        .split("data-zfb-build=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("island build identity");
    assert!(!build.is_empty());
    assert!(
        html.contains("data-props=\"{&quot;label&quot;:&quot;ready&quot;}\""),
        "{html}"
    );
    assert!(html.contains("<!--zr:1:"), "{html}");
    assert!(html.contains("<p>Waiting</p>"), "{html}");
    let bundle = fs::read_to_string(temp.path().join("dist/assets/islands.js")).unwrap();
    assert!(
        bundle.contains(build),
        "browser bundle identity differs from SSR"
    );
    assert!(
        bundle.contains("zudo-react/1"),
        "owned runtime absent from browser bundle"
    );
    assert!(
        !bundle.contains("from \"preact\""),
        "Preact import in owned bundle"
    );
}
