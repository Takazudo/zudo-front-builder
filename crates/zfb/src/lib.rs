//! zfb — zudo-front-builder library crate.
//!
//! This crate exposes the CLI parser, command dispatchers, configuration
//! loader, and structured output helpers used by the `zfb` binary.

pub mod bounded_join;
pub mod cli;
pub mod commands;
pub mod config;
pub mod diagnostics;
pub(crate) mod output;
pub mod render_pipeline;
// V8-bearing adapters (issue #371, sub-task 4.1a). Compiled in only
// when the `embed_v8` cargo feature is on; without the feature, the
// dispatch sites in `commands::dev` / `commands::build` /
// `render_pipeline` fall back to feature-gated stubs that fail loudly
// if a project unexpectedly hits an SSR-requiring path.
#[cfg(feature = "embed_v8")]
pub(crate) mod dev_rewrite_prewarm;
#[cfg(feature = "embed_v8")]
pub(crate) mod lazy_render_adapter;
#[cfg(feature = "embed_v8")]
pub(crate) mod ssr_adapter;
#[cfg(feature = "embed_v8")]
pub(crate) mod v8_host_adapter;

// Re-export the public dynamic-route planning types so adapter authors can
// build on them without pulling in the `render_pipeline` module directly.
pub use render_pipeline::{DeferredDynamicRoute, PendingDynamicRoute};

/// Render and print an [`anyhow::Error`] to stderr, prefixed with a red `✗`.
///
/// Used by the `zfb` binary entry point to report top-level command failures.
/// Wraps [`output::format_error`] + [`output::error`] so the binary does not
/// need to import the internal `output` module directly.
pub fn report_error(err: &anyhow::Error) {
    output::error(output::format_error(err).trim_end());
}

#[cfg(test)]
mod tailwind_version_parity {
    use zfb_toolchain_pins::EXPECTED_TAILWIND_VERSION;

    /// Assert that the three Tailwind version pins — the shared Rust const,
    /// `scripts/fetch-tailwind.mjs`, and `crates/zfb-css/README.md` — all carry
    /// the same version string.
    ///
    /// When bumping the pin, update all three in the same commit.  If this test
    /// fails after a single-file bump it is a reminder that the other two still
    /// need updating.
    #[test]
    fn tailwind_version_pins_are_in_sync() {
        // CARGO_MANIFEST_DIR is the path of this crate's Cargo.toml.
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();

        // 1. Read the canonical Rust-side pin from zfb-toolchain-pins.
        let pinned_ver = EXPECTED_TAILWIND_VERSION;

        // 2. Extract version from scripts/fetch-tailwind.mjs: `const TAILWIND_VERSION = "x.y.z";`
        let mjs = std::fs::read_to_string(workspace_root.join("scripts/fetch-tailwind.mjs"))
            .expect("read scripts/fetch-tailwind.mjs");
        let mjs_ver = extract_quoted_value(&mjs, "const TAILWIND_VERSION = ")
            .expect("TAILWIND_VERSION not found in scripts/fetch-tailwind.mjs");

        // 3. Extract version from crates/zfb-css/README.md:
        //    `| Pinned version        | **`x.y.z`**`
        let readme = std::fs::read_to_string(workspace_root.join("crates/zfb-css/README.md"))
            .expect("read crates/zfb-css/README.md");
        let readme_ver = readme
            .lines()
            .find(|l| l.contains("Pinned version"))
            .and_then(|l| {
                // Match the bold backtick-wrapped version: **`x.y.z`**
                let start = l.find("**`")?;
                let rest = &l[start + 3..];
                let end = rest.find("`**")?;
                Some(rest[..end].to_string())
            })
            .expect("Pinned version not found in crates/zfb-css/README.md");

        assert_eq!(
            pinned_ver, mjs_ver,
            "Tailwind version in zfb-toolchain-pins ({pinned_ver}) differs from scripts/fetch-tailwind.mjs ({mjs_ver})"
        );
        assert_eq!(
            pinned_ver, readme_ver,
            "Tailwind version in zfb-toolchain-pins ({pinned_ver}) differs from crates/zfb-css/README.md ({readme_ver})"
        );
    }

    /// Extract the first double-quoted string value after `prefix` on any line.
    fn extract_quoted_value(src: &str, prefix: &str) -> Option<String> {
        for line in src.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let rest = rest.trim_start().strip_prefix('"')?;
                let end = rest.find('"')?;
                return Some(rest[..end].to_string());
            }
        }
        None
    }
}
