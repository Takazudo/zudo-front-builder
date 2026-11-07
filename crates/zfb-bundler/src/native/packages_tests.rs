//! `Packages::External` and `banner` (#4633): mappings resolve before package
//! externalization, and only an ordinary unresolved package stays external.
use crate::request::*;
use std::{fs, path::Path, path::PathBuf};

fn temp_dir(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "zfb-packages-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn install(dir: &Path, name: &str, marker: &str) {
    write(
        &dir.join("node_modules").join(name).join("package.json"),
        &format!(r#"{{"name":"{name}","type":"module","main":"index.js"}}"#),
    );
    write(
        &dir.join("node_modules").join(name).join("index.js"),
        &format!("export const value = \"{marker}\";\n"),
    );
}

fn node_request(dir: &Path, entry: &str) -> BundleRequest {
    let mut request = BundleRequest::new(dir, entry, OutputLayout::file("out.mjs"), Platform::Node);
    request.resolve.packages = Packages::External;
    request
}

fn bundle_text(dir: &Path, request: &BundleRequest) -> String {
    let generated = crate::generate(request).unwrap();
    String::from_utf8(generated.file(&dir.join("out.mjs")).unwrap().to_vec()).unwrap()
}

#[test]
fn installed_package_stays_external_and_is_not_inlined() {
    let dir = temp_dir("installed");
    install(&dir, "pkg", "INSTALLED_PACKAGE_MARKER");
    write(
        &dir.join("entry.ts"),
        "import { value } from 'pkg';\nimport { platform } from 'node:os';\nexport default { value, platform };\n",
    );
    let request = node_request(&dir, "entry.ts");
    let generated = crate::generate(&request).unwrap();
    let bundle = String::from_utf8(generated.file(&dir.join("out.mjs")).unwrap().to_vec()).unwrap();
    assert!(!bundle.contains("INSTALLED_PACKAGE_MARKER"), "{bundle}");
    assert!(bundle.contains("from \"pkg\""), "{bundle}");
    assert!(bundle.contains("node:os"), "{bundle}");
    let imports = generated.graph["inputs"]["entry.ts"]["imports"]
        .as_array()
        .unwrap();
    assert!(
        imports
            .iter()
            .any(|edge| edge["path"] == "pkg" && edge["external"] == true),
        "{imports:?}"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn bundle_mode_still_inlines_the_installed_package() {
    let dir = temp_dir("bundled");
    install(&dir, "pkg", "INSTALLED_PACKAGE_MARKER");
    write(
        &dir.join("entry.ts"),
        "import { value } from 'pkg';\nexport default value;\n",
    );
    let mut request = node_request(&dir, "entry.ts");
    request.resolve.packages = Packages::Bundle;
    assert!(bundle_text(&dir, &request).contains("INSTALLED_PACKAGE_MARKER"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn uninstalled_package_stays_external() {
    let dir = temp_dir("uninstalled");
    write(
        &dir.join("entry.ts"),
        "import { value } from '@scope/absent/sub';\nexport default value;\n",
    );
    let bundle = bundle_text(&dir, &node_request(&dir, "entry.ts"));
    assert!(bundle.contains("@scope/absent/sub"), "{bundle}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn tsconfig_paths_resolve_before_package_externalization() {
    let dir = temp_dir("paths");
    install(&dir, "shadowed", "INSTALLED_SHADOWED_MARKER");
    install(&dir, "kept", "INSTALLED_KEPT_MARKER");
    write(
        &dir.join("node_modules/aliased-src/value.ts"),
        "export const value = \"NODE_MODULES_SOURCE_MARKER\";\n",
    );
    write(
        &dir.join("tsconfig.json"),
        r#"{"compilerOptions":{"baseUrl":".","paths":{
            "@lib/*":["./lib/*"],
            "shadowed":["./lib/shadow.ts"],
            "@nm/value":["./node_modules/aliased-src/value.ts"]
        }}}"#,
    );
    write(
        &dir.join("lib/helper.ts"),
        "export const helper = \"PATHS_ALIAS_MARKER\";\n",
    );
    write(
        &dir.join("lib/shadow.ts"),
        "export const value = \"LOCAL_SHADOW_MARKER\";\n",
    );
    write(
        &dir.join("lib/base.ts"),
        "export const base = \"BASE_URL_MARKER\";\n",
    );
    write(
        &dir.join("entry.ts"),
        "import { helper } from '@lib/helper';\n\
         import { value as shadow } from 'shadowed';\n\
         import { value as nm } from '@nm/value';\n\
         import { base } from 'lib/base';\n\
         import { value as kept } from 'kept';\n\
         export default { helper, shadow, nm, base, kept };\n",
    );
    let bundle = bundle_text(&dir, &node_request(&dir, "entry.ts"));
    for marker in [
        "PATHS_ALIAS_MARKER",
        "LOCAL_SHADOW_MARKER",
        "NODE_MODULES_SOURCE_MARKER",
        "BASE_URL_MARKER",
    ] {
        assert!(bundle.contains(marker), "{marker} missing: {bundle}");
    }
    assert!(!bundle.contains("INSTALLED_SHADOWED_MARKER"), "{bundle}");
    assert!(!bundle.contains("INSTALLED_KEPT_MARKER"), "{bundle}");
    assert!(bundle.contains("from \"kept\""), "{bundle}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn request_aliases_resolve_before_package_externalization() {
    let dir = temp_dir("aliases");
    install(&dir, "pkg", "INSTALLED_PACKAGE_MARKER");
    write(
        &dir.join("local.ts"),
        "export const value = \"REQUEST_ALIAS_MARKER\";\n",
    );
    write(
        &dir.join("entry.ts"),
        "import { value } from 'pkg';\nexport default value;\n",
    );
    let mut request = node_request(&dir, "entry.ts");
    request
        .resolve
        .aliases
        .push(("pkg".into(), dir.join("local.ts").to_string_lossy().into()));
    let bundle = bundle_text(&dir, &request);
    assert!(bundle.contains("REQUEST_ALIAS_MARKER"), "{bundle}");
    assert!(!bundle.contains("INSTALLED_PACKAGE_MARKER"), "{bundle}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn package_imports_resolve_and_a_missing_one_fails() {
    let dir = temp_dir("imports");
    write(
        &dir.join("package.json"),
        r##"{"name":"host","type":"module","imports":{"#local":"./local.ts"}}"##,
    );
    write(
        &dir.join("local.ts"),
        "export const value = \"PACKAGE_IMPORT_MARKER\";\n",
    );
    write(
        &dir.join("entry.ts"),
        "import { value } from '#local';\nexport default value;\n",
    );
    let bundle = bundle_text(&dir, &node_request(&dir, "entry.ts"));
    assert!(bundle.contains("PACKAGE_IMPORT_MARKER"), "{bundle}");

    write(
        &dir.join("entry.ts"),
        "import { value } from '#absent';\nexport default value;\n",
    );
    let error = format!(
        "{:#}",
        crate::generate(&node_request(&dir, "entry.ts")).unwrap_err()
    );
    assert!(error.contains("#absent"), "{error}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn malformed_tsconfig_is_an_error_not_an_external() {
    let dir = temp_dir("malformed");
    write(&dir.join("tsconfig.json"), "{ not json");
    write(
        &dir.join("entry.ts"),
        "import { value } from 'pkg';\nexport default value;\n",
    );
    let error = format!(
        "{:#}",
        crate::generate(&node_request(&dir, "entry.ts")).unwrap_err()
    );
    assert!(error.contains("tsconfig.json"), "{error}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn external_packages_reject_extra_module_dirs() {
    let mut request = node_request(Path::new("/srv"), "entry.ts");
    request.resolve.extra_module_dirs.push("/srv/hoist".into());
    let error = format!("{:#}", crate::generate(&request).unwrap_err());
    assert!(error.contains("extra_module_dirs"), "{error}");
}

#[test]
fn banner_is_prepended_verbatim() {
    let dir = temp_dir("banner");
    write(&dir.join("entry.ts"), "export default 1;\n");
    let mut request = node_request(&dir, "entry.ts");
    request.banner = Some("/* ZFB_BANNER_MARKER */ const require = 1;".into());
    let bundle = bundle_text(&dir, &request);
    assert!(
        bundle.starts_with("/* ZFB_BANNER_MARKER */ const require = 1;"),
        "{bundle}"
    );
    fs::remove_dir_all(&dir).unwrap();
}
