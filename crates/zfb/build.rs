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
//! * **Escape hatches.** If `ZFB_ESBUILD_BIN` or `ZFB_TAILWIND_BIN` are set,
//!   their respective download step is skipped entirely.
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
use zfb_toolchain_pins::EXPECTED_ESBUILD_VERSION;

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
const HONO_VERSION: &str = "4.12.23";

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

#[derive(Debug, Clone, Copy)]
enum Platform {
    LinuxX64,
    LinuxArm64,
    MacosArm64,
    MacosX64,
    WindowsX64,
}

/// Detect the current build platform from Cargo environment variables.
///
/// Returns `Err` with a helpful message when the target is not in the
/// supported set. The user can still build by setting `ZFB_ESBUILD_BIN`
/// and `ZFB_TAILWIND_BIN` to manually-fetched binaries.
fn detect_platform() -> Result<Platform, String> {
    // Cargo sets TARGET during build scripts.
    let target = std::env::var("TARGET").unwrap_or_default();
    // Match the Rust target triple patterns.
    if target.contains("x86_64-unknown-linux") {
        return Ok(Platform::LinuxX64);
    }
    if target.contains("aarch64-unknown-linux") {
        return Ok(Platform::LinuxArm64);
    }
    if target.contains("aarch64-apple-darwin") {
        return Ok(Platform::MacosArm64);
    }
    if target.contains("x86_64-apple-darwin") {
        return Ok(Platform::MacosX64);
    }
    if target.contains("x86_64-pc-windows") {
        return Ok(Platform::WindowsX64);
    }
    Err(format!(
        "unsupported target triple `{target}`. \
         Supported targets: x86_64-unknown-linux-*, aarch64-unknown-linux-*, \
         aarch64-apple-darwin, x86_64-apple-darwin, x86_64-pc-windows-*. \
         Set ZFB_ESBUILD_BIN and ZFB_TAILWIND_BIN to manually-fetched binaries \
         to proceed on an unsupported platform."
    ))
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

/// Download `url` into a `Vec<u8>`.
///
/// On network failure, returns a descriptive error that includes the two
/// escape-hatch env var names.
fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::blocking::Client::builder()
        .use_rustls_tls()
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let response = client.get(url).send().map_err(|e| {
        format!(
            "network error fetching `{url}`: {e}\n\
                 Hint: if no network is available, set ZFB_ESBUILD_BIN and \
                 ZFB_TAILWIND_BIN to paths of manually-downloaded binaries."
        )
    })?;

    if !response.status().is_success() {
        return Err(format!(
            "GET `{url}` returned HTTP {}: {}\n\
             Hint: if no network is available, set ZFB_ESBUILD_BIN and \
             ZFB_TAILWIND_BIN to paths of manually-downloaded binaries.",
            response.status().as_u16(),
            response.status().canonical_reason().unwrap_or("unknown")
        ));
    }

    response
        .bytes()
        .map(|b| b.to_vec())
        .map_err(|e| format!("failed to read response body from `{url}`: {e}"))
}

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

fn esbuild_platform_meta(platform: Platform) -> EsbuildPlatformMeta {
    match platform {
        Platform::LinuxX64 => EsbuildPlatformMeta {
            npm_pkg: "@esbuild/linux-x64",
            tarball_binary_path: "package/bin/esbuild",
            expected_sha256: ESBUILD_SHA256_LINUX_X64,
        },
        Platform::LinuxArm64 => EsbuildPlatformMeta {
            npm_pkg: "@esbuild/linux-arm64",
            tarball_binary_path: "package/bin/esbuild",
            expected_sha256: ESBUILD_SHA256_LINUX_ARM64,
        },
        Platform::MacosArm64 => EsbuildPlatformMeta {
            npm_pkg: "@esbuild/darwin-arm64",
            tarball_binary_path: "package/bin/esbuild",
            expected_sha256: ESBUILD_SHA256_MACOS_ARM64,
        },
        Platform::MacosX64 => EsbuildPlatformMeta {
            npm_pkg: "@esbuild/darwin-x64",
            tarball_binary_path: "package/bin/esbuild",
            expected_sha256: ESBUILD_SHA256_MACOS_X64,
        },
        Platform::WindowsX64 => EsbuildPlatformMeta {
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
/// No-op if `ZFB_ESBUILD_BIN` is set or if the binary is already present
/// with the correct SHA-256.
fn download_esbuild(platform: Platform, slot_path: &Path) -> Result<(), String> {
    if std::env::var_os("ZFB_ESBUILD_BIN").is_some() {
        println!("cargo:warning=ZFB_ESBUILD_BIN is set — skipping esbuild binary download.");
        return Ok(());
    }

    let meta = esbuild_platform_meta(platform);

    if binary_already_correct(slot_path, meta.expected_sha256) {
        println!(
            "cargo:warning=esbuild binary already present at {} with correct SHA-256 — skipping download.",
            slot_path.display()
        );
        return Ok(());
    }

    let url = esbuild_tarball_url(meta.npm_pkg, EXPECTED_ESBUILD_VERSION);
    println!("cargo:warning=Downloading esbuild {EXPECTED_ESBUILD_VERSION} from {url} ...");

    let tgz_bytes = fetch_bytes(&url)?;
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
fn tailwindcss_asset_name(platform: Platform) -> (&'static str, &'static str) {
    // Returns (asset_filename, expected_sha256).
    // Asset names match the tailwindlabs release convention.
    match platform {
        Platform::LinuxX64 => ("tailwindcss-linux-x64", TAILWIND_SHA256_LINUX_X64),
        Platform::LinuxArm64 => ("tailwindcss-linux-arm64", TAILWIND_SHA256_LINUX_ARM64),
        Platform::MacosArm64 => ("tailwindcss-macos-arm64", TAILWIND_SHA256_MACOS_ARM64),
        Platform::MacosX64 => ("tailwindcss-macos-x64", TAILWIND_SHA256_MACOS_X64),
        Platform::WindowsX64 => ("tailwindcss-windows-x64.exe", TAILWIND_SHA256_WIN_X64),
    }
}

/// Download and stage the tailwindcss v4 binary.
///
/// No-op if `ZFB_TAILWIND_BIN` is set or if the binary is already present
/// with the correct SHA-256.
fn download_tailwindcss(platform: Platform, slot_path: &Path) -> Result<(), String> {
    if std::env::var_os("ZFB_TAILWIND_BIN").is_some() {
        println!("cargo:warning=ZFB_TAILWIND_BIN is set — skipping tailwindcss binary download.");
        return Ok(());
    }

    let (asset_name, expected_sha) = tailwindcss_asset_name(platform);

    if binary_already_correct(slot_path, expected_sha) {
        println!(
            "cargo:warning=tailwindcss binary already present at {} with correct SHA-256 — skipping download.",
            slot_path.display()
        );
        return Ok(());
    }

    let release_base = format!(
        "https://github.com/tailwindlabs/tailwindcss/releases/download/v{TAILWIND_VERSION}"
    );
    let url = format!("{release_base}/{asset_name}");
    println!("cargo:warning=Downloading tailwindcss {TAILWIND_VERSION} from {url} ...");

    let binary_bytes = fetch_bytes(&url)?;

    let actual_sha = sha256_hex(&binary_bytes);
    if !actual_sha.eq_ignore_ascii_case(expected_sha) {
        return Err(format!(
            "SHA-256 mismatch for tailwindcss binary from `{url}`:\n\
             expected: {expected_sha}\n\
             got:      {actual_sha}\n\
             The release may have been re-cut or the download was corrupted. \
             Refusing to install. Set ZFB_TAILWIND_BIN to a pre-verified binary \
             to bypass the build-script download.",
        ));
    }

    stage_binary(slot_path, &binary_bytes).map_err(|e| {
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

/// Download and stage the esbuild and tailwindcss binaries.
///
/// This is a separate function (not inlined into `main`) so that sub-#198
/// (runtime embedding) can add its own top-level function without having to
/// restructure this file.
fn download_binaries() -> Result<(), String> {
    let workspace_root = find_workspace_root();
    let binaries_dir = workspace_root.join("crates").join("zfb").join("binaries");

    let esbuild_slot = binaries_dir.join("esbuild").join("esbuild");
    // Windows binary slot carries a .exe suffix.
    #[cfg(target_os = "windows")]
    let esbuild_slot = {
        let mut p = esbuild_slot;
        p.set_extension("exe");
        p
    };

    let tailwind_slot = binaries_dir.join("tailwindcss-v4");
    #[cfg(target_os = "windows")]
    let tailwind_slot = {
        let mut p = tailwind_slot;
        p.set_extension("exe");
        p
    };

    // Emit rerun triggers so Cargo re-invokes the build script when the
    // binary slots change (e.g. after a manual `rm` or after a fetch).
    println!("cargo:rerun-if-changed=crates/zfb-islands/src/esbuild.rs");
    println!("cargo:rerun-if-changed=scripts/fetch-tailwind.mjs");
    // Also rerun when either binary slot file changes.
    println!("cargo:rerun-if-changed={}", esbuild_slot.display());
    println!("cargo:rerun-if-changed={}", tailwind_slot.display());

    let platform = detect_platform()?;

    download_esbuild(platform, &esbuild_slot)?;
    download_tailwindcss(platform, &tailwind_slot)?;

    // Sub #212 — also stage the binaries into `$OUT_DIR/vendor/bin/` so the
    // existing `include_dir!("$ZFB_VENDOR_DIR")` snapshot embeds them next to
    // `@takazudo/*` and the framework packages. Consumers without a
    // workspace-relative `crates/zfb/binaries/` dir can then extract them at
    // runtime via `embedded_binary()` in `crates/zfb/src/render_pipeline.rs`.
    stage_binaries_into_vendor(&esbuild_slot, &tailwind_slot)?;

    Ok(())
}

/// Copy the staged esbuild and tailwindcss binaries from
/// `crates/zfb/binaries/` into `$OUT_DIR/vendor/bin/` so they ride along
/// inside `EMBEDDED_VENDOR` (the `include_dir!` snapshot in
/// `crates/zfb/src/render_pipeline.rs`). The `embedded_binary()` helper
/// then extracts whichever name a caller asks for at runtime.
///
/// Windows binaries ship with a `.exe` suffix; the destination filename
/// preserves that suffix. The executable bit is set on Unix so the
/// extracted file is invocable without an extra chmod.
fn stage_binaries_into_vendor(esbuild_slot: &Path, tailwind_slot: &Path) -> Result<(), String> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let bin_dir = out_dir.join("vendor").join("bin");
    fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("failed to create vendor bin dir {}: {e}", bin_dir.display()))?;

    // Re-emit rerun triggers so a manual re-fetch of either binary refreshes
    // the embedded snapshot on the next cargo build.
    println!("cargo:rerun-if-changed={}", esbuild_slot.display());
    println!("cargo:rerun-if-changed={}", tailwind_slot.display());

    // Esbuild → vendor/bin/esbuild (or .exe on Windows).
    let esbuild_dst_name = if cfg!(target_os = "windows") {
        "esbuild.exe"
    } else {
        "esbuild"
    };
    let esbuild_dst = bin_dir.join(esbuild_dst_name);
    copy_executable(esbuild_slot, &esbuild_dst)?;

    // Tailwind → vendor/bin/tailwindcss-v4 (or .exe on Windows).
    let tailwind_dst_name = if cfg!(target_os = "windows") {
        "tailwindcss-v4.exe"
    } else {
        "tailwindcss-v4"
    };
    let tailwind_dst = bin_dir.join(tailwind_dst_name);
    copy_executable(tailwind_slot, &tailwind_dst)?;

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
