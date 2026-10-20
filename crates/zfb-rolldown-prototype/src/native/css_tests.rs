//! Real native negative contract: raw CSS never grants publication authority.
use super::run_prepared;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "zfb-native-css-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new("unused-prepared-esbuild");
        cmd.current_dir(&self.0).args([
            "entry.js",
            "--bundle",
            "--format=esm",
            "--outfile=out/bundle.mjs",
            "--metafile=out/meta.json",
        ]);
        cmd
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn raw_css_default_and_custom_loaders_reject_without_publication() {
    for (filename, loader) in [
        ("style.css", None),
        ("style.theme", Some("--loader:.theme=css")),
    ] {
        let fixture = Fixture::new();
        fs::write(
            fixture.0.join("entry.js"),
            format!("import './{filename}'; export const value = 1;"),
        )
        .unwrap();
        fs::write(fixture.0.join(filename), ".raw { color: red; }").unwrap();
        let mut cmd = fixture.command();
        if let Some(loader) = loader {
            cmd.arg(loader);
        }
        for existing in [false, true] {
            if existing {
                fs::create_dir(fixture.0.join("out")).unwrap();
                fs::write(fixture.0.join("out/bundle.mjs"), "other owner bundle").unwrap();
                fs::write(fixture.0.join("out/meta.json"), "other owner metadata").unwrap();
            }
            let error = run_prepared(&cmd).expect_err("raw CSS must fail closed");
            let message = format!("{error:#}");
            assert!(
                message.contains("ZFB_ROLLDOWN_RAW_CSS_UNSUPPORTED"),
                "{message}"
            );
            assert!(message.contains(filename), "{message}");
            if existing {
                assert_eq!(
                    fs::read_to_string(fixture.0.join("out/bundle.mjs")).unwrap(),
                    "other owner bundle"
                );
                assert_eq!(
                    fs::read_to_string(fixture.0.join("out/meta.json")).unwrap(),
                    "other owner metadata"
                );
                assert_eq!(fs::read_dir(fixture.0.join("out")).unwrap().count(), 2);
            } else {
                assert!(
                    !fixture.0.join("out").exists(),
                    "failed generation created output"
                );
            }
        }
    }
}

#[test]
fn css_explicitly_loaded_as_text_or_file_remains_a_resource() {
    for loader in ["--loader:.css=text", "--loader:.css=file"] {
        let fixture = Fixture::new();
        fs::write(
            fixture.0.join("entry.js"),
            "import value from './style.css'; export default value;",
        )
        .unwrap();
        fs::write(fixture.0.join("style.css"), ".resource { color: blue; }").unwrap();
        let mut cmd = fixture.command();
        cmd.arg(loader);
        run_prepared(&cmd).expect("explicit resource loader must remain supported");
        assert!(fixture.0.join("out/bundle.mjs").is_file());
        assert!(fixture.0.join("out/meta.json").is_file());
    }
}

/// The lifecycle observer's failed-replacement page: an unconditional
/// top-level throw in a module whose namespace the entry exports.
fn top_level_throw_fixture() -> Fixture {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("pages")).unwrap();
    fs::write(
        fixture.0.join("entry.js"),
        "import * as page from './pages/index.js';\n\
         export const routes = { '/': page };\n\
         export default { fetch() { return routes; } };\n",
    )
    .unwrap();
    fs::write(
        fixture.0.join("pages/index.js"),
        "throw new Error('CLI_REPLACEMENT_REJECTED');\n\
         export default function Page() { return 'CLI_BAD'; }\n",
    )
    .unwrap();
    fixture
}

#[test]
fn unminified_top_level_throw_keeps_exported_bindings() {
    let fixture = top_level_throw_fixture();
    run_prepared(&fixture.command()).unwrap();
    let bundle = fs::read_to_string(fixture.0.join("out/bundle.mjs")).unwrap();
    assert!(bundle.contains("CLI_REPLACEMENT_REJECTED"), "{bundle}");
    // The pinned DCE-only pass emitted `export { routes }` with no declaration.
    assert!(bundle.contains("const routes ="), "{bundle}");
}

#[test]
fn minified_top_level_throw_keeps_exported_bindings() {
    let fixture = top_level_throw_fixture();
    fs::write(
        fixture.0.join("entry.js"),
        "import * as page from './pages/index.js';\n\
         export const routes = { '/': page };\n\
         export let count = 1;\n\
         export class Kept {}\n\
         export default { fetch() { return routes; } };\n",
    )
    .unwrap();
    let mut cmd = fixture.command();
    cmd.arg("--minify");
    run_prepared(&cmd).unwrap();
    let bundle = fs::read_to_string(fixture.0.join("out/bundle.mjs")).unwrap();
    assert!(bundle.contains("CLI_REPLACEMENT_REJECTED"), "{bundle}");
    // The compressor still drops everything after the throw, but every
    // exported local must stay declared or the module fails to link.
    let (code, exports) = bundle
        .trim_end()
        .split_once("export{")
        .unwrap_or_else(|| panic!("no export clause: {bundle}"));
    let (exports, tail) = exports.split_once('}').unwrap();
    let declared: Vec<&str> = code
        .split("var ")
        .chain(tail.split("var "))
        .skip(1)
        .flat_map(|decl| decl.split(';').next().unwrap().split(','))
        .map(|binding| binding.split('=').next().unwrap())
        .collect();
    let mut exported = Vec::new();
    for spec in exports.split(',') {
        let (local, name) = spec.split_once(" as ").unwrap_or((spec, spec));
        assert!(declared.contains(&local), "{name} is undeclared: {bundle}");
        exported.push(name);
    }
    exported.sort_unstable();
    assert_eq!(exported, ["Kept", "count", "default", "routes"], "{bundle}");
}
