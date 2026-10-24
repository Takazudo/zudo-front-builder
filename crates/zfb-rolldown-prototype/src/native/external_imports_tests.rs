//! Kept-external chunk imports are marked `external: true` with the raw id,
//! emitted chunk imports `external: false` with the outdir-joined path (#4476).
use super::external_imports::import_entry;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};

/// Runs a splitting ESM build of `entry.js` in a fresh temp dir and returns
/// the build result plus the metafile text (empty when none was written).
fn prepared_build(
    name: &str,
    files: &[(&str, &str)],
    extra_args: &[String],
) -> (anyhow::Result<()>, String) {
    let dir = std::env::temp_dir().join(format!("zfb-native-ext-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    for (path, source) in files {
        fs::write(dir.join(path), source).unwrap();
    }
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(&dir).args([
        "entry.js",
        "--bundle",
        "--format=esm",
        "--platform=neutral",
        "--splitting=true",
        "--outdir=out",
        "--metafile=out/meta.json",
    ]);
    cmd.args(extra_args);
    let result = super::run_prepared(&cmd);
    let meta = fs::read_to_string(dir.join("out/meta.json")).unwrap_or_default();
    fs::remove_dir_all(&dir).unwrap();
    (result, meta)
}

/// Bundles `entry.js` with splitting and returns every metafile output import.
fn bundle_imports(name: &str, files: &[(&str, &str)], externals: &[&str]) -> Vec<Value> {
    let mut args = vec!["--chunk-names=chunk-[hash]".to_string()];
    args.extend(
        externals
            .iter()
            .map(|external| format!("--external:{external}")),
    );
    let (result, meta) = prepared_build(name, files, &args);
    result.unwrap();
    let meta: Value = serde_json::from_str(&meta).unwrap();
    meta["outputs"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|output| output["imports"].as_array().unwrap().clone())
        .collect()
}

fn find<'a>(imports: &'a [Value], path_suffix: &str) -> &'a Value {
    imports
        .iter()
        .find(|import| import["path"].as_str().unwrap().ends_with(path_suffix))
        .unwrap_or_else(|| panic!("no import ending {path_suffix}: {imports:?}"))
}

#[test]
fn rejection_handled_import_of_a_missing_package_is_external() {
    let imports = bundle_imports(
        "missing",
        &[(
            "entry.js",
            "export const load = () => import(\"missing-pkg\").catch(() => {});\nimport(\"./lazy.js\");\n",
        ), ("lazy.js", "export default 1;\n")],
        &[],
    );
    let missing = find(&imports, "missing-pkg");
    assert_eq!(missing["path"], "missing-pkg");
    assert_eq!(missing["external"], true);
    assert_eq!(missing["kind"], "dynamic-import");
}

#[test]
fn relative_kept_external_import_keeps_its_raw_id() {
    let imports = bundle_imports(
        "relative",
        &[(
            "entry.js",
            "export const load = () => import(\"./islands.js\");\n",
        )],
        &["./islands.js"],
    );
    let external = find(&imports, "islands.js");
    // The id is whatever Rolldown reports for the kept-external edge, verbatim.
    assert_eq!(external["path"], "islands.js");
    assert_eq!(external["external"], true);
}

#[test]
fn static_external_import_is_external() {
    let imports = bundle_imports(
        "static",
        &[(
            "entry.js",
            "import { x } from \"ext-pkg\";\nexport const y = x;\n",
        )],
        &["ext-pkg"],
    );
    let external = find(&imports, "ext-pkg");
    assert_eq!(external["path"], "ext-pkg");
    assert_eq!(external["kind"], "import-statement");
    assert_eq!(external["external"], true);
}

#[test]
fn emitted_chunk_import_is_internal_with_an_outdir_joined_path() {
    let imports = bundle_imports(
        "chunk",
        &[
            (
                "entry.js",
                "import { s } from \"./shared.js\";\nexport const a = s;\nexport const lazy = () => import(\"./lazy.js\");\n",
            ),
            ("shared.js", "export const s = { n: 1 };\n"),
            (
                "lazy.js",
                "import { s } from \"./shared.js\";\nexport default s.n;\n",
            ),
        ],
        &[],
    );
    assert!(!imports.is_empty());
    for import in &imports {
        assert_eq!(import["external"], false, "{import}");
        let path = Path::new(import["path"].as_str().unwrap());
        assert!(
            path.is_absolute() && path.parent().unwrap().ends_with("out"),
            "{import}"
        );
    }
}

fn emitted(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| name.to_string()).collect()
}

#[test]
fn copy_reference_import_is_unchanged() {
    let copies = BTreeMap::from([(
        "__ROLLDOWN_COPY_MODULE__#ref".to_string(),
        "logo.svg".to_string(),
    )]);
    let entry = import_entry(
        "__ROLLDOWN_COPY_MODULE__#ref",
        "import-statement",
        "entry.js",
        &emitted(&[]),
        &copies,
        Path::new("/out"),
    )
    .unwrap();
    assert_eq!(
        entry,
        json!({"path":"/out/logo.svg","kind":"import-statement"})
    );
}

#[test]
fn external_id_spelled_like_an_emitted_chunk_fails_naming_the_id() {
    let error = import_entry(
        "islands.js",
        "dynamic-import",
        "islands.js",
        &emitted(&["islands.js"]),
        &BTreeMap::new(),
        Path::new("/out"),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("\"islands.js\""), "{error:#}");
}

#[test]
fn import_of_another_emitted_chunk_is_not_a_collision() {
    let entry = import_entry(
        "islands.js",
        "import-statement",
        "entry.js",
        &emitted(&["islands.js"]),
        &BTreeMap::new(),
        Path::new("/out"),
    )
    .unwrap();
    assert_eq!(entry["external"], false);
    assert_eq!(entry["path"], "/out/islands.js");
}

#[test]
fn real_build_with_an_external_spelled_like_the_entry_fails_naming_the_id() {
    let (result, _) = prepared_build(
        "collide",
        &[(
            "entry.js",
            "export const load = () => import(\"./islands.js\");\n",
        )],
        &[
            "--entry-names=islands".to_string(),
            "--external:./islands.js".to_string(),
        ],
    );
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("\"islands.js\""), "{error}");
}
