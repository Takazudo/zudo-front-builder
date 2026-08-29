//! Publication state for generated islands companion files.

#![cfg_attr(not(feature = "embed_v8"), allow(dead_code))]

use std::collections::{HashSet, VecDeque};

pub(crate) const RETAINED_ISLANDS_GENERATIONS: usize = 2;
pub(crate) const RETAINED_CLIENT_SCRIPT_GENERATIONS: usize = 1;

#[derive(Default)]
pub(crate) struct IslandsCompanionLedger {
    live: HashSet<String>,
    staged: Option<HashSet<String>>,
    unresolved: HashSet<String>,
    baseline_unresolved: HashSet<String>,
    retained: VecDeque<HashSet<String>>,
}

pub(crate) struct PrunePlan {
    pub(crate) candidates: HashSet<String>,
    pub(crate) keep: HashSet<String>,
}

#[derive(Default)]
pub(crate) struct ClientScriptCompanionLedger {
    live: HashSet<String>,
    staged: Option<ClientScriptGeneration>,
    unresolved: HashSet<String>,
    baseline_unresolved: HashSet<String>,
    retained: VecDeque<HashSet<String>>,
}

struct ClientScriptGeneration {
    names: HashSet<String>,
    changed: bool,
}

impl ClientScriptCompanionLedger {
    pub(crate) fn begin(&mut self) {
        self.baseline_unresolved = self.unresolved.clone();
    }

    pub(crate) fn protected_filenames(&self) -> HashSet<String> {
        let mut protected = self.live.clone();
        protected.extend(self.retained.iter().flatten().cloned());
        protected.extend(self.unresolved.iter().cloned());
        protected
    }

    pub(crate) fn track_candidate(&mut self, names: &HashSet<String>) {
        self.unresolved.extend(names.iter().cloned());
    }

    pub(crate) fn stage(&mut self, names: HashSet<String>, build_changed: bool) {
        // The build helper preserves protected old files on disk, so an
        // output-set removal can be publication-visible even when no bytes
        // were written or pruned and its raw `changed` flag is false.
        let changed = build_changed || names != self.live;
        self.staged = Some(ClientScriptGeneration { names, changed });
    }

    pub(crate) fn abort_candidate(&mut self) -> HashSet<String> {
        self.staged = None;
        let mut protected = self.live.clone();
        protected.extend(self.retained.iter().flatten().cloned());
        protected.extend(self.baseline_unresolved.iter().cloned());
        let candidates = self
            .unresolved
            .difference(&self.baseline_unresolved)
            .filter(|name| !protected.contains(*name))
            .cloned()
            .collect();
        self.unresolved = self.baseline_unresolved.clone();
        candidates
    }

    pub(crate) fn commit(&mut self) -> PrunePlan {
        let Some(next) = self.staged.take() else {
            return PrunePlan {
                candidates: HashSet::new(),
                keep: HashSet::new(),
            };
        };
        if !next.changed {
            self.live = next.names;
            return PrunePlan {
                candidates: HashSet::new(),
                keep: HashSet::new(),
            };
        }

        let previous = std::mem::replace(&mut self.live, next.names);
        self.retained.push_front(previous);
        let evicted = self
            .retained
            .split_off(self.retained.len().min(RETAINED_CLIENT_SCRIPT_GENERATIONS));
        let mut keep = self.live.clone();
        keep.extend(self.retained.iter().flatten().cloned());
        let mut candidates = std::mem::take(&mut self.unresolved);
        candidates.extend(evicted.into_iter().flatten());
        candidates.retain(|name| !keep.contains(name));
        PrunePlan { candidates, keep }
    }

    #[cfg(test)]
    fn retained_len(&self) -> usize {
        self.retained.len()
    }
}

impl IslandsCompanionLedger {
    pub(crate) fn begin(&mut self) {
        self.baseline_unresolved = self.unresolved.clone();
    }

    pub(crate) fn track_candidate(&mut self, names: &HashSet<String>) {
        self.unresolved.extend(names.iter().cloned());
    }

    pub(crate) fn stage(&mut self, names: HashSet<String>) {
        self.staged = Some(names);
    }

    pub(crate) fn abort_candidate(&mut self) -> HashSet<String> {
        self.staged = None;
        let mut protected = self.live.clone();
        protected.extend(self.retained.iter().flatten().cloned());
        let candidates = self
            .unresolved
            .difference(&self.baseline_unresolved)
            .filter(|name| !protected.contains(*name))
            .cloned()
            .collect();
        self.unresolved = self.baseline_unresolved.clone();
        candidates
    }

    pub(crate) fn commit(&mut self, preserve_lazy: bool) -> PrunePlan {
        if let Some(next) = self.staged.take() {
            let previous = std::mem::replace(&mut self.live, next);
            if !previous.is_empty() && previous != self.live {
                self.retained.push_front(previous);
            }
        }

        if preserve_lazy {
            self.unresolved.extend(self.live.iter().cloned());
            self.unresolved
                .extend(self.retained.iter().flatten().cloned());
            return PrunePlan {
                candidates: HashSet::new(),
                keep: HashSet::new(),
            };
        }

        let evicted = self
            .retained
            .split_off(self.retained.len().min(RETAINED_ISLANDS_GENERATIONS));
        let mut keep = self.live.clone();
        keep.extend(self.retained.iter().flatten().cloned());
        let mut candidates = std::mem::take(&mut self.unresolved);
        candidates.extend(evicted.into_iter().flatten());
        PrunePlan { candidates, keep }
    }

    #[cfg(test)]
    pub(crate) fn retained_len(&self) -> usize {
        self.retained.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientScriptCompanionLedger, IslandsCompanionLedger};
    use std::collections::HashSet;

    fn names(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn commit(ledger: &mut IslandsCompanionLedger, values: &[&str]) {
        ledger.track_candidate(&names(values));
        ledger.stage(names(values));
        let _ = ledger.commit(false);
    }

    fn commit_clients(
        ledger: &mut ClientScriptCompanionLedger,
        values: &[&str],
        changed: bool,
    ) -> super::PrunePlan {
        let values = names(values);
        ledger.track_candidate(&values);
        ledger.stage(values, changed);
        ledger.commit()
    }

    #[test]
    fn client_order_to_empty_to_cleanup_retires_one_changed_generation() {
        let mut ledger = ClientScriptCompanionLedger::default();
        let _ = commit_clients(&mut ledger, &["order.js"], true);

        let removal = commit_clients(&mut ledger, &[], false);
        assert!(removal.keep.contains("order.js"));
        assert!(!removal.candidates.contains("order.js"));
        assert_eq!(ledger.retained_len(), 1, "retained registry is populated");
        assert!(ledger.protected_filenames().contains("order.js"));

        let cleanup = commit_clients(&mut ledger, &["cleanup.js"], true);
        assert!(cleanup.candidates.contains("order.js"));
        assert!(!cleanup.keep.contains("order.js"));
        assert!(cleanup.keep.contains("cleanup.js"));
        assert!(!cleanup.candidates.contains("cleanup.js"));
    }

    #[test]
    fn client_changed_identical_generation_advances_retention_age() {
        let mut ledger = ClientScriptCompanionLedger::default();
        let _ = commit_clients(&mut ledger, &["A"], true);
        let _ = commit_clients(&mut ledger, &["B"], true);

        let plan = commit_clients(&mut ledger, &["B"], true);

        assert!(plan.candidates.contains("A"));
        assert!(plan.keep.contains("B"));
        assert!(!plan.candidates.contains("B"));
    }

    #[test]
    fn client_unchanged_identical_generation_does_not_stage_age_or_prune() {
        let mut ledger = ClientScriptCompanionLedger::default();
        let _ = commit_clients(&mut ledger, &["A"], true);
        let _ = commit_clients(&mut ledger, &["B"], true);

        let plan = commit_clients(&mut ledger, &["B"], false);

        assert!(plan.candidates.is_empty());
        assert!(plan.keep.is_empty());
        assert_eq!(ledger.retained_len(), 1);
        assert!(ledger.protected_filenames().contains("A"));
    }

    #[test]
    fn client_raw_unchanged_complete_set_transition_initializes_live_protection() {
        let mut ledger = ClientScriptCompanionLedger::default();

        let plan = commit_clients(&mut ledger, &["boot.js"], false);

        assert!(plan.keep.contains("boot.js"));
        assert_eq!(ledger.retained_len(), 1);
        assert!(ledger.protected_filenames().contains("boot.js"));
    }

    #[test]
    fn client_document_and_lazy_repair_commits_do_not_age_without_stage() {
        let mut ledger = ClientScriptCompanionLedger::default();
        let _ = commit_clients(&mut ledger, &["A"], true);
        let _ = commit_clients(&mut ledger, &["B"], true);

        for _ in 0..2 {
            let plan = ledger.commit();
            assert!(plan.candidates.is_empty());
            assert!(plan.keep.is_empty());
        }
        assert_eq!(ledger.retained_len(), 1);
        assert!(ledger.protected_filenames().contains("A"));
    }

    #[test]
    fn client_abort_discards_stage_and_restores_begin_baseline() {
        let mut ledger = ClientScriptCompanionLedger::default();
        let _ = commit_clients(&mut ledger, &["A"], true);
        ledger.track_candidate(&names(&["baseline.js"]));
        ledger.begin();
        ledger.track_candidate(&names(&["A", "new.js"]));
        ledger.stage(names(&["new.js"]), true);

        assert_eq!(ledger.abort_candidate(), names(&["new.js"]));
        assert!(ledger.protected_filenames().contains("baseline.js"));
        assert!(!ledger.protected_filenames().contains("new.js"));
        let plan = ledger.commit();
        assert!(plan.candidates.is_empty());
        assert!(ledger.protected_filenames().contains("A"));
    }

    #[test]
    fn client_failed_unstaged_and_rollback_uncertain_outputs_stay_protected() {
        let mut ledger = ClientScriptCompanionLedger::default();
        let _ = commit_clients(&mut ledger, &["A"], true);
        let _ = commit_clients(&mut ledger, &["B"], true);
        ledger.begin();
        ledger.track_candidate(&names(&["rollback-uncertain.js"]));

        let failed = ledger.commit();
        assert!(failed.candidates.is_empty());
        assert!(failed.keep.is_empty());
        assert_eq!(ledger.retained_len(), 1);
        assert!(ledger
            .protected_filenames()
            .contains("rollback-uncertain.js"));
        assert!(ledger.protected_filenames().contains("A"));

        let repaired = commit_clients(&mut ledger, &["C"], true);
        assert!(repaired.candidates.contains("rollback-uncertain.js"));
        assert!(repaired.candidates.contains("A"));
        assert!(repaired.keep.contains("C"));
    }

    #[test]
    fn client_blocked_terminal_preserves_stage_without_advancing_until_commit() {
        let mut ledger = ClientScriptCompanionLedger::default();
        let _ = commit_clients(&mut ledger, &["A"], true);
        ledger.begin();
        ledger.track_candidate(&names(&["B"]));
        ledger.stage(names(&["B"]), true);

        // A blocked publication terminal does not call `commit`: the stage
        // stays recoverable but the retained generation does not age yet.
        assert_eq!(ledger.retained_len(), 1);
        assert!(ledger.protected_filenames().contains("A"));
        assert!(ledger.protected_filenames().contains("B"));

        let recovered = ledger.commit();

        assert!(recovered.keep.contains("A"));
        assert!(recovered.keep.contains("B"));
        assert_eq!(ledger.retained_len(), 1);
    }

    #[test]
    fn document_only_commit_keeps_retained_generation() {
        let mut ledger = IslandsCompanionLedger::default();
        commit(&mut ledger, &["A"]);
        commit(&mut ledger, &["B"]);

        let plan = ledger.commit(false);

        assert!(plan.keep.contains("A"));
        assert!(!plan.candidates.contains("A"));
    }

    #[test]
    fn retention_is_bounded_by_islands_generations() {
        let mut ledger = IslandsCompanionLedger::default();
        commit(&mut ledger, &["A"]);
        commit(&mut ledger, &["B"]);
        let third = {
            ledger.track_candidate(&names(&["C"]));
            ledger.stage(names(&["C"]));
            ledger.commit(false)
        };
        assert!(third.keep.contains("A"));

        ledger.track_candidate(&names(&["D"]));
        ledger.stage(names(&["D"]));
        let fourth = ledger.commit(false);
        assert!(fourth.candidates.contains("A"));
        assert!(!fourth.keep.contains("A"));
        assert!(names(&["D", "C", "B"]).is_subset(&fourth.keep));
    }

    #[test]
    fn identical_rebundle_does_not_shift_retention_window() {
        let mut ledger = IslandsCompanionLedger::default();
        commit(&mut ledger, &["A"]);
        commit(&mut ledger, &["B"]);
        commit(&mut ledger, &["B"]);
        commit(&mut ledger, &["B"]);

        let plan = ledger.commit(false);
        assert!(plan.keep.contains("A"));
        assert_eq!(ledger.retained_len(), 1);
    }

    #[test]
    fn empty_previous_generation_is_not_retained() {
        let mut ledger = IslandsCompanionLedger::default();
        commit(&mut ledger, &["A"]);
        assert_eq!(ledger.retained_len(), 0);
    }

    #[test]
    fn lazy_preservation_accumulates_then_trims() {
        let mut ledger = IslandsCompanionLedger::default();
        commit(&mut ledger, &["A"]);
        for value in ["B", "C"] {
            ledger.track_candidate(&names(&[value]));
            ledger.stage(names(&[value]));
            let plan = ledger.commit(true);
            assert!(plan.candidates.is_empty());
            assert!(plan.keep.is_empty());
        }
        assert_eq!(ledger.retained_len(), 2);

        ledger.track_candidate(&names(&["D"]));
        ledger.stage(names(&["D"]));
        let plan = ledger.commit(false);
        assert!(plan.candidates.contains("A"));
        assert!(names(&["D", "C", "B"]).is_subset(&plan.keep));
    }

    #[test]
    fn abort_only_returns_unprotected_new_candidates() {
        let mut ledger = IslandsCompanionLedger::default();
        commit(&mut ledger, &["A"]);
        commit(&mut ledger, &["B"]);
        ledger.begin();
        ledger.track_candidate(&names(&["X", "A", "B"]));

        assert_eq!(ledger.abort_candidate(), names(&["X"]));
        assert!(ledger.abort_candidate().is_empty());
    }

    #[test]
    fn removing_islands_retains_then_eventually_evicts_previous_generation() {
        let mut ledger = IslandsCompanionLedger::default();
        commit(&mut ledger, &["A"]);
        commit(&mut ledger, &[]);
        assert_eq!(ledger.retained_len(), 1);
        commit(&mut ledger, &["B"]);
        commit(&mut ledger, &["C"]);
        ledger.track_candidate(&names(&["D"]));
        ledger.stage(names(&["D"]));
        let plan = ledger.commit(false);

        assert!(plan.candidates.contains("A"));
        assert!(!plan.keep.contains("A"));
    }

    #[test]
    fn abort_clears_staged_generation() {
        let mut ledger = IslandsCompanionLedger::default();
        commit(&mut ledger, &["A"]);
        ledger.begin();
        ledger.track_candidate(&names(&["X"]));
        ledger.stage(names(&["X"]));
        assert_eq!(ledger.abort_candidate(), names(&["X"]));

        let plan = ledger.commit(false);
        assert!(plan.keep.contains("A"));
        assert!(!plan.keep.contains("X"));
    }

    #[test]
    fn boot_candidate_tracked_before_begin_is_in_abort_baseline() {
        let mut ledger = IslandsCompanionLedger::default();
        ledger.track_candidate(&names(&["A"]));
        ledger.stage(names(&["A"]));
        ledger.begin();

        assert!(ledger.abort_candidate().is_empty());
    }
}
