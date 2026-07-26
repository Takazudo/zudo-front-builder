//! Deterministic embedded-host end-to-end proof for request-time `fetch`
//! and Web Crypto (issue #2019, V8 Request Time Parity epic #2012, Wave 7
//! — the confirm pass over Waves 2-6; the workerd/wrangler comparison is
//! the separate Wave 8 sub-issue, #2020, not this file's job).
//!
//! ## What this proves
//!
//! `crates/zfb-server/src/routes.rs` dispatches every `prerender = false`
//! request into the embedded V8 host through
//! `crates/zfb/src/ssr_adapter.rs`, the ONE production call site that
//! sets `DispatchMode::RequestTime` (research/2013-request-time-capability-contract.md,
//! "Mode distinction"). Waves 2-6 landed mode plumbing, the async fetch
//! transport, the JS fetch/Request/Response adaptation, OS entropy, and
//! Web Crypto — each proven at the unit/integration level inside
//! `zfb-render`. This file is the first test to drive that whole stack
//! through a REAL `zfb dev` process and a real HTTP response, which is
//! the concrete thing issue #1750 said was broken (`fetch` unconditionally
//! rejecting at request time).
//!
//! ## The five required cases (issue #2019 acceptance criteria)
//!
//! 1. `GET /api/happy` — the headline case: a single request-time
//!    dispatch performs a real outbound loopback fetch AND uses
//!    `crypto.getRandomValues`/`randomUUID`/`subtle.digest`, all in one
//!    handler, all succeeding.
//! 2. `GET /api/exhaust` — explicit RESOURCE-EXHAUSTION: 51 concurrent
//!    fetches in one dispatch, one more than
//!    `MAX_SUBREQUESTS_PER_DISPATCH = 50`; the Rust-side per-dispatch
//!    counter rejects the 51st regardless of `Promise.all` fan-out.
//! 3. `GET /api/refused` — explicit HOST-OP-FAILURE-ADJACENT case: a
//!    loopback port that is bound and then immediately released before
//!    `zfb dev` boots, so the Rust transport op itself fails
//!    (`FetchError::Transport`, connection refused) rather than any
//!    JS-side policy rejecting the call. See the "Scope note" below —
//!    this is deliberately NOT the contract's `HostUnavailable` row.
//! 4. `GET /api/unsupported` — an unsupported capability
//!    (`crypto.subtle.encrypt`, a key-bearing SubtleCrypto method,
//!    divergence D8) fails with a REQUEST-TIME-SPECIFIC diagnostic
//!    (`NotSupportedError`, "...is not implemented in the zfb embedded
//!    runtime...") and never the build-time-only "fetch() called from
//!    SSG runtime" wording — the exact defect epic #2012 exists to fix.
//! 5. `zfb build` over a SEPARATE, SSR-route-free fixture — build-time
//!    SSG still denies network access (guardrail 4): a default-`prerender`
//!    page's `fetch()` call rejects with the unchanged SSG denial
//!    message, and the build fails with that message surfaced.
//!
//! ## Scope note: case 3 is not the contract's `HostUnavailable` row
//!
//! research/2013-request-time-capability-contract.md's fetch table has
//! TWO distinct failure rows: "Transport failure" (any DNS/TCP/TLS
//! failure — `FetchError::Transport`) and "Host-op failure" (the op
//! itself cannot run at all, e.g. the channel is closed or the runtime
//! is shutting down — `FetchError::HostUnavailable`). The latter is only
//! reachable by removing the fetch extension from the runtime entirely
//! (already unit-tested inside `zfb-render`, e.g.
//! `crates/zfb-render/src/embedded_v8/fetch.rs`'s own tests) — there is
//! no way to force that state from outside a running `zfb dev` process.
//! Case 3 here exercises the deterministic, e2e-reachable neighbour:
//! a real connection refusal, which is still a genuine HOST-SIDE
//! (Rust transport op) failure rather than a JS-side policy rejection,
//! and is what an operator's local loopback dependency being down would
//! actually look like.
//!
//! ## Scope note: no `zfb preview` leg
//!
//! The issue brief asks this to run through both `zfb dev` and `zfb
//! preview`. `crates/zfb/tests/preview_cross_mode_e2e.rs` already
//! documents (see its "Scope decision" section) that `zfb preview`
//! (static) never boots a V8 host or dispatches SSR at request time —
//! `zfb build` hard-fails via `ensure_no_ssr_without_adapter` for any
//! project mixing a `prerender = false` route with no SSR-capable
//! adapter, and there is no Cloudflare/wrangler-backed preview leg wired
//! up yet (that is Wave 8, #2020). So there is architecturally nothing
//! for the embedded host to serve under `zfb preview` today. This file
//! covers `zfb dev` (cases 1-4, request-time) and `zfb build` (case 5,
//! build-time) instead — the two commands that actually reach the
//! embedded V8 host on this branch.
//!
//! ## Tiering
//!
//! NOT `#[ignore]`d — like `wasm_ssr_dev_smoke_e2e.rs`, this test
//! self-skips at runtime when esbuild is unavailable
//! (`locate_esbuild()` returns `None`) or when `zfb dev`/`zfb build`
//! exits with a known environment-gate indicator (`embed_v8` cfg off,
//! "no esbuild"). It spawns a real `zfb dev` AND a real `zfb build`, so
//! it joins the flock-adopting bucket of `.config/nextest.toml`'s
//! `e2e-heavy` test-group (see that file and CLAUDE.md's nextest
//! inventory, both updated alongside this test) and acquires
//! `zfb_test_utils::CrossBinaryE2eLock` for the whole test, matching
//! `preview_cross_mode_e2e.rs`'s convention of one lock-holding test per
//! binary.
//!
//! ## Determinism
//!
//! Every fetch target is a loopback server this file spawns itself, or a
//! loopback port deliberately left unlistened (guardrail 3 — no public
//! internet anywhere). All waits are condition-keyed polls against a
//! parsed ready banner or an HTTP response body marker — never a fixed
//! sleep gating an assertion.

#![cfg(unix)]

use std::fs;
use std::net::SocketAddr;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const RESPONSE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Deterministic loopback server (guardrail 3 — never the public internet)
// ---------------------------------------------------------------------------

/// A minimal loopback HTTP/1.1 server: answers every request with a fixed
/// 200 body, then closes the connection. Good enough for this file's
/// GET-only scenarios (the happy path and the 51-fetch resource-exhaustion
/// fan-out both only need a fast, deterministic 200).
///
/// Bound to `127.0.0.1:0` and read back via `local_addr` — never a
/// hard-coded port, which would collide across the parallel test binaries
/// this crate already runs (and across concurrent test runs on the same
/// host).
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

/// Bind an ephemeral loopback port and hold the listener open — do NOT
/// drop it here. Codex review flagged an earlier version of this helper
/// that bound-then-immediately-dropped the port at test start: `zfb dev`
/// takes real wall-clock time to boot, and another local process could
/// have claimed the freed port before `/api/refused` is ever requested,
/// making the case flaky (a stray listener would turn "connection
/// refused" into an unexpected 200). The caller keeps the returned
/// listener alive through the whole boot, and drops it immediately
/// before issuing the request that needs the connection refused — this
/// shrinks the reuse window from "however long boot takes" to
/// microseconds.
async fn bind_released_port_listener() -> TcpListener {
    TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener for the refused-port case")
}

// ---------------------------------------------------------------------------
// Fixture staging
// ---------------------------------------------------------------------------

fn dev_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("embedded-host-request-time")
}

fn ssg_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("embedded-host-request-time-ssg")
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let destination = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &destination)?;
        } else {
            fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}

/// Copy the dev fixture and substitute the loopback ports baked into the
/// page sources at test time (the embedded host has no `process.env` —
/// this is the same "stage dynamic values into the fixture" pattern
/// `wasm_ssr_dev_smoke_e2e.rs` uses for its Wasm bytes).
fn stage_dev_fixture(root: &Path, loopback_port: u16, refused_port: u16) {
    copy_dir(&dev_fixture_dir(), root).expect("copy embedded-host-request-time fixture");
    for name in ["happy.tsx", "exhaust.tsx", "refused.tsx"] {
        let path = root.join("pages").join("api").join(name);
        let source = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let substituted = source
            .replace("__LOOPBACK_PORT__", &loopback_port.to_string())
            .replace("__REFUSED_PORT__", &refused_port.to_string());
        fs::write(&path, substituted).unwrap_or_else(|e| panic!("write {path:?}: {e}"));
    }
}

// ---------------------------------------------------------------------------
// `zfb dev` process management (mirrors wasm_ssr_dev_smoke_e2e.rs /
// preview_cross_mode_e2e.rs: own process group, logs to files never
// pipes, group-kill on Drop)
// ---------------------------------------------------------------------------

struct DevServerGuard {
    child: std::process::Child,
    pgid: libc::pid_t,
}

impl DevServerGuard {
    fn try_exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().expect("try_wait on `zfb dev`")
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

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
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
            read_log(&self.stdout_path),
            read_log(&self.stderr_path),
        )
    }
}

/// Extract the ephemeral port from the dev ready banner
/// (`http://localhost:PORT`).
fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(index) = rest.find("http://") {
        let candidate = &rest[index + "http://".len()..];
        let token = candidate.split_whitespace().next().unwrap_or("");
        if let Some(colon) = token.find(':') {
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

fn spawn_dev(root: &Path, esbuild: &Path) -> DevSession {
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");

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
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    command.process_group(0);

    let child = command.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

/// Boot `zfb dev` and return its ephemeral port, or `None` if it exited
/// with a known environment-gate indicator (no esbuild / no `embed_v8`).
async fn boot_dev_or_skip(session: &mut DevSession) -> Option<u16> {
    let boot_start = Instant::now();
    loop {
        if let Some(status) = session.guard.try_exit_status() {
            let combined = session.logs();
            if combined.contains("embed_v8") || combined.contains("no esbuild") {
                eprintln!(
                    "[embedded_host_request_time_e2e] `zfb dev` exited with a known \
                     environment gate; skipping.\n{combined}",
                );
                return None;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before its ready banner.\n{}",
                session.logs(),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
            assert_ne!(
                port,
                0,
                "ready banner printed port 0 instead of the ephemeral bound port.\n{}",
                session.logs(),
            );
            return Some(port);
        }
        assert!(
            boot_start.elapsed() < BOOT_DEADLINE,
            "`zfb dev` did not print a parseable ready banner within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `GET {base_url}{path}` until the body contains `expect_marker`,
/// or panic with the last observation after `RESPONSE_DEADLINE`.
/// Condition-keyed: a lazy dev session builds an SSR route on its first
/// request, so there is no fixed delay to wait out.
async fn poll_for_marker(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
    expect_marker: &str,
    session: &DevSession,
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

// ---------------------------------------------------------------------------
// Cases 1-4: a real `zfb dev` session
// ---------------------------------------------------------------------------

/// Cases 1-4 of issue #2019's acceptance criteria — all through one real
/// `zfb dev --port 0` session, each hitting a distinct SSR route.
#[tokio::test(flavor = "multi_thread")]
async fn dev_serves_request_time_fetch_and_web_crypto() {
    // Acquired before any other synchronization, per
    // `cross_binary_lock.rs`'s documented lock ordering, and held for the
    // whole test (this file has only one spawning test).
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[embedded_host_request_time_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN to the pinned native binary."
        );
        return;
    };

    let loopback = LoopbackServer::spawn("loopback-ok").await;
    // Kept alive (not dropped) until immediately before the refused-case
    // request below — see `bind_released_port_listener`'s doc comment.
    let refused_listener = bind_released_port_listener().await;
    let refused_port = refused_listener
        .local_addr()
        .expect("read assigned refused-port listener address")
        .port();

    let tmp = tempfile::tempdir().expect("create dev fixture tempdir");
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize fixture root");
    stage_dev_fixture(&root, loopback.port(), refused_port);

    let mut session = spawn_dev(&root, &esbuild);
    let Some(port) = boot_dev_or_skip(&mut session).await else {
        return;
    };
    let base_url = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client");

    // ---- Case 1: the headline — fetch + Web Crypto succeed together ----
    let happy_body = poll_for_marker(
        &client,
        &base_url,
        "/api/happy",
        "HAPPY_FETCH_BODY:",
        &session,
    )
    .await;
    assert!(
        happy_body.contains("HAPPY_FETCH_BODY:loopback-ok"),
        "request-time fetch did not return the loopback server's body.\nbody:\n{happy_body}",
    );
    assert!(
        happy_body.contains("HAPPY_FETCH_STATUS:200"),
        "request-time fetch did not surface the loopback server's 200 status.\nbody:\n{happy_body}",
    );
    assert!(
        happy_body.contains("HAPPY_RANDOM_NONZERO:true"),
        "crypto.getRandomValues produced an all-zero buffer (should be OS-CSPRNG-backed).\n\
         body:\n{happy_body}",
    );
    // The fixture joins its markers with `|` into one JSX text node
    // (see `pages/api/happy.tsx`) specifically so this split is
    // unambiguous regardless of JSX inter-element whitespace collapsing.
    let uuid = happy_body
        .split("HAPPY_UUID:")
        .nth(1)
        .and_then(|rest| rest.split('|').next())
        .unwrap_or_default()
        .to_string();
    // A version-4 UUID's 15th hex character is "4" and the 20th is one
    // of {8, 9, a, b} — the exact bits `randomUUID` is contractually
    // required to set (research/2013-request-time-capability-contract.md).
    assert_eq!(
        uuid.len(),
        36,
        "crypto.randomUUID() did not return a 36-character UUID: {uuid:?}\nbody:\n{happy_body}",
    );
    assert_eq!(
        uuid.as_bytes()[14],
        b'4',
        "crypto.randomUUID() did not set the version-4 nibble: {uuid:?}",
    );
    assert!(
        matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
        "crypto.randomUUID() did not set the RFC 4122 variant bits: {uuid:?}",
    );
    let expected_digest_hex = {
        let mut hasher = Sha256::new();
        hasher.update(b"zfb-e2e-happy-path");
        let digest = hasher.finalize();
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    assert!(
        happy_body.contains(&format!("HAPPY_DIGEST:{expected_digest_hex}")),
        "crypto.subtle.digest(\"SHA-256\", ...) did not match the Rust-computed digest \
         {expected_digest_hex}.\nbody:\n{happy_body}",
    );

    // ---- Case 2: explicit resource exhaustion — the 51-subrequest cap ----
    let exhaust_body =
        poll_for_marker(&client, &base_url, "/api/exhaust", "EXHAUST_", &session).await;
    assert!(
        !exhaust_body.contains("EXHAUST_UNEXPECTED_SUCCESS"),
        "51 concurrent fetches in one dispatch all succeeded — the per-dispatch \
         50-subrequest budget did not reject the 51st.\nbody:\n{exhaust_body}",
    );
    assert!(
        exhaust_body.contains("EXHAUST_ERROR:")
            && exhaust_body.contains("exceeded the 50-subrequest limit"),
        "the 51st concurrent fetch did not fail with the contract's subrequest-limit \
         message.\nbody:\n{exhaust_body}",
    );

    // ---- Case 3: host-side transport failure (connection refused) ----
    // Drop the listener HERE, immediately before the request that needs
    // the port unclaimed — not at test start — to minimize the window
    // another local process could grab it first (Codex review finding).
    drop(refused_listener);
    let refused_body =
        poll_for_marker(&client, &base_url, "/api/refused", "REFUSED_", &session).await;
    assert!(
        !refused_body.contains("REFUSED_UNEXPECTED_SUCCESS"),
        "fetch to a released loopback port with nothing listening unexpectedly \
         succeeded.\nbody:\n{refused_body}",
    );
    assert!(
        refused_body.contains(&format!(
            "REFUSED_ERROR:fetch(http://127.0.0.1:{refused_port}/nope):"
        )),
        "connection-refused fetch did not surface the expected \
         `fetch(<url>): <cause>` transport-failure shape.\nbody:\n{refused_body}",
    );

    // ---- Case 4: unsupported capability — request-time-specific diagnostic ----
    let unsupported_body = poll_for_marker(
        &client,
        &base_url,
        "/api/unsupported",
        "UNSUPPORTED_",
        &session,
    )
    .await;
    assert!(
        !unsupported_body.contains("UNSUPPORTED_UNEXPECTED_SUCCESS"),
        "crypto.subtle.encrypt() unexpectedly succeeded — it must fail closed \
         (divergence D8).\nbody:\n{unsupported_body}",
    );
    assert!(
        unsupported_body.contains("UNSUPPORTED_ERROR_NAME:NotSupportedError"),
        "crypto.subtle.encrypt() did not fail with NotSupportedError.\n\
         body:\n{unsupported_body}",
    );
    assert!(
        unsupported_body.contains("is not implemented in the zfb embedded runtime"),
        "crypto.subtle.encrypt()'s diagnostic did not name the zfb embedded runtime.\n\
         body:\n{unsupported_body}",
    );
    assert!(
        !unsupported_body.contains("SSG runtime"),
        "an unsupported-capability failure at REQUEST TIME leaked the build-time-only \
         \"fetch() called from SSG runtime\" wording — this is precisely the defect \
         epic #2012 exists to fix.\nbody:\n{unsupported_body}",
    );
}

// ---------------------------------------------------------------------------
// Case 5: build-time SSG still denies network access (guardrail 4)
// ---------------------------------------------------------------------------

/// `zfb build` over a project with NO `prerender = false` route (so no
/// SSR adapter is required — see the file-level "Scope note"), whose one
/// page calls `fetch()` at build time. The build must fail, and the
/// failure output must carry the byte-identical, deliberately-preserved
/// SSG denial message.
#[tokio::test(flavor = "multi_thread")]
async fn build_still_denies_network_at_ssg_time() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[embedded_host_request_time_e2e] no esbuild binary available; skipping \
             the build-time SSG-denial case. Set ZFB_ESBUILD_BIN to the pinned native \
             binary."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("create ssg fixture tempdir");
    let root = tmp
        .path()
        .canonicalize()
        .expect("canonicalize ssg fixture root");
    copy_dir(&ssg_fixture_dir(), &root).expect("copy embedded-host-request-time-ssg fixture");

    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    if !output.status.success()
        && (combined.contains("embed_v8") || combined.contains("no esbuild"))
    {
        eprintln!(
            "[embedded_host_request_time_e2e] `zfb build` exited with a known \
             environment-gate indicator; skipping.\nstdout: {stdout}\nstderr: {stderr}"
        );
        return;
    }

    assert!(
        !output.status.success(),
        "`zfb build` unexpectedly SUCCEEDED for a page that calls fetch() at build \
         time — the SSG network denial (guardrail 4 of epic #2012) is not being \
         enforced.\nstatus: {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status,
    );
    assert!(
        combined.contains("SSG_DENIAL_MARKER:fetch() called from SSG runtime"),
        "`zfb build` failed, but not with the expected build-time SSG denial \
         message.\nstatus: {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status,
    );
    assert!(
        combined.contains("does not support outgoing network requests during build-time render"),
        "the SSG denial message's body changed — this is deliberate policy \
         (guardrail 4) and its wording must not drift as collateral damage of the \
         request-time work.\nstatus: {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status,
    );
}
