//! One consumed diagnostic action at a uniquely associated apply endpoint.
//! Buffers are initialized at capture startup. No allocation, queue lock,
//! application lock, retry, or flush wait spans the native action bracket.

use super::*;
use std::ffi::OsStr;

const TARGET: u64 = 13;
const ROLLUP_BYTES: usize = 16 * 1024;
const READ_LIMIT: u64 = 8;
const BRACKET_US: u64 = 2_000_000;
const CHUNK_BYTES: usize = 128;
// Two complete raw inputs, their read boundaries, action boundaries, the
// post getter and completion. Reserve the full bound before invoking FFI.
const RECORD_RESERVE: u64 = (2 * ROLLUP_BYTES / CHUNK_BYTES + 9) as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Arm {
    Sham,
    Trim,
}

impl Arm {
    pub(super) fn from_mode(mode: Option<&OsStr>) -> Option<Self> {
        match mode.and_then(OsStr::to_str) {
            Some("3") => Some(Self::Sham),
            Some("4") => Some(Self::Trim),
            _ => None,
        }
    }
}

pub(super) fn add_startup(value: &mut Value, arm: Arm) {
    value["allocator_reclamation"] = json!(arm);
    value["reclamation_target"] = json!(TARGET);
    value["reclamation_rollup_bytes"] = json!(ROLLUP_BYTES);
    value["reclamation_read_limit"] = json!(READ_LIMIT);
    value["reclamation_bracket_us"] = json!(BRACKET_US);
    value["reclamation_chunk_bytes"] = json!(CHUNK_BYTES);
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Side {
    Before,
    After,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Chunk {
    bytes: [u8; CHUNK_BYTES],
    len: usize,
}

impl Serialize for Chunk {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut hex = [0; CHUNK_BYTES * 2];
        for (i, byte) in self.bytes[..self.len].iter().enumerate() {
            hex[2 * i] = DIGITS[usize::from(byte >> 4)];
            hex[2 * i + 1] = DIGITS[usize::from(byte & 15)];
        }
        // The table above contains only ASCII.
        serializer.serialize_str(std::str::from_utf8(&hex[..2 * self.len]).unwrap())
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub(super) struct Rollup {
    rss_kib: u64,
    anonymous_kib: u64,
    private_clean_kib: u64,
    private_dirty_kib: u64,
    swap_kib: u64,
    anon_huge_pages_kib: u64,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(untagged)]
pub(super) enum Record {
    ReadStart {
        side: Side,
        started_us: u64,
    },
    ReadComplete {
        side: Side,
        started_us: u64,
        completed_us: u64,
        duration_us: u64,
        bytes: usize,
        reads: u64,
        eof: bool,
        outcome: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        rollup: Option<Rollup>,
    },
    Raw {
        side: Side,
        offset: usize,
        hex: Chunk,
    },
    ActionStart {
        arm: Arm,
        eligible: u64,
        started_us: u64,
    },
    ActionComplete {
        arm: Arm,
        eligible: u64,
        started_us: u64,
        completed_us: u64,
        duration_us: u64,
        performed: bool,
        trim_return: Option<i32>,
    },
    Complete {
        arm: Arm,
        eligible: u64,
        started_us: u64,
        completed_us: u64,
        duration_us: u64,
        performed: bool,
        outcome: &'static str,
    },
    Disarmed {
        reason: &'static str,
        eligible: u64,
    },
}

struct Storage {
    before: [u8; ROLLUP_BYTES],
    after: [u8; ROLLUP_BYTES],
}

struct State {
    eligible: u64,
    previous: Option<(u64, Context)>,
    storage: Option<Box<Storage>>,
}

pub(super) struct Observation {
    arm: Arm,
    spent: AtomicBool,
    state: Mutex<State>,
}

impl Observation {
    pub(super) fn new(arm: Arm) -> Self {
        Self {
            arm,
            spent: AtomicBool::new(false),
            state: Mutex::new(State {
                eligible: 0,
                previous: None,
                storage: Some(Box::new(Storage {
                    before: [0; ROLLUP_BYTES],
                    after: [0; ROLLUP_BYTES],
                })),
            }),
        }
    }

    pub(super) fn disarm(
        &self,
        buffer: &Buffer,
        context: Context,
        apply_id: u64,
        reason: &'static str,
    ) {
        if !self.spent.swap(true, Relaxed) {
            let eligible = self.state.try_lock().map_or(0, |state| state.eligible);
            emit(
                buffer,
                context,
                apply_id,
                Phase::ReclamationDisarmed,
                Record::Disarmed { reason, eligible },
                prov::elapsed_us(),
            );
        }
    }

    pub(super) fn returned(
        &self,
        buffer: &Buffer,
        context: Context,
        apply: ApplyRecord,
        pre: Option<AllocatorCompletion>,
    ) {
        if self.spent.load(Relaxed) {
            return;
        }
        if matches!(apply.apply_association, Some(ApplyAssociation::NoRefresh))
            && matches!(apply.apply_outcome, ApplyOutcome::Ok)
        {
            return;
        }
        let valid = matches!(apply.apply_association, Some(ApplyAssociation::Associated))
            && matches!(apply.apply_outcome, ApplyOutcome::Ok)
            && apply.refresh_outcome == Some(BoundaryOutcome::Refreshed)
            && context.tick.is_some_and(|tick| tick > 0)
            && context.refresh_id > 0
            && context.snapshot_gen > 0
            && context.host_id > 0;
        if !valid {
            self.disarm(buffer, context, apply.apply_id, "association");
            return;
        }
        let Some(pre) = pre.filter(|pre| pre.allocator.is_some()) else {
            self.disarm(buffer, context, apply.apply_id, "pre_allocator");
            return;
        };
        let Ok(mut state) = self.state.try_lock() else {
            self.disarm(buffer, context, apply.apply_id, "concurrent");
            return;
        };
        if self.spent.load(Relaxed) {
            return;
        }
        if state.previous.is_some_and(|(id, previous)| {
            apply.apply_id <= id
                || context.tick <= previous.tick
                || context.refresh_id <= previous.refresh_id
                || context.snapshot_gen <= previous.snapshot_gen
                || context.host_id <= previous.host_id
        }) {
            drop(state);
            self.disarm(buffer, context, apply.apply_id, "progression");
            return;
        }
        state.eligible += 1;
        state.previous = Some((apply.apply_id, context));
        if state.eligible != TARGET {
            return;
        }
        // Consume permanently before any action or read. The boxed raw storage
        // moves out of the state lock, which ends before the bracket continues.
        if self.spent.swap(true, Relaxed) {
            return;
        }
        let storage = state.storage.take();
        drop(state);
        let Some(mut storage) = storage else {
            return;
        };
        let mut backend = Native;
        run(
            buffer,
            context,
            apply,
            self.arm,
            pre,
            &mut storage,
            &mut backend,
        );
    }
}

fn emit(
    buffer: &Buffer,
    context: Context,
    apply_id: u64,
    phase: Phase,
    record: Record,
    occurred_us: u64,
) {
    buffer.record_at(
        context,
        phase,
        Details {
            reclamation: Some(record),
            reclamation_apply_id: apply_id,
            ..Details::default()
        },
        occurred_us,
    );
}

fn healthy(buffer: &Buffer) -> bool {
    !buffer.closed.load(Relaxed)
        && !buffer.output_failed.load(Relaxed)
        && buffer.contention.load(Relaxed) == 0
        && buffer.overflow.load(Relaxed) == 0
}

trait Backend {
    fn rollup(&mut self, input: &mut [u8; ROLLUP_BYTES]) -> Read;
    fn trim(&mut self) -> i32;
}

struct Native;
impl Backend for Native {
    fn rollup(&mut self, input: &mut [u8; ROLLUP_BYTES]) -> Read {
        read_rollup(input)
    }
    fn trim(&mut self) -> i32 {
        #[cfg(target_env = "gnu")]
        // SAFETY: this feature-only binding is reached once, after the full
        // apply and its locks returned, outside destruction and bounded join.
        unsafe {
            libc::malloc_trim(0)
        }
        #[cfg(not(target_env = "gnu"))]
        unreachable!("unsupported allocator baseline prevents this action")
    }
}

fn run(
    buffer: &Buffer,
    context: Context,
    apply: ApplyRecord,
    arm: Arm,
    pre: AllocatorCompletion,
    storage: &mut Storage,
    backend: &mut impl Backend,
) {
    let started_us = pre.allocator_started_us;
    let mut performed = false;
    let result = (|| {
        if !healthy(buffer) {
            return Err("telemetry");
        }
        if buffer.attempted.load(Relaxed) + RECORD_RESERVE > ATTEMPT_LIMIT
            || buffer.allocator_calls.load(Relaxed) >= ALLOCATOR_CALL_LIMIT
        {
            return Err("telemetry_budget");
        }
        let before = capture(
            buffer,
            context,
            apply.apply_id,
            Side::Before,
            &mut storage.before,
            backend,
        );
        raw(
            buffer,
            context,
            apply.apply_id,
            Side::Before,
            &storage.before[..before.bytes],
            before.completed_us,
        );
        if before.outcome != "ok" {
            return Err(before.outcome);
        }
        if !healthy(buffer) {
            return Err("telemetry");
        }
        if prov::elapsed_us().saturating_sub(started_us) > BRACKET_US {
            return Err("elapsed");
        }
        let action_start = prov::elapsed_us();
        emit(
            buffer,
            context,
            apply.apply_id,
            Phase::ReclamationActionStart,
            Record::ActionStart {
                arm,
                eligible: TARGET,
                started_us: action_start,
            },
            action_start,
        );
        // A lost start cannot authorize a treatment call.
        if !healthy(buffer) {
            return Err("telemetry");
        }
        let trim_return = match arm {
            Arm::Sham => None,
            Arm::Trim => {
                performed = true;
                Some(backend.trim())
            }
        };
        let action_end = prov::elapsed_us();
        emit(
            buffer,
            context,
            apply.apply_id,
            Phase::ReclamationActionComplete,
            Record::ActionComplete {
                arm,
                eligible: TARGET,
                started_us: action_start,
                completed_us: action_end,
                duration_us: action_end.saturating_sub(action_start),
                performed,
                trim_return,
            },
            action_end,
        );
        let after = capture(
            buffer,
            context,
            apply.apply_id,
            Side::After,
            &mut storage.after,
            backend,
        );
        let post = record_allocator_for(
            buffer,
            context,
            AllocatorContext::Unlocked,
            AllocatorPhase::ReclamationAfter,
            Some(BoundaryOutcome::Returned),
            Some(ApplyRecord {
                apply_association: None,
                ..apply
            }),
        );
        raw(
            buffer,
            context,
            apply.apply_id,
            Side::After,
            &storage.after[..after.bytes],
            after.completed_us,
        );
        if after.outcome != "ok" {
            return Err(after.outcome);
        }
        if !post.is_some_and(|post| post.allocator.is_some()) {
            return Err("post_allocator");
        }
        if !healthy(buffer) {
            return Err("telemetry");
        }
        if prov::elapsed_us().saturating_sub(started_us) > BRACKET_US {
            return Err("elapsed");
        }
        Ok(())
    })();
    let completed_us = prov::elapsed_us();
    emit(
        buffer,
        context,
        apply.apply_id,
        Phase::ReclamationComplete,
        Record::Complete {
            arm,
            eligible: TARGET,
            started_us,
            completed_us,
            duration_us: completed_us.saturating_sub(started_us),
            performed,
            outcome: result.err().unwrap_or("ok"),
        },
        completed_us,
    );
}

fn raw(buffer: &Buffer, context: Context, apply_id: u64, side: Side, input: &[u8], at_us: u64) {
    for (index, bytes) in input.chunks(CHUNK_BYTES).enumerate() {
        let mut chunk = Chunk {
            bytes: [0; CHUNK_BYTES],
            len: bytes.len(),
        };
        chunk.bytes[..bytes.len()].copy_from_slice(bytes);
        emit(
            buffer,
            context,
            apply_id,
            Phase::ReclamationRollupChunk,
            Record::Raw {
                side,
                offset: index * CHUNK_BYTES,
                hex: chunk,
            },
            at_us,
        );
    }
}

#[derive(Clone, Copy)]
struct Read {
    started_us: u64,
    completed_us: u64,
    bytes: usize,
    reads: u64,
    eof: bool,
    outcome: &'static str,
    rollup: Option<Rollup>,
}

fn capture(
    buffer: &Buffer,
    context: Context,
    apply_id: u64,
    side: Side,
    input: &mut [u8; ROLLUP_BYTES],
    backend: &mut impl Backend,
) -> Read {
    let started_us = prov::elapsed_us();
    emit(
        buffer,
        context,
        apply_id,
        Phase::ReclamationRollupStart,
        Record::ReadStart { side, started_us },
        started_us,
    );
    let mut read = backend.rollup(input);
    // Include open, read, close and parse in the complete interval.
    read.started_us = started_us;
    emit(
        buffer,
        context,
        apply_id,
        Phase::ReclamationRollupComplete,
        Record::ReadComplete {
            side,
            started_us,
            completed_us: read.completed_us,
            duration_us: read.completed_us.saturating_sub(started_us),
            bytes: read.bytes,
            reads: read.reads,
            eof: read.eof,
            outcome: read.outcome,
            rollup: read.rollup,
        },
        read.completed_us,
    );
    read
}

fn read_rollup(input: &mut [u8; ROLLUP_BYTES]) -> Read {
    let started_us = prov::elapsed_us();
    let mut result = Read {
        started_us,
        completed_us: started_us,
        bytes: 0,
        reads: 0,
        eof: false,
        outcome: "open",
        rollup: None,
    };
    // SAFETY: static NUL-terminated path and ordinary read-only descriptor.
    let fd = unsafe {
        libc::open(
            c"/proc/self/smaps_rollup".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
        )
    };
    if fd >= 0 {
        result = read_bounded(input, |remaining| {
            // SAFETY: the valid descriptor and writable slice remain live.
            unsafe { libc::read(fd, remaining.as_mut_ptr().cast(), remaining.len()) }
        });
        // SAFETY: this function owns fd. A failed close is observed, never retried.
        if unsafe { libc::close(fd) } != 0 {
            result.outcome = "close";
        }
    }

    result.completed_us = prov::elapsed_us();
    result
}

fn read_bounded(
    input: &mut [u8; ROLLUP_BYTES],
    mut read_once: impl FnMut(&mut [u8]) -> isize,
) -> Read {
    let started_us = prov::elapsed_us();
    let mut result = Read {
        started_us,
        completed_us: started_us,
        bytes: 0,
        reads: 0,
        eof: false,
        outcome: "read_limit",
        rollup: None,
    };
    let mut overflow = [0u8; 1];
    while result.reads < READ_LIMIT {
        let remaining = if result.bytes == ROLLUP_BYTES {
            &mut overflow[..]
        } else {
            &mut input[result.bytes..]
        };
        result.reads += 1;
        let read = read_once(remaining);
        if read < 0 {
            result.outcome = "read";
            break;
        }
        if read == 0 {
            result.eof = true;
            result.outcome = "ok";
            break;
        }
        if result.bytes == ROLLUP_BYTES {
            result.outcome = "too_large";
            break;
        }
        result.bytes += read as usize;
    }
    if result.outcome == "ok" {
        match parse_rollup(&input[..result.bytes]) {
            Ok(rollup) => result.rollup = Some(rollup),
            Err(reason) => result.outcome = reason,
        }
    }
    result.completed_us = prov::elapsed_us();
    result
}

fn parse_hex_address(input: &[u8]) -> Result<u64, &'static str> {
    if input.is_empty() {
        return Err("malformed");
    }
    input.iter().try_fold(0u64, |value, digit| {
        let nibble = match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            b'A'..=b'F' => digit - b'A' + 10,
            _ => return Err("malformed"),
        };
        value
            .checked_mul(16)
            .and_then(|value| value.checked_add(u64::from(nibble)))
            .ok_or("overflow")
    })
}

fn parse_rollup(input: &[u8]) -> Result<Rollup, &'static str> {
    let content = input.strip_suffix(b"\n").ok_or("malformed")?;
    let mut lines = content.split(|byte| *byte == b'\n');
    let header = lines.next().ok_or("malformed")?;
    let mut header_fields = header
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty());
    let range = header_fields.next().ok_or("malformed")?;
    let mut addresses = range.split(|byte| *byte == b'-');
    let start = parse_hex_address(addresses.next().ok_or("malformed")?)?;
    let end = parse_hex_address(addresses.next().ok_or("malformed")?)?;
    if start >= end
        || addresses.next().is_some()
        || header_fields.next() != Some(&b"---p"[..])
        || header_fields.next() != Some(&b"00000000"[..])
        || header_fields.next() != Some(&b"00:00"[..])
        || header_fields.next() != Some(&b"0"[..])
        || header_fields.next() != Some(&b"[rollup]"[..])
        || header_fields.next().is_some()
    {
        return Err("malformed");
    }
    let mut names: [&[u8]; 64] = [&[]; 64];
    let mut count = 0;
    let mut required = 0u8;
    let mut rollup = Rollup::default();
    for line in lines {
        if line.is_empty() {
            return Err("malformed");
        }
        let colon = line
            .iter()
            .position(|byte| *byte == b':')
            .ok_or("malformed")?;
        let name = &line[..colon];
        if name.is_empty()
            || !name
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            return Err("malformed");
        }
        if names[..count].contains(&name) {
            return Err("duplicate");
        }
        if count == names.len() {
            return Err("malformed");
        }
        names[count] = name;
        count += 1;
        let mut fields = line[colon + 1..]
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|field| !field.is_empty());
        let digits = fields.next().ok_or("malformed")?;
        let mut value = 0u64;
        for digit in digits {
            if !digit.is_ascii_digit() {
                return Err("malformed");
            }
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(u64::from(*digit - b'0')))
                .ok_or("overflow")?;
        }
        if fields.next() != Some(&b"kB"[..]) || fields.next().is_some() {
            return Err("malformed");
        }
        let (bit, slot) = match name {
            b"Rss" => (1, &mut rollup.rss_kib),
            b"Anonymous" => (2, &mut rollup.anonymous_kib),
            b"Private_Clean" => (4, &mut rollup.private_clean_kib),
            b"Private_Dirty" => (8, &mut rollup.private_dirty_kib),
            b"Swap" => (16, &mut rollup.swap_kib),
            b"AnonHugePages" => (32, &mut rollup.anon_huge_pages_kib),
            _ => continue,
        };
        required |= bit;
        *slot = value;
    }
    if required != 63 {
        return Err("missing");
    }
    Ok(rollup)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT: &[u8] = b"1000-2000 ---p 00000000 00:00 0 [rollup]\nRss: 1234 kB\nAnonymous: 1000 kB\nPrivate_Clean: 12 kB\nPrivate_Dirty: 999 kB\nSwap: 0 kB\nAnonHugePages: 0 kB\n";

    fn context(id: u64) -> Context {
        Context {
            refresh_id: id,
            snapshot_gen: id,
            host_id: id,
            tick: Some(id),
        }
    }

    fn apply(id: u64) -> ApplyRecord {
        ApplyRecord {
            apply_id: id,
            apply_outcome: ApplyOutcome::Ok,
            apply_association: Some(ApplyAssociation::Associated),
            refresh_outcome: Some(BoundaryOutcome::Refreshed),
        }
    }

    fn baseline() -> AllocatorCompletion {
        AllocatorCompletion {
            allocator_started_us: prov::elapsed_us(),
            allocator_completed_us: prov::elapsed_us(),
            allocator_duration_us: 0,
            #[cfg(target_env = "gnu")]
            allocator_outcome: AllocatorOutcome::Ok,
            #[cfg(not(target_env = "gnu"))]
            allocator_outcome: AllocatorOutcome::Unavailable,
            allocator_unavailable_reason: None,
            allocator: Some(AllocatorCounters {
                arena_bytes: 1,
                uordblks_bytes: 1,
                fordblks_bytes: 0,
                hblkhd_bytes: 0,
                hblks: 0,
                fsmblks_bytes: 0,
            }),
        }
    }

    fn rows(buffer: &Buffer) -> Vec<Value> {
        buffer
            .queue
            .lock()
            .unwrap()
            .iter()
            .map(|record| json!(record))
            .collect()
    }

    struct Fake {
        reads: usize,
        calls: usize,
        bad_read: Option<usize>,
    }

    impl Backend for Fake {
        fn rollup(&mut self, input: &mut [u8; ROLLUP_BYTES]) -> Read {
            self.reads += 1;
            input[..INPUT.len()].copy_from_slice(INPUT);
            if self.bad_read == Some(self.reads) {
                input[0] = b'X';
            }
            let parsed = parse_rollup(&input[..INPUT.len()]);
            Read {
                started_us: prov::elapsed_us(),
                completed_us: prov::elapsed_us(),
                bytes: INPUT.len(),
                reads: 2,
                eof: true,
                outcome: parsed.as_ref().err().copied().unwrap_or("ok"),
                rollup: parsed.ok(),
            }
        }

        fn trim(&mut self) -> i32 {
            self.calls += 1;
            0
        }
    }

    #[test]
    fn rollup_bounds_eof_and_field_validation_are_strict() {
        assert_eq!(parse_rollup(INPUT).unwrap().rss_kib, 1234);
        let mut malformed_header = INPUT.to_vec();
        malformed_header[0] = b'X';
        assert_eq!(parse_rollup(&malformed_header).unwrap_err(), "malformed");
        let blank = String::from_utf8(INPUT.to_vec())
            .unwrap()
            .replace("Rss:", "\nRss:");
        assert_eq!(parse_rollup(blank.as_bytes()).unwrap_err(), "malformed");
        let mut input = INPUT.to_vec();
        input.extend_from_slice(b"Rss: 2 kB\n");
        assert_eq!(parse_rollup(&input).unwrap_err(), "duplicate");
        assert_eq!(
            parse_rollup(&INPUT[..INPUT.len() - 1]).unwrap_err(),
            "malformed"
        );
        let missing = String::from_utf8(INPUT.to_vec())
            .unwrap()
            .replace("Swap: 0 kB\n", "");
        assert_eq!(parse_rollup(missing.as_bytes()).unwrap_err(), "missing");
        let overflow = String::from_utf8(INPUT.to_vec())
            .unwrap()
            .replace("Rss: 1234", "Rss: 18446744073709551616");
        assert_eq!(parse_rollup(overflow.as_bytes()).unwrap_err(), "overflow");
        let mut storage = [0; ROLLUP_BYTES];
        let mut calls = 0;
        let read = read_bounded(&mut storage, |out| {
            calls += 1;
            out[0] = b'X';
            1
        });
        assert_eq!(
            (read.outcome, calls, read.bytes, read.eof),
            ("read_limit", 8, 8, false)
        );
        let read = read_bounded(&mut storage, |out| {
            out.fill(b'X');
            out.len() as isize
        });
        assert_eq!(
            (read.outcome, read.reads, read.bytes),
            ("too_large", 2, ROLLUP_BYTES)
        );
        let read = read_bounded(&mut storage, |_| -1);
        assert_eq!((read.outcome, read.reads), ("read", 1));
        let mut remaining = INPUT;
        let read = read_bounded(&mut storage, |out| {
            let count = remaining.len();
            out[..count].copy_from_slice(remaining);
            remaining = &[];
            count as isize
        });
        assert_eq!((read.outcome, read.reads, read.eof), ("ok", 2, true));
    }

    #[test]
    fn invalid_pre_read_never_trims_and_post_failure_preserves_performed() {
        for (bad_read, expected_calls) in [(Some(1), 0), (Some(2), 1), (None, 1)] {
            let buffer = Buffer::new(true, true);
            let mut storage = Storage {
                before: [0; ROLLUP_BYTES],
                after: [0; ROLLUP_BYTES],
            };
            let mut fake = Fake {
                reads: 0,
                calls: 0,
                bad_read,
            };
            run(
                &buffer,
                context(13),
                apply(13),
                Arm::Trim,
                baseline(),
                &mut storage,
                &mut fake,
            );
            assert_eq!(fake.calls, expected_calls);
            let rows = rows(&buffer);
            let last = rows.last().unwrap();
            assert_eq!(last["phase"], "reclamation_complete");
            assert_eq!(last["reclamation"]["performed"], expected_calls == 1);
            if bad_read.is_some() {
                assert_eq!(last["reclamation"]["outcome"], "malformed");
            }
            if expected_calls == 1 {
                let action = rows
                    .iter()
                    .find(|r| r["phase"] == "reclamation_action_complete")
                    .unwrap();
                assert_eq!(action["reclamation"]["trim_return"], 0);
            }
        }
        let buffer = Buffer::new(true, true);
        let mut storage = Storage {
            before: [0; ROLLUP_BYTES],
            after: [0; ROLLUP_BYTES],
        };
        let mut fake = Fake {
            reads: 0,
            calls: 0,
            bad_read: None,
        };
        run(
            &buffer,
            context(13),
            apply(13),
            Arm::Sham,
            baseline(),
            &mut storage,
            &mut fake,
        );
        assert_eq!((fake.reads, fake.calls), (2, 0));
        let rows = rows(&buffer);
        let action = rows
            .iter()
            .find(|r| r["phase"] == "reclamation_action_complete")
            .unwrap();
        assert_eq!(action["reclamation"]["performed"], false);
        assert!(action["reclamation"]["trim_return"].is_null());
    }

    #[test]
    fn known_loss_or_budget_shortfall_prevents_action() {
        for kind in ["output", "contention", "overflow", "records", "calls"] {
            let buffer = Buffer::new(true, true);
            match kind {
                "output" => buffer.output_failed.store(true, Relaxed),
                "contention" => buffer.contention.store(1, Relaxed),
                "overflow" => buffer.overflow.store(1, Relaxed),
                "records" => buffer
                    .attempted
                    .store(ATTEMPT_LIMIT - RECORD_RESERVE + 1, Relaxed),
                _ => buffer.allocator_calls.store(ALLOCATOR_CALL_LIMIT, Relaxed),
            }
            let mut storage = Storage {
                before: [0; ROLLUP_BYTES],
                after: [0; ROLLUP_BYTES],
            };
            let mut fake = Fake {
                reads: 0,
                calls: 0,
                bad_read: None,
            };
            run(
                &buffer,
                context(13),
                apply(13),
                Arm::Trim,
                baseline(),
                &mut storage,
                &mut fake,
            );
            assert_eq!((fake.reads, fake.calls), (0, 0), "{kind}");
        }
    }

    #[test]
    fn endpoint_selection_consumes_once_and_errors_never_rearm() {
        for association in [
            ApplyAssociation::Missing,
            ApplyAssociation::Duplicate,
            ApplyAssociation::Foreign,
            ApplyAssociation::Nested,
        ] {
            let buffer = Buffer::new(true, true);
            let observation = Observation::new(Arm::Sham);
            observation.returned(
                &buffer,
                context(1),
                ApplyRecord {
                    apply_association: Some(association),
                    ..apply(1)
                },
                Some(baseline()),
            );
            for id in 2..=15 {
                observation.returned(&buffer, context(id), apply(id), Some(baseline()));
            }
            assert!(observation.spent.load(Relaxed));
            assert!(rows(&buffer)
                .iter()
                .all(|row| row["phase"] != "reclamation_action_start"));
        }
        let buffer = Buffer::new(true, true);
        let observation = Observation::new(Arm::Sham);
        observation.returned(
            &buffer,
            context(1),
            ApplyRecord {
                apply_association: Some(ApplyAssociation::NoRefresh),
                refresh_outcome: None,
                ..apply(1)
            },
            None,
        );
        for id in 2..=14 {
            observation.returned(&buffer, context(id), apply(id), Some(baseline()));
        }
        observation.returned(&buffer, context(15), apply(15), Some(baseline()));
        let rows = rows(&buffer);
        assert_eq!(
            rows.iter()
                .filter(|row| row["phase"] == "reclamation_action_start")
                .count(),
            1
        );
        assert!(rows
            .iter()
            .filter(|row| row["phase"] == "reclamation_action_start")
            .all(|row| row["apply_id"] == 14));
        assert!(observation.state.lock().unwrap().storage.is_none());
    }

    #[test]
    fn contention_unwind_and_failed_progression_cannot_select_a_later_endpoint() {
        let buffer = Buffer::with_reclamation(true, true, Some(Arm::Sham));
        let observation = buffer.reclamation.as_ref().unwrap();
        observation.returned(&buffer, context(1), apply(1), Some(baseline()));
        observation.returned(&buffer, context(1), apply(2), Some(baseline()));
        assert!(observation.spent.load(Relaxed));
        assert_eq!(
            rows(&buffer).last().unwrap()["reclamation"]["reason"],
            "progression"
        );

        let buffer = Buffer::with_reclamation(true, true, Some(Arm::Sham));
        let observation = buffer.reclamation.as_ref().unwrap();
        for id in 1..13 {
            observation.returned(&buffer, context(id), apply(id), Some(baseline()));
        }
        let unwind = std::panic::catch_unwind(|| {
            let _scope = ApplyScope::with_buffer(&buffer, true);
            panic!("delegated apply unwind");
        });
        assert!(unwind.is_err());
        assert!(APPLY.get().is_none());
        observation.returned(&buffer, context(13), apply(13), Some(baseline()));
        assert!(rows(&buffer)
            .iter()
            .all(|row| row["phase"] != "reclamation_action_start"));

        let buffer = Buffer::new(true, true);
        let guard = buffer.queue.lock().unwrap();
        let mut storage = Storage {
            before: [0; ROLLUP_BYTES],
            after: [0; ROLLUP_BYTES],
        };
        let mut fake = Fake {
            reads: 0,
            calls: 0,
            bad_read: None,
        };
        let before = Instant::now();
        run(
            &buffer,
            context(13),
            apply(13),
            Arm::Trim,
            baseline(),
            &mut storage,
            &mut fake,
        );
        assert_eq!(fake.calls, 0);
        assert!(before.elapsed() < Duration::from_secs(1));
        assert!(buffer.contention.load(Relaxed) > 0);
        drop(guard);
    }

    #[test]
    fn records_fit_trace_lines_at_maximum_identity_and_counter_widths() {
        let max = u64::MAX;
        let rollup = Rollup {
            rss_kib: max,
            anonymous_kib: max,
            private_clean_kib: max,
            private_dirty_kib: max,
            swap_kib: max,
            anon_huge_pages_kib: max,
        };
        let records = [
            Record::ReadComplete {
                side: Side::Before,
                started_us: max,
                completed_us: max,
                duration_us: max,
                bytes: ROLLUP_BYTES,
                reads: READ_LIMIT,
                eof: true,
                outcome: "ok",
                rollup: Some(rollup),
            },
            Record::Raw {
                side: Side::Before,
                offset: ROLLUP_BYTES,
                hex: Chunk {
                    bytes: [255; CHUNK_BYTES],
                    len: CHUNK_BYTES,
                },
            },
            Record::ActionComplete {
                arm: Arm::Trim,
                eligible: TARGET,
                started_us: max,
                completed_us: max,
                duration_us: max,
                performed: true,
                trim_return: Some(i32::MAX),
            },
            Record::Complete {
                arm: Arm::Trim,
                eligible: TARGET,
                started_us: max,
                completed_us: max,
                duration_us: max,
                performed: true,
                outcome: "telemetry_budget",
            },
        ];
        for record in records {
            let row = super::super::Record {
                occurrence: max,
                occurred_us: max,
                phase: Phase::ReclamationRollupComplete,
                context: context(max),
                details: Details {
                    reclamation: Some(record),
                    reclamation_apply_id: max,
                    ..Details::default()
                },
            };
            let line = prov::format_line("renderer_lifecycle", max, max, None, json!(row));
            assert!(
                line.len() + 1 <= NonblockingTraceSink::LINE_LIMIT_BYTES,
                "{} {line}",
                line.len() + 1
            );
        }
    }
}
