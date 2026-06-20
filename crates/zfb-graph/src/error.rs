//! Crate-wide error type.
//!
//! The graph itself is a pure in-memory data structure, but the
//! persistence layer (see [`crate::persist`]) performs IO and
//! (de)serialisation, so the error surface covers those failure modes.

/// Errors produced by `zfb-graph`.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// Filesystem IO failed while persisting or loading the graph.
    #[error("graph persistence IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Binary (de)serialisation failed.
    #[error("graph persistence codec error: {0}")]
    Codec(#[from] bincode::Error),
}
