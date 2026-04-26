//! Crate-wide error type.
//!
//! The graph itself is a pure in-memory data structure that does not perform
//! IO, so today's error surface is small. The type is reserved as a stable
//! public name so future fallible operations (graph persistence, cycle checks
//! against an external resolver, etc.) can fit without a breaking API change.

/// Errors produced by `zfb-graph`.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// Reserved variant. Currently no operation produces this; reserved so the
    /// enum is non-empty and downstream `match` arms can be exhaustive once
    /// fallible APIs are added (e.g., persistence, cross-resolver validation).
    #[error("internal graph error: {0}")]
    Internal(String),
}
