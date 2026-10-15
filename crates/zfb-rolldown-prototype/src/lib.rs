//! Internal, explicitly opted-in #318 feasibility adapter. No Rolldown types escape.
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::{enabled, run_prepared};
