use super::super::{job::CancelOutcome, lifecycle};
use super::*;
use rolldown::plugin::HookCloseBundleArgs;
use rolldown::plugin::{
    HookLoadArgs, HookLoadOutput, HookUsage, Plugin, PluginContext, SharedLoadPluginContext,
};
use rolldown_common::{CodeSplittingMode, OutputFormat, Platform, SourceMapType};
use std::borrow::Cow;
use std::{
    path::PathBuf,
    sync::{
        atomic::AtomicUsize,
        mpsc::{self, Receiver, SyncSender},
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
            let (started, running) = mpsc::sync_channel(1);
            tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                entered.send(()).unwrap();
                release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
            });
            // Runtime shutdown may discard a blocking task that has not started.
            // Establish active work before allowing close/runtime teardown.
            running.recv_timeout(Duration::from_secs(10))?;
        }
        Ok(())
    }
}

// Declared after the supervisor, so panic unwinding releases every gate before
// the root owner's unbounded drain. try_send cannot strand cleanup.
struct Release(SyncSender<()>);
impl Drop for Release {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

fn paused(
    owner: &Supervisor,
    fixture: &Fixture,
    phase: Phase,
    deadline: Option<Instant>,
) -> (Request, Receiver<()>, Release) {
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let request = owner
        .submit(
            fixture.options("export default () => 17;"),
            vec![Pause::new_shared(Pause {
                phase,
                entered,
                release: Arc::new(Mutex::new(resume)),
                closes: Arc::new(AtomicUsize::new(0)),
            })],
            deadline,
        )
        .unwrap();
    (request, ready, Release(release))
}
fn until(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "test deadlock watchdog"
        );
        thread::yield_now();
    }
}
fn joined(request: &mut Request) -> Joined {
    let mut result = None;
    until(|| {
        result = request.try_take();
        result.is_some()
    });
    result.unwrap()
}
fn busy(owner: &Supervisor, fixture: &Fixture) {
    assert!(matches!(
        owner.submit(fixture.options("export default 17;"), vec![], None),
        Err(AdmissionError::Busy)
    ));
}

#[test]
fn current_thread_executor_continues_after_dropping_a_polled_native_request() {
    let fixture = Fixture::new();
    let mut owner = Supervisor::new(1).unwrap();
    let (mut request, ready, release) = paused(&owner, &fixture, Phase::Load, None);
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        std::future::poll_fn(|cx| {
            assert!(Pin::new(&mut request).poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(request);
        // The paused native hook cannot finish until this executor makes progress.
        assert_eq!(tokio::spawn(async { 17 }).await.unwrap(), 17);
        assert_eq!(owner.status().len(), 1);
        assert!(owner.status()[0].cancellation_requested);
        assert!(!owner.status()[0].joined);
        release.0.send(()).unwrap();
    });
    owner.drain().unwrap();
    assert!(!fixture.output().exists());
}

#[test]
fn capacity_one_survives_cancelled_load_close_and_runtime_teardown() {
    for phase in [Phase::Load, Phase::Close, Phase::RuntimeShutdown] {
        let fixture = Fixture::new();
        let retry = Fixture::new();
        let mut owner = Supervisor::new(1).unwrap();
        let (mut request, ready, release) = paused(&owner, &fixture, phase, None);
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(request.cancel(), CancelOutcome::Cancelled);
        for _ in 0..20 {
            busy(&owner, &retry);
        }
        let status = owner.status();
        assert_eq!(status[0].id, request.id);
        assert!(!status[0].joined);
        assert!(
            matches!(owner.shutdown().unwrap(), Shutdown::Pending(work) if work.len() == 1 && work[0].id == request.id)
        );
        assert!(matches!(
            owner.submit(retry.options("export default 1;"), vec![], None),
            Err(AdmissionError::Closed)
        ));
        release.0.send(()).unwrap();
        assert!(joined(&mut request).publish(&fixture.output()).is_err());
        owner.drain().unwrap();
        assert_eq!(owner.shutdown().unwrap(), Shutdown::Drained);
        assert!(!fixture.output().exists());
    }
}

#[test]
fn cancellation_deadline_expires_without_client_polling_and_slot_reopens_only_after_join() {
    let fixture = Fixture::new();
    let retry = Fixture::new();
    let mut owner = Supervisor::new(1).unwrap();
    let (mut request, ready, release) = paused(&owner, &fixture, Phase::Load, None);
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    request.delivery.cancellation.expire_for_test();
    // Do not poll the future or call status (which also checks deadline).
    until(|| request.delivery.cancellation.is_cancelled());
    let status = owner.status();
    assert!(status[0].deadline_exceeded && status[0].cancellation_requested && !status[0].joined);
    for _ in 0..20 {
        busy(&owner, &retry);
    }
    release.0.send(()).unwrap();
    let completed = joined(&mut request);
    busy(&owner, &retry); // bounded completed output too
    assert!(completed.publish(&fixture.output()).is_err());
    let mut next = owner
        .submit(retry.options("export default 19;"), vec![], None)
        .unwrap();
    joined(&mut next).publish(&retry.output()).unwrap();
    owner.drain().unwrap();
}

#[test]
fn capacity_two_collects_fast_job_beside_blocked_job_without_head_of_line_blocking() {
    let slow = Fixture::new();
    let fast = Fixture::new();
    let again = Fixture::new();
    let mut owner = Supervisor::new(2).unwrap();
    let (slow_request, ready, release) = paused(&owner, &slow, Phase::Load, None);
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    let mut fast_request = owner
        .submit(fast.options("export default 23;"), vec![], None)
        .unwrap();
    joined(&mut fast_request).publish(&fast.output()).unwrap();
    let mut next = owner
        .submit(again.options("export default 29;"), vec![], None)
        .unwrap();
    joined(&mut next).publish(&again.output()).unwrap();
    assert!(owner
        .status()
        .iter()
        .any(|status| status.id == slow_request.id && !status.joined));
    drop(slow_request);
    release.0.send(()).unwrap();
    owner.drain().unwrap();
    assert!(!slow.output().exists());
}

#[test]
fn abandoned_receiver_late_success_is_discarded_and_capacity_reconciles() {
    for phase in [Phase::Close, Phase::RuntimeShutdown] {
        let fixture = Fixture::new();
        let mut owner = Supervisor::new(1).unwrap();
        let (request, ready, release) = paused(&owner, &fixture, phase, None);
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        let delivery = request.delivery.clone();
        drop(request);
        busy(&owner, &fixture);
        release.0.send(()).unwrap();
        until(|| owner.status().is_empty());
        assert!(delivery.mailbox.lock().unwrap().result.is_none());
        assert!(!fixture.output().exists());
        owner.drain().unwrap();
    }
}

#[test]
fn shutdown_revokes_delivered_unpublished_output_and_preserves_other_owner() {
    let fixture = Fixture::new();
    let mut owner = Supervisor::new(1).unwrap();
    let mut request = owner
        .submit(fixture.options("export default 31;"), vec![], None)
        .unwrap();
    let completed = joined(&mut request);
    let sentinel = fixture.0.join("other-owner.mjs");
    std::fs::write(&sentinel, "untouched").unwrap();
    owner.drain().unwrap();
    assert_eq!(owner.shutdown().unwrap(), Shutdown::Drained);
    assert!(completed.publish(&fixture.output()).is_err());
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "untouched");
    assert!(!fixture.output().exists());
}

#[test]
fn deadline_revokes_already_delivered_output_without_receiver_polling() {
    let fixture = Fixture::new();
    let mut owner = Supervisor::new(1).unwrap();
    let mut request = owner
        .submit(fixture.options("export default 31;"), vec![], None)
        .unwrap();
    let completed = joined(&mut request);
    request.delivery.cancellation.expire_for_test();
    until(|| request.delivery.cancellation.is_cancelled());
    assert!(completed.publish(&fixture.output()).is_err());
    owner.drain().unwrap();
    assert!(!fixture.output().exists());
}

#[test]
fn shutdown_and_deadline_after_publication_commit_never_acknowledge_cancellation() {
    let fixture = Fixture::new();
    let mut owner = Supervisor::new(1).unwrap();
    let mut request = owner
        .submit(fixture.options("export default 37;"), vec![], None)
        .unwrap();
    let completed = joined(&mut request);
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let output = fixture.output();
    thread::scope(|scope| {
        let publisher = scope.spawn(move || {
            completed.commit_with(|code| {
                entered.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(10)).unwrap();
                lifecycle::publish_new(&output, code)
            })
        });
        let release = Release(release);
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        request.delivery.cancellation.expire_for_test();
        until(|| request.delivery.cancellation.deadline_exceeded());
        owner.shutdown().unwrap();
        assert_eq!(request.cancel(), CancelOutcome::TooLate);
        assert!(!request.delivery.cancellation.is_cancelled());
        release.0.send(()).unwrap();
        publisher.join().unwrap().unwrap();
    });
    owner.drain().unwrap();
    assert!(fixture.output().exists());
}

#[test]
fn setup_failure_and_panic_roll_back_reserved_admission_exactly_once() {
    let fixture = Fixture::new();
    let mut owner = Supervisor::new(1).unwrap();
    let result = owner.submit_with(None, |_| Err(anyhow!("injected OS thread spawn failure")));
    assert!(
        matches!(result, Err(AdmissionError::Setup(message)) if message.contains("injected OS thread spawn failure"))
    );
    assert!(owner.status().is_empty());
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = owner.submit_with(None, |_| panic!("injected setup unwind"));
    }))
    .is_err());
    assert!(owner.status().is_empty());
    let mut next = owner
        .submit(fixture.options("export default 41;"), vec![], None)
        .unwrap();
    joined(&mut next).publish(&fixture.output()).unwrap();
    owner.drain().unwrap();
}

#[test]
fn reservation_precedes_spawn_and_cancellation_uses_same_signal() {
    let mut owner = Supervisor::new(1).unwrap();
    let fixture = Fixture::new();
    let mut request = owner
        .submit_with(None, |signal| {
            assert_eq!(owner.status().len(), 1);
            busy(&owner, &fixture);
            Job::spawn_worker_with(signal, |_| Ok("export default 43;".into()))
        })
        .unwrap();
    assert_eq!(request.cancel(), CancelOutcome::Cancelled);
    assert!(joined(&mut request).publish(&fixture.output()).is_err());
    owner.drain().unwrap();
}

#[test]
fn native_error_and_worker_panic_keep_diagnostics_and_reopen_capacity() {
    let fixture = Fixture::new();
    let mut owner = Supervisor::new(1).unwrap();
    let mut request = owner
        .submit(
            fixture.options("import './missing-native-module';"),
            vec![],
            None,
        )
        .unwrap();
    assert!(joined(&mut request)
        .publish(&fixture.output())
        .unwrap_err()
        .to_string()
        .contains("missing-native-module"));
    let mut panic_request = owner
        .submit_with(None, |signal| {
            Job::spawn_worker_with(signal, |_| panic!("supervised worker panic"))
        })
        .unwrap();
    assert!(joined(&mut panic_request)
        .publish(&fixture.output())
        .unwrap_err()
        .to_string()
        .contains("supervised worker panic"));
    let mut next = owner
        .submit(fixture.options("export default 47;"), vec![], None)
        .unwrap();
    joined(&mut next).publish(&fixture.output()).unwrap();
    owner.drain().unwrap();
}

#[test]
fn dropping_unpolled_successful_receiver_discards_queued_delivery() {
    let fixture = Fixture::new();
    let mut owner = Supervisor::new(1).unwrap();
    let request = owner
        .submit(fixture.options("export default 53;"), vec![], None)
        .unwrap();
    until(|| owner.status()[0].joined);
    let delivery = request.delivery.clone();
    drop(request);
    assert!(delivery.mailbox.lock().unwrap().result.is_none());
    until(|| owner.status().is_empty());
    owner.drain().unwrap();
    assert!(!fixture.output().exists());
}

#[derive(Debug)]
struct ExitGate {
    entered: SyncSender<()>,
    release: Receiver<()>,
}
impl Drop for ExitGate {
    fn drop(&mut self) {
        self.entered.send(()).unwrap();
        self.release.recv_timeout(Duration::from_secs(10)).unwrap();
    }
}
thread_local! {
    static EXIT_GATE: std::cell::RefCell<Option<ExitGate>> = const { std::cell::RefCell::new(None) };
}
#[derive(Debug)]
struct PauseThreadExit(Mutex<Option<ExitGate>>);
impl Plugin for PauseThreadExit {
    fn name(&self) -> Cow<'static, str> {
        "native-thread-exit-gate".into()
    }
    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::CloseBundle
    }
    async fn close_bundle(
        &self,
        _ctx: &PluginContext,
        _args: Option<&HookCloseBundleArgs<'_>>,
    ) -> Result<()> {
        EXIT_GATE.with(|gate| *gate.borrow_mut() = self.0.lock().unwrap().take());
        Ok(())
    }
}

#[test]
fn blocked_native_thread_destructor_does_not_block_other_joins_or_deadlines() {
    let slow = Fixture::new();
    let fast = Fixture::new();
    let retry = Fixture::new();
    let mut owner = Supervisor::new(2).unwrap();
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let mut request = owner
        .submit(
            slow.options("export default 59;"),
            vec![PauseThreadExit::new_shared(PauseThreadExit(Mutex::new(
                Some(ExitGate {
                    entered,
                    release: resume,
                }),
            )))],
            None,
        )
        .unwrap();
    let release = Release(release);
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(!owner.status()[0].joined);
    let mut second = owner
        .submit(fast.options("export default 61;"), vec![], None)
        .unwrap();
    joined(&mut second).publish(&fast.output()).unwrap();
    let (third, third_ready, third_release) = paused(&owner, &retry, Phase::Load, None);
    third_ready.recv_timeout(Duration::from_secs(10)).unwrap();
    third.delivery.cancellation.expire_for_test();
    until(|| third.delivery.cancellation.is_cancelled());
    assert!(matches!(owner.shutdown().unwrap(), Shutdown::Pending(work) if work.len() == 2));
    assert!(request.try_take().is_none());
    release.0.send(()).unwrap();
    third_release.0.send(()).unwrap();
    assert!(joined(&mut request).publish(&slow.output()).is_err());
    owner.drain().unwrap();
    assert_eq!(owner.shutdown().unwrap(), Shutdown::Drained);
    assert!(!slow.output().exists());
}

#[test]
fn failed_supervisor_never_becomes_successfully_drained() {
    let mut owner = Supervisor::new(1).unwrap();
    let registry = owner.registry.clone();
    let failed = owner.failed.clone();
    owner.reapers.push(thread::spawn(move || {
        guarded(&registry, &failed, || panic!("injected supervisor failure"))
    }));
    until(|| owner.failed.load(Ordering::Acquire));
    assert!(owner.shutdown().is_err());
    assert!(owner.drain().is_err());
    assert!(owner.shutdown().is_err());
}

#[test]
fn dropping_transferred_output_releases_capacity_without_publishing() {
    let fixture = Fixture::new();
    let mut owner = Supervisor::new(1).unwrap();
    let mut request = owner
        .submit(fixture.options("export default 67;"), vec![], None)
        .unwrap();
    let completed = joined(&mut request);
    busy(&owner, &fixture);
    drop(request); // publication authority already transferred
    drop(completed);
    let mut next = owner
        .submit(fixture.options("export default 71;"), vec![], None)
        .unwrap();
    joined(&mut next).publish(&fixture.output()).unwrap();
    owner.drain().unwrap();
}

#[test]
fn elapsed_real_instant_deadline_rejects_native_output() {
    let fixture = Fixture::new();
    let mut owner = Supervisor::new(1).unwrap();
    let mut request = owner
        .submit(
            fixture.options("export default 73;"),
            vec![],
            Some(Instant::now()),
        )
        .unwrap();
    assert!(request.delivery.cancellation.deadline_exceeded());
    assert!(joined(&mut request).publish(&fixture.output()).is_err());
    owner.drain().unwrap();
    assert!(!fixture.output().exists());
}
