#!/bin/sh
# shellcheck shell=sh
#
# install.sh — curl installer for zfb (macOS + Linux)
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Takazudo/zudo-front-builder/main/install.sh | sh
#
# Environment variables:
#   ZFB_VERSION    Pin a release.  Formats accepted:
#                    v0.X.Y              — specific stable or prerelease tag
#                    latest-prerelease  — most recent prerelease
#                    (unset)            — latest non-prerelease (default)
#   ZFB_INSTALL    Installation prefix. Binary lands at $ZFB_INSTALL/bin/zfb.
#                    Default: $HOME/.local
#
# Reference shape: rustup, deno, bun install scripts.

set -eu

# ── Constants ────────────────────────────────────────────────────────────────

REPO="Takazudo/zudo-front-builder"
ASSET_BASE="https://github.com/${REPO}/releases/download"
API_BASE="https://api.github.com/repos/${REPO}/releases"

# ── Helpers ──────────────────────────────────────────────────────────────────

say() {
  printf '%s\n' "$1"
}

err() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    err "required command not found: $1"
  fi
}

# ── Temporary directory with guaranteed cleanup ───────────────────────────────

tmp=""
cleanup() {
  if [ -n "$tmp" ] && [ -d "$tmp" ]; then
    rm -rf "$tmp"
  fi
}
trap cleanup EXIT INT TERM

tmp=$(mktemp -d)

# ── Platform detection ────────────────────────────────────────────────────────

detect_target() {
  _os=$(uname -s)
  _arch=$(uname -m)

  case "$_os" in
    Darwin)
      case "$_arch" in
        arm64|aarch64)  printf '%s' "aarch64-apple-darwin" ; return ;;
        x86_64)         printf '%s' "x86_64-apple-darwin"  ; return ;;
        *)              err "unsupported macOS architecture: $_arch" ;;
      esac
      ;;
    Linux)
      case "$_arch" in
        aarch64|arm64)  printf '%s' "aarch64-unknown-linux-gnu" ; return ;;
        x86_64|amd64)   printf '%s' "x86_64-unknown-linux-gnu"  ; return ;;
        *)              err "unsupported Linux architecture: $_arch" ;;
      esac
      ;;
    *)
      err "unsupported operating system: $_os (this script supports macOS and Linux only)"
      ;;
  esac
}

# ── Version resolution ────────────────────────────────────────────────────────

resolve_tag() {
  _version="${ZFB_VERSION:-}"

  if [ -z "$_version" ]; then
    # Default: latest non-prerelease GitHub Release (prereleases are excluded automatically)
    _tag=$(curl -fsSL "${API_BASE}/latest" \
           | grep -m1 '"tag_name":' \
           | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/')
    if [ -z "$_tag" ]; then
      err "could not resolve the latest release tag from GitHub API"
    fi
    printf '%s' "$_tag"
    return
  fi

  if [ "$_version" = "latest-prerelease" ]; then
    # Pick the first release entry where "prerelease": true. The naive
    # ?per_page=1 query just returns the most-recent release of ANY kind, so
    # once a stable release is published after a prerelease, "latest-prerelease"
    # would silently resolve to that stable tag. awk splits on '}' so each
    # record is one release object; the first one marked prerelease wins.
    _tag=$(curl -fsSL "${API_BASE}?per_page=30" \
           | awk -v RS='}' '/"prerelease":[[:space:]]*true/ { print; exit }' \
           | grep -m1 '"tag_name":' \
           | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/')
    if [ -z "$_tag" ]; then
      err "could not resolve the latest prerelease tag from GitHub API (no prerelease found in the last 30 releases)"
    fi
    printf '%s' "$_tag"
    return
  fi

  # Specific tag — must start with 'v' (mirrors install.ps1 rejection logic).
  case "$_version" in
    v*) printf '%s' "$_version" ;;
    *) err "ZFB_VERSION must start with 'v' (e.g. v0.2.0). Got: $_version" ;;
  esac
}

# ── Checksum helper ───────────────────────────────────────────────────────────

sha256_file() {
  # Compute SHA-256 of $1; print only the 64-hex-lowercase hash.
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    err "no sha256sum or shasum found — cannot verify checksum"
  fi
}

# ── Main ──────────────────────────────────────────────────────────────────────

main() {
  need_cmd curl
  need_cmd tar

  # Detect target triple
  target=$(detect_target)

  # Resolve install prefix
  install_dir="${ZFB_INSTALL:-${HOME}/.local}"
  bin_dir="${install_dir}/bin"

  # Resolve version tag
  say "Resolving zfb version..."
  tag=$(resolve_tag)
  # Archive uses semver without leading "v"
  semver="${tag#v}"
  say "  tag:    ${tag}"
  say "  semver: ${semver}"
  say "  target: ${target}"

  archive_name="zfb-${semver}-${target}.tar.gz"
  checksum_name="${archive_name}.sha256"
  archive_url="${ASSET_BASE}/${tag}/${archive_name}"
  checksum_url="${ASSET_BASE}/${tag}/${checksum_name}"

  # Download archive
  say "Downloading ${archive_name}..."
  curl -fSL --progress-bar -o "${tmp}/${archive_name}" "$archive_url"

  # Download checksum file
  say "Downloading ${checksum_name}..."
  curl -fSL --progress-bar -o "${tmp}/${checksum_name}" "$checksum_url"

  # Verify checksum
  say "Verifying checksum..."
  # Parse the first whitespace-delimited field (64-hex hash) from the .sha256 file.
  # Do NOT use shasum -c — portability is unreliable across platforms.
  expected=$(awk '{print $1}' "${tmp}/${checksum_name}")
  actual=$(sha256_file "${tmp}/${archive_name}")

  if [ "$actual" != "$expected" ]; then
    printf 'error: checksum mismatch!\n' >&2
    printf '  expected: %s\n' "$expected" >&2
    printf '  actual:   %s\n' "$actual" >&2
    exit 1
  fi

  say "  checksum OK"

  # Extract to temp dir then install
  say "Extracting..."
  tar -C "$tmp" -xzf "${tmp}/${archive_name}"

  mkdir -p "$bin_dir"
  cp "${tmp}/zfb" "${bin_dir}/zfb"
  chmod +x "${bin_dir}/zfb"

  # Clear macOS quarantine (no-op on Linux; xattr may not exist there)
  # Notarisation is deferred; this removes the download quarantine flag added by
  # browsers/curl on macOS so the binary can run without a Gatekeeper prompt.
  xattr -dr com.apple.quarantine "${bin_dir}/zfb" 2>/dev/null || true

  say "Installed zfb ${semver} to ${bin_dir}/zfb"

  # PATH hint
  case ":${PATH}:" in
    *":${bin_dir}:"*)
      # Already on PATH — nothing to do
      ;;
    *)
      say ""
      say "To use zfb, add the following to your shell profile:"
      say ""
      say "  export PATH=\"${bin_dir}:\$PATH\""
      say ""
      ;;
  esac
}

main
