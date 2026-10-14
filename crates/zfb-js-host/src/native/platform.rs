//! Wake-driven foreground V8 tasks. Background workers never enter an isolate.
use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::task::Waker;
use std::time::{Duration, Instant};

static QUEUES: LazyLock<Mutex<HashMap<usize, Weak<ForegroundQueue>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Default)]
struct QueueState {
    closed: bool,
    ready: Vec<v8::Task>,
    delayed: Vec<(Instant, v8::Task)>,
    waker: Option<Waker>,
}

#[derive(Default)]
pub(super) struct ForegroundQueue {
    state: Mutex<QueueState>,
}
impl ForegroundQueue {
    pub fn register(isolate: &v8::Isolate) -> (usize, Arc<Self>) {
        // SAFETY: upstream 147.4.0 declares UnsafeRawIsolatePtr repr(transparent)
        // over its native pointer. Use its bits ONLY as a registry key, never
        // dereference/reconstruct an isolate or use it from another thread.
        const _: () =
            assert!(std::mem::size_of::<v8::UnsafeRawIsolatePtr>() == std::mem::size_of::<usize>());
        let key = unsafe {
            std::mem::transmute::<v8::UnsafeRawIsolatePtr, usize>(isolate.as_raw_isolate_ptr())
        };
        let queue = Arc::new(Self::default());
        QUEUES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, Arc::downgrade(&queue));
        (key, queue)
    }

    pub fn take_ready(&self, waker: &Waker) -> (Vec<v8::Task>, Option<Instant>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Register before checking work; posting between this check and await
        // wakes the same driver rather than losing a foreground notification.
        state.waker = Some(waker.clone());
        let now = Instant::now();
        let mut index = 0;
        while index < state.delayed.len() {
            if state.delayed[index].0 <= now {
                let (_, task) = state.delayed.swap_remove(index);
                state.ready.push(task);
            } else {
                index += 1;
            }
        }
        let deadline = state.delayed.iter().map(|(at, _)| *at).min();
        (std::mem::take(&mut state.ready), deadline)
    }

    pub fn status(&self) -> (bool, Option<Instant>) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (
            !state.ready.is_empty(),
            state.delayed.iter().map(|(at, _)| *at).min(),
        )
    }

    fn post(&self, task: v8::Task, delay: Option<Duration>) {
        let wake = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return;
            }
            if let Some(at) = delay.and_then(|delay| Instant::now().checked_add(delay)) {
                state.delayed.push((at, task));
            } else {
                state.ready.push(task);
            }
            state.waker.clone()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }

    pub fn close(&self, key: usize) {
        QUEUES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
        let (ready, delayed) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.closed = true;
            state.waker = None;
            (
                std::mem::take(&mut state.ready),
                std::mem::take(&mut state.delayed),
            )
        };
        // Queued tasks are discarded while the owning isolate is still alive,
        // outside the mutex. Racing/late posts are discarded; upstream V8
        // cancellation governs their lifetime, just as in the pinned baseline.
        drop((ready, delayed));
    }
}

pub(super) struct NativePlatform;
impl NativePlatform {
    fn post(isolate: *mut c_void, task: v8::Task, delay: Option<Duration>) {
        // PlatformImpl is entered through upstream's C ABI trampoline. A custom
        // executor waker may panic; never unwind it through native V8 frames.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let queue = QUEUES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(isolate as usize))
                .and_then(Weak::upgrade);
            if let Some(queue) = queue {
                queue.post(task, delay);
            }
        }));
    }
}
impl v8::PlatformImpl for NativePlatform {
    fn post_task(&self, isolate: *mut c_void, task: v8::Task) {
        Self::post(isolate, task, None);
    }
    fn post_non_nestable_task(&self, isolate: *mut c_void, task: v8::Task) {
        Self::post(isolate, task, None);
    }
    fn post_delayed_task(&self, isolate: *mut c_void, task: v8::Task, seconds: f64) {
        Self::post(isolate, task, Duration::try_from_secs_f64(seconds).ok());
    }
    fn post_non_nestable_delayed_task(&self, isolate: *mut c_void, task: v8::Task, seconds: f64) {
        self.post_delayed_task(isolate, task, seconds);
    }
    fn post_idle_task(&self, _isolate: *mut c_void, _task: v8::IdleTask) {
        // Idle support is disabled when constructing this platform.
    }
}
