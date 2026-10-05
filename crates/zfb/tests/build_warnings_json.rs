//! Real command coverage for the versioned build warning report.
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use zfb_test_utils::{locate_esbuild, zfb_binary};

fn page(root: &Path) {
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), r#"export default function Page() { return <html><head><title>test</title></head><body>ok</body></html>; }"#).unwrap();
}

fn build(root: &Path, args: &[&str]) -> Option<Output> {
    let esbuild = locate_esbuild()?;
    let output = Command::new(zfb_binary!())
        .arg("build")
        .args(args)
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() && stderr.contains("embed_v8") {
        return None;
    }
    Some(output)
}

fn report(root: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(root.join("warnings.json")).unwrap()).unwrap()
}

#[test]
fn empty_success_and_early_config_failure_replace_stale_report() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    page(root);
    let Some(success) = build(root, &["--warnings-json", "warnings.json"]) else {
        return;
    };
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    let first = report(root);
    assert_eq!(first["schemaVersion"], 1);
    assert_eq!(first["command"], "build");
    assert_eq!(first["status"], "success");
    assert_eq!(first["diagnostics"], serde_json::json!([]));

    fs::write(root.join("zfb.config.json"), "{invalid json").unwrap();
    let failure = build(root, &["--warnings-json", "warnings.json"]).unwrap();
    assert!(!failure.status.success());
    let second = report(root);
    assert_eq!(second["status"], "failed");
    assert_eq!(second["diagnostics"], serde_json::json!([]));
}

#[test]
fn repeated_page_warnings_and_strict_failure_keep_each_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    page(root);
    fs::create_dir_all(root.join("content/docs")).unwrap();
    fs::write(root.join("content/docs/skipped.json"), "{}").unwrap();
    for name in ["one", "two"] {
        fs::write(
            root.join(format!("content/docs/{name}.mdx")),
            "---\ntitle: Test\n---\n\n<Widget client:load />\n[missing](#absent)\n",
        )
        .unwrap();
    }
    fs::write(
        root.join("zfb.config.json"),
        r#"{
      "collections": [{ "name": "docs", "path": "content/docs" }],
      "markdown": { "features": { "linkValidation": { "failOnBroken": false } } }
    }"#,
    )
    .unwrap();
    let Some(success) = build(root, &["--warnings-json", "warnings.json"]) else {
        return;
    };
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    let first = report(root);
    let warnings: Vec<_> = first["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "ZB001")
        .collect();
    assert!(warnings.len() >= 2, "{first}");
    assert!(
        warnings
            .iter()
            .any(|d| d["file"].as_str().is_some_and(|f| f.ends_with("one.mdx"))),
        "{first}"
    );
    assert!(
        warnings
            .iter()
            .any(|d| d["file"].as_str().is_some_and(|f| f.ends_with("two.mdx"))),
        "{first}"
    );
    assert!(String::from_utf8_lossy(&success.stderr).contains("zfb warn: ZB001"));
    let astro: Vec<_> = first["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "ZB014")
        .collect();
    assert_eq!(astro.len(), 2, "{first}");
    assert!(
        astro.iter().all(|d| d["line"] == 5 && d["byteColumn"] == 9),
        "{first}"
    );
    assert!(String::from_utf8_lossy(&success.stderr).contains("zfb warn: ZB014"));
    assert!(first["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"] == "ZB007"));
    assert!(String::from_utf8_lossy(&success.stderr).contains("zfb warn: ZB007"));

    let failure = build(
        root,
        &["--strict-broken", "--warnings-json", "warnings.json"],
    )
    .unwrap();
    assert!(!failure.status.success());
    let second = report(root);
    assert_eq!(second["status"], "failed");
    assert!(
        second["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "ZB007"),
        "preceding warning was lost: {second}"
    );
    assert!(
        second["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["code"] == "ZB001")
            .count()
            >= 2,
        "{second}"
    );
}

#[test]
fn plugin_final_log_lines_and_unwritable_destination() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    page(root);
    if !Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return;
    }
    fs::write(
        root.join("zfb.config.json"),
        r#"{"plugins":[{"name":"./plugin.mjs"}]}"#,
    )
    .unwrap();
    fs::write(
        root.join("plugin.mjs"),
        r#"export default {
      name: 'warning-plugin',
      postBuild(ctx) {
        ctx.logger.warn('FINAL-LOG-MARKER');
        process.stderr.write('FINAL-STDERR-MARKER\n');
        process.stdout.write('FINAL-STDOUT-MARKER\n');
      }
    };"#,
    )
    .unwrap();
    let Some(success) = build(root, &["--warnings-json", "warnings.json"]) else {
        return;
    };
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    let diagnostics = report(root)["diagnostics"].as_array().unwrap().clone();
    assert!(diagnostics
        .iter()
        .any(|d| d["code"] == "ZB010" && d["message"] == "FINAL-LOG-MARKER"));
    assert!(diagnostics.iter().any(|d| d["code"] == "ZB006"
        && d["message"]
            .as_str()
            .is_some_and(|m| m.contains("FINAL-STDERR-MARKER"))));

    assert!(diagnostics.iter().any(|d| d["code"] == "ZB006"
        && d["sourceId"] == "plugin-host:stdout"
        && d["message"]
            .as_str()
            .is_some_and(|m| m.contains("FINAL-STDOUT-MARKER"))));

    let bad = build(root, &["--warnings-json", "absent/warnings.json"]).unwrap();
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("diagnostic report"));
}

#[test]
fn wind_error_keeps_native_code_and_opaque_source_identity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(root.join("pages/index.tsx"), r#"export default function Page() { return <html><body><div className="rounded-missing">x</div></body></html>; }"#).unwrap();
    fs::write(root.join("zfb.config.json"), r#"{"wind":{"spec":1}}"#).unwrap();
    let Some(output) = build(root, &["--warnings-json", "warnings.json"]) else {
        return;
    };
    assert!(!output.status.success());
    let value = report(root);
    assert_eq!(value["status"], "failed");
    let wind = value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "ZW006")
        .unwrap_or_else(|| panic!("missing Wind diagnostic: {value}"));
    assert_eq!(wind["severity"], "error");
    assert!(wind["sourceId"].is_string(), "{wind}");
    assert!(
        wind.get("file").is_none(),
        "opaque source ID must not be labelled a file: {wind}"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("zfb error: ZW006"));
}
