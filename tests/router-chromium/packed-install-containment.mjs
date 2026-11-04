// Path identity for the packed native-region consumer.
// macOS tmpdir() is reached through a symlink (/var -> /private/var). Node's
// ESM resolver returns the canonical path, so a lexical prefix of the mkdtemp
// path rejects a genuine isolated install. The trust anchor is the physical
// isolated directory. An escaped node_modules realpath is a failure, not a
// replacement root.
import { realpathSync, statSync } from "node:fs";
import { dirname, isAbsolute, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const SDK_ENTRY_SUFFIX = "/dist/runtime.js";
const RUNTIME_ENTRY_SUFFIX = "/dist/client-router/index.js";

function fail(message) {
  const error = new Error(message);
  error.name = "PackedInstallContainmentError";
  throw error;
}

// Both arguments are absolute physical paths. This does not resolve symlinks.
function pathIsInside(parent, child) {
  const rel = relative(parent, child);
  if (rel === "") return true;
  if (isAbsolute(rel)) return false;
  return rel !== ".." && !rel.startsWith(`..${sep}`);
}

function entryPath(entry, label) {
  if (typeof entry !== "string" || entry.length === 0) {
    fail(`${label} entry is missing`);
  }
  if (!entry.startsWith("file:")) {
    if (!isAbsolute(entry)) fail(`${label} entry is not absolute: ${entry}`);
    return entry;
  }
  let path;
  try {
    path = fileURLToPath(entry);
  } catch (error) {
    fail(`${label} entry is not a usable file URL: ${entry} (${error.message})`);
  }
  if (!isAbsolute(path)) fail(`${label} entry is not absolute: ${path}`);
  return path;
}

function realDirectory(path, label) {
  let real;
  try {
    real = realpathSync(path);
  } catch (error) {
    fail(`${label} is missing or not resolvable: ${path} (${error.code ?? error.message})`);
  }
  let stat;
  try {
    stat = statSync(real);
  } catch (error) {
    fail(`${label} is missing or not resolvable: ${real} (${error.code ?? error.message})`);
  }
  if (!stat.isDirectory()) fail(`${label} is not a directory: ${real}`);
  return real;
}

function realFile(path, label) {
  let real;
  try {
    real = realpathSync(path);
  } catch (error) {
    fail(`${label} entry file is missing: ${path} (${error.code ?? error.message})`);
  }
  let stat;
  try {
    stat = statSync(real);
  } catch (error) {
    fail(`${label} entry file is missing: ${real} (${error.code ?? error.message})`);
  }
  if (!stat.isFile()) fail(`${label} entry is not a file: ${real}`);
  return real;
}

export function assertPackedInstallContainment({ isolated, sdkEntry, runtimeEntry }) {
  if (typeof isolated !== "string" || !isAbsolute(isolated)) {
    fail(`isolated install root must be an absolute path: ${String(isolated)}`);
  }
  const isolatedRoot = realDirectory(isolated, "isolated install root");
  const nodeModulesLexical = join(isolated, "node_modules");
  const nodeModulesRoot = realDirectory(nodeModulesLexical, "isolated node_modules");
  if (!pathIsInside(isolatedRoot, nodeModulesRoot)) {
    fail(
      `node_modules escapes the isolated install\n  expected inside: ${isolatedRoot}\n  resolved node_modules: ${nodeModulesRoot}`,
    );
  }

  if (typeof sdkEntry !== "string" || !sdkEntry.endsWith(SDK_ENTRY_SUFFIX)) {
    fail(
      `unexpected sdk entry shape\n  expected suffix: ${SDK_ENTRY_SUFFIX}\n  resolved entry: ${String(sdkEntry)}`,
    );
  }
  if (typeof runtimeEntry !== "string" || !runtimeEntry.endsWith(RUNTIME_ENTRY_SUFFIX)) {
    fail(
      `unexpected runtime entry shape\n  expected suffix: ${RUNTIME_ENTRY_SUFFIX}\n  resolved entry: ${String(runtimeEntry)}`,
    );
  }

  const sdkPath = realFile(entryPath(sdkEntry, "sdk"), "sdk");
  const runtimePath = realFile(entryPath(runtimeEntry, "runtime"), "runtime");
  if (!pathIsInside(nodeModulesRoot, sdkPath)) {
    fail(
      `sdk entry escapes the isolated install\n  expected inside: ${nodeModulesRoot}\n  resolved sdk: ${sdkPath}`,
    );
  }
  if (!pathIsInside(nodeModulesRoot, runtimePath)) {
    fail(
      `runtime entry escapes the isolated install\n  expected inside: ${nodeModulesRoot}\n  resolved runtime: ${runtimePath}`,
    );
  }

  return {
    isolatedRoot,
    nodeModulesRoot,
    sdkPath,
    runtimePath,
    sdkDist: dirname(sdkPath),
    runtimeDist: dirname(dirname(runtimePath)),
  };
}
