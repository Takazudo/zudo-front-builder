#!/usr/bin/env node

// zfb#2454: release-side contract gate. It imports every published entry from
// the complete dist, checks the exact runtime value exports and version stamp,
// then checks the closed four-directory resource layout before publish.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
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
  const packageJson = JSON.parse(readFileSync(resolve(args.package), "utf8"));
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
  const packedBytes = assertPackedArchive(resolve(args.tarball));
  console.log(
    `release dist has the closed four-artifact resource sets; tarball=${packedBytes} bytes`,
  );
}

await main();
