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

    /// Two source files have the same segment-kind shape but different
    /// parameter names (e.g. `docs/[a].tsx` vs `docs/[b].tsx`, or
    /// `docs/[...a].tsx` vs `docs/[...b].tsx`). They match exactly the same
    /// set of URLs regardless of how their params are named, so one would
    /// silently shadow the other via an arbitrary tiebreak. Keep one.
    #[error(
        "ambiguous route shape {shape:?}: {first} and {second} differ only in parameter names \
         and match the same URLs; rename one to a distinct path or remove it",
        first = first.display(),
        second = second.display(),
    )]
    AmbiguousShape {
        shape: String,
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

    /// A page route starts with the literal `__paths__` segment and has at
    /// least one more segment (`pages/__paths__/foo.tsx` → `/__paths__/foo`).
    /// `/__paths__/<route-key>` is the synthetic endpoint the build pipeline
    /// uses to evaluate `paths()` exports (`@takazudo/zfb-runtime`'s
    /// `createPageRouter` registers `/__paths__/:routeKey{.+}` before every
    /// user page), so such a page would never be served for GET/HEAD. A page
    /// at exactly `/__paths__`, or under a different first segment, is fine.
    #[error(
        "reserved route {template:?} in {path}: the `/__paths__/` prefix is reserved for \
         zfb's internal paths() enumeration endpoint, so this page would never be served; \
         move it to a different top-level directory",
        path = path.display(),
    )]
    ReservedRoutePrefix { path: PathBuf, template: String },
}
