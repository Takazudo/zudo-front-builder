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
# NOT relevant to the platform under test (on the linux-x64 CI runner:
# zfb-darwin-arm64, zfb-darwin-x64, zfb-linux-arm64-gnu, zfb-win32-x64-msvc)
# are left un-overridden. pnpm still
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
#   ZFB_SMOKE_PLATFORM=darwin-arm64 ZFB_DARWIN_ARM64_BINARY=/path/to/zfb \
#     ZFB_DARWIN_ARM64_CSS_ADDON=/path/to/zfb-css.node \
#     scripts/smoke-packed-clean-room.sh
#
# Platform under test (ZFB_SMOKE_PLATFORM, default linux-x64-gnu — the CI
# runner, so the required Scaffold E2E check is unchanged). It must match the
# host: the packed platform package is only installed when its os/cpu fields
# match, so a mismatched host would silently skip it. A darwin-* run is native
# Darwin evidence only when run on that Darwin architecture (#4231 item 2); a
# darwin-x64 run on an Apple-silicon Mac under Rosetta is not native arm64.
#
#   platform        packed package           binary env                 CSS addon env
#   linux-x64-gnu   zfb-linux-x64-gnu        ZFB_LINUX_X64_BINARY       ZFB_LINUX_X64_CSS_ADDON
#   darwin-arm64    zfb-darwin-arm64         ZFB_DARWIN_ARM64_BINARY    ZFB_DARWIN_ARM64_CSS_ADDON
#   darwin-x64      zfb-darwin-x64           ZFB_DARWIN_X64_BINARY      ZFB_DARWIN_X64_CSS_ADDON
#
# Required env: the platform's binary env (path to a freshly built release
# `zfb` binary to inject into packages/zfb-<platform>/ before packing) and its
# CSS addon env (a freshly built zfb-css.node).

ZFB_SMOKE_PLATFORM="${ZFB_SMOKE_PLATFORM:-linux-x64-gnu}"
case "$ZFB_SMOKE_PLATFORM" in
  linux-x64-gnu) HOST_OS=Linux; HOST_ARCH=x86_64; BINARY_VAR=ZFB_LINUX_X64_BINARY; ADDON_VAR=ZFB_LINUX_X64_CSS_ADDON ;;
  darwin-arm64) HOST_OS=Darwin; HOST_ARCH=arm64; BINARY_VAR=ZFB_DARWIN_ARM64_BINARY; ADDON_VAR=ZFB_DARWIN_ARM64_CSS_ADDON ;;
  darwin-x64) HOST_OS=Darwin; HOST_ARCH=x86_64; BINARY_VAR=ZFB_DARWIN_X64_BINARY; ADDON_VAR=ZFB_DARWIN_X64_CSS_ADDON ;;
  *)
    echo "::error::unsupported ZFB_SMOKE_PLATFORM '$ZFB_SMOKE_PLATFORM' (expected linux-x64-gnu, darwin-arm64 or darwin-x64)" >&2
    exit 2
    ;;
esac
if [[ "$(uname -s)" != "$HOST_OS" || "$(uname -m)" != "$HOST_ARCH" ]]; then
  echo "::error::ZFB_SMOKE_PLATFORM=$ZFB_SMOKE_PLATFORM needs a $HOST_OS/$HOST_ARCH host, got $(uname -s)/$(uname -m)" >&2
  exit 2
fi
ZFB_SMOKE_BINARY="${!BINARY_VAR:-}"
ZFB_SMOKE_CSS_ADDON="${!ADDON_VAR:-}"
[[ -n "$ZFB_SMOKE_BINARY" ]] || { echo "::error::$BINARY_VAR env var is required (path to the built zfb binary)" >&2; exit 2; }
[[ -n "$ZFB_SMOKE_CSS_ADDON" ]] || { echo "::error::$ADDON_VAR env var is required (freshly built CSS addon)" >&2; exit 2; }
PLATFORM_PKG="zfb-$ZFB_SMOKE_PLATFORM"

pass() { printf '[PASS] %s\n' "$1"; }
fail() { printf '[FAIL] %s\n' "$1" >&2; exit 1; }

if [[ ! -f packages/zfb/package.json ]]; then
  echo "::error::must be run from the repo root (packages/zfb/package.json not found in $(pwd))"
  exit 2
fi
if [[ ! -f "$ZFB_SMOKE_BINARY" ]]; then
  echo "::error::$BINARY_VAR does not exist: $ZFB_SMOKE_BINARY"
  exit 2
fi

# ── Stage 1: inject the freshly built binary, pack it, assert its exec bit ──

PLATFORM_DIR="packages/$PLATFORM_PKG"
cp "$ZFB_SMOKE_CSS_ADDON" "$PLATFORM_DIR/zfb-css.node"
chmod 0644 "$PLATFORM_DIR/zfb-css.node"
node scripts/smoke-css-node.mjs "$PLATFORM_DIR/zfb-css.node"
cp "$ZFB_SMOKE_BINARY" "$PLATFORM_DIR/zfb"
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

ZFB_SLUGIFY_TARBALL="$(pnpm_pack packages/zfb-slugify)"
ZFB_TARBALL="$(pnpm_pack packages/zfb)"
ZFB_RUNTIME_TARBALL="$(pnpm_pack packages/zfb-runtime)"
ZFB_ADAPTER_CF_TARBALL="$(pnpm_pack packages/zfb-adapter-cloudflare)"
CREATE_ZFB_TARBALL="$(pnpm_pack packages/create-zfb)"

for t in "$ZFB_SLUGIFY_TARBALL" "$ZFB_TARBALL" "$ZFB_RUNTIME_TARBALL" "$ZFB_ADAPTER_CF_TARBALL" "$CREATE_ZFB_TARBALL"; do
  [[ -f "$t" ]] || fail "expected tarball not found: $t"
done
pass "packed zfb-slugify, zfb, zfb-runtime, zfb-adapter-cloudflare, create-zfb"

# ── Stage 3: clean room — install create-zfb from its tarball, LOCAL overrides ──
# Work in a temp dir completely outside the checked-out workspace, same
# rationale as scripts/smoke-clean-room.sh: avoids pnpm picking up the
# monorepo's pnpm-workspace.yaml.

CLEAN_ROOM_DIR="$(mktemp -d)"
echo "Clean room: $CLEAN_ROOM_DIR"

# `overrides` forces every occurrence of these package names anywhere in
# the dependency graph to resolve to the local tarball, REGARDLESS of the
# version range originally declared (create-zfb's packed manifest pins an
# exact version; a locally-built dev binary that stamps `zfb new` scaffolds
# with the `0.0.0` placeholder version would otherwise mismatch it). This is
# what makes the test prove the LOCAL build works, rather than silently
# falling back to whatever the registry already has at that version number.
#
# pnpm 11 ignores `pnpm.overrides` in package.json (see this repo's own
# pnpm-workspace.yaml header comment) — overrides must live in
# pnpm-workspace.yaml instead, one per install root. Also set
# minimumReleaseAge: 0 here: without a working override this install would
# otherwise fall through to the real registry and could get blocked by that
# same-day-publish policy, masking the override bug as an unrelated
# supply-chain failure instead of the "did not install from local tarball"
# assertion below.
ZFB_SLUGIFY_TARBALL="$ZFB_SLUGIFY_TARBALL" \
ZFB_TARBALL="$ZFB_TARBALL" \
ZFB_RUNTIME_TARBALL="$ZFB_RUNTIME_TARBALL" \
ZFB_ADAPTER_CF_TARBALL="$ZFB_ADAPTER_CF_TARBALL" \
PLATFORM_TARBALL_PATH="$PLATFORM_TARBALL_PATH" \
PLATFORM_PKG="$PLATFORM_PKG" \
CREATE_ZFB_TARBALL="$CREATE_ZFB_TARBALL" \
CLEAN_ROOM_DIR="$CLEAN_ROOM_DIR" \
node -e '
const fs = require("node:fs");
const path = require("node:path");
const env = process.env;
const overrides = {
  "@takazudo/zfb": `file:${env.ZFB_TARBALL}`,
  "@takazudo/zfb-slugify": `file:${env.ZFB_SLUGIFY_TARBALL}`,
  "@takazudo/zfb-runtime": `file:${env.ZFB_RUNTIME_TARBALL}`,
  "@takazudo/zfb-adapter-cloudflare": `file:${env.ZFB_ADAPTER_CF_TARBALL}`,
  [`@takazudo/${env.PLATFORM_PKG}`]: `file:${env.PLATFORM_TARBALL_PATH}`,
};
const pkg = {
  name: "zfb-packed-clean-room",
  private: true,
  version: "0.0.0",
  dependencies: { "create-zfb": `file:${env.CREATE_ZFB_TARBALL}` },
};
fs.writeFileSync(
  path.join(env.CLEAN_ROOM_DIR, "package.json"),
  JSON.stringify(pkg, null, 2) + "\n",
);
const workspaceYaml = [
  "minimumReleaseAge: 0",
  "overrides:",
  ...Object.entries(overrides).map(([name, spec]) => `  "${name}": "${spec}"`),
  "",
].join("\n");
fs.writeFileSync(
  path.join(env.CLEAN_ROOM_DIR, "pnpm-workspace.yaml"),
  workspaceYaml,
);
'

(cd "$CLEAN_ROOM_DIR" && pnpm install)

if [[ ! -f "$CLEAN_ROOM_DIR/node_modules/create-zfb/bin/create-zfb.mjs" ]]; then
  fail "create-zfb did not install from its local tarball"
fi
# The platform package is a TRANSITIVE (optional) dependency of the
# overridden @takazudo/zfb, not a direct dependency of the clean-room root —
# pnpm's isolated node_modules only hoists direct deps to the top level, so
# it lives inside node_modules/.pnpm/... instead. `find` locates it
# regardless of the exact store layout.
INSTALLED_BINARY="$(find "$CLEAN_ROOM_DIR/node_modules" -path "*/@takazudo/$PLATFORM_PKG/zfb" -type f | head -1)"
if [[ -z "$INSTALLED_BINARY" ]]; then
  fail "@takazudo/$PLATFORM_PKG (override) did not install the platform binary"
fi
INSTALLED_BINARY_MODE="$(stat -c '%a' "$INSTALLED_BINARY" 2>/dev/null || stat -f '%Lp' "$INSTALLED_BINARY")"
if [[ "$INSTALLED_BINARY_MODE" != "755" ]]; then
  fail "installed platform binary is not 0755 (got $INSTALLED_BINARY_MODE) — exec bit lost somewhere between pack and install"
fi
pass "create-zfb + @takazudo/$PLATFORM_PKG installed from local tarballs (binary mode 0755)"

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

# Snapshot the pristine scaffold now, before Stage 5 installs and builds it, so
# Stage 7 can build the exact same source a second time. Taken here rather
# than copied from SITE_DIR after the build so no install or build output
# (node_modules, dist, .zfb-build) can leak into the second build.
LINKED_ROOM_DIR="$(mktemp -d)"
LINKED_SITE_DIR="$LINKED_ROOM_DIR/$SITE_NAME"
mkdir -p "$LINKED_SITE_DIR"
tar -C "$SITE_DIR" --exclude=./node_modules --exclude=./pnpm-lock.yaml -cf - . |
  tar -C "$LINKED_SITE_DIR" -xf -

# ── Stage 5: install + build the scaffolded project ──────────────────────────
# `zfb new` already attempted its own best-effort `pnpm install` against the
# scaffolded package.json's exact-pinned version (which the registry has no
# reason to satisfy for a local/dev binary build) — that failure is
# non-fatal (crates/zfb/src/commands/new.rs's PnpmOutcome::Failed only
# warns), but may have left a partial node_modules/lockfile. Clear it before
# our own override-driven install so Stage 5 starts from a known-clean state.
rm -rf "$SITE_DIR/node_modules" "$SITE_DIR/pnpm-lock.yaml"

# Same override set as Stage 3, re-applied to the SCAFFOLDED project's own
# install root (no pnpm-workspace.yaml links it back to the clean-room dir,
# so it needs its own). As in Stage 3, pnpm 11 only reads `overrides` from
# pnpm-workspace.yaml, not package.json's `pnpm` field — write it there.
ZFB_SLUGIFY_TARBALL="$ZFB_SLUGIFY_TARBALL" \
ZFB_TARBALL="$ZFB_TARBALL" \
ZFB_RUNTIME_TARBALL="$ZFB_RUNTIME_TARBALL" \
PLATFORM_TARBALL_PATH="$PLATFORM_TARBALL_PATH" \
PLATFORM_PKG="$PLATFORM_PKG" \
SITE_DIR="$SITE_DIR" \
node -e '
const fs = require("node:fs");
const path = require("node:path");
const env = process.env;
const overrides = {
  "@takazudo/zfb": `file:${env.ZFB_TARBALL}`,
  "@takazudo/zfb-slugify": `file:${env.ZFB_SLUGIFY_TARBALL}`,
  "@takazudo/zfb-runtime": `file:${env.ZFB_RUNTIME_TARBALL}`,
  [`@takazudo/${env.PLATFORM_PKG}`]: `file:${env.PLATFORM_TARBALL_PATH}`,
};
const workspaceYaml = [
  "minimumReleaseAge: 0",
  "overrides:",
  ...Object.entries(overrides).map(([name, spec]) => `  "${name}": "${spec}"`),
  "",
].join("\n");
fs.writeFileSync(
  path.join(env.SITE_DIR, "pnpm-workspace.yaml"),
  workspaceYaml,
);
'

(cd "$SITE_DIR" && pnpm install)
(cd "$SITE_DIR" && pnpm build)

# ── Stage 6: assert dist/ content ────────────────────────────────────────────
# Mirrors the assertion style used by scripts/smoke-clean-room.sh and
# tests/smoke/node-free/run.sh: grep rendered HTML for markers that can only
# be present if the content pipeline actually ran. `create-zfb <name>` (no
# --template) scaffolds the "basic-blog" template (crates/zfb/src/cli.rs
# default_value), whose pages/index.tsx renders an <h1> reading "basic-blog"
# and lists every post in the `blog` content collection, including the seed
# post's frontmatter title "Hello, zfb" (content/blog/hello-zfb.mdx). Both
# greps match on text, not markup — the heading carries utility CSS classes,
# so the literal tag string is not what lands in the HTML.

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

# ── Stage 7: linked-directory consumption emits the same bytes (#4231) ─────
# Builds the Stage 4 snapshot a second time, consuming the SAME packed
# package contents through symlinked directories instead of the pnpm store:
# the zfb, zfb-runtime, zfb-slugify and platform tarballs are extracted into
# a small pnpm workspace (which installs their own dependencies), and the
# site `link:`s @takazudo/zfb and @takazudo/zfb-runtime to those directories
# — the layout a monorepo or `pnpm link` consumer gets. The two dist/ trees
# must then be byte-identical: the island build token is derived from the
# resolved island graph, so where the package files live and how they were
# installed must not leak into the islands root, its chunks, or the HTML
# that carries the token.
#
# Deliberately NOT compared: the repo's own packages/* linked directly. Their
# workspace manifests export ./src/*.ts (publishConfig swaps in ./dist/*.js
# only at pack time), so that build compiles different source files and
# correctly emits different bytes and a different token. The site also stays
# outside the extracted-packages workspace: a site claimed by a
# pnpm-workspace.yaml takes the workspace as its first-party root
# (crates/zfb-types/src/first_party.rs, #1664), which by design changes
# island module labels.
LINKED_PKG_WS="$LINKED_ROOM_DIR/packages-ws"
mkdir -p "$LINKED_PKG_WS"
for spec in \
  "zfb:$ZFB_TARBALL" \
  "zfb-runtime:$ZFB_RUNTIME_TARBALL" \
  "zfb-slugify:$ZFB_SLUGIFY_TARBALL" \
  "$PLATFORM_PKG:$PLATFORM_TARBALL_PATH"; do
  name="${spec%%:*}"
  mkdir -p "$LINKED_PKG_WS/$name"
  tar -xzf "${spec#*:}" -C "$LINKED_PKG_WS/$name" --strip-components=1
done
# Each extracted package must resolve its @takazudo/* siblings to the
# extracted copies, never the registry, so the workspace links them. pnpm
# does not link a workspace member's peerDependencies, so zfb-runtime's peer
# @takazudo/zfb is satisfied the way a monorepo does it: a root dependency in
# the workspace's top-level node_modules, found by walking up from
# zfb-runtime's real path.
cat >"$LINKED_PKG_WS/package.json" <<'JSON'
{
  "name": "zfb-linked-packages",
  "private": true,
  "version": "0.0.0",
  "dependencies": { "@takazudo/zfb": "workspace:*" }
}
JSON
cat >"$LINKED_PKG_WS/pnpm-workspace.yaml" <<YAML
packages:
  - "zfb"
  - "zfb-runtime"
  - "zfb-slugify"
  - "$PLATFORM_PKG"
linkWorkspacePackages: true
minimumReleaseAge: 0
overrides:
  "@takazudo/zfb": "workspace:*"
  "@takazudo/zfb-runtime": "workspace:*"
  "@takazudo/zfb-slugify": "workspace:*"
  "@takazudo/$PLATFORM_PKG": "workspace:*"
YAML
(cd "$LINKED_PKG_WS" && pnpm install --prod)

LINKED_PKG_WS="$LINKED_PKG_WS" LINKED_SITE_DIR="$LINKED_SITE_DIR" node -e '
const fs = require("node:fs");
const path = require("node:path");
const env = process.env;
const overrides = {
  "@takazudo/zfb": `link:${path.join(env.LINKED_PKG_WS, "zfb")}`,
  "@takazudo/zfb-runtime": `link:${path.join(env.LINKED_PKG_WS, "zfb-runtime")}`,
};
const workspaceYaml = [
  "minimumReleaseAge: 0",
  "overrides:",
  ...Object.entries(overrides).map(([name, spec]) => `  "${name}": "${spec}"`),
  "",
].join("\n");
fs.writeFileSync(
  path.join(env.LINKED_SITE_DIR, "pnpm-workspace.yaml"),
  workspaceYaml,
);
'

(cd "$LINKED_SITE_DIR" && pnpm install)

# Non-vacuity: both packages must really be symlinks to the extracted
# directories, and the extracted zfb must reach the injected binary.
for name in zfb zfb-runtime; do
  link_path="$LINKED_SITE_DIR/node_modules/@takazudo/$name"
  [[ -L "$link_path" ]] || fail "linked site's @takazudo/$name is not a symlink: $link_path"
  resolved="$(cd "$link_path" && pwd -P)"
  expected="$(cd "$LINKED_PKG_WS/$name" && pwd -P)"
  [[ "$resolved" == "$expected" ]] || fail "linked site's @takazudo/$name resolves to $resolved, expected $expected"
done
LINKED_BINARY="$(cd "$LINKED_PKG_WS/zfb/node_modules/@takazudo/$PLATFORM_PKG" && pwd -P)/zfb"
cmp -s "$LINKED_BINARY" "$ZFB_SMOKE_BINARY" ||
  fail "linked zfb does not reach the injected binary (got $LINKED_BINARY)"
pass "linked site consumes the extracted packages through symlinks, with the injected binary"

(cd "$LINKED_SITE_DIR" && pnpm build)

bash scripts/assert-dist-byte-parity.sh \
  --require 'assets/islands-*.js' \
  --require 'index.html' \
  "$SITE_DIR/dist" "$LINKED_SITE_DIR/dist" ||
  fail "store-installed and linked-directory builds of the same scaffold emitted different dist/ bytes (#4231)"
pass "store-installed and linked-directory builds emit byte-identical dist/"

echo "All packed-clean-room smoke assertions passed."

# ── Optional: export the built dist for a downstream consumer (issue #2280) ──
# Opt-in only, and deliberately the LAST thing this script does: once every
# Stage-6 assertion above has passed, copy the just-built site's dist/ out to
# a caller-supplied path so another job (e.g. a showcase deploy) can reuse
# this exact build instead of re-running scaffold+build itself. Unset, this
# block is a no-op and the script's behavior is byte-identical to before.
#
# Guarded with the `:-` form, not a bare `$ZFB_SMOKE_DIST_OUT` reference:
# this script runs under `set -euo pipefail` and backs a required status
# check (Scaffold E2E), so an unguarded unset-variable reference would abort
# the job with "unbound variable" on every PR.
#
# The copy is deliberately NON-FATAL. Every assertion above has already
# passed by this point, so the smoke test's verdict is settled; this export
# only feeds a downstream, non-required showcase deploy. Letting a failed
# copy (e.g. the runner filling up — this duplicates the whole dist/) take
# down the required `Scaffold E2E` check would invert the blast radius the
# upload step's `continue-on-error` is there to protect. The downstream
# deploy job fails on its own `download-artifact` instead, which is where
# that failure belongs.
if [[ -n "${ZFB_SMOKE_DIST_OUT:-}" ]]; then
  if mkdir -p "$ZFB_SMOKE_DIST_OUT" && cp -R "$SITE_DIR/dist/." "$ZFB_SMOKE_DIST_OUT/"; then
    pass "exported dist/ to $ZFB_SMOKE_DIST_OUT"
  else
    echo "::warning::failed to export dist/ to $ZFB_SMOKE_DIST_OUT — the showcase deploy will fail on its download-artifact step, but this smoke test's assertions all passed." >&2
  fi
fi
