//! Linux dev renderer observations. Producers only enqueue owned numeric
//! records. A diagnostic stderr sink can neither delay a producer nor take
//! part in closing a host channel, disposing a host, or joining a thread.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use zfb_build::provenance_trace::{self as prov, NonblockingTraceSink, TraceWrite};
use zfb_render::HeapObservation;

const CAPACITY: usize = 128;
const ATTEMPT_LIMIT: u64 = 4096;
const ALLOCATOR_ENV_VAR: &str = "ZFB_DEV_RENDERER_MEMORY_ALLOCATOR";
const ALLOCATOR_CALL_LIMIT: u64 = 128;
const ALLOCATOR_METRIC_DOMAIN: &str = "gnu_mallinfo2.v1";
const DRAIN_POLL: Duration = Duration::from_millis(25);
const TERMINAL_BUDGET: Duration = Duration::from_millis(250);

static TRACE: OnceLock<Arc<Buffer>> = OnceLock::new();

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    RefreshStart,
    SnapshotStart,
    SnapshotReady,
    SnapshotFailed,
    BundleStart,
    BundleReady,
    RefreshSkipped,
    RefreshFailed,
    RefreshComplete,
    HostCreate,
    HostInitialized,
    HostBoot,
    HostBootFailed,
    ValidationOk,
    ValidationRejected,
    Adopted,
    RetireRequested,
    HostFinalHeap,
    HostDestroyed,
    JoinCompleted,
    JoinPanicked,
    JoinTimeout,
    JoinDisconnected,
    #[cfg(target_env = "gnu")]
    AllocatorStart,
    AllocatorComplete,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AllocatorPhase {
    Entry,
    SnapshotBefore,
    SnapshotAfter,
    BundleBefore,
    BundleAfter,
    RetirementAfter,
    RefreshReturn,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BoundaryOutcome {
    Ok,
    Rejected,
    Failed,
    Returned,
    Refreshed,
    Skipped,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum AllocatorOutcome {
    #[cfg(target_env = "gnu")]
    Ok,
    Unavailable,
    Limit,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum UnavailableReason {
    ApplicationWriteExclusion,
    #[cfg(not(target_env = "gnu"))]
    UnsupportedTarget,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum AllocatorContext {
    Unlocked,
    PublicationLocked,
}

impl AllocatorContext {
    fn unavailable(self) -> Option<UnavailableReason> {
        match self {
            Self::PublicationLocked => Some(UnavailableReason::ApplicationWriteExclusion),
            #[cfg(target_env = "gnu")]
            Self::Unlocked => None,
            #[cfg(not(target_env = "gnu"))]
            Self::Unlocked => Some(UnavailableReason::UnsupportedTarget),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
struct AllocatorCounters {
    arena_bytes: usize,
    uordblks_bytes: usize,
    fordblks_bytes: usize,
    hblkhd_bytes: usize,
    hblks: usize,
    fsmblks_bytes: usize,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct AllocatorCompletion {
    allocator_started_us: u64,
    allocator_completed_us: u64,
    allocator_duration_us: u64,
    allocator_outcome: AllocatorOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    allocator_unavailable_reason: Option<UnavailableReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allocator: Option<AllocatorCounters>,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct AllocatorRecord {
    metric_domain: &'static str,
    allocator_phase: AllocatorPhase,
    allocator_call: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    allocator_boundary_outcome: Option<BoundaryOutcome>,
    #[serde(flatten)]
    completion: Option<AllocatorCompletion>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BootFailure {
    Spawn,
    Reported,
    Channel,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct Context {
    refresh_id: u64,
    #[serde(skip_serializing_if = "is_zero")]
    snapshot_gen: u64,
    #[serde(skip_serializing_if = "is_zero")]
    host_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    tick: Option<u64>,
}

#[derive(Clone, Copy, Default, Debug, Serialize)]
struct Details {
    #[serde(skip_serializing_if = "Option::is_none")]
    boot: Option<bool>,
    #[serde(skip_serializing_if = "is_zero")]
    worker_gen: u64,
    #[serde(skip_serializing_if = "is_zero")]
    retired_by_refresh_id: u64,
    #[serde(skip_serializing_if = "is_zero")]
    tid: u64,
    #[serde(skip_serializing_if = "is_zero")]
    tid_start_ticks: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    heap: Option<HeapObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<BootFailure>,
    #[serde(flatten)]
    allocator_record: Option<AllocatorRecord>,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct Record {
    occurrence: u64,
    occurred_us: u64,
    phase: Phase,
    #[serde(flatten)]
    context: Context,
    #[serde(flatten)]
    details: Details,
}

struct Buffer {
    queue: Mutex<VecDeque<Record>>,
    attempted: AtomicU64,
    contention: AtomicU64,
    overflow: AtomicU64,
    refreshes: AtomicU64,
    hosts: AtomicU64,
    allocator_enabled: bool,
    allocator_calls: AtomicU64,
    closed: AtomicBool,
}

impl Buffer {
    fn new(allocator_enabled: bool) -> Self {
        Self {
            queue: Mutex::new(VecDeque::with_capacity(CAPACITY)),
            attempted: AtomicU64::new(0),
            contention: AtomicU64::new(0),
            overflow: AtomicU64::new(0),
            refreshes: AtomicU64::new(0),
            hosts: AtomicU64::new(0),
            allocator_enabled,
            allocator_calls: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        }
    }

    fn record(&self, context: Context, phase: Phase, details: Details) {
        self.record_at(context, phase, details, prov::elapsed_us());
    }

    fn record_at(&self, context: Context, phase: Phase, details: Details, occurred_us: u64) {
        if self.closed.load(Relaxed) {
            return;
        }
        let occurrence = self.attempted.fetch_add(1, Relaxed) + 1;
        if occurrence > ATTEMPT_LIMIT {
            self.overflow.fetch_add(1, Relaxed);
            return;
        }
        let Ok(mut queue) = self.queue.try_lock() else {
            self.contention.fetch_add(1, Relaxed);
            return;
        };
        if queue.len() == CAPACITY {
            self.overflow.fetch_add(1, Relaxed);
            return;
        }
        queue.push_back(Record {
            occurrence,
            occurred_us,
            phase,
            context,
            details,
        });
    }

    fn transfer(&self, pending: &mut VecDeque<Record>) {
        debug_assert!(pending.is_empty());
        if let Ok(mut queue) = self.queue.try_lock() {
            std::mem::swap(&mut *queue, pending);
        }
    }
}

struct Drain {
    pipe: NonblockingTraceSink,
    startup: Option<Value>,
    pending: VecDeque<Record>,
    emitted: u64,
    output_dropped: u64,
    reported_loss: bool,
}

impl Drain {
    fn watermark(&self, buffer: &Buffer) -> Value {
        json!({
            "attempted": buffer.attempted.load(Relaxed),
            "emitted": self.emitted,
            "contention": buffer.contention.load(Relaxed),
            "overflow": buffer.overflow.load(Relaxed),
            "output_dropped": self.output_dropped,
        })
    }

    fn pump(&mut self, buffer: &Buffer) {
        if let Some(startup) = &self.startup {
            if !matches!(
                self.pipe
                    .try_emit("renderer_lifecycle_start", startup.clone()),
                TraceWrite::Written
            ) {
                return;
            }
            self.startup = None;
        }
        if buffer.contention.load(Relaxed) != 0
            || buffer.overflow.load(Relaxed) != 0
            || self.output_dropped != 0
        {
            if !self.reported_loss {
                let loss = self.watermark(buffer);
                self.reported_loss = matches!(
                    self.pipe.try_emit("renderer_lifecycle_loss", loss),
                    TraceWrite::Written
                );
            }
        }
        if self.pending.is_empty() {
            buffer.transfer(&mut self.pending);
        }
        while let Some(record) = self.pending.front() {
            let fields = json!(record);
            match self.pipe.try_emit("renderer_lifecycle", fields) {
                TraceWrite::Written => self.emitted += 1,
                TraceWrite::Retry => break,
                TraceWrite::Dropped => self.output_dropped += 1,
            }
            self.pending.pop_front();
        }
    }
}

pub(crate) struct Capture {
    buffer: Arc<Buffer>,
    drain: Arc<Mutex<Drain>>,
}

impl Capture {
    pub(crate) fn start() -> Option<Self> {
        if !prov::enabled() {
            return None;
        }
        let pipe = NonblockingTraceSink::open().ok()?;
        let allocator_enabled =
            std::env::var_os(ALLOCATOR_ENV_VAR).is_some_and(|value| value == "1");
        let startup = clock_anchor(allocator_enabled)?;
        let buffer = Arc::new(Buffer::new(allocator_enabled));
        let drain = Arc::new(Mutex::new(Drain {
            pipe,
            startup: Some(startup),
            pending: VecDeque::with_capacity(CAPACITY),
            emitted: 0,
            output_dropped: 0,
            reported_loss: false,
        }));
        TRACE.set(buffer.clone()).ok()?;
        let (writer_buffer, writer_drain) = (buffer.clone(), drain.clone());
        // The process owns the detached drain. It may be delayed indefinitely;
        // neither host lifetime nor the bounded join depends on its progress.
        let _ = thread::Builder::new()
            .name("zfb-renderer-memory".into())
            .spawn(move || {
                while !writer_buffer.closed.load(Relaxed) {
                    if let Ok(mut drain) = writer_drain.try_lock() {
                        drain.pump(&writer_buffer);
                    }
                    thread::sleep(DRAIN_POLL);
                }
            });
        Some(Self { buffer, drain })
    }

    /// The outer dev command calls this after its host teardown. Give the
    /// bounded batches a short chance to clear transient output pressure. A
    /// missing/mismatched watermark remains incomplete evidence. This never
    /// waits for the drain worker and is outside host drop and bounded join.
    pub(crate) fn finish(self) {
        self.buffer.closed.store(true, Relaxed);
        let deadline = Instant::now() + TERMINAL_BUDGET;
        loop {
            if let Ok(mut drain) = self.drain.try_lock() {
                drain.pump(&self.buffer);
                let queue_empty = self
                    .buffer
                    .queue
                    .try_lock()
                    .is_ok_and(|queue| queue.is_empty());
                if drain.startup.is_none() && drain.pending.is_empty() && queue_empty {
                    let watermark = drain.watermark(&self.buffer);
                    match drain
                        .pipe
                        .try_emit("renderer_lifecycle_terminal", watermark)
                    {
                        TraceWrite::Written | TraceWrite::Dropped => return,
                        TraceWrite::Retry => {}
                    }
                }
            }
            if Instant::now() >= deadline {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn clock_anchor(allocator_enabled: bool) -> Option<Value> {
    let anchor_before_us = prov::elapsed_us();
    let mut monotonic = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: monotonic points to initialized, writable storage for one timespec.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut monotonic) } != 0 {
        return None;
    }
    let anchor_after_us = prov::elapsed_us();
    let clock_monotonic_us = u64::try_from(monotonic.tv_sec)
        .ok()?
        .checked_mul(1_000_000)?
        .checked_add(u64::try_from(monotonic.tv_nsec).ok()? / 1000)?;
    let mut record = json!({
        "schema_version": 1,
        "buffer_capacity": CAPACITY,
        "drain_capacity": CAPACITY,
        "attempt_limit": ATTEMPT_LIMIT,
        "line_limit_bytes": NonblockingTraceSink::LINE_LIMIT_BYTES,
        "terminal_budget_ms": TERMINAL_BUDGET.as_millis(),
        "pid": std::process::id(),
        "clock_monotonic_us": clock_monotonic_us,
        "anchor_before_us": anchor_before_us,
        "anchor_after_us": anchor_after_us,
    });
    if allocator_enabled {
        record["allocator_metric_domain"] = json!(ALLOCATOR_METRIC_DOMAIN);
        record["allocator_call_limit"] = json!(ALLOCATOR_CALL_LIMIT);
    }
    Some(record)
}

#[derive(Clone)]
struct Observation {
    buffer: Arc<Buffer>,
    context: Context,
    allocator_context: AllocatorContext,
}

impl Observation {
    fn record(&self, phase: Phase, details: Details) {
        self.buffer.record(self.context, phase, details);
    }
}

thread_local! {
    static REFRESH: RefCell<Option<Observation>> = const { RefCell::new(None) };
    static RETIRED_BY: Cell<u64> = const { Cell::new(0) };
}

pub(crate) struct Refresh {
    previous: Option<Observation>,
    complete: bool,
}

impl Refresh {
    pub(crate) fn begin(boot: bool, allocator_context: AllocatorContext) -> Option<Self> {
        Some(Self::with_buffer(
            TRACE.get()?.clone(),
            boot,
            allocator_context,
        ))
    }

    fn with_buffer(buffer: Arc<Buffer>, boot: bool, allocator_context: AllocatorContext) -> Self {
        let refresh_id = buffer.refreshes.fetch_add(1, Relaxed) + 1;
        let observation = Observation {
            buffer,
            allocator_context,
            context: Context {
                refresh_id,
                tick: prov::renderer_tick(),
                ..Context::default()
            },
        };
        observation.record(
            Phase::RefreshStart,
            Details {
                boot: Some(boot),
                ..Details::default()
            },
        );
        let previous = REFRESH.with(|slot| slot.replace(Some(observation)));
        Self {
            previous,
            complete: false,
        }
    }

    pub(crate) fn finish(&mut self, phase: Phase) {
        record_refresh(phase);
        self.complete = true;
    }

    pub(crate) fn returned(&mut self, outcome: BoundaryOutcome) {
        if !self.complete {
            self.finish(Phase::RefreshFailed);
        }
        record_allocator(AllocatorPhase::RefreshReturn, Some(outcome));
    }
}

impl Drop for Refresh {
    fn drop(&mut self) {
        if !self.complete {
            record_refresh(Phase::RefreshFailed);
        }
        REFRESH.with(|slot| slot.replace(self.previous.take()));
    }
}

pub(crate) fn record_refresh(phase: Phase) {
    REFRESH.with_borrow(|observation| {
        if let Some(observation) = observation {
            observation.record(phase, Details::default());
        }
    });
}

pub(crate) fn allocator_can_run() -> bool {
    REFRESH.with_borrow(|observation| {
        observation.as_ref().is_some_and(|obs| {
            obs.buffer.allocator_enabled && obs.allocator_context.unavailable().is_none()
        })
    })
}

/// Synchronous diagnostic call on the refresh thread, outside shared
/// application locks and host shutdown. A start record is queued before
/// entering the native call so a blocked getter leaves explicit evidence.
pub(crate) fn record_allocator(phase: AllocatorPhase, boundary_outcome: Option<BoundaryOutcome>) {
    let Some(observation) = REFRESH.with_borrow(|slot| {
        slot.as_ref()
            .filter(|obs| obs.buffer.allocator_enabled && !obs.buffer.closed.load(Relaxed))
            .cloned()
    }) else {
        return;
    };
    let buffer = &observation.buffer;
    let limit_record = ALLOCATOR_CALL_LIMIT + 1;
    let Ok(previous) = buffer
        .allocator_calls
        .fetch_update(Relaxed, Relaxed, |count| {
            (count < limit_record).then_some(count + 1)
        })
    else {
        return;
    };
    let call = previous + 1;
    let mut record = AllocatorRecord {
        metric_domain: ALLOCATOR_METRIC_DOMAIN,
        allocator_phase: phase,
        allocator_call: call,
        allocator_boundary_outcome: boundary_outcome,
        completion: None,
    };
    let emit = |phase, allocator_record, occurred_us| {
        buffer.record_at(
            observation.context,
            phase,
            Details {
                allocator_record: Some(allocator_record),
                ..Details::default()
            },
            occurred_us,
        );
    };
    let requested_us = prov::elapsed_us();
    let completion = if call == limit_record {
        AllocatorCompletion::skipped(requested_us, AllocatorOutcome::Limit, None)
    } else if let Some(reason) = observation.allocator_context.unavailable() {
        AllocatorCompletion::skipped(requested_us, AllocatorOutcome::Unavailable, Some(reason))
    } else {
        #[cfg(target_env = "gnu")]
        {
            emit(Phase::AllocatorStart, record, requested_us);
            let started_us = prov::elapsed_us();
            // SAFETY: the pinned libc binding uses the GNU ABI and takes no
            // arguments. The refresh caller holds no shared application
            // locks and is outside both owner destruction and bounded join.
            let counters = unsafe { libc::mallinfo2() };
            let completed_us = prov::elapsed_us();
            AllocatorCompletion {
                allocator_started_us: started_us,
                allocator_completed_us: completed_us,
                allocator_duration_us: completed_us - started_us,
                allocator_outcome: AllocatorOutcome::Ok,
                allocator_unavailable_reason: None,
                allocator: Some(AllocatorCounters {
                    arena_bytes: counters.arena,
                    uordblks_bytes: counters.uordblks,
                    fordblks_bytes: counters.fordblks,
                    hblkhd_bytes: counters.hblkhd,
                    hblks: counters.hblks,
                    fsmblks_bytes: counters.fsmblks,
                }),
            }
        }
        #[cfg(not(target_env = "gnu"))]
        unreachable!("unsupported targets were handled above")
    };
    record.completion = Some(completion);
    emit(
        Phase::AllocatorComplete,
        record,
        completion.allocator_completed_us,
    );
}

impl AllocatorCompletion {
    fn skipped(at_us: u64, outcome: AllocatorOutcome, reason: Option<UnavailableReason>) -> Self {
        Self {
            allocator_started_us: at_us,
            allocator_completed_us: at_us,
            allocator_duration_us: 0,
            allocator_outcome: outcome,
            allocator_unavailable_reason: reason,
            allocator: None,
        }
    }
}

pub(crate) fn record_snapshot(snapshot_gen: u64, failed: bool) {
    REFRESH.with_borrow_mut(|observation| {
        if let Some(observation) = observation {
            observation.context.snapshot_gen = snapshot_gen;
            let phase = if failed {
                Phase::SnapshotFailed
            } else {
                Phase::SnapshotReady
            };
            observation.record(phase, Details::default());
        }
    });
}

pub(crate) fn adopted(worker_gen: Option<u64>) {
    REFRESH.with_borrow(|observation| {
        if let Some(observation) = observation {
            observation.record(
                Phase::Adopted,
                Details {
                    worker_gen: worker_gen.unwrap_or(0),
                    ..Details::default()
                },
            );
        }
    });
}

pub(crate) struct Replacing(u64);

impl Replacing {
    pub(crate) fn current_refresh() -> Self {
        let retiring =
            REFRESH.with_borrow(|obs| obs.as_ref().map_or(0, |obs| obs.context.refresh_id));
        Self(RETIRED_BY.replace(retiring))
    }
}

impl Drop for Replacing {
    fn drop(&mut self) {
        RETIRED_BY.set(self.0);
    }
}

#[derive(Clone)]
pub(crate) struct Host(Observation);

impl Host {
    pub(crate) fn create() -> Option<Self> {
        REFRESH.with_borrow_mut(|observation| {
            let observation = observation.as_mut()?;
            observation.context.host_id = observation.buffer.hosts.fetch_add(1, Relaxed) + 1;
            observation.record(Phase::HostCreate, Details::default());
            Some(Self(observation.clone()))
        })
    }

    pub(crate) fn on_owner(&self) -> Owner<'_> {
        let (tid, tid_start_ticks) = thread_birth().unwrap_or_default();
        Owner {
            host: self,
            tid,
            tid_start_ticks,
        }
    }

    pub(crate) fn boot_failed(&self, failure: BootFailure) {
        self.0.record(
            Phase::HostBootFailed,
            Details {
                failure: Some(failure),
                ..Details::default()
            },
        );
    }

    pub(crate) fn retirement(&self, phase: Phase) {
        self.0.record(
            phase,
            Details {
                retired_by_refresh_id: RETIRED_BY.get(),
                ..Details::default()
            },
        );
    }
}

pub(crate) struct Owner<'a> {
    host: &'a Host,
    tid: u64,
    tid_start_ticks: u64,
}

impl Owner<'_> {
    pub(crate) fn boot_failed(&self) {
        self.host.0.record(
            Phase::HostBootFailed,
            Details {
                tid: self.tid,
                tid_start_ticks: self.tid_start_ticks,
                failure: Some(BootFailure::Reported),
                ..Details::default()
            },
        );
    }

    pub(crate) fn record(&self, phase: Phase, heap: Option<HeapObservation>) {
        self.host.0.record(
            phase,
            Details {
                tid: self.tid,
                tid_start_ticks: self.tid_start_ticks,
                heap,
                ..Details::default()
            },
        );
    }
}

fn thread_birth() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/thread-self/stat").ok()?;
    let (tid, _) = stat.split_once(' ')?;
    // comm is parenthesized and may contain spaces or parentheses. Fields
    // after its last ')' start at field 3, so field 22 is index 19.
    let (_, other) = stat.rsplit_once(')')?;
    let start_ticks = other.split_whitespace().nth(19)?;
    Some((tid.parse().ok()?, start_ticks.parse().ok()?))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn current_tid() -> u64 {
        thread_birth().unwrap().0
    }

    pub(crate) struct HostEvent {
        pub phase: Phase,
        pub tid: u64,
        pub tid_start_ticks: u64,
        pub heap: Option<HeapObservation>,
    }

    pub(crate) fn capture_host_events(f: impl FnOnce()) -> Vec<HostEvent> {
        let buffer = Arc::new(Buffer::new(false));
        {
            let mut refresh =
                Refresh::with_buffer(buffer.clone(), true, AllocatorContext::Unlocked);
            f();
            refresh.finish(Phase::RefreshComplete);
        }
        // The host and join had to complete before any sink read.
        let queue = buffer.queue.lock().unwrap();
        queue
            .iter()
            .map(|r| HostEvent {
                phase: r.phase,
                tid: r.details.tid,
                tid_start_ticks: r.details.tid_start_ticks,
                heap: r.details.heap,
            })
            .collect()
    }

    #[test]
    fn allocator_opt_out_preserves_old_record_shape() {
        let buffer = Arc::new(Buffer::new(false));
        let mut refresh = Refresh::with_buffer(buffer.clone(), false, AllocatorContext::Unlocked);
        record_allocator(AllocatorPhase::Entry, None);
        refresh.finish(Phase::RefreshComplete);
        refresh.returned(BoundaryOutcome::Refreshed);
        let queue = buffer.queue.lock().unwrap();
        assert_eq!(queue.len(), 2);
        assert_eq!(buffer.allocator_calls.load(Relaxed), 0);
        assert!(json!(queue[0]).get("metric_domain").is_none());
        assert!(clock_anchor(false)
            .unwrap()
            .get("allocator_call_limit")
            .is_none());
    }

    #[cfg(target_env = "gnu")]
    #[test]
    fn allocator_pairs_safely_record_real_counter_outcome_and_cap() {
        let buffer = Arc::new(Buffer::new(true));
        let mut refresh = Refresh::with_buffer(buffer.clone(), false, AllocatorContext::Unlocked);
        record_allocator(AllocatorPhase::Entry, None);
        record_snapshot(9, false);
        refresh.finish(Phase::RefreshComplete);
        refresh.returned(BoundaryOutcome::Refreshed);
        buffer.allocator_calls.store(ALLOCATOR_CALL_LIMIT, Relaxed);
        record_allocator(AllocatorPhase::SnapshotBefore, None);
        record_allocator(AllocatorPhase::SnapshotAfter, Some(BoundaryOutcome::Ok));

        let queue = buffer.queue.lock().unwrap();
        let events: Vec<_> = queue.iter().map(|record| json!(record)).collect();
        assert_eq!(events.len(), 8);
        assert_eq!(events[1]["phase"], "allocator_start");
        assert_eq!(events[2]["phase"], "allocator_complete");
        assert_eq!(events[2]["metric_domain"], ALLOCATOR_METRIC_DOMAIN);
        assert_eq!(events[2]["allocator_call"], 1);
        assert_eq!(events[2]["allocator_outcome"], "ok");
        let counters = events[2]["allocator"].as_object().unwrap();
        assert_eq!(counters.len(), 6);
        assert!(counters.values().all(|field| field.as_u64().is_some()));
        let requested = events[1]["occurred_us"].as_u64().unwrap();
        let started = events[2]["allocator_started_us"].as_u64().unwrap();
        let completed = events[2]["allocator_completed_us"].as_u64().unwrap();
        assert!(requested <= started && started <= completed);
        assert_eq!(events[2]["allocator_duration_us"], completed - started);
        assert_eq!(events[2]["occurred_us"], completed);

        assert_eq!(events[4]["phase"], "refresh_complete");
        assert_eq!(events[5]["allocator_phase"], "refresh_return");
        assert_eq!(events[5]["allocator_boundary_outcome"], "refreshed");
        assert_eq!(events[6]["refresh_id"], events[0]["refresh_id"]);
        assert_eq!(events[6]["snapshot_gen"], 9);

        assert_eq!(events[7]["phase"], "allocator_complete");
        assert_eq!(events[7]["allocator_outcome"], "limit");
        assert_eq!(events[7]["allocator_call"], ALLOCATOR_CALL_LIMIT + 1);
        assert_eq!(events[7]["allocator_duration_us"], 0);
        assert!(events[7].get("allocator").is_none());
        assert_eq!(
            clock_anchor(true).unwrap()["allocator_call_limit"],
            ALLOCATOR_CALL_LIMIT
        );
    }

    #[test]
    fn allocator_reports_publication_locked_checkpoint_without_start() {
        let buffer = Arc::new(Buffer::new(true));
        let _refresh =
            Refresh::with_buffer(buffer.clone(), false, AllocatorContext::PublicationLocked);
        assert!(!allocator_can_run());
        record_allocator(AllocatorPhase::Entry, None);
        let queue = buffer.queue.lock().unwrap();
        assert_eq!(queue.len(), 2);
        let event = json!(queue[1]);
        assert_eq!(event["phase"], "allocator_complete");
        assert_eq!(event["allocator_outcome"], "unavailable");
        assert_eq!(
            event["allocator_unavailable_reason"],
            "application_write_exclusion"
        );
        assert_eq!(event["allocator_duration_us"], 0);
        assert!(event.get("allocator").is_none());
    }

    #[test]
    fn producer_never_waits_for_contended_or_full_buffers() {
        let buffer = Buffer::new(false);
        let guard = buffer.queue.lock().unwrap();
        buffer.record(Context::default(), Phase::HostCreate, Details::default());
        assert_eq!(buffer.contention.load(Relaxed), 1);
        drop(guard);
        for _ in 0..ATTEMPT_LIMIT + 1 {
            buffer.record(Context::default(), Phase::HostCreate, Details::default());
        }
        assert_eq!(buffer.attempted.load(Relaxed), ATTEMPT_LIMIT + 2);
        assert_eq!(buffer.queue.lock().unwrap().len(), CAPACITY);
        assert_eq!(
            buffer.overflow.load(Relaxed),
            ATTEMPT_LIMIT + 1 - CAPACITY as u64
        );
    }
}
