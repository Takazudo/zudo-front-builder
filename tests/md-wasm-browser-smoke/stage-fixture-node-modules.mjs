/**
 * Materialise the fixture's one project-root node_modules view.
 *
 * zfb deliberately discovers only <project>/node_modules before constructing
 * its build shadows. The packed md-wasm package therefore has to live in that
 * view; putting it only beside the fixture leaves the shadow able to resolve
 * zfb's embedded framework dependencies but unable to resolve md-wasm.
 *
 * The md-wasm package is always linked from the lane's pnpm install,
 * which itself is asserted to come from wasm-md-artifact/zfb-md-wasm.tgz.
 * The four remaining links are zfb's ordinary framework/runtime plumbing,
 * supplied by the root pnpm install the browser CI job already performs. No
 * md-wasm source path, copied glue, or copied wasm file is involved.
 */

import { existsSync, mkdirSync, realpathSync, rmSync, symlinkSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const testRoot = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(testRoot, "..", "..");
const fixtureNodeModules = resolve(testRoot, "fixture-site", "node_modules");
const laneMdWasm = resolve(testRoot, "node_modules", "@takazudo", "zfb-md-wasm");

function requireDirectory(path, label) {
  if (!existsSync(path)) {
    throw new Error(`${label} is missing: ${path}`);
  }
  return realpathSync(path);
}

function installedDependency(owner, packageName) {
  return requireDirectory(
    resolve(repoRoot, "packages", owner, "node_modules", packageName),
    `${owner}'s installed ${packageName}`,
  );
}

function linkDirectory(target, destination) {
  mkdirSync(dirname(destination), { recursive: true });
  symlinkSync(target, destination, "dir");
}

const frameworkLinks = [
  [
    "@takazudo/zfb",
    requireDirectory(resolve(repoRoot, "packages", "zfb"), "zfb framework package"),
  ],
  [
    "@takazudo/zfb-runtime",
    requireDirectory(resolve(repoRoot, "packages", "zfb-runtime"), "zfb runtime package"),
  ],
  // Resolve through the consuming workspace packages. Another test fixture
  // may install a different version in pnpm's shared store; this fixture
  // needs the exact dependency identities used by zfb and its runtime.
  ["preact", installedDependency("zfb", "preact")],
  ["preact-render-to-string", installedDependency("zfb", "preact-render-to-string")],
  ["hono", installedDependency("zfb-runtime", "hono")],
];

rmSync(fixtureNodeModules, { recursive: true, force: true });
mkdirSync(fixtureNodeModules, { recursive: true });
linkDirectory(
  requireDirectory(laneMdWasm, "packed md-wasm package"),
  join(fixtureNodeModules, "@takazudo", "zfb-md-wasm"),
);
for (const [packageName, target] of frameworkLinks) {
  linkDirectory(target, join(fixtureNodeModules, packageName));
}

console.log(
  "fixture node_modules merges the asserted packed md-wasm package with zfb framework plumbing",
);
