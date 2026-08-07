import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  EXPECTED_WRANGLER_VERSION,
  checkWranglerPins,
  readLockImporterDependency,
} from "../../docs/scripts/check-wrangler-pin.mjs";

const temporaryDirectories = [];

function writeFixture({
  root = EXPECTED_WRANGLER_VERSION,
  docs = root,
  rootLock = root,
  docsLock = docs,
} = {}) {
  const directory = mkdtempSync(join(tmpdir(), "zfb-wrangler-pin-"));
  temporaryDirectories.push(directory);
  mkdirSync(join(directory, "docs"));
  writeFileSync(
    join(directory, "package.json"),
    JSON.stringify({ devDependencies: { wrangler: root } }),
  );
  writeFileSync(
    join(directory, "docs", "package.json"),
    JSON.stringify({ devDependencies: { wrangler: docs } }),
  );
  writeFileSync(
    join(directory, "pnpm-lock.yaml"),
    `lockfileVersion: '9.0'\n\nimporters:\n\n  .:\n    devDependencies:\n      wrangler:\n        specifier: ${rootLock}\n        version: ${rootLock}\n\n  docs:\n    devDependencies:\n      wrangler:\n        specifier: ${docsLock}\n        version: ${docsLock}\n`,
  );
  return directory;
}

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe("check-wrangler-pin", () => {
  it("accepts matching exact root/docs manifest and lockfile pins", () => {
    expect(checkWranglerPins(writeFixture())).toEqual([]);
  });

  it("reports manifest and lock importer drift independently", () => {
    const errors = checkWranglerPins(
      writeFixture({ root: "^4.85.0", docs: "4.84.0", rootLock: "4.83.0", docsLock: "4.82.0" }),
    );

    expect(errors).toHaveLength(6);
    expect(errors).toContain('package.json has "^4.85.0", expected "4.85.0"');
    expect(errors).toContain('docs/package.json has "4.84.0", expected "4.85.0"');
    expect(errors).toContain(
      'pnpm-lock.yaml importer . wrangler specifier is "4.83.0", expected "4.85.0"',
    );
    expect(errors).toContain(
      'pnpm-lock.yaml importer docs wrangler version is "4.82.0", expected "4.85.0"',
    );
  });

  it("reads quoted lockfile scalar values", () => {
    const lock = `importers:\n\n  .:\n    devDependencies:\n      wrangler:\n        specifier: '${EXPECTED_WRANGLER_VERSION}'\n        version: \"${EXPECTED_WRANGLER_VERSION}\"\n`;

    expect(readLockImporterDependency(lock, ".", "wrangler")).toEqual({
      specifier: EXPECTED_WRANGLER_VERSION,
      version: EXPECTED_WRANGLER_VERSION,
    });
  });
});
