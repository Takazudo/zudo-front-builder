//! Probe-only cancellation, join ownership and publication authority.
//! A bounded wait is NOT bounded native shutdown. Pending retains the owner.
use super::lifecycle;
use anyhow::{anyhow, bail, Context, Result};
use rolldown::plugin::{
    __inner::SharedPluginable, HookBuildStartArgs, HookGenerateBundleArgs, HookLoadArgs,
    HookLoadOutput, HookRenderStartArgs, HookUsage, Plugin, PluginContext, SharedLoadPluginContext,
};
use rolldown::BundlerOptions;
use std::{
    borrow::Cow,
    path::Path,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const ACTIVE: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;
const FINISHED: u8 = 3;

#[derive(Clone, Debug, Default)]
pub(super) struct Cancellation(Arc<AtomicU8>);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum CancelOutcome {
    /// Publication cannot begin. This says nothing about worker termination.
    Cancelled,
    /// Publication already began, or the job was already consumed.
    TooLate,
}

impl Cancellation {
    /// Competes atomically with publication. Never acknowledge cancellation
    /// and subsequently publish. Repeated cancellation is idempotent.
    pub(super) fn cancel(&self) -> CancelOutcome {
        match self
            .0
            .compare_exchange(ACTIVE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(CANCELLED) => CancelOutcome::Cancelled,
            Err(_) => CancelOutcome::TooLate,
        }
    }

    fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Acquire) == CANCELLED {
            bail!("plugin probe cancelled; publication revoked");
        }
        Ok(())
    }
}

#[derive(Debug)]
struct CancellationChecks(Cancellation);
impl Plugin for CancellationChecks {
    fn name(&self) -> Cow<'static, str> {
        "zfb-plugin-probe-cancellation".into()
    }
    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::BuildStart | HookUsage::Load | HookUsage::RenderStart | HookUsage::GenerateBundle
    }
    async fn build_start(
        &self,
        _ctx: &PluginContext,
        _args: &HookBuildStartArgs<'_>,
    ) -> Result<()> {
        self.0.check()
    }
    async fn load(
        &self,
        _ctx: SharedLoadPluginContext,
        _args: &HookLoadArgs<'_>,
    ) -> Result<Option<HookLoadOutput>> {
        self.0.check()?;
        Ok(None)
    }
    async fn render_start(
        &self,
        _ctx: &PluginContext,
        _args: &HookRenderStartArgs<'_>,
    ) -> Result<()> {
        self.0.check()
    }
    async fn generate_bundle(
        &self,
        _ctx: &PluginContext,
        _args: &mut HookGenerateBundleArgs<'_>,
    ) -> Result<()> {
        self.0.check()
    }
    // Do not cancel close hooks or clear the driver concurrently with native
    // work. The existing normal-return lifecycle helper still owns cleanup.
}

#[must_use = "retain the native worker owner until it is joined"]
pub(super) struct Job {
    worker: Option<JoinHandle<Result<String>>>,
    cancellation: Cancellation,
}

#[must_use = "Pending still owns live work; it is not a completed cancellation"]
pub(super) enum Wait {
    Joined(Completed),
    Pending(Job),
}

impl Job {
    pub(super) fn spawn(
        options: BundlerOptions,
        mut plugins: Vec<SharedPluginable>,
    ) -> Result<Self> {
        Self::spawn_worker(move |cancellation| {
            cancellation.check()?;
            plugins.insert(
                0,
                CancellationChecks::new_shared(CancellationChecks(cancellation)),
            );
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?;
            let generated = runtime.block_on(lifecycle::generate_and_close(options, plugins));
            // Do not use shutdown_timeout/background: blocking tasks can survive
            // those calls. Even a failed generation retains this join boundary.
            drop(runtime);
            generated
        })
    }

    fn spawn_worker(
        work: impl FnOnce(Cancellation) -> Result<String> + Send + 'static,
    ) -> Result<Self> {
        let cancellation = Cancellation::default();
        let signal = cancellation.clone();
        let worker = thread::Builder::new()
            .name("zfb-plugin-probe".into())
            .spawn(move || work(signal))
            .context("spawn owned native plugin probe")?;
        Ok(Self {
            worker: Some(worker),
            cancellation,
        })
    }

    pub(super) fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }

    /// Returns ownership on timeout. It neither cancels implicitly nor detaches
    /// the thread. Joining a finished handle also collects panic/error results.
    pub(super) fn wait(self, timeout: Duration) -> Wait {
        let start = Instant::now();
        loop {
            if self.worker.as_ref().expect("owned worker").is_finished() {
                return Wait::Joined(self.join());
            }
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Wait::Pending(self);
            }
            thread::sleep(remaining.min(Duration::from_millis(1)));
        }
    }

    /// Intentionally unbounded. Use wait() to retain a still-pending owner.
    pub(super) fn join(mut self) -> Completed {
        let result = self
            .worker
            .take()
            .expect("owned worker")
            .join()
            .unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("non-string panic");
                Err(anyhow!("native plugin worker panicked: {message}"))
            });
        Completed {
            result,
            cancellation: self.cancellation.clone(),
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.cancellation.cancel();
            // Safety backstop, NOT a bounded Drop. Silently detaching here would
            // make cancellation/cleanup claims false. Async production callers
            // must not adopt this blocking owner without a shutdown design.
            let _ = worker.join();
        }
    }
}

/// The only job object allowed to publish. It is constructed only after join;
/// the native worker never receives a publication handle or this object.
#[must_use = "publish the joined output or explicitly discard it"]
pub(super) struct Completed {
    result: Result<String>,
    cancellation: Cancellation,
}

impl Completed {
    pub(super) fn publish(self, path: &Path) -> Result<()> {
        self.commit_with(|code| lifecycle::publish_new(path, code))
    }

    fn commit_with(self, write: impl FnOnce(&str) -> Result<()>) -> Result<()> {
        let code = match self.result {
            Ok(code) => code,
            Err(error) => {
                let _ = self.cancellation.0.compare_exchange(
                    ACTIVE,
                    FINISHED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                return Err(error);
            }
        };
        if self
            .cancellation
            .0
            .compare_exchange(ACTIVE, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            bail!("plugin probe cancelled; joined output discarded without publication");
        }
        // Cancellation after this linearization point explicitly returns
        // TooLate. It cannot report success while the synchronous write runs.
        let result = write(&code);
        self.cancellation.0.store(FINISHED, Ordering::Release);
        result
    }
}

#[cfg(test)]
#[path = "job_tests.rs"]
mod tests;
