//! Run the build's island scanner preflight against a real linked package graph.
//!
//! Issue #3648 used zudolab/zudo-doc commit
//! `ac3c4bd8eb11d257b7246815146f4b8f47f598a6`, rebuilt with its pinned pnpm
//! (`install --ignore-scripts --frozen-lockfile`, then `build:workspace`).
//! In a separate project, link that rebuilt package at
//! `node_modules/@takazudo/zudo-doc` and create `pages/index.tsx`:
//!
//! ```tsx
//! import { createChrome } from "@takazudo/zudo-doc/chrome";
//! export default function Page() {
//!   return <html><body>Probe</body></html>;
//! }
//! ```
//!
//! Compile separately so a diagnostic deadline measures scanning, not compilation:
//!
//! ```sh
//! cargo build -p zfb-islands --example package_graph_probe --no-default-features
//! # Linux CI: a timeout is an inconclusive diagnostic cap, never a passing scan.
//! timeout 60s target/debug/examples/package_graph_probe /path/to/project
//! ```
//!
//! On guarded development machines, wrap the build in `heavy-guard.sh -- ...`
//! and the probe in `heavy-guard.sh --max-run 60 -- ...`.
//! The optional second argument selects a different entry relative to the project.
//! This probe uses the default filesystem resolver without config/plugin overrides,
//! matching the issue's minimal project. It exercises scanner preflight only;
//! a full `zfb build` remains a separate integration check.

use std::{path::PathBuf, time::Instant};

use anyhow::{bail, Context, Result};
use zfb_islands::{scan_islands_with_meta_and_first_party_root, FsResolver};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let Some(project) = args.next() else {
        bail!("usage: package_graph_probe PROJECT_ROOT [ENTRY_RELATIVE_TO_ROOT]");
    };
    let entry = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("pages/index.tsx"));
    if args.next().is_some() {
        bail!("usage: package_graph_probe PROJECT_ROOT [ENTRY_RELATIVE_TO_ROOT]");
    }
    let root = std::fs::canonicalize(&project).context("cannot resolve project root")?;
    let entry = std::fs::canonicalize(root.join(entry)).context("cannot resolve page entry")?;
    let resolver = FsResolver::new().with_project_root(&root);
    let started = Instant::now();
    let result = scan_islands_with_meta_and_first_party_root(&[entry], &resolver, Some(&root));
    eprintln!(
        "island-preflight elapsed_ms={}",
        started.elapsed().as_millis()
    );
    let (islands, meta) = result?;
    println!(
        "islands={} island_reachable_modules={}",
        islands.len(),
        meta.island_reachable_modules.len()
    );
    Ok(())
}
