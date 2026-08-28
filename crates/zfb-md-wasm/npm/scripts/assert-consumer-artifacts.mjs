#!/usr/bin/env node

import { statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const PACKAGE_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

// These are the emitted files imported by test/consumer-compatibility*.ts.
// Keep the list explicit: a present dist/ directory is not enough for the
// consumer declaration contract.
export const REQUIRED_CONSUMER_ARTIFACTS = Object.freeze([
  "dist/index.js",
  "dist/index.d.ts",
  "dist/highlight.js",
  "dist/highlight.d.ts",
  "dist/render.js",
  "dist/render.d.ts",
  "dist/parse.js",
  "dist/parse.d.ts",
]);

export function findMissingConsumerArtifacts(packageRoot = PACKAGE_ROOT) {
  return REQUIRED_CONSUMER_ARTIFACTS.filter((relativePath) => {
    try {
      return !statSync(resolve(packageRoot, relativePath)).isFile();
    } catch {
      return true;
    }
  });
}

export function formatMissingArtifactsError(missing) {
  return (
    `@takazudo/zfb-md-wasm test requires generated consumer artifacts; missing: ${missing.join(", ")}.\n` +
    "Build them first with `pnpm --filter @takazudo/zfb-md-wasm build`, then run its tests.\n" +
    "The ordinary workspace contract is `pnpm test:workspace`; md-wasm coverage runs in the dedicated build-then-test lane."
  );
}

function main() {
  const missing = findMissingConsumerArtifacts();
  if (missing.length === 0) return;

  console.error(formatMissingArtifactsError(missing));
  process.exitCode = 1;
}

const argument = process.argv[1];
if (argument !== undefined && import.meta.url === pathToFileURL(resolve(argument)).href) main();
