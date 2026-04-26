//! Core route data model.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A single segment of a route template, parsed from a path component.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Segment {
    /// Literal text that must match exactly. e.g. `about` in `/about`.
    Static(String),
    /// Single-segment dynamic parameter from `[name].tsx`. The string is the
    /// parameter name (no brackets).
    Dynamic(String),
    /// Catchall (rest) parameter from `[...name].tsx`. Matches one or more
    /// trailing segments. Only allowed as the final segment of a route.
    Catchall(String),
}

impl Segment {
    /// Render this segment using the canonical `:name` / `:name*` template
    /// syntax (Astro / Express style). Used for ambiguity detection and for
    /// human-readable diagnostics.
    pub fn template(&self) -> String {
        match self {
            Segment::Static(s) => s.clone(),
            Segment::Dynamic(name) => format!(":{name}"),
            Segment::Catchall(name) => format!(":{name}*"),
        }
    }
}

/// The kind of a route, used for sorting (static beats dynamic beats catchall).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RouteKind {
    Static,
    Dynamic,
    Catchall,
}

impl RouteKind {
    /// Lower numbers sort first (i.e. higher priority).
    pub(crate) fn order_key(self) -> u8 {
        match self {
            RouteKind::Static => 0,
            RouteKind::Dynamic => 1,
            RouteKind::Catchall => 2,
        }
    }
}

/// A single resolved route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    /// The source `.tsx` file this route was derived from.
    pub source_path: PathBuf,
    /// Parsed segments (excluding the leading `/`). An empty vec corresponds to
    /// the index route `/`.
    pub segments: Vec<Segment>,
    /// Coarse kind used for sorting and downstream dispatch.
    pub kind: RouteKind,
    /// Specificity score: higher is more specific. See [`crate::scan`] for the
    /// exact formula. Stable across versions only in relative terms.
    pub specificity: u32,
}

impl Route {
    /// Render the route as a `/`-separated template, e.g. `/blog/:slug` or
    /// `/docs/:slug*`. The empty (index) route renders as `/`.
    pub fn template(&self) -> String {
        if self.segments.is_empty() {
            return "/".to_string();
        }
        let mut out = String::new();
        for seg in &self.segments {
            out.push('/');
            out.push_str(&seg.template());
        }
        out
    }

}
