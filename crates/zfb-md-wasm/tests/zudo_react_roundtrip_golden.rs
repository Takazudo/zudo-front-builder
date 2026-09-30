//! Byte-exact compiler goldens for Markdown consumed by zudo-react.
//!
//! Regenerate the checked-in modules with
//! `ZFB_UPDATE_MD_ROUNDTRIP=1 cargo nextest run -p zfb-md-wasm --test zudo_react_roundtrip_golden`.
//!
//! Coverage note: native `compile` shares `compile_impl` with the wasm build,
//! but this test does not execute the wasm artifact or a packed md-wasm install.
//! The default-pipeline cases pin only their filename. The task-list and
//! footnote cases explicitly enable those GFM constructs because both are
//! disabled by default. This covers selected compiler output, not every
//! configurable plugin combination.

#[cfg(feature = "compile")]
#[test]
fn mdx_compile_matches_the_zudo_react_roundtrip_goldens() {
    use std::fs;
    use std::path::Path;

    const CASES: [&str; 7] = [
        "ordered-start-3",
        "ordered-resumed",
        "ordered-start-1",
        "task-list",
        "aligned-table",
        "footnote",
        "link-image-heading",
    ];

    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/zfb/src/__tests__/zudo-react/fixtures/md-roundtrip");
    let update = std::env::var("ZFB_UPDATE_MD_ROUNDTRIP").as_deref() == Ok("1");

    for case in CASES {
        let source_path = fixtures.join(format!("{case}.md"));
        let source = fs::read_to_string(&source_path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", source_path.display()));
        let options = match case {
            "task-list" => format!(
                r#"{{"filename":"{case}.md","pipeline":{{"gfm":{{"taskListItem":true}}}}}}"#
            ),
            "footnote" => format!(
                r#"{{"filename":"{case}.md","pipeline":{{"gfm":{{"footnoteDefinition":true}}}}}}"#
            ),
            _ => format!(r#"{{"filename":"{case}.md"}}"#),
        };
        let response = zfb_md_wasm::compile(&source, &options);
        let result: serde_json::Value = serde_json::from_str(&response).unwrap_or_else(|error| {
            panic!("invalid compile response for {case}: {error}\n{response}")
        });
        let diagnostics = result["diagnostics"].as_array().unwrap_or_else(|| {
            panic!("compile response for {case} has no diagnostics array: {result}")
        });
        assert!(
            diagnostics.is_empty(),
            "compile diagnostics for {case}: {diagnostics:#?}"
        );
        let code = result["code"]
            .as_str()
            .unwrap_or_else(|| panic!("compile response for {case} has no code: {result}"));

        let golden_path = fixtures.join(format!("{case}.mjs"));
        if update {
            fs::write(&golden_path, code)
                .unwrap_or_else(|error| panic!("writing {}: {error}", golden_path.display()));
        } else {
            let expected = fs::read_to_string(&golden_path)
                .unwrap_or_else(|error| panic!("reading {}: {error}", golden_path.display()));
            assert_eq!(
                code,
                expected,
                "compiler output differs from {}; regenerate with ZFB_UPDATE_MD_ROUNDTRIP=1",
                golden_path.display()
            );
        }
    }
}
