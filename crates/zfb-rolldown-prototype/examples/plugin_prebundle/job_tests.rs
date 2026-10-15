use super::*;
use rolldown::plugin::HookCloseBundleArgs;
use rolldown_common::{CodeSplittingMode, OutputFormat, Platform, SourceMapType};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        mpsc::{self, Receiver, SyncSender},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "zfb-cancel-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn options(&self, source: &str) -> BundlerOptions {
        let entry = self.0.join("plugin.ts");
        std::fs::write(&entry, source).unwrap();
        BundlerOptions {
            cwd: Some(self.0.clone()),
            input: Some(vec![entry.to_string_lossy().into_owned().into()]),
            file: Some(self.output().to_string_lossy().into_owned()),
            format: Some(OutputFormat::Esm),
            platform: Some(Platform::Node),
            code_splitting: Some(CodeSplittingMode::Bool(false)),
            sourcemap: Some(SourceMapType::Inline),
            ..Default::default()
        }
    }
    fn output(&self) -> PathBuf {
        self.0.join("output.mjs")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Load,
    Close,
    RuntimeShutdown,
}

#[derive(Debug)]
struct Pause {
    phase: Phase,
    entered: SyncSender<()>,
    release: Arc<Mutex<Receiver<()>>>,
    closes: Arc<AtomicUsize>,
}
impl Pause {
    fn block(&self) -> Result<()> {
        self.entered.send(())?;
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))?;
        Ok(())
    }
}
impl Plugin for Pause {
    fn name(&self) -> Cow<'static, str> {
        "zfb-cancellation-native-pause".into()
    }
    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::Load | HookUsage::CloseBundle
    }
    async fn load(
        &self,
        _ctx: SharedLoadPluginContext,
        args: &HookLoadArgs<'_>,
    ) -> Result<Option<HookLoadOutput>> {
        if self.phase == Phase::Load && args.id.ends_with("plugin.ts") {
            self.block()?;
        }
        Ok(None)
    }
    async fn close_bundle(
        &self,
        _ctx: &PluginContext,
        _args: Option<&HookCloseBundleArgs<'_>>,
    ) -> Result<()> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        if self.phase == Phase::Close {
            self.block()?;
        } else if self.phase == Phase::RuntimeShutdown {
            let entered = self.entered.clone();
            let release = self.release.clone();
            tokio::task::spawn_blocking(move || {
                entered.send(()).unwrap();
                release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
            });
        }
        Ok(())
    }
}

fn paused(
    fixture: &Fixture,
    phase: Phase,
) -> (Job, Receiver<()>, SyncSender<()>, Arc<AtomicUsize>) {
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let closes = Arc::new(AtomicUsize::new(0));
    let job = Job::spawn(
        fixture.options("export default () => 17;"),
        vec![Pause::new_shared(Pause {
            phase,
            entered,
            release: Arc::new(Mutex::new(resume)),
            closes: closes.clone(),
        })],
    )
    .unwrap();
    (job, ready, release, closes)
}

fn pending(job: Job) -> Job {
    match job.wait(Duration::from_millis(5)) {
        Wait::Pending(job) => job,
        Wait::Joined(_) => panic!("live native work must retain the pending owner"),
    }
}

#[test]
fn native_success_is_joined_before_caller_publication() {
    let fixture = Fixture::new();
    let job = Job::spawn(fixture.options("export default () => 17;"), Vec::new()).unwrap();
    let cancel = job.cancellation();
    let completed = match job.wait(Duration::from_secs(10)) {
        Wait::Joined(completed) => completed,
        Wait::Pending(_) => panic!("tiny native generation did not finish"),
    };
    assert!(!fixture.output().exists());
    completed.publish(&fixture.output()).unwrap();
    assert!(std::fs::read_to_string(fixture.output())
        .unwrap()
        .contains("sourceMappingURL=data:"));
    assert_eq!(cancel.cancel(), CancelOutcome::TooLate);
}

#[test]
fn active_native_load_cancellation_reaches_a_checkpoint_and_never_publishes() {
    let fixture = Fixture::new();
    let (job, ready, release, closes) = paused(&fixture, Phase::Load);
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    let cancel = job.cancellation();
    assert_eq!(cancel.cancel(), CancelOutcome::Cancelled);
    let job = pending(job);
    assert!(!fixture.output().exists());
    assert_eq!(
        closes.load(Ordering::SeqCst),
        0,
        "do not close while native load is active"
    );
    release.send(()).unwrap();
    let completed = job.join();
    assert!(completed
        .result
        .as_ref()
        .unwrap_err()
        .to_string()
        .contains("plugin probe cancelled"));
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    assert!(completed.publish(&fixture.output()).is_err());
    assert!(!fixture.output().exists());
}

#[test]
fn timeout_retains_owner_without_implicitly_cancelling_or_closing() {
    let fixture = Fixture::new();
    let (job, ready, release, closes) = paused(&fixture, Phase::Load);
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    let job = pending(job);
    assert!(job.cancellation().check().is_ok());
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    assert!(!fixture.output().exists());
    release.send(()).unwrap();
    job.join().publish(&fixture.output()).unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}

#[test]
fn cancellation_during_close_preserves_cleanup_and_discards_late_success() {
    let fixture = Fixture::new();
    let (job, ready, release, closes) = paused(&fixture, Phase::Close);
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(job.cancellation().cancel(), CancelOutcome::Cancelled);
    let job = pending(job);
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    assert!(!fixture.output().exists());
    release.send(()).unwrap();
    let completed = job.join();
    assert!(
        completed.result.is_ok(),
        "generation completed before close was paused"
    );
    assert!(completed
        .publish(&fixture.output())
        .unwrap_err()
        .to_string()
        .contains("discarded"));
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    assert!(!fixture.output().exists());
}

#[test]
fn live_runtime_blocking_task_prevents_joined_claim_after_generation_and_close() {
    let fixture = Fixture::new();
    let (job, ready, release, closes) = paused(&fixture, Phase::RuntimeShutdown);
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    let job = pending(job);
    assert_eq!(job.cancellation().cancel(), CancelOutcome::Cancelled);
    assert!(!fixture.output().exists());
    release.send(()).unwrap();
    let completed = job.join();
    assert!(completed.result.is_ok());
    assert!(completed.publish(&fixture.output()).is_err());
    assert!(!fixture.output().exists());
}

#[test]
fn cancellation_after_join_wins_before_publication_and_preserves_siblings() {
    let fixture = Fixture::new();
    let sentinel = fixture.0.join("other-owner.mjs");
    std::fs::write(&sentinel, "other-owner").unwrap();
    let job = Job::spawn(fixture.options("export default 17;"), Vec::new()).unwrap();
    let cancel = job.cancellation();
    let completed = job.join();
    assert!(completed.result.is_ok());
    assert_eq!(cancel.cancel(), CancelOutcome::Cancelled);
    assert_eq!(cancel.cancel(), CancelOutcome::Cancelled);
    assert!(completed.publish(&fixture.output()).is_err());
    assert!(!fixture.output().exists());
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "other-owner");
}

#[test]
fn cancellation_after_commit_started_reports_too_late_not_false_success() {
    let fixture = Fixture::new();
    let job = Job::spawn(fixture.options("export default 17;"), Vec::new()).unwrap();
    let cancel = job.cancellation();
    let completed = job.join();
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let output = fixture.output();
    let publisher = thread::spawn(move || {
        completed.commit_with(|code| {
            entered.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(10)).unwrap();
            lifecycle::publish_new(&output, code)
        })
    });
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(cancel.cancel(), CancelOutcome::TooLate);
    release.send(()).unwrap();
    publisher.join().unwrap().unwrap();
    assert!(fixture.output().exists());
}

#[test]
fn concurrent_cancellation_requests_are_idempotent() {
    let cancellation = Cancellation::default();
    thread::scope(|scope| {
        for _ in 0..8 {
            let cancellation = cancellation.clone();
            scope.spawn(move || assert_eq!(cancellation.cancel(), CancelOutcome::Cancelled));
        }
    });
    assert!(cancellation.check().is_err());
}

#[test]
fn dropping_owner_requests_cancellation_and_joins_instead_of_detaching() {
    let exited = Arc::new(AtomicBool::new(false));
    let finished = exited.clone();
    let job = Job::spawn_worker(move |cancel| {
        let deadline = Instant::now();
        while cancel.check().is_ok() {
            assert!(deadline.elapsed() < Duration::from_secs(10));
            thread::yield_now();
        }
        finished.store(true, Ordering::Release);
        Ok("must never publish".into())
    })
    .unwrap();
    let cancellation = job.cancellation();
    drop(job);
    assert!(exited.load(Ordering::Acquire));
    assert_eq!(cancellation.cancel(), CancelOutcome::Cancelled);
}

#[test]
fn worker_panic_is_joined_and_never_published() {
    let fixture = Fixture::new();
    let job = Job::spawn_worker(|_| panic!("deliberate-owned-worker-panic")).unwrap();
    let error = job.join().publish(&fixture.output()).unwrap_err();
    assert!(error.to_string().contains("deliberate-owned-worker-panic"));
    assert!(!fixture.output().exists());
}

#[test]
fn native_build_error_preserves_the_primary_diagnostic_without_output() {
    let fixture = Fixture::new();
    let job = Job::spawn(
        fixture.options("import './absent-native-source.ts';"),
        Vec::new(),
    )
    .unwrap();
    let error = job.join().publish(&fixture.output()).unwrap_err();
    assert!(error.to_string().contains("absent-native-source.ts"));
    assert!(!fixture.output().exists());
}
