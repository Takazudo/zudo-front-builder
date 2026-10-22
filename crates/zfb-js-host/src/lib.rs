//! ZFB-owned host types, services and shims. No engine is enabled by default.
#[cfg(feature = "services")]
pub mod crypto;
pub mod dispatch;
pub mod dispatch_mode;
#[cfg(feature = "services")]
pub mod fetch;
#[cfg(feature = "services")]
pub mod limits;
/// Loopback HTTP fixture shared by this crate's transport tests and, via
/// `#[path]`, by `zfb-render`'s direct-V8 integration tests.
#[cfg(all(test, feature = "services"))]
#[allow(dead_code)]
pub(crate) mod loopback_test_server;
#[cfg(feature = "services")]
pub mod modules;
#[cfg(feature = "direct-v8")]
pub mod native;
pub mod shims;

#[cfg(feature = "direct-v8")]
pub mod process;
