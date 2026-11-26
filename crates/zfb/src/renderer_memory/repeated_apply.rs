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
        if !healthy() || buffer.attempted.load(Relaxed) > ATTEMPT_LIMIT - 2 {
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
        if self.gate.load(SeqCst) != BUSY {
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
