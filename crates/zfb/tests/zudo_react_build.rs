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
