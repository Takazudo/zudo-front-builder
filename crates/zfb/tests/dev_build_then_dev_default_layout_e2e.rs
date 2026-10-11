//! Issue #4011: controlled build → dev default-layout reproduction.
//! Each case uses a real CLI process, a built MDX/two-island seed, and a single
//! source edit. The per-probe timeline and Drop diagnostics distinguish a
//! persistent 404 from a later 200 or a naturally exited listener.
//!
//! Eager-scratch boot observation (issue #4134): `dev.rs` prints
//! `[zfb-timing] boot: render complete` AFTER committing document publication,
//! so a 200 can be served before that stderr line reaches the captured log.
//! The #4011 contract is therefore "the post-boot state serves the document":
//! the test observes the boot line first, then accepts a fresh 200 carrying
//! both island markers. HTTP publication and the later diagnostic stderr write
//! are not claimed to be atomic; a 200 seen before the line is observed is
//! recorded as `early_200_before_boot_line` evidence, never a failure.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

const DEADLINE: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(300);
const MARKERS: [&str; 2] = [
    "data-zfb-island=\"Counter\"",
    "data-zfb-island=\"NamedCounter\"",
];
// Mirrors `format_boot_render_complete_line` in crates/zfb/src/commands/dev.rs.
const BOOT_RENDER_COMPLETE_LINE: &str = "[zfb-timing] boot: render complete";

#[derive(Clone, Copy)]
enum Mode {
    Default,
    EagerScratch,
    Timing,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::EagerScratch => "eager-scratch",
            Self::Timing => "timing",
        }
    }
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dev-build-then-dev-default-layout")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create fixture destination");
    for entry in fs::read_dir(from).expect("read fixture") {
        let entry = entry.expect("fixture entry");
        let destination = to.join(entry.file_name());
        if entry.file_type().expect("fixture type").is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).expect("copy fixture file");
        }
    }
}

fn clean_env(cmd: &mut Command) -> &mut Command {
    cmd.env_remove("ZFB_SCRATCH_DIR")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_BOOT_LAZY")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_DEV_TIMING")
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind free loopback port")
        .local_addr()
        .expect("loopback address")
        .port()
}

fn listener_state(port: u16) -> String {
    let output = if cfg!(target_os = "macos") {
        Command::new("lsof")
            .args(["-nP", "-i", &format!("TCP:{port}")])
            .output()
    } else {
        Command::new("ss").args(["-ltnp"]).output()
    };
    match output {
        Ok(output) => format!(
            "status={:?}; stdout={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) => format!("listener inspection unavailable: {error}"),
    }
}

struct DevSession {
    child: Child,
    port: u16,
    started: Instant,
    stdout: PathBuf,
    stderr: PathBuf,
    mode: Mode,
    evidence: Option<PathBuf>,
}

impl DevSession {
    fn record(&self, line: &str) {
        let Some(path) = &self.evidence else {
            return;
        };
        // Keep artifacts bounded even when a broken server emits huge logs.
        if fs::metadata(path).is_ok_and(|meta| meta.len() >= 256 * 1024) {
            return;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open default-layout evidence");
        writeln!(file, "{}", line.chars().take(16 * 1024).collect::<String>())
            .expect("write evidence");
    }

    fn logs(&self) -> String {
        format!(
            "stdout:\n{}\nstderr:\n{}",
            fs::read_to_string(&self.stdout).unwrap_or_default(),
            fs::read_to_string(&self.stderr).unwrap_or_default()
        )
    }

    fn stderr_logs(&self) -> String {
        fs::read_to_string(&self.stderr).unwrap_or_default()
    }

    fn status(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().expect("inspect dev child")
    }

    fn diagnostic(&mut self, phase: &str) -> String {
        format!(
            "[{}] {phase}: elapsed={:?}, natural_child_status={:?}, listener={}\n{}",
            self.mode.name(),
            self.started.elapsed(),
            self.status(),
            listener_state(self.port),
            self.logs()
        )
    }
}

impl Drop for DevSession {
    fn drop(&mut self) {
        // Inspect BEFORE our signal, so a natural exit is never reported as
        // the test's cleanup signal. Always publish both captured streams.
        let diagnostic = self.diagnostic("before cleanup");
        self.record(&diagnostic);
        self.record(&format!(
            "stdout_excerpt={}",
            fs::read_to_string(&self.stdout).unwrap_or_default()
        ));
        self.record(&format!(
            "stderr_excerpt={}",
            fs::read_to_string(&self.stderr).unwrap_or_default()
        ));
        eprintln!("{diagnostic}");
        let pgid = self.child.id() as libc::pid_t;
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

async fn ready(session: &mut DevSession) -> Instant {
    loop {
        let log = session.logs();
        if log.contains(&format!("ready on http://127.0.0.1:{}", session.port)) {
            let observed = Instant::now();
            let line = format!(
                "[{}] ready_observed_ms={}",
                session.mode.name(),
                session.started.elapsed().as_millis()
            );
            session.record(&line);
            eprintln!("{line}");
            return observed;
        }
        if let Some(status) = session.status() {
            panic!(
                "{}",
                session.diagnostic(&format!("natural exit before ready: {status:?}"))
            );
        }
        assert!(
            session.started.elapsed() < DEADLINE,
            "{}",
            session.diagnostic("ready timeout")
        );
        tokio::time::sleep(POLL).await;
    }
}

/// `boot_line`, when set, gates acceptance on the captured stderr: a probe is
/// accepted only if the line was already observed BEFORE that probe was sent,
/// so the accepted response is provably a post-boot one. An acceptable
/// response seen earlier is recorded as `early_200_before_boot_line` and
/// polling continues against the same `DEADLINE`. The other callers pass
/// `None` and keep their previous behavior.
async fn poll<F>(
    session: &mut DevSession,
    ready_at: Instant,
    url: &str,
    phase: &str,
    boot_line: Option<&str>,
    accept: F,
) -> String
where
    F: Fn(u16, &str) -> bool,
{
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("HTTP client");
    let started = Instant::now();
    let mut probes = 0;
    let mut not_found = 0;
    let mut early_200 = 0;
    let mut boot_line_observed_ms: Option<u128> = None;
    loop {
        probes += 1;
        if let Some(gate) = boot_line {
            if boot_line_observed_ms.is_none() && session.stderr_logs().contains(gate) {
                let observed = ready_at.elapsed().as_millis();
                boot_line_observed_ms = Some(observed);
                let line = format!(
                    "[{}] {phase} boot_line_observed_ms={observed} before_probe={probes} early_200_before_boot_line={early_200}",
                    session.mode.name()
                );
                session.record(&line);
                eprintln!("{line}");
            }
        }
        let post_boot = boot_line.is_none() || boot_line_observed_ms.is_some();
        let gate_suffix = if boot_line.is_some() {
            format!(
                " post_boot={post_boot} early_200_before_boot_line={early_200} boot_line_observed_ms={boot_line_observed_ms:?}"
            )
        } else {
            String::new()
        };
        let latest = match client.get(url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                if status == 404 {
                    not_found += 1;
                }
                let line = format!(
                    "[{}] {phase} probe={probes} ready_ms={} status={status} bytes={} markers=[{},{}]{gate_suffix}",
                    session.mode.name(), ready_at.elapsed().as_millis(), body.len(),
                    body.contains(MARKERS[0]), body.contains(MARKERS[1])
                );
                session.record(&line);
                eprintln!("{line}");
                if accept(status, &body) {
                    if post_boot {
                        let line = format!(
                            "[{}] {phase} accepted_ms={} not_found_probes={not_found}{gate_suffix}",
                            session.mode.name(),
                            started.elapsed().as_millis()
                        );
                        session.record(&line);
                        eprintln!("{line}");
                        return body;
                    }
                    // Valid, but not yet provably post-boot: evidence, not
                    // acceptance (issue #4134). Keep polling.
                    early_200 += 1;
                    let line = format!(
                        "[{}] {phase} early_200_before_boot_line probe={probes} ready_ms={} count={early_200}",
                        session.mode.name(),
                        ready_at.elapsed().as_millis()
                    );
                    session.record(&line);
                    eprintln!("{line}");
                }
                format!(
                    "status={status}, body={}",
                    body.chars().take(300).collect::<String>()
                )
            }
            Err(error) => {
                let line = format!(
                    "[{}] {phase} probe={probes} ready_ms={} error={error}",
                    session.mode.name(),
                    ready_at.elapsed().as_millis()
                );
                session.record(&line);
                eprintln!("{line}");
                format!("request error={error}")
            }
        };
        if let Some(status) = session.status() {
            panic!(
                "last probe: {latest}\n{}",
                session.diagnostic(&format!("natural exit during {phase}: {status:?}"))
            );
        }
        assert!(
            started.elapsed() < DEADLINE,
            "last probe: {latest}\n{}",
            session.diagnostic(&format!(
                "{phase} timeout; 404 probes={not_found}{gate_suffix}"
            ))
        );
        tokio::time::sleep(POLL).await;
    }
}

async fn run(mode: Mode) {
    let _lock = CrossBinaryE2eLock::acquire();
    let esbuild =
        locate_esbuild().expect("#4011 reproduction requires esbuild; set ZFB_ESBUILD_BIN");
    let evidence = std::env::var_os("ZFB_DEFAULT_LAYOUT_EVIDENCE_DIR").map(|dir| {
        let dir = PathBuf::from(dir);
        fs::create_dir_all(&dir).expect("create default-layout evidence directory");
        let file = dir.join(format!("{}.log", mode.name()));
        fs::write(&file, format!("case={}\n", mode.name())).expect("initialize evidence");
        file
    });
    let temp = tempfile::tempdir().expect("fixture tempdir");
    let root = temp.path();
    copy_tree(&fixture(), root);
    let port = free_port();

    let build = clean_env(&mut Command::new(zfb_binary!()))
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("run zfb build (V8 must be available)");
    let build_diagnostic = format!(
        "[{}] build status={:?}\nstdout:\n{}\nstderr:\n{}",
        mode.name(),
        build.status,
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    if let Some(path) = &evidence {
        fs::write(
            path,
            build_diagnostic.chars().take(64 * 1024).collect::<String>(),
        )
        .expect("write build evidence");
    }
    eprintln!("{build_diagnostic}");
    assert!(
        build.status.success(),
        "zfb build failed (V8/esbuild required)"
    );
    let seed =
        fs::read_to_string(root.join("dist/index.html")).expect("build must seed dist/index.html");
    assert!(
        MARKERS.iter().all(|marker| seed.contains(marker)),
        "built seed lacks both island markers: {seed}"
    );

    let stdout = root.join(".zfb-repro-stdout.log");
    let stderr = root.join(".zfb-repro-stderr.log");
    let mut cmd = Command::new(zfb_binary!());
    clean_env(&mut cmd)
        .arg("dev")
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .stdout(Stdio::from(fs::File::create(&stdout).expect("stdout log")))
        .stderr(Stdio::from(fs::File::create(&stderr).expect("stderr log")));
    if matches!(mode, Mode::EagerScratch) {
        cmd.args(["--scratch-dir", ".zfb-build/repro-session"])
            .env("ZFB_DEV_EAGER", "1");
    }
    if matches!(mode, Mode::Timing | Mode::EagerScratch) {
        cmd.env("ZFB_DEV_TIMING", "1");
    }
    cmd.process_group(0);
    let child = cmd.spawn().expect("spawn zfb dev (V8/esbuild required)");
    let mut session = DevSession {
        child,
        port,
        started: Instant::now(),
        stdout,
        stderr,
        mode,
        evidence,
    };
    let ready_at = ready(&mut session).await;
    let origin = format!("http://127.0.0.1:{port}");
    let home = format!("{origin}/");
    let asset = format!("{origin}/assets/islands.js");

    // dist_seed_for_keeps_the_seed_for_the_default_layout unit-pins Some(dist/).
    // Startup has no positive seed line: built dist plus the absence of the
    // scratch-only negative line is the observable default-layout evidence.
    let log = session.logs();
    if matches!(mode, Mode::EagerScratch) {
        assert!(
            log.contains("ignoring prebuilt") && log.contains("no dist/ fallback"),
            "scratch seed decision missing:\n{log}"
        );
    } else {
        assert!(
            !log.contains("ignoring prebuilt"),
            "default layout unexpectedly ignored dist seed:\n{log}"
        );
        let line = format!(
            "[{}] seed decision: default layout retains built {}",
            mode.name(),
            root.join("dist").display()
        );
        session.record(&line);
        eprintln!("{line}");
    }

    // Eager scratch: observe the boot-render line, THEN accept a fresh 200
    // with both markers (issue #4134). The other modes gate on nothing.
    let boot_gate = matches!(mode, Mode::EagerScratch).then_some(BOOT_RENDER_COMPLETE_LINE);
    let html = poll(
        &mut session,
        ready_at,
        &home,
        "initial HTML",
        boot_gate,
        |status, body| status == 200 && MARKERS.iter().all(|marker| body.contains(marker)),
    )
    .await;
    assert!(
        html.contains("Default layout MDX"),
        "MDX content absent: {html}"
    );
    if matches!(mode, Mode::EagerScratch) {
        // The gate guarantees this; the pin keeps the eager contract explicit
        // if the gate is ever dropped from the call above.
        let log = session.stderr_logs();
        assert!(
            log.contains(BOOT_RENDER_COMPLETE_LINE),
            "eager HTML accepted without an observed boot-render line:\n{log}"
        );
    }
    let initial_bundle = poll(
        &mut session,
        ready_at,
        &asset,
        "initial islands",
        None,
        |status, body| status == 200 && body.contains("Named source v1"),
    )
    .await;

    let source = root.join("components/counter.tsx");
    let old = fs::read_to_string(&source).expect("read island source");
    assert!(old.contains("Named source v1"));
    fs::write(&source, old.replace("Named source v1", "Named source v2"))
        .expect("one post-boot island source edit");
    let updated_bundle = poll(
        &mut session,
        ready_at,
        &asset,
        "rebundled islands",
        None,
        |status, body| status == 200 && body.contains("Named source v2"),
    )
    .await;
    assert_ne!(
        updated_bundle, initial_bundle,
        "islands.js bytes did not change after source edit"
    );
    poll(
        &mut session,
        ready_at,
        &home,
        "rerendered HTML",
        None,
        |status, body| {
            status == 200
                && body.contains("Named source v2")
                && MARKERS.iter().all(|marker| body.contains(marker))
        },
    )
    .await;
    if matches!(mode, Mode::Timing) {
        assert!(
            session.logs().contains("[zfb-timing]"),
            "timing mode emitted no lazy-render diagnostics"
        );
    }
}

#[tokio::test]
async fn default_layout_build_then_dev_serves_and_rebundles() {
    run(Mode::Default).await;
}

#[tokio::test]
async fn eager_scratch_build_then_dev_serves_after_boot_and_rebundles() {
    run(Mode::EagerScratch).await;
}

#[tokio::test]
async fn timing_default_layout_records_lazy_outcomes_and_rebundles() {
    run(Mode::Timing).await;
}
