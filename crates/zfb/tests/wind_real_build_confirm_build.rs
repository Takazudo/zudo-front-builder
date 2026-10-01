//! Real `zfb build` confirmation for wind cases W-A05 and W-A06 (#3269).
//!
//! Level 3/4 artifact evidence, tier T1. This binary self-skips only when
//! esbuild is unavailable locally; it is registered in nextest's unlocked
//! heavy lane because each case starts real `zfb build` processes.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};

use zfb_test_utils::{locate_esbuild, zfb_binary};

static BUILD_SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
const PACKAGE_ASSETS: &[(&str, &str)] = &[
    ("package-font", "woff2"),
    ("package-image", "svg"),
    ("nested-font", "woff2"),
    ("nested-image", "svg"),
];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("wind-assets")
}

fn wind_v3_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("wind-v3-functional")
}

fn copy_source_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let source_path = entry.path();
        let destination_path = destination.join(name.as_ref());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if matches!(name.as_ref(), "dist" | ".zfb-build" | ".zfb") {
                continue;
            }
            copy_source_tree(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

/// Add the binary's embedded framework packages beside the fixture package.
/// The tracked fixture has a real `node_modules/@fixture` directory, so the
/// whole node_modules root cannot be replaced with the embedded tree.
fn link_embedded_framework_packages(root: &Path) -> tempfile::TempDir {
    let (lease, embedded_node_modules) =
        zfb::render_pipeline::embedded_node_modules().expect("extract embedded node_modules");
    let project_node_modules = root.join("node_modules");
    fs::create_dir_all(&project_node_modules).expect("create fixture node_modules");
    for entry in fs::read_dir(&embedded_node_modules).expect("read embedded node_modules") {
        let entry = entry.expect("read embedded node_modules entry");
        let destination = project_node_modules.join(entry.file_name());
        if fs::symlink_metadata(&destination).is_ok() {
            continue;
        }
        symlink(entry.path(), destination).expect("link embedded framework package");
    }
    lease
}

fn copy_fixture_to_temp() -> (tempfile::TempDir, PathBuf, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("create wind-assets project tempdir");
    let root = temp.path().join("project");
    copy_source_tree(&fixture_dir(), &root).expect("copy wind-assets fixture");
    let embedded_node_modules = link_embedded_framework_packages(&root);
    (temp, root, embedded_node_modules)
}

fn copy_wind_v3_fixture_to_temp(label: &str) -> (tempfile::TempDir, PathBuf, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("create wind v3 confirmation tempdir");
    let root = temp.path().join(label);
    copy_source_tree(&wind_v3_fixture_dir(), &root).expect("copy wind v3 fixture");
    let embedded_node_modules = link_embedded_framework_packages(&root);
    (temp, root, embedded_node_modules)
}

fn run_build_output(root: &Path, esbuild: &Path) -> std::process::Output {
    Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn `zfb build`")
}

fn run_build(root: &Path, esbuild: &Path, label: &str) -> Option<String> {
    let output = run_build_output(root, esbuild);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let combined = format!("{stdout}{stderr}");
    if !output.status.success()
        && (combined.contains("embed_v8") || combined.contains("no esbuild"))
    {
        eprintln!("[{label}] known environment skip; {combined}");
        return None;
    }
    assert!(
        output.status.success(),
        "[{label}] `zfb build` failed with {:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status
    );
    Some(combined)
}

fn run_wind_audit_output(root: &Path, flags: &[&str]) -> std::process::Output {
    Command::new(zfb_binary!())
        .args(["wind", "audit", "--project-root"])
        .arg(root)
        .args(flags)
        .current_dir(root)
        .output()
        .expect("spawn `zfb wind audit`")
}

fn run_wind_audit(root: &Path) -> String {
    let output = run_wind_audit_output(root, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "`zfb wind audit` failed with {:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status
    );
    stdout
}

fn run_wind_explain(root: &Path, candidate: &str) -> std::process::Output {
    Command::new(zfb_binary!())
        .args(["wind", "explain", candidate, "--project-root"])
        .arg(root)
        .current_dir(root)
        .output()
        .expect("spawn `zfb wind explain`")
}

fn run_standalone_css(root: &Path) -> std::process::Output {
    Command::new(zfb_binary!())
        .args([
            "css",
            "--input",
            "styles/global.css",
            "--output",
            "../standalone.css",
            "--project-root",
            ".",
            "--source",
            ".",
            "--no-auto-source",
        ])
        .current_dir(root)
        .output()
        .expect("spawn standalone `zfb css`")
}

fn output_text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

fn set_default_transition_timing_function(root: &Path, timing: &str) {
    let config_path = root.join("zfb.config.json");
    let config = fs::read_to_string(&config_path).expect("read wind v3 config");
    let configured = config.replace(
        "\"spec\": 1,",
        &format!("\"spec\": 1,\n      \"defaultTransitionTimingFunction\": \"{timing}\","),
    );
    assert_ne!(
        configured, config,
        "wind spec field must exist in fixture config"
    );
    fs::write(config_path, configured).expect("configure default transition timing");
}

fn collect_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn read_stylesheet(dist: &Path) -> (PathBuf, String) {
    let mut stylesheets = collect_files(&dist.join("assets"))
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("styles-") && name.ends_with(".css"))
        });
    let stylesheet = stylesheets
        .next()
        .expect("expected dist/assets/styles-<hash>.css");
    assert!(
        stylesheets.next().is_none(),
        "expected one published global stylesheet"
    );
    let css = fs::read_to_string(&stylesheet).expect("read emitted global stylesheet");
    (stylesheet, css)
}

fn collect_html(dist: &Path) -> String {
    collect_files(dist)
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "html")
        })
        .map(|path| fs::read_to_string(path).expect("read emitted HTML"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn css_url_values(css: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = css;
    while let Some(start) = rest.find("url(") {
        rest = &rest[start + 4..];
        let Some(end) = rest.find(')') else {
            break;
        };
        let value = rest[..end].trim().trim_matches(['\'', '"']);
        urls.push(value.to_owned());
        rest = &rest[end + 1..];
    }
    urls
}

fn assert_package_companions(stylesheet: &Path, css: &str, source_assets: &[(&str, &str)]) {
    let urls = css_url_values(css);
    for (stem, extension) in source_assets {
        let prefix = format!("./{stem}-");
        let suffix = format!(".{extension}");
        let companion = urls
            .iter()
            .find(|url| url.starts_with(&prefix) && url.ends_with(&suffix))
            .unwrap_or_else(|| {
                panic!("missing package companion URL for {stem}.{extension}: {urls:#?}")
            });
        let hash = &companion[prefix.len()..companion.len() - suffix.len()];
        assert_eq!(
            hash.len(),
            8,
            "expected an eight-character asset hash in {companion}"
        );
        assert!(
            hash.chars().all(|character| character.is_ascii_hexdigit()),
            "expected hexadecimal content hash in {companion}"
        );
        let emitted = stylesheet
            .parent()
            .expect("stylesheet parent")
            .join(companion.trim_start_matches("./"));
        let source = fixture_dir()
            .join("node_modules/@fixture/wind-assets/assets")
            .join(format!("{stem}.{extension}"));
        assert_eq!(
            fs::read(&emitted).expect("read emitted package companion"),
            fs::read(&source).expect("read package source asset"),
            "companion bytes must come from the stylesheet's package asset"
        );
    }
}

fn prove_missing_companion_is_rejected(stylesheet: &Path, css: &str) {
    let companion_url = css_url_values(css)
        .into_iter()
        .find(|url| url.starts_with("./package-image-") && url.ends_with(".svg"))
        .expect("package image companion URL for negative proof");
    let companion = stylesheet
        .parent()
        .expect("stylesheet parent")
        .join(companion_url.trim_start_matches("./"));
    let filename = companion
        .file_name()
        .expect("companion filename")
        .to_string_lossy();
    let missing =
        companion.with_file_name(format!(".{filename}.{}.revert-proof", std::process::id()));
    fs::rename(&companion, &missing).expect("temporarily remove one package companion");

    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let assertion = catch_unwind(AssertUnwindSafe(|| {
        assert_package_companions(stylesheet, css, PACKAGE_ASSETS);
    }));
    std::panic::set_hook(previous_hook);
    fs::rename(&missing, &companion).expect("restore package companion after revert proof");

    let panic_message = assertion
        .expect_err("W-A06 package companion assertion must fail when its image is removed");
    let message = panic_message
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic_message.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    assert!(
        message.contains("read emitted package companion"),
        "missing companion should fail at the emitted-file assertion, got: {message}"
    );
    eprintln!("[W-A06] negative proof passed: deleting the package image companion fails the asset assertion.");
}

fn configure_base_build(root: &Path) {
    let config_path = root.join("zfb.config.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).expect("read fixture config"))
            .expect("parse fixture config");
    config["base"] = serde_json::Value::String("/wind-base/".into());
    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&config).expect("serialize base config"),
    )
    .expect("write base fixture config");

    let global_css_path = root.join("styles/global.css");
    let global_css = fs::read_to_string(&global_css_path).expect("read project CSS");
    let global_css = global_css
        .replace(
            "/assets/project-font.woff2",
            "/wind-base/assets/project-font.woff2",
        )
        .replace(
            "/assets/project-image.svg",
            "/wind-base/assets/project-image.svg",
        );
    fs::write(global_css_path, global_css).expect("write base-prefixed project CSS URLs");
}

#[test]
fn w_a05_client_only_literal_and_authored_classes_are_audited() {
    let _serial = BUILD_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[wind_real_build_confirm_build] no esbuild; skipping.");
        return;
    };
    let (_temp, root, _embedded_node_modules) = copy_fixture_to_temp();
    let Some(_build_log) = run_build(&root, &esbuild, "W-A05/W-A06 default base") else {
        return;
    };

    let dist = root.join("dist");
    let html = collect_html(&dist);
    let (_, css) = read_stylesheet(&dist);

    // W-A05: this literal exists only behind client state and therefore must
    // be absent from initial SSR HTML while its static rule is published.
    assert!(
        !html.contains("bg-client-only"),
        "the client-only branch unexpectedly rendered on the server"
    );
    assert!(
        css.contains(".bg-client-only"),
        "the full class literal in the client-side branch needs a generated rule"
    );
    assert!(
        html.contains("ordinary-card"),
        "an authored class must remain in the emitted HTML"
    );

    let audit = run_wind_audit(&root);
    assert!(
        !audit.contains("ordinary-card"),
        "authoredClasses must keep the ordinary class out of diagnostics and audit findings:\n{audit}"
    );
    assert!(
        ["bg-", "assembled"].iter().all(|fragment| {
            audit.contains(&format!(
                "{fragment} at default/components:assembled-class.tsx:"
            ))
        }),
        "the audit should name the dynamic fragment and its source origin:\n{audit}"
    );
    assert!(
        !audit.contains("bg-assembled"),
        "the assembled class must not be reported as a covered candidate:\n{audit}"
    );
    assert!(
        !css.contains(".bg-assembled"),
        "a class assembled from fragments must not get a generated rule"
    );
}

fn remove_assembled_class_probe(root: &Path) {
    let page_path = root.join("pages/index.tsx");
    let page = fs::read_to_string(&page_path).expect("read asset fixture page");
    let page = page
        .replace(
            "import AssembledClass from \"../components/assembled-class\";\n",
            "",
        )
        .replace("          <AssembledClass />\n", "");
    assert!(
        !page.contains("AssembledClass"),
        "W-A06 asset-only variant must remove the independent W-A05 probe"
    );
    fs::write(page_path, page).expect("write asset-only page variant");
    fs::remove_file(root.join("components/assembled-class.tsx"))
        .expect("remove unrelated dynamic-class probe for W-A06");
}

fn copy_first_build_dist_for_browser(dist: &Path) {
    if let Some(destination) = std::env::var_os("ZFB_WIND_REAL_BUILD_DIST") {
        let destination = PathBuf::from(destination);
        if destination.exists() {
            fs::remove_dir_all(&destination).expect("clear requested browser dist output");
        }
        copy_source_tree(dist, &destination).expect("copy first-build dist for browser suite");
        eprintln!(
            "[W-A06] copied first-build dist for the browser check to {}",
            destination.display()
        );
    }
}

fn assert_modules_and_global_css(dist: &Path, css: &str) {
    let html = collect_html(dist);

    // W-A06: CSS Modules and global authored CSS are present in the same
    // published global stylesheet, and the scoped module name agrees in HTML.
    assert!(
        css.contains(".global-authored-marker") && css.contains("--wind-global-marker"),
        "global authored CSS did not reach the published stylesheet"
    );
    let module_class = html
        .split(|character: char| character.is_whitespace() || matches!(character, '\"' | '\''))
        .find(|class| class.ends_with("_moduleMarker"))
        .expect("emitted HTML should contain the scoped CSS Module class");
    // lightningcss escapes a `-<digit>` scoped-name prefix in the selector (#3311).
    let mut module_selector = String::from(".");
    cssparser::serialize_identifier(module_class, &mut module_selector)
        .expect("writing to a String cannot fail");
    assert!(
        css.contains(&module_selector) && css.contains("--wind-module-marker"),
        "published CSS must contain the selector used by HTML: {module_selector}"
    );

    // The package stylesheet imports a nested sheet before declaring its own
    // rules. Both selectors must survive that import chain in cascade order;
    // their relative URLs are checked separately against the declaring files.
    let nested_rule = css
        .find(".asset-nested")
        .expect("nested package stylesheet rule in published CSS");
    let package_rule = css
        .find(".asset-package")
        .expect("package stylesheet rule in published CSS");
    let project_rule = css
        .find(".asset-project")
        .expect("project stylesheet rule in published CSS");
    assert!(
        nested_rule < package_rule && package_rule < project_rule,
        "nested package import, importing package stylesheet, and project rules must retain cascade order"
    );
}

fn assert_authored_and_package_assets(stylesheet: &Path, css: &str) {
    // Package assets are attributed to both the directly imported package
    // stylesheet and its nested import. Project URLs, external URLs, and the
    // inline data URL retain the authored spelling.
    assert_package_companions(stylesheet, css, PACKAGE_ASSETS);
    let urls = css_url_values(css);
    for authored_url in [
        "/assets/project-font.woff2",
        "/assets/project-image.svg",
        "https://assets.invalid/leave-unchanged.svg",
        "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'/%3E",
    ] {
        assert!(
            urls.iter().any(|url| url == authored_url),
            "project, external, and data references must remain unchanged; missing {authored_url:?} in:\n{css}"
        );
    }
}

#[test]
fn w_a06_real_build_preserves_package_assets_and_css_modules() {
    let _serial = BUILD_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[wind_real_build_confirm_build] no esbuild; skipping.");
        return;
    };
    let (_temp, root, _embedded_node_modules) = copy_fixture_to_temp();
    remove_assembled_class_probe(&root);
    let Some(_build_log) = run_build(&root, &esbuild, "W-A06 default base") else {
        return;
    };

    let dist = root.join("dist");
    copy_first_build_dist_for_browser(&dist);
    let (stylesheet, css) = read_stylesheet(&dist);
    assert_modules_and_global_css(&dist, &css);
    assert_authored_and_package_assets(&stylesheet, &css);
    prove_missing_companion_is_rejected(&stylesheet, &css);

    // A second real build sets `base`. Public project assets are copied under
    // that prefix, while authored CSS URLs retain their explicitly authored
    // prefix; package companions remain relative to the hashed stylesheet.
    configure_base_build(&root);
    let Some(_base_build_log) = run_build(&root, &esbuild, "W-A06 base path") else {
        return;
    };
    let base_dist = root.join("dist");
    let (base_stylesheet, base_css) = read_stylesheet(&base_dist);
    assert_modules_and_global_css(&base_dist, &base_css);
    assert!(
        base_dist
            .join("wind-base/assets/project-font.woff2")
            .is_file()
            && base_dist
                .join("wind-base/assets/project-image.svg")
                .is_file(),
        "public assets should be copied under the documented base path"
    );
    let base_urls = css_url_values(&base_css);
    for authored_url in [
        "/wind-base/assets/project-font.woff2",
        "/wind-base/assets/project-image.svg",
    ] {
        assert!(
            base_urls.iter().any(|url| url == authored_url),
            "project CSS URLs retain the base prefix authored by the project; missing {authored_url:?}"
        );
    }
    assert_package_companions(&base_stylesheet, &base_css, PACKAGE_ASSETS);
    for url in css_url_values(&base_css) {
        if url.starts_with("./package-") || url.starts_with("./nested-") {
            assert!(
                base_stylesheet
                    .parent()
                    .expect("base stylesheet parent")
                    .join(url.trim_start_matches("./"))
                    .is_file(),
                "relative package reference must resolve beside the base-build stylesheet: {url}"
            );
        }
    }
}

#[test]
fn w_a08_strict_wind_commands_report_hints_for_authored_and_malformed_classes() {
    let _serial = BUILD_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[wind_real_build_confirm_build] no esbuild; skipping W-A08 strict CLI proof.");
        return;
    };
    let (_temp, root, _embedded_node_modules) = copy_wind_v3_fixture_to_temp("invalid");

    let build = run_build_output(&root, &esbuild);
    let build_output = output_text(&build);
    if !build.status.success()
        && (build_output.contains("embed_v8") || build_output.contains("no esbuild"))
    {
        eprintln!("[W-A08 strict CLI] known environment skip; {build_output}");
        return;
    }
    assert!(
        !build.status.success(),
        "zfb build must reject malformed and unauthored candidates:\n{build_output}"
    );
    assert!(
        build_output.contains("aspect-1.5") && build_output.contains("aspect-[1.5/1]"),
        "zfb build must retain the malformed aspect candidate and its canonical hint:\n{build_output}"
    );

    let css = run_standalone_css(&root);
    let css_output = output_text(&css);
    assert!(
        !css.status.success(),
        "standalone zfb css must reject the same strict fixture:\n{css_output}"
    );
    assert!(
        css_output.contains("aspect-1.5") && css_output.contains("aspect-[1.5/1]"),
        "zfb css must retain the malformed aspect candidate and its canonical hint:\n{css_output}"
    );
    assert!(
        !root
            .parent()
            .expect("fixture parent")
            .join("standalone.css")
            .exists(),
        "failed standalone CSS compilation must not publish an output file"
    );

    let explain = run_wind_explain(&root, "aspect-1.5");
    let explain_output = output_text(&explain);
    assert!(
        explain.status.success() && explain_output.contains("aspect-[1.5/1]"),
        "wind explain must return the aspect spelling hint:\n{explain_output}"
    );

    let audit = run_wind_audit_output(&root, &["--fail-on", "error"]);
    let audit_output = output_text(&audit);
    assert!(
        !audit.status.success(),
        "strict wind audit must fail on error-severity findings:\n{audit_output}"
    );
    assert!(
        audit_output.contains("text-link")
            && audit_output.contains("wind.authoredClasses: { \"text-link\": true }")
            && audit_output.contains("aspect-[1.5/1]"),
        "wind audit must distinguish an authored class hint from the malformed aspect hint:\n{audit_output}"
    );
    assert!(
        audit_output.contains("unrecognized classes:\n  - ordinary-card"),
        "an ordinary class must be listed as unrecognized, not rejected as a utility:\n{audit_output}"
    );
}

#[test]
fn w_a08_real_build_css_explain_and_audit_agree_on_transition_timing() {
    let _serial = BUILD_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[wind_real_build_confirm_build] no esbuild; skipping W-A08 command parity.");
        return;
    };

    for (label, configured_timing, expected_timing) in [
        ("default", None, "ease"),
        ("configured", Some("linear"), "linear"),
    ] {
        let (_temp, root, _embedded_node_modules) = copy_wind_v3_fixture_to_temp(label);
        fs::remove_file(root.join("pages/diagnostics.tsx"))
            .expect("remove strict-only page from valid build fixture");
        if let Some(timing) = configured_timing {
            set_default_transition_timing_function(&root, timing);
        }

        let Some(_build_log) = run_build(&root, &esbuild, &format!("W-A08 {label} build")) else {
            return;
        };
        let dist = root.join("dist");
        let (stylesheet, build_css) = read_stylesheet(&dist);
        let html = collect_html(&dist);
        assert!(
            html.contains("aspect-16/9"),
            "new aspect syntax must reach emitted HTML"
        );
        assert!(
            build_css.contains("aspect-ratio: 16 / 9;"),
            "new aspect syntax must emit the expected aspect ratio:\n{build_css}"
        );
        assert!(
            build_css.contains(&format!("transition-timing-function: {expected_timing};")),
            "the default transition timing must reach build CSS:\n{build_css}"
        );
        assert!(
            build_css.contains("transition-timing-function: var(--zw-ease-gentle);"),
            "explicit easing token must remain an explicit utility:\n{build_css}"
        );
        assert!(
            build_css.contains("--zw-ease-gentle: ease-in-out;"),
            "the configured easing token must be emitted:\n{build_css}"
        );
        assert!(
            build_css.contains("--wind-reservation-control: present;")
                && !build_css.contains(".p-4"),
            "authored CSS must remain while its reserved utility stays suppressed:\n{build_css}"
        );
        assert!(
            html.contains("bg-red") && !build_css.contains(".bg-red"),
            "the runtime dynamic class must reach HTML without creating a completed CSS rule:\nHTML: {html}\nCSS: {build_css}"
        );

        let standalone = run_standalone_css(&root);
        let standalone_output = output_text(&standalone);
        assert!(
            standalone.status.success(),
            "standalone zfb css must succeed for the valid fixture:\n{standalone_output}"
        );
        let standalone_css = fs::read(root.parent().unwrap().join("standalone.css"))
            .expect("read standalone CSS output");
        assert_eq!(
            fs::read(stylesheet).expect("read build CSS bytes"),
            standalone_css,
            "build and standalone CSS must emit identical bytes for {label} timing"
        );

        let transition_explanation = run_wind_explain(&root, "transition-shadow");
        let explanation_output = output_text(&transition_explanation);
        assert!(
            transition_explanation.status.success()
                && explanation_output.contains(&format!(
                    "declaration: transition-timing-function: {expected_timing}"
                )),
            "wind explain must use the same {label} default as build/css:\n{explanation_output}"
        );
        let explicit_explanation = run_wind_explain(&root, "ease-gentle");
        let explicit_output = output_text(&explicit_explanation);
        assert!(
            explicit_explanation.status.success()
                && explicit_output.contains("ease-in-out")
                && explicit_output.contains("var(--zw-ease-gentle)"),
            "wind explain must report the explicit token and its configured value:\n{explicit_output}"
        );

        let audit = run_wind_audit(&root);
        assert!(
            audit.contains("outcome: complete")
                && audit.contains("bg-")
                && audit.contains("unrecognized classes:\n  - ordinary-card"),
            "wind audit must keep dynamic construction visible and classify ordinary classes separately:\n{audit}"
        );
    }
}
