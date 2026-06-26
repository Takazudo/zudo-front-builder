#!/usr/bin/env node
// Asserts that docs/package.json devDependencies.wrangler matches the expected
// pinned version. Run via `pnpm check:wrangler-pin` or in CI to catch drift.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, dirname } from "node:path";

const EXPECTED = "4.85.0";

const pkgPath = join(dirname(fileURLToPath(import.meta.url)), "..", "package.json");
const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
const actual = pkg.devDependencies?.wrangler;

if (actual !== EXPECTED) {
  console.error(
    `wrangler pin mismatch: docs/package.json has "${actual}", expected "${EXPECTED}".\n` +
      `Update the pin in check-wrangler-pin.mjs AND docs-deploy.yml / docs-pr-preview.yml.`,
  );
  process.exit(1);
}

console.log(`wrangler pin OK: ${actual}`);
