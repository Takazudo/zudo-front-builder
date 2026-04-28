//! Crate-wide error type.
//!
//! The graph itself is a pure in-memory data structure, but the
//! persistence layer (see [`crate::persist`]) performs IO and
//! (de)serialisation, so the error surface covers those failure modes.

/// Errors produced by `zfb-graph`.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// Reserved variant. Currently no in-memory operation produces this;
    /// reserved so the enum is non-empty and downstream `match` arms can
    /// be exhaustive once additional fallible APIs are added.
    #[error("internal graph error: {0}")]
    Internal(String),

    /// Filesystem IO failed while persisting or loading the graph.
    #[error("graph persistence IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Binary (de)serialisation failed.
    #[error("graph persistence codec error: {0}")]
    Codec(#[from] bincode::Error),

    /// On-disk file did not start with the expected magic / version
    /// header. Treated as "not a graph file we can read" and surfaced
    /// so callers can decide whether to discard and rebuild.
    #[error("graph persistence: invalid file header (not a zfb-graph snapshot, or version mismatch)")]
    BadHeader,
}
