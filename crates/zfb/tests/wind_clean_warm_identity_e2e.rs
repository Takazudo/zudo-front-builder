//! W-A04 live candidate ownership and clean/warm identity through `zfb dev`
//! (#3269). Level 4, tier T1: one real dev session processes source edits,
//! then a fresh session on the final source tree must serve byte-identical CSS.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use zfb_test_utils::{
    locate_esbuild, next_sse_event_name, open_sse, watcher_live_handshake, zfb_binary,
    CrossBinaryE2eLock, HandshakeOpts,
};

static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

const BOOT_DEADLINE: Duration = Duration::from_secs(90);
const WATCHER_DEADLINE: Duration = Duration::from_secs(35);
const CSS_DEADLINE: Duration = Duration::from_secs(45);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const CSS_URL: &str = "/assets/styles.css";
const HANDSHAKE_DIR: &str = "src/.watcher-handshake";

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("wind-ownership")
}

fn copy_source_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".zfb-dev-") {
            continue;
        }
        let source_path = entry.path();
        let destination_path = destination.join(name.as_ref());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if matches!(name.as_ref(), "dist" | ".zfb-build" | ".zfb") {
                continue;
            }
            copy_source_tree(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

/// Preserve the fixture's tracked package under `node_modules/@fixture` while
/// linking the binary's framework snapshot beside it for real SSR/esbuild.
fn link_embedded_framework_packages(root: &Path) -> tempfile::TempDir {
    let (lease, embedded_node_modules) =
        zfb::render_pipeline::embedded_node_modules().expect("extract embedded node_modules");
    let project_node_modules = root.join("node_modules");
    fs::create_dir_all(&project_node_modules).expect("create fixture node_modules");
    for entry in fs::read_dir(&embedded_node_modules).expect("read embedded node_modules") {
        let entry = entry.expect("read embedded node_modules entry");
        let destination = project_node_modules.join(entry.file_name());
        if fs::symlink_metadata(&destination).is_ok() {
            continue;
        }
        symlink(entry.path(), destination).expect("link embedded framework package");
    }
    lease
}

struct DevServerGuard {
    child: std::process::Child,
    process_group: libc::pid_t,
}

impl DevServerGuard {
    fn try_exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().expect("poll `zfb dev` process")
    }
}

impl Drop for DevServerGuard {
    fn drop(&mut self) {
        unsafe { libc::kill(-self.process_group, libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

struct DevSession {
    root: PathBuf,
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

fn spawn_dev(root: PathBuf, esbuild: &Path) -> DevSession {
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout = fs::File::create(&stdout_path).expect("create zfb dev stdout log");
    let stderr = fs::File::create(&stderr_path).expect("create zfb dev stderr log");
    let mut command = Command::new(zfb_binary!());
    command
        .arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(&root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .env_remove("ZFB_DEV_TEST_SLOW_BOOT_RENDER_MS");
    command.process_group(0);
    let child = command.spawn().expect("spawn `zfb dev --port 0`");
    let process_group = child.id() as libc::pid_t;
    DevSession {
        root,
        guard: DevServerGuard {
            child,
            process_group,
        },
        stdout_path,
        stderr_path,
    }
}

fn ready_port(log: &str) -> Option<u16> {
    let mut remaining = log;
    while let Some(index) = remaining.find("http://") {
        let address = &remaining[index + "http://".len()..];
        let token = address.split_whitespace().next().unwrap_or_default();
        if let Some(colon) = token.find(':') {
            let port = token[colon + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>();
            if let Ok(port) = port.parse() {
                return Some(port);
            }
        }
        remaining = &address[token.len().min(address.len())..];
    }
    None
}

async fn boot_and_handshake(session: &mut DevSession) -> Option<(String, reqwest::Client, u32)> {
    let start = Instant::now();
    let port = loop {
        if let Some(status) = session.guard.try_exit_status() {
            let logs = session.logs();
            if logs.contains("embed_v8") || logs.contains("no esbuild") {
                eprintln!("[wind_clean_warm_identity_e2e] known environment skip:\n{logs}");
                return None;
            }
            panic!("`zfb dev` exited with {status:?} before ready.\n{logs}");
        }
        let stdout = fs::read_to_string(&session.stdout_path).unwrap_or_default();
        if let Some(port) = ready_port(&stdout) {
            break port;
        }
        assert!(
            start.elapsed() < BOOT_DEADLINE,
            "`zfb dev` did not print a ready URL within {}s.\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    let base = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build dev HTTP client");

    let start = Instant::now();
    loop {
        if let Ok(response) = client.get(format!("{base}/")).send().await {
            if response.status().as_u16() == 200 {
                break;
            }
        }
        assert!(
            start.elapsed() < BOOT_DEADLINE,
            "GET / never answered 200 after the ready banner.\n{}",
            session.logs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Subscribe first, then use the shared condition-keyed handshake. Probe
    // files have a .probe suffix and contain no class-shaped text; they are
    // removed before the final source tree is copied to the clean session.
    let sse = open_sse(&base).await;
    assert_eq!(
        sse.status().as_u16(),
        200,
        "reload SSE endpoint must answer 200"
    );
    let observed = Arc::new(AtomicBool::new(false));
    let observed_by_reader = Arc::clone(&observed);
    let reader = tokio::spawn(async move {
        if let Ok(Some(_)) = next_sse_event_name(sse, WATCHER_DEADLINE).await {
            observed_by_reader.store(true, Ordering::SeqCst);
        }
    });
    let marker_root = session.root.join(HANDSHAKE_DIR);
    let handshake = watcher_live_handshake(
        HandshakeOpts::new(WATCHER_DEADLINE)
            .with_marker_interval(Duration::from_millis(400))
            .with_poll_interval(Duration::from_millis(25)),
        |index| {
            fs::create_dir_all(&marker_root).expect("create watcher handshake directory");
            fs::write(
                marker_root.join(format!("probe-{index}.probe")),
                b"watcher is live\n",
            )
            .expect("write watcher handshake marker");
        },
        || observed.load(Ordering::SeqCst),
    )
    .await;
    let _ = reader.await;
    assert!(
        handshake.live,
        "the dev watcher did not become live after {} fresh probes in {:?}.\n{}",
        handshake.markers_written,
        handshake.elapsed,
        session.logs()
    );
    Some((base, client, handshake.markers_written))
}

async fn wait_for_css_state(
    client: &reqwest::Client,
    base: &str,
    present: &[&str],
    absent: &[&str],
    phase: &str,
    session: &DevSession,
) -> String {
    let url = format!("{base}{CSS_URL}");
    let start = Instant::now();
    let mut last = String::from("(no response yet)");
    while start.elapsed() < CSS_DEADLINE {
        match client.get(&url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let css = response.text().await.unwrap_or_default();
                if status == 200
                    && present.iter().all(|needle| css.contains(needle))
                    && absent.iter().all(|needle| !css.contains(needle))
                {
                    return css;
                }
                last = format!("status {status}; CSS bytes:\n{css}");
            }
            Err(error) => last = format!("request failed: {error}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "[{phase}] served CSS did not reach the required state within {}s.\nLast observation: {last}\n{}",
        CSS_DEADLINE.as_secs(),
        session.logs()
    );
}

/// Wait until the watcher has no in-flight handshake tick before the real
/// source sequence. This is event-driven quiescence, not a fixed settle sleep.
async fn drain_ticks_until_quiescent(base: &str, session: &DevSession) {
    const QUIET_GAP: Duration = Duration::from_millis(1500);
    const DRAIN_DEADLINE: Duration = Duration::from_secs(20);
    let start = Instant::now();
    while start.elapsed() < DRAIN_DEADLINE {
        let response = open_sse(base).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "reload SSE endpoint must remain available while draining"
        );
        match next_sse_event_name(response, QUIET_GAP).await {
            Ok(Some(_)) => continue,
            Ok(None) => return,
            Err(error) => panic!(
                "failed to drain watcher ticks before an edit: {error:#}\n{}",
                session.logs()
            ),
        }
    }
    panic!(
        "watcher did not become quiescent within {}s.\n{}",
        DRAIN_DEADLINE.as_secs(),
        session.logs()
    );
}

fn remove_handshake_markers(root: &Path, markers_written: u32) {
    let marker_root = root.join(HANDSHAKE_DIR);
    for index in 0..markers_written {
        let _ = fs::remove_file(marker_root.join(format!("probe-{index}.probe")));
    }
    let _ = fs::remove_dir(&marker_root);
}

fn remove_manifest_declaration(root: &Path) {
    let config_path = root.join("zfb.config.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).expect("read wind config"))
            .expect("parse wind config");
    let removed = config["wind"]["manifests"]
        .as_object_mut()
        .expect("wind manifests object")
        .remove("wind-ownership-fixture");
    assert!(
        removed.is_some(),
        "expected fixture package manifest declaration"
    );
    fs::write(
        config_path,
        serde_json::to_vec_pretty(&config).expect("serialize config without manifest"),
    )
    .expect("remove package manifest declaration");
}

#[tokio::test(flavor = "multi_thread")]
async fn warm_source_sequence_matches_a_fresh_dev_session_byte_for_byte() {
    let _cross_binary_lock = CrossBinaryE2eLock::acquire();
    let _serial = SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[wind_clean_warm_identity_e2e] no esbuild; skipping.");
        return;
    };

    let warm_temp = tempfile::tempdir().expect("create warm project tempdir");
    let warm_root = warm_temp.path().join("project");
    copy_source_tree(&fixture_dir(), &warm_root).expect("copy wind-ownership fixture");
    let _warm_node_modules = link_embedded_framework_packages(&warm_root);
    let mut warm_session = spawn_dev(warm_root.clone(), &esbuild);
    let Some((warm_base, warm_client, warm_marker_count)) =
        boot_and_handshake(&mut warm_session).await
    else {
        return;
    };
    drain_ticks_until_quiescent(&warm_base, &warm_session).await;

    let initial_css = wait_for_css_state(
        &warm_client,
        &warm_base,
        &[
            ".bg-shared",
            ".bg-first-only",
            ".bg-second-only",
            ".bg-safelist",
            ".bg-safe-source-only",
            ".bg-manifest-v1",
            ".bg-deleted",
            ".bg-renamed",
        ],
        &[],
        "initial candidate set",
        &warm_session,
    )
    .await;

    // W-A04 duplicate owners: a candidate remains after the first source
    // drops it, then disappears after the second source drops it.
    let first_owner = warm_root.join("src/first-owner.tsx");
    fs::write(
        &first_owner,
        "export default function FirstOwner() { return <div />; }\n",
    )
    .expect("remove first owner's candidates once");
    let after_first_owner = wait_for_css_state(
        &warm_client,
        &warm_base,
        &[".bg-shared", ".bg-second-only"],
        &[".bg-first-only"],
        "first duplicate owner removed",
        &warm_session,
    )
    .await;
    assert_ne!(
        initial_css, after_first_owner,
        "first-owner marker must be retracted"
    );

    let second_owner = warm_root.join("src/second-owner.tsx");
    fs::write(
        &second_owner,
        "export default function SecondOwner() { return <div />; }\n",
    )
    .expect("remove second owner's candidates once");
    let after_second_owner = wait_for_css_state(
        &warm_client,
        &warm_base,
        &[],
        &[".bg-shared", ".bg-second-only"],
        "last duplicate owner removed",
        &warm_session,
    )
    .await;
    assert_ne!(after_first_owner, after_second_owner);

    // A safelist contribution outlives the source that also mentions it.
    fs::write(
        warm_root.join("src/safelist-owner.tsx"),
        "export default function SafelistOwner() { return <div />; }\n",
    )
    .expect("remove safelist source once");
    wait_for_css_state(
        &warm_client,
        &warm_base,
        &[".bg-safelist"],
        &[".bg-safe-source-only"],
        "safelist survives source deletion",
        &warm_session,
    )
    .await;

    // Replace the declared package manifest, then remove its declaration and
    // file together. Required manifests are never tested by deleting a still-
    // declared path; the spec requires that to remain a hard error.
    let manifest_path = warm_root.join("node_modules/@fixture/wind-ownership/wind.json");
    fs::write(
        &manifest_path,
        r#"{"schemaVersion":1,"specVersion":1,"producer":"wind-ownership-fixture","candidates":["bg-manifest-v2"]}"#,
    )
    .expect("replace package candidate manifest once");
    wait_for_css_state(
        &warm_client,
        &warm_base,
        &[".bg-manifest-v2"],
        &[".bg-manifest-v1"],
        "package manifest replaced",
        &warm_session,
    )
    .await;
    remove_manifest_declaration(&warm_root);
    fs::remove_file(&manifest_path).expect("remove undeclared package manifest");
    wait_for_css_state(
        &warm_client,
        &warm_base,
        &[],
        &[".bg-manifest-v2"],
        "package manifest owner removed",
        &warm_session,
    )
    .await;

    // A deleted file retracts its candidate. The output directories are
    // excluded from all source roots by the fixture config/ignore shape.
    fs::remove_file(warm_root.join("src/deleted.tsx")).expect("delete one source file");
    wait_for_css_state(
        &warm_client,
        &warm_base,
        &[],
        &[".bg-deleted"],
        "source file deleted",
        &warm_session,
    )
    .await;

    // Rename without changing bytes: the class remains in the live set and
    // the renamed path is the only source on disk that owns it.
    let old_path = warm_root.join("src/rename-before.tsx");
    let new_path = warm_root.join("src/rename-after.tsx");
    let renamed_bytes = fs::read(&old_path).expect("read rename fixture bytes");
    fs::rename(&old_path, &new_path).expect("rename source file without content edits");
    assert!(!old_path.exists(), "old source path should no longer exist");
    assert_eq!(
        fs::read(&new_path).expect("read renamed source bytes"),
        renamed_bytes,
        "the rename must not alter the file contents"
    );
    wait_for_css_state(
        &warm_client,
        &warm_base,
        &[".bg-renamed"],
        &[],
        "source renamed without content change",
        &warm_session,
    )
    .await;

    // One fresh file under the configured `src/` root adds a candidate.
    fs::write(
        warm_root.join("src/added.tsx"),
        "export default function AddedOwner() { return <div class=\"bg-added\" />; }\n",
    )
    .expect("add one source file under configured root");
    wait_for_css_state(
        &warm_client,
        &warm_base,
        &[".bg-renamed", ".bg-added", ".bg-safelist"],
        &[
            ".bg-shared",
            ".bg-first-only",
            ".bg-second-only",
            ".bg-safe-source-only",
            ".bg-manifest-v1",
            ".bg-manifest-v2",
            ".bg-deleted",
        ],
        "final warmed candidate set",
        &warm_session,
    )
    .await;

    // Remove the probe files so both the warm final tree and the clean
    // session's source tree have the same watched-root contents. They were
    // .probe files with no candidate-shaped content and never contributed.
    remove_handshake_markers(&warm_root, warm_marker_count);
    drain_ticks_until_quiescent(&warm_base, &warm_session).await;
    let warm_css = wait_for_css_state(
        &warm_client,
        &warm_base,
        &[".bg-renamed", ".bg-added", ".bg-safelist"],
        &[
            ".bg-shared",
            ".bg-first-only",
            ".bg-second-only",
            ".bg-safe-source-only",
            ".bg-manifest-v1",
            ".bg-manifest-v2",
            ".bg-deleted",
        ],
        "final warmed candidate set after handshake cleanup",
        &warm_session,
    )
    .await;
    let clean_temp = tempfile::tempdir().expect("create clean project tempdir");
    let clean_root = clean_temp.path().join("project");
    copy_source_tree(&warm_root, &clean_root).expect("copy final warm source tree");
    let _clean_node_modules = link_embedded_framework_packages(&clean_root);
    let mut clean_session = spawn_dev(clean_root, &esbuild);
    let Some((clean_base, clean_client, clean_marker_count)) =
        boot_and_handshake(&mut clean_session).await
    else {
        return;
    };
    remove_handshake_markers(&clean_session.root, clean_marker_count);
    drain_ticks_until_quiescent(&clean_base, &clean_session).await;
    let clean_css = wait_for_css_state(
        &clean_client,
        &clean_base,
        &[".bg-renamed", ".bg-added", ".bg-safelist"],
        &[
            ".bg-shared",
            ".bg-first-only",
            ".bg-second-only",
            ".bg-safe-source-only",
            ".bg-manifest-v1",
            ".bg-manifest-v2",
            ".bg-deleted",
        ],
        "final clean candidate set",
        &clean_session,
    )
    .await;

    if warm_css.as_bytes() != clean_css.as_bytes() {
        let evidence = std::env::temp_dir().join("zfb-3242-reviews/3269-css-diff");
        fs::create_dir_all(&evidence).expect("create clean/warm CSS evidence directory");
        fs::write(evidence.join("warm.css"), warm_css.as_bytes())
            .expect("save warm CSS diff evidence");
        fs::write(evidence.join("clean.css"), clean_css.as_bytes())
            .expect("save clean CSS diff evidence");
        panic!(
            "W-A04 clean/warm stylesheet bytes differ; saved both files under {}\n--- warm served CSS ---\n{warm_css}\n--- clean served CSS ---\n{clean_css}",
            evidence.display()
        );
    }
    eprintln!(
        "[W-A04] clean/warm byte comparison ran and matched ({} bytes).",
        warm_css.len()
    );
}
