//! Level-3 real-engine integration coverage for `zfb css` (#2600).
//!
//! The tests inspect the bytes and diagnostics from the built CLI. Fixtures
//! live below `tests/fixtures/css-*` and are copied to isolated projects before
//! each invocation. Only the build-parity case needs esbuild; CSS-only cases
//! run without external binaries or skip paths.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;
use zfb_test_utils::{locate_esbuild, zfb_binary};

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path)?;
        } else {
            panic!(
                "CSS command fixture contains unsupported non-file entry {}",
                source_path.display()
            );
        }
    }
    Ok(())
}

fn copied_fixture(name: &str) -> TempDir {
    let temp = tempfile::tempdir().expect("create CSS command fixture tempdir");
    copy_dir_recursive(&fixture_dir(name), temp.path()).expect("copy CSS command fixture");
    temp
}

fn run_css(
    project_root: &Path,
    input: &str,
    output: &str,
    explicit_project_root: Option<&str>,
    sources: &[&str],
    flags: &[&str],
) -> Output {
    let mut command = Command::new(zfb_binary!());
    command
        .arg("css")
        .args(["--input", input, "--output", output])
        .current_dir(project_root);
    if let Some(root) = explicit_project_root {
        command.args(["--project-root", root]);
    }
    for source in sources {
        command.args(["--source", source]);
    }
    command.args(flags).output().expect("spawn `zfb css`")
}

fn wind_audit_fixture(config: &str, source: &str) -> TempDir {
    let temp = tempfile::tempdir().expect("create wind audit fixture tempdir");
    fs::write(temp.path().join("package.json"), "{}\n").expect("write project package.json");
    fs::write(temp.path().join("zfb.config.json"), config).expect("write zfb.config.json");
    let source_path = temp.path().join("src/a.tsx");
    fs::create_dir_all(source_path.parent().expect("source file parent"))
        .expect("create source directory");
    fs::write(source_path, source).expect("write wind audit source");
    temp
}

fn run_wind_audit(project_root: &Path, flags: &[&str]) -> Output {
    Command::new(zfb_binary!())
        .args(["wind", "audit", "--project-root"])
        .arg(project_root)
        .args(flags)
        .current_dir(project_root)
        .output()
        .expect("spawn `zfb wind audit`")
}

fn process_stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn process_stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn package_export_fixture(package_name: &str, package_json: &str, specifier: &str) -> TempDir {
    let temp = tempfile::tempdir().expect("create package-export CSS fixture tempdir");
    fs::write(temp.path().join("package.json"), "{}\n").expect("write project package.json");
    let package_dir = temp.path().join("node_modules").join(package_name);
    fs::create_dir_all(package_dir.join("dist")).expect("create package CSS directory");
    fs::write(package_dir.join("package.json"), package_json)
        .expect("write package-export fixture package.json");
    fs::write(package_dir.join("dist/widget.css"), ".widget{color:red}")
        .expect("write package-export fixture CSS");
    fs::write(
        temp.path().join("entry.css"),
        format!("@import \"{specifier}\";\n"),
    )
    .expect("write package-export CSS entry");
    temp
}

fn explicit_source_exclusion_fixture() -> TempDir {
    let temp = tempfile::tempdir().expect("create explicit-source exclusion fixture tempdir");
    fs::write(temp.path().join("package.json"), "{}\n").expect("write project package.json");
    fs::write(temp.path().join("entry.css"), "").expect("write CSS entrypoint");
    for (relative, class_name) in [
        ("src/a.tsx", "grid"),
        ("dist/b.tsx", "flex"),
        ("lib/c.tsx", "block"),
        ("build/d.tsx", "hidden"),
    ] {
        let path = temp.path().join(relative);
        fs::create_dir_all(path.parent().expect("source file parent"))
            .expect("create source directory");
        fs::write(
            path,
            format!("export default () => <div className=\"{class_name}\" />;\n"),
        )
        .expect("write source file");
    }
    temp
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} should succeed; status={:?}\n{}",
        output.status,
        combined_output(output)
    );
}

fn assert_failure(output: &Output, context: &str) {
    assert!(
        !output.status.success(),
        "{context} should fail; status={:?}\n{}",
        output.status,
        combined_output(output)
    );
}

fn assert_deterministic_css(bytes: &[u8], project_root: &Path) {
    let text = String::from_utf8(bytes.to_vec()).expect("wind output must be UTF-8 CSS");
    assert!(
        !text.contains("sourceMappingURL"),
        "CSS output must not carry a source map comment"
    );
    assert!(
        !bytes.contains(&b'\r'),
        "CSS output must use LF line endings only"
    );
    let project = project_root.to_string_lossy();
    assert!(
        !text.contains(project.as_ref()),
        "CSS output leaked its absolute project path: {project:?}"
    );
    assert!(
        !text.contains("/private/tmp/") && !text.contains("/var/folders/"),
        "CSS output leaked an absolute temporary path"
    );
}

fn find_build_css(root: &Path) -> PathBuf {
    let assets = root.join("dist/assets");
    let mut files = fs::read_dir(&assets)
        .unwrap_or_else(|error| panic!("read {}: {error}", assets.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("css")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("styles-") && name.ends_with(".css"))
        })
        .collect::<Vec<_>>();
    files.sort();
    assert_eq!(
        files.len(),
        1,
        "expected exactly one hashed build stylesheet under {}; got {files:#?}",
        assets.display()
    );
    files.pop().expect("one build stylesheet")
}

#[test]
fn css_command_output_is_deterministic_and_matches_committed_golden() {
    let temp = copied_fixture("css-determinism");
    let first = run_css(
        temp.path(),
        "entry.css",
        "first.css",
        Some("."),
        &["src/index.html"],
        &["--no-auto-source"],
    );
    assert_success(&first, "first deterministic CSS run");
    let first_bytes = fs::read(temp.path().join("first.css")).expect("read first CSS output");

    let second = run_css(
        temp.path(),
        "entry.css",
        "second.css",
        Some("."),
        &["src/index.html"],
        &["--no-auto-source"],
    );
    assert_success(&second, "second deterministic CSS run");
    let second_bytes = fs::read(temp.path().join("second.css")).expect("read second CSS output");
    let golden = fs::read(temp.path().join("golden.css")).expect("read committed CSS golden");

    assert_eq!(
        first_bytes, second_bytes,
        "wind runs must be byte-identical"
    );
    assert_eq!(
        first_bytes, golden,
        "real-engine output must match the reviewed golden"
    );
    assert_deterministic_css(&first_bytes, temp.path());
}

#[test]
fn css_command_highlight_class_default_and_inline_modes() {
    let temp = copied_fixture("css-highlight");
    let default_mode = run_css(
        temp.path(),
        "entry.css",
        "default.css",
        Some("."),
        &["src/index.html"],
        &["--no-auto-source"],
    );
    assert_success(&default_mode, "config-provided class highlight mode");
    let default_css = fs::read_to_string(temp.path().join("default.css")).unwrap();
    assert!(default_css.contains("--zfb-hi-"));
    assert!(default_css.contains(".hi-kw"));

    let class_mode = run_css(
        temp.path(),
        "entry.css",
        "class.css",
        Some("."),
        &["src/index.html"],
        &["--no-auto-source", "--code-highlight-mode", "class"],
    );
    assert_success(&class_mode, "explicit class highlight mode");
    let class_css = fs::read_to_string(temp.path().join("class.css")).unwrap();
    assert!(class_css.contains("--zfb-hi-"));
    assert!(class_css.contains(".hi-kw"));

    let no_default = run_css(
        temp.path(),
        "entry.css",
        "no-default.css",
        Some("."),
        &["src/index.html"],
        &[
            "--no-auto-source",
            "--code-highlight-mode",
            "class",
            "--no-default-highlight-styles",
        ],
    );
    assert_success(&no_default, "class mode with default stylesheet disabled");
    let no_default_css = fs::read_to_string(temp.path().join("no-default.css")).unwrap();
    assert!(!no_default_css.contains("--zfb-hi-"));
    assert!(!no_default_css.contains(".hi-kw"));

    let inline_mode = run_css(
        temp.path(),
        "entry.css",
        "inline.css",
        Some("."),
        &["src/index.html"],
        &["--no-auto-source", "--code-highlight-mode", "inline"],
    );
    assert_success(&inline_mode, "explicit inline highlight mode");
    let inline_css = fs::read_to_string(temp.path().join("inline.css")).unwrap();
    assert!(!inline_css.contains("--zfb-hi-"));
    assert!(!inline_css.contains(".hi-kw"));
}

#[test]
fn css_command_explicit_sources_isolate_ambient_decoy() {
    let outer = tempfile::tempdir().expect("create explicit-source parent tempdir");
    let project = outer.path().join("project");
    copy_dir_recursive(&fixture_dir("css-explicit-source"), &project)
        .expect("copy explicit-source fixture");
    fs::create_dir_all(project.join("components")).unwrap();
    fs::write(
        project.join("components/ambient-decoy.html"),
        "<div class=\"hidden\"></div>\n",
    )
    .unwrap();

    let output = run_css(
        &project,
        "entry.css",
        "compiled.css",
        Some("."),
        &["src/allowed.html"],
        &["--no-auto-source"],
    );
    assert_success(&output, "explicit-source CSS compilation");
    let css = fs::read_to_string(project.join("compiled.css")).unwrap();
    assert!(
        css.contains(".bg-\\[\\#11aa22\\]"),
        "allowed utility missing:\n{css}"
    );
    assert!(
        !css.contains(".hidden") && !css.contains("#cc44dd"),
        "ambient utility outside the source plan leaked into CSS:\n{css}"
    );
}

#[test]
fn css_command_explicit_source_under_default_out_dir_fails() {
    let temp = explicit_source_exclusion_fixture();
    let output = run_css(
        temp.path(),
        "entry.css",
        "out.css",
        Some("."),
        &["dist/**/*.tsx"],
        &["--no-auto-source"],
    );

    assert_failure(&output, "explicit source under the default outDir");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ZW010"), "{stderr}");
    assert!(stderr.contains("configured outDir `dist`"), "{stderr}");
    assert!(stderr.contains("dist/**/*.tsx"), "{stderr}");
    assert!(!temp.path().join("out.css").exists());
}

#[test]
fn css_command_each_explicit_source_declaration_must_keep_a_match() {
    let temp = explicit_source_exclusion_fixture();
    let output = run_css(
        temp.path(),
        "entry.css",
        "out.css",
        Some("."),
        &[
            "src/**/*.tsx",
            "dist/**/*.tsx",
            "lib/**/*.tsx",
            "build/**/*.tsx",
        ],
        &["--no-auto-source"],
    );

    assert_failure(&output, "four explicit source declarations including dist");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ZW010"), "{stderr}");
    assert!(stderr.contains("configured outDir `dist`"), "{stderr}");
    assert!(stderr.contains("dist/**/*.tsx"), "{stderr}");
    assert!(!temp.path().join("out.css").exists());
}

#[test]
fn css_command_partial_explicit_source_exclusion_warns_and_keeps_accepted_classes() {
    let temp = explicit_source_exclusion_fixture();
    let output = run_css(
        temp.path(),
        "entry.css",
        "out.css",
        Some("."),
        &["**/*.tsx"],
        &["--no-auto-source"],
    );

    assert_success(&output, "partially excluded explicit source glob");
    let css = fs::read_to_string(temp.path().join("out.css")).expect("read CSS output");
    for selector in [".grid", ".block", ".hidden"] {
        assert!(
            css.contains(selector),
            "accepted {selector} is missing:\n{css}"
        );
    }
    assert!(!css.contains(".flex"), "excluded .flex was emitted:\n{css}");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let warnings = stderr
        .lines()
        .filter(|line| line.contains("ZW010"))
        .collect::<Vec<_>>();
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one ZW010 warning:\n{stderr}"
    );
    assert!(warnings[0].contains("dist"), "{stderr}");
}

#[test]
fn css_command_wind_sources_exclude_tests_and_scan_declared_package_root() {
    let temp = tempfile::tempdir().expect("create wind sources fixture tempdir");
    fs::write(temp.path().join("package.json"), "{}\n").expect("write project package.json");
    fs::write(temp.path().join("entry.css"), "").expect("write CSS entrypoint");
    fs::write(temp.path().join(".gitignore"), "node_modules\ndist\n").expect("write .gitignore");
    fs::write(
        temp.path().join("zfb.config.json"),
        r#"{"wind":{"sources":{"exclude":["src/**/__tests__/**"],"packageRoots":["@fixture/ui"]}}}"#,
    )
    .expect("write wind sources config");
    for (relative, class_name) in [
        ("src/a.tsx", "flex"),
        ("src/__tests__/a.test.tsx", "grid"),
        ("node_modules/@fixture/ui/dist/route.js", "block"),
    ] {
        let path = temp.path().join(relative);
        fs::create_dir_all(path.parent().expect("source file parent"))
            .expect("create source directory");
        fs::write(path, format!("export const c = \"{class_name}\";\n"))
            .expect("write source file");
    }

    let output = run_css(temp.path(), "entry.css", "out.css", Some("."), &[], &[]);
    assert_success(&output, "wind.sources exclusion and package root");
    let css = fs::read_to_string(temp.path().join("out.css")).expect("read CSS output");
    assert!(css.contains(".flex"), "project utility is missing:\n{css}");
    assert!(
        css.contains(".block"),
        "package-root utility is missing:\n{css}"
    );
    assert!(
        !css.contains(".grid"),
        "excluded test utility was emitted:\n{css}"
    );

    let output = run_css(
        temp.path(),
        "entry.css",
        "out.css",
        Some("."),
        &["src/__tests__/a.test.tsx"],
        &["--no-auto-source"],
    );
    assert_failure(&output, "explicit source excluded by wind.sources");
    let stderr = process_stderr(&output);
    assert!(stderr.contains("ZW010"), "{stderr}");
    assert!(stderr.contains("src/**/__tests__/**"), "{stderr}");
}

#[test]
fn css_command_rejects_malformed_wind_source_exclusion() {
    let temp = explicit_source_exclusion_fixture();
    fs::write(
        temp.path().join("zfb.config.json"),
        r#"{"wind":{"sources":{"exclude":["src/["]}}}"#,
    )
    .expect("write malformed wind sources config");
    let output = run_css(temp.path(), "entry.css", "out.css", Some("."), &[], &[]);
    assert_failure(&output, "malformed wind.sources.exclude");
    let stderr = process_stderr(&output);
    assert!(
        stderr.contains("wind.sources.exclude[0] \"src/[\" declared by project"),
        "{stderr}"
    );
}

#[test]
fn css_command_custom_out_dir_allows_dist_as_an_explicit_source() {
    let temp = explicit_source_exclusion_fixture();
    fs::write(
        temp.path().join("zfb.config.json"),
        "{\"outDir\":\"out\"}\n",
    )
    .expect("write custom outDir config");
    let output = run_css(
        temp.path(),
        "entry.css",
        "site.css",
        Some("."),
        &["dist/**/*.tsx"],
        &["--no-auto-source"],
    );

    assert_success(&output, "dist source with custom outDir");
    let css = fs::read_to_string(temp.path().join("site.css")).expect("read CSS output");
    assert!(css.contains(".flex"), "dist utility is missing:\n{css}");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("ZW010"));
}

#[test]
fn css_command_explicit_source_exclusion_runs_are_deterministic() {
    let temp = explicit_source_exclusion_fixture();
    let sources = ["src/**/*.tsx"];
    let first = run_css(
        temp.path(),
        "entry.css",
        "out.css",
        Some("."),
        &sources,
        &["--no-auto-source"],
    );
    assert_success(&first, "first explicit-source exclusion run");
    let first_bytes = fs::read(temp.path().join("out.css")).expect("read first CSS output");

    let second = run_css(
        temp.path(),
        "entry.css",
        "out.css",
        Some("."),
        &sources,
        &["--no-auto-source"],
    );
    assert_success(&second, "second explicit-source exclusion run");
    let second_bytes = fs::read(temp.path().join("out.css")).expect("read second CSS output");

    assert_eq!(
        first_bytes, second_bytes,
        "identical runs must emit identical CSS bytes"
    );
}

#[test]
fn css_command_replays_consumer_entrypoint_with_explicit_sources() {
    let temp = copied_fixture("css-consumer-replay");
    let sources = ["src/**/*.{tsx,ts,jsx,js}"];
    let first = run_css(
        temp.path(),
        "entry.css",
        "first.css",
        Some("."),
        &sources,
        &["--no-auto-source"],
    );
    assert_success(&first, "first consumer CSS replay");
    let first_bytes = fs::read(temp.path().join("first.css")).expect("read consumer CSS");
    let second = run_css(
        temp.path(),
        "entry.css",
        "second.css",
        Some("."),
        &sources,
        &["--no-auto-source"],
    );
    assert_success(&second, "second consumer CSS replay");
    let second_bytes = fs::read(temp.path().join("second.css")).expect("read consumer CSS");
    assert_eq!(
        first_bytes, second_bytes,
        "consumer wind runs must be identical"
    );
    assert_deterministic_css(&first_bytes, temp.path());

    let css = String::from_utf8(first_bytes).expect("consumer CSS output must be UTF-8");
    for (selector, declaration) in [
        (".flex", "display: flex"),
        (".grid", "display: grid"),
        (".bg-surface", "background-color: var(--zw-color-surface)"),
        (".text-fg", "color: var(--zw-color-fg)"),
        (".rounded-lg", "border-top-left-radius: var(--zw-radius-lg)"),
    ] {
        assert!(
            css.contains(selector) && css.contains(declaration),
            "consumer candidate {selector:?} with {declaration:?} missing:\n{css}"
        );
    }
    assert!(css.contains("--zfb-hi-"));
    assert!(css.contains(".hi-root") && css.contains("var(--zfb-hi-fg)"));
    for (marker, next_marker) in [
        (
            "--consumer-import-order-theme",
            "--consumer-import-order-safelist",
        ),
        (
            "--consumer-import-order-safelist",
            "--consumer-import-order-content",
        ),
        (
            "--consumer-import-order-content",
            "--consumer-import-order-page-loading",
        ),
        (
            "--consumer-import-order-page-loading",
            "--consumer-import-order-features",
        ),
    ] {
        assert!(
            css.find(marker).expect("ordered CSS marker present")
                < css.find(next_marker).expect("next marker present"),
            "package imports must preserve {marker:?} before {next_marker:?}"
        );
    }
    assert!(
        !css.contains("#d34db7"),
        "manifest decoy leaked into CSS:\n{css}"
    );
    for unresolved in ["@tailwind", "@apply", "@source", "@import"] {
        assert!(
            !css.contains(unresolved),
            "unresolved {unresolved} remained:\n{css}"
        );
    }
}

#[test]
fn css_command_missing_input_exits_nonzero() {
    let temp = copied_fixture("css-failures");
    let output = run_css(
        temp.path(),
        "missing.css",
        "compiled.css",
        Some("."),
        &[],
        &["--no-auto-source"],
    );
    assert_failure(&output, "missing CSS input");
    let diagnostics = combined_output(&output);
    assert!(
        diagnostics.contains("cannot read CSS input"),
        "{diagnostics}"
    );
    assert!(diagnostics.contains("missing.css"), "{diagnostics}");
    assert!(!temp.path().join("compiled.css").exists());
}

#[test]
fn css_command_zero_match_source_glob_exits_nonzero() {
    let temp = copied_fixture("css-failures");
    let output = run_css(
        temp.path(),
        "entry.css",
        "compiled.css",
        Some("."),
        &["src/no-match/**/*.html"],
        &["--no-auto-source"],
    );
    assert_failure(&output, "zero-match --source glob");
    let diagnostics = combined_output(&output);
    assert!(diagnostics.contains("matched zero files"), "{diagnostics}");
    assert!(diagnostics.contains("src/no-match"), "{diagnostics}");
    assert!(!temp.path().join("compiled.css").exists());
}

#[cfg(unix)]
#[test]
fn css_command_same_canonical_input_output_exits_nonzero() {
    let temp = copied_fixture("css-failures");
    std::os::unix::fs::symlink("entry.css", temp.path().join("alias.css"))
        .expect("create input/output alias");
    let output = run_css(
        temp.path(),
        "entry.css",
        "alias.css",
        Some("."),
        &["source.html"],
        &["--no-auto-source"],
    );
    assert_failure(&output, "same canonical CSS input/output");
    assert!(combined_output(&output).contains("same path"));
}

#[test]
fn css_command_missing_relative_import_exits_nonzero() {
    let temp = copied_fixture("css-failures");
    let output = run_css(
        temp.path(),
        "missing-relative-import.css",
        "compiled.css",
        Some("."),
        &["source.html"],
        &["--no-auto-source"],
    );
    assert_failure(&output, "missing relative CSS import");
    let diagnostics = combined_output(&output);
    assert!(
        diagnostics.contains("missing-relative.css"),
        "{diagnostics}"
    );
    assert!(
        diagnostics.contains("bundle") || diagnostics.contains("resolve"),
        "{diagnostics}"
    );
    assert!(!temp.path().join("compiled.css").exists());
}

#[test]
fn css_command_resolves_unscoped_package_css_exports_subpath() {
    let temp = package_export_fixture(
        "widget-css",
        r#"{"name":"widget-css","exports":{"./styles.css":"./dist/widget.css"}}"#,
        "widget-css/styles.css",
    );
    let output = run_css(
        temp.path(),
        "entry.css",
        "out.css",
        Some("."),
        &[],
        &["--no-auto-source"],
    );
    assert_success(&output, "unscoped package CSS exports subpath");
    let css = fs::read_to_string(temp.path().join("out.css")).expect("read output CSS");
    assert!(
        css.contains(".widget"),
        "package CSS was not bundled:\n{css}"
    );
}

#[test]
fn css_command_resolves_scoped_package_css_exports_subpath() {
    let temp = package_export_fixture(
        "@scope/widget",
        r#"{"name":"@scope/widget","exports":{"./styles.css":"./dist/widget.css"}}"#,
        "@scope/widget/styles.css",
    );
    let output = run_css(
        temp.path(),
        "entry.css",
        "out.css",
        Some("."),
        &[],
        &["--no-auto-source"],
    );
    assert_success(&output, "scoped package CSS exports subpath");
    let css = fs::read_to_string(temp.path().join("out.css")).expect("read output CSS");
    assert!(
        css.contains(".widget"),
        "package CSS was not bundled:\n{css}"
    );
}

#[test]
fn css_command_rejects_null_package_css_exports_subpath() {
    let temp = package_export_fixture(
        "widget-css",
        r#"{"name":"widget-css","exports":{"./styles.css":null}}"#,
        "widget-css/styles.css",
    );
    let output = run_css(
        temp.path(),
        "entry.css",
        "out.css",
        Some("."),
        &[],
        &["--no-auto-source"],
    );
    assert_failure(&output, "null package CSS exports subpath");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("null (blocked by the package)"),
        "expected null-export reason in stderr:\n{stderr}"
    );
    assert!(!temp.path().join("out.css").exists());
}

#[test]
fn css_command_real_unresolved_apply_exits_nonzero() {
    let temp = copied_fixture("css-failures");
    let output = run_css(
        temp.path(),
        "unresolved-apply.css",
        "compiled.css",
        Some("."),
        &["source.html"],
        &["--no-auto-source"],
    );
    assert_failure(&output, "leftover directive");
    let diagnostics = combined_output(&output);
    assert!(diagnostics.contains("ZW009"), "{diagnostics}");
    assert!(diagnostics.contains("@apply"), "{diagnostics}");
    assert!(!temp.path().join("compiled.css").exists());
}

#[test]
fn css_command_atomic_success_replaces_and_failure_preserves_output() {
    let success = copied_fixture("css-atomic");
    fs::write(success.path().join("compiled.css"), b"SUCCESS_SENTINEL").unwrap();
    let success_output = run_css(
        success.path(),
        "entry.css",
        "compiled.css",
        Some("."),
        &["source.html"],
        &["--no-auto-source"],
    );
    assert_success(&success_output, "atomic CSS success");
    let success_bytes = fs::read(success.path().join("compiled.css")).unwrap();
    assert_ne!(success_bytes, b"SUCCESS_SENTINEL");
    let success_css = String::from_utf8_lossy(&success_bytes);
    assert!(success_css.contains(".bg-\\[\\#334455\\]"), "{success_css}");
    assert!(!success_css.contains("SUCCESS_SENTINEL"));

    let failure = copied_fixture("css-atomic");
    fs::write(
        failure.path().join("entry.css"),
        ".broken { @apply unknown; }\n",
    )
    .unwrap();
    fs::write(failure.path().join("compiled.css"), b"FAILURE_SENTINEL").unwrap();
    let failure_output = run_css(
        failure.path(),
        "entry.css",
        "compiled.css",
        Some("."),
        &["source.html"],
        &["--no-auto-source"],
    );
    assert_failure(&failure_output, "atomic CSS failure");
    assert_eq!(
        fs::read(failure.path().join("compiled.css")).unwrap(),
        b"FAILURE_SENTINEL",
        "failed compilation must preserve the previous output bytes"
    );
}

#[test]
fn css_command_matches_build_stylesheet_for_equivalent_explicit_source_plan() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[css_command_build_parity] no esbuild binary available; skipping. Set ZFB_ESBUILD_BIN or stage crates/zfb/binaries/esbuild/esbuild."
        );
        return;
    };

    let outer = tempfile::tempdir().expect("create parity parent tempdir");
    let build_project = outer.path().join("build-project");
    let css_project = outer.path().join("css-project");
    copy_dir_recursive(&fixture_dir("css-build-parity"), &build_project)
        .expect("copy build-parity fixture for zfb build");
    copy_dir_recursive(&fixture_dir("css-build-parity"), &css_project)
        .expect("copy build-parity fixture for zfb css");

    let build = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&build_project)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build` for CSS parity");
    assert_success(&build, "build-parity zfb build");
    let build_css_path = find_build_css(&build_project);
    let build_css = fs::read(&build_css_path).expect("read hashed build stylesheet");

    // `zfb build` scans its conventional roots; the explicit standalone root
    // scans the same source tree. Both plans exclude build output, and the
    // fixture contains no CSS Modules, package roots or generated sources.
    let standalone = run_css(
        &css_project,
        "styles/global.css",
        "../standalone.css",
        Some("."),
        &["."],
        &["--no-auto-source"],
    );
    assert_success(&standalone, "build-parity standalone zfb css");
    let standalone_css =
        fs::read(outer.path().join("standalone.css")).expect("read standalone CSS parity output");
    assert_eq!(
        build_css,
        standalone_css,
        "build stylesheet {} and equivalent standalone wind output must be byte-identical",
        build_css_path.display()
    );
}

#[test]
fn wind_audit_prints_its_plan_and_build_plan_honors_source_exclusions() {
    let temp = wind_audit_fixture(
        r#"{"wind":{"sources":{"exclude":["src/**/__tests__/**"]}}}"#,
        "export default () => <div class=\"block\" />;\n",
    );
    let fixture = temp.path().join("src/__tests__/a.test.tsx");
    fs::create_dir_all(fixture.parent().expect("fixture parent")).expect("create fixture dir");
    fs::write(
        &fixture,
        "export default () => <div class=\"bg-missing\" />;\n",
    )
    .expect("write excluded fixture");

    for (flags, mode) in [
        (&[][..], "standalone"),
        (&["--plan", "standalone"][..], "standalone"),
        (&["--plan", "build"][..], "build"),
    ] {
        let output = run_wind_audit(temp.path(), flags);
        let stdout = process_stdout(&output);
        let stderr = process_stderr(&output);
        assert!(
            output.status.success(),
            "{mode} audit should succeed\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains(&format!("wind audit plan: {mode}\n")),
            "{stdout}"
        );
        assert!(stdout.contains("  root default/src src "), "{stdout}");
        assert!(
            stdout.contains("  excluded src/**/__tests__/** (wind.sources.exclude from project)"),
            "{stdout}"
        );
        assert!(
            !stdout.contains("bg-missing"),
            "an excluded source reached the {mode} audit:\n{stdout}"
        );
    }
}

#[test]
fn wind_audit_clean_project_succeeds_with_complete_report() {
    let temp = wind_audit_fixture(
        r#"{"wind":{"spec":1}}"#,
        "export default () => <div class=\"block\" />;\n",
    );
    let output = run_wind_audit(temp.path(), &[]);
    let stdout = process_stdout(&output);
    let stderr = process_stderr(&output);

    assert!(
        output.status.success(),
        "clean audit should succeed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("outcome: complete"),
        "unexpected stdout:\n{stdout}"
    );
    assert!(
        stderr.is_empty(),
        "clean audit should not write to stderr:\n{stderr}"
    );
}

#[test]
fn wind_cli_reports_authored_class_hint_and_reservation_controls_css() {
    let temp = wind_audit_fixture(
        r#"{"wind":{"spec":1}}"#,
        "const unrelated = 'text-link';\nexport default ({ active }) => <div class=\"text-link border-t-soft flex-row-custom ordinary-class\" className={`hover:text-link ${active ? 'block' : ''}`} />;\n",
    );
    let default = run_wind_audit(temp.path(), &[]);
    let stdout = process_stdout(&default);
    assert!(
        default.status.success(),
        "{stdout}\n{}",
        process_stderr(&default)
    );
    for candidate in ["text-link", "border-t-soft", "flex-row-custom"] {
        assert!(
            stdout.contains(&format!(
                "wind.authoredClasses: {{ \"{candidate}\": true }}"
            )),
            "{stdout}"
        );
    }
    assert!(
        stdout.contains("wind.authoredClasses: { \"hover:text-link\": true }"),
        "{stdout}"
    );
    assert!(stdout.contains("ZW006 auditInfo"), "{stdout}");
    assert!(
        stdout.contains("No generated utility CSS is emitted"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("wind.authoredClasses: { \"ordinary-class\""),
        "{stdout}"
    );

    let strict = run_wind_audit(temp.path(), &["--fail-on", "error"]);
    assert!(!strict.status.success());
    assert!(process_stderr(&strict).contains("--fail-on error"));

    let explain = Command::new(zfb_binary!())
        .args(["wind", "explain", "text-link", "--project-root"])
        .arg(temp.path())
        .current_dir(temp.path())
        .output()
        .expect("spawn `zfb wind explain`");
    assert!(explain.status.success(), "{}", process_stderr(&explain));
    assert!(process_stdout(&explain).contains("wind.authoredClasses: { \"text-link\": true }"));

    fs::write(
        temp.path().join("entry.css"),
        ".text-link { color: red; }\n",
    )
    .unwrap();
    let unreserved = run_css(
        temp.path(),
        "entry.css",
        "out.css",
        Some("."),
        &["src/a.tsx"],
        &["--no-auto-source"],
    );
    assert!(
        !unreserved.status.success(),
        "{}",
        process_stdout(&unreserved)
    );
    assert!(process_stderr(&unreserved).contains("wind.authoredClasses: { \"text-link\": true }"));
    assert!(!temp.path().join("out.css").exists());

    fs::write(
        temp.path().join("zfb.config.json"),
        r#"{"wind":{"spec":1,"authoredClasses":{"text-link":true,"border-t-soft":true,"flex-row-custom":true,"hover:text-link":true}}}"#,
    ).unwrap();
    let reserved = run_css(
        temp.path(),
        "entry.css",
        "out.css",
        Some("."),
        &["src/a.tsx"],
        &["--no-auto-source"],
    );
    assert!(reserved.status.success(), "{}", process_stderr(&reserved));
    let css = fs::read_to_string(temp.path().join("out.css")).unwrap();
    assert!(
        css.contains(".text-link"),
        "authored CSS should remain: {css}"
    );
    assert!(
        !css.contains(".border-t-soft") && !css.contains(".flex-row-custom"),
        "reserved utilities should not emit: {css}"
    );
}

#[test]
fn wind_audit_invalid_configuration_prints_report_and_fails() {
    let temp = wind_audit_fixture(
        r#"{"wind":{"spec":1,"tokens":{"radii":{"full":"9999px"}}}}"#,
        "export default () => <div class=\"block\" />;\n",
    );
    let output = run_wind_audit(temp.path(), &[]);
    let stdout = process_stdout(&output);
    let stderr = process_stderr(&output);

    assert!(
        !output.status.success(),
        "invalid configuration should fail\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("outcome: invalid configuration"),
        "unexpected stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("ZW007"),
        "invalid configuration report must contain ZW007:\n{stdout}"
    );
    assert!(
        stderr.contains("wind audit: invalid configuration"),
        "stderr should summarize the failure:\n{stderr}"
    );
}

#[test]
fn wind_audit_error_threshold_is_opt_in_and_includes_complete_report() {
    let temp = wind_audit_fixture(
        r#"{"wind":{"spec":1}}"#,
        "export default () => <div class=\"rounded-lg md:block\" />;\n",
    );

    let default_output = run_wind_audit(temp.path(), &[]);
    let default_stdout = process_stdout(&default_output);
    let default_stderr = process_stderr(&default_output);
    assert!(
        default_output.status.success(),
        "default audit should succeed\nstdout:\n{default_stdout}\nstderr:\n{default_stderr}"
    );
    assert!(
        default_stdout.contains("outcome: complete"),
        "unexpected stdout:\n{default_stdout}"
    );
    assert!(
        default_stdout.contains("ZW006"),
        "missing-token finding must be reported:\n{default_stdout}"
    );
    assert!(
        default_stdout.contains("ZW002"),
        "unknown-variant finding must be reported:\n{default_stdout}"
    );
    assert!(
        default_stderr.contains("--fail-on error"),
        "stderr should explain the strict threshold:\n{default_stderr}"
    );

    let strict_output = run_wind_audit(temp.path(), &["--fail-on", "error"]);
    let strict_stdout = process_stdout(&strict_output);
    let strict_stderr = process_stderr(&strict_output);
    assert!(
        !strict_output.status.success(),
        "strict audit should fail\nstdout:\n{strict_stdout}\nstderr:\n{strict_stderr}"
    );
    assert!(
        strict_stdout.contains("outcome: complete"),
        "strict failure should preserve the report:\n{strict_stdout}"
    );
    assert!(
        strict_stdout.contains("ZW006"),
        "strict report is missing ZW006:\n{strict_stdout}"
    );
    assert!(
        strict_stdout.contains("ZW002"),
        "strict report is missing ZW002:\n{strict_stdout}"
    );
    assert!(
        strict_stderr.contains("wind audit:"),
        "stderr should summarize the strict failure:\n{strict_stderr}"
    );
    assert!(
        strict_stderr.contains("--fail-on error"),
        "stderr should name the threshold:\n{strict_stderr}"
    );
}

#[test]
fn wind_audit_info_does_not_fail_warning_threshold() {
    let temp = wind_audit_fixture(
        r#"{"wind":{"spec":1}}"#,
        "export default () => <div class=\"block flex\" />;\n",
    );
    let output = run_wind_audit(temp.path(), &["--fail-on", "warning"]);
    let stdout = process_stdout(&output);
    let stderr = process_stderr(&output);

    assert!(
        output.status.success(),
        "auditInfo should not fail a warning threshold\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("outcome: complete"),
        "unexpected stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("ZW013 auditInfo"),
        "property conflict should be auditInfo:\n{stdout}"
    );
    assert!(
        stderr.is_empty(),
        "auditInfo should not write a threshold summary:\n{stderr}"
    );
}

#[test]
fn wind_audit_disabled_generation_succeeds_even_with_error_threshold() {
    let temp = wind_audit_fixture(
        r#"{"wind":false}"#,
        "export default () => <div class=\"rounded-lg md:block\" />;\n",
    );
    let output = run_wind_audit(temp.path(), &["--fail-on", "error"]);
    let stdout = process_stdout(&output);
    let stderr = process_stderr(&output);

    assert!(
        output.status.success(),
        "disabled audit should succeed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("outcome: generation disabled"),
        "unexpected stdout:\n{stdout}"
    );
    assert!(
        stderr.is_empty(),
        "disabled audit should not write to stderr:\n{stderr}"
    );
}

#[test]
fn wind_audit_malformed_manifest_names_its_path_on_stderr() {
    let temp = wind_audit_fixture(
        r#"{"wind":{"spec":1,"manifests":{"widgets":{"path":"./wind-manifest.json"}}}}"#,
        "export default () => <div class=\"block\" />;\n",
    );
    let manifest_path = temp.path().join("wind-manifest.json");
    fs::write(&manifest_path, "{ malformed json\n").expect("write malformed wind manifest");

    let output = run_wind_audit(temp.path(), &[]);
    let stdout = process_stdout(&output);
    let stderr = process_stderr(&output);

    assert!(
        !output.status.success(),
        "malformed manifest should fail\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.is_empty(),
        "manifest loading fails before a report is rendered:\n{stdout}"
    );
    assert!(
        stderr.contains("manifest widgets"),
        "stderr should identify the producer:\n{stderr}"
    );
    assert!(
        stderr.contains(&manifest_path.display().to_string()),
        "stderr should name the malformed manifest path:\n{stderr}"
    );
}

/// One-file wind project for the #3523–#3525 classification contracts.
fn wind_classification_fixture(wind: &str, source: &str) -> TempDir {
    let temp = tempfile::tempdir().expect("create wind classification fixture tempdir");
    fs::write(temp.path().join("package.json"), "{}\n").expect("write project package.json");
    fs::write(temp.path().join("entry.css"), "").expect("write CSS entrypoint");
    fs::write(
        temp.path().join("zfb.config.json"),
        format!(r#"{{"wind":{wind}}}"#),
    )
    .expect("write wind config");
    let path = temp.path().join("src/card.tsx");
    fs::create_dir_all(path.parent().expect("source parent")).expect("create src");
    fs::write(path, source).expect("write source");
    temp
}

fn run_wind_classification_css(project_root: &Path) -> Output {
    run_css(
        project_root,
        "entry.css",
        "out.css",
        Some("."),
        &["src/**/*.tsx"],
        &["--no-auto-source"],
    )
}

#[test]
fn css_command_bem_reset_only_project_builds() {
    let temp = wind_classification_fixture(
        r#"{"spec":1,"reset":"owned-v1"}"#,
        "export const N = () => <aside class=\"admonition__title card_title block__el--mod flex\" />;\n",
    );
    let output = run_wind_classification_css(temp.path());
    assert_success(&output, "BEM and snake_case authored classes");
    let css = fs::read_to_string(temp.path().join("out.css")).expect("read CSS output");
    assert!(
        css.contains(".flex"),
        "utility beside BEM names is missing:\n{css}"
    );
    for authored in ["admonition__title", "card_title", "block__el--mod"] {
        assert!(!css.contains(authored), "{authored} generated CSS:\n{css}");
    }
}

#[test]
fn css_command_conditional_ring_and_const_aria_fail_with_provenance() {
    let temp = wind_classification_fixture(
        r##"{"spec":1,"tokens":{"spacingUnit":"0.25rem","colors":{"accent":"#06c","muted":"#888","soft":"#eee"}}}"##,
        concat!(
            "const linkClass = \"px-3 aria-[current=page]:bg-soft\";\n",
            "export const B = ({ on }) => (\n",
            "  <button class={`flex border ${on ? \"border-accent ring-2\" : \"border-muted\"}`}>\n",
            "    <a class={linkClass}>x</a>\n",
            "  </button>\n",
            ");\n",
        ),
    );
    let output = run_wind_classification_css(temp.path());
    assert_failure(&output, "unsupported tokens in proven class expressions");
    let stderr = process_stderr(&output);
    assert!(stderr.contains("ZW004"), "{stderr}");
    let diagnostic_at = |location: &str, diagnostic: &str| {
        stderr
            .lines()
            .any(|line| line.contains(location) && line.contains(diagnostic))
    };
    assert!(
        diagnostic_at("card.tsx:3:", "ZW004 ring-2: "),
        "ring-2 must name its own line:\n{stderr}"
    );
    assert!(
        diagnostic_at("card.tsx:1:", "ZW004 aria-[current=page]:bg-soft: "),
        "const aria candidate must name its declaration:\n{stderr}"
    );
    assert!(
        !temp.path().join("out.css").exists(),
        "a failed run must not publish CSS"
    );
}

#[test]
fn css_command_foreign_names_warn_by_default_and_fail_under_strict() {
    let source = "export const C = () => <p class=\"line-clamp-2 flex\" />;\n";
    let temp = wind_classification_fixture(r#"{"spec":1}"#, source);
    let output = run_wind_classification_css(temp.path());
    assert_success(&output, "foreign utility at a class position, default");
    let combined = combined_output(&output);
    assert!(
        combined.contains("ZW014") && combined.contains("line-clamp-2"),
        "default run must warn:\n{combined}"
    );
    let css = fs::read_to_string(temp.path().join("out.css")).expect("read CSS output");
    assert!(css.contains(".flex"), "{css}");
    assert!(
        !css.contains("line-clamp"),
        "no CSS for a foreign name:\n{css}"
    );

    let strict = wind_classification_fixture(r#"{"spec":1,"strict":true}"#, source);
    let output = run_wind_classification_css(strict.path());
    assert_failure(&output, "foreign utility at a class position, strict");
    let stderr = process_stderr(&output);
    assert!(
        stderr
            .lines()
            .any(|line| line.contains("card.tsx:1:") && line.contains("ZW014 line-clamp-2: ")),
        "strict foreign name must keep its location:\n{stderr}"
    );
}

#[test]
fn css_command_unrelated_literals_never_become_strict() {
    let temp = wind_classification_fixture(
        r#"{"spec":1,"strict":true}"#,
        concat!(
            "const unused = \"line-clamp-2 ring-2\";\n",
            "it(\"renders container contents\", () => {});\n",
            "export const D = () => <div class=\"flex\" />;\n",
        ),
    );
    let output = run_wind_classification_css(temp.path());
    assert_success(&output, "strict mode with unrelated literals");
    let css = fs::read_to_string(temp.path().join("out.css")).expect("read CSS output");
    assert!(css.contains(".flex"), "{css}");
}

fn run_wind_explain_stdin(project_root: &Path, input: &str, flags: &[&str]) -> Output {
    use std::io::Write;
    let mut child = Command::new(zfb_binary!())
        .args(["wind", "explain", "--stdin", "--project-root"])
        .arg(project_root)
        .args(flags)
        .current_dir(project_root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn `zfb wind explain --stdin`");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write candidates");
    child
        .wait_with_output()
        .expect("wait for `zfb wind explain`")
}

#[test]
fn css_command_failures_list_each_location_once_on_stderr() {
    let temp = wind_classification_fixture(
        r#"{"spec":1}"#,
        "// 日本語\r\nexport const A = () => <div class=\"rounded-missing\" />;\r\nexport const B = () => <p class=\"rounded-missing\" />;\r\n",
    );
    let other = temp.path().join("src/other.tsx");
    fs::write(
        &other,
        "export const C = () => <i class=\"rounded-missing\" />;\n",
    )
    .expect("write second source");
    let output = run_wind_classification_css(temp.path());
    assert_failure(&output, "unknown token in two files");
    assert!(
        process_stdout(&output).is_empty(),
        "failures stay off stdout"
    );
    let stderr = process_stderr(&output);
    let lines: Vec<_> = stderr
        .lines()
        .filter(|line| line.contains("ZW006 rounded-missing: "))
        .collect();
    assert_eq!(
        lines.len(),
        3,
        "one line per distinct occurrence:\n{stderr}"
    );
    let column = "export const A = () => <div class=\"".len() + 1;
    assert!(
        lines
            .iter()
            .any(|line| line.contains(&format!("card.tsx:2:{column}: "))),
        "{stderr}"
    );
    assert!(
        lines.iter().any(|line| line.contains("other.tsx:1:")),
        "{stderr}"
    );
    assert!(
        !stderr.contains("; ZW006"),
        "no semicolon-joined list:\n{stderr}"
    );
}

#[test]
fn css_command_reports_every_forbidden_directive_in_one_run() {
    let temp = tempfile::tempdir().expect("create directive fixture");
    fs::write(temp.path().join("package.json"), "{}\n").expect("write package.json");
    fs::write(
        temp.path().join("entry.css"),
        "@import \"./theme.css\";\n@theme {}\n@theme static {}\n@source \"x\";\n",
    )
    .expect("write entry");
    fs::write(
        temp.path().join("theme.css"),
        ".a { color: red; }\n@apply p-1;\n",
    )
    .expect("write imported stylesheet");
    let output = run_css(temp.path(), "entry.css", "out.css", Some("."), &[], &[]);
    assert_failure(&output, "several forbidden directives");
    let stderr = process_stderr(&output);
    for location in [
        "entry.css:2:1",
        "entry.css:3:1",
        "entry.css:4:1",
        "theme.css:2:1",
    ] {
        assert!(
            stderr.contains(&format!("{location}: ZW009")),
            "{location}:\n{stderr}"
        );
    }
}

#[test]
fn wind_audit_json_is_stable_and_severity_never_changes_the_verdict() {
    let temp = wind_audit_fixture(
        r#"{"wind":{"spec":1}}"#,
        "import { h } from \"preact/hooks\";\nexport default () => <div class=\"rounded-missing\" />;\n",
    );
    let full = run_wind_audit(temp.path(), &["--json"]);
    assert!(full.status.success(), "{}", combined_output(&full));
    let stdout = process_stdout(&full);
    let document: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|error| panic!("{error}:\n{stdout}"));
    assert_eq!(document["schemaVersion"], 1);
    assert_eq!(document["command"], "audit");
    assert_eq!(document["coverage"]["mode"], "standalone");
    let diagnostic = document["report"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["candidate"] == "rounded-missing")
        .expect("diagnostic for the unknown token");
    let origin = &diagnostic["origin"];
    assert_eq!(origin["line"], 2);
    assert!(origin["byteColumn"].as_u64().unwrap() > 1);
    assert!(origin["byteOffset"].as_u64().unwrap() > origin["byteColumn"].as_u64().unwrap());
    assert!(
        !stdout.contains("preact/hooks"),
        "module specifier noise:\n{stdout}"
    );
    assert!(
        !stdout.contains("wind audit plan:"),
        "no prose on stdout:\n{stdout}"
    );
    assert_eq!(run_wind_audit(temp.path(), &["--json"]).stdout, full.stdout);

    for (severity, expected_info) in [("auditInfo", true), ("warning", false), ("error", false)] {
        let filtered = run_wind_audit(temp.path(), &["--json", "--severity", severity]);
        let value: serde_json::Value = serde_json::from_slice(&filtered.stdout).unwrap();
        let has_info = value["report"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["severity"] == "auditInfo");
        assert!(!has_info || expected_info, "{severity}: {value}");
        let strict = run_wind_audit(temp.path(), &["--fail-on", "error", "--severity", severity]);
        assert!(
            !strict.status.success(),
            "{severity} filter must not hide the verdict"
        );
    }
}

#[test]
fn wind_explain_batch_keeps_order_duplicates_and_leading_dashes() {
    let temp = wind_audit_fixture(r#"{"wind":{"spec":1}}"#, "export default () => null;\n");
    let output = run_wind_explain_stdin(
        temp.path(),
        "block\n\n-mt-px\nblock\nnot-a-utility\n",
        &["--json"],
    );
    assert!(output.status.success(), "{}", combined_output(&output));
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["schemaVersion"], 1);
    let candidates: Vec<_> = document["explanations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|explanation| explanation["candidate"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(candidates, ["block", "-mt-px", "block", "not-a-utility"]);

    let single = Command::new(zfb_binary!())
        .args(["wind", "explain", "--project-root"])
        .arg(temp.path())
        .args(["--", "-mt-px"])
        .output()
        .expect("spawn explain");
    assert!(single.status.success(), "{}", combined_output(&single));
    assert!(process_stdout(&single).contains("-mt-px"));
}

#[test]
fn wind_audit_ignores_viewport_root_links_style_text_and_hidden_inputs() {
    let temp = wind_audit_fixture(
        r#"{"wind":{"spec":1,"reset":"owned-v1"}}"#,
        concat!(
            "import \"../styles/global.css\";\n",
            "export default () => (\n",
            "  <html>\n",
            "    <head>\n",
            "      <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n",
            "      <style>{`.card { display: flex; }`}</style>\n",
            "    </head>\n",
            "    <body>\n",
            "      <a href=\"/\">Back to checks</a>\n",
            "      <input type=\"hidden\" name=\"token\" />\n",
            "    </body>\n",
            "  </html>\n",
            ");\n",
        ),
    );
    let output = run_wind_audit(temp.path(), &["--json"]);
    assert!(output.status.success(), "{}", combined_output(&output));
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document["report"]["diagnostics"],
        serde_json::json!([]),
        "standard markup must not produce audit noise"
    );

    fs::write(temp.path().join("entry.css"), "").expect("write entry");
    let css = run_css(temp.path(), "entry.css", "out.css", Some("."), &[], &[]);
    assert_success(&css, "markup without utilities");
    let emitted = fs::read_to_string(temp.path().join("out.css")).unwrap_or_default();
    for selector in [".flex", ".hidden"] {
        assert!(!emitted.contains(selector), "{selector} leaked:\n{emitted}");
    }
}
