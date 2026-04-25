//! `meta` export handling — STUB.
//!
//! Sub 6 fills this in. Expected responsibilities:
//! - Read `export const meta` from a page module via `RenderHost::get_export`.
//! - Resolve `meta.layout` against the project's layouts directory.
//! - Pass the resolved meta + layout pair into the page's render call.
//! - Handle missing `meta` gracefully (default-layout convention).

use serde::{Deserialize, Serialize};

/// Shape of the optional `meta` export. Sub 6 will lock the final schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageMeta {
    /// Optional layout specifier (e.g. `"@/layouts/blog"`).
    #[serde(default)]
    pub layout: Option<String>,
    /// Free-form additional fields the user wants to expose to layouts.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Placeholder. Sub 6 will replace this with a real implementation that
/// pulls `meta` off the loaded module via the `RenderHost`.
pub fn extract_meta(_specifier: &str) -> Option<PageMeta> {
    None
}
