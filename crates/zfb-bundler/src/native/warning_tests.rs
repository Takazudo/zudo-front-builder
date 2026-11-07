//! Esbuild-silent native warnings stay silent for the same typed request (#4404).
use super::bundle;
use crate::request::{BundleRequest, External, JsxRuntime, OutputLayout, Platform};
use serde_json::Value;
use std::fs;

#[test]
fn island_marker_and_tsconfig_jsx_source_add_no_warnings_but_others_remain() {
    let dir = std::env::temp_dir().join(format!("zfb-native-warnings-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    fs::write(
        dir.join("tsconfig.json"),
        r#"{"compilerOptions":{"jsx":"react-jsx","jsxImportSource":"preact"}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("entry.tsx"),
        "\"use client\";\nexport const view = () => <p>{eval(\"1\")}</p>;\n",
    )
    .unwrap();
    let mut request = BundleRequest::new(
        &dir,
        "entry.tsx",
        OutputLayout::file("out/bundle.mjs"),
        Platform::Browser,
    );
    request.jsx.runtime = Some(JsxRuntime::Automatic);
    request.jsx.import_source = Some("preact".into());
    request.externals.push(External::Package("preact".into()));
    request.tsconfig = Some("tsconfig.json".into());
    request.graph = Some("out/meta.json".into());
    bundle(&request).unwrap();
    let meta: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("out/meta.json")).unwrap()).unwrap();
    let warnings: Vec<&str> = meta["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    fs::remove_dir_all(&dir).unwrap();
    assert!(
        warnings
            .iter()
            .all(|w| !w.contains("CONFIGURATION_FIELD_CONFLICT")
                && !w.contains("MODULE_LEVEL_DIRECTIVE")),
        "{warnings:#?}"
    );
    assert!(warnings.iter().any(|w| w.contains("EVAL")), "{warnings:#?}");
}
