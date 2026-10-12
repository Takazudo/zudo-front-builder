#!/usr/bin/env bash
set -euo pipefail

# Issue #4167: build the real docs host against the WORKSPACE-built
# @takazudo/zfb-runtime (client router at the checked-out SHA) while every other
# dependency stays the published tuple (zudo-doc 6.1.1, runtime 4.3.3 otherwise).
# Everything happens in a disposable scratch git worktree of this repository;
# the real checkout is never installed into or edited.
#
#   git worktree add worktrees/scratch-ws-runtime <integration-sha>   # detached, clean
#   bash tests/docs-navigation-structure/build-workspace-runtime-host.sh \
#     --scratch "$PWD/worktrees/scratch-ws-runtime" \
#     --evidence "$HOME/cclogs/zfb/4167-ws-runtime-evidence"          # must not exist/be empty
#
# Optional: --with-override  also pins the runtime through pnpm-workspace.yaml
# `overrides` in the scratch worktree, for when zudo-doc pulls a second copy of
# the runtime transitively (the script reports duplicate copies and fails).
# Optional: --skip-initial-install  when the scratch already has node_modules.

fail() { echo "[FAIL] $*" >&2; exit 1; }

SCRATCH="" EVIDENCE="" WITH_OVERRIDE=0 SKIP_INITIAL=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --scratch) SCRATCH="${2:?--scratch needs a path}"; shift 2 ;;
    --evidence) EVIDENCE="${2:?--evidence needs a path}"; shift 2 ;;
    --with-override) WITH_OVERRIDE=1; shift ;;
    --skip-initial-install) SKIP_INITIAL=1; shift ;;
    *) fail "unknown argument: $1" ;;
  esac
done
[[ -n "$SCRATCH" ]] || fail "--scratch <disposable git worktree> is required"
[[ -n "$EVIDENCE" ]] || fail "--evidence <empty dir> is required"
[[ -d "$SCRATCH" ]] || fail "scratch path does not exist: $SCRATCH"

SCRATCH="$(cd "$SCRATCH" && pwd -P)"
[[ "$(git -C "$SCRATCH" rev-parse --show-toplevel)" == "$SCRATCH" ]] || fail "scratch must be a git worktree root: $SCRATCH"

# The first entry of `git worktree list` is always the repository's main worktree.
MAIN_WORKTREE="$(git -C "$SCRATCH" worktree list --porcelain | sed -n '1s/^worktree //p')"
MAIN_WORKTREE="$(cd "$MAIN_WORKTREE" && pwd -P)"
[[ "$SCRATCH" != "$MAIN_WORKTREE" ]] || fail "refusing to run against the main checkout: $SCRATCH"
[[ -f "$SCRATCH/docs/package.json" && -d "$SCRATCH/packages/zfb-runtime" ]] || fail "scratch does not look like the zfb repo"
[[ -z "$(git -C "$SCRATCH" status --porcelain)" ]] || fail "scratch worktree must be clean before the run"

mkdir -p "$EVIDENCE"
EVIDENCE="$(cd "$EVIDENCE" && pwd -P)"
case "$EVIDENCE/" in "$SCRATCH/"*) fail "evidence dir must be outside the scratch worktree" ;; esac
[[ -z "$(find "$EVIDENCE" -mindepth 1 -maxdepth 1 -print -quit)" ]] || fail "evidence dir must be empty: $EVIDENCE"

exec > >(tee -a "$EVIDENCE/run.log") 2>&1
cd "$SCRATCH"
SOURCE_SHA="$(git rev-parse HEAD)"
echo "Issue #4167 workspace-runtime docs host"
echo "Source SHA: $SOURCE_SHA"
echo "Scratch: $SCRATCH"
echo "Node: $(node --version)  pnpm: $(pnpm --version)"

if [[ "$SKIP_INITIAL" != 1 ]]; then
  echo '+ pnpm install --frozen-lockfile (scratch, pristine manifests, build toolchain)'
  pnpm install --frozen-lockfile
fi

for pkg in zfb-slugify zfb-runtime zfb; do
  echo "+ pnpm -C packages/$pkg build"
  pnpm -C "packages/$pkg" build
done

mkdir -p "$EVIDENCE/pack"
echo '+ pnpm -C packages/zfb-runtime pack'
PACK_OUT="$(pnpm -C packages/zfb-runtime pack --pack-destination "$EVIDENCE/pack" | tail -n 1)"
TARBALL="$PACK_OUT"
[[ -f "$TARBALL" ]] || fail "pnpm pack produced no runtime tarball: $PACK_OUT"
TARBALL_SHA256="$(node -e "console.log(require('node:crypto').createHash('sha256').update(require('node:fs').readFileSync(process.argv[1])).digest('hex'))" "$TARBALL")"
echo "Runtime tarball: $TARBALL sha256=$TARBALL_SHA256"

# Only docs/package.json changes (plus, with --with-override, the scratch's
# pnpm-workspace.yaml overrides); the published tuple stays otherwise intact.
PUBLISHED_SPEC="$(DOCS_PKG="$SCRATCH/docs/package.json" TARBALL="$TARBALL" node --input-type=module <<'NODE'
import fs from "node:fs";
const file = process.env.DOCS_PKG;
const manifest = JSON.parse(fs.readFileSync(file, "utf8"));
let section;
for (const s of ["dependencies", "devDependencies"])
  if (manifest[s]?.["@takazudo/zfb-runtime"]) section = s;
if (!section) throw new Error("docs/package.json does not depend on @takazudo/zfb-runtime");
const original = manifest[section]["@takazudo/zfb-runtime"];
manifest[section]["@takazudo/zfb-runtime"] = `file:${process.env.TARBALL}`;
fs.writeFileSync(file, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(original);
NODE
)"
echo "Published runtime spec replaced: $PUBLISHED_SPEC"
if [[ "$WITH_OVERRIDE" == 1 ]]; then
  SCRATCH="$SCRATCH" TARBALL="$TARBALL" node --input-type=module <<'NODE'
import fs from "node:fs";
const file = `${process.env.SCRATCH}/pnpm-workspace.yaml`;
const text = fs.readFileSync(file, "utf8");
if (!/^overrides:\s*$/m.test(text)) throw new Error("no overrides: block in pnpm-workspace.yaml");
fs.writeFileSync(
  file,
  text.replace(/^overrides:\s*$/m, `overrides:\n  "@takazudo/zfb-runtime": "file:${process.env.TARBALL}"`),
);
NODE
fi

echo '+ pnpm install --no-frozen-lockfile (scratch)'
pnpm install --no-frozen-lockfile

RUNTIME_LINK="$SCRATCH/docs/node_modules/@takazudo/zfb-runtime"
ls -l "$RUNTIME_LINK" | tee "$EVIDENCE/runtime-link-ls.txt"
REALPATH="$(cd "$RUNTIME_LINK" && pwd -P)"
echo "realpath: $REALPATH" | tee "$EVIDENCE/runtime-link-realpath.txt"
# The resolved graph lives in the lockfile; node_modules/.pnpm can keep an orphaned
# directory from the pristine install, so it is evidence only.
find "$SCRATCH/node_modules/.pnpm" -maxdepth 1 -name '@takazudo+zfb-runtime@*' | sort > "$EVIDENCE/runtime-store-dirs.txt"
COPIES="$(grep -oE "^  '?@takazudo/zfb-runtime@[^(':]+" "$SCRATCH/pnpm-lock.yaml" | sed -E "s/^  '?@takazudo\/zfb-runtime@//" | sort -u)"
printf '%s\n' "$COPIES" > "$EVIDENCE/runtime-copies.txt"
[[ "$(printf '%s\n' "$COPIES" | grep -c .)" == 1 ]] || fail "more than one @takazudo/zfb-runtime resolution in the scratch lockfile (see runtime-copies.txt); rerun with --with-override"
[[ "$COPIES" == file* ]] || fail "the scratch lockfile resolves @takazudo/zfb-runtime to $COPIES, not the packed tarball"
[[ "$REALPATH" == "$SCRATCH/node_modules/.pnpm/"* ]] || fail "docs runtime resolves outside the scratch pnpm store layout: $REALPATH"

echo '+ pnpm --filter docs build'
pnpm --filter docs build 2>&1 | tee "$EVIDENCE/docs-build.log"

echo '+ provenance'
ZFB_VERSION="$(pnpm -C docs exec zfb --version 2>&1 | tail -n 1)"
SOURCE_SHA="$SOURCE_SHA" TARBALL="$TARBALL" TARBALL_SHA256="$TARBALL_SHA256" \
SCRATCH="$SCRATCH" EVIDENCE="$EVIDENCE" REALPATH="$REALPATH" ZFB_VERSION="$ZFB_VERSION" \
PUBLISHED_SPEC="$PUBLISHED_SPEC" WITH_OVERRIDE="$WITH_OVERRIDE" \
node --input-type=module <<'NODE'
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";

const env = process.env;
const sha = (file) => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const walk = (dir, filter = () => true) => {
  const out = [];
  if (!fs.existsSync(dir)) return out;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full, filter));
    else if (filter(full)) out.push(full);
  }
  return out.sort();
};
const hashTree = (root) =>
  Object.fromEntries(walk(root).map((file) => [path.relative(root, file), sha(file)]));

// Published baseline: the spec the docs importer declared before the swap.
const publishedVersion = env.PUBLISHED_SPEC.replace(/^[\^~]/, "");
const pubDir = path.join(env.EVIDENCE, "published");
fs.mkdirSync(pubDir, { recursive: true });
execFileSync("npm", ["pack", `@takazudo/zfb-runtime@${publishedVersion}`, "--pack-destination", pubDir], { stdio: "inherit" });
const pubTar = fs.readdirSync(pubDir).find((f) => f.endsWith(".tgz"));
execFileSync("tar", ["-xzf", path.join(pubDir, pubTar), "-C", pubDir]);

const installedRouter = hashTree(path.join(env.REALPATH, "dist/client-router"));
const workspaceRouter = hashTree(path.join(env.SCRATCH, "packages/zfb-runtime/dist/client-router"));
const publishedRouter = hashTree(path.join(pubDir, "package/dist/client-router"));
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

const docsDist = path.join(env.SCRATCH, "docs/dist");
const routerAssets = walk(docsDist, (f) => f.endsWith(".js"))
  .filter((f) => fs.readFileSync(f, "utf8").includes("zfb:before-swap"))
  .map((f) => ({ path: path.relative(docsDist, f), sha256: sha(f) }));

const provenance = {
  issue: 4167,
  sourceSha: env.SOURCE_SHA,
  scratch: env.SCRATCH,
  zfbVersion: env.ZFB_VERSION,
  publishedRuntimeSpec: env.PUBLISHED_SPEC,
  withOverride: env.WITH_OVERRIDE === "1",
  runtimeTarball: { path: env.TARBALL, sha256: env.TARBALL_SHA256 },
  docsRuntimeRealpath: env.REALPATH,
  installedRouterSha256: installedRouter,
  workspaceDistRouterSha256: workspaceRouter,
  publishedRouterSha256: publishedRouter,
  installedEqualsWorkspaceDist: same(installedRouter, workspaceRouter),
  installedEqualsPublished: same(installedRouter, publishedRouter),
  docsDistRouterAssets: routerAssets,
};
fs.writeFileSync(path.join(env.EVIDENCE, "provenance.json"), `${JSON.stringify(provenance, null, 2)}\n`);
console.log(JSON.stringify({ ...provenance, installedRouterSha256: undefined, workspaceDistRouterSha256: undefined, publishedRouterSha256: undefined }, null, 2));
if (!provenance.installedEqualsWorkspaceDist) throw new Error("installed router bytes differ from packages/zfb-runtime/dist");
if (routerAssets.length === 0) throw new Error("no docs/dist JS asset contains the client router");
NODE

echo "[PASS] workspace-runtime host built; provenance: $EVIDENCE/provenance.json"
echo "Serve it: (cd $SCRATCH/docs && pnpm exec zfb preview --port <port>)"
