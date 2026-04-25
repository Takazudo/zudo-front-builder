//! zfb-runtime-spike
//!
//! Bounded spike that backs ADR-001. Defines the [`RenderHost`] trait that
//! every candidate JS runtime must implement, plus per-candidate
//! implementations gated behind feature flags so contributors only pay the
//! V8 compile cost when they explicitly want it.
//!
//! Not part of the production build. The trait shape here is normative for
//! ADR-001 and is the seed for `zfb-render`'s eventual `RenderHost`
//! abstraction.

pub mod host;
pub mod hosts;
pub mod metrics;

pub use host::{RenderHost, RenderInput, RenderOutput};
pub use metrics::{HostMetrics, MetricSample};
