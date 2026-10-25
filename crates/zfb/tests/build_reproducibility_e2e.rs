//! Level-3 build-output and Level-4 real-process confirmation for issue #3435.
//!
//! The contract compares complete `dist/` inventories and bytes from real
//! `zfb build` processes. Each successful build also ties the prerendered HTML
//! identity to the islands entry and Cloudflare SSR bundle, then scans all
//! emitted files for machine-local paths. The pnpm-shaped dependency and raw
//! island import force the SSR realpath escape and islands-shadow remap paths.
//!
//! The `lazy-shared-island` tests (#4449) parse every emitted browser module
//! and require that no chunk imports the fingerprinted islands entry and that
//! every relative specifier names a written file, on both bundler backends.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use zfb_test_utils::{locate_esbuild, zfb_binary};

#[path = "support/e2e_platform.rs"]
mod e2e_platform;

use e2e_platform::{link_dir, PNPM_PROGRAM};

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
    Command::new(PNPM_PROGRAM)
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

/// Recreates the symlink `source_path` at `target_path`, keeping its target.
fn copy_symlink(source_path: &Path, target_path: &Path) -> std::io::Result<()> {
    let link_target = fs::read_link(source_path)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(link_target, target_path)
    }
    #[cfg(windows)]
    {
        let resolved = source_path
            .parent()
            .unwrap_or(Path::new(""))
            .join(&link_target);
        if resolved.is_dir() {
            link_dir(&link_target, target_path)
        } else {
            std::os::windows::fs::symlink_file(link_target, target_path)
        }
    }
}

/// Spelling of an absolute path inside a JavaScript import specifier.
fn js_path_specifier(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.trim_start_matches(r"\\?\").replace('\\', "/")
    } else {
        text
    }
}

fn copy_tree_preserving_symlinks(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let kind = fs::symlink_metadata(&source_path)?.file_type();
        if kind.is_symlink() {
            copy_symlink(&source_path, &target_path)?;
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
            copy_symlink(&source_path, &target_path)?;
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
        // A Windows directory symlink or junction is removed with remove_dir.
        fs::remove_file(&adapter_link)
            .or_else(|_| fs::remove_dir(&adapter_link))
            .expect("remove existing adapter link");
    }
    link_dir(&adapter, &adapter_link).expect("link workspace adapter package");
    let bin_dir = node_modules.join(".bin");
    fs::create_dir_all(&bin_dir).expect("create local node_modules bin directory");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        "../@takazudo/zfb-adapter-cloudflare/bin/cli.mjs",
        bin_dir.join("zfb-adapter-cloudflare"),
    )
    .expect("link local adapter executable");
    // Windows has no executable symlinks; pnpm exec resolves a `.cmd` shim via PATHEXT.
    #[cfg(windows)]
    fs::write(
        bin_dir.join("zfb-adapter-cloudflare.cmd"),
        "@node \"%~dp0\\..\\@takazudo\\zfb-adapter-cloudflare\\bin\\cli.mjs\" %*\r\n",
    )
    .expect("write local adapter executable shim");

    let adapter_preflight = Command::new(PNPM_PROGRAM)
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
    ensure_repro_dependency(root);
    assert_repro_store_entry(root);
}

fn repro_store_entry(root: &Path) -> PathBuf {
    root.join(
        "node_modules/.pnpm/repro-realpath-dep@1.0.0/node_modules/repro-realpath-dep/index.js",
    )
}

fn assert_repro_store_entry(root: &Path) -> PathBuf {
    let store_entry = repro_store_entry(root);
    assert!(
        store_entry.is_file(),
        "synthetic pnpm store entry must exist at {}",
        store_entry.display()
    );
    fs::canonicalize(&store_entry).unwrap_or_else(|error| {
        panic!(
            "canonicalize synthetic pnpm store entry {}: {error}",
            store_entry.display()
        )
    })
}

fn ensure_repro_dependency(root: &Path) {
    let node_modules = root.join("node_modules");
    // The adapter runs through pnpm exec and may prune this synthetic entry.
    // Recreate it before every build; its regular logical file is staged, but
    // it re-exports from the root-specific store path to exercise realpath.
    let store_package = node_modules
        .join(".pnpm/repro-realpath-dep@1.0.0/node_modules")
        .join(ESCAPE_PACKAGE);
    fs::create_dir_all(&store_package).expect("create local pnpm store package");
    let store_package_json = serde_json::json!({
        "name": ESCAPE_PACKAGE,
        "version": "1.0.0",
        "type": "module",
        "main": "index.js",
    });
    let store_package_manifest = format!(
        "{}\n",
        serde_json::to_string_pretty(&store_package_json)
            .expect("serialize pnpm store package manifest")
    );
    fs::write(store_package.join("package.json"), &store_package_manifest)
        .expect("write pnpm store package manifest");
    let store_entry = store_package.join("index.js");
    fs::write(
        &store_entry,
        format!("export const marker = {REALPATH_EXPORT:?};\n"),
    )
    .expect("write pnpm store package entry");
    let store_entry_realpath = assert_repro_store_entry(root);

    let logical_package = node_modules.join(ESCAPE_PACKAGE);
    fs::create_dir_all(&logical_package).expect("create logical dependency package directory");
    // The bundler uses esbuild's neutral platform, which ignores `main` unless
    // mainFields is configured. An explicit exports entry makes bare package
    // resolution portable while the re-export targets this root's real store.
    let logical_package_json = serde_json::json!({
        "name": ESCAPE_PACKAGE,
        "version": "1.0.0",
        "type": "module",
        "main": "index.js",
        "exports": "./index.js",
    });
    let logical_package_manifest = format!(
        "{}\n",
        serde_json::to_string_pretty(&logical_package_json)
            .expect("serialize logical dependency package manifest")
    );
    fs::write(
        logical_package.join("package.json"),
        &logical_package_manifest,
    )
    .expect("write logical dependency package manifest");
    let store_entry_specifier = js_path_specifier(&store_entry_realpath);
    let quoted_store_entry_specifier = serde_json::to_string(&store_entry_specifier)
        .expect("encode pnpm store entry as a JavaScript string");
    let logical_entry = logical_package.join("index.js");
    fs::write(
        &logical_entry,
        format!("export {{ marker }} from {quoted_store_entry_specifier};\n"),
    )
    .expect("write staged logical dependency re-export");

    assert!(node_modules.is_dir());
    assert!(!fs::symlink_metadata(&node_modules)
        .expect("node_modules metadata")
        .file_type()
        .is_symlink());
    let logical_package_realpath =
        fs::canonicalize(&logical_package).expect("resolve logical dependency package directory");
    let store_package_realpath = fs::canonicalize(&store_package).unwrap();
    assert_ne!(
        logical_package_realpath, store_package_realpath,
        "logical package directory must remain in node_modules"
    );
    let logical_entry_metadata =
        fs::symlink_metadata(&logical_entry).expect("inspect logical dependency entry");
    assert!(
        logical_entry_metadata.file_type().is_file()
            && !logical_entry_metadata.file_type().is_symlink(),
        "logical dependency entry must be a regular file that staging can copy"
    );
    let logical_entry_realpath = fs::canonicalize(&logical_entry).unwrap();
    assert_ne!(
        logical_entry_realpath, store_entry_realpath,
        "logical entry stays in node_modules while its re-export targets the private store"
    );
    let logical_entry_source = fs::read_to_string(&logical_entry).unwrap();
    assert!(
        logical_entry_source.contains(&quoted_store_entry_specifier),
        "logical dependency re-export must target its absolute private store entry"
    );
}

fn run_build(root: &Path, esbuild: &Path, scratch_dir: Option<&Path>, define_value: Option<&str>) {
    ensure_repro_dependency(root);
    assert_repro_store_entry(root);
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

/// Whitespace-free JS with Rolldown's minifier backtick literals spelled as `"`.
fn compact_js(source: &str) -> String {
    source
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .map(|ch| if ch == '`' { '"' } else { ch })
        .collect()
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
    if let Some(text) = literal
        .strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .filter(|text| !text.contains(['`', '\\', '$']))
    {
        return text.to_string();
    }
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

    let islands_compact = compact_js(&islands_entry);
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

fn assert_island_label(snapshot: &BuildSnapshot, component: &str, label: &str) {
    let compact = compact_js(&snapshot.islands_entry);
    let call_arguments = format!("\"{component}\",\"{component}\",\"{label}\"");
    assert!(
        compact.contains(&call_arguments),
        "the emitted registration arguments must include the fourth-argument label {label:?}; expected suffix {call_arguments:?}"
    );
}

/// Every spelling a machine-local path can take in emitted bytes. On Windows
/// that adds forward-slash, backslash, JSON-escaped (`\\\\`) and `\\?\\`-prefixed
/// forms in both drive-letter cases; on unix the path is the only spelling.
fn path_spellings(text: &str) -> Vec<String> {
    let mut out = vec![text.to_owned()];
    if !cfg!(windows) {
        return out;
    }
    let bare = text.trim_start_matches(r"\\?\");
    let forward = bare.replace('\\', "/");
    let backslash = forward.replace('/', "\\");
    let mut bases = vec![forward, backslash];
    let bytes = bare.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let flipped_letter = if bytes[0].is_ascii_uppercase() {
            bytes[0].to_ascii_lowercase()
        } else {
            bytes[0].to_ascii_uppercase()
        } as char;
        let flipped: Vec<String> = bases
            .iter()
            .map(|base| format!("{flipped_letter}{}", &base[1..]))
            .collect();
        bases.extend(flipped);
    }
    for base in bases {
        if base.contains('\\') {
            out.push(base.replace('\\', "\\\\"));
            out.push(format!(r"\\?\{base}"));
        }
        out.push(base);
    }
    out.sort();
    out.dedup();
    out
}

fn assert_no_path_leaks(snapshot: &BuildSnapshot, root: &Path, scratch_dir: Option<&Path>) {
    let mut forbidden = Vec::<String>::new();
    let mut add_text = |text: String| {
        if !text.is_empty() && !forbidden.contains(&text) {
            forbidden.push(text);
        }
    };
    let mut add_path = |path: &Path| {
        let mut texts = vec![path.to_string_lossy().into_owned()];
        if let Ok(canonical) = fs::canonicalize(path) {
            texts.push(canonical.to_string_lossy().into_owned());
        }
        for text in texts {
            for spelling in path_spellings(&text) {
                add_text(spelling);
            }
        }
    };
    add_path(root);
    if let Some(scratch_dir) = scratch_dir {
        add_path(scratch_dir);
    }
    add_path(&std::env::temp_dir());
    for var in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(var) {
            add_path(Path::new(&home));
        }
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
    assert_island_label(snapshot, "Counter", ISLAND_LABEL);
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
    let shallow_store_entry = assert_repro_store_entry(&shallow);
    let deep_store_entry = assert_repro_store_entry(&deep);
    assert_ne!(
        shallow_store_entry, deep_store_entry,
        "private pnpm store entry realpaths must differ between roots"
    );
    for (root, store_entry) in [(&shallow, &shallow_store_entry), (&deep, &deep_store_entry)] {
        let logical_entry = root.join("node_modules/repro-realpath-dep/index.js");
        let metadata = fs::symlink_metadata(&logical_entry).unwrap();
        assert!(metadata.file_type().is_file() && !metadata.file_type().is_symlink());
        let logical_entry_realpath = fs::canonicalize(&logical_entry).unwrap();
        assert_ne!(
            logical_entry_realpath, *store_entry,
            "logical entry must be a staged file separate from the private store entry"
        );
        let source = fs::read_to_string(logical_entry).unwrap();
        assert!(
            source.contains(&serde_json::to_string(&js_path_specifier(store_entry)).unwrap()),
            "logical entry must re-export from this root's private store entry"
        );
    }

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
fn linked_workspace_ignored_artifacts_do_not_change_identity_but_declared_dist_edits_do() {
    let _guard = BUILD_LOCK.lock().unwrap();
    let Some(esbuild) = toolchain() else { return };
    let temp = tempfile::tempdir().expect("create linked-workspace tempdir");
    let workspace_root = temp.path();
    let app = workspace_root.join("app");
    let widget = workspace_root.join("widget");

    fs::write(
        workspace_root.join(".gitignore"),
        "node_modules/\n.zfb-build/\ndist/\ntest-results/\n",
    )
    .expect("write workspace ignore rules");
    // This workspace manifest establishes W for the bounded linked-package ignore chain.
    fs::write(
        workspace_root.join("pnpm-workspace.yaml"),
        "packages:\n  - app\n  - widget\nlinkWorkspacePackages: false\n",
    )
    .expect("write pnpm workspace manifest");
    scaffold_project(&app);

    fs::create_dir_all(app.join("plugins")).expect("create app plugin directory");
    fs::write(
        app.join("plugins/alias.mjs"),
        "export default { name: 'widget-alias', setup(ctx) { ctx.addAlias('@sample/widget', '../widget/dist/index.js'); } };\n",
    )
    .expect("write linked widget alias plugin");
    let config_path = app.join("zfb.config.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).expect("read fixture zfb config"))
            .expect("parse fixture zfb config");
    config["plugins"] = serde_json::json!([{"name": "./plugins/alias.mjs"}]);
    fs::write(
        config_path,
        format!("{}\n", serde_json::to_string_pretty(&config).unwrap()),
    )
    .expect("enable linked widget alias plugin");
    fs::write(
        app.join("pages/index.tsx"),
        "import { Island } from '@takazudo/zfb';\nimport { Probe } from '../components/probe';\n\nexport default function Page() {\n  return <html><head><title>Linked identity</title></head><body><Island when=\"load\"><Probe /></Island></body></html>;\n}\n",
    )
    .expect("write linked widget island page");
    fs::write(
        app.join("components/probe.tsx"),
        "'use client';\nimport { label } from '@sample/widget';\n\nexport function Probe() {\n  return <div>{label}</div>;\n}\n",
    )
    .expect("write linked widget Probe island");

    fs::create_dir_all(widget.join("dist")).expect("create widget dist directory");
    fs::write(
        widget.join("package.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::json!({
                "name": "@sample/widget",
                "type": "module",
                "version": "1.0.0",
                "exports": "./dist/index.js",
                "files": ["dist"],
            }))
            .unwrap()
        ),
    )
    .expect("write widget package manifest");
    let widget_entry = widget.join("dist/index.js");
    fs::write(&widget_entry, "export const label = 'Widget';\n").expect("write widget entry");

    let baseline = capture_build(&app, &esbuild, None, None);
    let artifacts = widget.join("test-results");
    fs::create_dir_all(&artifacts).expect("create ignored widget test-results directory");
    fs::write(artifacts.join("probe.json"), "{\"run\":\"one\"}\n")
        .expect("write ignored widget JSON result");
    fs::write(
        artifacts.join("report.html"),
        "<!doctype html><title>test report</title>\n",
    )
    .expect("write ignored widget HTML report");
    let with_artifacts = capture_build(&app, &esbuild, None, None);
    assert_eq!(
        baseline.build_id, with_artifacts.build_id,
        "ignored sibling test-results artifacts must not affect the owned build identity"
    );
    assert_dist_equal(
        &baseline,
        &with_artifacts,
        "ignored sibling widget test-results artifacts",
    );
    // The dedicated registration-label case below covers emitted source labels;
    // this fixture checks identity and dist stability for a linked package.
    assert_no_path_leaks(&baseline, &app, None);
    assert_no_path_leaks(&with_artifacts, &app, None);

    fs::write(&widget_entry, "export const label = 'Widget changed';\n")
        .expect("edit declared widget dist entry");
    let with_declared_edit = capture_build(&app, &esbuild, None, None);
    assert_ne!(
        with_artifacts.build_id, with_declared_edit.build_id,
        "edits to the widget's declared dist entry must affect the owned build identity"
    );
    assert_no_path_leaks(&with_declared_edit, &app, None);
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
        ["// ", "//#region "].iter().any(|prefix| snapshot
            .inner_module
            .contains(&format!("{prefix}node_modules/repro-realpath-dep/index.js"))),
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
    assert_island_label(&snapshot, "Counter", ISLAND_LABEL);
    assert_clean_build(&snapshot, &root, None);
}

/// #4231: HTML-only edits must not change any shipped browser entry/chunk
/// bytes, while an input imported by the island must invalidate the identity.
#[test]
fn unrelated_title_and_markdown_edits_preserve_island_entry_and_chunk_bytes() {
    let _guard = BUILD_LOCK.lock().unwrap();
    let Some(esbuild) = toolchain() else { return };
    let temp = tempfile::tempdir().expect("create graph-identity tempdir");
    let root = temp.path().join("app");
    scaffold_project(&root);

    // Keep a genuine dynamic edge so equality covers emitted chunks as well
    // as the root. The callback is shipped even though this build-output test
    // does not click the button.
    let counter_path = root.join("components/counter.tsx");
    let counter = fs::read_to_string(&counter_path).unwrap();
    assert!(counter.contains("<button type="));
    fs::write(
        &counter_path,
        counter.replace(
            "<button type=",
            "<button on:click={() => import(\"./counter-details\").then(({ detail }) => console.log(detail))} type=",
        ),
    ).unwrap();
    fs::write(
        root.join("components/counter-details.ts"),
        "export const detail = 'DYNAMIC-ISLAND-DETAIL';\n",
    )
    .unwrap();
    let markdown_path = root.join("pages/unrelated.md");
    fs::write(&markdown_path, "# Unrelated page\n\nOriginal prose.\n").unwrap();

    let browser_js = |snapshot: &BuildSnapshot| -> BTreeMap<PathBuf, Vec<u8>> {
        snapshot
            .dist
            .iter()
            .filter(|(path, _)| {
                path.starts_with("assets") && path.extension().is_some_and(|ext| ext == "js")
            })
            .map(|(path, bytes)| (path.clone(), bytes.clone()))
            .collect()
    };
    let baseline = capture_build(&root, &esbuild, None, None);
    let original_js = browser_js(&baseline);
    assert!(
        original_js.len() >= 2,
        "fixture must emit an entry and a dynamic chunk: {:?}",
        original_js.keys()
    );

    let page_path = root.join("pages/index.tsx");
    let page = fs::read_to_string(&page_path).unwrap();
    assert!(page.contains("Bundle reproducibility"));
    fs::write(
        &page_path,
        page.replace("Bundle reproducibility", "Changed page title"),
    )
    .unwrap();
    let title_edit = capture_build(&root, &esbuild, None, None);
    assert_ne!(
        baseline.dist.get(Path::new("index.html")),
        title_edit.dist.get(Path::new("index.html")),
        "the title edit must reach emitted HTML"
    );
    assert_eq!(baseline.build_id, title_edit.build_id);
    assert_eq!(
        original_js,
        browser_js(&title_edit),
        "unrelated page title changed raw browser bytes or filenames"
    );

    fs::write(&markdown_path, "# Unrelated page\n\nChanged prose.\n").unwrap();
    let markdown_edit = capture_build(&root, &esbuild, None, None);
    assert_ne!(
        title_edit.dist, markdown_edit.dist,
        "the Markdown edit must reach emitted output"
    );
    assert_eq!(baseline.build_id, markdown_edit.build_id);
    assert_eq!(
        original_js,
        browser_js(&markdown_edit),
        "unrelated Markdown changed raw browser bytes or filenames"
    );

    fs::write(
        root.join("components/counter-note.txt"),
        "RELEVANT-RAW-INPUT-CHANGED\n",
    )
    .unwrap();
    let relevant_edit = capture_build(&root, &esbuild, None, None);
    assert_ne!(
        markdown_edit.build_id, relevant_edit.build_id,
        "an imported raw input must invalidate SSR/client identity"
    );
    assert_ne!(
        markdown_edit.islands_entry, relevant_edit.islands_entry,
        "relevant raw input must change shipped root bytes"
    );
    assert!(relevant_edit
        .islands_entry
        .contains("RELEVANT-RAW-INPUT-CHANGED"));
    assert_ne!(original_js, browser_js(&relevant_edit));
    for snapshot in [&baseline, &title_edit, &markdown_edit, &relevant_edit] {
        assert_clean_build(snapshot, &root, None);
    }
}

#[test]
fn virtual_sibling_raw_import_identity_survives_fresh_preprocessing_shadows() {
    let _guard = BUILD_LOCK.lock().unwrap();
    let Some(esbuild) = toolchain() else { return };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("app");
    let sibling = temp.path().join("widget");
    fs::write(
        temp.path().join("pnpm-workspace.yaml"),
        "packages:\n  - app\n  - widget\nlinkWorkspacePackages: false\n",
    )
    .unwrap();
    scaffold_project(&root);
    fs::create_dir_all(&sibling).unwrap();
    fs::write(
        sibling.join("package.json"),
        r#"{"name":"virtual-widget","type":"module","version":"1.0.0"}"#,
    )
    .unwrap();
    fs::write(
        sibling.join("entry.ts"),
        "import note from './note.txt?raw'; export { note };\n",
    )
    .unwrap();
    fs::write(sibling.join("note.txt"), "VIRTUAL-SIBLING-RAW-ONE\n").unwrap();
    fs::create_dir_all(root.join("plugins")).unwrap();
    fs::write(root.join("plugins/virtual.mjs"), r#"
import path from 'node:path';
export default {
  name: 'virtual-sibling-identity',
  setup({ projectRoot, addVirtualModule }) {
    const sibling = path.resolve(projectRoot, '../widget/entry.ts');
    addVirtualModule('virtual:identity-note', () => `export { note } from ${JSON.stringify(sibling)};`);
  }
};
"#).unwrap();
    let config_path = root.join("zfb.config.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["plugins"] = serde_json::json!([{"name":"./plugins/virtual.mjs"}]);
    fs::write(config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let counter_path = root.join("components/counter.tsx");
    let counter = fs::read_to_string(&counter_path).unwrap();
    let old_import = "import note from \"./counter-note.txt?raw\";";
    assert!(counter.contains(old_import));
    fs::write(
        counter_path,
        counter.replace(old_import, "import { note } from 'virtual:identity-note';"),
    )
    .unwrap();

    // Each capture performs independent SSR and client preparations. Their
    // random shadow paths must never leak into the shared build identity.
    let first = capture_build(&root, &esbuild, None, None);
    let second = capture_build(&root, &esbuild, None, None);
    assert_eq!(first.build_id, second.build_id);
    assert_dist_equal(
        &first,
        &second,
        "fresh virtual-module preprocessing shadows",
    );
    assert!(second.islands_entry.contains("VIRTUAL-SIBLING-RAW-ONE"));
    fs::write(sibling.join("note.txt"), "VIRTUAL-SIBLING-RAW-TWO\n").unwrap();
    let changed = capture_build(&root, &esbuild, None, None);
    assert_ne!(second.build_id, changed.build_id);
    assert!(changed.islands_entry.contains("VIRTUAL-SIBLING-RAW-TWO"));
}

// ---------------------------------------------------------------------------
// #4449 (epic #4448, source bug #4444): production renames only the islands
// entry (`islands.js` -> `islands-<hash>.js`) and writes chunks verbatim, so a
// chunk importing the entry 404s. The emitted `dist/` is the oracle; nothing
// here predicts how either bundler splits the graph.
// ---------------------------------------------------------------------------
// The specifier oracle is the Rolldown crate's parser helper, re-exported by
// zfb-islands only when `rolldown-prototype` (a default feature) is on.
#[cfg(feature = "rolldown-prototype")]
mod lazy_shared_island {
    use super::*;

    fn emitted_specifiers(path: &Path, source: &str) -> Vec<String> {
        zfb_islands::module_specifiers(source)
            .unwrap_or_else(|error| panic!("emitted {}: {error}", path.display()))
    }

    /// Lexically joins a relative specifier onto the importer's dist directory.
    fn resolve_in_dist(importer: &Path, specifier: &str) -> PathBuf {
        let mut resolved = importer.parent().unwrap_or(Path::new("")).to_path_buf();
        for segment in specifier.split('/') {
            match segment {
                "." | "" => {}
                ".." => {
                    assert!(
                        resolved.pop(),
                        "{} climbs out of dist with {specifier:?}",
                        importer.display()
                    );
                }
                other => resolved.push(other),
            }
        }
        resolved
    }

    /// The islands entry under any of its names: the stable bundler output or a
    /// fingerprinted `islands-<hash>.js` (never a self-hashed `islands-chunk-*`
    /// or a copied `islands-resource-*`).
    fn names_islands_entry(specifier: &str, entry_file: &str) -> bool {
        let file = specifier.rsplit('/').next().unwrap_or(specifier);
        let file = file.split(['?', '#']).next().unwrap_or(file);
        file == "islands.js"
            || file == entry_file
            || (file.starts_with("islands-")
                && !file.starts_with("islands-chunk-")
                && !file.starts_with("islands-resource-")
                && file.ends_with(".js"))
    }

    fn run_lazy_shared_build(root: &Path, esbuild: &Path, bundler: &str) {
        let output = Command::new(zfb_binary!())
            .arg("build")
            .current_dir(root)
            .env("ZFB_ESBUILD_BIN", esbuild)
            .env("ZFB_BUNDLER", bundler)
            .env_remove("ZFB_ROLLDOWN_PROTOTYPE")
            .output()
            .expect("spawn real `zfb build`");
        assert!(
            output.status.success(),
            "`zfb build` ({bundler}) must succeed for {}\nstatus: {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            root.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    fn assert_no_chunk_imports_the_islands_entry(bundler: &str, esbuild: &Path) {
        let temp = tempfile::tempdir().expect("create lazy-shared tempdir");
        let root = temp.path().join("app");
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lazy-shared-island");
        copy_fixture(&fixture, &root).expect("copy lazy-shared-island fixture");
        run_lazy_shared_build(&root, esbuild, bundler);
        let dist = read_dist_inventory(&root);
        let render_js = || -> String {
            dist.iter()
                .filter(|(path, _)| {
                    path.extension()
                        .is_some_and(|ext| ext == "js" || ext == "mjs")
                })
                .map(|(path, bytes)| {
                    format!(
                        "--- {} ---\n{}",
                        path.display(),
                        String::from_utf8_lossy(bytes)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        // (iii) the entry is fingerprinted and referenced from HTML; chunks keep
        // their bundler-owned content names.
        let html = String::from_utf8(
            dist.get(Path::new("index.html"))
                .expect("fixture build must emit dist/index.html")
                .clone(),
        )
        .expect("prerendered HTML is UTF-8");
        assert!(
            html.contains("data-lazy-host"),
            "{bundler}: island SSR missing:\n{html}"
        );
        let entry = html_islands_entry(&html);
        let entry_file = entry
            .file_name()
            .and_then(|name| name.to_str())
            .expect("islands entry filename")
            .to_owned();
        assert!(
            entry_file.starts_with("islands-")
                && !entry_file.starts_with("islands-chunk-")
                && entry_file.len() > "islands-.js".len(),
            "{bundler}: HTML must reference a fingerprinted entry, got {}",
            entry.display()
        );
        assert!(
            dist.contains_key(&entry),
            "{bundler}: HTML entry {} was not written",
            entry.display()
        );
        assert!(
            !dist.contains_key(Path::new("assets/islands.js")),
            "{bundler}: production must not publish a stable assets/islands.js"
        );
        let chunks: Vec<&PathBuf> = dist
            .keys()
            .filter(|path| {
                path.starts_with("assets")
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            name.starts_with("islands-chunk-") && name.ends_with(".js")
                        })
            })
            .collect();
        assert!(
            !chunks.is_empty(),
            "{bundler}: the lazy import() must emit at least one islands-chunk-*.js:\n{}",
            render_js()
        );

        // (i) + (ii) over every emitted browser module under dist/assets.
        let mut checked = 0usize;
        for (path, bytes) in &dist {
            let is_js = path
                .extension()
                .is_some_and(|ext| ext == "js" || ext == "mjs");
            if !path.starts_with("assets") || !is_js {
                continue;
            }
            let source = String::from_utf8(bytes.clone())
                .unwrap_or_else(|_| panic!("{} is not UTF-8", path.display()));
            for specifier in emitted_specifiers(path, &source) {
                if path != &entry {
                    assert!(
                        !names_islands_entry(&specifier, &entry_file),
                        "{bundler}: {} imports the islands entry as {specifier:?}; production only \
                         publishes {entry_file}\n{}",
                        path.display(),
                        render_js()
                    );
                }
                if specifier.starts_with("./") || specifier.starts_with("../") {
                    let target = resolve_in_dist(path, &specifier);
                    assert!(
                        dist.contains_key(&target),
                        "{bundler}: {} imports {specifier:?}, but {} was not written\n{}",
                        path.display(),
                        target.display(),
                        render_js()
                    );
                    checked += 1;
                }
            }
        }
        assert!(
            checked > 0,
            "{bundler}: no relative specifier was checked in dist/assets\n{}",
            render_js()
        );
        eprintln!(
            "[build_reproducibility_e2e] lazy-shared-island ({bundler}): {} chunk(s), {checked} relative specifier(s) resolved",
            chunks.len()
        );
    }

    #[test]
    fn rolldown_lazy_chunks_never_import_the_fingerprinted_islands_entry() {
        let _guard = BUILD_LOCK.lock().unwrap();
        let Some(esbuild) = locate_esbuild() else {
            eprintln!("[build_reproducibility_e2e] no esbuild binary available; skipping");
            return;
        };
        assert_no_chunk_imports_the_islands_entry("rolldown", &esbuild);
    }

    #[test]
    fn esbuild_lazy_chunks_never_import_the_fingerprinted_islands_entry() {
        let _guard = BUILD_LOCK.lock().unwrap();
        let Some(esbuild) = locate_esbuild() else {
            eprintln!("[build_reproducibility_e2e] no esbuild binary available; skipping");
            return;
        };
        assert_no_chunk_imports_the_islands_entry("esbuild", &esbuild);
    }
}
