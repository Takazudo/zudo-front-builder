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
    let report = esbuild_style_unresolved(&rendered, &cwd).unwrap();
    fs::remove_dir_all(&cwd).unwrap();
    assert_eq!(
        report,
        "\u{2718} [ERROR] Could not resolve \"preact/jsx-runtime\"\n\n    components/box.tsx:3:6:"
    );
}
