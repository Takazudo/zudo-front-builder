//! Internal #318 Rolldown adapter, the default bundler when `native` is compiled in.
//! No Rolldown types escape.
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::{enabled, run_prepared};
