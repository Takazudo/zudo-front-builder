#!/usr/bin/env node

// zfb#2454: release-side contract gate. It imports every published entry from
// the complete dist, checks the exact runtime value exports and version stamp,
// then checks the closed four-directory resource layout before publish.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { ARTIFACTS } from "../crates/zfb-md-wasm/npm/scripts/build.mjs";
import {
  REQUIRED_PACKED_FILES,
  assertPackedArchive,
  assertPackedContents,
} from "../crates/zfb-md-wasm/npm/scripts/assert-packed.mjs";

const EXPECTED_EXPORTS = {
  ".": [
    "MdastAdapterError",
    "ZfbMdWasmTrapError",
    "ZfbMdWasmTrapRecoveryLimitError",
    "__forceTrapForTests",
    "__getTrapRecoveryStateForTests",
    "compile",
    "highlightCode",
    "init",
    "parseToAst",
    "renderHtml",
    "toMdastRoot",
    "version",
  ],
  "./highlight": [
    "ZfbMdWasmTrapError",
    "ZfbMdWasmTrapRecoveryLimitError",
    "__forceTrapForTests",
    "__getTrapRecoveryStateForTests",
    "highlightCode",
    "init",
    "version",
  ],
  "./render": [
    "ZfbMdWasmTrapError",
    "ZfbMdWasmTrapRecoveryLimitError",
    "__forceTrapForTests",
    "__getTrapRecoveryStateForTests",
    "init",
    "renderHtml",
    "version",
  ],
  "./parse": [
    "MdastAdapterError",
    "ZfbMdWasmTrapError",
    "ZfbMdWasmTrapRecoveryLimitError",
    "__forceTrapForTests",
    "__getTrapRecoveryStateForTests",
    "init",
    "parseToAst",
    "toMdastRoot",
    "version",
  ],
};

const ENTRY_FILES = {
  ".": "index.js",
  "./highlight": "highlight.js",
  "./render": "render.js",
  "./parse": "parse.js",
};

const SHIPPED_ARTIFACTS = ARTIFACTS.map(({ entry, dirName, outName }) => ({
  entry,
  path: `dist/${dirName}/${outName}_bg.wasm`,
}));

function usage() {
  throw new Error(
    "usage: assert-zfb-md-wasm-release.mjs --dist <dist> --package <package.json> --tarball <tgz>",
  );
}

function parseArgs(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--") || argv[index + 1] === undefined) usage();
    values[arg.slice(2)] = argv[++index];
  }
  if (!values.dist || !values.package || !values.tarball) usage();
  return values;
}

function filesUnder(path, prefix = "") {
  return readdirSync(path, { withFileTypes: true }).flatMap((entry) => {
    const relative = `${prefix}${entry.name}`;
    if (entry.isDirectory()) return filesUnder(resolve(path, entry.name), `${relative}/`);
    return entry.isFile() ? [relative] : [];
  });
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function hasExactKeys(value, expectedKeys) {
  const actualKeys = Object.keys(value).sort();
  return JSON.stringify(actualKeys) === JSON.stringify([...expectedKeys].sort());
}

/** Validate a parsed manifest against the expected four entries and their bytes. */
export function assertShippedArtifactsManifest(
  manifest,
  packageJson,
  readBytes,
  label = "manifest",
) {
  if (
    !isRecord(manifest) ||
    !hasExactKeys(manifest, ["schemaVersion", "name", "version", "artifacts"])
  ) {
    throw new Error(`${label} has an invalid top-level schema`);
  }
  if (manifest.schemaVersion !== 1) {
    throw new Error(`${label} schemaVersion is ${manifest.schemaVersion}; expected 1`);
  }
  if (
    !isRecord(packageJson) ||
    typeof packageJson.name !== "string" ||
    typeof packageJson.version !== "string"
  ) {
    throw new Error(`${label} package.json must have string name and version fields`);
  }
  if (manifest.name !== packageJson.name) {
    throw new Error(`${label} name is ${manifest.name}; expected ${packageJson.name}`);
  }
  if (manifest.version !== packageJson.version) {
    throw new Error(`${label} version is ${manifest.version}; expected ${packageJson.version}`);
  }
  if (
    !Array.isArray(manifest.artifacts) ||
    manifest.artifacts.length !== SHIPPED_ARTIFACTS.length
  ) {
    throw new Error(`${label} must contain exactly ${SHIPPED_ARTIFACTS.length} artifacts`);
  }

  const seenEntries = new Set();
  const seenPaths = new Set();
  for (const [index, expected] of SHIPPED_ARTIFACTS.entries()) {
    const actual = manifest.artifacts[index];
    if (!isRecord(actual) || !hasExactKeys(actual, ["entry", "path", "bytes", "sha256"])) {
      throw new Error(`${label} artifact ${index} has an invalid schema`);
    }
    if (typeof actual.entry !== "string" || typeof actual.path !== "string") {
      throw new Error(`${label} artifact ${index} entry and path must be strings`);
    }
    if (seenEntries.has(actual.entry)) {
      throw new Error(`${label} has duplicate artifact entry ${actual.entry}`);
    }
    if (seenPaths.has(actual.path)) {
      throw new Error(`${label} has duplicate artifact path ${actual.path}`);
    }
    seenEntries.add(actual.entry);
    seenPaths.add(actual.path);
    if (actual.entry !== expected.entry || actual.path !== expected.path) {
      throw new Error(
        `${label} artifact ${index} is ${actual.entry} → ${actual.path}; expected ${expected.entry} → ${expected.path}`,
      );
    }
    if (!Number.isSafeInteger(actual.bytes) || actual.bytes < 0) {
      throw new Error(`${label} artifact ${actual.entry} bytes must be a nonnegative safe integer`);
    }
    if (typeof actual.sha256 !== "string" || !/^[0-9a-f]{64}$/.test(actual.sha256)) {
      throw new Error(
        `${label} artifact ${actual.entry} sha256 must be 64 lowercase hex characters`,
      );
    }

    let bytes;
    try {
      bytes = readBytes(expected.path);
    } catch (error) {
      throw new Error(`${label} is missing ${expected.path}: ${error.message}`, { cause: error });
    }
    if (!(bytes instanceof Uint8Array)) {
      throw new Error(`${label} reader returned non-byte data for ${expected.path}`);
    }
    const actualBytes = bytes.byteLength;
    const actualSha256 = createHash("sha256").update(bytes).digest("hex");
    if (actual.bytes !== actualBytes) {
      throw new Error(
        `${label} artifact ${actual.entry} bytes is ${actual.bytes}; file has ${actualBytes}`,
      );
    }
    if (actual.sha256 !== actualSha256) {
      throw new Error(`${label} artifact ${actual.entry} sha256 does not match ${expected.path}`);
    }
  }

  return manifest;
}

function readJson(path, label) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new Error(`${label} could not be read as JSON: ${error.message}`, { cause: error });
  }
}

function compareShippedManifests(left, right) {
  const normalize = (manifest) => ({
    schemaVersion: manifest.schemaVersion,
    name: manifest.name,
    version: manifest.version,
    artifacts: manifest.artifacts.map(({ entry, path, bytes, sha256 }) => ({
      entry,
      path,
      bytes,
      sha256,
    })),
  });
  if (JSON.stringify(normalize(left)) !== JSON.stringify(normalize(right))) {
    throw new Error("packed shipped-artifacts.json differs from the validated dist manifest");
  }
}

async function assertEntry(label, modulePath, expected, packageVersion) {
  const module = await import(pathToFileURL(modulePath).href);
  const actual = Object.keys(module).sort();
  const expectedSorted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expectedSorted)) {
    throw new Error(
      `${label} exports changed: expected ${expectedSorted.join(", ")}; got ${actual.join(", ")}`,
    );
  }
  for (const value of expected) {
    if (typeof module[value] !== "function") {
      throw new Error(`${label} export ${value} is not callable at runtime`);
    }
  }
  await module.init();
  const actualVersion = await module.version();
  if (actualVersion !== packageVersion) {
    throw new Error(`${label} version() is ${actualVersion}; expected ${packageVersion}`);
  }
  console.log(`${label}: exact exports and version ${actualVersion}`);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const dist = resolve(args.dist);
  const packageJsonPath = resolve(args.package);
  const packageJson = readJson(packageJsonPath, "package.json");
  const packageVersion = packageJson.version;
  if (typeof packageVersion !== "string" || !packageVersion) {
    throw new Error("package.json has no version");
  }

  for (const [label, expected] of Object.entries(EXPECTED_EXPORTS)) {
    await assertEntry(label, resolve(dist, ENTRY_FILES[label]), expected, packageVersion);
  }

  const distPaths = filesUnder(dist).map((file) => `package/dist/${file}`);
  assertPackedContents(distPaths);
  for (const required of REQUIRED_PACKED_FILES) {
    if (!distPaths.includes(required)) throw new Error(`dist is missing ${required}`);
  }

  const distManifest = readJson(
    resolve(dist, "shipped-artifacts.json"),
    "dist/shipped-artifacts.json",
  );
  assertShippedArtifactsManifest(
    distManifest,
    packageJson,
    (path) => readFileSync(resolve(dist, path.slice("dist/".length))),
    "dist shipped-artifacts.json",
  );

  const tarballPath = resolve(args.tarball);
  const packedBytes = assertPackedArchive(tarballPath);
  const snapshotDir = mkdtempSync(join(tmpdir(), "zfb-md-wasm-release-"));
  try {
    execFileSync("tar", ["-xzf", tarballPath, "-C", snapshotDir], { stdio: "pipe" });
    const packageRoot = resolve(snapshotDir, "package");
    const packedPackageJson = readJson(resolve(packageRoot, "package.json"), "packed package.json");
    if (
      packedPackageJson.name !== packageJson.name ||
      packedPackageJson.version !== packageJson.version
    ) {
      throw new Error("packed package.json name/version differs from the workspace package.json");
    }
    const packedManifest = readJson(
      resolve(packageRoot, "dist/shipped-artifacts.json"),
      "packed dist/shipped-artifacts.json",
    );
    assertShippedArtifactsManifest(
      packedManifest,
      packedPackageJson,
      (path) => readFileSync(resolve(packageRoot, path)),
      "packed shipped-artifacts.json",
    );
    compareShippedManifests(distManifest, packedManifest);
  } finally {
    rmSync(snapshotDir, { recursive: true, force: true });
  }

  console.log(
    `release dist and packed tarball match the closed four-artifact manifest; tarball=${packedBytes} bytes`,
  );
}

const argument = process.argv[1];
if (argument !== undefined && import.meta.url === pathToFileURL(argument).href) {
  await main();
}
