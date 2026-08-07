#!/usr/bin/env node
// Keeps every committed Wrangler pin used by CI on one trusted literal.
// Run via `pnpm check:wrangler-pin` or in CI to catch manifest/lockfile drift.
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const EXPECTED_WRANGLER_VERSION = "4.85.0";

function unquoteYamlScalar(value) {
  const trimmed = value.trim();
  if (
    (trimmed.startsWith("'") && trimmed.endsWith("'")) ||
    (trimmed.startsWith('"') && trimmed.endsWith('"'))
  ) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

function childBlock(lines, start, end, indent, name) {
  const marker = `${" ".repeat(indent)}${name}:`;
  const blockStart = lines.findIndex(
    (line, index) => index >= start && index < end && line === marker,
  );
  if (blockStart === -1) return undefined;

  let blockEnd = end;
  for (let index = blockStart + 1; index < end; index += 1) {
    const line = lines[index];
    if (line.trim() === "") continue;
    const lineIndent = line.length - line.trimStart().length;
    if (lineIndent <= indent) {
      blockEnd = index;
      break;
    }
  }
  return { start: blockStart + 1, end: blockEnd };
}

export function readLockImporterDependency(lockText, importer, dependency) {
  const lines = lockText.split(/\r?\n/);
  const importerBlock = childBlock(lines, 0, lines.length, 2, importer);
  if (!importerBlock) return undefined;
  const devDependencies = childBlock(
    lines,
    importerBlock.start,
    importerBlock.end,
    4,
    "devDependencies",
  );
  if (!devDependencies) return undefined;
  const dependencyBlock = childBlock(
    lines,
    devDependencies.start,
    devDependencies.end,
    6,
    dependency,
  );
  if (!dependencyBlock) return undefined;

  const result = {};
  for (let index = dependencyBlock.start; index < dependencyBlock.end; index += 1) {
    const match = lines[index].match(/^ {8}(specifier|version):\s*(.+)$/);
    if (match) result[match[1]] = unquoteYamlScalar(match[2]);
  }
  return result;
}

export function checkWranglerPins(rootDir) {
  const rootPackage = JSON.parse(readFileSync(join(rootDir, "package.json"), "utf8"));
  const docsPackage = JSON.parse(readFileSync(join(rootDir, "docs", "package.json"), "utf8"));
  const lockText = readFileSync(join(rootDir, "pnpm-lock.yaml"), "utf8");
  const errors = [];

  const manifests = [
    ["package.json", rootPackage.devDependencies?.wrangler],
    ["docs/package.json", docsPackage.devDependencies?.wrangler],
  ];
  for (const [label, actual] of manifests) {
    if (actual !== EXPECTED_WRANGLER_VERSION) {
      errors.push(`${label} has \"${actual}\", expected \"${EXPECTED_WRANGLER_VERSION}\"`);
    }
  }

  for (const importer of [".", "docs"]) {
    const actual = readLockImporterDependency(lockText, importer, "wrangler");
    for (const field of ["specifier", "version"]) {
      if (actual?.[field] !== EXPECTED_WRANGLER_VERSION) {
        errors.push(
          `pnpm-lock.yaml importer ${importer} wrangler ${field} is \"${actual?.[field]}\", ` +
            `expected \"${EXPECTED_WRANGLER_VERSION}\"`,
        );
      }
    }
  }

  return errors;
}

const scriptPath = fileURLToPath(import.meta.url);
if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  const rootDir = resolve(dirname(scriptPath), "..", "..");
  const errors = checkWranglerPins(rootDir);
  if (errors.length > 0) {
    console.error(
      `wrangler pin mismatch:\n- ${errors.join("\n- ")}\n` +
        `Update the exact root/docs manifest pins, both lockfile importers, and the literal ` +
        `EXPECTED_WRANGLER_VERSION together. Showcase CI executes the root lockfile-installed binary; ` +
        `never construct a package specifier from editable manifest data beside Cloudflare secrets.`,
    );
    process.exit(1);
  }

  console.log(
    `wrangler pins OK: root/docs manifests and lock importers use ${EXPECTED_WRANGLER_VERSION}`,
  );
}
