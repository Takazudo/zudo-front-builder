import { closeSync, existsSync, openSync, readFileSync, readSync, realpathSync } from "node:fs";
import { dirname, extname, join, resolve } from "node:path";

const NODE_SCRIPT_EXTENSIONS = new Set([".js", ".cjs", ".mjs"]);

function readShebang(filePath) {
  const fd = openSync(filePath, "r");
  try {
    const buffer = Buffer.alloc(128);
    const length = readSync(fd, buffer, 0, buffer.length, 0);
    const head = buffer.subarray(0, length).toString("utf8");
    return head.startsWith("#!") ? head.split("\n", 1)[0] : "";
  } finally {
    closeSync(fd);
  }
}

// Node's node_modules lookup without its NODE_PATH and global-folder fallbacks: pnpm's bin shims
// export NODE_PATH=<root>/node_modules/.pnpm/node_modules, whose hoisted `typescript` is whichever
// version won hoisting, not necessarily the one this package pins.
function findManifest(packageDir, specifier) {
  for (let dir = resolve(packageDir); ; dir = dirname(dir)) {
    const candidate = join(dir, "node_modules", specifier, "package.json");
    if (existsSync(candidate)) return realpathSync(candidate);
    if (dirname(dir) === dir) return undefined;
  }
}

/**
 * Locate the TypeScript compiler a workspace package depends on and say how to launch it.
 *
 * Discovery goes through the compiler's own `package.json` `bin.tsc` entry. TypeScript 7's
 * `exports` map no longer exposes `./bin/tsc` (so `require.resolve("typescript/bin/tsc")`
 * throws); the manifest is read from disk, so no subpath has to be exported. Resolution starts
 * from `packageDir`, so a caller can launch the result from an unrelated working directory, such
 * as an isolated packed consumer, and still run the package's pinned compiler, never a global or
 * hoisted one.
 *
 * `specifier` selects an npm alias of the compiler (for example `typescript-5.9`) for
 * consumer-floor probes; it must still resolve to a `typescript` manifest.
 */
export function resolvePackageTsc(packageDir, specifier = "typescript") {
  const manifestPath = findManifest(packageDir, specifier);
  if (manifestPath === undefined) {
    throw new Error(`${packageDir} cannot resolve ${specifier}/package.json from its node_modules`);
  }
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  if (manifest.name !== "typescript") {
    throw new Error(`${manifestPath} is "${manifest.name}", not the typescript compiler`);
  }
  const bin = typeof manifest.bin === "string" ? manifest.bin : manifest.bin?.tsc;
  if (typeof bin !== "string" || bin.length === 0) {
    throw new Error(`${manifestPath} declares no tsc bin`);
  }
  const binPath = join(dirname(manifestPath), bin);
  const runsOnNode =
    NODE_SCRIPT_EXTENSIONS.has(extname(binPath)) || /\bnode\b/.test(readShebang(binPath));
  return {
    version: manifest.version,
    binPath,
    command: runsOnNode ? process.execPath : binPath,
    args: runsOnNode ? [binPath] : [],
  };
}
