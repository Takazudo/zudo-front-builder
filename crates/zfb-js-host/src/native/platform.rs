//! Wake-driven foreground V8 tasks. Background workers never enter an isolate.
use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::task::Waker;
use std::time::{Duration, Instant};

static QUEUES: LazyLock<Mutex<HashMap<usize, Weak<ForegroundQueue>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
pub(super) fn is_registered(queue: &Weak<ForegroundQueue>) -> bool {
    QUEUES
        .lock()
        .unwrap()
        .values()
        .any(|entry| entry.ptr_eq(queue))
}

#[derive(Default)]
struct QueueState {
    closed: bool,
    ready: Vec<v8::Task>,
    delayed: Vec<(Instant, v8::Task)>,
    waker: Option<Waker>,
}

#[derive(Default)]
pub struct ForegroundQueue {
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
        self.take_ready_at(waker, Instant::now())
    }

    fn take_ready_at(&self, waker: &Waker, now: Instant) -> (Vec<v8::Task>, Option<Instant>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return (Vec::new(), None);
        }
        // Register before checking work; posting between this check and await
        // wakes the same driver rather than losing a foreground notification.
        state.waker = Some(waker.clone());
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

pub struct NativePlatform;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::NativeHost;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    struct Notification {
        calls: AtomicUsize,
        thread: std::thread::Thread,
        panic: bool,
    }

    impl Wake for Notification {
        fn wake(self: Arc<Self>) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.thread.unpark();
            assert!(!self.panic, "executor waker panic fixture");
        }
    }

    fn notification(panic: bool) -> (Arc<Notification>, Waker) {
        let signal = Arc::new(Notification {
            calls: AtomicUsize::new(0),
            thread: std::thread::current(),
            panic,
        });
        (signal.clone(), signal.into())
    }

    fn begin_wasm(host: &mut NativeHost, waker: &Waker) {
        begin_wasm_answer(host, waker, 42);
    }

    fn begin_wasm_answer(host: &mut NativeHost, waker: &Waker, answer: u8) {
        let (tasks, _) = host.foreground.take_ready(waker);
        for task in tasks {
            task.run();
        }
        // A retained compiled module forces V8's cache-hit foreground-task path.
        let source = format!("const bytes = new Uint8Array([0,97,115,109,1,0,0,0,1,5,1,96,0,1,127,3,2,1,0,7,10,1,6,97,110,115,119,101,114,0,0,10,6,1,4,0,65,{answer},11]); globalThis.cachedModule = new WebAssembly.Module(bytes); globalThis.compilation = WebAssembly.compile(bytes);");
        host.execute_script("zfb:foreground-test", &source).unwrap();
        host.isolate.perform_microtask_checkpoint();
    }

    fn posted_tasks(host: &NativeHost, waker: &Waker) -> Vec<v8::Task> {
        let limit = Instant::now() + Duration::from_secs(10);
        loop {
            let (tasks, _) = host.foreground.take_ready(waker);
            if !tasks.is_empty() {
                return tasks;
            }
            let now = Instant::now();
            assert!(now < limit, "native foreground post watchdog expired");
            std::thread::park_timeout(limit - now);
        }
    }

    fn finish_wasm(host: &mut NativeHost) {
        let value = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(host.evaluate_config("export default {answer:new WebAssembly.Instance(await globalThis.compilation).exports.answer()};"))
            .unwrap();
        assert_eq!(value, serde_json::json!({"answer":42}));
    }

    #[test]
    fn delayed_native_tasks_become_eligible_and_wake_the_driver() {
        let mut host = NativeHost::new_config().unwrap();
        let (signal, waker) = notification(false);
        begin_wasm(&mut host, &waker);
        let tasks = posted_tasks(&host, &waker);
        let calls = signal.calls.load(Ordering::SeqCst);
        for task in tasks {
            host.foreground.post(task, Some(Duration::from_millis(20)));
        }
        assert!(signal.calls.load(Ordering::SeqCst) > calls);
        let deadline = host.foreground.status().1.unwrap();
        let delayed = host.foreground.state.lock().unwrap().delayed.len();
        let (ready, next) = host
            .foreground
            .take_ready_at(&waker, deadline - Duration::from_secs(1));
        assert_eq!(host.foreground.state.lock().unwrap().delayed.len(), delayed);
        assert_eq!(next, Some(deadline));
        for task in ready {
            task.run();
        }
        // The actual driver executes the real native task once it is eligible.
        finish_wasm(&mut host);
        assert!(host.foreground.status().1.is_none());
    }

    #[test]
    fn native_callback_contains_panicking_executor_waker() {
        let mut host = NativeHost::new_config().unwrap();
        let (signal, waker) = notification(true);
        // Real V8 compilation posts through rusty_v8's extern-C trampoline.
        begin_wasm(&mut host, &waker);
        let mut tasks = posted_tasks(&host, &waker);
        let limit = Instant::now() + Duration::from_secs(10);
        while signal.calls.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < limit, "native wake watchdog expired");
            std::thread::park_timeout(limit.saturating_duration_since(Instant::now()));
        }
        let (_, safe_waker) = notification(false);
        tasks.extend(host.foreground.take_ready(&safe_waker).0);
        for task in tasks {
            host.foreground.post(task, None);
        }
        finish_wasm(&mut host);
    }

    #[test]
    fn close_discards_native_tasks_and_late_posts_without_retaining_queue() {
        for mode in 0..4 {
            let mut host = NativeHost::new_config().unwrap();
            let (_, waker) = notification(false);
            begin_wasm_answer(&mut host, &waker, 50 + mode);
            let tasks = posted_tasks(&host, &waker);
            let key = *QUEUES
                .lock()
                .unwrap()
                .iter()
                .find(|(_, queue)| queue.ptr_eq(&Arc::downgrade(&host.foreground)))
                .unwrap()
                .0;
            let queue = host.foreground.clone();
            let weak = Arc::downgrade(&queue);
            if mode < 2 {
                for task in tasks {
                    queue.post(task, (mode == 1).then_some(Duration::from_secs(60)));
                }
                queue.close(key);
            } else {
                queue.close(key);
                for task in tasks {
                    if mode == 2 {
                        // A poster that captured the Arc before close.
                        queue.post(task, None);
                    } else {
                        // Rust helper registry-miss coverage, not a new ABI entry.
                        NativePlatform::post(key as *mut c_void, task, None);
                    }
                }
            }
            assert!(!QUEUES.lock().unwrap().contains_key(&key));
            assert_eq!(queue.status(), (false, None));
            assert!(queue.state.lock().unwrap().closed);
            assert!(queue.take_ready(&waker).0.is_empty());
            assert!(queue.state.lock().unwrap().waker.is_none());
            host.execute_script("zfb:still-alive", "6 * 7").unwrap();
            drop(queue);
            drop(host);
            assert!(weak.upgrade().is_none());
        }
    }
}
