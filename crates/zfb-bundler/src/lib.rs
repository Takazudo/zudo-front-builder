//! zfb's owned native bundler boundary (#4628): typed requests in, validated
//! bundles and a normalized module graph out. No Rolldown types escape.
#[cfg(feature = "native")]
mod native;
pub mod request;
#[cfg(feature = "native")]
#[doc(hidden)]
pub use native::module_specifiers::module_specifiers;
#[cfg(feature = "native")]
pub use native::{bundle, generate};
pub use request::*;
