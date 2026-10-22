//! Esbuild never lets a chunk import an entry, and zfb renames entries after
//! bundling, so a split build must not keep shared code in an entry chunk.
//! The packed md-wasm browser lane (`health.yml`, wasm-md browser) is the
//! end-to-end proof; this pins the decoded option it depends on.
use super::prepared::Request;
use std::process::Command;

#[test]
fn split_builds_do_not_fold_shared_code_into_an_already_loaded_entry() {
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(std::env::temp_dir()).args([
        "entry.js",
        "--bundle",
        "--format=esm",
        "--splitting",
        "--outdir=out",
    ]);
    let request = Request::decode(&cmd).unwrap();
    let optimization = request
        .options
        .experimental
        .and_then(|experimental| experimental.chunk_optimization)
        .expect("split builds configure chunk optimization");
    assert!(!optimization.is_avoid_redundant_chunk_loads_enabled());
    assert!(optimization.is_merge_common_chunks_enabled());
}
