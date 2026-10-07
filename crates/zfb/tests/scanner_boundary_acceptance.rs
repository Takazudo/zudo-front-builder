//! Real production acceptance for boundary-target discovery (#3508).
//!
//! This test builds a consumer that installs a real `npm pack` tarball. The
//! positive graph covers unused duplicate helpers, live helper/resource imports,
//! SDK and factory aliases, factory-member targets, the package composition
//! seam, default/named/namespace access, explicit/star/cyclic barrels, package
//! wrappers, direct calls, packed jsx/jsxs/jsxDEV calls, and minified runtime
//! identity. Invalid consumers prove target collisions, dynamic children, and
//! unsupported factory-member shapes fail the production CLI.
//!
//! The `AliasCounter` / `AliasNamedCounter` fixture cases adapt the documented
//! MDX components-map recipe (they are not verbatim copies) to keep the bare
//! `zfb` and `zfb/zudo-react` imports visible beside package-name cases.
//!
//! The positive `dist/` is copied to `target/scanner-boundary-acceptance/` for
//! the companion Playwright test. Its browser run is a separate guarded step so
//! the real build and Chromium never compete for machine resources.

#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;
use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

const EXPECTED_MARKERS: &[&str] = &[
    "AliasCounter",
    "AliasNamedCounter",
    "ConsumerA",
    "ConsumerB",
    "Counter",
    "DefaultPanel",
    "EqualDisplayName",
    "FactoryCounter",
    "HostPanel",
    "InnerName",
    "InferredArrow",
    "LiveCounter",
    "NamedDefault",
    "NamedCounter",
    "PackedCounter",
    "PreferredTarget",
    "default",
];

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scanner-boundary-acceptance")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create destination");
    for entry in fs::read_dir(source).expect("read fixture directory") {
        let entry = entry.expect("read fixture entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("read fixture entry type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

fn write(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("fixture path parent")).expect("create parent");
    fs::write(path, body).expect("write fixture source");
}

/// Copy the binary's embedded packages into the consumer's real node_modules
/// after npm installs the fixture tarball. npm may prune pre-existing
/// extraneous packages, so install the packed consumer first.
fn materialize_embedded_node_modules(root: &Path) {
    let (_lease, embedded) =
        zfb::render_pipeline::embedded_node_modules().expect("extract embedded @takazudo packages");
    copy_dir(&embedded, &root.join("node_modules"));
    for package in [
        "@takazudo/zfb/package.json",
        "@takazudo/zfb-runtime/package.json",
    ] {
        assert!(
            root.join("node_modules").join(package).is_file(),
            "embedded runtime package was not preserved: {package}"
        );
    }
}

fn node_available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn install_packed_widgets(root: &Path) -> PathBuf {
    let package_source = root.join("package-source");
    let tarballs = root.join(".scanner-boundary-tarballs");
    fs::create_dir_all(&tarballs).expect("create tarball directory");
    let packed = Command::new("npm")
        .args(["pack", "--json", "--pack-destination"])
        .arg(&tarballs)
        .current_dir(&package_source)
        .output()
        .expect("spawn npm pack for fixture package");
    assert!(
        packed.status.success(),
        "npm pack failed for the fixture package\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&packed.stdout),
        String::from_utf8_lossy(&packed.stderr)
    );
    let mut archives: Vec<_> = fs::read_dir(&tarballs)
        .expect("read packed tarballs")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "tgz"))
        .collect();
    archives.sort();
    assert_eq!(
        archives.len(),
        1,
        "npm pack must create one fixture tarball"
    );
    let archive = archives.pop().expect("packed fixture tarball");

    let installed = Command::new("npm")
        .args([
            "install",
            "--offline",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--no-save",
            "--package-lock=false",
        ])
        .arg(&archive)
        .current_dir(root)
        .output()
        .expect("spawn npm install for packed fixture");
    assert!(
        installed.status.success(),
        "npm install of the packed fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&installed.stdout),
        String::from_utf8_lossy(&installed.stderr)
    );

    let package = root.join("node_modules/@fixture/widgets");
    let package_metadata = fs::symlink_metadata(&package).expect("inspect installed tarball");
    assert!(
        package_metadata.is_dir() && !package_metadata.file_type().is_symlink(),
        "@fixture/widgets must be the installed directory from a real tarball"
    );
    let package_json: serde_json::Value = serde_json::from_slice(
        &fs::read(package.join("package.json")).expect("read installed package metadata"),
    )
    .expect("parse installed package metadata");
    assert_eq!(package_json["name"], "@fixture/widgets");
    assert_eq!(package_json["version"], "1.0.0");
    eprintln!(
        "[scanner_boundary_acceptance] installed packed consumer package {}",
        archive.display()
    );
    archive
}

fn run_build(root: &Path, esbuild: &Path) -> Output {
    Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn production zfb build")
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn extract_html_markers(html: &str) -> Vec<String> {
    let attribute = "data-zfb-island=\"";
    html.match_indices(attribute)
        .filter_map(|(offset, _)| {
            html[offset + attribute.len()..]
                .split_once('"')
                .map(|(name, _)| name.to_string())
        })
        .collect()
}

fn json_array_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' => depth += 1,
            b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Read the exact static SSR allowlist embedded in the real server bundle.
fn extract_ssr_allowlist(bundle: &str) -> Option<Vec<String>> {
    let key = "zudoReactIslands";
    let mut from = 0usize;
    while let Some(relative) = bundle.get(from..)?.find(key) {
        let key_end = from + relative + key.len();
        let nearby = bundle.get(key_end..)?;
        if let Some(equal_relative) = nearby.find('=') {
            if !nearby[..equal_relative].trim().is_empty() {
                from = key_end;
                continue;
            }
            let assignment = nearby.get(equal_relative + 1..)?;
            let leading_space = assignment.len() - assignment.trim_start().len();
            let array_start = key_end + equal_relative + 1 + leading_space;
            if bundle.as_bytes().get(array_start) != Some(&b'[') {
                from = key_end;
                continue;
            }
            if let Some(array_end) = json_array_end(bundle, array_start) {
                if let Ok(names) =
                    serde_json::from_str::<Vec<String>>(&bundle[array_start..array_end])
                {
                    return Some(names);
                }
            }
        }
        from = key_end;
    }
    None
}

fn expected_marker_set() -> BTreeSet<String> {
    EXPECTED_MARKERS
        .iter()
        .map(|marker| marker.to_string())
        .collect()
}

fn expected_home_marker_set() -> BTreeSet<String> {
    expected_marker_set()
        .into_iter()
        .filter(|marker| marker != "HostPanel")
        .collect()
}

const COMPOSITION_ROUTES: &[(&str, &[&str], bool)] = &[
    ("factory-static-spread", &["FactoryCounter"], false),
    ("factory-composition-default", &["DefaultPanel"], true),
    ("factory-composition-host-override", &["HostPanel"], true),
    ("factory-composition-settings-disabled", &[], false),
    ("factory-composition-default-suppressed", &[], false),
    (
        "factory-composition-host-with-default-suppressed",
        &["HostPanel"],
        true,
    ),
    ("factory-map-default", &["DefaultPanel"], true),
    ("factory-map-host-override", &["HostPanel"], true),
    ("factory-map-settings-disabled", &[], false),
];

fn write_factory_override_control_page(root: &Path) {
    write(
        root,
        "pages/factory-target-overrides/index.tsx",
        r#"import { Island } from "@takazudo/zfb";
import { FactoryCounter } from "../../components/factory-counter";
import { HostPanel } from "../../components/host-panel";

const defaults = { FactoryCounter };
const override = { FactoryCounter: HostPanel };

function createExplicitLast(deps: { FactoryCounter: typeof FactoryCounter }) {
  const Target = deps.FactoryCounter;
  return function ExplicitLast() { return <><Island><Target /></Island></>; };
}
function createSpreadLast(deps: { FactoryCounter: typeof FactoryCounter }) {
  const Target = deps.FactoryCounter;
  return function SpreadLast() { return <><Island><Target /></Island></>; };
}
function createSourceOverrideLast(deps: { FactoryCounter: typeof FactoryCounter }) {
  const Target = deps.FactoryCounter;
  return function SourceOverrideLast() { return <><Island><Target /></Island></>; };
}
function createDefaultsLast(deps: { FactoryCounter: typeof FactoryCounter }) {
  const Target = deps.FactoryCounter;
  return function DefaultsLast() { return <><Island><Target /></Island></>; };
}

const ExplicitLast = createExplicitLast({ ...defaults, FactoryCounter: HostPanel });
const SpreadLast = createSpreadLast({ FactoryCounter: HostPanel, ...defaults });
const SourceOverrideLast = createSourceOverrideLast({ ...defaults, ...override });
const DefaultsLast = createDefaultsLast({ ...override, ...defaults });

export default function Page() {
  return <html><body>
    <ExplicitLast />
    <SpreadLast />
    <SourceOverrideLast />
    <DefaultsLast />
  </body></html>;
}
"#,
    );
}

fn assert_no_published_islands(root: &Path) {
    let assets = root.join("dist/assets");
    let found = fs::read_dir(&assets)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("islands-") || name == "islands.js")
        .collect::<Vec<_>>();
    assert!(
        found.is_empty(),
        "failed build published an islands registry: {found:?}"
    );
}

fn make_minimal_project(root: &Path) {
    fs::create_dir_all(root).expect("create invalid fixture project");
    fs::write(
        root.join("package.json"),
        r#"{"private":true,"type":"module"}"#,
    )
    .expect("write invalid fixture package manifest");
    fs::write(root.join("zfb.config.json"), r#"{"wind":false}"#)
        .expect("write invalid fixture config");
    materialize_embedded_node_modules(root);
}

struct DevServerGuard {
    child: Child,
    pgid: libc::pid_t,
}

impl DevServerGuard {
    fn try_exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().expect("try_wait on zfb dev")
    }
}

impl Drop for DevServerGuard {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

struct DevSession {
    guard: DevServerGuard,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl DevSession {
    fn stdout(&self) -> String {
        fs::read_to_string(&self.stdout_path).unwrap_or_default()
    }

    fn stderr(&self) -> String {
        fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }

    fn logs(&self) -> String {
        format!(
            "--- zfb dev stdout ---\n{}\n--- zfb dev stderr ---\n{}",
            self.stdout(),
            self.stderr()
        )
    }
}

fn spawn_dev(root: &Path, esbuild: &Path) -> DevSession {
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout = fs::File::create(&stdout_path).expect("create dev stdout log");
    let stderr = fs::File::create(&stderr_path).expect("create dev stderr log");
    let mut command = Command::new(zfb_binary!());
    command
        .arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .env("ZFB_DEV_TIMING", "1")
        // Keep this integration assertion focused on the scanner tick: lazy
        // rendering intentionally defers route work until another request,
        // while this test needs each edit to complete (or fail) before it
        // verifies the last-good registry and recovery.
        .env("ZFB_DEV_EAGER", "1")
        .env("ZFB_DEV_BOOT_LAZY", "0")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_ORCH_PANIC_ON_TICK")
        .env_remove("ZFB_DEV_TEST_ORCH_STOP_MS")
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    command.process_group(0);
    let child = command.spawn().expect("spawn zfb dev");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(index) = rest.find("http://") {
        let candidate = &rest[index + "http://".len()..];
        let authority = candidate.split_whitespace().next().unwrap_or_default();
        if let Some(colon) = authority.rfind(':') {
            let digits = authority[colon + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
        rest = &rest[index + "http://".len()..];
    }
    None
}

async fn wait_for_dev_ready(session: &mut DevSession) -> u16 {
    let started = Instant::now();
    let deadline = Duration::from_secs(120);
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            panic!(
                "zfb dev exited before readiness with {status:?}\n{}",
                session.logs()
            );
        }
        if let Some(port) = parse_ready_port(&session.stdout()) {
            assert_ne!(port, 0, "zfb dev ready banner reported port zero");
            return port;
        }
        assert!(
            started.elapsed() < deadline,
            "zfb dev did not become ready within {}s\n{}",
            deadline.as_secs(),
            session.logs()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn appended_logs(session: &DevSession, old_stdout: &str, old_stderr: &str) -> String {
    let stdout = session.stdout();
    let stderr = session.stderr();
    format!(
        "{}\n{}",
        stdout.strip_prefix(old_stdout).unwrap_or(&stdout),
        stderr.strip_prefix(old_stderr).unwrap_or(&stderr)
    )
}

async fn wait_for_dev_log(
    session: &mut DevSession,
    old_stdout: &str,
    old_stderr: &str,
    needle: &str,
) -> String {
    let started = Instant::now();
    let deadline = Duration::from_secs(60);
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            panic!(
                "zfb dev exited while waiting for {needle:?} ({status:?})\n{}",
                session.logs()
            );
        }
        let appended = appended_logs(session, old_stdout, old_stderr);
        if appended.contains(needle) {
            return appended;
        }
        assert!(
            started.elapsed() < deadline,
            "zfb dev did not log {needle:?} within {}s\n{}",
            deadline.as_secs(),
            session.logs()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_dev_page_marker(session: &mut DevSession, port: u16, marker: &str) -> String {
    let started = Instant::now();
    let deadline = Duration::from_secs(90);
    let url = format!("http://127.0.0.1:{port}/");
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            panic!(
                "zfb dev exited while waiting for page marker {marker:?} ({status:?})\n{}",
                session.logs()
            );
        }
        if let Ok(response) = reqwest::get(&url).await {
            if response.status().is_success() {
                if let Ok(body) = response.text().await {
                    if body.contains(marker) {
                        return body;
                    }
                }
            }
        }
        assert!(
            started.elapsed() < deadline,
            "dev page did not contain {marker:?} within {}s\n{}",
            deadline.as_secs(),
            session.logs()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[test]
fn production_and_packed_consumers_validate_exact_boundary_registry() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let esbuild = locate_esbuild().expect(
        "scanner boundary acceptance requires the pinned esbuild binary; stage it in the test environment",
    );
    assert!(
        node_available("node"),
        "scanner boundary acceptance requires Node.js"
    );
    assert!(
        node_available("npm"),
        "scanner boundary acceptance requires npm"
    );

    let scratch = tempfile::tempdir().expect("acceptance scratch directory");
    let workspace = workspace_root();

    // Build the accepted local + packed graph with actual npm pack/install
    // semantics. The installed package is a physical node_modules directory,
    // not a workspace link.
    let positive = scratch.path().join("positive-consumer");
    copy_dir(&fixture_root(), &positive);
    install_packed_widgets(&positive);
    materialize_embedded_node_modules(&positive);
    write_factory_override_control_page(&positive);
    let built = run_build(&positive, &esbuild);
    assert!(
        built.status.success(),
        "positive packed-consumer production build failed\n{}",
        combined_output(&built)
    );

    let html = fs::read_to_string(positive.join("dist/index.html")).expect("read built page");
    assert!(
        html.contains("LOCAL_LIVE_RESOURCE"),
        "live local helper resource missing: {html}"
    );
    assert!(
        html.contains("PACKED_LIVE_RESOURCE"),
        "live packed helper resource missing: {html}"
    );
    let actual_html_markers: BTreeSet<_> = extract_html_markers(&html).into_iter().collect();
    assert_eq!(
        actual_html_markers,
        expected_home_marker_set(),
        "home SSR must emit exactly the concrete target markers on that route"
    );
    let host_override_html = fs::read_to_string(positive.join("dist/host-override/index.html"))
        .expect("read host-override route HTML");
    assert_eq!(
        extract_html_markers(&host_override_html)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["HostPanel".to_string()]),
        "host override route must render the host-owned target only"
    );
    let override_html =
        fs::read_to_string(positive.join("dist/factory-target-overrides/index.html"))
            .expect("read fresh-consumer override control route HTML");
    assert_eq!(
        extract_html_markers(&override_html),
        vec![
            "HostPanel".to_string(),
            "FactoryCounter".to_string(),
            "HostPanel".to_string(),
            "FactoryCounter".to_string(),
        ],
        "explicit/source and source/source override directions must select the rightmost target"
    );
    let mut route_expectations = serde_json::Map::new();
    for (route, markers, feature_shim) in COMPOSITION_ROUTES {
        let route_html = fs::read_to_string(positive.join(format!("dist/{route}/index.html")))
            .unwrap_or_else(|error| panic!("read {route} route HTML: {error}"));
        let actual = extract_html_markers(&route_html);
        let expected_markers: Vec<String> = markers.iter().map(|marker| (*marker).into()).collect();
        assert_eq!(
            actual, expected_markers,
            "{route} must emit the exact selected markers"
        );
        assert_eq!(
            route_html.contains("id=\"factory-feature-shim\""),
            *feature_shim,
            "{route} must emit the feature shim only when a feature boundary is rendered"
        );
        if markers.is_empty() {
            assert!(!route_html.contains("data-zfb-island-mounted"), "{route}");
            assert!(!route_html.contains("data-zfb-island-skip-ssr"), "{route}");
            assert!(
                !route_html.contains("data-zfb-island-skip-ssr-mounted"),
                "{route}"
            );
            assert!(!route_html.contains("id=\"default-panel\""), "{route}");
            assert!(!route_html.contains("id=\"host-panel\""), "{route}");
        }
        route_expectations.insert(
            (*route).to_string(),
            json!({ "markers": markers, "count": markers.len(), "featureShim": feature_shim }),
        );
    }
    for marker in ["Counter", "NamedCounter"] {
        assert_eq!(
            extract_html_markers(&html)
                .iter()
                .filter(|actual| actual.as_str() == marker)
                .count(),
            1,
            "the collection MDX map must render one {marker} island"
        );
    }
    for marker in ["Counter", "NamedCounter"] {
        let marker_attribute = format!("data-zfb-island=\"{marker}\"");
        let attributes = html
            .split_once(&marker_attribute)
            .and_then(|(_, rest)| rest.split_once('>').map(|(attributes, _)| attributes))
            .unwrap_or_else(|| panic!("missing SSR opening tag for {marker}: {html}"));
        assert!(
            attributes.contains("data-props=\"{}\""),
            "MDX {marker} island must serialize only its empty JSON props: {attributes}"
        );
    }
    assert!(
        !html.contains("readState"),
        "unused duplicate helper exports must not become SSR island markers"
    );

    let ssr_bundle = fs::read_to_string(positive.join(".zfb-build/bundle.mjs"))
        .expect("read production SSR bundle");
    let ssr_names = extract_ssr_allowlist(&ssr_bundle)
        .expect("extract static SSR island allowlist from real bundle");
    assert_eq!(
        ssr_names.len(),
        expected_marker_set().len(),
        "SSR allowlist must contain each marker exactly once"
    );
    let ssr_name_set: BTreeSet<_> = ssr_names.into_iter().collect();
    assert_eq!(
        ssr_name_set,
        expected_marker_set(),
        "SSR allowlist must be exactly the concrete target marker set"
    );

    let assets = positive.join("dist/assets");
    let browser_bundle = fs::read_dir(&assets)
        .expect("read built assets")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("islands-") && name.ends_with(".js"))
        })
        .expect("production build emits the shared islands bundle");
    let browser_source = fs::read_to_string(&browser_bundle).expect("read minified islands bundle");
    let reactive_runtime_definitions = browser_source
        .matches(r#"@takazudo/zfb/zudo-react/runtime-definition-v1":!0"#)
        .count();
    assert_eq!(
        reactive_runtime_definitions, 1,
        "minified client bundle must have one zudo-react reactive core; duplicate runtime definitions break packed-island subscriptions"
    );
    let build_tokens: BTreeSet<_> = html
        .split("data-zfb-build=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(token, _)| token.to_string()))
        .collect();
    assert_eq!(
        build_tokens.len(),
        1,
        "all SSR wrappers share one build token"
    );
    let build_token = build_tokens.iter().next().expect("build token");
    assert_eq!(
        build_token.len(),
        16,
        "production token keeps the public width"
    );
    assert!(
        build_token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "production token remains lowercase hexadecimal: {build_token}"
    );
    assert!(
        browser_source.contains(build_token),
        "minified browser registry must carry the same SSR build identity"
    );
    for marker in EXPECTED_MARKERS {
        assert!(
            browser_source.contains(marker),
            "minified shared bundle is missing marker {marker:?}"
        );
    }

    // Preserve the exact real-build output for the independent Playwright
    // guard. No generated bundle or tarball is checked into the repository.
    let artifacts = workspace.join("target/scanner-boundary-acceptance");
    if artifacts.exists() {
        fs::remove_dir_all(&artifacts).expect("clear previous local browser artifact");
    }
    fs::create_dir_all(&artifacts).expect("create browser artifact directory");
    copy_dir(&positive.join("dist"), &artifacts.join("dist"));
    let metadata = json!({
        "markers": EXPECTED_MARKERS,
        "pageMarkers": expected_home_marker_set(),
        "wrapperCount": extract_html_markers(&html).len(),
        "routes": route_expectations,
        "buildToken": build_token,
    });
    fs::write(
        artifacts.join("acceptance.json"),
        serde_json::to_vec_pretty(&metadata).expect("serialize acceptance metadata"),
    )
    .expect("write browser acceptance metadata");

    // A reachable helper-only client module and a route with no boundary
    // targets are a valid empty registry. Preserve unrelated client-script
    // output while proving helpers do not appear in SSR metadata or assets.
    let helper_only = scratch.path().join("helper-only-zero-boundary");
    make_minimal_project(&helper_only);
    write(
        &helper_only,
        "components/helper-only.tsx",
        r#""use client";
export function readState() { return "reachable helper without a boundary"; }
"#,
    );
    write(
        &helper_only,
        "pages/index.tsx",
        r#"import { readState } from "../components/helper-only";
export default function Home() {
  return <html><body><p>{readState()}</p></body></html>;
}
"#,
    );
    write(
        &helper_only,
        "pages/keep.client.ts",
        "console.log(\"ZERO_BOUNDARY_CLIENT_SCRIPT\");\n",
    );
    let built = run_build(&helper_only, &esbuild);
    assert!(
        built.status.success(),
        "reachable helper-only client module must be accepted without island targets\n{}",
        combined_output(&built)
    );
    let helper_html = fs::read_to_string(helper_only.join("dist/index.html"))
        .expect("read helper-only route HTML");
    assert!(
        helper_html.contains("reachable helper without a boundary"),
        "{helper_html}"
    );
    assert!(
        extract_html_markers(&helper_html).is_empty(),
        "zero-boundary route must not emit island markers: {helper_html}"
    );
    let helper_ssr = fs::read_to_string(helper_only.join(".zfb-build/bundle.mjs"))
        .expect("read helper-only SSR bundle");
    assert_eq!(
        extract_ssr_allowlist(&helper_ssr).expect("empty SSR island allowlist"),
        Vec::<String>::new(),
        "helper names must not leak into the zero-boundary SSR allowlist"
    );
    assert_no_published_islands(&helper_only);
    // Production ships client scripts only under their content-hashed name.
    let client_dir = helper_only.join("dist/assets/client");
    let client_scripts: Vec<PathBuf> = fs::read_dir(&client_dir)
        .unwrap_or_else(|error| panic!("read client script dir {}: {error}", client_dir.display()))
        .map(|entry| entry.expect("client script dir entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("keep-") && name.ends_with(".js"))
        })
        .collect();
    assert_eq!(
        client_scripts.len(),
        1,
        "island-free build must ship exactly one hashed keep client script: {client_scripts:?}"
    );
    let client_script_source =
        fs::read_to_string(&client_scripts[0]).expect("read preserved client script");
    assert!(
        client_script_source.contains("ZERO_BOUNDARY_CLIENT_SCRIPT"),
        "island-free builds must preserve independent client-script output"
    );

    // A single source file declares two distinct function expressions with
    // the same actual runtime name. They must collide before browser emission.
    let local_collision = scratch.path().join("local-collision");
    make_minimal_project(&local_collision);
    write(
        &local_collision,
        "components/twins.tsx",
        r#""use client";
export const First = function SameFileTwin() { return <button>first</button>; };
export const Second = function SameFileTwin() { return <button>second</button>; };
"#,
    );
    write(
        &local_collision,
        "pages/index.tsx",
        r#"import { Island } from "@takazudo/zfb";
import { First, Second } from "../components/twins";
export default function Home() {
  return <html><body><Island><First /></Island><Island><Second /></Island></body></html>;
}
"#,
    );
    let rejected = run_build(&local_collision, &esbuild);
    assert!(
        !rejected.status.success(),
        "two local same-name targets must fail the production build\n{}",
        combined_output(&rejected)
    );
    let diagnostic = combined_output(&rejected);
    assert!(
        diagnostic.contains("ambiguous owned island marker"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("SameFileTwin"), "{diagnostic}");
    assert!(
        diagnostic.contains("twins.tsx"),
        "both local definitions must be located: {diagnostic}"
    );
    assert!(
        diagnostic.contains("pages/index.tsx"),
        "both boundary sites must be located: {diagnostic}"
    );
    assert_no_published_islands(&local_collision);

    // The same real collision rule applies inside one installed package. This
    // also records the npm tarball as the package provenance for the negative.
    let package_collision = scratch.path().join("packed-collision");
    copy_dir(&fixture_root(), &package_collision);
    let archive = install_packed_widgets(&package_collision);
    materialize_embedded_node_modules(&package_collision);
    write(
        &package_collision,
        "pages/index.tsx",
        r#"import { Island } from "@takazudo/zfb";
import { PackedTwin as First } from "@fixture/widgets/conflict-one";
import { PackedTwin as Second } from "@fixture/widgets/conflict-two";
export default function Home() {
  return <html><body><Island><First /></Island><Island><Second /></Island></body></html>;
}
"#,
    );
    let rejected = run_build(&package_collision, &esbuild);
    assert!(
        !rejected.status.success(),
        "same-package same-name targets must fail despite tarball ownership\n{}",
        combined_output(&rejected)
    );
    let diagnostic = combined_output(&rejected);
    assert!(
        diagnostic.contains("ambiguous owned island marker"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("PackedTwin"), "{diagnostic}");
    assert!(
        diagnostic.contains("conflicts/one.js"),
        "first packaged definition missing: {diagnostic}"
    );
    assert!(
        diagnostic.contains("conflicts/two.js"),
        "second packaged definition missing: {diagnostic}"
    );
    assert!(
        diagnostic.contains("pages/index.tsx"),
        "packed boundary sites must be located: {diagnostic}"
    );
    assert_no_published_islands(&package_collision);
    eprintln!(
        "[scanner_boundary_acceptance] same-package collision rejected from {}",
        archive.display()
    );

    // A demanded name provided by two different `export *` branches is
    // ambiguous even when neither provider is a client entry on its own.
    let ambiguous_star = scratch.path().join("ambiguous-packed-star");
    copy_dir(&fixture_root(), &ambiguous_star);
    let archive = install_packed_widgets(&ambiguous_star);
    materialize_embedded_node_modules(&ambiguous_star);
    write(
        &ambiguous_star,
        "pages/index.tsx",
        r#"import { Island } from "@takazudo/zfb";
import { StarChoice } from "@fixture/widgets/ambiguous";
export default function Home() {
  return <html><body><Island><StarChoice /></Island></body></html>;
}
"#,
    );
    let rejected = run_build(&ambiguous_star, &esbuild);
    assert!(
        !rejected.status.success(),
        "demanded ambiguous star export must fail production build\n{}",
        combined_output(&rejected)
    );
    let diagnostic = combined_output(&rejected);
    assert!(diagnostic.contains("ambiguous export *"), "{diagnostic}");
    assert!(diagnostic.contains("StarChoice"), "{diagnostic}");
    assert!(diagnostic.contains("pages/index.tsx"), "{diagnostic}");
    assert_no_published_islands(&ambiguous_star);
    eprintln!(
        "[scanner_boundary_acceptance] demanded star conflict rejected from {}",
        archive.display()
    );

    // Identical source and compiled copies are distinct definitions without
    // trusted source/shadow provenance; byte equality cannot suppress a real
    // target collision.
    let source_dist = scratch.path().join("source-dist-collision");
    make_minimal_project(&source_dist);
    let duplicate_definition =
        "\"use client\";\nexport function SourceDistTwin() { return null; }\n";
    write(&source_dist, "components/source.tsx", duplicate_definition);
    write(&source_dist, "compiled/source.js", duplicate_definition);
    write(
        &source_dist,
        "pages/index.tsx",
        r#"import { Island } from "@takazudo/zfb";
import { SourceDistTwin as SourceCopy } from "../components/source";
import { SourceDistTwin as BuiltCopy } from "../compiled/source.js";
export default function Home() {
  return <html><body><Island><SourceCopy /></Island><Island><BuiltCopy /></Island></body></html>;
}
"#,
    );
    let rejected = run_build(&source_dist, &esbuild);
    assert!(
        !rejected.status.success(),
        "independent source and compiled copies must not deduplicate by bytes\n{}",
        combined_output(&rejected)
    );
    let diagnostic = combined_output(&rejected);
    assert!(
        diagnostic.contains("ambiguous owned island marker"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("SourceDistTwin"), "{diagnostic}");
    assert!(diagnostic.contains("components/source.tsx"), "{diagnostic}");
    assert!(diagnostic.contains("compiled/source.js"), "{diagnostic}");
    assert_no_published_islands(&source_dist);

    // A known conflicting literal displayName must stay fail-closed in the
    // real SSR renderer, even if the target has only one scanner definition.
    let conflicting_display_name = scratch.path().join("conflicting-display-name");
    make_minimal_project(&conflicting_display_name);
    write(
        &conflicting_display_name,
        "components/wrong-name.tsx",
        r#""use client";
export function ActualFunctionName() { return null; }
ActualFunctionName.displayName = "PretendName";
"#,
    );
    write(
        &conflicting_display_name,
        "pages/index.tsx",
        r#"import { Island } from "@takazudo/zfb";
import { ActualFunctionName } from "../components/wrong-name";
export default function Home() {
  return <html><body><Island><ActualFunctionName /></Island></body></html>;
}
"#,
    );
    let rejected = run_build(&conflicting_display_name, &esbuild);
    assert!(
        !rejected.status.success(),
        "a conflicting displayName must fail the real production build\n{}",
        combined_output(&rejected)
    );
    let diagnostic = combined_output(&rejected);
    assert!(diagnostic.contains("displayName"), "{diagnostic}");
    assert!(diagnostic.contains("PretendName"), "{diagnostic}");
    assert_no_published_islands(&conflicting_display_name);

    // The originally documented shape stores a direct Island-returning
    // wrapper in an object. Keep this opaque-container rejection while the
    // supported fragment recipe is covered by the positive collection fixture.
    let opaque_map_wrapper = scratch.path().join("opaque-map-wrapper");
    make_minimal_project(&opaque_map_wrapper);
    write(
        &opaque_map_wrapper,
        "zfb.config.json",
        r#"{"wind":false,"collections":[{"name":"content","path":"content"}]}"#,
    );
    write(
        &opaque_map_wrapper,
        "components/counter.tsx",
        r#""use client";
import { signal } from "@takazudo/zfb/zudo-react";
export default function Counter() {
  const count = signal(0);
  return <button on:click={() => { count.value += 1; }}>Count: {count}</button>;
}
"#,
    );
    write(
        &opaque_map_wrapper,
        "pages/_mdx-components.tsx",
        r#"import { Island } from "@takazudo/zfb";
import Counter from "../components/counter";
function WrappedCounter() {
  return <Island when="load"><Counter /></Island>;
}
export const components = { Counter: WrappedCounter };
"#,
    );
    write(
        &opaque_map_wrapper,
        "content/index.mdx",
        "---\ntitle: Opaque map wrapper\n---\n\n<Counter />\n",
    );
    write(
        &opaque_map_wrapper,
        "pages/index.tsx",
        r#"import { getCollection } from "@takazudo/zfb/content";
import { components } from "./_mdx-components";
export default function Home() {
  const entry = getCollection("content").find((candidate) => candidate.slug === "index");
  return <html><body>{entry ? <entry.Content components={components} /> : null}</body></html>;
}
"#,
    );
    let rejected = run_build(&opaque_map_wrapper, &esbuild);
    assert!(
        !rejected.status.success(),
        "a direct Island-returning wrapper in a components object must remain rejected\n{}",
        combined_output(&rejected)
    );
    let diagnostic = combined_output(&rejected);
    assert!(
        diagnostic.contains("unsupported island registration"),
        "the opaque map wrapper needs the existing named scanner diagnostic: {diagnostic}"
    );
    assert!(
        diagnostic.contains("boundary wrapper escapes into an opaque container"),
        "the opaque map wrapper diagnostic changed: {diagnostic}"
    );
    assert!(
        diagnostic.contains("pages/_mdx-components.tsx"),
        "the opaque map wrapper diagnostic must locate its source: {diagnostic}"
    );
    assert_no_published_islands(&opaque_map_wrapper);

    // Unsupported dynamic child selection is an actionable production error,
    // never a warning that publishes a successful empty registry.
    let dynamic = scratch.path().join("dynamic-child");
    make_minimal_project(&dynamic);
    write(
        &dynamic,
        "components/targets.tsx",
        r#""use client";
export function FirstTarget() { return <button>first</button>; }
export function SecondTarget() { return <button>second</button>; }
"#,
    );
    write(
        &dynamic,
        "pages/index.tsx",
        r#"import { Island } from "@takazudo/zfb";
import { FirstTarget, SecondTarget } from "../components/targets";
const chooseFirst = true;
export default function Home() {
  return <html><body><Island>{chooseFirst ? <FirstTarget /> : <SecondTarget />}</Island></body></html>;
}
"#,
    );
    let rejected = run_build(&dynamic, &esbuild);
    assert!(
        !rejected.status.success(),
        "dynamic child selection must fail the production build\n{}",
        combined_output(&rejected)
    );
    let diagnostic = combined_output(&rejected);
    assert!(
        diagnostic.contains("unsupported island registration"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("pages/index.tsx"),
        "diagnostic must locate the source site: {diagnostic}"
    );
    assert!(
        diagnostic.contains("separate boundaries") || diagnostic.contains("directly"),
        "diagnostic must state a static rewrite: {diagnostic}"
    );
    assert_no_published_islands(&dynamic);

    // The bounded factory contract reports the exact unsupported shape and
    // never leaves a partial islands registry behind.
    for (name, call, reason) in [
        (
            "factory-spread",
            "const FactoryBoundary = createFactoryBoundary({ ...defaults, FactoryCounter });",
            "passes a non-literal argument",
        ),
        (
            "factory-nullish",
            "const FactoryBoundary = createFactoryBoundary({ FactoryCounter: FactoryCounter ?? undefined });",
            "selects FactoryCounter conditionally (??, ternary, ||)",
        ),
        (
            "factory-escape",
            "const FactoryBoundary = createFactoryBoundary({ FactoryCounter });\n  retainFactory(createFactoryBoundary);",
            "factory createFactoryBoundary escapes as a value",
        ),
        (
            "factory-spread-mutated",
            "const defaults = { FactoryCounter }; defaults.FactoryCounter = FactoryCounter; const FactoryBoundary = createFactoryBoundary({ ...defaults });",
            "passes a non-literal argument",
        ),
        (
            "factory-spread-nested",
            "const inner = { FactoryCounter }; const defaults = { ...inner }; const FactoryBoundary = createFactoryBoundary({ ...defaults });",
            "passes a non-literal argument",
        ),
        (
            "factory-spread-proto",
            "const defaults = { '__proto__': {}, FactoryCounter }; const FactoryBoundary = createFactoryBoundary({ ...defaults });",
            "passes a non-literal argument",
        ),
        (
            "factory-spread-duplicate-argument",
            "const FactoryBoundary = createFactoryBoundary({ FactoryCounter, FactoryCounter: Other });",
            "passes a non-literal argument",
        ),
        (
            "factory-spread-duplicate-source",
            "const defaults = { FactoryCounter, FactoryCounter: Other }; const FactoryBoundary = createFactoryBoundary({ ...defaults });",
            "passes a non-literal argument",
        ),
        (
            "factory-spread-missing",
            "const defaults = { Other }; const FactoryBoundary = createFactoryBoundary({ ...defaults });",
            "property FactoryCounter missing at call site",
        ),
    ] {
        let rejected_root = scratch.path().join(name);
        make_minimal_project(&rejected_root);
        write(
            &rejected_root,
            "components/factory-counter.tsx",
            r#""use client";
export function FactoryCounter() { return <button>Factory counter</button>; }
export function Other() { return <button>Other</button>; }
"#,
        );
        write(
            &rejected_root,
            "pages/index.tsx",
            &format!(
                r#"import {{ Island }} from "@takazudo/zfb";
import {{ FactoryCounter, Other }} from "../components/factory-counter";
function createFactoryBoundary(deps: {{ FactoryCounter: typeof FactoryCounter }}) {{
  const Target = deps.FactoryCounter;
  return function FactoryBoundary() {{
    return <><Island><Target /></Island></>;
  }};
}}
function retainFactory(value: unknown) {{ return value; }}
{call}
export default function Home() {{
  return <html><body><FactoryBoundary /></body></html>;
}}
"#
            ),
        );
        let rejected = run_build(&rejected_root, &esbuild);
        assert!(
            !rejected.status.success(),
            "{name} must fail the production build\n{}",
            combined_output(&rejected)
        );
        let diagnostic = combined_output(&rejected);
        assert!(
            diagnostic.contains(reason),
            "{name} must report {reason:?}: {diagnostic}"
        );
        if name != "factory-escape" {
            assert!(
                diagnostic.contains("target FactoryBoundary has unsupported initializer:"),
                "{name} must keep the source-located outer diagnostic: {diagnostic}"
            );
            assert!(
                diagnostic.contains("pages/index.tsx")
                    && diagnostic.contains("; see concepts/islands#boundary-discovery-and-migration"),
                "{name} must retain the source location and migration anchor: {diagnostic}"
            );
        }
        assert_no_published_islands(&rejected_root);
    }

    for (name, map, reason) in [
        (
            "factory-map-opaque",
            "const components = { Panel: BarePanelIsland };",
            "boundary wrapper escapes into an opaque container",
        ),
        (
            "factory-map-forwarding-opaque",
            "const components = { Island: ForwardBoundary };",
            "boundary wrapper escapes into an opaque container",
        ),
        (
            "factory-map-opaque-shorthand",
            "const components = { ForwardBoundary };",
            "boundary wrapper escapes into an opaque object",
        ),
    ] {
        let rejected_root = scratch.path().join(name);
        make_minimal_project(&rejected_root);
        write(
            &rejected_root,
            "components/factory-counter.tsx",
            "\"use client\"; export function FactoryCounter() { return <button>Counter</button>; }",
        );
        write(
            &rejected_root,
            "pages/index.tsx",
            &format!(
                r#"import {{ Island }} from "@takazudo/zfb";
import {{ FactoryCounter }} from "../components/factory-counter";
function BarePanelIsland() {{ return <Island><FactoryCounter /></Island>; }}
function ForwardBoundary({{ children }}) {{ return <Island>{{children}}</Island>; }}
{map}
export default function Home() {{ return <html><body><p>map</p></body></html>; }}
"#
            ),
        );
        let rejected = run_build(&rejected_root, &esbuild);
        assert!(!rejected.status.success(), "{name} must fail");
        assert!(
            combined_output(&rejected).contains(reason),
            "{name}: {}",
            combined_output(&rejected)
        );
        assert_no_published_islands(&rejected_root);
    }

    let json_function = scratch.path().join("factory-function-json-prop");
    make_minimal_project(&json_function);
    write(
        &json_function,
        "components/client.tsx",
        "\"use client\"; export function FactoryCounter() { return <button>Counter</button>; } export function PropsReceiver({ component }) { return <button>Receiver</button>; }",
    );
    write(
        &json_function,
        "pages/index.tsx",
        r#"import { Island } from "@takazudo/zfb";
import { FactoryCounter, PropsReceiver } from "../components/client";
export default function Home() { return <html><body><Island><PropsReceiver component={FactoryCounter} /></Island></body></html>; }
"#,
    );
    let rejected = run_build(&json_function, &esbuild);
    assert!(
        !rejected.status.success(),
        "function-valued JSON prop must fail SSR"
    );
    let diagnostic = combined_output(&rejected);
    assert!(
        diagnostic.contains("ZR_ISLAND_PROPS PropsReceiver: ZR_PROPS_FUNCTION at props.component"),
        "{diagnostic}"
    );
    assert!(
        !json_function.join("dist/index.html").exists(),
        "failed SSR must not publish a page"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dev_invalid_registration_rebuild_preserves_registry_and_recovers() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let esbuild = locate_esbuild().expect(
        "scanner dev acceptance requires the pinned esbuild binary; stage it in the test environment",
    );
    let project = tempfile::tempdir().expect("dev acceptance fixture");
    let root = project
        .path()
        .canonicalize()
        .expect("canonicalize dev project root");
    make_minimal_project(&root);
    write(
        &root,
        "components/stable.tsx",
        r#""use client";
export function StableCounter() { return <button>stable counter</button>; }
"#,
    );
    write(
        &root,
        "components/duplicate-a.tsx",
        r#""use client";
export function DevTwin() { return <button>first conflicting target</button>; }
"#,
    );
    write(
        &root,
        "components/duplicate-b.tsx",
        r#""use client";
export function DevTwin() { return <button>second conflicting target</button>; }
"#,
    );
    write(
        &root,
        "pages/index.tsx",
        r#"import { Island } from "@takazudo/zfb";
import { StableCounter } from "../components/stable";
export default function Home() {
  return <html><body><Island><StableCounter /></Island><p>valid initial graph</p></body></html>;
}
"#,
    );

    let mut session = spawn_dev(&root, &esbuild);
    let port = wait_for_dev_ready(&mut session).await;
    let initial_html = wait_for_dev_page_marker(&mut session, port, "StableCounter").await;
    assert!(
        !session.logs().contains("cold-lazy"),
        "dev fixture must exercise eager rebuilds, not cold-lazy route deferral\n{}",
        session.logs()
    );
    let initial_markers: BTreeSet<_> = extract_html_markers(&initial_html).into_iter().collect();
    assert_eq!(
        initial_markers,
        BTreeSet::from(["StableCounter".to_string()])
    );

    let asset_path = root.join(".zfb-build/dev-assets/assets/islands.js");
    let initial_asset = fs::read(&asset_path).unwrap_or_else(|error| {
        panic!(
            "read initial dev islands asset {}: {error}",
            asset_path.display()
        )
    });
    let initial_asset_source =
        String::from_utf8(initial_asset.clone()).expect("dev asset is UTF-8");
    assert!(
        initial_asset_source.contains("StableCounter"),
        "{initial_asset_source}"
    );
    assert!(
        !initial_asset_source.contains("DevTwin"),
        "{initial_asset_source}"
    );

    // The route becomes ambiguous only when both same-name functions are
    // demanded as boundary children. A failed tick must be visible while the
    // prior complete client registry remains on disk and served.
    let before_invalid_stdout = session.stdout();
    let before_invalid_stderr = session.stderr();
    write(
        &root,
        "pages/index.tsx",
        r#"import { Island } from "@takazudo/zfb";
import { DevTwin as First } from "../components/duplicate-a";
import { DevTwin as Second } from "../components/duplicate-b";
export default function Home() {
  return <html><body><Island><First /></Island><Island><Second /></Island></body></html>;
}
"#,
    );
    let failure_logs = wait_for_dev_log(
        &mut session,
        &before_invalid_stdout,
        &before_invalid_stderr,
        "ambiguous owned island marker",
    )
    .await;
    assert!(failure_logs.contains("DevTwin"), "{failure_logs}");
    assert!(failure_logs.contains("duplicate-a.tsx"), "{failure_logs}");
    assert!(failure_logs.contains("duplicate-b.tsx"), "{failure_logs}");
    assert!(
        session.guard.try_exit_status().is_none(),
        "dev server must stay alive after a failed island rebuild\n{}",
        session.logs()
    );
    let failed_tick_page = reqwest::get(format!("http://127.0.0.1:{port}/"))
        .await
        .expect("dev server remains reachable after the failed rebuild");
    assert!(
        failed_tick_page.status().is_success(),
        "last successful SSR renderer remains available after the failed tick: {}\n{}",
        failed_tick_page.status(),
        session.logs()
    );
    assert_eq!(
        fs::read(&asset_path).expect("read dev islands asset after failed rebuild"),
        initial_asset,
        "failed registration must not partially replace the last complete registry"
    );
    let served_after_failure = reqwest::get(format!("http://127.0.0.1:{port}/assets/islands.js"))
        .await
        .expect("last successful islands asset stays served");
    assert!(served_after_failure.status().is_success());
    let served_after_failure_bytes = served_after_failure
        .bytes()
        .await
        .expect("read served islands asset")
        .to_vec();
    assert_eq!(
        served_after_failure_bytes, initial_asset,
        "the browser must continue receiving the last complete registry"
    );

    // Fix the source and require a successful publication whose SSR markers
    // and client asset again describe the same single target.
    let before_recovery_stdout = session.stdout();
    let before_recovery_stderr = session.stderr();
    write(
        &root,
        "pages/index.tsx",
        r#"import { Island } from "@takazudo/zfb";
import { StableCounter } from "../components/stable";
export default function Home() {
  return <html><body><Island><StableCounter /></Island><p>recovered valid graph</p></body></html>;
}
"#,
    );
    let recovery_logs = wait_for_dev_log(
        &mut session,
        &before_recovery_stdout,
        &before_recovery_stderr,
        "[zfb-timing] tick: islands published",
    )
    .await;
    assert!(
        !recovery_logs.contains("ambiguous owned island marker"),
        "corrected dev rebuild should succeed: {recovery_logs}"
    );
    let recovered_html =
        wait_for_dev_page_marker(&mut session, port, "recovered valid graph").await;
    let recovered_markers: BTreeSet<_> =
        extract_html_markers(&recovered_html).into_iter().collect();
    assert_eq!(
        recovered_markers,
        BTreeSet::from(["StableCounter".to_string()]),
        "recovery must recompute the same exact target set"
    );
    let recovered_asset =
        fs::read_to_string(&asset_path).expect("read recovered dev islands asset");
    assert!(
        recovered_asset.contains("StableCounter"),
        "{recovered_asset}"
    );
    assert!(!recovered_asset.contains("DevTwin"), "{recovered_asset}");
}
