//! Level-3 build-output and Level-4 real-process confirmation for issue #3435.
//!
//! The contract compares complete `dist/` inventories and bytes from real
//! `zfb build` processes. Each successful build also ties the prerendered HTML
//! identity to the islands entry and Cloudflare SSR bundle, then scans all
//! emitted files for machine-local paths. The pnpm-shaped dependency and raw
//! island import force the SSR realpath escape and islands-shadow remap paths.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use zfb_test_utils::{locate_esbuild, zfb_binary};

const ISLAND_LABEL: &str = "components/counter.tsx";
const REALPATH_EXPORT: &str = "REALPATH_ESCAPE_EXPORT_SURVIVES";
const ESCAPE_PACKAGE: &str = "repro-realpath-dep";

static BUILD_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug)]
struct BuildSnapshot {
    dist: BTreeMap<PathBuf, Vec<u8>>,
    build_id: String,
    islands_entry: String,
    inner_module: String,
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("bundle-repro")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/zfb must live two levels under the workspace root")
        .to_path_buf()
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn pnpm_available() -> bool {
    Command::new("pnpm")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn toolchain() -> Option<PathBuf> {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[build_reproducibility_e2e] no esbuild binary available; skipping");
        return None;
    };
    if !node_available() {
        eprintln!("[build_reproducibility_e2e] node is unavailable; skipping");
        return None;
    }
    if !pnpm_available() {
        eprintln!("[build_reproducibility_e2e] pnpm is unavailable; skipping");
        return None;
    }
    Some(esbuild)
}

fn copy_tree_preserving_symlinks(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let kind = fs::symlink_metadata(&source_path)?.file_type();
        if kind.is_symlink() {
            let link_target = fs::read_link(&source_path)?;
            std::os::unix::fs::symlink(link_target, target_path)?;
        } else if kind.is_dir() {
            copy_tree_preserving_symlinks(&source_path, &target_path)?;
        } else if kind.is_file() {
            fs::copy(source_path, target_path)?;
        }
    }
    Ok(())
}

fn copy_fixture(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let kind = fs::symlink_metadata(&source_path)?.file_type();
        if kind.is_symlink() {
            let link_target = fs::read_link(&source_path)?;
            std::os::unix::fs::symlink(link_target, target_path)?;
        } else if kind.is_dir() {
            copy_fixture(&source_path, &target_path)?;
        } else if kind.is_file() {
            fs::copy(source_path, target_path)?;
        }
    }
    Ok(())
}

fn scaffold_project(root: &Path) {
    copy_fixture(&fixture_dir(), root).expect("copy bundle reproducibility fixture");

    // Each root gets its own physical vendor tree. Copying symlinks as
    // symlinks preserves pnpm's resolution shape while ensuring relative
    // package links resolve under this root rather than a shared extraction.
    let (_embedded_handle, embedded_node_modules) =
        zfb::render_pipeline::embedded_node_modules().expect("extract embedded node_modules");
    let node_modules = root.join("node_modules");
    copy_tree_preserving_symlinks(&embedded_node_modules, &node_modules)
        .expect("copy embedded node_modules preserving symlinks");

    let adapter = workspace_root().join("packages/zfb-adapter-cloudflare");
    assert!(
        adapter.join("package.json").is_file() && adapter.join("bin/cli.mjs").is_file(),
        "workspace Cloudflare adapter package must exist at {}",
        adapter.display()
    );
    let scope = node_modules.join("@takazudo");
    fs::create_dir_all(&scope).expect("create @takazudo package scope");
    let adapter_link = scope.join("zfb-adapter-cloudflare");
    if fs::symlink_metadata(&adapter_link).is_ok() {
        fs::remove_file(&adapter_link).expect("remove existing adapter link");
    }
    std::os::unix::fs::symlink(&adapter, &adapter_link).expect("link workspace adapter package");
    let bin_dir = node_modules.join(".bin");
    fs::create_dir_all(&bin_dir).expect("create local node_modules bin directory");
    std::os::unix::fs::symlink(
        "../@takazudo/zfb-adapter-cloudflare/bin/cli.mjs",
        bin_dir.join("zfb-adapter-cloudflare"),
    )
    .expect("link local adapter executable");

    // Model pnpm's logical package symlink into a package store under the
    // fixture's own node_modules. The target is outside each transient build
    // shadow, so esbuild emits a traversal-style module comment before #3434.
    let store_package = node_modules
        .join(".pnpm/repro-realpath-dep@1.0.0/node_modules")
        .join(ESCAPE_PACKAGE);
    fs::create_dir_all(&store_package).expect("create local pnpm store package");
    let package_json = serde_json::json!({
        "name": ESCAPE_PACKAGE,
        "version": "1.0.0",
        "type": "module",
        "main": "index.js",
    });
    fs::write(
        store_package.join("package.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&package_json)
                .expect("serialize pnpm store package manifest")
        ),
    )
    .expect("write pnpm store package manifest");
    fs::write(
        store_package.join("index.js"),
        format!("export const marker = {REALPATH_EXPORT:?};\n"),
    )
    .expect("write pnpm store package entry");
    std::os::unix::fs::symlink(
        ".pnpm/repro-realpath-dep@1.0.0/node_modules/repro-realpath-dep",
        node_modules.join(ESCAPE_PACKAGE),
    )
    .expect("link logical dependency to pnpm store");

    assert!(node_modules.is_dir());
    assert!(!fs::symlink_metadata(&node_modules)
        .expect("node_modules metadata")
        .file_type()
        .is_symlink());
    let logical_package = fs::canonicalize(node_modules.join(ESCAPE_PACKAGE))
        .expect("resolve pnpm-style dependency symlink");
    assert_eq!(logical_package, fs::canonicalize(store_package).unwrap());

    let adapter_preflight = Command::new("pnpm")
        .args(["exec", "zfb-adapter-cloudflare", "--help"])
        .current_dir(root)
        .output()
        .expect("spawn pnpm adapter CLI preflight");
    assert!(
        adapter_preflight.status.success(),
        "fixture must resolve its declared Cloudflare adapter through pnpm exec\nstatus: {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        adapter_preflight.status,
        String::from_utf8_lossy(&adapter_preflight.stdout),
        String::from_utf8_lossy(&adapter_preflight.stderr),
    );
}

fn run_build(root: &Path, esbuild: &Path, scratch_dir: Option<&Path>, define_value: Option<&str>) {
    let mut command = Command::new(zfb_binary!());
    command
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild);
    if let Some(scratch_dir) = scratch_dir {
        command.arg("--scratch-dir").arg(scratch_dir);
    }
    if let Some(value) = define_value {
        command.arg("--define").arg(format!(r#"__X__="{value}""#));
    }
    let output = command.output().expect("spawn real `zfb build`");
    assert!(
        output.status.success(),
        "`zfb build` must succeed for {}\nstatus: {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        root.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn read_dist_inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in
            fs::read_dir(dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        {
            let entry = entry.expect("read dist entry");
            let path = entry.path();
            let kind = fs::symlink_metadata(&path)
                .unwrap_or_else(|error| panic!("metadata {}: {error}", path.display()))
                .file_type();
            if kind.is_dir() {
                visit(root, &path, files);
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .expect("dist file must stay under dist")
                    .to_path_buf();
                let bytes = fs::read(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
                files.insert(relative, bytes);
            } else {
                panic!("dist contains a non-regular entry: {}", path.display());
            }
        }
    }

    let dist_root = root.join("dist");
    assert!(
        dist_root.is_dir(),
        "zfb build did not emit {}",
        dist_root.display()
    );
    let mut files = BTreeMap::new();
    visit(&dist_root, &dist_root, &mut files);
    files
}

fn quoted_after<'a>(source: &'a str, marker: &str) -> &'a str {
    source
        .split_once(marker)
        .unwrap_or_else(|| panic!("{marker:?} not found in output"))
        .1
        .split('"')
        .next()
        .expect("quoted value after marker")
}

fn html_islands_entry(html: &str) -> PathBuf {
    let entry = html
        .split("src=\"")
        .filter_map(|tail| tail.split('"').next())
        .find(|src| src.starts_with("/assets/islands") && src.ends_with(".js"))
        .unwrap_or_else(|| panic!("prerendered HTML has no islands entry:\n{html}"));
    PathBuf::from(entry.trim_start_matches('/'))
}

fn string_assignment(source: &str, name: &str) -> String {
    let compact: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();
    let assignment = compact
        .split_once(&format!("{name}="))
        .unwrap_or_else(|| panic!("{name} assignment not found in emitted SSR bundle"))
        .1;
    let literal = assignment
        .split(';')
        .next()
        .expect("assignment has a value");
    serde_json::from_str(literal)
        .unwrap_or_else(|error| panic!("parse {name} JSON string {literal:?}: {error}"))
}

fn capture_build(
    root: &Path,
    esbuild: &Path,
    scratch_dir: Option<&Path>,
    define_value: Option<&str>,
) -> BuildSnapshot {
    run_build(root, esbuild, scratch_dir, define_value);
    let dist = read_dist_inventory(root);
    let html_bytes = dist
        .get(Path::new("index.html"))
        .expect("fixture build must emit dist/index.html");
    let html = String::from_utf8(html_bytes.clone()).expect("prerendered HTML is UTF-8");
    let build_id = quoted_after(&html, "data-zfb-build=\"").to_owned();
    let entry_path = html_islands_entry(&html);
    let islands_entry = String::from_utf8(
        dist.get(&entry_path)
            .unwrap_or_else(|| {
                panic!(
                    "HTML islands asset missing from dist: {}",
                    entry_path.display()
                )
            })
            .clone(),
    )
    .expect("islands entry is UTF-8");
    let inner_module = String::from_utf8(
        dist.get(Path::new("_zfb_inner.mjs"))
            .expect("Cloudflare adapter build must emit _zfb_inner.mjs")
            .clone(),
    )
    .expect("SSR inner module is UTF-8");

    let islands_compact: String = islands_entry
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    assert!(
        islands_compact.contains(&format!("build:\"{build_id}\"")),
        "islands entry identity must equal HTML data-zfb-build={build_id}\n{}",
        islands_entry
    );
    let inner_build_id = string_assignment(&inner_module, "zudoReactBuild");
    assert_eq!(
        inner_build_id, build_id,
        "HTML and _zfb_inner.mjs must share the same owned build identity"
    );

    assert!(
        inner_module.contains(REALPATH_EXPORT),
        "SSR route must consume the pnpm-style realpath dependency export"
    );
    BuildSnapshot {
        dist,
        build_id,
        islands_entry,
        inner_module,
    }
}

fn assert_dist_equal(left: &BuildSnapshot, right: &BuildSnapshot, context: &str) {
    let all_paths: std::collections::BTreeSet<_> =
        left.dist.keys().chain(right.dist.keys()).cloned().collect();
    let differing: Vec<_> = all_paths
        .into_iter()
        .filter(|path| left.dist.get(path) != right.dist.get(path))
        .collect();
    assert!(
        differing.is_empty(),
        "{context}: relative dist inventory and file bytes must match exactly; differing entries: {differing:?}"
    );
}

fn assert_project_relative_island_label(snapshot: &BuildSnapshot) {
    let compact: String = snapshot
        .islands_entry
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    let call_arguments = format!("\"Counter\",\"Counter\",\"{ISLAND_LABEL}\"");
    assert!(
        compact.contains(&call_arguments),
        "the emitted registration arguments must include the fourth-argument label {ISLAND_LABEL:?}; expected suffix {call_arguments:?}"
    );
}

fn assert_no_path_leaks(snapshot: &BuildSnapshot, root: &Path, scratch_dir: Option<&Path>) {
    let mut forbidden = Vec::<String>::new();
    let mut add_path = |path: &Path| {
        let text = path.to_string_lossy().into_owned();
        if !text.is_empty() && !forbidden.contains(&text) {
            forbidden.push(text);
        }
        if let Ok(canonical) = fs::canonicalize(path) {
            let text = canonical.to_string_lossy().into_owned();
            if !text.is_empty() && !forbidden.contains(&text) {
                forbidden.push(text);
            }
        }
    };
    add_path(root);
    if let Some(scratch_dir) = scratch_dir {
        add_path(scratch_dir);
    }
    add_path(&std::env::temp_dir());
    if let Some(home) = std::env::var_os("HOME") {
        add_path(Path::new(&home));
    }
    forbidden.push("// ../".to_owned());

    for (relative, bytes) in &snapshot.dist {
        for needle in &forbidden {
            let needle = needle.as_bytes();
            assert!(
                needle.is_empty() || !bytes.windows(needle.len()).any(|window| window == needle),
                "dist/{} contains forbidden machine-local path/comment {:?}",
                relative.display(),
                String::from_utf8_lossy(needle)
            );
        }
    }
}

fn assert_clean_build(snapshot: &BuildSnapshot, root: &Path, scratch_dir: Option<&Path>) {
    assert_project_relative_island_label(snapshot);
    assert_no_path_leaks(snapshot, root, scratch_dir);
}

#[test]
fn two_checkout_roots_emit_identical_dist_bytes() {
    let _guard = BUILD_LOCK.lock().unwrap();
    let Some(esbuild) = toolchain() else { return };
    eprintln!("[build_reproducibility_e2e] esbuild: {}", esbuild.display());

    let temp = tempfile::tempdir().expect("create two-root parent tempdir");
    let shallow = temp.path().join("a/app");
    let deep = temp.path().join("b/x/y/app");
    scaffold_project(&shallow);
    scaffold_project(&deep);
    let shallow_nm = fs::canonicalize(shallow.join("node_modules")).unwrap();
    let deep_nm = fs::canonicalize(deep.join("node_modules")).unwrap();
    assert_ne!(
        shallow_nm, deep_nm,
        "each root needs its own node_modules tree"
    );
    assert_ne!(
        fs::canonicalize(shallow.join("node_modules").join(ESCAPE_PACKAGE)).unwrap(),
        fs::canonicalize(deep.join("node_modules").join(ESCAPE_PACKAGE)).unwrap(),
        "pnpm-style package realpaths must differ between roots"
    );

    let a = capture_build(&shallow, &esbuild, None, None);
    let b = capture_build(&deep, &esbuild, None, None);
    assert_eq!(a.build_id, b.build_id, "same inputs must share an identity");
    assert_dist_equal(&a, &b, "different checkout roots");
    assert_clean_build(&a, &shallow, None);
    assert_clean_build(&b, &deep, None);
}

#[test]
fn scratch_location_does_not_change_dist_bytes() {
    let _guard = BUILD_LOCK.lock().unwrap();
    let Some(esbuild) = toolchain() else { return };
    let temp = tempfile::tempdir().expect("create scratch-path tempdir");
    let root = temp.path().join("app");
    let inside = root.join(".zfb-build/repro-inside");
    let outside = temp.path().join("outside-scratch");
    scaffold_project(&root);

    let inside_build = capture_build(&root, &esbuild, Some(&inside), None);
    let outside_build = capture_build(&root, &esbuild, Some(&outside), None);
    assert_eq!(inside_build.build_id, outside_build.build_id);
    assert_dist_equal(
        &inside_build,
        &outside_build,
        "inside vs outside scratch dirs",
    );
    assert_clean_build(&inside_build, &root, Some(&inside));
    assert_clean_build(&outside_build, &root, Some(&outside));
}

#[test]
fn gitignored_test_results_do_not_change_dist_or_identity() {
    let _guard = BUILD_LOCK.lock().unwrap();
    let Some(esbuild) = toolchain() else { return };
    let temp = tempfile::tempdir().expect("create ignored-file tempdir");
    let root = temp.path().join("app");
    scaffold_project(&root);

    let baseline = capture_build(&root, &esbuild, None, None);
    let result = root.join("test-results/.last-run.json");
    fs::create_dir_all(result.parent().unwrap()).expect("create gitignored result directory");
    fs::write(&result, "{\"run\":\"one\"}\n").expect("write gitignored result file");
    let with_result = capture_build(&root, &esbuild, None, None);
    assert_eq!(
        baseline.build_id, with_result.build_id,
        "gitignored test-results files must not affect the owned build identity"
    );
    assert_dist_equal(&baseline, &with_result, "gitignored test-results file");
    assert_clean_build(&baseline, &root, None);
    assert_clean_build(&with_result, &root, None);
}

#[test]
fn hidden_wrangler_state_does_not_change_dist_or_identity() {
    let _guard = BUILD_LOCK.lock().unwrap();
    let Some(esbuild) = toolchain() else { return };
    let temp = tempfile::tempdir().expect("create hidden-file tempdir");
    let root = temp.path().join("app");
    scaffold_project(&root);

    let baseline = capture_build(&root, &esbuild, None, None);
    let state = root.join(".wrangler/state/x.json");
    fs::create_dir_all(state.parent().unwrap()).expect("create hidden Wrangler state directory");
    fs::write(&state, "{\"state\":\"one\"}\n").expect("write hidden Wrangler state file");
    let with_state = capture_build(&root, &esbuild, None, None);
    assert_eq!(
        baseline.build_id, with_state.build_id,
        "unignored hidden Wrangler state must not affect the owned build identity"
    );
    assert_dist_equal(&baseline, &with_state, "hidden .wrangler state file");
    assert_clean_build(&baseline, &root, None);
    assert_clean_build(&with_state, &root, None);
}

#[test]
fn different_defines_produce_different_lockstep_build_ids() {
    let _guard = BUILD_LOCK.lock().unwrap();
    let Some(esbuild) = toolchain() else { return };
    let temp = tempfile::tempdir().expect("create define tempdir");
    let root = temp.path().join("app");
    scaffold_project(&root);

    let one = capture_build(&root, &esbuild, None, Some("1"));
    let two = capture_build(&root, &esbuild, None, Some("2"));
    assert_ne!(
        one.build_id, two.build_id,
        "define values are build identity inputs"
    );
    assert_clean_build(&one, &root, None);
    assert_clean_build(&two, &root, None);
}

#[test]
fn dist_has_no_build_machine_paths_or_realpath_comments() {
    let _guard = BUILD_LOCK.lock().unwrap();
    let Some(esbuild) = toolchain() else { return };
    let temp = tempfile::tempdir().expect("create path-leak tempdir");
    let root = temp.path().join("a/app");
    scaffold_project(&root);

    let snapshot = capture_build(&root, &esbuild, None, None);
    assert_no_path_leaks(&snapshot, &root, None);
    assert!(
        snapshot
            .inner_module
            .contains("// node_modules/repro-realpath-dep/index.js"),
        "realpath escape dependency must survive in _zfb_inner.mjs under its stable label:\n{}",
        snapshot.inner_module
    );
}

#[test]
fn remapped_island_registration_uses_project_relative_label() {
    let _guard = BUILD_LOCK.lock().unwrap();
    let Some(esbuild) = toolchain() else { return };
    let temp = tempfile::tempdir().expect("create island-label tempdir");
    let root = temp.path().join("app");
    scaffold_project(&root);

    let snapshot = capture_build(&root, &esbuild, None, None);
    assert_project_relative_island_label(&snapshot);
    assert_clean_build(&snapshot, &root, None);
}
