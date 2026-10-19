use futures_util::FutureExt;
use std::fmt::Debug;
use std::future::Future;
use std::panic::{resume_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::{AbortHandle, JoinHandle};

#[derive(Default)]
struct Registry(Vec<(&'static str, JoinHandle<String>)>);

impl Drop for Registry {
    fn drop(&mut self) {
        for (name, task) in &self.0 {
            eprintln!("[test-cleanup] abort-on-drop {name}; join unproven");
            task.abort();
        }
    }
}

/// Own long-lived test tasks so failed assertions still abort and join them.
#[derive(Clone, Default)]
pub struct TestTasks(Arc<Mutex<Registry>>);

impl TestTasks {
    pub fn spawn<F, T>(&self, name: &'static str, future: F) -> AbortHandle
    where
        F: Future<Output = T> + Send + 'static,
        T: Debug + Send + 'static,
    {
        let task = tokio::spawn(async move { format!("{:?}", future.await) });
        let handle = task.abort_handle();
        self.0.lock().unwrap().0.push((name, task));
        handle
    }

    async fn cleanup(&self, budget: Duration) -> Vec<String> {
        let _phase = crate::TestPhase::new("abort-and-join-test-tasks");
        let mut tasks = std::mem::take(&mut self.0.lock().unwrap().0);
        for (_, task) in &tasks {
            task.abort();
        }
        let deadline = tokio::time::Instant::now() + budget;
        let mut errors = Vec::new();
        for (name, mut task) in tasks.drain(..) {
            match tokio::time::timeout_at(deadline, &mut task).await {
                Ok(Err(error))
                    if error.is_cancelled() && tokio::time::Instant::now() < deadline =>
                {
                    eprintln!("[test-cleanup] joined cancelled {name}");
                }
                other => errors.push(format!("unexpected task completion {name}: {other:?}")),
            }
        }
        errors
    }
}

/// Preserve the first assertion panic while reporting cleanup separately.
/// A synchronous native call can still prevent runtime progress: phase receipts
/// and the external runner limit remain the backstop for that case.
pub async fn with_test_tasks<F, Fut>(test: F)
where
    F: FnOnce(TestTasks) -> Fut,
    Fut: Future<Output = ()>,
{
    let tasks = TestTasks::default();
    let result = AssertUnwindSafe(test(tasks.clone())).catch_unwind().await;
    let errors = tasks.cleanup(Duration::from_secs(10)).await;
    for error in &errors {
        eprintln!("[test-cleanup] {error}");
    }
    if let Err(panic) = result {
        resume_unwind(panic);
    }
    assert!(errors.is_empty(), "test cleanup failed: {errors:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn late_cancellation_observation_fails_cleanup_budget() {
        struct SlowDrop;
        impl Drop for SlowDrop {
            fn drop(&mut self) {
                std::thread::sleep(Duration::from_millis(40));
            }
        }
        let tasks = TestTasks::default();
        let owned = SlowDrop;
        tasks.spawn("slow-drop", async move {
            let _owned = owned;
            std::future::pending::<()>().await;
        });
        let errors = tasks.cleanup(Duration::from_millis(10)).await;
        assert!(!errors.is_empty(), "late cancellation cannot pass cleanup");
    }

    #[tokio::test]
    async fn abort_is_joined() {
        with_test_tasks(|tasks| async move {
            tasks.spawn("pending", std::future::pending::<()>());
        })
        .await;
    }

    async fn early_exit(panic: bool, error: bool) {
        with_test_tasks(|tasks| async move {
            let handle = tasks.spawn("early", async move {
                assert!(!panic, "task panic");
                if error {
                    Err("task error")
                } else {
                    Ok(())
                }
            });
            while !handle.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await;
    }

    #[tokio::test]
    #[should_panic(expected = "unexpected task completion")]
    async fn task_error_is_failure() {
        early_exit(false, true).await;
    }

    #[tokio::test]
    #[should_panic(expected = "unexpected task completion")]
    async fn task_panic_is_failure() {
        early_exit(true, false).await;
    }

    #[tokio::test]
    #[should_panic(expected = "unexpected task completion")]
    async fn early_success_is_failure() {
        early_exit(false, false).await;
    }

    #[tokio::test]
    async fn assertion_failure_still_joins_and_preserves_first_panic() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        struct Dropped(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let drop_flag = dropped.clone();
        let result = AssertUnwindSafe(with_test_tasks(|tasks| async move {
            let owned = Dropped(drop_flag);
            tasks.spawn("pending", async move {
                let _owned = owned;
                std::future::pending::<()>().await;
            });
            panic!("original assertion");
        }))
        .catch_unwind()
        .await;
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
        let panic = result.unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"original assertion"));
    }
}
