//! `paths()` runtime — STUB.
//!
//! Sub 5 fills this in. Expected responsibilities:
//! - For each dynamic page, call its `paths()` export through the host.
//! - Return `[{ params, props }]` rows that `zfb-router` multiplies into
//!   concrete URLs.
//! - Cache results across rebuilds.

use serde::{Deserialize, Serialize};

/// One row of the result `paths()` returns. Sub 5 may extend / replace this
/// with a richer type.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PathRow {
    /// Per-segment parameter bindings (e.g. `{"slug": "hello"}`).
    pub params: serde_json::Map<String, serde_json::Value>,
    /// Optional props to pass to the page render call.
    #[serde(default)]
    pub props: serde_json::Value,
}

/// Placeholder. Sub 5 will replace this with a real implementation that
/// drives the `RenderHost` to invoke a page's `paths()` export.
pub fn resolve_paths(_specifier: &str) -> Vec<PathRow> {
    Vec::new()
}
