//! Error types for the router crate.

use std::path::PathBuf;

/// Errors raised while scanning or parsing a pages directory.
#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    /// The pages directory does not exist or is not a directory.
    #[error("pages directory not found: {0}")]
    PagesDirMissing(PathBuf),

    /// I/O failure during directory traversal.
    #[error("io error while scanning {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A `[bracket]` segment was malformed (empty name, missing close bracket, etc.).
    #[error("invalid route segment {segment:?} in {path}: {message}", path = path.display())]
    InvalidSegment {
        path: PathBuf,
        segment: String,
        message: String,
    },

    /// A catchall segment (`[...slug]`) appeared in a position other than the
    /// final path segment.
    #[error(
        "catchall segment {segment:?} in {path} must be the last segment of the route",
        path = path.display(),
    )]
    CatchallNotLast { path: PathBuf, segment: String },

    /// Two source files map to the same canonical route template.
    #[error(
        "ambiguous route {template:?}: produced by both {first} and {second}",
        first = first.display(),
        second = second.display(),
    )]
    AmbiguousRoute {
        template: String,
        first: PathBuf,
        second: PathBuf,
    },

    /// An optional catchall route (`[[...name]]`) overlaps another route:
    /// either both serve the same bare URL (the zero-segment case), or
    /// another catchall occupies the same position (full overlap on every
    /// non-empty path, regardless of param name).
    #[error(
        "conflicting routes {first} and {second}: {reason}",
        first = first.display(),
        second = second.display(),
    )]
    OptionalCatchallConflict {
        first: PathBuf,
        second: PathBuf,
        reason: String,
    },
}
