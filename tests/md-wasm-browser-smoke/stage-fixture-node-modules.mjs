/**
 * Materialise the fixture's one project-root node_modules view.
 *
 * zfb deliberately discovers only <project>/node_modules before constructing
 * its build shadows. The packed md-wasm package therefore has to live in that
 * view; putting it only beside the fixture leaves the shadow able to resolve
 * zfb's embedded framework dependencies but unable to resolve md-wasm.
 *
 * The direct-highlight package is always linked from the lane's pnpm install,
 * which itself is asserted to come from wasm-md-artifact/zfb-md-wasm.tgz.
 * The four remaining links are zfb's ordinary framework/runtime plumbing,
 * supplied by the root pnpm install the browser CI job already performs. No
 * md-wasm source path, copied glue, or copied wasm file is involved.
 */

import {
  existsSync,
  mkdirSync,
  readdirSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const testRoot = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(testRoot, "..", "..");
const fixtureNodeModules = resolve(testRoot, "fixture-site", "node_modules");
const laneMdWasm = resolve(testRoot, "node_modules", "@takazudo", "zfb-md-wasm");
const fixtureLoader = join(fixtureNodeModules, "@zfb-fixture", "md-wasm-loader");

function requireDirectory(path, label) {
  if (!existsSync(path)) {
    throw new Error(`${label} is missing: ${path}`);
  }
  return realpathSync(path);
}

function findPnpmPackage(packageName) {
  const store = resolve(repoRoot, "node_modules", ".pnpm");
  const candidates = [
    ...new Set(
      readdirSync(store)
        .map((entry) => join(store, entry, "node_modules", packageName))
        .filter(existsSync)
        // A pnpm dependent's nested link and its direct store package can
        // name the same physical dependency. Deduplicate those links before
        // rejecting genuinely different installed versions.
        .map((path) => realpathSync(path)),
    ),
  ];
  if (candidates.length !== 1) {
    throw new Error(
      `expected exactly one root pnpm store entry for ${packageName}, found ${candidates.length}`,
    );
  }
  return candidates[0];
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
  ["preact", findPnpmPackage("preact")],
  ["preact-render-to-string", findPnpmPackage("preact-render-to-string")],
  ["hono", findPnpmPackage("hono")],
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

// The fixture's source component remains project-local so the real zfb island
// scanner can discover it. This deliberately installed adapter is its one
// package boundary: the scanner follows the adapter (to preserve the client
// graph) but, by design, does not recursively crawl a regular package's bare
// dependencies. Esbuild still resolves the adapter's user-triggered root
// import below and therefore bundles the asserted md-wasm tarball normally.
//
// Keeping that import inside a regular installed package also means the
// runtime's query-versioned glue import remains a browser-runtime concern;
// it must not be mistaken for a first-party `?raw` preprocessing request.
mkdirSync(fixtureLoader, { recursive: true });
writeFileSync(
  join(fixtureLoader, "package.json"),
  `${JSON.stringify(
    {
      name: "@zfb-fixture/md-wasm-loader",
      private: true,
      type: "module",
      exports: "./index.js",
    },
    null,
    2,
  )}\n`,
);
writeFileSync(
  join(fixtureLoader, "index.js"),
  `export const loadMdWasm = () => import("@takazudo/zfb-md-wasm");\n`,
);

console.log(
  "fixture node_modules merges the asserted packed md-wasm package with zfb framework plumbing",
);
