//! Pinned external tool versions for the zfb toolchain.
//!
//! This crate is the **single source of truth** for the version strings of the
//! external CLI tools that `zfb` drives at runtime. All other crates that
//! need to compare or display these versions import from here rather than
//! duplicating the strings.
//!
//! ## Bump procedure
//!
//! 1. Update the constants below.
//! 2. Bring the root `package.json` `devDependencies` in sync (wrangler pin).
//! 3. Run `cargo build --workspace` to catch any compile-time consumers that
//!    were relying on the old string value.
//!
//! See `CONTRIBUTING.md "External tool version pins"` for the full runbook.

/// Pinned `wrangler` CLI version. `zfb preview` runs
/// `pnpm exec wrangler --version` before handing off to
/// `wrangler dev` and aborts with a clear error if the reported
/// version does not match this constant.
///
/// Kept in lock-step with the exact-pinned `wrangler` entry in the root
/// `package.json` (and with [`EXPECTED_WORKERD_VERSION`] below) so the
/// preview pipeline is reproducible. To bump, see `CONTRIBUTING.md
/// "External tool version pins"`.
pub const EXPECTED_WRANGLER_VERSION: &str = "4.85.0";

/// Pinned `workerd` package version. Not controlled directly (workerd
/// is a transitive dep of wrangler); the lockfile snapshots the exact
/// resolved version. Kept here so a single `grep EXPECTED_WORKERD_VERSION`
/// surfaces the workerd pin alongside the rest of the external-tool
/// version pins. To bump, see `CONTRIBUTING.md "External tool version pins"`.
pub const EXPECTED_WORKERD_VERSION: &str = "1.20260424.1";

/// Pinned `esbuild` CLI version. `zfb-islands` runs `esbuild --version`
/// before each bundle and aborts with a clear error if the reported version
/// does not match this constant. `crates/zfb/build.rs` uses this string to
/// construct the npm registry download URL at compile time.
///
/// To bump, follow the "External tool version pins" procedure in
/// `CONTRIBUTING.md` at the workspace root: update this constant, refresh
/// `EXPECTED_ESBUILD_SHA256` in `crates/zfb-islands/src/esbuild.rs` and the
/// SHA-256 table in `crates/zfb/build.rs`, then drop the new binary under
/// `crates/zfb/binaries/esbuild/esbuild`.
pub const EXPECTED_ESBUILD_VERSION: &str = "0.25.12";
