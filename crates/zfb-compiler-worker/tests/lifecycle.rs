//! Real worker processes hosted by `compiler-worker-test-host`, an executable
//! that is not the zfb CLI.
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use zfb_compiler_worker::{
    BundleRequest, CompileJob, CompilerWorker, TestHook, WorkerError, WorkerLauncher, PRIVATE_ARG,
    TEST_HOOKS_ENV,
};
use zfb_rolldown_prototype::{OutputLayout, Platform};

const HANG: Duration = Duration::from_millis(3000);
const SHORT_DEADLINE: Duration = Duration::from_millis(700);
const DEADLINE: Duration = Duration::from_secs(20);

struct Fixture {
    root: PathBuf,
    scratch: PathBuf,
    site: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "zfb-compiler-worker-test-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("scratch")).unwrap();
        fs::create_dir_all(root.join("site")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let site = root.join("site");
        fs::write(
            site.join("dep.ts"),
            "export const marker: string = 'DEP_MARKER';\n",
        )
        .unwrap();
        fs::write(
            site.join("entry.ts"),
            "import { marker } from './dep';\nconst value: number = 1;\nexport default { marker, value };\n",
        )
        .unwrap();
        Self {
            scratch: root.join("scratch"),
            root,
            site,
        }
    }

    fn request(&self, out: &str) -> BundleRequest {
        let mut request = BundleRequest::new(
            &self.site,
            "entry.ts",
            OutputLayout::File(format!("{out}/entry.mjs").into()),
            Platform::Node,
        );
        request.graph = Some(format!("{out}/meta.json").into());
        request
    }

    fn output(&self, out: &str) -> PathBuf {
        self.site.join(out).join("entry.mjs")
    }

    fn worker(&self) -> CompilerWorker {
        CompilerWorker::new(launcher())
            .with_scratch_root(&self.scratch)
            .with_shutdown_grace(Duration::from_secs(2))
    }

    fn scratch_entries(&self) -> Vec<PathBuf> {
        fs::read_dir(&self.scratch)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect()
    }

    fn assert_nothing_published(&self, out: &str) {
        assert!(
            !self.site.join(out).exists(),
            "{} must not be published",
            self.site.join(out).display()
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn launcher() -> WorkerLauncher {
    WorkerLauncher::new(env!("CARGO_BIN_EXE_compiler-worker-test-host"))
        .arg(PRIVATE_ARG)
        .env(TEST_HOOKS_ENV, "1")
}

#[cfg(unix)]
fn process_gone(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&output.stdout);
    let stat = stat.trim();
    stat.is_empty() || stat.starts_with('Z')
}

#[cfg(not(unix))]
fn process_gone(_pid: u32) -> bool {
    true
}

fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read_pid(path: &Path) -> u32 {
    let mut pid = None;
    eventually("descendant pid file", || {
        pid = fs::read_to_string(path)
            .ok()
            .and_then(|text| text.trim().parse().ok());
        pid.is_some()
    });
    pid.unwrap()
}

/// Waits past the moment a hung worker would have finished, then checks
/// nothing appeared.
fn assert_no_late_publication(fixture: &Fixture, out: &str, started: Instant) {
    let past = started + HANG + Duration::from_millis(500);
    std::thread::sleep(past.saturating_duration_since(Instant::now()));
    fixture.assert_nothing_published(out);
}

#[tokio::test(flavor = "multi_thread")]
async fn native_success_publishes_validated_files() {
    let fixture = Fixture::new("success");
    let output = fixture
        .worker()
        .compile(CompileJob::new(fixture.request("out"), DEADLINE))
        .await
        .unwrap();
    let bundle = fs::read_to_string(fixture.output("out")).unwrap();
    assert!(bundle.contains("DEP_MARKER"), "{bundle}");
    assert!(
        !bundle.contains(": string"),
        "types must be stripped: {bundle}"
    );
    assert!(fixture.site.join("out/meta.json").is_file());
    let mut published: Vec<_> = output.files.iter().map(|file| file.path.clone()).collect();
    published.sort();
    assert_eq!(
        published,
        vec![fixture.output("out"), fixture.site.join("out/meta.json")]
    );
    assert!(
        output.graph["inputs"]["dep.ts"].is_object(),
        "{}",
        output.graph
    );
    assert!(fixture.scratch_entries().is_empty());
    assert!(process_gone(output.identity.pid()));
    assert_eq!(output.identity.nonce().len(), 32);
}

#[test]
fn blocking_api_compiles_outside_a_runtime() {
    let fixture = Fixture::new("blocking");
    fixture
        .worker()
        .compile_blocking(CompileJob::new(fixture.request("out"), DEADLINE))
        .unwrap();
    assert!(fs::read_to_string(fixture.output("out"))
        .unwrap()
        .contains("DEP_MARKER"));
}

#[tokio::test(flavor = "multi_thread")]
async fn compile_error_surfaces_the_diagnostic() {
    let fixture = Fixture::new("compile-error");
    fs::write(
        fixture.site.join("entry.ts"),
        "import { gone } from './does-not-exist';\nexport default gone;\n",
    )
    .unwrap();
    let error = fixture
        .worker()
        .compile(CompileJob::new(fixture.request("out"), DEADLINE))
        .await
        .unwrap_err();
    let WorkerError::Compile { diagnostic } = &error else {
        panic!("expected a compile error, got {error:?}");
    };
    assert!(diagnostic.contains("does-not-exist"), "{diagnostic}");
    fixture.assert_nothing_published("out");
    assert!(fixture.scratch_entries().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_of_a_hanging_generation_revokes_and_reaps() {
    let fixture = Fixture::new("hang-generate");
    let started = Instant::now();
    let job = fixture.worker().start(
        CompileJob::new(fixture.request("out"), SHORT_DEADLINE).with_test_hook(
            TestHook::HangBeforeGenerate {
                millis: HANG.as_millis() as u64,
            },
        ),
    );
    let job = job.unwrap();
    let pid = job.identity().pid();
    let error = job.wait().await.unwrap_err();
    assert!(
        matches!(error, WorkerError::Timeout { .. }),
        "expected timeout, got {error:?}"
    );
    assert!(
        started.elapsed() < HANG,
        "timeout must not wait for the hang"
    );
    assert!(process_gone(pid));
    assert!(fixture.scratch_entries().is_empty());
    assert_no_late_publication(&fixture, "out", started);
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_of_a_hanging_close_path_publishes_nothing() {
    let fixture = Fixture::new("hang-close");
    let started = Instant::now();
    let error = fixture
        .worker()
        .compile(
            CompileJob::new(fixture.request("out"), Duration::from_millis(2000)).with_test_hook(
                TestHook::HangAfterResponse {
                    millis: HANG.as_millis() as u64,
                },
            ),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, WorkerError::Timeout { .. }),
        "expected timeout, got {error:?}"
    );
    assert!(fixture.scratch_entries().is_empty());
    assert_no_late_publication(&fixture, "out", started);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_revokes_and_reaps() {
    let fixture = Fixture::new("cancel");
    let started = Instant::now();
    let job = fixture
        .worker()
        .start(
            CompileJob::new(fixture.request("out"), DEADLINE).with_test_hook(
                TestHook::HangBeforeGenerate {
                    millis: HANG.as_millis() as u64,
                },
            ),
        )
        .unwrap();
    let pid = job.identity().pid();
    let cancel = job.cancel_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();
    });
    let error = job.wait().await.unwrap_err();
    assert!(
        matches!(error, WorkerError::Cancelled),
        "expected cancellation, got {error:?}"
    );
    assert!(started.elapsed() < HANG);
    assert!(process_gone(pid));
    assert!(fixture.scratch_entries().is_empty());
    assert_no_late_publication(&fixture, "out", started);
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_submissions_each_get_their_own_identity() {
    let fixture = Fixture::new("repeated");
    let worker = fixture.worker();
    let mut identities = Vec::new();
    for index in 0..3 {
        let out = format!("seq-{index}");
        let output = worker
            .compile(CompileJob::new(fixture.request(&out), DEADLINE))
            .await
            .unwrap();
        assert!(fixture.output(&out).is_file());
        identities.push(output.identity);
    }
    let concurrent = (0..4).map(|index| {
        let worker = worker.clone();
        let request = fixture.request(&format!("par-{index}"));
        tokio::spawn(async move { worker.compile(CompileJob::new(request, DEADLINE)).await })
    });
    for handle in concurrent.collect::<Vec<_>>() {
        identities.push(handle.await.unwrap().unwrap().identity);
    }
    for index in 0..4 {
        assert!(fs::read_to_string(fixture.output(&format!("par-{index}")))
            .unwrap()
            .contains("DEP_MARKER"));
    }
    let nonces: std::collections::BTreeSet<_> = identities.iter().map(|id| id.nonce()).collect();
    let jobs: std::collections::BTreeSet<_> = identities.iter().map(|id| id.job_id()).collect();
    assert_eq!(nonces.len(), identities.len());
    assert_eq!(jobs.len(), identities.len());
    assert!(fixture.scratch_entries().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_running_job_reaps_cleans_and_never_publishes() {
    let fixture = Fixture::new("drop-job");
    let started = Instant::now();
    let job = fixture
        .worker()
        .start(
            CompileJob::new(fixture.request("out"), DEADLINE).with_test_hook(
                TestHook::HangBeforeGenerate {
                    millis: HANG.as_millis() as u64,
                },
            ),
        )
        .unwrap();
    let pid = job.identity().pid();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fixture.scratch_entries().len(), 1, "job owns a private dir");
    drop(job);
    eventually("dropped worker reaped", || process_gone(pid));
    eventually("private dir removed", || {
        fixture.scratch_entries().is_empty()
    });
    assert!(started.elapsed() < HANG);
    assert_no_late_publication(&fixture, "out", started);
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_compile_future_reaps_cleans_and_never_publishes() {
    let fixture = Fixture::new("drop-future");
    let started = Instant::now();
    let worker = fixture.worker();
    let pending = worker.compile(
        CompileJob::new(fixture.request("out"), DEADLINE).with_test_hook(
            TestHook::HangBeforeGenerate {
                millis: HANG.as_millis() as u64,
            },
        ),
    );
    assert!(tokio::time::timeout(Duration::from_millis(300), pending)
        .await
        .is_err());
    eventually("private dir removed", || {
        fixture.scratch_entries().is_empty()
    });
    assert!(started.elapsed() < HANG);
    assert_no_late_publication(&fixture, "out", started);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn timeout_kills_descendants_in_the_worker_group() {
    let fixture = Fixture::new("descendant");
    let pid_file = fixture.root.join("descendant.pid");
    let started = Instant::now();
    let error = fixture
        .worker()
        .compile(
            CompileJob::new(fixture.request("out"), Duration::from_millis(1000)).with_test_hook(
                TestHook::SpawnDescendant {
                    pid_file: pid_file.clone(),
                    millis: HANG.as_millis() as u64,
                },
            ),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, WorkerError::Timeout { .. }),
        "expected timeout, got {error:?}"
    );
    let descendant = read_pid(&pid_file);
    eventually("descendant killed", || process_gone(descendant));
    assert!(fixture.scratch_entries().is_empty());
    assert_no_late_publication(&fixture, "out", started);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn unconfirmed_shutdown_retains_the_private_dir() {
    let fixture = Fixture::new("escaped");
    let pid_file = fixture.root.join("escaped.pid");
    let started = Instant::now();
    let error = fixture
        .worker()
        .with_shutdown_grace(Duration::from_millis(500))
        .compile(
            CompileJob::new(fixture.request("out"), Duration::from_millis(1000)).with_test_hook(
                TestHook::SpawnEscapedDescendant {
                    pid_file: pid_file.clone(),
                    millis: HANG.as_millis() as u64,
                },
            ),
        )
        .await
        .unwrap_err();
    let escaped = read_pid(&pid_file);
    unsafe {
        libc::kill(escaped as libc::pid_t, libc::SIGKILL);
    }
    let WorkerError::ShutdownIncomplete { private_dir, cause } = &error else {
        panic!("expected incomplete shutdown, got {error:?}");
    };
    assert!(cause.contains("output stream stayed open"), "{cause}");
    assert!(private_dir.is_dir(), "the private dir must be retained");
    assert!(error.to_string().contains("shutdown is incomplete"));
    assert_no_late_publication(&fixture, "out", started);
}

async fn rejected(label: &str, hook: TestHook) -> (Fixture, String) {
    let fixture = Fixture::new(label);
    let error = fixture
        .worker()
        .compile(CompileJob::new(fixture.request("out"), DEADLINE).with_test_hook(hook))
        .await
        .unwrap_err();
    let WorkerError::Rejected { reason } = error else {
        panic!("expected a rejection, got {error:?}");
    };
    fixture.assert_nothing_published("out");
    (fixture, reason)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_written_outside_the_private_dir_is_rejected() {
    let (fixture, reason) = rejected("escape", TestHook::EscapePrivateDir).await;
    assert!(
        reason.contains("not inside the private directory"),
        "{reason}"
    );
    let leftovers = fixture.scratch_entries();
    assert_eq!(leftovers.len(), 1, "{leftovers:?}");
    assert!(leftovers[0].to_string_lossy().ends_with("-escaped"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_misreported_digest_is_rejected() {
    let (fixture, reason) = rejected("digest", TestHook::MisreportDigest).await;
    assert!(reason.contains("digest does not match"), "{reason}");
    assert!(fixture.scratch_entries().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_foreign_destination_is_rejected() {
    let foreign = std::env::temp_dir().join("zfb-compiler-worker-foreign.mjs");
    let (_fixture, reason) = rejected(
        "foreign",
        TestHook::ForeignDestination {
            destination: foreign.clone(),
        },
    )
    .await;
    assert!(reason.contains("not a destination"), "{reason}");
    assert!(!foreign.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreported_private_file_is_rejected() {
    let (_fixture, reason) = rejected("unreported", TestHook::UnreportedFile).await;
    assert!(reason.contains("unreported file"), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_identity_nonce_is_rejected() {
    let (_fixture, reason) = rejected("nonce", TestHook::WrongNonce).await;
    assert!(reason.contains("identity mismatch"), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_hooks_are_refused_without_the_env_gate() {
    let fixture = Fixture::new("hooks-off");
    let launcher = WorkerLauncher::new(env!("CARGO_BIN_EXE_compiler-worker-test-host"))
        .arg(PRIVATE_ARG)
        .env_remove(TEST_HOOKS_ENV);
    let error = CompilerWorker::new(launcher)
        .with_scratch_root(&fixture.scratch)
        .compile(
            CompileJob::new(fixture.request("out"), DEADLINE)
                .with_test_hook(TestHook::MisreportDigest),
        )
        .await
        .unwrap_err();
    let WorkerError::WorkerFailed { stderr, .. } = &error else {
        panic!("expected a worker failure, got {error:?}");
    };
    assert!(stderr.contains("test hooks are disabled"), "{stderr}");
    fixture.assert_nothing_published("out");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_started_without_the_private_mode_fails_clearly() {
    let fixture = Fixture::new("no-private-arg");
    let launcher = WorkerLauncher::new(env!("CARGO_BIN_EXE_compiler-worker-test-host"));
    let error = CompilerWorker::new(launcher)
        .with_scratch_root(&fixture.scratch)
        .compile(CompileJob::new(fixture.request("out"), DEADLINE))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("without the private worker argument"),
        "{error}"
    );
    fixture.assert_nothing_published("out");
    assert!(fixture.scratch_entries().is_empty());
}

#[test]
fn a_missing_launcher_is_an_actionable_error() {
    let error = CompilerWorker::require(None).unwrap_err();
    assert!(matches!(error, WorkerError::MissingLauncher));
    let message = error.to_string();
    assert!(
        message.contains("did not provide a compiler worker launcher"),
        "{message}"
    );
    assert!(message.contains("run_if_requested"), "{message}");
    assert!(message.contains("zfb CLI"), "{message}");
}

#[test]
fn the_cli_launcher_strips_the_test_hook_gate() {
    let launcher = WorkerLauncher::zfb_cli().unwrap();
    assert_eq!(launcher.program(), std::env::current_exe().unwrap());
    assert_eq!(launcher.args().collect::<Vec<_>>(), vec![PRIVATE_ARG]);
}
