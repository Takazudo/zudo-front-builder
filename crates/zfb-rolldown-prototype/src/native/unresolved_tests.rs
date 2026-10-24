//! A span-less unresolved import is still re-rendered with an esbuild location.
use super::esbuild_style_unresolved;
use std::fs;

#[test]
fn automatic_runtime_import_without_a_span_points_at_the_pragma() {
    let dir = std::env::temp_dir().join(format!("zfb-native-unresolved-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let cwd = fs::canonicalize(dir).unwrap();
    fs::create_dir(cwd.join("components")).unwrap();
    fs::write(
        cwd.join("components/box.tsx"),
        "// note\n\n  /** @jsxImportSource preact */\nexport const Box = () => <p />;\n",
    )
    .unwrap();
    let importer = cwd.join("components/box.tsx");
    let rendered = [format!(
        "[UNRESOLVED_IMPORT] Could not resolve 'preact/jsx-runtime' in {}\n  │ \n  │ Help: ignored",
        importer.display()
    )];
    let report = esbuild_style_unresolved(&rendered, &[cwd.clone()]).unwrap();
    fs::remove_dir_all(&cwd).unwrap();
    assert_eq!(
        report,
        "\u{2718} [ERROR] Could not resolve \"preact/jsx-runtime\"\n\n    components/box.tsx:3:6:"
    );
}

#[test]
fn spanned_import_gets_a_zero_based_column_and_an_esbuild_code_frame() {
    let dir = std::env::temp_dir().join(format!("zfb-native-spanned-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let cwd = fs::canonicalize(dir).unwrap();
    fs::create_dir(cwd.join("components")).unwrap();
    fs::write(
        cwd.join("components/toggle.tsx"),
        "\"use client\";\nimport nope from \"./does-not-exist\"; console.log(nope);\n",
    )
    .unwrap();
    let importer = cwd.join("components/toggle.tsx");
    let rendered = [format!(
        "[UNRESOLVED_IMPORT] Could not resolve './does-not-exist' in {0}\n   ╭─[ {0}:2:18 ]\n   │\n 2 │ import nope from \"./does-not-exist\";\n───╯",
        importer.display()
    )];
    let report = esbuild_style_unresolved(&rendered, &[cwd.clone()]).unwrap();
    fs::remove_dir_all(&cwd).unwrap();
    // Byte-for-byte what esbuild printed for the same import in 4.6.0 (#4426).
    assert_eq!(
        report,
        "\u{2718} [ERROR] Could not resolve \"./does-not-exist\"\n\n    components/toggle.tsx:2:17:\n      2 \u{2502} import nope from \"./does-not-exist\"; console.log(nope);\n        \u{2575}                  ~~~~~~~~~~~~~~~~~~"
    );
}

/// Bundles `optional.ts` with Rolldown and returns the result plus the output.
fn bundle_optional(name: &str, source: &str) -> (anyhow::Result<()>, String) {
    let dir = std::env::temp_dir().join(format!("zfb-native-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let cwd = fs::canonicalize(dir).unwrap();
    fs::write(cwd.join("optional.ts"), source).unwrap();
    let mut cmd = std::process::Command::new("unused-prepared-esbuild");
    cmd.current_dir(&cwd).args([
        "optional.ts",
        "--bundle",
        "--format=esm",
        "--platform=neutral",
        "--outfile=out/bundle.mjs",
        "--metafile=out/meta.json",
    ]);
    let result = super::run_prepared(&cmd);
    let output = fs::read_to_string(cwd.join("out/bundle.mjs")).unwrap_or_default();
    fs::remove_dir_all(&cwd).unwrap();
    (result, output)
}

#[test]
fn rejection_handled_import_of_a_missing_package_stays_external() {
    let (result, output) = bundle_optional(
        "optional-peer",
        "export function load(): Promise<unknown> {\n  return import(\"not-installed-optional-peer\").then((m) => m, () => null);\n}\n",
    );
    result.unwrap();
    assert!(
        output.contains("import(\"not-installed-optional-peer\")"),
        "{output}"
    );
}

#[test]
fn unhandled_import_of_a_missing_package_still_fails_like_esbuild() {
    let (result, _) = bundle_optional(
        "unhandled-peer",
        "export function load(): Promise<unknown> {\n  return import(\"not-installed-optional-peer\");\n}\n",
    );
    let error = format!("{:#}", result.unwrap_err());
    assert!(
        error.contains("Could not resolve \"not-installed-optional-peer\""),
        "{error}"
    );
}
