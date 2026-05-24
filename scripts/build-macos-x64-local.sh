#!/usr/bin/env bash
# build-macos-x64-local.sh
#
# Escape hatch for the chronically queue-starved GHA `macos-13` (legacy Intel)
# runner (issue #437). Builds the macOS-x64 (`x86_64-apple-darwin`) release
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

# ── Toolchain target (idempotent — Apple Silicon hosts need this once) ────────

echo "==> Ensuring rust target ${target} is installed"
rustup target add "$target"

# ── Install node deps before cargo build ──────────────────────────────────────
# build.rs embeds framework packages from node_modules at compile time.

echo "==> Installing node dependencies (pnpm install --frozen-lockfile)"
# CI=true so pnpm runs non-interactively. pnpm 11 (#440) purges a node_modules
# left by an incompatible pnpm version and would otherwise prompt for
# confirmation — which aborts under no-TTY. The GHA runner gets CI=true for
# free; set it here to replicate that environment for local builds.
CI=true pnpm install --frozen-lockfile

# ── Build ─────────────────────────────────────────────────────────────────────

echo "==> cargo build -p zfb --release --target ${target}"
cargo build -p zfb --release --target "$target"

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
  # In the documented recovery flow the user cancels the stalled tag run, so the
  # release-assets job (which creates the GH Release) never ran — the Release may
  # not exist yet. `gh release upload` requires an existing Release, so create a
  # minimal one first. The re-dispatched release.yml release-assets job later
  # updates this same Release with the other 4 archives and the final prerelease
  # flag (softprops/action-gh-release updates an existing release in place).
  if ! gh release view "$upload_tag" >/dev/null 2>&1; then
    echo "==> GH Release ${upload_tag} does not exist yet — creating it"
    # Match the channel policy: *-next.* / *-beta.* / *-rc.* tags are prereleases.
    prerelease_flag=""
    if [[ "$upload_tag" =~ -next\. ]] || [[ "$upload_tag" =~ -beta\. ]] || [[ "$upload_tag" =~ -rc\. ]]; then
      prerelease_flag="--prerelease"
    fi
    gh release create "$upload_tag" \
      --title "$upload_tag" \
      --notes "Release ${upload_tag} (macOS-x64 built locally via escape hatch — issue #437; remaining assets uploaded by release.yml)" \
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
  echo "    The workflow's detect-mac-local job will see the pre-uploaded archive and skip the macos-13 build leg."
else
  echo ""
  echo "Not uploaded. To attach these to the draft GH Release for <tag>, run:"
  echo "  gh release upload <tag> \\"
  echo "    \"${archive_path}\" \\"
  echo "    \"${archive_path}.sha256\" --clobber"
  echo "Then publish the draft Release (gh release edit <tag> --draft=false or web UI) to trigger release.yml."
  echo "The workflow's detect-mac-local job will see the pre-uploaded archive and skip the macos-13 build leg."
fi
