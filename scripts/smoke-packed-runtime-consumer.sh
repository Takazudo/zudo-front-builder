#!/usr/bin/env bash
set -euo pipefail

# Issue #3484: confirm that the packed public packages build a static consumer
# without a direct Hono dependency. This intentionally runs the workspace-built
# binary against an isolated pnpm install outside the repository. The
# deterministic pnpm-owner staging fixture lives in bundler.rs; this script is
# the separate, network-dependent package-boundary confirmation.
#
# The caller must build `packages/zfb` and `packages/zfb-runtime` and provide a
# zfb binary produced from the same checked-out source SHA:
#
#   pnpm -C packages/zfb build
#   pnpm -C packages/zfb-runtime build
#   cargo build -p zfb --bin zfb
#   ZFB_BINARY="$PWD/target/debug/zfb" \
#   ZFB_BINARY_SOURCE_SHA="$(git rev-parse HEAD)" \
#   ZFB_BINARY_PROVENANCE="local cargo build" \
#   bash scripts/smoke-packed-runtime-consumer.sh
#
# `cargo build` is a heavy local build and should use the machine heavy guard.
# The script itself only packs JS packages, installs their tarballs in a clean
# room, and runs the supplied binary. It retains logs and metadata under the
# artifact directory printed at the end (or ZFB_PACKED_RUNTIME_ARTIFACT_DIR).

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT_DIR"

fail() { echo "[FAIL] $*" >&2; exit 1; }
pass() { printf '[PASS] %s\n' "$*"; }

: "${ZFB_BINARY:?set ZFB_BINARY to a workspace-built zfb executable}"
: "${ZFB_BINARY_SOURCE_SHA:?set ZFB_BINARY_SOURCE_SHA to the source SHA used to build ZFB_BINARY}"
: "${ZFB_BINARY_PROVENANCE:?set ZFB_BINARY_PROVENANCE to describe how ZFB_BINARY was built}"

if [[ "$ZFB_BINARY" != /* ]]; then
  ZFB_BINARY="$ROOT_DIR/$ZFB_BINARY"
fi
[[ -f "$ZFB_BINARY" && -x "$ZFB_BINARY" ]] || fail "ZFB_BINARY is missing or not executable: $ZFB_BINARY"

SOURCE_SHA="$(git rev-parse HEAD)"
[[ "$ZFB_BINARY_SOURCE_SHA" == "$SOURCE_SHA" ]] || fail "binary source SHA $ZFB_BINARY_SOURCE_SHA does not match checkout $SOURCE_SHA"

if [[ -n "${ZFB_PACKED_RUNTIME_ARTIFACT_DIR:-}" ]]; then
  ARTIFACT_DIR="$ZFB_PACKED_RUNTIME_ARTIFACT_DIR"
  mkdir -p "$ARTIFACT_DIR"
else
  ARTIFACT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/zfb-packed-runtime-consumer.XXXXXX")"
fi
ARTIFACT_DIR="$(cd "$ARTIFACT_DIR" && pwd -P)"
case "$ARTIFACT_DIR/" in
  "$ROOT_DIR/"*) fail "artifact/scratch directory must be outside the repository: $ARTIFACT_DIR" ;;
esac

if [[ -n "$(find "$ARTIFACT_DIR" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
  fail "artifact directory must be empty before the run: $ARTIFACT_DIR"
fi

PACK_DIR="$ARTIFACT_DIR/pack"
CONSUMER_DIR="$ARTIFACT_DIR/consumer"
RUN_LOG="$ARTIFACT_DIR/run.log"
INSTALL_LOG="$ARTIFACT_DIR/pnpm-install.log"
BUILD_LOG="$ARTIFACT_DIR/zfb-build.log"
METADATA="$ARTIFACT_DIR/metadata.txt"
mkdir -p "$PACK_DIR" "$CONSUMER_DIR"
exec > >(tee -a "$RUN_LOG") 2>&1

echo "Issue #3484 packed runtime consumer regression"
echo "Source SHA: $SOURCE_SHA"
echo "Node: $(node --version)"
echo "pnpm: $(pnpm --version)"
echo "zfb binary: $ZFB_BINARY"
echo "zfb binary provenance: $ZFB_BINARY_PROVENANCE"

BINARY_SHA256="$(node --input-type=module - "$ZFB_BINARY" <<'NODE'
import { createReadStream } from "node:fs";
import { createHash } from "node:crypto";

const binary = process.argv[2];
const hash = createHash("sha256");
await new Promise((resolve, reject) => {
  createReadStream(binary).on("error", reject).pipe(hash).on("error", reject).on("finish", resolve);
});
console.log(hash.digest("hex"));
NODE
 )"
printf 'zfb binary SHA-256: %s\n' "$BINARY_SHA256"

echo '+ ZFB_BINARY --version'
if ! BINARY_VERSION="$("$ZFB_BINARY" --version 2>&1)"; then
  printf '%s\n' "$BINARY_VERSION"
  fail "workspace-built binary --version failed"
fi
printf 'zfb binary version: %s\n' "$BINARY_VERSION"

PACK_ZFB_LOG="$ARTIFACT_DIR/pack-zfb.log"
PACK_RUNTIME_LOG="$ARTIFACT_DIR/pack-zfb-runtime.log"
echo '+ pnpm -C packages/zfb pack --pack-destination <artifact>/pack'
pnpm -C packages/zfb pack --pack-destination "$PACK_DIR" >"$PACK_ZFB_LOG" 2>&1 || {
  cat "$PACK_ZFB_LOG"
  fail "pnpm pack failed for @takazudo/zfb"
}
ZFB_TARBALL="$(tail -n 1 "$PACK_ZFB_LOG")"
[[ -f "$ZFB_TARBALL" ]] || fail "pnpm pack did not produce a zfb tarball: $ZFB_TARBALL"
printf 'Packed @takazudo/zfb: %s\n' "$ZFB_TARBALL"

echo '+ pnpm -C packages/zfb-runtime pack --pack-destination <artifact>/pack'
pnpm -C packages/zfb-runtime pack --pack-destination "$PACK_DIR" >"$PACK_RUNTIME_LOG" 2>&1 || {
  cat "$PACK_RUNTIME_LOG"
  fail "pnpm pack failed for @takazudo/zfb-runtime"
}
RUNTIME_TARBALL="$(tail -n 1 "$PACK_RUNTIME_LOG")"
[[ -f "$RUNTIME_TARBALL" ]] || fail "pnpm pack did not produce a zfb-runtime tarball: $RUNTIME_TARBALL"
printf 'Packed @takazudo/zfb-runtime: %s\n' "$RUNTIME_TARBALL"

tar -xOf "$ZFB_TARBALL" package/package.json > "$ARTIFACT_DIR/zfb-package.json"
tar -xOf "$RUNTIME_TARBALL" package/package.json > "$ARTIFACT_DIR/zfb-runtime-package.json"

ZFB_VERSION="$(PACKAGE_MANIFEST="$ARTIFACT_DIR/zfb-package.json" node -p "JSON.parse(require('node:fs').readFileSync(process.env.PACKAGE_MANIFEST,'utf8')).version")"
RUNTIME_VERSION="$(PACKAGE_MANIFEST="$ARTIFACT_DIR/zfb-runtime-package.json" node -p "JSON.parse(require('node:fs').readFileSync(process.env.PACKAGE_MANIFEST,'utf8')).version")"
RUNTIME_HONO_RANGE="$(PACKAGE_MANIFEST="$ARTIFACT_DIR/zfb-runtime-package.json" node -p "JSON.parse(require('node:fs').readFileSync(process.env.PACKAGE_MANIFEST,'utf8')).dependencies?.hono ?? ''")"
[[ -n "$RUNTIME_HONO_RANGE" ]] || fail "packed zfb-runtime manifest does not declare dependencies.hono"
printf 'Packed package versions: @takazudo/zfb@%s, @takazudo/zfb-runtime@%s\n' "$ZFB_VERSION" "$RUNTIME_VERSION"
printf 'Packed runtime Hono range: %s\n' "$RUNTIME_HONO_RANGE"
[[ "$BINARY_VERSION" == *"$ZFB_VERSION"* ]] || fail "zfb binary version does not contain packed @takazudo/zfb version $ZFB_VERSION"

ZFB_TARBALL="$ZFB_TARBALL" \
RUNTIME_TARBALL="$RUNTIME_TARBALL" \
CONSUMER_DIR="$CONSUMER_DIR" \
PNPM_VERSION="$(pnpm --version)" \
node --input-type=module <<'NODE'
import fs from "node:fs";
import path from "node:path";

const env = process.env;
const manifest = {
  name: "zfb-packed-runtime-consumer",
  version: "0.0.0",
  private: true,
  type: "module",
  packageManager: `pnpm@${env.PNPM_VERSION}`,
  dependencies: {
    "@takazudo/zfb": `file:${env.ZFB_TARBALL}`,
    "@takazudo/zfb-runtime": `file:${env.RUNTIME_TARBALL}`,
  },
};
fs.writeFileSync(path.join(env.CONSUMER_DIR, "package.json"), `${JSON.stringify(manifest, null, 2)}\n`);
// This is an empty one-package workspace only to pin pnpm's resolver settings
// (isolated linker, no public hoist, and no release-age race). It has no member
// links, overrides, or Hono dependency that could mask package ownership.
fs.writeFileSync(
  path.join(env.CONSUMER_DIR, "pnpm-workspace.yaml"),
  [
    "packages: []",
    "linkWorkspacePackages: false",
    "nodeLinker: isolated",
    "hoist: false",
    "minimumReleaseAge: 0",
    "",
  ].join("\n"),
);
fs.writeFileSync(path.join(env.CONSUMER_DIR, "zfb.config.json"), '{"wind":false}\n');
fs.mkdirSync(path.join(env.CONSUMER_DIR, "pages"), { recursive: true });
fs.writeFileSync(
  path.join(env.CONSUMER_DIR, "pages/index.tsx"),
  'export default function Home() { return <main><h1>packed-runtime-hono-marker</h1></main>; }\n',
);
NODE

echo '+ (cd <external consumer> && pnpm install --no-optional --ignore-scripts)'
(cd "$CONSUMER_DIR" && pnpm install --no-optional --ignore-scripts) 2>&1 | tee "$INSTALL_LOG"

CONSUMER_DIR="$CONSUMER_DIR" RUNTIME_EVIDENCE_FILE="$ARTIFACT_DIR/runtime-resolution.json" node --input-type=module <<'NODE'
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";

const root = process.env.CONSUMER_DIR;
const rootManifest = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
for (const field of ["dependencies", "devDependencies", "optionalDependencies"]) {
  assert.equal(Object.hasOwn(rootManifest[field] ?? {}, "hono"), false, `consumer has direct Hono in ${field}`);
}
const rootHono = path.join(root, "node_modules/hono");
assert.equal(Boolean(fs.existsSync(rootHono) || fs.lstatSync(rootHono, { throwIfNoEntry: false })?.isSymbolicLink()), false,
  "strict pnpm consumer unexpectedly has root node_modules/hono");

const runtimeRoot = fs.realpathSync(path.join(root, "node_modules/@takazudo/zfb-runtime"));
const runtimeManifest = JSON.parse(fs.readFileSync(path.join(runtimeRoot, "package.json"), "utf8"));
assert.ok(runtimeManifest.dependencies?.hono, "installed runtime manifest does not declare Hono");
const ownerNodeModules = path.dirname(path.dirname(runtimeRoot));
const runtimeHonoLink = path.join(ownerNodeModules, "hono");
assert.ok(fs.existsSync(runtimeHonoLink), "Hono is not linked in the runtime package's pnpm dependency view");
const runtimeRequire = createRequire(path.join(runtimeRoot, "dist/router.js"));
const resolvedHono = fs.realpathSync(runtimeRequire.resolve("hono"));
const ownedHono = fs.realpathSync(runtimeHonoLink);
assert.ok(resolvedHono.startsWith(`${ownedHono}${path.sep}`),
  `runtime resolves Hono outside its pnpm-owned dependency link: ${resolvedHono}`);
console.log(`Runtime-owned Hono: ${resolvedHono}`);
console.log(`Runtime package: ${runtimeRoot}`);
const honoManifest = JSON.parse(fs.readFileSync(path.join(ownedHono, "package.json"), "utf8"));
fs.writeFileSync(process.env.RUNTIME_EVIDENCE_FILE, `${JSON.stringify({
  runtimeRoot,
  runtimeHonoRange: runtimeManifest.dependencies.hono,
  runtimeHonoVersion: honoManifest.version,
  runtimeHonoPath: resolvedHono,
  rootHonoPresent: false,
}, null, 2)}\n`);
NODE
pass "packed consumer has no direct/root Hono link; Hono resolves through packed zfb-runtime"

cat > "$METADATA" <<EOF
issue=3484
source_sha=$SOURCE_SHA
binary_path=$ZFB_BINARY
binary_source_sha=$ZFB_BINARY_SOURCE_SHA
binary_provenance=$ZFB_BINARY_PROVENANCE
binary_version=$BINARY_VERSION
node_version=$(node --version)
pnpm_version=$(pnpm --version)
binary_sha256=$BINARY_SHA256
zfb_package_version=$ZFB_VERSION
zfb_runtime_package_version=$RUNTIME_VERSION
zfb_runtime_hono_range=$RUNTIME_HONO_RANGE
installed_hono_version=$(PACKAGE_MANIFEST="$ARTIFACT_DIR/runtime-resolution.json" node -p "JSON.parse(require('node:fs').readFileSync(process.env.PACKAGE_MANIFEST,'utf8')).runtimeHonoVersion")
zfb_tarball=$ZFB_TARBALL
zfb_runtime_tarball=$RUNTIME_TARBALL
install_command=pnpm install --no-optional --ignore-scripts
build_command=ZFB_BINARY build
consumer_manifest=$ARTIFACT_DIR/zfb-package.json
runtime_manifest=$ARTIFACT_DIR/zfb-runtime-package.json
EOF

cat > "$ARTIFACT_DIR/snapshot-node-modules.mjs" <<'NODE'
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

const [root, output] = process.argv.slice(2);
const records = [];
async function walk(relative = "") {
  const current = path.join(root, relative);
  for (const entry of await fs.readdir(current, { withFileTypes: true })) {
    const child = path.join(relative, entry.name);
    const fullPath = path.join(root, child);
    const stat = await fs.lstat(fullPath);
    const record = { path: child.split(path.sep).join("/"), mode: stat.mode & 0o777 };
    if (stat.isSymbolicLink()) {
      record.type = "symlink";
      record.target = await fs.readlink(fullPath);
    } else if (stat.isDirectory()) {
      record.type = "directory";
      records.push(record);
      await walk(child);
      continue;
    } else if (stat.isFile()) {
      record.type = "file";
      const hash = createHash("sha256");
      const contents = await fs.readFile(fullPath);
      record.sha256 = hash.update(contents).digest("hex");
      record.size = stat.size;
    } else {
      record.type = "other";
    }
    records.push(record);
  }
}
await walk();
records.sort((a, b) => a.path.localeCompare(b.path));
await fs.writeFile(output, `${JSON.stringify(records, null, 2)}\n`);
console.log(`node_modules snapshot: ${records.length} entries -> ${output}`);
NODE

node "$ARTIFACT_DIR/snapshot-node-modules.mjs" "$CONSUMER_DIR/node_modules" "$ARTIFACT_DIR/node_modules-before.json"

echo '+ ZFB_BINARY build (cwd: external consumer)'
if ! (cd "$CONSUMER_DIR" && "$ZFB_BINARY" build) 2>&1 | tee "$BUILD_LOG"; then
  fail "workspace-built zfb failed to build the packed consumer; see $BUILD_LOG"
fi

DIST_INDEX="$CONSUMER_DIR/dist/index.html"
[[ -f "$DIST_INDEX" ]] || fail "packed consumer did not produce dist/index.html"
if ! grep -q 'packed-runtime-hono-marker' "$DIST_INDEX"; then
  fail "dist/index.html is missing the expected rendered marker"
fi
pass "static wind:false page rendered the expected HTML marker"

# Capture the installed tree after the build. The comparison covers every
# node_modules path, symlink target, mode, and regular-file digest, while
# ignoring access/modification timestamps.
node "$ARTIFACT_DIR/snapshot-node-modules.mjs" "$CONSUMER_DIR/node_modules" "$ARTIFACT_DIR/node_modules-after.json"
if ! cmp -s "$ARTIFACT_DIR/node_modules-before.json" "$ARTIFACT_DIR/node_modules-after.json"; then
  diff -u "$ARTIFACT_DIR/node_modules-before.json" "$ARTIFACT_DIR/node_modules-after.json" > "$ARTIFACT_DIR/node_modules-diff.txt" || true
  fail "zfb build changed the installed node_modules tree; see $ARTIFACT_DIR/node_modules-diff.txt"
fi
pass "zfb build left the installed node_modules tree unchanged"

if [[ -e "$CONSUMER_DIR/node_modules/hono" || -L "$CONSUMER_DIR/node_modules/hono" ]]; then
  fail "zfb build created a root node_modules/hono link"
fi
pass "root node_modules/hono remains absent after build"

echo "All packed runtime consumer assertions passed."
echo "Evidence directory: $ARTIFACT_DIR"
