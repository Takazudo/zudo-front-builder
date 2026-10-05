//! Build-owned diagnostic collector. Synchronous producers use a scoped
//! thread-local handle; async plugin readers retain an explicit sink clone.
use std::collections::BTreeSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use crate::build_diagnostics::BuildDiagnostic;

#[derive(Clone, Default)]
pub struct BuildDiagnosticSink(Arc<BuildDiagnosticSinkInner>);

#[derive(Default)]
struct BuildDiagnosticSinkInner {
    diagnostics: Mutex<Vec<BuildDiagnostic>>,
    incomplete: AtomicBool,
    seen_foreign_pragmas: Mutex<BTreeSet<String>>,
}

impl BuildDiagnosticSink {
    /// Reserve a source-location identity for ZB005 within this build.
    /// Other diagnostic classes retain repeated occurrences.
    pub fn reserve_foreign_pragma(&self, key: String) -> bool {
        self.0
            .seen_foreign_pragmas
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key)
    }
    pub fn push(&self, diagnostic: BuildDiagnostic) {
        self.0
            .diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(diagnostic);
    }

    pub fn mark_incomplete(&self) {
        self.0.incomplete.store(true, Ordering::Release);
    }
    pub fn is_complete(&self) -> bool {
        !self.0.incomplete.load(Ordering::Acquire)
    }

    pub fn snapshot_sorted(&self) -> Vec<BuildDiagnostic> {
        let mut diagnostics = self
            .0
            .diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        diagnostics.sort_by(|a, b| {
            (
                &a.file,
                &a.source_id,
                &a.line,
                &a.byte_column,
                &a.code,
                &a.message,
                &a.severity,
            )
                .cmp(&(
                    &b.file,
                    &b.source_id,
                    &b.line,
                    &b.byte_column,
                    &b.code,
                    &b.message,
                    &b.severity,
                ))
        });
        diagnostics
    }
}

thread_local! {
    static ACTIVE: std::cell::RefCell<Option<BuildDiagnosticSink>> = const { std::cell::RefCell::new(None) };
}

/// Scope synchronous producer work to its owning build. Nested calls restore
/// the previous context, so independent builds on other threads cannot leak.
pub fn with_sink<T>(sink: &BuildDiagnosticSink, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<BuildDiagnosticSink>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTIVE.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let previous = ACTIVE.with(|slot| slot.replace(Some(sink.clone())));
    let _restore = Restore(previous);
    f()
}

pub fn emit(diagnostic: BuildDiagnostic) {
    eprintln!("{}", diagnostic.render());
    ACTIVE.with(|slot| {
        if let Some(sink) = slot.borrow().as_ref() {
            sink.push(diagnostic);
        }
    });
}

pub fn record(diagnostic: BuildDiagnostic) {
    ACTIVE.with(|slot| {
        if let Some(sink) = slot.borrow().as_ref() {
            sink.push(diagnostic);
        }
    });
}

pub fn in_scope() -> bool {
    ACTIVE.with(|slot| slot.borrow().is_some())
}

pub fn reserve_foreign_pragma(key: String) -> Option<bool> {
    ACTIVE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|sink| sink.reserve_foreign_pragma(key))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_diagnostics::{BuildDiagnostic, DiagnosticSeverity};

    fn warning(message: &str) -> BuildDiagnostic {
        BuildDiagnostic::new("ZB031", DiagnosticSeverity::Warning, message)
    }

    #[test]
    fn nested_scopes_restore_owner_and_keep_repeats() {
        let outer = BuildDiagnosticSink::default();
        let inner = BuildDiagnosticSink::default();
        with_sink(&outer, || {
            record(warning("outer"));
            with_sink(&inner, || record(warning("inner")));
            record(warning("outer"));
        });
        record(warning("unowned"));
        assert_eq!(outer.snapshot_sorted().len(), 2);
        assert_eq!(inner.snapshot_sorted().len(), 1);
    }

    #[test]
    fn panicking_nested_scope_restores_previous_owner() {
        let outer = BuildDiagnosticSink::default();
        let inner = BuildDiagnosticSink::default();
        with_sink(&outer, || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                with_sink(&inner, || {
                    record(warning("inner"));
                    panic!("test unwind");
                });
            }));
            assert!(result.is_err());
            record(warning("outer after unwind"));
        });
        assert_eq!(inner.snapshot_sorted()[0].message, "inner");
        assert_eq!(outer.snapshot_sorted()[0].message, "outer after unwind");
        assert!(!in_scope());
    }

    #[test]
    fn concurrent_threads_have_independent_scopes() {
        let sinks = [
            BuildDiagnosticSink::default(),
            BuildDiagnosticSink::default(),
        ];
        std::thread::scope(|scope| {
            for (i, sink) in sinks.iter().enumerate() {
                scope.spawn(move || with_sink(sink, || record(warning(&i.to_string()))));
            }
        });
        assert_eq!(sinks[0].snapshot_sorted()[0].message, "0");
        assert_eq!(sinks[1].snapshot_sorted()[0].message, "1");
    }

    #[test]
    fn owned_clone_survives_scope_exit() {
        let sink = BuildDiagnosticSink::default();
        let reader_owner = with_sink(&sink, || sink.clone());
        std::thread::spawn(move || reader_owner.push(warning("late")))
            .join()
            .unwrap();
        assert_eq!(sink.snapshot_sorted()[0].message, "late");
    }

    #[test]
    fn foreign_pragma_reservation_is_per_build_and_does_not_dedupe_other_codes() {
        let first = BuildDiagnosticSink::default();
        let second = BuildDiagnosticSink::default();
        with_sink(&first, || {
            assert_eq!(reserve_foreign_pragma("src/a.tsx:2:4".into()), Some(true));
            assert_eq!(reserve_foreign_pragma("src/a.tsx:2:4".into()), Some(false));
            record(warning("same"));
            record(warning("same"));
        });
        with_sink(&second, || {
            assert_eq!(reserve_foreign_pragma("src/a.tsx:2:4".into()), Some(true))
        });
        assert_eq!(first.snapshot_sorted().len(), 2);
    }
}
