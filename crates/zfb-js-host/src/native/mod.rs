//! Non-default direct-V8 feasibility host. Every isolate stays on its creator thread.
//!
//! This module is deliberately separate from engine-neutral services. Its public
//! results contain owned Rust data; local V8 handles never cross an await.
mod bootstrap;
mod bridge;
mod config;
mod core;
mod diagnostics;
mod driver;
mod modules;
mod platform;
mod services;
mod values;
mod worker;
mod worker_modules;
pub use worker::ModuleHandle;

pub use config::evaluate_config_bundle;
pub use core::NativeHost;
pub use diagnostics::NativeError;

pub const ENGINE_ID: &str = "direct v8 147.4.0";

#[cfg(test)]
mod tests;

#[cfg(test)]
mod services_tests;

#[cfg(test)]
mod worker_tests;
