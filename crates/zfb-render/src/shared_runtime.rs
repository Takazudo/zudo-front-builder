//! Deno adapter for the shared foreground platform in combined-feature binaries.
//! Default-only Deno builds retain Deno's own foreground platform unchanged.
use deno_core::{error::CoreError, JsRuntime, PollEventLoopOptions, RuntimeOptions};
use std::future::{poll_fn, Future};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use zfb_js_host::native::platform::ForegroundQueue;

pub(crate) struct SharedRuntime {
    foreground: ForegroundRegistration,
    runtime: JsRuntime,
}
struct ForegroundRegistration {
    queue: Arc<ForegroundQueue>,
    spawner: deno_core::V8CrossThreadTaskSpawner,
    key: usize,
    sleeper: Option<Pin<Box<tokio::time::Sleep>>>,
}
impl Drop for ForegroundRegistration {
    fn drop(&mut self) {
        self.queue.close(self.key);
    }
}
impl ForegroundRegistration {
    fn poll(&mut self, cx: &mut Context<'_>) {
        let (tasks, deadline) = self.queue.take_ready(cx.waker());
        for task in tasks {
            self.spawner.spawn(move |_| task.run());
        }
        if let Some(deadline) = deadline {
            let deadline = tokio::time::Instant::from_std(deadline);
            match &mut self.sleeper {
                Some(timer) => timer.as_mut().reset(deadline),
                None => self.sleeper = Some(Box::pin(tokio::time::sleep_until(deadline))),
            }
            if self.sleeper.as_mut().unwrap().as_mut().poll(cx).is_ready() {
                cx.waker().wake_by_ref();
            }
        } else {
            self.sleeper = None;
        }
    }
    async fn drive<F: Future>(&mut self, future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        poll_fn(|cx| {
            self.poll(cx);
            future.as_mut().poll(cx)
        })
        .await
    }
}
impl SharedRuntime {
    pub fn new(options: RuntimeOptions) -> Self {
        // ZFB installs ops-only extensions; authored JS runs only after this
        // constructor registers the queue. Do not add eager JS extensions here.
        let mut runtime = JsRuntime::new(options);
        let spawner = runtime
            .op_state()
            .borrow()
            .borrow::<deno_core::V8CrossThreadTaskSpawner>()
            .clone();
        let (key, queue) = ForegroundQueue::register(runtime.v8_isolate());
        Self {
            foreground: ForegroundRegistration {
                key,
                queue,
                spawner,
                sleeper: None,
            },
            runtime,
        }
    }
    pub async fn run_event_loop(&mut self, options: PollEventLoopOptions) -> Result<(), CoreError> {
        self.foreground
            .drive(self.runtime.run_event_loop(options))
            .await
    }
    pub async fn with_event_loop_promise<'fut, T, E>(
        &mut self,
        future: impl Future<Output = Result<T, E>> + Unpin + 'fut,
        options: PollEventLoopOptions,
    ) -> Result<T, CoreError>
    where
        CoreError: From<E>,
    {
        self.foreground
            .drive(self.runtime.with_event_loop_promise(future, options))
            .await
    }
    pub fn poll_event_loop(
        &mut self,
        cx: &mut Context<'_>,
        options: PollEventLoopOptions,
    ) -> Poll<Result<(), CoreError>> {
        self.foreground.poll(cx);
        self.runtime.poll_event_loop(cx, options)
    }
}
impl Deref for SharedRuntime {
    type Target = JsRuntime;
    fn deref(&self) -> &JsRuntime {
        &self.runtime
    }
}
impl DerefMut for SharedRuntime {
    fn deref_mut(&mut self) -> &mut JsRuntime {
        &mut self.runtime
    }
}
