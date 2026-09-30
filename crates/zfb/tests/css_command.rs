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
