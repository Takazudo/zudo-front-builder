//! Central confirm pass: the same request-time SSR handlers against the
//! **real Workers runtime** (issue #2020, V8 Request Time Parity epic
//! #2012, Wave 8 — the epic's final gate over Waves 1-7).
//!
//! ## Why this file exists
//!
//! `crates/zfb/tests/embedded_host_request_time_e2e.rs` (#2019, Wave 7)
//! proved the embedded V8 host's `fetch` and Web Crypto now WORK. That is
//! only half of epic #2012's claim
//! (research/2013-request-time-capability-contract.md's whole premise):
//! the other half is that supported behaviour matches real production
//! Cloudflare Workers, and that every place it doesn't is an already
//! documented divergence (D1-D9), not a surprise. Nothing else in this
//! repo drives the SAME handler source through both runtimes and diffs
//! the result — this file is that comparison.
//!
//! ## Method
//!
//! Three page sources under `tests/fixtures/workerd-parity/pages/api/`
//! are each built ONCE for the `@takazudo/zfb-adapter-cloudflare` adapter
//! (real `zfb build`, following `wasm_ssr_adapter_e2e.rs`'s scaffolding
//! pattern) and served UNMODIFIED a second time under `zfb dev`'s
//! embedded V8 host. The adapter output is then served by a real,
//! workspace-pinned `wrangler dev` (real workerd, local mode — no
//! `--remote`, no Cloudflare account, no public internet: guardrail 3 of
//! epic #2012). The exact same HTTP requests are issued against both
//! servers and the responses are compared:
//!
//! - **`/api/happy`** — a supported-everywhere contract row (fetch +
//!   `getRandomValues` + `randomUUID` + `subtle.digest("SHA-256", ...)`).
//!   Both runtimes must succeed, and the loopback fetch body/status and
//!   the SHA-256 digest hex must be **byte-identical** across runtimes —
//!   the strongest possible proof that a "supported" contract row means
//!   the same thing in both places.
//! - **`/api/legacy-digest`** — divergence D7 (`crypto.subtle.digest`
//!   does not support `MD5` here; workerd does, as a legacy extension).
//!   Real workerd must succeed with the exact RFC 1321 test-vector digest
//!   for `"abc"`; the embedded host must fail closed with
//!   `NotSupportedError`.
//! - **`/api/keyed-crypto`** — divergence D8 (key-bearing SubtleCrypto
//!   fails closed here; workerd implements the full matrix). A genuine
//!   AES-GCM generate-key -> encrypt -> decrypt round trip: real workerd
//!   must actually recover the plaintext (proof of real support, not
//!   just "didn't throw"), while the embedded host must fail closed with
//!   `NotSupportedError` naming the zfb embedded runtime.
//!
//! ## What this file does NOT (re-)cover
//!
//! - **Guardrail 4 (the SSG build-time network denial)** is already
//!   directly regression-tested by
//!   `embedded_host_request_time_e2e.rs`'s
//!   `build_still_denies_network_at_ssg_time`. Re-deriving it here against
//!   a THIRD fixture would add nothing.
//! - **D1-D6, D9** (fetch timeout, body caps, `Response.body === null`,
//!   `Error` vs real `DOMException`, loopback reachability, no
//!   `accept-encoding`/decompression, the 50-subrequest cap) are either
//!   already unit/e2e-tested against the embedded host alone (#2015-#2019)
//!   or are, by construction, NOT observable from outside the process
//!   (D1/D9 are about *this host's own* limits, not something production
//!   would ever hit locally). Re-deriving all nine rows through a real
//!   `wrangler dev` process would multiply this file's already-heavy
//!   process-spawning cost for rows that add no new evidence over the
//!   existing unit coverage. The three rows selected above are exactly
//!   the ones where "does production actually do the opposite of what we
//!   do" was previously an assertion in a markdown file, not a proven
//!   fact — that gap is this file's job.
//!
//! ## Tiering
//!
//! `#[ignore]`d as `env-gate: wrangler` (workspace-pinned 4.85.0, same
//! pin as `wasm_ssr_adapter_e2e.rs`) — it needs a real `wrangler dev`
//! process, which is heavier than the `deploy --dry-run` that file uses.
//! Spawns a real `zfb build`, a real `zfb dev --port 0`, AND a real
//! `wrangler dev --port 0`, so it joins the flock-adopting bucket of
//! `.config/nextest.toml`'s `e2e-heavy` test-group and acquires
//! `zfb_test_utils::CrossBinaryE2eLock` for the whole test.
//!
//! ## Determinism
//!
//! The `/api/happy` fetch target is a loopback server this file spawns
//! itself (guardrail 3 — never the public internet). All waits are
//! condition-keyed polls against a parsed ready banner — never a fixed
//! sleep gating an assertion. `WRANGLER_SEND_METRICS=false` and `CI=true`
//! are set on the `wrangler dev` child to suppress telemetry and update
//! checks, which would otherwise be an undeclared network dependency.

#![cfg(unix)]

use std::fs;
use std::net::SocketAddr;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

const EXPECTED_WRANGLER_VERSION: &str = "4.85.0";
const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const RESPONSE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

// RFC 1321's own MD5 test vector for the ASCII input "abc" — used so the
// production-side assertion is checked against a well-known value, not
// merely "did wrangler return something".
const MD5_ABC: &str = "900150983cd24fb0d6963f7d28e17f72";
const KEYED_CRYPTO_PLAINTEXT: &str = "zfb-e2e-workerd-parity-plaintext";

// ---------------------------------------------------------------------------
// Deterministic loopback server (guardrail 3)
// ---------------------------------------------------------------------------

/// Answers every request with a fixed 200 body, then closes the
/// connection. Shared by both the `wrangler dev` leg and the `zfb dev`
/// leg so the SAME server proves the SAME fetch response reaches both
/// runtimes.
struct LoopbackServer {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl LoopbackServer {
    async fn spawn(body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("read assigned local port");
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let mut total = 0usize;
                    loop {
                        if total == buf.len() {
                            buf.resize(buf.len() * 2, 0);
                        }
                        let n = match stream.read(&mut buf[total..]).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => n,
                        };
                        total += n;
                        if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self { addr, task }
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }
}

// ---------------------------------------------------------------------------
// Fixture staging
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("workerd-parity")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/zfb must live two levels under the workspace root")
        .to_path_buf()
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
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

/// Copy the checked-in fixture and substitute the loopback port baked
/// into `pages/api/happy.tsx` (the embedded host has no `process.env` —
/// same "stage dynamic values at test time" pattern
/// `embedded_host_request_time_e2e.rs` and `wasm_ssr_dev_smoke_e2e.rs`
/// use).
fn stage_fixture(root: &Path, loopback_port: u16) {
    copy_dir(&fixture_dir(), root).expect("copy workerd-parity fixture");
    let path = root.join("pages/api/happy.tsx");
    let source = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let substituted = source.replace("__LOOPBACK_PORT__", &loopback_port.to_string());
    fs::write(&path, substituted).unwrap_or_else(|e| panic!("write {path:?}: {e}"));
}

/// Wire the fixture's `node_modules` the same way
/// `wasm_ssr_adapter_e2e.rs`'s `scaffold_fixture` does: the embedded
/// framework tree (preact / preact-render-to-string) plus the actual
/// workspace Cloudflare adapter package linked in as a fixture-local
/// dependency, so `zfb build`'s adapter dispatch (`pnpm exec
/// zfb-adapter-cloudflare`) resolves the real, unpublished workspace
/// code under test rather than any registry version.
fn wire_adapter_node_modules(root: &Path, adapter_package: &Path) -> tempfile::TempDir {
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
    node_modules_handle
}

fn run_build(root: &Path, esbuild: &Path) {
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn `zfb build`");
    assert!(
        output.status.success(),
        "`zfb build` must succeed for the workerd-parity fixture\nstatus: {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        root.join("dist/_worker.js").is_file(),
        "adapter build must emit dist/_worker.js"
    );
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

// ---------------------------------------------------------------------------
// Process management: one guard shape shared by both the `zfb dev` leg
// and the `wrangler dev` leg (mirrors `embedded_host_request_time_e2e.rs`
// / `preview_cross_mode_e2e.rs`: own process group, logs to files never
// pipes, group-kill on Drop).
// ---------------------------------------------------------------------------

struct ServerGuard {
    child: std::process::Child,
    pgid: libc::pid_t,
    label: &'static str,
}

impl ServerGuard {
    fn try_exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().expect("try_wait on child process")
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

struct ServerSession {
    guard: ServerGuard,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl ServerSession {
    fn logs(&self) -> String {
        format!(
            "--- {} stdout ---\n{}\n--- {} stderr ---\n{}",
            self.guard.label,
            fs::read_to_string(&self.stdout_path).unwrap_or_default(),
            self.guard.label,
            fs::read_to_string(&self.stderr_path).unwrap_or_default(),
        )
    }
}

fn spawn_process_group(
    mut command: Command,
    root: &Path,
    label: &'static str,
    log_prefix: &str,
) -> ServerSession {
    let stdout_path = root.join(format!(".{log_prefix}-stdout.log"));
    let stderr_path = root.join(format!(".{log_prefix}-stderr.log"));
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");
    command
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    command.process_group(0);
    let child = command
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {label}: {e}"));
    let pgid = child.id() as libc::pid_t;
    ServerSession {
        guard: ServerGuard { child, pgid, label },
        stdout_path,
        stderr_path,
    }
}

/// Extract the ephemeral port from a "http://<host>:PORT" substring
/// anywhere in the log (matches both `zfb dev`'s ready banner and
/// wrangler's `Ready on http://127.0.0.1:PORT` line).
fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(index) = rest.find("http://") {
        let candidate = &rest[index + "http://".len()..];
        let token = candidate.split_whitespace().next().unwrap_or("");
        if let Some(colon) = token.rfind(':') {
            let digits: String = token[colon + 1..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
        rest = &rest[index + "http://".len()..];
    }
    None
}

async fn boot_or_skip(session: &mut ServerSession) -> Option<u16> {
    let boot_start = Instant::now();
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            let combined = session.logs();
            if combined.contains("embed_v8") || combined.contains("no esbuild") {
                eprintln!(
                    "[workerd_parity_e2e] {} exited with a known environment gate; skipping.\n{combined}",
                    session.guard.label,
                );
                return None;
            }
            panic!(
                "{} exited prematurely (status {status:?}) before its ready banner.\n{}",
                session.guard.label,
                session.logs(),
            );
        }
        if let Some(port) =
            parse_ready_port(&fs::read_to_string(&session.stdout_path).unwrap_or_default())
        {
            assert_ne!(
                port,
                0,
                "{}'s ready banner printed port 0 instead of the ephemeral bound port.\n{}",
                session.guard.label,
                session.logs(),
            );
            return Some(port);
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "{} did not print a parseable ready banner within {}s.\n{}",
            session.guard.label,
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn poll_for_marker(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
    expect_marker: &str,
    session: &ServerSession,
) -> String {
    let url = format!("{base_url}{path}");
    let start = Instant::now();
    loop {
        let last_observation = match client.get(&url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                if status == 200 && body.contains(expect_marker) {
                    return body;
                }
                format!("status {status}, body:\n{body}")
            }
            Err(error) => format!("request error: {error}"),
        };
        assert!(
            start.elapsed() < RESPONSE_DEADLINE,
            "GET {url} did not return 200 with {expect_marker:?} within {}s. \
             Last observation: {last_observation}\n{}",
            RESPONSE_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn spawn_zfb_dev(root: &Path, esbuild: &Path) -> ServerSession {
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
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS");
    spawn_process_group(command, root, "zfb dev", "zfb-dev")
}

fn spawn_wrangler_dev(root: &Path, wrangler: &Path) -> ServerSession {
    let mut command = Command::new(wrangler);
    command
        .args(["dev", "--port", "0", "--ip", "127.0.0.1"])
        .current_dir(root)
        // Local-mode `wrangler dev` needs no Cloudflare account or public
        // internet access; these two env vars suppress its own telemetry
        // and update-check network calls so the test has no undeclared
        // network dependency (guardrail 3).
        .env("WRANGLER_SEND_METRICS", "false")
        .env("CI", "true");
    spawn_process_group(command, root, "wrangler dev", "wrangler-dev")
}

// ---------------------------------------------------------------------------
// Assertions shared by both runtimes' happy-path response
// ---------------------------------------------------------------------------

struct HappyParsed {
    fetch_body: String,
    fetch_status: String,
    random_nonzero: String,
    uuid: String,
    digest: String,
}

fn parse_happy_body(body: &str, runtime_label: &str) -> HappyParsed {
    // The last marker in the joined `HAPPY_*` string has no trailing `|` —
    // it runs straight into the rest of the rendered HTML (which differs
    // between runtimes: the embedded host injects a dev-only livereload
    // `<script>` tag). Bound every field to its value's own alphabet
    // (alphanumeric + `-`, which covers hex digests, UUIDs, "true"/"false",
    // status codes, and "loopback-ok") rather than splitting on `|`, so a
    // trailing HTML tag can never be captured as part of the value.
    let field = |marker: &str| -> String {
        body.split(marker)
            .nth(1)
            .map(|rest| {
                rest.chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect::<String>()
            })
            .unwrap_or_else(|| panic!("{runtime_label}: missing {marker:?} in body:\n{body}"))
    };
    HappyParsed {
        fetch_body: field("HAPPY_FETCH_BODY:"),
        fetch_status: field("HAPPY_FETCH_STATUS:"),
        random_nonzero: field("HAPPY_RANDOM_NONZERO:"),
        uuid: field("HAPPY_UUID:"),
        digest: field("HAPPY_DIGEST:"),
    }
}

fn assert_valid_v4_uuid(uuid: &str, runtime_label: &str) {
    assert_eq!(
        uuid.len(),
        36,
        "{runtime_label}: randomUUID() did not return a 36-character UUID: {uuid:?}",
    );
    assert_eq!(
        uuid.as_bytes()[14],
        b'4',
        "{runtime_label}: randomUUID() did not set the version-4 nibble: {uuid:?}",
    );
    assert!(
        matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
        "{runtime_label}: randomUUID() did not set the RFC 4122 variant bits: {uuid:?}",
    );
}

// ---------------------------------------------------------------------------
// The confirm pass
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
#[ignore = "env-gate: wrangler — workspace-pinned 4.85.0; run cargo test -p zfb --test workerd_parity_e2e -- --ignored"]
async fn embedded_host_matches_real_workerd_for_supported_rows_and_documented_divergences() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();

    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[workerd_parity_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN to the pinned native binary."
        );
        return;
    };
    if !pnpm_available() {
        eprintln!("[workerd_parity_e2e] pnpm is not available; skipping.");
        return;
    }
    let Some(wrangler) = locate_pinned_wrangler() else {
        eprintln!(
            "[workerd_parity_e2e] workspace-pinned wrangler is absent; skipping env-gate. \
             Run pnpm install --frozen-lockfile or set ZFB_WRANGLER_BIN."
        );
        return;
    };
    assert_pinned_wrangler_version(&wrangler);

    let loopback = LoopbackServer::spawn("loopback-ok").await;

    // ---- Production leg: real `zfb build` (Cloudflare adapter) + real `wrangler dev` ----
    let adapter_package = workspace_adapter_package();
    let prod_tmp = tempfile::tempdir().expect("create production fixture tempdir");
    let prod_root = prod_tmp
        .path()
        .canonicalize()
        .expect("canonicalize production fixture root");
    stage_fixture(&prod_root, loopback.port());
    let _node_modules_handle = wire_adapter_node_modules(&prod_root, &adapter_package);
    run_build(&prod_root, &esbuild);

    let mut wrangler_session = spawn_wrangler_dev(&prod_root, &wrangler);
    let Some(wrangler_port) = boot_or_skip(&mut wrangler_session).await else {
        return;
    };
    let wrangler_base = format!("http://127.0.0.1:{wrangler_port}");

    // ---- Dev leg: real `zfb dev` embedded V8 host, the SAME page sources ----
    let dev_tmp = tempfile::tempdir().expect("create dev fixture tempdir");
    let dev_root = dev_tmp
        .path()
        .canonicalize()
        .expect("canonicalize dev fixture root");
    stage_fixture(&dev_root, loopback.port());

    let mut dev_session = spawn_zfb_dev(&dev_root, &esbuild);
    let Some(dev_port) = boot_or_skip(&mut dev_session).await else {
        return;
    };
    let dev_base = format!("http://localhost:{dev_port}");

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client");

    // =========================================================================
    // Case 1: `/api/happy` — every supported row must match byte-for-byte.
    // =========================================================================
    let wrangler_happy = poll_for_marker(
        &client,
        &wrangler_base,
        "/api/happy",
        "HAPPY_FETCH_BODY:",
        &wrangler_session,
    )
    .await;
    let dev_happy = poll_for_marker(
        &client,
        &dev_base,
        "/api/happy",
        "HAPPY_FETCH_BODY:",
        &dev_session,
    )
    .await;
    let wrangler_parsed = parse_happy_body(&wrangler_happy, "wrangler/workerd");
    let dev_parsed = parse_happy_body(&dev_happy, "zfb dev/embedded host");

    assert_eq!(
        wrangler_parsed.fetch_body, "loopback-ok",
        "real workerd's outbound fetch did not reach the loopback server.\nbody:\n{wrangler_happy}",
    );
    assert_eq!(
        dev_parsed.fetch_body, "loopback-ok",
        "the embedded host's outbound fetch did not reach the loopback server.\nbody:\n{dev_happy}",
    );
    assert_eq!(
        wrangler_parsed.fetch_body, dev_parsed.fetch_body,
        "fetch response body diverged between real workerd and the embedded host",
    );
    assert_eq!(
        wrangler_parsed.fetch_status, "200",
        "real workerd's fetch did not surface a 200 status.\nbody:\n{wrangler_happy}"
    );
    assert_eq!(
        wrangler_parsed.fetch_status, dev_parsed.fetch_status,
        "fetch response status diverged between real workerd and the embedded host",
    );
    assert_eq!(
        wrangler_parsed.random_nonzero, "true",
        "real workerd's crypto.getRandomValues produced an all-zero buffer.\nbody:\n{wrangler_happy}"
    );
    assert_eq!(
        dev_parsed.random_nonzero, "true",
        "the embedded host's crypto.getRandomValues produced an all-zero buffer.\nbody:\n{dev_happy}"
    );
    assert_valid_v4_uuid(&wrangler_parsed.uuid, "wrangler/workerd");
    assert_valid_v4_uuid(&dev_parsed.uuid, "zfb dev/embedded host");
    assert_eq!(
        wrangler_parsed.digest, dev_parsed.digest,
        "SHA-256 digest of the identical fixed input diverged between real workerd \
         ({}) and the embedded host ({}) — a supported contract row must produce \
         byte-identical output.",
        wrangler_parsed.digest, dev_parsed.digest,
    );

    // =========================================================================
    // Case 2: `/api/legacy-digest` — divergence D7 (MD5).
    // =========================================================================
    let wrangler_legacy = poll_for_marker(
        &client,
        &wrangler_base,
        "/api/legacy-digest",
        "LEGACY_DIGEST_",
        &wrangler_session,
    )
    .await;
    assert!(
        wrangler_legacy.contains(&format!("LEGACY_DIGEST_OK:{MD5_ABC}")),
        "real workerd must support crypto.subtle.digest(\"MD5\", ...) as a documented \
         legacy extension, matching the RFC 1321 test vector for \"abc\".\nbody:\n{wrangler_legacy}",
    );

    let dev_legacy = poll_for_marker(
        &client,
        &dev_base,
        "/api/legacy-digest",
        "LEGACY_DIGEST_",
        &dev_session,
    )
    .await;
    assert!(
        !dev_legacy.contains("LEGACY_DIGEST_OK:"),
        "divergence D7 requires the embedded host to fail closed on MD5, not succeed.\n\
         body:\n{dev_legacy}",
    );
    assert!(
        dev_legacy.contains("LEGACY_DIGEST_ERROR_NAME:NotSupportedError"),
        "the embedded host's MD5 rejection must be NotSupportedError, per the D7 \
         contract row.\nbody:\n{dev_legacy}",
    );
    assert!(
        dev_legacy.contains("This host implements SHA-1, SHA-256, SHA-384, SHA-512"),
        "the embedded host's MD5 rejection must name its supported algorithm set, per \
         the D7 contract row.\nbody:\n{dev_legacy}",
    );

    // =========================================================================
    // Case 3: `/api/keyed-crypto` — divergence D8 (key-bearing SubtleCrypto).
    // =========================================================================
    let wrangler_keyed = poll_for_marker(
        &client,
        &wrangler_base,
        "/api/keyed-crypto",
        "KEYED_CRYPTO_",
        &wrangler_session,
    )
    .await;
    assert!(
        wrangler_keyed.contains(&format!("KEYED_CRYPTO_OK:{KEYED_CRYPTO_PLAINTEXT}")),
        "real workerd must complete a genuine AES-GCM generateKey -> encrypt -> decrypt \
         round trip and recover the exact plaintext.\nbody:\n{wrangler_keyed}",
    );

    let dev_keyed = poll_for_marker(
        &client,
        &dev_base,
        "/api/keyed-crypto",
        "KEYED_CRYPTO_",
        &dev_session,
    )
    .await;
    assert!(
        !dev_keyed.contains("KEYED_CRYPTO_OK:"),
        "divergence D8 requires the embedded host to fail closed on key-bearing \
         SubtleCrypto, not succeed.\nbody:\n{dev_keyed}",
    );
    assert!(
        dev_keyed.contains("KEYED_CRYPTO_ERROR_NAME:NotSupportedError"),
        "the embedded host's key-bearing SubtleCrypto rejection must be \
         NotSupportedError, per the D8 contract row.\nbody:\n{dev_keyed}",
    );
    assert!(
        dev_keyed.contains("is not implemented in the zfb embedded runtime"),
        "the embedded host's key-bearing SubtleCrypto rejection must name the zfb \
         embedded runtime, per the D8 contract row.\nbody:\n{dev_keyed}",
    );
}
