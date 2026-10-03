//! Real owned-runtime SSR builds, registered in the unlocked heavy lane. The
//! `diagnostics` fixture checks build-only render diagnostics: authored span,
//! structural context and generated stack, with no staging path as location.

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
    let assets = temp.path().join("dist/assets");
    let island_assets: Vec<_> = fs::read_dir(&assets)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("islands-")
                        && !name.starts_with("islands-chunk-")
                        && name.ends_with(".js")
                })
        })
        .collect();
    assert_eq!(
        island_assets.len(),
        1,
        "expected one hashed island entry: {island_assets:?}"
    );
    let filename = island_assets[0].file_name().unwrap().to_str().unwrap();
    assert!(
        html.contains(&format!("/assets/{filename}")),
        "HTML must load emitted bundle: {html}"
    );
    let bundle = fs::read_to_string(&island_assets[0]).unwrap();
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

#[test]
fn owned_island_build_rejects_conflicting_display_name() {
    let esbuild = locate_esbuild().expect("owned island build requires esbuild");
    let temp = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zudo-react-build/island");
    copy_dir(&fixture, temp.path());
    let component = temp.path().join("components/counter.tsx");
    let source = fs::read_to_string(&component).unwrap();
    fs::write(&component, format!("{source}\n(Counter as typeof Counter & {{ displayName?: string }}).displayName = \"Wrong\";\n")).unwrap();
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(temp.path())
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn zfb build");
    assert!(
        !output.status.success(),
        "conflicting displayName should fail the build"
    );
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        diagnostic.contains("ZR_ISLAND_IDENTITY") && diagnostic.contains("Wrong"),
        "{diagnostic}"
    );
}

/// Build-only (no preceding `zfb check`) run of the `diagnostics` fixture after
/// `edit` rewrites it. Returns the CLI's combined output of the failed build.
fn failed_diagnostics_build(edit: impl FnOnce(&Path)) -> String {
    let esbuild = locate_esbuild().expect("render diagnostic build requires esbuild");
    let temp = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zudo-react-build/diagnostics");
    copy_dir(&fixture, temp.path());
    edit(temp.path());
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(temp.path())
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn zfb build");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "build should fail:\n{text}");
    text
}

fn assert_no_internal_primary_location(output: &str) {
    for leaked in [
        "zfb-bundler-",
        "zfb-shadow-session-",
        "[zfb-render-diagnostic]",
        " — internal frame zfb-",
    ] {
        assert!(!output.contains(leaked), "leaked {leaked:?}:\n{output}");
    }
}

fn replace_in(root: &Path, file: &str, from: &str, to: &str) {
    let path = root.join(file);
    let source = fs::read_to_string(&path).unwrap();
    assert!(source.contains(from), "{file} lacks {from:?}");
    fs::write(&path, source.replace(from, to)).unwrap();
}

#[test]
fn build_only_dialect_error_reports_the_authored_element_span() {
    let output = failed_diagnostics_build(|_| {});
    assert!(
        output.contains(
            "[zudo-react] ZR_PROP_DIALECT render static-render root: use `autocomplete` instead of `autoComplete` — at components/search-field.tsx:4:7"
        ),
        "{output}"
    );
    // The worker's structural message and generated-bundle stack follow the span.
    assert!(
        output.contains(
            "ZR_PROP_DIALECT: input.autoComplete (use `autocomplete` instead of `autoComplete`) at root in static render"
        ),
        "{output}"
    );
    assert!(output.contains("file:///zfb/bundle.mjs:"), "{output}");
    assert_no_internal_primary_location(&output);
}

#[test]
fn build_only_charset_error_points_at_the_layout() {
    let output = failed_diagnostics_build(|root| {
        replace_in(
            root,
            "layouts/document.tsx",
            "<meta charset=",
            "<meta charSet=",
        );
        replace_in(
            root,
            "components/search-field.tsx",
            "autoComplete=",
            "autocomplete=",
        );
    });
    assert!(
        output.contains(
            "[zudo-react] ZR_PROP_DIALECT render static-render root[0][0]: use `charset` instead of `charSet` — at layouts/document.tsx:5:9"
        ),
        "{output}"
    );
    assert_no_internal_primary_location(&output);
}

#[test]
fn build_only_h_description_keeps_structural_context_without_a_span() {
    let output = failed_diagnostics_build(|root| {
        fs::write(
            root.join("components/search-field.tsx"),
            "import { h } from \"@takazudo/zfb/zudo-react\";\n\nexport function SearchField() {\n  return h(\"form\", { role: \"search\" }, h(\"input\", { name: \"q\", autoComplete: \"off\" }));\n}\n",
        )
        .unwrap();
    });
    assert!(
        output.contains(
            "[zudo-react] ZR_PROP_DIALECT render static-render root: use `autocomplete` instead of `autoComplete`"
        ),
        "{output}"
    );
    assert!(!output.contains(" — at "), "{output}");
    assert!(!output.contains("search-field.tsx:"), "{output}");
    assert_no_internal_primary_location(&output);
}
