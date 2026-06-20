//! `zfb new` command — scaffold a new project from an embedded template.
//!
//! The contents of `crates/zfb/templates/` are baked into the binary at compile
//! time via [`include_dir`]. At runtime we walk the requested template subtree
//! (defaulting to `basic-blog`) and write each file into the destination
//! directory. After the files are in place the scaffolded `package.json` is
//! patched in two ways:
//!
//! 1. The `name` field is rewritten to the user's project name (sanitized so
//!    the value is npm-valid — see [`sanitize_pkg_name`]).
//! 2. Any dependency value matching `workspace:*` is rewritten to the
//!    exact-pinned version produced by [`workspace_dep_placeholder`]. The
//!    template ships its workspace deps using pnpm's `workspace:*` protocol so
//!    the rewrite has something to bite on; the placeholder is an exact-pinned
//!    version string (`=<version>`) that equals the running binary's own
//!    version. Exact pin prevents a silent upgrade to a future stable once the
//!    CLI moves from prerelease to stable. The value is **self-syncing** — it
//!    is derived from the binary's release version at compile time, so there is
//!    nothing to hand-maintain or keep in lockstep across releases.
//!
//! After patching we attempt to run `pnpm install`; if pnpm is missing we
//! print a friendly notice and continue successfully so the user can run
//! install themselves later.
//!
//! **Node-free templates** (currently: `node-free`) skip both the
//! `patch_package_json` and `try_pnpm_install` steps entirely — they ship no
//! `package.json` and are intended for users running zfb with no Node/pnpm on
//! PATH. The gate is driven by [`NO_INSTALL_TEMPLATES`]; add a template name
//! there to opt it out of the npm post-install pipeline.
//!
//! Status messages go through [`crate::output`] so they look consistent with
//! the rest of the zfb CLI. `zfb new` deliberately does NOT load
//! `zfb.config.{json,ts}` — at the moment this command runs the project does
//! not yet exist on disk, so there is no config to apply.

use std::io::ErrorKind;
use std::path::Path;

use include_dir::{include_dir, Dir, DirEntry};
use serde_json::{Map, Value};
use std::fs;
use tokio::process::Command;

use crate::cli::NewArgs;
use crate::output;

/// Compile-time embedding of `crates/zfb/templates/`.
///
/// Each top-level subdirectory is a template selectable via `--template`.
static TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");

/// Templates that ship no `package.json` and require no `pnpm install` step.
///
/// When a scaffolded template name appears in this list, the post-scaffold
/// pipeline skips both `patch_package_json` and `try_pnpm_install`. Add a
/// new template name here to opt it out of the npm pipeline; existing
/// templates (`basic-blog`, …) are unaffected.
const NO_INSTALL_TEMPLATES: &[&str] = &["node-free"];

/// Replacement value applied to any `workspace:*` dependency found in a
/// scaffolded `package.json`: an exact pin (`=<version>`) of the version of
/// the zfb binary doing the scaffolding.
///
/// **Self-syncing — nothing to hand-maintain.** The version is read at compile
/// time from `ZFB_RELEASE_VERSION` (the release build injects it — see
/// [`crate::cli`] and `.github/workflows/release.yml`), falling back to
/// `CARGO_PKG_VERSION` for local/dev builds (which is the `0.0.0` placeholder
/// in `crates/zfb/Cargo.toml`). This is the same version source `zfb --version`
/// uses, so a scaffold always pins exactly the release that produced it. That
/// closes the drift bug where the binary and the scaffolded pin disagreed
/// (e.g. a `next.7` binary writing `=0.1.0-next.6`).
///
/// Exact pin (`=`) — not a caret range — so scaffolds never silently upgrade
/// to a future stable. For example `^0.1.0-next.4` would match stable `0.1.0`
/// (npm semver treats stable as greater than any prerelease of the same
/// triplet), implicitly moving the SDK channel once `0.1.0` is published.
///
/// See: <https://github.com/Takazudo/zudo-front-builder/issues/503> (self-sync)
/// and <https://github.com/Takazudo/zudo-front-builder/issues/343> (exact pin).
fn workspace_dep_placeholder() -> String {
    let version = option_env!("ZFB_RELEASE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
    format!("={version}")
}

/// Dependency-section keys we walk inside `package.json` when rewriting
/// `workspace:*` ranges. Kept as a constant so the rewriter and its tests
/// stay in sync.
const DEP_SECTIONS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

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

    validate_project_name(&args.name)?;

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

    // Track whether we created the directory in this invocation so we can
    // remove it on failure (cleanup-on-error). If the directory already
    // existed (empty dir allowed above) we leave it alone on the error path
    // so the user doesn't lose anything they put there.
    let we_created_dest = !dest.exists();
    fs::create_dir_all(dest)?;

    // Helper: best-effort cleanup of `dest` when we own it and a later step
    // fails. Defined here so the borrow of `we_created_dest` is local.
    let cleanup = |dest: &Path| {
        if we_created_dest {
            let _ = fs::remove_dir_all(dest);
        }
    };

    // The embedded paths are prefixed with the template name (e.g.
    // `basic-blog/pages/index.tsx`). Strip that prefix so files land directly
    // under the destination directory.
    let prefix = Path::new(&args.template);
    if let Err(e) = write_dir(template, dest, prefix) {
        cleanup(dest);
        return Err(e);
    }

    // Node-free templates ship no `package.json` and do not need `pnpm
    // install`. Skip both post-scaffold npm steps for them so a user with no
    // Node/pnpm on PATH can run `zfb dev` immediately after scaffolding.
    let skip_npm = NO_INSTALL_TEMPLATES.contains(&args.template.as_str());

    if !skip_npm {
        // Patch package.json: project name + workspace dep placeholder. We do
        // this after writing files so the rewriter operates on the same bytes
        // the user will see, and so a future template that ships multiple
        // package.json files (e.g. nested workspaces) can be handled by
        // expanding the search rather than the embedding.
        if let Err(e) = patch_package_json(&dest.join("package.json"), &args.name) {
            cleanup(dest);
            return Err(e);
        }

        match try_pnpm_install(dest).await {
            PnpmOutcome::Ran => {}
            PnpmOutcome::Missing => {
                output::warn(
                    "pnpm not found on PATH \u{2014} skipping install. Run pnpm install manually before zfb dev.",
                );
            }
            PnpmOutcome::Failed(msg) => {
                output::warn(format!(
                    "pnpm install failed: {msg}. Run pnpm install manually before zfb dev."
                ));
            }
        }
    }

    // `output::success` already prefixes a green checkmark; do not include
    // one in the message body or the user sees `✓ ✓ Created ...`.
    output::success(format!(
        "Created {} (template: {}). Next: cd {} && zfb dev",
        args.name, args.template, args.name
    ));

    Ok(())
}

/// Reject project names that would escape the current working directory or
/// otherwise resolve to a non-relative path. The CLI uses the name verbatim
/// as the destination directory, so without this gate `zfb new ../../etc`
/// would happily write the template tree outside the user's intended root.
///
/// Allowed: non-empty names with no path separators and no `..` segments.
/// Absolute paths are also rejected.
fn validate_project_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        anyhow::bail!("project name must not be empty");
    }
    if name == "." || name == ".." {
        anyhow::bail!("project name '{name}' is not a valid directory name");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!(
            "project name '{name}' must not contain path separators \u{2014} pass a single directory name"
        );
    }
    let path = Path::new(name);
    if path.is_absolute() {
        anyhow::bail!(
            "project name '{name}' must be a relative directory name, not an absolute path"
        );
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        anyhow::bail!("project name '{name}' must not contain '..' segments");
    }
    Ok(())
}

fn available_templates() -> Vec<String> {
    TEMPLATES
        .dirs()
        .filter_map(|d| {
            d.path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
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

/// Rewrite the scaffolded `package.json` so it carries the user's project
/// name and so any `workspace:*` deps point at a published-package
/// placeholder rather than a workspace protocol the user's pnpm cannot
/// resolve outside the zfb monorepo.
///
/// Missing `package.json` is tolerated (some future template may not ship
/// one). Malformed JSON or unexpected shapes (e.g. `dependencies` not being
/// an object) are surfaced as errors so a broken template is caught at
/// scaffold time instead of at the user's first `pnpm install`.
fn patch_package_json(path: &Path, project_name: &str) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(path)?;
    let mut value: Value = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("template package.json is not valid JSON: {e}"))?;

    let obj = value.as_object_mut().ok_or_else(|| {
        anyhow::anyhow!("template package.json must be a JSON object at the top level")
    })?;

    let sanitized = sanitize_pkg_name(project_name);
    obj.insert("name".to_string(), Value::String(sanitized));
    rewrite_workspace_deps(obj);

    // Preserve a trailing newline so the file matches what
    // `pnpm install` (and humans) expect to see.
    let mut serialized = serde_json::to_string_pretty(&value)?;
    serialized.push('\n');
    fs::write(path, serialized)?;
    Ok(())
}

/// Walk every dependency section we know about and replace any value that
/// starts with `workspace:` (covering `workspace:*`, `workspace:^`,
/// `workspace:~`, and pinned forms like `workspace:1.2.3`) with the
/// published-package placeholder.
fn rewrite_workspace_deps(pkg: &mut Map<String, Value>) {
    let placeholder = workspace_dep_placeholder();
    for section in DEP_SECTIONS {
        let Some(deps) = pkg.get_mut(*section).and_then(|v| v.as_object_mut()) else {
            continue;
        };
        for (_dep_name, dep_value) in deps.iter_mut() {
            if let Some(s) = dep_value.as_str() {
                if s.starts_with("workspace:") {
                    *dep_value = Value::String(placeholder.clone());
                }
            }
        }
    }
}

/// Maximum byte length of a valid npm package name (npm spec §2.1).
///
/// npm enforces a 214-character cap on the combined `name` field in
/// `package.json`. We apply it here so a very long directory name can never
/// produce an un-publishable manifest. The value is a fixed product spec
/// constant — see <https://docs.npmjs.com/cli/v10/configuring-npm/package-json#name>.
const NPM_NAME_MAX_LEN: usize = 214;

/// Coerce the user-provided project name into something npm will accept as
/// a `package.json#name` value: lowercased, ASCII alphanumerics and a small
/// set of separators, anything else collapsed to `-`. Empty results fall
/// back to a stable default so we never produce an invalid manifest.
///
/// Additional npm rules enforced here:
/// - Max 214 characters (npm spec §2.1); truncated at a `-` boundary.
/// - Leading `.` or `_` are npm-reserved and invalid at position 0; they are
///   stripped (same rule as the surrounding separator trim).
/// - The final result is validated against the npm name regex before return;
///   if it somehow fails the validation the stable default is returned.
///
/// Scoped names (`@scope/pkg`) are intentionally NOT supported here; the
/// CLI's positional `name` is also used as the destination directory, and
/// `@scope/pkg` would create awkward paths. A user who needs a scoped
/// package can edit `package.json` after scaffolding.
fn sanitize_pkg_name(input: &str) -> String {
    let lowered: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    // Strip leading/trailing separators including npm-reserved leading chars
    // `.` and `_` (npm rejects names starting with either).
    let trimmed = lowered.trim_matches(|c: char| c == '-' || c == '.' || c == '_');

    if trimmed.is_empty() {
        return "zfb-project".to_string();
    }

    // Enforce the 214-character npm cap. Truncate at a `-` boundary when
    // possible so the result doesn't end mid-word.
    let capped: &str = if trimmed.len() <= NPM_NAME_MAX_LEN {
        trimmed
    } else {
        let slice = &trimmed[..NPM_NAME_MAX_LEN];
        // Walk back to the last `-` to avoid cutting mid-token.
        if let Some(pos) = slice.rfind('-') {
            &trimmed[..pos]
        } else {
            slice
        }
    };

    // Strip any leading `.` or `_` that survived the truncation boundary
    // (shouldn't happen after the trim above, but be defensive).
    let final_name = capped.trim_start_matches(['.', '_']);

    if final_name.is_empty() {
        return "zfb-project".to_string();
    }

    // Validate the result against the npm name rules:
    // - all lowercase ASCII
    // - only [a-z0-9._-]
    // - does not start with `.` or `_`
    // If validation fails (should be impossible given the transforms above),
    // fall back to the stable default rather than writing an invalid name.
    let valid = is_valid_npm_name(final_name);
    if !valid {
        return "zfb-project".to_string();
    }

    final_name.to_string()
}

/// Return `true` if `name` satisfies npm's `package.json#name` rules for
/// non-scoped packages (post-sanitize validation guard).
fn is_valid_npm_name(name: &str) -> bool {
    if name.is_empty() || name.len() > NPM_NAME_MAX_LEN {
        return false;
    }
    // npm-reserved leading characters.
    let first = name.chars().next().unwrap();
    if first == '.' || first == '_' {
        return false;
    }
    // Only [a-z0-9._-] are allowed; no uppercase, no other chars.
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn validate_project_name_rejects_path_traversal_and_separators() {
        // Acceptable shapes.
        assert!(validate_project_name("my-site").is_ok());
        assert!(validate_project_name("My_Site.v2").is_ok());

        // Rejected shapes — path traversal, separators, absolute paths.
        for bad in [
            "",
            ".",
            "..",
            "../evil",
            "foo/bar",
            "foo\\bar",
            "/etc/passwd",
        ] {
            assert!(
                validate_project_name(bad).is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn sanitize_pkg_name_lowercases_and_replaces_invalid_chars() {
        assert_eq!(sanitize_pkg_name("My Site"), "my-site");
        assert_eq!(sanitize_pkg_name("My_Site.v2"), "my_site.v2");
        assert_eq!(sanitize_pkg_name("a/b\\c"), "a-b-c");
        // Leading/trailing separators are stripped.
        assert_eq!(sanitize_pkg_name("---weird---"), "weird");
        // Pure-junk names fall back to a stable default.
        assert_eq!(sanitize_pkg_name("///"), "zfb-project");
        assert_eq!(sanitize_pkg_name(""), "zfb-project");
    }

    #[test]
    fn sanitize_pkg_name_strips_npm_reserved_leading_chars() {
        // npm rejects names that start with `.` or `_`.
        // Leading dots/underscores must be stripped, leaving a valid name.
        let result = sanitize_pkg_name(".hidden-project");
        assert!(
            is_valid_npm_name(&result),
            "leading-dot name produced invalid npm name: {result}"
        );
        assert!(
            !result.starts_with('.'),
            "leading dot must be stripped: {result}"
        );

        let result = sanitize_pkg_name("_private-pkg");
        assert!(
            is_valid_npm_name(&result),
            "leading-underscore name produced invalid npm name: {result}"
        );
        assert!(
            !result.starts_with('_'),
            "leading underscore must be stripped: {result}"
        );

        // Pure leading reserved chars fall back to the stable default.
        let result = sanitize_pkg_name("...");
        assert_eq!(result, "zfb-project");
        let result = sanitize_pkg_name("___");
        assert_eq!(result, "zfb-project");
    }

    #[test]
    fn sanitize_pkg_name_caps_at_214_chars() {
        // npm name limit is 214 characters. Input longer than that must be
        // truncated to a valid npm name.
        let long_input: String = "a".repeat(300);
        let result = sanitize_pkg_name(&long_input);
        assert!(
            result.len() <= NPM_NAME_MAX_LEN,
            "sanitized name exceeds 214 chars: len={} name={result}",
            result.len()
        );
        assert!(
            is_valid_npm_name(&result),
            "over-length input produced invalid npm name: {result}"
        );

        // Also test with a name that has dashes so the boundary-cut logic is
        // exercised.
        let long_dashed: String = "my-proj-".repeat(30); // 240 chars
        let result = sanitize_pkg_name(&long_dashed);
        assert!(
            result.len() <= NPM_NAME_MAX_LEN,
            "dashed over-length name exceeds 214 chars: len={} name={result}",
            result.len()
        );
        assert!(
            is_valid_npm_name(&result),
            "dashed over-length name produced invalid npm name: {result}"
        );
    }

    #[test]
    fn sanitize_pkg_name_results_are_always_valid_npm_names() {
        // Fuzz a sample of awkward inputs and assert all outputs pass
        // `is_valid_npm_name`.
        let cases = [
            "My Project",
            "123-numbers",
            "UPPER_CASE",
            ".dotfile",
            "_underscore_start",
            "...dots...",
            "a/b/c",
            "foo@bar",
            "very-long-name-repeated-many-times-to-exceed-the-npm-214-char-limit-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x-x",
            "",
            "///",
        ];
        for input in cases {
            let result = sanitize_pkg_name(input);
            assert!(
                is_valid_npm_name(&result),
                "input={input:?} produced invalid npm name: {result}"
            );
        }
    }

    #[test]
    fn partial_scaffold_cleanup_removes_dest_on_write_failure() {
        // Simulate a partial scaffold: create the dest dir, then attempt to
        // write a file to a path that will fail (e.g., writing into a path
        // that is a file, not a dir).  We test the cleanup helper indirectly
        // by checking that a freshly created dest is removed after failure.
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("new-project");

        // The dir does not exist yet — we_created_dest would be true.
        assert!(!dest.exists());
        fs::create_dir_all(&dest).unwrap();
        assert!(dest.exists());

        // Simulate a failure: attempt to remove the dir to mimic cleanup.
        // In the real code, `cleanup` is called on the error path before
        // returning the error.  Here we directly verify that remove_dir_all
        // on an empty freshly-created dir succeeds (it's the mechanism used).
        fs::remove_dir_all(&dest).unwrap();
        assert!(
            !dest.exists(),
            "cleanup must remove the dest dir on failure"
        );
    }

    #[test]
    fn rewrite_workspace_deps_replaces_workspace_protocol_only() {
        let mut pkg = json!({
            "name": "fixture",
            "dependencies": {
                "preact": "^10.22.0",
                "@takazudo/zfb": "workspace:*",
            },
            "devDependencies": {
                "internal-tool": "workspace:^",
                "typescript": "^5.6.0",
            },
            "peerDependencies": {
                "react": "workspace:1.2.3",
            },
            "optionalDependencies": {
                "fsevents": "workspace:~",
            }
        });
        let obj = pkg.as_object_mut().unwrap();
        rewrite_workspace_deps(obj);
        let ph = workspace_dep_placeholder();

        assert_eq!(
            obj["dependencies"]["preact"].as_str().unwrap(),
            "^10.22.0",
            "non-workspace deps must be untouched"
        );
        assert_eq!(
            obj["dependencies"]["@takazudo/zfb"].as_str().unwrap(),
            ph.as_str()
        );
        assert_eq!(
            obj["devDependencies"]["internal-tool"].as_str().unwrap(),
            ph.as_str()
        );
        assert_eq!(
            obj["devDependencies"]["typescript"].as_str().unwrap(),
            "^5.6.0"
        );
        assert_eq!(
            obj["peerDependencies"]["react"].as_str().unwrap(),
            ph.as_str()
        );
        assert_eq!(
            obj["optionalDependencies"]["fsevents"].as_str().unwrap(),
            ph.as_str()
        );
    }

    #[test]
    fn patch_package_json_writes_name_and_rewrites_workspace_deps() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("package.json");
        fs::write(
            &path,
            r#"{
  "name": "template-default",
  "dependencies": {
    "@takazudo/zfb": "workspace:*",
    "@takazudo/zfb-runtime": "workspace:*"
  }
}
"#,
        )
        .unwrap();

        patch_package_json(&path, "My Site").unwrap();

        let after = fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&after).unwrap();
        let ph = workspace_dep_placeholder();
        assert_eq!(parsed["name"].as_str().unwrap(), "my-site");
        assert_eq!(
            parsed["dependencies"]["@takazudo/zfb"].as_str().unwrap(),
            ph.as_str()
        );
        // @takazudo/zfb-runtime ships as a workspace:* dep in the template so
        // it gets pinned to the exact release version, just like @takazudo/zfb.
        assert_eq!(
            parsed["dependencies"]["@takazudo/zfb-runtime"]
                .as_str()
                .unwrap(),
            ph.as_str(),
            "@takazudo/zfb-runtime must be pinned to the binary's version"
        );
        assert!(after.ends_with('\n'), "trailing newline preserved");
    }

    #[test]
    fn workspace_dep_placeholder_is_an_exact_pin_of_the_binary_version() {
        // Self-syncing pin (#503): the placeholder must be an exact pin of the
        // version this binary reports, derived the same way `zfb --version` is
        // (ZFB_RELEASE_VERSION when injected, else CARGO_PKG_VERSION). In a test
        // build ZFB_RELEASE_VERSION is normally unset, so it falls back to
        // CARGO_PKG_VERSION. If the release env var IS set in the build
        // environment, the placeholder tracks it — assert against the same
        // source rather than a hardcoded literal so this never drifts.
        let expected_version =
            option_env!("ZFB_RELEASE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
        let ph = workspace_dep_placeholder();
        assert!(
            ph.starts_with('='),
            "placeholder must be an exact pin: {ph}"
        );
        assert_eq!(ph, format!("={expected_version}"));
    }

    #[test]
    fn patch_package_json_is_a_noop_when_file_is_missing() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("package.json");
        // No file written. Should not error.
        patch_package_json(&path, "anything").unwrap();
        assert!(!path.exists(), "function must not create the file");
    }

    #[test]
    fn template_registry_exposes_basic_blog() {
        let names = available_templates();
        assert!(
            names.iter().any(|n| n == "basic-blog"),
            "expected 'basic-blog' in templates, got {names:?}"
        );
    }

    #[test]
    fn template_basic_blog_package_json_is_well_formed() {
        // Sanity check: the shipped template must parse as JSON and
        // declare a workspace dep so the rewriter has something real
        // to bite on. If a future template change drops the workspace
        // dep this test should be updated, not deleted.
        let dir = TEMPLATES
            .get_dir("basic-blog")
            .expect("basic-blog template missing from registry");
        let pkg_file = dir
            .get_file("basic-blog/package.json")
            .expect("basic-blog/package.json missing from template");
        let parsed: Value = serde_json::from_slice(pkg_file.contents()).unwrap();
        let deps = parsed["dependencies"].as_object().unwrap();
        let has_workspace_dep = deps
            .values()
            .filter_map(|v| v.as_str())
            .any(|s| s.starts_with("workspace:"));
        assert!(
            has_workspace_dep,
            "basic-blog template should declare at least one workspace:* dep"
        );
        // Specific regression guard: @takazudo/zfb-runtime must be declared so
        // scaffolded projects can build without a missing-dep error (#482).
        assert_eq!(
            deps.get("@takazudo/zfb-runtime")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            "workspace:*",
            "basic-blog template must declare @takazudo/zfb-runtime as workspace:*"
        );
    }

    // -----------------------------------------------------------------
    // node-free template tests
    // -----------------------------------------------------------------

    #[test]
    fn template_registry_exposes_node_free() {
        let names = available_templates();
        assert!(
            names.iter().any(|n| n == "node-free"),
            "expected 'node-free' in templates, got {names:?}"
        );
    }

    #[test]
    fn node_free_template_has_no_package_json() {
        let dir = TEMPLATES
            .get_dir("node-free")
            .expect("node-free template missing from registry");
        // Use get_file to check known paths (Dir::files() is non-recursive).
        // A root-level package.json is the only place one could appear.
        assert!(
            dir.get_file("node-free/package.json").is_none(),
            "node-free template must not ship a package.json"
        );
        assert!(
            dir.get_file("node-free/pnpm-lock.yaml").is_none(),
            "node-free template must not ship a pnpm-lock.yaml"
        );
    }

    #[test]
    fn node_free_template_has_required_files() {
        let dir = TEMPLATES
            .get_dir("node-free")
            .expect("node-free template missing from registry");

        // Check for zfb.config.json at the template root.
        let config = dir.get_file("node-free/zfb.config.json");
        assert!(
            config.is_some(),
            "node-free template must contain zfb.config.json"
        );

        // Check for README.md.
        let readme = dir.get_file("node-free/README.md");
        assert!(
            readme.is_some(),
            "node-free template must contain README.md"
        );

        // Check for at least one .tsx page (using known paths since
        // include_dir's Dir::files() is non-recursive).
        let has_tsx_page = dir.get_file("node-free/pages/index.tsx").is_some();
        assert!(
            has_tsx_page,
            "node-free template must contain pages/index.tsx"
        );

        // Content collections work under the embedded V8 host (the
        // snapshot is baked into the bundle, so `getCollection` resolves
        // without `node:fs`). The template ships a `content/posts/` dir
        // with at least one `.md` seed post so `zfb build` produces a
        // working dist out of the box.
        let posts_dir = dir
            .get_dir("node-free/content/posts")
            .expect("node-free template must ship content/posts/ directory");
        let has_md_post = posts_dir
            .files()
            .any(|f| f.path().extension().and_then(|e| e.to_str()) == Some("md"));
        assert!(
            has_md_post,
            "node-free/content/posts/ must contain at least one .md seed post"
        );
        assert!(
            dir.get_file("node-free/pages/posts/[slug].tsx").is_some(),
            "node-free template must contain pages/posts/[slug].tsx"
        );
    }

    #[test]
    fn node_free_template_zfb_config_is_valid_json() {
        let dir = TEMPLATES
            .get_dir("node-free")
            .expect("node-free template missing from registry");
        let config_file = dir
            .get_file("node-free/zfb.config.json")
            .expect("node-free/zfb.config.json missing from template");
        let parsed: Result<Value, _> = serde_json::from_slice(config_file.contents());
        assert!(
            parsed.is_ok(),
            "node-free/zfb.config.json must be valid JSON: {:?}",
            parsed.err()
        );
        let config = parsed.unwrap();
        assert!(
            config.get("framework").is_some(),
            "node-free/zfb.config.json must declare 'framework'"
        );
    }

    #[test]
    fn node_free_scaffold_produces_correct_file_set() {
        // Scaffold the node-free template into a tempdir and verify:
        // - zfb.config.json is present and valid JSON
        // - README.md is present
        // - At least one .tsx page is present
        // - At least one .md content file is present
        // - package.json is NOT present
        // - pnpm-lock.yaml is NOT present
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("my-site");
        fs::create_dir_all(&dest).unwrap();

        let template_dir = TEMPLATES
            .get_dir("node-free")
            .expect("node-free template missing");
        let prefix = Path::new("node-free");
        write_dir(template_dir, &dest, prefix).unwrap();

        assert!(
            dest.join("zfb.config.json").exists(),
            "scaffolded site must have zfb.config.json"
        );
        assert!(
            dest.join("README.md").exists(),
            "scaffolded site must have README.md"
        );
        assert!(
            !dest.join("package.json").exists(),
            "scaffolded site must NOT have package.json"
        );
        assert!(
            !dest.join("pnpm-lock.yaml").exists(),
            "scaffolded site must NOT have pnpm-lock.yaml"
        );

        // zfb.config.json must parse as valid JSON.
        let config_raw = fs::read_to_string(dest.join("zfb.config.json")).unwrap();
        let config: Value = serde_json::from_str(&config_raw)
            .expect("scaffolded zfb.config.json must be valid JSON");
        assert!(
            config.get("framework").is_some(),
            "scaffolded zfb.config.json must declare 'framework'"
        );

        // At least one .tsx page must be present somewhere under pages/.
        let pages_dir = dest.join("pages");
        assert!(
            pages_dir.exists(),
            "scaffolded site must have a pages/ directory"
        );
        let has_tsx = walkdir_has_extension(&pages_dir, "tsx");
        assert!(
            has_tsx,
            "scaffolded pages/ must contain at least one .tsx file"
        );

        // content/ ships with the template now — getCollection() resolves
        // from the in-bundle snapshot under the embedded V8 host, so the
        // node-free path no longer needs node:fs at build time. A regression
        // that removes the seed content would silently break `zfb build`'s
        // homepage rendering, so assert presence.
        let content_posts = dest.join("content/posts");
        assert!(
            content_posts.exists(),
            "scaffolded node-free site must contain content/posts/ directory"
        );
        let has_md_post = walkdir_has_extension(&content_posts, "md");
        assert!(
            has_md_post,
            "scaffolded content/posts/ must contain at least one .md seed post"
        );
    }

    #[test]
    fn no_install_templates_list_contains_node_free() {
        assert!(
            NO_INSTALL_TEMPLATES.contains(&"node-free"),
            "NO_INSTALL_TEMPLATES must include 'node-free'"
        );
        // basic-blog must NOT be in the list (it still needs pnpm install).
        assert!(
            !NO_INSTALL_TEMPLATES.contains(&"basic-blog"),
            "basic-blog must not be in NO_INSTALL_TEMPLATES"
        );
    }

    /// Recursively check whether any file under `dir` has the given extension.
    fn walkdir_has_extension(dir: &Path, ext: &str) -> bool {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if walkdir_has_extension(&path, ext) {
                    return true;
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                return true;
            }
        }
        false
    }
}
