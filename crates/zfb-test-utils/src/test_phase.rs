use std::panic::Location;
use std::time::Instant;

/// Begin/end receipts survive a runner killing a test inside a synchronous
/// OS call. They measure phases, not product deadlines or successful outcomes.
#[derive(Debug)]
pub struct TestPhase {
    label: String,
    caller: &'static Location<'static>,
    start: Instant,
}

impl TestPhase {
    #[track_caller]
    pub fn new(label: impl Into<String>) -> Self {
        let phase = Self {
            label: label.into(),
            caller: Location::caller(),
            start: Instant::now(),
        };
        eprintln!("[test-phase] begin {} at {}", phase.label, phase.caller);
        phase
    }
}

impl Drop for TestPhase {
    fn drop(&mut self) {
        eprintln!(
            "[test-phase] end {} at {} elapsed={:?} unwinding={}",
            self.label,
            self.caller,
            self.start.elapsed(),
            std::thread::panicking(),
        );
    }
}
