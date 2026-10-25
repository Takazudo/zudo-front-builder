//! Kept-external chunk imports are marked `external: true` with the raw id,
//! emitted chunk imports omit `external` and carry the outdir-joined path
//! (#4476, #4506).
use super::external_imports::{copied_input_edge, edge, import_entry};
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

/// Bundles `entry.js` with splitting and returns the parsed metafile.
fn bundle_meta(name: &str, files: &[(&str, &str)], extra_args: &[&str]) -> Value {
    let mut args = vec!["--chunk-names=chunk-[hash]".to_string()];
    args.extend(extra_args.iter().map(|arg| arg.to_string()));
    let (result, meta) = prepared_build(name, files, &args);
    result.unwrap();
    serde_json::from_str(&meta).unwrap()
}

/// Bundles `entry.js` with splitting and returns every metafile output import.
fn bundle_imports(name: &str, files: &[(&str, &str)], externals: &[&str]) -> Vec<Value> {
    let args: Vec<_> = externals
        .iter()
        .map(|external| format!("--external:{external}"))
        .collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    bundle_meta(name, files, &args)["outputs"]
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
        assert!(import.get("external").is_none(), "{import}");
        let path = Path::new(import["path"].as_str().unwrap());
        assert!(
            path.is_absolute() && path.parent().unwrap().ends_with("out"),
            "{import}"
        );
    }
}

#[test]
fn every_internal_output_import_is_its_output_key_byte_for_byte() {
    // #4542: static and dynamic chunk imports reuse the target's
    // `outputs` key spelling exactly; kept externals keep their raw id instead.
    let meta = bundle_meta(
        "output-keys",
        &[
            (
                "entry.js",
                "import logo from \"./logo.svg\";\nimport { s } from \"./shared.js\";\n\
                 export const a = [logo, s];\n\
                 export const lazy = () => import(\"./lazy.js\");\n\
                 export const ext = () => import(\"ext-pkg\");\n",
            ),
            ("shared.js", "export const s = { n: 1 };\n"),
            (
                "lazy.js",
                "import { s } from \"./shared.js\";\nexport default s.n;\n",
            ),
            ("logo.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n"),
        ],
        &["--loader:.svg=file", "--external:ext-pkg"],
    );
    let outputs = meta["outputs"].as_object().unwrap();
    let (mut statics, mut dynamics, mut copies, mut externals) = (0, 0, 0, 0);
    for import in outputs
        .values()
        .filter_map(|output| output["imports"].as_array())
        .flatten()
    {
        if import.get("external").is_some() {
            assert_eq!(import["external"], true, "{import}");
            externals += 1;
            continue;
        }
        let path = import["path"].as_str().unwrap();
        let target = outputs
            .get(path)
            .unwrap_or_else(|| panic!("{path} is not an outputs key: {:?}", outputs.keys()));
        if target.get("imports").is_none() {
            copies += 1;
        } else if import["kind"] == "dynamic-import" {
            dynamics += 1;
        } else {
            statics += 1;
        }
    }
    // Rolldown 24bc2d0b lists no emitted asset among chunk imports (#4505), so a
    // copy-asset edge is not expected here; its absence is the #4490 asset gap.
    assert_eq!(
        copies, 0,
        "a copy edge appeared; extend this test to cover it"
    );
    assert!(
        statics > 0 && dynamics > 0 && externals > 0,
        "static={statics} dynamic={dynamics} copy={copies} external={externals}: {meta}"
    );
}

/// The outdir-joined, natively separated spelling of an output under `/out` (#4542).
fn native_out(name: &str) -> String {
    Path::new("/out").join(name).to_str().unwrap().to_owned()
}

#[test]
fn output_paths_keep_the_host_separator() {
    #[cfg(windows)]
    assert_eq!(native_out("logo.svg"), r"/out\logo.svg");
    #[cfg(not(windows))]
    assert_eq!(native_out("logo.svg"), "/out/logo.svg");
}

fn emitted(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| name.to_string()).collect()
}

#[test]
fn every_edge_kind_marks_only_external_edges() {
    // Esbuild v0.25.12 writes `"external": true` on external edges and omits the
    // field elsewhere, on inputs and outputs alike; it never writes `false` (#4506).
    assert_eq!(
        edge(json!("dep.ts"), "import-statement", false),
        json!({"path":"dep.ts","kind":"import-statement"})
    );
    assert_eq!(
        edge(json!("ext-pkg"), "dynamic-import", true),
        json!({"path":"ext-pkg","kind":"dynamic-import","external":true})
    );
    // Input copy edge: internal, with the private marker restored later.
    assert_eq!(
        copied_input_edge(
            "__ROLLDOWN_COPY_MODULE__#ref",
            "import-statement",
            "logo.svg".to_string()
        ),
        json!({"path":"__ROLLDOWN_COPY_MODULE__#ref","kind":"import-statement","zfbCopiedOutput":"logo.svg"})
    );
    // Output imports: copy reference, emitted chunk, kept external.
    let copies = BTreeMap::from([(
        "__ROLLDOWN_COPY_MODULE__#ref".to_string(),
        "logo.svg".to_string(),
    )]);
    let output = |id| {
        import_entry(
            id,
            "import-statement",
            &emitted(&["chunk.js"]),
            &copies,
            Path::new("/out"),
        )
        .unwrap()
    };
    assert_eq!(
        output("__ROLLDOWN_COPY_MODULE__#ref"),
        json!({"path":native_out("logo.svg"),"kind":"import-statement"})
    );
    assert_eq!(
        output("chunk.js"),
        json!({"path":native_out("chunk.js"),"kind":"import-statement"})
    );
    assert_eq!(
        output("ext-pkg"),
        json!({"path":"ext-pkg","kind":"import-statement","external":true})
    );
}

#[test]
fn self_named_import_is_internal() {
    // Rolldown lists a chunk's own filename among its imports (the SSR bundle does);
    // a guard that rejected it broke every `zfb build` (smoke run 40107596010).
    let entry = import_entry(
        "bundle.mjs",
        "dynamic-import",
        &emitted(&["bundle.mjs"]),
        &BTreeMap::new(),
        Path::new("/out"),
    )
    .unwrap();
    assert!(entry.get("external").is_none(), "{entry}");
    assert_eq!(entry["path"], native_out("bundle.mjs"));
}

#[test]
fn import_of_another_emitted_chunk_is_not_a_collision() {
    let entry = import_entry(
        "islands.js",
        "import-statement",
        &emitted(&["islands.js"]),
        &BTreeMap::new(),
        Path::new("/out"),
    )
    .unwrap();
    assert!(entry.get("external").is_none(), "{entry}");
    assert_eq!(entry["path"], native_out("islands.js"));
}

#[test]
fn real_build_with_an_external_spelled_like_the_entry_is_indistinguishable() {
    // Known limitation (#4490): without external provenance from Rolldown, a kept
    // external spelled like an emitted chunk is recorded as that internal chunk.
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
    result.expect("an external spelled like the entry no longer fails the build");
}

#[test]
fn external_spelled_like_an_emitted_asset_stays_external() {
    // Rolldown never lists an asset in chunk imports (#4505), so an id spelled
    // like an emitted asset is a kept external; esbuild reports it the same way.
    let entry = import_entry(
        "logo.svg",
        "import-statement",
        &emitted(&["entry.js"]),
        &BTreeMap::new(),
        Path::new("/out"),
    )
    .unwrap();
    assert_eq!(entry["external"], true, "{entry}");
}
