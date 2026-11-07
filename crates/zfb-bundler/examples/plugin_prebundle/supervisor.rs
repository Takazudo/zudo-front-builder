//! Prototype-private ownership. Only the synchronous root owner may drain.
use super::job::{CancelOutcome, Cancellation, Completed, Job};
use anyhow::{anyhow, Result};
use rolldown::{plugin::__inner::SharedPluginable, BundlerOptions};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Waker},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

type Id = u64;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum AdmissionError {
    Busy,
    Closed,
    Setup(String),
}
impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for AdmissionError {}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Status {
    pub id: Id,
    pub joined: bool,
    pub cancellation_requested: bool,
    pub deadline_exceeded: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Shutdown {
    Pending(Vec<Status>),
    Drained,
}

#[derive(Default)]
struct Mailbox {
    result: Option<Completed>,
    waker: Option<Waker>,
}
struct Delivery {
    mailbox: Mutex<Mailbox>,
    released: AtomicBool,
    cancellation: Cancellation,
}
struct Entry {
    lane: usize,
    job: Option<Job>,
    joined: bool,
    delivery: Arc<Delivery>,
}
struct Registry {
    closed: bool,
    next_id: Id,
    capacity: usize,
    entries: BTreeMap<Id, Entry>,
}

/// Not clonable and never owned by a request. Construct/drain on a synchronous
/// root thread; Drop is an intentionally unbounded ownership safety backstop.
pub(super) struct Supervisor {
    registry: Arc<Mutex<Registry>>,
    reapers: Vec<JoinHandle<()>>,
    failed: Arc<AtomicBool>,
}

impl Supervisor {
    pub(super) fn new(capacity: usize) -> Result<Self> {
        anyhow::ensure!(capacity > 0, "native supervisor capacity must be positive");
        let registry = Arc::new(Mutex::new(Registry {
            closed: false,
            next_id: 0,
            capacity,
            entries: BTreeMap::new(),
        }));
        let mut owner = Self {
            registry,
            reapers: Vec::new(),
            failed: Arc::new(AtomicBool::new(false)),
        };
        for lane in 0..capacity {
            let shared = owner.registry.clone();
            let failed = owner.failed.clone();
            owner.reapers.push(
                thread::Builder::new()
                    .name(format!("zfb-probe-join-{lane}"))
                    .spawn(move || {
                        guarded(&shared, &failed, || collect_lane(&shared, lane, &failed))
                    })?,
            );
        }
        let shared = owner.registry.clone();
        let failed = owner.failed.clone();
        owner.reapers.push(
            thread::Builder::new()
                .name("zfb-probe-deadlines".into())
                .spawn(move || guarded(&shared, &failed, || supervise(&shared, &failed)))?,
        );
        Ok(owner)
    }

    pub(super) fn submit(
        &self,
        options: BundlerOptions,
        plugins: Vec<SharedPluginable>,
        cancellation_deadline: Option<Instant>,
    ) -> std::result::Result<Request, AdmissionError> {
        self.submit_with(cancellation_deadline, |signal| {
            Job::spawn_with(signal, options, plugins)
        })
    }

    fn submit_with(
        &self,
        deadline: Option<Instant>,
        spawn: impl FnOnce(Cancellation) -> Result<Job>,
    ) -> std::result::Result<Request, AdmissionError> {
        let delivery = Arc::new(Delivery {
            mailbox: Mutex::new(Mailbox::default()),
            released: AtomicBool::new(false),
            cancellation: Cancellation::with_deadline(deadline),
        });
        let id = {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if registry.closed {
                return Err(AdmissionError::Closed);
            }
            // Joined output still occupies a slot until explicitly consumed or
            // discarded. No unbounded completed-bundle queue replaces workers.
            registry.entries.retain(|_, entry| {
                !(entry.joined && entry.delivery.released.load(Ordering::Acquire))
            });
            if registry.entries.len() == registry.capacity {
                return Err(AdmissionError::Busy);
            }
            let id = registry.next_id;
            registry.next_id = registry
                .next_id
                .checked_add(1)
                .expect("job identity exhausted");
            let lane = (0..registry.capacity)
                .find(|lane| !registry.entries.values().any(|entry| entry.lane == *lane))
                .expect("free lane");
            registry.entries.insert(
                id,
                Entry {
                    lane,
                    job: None,
                    joined: false,
                    delivery: delivery.clone(),
                },
            );
            id
        };
        // The reservation is visible to shutdown before setup; its signal is
        // also the worker's signal. Unwinding/setup failure rolls it back.
        let mut reservation = Reservation {
            registry: self.registry.clone(),
            id,
            armed: true,
        };
        let job = spawn(delivery.cancellation.clone())
            .map_err(|error| AdmissionError::Setup(format!("{error:#}")))?;
        self.registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entries
            .get_mut(&id)
            .expect("reserved job")
            .job = Some(job);
        reservation.armed = false;
        Ok(Request {
            id,
            delivery,
            armed: true,
        })
    }

    pub(super) fn status(&self) -> Vec<Status> {
        statuses(
            &self
                .registry
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        )
    }

    /// Closes admission and revokes unpublished output without waiting for work.
    pub(super) fn shutdown(&mut self) -> Result<Shutdown> {
        {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            registry.closed = true;
            for entry in registry.entries.values() {
                entry.delivery.cancellation.cancel();
            }
        }
        if self.failed.load(Ordering::Acquire) {
            return Err(anyhow!("native supervisor panicked; root drain required"));
        }
        Ok(if self.reapers.is_empty() {
            Shutdown::Drained
        } else {
            Shutdown::Pending(self.status())
        })
    }

    /// Explicitly unbounded; never call from a request's async executor.
    pub(super) fn drain(&mut self) -> Result<()> {
        let _ = self.shutdown();
        for reaper in self.reapers.drain(..) {
            if reaper.join().is_err() {
                self.failed.store(true, Ordering::Release);
            }
        }
        if self.failed.load(Ordering::Acquire) {
            // Failure never detaches workers or changes to a successful report.
            // This fallback is deliberately on the synchronous root drain.
            let entries = std::mem::take(
                &mut self
                    .registry
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .entries,
            );
            drop(entries);
            return Err(anyhow!(
                "native supervisor panicked; root collected remaining owners"
            ));
        }
        Ok(())
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.drain();
    }
}

struct Reservation {
    registry: Arc<Mutex<Registry>>,
    id: Id,
    armed: bool,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if self.armed {
            self.registry
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .entries
                .remove(&self.id);
        }
    }
}

fn statuses(registry: &Registry) -> Vec<Status> {
    registry
        .entries
        .iter()
        .map(|(&id, entry)| {
            entry.delivery.cancellation.expire();
            Status {
                id,
                joined: entry.joined,
                cancellation_requested: entry.delivery.cancellation.is_cancelled(),
                deadline_exceeded: entry.delivery.cancellation.deadline_exceeded(),
            }
        })
        .collect()
}

fn guarded(registry: &Mutex<Registry>, failed: &AtomicBool, work: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
        failed.store(true, Ordering::Release);
        let mut registry = registry.lock().unwrap_or_else(|error| error.into_inner());
        registry.closed = true;
        for entry in registry.entries.values() {
            entry.delivery.cancellation.cancel();
        }
    }
}

fn supervise(registry: &Mutex<Registry>, failed: &AtomicBool) {
    loop {
        {
            let mut registry = registry.lock().unwrap_or_else(|error| error.into_inner());
            let closed = registry.closed;
            registry.entries.retain(|_, entry| {
                !(entry.joined && (closed || entry.delivery.released.load(Ordering::Acquire)))
            });
            if failed.load(Ordering::Acquire) || (closed && registry.entries.is_empty()) {
                return;
            }
            for entry in registry.entries.values() {
                entry.delivery.cancellation.expire();
            }
        }
        thread::park_timeout(Duration::from_millis(1));
    }
}

fn collect_lane(registry: &Mutex<Registry>, lane: usize, failed: &AtomicBool) {
    loop {
        let work = {
            let mut registry = registry.lock().unwrap_or_else(|error| error.into_inner());
            let closed = registry.closed;
            match registry
                .entries
                .iter_mut()
                .find(|(_, entry)| entry.lane == lane)
            {
                Some((_, entry))
                    if closed
                        && (entry.joined
                            || (failed.load(Ordering::Acquire) && entry.job.is_none())) =>
                {
                    return
                }
                Some((&id, entry)) => entry
                    .job
                    .take()
                    .map(|job| (id, job, entry.delivery.clone())),
                None if closed => return,
                None => None,
            }
        };
        if let Some((id, job, delivery)) = work {
            // Fixed join lanes isolate even thread-local destructors: Rust's
            // is_finished can be true while join still waits on those destructors.
            let completed = job.join();
            registry
                .lock()
                .unwrap()
                .entries
                .get_mut(&id)
                .unwrap()
                .joined = true;
            let wake = {
                let mut mailbox = delivery.mailbox.lock().unwrap();
                if !delivery.released.load(Ordering::Acquire) {
                    mailbox.result = Some(completed);
                }
                mailbox.waker.take()
            };
            if let Some(waker) = wake {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| waker.wake()));
            }
        }
        thread::park_timeout(Duration::from_millis(1));
    }
}

/// Dropping a request/future only revokes and discards delivery. It never owns a
/// Job, runtime, reaper handle, or root owner, including before its first poll.
#[must_use]
pub(super) struct Request {
    pub id: Id,
    delivery: Arc<Delivery>,
    armed: bool,
}
impl Request {
    pub(super) fn cancel(&self) -> CancelOutcome {
        self.delivery.cancellation.cancel()
    }
    pub(super) fn try_take(&mut self) -> Option<Joined> {
        let result = self.delivery.mailbox.lock().unwrap().result.take()?;
        self.armed = false;
        Some(Joined {
            completed: Some(result),
            delivery: self.delivery.clone(),
        })
    }
}
impl Future for Request {
    type Output = Joined;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Joined> {
        let result = {
            let mut mailbox = self.delivery.mailbox.lock().unwrap();
            if let Some(result) = mailbox.result.take() {
                Some(result)
            } else {
                mailbox.waker = Some(cx.waker().clone());
                None
            }
        };
        match result {
            Some(result) => {
                self.armed = false;
                Poll::Ready(Joined {
                    completed: Some(result),
                    delivery: self.delivery.clone(),
                })
            }
            None => Poll::Pending,
        }
    }
}
fn abandon(delivery: &Delivery) {
    delivery.cancellation.cancel();
    delivery.released.store(true, Ordering::Release);
    // Serialize with delivery, so a late successful send cannot retain bytes.
    let mut mailbox = delivery.mailbox.lock().unwrap();
    mailbox.result.take();
    mailbox.waker.take();
}
impl Drop for Request {
    fn drop(&mut self) {
        if self.armed {
            abandon(&self.delivery);
        }
    }
}

#[must_use]
pub(super) struct Joined {
    completed: Option<Completed>,
    delivery: Arc<Delivery>,
}
impl Joined {
    pub(super) fn publish(mut self, path: &std::path::Path) -> Result<()> {
        self.completed.take().expect("joined output").publish(path)
    }
    #[cfg(test)]
    fn commit_with(mut self, write: impl FnOnce(&str) -> Result<()>) -> Result<()> {
        self.completed
            .take()
            .expect("joined output")
            .commit_with(write)
    }
}
impl Drop for Joined {
    fn drop(&mut self) {
        abandon(&self.delivery);
    }
}

#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod tests;
