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

// ---------------------------------------------------------------------------
// Vendor binary override policy (issue #1772)
// ---------------------------------------------------------------------------
//
// `crates/zfb/build.rs` stages two vendor binaries (esbuild, tailwindcss-v4)
// into the embedded vendor snapshot, each either downloaded into a
// platform-specific slot or overridden via `ZFB_ESBUILD_BIN` /
// `ZFB_TAILWIND_BIN`. The types and function below are the **pure** decision
// logic for "where does this binary's bytes come from" — build.rs collects
// the facts (env var value, resolved `TARGET`, whether the on-disk slot
// already matches the pinned SHA-256) and hands them here; all filesystem
// I/O, path validation, and error-message formatting stay in build.rs.

/// The five vendor binary platforms zfb's download-and-slot flow supports,
/// matched by **exact** Rust target triple rather than a substring match —
/// a substring match on `x86_64-unknown-linux` would also match
/// `x86_64-unknown-linux-musl`, which is not `-gnu` and has no pinned SHA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VendorPlatform {
    LinuxX64Gnu,
    LinuxArm64Gnu,
    MacosArm64,
    MacosX64,
    Win32X64Msvc,
}

impl VendorPlatform {
    /// Resolve a `VendorPlatform` from Cargo's `TARGET` build-script env
    /// var. Returns `None` for any target outside the five supported
    /// triples (e.g. musl, or any triple the download flow hasn't been
    /// extended to) — those targets can still build if every binary is
    /// supplied via an override.
    pub fn from_target_triple(target: &str) -> Option<Self> {
        match target {
            "x86_64-unknown-linux-gnu" => Some(Self::LinuxX64Gnu),
            "aarch64-unknown-linux-gnu" => Some(Self::LinuxArm64Gnu),
            "aarch64-apple-darwin" => Some(Self::MacosArm64),
            "x86_64-apple-darwin" => Some(Self::MacosX64),
            "x86_64-pc-windows-msvc" => Some(Self::Win32X64Msvc),
            _ => None,
        }
    }

    /// The filename suffix applied to the embedded vendor binary name for
    /// this platform (`.exe` on Windows, empty everywhere else).
    pub fn exe_suffix(&self) -> &'static str {
        match self {
            Self::Win32X64Msvc => ".exe",
            _ => "",
        }
    }
}

/// The `.exe`-or-not suffix for the embedded vendor binary name, derived
/// from Cargo's resolved `TARGET` triple — **not** the host's
/// `cfg!(target_os)`. Build scripts run on the host, so `cfg!(target_os)`
/// there describes the machine doing the compiling, not the machine the
/// compiled binary will run on; using it to name a cross-compiled vendor
/// entry would silently mis-name the file that `embedded_binary()`
/// (`crates/zfb/src/render_pipeline.rs`) looks up at runtime on the
/// *target* machine.
///
/// Falls back to a substring check on `target` for triples
/// `VendorPlatform` doesn't recognize (e.g. a not-yet-supported Windows
/// triple), so an override-only build on an unsupported platform still
/// gets a plausible embedded filename.
pub fn exe_suffix_for_target(target: &str) -> &'static str {
    if let Some(platform) = VendorPlatform::from_target_triple(target) {
        return platform.exe_suffix();
    }
    if target.contains("windows") {
        ".exe"
    } else {
        ""
    }
}

/// Where a single vendor binary's bytes should come from, decided purely
/// from already-collected facts (no I/O performed here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinarySource {
    /// The override env var was set to a non-empty value. Carries the raw
    /// (unvalidated) path string — build.rs is responsible for requiring
    /// it to be absolute, existing, and a regular file before staging it.
    Override(String),
    /// No override; the existing slot already holds a binary matching the
    /// pinned SHA-256 — no download needed.
    Slot,
    /// No override; the slot is missing or doesn't match the pinned
    /// SHA-256 — build.rs must download it.
    NeedsDownload,
    /// No override, and the resolved target isn't one of the platforms the
    /// download-and-slot flow supports. build.rs treats this as a hard
    /// error unless every binary that reaches this state has its own
    /// override.
    Unsupported,
}

/// Decide a single binary's source from three independent, already-known
/// facts:
///
/// - `override_env_value`: the raw env var value, if the var is set at all
///   (`None` = unset). An empty string is treated the same as unset, per
///   the documented override contract in `BUILDING.md`.
/// - `platform_supported`: whether `VendorPlatform::from_target_triple`
///   resolved the current `TARGET`.
/// - `slot_already_correct`: whether the existing slot file (if any)
///   already matches the pinned SHA-256. Irrelevant (and ignored) once an
///   override wins, or when the platform is unsupported.
pub fn resolve_binary_source(
    override_env_value: Option<&str>,
    platform_supported: bool,
    slot_already_correct: bool,
) -> BinarySource {
    if let Some(value) = override_env_value {
        if !value.is_empty() {
            return BinarySource::Override(value.to_string());
        }
    }
    if !platform_supported {
        return BinarySource::Unsupported;
    }
    if slot_already_correct {
        BinarySource::Slot
    } else {
        BinarySource::NeedsDownload
    }
}

#[cfg(test)]
mod override_policy_tests {
    use super::*;

    #[test]
    fn valid_override_wins_regardless_of_platform_or_slot_state() {
        assert_eq!(
            resolve_binary_source(Some("/abs/path/to/esbuild"), true, false),
            BinarySource::Override("/abs/path/to/esbuild".to_string())
        );
        assert_eq!(
            resolve_binary_source(Some("/abs/path/to/esbuild"), true, true),
            BinarySource::Override("/abs/path/to/esbuild".to_string())
        );
        // Wins even when the platform is unsupported — this is exactly the
        // escape hatch overrides exist for.
        assert_eq!(
            resolve_binary_source(Some("/abs/path/to/esbuild"), false, false),
            BinarySource::Override("/abs/path/to/esbuild".to_string())
        );
    }

    #[test]
    fn unsupported_synthetic_target_with_no_override_is_unsupported() {
        let synthetic_target = "riscv64gc-unknown-linux-gnu";
        let supported = VendorPlatform::from_target_triple(synthetic_target).is_some();
        assert!(!supported);
        assert_eq!(
            resolve_binary_source(None, supported, false),
            BinarySource::Unsupported
        );
    }

    #[test]
    fn unsupported_synthetic_target_with_override_still_resolves() {
        let synthetic_target = "riscv64gc-unknown-linux-gnu";
        let supported = VendorPlatform::from_target_triple(synthetic_target).is_some();
        assert_eq!(
            resolve_binary_source(Some("/abs/riscv-esbuild"), supported, false),
            BinarySource::Override("/abs/riscv-esbuild".to_string())
        );
    }

    #[test]
    fn musl_targets_are_not_matched_by_the_gnu_platform() {
        assert_eq!(
            VendorPlatform::from_target_triple("x86_64-unknown-linux-musl"),
            None
        );
        assert_eq!(
            VendorPlatform::from_target_triple("aarch64-unknown-linux-musl"),
            None
        );
        // The gnu siblings still match exactly, proving this isn't a
        // regression in the gnu triples themselves.
        assert_eq!(
            VendorPlatform::from_target_triple("x86_64-unknown-linux-gnu"),
            Some(VendorPlatform::LinuxX64Gnu)
        );
        assert_eq!(
            VendorPlatform::from_target_triple("aarch64-unknown-linux-gnu"),
            Some(VendorPlatform::LinuxArm64Gnu)
        );
    }

    #[test]
    fn empty_string_env_value_is_treated_as_unset() {
        assert_eq!(
            resolve_binary_source(Some(""), true, true),
            BinarySource::Slot
        );
        assert_eq!(
            resolve_binary_source(Some(""), true, false),
            BinarySource::NeedsDownload
        );
        assert_eq!(
            resolve_binary_source(Some(""), false, false),
            BinarySource::Unsupported
        );
    }

    #[test]
    fn no_override_supported_platform_slot_already_correct() {
        assert_eq!(resolve_binary_source(None, true, true), BinarySource::Slot);
    }

    #[test]
    fn no_override_supported_platform_needs_download() {
        assert_eq!(
            resolve_binary_source(None, true, false),
            BinarySource::NeedsDownload
        );
    }

    #[test]
    fn mixed_override_and_download_are_independent_per_binary() {
        // esbuild overridden, tailwind still needs a download — the
        // documented per-binary independence contract.
        let esbuild = resolve_binary_source(Some("/abs/esbuild"), true, false);
        let tailwind = resolve_binary_source(None, true, false);
        assert_eq!(esbuild, BinarySource::Override("/abs/esbuild".to_string()));
        assert_eq!(tailwind, BinarySource::NeedsDownload);
    }

    #[test]
    fn all_five_supported_triples_resolve_exactly() {
        assert_eq!(
            VendorPlatform::from_target_triple("x86_64-unknown-linux-gnu"),
            Some(VendorPlatform::LinuxX64Gnu)
        );
        assert_eq!(
            VendorPlatform::from_target_triple("aarch64-unknown-linux-gnu"),
            Some(VendorPlatform::LinuxArm64Gnu)
        );
        assert_eq!(
            VendorPlatform::from_target_triple("aarch64-apple-darwin"),
            Some(VendorPlatform::MacosArm64)
        );
        assert_eq!(
            VendorPlatform::from_target_triple("x86_64-apple-darwin"),
            Some(VendorPlatform::MacosX64)
        );
        assert_eq!(
            VendorPlatform::from_target_triple("x86_64-pc-windows-msvc"),
            Some(VendorPlatform::Win32X64Msvc)
        );
    }

    #[test]
    fn exe_suffix_only_applies_to_windows() {
        assert_eq!(VendorPlatform::Win32X64Msvc.exe_suffix(), ".exe");
        assert_eq!(VendorPlatform::LinuxX64Gnu.exe_suffix(), "");
        assert_eq!(VendorPlatform::LinuxArm64Gnu.exe_suffix(), "");
        assert_eq!(VendorPlatform::MacosArm64.exe_suffix(), "");
        assert_eq!(VendorPlatform::MacosX64.exe_suffix(), "");
    }

    #[test]
    fn exe_suffix_for_target_falls_back_to_substring_for_unrecognized_windows_triple() {
        assert_eq!(exe_suffix_for_target("x86_64-pc-windows-msvc"), ".exe");
        assert_eq!(
            exe_suffix_for_target("aarch64-pc-windows-msvc"),
            ".exe",
            "not one of the 5 supported triples, but still a windows target"
        );
        assert_eq!(exe_suffix_for_target("x86_64-unknown-linux-musl"), "");
    }
}
