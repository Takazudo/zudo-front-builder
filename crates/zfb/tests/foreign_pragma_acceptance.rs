//! Real-binary acceptance for foreign `@jsxImportSource` pragma warnings (#3514).
//!
//! A consumer project gets a real `node_modules`: the binary's embedded
//! `@takazudo` packages plus, where a case needs them, `npm pack` tarballs of
//! this monorepo's installed Preact and of a fixture widget package. The matrix:
//!
//! - A: a leftover Preact pragma with Preact absent warns, naming the authored
//!   file and position, before esbuild's unresolved `preact/jsx-runtime`.
//! - B: the same pragma with real Preact installed warns before the render
//!   failure.
//! - C: the pragma changed to the owned runtime builds and renders, silently.
//! - D: a Preact widget package (its own `.jsx` source carries a Preact
//!   pragma) lazily mounted behind a DOM host from an island builds without
//!   any pragma warning; dependencies are never scanned.
//! - Dev: the same warning reaches `zfb dev` output when an edit introduces
//!   the pragma.
//!
//! Every case spawns `zfb build` or `zfb dev`, so the binary holds the
//! cross-binary flock and sits in nextest's `e2e-heavy-locked` group.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

const WARNING_PREFIX: &str = "zfb warn: components/search-box.tsx:";
const ADVICE: &str =
    "Remove the pragma, or change it to `@jsxImportSource @takazudo/zfb/zudo-react`";

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/foreign-pragma-acceptance")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn replace_in(root: &Path, file: &str, from: &str, to: &str) {
    let path = root.join(file);
    let source = fs::read_to_string(&path).unwrap();
    assert!(source.contains(from), "{file} lacks {from:?}");
    fs::write(&path, source.replacen(from, to, 1)).unwrap();
}

/// This monorepo's own installed Preact (`docs/` depends on it).
fn workspace_preact() -> PathBuf {
    let pnpm = repo_root().join("node_modules/.pnpm");
    let mut candidates: Vec<PathBuf> = fs::read_dir(&pnpm)
        .unwrap_or_else(|error| panic!("read {}: {error}", pnpm.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("preact@"))
        })
        .collect();
    candidates.sort();
    candidates
        .pop()
        .unwrap_or_else(|| {
            panic!(
                "no preact@ entry under {}; run pnpm install",
                pnpm.display()
            )
        })
        .join("node_modules/preact")
}

fn npm_pack(package_dir: &Path, destination: &Path) -> PathBuf {
    fs::create_dir_all(destination).unwrap();
    let before: Vec<_> = fs::read_dir(destination)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    // Some npm versions run `prepare` (Preact's needs husky) even with
    // --ignore-scripts, so pack a copy whose manifest declares no scripts.
    let staging = tempfile::tempdir().unwrap();
    let staged = staging.path().join("package");
    copy_dir(package_dir, &staged);
    let manifest_path = staged.join("package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest.as_object_mut().unwrap().remove("scripts");
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let packed = Command::new("npm")
        .args(["pack", "--ignore-scripts", "--pack-destination"])
        .arg(destination)
        .current_dir(&staged)
        .output()
        .expect("spawn npm pack");
    assert!(
        packed.status.success(),
        "npm pack {} failed:\n{}",
        package_dir.display(),
        combined(&packed)
    );
    fs::read_dir(destination)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| !before.contains(path) && path.extension().is_some_and(|ext| ext == "tgz"))
        .expect("npm pack created a tarball")
}

/// A consumer copy of the fixture. With `with_preact`, real Preact and the
/// widget package are installed from tarballs before the embedded `@takazudo`
/// packages are copied in (npm prunes extraneous packages it did not install).
fn consumer(with_preact: bool) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    copy_dir(&fixture_root(), &root);
    if with_preact {
        let tarballs = root.join(".tarballs");
        let preact = npm_pack(&workspace_preact(), &tarballs);
        let widget = npm_pack(&root.join("package-source/legacy-widget"), &tarballs);
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
            .arg(&preact)
            .arg(&widget)
            .current_dir(&root)
            .output()
            .expect("spawn npm install");
        assert!(
            installed.status.success(),
            "npm install of the packed fixtures failed:\n{}",
            combined(&installed)
        );
        assert!(root
            .join("node_modules/preact/jsx-runtime/package.json")
            .is_file());
        assert!(root
            .join("node_modules/@fixture/legacy-widget/index.jsx")
            .is_file());
    }
    let (_lease, embedded) =
        zfb::render_pipeline::embedded_node_modules().expect("extract embedded @takazudo packages");
    copy_dir(&embedded, &root.join("node_modules"));
    (temp, root)
}

fn combined(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn build(root: &Path, esbuild: &Path) -> Output {
    Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn zfb build")
}

fn tools() -> Option<PathBuf> {
    let npm = Command::new("npm")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    match (locate_esbuild(), npm) {
        (Some(esbuild), true) => Some(esbuild),
        _ => {
            eprintln!("[foreign_pragma_acceptance] esbuild or npm unavailable; skipping");
            None
        }
    }
}

/// The pragma sits on line 3, indented, so a reported position proves the
/// authored file was read rather than a staged copy or the bundle.
fn add_preact_pragma(root: &Path) {
    replace_in(
        root,
        "components/search-box.tsx",
        "export function SearchBox",
        "// Leftover from the Preact era.\n\n  /** @jsxImportSource preact */\nexport function SearchBox",
    );
}

const PREACT_WARNING: &str = "zfb warn: components/search-box.tsx:3:7: per-file `@jsxImportSource preact` pragma overrides the project's JSX import source";

#[test]
fn case_a_missing_preact_warns_before_the_unresolved_runtime() {
    let _lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = tools() else { return };
    let (_temp, root) = consumer(false);
    add_preact_pragma(&root);
    let output = build(&root, &esbuild);
    let text = combined(&output);
    assert!(!output.status.success(), "{text}");
    let warning = text
        .find(PREACT_WARNING)
        .unwrap_or_else(|| panic!("{text}"));
    let unresolved = text
        .find("Could not resolve \"preact/jsx-runtime\"")
        .unwrap_or_else(|| panic!("{text}"));
    assert!(warning < unresolved, "{text}");
    assert!(text.contains(ADVICE), "{text}");
    assert_eq!(text.matches(WARNING_PREFIX).count(), 1, "{text}");
    for staging in ["zfb-bundler-", "zfb-shadow-session-"] {
        assert!(!text.contains(staging), "{text}");
    }
}

#[test]
fn case_b_installed_preact_warns_before_the_render_failure() {
    let _lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = tools() else { return };
    let (_temp, root) = consumer(true);
    add_preact_pragma(&root);
    let output = build(&root, &esbuild);
    let text = combined(&output);
    assert!(!output.status.success(), "{text}");
    let warning = text
        .find(PREACT_WARNING)
        .unwrap_or_else(|| panic!("{text}"));
    let render = text.find("ZR_").unwrap_or_else(|| panic!("{text}"));
    assert!(warning < render, "{text}");
    assert!(!text.contains("Could not resolve"), "{text}");
    assert_eq!(text.matches(WARNING_PREFIX).count(), 1, "{text}");
}

#[test]
fn case_c_owned_pragma_renders_without_a_warning() {
    let _lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = tools() else { return };
    let (_temp, root) = consumer(true);
    replace_in(
        &root,
        "components/search-box.tsx",
        "export function SearchBox",
        "/** @jsxImportSource @takazudo/zfb/zudo-react */\nexport function SearchBox",
    );
    let output = build(&root, &esbuild);
    let text = combined(&output);
    assert!(output.status.success(), "{text}");
    assert!(!text.contains("@jsxImportSource"), "{text}");
    let html = fs::read_to_string(root.join("dist/index.html")).unwrap();
    assert!(html.contains("owned search box"), "{html}");
}

#[test]
fn case_d_lazy_preact_widget_behind_a_dom_host_builds_without_a_warning() {
    let _lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = tools() else { return };
    let (_temp, root) = consumer(true);
    fs::write(
        root.join("pages/index.tsx"),
        r#"import { Island } from "@takazudo/zfb";
import { SearchBox } from "../components/search-box";
import { ThirdPartyWidget } from "../components/third-party-widget";

export default function Home() {
  return (
    <html lang="en">
      <head>
        <title>Foreign pragma acceptance</title>
      </head>
      <body>
        <SearchBox />
        <Island when="load">
          <ThirdPartyWidget />
        </Island>
      </body>
    </html>
  );
}
"#,
    )
    .unwrap();
    let output = build(&root, &esbuild);
    let text = combined(&output);
    assert!(output.status.success(), "{text}");
    assert!(!text.contains("@jsxImportSource"), "{text}");
    assert!(!text.contains("legacy-widget"), "{text}");
    let html = fs::read_to_string(root.join("dist/index.html")).unwrap();
    assert!(
        html.contains("data-zfb-island=\"ThirdPartyWidget\""),
        "{html}"
    );
    assert!(html.contains("owned search box"), "{html}");
    let assets: String = walk(&root.join("dist"))
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "js"))
        .map(|path| fs::read_to_string(path).unwrap_or_default())
        .collect();
    assert!(
        assets.contains("legacy preact widget"),
        "widget code missing from client assets"
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

struct DevServer {
    child: Child,
    stdout: PathBuf,
    stderr: PathBuf,
}

impl DevServer {
    fn logs(&self) -> String {
        format!(
            "{}\n{}",
            fs::read_to_string(&self.stdout).unwrap_or_default(),
            fs::read_to_string(&self.stderr).unwrap_or_default()
        )
    }

    fn wait_for(&mut self, needle: &str) -> String {
        let started = Instant::now();
        loop {
            let logs = self.logs();
            if logs.contains(needle) {
                return logs;
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("zfb dev exited ({status:?}) before {needle:?}\n{logs}");
            }
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "zfb dev did not log {needle:?}\n{logs}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for DevServer {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

#[test]
fn dev_reports_a_pragma_introduced_by_an_edit() {
    let _lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = tools() else { return };
    let (_temp, root) = consumer(false);
    let stdout = root.join(".zfb-dev-stdout.log");
    let stderr = root.join(".zfb-dev-stderr.log");
    let mut command = Command::new(zfb_binary!());
    command
        .args(["dev", "--port", "0"])
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .env("ZFB_DEV_EAGER", "1")
        .env("ZFB_DEV_BOOT_LAZY", "0")
        .stdout(Stdio::from(fs::File::create(&stdout).unwrap()))
        .stderr(Stdio::from(fs::File::create(&stderr).unwrap()));
    command.process_group(0);
    let mut server = DevServer {
        child: command.spawn().expect("spawn zfb dev"),
        stdout,
        stderr,
    };
    server.wait_for("http://");
    assert!(
        !server.logs().contains("@jsxImportSource"),
        "{}",
        server.logs()
    );

    add_preact_pragma(&root);
    let logs = server.wait_for(PREACT_WARNING);
    assert!(logs.contains(ADVICE), "{logs}");
}
