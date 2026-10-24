//! Esbuild-shaped `external` marking for chunk imports. At the pinned Rolldown
//! revision `chunk.imports` / `chunk.dynamic_imports` mix emitted chunk
//! filenames with kept-external module ids, so membership in the emitted
//! filename set decides which is which (never filesystem existence or `./`
//! stripping).
use super::copy_reference;
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) fn import_entry(
    id: &str,
    kind: &str,
    emitted: &BTreeSet<String>,
    copy_outputs: &BTreeMap<String, String>,
    outdir: &Path,
) -> Result<Value> {
    if copy_reference::reference(id)?.is_some() {
        let path = copy_reference::output_name(id, copy_outputs)?;
        return Ok(json!({"path":outdir.join(path),"kind":kind}));
    }
    if !emitted.contains(id) {
        return Ok(json!({"path":id,"kind":kind,"external":true}));
    }
    // Rolldown does list a chunk's own filename here (the SSR bundle imports
    // itself), so an emitted name is internal even when it is the importer's own;
    // an external spelled like an emitted chunk is indistinguishable (#4490).
    Ok(json!({"path":outdir.join(id),"kind":kind,"external":false}))
}
