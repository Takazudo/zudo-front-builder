//! Canonical shared data types for the zfb framework.
//!
//! This crate holds types that are used by multiple crates within the zfb
//! workspace, preventing code duplication and circular dependencies.

pub mod segment;

pub use segment::Segment;
