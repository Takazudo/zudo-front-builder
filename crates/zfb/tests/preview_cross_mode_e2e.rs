//! Preview↔dev cross-mode acceptance E2E (issue #1547 — Confirm gate for
//! epic #1541 Preview Parity).
//!
//! ## What this test proves
//!
//! Waves 1-3 landed the pieces separately: a `previewMiddleware` plugin
//! hook, a Cloudflare-subset `_redirects` engine, a shared mode-aware
//! plugin-dispatch seam, and the wiring of all three into both `zfb dev`
//! and `zfb preview` (static). No test to date has proven that the two
//! integration points actually agree — this file boots BOTH a real
//! `zfb dev --port 0` and a real `zfb preview --port 0` (over a real `zfb
//! build` output) against the SAME fixture and asserts the SAME literal
//! outcomes against each one. If the two integration points ever drift —
//! one honouring a `_redirects` rule the other doesn't, or a plugin
//! handler behaving differently under `devMiddleware` vs
//! `previewMiddleware` — one mode's assertions fail while the other's
//! pass, instead of the drift going unnoticed.
//!
//! ## Fixture shape (`tests/fixtures/preview-cross-mode/`)
//!
//! - `preset.mjs` registers ONE handler (`echoHandler`) verbatim under
//!   both `devMiddleware` and `previewMiddleware` at `/api/echo` — proof
//!   the same plugin code produces the same response under both hooks.
//! - `public/_redirects` carries one plain redirect, one `200` rewrite
//!   (to a real rendered page, not a raw static asset), and one splat
//!   rule with `:splat` substitution — see that file for the exact rules.
//! - `pages/new-page.tsx` / `pages/rewritten.tsx` are the redirect/rewrite
//!   targets; `pages/index.tsx` is a plain home page.
//!
//! ## Scope decision — the dev-only SSR-route POST leg
//!
//! The sub-issue asks for a POST-to-a-dynamic-SSR-route (`prerender =
//! false`) scenario to document that the passthrough leg is DEV-ONLY:
//! `zfb preview` (static) never boots a V8 host or dispatches SSR at
//! request time, so there is architecturally nothing for such a route to
//! reach there. This fixture deliberately does NOT include a `prerender =
//! false` route: `zfb build` hard-fails
//! (`ensure_no_ssr_without_adapter`) for any project that has an SSR-only
//! route but no SSR-capable adapter configured, and this fixture's
//! `adapter: "none"` static-preview leg is exactly the case that check
//! guards. Adding one would either break the shared `zfb build` step this
//! test's preview half depends on, or require a SECOND fixture wired to
//! the Cloudflare adapter — which is precisely the domain of the
//! wrangler-dev consistency leg (tracked separately in the sub's closing
//! comment; unavailable in this sandboxed environment, see the PR/issue
//! discussion). Instead, the POST-bypass scenario below proves the
//! narrower, fixture-compatible half of the same claim: a POST to a URL
//! that WOULD match a `_redirects` rule under GET reaches neither the
//! rule nor a resurrected redirect in either mode — both fall through to
//! the identical `405 Method Not Allowed` (`Redirects::match_request` is
//! GET/HEAD-only; no plugin claims that path either).
//!
//! ## CI conventions
//!
//! Modeled on `crates/zfb/tests/dev_serve_e2e.rs` and
//! `crates/zfb/tests/dev_build_static_parity.rs`: own process group per
//! spawned server, stdout/stderr captured to files (never pipes),
//! group-kill on `Drop`, an overall wall-clock watchdog, and
//! `zfb_test_utils::CrossBinaryE2eLock` (issue #1339) acquired BEFORE any
//! other synchronization, held for the whole test (build + both spawned
//! servers) since this file has only one spawning test — see
//! `zfb-test-utils/src/cross_binary_lock.rs` for the lock-ordering
//! rationale. This binary is a new member of `.config/nextest.toml`'s
//! `e2e-heavy` test-group (flock-adopting bucket) — see that file's
//! comments and `CLAUDE.md`'s nextest inventory prose, both updated
//! alongside this test. NOT `#[ignore]`d: it runs in T1 (`health.yml`)
//! like its siblings.
//!
//! ## Determinism
//!
//! Readiness for both servers is an HTTP poll of `GET /` after the ready
//! banner is parsed ONLY for the ephemeral port (`--port 0`). Unlike
//! `dev_serve_e2e.rs` this test performs no file edits after boot, so it
//! needs neither the SSE watcher-live handshake nor tick-completion
//! signals — every assertion below is a synchronous fact about the boot
//! snapshot (the parsed `_redirects` ruleset, the registered plugin
//! handlers), so a short deadline-bounded poll (absorbing only
//! connection-establishment jitter right after boot) is correct, not a
//! race.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

/// Overall wall-clock watchdog covering the build step plus both spawned
/// servers plus all scenario assertions. Comfortably under
/// `CrossBinaryE2eLock`'s 360s acquire timeout (a slow-but-healthy run
/// here must never starve a sibling e2e binary queued on the lock) and
/// under the nextest `e2e-heavy` group's 600s terminate-after.
const OVERALL_DEADLINE: Duration = Duration::from_secs(240);

/// Deadline for `zfb dev` to print its ready banner and answer `GET /`
/// with 200 — boots a real V8 host + esbuild, generous for cold debug
/// builds (mirrors `dev_serve_e2e.rs::BOOT_DEADLINE`).
const DEV_BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// Deadline for `zfb preview` to print its ready banner and answer `GET
/// /` with 200 — no V8/esbuild boot, only a plugin-host Node subprocess
/// spawn, so this is far tighter than the dev boot deadline. Still
/// generous for a loaded CI runner.
const PREVIEW_BOOT_DEADLINE: Duration = Duration::from_secs(30);

/// Per-scenario deadline for an HTTP assertion after boot.
const SCENARIO_DEADLINE: Duration = Duration::from_secs(30);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy)]
enum CrossModeCase {
    Default,
    CustomBase,
}

impl CrossModeCase {
    fn preview_readiness_path(self) -> &'static str {
        "/"
    }

    fn dev_readiness_path(self) -> &'static str {
        match self {
            CrossModeCase::Default => "/",
            CrossModeCase::CustomBase => "/pj/site/",
        }
    }

    fn preview_label(self) -> &'static str {
        match self {
            CrossModeCase::Default => "preview",
            CrossModeCase::CustomBase => "preview-custom-base",
        }
    }

    fn dev_label(self) -> &'static str {
        match self {
            CrossModeCase::Default => "dev",
            CrossModeCase::CustomBase => "dev-custom-base",
        }
    }
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("preview-cross-mode")
}

/// `true` when `node` is on PATH (the plugin host needs it in both dev
/// and preview mode).
fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Recursive directory copy (same shape as `dev_serve_e2e.rs::copy_dir`).
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
}

fn configure_fixture_case(root: &Path, case: CrossModeCase) {
    match case {
        CrossModeCase::Default => {}
        CrossModeCase::CustomBase => {
            fs::write(
                root.join("zfb.config.json"),
                r#"{
  "framework": "preact",
  "base": "/pj/site/",
  "plugins": [{ "name": "./preset.mjs" }]
}
"#,
            )
            .expect("write custom-base zfb.config.json");
            fs::write(
                root.join("public/_redirects"),
                "\
# Custom-base parity rules: spec-form sources include the base prefix.
/pj/site/old-page /pj/site/new-page 301
/pj/site/rewrite-me /rewritten/?literal=1 200
/pj/site/blog/* /pj/site/new-page?src=:splat 301
",
            )
            .expect("write custom-base _redirects");
        }
    }
}

/// Owns one spawned `zfb <subcommand>` process (dev OR preview). Drop
/// group-kills the whole process group so both the dev server / preview
/// server AND anything it spawned (V8, esbuild, the plugin host) are
/// reaped on success, assertion-failure, and watchdog-timeout paths
/// alike.
struct ChildGuard {
    child: std::process::Child,
    pgid: libc::pid_t,
}

impl ChildGuard {
    fn try_exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().expect("try_wait on spawned `zfb`")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Best-effort: ESRCH (already gone) is harmless.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

/// One spawned `zfb dev` or `zfb preview` session: process guard + log
/// paths + a label for panic messages.
struct ServerSession {
    label: &'static str,
    guard: ChildGuard,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

fn read_log(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_default()
}

impl ServerSession {
    fn logs(&self) -> String {
        format!(
            "--- {} stdout ---\n{}\n--- {} stderr ---\n{}",
            self.label,
            read_log(&self.stdout_path),
            self.label,
            read_log(&self.stderr_path),
        )
    }
}

/// Extract the port from a ready banner shared by both `zfb dev` and `zfb
/// preview` (`→ ready on http://localhost:PORT/`, or the multi-line
/// `Local: http://localhost:PORT/` form). Tolerates ANSI styling around
/// the URL and skips unrelated `http://` occurrences by scanning every
/// match until one yields a parseable port (identical heuristic to
/// `dev_serve_e2e.rs::parse_ready_port`).
fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(idx) = rest.find("http://") {
        let candidate = &rest[idx + "http://".len()..];
        let token: &str = candidate.split_whitespace().next().unwrap_or("");
        if let Some(colon) = token.find(':') {
            let digits: String = token[colon + 1..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
        rest = &rest[idx + "http://".len()..];
    }
    None
}

/// Spawn `zfb <args>` over `root`, in its own process group, with
/// stdout/stderr redirected to files (NEVER pipes — a piped child that
/// outgrows the OS pipe buffer blocks on write and masquerades as a
/// hang, `build_terminates.rs` pattern).
fn spawn_zfb(
    root: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    label: &'static str,
) -> ServerSession {
    let stdout_path = root.join(format!(".zfb-{label}-stdout.log"));
    let stderr_path = root.join(format!(".zfb-{label}-stderr.log"));
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");

    let mut cmd = Command::new(zfb_binary!());
    cmd.args(args)
        .current_dir(root)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    // Strip any inherited lazy-dev-render switches — irrelevant to
    // `preview` but harmless to strip unconditionally, and load-bearing
    // for `dev` (mirrors dev_serve_e2e.rs::spawn_dev).
    cmd.env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    // New process group (PGID == child PID) so kill(-pgid, SIGKILL) reaps
    // the server plus any helper process it spawned (V8/esbuild for dev,
    // the plugin-host Node subprocess for both).
    cmd.process_group(0);

    let child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("spawn `zfb {args:?}`: {e}"));
    let pgid = child.id() as libc::pid_t;
    ServerSession {
        label,
        guard: ChildGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

/// Wait for the ready banner and confirm the case's readiness URL
/// answers 200. Panics if the process exits prematurely, or the deadline
/// is exceeded, with the captured logs attached — there is no
/// "known-skip indicator" branch here (unlike `dev_serve_e2e.rs`): by
/// the time this is called, `locate_esbuild()`/`node_available()` have
/// already gated the whole test, so a premature exit is a real failure,
/// not an environment gap.
async fn boot_and_get_port(
    session: &mut ServerSession,
    client: &reqwest::Client,
    boot_deadline: Duration,
    readiness_path: &str,
) -> u16 {
    let boot_start = Instant::now();
    let port = loop {
        if let Some(status) = session.guard.try_exit_status() {
            panic!(
                "`{}` exited prematurely (status {status:?}) before printing the ready \
                 banner.\n{}",
                session.label,
                session.logs(),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
            assert_ne!(
                port,
                0,
                "`{}` ready banner printed port 0 — the `--port 0` actual-bound-port \
                 contract regressed.\n{}",
                session.label,
                session.logs(),
            );
            break port;
        }
        assert!(
            boot_start.elapsed() < boot_deadline,
            "`{}` did not print a parseable ready banner within {}s.\n{}",
            session.label,
            boot_deadline.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    let base = format!("http://localhost:{port}");
    let start = Instant::now();
    loop {
        if let Ok(resp) = client.get(format!("{base}{readiness_path}")).send().await {
            if resp.status().as_u16() == 200 {
                return port;
            }
        }
        assert!(
            start.elapsed() < boot_deadline,
            "`{}` GET {readiness_path} never answered 200 within {}s after the ready banner.\n{}",
            session.label,
            boot_deadline.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// ---------------------------------------------------------------------------
// Level-3 build step (no HTTP — `zfb build` as a blocking subprocess)
// ---------------------------------------------------------------------------

/// Run `zfb build` against `root`. Returns `false` when the process exits
/// with a known-skip indicator (V8/esbuild absent) — matches the
/// `dev_build_static_parity.rs::run_build` convention. Sync on purpose;
/// the caller hops to `spawn_blocking` so the async runtime isn't parked.
fn run_build(root: &Path, esbuild: &Path) -> bool {
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    if !output.status.success() {
        if combined.contains("embed_v8") || combined.contains("no esbuild") {
            eprintln!(
                "[preview_cross_mode_e2e] `zfb build` exited with known-skip indicator; \
                 skipping.\nstdout: {stdout}\nstderr: {stderr}"
            );
            return false;
        }
        panic!(
            "`zfb build` failed unexpectedly.\nstatus: {:?}\nstdout: {stdout}\nstderr: {stderr}",
            output.status,
        );
    }
    true
}

// ---------------------------------------------------------------------------
// Scenario assertion helpers
// ---------------------------------------------------------------------------

/// Poll GET `url` until it answers `expected_status` with EXACTLY
/// `expected_location` in the `Location` header. Used for the redirect
/// rules (plain + splat) — never for the rewrite rule, which serves
/// content directly with no `Location` header.
async fn poll_get_redirect(
    client: &reqwest::Client,
    url: &str,
    expected_status: u16,
    expected_location: &str,
    deadline: Duration,
    phase: &str,
    session: &ServerSession,
) {
    let start = Instant::now();
    let mut last = String::from("(no response yet)");
    while start.elapsed() < deadline {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let location = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                if status == expected_status && location == expected_location {
                    return;
                }
                last = format!("status {status}, Location {location:?}");
            }
            Err(e) => last = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{}][{phase}] GET {url} did not answer {expected_status} with Location \
         {expected_location:?} within {}s. Last: {last}\n{}",
        session.label,
        deadline.as_secs(),
        session.logs(),
    );
}

/// Poll GET `url` until it answers `expected_status` with a body
/// containing `needle`.
async fn poll_get_contains(
    client: &reqwest::Client,
    url: &str,
    expected_status: u16,
    needle: &str,
    deadline: Duration,
    phase: &str,
    session: &ServerSession,
) {
    let start = Instant::now();
    let mut last = String::from("(no response yet)");
    while start.elapsed() < deadline {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if status == expected_status && body.contains(needle) {
                    return;
                }
                last = format!("status {status}, body:\n{body}");
            }
            Err(e) => last = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{}][{phase}] GET {url} did not serve status={expected_status} containing \
         {needle:?} within {}s. Last: {last}\n{}",
        session.label,
        deadline.as_secs(),
        session.logs(),
    );
}

/// Poll GET `url` until it answers `expected_status`, regardless of body
/// content.
async fn poll_get_status(
    client: &reqwest::Client,
    url: &str,
    expected_status: u16,
    deadline: Duration,
    phase: &str,
    session: &ServerSession,
) {
    let start = Instant::now();
    let mut last = String::from("(no response yet)");
    while start.elapsed() < deadline {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == expected_status {
                    return;
                }
                last = format!("status {status}");
            }
            Err(e) => last = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{}][{phase}] GET {url} did not answer {expected_status} within {}s. Last: {last}\n{}",
        session.label,
        deadline.as_secs(),
        session.logs(),
    );
}

/// Poll POST `url` (empty body) until it answers `expected_status`,
/// regardless of body content.
async fn poll_post_status(
    client: &reqwest::Client,
    url: &str,
    expected_status: u16,
    deadline: Duration,
    phase: &str,
    session: &ServerSession,
) {
    let start = Instant::now();
    let mut last = String::from("(no response yet)");
    while start.elapsed() < deadline {
        match client.post(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == expected_status {
                    return;
                }
                last = format!("status {status}");
            }
            Err(e) => last = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{}][{phase}] POST {url} did not answer {expected_status} within {}s. \
         Last: {last}\n{}",
        session.label,
        deadline.as_secs(),
        session.logs(),
    );
}

/// Poll POST `url` (empty body) until it answers `expected_status` with a
/// body containing `needle`.
async fn poll_post_contains(
    client: &reqwest::Client,
    url: &str,
    expected_status: u16,
    needle: &str,
    deadline: Duration,
    phase: &str,
    session: &ServerSession,
) {
    let start = Instant::now();
    let mut last = String::from("(no response yet)");
    while start.elapsed() < deadline {
        match client.post(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if status == expected_status && body.contains(needle) {
                    return;
                }
                last = format!("status {status}, body:\n{body}");
            }
            Err(e) => last = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{}][{phase}] POST {url} did not serve status={expected_status} containing \
         {needle:?} within {}s. Last: {last}\n{}",
        session.label,
        deadline.as_secs(),
        session.logs(),
    );
}

// ---------------------------------------------------------------------------
// Shared scenarios — run VERBATIM against both `zfb dev` and `zfb
// preview`. Asserting the SAME literal expected values against each
// mode's base URL is how this test proves cross-mode parity: if the two
// integration points ever diverge, one mode's assertions fail while the
// other's pass.
// ---------------------------------------------------------------------------

/// GET assertions for all three `_redirects` rules, including
/// query-string preservation, plus the plugin handler's GET response.
async fn run_shared_get_scenarios(client: &reqwest::Client, base: &str, session: &ServerSession) {
    // Rule 1 — plain redirect. Query string is preserved verbatim
    // (appended with `?` since the target carries none of its own).
    poll_get_redirect(
        client,
        &format!("{base}/old-page?x=1"),
        301,
        "/new-page?x=1",
        SCENARIO_DEADLINE,
        "rule 1: plain redirect preserves the query string",
        session,
    )
    .await;

    // Rule 2 — 200 rewrite. Serves the rewritten page's content directly
    // (no Location header, no status change); the request's own query
    // string is ignored per the `_redirects` "rewrites ignore the
    // request's query" contract, since the client-visible URL never
    // changes.
    poll_get_contains(
        client,
        &format!("{base}/rewrite-me?extra=1"),
        200,
        "CROSS_MODE_REWRITTEN_MARKER",
        SCENARIO_DEADLINE,
        "rule 2: 200 rewrite serves the target page's content",
        session,
    )
    .await;

    // Rule 3 — splat rule. Proves multi-segment source capture, `:splat`
    // substitution into the target, AND query-string preservation merged
    // with `&` (the target already carries its own `?src=...`).
    poll_get_redirect(
        client,
        &format!("{base}/blog/2024/07/hello?x=1"),
        301,
        "/new-page?src=2024/07/hello&x=1",
        SCENARIO_DEADLINE,
        "rule 3: splat capture + substitution + query merge",
        session,
    )
    .await;

    // The plugin handler itself, over GET — same handler function as the
    // POST case below, registered under `devMiddleware`/`previewMiddleware`
    // respectively.
    poll_get_contains(
        client,
        &format!("{base}/api/echo"),
        200,
        "CROSS_MODE_PLUGIN_MARKER method=GET",
        SCENARIO_DEADLINE,
        "plugin handler responds to GET",
        session,
    )
    .await;
}

/// Custom-base GET assertions for issue #1594. The rule sources are
/// authored in Cloudflare/preview spec form, so they include `/pj/site`.
/// Dev must reconstruct that full source path before matching, while
/// preview naturally feeds the full request path to the same matcher.
async fn run_custom_base_get_scenarios(
    client: &reqwest::Client,
    base: &str,
    session: &ServerSession,
) {
    poll_get_redirect(
        client,
        &format!("{base}/pj/site/old-page?x=1"),
        301,
        "/pj/site/new-page?x=1",
        SCENARIO_DEADLINE,
        "custom base: redirect preserves exact base-prefixed Location + query",
        session,
    )
    .await;

    poll_get_contains(
        client,
        &format!("{base}/pj/site/rewrite-me?extra=1"),
        200,
        "CROSS_MODE_REWRITTEN_MARKER",
        SCENARIO_DEADLINE,
        "custom base: 200 rewrite matches a base-prefixed source before serving target content",
        session,
    )
    .await;

    poll_get_redirect(
        client,
        &format!("{base}/pj/site/blog/2024/07/hello?x=1"),
        301,
        "/pj/site/new-page?src=2024/07/hello&x=1",
        SCENARIO_DEADLINE,
        "custom base: splat capture + exact base-prefixed Location",
        session,
    )
    .await;
}

/// POST assertions: `_redirects` bypass (no rule fires for a non-GET/HEAD
/// method) and the shared plugin handler's POST response.
async fn run_shared_post_scenarios(client: &reqwest::Client, base: &str, session: &ServerSession) {
    // `/old-page` matches rule 1 under GET (asserted above). Under POST,
    // `Redirects::match_request` is GET/HEAD-only, so the rule must NOT
    // fire — and no plugin claims `/old-page` either, so both `zfb dev`
    // and `zfb preview` fall through to the identical `405 Method Not
    // Allowed` (dev: the page-cache-fallback method gate in
    // `zfb-server::routes::serve_page`; preview: the equivalent gate in
    // `serve_preview_request`). A 301 here would mean `_redirects` wrongly
    // fired for a non-GET/HEAD request.
    poll_post_status(
        client,
        &format!("{base}/old-page"),
        405,
        SCENARIO_DEADLINE,
        "POST /old-page bypasses _redirects (no rule fires; both modes 405 identically)",
        session,
    )
    .await;

    // The SAME plugin handler, over POST — proof `devMiddleware` and
    // `previewMiddleware` both dispatch every HTTP method to the
    // registered handler, not just GET/HEAD (issue #230 parity, reused
    // here for the preview hook).
    poll_post_contains(
        client,
        &format!("{base}/api/echo"),
        200,
        "CROSS_MODE_PLUGIN_MARKER method=POST",
        SCENARIO_DEADLINE,
        "POST /api/echo reaches the shared plugin handler",
        session,
    )
    .await;
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

async fn run_cross_mode_case(
    case: CrossModeCase,
    client: &reqwest::Client,
    esbuild: &Path,
) -> bool {
    // ── Build once, for the preview half ──────────────────────────
    let preview_tmp = tempfile::tempdir().expect("tempdir for preview root");
    let preview_root = preview_tmp
        .path()
        .canonicalize()
        .expect("canonicalize preview root");
    copy_dir(&fixture_dir(), &preview_root).expect("copy fixture into preview root");
    configure_fixture_case(&preview_root, case);

    let build_ok = {
        let root = preview_root.clone();
        let esbuild = esbuild.to_path_buf();
        tokio::task::spawn_blocking(move || run_build(&root, &esbuild))
            .await
            .expect("join zfb build task")
    };
    if !build_ok {
        return false; // known-skip, already logged by run_build
    }

    // ── Dev gets its OWN fresh, unbuilt copy of the fixture ───────
    let dev_tmp = tempfile::tempdir().expect("tempdir for dev root");
    let dev_root = dev_tmp
        .path()
        .canonicalize()
        .expect("canonicalize dev root");
    copy_dir(&fixture_dir(), &dev_root).expect("copy fixture into dev root");
    configure_fixture_case(&dev_root, case);

    // ── Preview half ───────────────────────────────────────────────
    let mut preview_session = spawn_zfb(
        &preview_root,
        &["preview", "--port", "0"],
        &[],
        case.preview_label(),
    );
    let preview_port = boot_and_get_port(
        &mut preview_session,
        client,
        PREVIEW_BOOT_DEADLINE,
        case.preview_readiness_path(),
    )
    .await;
    let preview_base = format!("http://localhost:{preview_port}");

    match case {
        CrossModeCase::Default => {
            run_shared_get_scenarios(client, &preview_base, &preview_session).await;
            run_shared_post_scenarios(client, &preview_base, &preview_session).await;

            // Sanity: an unmatched route 404s cleanly (not a panic/500)
            // — cheap belt-and-suspenders check that the
            // redirects/plugin wiring above didn't swallow the ordinary
            // static-file waterfall.
            poll_get_status(
                client,
                &format!("{preview_base}/nonexistent-route-for-404-check"),
                404,
                SCENARIO_DEADLINE,
                "sanity: an unmatched route 404s (not a panic/500)",
                &preview_session,
            )
            .await;
        }
        CrossModeCase::CustomBase => {
            run_custom_base_get_scenarios(client, &preview_base, &preview_session).await;
        }
    }

    // Explicitly release the preview server before booting dev — the
    // two never need to run concurrently, and shutting one down first
    // keeps the failure logs for whichever phase is running unambiguous.
    drop(preview_session);

    // ── Dev half — same scenarios, same expected literal values ────
    let mut dev_session = spawn_zfb(
        &dev_root,
        &["dev", "--port", "0"],
        &[(
            "ZFB_ESBUILD_BIN",
            esbuild.to_str().expect("esbuild path is UTF-8"),
        )],
        case.dev_label(),
    );
    let dev_port = boot_and_get_port(
        &mut dev_session,
        client,
        DEV_BOOT_DEADLINE,
        case.dev_readiness_path(),
    )
    .await;
    let dev_base = format!("http://localhost:{dev_port}");

    match case {
        CrossModeCase::Default => {
            run_shared_get_scenarios(client, &dev_base, &dev_session).await;
            run_shared_post_scenarios(client, &dev_base, &dev_session).await;
        }
        CrossModeCase::CustomBase => {
            run_custom_base_get_scenarios(client, &dev_base, &dev_session).await;
        }
    }

    true
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_and_dev_agree_on_redirects_and_plugin_middleware() {
    // Cross-binary lock acquired BEFORE anything else (issue #1339) — see
    // the lock-ordering note in zfb-test-utils/src/cross_binary_lock.rs.
    // Held for the whole test (build + both spawned servers): this file
    // has only one spawning test, so there is no in-binary SERIAL mutex
    // to layer underneath it.
    let _e2e_lock = CrossBinaryE2eLock::acquire();

    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[preview_cross_mode_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };
    if !node_available() {
        eprintln!(
            "[preview_cross_mode_e2e] node not on PATH; skipping (the plugin host needs it)."
        );
        return;
    }

    let overall = async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("build reqwest client");

        if !run_cross_mode_case(CrossModeCase::Default, &client, &esbuild).await {
            return;
        }
        run_cross_mode_case(CrossModeCase::CustomBase, &client, &esbuild).await;
    };

    tokio::time::timeout(OVERALL_DEADLINE, overall)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "[watchdog] preview/dev cross-mode E2E did not finish within {}s — this \
                 indicates a hang (both spawned process groups are killed by their guards' \
                 Drop regardless of how this panic unwinds).",
                OVERALL_DEADLINE.as_secs(),
            )
        });
}
