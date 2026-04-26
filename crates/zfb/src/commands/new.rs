//! `zfb new` command — scaffold a new project from an embedded template.
//!
//! The contents of `crates/zfb/templates/` are baked into the binary at compile
//! time via [`include_dir`]. At runtime we walk the requested template subtree
//! (defaulting to `default`) and write each file into the destination directory.
//! After the files are in place we attempt to run `pnpm install`; if pnpm is
//! missing we print a friendly notice and continue successfully so the user can
//! run install themselves later.

use std::io::ErrorKind;
use std::path::Path;

use include_dir::{include_dir, Dir, DirEntry};
use std::fs;
use tokio::process::Command;

use crate::cli::NewArgs;

/// Compile-time embedding of `crates/zfb/templates/`.
///
/// Each top-level subdirectory is a template selectable via `--template`.
static TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");

pub async fn run(args: &NewArgs) -> anyhow::Result<()> {
    let template = TEMPLATES.get_dir(args.template.as_str()).ok_or_else(|| {
        let available = available_templates();
        anyhow::anyhow!(
            "unknown template '{}'. Available templates: {}",
            args.template,
            if available.is_empty() {
                "(none)".to_string()
            } else {
                available.join(", ")
            }
        )
    })?;

    let dest = Path::new(&args.name);
    if dest.exists() {
        let meta = fs::metadata(dest)?;
        if !meta.is_dir() {
            anyhow::bail!(
                "destination '{}' already exists and is not a directory",
                args.name
            );
        }
        if !is_empty_dir(dest)? {
            anyhow::bail!(
                "destination '{}' already exists and is not empty",
                args.name
            );
        }
    }

    fs::create_dir_all(dest)?;

    // The embedded paths are prefixed with the template name (e.g.
    // `default/pages/index.tsx`). Strip that prefix so files land directly
    // under the destination directory.
    let prefix = Path::new(&args.template);
    write_dir(template, dest, prefix)?;

    match try_pnpm_install(dest).await {
        PnpmOutcome::Ran => {}
        PnpmOutcome::Missing => {
            println!(
                "pnpm not found on PATH \u{2014} skipping install. Run pnpm install manually before zfb dev."
            );
        }
        PnpmOutcome::Failed(msg) => {
            println!("pnpm install failed: {msg}. Run pnpm install manually before zfb dev.");
        }
    }

    println!(
        "\u{2713} Created {} (template: {}). Next: cd {} && zfb dev",
        args.name, args.template, args.name
    );

    Ok(())
}

fn available_templates() -> Vec<String> {
    TEMPLATES
        .dirs()
        .filter_map(|d| d.path().file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect()
}

fn is_empty_dir(path: &Path) -> anyhow::Result<bool> {
    let meta = fs::metadata(path)?;
    if !meta.is_dir() {
        // A non-directory existing entry should be treated as "non-empty".
        return Ok(false);
    }
    Ok(fs::read_dir(path)?.next().is_none())
}

/// Recursively write every entry in `dir` to disk under `dest`, with `prefix`
/// stripped from the embedded path.
fn write_dir(dir: &Dir<'_>, dest: &Path, prefix: &Path) -> anyhow::Result<()> {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(sub) => {
                let rel = sub.path().strip_prefix(prefix).unwrap_or(sub.path());
                let target = dest.join(rel);
                fs::create_dir_all(&target)?;
                write_dir(sub, dest, prefix)?;
            }
            DirEntry::File(file) => {
                let rel = file.path().strip_prefix(prefix).unwrap_or(file.path());
                let target = dest.join(rel);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&target, file.contents())?;
            }
        }
    }
    Ok(())
}

enum PnpmOutcome {
    Ran,
    Missing,
    Failed(String),
}

async fn try_pnpm_install(dest: &Path) -> PnpmOutcome {
    let mut cmd = Command::new("pnpm");
    cmd.arg("install").current_dir(dest);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == ErrorKind::NotFound => return PnpmOutcome::Missing,
        Err(e) => return PnpmOutcome::Failed(e.to_string()),
    };

    match child.wait().await {
        Ok(status) if status.success() => PnpmOutcome::Ran,
        Ok(status) => PnpmOutcome::Failed(format!("pnpm exited with status {status}")),
        Err(e) => PnpmOutcome::Failed(e.to_string()),
    }
}
