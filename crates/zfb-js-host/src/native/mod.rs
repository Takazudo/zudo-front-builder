//! Non-default direct-V8 feasibility host. Every isolate stays on its creator thread.
//!
//! This module is deliberately separate from engine-neutral services. Its public
//! results contain owned Rust data; local V8 handles never cross an await.
mod bootstrap;
mod config;
mod core;
mod diagnostics;
mod modules;

pub use config::evaluate_config_bundle;
pub use core::NativeHost;
pub use diagnostics::NativeError;

pub const ENGINE_ID: &str = "direct v8 147.4.0";

#[cfg(test)]
mod tests;
