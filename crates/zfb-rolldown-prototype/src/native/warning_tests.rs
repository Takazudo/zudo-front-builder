//! Esbuild-silent native warnings stay silent for the same prepared argv (#4404).
use super::run_prepared;
use serde_json::Value;
use std::{fs, process::Command};

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
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(&dir).args([
        "entry.tsx",
        "--bundle",
        "--format=esm",
        "--jsx=automatic",
        "--jsx-import-source=preact",
        "--external:preact",
        "--tsconfig=tsconfig.json",
        "--outfile=out/bundle.mjs",
        "--metafile=out/meta.json",
    ]);
    run_prepared(&cmd).unwrap();
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
