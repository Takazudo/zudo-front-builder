//! Esbuild-shaped `external` marking for chunk imports. At the pinned Rolldown
//! revision `chunk.imports` / `chunk.dynamic_imports` mix emitted chunk
//! filenames with kept-external module ids, so membership in the emitted
//! filename set decides which is which (never filesystem existence or `./`
//! stripping).
use super::copy_reference;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) fn import_entry(
    id: &str,
    kind: &str,
    own_filename: &str,
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
    // The API carries no external provenance, but a chunk never imports itself:
    // its own filename here is an external id spelled like the chunk (#4471).
    if id == own_filename {
        bail!("external import {id:?} has the same id as the emitted chunk that imports it; the metafile cannot tell them apart");
    }
    Ok(json!({"path":outdir.join(id),"kind":kind,"external":false}))
}
