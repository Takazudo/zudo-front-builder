#!/usr/bin/env bash
# build-macos-x64-local.sh
#
# Escape hatch for the Intel macOS CI runner (issue #437), originally written
# because `macos-13` was chronically queue-starved. That image was retired
# 2025-12-04; the CI leg now targets `macos-15-intel`, which itself retires in
# Fall 2027 — after which Actions has no x86_64 macOS runner and this script's
# cross-compile-from-arm64 approach becomes the only way to produce the archive.
# Builds the macOS-x64 (`x86_64-apple-darwin`) release
# binary locally on a Mac and produces the GH Release archive + sha256 in the
# EXACT format release.yml would have produced on the runner.
#
# Usage:
#   ./scripts/build-macos-x64-local.sh [--upload <tag>] [--out-dir DIR]
#
# --upload <tag>  After building, upload the archive + .sha256 to the GitHub
#                 Release for <tag> via `gh release upload --clobber`. The tag
#                 (e.g. v0.1.0-next.1) must already have a Release. Without this
#                 flag the files are only written locally and the upload command
#                 is printed for you to run manually.
#
# --out-dir DIR   Where to write the archive + .sha256. Default: repo root.
#
# Prerequisites (matches release.yml's build job):
#   - Rust toolchain (rustup); the script runs `rustup target add
#     x86_64-apple-darwin` itself (idempotent — needed on Apple Silicon hosts).
#   - node_modules present. build.rs (embed_framework_packages) reads
#     node_modules/.pnpm/preact@*/... at compile time, so the script runs
#     `pnpm install --frozen-lockfile` before cargo build — skipping it yields a
#     binary missing embedded framework assets that only fails at the user's
#     install time. (release.yml installs deps before cargo build for the same
#     reason.)
#
# Archive / checksum contract (LOCKED — must match release.yml exactly, see the
# header of .github/workflows/release.yml and RELEASE_DAY_CHECKLIST.md):
#   Archive name:  zfb-{semver}-x86_64-apple-darwin.tar.gz
#     {semver} = packages/zfb/package.json .version, no leading "v"
#   Contents:      exactly one file at the archive root — "zfb" (no nested dir),
#                  produced by `tar -C packages/zfb-darwin-x64 -czf <archive> zfb`
#   Checksum file: {archive-name}.sha256, one line
#                  "<64-hex-lowercase>  <archive-basename>" (two spaces, GNU
#                  sha256sum format; S4/S5/S6 installers parse field 1).
set -euo pipefail

# ── Argument parsing ──────────────────────────────────────────────────────────

upload_tag=""
out_dir=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --upload)
      upload_tag="$2"
      shift 2
      ;;
    --out-dir)
      out_dir="$2"
      shift 2
      ;;
    -*)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
    *)
      echo "Unexpected positional argument: $1" >&2
      exit 1
      ;;
  esac
done

# ── Locate repo root ──────────────────────────────────────────────────────────

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ -z "$out_dir" ]]; then
  out_dir="$repo_root"
fi
mkdir -p "$out_dir"

# ── Platform sanity check ─────────────────────────────────────────────────────

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "ERROR: this script must run on macOS — cross-compiling to" >&2
  echo "       x86_64-apple-darwin from Linux/Windows needs significant SDK" >&2
  echo "       setup and is out of scope (issue #437)." >&2
  exit 1
fi

target="x86_64-apple-darwin"
platform_pkg="packages/zfb-darwin-x64"

# ── Resolve semver (single source of truth, matches release.yml) ──────────────

semver="$(node -p "require('./packages/zfb/package.json').version")"
archive="zfb-${semver}-${target}.tar.gz"
echo "==> Building zfb ${semver} for ${target}"

# ── Fix (a): Homebrew rust shadows rustup → ensure rustup toolchain wins ──────
# When ~/.cargo/bin is not ahead of /opt/homebrew/bin on PATH, `cargo` may
# resolve to Homebrew's host-only rust which cannot cross-compile x86_64-apple-
# darwin (error[E0463]: can't find crate for `core`). Detect and fix it here,
# before any cargo/rustup invocation, so the whole script uses the right cargo.
if ! command -v rustup >/dev/null 2>&1; then
  echo "ERROR: rustup not found. Install rustup (https://rustup.rs/) and ensure" >&2
  echo "       ~/.cargo/bin precedes /opt/homebrew/bin on PATH." >&2
  exit 1
fi
if cargo --version 2>/dev/null | grep -q '(Homebrew)'; then
  echo "==> Homebrew rust detected on PATH — prepending rustup toolchain bin"
  _rustup_toolchain="$(rustup show active-toolchain | awk '{print $1}')"
  _rustup_toolchain_bin="$HOME/.rustup/toolchains/${_rustup_toolchain}/bin"
  if [[ ! -d "$_rustup_toolchain_bin" ]]; then
    echo "ERROR: resolved rustup toolchain bin not found: ${_rustup_toolchain_bin}" >&2
    echo "       Run 'rustup toolchain install ${_rustup_toolchain}' and retry." >&2
    exit 1
  fi
  export PATH="${_rustup_toolchain_bin}:$PATH"
  echo "    cargo now: $(cargo --version)"
fi

# ── Toolchain target (idempotent — Apple Silicon hosts need this once) ────────

echo "==> Ensuring rust target ${target} is installed"
rustup target add "$target"

# Fix (c): wait for the target to be fully registered before building.
# On first-ever install the rust-std download can lag behind cargo build and
# produce the same E0463 error. Poll until the target is listed (bounded).
_target_wait=0
_target_timeout=60
until rustup target list --installed | grep -qx "$target"; do
  if [[ $_target_wait -ge $_target_timeout ]]; then
    echo "ERROR: ${target} still not listed by 'rustup target list --installed'" >&2
    echo "       after ${_target_timeout}s. Check your rustup installation." >&2
    exit 1
  fi
  echo "    Waiting for ${target} to finish installing... (${_target_wait}s)"
  sleep 2
  _target_wait=$(( _target_wait + 2 ))
done
echo "    Target ${target} is ready."

# ── Install node deps before cargo build ──────────────────────────────────────
# build.rs embeds framework packages from node_modules at compile time.

echo "==> Installing node dependencies (pnpm install --frozen-lockfile)"
# CI=true so pnpm runs non-interactively. pnpm 11 (#440) purges a node_modules
# left by an incompatible pnpm version and would otherwise prompt for
# confirmation — which aborts under no-TTY. The GHA runner gets CI=true for
# free; set it here to replicate that environment for local builds.
CI=true pnpm install --frozen-lockfile

# ── Build ─────────────────────────────────────────────────────────────────────

# Binary slot save/restore (issue #2189, fixing the pollution source behind
# #2178). The cross-build below overwrites the shared, arch-unqualified
# crates/zfb/binaries/{esbuild/esbuild,tailwindcss-v4} slots with
# darwin-x64 binaries (build.rs correctly detects the staged arm64 slots
# fail the x64 SHA-256 pins). Left alone, that leaves the main repo's slots
# wrong-arch for the host until something re-runs the build script
# natively. Save both slots now and restore them immediately after `cargo
# build` succeeds — not only at script exit, so the packaging/upload steps
# below don't run any longer than necessary with the slots polluted. An EXIT
# trap is armed first so a failure mid-build restores them too; it is
# disarmed again right after the explicit post-build restore so it doesn't
# fire a second time on normal script exit. Either path, a FAILED restore
# leaves the backup directory on disk (the helper names it plus a per-slot
# recovery mapping on stderr) — it is then the only copy of the originals.
source "${repo_root}/scripts/release-binary-slot-restore.sh"
save_binary_slots

_slot_restore_on_exit() {
  local exit_status=$?
  if ! restore_binary_slots; then
    echo "ERROR: binary slot restore failed while handling script exit (original exit status ${exit_status}) — repo slots may be left wrong-arch until the next native build" >&2
    echo "       The pre-build originals were RETAINED (not deleted) at: ${BINARY_SLOT_BACKUP_DIR} — see the per-slot recovery mapping printed above" >&2
  fi
  exit "$exit_status"
}
trap _slot_restore_on_exit EXIT

echo "==> cargo build -p zfb --release --target ${target}"
ZFB_RELEASE_VERSION="$semver" cargo build -p zfb --release --target "$target"

_slot_restore_status=0
restore_binary_slots || _slot_restore_status=$?
trap - EXIT
if [[ "$_slot_restore_status" -ne 0 ]]; then
  echo "ERROR: failed to restore one or more binary slots to their pre-build state after the cross-target build" >&2
  echo "       The pre-build originals were RETAINED (not deleted) at: ${BINARY_SLOT_BACKUP_DIR} — see the per-slot recovery mapping printed above" >&2
  exit 1
fi

# Resolve the actual cargo target directory rather than assuming "./target".
# A host-level ~/.cargo/config.toml (or CARGO_TARGET_DIR) can redirect builds
# elsewhere; `cargo metadata` reports the effective location regardless.
target_root="$(cargo metadata --format-version 1 --no-deps \
  | node -e "let d='';process.stdin.on('data',c=>d+=c).on('end',()=>console.log(JSON.parse(d).target_directory))")"
built_binary="${target_root}/${target}/release/zfb"
if [[ ! -f "$built_binary" ]]; then
  echo "ERROR: expected binary not found: ${built_binary}" >&2
  exit 1
fi

# ── Assert -V equals release semver (mirrors release.yml) ─────────────────────
# Rosetta path: run the binary and assert short `-V` output equals
# "zfb $semver" — the only STRONG check this script performs. It catches a
# missed ZFB_RELEASE_VERSION injection before the binary is archived.
# Fix (b): on Apple Silicon without Rosetta 2 the x86_64 binary cannot execute
# (Bad CPU type in executable), so fall back to a static rodata scan. That
# fallback is ONE-WAY (issue #748): the semver being ABSENT proves the stamp
# was never injected (hard fail), but its PRESENCE cannot prove a correct
# stamp — so on a match the script only warns and continues; it does NOT
# claim the version was verified.
#
# Empirical facts behind the one-way design (issue #748 — measured against a
# real stamped x64 binary; do not re-attempt these disproved "fixes"):
#   - clap stores the program name ("zfb") and the version as SEPARATE rodata
#     strings: grep -aF "zfb $semver" NEVER matches even a correctly-stamped
#     binary, so the contiguous literal cannot be asserted.
#   - Bare-substring signals are noisy in BOTH directions: "0.0.0" appears
#     ~32x (vendored package metadata), the real semver ~9x, and even "9.9.9"
#     matches a rodata digit lookup table — so neither a positive bare-semver
#     match nor a "0.0.0 placeholder absent" assertion is a reliable guard.
# The version string is embedded via option_env!("ZFB_RELEASE_VERSION") in
# crates/zfb/src/cli.rs and appears literally in the binary's read-only data.
expected_version="zfb $semver"
if arch -x86_64 /usr/bin/true 2>/dev/null; then
  # Rosetta present — execute the binary directly (original assertion).
  actual_version="$("$built_binary" -V)"
  echo "Expected: $expected_version"
  echo "Actual:   $actual_version"
  if [[ "$actual_version" != "$expected_version" ]]; then
    echo "ERROR: built binary -V mismatch: got '$actual_version', want '$expected_version'" >&2
    echo "       (ZFB_RELEASE_VERSION injection likely missing)" >&2
    exit 1
  fi
else
  # No Rosetta — best-effort static scan (one-way; see comment block above).
  echo "==> Rosetta absent — best-effort static scan for '$semver' (NOT a real verification)"
  if ! grep -aF "$semver" "$built_binary" >/dev/null 2>&1; then
    echo "ERROR: '$semver' not found anywhere in the binary." >&2
    echo "       ZFB_RELEASE_VERSION injection definitely failed — the stamped" >&2
    echo "       version would appear literally in the binary's rodata." >&2
    exit 1
  fi
  echo "WARNING: version stamp NOT verified — only a best-effort substring scan ran." >&2
  echo "         '$semver' appears somewhere in the binary, but a bare-substring" >&2
  echo "         match cannot distinguish the injected version stamp from vendored" >&2
  echo "         package metadata, nor '0.1.0' from '0.1.0-next.N' (issue #748)." >&2
  echo "         For a real check: install Rosetta 2 (softwareupdate --install-rosetta)" >&2
  echo "         and re-run, or rely on release CI's -V assertions" >&2
  echo "         (.github/workflows/release.yml) for published artifacts." >&2
fi

# ── Place binary in the platform package (mirrors release.yml) ────────────────
# The archive is created with `-C <platform_pkg>` so its single root entry is
# exactly "zfb" with no nested directory.

cp "$built_binary" "${platform_pkg}/zfb"
chmod +x "${platform_pkg}/zfb"

# ── Create archive (Unix tar.gz, contract format) ─────────────────────────────

archive_path="${out_dir}/${archive}"
echo "==> Creating archive ${archive_path}"
tar -C "$platform_pkg" -czf "$archive_path" zfb

# ── Generate sha256 checksum (GNU two-space format) ───────────────────────────
# Prefer coreutils sha256sum; fall back to the always-present macOS `shasum`.
# Both emit "<hash>  <basename>" (two spaces). Run from out_dir so the file
# records only the basename (installers rely on basename-only).

echo "==> Generating ${archive}.sha256"
if command -v sha256sum >/dev/null 2>&1; then
  ( cd "$out_dir" && sha256sum "$archive" > "${archive}.sha256" )
else
  ( cd "$out_dir" && shasum -a 256 "$archive" > "${archive}.sha256" )
fi
echo "--- checksum file contents ---"
cat "${archive_path}.sha256"

echo "==> Built:"
echo "    ${archive_path}"
echo "    ${archive_path}.sha256"

# ── Optional upload to the GH Release ─────────────────────────────────────────

if [[ -n "$upload_tag" ]]; then
  # Normally `/l-make-release` creates the draft Release before this script runs.
  # If the draft does not exist yet, create one now (as a draft so that publishing
  # it later — after assets are uploaded — is what fires `release: published`).
  # IMPORTANT: Must be created as --draft so the release: published event fires
  # only after the user explicitly publishes it, not during this upload step.
  if ! gh release view "$upload_tag" >/dev/null 2>&1; then
    echo "==> GH Release ${upload_tag} does not exist yet — creating a draft"
    # Match the channel policy: *-next.* / *-beta.* / *-rc.* tags are prereleases.
    prerelease_flag=""
    if [[ "$upload_tag" =~ -next\. ]] || [[ "$upload_tag" =~ -beta\. ]] || [[ "$upload_tag" =~ -rc\. ]]; then
      prerelease_flag="--prerelease"
    fi
    gh release create "$upload_tag" \
      --title "$upload_tag" \
      --notes "Release ${upload_tag} (macOS-x64 pre-built locally; remaining assets uploaded by release.yml)" \
      --draft \
      $prerelease_flag
  fi

  echo "==> Uploading to GitHub Release ${upload_tag} (--clobber)"
  # --clobber makes re-runs idempotent (overwrites an existing asset of the
  # same name instead of erroring).
  gh release upload "$upload_tag" \
    "$archive_path" \
    "${archive_path}.sha256" \
    --clobber
  echo "==> Uploaded. Now publish the draft GH Release (gh release edit ${upload_tag} --draft=false or web UI) to trigger release.yml."
  echo "    The workflow's detect-mac-local job will see the pre-uploaded archive and skip the macos-15-intel build leg."
else
  echo ""
  echo "Not uploaded. To attach these to the draft GH Release for <tag>, run:"
  echo "  gh release upload <tag> \\"
  echo "    \"${archive_path}\" \\"
  echo "    \"${archive_path}.sha256\" --clobber"
  echo "Then publish the draft Release (gh release edit <tag> --draft=false or web UI) to trigger release.yml."
  echo "The workflow's detect-mac-local job will see the pre-uploaded archive and skip the macos-15-intel build leg."
fi
