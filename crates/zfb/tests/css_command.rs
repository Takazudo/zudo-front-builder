//! Level-3 real-engine integration coverage for `zfb css` (#2600).
//!
//! These tests inspect emitted CSS bytes and subprocess exit diagnostics. They
//! use the real staged Tailwind v4 executable, so every scenario is an
//! env-gated ignored test. The fixtures deliberately live below
//! `tests/fixtures/css-*`; each test copies its fixture to a fresh temporary
//! project before invoking the already-built `zfb` binary.
//!
//! Binary design: css-only scenarios and the build-parity scenario stay in one
//! test binary so the command contract has one obvious scoped CI step. The
//! parity scenario also spawns `zfb build` (and therefore esbuild); the whole
//! binary is registered in nextest's `e2e-heavy` build-only bucket. This is
//! intentional: the extra cross-binary serialization keeps this build leg away
//! from the other V8/esbuild binaries. Each test uses its own temp project,
//! while the Tailwind engine's cross-process warm-up lock handles its own
//! concurrent subprocesses within this binary.
//!
//! The health workflow runs this binary directly with `--ignored` and both
//! staged binary paths. The weekly exam runs the same test names through its
//! exact-name ignored filterset. No browser or held-open server is involved.

use std::env;
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

/// Resolve exactly the Tailwind slot used by the health/exam env-gate steps.
/// An explicit operator override wins; unlike esbuild, this lookup deliberately
/// has no PATH/pnpm fallback so a local run cannot accidentally use another
/// Tailwind major version.
fn locate_tailwind() -> Option<PathBuf> {
    if let Some(path) = env::var_os("ZFB_TAILWIND_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let slot = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/tailwindcss-v4");
    slot.is_file().then_some(slot)
}

fn skip_without_tailwind(test: &str) -> Option<PathBuf> {
    let path = locate_tailwind();
    if path.is_none() {
        eprintln!(
            "[{test}] no staged Tailwind v4 binary; skipping. Set ZFB_TAILWIND_BIN or stage crates/zfb/binaries/tailwindcss-v4."
        );
    }
    path
}

fn run_css(
    project_root: &Path,
    tailwind: &Path,
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
        .current_dir(project_root)
        .env("ZFB_TAILWIND_BIN", tailwind);
    if let Some(root) = explicit_project_root {
        command.args(["--project-root", root]);
    }
    for source in sources {
        command.args(["--source", source]);
    }
    command.args(flags).output().expect("spawn `zfb css`")
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

fn has_date_or_time_shape(bytes: &[u8]) -> bool {
    bytes.windows(10).any(|window| {
        window[4] == b'-'
            && window[7] == b'-'
            && window[..4].iter().all(|byte| byte.is_ascii_digit())
            && window[5..7].iter().all(|byte| byte.is_ascii_digit())
            && window[8..].iter().all(|byte| byte.is_ascii_digit())
    }) || bytes.windows(10).any(|window| {
        window[4] == b'/'
            && window[7] == b'/'
            && window[..4].iter().all(|byte| byte.is_ascii_digit())
            && window[5..7].iter().all(|byte| byte.is_ascii_digit())
            && window[8..].iter().all(|byte| byte.is_ascii_digit())
    }) || bytes.windows(8).any(|window| {
        window[2] == b':'
            && window[5] == b':'
            && window[..2].iter().all(|byte| byte.is_ascii_digit())
            && window[3..5].iter().all(|byte| byte.is_ascii_digit())
            && window[6..].iter().all(|byte| byte.is_ascii_digit())
    })
}

fn assert_deterministic_css(bytes: &[u8], project_root: &Path) {
    let text = String::from_utf8(bytes.to_vec()).expect("Tailwind output must be UTF-8 CSS");
    assert!(
        text.contains("/*! tailwindcss v4.2.0"),
        "Tailwind's version banner must pass through unchanged"
    );
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
    let temp_root_path = env::temp_dir();
    let temp_root = temp_root_path.to_string_lossy();
    assert!(
        !text.contains(temp_root.as_ref()),
        "CSS output leaked the OS temp directory: {temp_root:?}"
    );
    for fragment in ["zfb-tailwind-entry-", "zfb-tailwind-out-"] {
        assert!(
            !text.contains(fragment),
            "CSS output leaked a temporary filename prefix {fragment:?}"
        );
    }
    for fragment in [
        "/private/tmp/",
        "/tmp/",
        "/var/folders/",
        "/Users/",
        "/home/",
        "file://",
    ] {
        assert!(
            !text.contains(fragment),
            "CSS output leaked an absolute filesystem path fragment {fragment:?}"
        );
    }
    assert!(
        !bytes.windows(3).enumerate().any(|(index, window)| {
            window[0].is_ascii_alphabetic()
                && window[1] == b':'
                && matches!(window[2], b'/' | b'\\')
                // Do not mistake the `s:/` suffix of `https://` for a
                // Windows drive path.
                && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
        }),
        "CSS output leaked a Windows absolute filesystem path"
    );
    assert!(
        !has_date_or_time_shape(bytes),
        "CSS output must not contain a timestamp or date-like token"
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
#[ignore = "env-gate: tailwindcss v4 — requires ZFB_TAILWIND_BIN or the staged binary"]
fn css_command_output_is_deterministic_and_matches_committed_golden() {
    let Some(tailwind) = skip_without_tailwind("css_command_determinism") else {
        return;
    };
    let temp = copied_fixture("css-determinism");

    let first = run_css(
        temp.path(),
        &tailwind,
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
        &tailwind,
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
        "two independent real-engine CSS runs must be byte-identical"
    );
    assert_eq!(
        first_bytes, golden,
        "real-engine output must match the committed minimal CSS golden"
    );
    assert_deterministic_css(&first_bytes, temp.path());
}

#[test]
#[ignore = "env-gate: tailwindcss v4 — requires ZFB_TAILWIND_BIN or the staged binary"]
fn css_command_highlight_class_default_and_inline_modes() {
    let Some(tailwind) = skip_without_tailwind("css_command_highlight_modes") else {
        return;
    };
    let temp = copied_fixture("css-highlight");

    let default_mode = run_css(
        temp.path(),
        &tailwind,
        "entry.css",
        "default.css",
        Some("."),
        &["src/index.html"],
        &["--no-auto-source"],
    );
    assert_success(&default_mode, "config-provided class highlight mode");
    let default_css = fs::read_to_string(temp.path().join("default.css")).unwrap();
    assert!(
        default_css.contains("--zfb-hi-"),
        "class mode must emit zfb tokens"
    );
    assert!(
        default_css.contains(".hi-kw"),
        "class mode must emit semantic role rules"
    );

    let class_mode = run_css(
        temp.path(),
        &tailwind,
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
        &tailwind,
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
        &tailwind,
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
#[ignore = "env-gate: tailwindcss v4 — requires ZFB_TAILWIND_BIN or the staged binary"]
fn css_command_explicit_sources_isolate_ambient_decoy() {
    let Some(tailwind) = skip_without_tailwind("css_command_explicit_source") else {
        return;
    };
    let outer = tempfile::tempdir().expect("create explicit-source parent tempdir");
    let project = outer.path().join("project");
    copy_dir_recursive(&fixture_dir("css-explicit-source"), &project)
        .expect("copy explicit-source fixture");

    // Keep this decoy outside the explicit source set, in a default content
    // root, while the temporary project has no `.git` directory. That makes
    // it outside any git-visible root but still visible to Tailwind's ambient
    // detector: if `zfb css` accidentally leaves ambient detection on, this
    // file is the only place the decoy class can come from.
    fs::create_dir_all(project.join("components")).unwrap();
    fs::write(
        project.join("components/ambient-decoy.html"),
        "<div class=\"bg-[#cc44dd]\"></div>\n",
    )
    .unwrap();

    let output = run_css(
        &project,
        &tailwind,
        "entry.css",
        "compiled.css",
        Some("."),
        &["src/allowed.html"],
        &["--no-auto-source"],
    );
    assert_success(&output, "explicit-source CSS compilation");
    let css = fs::read_to_string(project.join("compiled.css")).unwrap();
    assert!(
        css.contains("bg-\\[\\#11aa22\\]"),
        "the explicitly named utility must be emitted:\n{css}"
    );
    assert!(
        !css.contains("bg-\\[\\#cc44dd\\]") && !css.contains("#cc44dd"),
        "the ambient decoy outside the explicit source set leaked into CSS:\n{css}"
    );
}

#[test]
#[ignore = "env-gate: tailwindcss v4 — requires ZFB_TAILWIND_BIN or the staged binary"]
fn css_command_missing_input_exits_nonzero() {
    let Some(tailwind) = skip_without_tailwind("css_command_missing_input") else {
        return;
    };
    let temp = copied_fixture("css-failures");
    let output = run_css(
        temp.path(),
        &tailwind,
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
#[ignore = "env-gate: tailwindcss v4 — requires ZFB_TAILWIND_BIN or the staged binary"]
fn css_command_zero_match_source_glob_exits_nonzero() {
    let Some(tailwind) = skip_without_tailwind("css_command_zero_match_source") else {
        return;
    };
    let temp = copied_fixture("css-failures");
    let output = run_css(
        temp.path(),
        &tailwind,
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
#[ignore = "env-gate: tailwindcss v4 — requires ZFB_TAILWIND_BIN or the staged binary"]
fn css_command_same_canonical_input_output_exits_nonzero() {
    let Some(tailwind) = skip_without_tailwind("css_command_same_path") else {
        return;
    };
    let temp = copied_fixture("css-failures");
    std::os::unix::fs::symlink("entry.css", temp.path().join("alias.css"))
        .expect("create input/output alias");
    let output = run_css(
        temp.path(),
        &tailwind,
        "entry.css",
        "alias.css",
        Some("."),
        &["source.html"],
        &["--no-auto-source"],
    );
    assert_failure(&output, "same canonical CSS input/output");
    let diagnostics = combined_output(&output);
    assert!(diagnostics.contains("same path"), "{diagnostics}");
}

#[test]
#[ignore = "env-gate: tailwindcss v4 — requires ZFB_TAILWIND_BIN or the staged binary"]
fn css_command_missing_relative_import_exits_nonzero() {
    let Some(tailwind) = skip_without_tailwind("css_command_missing_import") else {
        return;
    };
    let temp = copied_fixture("css-failures");
    let output = run_css(
        temp.path(),
        &tailwind,
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
        diagnostics.contains("resolve") || diagnostics.contains("Tailwind CSS compilation failed")
    );
    assert!(!temp.path().join("compiled.css").exists());
}

#[test]
#[ignore = "env-gate: tailwindcss v4 — requires ZFB_TAILWIND_BIN or the staged binary"]
fn css_command_real_unresolved_apply_exits_nonzero() {
    let Some(tailwind) = skip_without_tailwind("css_command_unresolved_apply") else {
        return;
    };
    let temp = copied_fixture("css-failures");
    let output = run_css(
        temp.path(),
        &tailwind,
        "unresolved-apply.css",
        "compiled.css",
        Some("."),
        &["source.html"],
        &["--no-auto-source"],
    );
    assert_failure(&output, "real Tailwind unresolved @apply directive");
    let diagnostics = combined_output(&output);
    assert!(
        diagnostics.contains("totally-not-a-real-utility"),
        "the unresolved utility must be named:\n{diagnostics}"
    );
    assert!(
        diagnostics.contains("Cannot apply unknown utility class")
            || diagnostics.contains("unknown utility"),
        "the real engine must identify the unresolved @apply:\n{diagnostics}"
    );
    assert!(!temp.path().join("compiled.css").exists());
}

#[test]
#[ignore = "env-gate: tailwindcss v4 — requires ZFB_TAILWIND_BIN or the staged binary"]
fn css_command_atomic_success_replaces_and_failure_preserves_output() {
    let Some(tailwind) = skip_without_tailwind("css_command_atomicity") else {
        return;
    };

    let success = copied_fixture("css-atomic");
    fs::write(success.path().join("compiled.css"), b"SUCCESS_SENTINEL").unwrap();
    let success_output = run_css(
        success.path(),
        &tailwind,
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
    assert!(success_css.contains("/*! tailwindcss v4.2.0"));
    assert!(success_css.contains("#334455"));
    assert!(!success_css.contains("SUCCESS_SENTINEL"));

    let failure = copied_fixture("css-atomic");
    fs::write(
        failure.path().join("entry.css"),
        "@import \"tailwindcss\";\n.broken { @apply totally-not-a-real-utility; }\n",
    )
    .unwrap();
    fs::write(failure.path().join("compiled.css"), b"FAILURE_SENTINEL").unwrap();
    let failure_output = run_css(
        failure.path(),
        &tailwind,
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
        "failed CSS compilation must leave the previous output bytes untouched"
    );
}

#[test]
#[ignore = "env-gate: tailwindcss v4 + esbuild — requires the staged Tailwind and esbuild binaries"]
fn css_command_matches_build_stylesheet_for_equivalent_explicit_source_plan() {
    let Some(tailwind) = skip_without_tailwind("css_command_build_parity") else {
        return;
    };
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
        .env("ZFB_TAILWIND_BIN", &tailwind)
        .output()
        .expect("spawn `zfb build` for CSS parity");
    assert_success(&build, "build-parity zfb build");
    let build_css_path = find_build_css(&build_project);
    let build_css = fs::read(&build_css_path).expect("read hashed build stylesheet");

    // `zfb build` leaves Tailwind's ambient source detection enabled, and its
    // build bundle is part of that ambient root. Copy only that generated
    // bundle (not `dist/`) into the otherwise fresh css project, then use
    // `--source .` as the equivalent explicit plan. This scans the same
    // source set while exercising the standalone command's source(none)
    // contract without allowing the build's already-emitted CSS to mask a
    // missing utility. The fixture has no CSS Modules, and there is no
    // framework CSS override, so the complete hashed build asset is exactly
    // the Tailwind utility/framework portion being compared.
    copy_dir_recursive(
        &build_project.join(".zfb-build"),
        &css_project.join(".zfb-build"),
    )
    .expect("copy the build bundle into the equivalent CSS source plan");
    let standalone = run_css(
        &css_project,
        &tailwind,
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
        build_css, standalone_css,
        "the hashed `zfb build` stylesheet {} and equivalent explicit-plan `zfb css` output must be byte-identical",
        build_css_path.display()
    );
}
