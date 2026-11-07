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

/// Tested `wrangler` CLI baseline, enforced by `zfb preview` as the
/// **minimum supported version** (issue 2379 relaxed this from an
/// exact-equality pin): before handing off to `wrangler dev`, preview
/// runs `pnpm exec wrangler --version` and aborts with a
/// consumer-actionable error only when the reported version is *older*
/// than this constant. An equal version passes silently; a newer one
/// proceeds with an info line (or a warning on an untested major).
///
/// Kept in lock-step with the exact-pinned `wrangler` entry in the root
/// `package.json` (and with [`EXPECTED_WORKERD_VERSION`] below) so this
/// repo's own CI and e2e runs stay reproducible. To bump, see
/// `CONTRIBUTING.md "External tool version pins"`.
pub const EXPECTED_WRANGLER_VERSION: &str = "4.85.0";

/// Pinned `workerd` package version. Not controlled directly (workerd
/// is a transitive dep of wrangler); the lockfile snapshots the exact
/// resolved version. Kept here so a single `grep EXPECTED_WORKERD_VERSION`
/// surfaces the workerd pin alongside the rest of the external-tool
/// version pins. To bump, see `CONTRIBUTING.md "External tool version pins"`.
pub const EXPECTED_WORKERD_VERSION: &str = "1.20260424.1";
