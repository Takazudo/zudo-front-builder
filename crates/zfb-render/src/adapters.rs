//! Framework JSX runtime adapters.
//!
//! The adapter selects which `<x>/jsx-runtime` module backs the JSX transform
//! and which render-to-string entry point is invoked at SSR time.
//!
//! **Status:** stub. Sub 4 fills in the `preact` and `react` adapters and
//! decides what the public API of this module looks like. The submodules are
//! re-declared here so the crate root can `pub use adapters::*` without
//! waiting for Sub 4.

pub mod preact;
pub mod react;

/// Default re-export — currently the `preact` adapter, matching the v1 MVP
/// default (`framework: "preact"` in `zfb.config.ts`).
pub use preact as default_adapter;
