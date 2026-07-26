//! Sub #1994 (Page Extension Contract epic #1990, Wave 4 — the central
//! confirm pass) — a single fixture covering ALL SEVEN routable page
//! shapes plus a dynamic `.ts` page, driven through a real `zfb build`
//! AND a real `zfb dev` session, plus a fast cross-layer drift guard.
//!
//! ## Why a NEW binary instead of extending `page_extension_route_table_build.rs`
//!
//! Wave 3's `page_extension_route_table_build.rs` already proves `.ts` /
//! `.js` / `.jsx` reach the production route table via a real `zfb
//! build` — but it is a BUILD-ONLY binary (nextest's build-command
//! bucket, no `zfb dev` spawn, no OS-level `CrossBinaryE2eLock`
//! adoption). This sub-issue additionally requires a `zfb dev` pass, a
//! `.md`/`.mdx`/`.html`/dynamic-`.ts` widened fixture, and a fast
//! grep-level cross-layer drift guard. Adding a `zfb dev` spawn to the
//! existing binary would silently change its resource profile (from
//! "one V8+esbuild boot" to "boot, tear down, boot again") without
//! updating its own header comment or its narrower Wave-3 scope
//! (`.ts`/`.js`/`.jsx` only). A new binary keeps Wave 3's test doing
//! exactly what it says, and lets this file be explicit about being a
//! `CrossBinaryE2eLock`-adopting, build-THEN-dev binary from the start —
//! this file joins `.config/nextest.toml`'s `e2e-heavy` group as a NEW
//! flock-adopting member (not the build-only bucket Wave 3's binary
//! lives in).
//!
//! ## What this proves
//!
//! - All seven `zfb_types::ROUTABLE_PAGE_EXTENSIONS` shapes
//!   (`tsx`/`ts`/`jsx`/`js`/`mdx`/`md`/`html`) plus a DYNAMIC `.ts` page
//!   (`[slug].ts`, resolving a `paths()` export exactly like `[slug].tsx`
//!   would) reach both the production route table (`zfb build` → emitted
//!   `dist/`) and the dev server (`zfb dev` → live HTTP).
//! - No `pages/ file has an unrecognised extension and will be skipped`
//!   warning fires for any of the seven, in either mode — the absence of
//!   the warning is as load-bearing as the presence of the route.
//! - `crates/zfb-router/src/scan.rs` and `crates/zfb-build/src/bundler.rs`
//!   both read the shared `zfb_types::ROUTABLE_PAGE_EXTENSIONS` /
//!   `SCRIPT_PAGE_EXTENSIONS` subsets rather than carrying an independent
//!   literal allowlist — this is #1742's third expected outcome ("add a
//!   cross-layer command test so the two allowlists cannot drift again")
//!   and proving it landed is this sub-issue's job. That check needs no
//!   esbuild/V8 and runs as an ordinary (non-`#[ignore]`d) test.
//!
//! ## CI conventions
//!
//! Modeled on `preview_cross_mode_e2e.rs`: own process group per spawned
//! server, stdout/stderr captured to files (never pipes), group-kill on
//! `Drop`, an overall wall-clock watchdog, and
//! `zfb_test_utils::CrossBinaryE2eLock` (issue #1339) acquired BEFORE
//! anything else — this file has only one spawning test, so there is no
//! in-binary `SERIAL` mutex to layer underneath it.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

/// Overall wall-clock watchdog covering the build step plus the spawned
/// dev server plus all scenario assertions. Comfortably under
/// `CrossBinaryE2eLock`'s 360s acquire timeout and under the nextest
/// `e2e-heavy` group's 600s terminate-after.
const OVERALL_DEADLINE: Duration = Duration::from_secs(240);

/// Deadline for `zfb dev` to print its ready banner and answer `GET /`
/// with 200 — boots a real V8 host + esbuild.
const DEV_BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// Per-scenario deadline for an HTTP assertion after boot.
const SCENARIO_DEADLINE: Duration = Duration::from_secs(30);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Fixture — all seven routable page shapes plus a dynamic `.ts` page.
// ---------------------------------------------------------------------------

/// Content markers, one per shape, so each assertion can be unambiguous
/// about which page source produced the served/emitted bytes.
const TSX_MARKER: &str = "MARKER_TSX_INDEX";
const PLAIN_TS_MARKER: &str = "MARKER_PLAIN_TS";
const SCRIPT_JS_MARKER: &str = "MARKER_SCRIPT_JS";
const COMPONENT_JSX_MARKER: &str = "MARKER_COMPONENT_JSX";
const DOC_MDX_MARKER: &str = "MARKER_DOC_MDX";
const NOTE_MD_MARKER: &str = "MARKER_NOTE_MD";
const STATIC_HTML_MARKER: &str = "MARKER_STATIC_HTML";
const DYNAMIC_TS_ALPHA_MARKER: &str = "MARKER_DYNAMIC_TS_alpha";
const DYNAMIC_TS_BETA_MARKER: &str = "MARKER_DYNAMIC_TS_beta";

fn write_fixture(root: &Path) {
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact" }
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("pages")).unwrap();

    // `pages/index.tsx` — the ordinary, always-supported shape.
    fs::write(
        root.join("pages/index.tsx"),
        format!(
            r#"export default function Page() {{
  return (
    <html lang="en">
      <head><title>tsx page</title></head>
      <body><p>{TSX_MARKER}</p></body>
    </html>
  );
}}
"#
        ),
    )
    .unwrap();

    // `pages/plain.ts` — plain `.ts`, no JSX syntax; uses preact's `h()`
    // directly (same convention as Wave 3's
    // `page_extension_route_table_build.rs`).
    fs::write(
        root.join("pages/plain.ts"),
        format!(
            r#"import {{ h }} from "preact";

export default function Page() {{
  return h(
    "html",
    {{ lang: "en" }},
    h("head", null, h("title", null, "ts page")),
    h("body", null, h("p", null, "{PLAIN_TS_MARKER}")),
  );
}}
"#
        ),
    )
    .unwrap();

    // `pages/script.js` — plain `.js`.
    fs::write(
        root.join("pages/script.js"),
        format!(
            r#"import {{ h }} from "preact";

export default function Page() {{
  return h(
    "html",
    {{ lang: "en" }},
    h("head", null, h("title", null, "js page")),
    h("body", null, h("p", null, "{SCRIPT_JS_MARKER}")),
  );
}}
"#
        ),
    )
    .unwrap();

    // `pages/component.jsx` — `.jsx` DOES support JSX syntax; this
    // fixture deliberately still uses `h()` so it doesn't accidentally
    // depend on JSX-transform behavior `.tsx` already covers elsewhere
    // (same rationale as Wave 3's fixture).
    fs::write(
        root.join("pages/component.jsx"),
        format!(
            r#"import {{ h }} from "preact";

export default function Page() {{
  return h(
    "html",
    {{ lang: "en" }},
    h("head", null, h("title", null, "jsx page")),
    h("body", null, h("p", null, "{COMPONENT_JSX_MARKER}")),
  );
}}
"#
        ),
    )
    .unwrap();

    // `pages/doc.mdx` — MDX page source, compiled directly (no shell
    // wrapper, unlike `.md`).
    fs::write(
        root.join("pages/doc.mdx"),
        format!(
            r#"---
title: MDX doc page
---

# {DOC_MDX_MARKER}
"#
        ),
    )
    .unwrap();

    // `pages/note.md` — plain Markdown page source, compiled + wrapped in
    // the minimal HTML shell (`render_md_page_shell`).
    fs::write(
        root.join("pages/note.md"),
        format!(
            r#"---
title: Note page
---

# {NOTE_MD_MARKER}
"#
        ),
    )
    .unwrap();

    // `pages/static.html` — static HTML page source: bypasses the JS
    // bundle entirely and is emitted verbatim (minus frontmatter).
    fs::write(
        root.join("pages/static.html"),
        format!(
            r#"---
title: Static page
---
<!doctype html>
<html>
  <head><title>static page</title></head>
  <body><p>{STATIC_HTML_MARKER}</p></body>
</html>
"#
        ),
    )
    .unwrap();

    // `pages/[slug].ts` — DYNAMIC `.ts` page: must resolve `paths()` and
    // expand exactly like `[slug].tsx` would. Two slugs so expansion
    // (not just single-route acceptance) is proven.
    fs::write(
        root.join("pages/[slug].ts"),
        format!(
            r#"import {{ h }} from "preact";

export function paths() {{
  return [
    {{ params: {{ slug: "alpha" }}, props: {{ marker: "{DYNAMIC_TS_ALPHA_MARKER}" }} }},
    {{ params: {{ slug: "beta" }}, props: {{ marker: "{DYNAMIC_TS_BETA_MARKER}" }} }},
  ];
}}

export default function Page({{ marker }}) {{
  return h(
    "html",
    {{ lang: "en" }},
    h("head", null, h("title", null, "dynamic ts page")),
    h("body", null, h("p", null, marker)),
  );
}}
"#
        ),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Level-3 build step (blocking subprocess, no HTTP)
// ---------------------------------------------------------------------------

fn is_known_skip(combined: &str) -> bool {
    combined.contains("embed_v8")
        || combined.contains("no esbuild")
        || combined.contains("no tailwind")
        || (combined.contains("tailwindcss") && combined.contains("not found"))
}

/// Wall-clock deadline for the `zfb build` subprocess itself, comfortably
/// under `OVERALL_DEADLINE` (240s) so a hang is caught — and the process
/// group killed — well before the outer `tokio::time::timeout` would
/// otherwise abandon the `spawn_blocking` join handle while
/// `Command::output()` (used by an earlier revision of this test) kept
/// the child running unbounded (codex review finding, issue #1994).
const BUILD_DEADLINE: Duration = Duration::from_secs(180);

/// Owns the spawned `zfb build` process. Drop group-kills the whole
/// process group so a hung build can never outlive this test, whether
/// the deadline poll loop below catches it first or the test panics /
/// unwinds for any other reason.
struct BuildGuard {
    child: std::process::Child,
    pgid: libc::pid_t,
}

impl Drop for BuildGuard {
    fn drop(&mut self) {
        // Best-effort: ESRCH (already gone) is harmless.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

/// Runs `zfb build` against `root`. Returns `None` on a known-skip
/// indicator (V8/esbuild unavailable — already logged), `Some(stderr)`
/// on success so the caller can assert on the absence of the
/// "unrecognised extension" warning.
///
/// Spawned in its own process group with stdout/stderr captured to
/// files (never `Command::output()`'s pipes, and never an unbounded
/// blocking wait): a poll loop with `BUILD_DEADLINE` guarantees this
/// synchronous function always returns and the child is always killed,
/// even if the build subprocess hangs — `Command::output()` alone would
/// block this `spawn_blocking` thread (and keep the child alive)
/// indefinitely past the outer async watchdog.
fn run_build(root: &Path, esbuild: &Path) -> Option<String> {
    let stdout_path = root.join(".zfb-build-stdout.log");
    let stderr_path = root.join(".zfb-build-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create build stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create build stderr log file");

    let mut cmd = Command::new(zfb_binary!());
    cmd.arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    cmd.process_group(0);

    let child = cmd.spawn().expect("spawn `zfb build`");
    let pgid = child.id() as libc::pid_t;
    let mut guard = BuildGuard { child, pgid };

    let start = Instant::now();
    let status = loop {
        if let Some(status) = guard.child.try_wait().expect("try_wait on `zfb build`") {
            break status;
        }
        if start.elapsed() >= BUILD_DEADLINE {
            panic!(
                "`zfb build` did not exit within {}s — treating as a hang; process group \
                 killed by BuildGuard's Drop.\nstdout: {}\nstderr: {}",
                BUILD_DEADLINE.as_secs(),
                read_log(&stdout_path),
                read_log(&stderr_path),
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    let stdout = read_log(&stdout_path);
    let stderr = read_log(&stderr_path);
    let combined = format!("{stdout}{stderr}");

    if !status.success() {
        if is_known_skip(&combined) {
            eprintln!(
                "[page_extension_full_matrix_e2e] `zfb build` exited with a known-skip \
                 indicator; skipping.\nstdout: {stdout}\nstderr: {stderr}"
            );
            return None;
        }
        panic!(
            "`zfb build` failed unexpectedly for the seven-shape fixture.\n\
             status: {status:?}\nstdout: {stdout}\nstderr: {stderr}",
        );
    }
    Some(stderr)
}

fn read_dist_html(dist: &Path, rel: &str) -> String {
    let path = dist.join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "expected emitted page at {}: {e}\ndist/ contents: {:#?}",
            path.display(),
            fs::read_dir(dist)
                .ok()
                .map(|rd| rd.flatten().map(|e| e.path()).collect::<Vec<_>>())
        )
    })
}

fn assert_build_route(dist: &Path, rel: &str, marker: &str, shape: &str) {
    let html = read_dist_html(dist, rel);
    assert!(
        html.contains(marker),
        "{shape} page content ({marker:?}) must reach the emitted HTML at {rel}\n\
         --- html ---\n{html}"
    );
}

/// Runs the build half of the confirm pass over a fresh fixture copy.
/// Returns `false` on known-skip (already logged).
fn run_build_phase(root: &Path, esbuild: &Path) -> bool {
    write_fixture(root);

    let Some(stderr) = run_build(root, esbuild) else {
        return false;
    };

    let dist = root.join("dist");

    assert_build_route(&dist, "index.html", TSX_MARKER, ".tsx");
    assert_build_route(&dist, "plain/index.html", PLAIN_TS_MARKER, ".ts");
    assert_build_route(&dist, "script/index.html", SCRIPT_JS_MARKER, ".js");
    assert_build_route(&dist, "component/index.html", COMPONENT_JSX_MARKER, ".jsx");
    assert_build_route(&dist, "doc/index.html", DOC_MDX_MARKER, ".mdx");
    assert_build_route(&dist, "note/index.html", NOTE_MD_MARKER, ".md");
    assert_build_route(&dist, "static/index.html", STATIC_HTML_MARKER, ".html");

    // Dynamic `.ts` page — both `paths()`-enumerated slugs must expand,
    // exactly like a `[slug].tsx` sibling would.
    assert_build_route(
        &dist,
        "alpha/index.html",
        DYNAMIC_TS_ALPHA_MARKER,
        "dynamic .ts (alpha)",
    );
    assert_build_route(
        &dist,
        "beta/index.html",
        DYNAMIC_TS_BETA_MARKER,
        "dynamic .ts (beta)",
    );

    // No spurious "unrecognised extension" warning for any of the seven
    // shapes — a regression here would mean one of them silently fell
    // through the router's accepted-extension gate on the production
    // build path.
    assert!(
        !stderr.contains("pages/ file has an unrecognised extension"),
        "expected no `unrecognised extension` warnings for the seven-shape fixture; \
         got stderr:\n{stderr}"
    );

    true
}

// ---------------------------------------------------------------------------
// Level-4 dev step (real `zfb dev`, HTTP polling)
// ---------------------------------------------------------------------------

/// Owns the spawned `zfb dev` process. Drop group-kills the whole
/// process group, so the dev server (and anything it spawned — V8,
/// esbuild) is reaped on success, assertion-failure, and
/// watchdog-timeout paths alike.
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
        // Best-effort: ESRCH (already gone) is harmless.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

fn read_log(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_default()
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

/// Extract the port from the dev ready banner (identical heuristic to
/// `dev_serve_e2e.rs::parse_ready_port` / `preview_cross_mode_e2e.rs`).
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

/// Spawn `zfb dev --port 0` over `root`, in its own process group, with
/// stdout/stderr redirected to files (never pipes — a piped child that
/// outgrows the OS pipe buffer blocks on write and masquerades as a
/// hang, `build_terminates.rs` pattern).
fn spawn_dev(root: &Path, esbuild: &Path) -> DevSession {
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");

    let mut cmd = Command::new(zfb_binary!());
    cmd.arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    cmd.env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS");
    cmd.process_group(0);

    let child = cmd.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

async fn boot_and_get_port(session: &mut DevSession, client: &reqwest::Client) -> u16 {
    let boot_start = Instant::now();
    let port = loop {
        if let Some(status) = session.guard.try_exit_status() {
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before printing the ready \
                 banner.\n{}",
                session.logs(),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&session.stdout_path)) {
            assert_ne!(
                port,
                0,
                "`zfb dev` ready banner printed port 0 — the `--port 0` actual-bound-port \
                 contract regressed.\n{}",
                session.logs(),
            );
            break port;
        }
        assert!(
            boot_start.elapsed() < DEV_BOOT_DEADLINE,
            "`zfb dev` did not print a parseable ready banner within {}s.\n{}",
            DEV_BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    let base = format!("http://localhost:{port}");
    let start = Instant::now();
    loop {
        if let Ok(resp) = client.get(format!("{base}/")).send().await {
            if resp.status().as_u16() == 200 {
                return port;
            }
        }
        assert!(
            start.elapsed() < DEV_BOOT_DEADLINE,
            "`zfb dev` GET / never answered 200 within {}s after the ready banner.\n{}",
            DEV_BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll GET `url` until it answers 200 with a body containing `needle`.
async fn poll_get_contains(
    client: &reqwest::Client,
    url: &str,
    needle: &str,
    session: &DevSession,
) {
    let start = Instant::now();
    let mut last = String::from("(no response yet)");
    while start.elapsed() < SCENARIO_DEADLINE {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if status == 200 && body.contains(needle) {
                    return;
                }
                last = format!("status {status}, body:\n{body}");
            }
            Err(e) => last = format!("request error: {e}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "GET {url} did not serve 200 containing {needle:?} within {}s. Last: {last}\n{}",
        SCENARIO_DEADLINE.as_secs(),
        session.logs(),
    );
}

/// Runs the dev half of the confirm pass over a fresh fixture copy.
async fn run_dev_phase(root: &Path, esbuild: &Path, client: &reqwest::Client) {
    write_fixture(root);

    let mut session = spawn_dev(root, esbuild);
    let port = boot_and_get_port(&mut session, client).await;
    let base = format!("http://localhost:{port}");

    poll_get_contains(client, &format!("{base}/"), TSX_MARKER, &session).await;
    poll_get_contains(client, &format!("{base}/plain"), PLAIN_TS_MARKER, &session).await;
    poll_get_contains(
        client,
        &format!("{base}/script"),
        SCRIPT_JS_MARKER,
        &session,
    )
    .await;
    poll_get_contains(
        client,
        &format!("{base}/component"),
        COMPONENT_JSX_MARKER,
        &session,
    )
    .await;
    poll_get_contains(client, &format!("{base}/doc"), DOC_MDX_MARKER, &session).await;
    poll_get_contains(client, &format!("{base}/note"), NOTE_MD_MARKER, &session).await;
    poll_get_contains(
        client,
        &format!("{base}/static"),
        STATIC_HTML_MARKER,
        &session,
    )
    .await;

    // Dynamic `.ts` page — both slugs must be served, resolving the same
    // `paths()` contract a `[slug].tsx` sibling would.
    poll_get_contains(
        client,
        &format!("{base}/alpha"),
        DYNAMIC_TS_ALPHA_MARKER,
        &session,
    )
    .await;
    poll_get_contains(
        client,
        &format!("{base}/beta"),
        DYNAMIC_TS_BETA_MARKER,
        &session,
    )
    .await;

    // No spurious "unrecognised extension" warning fired for any of the
    // seven shapes on the dev path either.
    let stderr = read_log(&session.stderr_path);
    assert!(
        !stderr.contains("pages/ file has an unrecognised extension"),
        "expected no `unrecognised extension` warnings from `zfb dev` for the seven-shape \
         fixture; got stderr:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// The heavy test — build THEN dev over the seven-shape + dynamic-.ts fixture.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn seven_page_shapes_plus_dynamic_ts_build_and_serve_end_to_end() {
    // Cross-binary lock acquired BEFORE anything else (issue #1339) — see
    // the lock-ordering note in zfb-test-utils/src/cross_binary_lock.rs.
    // Held for the whole test (build + the spawned dev server): this
    // file has only one spawning test, so there is no in-binary SERIAL
    // mutex to layer underneath it.
    let _e2e_lock = CrossBinaryE2eLock::acquire();

    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[page_extension_full_matrix_e2e] no esbuild binary available; set \
             ZFB_ESBUILD_BIN, place the binary at crates/zfb/binaries/esbuild/esbuild, or \
             install esbuild on PATH to enable this test. Skipping."
        );
        return;
    };

    let overall = async {
        // ── Build half — its own fresh fixture copy ───────────────
        let build_tmp = tempfile::tempdir().expect("tempdir for build root");
        let build_root = build_tmp
            .path()
            .canonicalize()
            .expect("canonicalize build root");

        let build_ok = {
            let root = build_root.clone();
            let esbuild = esbuild.clone();
            tokio::task::spawn_blocking(move || run_build_phase(&root, &esbuild))
                .await
                .expect("join zfb build task")
        };
        if !build_ok {
            return; // known-skip, already logged by run_build
        }

        // ── Dev half — its OWN fresh, unbuilt copy of the fixture ─
        let dev_tmp = tempfile::tempdir().expect("tempdir for dev root");
        let dev_root = dev_tmp
            .path()
            .canonicalize()
            .expect("canonicalize dev root");

        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("build reqwest client");

        run_dev_phase(&dev_root, &esbuild, &client).await;
    };

    tokio::time::timeout(OVERALL_DEADLINE, overall)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "[watchdog] seven-shape build+dev confirm pass did not finish within {}s — \
                 this indicates a hang (both the build subprocess and the spawned dev \
                 process group are killed/reaped by their own mechanisms regardless of how \
                 this panic unwinds).",
                OVERALL_DEADLINE.as_secs(),
            )
        });
}

// ---------------------------------------------------------------------------
// Fast cross-layer drift guard (no esbuild/V8 needed — an ordinary test).
// ---------------------------------------------------------------------------

/// The workspace root — two levels above `crates/zfb`.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root two levels above crates/zfb")
        .to_path_buf()
}

/// #1742's third expected outcome: "add a cross-layer command test so
/// the two allowlists cannot drift again". Asserts both the router
/// (`zfb-router/src/scan.rs`) and the bundler
/// (`zfb-build/src/bundler.rs`) source reference the shared
/// `zfb_types::ROUTABLE_PAGE_EXTENSIONS` constant rather than each
/// carrying its own independent literal allowlist. This is a
/// source-text check (not a runtime behavior check — the heavy test
/// above already proves runtime behavior), but it is exactly the kind
/// of guard that catches a future edit that reintroduces a second,
/// independently-drifting literal in either crate.
#[test]
fn router_and_bundler_share_the_zfb_types_page_extension_allowlist() {
    let ws = workspace_root();
    let scan_rs = fs::read_to_string(ws.join("crates/zfb-router/src/scan.rs"))
        .expect("read crates/zfb-router/src/scan.rs");
    let bundler_rs = fs::read_to_string(ws.join("crates/zfb-build/src/bundler.rs"))
        .expect("read crates/zfb-build/src/bundler.rs");

    assert!(
        scan_rs.contains("zfb_types::ROUTABLE_PAGE_EXTENSIONS"),
        "crates/zfb-router/src/scan.rs must consume `zfb_types::ROUTABLE_PAGE_EXTENSIONS` \
         instead of carrying its own independent extension allowlist"
    );
    assert!(
        bundler_rs.contains("zfb_types::ROUTABLE_PAGE_EXTENSIONS"),
        "crates/zfb-build/src/bundler.rs must consume `zfb_types::ROUTABLE_PAGE_EXTENSIONS` \
         instead of carrying its own independent extension allowlist"
    );

    // No independent literal copy of the full seven-extension list
    // survives outside `zfb-types` in either crate (in either array
    // literal spelling — with or without inter-element spaces).
    for needle in [
        r#"["tsx", "ts", "jsx", "js", "mdx", "md", "html"]"#,
        r#"["tsx","ts","jsx","js","mdx","md","html"]"#,
    ] {
        assert!(
            !scan_rs.contains(needle),
            "crates/zfb-router/src/scan.rs must not carry its own literal copy of the \
             page-extension list — found: {needle}"
        );
        assert!(
            !bundler_rs.contains(needle),
            "crates/zfb-build/src/bundler.rs must not carry its own literal copy of the \
             page-extension list — found: {needle}"
        );
    }
}
