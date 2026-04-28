//! Adapter selection — dispatches the SSR-bundle hand-off to a
//! framework-agnostic deploy-target adapter.
//!
//! ## Why an adapter abstraction
//!
//! `zfb build` always runs the same SSG-render path. What changes per
//! deploy target is the SHAPE of the SSR worker entry the build needs
//! to emit alongside the static HTML:
//!
//! - Cloudflare Pages: `dist/_worker.js` (advanced mode) wrapping the
//!   ESM bundle with `(request, env, ctx) => Response`.
//! - Node SSR: `dist/server/index.mjs` exporting a Node `http.Server`
//!   handler.
//! - Netlify: `.netlify/functions/<name>.mjs` per route.
//!
//! Each of these lives in its own npm package (`@takazudo/zfb-adapter-*`)
//! that ships a thin CLI: `<bin> bundle <input> --outdir <dir>`. The
//! build orchestrator does not import the adapter directly — it shells
//! out to the CLI so adapters can be installed à la carte without
//! pulling them into the Rust build graph.
//!
//! This module is the dispatch shim. The choice of adapter comes from
//! `zfb.config.json`'s `adapter` field; the package name is opaque to
//! the dispatcher (it just runs `pnpm exec <bin> bundle …`).
//!
//! ## SSR-without-adapter is a hard error
//!
//! If any route exports `prerender = false` but the project did not
//! configure an adapter, the build cannot produce a deployable artifact
//! for that route. We surface this as an immediate error rather than
//! silently dropping the SSR routes — see [`ensure_no_ssr_without_adapter`].

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Adapter selection from `zfb.config.json`.
///
/// `None` corresponds to the default `adapter: "none"` (or omitted)
/// — the build emits pure-static `dist/` and rejects any route with
/// `prerender = false`. `Package(name)` selects an npm package whose
/// bin is invoked through `pnpm exec`.
///
/// Kept as a flat enum (not a trait) because the dispatch surface is
/// small (one `bundle` subcommand today) and the adapter-side logic
/// lives in the npm package, not in Rust. When a second hook lands
/// (e.g. `deploy`), promote this to a trait + per-adapter struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdapterChoice {
    /// No adapter configured. SSR routes will be rejected at build
    /// time.
    None,
    /// An npm-published adapter selected by package name (e.g.
    /// `@takazudo/zfb-adapter-cloudflare`).
    Package(String),
}

impl AdapterChoice {
    /// Parse the `adapter` field as written in `zfb.config.json`.
    ///
    /// Accepted shapes:
    /// - `None` / omitted → [`AdapterChoice::None`].
    /// - `Some("none")` → [`AdapterChoice::None`] (canonical "no adapter" form).
    /// - `Some("@scope/name")` or `Some("name")` → [`AdapterChoice::Package`].
    ///
    /// Empty / whitespace-only strings are rejected with a clear error
    /// so a typo in the config doesn't silently fall back to no-adapter.
    pub fn from_config(value: Option<&str>) -> Result<Self> {
        match value {
            None => Ok(AdapterChoice::None),
            Some(v) => {
                let trimmed = v.trim();
                if trimmed.is_empty() {
                    bail!(
                        "adapter: empty string is not a valid value. Use \"none\" to opt out, \
                         or a package name like \"@takazudo/zfb-adapter-cloudflare\"."
                    );
                }
                if trimmed == "none" {
                    return Ok(AdapterChoice::None);
                }
                Ok(AdapterChoice::Package(trimmed.to_string()))
            }
        }
    }

    /// True when this is `None`.
    pub fn is_none(&self) -> bool {
        matches!(self, AdapterChoice::None)
    }

    /// Return the package name when this is `Package(_)`.
    pub fn package_name(&self) -> Option<&str> {
        match self {
            AdapterChoice::Package(p) => Some(p.as_str()),
            AdapterChoice::None => None,
        }
    }
}

/// One SSR-only route the adapter dispatcher needs to know about.
///
/// Mirrors [`crate::renderer::SsrRouteEntry`] but the dispatcher uses
/// only `route_key` for the no-adapter error message — `url_path` is
/// included for symmetry with the renderer's manifest shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsrRouteRef<'a> {
    pub route_key: &'a str,
    pub url_path: &'a str,
}

/// Reject builds that have SSR routes but no adapter to handle them.
///
/// Returns `Err` with a message naming the offending route(s) so the
/// user can either remove `prerender = false` or wire up an adapter.
/// The error pinpoints the FIRST SSR route only — once the user fixes
/// that one (or wires the adapter), the build continues and any other
/// SSR routes are picked up at the same time.
pub fn ensure_no_ssr_without_adapter(
    adapter: &AdapterChoice,
    ssr_routes: &[SsrRouteRef<'_>],
) -> Result<()> {
    if !adapter.is_none() {
        return Ok(());
    }
    if ssr_routes.is_empty() {
        return Ok(());
    }
    let first = ssr_routes
        .first()
        .expect("checked non-empty above");
    let extra = if ssr_routes.len() > 1 {
        format!(" (and {} more)", ssr_routes.len() - 1)
    } else {
        String::new()
    };
    Err(anyhow!(
        "no adapter configured but route {route} requires SSR{extra}. \
         Either set `adapter` in zfb.config.json (e.g. \"@takazudo/zfb-adapter-cloudflare\") \
         or remove `export const prerender = false` from the page.",
        route = first.route_key,
        extra = extra,
    ))
}

/// Output of [`run_adapter_bundle`].
#[derive(Debug, Clone)]
pub struct AdapterBundleOutput {
    /// Stdout captured from the adapter CLI. Surfaced to the user so
    /// the per-adapter "wrote dist/_worker.js" lines are visible in the
    /// build log without us hard-coding the file shape.
    pub stdout: String,
    /// Stderr captured from the adapter CLI. Empty on a clean run.
    pub stderr: String,
}

/// Inputs to [`run_adapter_bundle`].
#[derive(Debug, Clone)]
pub struct AdapterBundleInput {
    /// Project root. The CLI is invoked with this as its CWD so
    /// `pnpm exec` resolves the bin from `node_modules/.bin/`.
    pub project_root: PathBuf,
    /// Path (absolute or project-relative) to the ESM bundle the
    /// adapter should wrap. Forwarded as a positional arg.
    pub input_bundle: PathBuf,
    /// Output directory the adapter writes its artifact(s) into.
    /// Typically the project's `dist/`.
    pub outdir: PathBuf,
}

/// Invoke the adapter package's CLI to wrap the SSR bundle.
///
/// The dispatcher does NOT know the per-adapter file shape (e.g.
/// Cloudflare's `_worker.js` vs Node's `server/index.mjs`). It only
/// pins the CLI contract:
///
/// ```text
/// <package-bin> bundle <input> --outdir <outdir>
/// ```
///
/// We invoke through `pnpm exec` so the user does not need to install
/// the adapter globally; pnpm resolves the bin from the workspace's
/// `node_modules/.bin/`. If the package is not installed, `pnpm exec`
/// surfaces a clean "command not found" error which we wrap in a
/// pointer at the offending package name.
///
/// `runner` is an indirection seam used in tests to assert the dispatch
/// shape without spawning a real subprocess. Production calls
/// [`run_adapter_bundle`] which threads in [`DefaultAdapterRunner`].
pub fn run_adapter_bundle(
    adapter: &AdapterChoice,
    input: AdapterBundleInput,
) -> Result<AdapterBundleOutput> {
    run_adapter_bundle_with(adapter, input, &DefaultAdapterRunner)
}

/// Indirection seam over `pnpm exec` so unit tests can verify the
/// dispatch shape without spawning a real subprocess.
pub trait AdapterRunner {
    fn run(
        &self,
        package: &str,
        input: &AdapterBundleInput,
    ) -> Result<AdapterBundleOutput>;
}

/// Production runner — shells out to `pnpm exec <bin>`.
pub struct DefaultAdapterRunner;

impl AdapterRunner for DefaultAdapterRunner {
    fn run(
        &self,
        package: &str,
        input: &AdapterBundleInput,
    ) -> Result<AdapterBundleOutput> {
        // The bin name follows the convention `<package>` for a non-
        // scoped name and `<short-name>` for a scoped one (the npm
        // standard: `@scope/zfb-adapter-cloudflare` ships
        // `bin: { "zfb-adapter-cloudflare": "..." }`). Rather than
        // re-derive the bin name here, we rely on `pnpm exec
        // <package>` which delegates to whatever the package's
        // `package.json` declared in its `bin` map.
        //
        // For scoped packages, pnpm's `exec` accepts the bin name (not
        // the package name). We extract the short name after the `/`
        // for scoped packages; non-scoped names pass through.
        let bin_name = bin_name_for_package(package);

        let mut cmd = Command::new("pnpm");
        cmd.current_dir(&input.project_root);
        cmd.arg("exec");
        cmd.arg(bin_name);
        cmd.arg("bundle");
        cmd.arg(&input.input_bundle);
        cmd.arg("--outdir");
        cmd.arg(&input.outdir);

        let output = cmd.output().with_context(|| {
            format!(
                "spawning `pnpm exec {bin_name} bundle ...` for adapter {package}"
            )
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        if !output.status.success() {
            bail!(
                "adapter `{package}` failed (exit {status}): {stderr}\n\
                 Hint: ensure the package is installed in the workspace \
                 (e.g. `pnpm add -D {package}`).",
                status = output.status,
                stderr = stderr.trim(),
            );
        }

        Ok(AdapterBundleOutput { stdout, stderr })
    }
}

/// Drive the dispatch through a custom [`AdapterRunner`]. Production
/// calls [`run_adapter_bundle`] which uses [`DefaultAdapterRunner`].
pub fn run_adapter_bundle_with<R: AdapterRunner>(
    adapter: &AdapterChoice,
    input: AdapterBundleInput,
    runner: &R,
) -> Result<AdapterBundleOutput> {
    let package = match adapter {
        AdapterChoice::None => bail!(
            "run_adapter_bundle called with AdapterChoice::None — \
             callers should branch on `is_none()` first"
        ),
        AdapterChoice::Package(p) => p.as_str(),
    };
    if !input.project_root.is_dir() {
        bail!(
            "adapter dispatch: project_root does not exist or is not a directory: {}",
            input.project_root.display()
        );
    }
    if !input.input_bundle.is_file() {
        bail!(
            "adapter dispatch: input bundle does not exist: {}",
            input.input_bundle.display()
        );
    }
    runner.run(package, &input)
}

/// Derive the bin name to invoke for a given adapter package.
///
/// Scoped packages (`@scope/foo`) → `foo`. Non-scoped → the name
/// verbatim. The convention is documented in the
/// `@takazudo/zfb-adapter-*` packages' `package.json` (`bin` key).
fn bin_name_for_package(package: &str) -> &str {
    if let Some(stripped) = package.strip_prefix('@') {
        if let Some(slash) = stripped.find('/') {
            return &stripped[slash + 1..];
        }
    }
    package
}

/// Small utility used by [`Path`]-aware callers.
#[allow(dead_code)]
pub(crate) fn path_to_string_lossy(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use tempfile::tempdir;

    #[test]
    fn from_config_defaults_to_none_when_omitted() {
        assert_eq!(
            AdapterChoice::from_config(None).unwrap(),
            AdapterChoice::None
        );
    }

    #[test]
    fn from_config_canonical_none_string_yields_none() {
        assert_eq!(
            AdapterChoice::from_config(Some("none")).unwrap(),
            AdapterChoice::None
        );
    }

    #[test]
    fn from_config_package_name_yields_package() {
        assert_eq!(
            AdapterChoice::from_config(Some("@takazudo/zfb-adapter-cloudflare")).unwrap(),
            AdapterChoice::Package("@takazudo/zfb-adapter-cloudflare".to_string()),
        );
    }

    #[test]
    fn from_config_empty_string_is_rejected() {
        let err = AdapterChoice::from_config(Some("   ")).unwrap_err();
        assert!(format!("{err}").contains("empty string"));
    }

    #[test]
    fn ensure_no_ssr_without_adapter_passes_when_no_routes() {
        let res = ensure_no_ssr_without_adapter(&AdapterChoice::None, &[]);
        assert!(res.is_ok());
    }

    #[test]
    fn ensure_no_ssr_without_adapter_passes_when_adapter_set_even_with_routes() {
        let route = SsrRouteRef {
            route_key: "/api/foo",
            url_path: "/api/foo",
        };
        let res = ensure_no_ssr_without_adapter(
            &AdapterChoice::Package("anything".into()),
            &[route],
        );
        assert!(res.is_ok());
    }

    #[test]
    fn ensure_no_ssr_without_adapter_errors_when_routes_present_and_no_adapter() {
        let routes = vec![
            SsrRouteRef {
                route_key: "/api/foo",
                url_path: "/api/foo",
            },
            SsrRouteRef {
                route_key: "/api/bar",
                url_path: "/api/bar",
            },
        ];
        let err = ensure_no_ssr_without_adapter(&AdapterChoice::None, &routes).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("/api/foo"), "{msg}");
        assert!(msg.contains("and 1 more"), "{msg}");
        assert!(msg.contains("@takazudo/zfb-adapter-cloudflare"), "{msg}");
    }

    #[test]
    fn bin_name_for_package_handles_scoped_and_unscoped() {
        assert_eq!(
            bin_name_for_package("@takazudo/zfb-adapter-cloudflare"),
            "zfb-adapter-cloudflare"
        );
        assert_eq!(bin_name_for_package("zfb-adapter-foo"), "zfb-adapter-foo");
        assert_eq!(bin_name_for_package("@scope/with/slashes"), "with/slashes");
    }

    /// Fake runner that records dispatch arguments so the callers's
    /// orchestration can be asserted without spawning pnpm.
    struct FakeRunner {
        calls: RefCell<Vec<(String, AdapterBundleInput)>>,
    }
    impl FakeRunner {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
            }
        }
    }
    impl AdapterRunner for FakeRunner {
        fn run(
            &self,
            package: &str,
            input: &AdapterBundleInput,
        ) -> Result<AdapterBundleOutput> {
            self.calls
                .borrow_mut()
                .push((package.to_string(), input.clone()));
            Ok(AdapterBundleOutput {
                stdout: format!("fake adapter {package} ok\n"),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn run_adapter_bundle_with_dispatches_to_runner() {
        let tmp = tempdir().unwrap();
        let bundle = tmp.path().join("bundle.mjs");
        std::fs::write(&bundle, "// stub\n").unwrap();
        let input = AdapterBundleInput {
            project_root: tmp.path().to_path_buf(),
            input_bundle: bundle.clone(),
            outdir: tmp.path().join("dist"),
        };
        let runner = FakeRunner::new();
        let out = run_adapter_bundle_with(
            &AdapterChoice::Package("@takazudo/zfb-adapter-cloudflare".into()),
            input.clone(),
            &runner,
        )
        .unwrap();
        assert!(out.stdout.contains("fake adapter"));
        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "@takazudo/zfb-adapter-cloudflare");
        assert_eq!(calls[0].1.input_bundle, bundle);
    }

    #[test]
    fn run_adapter_bundle_with_rejects_none_choice() {
        let tmp = tempdir().unwrap();
        let bundle = tmp.path().join("b.mjs");
        std::fs::write(&bundle, "//").unwrap();
        let input = AdapterBundleInput {
            project_root: tmp.path().to_path_buf(),
            input_bundle: bundle,
            outdir: tmp.path().join("dist"),
        };
        let runner = FakeRunner::new();
        let err = run_adapter_bundle_with(&AdapterChoice::None, input, &runner).unwrap_err();
        assert!(format!("{err}").contains("AdapterChoice::None"));
    }

    #[test]
    fn run_adapter_bundle_with_rejects_missing_input_bundle() {
        let tmp = tempdir().unwrap();
        let input = AdapterBundleInput {
            project_root: tmp.path().to_path_buf(),
            input_bundle: tmp.path().join("does-not-exist.mjs"),
            outdir: tmp.path().join("dist"),
        };
        let runner = FakeRunner::new();
        let err = run_adapter_bundle_with(
            &AdapterChoice::Package("anything".into()),
            input,
            &runner,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("does not exist"));
    }
}
