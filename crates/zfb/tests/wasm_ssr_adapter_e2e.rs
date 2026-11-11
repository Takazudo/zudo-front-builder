//! Level-4 confirmation for the Wasm SSR adapter handoff (issue #1510).
//!
//! A single real `zfb build` must keep the full SSG bundle broad enough to
//! render static routes, then build a second SSR-only bundle for the adapter.
//! This fixture gives each pass a distinct consumed Wasm import so a
//! tree-shaken or accidentally shared asset manifest cannot pass by accident.
//!
//! The ignored Wrangler check intentionally stops at the deploy bundle. A
//! workerd execution lane is platform-heavy and remains deferred; this test
//! proves Wrangler classifies the runtime module as `compiled-wasm` instead.

#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use zfb_test_utils::zfb_binary;

const EXPECTED_WRANGLER_VERSION: &str = "4.85.0";
const SSG_WASM_VALUE: u8 = 17;
const SSR_WASM_VALUE: u8 = 29;

// A minimal Wasm module exporting `answer(): i32`. The final immediate differs
// between the two files, making an accidental same-asset assertion impossible.
const SSG_WASM: &[u8] = &[
    0x00,
    0x61,
    0x73,
    0x6d,
    0x01,
    0x00,
    0x00,
    0x00,
    0x01,
    0x05,
    0x01,
    0x60,
    0x00,
    0x01,
    0x7f,
    0x03,
    0x02,
    0x01,
    0x00,
    0x07,
    0x0a,
    0x01,
    0x06,
    0x61,
    0x6e,
    0x73,
    0x77,
    0x65,
    0x72,
    0x00,
    0x00,
    0x0a,
    0x06,
    0x01,
    0x04,
    0x00,
    0x41,
    SSG_WASM_VALUE,
    0x0b,
];
const SSR_WASM: &[u8] = &[
    0x00,
    0x61,
    0x73,
    0x6d,
    0x01,
    0x00,
    0x00,
    0x00,
    0x01,
    0x05,
    0x01,
    0x60,
    0x00,
    0x01,
    0x7f,
    0x03,
    0x02,
    0x01,
    0x00,
    0x07,
    0x0a,
    0x01,
    0x06,
    0x61,
    0x6e,
    0x73,
    0x77,
    0x65,
    0x72,
    0x00,
    0x00,
    0x0a,
    0x06,
    0x01,
    0x04,
    0x00,
    0x41,
    SSR_WASM_VALUE,
    0x0b,
];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("wasm-ssr-confirm")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/zfb must live two levels under the workspace root")
        .to_path_buf()
}

fn copy_fixture(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_fixture(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn pnpm_available() -> bool {
    Command::new("pnpm")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn workspace_adapter_package() -> PathBuf {
    let adapter = workspace_root().join("packages/zfb-adapter-cloudflare");
    assert!(
        adapter.join("package.json").is_file()
            && adapter.join("bin/cli.mjs").is_file()
            && adapter.join("src/emit-worker.mjs").is_file(),
        "the workspace Cloudflare adapter package must provide its CLI at {}",
        adapter.display(),
    );
    adapter
}

/// Copy the checked-in sources and write the binary Wasm inputs at test time.
/// Keeping the tiny binaries here makes the values visibly distinct without
/// requiring a binary fixture file in git.
fn scaffold_fixture(adapter_package: &Path) -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("create Wasm SSR confirmation tempdir");
    let root = temp.path().join("project");
    copy_fixture(&fixture_dir(), &root).expect("copy Wasm SSR fixture");

    fs::create_dir_all(root.join("wasm")).expect("create fixture Wasm directory");
    fs::write(root.join("wasm/ssg-only.wasm"), SSG_WASM).expect("write SSG Wasm fixture");
    fs::write(root.join("wasm/ssr-only.wasm"), SSR_WASM).expect("write SSR Wasm fixture");

    // Use the same embedded framework tree that the binary uses for a
    // consumer without a local install, then add the actual workspace adapter
    // as a fixture-local package. The `.bin` link is what `pnpm exec` resolves
    // during the production adapter dispatch; no PATH substitution is used.
    let (node_modules_handle, node_modules) =
        zfb::render_pipeline::embedded_node_modules().expect("extract embedded node_modules");
    let scope = node_modules.join("@takazudo");
    fs::create_dir_all(&scope).expect("create adapter scope directory");
    std::os::unix::fs::symlink(adapter_package, scope.join("zfb-adapter-cloudflare"))
        .expect("link workspace adapter package into fixture node_modules");
    let bin_dir = node_modules.join(".bin");
    fs::create_dir_all(&bin_dir).expect("create fixture node_modules bin directory");
    std::os::unix::fs::symlink(
        "../@takazudo/zfb-adapter-cloudflare/bin/cli.mjs",
        bin_dir.join("zfb-adapter-cloudflare"),
    )
    .expect("link fixture-local adapter bin");
    std::os::unix::fs::symlink(&node_modules, root.join("node_modules"))
        .expect("symlink fixture node_modules");

    (temp, node_modules_handle, root)
}

fn run_build(root: &Path) {
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .output()
        .expect("spawn `zfb build`");

    assert!(
        output.status.success(),
        "`zfb build` must succeed for the Wasm SSR fixture\nstatus: {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn wasm_assets_in(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .filter_map(|entry| {
            let entry = entry.expect("read directory entry");
            let path = entry.path();
            (path.extension().and_then(|ext| ext.to_str()) == Some("wasm"))
                .then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect()
}

fn named_wasm_asset<'a>(assets: &'a BTreeSet<String>, prefix: &str) -> &'a str {
    let matching: Vec<_> = assets
        .iter()
        .filter(|name| name.starts_with(prefix) && name.ends_with(".wasm"))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected one Wasm asset beginning with {prefix:?}; got {assets:#?}"
    );
    matching[0]
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn assert_wasm_two_pass_contract(root: &Path) -> String {
    let bundle_dir = root.join(".zfb-build");
    let full_bundle = read_text(&bundle_dir.join("bundle.mjs"));
    let runtime_bundle = read_text(&bundle_dir.join("bundle-runtime.mjs"));
    let bundle_assets = wasm_assets_in(&bundle_dir);
    let ssg_asset = named_wasm_asset(&bundle_assets, "ssg-only-");
    let runtime_asset = named_wasm_asset(&bundle_assets, "ssr-only-");

    // The first pass is what V8 evaluates to render SSG output, so it must
    // retain both modules. The adapter's second pass must exclude SSG-only
    // code and its copied module altogether.
    assert!(
        full_bundle.contains(ssg_asset),
        "the full SSG bundle must import its hashed Wasm asset {ssg_asset:?}"
    );
    assert!(
        full_bundle.contains(runtime_asset),
        "the full SSG bundle must also import the SSR Wasm asset {runtime_asset:?}"
    );
    assert!(
        runtime_bundle.contains(runtime_asset),
        "the runtime-only bundle must import its hashed Wasm asset {runtime_asset:?}"
    );
    assert!(
        !runtime_bundle.contains(ssg_asset),
        "the runtime-only bundle must not retain SSG-only Wasm asset {ssg_asset:?}"
    );

    let dist = root.join("dist");
    let index_html = read_text(&dist.join("index.html"));
    assert!(
        index_html.contains(&format!("SSG_WASM_VALUE:{SSG_WASM_VALUE}")),
        "the SSG page must instantiate the imported Wasm module through V8\n--- index.html ---\n{index_html}"
    );
    assert!(
        !dist.join("runtime").exists(),
        "the prerender=false runtime page must not be emitted as static HTML"
    );

    let dist_assets = wasm_assets_in(&dist);
    assert_eq!(
        dist_assets,
        BTreeSet::from([runtime_asset.to_owned()]),
        "the adapter output must contain only the runtime Wasm module"
    );
    assert!(
        !dist.join(ssg_asset).exists(),
        "the adapter output must not copy the SSG-only Wasm module"
    );
    assert_eq!(
        fs::read(dist.join(runtime_asset)).expect("read copied runtime Wasm"),
        SSR_WASM,
        "the copied runtime Wasm bytes must match the consumed SSR source module"
    );

    let assetsignore = read_text(&dist.join(".assetsignore"));
    let ignored: Vec<_> = assetsignore.lines().collect();
    let runtime_asset_rule = format!("/{runtime_asset}");
    assert_eq!(
        ignored,
        ["/_worker.js", "/_zfb_inner.mjs", runtime_asset_rule.as_str()],
        ".assetsignore must contain only the root-anchored Worker files and runtime Wasm asset, once each"
    );

    assert!(
        dist.join("_worker.js").is_file(),
        "adapter must emit _worker.js"
    );
    let inner = read_text(&dist.join("_zfb_inner.mjs"));
    assert!(
        inner.contains(runtime_asset),
        "_zfb_inner.mjs must import the exact runtime asset filename copied into dist"
    );
    assert!(
        !inner.contains(ssg_asset),
        "_zfb_inner.mjs must not import the SSG-only asset"
    );
    assert!(
        inner.contains("SSR_WASM_VALUE"),
        "the SSR route must consume the Wasm module in emitted runtime code"
    );

    runtime_asset.to_owned()
}

fn locate_pinned_wrangler() -> Option<PathBuf> {
    let configured = std::env::var_os("ZFB_WRANGLER_BIN").map(PathBuf::from);
    configured.or_else(|| {
        let workspace_bin = workspace_root().join("node_modules/.bin/wrangler");
        workspace_bin.is_file().then_some(workspace_bin)
    })
}

fn assert_pinned_wrangler_version(wrangler: &Path) {
    let output = Command::new(wrangler)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("run {} --version: {error}", wrangler.display()));
    let version = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && version.trim() == EXPECTED_WRANGLER_VERSION,
        "expected workspace-pinned wrangler {EXPECTED_WRANGLER_VERSION}, got status {} / stdout {:?} / stderr {:?}",
        output.status,
        version.trim(),
        String::from_utf8_lossy(&output.stderr).trim(),
    );
}

/// One real build covers the composed two-pass contract: V8 successfully loads
/// both consumed modules for SSG, while the final adapter output carries only
/// the runtime module through its bundle and static-assets exclusion list.
#[test]
fn build_keeps_ssg_and_ssr_wasm_assets_in_their_respective_passes() {
    if !pnpm_available() {
        eprintln!("[wasm_ssr_adapter_e2e] pnpm is not available; skipping.");
        return;
    }

    let adapter = workspace_adapter_package();
    let (_temp, _node_modules, root) = scaffold_fixture(&adapter);
    run_build(&root);
    let _runtime_asset = assert_wasm_two_pass_contract(&root);
}

/// Paths that `git check-ignore` reports as ignored by `policy`, used as the
/// gitignore-style reference matcher for the rule subset the adapter emits
/// (root-anchored literals) plus plain user globs, anchors and negations.
fn git_ignored_paths(policy: &str, paths: &[&str]) -> Vec<String> {
    use std::io::Write;
    use std::process::Stdio;

    let repo = tempfile::tempdir().expect("create git check-ignore tempdir");
    let init = Command::new("git")
        .args(["init", "-q"])
        .arg(repo.path())
        .status()
        .expect("spawn git init");
    assert!(init.success(), "git init must succeed");
    fs::write(repo.path().join(".gitignore"), policy).expect("write policy as .gitignore");
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["check-ignore", "--no-index", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn git check-ignore");
    child
        .stdin
        .take()
        .expect("check-ignore stdin")
        .write_all((paths.join("\n") + "\n").as_bytes())
        .expect("write check-ignore paths");
    let output = child.wait_with_output().expect("wait for git check-ignore");
    // check-ignore exits 1 when no path is ignored.
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "git check-ignore failed: {}",
        output.status
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Combined acceptance for #4681 + #4682: a subpath build places the user's
/// public policy at `dist/.assetsignore` (Wrangler's only reading location)
/// before adapter emission, and the final root file preserves it unchanged
/// and ends with the adapter's effective root-anchored exclusions. The user's
/// patterns stay relative to the upload root; nothing is rebased under `base`.
#[test]
fn subpath_build_keeps_public_assetsignore_at_root_with_generated_exclusions_last() {
    if !pnpm_available() {
        eprintln!("[wasm_ssr_adapter_e2e] pnpm is not available; skipping.");
        return;
    }

    let adapter = workspace_adapter_package();
    let (_temp, _node_modules, root) = scaffold_fixture(&adapter);
    fs::write(
        root.join("zfb.config.json"),
        "{\n  \"adapter\": \"@takazudo/zfb-adapter-cloudflare\",\n  \"base\": \"/sub/\"\n}\n",
    )
    .expect("rewrite fixture config with a subpath base");
    let user_policy =
        "private.txt\n/root-only.txt\n/sub/secret.txt\n!_worker.js\n!_zfb_inner.mjs\n";
    fs::create_dir_all(root.join("public")).expect("create public dir");
    fs::write(root.join("public/.assetsignore"), user_policy).expect("write public policy");
    for name in ["private.txt", "root-only.txt", "secret.txt", "visible.txt"] {
        fs::write(root.join("public").join(name), name).expect("write public file");
    }

    run_build(&root);

    let dist = root.join("dist");
    assert!(
        !dist.join("sub/.assetsignore").exists(),
        "the public policy must not be relocated beneath the base segment"
    );
    for name in ["private.txt", "root-only.txt", "secret.txt", "visible.txt"] {
        assert!(
            dist.join("sub").join(name).is_file(),
            "public/{name} must follow the base segment"
        );
    }
    let runtime_assets = wasm_assets_in(&dist);
    let runtime_asset = named_wasm_asset(&runtime_assets, "ssr-only-");
    let policy = read_text(&dist.join(".assetsignore"));
    let generated_block = format!("/_worker.js\n/_zfb_inner.mjs\n/{runtime_asset}\n");
    assert_eq!(
        policy,
        format!("{user_policy}{generated_block}"),
        "the root policy must be the user's bytes followed by the generated block"
    );

    let runtime_asset_path = runtime_asset.to_owned();
    assert_eq!(
        git_ignored_paths(
            &policy,
            &[
                "sub/private.txt",
                "sub/root-only.txt",
                "sub/secret.txt",
                "sub/visible.txt",
                "_worker.js",
                "_zfb_inner.mjs",
                runtime_asset_path.as_str(),
                "sub/_worker.js",
            ],
        ),
        [
            "sub/private.txt",
            "sub/secret.txt",
            "_worker.js",
            "_zfb_inner.mjs",
            runtime_asset_path.as_str(),
        ],
        "unanchored rules match under the base, root anchors are not rebased, and \
         generated exclusions win over later user negations"
    );
}

/// The deploy handoff needs the workspace-pinned Wrangler binary. It stays
/// ignored by default because it is a platform/tooling env-gate, but when the
/// tool is present it must prove the final Wasm import is a compiled module.
#[test]
#[ignore = "env-gate: wrangler — workspace-pinned 4.85.0; run cargo test -p zfb --test wasm_ssr_adapter_e2e -- --ignored"]
fn wrangler_dry_run_attaches_runtime_wasm_as_compiled_module() {
    let Some(wrangler) = locate_pinned_wrangler() else {
        eprintln!(
            "[wasm_ssr_adapter_e2e] workspace-pinned wrangler is absent; skipping env-gate. \\
             Run pnpm install --frozen-lockfile or set ZFB_WRANGLER_BIN."
        );
        return;
    };
    if !pnpm_available() {
        eprintln!("[wasm_ssr_adapter_e2e] pnpm is not available; skipping wrangler env-gate.");
        return;
    }

    assert_pinned_wrangler_version(&wrangler);
    let adapter = workspace_adapter_package();
    let (_temp, _node_modules, root) = scaffold_fixture(&adapter);
    run_build(&root);
    let runtime_asset = assert_wasm_two_pass_contract(&root);

    // `--no-bundle` makes Wrangler list its module attachments. The normal
    // build assertion above already proves this exact file is imported by the
    // runtime bundle; this handoff verifies Wrangler's default Wasm rule
    // classifies that consumed module as an uploadable compiled-wasm module.
    let outdir = root.join(".wrangler-dry-run");
    let output = Command::new(&wrangler)
        .args(["deploy", "--dry-run", "--no-bundle", "--outdir"])
        .arg(&outdir)
        .current_dir(&root)
        .output()
        .unwrap_or_else(|error| panic!("spawn {} deploy --dry-run: {error}", wrangler.display()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "wrangler deploy --dry-run must accept the adapter output\nstatus: {}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );
    let report = format!("{stdout}\n{stderr}");
    assert!(
        report.contains(&runtime_asset),
        "wrangler's no-bundle deploy report must list the copied runtime Wasm module {runtime_asset:?}\n{report}"
    );
    assert!(
        report.contains("compiled-wasm"),
        "wrangler must classify the runtime Wasm upload as a CompiledWasm module\n{report}"
    );
    assert!(
        outdir.is_dir(),
        "wrangler --outdir must materialize the deploy bundle for inspection"
    );
    assert_eq!(
        fs::read(outdir.join(&runtime_asset)).expect("read Wrangler-emitted runtime Wasm"),
        SSR_WASM,
        "wrangler's dry-run output must retain the exact compiled Wasm upload"
    );
}
