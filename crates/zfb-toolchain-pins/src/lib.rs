//! Pinned external tool versions for the zfb toolchain.
//!
//! This crate is the **single source of truth** for the version strings of the
//! external CLI tools that `zfb` drives at runtime. All other crates that
//! need to compare or display these versions import from here rather than
//! duplicating the strings.
//!
//! ## Bump procedure
//!
//! 1. Update the three constants below.
//! 2. Bring the root `package.json` `devDependencies` in sync (wrangler pin).
//! 3. Run `cargo build --workspace` to catch any compile-time consumers that
//!    were relying on the old string value.
//!
//! See `CONTRIBUTING.md "External tool version pins"` for the full runbook.

/// Pinned `wrangler` CLI version. `zfb preview` runs
/// `pnpm exec wrangler --version` before handing off to
/// `wrangler pages dev` and aborts with a clear error if the reported
/// version does not match this constant.
///
/// Kept in lock-step with the exact-pinned `wrangler` entry in the root
/// `package.json` (and with [`EXPECTED_MINIFLARE_VERSION`] /
/// [`EXPECTED_WORKERD_VERSION`] below) so the SSR/preview pipeline is
/// reproducible. To bump, see `CONTRIBUTING.md "External tool version pins"`.
pub const EXPECTED_WRANGLER_VERSION: &str = "4.85.0";

/// Pinned `miniflare` package version. zfb does not spawn miniflare
/// directly today (it goes through `pnpm exec wrangler pages dev`),
/// but the version is recorded here alongside the wrangler and workerd
/// pins so a single crate grep surfaces all three. Miniflare is a
/// `wrangler` internal dep; this pin tracks the transitive resolution
/// in the lockfile so CI can catch silent upgrades.
pub const EXPECTED_MINIFLARE_VERSION: &str = "4.20260424.0";

/// Pinned `workerd` package version that miniflare drives. Not
/// controlled directly (workerd is miniflare's peer dependency /
/// `miniflare`'s peer dependency); the lockfile snapshots the exact
/// resolved version. Kept here so a single `grep EXPECTED_WORKERD_VERSION`
/// surfaces the workerd pin alongside the rest of the external-tool
/// version pins. To bump, see `CONTRIBUTING.md "External tool version pins"`.
pub const EXPECTED_WORKERD_VERSION: &str = "1.20260424.1";
