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
//!    via [`zfb_content::frontmatter::extract`] and validates the
//!    resulting JSON value against the per-collection JSON Schema
//!    (`collections[].schema`) using [`zfb_content::schema::validate`].
//! 3. **Invokes `tsc --noEmit`** as a subprocess. Resolution order:
//!    `node_modules/.bin/tsc` first (including `tsc.cmd` on Windows),
//!    falling back to a globally-installed `tsc` on `$PATH`. On Windows
//!    the PATH fallback returns the resolved shim path (e.g.
//!    `/usr/local/bin/tsc.cmd`) rather than a bare `"tsc"` name. The
//!    subprocess inherits stdio so the user sees tsc's normal report
//!    verbatim. Skip with `--skip-tsc`.
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

    // Honour the same include / exclude filter the walker / bundler
    // apply so a frontmatter check doesn't fire schema errors against
    // sibling files that wouldn't have made it into the collection in
    // the first place (e.g. EN siblings under a JA collection with an
    // `*.en.mdx` exclude pattern).
    let filter = zfb_content::collection::CollectionFilter::new(
        collection.include.as_deref(),
        collection.exclude.as_deref(),
        collection.id_strip_suffix.as_deref(),
    )
    .with_context(|| format!("collection {:?}: invalid filter glob", collection.name))?;
    if !filter.is_noop() {
        files.retain(|p| {
            let rel = p.strip_prefix(&dir).unwrap_or(p);
            // Render to forward-slash form because the filter
            // patterns are authored against POSIX paths.
            let rel_posix = if std::path::MAIN_SEPARATOR == '/' {
                rel.to_string_lossy().into_owned()
            } else {
                rel.to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            };
            filter.matches_relative(&rel_posix)
        });
    }

    let mut issues = Vec::new();
    for path in &files {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                issues.push(format!("{}: io error reading file: {}", path.display(), e));
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
        // Use the raw file_type (does not follow symlinks) so that a symlink
        // to a directory is seen as a symlink, not a dir. Recursing through
        // symlinked directories can produce infinite loops when a symlink
        // points at an ancestor directory. Skipping symlinks entirely is the
        // safe bound: entry files should not live behind a symlink in a
        // standard zfb project layout.
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        } else if ft.is_dir() {
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
///
/// No manual cmd.exe /c wrapping is needed: Rust std ≥ 1.77.2 routes
/// explicit `.cmd`/`.bat` paths through cmd.exe automatically (BatBadBut
/// fix). If a future MSRV constraint drops below 1.77.2, revisit this.
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
    // Fall back to PATH lookup. Return the resolved path so that spawning
    // the explicit path (e.g. tsc.cmd on Windows) works correctly.
    if let Some(resolved) = which_in_path("tsc") {
        return Ok(resolved);
    }
    bail!(
        "could not find `tsc`. Install TypeScript (e.g. `pnpm add -D typescript`) or run with `--skip-tsc`.",
    )
}

/// Probe PATH for `name`, `name.exe`, then `name<ext>` for each entry in
/// `extra_exts`. Returns the first existing path found.
///
/// `extra_exts` entries must include the leading dot (e.g. `".cmd"`).
fn which_in_path_with_exts(name: &str, extra_exts: &[&str]) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        let with_exe = dir.join(format!("{name}.exe"));
        if with_exe.exists() {
            return Some(with_exe);
        }
        for ext in extra_exts {
            debug_assert!(
                ext.starts_with('.'),
                "extra_exts entries must include the leading dot (got `{ext}`)"
            );
            let with_ext = dir.join(format!("{name}{ext}"));
            if with_ext.exists() {
                return Some(with_ext);
            }
        }
    }
    None
}

/// Probe PATH for `name`, additionally checking `.cmd` and `.bat` extensions
/// on Windows (where npm installs shim `.cmd` files into `node_modules/.bin`).
fn which_in_path(name: &str) -> Option<PathBuf> {
    // Runtime cfg! so the extension list is compiled in on all platforms but
    // only activates on Windows; Linux CI still compiles the full code path.
    let extra_exts: &[&str] = if cfg!(target_os = "windows") {
        &[".cmd", ".bat"]
    } else {
        &[]
    };
    which_in_path_with_exts(name, extra_exts)
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
            include: None,
            exclude: None,
            id_strip_suffix: None,
        };

        let issues = validate_collection(&tmp.path, &collection).unwrap();
        assert!(issues.is_empty(), "expected no issues, got: {issues:?}");
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
            include: None,
            exclude: None,
            id_strip_suffix: None,
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
            include: None,
            exclude: None,
            id_strip_suffix: None,
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
            include: None,
            exclude: None,
            id_strip_suffix: None,
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
            include: None,
            exclude: None,
            id_strip_suffix: None,
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
        assert_eq!(render_summary(1, false), "check failed: 1 schema violation");
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

    // Serialize PATH-mutating tests to avoid races between test threads.
    static PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Save the current PATH, set it to `new_path`, run `f`, then restore.
    fn with_path<F: FnOnce()>(new_path: &std::ffi::OsStr, f: F) {
        let _guard = PATH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = env::var_os("PATH");
        env::set_var("PATH", new_path);
        f();
        match saved {
            Some(v) => env::set_var("PATH", v),
            None => env::remove_var("PATH"),
        }
    }

    #[test]
    fn which_in_path_with_exts_finds_bare_name() {
        let tmp = TmpDir::new("bare-name");
        // Write a file with bare name (no extension).
        std::fs::write(tmp.path.join("tsc"), "").unwrap();

        with_path(tmp.path.as_os_str(), || {
            let result = which_in_path_with_exts("tsc", &[]);
            assert!(result.is_some(), "expected Some, got None");
            let p = result.unwrap();
            assert!(p.is_absolute(), "path should be absolute: {}", p.display());
            assert_eq!(p.file_name().unwrap().to_str().unwrap(), "tsc");
        });
    }

    #[test]
    fn which_in_path_with_exts_finds_cmd_extension_on_any_os() {
        let tmp = TmpDir::new("cmd-ext");
        // Write tsc.cmd — this tests extension probing on Linux CI too.
        std::fs::write(tmp.path.join("tsc.cmd"), "").unwrap();

        with_path(tmp.path.as_os_str(), || {
            let result = which_in_path_with_exts("tsc", &[".cmd"]);
            assert!(result.is_some(), "expected Some, got None");
            let p = result.unwrap();
            let name = p.file_name().unwrap().to_str().unwrap();
            assert_eq!(
                name,
                "tsc.cmd",
                "path should end in tsc.cmd, got: {}",
                p.display()
            );
        });
    }

    #[test]
    fn which_in_path_with_exts_returns_none_when_absent() {
        let tmp = TmpDir::new("absent");
        // No tsc* files in the tmp dir.

        with_path(tmp.path.as_os_str(), || {
            let result = which_in_path_with_exts("tsc", &[".cmd", ".bat"]);
            assert!(result.is_none(), "expected None, got {:?}", result);
        });
    }

    #[test]
    fn locate_tsc_returns_resolved_path_not_bare_name() {
        // Use the project-local candidate path (node_modules/.bin/tsc) so
        // no PATH mutation is needed — the local check runs first in locate_tsc.
        let tmp = TmpDir::new("locate-tsc");
        let bin_dir = tmp.path.join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("tsc"), "").unwrap();

        let result = locate_tsc(&tmp.path).expect("locate_tsc should succeed");
        assert!(
            result.is_absolute(),
            "resolved path should be absolute, got: {}",
            result.display()
        );
        assert_ne!(
            result,
            PathBuf::from("tsc"),
            "locate_tsc must not return bare 'tsc'"
        );
    }
}
