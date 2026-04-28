//! Canonical `Segment` type for route templates.
//!
//! Both `zfb-router` and `zfb-render` previously carried their own parallel
//! definitions of `Segment`. This module holds the single canonical type;
//! both crates re-export from here.
//!
//! The `zfb-render::paths::Segment` stub carried a note saying it was
//! "temporary" and would be replaced at merge time — this is that merge.

use serde::{Deserialize, Serialize};

/// A single segment of a route template, parsed from a path component.
///
/// Variants:
/// - `Static("about")` — literal segment, must match exactly.
/// - `Dynamic("slug")` — single-segment dynamic param from `[slug].tsx`.
/// - `Catchall("slug")` — rest param from `[...slug].tsx`; matches one or
///   more trailing segments.
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
    /// Render this segment using the canonical `:name` / `:name{.+}` template
    /// syntax (Hono path-pattern style). Used for ambiguity detection,
    /// `/__paths__/` worker lookups, and human-readable diagnostics.
    ///
    /// Catchall segments emit Hono's regex-quantifier form `:name{.+}`
    /// rather than the Astro / Express `:name*` style. This keeps the
    /// route-key produced by `Route::template()` and the route-key
    /// registered by `bracket_to_hono` (in zfb-build) bit-identical, so
    /// the worker's `pagesByRoute` map can be looked up from the
    /// build/dev side without a separate translation step.
    pub fn template(&self) -> String {
        match self {
            Segment::Static(s) => s.clone(),
            Segment::Dynamic(name) => format!(":{name}"),
            Segment::Catchall(name) => format!(":{name}{{.+}}"),
        }
    }
}
