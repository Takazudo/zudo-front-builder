//! Canonical shared data types for the zfb framework.
//!
//! This crate holds types that are used by multiple crates within the zfb
//! workspace, preventing code duplication and circular dependencies.

pub mod asset_urls;
pub mod segment;

pub use asset_urls::{
    DIST_ASSETS_DIR, STABLE_ASSETS_URL_PREFIX, STABLE_CSS_FILENAME, STABLE_CSS_URL,
    STABLE_ISLANDS_FILENAME, STABLE_ISLANDS_URL,
};
pub use segment::Segment;
