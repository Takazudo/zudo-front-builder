//! Cargo build script for the `zfb` binary crate.
//!
//! This script downloads and stages the pinned esbuild and tailwindcss v4
//! standalone binaries into `crates/zfb/binaries/` so that `cargo install zfb`
//! works on a machine with no pnpm or Node.js.
//!
//! ## Design notes
//!
//! * **Idempotent.** If a binary already exists at its slot path and its
//!   SHA-256 matches the pinned constant, the download is skipped. Re-runs
//!   (incremental cargo builds) are a fast no-op.
//! * **Escape hatches.** If `ZFB_ESBUILD_BIN` or `ZFB_TAILWIND_BIN` is set to
//!   a non-empty absolute path, that binary is staged directly into the
//!   vendor snapshot in place of a download — each binary resolves its
//!   source **independently**, so one can be overridden while the other
//!   still downloads. Overrides skip SHA-256 pinning entirely (documented
//!   trust boundary — see `BUILDING.md`). The pure decision of which source
//!   each binary uses lives in `zfb_toolchain_pins::resolve_binary_source`
//!   (unit-tested there); this file only does the I/O and validation.
//! * **Hard SHA mismatch.** A checksum mismatch on a downloaded binary is a
//!   build failure — we never silently accept a binary whose hash differs from
//!   the pinned constant.
//! * **Network unavailable.** A network error produces a clear, actionable
//!   error message pointing the user at the escape-hatch env vars.
//! * **Extensible.** `main()` calls `download_binaries()` so sub-#198 (runtime
//!   embedding) can add its own top-level function without restructuring this
//!   file.
//!
//! ## Version / SHA-256 pins
//!
//! esbuild version : single source of truth is `crates/zfb-toolchain-pins/src/lib.rs`
//!                   (`EXPECTED_ESBUILD_VERSION`). Consumed here via the
//!                   `zfb-toolchain-pins` build-dependency — no local copy.
//! tailwindcss ver : pinned in `scripts/fetch-tailwind.mjs` (`TAILWIND_VERSION`).
//!                   **Must be kept in sync with the constants in this file.**
//!
//! When bumping the esbuild pin, update `crates/zfb-toolchain-pins/src/lib.rs`
//! and the SHA-256 table below in the same commit. For tailwindcss, update the
//! source-of-truth file and the SHA-256 table below in the same commit.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Version pins
// ---------------------------------------------------------------------------

/// Pinned esbuild version. Imported from `zfb-toolchain-pins`, the single
/// source of truth for all external tool version pins. To bump, update
/// `crates/zfb-toolchain-pins/src/lib.rs` and the SHA-256 table below.
use zfb_toolchain_pins::{
    exe_suffix_for_target, resolve_binary_source, BinarySource, VendorPlatform,
    EXPECTED_ESBUILD_VERSION,
};

/// Pinned tailwindcss v4 version.  Mirror of `TAILWIND_VERSION` in
/// `scripts/fetch-tailwind.mjs` — must be kept in sync.
const TAILWIND_VERSION: &str = "4.2.0";

// ---------------------------------------------------------------------------
// Framework package version pins (sub #209 — embed framework runtimes)
//
// `preact`, `preact-render-to-string`, and `hono` are the runtime framework
// packages a `zfb`-built app imports at bundle time. They are NOT
// downloaded by `build.rs`; instead, zfb's own `pnpm install` resolves the
// versions in `pnpm-lock.yaml`, and `embed_framework_packages()` copies the
// resolved trees from `node_modules/.pnpm/<name>@<ver>*/node_modules/<name>`
// into `$OUT_DIR/vendor/<name>/`. The constants below are the contract
// between zfb and its consumers — bump these whenever you bump the
// corresponding entry in zfb's `pnpm-lock.yaml`.
//
// Source of truth: zfb's own `pnpm-lock.yaml`. To verify alignment, run:
//
//   pnpm list preact preact-render-to-string hono --depth 0 -r
//
// from the workspace root and confirm the output matches the constants below.
// ---------------------------------------------------------------------------

/// Pinned `preact` version. Mirror of the `preact` entry in `pnpm-lock.yaml`.
const PREACT_VERSION: &str = "10.29.1";

/// Pinned `preact-render-to-string` version.
const PREACT_RTS_VERSION: &str = "6.6.7";

/// Pinned `hono` version (transitive dep of `@takazudo/zfb-runtime`).
const HONO_VERSION: &str = "4.12.25";

// ---------------------------------------------------------------------------
// SHA-256 constants — esbuild 0.25.12 (from the npm package binary)
//
// These are SHA-256 digests of the *extracted* esbuild binary inside each
// platform-specific npm package tarball (e.g. `@esbuild/linux-x64`).
// They are **not** the digest of the .tgz itself.
//
// To reproduce:
//   curl -sL https://registry.npmjs.org/@esbuild/linux-x64/-/linux-x64-0.25.12.tgz \
//     | tar -Oxz package/bin/esbuild | sha256sum
//
// Verified on 2026-05-05 against npm registry.
// ---------------------------------------------------------------------------
const ESBUILD_SHA256_LINUX_X64: &str =
    "bab29b2ca7a9e89b67cf720b77b2d743f9f31f5cf0d5bd74ee8c8de30ced7014";
const ESBUILD_SHA256_LINUX_ARM64: &str =
    "840ad255d6fd587b126d8b2d59ab506d8562785b9bc76249dc3b0e1bdd2ca449";
const ESBUILD_SHA256_MACOS_ARM64: &str =
    "3e030ee2aa86ad3c33e5e95ae0e53bb03de40e0da35c9b1180a67de4a497cae5";
const ESBUILD_SHA256_MACOS_X64: &str =
    "bd09e65a6a1a903c40269d3a4ae23ffc6139f691703728c1faf25f62e48baa40";
const ESBUILD_SHA256_WIN_X64: &str =
    "cae1bbc86f4df800b01d99e28aea0a154b02243de6797e98f48a9b88a64a7be0";

// ---------------------------------------------------------------------------
// SHA-256 constants — tailwindcss 4.2.0 (from GitHub release sha256sums.txt)
//
// Source:
//   https://github.com/tailwindlabs/tailwindcss/releases/download/v4.2.0/sha256sums.txt
//
// Verified on 2026-05-05.
// ---------------------------------------------------------------------------
const TAILWIND_SHA256_LINUX_X64: &str =
    "8f65e2d21c675f1e8d265219979d17d10634c1f553a2f583265b7edb28726432";
const TAILWIND_SHA256_LINUX_ARM64: &str =
    "376fd4da2c29eb81ae0638cd2f84a4304af92532f2f1576555f41bdb44c185da";
const TAILWIND_SHA256_MACOS_ARM64: &str =
    "d9e759fd6612dd442a9caa49d366b24e5097ea9802d35829da3f6db6ee5c2043";
const TAILWIND_SHA256_MACOS_X64: &str =
    "18cd6bb94d0f26ff8a0fa8a966beb9ea36bea2c7c444397f7619a2b880260e65";
const TAILWIND_SHA256_WIN_X64: &str =
    "3ee303c62115af89d1036da8a945cd51bdca653f39634e437358c17a3d3fbbc7";

// ---------------------------------------------------------------------------
// Platform detection
// ---------------------------------------------------------------------------
//
// `VendorPlatform` (matched against Cargo's `TARGET` env var by **exact**
// triple, not substring) lives in `zfb-toolchain-pins` so its policy
// interactions with the override env vars can be unit-tested in that crate.
// See `resolve_vendor_source` below for how it's combined with an override
// env var and the on-disk slot state into a `BinarySource` decision.

/// Build the error message for a binary that ended up `BinarySource::Unsupported`
/// — no override was given for it, and `target` isn't one of the platforms the
/// download-and-slot flow supports.
fn unsupported_target_message(target: &str) -> String {
    format!(
        "unsupported target triple `{target}`. \
         Supported targets: x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, \
         aarch64-apple-darwin, x86_64-apple-darwin, x86_64-pc-windows-msvc. \
         Set ZFB_ESBUILD_BIN and/or ZFB_TAILWIND_BIN to absolute paths of \
         pre-verified binaries to proceed on an unsupported platform — each \
         binary resolves its source independently, so only the binaries \
         lacking a supported platform need an override."
    )
}

// ---------------------------------------------------------------------------
// SHA-256 helpers
// ---------------------------------------------------------------------------

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Compute the SHA-256 of the file at `path` by reading it in 64 KiB chunks,
/// avoiding a full in-memory buffer for large files (e.g. the ~75 MB tailwind
/// binary).
fn sha256_hex_file(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Return `true` if `path` exists and its SHA-256 matches `expected_hex`.
fn binary_already_correct(path: &Path, expected_hex: &str) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    sha256_hex(&bytes).eq_ignore_ascii_case(expected_hex)
}

// ---------------------------------------------------------------------------
// HTTP download helper
// ---------------------------------------------------------------------------
//
// Downloads are handled by `zfb_binfetch::fetch_to_file`, which streams each
// URL directly to a caller-supplied temp path with retry (5 attempts by
// default), connect + overall timeouts, and automatic cleanup of partial
// writes on failure. The caller owns SHA-256 verification, chmod, and atomic
// rename into the final slot. See `crates/zfb-binfetch/src/lib.rs` for the
// full implementation.

// ---------------------------------------------------------------------------
// esbuild download
// ---------------------------------------------------------------------------

/// Platform-specific metadata for the esbuild npm package.
struct EsbuildPlatformMeta {
    /// npm package name, e.g. `@esbuild/linux-x64`.
    npm_pkg: &'static str,
    /// Path inside the tarball where the binary lives.
    tarball_binary_path: &'static str,
    /// Expected SHA-256 of the *extracted* binary (not the tarball).
    expected_sha256: &'static str,
}

fn esbuild_platform_meta(platform: VendorPlatform) -> EsbuildPlatformMeta {
    match platform {
        VendorPlatform::LinuxX64Gnu => EsbuildPlatformMeta {
            npm_pkg: "@esbuild/linux-x64",
            tarball_binary_path: "package/bin/esbuild",
            expected_sha256: ESBUILD_SHA256_LINUX_X64,
        },
        VendorPlatform::LinuxArm64Gnu => EsbuildPlatformMeta {
            npm_pkg: "@esbuild/linux-arm64",
            tarball_binary_path: "package/bin/esbuild",
            expected_sha256: ESBUILD_SHA256_LINUX_ARM64,
        },
        VendorPlatform::MacosArm64 => EsbuildPlatformMeta {
            npm_pkg: "@esbuild/darwin-arm64",
            tarball_binary_path: "package/bin/esbuild",
            expected_sha256: ESBUILD_SHA256_MACOS_ARM64,
        },
        VendorPlatform::MacosX64 => EsbuildPlatformMeta {
            npm_pkg: "@esbuild/darwin-x64",
            tarball_binary_path: "package/bin/esbuild",
            expected_sha256: ESBUILD_SHA256_MACOS_X64,
        },
        VendorPlatform::Win32X64Msvc => EsbuildPlatformMeta {
            npm_pkg: "@esbuild/win32-x64",
            // Windows package: binary is at package/esbuild.exe (no bin/ subdir).
            tarball_binary_path: "package/esbuild.exe",
            expected_sha256: ESBUILD_SHA256_WIN_X64,
        },
    }
}

/// Build the npm registry download URL for the platform-specific esbuild
/// package, e.g.:
///   `https://registry.npmjs.org/@esbuild/linux-x64/-/linux-x64-0.25.12.tgz`
fn esbuild_tarball_url(npm_pkg: &str, version: &str) -> String {
    // npm_pkg is "@esbuild/linux-x64"; the basename after the "/" is the
    // scoped package name used in the tarball URL segment.
    let basename = npm_pkg.split('/').next_back().unwrap_or(npm_pkg);
    format!("https://registry.npmjs.org/{npm_pkg}/-/{basename}-{version}.tgz")
}

/// Extract a single file from a gzipped tar archive (held in memory).
///
/// `entry_path` is matched case-insensitively against the path stored in
/// each tar entry so the function works on both case-sensitive (Linux) and
/// case-insensitive (macOS/Windows) file systems.
fn extract_from_tgz(tgz_bytes: &[u8], entry_path: &str) -> Result<Vec<u8>, String> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let gz = GzDecoder::new(io::Cursor::new(tgz_bytes));
    let mut archive = Archive::new(gz);

    for entry in archive
        .entries()
        .map_err(|e| format!("failed to read tar entries: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("tar entry error: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("tar entry path error: {e}"))?;
        let path_str = path.to_string_lossy();

        if path_str.eq_ignore_ascii_case(entry_path) {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| format!("failed to read tar entry `{entry_path}`: {e}"))?;
            return Ok(buf);
        }
    }

    Err(format!(
        "entry `{entry_path}` not found in tarball (checked all entries)"
    ))
}

/// Download and stage the esbuild binary.
///
/// Caller (`resolve_vendor_source` in `download_binaries`) has already
/// established that no override is set and the slot doesn't already hold
/// the correct binary — this function unconditionally downloads.
fn download_esbuild(platform: VendorPlatform, slot_path: &Path) -> Result<(), String> {
    let meta = esbuild_platform_meta(platform);

    let url = esbuild_tarball_url(meta.npm_pkg, EXPECTED_ESBUILD_VERSION);
    println!("cargo:warning=Downloading esbuild {EXPECTED_ESBUILD_VERSION} from {url} ...");

    // Ensure the slot's parent directory exists before writing the temp file.
    let slot_parent = slot_path
        .parent()
        .ok_or_else(|| format!("slot path {} has no parent directory", slot_path.display()))?;
    fs::create_dir_all(slot_parent).map_err(|e| {
        format!(
            "failed to create esbuild dir {}: {e}",
            slot_parent.display()
        )
    })?;

    // Stream the .tgz to a sibling temp file, then read into memory for
    // extraction.  The tgz is small (~a few MB) so in-memory extraction is
    // acceptable; the heavy ~75 MB tailwind binary uses a streaming hash
    // instead (see `download_tailwindcss`).
    let tgz_tmp = slot_parent.join("esbuild-download.tgz.tmp");
    zfb_binfetch::fetch_to_file(&url, &tgz_tmp, &zfb_binfetch::FetchOpts::default())
        .map_err(|e| format!("download failed for esbuild from `{url}`: {e}"))?;

    let tgz_bytes = fs::read(&tgz_tmp);
    // Always clean up the temp tgz, even if the read fails.
    let _ = fs::remove_file(&tgz_tmp);
    let tgz_bytes = tgz_bytes.map_err(|e| format!("failed to read esbuild tgz temp file: {e}"))?;

    let binary_bytes = extract_from_tgz(&tgz_bytes, meta.tarball_binary_path)?;

    let actual_sha = sha256_hex(&binary_bytes);
    if !actual_sha.eq_ignore_ascii_case(meta.expected_sha256) {
        return Err(format!(
            "SHA-256 mismatch for esbuild binary from `{url}`:\n\
             expected: {}\n\
             got:      {actual_sha}\n\
             The release may have been re-cut or the download was corrupted. \
             Refusing to install. Set ZFB_ESBUILD_BIN to a pre-verified binary \
             to bypass the build-script download.",
            meta.expected_sha256
        ));
    }

    stage_binary(slot_path, &binary_bytes).map_err(|e| {
        format!(
            "failed to stage esbuild binary at {}: {e}",
            slot_path.display()
        )
    })?;

    println!(
        "cargo:warning=✓ esbuild {EXPECTED_ESBUILD_VERSION} installed at {} (sha256 {actual_sha})",
        slot_path.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// tailwindcss download
// ---------------------------------------------------------------------------

/// Platform-specific asset name for the tailwindcss GitHub release.
fn tailwindcss_asset_name(platform: VendorPlatform) -> (&'static str, &'static str) {
    // Returns (asset_filename, expected_sha256).
    // Asset names match the tailwindlabs release convention.
    match platform {
        VendorPlatform::LinuxX64Gnu => ("tailwindcss-linux-x64", TAILWIND_SHA256_LINUX_X64),
        VendorPlatform::LinuxArm64Gnu => ("tailwindcss-linux-arm64", TAILWIND_SHA256_LINUX_ARM64),
        VendorPlatform::MacosArm64 => ("tailwindcss-macos-arm64", TAILWIND_SHA256_MACOS_ARM64),
        VendorPlatform::MacosX64 => ("tailwindcss-macos-x64", TAILWIND_SHA256_MACOS_X64),
        VendorPlatform::Win32X64Msvc => ("tailwindcss-windows-x64.exe", TAILWIND_SHA256_WIN_X64),
    }
}

/// Download and stage the tailwindcss v4 binary.
///
/// Caller (`resolve_vendor_source` in `download_binaries`) has already
/// established that no override is set and the slot doesn't already hold
/// the correct binary — this function unconditionally downloads.
fn download_tailwindcss(platform: VendorPlatform, slot_path: &Path) -> Result<(), String> {
    let (asset_name, expected_sha) = tailwindcss_asset_name(platform);

    let release_base = format!(
        "https://github.com/tailwindlabs/tailwindcss/releases/download/v{TAILWIND_VERSION}"
    );
    let url = format!("{release_base}/{asset_name}");
    println!("cargo:warning=Downloading tailwindcss {TAILWIND_VERSION} from {url} ...");

    // Ensure the `binaries/` parent directory exists before writing the temp
    // file.  `fetch_to_file` requires the dest's parent to exist.
    if let Some(parent) = slot_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create binaries dir {}: {e}", parent.display()))?;
    }

    // Stream directly to a .tmp sibling — avoids buffering the full ~75 MB
    // tailwind binary in memory.  The hash is computed by reading the on-disk
    // temp file in chunks (see `sha256_hex_file`).
    let tmp = slot_path.with_extension("tmp");
    zfb_binfetch::fetch_to_file(&url, &tmp, &zfb_binfetch::FetchOpts::default())
        .map_err(|e| format!("download failed for tailwindcss from `{url}`: {e}"))?;

    let actual_sha = match sha256_hex_file(&tmp) {
        Ok(sha) => sha,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "failed to hash tailwindcss temp file {}: {e}",
                tmp.display()
            ));
        }
    };

    if !actual_sha.eq_ignore_ascii_case(expected_sha) {
        let _ = fs::remove_file(&tmp);
        return Err(format!(
            "SHA-256 mismatch for tailwindcss binary from `{url}`:\n\
             expected: {expected_sha}\n\
             got:      {actual_sha}\n\
             The release may have been re-cut or the download was corrupted. \
             Refusing to install. Set ZFB_TAILWIND_BIN to a pre-verified binary \
             to bypass the build-script download.",
        ));
    }

    stage_binary_from_file(&tmp, slot_path).map_err(|e| {
        // Best-effort cleanup: if chmod or rename fails, remove the stale temp
        // file so a subsequent build can retry the fetch cleanly.
        let _ = fs::remove_file(&tmp);
        format!(
            "failed to stage tailwindcss binary at {}: {e}",
            slot_path.display()
        )
    })?;

    println!(
        "cargo:warning=✓ tailwindcss {TAILWIND_VERSION} installed at {} (sha256 {actual_sha})",
        slot_path.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Binary staging helper
// ---------------------------------------------------------------------------

/// Promote a fully-downloaded temp file to its final slot path atomically.
///
/// Ensures `dest`'s parent directory exists, sets the executable bit (`0o755`)
/// on Unix, then renames `tmp` into `dest`. The caller must have already
/// verified the SHA-256 checksum. Used by `download_tailwindcss` to avoid
/// loading the full binary into memory a second time after streaming to disk.
fn stage_binary_from_file(tmp: &Path, dest: &Path) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp, fs::Permissions::from_mode(0o755))?;
    }

    fs::rename(tmp, dest)?;
    Ok(())
}

/// Write `bytes` to `dest` atomically (via a `.tmp` sibling) and make the
/// file executable on Unix.
fn stage_binary(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp = dest.with_extension("tmp");
    fs::write(&tmp, bytes)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    }

    fs::rename(&tmp, dest)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Workspace root + slot paths
// ---------------------------------------------------------------------------

/// Locate the workspace root by walking up from `CARGO_MANIFEST_DIR`.
///
/// Returns the first ancestor directory containing a `Cargo.toml` whose
/// `[workspace]` member list includes the current crate, which in practice
/// is the root `Cargo.toml` because `crates/*` are members of the workspace
/// at the repo root. We detect the workspace root as the directory containing
/// a `Cargo.toml` that has a `[workspace]` section — the simplest reliable
/// heuristic here is to check for a `pnpm-workspace.yaml` (which lives only
/// at the repo root) alongside `Cargo.toml`.
///
/// Fallback: if nothing is found, use `CARGO_MANIFEST_DIR` (i.e. treat
/// `crates/zfb/` as the base and build relative paths from there). In that
/// case the slot paths will be relative to the manifest dir.
fn find_workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is set by Cargo to the directory of this crate's
    // Cargo.toml (i.e. crates/zfb/).
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));

    let mut dir = manifest_dir.clone();
    loop {
        let toml = dir.join("Cargo.toml");
        let pnpm_ws = dir.join("pnpm-workspace.yaml");
        if toml.exists() && pnpm_ws.exists() {
            return dir;
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => break,
        }
    }

    // Fallback: two levels up from `crates/zfb/`
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or(manifest_dir)
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Append `suffix` (e.g. `".exe"`, or `""`) to `base`'s filename as a real
/// extension, replacing whatever extension (if any) `base` already has.
/// Kept as a small helper so the two slot-path constructions in
/// `download_binaries` stay one-liners.
fn with_exe_suffix(base: PathBuf, suffix: &str) -> PathBuf {
    if suffix.is_empty() {
        return base;
    }
    let mut p = base;
    p.set_extension(suffix.trim_start_matches('.'));
    p
}

/// Validate a `ZFB_ESBUILD_BIN` / `ZFB_TAILWIND_BIN` override path and
/// return it ready to stage.
///
/// Per the documented override contract (`BUILDING.md`), the path must be
/// **absolute** and must point at an existing regular file. SHA-256
/// pinning is deliberately skipped for overrides — the operator supplying
/// the path is responsible for having verified it.
///
/// Takes the raw `OsString` rather than a `String` so a legally non-UTF-8
/// Unix path round-trips intact; `to_string_lossy()` is used only inside
/// error/diagnostic messages, never to build the `Path` that's actually
/// checked against the filesystem.
fn validate_override_path(env_var: &str, raw: &std::ffi::OsStr) -> Result<PathBuf, String> {
    let path = Path::new(raw);
    let display = raw.to_string_lossy();
    if !path.is_absolute() {
        return Err(format!(
            "{env_var} must be an absolute path (got `{display}`). Overrides stage \
             a pre-verified binary directly into the vendor snapshot with no \
             SHA-256 check, so build.rs requires an unambiguous absolute path \
             rather than resolving a relative one against an arbitrary cwd."
        ));
    }
    let metadata = fs::metadata(path).map_err(|e| {
        format!("{env_var} points at `{display}`, which does not exist or is not readable: {e}")
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "{env_var} points at `{display}`, which is not a regular file."
        ));
    }
    println!(
        "cargo:warning={env_var} is set — staging `{display}` directly into the vendor \
         snapshot. SHA-256 pinning is skipped for override binaries; verifying \
         the binary is the operator's responsibility (see BUILDING.md)."
    );
    println!("cargo:rerun-if-changed={}", path.display());
    Ok(path.to_path_buf())
}

/// Combine an override env var, the resolved `VendorPlatform` (if any), and
/// the on-disk slot state into a `BinarySource` decision via
/// `zfb_toolchain_pins::resolve_binary_source`. The slot's SHA-256 is only
/// checked when it could actually matter (no override present, and the
/// platform is supported) — avoids hashing the ~75 MB tailwind slot file
/// for no reason when it's about to be overridden anyway.
fn resolve_vendor_source(
    env_var: &str,
    vendor_platform: Option<VendorPlatform>,
    slot_path: &Path,
    expected_sha256: impl Fn(VendorPlatform) -> &'static str,
) -> BinarySource {
    let raw = std::env::var_os(env_var);
    let override_present = raw.as_deref().is_some_and(|s| !s.is_empty());

    let slot_already_correct = if override_present {
        false
    } else {
        vendor_platform
            .map(|p| binary_already_correct(slot_path, expected_sha256(p)))
            .unwrap_or(false)
    };

    resolve_binary_source(
        raw.as_deref(),
        vendor_platform.is_some(),
        slot_already_correct,
    )
}

/// Download and stage the esbuild and tailwindcss binaries.
///
/// Each binary resolves its source independently: an override wins
/// unconditionally (staged as-is, no SHA-256 check); otherwise the
/// existing download-and-slot flow runs, which itself requires a
/// supported `TARGET`. `detect_platform`-equivalent errors are only raised
/// when a binary actually needs the download flow and the platform isn't
/// supported — an override-only build on an unsupported platform (e.g.
/// musl) never hits that error.
///
/// This is a separate function (not inlined into `main`) so that sub-#198
/// (runtime embedding) can add its own top-level function without having to
/// restructure this file.
fn download_binaries() -> Result<(), String> {
    let workspace_root = find_workspace_root();
    let binaries_dir = workspace_root.join("crates").join("zfb").join("binaries");

    let target = std::env::var("TARGET").unwrap_or_default();
    let vendor_platform = VendorPlatform::from_target_triple(&target);
    let exe_suffix = exe_suffix_for_target(&target);

    let esbuild_slot = with_exe_suffix(binaries_dir.join("esbuild").join("esbuild"), exe_suffix);
    let tailwind_slot = with_exe_suffix(binaries_dir.join("tailwindcss-v4"), exe_suffix);

    // Emit rerun triggers so Cargo re-invokes the build script when the
    // binary slots or override env vars change (e.g. after a manual `rm`,
    // after a fetch, or after pointing ZFB_*_BIN at a different file).
    println!("cargo:rerun-if-changed=crates/zfb-islands/src/esbuild.rs");
    println!("cargo:rerun-if-changed=scripts/fetch-tailwind.mjs");
    println!("cargo:rerun-if-changed={}", esbuild_slot.display());
    println!("cargo:rerun-if-changed={}", tailwind_slot.display());
    println!("cargo:rerun-if-env-changed=ZFB_ESBUILD_BIN");
    println!("cargo:rerun-if-env-changed=ZFB_TAILWIND_BIN");

    let esbuild_source =
        resolve_vendor_source("ZFB_ESBUILD_BIN", vendor_platform, &esbuild_slot, |p| {
            esbuild_platform_meta(p).expected_sha256
        });
    let tailwind_source =
        resolve_vendor_source("ZFB_TAILWIND_BIN", vendor_platform, &tailwind_slot, |p| {
            tailwindcss_asset_name(p).1
        });

    if matches!(esbuild_source, BinarySource::Unsupported)
        || matches!(tailwind_source, BinarySource::Unsupported)
    {
        return Err(unsupported_target_message(&target));
    }

    let esbuild_final = match esbuild_source {
        BinarySource::Override(raw) => validate_override_path("ZFB_ESBUILD_BIN", &raw)?,
        BinarySource::Slot => {
            println!(
                "cargo:warning=esbuild binary already present at {} with correct SHA-256 — skipping download.",
                esbuild_slot.display()
            );
            esbuild_slot.clone()
        }
        BinarySource::NeedsDownload => {
            let platform =
                vendor_platform.expect("NeedsDownload implies platform_supported was true");
            download_esbuild(platform, &esbuild_slot)?;
            esbuild_slot.clone()
        }
        BinarySource::Unsupported => unreachable!("handled above"),
    };

    let tailwind_final = match tailwind_source {
        BinarySource::Override(raw) => validate_override_path("ZFB_TAILWIND_BIN", &raw)?,
        BinarySource::Slot => {
            println!(
                "cargo:warning=tailwindcss binary already present at {} with correct SHA-256 — skipping download.",
                tailwind_slot.display()
            );
            tailwind_slot.clone()
        }
        BinarySource::NeedsDownload => {
            let platform =
                vendor_platform.expect("NeedsDownload implies platform_supported was true");
            download_tailwindcss(platform, &tailwind_slot)?;
            tailwind_slot.clone()
        }
        BinarySource::Unsupported => unreachable!("handled above"),
    };

    // Sub #212 — also stage the binaries into `$OUT_DIR/vendor/bin/` so the
    // existing `include_dir!("$ZFB_VENDOR_DIR")` snapshot embeds them next to
    // `@takazudo/*` and the framework packages. Consumers without a
    // workspace-relative `crates/zfb/binaries/` dir can then extract them at
    // runtime via `embedded_binary()` in `crates/zfb/src/render_pipeline.rs`.
    stage_binaries_into_vendor(&esbuild_final, &tailwind_final, exe_suffix)?;

    Ok(())
}

/// Copy the resolved esbuild and tailwindcss binaries (each either a slot
/// path or a validated override path) into `$OUT_DIR/vendor/bin/` so they
/// ride along inside `EMBEDDED_VENDOR` (the `include_dir!` snapshot in
/// `crates/zfb/src/render_pipeline.rs`). The `embedded_binary()` helper
/// then extracts whichever name a caller asks for at runtime.
///
/// `exe_suffix` (`".exe"` or `""`) names the canonical embedded filename —
/// derived from Cargo's resolved `TARGET`, not the host's `cfg!(target_os)`,
/// so a cross-compiling host never mis-names the target-platform entry.
/// The executable bit is set on Unix so the extracted file is invocable
/// without an extra chmod.
fn stage_binaries_into_vendor(
    esbuild_src: &Path,
    tailwind_src: &Path,
    exe_suffix: &str,
) -> Result<(), String> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let bin_dir = out_dir.join("vendor").join("bin");
    fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("failed to create vendor bin dir {}: {e}", bin_dir.display()))?;

    let esbuild_dst = bin_dir.join(format!("esbuild{exe_suffix}"));
    copy_executable(esbuild_src, &esbuild_dst)?;

    let tailwind_dst = bin_dir.join(format!("tailwindcss-v4{exe_suffix}"));
    copy_executable(tailwind_src, &tailwind_dst)?;

    Ok(())
}

/// Copy `src` to `dst` and (on Unix) preserve / re-apply the 0o755
/// executable bit so the extracted binary is invocable without an extra
/// chmod.
fn copy_executable(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Err(format!(
            "expected staged binary at {} to exist before vendor staging — \
             download_binaries() should have produced it",
            src.display()
        ));
    }
    fs::copy(src, dst)
        .map_err(|e| format!("failed to copy {} → {}: {e}", src.display(), dst.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dst, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("failed to set executable bit on {}: {e}", dst.display()))?;
    }
    Ok(())
}

fn main() {
    // Re-run this build script (and thus recompile the binary) whenever the
    // release version env var changes. Without this directive, cargo will NOT
    // rebuild when only ZFB_RELEASE_VERSION changes, causing stale --version
    // output across local experiments or repeated CI builds.
    println!("cargo:rerun-if-env-changed=ZFB_RELEASE_VERSION");

    // Sub 198 — embed @takazudo/zfb + zfb-runtime TypeScript source so the
    // installed binary works on a consumer with no node_modules.
    embed_runtime();

    // Sub 209 — embed the framework runtime packages (preact,
    // preact-render-to-string, hono) so a consumer with no node_modules can
    // still bundle a page that imports them.
    embed_framework_packages();

    // Sub 197 — download pinned esbuild + tailwindcss standalone binaries.
    if let Err(e) = download_binaries() {
        // Emit as a compile_error! so the build output is clearly visible.
        println!("cargo:error={e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Sub 198 — embed @takazudo/zfb and @takazudo/zfb-runtime
// ---------------------------------------------------------------------------

/// Copy the TypeScript source of `@takazudo/zfb` and `@takazudo/zfb-runtime`
/// from `packages/` into `$OUT_DIR/vendor/@takazudo/` so `include_dir!` can
/// embed them in the binary at compile time.
///
/// Only `src/` (non-test files) and `package.json` are copied — `node_modules`,
/// `tsconfig.json`, `vitest.config.ts`, etc. are dev-only artefacts that must
/// not bloat the binary.
///
/// The function emits `cargo:rustc-env=ZFB_VENDOR_DIR=<path>` so the macro
/// invocation `include_dir!(env!("ZFB_VENDOR_DIR"))` resolves at compile time.
fn embed_runtime() {
    // Locate workspace root: two levels up from `crates/zfb/` (the manifest dir).
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .parent() // crates/
        .and_then(|p| p.parent()) // workspace root
        .expect("could not resolve workspace root from CARGO_MANIFEST_DIR");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let vendor_dir = out_dir.join("vendor").join("@takazudo");
    std::fs::create_dir_all(&vendor_dir).expect("failed to create vendor dir");

    // Packages to embed: (source package dir, dest package name)
    let packages = [
        ("packages/zfb", "zfb"),
        ("packages/zfb-runtime", "zfb-runtime"),
    ];

    for (pkg_rel, pkg_name) in &packages {
        let src_pkg = workspace_root.join(pkg_rel);
        let dst_pkg = vendor_dir.join(pkg_name);

        // Copy package.json.
        let src_json = src_pkg.join("package.json");
        let dst_json = dst_pkg.join("package.json");
        std::fs::create_dir_all(&dst_pkg).expect("failed to create vendor package dir");
        std::fs::copy(&src_json, &dst_json).unwrap_or_else(|e| {
            panic!("failed to copy {}: {e}", src_json.display());
        });

        // Copy src/ — only non-test .ts files (skip __tests__ directory).
        let src_src = src_pkg.join("src");
        let dst_src = dst_pkg.join("src");
        copy_ts_src(&src_src, &dst_src);

        // Re-run if any source file changes.
        println!("cargo:rerun-if-changed={}", src_pkg.display());
    }

    // Emit vendor dir path as env var for the include_dir! macro.
    // include_dir! resolves env!(...) at macro-expansion time when the
    // env var is set via cargo:rustc-env.
    let vendor_root = out_dir.join("vendor");
    println!("cargo:rustc-env=ZFB_VENDOR_DIR={}", vendor_root.display());
}

/// Recursively copy `.ts` source files from `src` to `dst`, skipping any
/// directory named `__tests__` and any file whose name starts with `__`.
fn copy_ts_src(src: &Path, dst: &Path) {
    let rd = match std::fs::read_dir(src) {
        Ok(rd) => rd,
        Err(e) => panic!("failed to read dir {}: {e}", src.display()),
    };
    std::fs::create_dir_all(dst).expect("failed to create dst dir");

    for entry in rd.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip __tests__ and hidden dirs.
        if name_str.starts_with("__") || name_str.starts_with('.') {
            continue;
        }

        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if ft.is_dir() {
            copy_ts_src(&entry.path(), &dst.join(&name));
        } else if ft.is_file() {
            // Only embed .ts source files.
            if name_str.ends_with(".ts") {
                let dst_file = dst.join(&name);
                std::fs::copy(entry.path(), &dst_file).unwrap_or_else(|e| {
                    panic!("failed to copy {}: {e}", entry.path().display());
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Sub 209 — embed framework runtime packages (preact, preact-render-to-string,
// hono) so esbuild can resolve them from the embedded extraction with no
// consumer-side node_modules.
// ---------------------------------------------------------------------------

/// Copy the published trees of `preact`, `preact-render-to-string`, and `hono`
/// from zfb's pnpm-installed `node_modules/.pnpm/` store into
/// `$OUT_DIR/vendor/<pkg>/` so they ride along inside `EMBEDDED_VENDOR` (the
/// `include_dir!` snapshot in `crates/zfb/src/render_pipeline.rs`). The
/// extraction at runtime then produces `node_modules/<pkg>/` siblings beside
/// `node_modules/@takazudo/`, giving esbuild a complete tree to resolve from.
///
/// pnpm's content-addressable layout puts each direct dep at
/// `node_modules/.pnpm/<name>@<version>/node_modules/<name>/`. When a package
/// has injected peer deps (e.g. `preact-render-to-string`), pnpm appends a
/// `_<peer>@<peerver>` suffix to the directory name, so we match on the
/// `<name>@<version>` prefix and accept the first match.
///
/// Only files needed at bundle time are copied — `node_modules/` (no nested
/// deps), `__tests__/`, hidden dirs, and source-map files are filtered out to
/// keep the embedded snapshot lean. The three packages currently have
/// no nested deps that need following:
///
/// - `preact`: zero runtime deps.
/// - `preact-render-to-string`: peer-only on `preact` (resolved via the
///   embedded `node_modules/preact` sibling at runtime).
/// - `hono`: zero runtime deps.
///
/// Source of truth for versions: `pnpm-lock.yaml` (mirrored by the
/// `*_VERSION` constants near the top of this file).
fn embed_framework_packages() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("could not resolve workspace root from CARGO_MANIFEST_DIR");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let vendor_dir = out_dir.join("vendor");
    std::fs::create_dir_all(&vendor_dir).expect("failed to create vendor dir");

    let pnpm_store = workspace_root.join("node_modules").join(".pnpm");

    let packages: [(&str, &str); 3] = [
        ("preact", PREACT_VERSION),
        ("preact-render-to-string", PREACT_RTS_VERSION),
        ("hono", HONO_VERSION),
    ];

    for (name, version) in &packages {
        let src_pkg = locate_pnpm_pkg(&pnpm_store, name, version).unwrap_or_else(|| {
            panic!(
                "framework package `{name}@{version}` not found under {}.\n\
                 Run `pnpm install --frozen-lockfile` from the workspace root, \
                 then re-run cargo build.\n\
                 If the package has been bumped in pnpm-lock.yaml, also update \
                 the *_VERSION constant in crates/zfb/build.rs.",
                pnpm_store.display()
            )
        });

        let dst_pkg = vendor_dir.join(name);
        copy_pkg_published(&src_pkg, &dst_pkg);

        // Re-run if the source package contents change (e.g. a `pnpm install`
        // bump or a deliberate edit during local hacking).
        println!("cargo:rerun-if-changed={}", src_pkg.display());
    }

    // The cargo:rustc-env=ZFB_VENDOR_DIR=... line is emitted by
    // `embed_runtime()` and points at the same `$OUT_DIR/vendor` dir, so the
    // framework packages we just staged are picked up by the existing
    // `include_dir!("$ZFB_VENDOR_DIR")` macro in `render_pipeline.rs`.
}

/// Find the pnpm-store directory for `<name>@<version>` under
/// `node_modules/.pnpm/`. Accepts either the bare directory name
/// `<name>@<version>` or any peer-suffixed variant `<name>@<version>_*`.
fn locate_pnpm_pkg(pnpm_store: &Path, name: &str, version: &str) -> Option<PathBuf> {
    let exact_prefix = format!("{name}@{version}");
    let entries = std::fs::read_dir(pnpm_store).ok()?;
    for entry in entries.flatten() {
        let entry_name = entry.file_name();
        let entry_str = entry_name.to_string_lossy();
        // Accept exact match or `<name>@<version>_<peer-suffix>` variants.
        let matches = entry_str == exact_prefix
            || entry_str
                .strip_prefix(&exact_prefix)
                .is_some_and(|rest| rest.starts_with('_'));
        if !matches {
            continue;
        }
        let pkg_dir = entry.path().join("node_modules").join(name);
        if pkg_dir.is_dir() {
            return Some(pkg_dir);
        }
    }
    None
}

/// Recursively copy `src` to `dst`, skipping directories and files that are
/// dev-only or recursive (so we do not pull a vendored package's own
/// `node_modules/` into the binary). The filter is intentionally conservative:
/// we keep `package.json`, every `dist/`, `src/`, and any sibling published
/// JS / TS / d.ts entry files; we drop nested `node_modules/`, `__tests__/`,
/// hidden dirs, and `*.map` source maps.
fn copy_pkg_published(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("failed to create vendor pkg dir");
    let rd = std::fs::read_dir(src)
        .unwrap_or_else(|e| panic!("failed to read framework pkg dir {}: {e}", src.display()));
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Skip nested node_modules (recursive copy hazard) and hidden files.
        if name_str == "node_modules" || name_str.starts_with('.') {
            continue;
        }
        // Skip test directories.
        if name_str == "__tests__" || name_str == "test" || name_str == "tests" {
            continue;
        }
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            copy_pkg_published(&entry.path(), &dst.join(&name));
        } else if ft.is_file() {
            // Drop source maps to keep the embedded snapshot lean. The bundle
            // does not need them at consumer build time.
            if name_str.ends_with(".map") {
                continue;
            }
            let dst_file = dst.join(&name);
            std::fs::copy(entry.path(), &dst_file).unwrap_or_else(|e| {
                panic!("failed to copy {}: {e}", entry.path().display());
            });
        }
        // Symlinks and other non-regular entries are skipped — the pnpm store
        // never uses symlinks for the actual published files (only for the
        // top-level `node_modules/<name>` aliasing, which we don't touch).
    }
}
