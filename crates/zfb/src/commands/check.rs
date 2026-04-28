//! `zfb check` — typecheck + content schema validation.
//!
//! Astro ships `astro check`, which combines TypeScript strict-mode
//! typechecking with collection-schema validation. `zfb check` plays
//! the same role for zfb projects so `pnpm check` keeps working as the
//! Astro→zfb migration progresses (see super-epic
//! [zudolab/zudo-doc#473](https://github.com/zudolab/zudo-doc/issues/473)).
//!
//! ## What this command does
//!
//! 1. **Loads `zfb.config.json`** via [`crate::config::load_from_dir`].
//!    Reuses the same loader the build/dev commands use so we stay in
//!    sync with config-shape evolutions.
//! 2. **Walks every collection** declared in
//!    `config.collections[]`. For each entry it parses the frontmatter
//!    via [`zfb_content::extract_frontmatter`] and validates the
//!    resulting JSON value against the per-collection JSON Schema
//!    (`collections[].schema`) using [`zfb_content::schema::validate`].
//! 3. **Invokes `tsc --noEmit`** as a subprocess. Resolution order:
//!    `node_modules/.bin/tsc` first, falling back to a globally-
//!    installed `tsc` on `$PATH`. The subprocess inherits stdio so the
//!    user sees tsc's normal report verbatim. Skip with `--skip-tsc`.
//!
//! Either failure mode produces a non-zero exit. The tally line at the
//! end (`✗ 1 schema violation, 2 type errors`) gives the user a single
//! place to look for the bottom line.
//!
//! ## Error rendering
//!
//! Schema violations are framed with a one-line per-issue summary. The
//! full file:line context for tsc errors is whatever tsc emits — we do
//! not wrap or filter it. (The error-ux topic running in parallel is
//! building a framed-error renderer; once it lands, the schema-issue
//! path here can adopt it. Until then, plain prose keeps the surface
//! minimal and predictable.)

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use anyhow::{anyhow, bail, Context, Result};

use zfb_content::frontmatter;
use zfb_content::schema as zfb_schema;

use crate::cli::CheckArgs;
use crate::config::{self, CollectionDef};
use crate::output;

pub async fn run(args: &CheckArgs) -> Result<()> {
    let project_root = env::current_dir().context("failed to read current working directory")?;

    let cfg = config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration")?;

    let mut schema_issues: Vec<String> = Vec::new();
    for collection in &cfg.collections {
        match validate_collection(&project_root, collection) {
            Ok(found) => schema_issues.extend(found),
            Err(e) => {
                // A walker error (bad YAML, missing dir we couldn't
                // read, etc.) counts as a check failure — fold it into
                // the same issue list so the final tally is honest.
                schema_issues.push(format!("{}: {}", collection.name, e));
            }
        }
    }

    for issue in &schema_issues {
        output::error(format!("schema: {issue}"));
    }

    let tsc_failed = if args.skip_tsc {
        output::info("skipping tsc (--skip-tsc)");
        false
    } else {
        run_tsc(&project_root)?
    };

    if !schema_issues.is_empty() || tsc_failed {
        let parts = render_summary(schema_issues.len(), tsc_failed);
        bail!("{}", parts);
    }

    output::success(format!(
        "checked {} collection{} and tsc — no errors",
        cfg.collections.len(),
        if cfg.collections.len() == 1 { "" } else { "s" },
    ));
    Ok(())
}

/// Walk one collection and validate every entry's frontmatter against
/// the declared schema. Returns a flat list of human-readable issues.
/// Collections without a `schema` field are walked anyway (so syntax
/// errors in frontmatter still surface), they just don't get
/// schema-shape checks.
fn validate_collection(project_root: &Path, collection: &CollectionDef) -> Result<Vec<String>> {
    let dir = project_root.join(&collection.path);
    if !dir.exists() {
        // A non-existent directory is a soft warning, not an error —
        // matches `walk_collection`'s behaviour and keeps freshly-
        // scaffolded projects (where the dir hasn't been created yet)
        // from exploding.
        output::warn(format!(
            "collection {:?}: directory {} does not exist; skipping",
            collection.name,
            dir.display()
        ));
        return Ok(Vec::new());
    }

    let mut files: Vec<PathBuf> = Vec::new();
    collect_entry_files(&dir, &mut files).map_err(|e| {
        anyhow!(
            "reading collection {:?} at {}: {}",
            collection.name,
            dir.display(),
            e
        )
    })?;
    files.sort();

    let mut issues = Vec::new();
    for path in &files {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                issues.push(format!(
                    "{}: io error reading file: {}",
                    path.display(),
                    e
                ));
                continue;
            }
        };
        let uf = match frontmatter::extract(path, &raw) {
            Ok(uf) => uf,
            Err(e) => {
                issues.push(format!(
                    "{}: frontmatter parse error: {}",
                    path.display(),
                    e
                ));
                continue;
            }
        };

        if let Some(schema) = &collection.schema {
            for issue in zfb_schema::validate(&uf.value, schema) {
                issues.push(format!(
                    "{}: collection {:?}: {}",
                    path.display(),
                    collection.name,
                    issue
                ));
            }
        }
    }
    Ok(issues)
}

fn collect_entry_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_entry_files(&p, out)?;
        } else if ft.is_file() && is_entry_file(&p) {
            out.push(p);
        }
    }
    Ok(())
}

fn is_entry_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("md") | Some("mdx") | Some("tsx")
    )
}

/// Run `tsc --noEmit` as a subprocess. Returns `true` if tsc reported
/// any errors (non-zero exit), `false` if clean.
///
/// Resolution order: prefer `<project>/node_modules/.bin/tsc` (so the
/// version pinned in the project's lockfile wins), fall back to `tsc`
/// on `$PATH`, fall back to "tsc not found" with an actionable hint.
fn run_tsc(project_root: &Path) -> Result<bool> {
    let tsc_bin = locate_tsc(project_root)?;
    output::info(format!("running {} --noEmit", tsc_bin.display()));

    let status = StdCommand::new(&tsc_bin)
        .arg("--noEmit")
        .current_dir(project_root)
        .status()
        .with_context(|| format!("failed to spawn {}", tsc_bin.display()))?;

    Ok(!status.success())
}

fn locate_tsc(project_root: &Path) -> Result<PathBuf> {
    let candidates = [
        project_root.join("node_modules").join(".bin").join("tsc"),
        project_root
            .join("node_modules")
            .join(".bin")
            .join("tsc.cmd"), // Windows
    ];
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    // Fall back to PATH lookup. We let the OS resolve so the spawn fails
    // with a clear "command not found" downstream if it's missing.
    if which_in_path("tsc").is_some() {
        return Ok(PathBuf::from("tsc"));
    }
    bail!(
        "could not find `tsc`. Install TypeScript (e.g. `pnpm add -D typescript`) or run with `--skip-tsc`.",
    )
}

fn which_in_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        let with_ext = dir.join(format!("{name}.exe"));
        if with_ext.exists() {
            return Some(with_ext);
        }
    }
    None
}

fn render_summary(schema_count: usize, tsc_failed: bool) -> String {
    let mut parts = Vec::new();
    if schema_count > 0 {
        parts.push(format!(
            "{schema_count} schema violation{}",
            if schema_count == 1 { "" } else { "s" },
        ));
    }
    if tsc_failed {
        parts.push("type errors".to_string());
    }
    if parts.is_empty() {
        // Defensive — render_summary shouldn't be called when both are
        // zero. Keep the message non-empty so downstream `bail!` has a
        // signal.
        return "check failed".to_string();
    }
    format!("check failed: {}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::JsonSchema;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Self-cleaning temp dir; mirrors the helper in zfb-content's
    /// collection.rs tests so we don't depend on the `tempfile` crate
    /// for fixture work.
    struct TmpDir {
        path: PathBuf,
    }
    impl TmpDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!(
                "zfb-check-{label}-{nanos}-{n}-{pid}",
                pid = std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("create tmp dir");
            Self { path: dir }
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn write(root: &Path, rel: &str, contents: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    #[test]
    fn validate_collection_returns_empty_for_valid_entries() {
        let tmp = TmpDir::new("valid");
        write(
            &tmp.path,
            "content/blog/a.md",
            "---\ntitle: A\ndate: 2026-01-01\n---\nbody\n",
        );
        write(
            &tmp.path,
            "content/blog/b.md",
            "---\ntitle: B\ndate: 2026-02-02\n---\nbody\n",
        );

        let collection = CollectionDef {
            name: "blog".into(),
            path: PathBuf::from("content/blog"),
            schema: Some(
                JsonSchema::try_from_value(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "date": { "type": "string" }
                    },
                    "required": ["title", "date"]
                }))
                .unwrap(),
            ),
        };

        let issues = validate_collection(&tmp.path, &collection).unwrap();
        assert!(
            issues.is_empty(),
            "expected no issues, got: {issues:?}"
        );
    }

    #[test]
    fn validate_collection_flags_type_mismatch_with_path() {
        let tmp = TmpDir::new("type-mismatch");
        write(
            &tmp.path,
            "content/docs/intro.md",
            "---\ntitle: Intro\nsidebar_position: \"1\"\n---\nbody\n",
        );

        let collection = CollectionDef {
            name: "docs".into(),
            path: PathBuf::from("content/docs"),
            schema: Some(
                JsonSchema::try_from_value(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "sidebar_position": { "type": "number" }
                    },
                    "required": ["title"]
                }))
                .unwrap(),
            ),
        };

        let issues = validate_collection(&tmp.path, &collection).unwrap();
        assert_eq!(issues.len(), 1, "issues: {issues:?}");
        let msg = &issues[0];
        assert!(msg.contains("intro.md"), "should name the file: {msg}");
        assert!(msg.contains("sidebar_position"), "{msg}");
        assert!(msg.contains("expected number"), "{msg}");
    }

    #[test]
    fn validate_collection_flags_missing_required_field() {
        let tmp = TmpDir::new("missing-required");
        write(
            &tmp.path,
            "content/blog/post.md",
            "---\ndate: 2026-04-28\n---\nbody\n",
        );

        let collection = CollectionDef {
            name: "blog".into(),
            path: PathBuf::from("content/blog"),
            schema: Some(
                JsonSchema::try_from_value(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "date": { "type": "string" }
                    },
                    "required": ["title", "date"]
                }))
                .unwrap(),
            ),
        };

        let issues = validate_collection(&tmp.path, &collection).unwrap();
        assert_eq!(issues.len(), 1, "issues: {issues:?}");
        assert!(issues[0].contains("title"), "{}", issues[0]);
        assert!(issues[0].contains("missing"), "{}", issues[0]);
    }

    #[test]
    fn validate_collection_with_no_schema_skips_shape_check() {
        // Without a schema, an entry whose frontmatter parses cleanly
        // produces zero issues regardless of shape.
        let tmp = TmpDir::new("no-schema");
        write(
            &tmp.path,
            "content/blog/a.md",
            "---\nwhatever: 7\n---\nbody\n",
        );

        let collection = CollectionDef {
            name: "blog".into(),
            path: PathBuf::from("content/blog"),
            schema: None,
        };

        let issues = validate_collection(&tmp.path, &collection).unwrap();
        assert!(
            issues.is_empty(),
            "schema-less collection produced issues: {issues:?}"
        );
    }

    #[test]
    fn validate_collection_flags_malformed_frontmatter() {
        let tmp = TmpDir::new("bad-yaml");
        write(
            &tmp.path,
            "content/blog/bad.md",
            "---\ntitle: [unterminated\n---\nbody\n",
        );

        let collection = CollectionDef {
            name: "blog".into(),
            path: PathBuf::from("content/blog"),
            schema: None,
        };

        let issues = validate_collection(&tmp.path, &collection).unwrap();
        assert_eq!(issues.len(), 1);
        assert!(
            issues[0].contains("frontmatter parse error"),
            "{}",
            issues[0]
        );
    }

    #[test]
    fn render_summary_shapes() {
        assert_eq!(
            render_summary(1, false),
            "check failed: 1 schema violation"
        );
        assert_eq!(
            render_summary(3, false),
            "check failed: 3 schema violations"
        );
        assert_eq!(render_summary(0, true), "check failed: type errors");
        assert_eq!(
            render_summary(2, true),
            "check failed: 2 schema violations, type errors"
        );
    }

}
