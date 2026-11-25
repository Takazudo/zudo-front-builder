//! Dev-only structured provenance trace (`ZFB_DEV_PROVENANCE_TRACE=1`).
//!
//! # Line format (external contract — parsed by `scripts/dev-browser-observe`)
//!
//! Every event is ONE stderr line:
//!
//! ```text
//! [zfb-prov] {"ev":"<name>","seq":<u64>,"ms":<u64>,"tick":<u64>?,...event fields}
//! ```
//!
//! - `ev` — event name: `intake`, `select`, `render_plan`, `snapshot`,
//!   `worker`, `refresh`, `observations`, `edges`, `stale`, `publish`,
//!   `reject`.
//! - `seq` — per-process, strictly increasing, assigned under the same lock
//!   that writes the line, so line order equals `seq` order.
//! - `ms` — milliseconds since the first trace call of the process
//!   (monotonic clock), sampled under the `seq` lock, so it never
//!   decreases in `seq` order.
//! - `tick` — present only when the event was emitted from inside a watcher
//!   tick (thread-scoped, see [`TickScope`]) or carries it explicitly
//!   (`intake`). It is the id `intake` allocated for that tick.
//! - Generation ids: `snapshot_gen` counts content snapshot builds and
//!   `worker_gen` counts renderer host adoptions; `worker` links them.
//!   `drain` counts content-trace drains. All start at 1; 0 never occurs.
//! - Lists are capped at [`LIST_CAP`] items; a capped list `x` is
//!   accompanied by `x_truncated: <omitted count>`.
//!
//! Field schemas per event are documented at their emit sites and in the
//! issue #4753 harness notes. When the env var is unset every entry point
//! here is a single cached-bool check; no ids are allocated and nothing is
//! written.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde_json::{Map, Value};

/// Env var that turns the trace on (`1` / `true`, case-insensitive).
pub const ENV_VAR: &str = "ZFB_DEV_PROVENANCE_TRACE";

/// Line prefix of every trace line.
pub const LINE_PREFIX: &str = "[zfb-prov] ";

/// Maximum items serialized per list field.
pub const LIST_CAP: usize = 200;

static ENABLED: OnceLock<bool> = OnceLock::new();
static EPOCH: OnceLock<Instant> = OnceLock::new();
static EMIT_LOCK: Mutex<u64> = Mutex::new(0);
static TICK_IDS: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT_GENS: AtomicU64 = AtomicU64::new(0);
static WORKER_GENS: AtomicU64 = AtomicU64::new(0);
static DRAIN_IDS: AtomicU64 = AtomicU64::new(0);
static CURRENT_GENERATION: Mutex<Option<Generation>> = Mutex::new(None);
static LAST_DRAINED_WORKER: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static CURRENT_TICK: Cell<Option<u64>> = const { Cell::new(None) };
    static LAST_DRAIN: Cell<Option<u64>> = const { Cell::new(None) };
    static LAST_DRAIN_CONSUMERS: RefCell<Option<Vec<std::path::PathBuf>>> =
        const { RefCell::new(None) };
}

fn parse_enabled(raw: Option<&str>) -> bool {
    raw.map(|raw| {
        let t = raw.trim();
        t.eq_ignore_ascii_case("1") || t.eq_ignore_ascii_case("true")
    })
    .unwrap_or(false)
}

/// Whether the trace is on. Reads [`ENV_VAR`] once per process.
pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        let on = parse_enabled(std::env::var(ENV_VAR).ok().as_deref());
        if on {
            EPOCH.get_or_init(Instant::now);
        }
        on
    })
}

/// The live renderer generation: the host adoption id and the content
/// snapshot generation its bundle embedded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation {
    pub worker_gen: u64,
    pub snapshot_gen: u64,
}

impl Generation {
    pub fn to_json(self) -> Value {
        serde_json::json!({ "worker_gen": self.worker_gen, "snapshot_gen": self.snapshot_gen })
    }
}

/// Allocate a tick id (`None` when the trace is off).
pub fn next_tick_id() -> Option<u64> {
    enabled().then(|| TICK_IDS.fetch_add(1, Ordering::Relaxed) + 1)
}

/// Allocate a content snapshot generation (`None` when off).
pub fn next_snapshot_gen() -> Option<u64> {
    enabled().then(|| SNAPSHOT_GENS.fetch_add(1, Ordering::Relaxed) + 1)
}

/// Allocate a content-trace drain id and remember it as this thread's
/// latest drain (`None` when off).
pub fn next_drain_id() -> Option<u64> {
    if !enabled() {
        return None;
    }
    let id = DRAIN_IDS.fetch_add(1, Ordering::Relaxed) + 1;
    LAST_DRAIN.with(|cell| cell.set(Some(id)));
    Some(id)
}

/// The latest drain id allocated on this thread.
pub fn last_drain_on_thread() -> Option<u64> {
    LAST_DRAIN.with(Cell::get)
}

/// Forget this thread's latest drain id, so a drain attempt that returns
/// before allocating one is not attributed an older drain's id.
pub fn clear_last_drain_on_thread() {
    if enabled() {
        LAST_DRAIN.with(|cell| cell.set(None));
        LAST_DRAIN_CONSUMERS.with(|cell| cell.borrow_mut().take());
    }
}

/// Remember the consumer page ids (source paths) whose observation slots
/// this thread's latest drain reported.
pub fn note_drain_consumers(consumers: impl IntoIterator<Item = std::path::PathBuf>) {
    if enabled() {
        let consumers: Vec<_> = consumers.into_iter().collect();
        LAST_DRAIN_CONSUMERS.with(|cell| *cell.borrow_mut() = Some(consumers));
    }
}

/// Whether this thread's latest drain reported a slot for `consumer`
/// (`None` when no drain reported observations on this thread).
pub fn last_drain_contains(consumer: &std::path::Path) -> Option<bool> {
    LAST_DRAIN_CONSUMERS.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|consumers| consumers.iter().any(|c| c == consumer))
    })
}

/// Record that worker generation `worker_gen` answered a drain request.
/// Returns `true` for the first drain answered by that generation (the
/// worker's trace buffer then holds everything it recorded since adoption).
pub fn note_drain_of_generation(worker_gen: u64) -> bool {
    LAST_DRAINED_WORKER.swap(worker_gen, Ordering::Relaxed) != worker_gen
}

/// The currently adopted generation, if any (always `None` when off).
pub fn current_generation() -> Option<Generation> {
    if !enabled() {
        return None;
    }
    *CURRENT_GENERATION.lock().unwrap_or_else(|p| p.into_inner())
}

/// An event built while a lock was held, written once it is released.
#[must_use = "call `emit` after releasing the lock"]
pub struct PendingEvent {
    ev: &'static str,
    fields: Value,
}

impl PendingEvent {
    pub fn new(ev: &'static str, fields: Value) -> Self {
        Self { ev, fields }
    }

    /// The generation in this owned adoption event, without reading shared
    /// generation state again or acquiring any lock at the diagnostic seam.
    #[cfg(all(feature = "renderer-memory-observe", target_os = "linux"))]
    pub fn worker_generation(&self) -> Option<u64> {
        self.fields["worker_gen"].as_u64()
    }

    pub fn emit(self) {
        emit(self.ev, self.fields);
    }
}

/// Record a renderer host adoption built from `snapshot_gen` and return the
/// `worker` event. Call while holding the renderer slot lock so a reader
/// that samples [`current_generation`] under the same lock sees the host
/// it rendered with; emit the returned event after releasing it.
///
/// `worker`: `{worker_gen, snapshot_gen, phase: "boot"|"refresh",
/// previous: {worker_gen, snapshot_gen}|null}`
pub fn adopt_worker(snapshot_gen: Option<u64>, phase: &str) -> Option<PendingEvent> {
    if !enabled() {
        return None;
    }
    let worker_gen = WORKER_GENS.fetch_add(1, Ordering::Relaxed) + 1;
    let generation = Generation {
        worker_gen,
        snapshot_gen: snapshot_gen.unwrap_or(0),
    };
    let previous = CURRENT_GENERATION
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .replace(generation);
    Some(PendingEvent::new(
        "worker",
        serde_json::json!({
            "worker_gen": generation.worker_gen,
            "snapshot_gen": generation.snapshot_gen,
            "phase": phase,
            "previous": previous.map(Generation::to_json),
        }),
    ))
}

/// Marks the current thread as running tick `id` until dropped, so events
/// emitted from code the tick calls carry `"tick": id`.
pub struct TickScope {
    previous: Option<u64>,
}

impl TickScope {
    pub fn enter(id: Option<u64>) -> Self {
        let previous = CURRENT_TICK.with(|cell| cell.replace(id));
        Self { previous }
    }
}

impl Drop for TickScope {
    fn drop(&mut self) {
        CURRENT_TICK.with(|cell| cell.set(self.previous));
    }
}

/// Serialize a list, capping it at [`LIST_CAP`]. Inserts `key` and, when
/// capped, `<key>_truncated` into `into`.
pub fn insert_capped<T>(
    into: &mut Map<String, Value>,
    key: &str,
    items: impl IntoIterator<Item = T>,
    to_value: impl Fn(T) -> Value,
) {
    let mut list = Vec::new();
    let mut omitted = 0usize;
    for item in items {
        if list.len() < LIST_CAP {
            list.push(to_value(item));
        } else {
            omitted += 1;
        }
    }
    into.insert(key.to_string(), Value::Array(list));
    if omitted > 0 {
        into.insert(format!("{key}_truncated"), Value::from(omitted));
    }
}

/// A path as a JSON string (lossy for non-UTF-8 paths).
pub fn path_value(path: &std::path::Path) -> Value {
    Value::String(path.to_string_lossy().into_owned())
}

/// Build one trace line. `fields` must be a JSON object (anything else is
/// stored under `"data"`); `ev`/`seq`/`ms` are placed first and override
/// same-named fields, and `tick` is added only when absent from `fields`.
pub fn format_line(ev: &str, seq: u64, ms: u64, tick: Option<u64>, fields: Value) -> String {
    let mut out = Map::new();
    out.insert("ev".into(), Value::from(ev));
    out.insert("seq".into(), Value::from(seq));
    out.insert("ms".into(), Value::from(ms));
    let fields = match fields {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        other => {
            let mut map = Map::new();
            map.insert("data".into(), other);
            map
        }
    };
    if let Some(tick) = tick {
        if !fields.contains_key("tick") {
            out.insert("tick".into(), Value::from(tick));
        }
    }
    for (key, value) in fields {
        if !matches!(key.as_str(), "ev" | "seq" | "ms") {
            out.insert(key, value);
        }
    }
    format!("{LINE_PREFIX}{}", Value::Object(out))
}

/// Emit one event line to stderr. No-op when the trace is off; callers that
/// build non-trivial `fields` should check [`enabled`] first.
pub fn emit(ev: &str, fields: Value) {
    if !enabled() {
        return;
    }
    let tick = CURRENT_TICK.with(Cell::get);
    let mut seq = EMIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    *seq += 1;
    // Sampled under the same lock as `seq`, so a higher `seq` never carries a lower `ms`.
    let ms = EPOCH
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    eprintln!("{}", format_line(ev, *seq, ms, tick, fields));
}

/// Actual native watcher tick captured on the refresh thread before spawning.
#[cfg(all(feature = "renderer-memory-observe", target_os = "linux"))]
pub fn renderer_tick() -> Option<u64> {
    CURRENT_TICK.with(Cell::get)
}

/// Relative monotonic time shared by the existing trace and delayed numeric
/// renderer observations. Callers first check the existing runtime opt-in.
#[cfg(all(feature = "renderer-memory-observe", target_os = "linux"))]
pub fn elapsed_us() -> u64 {
    EPOCH
        .get_or_init(Instant::now)
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Linux diagnostic output which leaves ordinary stderr's flags untouched.
/// Node's child-process stderr is a socket on Linux. Shell pipes use a
/// separately opened file description with O_NONBLOCK.
#[cfg(all(feature = "renderer-memory-observe", target_os = "linux"))]
pub struct NonblockingTraceSink(TraceSink);

#[cfg(all(feature = "renderer-memory-observe", target_os = "linux"))]
enum TraceSink {
    Socket(std::os::fd::RawFd),
    Pipe(std::fs::File),
}

#[cfg(all(feature = "renderer-memory-observe", target_os = "linux"))]
impl TraceSink {
    fn write_once(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        use std::io::Write;
        match self {
            Self::Pipe(file) => file.write(bytes),
            Self::Socket(fd) => {
                // SAFETY: bytes describes readable storage for the duration of
                // send. An invalid/closed fd is reported as an I/O error.
                let sent = unsafe {
                    libc::send(
                        *fd,
                        bytes.as_ptr().cast(),
                        bytes.len(),
                        libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                    )
                };
                usize::try_from(sent).map_err(|_| std::io::Error::last_os_error())
            }
        }
    }
}

#[cfg(all(feature = "renderer-memory-observe", target_os = "linux"))]
pub enum TraceWrite {
    Written,
    Retry,
    Dropped,
}

#[cfg(all(feature = "renderer-memory-observe", target_os = "linux"))]
impl NonblockingTraceSink {
    pub const LINE_LIMIT_BYTES: usize = 1024;

    pub fn open() -> std::io::Result<Self> {
        use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
        const STDERR_PATH: &str = "/proc/self/fd/2";
        let file_type = std::fs::metadata(STDERR_PATH)?.file_type();
        if file_type.is_socket() {
            return Ok(Self(TraceSink::Socket(libc::STDERR_FILENO)));
        }
        if file_type.is_fifo() {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(STDERR_PATH)?;
            // Check the actual open file too, so a racing replacement cannot
            // turn this into a blocking regular-file write.
            if file.metadata()?.file_type().is_fifo() {
                return Ok(Self(TraceSink::Pipe(file)));
            }
        }
        Err(std::io::Error::other(
            "renderer observation requires a stderr socket or pipe",
        ))
    }

    /// A single nonblocking send/write bounded below Linux PIPE_BUF, with no
    /// retry while the existing trace sequencing lock is held. A short
    /// socket write is reported as loss, never continued as a second line.
    pub fn try_emit(&mut self, ev: &str, fields: Value) -> TraceWrite {
        use std::io::ErrorKind;
        let Ok(mut seq) = EMIT_LOCK.try_lock() else {
            return TraceWrite::Retry;
        };
        let Some(next) = seq.checked_add(1) else {
            return TraceWrite::Dropped;
        };
        let mut line = format_line(ev, next, elapsed_us() / 1000, None, fields);
        line.push('\n');
        if line.len() > Self::LINE_LIMIT_BYTES {
            return TraceWrite::Dropped;
        }
        match self.0.write_once(line.as_bytes()) {
            Ok(n) if n == line.len() => {
                *seq = next;
                TraceWrite::Written
            }
            Err(error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) =>
            {
                TraceWrite::Retry
            }
            _ => TraceWrite::Dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(feature = "renderer-memory-observe", target_os = "linux"))]
    #[test]
    fn renderer_trace_returns_on_full_socket_without_changing_flags() {
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixStream;

        let (socket, _reader) = UnixStream::pair().unwrap();
        let fd = socket.as_raw_fd();
        // SAFETY: fd is held open by socket for the duration of the test.
        let before = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(before >= 0);
        assert_eq!(before & libc::O_NONBLOCK, 0);

        let mut sink = NonblockingTraceSink(TraceSink::Socket(fd));
        while sink.0.write_once(&[0; 1024]).is_ok() {}
        assert!(matches!(
            sink.try_emit("renderer_lifecycle", serde_json::json!({"occurrence": 1})),
            TraceWrite::Retry
        ));
        // SAFETY: socket still owns fd.
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFL) }, before);
    }

    #[test]
    fn gate_parses_only_truthy_values() {
        assert!(parse_enabled(Some("1")));
        assert!(parse_enabled(Some(" TRUE ")));
        assert!(!parse_enabled(None));
        assert!(!parse_enabled(Some("")));
        assert!(!parse_enabled(Some("0")));
        assert!(!parse_enabled(Some("yes")));
    }

    #[test]
    fn line_is_prefixed_single_line_json_with_header_first() {
        let line = format_line(
            "select",
            7,
            42,
            Some(3),
            serde_json::json!({"paths": ["a\nb"], "seq": 99}),
        );
        assert!(line.starts_with(LINE_PREFIX));
        assert!(!line.contains('\n'));
        let body = line.strip_prefix(LINE_PREFIX).unwrap();
        assert!(body.starts_with(r#"{"ev":"select","seq":7,"ms":42,"tick":3,"#));
        let parsed: Value = serde_json::from_str(body).unwrap();
        assert_eq!(parsed["seq"], 7, "header fields win over same-named fields");
        assert_eq!(parsed["paths"][0], "a\nb");
    }

    #[test]
    fn explicit_tick_field_is_not_overridden() {
        let line = format_line("intake", 1, 0, Some(9), serde_json::json!({"tick": 2}));
        let parsed: Value = serde_json::from_str(line.strip_prefix(LINE_PREFIX).unwrap()).unwrap();
        assert_eq!(parsed["tick"], 2);
    }

    #[test]
    fn no_tick_field_outside_a_tick() {
        let line = format_line("worker", 1, 0, None, serde_json::json!({}));
        assert_eq!(line, r#"[zfb-prov] {"ev":"worker","seq":1,"ms":0}"#);
    }

    #[test]
    fn capped_list_reports_truncation() {
        let mut map = Map::new();
        insert_capped(&mut map, "pages", 0..(LIST_CAP + 5), Value::from);
        assert_eq!(map["pages"].as_array().unwrap().len(), LIST_CAP);
        assert_eq!(map["pages_truncated"], 5);
        let mut small = Map::new();
        insert_capped(&mut small, "pages", 0..2, Value::from);
        assert!(!small.contains_key("pages_truncated"));
    }

    #[test]
    fn drain_of_generation_is_first_only_once_per_worker() {
        // Uses worker ids far from any a real adoption in this process
        // would allocate.
        assert!(note_drain_of_generation(1_000_001));
        assert!(!note_drain_of_generation(1_000_001));
        assert!(note_drain_of_generation(1_000_002));
        assert!(note_drain_of_generation(1_000_001));
    }

    #[test]
    fn tick_scope_restores_previous_tick() {
        assert_eq!(CURRENT_TICK.with(Cell::get), None);
        {
            let _outer = TickScope::enter(Some(1));
            {
                let _inner = TickScope::enter(Some(2));
                assert_eq!(CURRENT_TICK.with(Cell::get), Some(2));
            }
            assert_eq!(CURRENT_TICK.with(Cell::get), Some(1));
        }
        assert_eq!(CURRENT_TICK.with(Cell::get), None);
    }

    /// The test process never sets the env var, so every entry point is
    /// inert: no ids, no generation, and `emit` writes nothing.
    #[test]
    fn gate_off_allocates_nothing() {
        if std::env::var(ENV_VAR).is_ok() {
            return;
        }
        assert!(!enabled());
        assert_eq!(next_tick_id(), None);
        assert_eq!(next_snapshot_gen(), None);
        assert_eq!(next_drain_id(), None);
        assert!(adopt_worker(Some(1), "boot").is_none());
        note_drain_consumers([std::path::PathBuf::from("/p")]);
        assert_eq!(last_drain_contains(std::path::Path::new("/p")), None);
        assert_eq!(current_generation(), None);
        emit("worker", serde_json::json!({}));
        assert_eq!(*EMIT_LOCK.lock().unwrap(), 0, "no line was emitted");
    }
}
