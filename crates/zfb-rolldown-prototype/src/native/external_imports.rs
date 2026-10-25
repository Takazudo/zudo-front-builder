//! Esbuild-shaped `external` marking for chunk imports. At the pinned Rolldown
//! revision `chunk.imports` / `chunk.dynamic_imports` mix emitted chunk
//! filenames with kept-external module ids, so membership in the emitted
//! filename set decides which is which (never filesystem existence or `./`
//! stripping).
//!
//! They never list an emitted asset (#4505). At rolldown rev 24bc2d0b,
//! `crates/rolldown/src/utils/chunk/finalize_chunks.rs` `finalize_assets` fills
//! them from the instances of `cross_chunk_imports` / `cross_chunk_dynamic_imports`
//! plus `ExternalModule::get_file_name` of the chunk's external modules. Those
//! instances come only from `EcmaGenerator::instantiate_chunk`
//! (`render_chunk_to_assets.rs` `instantiate_chunks`), so every one is an ECMA
//! chunk; sourcemaps and `emit_file` assets are added without a chunk link.
//!
//! Every edge kind (input imports, input copy edges, output chunk/external and
//! copy-reference imports) uses one `external` shape, built by [`edge`] (#4506).
use super::copy_reference;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Like [`import_entry`], but fails loudly when a kept-external id is spelled
/// like an emitted asset: by the invariant above Rolldown never lists an asset
/// here, so such an id would be a bare external for a file in the outdir.
pub(super) fn checked_import_entry(
    id: &str,
    kind: &str,
    emitted: &BTreeSet<String>,
    emitted_assets: &BTreeSet<String>,
    copy_outputs: &BTreeMap<String, String>,
    outdir: &Path,
) -> Result<Value> {
    let entry = import_entry(id, kind, emitted, copy_outputs, outdir)?;
    if entry["external"] == true && emitted_assets.contains(id) {
        bail!("external import {id} has the same name as an emitted asset");
    }
    Ok(entry)
}

/// Esbuild v0.25.12's metafile writers emit `"external": true` on an external
/// edge and omit the field on every other edge; none writes `false`.
/// Input imports: `internal/bundler/bundler.go` (`processScannedFiles`) writes
/// `"external": true` for a record with no bundled source and `"original"` without it for
/// a resolved one, before a copy-loader target moves to `CopySourceIndex`, so an
/// input copy edge is a plain internal edge. Output imports:
/// `js_printer.go` `printPath` / `css_printer.go` `recordImportPathForMetafile`
/// add it unless `ShouldNotBeExternalInMetafile` is set, which `linker.go` sets on
/// cross-chunk imports, copy-loader paths and external-dynamic-import rewrites.
pub(super) fn edge(path: Value, kind: &str, external: bool) -> Value {
    let mut entry = json!({"path":path,"kind":kind});
    if external {
        entry["external"] = Value::Bool(true);
    }
    entry
}

/// An input-side edge to a Rolldown copy reference: internal, carrying the
/// private output marker that `copy_reference::restore_sources` later replaces.
pub(super) fn copied_input_edge(id: &str, kind: &str, filename: String) -> Value {
    let mut entry = edge(json!(id), kind, false);
    entry["zfbCopiedOutput"] = Value::String(filename);
    entry
}

pub(super) fn import_entry(
    id: &str,
    kind: &str,
    emitted: &BTreeSet<String>,
    copy_outputs: &BTreeMap<String, String>,
    outdir: &Path,
) -> Result<Value> {
    if copy_reference::reference(id)?.is_some() {
        let path = copy_reference::output_name(id, copy_outputs)?;
        return Ok(edge(json!(outdir.join(path)), kind, false));
    }
    if !emitted.contains(id) {
        return Ok(edge(json!(id), kind, true));
    }
    // Rolldown does list a chunk's own filename here (the SSR bundle imports
    // itself), so an emitted name is internal even when it is the importer's own;
    // an external spelled like an emitted chunk is indistinguishable (#4490).
    Ok(edge(json!(outdir.join(id)), kind, false))
}
