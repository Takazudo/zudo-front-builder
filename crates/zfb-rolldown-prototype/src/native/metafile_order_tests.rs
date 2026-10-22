//! Metafile imports keep source order, as esbuild's do: island graph identity
//! walks them in this order, so a path sort would make the build token depend
//! on where packages are installed (#4231).
use super::run_prepared;
use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn metafile_imports_follow_source_order_not_path_order() {
    let dir = std::env::temp_dir().join(format!("zfb-native-import-order-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    fs::write(dir.join("zeta.js"), "globalThis.zeta = 1;\n").unwrap();
    fs::write(dir.join("alpha.js"), "globalThis.alpha = 1;\n").unwrap();
    fs::write(
        dir.join("entry.js"),
        "import \"./zeta.js\";\nimport \"./alpha.js\";\n",
    )
    .unwrap();
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(&dir).args([
        "entry.js",
        "--bundle",
        "--format=esm",
        "--outfile=out/bundle.mjs",
        "--metafile=out/meta.json",
    ]);
    run_prepared(&cmd).unwrap();
    let meta: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("out/meta.json")).unwrap()).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    let entry = meta["inputs"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(key, _)| key.ends_with("entry.js"))
        .map(|(_, input)| input)
        .expect("entry input");
    let imports: Vec<&str> = entry["imports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|import| import["path"].as_str().unwrap())
        .collect();
    assert_eq!(imports.len(), 2, "{imports:?}");
    assert!(imports[0].ends_with("zeta.js"), "{imports:?}");
    assert!(imports[1].ends_with("alpha.js"), "{imports:?}");
}
