use std::fs;

use zfb_css::{bundle_authored_css, scan_css_urls, AuthoredCssEngine, CssPipeline, CssPipelineConfig};

#[test]
fn package_font_from_import_reaches_emitter_as_companion() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let entry = root.join("styles/global.css");
    let package = root.join("node_modules/@acme/fonts");
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    fs::create_dir_all(package.join("files")).unwrap();
    fs::write(&entry, "@import '@acme/fonts/style.css';").unwrap();
    fs::write(package.join("package.json"), r#"{"name":"@acme/fonts","version":"1.0.0"}"#).unwrap();
    fs::write(package.join("style.css"), "@font-face{src:url('./files/a.woff2')}").unwrap();
    fs::write(package.join("files/a.woff2"), b"font bytes").unwrap();

    let bundled = bundle_authored_css(&entry, root, &fs::read_to_string(&entry).unwrap()).unwrap();
    let emitted = CssPipeline::new(
        AuthoredCssEngine::new(bundled),
        CssPipelineConfig { auto_discover_modules: false, ..CssPipelineConfig::default() },
    ).build_emitter().unwrap();
    let css = String::from_utf8(emitted.bytes).unwrap();
    let references = scan_css_urls(&css);
    assert_eq!(emitted.companions.len(), 1, "CSS: {css}; references: {references:?}");
    assert_eq!(references.len(), 1);
    assert!(references[0].decoded.starts_with("./a-"));
    assert!(references[0].decoded.ends_with(".woff2"));
    assert_eq!(emitted.companions[0].bytes, b"font bytes");
}
