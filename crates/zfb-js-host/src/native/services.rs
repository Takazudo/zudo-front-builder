//! Owned operation state. No V8 local handle or borrowed buffer lives in a future.
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use crate::crypto::{EntropySource, OsEntropy};
use crate::dispatch_mode::DispatchMode;
use crate::fetch::{build_fetch_client, FetchConfig, FetchError, FetchOutcome, SubrequestCounter};

pub(super) type Transport = Pin<Box<dyn Future<Output = Result<FetchOutcome, FetchError>>>>;
pub(super) struct PendingFetch {
    pub id: u64,
    pub url: String,
    pub cancel_id: Option<u32>,
    pub resolver: v8::Global<v8::PromiseResolver>,
    pub transport: Transport,
}

pub(super) struct Services {
    pub mode: DispatchMode,
    pub budget: Rc<SubrequestCounter>,
    pub config: FetchConfig,
    pub client: Option<reqwest::Client>,
    pub entropy: Rc<dyn EntropySource>,
    pub pending: Vec<PendingFetch>,
    pub cancellations: HashMap<u32, u64>,
    pub next_id: u64,
}
impl Services {
    pub fn new(config: FetchConfig, entropy: Rc<dyn EntropySource>) -> Self {
        Self {
            mode: DispatchMode::BuildTime,
            budget: Rc::new(SubrequestCounter::new()),
            config,
            client: build_fetch_client().ok(),
            entropy,
            pending: Vec::new(),
            cancellations: HashMap::new(),
            next_id: 0,
        }
    }
    pub fn defaults() -> Self {
        Self::new(FetchConfig::default(), Rc::new(OsEntropy))
    }
    pub fn begin_dispatch(&mut self, mode: DispatchMode) {
        self.mode = mode;
        self.budget = Rc::new(SubrequestCounter::new());
    }
    pub fn remove_cancelled(&mut self, cancel_id: u32) -> Option<PendingFetch> {
        let id = self.cancellations.remove(&cancel_id)?;
        let index = self
            .pending
            .iter()
            .position(|operation| operation.id == id)?;
        Some(self.pending.swap_remove(index))
    }
    pub fn forget(&mut self, operation: &PendingFetch) {
        if let Some(cancel_id) = operation.cancel_id {
            if self.cancellations.get(&cancel_id) == Some(&operation.id) {
                self.cancellations.remove(&cancel_id);
            }
        }
    }
}

pub(super) fn state(scope: &v8::PinScope) -> Option<Rc<RefCell<Services>>> {
    scope.get_slot::<Rc<RefCell<Services>>>().cloned()
}
