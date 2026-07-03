#!/usr/bin/env bash
set -euo pipefail

# scripts/smoke-packed-clean-room.sh
#
# PR-time (pre-publish) clean-room smoke for the `npm create zfb` consumption
# path (issue #1345). Companion to scripts/smoke-clean-room.sh, which runs
# the SAME shape of test AFTER publish, against the real registry. This
# script never touches the registry for the packages under test: it packs
# them into local tarballs, installs create-zfb from ITS tarball with
# pnpm.overrides pinning every @takazudo/* dependency at the local tarballs,
# scaffolds a site, builds it, and asserts on dist/ content — proving the
# packages actually work together before anything is published.
#
# This test class would have caught two real past incidents pre-publish:
#   - the 0.1.0-next.27 packaging break (files allowlist omitted a file that
#     bin/cli.mjs / dist/build.js imported at runtime — ERR_MODULE_NOT_FOUND
#     for every adapter consumer). packages/zfb-adapter-cloudflare's
#     packaging.test.ts guards this STATICALLY (files allowlist text vs.
#     import text) but never runs `npm pack` or installs the result, so it
#     cannot prove the tarball actually resolves at runtime — this script can.
#   - the #447/#441 exec-bit strip (`pnpm pack`/`pnpm publish` normalises file
#     modes to 0644, silently shipping a non-executable platform binary).
#     Stage 1 below packs the platform binary with `npm pack` (matching
#     scripts/publish-npm-packages.sh's production behavior) and asserts the
#     0755 bit survives into the tarball.
#
# Known gap (documented, not fixed here): the 4 platform optionalDependencies
# NOT relevant to the linux-x64 CI runner (zfb-darwin-arm64, zfb-darwin-x64,
# zfb-linux-arm64-gnu, zfb-win32-x64-msvc) are left un-overridden. pnpm still
# resolves their MANIFEST (a lightweight metadata call, not a tarball
# download) against the real registry to evaluate the os/cpu match, then
# skips installing them (they're optionalDependencies — an unresolvable
# entry is a soft skip, not a hard failure, so this is safe even when a PR's
# version isn't published yet). Overriding all 5 to fully avoid the registry
# would require synthesizing 4 stub tarballs with correct os/cpu fields for
# packages this job cannot otherwise test; not worth the complexity today.
#
# Requires the workspace JS build to already be up to date for the packages
# under test (pnpm install + pnpm --filter ... build) — this script only
# packs, overrides, scaffolds, builds, and asserts.
#
# Usage:
#   ZFB_LINUX_X64_BINARY=/path/to/zfb scripts/smoke-packed-clean-room.sh
#
# Required env:
#   ZFB_LINUX_X64_BINARY — path to a freshly built x86_64-unknown-linux-gnu
#                           release `zfb` binary (from build-binary's staged
#                           smoke-ctx artifact) to inject into
#                           packages/zfb-linux-x64-gnu/ before packing.

: "${ZFB_LINUX_X64_BINARY:?ZFB_LINUX_X64_BINARY env var is required (path to the built zfb binary)}"

pass() { printf '[PASS] %s\n' "$1"; }
fail() { printf '[FAIL] %s\n' "$1" >&2; exit 1; }

if [[ ! -f packages/zfb/package.json ]]; then
  echo "::error::must be run from the repo root (packages/zfb/package.json not found in $(pwd))"
  exit 2
fi
if [[ ! -f "$ZFB_LINUX_X64_BINARY" ]]; then
  echo "::error::ZFB_LINUX_X64_BINARY does not exist: $ZFB_LINUX_X64_BINARY"
  exit 2
fi

# ── Stage 1: inject the freshly built binary, pack it, assert its exec bit ──

PLATFORM_DIR="packages/zfb-linux-x64-gnu"
cp "$ZFB_LINUX_X64_BINARY" "$PLATFORM_DIR/zfb"
chmod 0755 "$PLATFORM_DIR/zfb"

TARBALL_DIR="$(mktemp -d)"
echo "Packing tarballs into $TARBALL_DIR ..."

# `npm pack`, NOT `pnpm pack`: pnpm pack normalises file modes to 0644,
# stripping the 0755 exec bit off the `zfb` binary (see
# scripts/publish-npm-packages.sh's PLATFORM_DIRS comment — same reason
# `npm publish` is used there, not pnpm). Using pnpm here would silently
# launder the exact class of bug (#447/#441) this test exists to catch.
PLATFORM_TARBALL_NAME="$(cd "$PLATFORM_DIR" && npm pack --pack-destination "$TARBALL_DIR" | tail -1)"
PLATFORM_TARBALL_PATH="$TARBALL_DIR/$PLATFORM_TARBALL_NAME"
echo "Packed platform binary: $PLATFORM_TARBALL_PATH"

# Assert the exec bit (0755) survived into the tarball. `tar -tvf`'s first
# field is the symbolic mode (e.g. -rwxr-xr-x); matched as a literal string
# rather than parsed to octal, to avoid depending on a specific tar
# implementation's numeric-mode output format.
PLATFORM_BIN_LINE="$(tar -tvf "$PLATFORM_TARBALL_PATH" | grep -E 'package/zfb$' || true)"
if [[ -z "$PLATFORM_BIN_LINE" ]]; then
  fail "packed platform tarball does not contain package/zfb: $PLATFORM_TARBALL_PATH"
fi
if ! printf '%s\n' "$PLATFORM_BIN_LINE" | grep -qE '^-rwxr-xr-x'; then
  fail "packed zfb binary is not 0755 in the tarball (exec bit stripped — #447/#441-class regression): $PLATFORM_BIN_LINE"
fi
pass "packed zfb binary retains 0755 exec bit in the tarball ($PLATFORM_BIN_LINE)"

# ── Stage 2: pack the remaining publishable packages ────────────────────────
# `pnpm pack` (not npm) so pnpm rewrites `workspace:*` deps to concrete
# versions at pack time, same as scripts/publish-npm-packages.sh's
# NONPLATFORM_DIRS. None of these ship an executable file, so pnpm pack's
# 0644 mode normalisation is a non-issue here.

# Unlike `npm pack` (which prints just the filename on stdout, notices go to
# stderr), `pnpm pack --pack-destination` prints the full ABSOLUTE path of
# the produced tarball as its last stdout line — so this helper returns that
# path as-is, with no `$TARBALL_DIR/` prefix added by the caller.
pnpm_pack() {
  (cd "$1" && pnpm pack --pack-destination "$TARBALL_DIR" | tail -1)
}

ZFB_TARBALL="$(pnpm_pack packages/zfb)"
ZFB_RUNTIME_TARBALL="$(pnpm_pack packages/zfb-runtime)"
ZFB_ADAPTER_CF_TARBALL="$(pnpm_pack packages/zfb-adapter-cloudflare)"
CREATE_ZFB_TARBALL="$(pnpm_pack packages/create-zfb)"

for t in "$ZFB_TARBALL" "$ZFB_RUNTIME_TARBALL" "$ZFB_ADAPTER_CF_TARBALL" "$CREATE_ZFB_TARBALL"; do
  [[ -f "$t" ]] || fail "expected tarball not found: $t"
done
pass "packed zfb, zfb-runtime, zfb-adapter-cloudflare, create-zfb"

# ── Stage 3: clean room — install create-zfb from its tarball, LOCAL overrides ──
# Work in a temp dir completely outside the checked-out workspace, same
# rationale as scripts/smoke-clean-room.sh: avoids pnpm picking up the
# monorepo's pnpm-workspace.yaml.

CLEAN_ROOM_DIR="$(mktemp -d)"
echo "Clean room: $CLEAN_ROOM_DIR"

# pnpm.overrides forces every occurrence of these package names anywhere in
# the dependency graph to resolve to the local tarball, REGARDLESS of the
# version range originally declared (create-zfb's packed manifest pins an
# exact version; a locally-built dev binary that stamps `zfb new` scaffolds
# with the `0.0.0` placeholder version would otherwise mismatch it). This is
# what makes the test prove the LOCAL build works, rather than silently
# falling back to whatever the registry already has at that version number.
ZFB_TARBALL="$ZFB_TARBALL" \
ZFB_RUNTIME_TARBALL="$ZFB_RUNTIME_TARBALL" \
ZFB_ADAPTER_CF_TARBALL="$ZFB_ADAPTER_CF_TARBALL" \
PLATFORM_TARBALL_PATH="$PLATFORM_TARBALL_PATH" \
CREATE_ZFB_TARBALL="$CREATE_ZFB_TARBALL" \
CLEAN_ROOM_DIR="$CLEAN_ROOM_DIR" \
node -e '
const fs = require("node:fs");
const path = require("node:path");
const env = process.env;
const overrides = {
  "@takazudo/zfb": `file:${env.ZFB_TARBALL}`,
  "@takazudo/zfb-runtime": `file:${env.ZFB_RUNTIME_TARBALL}`,
  "@takazudo/zfb-adapter-cloudflare": `file:${env.ZFB_ADAPTER_CF_TARBALL}`,
  "@takazudo/zfb-linux-x64-gnu": `file:${env.PLATFORM_TARBALL_PATH}`,
};
const pkg = {
  name: "zfb-packed-clean-room",
  private: true,
  version: "0.0.0",
  dependencies: { "create-zfb": `file:${env.CREATE_ZFB_TARBALL}` },
  pnpm: { overrides },
};
fs.writeFileSync(
  path.join(env.CLEAN_ROOM_DIR, "package.json"),
  JSON.stringify(pkg, null, 2) + "\n",
);
'

(cd "$CLEAN_ROOM_DIR" && pnpm install)

if [[ ! -f "$CLEAN_ROOM_DIR/node_modules/create-zfb/bin/create-zfb.mjs" ]]; then
  fail "create-zfb did not install from its local tarball"
fi
# @takazudo/zfb-linux-x64-gnu is a TRANSITIVE (optional) dependency of the
# overridden @takazudo/zfb, not a direct dependency of the clean-room root —
# pnpm's isolated node_modules only hoists direct deps to the top level, so
# it lives inside node_modules/.pnpm/... instead. `find` locates it
# regardless of the exact store layout.
INSTALLED_BINARY="$(find "$CLEAN_ROOM_DIR/node_modules" -path '*/@takazudo/zfb-linux-x64-gnu/zfb' -type f | head -1)"
if [[ -z "$INSTALLED_BINARY" ]]; then
  fail "@takazudo/zfb-linux-x64-gnu (override) did not install the platform binary"
fi
INSTALLED_BINARY_MODE="$(stat -c '%a' "$INSTALLED_BINARY" 2>/dev/null || stat -f '%Lp' "$INSTALLED_BINARY")"
if [[ "$INSTALLED_BINARY_MODE" != "755" ]]; then
  fail "installed platform binary is not 0755 (got $INSTALLED_BINARY_MODE) — exec bit lost somewhere between pack and install"
fi
pass "create-zfb + @takazudo/zfb-linux-x64-gnu installed from local tarballs (binary mode 0755)"

# ── Stage 4: scaffold via the packed create-zfb ──────────────────────────────
# `zfb new` rejects path separators in the project name (validate_project_name
# in crates/zfb/src/commands/new.rs), so a single-segment relative name is
# required. Invoked directly (not via `pnpm exec`/`npx`) so there is no
# ambiguity about which install this resolves against.

SITE_NAME="packed-e2e-site"
SITE_DIR="$CLEAN_ROOM_DIR/$SITE_NAME"

(cd "$CLEAN_ROOM_DIR" && node "$CLEAN_ROOM_DIR/node_modules/create-zfb/bin/create-zfb.mjs" "$SITE_NAME")

if [[ ! -f "$SITE_DIR/package.json" ]]; then
  fail "scaffold did not produce $SITE_DIR/package.json"
fi
pass "scaffold produced $SITE_DIR"

# ── Stage 5: install + build the scaffolded project ──────────────────────────
# `zfb new` already attempted its own best-effort `pnpm install` against the
# scaffolded package.json's exact-pinned version (which the registry has no
# reason to satisfy for a local/dev binary build) — that failure is
# non-fatal (crates/zfb/src/commands/new.rs's PnpmOutcome::Failed only
# warns), but may have left a partial node_modules/lockfile. Clear it before
# our own override-driven install so Stage 5 starts from a known-clean state.
rm -rf "$SITE_DIR/node_modules" "$SITE_DIR/pnpm-lock.yaml"

# Same override set as Stage 3, re-applied to the SCAFFOLDED project's own
# package.json: it is its own independent pnpm project root (no
# pnpm-workspace.yaml links it back to the clean-room dir), so pnpm.overrides
# must be declared here too, not just in the clean-room root.
ZFB_TARBALL="$ZFB_TARBALL" \
ZFB_RUNTIME_TARBALL="$ZFB_RUNTIME_TARBALL" \
PLATFORM_TARBALL_PATH="$PLATFORM_TARBALL_PATH" \
SITE_DIR="$SITE_DIR" \
node -e '
const fs = require("node:fs");
const path = require("node:path");
const env = process.env;
const pkgPath = path.join(env.SITE_DIR, "package.json");
const pkg = JSON.parse(fs.readFileSync(pkgPath, "utf8"));
pkg.pnpm = pkg.pnpm || {};
pkg.pnpm.overrides = {
  ...(pkg.pnpm.overrides || {}),
  "@takazudo/zfb": `file:${env.ZFB_TARBALL}`,
  "@takazudo/zfb-runtime": `file:${env.ZFB_RUNTIME_TARBALL}`,
  "@takazudo/zfb-linux-x64-gnu": `file:${env.PLATFORM_TARBALL_PATH}`,
};
fs.writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + "\n");
'

(cd "$SITE_DIR" && pnpm install)
(cd "$SITE_DIR" && pnpm build)

# ── Stage 6: assert dist/ content ────────────────────────────────────────────
# Mirrors the assertion style used by scripts/smoke-clean-room.sh and
# tests/smoke/node-free/run.sh: grep rendered HTML for markers that can only
# be present if the content pipeline actually ran. `create-zfb <name>` (no
# --template) scaffolds the "basic-blog" template (crates/zfb/src/cli.rs
# default_value), whose pages/index.tsx renders `<h1>basic-blog</h1>` and
# lists every post in the `blog` content collection, including the seed
# post's frontmatter title "Hello, zfb" (content/blog/hello-zfb.mdx).

DIST_INDEX="$SITE_DIR/dist/index.html"

DIST_FILE=$(find "$SITE_DIR/dist" -type f | head -1)
if [[ -z "$DIST_FILE" ]]; then
  fail "dist/ is empty after build — packed-tarball scaffold or build is broken."
fi
pass "dist/ is populated (first file: $DIST_FILE)"

if [[ ! -f "$DIST_INDEX" ]]; then
  fail "dist/index.html not found after build."
fi
pass "dist/index.html exists"

if ! grep -q "basic-blog" "$DIST_INDEX"; then
  fail "dist/index.html does not contain expected content: basic-blog"
fi
pass "dist/index.html contains expected content (basic-blog)"

if ! grep -q "Hello, zfb" "$DIST_INDEX"; then
  fail "dist/index.html does not list the seed post title: Hello, zfb"
fi
pass "dist/index.html lists the seed post (Hello, zfb)"

echo "All packed-clean-room smoke assertions passed."
