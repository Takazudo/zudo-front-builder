//! Level-3/4 combined proof that zfb builds and serves without esbuild (#4636,
//! esbuild-removal epic #4628).
//!
//! One checked-in project (`tests/fixtures/esbuild-free-combined/`) exercises
//! every JS/TS bundling surface that esbuild used to own and that the embedded
//! native Rolldown backend now owns: a TypeScript `zfb.config.ts`, a
//! TypeScript plugin (virtual module + `postBuild` hook), prerendered SSR
//! pages, a request-time SSR route (dev), a `"use client"` island with a lazy
//! `import()`, a client script, a module worker it registers, a CSS Module, a
//! Wasm import and a `?raw` text resource. The Wasm module is written at test
//! time so no binary file lives in git (shape reused from
//! `wasm_ssr_adapter_e2e.rs`); the lazy island reuses `lazy-shared-island`'s
//! shape, the client script + worker reuse `dev-sibling-watch`'s, and the dev
//! session helpers follow `scanner_boundary_acceptance.rs` and
//! `dev_serve_e2e.rs`.
//!
//! The emitted output is judged by behavior, never by byte equality with an
//! esbuild build: the client script and module worker are executed under Node,
//! the lazy chunk is imported and called under Node, the page markers prove V8
//! evaluated the SSR bundle (Wasm instantiation, config define, plugin virtual
//! module), and the CSS Module's scoped class must appear both in the HTML and
//! in an emitted stylesheet.
//!
//! Every zfb process runs with esbuild made unavailable: each `PATH` entry
//! holding an `esbuild` executable is removed, `ZFB_ESBUILD_BIN` and the
//! retired bundler selectors are unset, and a trap `esbuild` placed first on
//! `PATH` records any invocation and exits non-zero. The trap log must stay
//! absent. The installed-artifact counterpart (packed tarballs, strace) is
//! `scripts/smoke-esbuild-free-artifact.sh`.
//!
//! Failure diagnostics: a syntax error in `pages/index.tsx` and a render-time
//! throw inside `lib/explode.ts` must each fail `zfb build` with a diagnostic
//! naming the original source file and line — the second only through the
//! bundle's source map. The dev session edits a page, introduces a syntax
//! error (the watcher stays alive, reports the original file:line, and keeps
//! serving the last good page), then fixes it and recovers. Waits are keyed on
//! SSE `page` events and served content, never bare sleeps.
//!
//! Not covered here: a request-time SSR route in `zfb build` (that needs an
//! adapter; `wasm_ssr_adapter_e2e` and `workerd_parity_e2e` cover it), and
//! real-browser execution of the island (`tests/router-chromium/`).
//!
//! nextest: `e2e-heavy-locked`; every process-spawning test holds
//! `CrossBinaryE2eLock`.
//! Not `#[ignore]`d; the tests self-skip only when `node` is absent, which the
//! TypeScript plugin host and the execution checks require.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[path = "support/process_guard.rs"]
mod process_guard;

use process_guard::ProcessTreeGuard;
use zfb_test_utils::{
    next_sse_event_name, open_sse, watcher_live_handshake, zfb_binary, CrossBinaryE2eLock,
    HandshakeOpts,
};

const WASM_ANSWER: u8 = 41;
// `answer(): i32` returning WASM_ANSWER (same encoding as wasm_ssr_adapter_e2e).
#[rustfmt::skip]
const ANSWER_WASM: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, 0x03,
    0x02, 0x01, 0x00, 0x07, 0x0a, 0x01, 0x06, 0x61, 0x6e, 0x73, 0x77, 0x65, 0x72, 0x00, 0x00, 0x0a,
    0x06, 0x01, 0x04, 0x00, 0x41, WASM_ANSWER, 0x0b,
];

const PAGE_V1: &str = "ZFB_COMBINED_PAGE:v1";
const CONFIG_MARKER: &str = "ZFB_TS_CONFIG:combined";
const PLUGIN_STAMP: &str = "ZFB_TS_PLUGIN_STAMP";
const RAW_MARKER: &str = "ZFB_RAW_TEXT_RESOURCE";
const CARD_MARKER: &str = "ZFB_CSS_MODULE_CARD";
const CLIENT_MARKER: &str = "ZFB_COMBINED_CLIENT_SCRIPT";
const WORKER_MARKER: &str = "ZFB_COMBINED_WORKER";
const LAZY_MARKER: &str = "ZFB_LAZY_PANEL_CHUNK";
const SSR_MARKER: &str = "ZFB_REQUEST_SSR:";

const TRAP_LOG: &str = "esbuild-trap.log";
const TICK_FAILURE_PREFIX: &str = "[zfb] rebuild tick failed; watcher staying alive:";

const BOOT_DEADLINE: Duration = Duration::from_secs(120);
const SCENARIO_DEADLINE: Duration = Duration::from_secs(90);
const SSE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/esbuild-free-combined")
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn require_node(test: &str) -> bool {
    if node_available() {
        return true;
    }
    eprintln!("[esbuild_free_combined_e2e] {test}: node is not on PATH; skipping");
    false
}

fn copy_dir(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap_or_else(|e| panic!("create {}: {e}", target.display()));
    for entry in fs::read_dir(source).unwrap_or_else(|e| panic!("read {}: {e}", source.display())) {
        let entry = entry.expect("fixture entry");
        let from = entry.path();
        let to = target.join(entry.file_name());
        if entry.file_type().expect("fixture file type").is_dir() {
            copy_dir(&from, &to);
        } else {
            fs::copy(&from, to).unwrap_or_else(|e| panic!("copy {}: {e}", from.display()));
        }
    }
}

fn write(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(&path, body).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// A fresh copy of the fixture plus the test-time Wasm input.
struct Project {
    _temp: tempfile::TempDir,
    root: PathBuf,
    trap_dir: PathBuf,
}

impl Project {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("create combined fixture tempdir");
        let root = temp.path().join("app");
        copy_dir(&fixture_dir(), &root);
        fs::create_dir_all(root.join("wasm")).expect("create wasm directory");
        fs::write(root.join("wasm/answer.wasm"), ANSWER_WASM).expect("write Wasm input");
        let trap_dir = temp.path().join("esbuild-trap");
        install_esbuild_trap(&trap_dir);
        Project {
            _temp: temp,
            root,
            trap_dir,
        }
    }

    fn page_source(&self) -> String {
        read_text(&self.root.join("pages/index.tsx"))
    }

    fn trap_log(&self) -> PathBuf {
        self.trap_dir.join(TRAP_LOG)
    }

    fn assert_esbuild_never_invoked(&self, context: &str) {
        let log = self.trap_log();
        assert!(
            !log.exists(),
            "{context}: zfb invoked an `esbuild` executable from PATH:\n{}",
            fs::read_to_string(log).unwrap_or_default()
        );
    }

    /// `zfb` with esbuild unavailable: no PATH entry holds a real `esbuild`,
    /// the trap shadows any lookup, and the retired selectors are unset.
    fn zfb(&self) -> Command {
        let mut command = Command::new(zfb_binary!());
        command
            .current_dir(&self.root)
            .env("PATH", esbuild_free_path(&self.trap_dir))
            .env_remove("ZFB_ESBUILD_BIN")
            .env_remove("ZFB_BUNDLER")
            .env_remove("ZFB_ROLLDOWN_PROTOTYPE");
        command
    }

    fn build(&self) -> Output {
        self.zfb()
            .arg("build")
            .output()
            .expect("spawn real `zfb build`")
    }
}

/// A trap `esbuild` that records its arguments and fails loudly.
fn install_esbuild_trap(dir: &Path) {
    fs::create_dir_all(dir).expect("create esbuild trap directory");
    let trap = dir.join("esbuild");
    let log = dir.join(TRAP_LOG);
    fs::write(
        &trap,
        format!(
            "#!/bin/sh\nprintf 'esbuild invoked: %s\\n' \"$*\" >> '{}'\nexit 97\n",
            log.display()
        ),
    )
    .expect("write esbuild trap");
    fs::set_permissions(trap, fs::Permissions::from_mode(0o755)).expect("chmod esbuild trap");
}

/// The inherited `PATH` minus every directory holding an `esbuild`, behind
/// the trap. A dropped directory's `node` is linked into the trap directory
/// so the plugin host still finds it.
fn esbuild_free_path(trap_dir: &Path) -> std::ffi::OsString {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let mut kept = Vec::new();
    for dir in std::env::split_paths(&inherited) {
        if !dir.join("esbuild").exists() {
            kept.push(dir);
            continue;
        }
        let node = dir.join("node");
        let linked = trap_dir.join("node");
        if node.is_file() && !linked.exists() {
            std::os::unix::fs::symlink(node, linked).expect("link node beside the trap");
        }
    }
    std::env::join_paths(std::iter::once(trap_dir.to_path_buf()).chain(kept))
        .expect("join esbuild-free PATH")
}

fn dump(output: &Output) -> String {
    format!(
        "status={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("dist entry").path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn files_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files(dir, &mut files);
    files.retain(|path| path.extension().is_some_and(|ext| ext == extension));
    files.sort();
    files
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .expect("UTF-8 file name")
}

/// Every `src="..."` value in `html`.
fn script_sources(html: &str) -> Vec<&str> {
    html.split("src=\"")
        .skip(1)
        .filter_map(|tail| tail.split('"').next())
        .collect()
}

/// The `class` value of the element that also carries `data_attr`.
fn class_of_element_with(html: &str, data_attr: &str) -> String {
    let at = html
        .find(data_attr)
        .unwrap_or_else(|| panic!("no element with {data_attr} in:\n{html}"));
    let open = html[..at].rfind('<').expect("element start");
    let close = at + html[at..].find('>').expect("element end");
    let tag = &html[open..close];
    tag.split("class=\"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .unwrap_or_else(|| panic!("element {tag:?} has no class attribute"))
        .to_owned()
}

/// Runs a small ES module under Node and returns its stdout.
fn run_node_module(scratch: &Path, label: &str, source: &str, arg: &Path) -> String {
    fs::create_dir_all(scratch).expect("create node scratch dir");
    let runner = scratch.join(format!("{label}.mjs"));
    fs::write(&runner, source).expect("write node runner");
    let output = Command::new("node")
        .arg(runner)
        .arg(arg)
        .output()
        .expect("spawn node");
    assert!(
        output.status.success(),
        "{label}: executing emitted {} under node failed\n{}",
        arg.display(),
        dump(&output)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// 1-based line of the first line containing `needle`.
fn line_of(source: &str, needle: &str) -> usize {
    source
        .lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not in source:\n{source}"))
        + 1
}

fn contains_location(text: &str, relative: &str, line: usize) -> bool {
    let native = relative.replace('/', std::path::MAIN_SEPARATOR_STR);
    [relative.to_owned(), native].iter().any(|path| {
        text.contains(&format!("{path}:{line}:")) || text.contains(&format!("{path}:{line})"))
    })
}

#[test]
fn version_names_the_embedded_rolldown_backend() {
    let project = Project::new();
    let output = project
        .zfb()
        .arg("--version")
        .output()
        .expect("spawn `zfb --version`");
    assert!(output.status.success(), "{}", dump(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("embedded bundler: rolldown-"),
        "`zfb --version` must name the embedded Rolldown backend\n{}",
        dump(&output)
    );
    project.assert_esbuild_never_invoked("zfb --version");
}

#[test]
fn build_bundles_every_surface_natively_and_the_output_runs() {
    if !require_node("build") {
        return;
    }
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let project = Project::new();
    let output = project.build();
    assert!(
        output.status.success(),
        "esbuild-free `zfb build` must succeed\n{}",
        dump(&output)
    );
    project.assert_esbuild_never_invoked("zfb build");

    let root = &project.root;
    let dist = root.join("dist");
    let html = read_text(&dist.join("index.html"));

    // SSR (prerender) in V8: TS config define, TS plugin virtual module, Wasm
    // instantiation and the `?raw` resource all reach the rendered HTML.
    for (needle, what) in [
        (PAGE_V1, "prerendered page body"),
        (CONFIG_MARKER, "zfb.config.ts define"),
        (PLUGIN_STAMP, "TS plugin virtual module"),
        (RAW_MARKER, "`?raw` text resource"),
        (CARD_MARKER, "CSS Module component"),
        ("data-lazy-host", "island SSR"),
    ] {
        assert!(
            html.contains(needle),
            "{what}: {needle:?} missing from index.html:\n{html}"
        );
    }
    let wasm_marker = format!("ZFB_WASM_ANSWER:{WASM_ANSWER}");
    assert!(
        html.contains(&wasm_marker),
        "the page must instantiate the Wasm module during SSR ({wasm_marker}):\n{html}"
    );
    assert!(
        !html.contains("__COMBINED_CONFIG_MARKER__"),
        "the config define must be substituted:\n{html}"
    );

    // TS plugin postBuild receipt.
    let receipt: serde_json::Value =
        serde_json::from_str(&read_text(&dist.join("ts-plugin-postbuild.json")))
            .expect("TS plugin postBuild receipt is JSON");
    assert_eq!(receipt["stamp"], PLUGIN_STAMP, "{receipt}");
    assert!(
        receipt["paths"]
            .as_array()
            .is_some_and(|paths| paths.iter().any(|path| path == "index.html")),
        "postBuild must see the emitted index.html route: {receipt}"
    );

    // CSS Module: a scoped class in the HTML and the same selector in CSS.
    let card_class = class_of_element_with(&html, "data-card");
    assert!(
        card_class.contains("combinedCard") && card_class != "combinedCard",
        "the CSS Module class must be scoped, got {card_class:?}"
    );
    let stylesheets = files_with_extension(&dist, "css");
    assert!(
        stylesheets
            .iter()
            .any(|css| read_text(css).contains(&format!(".{card_class}"))),
        "no emitted stylesheet defines .{card_class}: {stylesheets:#?}"
    );

    // Client script + module worker, executed under Node.
    let sources = script_sources(&html);
    let client_src = sources
        .iter()
        .find(|src| src.starts_with("/assets/client/combined") && src.ends_with(".js"))
        .unwrap_or_else(|| panic!("no client script tag in index.html: {sources:?}\n{html}"));
    let client_entry = dist.join(client_src.trim_start_matches('/'));
    let client_body = read_text(&client_entry);
    let scratch = root.join(".node-runners");
    let client_stdout = run_node_module(
        &scratch,
        "client-script",
        "import { pathToFileURL } from 'node:url';\n\
         console.info = (...args) => console.log(args.join(' '));\n\
         await import(pathToFileURL(process.argv[2]).href);\n",
        &client_entry,
    );
    assert!(
        client_stdout.contains(CLIENT_MARKER),
        "the emitted client script must run and log {CLIENT_MARKER}: {client_stdout}"
    );
    let worker = files_with_extension(&dist.join("assets/client"), "js")
        .into_iter()
        .find(|path| path != &client_entry && read_text(path).contains(WORKER_MARKER))
        .unwrap_or_else(|| panic!("no emitted module worker containing {WORKER_MARKER}"));
    assert!(
        client_body.contains(file_name(&worker)),
        "the client script must reference its emitted worker {}:\n{client_body}",
        file_name(&worker)
    );
    assert!(
        !client_body.contains("combined.worker.ts"),
        "the worker URL must point at emitted JS, not the TS source:\n{client_body}"
    );
    let worker_stdout = run_node_module(
        &scratch,
        "module-worker",
        "import { pathToFileURL } from 'node:url';\n\
         globalThis.self = { postMessage: (message) => console.log('POSTED ' + JSON.stringify(message)) };\n\
         await import(pathToFileURL(process.argv[2]).href);\n",
        &worker,
    );
    assert!(
        worker_stdout.contains(&format!("POSTED [\"{WORKER_MARKER}\",6]")),
        "the emitted module worker must run and post its message: {worker_stdout}"
    );

    // Island with a lazy import(): a separate chunk, referenced by a dynamic
    // import, that runs and shares state with the eager island module.
    let islands_src = sources
        .iter()
        .find(|src| src.starts_with("/assets/islands-") && src.ends_with(".js"))
        .unwrap_or_else(|| panic!("no islands entry script in index.html: {sources:?}"));
    let islands_entry = dist.join(islands_src.trim_start_matches('/'));
    let browser_modules = files_with_extension(&dist.join("assets"), "js");
    let lazy_chunk = browser_modules
        .iter()
        .find(|path| *path != &islands_entry && read_text(path).contains(LAZY_MARKER))
        .unwrap_or_else(|| {
            panic!("the lazy import() must emit its own chunk containing {LAZY_MARKER}: {browser_modules:#?}")
        });
    assert!(
        !read_text(&islands_entry).contains(LAZY_MARKER),
        "the lazily imported panel must not be inlined into the islands entry"
    );
    let lazy_name = file_name(lazy_chunk);
    assert!(
        browser_modules.iter().any(|path| {
            let body = read_text(path);
            path != lazy_chunk && body.contains("import(") && body.contains(lazy_name)
        }),
        "no emitted module dynamically imports {lazy_name}"
    );
    let lazy_stdout = run_node_module(
        &scratch,
        "lazy-chunk",
        "import { pathToFileURL } from 'node:url';\n\
         const chunk = await import(pathToFileURL(process.argv[2]).href);\n\
         for (const value of Object.values(chunk)) {\n\
           if (typeof value === 'function' && value.length === 0) {\n\
             const result = value();\n\
             if (typeof result === 'string') console.log('RESULT ' + result);\n\
           }\n\
         }\n",
        lazy_chunk,
    );
    assert!(
        lazy_stdout.contains(&format!("RESULT {LAZY_MARKER} owner=")),
        "the emitted lazy chunk must run and describe the shared state: {lazy_stdout}"
    );

    // No emitted browser module may carry an untransformed `?raw` request.
    for path in browser_modules {
        assert!(
            !read_text(&path).contains("?raw"),
            "{} still carries an unbundled `?raw` request",
            path.display()
        );
    }
}

#[test]
fn build_failures_point_at_the_original_typescript_source() {
    if !require_node("build failures") {
        return;
    }
    let _e2e_lock = CrossBinaryE2eLock::acquire();

    // A syntax error is reported against the original page source.
    let syntax = Project::new();
    let broken = syntax.page_source().replace(
        "<p id=\"page\">ZFB_COMBINED_PAGE:v1</p>",
        "<p id=\"page\">ZFB_COMBINED_PAGE:v1</p>\n          {const broken: = 1}",
    );
    write(&syntax.root, "pages/index.tsx", &broken);
    let broken_line = line_of(&broken, "const broken: = 1");
    let output = syntax.build();
    assert!(
        !output.status.success(),
        "a syntax error must fail `zfb build`\n{}",
        dump(&output)
    );
    let text = dump(&output);
    assert!(
        contains_location(&text, "pages/index.tsx", broken_line),
        "the syntax diagnostic must point at pages/index.tsx:{broken_line}\n{text}"
    );
    syntax.assert_esbuild_never_invoked("zfb build (syntax error)");

    // A render-time throw in a TS helper is mapped back through the bundle's
    // source map to the helper's own line.
    let runtime = Project::new();
    let helper = "// Thrown while V8 renders the page; the frame must map to this file.\n\
                  type Reason = \"render\";\n\
                  export function explode(reason: Reason): string {\n\
                  \x20 throw new Error(`ZFB_COMBINED_RENDER_THROW:${reason}`);\n\
                  }\n";
    write(&runtime.root, "lib/explode.ts", helper);
    let throw_line = line_of(helper, "throw new Error");
    let page = runtime
        .page_source()
        .replace(
            "import { LazyHost } from \"../components/lazy-host\";",
            "import { LazyHost } from \"../components/lazy-host\";\nimport { explode } from \"../lib/explode\";",
        )
        .replace(
            "<p id=\"page\">ZFB_COMBINED_PAGE:v1</p>",
            "<p id=\"page\">{explode(\"render\")}</p>",
        );
    write(&runtime.root, "pages/index.tsx", &page);
    let output = runtime.build();
    assert!(
        !output.status.success(),
        "a render-time throw must fail `zfb build`\n{}",
        dump(&output)
    );
    let text = dump(&output);
    assert!(
        text.contains("ZFB_COMBINED_RENDER_THROW:render"),
        "the thrown message must be reported\n{text}"
    );
    assert!(
        contains_location(&text, "lib/explode.ts", throw_line),
        "the render error must be source-mapped to lib/explode.ts:{throw_line}\n{text}"
    );
    runtime.assert_esbuild_never_invoked("zfb build (render throw)");
}

struct DevSession {
    tree: ProcessTreeGuard,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl DevSession {
    fn spawn(project: &Project) -> Self {
        let stdout_path = project.root.join(".zfb-dev-stdout.log");
        let stderr_path = project.root.join(".zfb-dev-stderr.log");
        let stdout = fs::File::create(&stdout_path).expect("create dev stdout log");
        let stderr = fs::File::create(&stderr_path).expect("create dev stderr log");
        let mut command = project.zfb();
        command
            .args(["dev", "--port", "0"])
            // Each edit must complete (or fail) its tick before the
            // assertions below, as in scanner_boundary_acceptance.rs.
            .env("ZFB_DEV_EAGER", "1")
            .env("ZFB_DEV_BOOT_LAZY", "0")
            .env_remove("ZFB_LAZY_DEV_RENDER")
            .env_remove("ZFB_DEV_DEFER_BUNDLE")
            .env_remove("ZFB_DEV_TIMING")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let tree = ProcessTreeGuard::spawn(&mut command).expect("spawn zfb dev");
        DevSession {
            tree,
            stdout_path,
            stderr_path,
        }
    }

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

    fn assert_alive(&mut self, phase: &str) {
        if let Some(status) = self.tree.try_wait().expect("try_wait on zfb dev") {
            panic!("{phase}: zfb dev exited ({status:?})\n{}", self.logs());
        }
    }
}

fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(index) = rest.find("http://") {
        let candidate = &rest[index + "http://".len()..];
        let authority = candidate.split_whitespace().next().unwrap_or_default();
        if let Some(colon) = authority.rfind(':') {
            let digits: String = authority[colon + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
        rest = candidate;
    }
    None
}

async fn wait_for_ready(session: &mut DevSession) -> String {
    let started = Instant::now();
    loop {
        session.assert_alive("boot");
        if let Some(port) = parse_ready_port(&session.stdout()) {
            assert_ne!(port, 0, "ready banner reported port 0\n{}", session.logs());
            return format!("http://localhost:{port}");
        }
        assert!(
            started.elapsed() < BOOT_DEADLINE,
            "zfb dev was not ready within {}s\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Polls `url` until a 200 body contains `needle`, returning that body.
async fn wait_for_body(session: &mut DevSession, url: &str, needle: &str, phase: &str) -> String {
    let started = Instant::now();
    let mut last = String::from("no response");
    loop {
        session.assert_alive(phase);
        match reqwest::get(url).await {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                if status.is_success() && body.contains(needle) {
                    return body;
                }
                last = format!("HTTP {status}: {body}");
            }
            Err(error) => last = error.to_string(),
        }
        assert!(
            started.elapsed() < SCENARIO_DEADLINE,
            "{phase}: {url} did not serve {needle:?} within {}s; last: {last}\n{}",
            SCENARIO_DEADLINE.as_secs(),
            session.logs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn wait_for_stderr(
    session: &mut DevSession,
    offset: usize,
    needle: &str,
    phase: &str,
) -> String {
    let started = Instant::now();
    loop {
        session.assert_alive(phase);
        let stderr = session.stderr();
        let window = stderr.get(offset..).unwrap_or_default().to_owned();
        if window.contains(needle) {
            return window;
        }
        assert!(
            started.elapsed() < SCENARIO_DEADLINE,
            "{phase}: zfb dev did not log {needle:?} within {}s\n{}",
            SCENARIO_DEADLINE.as_secs(),
            session.logs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn subscribe(base: &str) -> reqwest::Response {
    let response = open_sse(base).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "SSE endpoint must answer 200"
    );
    response
}

async fn expect_page_event(sse: reqwest::Response, session: &DevSession, phase: &str) {
    let event = next_sse_event_name(sse, SSE_DEADLINE)
        .await
        .expect("read SSE stream");
    assert_eq!(
        event.as_deref(),
        Some("page"),
        "{phase}: the edit must broadcast an SSE `page` event\n{}",
        session.logs()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_serves_every_surface_and_recovers_from_a_syntax_error() {
    if !require_node("dev") {
        return;
    }
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let project = Project::new();
    copy_dir(&project.root.join("dev-ssr"), &project.root.join("pages"));
    let mut session = DevSession::spawn(&project);
    let base = wait_for_ready(&mut session).await;

    let html = wait_for_body(&mut session, &format!("{base}/"), PAGE_V1, "boot").await;
    let wasm_marker = format!("ZFB_WASM_ANSWER:{WASM_ANSWER}");
    for needle in [
        CONFIG_MARKER,
        PLUGIN_STAMP,
        RAW_MARKER,
        CARD_MARKER,
        wasm_marker.as_str(),
    ] {
        assert!(
            html.contains(needle),
            "dev SSR is missing {needle:?}:\n{html}"
        );
    }
    wait_for_body(
        &mut session,
        &format!("{base}/items/alpha/"),
        &format!("{SSR_MARKER}alpha"),
        "request-time SSR",
    )
    .await;
    let client_src = script_sources(&html)
        .into_iter()
        .find(|src| src.starts_with("/assets/client/combined") && src.ends_with(".js"))
        .unwrap_or_else(|| panic!("no dev client script tag:\n{html}"))
        .to_owned();
    wait_for_body(
        &mut session,
        &format!("{base}{client_src}"),
        CLIENT_MARKER,
        "dev client script",
    )
    .await;

    // Prove the watcher is past its startup window before the real edits.
    let page_seen = Arc::new(AtomicBool::new(false));
    {
        let sse = subscribe(&base).await;
        let seen = Arc::clone(&page_seen);
        tokio::spawn(async move {
            if let Ok(Some(name)) = next_sse_event_name(sse, BOOT_DEADLINE).await {
                if name == "page" {
                    seen.store(true, Ordering::SeqCst);
                }
            }
        });
    }
    let warmup_root = project.root.clone();
    let handshake = watcher_live_handshake(
        HandshakeOpts::new(SSE_DEADLINE),
        |index| {
            write(
                &warmup_root,
                &format!("pages/warmup-{index}.tsx"),
                "export default function Warmup() { return <html><body>warmup</body></html>; }\n",
            )
        },
        || page_seen.load(Ordering::SeqCst),
    )
    .await;
    assert!(
        handshake.live,
        "watcher never reported a warmup page within {:?} ({} markers)\n{}",
        handshake.elapsed,
        handshake.markers_written,
        session.logs()
    );

    // 1. Edit: the new page body is served.
    let original = project.page_source();
    let sse = subscribe(&base).await;
    write(
        &project.root,
        "pages/index.tsx",
        &original.replace(PAGE_V1, "ZFB_COMBINED_PAGE:v2"),
    );
    expect_page_event(sse, &session, "edit").await;
    let edited = wait_for_body(
        &mut session,
        &format!("{base}/"),
        "ZFB_COMBINED_PAGE:v2",
        "edit",
    )
    .await;
    assert!(
        edited.contains(PLUGIN_STAMP) && edited.contains(&wasm_marker),
        "{edited}"
    );

    // 2. Failure: the watcher stays alive, names the original file:line and
    //    keeps serving the last good page.
    let stderr_offset = session.stderr().len();
    let broken = original.replace(
        &format!("<p id=\"page\">{PAGE_V1}</p>"),
        "<p id=\"page\">ZFB_COMBINED_PAGE:broken</p>\n          {const broken: = 1}",
    );
    let broken_line = line_of(&broken, "const broken: = 1");
    write(&project.root, "pages/index.tsx", &broken);
    let window = wait_for_stderr(&mut session, stderr_offset, TICK_FAILURE_PREFIX, "failure").await;
    assert!(
        contains_location(&window, "pages/index.tsx", broken_line),
        "the dev failure must point at pages/index.tsx:{broken_line}:\n{window}"
    );
    let last_good = wait_for_body(
        &mut session,
        &format!("{base}/"),
        "ZFB_COMBINED_PAGE:v2",
        "failure",
    )
    .await;
    assert!(
        !last_good.contains("ZFB_COMBINED_PAGE:broken"),
        "{last_good}"
    );

    // 3. Recovery: fixing the source publishes the new page again.
    let sse = subscribe(&base).await;
    write(
        &project.root,
        "pages/index.tsx",
        &original.replace(PAGE_V1, "ZFB_COMBINED_PAGE:v3"),
    );
    expect_page_event(sse, &session, "recovery").await;
    let recovered = wait_for_body(
        &mut session,
        &format!("{base}/"),
        "ZFB_COMBINED_PAGE:v3",
        "recovery",
    )
    .await;
    for needle in [
        CONFIG_MARKER,
        PLUGIN_STAMP,
        CARD_MARKER,
        wasm_marker.as_str(),
    ] {
        assert!(
            recovered.contains(needle),
            "recovered page is missing {needle:?}:\n{recovered}"
        );
    }

    project.assert_esbuild_never_invoked("zfb dev");
}
