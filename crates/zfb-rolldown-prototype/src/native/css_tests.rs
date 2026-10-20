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
/// top-level throw in a module whose namespace `entry` exports.
fn top_level_throw_fixture(entry: &str) -> Fixture {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("pages")).unwrap();
    fs::write(fixture.0.join("entry.js"), entry).unwrap();
    fs::write(
        fixture.0.join("pages/index.js"),
        "throw new Error('CLI_REPLACEMENT_REJECTED');\n\
         export default function Page() { return 'CLI_BAD'; }\n",
    )
    .unwrap();
    fixture
}

const ROUTES_ENTRY: &str = "import * as page from './pages/index.js';\n\
     export const routes = { '/': page };\n\
     export default { fetch() { return routes; } };\n";

/// Runs the emitted bundle. An export without a declaration fails at link
/// time with a SyntaxError, before the page's throw could run.
fn assert_bundle_throws_the_page_error(fixture: &Fixture) {
    let output = Command::new("node")
        .current_dir(&fixture.0)
        .arg("out/bundle.mjs")
        .output()
        .expect("node runs the emitted bundle");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let bundle = fs::read_to_string(fixture.0.join("out/bundle.mjs")).unwrap();
    assert!(!output.status.success(), "{bundle}");
    assert!(
        stderr.contains("Error: CLI_REPLACEMENT_REJECTED") && !stderr.contains("SyntaxError"),
        "{stderr}\n{bundle}"
    );
}

#[test]
fn unminified_top_level_throw_keeps_exported_bindings() {
    let fixture = top_level_throw_fixture(ROUTES_ENTRY);
    run_prepared(&fixture.command()).unwrap();
    let bundle = fs::read_to_string(fixture.0.join("out/bundle.mjs")).unwrap();
    // The pinned DCE-only pass emitted `export { routes }` with no declaration.
    assert!(bundle.contains("const routes ="), "{bundle}");
    assert_bundle_throws_the_page_error(&fixture);
}

#[test]
fn minified_top_level_throw_keeps_exported_bindings() {
    let fixture = top_level_throw_fixture(&format!(
        "{ROUTES_ENTRY}\
         export let count = 1;\n\
         export class Kept {{}}\n\
         export function handler() {{ return count; }}\n\
         export {{ shared }} from './lib.js';\n"
    ));
    fs::write(fixture.0.join("lib.js"), "export const shared = 2;\n").unwrap();
    let mut cmd = fixture.command();
    cmd.arg("--minify");
    run_prepared(&cmd).unwrap();
    // The compressor still drops every statement after the throw.
    let bundle = fs::read_to_string(fixture.0.join("out/bundle.mjs")).unwrap();
    assert!(!bundle.contains("CLI_BAD"), "{bundle}");
    assert_bundle_throws_the_page_error(&fixture);
}
