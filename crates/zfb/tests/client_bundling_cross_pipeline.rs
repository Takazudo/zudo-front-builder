//! Cross-pipeline acceptance gate for the client-bundling epic (#1497 / #1504).
//!
//! This is intentionally one env-gated, real-binary test. It runs both
//! `zfb build` and `zfb dev --port 0` over the same checked-in fixture and
//! verifies the public contract at the emitted-file / HTTP boundary:
//!
//! - a `"use client"` island and a discovered `*.client.ts` entry both bundle;
//! - terminal `?raw` imports and a configured inline loader are inlined;
//! - configured defines and the three reserved mode defines are substituted;
//! - production folds the DEV-only branches while development retains them;
//! - direct and nested module workers are emitted in the correct namespace,
//!   with the parent URLs carrying their graph-version query;
//! - the same sources pass through SSR without unresolved browser-bundle
//!   syntax leaking into the server bundle.
//!
//! The test is ignored by default because it boots embedded V8 and several
//! real esbuild subprocesses. `health.yml` runs it with the pinned workspace
//! esbuild slot, and `.config/nextest.toml` serializes the test binary with the
//! other heavy zfb process-spawning binaries.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, next_sse_event_name, zfb_binary, CrossBinaryE2eLock};

const BOOT_DEADLINE: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SSE_DEADLINE: Duration = Duration::from_secs(30);
const CLIENT_RELATIVE_RAW_PATH: &str = "components/cross-pipeline-client/demo.frag";
const CLIENT_ALIAS_RAW_PATH: &str = "components/cross-pipeline-client/alias.frag";
const SUB_PACKAGES_DIR: &str = "sub-packages";
const UPDATED_ALIAS_RAW_TEXT: &str = "\
ZFB_CLIENT_ALIAS_RAW_PAYLOAD_V2
alias raw payload refreshed through tsconfig paths
";

static SERIAL: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("client-bundling-cross-pipeline")
}

fn copy_fixture(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some("node_modules" | "dist" | ".zfb-build")) {
            continue;
        }
        let target = dst.join(&name);
        if entry.file_type()?.is_dir() {
            copy_fixture(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn raw_text_forms(text: &str) -> [String; 2] {
    [
        text.to_string(),
        serde_json::to_string(text).expect("serialize raw payload as a JavaScript string literal"),
    ]
}

fn assert_embeds_raw_text(body: &str, expected: &str, label: &str) {
    let forms = raw_text_forms(expected);
    assert!(
        forms.iter().any(|form| body.contains(form)),
        "{label} must embed the exact raw text; neither literal nor JS-string form was found\n\
         --- expected text ---\n{expected}\n--- expected JS string ---\n{}\n--- body ---\n{}",
        forms[1],
        truncate(body, 4_000),
    );
}

fn assert_does_not_embed_raw_text(body: &str, unexpected: &str, label: &str) {
    let forms = raw_text_forms(unexpected);
    assert!(
        forms.iter().all(|form| !body.contains(form)),
        "{label} unexpectedly still embeds the old raw text\n--- old text ---\n{unexpected}\n\
         --- body ---\n{}",
        truncate(body, 4_000),
    );
}

fn assert_client_raw_texts_embedded(root: &Path, body: &str, label: &str) {
    let relative = read_text(&root.join(CLIENT_RELATIVE_RAW_PATH));
    let aliased = read_text(&root.join(CLIENT_ALIAS_RAW_PATH));
    assert_embeds_raw_text(body, &relative, &format!("{label} relative ?raw"));
    assert_embeds_raw_text(body, &aliased, &format!("{label} aliased ?raw"));
}

fn assert_text_omits_sub_packages(text: &str, label: &str) {
    assert!(
        !text.contains(SUB_PACKAGES_DIR),
        "{label} must not mention {SUB_PACKAGES_DIR:?}\n--- text ---\n{}",
        truncate(text, 4_000),
    );
}

fn assert_build_output_omits_sub_packages(stdout: &str, stderr: &str, label: &str) {
    assert_text_omits_sub_packages(stdout, &format!("{label} stdout"));
    assert_text_omits_sub_packages(stderr, &format!("{label} stderr"));
}

fn assert_contains_all(body: &str, markers: &[&str], label: &str) {
    for marker in markers {
        assert!(
            body.contains(marker),
            "{label} is missing {marker:?}\n--- body ---\n{}",
            truncate(body, 4_000),
        );
    }
}

fn assert_contains_none(body: &str, markers: &[&str], label: &str) {
    for marker in markers {
        assert!(
            !body.contains(marker),
            "{label} unexpectedly contains {marker:?}\n--- body ---\n{}",
            truncate(body, 4_000),
        );
    }
}

fn assert_mode_tuple(body: &str, marker: &str, development: bool, label: &str) {
    let occurrences = body.match_indices(marker).collect::<Vec<_>>();
    assert_eq!(
        occurrences.len(),
        1,
        "{label} must contain exactly one mode tuple marker {marker:?}; found {}\n--- body ---\n{}",
        occurrences.len(),
        truncate(body, 4_000),
    );

    // Ignore formatting, but require the marker and all three substituted
    // literals to remain adjacent and ordered in the emitted array. Production
    // esbuild may shorten `true`/`false` to `!0`/`!1` while minifying.
    let tuple_tail = &body[occurrences[0].0 + marker.len()..];
    let compact = tuple_tail
        .chars()
        .take(512)
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    let (dev_tokens, prod_tokens, node_env) = if development {
        (["true", "!0"], ["false", "!1"], "development")
    } else {
        (["false", "!1"], ["true", "!0"], "production")
    };
    let quotes = ['"', '\'', '`'];
    let matches_expected = quotes.iter().any(|marker_quote| {
        dev_tokens.iter().any(|dev| {
            prod_tokens.iter().any(|prod| {
                quotes.iter().any(|node_quote| {
                    compact.starts_with(&format!(
                        "{marker_quote},{dev},{prod},{node_quote}{node_env}{node_quote}"
                    ))
                })
            })
        })
    });
    assert!(
        matches_expected,
        "{label} mode tuple {marker:?} must be followed immediately by DEV={}, PROD={}, NODE_ENV={node_env:?}; got {:?}",
        development,
        !development,
        truncate(&compact, 240),
    );
}

fn truncate(value: &str, max: usize) -> &str {
    let end = value
        .char_indices()
        .nth(max)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    &value[..end]
}

fn find_single_asset(dir: &Path, prefix: &str) -> PathBuf {
    let mut matches = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read asset directory {}: {error}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".js"))
        })
        .collect::<Vec<_>>();
    matches.sort();
    assert_eq!(
        matches.len(),
        1,
        "expected one {prefix}*.js entry in {}, got {matches:#?}",
        dir.display(),
    );
    matches.remove(0)
}

struct WorkerNames {
    island: String,
    island_nested: String,
    client: String,
    client_nested: String,
}

impl WorkerNames {
    fn for_root(root: &Path) -> Self {
        let filename = |relative: &str| {
            zfb_types::module_worker_filename(root, &root.join(relative))
                .unwrap_or_else(|error| panic!("derive worker filename for {relative}: {error}"))
        };
        Self {
            island: filename("components/cross-pipeline/worker.ts"),
            island_nested: filename("components/cross-pipeline/nested.ts"),
            client: filename("components/cross-pipeline-client/worker.ts"),
            client_nested: filename("components/cross-pipeline-client/nested.ts"),
        }
    }
}

fn assert_parent_worker_url(body: &str, worker: &str, label: &str) {
    let needle = format!("./{worker}?v=");
    let occurrences = body.match_indices(&needle).collect::<Vec<_>>();
    assert!(
        occurrences.len() == 1,
        "{label} must contain exactly one URL {needle}<8-lower-hex>; found {}\n--- body ---\n{}",
        occurrences.len(),
        truncate(body, 4_000),
    );
    let query_start = occurrences[0].0 + needle.len();
    let query = &body[query_start..];
    assert!(
        query.len() >= 8,
        "{label} worker query ended before its 8-character digest: {query:?}",
    );
    let digest_bytes = &query.as_bytes()[..8];
    let digest = std::str::from_utf8(digest_bytes)
        .unwrap_or_else(|_| panic!("{label} worker query digest is not ASCII: {digest_bytes:?}"));
    assert!(
        digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{label} worker query digest must be exactly lowercase hex, got {digest:?}",
    );
    assert!(
        query
            .as_bytes()
            .get(8)
            .is_none_or(|byte| !byte.is_ascii_hexdigit()),
        "{label} worker query digest must end after exactly 8 hex characters: {query:?}",
    );
}

fn assert_island_pipeline(
    entry: &str,
    worker: &str,
    nested: &str,
    names: &WorkerNames,
    development: bool,
) {
    assert_contains_all(
        entry,
        &[
            "ZFB_ISLAND_RAW_PAYLOAD",
            "ZFB_ISLAND_CONFIGURED_LOADER",
            "ZFB_CONFIGURED_DEFINE_VALUE",
        ],
        "islands entry",
    );
    assert_mode_tuple(
        entry,
        "ZFB_ISLAND_ENTRY_MODE_TUPLE",
        development,
        "islands entry",
    );
    assert_parent_worker_url(entry, &names.island, "islands entry");

    assert_contains_all(
        worker,
        &[
            "ZFB_ISLAND_WORKER_ENTRY",
            "ZFB_ISLAND_WORKER_RAW_PAYLOAD",
            "ZFB_ISLAND_WORKER_CONFIGURED_LOADER",
            "ZFB_CONFIGURED_DEFINE_VALUE",
        ],
        "islands worker",
    );
    assert_mode_tuple(
        worker,
        "ZFB_ISLAND_WORKER_MODE_TUPLE",
        development,
        "islands worker",
    );
    assert_parent_worker_url(worker, &names.island_nested, "islands worker");

    assert_contains_all(
        nested,
        &[
            "ZFB_ISLAND_NESTED_WORKER",
            "ZFB_ISLAND_NESTED_CONFIGURED_LOADER",
            "ZFB_CONFIGURED_DEFINE_VALUE",
        ],
        "nested islands worker",
    );
    assert_mode_tuple(
        nested,
        "ZFB_ISLAND_NESTED_MODE_TUPLE",
        development,
        "nested islands worker",
    );

    if development {
        assert_contains_all(
            entry,
            &[
                "ZFB_ISLAND_META_DEV",
                "ZFB_ISLAND_NODE_DEV",
                "ZFB_ISLAND_DEV_BRANCH",
            ],
            "islands development mode",
        );
    } else {
        assert_contains_all(
            entry,
            &["ZFB_ISLAND_META_PROD", "ZFB_ISLAND_NODE_PROD"],
            "islands production mode",
        );
        assert_contains_none(
            entry,
            &[
                "ZFB_ISLAND_META_DEV",
                "ZFB_ISLAND_NODE_DEV",
                "ZFB_ISLAND_DEV_BRANCH",
            ],
            "islands production mode",
        );
    }

    if development {
        assert_contains_all(
            worker,
            &[
                "ZFB_ISLAND_WORKER_META_DEV",
                "ZFB_ISLAND_WORKER_NODE_DEV",
                "ZFB_ISLAND_WORKER_DEV_BRANCH",
            ],
            "islands worker development mode",
        );
    } else {
        assert_contains_all(
            worker,
            &["ZFB_ISLAND_WORKER_META_PROD", "ZFB_ISLAND_WORKER_NODE_PROD"],
            "islands worker production mode",
        );
        assert_contains_none(
            worker,
            &[
                "ZFB_ISLAND_WORKER_META_DEV",
                "ZFB_ISLAND_WORKER_NODE_DEV",
                "ZFB_ISLAND_WORKER_DEV_BRANCH",
            ],
            "islands worker production mode",
        );
    }

    let nested_required = if development {
        ["ZFB_ISLAND_NESTED_META_DEV", "ZFB_ISLAND_NESTED_NODE_DEV"]
    } else {
        ["ZFB_ISLAND_NESTED_META_PROD", "ZFB_ISLAND_NESTED_NODE_PROD"]
    };
    assert_contains_all(nested, &nested_required, "nested islands worker mode");
    if !development {
        assert_contains_none(
            nested,
            &["ZFB_ISLAND_NESTED_META_DEV", "ZFB_ISLAND_NESTED_NODE_DEV"],
            "nested islands worker production mode",
        );
    }
}

fn assert_client_pipeline(
    entry: &str,
    worker: &str,
    nested: &str,
    names: &WorkerNames,
    development: bool,
) {
    assert_contains_all(
        entry,
        &[
            "ZFB_CLIENT_ENTRY",
            "ZFB_CLIENT_RAW_PAYLOAD",
            "ZFB_CLIENT_ALIAS_RAW_PAYLOAD_V1",
            "ZFB_CLIENT_CONFIGURED_LOADER",
            "ZFB_CONFIGURED_DEFINE_VALUE",
        ],
        "client-script entry",
    );
    assert_mode_tuple(
        entry,
        "ZFB_CLIENT_ENTRY_MODE_TUPLE",
        development,
        "client-script entry",
    );
    assert_parent_worker_url(entry, &names.client, "client-script entry");

    assert_contains_all(
        worker,
        &[
            "ZFB_CLIENT_WORKER_ENTRY",
            "ZFB_CLIENT_WORKER_RAW_PAYLOAD",
            "ZFB_CLIENT_WORKER_CONFIGURED_LOADER",
            "ZFB_CONFIGURED_DEFINE_VALUE",
        ],
        "client-script worker",
    );
    assert_mode_tuple(
        worker,
        "ZFB_CLIENT_WORKER_MODE_TUPLE",
        development,
        "client-script worker",
    );
    assert_parent_worker_url(worker, &names.client_nested, "client-script worker");

    assert_contains_all(
        nested,
        &[
            "ZFB_CLIENT_NESTED_WORKER",
            "ZFB_CLIENT_NESTED_CONFIGURED_LOADER",
            "ZFB_CONFIGURED_DEFINE_VALUE",
        ],
        "nested client-script worker",
    );
    assert_mode_tuple(
        nested,
        "ZFB_CLIENT_NESTED_MODE_TUPLE",
        development,
        "nested client-script worker",
    );

    if development {
        assert_contains_all(
            entry,
            &[
                "ZFB_CLIENT_META_DEV",
                "ZFB_CLIENT_NODE_DEV",
                "ZFB_CLIENT_DEV_BRANCH",
            ],
            "client-script development mode",
        );
    } else {
        assert_contains_all(
            entry,
            &["ZFB_CLIENT_META_PROD", "ZFB_CLIENT_NODE_PROD"],
            "client-script production mode",
        );
        assert_contains_none(
            entry,
            &[
                "ZFB_CLIENT_META_DEV",
                "ZFB_CLIENT_NODE_DEV",
                "ZFB_CLIENT_DEV_BRANCH",
            ],
            "client-script production mode",
        );
    }

    if development {
        assert_contains_all(
            worker,
            &[
                "ZFB_CLIENT_WORKER_META_DEV",
                "ZFB_CLIENT_WORKER_NODE_DEV",
                "ZFB_CLIENT_WORKER_DEV_BRANCH",
            ],
            "client-script worker development mode",
        );
    } else {
        assert_contains_all(
            worker,
            &["ZFB_CLIENT_WORKER_META_PROD", "ZFB_CLIENT_WORKER_NODE_PROD"],
            "client-script worker production mode",
        );
        assert_contains_none(
            worker,
            &[
                "ZFB_CLIENT_WORKER_META_DEV",
                "ZFB_CLIENT_WORKER_NODE_DEV",
                "ZFB_CLIENT_WORKER_DEV_BRANCH",
            ],
            "client-script worker production mode",
        );
    }

    let nested_required = if development {
        ["ZFB_CLIENT_NESTED_META_DEV", "ZFB_CLIENT_NESTED_NODE_DEV"]
    } else {
        ["ZFB_CLIENT_NESTED_META_PROD", "ZFB_CLIENT_NESTED_NODE_PROD"]
    };
    assert_contains_all(nested, &nested_required, "nested client-script worker mode");
    if !development {
        assert_contains_none(
            nested,
            &["ZFB_CLIENT_NESTED_META_DEV", "ZFB_CLIENT_NESTED_NODE_DEV"],
            "nested client-script worker production mode",
        );
    }
}

fn run_zfb_build(root: &Path, esbuild: &Path, label: &str) -> (String, String) {
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn `zfb build`");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "`zfb build` failed for {label}\nstatus: {:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );
    (stdout, stderr)
}

fn run_and_assert_production(root: &Path, esbuild: &Path) {
    let (stdout, stderr) = run_zfb_build(root, esbuild, "the cross-pipeline fixture");
    assert_build_output_omits_sub_packages(&stdout, &stderr, "cross-pipeline production build");

    let names = WorkerNames::for_root(root);
    let assets = root.join("dist/assets");
    let island_entry = read_text(&find_single_asset(&assets, "islands-"));
    let island_worker = read_text(&assets.join(&names.island));
    let island_nested = read_text(&assets.join(&names.island_nested));
    assert!(
        !assets.join("client").join(&names.island).exists()
            && !assets.join("client").join(&names.island_nested).exists(),
        "islands worker companions must not leak into dist/assets/client/",
    );
    assert_island_pipeline(&island_entry, &island_worker, &island_nested, &names, false);

    let client_dir = assets.join("client");
    let client_entry = read_text(&find_single_asset(&client_dir, "cross-pipeline-"));
    let client_worker = read_text(&client_dir.join(&names.client));
    let client_nested = read_text(&client_dir.join(&names.client_nested));
    assert!(
        !assets.join(&names.client).exists() && !assets.join(&names.client_nested).exists(),
        "client-script worker companions must not leak into dist/assets/",
    );
    assert_client_pipeline(&client_entry, &client_worker, &client_nested, &names, false);
    assert_client_raw_texts_embedded(root, &client_entry, "production client entry");

    for (label, body) in [
        ("production islands entry", island_entry.as_str()),
        ("production islands worker", island_worker.as_str()),
        ("production nested islands worker", island_nested.as_str()),
        ("production client entry", client_entry.as_str()),
        ("production client worker", client_worker.as_str()),
        ("production nested client worker", client_nested.as_str()),
    ] {
        assert_contains_none(
            body,
            &[
                "?raw",
                "__CROSS_PIPELINE_DEFINE__",
                "import.meta.env.DEV",
                "import.meta.env.PROD",
                "process.env.NODE_ENV",
            ],
            label,
        );
    }

    let html = read_text(&root.join("dist/index.html"));
    assert_contains_all(
        &html,
        &[
            "ZFB_CROSS_PIPELINE_PAGE",
            "ZFB_CROSS_PIPELINE_ISLAND",
            "ZFB_ISLAND_RAW_PAYLOAD",
            "ZFB_ISLAND_CONFIGURED_LOADER",
            "ZFB_CONFIGURED_DEFINE_VALUE",
            "ZFB_ISLAND_META_PROD",
            "ZFB_ISLAND_NODE_PROD",
            "/assets/islands-",
            "/assets/client/cross-pipeline-",
        ],
        "production SSR HTML",
    );
    assert_contains_none(
        &html,
        &[
            "?raw",
            "./worker.ts",
            "ZFB_ISLAND_WORKER_ENTRY",
            "ZFB_CLIENT_WORKER_ENTRY",
        ],
        "production SSR HTML",
    );

    // A successful page render proves the island passed through the server
    // pipeline. Inspect the actual SSR artifact as a second guard against the
    // preprocessing-only syntax leaking past that pass.
    let ssr_bundle = read_text(&root.join(".zfb-build/bundle.mjs"));
    assert_contains_none(
        &ssr_bundle,
        &[
            "?raw",
            "./demo.frag?raw",
            "./worker.ts",
            "__CROSS_PIPELINE_DEFINE__",
            "import.meta.env.DEV",
            "import.meta.env.PROD",
            "process.env.NODE_ENV",
            "ZFB_ISLAND_WORKER_ENTRY",
            "ZFB_CLIENT_WORKER_ENTRY",
        ],
        "production SSR bundle",
    );
}

struct DevServerGuard {
    child: std::process::Child,
    pgid: libc::pid_t,
}

impl DevServerGuard {
    fn try_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().expect("poll `zfb dev` child")
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
    fn logs(&self) -> String {
        format!(
            "--- zfb dev stdout ---\n{}\n--- zfb dev stderr ---\n{}",
            fs::read_to_string(&self.stdout_path).unwrap_or_default(),
            fs::read_to_string(&self.stderr_path).unwrap_or_default(),
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
        .env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    command.process_group(0);
    let child = command.spawn().expect("spawn `zfb dev --port 0`");
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
        let token = candidate.split_whitespace().next().unwrap_or_default();
        if let Some(colon) = token.find(':') {
            let digits = token[colon + 1..]
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

async fn wait_for_ready(session: &mut DevSession) -> u16 {
    let started = Instant::now();
    loop {
        if let Some(status) = session.guard.try_status() {
            panic!(
                "`zfb dev` exited before readiness with {status:?}\n{}",
                session.logs(),
            );
        }
        if let Some(port) =
            parse_ready_port(&fs::read_to_string(&session.stdout_path).unwrap_or_default())
        {
            assert_ne!(port, 0, "ready banner reported literal port 0");
            return port;
        }
        assert!(
            started.elapsed() < BOOT_DEADLINE,
            "`zfb dev --port 0` did not become ready in {}s\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn poll_body(
    client: &reqwest::Client,
    url: &str,
    marker: &str,
    label: &str,
    session: &DevSession,
) -> String {
    let started = Instant::now();
    let mut last = String::from("no response");
    while started.elapsed() < BOOT_DEADLINE {
        match client.get(url).send().await {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                if status.as_u16() == 200 && body.contains(marker) {
                    return body;
                }
                last = format!("status={status}, body={}", truncate(&body, 500));
            }
            Err(error) => last = format!("request failed: {error}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "{label}: {url} did not return 200 with {marker:?} in {}s; last={last}\n{}",
        BOOT_DEADLINE.as_secs(),
        session.logs(),
    );
}

async fn subscribe_reload(
    client: &reqwest::Client,
    origin: &str,
    session: &DevSession,
) -> reqwest::Response {
    let url = format!("{origin}/__zfb/reload");
    let response = client.get(&url).send().await.unwrap_or_else(|error| {
        panic!("subscribe to {url}: {error}\n{}", session.logs());
    });
    assert_eq!(
        response.status().as_u16(),
        200,
        "SSE endpoint {url} must answer 200\n{}",
        session.logs(),
    );
    response
}

async fn assert_alias_raw_dev_invalidation(
    root: &Path,
    client: &reqwest::Client,
    origin: &str,
    session: &DevSession,
) {
    let old_alias = read_text(&root.join(CLIENT_ALIAS_RAW_PATH));
    let client_url = format!("{origin}/assets/client/cross-pipeline.js");
    let sse = subscribe_reload(client, origin, session).await;
    fs::write(root.join(CLIENT_ALIAS_RAW_PATH), UPDATED_ALIAS_RAW_TEXT)
        .expect("edit aliased raw target while zfb dev is running");

    let event = next_sse_event_name(sse, SSE_DEADLINE)
        .await
        .expect("read SSE stream after aliased raw edit");
    assert!(
        event.is_some(),
        "aliased ?raw edit must trigger a dev reload event within {}s\n{}",
        SSE_DEADLINE.as_secs(),
        session.logs(),
    );

    let refreshed = poll_body(
        client,
        &client_url,
        "ZFB_CLIENT_ALIAS_RAW_PAYLOAD_V2",
        "development client-script entry after aliased raw edit",
        session,
    )
    .await;
    assert_embeds_raw_text(
        &refreshed,
        UPDATED_ALIAS_RAW_TEXT,
        "development client entry after aliased raw edit",
    );
    assert_does_not_embed_raw_text(
        &refreshed,
        &old_alias,
        "development client entry after aliased raw edit",
    );
}

async fn run_and_assert_development(root: &Path, esbuild: &Path) {
    let names = WorkerNames::for_root(root);
    let mut session = spawn_dev(root, esbuild);
    let port = wait_for_ready(&mut session).await;
    let origin = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build loopback HTTP client");

    let html = poll_body(
        &client,
        &format!("{origin}/"),
        "ZFB_CROSS_PIPELINE_PAGE",
        "development SSR page",
        &session,
    )
    .await;
    assert_contains_all(
        &html,
        &[
            "ZFB_CROSS_PIPELINE_ISLAND",
            "ZFB_ISLAND_RAW_PAYLOAD",
            "ZFB_ISLAND_CONFIGURED_LOADER",
            "ZFB_CONFIGURED_DEFINE_VALUE",
            "ZFB_ISLAND_META_DEV",
            "ZFB_ISLAND_NODE_DEV",
            "/assets/client/cross-pipeline.js",
        ],
        "development SSR HTML",
    );

    let island_entry = poll_body(
        &client,
        &format!("{origin}/assets/islands.js"),
        "ZFB_ISLAND_RAW_PAYLOAD",
        "development islands entry",
        &session,
    )
    .await;
    let island_worker = poll_body(
        &client,
        &format!("{origin}/assets/{}", names.island),
        "ZFB_ISLAND_WORKER_ENTRY",
        "development islands worker",
        &session,
    )
    .await;
    let island_nested = poll_body(
        &client,
        &format!("{origin}/assets/{}", names.island_nested),
        "ZFB_ISLAND_NESTED_WORKER",
        "development nested islands worker",
        &session,
    )
    .await;
    assert_island_pipeline(&island_entry, &island_worker, &island_nested, &names, true);

    let client_entry = poll_body(
        &client,
        &format!("{origin}/assets/client/cross-pipeline.js"),
        "ZFB_CLIENT_RAW_PAYLOAD",
        "development client-script entry",
        &session,
    )
    .await;
    let client_worker = poll_body(
        &client,
        &format!("{origin}/assets/client/{}", names.client),
        "ZFB_CLIENT_WORKER_ENTRY",
        "development client-script worker",
        &session,
    )
    .await;
    let client_nested = poll_body(
        &client,
        &format!("{origin}/assets/client/{}", names.client_nested),
        "ZFB_CLIENT_NESTED_WORKER",
        "development nested client-script worker",
        &session,
    )
    .await;

    // The HTTP bodies above must be backed by companions in each pipeline's
    // distinct on-disk namespace. Checking both the expected and wrong path
    // prevents a permissive static-server fallback from hiding layout drift.
    let dev_assets = root.join(".zfb-build/dev-assets/assets");
    assert_eq!(read_text(&dev_assets.join(&names.island)), island_worker);
    assert_eq!(
        read_text(&dev_assets.join(&names.island_nested)),
        island_nested,
    );
    assert_eq!(
        read_text(&dev_assets.join("client").join(&names.client)),
        client_worker,
    );
    assert_eq!(
        read_text(&dev_assets.join("client").join(&names.client_nested)),
        client_nested,
    );
    assert!(
        !dev_assets.join("client").join(&names.island).exists()
            && !dev_assets
                .join("client")
                .join(&names.island_nested)
                .exists()
            && !dev_assets.join(&names.client).exists()
            && !dev_assets.join(&names.client_nested).exists(),
        "dev worker companions must remain in their owning assets namespace",
    );
    assert_client_pipeline(&client_entry, &client_worker, &client_nested, &names, true);
    assert_client_raw_texts_embedded(root, &client_entry, "development client entry");

    for (label, body) in [
        ("development islands entry", island_entry.as_str()),
        ("development islands worker", island_worker.as_str()),
        ("development nested islands worker", island_nested.as_str()),
        ("development client entry", client_entry.as_str()),
        ("development client worker", client_worker.as_str()),
        ("development nested client worker", client_nested.as_str()),
    ] {
        assert_contains_none(
            body,
            &[
                "?raw",
                "__CROSS_PIPELINE_DEFINE__",
                "import.meta.env.DEV",
                "import.meta.env.PROD",
                "process.env.NODE_ENV",
            ],
            label,
        );
    }
    assert_text_omits_sub_packages(&session.logs(), "cross-pipeline development output");
}

fn run_and_assert_workspace_shape_production(root: &Path, esbuild: &Path) {
    let (stdout, stderr) = run_zfb_build(root, esbuild, "the pnpm-workspace raw-hardening fixture");
    assert_build_output_omits_sub_packages(&stdout, &stderr, "pnpm-workspace raw-hardening build");

    let client_dir = root.join("dist/assets/client");
    let client_entry = read_text(&find_single_asset(&client_dir, "cross-pipeline-"));
    assert_client_raw_texts_embedded(
        root,
        &client_entry,
        "pnpm-workspace production client entry",
    );
    assert_contains_none(
        &client_entry,
        &["?raw"],
        "pnpm-workspace production client entry",
    );
}

async fn run_and_assert_workspace_shape_development(root: &Path, esbuild: &Path) {
    let mut session = spawn_dev(root, esbuild);
    let port = wait_for_ready(&mut session).await;
    let origin = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build loopback HTTP client");

    let client_entry = poll_body(
        &client,
        &format!("{origin}/assets/client/cross-pipeline.js"),
        "ZFB_CLIENT_ALIAS_RAW_PAYLOAD_V1",
        "pnpm-workspace development client entry",
        &session,
    )
    .await;
    assert_client_raw_texts_embedded(
        root,
        &client_entry,
        "pnpm-workspace development client entry",
    );

    assert_alias_raw_dev_invalidation(root, &client, &origin, &session).await;
    assert_text_omits_sub_packages(&session.logs(), "pnpm-workspace development output");
}

// Issue #1674: a genuinely-CLAIMED pnpm-workspace sub-package host whose client
// graph reaches SIBLING workspace source through `?raw` (entry) and a
// project-local module worker (whose own graph reaches a second sibling
// `?raw`). The build runs with the host — `sub-packages/reroot-host` — as the
// project root; `first_party_root_for` widens to the copied workspace root so
// the sibling raw payloads inline instead of erroring "outside the mirrorable
// project tree". The two sibling payloads live under `sub-packages/reroot-shared`.
const REROOT_HOST_REL: &str = "sub-packages/reroot-host";
const REROOT_ENTRY_RAW: &str = "ZFB_REROOT_SIBLING_ENTRY_RAW_PAYLOAD";
const REROOT_WORKER_RAW: &str = "ZFB_REROOT_SIBLING_WORKER_RAW_PAYLOAD";

// Issue #1701 (Wave 2 confirm pass): the registered virtual module
// `virtual:reroot-vmodule-host` (`sub-packages/reroot-host/preset.mjs`)
// absolute-imports `sub-packages/vmodule-sibling/vmodule-host.ts`, which
// exercises the sibling rewrite kinds the CLIENT-script pipeline supports —
// a `?raw` import and a nested module worker — so the workspace tier of
// `remap_virtual_module_project_imports_to_shadow` (#1699/#1700/#1701) is
// proven to carry those preprocessing passes through a virtual-module edge,
// not just the plain relative sibling `?raw` the rest of this test covers.
//
// `import.meta.glob` is deliberately absent from that sibling: client scripts
// do not expand `import.meta.glob` at all (issue #1404), so a glob reached
// through this CLIENT virtual-module edge would ship unexpanded regardless of
// the remap — that boundary is #1404's, not this epic's. The
// `assert_contains_none` calls below still guard that no unexpanded
// `import.meta.glob(` leaks into a client bundle. Glob expansion for a
// virtual-module sibling is an SSR/bundler-path concern (`run_esbuild`).
const REROOT_VMODULE_ENTRY_MARKER: &str = "ZFB_VMODULE_SIBLING_ENTRY";
const REROOT_VMODULE_RAW: &str = "ZFB_VMODULE_SIBLING_RAW_PAYLOAD";
const REROOT_VMODULE_WORKER_MARKER: &str = "ZFB_VMODULE_WORKER_ENTRY";
const REROOT_VMODULE_WORKER_ORIGINAL_URL: &str = "./vmodule-worker.ts";

fn reroot_worker_filename(host_root: &Path) -> String {
    zfb_types::module_worker_filename(host_root, &host_root.join("src/reroot.worker.ts"))
        .expect("derive reroot host worker companion filename")
}

// The virtual-module sibling's own worker source lives OUTSIDE the host
// project (under `sub-packages/vmodule-sibling`, a workspace sibling of
// `reroot-host`), so its companion name goes through the workspace-scoped
// `worker--ws-` branch (`module_worker_filename_scoped`) rather than the
// project-local branch `reroot_worker_filename` above exercises.
fn reroot_vmodule_worker_filename(host_root: &Path) -> String {
    let sibling_worker = host_root
        .parent()
        .expect("reroot host has a sub-packages parent")
        .join("vmodule-sibling/vmodule-worker.ts");
    let first_party_root = zfb_types::first_party_root_for(host_root);
    zfb_types::module_worker_filename_scoped(host_root, &first_party_root, &sibling_worker)
        .expect("derive virtual-module sibling worker companion filename")
}

fn run_and_assert_reroot_host_production(host_root: &Path, esbuild: &Path) {
    run_zfb_build(host_root, esbuild, "the workspace reroot host");

    let client_dir = host_root.join("dist/assets/client");
    let client_entry = read_text(&find_single_asset(&client_dir, "reroot-"));
    assert_contains_all(
        &client_entry,
        &[
            "ZFB_REROOT_CLIENT_ENTRY",
            REROOT_ENTRY_RAW,
            REROOT_VMODULE_ENTRY_MARKER,
            REROOT_VMODULE_RAW,
        ],
        "reroot host production client entry",
    );
    assert_contains_none(
        &client_entry,
        &[
            "?raw",
            "import.meta.glob(",
            REROOT_VMODULE_WORKER_ORIGINAL_URL,
        ],
        "reroot host production client entry",
    );
    let vmodule_worker_filename = reroot_vmodule_worker_filename(host_root);
    assert_parent_worker_url(
        &client_entry,
        &vmodule_worker_filename,
        "reroot host production client entry",
    );

    let worker = read_text(&client_dir.join(reroot_worker_filename(host_root)));
    assert_contains_all(
        &worker,
        &["ZFB_REROOT_WORKER_ENTRY", REROOT_WORKER_RAW],
        "reroot host production worker",
    );
    assert_contains_none(&worker, &["?raw"], "reroot host production worker");

    let vmodule_worker_path = client_dir.join(&vmodule_worker_filename);
    assert!(
        vmodule_worker_path.is_file(),
        "reroot host production build must emit the virtual-module sibling worker companion at {}",
        vmodule_worker_path.display(),
    );
    let vmodule_worker = read_text(&vmodule_worker_path);
    assert_contains_all(
        &vmodule_worker,
        &[REROOT_VMODULE_WORKER_MARKER],
        "reroot host production virtual-module sibling worker",
    );
    assert_contains_none(
        &vmodule_worker,
        &["?raw", "import.meta.glob("],
        "reroot host production virtual-module sibling worker",
    );

    // Issue #1758: the ISLANDS pipeline bundles this same registered virtual
    // module through a `"use client"` island (RerootIsland.tsx), independent
    // of the client-script entry above — proving
    // `remap_project_plugin_virtual_modules_to_shadow` (reached via
    // `build_default_islands_payload`) carries the sibling `?raw` +
    // nested-module-worker rewrites through the ISLANDS call site too.
    let islands_dir = host_root.join("dist/assets");
    let island_entry = read_text(&find_single_asset(&islands_dir, "islands-"));
    assert_contains_all(
        &island_entry,
        &[
            "ZFB_REROOT_ISLAND",
            REROOT_VMODULE_ENTRY_MARKER,
            REROOT_VMODULE_RAW,
        ],
        "reroot host production islands entry",
    );
    assert_contains_none(
        &island_entry,
        &[
            "?raw",
            "import.meta.glob(",
            REROOT_VMODULE_WORKER_ORIGINAL_URL,
        ],
        "reroot host production islands entry",
    );
    assert_parent_worker_url(
        &island_entry,
        &vmodule_worker_filename,
        "reroot host production islands entry",
    );

    // The islands pipeline is a separate esbuild flow from the client-script
    // pipeline, so it emits its OWN copy of the shared vmodule worker
    // companion into the islands namespace (`dist/assets/`) — distinct from
    // the client-script copy already asserted above (`dist/assets/client/`).
    let islands_vmodule_worker_path = islands_dir.join(&vmodule_worker_filename);
    assert!(
        islands_vmodule_worker_path.is_file(),
        "reroot host production build must emit the virtual-module sibling worker companion \
         into the islands namespace at {}",
        islands_vmodule_worker_path.display(),
    );
    let islands_vmodule_worker = read_text(&islands_vmodule_worker_path);
    assert_contains_all(
        &islands_vmodule_worker,
        &[REROOT_VMODULE_WORKER_MARKER],
        "reroot host production islands virtual-module sibling worker",
    );
    assert_contains_none(
        &islands_vmodule_worker,
        &["?raw", "import.meta.glob("],
        "reroot host production islands virtual-module sibling worker",
    );

    let html = read_text(&host_root.join("dist/index.html"));
    assert_contains_all(
        &html,
        &[
            "ZFB_REROOT_HOST_PAGE",
            "ZFB_REROOT_ISLAND",
            "/assets/islands-",
        ],
        "reroot host production HTML",
    );
}

async fn run_and_assert_reroot_host_development(host_root: &Path, esbuild: &Path) {
    let mut session = spawn_dev(host_root, esbuild);
    let port = wait_for_ready(&mut session).await;
    let origin = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build loopback HTTP client");

    let html = poll_body(
        &client,
        &format!("{origin}/"),
        "ZFB_REROOT_HOST_PAGE",
        "reroot host development SSR page",
        &session,
    )
    .await;
    // Unlike the production HTML (which injects an `/assets/islands-<hash>`
    // hydration script tag), the development SSR pass does not embed an
    // `/assets/islands.js` reference — only the island's server-rendered
    // content, matching the base fixture's own dev-HTML assertion shape.
    assert_contains_all(
        &html,
        &["ZFB_REROOT_ISLAND"],
        "reroot host development SSR HTML",
    );

    let client_entry = poll_body(
        &client,
        &format!("{origin}/assets/client/reroot.js"),
        "ZFB_REROOT_CLIENT_ENTRY",
        "reroot host development client entry",
        &session,
    )
    .await;
    assert_contains_all(
        &client_entry,
        &[
            REROOT_ENTRY_RAW,
            REROOT_VMODULE_ENTRY_MARKER,
            REROOT_VMODULE_RAW,
        ],
        "reroot host development client entry",
    );
    assert_contains_none(
        &client_entry,
        &[
            "?raw",
            "import.meta.glob(",
            REROOT_VMODULE_WORKER_ORIGINAL_URL,
        ],
        "reroot host development client entry",
    );
    let vmodule_worker_filename = reroot_vmodule_worker_filename(host_root);
    assert_parent_worker_url(
        &client_entry,
        &vmodule_worker_filename,
        "reroot host development client entry",
    );

    let worker_filename = reroot_worker_filename(host_root);
    let worker = poll_body(
        &client,
        &format!("{origin}/assets/client/{worker_filename}"),
        "ZFB_REROOT_WORKER_ENTRY",
        "reroot host development worker",
        &session,
    )
    .await;
    assert_contains_all(
        &worker,
        &[REROOT_WORKER_RAW],
        "reroot host development worker",
    );
    assert_contains_none(&worker, &["?raw"], "reroot host development worker");

    let vmodule_worker = poll_body(
        &client,
        &format!("{origin}/assets/client/{vmodule_worker_filename}"),
        REROOT_VMODULE_WORKER_MARKER,
        "reroot host development virtual-module sibling worker",
        &session,
    )
    .await;
    assert_contains_none(
        &vmodule_worker,
        &["?raw", "import.meta.glob("],
        "reroot host development virtual-module sibling worker",
    );

    // Issue #1758: the ISLANDS pipeline serves its own copy of the same
    // registered virtual module (through RerootIsland.tsx) under the islands
    // namespace (`/assets/`), independent of the client-script copy above
    // (`/assets/client/`).
    let island_entry = poll_body(
        &client,
        &format!("{origin}/assets/islands.js"),
        "ZFB_REROOT_ISLAND",
        "reroot host development islands entry",
        &session,
    )
    .await;
    assert_contains_all(
        &island_entry,
        &[REROOT_VMODULE_ENTRY_MARKER, REROOT_VMODULE_RAW],
        "reroot host development islands entry",
    );
    assert_contains_none(
        &island_entry,
        &[
            "?raw",
            "import.meta.glob(",
            REROOT_VMODULE_WORKER_ORIGINAL_URL,
        ],
        "reroot host development islands entry",
    );
    assert_parent_worker_url(
        &island_entry,
        &vmodule_worker_filename,
        "reroot host development islands entry",
    );

    let islands_vmodule_worker = poll_body(
        &client,
        &format!("{origin}/assets/{vmodule_worker_filename}"),
        REROOT_VMODULE_WORKER_MARKER,
        "reroot host development islands virtual-module sibling worker",
        &session,
    )
    .await;
    assert_contains_none(
        &islands_vmodule_worker,
        &["?raw", "import.meta.glob("],
        "reroot host development islands virtual-module sibling worker",
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "env-gate: pinned esbuild binary — ZFB_ESBUILD_BIN=<absolute path to pinned esbuild> \
            cargo test -p zfb --test client_bundling_cross_pipeline -- --ignored"]
async fn real_binary_build_and_dev_cover_client_bundling_contract() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let esbuild = locate_esbuild().expect(
        "client-bundling cross-pipeline test requires the pinned esbuild binary; \
         set ZFB_ESBUILD_BIN to an absolute executable path",
    );
    assert!(
        esbuild.is_absolute(),
        "locate_esbuild must resolve an absolute binary path, got {}",
        esbuild.display(),
    );

    let production = tempfile::tempdir().expect("production fixture tempdir");
    copy_fixture(&fixture_dir(), production.path()).expect("copy production fixture");
    run_and_assert_production(production.path(), &esbuild);

    let development = tempfile::tempdir().expect("development fixture tempdir");
    copy_fixture(&fixture_dir(), development.path()).expect("copy development fixture");
    run_and_assert_development(development.path(), &esbuild).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "env-gate: pinned esbuild binary — ZFB_ESBUILD_BIN=<absolute path to pinned esbuild> \
            cargo test -p zfb --test client_bundling_cross_pipeline -- --ignored"]
async fn real_binary_build_and_dev_cover_pnpm_workspace_raw_hardening_contract() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let esbuild = locate_esbuild().expect(
        "pnpm-workspace raw-hardening cross-pipeline test requires the pinned esbuild binary; \
         set ZFB_ESBUILD_BIN to an absolute executable path",
    );
    assert!(
        esbuild.is_absolute(),
        "locate_esbuild must resolve an absolute binary path, got {}",
        esbuild.display(),
    );

    let production = tempfile::tempdir().expect("pnpm-workspace production fixture tempdir");
    copy_fixture(&fixture_dir(), production.path())
        .expect("copy pnpm-workspace production fixture");
    run_and_assert_workspace_shape_production(production.path(), &esbuild);

    let development = tempfile::tempdir().expect("pnpm-workspace development fixture tempdir");
    copy_fixture(&fixture_dir(), development.path())
        .expect("copy pnpm-workspace development fixture");
    run_and_assert_workspace_shape_development(development.path(), &esbuild).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "env-gate: pinned esbuild binary — ZFB_ESBUILD_BIN=<absolute path to pinned esbuild> \
            cargo test -p zfb --test client_bundling_cross_pipeline -- --ignored"]
async fn real_binary_build_and_dev_cover_workspace_reroot_host_contract() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let esbuild = locate_esbuild().expect(
        "workspace reroot-host cross-pipeline test requires the pinned esbuild binary; \
         set ZFB_ESBUILD_BIN to an absolute executable path",
    );
    assert!(
        esbuild.is_absolute(),
        "locate_esbuild must resolve an absolute binary path, got {}",
        esbuild.display(),
    );

    let production = tempfile::tempdir().expect("reroot-host production fixture tempdir");
    copy_fixture(&fixture_dir(), production.path()).expect("copy reroot-host production fixture");
    run_and_assert_reroot_host_production(&production.path().join(REROOT_HOST_REL), &esbuild);

    let development = tempfile::tempdir().expect("reroot-host development fixture tempdir");
    copy_fixture(&fixture_dir(), development.path()).expect("copy reroot-host development fixture");
    run_and_assert_reroot_host_development(&development.path().join(REROOT_HOST_REL), &esbuild)
        .await;
}
