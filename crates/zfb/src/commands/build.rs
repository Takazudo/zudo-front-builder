//! `zfb build` command — one-shot production build.
//!
//! Contract:
//!   pub async fn run(args: &crate::cli::BuildArgs) -> anyhow::Result<()>
//!
//! `args.outdir` is the production output directory (default `dist`).
//! Resolved relative to the current working directory if not absolute.
//!
//! ## v1 scope-down (Wave 2 / Sub 5)
//!
//! At the time this command was wired up, the surrounding pieces of the
//! production build pipeline are not yet in place:
//!
//! - `zfb-render` / `zfb-content` / `zfb-router` (Epic 3) is not connected
//!   into the bin crate, so we have no real `PageRenderer` to plug into
//!   the `zfb_build::BuildOrchestrator`.
//! - The dependency-graph loader (the thing that populates
//!   `zfb_graph::DependencyGraph` from project sources) has not landed,
//!   so the orchestrator's `graph.pages()` would be empty and a one-shot
//!   `tick` against a "full rebuild" plan resolves to zero work.
//! - `crate::config::load_from_dir` is owned by Wave 2 / Sub 3 and merges
//!   in after this branch.
//!
//! Per the cmd-build brief, v1 is allowed to ship a stub that:
//!
//! 1. validates we're inside something that looks like a project root
//!    (presence of a `pages/` directory),
//! 2. atomically writes a stub `<outdir>/index.html` so `zfb preview` has
//!    something to serve, and
//! 3. prints the canonical summary line `✓ N pages built in Xs`.
//!
//! The real renderer wires in once Epic 3 + the graph loader land. This
//! file's `run` signature, the project-root validation, and the summary
//! contract should remain stable across that swap.

use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use zfb_build::atomic_write_string;

use crate::cli::BuildArgs;

pub async fn run(args: &BuildArgs) -> Result<()> {
    let started = Instant::now();

    let project_root = env::current_dir().context("failed to read current working directory")?;

    // v1 project-root sanity check. The eventual config loader will give
    // us a richer "is this a zfb project?" signal; until then, the
    // presence of a `pages/` directory is the cheapest heuristic that
    // matches the framework's vocabulary (see zfb-build's default watch
    // roots, which include `pages`).
    let pages_dir = project_root.join("pages");
    if !pages_dir.is_dir() {
        return Err(anyhow!(
            "no `pages/` directory found in {}; run `zfb build` from a project root",
            project_root.display()
        ));
    }

    let outdir = resolve_outdir(&project_root, &args.outdir);

    // v1: emit a single stub index.html. When the real renderer wires in,
    // this loop becomes "for each rendered page, atomic_write_string the
    // HTML to <outdir>/<output_path>".
    let index_path = outdir.join("index.html");
    let stub_html = stub_index_html();
    atomic_write_string(&index_path, &stub_html)
        .with_context(|| format!("failed to write {}", index_path.display()))?;

    // v1 always emits one page (the stub index). Real builds will
    // increment per RenderedPage.
    let pages_built: usize = 1;
    let elapsed = started.elapsed().as_secs_f64();
    println!("✓ {pages_built} pages built in {elapsed:.2}s");

    Ok(())
}

/// Resolve `outdir` against `project_root`. If `outdir` is absolute it is
/// used as-is; if relative it is joined onto `project_root`. The result is
/// returned without canonicalising — the directory may not exist yet, and
/// `atomic_write_string` will create parents on demand.
fn resolve_outdir(project_root: &Path, outdir: &Path) -> PathBuf {
    if outdir.is_absolute() {
        outdir.to_path_buf()
    } else {
        project_root.join(outdir)
    }
}

/// Stub HTML emitted by the v1 build so `zfb preview` has something to
/// serve. Intentionally minimal — once the real renderer lands this
/// function disappears.
fn stub_index_html() -> String {
    "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>zfb build</title>\n</head>\n<body>\n<h1>zfb build (v1 stub)</h1>\n<p>The production renderer is not yet wired in. This page is a placeholder so <code>zfb preview</code> has something to serve.</p>\n</body>\n</html>\n".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_outdir_keeps_absolute_paths() {
        let root = Path::new("/proj");
        let abs = PathBuf::from("/tmp/zfb-out");
        assert_eq!(resolve_outdir(root, &abs), abs);
    }

    #[test]
    fn resolve_outdir_joins_relative_paths_onto_root() {
        let root = Path::new("/proj");
        let rel = PathBuf::from("dist");
        assert_eq!(resolve_outdir(root, &rel), PathBuf::from("/proj/dist"));
    }

    #[test]
    fn stub_html_is_well_formed_html() {
        let html = stub_index_html();
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("zfb build"));
        assert!(html.trim_end().ends_with("</html>"));
    }
}
