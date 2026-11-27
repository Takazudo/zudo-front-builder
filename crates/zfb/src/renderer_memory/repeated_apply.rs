//! Finite diagnostic intervention after the delegated dev apply has returned.
use super::*;
use std::ffi::OsStr;
use std::sync::atomic::{AtomicU8, Ordering::SeqCst};

const IDLE: u8 = 0;
const BUSY: u8 = 1;
const STOPPED: u8 = 2;
const LIMIT_US: u64 = 2_000_000;

#[derive(Clone, Copy, Debug, Serialize)]
pub(super) struct Receipt {
    arm: reclamation::Arm,
    ordinal: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trim_return: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

pub(super) struct Observation {
    arm: reclamation::Arm,
    cap: u64,
    gate: AtomicU8,
    ordinal: AtomicU64,
    previous: Mutex<Option<(u64, Context)>>,
}

// Compare the live process lookup with symbols in the already-loaded GNU
// libc. RTLD_NOLOAD prohibits bringing a different provider into the process.
#[cfg(target_env = "gnu")]
fn supported_provider() -> bool {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    // The admitted target is the original x86-64 WSL distribution. Require
    // its actual system libc device/inode; a matching SONAME alone is weak.
    if ["LD_PRELOAD", "LD_AUDIT", "LD_LIBRARY_PATH"].iter().any(|key| std::env::var_os(key).is_some())
        || ["/etc/ld.so.preload", "/etc/ld.so.audit"].iter().any(|path| !matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)) {
        return false;
    }
    let Ok(system) = std::fs::metadata("/usr/lib/x86_64-linux-gnu/libc.so.6") else {
        return false;
    };
    // SAFETY: each lookup uses a NUL-terminated static symbol name. A
    // successful dladdr supplies its live object's NUL-terminated name for
    // the lifetime of this retained handle. Never call through a lookup.
    unsafe {
        let handle = libc::dlopen(c"libc.so.6".as_ptr(), libc::RTLD_NOLOAD | libc::RTLD_NOW);
        if handle.is_null() {
            return false;
        }
        let ok = [
            c"malloc",
            c"calloc",
            c"realloc",
            c"free",
            c"aligned_alloc",
            c"posix_memalign",
            c"mallinfo2",
            c"malloc_trim",
        ]
        .into_iter()
        .all(|symbol| {
            let address = libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr());
            let native = libc::dlsym(handle, symbol.as_ptr());
            if address.is_null() || address != native {
                return false;
            }
            let mut info = std::mem::MaybeUninit::<libc::Dl_info>::uninit();
            if libc::dladdr(address, info.as_mut_ptr()) == 0 {
                return false;
            }
            let file = info.assume_init().dli_fname;
            if file.is_null() {
                return false;
            }
            let path = OsStr::from_bytes(CStr::from_ptr(file).to_bytes());
            std::fs::metadata(path)
                .is_ok_and(|file| file.dev() == system.dev() && file.ino() == system.ino())
        });
        libc::dlclose(handle);
        ok
    }
}

#[cfg(not(target_env = "gnu"))]
fn supported_provider() -> bool {
    false
}

impl Observation {
    pub(super) fn from_mode(mode: Option<&OsStr>) -> Option<Self> {
        let arm = match mode?.to_str()? {
            "5" if cfg!(target_env = "gnu") => reclamation::Arm::Sham,
            "6" if cfg!(target_env = "gnu") => reclamation::Arm::Trim,
            _ => return None,
        };
        let cap = match std::env::var("ZFB_DEV_REPEATED_APPLY_CAP").ok()?.as_str() {
            "4" => 4,
            "13" => 13,
            _ => return None,
        };
        if REPEATED_SHUTDOWN.load(SeqCst) || !supported_provider() {
            return None;
        }
        Some(Self::new(arm, cap))
    }

    fn new(arm: reclamation::Arm, cap: u64) -> Self {
        Self {
            arm,
            cap,
            gate: AtomicU8::new(IDLE),
            ordinal: AtomicU64::new(0),
            previous: Mutex::new(None),
        }
    }
    pub(super) fn add_startup(&self, value: &mut Value) {
        value["repeated_apply_arm"] = json!(self.arm);
        value["repeated_apply_cap"] = json!(self.cap);
        value["repeated_apply_bracket_us"] = json!(LIMIT_US);
    }
    pub(super) fn stop(&self) {
        self.gate.store(STOPPED, SeqCst);
    }

    pub(super) fn disarm(&self, buffer: &Buffer, context: Context, id: u64, reason: &'static str) {
        if self.gate.swap(STOPPED, SeqCst) != STOPPED {
            self.emit(
                buffer,
                context,
                id,
                Phase::RepeatedApplyDisarmed,
                Receipt {
                    arm: self.arm,
                    ordinal: self.ordinal.load(Relaxed),
                    started_us: None,
                    completed_us: None,
                    trim_return: None,
                    reason: Some(reason),
                },
            );
        }
    }

    fn emit(&self, buffer: &Buffer, context: Context, id: u64, phase: Phase, receipt: Receipt) {
        buffer.record(
            context,
            phase,
            Details {
                repeated_apply: Some(receipt),
                reclamation_apply_id: id,
                ..Details::default()
            },
        );
    }

    pub(super) fn returned(
        &self,
        buffer: &Buffer,
        context: Context,
        apply: ApplyRecord,
        pre: Option<AllocatorCompletion>,
    ) {
        self.observe(
            buffer,
            context,
            apply,
            pre.is_some_and(|p| p.allocator.is_some()),
            || {
                #[cfg(target_env = "gnu")]
                // SAFETY: GNU accepts zero padding; the caller has returned from
                // the delegated apply and holds neither publication nor state locks.
                unsafe {
                    libc::malloc_trim(0)
                }
                #[cfg(not(target_env = "gnu"))]
                unreachable!("unsupported target cannot arm this observation")
            },
        );
    }

    fn observe(
        &self,
        buffer: &Buffer,
        context: Context,
        apply: ApplyRecord,
        pre_ok: bool,
        trim: impl FnOnce() -> i32,
    ) {
        let id = apply.apply_id;
        if self.gate.load(SeqCst) == STOPPED {
            return;
        }
        if matches!(apply.apply_association, Some(ApplyAssociation::NoRefresh))
            && matches!(apply.apply_outcome, ApplyOutcome::Ok)
        {
            return;
        }
        if !matches!(apply.apply_association, Some(ApplyAssociation::Associated))
            || !matches!(apply.apply_outcome, ApplyOutcome::Ok)
            || apply.refresh_outcome != Some(BoundaryOutcome::Refreshed)
            || !context.tick.is_some_and(|t| t > 0)
            || context.refresh_id == 0
            || context.snapshot_gen == 0
            || context.host_id == 0
            || !pre_ok
        {
            self.disarm(buffer, context, id, "association");
            return;
        }
        if self
            .gate
            .compare_exchange(IDLE, BUSY, SeqCst, SeqCst)
            .is_err()
        {
            self.disarm(buffer, context, id, "concurrent");
            return;
        }
        {
            let Ok(mut previous) = self.previous.try_lock() else {
                self.disarm(buffer, context, id, "concurrent");
                return;
            };
            if previous.is_some_and(|(prior, p)| {
                id <= prior
                    || context.tick <= p.tick
                    || context.refresh_id <= p.refresh_id
                    || context.snapshot_gen <= p.snapshot_gen
                    || context.host_id <= p.host_id
            }) {
                self.disarm(buffer, context, id, "progression");
                return;
            }
            *previous = Some((id, context));
        }
        let healthy = || {
            !buffer.closed.load(Relaxed)
                && !buffer.output_failed.load(Relaxed)
                && buffer.contention.load(Relaxed) == 0
                && buffer.overflow.load(Relaxed) == 0
        };
        // Two action rows plus one potential disarm must fit. Other producers
        // can still race after this check, so loss after admission remains a
        // failed observation and permanently stops future actions.
        let room = buffer
            .queue
            .try_lock()
            .is_ok_and(|queue| queue.len() <= CAPACITY - 3);
        if !healthy() || !room || buffer.attempted.load(Relaxed) > ATTEMPT_LIMIT - 3 {
            self.disarm(buffer, context, id, "evidence");
            return;
        }
        let ordinal = self.ordinal.fetch_add(1, Relaxed) + 1;
        let started = prov::elapsed_us();
        let mut receipt = Receipt {
            arm: self.arm,
            ordinal,
            started_us: Some(started),
            completed_us: None,
            trim_return: None,
            reason: None,
        };
        self.emit(buffer, context, id, Phase::RepeatedApplyStart, receipt);
        if !healthy() {
            self.disarm(buffer, context, id, "evidence");
            return;
        }
        // This atomic read is the acceptance boundary with concurrent shutdown.
        // An already accepted action may complete after stop().
        if self.gate.load(SeqCst) != BUSY || REPEATED_SHUTDOWN.load(SeqCst) {
            return;
        }
        if self.arm == reclamation::Arm::Trim {
            receipt.trim_return = Some(trim());
        }
        let end = prov::elapsed_us();
        receipt.completed_us = Some(end);
        self.emit(buffer, context, id, Phase::RepeatedApplyComplete, receipt);
        if end.saturating_sub(started) > LIMIT_US {
            self.disarm(buffer, context, id, "duration");
        } else if !healthy() {
            self.disarm(buffer, context, id, "evidence");
        } else if ordinal == self.cap {
            self.disarm(buffer, context, id, "cap");
        } else {
            let _ = self.gate.compare_exchange(BUSY, IDLE, SeqCst, SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(n: u64) -> (Context, ApplyRecord) {
        (
            Context {
                refresh_id: n,
                snapshot_gen: n,
                host_id: n,
                tick: Some(n),
            },
            ApplyRecord {
                apply_id: n,
                apply_outcome: ApplyOutcome::Ok,
                apply_association: Some(ApplyAssociation::Associated),
                refresh_outcome: Some(BoundaryOutcome::Refreshed),
            },
        )
    }

    #[test]
    fn repeated_arms_record_only_the_bounded_progressing_endpoints() {
        for arm in [reclamation::Arm::Sham, reclamation::Arm::Trim] {
            let b = Buffer::new(true, true);
            let o = Observation::new(arm, 4);
            let calls = Cell::new(0);
            let (ctx, mut noop) = endpoint(1);
            noop.apply_association = Some(ApplyAssociation::NoRefresh);
            o.observe(&b, ctx, noop, false, || panic!("no-refresh"));
            for n in 1..=6 {
                let (c, a) = endpoint(n);
                o.observe(&b, c, a, true, || {
                    calls.set(calls.get() + 1);
                    0
                });
            }
            assert_eq!(
                calls.get(),
                if arm == reclamation::Arm::Trim { 4 } else { 0 }
            );
            let events: Vec<_> = b.queue.lock().unwrap().iter().map(|r| json!(r)).collect();
            assert_eq!(events.len(), 9);
            for (i, pair) in events[..8].chunks_exact(2).enumerate() {
                assert_eq!(pair[0]["phase"], "repeated_apply_start");
                assert_eq!(pair[1]["phase"], "repeated_apply_complete");
                assert_eq!(pair[1]["repeated_apply"]["ordinal"], i + 1);
                assert_eq!(pair[1]["apply_id"], i + 1);
                assert_eq!(
                    pair[1]["repeated_apply"]["trim_return"],
                    if arm == reclamation::Arm::Trim {
                        json!(0)
                    } else {
                        Value::Null
                    }
                );
            }
            assert_eq!(events[8]["repeated_apply"]["reason"], "cap");
        }
    }

    #[test]
    fn thirteenth_endpoint_caps_or_permanently_rejects_the_remaining_actions() {
        for arm in [reclamation::Arm::Sham, reclamation::Arm::Trim] {
            for reject_thirteenth in [false, true] {
                let buffer = Buffer::new(true, true);
                let observation = Observation::new(arm, 13);
                let calls = Cell::new(0);
                for id in 1..=14 {
                    let (mut context, apply) = endpoint(id);
                    if reject_thirteenth && id == 13 {
                        context.refresh_id = 12;
                    }
                    observation.observe(&buffer, context, apply, true, || {
                        calls.set(calls.get() + 1);
                        0
                    });
                }
                let count = if reject_thirteenth { 12 } else { 13 };
                assert_eq!(
                    calls.get(),
                    if arm == reclamation::Arm::Trim {
                        count
                    } else {
                        0
                    }
                );
                assert_eq!(observation.gate.load(SeqCst), STOPPED);
                let events: Vec<_> = buffer
                    .queue
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|r| json!(r))
                    .collect();
                let (disarm, actions) = events.split_last().unwrap();
                assert_eq!(actions.len(), count * 2);
                for (i, pair) in actions.chunks_exact(2).enumerate() {
                    assert_eq!(pair[0]["phase"], "repeated_apply_start");
                    assert_eq!(pair[1]["phase"], "repeated_apply_complete");
                    for row in pair {
                        assert_eq!(row["repeated_apply"]["arm"], json!(arm));
                        assert_eq!(row["apply_id"], i + 1);
                        assert_eq!(row["repeated_apply"]["ordinal"], i + 1);
                    }
                    assert_eq!(pair[0]["repeated_apply"]["trim_return"], Value::Null);
                    assert_eq!(
                        pair[1]["repeated_apply"]["trim_return"],
                        if arm == reclamation::Arm::Trim {
                            json!(0)
                        } else {
                            Value::Null
                        }
                    );
                }
                assert_eq!(disarm["phase"], "repeated_apply_disarmed");
                assert_eq!(disarm["repeated_apply"]["arm"], json!(arm));
                assert_eq!(disarm["repeated_apply"]["trim_return"], Value::Null);
                assert_eq!(disarm["apply_id"], 13);
                assert_eq!(disarm["repeated_apply"]["ordinal"], count);
                assert_eq!(
                    disarm["repeated_apply"]["reason"],
                    if reject_thirteenth {
                        "progression"
                    } else {
                        "cap"
                    }
                );
            }
        }
    }

    #[test]
    fn rejected_or_ambiguous_apply_permanently_prevents_recovery_actions() {
        for association in [
            ApplyAssociation::Associated,
            ApplyAssociation::Nested,
            ApplyAssociation::Duplicate,
            ApplyAssociation::Foreign,
            ApplyAssociation::Missing,
        ] {
            let b = Buffer::new(true, true);
            let o = Observation::new(reclamation::Arm::Trim, 4);
            let (c, mut a) = endpoint(1);
            a.apply_association = Some(association);
            if matches!(association, ApplyAssociation::Associated) {
                a.apply_outcome = ApplyOutcome::Error;
            }
            o.observe(&b, c, a, true, || panic!("ineligible"));
            let (c, a) = endpoint(2);
            o.observe(&b, c, a, true, || panic!("recovery"));
            assert_eq!(o.gate.load(SeqCst), STOPPED);
            assert_eq!(o.ordinal.load(Relaxed), 0);
        }
    }

    #[test]
    fn duplicate_or_lost_evidence_prevents_further_actions() {
        for loss in [false, true] {
            let b = Buffer::new(true, true);
            let o = Observation::new(reclamation::Arm::Trim, 4);
            let (c, a) = endpoint(1);
            o.observe(&b, c, a, true, || 1);
            let (c, a) = if loss {
                b.output_failed.store(true, Relaxed);
                endpoint(2)
            } else {
                endpoint(1)
            };
            o.observe(&b, c, a, true, || {
                panic!("repeated identity or missing evidence")
            });
            assert_eq!(o.gate.load(SeqCst), STOPPED);
        }
    }

    #[test]
    fn shutdown_or_concurrent_apply_cannot_reopen_in_flight_gate() {
        for concurrent in [true, false] {
            let b = Buffer::new(true, true);
            let o = Observation::new(reclamation::Arm::Trim, 4);
            let (c, a) = endpoint(1);
            o.observe(&b, c, a, true, || {
                if concurrent {
                    let (c, a) = endpoint(2);
                    o.observe(&b, c, a, true, || panic!("overlap"));
                } else {
                    o.stop();
                }
                0
            });
            let (c, a) = endpoint(3);
            o.observe(&b, c, a, true, || panic!("reopened"));
            assert_eq!(o.gate.load(SeqCst), STOPPED);
        }
    }

    #[test]
    fn bounded_queue_and_lifecycle_headroom_are_required_before_entry() {
        for exhausted_queue in [true, false] {
            let b = Buffer::new(true, true);
            if exhausted_queue {
                for _ in 0..CAPACITY - 2 {
                    b.record(Context::default(), Phase::RefreshStart, Details::default());
                }
            } else {
                b.attempted.store(ATTEMPT_LIMIT - 2, Relaxed);
            }
            let o = Observation::new(reclamation::Arm::Trim, 4);
            let (c, a) = endpoint(1);
            o.observe(&b, c, a, true, || panic!("insufficient headroom"));
            assert_eq!(o.gate.load(SeqCst), STOPPED);
            assert_eq!(o.ordinal.load(Relaxed), 0);
        }
    }

    #[test]
    fn unwinding_scope_disarms_repeated_actions() {
        let mut b = Buffer::new(true, true);
        b.repeated_apply = Some(Observation::new(reclamation::Arm::Trim, 4));
        drop(ApplyScope::with_buffer(&b, true));
        assert_eq!(
            b.repeated_apply.as_ref().unwrap().gate.load(SeqCst),
            STOPPED
        );
    }
}
